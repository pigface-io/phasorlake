//! A small lake with every pattern the kernel must handle: one fact from
//! three sources, two revisions, both intervention runs, versions across
//! snapshots with an open end, a table without a key, a table derived
//! from a file, and a file of an older schema.

#![allow(dead_code)]

use std::sync::Arc;

use arrow_array::{
    ArrayRef, Decimal128Array, Float64Array, Int64Array, RecordBatch, StringArray,
    TimestampMicrosecondArray,
};
use arrow_schema::{DataType, SchemaRef};
use utilake_core::Fingerprint;
use utilake_kit::Transform;
use utilake_mms_registry::{Market, Registry, TableId};
use utilake_query::{Lake, LakeFile, MarketOffset, catalog, parse_local};
use utilake_writer::manifest::LiveOutput;
use utilake_writer::{FileMeta, LakeWriter, ParquetHive, Partition, PartitionKey};

pub const NEM: MarketOffset = MarketOffset(36_000);

/// A value of one cell.
#[derive(Debug, Clone, Copy)]
pub enum V {
    S(&'static str),
    I(i64),
    F(f64),
    /// A decimal, unscaled.
    D(i128),
    /// A time, microseconds since 1970.
    T(i64),
    N,
}

/// `2024-01-01T00:05` in the market's clock, as microseconds.
pub fn ts(text: &str) -> i64 {
    parse_local::<utilake_query::time::Valid>(text, NEM)
        .unwrap()
        .get()
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
                    _ => None,
                })
                .collect::<Vec<_>>(),
        )),
        DataType::Decimal128(p, s) => Arc::new(
            Decimal128Array::from(
                vals.iter()
                    .map(|v| match v {
                        V::D(d) => Some(*d),
                        V::I(i) => Some(i128::from(*i) * 10i128.pow(u32::from(*s as u8))),
                        // A float lands at the column's scale.
                        V::F(f) => Some((f * 10f64.powi(i32::from(*s))).round() as i128),
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
                        V::T(t) => Some(*t),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
            )
            .with_timezone_opt(tz.clone()),
        ),
        other => panic!("no fixture column of type {other}"),
    }
}

/// A batch of the lake schema: the named columns as given, the rest null.
pub fn batch(schema: &SchemaRef, n: usize, cols: &[(&str, Vec<V>)]) -> RecordBatch {
    let arrays: Vec<ArrayRef> = schema
        .fields()
        .iter()
        .map(|f| {
            let vals = cols
                .iter()
                .find(|(name, _)| name == f.name())
                .map_or_else(|| vec![V::N; n], |(_, v)| v.clone());
            assert_eq!(vals.len(), n, "{}", f.name());
            column(f.data_type(), &vals)
        })
        .collect();
    RecordBatch::try_new(schema.clone(), arrays).unwrap()
}

/// One source file of a table to write.
pub struct Source<'a> {
    pub table: &'a str,
    pub partition: Partition,
    pub schema_id: Option<u32>,
    pub url: &'a str,
    pub entry: &'a str,
    pub last_modified_at: Option<&'a str>,
    pub source_period: Option<&'a str>,
    pub rows: RecordBatch,
}

/// The lake under a temp dir.
pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub registry: Arc<Registry>,
    pub outputs: Vec<LiveOutput>,
    /// Whether rows are stamped with what the server said of their file;
    /// off, only a time in the file's name stamps them.
    pub server_times: bool,
}

