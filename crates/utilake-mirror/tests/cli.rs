//! The binary end to end in a temp home, offline: every command, human and
//! JSON output, exit codes.

use std::fmt::Write as _;
use std::process::{Command, Output};

struct Cli {
    home: tempfile::TempDir,
}

impl Cli {
    fn new() -> Self {
        Self {
            home: tempfile::tempdir().unwrap(),
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_utilake-mirror"))
            .arg("--home")
            .arg(self.home.path())
            .arg("--no-progress")
            .args(args)
            .env_remove("UTILAKE_MIRROR_HOME")
            .output()
            .expect("the binary runs")
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{args:?} failed: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn code(&self, args: &[&str]) -> (i32, String, String) {
        let out = self.run(args);
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }
}

/// The mirror crawls from the pages it kept, offline, and counts what the
/// archive holds of what the crawl listed.
#[test]
fn the_mirror_crawls_kept_pages_and_reports_its_status() {
    let cli = Cli::new();
    let root = "https://h/A/";
    let pages = cli.home.path().join("state/folders/h/A");
    std::fs::create_dir_all(pages.join("pages")).unwrap();
    std::fs::create_dir_all(pages.join("b/pages")).unwrap();
    std::fs::write(
        pages.join("pages/index.html"),
        "<pre><A HREF=\"/\">[To Parent Directory]</A><br><br> Monday, October 12, 2026 10:45 AM &lt;dir&gt; <A HREF=\"/A/b/\">b</A><br> Monday, October 12, 2026 10:45 AM 9 <A HREF=\"/A/a.zip\">a.zip</A><br> Monday, October 12, 2026 10:45 AM &lt;dir&gt; <A HREF=\"/Z/\">Z</A><br></pre>",
    )
    .unwrap();
    std::fs::write(pages.join("b/pages/index.html"), "<pre></pre>").unwrap();

    let status = cli.ok(&["status", "--root", root]);
    assert!(
        status.contains("crawl in progress: 2 pages read"),
        "{status}"
    );

    let out = cli.ok(&["--offline", "crawl", "--root", root]);
    assert!(
        out.contains("1 folders, 1 files (0.0 GB), 1 links out of the roots; 2 pages read, 0 of them from nemweb"),
        "{out}"
    );
    let out = cli.ok(&["--offline", "--json", "crawl", "--root", root]);
    assert!(out.contains("\"already_done\":true"), "{out}");
    let out = cli.ok(&["--offline", "crawl", "--root", root]);
    assert!(out.contains("complete already"), "{out}");

    let file = cli.home.path().join("archive/h/A/a.zip");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, b"zip bytes").unwrap();
    let status = cli.ok(&["status", "--root", root]);
    assert!(status.contains("1/1"), "{status}");
    assert!(status.contains("1 links out of the roots"), "{status}");
    let status = cli.ok(&["--json", "status", "--root", root]);
    assert!(status.contains("\"archived_files\":1"), "{status}");

    // Fresh, offline: the first page cannot be asked for.
    let (code, out, _) = cli.code(&["--offline", "--json", "crawl", "--fresh", "--root", root]);
    assert_eq!(code, 1);
    assert!(out.contains("net.offline"), "{out}");
    let (code, _, err) = cli.code(&["crawl", "--root", "https://h/A"]);
    assert_eq!(code, 1);
    assert!(err.contains("config.mirror_root"), "{err}");
    let (code, _, _) = cli.code(&["status"]);
    assert_eq!(code, 0);
}

/// `run` crawls and downloads; offline, a file the archive lacks
/// fails and one it holds is counted.
#[test]
fn the_mirror_downloads_what_the_crawl_listed() {
    let cli = Cli::new();
    let root = "https://h/A/";
    let (code, _, err) = cli.code(&["--offline", "download"]);
    assert_eq!(code, 1);
    assert!(err.contains("mirror.crawl_incomplete"), "{err}");

    let pages = cli.home.path().join("state/folders/h/A");
    std::fs::create_dir_all(pages.join("pages")).unwrap();
    std::fs::write(
        pages.join("pages/index.html"),
        "<pre> Monday, October 12, 2026 10:45 AM 9 <A HREF=\"/A/a.zip\">a.zip</A><br></pre>",
    )
    .unwrap();
    let (code, out, err) = cli.code(&["--offline", "run", "--root", root]);
    assert_eq!(code, 1);
    assert!(
        out.contains(
            "1 files: 0 archived already, 0 removed as duplicates, 0 downloaded (0.0 GB), 1 failed"
        ),
        "{out}"
    );
    assert!(err.contains("net.offline"), "{err}");

    let dir = cli.home.path().join("archive/h/A");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.zip"), b"zip bytes").unwrap();
    std::fs::write(
        dir.join("a.zip.json"),
        format!(
            r#"{{"url":"https://h/A/a.zip","size":9,"sha256":"{}","fetched_at":"2026-10-15T10:00:05Z","tool":"nemlake"}}"#,
            utilake_mirror::net::archive::sha256_hex(b"zip bytes")
        ),
    )
    .unwrap();
    let out = cli.ok(&["--offline", "--json", "run", "--root", root]);
    assert!(out.contains("\"event\":\"file_done\""), "{out}");
    assert!(out.contains("\"cached\":1"), "{out}");
    let out = cli.ok(&["--offline", "download"]);
    assert!(out.contains("1 archived already"), "{out}");
    // The CRC-32 its download predates is recorded; then nothing is left.
    let out = cli.ok(&["--progress", "hash"]);
    assert!(
        out.contains("1 downloaded files: 1 read, 1 CRC-32s recorded, 0 not as recorded"),
        "{out}"
    );
    let out = cli.ok(&["--json", "hash", "--all"]);
    assert!(
        out.contains("\"event\":\"file_hashed\"") && out.contains("\"recorded\":0"),
        "{out}"
    );
    std::fs::write(dir.join("a.zip"), b"zip bytez").unwrap();
    let (code, _, err) = cli.code(&["hash", "--all"]);
    assert_eq!(code, 1);
    assert!(err.contains("cache.sha256_mismatch"), "{err}");
    std::fs::write(dir.join("a.zip"), b"zip bytes").unwrap();
    let status = cli.ok(&["status", "--root", root]);
    assert!(
        status.contains("mirror not running; crawl complete"),
        "{status}"
    );
}

use utilake_net::testserver::{Reply, Server};

/// An IIS listing of `(href, size)`: a size of `None` is a folder.
fn page(entries: &[(&str, Option<usize>)]) -> Vec<u8> {
    let mut body = String::from("<pre><A HREF=\"/\">[To Parent Directory]</A><br><br>");
    for (href, size) in entries {
        let what = size.map_or("&lt;dir&gt;".to_owned(), |s| s.to_string());
        let _ = write!(
            body,
            " Monday, October 12, 2026 10:45 AM {what} <A HREF=\"{href}\">x</A><br>"
        );
    }
    body.push_str("</pre>");
    body.into_bytes()
}

fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
    use std::io::Write as _;
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        w.start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        w.write_all(bytes).unwrap();
    }
    w.finish().unwrap().into_inner()
}

