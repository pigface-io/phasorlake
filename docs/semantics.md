# The semantics of a read

The lake keeps every row of every file. A read turns an interval of one
table into one frame: every file projected onto one schema, one row per
fact, the versions in force across the window. The rules come from the
table's published roles, so the same rules read every declared table of
every market and no table needs code of its own.

## Two time axes

AEMO's data has two clocks:

- **Valid time**: when a fact applies. `SETTLEMENTDATE`,
  `INTERVAL_DATETIME`, `EFFECTIVEDATE`, `TRADINGDATE`.
- **Knowledge time**: when a row could be read. Never before its source
  went public (`_ul_server_last_modified`, or the earlier evidence a
  reader trusts), and no earlier than `LASTCHANGED` or `RUN_DATETIME`
  say where those are later.

Dedupe, "as of" reads, effective-date windows and forecast vintages are
one question asked of these axes: which revision of a fact is the truth,
as known at some time.

## The roles

Every declared table carries roles in its Iceberg metadata
(`utilake.temporal.*`), derived from AEMO's primary key and the table's
columns (`crates/utilake-mms-registry/src/file_model/temporal.rs`):

| role | meaning | example (DISPATCHPRICE) |
|---|---|---|
| `entity` | what a fact is about; two rows of one entity and valid time are revisions of one fact | `REGIONID`, `DISPATCHINTERVAL`, `INTERVENTION` |
| `valid` | when it applies, and the axis kind: `interval_end`, `instant`, `day`, `from`, `timeless` | `SETTLEMENTDATE`, `interval_end` |
| `valid.until` | on a `from` table, the column that ends a version early (`END_DATE`, `VALIDTO`); AEMO's "no end" dates are open | DUDETAILSUMMARY: `END_DATE` |
| `order` | which revision is newer, most significant first | `RUNNO`, `LASTCHANGED` |
| `cutoff` | the column a read "as known at" is held to; none: the source's server time | `LASTCHANGED` |

`INTERVENTION` is always an entity: AEMO publishes an intervention run's
rows beside the normal run's, and merging them would lose one.

The roles are one rule, shared with the publisher
(`utilake_iceberg::derive`), run over the key and the columns; a table
whose declaration carries a `[temporal]` block has roles a person set
(`origin = hand`). The `day` kind is such a choice: the census shows a
valid column seen only at midnight, and the fold says so.

## The window

What `--from A --to B` keeps depends on the axis. A time that ends an
interval is the end of the period the row covers: the row at `00:05` is
the first of the day and the row at `24:00` its last, so a day is
`(midnight, next midnight]`. A day label is written at midnight but
stands for the market's trading day, which starts at `s` after midnight
(04:00 in the NEM, 08:00 in the WEM, `trading_day_start_secs` in
`market.toml`): the label `L` covers `[L + s, L + s + 1 day)`.

| axis | `--from A --to B` | `--at T` |
|---|---|---|
| `interval_end` | `A < t <= B` | refused (the interval containing `T` needs a length the registry does not hold) |
| `instant` | `A <= t < B` | `t = T` |
| `day` | the labels whose trading day overlaps `[A, B)`: `L + s < B` and `L + s + 1 day > A` | the label whose trading day holds `T` |
| `from` | the version in force at `A`, and every later version until `B` | the version in force at `T` |
| `timeless` | every row (a warning says so) | every row |
| no roles (a table derived from a file) | `A <= t < B` on the time column, raw only | refused |

`--day D` on the command line is the trading day of `D`: `--from D+s
--to D+s+1 day`, which on a `day` table is that one label, and on an
interval table the 288 intervals from `04:05` to `04:00` the next day.

On a `from` table each returned version carries `_ul_valid_to`: the next
valid time of the same entity, or the end column's value when the table
has one and it comes first; null for the version in force now. AEMO's
"no end" dates (31 December 2999 and 9999) are open ends, and an end
before the version's own start says nothing. A later join to interval
data is an as-of join over `[valid, _ul_valid_to)`.

Times on the command line are read in the table's market clock: a bare
date is that day's midnight there; RFC 3339 with an offset is taken as
written.

## Resolutions

- `--latest` (the default): one row per (entity…, valid). The row with
  the greatest (order…, source) wins. A null in an order column loses to
  any value; a NaN is left out and reported (`query.order_nan`).
- `--known-at K`: as `--latest`, or with `--revisions` every revision,
  over the rows whose known time is at or before `K`. A row is known from
  the later of its source's server time (`_ul_server_last_modified`) and
  its cutoff value, never before it went public: a rebid AEMO stamped at 09:00 but published in a
  file at 09:05 is not known at 09:02. Where the server time is unknown
  the cutoff alone decides; a file with neither for any source cannot be
  shown known by `K`, so it is left out with `query.stamp_unknown`.
