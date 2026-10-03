//! Sampling I rows from real files: which `(report_type, report_subtype,
//! version)` a table is published under, the exact column list of each
//! version, and simple evidence about the data (maximum string lengths).

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use nemlake_core::{Category, Diagnose, Diagnostic, Fingerprint, YearMonth, normalize_name};

/// One block's header as seen in a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampledBlock {
    /// Report type.
    pub report_type: String,
    /// Report subtype.
    pub report_subtype: String,
    /// Version.
    pub version: u16,
    /// Normalised column names, trailing empties dropped.
    pub columns: Vec<String>,
    /// Fingerprint of `columns`.
    pub fingerprint: Fingerprint,
    /// D rows seen.
    pub rows: u64,
    /// Longest value per column, in bytes, over the rows seen.
    pub max_len: Vec<u32>,
    /// Columns whose every sampled value was empty.
    pub all_null: Vec<bool>,
    /// Columns whose every non-empty sampled value parsed as a number.
    pub all_numeric: Vec<bool>,
    /// Columns whose every non-empty sampled value parsed as an MMS timestamp.
    pub all_timestamp: Vec<bool>,
    /// What the numeric values of each column need to be held exactly.
    pub digits: Vec<Digits>,
}

/// The decimal shape of the values sampled in one column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Digits {
    /// Most digits before the decimal point, leading zeros not counted.
    pub integer: u8,
    /// Most digits after the decimal point, trailing zeros not counted.
    pub fraction: u8,
    /// A value was numeric but not a plain decimal (`6E-05`): no
    /// `NUMBER(p,s)` reading of the text is exact.
    pub exponent: bool,
}

impl Digits {
    /// Folds one value in. Text that is not a number is ignored; the
    /// `all_numeric` flag already says so.
    pub fn observe(&mut self, value: &str) {
        let body = value.trim().trim_start_matches(['+', '-']);
        if body.is_empty() {
            return;
        }
        let (int, frac) = body.split_once('.').unwrap_or((body, ""));
        if !int.bytes().chain(frac.bytes()).all(|b| b.is_ascii_digit()) {
            if nemlake_core::num::parse_f64(value.trim().as_bytes()).is_ok() {
                self.exponent = true;
            }
            return;
        }
        let int = int.trim_start_matches('0').len().min(255) as u8;
        let frac = frac.trim_end_matches('0').len().min(255) as u8;
        self.integer = self.integer.max(int);
        self.fraction = self.fraction.max(frac);
    }

    /// Whether every value seen fits `NUMBER(precision, scale)` exactly.
    #[must_use]
    pub fn fits(&self, precision: u8, scale: u8) -> bool {
        !self.exponent
            && self.fraction <= scale
            && u16::from(self.integer) + u16::from(scale) <= u16::from(precision)
    }
}

/// Everything sampled from one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampledFile {
    /// Table name taken from the file name (`PUBLIC_DVD_{TABLE}_{YYYYMM}0000`),
    /// when it has that shape.
    pub table: Option<String>,
    /// Archive month from the file name, when present.
    pub month: Option<YearMonth>,
    /// Blocks in order.
    pub blocks: Vec<SampledBlock>,
}