impl Cli {
    fn config(&self, text: &str) {
        std::fs::create_dir_all(self.home.path()).unwrap();
        std::fs::write(self.home.path().join("config.toml"), text).unwrap();
    }
}

const FAST: &str =
    "request_interval_ms = 0\nattempts = 2\nbackoff_ms = 1\nprogress_every_secs = 0\n";

/// A whole run against a local nemweb that refuses once and breaks a file
/// once, then the duplicate zip compared away, mapped and summarised.
#[test]
fn a_run_waits_out_failures_then_compare_map_and_summary() {
    let srv = Server::start();
    let t1 = zip_of(&[("T1.CSV", b"a,b\n")]);
    let whole = zip_of(&[("M/S/t1.zip", &t1)]);
    // N.zip holds a file its folder lacks, and its t2.zip is not the folder's.
    let t2 = zip_of(&[("T2.CSV", b"c,d\n")]);
    let other = zip_of(&[("T2.CSV", b"not c,d\n")]);
    let n_whole = zip_of(&[("N/t2.zip", &other), ("N/extra.txt", b"only here")]);
    srv.route("/A/N/", Reply::Ok(page(&[("/A/N/t2.zip", Some(t2.len()))])));
    srv.route("/A/N/t2.zip", Reply::Ok(t2.clone()));
    // Peeking N.zip fails on the network the first time (both tries).
    srv.route("/A/N.zip", Reply::Status(500));
    srv.route("/A/N.zip", Reply::Status(500));
    srv.route("/A/N.zip", Reply::Ok(n_whole.clone()));
    srv.route("/A/", Reply::Status(403));
    srv.route(
        "/A/",
        Reply::Ok(page(&[
            ("/A/M/", None),
            ("/A/M.zip", Some(whole.len())),
            ("/A/N/", None),
            ("/A/N.zip", Some(n_whole.len())),
            ("/A/x.txt", Some(5)),
        ])),
    );
    srv.route("/A/M/", Reply::Ok(page(&[("/A/M/S/", None)])));
    srv.route(
        "/A/M/S/",
        Reply::Ok(page(&[("/A/M/S/t1.zip", Some(t1.len()))])),
    );
    srv.route("/A/M/S/t1.zip", Reply::Ok(t1.clone()));
    srv.route("/A/M.zip", Reply::Ok(whole.clone()));
    for _ in 0..2 {
        srv.route(
            "/A/x.txt",
            Reply::Truncate {
                claimed: 5,
                sent: b"he".to_vec(),
            },
        );
    }
    srv.route("/A/x.txt", Reply::Ok(b"hello".to_vec()));
    let cli = Cli::new();
    cli.config(FAST);
    let root = srv.url("/A/");
    // Refused first; then the crawl, and nothing downloaded to compare yet.
    let (code, out, _) = cli.code(&["--json", "crawl", "--root", &root]);
    assert_eq!(code, 1);
    assert!(out.contains("net.blocked"), "{out}");
    let crawled = cli.run(&["--progress", "crawl", "--root", &root]);
    assert!(String::from_utf8_lossy(&crawled.stderr).contains("pages read"));
    let out = cli.ok(&["--json", "crawl", "--fresh", "--root", &root]);
    assert!(out.contains("\"event\":\"page_read\""), "{out}");
    let waiting = cli.ok(&["compare"]);
    assert!(
        waiting.contains("0 compared, 2 waiting for downloads"),
        "{waiting}"
    );
    let (code, out, _) = cli.code(&["--json", "peek"]);
    assert_eq!(code, 1);
    assert!(out.contains("\"event\":\"zip_peeked\""), "{out}");
    assert!(out.contains("net.retries_exhausted"), "{out}");
    let peeked = cli.run(&["--progress", "peek"]);
    assert!(peeked.status.success());
    assert!(String::from_utf8_lossy(&peeked.stderr).contains("1/1 zips peeked"));
    assert!(String::from_utf8_lossy(&peeked.stdout).contains("1 zips peeked: 0 archived, 1 read"));
    let out = cli.run(&["--progress", "run", "--wait-minutes", "0", "--root", &root]);
    let (stdout, stderr) = (
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(out.status.success(), "{stdout}{stderr}");
    assert!(stderr.contains("mirror.files_failed"), "{stderr}");
    assert!(stderr.contains("waiting 0 minutes"), "{stderr}");
    assert!(stderr.contains("/5 files"), "{stderr}");
    assert!(stdout.contains("0 zips peeked"), "{stdout}");
    // The run kept its counts; with no run, status counts for itself.
    let kept = std::fs::read_to_string(cli.home.path().join("state/cache/status.json")).unwrap();
    assert!(
        kept.contains("\"archived_files\"") && kept.contains("\"peek\""),
        "{kept}"
    );
    assert!(
        stdout.contains("5 files: 4 archived already, 0 removed as duplicates, 1 downloaded"),
        "{stdout}"
    );
    assert!(srv.hits().iter().filter(|h| *h == "/A/").count() >= 2);

    let compared = cli.run(&["compare"]);
    let (table, notes) = (
        String::from_utf8_lossy(&compared.stdout),
        String::from_utf8_lossy(&compared.stderr),
    );
    assert!(table.contains("M.zip") && table.contains("same"), "{table}");
    assert!(
        table.contains("N.zip") && table.contains("differs"),
        "{table}"
    );
    assert!(notes.contains("only in the zip: N/extra.txt"), "{notes}");
    assert!(notes.contains("differs from the folder: t2.zip"), "{notes}");
    let json = cli.ok(&["--json", "compare"]);
    assert!(json.contains("\"verdict\":\"same\""), "{json}");
    let deleted = cli.ok(&["compare", "--delete"]);
    assert!(deleted.contains("removed"), "{deleted}");
    assert!(
        deleted.contains("2 compared, 0 waiting for downloads"),
        "{deleted}"
    );
    let after = cli.ok(&["compare"]);
    assert!(after.contains("1 compared"), "{after}");
    // The removal's evidence, checked again; then a folder file damaged.
    let verified = cli.ok(&["compare", "--verify"]);
    assert!(
        verified.contains("holds") && verified.contains("1 removed zips checked, 0 no longer held"),
        "{verified}"
    );
    let t1 = utilake_mirror::net::Archive::new(cli.home.path().join("archive"))
        .path_for(&srv.url("/A/M/S/t1.zip"));
    let mut bytes = std::fs::read(&t1).unwrap();
    bytes[0] ^= 1;
    std::fs::write(&t1, &bytes).unwrap();
    let (code, out, err) = cli.code(&["compare", "--verify"]);
    assert_eq!(code, 1);
    assert!(out.contains("BROKEN"), "{out}");
    assert!(err.contains("is no longer the zip's"), "{err}");
    let (code, out, _) = cli.code(&["--json", "compare", "--verify"]);
    assert_eq!(code, 1);
    assert!(out.contains("\"event\":\"verified\""), "{out}");
    bytes[0] ^= 1;
    std::fs::write(&t1, &bytes).unwrap();
    let status = cli.ok(&["status", "--root", &root]);
    assert!(status.contains("(1 removed)"), "{status}");
    let again = cli.ok(&["download"]);
    assert!(
        again.contains("1 removed as duplicates, 0 downloaded"),
        "{again}"
    );

    let map = cli.ok(&["--jobs", "2", "map", "--root", &root]);
    assert!(map.contains("1 removed as duplicates"), "{map}");
    let json = cli.ok(&["--json", "map", "--root", &root]);
    assert!(json.contains("\"event\":\"map_done\""), "{json}");
    let md = cli.home.path().join("out/map.md");
    let md = md.to_str().unwrap();
    let (code, _, err) = cli.code(&["summary", "--out", md, "--check"]);
    assert_eq!(code, 1);
    assert!(err.contains("mirror.summary_stale"), "{err}");
    assert!(cli.ok(&["summary", "--out", md]).contains("lines written"));
    let text = std::fs::read_to_string(md).unwrap();
    assert!(
        text.contains("M.zip/ (zip)") || text.contains("M.zip  "),
        "{text}"
    );
    assert!(
        cli.ok(&["summary", "--out", md, "--check"])
            .contains("is the summary")
    );
    let json = cli.ok(&["--json", "summary", "--out", md, "--check"]);
    assert!(json.contains("\"checked\":true"), "{json}");
    // Nowhere to write it: under a file, or onto a folder.
    let under_file = format!("{md}/x.md");
    let (code, _, err) = cli.code(&["summary", "--out", &under_file]);
    assert_eq!((code, err.contains("mirror.io")), (1, true), "{err}");
    let onto_folder = cli.home.path().join("out");
    let (code, _, err) = cli.code(&["summary", "--out", onto_folder.to_str().unwrap()]);
    assert_eq!((code, err.contains("mirror.io")), (1, true), "{err}");
}

/// Waiting has its limits; JSON says what it waits for.
#[test]
fn refusals_in_a_row_end_the_run() {
    let srv = Server::start();
    srv.route("/B/", Reply::Status(403));
    let cli = Cli::new();
    cli.config(FAST);
    let root = srv.url("/B/");
    let (code, out, _) = cli.code(&[
        "--json",
        "run",
        "--refusals",
        "1",
        "--wait-minutes",
        "0",
        "--root",
        &root,
    ]);
    assert_eq!(code, 1);
    assert!(out.contains("\"event\":\"waiting\""), "{out}");
    assert_eq!(out.matches("net.blocked").count(), 2, "{out}");
    let (code, out, _) = cli.code(&["--json", "run", "--root", &srv.url("/C/")]);
    assert_eq!(code, 1);
    assert!(out.contains("net.http_status"), "{out}");
}

/// A page or a file that stops coming fails after the read timeout, each
/// try, and says so.
#[test]
fn what_stops_coming_times_out() {
    let srv = Server::start();
    srv.route("/S/", Reply::Stall);
    srv.route("/D/", Reply::Whole(page(&[("/D/f.zip", Some(10))])));
    srv.route(
        "/D/f.zip",
        Reply::StallBody {
            claimed: 10,
            sent: b"abc".to_vec(),
        },
    );
    let cli = Cli::new();
    cli.config(&format!("{FAST}read_timeout_secs = 1\n"));
    let (code, _, err) = cli.code(&["crawl", "--root", &srv.url("/S/")]);
    assert_eq!(code, 1);
    assert!(
        err.contains("net.retries_exhausted") && err.contains("timed out"),
        "{err}"
    );
    let cli = Cli::new();
    cli.config(&format!("{FAST}read_timeout_secs = 1\n"));
    cli.ok(&["crawl", "--root", &srv.url("/D/")]);
    let (code, out, err) = cli.code(&["download"]);
    assert_eq!(code, 1);
    assert!(out.contains("1 failed"), "{out}");
    assert!(err.contains("timed out after 1 seconds reading"), "{err}");
}

/// Another run holds the lock; the home or the config cannot be used.
#[test]
fn the_lock_the_home_and_the_config() {
    let cli = Cli::new();
    let mirror = utilake_mirror::Mirror::new(cli.home.path().join("state"));
    let held = mirror.lock().unwrap();
    let (code, _, err) = cli.code(&["--offline", "crawl", "--root", "https://h/A/"]);
    assert_eq!(code, 1);
    assert!(err.contains("mirror.locked"), "{err}");
    assert!(cli.ok(&["status"]).starts_with("mirror running"));
    // A run's kept counts are what status shows while it runs.
    let mut kept = utilake_mirror::Status {
        crawl_done: true,
        peek: Some(utilake_mirror::PeekCount { done: 2, total: 5 }),
        downloading: Some(1_500_000_000),
        ..utilake_mirror::Status::default()
    };
    kept.pages = 7;
    mirror.write_progress(&kept).unwrap();
    let out = cli.ok(&["status"]);
    assert!(
        out.contains("counts kept by the run, as of") && out.contains("zips peeked: 2/5"),
        "{out}"
    );
    assert!(out.contains("7 pages read"), "{out}");
    assert!(out.contains("downloading now: 1.50 GB so far"), "{out}");
    let full = cli.ok(&["status", "--full"]);
    assert!(!full.contains("kept by the run"), "{full}");
    drop(held);
    // No flags about progress: stderr is not a terminal, so no progress line.
    let plain = Command::new(env!("CARGO_BIN_EXE_utilake-mirror"))
        .arg("--home")
        .arg(cli.home.path())
        .arg("status")
        .output()
        .unwrap();
    assert!(plain.status.success() && plain.stderr.is_empty());

    cli.config("requests = 1\n");
    let (code, _, err) = cli.code(&["status"]);
    assert_eq!(code, 2);
    assert!(err.contains("config.parse"), "{err}");
    // A user agent no HTTP header can carry.
    cli.config("user_agent_suffix = \"\\n\"\n");
    let (code, _, err) = cli.code(&["status"]);
    assert_eq!(code, 2);
    assert!(err.contains("Connecting to"), "{err}");

    let out = Command::new(env!("CARGO_BIN_EXE_utilake-mirror"))
        .arg("status")
        .env_remove("UTILAKE_MIRROR_HOME")
        .env_remove("HOME")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("No home directory"));
}

