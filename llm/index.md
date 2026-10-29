# phasor: index for agents

Layers, pointing down (`layers.json`, drawn in `CONTRIBUTING.md`): cli/mirror → pipeline → {acquire, writer} → kit → {net, core}; nem/wem/gas → {acquire, `crates/phasor-mms-conform`} → `crates/phasor-registry` → kit. The pipeline runs over the kit's ports and names no format; the decoder never sees the registry. The MMS and plain decoders (`phasor-mms-decode`, `phasor-plain-decode`), the Iceberg contract (`phasor-iceberg`), the gates, the census and the histories live in phasor-schemas, pinned by rev in `Cargo.toml`. Async at the edges, sync in the core.
Entries name where it starts.

## Rules a hook or CI enforces

- Run tools through `just`, never with hand-picked flags. `just check` runs every hook, as CI does.
- Every dependency between the workspace's crates is one `layers.json` declares, and every one it declares exists (`just layers`, `crates/xtask/src/layers.rs`).
- `unsafe` is forbidden in every crate, so no mmap: zips are read through `BufReader<File>` (`phasor-zip`).
- The hot path is the decoder's scanner and field splitter (phasor-schemas, `phasor-mms-decode`), `crates/phasor-mms-conform/src/arrow_sink.rs`, `crates/phasor-mms-conform/src/builders.rs`, `crates/phasor-mms-conform/src/mmstime.rs`, `crates/phasor-core/src/num.rs`: no allocation per row, no `chrono`/`jiff`/`regex`, nothing `dyn` or `async` in the loop; `just bench-fast` before and after.
- A coverage floor in `coverage.json` only goes up; a budget in `bench/budgets.json` only tightens. Loosening one needs a `coverage-floor:` or `bench-budget:` commit trailer (`just budget-check`).
- Every failure mode is a `Diagnostic` (`crates/phasor-core/src/diag.rs`) with a stable `category.snake_name` code, a full-sentence message and the layer's context (request, URL, file, entry, table, version, line, sample). Codes are never renamed; `docs/errors.md` is generated (`just errors-doc`).
- Table shapes come only from the registry (`crates/phasor-registry`); nothing else hardcodes a column list. Tables are the Iceberg metadata phasor-schemas publishes, copied per market into `schemas/nem/pulled/` by `just schemas-pull REV`, never edited; `just schemas-check` holds each copy to its manifest and `Cargo.lock`'s rev (`crates/xtask/src/schemas.rs`).
- Parquet is written only through an `IngestSession` (`crates/phasor-writer/src/session.rs`), which alone decides partitions, file rollover and row stamps; a file exists only after rename.
- The manifest schema changes only by a new numbered file in `crates/phasor-writer/src/manifest/migrations/`, never by editing an old one.
- Every HTTP request goes through `Requests` (`crates/phasor-net/src/fetch.rs`): pacing, retries, the refusal breaker, `--offline`. A file downloads whole, once; a zip holding more than asked is read by its directory and only picked entries fetched, kept as `{zip}!{entry}` (`fetch_picks`).
- Every time carries its zone; naive is a defect. Lake: UTC microseconds as `Timestamp(Microsecond, Some(tz))`, `+10:00` NEM, `+08:00` WEM; `LakeWriter` refuses the rest (`store.naive_timestamp`). Records: a time field stays as it came; `{field}_at` is the instant, RFC 3339 with offset (`crates/phasor-core/src/timetext.rs`).
- Fixtures under `testdata/` are AEMO public excerpts of at most 20 data rows each; attribution in `testdata/LICENCE.md`. Never commit a whole archive file.
- Network tests run only with `PHASOR_NET_TESTS=1` (`just net-test`). Unit tests use a temp home.
- Commits follow Conventional Commits (`feat:`, `fix:`, `chore:`) with an imperative subject and no tool attribution.

## Features

