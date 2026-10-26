//! `docs/errors.md` lists every diagnostic code. The codes are found in the
//! source as string literals of the form `"category.snake_name"` passed to
//! `Diagnostic::error`/`warning`/`new` or named as a `code` field, so a new
//! failure mode shows up here automatically and the doc cannot drift.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;

const CATEGORIES: [&str; 14] = [
    "config", "net", "listing", "cache", "zip", "csv", "json", "parse", "schema", "store",
    "manifest", "verify", "query", "internal",
];

/// Codes utilake reports that a utilake-schemas crate raises on its behalf,
/// with where: the decoders' and the zip walker's own diagnostics. Listed
/// beside utilake's own, since they reach its users the same way.
const INHERITED: [(&str, &str); 13] = [
    ("csv.block_mismatch", MMS_DECODE),
    ("csv.data_before_header", MMS_DECODE),
    ("csv.field_count_mismatch", MMS_DECODE),
    ("csv.header_malformed", MMS_DECODE),
    ("csv.header_missing", PLAIN_DECODE),
    ("csv.line_too_long", MMS_DECODE),
    ("csv.row_too_short", MMS_DECODE),
    ("csv.too_many_bad_rows", MMS_DECODE),
    ("csv.unknown_record", MMS_DECODE),
    ("json.not_wemde", PLAIN_DECODE),
    ("json.syntax", PLAIN_DECODE),
    ("zip.no_entry", ZIP),
    ("zip.spool_failed", ZIP),
];
const MMS_DECODE: &str = "utilake-schemas/crates/utilake-mms-decode/src/diagnose.rs";
const PLAIN_DECODE: &str = "utilake-schemas/crates/utilake-plain-decode/src/diagnose.rs";
const ZIP: &str = "utilake-schemas/crates/utilake-zip/src/diagnose.rs";

