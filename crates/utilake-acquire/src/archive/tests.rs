use std::path::PathBuf;

use utilake_core::timetext::Stamp;

use super::*;
use utilake_core::Diagnose as _;

/// A reader interrupted once, then done.
struct Flaky(usize, &'static [u8]);

impl Read for Flaky {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0 += 1;
        match self.0 {
            1 => Err(std::io::Error::from(std::io::ErrorKind::Interrupted)),
            2 => {
                let n = self.1.len().min(buf.len());
                buf[..n].copy_from_slice(&self.1[..n]);
                self.1 = &self.1[n..];
                Ok(n)
            }
            3 => Ok(0),
            _ => Err(std::io::Error::other("gone")),
        }
    }
}

/// `bytes` held at `url`'s path with a record that they were fetched there.
fn hold(a: &Archive, url: &str, bytes: &[u8]) -> PathBuf {
    let path = a.path_for(url);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, bytes).unwrap();
    let p = Provenance::new(
        Content::of(bytes),
        "t",
        Address {
            url: url.into(),
            seen: Stamp::new(1_800_000_000, 0),
            gone: None,
            via: Via::Fetched {
                listed: None,
                served: None,
            },
        },
    );
    Archive::write(&path, &p).unwrap();
    path
}

#[test]
fn hashing_rides_out_an_interruption_and_reports_a_failure() {
    let (sha, len) = sha256_read(&mut Flaky(0, b"abc")).unwrap();
    assert_eq!((sha, len), (sha256_hex(b"abc"), 3));
    let mut gone = Flaky(3, b"");
    assert_eq!(sha256_read(&mut gone).unwrap_err().to_string(), "gone");
    let dir = tempfile::tempdir().unwrap();
    assert!(sha256_file(&dir.path().join("none")).is_err());
}

#[test]
fn the_store_errors_are_the_fetch_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.zip");
    std::fs::write(Archive::record_path(&path), b"{").unwrap();
    let e: FetchError = Archive::read(&path).unwrap_err().into();
    assert_eq!(e.diagnostic().code, "cache.sidecar_corrupt");
    std::fs::remove_file(Archive::record_path(&path)).unwrap();
    std::fs::create_dir(Archive::record_path(&path)).unwrap();
    let e: FetchError = Archive::read(&path).unwrap_err().into();
    assert_eq!(e.diagnostic().code, "cache.io");
}

#[test]
fn lookup_verify_addresses_and_entries() {
    let dir = tempfile::tempdir().unwrap();
    let a = Archive::new(dir.path());
    let url = "https://www.nemweb.com.au/Reports/CURRENT/X/f.zip";
    assert!(a.lookup(url).unwrap().is_none());
    let path = hold(&a, url, b"abc");
    let found = a.lookup(url).unwrap().unwrap();
    verify(&found).unwrap();
    // Found again in the day's zip: one more address, once.
    let entry = Address {
        url: "https://www.nemweb.com.au/Reports/ARCHIVE/X/D.zip!f.zip".into(),
        seen: Stamp::new(1_800_000_100, 0),
        gone: None,
        via: Via::Entry,
    };
    assert!(add_address(&path, entry.clone()).unwrap());
    assert!(!add_address(&path, entry.clone()).unwrap());
    assert!(!add_address(&dir.path().join("none"), entry).unwrap());
    // An AppleDouble twin is no file of the archive.
    std::fs::write(path.with_file_name("._f.zip"), b"x").unwrap();
    let all = entries_under(&a, None);
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].provenance.addresses.len(), 2);
    assert!(entries_under(&a, Some(&dir.path().join("none"))).is_empty());
    // Bytes changed under the record, or gone.
    std::fs::write(&path, b"abd").unwrap();
    assert_eq!(
        verify(&found).unwrap_err().diagnostic().code,
        "cache.sha256_mismatch"
    );
    std::fs::remove_file(&path).unwrap();
    assert_eq!(verify(&found).unwrap_err().diagnostic().code, "cache.io");
}
