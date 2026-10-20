# utilake: index for agents

Layers, pointing down: `crates/utilake-cli` → `crates/utilake-pipeline` → {`crates/utilake-acquire`, `crates/utilake-kit`, `crates/utilake-writer`}; `crates/utilake-mms-conform` (the MMS kit) → {`crates/utilake-mms-decode`, `crates/utilake-mms-registry`, `crates/utilake-kit`} → `crates/utilake-core`. The pipeline runs over the kit's ports and names no format; decode never sees the registry; census is tooling, mdff a leaf. Async at the edges, sync in the core.
Each entry names where a thing starts.

## Rules a hook or CI enforces

- Run tools through `just`, never with hand-picked flags. `just check` runs every hook, as CI does.
- `unsafe` is forbidden in every crate, so no mmap: zips are read through `BufReader<File>` (`crates/utilake-kit/src/container.rs`).
- The hot path is `crates/utilake-mms-decode/src/scan.rs`, `crates/utilake-mms-decode/src/fields.rs`, `crates/utilake-mms-conform/src/arrow_sink.rs`, `crates/utilake-mms-conform/src/builders.rs`, `crates/utilake-mms-decode/src/mmstime.rs`, `crates/utilake-core/src/num.rs`: no allocation per row, no `chrono`/`jiff`/`regex`, nothing `dyn` or `async` in the loop; `just bench-fast` before and after.
- A coverage floor in `coverage.json` only goes up; a budget in `bench/budgets.json` only tightens. Loosening one needs a `coverage-floor:` or `bench-budget:` commit trailer (`just budget-check`).
- Every failure mode is a `Diagnostic` (`crates/utilake-core/src/diag.rs`) with a stable `category.snake_name` code, a full-sentence message and the layer's context (request, URL, file, entry, table, version, line, sample). Codes are never renamed; `docs/errors.md` is generated (`just errors-doc`).
- Table shapes come only from the registry (`crates/utilake-mms-registry`); nothing else hardcodes a column list. Each table is a directory (`schemas/nem/DISPATCH_UNIT_SCADA/`): our hand-editable table file and AEMO's control files kept byte for byte beside it (hash checked at build time). Tools only append increments and columns, never rewriting an increment, column id or type.
- Parquet is written only through `LakeWriter` (`crates/utilake-writer/src/writer.rs`); a file exists only after rename, and the manifest row for it is written in the same step.
- The manifest schema changes only by a new numbered file in `crates/utilake-writer/src/manifest/migrations/`, never by editing an old one.
- Every HTTP request goes through `Fetcher` (`crates/utilake-acquire/src/fetcher.rs`): one user agent, pacing, retries, the archive, `--offline`. Files download whole, once; no cache, no ranged reads.
- Every time carries its zone; naive is a defect. Lake: UTC nanoseconds as `Timestamp(Nanosecond, Some(tz))`, `+10:00` NEM, `+08:00` WEM; `LakeWriter` refuses the rest (`store.naive_timestamp`). Records: a time field stays as it came; `{field}_at` is the instant, RFC 3339 with offset (`crates/utilake-core/src/timetext.rs`).
- Fixtures under `testdata/` are AEMO public excerpts of at most 20 data rows each; attribution in `testdata/LICENCE.md`. Never commit a whole archive file.
- Network tests run only with `UTILAKE_NET_TESTS=1` (`just net-test`). Unit tests use a temp home.
- Commits follow Conventional Commits (`feat:`, `fix:`, `chore:`) with an imperative subject and no tool attribution.

## Features

