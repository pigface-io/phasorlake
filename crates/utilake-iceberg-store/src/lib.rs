//! Iceberg tables on disk: one writer, a pointer to the current metadata.
//!
//! A table is a directory. `metadata/vN.metadata.json` is the N-th version
//! of its metadata and `metadata/version-hint.text` names the current one,
//! the layout readers find on their own (`iceberg_scan('<dir>')`). Each new
//! version is written beside the old ones and the hint is swapped by rename,
//! so a reader sees the old table or the new one, never half of either.
//!
//! The census decides every schema ([`utilake_iceberg::Schema`]); the store
//! records them and never invents one. Setting a schema identical to an
//! earlier one makes that earlier schema current again, with its id, so a
//! table that goes back and forth between two column lists keeps two
//! schemas, not a new one per change. Appends write a manifest and a
//! manifest list with the library's writers and add one snapshot whose
//! summary carries the caller's properties: the run of AEMO files it holds.
//!
//! Ids and times come from the caller, so the same census writes the same
//! metadata.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use iceberg::io::FileIO;
use iceberg::spec::{
    DataContentType, DataFileBuilder, DataFileFormat, FormatVersion, Literal, ManifestFile,
    ManifestList, ManifestListWriter, ManifestWriterBuilder, Operation, Snapshot, Struct, Summary,
    TableMetadata, TableMetadataBuilder,
};
use serde_json::json;
use utilake_core::{Category, Diagnose, Diagnostic};
use utilake_iceberg::{PartitionSpec, Schema};

/// Why a table could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The library refused a file or a change.
    #[error("Iceberg refused it: {0}")]
    Iceberg(#[from] iceberg::Error),
    /// A file could not be read or written.
    #[error("The table's files could not be read or written: {0}")]
    Io(#[from] std::io::Error),
    /// Metadata that does not parse.
    #[error("The table's metadata is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// A file's partition does not fit the table's spec.
    #[error(
        "{path}: the table is partitioned by month, so every file needs its month, and only then."
    )]
    Partition {
        /// The data file.
        path: String,
    },
}

