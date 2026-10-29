# Layout: generic crates, formats, markets

phasor builds one lake from many sources. The workspace is cut so that a
new source adds crates and changes none.

## Three groups

The layers and every dependency between crates are in `CONTRIBUTING.md`
("Layout"). The groups:

- **Generic** (`phasor-core`, `phasor-net`, `phasor-kit`, `phasor-acquire`,
  `phasor-iceberg-store`, `phasor-writer`, `phasor-pipeline`, `phasor-cli`,
  `phasor-mirror`, `phasor-py`): what every source shares. The pipeline runs over the
  ports in `phasor-kit` and names no format. `FeedFolders`
  (`crates/phasor-acquire/src/feed_folders.rs`) is a feed's folders as a
  `Source`, for any feed the shared feeds table names. The decoders, the zip
  walker, the Iceberg contract and the gates come from phasor-schemas.
- **Formats and tables** (`phasor-registry`, `phasor-mms-conform`): the table model and
  block resolution, and the MMS, plain CSV, WEMDE and sniffing decoders into
  typed Arrow batches. A format another market also uses is shared, which is
  why these are not named for the NEM.
- **Markets** (`phasor-nem`, `phasor-wem`, `phasor-gas`): a market's sources,
  its settings under `[sources.<kit>]`, its command line, and `kit()`.
  `crates/phasor-cli/src/kits.rs` lists the kits a binary reads with: those
  whose sources know a file by its path first, the NEM last, since it reads a
  file no source claims.

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
  keys are under `phasor.`.

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
