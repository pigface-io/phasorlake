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
//! - an output whose block had no schema of the history (a column list the
//!   census has not folded) is written under the table's last schema, and
//!   its snapshot says so (`utilake.schema=latest`): a reader takes its
//!   columns by field id all the same.
//!
//! The table is not partitioned. Lake files are split by the market-time
//! month, and Iceberg's month transform is the UTC month, so a month
//! partition would put the first ten hours of each month in the wrong one.
//! Snapshot ids and times come from the outputs, so the same manifest and
//! history write the same metadata.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use utilake_iceberg::{PartitionSpec, Schema};
use utilake_iceberg_store::{Append, NewFile, Result, Store};

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

/// Builds the Iceberg metadata of table `t` in `dir`, from `outputs` (its
/// live outputs in manifest order). Any metadata already there is replaced.
pub async fn build_table(dir: &Path, t: &TableSchemas, outputs: &[LiveOutput]) -> Result<Built> {
    let meta = dir.join("metadata");
    if meta.exists() {
        std::fs::remove_dir_all(&meta)?;
    }
    let store = Store::at(dir);
    let Some(first) = t.schemas.first() else {
        return Ok(Built::default());
    };
    let mut at = month_ms(outputs.first().and_then(|o| o.source_period.as_deref()));
    store.create(&t.uuid, first, &PartitionSpec::by_month(None), at)?;
    for s in &t.schemas[1..] {
        at += 1;
        store.set_schema(s, at)?;
    }
    let latest = t.schemas.len() - 1;
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
        store.set_schema(&t.schemas[schema], at)?;
        let mut properties = BTreeMap::from([
            ("utilake.first-file".to_owned(), run[0].url.clone()),
            (
                "utilake.last-file".to_owned(),
                run[run.len() - 1].url.clone(),
            ),
            ("utilake.version".to_owned(), version.to_string()),
        ]);
        for (k, o) in [
            ("first-period", &run[0]),
            ("last-period", &run[run.len() - 1]),
        ] {
            if let Some(p) = &o.source_period {
                properties.insert(format!("utilake.{k}"), p.clone());
            }
        }
        if unknown {
            properties.insert("utilake.schema".to_owned(), "latest".to_owned());
        }
        built.snapshots += 1;
        at += 1;
        store
            .append(Append {
                files: run
                    .iter()
                    .map(|o| NewFile {
                        path: PathBuf::from(&o.path),
                        rows: o.rows,
                        bytes: o.bytes,
                        month: None,
                    })
                    .collect(),
                snapshot_id: i64::try_from(built.snapshots).unwrap_or(i64::MAX),
                at_ms: at,
                properties,
            })
            .await?;
        built.files += run.len();
        built.rows += run.iter().map(|o| o.rows).sum::<u64>();
        i = j;
    }
    Ok(built)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs::File;
    use std::sync::Arc;

    use arrow_array::{ArrayRef, Float64Array, RecordBatch, StringArray};
    use arrow_schema::{DataType, Field as ArrowField, Schema as ArrowSchema};
    use parquet::arrow::ArrowWriter;
    use utilake_iceberg::{Field, IcebergType};

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
            entry: "E.CSV".into(),
            min_time: None,
            max_time: None,
        }
    }

    #[tokio::test]
    async fn runs_of_one_schema_and_version_are_snapshots() {
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
        };
        let built = build_table(&dir, &t, &outs).await.unwrap();
        assert_eq!(
            built,
            Built {
                snapshots: 4,
                files: 5,
                rows: 10
            }
        );
        let (version, meta) = Store::at(&dir).current().unwrap();
        let mut ids: Vec<i32> = meta.schemas_iter().map(|s| s.schema_id()).collect();
        ids.sort_unstable();
        assert_eq!(ids, [0, 1], "the history's schemas, with its ids");
        let mut snaps: Vec<_> = meta.snapshots().collect();
        snaps.sort_by_key(|s| s.sequence_number());
        assert_eq!(
            snaps.iter().map(|s| s.schema_id()).collect::<Vec<_>>(),
            [Some(0), Some(1), Some(1), Some(1)]
        );
        let props = |i: usize| &snaps[i].summary().additional_properties;
        assert_eq!(props(0)["utilake.first-file"], "https://h/2024-06.zip");
        assert_eq!(props(0)["utilake.last-period"], "2024-07");
        assert_eq!(props(2)["utilake.version"], "2");
        assert_eq!(props(3)["utilake.schema"], "latest");
        assert!(
            snaps
                .windows(2)
                .all(|w| w[0].timestamp_ms() < w[1].timestamp_ms())
        );
        // Built again from the same facts: the same bytes.
        let read =
            || std::fs::read(dir.join(format!("metadata/v{version}.metadata.json"))).unwrap();
        let first = read();
        build_table(&dir, &t, &outs).await.unwrap();
        assert!(first == read(), "a rebuild wrote other metadata");
        // No schemas, no table.
        let none = TableSchemas {
            schemas: Vec::new(),
            ..t
        };
        assert_eq!(
            build_table(&dir, &none, &outs).await.unwrap(),
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