impl Fixture {
    pub fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
            registry: utilake_nem::registry(),
            outputs: Vec::new(),
            server_times: true,
        }
    }

    pub fn id(&self, table: &str) -> TableId {
        self.registry
            .table_by_name(table)
            .or_else(|| {
                let (rt, st) = table.split_once('.')?;
                utilake_mms_registry::Market::ALL
                    .into_iter()
                    .find_map(|m| self.registry.table_by_pair(m, rt, st))
            })
            .unwrap_or_else(|| panic!("no table {table}"))
    }

    pub fn schema(&self, table: &str) -> SchemaRef {
        self.registry.lake_schema(self.id(table)).clone()
    }

    /// Writes a source's rows as one lake file and records it.
    pub fn write(&mut self, s: &Source<'_>) -> &LiveOutput {
        self.write_together(std::slice::from_ref(s))
    }

    /// Writes the rows of several entries of one source file as one lake
    /// file, each row naming its entry, and records it: what the pipeline
    /// writes of a zip. Table, partition, URL and publish time are the
    /// first's.
    pub fn write_together(&mut self, sources: &[Source<'_>]) -> &LiveOutput {
        let s = &sources[0];
        let id = self.id(s.table);
        let table = self.registry.table(id);
        let schema = self.schema(s.table);
        let lake = utilake_writer::provenance::lake_schema(&schema);
        let time_column = table
            .time_column
            .as_deref()
            .and_then(|n| schema.index_of(n).ok());
        let mut hive = ParquetHive::new(self.dir.path().join("lake"));
        let key = PartitionKey {
            market: Market::Nem.to_string(),
            table_dir: self.registry.table_dir(id),
            partition: s.partition,
        };
        let stem = s
            .url
            .rsplit('/')
            .next()
            .unwrap()
            .split('.')
            .next()
            .unwrap()
            .to_ascii_lowercase()
            .replace('%', "_");
        let meta = FileMeta {
            table: table.name.clone(),
            source_stem: stem,
            schema_version: 1,
            fingerprint: Fingerprint(self.outputs.len() as u128 + 1),
            source_url: s.url.to_owned(),
            source_sha256: format!("{:064x}", self.outputs.len() + 1),
            time_column,
            part: 1,
            transform: Transform {
                block_family: String::new(),
                block_name: String::new(),
                source_version: 1,
                source_columns: Vec::new(),
                source_period: s.source_period.map(str::to_owned),
                plan_hash: String::new(),
                schema_source: "registry".into(),
                resolution: "conforms".into(),
                shape_id: None,
                schema_id: s.schema_id,
            },
        };
        let w = hive.open(&key, lake.clone(), &meta).unwrap();
        let modified = s.last_modified_at.filter(|_| self.server_times);
        let published = utilake_core::published::published(s.url, modified, NEM.0);
        for e in sources {
            let stamp = utilake_writer::provenance::Stamp::new(
                &utilake_query::source_name(s.url, e.entry),
                published,
            );
            let rows = utilake_writer::provenance::stamped(&e.rows, &stamp, &lake).unwrap();
            hive.write(w, &rows).unwrap();
        }
        let written = hive.close(w).unwrap();
        hive.commit().unwrap();
        self.outputs.push(LiveOutput {
            table: table.name.clone(),
            path: written.path,
            rows: written.rows,
            bytes: written.bytes,
            schema_id: s.schema_id,
            source_version: 1,
            source_period: s.source_period.map(str::to_owned),
            url: s.url.to_owned(),
            entries: sources
                .iter()
                .map(|e| utilake_writer::manifest::EntryShare {
                    entry: e.entry.to_owned(),
                    published,
                    ..Default::default()
                })
                .collect(),
            min_time: written.min_time,
            max_time: written.max_time,
            partition: written.partition.to_string(),
            last_modified_at: s.last_modified_at.map(str::to_owned),
        });
        self.outputs.last().unwrap()
    }

    pub fn files(&self) -> Vec<LakeFile> {
        catalog(&self.outputs).files
    }

    pub fn lake(&self) -> Lake<'_> {
        let c = catalog(&self.outputs);
        Lake {
            registry: &self.registry,
            files: c.files,
            sources: c.sources,
            version: "test",
        }
    }
}

