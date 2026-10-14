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
    // A quarantine file no current ingest recorded is an orphan.
    let qdir = cli.home.path().join("quarantine/region");
    std::fs::create_dir_all(&qdir).unwrap();
    std::fs::write(qdir.join("left_over.jsonl"), "{}\n").unwrap();
    let (code, out, _) = cli.code(&["verify", "--table", "REGION"]);
    assert_eq!(code, 1);
    assert!(out.contains("verify.orphan_quarantine"), "{out}");
    let (_, out, _) = cli.code(&["verify", "--table", "DISPATCH_UNIT_SCADA"]);
    assert!(!out.contains("orphan"), "another table's quarantine: {out}");
    std::fs::remove_file(qdir.join("left_over.jsonl")).unwrap();
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
    std::fs::write(&bad, b"I,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID,SCADAVALUE\r\nD,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:05:00\",A,1,extra\r\nD,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:05:00\",B,1\r\nC,\"END OF REPORT\",4\r\n").unwrap();
    let (code, out, err) = cli.code(&["ingest", bad.to_str().unwrap()]);
    assert_eq!(code, 1, "{out}{err}");
    assert!(out.contains("csv.too_many_bad_rows"), "{out}");
    let quarantine = walk(&cli.home.path().join("quarantine"));
    assert_eq!(quarantine.len(), 1, "{quarantine:?}");
    assert!(
        std::fs::read_to_string(&quarantine[0])
            .unwrap()
            .contains("csv.field_count_mismatch"),
        "{err}"
    );
    // Every error is printed as a JSON object, as it happens. The row's
    // own error says what was expected and what was there, and the
    // refusal that follows repeats it.
    let printed: Vec<serde_json::Value> = err
        .lines()
        .filter(|l| l.starts_with('{'))
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{e}: {l}")))
        .collect();
    let row = printed
        .iter()
        .find(|d| d["code"] == "csv.field_count_mismatch")
        .unwrap_or_else(|| panic!("no row error in {err}"));
    assert_eq!(row["severity"], "error");
    assert_eq!(row["observed"], "4 fields");
    assert_eq!(row["expected"], "3 fields");
    assert_eq!(row["line"], 2);
    assert_eq!(row["table"], "DISPATCH_UNIT_SCADA");
    assert!(row["sample"]["text"].as_str().unwrap().contains("extra"));
    let refusal = printed
        .iter()
        .find(|d| d["code"] == "csv.too_many_bad_rows")
        .unwrap_or_else(|| panic!("no refusal in {err}"));
    assert_eq!(refusal["observed"], "4 fields");
    assert!(
        refusal["message"]
            .as_str()
            .unwrap()
            .contains("The first is line 2"),
        "{refusal}"
    );
    assert!(
        refusal["cause"][0]
            .as_str()
            .unwrap()
            .ends_with("[csv.field_count_mismatch]")
    );
    // The summary on stdout gives them as JSON too.
    assert!(
        out.contains("error csv.field_count_mismatch ×1\n{\"code\":\"csv.field_count_mismatch\""),
        "{out}"
    );
    // And so does `errors`, with or without --json: one object a line.
    let errors = cli.ok(&["errors", "--last"]);
    assert!(errors.contains("csv.too_many_bad_rows"), "{errors}");
    assert!(
        errors.contains("\"code\":\"csv.field_count_mismatch\""),
        "{errors}"
    );
    for line in errors.lines() {
        let v: serde_json::Value =
            serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {line}"));
        assert!(v["diagnostic"]["code"].is_string(), "{line}");
    }
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
    // A value that does not fit its column keeps its row; strict mode
    // refuses the file for it, saying which column and line.
    let misfit = cli.home.path().join("misfit.csv");
    std::fs::write(&misfit, b"I,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID,SCADAVALUE\r\nD,DISPATCH,UNIT_SCADA,1,\"2024/13/01 00:05:00\",A,1\r\nD,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:05:00\",B,1\r\nC,\"END OF REPORT\",4\r\n").unwrap();
    let cli3 = Cli::new();
    let (code, out, err) = cli3.code(&["ingest", misfit.to_str().unwrap()]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("2 rows (0 quarantined)"), "{out}");
    assert!(out.contains("warning parse.value_kept_raw ×1"), "{out}");
    let (code, out, err) = cli3.code(&["ingest", misfit.to_str().unwrap(), "--strict"]);
    assert_eq!(code, 0, "already ingested: {out}{err}");
    let cli4 = Cli::new();
    let (code, out, err) = cli4.code(&["ingest", misfit.to_str().unwrap(), "--strict"]);
    assert_eq!(code, 1, "{out}{err}");
    let refusal: serde_json::Value = err
        .lines()
        .filter(|l| l.starts_with('{'))
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .find(|d| d["code"] == "parse.timestamp")
        .unwrap_or_else(|| panic!("no refusal in {err}"));
    assert_eq!(refusal["severity"], "error");
    assert_eq!(refusal["column"], "SETTLEMENTDATE");
    assert_eq!(refusal["line"], 2);
    assert_eq!(refusal["observed"], "2024/13/01 00:05:00");
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
/// conform to the increment of its table in force that month (strict mode
/// refuses a deviation) and every row must parse into its column's type.
/// January 2024 has known deviations: the `_ALL` files of three P5MIN
/// tables carry other columns than the file each table's control file
/// loads, under the same report pair; and five tables' files carry
/// columns their control file of the month does not list, which a census
/// of every month is to settle.
#[test]
fn every_archive_table_ingests_strictly() {
    every_head_ingests_strictly(
        "MMSDM_2024_01",
        161,
        &[
            "DISPATCHCONSTRAINT",
            "MTPASA_REGIONRESULT",
            "P5MIN_CONSTRAINTSOLUTION1",
            "P5MIN_CONSTRAINTSOLUTION2",
            "P5MIN_CONSTRAINTSOLUTION3",
            "P5MIN_CONSTRAINTSOLUTION4",
            "PREDISPATCHCONSTRAINT_D",
            "PREDISPATCHCONSTRAINT1",
            "PREDISPATCHCONSTRAINT2",
            "PREDISPATCHPRICE",
            "PREDISPATCHPRICE_D",
            "P5MIN_CONSTRAINTSOLUTION1_ALL",
            "P5MIN_CONSTRAINTSOLUTION2_ALL",
            "P5MIN_CONSTRAINTSOLUTION3_ALL",
            "P5MIN_CONSTRAINTSOLUTION4_ALL",
            "P5MIN_REGIONSOLUTION_ALL",
            "P5MIN_UNITSOLUTION_ALL",
        ],
    );
    every_head_ingests_strictly("MMSDM_2026_08", 309, &[]);
}

