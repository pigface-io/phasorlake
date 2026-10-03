# Contributing

## Commands

Everything runs through `just`; `just --list` shows the recipes. `just setup`
installs the pinned tools and the git hooks once; `just check` runs every hook
of both stages on every file, which is exactly what CI runs.

## Layout

```
crates/nemlake-core          L0  vocabulary: identifiers, time, numbers, diagnostics. No I/O.
crates/nemlake-schema-model  L1  the table model, TOML files, validation, evolution rules
crates/nemlake-schema        L1  the registry: build.rs validates schemas/, resolve() on the hot path
crates/nemlake-schema-bootstrap  tooling that regenerates schemas/ from testdata/
crates/nemlake-mms           L2  the MMS row format: scanner, typed Arrow sink, zips
crates/nemlake-fetch         L3  sources, HTTP transport, cache (async)
crates/nemlake-store         L3  Parquet writer, partitioner, SQLite manifest actor
crates/nemlake-pipeline      L4  list → fetch → ingest → commit as tokio streams
crates/nemlake-cli           L5  the command: flags, rendering, exit codes
crates/nemlake-bench             end-to-end benchmark harness
crates/xtask                     the gates
```

Dependencies point down only. Nothing below the pipeline knows about tokio
except the fetcher and the manifest's async handle. Parsing and Parquet
encoding are synchronous and run on blocking workers, one source file per
worker; the async and sync worlds meet once per file.

## Test tiers

| tier | where | gate |
|---|---|---|
| unit and integration | every crate, `cargo test` | `just test`, pre-commit |
| coverage | `cargo llvm-cov` over the workspace | `just cover` against `coverage.json`, pre-push |
| schema regeneration | `nemlake-schema-bootstrap regen --check` | `just schema-check`, pre-push |
| schema compatibility | table files at `origin/main` against the working tree | `just schema-compat`, pre-push |
| benchmarks | criterion plus `nemlake-bench` | `just bench-fast` pre-push, `just bench` in CI, against `bench/budgets.json` |
| network | `crates/nemlake-fetch/tests/net.rs` | `just net-test`, weekly in CI |

## Floors and budgets

A coverage floor in `coverage.json` only goes up. A benchmark budget in
`bench/budgets.json` only tightens: `just bench-tighten` moves budgets to the
latest measurement when that is better, never the other way. `just budget-check`
fails when either moved the wrong way against `origin/main`, unless a commit in
the range explains it with a `coverage-floor:` or `bench-budget:` line.

The registry has the same ratchet. A table file may gain columns, versions
and variants, widen a type or make a column nullable. It may not delete a
table, delete or renumber a column, narrow a type, or stop declaring a
column list it declared before. `just schema-compat` checks that against
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

nemlake is licensed under the Apache License 2.0 (`LICENSE`). A contribution
you submit for inclusion is under that licence, as its section 5 says, with
no other terms.
