//! Iceberg tables on disk: `metadata/vN.metadata.json` versions beside
//! `metadata/version-hint.text`, swapped by rename so a reader never sees half a
//! table. Schemas come from the census; ids and times from the caller. See docs/iceberg.md.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use phasor_core::{Category, Diagnose, Diagnostic};
use phasor_iceberg::Schema;
use phasor_iceberg::table::avro::AvroError;
use phasor_iceberg::table::{
    DataFile, ManifestFile, Snapshot, TableMetadata, read_manifest_list, write_manifest,
    write_manifest_list,
};

/// Why a table could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A file could not be read or written.
    #[error("The table's files could not be read or written: {0}")]
    Io(#[from] std::io::Error),
    /// Metadata that does not parse.
    #[error("The table's metadata is not valid: {0}")]
    Json(#[from] serde_json::Error),
    /// A manifest or manifest list that does not read.
    #[error("A manifest of the table is not valid: {0}")]
    Manifest(#[from] AvroError),
    /// A location the store cannot open.
    #[error("{0} is not a file on this machine (file://…)")]
    Location(String),
}

impl Diagnose for Error {
    fn diagnostic(&self) -> Diagnostic {
        let msg = self.to_string();
        match self {
            Self::Io(_) => Diagnostic::error("store.iceberg_io", Category::STORE, msg),
            Self::Json(_) => Diagnostic::error("store.iceberg_metadata", Category::STORE, msg),
            Self::Manifest(_) => Diagnostic::error("store.iceberg_manifest", Category::STORE, msg),
            Self::Location(path) => {
                Diagnostic::error("store.iceberg_location", Category::STORE, msg).with_file(path)
            }
        }
    }
}

/// What a result of the store holds.
pub type Result<T> = std::result::Result<T, Error>;

/// A data file to add.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewFile {
    /// Where it is, absolute.
    pub path: PathBuf,
    /// Its rows.
    pub rows: u64,
    /// Its size.
    pub bytes: u64,
}

/// One append: the files of a run, and what the snapshot says of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Append {
    /// The files.
    pub files: Vec<NewFile>,
    /// The snapshot's id.
    pub snapshot_id: i64,
    /// When it is committed, milliseconds since 1970.
    pub at_ms: i64,
    /// Properties for the snapshot summary.
    pub properties: BTreeMap<String, String>,
}

/// One table directory.
#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

fn url(path: &Path) -> String {
    format!("file://{}", path.display())
}

fn local(location: &str) -> Result<PathBuf> {
    location
        .strip_prefix("file://")
        .map(PathBuf::from)
        .ok_or_else(|| Error::Location(location.to_owned()))
}

