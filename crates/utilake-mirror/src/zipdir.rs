//! A zip's central directory from byte spans of the file, without the rest
//! of it: the end record in the file's last bytes says where the directory
//! is, and the directory lists every entry's name, sizes, CRC-32, method and
//! time. Nothing is decompressed, so a zip bomb costs nothing here; sizes
//! are what the zip declares.
//!
//! [`read_directory`] drives the reads through a [`Spans`] source: ranged
//! requests to nemweb, or a file on disk.

use serde::{Deserialize, Serialize};

use crate::stamp::{AEST, Stamp};

/// The end record, the longest comment it may be followed by, and the
/// zip64 locator that may come before it.
pub const TAIL: u64 = 20 + 22 + 0xFFFF;

const EOCD: &[u8; 4] = b"PK\x05\x06";
const ZIP64_LOCATOR: &[u8; 4] = b"PK\x06\x07";
const ZIP64_EOCD: &[u8; 4] = b"PK\x06\x06";
const CENTRAL: &[u8; 4] = b"PK\x01\x02";

/// One entry of a zip, as its central directory records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// The name, folders separated by `/` (a `\` in the zip is read as
    /// one); a folder's ends in `/`.
    pub name: String,
    /// Uncompressed size, as declared.
    pub size: u64,
    /// Compressed size, as declared.
    pub compressed: u64,
    /// CRC-32 of the uncompressed bytes.
    pub crc32: u32,
    /// Compression method (0 stored, 8 deflated).
    pub method: u16,
    /// Modified, `YYYY-MM-DD HH:MM:SS`, from the DOS date and time.
    pub modified: String,
    /// The same time as an instant, AEST (AEMO packs its zips there); none
    /// when the DOS fields are out of range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_at: Option<Stamp>,
}

impl Entry {
    /// The entry with `modified_at` derived from `modified` when a record
    /// written before it was kept lacks it.
    pub fn derived(mut self) -> Self {
        if self.modified_at.is_none() {
            self.modified_at = Stamp::from_dos_text(&self.modified);
        }
        self
    }
}

/// Why a directory could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DirError {
    /// No end record in the last bytes: not a zip.
    #[error("no end-of-central-directory record in the last {0} bytes: not a zip")]
    NotAZip(u64),
    /// The records point somewhere they cannot be.
    #[error("the central directory is damaged: {0}")]
    Damaged(&'static str),
}

/// Something byte spans of a file can be read from.
pub trait Spans {
    /// Why a read failed.
    type Error: From<DirError>;
    /// Bytes `start..end` of the file.
    fn read(&mut self, start: u64, end: u64) -> impl Future<Output = Result<Vec<u8>, Self::Error>>;
}

/// Where the directory is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Found {
    /// The directory's offset and size.
    Directory(u64, u64),
    /// A zip64 end record at this offset holds them.
    Zip64(u64),
}

/// Every entry of the zip of `len` bytes, from at most three reads: its
/// tail, a zip64 end record when it lies outside the tail, and the
/// directory when it does.
pub async fn read_directory<S: Spans>(src: &mut S, len: u64) -> Result<Vec<Entry>, S::Error> {
    let tail_start = len.saturating_sub(TAIL);
    let tail = src.read(tail_start, len).await?;
    let (offset, size) = match locate(&tail)? {
        Found::Directory(o, s) => (o, s),
        Found::Zip64(at) => {
            let record = if at >= tail_start {
                tail[usize::try_from(at - tail_start).map_err(|_| damaged())?..].to_vec()
            } else {
                src.read(at, at + 56).await?
            };
            zip64_directory(&record)?
        }
    };
    let end = offset
        .checked_add(size)
        .filter(|&e| e <= len)
        .ok_or_else(damaged)?;
    let directory = if offset >= tail_start {
        let from = usize::try_from(offset - tail_start).map_err(|_| damaged())?;
        let to = usize::try_from(end - tail_start).map_err(|_| damaged())?;
        tail[from..to].to_vec()
    } else {
        src.read(offset, end).await?
    };
    Ok(entries(&directory)?)
}

fn damaged() -> DirError {
    DirError::Damaged("an offset outside the file")
}