pub const DVD: &str = "https://nemweb.com.au/Data_Archive/Wholesale_Electricity/MMSDM/2024/MMSDM_2024_01/MMSDM/DATA/PUBLIC_DVD_DISPATCHPRICE_202401010000.zip";
pub const ARCHIVE: &str = "https://nemweb.com.au/Data_Archive/Wholesale_Electricity/MMSDM/2024/MMSDM_2024_01/MMSDM/DATA/PUBLIC_ARCHIVE%23DISPATCHPRICE%23FILE01%23202401010000.zip";
pub const REPORT: &str = "https://nemweb.com.au/Reports/ARCHIVE/DispatchIS_Reports/PUBLIC_DISPATCHIS_202401010005_0000000400000001.zip";
pub const DEC: &str = "https://nemweb.com.au/Data_Archive/Wholesale_Electricity/MMSDM/2023/MMSDM_2023_12/MMSDM/DATA/PUBLIC_DVD_DISPATCHPRICE_202312010000.zip";

/// DISPATCHPRICE: one day's first intervals from a DVD month, an archive
/// copy of it, and a live report, plus the last interval of December in
/// its own partition.
///
/// | interval | source | RUNNO | INTERVENTION | RRP | LASTCHANGED |
/// |---|---|---|---|---|---|
/// | 12-31 24:00 | dec dvd | 1 | 0 | 50 | 00:00:30 |
/// | 00:05 | report | 1 | 0 | 99 | 00:05:30 |
/// | 00:05 | dvd | 1 | 0 | 100 | 00:05:30 |
/// | 00:05 | archive | 1 | 0 | 101 | 00:05:30 |
/// | 00:10 | report | 1 | 0 | 110 | 00:10:30 |
/// | 00:10 | dvd | 1 | 0 | 110 | 00:10:30 |
/// | 00:10 | dvd | 2 | 0 | 111 | 00:12:00 |
/// | 00:15 | dvd | 1 | 0 | 120 | 00:15:30 |
/// | 00:15 | dvd | 1 | 1 | 125 | 00:15:30 |
pub fn dispatchprice(fx: &mut Fixture) {
    let schema = fx.schema("DISPATCHPRICE");
    let t = |s: &str| V::T(ts(s));
    let month = Partition::Month {
        year: 2024,
        month: 1,
    };
    let rows = |n: usize, cols: &[(&str, Vec<V>)]| batch(&schema, n, cols);
    fx.write(&Source {
        table: "DISPATCHPRICE",
        partition: Partition::Month {
            year: 2023,
            month: 12,
        },
        schema_id: Some(2),
        url: DEC,
        entry: "PUBLIC_DVD_DISPATCHPRICE_202312010000.CSV",
        last_modified_at: Some("2024-01-05T00:00:00Z"),
        source_period: Some("2023-12"),
        rows: rows(
            1,
            &[
                ("SETTLEMENTDATE", vec![t("2024-01-01T00:00")]),
                ("RUNNO", vec![V::I(1)]),
                ("REGIONID", vec![V::S("NSW1")]),
                ("DISPATCHINTERVAL", vec![V::I(20_231_231_288)]),
                ("INTERVENTION", vec![V::I(0)]),
                ("RRP", vec![V::I(50)]),
                ("LASTCHANGED", vec![t("2024-01-01T00:00:30")]),
            ],
        ),
    });
    fx.write(&Source {
        table: "DISPATCHPRICE",
        partition: month,
        schema_id: Some(0),
        url: REPORT,
        entry: "PUBLIC_DISPATCHIS_202401010005_0000000400000001.CSV",
        last_modified_at: None,
        source_period: Some("2024-01"),
        rows: rows(
            2,
            &[
                (
                    "SETTLEMENTDATE",
                    vec![t("2024-01-01T00:05"), t("2024-01-01T00:10")],
                ),
                ("RUNNO", vec![V::I(1), V::I(1)]),
                ("REGIONID", vec![V::S("NSW1"), V::S("NSW1")]),
                (
                    "DISPATCHINTERVAL",
                    vec![V::I(20_240_101_001), V::I(20_240_101_002)],
                ),
                ("INTERVENTION", vec![V::I(0), V::I(0)]),
                ("RRP", vec![V::I(99), V::I(110)]),
                (
                    "LASTCHANGED",
                    vec![t("2024-01-01T00:05:30"), t("2024-01-01T00:10:30")],
                ),
                // A column schema 0 lacks: the file has it, the projection
                // must not.
                ("PRE_AP_LOWER1_PRICE", vec![V::I(7), V::I(7)]),
            ],
        ),
    });
    fx.write(&Source {
        table: "DISPATCHPRICE",
        partition: month,
        schema_id: Some(2),
        url: DVD,
        entry: "PUBLIC_DVD_DISPATCHPRICE_202401010000.CSV",
        last_modified_at: Some("2024-02-05T00:00:00Z"),
        source_period: Some("2024-01"),
        rows: rows(
            5,
            &[
                (
                    "SETTLEMENTDATE",
                    vec![
                        t("2024-01-01T00:05"),
                        t("2024-01-01T00:10"),
                        t("2024-01-01T00:10"),
                        t("2024-01-01T00:15"),
                        t("2024-01-01T00:15"),
                    ],
                ),
                ("RUNNO", vec![V::I(1), V::I(1), V::I(2), V::I(1), V::I(1)]),
                ("REGIONID", vec![V::S("NSW1"); 5]),
                (
                    "DISPATCHINTERVAL",
                    vec![
                        V::I(20_240_101_001),
                        V::I(20_240_101_002),
                        V::I(20_240_101_002),
                        V::I(20_240_101_003),
                        V::I(20_240_101_003),
                    ],
                ),
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
                        t("2024-01-01T00:05:30"),
                        t("2024-01-01T00:10:30"),
                        t("2024-01-01T00:12:00"),
                        t("2024-01-01T00:15:30"),
                        t("2024-01-01T00:15:30"),
                    ],
                ),
                ("PRE_AP_LOWER1_PRICE", vec![V::I(8); 5]),
            ],
        ),
    });
    fx.write(&Source {
        table: "DISPATCHPRICE",
        partition: month,
        schema_id: Some(2),
        url: ARCHIVE,
        entry: "PUBLIC_ARCHIVE#DISPATCHPRICE#FILE01#202401010000.CSV",
        last_modified_at: Some("2024-02-06T00:00:00Z"),
        source_period: Some("2024-01"),
        rows: rows(
            1,
            &[
                ("SETTLEMENTDATE", vec![t("2024-01-01T00:05")]),
                ("RUNNO", vec![V::I(1)]),
                ("REGIONID", vec![V::S("NSW1")]),
                ("DISPATCHINTERVAL", vec![V::I(20_240_101_001)]),
                ("INTERVENTION", vec![V::I(0)]),
                ("RRP", vec![V::I(101)]),
                ("LASTCHANGED", vec![t("2024-01-01T00:05:30")]),
            ],
        ),
    });
}

