# utilake: index for agents

Layers, pointing down: `crates/utilake-cli` → `crates/utilake-pipeline` → {`crates/utilake-acquire`, `crates/utilake-kit`, `crates/utilake-writer`}; `crates/utilake-mms-conform` (the MMS kit) → {`crates/utilake-mms-registry`, `crates/utilake-kit`} → `crates/utilake-core` → `utilake-mms-decode`. The pipeline runs over the kit's ports and names no format; the decoder never sees the registry; mdff a leaf. The MMS decoder (`utilake-mms-decode`: scanner, fields, I rows, names, fingerprints, shape ids, archive file names, the month), the Iceberg contract (`utilake-iceberg`), the gates (`utilake-gates`), the census and the NEM histories live in the utilake-schemas repository, pinned by rev in `Cargo.toml`. Async at the edges, sync in the core.
Entries name where it starts.

## Rules a hook or CI enforces

- Run tools through `just`, never with hand-picked flags. `just check` runs every hook, as CI does.
- `unsafe` is forbidden in every crate, so no mmap: zips are read through `BufReader<File>` (`crates/utilake-kit/src/container.rs`).
- The hot path is the decoder's scanner and field splitter (utilake-schemas, `utilake-mms-decode`), `crates/utilake-mms-conform/src/arrow_sink.rs`, `crates/utilake-mms-conform/src/builders.rs`, `crates/utilake-mms-conform/src/mmstime.rs`, `crates/utilake-core/src/num.rs`: no allocation per row, no `chrono`/`jiff`/`regex`, nothing `dyn` or `async` in the loop; `just bench-fast` before and after.
- A coverage floor in `coverage.json` only goes up; a budget in `bench/budgets.json` only tightens. Loosening one needs a `coverage-floor:` or `bench-budget:` commit trailer (`just budget-check`).
- Every failure mode is a `Diagnostic` (`crates/utilake-core/src/diag.rs`) with a stable `category.snake_name` code, a full-sentence message and the layer's context (request, URL, file, entry, table, version, line, sample). Codes are never renamed; `docs/errors.md` is generated (`just errors-doc`).
- Table shapes come only from the registry (`crates/utilake-mms-registry`); nothing else hardcodes a column list. NEM tables are the Iceberg histories utilake-schemas publishes, copied into `schemas/nem/pulled/` by `just schemas-pull REV` and never edited: `just schemas-check` holds the copy to its manifest and its `SOURCE` to the rev `Cargo.lock` takes `utilake-iceberg` from (`crates/xtask/src/schemas.rs`).
- Parquet is written only through `LakeWriter` (`crates/utilake-writer/src/writer.rs`); a file exists only after rename, and the manifest row for it is written in the same step.
- The manifest schema changes only by a new numbered file in `crates/utilake-writer/src/manifest/migrations/`, never by editing an old one.
- Every HTTP request goes through `Fetcher` (`crates/utilake-acquire/src/fetcher.rs`): one user agent, pacing, retries, the archive, `--offline`. Files download whole, once; no cache, no ranged reads.
- Every time carries its zone; naive is a defect. Lake: UTC nanoseconds as `Timestamp(Nanosecond, Some(tz))`, `+10:00` NEM, `+08:00` WEM; `LakeWriter` refuses the rest (`store.naive_timestamp`). Records: a time field stays as it came; `{field}_at` is the instant, RFC 3339 with offset (`crates/utilake-core/src/timetext.rs`).
- Fixtures under `testdata/` are AEMO public excerpts of at most 20 data rows each; attribution in `testdata/LICENCE.md`. Never commit a whole archive file.
- Network tests run only with `UTILAKE_NET_TESTS=1` (`just net-test`). Unit tests use a temp home.
- Commits follow Conventional Commits (`feat:`, `fix:`, `chore:`) with an imperative subject and no tool attribution.

## Features

