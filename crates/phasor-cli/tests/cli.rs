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

/// Copies the directory `from` into `to`, whole.
fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let p = e.unwrap().path();
        let dest = to.join(p.file_name().unwrap());
        if p.is_dir() {
            copy_dir(&p, &dest);
        } else {
            std::fs::copy(&p, &dest).unwrap();
        }
    }
}

impl Cli {
    fn new() -> Self {
        Self {
            home: tempfile::tempdir().unwrap(),
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_phasor"))
            .arg("--home")
            .arg(self.home.path())
            .arg("--no-progress")
            .args(args)
            .env_remove("PHASOR_HOME")
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
        "import",
        scada.to_str().unwrap(),
        zip_path.to_str().unwrap(),
    ]);
    assert!(out.contains("2 listed, 2 ingested"), "{out}");
    // Idempotent.
    let out = cli.ok(&["import", scada.to_str().unwrap()]);
    assert!(out.contains("1 skipped"), "{out}");
    let ls = cli.ok(&["list", "--ingests"]);
    assert!(
        ls.contains("DISPATCH_UNIT_SCADA") && ls.contains("REGION"),
        "{ls}"
    );
    let ls_json = cli.ok(&["--json", "list", "REGION", "--ingests"]);
    assert!(ls_json.contains("\"table\":\"REGION\""), "{ls_json}");
    let runs = cli.ok(&["list", "--runs"]);
    assert!(runs.contains("import"), "{runs}");
    let runs_json = cli.ok(&["--json", "list", "--runs"]);
    assert!(runs_json.contains("\"exit_code\":0"), "{runs_json}");
    assert!(cli.ok(&["errors"]).contains("no diagnostics"));
    assert!(cli.ok(&["--json", "errors", "--last"]).is_empty());
    // Every table of the registry has its history: REGION's Iceberg
    // table is built from it. A table derived from a file has none.
    let built = cli.ok(&["--json", "iceberg", "--table", "REGION"]);
    assert!(built.contains("\"snapshots\":\"1\""), "{built}");
    let unknown = cli.home.path().join("unknown.csv");
    std::fs::write(
        &unknown,
        "C,NEMP.WORLD,TEST\nI,PHASOR_TEST,NOBODY,1,A\nD,PHASOR_TEST,NOBODY,1,x\nC,\"END OF REPORT\",3\n",
    )
    .unwrap();
    cli.ok(&["import", unknown.to_str().unwrap()]);
    // `--kit` names the kit that reads every file: a GBB file under the
    // feed's folders reads as its declared table; the same bytes under a
    // name no feed's rule reads name no block; an MMS file read by the NEM
    // kit whatever its name; a kit the binary lacks is refused.
    let gbb_dir = cli.home.path().join("mirror/Reports/CURRENT/GBB");
    std::fs::create_dir_all(&gbb_dir).unwrap();
    let gbb = gbb_dir.join("GasBBActualFlowStorageLast31.CSV");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testdata/plain/gbb_actual_flow_storage_last31.csv"),
        &gbb,
    )
    .unwrap();
    let out = cli.ok(&["import", gbb.to_str().unwrap(), "--kit", "gas"]);
    assert!(out.contains("1 listed, 1 ingested"), "{out}");
    assert!(
        cli.ok(&["list", "--ingests"])
            .contains("GBB_ACTUAL_FLOW_STORAGE"),
        "the declared gas table"
    );
    let loose = cli.home.path().join("flows.csv");
    std::fs::copy(&gbb, &loose).unwrap();
    let (code, out, err) = cli.code(&["import", loose.to_str().unwrap(), "--kit", "gas"]);
    assert_ne!(code, 0);
    assert!(
        format!("{out}{err}").contains("csv.unnamed_block"),
        "{out}{err}"
    );
    let renamed = cli.home.path().join("facility-scada-2023-10.csv");
    std::fs::copy(fixtures().join("REGION_v1_head.csv"), &renamed).unwrap();
    let out = cli.ok(&["import", renamed.to_str().unwrap(), "--kit", "nem"]);
    assert!(out.contains("1 ingested"), "{out}");
    let notes = cli.home.path().join("notes.txt");
    std::fs::write(&notes, "x").unwrap();
    let (_, out, err) = cli.code(&["import", notes.to_str().unwrap(), "--kit", "nem"]);
    assert!(
        format!("{out}{err}").contains("not a file the nem kit reads"),
        "{out}{err}"
    );
    let (code, _, err) = cli.code(&["import", gbb.to_str().unwrap(), "--kit", "mars"]);
    assert_eq!(code, 1);
    assert!(
        err.contains("No kit named mars") && err.contains("gas, wem, nem"),
        "{err}"
    );
    // `ingest` reads a gas table from the feed that publishes it.
    let (code, out, err) = cli.code(&[
        "--offline",
        "ingest",
        "GBB_ACTUAL_FLOW_STORAGE",
        "--from",
        "2026-10",
        "--to",
        "2026-10",
        "--dry-run",
    ]);
    assert_eq!(code, 0, "{out}{err}");
    let (code, _, err) = cli.code(&[
        "--offline",
        "ingest",
        "GBB_ACTUAL_FLOW_STORAGE",
        "--from",
        "2026-10",
        "--to",
        "2026-10",
        "--source",
        "gas-sttm",
    ]);
    assert!(code == 0 || !err.contains("has no source"), "{err}");
    let (code, out, _) = cli.code(&["iceberg", "--table", "PHASOR_TEST_NOBODY"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("PHASOR_TEST_NOBODY") && out.contains("no history"),
        "{out}"
    );
    let (code, out, _) = cli.code(&["--json", "iceberg", "--table", "NONE"]);
    assert_eq!((code, out.as_str()), (0, ""));
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
    // SCADA's Iceberg table: the ingest left it current, and a rebuild
    // gives the same.
    let meta = cli
        .home
        .path()
        .join("lake/nem/dispatch_unit_scada/metadata/version-hint.text");
    assert!(meta.is_file(), "the ingest builds the table's metadata");
    // The lake's table carries the declaration phasor-schemas publishes.
    let hint = std::fs::read_to_string(&meta).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(
        &std::fs::read(meta.with_file_name(format!("v{}.metadata.json", hint.trim()))).unwrap(),
    )
    .unwrap();
    // A table's key and roles are a reader's: the lake carries neither.
    assert!(doc["properties"].get("phasor.temporal.entity").is_none());
    assert!(doc["properties"].get("phasor.key").is_none());
    assert_eq!(doc["properties"]["phasor.lake.format"], "5");
    let built = cli.ok(&["--json", "iceberg", "--table", "DISPATCH_UNIT_SCADA"]);
    assert!(built.contains("\"snapshots\":\"1\""), "{built}");
    assert!(meta.is_file());
    // A home copied whole names its own files: the manifest's paths are the
    // home's, `iceberg` writes the metadata for where the lake is now, and
    // nothing of the old place is left in it.
    let moved = Cli::new();
    copy_dir(cli.home.path(), moved.home.path());
    let verify = moved.ok(&["verify"]);
    assert!(verify.contains("verified, no problems"), "{verify}");
    moved.ok(&["iceberg", "--table", "DISPATCH_UNIT_SCADA"]);
    let table = moved.home.path().join("lake/nem/dispatch_unit_scada");
    let old = cli.home.path().display().to_string();
    let new = moved.home.path().display().to_string();
    for e in std::fs::read_dir(table.join("metadata")).unwrap() {
        let bytes = std::fs::read(e.unwrap().path()).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains(&old), "{text}");
    }
    let hint = std::fs::read_to_string(table.join("metadata/version-hint.text")).unwrap();
    let doc =
        std::fs::read_to_string(table.join(format!("metadata/v{}.metadata.json", hint.trim())))
            .unwrap();
    assert!(
        doc.contains(&format!(
            "\"location\":\"file://{new}/lake/nem/dispatch_unit_scada\""
        )),
        "{doc}"
    );
    // A window the flags cannot read, and a table nobody declared.
    let (code, _, err) = cli.code(&[
        "--offline",
        "fetch",
        "DISPATCH_UNIT_SCADA",
        "--from",
        "soon",
        "--to",
        "2024-02-01",
    ]);
    assert!(code != 0 && err.contains("not a month, a day"), "{err}");
    // Documents offline: each refused, said, and the exit says the network.
    let (code, _, err) = cli.code(&["--offline", "fetch", "--documents"]);
    assert!(
        code == 4 && err.contains("Registration-and-Exemption-List"),
        "{err}"
    );
    // A plan that cannot list offline: the exit says the network, and only
    // offline does (a refusal or a server error is a failure, 1).
    let (code, _, err) = cli.code(&["--offline", "fetch", "DISPATCHPRICE", "--at", "2024-01-01"]);
    assert_eq!(code, 4, "{err}");
    let (code, _, err) = cli.code(&["--offline", "fetch", "NOBODY", "--at", "2024-01-01"]);
    assert!(
        code != 0 && err.contains("No table is named NOBODY"),
        "{err}"
    );
    // What there is.
    let tables = cli.ok(&["list"]);
    assert!(
        tables.contains("DISPATCH_UNIT_SCADA") && tables.contains("SETTLEMENTDATE"),
        "{tables}"
    );
    let grep = cli.ok(&["--json", "list", "--grep", "scada"]);
    assert!(
        grep.contains("\"table\":\"DISPATCH_UNIT_SCADA\"") && !grep.contains("REGION"),
        "{grep}"
    );
    let cols = cli.ok(&["list", "DISPATCH_UNIT_SCADA"]);
    assert!(
        cols.contains("SCADAVALUE") && cols.contains("time"),
        "{cols}"
    );
    assert!(
        cols.contains("Dispatchable Unit Identifier"),
        "the data model's words: {cols}"
    );
    let cov = cli.ok(&["list", "DISPATCH_UNIT_SCADA", "--coverage"]);
    assert!(
        cov.contains("year=2024/month=01") && cov.contains("no gap"),
        "{cov}"
    );
    let files = cli.ok(&["list", "DISPATCH_UNIT_SCADA", "--files"]);
    assert!(
        files.contains("1 file(s)") && files.contains("local"),
        "{files}"
    );
    // The lake's checks leave the Iceberg metadata beside the files alone.
    assert!(cli.ok(&["verify"]).contains("no problems"));
    std::fs::remove_dir_all(meta.parent().unwrap()).unwrap();
    std::fs::remove_dir_all(cli.home.path().join("lake/nem/region/metadata")).unwrap();
    // Snapshot partition for REGION, month partition for SCADA.
    let lake = cli.home.path().join("lake/nem");
    assert!(lake.join("region/data/snapshot=202401").is_dir());
    assert!(
        lake.join("dispatch_unit_scada/data/year=2024/month=01")
            .is_dir()
    );
    // A directory argument ingests its zips and CSVs; an empty one lists nothing.
    let empty = cli.home.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let out = cli.ok(&["import", empty.to_str().unwrap()]);
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
    let (code, out, err) = cli.code(&["import", bad.to_str().unwrap()]);
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
            .contains("the first is line 2"),
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
    let failed = cli.ok(&["list", "--ingests", "--failed"]);
    assert!(failed.contains("failed"), "{failed}");
    // Tolerating the row succeeds, in JSON too.
    let cli2 = Cli::new();
    let (code, out, _) = cli2.code(&[
        "--json",
        "import",
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
    let (code, out, err) = cli3.code(&["import", misfit.to_str().unwrap()]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("2 rows (0 quarantined)"), "{out}");
    assert!(out.contains("warning parse.value_kept_raw ×1"), "{out}");
    let (code, out, err) = cli3.code(&["import", misfit.to_str().unwrap(), "--strict"]);
    assert_eq!(code, 0, "already ingested: {out}{err}");
    let cli4 = Cli::new();
    let (code, out, err) = cli4.code(&["import", misfit.to_str().unwrap(), "--strict"]);
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
    let (code, out, _) = cli.code(&["import", strict.to_str().unwrap(), "--strict"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("csv.footer_count_mismatch"), "{out}");
    // Fail-fast stops at the first bad row even when the tolerance would allow it.
    let (code, out, _) = cli.code(&[
        "import",
        bad.to_str().unwrap(),
        "--fail-fast",
        "--max-bad-rows",
        "5",
    ]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("csv.stopped_fail_fast"), "{out}");
}

/// Every block in the heads of two months resolves to a schema strictly and every row parses.
#[test]
fn every_archive_table_ingests_strictly() {
    every_head_ingests_strictly("MMSDM_2024_01", 161, &[]);
    every_head_ingests_strictly("MMSDM_2026_08", 309, &[]);
}

fn every_head_ingests_strictly(month: &str, at_least: usize, deviating: &[&str]) {
    let heads = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/bootstrap")
        .join(month)
        .join("heads");
    let files = std::fs::read_dir(&heads).unwrap().count();
    assert!(files >= at_least, "{files} head fixtures of {month}");
    let deviates = |l: &str| {
        deviating
            .iter()
            .any(|d| l.contains(&format!("_{d}_")) || l.contains(&format!("#{d}#")))
    };
    let cli = Cli::new();
    let (code, out, err) = cli.code(&["--json", "import", heads.to_str().unwrap(), "--strict"]);
    assert_eq!(
        code == 0,
        deviating.is_empty(),
        "{month}: {err}{}",
        &out[out.len().saturating_sub(2000)..]
    );
    let ls = cli.ok(&["--json", "list", "--ingests", "--limit", "1000"]);
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
    // and a version no run of the table's history came under.
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
        let (code, out, err) = cli.code(&["--json", "import", heads.to_str().unwrap()]);
        assert_eq!(code, 0, "{month}: {err}{out}");
        let ls = cli.ok(&["--json", "list", "--ingests", "--limit", "1000"]);
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
            "{}I,PHASOR_TEST,NEW_THING,1,WHEN,AMOUNT\nD,PHASOR_TEST,NEW_THING,1,\"2026/10/01 13:40:00\",1.5\n{}",
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
    let history = cli.ok(&["schema", "history", "DISPATCH_UNIT_SCADA"]);
    assert!(
        history.contains("2010-11")
            && history.contains("2024-10")
            && history.contains("+LASTCHANGED")
            && history.contains("DISPATCH.UNIT_SCADA"),
        "{history}"
    );
    let json: serde_json::Value =
        serde_json::from_str(&cli.ok(&["--json", "schema", "history", "DISPATCH_UNIT_SCADA"]))
            .unwrap();
    assert_eq!(json["format-version"], 3);
    assert_eq!(json["schemas"][0]["schema-id"], 0);
    assert_eq!(cli.code(&["schema", "history", "NOPE"]).0, 1);
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
    // three, as its schema 0 lists, so the fourth is written null.
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
    assert_eq!(plan["version"], 0);
    assert_eq!(plan["plan"]["schema"], 0);
    assert_eq!(plan["plan"]["table"], "DISPATCH_UNIT_SCADA");
    assert_eq!(plan["plan"]["fields"].as_array().unwrap().len(), 4);
    assert_eq!(plan["plan"]["routes"].as_array().unwrap().len(), 3);
    assert_eq!(plan["plan"]["missing"], serde_json::json!([3]));
    assert_eq!(
        plan["plan"]["routes"][2]["cast"],
        serde_json::json!({"kind": "decimal128", "precision": 16, "scale": 6})
    );
    let (code, _, err) = cli.code(&["schema", "plan", "NOBODY", "KNOWS", "1", "--columns", "A"]);
    assert_eq!(code, 0, "{err}");
    let (code, _, err) = cli.code(&["schema", "plan", "X", "Y", "1", "--columns", ","]);
    assert_eq!(code, 5);
    assert!(err.contains("schema.no_columns"), "{err}");
}

/// An undeclared table and column are ingested as text and reported; strict mode refuses them.
#[test]
fn what_the_registry_does_not_declare_is_kept_not_skipped() {
    let cli = Cli::new();
    let csv = cli.home.path().join("PUBLIC_DVD_NOBODY_202403010000.CSV");
    std::fs::write(
        &csv,
        "C,NEMP.WORLD,TEST\nI,PHASOR_TEST,NOBODY,1,CONTRACTYEAR,LABEL\nD,PHASOR_TEST,NOBODY,1,2024,first\nD,PHASOR_TEST,NOBODY,1,2025,second\nI,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID,SCADAVALUE,QUALITY\nD,DISPATCH,UNIT_SCADA,1,\"2024/03/01 00:05:00\",BW01,1.5,GOOD\nC,\"END OF REPORT\",7\n",
    )
    .unwrap();
    let csv = csv.to_str().unwrap();
    let (code, out, err) = cli.code(&["import", csv]);
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
        .join("lake/nem/_unregistered/phasor_test.nobody/data/year=2024/month=03");
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
        .join("lake/nem/dispatch_unit_scada/data/year=2024/month=03");
    assert_eq!(std::fs::read_dir(&scada).unwrap().count(), 1);
    let ls = cli.ok(&["--json", "list", "--ingests"]);
    assert_eq!(ls.lines().count(), 2, "{ls}");
    assert!(
        ls.contains("\"table\":\"PHASOR_TEST_NOBODY\"") && ls.contains("\"rows\":2"),
        "{ls}"
    );

    // Strict mode refuses the file instead.
    let strict = cli.home.path().join("PUBLIC_DVD_NOBODY_202404010000.CSV");
    std::fs::copy(csv, &strict).unwrap();
    let (code, out, err) = cli.code(&["import", strict.to_str().unwrap(), "--strict"]);
    assert_ne!(code, 0, "{out}{err}");
    assert!(out.contains("schema.unknown_table"), "{out}");
}

/// An undeclared table is text until a schema is adopted; `reingest` then rewrites it typed.
#[test]
fn reingest_brings_the_lake_up_to_the_schema() {
    let cli = Cli::new();
    let csv = cli.home.path().join("PUBLIC_DVD_FOLLOWS_202403010000.CSV");
    std::fs::write(
        &csv,
        "C,NEMP.WORLD,TEST\nI,PHASOR_TEST,FOLLOWS,1,SETTLEMENTDATE,UNIT,VALUE\nD,PHASOR_TEST,FOLLOWS,1,\"2024/03/01 00:05:00\",A1,1.5\nD,PHASOR_TEST,FOLLOWS,1,\"2024/03/01 00:10:00\",A1,2.5\nC,\"END OF REPORT\",5\n",
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

    let (code, out, err) = cli.code(&["import", csv_arg]);
    assert_eq!(code, 0, "{out}{err}");
    let before = parquet();
    assert_eq!(before.len(), 1, "{before:?}");
    assert!(
        before[0].contains("nem/_unregistered/phasor_test.follows/"),
        "{before:?}"
    );
    let ls = cli.ok(&["--json", "list", "--ingests"]);
    assert!(ls.contains("\"schema_source\":\"file\""), "{ls}");
    assert!(ls.contains("\"resolution\":\"derived\""), "{ls}");
    assert!(ls.contains("\"plan_hash\":\"plan:"), "{ls}");
    assert!(
        ls.contains("\"source_columns\":[\"SETTLEMENTDATE\",\"UNIT\",\"VALUE\"]"),
        "{ls}"
    );
    // Nothing is behind yet.
    let out = cli.ok(&["ingest"]);
    assert!(out.contains("0 listed"), "{out}");

    // Adopt a schema for the table: the source is now behind it.
    user_table(&cli, "PHASOR_TEST_FOLLOWS", FOLLOWS);
    let out = cli.ok(&["ingest", "--dry-run"]);
    assert!(
        out.contains("1 listed") && out.contains("PUBLIC_DVD_FOLLOWS"),
        "{out}"
    );
    assert_eq!(parquet(), before, "a dry run writes nothing");
    let (code, out, err) = cli.code(&["ingest"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("1 listed, 1 ingested"), "{out}");
    assert!(out.contains("manifest.rules_changed"), "{out}");
    let after = parquet();
    assert_eq!(after.len(), 1, "{after:?}");
    assert!(after[0].contains("nem/phasor_test_follows/"), "{after:?}");
    assert!(
        !lake.join("nem/_unregistered").exists(),
        "the directory the text files were in is gone with them"
    );
    let ls = cli.ok(&["--json", "list", "--ingests"]);
    assert_eq!(ls.lines().count(), 1, "{ls}");
    assert!(ls.contains("\"schema_source\":\"user\""), "{ls}");
    assert!(ls.contains("\"resolution\":\"conforms\""), "{ls}");
    assert!(cli.ok(&["verify"]).contains("no problems"));
    // Up to date again; an ordinary ingest of the same file is skipped.
    assert!(cli.ok(&["ingest"]).contains("0 listed"));
    assert!(cli.ok(&["import", csv_arg]).contains("1 skipped"));

    // Without the user's table the plan is the old one, and the source
    // is no longer there to read.
    std::fs::remove_file(&csv).unwrap();
    let (code, out, err) = cli.code(&["--no-user-schemas", "ingest"]);
    assert_ne!(code, 0, "{out}{err}");
    assert!(out.contains("cache.source_missing"), "{out}");
    assert_eq!(parquet(), after, "what is in the lake stays");
}

/// A user's fix for deviating files puts exactly the sources it touches behind.
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
                "C,NEMP.WORLD,TEST\nI,PHASOR_TEST,FIXED,1,SETTLEMENTDATE,UNIT,VALUE{cols}\nD,PHASOR_TEST,FIXED,1,\"{stamp}\",A1,2.5{extra}\nC,\"END OF REPORT\",4\n"
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
    let table = user_table(&cli, "PHASOR_TEST_FIXED", FIXED);
    let (code, out, err) = cli.code(&["import", &old, "--strict"]);
    assert_eq!(code, 0, "{out}{err}");
    // The 2024 file carries a column the table's schema does not list:
    // strict mode refuses it, and otherwise it is kept as text of its own.
    let (code, out, _) = cli.code(&["import", &new, "--strict"]);
    assert_ne!(code, 0, "{out}");
    assert!(out.contains("schema.deviates"), "{out}");
    let (code, out, _) = cli.code(&["import", &new]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("schema.deviates"), "{out}");

    // The fix: QUALITY is a column of the table, and its files from 2020
    // on carry it.
    let fixed = FIXED.replace(
        "]\n\n[[schema]]",
        "    { id = 4, name = \"QUALITY\", type = \"string\", nullable = true, note = \"Files carry QUALITY from 2020.\" },\n]\n\n[[schema]]",
    ) + "\n[[schema]]\nid = 1\nfrom = \"2020-01\"\nversions = [1]\ncolumns = [\"SETTLEMENTDATE\", \"UNIT\", \"VALUE\", \"QUALITY\"]\n";
    std::fs::write(&table, &fixed).unwrap();
    assert!(
        cli.ok(&["schema", "history", "PHASOR_TEST_FIXED"])
            .contains("+QUALITY")
    );
    // Only the 2024 file is behind: the 2010 file's shape does not carry
    // the new column, so the plan it is read by has not changed.
    let out = cli.ok(&["ingest", "--dry-run"]);
    assert!(
        out.contains("1 listed") && out.contains("FIXED_202403"),
        "{out}"
    );
    let (code, out, err) = cli.code(&["ingest", "--strict"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("1 ingested"), "{out}");
    let ls = cli.ok(&["--json", "list", "--ingests"]);
    for period in ["2010-03", "2024-03"] {
        let line = ls
            .lines()
            .find(|l| l.contains(&format!("\"source_period\":\"{period}\"")))
            .unwrap_or_else(|| panic!("no ingest of {period}: {ls}"));
        assert!(line.contains("\"resolution\":\"conforms\""), "{line}");
    }
    assert!(cli.ok(&["ingest"]).contains("0 listed"));

    // A schema that lists a column the table does not have is refused
    // with the reason.
    std::fs::write(&table, fixed.replace("\"QUALITY\"]", "\"NOWHERE\"]")).unwrap();
    let (code, _, err) = cli.code(&["schema", "list"]);
    assert_ne!(code, 0);
    assert!(err.contains("NOWHERE, which is not a column"), "{err}");
}

/// A table in `{home}/schemas/` takes effect for every command and replaces a built-in one.
#[test]
fn user_schemas_take_effect() {
    let cli = Cli::new();
    // Two report pairs no table claims, in one file.
    let new = cli.home.path().join("new_pairs.csv");
    std::fs::write(
        &new,
        "C,NEMP.WORLD,TEST\nI,PHASOR_TEST,FIRST,1,SETTLEMENTDATE,UNIT,VALUE\nD,PHASOR_TEST,FIRST,1,\"2024/01/01 00:05:00\",A1,1.5\nI,PHASOR_TEST,SECOND,1,SETTLEMENTDATE,FLAG\nD,PHASOR_TEST,SECOND,1,\"2024/01/01 00:05:00\",Y\nC,\"END OF REPORT\",6\n",
    )
    .unwrap();
    let new = new.to_str().unwrap();
    let (code, out, _) = cli.code(&["inspect", new]);
    assert_eq!(code, 5, "{out}");
    user_table(&cli, "PHASOR_TEST_FIRST", &FIXED.replace("FIXED", "FIRST"));
    user_table(
        &cli,
        "PHASOR_TEST_SECOND",
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
        out.contains("PHASOR_TEST_FIRST") && !out.contains("schema.unknown_table"),
        "{out}"
    );
    assert!(cli.ok(&["schema", "list"]).contains("PHASOR_TEST_SECOND"));
    // The file now ingests in strict mode.
    let (code, out, err) = cli.code(&["import", new, "--strict"]);
    assert_eq!(code, 0, "{out}{err}");
    let ls = cli.ok(&["--json", "list", "--ingests", "--limit", "100"]);
    assert!(ls.contains("\"table\":\"PHASOR_TEST_FIRST\""), "{ls}");
    assert!(ls.contains("\"table\":\"PHASOR_TEST_SECOND\""), "{ls}");
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
    let (code, out, err) = cli.code(&["import", v2, "--strict"]);
    assert_ne!(code, 0, "a deviation is refused in strict mode: {out}{err}");
    let show = cli.ok(&["schema", "show", "DISPATCH_UNIT_SCADA"]);
    let toml = &show[..show.find("\n# Arrow lake schema").unwrap()];
    let ours = toml.replacen(
        "]\n\n[[schema]]",
        "    { id = 5, name = \"QUALITY\", type = \"string\", nullable = true },\n]\n\n[[schema]]",
        1,
    ) + "\n[[schema]]\nid = 9\nfrom = \"2026-09\"\nversions = [2]\ncolumns = [\"SETTLEMENTDATE\", \"DUID\", \"SCADAVALUE\", \"LASTCHANGED\", \"QUALITY\"]\n";
    user_table(&cli, "DISPATCH_UNIT_SCADA", &ours);
    let (code, out, err) = cli.code(&["import", v2, "--strict"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(
        cli.ok(&["schema", "history", "DISPATCH_UNIT_SCADA"])
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

/// Downloads are listed, checked against their record, and never removed by a cache command.
#[test]
fn the_archive_lists_and_verifies_every_download() {
    let cli = Cli::new();
    assert!(cli.ok(&["list", "--archive"]).contains("0 file(s)"));
    assert!(cli.ok(&["--json", "list", "--archive"]).is_empty());
    assert!(
        cli.ok(&["verify", "--archive"])
            .contains("0 file(s) checked")
    );

    // One archived download, at the path its URL gives it, with its record
    // (here as an older phasor wrote it, with more than this one keeps).
    let dir = cli.home.path().join("archive/h/DATA");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a.zip");
    std::fs::write(&file, b"zip bytes").unwrap();
    std::fs::write(
        dir.join("a.zip.json"),
        format!(
            r#"{{"url":"https://h/DATA/a.zip","size":9,"sha256":"{}","fetched_at":"2026-10-03T10:00:05Z","last_modified":"Tue, 13 Feb 2024 08:27:00 GMT","request":{{"by":"fetch nem REGION 2024-01..2024-01"}}}}"#,
            phasor_acquire::archive::sha256_hex(b"zip bytes")
        ),
    )
    .unwrap();
    // A partial download beside it is not an archived file.
    std::fs::write(dir.join("b.zip.part"), b"half").unwrap();
    let out = cli.ok(&["list", "--archive"]);
    assert!(out.contains("1 file(s)"), "{out}");
    assert!(
        out.contains("https://h/DATA/a.zip")
            && out.contains("2026-10-03T10:00:05Z")
            && out.contains("Tue, 13 Feb 2024")
            && out.contains("2024-02-13T08:27:00Z"),
        "the record's HTTP date as written, and the instant derived from it: {out}"
    );
    let out = cli.ok(&["--json", "list", "--archive"]);
    assert_eq!(out.lines().count(), 1, "{out}");
    assert!(out.contains("\"record\""), "{out}");
    assert!(
        cli.ok(&["verify", "--archive"])
            .contains("1 file(s) checked, 0 do not match")
    );
    // A file that changed on disk is found out.
    std::fs::write(&file, b"zip bytez").unwrap();
    let (code, out, err) = cli.code(&["verify", "--archive"]);
    assert_eq!(code, 1, "{out}{err}");
    assert!(out.contains("1 file(s) checked, 1 do not match"), "{out}");
    assert!(
        err.contains("cache.sha256_mismatch") && err.contains("a.zip"),
        "{err}"
    );
    let (code, out, _) = cli.code(&["--json", "verify", "--archive"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("\"code\":\"cache.sha256_mismatch\""), "{out}");
}

#[test]
fn cache_wem_fetch_and_config() {
    let cli = Cli::new();
    // Offline: a fetch that needs the network fails with the offline code and exit 4.
    let (code, out, err) = cli.code(&[
        "--offline",
        "fetch",
        "REGION",
        "--from",
        "2024-01",
        "--to",
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
        "--market",
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
            "REGION",
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
    // A served manifest lists its files, as a table and as JSON lines.
    let yaml = "- url: \"https://data.wa.aemo.com.au/datafiles/facility-scada/facility-scada-2023-10.csv\"\n  year: 2023\n  month: 10\n  generatedAt: \"Thu Nov 02 2023 11:35:00Z\"\n";
    let base = serve_text(yaml.to_owned());
    std::fs::write(
        cli.home.path().join("config.toml"),
        format!("request_interval_ms = 0\n[sources.wem]\nbase_url = \"{base}\"\n"),
    )
    .unwrap();
    let out = cli.ok(&[
        "wem",
        "manifest",
        "facility-scada",
        "--from",
        "2023-10",
        "--to",
        "2023-10",
    ]);
    assert!(
        out.contains("facility-scada-2023-10.csv") && out.contains("2023-11-02T11:35:00Z"),
        "{out}"
    );
    assert!(out.contains("1 file(s)"), "{out}");
    let out = cli.ok(&["--json", "wem", "manifest", "facility-scada"]);
    assert!(out.contains("\"period\":\"2023-10\""), "{out}");
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

/// Writes a user's table at `{home}/schemas/nem/{TABLE}/table.toml`; its path.
fn user_table(cli: &Cli, name: &str, text: &str) -> PathBuf {
    let dir = cli.home.path().join("schemas/nem").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("table.toml");
    std::fs::write(&path, text).unwrap();
    path
}

/// A user's table for (PHASOR_TEST, FOLLOWS), as a person writes one.
const FOLLOWS: &str = r#"schema_format = 5
name = "PHASOR_TEST_FOLLOWS"
report_pairs = [["PHASOR_TEST", "FOLLOWS"]]
load_mode = "append"
time_column = "SETTLEMENTDATE"

[[column]]
id = 1
name = "SETTLEMENTDATE"
type = "timestamp"

[[column]]
id = 2
name = "UNIT"
type = "string"

[[column]]
id = 3
name = "VALUE"
type = "double"

[[schema]]
id = 0
versions = [1]
columns = ["SETTLEMENTDATE", "UNIT", "VALUE"]
"#;

/// A user's table for (PHASOR_TEST, FIXED), in the form phasor writes.
const FIXED: &str = r#"schema_format = 5
name = "PHASOR_TEST_FIXED"
report_pairs = [["PHASOR_TEST", "FIXED"]]
load_mode = "append"
time_column = "SETTLEMENTDATE"

column = [
    { id = 1, name = "SETTLEMENTDATE", type = "timestamp", nullable = true },
    { id = 2, name = "UNIT", type = "string", nullable = true },
    { id = 3, name = "VALUE", type = "double", nullable = true },
]

[[schema]]
id = 0
from = "2009-07"
versions = [1]
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

/// A local HTTP server answering every request with `text` until the test ends; its base URL.
fn serve_text(text: String) -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for sock in listener.incoming().flatten() {
            let mut sock = sock;
            let mut buf = vec![0u8; 4096];
            let _ = sock.read(&mut buf);
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/yaml\r\nConnection: close\r\n\r\n",
                text.len()
            );
            let _ = sock.write_all(head.as_bytes());
            let _ = sock.write_all(text.as_bytes());
            let _ = sock.shutdown(std::net::Shutdown::Write);
        }
    });
    format!("http://127.0.0.1:{port}")
}

struct MonthServer {
    base: String,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    requests: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl MonthServer {
    fn start(zip: Vec<u8>) -> Self {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (s2, r2) = (stop.clone(), requests.clone());
        let year = "/Data_Archive/Wholesale_Electricity/MMSDM/2024/";
        let listing = format!(
            "<html><body><H1>nemweb.com.au - {year}</H1><hr><pre><A HREF=\"/x/\">[To Parent Directory]</A><br>   Tuesday, February 13, 2024 11:49 AM {} <A HREF=\"{year}MMSDM_2024_01.zip\">MMSDM_2024_01.zip</A><br></pre><hr></body></html>",
            zip.len()
        );
        std::thread::spawn(move || {
            while !s2.load(std::sync::atomic::Ordering::Relaxed) {
                let Ok((mut sock, _)) = listener.accept() else {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                };
                sock.set_nonblocking(false).unwrap();
                let mut buf = vec![0u8; 8192];
                let n = sock.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).into_owned();
                let path = req.split_whitespace().nth(1).unwrap_or("").to_owned();
                let range = req.lines().find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("range: bytes=")
                        .map(str::to_owned)
                });
                r2.lock()
                    .unwrap()
                    .push(format!("{path} {}", range.clone().unwrap_or_default()));
                let (status, headers, body): (&str, String, Vec<u8>) = if path == year {
                    (
                        "200 OK",
                        "Content-Type: text/html".into(),
                        listing.clone().into_bytes(),
                    )
                } else if path == format!("{year}MMSDM_2024_01.zip") {
                    match range.as_deref() {
                        Some(r) => {
                            let total = zip.len();
                            let (a, b) = match r.split_once('-') {
                                Some(("", n)) => (total.saturating_sub(n.parse().unwrap()), total),
                                Some((a, "")) => (a.parse().unwrap(), total),
                                Some((a, b)) => (
                                    a.parse().unwrap(),
                                    (b.parse::<usize>().unwrap() + 1).min(total),
                                ),
                                None => (0, total),
                            };
                            (
                                "206 Partial Content",
                                format!("Content-Range: bytes {a}-{}/{total}", b - 1),
                                zip[a..b].to_vec(),
                            )
                        }
                        None => ("200 OK", String::new(), zip.clone()),
                    }
                } else {
                    ("404 Not Found", String::new(), b"<html>404</html>".to_vec())
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{headers}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(head.as_bytes());
                let _ = sock.write_all(&body);
                let _ = sock.shutdown(std::net::Shutdown::Write);
                let mut rest = [0u8; 256];
                while matches!(sock.read(&mut rest), Ok(n) if n > 0) {}
            }
        });
        Self {
            base: format!("http://127.0.0.1:{port}"),
            stop,
            requests,
        }
    }
}

impl Drop for MonthServer {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// `get` over a whole-month zip: planned, picked by range, ingested; dry runs and refusals.
#[test]
fn fetch_reads_an_interval_of_a_table_from_a_whole_month_zip_into_the_lake() {
    use std::io::Write as _;
    let cli = Cli::new();
    let tmp = cli.home.path();
    let inner = |name: &str, fixture: &str| {
        let p = tmp.join(name);
        make_zip(
            &p,
            &[(
                &name.replace(".zip", ".CSV"),
                &std::fs::read(fixtures().join(fixture)).unwrap(),
            )],
        );
        std::fs::read(&p).unwrap()
    };
    let trading = inner(
        "PUBLIC_DVD_TRADINGPRICE_202401010000.zip",
        "TRADINGPRICE_v2_head.csv",
    );
    let dispatch = inner(
        "PUBLIC_DVD_DISPATCHPRICE_202401010000.zip",
        "DISPATCHPRICE_v4_head.csv",
    );
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    w.start_file("PUBLIC_DVD_TRADINGPRICE_202401010000.zip", stored)
        .unwrap();
    w.write_all(&trading).unwrap();
    w.start_file("PUBLIC_DVD_DISPATCHPRICE_202401010000.zip", stored)
        .unwrap();
    w.write_all(&dispatch).unwrap();
    w.start_file("README.txt", stored).unwrap();
    w.write_all(&vec![b'.'; 2 * (trading.len() + dispatch.len())])
        .unwrap();
    let whole = w.finish().unwrap().into_inner();
    let server = MonthServer::start(whole);
    std::fs::write(
        tmp.join("config.toml"),
        format!(
            "request_interval_ms = 0\n[sources.nem]\nbase_url = \"{}\"\n",
            server.base
        ),
    )
    .unwrap();
    let lake = tmp.join("lake/nem");
    // Offline: nothing can be fetched.
    let (code, _, _) = cli.code(&[
        "--offline",
        "fetch",
        "TRADINGPRICE",
        "--from",
        "2024-01",
        "--to",
        "2024-01",
    ]);
    assert_ne!(code, 0);
    // Refusals before any request.
    let (code, _, err) = cli.code(&["fetch", "NOPE", "--from", "2024-01", "--to", "2024-01"]);
    assert!(code != 0 && err.contains("No table is named NOPE"), "{err}");
    let (code, _, err) = cli.code(&[
        "fetch",
        "TRADINGPRICE",
        "--from",
        "2024-02",
        "--to",
        "2024-01",
    ]);
    assert!(code != 0 && err.contains("is not after"), "{err}");
    let (code, _, err) = cli.code(&[
        "fetch",
        "TRADINGPRICE",
        "--market",
        "mars",
        "--from",
        "2024-01",
        "--to",
        "2024-01",
    ]);
    assert!(code != 0 && err.contains("No market named mars"), "{err}");
    // A dry run: the plan and the listing, nothing fetched or read.
    let text = cli.ok(&[
        "fetch",
        "TRADINGPRICE",
        "--from",
        "2024-01",
        "--to",
        "2024-01",
        "--dry-run",
    ]);
    assert!(text.contains("nem-mmsdm: 1 file(s) over 2024-01"), "{text}");
    assert!(!lake.join("tradingprice").exists());
    // Download only: into the archive, nothing read into the lake.
    cli.ok(&[
        "fetch",
        "TRADINGPRICE",
        "--from",
        "2024-01",
        "--to",
        "2024-01",
        "--no-ingest",
    ]);
    assert!(!lake.join("tradingprice").exists());
    assert!(!cli.ok(&["list", "--ingests"]).contains("TRADINGPRICE"));
    // The run: one table out of the zip by range, read into the lake.
    let text = cli.ok(&[
        "fetch",
        "TRADINGPRICE",
        "--from",
        "2024-01",
        "--to",
        "2024-01",
    ]);
    assert!(text.contains("1 ingested"), "{text}");
    assert!(lake.join("tradingprice").is_dir(), "{text}");
    assert!(
        lake.join("tradingprice/metadata/version-hint.text")
            .is_file(),
        "fetch leaves the table's Iceberg metadata current"
    );
    let requests = server.requests.lock().unwrap().clone();
    assert!(
        requests
            .iter()
            .any(|r| r
                .starts_with("/Data_Archive/Wholesale_Electricity/MMSDM/2024/MMSDM_2024_01.zip -")),
        "the tail by range: {requests:?}"
    );
    assert!(
        !requests
            .iter()
            .any(|r| r == "/Data_Archive/Wholesale_Electricity/MMSDM/2024/MMSDM_2024_01.zip "),
        "never whole: {requests:?}"
    );
    // JSON output names the run; the lake holds the one table.
    let json = cli.ok(&[
        "--json",
        "fetch",
        "TRADINGPRICE",
        "--from",
        "2024-01-02",
        "--to",
        "2024-01-03",
    ]);
    let last = json.lines().last().unwrap();
    let v: serde_json::Value = serde_json::from_str(last).unwrap();
    assert_eq!(v["type"], "fetch", "{last}");
    let ls = cli.ok(&["list", "--ingests"]);
    assert!(
        ls.contains("TRADINGPRICE") && !ls.contains("DISPATCHPRICE"),
        "{ls}"
    );
    // A side forced, an excluded table, and a JSON dry run.
    let text = cli.ok(&[
        "fetch",
        "TRADINGPRICE,DISPATCHPRICE",
        "--exclude",
        "DISPATCHPRICE",
        "--source",
        "monthly",
        "--from",
        "2024-01",
        "--to",
        "2024-01",
    ]);
    assert!(
        text.contains("plan: every month from the monthly archive, as asked"),
        "{text}"
    );
    assert!(!lake.join("dispatchprice").exists());
    let (_, json, err) = cli.code(&[
        "--json",
        "fetch",
        "TRADINGPRICE",
        "--source",
        "daily",
        "--from",
        "2024-01",
        "--to",
        "2024-01",
        "--dry-run",
    ]);
    assert!(
        json.contains("\"type\":\"fetch\"") || err.contains("report folder"),
        "{json}{err}"
    );
    // Downloads are capped: AEMO's server blocks a client that asks too fast.
    let (code, _, err) = cli.code(&[
        "fetch",
        "TRADINGPRICE",
        "--from",
        "2024-01",
        "--to",
        "2024-01",
        "--downloads",
        "9",
    ]);
    assert!(code != 0 && err.contains("9"), "{err}");
}

/// Each command's refusals: what is missing or wrong is said, with a failing code.
#[test]
fn every_command_says_what_it_needs() {
    let cli = Cli::new();
    let refused = |args: &[&str], says: &str| {
        let (code, out, err) = cli.code(args);
        assert_eq!(code, 1, "{args:?}: {out}{err}");
        assert!(err.contains(says), "{args:?}: {err}");
    };
    refused(
        &["list", "NOPE"],
        "No table is named NOPE; `phasor list` names them.",
    );
    refused(&["list", "NOPE", "--ingests"], "No table is named NOPE.");
    refused(
        &["ingest", "--source", "nem-mmsdm"],
        "--source goes with a window",
    );
    refused(
        &["ingest", "DISPATCHPRICE"],
        "Name a window with the tables",
    );
    refused(
        &[
            "ingest",
            "DISPATCHPRICE",
            "--from",
            "2024-01",
            "--to",
            "2024-01",
            "--source",
            "nem-nope",
        ],
        "The nem kit has no source nem-nope; it has nem-reports, nem-mmsdm.",
    );
    refused(&["fetch", "DISPATCHPRICE"], "fetch needs a window");
    refused(
        &["fetch", "--from", "2024-01", "--to", "2024-01"],
        "Name a table, or --market",
    );
    refused(
        &[
            "ingest",
            "DISPATCHPRICE,DWGM_INT029A_SYSTEM_NOTICES",
            "--from",
            "2024-01",
            "--to",
            "2024-01",
        ],
        "DWGM_INT029A_SYSTEM_NOTICES is of the gas market, the others of nem; one market a command.",
    );
    refused(
        &["schema", "plan", "", "PRICE", "1", "--columns", "A"],
        "A report type and subtype are names",
    );
    // A table of one's own that no source of its kit publishes names none to read.
    let gas = cli.home.path().join("schemas/gas/PHASOR_TEST_GAS");
    std::fs::create_dir_all(&gas).unwrap();
    std::fs::write(
        gas.join("table.toml"),
        FOLLOWS.replace("PHASOR_TEST_FOLLOWS", "PHASOR_TEST_GAS"),
    )
    .unwrap();
    refused(
        &[
            "ingest",
            "PHASOR_TEST_GAS",
            "--from",
            "2024-01",
            "--to",
            "2024-01",
        ],
        "No source of the gas kit publishes PHASOR_TEST_GAS; name one with --source.",
    );
    assert!(cli.ok(&["list", "--runs"]).contains("no runs yet"));
    let none = cli.ok(&["list", "DISPATCHPRICE", "--coverage"]);
    assert!(none.contains("0 partition(s), no gap"), "{none}");
    // A daily source reads its archive by day.
    let out = cli.ok(&[
        "ingest",
        "DISPATCHPRICE",
        "--day",
        "2024-01-15",
        "--source",
        "nem-reports",
        "--dry-run",
    ]);
    assert!(out.contains("0 listed"), "{out}");
    // One market's tables, the others passed over; debug logging on.
    let gas = cli.ok(&["-vv", "list", "--market", "gas"]);
    assert!(gas.contains("DWGM_INT029A_SYSTEM_NOTICES") && !gas.contains("DISPATCHPRICE"));
    // A table's columns: name, type, role, field id, words; nullability is a reader's.
    let columns = cli.ok(&["list", "DISPATCHPRICE"]);
    let header = columns.lines().find(|l| l.starts_with("column")).unwrap();
    assert_eq!(
        header.split_whitespace().collect::<Vec<_>>(),
        ["column", "type", "role", "field", "description"]
    );
}

/// What `inspect` cannot read is an error, or for one entry of a zip, said and passed over.
#[test]
fn inspect_says_what_it_cannot_read() {
    let cli = Cli::new();
    let tmp = cli.home.path();
    let (code, _, err) = cli.code(&["inspect", tmp.join("gone.csv").to_str().unwrap()]);
    assert_eq!(code, 1, "{err}");
    let headless = tmp.join("headless.csv");
    std::fs::write(&headless, "D,DISPATCH,PRICE,4,1\r\n").unwrap();
    let (code, _, err) = cli.code(&["inspect", headless.to_str().unwrap()]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("csv.data_before_header"), "{err}");
    // An entry in a compression no reader here knows (LZMA, method 14).
    let price = std::fs::read(fixtures().join("DISPATCHPRICE_v4_head.csv")).unwrap();
    let mixed = tmp.join("mixed.zip");
    make_zip(&mixed, &[("A.CSV", &price), ("B.CSV", &price)]);
    let mut bytes = std::fs::read(&mixed).unwrap();
    for (signature, at) in [(b"PK\x03\x04", 8), (b"PK\x01\x02", 10)] {
        let second = bytes
            .windows(4)
            .enumerate()
            .filter(|(_, w)| w == signature)
            .nth(1)
            .map(|(i, _)| i)
            .unwrap();
        bytes[second + at..second + at + 2].copy_from_slice(&14u16.to_le_bytes());
    }
    std::fs::write(&mixed, bytes).unwrap();
    let (code, out, err) = cli.code(&["inspect", mixed.to_str().unwrap()]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(err.contains("zip.bad_entry"), "{err}");
    assert!(out.contains("A.CSV") && !out.contains("B.CSV"), "{out}");
}

/// A home that cannot be made, and a config file a kit refuses, stop the command before it starts.
#[test]
fn a_bad_home_or_config_is_explained() {
    let cli = Cli::new();
    let file = cli.home.path().join("a-file");
    std::fs::write(&file, "").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_phasor"))
        .arg("--home")
        .arg(file.join("home"))
        .args(["list", "--runs"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("could not be created"));
    let config = cli.home.path().join("config.toml");
    for (text, says) in [
        (
            "[sources.nem]\nbase_urll = \"http://a\"\n",
            "(in the config file)",
        ),
        (
            "[sources.nem]\nbase_url = \"not a url\"\n",
            "listing.parse_failed",
        ),
    ] {
        std::fs::write(&config, text).unwrap();
        let (code, _, err) = cli.code(&["list", "--runs"]);
        assert_eq!(code, 1, "{err}");
        assert!(err.contains(says), "{text}: {err}");
    }
}

/// `list --coverage` gives the months the lake holds of a table and the months between with none.
#[test]
fn coverage_shows_the_months_held_and_the_gaps() {
    let cli = Cli::new();
    let tmp = cli.home.path();
    let january = std::fs::read_to_string(fixtures().join("DISPATCHPRICE_v4_head.csv")).unwrap();
    let march = tmp.join("DISPATCHPRICE_march.csv");
    std::fs::write(&march, january.replace("2024/01/", "2024/03/")).unwrap();
    // Two entries of one zip feed one file of the lake.
    let both = tmp.join("PUBLIC_DISPATCHIS_TWO.zip");
    make_zip(
        &both,
        &[("A.CSV", january.as_bytes()), ("B.CSV", january.as_bytes())],
    );
    cli.ok(&["import", both.to_str().unwrap(), march.to_str().unwrap()]);
    let out = cli.ok(&["list", "DISPATCHPRICE", "--coverage"]);
    assert!(out.contains("2 partition(s); gaps: 2024-02"), "{out}");
    let json = cli.ok(&["--json", "list", "DISPATCHPRICE", "--coverage"]);
    assert!(
        json.lines()
            .last()
            .unwrap()
            .contains("\"gaps\":[\"2024-02\"]"),
        "{json}"
    );
    assert_eq!(json.lines().count(), 3, "{json}");
    let files = cli.ok(&["list", "DISPATCHPRICE", "--files"]);
    assert!(
        files.contains("PUBLIC_DISPATCHIS_TWO.zip (2 entries)"),
        "{files}"
    );
    let files = cli.ok(&["--json", "list", "--files"]);
    assert_eq!(files.lines().count(), 2, "{files}");
}

/// A manifest broken under the binary is reported as the manifest's error; the import still writes.
#[test]
fn a_broken_manifest_is_reported() {
    let cli = Cli::new();
    cli.ok(&["list", "--runs"]);
    let db = rusqlite::Connection::open(cli.home.path().join("manifest.sqlite")).unwrap();
    db.execute_batch("DROP TABLE runs").unwrap();
    drop(db);
    let (code, _, err) = cli.code(&["list", "--runs"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("runs"), "{err}");
    // No run to record the import under: nothing to rebuild the Iceberg tables from.
    let scada = fixtures().join("DISPATCH_UNIT_SCADA_v1_head.csv");
    let (_, out, err) = cli.code(&["import", scada.to_str().unwrap()]);
    assert!(err.contains("runs") || out.contains("runs"), "{out}{err}");
}

/// `fetch --documents` keeps a version of each document the config names, again only when it changed.
#[test]
fn documents_are_fetched_from_where_the_config_says() {
    let cli = Cli::new();
    let base = serve_text("a registration list".into());
    std::fs::write(
        cli.home.path().join("config.toml"),
        format!("[sources.nem]\ndocuments = [\"{base}/list.xlsx\"]\n"),
    )
    .unwrap();
    let out = cli.ok(&["fetch", "--documents"]);
    assert!(out.starts_with("new version: "), "{out}");
    let json = cli.ok(&["--json", "fetch", "--documents"]);
    assert!(
        json.contains("\"type\":\"document\"") && json.contains("\"new\":false"),
        "{json}"
    );
}

/// A fetch whose listing is in flight, sent `signals` SIGINTs a moment apart:
/// its exit code, stdout, stderr and how long it took after the first.
#[cfg(unix)]
fn interrupted_fetch(signals: usize) -> (Option<i32>, String, String, std::time::Duration) {
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicBool, Ordering};
    // A nemweb that answers, the first time slowly, that it has no such month.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let asked = std::sync::Arc::new(AtomicBool::new(false));
    let seen = asked.clone();
    std::thread::spawn(move || {
        for sock in listener.incoming().flatten() {
            let mut sock = sock;
            let _ = sock.read(&mut [0u8; 4096]);
            if !seen.swap(true, Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(1500));
            }
            let _ = sock.write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });
    let cli = Cli::new();
    std::fs::write(
        cli.home.path().join("config.toml"),
        format!("[sources.nem]\nbase_url = \"http://127.0.0.1:{port}\"\n"),
    )
    .unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_phasor"))
        .arg("--home")
        .arg(cli.home.path())
        .args([
            "--no-progress",
            "fetch",
            "DISPATCHPRICE",
            "--from",
            "2024-01",
            "--to",
            "2024-01",
        ])
        .env_remove("PHASOR_HOME")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !asked.load(Ordering::SeqCst) {
        assert!(
            std::time::Instant::now() < deadline,
            "the listing was never asked for"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    // The listing is in flight: the handler has long been in place.
    std::thread::sleep(std::time::Duration::from_millis(200));
    let first = std::time::Instant::now();
    for _ in 0..signals {
        let kill = Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .unwrap();
        assert!(kill.success());
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let out = child.wait_with_output().unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        first.elapsed(),
    )
}

/// Ctrl-C lets the files in flight finish, says so, and exits 130; a second
/// stops at once, 130 too. A run that cannot plan fails.
#[test]
#[cfg(unix)]
fn ctrl_c_finishes_the_files_in_flight_and_twice_stops_at_once() {
    let (code, out, err, _) = interrupted_fetch(1);
    assert!(
        err.contains("interrupted; finishing the files in flight"),
        "{err}"
    );
    assert_eq!(code, Some(130), "{out}{err}");
    assert!(out.contains("interrupted:"), "{out}");
    let (code, _, err, took) = interrupted_fetch(2);
    assert!(
        err.contains("stopped; the next run takes up what this one left"),
        "{err}"
    );
    assert_eq!(code, Some(130), "{err}");
    // Gone before the slow listing answered.
    assert!(took < std::time::Duration::from_millis(1200), "{took:?}");
    // A listing phasor cannot read fails the run before it starts.
    let cli = Cli::new();
    let base = serve_text("not a listing at all".into());
    std::fs::write(
        cli.home.path().join("config.toml"),
        format!("[sources.nem]\nbase_url = \"{base}\"\n"),
    )
    .unwrap();
    let (code, _, err) = cli.code(&[
        "fetch",
        "DISPATCHPRICE",
        "--from",
        "2024-01",
        "--to",
        "2024-01",
    ]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("listing.parse_failed"), "{err}");
}
