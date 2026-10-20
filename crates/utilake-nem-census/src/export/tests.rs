use std::path::{Path, PathBuf};

use utilake_mms_registry::file_model::ctl::sha256;

use super::*;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A control file whose history line names a data-model version.
const CTL_V423: &str = "-- Title:\tPUBLIC_DVD_UNIT_201501.ctl\n-- Visibility Id : PUBLIC\n-- History :\n--      MMS Data Model Version   Template Generated On\n--      PUBLIC\t\t\t       v4.23\t\nLOAD DATA\nINFILE PUBLIC_DVD_UNIT_201501010000.csv\nAPPEND INTO TABLE UNIT\nWHEN (1:1) = 'D'\nFIELDS TERMINATED BY ','\nOPTIONALLY ENCLOSED BY '\"'\nTRAILING NULLCOLS\n(\nFILLER1 FILLER,\nSETTLEMENTDATE DATE \"YYYY/MM/DD HH24:MI:SS\",\nDUID CHAR(20),\nSCADAVALUE FLOAT EXTERNAL)\n";

/// A control file whose history line has no version.
const CTL_BLANK: &str = "-- Title:\tPUBLIC_ARCHIVE#UNIT#FILE01#202410.ctl\n-- Visibility Id : PUBLIC\n--      PUBLIC\t\t\t       v\nLOAD DATA\nINFILE PUBLIC_ARCHIVE#UNIT#FILE01#202410010000.CSV\nAPPEND INTO TABLE UNIT\nWHEN (1:1) = 'D'\nFIELDS TERMINATED BY ','\nOPTIONALLY ENCLOSED BY '\"'\nTRAILING NULLCOLS\n(\nFILLER1 FILLER,\nSETTLEMENTDATE DATE \"YYYY/MM/DD HH24:MI:SS\",\nDUID CHAR(20),\nSCADAVALUE FLOAT EXTERNAL,\nLASTCHANGED DATE \"YYYY/MM/DD HH24:MI:SS\")\n";

fn unit_table(sha1: &str, sha2: &str) -> String {
    format!(
        r#"schema_format = 3
name = "UNIT"
description = "Unit MW from SCADA."
report_pairs = [["DISPATCH", "UNIT"]]
load_mode = "append"
primary_key = ["SETTLEMENTDATE", "DUID"]
time_column = "SETTLEMENTDATE"
next_column_id = 5

column = [
    {{ id = 1, name = "SETTLEMENTDATE", type = "timestamp_ns", nullable = false }},
    {{ id = 2, name = "DUID", type = "string", nullable = false }},
    {{ id = 3, name = "SCADAVALUE", type = "double", nullable = true }},
    {{ id = 4, name = "LASTCHANGED", type = "timestamp_ns", nullable = true }},
]

[temporal]
origin = "derived"
entity = ["DUID"]
valid = {{ kind = "interval_end", column = "SETTLEMENTDATE" }}
order = ["LASTCHANGED"]
cutoff = "LASTCHANGED"

[[increment]]
n = 1
from = "2015-01"
ctl = "001.ctl"
ctl_sha256 = "{sha1}"
aemo_ctl = "PUBLIC_DVD_UNIT_201501.ctl"
aemo_versions = []
columns = ["SETTLEMENTDATE", "DUID", "SCADAVALUE"]

[[increment]]
n = 2
from = "2024-10"
ctl = "002.ctl"
ctl_sha256 = "{sha2}"
aemo_ctl = "PUBLIC_ARCHIVE#UNIT#FILE01#202410.ctl"
aemo_versions = [1]
columns = ["SETTLEMENTDATE", "DUID", "SCADAVALUE", "LASTCHANGED"]
"#
    )
}

const REGION_TABLE: &str = r#"schema_format = 3
name = "REGION"
report_pairs = [["DISPATCH", "REGION"]]
load_mode = "snapshot"
primary_key = ["REGIONID"]
next_column_id = 3

column = [
    { id = 1, name = "REGIONID", type = "string", nullable = false },
    { id = 2, name = "DESCRIPTION", type = "string", nullable = true },
]

[temporal]
origin = "derived"
entity = ["REGIONID"]
valid = { kind = "timeless" }
order = []

[[increment]]
n = 1
from = "2015-01"
aemo_versions = []
columns = ["REGIONID", "DESCRIPTION"]
"#;

/// Every lake type, no increments, roles chosen by hand.
const TYPES_TABLE: &str = r#"schema_format = 3
name = "TYPES"
report_pairs = [["TEST", "TYPES"]]
load_mode = "snapshot"
primary_key = ["EFFECTIVEDATE"]
next_column_id = 7

