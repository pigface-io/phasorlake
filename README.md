# utilake

AEMO market data into a local Parquet lake, from one Rust binary.
Pronounced *util-lake*: a lake for the utilities' data, the electricity
markets first.

`utilake` downloads the National Electricity Market's monthly archive files
from nemweb, parses their `C`/`I`/`D` row format through a schema registry
that knows every schema each table's files have had, and writes typed,
Hive-partitioned Parquet that DuckDB, Polars and anything else that reads
Parquet can query directly. A SQLite manifest records every file fetched,
every ingest, every output and every error, so a second run is a no-op and
a failure is explained.

## Quick start

```sh
cargo install --path crates/utilake-cli --locked
utilake get DISPATCH_UNIT_SCADA --from 2024-01-15 --to 2024-01-18
```

`get` plans the interval across the market's sources (whole published
months from the monthly archive, the rest from the report folders a day
or a five-minute file at a time), downloads only what it needs (a zip
that holds more than asked is read by its directory and only the entries
wanted are fetched), reads it all into the lake, and writes one square
frame per table: every file projected onto one schema, one row per fact,
the versions in force across the window, as Parquet parts under
`./utilake-get/nem/DISPATCH_UNIT_SCADA/` with `DISPATCH_UNIT_SCADA.json`
beside them saying which files its rows came from and what the window
holds. `--from` and `--to` take a month, a day, a local time
(`2024-01-15T10:00`, market time) or an RFC 3339 time; a month or a day as
the end is included through its last interval, a time is where the window
stops. `--day D` is one trading day (04:00 to 04:00). `--at T` is the
version in force at an instant. `--offline` reads the lake as it is.
`--dry-run` shows what would be fetched and read. Every command's `--help`
ends with examples.

The stages on their own:

```sh
utilake fetch DISPATCH_UNIT_SCADA --from 2024-01 --to 2024-01
utilake ingest DISPATCH_UNIT_SCADA --from 2024-01 --to 2024-01
utilake import PUBLIC_DVD_DISPATCHPRICE_202401010000.zip
```

`fetch` downloads into the archive, each file once, politely; `ingest`
writes Parquet from what is archived, without the network (with no
arguments, everything archived and not yet in the lake, and everything
behind its rules); `import` reads files you already have.

The result lands under `~/.utilake/lake/nem/dispatch_unit_scada/year=2024/month=01/`.
Query it:

```sql
select count(*), typeof(settlementdate)
from read_parquet('~/.utilake/lake/nem/dispatch_unit_scada/**/*.parquet', hive_partitioning = true);
```

Run the same commands again and they finish in milliseconds: the archive has
the file, and the manifest knows it is already in the lake.

Resolution, projection and the same read as SQL:

```sh
utilake get DUDETAIL --at 2024-01-15 --columns DUID,REGISTEREDCAPACITY --offline
utilake get DISPATCHPRICE --day 2024-01-15 --known-at 2024-01-15T12:00 -o prices.parquet
utilake get DISPATCHPRICE --from 2024-01-15 --to 2024-01-15 --format sql -o -
utilake list --grep price
```

`docs/semantics.md` says what a window and a revision mean on each kind of
table.

## Commands