pub const SNAP_JAN: &str = "https://nemweb.com.au/Data_Archive/Wholesale_Electricity/MMSDM/2024/MMSDM_2024_01/MMSDM/DATA/PUBLIC_DVD_DUDETAIL_202401010000.zip";
pub const SNAP_FEB: &str = "https://nemweb.com.au/Data_Archive/Wholesale_Electricity/MMSDM/2024/MMSDM_2024_02/MMSDM/DATA/PUBLIC_DVD_DUDETAIL_202402010000.zip";

/// DUDETAIL: two snapshots.
///
/// | DUID | EFFECTIVEDATE | VERSIONNO | REGISTEREDCAPACITY | in |
/// |---|---|---|---|---|
/// | A | 2020-01-01 | 1 | 10 | both |
/// | A | 2023-06-01 | 1 | 20 | both |
/// | A | 2023-06-01 | 2 | 21 | feb (a correction, LASTCHANGED later) |
/// | B | 2021-01-01 | 1 | 30 | both |
/// | C | 2022-01-01 | 1 | 40 | both |
/// | C | "no end" | 1 | 41 | both |
pub fn dudetail(fx: &mut Fixture) {
    let schema = fx.schema("DUDETAIL");
    let t = |s: &str| V::T(ts(s));
    let open = V::T(utilake_query::time::OPEN_US);
    let base = |extra: bool| {
        let mut eff = vec![
            t("2020-01-01T00:00"),
            t("2023-06-01T00:00"),
            t("2021-01-01T00:00"),
            t("2022-01-01T00:00"),
            open,
        ];
        let mut duid = vec![V::S("A"), V::S("A"), V::S("B"), V::S("C"), V::S("C")];
        let mut ver = vec![V::I(1); 5];
        let mut cap = vec![V::F(10.0), V::F(20.0), V::F(30.0), V::F(40.0), V::F(41.0)];
        let mut lc = vec![t("2023-12-01T00:00"); 5];
        if extra {
            eff.push(t("2023-06-01T00:00"));
            duid.push(V::S("A"));
            ver.push(V::I(2));
            cap.push(V::F(21.0));
            lc.push(t("2024-01-20T00:00"));
        }
        let n = eff.len();
        batch(
            &schema,
            n,
            &[
                ("EFFECTIVEDATE", eff),
                ("DUID", duid),
                ("VERSIONNO", ver),
                ("REGISTEREDCAPACITY", cap),
                ("LASTCHANGED", lc),
            ],
        )
    };
    fx.write(&Source {
        table: "DUDETAIL",
        partition: Partition::Snapshot {
            year: 2024,
            month: 1,
        },
        schema_id: Some(6),
        url: SNAP_JAN,
        entry: "PUBLIC_DVD_DUDETAIL_202401010000.CSV",
        last_modified_at: Some("2024-02-05T00:00:00Z"),
        source_period: Some("2024-01"),
        rows: base(false),
    });
    fx.write(&Source {
        table: "DUDETAIL",
        partition: Partition::Snapshot {
            year: 2024,
            month: 2,
        },
        schema_id: Some(6),
        url: SNAP_FEB,
        entry: "PUBLIC_DVD_DUDETAIL_202402010000.CSV",
        last_modified_at: Some("2024-03-05T00:00:00Z"),
        source_period: Some("2024-02"),
        rows: base(true),
    });
}

