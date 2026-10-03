//! The binary end to end over the fixtures: every subcommand, human and
//! JSON output, exit codes.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/csv")
}

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
        Command::new(env!("CARGO_BIN_EXE_nemlake"))
            .arg("--home")
            .arg(self.home.path())
            .arg("--no-progress")
            .args(args)
            .env_remove("NEMLAKE_HOME")
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

fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
    use std::io::Write;
    let f = std::fs::File::create(path).unwrap();
    let mut w = zip::ZipWriter::new(f);
    for (name, bytes) in entries {
        w.start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        w.write_all(bytes).unwrap();
    }
    w.finish().unwrap();
}

#[test]
fn ingest_ls_errors_verify_round_trip() {
    let cli = Cli::new();
    let scada = fixtures().join("DISPATCH_UNIT_SCADA_v1_head.csv");
    let zip_path = cli.home.path().join("PUBLIC_DVD_REGION_202401010000.zip");
    make_zip(
        &zip_path,
        &[(
            "PUBLIC_DVD_REGION_202401010000.CSV",
            &std::fs::read(fixtures().join("REGION_v1_head.csv")).unwrap(),
        )],
    );
    let out = cli.ok(&[
        "ingest",
        scada.to_str().unwrap(),
        zip_path.to_str().unwrap(),
    ]);
    assert!(out.contains("2 listed, 2 ingested"), "{out}");
    // Idempotent.
    let out = cli.ok(&["ingest", scada.to_str().unwrap()]);
    assert!(out.contains("1 skipped"), "{out}");
    let ls = cli.ok(&["ls"]);
    assert!(
        ls.contains("DISPATCH_UNIT_SCADA") && ls.contains("REGION"),
        "{ls}"
    );
    let ls_json = cli.ok(&["--json", "ls", "--table", "REGION"]);
    assert!(ls_json.contains("\"table\":\"REGION\""), "{ls_json}");
    let runs = cli.ok(&["ls", "--runs"]);
    assert!(runs.contains("ingest"), "{runs}");
    let runs_json = cli.ok(&["--json", "ls", "--runs"]);
    assert!(runs_json.contains("\"exit_code\":0"), "{runs_json}");
    assert!(cli.ok(&["errors"]).contains("no diagnostics"));
    assert!(cli.ok(&["--json", "errors", "--last"]).is_empty());
    let verify = cli.ok(&["verify"]);
    assert!(verify.contains("verified, no problems"), "{verify}");
    let verify_json = cli.ok(&["--json", "verify", "--table", "REGION"]);
    assert!(verify_json.contains("\"problems\":[]"), "{verify_json}");
    // Snapshot partition for REGION, month partition for SCADA.
    let lake = cli.home.path().join("lake/nem");
    assert!(lake.join("region/snapshot=202401").is_dir());
    assert!(lake.join("dispatch_unit_scada/year=2024/month=01").is_dir());
    // A directory argument ingests its zips and CSVs; an empty one lists nothing.
    let empty = cli.home.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let out = cli.ok(&["ingest", empty.to_str().unwrap()]);
    assert!(out.contains("0 listed"), "{out}");
    // Delete a Parquet file: verify reports it.
    let mut files: Vec<PathBuf> = walk(&lake);
    files.sort();
    std::fs::remove_file(&files[0]).unwrap();
    let (code, out, _) = cli.code(&["verify"]);
    assert_eq!(code, 1);
    assert!(out.contains("verify.missing_file"), "{out}");
}

