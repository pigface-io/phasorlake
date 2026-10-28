//! A zip's central directory from byte spans of the file, read by
//! `phasor_zip::zipdir` through a [`Spans`] source, with each entry's time
//! as an instant in the host's clock, in the form `peek.ndjson` keeps.

pub use phasor_zip::zipdir::{DirError, TAIL};
use phasor_zip::zipdir::{DirectoryReader, Step};
use serde::{Deserialize, Serialize};

use crate::stamp::{AEST, Stamp};

/// One entry of a zip, as its central directory records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// The name, folders separated by `/` (a `\` is read as one); a folder's ends in `/`.
    pub name: String,
    /// Uncompressed size, as declared.
    pub size: u64,
    /// Compressed size, as declared.
    pub compressed: u64,
    /// CRC-32 of the uncompressed bytes.
    pub crc32: u32,
    /// Compression method (0 stored, 8 deflated).
    pub method: u16,
    /// Modified as `YYYY-MM-DD HH:MM:SS` text, read only to derive its instant; never written.
    #[serde(default, skip_serializing)]
    pub modified: String,
    /// The modified time as an instant; none when the DOS fields are out of range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_at: Option<Stamp>,
}

impl Entry {
    /// The entry with `modified_at` derived from `modified` when it lacks it.
    pub fn derived(mut self) -> Self {
        if self.modified_at.is_none() {
            self.modified_at = Stamp::from_naive(&self.modified, AEST);
        }
        self
    }
}

impl Entry {
    /// A directory entry with its DOS time read `offset` seconds east of UTC.
    pub fn from_zip(e: phasor_zip::zipdir::Entry, offset: i32) -> Self {
        Self {
            modified: e.modified_text(),
            modified_at: e.modified_at(offset),
            name: e.name,
            size: e.size,
            compressed: e.compressed,
            crc32: e.crc32,
            method: e.method,
        }
    }
}

/// A file on disk, read by span.
pub struct OnDisk(pub std::fs::File);

impl Spans for OnDisk {
    type Error = DirError;
    async fn read(&mut self, start: u64, end: u64) -> Result<Vec<u8>, DirError> {
        use std::io::{Read as _, Seek as _, SeekFrom};
        let mut buf = vec![0u8; usize::try_from(end - start).unwrap_or(0)];
        self.0
            .seek(SeekFrom::Start(start))
            .and_then(|_| self.0.read_exact(&mut buf))
            .map_err(|_| DirError::Damaged("the file could not be read"))?;
        Ok(buf)
    }
}

/// Something byte spans of a file can be read from.
pub trait Spans {
    /// Why a read failed.
    type Error: From<DirError>;
    /// Bytes `start..end` of the file.
    fn read(&mut self, start: u64, end: u64) -> impl Future<Output = Result<Vec<u8>, Self::Error>>;
}

