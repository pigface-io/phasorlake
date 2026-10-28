//! The lake's Iceberg tables, built from the manifest.
//!
//! A table's Iceberg metadata is a function of two things: the schemas the
//! census folded into its history, and the live outputs the manifest
//! records. [`build_table`] writes it from those alone, so a reingest, a
//! removed source or a refolded history is a rebuild, never an edit:
//!
//! - every schema of the history is added in id order, so the table's
//!   schema ids are the history's;
//! - the outputs are taken in the order the manifest gives (source month,
//!   file, entry), and each run of outputs under one schema and one AEMO
//!   version is one snapshot, with that schema current; its summary names
//!   the run's first and last source file, its months and its version;
//! - after the runs, the schema of every field the history has, under its
//!   newest name, is made current ([`union`]): the lake's files hold every
//!   field, so a reader by field id reads every column of every file, and
//!   a snapshot keeps its run's schema for a read of the table as it was;
//! - an output whose block had no schema of the history (a column list no
//!   run of it fits) is written under the table's last schema, and
//!   its snapshot says so (`phasor.schema=latest`): a reader takes its
//!   columns by field id all the same.
//!
//! The table is not partitioned. Lake files are split by the market-time
//! month, and Iceberg's month transform is the UTC month, so a month
//! partition would put the first ten hours of each month in the wrong one.
//! Snapshot ids and times come from the outputs, so the same manifest and
//! history write the same metadata.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use phasor_iceberg::Schema;
use phasor_iceberg_store::{Append, NewFile, Result, Store};

use crate::manifest::LiveOutput;

/// A table's name, id and the schemas of its history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableSchemas {
    /// The table.
    pub table: String,
    /// The table's UUID.
    pub uuid: String,
    /// Its schemas, by id from 0.
    pub schemas: Vec<Schema>,
    /// Its properties as phasor-schemas publishes them: its declaration,
    /// roles, key, breaks and the data model's words.
    pub properties: BTreeMap<String, String>,
    /// What the data model says of each column, by field id.
    pub docs: BTreeMap<u32, String>,
}

/// What building one table wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Built {
    /// Snapshots.
    pub snapshots: usize,
    /// Data files.
    pub files: usize,
    /// Rows.
    pub rows: u64,
}

/// A deterministic UUID for a table: the bytes of two hashes of its name,
/// laid out as a version 8 UUID.
pub fn table_uuid(name: &str) -> String {
    let h = |seed: u64| fnv(name.as_bytes(), seed);
    let hex = format!("{:016x}{:016x}", h(0), h(1));
    format!(
        "{}-{}-8{}-a{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[13..16],
        &hex[17..20],
        &hex[20..32]
    )
}

/// FNV-1a with a seed: enough to spread names, and no dependency.
fn fnv(bytes: &[u8], seed: u64) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325_u64 ^ seed.wrapping_mul(0x0100_0000_01b3);
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Milliseconds since 1970 at the start of `YYYY-MM` (UTC), or 0.
fn month_ms(period: Option<&str>) -> i64 {
    let Some((y, m)) = period.and_then(|p| p.split_once('-')) else {
        return 0;
    };
    let (Ok(y), Ok(m)) = (y.parse::<i64>(), m.parse::<i64>()) else {
        return 0;
    };
    // Days from 1970-01-01 to the first of the month (civil calendar).
    let (y, m) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 719_468) * 86_400_000
}

