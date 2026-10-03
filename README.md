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
| `nemlake cache ls\|clear` | the download cache |
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

Types: `DATE` → `Timestamp(us, +10:00)` (NEM time is fixed AEST), `NUMBER(p,s)`
→ `Float64` (or `Decimal128(p, s)` with `--decimal`), `NUMBER(p,0)` → `Int64`,
`VARCHAR2`/`CHAR` → `Utf8`.

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

M0: the NEM monthly archive from 2024 on, 22 tables in the registry, Parquet +
manifest. Next: every public table and older archive layouts (M1), live
5-minute feeds and compaction (M2), WEM and an Iceberg writer (M3).

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
