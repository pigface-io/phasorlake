//! Rows that failed to parse, kept as JSON lines next to the lake so nothing
//! is silently lost.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use utilake_core::Diagnostic;

/// Appends quarantined rows to `{dir}/{table}/{source_stem}.jsonl`.
#[derive(Debug)]
pub struct QuarantineWriter {
    path: PathBuf,
    out: Option<BufWriter<File>>,
    /// Rows written.
    pub count: u64,
}

impl QuarantineWriter {
    /// A writer for one source; the file is created on the first row.
    pub fn new(dir: &Path, table: &str, source_stem: &str) -> Self {
        Self {
            path: dir
                .join(table.to_ascii_lowercase())
                .join(format!("{source_stem}.jsonl")),
            out: None,
            count: 0,
        }
    }

    /// Where the rows go.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Writes one row with its diagnostic. The file is truncated on first
    /// write so a re-ingest of the same source is idempotent.
    pub fn write(&mut self, raw: &[u8], diag: &Diagnostic) -> std::io::Result<()> {
        if self.out.is_none() {
            if let Some(p) = self.path.parent() {
                std::fs::create_dir_all(p)?;
            }
            self.out = Some(BufWriter::new(File::create(&self.path)?));
        }
        let w = self.out.as_mut().expect("just created");
        let rec = serde_json::json!({
            "line": diag.line,
            "byte_offset": diag.byte_offset,
            "code": diag.code,
            "message": diag.message,
            "column": diag.column,
            "raw": String::from_utf8_lossy(raw),
        });
        serde_json::to_writer(&mut *w, &rec)?;
        w.write_all(b"\n")?;
        self.count += 1;
        Ok(())
    }

    /// Flushes and closes.
    pub fn finish(mut self) -> std::io::Result<Option<PathBuf>> {
        if let Some(mut w) = self.out.take() {
            w.flush()?;
            return Ok(Some(self.path));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use utilake_core::Category;

    use super::*;

    #[test]
    fn writes_jsonl_lazily() {
        let dir = tempfile::tempdir().unwrap();
        let q = QuarantineWriter::new(dir.path(), "DISPATCH_UNIT_SCADA", "public_dvd_x");
        assert_eq!(
            q.path(),
            dir.path()
                .join("dispatch_unit_scada")
                .join("public_dvd_x.jsonl")
        );
        assert_eq!(q.finish().unwrap(), None, "no rows, no file");
        let mut q = QuarantineWriter::new(dir.path(), "T", "s");
        let d = Diagnostic::error("parse.float", Category::Parse, "Bad.")
            .at(7, 70)
            .with_column("X");
        q.write(b"D,1,2", &d).unwrap();
        q.write(b"D,3,\xff", &d).unwrap();
        assert_eq!(q.count, 2);
        let p = q.finish().unwrap().unwrap();
        let text = std::fs::read_to_string(p).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        let v: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(v["line"], 7);
        assert_eq!(v["code"], "parse.float");
        assert_eq!(v["raw"], "D,1,2");
        assert_eq!(v["column"], "X");
    }
}