column = [
    { id = 1, name = "EFFECTIVEDATE", type = "timestamp_ns", nullable = true },
    { id = 2, name = "S", type = "string", nullable = true },
    { id = 3, name = "D", type = "double", nullable = true },
    { id = 4, name = "L", type = "long", nullable = true },
    { id = 5, name = "B", type = "boolean", nullable = true },
    { id = 6, name = "DAY", type = "date", nullable = true },
]

[temporal]
origin = "hand"
entity = []
valid = { kind = "from", column = "EFFECTIVEDATE" }
order = []
"#;

const UNIT_SHAPES: &str = r#"[[shape]]
id = "1111111111111111"
report_type = "DISPATCH"
report_subtype = "UNIT"
version = 1
first_seen = "2015-01"
last_seen = "2020-12"
columns = [["SETTLEMENTDATE", "timestamp"], ["DUID", "varchar"], ["SCADAVALUE", "number"]]

[[shape]]
id = "1111111111111111"
report_type = "DISPATCH"
report_subtype = "UNIT"
version = 2
first_seen = "2021-01"
last_seen = "2024-09"
columns = [["SETTLEMENTDATE", "timestamp"], ["DUID", "varchar"], ["SCADAVALUE", "number"]]

[[shape]]
id = "2222222222222222"
report_type = "DISPATCH"
report_subtype = "UNIT"
version = 1
first_seen = "2024-10"
last_seen = "2024-11"
columns = [["SETTLEMENTDATE", "timestamp"], ["DUID", "varchar"], ["SCADAVALUE", "number"], ["LASTCHANGED", "timestamp"]]

[[shape]]
id = "3333333333333333"
report_type = "DISPATCH"
report_subtype = "UNIT"
version = 3
first_seen = "2024-11"
last_seen = "2024-11"
columns = [["SETTLEMENTDATE", "timestamp"], ["DUID", "varchar"], ["QUALITY", "unknown"]]
"#;

const REGION_SHAPES: &str = r#"[[shape]]
id = "4444444444444444"
report_type = "DISPATCH"
report_subtype = "REGION"
version = 1
first_seen = "2015-01"
last_seen = "2024-11"
columns = [["REGIONID", "varchar"], ["DESCRIPTION", "varchar"]]
"#;

const TYPES_SHAPES: &str = r#"[[shape]]
id = "5555555555555555"
report_type = "TEST"
report_subtype = "TYPES"
version = 1
first_seen = "2024-11"
last_seen = "2024-11"
columns = [["EFFECTIVEDATE", "unknown"], ["S", "unknown"], ["D", "unknown"], ["L", "unknown"], ["B", "unknown"], ["DAY", "unknown"], ["X", "unknown"]]

[[shape]]
id = "6666666666666666"
report_type = "TEST"
report_subtype = "TYPES"
version = 1
first_seen = "2024-11"
last_seen = "2024-11"
columns = [["EFFECTIVEDATE", "date"], ["S", "integer"], ["D", "number"], ["L", "number"], ["DAY", "date"]]
"#;

const OFFER_SHAPES: &str = r#"[[shape]]
id = "7777777777777777"
report_type = "OFFER"
report_subtype = "X"
version = 1
first_seen = "2024-10"
last_seen = "2024-11"
columns = [["OFFERDATE", "timestamp"], ["PRICE", "number"], ["NOTE", "unknown"]]
"#;

/// A census before rows were counted: one file of one block, one of two.
const CENSUS_2015_01: &str = r#""PUBLIC_DVD_UNIT_201501010000.zip" = "1111111111111111"
"PUBLIC_DVD_REGION_201501010000.zip" = ["4444444444444444", "4444444444444444"]
"#;

/// Two blocks of one table in one file, a legacy entry beside a counted one,
/// and a shape no shapes file knows.
const CENSUS_2024_10: &str = r#"[["PUBLIC_ARCHIVE#UNIT#FILE01#202410010000.zip"]]
shape = "1111111111111111"
rows = 10

[["PUBLIC_ARCHIVE#UNIT#FILE01#202410010000.zip"]]
shape = "2222222222222222"
rows = 20

[["PUBLIC_ARCHIVE#UNIT#FILE02#202410010000.zip"]]
shape = "2222222222222222"
rows = 5

[["PUBLIC_ARCHIVE#REGION#FILE01#202410010000.zip"]]
shape = "4444444444444444"
rows = 5

[["PUBLIC_ARCHIVE#OFFER#FILE01#202410010000.zip"]]
shape = "7777777777777777"
rows = 7

