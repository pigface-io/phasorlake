# Layout: generic crates, formats, markets

phasor builds one lake from many sources. The workspace is cut so that a
new source adds crates and changes none.

## Three groups

- **Generic** (`phasor-core`, `phasor-iceberg`, `phasor-acquire`,
  `phasor-kit`, `phasor-writer`, `phasor-pipeline`, `phasor-cli`,
  `phasor-mirror`): what every source shares. Nothing reads the lake
  back: that is the reader's own SQL. `phasor-iceberg` is the
  lake's Iceberg contract (types, field ids, legal updates, the interval
  planner); it, the MMS decoder (`phasor-mms-decode`), the plain decoder
  (`phasor-plain-decode`: header-row CSV and WEMDE JSON, the feeds table
  and the block-naming rules), the zip walker (`phasor-zip`) and the
  repository gates (`phasor-gates`) come from the phasor-schemas
  repository at the revision `Cargo.toml` pins, so the lake, its census and
  its schemas read AEMO's files through one scanner and agree on every id.
  `phasor-mirror` is the mirror (binary `phasor-mirror`): every file
  under nemweb's archive folders, and under any other host its config
  names (`data.wa.aemo.com.au`), and the catalog of it, which the lake and
  the census read from. The pipeline runs over the ports in `phasor-kit`
  and names no format: a `Source` lists and describes files, a `Decoder`
  reads an entry into `Block`s for a `BlockSink`, a `Catalog` declares the
  tables, and a `SourceKit` bundles them for one market; `FeedFolders`
  there is a feed's folders as a `Source`, for any feed the shared table
  names. The compiler holds the rule: `cargo tree -p phasor-pipeline`
  names no format or market crate.
- **Formats** (`phasor-mms-*`): how one publication format is read. MMS is
  AEMO's C/I/D rows with SQL*Loader control files; the shared `decode`
  scans them, `registry` holds the table model and resolves a block's
  shape, `conform` casts rows to the lake's types (its value faults and
  sink failures are its own, diagnosed by `phasor-core`) and implements
  the kit's `Decoder` and `Catalog`: the MMS decoder, and in `plain.rs`
  the header-row CSV decoder, the WEMDE decoder and the sniffing decoder
  that sends a `.csv` to one or the other by its first bytes, all through
  the one Arrow sink. A format that another market also uses is shared,
  which is why these are `-mms-`, not `-nem-`.
- **Markets** (`phasor-nem`, `phasor-wem`, `phasor-gas`): what one
  market brings. Its sources (where the files are and what their names
  say), its settings under `[sources.<kit>]`, its command line, and `kit()`,
  which assembles the market's sources with a format's decoder and catalog.
  `crates/phasor-cli/src/kits.rs` is the one list of kits a binary reads
  with: the kits whose sources know a file by its path first, the NEM last,
  since it is the kit a file no source claims is read with.

## Names

- The product is `phasor` (its repositories are `phasorlake`,
  `phasorlake-schemas` and `phasorlake-datasets`): a lake for the
  utilities' data, the electricity markets first.
- A crate is `phasor-<market>` or `phasor-<format>-<stage>`. Every MMS
  file is NEM, but not every NEM file is MMS (FCAS four-second data and the
  registration workbooks are not), so format and market are separate axes.
- What is persisted does not follow a crate's name: manifest source ids
  (`nem-mmsdm`, `nem-reports`, `gas-gbb`, `gas-gsh`, `gas-sttm`,
  `gas-dwgm`, `wem-datafiles`, `wem-wemde`), the `schemas/<market>` tree,
  sidecar fields and the shape-id hash. The Parquet footer and Iceberg
  keys are under `phasor.`; a lake written under an earlier name is not
  read, and is ingested again from the archive.

## Adding a source

1. A format crate, if the publication format is new: a scanner and a
   `Decoder` that hands typed blocks to the sink. A kit may have several
   decoders; the entry's name picks one.
2. A market crate: a `Source` with `id`, `list`, `archived` and `describe`,
   a config struct read from `[sources.<kit>]`, and `kit()`. Register it in
   `kits.rs`.
3. A coverage floor for each new crate in `coverage.json`, a line in
   `llm/index.md`, and this file.

Shared with the census and the explorer, by git dependency on
phasor-schemas, are the decoders, the Iceberg contract and the feeds
table; the lake adds nothing to a format or a feed that the census does not
read the same way.
