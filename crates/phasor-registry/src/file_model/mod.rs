//! The schema model: each table's columns and its history as an Iceberg
//! table, and the rules a table must satisfy. It knows nothing of Arrow, so
//! `build.rs` compiles it on its own to validate `schemas/`.

pub mod blob;
pub mod doc;
pub mod error;
pub mod file;
pub mod folders;
pub mod model;
pub mod validate;

pub use error::SchemaFileError;
pub use folders::{FOLDERS_FILE, Folder, FolderBlock, Side};
pub use model::{
    Column, ColumnId, LakeType, LoadMode, Market, MarketDef, Run, Table, TableSchema,
    TimestampFormat,
};