[["PUBLIC_ARCHIVE#LOST#FILE01#202410010000.zip"]]
shape = "9999999999999999"
rows = 1
"#;

const CENSUS_2024_11: &str = r#"[["PUBLIC_ARCHIVE#UNIT#FILE01#202411010000.zip"]]
shape = "2222222222222222"
rows = 30

[["PUBLIC_ARCHIVE#UNIT#FILE02#202411010000.zip"]]
shape = "3333333333333333"
rows = 3

[["PUBLIC_ARCHIVE#TYPES#FILE01#202411010000.zip"]]
shape = "5555555555555555"
rows = 1

[["PUBLIC_ARCHIVE#TYPES#FILE02#202411010000.zip"]]
shape = "6666666666666666"
rows = 1

[["PUBLIC_ARCHIVE#OFFER#FILE01#202411010000.zip"]]
shape = "7777777777777777"
rows = 8
"#;

const DATAMODEL: &str = r#"{
  "UNIT": {
    "page": "MMS_1.htm", "package": "DISPATCH", "description": "Unit MW.", "visibility": "Public",
    "primary_key": ["DUID", "SETTLEMENTDATE"], "indexes": ["LASTCHANGED"],
    "columns": [
      {"name": "SETTLEMENTDATE", "type": "Date", "mandatory": true, "comment": "Market date and time starting at 04:05"},
      {"name": "DUID", "type": "varchar2(20)", "mandatory": true, "comment": "Dispatchable unit identifier"},
      {"name": "SCADAVALUE", "type": "Number(15,5)", "comment": ""}
    ]
  },
  "REGION": {
    "page": "MMS_2.htm", "package": "MARKET_CONFIG", "description": "The regions.",
    "primary_key": ["REGIONID"], "indexes": [],
    "columns": [{"name": "REGIONID", "type": "varchar2(10)", "mandatory": true, "comment": ""}]
  }
}
"#;

