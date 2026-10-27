//! What was read, and by which rules: recorded with every lake file.

use serde::{Deserialize, Serialize};

/// How the rows of a file came to be: the shape that was read and the rules
/// it was read with. Recorded in the footer of every Parquet file and with
/// every ingest in the manifest, so the path from an archived source to a
/// lake file can be read back and checked.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Transform {
    /// The family the source puts the block in: the report type of an MMS
    /// I row. Recorded under the name the first files were written with.
    #[serde(rename = "report_type")]
    pub block_family: String,
    /// The block's name within its family: the report subtype of an MMS I
    /// row. Recorded under the name the first files were written with.
    #[serde(rename = "report_subtype")]
    pub block_name: String,
    /// Version the source published the block with.
    pub source_version: u16,
    /// Columns of the block's header, in order.
    pub source_columns: Vec<String>,
    /// The month of the source, `YYYY-MM`, when it is known. Rules can be
    /// bounded by months, so it is part of what was read by which rules.
    #[serde(default)]
    pub source_period: Option<String>,
    /// Hash of the conformance plan the shape was read with
    /// (`plan:<32 hex digits>`). Empty, or an older kind of hash, for an
    /// ingest recorded before plans were.
    pub plan_hash: String,
    /// Where the table's definition came from: `registry`, `user`, `file`.
    pub schema_source: String,
    /// How the shape matched the definition: `conforms`, `deviates` or
    /// `derived` (older ingests recorded other words).
    pub resolution: String,
    /// The census's id of the shape, from its header (an MMS block's
    /// report pair and columns); older ingests may list several,
    /// comma-separated.
    #[serde(default)]
    pub shape_id: Option<String>,
    /// The Iceberg schema of the table's history the block resolved to;
    /// none when no run of the history fits it.
    #[serde(default)]
    pub schema_id: Option<u32>,
}