/// A whole-month zip whose listing matches its folder is removed before it
/// is ever downloaded, and the download then leaves it alone.
#[test]
fn a_duplicate_is_ruled_on_from_its_listing_and_never_downloaded() {
    let cli = Cli::new();
    let root = "https://h/A/";
    let pages = cli.home.path().join("state/folders/h/A");
    std::fs::create_dir_all(pages.join("pages")).unwrap();
    std::fs::create_dir_all(pages.join("M/pages")).unwrap();
    let t = zip_of(&[("T.CSV", b"a\n")]);
    std::fs::write(
        pages.join("pages/index.html"),
        page(&[("/A/M/", None), ("/A/M.zip", Some(20_000_000))]),
    )
    .unwrap();
    std::fs::write(
        pages.join("M/pages/index.html"),
        page(&[("/A/M/t.zip", Some(t.len()))]),
    )
    .unwrap();
    cli.ok(&["--offline", "crawl", "--root", root]);
    let dir = cli.home.path().join("archive/h/A/M");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("t.zip"), &t).unwrap();
    std::fs::write(
        dir.join("t.zip.json"),
        format!(
            r#"{{"url":"https://h/A/M/t.zip","size":{},"sha256":"{}","fetched_at":"2026-10-05T00:00:00Z","tool":"x"}}"#,
            t.len(),
            utilake_mirror::net::archive::sha256_hex(&t)
        ),
    )
    .unwrap();
    let crc = {
        let mut h = crc32fast::Hasher::new();
        h.update(&t);
        h.finalize()
    };
    std::fs::write(
        cli.home.path().join("state/peeks.ndjson"),
        format!(
            r#"{{"url":"https://h/A/M.zip","peeked_at":"2026-10-15T00:00:00Z","etag":"\"e1\"","last_modified":"Mon, 12 Oct 2026 00:32:30 GMT","listed_size":20000000,"listed_modified":"Monday, October 12, 2026 10:45 AM","entries":[{{"name":"M/t.zip","size":{},"compressed":{},"crc32":{crc},"method":0,"modified":"m"}}]}}"#,
            t.len(),
            t.len()
        ) + "\n",
    )
    .unwrap();
    let out = cli.ok(&["compare", "--delete"]);
    assert!(out.contains("listing") && out.contains("removed"), "{out}");
    assert!(
        out.contains("0.0 GB freed, 0.0 GB not to download"),
        "{out}"
    );
    let removed = std::fs::read_to_string(cli.home.path().join("state/removals.ndjson")).unwrap();
    assert!(removed.contains("\"downloaded\":false"), "{removed}");
    let out = cli.ok(&["--offline", "download"]);
    assert!(
        out.contains("1 archived already, 1 removed as duplicates"),
        "{out}"
    );
}

