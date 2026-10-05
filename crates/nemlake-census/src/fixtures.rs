//! Test fixtures cut from a month of the local archive.
//!
//! The schema regeneration reads, per month, the control files, the
//! archive's README and the head of every data file: each block's I row
//! and its first few D rows. `fixtures` writes them from the files a
//! `nemlake fetch nem` archived, so the committed fixtures are excerpts of
//! real downloads, with no request of their own.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use nemlake_core::{Category, Diagnose, Diagnostic};

/// What [`write_fixtures`] wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Written {
    /// Heads, one per data file.
    pub heads: usize,
    /// Control files.
    pub ctls: usize,
    /// Whether the README was among the files.
    pub readme: bool,
    /// Files that could not be read.
    pub problems: Vec<Diagnostic>,
}

/// The head of a CSV stream: every C and I row and the first `max_rows` D
/// rows of each block, read to the end so a block late in the file is
/// kept too. The footer is kept only when nothing was dropped, so the
/// head's line count still matches it.
pub fn trim_reader(r: impl Read, max_rows: usize) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(64 << 10);
    let mut footer: Option<Vec<u8>> = None;
    let mut rows_in_block = 0usize;
    let mut saw_i = false;
    let mut dropped = false;
    let mut reader = BufReader::with_capacity(1 << 20, r);
    let mut line = Vec::with_capacity(4096);
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        if !line.ends_with(b"\n") {
            // A last line without its end is not a whole row.
            break;
        }
        match line.first() {
            Some(b'C') if line.windows(13).any(|w| w == b"END OF REPORT") => {
                footer = Some(line.clone());
            }
            Some(b'C') => out.extend_from_slice(&line),
            Some(b'I') => {
                saw_i = true;
                rows_in_block = 0;
                out.extend_from_slice(&line);
            }
            Some(b'D') if saw_i && rows_in_block < max_rows => {
                rows_in_block += 1;
                out.extend_from_slice(&line);
            }
            _ => dropped = true,
        }
    }
    if let Some(f) = footer.filter(|_| !dropped) {
        out.extend_from_slice(&f);
    }
    Ok(out)
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), Box<Diagnostic>> {
    let fail = |e: std::io::Error| {
        Box::new(
            Diagnostic::error(
                "schema.fixture_write_failed",
                Category::Schema,
                format!("The fixture {} could not be written.", path.display()),
            )
            .with_file(path.display().to_string())
            .caused_by(&e),
        )
    };
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).map_err(fail)?;
    }
    std::fs::write(path, bytes).map_err(fail)
}

fn base_name(name: &str) -> &str {
    name.rsplit(['/', '!']).next().unwrap_or(name)
}

