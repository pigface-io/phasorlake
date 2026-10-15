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
        Command::new(env!("CARGO_BIN_EXE_nemweb-mirror"))
            .arg("--home")
            .arg(self.home.path())
            .arg("--no-progress")
            .args(args)
            .env_remove("NEMWEB_MIRROR_HOME")
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
    let pages = cli.home.path().join("mirror/crawl/listings/h/A");
    std::fs::create_dir_all(pages.join("b")).unwrap();
    std::fs::write(
        pages.join("index.html"),
        "<pre><A HREF=\"/\">[To Parent Directory]</A><br><br> Monday, October 12, 2026 10:45 AM &lt;dir&gt; <A HREF=\"/A/b/\">b</A><br> Monday, October 12, 2026 10:45 AM 9 <A HREF=\"/A/a.zip\">a.zip</A><br> Monday, October 12, 2026 10:45 AM &lt;dir&gt; <A HREF=\"/Z/\">Z</A><br></pre>",
    )
    .unwrap();
    std::fs::write(pages.join("b/index.html"), "<pre></pre>").unwrap();

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

    let pages = cli.home.path().join("mirror/crawl/listings/h/A");
    std::fs::create_dir_all(&pages).unwrap();
    std::fs::write(
        pages.join("index.html"),
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
            nemweb_mirror::net::archive::sha256_hex(b"zip bytes")
        ),
    )
    .unwrap();
    let out = cli.ok(&["--offline", "--json", "run", "--root", root]);
    assert!(out.contains("\"event\":\"file_done\""), "{out}");
    assert!(out.contains("\"cached\":1"), "{out}");
    let out = cli.ok(&["--offline", "download"]);
    assert!(out.contains("1 archived already"), "{out}");
    let status = cli.ok(&["status", "--root", root]);
    assert!(
        status.contains("mirror not running; crawl complete"),
        "{status}"
    );
}

#[path = "../src/net/testserver.rs"]
mod server;

use server::{Reply, Server};

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
        std::fs::create_dir_all(self.home.path().join("mirror")).unwrap();
        std::fs::write(self.home.path().join("mirror/config.toml"), text).unwrap();
    }
}

const FAST: &str = "request_interval_ms = 0\nattempts = 2\nbackoff_ms = 1\n";

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
    assert!(String::from_utf8_lossy(&peeked.stdout).contains("1 zips peeked: 1 read"));
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
    let mirror = nemweb_mirror::Mirror::new(cli.home.path().join("mirror"));
    let held = mirror.lock().unwrap();
    let (code, _, err) = cli.code(&["--offline", "crawl", "--root", "https://h/A/"]);
    assert_eq!(code, 1);
    assert!(err.contains("mirror.locked"), "{err}");
    assert!(cli.ok(&["status"]).starts_with("mirror running"));
    drop(held);
    // No flags about progress: stderr is not a terminal, so no progress line.
    let plain = Command::new(env!("CARGO_BIN_EXE_nemweb-mirror"))
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

    let out = Command::new(env!("CARGO_BIN_EXE_nemweb-mirror"))
        .arg("status")
        .env_remove("NEMWEB_MIRROR_HOME")
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
    let pages = cli.home.path().join("mirror/crawl/listings/h/A");
    std::fs::create_dir_all(pages.join("M")).unwrap();
    let t = zip_of(&[("T.CSV", b"a\n")]);
    std::fs::write(
        pages.join("index.html"),
        page(&[("/A/M/", None), ("/A/M.zip", Some(20_000_000))]),
    )
    .unwrap();
    std::fs::write(
        pages.join("M/index.html"),
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
            r#"{{"url":"https://h/A/M/t.zip","size":{},"sha256":"x","fetched_at":"t","tool":"x"}}"#,
            t.len()
        ),
    )
    .unwrap();
    let crc = {
        let mut h = crc32fast::Hasher::new();
        h.update(&t);
        h.finalize()
    };
    std::fs::write(
        cli.home.path().join("mirror/peek.ndjson"),
        format!(
            r#"{{"url":"https://h/A/M.zip","entries":[{{"name":"M/t.zip","size":{},"compressed":{},"crc32":{crc},"method":0,"modified":"m"}}]}}"#,
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
    let removed = std::fs::read_to_string(cli.home.path().join("mirror/removed.ndjson")).unwrap();
    assert!(removed.contains("\"downloaded\":false"), "{removed}");
    let out = cli.ok(&["--offline", "download"]);
    assert!(
        out.contains("1 archived already, 1 removed as duplicates"),
        "{out}"
    );
}