impl Diagnose for Error {
    fn diagnostic(&self) -> Diagnostic {
        let msg = self.to_string();
        match self {
            Self::Iceberg(_) => Diagnostic::error("store.iceberg_refused", Category::Store, msg),
            Self::Io(_) => Diagnostic::error("store.iceberg_io", Category::Store, msg),
            Self::Json(_) => Diagnostic::error("store.iceberg_metadata", Category::Store, msg),
            Self::Partition { path } => {
                Diagnostic::error("store.iceberg_partition", Category::Store, msg).with_file(path)
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
    /// Months since 1970-01 of its rows, for a table partitioned by month.
    pub month: Option<i32>,
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
    io: FileIO,
}

impl Store {
    /// The table in `dir`, absolute; nothing is read yet.
    pub fn at(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
            io: FileIO::new_with_fs(),
        }
    }

    fn url(path: &Path) -> String {
        format!("file://{}", path.display())
    }

    fn meta_dir(&self) -> PathBuf {
        self.dir.join("metadata")
    }

    /// Creates the table with its first schema; the metadata as written.
    pub fn create(
        &self,
        uuid: &str,
        schema: &Schema,
        spec: &PartitionSpec,
        at_ms: i64,
    ) -> Result<TableMetadata> {
        let last_partition_id = spec.fields.iter().map(|f| f.field_id).max().unwrap_or(999);
        let metadata: TableMetadata = serde_json::from_value(json!({
            "format-version": 3,
            "table-uuid": uuid,
            "location": Self::url(&self.dir),
            "last-sequence-number": 0,
            "last-updated-ms": at_ms,
            "last-column-id": schema.last_column_id,
            "schemas": [schema.to_json()],
            "current-schema-id": schema.schema_id,
            "partition-specs": [spec.to_json()],
            "default-spec-id": spec.spec_id,
            "last-partition-id": last_partition_id,
            "properties": {},
            "snapshots": [],
            "snapshot-log": [],
            "metadata-log": [],
            "sort-orders": [{"order-id": 0, "fields": []}],
            "default-sort-order-id": 0,
            "refs": {},
            "next-row-id": 0,
        }))?;
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
        let bytes = std::fs::read(self.meta_dir().join(format!("v{version}.metadata.json")))?;
        Ok((version, serde_json::from_slice(&bytes)?))
    }

    /// Writes version `version` and points the hint at it.
    fn publish(&self, metadata: &TableMetadata, version: u32) -> Result<()> {
        let dir = self.meta_dir();
        std::fs::write(
            dir.join(format!("v{version}.metadata.json")),
            serde_json::to_vec(metadata)?,
        )?;
        let tmp = dir.join(".version-hint.text");
        std::fs::write(&tmp, version.to_string())?;
        std::fs::rename(tmp, dir.join("version-hint.text"))?;
        Ok(())
    }

    /// Makes `schema` current: an earlier schema with the same columns and
    /// key becomes current again, any other is added. Returns the id the
    /// table knows it by; `at_ms` is when the change is recorded.
    pub fn set_schema(&self, schema: &Schema, at_ms: i64) -> Result<i32> {
        let (version, metadata) = self.current()?;
        let wanted: iceberg::spec::Schema = serde_json::from_value(schema.to_json())?;
        let found = metadata
            .schemas_iter()
            .find(|old| {
                let mut a: Vec<i32> = old.identifier_field_ids().collect();
                let mut b: Vec<i32> = wanted.identifier_field_ids().collect();
                a.sort_unstable();
                b.sort_unstable();
                old.as_struct() == wanted.as_struct() && a == b
            })
            .map(|old| old.schema_id());
        if found == Some(metadata.current_schema_id()) {
            return Ok(metadata.current_schema_id());
        }
        let location = Self::url(&self.meta_dir().join(format!("v{version}.metadata.json")));
        let builder = TableMetadataBuilder::new_from_metadata(metadata, Some(location));
        let builder = match found {
            Some(id) => builder.set_current_schema(id)?,
            None => builder.add_current_schema(wanted)?,
        };
        // The library stamps its own clock on a schema change; the caller's
        // time keeps the metadata reproducible.
        let mut next = serde_json::to_value(builder.build()?.metadata)?;
        next["last-updated-ms"] = json!(at_ms);
        let next: TableMetadata = serde_json::from_value(next)?;
        self.publish(&next, version + 1)?;
        Ok(next.current_schema_id())
    }

    /// Adds the files as one snapshot under the current schema.
    pub async fn append(&self, append: Append) -> Result<TableMetadata> {
        let (version, metadata) = self.current()?;
        let spec = metadata.default_partition_spec().as_ref().clone();
        let partitioned = !spec.fields().is_empty();
        let id = append.snapshot_id;
        let sequence = metadata.next_sequence_number();

        let manifest = self.meta_dir().join(format!("{id}-m0.avro"));
        let mut writer = ManifestWriterBuilder::new(
            self.io.new_output(Self::url(&manifest))?,
            Some(id),
            metadata.current_schema().clone(),
            spec,
        )
        .build_v3_data();
        let (mut rows, mut bytes) = (0u64, 0u64);
        for f in &append.files {
            let path = Self::url(&f.path);
            let partition = match (partitioned, f.month) {
                (true, Some(m)) => Struct::from_iter([Some(Literal::int(m))]),
                (false, None) => Struct::empty(),
                _ => return Err(Error::Partition { path }),
            };
            let file = DataFileBuilder::default()
                .content(DataContentType::Data)
                .file_path(path)
                .file_format(DataFileFormat::Parquet)
                .partition(partition)
                .partition_spec_id(metadata.default_partition_spec_id())
                .record_count(f.rows)
                .file_size_in_bytes(f.bytes)
                .build()
                .expect("a data file with every field set builds");
            writer.add_file(file, sequence)?;
            rows += f.rows;
            bytes += f.bytes;
        }
        let added = writer.write_manifest_file().await?;

        let parent = metadata.current_snapshot();
        let mut manifests: Vec<ManifestFile> = Vec::new();
        if let Some(p) = parent {
            let content = self.io.new_input(p.manifest_list())?.read().await?;
            manifests.extend(
                ManifestList::parse_with_version(&content, FormatVersion::V3)?.consume_entries(),
            );
        }
        manifests.push(added);

        let list = self.meta_dir().join(format!("snap-{id}.avro"));
        let first_row_id = metadata.next_row_id();
        let mut list_writer = ManifestListWriter::v3(
            self.io.new_output(Self::url(&list))?.writer().await?,
            id,
            parent.map(|p| p.snapshot_id()),
            sequence,
            Some(first_row_id),
        );
        list_writer.add_manifests(manifests.into_iter())?;
        let next_row_id = list_writer.next_row_id().unwrap_or(first_row_id);
        list_writer.close().await?;

        let mut properties: HashMap<String, String> = append.properties.into_iter().collect();
        properties.insert("added-data-files".into(), append.files.len().to_string());
        properties.insert("added-records".into(), rows.to_string());
        properties.insert("added-files-size".into(), bytes.to_string());
        let snapshot = Snapshot::builder()
            .with_manifest_list(Self::url(&list))
            .with_snapshot_id(id)
            .with_parent_snapshot_id(parent.map(|p| p.snapshot_id()))
            .with_sequence_number(sequence)
            .with_timestamp_ms(append.at_ms)
            .with_summary(Summary {
                operation: Operation::Append,
                additional_properties: properties,
            })
            .with_schema_id(metadata.current_schema_id())
            .with_row_range(first_row_id, next_row_id - first_row_id)
            .build();
        let location = Self::url(&self.meta_dir().join(format!("v{version}.metadata.json")));
        let next = TableMetadataBuilder::new_from_metadata(metadata, Some(location))
            .set_branch_snapshot(snapshot, "main")?
            .build()?
            .metadata;
        self.publish(&next, version + 1)?;
        Ok(next)
    }
}
