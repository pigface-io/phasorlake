# utilake: index for agents

Layers, pointing down: `crates/utilake-cli` → {`crates/utilake-pipeline`, `crates/utilake-query`} → {`crates/utilake-acquire`, `crates/utilake-kit`, `crates/utilake-writer`}; `crates/utilake-mms-conform` (the MMS kit) → {`crates/utilake-mms-registry`, `crates/utilake-kit`} → `crates/utilake-core` → `utilake-mms-decode`. The pipeline runs over the kit's ports and names no format; the decoder never sees the registry; mdff a leaf. The MMS and plain decoders (`utilake-mms-decode`, `utilake-plain-decode`), the Iceberg contract (`utilake-iceberg`), the gates, the census and the histories live in utilake-schemas, pinned by rev in `Cargo.toml`. Async at the edges, sync in the core.
Entries name where it starts.

## Rules a hook or CI enforces

- Run tools through `just`, never with hand-picked flags. `just check` runs every hook, as CI does.
- `unsafe` is forbidden in every crate, so no mmap: zips are read through `BufReader<File>` (`crates/utilake-kit/src/container.rs`).
- The hot path is the decoder's scanner and field splitter (utilake-schemas, `utilake-mms-decode`), `crates/utilake-mms-conform/src/arrow_sink.rs`, `crates/utilake-mms-conform/src/builders.rs`, `crates/utilake-mms-conform/src/mmstime.rs`, `crates/utilake-core/src/num.rs`: no allocation per row, no `chrono`/`jiff`/`regex`, nothing `dyn` or `async` in the loop; `just bench-fast` before and after.
- A coverage floor in `coverage.json` only goes up; a budget in `bench/budgets.json` only tightens. Loosening one needs a `coverage-floor:` or `bench-budget:` commit trailer (`just budget-check`).
- Every failure mode is a `Diagnostic` (`crates/utilake-core/src/diag.rs`) with a stable `category.snake_name` code, a full-sentence message and the layer's context (request, URL, file, entry, table, version, line, sample). Codes are never renamed; `docs/errors.md` is generated (`just errors-doc`).
- Table shapes come only from the registry (`crates/utilake-mms-registry`); nothing else hardcodes a column list. Tables are the Iceberg metadata utilake-schemas publishes, copied per market into `schemas/nem/pulled/` by `just schemas-pull REV`, never edited; `just schemas-check` holds each copy to its manifest and `Cargo.lock`'s rev (`crates/xtask/src/schemas.rs`).
- Parquet is written only through `LakeWriter` (`crates/utilake-writer/src/writer.rs`); a file exists only after rename, and the manifest row for it is written in the same step.
- The manifest schema changes only by a new numbered file in `crates/utilake-writer/src/manifest/migrations/`, never by editing an old one.
- Every HTTP request goes through `Fetcher` (`crates/utilake-acquire/src/fetcher.rs`): one user agent, pacing, retries, archive, `--offline`. A file downloads whole, once; a zip holding more than asked is read by its directory and only picked entries fetched, kept as `{zip}!{entry}` (`fetch_picks`).
- Every time carries its zone; naive is a defect. Lake: UTC microseconds as `Timestamp(Microsecond, Some(tz))`, `+10:00` NEM, `+08:00` WEM; `LakeWriter` refuses the rest (`store.naive_timestamp`). Records: a time field stays as it came; `{field}_at` is the instant, RFC 3339 with offset (`crates/utilake-core/src/timetext.rs`).
- Fixtures under `testdata/` are AEMO public excerpts of at most 20 data rows each; attribution in `testdata/LICENCE.md`. Never commit a whole archive file.
- Network tests run only with `UTILAKE_NET_TESTS=1` (`just net-test`). Unit tests use a temp home.
- Commits follow Conventional Commits (`feat:`, `fix:`, `chore:`) with an imperative subject and no tool attribution.

## Features