- `--revisions`: every revision of every fact, each once: one row per
  (entity…, valid, order…). A revision published twice (a DVD and an
  archive copy of one month, a live report and the settled month,
  `DATA` and `*_ALL_DATA`) is one row, its latest-published copy.
- `--raw`: every row of every file as written, projected onto the schema.

The source is the last tiebreak, in a total order: by server time (a
source with none first), then URL, then entry. Copies equal in all of it
fall to the row read last. So a permutation of the files read cannot
change a winner, and reading the same month again cannot double a fact.

## Provenance in every row

The lake writes every row it reads, every revision and every copy:
nothing is aggregated or dropped on the way in, and "latest" or "known
at" are questions asked of the rows when they are read. A lake file holds
the rows of one source file's entries (a zip's, cut at the file size in
`[lake]`), and every row carries where it came from and every time that
source was stamped with. Each time is its own column, named for the place
it was read from and transcribed as found. Nothing is combined, ranked or
estimated when the lake is written: which time a row counts as known by is
a question for the read.

| column | type | holds |
|---|---|---|
| `_ul_source` | dictionary of text | the source, `url!entry` (the URL alone for a file read whole) |
| `_ul_server_last_modified` | timestamp, UTC | the HTTP `Last-Modified` the web server gave for the source's bytes; null when no server gave one (a local file) |
| `_ul_zip_entry_modified` | timestamp, UTC | the entry's modified time in its zip's central directory (DOS date and time, 2 s resolution), read in the market's clock; null for a file read whole or a zip entry with no date |
| `_ul_filename_timestamp` | timestamp, UTC | the first run of exactly fourteen digits (`YYYYMMDDHHMMSS`) in the source's file name, read in the market's clock; null when the name has none |

What each one is, and how far from the moment the row went public it can
be:

- **`_ul_server_last_modified`** is the proof that the bytes were public:
  the server served them with this time. For the entry of a rollup that
  the mirror also holds from its own address (the record beside it names
  the rollup's entry as the same bytes), it is the entry's own
  `Last-Modified`; otherwise the source file's. It is never early. It can
  be very late: a monthly DVD is weeks after the rows it holds, and AEMO
  re-touched most of its archive in April 2026, so an archived file of
  2016 says 2026.
- **`_ul_zip_entry_modified`** is when AEMO's packer wrote the entry,
  seconds before the zip was uploaded. It is at or before the moment the
  row was public, usually by seconds. The zip format stores it without a
  time zone; it is read in the market's clock (AEST for the NEM's site,
  AWST for the WEM's), the clock of every AEMO time in the lake.
- **`_ul_filename_timestamp`** is AEMO's creation time to the second, as
  in `PUBLIC_YESTBID_201612220000_20161223040519.zip` or gas
  `int651_…~20261023113222.csv`, read in the market's clock. The twelve
  digit stamp most names carry is the interval the report is about, not
  when it went out, so it is not a time column: it can be a day before
  the report was public. A run of sixteen digits is a sequence number,
  not a time.

`LASTCHANGED` is when AEMO changed the row, not when it went out: bids
are public the next day. It bounds a row's known time from below only.

Rows of one source are contiguous, so each of the four columns is one run
per source per row group and costs almost nothing.

Because the rows carry them, any reader gets the same answers without
utilake: DuckDB, Polars, a copy or compaction of the files. An as-of read
for a backtest or a model's features:

```sql
-- every row public by :t, the latest copy of each fact: no look-ahead
SELECT * FROM read_parquet('lake/nem/dispatchprice/**/*.parquet')
WHERE greatest(LASTCHANGED, _ul_server_last_modified) <= :t
QUALIFY row_number() OVER (PARTITION BY REGIONID, DISPATCHINTERVAL, INTERVENTION, SETTLEMENTDATE
        ORDER BY RUNNO DESC NULLS LAST, LASTCHANGED DESC NULLS LAST,
                 _ul_server_last_modified DESC NULLS LAST, _ul_source DESC) = 1
```

The same read known by the earliest evidence instead of the server's
word: `least` skips nulls in DuckDB and Postgres, and a row with none of
the three is never known.

```sql
WHERE greatest(LASTCHANGED, least(_ul_server_last_modified,
        _ul_zip_entry_modified, _ul_filename_timestamp)) <= :t
```

Which evidence to trust is the reader's choice. The server's word is
the proof, and it is never early.

Each file's footer counts its rows of each source (`utilake.sources`),
and the manifest keeps what each entry gave (`ingest_entries`); `utilake
verify` holds the two to each other. A file without the four columns was
written before lake format 3 (`PLAN_FORMAT` 7), and `utilake ingest`
writes it again; a read refuses it with `query.lake_format`.

A table with no key and no valid time (39 undeclared and a few declared
ones) collapses only rows equal in every column: the floor, never the
rule.

## Filters and projection

`--where COL=v1,v2` on an entity column runs before resolution, so it
only removes whole facts. A filter on any other column runs after
resolution, with `query.filter_after_resolution` noting it: removing a
row by its value first could remove the revision that would have won.
`--columns` picks what is returned; the role columns are always read.

## What a frame carries

The target schema's columns (the current one, or `--as-of N`), each with
its lake field id, then:

| column | meaning |
|---|---|
| `_ul_source` | the source of the row, `url!entry` |
| `_ul_server_last_modified` | as in the lake: the server's `Last-Modified` for the source |
| `_ul_zip_entry_modified` | as in the lake: the entry's time in its zip's directory |
| `_ul_filename_timestamp` | as in the lake: the fourteen-digit stamp in the source's name |
| `_ul_known_from` | when the row was known: the later of its server time and its cutoff value |
| `_ul_valid_to` | on a `from` table, where the version stops; null for open |

The `_ul_` prefix is reserved: no table may declare such a column. A raw
read carries `_ul_source` and the three times.

A read is written as Parquet (field ids and `utilake.*` footer keys:
`version`, `table`, `schema_id`, `resolution`, `window`, `known_at`), CSV
or an Arrow IPC file. The path says the shape: one whose extension names
a format (`x.parquet`, `x.csv`, `x.arrow`) is one file of that format;
any other path is a directory of parts, `part-00000.parquet`,
`part-00001.parquet`, …, a new part beginning once the open one reaches
the target size (128 MiB; a Parquet part may run over by one row group).
Parquet is zstd at level 3 unless asked otherwise (`snappy`, `none`).
Each part is written whole under a `.tmp` name and renamed, so a part
that exists is complete. Beside the parts, `{TABLE}.json` (beside a
file, `{file}.json`) names the window, the schema, the resolution, every
source with its three times, every file written, the counts of rows
read, returned and left out, and, on an interval table, `intervals`: the
step read off the frame (the most common gap between distinct valid
times, as an ISO 8601 duration), every gap seen, how many distinct times
are present against how many the window holds at that step, and the
first twenty gaps between them. The registry declares no interval length
and a table's can change (TRADINGPRICE went from 30 to 5 minutes in
October 2021), so the step is measured, never assumed, and a window
across such a change shows both.

## Memory

A table partitioned by its valid time (append tables whose valid column
is the time column) resolves one month partition at a time, because the
key holds the valid time and a fact never spans two; each partition's
frame is written before the next is read, so a read holds one partition
in memory, however long the window. Snapshot and `from` tables, and
tables whose valid time is not the partition column, resolve whole.

## The same read as SQL

The read renders as one standard SQL query: the rows, each with its own
source and three times, `row_number()` over the fact with the same ranking
(the first of each fact kept), `lead()` over distinct valid times for
`_ul_valid_to`, then the window, with `TIMESTAMP WITH TIME ZONE` literals
to the microsecond. Only the first clause, where the rows come from, is of
a dialect: for DuckDB every lake file read in place with `read_parquet`
and projected onto the schema; for Postgres one relation named for the
table, already in the schema, whose rows carry `_ul_source` and
the three time columns as the lake files hold them. Everything after is the same
text.

It is an independent statement of the roles. Every test run runs the
DuckDB form through DuckDB and compares the rows with the kernel's on a
fixture lake of every pattern above (an oracle in the testing sense: the
reference answer). The Postgres form differs by that one clause and has
no run in CI, which would need a server and a loader.

## What a read does not do

- Join tables: an as-of join of a fact table to its standing data by
  shared entity columns, or an exact join on a version pointer
  (`GENCONID_EFFECTIVEDATE` + `GENCONID_VERSIONNO`), is a later step over
  resolved frames.
- Fill a grid: every interval × every entity with nulls where nothing was
  published needs an interval length the registry does not hold.
- Choose an intervention run: both survive; filter on `INTERVENTION`.
- Alias entities (a battery's charge and discharge units, a renamed DUID)
  or shift columns in time: consumer rules, never inferred by the lake.
- Cache a resolution: resolution is a pure function of the raw rows and
  the roles, so a compaction would only be an optimisation.