- **Iceberg** v3 contract: the `utilake-iceberg` crate of utilake-schemas, at the rev `Cargo.toml` pins. Tables on disk: `crates/utilake-iceberg-store/src/`.
- **Vocabulary**: calendar, periods, numbers, names, fingerprints, home layout, config, diagnostics, aggregation. `crates/utilake-core/src/`.
- **Schema model** (`crates/utilake-mms-registry/src/file_model/`): a table's columns, the history's fields by id with exact types (`string`, `double`, `long`, `boolean`, `date`, `timestamp` (µs), `decimal(P,S)`, one to one with Iceberg's), and its history: schemas (fields in file order) and runs (report pair, version, release months) (`crates/utilake-mms-registry/src/file_model/model.rs`). A pulled table document (`crates/utilake-mms-registry/src/file_model/doc.rs`); a user's hand-written TOML table with `[[schema]]` lists (`crates/utilake-mms-registry/src/file_model/file.rs`). Derived time roles (`[temporal]`). Validation, the postcard blob; the shape id: pair and names, the census's bytes (the decoder's `shape`, re-exported by `crates/utilake-mms-registry/src/lib.rs`; raise `SHAPE_FORMAT` there when it changes).
- **Registry**: `crates/utilake-mms-registry/build.rs` validates `schemas/` at compile time; a block resolves to the schema `History::resolve` names for its pair, version, columns and source month: `conforms`, `deviates` (no run fits; strict mode refuses; a column the table lacks is kept as text) or `derived` (no table claims the pair; every column text). The result is a `ConformancePlan` (`crates/utilake-mms-registry/src/plan.rs`); `utilake schema plan` prints one. `crates/utilake-mms-registry/src/resolve.rs`, `crates/utilake-mms-registry/src/registry.rs`. `PlanHash` covers only what decides a shape's output, the schema included (raise `PLAN_FORMAT` when conversion changes). User tables under the home overlay the embedded ones (`Registry::with_overlay`).
- **Census**: the census, its fold into each table's Iceberg history and the export live in the utilake-schemas repository (github.com/g0itb4/utilake-schemas); utilake reads the histories it publishes (`schemas/nem/pulled/`) with its `utilake-iceberg` crate, at the rev `Cargo.toml` pins. Fixtures: `testdata/bootstrap/`.
- **NEM kit** (`crates/utilake-nem/src/`): nemweb's archive as a `Source`, AEMO's file names (the decoder's `filename`, re-exported as `utilake_nem::names`), `[sources.nem]`, `fetch nem`; `kit()` adds the MMS decoder and catalog. `crates/utilake-cli/src/kits.rs` lists the kits.
- **WEM kit** (`crates/utilake-wem/src/`): the datafiles manifests as a `Source`, `[sources.wem]`; no decoder yet (`config.no_decoder`).
- **Decode** (MMS format): the I row, MMS timestamps, scanner, field splitter; hands each block's I row and its untyped rows to a `Sink`. the decoder (`utilake-mms-decode`, in utilake-schemas). **Kit**: the ports and the zip walker, `crates/utilake-kit/src/`.
- **Conform**: the typed Arrow sink, resolving each block via the registry, two-pass rows (parse all, then append), the value net (a value that does not fit is null, its text in `{col}__raw`; only a malformed row is quarantined), timestamp clamping; the inspect sink; the kit's decoder and catalog in `crates/utilake-mms-conform/src/kit.rs`.
- **Acquire**: the `Source` port; paced requests, a refusal (403/429) trips a breaker, bodies streamed as `Bytes` to a blocking writer (`crates/utilake-acquire/src/fetcher.rs`); IIS listing parser, offline and canned transports; the archive mirrors each URL with a sidecar (`crates/utilake-acquire/src/archive.rs`). The NEM archive's month layouts are the kit's (`crates/utilake-nem/src/sources/mmsdm.rs`).
- **Mirror**: nemweb-mirror fills the archive.
- **Writer**: Hive Parquet writer with atomic rename (`crates/utilake-writer/src/parquet_hive.rs`), month partitioner, SQLite manifest and its async actor (`crates/utilake-writer/src/manifest/`).
- **Pipeline**: list → fetch → ingest on blocking workers → commit (`RunOptions::ingest` false: download only; `Input::Archive`: archived months). A batch with columns its open file lacks (`{col}__raw`, a kept null) continues the partition in a further part file. Each ingest records its transform (source shape, plan hash, census shape id, schema source) in the Parquet footer and manifest; a source whose plan changed is read again (`plans_current`; `Input::Stale` lists them). `crates/utilake-pipeline/src/run.rs`, `crates/utilake-pipeline/src/ingest_file.rs`, `crates/utilake-pipeline/src/report.rs`.
- **Command**: `fetch nem` (download only), `ingest` (archived months or local files), `reingest`, `inspect`, `ls`, `errors`, `verify`, `schema`, `archive`, `wem`. `crates/utilake-cli/src/cli.rs`, `crates/utilake-cli/src/commands/`.
- **MDFF**: NEM12/13 codes, units; rules in `reference/mdff/`. `crates/utilake-mdff/src/`.
- **Gates**: `just cover`, `just bench`, `just bench-fast`, `just budget-check`, `just llm-check`, `just errors-check`, `just schemas-check`. `crates/xtask/src/`, `crates/utilake-bench/src/main.rs`, `bench/budgets.json`, `coverage.json`.

## Documents

- `README.md`: what it is, quick start, layout, types.
- `CONTRIBUTING.md`: commands, layering, tests, budgets.
- `docs/errors.md`: diagnostic codes.
- `docs/layout.md`: crate groups; adding a source.
- `docs/iceberg.md`: the Iceberg V3 contract: types, field ids, updates.