/// The end record in `tail` and where it says the directory is.
fn locate(tail: &[u8]) -> Result<Found, DirError> {
    let at = tail
        .windows(4)
        .rposition(|w| w == EOCD)
        .filter(|&at| tail.len() >= at + 22)
        .ok_or(DirError::NotAZip(tail.len() as u64))?;
    let eocd = &tail[at..];
    let size = u64::from(u32_at(eocd, 12));
    let offset = u64::from(u32_at(eocd, 16));
    // A zip64 locator sits just before the end record.
    if at >= 20 && &tail[at - 20..at - 16] == ZIP64_LOCATOR {
        return Ok(Found::Zip64(u64_at(&tail[at - 20..], 8)));
    }
    Ok(Found::Directory(offset, size))
}

/// The directory's offset and size from a zip64 end record.
fn zip64_directory(record: &[u8]) -> Result<(u64, u64), DirError> {
    if record.len() < 56 || &record[..4] != ZIP64_EOCD {
        return Err(DirError::Damaged(
            "no zip64 end record where the locator points",
        ));
    }
    Ok((u64_at(record, 48), u64_at(record, 40)))
}

/// Every record of a central directory.
fn entries(mut cd: &[u8]) -> Result<Vec<Entry>, DirError> {
    let mut out = Vec::with_capacity(cd.len() / 80);
    // Entries packed together share a time: the last one's is kept.
    let mut last: Option<(u16, u16, String, Option<Stamp>)> = None;
    while !cd.is_empty() {
        if cd.len() < 46 || &cd[..4] != CENTRAL {
            return Err(DirError::Damaged(
                "a record that is not a central directory record",
            ));
        }
        let name_len = usize::from(u16_at(cd, 28));
        let extra_len = usize::from(u16_at(cd, 30));
        let comment_len = usize::from(u16_at(cd, 32));
        let end = 46 + name_len + extra_len + comment_len;
        if cd.len() < end {
            return Err(DirError::Damaged("a record longer than the directory"));
        }
        let mut size = u64::from(u32_at(cd, 24));
        let mut compressed = u64::from(u32_at(cd, 20));
        // Sizes too big for 32 bits are in the zip64 extra field, in order.
        let mut extra = &cd[46 + name_len..46 + name_len + extra_len];
        while extra.len() >= 4 {
            let (id, len) = (u16_at(extra, 0), usize::from(u16_at(extra, 2)));
            let body = &extra[4..(4 + len).min(extra.len())];
            if id == 1 {
                let mut fields = body.chunks_exact(8).map(|c| u64_at(c, 0));
                if size == u64::from(u32::MAX) {
                    size = fields
                        .next()
                        .ok_or(DirError::Damaged("a zip64 size missing"))?;
                }
                if compressed == u64::from(u32::MAX) {
                    compressed = fields
                        .next()
                        .ok_or(DirError::Damaged("a zip64 size missing"))?;
                }
            }
            extra = &extra[(4 + len).min(extra.len())..];
        }
        let (date, time) = (u16_at(cd, 14), u16_at(cd, 12));
        let (modified, modified_at) = match &last {
            Some((d, t, text, at)) if (*d, *t) == (date, time) => (text.clone(), *at),
            _ => {
                let (text, at) = (dos_time(date, time), dos_stamp(date, time));
                last = Some((date, time, text.clone(), at));
                (text, at)
            }
        };
        out.push(Entry {
            // Zips made on Windows separate folders with `\`.
            name: String::from_utf8_lossy(&cd[46..46 + name_len]).replace('\\', "/"),
            size,
            compressed,
            crc32: u32_at(cd, 16),
            method: u16_at(cd, 10),
            modified,
            modified_at,
        });
        cd = &cd[end..];
    }
    Ok(out)
}

/// A DOS date and time as `YYYY-MM-DD HH:MM:SS`.
fn dos_time(date: u16, time: u16) -> String {
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        1980 + (date >> 9),
        (date >> 5) & 0xF,
        date & 0x1F,
        time >> 11,
        (time >> 5) & 0x3F,
        (time & 0x1F) * 2
    )
}

