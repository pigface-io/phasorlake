//! Zips broken in the ways a real one can be, for tests.

use std::io::{Cursor, Write as _};

/// A zip of one stored entry, `name` holding `bytes`.
pub fn stored(name: &str, bytes: &[u8]) -> Vec<u8> {
    one(name, bytes, zip::CompressionMethod::Stored)
}

/// A zip of one deflated entry, `name` holding `bytes`.
pub fn deflated(name: &str, bytes: &[u8]) -> Vec<u8> {
    one(name, bytes, zip::CompressionMethod::Deflated)
}

fn one(name: &str, bytes: &[u8], method: zip::CompressionMethod) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    w.start_file(
        name,
        zip::write::SimpleFileOptions::default().compression_method(method),
    )
    .unwrap();
    w.write_all(bytes).unwrap();
    w.finish().unwrap().into_inner()
}

/// A stored zip of two entries, `a` then `b`, whose second entry's local
/// header no longer starts as one: the directory lists it, but it cannot
/// be found.
pub fn bad_second_header() -> Vec<u8> {
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for name in ["a", "b"] {
        w.start_file(name, opts).unwrap();
        w.write_all(b"x").unwrap();
    }
    let mut zip = w.finish().unwrap().into_inner();
    let second = zip
        .windows(4)
        .enumerate()
        .filter(|(_, w)| *w == b"PK\x03\x04")
        .nth(1)
        .expect("two entries")
        .0;
    zip[second..second + 4].copy_from_slice(b"XXXX");
    zip
}

/// The first entry marked encrypted, in its local header and in the
/// directory: listed, but not readable without a password.
pub fn encrypted(mut zip: Vec<u8>) -> Vec<u8> {
    zip[6] |= 1;
    let cd = zip
        .windows(4)
        .position(|w| w == b"PK\x01\x02")
        .expect("a directory");
    zip[cd + 8] |= 1;
    zip
}

/// The first entry's compressed bytes overwritten with ones deflate rejects.
pub fn corrupt_data(mut zip: Vec<u8>) -> Vec<u8> {
    let name = usize::from(u16::from_le_bytes([zip[26], zip[27]]));
    let extra = usize::from(u16::from_le_bytes([zip[28], zip[29]]));
    let data = 30 + name + extra;
    zip[data] = 0xff;
    zip
}

/// The first entry's uncompressed size, in its local header and in the
/// directory, declared 5 bytes short of what it holds.
pub fn understated(zip: &[u8]) -> Vec<u8> {
    let mut zip = zip.to_vec();
    let size = u32::from_le_bytes(zip[22..26].try_into().expect("four bytes")) - 5;
    zip[22..26].copy_from_slice(&size.to_le_bytes());
    let cd = zip
        .windows(4)
        .position(|w| w == b"PK\x01\x02")
        .expect("a directory");
    zip[cd + 24..cd + 28].copy_from_slice(&size.to_le_bytes());
    zip
}