/// Why a file could not be sampled.
#[derive(Debug, thiserror::Error)]
pub enum SampleError {
    /// Could not read.
    #[error("The file {path} could not be read for sampling.")]
    Read {
        /// The file.
        path: String,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
    /// An I row was malformed.
    #[error("The I row at {path}:{line} is malformed: {message}")]
    IRow {
        /// The file.
        path: String,
        /// Line.
        line: u64,
        /// What is wrong.
        message: String,
    },
    /// The zip had no CSV entry.
    #[error("The archive {path} has no .CSV entry to sample.")]
    NoCsv {
        /// The file.
        path: String,
    },
    /// The zip could not be opened.
    #[error("The archive {path} could not be opened: {message}")]
    Zip {
        /// The file.
        path: String,
        /// What is wrong.
        message: String,
    },
}

impl Diagnose for SampleError {
    fn diagnostic(&self) -> Diagnostic {
        let msg = self.to_string();
        match self {
            Self::Read { path, source } => {
                Diagnostic::error("schema.sample_read", Category::Schema, msg)
                    .with_file(path.clone())
                    .caused_by(source)
            }
            Self::IRow { path, line, .. } => {
                Diagnostic::error("schema.sample_irow", Category::Schema, msg)
                    .with_file(path.clone())
                    .at(*line, 0)
            }
            Self::NoCsv { path } => {
                Diagnostic::error("schema.sample_no_csv", Category::Schema, msg)
                    .with_file(path.clone())
            }
            Self::Zip { path, .. } => Diagnostic::error("schema.sample_zip", Category::Schema, msg)
                .with_file(path.clone()),
        }
    }
}

/// The table part and month of a data file's stem, in either archive
/// naming scheme. In the old scheme the table may end in a split digit
/// (`BIDPEROFFER1`), which is kept; the merger strips it. Control-file stems
/// (a six-digit month) are not data files and give `None`.
pub fn table_from_stem(stem: &str) -> Option<(String, YearMonth)> {
    if stem.contains('.') {
        return None;
    }
    let digits = stem.bytes().rev().take_while(u8::is_ascii_digit).count();
    if digits != 12 {
        return None;
    }
    let n = nemlake_core::archive_name::parse(stem)?;
    Some((n.table, n.month))
}

/// Splits a line on commas, honouring double quotes, into owned fields.
/// Slow and simple: this is the sampler, not the hot path.
fn split_fields(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if in_q && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => in_q = !in_q,
            ',' if !in_q => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// Samples the C/I/D rows of CSV text, keeping at most `max_rows` D rows per
/// block for the length statistics.
pub fn sample_reader<R: Read>(
    path: &str,
    r: R,
    max_rows: u64,
) -> Result<Vec<SampledBlock>, SampleError> {
    let mut blocks: Vec<SampledBlock> = Vec::new();
    let mut buf = String::new();
    for (idx, line) in BufReader::new(r).lines().enumerate() {
        let line_no = idx as u64 + 1;
        let line = line.map_err(|source| SampleError::Read {
            path: path.to_owned(),
            source,
        })?;
        let line = line.trim_end_matches('\r');
        match line.as_bytes().first() {
            Some(b'I') => {
                let fields = split_fields(line);
                if fields.len() < 5 {
                    return Err(SampleError::IRow {
                        path: path.to_owned(),
                        line: line_no,
                        message: format!("only {} fields", fields.len()),
                    });
                }
                let version = fields[3]
                    .trim()
                    .parse::<u16>()
                    .map_err(|_| SampleError::IRow {
                        path: path.to_owned(),
                        line: line_no,
                        message: format!("version {:?} is not an integer", fields[3]),
                    })?;
                let mut columns: Vec<String> = Vec::new();
                for f in &fields[4..] {
                    match normalize_name(f, &mut buf) {
                        Some(n) => columns.push(n.to_owned()),
                        None if f.trim().is_empty() => columns.push(String::new()),
                        None => {
                            return Err(SampleError::IRow {
                                path: path.to_owned(),
                                line: line_no,
                                message: format!("column name {f:?} is not a name"),
                            });
                        }
                    }
                }
                while columns.last().is_some_and(String::is_empty) {
                    columns.pop();
                }
                if columns.iter().any(String::is_empty) {
                    return Err(SampleError::IRow {
                        path: path.to_owned(),
                        line: line_no,
                        message: "an empty column name is not at the end".into(),
                    });
                }
                let n = columns.len();
                let rt = normalize_name(&fields[1], &mut buf)
                    .unwrap_or("")
                    .to_owned();
                let st = normalize_name(&fields[2], &mut buf)
                    .unwrap_or("")
                    .to_owned();
                blocks.push(SampledBlock {
                    report_type: rt,
                    report_subtype: st,
                    version,
                    fingerprint: Fingerprint::of_names(columns.iter().map(String::as_str)),
                    columns,
                    rows: 0,
                    max_len: vec![0; n],
                    all_null: vec![true; n],
                    all_numeric: vec![true; n],
                    all_timestamp: vec![true; n],
                    digits: vec![Digits::default(); n],
                });
            }
            Some(b'D') => {
                let Some(b) = blocks.last_mut() else { continue };
                b.rows += 1;
                if b.rows > max_rows {
                    continue;
                }
                let fields = split_fields(line);
                for (i, f) in fields.iter().skip(4).enumerate().take(b.max_len.len()) {
                    b.max_len[i] = b.max_len[i].max(f.len() as u32);
                    if !f.is_empty() {
                        b.all_null[i] = false;
                        b.digits[i].observe(f);
                        if nemlake_core::num::parse_f64(f.as_bytes()).is_err() {
                            b.all_numeric[i] = false;
                        }
                        if nemlake_core::nemtime::parse_micros(f.as_bytes(), 0).is_err() {
                            b.all_timestamp[i] = false;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(blocks)
}

/// Samples a `.CSV` or a `.zip` whose first `.CSV` entry is read.
pub fn sample_path(path: &Path, max_rows: u64) -> Result<SampledFile, SampleError> {
    let p = path.display().to_string();
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let (table, month) = match table_from_stem(stem) {
        Some((t, m)) => (Some(t), Some(m)),
        None => (None, None),
    };
    let is_zip = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("zip"));
    let blocks = if is_zip {
        let f = std::fs::File::open(path).map_err(|source| SampleError::Read {
            path: p.clone(),
            source,
        })?;
        let mut archive = zip::ZipArchive::new(f).map_err(|e| SampleError::Zip {
            path: p.clone(),
            message: e.to_string(),
        })?;
        let idx = (0..archive.len())
            .find(|&i| {
                archive
                    .by_index(i)
                    .is_ok_and(|e| e.name().to_ascii_uppercase().ends_with(".CSV"))
            })
            .ok_or_else(|| SampleError::NoCsv { path: p.clone() })?;
        let entry = archive.by_index(idx).map_err(|e| SampleError::Zip {
            path: p.clone(),
            message: e.to_string(),
        })?;
        sample_reader(&p, entry, max_rows)?
    } else {
        let f = std::fs::File::open(path).map_err(|source| SampleError::Read {
            path: p.clone(),
            source,
        })?;
        sample_reader(&p, f, max_rows)?
    };
    Ok(SampledFile {
        table,
        month,
        blocks,
    })
}

/// Samples one path: a `.CSV`, a `.zip`, or a bundle of heads written by the
/// `heads --bundle` subcommand, which yields one [`SampledFile`] per head.
pub fn sample_file(path: &Path, max_rows: u64) -> Result<Vec<SampledFile>, SampleError> {
    let is_zip = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("zip"));
    if !is_zip {
        let p = path.display().to_string();
        let text = std::fs::read(path).map_err(|source| SampleError::Read {
            path: p.clone(),
            source,
        })?;
        if text.starts_with(crate::remote::BUNDLE_MARKER.as_bytes()) {
            return sample_bundle(&p, &text, max_rows);
        }
    }
    Ok(vec![sample_path(path, max_rows)?])
}

/// Splits a bundle at its `C,NEMLAKE.HEAD,{stem}` rows and samples each part
/// as the file it was cut from.
fn sample_bundle(path: &str, text: &[u8], max_rows: u64) -> Result<Vec<SampledFile>, SampleError> {
    let marker = crate::remote::HEAD_MARKER.as_bytes();
    let mut out = Vec::new();
    let mut current: Option<(String, usize)> = None;
    let mut pos = 0usize;
    let flush = |stem: &str, body: &[u8], out: &mut Vec<SampledFile>| {
        let blocks = sample_reader(&format!("{path}!{stem}"), body, max_rows)?;
        let (table, month) = match table_from_stem(stem) {
            Some((t, m)) => (Some(t), Some(m)),
            None => (None, None),
        };
        if !blocks.is_empty() {
            out.push(SampledFile {
                table,
                month,
                blocks,
            });
        }
        Ok::<(), SampleError>(())
    };
    for line in text.split_inclusive(|&b| b == b'\n') {
        if line.starts_with(marker) {
            if let Some((stem, start)) = current.take() {
                flush(&stem, &text[start..pos], &mut out)?;
            }
            let stem = String::from_utf8_lossy(&line[marker.len()..])
                .trim()
                .to_owned();
            current = Some((stem, pos + line.len()));
        }
        pos += line.len();
    }
    if let Some((stem, start)) = current {
        flush(&stem, &text[start..], &mut out)?;
    }
    Ok(out)
}

/// Samples every `.csv`/`.CSV`/`.zip` file in a directory, non-recursively,
/// in name order. Files that are not MMS files (no `I` row) are skipped.
pub fn sample_dir(dir: &Path, max_rows: u64) -> Result<Vec<SampledFile>, SampleError> {
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .map_err(|source| SampleError::Read {
            path: dir.display().to_string(),
            source,
        })?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("csv") || e.eq_ignore_ascii_case("zip"))
        })
        .collect();
    paths.sort();
    let mut out = Vec::new();
    for p in paths {
        out.extend(
            sample_file(&p, max_rows)?
                .into_iter()
                .filter(|s| !s.blocks.is_empty()),
        );
    }
    Ok(out)
}

/// Groups sampled blocks by `(report_type, report_subtype)`, then by version.
pub type PairIndex =
    BTreeMap<(String, String), BTreeMap<u16, Vec<(Option<YearMonth>, SampledBlock)>>>;

/// Indexes a set of sampled files by report pair and version.
pub fn index_by_pair(files: &[SampledFile]) -> PairIndex {
    let mut idx: PairIndex = BTreeMap::new();
    for f in files {
        for b in &f.blocks {
            idx.entry((b.report_type.clone(), b.report_subtype.clone()))
                .or_default()
                .entry(b.version)
                .or_default()
                .push((f.month, b.clone()));
        }
    }
    idx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/csv")
    }

    #[test]
    fn stems() {
        let (t, m) = table_from_stem("PUBLIC_DVD_DISPATCH_UNIT_SCADA_202401010000").unwrap();
        assert_eq!(t, "DISPATCH_UNIT_SCADA");
        assert_eq!(m.to_string(), "2024-01");
        assert_eq!(
            table_from_stem("PUBLIC_DVD_BIDPEROFFER1_202401010000")
                .unwrap()
                .0,
            "BIDPEROFFER1"
        );
        assert_eq!(
            table_from_stem("PUBLIC_DVD_P5MIN_CONSTRAINTSOLUTION1_ALL_202401010000")
                .unwrap()
                .0,
            "P5MIN_CONSTRAINTSOLUTION1"
        );
        assert!(table_from_stem("PUBLIC_DISPATCHIS_202610011340_0000000540580033").is_none());
        assert!(table_from_stem("PUBLIC_DVD_X_2024").is_none());
        assert!(table_from_stem("PUBLIC_DVD_X_202401010001").is_none());
        assert!(table_from_stem("PUBLIC_DVD_X_2024010100000").is_none());
        assert!(table_from_stem("PUBLIC_DVD_X_202499010000").is_none());
        assert!(table_from_stem("nothing").is_none());
    }

    #[test]
    fn splits_quoted_fields() {
        assert_eq!(
            split_fields(r#"D,a,"b, c","q""q",,"#),
            ["D", "a", "b, c", "q\"q", "", ""]
        );
    }

    #[test]
    fn samples_single_block_fixture() {
        let s = sample_path(&fixtures().join("REGION_v1_head.csv"), 100).unwrap();
        assert_eq!(s.table, None, "fixture names are not archive stems");
        assert_eq!(s.blocks.len(), 1);
        let b = &s.blocks[0];
        assert_eq!(
            (b.report_type.as_str(), b.report_subtype.as_str(), b.version),
            ("MARKET_CONFIG", "REGION", 1)
        );
        assert_eq!(
            b.columns,
            ["REGIONID", "DESCRIPTION", "REGIONSTATUS", "LASTCHANGED"]
        );
        assert_eq!(b.rows, 6);
        assert_eq!(b.max_len[0], 6); // SNOWY1
        assert_eq!(b.max_len[3], 19);
        assert!(b.all_null.iter().all(|&n| !n));
        assert_eq!(b.all_timestamp, [false, false, false, true]);
        assert_eq!(b.digits.len(), 4);
        assert_eq!(b.all_numeric, [false, false, false, false]);
    }

    #[test]
    fn trailing_comma_in_i_row_is_dropped() {
        let s = sample_path(
            &fixtures().join("DISPATCHINTERCONNECTORRES_v3_head.csv"),
            100,
        )
        .unwrap();
        let b = &s.blocks[0];
        assert!(!b.columns.last().unwrap().is_empty());
        assert!(b.columns.iter().all(|c| !c.is_empty()));
        assert!(b.columns.contains(&"IMPORTGENCONID".to_owned()));
        assert_eq!(
            b.fingerprint,
            Fingerprint::of_names(b.columns.iter().map(String::as_str))
        );
    }

    #[test]
    fn samples_multi_block_and_caps_rows() {
        let s = sample_path(&fixtures().join("multi_block_dispatchis.csv"), 2).unwrap();
        assert_eq!(s.blocks.len(), 7);
        assert_eq!(s.blocks[2].report_subtype, "PRICE");
        assert_eq!(s.blocks[2].version, 5);
        assert_eq!(s.blocks[2].rows, 3, "rows are counted past the cap");
        let files = sample_dir(&fixtures(), 5).unwrap();
        assert!(files.len() >= 20);
        let idx = index_by_pair(&files);
        let price = &idx[&("DISPATCH".to_owned(), "PRICE".to_owned())];
        assert!(
            price.contains_key(&4) && price.contains_key(&5),
            "{:?}",
            price.keys()
        );
    }

    #[test]
    fn samples_zip_built_from_fixture() {
        let dir = tempfile::tempdir().unwrap();
        let zp = dir.path().join("PUBLIC_DVD_REGION_202401010000.zip");
        {
            let f = std::fs::File::create(&zp).unwrap();
            let mut w = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            w.start_file("README.txt", opts).unwrap();
            std::io::Write::write_all(&mut w, b"hi").unwrap();
            w.start_file("PUBLIC_DVD_REGION_202401010000.CSV", opts)
                .unwrap();
            std::io::Write::write_all(
                &mut w,
                &std::fs::read(fixtures().join("REGION_v1_head.csv")).unwrap(),
            )
            .unwrap();
            w.finish().unwrap();
        }
        let s = sample_path(&zp, 100).unwrap();
        assert_eq!(s.table.as_deref(), Some("REGION"));
        assert_eq!(s.month.map(|m| m.to_string()).as_deref(), Some("2024-01"));
        assert_eq!(s.blocks.len(), 1);
        // A zip with no CSV.
        let zp2 = dir.path().join("x.zip");
        {
            let f = std::fs::File::create(&zp2).unwrap();
            let mut w = zip::ZipWriter::new(f);
            w.start_file("a.txt", zip::write::SimpleFileOptions::default())
                .unwrap();
            w.finish().unwrap();
        }
        assert!(matches!(
            sample_path(&zp2, 1),
            Err(SampleError::NoCsv { .. })
        ));
        std::fs::write(dir.path().join("bad.zip"), b"not a zip").unwrap();
        assert!(matches!(
            sample_path(&dir.path().join("bad.zip"), 1),
            Err(SampleError::Zip { .. })
        ));
    }

    #[test]
    fn errors() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.csv");
        std::fs::write(&p, "I,A,B\n").unwrap();
        let e = sample_path(&p, 1).unwrap_err();
        assert!(e.to_string().contains("only 3 fields"));
        assert_eq!(e.diagnostic().code, "schema.sample_irow");
        std::fs::write(&p, "I,A,B,x,C\n").unwrap();
        assert!(
            sample_path(&p, 1)
                .unwrap_err()
                .to_string()
                .contains("not an integer")
        );
        std::fs::write(&p, "I,A,B,1,C,,D\n").unwrap();
        assert!(
            sample_path(&p, 1)
                .unwrap_err()
                .to_string()
                .contains("not at the end")
        );
        std::fs::write(&p, "I,A,B,1,\"\"\"\n").unwrap();
        assert!(
            sample_path(&p, 1)
                .unwrap_err()
                .to_string()
                .contains("is not a name")
        );
        assert!(matches!(
            sample_path(&dir.path().join("none.csv"), 1),
            Err(SampleError::Read { .. })
        ));
        assert!(sample_dir(&dir.path().join("none"), 1).is_err());
        assert_eq!(
            sample_dir(dir.path(), 1).unwrap_err().diagnostic().code,
            "schema.sample_irow"
        );
        for e in [
            SampleError::NoCsv { path: "p".into() },
            SampleError::Zip {
                path: "p".into(),
                message: "m".into(),
            },
            SampleError::Read {
                path: "p".into(),
                source: std::io::Error::other("x"),
            },
        ] {
            assert!(e.diagnostic().code.starts_with("schema."));
        }
    }

    #[test]
    fn digits_measure_what_a_decimal_needs() {
        let mut d = Digits::default();
        for v in [
            "-6425.30938268",
            "0.00000000",
            "213854.1",
            "+7",
            "007.500",
            "",
            "text",
        ] {
            d.observe(v);
        }
        assert_eq!((d.integer, d.fraction, d.exponent), (6, 8, false));
        assert!(d.fits(16, 8));
        assert!(!d.fits(16, 6), "eight fractional digits do not fit scale 6");
        assert!(!d.fits(13, 8), "six integer digits do not fit 13 - 8");
        assert!(d.fits(14, 8));
        d.observe("6E-05");
        assert!(d.exponent);
        assert!(!d.fits(38, 20));
        assert!(Digits::default().fits(1, 0), "nothing seen fits anything");
    }
}
