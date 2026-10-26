//! Every example in `docs/reading.md` runs through the DuckDB CLI over a
//! small lake written as the pipeline writes it, and returns the rows
//! written here. The doc is the only way utilake describes a read, so a
//! query there that drifts from the lake fails the tests.
//!
//! The lake: DISPATCHPRICE from a live report, the month's DVD and an
//! archive copy re-touched in 2026 whose zip entry is dated the night it
//! was written; DUDETAIL in two monthly snapshots, the second holding a
//! correction and the version that ends it; BIDDAYOFFER_D with a NaN
//! version; SSM_ENABLEMENT_COSTS twice, with no key.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use arrow_array::{
    ArrayRef, Decimal128Array, Float64Array, Int64Array, RecordBatch, StringArray,
    TimestampMicrosecondArray,
};
use arrow_schema::{DataType, SchemaRef};
use utilake_core::{Fingerprint, source_times, timetext};
use utilake_kit::Transform;
use utilake_mms_registry::{Market, Registry};
use utilake_writer::{FileMeta, LakeWriter, ParquetHive, Partition, PartitionKey};
use utilake_writer::{Stamp, Times, lake_schema, stamped};

/// The NEM's clock, AEST.
const NEM: i32 = 36_000;

/// `2024-01-01T00:05[:SS]` in the market's clock, as microseconds.
fn ts(text: &str) -> i64 {
    let padded = if text.len() == 16 {
        format!("{text}:00")
    } else {
        text.to_owned()
    };
    let s = timetext::parse_naive(&padded, NEM).unwrap_or_else(|| panic!("{text}"));
    s.secs_utc * 1_000_000
}

