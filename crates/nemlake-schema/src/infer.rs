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
            if nemlake_core::nemtime::parse_local_seconds(f).is_ok() {
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
    let name = format!("{}_{}", irow.report_type, irow.report_subtype);
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
        provenance: Provenance {
            note: Some("draft; every column is inferred".into()),
            ..Default::default()
        },
    }
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