/// Builds the Iceberg metadata of table `t`, of `market`, whose files go
/// under `table_dir`, in the lake at `lake`, from `outputs` (its live
/// outputs in manifest order). Any metadata already there is replaced.
pub fn build_table(
    lake: &Path,
    market: &str,
    table_dir: &str,
    t: &TableSchemas,
    outputs: &[LiveOutput],
) -> Result<Built> {
    let dir = &lake.join(market).join(table_dir);
    let meta = dir.join("metadata");
    if meta.exists() {
        std::fs::remove_dir_all(&meta)?;
    }
    let store = Store::at(dir);
    // A lake file holds its table's columns, then each row's source and
    // publish time; so does each schema of the table the lake publishes.
    let schemas: Vec<Schema> = t
        .schemas
        .iter()
        .map(phasor_iceberg::provenance::lake_schema)
        .collect();
    let Some(first) = schemas.first() else {
        return Ok(Built::default());
    };
    let mut at = month_ms(outputs.first().and_then(|o| o.source_period.as_deref()));
    store.create(&t.uuid, first, at)?;
    for s in &schemas[1..] {
        at += 1;
        store.set_schema(s, at)?;
    }
    let latest = schemas.len() - 1;
    let mut built = Built::default();
    let mut i = 0;
    while i < outputs.len() {
        let key = |o: &LiveOutput| {
            (
                o.schema_id.map_or(latest, |s| s as usize).min(latest),
                o.source_version,
                o.schema_id.is_none(),
            )
        };
        let (schema, version, unknown) = key(&outputs[i]);
        let mut j = i;
        while j < outputs.len() && key(&outputs[j]) == (schema, version, unknown) {
            j += 1;
        }
        let run = &outputs[i..j];
        at = at.max(month_ms(run[0].source_period.as_deref())) + 1;
        store.set_schema(&schemas[schema], at)?;
        let mut properties = BTreeMap::from([
            ("phasor.first-file".to_owned(), run[0].url.clone()),
            (
                "phasor.last-file".to_owned(),
                run[run.len() - 1].url.clone(),
            ),
            ("phasor.version".to_owned(), version.to_string()),
        ]);
        for (k, o) in [
            ("first-period", &run[0]),
            ("last-period", &run[run.len() - 1]),
        ] {
            if let Some(p) = &o.source_period {
                properties.insert(format!("phasor.{k}"), p.clone());
            }
        }
        if unknown {
            properties.insert("phasor.schema".to_owned(), "latest".to_owned());
        }
        built.snapshots += 1;
        at += 1;
        let append = Append {
            files: run
                .iter()
                .map(|o| NewFile {
                    path: PathBuf::from(&o.path),
                    rows: o.rows,
                    bytes: o.bytes,
                })
                .collect(),
            snapshot_id: i64::try_from(built.snapshots).unwrap_or(i64::MAX),
            at_ms: at,
            properties,
        };
        store.append(append)?;
        built.files += run.len();
        built.rows += run.iter().map(|o| o.rows).sum::<u64>();
        i = j;
    }
    // The table reads as its files hold it: every field of every schema,
    // each with the data model's words, and the table's declaration.
    let mut all = union(&t.schemas);
    for f in &mut all.fields {
        f.doc = t.docs.get(&f.id).cloned();
    }
    store.set_schema(&phasor_iceberg::provenance::lake_schema(&all), at + 1)?;
    let mut properties = t.properties.clone();
    properties.insert(
        "phasor.lake.format".to_owned(),
        crate::provenance::LAKE_FORMAT.to_owned(),
    );
    store.set_properties(properties, at + 2)?;
    Ok(built)
}