/// A run peeks the zips beside a folder of the same name and downloads,
/// together, and keeps its counts as it goes.
#[test]
fn a_run_keeps_its_counts_for_status() {
    let srv = Server::start();
    let z = zip_of(&[("a.csv", b"1\n")]);
    let other = zip_of(&[("b.csv", b"2\n")]);
    // A file big enough that the fetcher reports its progress on the way.
    let big = vec![7u8; 9 << 20];
    srv.route(
        "/Z/",
        Reply::Ok(page(&[
            ("/Z/z/", None),
            ("/Z/z.zip", Some(z.len())),
            ("/Z/other.zip", Some(other.len())),
            ("/Z/big.bin", Some(big.len())),
        ])),
    );
    srv.route("/Z/z/", Reply::Ok(page(&[("/Z/z/a.csv", Some(2))])));
    srv.route("/Z/z/a.csv", Reply::Ok(b"1\n".to_vec()));
    srv.route("/Z/z.zip", Reply::Ok(z.clone()));
    srv.route("/Z/other.zip", Reply::Ok(other));
    srv.route("/Z/big.bin", Reply::Ok(big));
    let cli = Cli::new();
    cli.config(FAST);
    cli.ok(&["run", "--delete-duplicates", "--root", &srv.url("/Z/")]);
    let kept = std::fs::read_to_string(cli.home.path().join("state/cache/status.json")).unwrap();
    // Only the zip beside its folder is peeked: it is found the same as the
    // folder and not downloaded; the other zip is downloaded unpeeked.
    assert!(kept.contains("\"peek\":{\"done\":1,\"total\":1}"), "{kept}");
    assert!(kept.contains("\"archived_files\":3"), "{kept}");
    let removed = std::fs::read_to_string(cli.home.path().join("state/removals.ndjson")).unwrap();
    assert!(removed.contains("/Z/z.zip"), "{removed}");
    assert!(kept.contains("\"downloading\":0"), "{kept}");
}

