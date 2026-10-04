//! Drafting a table definition for an I row the registry does not know.

use nemlake_core::{Fingerprint, IRow};
use nemlake_schema_model::{
    ColumnDef, ColumnId, ColumnType, Dedupe, LoadMode, Market, Provenance, SchemaVersion,
    SourceKind, TableDef, VariantDef, VersionDef, Visibility,
};

/// Evidence about one column gathered from sampled D rows.
#[derive(Debug, Clone, Default)]
pub struct ColumnEvidence {
    /// Non-empty values seen.
    pub non_null: u64,
    /// Non-empty values that parsed as a number.
    pub numeric: u64,
    /// Non-empty values that parsed as a whole number.
    pub integer: u64,
    /// Non-empty values that parsed as an MMS timestamp.
    pub timestamp: u64,
    /// Longest value in bytes.
    pub max_len: u32,
}

/// Accumulates evidence over D rows (fields after the four-field prefix).
#[derive(Debug, Clone, Default)]
pub struct Sampler {
    /// One entry per I-row column.
    pub columns: Vec<ColumnEvidence>,
    /// Rows seen.
    pub rows: u64,
}

impl Sampler {
    /// A sampler for `width` columns.
    pub fn new(width: usize) -> Self {
        Self {
            columns: vec![ColumnEvidence::default(); width],
            rows: 0,
        }
    }

    /// Records one row's fields.
    pub fn observe<'a>(&mut self, fields: impl IntoIterator<Item = &'a [u8]>) {
        self.rows += 1;
        for (ev, f) in self.columns.iter_mut().zip(fields) {
            if f.is_empty() {
                continue;
            }
            ev.non_null += 1;
            ev.max_len = ev.max_len.max(f.len() as u32);
            if nemlake_core::num::parse_f64(f).is_ok() {
                ev.numeric += 1;
            }
            if nemlake_core::num::parse_i64(f).is_ok() {
                ev.integer += 1;
            }
            if nemlake_core::nemtime::is_timestamp(f) {
                ev.timestamp += 1;
            }
        }
    }

    /// The type the evidence supports for a column.
    pub fn column_type(&self, i: usize) -> ColumnType {
        let ev = &self.columns[i];
        if ev.non_null == 0 {
            return ColumnType::Utf8 {
                max_len: None,
                dict: false,
            };
        }
        if ev.timestamp == ev.non_null {
            return ColumnType::Timestamp;
        }
        if ev.numeric == ev.non_null {
            return ColumnType::Number {
                precision: None,
                scale: None,
            };
        }
        ColumnType::Utf8 {
            max_len: Some(ev.max_len),
            dict: false,
        }
    }
}

/// Drafts a table for an unknown I row, named `{REPORT_TYPE}_{REPORT_SUBTYPE}`.
///
/// Every column is marked `src = ["inferred"]`, which the validator rejects
/// until a human curates it, so a draft is written to the drafts directory
/// and never into the committed registry by accident.
pub fn draft_table(market: Market, irow: &IRow<'_>, sampler: &Sampler) -> TableDef {
    // Some tables publish no subtype: the pair (GENCONDATA, "") is GENCONDATA.
    let name = if irow.report_subtype.is_empty() {
        irow.report_type.to_owned()
    } else {
        format!("{}_{}", irow.report_type, irow.report_subtype)
    };
    let columns: Vec<ColumnDef> = irow
        .columns
        .iter()
        .enumerate()
        .map(|(i, c)| ColumnDef {
            id: ColumnId(i as u32 + 1),
            name: (*c).to_owned(),
            aliases: vec![],
            ty: if i < sampler.columns.len() {
                sampler.column_type(i)
            } else {
                ColumnType::Utf8 {
                    max_len: None,
                    dict: false,
                }
            },
            nullable: true,
            oracle: None,
            loader: None,
            added_in: SchemaVersion(irow.version),
            removed_in: None,
            description: None,
            src: vec!["inferred".into()],
            curated: false,
        })
        .collect();
    let time_column = columns
        .iter()
        .find(|c| c.ty == ColumnType::Timestamp)
        .map(|c| c.name.clone());
    TableDef {
        market,
        source: SourceKind::Mms,
        name,
        package: "UNKNOWN".into(),
        visibility: Visibility::Public,
        description: Some(format!(
            "Draft inferred from {} sampled rows; curate before committing.",
            sampler.rows
        )),
        report_pairs: vec![(irow.report_type.to_owned(), irow.report_subtype.to_owned())],
        datasets: vec![],
        archive_files: vec![],
        feeds: vec![],
        load_mode: LoadMode::Append,
        dedupe: Dedupe::None,
        primary_key: vec![],
        time_column,
        next_column_id: columns.len() as u32 + 1,
        deprecated: false,
        columns,
        versions: vec![VersionDef {
            n: SchemaVersion(irow.version),
            columns: irow.columns.iter().map(|c| (*c).to_owned()).collect(),
            fingerprint: irow.fingerprint(),
            first_seen: None,
            last_seen: None,
            src: Some("inferred".into()),
            variants: Vec::new(),
        }],
        rules: vec![],
        provenance: Provenance {
            note: Some("draft; every column is inferred".into()),
            ..Default::default()
        },
    }
}