/// Writes the fixtures of a month's archived `files` into `out`:
/// `CTL/{name}.ctl`, `README.txt`, and `heads/{zip stem}.CSV` with at most
/// `max_rows` D rows per block. A whole-month zip gives the same, from its
/// members. `tmp` takes nested zips while they are read.
pub fn write_fixtures(
    files: &[PathBuf],
    tmp: &Path,
    out: &Path,
    max_rows: usize,
) -> Result<Written, Box<Diagnostic>> {
    let mut w = Written::default();
    let other = |name: &str, r: &mut dyn Read, w: &mut Written| -> Result<(), Box<Diagnostic>> {
        let base = base_name(name);
        let lower = base.to_ascii_lowercase();
        let target = if lower.ends_with(".ctl") {
            w.ctls += 1;
            out.join("CTL").join(base)
        } else if lower == "readme.txt" {
            w.readme = true;
            out.join("README.txt")
        } else {
            return Ok(());
        };
        let mut bytes = Vec::new();
        r.read_to_end(&mut bytes).map_err(|e| {
            Box::new(
                Diagnostic::error(
                    "schema.fixture_read_failed",
                    Category::Schema,
                    format!("The archived file {name} cannot be read."),
                )
                .caused_by(&e),
            )
        })?;
        write(&target, &bytes)
    };
    for path in files {
        let name = base_name(&path.to_string_lossy()).to_owned();
        let lower = name.to_ascii_lowercase();
        if !lower.ends_with(".zip") {
            if !lower.ends_with(".ctl") && lower != "readme.txt" {
                continue;
            }
            match std::fs::File::open(path) {
                Ok(mut f) => other(&name, &mut f, &mut w)?,
                Err(e) => w.problems.push(
                    Diagnostic::error(
                        "schema.fixture_read_failed",
                        Category::Schema,
                        format!("The archived file {} cannot be read.", path.display()),
                    )
                    .caused_by(&e),
                ),
            }
            continue;
        }
        let mut heads = 0;
        let mut others: Vec<Diagnostic> = Vec::new();
        let walked = nemlake_decode::archive::walk_csv_entries::<Box<Diagnostic>>(
            path,
            tmp,
            &mut |e| {
                // The head is named for the zip the CSV came out of.
                let zip = match e.name.rsplit_once('!') {
                    Some((z, _)) => base_name(z).to_owned(),
                    None => name.clone(),
                };
                let stem = zip.strip_suffix(".zip").unwrap_or(&zip);
                let head = trim_reader(e.reader, max_rows).map_err(|err| {
                    Box::new(
                        Diagnostic::error(
                            "schema.fixture_read_failed",
                            Category::Schema,
                            format!("Entry {} of {} cannot be read.", e.name, path.display()),
                        )
                        .caused_by(&err),
                    )
                })?;
                write(&out.join("heads").join(format!("{stem}.CSV")), &head)?;
                heads += 1;
                Ok(())
            },
            &mut |e| {
                if let Err(d) = other(&e.name, e.reader, &mut w) {
                    others.push(*d);
                }
            },
        );
        w.heads += heads;
        w.problems.extend(others);
        match walked {
            Ok(_)
            | Err(nemlake_decode::archive::WalkError::Archive(
                nemlake_decode::archive::ArchiveError::NoCsv { .. },
            )) => {}
            Err(nemlake_decode::archive::WalkError::Archive(a)) => w.problems.push(a.diagnostic()),
            Err(nemlake_decode::archive::WalkError::Callback(d)) => w.problems.push(*d),
        }
    }
    Ok(w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_every_block_and_keeps_the_footer_only_when_whole() {
        let csv = b"C,x\nI,A,B,1,X\nD,A,B,1,1\nD,A,B,1,2\nD,A,B,1,3\nI,A,C,2,Y\nD,A,C,2,1\nC,\"END OF REPORT\",8\nD,partial";
        let t = String::from_utf8(trim_reader(csv.as_slice(), 2).unwrap()).unwrap();
        assert_eq!(
            t,
            "C,x\nI,A,B,1,X\nD,A,B,1,1\nD,A,B,1,2\nI,A,C,2,Y\nD,A,C,2,1\n"
        );
        let whole = b"I,A,B,1,X\r\nD,A,B,1,1\r\nC,\"END OF REPORT\",3\r\n";
        assert_eq!(trim_reader(whole.as_slice(), 5).unwrap(), whole);
    }

    #[test]
    fn unreadable_files_are_problems_and_an_unwritable_output_fails() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join("tmp");
        let bad = dir.path().join("bad.zip");
        std::fs::write(&bad, b"not a zip").unwrap();
        let missing = dir.path().join("gone").join("README.txt");
        let ctl = dir.path().join("PUBLIC_DVD_X_202401.ctl");
        std::fs::write(&ctl, b"LOAD DATA").unwrap();
        let out = dir.path().join("out");
        let w = write_fixtures(
            &[
                bad.clone(),
                missing,
                ctl.clone(),
                dir.path().join("notes.md"),
            ],
            &tmp,
            &out,
            5,
        )
        .unwrap();
        assert_eq!(w.problems.len(), 2, "{:?}", w.problems);
        assert_eq!((w.heads, w.ctls, w.readme), (0, 1, false));
        // An output path that is a file cannot take the fixtures.
        let blocked = dir.path().join("blocked");
        std::fs::write(&blocked, b"x").unwrap();
        let e = write_fixtures(&[ctl], &tmp, &blocked, 5).unwrap_err();
        assert_eq!(e.code, "schema.fixture_write_failed");
        // A zip whose CSV is fine but whose output is blocked reports it.
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            use std::io::Write as _;
            let mut z = zip::ZipWriter::new(&mut cursor);
            z.start_file("A.CSV", zip::write::SimpleFileOptions::default())
                .unwrap();
            z.write_all(b"I,A,B,1,X\nD,A,B,1,1\n").unwrap();
            z.start_file("README.txt", zip::write::SimpleFileOptions::default())
                .unwrap();
            z.write_all(b"r").unwrap();
            z.finish().unwrap();
        }
        let good = dir.path().join("PUBLIC_DVD_A_202401010000.zip");
        std::fs::write(&good, cursor.into_inner()).unwrap();
        // The walk stops at the head it cannot write.
        let w = write_fixtures(std::slice::from_ref(&good), &tmp, &blocked, 5).unwrap();
        assert_eq!(w.problems.len(), 1, "{:?}", w.problems);
        let w = write_fixtures(&[good], &tmp, &out, 5).unwrap();
        assert_eq!((w.heads, w.readme), (1, true));
        assert!(out.join("heads/PUBLIC_DVD_A_202401010000.CSV").is_file());
    }
}
