//! Writes every table file again in the current `schema_format`.
//!
//! The reader still reads an older format and fills in what the new one
//! adds (format 3's `[temporal]` is derived from the key and columns), so
//! writing each table back is the whole migration. Nothing a table already
//! says changes: the writer is deterministic, and a file already current is
//! left untouched.

use std::path::{Path, PathBuf};

use utilake_core::{Category, Diagnose, Diagnostic};
use utilake_registry::file_model::blob::SchemaSet;
use utilake_registry::file_model::file::{TABLE_FILE, write_table};

/// Rewrites, or with `check` only lists, every table file under `schemas`
/// whose text is not what the current format writes. Returns those paths.
pub fn run(schemas: &Path, check: bool) -> Result<Vec<PathBuf>, Box<Diagnostic>> {
    let set = SchemaSet::read_dir(schemas).map_err(|e| Box::new(e.diagnostic()))?;
    let mut changed = Vec::new();
    for t in &set.tables {
        let path = schemas
            .join(t.market.as_str())
            .join(&t.name)
            .join(TABLE_FILE);
        let now = std::fs::read_to_string(&path).map_err(|e| failed(&path, &e))?;
        let text = write_table(t);
        if now != text {
            if !check {
                std::fs::write(&path, &text).map_err(|e| failed(&path, &e))?;
            }
            changed.push(path);
        }
    }
    Ok(changed)
}

fn failed(path: &Path, e: &std::io::Error) -> Box<Diagnostic> {
    Box::new(
        Diagnostic::error(
            "schema.migrate_io",
            Category::Schema,
            format!(
                "The schema file {} could not be read or written.",
                path.display()
            ),
        )
        .with_file(path.display().to_string())
        .caused_by(e),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORMAT_2: &str = r#"schema_format = 2
name = "REGION"
report_pairs = [["DISPATCH", "REGION"]]
load_mode = "snapshot"
dedupe = "pk"
primary_key = ["REGIONID"]
next_column_id = 2

column = [
    { id = 1, name = "REGIONID", type = "string", nullable = false },
]

[[increment]]
n = 1
from = "2024-01"
aemo_versions = []
columns = [
    "REGIONID",
]
"#;

    #[test]
    fn rewrites_an_old_format_once() {
        let dir = tempfile::tempdir().unwrap();
        let table = dir.path().join("nem/REGION");
        std::fs::create_dir_all(&table).unwrap();
        std::fs::write(table.join(TABLE_FILE), FORMAT_2).unwrap();
        let listed = run(dir.path(), true).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(
            std::fs::read_to_string(table.join(TABLE_FILE)).unwrap(),
            FORMAT_2,
            "check writes nothing"
        );
        assert_eq!(run(dir.path(), false).unwrap().len(), 1);
        let text = std::fs::read_to_string(table.join(TABLE_FILE)).unwrap();
        assert!(text.contains("schema_format = 3"), "{text}");
        assert!(
            text.contains("[temporal]\norigin = \"derived\"\nentity = [\"REGIONID\"]"),
            "{text}"
        );
        assert!(!text.contains("dedupe"), "{text}");
        assert!(run(dir.path(), true).unwrap().is_empty(), "now current");
    }

    #[test]
    fn a_broken_directory_is_a_diagnostic() {
        let dir = tempfile::tempdir().unwrap();
        let table = dir.path().join("nem/BAD");
        std::fs::create_dir_all(&table).unwrap();
        std::fs::write(table.join(TABLE_FILE), "schema_format = 3\n").unwrap();
        assert!(run(dir.path(), true).is_err());
    }
}