#[test]
fn bad_rows_fail_and_are_explained() {
    let cli = Cli::new();
    let bad = cli.home.path().join("bad.csv");
    std::fs::write(&bad, b"I,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID,SCADAVALUE\r\nD,DISPATCH,UNIT_SCADA,1,\"2024/13/01 00:05:00\",A,1\r\nD,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:05:00\",B,1\r\nC,\"END OF REPORT\",4\r\n").unwrap();
    let (code, out, err) = cli.code(&["ingest", bad.to_str().unwrap()]);
    assert_eq!(code, 1, "{out}{err}");
    assert!(out.contains("csv.too_many_bad_rows"), "{out}");
    let quarantine = walk(&cli.home.path().join("quarantine"));
    assert_eq!(quarantine.len(), 1, "{quarantine:?}");
    assert!(
        std::fs::read_to_string(&quarantine[0])
            .unwrap()
            .contains("parse.timestamp"),
        "{err}"
    );
    let errors = cli.ok(&["errors", "--last"]);
    assert!(errors.contains("csv.too_many_bad_rows"), "{errors}");
    let errors_json = cli.ok(&[
        "--json",
        "errors",
        "--code",
        "csv.too_many_bad_rows",
        "--errors-only",
    ]);
    assert!(
        errors_json.contains("\"code\":\"csv.too_many_bad_rows\""),
        "{errors_json}"
    );
    let failed = cli.ok(&["ls", "--failed"]);
    assert!(failed.contains("failed"), "{failed}");
    // Tolerating the row succeeds, in JSON too.
    let cli2 = Cli::new();
    let (code, out, _) = cli2.code(&[
        "--json",
        "ingest",
        bad.to_str().unwrap(),
        "--max-bad-rows",
        "5",
    ]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("\"type\":\"run\""), "{out}");
    // Strict mode refuses a footer mismatch; fail-fast stops at the first bad row.
    let strict = cli.home.path().join("strict.csv");
    std::fs::write(&strict, b"I,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID,SCADAVALUE\r\nD,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:05:00\",B,1\r\nC,\"END OF REPORT\",99\r\n").unwrap();
    let (code, out, _) = cli.code(&["ingest", strict.to_str().unwrap(), "--strict"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("csv.footer_count_mismatch"), "{out}");
    // Fail-fast stops at the first bad row even when the tolerance would allow it.
    let (code, out, _) = cli.code(&[
        "ingest",
        bad.to_str().unwrap(),
        "--fail-fast",
        "--max-bad-rows",
        "5",
    ]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("csv.stopped_fail_fast"), "{out}");
}

/// The head of every zip of the January 2024 archive: each one must resolve
/// exactly against the registry (strict mode refuses drift) and every row
/// must parse into its declared types, as floats and as exact decimals.
#[test]
fn every_archive_table_ingests_strictly() {
    let heads =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/bootstrap/MMSDM_2024_01/heads");
    let files = std::fs::read_dir(&heads).unwrap().count();
    assert!(files >= 161, "{files} head fixtures");
    for mode in [&[][..], &["--decimal"][..]] {
        let cli = Cli::new();
        let mut args: Vec<&str> = mode.to_vec();
        args.extend(["--json", "ingest", heads.to_str().unwrap(), "--strict"]);
        let (code, out, err) = cli.code(&args);
        assert_eq!(
            code,
            0,
            "{mode:?}: {err}{}",
            &out[out.len().saturating_sub(2000)..]
        );
        let ls = cli.ok(&["--json", "ls", "--limit", "1000"]);
        let rows: Vec<&str> = ls.lines().collect();
        assert_eq!(rows.len(), files, "{mode:?}: one ingest per head");
        for r in &rows {
            assert!(
                r.contains("\"status\":\"ok\"") && r.contains("\"bad_rows\":0"),
                "{mode:?}: {r}"
            );
        }
        let errors = cli.ok(&["--json", "errors", "--limit", "1000"]);
        assert!(
            !errors.contains("\"parse."),
            "{mode:?}: no parse diagnostics: {errors}"
        );
        // The only schema diagnostic allowed is the note that a column without
        // a usable precision is a float in decimal mode.
        for line in errors.lines().filter(|l| l.contains("\"schema.")) {
            assert!(
                !mode.is_empty() && line.contains("schema.decimal_fallback"),
                "{mode:?}: {line}"
            );
        }
    }
}

#[test]
fn inspect_and_schema_commands() {
    let cli = Cli::new();
    let multi = fixtures().join("multi_block_dispatchis.csv");
    let (code, out, _) = cli.code(&["inspect", multi.to_str().unwrap()]);
    assert_eq!(code, 5, "unknown tables make inspect exit 5: {out}");
    assert!(
        out.contains("DISPATCH.PRICE")
            && out.contains("exact")
            && out.contains("schema.unknown_table"),
        "{out}"
    );
    let (code, out, _) = cli.code(&["--json", "inspect", multi.to_str().unwrap(), "--infer"]);
    assert_eq!(code, 5);
    assert!(
        out.contains("\"drafts\":[{") && out.contains("DISPATCH_LOCAL_PRICE"),
        "{out}"
    );
    let (code, out, _) = cli.code(&["inspect", multi.to_str().unwrap(), "--infer"]);
    assert_eq!(code, 5);
    assert!(out.contains("# draft for DISPATCH_LOCAL_PRICE"), "{out}");
    let zip_path = cli.home.path().join("PUBLIC_DVD_REGION_202401010000.zip");
    make_zip(
        &zip_path,
        &[
            (
                "PUBLIC_DVD_REGION_202401010000.CSV",
                &std::fs::read(fixtures().join("REGION_v1_head.csv")).unwrap(),
            ),
            ("readme.txt", b"x"),
        ],
    );
    let (code, out, err) = cli.code(&["inspect", zip_path.to_str().unwrap()]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("MARKET_CONFIG.REGION"), "{out}");
    assert!(err.contains("skipped 1 non-CSV entry"), "{err}");
    let (code, _, err) = cli.code(&[
        "inspect",
        cli.home.path().join("nope.zip").to_str().unwrap(),
    ]);
    assert_eq!(code, 1);
    assert!(err.contains("zip.open_failed"), "{err}");

    let list = cli.ok(&["schema", "list"]);
    assert!(
        list.contains("DISPATCH_UNIT_SCADA") && list.contains("report pairs"),
        "{list}"
    );
    assert!(
        cli.ok(&["--json", "schema", "list"])
            .contains("\"name\":\"DISPATCH_UNIT_SCADA\"")
    );
    let show = cli.ok(&["schema", "show", "dispatch_unit_scada"]);
    assert!(
        show.contains("name = \"DISPATCH_UNIT_SCADA\"") && show.contains("# Arrow lake schema"),
        "{show}"
    );
    assert!(
        cli.ok(&["--json", "schema", "show", "REGION"])
            .contains("\"report_pairs\"")
    );
    let (code, _, err) = cli.code(&["schema", "show", "NOPE"]);
    assert_eq!(code, 1);
    assert!(err.contains("no table NOPE"), "{err}");
    let diff = cli.ok(&["schema", "diff", "DISPATCHPRICE", "4", "5"]);
    assert!(diff.contains("added column"), "{diff}");
    assert!(
        cli.ok(&["schema", "diff", "DISPATCHPRICE", "4", "4"])
            .contains("no changes")
    );
    assert!(
        cli.ok(&["--json", "schema", "diff", "DISPATCHPRICE", "4", "5"])
            .contains("added column")
    );
    assert_eq!(
        cli.code(&["schema", "diff", "DISPATCHPRICE", "4", "99"]).0,
        1
    );
    let lineage = cli.ok(&["schema", "lineage", "DISPATCHPRICE"]);
    assert!(
        lineage.contains("version") && lineage.contains('5'),
        "{lineage}"
    );
    assert!(
        cli.ok(&["--json", "schema", "lineage", "DISPATCHPRICE"])
            .starts_with('[')
    );
    assert_eq!(cli.code(&["schema", "lineage", "NOPE"]).0, 1);
    assert!(cli.ok(&["schema", "validate"]).contains("valid"));
    let schemas = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas");
    let (code, out, _) = cli.code(&["schema", "validate", schemas.to_str().unwrap()]);
    assert_eq!(code, 0, "{out}");
    let (code, _, err) = cli.code(&[
        "schema",
        "validate",
        cli.home.path().join("none").to_str().unwrap(),
    ]);
    assert_eq!(code, 5);
    assert!(err.contains("schema."), "{err}");
}

/// `schema infer` drafts what a file adds; a draft renamed into
/// `{home}/schemas/` is in effect for every command, without a rebuild.
#[test]
fn schema_infer_drafts_and_user_schemas_take_effect() {
    let cli = Cli::new();
    // Two report pairs no table claims, in one file.
    let new = cli.home.path().join("new_pairs.csv");
    std::fs::write(
        &new,
        "C,NEMP.WORLD,TEST\nI,NEMLAKE_TEST,FIRST,1,SETTLEMENTDATE,UNIT,VALUE\nD,NEMLAKE_TEST,FIRST,1,\"2024/01/01 00:05:00\",A1,1.5\nI,NEMLAKE_TEST,SECOND,1,SETTLEMENTDATE,FLAG\nD,NEMLAKE_TEST,SECOND,1,\"2024/01/01 00:05:00\",Y\nC,\"END OF REPORT\",6\n",
    )
    .unwrap();
    let new = new.to_str().unwrap();
    let dir = cli.home.path().join("schemas/nem/mms");
    let adopt = |table: &str| {
        std::fs::rename(
            dir.join(format!("DRAFT_{table}.toml")),
            dir.join(format!("{table}.toml")),
        )
        .unwrap();
    };

    // Each pair gets a draft.
    let out = cli.ok(&["schema", "infer", new]);
    assert!(out.contains("# draft for NEMLAKE_TEST_FIRST"), "{out}");
    assert!(out.contains("# draft for NEMLAKE_TEST_SECOND"), "{out}");
    assert!(out.contains("\"inferred\""), "{out}");
    assert!(!dir.exists(), "printing writes nothing");
    let out = cli.ok(&["schema", "infer", new, "--write"]);
    assert!(out.contains("2 draft(s)"), "{out}");
    assert!(dir.join("DRAFT_NEMLAKE_TEST_FIRST.toml").is_file());
    assert!(dir.join("DRAFT_NEMLAKE_TEST_SECOND.toml").is_file());
    // A draft is inert until it is renamed.
    let (code, out, _) = cli.code(&["inspect", new]);
    assert_eq!(code, 5, "{out}");
    adopt("NEMLAKE_TEST_FIRST");
    adopt("NEMLAKE_TEST_SECOND");
    let (code, out, err) = cli.code(&["inspect", new]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(
        out.contains("NEMLAKE_TEST_FIRST") && !out.contains("schema.unknown_table"),
        "{out}"
    );
    assert!(
        cli.ok(&["schema", "infer", new])
            .contains("nothing to infer")
    );
    assert!(cli.ok(&["schema", "list"]).contains("NEMLAKE_TEST_SECOND"));
    // The file now ingests in strict mode.
    let (code, out, err) = cli.code(&["ingest", new, "--strict"]);
    assert_eq!(code, 0, "{out}{err}");
    let ls = cli.ok(&["--json", "ls", "--limit", "100"]);
    assert!(ls.contains("\"table\":\"NEMLAKE_TEST_FIRST\""), "{ls}");
    assert!(ls.contains("\"table\":\"NEMLAKE_TEST_SECOND\""), "{ls}");
    // Without the user's tables the pairs are unknown again.
    let (code, _, _) = cli.code(&["--no-user-schemas", "inspect", new]);
    assert_eq!(code, 5);

    // A known table published under a new version with a new column.
    let v2 = cli.home.path().join("scada_v2.csv");
    std::fs::write(
        &v2,
        "C,NEMP.WORLD,TEST\nI,DISPATCH,UNIT_SCADA,2,SETTLEMENTDATE,DUID,SCADAVALUE,QUALITY\nD,DISPATCH,UNIT_SCADA,2,\"2024/01/01 00:05:00\",BW01,1.5,GOOD\nC,\"END OF REPORT\",4\n",
    )
    .unwrap();
    let v2 = v2.to_str().unwrap();
    let (code, out, err) = cli.code(&["ingest", v2, "--strict"]);
    assert_ne!(
        code, 0,
        "an undeclared version is refused in strict mode: {out}{err}"
    );
    let out = cli.ok(&["--json", "schema", "infer", v2, "--write"]);
    assert!(
        out.contains("\"table\":\"DISPATCH_UNIT_SCADA\"") && out.contains("QUALITY"),
        "{out}"
    );
    assert!(out.contains("\"issues\":[]"), "{out}");
    adopt("DISPATCH_UNIT_SCADA");
    let (code, out, err) = cli.code(&["ingest", v2, "--strict"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(
        cli.ok(&["schema", "lineage", "DISPATCH_UNIT_SCADA"])
            .contains("QUALITY")
    );

    // A broken file in the user's directory stops every command, and says how to go on.
    std::fs::write(dir.join("BROKEN.toml"), "schema_format = 1\nname = 3\n").unwrap();
    let (code, _, err) = cli.code(&["schema", "list"]);
    assert_ne!(code, 0);
    assert!(
        err.contains("BROKEN.toml") && err.contains("--no-user-schemas"),
        "{err}"
    );
    assert!(
        cli.ok(&["--no-user-schemas", "schema", "list"])
            .contains("DISPATCH_UNIT_SCADA")
    );
}

/// `cache gc`: leftovers always go; a download goes when the lake already
/// holds it (`--ingested`) or when it is old (`--older-than`).
#[test]
fn cache_gc_removes_what_is_no_longer_needed() {
    let cli = Cli::new();
    assert!(cli.ok(&["cache", "gc"]).contains("removed 0 file(s)"));
    // Ingest a local file so the manifest knows its URL and checksum.
    let csv = cli.home.path().join("PUBLIC_DVD_REGION_202401010000.CSV");
    std::fs::copy(fixtures().join("REGION_v1_head.csv"), &csv).unwrap();
    cli.ok(&["ingest", csv.to_str().unwrap()]);
    let ls = cli.ok(&["--json", "ls"]);
    let url = ls
        .split("\"url\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("ls reports the source URL")
        .to_owned();
    let sha = nemlake_fetch::cache::sha256_hex(&std::fs::read(&csv).unwrap());

    // Three downloads in the cache: that file, an unknown recent one, an unknown old one.
    let cache = cli.home.path().join("cache/nem-mmsdm");
    std::fs::create_dir_all(&cache).unwrap();
    let put = |name: &str, url: &str, sha: &str, fetched_at: &str| {
        std::fs::write(cache.join(name), b"zip bytes").unwrap();
        std::fs::write(
            cache.join(format!("{name}.json")),
            format!(
                r#"{{"url":"{url}","etag":null,"last_modified":null,"size":9,"sha256":"{sha}","fetched_at":"{fetched_at}","accept_ranges":true}}"#
            ),
        )
        .unwrap();
    };
    put("aa-ingested.zip", &url, &sha, "2099-01-01T00:00:00Z");
    put(
        "bb-recent.zip",
        "https://h/recent.zip",
        "00",
        "2099-01-01T00:00:00Z",
    );
    put(
        "cc-old.zip",
        "https://h/old.zip",
        "00",
        "2020-01-01T00:00:00Z",
    );
    assert!(cli.ok(&["cache", "ls"]).contains("3 file(s)"));

    // Nothing is a leftover, and without a flag no download is touched.
    assert!(cli.ok(&["cache", "gc"]).contains("removed 0 file(s)"));
    // A dry run names what would go and removes nothing.
    let out = cli.ok(&[
        "cache",
        "gc",
        "--ingested",
        "--older-than",
        "30",
        "--dry-run",
    ]);
    assert!(
        out.contains("would remove 4 file(s)") && out.contains("2 download(s)"),
        "{out}"
    );
    assert!(
        out.contains("aa-ingested.zip") && out.contains("download already in the lake"),
        "{out}"
    );
    assert!(
        out.contains("cc-old.zip") && out.contains("download older than asked"),
        "{out}"
    );
    assert!(!out.contains("bb-recent.zip"), "{out}");
    assert!(cli.ok(&["cache", "ls"]).contains("3 file(s)"));
    // One flag at a time.
    let out = cli.ok(&["--json", "cache", "gc", "--ingested"]);
    assert_eq!(out.lines().count(), 2, "{out}");
    assert!(
        out.contains("\"removed\":true") && out.contains("aa-ingested.zip"),
        "{out}"
    );
    assert!(!cache.join("aa-ingested.zip").exists());
    assert!(!cache.join("aa-ingested.zip.json").exists());
    assert!(
        cli.ok(&["cache", "gc", "--older-than", "30"])
            .contains("removed 2 file(s)")
    );
    assert!(cli.ok(&["cache", "ls"]).contains("1 file(s)"));
    assert!(cache.join("bb-recent.zip").exists());
    // The lake is untouched and the file still counts as ingested.
    assert!(cli.ok(&["--json", "ls"]).contains("\"status\":\"ok\""));
}

#[test]
fn cache_wem_fetch_and_config() {
    let cli = Cli::new();
    assert!(cli.ok(&["cache", "ls"]).contains("0 file(s)"));
    assert!(cli.ok(&["--json", "cache", "ls"]).is_empty());
    assert!(cli.ok(&["cache", "clear"]).contains("removed 0"));
    // Offline: a fetch that needs the network fails with the offline code and exit 4.
    let (code, out, err) = cli.code(&[
        "--offline",
        "fetch",
        "nem",
        "--table",
        "REGION",
        "--from",
        "2024-01",
    ]);
    assert_eq!(code, 4, "{out}{err}");
    assert!(
        out.contains("net.offline") || err.contains("net.offline"),
        "{out}{err}"
    );
    let (code, out, _) = cli.code(&[
        "--offline",
        "--json",
        "fetch",
        "nem",
        "--from",
        "2024-01",
        "--to",
        "2024-01",
        "--dry-run",
    ]);
    assert_eq!(code, 4, "{out}");
    assert_eq!(
        cli.code(&[
            "--offline",
            "fetch",
            "nem",
            "--from",
            "2024-02",
            "--to",
            "2024-01"
        ])
        .0,
        1,
        "to before from"
    );
    let (code, _, err) = cli.code(&["--offline", "wem", "manifest", "facility-scada"]);
    assert_eq!(code, 1);
    assert!(err.contains("net.offline"), "{err}");
    // An unknown config key is an error naming the key.
    std::fs::write(cli.home.path().join("config.toml"), "jobz = 1\n").unwrap();
    let (code, _, err) = cli.code(&["schema", "list"]);
    assert_eq!(code, 1);
    assert!(err.contains("jobz"), "{err}");
    std::fs::write(
        cli.home.path().join("config.toml"),
        "jobs = 1\nstrict = true\ndecimal = true\n",
    )
    .unwrap();
    assert!(cli.ok(&["-v", "schema", "list"]).contains("DISPATCH"));
    assert!(
        cli.ok(&["--decimal", "schema", "show", "DISPATCH_UNIT_SCADA"])
            .contains("Decimal128(16, 6)")
    );
}

fn walk(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.filter_map(Result::ok) {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out
}
