//! Drafting a table definition for an I row the registry does not know.

use nemlake_core::IRow;
use nemlake_schema_model::{
    ColumnDef, ColumnId, ColumnType, Dedupe, LoadMode, Market, Provenance, SchemaVersion,
    SourceKind, TableDef, VersionDef, Visibility,
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
        }],
        provenance: Provenance {
            note: Some("draft; every column is inferred".into()),
            ..Default::default()
        },
    }
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
}