/// Codes that are no longer raised. A code is a promise to whoever matches
/// on it, so a retired one stays listed and may never be given a new meaning.
const RETIRED: [(&str, &str); 33] = [
    (
        "zip.no_csv",
        "an archive with no .CSV entry; the walk takes what a decoder accepts, so it is `zip.no_entry`",
    ),
    (
        "zip.size_mismatch",
        "an entry that inflated past the size it declared; the zip reader refuses it itself since zip 9, as `zip.bad_entry`",
    ),
    (
        "listing.era_unsupported",
        "months before 2024 could not be listed; every archive month is now readable and a month the archive does not publish is `listing.month_missing`",
    ),
    (
        "schema.merge_version_conflict",
        "the schema bootstrap refused two different column lists under one version number; each list is now a variant of the version",
    ),
    (
        "schema.version_drift",
        "a block whose columns differed from its declared version; resolution is now against increments, and such a block is `schema.deviates`",
    ),
    (
        "schema.unknown_version",
        "an I-row version the table did not declare; increments decide now, and an unseen version is the warning `schema.unseen_version`",
    ),
    (
        "schema.version_aliased",
        "an undeclared version whose columns were those of a declared one; resolution is by increments and columns now",
    ),
    (
        "schema.compatible_drift",
        "a block missing declared columns; now `schema.deviates`",
    ),
    (
        "schema.unknown_columns",
        "a block with columns the table did not declare; now `schema.deviates`",
    ),
    (
        "schema.decimal_fallback",
        "decimal mode read a column without precision as a float; decimal mode is gone and every number is a double",
    ),
    (
        "schema.rule_conflict",
        "a hand-written rule routed a column the file also carried; rules are gone, fixes go in an increment's columns",
    ),
    (
        "schema.census_unresolved",
        "a census shape neither declared nor read by a rule; now `schema.census_deviates`",
    ),
    (
        "schema.breaking_change",
        "a table file changed in a way the lake does not survive; tables come from the histories utilake-schemas publishes, which hold their own changes",
    ),
    (
        "query.at_on_interval_table",
        "raised by utilake's read of the lake, which is gone: utilake writes the lake and reading it is the reader's own SQL",
    ),
    (
        "query.bad_filter_value",
        "raised by utilake's read of the lake, which is gone: utilake writes the lake and reading it is the reader's own SQL",
    ),
    (
        "query.bad_time",
        "raised by utilake's read of the lake, which is gone: utilake writes the lake and reading it is the reader's own SQL",
    ),
    (
        "query.filter_after_resolution",
        "raised by utilake's read of the lake, which is gone: utilake writes the lake and reading it is the reader's own SQL",
    ),
    (
        "query.lake_format",
        "raised by utilake's read of the lake, which is gone: utilake writes the lake and reading it is the reader's own SQL",
    ),
    (
        "query.order_nan",
        "raised by utilake's read of the lake, which is gone: utilake writes the lake and reading it is the reader's own SQL",
    ),
    (
        "query.plan_failed",
        "raised by utilake's read of the lake, which is gone: utilake writes the lake and reading it is the reader's own SQL",
    ),
    (
        "query.raw_only",
        "raised by utilake's read of the lake, which is gone: utilake writes the lake and reading it is the reader's own SQL",
    ),
    (
        "query.read_failed",
        "raised by utilake's read of the lake, which is gone: utilake writes the lake and reading it is the reader's own SQL",
    ),
    (
        "query.role_missing",
        "raised by utilake's read of the lake, which is gone: utilake writes the lake and reading it is the reader's own SQL",
    ),
    (
        "query.role_type",
        "raised by utilake's read of the lake, which is gone: utilake writes the lake and reading it is the reader's own SQL",
    ),
    (
        "query.roles_invalid",
        "raised by utilake's read of the lake, which is gone: utilake writes the lake and reading it is the reader's own SQL",
    ),
    (
        "query.source_unknown",
        "raised by utilake's read of the lake, which is gone: utilake writes the lake and reading it is the reader's own SQL",
    ),
    (
        "query.stamp_unknown",
        "raised by utilake's read of the lake, which is gone: utilake writes the lake and reading it is the reader's own SQL",
    ),
    (
        "query.unknown_column",
        "raised by utilake's read of the lake, which is gone: utilake writes the lake and reading it is the reader's own SQL",
    ),
    (
        "query.unknown_table",
        "raised by utilake's read of the lake, which is gone: utilake writes the lake and reading it is the reader's own SQL",
    ),
    (
        "query.window_empty",
        "raised by utilake's read of the lake, which is gone: utilake writes the lake and reading it is the reader's own SQL",
    ),
    (
        "query.window_ignored",
        "raised by utilake's read of the lake, which is gone: utilake writes the lake and reading it is the reader's own SQL",
    ),
    (
        "query.window_uncovered",
        "raised by utilake's read of the lake, which is gone: utilake writes the lake and reading it is the reader's own SQL",
    ),
    (
        "query.write_failed",
        "raised by utilake's read of the lake, which is gone: utilake writes the lake and reading it is the reader's own SQL",
    ),
];

/// The retired codes that live code raises again.
pub fn reused(codes: &BTreeMap<String, String>) -> Vec<&'static str> {
    RETIRED
        .iter()
        .map(|(c, _)| *c)
        .filter(|c| codes.contains_key(*c))
        .collect()
}

/// Every `"category.name"` literal in the `.rs` files under `crates/`, with
/// the first file that uses it.
pub fn collect_codes(root: &Path) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    let mut stack = vec![root.join("crates")];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        let mut entries: Vec<_> = rd.filter_map(Result::ok).map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                if p.file_name().is_some_and(|n| n != "target") {
                    stack.push(p);
                }
                continue;
            }
            if p.extension().is_none_or(|e| e != "rs")
                || p.components().any(|c| {
                    c.as_os_str() == "xtask"
                        || c.as_os_str() == "tests"
                        || c.as_os_str() == "benches"
                })
            {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&p) else {
                continue;
            };
            // Test modules invent codes; only shipped code counts.
            let text = text.split("#[cfg(test)]").next().unwrap_or("");
            let rel = p
                .strip_prefix(root)
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/");
            for code in literals(text) {
                out.entry(code).or_insert_with(|| rel.clone());
            }
        }
    }
    out
}

