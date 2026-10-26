//! The gas feeds' `.csv` files, each read as its feed's.
//!
//! One kit has one list of decoders, tried by an entry's name, and every
//! gas feed's files are `.csv`; which feed a file belongs to, and so how
//! its block is named, its folder says. This decoder finds the feed by the
//! roots in the file's URL and hands the entry to that feed's sniffing
//! decoder: MMS rows (the GSH files) through the MMS reader, a header row
//! through the plain one, a page saved as `.csv` skipped.

use std::io::Read;
use std::sync::Arc;

use utilake_core::{Category, Diagnostic};
use utilake_kit::{BlockSink, DecodeCtx, DecodeStats, Decoder};
use utilake_mms_conform::SniffedCsv;
use utilake_plain_decode::Feed;
use utilake_plain_decode::feeds::feeds_of;
use utilake_registry::Registry;
use utilake_zip::has_extension;

/// The gas feeds' `.csv` reader.
pub struct GasCsv {
    feeds: Vec<(&'static Feed, SniffedCsv)>,
}

impl GasCsv {
    /// A reader over `registry` for every gas feed.
    pub fn new(registry: &Arc<Registry>) -> Self {
        Self {
            feeds: feeds_of("gas")
                .map(|f| (f, SniffedCsv::new(registry.clone(), f)))
                .collect(),
        }
    }

    /// The feed whose roots the path lies under.
    fn feed_of(&self, path: &str) -> Option<&(&'static Feed, SniffedCsv)> {
        self.feeds.iter().find(|(f, _)| {
            f.roots
                .iter()
                .map(|r| (*r).to_owned())
                .chain(f.local_roots())
                .any(|r| path.contains(&format!("/{}/", r.trim_matches('/'))))
        })
    }
}

impl Decoder for GasCsv {
    fn id(&self) -> &'static str {
        "csv"
    }

    fn accepts(&self, entry_name: &str) -> bool {
        has_extension(entry_name, ".csv")
    }

