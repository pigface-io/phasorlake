//! Reading one member of a remote zip without downloading the zip.
//!
//! Before 2015 the archive publishes each month only as one zip of 1.4 to
//! 2.1 GB (`MMSDM_2012_01.zip`) holding the per-table zips. A zip's index,
//! the central directory, sits at its end, so three ranged GETs are enough
//! to fetch one table: the last 64 KiB (the end record and usually the whole
//! directory), the directory itself when it did not fit, and the member's
//! own bytes. This module is the format half: pure functions over bytes.
//! The [`Fetcher`](crate::Fetcher) does the requests.
//!
//! Supported: stored and deflated members, zip64 sizes and offsets.
//! Not supported, and reported as such: encryption, multi-disk archives.

use flate2::{Decompress, FlushDecompress, Status};
use serde::{Deserialize, Serialize};

/// How many trailing bytes to ask for first: the end record (22 bytes), the
/// longest possible comment (65,535) and the zip64 locator (20).
pub const TAIL_BYTES: u64 = 22 + 65_535 + 20;

/// Slack requested after a member's local header for an extra field longer
/// than the one the central directory showed.
pub const LOCAL_EXTRA_SLACK: u64 = 4096;

const EOCD_SIG: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
const EOCD64_LOCATOR_SIG: [u8; 4] = [0x50, 0x4b, 0x06, 0x07];
const EOCD64_SIG: [u8; 4] = [0x50, 0x4b, 0x06, 0x06];
const CENTRAL_SIG: [u8; 4] = [0x50, 0x4b, 0x01, 0x02];
const LOCAL_SIG: [u8; 4] = [0x50, 0x4b, 0x03, 0x04];

/// One file inside a remote zip, from its central directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZipMember {
    /// Path inside the zip, `/` separated.
    pub name: String,
    /// Compression method: 0 stored, 8 deflate.
    pub method: u16,
    /// CRC-32 of the uncompressed bytes.
    pub crc32: u32,
    /// Bytes in the zip.
    pub compressed_size: u64,
    /// Bytes once inflated.
    pub uncompressed_size: u64,
    /// Offset of the member's local file header in the zip.
    pub header_offset: u64,
    /// Length of name plus extra field in the central directory; the local
    /// header's are usually the same.
    pub name_and_extra: u32,
    /// General-purpose flags (bit 0 = encrypted).
    pub flags: u16,
}

impl ZipMember {
    /// The last path segment.
    #[must_use]
    pub fn file_name(&self) -> &str {
        self.name.rsplit('/').next().unwrap_or(&self.name)
    }

    /// Whether the name ends in `/`.
    #[cfg(test)]
    pub(crate) fn is_dir(&self) -> bool {
        self.name.ends_with('/')
    }

    /// The inclusive byte span to request for the local header and the data,
    /// with slack for a longer local extra field, clamped to `total` bytes.
    #[must_use]
    pub fn request_span(&self, total: Option<u64>) -> (u64, u64) {
        let end = self.header_offset
            + 30
            + u64::from(self.name_and_extra)
            + LOCAL_EXTRA_SLACK
            + self.compressed_size;
        let end = total.map_or(end, |t| end.min(t));
        (
            self.header_offset,
            end.saturating_sub(1).max(self.header_offset),
        )
    }
}

/// Where the central directory is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectoryLocation {
    /// Offset of the first central header.
    pub offset: u64,
    /// Size of the directory in bytes.
    pub size: u64,
    /// Number of entries.
    pub entries: u64,
}

/// What the tail of a zip says about its directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tail {
    /// The directory location is known.
    Located(DirectoryLocation),
    /// A zip64 archive whose zip64 end record lies before the bytes given;
    /// read 56 bytes at this offset and call [`parse_eocd64`].
    NeedEocd64 {
        /// Offset of the zip64 end-of-central-directory record.
        offset: u64,
    },
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

fn u64_at(b: &[u8], i: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[i..i + 8]);
    u64::from_le_bytes(a)
}

