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

/// One report folder, under one root of one feed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    /// The feed: `reports` for the NEM's report folders.
    pub feed: String,
    /// `ARCHIVE` or `CURRENT`.
    pub label: String,
    /// The feed's root under the host: `Reports/ARCHIVE`.
    pub root: String,
    /// The folder under the root: `Dispatch_SCADA`.
    pub folder: String,
    /// The blocks its files carried.
    pub blocks: Vec<FolderBlock>,
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
    /// The folder's path under the host, `Reports/ARCHIVE/Dispatch_SCADA`.
    pub fn path(&self) -> String {
        format!("{}/{}", self.root.trim_end_matches('/'), self.folder)
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
                {"feed": "reports", "label": "ARCHIVE", "root": "Reports/ARCHIVE/", "folder": "Dispatch_SCADA",
                 "blocks": [{"report": ["DISPATCH", "UNIT_SCADA"], "version": 1,
                             "first-release": "2024-01", "last-release": "2026-10", "months": 34}]}
            ]}"#,
        )
        .unwrap();
        let f = read_folders(&p).unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].path(), "Reports/ARCHIVE/Dispatch_SCADA");
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