impl Store {
    /// The table in `dir`, absolute; nothing is read yet.
    pub fn at(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
        }
    }

    fn meta_dir(&self) -> PathBuf {
        self.dir.join("metadata")
    }

    fn version_file(&self, version: u32) -> PathBuf {
        self.meta_dir().join(format!("v{version}.metadata.json"))
    }

    /// Creates the table with its first schema; the metadata as written.
    pub fn create(&self, uuid: &str, schema: &Schema, at_ms: i64) -> Result<TableMetadata> {
        let metadata = TableMetadata::new(uuid, &url(&self.dir), schema, at_ms);
        std::fs::create_dir_all(self.meta_dir())?;
        self.publish(&metadata, 1)?;
        Ok(metadata)
    }

    /// The current version number and metadata.
    pub fn current(&self) -> Result<(u32, TableMetadata)> {
        let hint = std::fs::read_to_string(self.meta_dir().join("version-hint.text"))?;
        let version: u32 = hint.trim().parse().map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("version-hint.text holds {hint:?}, not a version"),
            )
        })?;
        let bytes = std::fs::read(self.version_file(version))?;
        Ok((version, serde_json::from_slice(&bytes)?))
    }

    /// Writes version `version` and points the hint at it.
    fn publish(&self, metadata: &TableMetadata, version: u32) -> Result<()> {
        let dir = self.meta_dir();
        std::fs::write(self.version_file(version), serde_json::to_vec(metadata)?)?;
        let tmp = dir.join(".version-hint.text");
        std::fs::write(&tmp, version.to_string())?;
        std::fs::rename(tmp, dir.join("version-hint.text"))?;
        Ok(())
    }

    /// Makes `schema` current: an earlier schema with the same columns and key becomes
    /// current again, with its id, else it is added. Returns the schema id.
    pub fn set_schema(&self, schema: &Schema, at_ms: i64) -> Result<u32> {
        let (version, metadata) = self.current()?;
        let mut next = metadata.next(&url(&self.version_file(version)), at_ms);
        let (id, changed) = next.set_schema(schema);
        if changed {
            self.publish(&next, version + 1)?;
        }
        Ok(id)
    }

    /// Sets the table's properties at `at_ms`; the same properties again write nothing.
    pub fn set_properties(&self, properties: BTreeMap<String, String>, at_ms: i64) -> Result<()> {
        let (version, metadata) = self.current()?;
        if metadata.properties == properties {
            return Ok(());
        }
        let mut next = metadata.next(&url(&self.version_file(version)), at_ms);
        next.properties = properties;
        self.publish(&next, version + 1)
    }

    /// Adds the files as one snapshot under the current schema.
    pub fn append(&self, append: Append) -> Result<TableMetadata> {
        let (version, metadata) = self.current()?;
        let id = append.snapshot_id;
        let sequence = metadata.last_sequence_number + 1;
        let first_row_id = metadata.next_row_id;
        let files: Vec<DataFile> = append
            .files
            .iter()
            .map(|f| DataFile {
                path: url(&f.path),
                records: f.rows,
                bytes: f.bytes,
            })
            .collect();
        let rows: u64 = append.files.iter().map(|f| f.rows).sum();
        let bytes: u64 = append.files.iter().map(|f| f.bytes).sum();
        let added_rows = i64::try_from(rows).unwrap_or(i64::MAX);

        let manifest = self.meta_dir().join(format!("{id}-m0.avro"));
        let content = write_manifest(metadata.current_schema(), id, sequence, &files)?;
        std::fs::write(&manifest, &content)?;
        let parent = metadata.current_snapshot();
        let mut manifests = match parent {
            Some(p) => read_manifest_list(&std::fs::read(local(&p.manifest_list)?)?)?,
            None => Vec::new(),
        };
        manifests.push(ManifestFile {
            path: url(&manifest),
            length: i64::try_from(content.len()).unwrap_or(i64::MAX),
            sequence_number: sequence,
            min_sequence_number: sequence,
            added_snapshot_id: id,
            added_files: i32::try_from(files.len()).unwrap_or(i32::MAX),
            added_rows,
            first_row_id: Some(first_row_id),
        });
        let list = self.meta_dir().join(format!("snap-{id}.avro"));
        let parent_id = parent.map(|p| p.snapshot_id);
        let listed = write_manifest_list(id, parent_id, sequence, first_row_id, &manifests)?;
        std::fs::write(&list, listed)?;

        let mut summary = append.properties;
        summary.insert("operation".into(), "append".into());
        summary.insert("added-data-files".into(), files.len().to_string());
        summary.insert("added-records".into(), rows.to_string());
        summary.insert("added-files-size".into(), bytes.to_string());
        let mut next = metadata.next(&url(&self.version_file(version)), append.at_ms);
        next.add_snapshot(Snapshot {
            snapshot_id: id,
            parent_snapshot_id: parent_id,
            sequence_number: sequence,
            timestamp_ms: append.at_ms,
            manifest_list: url(&list),
            summary,
            schema_id: metadata.current_schema_id,
            first_row_id,
            added_rows,
        });
        self.publish(&next, version + 1)?;
        Ok(next)
    }
}
