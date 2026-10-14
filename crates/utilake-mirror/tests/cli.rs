//! The binary end to end in a temp home, offline: every command, human and
//! JSON output, exit codes.

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