/// Parses the end of a zip. `tail` is the last bytes of the file and
/// `tail_start` the offset of its first byte.
pub fn parse_tail(tail: &[u8], tail_start: u64) -> Result<Tail, String> {
    // The end record is the last signature whose comment length reaches the
    // end of the file exactly.
    let mut found = None;
    let mut i = tail.len().saturating_sub(22);
    loop {
        if tail.len() >= i + 22
            && tail[i..i + 4] == EOCD_SIG
            && i + 22 + usize::from(u16_at(tail, i + 20)) == tail.len()
        {
            found = Some(i);
            break;
        }
        if i == 0 {
            break;
        }
        i -= 1;
    }
    let Some(i) = found else {
        return Err(
            "no end-of-central-directory record in the last bytes; not a zip, or truncated".into(),
        );
    };
    let disk = u16_at(tail, i + 4);
    let cd_disk = u16_at(tail, i + 6);
    let entries_here = u16_at(tail, i + 8);
    let entries = u16_at(tail, i + 10);
    let size = u32_at(tail, i + 12);
    let offset = u32_at(tail, i + 16);
    let needs64 = entries == u16::MAX || size == u32::MAX || offset == u32::MAX;
    if !needs64 {
        if disk != 0 || cd_disk != 0 || entries_here != entries {
            return Err("multi-disk zip archives are not supported".into());
        }
        return Ok(Tail::Located(DirectoryLocation {
            offset: u64::from(offset),
            size: u64::from(size),
            entries: u64::from(entries),
        }));
    }
    // zip64: a 20-byte locator sits immediately before the end record.
    if i < 20 || tail[i - 20..i - 16] != EOCD64_LOCATOR_SIG {
        return Err("the end record needs zip64 fields but no zip64 locator precedes it".into());
    }
    let eocd64_offset = u64_at(tail, i - 12);
    if eocd64_offset >= tail_start {
        let rel = (eocd64_offset - tail_start) as usize;
        if rel + 56 <= tail.len() {
            return parse_eocd64(&tail[rel..rel + 56]).map(Tail::Located);
        }
    }
    Ok(Tail::NeedEocd64 {
        offset: eocd64_offset,
    })
}

/// Parses a zip64 end-of-central-directory record (its first 56 bytes).
pub fn parse_eocd64(rec: &[u8]) -> Result<DirectoryLocation, String> {
    if rec.len() < 56 || rec[..4] != EOCD64_SIG {
        return Err("the zip64 end record is missing or truncated".into());
    }
    if u32_at(rec, 16) != 0 || u32_at(rec, 20) != 0 {
        return Err("multi-disk zip archives are not supported".into());
    }
    Ok(DirectoryLocation {
        entries: u64_at(rec, 32),
        size: u64_at(rec, 40),
        offset: u64_at(rec, 48),
    })
}

/// Parses a whole central directory.
pub fn parse_directory(cd: &[u8]) -> Result<Vec<ZipMember>, String> {
    let mut out = Vec::new();
    let mut p = 0usize;
    while p + 4 <= cd.len() && cd[p..p + 4] == CENTRAL_SIG {
        if p + 46 > cd.len() {
            return Err(format!("central header {} is truncated", out.len() + 1));
        }
        let flags = u16_at(cd, p + 8);
        let method = u16_at(cd, p + 10);
        let crc32 = u32_at(cd, p + 16);
        let mut compressed_size = u64::from(u32_at(cd, p + 20));
        let mut uncompressed_size = u64::from(u32_at(cd, p + 24));
        let name_len = usize::from(u16_at(cd, p + 28));
        let extra_len = usize::from(u16_at(cd, p + 30));
        let comment_len = usize::from(u16_at(cd, p + 32));
        let mut header_offset = u64::from(u32_at(cd, p + 42));
        let name_start = p + 46;
        let extra_start = name_start + name_len;
        let next = extra_start + extra_len + comment_len;
        if next > cd.len() {
            return Err(format!(
                "central header {} runs past the directory",
                out.len() + 1
            ));
        }
        let name = String::from_utf8_lossy(&cd[name_start..extra_start]).into_owned();
        // zip64 extended information: only the fields that overflowed, in order.
        let extra = &cd[extra_start..extra_start + extra_len];
        let mut e = 0usize;
        while e + 4 <= extra.len() {
            let id = u16_at(extra, e);
            let len = usize::from(u16_at(extra, e + 2));
            let body_start = e + 4;
            let body_end = (body_start + len).min(extra.len());
            if id == 0x0001 {
                let mut q = body_start;
                for field in [
                    &mut uncompressed_size,
                    &mut compressed_size,
                    &mut header_offset,
                ] {
                    if *field == u64::from(u32::MAX) {
                        if q + 8 > body_end {
                            return Err(format!("the zip64 field of member {name} is truncated"));
                        }
                        *field = u64_at(extra, q);
                        q += 8;
                    }
                }
            }
            e = body_end;
        }
        out.push(ZipMember {
            name,
            method,
            crc32,
            compressed_size,
            uncompressed_size,
            header_offset,
            name_and_extra: (name_len + extra_len) as u32,
            flags,
        });
        p = next;
    }
    if out.is_empty() && !cd.is_empty() {
        return Err("the central directory does not start with a central header".into());
    }
    Ok(out)
}