/// An IIS listing of `(href, when, size)`: a size of `None` is a folder.
fn dated(entries: &[(&str, &str, Option<usize>)]) -> Vec<u8> {
    let mut body = String::from("<pre><A HREF=\"/\">[To Parent Directory]</A><br><br>");
    for (href, when, size) in entries {
        let what = size.map_or("&lt;dir&gt;".to_owned(), |s| s.to_string());
        let _ = write!(body, " {when} {what} <A HREF=\"{href}\">x</A><br>");
    }
    body.push_str("</pre>");
    body.into_bytes()
}

/// `watch` reads each folder again, downloads a file rewritten in place
/// as a later version, rules on a whole-month zip new to nemweb from its
/// listing, waits out a missing archive and refusals, and stops on a page
/// it cannot read.
#[test]
fn a_watch_keeps_each_version_and_what_is_new() {
    const MON: &str = "Monday, October 12, 2026 10:45 AM";
    const TUE: &str = "Tuesday, October 13, 2026 11:53 AM";
    const WED: &str = "Wednesday, October 14, 2026 09:00 AM";
    let srv = Server::start();
    let t = b"t,u\n".to_vec();
    let whole = zip_of(&[("M/t.csv", &t)]);
    srv.route(
        "/W/",
        Reply::Ok(dated(&[("/W/G/", MON, None), ("/W/M/", MON, None)])),
    );
    srv.route(
        "/W/",
        Reply::Ok(dated(&[
            ("/W/G/", MON, None),
            ("/W/M/", MON, None),
            ("/W/M.zip", TUE, Some(whole.len())),
        ])),
    );
    srv.route(
        "/W/M/",
        Reply::Ok(dated(&[("/W/M/t.csv", MON, Some(t.len()))])),
    );
    srv.route("/W/M/t.csv", Reply::Ok(t.clone()));
    srv.route("/W/M.zip", Reply::Ok(whole.clone()));
    for (when, size) in [(MON, 3), (TUE, 4), (TUE, 4), (WED, 5)] {
        srv.route(
            "/W/G/",
            Reply::Ok(dated(&[("/W/G/r.csv", when, Some(size))])),
        );
    }
    for body in [&b"one"[..], b"four", b"five!"] {
        srv.route("/W/G/r.csv", Reply::Ok(body.to_vec()));
    }
    let cli = Cli::new();
    let base = srv.url("");
    // Every folder read in every round, so each round's answers are known.
    let every = |secs: u64, least: u64| {
        format!(
            "{FAST}base_url = \"{base}\"\nwatch_fast = [\"W/G/\"]\nwatch_current = [\"W/\"]\nwatch_fast_secs = {secs}\nwatch_current_secs = {secs}\nwatch_secs = {secs}\nwatch_least_secs = {least}\n"
        )
    };
    cli.config(&every(0, 0));
    let root = srv.url("/W/");
    let (code, out, _) = cli.code(&["--json", "watch", "--root", &root]);
    assert_eq!(code, 1);
    assert!(out.contains("mirror.crawl_incomplete"), "{out}");
    cli.ok(&["run", "--root", &root]);
    // A file of a folder that does not change, gone from the archive (a
    // download cut off), is fetched again when its folder is read.
    let archive = cli.home.path().join("archive");
    let host = srv
        .url("/W/")
        .split_once("://")
        .unwrap()
        .1
        .replace(':', "_");
    let t_csv = archive.join(&host).join("M/t.csv");
    std::fs::remove_file(&t_csv).unwrap();

    let out = cli.ok(&[
        "--json",
        "watch",
        "--root",
        &root,
        "--rounds",
        "2",
        "--delete-duplicates",
    ]);
    assert_eq!(out.matches("\"event\":\"watch_round\"").count(), 2, "{out}");
    assert!(out.contains("\"event\":\"zip_peeked\""), "{out}");
    assert_eq!(std::fs::read(&t_csv).unwrap(), t);
    let g = archive.join(&host).join("G");
    assert_eq!(std::fs::read(g.join("r.csv")).unwrap(), b"one");
    assert_eq!(
        std::fs::read(g.join("r.csv@20261013T115300+1000")).unwrap(),
        b"four"
    );
    let removed = std::fs::read_to_string(cli.home.path().join("state/removals.ndjson")).unwrap();
    assert!(removed.contains("M.zip"), "{removed}");

    let out = cli.ok(&["watch", "--root", &root, "--rounds", "1"]);
    assert!(
        out.contains("1 changed, 0 added, 0 gone; 1 files new or rewritten"),
        "{out}"
    );
    assert_eq!(
        std::fs::read(g.join("r.csv@20261014T090000+1000")).unwrap(),
        b"five!"
    );

    // A version the same as one before it, as kept before a download
    // compared with every version: found, then pruned.
    let dup = g.join("r.csv@20261015T090000+1000");
    std::fs::copy(g.join("r.csv@20261014T090000+1000"), &dup).unwrap();
    std::fs::copy(
        g.join("r.csv@20261014T090000+1000.provenance.json"),
        g.join("r.csv@20261015T090000+1000.provenance.json"),
    )
    .unwrap();
    let out = cli.ok(&["versions"]);
    assert!(out.contains("1 files with 4 versions: 1 the same bytes as one before, 0 zips packed again; 0 removed"), "{out}");
    let out = cli.ok(&["--json", "versions", "--prune"]);
    assert!(out.contains("\"event\":\"version_same\""), "{out}");
    assert!(out.contains("\"removed\":1"), "{out}");
    assert!(!dup.exists());
    let journal = std::fs::read_to_string(
        cli.home
            .path()
            .join("state/folders")
            .join(&host)
            .join("G/events.ndjson"),
    )
    .unwrap();
    assert!(journal.contains("\"event\":\"same\""), "{journal}");

    // Between rounds, the watch sleeps until a folder is due.
    cli.config(&every(1, 0));
    let out = cli.ok(&["--json", "watch", "--root", &root, "--rounds", "2"]);
    assert_eq!(out.matches("\"event\":\"watch_round\"").count(), 2, "{out}");

    // An archive linked to a drive not mounted is waited for.
    let away = cli.home.path().join("archive.away");
    std::fs::rename(&archive, &away).unwrap();
    std::os::unix::fs::symlink(cli.home.path().join("unmounted"), &archive).unwrap();
    let (code, _, err) = cli.code(&[
        "watch",
        "--root",
        &root,
        "--rounds",
        "1",
        "--wait-minutes",
        "0",
    ]);
    assert_eq!(code, 0);
    assert!(err.contains("archive_missing"), "{err}");
    std::fs::remove_file(&archive).unwrap();
    std::fs::rename(&away, &archive).unwrap();

    // A folder gone and one new; then refusals in a row end the watch. A
    // page that does not read stops it.
    srv.route("/X/", Reply::Ok(dated(&[("/X/d/", MON, None)])));
    srv.route("/X/", Reply::Ok(dated(&[("/X/n/", TUE, None)])));
    srv.route("/X/", Reply::Status(403));
    srv.route("/X/d/", Reply::Ok(dated(&[("/X/d/a.csv", MON, Some(1))])));
    srv.route("/X/d/", Reply::Status(404));
    srv.route("/X/n/", Reply::Ok(dated(&[("/X/n/b.csv", TUE, Some(1))])));
    srv.route("/X/n/b.csv", Reply::Ok(b"b".to_vec()));
    srv.route("/Y/", Reply::Ok(dated(&[("/Y/a.csv", MON, Some(1))])));
    srv.route(
        "/Y/",
        Reply::Ok(b"<pre> not a listing line <br></pre>".to_vec()),
    );
    for (r, code, what) in [
        ("/X/", 1, "net.blocked"),
        ("/Y/", 1, "listing.unparsed_line"),
    ] {
        let cli = Cli::new();
        cli.config(&format!(
            "{FAST}base_url = \"{base}\"\nwatch_secs = 0\nwatch_least_secs = 0\n"
        ));
        let root = srv.url(r);
        cli.ok(&["crawl", "--root", &root]);
        let (c, out, _) = cli.code(&[
            "--json",
            "watch",
            "--root",
            &root,
            "--refusals",
            "1",
            "--wait-minutes",
            "0",
        ]);
        assert_eq!(c, code, "{out}");
        assert!(out.contains(what), "{out}");
        if r == "/X/" {
            assert!(out.contains("\"added\":[\""), "{out}");
            assert!(out.contains("\"removed\":[\""), "{out}");
            assert!(out.contains("\"event\":\"waiting\""), "{out}");
        }
    }
}