- **Iceberg** v3 contract: the `phasor-iceberg` crate of phasor-schemas, at the rev `Cargo.toml` pins. Tables on disk: `crates/phasor-iceberg-store/src/`.
- **Vocabulary**: calendar, periods, numbers, names, home, config, diagnostics. `crates/phasor-core/src/`.
- **Schema model** (`crates/phasor-registry/src/file_model/`): a table's columns, the history's fields by id with Iceberg's exact types, and its history: schemas (fields in file order) and runs (report pair, version, release months) (`crates/phasor-registry/src/file_model/model.rs`). Pulled docs (`crates/phasor-registry/src/file_model/doc.rs`); hand-written TOML tables (`crates/phasor-registry/src/file_model/file.rs`). `[temporal]` roles. Which report folder carries each block: `crates/phasor-registry/src/file_model/folders.rs` (pulled with the tables). Validation, the postcard blob; the shape id (the decoder's `shape`, re-exported by `crates/phasor-registry/src/lib.rs`; raise `SHAPE_FORMAT` when it changes).
- **Registry**: `crates/phasor-registry/build.rs` validates `schemas/` at compile time; a block resolves to the schema `History::resolve` names: `conforms`, `deviates` (no run fits; strict mode refuses) or `derived` (no table claims the pair; every column text). `ConformancePlan` (`crates/phasor-registry/src/plan.rs`). `crates/phasor-registry/src/resolve.rs`, `crates/phasor-registry/src/registry.rs`; `PlanHash` covers only what decides a shape's output (raise `PLAN_FORMAT` when conversion changes). User tables overlay the embedded (`Registry::with_overlay`).
- **Census**: in phasor-schemas; phasor reads what it publishes, pulled per market. Fixtures: `testdata/bootstrap/`.
- **NEM kit** (`crates/phasor-nem/src/`): nemweb's archive as a `Source`, AEMO's file names (the decoder's `filename`, re-exported as `phasor_nem::names`), `[sources.nem]`; `kit()` adds the MMS decoder and catalog. `crates/phasor-cli/src/kits.rs` lists the kits.
- **WEM kit** (`crates/phasor-wem/src/`): the datafiles and WEMDE daily zips as sources, the plain CSV and WEMDE decoders, `[sources.wem]`.
- **Gas kit** (`crates/phasor-gas/src/`): GBB, GSH, STTM, DWGM folders as sources, a CSV read by its first bytes, `[sources.gas]`. A feed's folders as a source: `crates/phasor-acquire/src/feed_folders.rs`; a file no source claims goes to the last kit.
- **Decode** (MMS format): the I row, MMS timestamps, scanner, field splitter; hands each block's I row and its untyped rows to a `Sink`; plain CSV and WEMDE through the same sink, `crates/phasor-mms-conform/src/plain.rs`. **Kit**: the ports only (`Source`, `Listings`, `Decoder`, `Catalog`), `crates/phasor-kit/src/`.
- **Conform**: the typed Arrow sink, resolving each block via the registry, two-pass rows (parse all, then append), the value net (a value that does not fit is null, its text in `{col}__raw`; only a malformed row is quarantined), timestamp clamping; the inspect sink; the kit's decoder in `crates/phasor-mms-conform/src/kit.rs`. Every market's tables as a kit's catalog: `crates/phasor-registry/src/catalog.rs`.
- **Acquire**: the adapters: `Fetcher` (`crates/phasor-acquire/src/fetcher.rs`) over `phasor-net`, `FeedFolders`, the IIS listing parser. **Net**: transports, paced requests and the refusal breaker, bodies streamed to a blocking writer, the archive's helpers (`crates/phasor-net/src/`).
- **Writer**: one source file's write (`crates/phasor-writer/src/session.rs`) over the Hive Parquet writer with atomic rename (`crates/phasor-writer/src/parquet_hive.rs`), month partitioner, quarantine, SQLite manifest and its async actor (`crates/phasor-writer/src/manifest/`).
- **Pipeline**: list → fetch → ingest on blocking workers → commit (`RunOptions::ingest` false: download only; inputs `Archive`, `Listed`, `Local`). `crates/phasor-pipeline/src/get.rs` plans an interval across a kit's sources. Each ingest records its transform in the Parquet footer and manifest; a source whose plan changed is read again (`Input::Stale`). `crates/phasor-pipeline/src/run.rs`, `crates/phasor-pipeline/src/ingest_file.rs`, `crates/phasor-pipeline/src/report.rs`. `crates/phasor-pipeline/src/decode.rs`: the one walk over a file's entries.
- **Command**: `fetch` (downloads and ingests; `--no-ingest`), `import`, `ingest`, `list`, `inspect`, `errors`, `verify`, `schema`, `wem`; shared flag structs and `--help` examples (a test holds them) in `crates/phasor-cli/src/cli.rs`; `crates/phasor-cli/src/commands/`.
- **Gates**: `just cover`, `just bench`, `just bench-fast`, `just budget-check`, `just llm-check`, `just errors-check`, `just schemas-check`. `crates/xtask/src/`, `crates/phasor-bench/src/main.rs`, `bench/budgets.json`, `coverage.json`.
- **Python** (`crates/phasor-py/src/lib.rs`, `python/phasorlake/`): `decode` over `decode_file`, `inspect`, `table`, the binaries as scripts; ruff, mypy, basedpyright, stubtest (`just py-test`).
- **Mirror** (`crates/phasor-mirror/src/`): nemweb's archive folders and any `[[hosts]]` configured (`crates/phasor-mirror/src/hosts.rs`: roots, clock, pace; a fetcher per host, `crates/phasor-mirror/src/net/fetchers.rs`), its records and catalog.

## Documents

- `README.md`: what it is, quick start, layout, types.
- `CONTRIBUTING.md`: commands, layering, tests, budgets.
- `docs/errors.md`: diagnostic codes.
- `docs/layout.md`: crate groups; adding a source.
- `docs/iceberg.md`: the Iceberg V3 contract: types, field ids, updates.
- `docs/semantics.md`: the lake: axes, roles, window, revisions, provenance columns. `docs/reading.md`: reads as SQL, each example run by `crates/phasor-e2e/tests/reading.rs`.