/// SSM_ENABLEMENT_COSTS has no key: the same two rows in two files.
pub fn keyless(fx: &mut Fixture) {
    let schema = fx.schema("SSM_ENABLEMENT_COSTS");
    let t = |s: &str| V::T(ts(s));
    let rows = || {
        batch(
            &schema,
            2,
            &[
                (
                    "END_TRADINGDATE",
                    vec![t("2024-01-02T00:00"), t("2024-01-02T00:00")],
                ),
                ("ENABLEMENT_REASON", vec![V::S("x"), V::S("y")]),
                ("ESTIMATED_COSTS", vec![V::I(1), V::I(2)]),
                (
                    "LASTCHANGED",
                    vec![t("2024-01-03T00:00"), t("2024-01-03T00:00")],
                ),
            ],
        )
    };
    for (url, lm) in [
        (
            "https://h/MMSDM_2024_01/PUBLIC_DVD_SSM_ENABLEMENT_COSTS_202401010000.zip",
            "2024-02-05T00:00:00Z",
        ),
        (
            "https://h/MMSDM_2024_01/PUBLIC_ARCHIVE%23SSM_ENABLEMENT_COSTS%23FILE01%23202401010000.zip",
            "2024-02-06T00:00:00Z",
        ),
    ] {
        fx.write(&Source {
            table: "SSM_ENABLEMENT_COSTS",
            partition: Partition::Unknown,
            schema_id: Some(0),
            url,
            entry: "X.CSV",
            last_modified_at: Some(lm),
            source_period: Some("2024-01"),
            rows: rows(),
        });
    }
}