| command | what it does |
|---|---|
| `utilake get T,T (--from A --to B \| --at T \| --day D) [--known-at K] [--revisions \| --raw] [--as-of N] [--columns A,B] [--where COL=v,v] [-o DIR\|FILE] [--format parquet\|csv\|arrow\|sql] [--compression zstd\|snappy\|none] [--source auto\|monthly\|daily] [--offline] [--dry-run]` | an interval of the tables, fetched where it is published most finely, read into the lake, and written as one frame per table (one row per fact, the versions in force) as Parquet parts under `{out}/{market}/{TABLE}/` with a sidecar of its sources and intervals; `--offline` reads the lake as it is; `--format sql` writes the same read as a query. Times in the market's clock. `docs/semantics.md` |
| `utilake fetch T,T --from A --to B [--source auto\|monthly\|daily] [--downloads N] [--dry-run]` | the files that cover the window, into the archive, each once; nothing read into the lake |
| `utilake import <zip\|csv\|dir>...` | files you already have, into the lake; no network |
| `utilake ingest [T,T --from A --to B] [--source ID] [--dry-run]` | archived files into the lake, no network; with no arguments, every archived file not yet in the lake and every lake file behind its rules |
| `utilake list [T] [--market M] [--kind K] [--grep TEXT] [--coverage] [--values COL] [--files] [--archive] [--ingests [--failed]] [--runs]` | every table with its roles and what the lake holds; one table's columns; the months held and the gaps; an entity column's values; the files, the archive, the manifest |
| `utilake inspect <file>` | every block in a file, its version and how the registry resolves it |
| `utilake errors [--last] [--table T] [--code C]` | diagnostics recorded by past runs |
| `utilake verify [--table T] [--archive]` | every Parquet file the manifest lists exists with its size; `--archive` hashes every download again |
| `utilake schema list\|show\|history\|validate` | the registry; `history T` lists a table's schemas, with the columns each adds or drops, and the runs of files under each (`--json`: its Iceberg history) |
| `utilake schema plan <type> <subtype> <version> --columns A,B,C [--month YYYY-MM]` | the conformance plan a shape is read with, as JSON: where each file column goes, its cast, what is written null |
| `utilake wem manifest <dataset>` | a WEM dataset's file list (WEM ingestion comes later) |

Global flags: `--home DIR` (or `UTILAKE_HOME`), `--json`, `-v`, `--offline`,
`--jobs N`, `--no-user-schemas`. Ingest flags: `--max-bad-rows N`, `--strict`,
`--fail-fast`.

## How the data is laid out

```
~/.utilake/
  lake/nem/{table}/year=YYYY/month=MM/{source}-v{version}-{shape8}-{sha8}.parquet   appended tables, partitioned by market-local month
  lake/nem/{table}/snapshot=YYYYMM/...                                     standing-data tables (REGION, PARTICIPANT, ...): each month's file is the whole table
  archive/{host}/{path of the URL}[.json]                                  every download as the server sent it, with its record
  archive/{host}/{path of a zip}!{entry}[.json]                            one entry of a zip, fetched alone by byte range; its record says where in the zip it was
  cache/tmp/                                                               scratch files of a running ingest, safe to delete
  quarantine/{table}/{source}.jsonl                                        rows whose structure was wrong for their block, with the reason
  manifest.sqlite                                                          runs, files, ingests, outputs, diagnostics
```

An entry of a zip is the same source whether it was fetched alone (as
`{zip}!{entry}`) or inside the whole zip: a read of the whole zip leaves
the entries the lake already holds from a fetch of each alone, and an
entry fetched alone whose zip was read whole is skipped, so `get` and
`fetch` never write a row twice. Whole months come from the monthly
archive, the settled publication; days and five-minute files from the
report folders, the live one; `get` never mixes the two within a month,
and each frame's sidecar names the vintage of every source.

Every Parquet file carries the lake schema of its table: every column the
table has ever had, each with its id as `PARQUET:field_id`, so files of
every schema read as one table and an Iceberg catalogue adopts the column
ids unchanged.

Nothing a file carries is skipped. A table no schema declares is ingested
from its own header, every column as text, under
`lake/nem/_unregistered/{report type}.{report subtype}/`; a column a table's
schema does not have is kept beside the declared ones as text under its own
name; a column the file lacks is null. Each case is reported
(`schema.unknown_table`, `schema.deviates`). `--strict` refuses such files
instead.

Types are exact, one to one with Iceberg's: each column has the type its
field has in the table's history, which the census chose from everything
AEMO published in that column and from AEMO's declarations.

| `type` | Arrow | Iceberg V3 |
|---|---|---|
| `string` | `Utf8` | `string` |
| `double` | `Float64` | `double` |
| `long` | `Int64` | `long` |
| `boolean` | `Boolean` | `boolean` |
| `date` | `Date32` | `date` |
| `timestamp` | `Timestamp(µs, +10:00)` | `timestamptz` |
| `decimal(P,S)` | `Decimal128(P, S)` | `decimal(P, S)` |

Most numbers are `decimal(P,S)` (RRP is `decimal(15,5)`), read exactly
and never rounded; counters such as RUNNO are `long`. AEMO writes a few
small values in exponent form (`-3E-05`); they are read exactly too when
the column's scale holds them. NEM time is fixed AEST.