/// Types the inferred columns of a draft from a control file, AEMO's own
/// statement of how a table's file is loaded.
///
/// The control file is the base and the rows have the last word. A column
/// the control file names takes its loader type where every sampled value
/// can be read as that type: text takes the declared length (or the longest
/// value seen, if longer), `FLOAT EXTERNAL` is a number, `INTEGER EXTERNAL`
/// a whole number, a `DATE` with a time of day a timestamp. Where a value
/// does not fit, the type guessed from the rows stays, the column remains
/// `inferred`, and the returned notes say which and why. A column typed
/// this way is no longer `inferred`: its sources are the control file and
/// the sample.
///
/// Only columns still marked `inferred` are touched, so what the registry
/// or a person already decided stands. Returns one note per column the
/// rows overruled.
pub fn apply_ctl(
    t: &mut TableDef,
    irow: &IRow<'_>,
    sampler: &Sampler,
    ctl: &nemlake_schema_model::ctl::CtlFile,
) -> Vec<String> {
    let mut notes = Vec::new();
    for c in &mut t.columns {
        if c.src != ["inferred"] {
            continue;
        }
        let Some(loader) = ctl
            .data_columns()
            .find(|k| k.name.eq_ignore_ascii_case(&c.name))
        else {
            continue;
        };
        let Some(declared) = loader.ty.column_type() else {
            continue;
        };
        // A whole number is never drafted: AEMO's data contradicts declared
        // whole numbers, and a number holds every whole value exactly.
        let declared = match declared {
            ColumnType::Integer => ColumnType::Number {
                precision: None,
                scale: None,
            },
            other => other,
        };
        let evidence = irow
            .columns
            .iter()
            .position(|n| *n == c.name)
            .and_then(|i| sampler.columns.get(i));
        let fits = evidence.is_none_or(|ev| match declared {
            ColumnType::Utf8 { .. } => true,
            ColumnType::Number { .. } | ColumnType::Integer => ev.numeric == ev.non_null,
            ColumnType::Timestamp => ev.timestamp == ev.non_null,
            // A day without a time, or a flag: the sampler cannot vouch
            // for either, so only an empty sample lets them through.
            ColumnType::Date | ColumnType::Boolean => ev.non_null == 0,
        });
        if !fits {
            notes.push(format!(
                "column {}: the control file says {} but a sampled value cannot be read as that; kept as {}",
                c.name,
                loader.ty.render(),
                c.ty.kind_name()
            ));
            continue;
        }
        c.ty = match declared {
            ColumnType::Utf8 { max_len, dict } => ColumnType::Utf8 {
                max_len: max_len.max(evidence.map(|ev| ev.max_len)),
                dict,
            },
            other => other,
        };
        c.loader = Some(loader.ty.render());
        c.src = vec!["ctl".into(), "sampled".into()];
    }
    if t.time_column.is_none() {
        t.time_column = t
            .columns
            .iter()
            .find(|c| c.ty == ColumnType::Timestamp)
            .map(|c| c.name.clone());
    } else if let Some(tc) = &t.time_column
        && t.column(tc)
            .is_none_or(|c| !matches!(c.ty, ColumnType::Timestamp | ColumnType::Date))
    {
        // The rows looked like times; the control file says otherwise.
        t.time_column = t
            .columns
            .iter()
            .find(|c| c.ty == ColumnType::Timestamp)
            .map(|c| c.name.clone());
    }
    notes
}