/// The offset of a member's data relative to its local header, once at least
/// the 30 fixed bytes are in hand. `None` means more bytes are needed;
/// an error means the bytes are not a local header.
pub fn local_data_offset(header: &[u8]) -> Result<Option<usize>, String> {
    if header.len() < 30 {
        return Ok(None);
    }
    if header[..4] != LOCAL_SIG {
        return Err("the bytes at the member's offset are not a local file header".into());
    }
    Ok(Some(
        30 + usize::from(u16_at(header, 26)) + usize::from(u16_at(header, 28)),
    ))
}

/// Streaming raw-deflate decoder: feed compressed chunks, get inflated bytes.
pub struct Inflater {
    inner: Decompress,
    done: bool,
}

impl Default for Inflater {
    fn default() -> Self {
        Self::new()
    }
}

impl Inflater {
    /// A decoder for a raw deflate stream (no zlib header), as zip stores it.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Decompress::new(false),
            done: false,
        }
    }

    /// Whether the end of the deflate stream was reached.
    #[cfg(test)]
    pub(crate) const fn finished(&self) -> bool {
        self.done
    }

    /// Inflates `input`, appending to `out`. Bytes after the end of the
    /// stream are ignored.
    pub fn feed(&mut self, mut input: &[u8], out: &mut Vec<u8>) -> Result<(), String> {
        while !self.done {
            if out.capacity() - out.len() < 32 * 1024 {
                out.reserve(64 * 1024);
            }
            let before_in = self.inner.total_in();
            let before_out = self.inner.total_out();
            let status = self
                .inner
                .decompress_vec(input, out, FlushDecompress::None)
                .map_err(|e| e.to_string())?;
            let used = (self.inner.total_in() - before_in) as usize;
            let produced = self.inner.total_out() - before_out;
            input = &input[used..];
            match status {
                Status::StreamEnd => self.done = true,
                Status::Ok | Status::BufError => {
                    if input.is_empty() && out.capacity() > out.len() {
                        break;
                    }
                    if used == 0 && produced == 0 && out.capacity() > out.len() {
                        return Err("the deflate stream made no progress".into());
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;

    /// A zip with the given members; `stored` names are not compressed.
    pub(crate) fn make_zip(members: &[(&str, &[u8])], stored: &[&str], zip64: bool) -> Vec<u8> {
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut cursor);
            for (name, bytes) in members {
                let method = if stored.contains(name) {
                    zip::CompressionMethod::Stored
                } else {
                    zip::CompressionMethod::Deflated
                };
                let opts = zip::write::SimpleFileOptions::default()
                    .compression_method(method)
                    .large_file(zip64);
                if name.ends_with('/') {
                    w.add_directory(name.trim_end_matches('/'), opts).unwrap();
                } else {
                    w.start_file(*name, opts).unwrap();
                    w.write_all(bytes).unwrap();
                }
            }
            w.finish().unwrap();
        }
        cursor.into_inner()
    }

    fn locate(z: &[u8]) -> DirectoryLocation {
        let start = z.len().saturating_sub(TAIL_BYTES as usize);
        match parse_tail(&z[start..], start as u64).unwrap() {
            Tail::Located(l) => l,
            Tail::NeedEocd64 { offset } => {
                parse_eocd64(&z[offset as usize..offset as usize + 56]).unwrap()
            }
        }
    }

    fn extract(z: &[u8], m: &ZipMember) -> Vec<u8> {
        let (start, end) = m.request_span(Some(z.len() as u64));
        let bytes = &z[start as usize..=end as usize];
        let data = local_data_offset(bytes).unwrap().unwrap();
        let comp = &bytes[data..data + m.compressed_size as usize];
        match m.method {
            0 => comp.to_vec(),
            8 => {
                let mut inf = Inflater::new();
                let mut out = Vec::new();
                // Feed in awkward chunk sizes to exercise the loop.
                for chunk in comp.chunks(7) {
                    inf.feed(chunk, &mut out).unwrap();
                }
                assert!(inf.finished());
                out
            }
            other => panic!("method {other}"),
        }
    }

    #[test]
    fn lists_and_extracts_members() {
        let big: Vec<u8> = (0..200_000u32)
            .flat_map(|i| (i % 251).to_le_bytes())
            .collect();
        let z = make_zip(
            &[
                ("01/", b""),
                ("01/DATA/a.zip", b"hello hello hello hello"),
                ("01/DATA/big.bin", &big),
                ("01/readme.txt", b"stored text"),
            ],
            &["01/readme.txt"],
            false,
        );
        let loc = locate(&z);
        assert_eq!(loc.entries, 4);
        let cd = &z[loc.offset as usize..(loc.offset + loc.size) as usize];
        let members = parse_directory(cd).unwrap();
        assert_eq!(members.len(), 4);
        assert!(members[0].is_dir());
        assert_eq!(members[1].file_name(), "a.zip");
        assert_eq!(members[1].method, 8);
        assert_eq!(members[3].method, 0);
        assert_eq!(extract(&z, &members[1]), b"hello hello hello hello");
        assert_eq!(extract(&z, &members[2]), big);
        assert_eq!(extract(&z, &members[3]), b"stored text");
        assert_eq!(members[2].uncompressed_size, big.len() as u64);
        assert_eq!(crc32fast::hash(&big), members[2].crc32);
        // The span is clamped to the file and starts at the header.
        let (s, e) = members[3].request_span(Some(z.len() as u64));
        assert_eq!(s, members[3].header_offset);
        assert!(e < z.len() as u64);
        let (_, e) = members[3].request_span(None);
        assert!(e >= z.len() as u64 - 1);
    }

    #[test]
    fn reads_zip64_fields() {
        let z = make_zip(
            &[("a/x.csv", b"1,2,3\n"), ("a/y.csv", b"4,5,6\n")],
            &[],
            true,
        );
        let loc = locate(&z);
        assert_eq!(loc.entries, 2);
        let members =
            parse_directory(&z[loc.offset as usize..(loc.offset + loc.size) as usize]).unwrap();
        assert_eq!(extract(&z, &members[0]), b"1,2,3\n");
        assert_eq!(extract(&z, &members[1]), b"4,5,6\n");

        // A hand-built zip64 tail whose end record lies before the tail given.
        let mut tail = Vec::new();
        tail.extend_from_slice(&EOCD64_LOCATOR_SIG);
        tail.extend_from_slice(&0u32.to_le_bytes());
        tail.extend_from_slice(&5_000_000_000u64.to_le_bytes());
        tail.extend_from_slice(&1u32.to_le_bytes());
        tail.extend_from_slice(&EOCD_SIG);
        tail.extend_from_slice(&[0, 0, 0, 0]);
        tail.extend_from_slice(&u16::MAX.to_le_bytes());
        tail.extend_from_slice(&u16::MAX.to_le_bytes());
        tail.extend_from_slice(&u32::MAX.to_le_bytes());
        tail.extend_from_slice(&u32::MAX.to_le_bytes());
        tail.extend_from_slice(&0u16.to_le_bytes());
        assert_eq!(
            parse_tail(&tail, 6_000_000_000).unwrap(),
            Tail::NeedEocd64 {
                offset: 5_000_000_000
            }
        );
        let mut rec = Vec::new();
        rec.extend_from_slice(&EOCD64_SIG);
        rec.extend_from_slice(&44u64.to_le_bytes());
        rec.extend_from_slice(&[45, 0, 45, 0]);
        rec.extend_from_slice(&0u32.to_le_bytes());
        rec.extend_from_slice(&0u32.to_le_bytes());
        rec.extend_from_slice(&70_000u64.to_le_bytes());
        rec.extend_from_slice(&70_000u64.to_le_bytes());
        rec.extend_from_slice(&9_000_000u64.to_le_bytes());
        rec.extend_from_slice(&4_990_000_000u64.to_le_bytes());
        assert_eq!(
            parse_eocd64(&rec).unwrap(),
            DirectoryLocation {
                offset: 4_990_000_000,
                size: 9_000_000,
                entries: 70_000
            }
        );
        // The same record inside the tail is parsed in one step.
        let mut whole = rec.clone();
        let mut t2 = tail.clone();
        t2[8..16].copy_from_slice(&100u64.to_le_bytes());
        whole.extend_from_slice(&t2);
        assert_eq!(
            parse_tail(&whole, 100).unwrap(),
            Tail::Located(parse_eocd64(&rec).unwrap())
        );
        // Central header with all three zip64 fields.
        let mut cd = Vec::new();
        cd.extend_from_slice(&CENTRAL_SIG);
        cd.extend_from_slice(&[45, 0, 45, 0, 0, 0, 8, 0, 0, 0, 0, 0]);
        cd.extend_from_slice(&7u32.to_le_bytes());
        cd.extend_from_slice(&u32::MAX.to_le_bytes());
        cd.extend_from_slice(&u32::MAX.to_le_bytes());
        cd.extend_from_slice(&1u16.to_le_bytes());
        cd.extend_from_slice(&28u16.to_le_bytes());
        // comment length, disk, internal attributes, then external attributes.
        cd.extend_from_slice(&[0; 6]);
        cd.extend_from_slice(&0u32.to_le_bytes());
        cd.extend_from_slice(&u32::MAX.to_le_bytes());
        cd.push(b'x');
        cd.extend_from_slice(&1u16.to_le_bytes());
        cd.extend_from_slice(&24u16.to_le_bytes());
        cd.extend_from_slice(&6_000_000_000u64.to_le_bytes());
        cd.extend_from_slice(&5_000_000_000u64.to_le_bytes());
        cd.extend_from_slice(&7_000_000_000u64.to_le_bytes());
        let m = &parse_directory(&cd).unwrap()[0];
        assert_eq!(
            (m.uncompressed_size, m.compressed_size, m.header_offset),
            (6_000_000_000, 5_000_000_000, 7_000_000_000)
        );
        assert_eq!(m.crc32, 7);
        // A zip64 field cut short is an error naming the member.
        let cut = &cd[..cd.len() - 4];
        let mut cut = cut.to_vec();
        let extra_len_at = 30;
        cut[extra_len_at..extra_len_at + 2].copy_from_slice(&24u16.to_le_bytes());
        assert!(parse_directory(&cut).unwrap_err().contains("member x"));
    }

    #[test]
    fn rejects_what_is_not_a_zip() {
        assert!(parse_tail(b"<html>not a zip at all, just a page</html>", 0).is_err());
        assert!(parse_tail(b"", 0).is_err());
        assert!(parse_eocd64(b"short").is_err());
        assert!(parse_directory(b"garbage that is not a header").is_err());
        assert_eq!(parse_directory(b"").unwrap(), vec![]);
        let mut truncated = CENTRAL_SIG.to_vec();
        truncated.extend_from_slice(&[0; 10]);
        assert!(
            parse_directory(&truncated)
                .unwrap_err()
                .contains("truncated")
        );
        let mut overrun = CENTRAL_SIG.to_vec();
        overrun.extend_from_slice(&[0; 42]);
        overrun[28] = 200;
        assert!(parse_directory(&overrun).unwrap_err().contains("runs past"));
        assert_eq!(local_data_offset(b"PK").unwrap(), None);
        assert!(local_data_offset(&[0u8; 30]).is_err());
        // Multi-disk and a zip64 end record without its locator.
        let mut eocd = EOCD_SIG.to_vec();
        eocd.extend_from_slice(&[1, 0, 0, 0, 1, 0, 1, 0]);
        eocd.extend_from_slice(&[0; 10]);
        assert!(parse_tail(&eocd, 0).unwrap_err().contains("multi-disk"));
        let mut eocd = EOCD_SIG.to_vec();
        eocd.extend_from_slice(&[0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]);
        eocd.extend_from_slice(&[0; 10]);
        assert!(parse_tail(&eocd, 0).unwrap_err().contains("zip64 locator"));
        let mut multi = EOCD64_SIG.to_vec();
        multi.extend_from_slice(&[0; 52]);
        multi[16] = 1;
        assert!(parse_eocd64(&multi).unwrap_err().contains("multi-disk"));
        // Corrupt deflate data is an error, not a hang.
        let mut inf = Inflater::default();
        let mut out = Vec::new();
        assert!(inf.feed(&[0xff; 64], &mut out).is_err());
    }

    #[test]
    fn end_record_with_a_comment_is_found() {
        let mut z = make_zip(&[("a.txt", b"x")], &[], false);
        let n = z.len();
        // Give the archive a comment that itself contains an end signature.
        let comment = [b'c', 0x50, 0x4b, 0x05, 0x06, b'c', b'c', b'c'];
        z[n - 2..].copy_from_slice(&(comment.len() as u16).to_le_bytes());
        z.extend_from_slice(&comment);
        let loc = locate(&z);
        assert_eq!(loc.entries, 1);
    }
}