/// Every entry of the zip of `len` bytes, in at most three reads, with
/// times read `offset` seconds east of UTC.
pub async fn read_directory_in<S: Spans>(
    src: &mut S,
    len: u64,
    offset: i32,
) -> Result<Vec<Entry>, S::Error> {
    let mut reader = DirectoryReader::new(len);
    let mut step = reader.next();
    loop {
        match step {
            Step::Read { start, end } => {
                step = reader.feed(src.read(start, end).await?)?;
            }
            Step::Done(entries) => {
                return Ok(entries
                    .into_iter()
                    .map(|e| Entry::from_zip(e, offset))
                    .collect());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write as _};

    use super::*;

    /// A file in memory, counting its reads.
    struct Mem(Vec<u8>, usize);

    impl Spans for Mem {
        type Error = DirError;
        async fn read(&mut self, start: u64, end: u64) -> Result<Vec<u8>, DirError> {
            self.1 += 1;
            Ok(self.0[start as usize..end as usize].to_vec())
        }
    }

    fn zip(comment: &str) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let at = zip::DateTime::from_date_and_time(2026, 10, 1, 12, 34, 56).unwrap();
        let opts = zip::write::SimpleFileOptions::default().last_modified_time(at);
        w.add_directory("d/", opts).unwrap();
        w.start_file("d/x.csv", opts).unwrap();
        w.write_all(&b"x,y\n".repeat(100)).unwrap();
        w.start_file(
            "s.bin",
            opts.compression_method(zip::CompressionMethod::Stored),
        )
        .unwrap();
        w.write_all(b"abc").unwrap();
        w.set_comment(comment).unwrap();
        w.finish().unwrap().into_inner()
    }

    async fn read(bytes: Vec<u8>) -> (Result<Vec<Entry>, DirError>, usize) {
        let len = bytes.len() as u64;
        let mut m = Mem(bytes, 0);
        let r = read_directory_in(&mut m, len, AEST).await;
        (r, m.1)
    }

    #[tokio::test]
    async fn lists_a_zip_from_its_tail_with_the_time_as_an_instant() {
        let (r, reads) = read(zip("")).await;
        let e = r.unwrap();
        assert_eq!(reads, 1, "the directory is in the tail");
        let names: Vec<_> = e.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["d/", "d/x.csv", "s.bin"]);
        assert_eq!((e[1].size, e[1].method), (400, 8));
        assert!(e[1].compressed < 400);
        assert_eq!(e[1].crc32, crc32fast::hash(&b"x,y\n".repeat(100)));
        assert_eq!(e[1].modified, "2026-10-01 12:34:56");
        assert_eq!(
            e[1].modified_at.unwrap().to_string(),
            "2026-10-01T12:34:56+10:00"
        );
        assert_eq!((e[2].size, e[2].compressed, e[2].method), (3, 3, 0));
        // A record written without the instant derives it; DOS fields out
        // of range (month 0) leave none.
        let old: Entry = serde_json::from_str(
            r#"{"name":"a","size":1,"compressed":1,"crc32":1,"method":0,"modified":"2017-01-12 12:24:16"}"#,
        )
        .unwrap();
        assert!(old.modified_at.is_none());
        assert_eq!(
            old.derived().modified_at.unwrap().to_string(),
            "2017-01-12T12:24:16+10:00"
        );
        let kept = e[1].clone().derived();
        assert_eq!(kept.modified_at, e[1].modified_at);
        let zero = Entry::from_zip(
            phasor_zip::zipdir::Entry {
                name: "z".into(),
                size: 0,
                compressed: 0,
                crc32: 0,
                method: 0,
                dos_date: 0,
                dos_time: 0,
                local_header_offset: 0,
            },
            AEST,
        );
        assert_eq!(zero.modified, "1980-00-00 00:00:00");
        assert!(zero.modified_at.is_none());
    }

    #[tokio::test]
    async fn a_zip_from_another_host_is_read_in_its_clock() {
        let bytes = zip("");
        let len = bytes.len() as u64;
        let e = read_directory_in(&mut Mem(bytes, 0), len, 8 * 3600)
            .await
            .unwrap();
        assert_eq!(e[1].modified, "2026-10-01 12:34:56");
        assert_eq!(
            e[1].modified_at.unwrap().to_string(),
            "2026-10-01T12:34:56+08:00"
        );
        assert_eq!(
            Stamp::from_naive("2026-10-01 12:34:56", 8 * 3600),
            e[1].modified_at
        );
    }

    #[tokio::test]
    async fn a_long_comment_puts_the_directory_outside_the_tail() {
        let (r, reads) = read(zip(&"c".repeat(0xFFFF))).await;
        assert_eq!(r.unwrap().len(), 3);
        assert_eq!(reads, 2);
    }

    #[tokio::test]
    async fn not_a_zip_and_a_file_on_disk() {
        assert_eq!(
            read(b"just text".to_vec()).await.0,
            Err(DirError::NotAZip(9))
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.zip");
        std::fs::write(&path, zip("")).unwrap();
        let file = std::fs::File::open(&path).unwrap();
        let len = file.metadata().unwrap().len();
        let e = read_directory_in(&mut OnDisk(file), len, AEST)
            .await
            .unwrap();
        assert_eq!(e.len(), 3);
        assert_eq!(phasor_zip::zipdir::contents(&path).unwrap().len(), 2);
        // A span past the file fails as the disk.
        let mut short = OnDisk(std::fs::File::open(&path).unwrap());
        assert_eq!(
            short.read(0, len + 10).await,
            Err(DirError::Damaged("the file could not be read"))
        );
        std::fs::write(&path, b"text").unwrap();
        assert!(phasor_zip::zipdir::contents(&path).is_none());
    }

    #[tokio::test]
    async fn windows_folders_read_as_slashes() {
        let z = crate::testzip::windows(&[("M|S|t.csv", b"x"), ("M|", b"")]);
        let names: Vec<String> = read(z)
            .await
            .0
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["M/S/t.csv", "M/"]);
    }
}