/// A DOS date and time as an instant in AEST.
fn dos_stamp(date: u16, time: u16) -> Option<Stamp> {
    Stamp::at(
        1980 + i64::from(date >> 9),
        ((date >> 5) & 0xF) as u8,
        (date & 0x1F) as u8,
        (time >> 11) as u8,
        ((time >> 5) & 0x3F) as u8,
        ((time & 0x1F) * 2) as u8,
        AEST,
    )
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().expect("four bytes"))
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().expect("eight bytes"))
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
        let r = read_directory(&mut m, len).await;
        (r, m.1)
    }

    #[tokio::test]
    async fn lists_a_zip_from_its_tail() {
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
        assert_eq!(dos_time(0, 0), "1980-00-00 00:00:00");
        assert!(dos_stamp(0, 0).is_none());
        let kept = e[1].clone().derived();
        assert_eq!(kept.modified_at, e[1].modified_at);
    }

    #[tokio::test]
    async fn a_long_comment_puts_the_directory_outside_the_tail() {
        let (r, reads) = read(zip(&"c".repeat(0xFFFF))).await;
        assert_eq!(r.unwrap().len(), 3);
        assert_eq!(reads, 2);
    }

    /// A zip64 archive of one stored entry: its sizes in the zip64 extra
    /// field, the directory found through the zip64 end record.
    fn zip64(comment_len: usize) -> Vec<u8> {
        let name = b"big.bin";
        let mut z = Vec::new();
        z.extend_from_slice(b"PK\x03\x04");
        z.extend_from_slice(&[0; 26]);
        z.extend_from_slice(b"data");
        let cd_at = z.len() as u64;
        z.extend_from_slice(b"PK\x01\x02");
        z.extend_from_slice(&[45, 0, 45, 0, 0, 0, 0, 0]);
        z.extend_from_slice(&[0, 0, 0x21, 0x5A]); // time 0, date 2025-01-01
        z.extend_from_slice(&7u32.to_le_bytes()); // crc
        z.extend_from_slice(&u32::MAX.to_le_bytes()); // compressed
        z.extend_from_slice(&u32::MAX.to_le_bytes()); // size
        z.extend_from_slice(&(name.len() as u16).to_le_bytes());
        z.extend_from_slice(&20u16.to_le_bytes()); // extra
        z.extend_from_slice(&[0; 10]); // comment length, disk, attributes
        z.extend_from_slice(&u32::MAX.to_le_bytes()); // local header offset
        z.extend_from_slice(name);
        z.extend_from_slice(&1u16.to_le_bytes());
        z.extend_from_slice(&16u16.to_le_bytes());
        z.extend_from_slice(&5_000_000_000u64.to_le_bytes());
        z.extend_from_slice(&4_000_000_000u64.to_le_bytes());
        let cd_len = z.len() as u64 - cd_at;
        let z64_at = z.len() as u64;
        z.extend_from_slice(b"PK\x06\x06");
        z.extend_from_slice(&44u64.to_le_bytes());
        z.extend_from_slice(&[45, 0, 45, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        z.extend_from_slice(&1u64.to_le_bytes());
        z.extend_from_slice(&1u64.to_le_bytes());
        z.extend_from_slice(&cd_len.to_le_bytes());
        z.extend_from_slice(&cd_at.to_le_bytes());
        z.extend_from_slice(b"PK\x06\x07");
        z.extend_from_slice(&0u32.to_le_bytes());
        z.extend_from_slice(&z64_at.to_le_bytes());
        z.extend_from_slice(&1u32.to_le_bytes());
        z.extend_from_slice(b"PK\x05\x06");
        z.extend_from_slice(&[0xFF; 16]);
        z.extend_from_slice(&(comment_len as u16).to_le_bytes());
        z.extend(std::iter::repeat_n(b'c', comment_len));
        z
    }

    #[tokio::test]
    async fn zip64_sizes_and_records() {
        let (r, reads) = read(zip64(0)).await;
        let e = r.unwrap();
        assert_eq!(reads, 1);
        assert_eq!((e[0].size, e[0].compressed), (5_000_000_000, 4_000_000_000));
        assert_eq!(e[0].modified, "2025-01-01 00:00:00");
        assert_eq!(
            e[0].modified_at.unwrap().to_string(),
            "2025-01-01T00:00:00+10:00"
        );
        // The zip64 end record and the directory both outside the tail.
        let mut padded = vec![0u8; 70_000];
        let small = zip64(0xFFFF);
        let shift = padded.len() as u64;
        // Move everything along: offsets in the records grow with it.
        let mut z = small.clone();
        let cd_at = 4 + 26 + 4;
        let z64_at = z.windows(4).position(|w| w == b"PK\x06\x06").unwrap();
        let loc = z.windows(4).position(|w| w == b"PK\x06\x07").unwrap();
        z[z64_at + 48..z64_at + 56].copy_from_slice(&(cd_at as u64 + shift).to_le_bytes());
        z[loc + 8..loc + 16].copy_from_slice(&(z64_at as u64 + shift).to_le_bytes());
        padded.extend_from_slice(&z);
        let (r, reads) = read(padded).await;
        assert_eq!(r.unwrap()[0].name, "big.bin");
        assert_eq!(reads, 3);
    }

    #[tokio::test]
    async fn not_a_zip_and_damaged_directories() {
        assert_eq!(
            read(b"just text".to_vec()).await.0,
            Err(DirError::NotAZip(9))
        );
        // The end record points past the file.
        let mut z = zip("");
        let eocd = z.len() - 22;
        z[eocd + 16..eocd + 20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(read(z).await.0, Err(DirError::Damaged(_))));
        // A record that is not one.
        let mut z = zip("");
        let cd = z.windows(4).position(|w| w == b"PK\x01\x02").unwrap();
        z[cd] = b'X';
        assert!(matches!(read(z).await.0, Err(DirError::Damaged(_))));
        // A record longer than the directory.
        let mut z = zip("");
        let cd = z.windows(4).rposition(|w| w == b"PK\x01\x02").unwrap();
        z[cd + 28] = 0xFF;
        assert!(matches!(read(z).await.0, Err(DirError::Damaged(_))));
        // A locator pointing at no zip64 end record.
        let mut z = zip64(0);
        let z64 = z.windows(4).position(|w| w == b"PK\x06\x06").unwrap();
        z[z64] = b'X';
        assert!(matches!(read(z).await.0, Err(DirError::Damaged(_))));
        // Sizes marked as zip64 with no zip64 field for them.
        let mut z = zip64(0);
        let extra = z.windows(4).position(|w| w == b"big.").unwrap() + 7;
        z[extra..extra + 2].copy_from_slice(&9u16.to_le_bytes());
        let e = read(z).await.0.unwrap();
        assert_eq!(
            e[0].size,
            u64::from(u32::MAX),
            "an unknown extra field is skipped"
        );
        let mut z = zip64(0);
        z[extra + 2..extra + 4].copy_from_slice(&0u16.to_le_bytes());
        assert!(matches!(read(z).await.0, Err(DirError::Damaged(_))));
        let mut z = zip64(0);
        z[extra + 2..extra + 4].copy_from_slice(&8u16.to_le_bytes());
        assert!(matches!(read(z).await.0, Err(DirError::Damaged(_))));
        // Only one of the sizes too big for 32 bits: it takes the field.
        let cd = zip64(0)
            .windows(4)
            .position(|w| w == b"PK\x01\x02")
            .unwrap();
        let mut z = zip64(0);
        z[cd + 20..cd + 24].copy_from_slice(&9u32.to_le_bytes());
        let e = read(z).await.0.unwrap();
        assert_eq!((e[0].size, e[0].compressed), (5_000_000_000, 9));
        let mut z = zip64(0);
        z[cd + 24..cd + 28].copy_from_slice(&9u32.to_le_bytes());
        let e = read(z).await.0.unwrap();
        assert_eq!((e[0].size, e[0].compressed), (9, 5_000_000_000));
        assert_eq!(
            DirError::NotAZip(9).to_string(),
            "no end-of-central-directory record in the last 9 bytes: not a zip"
        );
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
