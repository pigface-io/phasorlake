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

/// The head of every zip of the months the registry is built from, the
/// anchor (August 2026) and an earlier one (January 2024): each one must
/// resolve exactly against the registry (strict mode refuses drift) and
/// every row must parse into its declared types, as floats and as exact
/// decimals.
#[test]
fn every_archive_table_ingests_strictly() {
    every_head_ingests_strictly("MMSDM_2024_01", 161);
    every_head_ingests_strictly("MMSDM_2026_08", 309);
}

fn every_head_ingests_strictly(month: &str, at_least: usize) {
    let heads = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/bootstrap")
        .join(month)
        .join("heads");
    let files = std::fs::read_dir(&heads).unwrap().count();
    assert!(files >= at_least, "{files} head fixtures of {month}");
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
        // No row fails to parse. The one parse diagnostic allowed is the
        // warning that a "no end" date beyond the nanosecond range was
        // clamped, which the standing data of each month does contain.
        for line in errors.lines().filter(|l| l.contains("\"parse.")) {
            assert!(
                line.contains("parse.timestamp_clamped")
                    && line.contains("\"severity\":\"warning\""),
                "{mode:?}: {line}"
            );
        }
        assert!(
            errors.contains("parse.timestamp_clamped")
                && (errors.contains("2999/12/31 00:00:00")
                    || errors.contains("9999/12/31 00:00:00")),
            "{mode:?}: the clamp is reported: {errors}"
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
    // A real multi-block file, with one more block for a report pair no
    // table claims put in before the footer.
    let fixture = std::fs::read_to_string(fixtures().join("multi_block_dispatchis.csv")).unwrap();
    let footer = fixture.rfind("C,\"END OF REPORT\"").unwrap();
    let multi = cli.home.path().join("multi_block.csv");
    std::fs::write(
        &multi,
        format!(
            "{}I,NEMLAKE_TEST,NEW_THING,1,WHEN,AMOUNT\nD,NEMLAKE_TEST,NEW_THING,1,\"2026/10/01 13:40:00\",1.5\n{}",
            &fixture[..footer],
            &fixture[footer..]
        ),
    )
    .unwrap();
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
        out.contains("\"drafts\":[{") && out.contains("NEMLAKE_TEST_NEW_THING"),
        "{out}"
    );
    let (code, out, _) = cli.code(&["inspect", multi.to_str().unwrap(), "--infer"]);
    assert_eq!(code, 5);
    assert!(out.contains("# draft for NEMLAKE_TEST_NEW_THING"), "{out}");
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

/// A table and a column nobody declared are ingested all the same, as
/// text, and reported; only strict mode refuses them.
#[test]
fn what_the_registry_does_not_declare_is_kept_not_skipped() {
    let cli = Cli::new();
    let csv = cli.home.path().join("PUBLIC_DVD_NOBODY_202403010000.CSV");
    std::fs::write(
        &csv,
        "C,NEMP.WORLD,TEST\nI,NEMLAKE_TEST,NOBODY,1,CONTRACTYEAR,LABEL\nD,NEMLAKE_TEST,NOBODY,1,2024,first\nD,NEMLAKE_TEST,NOBODY,1,2025,second\nI,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID,SCADAVALUE,QUALITY\nD,DISPATCH,UNIT_SCADA,1,\"2024/03/01 00:05:00\",BW01,1.5,GOOD\nC,\"END OF REPORT\",7\n",
    )
    .unwrap();
    let csv = csv.to_str().unwrap();
    let (code, out, err) = cli.code(&["ingest", csv]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("3 rows (0 quarantined)"), "{out}");
    assert!(
        out.contains("schema.unknown_table") && out.contains("schema.unknown_columns"),
        "{out}"
    );
    // The undeclared table is apart from the registry's, under its report pair.
    let derived = cli
        .home
        .path()
        .join("lake/nem/_unregistered/nemlake_test.nobody/year=2024/month=03");
    assert_eq!(
        std::fs::read_dir(&derived).unwrap().count(),
        1,
        "{}",
        derived.display()
    );
    // The declared table got its row, the undeclared column with it.
    let scada = cli
        .home
        .path()
        .join("lake/nem/dispatch_unit_scada/year=2024/month=03");
    assert_eq!(std::fs::read_dir(&scada).unwrap().count(), 1);
    let ls = cli.ok(&["--json", "ls"]);
    assert_eq!(ls.lines().count(), 2, "{ls}");
    assert!(
        ls.contains("\"table\":\"NEMLAKE_TEST_NOBODY\"") && ls.contains("\"rows\":2"),
        "{ls}"
    );

    // Strict mode refuses the file instead.
    let strict = cli.home.path().join("PUBLIC_DVD_NOBODY_202404010000.CSV");
    std::fs::copy(csv, &strict).unwrap();
    let (code, out, err) = cli.code(&["ingest", strict.to_str().unwrap(), "--strict"]);
    assert_ne!(code, 0, "{out}{err}");
    assert!(out.contains("schema.unknown_table"), "{out}");
}

/// A table nobody declared is kept as text; once a schema for it is
/// adopted, `reingest` reads the source again from disk and the lake
/// follows: typed files in the table's own directory, the text ones gone.
#[test]
fn reingest_brings_the_lake_up_to_the_rules() {
    let cli = Cli::new();
    let csv = cli.home.path().join("PUBLIC_DVD_FOLLOWS_202403010000.CSV");
    std::fs::write(
        &csv,
        "C,NEMP.WORLD,TEST\nI,NEMLAKE_TEST,FOLLOWS,1,SETTLEMENTDATE,UNIT,VALUE\nD,NEMLAKE_TEST,FOLLOWS,1,\"2024/03/01 00:05:00\",A1,1.5\nD,NEMLAKE_TEST,FOLLOWS,1,\"2024/03/01 00:10:00\",A1,2.5\nC,\"END OF REPORT\",5\n",
    )
    .unwrap();
    let csv_arg = csv.to_str().unwrap();
    let lake = cli.home.path().join("lake");
    let parquet = || -> Vec<String> {
        walk(&lake)
            .into_iter()
            .map(|p| p.display().to_string())
            .filter(|p| p.ends_with(".parquet"))
            .collect()
    };

    let (code, out, err) = cli.code(&["ingest", csv_arg]);
    assert_eq!(code, 0, "{out}{err}");
    let before = parquet();
    assert_eq!(before.len(), 1, "{before:?}");
    assert!(
        before[0].contains("nem/_unregistered/nemlake_test.follows/"),
        "{before:?}"
    );
    let ls = cli.ok(&["--json", "ls"]);
    assert!(ls.contains("\"schema_source\":\"file\""), "{ls}");
    assert!(ls.contains("\"resolution\":\"derived\""), "{ls}");
    assert!(ls.contains("\"rules\":\"rules:"), "{ls}");
    assert!(
        ls.contains("\"source_columns\":[\"SETTLEMENTDATE\",\"UNIT\",\"VALUE\"]"),
        "{ls}"
    );
    // Nothing is behind yet.
    let out = cli.ok(&["reingest"]);
    assert!(out.contains("0 listed"), "{out}");

    // Adopt a schema for the table: the source is now behind the rules.
    cli.ok(&["schema", "infer", csv_arg, "--write"]);
    let dir = cli.home.path().join("schemas/nem/mms");
    std::fs::rename(
        dir.join("DRAFT_NEMLAKE_TEST_FOLLOWS.toml"),
        dir.join("NEMLAKE_TEST_FOLLOWS.toml"),
    )
    .unwrap();
    let out = cli.ok(&["reingest", "--dry-run"]);
    assert!(
        out.contains("1 listed") && out.contains("PUBLIC_DVD_FOLLOWS"),
        "{out}"
    );
    assert_eq!(parquet(), before, "a dry run writes nothing");
    let (code, out, err) = cli.code(&["reingest"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("1 listed, 1 ingested"), "{out}");
    assert!(out.contains("manifest.rules_changed"), "{out}");
    let after = parquet();
    assert_eq!(after.len(), 1, "{after:?}");
    assert!(after[0].contains("nem/nemlake_test_follows/"), "{after:?}");
    assert!(
        !lake.join("nem/_unregistered").exists(),
        "the directory the text files were in is gone with them"
    );
    let ls = cli.ok(&["--json", "ls"]);
    assert_eq!(ls.lines().count(), 1, "{ls}");
    assert!(ls.contains("\"schema_source\":\"user\""), "{ls}");
    assert!(ls.contains("\"resolution\":\"exact\""), "{ls}");
    assert!(cli.ok(&["verify"]).contains("no problems"));
    // Up to date again; an ordinary ingest of the same file is skipped.
    assert!(cli.ok(&["reingest"]).contains("0 listed"));
    assert!(cli.ok(&["ingest", csv_arg]).contains("1 skipped"));

    // Without the user's table the rules are the old ones, and the source
    // is no longer there to read.
    std::fs::remove_file(&csv).unwrap();
    let (code, out, err) = cli.code(&["--no-user-schemas", "reingest"]);
    assert_ne!(code, 0, "{out}{err}");
    assert!(out.contains("cache.source_missing"), "{out}");
    assert_eq!(parquet(), after, "what is in the lake stays");
}

/// A control file from AEMO's archive types a draft: its loader types are
/// the base, the table takes the name the control file loads into, and a
/// sampled value that does not fit overrules it with a note.
#[test]
fn schema_infer_types_a_draft_from_a_control_file() {
    let cli = Cli::new();
    let csv = cli.home.path().join("PUBLIC_DVD_CTLTYPED_202403010000.CSV");
    std::fs::write(
        &csv,
        "C,NEMP.WORLD,TEST\nI,NEMLAKE_TEST,CTLTYPED,1,SETTLEMENTDATE,UNIT,VALUE,FLAG\nD,NEMLAKE_TEST,CTLTYPED,1,\"2024/03/01 00:05:00\",7,1.5,yes\nC,\"END OF REPORT\",4\n",
    )
    .unwrap();
    let ctl = cli.home.path().join("PUBLIC_DVD_CTLTYPED_202403.ctl");
    std::fs::write(
        &ctl,
        "LOAD DATA\nINFILE PUBLIC_DVD_CTLTYPED_202403010000.CSV\nAPPEND INTO TABLE NEMLAKE_CTLTYPED\nWHEN (1:1) = 'D'\nFIELDS TERMINATED BY ','\nOPTIONALLY ENCLOSED BY '\"'\nTRAILING NULLCOLS\n(row_type FILLER,\nreport_type FILLER,\nreport_subtype FILLER,\nreport_version FILLER,\nSETTLEMENTDATE DATE \"yyyy/mm/dd hh24:mi:ss\",\nUNIT CHAR(20),\nVALUE FLOAT EXTERNAL,\nFLAG FLOAT EXTERNAL)\n",
    )
    .unwrap();
    let (csv, ctl) = (csv.to_str().unwrap(), ctl.to_str().unwrap());
    // Without it the rows alone decide: UNIT, a number in this sample, is
    // guessed a number.
    let out = cli.ok(&["schema", "infer", csv]);
    assert!(out.contains("# draft for NEMLAKE_TEST_CTLTYPED"), "{out}");
    assert!(!out.contains("CHAR(20)"), "{out}");
    // With it: the table's own name, UNIT as text of 20, and FLAG, which
    // the control file calls a number, stays text because `yes` is not one.
    let out = cli.ok(&["schema", "infer", csv, "--ctl", ctl]);
    assert!(out.contains("# draft for NEMLAKE_CTLTYPED"), "{out}");
    assert!(
        out.contains("loader = \"CHAR(20)\"") && out.contains("max_len = 20"),
        "{out}"
    );
    assert!(out.contains("PUBLIC_DVD_CTLTYPED_202403.ctl"), "{out}");
    assert!(
        out.contains("# note: column FLAG: the control file says FLOAT EXTERNAL"),
        "{out}"
    );
    let json = cli.ok(&["--json", "schema", "infer", csv, "--ctl", ctl, "--write"]);
    assert!(
        json.contains("\"table\":\"NEMLAKE_CTLTYPED\"")
            && json.contains("\"notes\":[\"column FLAG"),
        "{json}"
    );
    let dir = cli.home.path().join("schemas/nem/mms");
    std::fs::rename(
        dir.join("DRAFT_NEMLAKE_CTLTYPED.toml"),
        dir.join("NEMLAKE_CTLTYPED.toml"),
    )
    .unwrap();
    let (code, out, err) = cli.code(&["ingest", csv, "--strict"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("1 rows (0 quarantined)"), "{out}");
    // A control file that cannot be parsed stops the command and says why.
    let bad = cli.home.path().join("bad.ctl");
    std::fs::write(&bad, "not a control file").unwrap();
    let (code, _, err) = cli.code(&["schema", "infer", csv, "--ctl", bad.to_str().unwrap()]);
    assert_ne!(code, 0);
    assert!(err.contains("bad.ctl"), "{err}");
    let (code, _, err) = cli.code(&["schema", "infer", csv, "--ctl", "/nonexistent/x.ctl"]);
    assert_ne!(code, 0);
    assert!(err.contains("could not be read"), "{err}");
}

/// Files of an earlier interval that today's table cannot read, a value
/// with its unit where a number is declared, are brought in by a rule
/// written by hand in the table file; changing the rule puts exactly the
/// sources it touches behind.
#[test]
fn a_hand_written_rule_brings_in_the_files_of_an_interval() {
    let cli = Cli::new();
    let write = |name: &str, stamp: &str, value: &str| -> String {
        let p = cli.home.path().join(name);
        std::fs::write(
            &p,
            format!(
                "C,NEMP.WORLD,TEST\nI,NEMLAKE_TEST,RULED,1,SETTLEMENTDATE,UNIT,VALUE\nD,NEMLAKE_TEST,RULED,1,\"{stamp}\",A1,{value}\nC,\"END OF REPORT\",4\n"
            ),
        )
        .unwrap();
        p.to_str().unwrap().to_owned()
    };
    let new = write(
        "PUBLIC_DVD_RULED_202403010000.CSV",
        "2024/03/01 00:05:00",
        "2.5",
    );
    let old = write(
        "PUBLIC_DVD_RULED_201003010000.CSV",
        "2010/03/01 00:05:00",
        "1.5 MW",
    );
    // The table as today's file shows it: VALUE is a number.
    cli.ok(&["schema", "infer", &new, "--write"]);
    let dir = cli.home.path().join("schemas/nem/mms");
    let table = dir.join("NEMLAKE_TEST_RULED.toml");
    std::fs::rename(dir.join("DRAFT_NEMLAKE_TEST_RULED.toml"), &table).unwrap();
    let (code, out, err) = cli.code(&["ingest", &new]);
    assert_eq!(code, 0, "{out}{err}");
    // The 2010 file does not fit it: its row cannot be read.
    let (code, out, _) = cli.code(&["ingest", &old]);
    assert_ne!(code, 0, "{out}");
    assert!(out.contains("csv.too_many_bad_rows"), "{out}");

    // A column to keep the old values as published, and a rule that sends
    // them there for the files up to 2011-12.
    let text = std::fs::read_to_string(&table).unwrap();
    assert!(text.contains("next_column_id = 4"), "{text}");
    let ruled = format!(
        "{}\n[[column]]\nid = 4\nname = \"VALUE_AS_PUBLISHED\"\ntype = \"utf8\"\nadded_in = 1\nsrc = [\"manual\"]\n\n[[rule]]\nnote = \"Until 2011 the value carried its unit.\"\nto = \"2011-12\"\n\n[[rule.column]]\nsource = \"VALUE\"\ncolumn = \"VALUE_AS_PUBLISHED\"\n",
        text.replace("next_column_id = 4", "next_column_id = 5")
    );
    std::fs::write(&table, &ruled).unwrap();
    assert!(
        cli.ok(&["schema", "show", "NEMLAKE_TEST_RULED"])
            .contains("VALUE_AS_PUBLISHED")
    );
    // The table changed, so the 2024 file is behind (it gains a column)
    // and the 2010 file, which failed, is still to be read.
    let out = cli.ok(&["reingest", "--dry-run"]);
    assert!(out.contains("2 listed"), "{out}");
    let (code, out, err) = cli.code(&["reingest"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(
        out.contains("2 ingested") && out.contains("(0 quarantined)"),
        "{out}"
    );
    let ls = cli.ok(&["--json", "ls"]);
    let line = |period: &str| {
        ls.lines()
            .find(|l| l.contains(&format!("\"source_period\":\"{period}\"")))
            .unwrap_or_else(|| panic!("no ingest of {period}: {ls}"))
            .to_owned()
    };
    assert!(
        line("2010-03").contains("\"resolution\":\"exact+rule\""),
        "{ls}"
    );
    assert!(line("2010-03").contains("\"status\":\"ok\""), "{ls}");
    assert!(line("2024-03").contains("\"resolution\":\"exact\""), "{ls}");
    assert!(cli.ok(&["reingest"]).contains("0 listed"));

    // Widen the rule to every month: only the 2024 source is behind now.
    std::fs::write(&table, ruled.replace("to = \"2011-12\"\n", "")).unwrap();
    let out = cli.ok(&["reingest", "--dry-run"]);
    assert!(
        out.contains("1 listed") && out.contains("RULED_202403"),
        "{out}"
    );

    // A rule that names a column nobody declared is refused with the reason.
    std::fs::write(
        &table,
        ruled.replace("column = \"VALUE_AS_PUBLISHED\"", "column = \"NOWHERE\""),
    )
    .unwrap();
    let (code, _, err) = cli.code(&["schema", "list"]);
    assert_ne!(code, 0);
    assert!(err.contains("not a declared column"), "{err}");
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

/// Downloads are an archive: listed with how they were requested, checked
/// against their record, and never removed by any cache command.
#[test]
fn archive_is_kept_and_cache_commands_only_remove_throwaway_files() {
    let cli = Cli::new();
    assert!(cli.ok(&["archive", "ls"]).contains("0 file(s)"));
    assert!(cli.ok(&["--json", "archive", "ls"]).is_empty());
    assert!(cli.ok(&["archive", "verify"]).contains("0 file(s) checked"));
    assert!(
        cli.ok(&["cache", "gc"])
            .contains("removed 0 leftover file(s)")
    );
    assert!(
        cli.ok(&["cache", "clear"])
            .contains("removed 0 cached listing(s)")
    );

    // One archived download, at the path its URL gives it, with its record.
    let dir = cli.home.path().join("archive/h/DATA");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a.zip");
    std::fs::write(&file, b"zip bytes").unwrap();
    std::fs::write(
        dir.join("a.zip.json"),
        format!(
            r#"{{"url":"https://h/DATA/a.zip","size":9,"sha256":"{}","fetched_at":"2026-10-03T10:00:05Z","request":{{"by":"fetch nem REGION 2024-01..2024-01","tool":"nemlake/0.1.0","source":"nem-mmsdm","started_at":"2026-10-03T10:00:00Z","request_url":"https://h/DATA/a.zip","http_status":200,"attempts":1}}}}"#,
            nemlake_fetch::cache::sha256_hex(b"zip bytes")
        ),
    )
    .unwrap();
    let out = cli.ok(&["archive", "ls"]);
    assert!(out.contains("1 file(s)"), "{out}");
    assert!(out.contains("fetch nem REGION 2024-01..2024-01"), "{out}");
    assert!(
        out.contains("https://h/DATA/a.zip") && out.contains("2026-10-03T10:00:05Z"),
        "{out}"
    );
    let out = cli.ok(&["--json", "archive", "ls"]);
    assert_eq!(out.lines().count(), 1, "{out}");
    assert!(
        out.contains("\"record\"") && out.contains("\"started_at\":\"2026-10-03T10:00:00Z\""),
        "{out}"
    );
    assert!(
        cli.ok(&["archive", "verify"])
            .contains("1 file(s) checked, 0 do not match")
    );

    // Leftovers beside it: a partial download nobody resumed, and a listing.
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 86_400);
    let part = dir.join("b.zip.part");
    std::fs::write(&part, b"half").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&part)
        .unwrap()
        .set_modified(old)
        .unwrap();
    let listings = cli.home.path().join("cache/listings");
    std::fs::create_dir_all(&listings).unwrap();
    std::fs::write(listings.join("abc.html"), b"<pre></pre>").unwrap();
    std::fs::write(listings.join("abc.html.json"), b"{}").unwrap();

    let out = cli.ok(&["cache", "gc", "--dry-run"]);
    assert!(out.contains("would remove 1 leftover file(s)"), "{out}");
    assert!(
        out.contains("b.zip.part") && out.contains("partial download nobody resumed"),
        "{out}"
    );
    assert!(part.exists());
    // Kept for a resume while it is young enough.
    assert!(
        cli.ok(&["cache", "gc", "--keep-partial", "60"])
            .contains("removed 0 leftover")
    );
    let out = cli.ok(&["--json", "cache", "gc"]);
    assert!(
        out.contains("\"removed\":true") && out.contains("b.zip.part"),
        "{out}"
    );
    assert!(!part.exists());
    assert!(
        cli.ok(&["cache", "clear"])
            .contains("removed 1 cached listing(s)")
    );
    assert!(!listings.join("abc.html").exists());

    // None of that touched the archived file.
    assert_eq!(std::fs::read(&file).unwrap(), b"zip bytes");
    assert!(cli.ok(&["archive", "ls"]).contains("1 file(s)"));
    // A file that changed on disk is found out.
    std::fs::write(&file, b"zip bytez").unwrap();
    let (code, out, err) = cli.code(&["archive", "verify"]);
    assert_eq!(code, 1, "{out}{err}");
    assert!(out.contains("1 file(s) checked, 1 do not match"), "{out}");
    assert!(
        err.contains("cache.sha256_mismatch") && err.contains("a.zip"),
        "{err}"
    );
}

#[test]
fn cache_wem_fetch_and_config() {
    let cli = Cli::new();
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