/// Relayout moves both listings of a report folder into one, says what it
/// would do first, and fails when a sidecar does not read.
#[test]
fn relayout_moves_both_listings_into_one_folder() {
    let cli = Cli::new();
    let archive = cli.home.path().join("archive/www.nemweb.com.au/Reports");
    let put = |listing: &str, name: &str, bytes: &[u8]| {
        let dir = archive.join(listing).join("X");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), bytes).unwrap();
        let url = format!("https://www.nemweb.com.au/Reports/{listing}/X/{name}");
        let sidecar = format!(
            "{{\"url\":\"{url}\",\"size\":{},\"sha256\":\"{}\",\"fetched_at\":\"2026-10-05T00:00:00Z\"}}",
            bytes.len(),
            utilake_mirror::net::archive::sha256_hex(bytes)
        );
        std::fs::write(dir.join(format!("{name}.json")), sidecar).unwrap();
    };
    put("CURRENT", "a.csv", b"a");
    put("CURRENT", "m.zip", b"partial");
    put("ARCHIVE", "m.zip", b"final");
    put("ARCHIVE", "s.csv", b"s");
    put("CURRENT", "s.csv", b"s");
    let dry = cli.ok(&["relayout", "--dry-run"]);
    assert!(
        dry.contains("dry run: 1 folders moved whole, 1 moved, 1 kept as versions"),
        "{dry}"
    );
    assert!(archive.join("CURRENT/X/a.csv").exists());
    let json = cli.ok(&["--json", "relayout", "--dry-run"]);
    assert!(
        json.contains("\"relayout_step\"") && json.contains("\"relayout_done\""),
        "{json}"
    );
    let done = cli.run(&["relayout"]);
    assert!(done.status.success());
    let out = String::from_utf8_lossy(&done.stdout);
    assert!(
        out.contains("1 folders moved whole, 1 moved, 1 kept as versions, 1 the same"),
        "{out}"
    );
    assert!(String::from_utf8_lossy(&done.stderr).contains("kept as a version"));
    assert!(archive.join("X/a.csv").exists());
    // A sidecar that does not read: left, and the run says so.
    std::fs::create_dir_all(archive.join("CURRENT/X")).unwrap();
    std::fs::write(archive.join("CURRENT/X/b.csv"), b"b").unwrap();
    std::fs::write(archive.join("CURRENT/X/b.csv.json"), b"{").unwrap();
    let (code, out, _) = cli.code(&["relayout"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("1 unreadable"), "{out}");
}

