//! What was read, and by which rules: recorded with every lake file.

use serde::{Deserialize, Serialize};

/// How a file's rows came to be: the shape read and the rules it was read with.
/// Recorded in every Parquet footer and every manifest ingest.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Transform {
    /// The block's family (an MMS I row's report type), under its original field name.
    #[serde(rename = "report_type")]
    pub block_family: String,
    /// The block's name in its family (an MMS I row's report subtype), under its original field name.
    #[serde(rename = "report_subtype")]
    pub block_name: String,
    /// Version the source published the block with.
    pub source_version: u16,
    /// Columns of the block's header, in order.
    pub source_columns: Vec<String>,
    /// The source's month, `YYYY-MM`, when known; rules can be bounded by months.
    #[serde(default)]
    pub source_period: Option<String>,
    /// Hash of the conformance plan the shape was read with (`plan:<32 hex digits>`);
    /// empty or another kind of hash on older ingests.
    pub plan_hash: String,
    /// Where the table's definition came from: `registry`, `user`, `file`.
    pub schema_source: String,
    /// How the shape matched the definition: `conforms`, `deviates` or `derived`.
    pub resolution: String,
    /// The census's shape id from its header; older ingests may list several, comma-separated.
    #[serde(default)]
    pub shape_id: Option<String>,
    /// The Iceberg schema of the table's history the block resolved to, if any.
    #[serde(default)]
    pub schema_id: Option<u32>,
}
