use super::*;
use crate::diag::Diagnose as _;

const URL: &str = "https://h/d/f.zip";

/// Writes `bytes` at `url`'s path in `a` with a record that they were
/// fetched from `url`.
fn hold(a: &Archive, url: &str, bytes: &[u8]) -> PathBuf {
    let path = a.path_for(url);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, bytes).unwrap();
    let p = Provenance::new(
        Content::of(bytes),
        "t",
        Address {
            url: url.into(),
            seen: Stamp::from_unix(1_800_000_000),
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
fn dir_names_reads_each_directory_once() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a"), b"").unwrap();
    let mut names = DirNames::default();
    assert!(names.holds(&dir.path().join("a")));
    // Read once: a file made since is not seen.
    std::fs::write(dir.path().join("b"), b"").unwrap();
    assert!(!names.holds(&dir.path().join("b")));
    assert!(!names.holds(&dir.path().join("none/a")));
    assert!(!names.holds(Path::new("/")));
}

#[test]
fn hashes_and_apple_double_files() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("f");
    std::fs::write(&f, b"abc").unwrap();
    assert_eq!(content_of(&f).unwrap(), Content::of(b"abc"));
    assert_eq!(sha256_hex(b"abc"), Content::of(b"abc").sha256.hex());
    assert_eq!(
        content_of(dir.path()).unwrap_err().diagnostic().code,
        "cache.io"
    );
    assert!(is_apple_double(Path::new("/x/._f.csv")) && !is_apple_double(&f));
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
fn lookup_versions_verify_and_remove() {
    let dir = tempfile::tempdir().unwrap();
    let a = Archive::new(dir.path());
    let path = hold(&a, URL, b"abc");
    let first = lookup_at(&path, URL).unwrap().unwrap();
    assert_eq!(first.path, path);
    assert!(lookup_at(&path, "https://h/other").unwrap().is_none());
    verify(&first).unwrap();
    // A later version beside it, from another URL.
    let later = Archive::version_path(&path, Stamp::from_unix(1_800_000_100));
    let other = "https://h/e/f.zip";
    std::fs::write(&later, b"abcd").unwrap();
    let p = Provenance::new(
        Content::of(b"abcd"),
        "t",
        Address {
            url: other.into(),
            ..first.provenance.origin().clone()
        },
    );
    Archive::write(&later, &p).unwrap();
    assert_eq!(versions(&path, URL).unwrap().len(), 1);
    assert_eq!(versions(&path, other).unwrap()[0].path, later);
    // Changed bytes no longer match the record.
    std::fs::write(&path, b"abd").unwrap();
    assert_eq!(
        verify(&first).unwrap_err().diagnostic().code,
        "cache.sha256_mismatch"
    );
    std::fs::remove_file(&path).unwrap();
    assert_eq!(verify(&first).unwrap_err().diagnostic().code, "cache.io");
    // Removing a file takes its record, of either name, with it.
    std::fs::write(dir.path().join("x"), b"x").unwrap();
    std::fs::write(dir.path().join("x.json"), b"{}").unwrap();
    remove(&dir.path().join("x")).unwrap();
    assert!(!dir.path().join("x").exists() && !dir.path().join("x.json").exists());
    remove(&later).unwrap();
    assert!(!later.exists() && !Archive::record_path(&later).exists());
    assert!(remove(&later).is_err(), "gone already");
    std::fs::create_dir_all(Archive::record_path(&dir.path().join("y"))).unwrap();
    std::fs::create_dir_all(Archive::record_path(&dir.path().join("y")).join("z")).unwrap();
    assert!(remove(&dir.path().join("y")).is_err());
}

#[test]
fn addresses_are_added_and_folders_go() {
    let dir = tempfile::tempdir().unwrap();
    let a = Archive::new(dir.path());
    let folder = "https://www.nemweb.com.au/Reports/CURRENT/X/";
    let url = format!("{folder}f.zip");
    let path = hold(&a, &url, b"abc");
    let entry = Address {
        url: "https://www.nemweb.com.au/Reports/ARCHIVE/X/D.zip!f.zip".into(),
        seen: Stamp::from_unix(1_800_000_100),
        gone: None,
        via: Via::Entry,
    };
    assert!(add_address(&path, entry.clone()).unwrap());
    assert!(!add_address(&path, entry.clone()).unwrap(), "named already");
    assert!(
        !add_address(&dir.path().join("none"), entry).unwrap(),
        "no record"
    );
    // AppleDouble twins and a file with no record are not entries.
    std::fs::write(path.with_file_name("._f.zip"), b"x").unwrap();
    std::fs::write(path.with_file_name("bare.zip"), b"x").unwrap();
    std::fs::write(path.with_file_name("cut.zip"), b"x").unwrap();
    std::fs::write(path.with_file_name("cut.zip.provenance.json"), b"{").unwrap();
    let all = entries_under(&a, None);
    assert_eq!(all.iter().map(|h| &h.path).collect::<Vec<_>>(), [&path]);
    assert!(entries_under(&a, Some(&dir.path().join("none"))).is_empty());
    // The CURRENT listing goes: its address is marked, the entry's not.
    let at = Stamp::from_unix(1_800_000_200);
    assert_eq!(folder_gone(&a, folder, at).unwrap(), 1);
    assert_eq!(folder_gone(&a, folder, at).unwrap(), 0);
    let p = Archive::read(&path).unwrap().unwrap();
    assert_eq!(p.origin().gone, Some(at));
    assert_eq!(p.addresses[1].gone, None);
}