fn put(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A schemas directory with three declared tables, one undeclared shape
/// file and three census months, under `dir/schemas`.
fn fixture(dir: &Path) -> Inputs {
    let schemas = dir.join("schemas");
    let nem = schemas.join("nem");
    std::fs::create_dir_all(&nem).unwrap();
    std::fs::copy(
        root().join("schemas/nem/market.toml"),
        nem.join("market.toml"),
    )
    .unwrap();
    put(&nem.join("UNIT/ctl/001.ctl"), CTL_V423);
    put(&nem.join("UNIT/ctl/002.ctl"), CTL_BLANK);
    put(
        &nem.join("UNIT/table.toml"),
        &unit_table(&sha256(CTL_V423.as_bytes()), &sha256(CTL_BLANK.as_bytes())),
    );
    put(&nem.join("REGION/table.toml"), REGION_TABLE);
    put(&nem.join("TYPES/table.toml"), TYPES_TABLE);
    put(&nem.join("shapes/UNIT.toml"), UNIT_SHAPES);
    put(&nem.join("shapes/REGION.toml"), REGION_SHAPES);
    put(&nem.join("shapes/TYPES.toml"), TYPES_SHAPES);
    put(&nem.join("shapes/OFFER_X.toml"), OFFER_SHAPES);
    put(&nem.join("census/2015-01.toml"), CENSUS_2015_01);
    put(&nem.join("census/2024-10.toml"), CENSUS_2024_10);
    put(&nem.join("census/2024-11.toml"), CENSUS_2024_11);
    put(&nem.join("census/notes.txt"), "not a month");
    put(&nem.join("census/reports/ARCHIVE/x.toml"), "");
    let datamodel = dir.join("datamodel.json");
    put(&datamodel, DATAMODEL);
    Inputs {
        schemas,
        datamodel: Some(datamodel),
        base_url: "https://example.test/schemas/".to_owned(),
    }
}

fn rendered(dir: &Path) -> Rendered {
    Export::load(&fixture(dir)).unwrap().render().unwrap()
}

#[test]
fn renders_one_document_per_shape_and_an_index_per_table() {
    let dir = tempfile::tempdir().unwrap();
    let r = rendered(dir.path());
    let paths: Vec<&str> = r.files.keys().map(String::as_str).collect();
    assert_eq!(
        paths,
        [
            "LICENCE.md",
            "manifest.json",
            "nem/OFFER_X/7777777777777777.schema.json",
            "nem/OFFER_X/index.json",
            "nem/REGION/4444444444444444.schema.json",
            "nem/REGION/index.json",
            "nem/TYPES/5555555555555555.schema.json",
            "nem/TYPES/6666666666666666.schema.json",
            "nem/TYPES/index.json",
            "nem/UNIT/1111111111111111.schema.json",
            "nem/UNIT/2222222222222222.schema.json",
            "nem/UNIT/3333333333333333.schema.json",
            "nem/UNIT/index.json",
        ]
    );
    assert_eq!(r.problems.len(), 1, "{:?}", r.problems);
    assert_eq!(r.problems[0].code, "schema.export_unknown_shape");
    assert!(
        r.problems[0].message.contains("9999999999999999"),
        "{}",
        r.problems[0].message
    );
    insta::assert_snapshot!(
        "unit_1111",
        r.files["nem/UNIT/1111111111111111.schema.json"]
    );
    insta::assert_snapshot!(
        "unit_3333",
        r.files["nem/UNIT/3333333333333333.schema.json"]
    );
    insta::assert_snapshot!("unit_index", r.files["nem/UNIT/index.json"]);
    insta::assert_snapshot!(
        "region_4444",
        r.files["nem/REGION/4444444444444444.schema.json"]
    );
    insta::assert_snapshot!(
        "types_5555",
        r.files["nem/TYPES/5555555555555555.schema.json"]
    );
    insta::assert_snapshot!(
        "types_6666",
        r.files["nem/TYPES/6666666666666666.schema.json"]
    );
    insta::assert_snapshot!(
        "offer_7777",
        r.files["nem/OFFER_X/7777777777777777.schema.json"]
    );
    insta::assert_snapshot!("manifest", r.files["manifest.json"]);
    assert_eq!(r.files["LICENCE.md"], LICENCE);
    let provenance: serde_json::Value = serde_json::from_str(&r.provenance).unwrap();
    assert_eq!(provenance["datamodel"], "datamodel.json");
    assert_eq!(provenance["generator"], "utilake-nem-census export");
    assert!(provenance["generated_at"].as_str().unwrap().ends_with('Z'));
    assert!(
        provenance["utilake_commit"].is_null(),
        "a scratch directory is no repository"
    );
}

#[test]
fn a_second_render_is_byte_identical() {
    let dir = tempfile::tempdir().unwrap();
    let a = rendered(dir.path());
    let b = rendered(dir.path());
    assert_eq!(a.files, b.files);
}

#[test]
fn write_owns_nem_and_check_tells_what_differs() {
    let dir = tempfile::tempdir().unwrap();
    let r = rendered(dir.path());
    let out = dir.path().join("out");
    let stale = check(&out, &r).unwrap();
    assert_eq!(stale.len(), r.files.len());
    assert!(stale.iter().all(|s| s.why == "missing"), "{stale:?}");
    assert_eq!(
        stale[0].diagnostic().code,
        "schema.export_stale",
        "{}",
        stale[0].diagnostic().render()
    );

    let w = write(&out, &r).unwrap();
    assert_eq!((w.written, w.kept, w.removed), (r.files.len(), 0, 0));
    assert!(out.join("provenance.json").is_file());
    assert!(check(&out, &r).unwrap().is_empty());

    put(&out.join("nem/UNIT/index.json"), "{}");
    put(&out.join("nem/GONE/deep/old.schema.json"), "{}");
    put(&out.join("nem/UNIT/extra.json"), "{}");
    put(&out.join("unrelated.txt"), "kept: outside nem/");
    let stale = check(&out, &r).unwrap();
    let listed: Vec<(&str, &str)> = stale.iter().map(|s| (s.path.as_str(), s.why)).collect();
    assert_eq!(
        listed,
        [
            ("nem/UNIT/index.json", "differs"),
            ("nem/GONE/deep/old.schema.json", "extra"),
            ("nem/UNIT/extra.json", "extra"),
        ]
    );

    let w = write(&out, &r).unwrap();
    assert_eq!((w.written, w.kept, w.removed), (1, r.files.len() - 1, 2));
    assert!(!out.join("nem/GONE").exists(), "emptied directories go");
    assert!(
        out.join("nem/UNIT").is_dir(),
        "a directory still in use stays"
    );
    assert!(out.join("unrelated.txt").is_file());
    assert!(check(&out, &r).unwrap().is_empty());
}

#[test]
fn a_directory_that_cannot_be_read_is_a_diagnostic() {
    let dir = tempfile::tempdir().unwrap();
    let inputs = fixture(dir.path());
    let census = census_dir(&inputs.schemas);
    std::fs::remove_dir_all(&census).unwrap();
    std::fs::write(&census, "a file where a directory should be").unwrap();
    let d = Export::load(&inputs).unwrap_err();
    assert_eq!(d.code, "schema.export_io", "{}", d.render());

    let r = {
        std::fs::remove_file(&census).unwrap();
        Export::load(&inputs).unwrap().render().unwrap()
    };
    let out = dir.path().join("out");
    put(&out.join("nem"), "a file where nem/ should be");
    assert_eq!(check(&out, &r).unwrap_err().code, "schema.export_io");
    assert_eq!(write(&out, &r).unwrap_err().code, "schema.export_io");

    let out = dir.path().join("out2");
    std::fs::create_dir_all(out.join("nem/UNIT/index.json")).unwrap();
    assert_eq!(
        check(&out, &r).unwrap_err().code,
        "schema.export_io",
        "a directory where a file should be"
    );
    assert_eq!(write(&out, &r).unwrap_err().code, "schema.export_io");
}

#[test]
fn a_control_file_an_increment_names_must_exist() {
    let dir = tempfile::tempdir().unwrap();
    let inputs = fixture(dir.path());
    std::fs::remove_file(inputs.schemas.join("nem/UNIT/ctl/001.ctl")).unwrap();
    let d = Export::load(&inputs).unwrap().render().unwrap_err();
    assert_eq!(d.code, "schema.export_io", "{}", d.render());
    assert!(d.render().contains("001.ctl"), "{}", d.render());
}

#[test]
fn without_a_census_or_a_data_model_the_documents_still_render() {
    let dir = tempfile::tempdir().unwrap();
    let mut inputs = fixture(dir.path());
    std::fs::remove_dir_all(census_dir(&inputs.schemas)).unwrap();
    inputs.datamodel = None;
    let r = Export::load(&inputs).unwrap().render().unwrap();
    assert!(r.problems.is_empty());
    let manifest: serde_json::Value = serde_json::from_str(&r.files["manifest.json"]).unwrap();
    assert!(manifest["months"].is_null());
    let unit: serde_json::Value =
        serde_json::from_str(&r.files["nem/UNIT/1111111111111111.schema.json"]).unwrap();
    assert_eq!(unit["x-seen"]["months"], serde_json::json!([]));
    assert!(unit["x-table"].get("package").is_none());
}

#[test]
fn model_version_reads_the_history_line() {
    assert_eq!(model_version(CTL_V423).as_deref(), Some("v4.23"));
    assert_eq!(model_version(CTL_BLANK), None);
    assert_eq!(
        model_version("--      PUBLIC   v5\n"),
        Some("v5".to_owned())
    );
    assert_eq!(
        model_version("--      PUBLIC   version\n"),
        None,
        "not a number"
    );
    assert_eq!(
        model_version("--      public   v4.23\n"),
        None,
        "scope is upper case"
    );
    assert_eq!(
        model_version("##      PUBLIC   v4.23\n"),
        None,
        "a comment line"
    );
    assert_eq!(
        model_version("--      PUBLIC   v4.23x\n"),
        None,
        "digits and dots only"
    );
    assert_eq!(model_version(""), None);
}

/// The committed schemas export whole: every shape in every shapes file
/// gets a document, and the provenance names this repository's commit.
#[test]
fn the_committed_schemas_export_whole() {
    let schemas = root().join("schemas");
    let inputs = Inputs {
        schemas: schemas.clone(),
        datamodel: Some(root().join("testdata/bootstrap/datamodel/mms_data_model_v5.0.0.1.json")),
        base_url: DEFAULT_BASE_URL.to_owned(),
    };
    let set = SchemaSet::read_dir(&schemas).unwrap();
    let r = Export::load(&inputs).unwrap().render().unwrap();
    for (table, entries) in &set.shapes {
        for e in entries {
            let path = format!("nem/{table}/{}.schema.json", e.id);
            assert!(r.files.contains_key(&path), "{path} missing");
        }
        assert!(r.files.contains_key(&format!("nem/{table}/index.json")));
    }
    let manifest: serde_json::Value = serde_json::from_str(&r.files["manifest.json"]).unwrap();
    assert_eq!(manifest["tables"], set.shapes.len());
    let provenance: serde_json::Value = serde_json::from_str(&r.provenance).unwrap();
    assert_eq!(
        provenance["utilake_commit"].as_str().map(str::len),
        Some(40)
    );
    assert_eq!(provenance["schemas_tree"].as_str().map(str::len), Some(40));
    assert!(provenance["utilake_dirty"].is_boolean());
    eprintln!(
        "{} documents, {} problem(s) in the committed schemas",
        r.files.len(),
        r.problems.len()
    );
}