fn every_head_ingests_strictly(month: &str, at_least: usize, deviating: &[&str]) {
    let heads = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/bootstrap")
        .join(month)
        .join("heads");
    let files = std::fs::read_dir(&heads).unwrap().count();
    assert!(files >= at_least, "{files} head fixtures of {month}");
    let deviates = |l: &str| deviating.iter().any(|d| l.contains(&format!("_{d}_")));
    let cli = Cli::new();
    let (code, out, err) = cli.code(&["--json", "ingest", heads.to_str().unwrap(), "--strict"]);
    assert_eq!(
        code == 0,
        deviating.is_empty(),
        "{month}: {err}{}",
        &out[out.len().saturating_sub(2000)..]
    );
    let ls = cli.ok(&["--json", "ls", "--limit", "1000"]);
    let rows: Vec<&str> = ls.lines().collect();
    assert_eq!(
        rows.len() + deviating.len(),
        files,
        "{month}: one ingest per head"
    );
    for r in &rows {
        assert!(
            r.contains("\"status\":\"ok\"")
                && r.contains("\"bad_rows\":0")
                && r.contains("\"resolution\":\"conforms\""),
            "{month}: {r}"
        );
    }
    let errors = cli.ok(&["--json", "errors", "--limit", "1000"]);
    // No row fails to parse. The one parse diagnostic allowed is the
    // warning that a "no end" date beyond the nanosecond range was
    // clamped, which the standing data of each month does contain.
    for line in errors.lines().filter(|l| l.contains("\"parse.")) {
        assert!(
            line.contains("parse.timestamp_clamped") && line.contains("\"severity\":\"warning\""),
            "{month}: {line}"
        );
    }
    assert!(
        errors.contains("parse.timestamp_clamped")
            && (errors.contains("2999/12/31 00:00:00") || errors.contains("9999/12/31 00:00:00")),
        "{month}: the clamp is reported: {errors}"
    );
    // The only schema diagnostics are the refusal of each known deviation
    // and, for a month without a census, a version its increment has not
    // recorded.
    let mut refused = 0;
    for line in errors
        .lines()
        .filter(|l| l.contains("\"schema.") && !l.contains("schema.unseen_version"))
    {
        assert!(
            line.contains("schema.deviates") && deviates(line),
            "{month}: {line}"
        );
        refused += 1;
    }
    assert_eq!(refused, deviating.len(), "{month}: {errors}");
    // Without --strict a deviating file is kept whole.
    if !deviating.is_empty() {
        let cli = Cli::new();
        let (code, out, err) = cli.code(&["--json", "ingest", heads.to_str().unwrap()]);
        assert_eq!(code, 0, "{month}: {err}{out}");
        let ls = cli.ok(&["--json", "ls", "--limit", "1000"]);
        assert_eq!(ls.lines().count(), files);
        let kept: Vec<&str> = ls.lines().filter(|l| deviates(l)).collect();
        assert_eq!(kept.len(), deviating.len(), "{ls}");
        for r in kept {
            assert!(
                r.contains("\"bad_rows\":0") && r.contains("\"resolution\":\"deviates\""),
                "{r}"
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
            && out.contains("conforms")
            && out.contains("schema.unknown_table"),
        "{out}"
    );
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
    let incs = cli.ok(&["schema", "increments", "DISPATCH_UNIT_SCADA"]);
    assert!(
        incs.contains("2015-01") && incs.contains("2024-10") && incs.contains("+LASTCHANGED"),
        "{incs}"
    );
    assert!(
        incs.contains("PUBLIC_ARCHIVE#DISPATCH_UNIT_SCADA#FILE01#202410.ctl"),
        "{incs}"
    );
    assert!(
        cli.ok(&["--json", "schema", "increments", "DISPATCH_UNIT_SCADA"])
            .starts_with('[')
    );
    assert_eq!(cli.code(&["schema", "increments", "NOPE"]).0, 1);
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
    // The plan of a shape: the table has four columns, the file of 2024
    // three, as the increment then lists, so the fourth is written null.
    let plan: serde_json::Value = serde_json::from_str(&cli.ok(&[
        "schema",
        "plan",
        "dispatch",
        "unit_scada",
        "1",
        "--columns",
        "SETTLEMENTDATE,DUID,SCADAVALUE",
        "--month",
        "2024-01",
    ]))
    .unwrap();
    assert_eq!(plan["resolution"], "conforms");
    assert_eq!(plan["version"], 1);
    assert_eq!(plan["plan"]["increment"], 1);
    assert_eq!(plan["plan"]["table"], "DISPATCH_UNIT_SCADA");
    assert_eq!(plan["plan"]["fields"].as_array().unwrap().len(), 4);
    assert_eq!(plan["plan"]["routes"].as_array().unwrap().len(), 3);
    assert_eq!(plan["plan"]["missing"], serde_json::json!([3]));
    assert_eq!(plan["plan"]["routes"][2]["cast"]["kind"], "float64");
    let (code, _, err) = cli.code(&["schema", "plan", "NOBODY", "KNOWS", "1", "--columns", "A"]);
    assert_eq!(code, 0, "{err}");
    let (code, _, err) = cli.code(&["schema", "plan", "X", "Y", "1", "--columns", ","]);
    assert_eq!(code, 5);
    assert!(err.contains("schema.no_columns"), "{err}");
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
        out.contains("schema.unknown_table") && out.contains("schema.deviates"),
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
fn reingest_brings_the_lake_up_to_the_schema() {
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
    assert!(ls.contains("\"plan_hash\":\"plan:"), "{ls}");
    assert!(
        ls.contains("\"source_columns\":[\"SETTLEMENTDATE\",\"UNIT\",\"VALUE\"]"),
        "{ls}"
    );
    // Nothing is behind yet.
    let out = cli.ok(&["reingest"]);
    assert!(out.contains("0 listed"), "{out}");

    // Adopt a schema for the table: the source is now behind it.
    user_table(&cli, "NEMLAKE_TEST_FOLLOWS", FOLLOWS);
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
    assert!(ls.contains("\"resolution\":\"conforms\""), "{ls}");
    assert!(cli.ok(&["verify"]).contains("no problems"));
    // Up to date again; an ordinary ingest of the same file is skipped.
    assert!(cli.ok(&["reingest"]).contains("0 listed"));
    assert!(cli.ok(&["ingest", csv_arg]).contains("1 skipped"));

    // Without the user's table the plan is the old one, and the source
    // is no longer there to read.
    std::fs::remove_file(&csv).unwrap();
    let (code, out, err) = cli.code(&["--no-user-schemas", "reingest"]);
    assert_ne!(code, 0, "{out}{err}");
    assert!(out.contains("cache.source_missing"), "{out}");
    assert_eq!(parquet(), after, "what is in the lake stays");
}

/// A user's fix for files that deviate from their increment, a column and
/// an increment, puts exactly the sources it touches behind.
#[test]
fn a_fix_in_table_toml_brings_in_the_files_that_deviate() {
    let cli = Cli::new();
    let write = |name: &str, stamp: &str, quality: bool| -> String {
        let p = cli.home.path().join(name);
        let (cols, extra) = if quality {
            (",QUALITY", ",GOOD")
        } else {
            ("", "")
        };
        std::fs::write(
            &p,
            format!(
                "C,NEMP.WORLD,TEST\nI,NEMLAKE_TEST,FIXED,1,SETTLEMENTDATE,UNIT,VALUE{cols}\nD,NEMLAKE_TEST,FIXED,1,\"{stamp}\",A1,2.5{extra}\nC,\"END OF REPORT\",4\n"
            ),
        )
        .unwrap();
        p.to_str().unwrap().to_owned()
    };
    let old = write(
        "PUBLIC_DVD_FIXED_201003010000.CSV",
        "2010/03/01 00:05:00",
        false,
    );
    let new = write(
        "PUBLIC_DVD_FIXED_202403010000.CSV",
        "2024/03/01 00:05:00",
        true,
    );
    let table = user_table(&cli, "NEMLAKE_TEST_FIXED", FIXED);
    let (code, out, err) = cli.code(&["ingest", &old, "--strict"]);
    assert_eq!(code, 0, "{out}{err}");
    // The 2024 file carries a column the increment does not list: strict
    // mode refuses it, and otherwise it is kept as text of its own.
    let (code, out, _) = cli.code(&["ingest", &new, "--strict"]);
    assert_ne!(code, 0, "{out}");
    assert!(out.contains("schema.deviates"), "{out}");
    let (code, out, _) = cli.code(&["ingest", &new]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("schema.deviates"), "{out}");

    // The fix: QUALITY is a column of the table from 2020 on.
    let fixed = FIXED.replace("next_column_id = 4", "next_column_id = 5").replace(
        "]\n\n[[increment]]",
        "    { id = 4, name = \"QUALITY\", type = \"string\", nullable = true },\n]\n\n[[increment]]",
    ) + "\n[[increment]]\nn = 2\nfrom = \"2020-01\"\ncolumns = [\"SETTLEMENTDATE\", \"UNIT\", \"VALUE\", \"QUALITY\"]\nnote = \"Files carry QUALITY from 2020.\"\n";
    std::fs::write(&table, &fixed).unwrap();
    assert!(
        cli.ok(&["schema", "increments", "NEMLAKE_TEST_FIXED"])
            .contains("+QUALITY")
    );
    // Only the 2024 file is behind: the 2010 file's shape does not carry
    // the new column, so the plan it is read by has not changed.
    let out = cli.ok(&["reingest", "--dry-run"]);
    assert!(
        out.contains("1 listed") && out.contains("FIXED_202403"),
        "{out}"
    );
    let (code, out, err) = cli.code(&["reingest", "--strict"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("1 ingested"), "{out}");
    let ls = cli.ok(&["--json", "ls"]);
    for period in ["2010-03", "2024-03"] {
        let line = ls
            .lines()
            .find(|l| l.contains(&format!("\"source_period\":\"{period}\"")))
            .unwrap_or_else(|| panic!("no ingest of {period}: {ls}"));
        assert!(line.contains("\"resolution\":\"conforms\""), "{line}");
    }
    assert!(cli.ok(&["reingest"]).contains("0 listed"));

    // An increment that lists a column the table does not have is refused
    // with the reason.
    std::fs::write(&table, fixed.replace("\"QUALITY\"]", "\"NOWHERE\"]")).unwrap();
    let (code, _, err) = cli.code(&["schema", "list"]);
    assert_ne!(code, 0);
    assert!(err.contains("NOWHERE, which is not a column"), "{err}");
}

/// A table in `{home}/schemas/` is in effect for every command, without a
/// rebuild; one of the same name replaces the built-in table.
#[test]
fn user_schemas_take_effect() {
    let cli = Cli::new();
    // Two report pairs no table claims, in one file.
    let new = cli.home.path().join("new_pairs.csv");
    std::fs::write(
        &new,
        "C,NEMP.WORLD,TEST\nI,NEMLAKE_TEST,FIRST,1,SETTLEMENTDATE,UNIT,VALUE\nD,NEMLAKE_TEST,FIRST,1,\"2024/01/01 00:05:00\",A1,1.5\nI,NEMLAKE_TEST,SECOND,1,SETTLEMENTDATE,FLAG\nD,NEMLAKE_TEST,SECOND,1,\"2024/01/01 00:05:00\",Y\nC,\"END OF REPORT\",6\n",
    )
    .unwrap();
    let new = new.to_str().unwrap();
    let (code, out, _) = cli.code(&["inspect", new]);
    assert_eq!(code, 5, "{out}");
    user_table(&cli, "NEMLAKE_TEST_FIRST", &FIXED.replace("FIXED", "FIRST"));
    user_table(
        &cli,
        "NEMLAKE_TEST_SECOND",
        &FIXED
            .replace("FIXED", "SECOND")
            .replace("UNIT", "FLAG")
            .replace(", \"VALUE\"]", "]")
            .replace(
                "    { id = 3, name = \"VALUE\", type = \"double\", nullable = true },\n",
                "",
            ),
    );
    let (code, out, err) = cli.code(&["inspect", new]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(
        out.contains("NEMLAKE_TEST_FIRST") && !out.contains("schema.unknown_table"),
        "{out}"
    );
    assert!(cli.ok(&["schema", "list"]).contains("NEMLAKE_TEST_SECOND"));
    // The file now ingests in strict mode.
    let (code, out, err) = cli.code(&["ingest", new, "--strict"]);
    assert_eq!(code, 0, "{out}{err}");
    let ls = cli.ok(&["--json", "ls", "--limit", "100"]);
    assert!(ls.contains("\"table\":\"NEMLAKE_TEST_FIRST\""), "{ls}");
    assert!(ls.contains("\"table\":\"NEMLAKE_TEST_SECOND\""), "{ls}");
    assert!(ls.contains("\"schema_source\":\"user\""), "{ls}");
    // Without the user's tables the pairs are unknown again.
    let (code, _, _) = cli.code(&["--no-user-schemas", "inspect", new]);
    assert_eq!(code, 5);

    // A built-in table published with a new column: refused in strict
    // mode until the user's own DISPATCH_UNIT_SCADA lists it.
    let v2 = cli.home.path().join("scada_v2.csv");
    std::fs::write(
        &v2,
        "C,NEMP.WORLD,TEST\nI,DISPATCH,UNIT_SCADA,2,SETTLEMENTDATE,DUID,SCADAVALUE,LASTCHANGED,QUALITY\nD,DISPATCH,UNIT_SCADA,2,\"2024/01/01 00:05:00\",BW01,1.5,\"2024/01/01 00:06:00\",GOOD\nC,\"END OF REPORT\",4\n",
    )
    .unwrap();
    let v2 = v2.to_str().unwrap();
    let (code, out, err) = cli.code(&["ingest", v2, "--strict"]);
    assert_ne!(code, 0, "a deviation is refused in strict mode: {out}{err}");
    let show = cli.ok(&["schema", "show", "DISPATCH_UNIT_SCADA"]);
    let toml = &show[..show.find("\n# Arrow lake schema").unwrap()];
    let ours = toml
        .lines()
        .filter(|l| !l.starts_with("ctl"))
        .collect::<Vec<_>>()
        .join("\n")
        .replace("next_column_id = 5", "next_column_id = 6")
        .replacen(
            "]\n\n[[increment]]",
            "    { id = 5, name = \"QUALITY\", type = \"string\", nullable = true },\n]\n\n[[increment]]",
            1,
        )
        + "\n\n[[increment]]\nn = 3\nfrom = \"2026-09\"\naemo_versions = [2]\ncolumns = [\"SETTLEMENTDATE\", \"DUID\", \"SCADAVALUE\", \"LASTCHANGED\", \"QUALITY\"]\n";
    user_table(&cli, "DISPATCH_UNIT_SCADA", &ours);
    let (code, out, err) = cli.code(&["ingest", v2, "--strict"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(
        cli.ok(&["schema", "increments", "DISPATCH_UNIT_SCADA"])
            .contains("+QUALITY")
    );

    // A broken file in the user's directory stops every command, and says how to go on.
    user_table(&cli, "BROKEN", "schema_format = 2\nname = 3\n");
    let (code, _, err) = cli.code(&["schema", "list"]);
    assert_ne!(code, 0);
    assert!(
        err.contains("BROKEN") && err.contains("--no-user-schemas"),
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
fn the_archive_lists_and_verifies_every_download() {
    let cli = Cli::new();
    assert!(cli.ok(&["archive", "ls"]).contains("0 file(s)"));
    assert!(cli.ok(&["--json", "archive", "ls"]).is_empty());
    assert!(cli.ok(&["archive", "verify"]).contains("0 file(s) checked"));

    // One archived download, at the path its URL gives it, with its record
    // (here as an older nemlake wrote it, with more than this one keeps).
    let dir = cli.home.path().join("archive/h/DATA");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a.zip");
    std::fs::write(&file, b"zip bytes").unwrap();
    std::fs::write(
        dir.join("a.zip.json"),
        format!(
            r#"{{"url":"https://h/DATA/a.zip","size":9,"sha256":"{}","fetched_at":"2026-10-03T10:00:05Z","last_modified":"Tue, 13 Feb 2024 08:27:00 GMT","request":{{"by":"fetch nem REGION 2024-01..2024-01"}}}}"#,
            nemlake_acquire::archive::sha256_hex(b"zip bytes")
        ),
    )
    .unwrap();
    // A partial download beside it is not an archived file.
    std::fs::write(dir.join("b.zip.part"), b"half").unwrap();
    let out = cli.ok(&["archive", "ls"]);
    assert!(out.contains("1 file(s)"), "{out}");
    assert!(
        out.contains("https://h/DATA/a.zip")
            && out.contains("2026-10-03T10:00:05Z")
            && out.contains("Tue, 13 Feb 2024"),
        "{out}"
    );
    let out = cli.ok(&["--json", "archive", "ls"]);
    assert_eq!(out.lines().count(), 1, "{out}");
    assert!(out.contains("\"record\""), "{out}");
    assert!(
        cli.ok(&["archive", "verify"])
            .contains("1 file(s) checked, 0 do not match")
    );
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
        "jobs = 1\nstrict = true\n",
    )
    .unwrap();
    assert!(cli.ok(&["-v", "schema", "list"]).contains("DISPATCH"));
}

/// A user's own table, as `{home}/schemas/nem/{TABLE}/table.toml`; the
/// path of the file.
fn user_table(cli: &Cli, name: &str, text: &str) -> PathBuf {
    let dir = cli.home.path().join("schemas/nem").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("table.toml");
    std::fs::write(&path, text).unwrap();
    path
}

/// A user's table for (NEMLAKE_TEST, FOLLOWS), as a person writes one.
const FOLLOWS: &str = r#"schema_format = 2
name = "NEMLAKE_TEST_FOLLOWS"
report_pairs = [["NEMLAKE_TEST", "FOLLOWS"]]
load_mode = "append"
dedupe = "none"
time_column = "SETTLEMENTDATE"
next_column_id = 4

[[column]]
id = 1
name = "SETTLEMENTDATE"
type = "timestamp_ns"

[[column]]
id = 2
name = "UNIT"
type = "string"

[[column]]
id = 3
name = "VALUE"
type = "double"

[[increment]]
n = 1
from = "2009-07"
columns = ["SETTLEMENTDATE", "UNIT", "VALUE"]
"#;

/// A user's table for (NEMLAKE_TEST, FIXED), in the form nemlake writes.
const FIXED: &str = r#"schema_format = 2
name = "NEMLAKE_TEST_FIXED"
report_pairs = [["NEMLAKE_TEST", "FIXED"]]
load_mode = "append"
dedupe = "none"
primary_key = []
time_column = "SETTLEMENTDATE"
next_column_id = 4

column = [
    { id = 1, name = "SETTLEMENTDATE", type = "timestamp_ns", nullable = true },
    { id = 2, name = "UNIT", type = "string", nullable = true },
    { id = 3, name = "VALUE", type = "double", nullable = true },
]

[[increment]]
n = 1
from = "2009-07"
aemo_versions = [1]
columns = ["SETTLEMENTDATE", "UNIT", "VALUE"]
"#;

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
