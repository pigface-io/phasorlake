use super::*;
use crate::crawl::RemoteKind;

const CUR: &str = "https://www.nemweb.com.au/Reports/CURRENT/X/";
const ARC: &str = "https://www.nemweb.com.au/Reports/ARCHIVE/X/";

fn put(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn link(url: &str, on: &str) -> Remote {
    Remote {
        url: url.into(),
        kind: RemoteKind::File,
        size: Some(1),
        modified: "Monday, October 12, 2026 10:45 AM".into(),
        modified_at: Stamp::parse("2026-10-12T10:45:00+10:00").ok(),
        listed_on: on.into(),
        crawled_at: None,
    }
}

fn line(r: &Remote) -> String {
    serde_json::to_string(r).unwrap() + "\n"
}

/// A home in the layout before `state/`, with every record it kept.
fn old_home() -> (tempfile::TempDir, Home) {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path());
    let old = home.old_records();
    put(&old.join("config.toml"), "request_interval_ms = 750\n");
    for name in [
        "peek.ndjson",
        "removed.ndjson",
        "held.ndjson",
        "relayout.ndjson",
    ] {
        put(&old.join(name), "{}\n");
    }
    put(&old.join("progress.json"), "{}");
    put(&old.join("map/map.ndjson"), "{}\n");
    put(&old.join("map/tree.txt"), "tree\n");
    put(&old.join("lock"), "");
    put(&old.join("tmp/x"), "");
    put(&old.join("notes.txt"), "mine");
    put(&old.join("._notes.txt"), "twin");
    // The crawl: both listings of X, a folder since gone, an overlay that
    // replaces what CURRENT listed.
    let crawl = old.join("crawl");
    put(&crawl.join("done"), "");
    let record = [
        link(&format!("{CUR}a.zip"), CUR),
        link(&format!("{CUR}old.zip"), CUR),
        link(&format!("{ARC}d.zip"), ARC),
        link(
            "https://www.nemweb.com.au/Reports/CURRENT/G/g.zip",
            "https://www.nemweb.com.au/Reports/CURRENT/G/",
        ),
    ];
    put(
        &crawl.join("remote.ndjson"),
        &record.iter().map(line).collect::<String>(),
    );
    let overlay = serde_json::json!({
        "folder": CUR,
        "links": [link(&format!("{CUR}a.zip"), CUR), link(&format!("{CUR}b.zip"), CUR)],
    });
    put(
        &crawl.join("folders/www.nemweb.com.au/Reports/CURRENT/X/listing.json"),
        &overlay.to_string(),
    );
    put(
        &crawl.join("folders/www.nemweb.com.au/Reports/CURRENT/G/gone"),
        "https://www.nemweb.com.au/Reports/CURRENT/G/",
    );
    put(&crawl.join("folders/www.nemweb.com.au/stray.txt"), "?");
    put(
        &crawl.join("listings/www.nemweb.com.au/Reports/CURRENT/X/index.html"),
        "<cur>",
    );
    put(
        &crawl.join("listings/www.nemweb.com.au/Reports/ARCHIVE/X/index.html"),
        "<arc>",
    );
    put(
        &crawl.join("listings/www.nemweb.com.au/Reports/index.html"),
        "<reports>",
    );
    put(
        &old.join("journal/www.nemweb.com.au/Reports/X/changes.ndjson"),
        "{\"seen_at\":\"2026-10-20T00:00:00Z\",\"url\":\"u\",\"event\":\"same\"}\n",
    );
    // The other listing's journal, not yet merged by a relayout.
    put(
        &old.join("journal/www.nemweb.com.au/Reports/CURRENT/X/changes.ndjson"),
        "{\"seen_at\":\"2026-10-19T00:00:00Z\",\"url\":\"v\",\"event\":\"repacked\"}\n\nnot json\n",
    );
    (dir, home)
}

/// An archive with records of format 1 and 2, one that does not read, and
/// a file with none.
fn archive_of(home: &Home) -> (Archive, Vec<PathBuf>) {
    let archive = Archive::new(home.archive());
    let dir = home.archive().join("www.nemweb.com.au/Reports/X");
    let mut files = Vec::new();
    for (name, bytes) in [("a.zip", "aa"), ("d.zip", "dd")] {
        let path = dir.join(name);
        put(&path, bytes);
        let v1 = serde_json::json!({
            "url": format!("{CUR}{name}"), "size": 2,
            "sha256": crate::net::archive::sha256_hex(bytes.as_bytes()),
            "fetched_at": "2026-10-20T00:00:00Z",
        });
        put(&dir.join(format!("{name}.json")), &v1.to_string());
        files.push(path);
    }
    let new = dir.join("n.zip");
    put(&new, "n");
    Archive::write(
        &new,
        &crate::net::archive::test_record(&format!("{CUR}n.zip"), b"n"),
    )
    .unwrap();
    // Both records, as a run cut off after the new one leaves them: the
    // new one is kept, the old one goes.
    let both = dir.join("both.zip");
    put(&both, "b");
    Archive::write(
        &both,
        &crate::net::archive::test_record(&format!("{CUR}both.zip"), b"b"),
    )
    .unwrap();
    put(&dir.join("both.zip.json"), "{}");
    put(&dir.join("cut.zip"), "c");
    put(&dir.join("cut.zip.json"), "{");
    put(&dir.join("bare.zip"), "b");
    (archive, files)
}

