# Contributing

## Commands

Everything runs through `just`; `just --list` shows the recipes. `just setup`
installs the pinned tools and the git hooks once; `just check` runs every hook
of both stages on every file, which is exactly what CI runs.

## Layout

Each crate has one job. `layers.json` declares every dependency between
them and `just layers` holds the code to it: an edge it does not list fails,
and so does a listed edge that is gone. Dependencies point down only:

```
phasor-schemas (pinned by rev): mms-decode, plain-decode, provenance, zip, iceberg, gates

core            the vocabulary: periods, numbers, names, diagnostics, home, config
net             HTTP: transports, paced and retried requests, the refusal breaker,
                bodies streamed to disk, the archive's helpers          -> core
kit             the ports: Source and Listings, Decoder and BlockSink, Catalog,
                the file types they pass; no I/O                        -> core, net (no tokio)
acquire         the adapters: Fetcher (implements Listings), FeedFolders,
                nemweb's listing pages                                  -> kit, net
registry        every market's tables, block resolution, the catalog    -> kit, core
mms-conform     the MMS and plain decoders into typed Arrow batches      -> registry, kit
nem, wem, gas   a market's kit: its sources, decoders and catalog       -> acquire, mms-conform
iceberg-store   Iceberg tables on disk                                  -> core
writer          everything written under the home: the ingest session,
                quarantine, manifest, supersede, Iceberg, verify        -> iceberg-store, kit
pipeline        orchestration over the ports and the writer only        -> kit, acquire, writer
cli, mirror     the binaries
bench, e2e      tooling and whole-stack tests; nothing depends on them
```

The pipeline names no format and no market (`cargo tree -p phasor-pipeline`
shows no `mms`, `nem`, `wem` or `gas`); a market crate depends on the format
it reads; the decoder never depends on the registry; kit pulls in no reqwest
and no tokio. Async lives at the edges (the fetcher, the manifest's handle):
parsing and Parquet encoding are synchronous and run on blocking workers, one
source file per worker, and the two worlds meet once per file.

## Test tiers

| tier | where | gate |
|---|---|---|
| unit and integration | every crate, `cargo test` | `just test`, pre-commit |
| coverage | `cargo llvm-cov` over the workspace | `just cover` against `coverage.json`, pre-push |
| pulled schemas | `schemas/nem/pulled/` against its manifest and the rev in `Cargo.lock` | `just schemas-check`, pre-commit |
| benchmarks | criterion plus `phasor-bench` | `just bench-fast` pre-push, `just bench` in CI, against `bench/budgets.json` |
| network | `crates/phasor-nem/tests/net.rs` | `just net-test`, weekly in CI |

## Floors and budgets

A coverage floor in `coverage.json` only goes up. A benchmark budget in
`bench/budgets.json` only tightens: `just bench-tighten` moves budgets to the
latest measurement when that is better, never the other way. `just budget-check`
fails when either moved the wrong way against `origin/main`, unless a commit in
the range explains it with a `coverage-floor:` or `bench-budget:` line.

The NEM tables are not edited here. They are the histories phasor-schemas
publishes, and what a history may change from one commit to the next is
held there (its `compat` check). Here they move only together with the
crate that reads them: `just schemas-pull REV` copies the tables at a
commit into `schemas/nem/pulled/`, the `phasor-iceberg` rev in
`Cargo.toml` moves to the same commit, and `just schemas-check` fails until
both agree and every file is its manifest's. A commit that changes what
the lake holds explains it with a `schema-break:` line.

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

phasor is licensed under the Apache License 2.0 (`LICENSE`). A contribution
you submit for inclusion is under that licence, as its section 5 says, with
no other terms.