/// A table no history declares, derived from an I row: text columns.
pub fn derived(fx: &mut Fixture) -> String {
    let irow = utilake_mms_decode::IRow {
        report_type: "UTILAKE",
        report_subtype: "NOBODY",
        version: 1,
        columns: &["WHEN", "WHAT"],
    };
    let id = fx.registry.derive(Market::Nem, &irow).unwrap();
    let name = fx.registry.table(id).name.clone();
    let schema = fx.registry.lake_schema(id).clone();
    let rows = batch(
        &schema,
        2,
        &[
            (
                "WHEN",
                vec![V::S("2024/01/01 00:05:00"), V::S("2024/01/01 00:10:00")],
            ),
            ("WHAT", vec![V::S("a"), V::S("a")]),
        ],
    );
    fx.write(&Source {
        table: "UTILAKE.NOBODY",
        partition: Partition::Month {
            year: 2024,
            month: 1,
        },
        schema_id: None,
        url: "file:///x/nobody.csv",
        entry: "",
        last_modified_at: None,
        source_period: Some("2024-01"),
        rows,
    });
    name
}

pub const BIDS_DEC: &str = "https://nemweb.com.au/Data_Archive/Wholesale_Electricity/MMSDM/2023/MMSDM_2023_12/MMSDM/DATA/PUBLIC_DVD_BIDDAYOFFER_D_202312010000.zip";
pub const BIDS_JAN: &str = "https://nemweb.com.au/Data_Archive/Wholesale_Electricity/MMSDM/2024/MMSDM_2024_01/MMSDM/DATA/PUBLIC_DVD_BIDDAYOFFER_D_202401010000.zip";

/// BIDDAYOFFER_D labels its rows with the trading day, written at
/// midnight: one unit, two bid types, the trading days of 31 December and
/// 1 January, each in its month's partition; January's offer has two
/// versions.
///
/// | SETTLEMENTDATE | DUID | BIDTYPE | VERSIONNO | PRICEBAND1 |
/// |---|---|---|---|---|
/// | 2023-12-31 | A | ENERGY | 1 | 10 |
/// | 2023-12-31 | A | RAISE6SEC | 1 | 11 |
/// | 2024-01-01 | A | ENERGY | 1 | 20 |
/// | 2024-01-01 | A | ENERGY | 2 | 21 |
/// | 2024-01-01 | A | RAISE6SEC | 1 | 22 |
pub fn biddayoffer(fx: &mut Fixture) {
    let schema = fx.schema("BIDDAYOFFER_D");
    let t = |s: &str| V::T(ts(s));
    fx.write(&Source {
        table: "BIDDAYOFFER_D",
        partition: Partition::Month {
            year: 2023,
            month: 12,
        },
        schema_id: None,
        url: BIDS_DEC,
        entry: "PUBLIC_DVD_BIDDAYOFFER_D_202312010000.CSV",
        last_modified_at: Some("2024-01-05T00:00:00Z"),
        source_period: Some("2023-12"),
        rows: batch(
            &schema,
            2,
            &[
                (
                    "SETTLEMENTDATE",
                    vec![t("2023-12-31T00:00"), t("2023-12-31T00:00")],
                ),
                ("DUID", vec![V::S("A"), V::S("A")]),
                ("BIDTYPE", vec![V::S("ENERGY"), V::S("RAISE6SEC")]),
                (
                    "OFFERDATE",
                    vec![t("2023-12-30T12:00"), t("2023-12-30T12:00")],
                ),
                ("VERSIONNO", vec![V::F(1.0), V::F(1.0)]),
                ("PRICEBAND1", vec![V::F(10.0), V::F(11.0)]),
                (
                    "LASTCHANGED",
                    vec![t("2023-12-30T12:00"), t("2023-12-30T12:00")],
                ),
            ],
        ),
    });
    fx.write(&Source {
        table: "BIDDAYOFFER_D",
        partition: Partition::Month {
            year: 2024,
            month: 1,
        },
        schema_id: None,
        url: BIDS_JAN,
        entry: "PUBLIC_DVD_BIDDAYOFFER_D_202401010000.CSV",
        last_modified_at: Some("2024-02-05T00:00:00Z"),
        source_period: Some("2024-01"),
        rows: batch(
            &schema,
            3,
            &[
                (
                    "SETTLEMENTDATE",
                    vec![
                        t("2024-01-01T00:00"),
                        t("2024-01-01T00:00"),
                        t("2024-01-01T00:00"),
                    ],
                ),
                ("DUID", vec![V::S("A"), V::S("A"), V::S("A")]),
                (
                    "BIDTYPE",
                    vec![V::S("ENERGY"), V::S("ENERGY"), V::S("RAISE6SEC")],
                ),
                (
                    "OFFERDATE",
                    vec![
                        t("2023-12-31T12:00"),
                        t("2023-12-31T18:00"),
                        t("2023-12-31T12:00"),
                    ],
                ),
                ("VERSIONNO", vec![V::F(1.0), V::F(2.0), V::F(1.0)]),
                ("PRICEBAND1", vec![V::F(20.0), V::F(21.0), V::F(22.0)]),
                (
                    "LASTCHANGED",
                    vec![
                        t("2023-12-31T12:00"),
                        t("2023-12-31T18:00"),
                        t("2023-12-31T12:00"),
                    ],
                ),
            ],
        ),
    });
}