- **Iceberg** v3 contract: `crates/utilake-iceberg/src/`.
- **Vocabulary**: calendar, periods, numbers, names, fingerprints, home layout, config, diagnostics, aggregation. `crates/utilake-core/src/`.
- **Schema model** (`crates/utilake-mms-registry/src/file_model/`): a table's columns with exact types (`string`, `double`, `long`, `boolean`, `date`, `timestamp_ns`) and stable ids (Parquet field ids), and increments `TABLE@n`: the file shape an AEMO control file sets from a month on, with its CTL and the I-row versions seen (`crates/utilake-mms-registry/src/file_model/model.rs`). A renamed column is a new column. Derived time roles (`[temporal]`). Validation, breaking changes (only Iceberg promotions), the postcard blob, the control-file parser (`crates/utilake-mms-registry/src/file_model/ctl.rs`); the shape id (`crates/utilake-mms-registry/src/shape.rs`; raise `SHAPE_FORMAT` when it changes).
- **Registry**: `crates/utilake-mms-registry/build.rs` validates `schemas/` at compile time; a block of month M resolves against the increment in force at M: `conforms`, `deviates` (strict mode refuses; a column the table lacks is kept as text) or `derived` (no table claims the pair; every column text). The result is a `ConformancePlan` (`crates/utilake-mms-registry/src/plan.rs`): routes, casts, key and time column; `utilake schema plan` prints one. `crates/utilake-mms-registry/src/resolve.rs`, `crates/utilake-mms-registry/src/registry.rs`, `crates/utilake-mms-registry/src/arrow.rs`. `PlanHash` covers only what decides a shape's output, the increment included (raise `PLAN_FORMAT` when conversion changes). User tables under the home overlay the embedded ones (`Registry::with_overlay`).
- **Census**: `just census` records each block (shape, rows) of a month's archived files in `schemas/nem/census/` and `schemas/nem/shapes/` (`crates/utilake-nem-census/src/census.rs`); `just schema-sync` appends an increment where a month's CTL lays a table out anew or starts a missing table (`crates/utilake-nem-census/src/sync.rs`); `just schema-sync-check` gates it. `just schema-fixtures` cuts a month's CTLs, README and heads (`crates/utilake-nem-census/src/fixtures.rs`); `just schema-datamodel` reads AEMO's data model (`crates/utilake-nem-census/src/dmreport.rs`); `just census-models` records the archive's create scripts (2022-10 on) in `schemas/nem/models/` (`crates/utilake-nem-census/src/models.rs`); `just schema-export` writes JSON Schema per table and shape into a `nem-schemas` checkout, `just schema-export-check` gates it (`crates/utilake-nem-census/src/export/`). Standing data is AEMO's README list plus `crates/utilake-nem-census/snapshot_tables.toml` (`crates/utilake-nem-census/src/standing.rs`). Fixtures: `testdata/bootstrap/`.
- **NEM kit** (`crates/utilake-nem/src/`): nemweb's archive as a `Source`, AEMO's file names (`crates/utilake-nem/src/names.rs`), `[sources.nem]`, `fetch nem`; `kit()` adds the MMS decoder and catalog. `crates/utilake-cli/src/kits.rs` lists the kits.
- **WEM kit** (`crates/utilake-wem/src/`): the datafiles manifests as a `Source`, `[sources.wem]`; no decoder yet (`config.no_decoder`).
- **Decode** (MMS format): the I row, MMS timestamps, scanner, field splitter; hands each block's I row and its untyped rows to a `Sink`. `crates/utilake-mms-decode/src/`. **Kit**: the ports and the zip walker, `crates/utilake-kit/src/`.
- **Conform**: the typed Arrow sink, resolving each block via the registry, two-pass rows (parse all, then append), the value net (a value that does not fit is null, its text in `{col}__raw`; only a malformed row is quarantined), timestamp clamping; the inspect sink; the kit's decoder and catalog in `crates/utilake-mms-conform/src/kit.rs`.
- **Acquire**: the `Source` port; paced requests, a refusal (403/429) trips a breaker, bodies streamed as `Bytes` to a blocking writer (`crates/utilake-acquire/src/fetcher.rs`); IIS listing parser, offline and canned transports; the archive mirrors each URL with a sidecar (`crates/utilake-acquire/src/archive.rs`). The NEM archive's month layouts are the kit's (`crates/utilake-nem/src/sources/mmsdm.rs`).
- **Mirror**: nemweb-mirror (own repo) fills the archive.
- **Writer**: Hive Parquet writer with atomic rename (`crates/utilake-writer/src/parquet_hive.rs`), month partitioner, SQLite manifest and its async actor (`crates/utilake-writer/src/manifest/`).
- **Pipeline**: list → fetch → ingest on blocking workers → commit (`RunOptions::ingest` false: download only; `Input::Archive`: archived months). A batch with columns its open file lacks (`{col}__raw`, a kept null) continues the partition in a further part file. Each ingest records its transform (source shape, plan hash, census shape id, schema source) in the Parquet footer and manifest; a source whose plan changed is read again (`plans_current`; `Input::Stale` lists them). `crates/utilake-pipeline/src/run.rs`, `crates/utilake-pipeline/src/ingest_file.rs`, `crates/utilake-pipeline/src/report.rs`.
- **Command**: `fetch nem` (download only), `ingest` (archived months or local files), `reingest`, `inspect`, `ls`, `errors`, `verify`, `schema`, `archive`, `wem`. `crates/utilake-cli/src/cli.rs`, `crates/utilake-cli/src/commands/`.
- **MDFF**: NEM12/13 codes, units; rules in `reference/mdff/`. `crates/utilake-mdff/src/`.
- **Gates**: `just cover`, `just bench`, `just bench-fast`, `just budget-check`, `just llm-check`, `just errors-check`, `just census-check` (`crates/xtask/src/census_check.rs`). `crates/xtask/src/`, `crates/utilake-bench/src/main.rs`, `bench/budgets.json`, `coverage.json`.

## Documents

- `README.md`: what it is, quick start, layout, types.
- `CONTRIBUTING.md`: commands, layering, tests, budgets.
- `docs/errors.md`: every diagnostic code.
- `docs/layout.md`: the crate groups; adding a source.
- `docs/iceberg.md`: the Iceberg V3 contract: types, field ids, updates.
