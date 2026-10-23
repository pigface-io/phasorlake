# The semantics of a read

The lake keeps every row of every file. `utilake read` turns an interval
of one table into one frame: every file projected onto one schema, one
row per fact, the versions in force across the window. The rules come
from the table's published roles, so the same kernel reads every declared
table of every market and no table needs code of its own.

## Two time axes

AEMO's data has two clocks:

- **Valid time**: when a fact applies. `SETTLEMENTDATE`,
  `INTERVAL_DATETIME`, `EFFECTIVEDATE`, `TRADINGDATE`.
- **Knowledge time**: when a row was recorded or could be read.
  `LASTCHANGED`, `RUN_DATETIME`, and failing those the time the source
  file was published.

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
| `valid` | when it applies, and the axis kind: `interval_end`, `instant`, `from`, `timeless` | `SETTLEMENTDATE`, `interval_end` |
| `order` | which revision is newer, most significant first | `RUNNO`, `LASTCHANGED` |
| `cutoff` | the column a read "as known at" is held to; none: the source's publish time | `LASTCHANGED` |

`INTERVENTION` is always an entity: AEMO publishes an intervention run's
rows beside the normal run's, and merging them would lose one.

## The window

What `--from A --to B` keeps depends on the axis. A time that ends an
interval is the end of the period the row covers: the row at `00:05` is
the first of the day and the row at `24:00` its last, so a day is
`(midnight, next midnight]`.

| axis | `--from A --to B` | `--at T` |
|---|---|---|
| `interval_end` | `A < t <= B` | refused (the interval containing `T` needs a length the registry does not hold) |
| `instant` | `A <= t < B` | `t = T` |
| `from` | the version in force at `A`, and every later version until `B` | the version in force at `T` |
| `timeless` | every row (a warning says so) | every row |
| no roles (a table derived from a file) | `A <= t < B` on the time column, raw only | refused |

On a `from` table each returned version carries `_nl_valid_to`: the next
valid time of the same entity, null for the version in force now. AEMO's
"no end" dates (31 December 2999 and 9999) are open ends. A later join to
interval data is an as-of join over `[valid, _nl_valid_to)`.

Times on the command line are read in the table's market clock: a bare
date is that day's midnight there; RFC 3339 with an offset is taken as
written.

## Resolutions

- `--latest` (the default): one row per (entity…, valid). The row with
  the greatest (order…, source) wins. A null in an order column loses to
  any value; a NaN is left out and reported (`query.order_nan`).
- `--known-at K`: as `--latest`, over the rows whose known time is at or
  before `K`. The known time is the cutoff column's value, else the
  source's publish time. A source with neither cannot be shown known by
  `K`, so it is left out with `query.stamp_unknown`.
- `--revisions`: every revision of every fact, each once: one row per
  (entity…, valid, order…). A revision published twice (a DVD and an
  archive copy of one month, a live report and the settled month,
  `DATA` and `*_ALL_DATA`) is one row, its latest-published copy.
- `--raw`: every row of every file as written, projected onto the schema.

The source is the last tiebreak, in a total order: by publish time (a
source with none first), then URL, then entry. So a permutation of the
files read cannot change a winner, and reading the same month again
cannot double a fact.

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
| `_nl_source` | the source of the row, `url!entry` |
| `_nl_known` | when the row was known: the cutoff value, else the publish time |
| `_nl_valid_to` | on a `from` table, where the version stops; null for open |

The `_nl_` prefix is reserved: no table may declare such a column. A raw
read carries `_nl_source` only. `-o` writes Parquet (field ids and
`utilake.*` footer keys: `version`, `table`, `schema_id`, `resolution`,
`window`, `known_at`), CSV or an Arrow IPC file, and a JSON sidecar
beside it with the window, the schema, the resolution, every source with
its publish time, and the counts of rows read, returned and left out.

## Memory

A table partitioned by its valid time (append tables whose valid column
is the time column) resolves one month partition at a time, because the
key holds the valid time and a fact never spans two. Snapshot and
`from` tables, and tables whose valid time is not the partition column,
resolve whole.

## The SQL oracle

`utilake sql` renders the same read as one DuckDB query: the files
projected and unioned, each row joined to its source's publish time,
`QUALIFY row_number()` over the fact with the same ranking, `lead()`
over distinct valid times for `_nl_valid_to`, then the window. It is an
independent statement of the roles: `just oracle-test` runs it through
DuckDB and compares the rows with the kernel's on a fixture lake of every
pattern above.

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
