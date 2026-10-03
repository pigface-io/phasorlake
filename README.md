# nemlake

AEMO market data into a local Parquet lake, from one Rust binary.

`nemlake` downloads the National Electricity Market's monthly archive files
from nemweb, parses their `C`/`I`/`D` row format through a schema registry
that knows every table version AEMO has published, and writes typed,
Hive-partitioned Parquet that DuckDB, Polars and anything else that reads
Parquet can query directly. A SQLite manifest records every file fetched,
every ingest, every output and every error, so a second run is a no-op and
a failure is explained.

## Quick start

```sh
cargo install --path crates/nemlake-cli --locked
nemlake fetch nem --table DISPATCH_UNIT_SCADA --from 2024-01 --to 2024-01
```

The result lands under `~/.nemlake/lake/nem/dispatch_unit_scada/year=2024/month=01/`.
Query it:

```sql
select count(*), typeof(settlementdate)
from read_parquet('~/.nemlake/lake/nem/dispatch_unit_scada/**/*.parquet', hive_partitioning = true);
```

Run the same command again and it finishes in milliseconds: the manifest knows
the file is already in the lake.

## Commands

| command | what it does |
|---|---|
| `nemlake fetch nem --table T[,T] --from YYYY-MM [--to YYYY-MM]` | list the archive, download, ingest |
| `nemlake ingest <zip\|csv\|dir>...` | ingest local files, no network |
| `nemlake inspect <file> [--infer]` | every block in a file, its version and how the registry resolves it; draft a schema for unknown ones |
| `nemlake ls [--table T] [--failed] [--runs]` | what the manifest knows |
| `nemlake errors [--last] [--table T] [--code C]` | diagnostics recorded by past runs |
| `nemlake verify` | every Parquet file the manifest lists exists with its size |
| `nemlake schema list\|show\|diff\|lineage\|validate` | the registry |
| `nemlake schema infer <file> [--write]` | draft what a file adds to the registry: a new table, version or variant |
| `nemlake cache ls\|gc\|clear` | the download cache; `gc` removes leftovers of interrupted work and, with `--ingested` or `--older-than DAYS`, downloads no longer needed |
| `nemlake wem manifest <dataset>` | a WEM dataset's file list (WEM ingestion comes later) |

Global flags: `--home DIR` (or `NEMLAKE_HOME`), `--json`, `-v`, `--offline`,
`--jobs N`, `--decimal`. Ingest flags: `--max-bad-rows N`, `--strict`,
`--fail-fast`, `--no-skip-unknown`.

## How the data is laid out

```
~/.nemlake/
  lake/nem/{table}/year=YYYY/month=MM/{source}-v{version}-{sha8}.parquet   appended tables, partitioned by market-local month
  lake/nem/{table}/snapshot=YYYYMM/...                                     standing-data tables (REGION, PARTICIPANT, ...)
  cache/{source}/{hash}-{filename}[.json]                                  downloads with their sidecars
  quarantine/{table}/{source}.jsonl                                        rows that failed to parse, with the reason
  manifest.sqlite                                                          runs, files, ingests, outputs, diagnostics
```

Every Parquet file carries the unified lake schema of its table: every column
AEMO has ever published for it, so files from different versions read as one
table and a later Iceberg catalogue can adopt the column ids unchanged.

AEMO appends and renames columns without bumping the I-row version, so one
version can have several real shapes. The registry records each extra shape
as a `[[version.variant]]` of its version; a file of any declared shape
resolves exactly, and only a shape nobody has seen before is reported as
drift.

Types: `DATE` and `TIMESTAMP` → `Timestamp(ns, +10:00)` (NEM time is fixed
AEST), `NUMBER(p,s)` → `Float64` (or `Decimal128(p, s)` with `--decimal`),
`NUMBER(p,0)` → `Int64`, `VARCHAR2`/`CHAR` → `Utf8`.

Every timestamp in the lake is nanoseconds since the epoch in UTC, one unit
with no exceptions, and a fractional second is kept to the last digit. That
unit spans 1677-09-21 to 2262-04-11. AEMO writes "no end" as `9999/12/31`
or `2999/12/31`; such a value is stored as the last instant of the range,
so `END_DATE > now()` still holds, and each column it happens in is
reported as `parse.timestamp_clamped` with a count and an example.

## Your own tables

AEMO adds tables and versions faster than releases follow. When a file does
not resolve, `nemlake schema infer <file> --write` drafts what is missing:
a table for a report pair nobody declared, or a new version or variant of a
table that exists, with the types of new columns guessed from the rows. The
drafts land in `~/.nemlake/schemas/nem/mms/DRAFT_{TABLE}.toml` and do nothing
until you have looked at them and renamed one to `{TABLE}.toml`. From then
on every command uses it: a file named after a built-in table replaces it,
any other adds a table. `--no-user-schemas` runs without them.

## Which months

The archive begins in July 2009. From 2015 each month is a directory of
per-table zips and nemlake downloads the ones asked for. Before 2015 a month
exists only as one zip of 1.4 to 2.1 GB holding those per-table zips;
nemlake reads that zip's index with ranged requests and downloads only the
members asked for, checked against the CRC-32 the index records. One table
of January 2012 is a 12 MB download, not 1.9 GB.

File names changed along the way, from `PUBLIC_DVD_{TABLE}_{YYYYMM}010000.zip`
to `PUBLIC_ARCHIVE#{TABLE}#FILE{NN}#{YYYYMM}010000.zip`; both are read.

AEMO's web server blocks a client that asks too fast. nemlake leaves 250 ms
between requests (`request_interval_ms` in `config.toml`), and when the
server refuses it (HTTP 403, or 429 through every retry) it sends nothing
more in that run and reports `net.blocked` for what is left.

A download that breaks keeps what arrived. The next attempt, or the next
run, asks for the rest with `Range` and `If-Range`, so a file that changed
in the meantime is fetched whole again instead of being stitched together.

## Errors

Every error and warning carries a stable code (`csv.field_count_mismatch`,
`net.content_length_mismatch`, `schema.version_drift`, ...), a full-sentence
message and the context: the request, the URL, the cache file, the zip entry,
the table and version, the line and byte offset, and a sample of the offending
input. A run attempts every file and reports each failure; the exit code says
whether all, some or none succeeded (0, 3, 1), 4 for an offline cache miss, 5
for a schema that could not be resolved in strict mode. `docs/errors.md` lists
every code.

## Measured

On an Apple M-series laptop, debug off: the scanner alone reads 1.5 GiB/s of
MMS CSV; the typed path (parse into Arrow) 530 MiB/s. The January 2024
DISPATCH_UNIT_SCADA archive (22.6 MB zip, 4.19 million rows) downloads,
parses and writes in about 7 seconds, 11.3 MB of Parquet. Budgets that these
numbers may not regress below live in `bench/budgets.json`.

## Status

Working today: the NEM monthly archive from July 2009 on, Parquet + manifest,
every table of the January 2024 archive (143) in the registry, exact decimals
with `--decimal`, and downloads that resume where they stopped. Next: the
table versions of earlier years; then live 5-minute feeds and compaction;
then WEM and an Iceberg writer.

## Developing

```sh
just setup      # pinned tools and git hooks
just check      # every hook, as CI runs it
just bench      # benchmarks against their budgets
```

See `CONTRIBUTING.md`.

## Licence

Apache License 2.0; see `LICENSE` and `NOTICE`. The excerpts of AEMO data
under `testdata/` are AEMO's and are not covered by it; see
`testdata/LICENCE.md`.
