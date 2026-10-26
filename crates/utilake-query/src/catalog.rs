//! The lake's files of a table and the sources their rows came from, each
//! source with every time it was stamped with, in one total order (by the
//! server's word) that the kernel
//! breaks ties by. A file holds the rows of one source file's entries;
//! each row names its source in `_ul_source`.

use std::sync::Arc;

use utilake_writer::manifest::LiveOutput;

use crate::time::KnownTime;

/// One source of rows: a source file, or one entry of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// The source file's URL.
    pub url: Arc<str>,
    /// The entry of the source file; empty for a file read whole.
    pub entry: Arc<str>,
    /// The HTTP `Last-Modified` the web server gave for its bytes.
    pub server_last_modified: Option<KnownTime>,
    /// Its modified time in its zip's central directory.
    pub zip_entry_modified: Option<KnownTime>,
    /// The fourteen-digit stamp in its file name.
    pub filename_timestamp: Option<KnownTime>,
}

impl Source {
    /// The source as `url!entry`, the `_ul_source` of its rows.
    pub fn name(&self) -> String {
        source_name(&self.url, &self.entry)
    }

    /// Where it stands among sources.
    pub fn stamp(&self) -> SourceStamp<'_> {
        SourceStamp {
            server_last_modified: self.server_last_modified,
            url: &self.url,
            entry: &self.entry,
        }
    }
}

/// `url!entry`, or the URL alone for a file read whole: the name the
/// writer gives a row's source.
pub fn source_name(url: &str, entry: &str) -> String {
    if entry.is_empty() {
        url.to_owned()
    } else {
        format!("{url}!{entry}")
    }
}

/// One Parquet file of the lake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LakeFile {
    /// The table.
    pub table: String,
    /// Where the file is.
    pub path: String,
    /// The Iceberg schema it was written under, when known.
    pub schema_id: Option<u32>,
    /// Its partition, as the directory names it.
    pub partition: String,
    /// Its least time value, microseconds since 1970 (UTC).
    pub min_us: Option<i64>,
    /// Its greatest.
    pub max_us: Option<i64>,
    /// The source file's URL.
    pub url: Arc<str>,
    /// The month of the source, `YYYY-MM`.
    pub source_period: Option<String>,
    /// The sources its rows may name, by name, each with its rank among
    /// the lake's sources ([`Lake::sources`](crate::Lake)); sorted by name.
    pub sources: Box<[(Arc<str>, u32)]>,
}

impl LakeFile {
    /// The rank of the source named `name`, when the file's rows may name it.
    pub fn rank_of(&self, name: &str) -> Option<u32> {
        self.sources
            .binary_search_by(|(n, _)| n.as_ref().cmp(name))
            .ok()
            .map(|i| self.sources[i].1)
    }
}

/// The total order of sources: by publish time (a source without one
/// before every source with one), then URL, then entry. Among copies of
/// one row the greatest stamp wins, whatever order the files were read in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SourceStamp<'a> {
    /// The server's `Last-Modified`: copies of a fact rank by the server's
    /// word, whichever time a read is known by.
    pub server_last_modified: Option<KnownTime>,
    /// The URL.
    pub url: &'a str,
    /// The entry.
    pub entry: &'a str,
}

/// The lake's files and the sources of their rows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Catalog {
    /// The files, in the order of their first source, then by path.
    pub files: Vec<LakeFile>,
    /// Every source, in stamp order: a source's index is its rank.
    pub sources: Vec<Source>,
}