Every timestamp in the lake is microseconds since the epoch in UTC, one unit
with no exceptions: Iceberg's `timestamptz`, which every Iceberg reader
reads (DuckDB's refuses `timestamptz_ns`), and AEMO's fractional seconds
have at most three digits (see `docs/iceberg.md`). Values are clamped to the
range nanoseconds span, 1677-09-21 to 2262-04-11. AEMO
writes "no end" as `9999/12/31` or `2999/12/31`; such a value is stored as
the last instant of the range,
so `END_DATE > now()` still holds, and each column it happens in is
reported as `parse.timestamp_clamped` with a count and an example.

No row is dropped for one of its values. A value its column's type cannot
hold (a fraction in a `long` column, a month 13)
leaves the column null in that row and is kept, as published, in a text
column beside it named `{column}__raw`, which exists only in the files that
need it (`parse.value_kept_raw`). An empty value in a column that does not
allow nulls is kept as a null (`parse.null_kept`). `--strict` refuses the
file at the first of either. Only a row whose structure is wrong for its
block, such as more fields than the I row declares, goes to the quarantine.

## How a file was made

A Parquet file is a function of three things: the bytes of its source, the
shape of the block it was read from, and the plan the shape was read by.
All three are written into the file's footer and into the manifest:

| footer key | what it says |
|---|---|
| `utilake.source_url`, `utilake.source_sha256`, `utilake.entry` | the source: where it came from, its hash, the CSV inside it |
| `utilake.report_type`, `utilake.report_subtype`, `utilake.source_version`, `utilake.source_columns`, `utilake.fingerprint` | the shape: the block's I row as AEMO published it |
| `utilake.source_period` | the month the source was released in, which picks among the runs of the table's history that fit a block |
| `utilake.plan` | the hash of the conformance plan the shape was read by: which file column feeds which lake column, that column's id, type and nullability, how the value is parsed, the key and time columns, the load mode, the directory |
| `utilake.shape_id` | the id the census gives the shape: the block's report pair and columns, hashed with the census's own canonical bytes, so it can be looked up in the census |
| `utilake.schema_source` | where the table's definition came from: `registry` (built in), `user` (your own table file), `file` (nobody declared it, so the file's own header) |
| `utilake.resolution`, `utilake.table`, `utilake.schema_version` | how the shape matched: `conforms` (a run of the table's history has its columns), `deviates` or `derived`; the table, and the schema of its history the block was read with (AEMO's version is `source_version`) |

The plan hash changes when the output of that shape would: a column it
carries gets another type, a table gets a schema where it had none, the
schema it is read with changes. It does not
change when a description is edited, nor when the table gains a column the
shape does not carry: its files simply lack that column, which reads as
null. A source whose recorded hash is no longer what the current schemas
give is behind: the next `fetch` or
`ingest` that covers it reads it again instead of skipping it, and
`utilake ingest` with no arguments does that for every source in the
manifest, straight from the archive. The Parquet files the earlier read
wrote are replaced, and the run reports each such source as
`manifest.rules_changed`. `utilake ingest --dry-run` lists what is behind
and changes nothing.

## The archive of downloads

Every file utilake downloads is kept exactly as the server sent it, at the
path its URL spells out:

```
~/.utilake/archive/www.nemweb.com.au/Data_Archive/Wholesale_Electricity/MMSDM/2024/MMSDM_2024_01/MMSDM_Historical_Data_SQLLoader/DATA/
  PUBLIC_DVD_DISPATCHPRICE_202401010000.zip
  PUBLIC_DVD_DISPATCHPRICE_202401010000.zip.json
```

The `.json` beside it records where the file came from: the URL, when the
download completed, the server's `ETag` and `Last-Modified` (when AEMO
published it), the time the directory listing showed, the size and sha256
of the bytes, and the utilake version that fetched it.

