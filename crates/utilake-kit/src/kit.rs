//! A kit bundles what one market or format needs; the registry holds every
//! kit a binary was assembled with.

use std::sync::Arc;

use url::Url;
use utilake_acquire::{RemoteFile, Source};

use crate::catalog::Catalog;
use crate::decoder::Decoder;

/// Everything one market brings: where its files are, how they are read,
/// and what tables they hold.
pub struct SourceKit {
    /// The kit's id: `nem`, `wem`.
    pub id: &'static str,
    /// The sources, each with its own id.
    pub sources: Vec<Arc<dyn Source>>,
    /// The decoders, tried in order against an entry's name.
    pub decoders: Vec<Arc<dyn Decoder>>,
    /// The tables.
    pub catalog: Arc<dyn Catalog>,
}

impl SourceKit {
    /// The decoder that reads an entry of this name, if any.
    pub fn decoder_for(&self, entry_name: &str) -> Option<&Arc<dyn Decoder>> {
        self.decoders.iter().find(|d| d.accepts(entry_name))
    }
}

/// Every kit a binary was assembled with.
#[derive(Default)]
pub struct KitRegistry {
    kits: Vec<SourceKit>,
}

impl KitRegistry {
    /// A registry of these kits, in order.
    pub fn new(kits: Vec<SourceKit>) -> Self {
        Self { kits }
    }

    /// Every kit.
    pub fn kits(&self) -> &[SourceKit] {
        &self.kits
    }

    /// A kit by its id.
    pub fn kit(&self, id: &str) -> Option<&SourceKit> {
        self.kits.iter().find(|k| k.id == id)
    }

    /// The kit and source of a source id, as the manifest records it.
    pub fn source(&self, source_id: &str) -> Option<(&SourceKit, &Arc<dyn Source>)> {
        self.kits.iter().find_map(|k| {
            k.sources
                .iter()
                .find(|s| s.id() == source_id)
                .map(|s| (k, s))
        })
    }

    /// The kit whose source recognises a file by its URL, with what the
    /// source says of it. The first kit to claim it wins.
    pub fn claim(&self, url: &Url) -> Option<(&SourceKit, RemoteFile)> {
        self.kits.iter().find_map(|k| {
            k.sources
                .iter()
                .find_map(|s| s.describe(url))
                .map(|r| (k, r))
        })
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::path::Path;

    use async_trait::async_trait;
    use utilake_acquire::{FetchError, Fetcher, Query};
    use utilake_core::Diagnostic;

    use super::*;
    use crate::decoder::{BlockSink, DecodeCtx, DecodeStats, TableRef};
    use crate::transform::Transform;

    struct Src(&'static str);

    #[async_trait]
    impl Source for Src {
        fn id(&self) -> &'static str {
            self.0
        }
        async fn list(&self, _: &Query, _: &Fetcher) -> Result<Vec<RemoteFile>, FetchError> {
            Ok(Vec::new())
        }
        fn describe(&self, url: &Url) -> Option<RemoteFile> {
            url.path().ends_with(".csv").then(|| RemoteFile {
                url: url.clone(),
                source: self.0,
                table: None,
                period: None,
                size: None,
                modified: None,
                modified_at: None,
            })
        }
    }

    struct Dec;

    impl Decoder for Dec {
        fn id(&self) -> &'static str {
            "test"
        }
        fn accepts(&self, entry_name: &str) -> bool {
            entry_name.ends_with(".csv")
        }
        /// Reads nothing, but drives the sink once through every call.
        fn decode(
            &self,
            reader: &mut dyn Read,
            ctx: &DecodeCtx<'_>,
            sink: &mut dyn BlockSink,
            scratch: &mut Vec<u8>,
        ) -> Result<DecodeStats, Box<Diagnostic>> {
            let bytes = reader.read_to_end(scratch).map_err(|e| {
                Box::new(
                    Diagnostic::error("csv.read_failed", utilake_core::Category::Csv, "x")
                        .caused_by(&e),
                )
            })?;
            let table = Arc::new(TableRef {
                market: "t".into(),
                name: "T".into(),
                dir: "t".into(),
                utc_offset_secs: 0,
                load_mode: crate::decoder::LoadMode::Append,
                time_column: None,
            });
            let transform = Arc::new(Transform::default());
            let fp = utilake_core::Fingerprint(1);
            sink.block_started(&table, 1, fp, &transform, "conforms");
            sink.batch(crate::decoder::Block {
                table,
                version: 1,
                fingerprint: fp,
                transform,
                batch: arrow_array::RecordBatch::new_empty(Arc::new(arrow_schema::Schema::empty())),
            })?;
            sink.warning(Diagnostic::warning(
                "csv.footer_count_mismatch",
                utilake_core::Category::Csv,
                "x",
            ));
            let flow = sink.bad_row(
                b"D,x",
                crate::decoder::RowAt::default(),
                Diagnostic::error("csv.field_count_mismatch", utilake_core::Category::Csv, "x"),
            );
            assert_eq!(flow, crate::decoder::Flow::Continue);
            assert_eq!(ctx.entry, "x.csv");
            Ok(DecodeStats {
                bytes: bytes as u64,
                blocks: 1,
                bad_rows: 1,
                ..DecodeStats::default()
            })
        }
    }

