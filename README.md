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
| `nemlake reingest [--dry-run]` | read again, from disk, every source whose lake files are behind the current rules; no network |
| `nemlake inspect <file> [--infer]` | every block in a file, its version and how the registry resolves it; draft a schema for unknown ones |
| `nemlake ls [--table T] [--failed] [--runs]` | what the manifest knows |
| `nemlake errors [--last] [--table T] [--code C]` | diagnostics recorded by past runs |
| `nemlake verify` | every Parquet file the manifest lists exists with its size |
| `nemlake schema list\|show\|diff\|lineage\|validate` | the registry |
| `nemlake schema infer <file> [--write]` | draft what a file adds to the registry: a new table, version or variant |
| `nemlake archive ls\|verify` | every downloaded file with when and how it was requested; `verify` hashes each one again |
| `nemlake cache gc\|clear` | throwaway files only: `gc` removes leftovers of interrupted work, `clear` the cached listings; neither touches the archive |
| `nemlake wem manifest <dataset>` | a WEM dataset's file list (WEM ingestion comes later) |

Global flags: `--home DIR` (or `NEMLAKE_HOME`), `--json`, `-v`, `--offline`,
`--jobs N`, `--decimal`. Ingest flags: `--max-bad-rows N`, `--strict`,
`--fail-fast`.

## How the data is laid out

```
~/.nemlake/
  lake/nem/{table}/year=YYYY/month=MM/{source}-v{version}-{shape8}-{sha8}.parquet   appended tables, partitioned by market-local month
  lake/nem/{table}/snapshot=YYYYMM/...                                     standing-data tables (REGION, PARTICIPANT, ...)
  archive/{host}/{path of the URL}[.json]                                  every download as the server sent it, with its request record
  cache/listings/, cache/tmp/                                              listings and scratch files, safe to delete
  quarantine/{table}/{source}.jsonl                                        rows that failed to parse, with the reason
  manifest.sqlite                                                          runs, files, ingests, outputs, diagnostics
```

Every Parquet file carries the unified lake schema of its table: every column
AEMO has ever published for it, so files from different versions read as one
table and a later Iceberg catalogue can adopt the column ids unchanged.

Nothing a file carries is skipped. A table no schema declares is ingested
from its own header, every column as text, under
`lake/nem/_unregistered/{report type}.{report subtype}/`; a column a table's
schema does not have is kept beside the declared ones as text under its own
name; a column the file lacks is null. Each case is reported, and
`nemlake schema infer` drafts the schema that would make it a declared
shape. `--strict` refuses such files instead.

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

## How a file was made

A Parquet file is a function of three things: the bytes of its source, the
shape of the block it was read from, and the rules the shape was read by.
All three are written into the file's footer and into the manifest:

| footer key | what it says |
|---|---|
| `nemlake.source_url`, `nemlake.source_sha256`, `nemlake.entry` | the source: where it came from, its hash, the CSV inside it |
| `nemlake.report_type`, `nemlake.report_subtype`, `nemlake.source_version`, `nemlake.source_columns`, `nemlake.fingerprint` | the shape: the block's I row as AEMO published it |
| `nemlake.source_period` | the month of the source, which rules for an interval are chosen by |
| `nemlake.rules` | a hash of the rules: the lake columns and their types, which field feeds which column and how it is parsed, the partitioning, the directory |
| `nemlake.schema_source` | where the table's definition came from: `registry` (built in), `user` (your own table file), `file` (nobody declared it, so the file's own header) |
| `nemlake.resolution`, `nemlake.table`, `nemlake.schema_version` | how the shape matched the definition: `exact`, `aliased`, `drift` or `derived`, and as which table and version |

The rules hash changes when the output would: a column gets another type,
a table gets a schema where it had none, `--decimal` is turned on. It does
not change when a description is edited. A source whose recorded hash is
no longer what the current rules give is behind: the next `fetch` or
`ingest` that covers it reads it again instead of skipping it, and
`nemlake reingest` does that for every source in the manifest, straight
from the archive. The Parquet files the earlier read wrote are replaced,
and the run reports each such source as `manifest.rules_changed`.
`nemlake reingest --dry-run` lists what is behind and changes nothing.

## The archive of downloads

Every file nemlake downloads is kept exactly as the server sent it, at the
path its URL spells out:

```
~/.nemlake/archive/www.nemweb.com.au/Data_Archive/Wholesale_Electricity/MMSDM/2024/MMSDM_2024_01/MMSDM_Historical_Data_SQLLoader/DATA/
  PUBLIC_DVD_DISPATCHPRICE_202401010000.zip
  PUBLIC_DVD_DISPATCHPRICE_202401010000.zip.json
```