Times in it, and in every record utilake writes: every time field is RFC
3339 with an offset, or stands beside one that is. `fetched_at` is when
the download completed, UTC. `last_modified` and `listed_modified` are
kept as the server and the listing wrote them; `last_modified_at` and
`listed_modified_at` beside them are the same times with their offset: the
HTTP date in UTC (`Z`), the listing's time in AEST (`+10:00`: nemweb lists
in Australian Eastern Standard Time, which has no daylight saving). A
sidecar written before the `_at` fields were kept lacks them;
`Archive::read_sidecar` derives them from the text beside each
(`Sidecar::derived`), and the sidecar on disk is left as it is. The
convention holds everywhere a time comes in as text: the field keeps the
text as it came, and its twin `{field}_at` holds the instant (`modified`
and `modified_at` in listing output, `last_modified_at` in the manifest's
`files`); a field whose name already ends in `_at` gets `{field}_utc`,
the instant in UTC (the WEM manifest's `generated_at_utc`). A time
written without an offset is a defect.

A file is downloaded once. Nothing in utilake replaces or deletes an
archived file; to fetch one again, delete it and its record. Everything in
the lake is derived from the archive, which is what `utilake ingest` does
when a table's schema improves. `utilake verify --archive` hashes every
file against its record.

## The mirror

`utilake-mirror` (`crates/utilake-mirror`) keeps a copy of every file under
nemweb's archive folders and a catalog of it: it crawls every listing page
under `Data_Archive/`, `Reports/ARCHIVE/` and `Reports/CURRENT/`, peeks
into each zip's central directory before downloading, downloads every
listed file once at a paced rate (a refusal is waited out), keeps every
version of a file nemweb rewrites in place, watches the folders on a
schedule of their own after the crawl, maps every zip as a folder, and
compares each month's zip with its folder so most zips are never
downloaded. Its home is `UTILAKE_MIRROR_HOME` (default `~/.utilake-mirror`):
the archive, with a sidecar beside every file, the journal and the catalog.
`crates/utilake-mirror/README.md` has the commands and the layout on disk;
the census in utilake-schemas and `utilake` read what it keeps.

## Our schemas: the shared histories

A table's schemas are its history as an Iceberg table, which
utilake-schemas folds from its census of every file AEMO published and
publishes as the table's Iceberg table metadata,
`nem/{TABLE}/metadata/v1.metadata.json`, with a manifest of every file's
hash (github.com/g0itb4/utilake-schemas; its README describes what the
metadata holds). utilake keeps a copy, taken at one commit and never
edited:

```
schemas/nem/pulled/
  SOURCE                                       the utilake-schemas commit it was taken at
  manifest.json                                the sha256 of every file published then
  DISPATCH_UNIT_SCADA/metadata/v1.metadata.json  the table: its schemas, its runs as snapshots, its declaration
  ...
```

`just schemas-pull REV` takes the copy at a commit of a utilake-schemas
checkout (`../utilake-schemas` unless you name another), whatever its
working tree holds. `just schemas-check`, a hook and part of `just check`,
holds every file to the manifest and `SOURCE` to the commit `Cargo.lock`
takes the `utilake-iceberg` crate from, so the histories and the code that
reads them are always of one commit. A newer census is a pull and a bump of
that rev, together.

The registry reads each table through `utilake_iceberg::from_metadata`.
The table's columns are the history's fields, by id, each typed by its
Iceberg type; a column takes nulls unless every schema requires it; the
schema a reader gets today is the metadata's `current-schema-id`, which
`utilake get` targets unless `--as-of` names another. A block is read with
the schema `History::resolve` names for its report pair, AEMO version,
columns in order and the month its source was released in (with no month
known, the latest run's): of the runs whose schema has exactly its columns,
those of its version first, the last released by that month, else the
first. Such a block conforms; the census guarantees every block it saw
does. A block no run fits deviates: it is kept whole (a column the table
lacks as text of its own, a column the file lacks as null), compared with
the schema in force for its pair then and reported, and `--strict` refuses
it. A renamed column is a new column. A table with a breaking column (one
name under two fields) is refused until utilake reads a column by more
than one field.

Where AEMO's files have columns the history has not seen, the fix is the
census's, in utilake-schemas; until then a table of your own reads them.

## Your own tables

A table of your own goes in `~/.utilake/schemas/nem/{TABLE}/`. Every command
uses it: one named after a built-in table replaces it, any other adds a
table. `--no-user-schemas` runs without them. What was ingested before the
table existed is brought up to it by `utilake ingest`.

It is either a `table.json` of the pulled form, such as a newer one from
utilake-schemas, or a `table.toml` written by hand. `utilake schema show T`
prints any table as one, a start for your own:

```toml
schema_format = 4
name = "DISPATCH_UNIT_SCADA"
report_pairs = [["DISPATCH", "UNIT_SCADA"]]
load_mode = "append"
primary_key = ["SETTLEMENTDATE", "DUID"]
time_column = "SETTLEMENTDATE"

column = [      # one per lake column, ever; an id is never reused
    { id = 1, name = "SETTLEMENTDATE", type = "timestamp", nullable = false },
    { id = 2, name = "DUID", type = "string", nullable = false },
    { id = 3, name = "SCADAVALUE", type = "decimal(16,6)", nullable = true },
    { id = 4, name = "LASTCHANGED", type = "timestamp", nullable = true },
]

[temporal]
origin = "derived"
entity = ["DUID"]
valid = { kind = "interval_end", column = "SETTLEMENTDATE" }
order = ["LASTCHANGED"]
cutoff = "LASTCHANGED"

[[schema]]      # one per column list its files carry
id = 1
from = "2024-10"       # the month its files start; the NEM's first if left out
versions = [1]         # the AEMO versions they come under
columns = ["SETTLEMENTDATE", "DUID", "SCADAVALUE", "LASTCHANGED"]   # file order
```

Its schemas become the table's history, one run per schema, report pair
and version from `from`, and its blocks resolve through it as a pulled
table's do. A `table.toml` of an older format, with `[[increment]]`
lists, is refused with how to write it now.

## The census of every shape

The census of every file AEMO published, each table's Iceberg history folded
from it, and the export of each table's metadata live in the utilake-schemas repository
(github.com/g0itb4/utilake-schemas): MMSDM months and the report folders
(`Reports/ARCHIVE`, `Reports/CURRENT`), including each version of a file
nemweb rewrote in place. utilake reads the histories it publishes
(`schemas/nem/pulled/`) with its `utilake-iceberg` crate, the Iceberg
contract both repositories share, at the rev `Cargo.toml` pins. Every block
utilake reads carries the census's id of its shape (`utilake.shape_id`).

## Which months

The archive begins in July 2009. From 2015 each month is a directory of
per-table zips and utilake downloads the ones asked for. Before 2015 a month
exists only as one zip of 1.4 to 2.1 GB holding those per-table zips;
utilake downloads it whole, once, and reads the per-table zips nested in it
from disk.

File names changed along the way, from `PUBLIC_DVD_{TABLE}_{YYYYMM}010000.zip`
to `PUBLIC_ARCHIVE#{TABLE}#FILE{NN}#{YYYYMM}010000.zip`; both are read.

The histories cover the census's months, July 2009 to April 2026, so
every file of those months resolves to the schema its run has, the `_ALL`
files of the P5MIN tables included. A later file whose columns no run has
deviates until the census reaches its month: of August 2026, the files of
22 tables do (LASTCHANGED on the FPP tables, a new column or three on
BILLINGCALENDAR, BILLINGRUNTRK, GENCONDATA and NEGATIVE_RESIDUE). The 39
tables the census found that the data model does not declare are not in
the registry; their files are kept as text, as for any table nobody
declares.

AEMO's web server blocks a client that asks too fast. utilake leaves 250 ms
between requests (`request_interval_ms` in `config.toml`), and when the
server refuses it (HTTP 403, or 429 through every retry) it sends nothing
more in that run and reports `net.blocked` for what is left.

A download that breaks leaves nothing behind; the next attempt fetches the
file whole. Its bytes go from the network to disk without a copy, hashed on
the way on a thread of their own.

## Errors

Every error and warning carries a stable code (`csv.field_count_mismatch`,
`net.content_length_mismatch`, `schema.deviates`, ...), a full-sentence
message and the context: the request, the URL, the archived file, the zip entry,
the table and version, the line and byte offset, and a sample of the offending
input. A run attempts every file and reports each failure; the exit code says
whether all, some or none succeeded (0, 3, 1), 4 for a file not archived when offline, 5
for a schema that could not be resolved in strict mode. `docs/errors.md` lists
every code.

Errors and warnings are printed as JSON, one object a line, as they happen
on stderr and again in the summary and in `utilake errors`:

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
every table the data model declares (241) in the registry, each with the
Iceberg history utilake-schemas folds from the census of every file,
downloads that resume where they stopped, and a lake that can be read
again from the archive when a schema changes. Next: live 5-minute feeds
and compaction; then WEM.

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
