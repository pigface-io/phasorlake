# The semantics of the lake

The lake keeps every row of every file: every revision of every fact, and
every copy of it from every source that published it. This page says what
a table's columns mean in time, the roles that say which rows are
revisions of one fact, and what each row records of where it came from.
`docs/reading.md` turns these into queries. The rules come from each
table's published roles, so the same queries read every declared table of
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

What a window from `A` to `B` keeps depends on the axis. A time that ends
an interval is the end of the period the row covers: the row at `00:05`
is the first of the day and the row at `24:00` its last, so a day is
`(midnight, next midnight]`. A day label is written at midnight but
stands for the market's trading day, which starts at `s` after midnight
(04:00 in the NEM, 08:00 in the WEM, `trading_day_start_secs` in
`market.toml`): the label `L` covers `[L + s, L + s + 1 day)`.

| axis | from `A` to `B` | at `T` |
|---|---|---|
| `interval_end` | `A < t <= B` | the interval containing `T` needs a length the registry does not hold |
| `instant` | `A <= t < B` | `t = T` |
| `day` | the labels whose trading day overlaps `[A, B)`: `L + s < B` and `L + s + 1 day > A` | the label whose trading day holds `T` |
| `from` | the version in force at `A`, and every later version until `B` | the version in force at `T` |
| `timeless` | every row | every row |

`fetch --day D` fetches the trading day of `D`: from `D + s` to `D + s +
1 day`, which on a `day` table is that one label, and on an interval
table the 288 intervals from `04:05` to `04:00` the next day.

On a `from` table a version ends where the next valid time of the same
entity begins, or at the end column's value when the table has one and it
comes first; the version in force now has no end. AEMO's "no end" dates
(31 December 2999 and 9999) are open ends, and an end before the
version's own start says nothing.

## Revisions and copies

A fact is one (entity…, valid). Its revisions are its rows that differ in
the order columns; the newest has the greatest (order…). A null in an
order column loses to any value, and a NaN must be left out, since an
engine may sort it above every number. A revision is known from the later
of its cutoff value and the time its source was public, never before it
went public: a rebid AEMO stamped at 09:00 but published in a file at
09:05 is not known at 09:02.

A revision published by two sources (a DVD and an archive copy of one
month, a live report and the settled month, `DATA` and `*_ALL_DATA`) is
two copies of one row. Copies rank by the server's time (a source with
none first), then the source: a total order, so a permutation of the
files read cannot change which copy wins, and reading the same month
again cannot double a fact.

A table with no key and no valid time collapses only rows equal in every
column.

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
utilake: DuckDB, Polars, a copy or compaction of the files.
`docs/reading.md` gives the queries, including an as-of read known by the
server's word and one known by the earliest of the three.

Each file's footer counts its rows of each source (`utilake.sources`),
and the manifest keeps what each entry gave (`ingest_entries`); `utilake
verify` holds the two to each other. A file without the four columns was
written before lake format 3 (`PLAN_FORMAT` 7), and `utilake ingest`
writes it again.