/// A value of one cell.
#[derive(Debug, Clone, Copy)]
enum V {
    S(&'static str),
    I(i64),
    F(f64),
    T(&'static str),
    N,
}

fn column(dt: &DataType, vals: &[V]) -> ArrayRef {
    match dt {
        DataType::Utf8 => Arc::new(StringArray::from(
            vals.iter()
                .map(|v| match v {
                    V::S(s) => Some(*s),
                    _ => None,
                })
                .collect::<Vec<_>>(),
        )),
        DataType::Int64 => Arc::new(Int64Array::from(
            vals.iter()
                .map(|v| match v {
                    V::I(i) => Some(*i),
                    _ => None,
                })
                .collect::<Vec<_>>(),
        )),
        DataType::Float64 => Arc::new(Float64Array::from(
            vals.iter()
                .map(|v| match v {
                    V::F(f) => Some(*f),
                    V::I(i) => Some(*i as f64),
                    _ => None,
                })
                .collect::<Vec<_>>(),
        )),
        DataType::Decimal128(p, s) => Arc::new(
            Decimal128Array::from(
                vals.iter()
                    .map(|v| match v {
                        V::I(i) => Some(i128::from(*i) * 10i128.pow(u32::from(*s as u8))),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
            )
            .with_precision_and_scale(*p, *s)
            .unwrap(),
        ),
        DataType::Timestamp(_, tz) => Arc::new(
            TimestampMicrosecondArray::from(
                vals.iter()
                    .map(|v| match v {
                        V::T(t) => Some(ts(t)),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
            )
            .with_timezone_opt(tz.clone()),
        ),
        other => panic!("no fixture column of type {other}"),
    }
}

/// One source file's rows of a table.
struct Source {
    table: &'static str,
    partition: Partition,
    url: &'static str,
    entry: &'static str,
    /// The server's `Last-Modified`, RFC 3339.
    server: Option<&'static str>,
    /// The zip entry's modified time, in the market's clock.
    zip_entry: Option<&'static str>,
    /// The schema of the table's history its block was read with; the
    /// latest when none is given.
    schema_id: Option<u32>,
    rows: Vec<(&'static str, Vec<V>)>,
}

/// The lake under a temp dir: the home the doc's queries run from.
struct Lake {
    home: tempfile::TempDir,
    registry: Arc<Registry>,
    files: u128,
    /// What each written file is, as the manifest would list it.
    outputs: Vec<utilake_writer::manifest::LiveOutput>,
}

impl Lake {
    fn new() -> Self {
        Self {
            home: tempfile::tempdir().unwrap(),
            registry: utilake_nem::registry(),
            files: 0,
            outputs: Vec::new(),
        }
    }

    fn schema(&self, table: &str) -> SchemaRef {
        let id = self.registry.table_by_name(table).unwrap();
        self.registry.lake_schema(id).clone()
    }

    /// Writes a source's rows as one lake file, each row stamped with its
    /// source and times as an ingest stamps them.
    fn write(&mut self, s: &Source) {
        let id = self.registry.table_by_name(s.table).unwrap();
        let table = self.registry.table(id);
        let schema = self.schema(s.table);
        let n = s.rows[0].1.len();
        let arrays: Vec<ArrayRef> = schema
            .fields()
            .iter()
            .map(|f| {
                let vals = s
                    .rows
                    .iter()
                    .find(|(name, _)| name == f.name())
                    .map_or_else(|| vec![V::N; n], |(_, v)| v.clone());
                assert_eq!(vals.len(), n, "{}", f.name());
                column(f.data_type(), &vals)
            })
            .collect();
        let batch = RecordBatch::try_new(schema.clone(), arrays).unwrap();
        let lake = lake_schema(&schema);
        let mut hive = ParquetHive::new(self.home.path().join("lake"));
        let key = PartitionKey {
            market: Market::Nem.to_string(),
            table_dir: self.registry.table_dir(id),
            partition: s.partition,
        };
        self.files += 1;
        let meta = FileMeta {
            table: table.name.clone(),
            source_stem: format!("source{}", self.files),
            schema_version: 1,
            fingerprint: Fingerprint(self.files),
            source_url: s.url.to_owned(),
            source_sha256: format!("{:064x}", self.files),
            time_column: table
                .time_column
                .as_deref()
                .and_then(|c| schema.index_of(c).ok()),
            part: 1,
            transform: Transform {
                block_family: String::new(),
                block_name: String::new(),
                source_version: 1,
                source_columns: Vec::new(),
                source_period: Some("2024-01".into()),
                plan_hash: String::new(),
                schema_source: "registry".into(),
                resolution: "conforms".into(),
                shape_id: None,
                schema_id: None,
            },
        };
        let times = Times {
            server_last_modified: source_times::server_last_modified(s.server),
            zip_entry_modified: s.zip_entry.map(ts),
            filename_timestamp: source_times::filename_timestamp(s.entry, NEM),
        };
        let stamp = Stamp::new(&format!("{}!{}", s.url, s.entry), times);
        let w = hive.open(&key, lake.clone(), &meta).unwrap();
        hive.write(w, &stamped(&batch, &stamp, &lake).unwrap())
            .unwrap();
        let written = hive.close(w).unwrap();
        hive.commit().unwrap();
        self.outputs.push(utilake_writer::manifest::LiveOutput {
            table: table.name.clone(),
            path: written.path,
            rows: written.rows,
            bytes: written.bytes,
            schema_id: s.schema_id.or_else(|| {
                self.registry
                    .history(id)
                    .map(|h| h.schemas.iter().map(|s| s.schema_id).max().unwrap_or(0))
            }),
            source_version: 1,
            source_period: Some("2024-01".into()),
            url: s.url.to_owned(),
            entries: Vec::new(),
            min_time: written.min_time,
            max_time: written.max_time,
            partition: written.partition.to_string(),
            last_modified_at: s.server.map(str::to_owned),
        });
    }

    /// Each table's Iceberg metadata, as an ingest leaves it.
    fn iceberg(&self) {
        let mut tables: Vec<&str> = self.outputs.iter().map(|o| o.table.as_str()).collect();
        tables.sort_unstable();
        tables.dedup();
        for name in tables {
            let id = self.registry.table_by_name(name).unwrap();
            let Some(h) = self.registry.history(id) else {
                continue;
            };
            let published = self.registry.table(id).published();
            let t = utilake_writer::iceberg::TableSchemas {
                table: name.to_owned(),
                uuid: utilake_writer::iceberg::table_uuid(name),
                schemas: h.schemas.clone(),
                properties: published.properties,
                docs: published.docs,
            };
            let outs: Vec<_> = self
                .outputs
                .iter()
                .filter(|o| o.table == name)
                .cloned()
                .collect();
            utilake_writer::iceberg::build_table(
                &self.home.path().join("lake"),
                &Market::Nem.to_string(),
                &self.registry.table_dir(id),
                &t,
                &outs,
            )
            .unwrap();
        }
    }
}

const MONTH: Partition = Partition::Month {
    year: 2024,
    month: 1,
};

fn dispatchprice(lake: &mut Lake) {
    let common = |n: usize| {
        vec![
            ("REGIONID", vec![V::S("NSW1"); n]),
            ("DISPATCHINTERVAL", vec![V::I(1); n]),
        ]
    };
    let rows = |cols: Vec<(&'static str, Vec<V>)>| {
        let n = cols[0].1.len();
        let mut all = common(n);
        all.extend(cols);
        all
    };
    // A live report, public within seconds.
    lake.write(&Source {
        table: "DISPATCHPRICE",
        partition: MONTH,
        url: "https://nemweb.com.au/Reports/CURRENT/DispatchIS_Reports/PUBLIC_DISPATCHIS_202401010005_0000000400000001.zip",
        entry: "PUBLIC_DISPATCHIS_202401010005_0000000400000001.CSV",
        server: Some("2023-12-31T14:05:40Z"),
        zip_entry: None,
        schema_id: None,
        rows: rows(vec![
            (
                "SETTLEMENTDATE",
                vec![V::T("2024-01-01T00:05"), V::T("2024-01-01T00:10")],
            ),
            ("RUNNO", vec![V::I(1), V::I(1)]),
            ("INTERVENTION", vec![V::I(0), V::I(0)]),
            ("RRP", vec![V::I(99), V::I(110)]),
            (
                "LASTCHANGED",
                vec![V::T("2024-01-01T00:05:30"), V::T("2024-01-01T00:10:30")],
            ),
        ]),
    });
    // The month's DVD, a month later: a rerun at 00:10, and both
    // intervention runs at 00:15.
    lake.write(&Source {
        table: "DISPATCHPRICE",
        partition: MONTH,
        url: "https://nemweb.com.au/Data_Archive/Wholesale_Electricity/MMSDM/2024/MMSDM_2024_01/MMSDM/DATA/PUBLIC_DVD_DISPATCHPRICE_202401010000.zip",
        entry: "PUBLIC_DVD_DISPATCHPRICE_202401010000.CSV",
        server: Some("2024-02-05T00:00:00Z"),
        zip_entry: None,
        schema_id: None,
        rows: rows(vec![
            (
                "SETTLEMENTDATE",
                vec![
                    V::T("2024-01-01T00:05"),
                    V::T("2024-01-01T00:10"),
                    V::T("2024-01-01T00:10"),
                    V::T("2024-01-01T00:15"),
                    V::T("2024-01-01T00:15"),
                ],
            ),
            ("RUNNO", vec![V::I(1), V::I(1), V::I(2), V::I(1), V::I(1)]),
            (
                "INTERVENTION",
                vec![V::I(0), V::I(0), V::I(0), V::I(0), V::I(1)],
            ),
            (
                "RRP",
                vec![V::I(100), V::I(110), V::I(111), V::I(120), V::I(125)],
            ),
            (
                "LASTCHANGED",
                vec![
                    V::T("2024-01-01T00:05:30"),
                    V::T("2024-01-01T00:10:30"),
                    V::T("2024-01-01T00:12:00"),
                    V::T("2024-01-01T00:15:30"),
                    V::T("2024-01-01T00:15:30"),
                ],
            ),
        ]),
    });
    // An archive copy the server re-touched in 2026, its entry written
    // five seconds after the interval's LASTCHANGED.
    lake.write(&Source {
        table: "DISPATCHPRICE",
        partition: MONTH,
        url: "https://nemweb.com.au/Reports/ARCHIVE/DispatchIS_Reports/PUBLIC_DISPATCHIS_20240101.zip",
        entry: "PUBLIC_DISPATCHIS_202401010005_0000000400000001.zip",
        server: Some("2026-04-15T00:00:00Z"),
        zip_entry: Some("2024-01-01T00:05:35"),
        schema_id: None,
        rows: rows(vec![
            ("SETTLEMENTDATE", vec![V::T("2024-01-01T00:05")]),
            ("RUNNO", vec![V::I(1)]),
            ("INTERVENTION", vec![V::I(0)]),
            ("RRP", vec![V::I(101)]),
            ("LASTCHANGED", vec![V::T("2024-01-01T00:05:30")]),
        ]),
    });
}

fn dudetail(lake: &mut Lake) {
    let snapshot = |month| Partition::Snapshot { year: 2024, month };
    lake.write(&Source {
        table: "DUDETAIL",
        partition: snapshot(1),
        url: "https://nemweb.com.au/Data_Archive/Wholesale_Electricity/MMSDM/2024/MMSDM_2024_01/MMSDM/DATA/PUBLIC_DVD_DUDETAIL_202401010000.zip",
        entry: "PUBLIC_DVD_DUDETAIL_202401010000.CSV",
        server: Some("2024-02-05T00:00:00Z"),
        zip_entry: None,
        schema_id: None,
        rows: vec![
            ("DUID", vec![V::S("A"), V::S("A"), V::S("B")]),
            (
                "EFFECTIVEDATE",
                vec![
                    V::T("2020-01-01T00:00"),
                    V::T("2023-06-01T00:00"),
                    V::T("2021-01-01T00:00"),
                ],
            ),
            ("VERSIONNO", vec![V::I(1); 3]),
            ("REGISTEREDCAPACITY", vec![V::I(10), V::I(20), V::I(30)]),
            ("LASTCHANGED", vec![V::T("2023-12-01T00:00"); 3]),
        ],
    });
    // February's snapshot: everything again, a correction of A's 2023
    // version, and A's next version, which ends it.
    lake.write(&Source {
        table: "DUDETAIL",
        partition: snapshot(2),
        url: "https://nemweb.com.au/Data_Archive/Wholesale_Electricity/MMSDM/2024/MMSDM_2024_02/MMSDM/DATA/PUBLIC_DVD_DUDETAIL_202402010000.zip",
        entry: "PUBLIC_DVD_DUDETAIL_202402010000.CSV",
        server: Some("2024-03-05T00:00:00Z"),
        zip_entry: None,
        schema_id: None,
        rows: vec![
            (
                "DUID",
                vec![V::S("A"), V::S("A"), V::S("A"), V::S("A"), V::S("B")],
            ),
            (
                "EFFECTIVEDATE",
                vec![
                    V::T("2020-01-01T00:00"),
                    V::T("2023-06-01T00:00"),
                    V::T("2023-06-01T00:00"),
                    V::T("2024-03-01T00:00"),
                    V::T("2021-01-01T00:00"),
                ],
            ),
            (
                "VERSIONNO",
                vec![V::I(1), V::I(1), V::I(2), V::I(1), V::I(1)],
            ),
            (
                "REGISTEREDCAPACITY",
                vec![V::I(10), V::I(20), V::I(21), V::I(25), V::I(30)],
            ),
            (
                "LASTCHANGED",
                vec![
                    V::T("2023-12-01T00:00"),
                    V::T("2023-12-01T00:00"),
                    V::T("2024-01-20T00:00"),
                    V::T("2024-02-10T00:00"),
                    V::T("2023-12-01T00:00"),
                ],
            ),
        ],
    });
}

fn biddayoffer(lake: &mut Lake) {
    lake.write(&Source {
        table: "BIDDAYOFFER_D",
        partition: MONTH,
        url: "https://nemweb.com.au/Data_Archive/Wholesale_Electricity/MMSDM/2024/MMSDM_2024_01/MMSDM/DATA/PUBLIC_DVD_BIDDAYOFFER_D_202401010000.zip",
        entry: "PUBLIC_DVD_BIDDAYOFFER_D_202401010000.CSV",
        server: Some("2024-02-05T00:00:00Z"),
        zip_entry: None,
        schema_id: None,
        rows: vec![
            (
                "SETTLEMENTDATE",
                vec![
                    V::T("2023-12-31T00:00"),
                    V::T("2024-01-01T00:00"),
                    V::T("2024-01-01T00:00"),
                    V::T("2024-01-01T00:00"),
                    V::T("2024-01-01T00:00"),
                ],
            ),
            ("DUID", vec![V::S("A"); 5]),
            (
                "BIDTYPE",
                vec![
                    V::S("ENERGY"),
                    V::S("ENERGY"),
                    V::S("ENERGY"),
                    V::S("ENERGY"),
                    V::S("RAISE6SEC"),
                ],
            ),
            (
                "OFFERDATE",
                vec![
                    V::T("2023-12-30T10:00"),
                    V::T("2023-12-31T10:00"),
                    V::T("2023-12-31T12:00"),
                    V::T("2023-12-31T12:00"),
                    V::T("2023-12-31T10:00"),
                ],
            ),
            (
                "VERSIONNO",
                vec![V::F(1.0), V::F(1.0), V::F(2.0), V::F(f64::NAN), V::F(1.0)],
            ),
            (
                "PRICEBAND1",
                vec![V::I(10), V::I(20), V::I(21), V::I(99), V::I(22)],
            ),
            (
                "LASTCHANGED",
                vec![
                    V::T("2023-12-30T10:00"),
                    V::T("2023-12-31T10:00"),
                    V::T("2023-12-31T12:00"),
                    V::T("2023-12-31T12:00"),
                    V::T("2023-12-31T10:00"),
                ],
            ),
        ],
    });
}

/// An older BIDDAYOFFER_D file, read with the table's first schema: it
/// still carries MR_FACTOR, which later schemas dropped.
fn older_bids(lake: &mut Lake) {
    lake.write(&Source {
        table: "BIDDAYOFFER_D",
        partition: Partition::Month {
            year: 2023,
            month: 12,
        },
        url: "https://nemweb.com.au/Data_Archive/Wholesale_Electricity/MMSDM/2023/MMSDM_2023_12/MMSDM/DATA/PUBLIC_DVD_BIDDAYOFFER_D_202312010000.zip",
        entry: "PUBLIC_DVD_BIDDAYOFFER_D_202312010000.CSV",
        server: Some("2024-01-05T00:00:00Z"),
        zip_entry: None,
        schema_id: Some(0),
        rows: vec![
            ("SETTLEMENTDATE", vec![V::T("2023-12-01T00:00")]),
            ("DUID", vec![V::S("A")]),
            ("BIDTYPE", vec![V::S("ENERGY")]),
            ("OFFERDATE", vec![V::T("2023-11-30T10:00")]),
            ("VERSIONNO", vec![V::F(1.0)]),
            ("PRICEBAND1", vec![V::I(5)]),
            ("MR_FACTOR", vec![V::I(1)]),
            ("LASTCHANGED", vec![V::T("2023-11-30T10:00")]),
        ],
    });
}

fn keyless(lake: &mut Lake) {
    for (url, server) in [
        (
            "https://nemweb.com.au/MMSDM_2024_01/PUBLIC_DVD_SSM_ENABLEMENT_COSTS_202401010000.zip",
            "2024-02-05T00:00:00Z",
        ),
        (
            "https://nemweb.com.au/MMSDM_2024_01/PUBLIC_ARCHIVE%23SSM_ENABLEMENT_COSTS%23FILE01%23202401010000.zip",
            "2024-02-06T00:00:00Z",
        ),
    ] {
        lake.write(&Source {
            table: "SSM_ENABLEMENT_COSTS",
            partition: Partition::Unknown,
            url,
            entry: "X.CSV",
            server: Some(server),
            zip_entry: None,
            schema_id: None,
            rows: vec![
                (
                    "END_TRADINGDATE",
                    vec![V::T("2024-01-02T00:00"), V::T("2024-01-02T00:00")],
                ),
                ("ENABLEMENT_REASON", vec![V::S("x"), V::S("y")]),
                ("ESTIMATED_COSTS", vec![V::I(1), V::I(2)]),
                (
                    "LASTCHANGED",
                    vec![V::T("2024-01-03T00:00"), V::T("2024-01-03T00:00")],
                ),
            ],
        });
    }
}

/// Each example of the doc: its name and its SQL.
fn examples(doc: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = doc;
    while let Some(i) = rest.find("<!-- example: ") {
        let after = &rest[i + "<!-- example: ".len()..];
        let name = after[..after.find(" -->").unwrap()].to_owned();
        let open = after.find("```sql\n").unwrap() + "```sql\n".len();
        let body = &after[open..];
        let close = body.find("```").unwrap();
        out.push((name, body[..close].trim().to_owned()));
        rest = &body[close..];
    }
    out
}

/// Copies the directory `from` into `to`, whole.
fn copy_dir(from: &Path, to: &Path) {
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

/// The rows of a query through the DuckDB CLI, as CSV with a header, run
/// from `home` as the doc says.
fn duck(home: &Path, sql: &str) -> String {
    let out = Command::new("duckdb")
        .current_dir(home)
        .args([
            "-csv",
            "-c",
            &format!("SET TimeZone = 'UTC'; LOAD iceberg; {sql}"),
        ])
        .output()
        .expect("the duckdb CLI is on PATH: run `just duckdb`");
    assert!(
        out.status.success(),
        "{}\n{sql}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

const REPORT: &str = "https://nemweb.com.au/Reports/CURRENT/DispatchIS_Reports/PUBLIC_DISPATCHIS_202401010005_0000000400000001.zip!PUBLIC_DISPATCHIS_202401010005_0000000400000001.CSV";
const DVD: &str = "https://nemweb.com.au/Data_Archive/Wholesale_Electricity/MMSDM/2024/MMSDM_2024_01/MMSDM/DATA/PUBLIC_DVD_DISPATCHPRICE_202401010000.zip!PUBLIC_DVD_DISPATCHPRICE_202401010000.CSV";
const ARCHIVE: &str = "https://nemweb.com.au/Reports/ARCHIVE/DispatchIS_Reports/PUBLIC_DISPATCHIS_20240101.zip!PUBLIC_DISPATCHIS_202401010005_0000000400000001.zip";

/// What each example returns over the fixture lake, times in UTC.
fn expected() -> BTreeMap<&'static str, String> {
    let lines = |rows: &[&str]| rows.iter().map(|r| format!("{r}\n")).collect::<String>();
    BTreeMap::from([
        // The archive copy of 00:05 was published last of three; the rerun
        // wins at 00:10; both intervention runs at 00:15.
        (
            "latest",
            lines(&[
                "SETTLEMENTDATE,INTERVENTION,RUNNO,RRP,_ul_source",
                &format!("2023-12-31 14:05:00+00,0,1,101.00000,{ARCHIVE}"),
                &format!("2023-12-31 14:10:00+00,0,2,111.00000,{DVD}"),
                &format!("2023-12-31 14:15:00+00,0,1,120.00000,{DVD}"),
                &format!("2023-12-31 14:15:00+00,1,1,125.00000,{DVD}"),
            ]),
        ),
        (
            "revisions",
            lines(&[
                "SETTLEMENTDATE,INTERVENTION,RUNNO,RRP",
                "2023-12-31 14:05:00+00,0,1,101.00000",
                "2023-12-31 14:10:00+00,0,1,110.00000",
                "2023-12-31 14:10:00+00,0,2,111.00000",
                "2023-12-31 14:15:00+00,0,1,120.00000",
                "2023-12-31 14:15:00+00,1,1,125.00000",
            ]),
        ),
        // At 00:11 only the live report was public: not the rerun at
        // 00:12, not the DVD of February, not the copy the server dates 2026.
        (
            "known-at",
            lines(&[
                "SETTLEMENTDATE,RRP,_ul_source",
                &format!("2023-12-31 14:05:00+00,99.00000,{REPORT}"),
                &format!("2023-12-31 14:10:00+00,110.00000,{REPORT}"),
            ]),
        ),
        // By the zip entry's time the archive copy was known at 00:05:35,
        // and among known copies it ranks first by the server's time.
        (
            "known-at-earliest",
            lines(&[
                "SETTLEMENTDATE,RRP,_ul_source",
                &format!("2023-12-31 14:05:00+00,101.00000,{ARCHIVE}"),
                &format!("2023-12-31 14:10:00+00,110.00000,{REPORT}"),
            ]),
        ),
        // A's corrected 2023 version, ended by the version only February's
        // snapshot holds; B open.
        (
            "in-force",
            lines(&[
                "DUID,EFFECTIVEDATE,VERSIONNO,REGISTEREDCAPACITY,valid_to",
                "A,2023-05-31 14:00:00+00,2,21.0,2024-02-29 14:00:00+00",
                "B,2020-12-31 14:00:00+00,1,30.0,NULL",
            ]),
        ),
        (
            "order-nan",
            lines(&[
                "SETTLEMENTDATE,DUID,BIDTYPE,VERSIONNO,PRICEBAND1",
                "2023-11-30 14:00:00+00,A,ENERGY,1.0,5.00",
                "2023-12-30 14:00:00+00,A,ENERGY,1.0,10.00",
                "2023-12-31 14:00:00+00,A,ENERGY,2.0,21.00",
                "2023-12-31 14:00:00+00,A,RAISE6SEC,1.0,22.00",
            ]),
        ),
        (
            "trading-day",
            lines(&["DUID,BIDTYPE,revisions", "A,ENERGY,3", "A,RAISE6SEC,1"]),
        ),
        ("keyless", String::new()),
        // Read by field id, the older file's MR_FACTOR is there; the later
        // ones, which no longer carry it, read null.
        (
            "across-changes",
            lines(&[
                "SETTLEMENTDATE,VERSIONNO,MR_FACTOR",
                "2023-11-30 14:00:00+00,1.0,1.000000",
                "2023-12-30 14:00:00+00,1.0,NULL",
                "2023-12-31 14:00:00+00,1.0,NULL",
                "2023-12-31 14:00:00+00,2.0,NULL",
            ]),
        ),
        (
            "iceberg-latest",
            lines(&[
                "SETTLEMENTDATE,RRP",
                "2023-12-31 14:05:00+00,101.00000",
                "2023-12-31 14:10:00+00,111.00000",
            ]),
        ),
        // The same, read from a copy of the home.
        (
            "moved",
            lines(&[
                "SETTLEMENTDATE,RRP",
                "2023-12-31 14:05:00+00,101.00000",
                "2023-12-31 14:10:00+00,111.00000",
            ]),
        ),
        (
            "sources",
            lines(&[
                "_ul_source,rows,public_by_server",
                &format!("{DVD},5,2024-02-05 00:00:00+00"),
                &format!("{ARCHIVE},1,2026-04-15 00:00:00+00"),
                &format!("{REPORT},2,2023-12-31 14:05:40+00"),
            ]),
        ),
        (
            "entity-values",
            lines(&[
                "DUID,rows,first,last",
                "A,6,2019-12-31 14:00:00+00,2024-02-29 14:00:00+00",
                "B,2,2020-12-31 14:00:00+00,2020-12-31 14:00:00+00",
            ]),
        ),
        ("interval-step", lines(&["step,seen", "00:05:00,2"])),
    ])
}

#[test]
fn every_example_in_the_reading_doc_returns_its_rows() {
    let doc_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/reading.md");
    let doc = std::fs::read_to_string(&doc_path).unwrap();
    let mut lake = Lake::new();
    dispatchprice(&mut lake);
    dudetail(&mut lake);
    biddayoffer(&mut lake);
    older_bids(&mut lake);
    keyless(&mut lake);
    lake.iceberg();
    let mut want = expected();
    let examples = examples(&doc);
    assert!(examples.len() >= 10, "{} examples", examples.len());
    for (name, sql) in &examples {
        let rows = want
            .remove(name.as_str())
            .unwrap_or_else(|| panic!("no rows written for the example {name}"));
        if name == "moved" {
            // Run where the lake was copied to, its metadata naming the
            // old place: the files are found under each table's `data/`.
            let copy = tempfile::tempdir().unwrap();
            copy_dir(&lake.home.path().join("lake"), &copy.path().join("lake"));
            assert_eq!(duck(copy.path(), sql), rows, "example {name}:\n{sql}");
            continue;
        }
        let got = duck(lake.home.path(), sql);
        if name == "keyless" {
            // Two copies of two rows: the two, and every column of the table.
            let mut lines = got.lines();
            let header = lines.next().unwrap();
            assert!(
                header.starts_with("END_TRADINGDATE,ENABLEMENT_REASON,ESTIMATED_COSTS")
                    && !header.contains("_ul_"),
                "{header}"
            );
            let reasons: Vec<&str> = lines.map(|l| l.split(',').nth(1).unwrap()).collect();
            assert_eq!(reasons, ["x", "y"], "{got}");
            continue;
        }
        assert_eq!(got, rows, "example {name}:\n{sql}");
        if name == "order-nan" {
            // Without the filter, the NaN version wins: why it is there.
            let unfiltered = sql.replace(
                "WHERE VERSIONNO IS DISTINCT FROM CAST('NaN' AS DOUBLE)\n",
                "",
            );
            assert_ne!(&unfiltered, sql);
            assert!(duck(lake.home.path(), &unfiltered).contains(",nan,99.00"));
        }
    }
    assert!(want.is_empty(), "rows for examples the doc lacks: {want:?}");
}
