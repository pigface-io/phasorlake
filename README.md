# utilake

AEMO market data into a local Parquet lake, from one Rust binary.
Pronounced *util-lake*: a lake for the utilities' data, the electricity
markets first.

`utilake` downloads the National Electricity Market's monthly archive files
from nemweb, parses their `C`/`I`/`D` row format through a schema registry
that knows every table version AEMO has published, and writes typed,
Hive-partitioned Parquet that DuckDB, Polars and anything else that reads
Parquet can query directly. A SQLite manifest records every file fetched,
every ingest, every output and every error, so a second run is a no-op and
a failure is explained.

## Quick start

```sh
cargo install --path crates/utilake-cli --locked
utilake fetch nem --table DISPATCH_UNIT_SCADA --from 2024-01
utilake ingest --from 2024-01 --table DISPATCH_UNIT_SCADA
```

`fetch` downloads into the archive, each file once; `ingest` writes Parquet
from what is archived, without the network.

The result lands under `~/.utilake/lake/nem/dispatch_unit_scada/year=2024/month=01/`.
Query it:

```sql
select count(*), typeof(settlementdate)
from read_parquet('~/.utilake/lake/nem/dispatch_unit_scada/**/*.parquet', hive_partitioning = true);
```

Run the same commands again and they finish in milliseconds: the archive has
the file, and the manifest knows it is already in the lake.

## Commands

| command | what it does |
|---|---|
| `utilake fetch nem --from YYYY-MM [--to YYYY-MM] [--table T,T] [--exclude T,T] [--dry-run]` | list the archive and download each file not archived yet, with the month's control files and README. Without `--table`: every table the archive has for those months. `--dry-run` lists the files and totals their size |
| `utilake ingest --from YYYY-MM [--to YYYY-MM] [--table T,T] [--exclude T,T]` | write Parquet from the archived files of those months, no network |
| `utilake ingest <zip\|csv\|dir>...` | ingest local files, no network |
| `utilake reingest [--dry-run]` | read again, from disk, every source whose lake files are behind the current schemas; no network |
| `utilake inspect <file>` | every block in a file, its version and how the registry resolves it |
| `utilake ls [--table T] [--failed] [--runs]` | what the manifest knows |
| `utilake errors [--last] [--table T] [--code C]` | diagnostics recorded by past runs |
| `utilake verify` | every Parquet file the manifest lists exists with its size |
| `utilake schema list\|show\|increments\|validate` | the registry; `increments T` lists a table's increments with the columns each adds or drops |
| `utilake schema plan <type> <subtype> <version> --columns A,B,C [--month YYYY-MM]` | the conformance plan a shape is read with, as JSON: where each file column goes, its cast, what is written null |
| `utilake archive ls\|verify` | every downloaded file with where and when it came from; `verify` hashes each one again |
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
  cache/tmp/                                                               scratch files of a running ingest, safe to delete
  quarantine/{table}/{source}.jsonl                                        rows whose structure was wrong for their block, with the reason
  manifest.sqlite                                                          runs, files, ingests, outputs, diagnostics
```

Every Parquet file carries the lake schema of its table: every column the
table has ever had, each with its id as `PARQUET:field_id`, so files of
every increment read as one table and a later Iceberg catalogue can adopt
the column ids unchanged.

Nothing a file carries is skipped. A table no schema declares is ingested
from its own header, every column as text, under
`lake/nem/_unregistered/{report type}.{report subtype}/`; a column a table's
schema does not have is kept beside the declared ones as text under its own
name; a column the file lacks is null. Each case is reported
(`schema.unknown_table`, `schema.deviates`). `--strict` refuses such files
instead.

Types are exact, as Parquet and Iceberg name them, and ours: a table's
`table.toml` says each column's type, whatever a later control file says.

| `type` | Arrow | Iceberg V3 | a new column gets it from |
|---|---|---|---|
| `string` | `Utf8` | `string` | `CHAR(n)` |
| `double` | `Float64` | `double` | `FLOAT`, `DECIMAL` and `INTEGER EXTERNAL` |
| `long` | `Int64` | `long` | (only by hand) |
| `boolean` | `Boolean` | `boolean` | (only by hand) |
| `date` | `Date32` | `date` | `DATE`/`TIMESTAMP` without an hour |
| `timestamp_ns` | `Timestamp(ns, +10:00)` | `timestamptz_ns` | `DATE`/`TIMESTAMP` with an hour |

Every number is a `double`: AEMO's control files load numbers as
`FLOAT EXTERNAL`, its data puts fractions in columns the data model
declares `NUMBER(p,0)`, and a `double` holds every whole number of up to 15
digits exactly. NEM time is fixed AEST.

Every timestamp in the lake is nanoseconds since the epoch in UTC, one unit
with no exceptions, and a fractional second is kept to the last digit, so a
later Iceberg table of the lake is format V3, the first with `timestamp_ns`
(see `docs/iceberg.md`). That unit spans 1677-09-21 to 2262-04-11. AEMO
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
| `utilake.source_period` | the month of the source, which decides the increment a block is read against |
| `utilake.plan` | the hash of the conformance plan the shape was read by: which file column feeds which lake column, that column's id, type and nullability, how the value is parsed, the key and time columns, the load mode, the directory |
| `utilake.shape_id` | the ids the census gives the shape (`just census`), when it has seen it |
| `utilake.schema_source` | where the table's definition came from: `registry` (built in), `user` (your own table file), `file` (nobody declared it, so the file's own header) |
| `utilake.resolution`, `utilake.table`, `utilake.schema_version` | how the shape matched: `conforms` (its columns are its increment's), `deviates` or `derived`; the table, and the increment (`TABLE@n`, our version; AEMO's is `source_version`) |

The plan hash changes when the output of that shape would: a column it
carries gets another type, a table gets a schema where it had none, the
increment it is read against changes. It does not
change when a description is edited, nor when the table gains a column the
shape does not carry: its files simply lack that column, which reads as
null. A source whose recorded hash is no longer what the current schemas
give is behind: the next `fetch` or
`ingest` that covers it reads it again instead of skipping it, and
`utilake reingest` does that for every source in the manifest, straight
from the archive. The Parquet files the earlier read wrote are replaced,
and the run reports each such source as `manifest.rules_changed`.
`utilake reingest --dry-run` lists what is behind and changes nothing.

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
the lake, and the census of every shape AEMO published (`just census`), is
derived from the archive, which is what `utilake reingest` does when a
table's schema improves. `utilake archive verify` hashes every
file against its record.

## Our schema, in increments

Each table is a directory under `schemas/nem/`:

```
schemas/nem/DISPATCH_UNIT_SCADA/
  table.toml      our schema: persists, hand-editable
  ctl/001.ctl     AEMO's control file of increment 1, byte for byte as archived
  ctl/002.ctl     ... of increment 2