/// Adds the shape of `irow` to a table the registry already has: a new
/// version when its number is not declared, a variant when the number is
/// declared with another column list. Columns the table lacks are added
/// with a type from `sampler` and `src = ["inferred"]`. Returns `None` when
/// the shape is already declared.
///
/// The result is structurally valid: a column that the new shape lacks
/// becomes nullable, and a column's `added_in`/`removed_in` are moved so
/// they still bracket the versions that list it.
pub fn extend_table(t: &TableDef, irow: &IRow<'_>, sampler: &Sampler) -> Option<TableDef> {
    let fp = irow.fingerprint();
    let version = SchemaVersion(irow.version);
    if let Some(v) = t.version(version)
        && (v.fingerprint == fp || v.variant(fp).is_some())
    {
        return None;
    }
    let mut out = t.clone();
    for (i, name) in irow.columns.iter().enumerate() {
        if out.column(name).is_some() {
            continue;
        }
        out.columns.push(ColumnDef {
            id: ColumnId(out.next_column_id),
            name: (*name).to_owned(),
            aliases: vec![],
            ty: if i < sampler.columns.len() {
                sampler.column_type(i)
            } else {
                ColumnType::Utf8 {
                    max_len: None,
                    dict: false,
                }
            },
            nullable: true,
            oracle: None,
            loader: None,
            added_in: version,
            removed_in: None,
            description: None,
            src: vec!["inferred".into()],
            curated: false,
        });
        out.next_column_id += 1;
    }
    let observed: Vec<String> = irow.columns.iter().map(|c| (*c).to_owned()).collect();
    // The same list with every alias replaced by its column's name.
    let canonical: Vec<String> = irow
        .columns
        .iter()
        .map(|c| {
            out.column(c)
                .map_or_else(|| (*c).to_owned(), |d| d.name.clone())
        })
        .collect();
    let variant = VariantDef {
        columns: observed,
        fingerprint: fp,
        first_seen: None,
        last_seen: None,
    };
    let new_version = out.version(version).is_none();
    if new_version {
        let main_fp = Fingerprint::of_names(canonical.iter().map(String::as_str));
        out.versions.push(VersionDef {
            n: version,
            columns: canonical.clone(),
            fingerprint: main_fp,
            first_seen: None,
            last_seen: None,
            src: Some("inferred".into()),
            variants: if main_fp == fp { vec![] } else { vec![variant] },
        });
        out.versions.sort_by_key(|v| v.n);
    } else if let Some(v) = out.versions.iter_mut().find(|v| v.n == version) {
        v.variants.push(variant);
    }
    for c in &mut out.columns {
        if canonical.contains(&c.name) {
            if new_version {
                c.added_in = c.added_in.min(version);
                if c.removed_in.is_some_and(|r| r <= version) {
                    c.removed_in = None;
                }
            }
        } else {
            // Files of this shape have no value for the column.
            c.nullable = true;
        }
    }
    let pair = (irow.report_type.to_owned(), irow.report_subtype.to_owned());
    if !out.report_pairs.contains(&pair) {
        out.report_pairs.push(pair);
    }
    let what = if new_version {
        "version"
    } else {
        "variant of version"
    };
    let added = format!("{what} {} added by inference", irow.version);
    out.provenance.note = Some(match out.provenance.note.take() {
        Some(n) => format!("{n} | {added}"),
        None => added,
    });
    Some(out)
}

#[cfg(test)]
mod tests {
    use nemlake_schema_model::validate::validate_table;

    use super::*;