/// The lake's files and sources from the manifest's live outputs, each
/// source public when the writer stamped its rows.
pub fn catalog(outputs: &[LiveOutput]) -> Catalog {
    let mut sources: Vec<Source> = outputs
        .iter()
        .flat_map(|o| {
            let url: Arc<str> = Arc::from(o.url.as_str());
            o.entries.iter().map(move |e| Source {
                url: url.clone(),
                entry: Arc::from(e.entry.as_str()),
                server_last_modified: e.server_last_modified.map(KnownTime::new),
                zip_entry_modified: e.zip_entry_modified.map(KnownTime::new),
                filename_timestamp: e.filename_timestamp.map(KnownTime::new),
            })
        })
        .collect();
    sources.sort_by(|a, b| a.stamp().cmp(&b.stamp()));
    sources.dedup_by(|a, b| a.url == b.url && a.entry == b.entry);
    let ranks: std::collections::HashMap<(&str, &str), u32> = sources
        .iter()
        .enumerate()
        .map(|(i, s)| {
            (
                (&*s.url, &*s.entry),
                u32::try_from(i).expect("fewer than 2^32 sources"),
            )
        })
        .collect();
    let rank = |url: &str, entry: &str| ranks[&(url, entry)];
    let mut files: Vec<LakeFile> = outputs
        .iter()
        .map(|o| {
            let mut named: Vec<(Arc<str>, u32)> = o
                .entries
                .iter()
                .map(|e| {
                    (
                        Arc::from(source_name(&o.url, &e.entry)),
                        rank(&o.url, &e.entry),
                    )
                })
                .collect();
            named.sort_by(|a, b| a.0.cmp(&b.0));
            LakeFile {
                table: o.table.clone(),
                path: o.path.clone(),
                schema_id: o.schema_id,
                partition: o.partition.clone(),
                min_us: o.min_time.map(|t| t.div_euclid(1_000)),
                max_us: o.max_time.map(|t| t.div_euclid(1_000)),
                url: Arc::from(o.url.as_str()),
                source_period: o.source_period.clone(),
                sources: named.into_boxed_slice(),
            }
        })
        .collect();
    let first = |f: &LakeFile| f.sources.iter().map(|(_, r)| *r).min();
    files.sort_by(|a, b| first(a).cmp(&first(b)).then_with(|| a.path.cmp(&b.path)));
    Catalog { files, sources }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(url: &str, entries: &[&str], published: Option<i64>, path: &str) -> LiveOutput {
        LiveOutput {
            table: "T".into(),
            path: path.into(),
            rows: 1,
            bytes: 1,
            schema_id: Some(0),
            source_version: 1,
            source_period: Some("2024-01".into()),
            url: url.into(),
            entries: entries
                .iter()
                .map(|e| utilake_writer::manifest::EntryShare {
                    entry: (*e).to_owned(),
                    server_last_modified: published,
                    ..Default::default()
                })
                .collect(),
            min_time: Some(2_000),
            max_time: Some(3_999),
            partition: "year=2024/month=01".into(),
            last_modified_at: None,
        }
    }

    #[test]
    fn sources_and_files_are_in_stamp_order() {
        let outs = [
            output(
                "https://h/PUBLIC_DVD_T_202401010000.zip",
                &["T.CSV"],
                Some(1_707_091_200_000_000),
                "b.parquet",
            ),
            output(
                "https://h/PUBLIC_T_202401020005_1.zip",
                &["T2.CSV", "T1.CSV"],
                Some(1_704_031_500_000_000),
                "a.parquet",
            ),
            // A second file of the same source file: its sources are not
            // counted twice.
            output(
                "https://h/PUBLIC_T_202401020005_1.zip",
                &["T1.CSV"],
                Some(1_704_031_500_000_000),
                "a2.parquet",
            ),
            output("file:///x/T.CSV", &[""], None, "c.parquet"),
        ];
        let c = catalog(&outs);
        let names: Vec<String> = c.sources.iter().map(Source::name).collect();
        // No stamp first, then the report of 2 January (its entries by
        // name), then the archive month published in February.
        assert_eq!(
            names,
            [
                "file:///x/T.CSV",
                "https://h/PUBLIC_T_202401020005_1.zip!T1.CSV",
                "https://h/PUBLIC_T_202401020005_1.zip!T2.CSV",
                "https://h/PUBLIC_DVD_T_202401010000.zip!T.CSV",
            ]
        );
        assert!(c.sources.windows(2).all(|w| w[0].stamp() < w[1].stamp()));
        let paths: Vec<&str> = c.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["c.parquet", "a.parquet", "a2.parquet", "b.parquet"]);
        let a = &c.files[1];
        assert_eq!(
            a.rank_of("https://h/PUBLIC_T_202401020005_1.zip!T2.CSV"),
            Some(2)
        );
        assert_eq!(
            a.rank_of("https://h/PUBLIC_T_202401020005_1.zip!T1.CSV"),
            Some(1)
        );
        assert_eq!(a.rank_of("https://h/other.zip!T.CSV"), None);
        assert_eq!((a.min_us, a.max_us), (Some(2), Some(3)));
        assert_eq!(c.files[0].rank_of("file:///x/T.CSV"), Some(0));
    }
}
