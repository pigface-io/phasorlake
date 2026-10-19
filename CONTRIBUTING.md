# Contributing

## Commands

Everything runs through `just`; `just --list` shows the recipes. `just setup`
installs the pinned tools and the git hooks once; `just check` runs every hook
of both stages on every file, which is exactly what CI runs.

## Layout

```
Generic, for every source:
crates/utilake-core          L0  vocabulary: calendar and periods, numbers, names, fingerprints, diagnostics, config, home. No I/O.
crates/utilake-acquire       L1  the Source port, HTTP transport with pacing and a breaker, the archive of downloads (async)
crates/utilake-kit           L1  the ports a kit implements (Decoder, BlockSink, Catalog, SourceKit), Transform, the zip walker, quarantine
crates/utilake-writer        L1  Parquet writer, partitioner, SQLite manifest actor, verify
crates/utilake-pipeline      L2  list → fetch → decode → write → commit as tokio streams, over the kit ports only
crates/utilake-cli           L3  the command: flags, rendering, exit codes; kits.rs lists the kits a binary reads with

The MMS format (AEMO's C/I/D rows and SQL*Loader control files):
crates/utilake-mms-decode    F   the I row, MMS timestamps, scanner, field splitter; no registry
crates/utilake-mms-registry  F   the table model (file_model), the shape id, the registry: build.rs validates schemas/, resolve() on the hot path
crates/utilake-mms-conform   F   resolves blocks through the registry, casts to typed Arrow batches; the kit's Decoder and Catalog

The markets:
crates/utilake-nem           M   the NEM kit: nemweb's monthly archive, AEMO's file names, [sources.nem], fetch nem, kit()
crates/utilake-nem-census    M   the NEM kit's schema tool: the census of shapes, and the sync of schemas/ to the archive's control files
crates/utilake-wem           M   the WEM kit: the datafiles source and [sources.wem]; no decoder yet
crates/utilake-mdff          F   NEM12/13 meter data codes, std only, not yet used

Tooling:
crates/utilake-bench             end-to-end benchmark harness
crates/xtask                     the gates
```

Dependencies point down only, and across the three groups in one direction:
the generic crates never depend on a format or a market crate (`cargo tree -p
utilake-pipeline` names no `mms`, `nem` or `wem`); a market crate depends on
the format it reads; the decoder never depends on the registry. Nothing below
the pipeline knows about tokio except the fetcher and the manifest's async
handle. Parsing and Parquet
encoding are synchronous and run on blocking workers, one source file per
worker; the async and sync worlds meet once per file.

## Test tiers

| tier | where | gate |
|---|---|---|
| unit and integration | every crate, `cargo test` | `just test`, pre-commit |
| coverage | `cargo llvm-cov` over the workspace | `just cover` against `coverage.json`, pre-push |
| schema sync | `utilake-nem-census sync --check` over the archived months | `just schema-sync-check`, pre-push |
| census | every recorded block conforms to the increment in force, control-file types fit ours | `just census-check`, pre-push |
| schema compatibility | table files at `origin/main` against the working tree | `just schema-compat`, pre-push |
| benchmarks | criterion plus `utilake-bench` | `just bench-fast` pre-push, `just bench` in CI, against `bench/budgets.json` |
| network | `crates/utilake-nem/tests/net.rs` | `just net-test`, weekly in CI |

## Floors and budgets

A coverage floor in `coverage.json` only goes up. A benchmark budget in
`bench/budgets.json` only tightens: `just bench-tighten` moves budgets to the
latest measurement when that is better, never the other way. `just budget-check`
fails when either moved the wrong way against `origin/main`, unless a commit in
the range explains it with a `coverage-floor:` or `bench-budget:` line.

The registry has the same ratchet. A table file may gain columns and
increments, fix an increment's columns, or make a column nullable. It may
not delete a table, delete, rename or renumber a column, change a type
other than by an Iceberg promotion (none of ours is one), or remove an
increment or change its month or control file. `just schema-compat` checks that against
`origin/main`; a break made on purpose is explained with a `schema-break:`
line in a commit message.

Benchmark categories: `parse` (scanner, typed path, timestamp, float),
`registry` (resolve, load), `store` (Parquet write, partition split) and
`e2e` (wall time, peak heap, allocations and peak RSS of ingesting the
fixture zip). Budgets are taken on an Apple M-series laptop; CI's macOS runner
is the same class.

## Errors

Every failure mode is a `Diagnostic` with a stable code. Add the variant to the
crate's error enum with a full-sentence `Display`, give it a code in its
`Diagnose` impl, and run `just errors-doc`. Messages name the thing that is
wrong and what was expected; context (request, file, entry, line, sample) is
attached as fields, not formatted into the message.

## Style

Comments say why, in full sentences. Errors are full sentences. `cargo fmt`,
clippy with warnings as errors, no `unsafe`. Commits follow Conventional
Commits with an imperative subject and carry no tool attribution.

## Licence

utilake is licensed under the Apache License 2.0 (`LICENSE`). A contribution
you submit for inclusion is under that licence, as its section 5 says, with
no other terms.