pub const SUMMARY_JAN: &str = "https://nemweb.com.au/Data_Archive/Wholesale_Electricity/MMSDM/2024/MMSDM_2024_01/MMSDM/DATA/PUBLIC_DVD_DUDETAILSUMMARY_202401010000.zip";

/// DUDETAILSUMMARY is in force from START_DATE and ends at END_DATE: unit
/// A has two versions, the first ending where the second starts; unit B
/// retired in June 2023; unit C's end is AEMO's "no end".
///
/// | DUID | START_DATE | END_DATE | REGIONID |
/// |---|---|---|---|
/// | A | 2020-01-01 | 2022-01-01 | NSW1 |
/// | A | 2022-01-01 | 2999-12-31 (open) | VIC1 |
/// | B | 2021-01-01 | 2023-06-01 | QLD1 |
/// | C | 2021-01-01 | 2999-12-31 (open) | SA1 |
pub fn dudetailsummary(fx: &mut Fixture) {
    let schema = fx.schema("DUDETAILSUMMARY");
    let t = |s: &str| V::T(ts(s));
    let open = || V::T(utilake_query::time::OPEN_US);
    fx.write(&Source {
        table: "DUDETAILSUMMARY",
        partition: Partition::Snapshot {
            year: 2024,
            month: 1,
        },
        schema_id: None,
        url: SUMMARY_JAN,
        entry: "PUBLIC_DVD_DUDETAILSUMMARY_202401010000.CSV",
        last_modified_at: Some("2024-02-05T00:00:00Z"),
        source_period: Some("2024-01"),
        rows: batch(
            &schema,
            4,
            &[
                ("DUID", vec![V::S("A"), V::S("A"), V::S("B"), V::S("C")]),
                (
                    "START_DATE",
                    vec![
                        t("2020-01-01T00:00"),
                        t("2022-01-01T00:00"),
                        t("2021-01-01T00:00"),
                        t("2021-01-01T00:00"),
                    ],
                ),
                (
                    "END_DATE",
                    vec![t("2022-01-01T00:00"), open(), t("2023-06-01T00:00"), open()],
                ),
                (
                    "REGIONID",
                    vec![V::S("NSW1"), V::S("VIC1"), V::S("QLD1"), V::S("SA1")],
                ),
                ("LASTCHANGED", vec![t("2023-12-01T00:00"); 4]),
            ],
        ),
    });
}