    /// The control file is the base; the rows have the last word.
    #[test]
    fn a_control_file_types_a_draft_where_the_rows_agree() {
        let ctl = nemlake_schema_model::ctl::parse(
            "x.ctl",
            r#"LOAD DATA
INFILE PUBLIC_DVD_NEW_THING_202401010000.CSV
APPEND INTO TABLE NEW_THING
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
WHEN DATE "yyyy/mm/dd hh24:mi:ss",
AMOUNT FLOAT EXTERNAL,
HOWMANY INTEGER EXTERNAL,
PARTS INTEGER EXTERNAL,
LABEL CHAR(4),
CODE FLOAT EXTERNAL,
EMPTY CHAR(3),
DAY DATE "yyyy/mm/dd",
STAMP CHAR(30))
"#,
        )
        .unwrap();
        let cols = [
            "WHEN", "AMOUNT", "HOWMANY", "PARTS", "LABEL", "CODE", "EMPTY", "DAY", "STAMP",
            "BESIDE",
        ];
        let irow = IRow {
            report_type: "NEW",
            report_subtype: "THING",
            version: 1,
            columns: &cols,
        };
        let mut s = Sampler::new(cols.len());
        s.observe([
            b"2024/01/01 00:05:00".as_slice(),
            b"1.5",
            b"3",
            b"2.5",
            b"longer",
            b"abc",
            b"",
            b"2024/01/01",
            b"2024/01/01 00:05:00",
            b"7",
        ]);
        assert_eq!((s.columns[2].integer, s.columns[3].integer), (1, 0));
        let mut t = draft_table(Market::Nem, &irow, &s);
        // The rows alone took STAMP for the time column; the control file
        // says it is text, and WHEN is the time.
        t.time_column = Some("STAMP".into());
        let notes = apply_ctl(&mut t, &irow, &s, &ctl);
        let col = |n: &str| t.column(n).unwrap();
        assert_eq!(col("WHEN").ty, ColumnType::Timestamp);
        assert_eq!(col("WHEN").src, ["ctl", "sampled"]);
        assert_eq!(
            col("WHEN").loader.as_deref(),
            Some("DATE \"yyyy/mm/dd hh24:mi:ss\"")
        );
        assert!(matches!(col("AMOUNT").ty, ColumnType::Number { .. }));
        // INTEGER EXTERNAL is drafted as a number all the same.
        assert!(matches!(col("HOWMANY").ty, ColumnType::Number { .. }));
        // A whole number was declared and 2.5 was published: a number,
        // which is what the control file is read as anyway.
        assert!(matches!(col("PARTS").ty, ColumnType::Number { .. }));
        assert_eq!(col("PARTS").src, ["ctl", "sampled"]);
        assert_eq!(col("PARTS").loader.as_deref(), Some("INTEGER EXTERNAL"));
        // Text takes the declared length, or the longest value if longer.
        assert_eq!(
            col("LABEL").ty,
            ColumnType::Utf8 {
                max_len: Some(6),
                dict: false
            }
        );
        // A number was declared and text was published: text.
        assert!(matches!(col("CODE").ty, ColumnType::Utf8 { .. }));
        assert_eq!(col("CODE").src, ["inferred"]);
        // Nothing sampled: the control file stands.
        assert_eq!(
            col("EMPTY").ty,
            ColumnType::Utf8 {
                max_len: Some(3),
                dict: false
            }
        );
        assert_eq!(col("EMPTY").src, ["ctl", "sampled"]);
        // A day without a time cannot be vouched for by the sample.
        assert_eq!(col("DAY").src, ["inferred"]);
        // What looked like a time is text by declaration; the time column
        // moves to the one that is a time.
        assert!(matches!(col("STAMP").ty, ColumnType::Utf8 { .. }));
        assert_eq!(t.time_column.as_deref(), Some("WHEN"));
        // A column the control file does not name stays a guess.
        assert_eq!(col("BESIDE").src, ["inferred"]);
        assert!(col("BESIDE").loader.is_none());
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert!(notes[0].contains("CODE") && notes[0].contains("FLOAT EXTERNAL"));
        assert!(notes[1].contains("DAY"));
        // Applying it again changes nothing: decided columns are left alone.
        let before = t.clone();
        assert_eq!(apply_ctl(&mut t, &irow, &s, &ctl).len(), 2);
        assert_eq!(t, before);
        // A draft with no time column gets the first timestamp.
        let mut t2 = draft_table(Market::Nem, &irow, &Sampler::new(0));
        assert!(t2.time_column.is_none());
        assert!(apply_ctl(&mut t2, &irow, &Sampler::new(0), &ctl).is_empty());
        assert_eq!(t2.time_column.as_deref(), Some("WHEN"));
        assert_eq!(t2.column("DAY").unwrap().ty, ColumnType::Date);
    }

    #[test]
    fn infers_types_from_evidence() {
        let cols = ["WHEN", "AMOUNT", "LABEL", "EMPTY"];
        let irow = IRow {
            report_type: "NEW",
            report_subtype: "THING",
            version: 1,
            columns: &cols,
        };
        let mut s = Sampler::new(4);
        s.observe([b"2024/01/01 00:05:00".as_slice(), b"1.5", b"abc", b""]);
        s.observe([b"2024/01/01 00:10:00".as_slice(), b"2", b"", b""]);
        assert_eq!(s.rows, 2);
        assert_eq!(s.column_type(0), ColumnType::Timestamp);
        assert_eq!(
            s.column_type(1),
            ColumnType::Number {
                precision: None,
                scale: None
            }
        );
        assert_eq!(
            s.column_type(2),
            ColumnType::Utf8 {
                max_len: Some(3),
                dict: false
            }
        );
        assert_eq!(
            s.column_type(3),
            ColumnType::Utf8 {
                max_len: None,
                dict: false
            }
        );
        let t = draft_table(Market::Nem, &irow, &s);
        assert_eq!(t.name, "NEW_THING");
        assert_eq!(t.time_column.as_deref(), Some("WHEN"));
        assert_eq!(t.versions[0].fingerprint, irow.fingerprint());
        assert_eq!(t.next_column_id, 5);
        let issues = validate_table(&t);
        assert!(
            issues.iter().all(|i| i.message.contains("inferred")),
            "{issues:?}"
        );
        assert_eq!(issues.len(), 4);
        // Text and timestamps mixed is text; a short sampler covers extra columns.
        let mut s2 = Sampler::new(1);
        s2.observe([b"2024/01/01 00:05:00".as_slice()]);
        s2.observe([b"x".as_slice()]);
        assert_eq!(
            s2.column_type(0),
            ColumnType::Utf8 {
                max_len: Some(19),
                dict: false
            }
        );
        let t2 = draft_table(Market::Nem, &irow, &s2);
        assert_eq!(
            t2.columns[3].ty,
            ColumnType::Utf8 {
                max_len: None,
                dict: false
            }
        );
        let toml = nemlake_schema_model::file::write_table(&t2);
        assert!(toml.contains("name = \"NEW_THING\""));
    }

    /// A curated table of three columns published as `version`, built
    /// here so the test does not depend on what the registry holds.
    fn scada(version: u16) -> TableDef {
        let cols = ["SETTLEMENTDATE", "DUID", "SCADAVALUE"];
        let irow = IRow {
            report_type: "DISPATCH",
            report_subtype: "UNIT_SCADA",
            version,
            columns: &cols,
        };
        let mut s = Sampler::new(3);
        s.observe([
            b"2024/01/01 00:05:00".as_slice(),
            b"BW01".as_slice(),
            b"1.5".as_slice(),
        ]);
        let mut t = draft_table(Market::Nem, &irow, &s);
        t.name = "DISPATCH_UNIT_SCADA".into();
        t.primary_key = vec!["SETTLEMENTDATE".into(), "DUID".into()];
        t.dedupe = Dedupe::Pk;
        for c in &mut t.columns {
            c.src = vec!["ctl".into()];
            c.nullable = c.name == "SCADAVALUE";
        }
        assert_eq!(validate_table(&t), vec![]);
        t
    }

    #[test]
    fn extends_a_known_table_with_new_shapes() {
        use nemlake_schema_model::validate::validate_structure;
        let t = &scada(1);
        let row = |version, columns| IRow {
            report_type: "DISPATCH",
            report_subtype: "UNIT_SCADA",
            version,
            columns,
        };
        // A shape the table declares needs nothing.
        let declared = ["SETTLEMENTDATE", "DUID", "SCADAVALUE"];
        assert!(extend_table(t, &row(1, &declared), &Sampler::new(3)).is_none());

        // A new version with a new column.
        let wider = ["SETTLEMENTDATE", "DUID", "SCADAVALUE", "QUALITY"];
        let v2 = row(2, &wider);
        let mut s = Sampler::new(4);
        s.observe([
            b"2024/01/01 00:05:00".as_slice(),
            b"BW01".as_slice(),
            b"1.5".as_slice(),
            b"GOOD".as_slice(),
        ]);
        let t2 = extend_table(t, &v2, &s).unwrap();
        assert_eq!(validate_structure(&t2), vec![]);
        assert_eq!(t2.versions.len(), 2);
        assert_eq!(t2.versions[1].n, SchemaVersion(2));
        assert_eq!(t2.versions[1].columns, wider);
        assert_eq!(t2.versions[1].fingerprint, v2.fingerprint());
        assert!(t2.versions[1].variants.is_empty());
        let q = t2.column("QUALITY").unwrap();
        assert_eq!(q.id, ColumnId(t.next_column_id));
        assert_eq!(q.added_in, SchemaVersion(2));
        assert_eq!(q.src, ["inferred"]);
        assert!(q.nullable);
        assert_eq!(
            q.ty,
            ColumnType::Utf8 {
                max_len: Some(4),
                dict: false
            }
        );
        assert_eq!(t2.next_column_id, t.next_column_id + 1);
        // Only the new column is uncurated; the rest of the table is untouched.
        let issues = validate_table(&t2);
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert!(issues[0].message.contains("QUALITY has src inferred"));
        assert_eq!(t2.columns[..3], t.columns[..]);
        assert!(
            t2.provenance
                .note
                .as_deref()
                .unwrap()
                .contains("version 2 added by inference")
        );
        // Once added, the same shape needs nothing.
        assert!(extend_table(&t2, &v2, &s).is_none());

        // The declared version with another list is a variant; a key column
        // it lacks has to admit null.
        let short = ["SETTLEMENTDATE", "SCADAVALUE"];
        let t3 = extend_table(&t2, &row(1, &short), &Sampler::new(2)).unwrap();
        assert_eq!(t3.versions.len(), 2);
        assert_eq!(t3.versions[0].variants.len(), 1);
        assert_eq!(t3.versions[0].variants[0].columns, short);
        assert!(!t2.column("DUID").unwrap().nullable);
        assert!(t3.column("DUID").unwrap().nullable);
        assert!(!t3.column("SETTLEMENTDATE").unwrap().nullable);
        assert_eq!(validate_structure(&t3), vec![]);
        assert!(
            t3.provenance
                .note
                .as_deref()
                .unwrap()
                .contains("variant of version 1 added by inference")
        );

        // The same table under another report pair: the pair is recorded.
        let other = IRow {
            report_type: "DISPATCH",
            report_subtype: "UNIT_SCADA_OLD",
            version: 3,
            columns: &declared,
        };
        let t4 = extend_table(t, &other, &Sampler::new(3)).unwrap();
        assert_eq!(t4.report_pairs.len(), t.report_pairs.len() + 1);
        assert_eq!(validate_structure(&t4), vec![]);

        // A version older than any declared: the columns it lists were there
        // earlier than the table said, and a key it lacks admits null.
        let late = scada(5);
        assert!(late.columns.iter().all(|c| c.added_in == SchemaVersion(5)));
        let head = ["SETTLEMENTDATE", "SCADAVALUE"];
        let p2 = extend_table(&late, &row(1, &head), &Sampler::new(2)).unwrap();
        assert_eq!(p2.versions.len(), 2);
        assert_eq!(p2.versions[0].n, SchemaVersion(1));
        assert_eq!(p2.versions[0].columns, head);
        assert_eq!(p2.column("SCADAVALUE").unwrap().added_in, SchemaVersion(1));
        assert_eq!(p2.column("DUID").unwrap().added_in, SchemaVersion(5));
        assert!(p2.column("DUID").unwrap().nullable);
        assert_eq!(p2.columns.len(), 3);
        assert_eq!(validate_structure(&p2), vec![]);
    }
}
