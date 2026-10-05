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
nemlake fetch nem --table DISPATCH_UNIT_SCADA --from 2024-01
nemlake ingest --from 2024-01 --table DISPATCH_UNIT_SCADA
```

`fetch` downloads into the archive, each file once; `ingest` writes Parquet
from what is archived, without the network.

The result lands under `~/.nemlake/lake/nem/dispatch_unit_scada/year=2024/month=01/`.
Query it:

```sql
select count(*), typeof(settlementdate)
from read_parquet('~/.nemlake/lake/nem/dispatch_unit_scada/**/*.parquet', hive_partitioning = true);
```

Run the same commands again and they finish in milliseconds: the archive has
the file, and the manifest knows it is already in the lake.

## Commands

| command | what it does |
|---|---|
| `nemlake fetch nem --from YYYY-MM [--to YYYY-MM] [--table T,T] [--exclude T,T] [--dry-run]` | list the archive and download each file not archived yet, with the month's control files and README. Without `--table`: every table the archive has for those months. `--dry-run` lists the files and totals their size |
| `nemlake ingest --from YYYY-MM [--to YYYY-MM] [--table T,T] [--exclude T,T]` | write Parquet from the archived files of those months, no network |
| `nemlake ingest <zip\|csv\|dir>...` | ingest local files, no network |
| `nemlake reingest [--dry-run]` | read again, from disk, every source whose lake files are behind the current rules; no network |
| `nemlake inspect <file> [--infer]` | every block in a file, its version and how the registry resolves it; draft a schema for unknown ones |
| `nemlake ls [--table T] [--failed] [--runs]` | what the manifest knows |
| `nemlake errors [--last] [--table T] [--code C]` | diagnostics recorded by past runs |
| `nemlake verify` | every Parquet file the manifest lists exists with its size |
| `nemlake schema list\|show\|diff\|lineage\|validate` | the registry |
| `nemlake schema plan <type> <subtype> <version> --columns A,B,C [--month YYYY-MM]` | the conformance plan a shape is read with, as JSON: where each file column goes, its cast, what is written null |
| `nemlake schema infer <file> [--write] [--ctl FILE]` | draft what a file adds to the registry: a new table, version or variant; `--ctl` types it from AEMO's control file for the table |
| `nemlake archive ls\|verify` | every downloaded file with where and when it came from; `verify` hashes each one again |
| `nemlake wem manifest <dataset>` | a WEM dataset's file list (WEM ingestion comes later) |

Global flags: `--home DIR` (or `NEMLAKE_HOME`), `--json`, `-v`, `--offline`,
`--jobs N`, `--decimal`. Ingest flags: `--max-bad-rows N`, `--strict`,
`--fail-fast`.

## How the data is laid out

```
~/.nemlake/
  lake/nem/{table}/year=YYYY/month=MM/{source}-v{version}-{shape8}-{sha8}.parquet   appended tables, partitioned by market-local month
  lake/nem/{table}/snapshot=YYYYMM/...                                     standing-data tables (REGION, PARTICIPANT, ...): each month's file is the whole table
  archive/{host}/{path of the URL}[.json]                                  every download as the server sent it, with its record
  cache/tmp/                                                               scratch files of a running ingest, safe to delete
  quarantine/{table}/{source}.jsonl                                        rows whose structure was wrong for their block, with the reason
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
`VARCHAR2`/`CHAR` → `Utf8`. Whole numbers are `Float64` too: AEMO's control
files load every numeric column as `FLOAT EXTERNAL`, its data puts fractions
in columns the data model declares `NUMBER(p,0)`, and `Float64` holds every
whole number of up to 15 digits exactly. A column is `Int64` only where a
table file says so by hand (`type = "integer"`, `curated = true`).

Every timestamp in the lake is nanoseconds since the epoch in UTC, one unit
with no exceptions, and a fractional second is kept to the last digit, so a
later Iceberg table of the lake is format V3, the first with `timestamp_ns`
(see `docs/iceberg.md`). That unit spans 1677-09-21 to 2262-04-11. AEMO
writes "no end" as `9999/12/31` or `2999/12/31`; such a value is stored as
the last instant of the range,
so `END_DATE > now()` still holds, and each column it happens in is
reported as `parse.timestamp_clamped` with a count and an example.

No row is dropped for one of its values. A value its column's type cannot
hold (a fraction where `--decimal` gives `Decimal128(6, 0)`, a month 13)
leaves the column null in that row and is kept, as published, in a text
column beside it named `{column}__raw`, which exists only in the files that
need it (`parse.value_kept_raw`). An empty value in a column that does not
allow nulls is kept as a null (`parse.null_kept`). `--strict` refuses the
file at the first of either. Only a row whose structure is wrong for its
block, such as more fields than the I row declares, goes to the quarantine.

## How a file was made

A Parquet file is a function of three things: the bytes of its source, the
shape of the block it was read from, and the rules the shape was read by.
All three are written into the file's footer and into the manifest:

| footer key | what it says |
|---|---|
| `nemlake.source_url`, `nemlake.source_sha256`, `nemlake.entry` | the source: where it came from, its hash, the CSV inside it |
| `nemlake.report_type`, `nemlake.report_subtype`, `nemlake.source_version`, `nemlake.source_columns`, `nemlake.fingerprint` | the shape: the block's I row as AEMO published it |
| `nemlake.source_period` | the month of the source, which rules for an interval are chosen by |
| `nemlake.plan` | the hash of the conformance plan the shape was read by: which file column feeds which lake column, that column's id, type and nullability, how the value is parsed, the key and time columns, the load mode, the directory |
| `nemlake.shape_id` | the ids the census gives the shape (`just census`), when it has seen it |
| `nemlake.schema_source` | where the table's definition came from: `registry` (built in), `user` (your own table file), `file` (nobody declared it, so the file's own header) |
| `nemlake.resolution`, `nemlake.table`, `nemlake.schema_version` | how the shape matched the definition: `exact`, `aliased`, `drift` or `derived`, and as which table and version |

The plan hash changes when the output of that shape would: a column it
carries gets another type, a table gets a schema where it had none,
`--decimal` is turned on, a rule routes one of its columns. It does not
change when a description is edited, nor when the table gains a column the
shape does not carry: its files simply lack that column, which reads as
null. A source whose recorded hash is no longer what the current rules
give is behind: the next `fetch` or
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

The `.json` beside it records where the file came from: the URL, when the
download completed, the server's `ETag` and `Last-Modified` (when AEMO
published it), the size and sha256 of the bytes, and the nemlake version
that fetched it.

A file is downloaded once. Nothing in nemlake replaces or deletes an
archived file; to fetch one again, delete it and its record. Everything in
the lake, and the census of every shape AEMO published (`just census`), is
derived from the archive, which is what `nemlake reingest` does when the
rules for reading a table improve. `nemlake archive verify` hashes every
file against its record.

## Your own tables

AEMO adds tables and versions faster than releases follow. When a file does
not resolve, `nemlake schema infer <file> --write` drafts what is missing:
a table for a report pair nobody declared, or a new version or variant of a
table that exists, with the types of new columns guessed from the rows.
Give it the table's control file from the same archive month with `--ctl`
(the `.ctl` files sit in the month's `CTL/` directory) and the draft is
typed from AEMO's own loader definition instead, takes the table's real
name, and keeps a guess only where a sampled value does not fit what the
control file says. The
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
shape = "a3f9c2e1b4d07785"       # only blocks of this shape, from schemas/nem/shapes/
# from = "2009-07"               # first source month
# to = "2011-12"                 # last source month, inclusive
# version = 1                    # only blocks published as this version
# fingerprint = "xxh3:..."       # only this exact column list

[[rule.column]]
source = "RRP"                   # the column's name in the file
column = "RRP_AS_PUBLISHED"      # the lake column it feeds in this interval
```

A rule says which files it is for, and at least one scope is required. Name
the shape: the census (`just census`) records every shape a table
was published in, with its id and the months it was seen, under
`schemas/nem/shapes/`, and a rule for a shape applies to exactly the blocks
of that shape in the months the census saw it. A span of months is for the
rare case of one shape whose meaning changed over time; it may be combined
with `shape`.

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
rules were chosen by, and the plan hash differs from that of a file read
without them. Edit a rule and `nemlake reingest` reads again exactly the
sources it affects.

## Which months

The archive begins in July 2009. From 2015 each month is a directory of
per-table zips and nemlake downloads the ones asked for. Before 2015 a month
exists only as one zip of 1.4 to 2.1 GB holding those per-table zips;
nemlake downloads it whole, once, and reads the per-table zips nested in it
from disk.

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

A download that breaks leaves nothing behind; the next attempt fetches the
file whole. Its bytes go from the network to disk without a copy, hashed on
the way on a thread of their own.

## Errors

Every error and warning carries a stable code (`csv.field_count_mismatch`,
`net.content_length_mismatch`, `schema.version_drift`, ...), a full-sentence
message and the context: the request, the URL, the archived file, the zip entry,
the table and version, the line and byte offset, and a sample of the offending
input. A run attempts every file and reports each failure; the exit code says
whether all, some or none succeeded (0, 3, 1), 4 for a file not archived when offline, 5
for a schema that could not be resolved in strict mode. `docs/errors.md` lists
every code.

Errors and warnings are printed as JSON, one object a line, as they happen
on stderr and again in the summary and in `nemlake errors`:

```json
{"code":"parse.integer","category":"parse","severity":"error","message":"Column REGISTEREDCAPACITY: the value 13.55 is a number with a fraction or exponent, but the column is declared a whole number (Int64).","table":"DUDETAIL","version":7,"line":133,"column":"REGISTEREDCAPACITY","expected":"a whole number (Int64)","observed":"13.55","sample":{"text":"D,PARTICIPANT_REGISTRATION,DUDETAIL,7,..."}}
```

A row that cannot be read is reported with its own error: the column, what
was declared, what the file has, the line and the row itself. When such rows
refuse a file (`csv.too_many_bad_rows`), the refusal repeats the first row's
fault in its message, its `column`, `expected` and `observed`, and its
`cause`. A failed write carries what the operating system said in `cause`.

A source whose rows spread over more months than a worker may hold open is
written in parts: `...-p2.parquet`, `...-p3.parquet` beside the first file
of a month. At most 128 Parquet files are open across all workers, so a run
stays within the few hundred open files an operating system allows.

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