/// A home in the layout before `state/` is refused until it is upgraded;
/// the upgrade moves its records, says so, and leaves the bytes alone.
#[test]
fn a_home_before_state_is_upgraded_first() {
    let cli = Cli::new();
    let old = cli.home.path().join("mirror");
    std::fs::create_dir_all(old.join("crawl")).unwrap();
    std::fs::write(old.join("config.toml"), "request_interval_ms = 750\n").unwrap();
    std::fs::write(old.join("peek.ndjson"), "").unwrap();
    std::fs::write(old.join("mine.txt"), "kept").unwrap();
    let line = r#"{"url":"https://h/A/a.zip","kind":"file","size":2,"modified":"Monday, October 12, 2026 10:45 AM","listed_on":"https://h/A/"}"#;
    std::fs::write(old.join("crawl/remote.ndjson"), format!("{line}\n")).unwrap();
    let data = cli.home.path().join("archive/h/A/a.zip");
    std::fs::create_dir_all(data.parent().unwrap()).unwrap();
    std::fs::write(&data, b"aa").unwrap();
    let v1 = format!(
        r#"{{"url":"https://h/A/a.zip","size":2,"sha256":"{}","fetched_at":"2026-10-20T00:00:00Z"}}"#,
        utilake_mirror::net::archive::sha256_hex(b"aa")
    );
    std::fs::write(cli.home.path().join("archive/h/A/a.zip.json"), v1).unwrap();
    std::fs::write(cli.home.path().join("archive/h/A/b.zip"), b"b").unwrap();
    std::fs::write(cli.home.path().join("archive/h/A/b.zip.json"), "{").unwrap();

    let (code, _, err) = cli.code(&["status"]);
    assert_eq!(code, 1);
    assert!(err.contains("mirror.old_layout"), "{err}");
    // Exit 1 all the same: one record does not read.
    let (code, dry, _) = cli.code(&["upgrade", "--dry-run"]);
    assert_eq!(code, 1);
    assert!(
        dry.starts_with(
            "dry run: 2 records moved, 1 listing files written, 1 provenance records written again"
        ),
        "{dry}"
    );
    let json = cli.code(&["--json", "upgrade", "--dry-run"]).1;
    assert!(
        json.contains("\"upgrade_step\"") && json.contains("\"upgrade_done\""),
        "{json}"
    );
    // A folder past the cap keeps its records, and is named.
    let capped = cli.code(&["upgrade", "--dry-run", "--max-entries", "1"]).1;
    assert!(
        capped.contains("0 provenance records written again")
            && capped.contains("skipped: ")
            && capped.contains("entries)"),
        "{capped}"
    );
    let (code, out, _) = cli.code(&["upgrade"]);
    assert_eq!(code, 1, "one record does not read: {out}");
    assert!(out.contains("left: ") && out.contains("mine.txt"), "{out}");
    assert!(cli.home.path().join("config.toml").exists());
    assert!(
        cli.home
            .path()
            .join("archive/h/A/a.zip.provenance.json")
            .exists()
    );
    assert_eq!(std::fs::read(&data).unwrap(), b"aa");
    let again = cli.code(&["upgrade", "--skip-records"]).1;
    assert!(
        again.starts_with("0 records moved, 0 listing files written, 0"),
        "{again}"
    );
    cli.ok(&["status"]);
    // The held cache written again from the records.
    assert!(cli.ok(&["reindex"]).starts_with("1 held lines written"));
    assert!(cli.ok(&["--json", "reindex"]).contains("\"reindex_done\""));
}
