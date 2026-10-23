//! The lake's files of a table, each with where it came from and when
//! that was published, in one total order that the kernel breaks ties by.

use std::sync::Arc;

use utilake_writer::manifest::LiveOutput;

use crate::stamp::published;
use crate::time::{KnownTime, MarketOffset};

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
    /// The entry of the source file.
    pub entry: Arc<str>,
    /// The month of the source, `YYYY-MM`.
    pub source_period: Option<String>,
    /// When the source was published.
    pub published: Option<KnownTime>,
}

impl LakeFile {
    /// The source as `url!entry`, the `_nl_source` of its rows.
    pub fn source(&self) -> String {
        if self.entry.is_empty() {
            self.url.to_string()
        } else {
            format!("{}!{}", self.url, self.entry)
        }
    }

    /// Where it stands among sources.
    pub fn stamp(&self) -> SourceStamp<'_> {
        SourceStamp {
            published: self.published,
            url: &self.url,
            entry: &self.entry,
        }
    }
}

/// The total order of sources: by publish time (a source without one
/// before every source with one), then URL, then entry. Among copies of
/// one row the greatest stamp wins, whatever order the files were read in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SourceStamp<'a> {
    /// When published.
    pub published: Option<KnownTime>,
    /// The URL.
    pub url: &'a str,
    /// The entry.
    pub entry: &'a str,
}

/// The lake's files from the manifest's live outputs, in stamp order
/// (then by path, so two files of one source keep one order).
pub fn catalog(outputs: &[LiveOutput], off: MarketOffset) -> Vec<LakeFile> {
    let mut files: Vec<LakeFile> = outputs
        .iter()
        .map(|o| LakeFile {
            table: o.table.clone(),
            path: o.path.clone(),
            schema_id: o.schema_id,
            partition: o.partition.clone(),
            min_us: o.min_time.map(|t| t.div_euclid(1_000)),
            max_us: o.max_time.map(|t| t.div_euclid(1_000)),
            url: Arc::from(o.url.as_str()),
            entry: Arc::from(o.entry.as_str()),
            source_period: o.source_period.clone(),
            published: published(&o.url, o.last_modified_at.as_deref(), off),
        })
        .collect();
    files.sort_by(|a, b| a.stamp().cmp(&b.stamp()).then_with(|| a.path.cmp(&b.path)));
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(url: &str, entry: &str, lm: Option<&str>, path: &str) -> LiveOutput {
        LiveOutput {
            table: "T".into(),
            path: path.into(),
            rows: 1,
            bytes: 1,
            schema_id: Some(0),
            source_version: 1,
            source_period: Some("2024-01".into()),
            url: url.into(),
            entry: entry.into(),
            min_time: Some(2_000),
            max_time: Some(3_999),
            partition: "year=2024/month=01".into(),
            last_modified_at: lm.map(Into::into),
        }
    }

    #[test]
    fn files_are_in_stamp_order() {
        let outs = [
            output(
                "https://h/PUBLIC_DVD_T_202401010000.zip",
                "T.CSV",
                Some("2024-02-05T00:00:00Z"),
                "b.parquet",
            ),
            output(
                "https://h/PUBLIC_T_202401020005_1.zip",
                "T.CSV",
                None,
                "a.parquet",
            ),
            output("file:///x/T.CSV", "", None, "c.parquet"),
        ];
        let files = catalog(&outs, MarketOffset(36_000));
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        // No stamp first, then the report of 2 January, then the archive
        // month published in February.
        assert_eq!(paths, ["c.parquet", "a.parquet", "b.parquet"]);
        assert_eq!(files[0].source(), "file:///x/T.CSV");
        assert_eq!(
            files[1].source(),
            "https://h/PUBLIC_T_202401020005_1.zip!T.CSV"
        );
        assert_eq!((files[1].min_us, files[1].max_us), (Some(2), Some(3)));
        assert!(files[0].stamp() < files[1].stamp());
        assert!(files[1].stamp() < files[2].stamp());
    }
}