/// Every field any schema has, by id in id order (the order the lake's
/// files hold them), under its newest name: required where every schema
/// with it requires it, and the identifier fields the schemas' keys share
/// that stay required. Its id is one past the schemas'.
pub fn union(schemas: &[Schema]) -> Schema {
    let mut fields: Vec<phasor_iceberg::Field> = Vec::new();
    for f in schemas.iter().flat_map(|s| &s.fields) {
        let required = schemas
            .iter()
            .filter_map(|s| s.field(f.id))
            .all(|g| g.required);
        let f = phasor_iceberg::Field {
            required,
            ..f.clone()
        };
        match fields.binary_search_by_key(&f.id, |g| g.id) {
            Ok(i) => fields[i] = f,
            Err(i) => fields.insert(i, f),
        }
    }
    let mut identifier_field_ids: Vec<u32> = schemas
        .iter()
        .flat_map(|s| s.identifier_field_ids.iter().copied())
        .filter(|id| fields.iter().any(|f| f.id == *id && f.required))
        .collect();
    identifier_field_ids.sort_unstable();
    identifier_field_ids.dedup();
    Schema {
        schema_id: schemas.iter().map(|s| s.schema_id + 1).max().unwrap_or(0),
        last_column_id: fields.iter().map(|f| f.id).max().unwrap_or(0),
        fields,
        identifier_field_ids,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs::File;
    use std::sync::Arc;

    use arrow_array::{ArrayRef, Float64Array, RecordBatch, StringArray};
    use arrow_schema::{DataType, Field as ArrowField, Schema as ArrowSchema};
    use parquet::arrow::ArrowWriter;
    use phasor_iceberg::{Field, IcebergType};

    use super::*;

    fn field(id: u32, name: &str, ty: IcebergType, required: bool) -> Field {
        Field {
            id,
            name: name.into(),
            ty,
            required,
            doc: None,
        }
    }

    fn schemas() -> Vec<Schema> {
        let region = field(1, "REGIONID", IcebergType::String, true);
        let rrp = field(2, "RRP", IcebergType::Double, false);
        let raise = field(3, "RAISE1SECRRP", IcebergType::Double, false);
        let schema = |id, fields: Vec<Field>| Schema {
            schema_id: id,
            fields,
            identifier_field_ids: vec![1],
            last_column_id: 3,
        };
        vec![
            schema(0, vec![region.clone(), rrp.clone()]),
            schema(1, vec![region, raise, rrp]),
        ]
    }

    /// A Parquet file of `rows` rows with the columns named (field ids as
    /// the schemas give them); its size.
    fn parquet(path: &Path, rows: usize, raise: bool) -> u64 {
        let f = |name: &str, ty, id: u32, nullable| {
            ArrowField::new(name, ty, nullable).with_metadata(HashMap::from([(
                "PARQUET:field_id".to_owned(),
                id.to_string(),
            )]))
        };
        let mut fields = vec![f("REGIONID", DataType::Utf8, 1, false)];
        let mut cols: Vec<ArrayRef> = vec![Arc::new(StringArray::from(vec!["NSW1"; rows]))];
        if raise {
            fields.push(f("RAISE1SECRRP", DataType::Float64, 3, true));
            cols.push(Arc::new(Float64Array::from(vec![1.5; rows])));
        }
        fields.push(f("RRP", DataType::Float64, 2, true));
        cols.push(Arc::new(Float64Array::from(vec![99.0; rows])));
        let batch = RecordBatch::try_new(Arc::new(ArrowSchema::new(fields)), cols).unwrap();
        let mut w =
            ArrowWriter::try_new(File::create(path).unwrap(), batch.schema(), None).unwrap();
        w.write(&batch).unwrap();
        w.close().unwrap();
        std::fs::metadata(path).unwrap().len()
    }

    fn output(
        path: &Path,
        rows: u64,
        bytes: u64,
        schema: Option<u32>,
        version: u16,
        period: &str,
    ) -> LiveOutput {
        LiveOutput {
            table: "DISPATCHPRICE".into(),
            path: path.display().to_string(),
            rows,
            bytes,
            schema_id: schema,
            source_version: version,
            source_period: Some(period.into()),
            url: format!("https://h/{period}.zip"),
            entries: vec![crate::manifest::EntryShare {
                entry: "E.CSV".into(),
                ..Default::default()
            }],
            min_time: None,
            max_time: None,
            partition: String::new(),
            last_modified_at: None,
        }
    }

    #[test]
    fn the_union_is_every_field_under_its_newest_name() {
        let old = Schema {
            schema_id: 0,
            fields: vec![
                field(1, "REGIONID", IcebergType::String, true),
                field(2, "SPELT", IcebergType::Double, true),
            ],
            identifier_field_ids: vec![1],
            last_column_id: 2,
        };
        let new = Schema {
            schema_id: 3,
            fields: vec![
                field(2, "SPELLED", IcebergType::Double, false),
                field(1, "REGIONID", IcebergType::String, true),
                field(4, "AREAID", IcebergType::String, true),
            ],
            identifier_field_ids: vec![1, 4],
            last_column_id: 4,
        };
        let u = union(&[old.clone(), new]);
        assert_eq!(u.schema_id, 4);
        assert_eq!(u.last_column_id, 4);
        let named: Vec<(u32, &str, bool)> = u
            .fields
            .iter()
            .map(|f| (f.id, f.name.as_str(), f.required))
            .collect();
        assert_eq!(
            named,
            [
                (1, "REGIONID", true),
                (2, "SPELLED", false),
                (4, "AREAID", true)
            ]
        );
        assert_eq!(u.identifier_field_ids, [1, 4]);
        assert_eq!(union(&[]).fields, []);
        assert_eq!(union(std::slice::from_ref(&old)).fields, old.fields);
    }

    #[test]
    fn runs_of_one_schema_and_version_are_snapshots() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("nem").join("dispatchprice");
        std::fs::create_dir_all(&dir).unwrap();
        let mut outs = Vec::new();
        for (i, (period, raise, schema, version)) in [
            ("2024-06", false, Some(0), 1),
            ("2024-07", false, Some(0), 1),
            ("2024-08", true, Some(1), 1),
            ("2024-09", true, Some(1), 2),
            ("2024-10", true, None, 2),
        ]
        .into_iter()
        .enumerate()
        {
            let p = dir.join(format!("{i}.parquet"));
            let n = parquet(&p, 2, raise);
            outs.push(output(&p, 2, n, schema, version, period));
        }
        let t = TableSchemas {
            table: "DISPATCHPRICE".into(),
            uuid: table_uuid("DISPATCHPRICE"),
            schemas: schemas(),
            properties: BTreeMap::from([(
                "phasor.temporal.entity".to_owned(),
                r#"["REGIONID"]"#.to_owned(),
            )]),
            docs: BTreeMap::from([(2, "The regional price".to_owned())]),
        };
        let built = build_table(tmp.path(), "nem", "dispatchprice", &t, &outs).unwrap();
        assert_eq!(
            built,
            Built {
                snapshots: 4,
                files: 5,
                rows: 10
            }
        );
        let (version, meta) = Store::at(&dir).current().unwrap();
        let ids: Vec<u32> = meta.schemas.iter().map(|s| s.schema_id).collect();
        assert_eq!(
            ids,
            [0, 1, 2],
            "the history's schemas, with its ids, and their union"
        );
        assert_eq!(meta.current_schema_id, 2, "the union is current");
        // The table carries its declaration, and its columns their words.
        assert_eq!(meta.properties["phasor.temporal.entity"], r#"["REGIONID"]"#);
        assert_eq!(meta.properties["phasor.lake.format"], "5");
        let rrp = meta.schemas[2]
            .fields
            .iter()
            .find(|f| f.name == "RRP")
            .unwrap();
        assert_eq!(rrp.doc.as_deref(), Some("The regional price"));
        let last = meta.schemas[2]
            .fields
            .iter()
            .map(|f| f.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            &last[..3],
            ["REGIONID", "RRP", "RAISE1SECRRP"],
            "every field, in id order"
        );
        assert_eq!(
            &last[last.len() - 4..],
            [
                "_phasor_source",
                "_phasor_server_last_modified",
                "_phasor_zip_entry_modified",
                "_phasor_filename_timestamp"
            ]
        );
        let snaps = &meta.snapshots;
        assert_eq!(
            snaps.iter().map(|s| s.schema_id).collect::<Vec<_>>(),
            [0, 1, 1, 1]
        );
        let props = |i: usize| &snaps[i].summary;
        assert_eq!(props(0)["phasor.first-file"], "https://h/2024-06.zip");
        assert_eq!(props(0)["phasor.last-period"], "2024-07");
        assert_eq!(props(2)["phasor.version"], "2");
        assert_eq!(props(3)["phasor.schema"], "latest");
        assert!(
            snaps
                .windows(2)
                .all(|w| w[0].timestamp_ms < w[1].timestamp_ms)
        );
        // Built again from the same facts: the same bytes.
        let read =
            || std::fs::read(dir.join(format!("metadata/v{version}.metadata.json"))).unwrap();
        let first = read();
        build_table(tmp.path(), "nem", "dispatchprice", &t, &outs).unwrap();
        assert!(first == read(), "a rebuild wrote other metadata");
        // No schemas, no table.
        let none = TableSchemas {
            schemas: Vec::new(),
            ..t
        };
        assert_eq!(
            build_table(tmp.path(), "nem", "dispatchprice", &none, &outs).unwrap(),
            Built::default()
        );
        assert!(!dir.join("metadata").exists());
    }

    #[test]
    fn uuids_and_months() {
        let u = table_uuid("DISPATCHPRICE");
        assert_eq!(u, table_uuid("DISPATCHPRICE"));
        assert_ne!(u, table_uuid("DISPATCHLOAD"));
        let groups: Vec<usize> = u.split('-').map(str::len).collect();
        assert_eq!(groups, [8, 4, 4, 4, 12]);
        assert_eq!((&u[14..15], &u[19..20]), ("8", "a"));
        assert_eq!(month_ms(Some("1970-01")), 0);
        assert_eq!(month_ms(Some("2024-08")), 1_722_470_400_000);
        assert_eq!(month_ms(Some("2024-01")), 1_704_067_200_000);
        assert_eq!(month_ms(Some("2024")), 0);
        assert_eq!(month_ms(Some("x-y")), 0);
        assert_eq!(month_ms(None), 0);
    }
}