    struct Cat;

    impl Catalog for Cat {
        fn market(&self) -> &str {
            "t"
        }
        fn tables(&self) -> Vec<Arc<TableRef>> {
            Vec::new()
        }
        fn table(&self, _: &str) -> Option<Arc<TableRef>> {
            None
        }
        fn plans_current(&self, _: &[Transform]) -> bool {
            true
        }
        fn with_overlay(&self, _: &Path) -> Result<Arc<dyn Catalog>, Box<Diagnostic>> {
            Ok(Arc::new(Cat))
        }
    }

    #[tokio::test]
    async fn the_fakes_answer() {
        use utilake_acquire::{Archive, FetcherConfig, OfflineTransport};
        let dir = tempfile::tempdir().unwrap();
        let fetcher = Fetcher::new(
            Arc::new(OfflineTransport),
            Archive::new(dir.path()),
            FetcherConfig::default(),
        );
        let q = Query {
            tables: Vec::new(),
            exclude: Vec::new(),
            range: utilake_core::TimeRange::months(
                utilake_core::YearMonth::new(2024, 1).unwrap(),
                utilake_core::YearMonth::new(2024, 1).unwrap(),
            ),
            instants: None,
        };
        assert!(Src("s").list(&q, &fetcher).await.unwrap().is_empty());
        struct Sink;
        impl BlockSink for Sink {
            fn block_started(
                &mut self,
                _: &Arc<TableRef>,
                _: u16,
                _: utilake_core::Fingerprint,
                _: &Arc<Transform>,
                _: &str,
            ) {
            }
            fn batch(&mut self, _: crate::decoder::Block) -> Result<(), Box<Diagnostic>> {
                Ok(())
            }
            fn warning(&mut self, _: Diagnostic) {}
            fn bad_row(
                &mut self,
                _: &[u8],
                _: crate::decoder::RowAt,
                _: Diagnostic,
            ) -> crate::decoder::Flow {
                crate::decoder::Flow::Continue
            }
        }
        let local = utilake_acquire::LocalFile {
            remote: RemoteFile {
                url: Url::parse("https://h/x.csv").unwrap(),
                source: "s",
                table: None,
                period: None,
                size: None,
                modified: None,
                modified_at: None,
            },
            path: dir.path().join("x.csv"),
            sha256: String::new(),
            size: 0,
            cached: true,
            etag: None,
            last_modified: None,
            last_modified_at: None,
        };
        let options = crate::decoder::DecodeOptions {
            strictness: crate::decoder::Strictness::Permissive,
            max_bad_rows: 0,
            batch_rows: 1,
        };
        let ctx = DecodeCtx {
            file: &local,
            entry: "x.csv",
            period: None,
            options: &options,
        };
        let stats = Dec
            .decode(
                &mut std::io::Cursor::new(b"abc"),
                &ctx,
                &mut Sink,
                &mut Vec::new(),
            )
            .unwrap();
        assert_eq!((stats.bytes, stats.blocks, stats.bad_rows), (3, 1, 1));
        assert_eq!(Dec.id(), "test");
        assert!(Cat.tables().is_empty());
        assert!(Cat.table("x").is_none());
        assert!(Cat.plans_current(&[]));
        assert_eq!(Cat.with_overlay(dir.path()).unwrap().market(), "t");
    }

    #[test]
    fn finds_kits_sources_and_decoders() {
        let kits = KitRegistry::new(vec![
            SourceKit {
                id: "a",
                sources: vec![Arc::new(Src("a-one"))],
                decoders: vec![Arc::new(Dec)],
                catalog: Arc::new(Cat),
            },
            SourceKit {
                id: "b",
                sources: vec![Arc::new(Src("b-one")), Arc::new(Src("b-two"))],
                decoders: Vec::new(),
                catalog: Arc::new(Cat),
            },
        ]);
        assert_eq!(kits.kits().len(), 2);
        assert_eq!(kits.kit("b").unwrap().id, "b");
        assert!(kits.kit("c").is_none());
        let (k, s) = kits.source("b-two").unwrap();
        assert_eq!((k.id, s.id()), ("b", "b-two"));
        assert!(kits.source("nope").is_none());
        let (k, r) = kits.claim(&Url::parse("https://h/x.csv").unwrap()).unwrap();
        assert_eq!((k.id, r.source), ("a", "a-one"));
        assert!(
            kits.claim(&Url::parse("https://h/x.zip").unwrap())
                .is_none()
        );
        assert!(kits.kit("a").unwrap().decoder_for("y.csv").is_some());
        assert!(kits.kit("a").unwrap().decoder_for("y.zip").is_none());
        assert!(kits.kit("b").unwrap().decoder_for("y.csv").is_none());
        assert_eq!(KitRegistry::default().kits().len(), 0);
        assert_eq!(kits.kit("a").unwrap().catalog.market(), "t");
    }
}