fn literals(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while let Some(q) = text[i..].find('"') {
        let start = i + q + 1;
        let Some(len) = text[start..].find('"') else {
            break;
        };
        let lit = &text[start..start + len];
        i = start + len + 1;
        if let Some((cat, name)) = lit.split_once('.')
            && CATEGORIES.contains(&cat)
            && !name.is_empty()
            && !matches!(
                name,
                "toml"
                    | "json"
                    | "rs"
                    | "md"
                    | "yaml"
                    | "csv"
                    | "zip"
                    | "sqlite"
                    | "html"
                    | "txt"
                    | "parquet"
            )
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b == b'_' || b.is_ascii_digit())
            && !name.contains("..")
        {
            // Skip prose like "the file.name" by requiring the literal to stand alone.
            let before = bytes[..start - 1]
                .iter()
                .rev()
                .find(|b| !b.is_ascii_whitespace())
                .copied();
            if matches!(before, Some(b'(' | b',' | b'=' | b'[' | b'{' | b'>' | b':')) {
                out.push(lit.to_owned());
            }
        }
    }
    out
}

/// Renders the catalogue.
pub fn render(codes: &BTreeMap<String, String>) -> String {
    use std::fmt::Write as _;
    let mut s = String::from(
        "# Diagnostic codes\n\nEvery error and warning utilake reports carries one of these codes. Codes are stable: never renamed, never reused. The category is the first segment. This file is generated by `just errors-doc`; `just errors-check` fails when it is out of date.\n\n",
    );
    let mut by_cat: BTreeMap<&str, Vec<(&str, &str)>> = BTreeMap::new();
    for (code, file) in codes {
        let cat = code.split_once('.').map_or("", |(c, _)| c);
        by_cat.entry(cat).or_default().push((code, file));
    }
    for cat in CATEGORIES {
        let Some(list) = by_cat.get(cat) else {
            continue;
        };
        let _ = write!(s, "## {cat}\n\n| code | raised in |\n|---|---|\n");
        for (code, file) in list {
            let _ = writeln!(s, "| `{code}` | `{file}` |");
        }
        s.push('\n');
    }
    s.push_str("## Retired\n\nNo longer raised; kept so the code is never reused.\n\n| code | was |\n|---|---|\n");
    for (code, was) in RETIRED {
        let _ = writeln!(s, "| `{code}` | {was} |");
    }
    // Exactly one trailing newline, as the end-of-file hook enforces.

    s.trim_end().to_owned() + "\n"
}

pub fn run(write: bool, check: bool) -> Result<bool> {
    let root = crate::repo_root();
    let mut codes = collect_codes(&root);
    for (code, from) in INHERITED {
        codes
            .entry(code.to_owned())
            .or_insert_with(|| from.to_owned());
    }
    let again = reused(&codes);
    if !again.is_empty() {
        eprintln!(
            "retired diagnostic code(s) raised again: {}; pick a new code",
            again.join(", ")
        );
        return Ok(false);
    }
    let text = render(&codes);
    let path = root.join("docs/errors.md");
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    if write {
        std::fs::create_dir_all(path.parent().expect("docs/"))?;
        std::fs::write(&path, &text)?;
        println!("docs/errors.md: {} codes written", codes.len());
        return Ok(true);
    }
    if check && current != text {
        eprintln!("docs/errors.md is out of date; run `just errors-doc`");
        return Ok(false);
    }
    println!("docs/errors.md: {} codes, up to date", codes.len());
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_codes() {
        let src = r#"Diagnostic::error("csv.field_count_mismatch", Category::CSV, "x"); let c = "net.offline"; code: "schema.drift", "not a code", "the file.txt", "config.toml""#;
        let l = literals(src);
        assert_eq!(
            l,
            ["csv.field_count_mismatch", "net.offline", "schema.drift"]
        );
        let mut m = BTreeMap::new();
        m.insert("net.offline".to_owned(), "crates/x.rs".to_owned());
        let r = render(&m);
        assert!(r.contains("## net") && r.contains("`net.offline`"));
        assert!(r.contains("## Retired") && r.contains("`listing.era_unsupported`"));
        assert!(reused(&m).is_empty());
        m.insert(
            "listing.era_unsupported".to_owned(),
            "crates/y.rs".to_owned(),
        );
        assert_eq!(reused(&m), ["listing.era_unsupported"]);
        let codes = collect_codes(&crate::repo_root());
        assert!(codes.contains_key("parse.timestamp"), "{codes:?}");
        assert!(codes.len() > 40);
    }
}