- **Iceberg** v3 contract: the `utilake-iceberg` crate of utilake-schemas, at the rev `Cargo.toml` pins. Tables on disk: `crates/utilake-iceberg-store/src/`.
- **Vocabulary**: calendar, periods, numbers, names, home, config, diagnostics. `crates/utilake-core/src/`.
- **Schema model** (`crates/utilake-mms-registry/src/file_model/`): a table's columns, the history's fields by id with Iceberg's exact types, and its history: schemas (fields in file order) and runs (report pair, version, release months) (`crates/utilake-mms-registry/src/file_model/model.rs`). Pulled docs (`crates/utilake-mms-registry/src/file_model/doc.rs`); hand-written TOML tables (`crates/utilake-mms-registry/src/file_model/file.rs`). `[temporal]` roles. Which report folder carries each block: `crates/utilake-mms-registry/src/file_model/folders.rs` (pulled with the tables). Validation, the postcard blob; the shape id (the decoder's `shape`, re-exported by `crates/utilake-mms-registry/src/lib.rs`; raise `SHAPE_FORMAT` when it changes).
- **Registry**: `crates/utilake-mms-registry/build.rs` validates `schemas/` at compile time; a block resolves to the schema `History::resolve` names: `conforms`, `deviates` (no run fits; strict mode refuses) or `derived` (no table claims the pair; every column text). `ConformancePlan` (`crates/utilake-mms-registry/src/plan.rs`). `crates/utilake-mms-registry/src/resolve.rs`, `crates/utilake-mms-registry/src/registry.rs`; `PlanHash` covers only what decides a shape's output (raise `PLAN_FORMAT` when conversion changes). User tables overlay the embedded (`Registry::with_overlay`).
- **Census**: in utilake-schemas; utilake reads what it publishes, pulled per market. Fixtures: `testdata/bootstrap/`.
- **NEM kit** (`crates/utilake-nem/src/`): nemweb's archive as a `Source`, AEMO's file names (the decoder's `filename`, re-exported as `utilake_nem::names`), `[sources.nem]`; `kit()` adds the MMS decoder and catalog. `crates/utilake-cli/src/kits.rs` lists the kits.
- **WEM kit** (`crates/utilake-wem/src/`): the datafiles and WEMDE daily zips as sources, the plain CSV and WEMDE decoders, `[sources.wem]`.
- **Gas kit** (`crates/utilake-gas/src/`): GBB, GSH, STTM, DWGM folders as sources, a CSV read by its first bytes, `[sources.gas]`. A feed's folders as a source: `crates/utilake-kit/src/feed_folders.rs`; a file no source claims goes to the last kit.
- **Decode** (MMS format): the I row, MMS timestamps, scanner, field splitter; hands each block's I row and its untyped rows to a `Sink`; plain CSV and WEMDE through the same sink, `crates/utilake-mms-conform/src/plain.rs`. **Kit**: the ports and the zip walker, `crates/utilake-kit/src/`.
- **Conform**: the typed Arrow sink, resolving each block via the registry, two-pass rows (parse all, then append), the value net (a value that does not fit is null, its text in `{col}__raw`; only a malformed row is quarantined), timestamp clamping; the inspect sink; the kit's decoder and catalog in `crates/utilake-mms-conform/src/kit.rs`.
- **Acquire**: the `Source` port; paced requests, a refusal (403/429) trips a breaker, bodies streamed as `Bytes` to a blocking writer (`crates/utilake-acquire/src/fetcher.rs`); IIS listing parser, offline and canned transports; the archive mirrors each URL with a sidecar (`crates/utilake-acquire/src/archive.rs`).
- **Writer**: Hive Parquet writer with atomic rename (`crates/utilake-writer/src/parquet_hive.rs`), month partitioner, SQLite manifest and its async actor (`crates/utilake-writer/src/manifest/`).
- **Pipeline**: list → fetch → ingest on blocking workers → commit (`RunOptions::ingest` false: download only; inputs `Archive`, `Listed`, `Local`). `crates/utilake-pipeline/src/get.rs` plans an interval across a kit's sources. Each ingest records its transform in the Parquet footer and manifest; a source whose plan changed is read again (`Input::Stale`). `crates/utilake-pipeline/src/run.rs`, `crates/utilake-pipeline/src/ingest_file.rs`, `crates/utilake-pipeline/src/report.rs`.
- **Query** (`crates/utilake-query/src/`): one frame of a table over a window: `bind` (roles, files), `scan` (by field id), `resolve` (one row per fact, by order then source), `window` (the interval per axis), `output` (parts), `intervals`, `sql` (standard SQL, DuckDB or Postgres source; `just oracle-test`).
- **Command**: `get` (fetch + ingest + one frame per table via the query crate), `fetch`, `import`, `ingest`, `list`, `inspect`, `errors`, `verify`, `schema`, `wem`; shared flag structs and `--help` examples (a test holds them) in `crates/utilake-cli/src/cli.rs`; `crates/utilake-cli/src/commands/`.
- **MDFF**: NEM12/13 codes, units; rules in `reference/mdff/`. `crates/utilake-mdff/src/`.
- **Gates**: `just cover`, `just bench`, `just bench-fast`, `just budget-check`, `just llm-check`, `just errors-check`, `just schemas-check`. `crates/xtask/src/`, `crates/utilake-bench/src/main.rs`, `bench/budgets.json`, `coverage.json`.
- **Mirror** (`crates/utilake-mirror/src/`): nemweb's archive folders and any `[[hosts]]` configured (`crates/utilake-mirror/src/hosts.rs`: roots, clock, pace; a fetcher per host, `crates/utilake-mirror/src/net/fetchers.rs`), and its catalog.

## Documents

- `README.md`: what it is, quick start, layout, types.
- `CONTRIBUTING.md`: commands, layering, tests, budgets.
- `docs/errors.md`: diagnostic codes.
- `docs/layout.md`: crate groups; adding a source.
- `docs/iceberg.md`: the Iceberg V3 contract: types, field ids, updates.
- `docs/semantics.md`: a read: axes, roles, window, resolutions.