```

```toml
schema_format = 2
name = "DISPATCH_UNIT_SCADA"
report_pairs = [["DISPATCH", "UNIT_SCADA"]]
load_mode = "append"
dedupe = "pk_lastchanged"
primary_key = ["SETTLEMENTDATE", "DUID"]
time_column = "SETTLEMENTDATE"
next_column_id = 5

column = [      # one per lake column, ever; an id is never reused
    { id = 1, name = "SETTLEMENTDATE", type = "timestamp_ns", nullable = false },
    { id = 2, name = "DUID", type = "string", nullable = false },
    { id = 3, name = "SCADAVALUE", type = "double", nullable = true },
    { id = 4, name = "LASTCHANGED", type = "timestamp_ns", nullable = true },
]

[[increment]]   # DISPATCH_UNIT_SCADA@2, in force from 2026-08 until the next
n = 2
from = "2026-08"
ctl = "002.ctl"
ctl_sha256 = "1ae68ab0..."
aemo_ctl = "PUBLIC_ARCHIVE#DISPATCH_UNIT_SCADA#FILE01#202608.ctl"
aemo_versions = [1]    # the I-row versions seen under it
columns = ["SETTLEMENTDATE", "DUID", "SCADAVALUE", "LASTCHANGED"]   # file order
```

The columns and their types are ours and persist. An increment is the file
shape one AEMO control file sets from a month on: our version, apart from
AEMO's. A block of month M is read against the increment in force at M
(with no month known: the increment with its columns, else the latest). It
conforms when its columns are the increment's; otherwise it deviates, is
kept whole (a column the table lacks as text of its own, a column the file
lacks as null) and is reported, and `--strict` refuses it. A renamed column
is a new column.

`just schema-sync FROM TO` follows the archive: a month whose control file
lays a table out anew appends an increment, with new columns given fresh
ids and their default types, and a table the schemas lack is started from
its control file, the data model and the census. It never changes what is
there. Where the files differ from their control file, the fix is yours,
in `table.toml`: edit the increment's `columns`, a column's `type`, or add a
column, with a `note` saying why. Types change only by an Iceberg promotion
(`just schema-compat`), and none of ours is one: a column of another type
is a new column.

AEMO's PDR control files can be kept under `reference/pdr/` for
comparison (`utilake-nem-census pdr-diff`); resolution never reads them.

## Your own tables

A table of your own goes in `~/.utilake/schemas/nem/{TABLE}/table.toml`,
laid out like the ones above (control files are optional there). Every
command uses it: one named after a built-in table replaces it, any other
adds a table. `--no-user-schemas` runs without them. What was ingested
before the table existed is brought up to it by `utilake reingest`.

## The census of every shape

utilake documents what AEMO published, from evidence. Every file of the
archive is downloaded once (`utilake fetch nem`), and `just census FROM TO`
reads each archived file of those months in full, offline, recording every
block (one per I row: a file of many tables has many) with its shape and
rows under `schemas/nem/census/` and `schemas/nem/shapes/`. That static map
is committed. A new month goes forward with `just census-forward YYYY-MM`:
download, census, sync the schemas to the month's control files, and gate
the census against them (`just census-check`).
Parquet is optional: `utilake ingest --from` writes it from the archive.

## Which months

The archive begins in July 2009. From 2015 each month is a directory of
per-table zips and utilake downloads the ones asked for. Before 2015 a month
exists only as one zip of 1.4 to 2.1 GB holding those per-table zips;
utilake downloads it whole, once, and reads the per-table zips nested in it
from disk.

File names changed along the way, from `PUBLIC_DVD_{TABLE}_{YYYYMM}010000.zip`
to `PUBLIC_ARCHIVE#{TABLE}#FILE{NN}#{YYYYMM}010000.zip`; both are read.

The increments come from the control files of every month from January
2015 to August 2026, built in one pass (`just schema-rebuild`), so each
starts in the month its layout first appeared; 2009 to 2014 follow once
those months are downloaded. Every file of August 2026 conforms to its
increment. In January 2024 the `_ALL` files of three P5MIN tables carry
other columns than the files their control files load, and five tables'
files carry columns their control file does not list; those files are
kept and flagged until a census of every month settles them. A file of
another month is read against the increment in force then.

AEMO's web server blocks a client that asks too fast. utilake leaves 250 ms
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
every table of the August 2026 and January 2024 archives (241) in the
registry, each in increments set by AEMO's control files, downloads that
resume where they stopped, and a lake that can be read again from the
archive when a schema changes. Next: download the whole archive and sync
the increments of every month from July 2009; then live 5-minute feeds
and compaction; then WEM and an Iceberg writer.

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
