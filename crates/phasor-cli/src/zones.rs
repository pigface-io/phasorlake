//! The zones of the `phasor` command, for `zonetrace` (feature `zones`):
//! 800 to 899. The numbers are fixed, so a report from one revision diffs
//! against the next. A command's root is a stretch (the command is
//! asynchronous); what runs on the main thread before it, the context,
//! is a tree of its own.

use zonetrace::Zone;

/// `fetch`.
pub static FETCH: Zone = Zone::new(800, "phasor.fetch");
/// `import`.
pub static IMPORT: Zone = Zone::new(801, "phasor.import");
/// `ingest`.
pub static INGEST: Zone = Zone::new(802, "phasor.ingest");
/// `list`.
pub static LIST: Zone = Zone::new(803, "phasor.list");
/// `verify`.
pub static VERIFY: Zone = Zone::new(804, "phasor.verify");
/// `iceberg`.
pub static ICEBERG: Zone = Zone::new(805, "phasor.iceberg");
/// `inspect`.
pub static INSPECT: Zone = Zone::new(806, "phasor.inspect");
/// `errors`.
pub static ERRORS: Zone = Zone::new(807, "phasor.errors");
/// `schema`.
pub static SCHEMA: Zone = Zone::new(808, "phasor.schema");
/// `wem`.
pub static WEM: Zone = Zone::new(809, "phasor.wem");
/// The context assembled: home, config, registry and kits.
pub static CONTEXT_NEW: Zone = Zone::new(810, "context.new");
/// The kits assembled from the config.
pub static KITS_ASSEMBLE: Zone = Zone::new(811, "kits.assemble");
/// The manifest opened: SQLite and its migrations.
pub static MANIFEST_OPEN: Zone = Zone::new(813, "manifest.open");
/// Every table's Iceberg metadata refreshed after a run.
pub static ICEBERG_REFRESH: Zone = Zone::new(820, "iceberg.refresh");
/// The path of a [`FETCH`] stretch.
pub static FETCH_PATH: [&Zone; 1] = [&FETCH];
/// The path of an [`IMPORT`] stretch.
pub static IMPORT_PATH: [&Zone; 1] = [&IMPORT];
/// The path of an [`INGEST`] stretch.
pub static INGEST_PATH: [&Zone; 1] = [&INGEST];
/// The path of a [`LIST`] stretch.
pub static LIST_PATH: [&Zone; 1] = [&LIST];
/// The path of a [`VERIFY`] stretch.
pub static VERIFY_PATH: [&Zone; 1] = [&VERIFY];
/// The path of an [`ICEBERG`] stretch.
pub static ICEBERG_PATH: [&Zone; 1] = [&ICEBERG];
/// The path of an [`INSPECT`] stretch.
pub static INSPECT_PATH: [&Zone; 1] = [&INSPECT];
/// The path of an [`ERRORS`] stretch.
pub static ERRORS_PATH: [&Zone; 1] = [&ERRORS];
/// The path of a [`SCHEMA`] stretch.
pub static SCHEMA_PATH: [&Zone; 1] = [&SCHEMA];
/// The path of a [`WEM`] stretch.
pub static WEM_PATH: [&Zone; 1] = [&WEM];
/// The path of an [`ICEBERG_REFRESH`] stretch.
pub static ICEBERG_REFRESH_PATH: [&Zone; 1] = [&ICEBERG_REFRESH];

/// The root path of a command's tree.
#[must_use]
pub fn root_path(command: &crate::cli::Command) -> &'static [&'static Zone] {
    use crate::cli::Command;
    match command {
        Command::Fetch(_) => &FETCH_PATH,
        Command::Import(_) => &IMPORT_PATH,
        Command::Ingest(_) => &INGEST_PATH,
        Command::List(_) => &LIST_PATH,
        Command::Verify { .. } => &VERIFY_PATH,
        Command::Iceberg { .. } => &ICEBERG_PATH,
        Command::Inspect { .. } => &INSPECT_PATH,
        Command::Errors { .. } => &ERRORS_PATH,
        Command::Schema { .. } => &SCHEMA_PATH,
        Command::Wem { .. } => &WEM_PATH,
    }
}