The `.json` beside it records what makes the file traceable: the URL, when
the request was sent and when the download completed, the nemlake command
that asked for it, the tool version and user agent, the status and headers
the server answered with, how many attempts it took and whether one resumed
an earlier one, the size and sha256 of the bytes, and what the listing said
about the file. A file read out of a whole-month zip sits under that zip's
path with `.members` added, and its record names the zip, the member, its
offset and the CRC-32 it was checked against.

Nothing in nemlake deletes an archived file. When the same URL later
serves other bytes, the earlier file is kept beside the new one and the
new record points at it. Everything in the lake can be derived again from
the archive, which is what `nemlake reingest` does when the rules for
reading a table improve. `nemlake archive verify` hashes every file
against its record.

## Your own tables

AEMO adds tables and versions faster than releases follow. When a file does
not resolve, `nemlake schema infer <file> --write` drafts what is missing:
a table for a report pair nobody declared, or a new version or variant of a
table that exists, with the types of new columns guessed from the rows. The
drafts land in `~/.nemlake/schemas/nem/mms/DRAFT_{TABLE}.toml` and do nothing
until you have looked at them and renamed one to `{TABLE}.toml`. From then
on every command uses it: a file named after a built-in table replaces it,
any other adds a table. `--no-user-schemas` runs without them. What was
ingested before the table file existed is brought up to it by
`nemlake reingest`.

## Rules for an interval

AEMO's files are not uniform over seventeen years, and no one set of rules
explains them all. nemlake does not try. The registry describes what is
published today; an older file is read by its own header, and whatever does
not fit today's table is kept beside it rather than forced into it.

Where you know better for some stretch of time, say so in the table file,
by hand, for exactly the files you mean:

```toml
[[column]]                       # a column of your own to receive the values
id = 14
name = "RRP_AS_PUBLISHED"
type = "utf8"
added_in = 1
src = ["manual"]

[[rule]]
note = "Until 2011-12 RRP carried a currency suffix; checked against the 2010-03 and 2011-12 archives."
from = "2009-07"                 # first source month, optional
to = "2011-12"                   # last source month, inclusive, optional
version = 1                      # only blocks published as this version, optional
# fingerprint = "xxh3:..."       # only this exact column list, optional

[[rule.column]]
source = "RRP"                   # the column's name in the file
column = "RRP_AS_PUBLISHED"      # the lake column it feeds in this interval
```

A rule routes named file columns to declared lake columns; the lake
column's type decides how the values are read. With that, a column
published under another name for a few years can feed the column it
belongs to, and a column whose values were of another kind before a market
change can be kept as published in a column of its own. Rules apply in the
order written, each needs a `note` saying why it exists, and none is ever
generated or inferred: regenerating the registry carries them over as
written. A rule never loses data. If a file carries both the routed column
and the column it would feed, both are kept and the run reports
`schema.rule_conflict`.

Which rules read a file is on record: the `nemlake.resolution` of a file a
rule touched ends in `+rule`, `nemlake.source_period` is the month the
rules were chosen by, and the rules hash differs from that of a file read
without them. Edit a rule and `nemlake reingest` reads again exactly the
sources it affects.

## Which months

The archive begins in July 2009. From 2015 each month is a directory of
per-table zips and nemlake downloads the ones asked for. Before 2015 a month
exists only as one zip of 1.4 to 2.1 GB holding those per-table zips;
nemlake reads that zip's index with ranged requests and downloads only the
members asked for, checked against the CRC-32 the index records. One table
of January 2012 is a 12 MB download, not 1.9 GB.

File names changed along the way, from `PUBLIC_DVD_{TABLE}_{YYYYMM}010000.zip`
to `PUBLIC_ARCHIVE#{TABLE}#FILE{NN}#{YYYYMM}010000.zip`; both are read.

The registry is anchored on what AEMO publishes today: its tables, column
ids and types come from the newest month it was built from (August 2026),
and the shapes of an earlier month (January 2024) are declared against
that anchor. Every file of those two months resolves exactly. A file of
another month resolves exactly where its shape is one of theirs, which
most are; a shape nobody has declared is still ingested whole, as
described above, and `nemlake schema infer` drafts its declaration.

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
every table of the August 2026 and January 2024 archives (241) in the
registry, exact decimals with `--decimal`, downloads that resume where they
stopped, and a lake that can be read again from the archive when the rules
change. Next: the shapes of earlier years as declared variants; then live
5-minute feeds and compaction; then WEM and an Iceberg writer.

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