    fn decode(
        &self,
        reader: &mut dyn Read,
        ctx: &DecodeCtx<'_>,
        sink: &mut dyn BlockSink,
        scratch: &mut Vec<u8>,
    ) -> Result<DecodeStats, Box<Diagnostic>> {
        let path = ctx.file.remote.url.path();
        let Some((_, decoder)) = self.feed_of(path) else {
            return Err(Box::new(
                Diagnostic::error(
                    "csv.unnamed_block",
                    Category::Csv,
                    format!(
                        "The file {path} lies under none of the gas feeds' folders ({}), so no rule names its block.",
                        self.feeds
                            .iter()
                            .flat_map(|(f, _)| f.roots.iter().copied())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )
                .with_entry(ctx.entry.to_owned()),
            ));
        };
        decoder.decode(reader, ctx, sink, scratch)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use url::Url;
    use utilake_kit::{Block, DecodeOptions, Flow, RowAt, Strictness, TableRef, Transform};
    use utilake_kit::{LocalFile, RemoteFile};
    use utilake_mms_decode::Fingerprint;

    use super::*;

    #[derive(Default)]
    struct Collect {
        blocks: Vec<(String, String)>,
        warnings: Vec<Diagnostic>,
    }

    impl BlockSink for Collect {
        fn block_started(
            &mut self,
            table: &Arc<TableRef>,
            _: u16,
            _: Fingerprint,
            _: &Arc<Transform>,
            _: &str,
        ) {
            self.blocks.push((table.name.clone(), table.market.clone()));
        }
        fn batch(&mut self, _: Block) -> Result<(), Box<Diagnostic>> {
            Ok(())
        }
        fn warning(&mut self, d: Diagnostic) {
            self.warnings.push(d);
        }
        fn bad_row(&mut self, _: &[u8], _: RowAt, _: Diagnostic) -> Flow {
            Flow::Continue
        }
    }

    fn decode(
        d: &GasCsv,
        url: &str,
        entry: &str,
        bytes: &[u8],
        sink: &mut Collect,
    ) -> Result<DecodeStats, Box<Diagnostic>> {
        let file = LocalFile {
            remote: RemoteFile {
                url: Url::parse(url).unwrap(),
                source: "test",
                table: None,
                period: None,
                size: None,
                modified: None,
                modified_at: None,
            },
            path: std::path::PathBuf::from("/none"),
            sha256: String::new(),
            size: bytes.len() as u64,
            cached: true,
            etag: None,
            last_modified: None,
            last_modified_at: None,
            fetched_at: None,
            previous: None,
        };
        let options = DecodeOptions {
            strictness: Strictness::Permissive,
            max_bad_rows: 5,
            batch_rows: 1024,
        };
        let ctx = DecodeCtx {
            file: &file,
            entry,
            period: None,
            options: &options,
            previous: None,
        };
        d.decode(&mut Cursor::new(bytes), &ctx, sink, &mut Vec::new())
    }

    #[test]
    fn each_file_is_read_as_its_folders_feed() {
        let d = GasCsv::new(&Registry::load_embedded());
        assert_eq!(d.id(), "csv");
        assert!(d.accepts("x.CSV") && !d.accepts("x.zip"));
        let mut sink = Collect::default();
        // GBB: a header row, named by the file.
        let gbb = b"GasDate,FacilityName,FacilityId\n2026/09/20,Adelaide Brighton,555091\n";
        let s = decode(
            &d,
            "https://www.nemweb.com.au/Reports/CURRENT/GBB/GasBBActualFlowStorageLast31.CSV",
            "GasBBActualFlowStorageLast31.CSV",
            gbb,
            &mut sink,
        )
        .unwrap();
        assert_eq!(s.rows, 1);
        assert_eq!(
            sink.blocks[0],
            ("GBB_ACTUAL_FLOW_STORAGE".to_owned(), "gas".to_owned())
        );
        // STTM, from a mirror's path: an entry of a day's zip.
        let sttm = b"gas_date,hub,price\n01 Oct 2026,ADL,10.98\n";
        let s = decode(
            &d,
            "file:///data/x/archive/www.nemweb.com.au/Reports/CURRENT/STTM/Day01.zip",
            "int651_v1_ex_ante_market_price_rpt_1~20261001113227.csv",
            sttm,
            &mut sink,
        )
        .unwrap();
        assert_eq!(s.rows, 1);
        assert_eq!(sink.blocks[1].0, "STTM_INT651_EX_ANTE_MARKET_PRICE");
        // GSH: MMS rows, the I row naming the block.
        let gsh = b"C,GSH,TEST,1\nI,GSH,BENCHMARK_PRICE,1,GAS_DATE,HUB\nD,GSH,BENCHMARK_PRICE,1,\"2026/10/01 00:00:00\",WAL\nC,\"END OF REPORT\",4\n";
        let s = decode(&d, "https://www.nemweb.com.au/Reports/ARCHIVE/GSH/Benchmark_Price/PUBLIC_BENCHMARKPRICE_20261001.zip", "PUBLIC_BENCHMARKPRICE_202610010000_1.CSV", gsh, &mut sink).unwrap();
        assert_eq!(s.rows, 1);
        assert_eq!(sink.blocks[2].0, "GSH_BENCHMARK_PRICE");
        // A row with more fields than the header: refused, the sink told.
        let wide = b"GasDate,FacilityName\n2026/09/20,A,extra,more\n2026/09/21,B\n";
        let s = decode(
            &d,
            "https://www.nemweb.com.au/Reports/CURRENT/GBB/GasBBActualFlowStorageLast31.CSV",
            "GasBBActualFlowStorageLast31.CSV",
            wide,
            &mut sink,
        )
        .unwrap();
        assert_eq!((s.rows, s.bad_rows), (1, 1));
        // A file under no gas folder names no block.
        let e = decode(
            &d,
            "https://www.nemweb.com.au/Reports/CURRENT/Dispatch_SCADA/x.csv",
            "x.csv",
            gbb,
            &mut sink,
        )
        .unwrap_err();
        assert_eq!(e.code, "csv.unnamed_block");
        assert!(e.message.contains("Reports/CURRENT/GBB"), "{}", e.message);
    }
}
