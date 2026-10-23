# Layout: generic crates, formats, markets

utilake builds one lake from many sources. The workspace is cut so that a
new source adds crates and changes none.

## Three groups

- **Generic** (`utilake-core`, `utilake-iceberg`, `utilake-acquire`,
  `utilake-kit`, `utilake-writer`, `utilake-pipeline`, `utilake-query`,
  `utilake-cli`, `utilake-mirror`): what every source shares.
  `utilake-query` reads the lake back: one frame of one table over a
  window, resolved by the roles the registry publishes; it sits beside
  the pipeline under the CLI and reads the manifest's rows only. `utilake-iceberg` is the
  lake's Iceberg contract (types, field ids, legal updates, the interval
  planner); it, the MMS decoder (`utilake-mms-decode`), the zip walker
  (`utilake-zip`) and the repository gates (`utilake-gates`) come from the
  utilake-schemas repository at the
  revision `Cargo.toml` pins, so the lake, its census and its schemas read
  AEMO's files through one scanner and agree on every id. `utilake-mirror`
  is the nemweb mirror (binary `utilake-mirror`): every file under nemweb's
  archive folders and the catalog of it, which the lake and the census
  read from. The pipeline runs over the ports in `utilake-kit` and names no
  format: a `Source` lists and describes files, a `Decoder` reads an entry
  into `Block`s for a `BlockSink`, a `Catalog` declares the tables, and a
  `SourceKit` bundles them for one market. The compiler holds the rule:
  `cargo tree -p utilake-pipeline` names no format or market crate.
- **Formats** (`utilake-mms-*`): how one publication format is read. MMS is
  AEMO's C/I/D rows with SQL*Loader control files; the shared `decode`
  scans them, `registry` holds the table model and resolves a block's
  shape, `conform` casts rows to the lake's types (its value faults and
  sink failures are its own, diagnosed by `utilake-core`) and implements
  the kit's `Decoder` and `Catalog`. A format that another market also uses is shared, which is why
  these are `-mms-`, not `-nem-`.
- **Markets** (`utilake-nem`, `utilake-wem`, later `utilake-gas`): what one
  market brings. Its sources (where the files are and what their names
  say), its settings under `[sources.<kit>]`, its command line, and `kit()`,
  which assembles the market's sources with a format's decoder and catalog.
  `crates/utilake-cli/src/kits.rs` is the one list of kits a binary reads
  with.

## Names

- The product is `utilake`: a lake for the utilities' data, the electricity
  markets first. The earlier name, `nemlake`, named one source.
- A crate is `utilake-<market>` or `utilake-<format>-<stage>`. Every MMS
  file is NEM, but not every NEM file is MMS (FCAS four-second data and the
  registration workbooks are not), so format and market are separate axes.
- What is persisted never changes with a name: manifest source ids
  (`nem-mmsdm`, `wem-datafiles`), the `schemas/<market>` tree, sidecar
  fields, the shape-id hash, and the Parquet footer keys (read under both
  `utilake.` and the earlier `nemlake.` prefix).

## Adding a source

1. A format crate, if the publication format is new: a scanner and a
   `Decoder` that hands typed blocks to the sink. A kit may have several
   decoders; the entry's name picks one.
2. A market crate: a `Source` with `id`, `list`, `archived` and `describe`,
   a config struct read from `[sources.<kit>]`, and `kit()`. Register it in
   `kits.rs`.
3. A coverage floor for each new crate in `coverage.json`, a line in
   `llm/index.md`, and this file.

Shared with the mirror and the catalog repositories, by git dependency, is
`utilake-contract`: the time stamp, the diagnostic, the archive and its
sidecar, AEMO's file names, the lake layout and footer keys, and the public
index. The core re-exports it.