#[test]
fn a_dry_run_says_what_would_move_and_moves_nothing() {
    let (_dir, home) = old_home();
    let (archive, _) = archive_of(&home);
    let mut steps = Vec::new();
    let s = upgrade(&home, &archive, true, true, |st| steps.push(st.action)).unwrap();
    assert!(s.dry_run);
    assert_eq!((s.moved, s.listings, s.records), (8 + 5 + 1, 1, 3));
    assert_eq!(s.unreadable.len(), 1);
    assert_eq!(s.left, [home.old_records().join("notes.txt")]);
    assert!(steps.contains(&Action::Listings));
    assert!(home.old_records().join("crawl/remote.ndjson").exists());
    assert!(!home.records().dir().exists());
}

#[test]
fn every_record_moves_to_its_name_and_a_second_run_does_nothing() {
    let (_dir, home) = old_home();
    let (archive, files) = archive_of(&home);
    let before: Vec<_> = files
        .iter()
        .map(|p| std::fs::metadata(p).unwrap().modified().unwrap())
        .collect();
    let s = upgrade(&home, &archive, false, true, |_| {}).unwrap();
    assert_eq!((s.moved, s.listings, s.records), (8 + 5 + 1, 1, 3));
    let state = home.records();
    let cache = state.cache_dir();
    for p in [
        home.config_file(),
        state.dir().join("peeks.ndjson"),
        state.dir().join("removals.ndjson"),
        cache.join("held.ndjson"),
        cache.join("status.json"),
        cache.join("catalog.ndjson"),
        cache.join("catalog.txt"),
        state.migrations_dir().join("relayout.ndjson"),
        state.migrations_dir().join("remote.ndjson.migrated"),
        state.migrations_dir().join("upgrade.ndjson"),
    ] {
        assert!(p.exists(), "{}", p.display());
    }
    // One listing file for both listings of X; G is gone; the overlay won.
    let urls: Vec<String> = state.record().unwrap().into_iter().map(|r| r.url).collect();
    assert_eq!(
        urls,
        [
            format!("{ARC}d.zip"),
            format!("{CUR}a.zip"),
            format!("{CUR}b.zip")
        ]
    );
    assert!(state.listing_path(CUR) == state.listing_path(ARC));
    let page =
        |u: &str| std::fs::read_to_string(state.page_path(&url::Url::parse(u).unwrap())).unwrap();
    assert_eq!((page(CUR), page(ARC)), ("<cur>".into(), "<arc>".into()));
    assert_eq!(page("https://www.nemweb.com.au/Reports/"), "<reports>");
    // Both journals in one, in time order.
    let events = state.journal(CUR).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!((events[0].url.as_str(), events[1].url.as_str()), ("v", "u"));
    assert!(state.done_path().exists());
    // The old folder holds only what no step knows.
    assert!(home.old_records().join("notes.txt").exists());
    assert!(!home.old_records().join("crawl").exists());
    // The records are format 2; the bytes untouched.
    for (p, t) in files.iter().zip(before) {
        assert!(Archive::record_path(p).exists());
        assert!(!p.with_extension("zip.json").exists());
        assert_eq!(std::fs::metadata(p).unwrap().modified().unwrap(), t);
    }
    assert!(s.unreadable[0].ends_with("cut.zip.json"));
    let both = files[0].with_file_name("both.zip");
    assert!(!both.with_extension("zip.json").exists());
    assert!(Archive::read(&both).unwrap().is_some());
    // Again: nothing to do.
    let again = upgrade(&home, &archive, false, true, |_| {}).unwrap();
    assert_eq!((again.moved, again.listings, again.records), (0, 0, 0));
}

#[test]
fn a_home_already_upgraded_or_never_used_has_nothing_to_move() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path());
    let archive = Archive::new(home.archive());
    let s = upgrade(&home, &archive, false, false, |_| {}).unwrap();
    assert_eq!(s, UpgradeSummary::default());
    // A crawl record with no overlays, and overlays with no crawl record.
    // Without the overlays G is not gone: two folders; without the crawl
    // record only the overlay's X.
    for (gone, listings) in [("folders", 2), ("remote.ndjson", 1)] {
        let (_d, home) = old_home();
        let crawl = home.old_records().join("crawl");
        let p = crawl.join(gone);
        if p.is_dir() {
            std::fs::remove_dir_all(&p).unwrap();
        } else {
            std::fs::remove_file(&p).unwrap();
        }
        let archive = Archive::new(home.archive());
        let s = upgrade(&home, &archive, false, false, |_| {}).unwrap();
        assert_eq!(s.listings, listings, "{gone}");
    }
    // Records left in the archive when asked, and an archive not there.
    let (_d, home) = old_home();
    let (archive, files) = archive_of(&home);
    upgrade(&home, &archive, false, false, |_| {}).unwrap();
    assert!(!Archive::record_path(&files[0]).exists());
    let gone = Archive::new(home.root().join("none"));
    assert!(upgrade(&home, &gone, false, true, |_| {}).is_err());
}

#[test]
fn what_cannot_be_read_stops_the_run() {
    let (_dir, home) = old_home();
    let archive = Archive::new(home.archive());
    let crawl = home.old_records().join("crawl");
    put(
        &crawl.join("folders/www.nemweb.com.au/Reports/CURRENT/X/listing.json"),
        "{",
    );
    let e = upgrade(&home, &archive, false, false, |_| {}).unwrap_err();
    assert!(e.to_string().contains("listing.json"), "{e}");
    // A crawl record that is not a file.
    std::fs::remove_dir_all(crawl.join("folders")).unwrap();
    std::fs::remove_file(crawl.join("remote.ndjson")).unwrap();
    std::fs::create_dir(crawl.join("remote.ndjson")).unwrap();
    assert!(upgrade(&home, &archive, false, false, |_| {}).is_err());
    assert!(mtime(&home.root().join("none")).is_none());
}
