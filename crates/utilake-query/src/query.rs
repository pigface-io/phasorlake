//! What a read asks for, before and after it is bound to a table.
//!
//! A [`Query`] names a table, a window on its valid axis, a resolution and
//! what to return. [`crate::bind()`] proves it against the registry and the
//! lake's files and gives a [`Bound`]: the target schema, every role as a
//! position in it, and the projection of each file onto it. Only a
//! `Bound` can be read, so a read never looks a column up by name or
//! guesses a type:
//!
//! ```compile_fail
//! use utilake_query::{Query, Resolution, ValidWindow, ValidTime};
//! let q = Query::new("DISPATCHPRICE", ValidWindow::At(ValidTime::new(0)), Resolution::Raw);
//! let lake: utilake_query::Lake<'_> = unimplemented!();
//! let _ = utilake_query::read(&q, &lake);
//! ```

use arrow_schema::SchemaRef;
use utilake_core::Diagnostic;
use utilake_iceberg::{Column, Target};
use utilake_mms_registry::file_model::ValidAxis;

use crate::time::{KnownTime, MarketOffset, ValidWindow};

/// Which rows of a fact a read returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Resolution {
    /// One row per fact: the newest revision, as known at `known_at`
    /// (`None`: as known now).
    Latest {
        /// Keep only what was known by then.
        known_at: Option<KnownTime>,
    },
    /// Every revision of every fact, each once.
    Revisions,
    /// Every row of every file, as written.
    Raw,
}

impl Resolution {
    /// The name in output metadata.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Latest { .. } => "latest",
            Self::Revisions => "revisions",
            Self::Raw => "raw",
        }
    }
}

/// Which columns a read returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Projection {
    /// Every column of the target schema.
    All,
    /// These, in this order.
    Columns(Vec<String>),
}

/// Keep the rows whose column is one of the values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    /// The column.
    pub column: String,
    /// The values, as text in the column's type.
    pub values: Vec<String>,
}

/// A read, as asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    /// The table.
    pub table: String,
    /// The rows on the valid axis.
    pub window: ValidWindow,
    /// Which rows of each fact.
    pub resolution: Resolution,
    /// The schema to read as.
    pub target: Target,
    /// The columns to return.
    pub columns: Projection,
    /// Rows to keep.
    pub filters: Vec<Filter>,
}

impl Query {
    /// A read of every column of the current schema, no filters.
    pub fn new(table: impl Into<String>, window: ValidWindow, resolution: Resolution) -> Self {
        Self {
            table: table.into(),
            window,
            resolution,
            target: Target::Latest,
            columns: Projection::All,
            filters: Vec::new(),
        }
    }

    /// Reads as this schema.
    #[must_use]
    pub fn with_target(mut self, target: Target) -> Self {
        self.target = target;
        self
    }

    /// Returns these columns.
    #[must_use]
    pub fn with_columns(mut self, columns: Vec<String>) -> Self {
        self.columns = if columns.is_empty() {
            Projection::All
        } else {
            Projection::Columns(columns)
        };
        self
    }

    /// Keeps rows that pass every filter.
    #[must_use]
    pub fn with_filters(mut self, filters: Vec<Filter>) -> Self {
        self.filters = filters;
        self
    }
}

/// A column's position in the target schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FieldIx(pub usize);

/// What the kernel resolves at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Unit {
    /// One partition at a time: the key holds the valid time, which the
    /// partition is of, so a fact never spans two.
    Partition,
    /// The whole table: a fact may be in any file.
    Table,
}

/// A filter bound to a column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundFilter {
    /// The column.
    pub column: FieldIx,
    /// The values, as given.
    pub values: Vec<String>,
}

/// The roles of the table, as positions in the target schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundSpec {
    /// What a fact is about.
    pub entity: Vec<FieldIx>,
    /// When it applies.
    pub valid: ValidAxis<FieldIx>,
    /// Which revision is newer, most significant first.
    pub order: Vec<FieldIx>,
    /// The time a revision was known; none: its source's publish time.
    pub cutoff: Option<FieldIx>,
    /// What the kernel resolves at once.
    pub unit: Unit,
    /// A table with no key and no valid time: only rows equal in every
    /// column are one fact.
    pub collapse_all: bool,
}

/// One file of the lake, projected onto the target schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRead {
    /// Its position in [`crate::Lake::files`].
    pub file: usize,
    /// The target's columns, each read from the file, cast, or null.
    pub columns: Vec<Column>,
}

/// A read proven against a table and the lake's files.
#[derive(Debug, Clone)]
pub struct Bound {
    /// As asked.
    pub query: Query,
    /// The table's name as the registry has it.
    pub table: String,
    /// The schema read as; none for a table without a history.
    pub schema_id: Option<u32>,
    /// The target schema: the columns a frame has before the `_nl_` ones.
    pub schema: SchemaRef,
    /// The roles; none for a table without a history (read raw only).
    pub spec: Option<BoundSpec>,
    /// The column a raw read of a table without roles bounds on.
    pub time: Option<FieldIx>,
    /// The files whose data may fall in the window, in source order.
    pub files: Vec<FileRead>,
    /// Filters on entity columns: applied before resolution.
    pub entity_filters: Vec<BoundFilter>,
    /// Every other filter: applied after.
    pub value_filters: Vec<BoundFilter>,
    /// The target columns to return, in order; none: all.
    pub keep: Option<Vec<FieldIx>>,
    /// The market's clock.
    pub offset: MarketOffset,
    /// What binding noted.
    pub warnings: Vec<Diagnostic>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::ValidTime;

    #[test]
    fn a_query_is_built_up() {
        let w = ValidWindow::At(ValidTime::new(0));
        let q = Query::new("T", w, Resolution::Raw);
        assert_eq!(q.target, Target::Latest);
        assert_eq!(q.columns, Projection::All);
        let q = q
            .with_target(Target::AsOf(2))
            .with_columns(vec!["A".into()])
            .with_filters(vec![Filter {
                column: "A".into(),
                values: vec!["x".into()],
            }]);
        assert_eq!(q.target, Target::AsOf(2));
        assert_eq!(q.columns, Projection::Columns(vec!["A".into()]));
        assert_eq!(q.filters.len(), 1);
        assert_eq!(q.with_columns(vec![]).columns, Projection::All);
        assert_eq!(Resolution::Raw.name(), "raw");
        assert_eq!(Resolution::Revisions.name(), "revisions");
        assert_eq!(Resolution::Latest { known_at: None }.name(), "latest");
    }
}
