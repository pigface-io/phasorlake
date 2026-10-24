//! Which report folder of nemweb carries each block: `folders.json`, as
//! utilake-schemas exports it from its census of the report folders and
//! `just schemas-pull` copies it beside the tables. A source that reads an
//! interval of a table from the report folders rather than the monthly
//! archive asks it which folders carry the table's report pairs.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::file_model::error::SchemaFileError;

/// The file's name under a market's `pulled/` directory.
pub const FOLDERS_FILE: &str = "folders.json";

/// One report folder of one feed, as the archive keeps it: one local
/// folder whichever listing AEMO showed its files under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    /// The feed: `reports` for the NEM's report folders.
    pub feed: String,
    /// The feed's root under the host, as the archive keeps it: `Reports`.
    pub root: String,
    /// The folder under the root: `Dispatch_SCADA`.
    pub folder: String,
    /// The listings AEMO showed its files under (`ARCHIVE`, `CURRENT`);
    /// none for a folder listed once.
    #[serde(default)]
    pub listings: Vec<String>,
    /// The blocks its files carried.
    pub blocks: Vec<FolderBlock>,
}

/// One place a folder is listed on its host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Side {
    /// The listing, upper-case (`ARCHIVE`, `CURRENT`); empty for a host
    /// that lists the folder once.
    pub label: String,
    /// Its path under the host: `Reports/ARCHIVE/Dispatch_SCADA`.
    pub path: String,
}

/// One block a folder carried.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderBlock {
    /// The report type and subtype.
    pub report: [String; 2],
    /// AEMO's version of the report.
    pub version: u16,
    /// The month of the first file carrying it.
    #[serde(rename = "first-release")]
    pub first_release: String,
    /// The month of the last.
    #[serde(rename = "last-release")]
    pub last_release: String,
    /// How many months carried it.
    pub months: u64,
}

impl Folder {
    /// The folder's path as the archive keeps it, `Reports/Dispatch_SCADA`.
    pub fn path(&self) -> String {
        format!("{}/{}", self.root.trim_end_matches('/'), self.folder)
    }

    /// Where its host lists it: one side per listing, as
    /// `utilake_provenance::layout::listed_path` puts the listing back;
    /// a folder of no listing is listed at its own path, under the listing
    /// that path names, if any.
    pub fn sides(&self) -> Vec<Side> {
        use utilake_provenance::layout;
        let host =
            utilake_plain_decode::feeds::feed(&self.feed).map_or("www.nemweb.com.au", |f| f.host);
        let path = self.path();
        if self.listings.is_empty() {
            let label = layout::listing(&format!("https://{host}/{path}/_")).unwrap_or_default();
            return vec![Side { label, path }];
        }
        self.listings
            .iter()
            .map(|l| Side {
                label: l.to_ascii_uppercase(),
                path: layout::listed_path(host, &path, l),
            })
            .collect()
    }

    /// Whether the folder carried this report pair.
    pub fn carries(&self, report_type: &str, report_subtype: &str) -> bool {
        self.blocks
            .iter()
            .any(|b| b.report[0] == report_type && b.report[1] == report_subtype)
    }
}

#[derive(Deserialize)]
struct Document {
    folders: Vec<Folder>,
}

/// The folders `folders.json` at `path` lists.
pub fn read_folders(path: &Path) -> Result<Vec<Folder>, SchemaFileError> {
    let text = std::fs::read_to_string(path).map_err(|source| SchemaFileError::Read {
        path: path.display().to_string(),
        source,
    })?;
    let doc: Document = serde_json::from_str(&text).map_err(|e| SchemaFileError::Parse {
        path: path.display().to_string(),
        line: Some(e.line() as u64),
        message: e.to_string(),
    })?;
    Ok(doc.folders)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folders_file_reads_and_a_folder_knows_its_path_and_pairs() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(FOLDERS_FILE);
        std::fs::write(
            &p,
            r#"{"x-format": 3, "folders": [
                {"feed": "reports", "root": "Reports/", "folder": "Dispatch_SCADA",
                 "listings": ["ARCHIVE", "current"],
                 "blocks": [{"report": ["DISPATCH", "UNIT_SCADA"], "version": 1,
                             "first-release": "2024-01", "last-release": "2026-10", "months": 34}]},
                {"feed": "reports", "root": "Reports", "folder": "ARCHIVE/Old", "blocks": []},
                {"feed": "datafiles", "root": "public/public-data/datafiles", "folder": "x", "blocks": []}
            ]}"#,
        )
        .unwrap();
        let f = read_folders(&p).unwrap();
        assert_eq!(f.len(), 3);
        assert_eq!(f[0].path(), "Reports/Dispatch_SCADA");
        let sides = |i: usize| {
            f[i].sides()
                .into_iter()
                .map(|s| (s.label, s.path))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            sides(0),
            [
                (
                    "ARCHIVE".to_owned(),
                    "Reports/ARCHIVE/Dispatch_SCADA".to_owned()
                ),
                (
                    "CURRENT".to_owned(),
                    "Reports/CURRENT/Dispatch_SCADA".to_owned()
                )
            ]
        );
        // A folder recorded before the listings were merged is listed at
        // its own path, under the listing that path names.
        assert_eq!(
            sides(1),
            [("ARCHIVE".to_owned(), "Reports/ARCHIVE/Old".to_owned())]
        );
        assert_eq!(
            sides(2),
            [(String::new(), "public/public-data/datafiles/x".to_owned())]
        );
        assert!(f[0].carries("DISPATCH", "UNIT_SCADA"));
        assert!(!f[0].carries("DISPATCH", "PRICE"));
        assert_eq!(f[0].blocks[0].months, 34);
        std::fs::write(&p, b"not json").unwrap();
        assert!(matches!(
            read_folders(&p),
            Err(SchemaFileError::Parse { .. })
        ));
        assert!(matches!(
            read_folders(&dir.path().join("none.json")),
            Err(SchemaFileError::Read { .. })
        ));
    }
}
