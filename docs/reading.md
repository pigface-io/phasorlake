# Reading the lake

utilake writes the lake and does not read it. Every row of every file is
kept: every revision, every copy of a fact from every source that
published it, each with where it came from and every time that source was
stamped with (`docs/semantics.md`). "The latest", "every revision" and "as
known at a time" are questions you ask of those rows in your own SQL,
with your own engine. This page gives each question as one query.

Every block marked as an example below is run by the tests against a
small fixture lake through the DuckDB CLI, and its rows are checked, so
the queries on this page are the ones that work. Run them from the
utilake home (`{home}`, default `~/.utilake`), where `lake/` is.

## Where to read from

Each table is a directory of Hive-partitioned Parquet,
`lake/{market}/{table}/`, with its Iceberg metadata beside the files in
`metadata/` (kept current by every ingest). Two ways in:

- **The Iceberg table**, `iceberg_scan('lake/nem/dudetail')` in DuckDB
  (the `iceberg` extension), or any Iceberg reader. It reads every column
  by its field id, so across a schema break (a column whose type changed
  is a new field under the same name) older files give null for the new
  field, as the table's history says.
- **The Parquet files**, `read_parquet('lake/nem/dudetail/**/*.parquet',
  hive_partitioning = true, union_by_name = true)`. It reads columns by
  name: across a schema break the engine unifies the two types instead,
  which may silently widen or fail. The examples below read this way
  because it needs nothing but Parquet.

## A table's roles

The queries are the same for every table but for its columns.
`utilake list TABLE` (or `--json`) names them:

| role | what it is | DISPATCHPRICE |
|---|---|---|
| entity | what a fact is about | `REGIONID`, `DISPATCHINTERVAL`, `INTERVENTION` |
| valid | when it applies, and its kind | `SETTLEMENTDATE`, `interval_end` |
| order | which revision is newer, most significant first | `RUNNO`, `LASTCHANGED` |
| cutoff | the column a row's known time is held to | `LASTCHANGED` |

A fact is one (entity…, valid). Its revisions differ in the order
columns. The same revision published by two sources (a live report and
the monthly DVD, a DVD and its archive copy) is two copies of one row.

## The latest revision of each fact

Rank the rows of each fact by the order columns, newest first, then by
the source's server time and the source itself so that copies of one
revision always rank the same way, and keep the first. A null in an order
column loses to any value.

<!-- example: latest -->
```sql
SELECT SETTLEMENTDATE, INTERVENTION, RUNNO, RRP, _ul_source
FROM read_parquet('lake/nem/dispatchprice/**/*.parquet', hive_partitioning = true, union_by_name = true)
WHERE SETTLEMENTDATE > TIMESTAMPTZ '2024-01-01 00:00:00+10:00'
  AND SETTLEMENTDATE <= TIMESTAMPTZ '2024-01-01 00:15:00+10:00'
QUALIFY row_number() OVER (
    PARTITION BY REGIONID, DISPATCHINTERVAL, INTERVENTION, SETTLEMENTDATE
    ORDER BY RUNNO DESC NULLS LAST, LASTCHANGED DESC NULLS LAST,
             _ul_server_last_modified DESC NULLS LAST, _ul_source DESC) = 1
ORDER BY SETTLEMENTDATE, INTERVENTION
```

The window here is that of an `interval_end` table: a time ends the
interval the row covers, so a day is `(midnight, next midnight]` and the
row at 00:05 is its first. An `instant` table takes `[from, to)`.

## Every revision, each once

The same ranking with the order columns in the partition: one row per
revision, its copies collapsed to the one published last.

<!-- example: revisions -->
```sql
SELECT SETTLEMENTDATE, INTERVENTION, RUNNO, RRP
FROM read_parquet('lake/nem/dispatchprice/**/*.parquet', hive_partitioning = true, union_by_name = true)
WHERE SETTLEMENTDATE > TIMESTAMPTZ '2024-01-01 00:00:00+10:00'
  AND SETTLEMENTDATE <= TIMESTAMPTZ '2024-01-01 00:15:00+10:00'
QUALIFY row_number() OVER (
    PARTITION BY REGIONID, DISPATCHINTERVAL, INTERVENTION, SETTLEMENTDATE, RUNNO, LASTCHANGED
    ORDER BY _ul_server_last_modified DESC NULLS LAST, _ul_source DESC) = 1
ORDER BY SETTLEMENTDATE, INTERVENTION, RUNNO
```

## As known at a time: no look-ahead

For a backtest or a model's features, a row may be used only once it was
public. A row is known from the later of its cutoff value and the time
its source was public; filter on that before ranking, so a revision
published after the time cannot win. With the server's `Last-Modified`
as the time a source was public, nothing is ever early:

<!-- example: known-at -->
```sql
SELECT SETTLEMENTDATE, RRP, _ul_source
FROM read_parquet('lake/nem/dispatchprice/**/*.parquet', hive_partitioning = true, union_by_name = true)
WHERE SETTLEMENTDATE > TIMESTAMPTZ '2024-01-01 00:00:00+10:00'
  AND SETTLEMENTDATE <= TIMESTAMPTZ '2024-01-01 00:15:00+10:00'
  AND greatest(LASTCHANGED, _ul_server_last_modified) <= TIMESTAMPTZ '2024-01-01 00:11:00+10:00'
QUALIFY row_number() OVER (
    PARTITION BY REGIONID, DISPATCHINTERVAL, INTERVENTION, SETTLEMENTDATE
    ORDER BY RUNNO DESC NULLS LAST, LASTCHANGED DESC NULLS LAST,
             _ul_server_last_modified DESC NULLS LAST, _ul_source DESC) = 1
ORDER BY SETTLEMENTDATE
```

`greatest` skips nulls, so a source no server stamped (a file imported
from disk) is known from its cutoff alone, and a row with neither is
never known.

The server's word can be very late: AEMO re-touched most of its archive in
April 2026, so an archived copy says 2026 however old its rows are. The
zip entry's own time and the creation stamp in a file name are earlier
evidence (`docs/semantics.md` says how far each can be trusted). Known by
the earliest of the three:

<!-- example: known-at-earliest -->
```sql
SELECT SETTLEMENTDATE, RRP, _ul_source
FROM read_parquet('lake/nem/dispatchprice/**/*.parquet', hive_partitioning = true, union_by_name = true)
WHERE SETTLEMENTDATE > TIMESTAMPTZ '2024-01-01 00:00:00+10:00'
  AND SETTLEMENTDATE <= TIMESTAMPTZ '2024-01-01 00:15:00+10:00'
  AND greatest(LASTCHANGED, least(_ul_server_last_modified, _ul_zip_entry_modified,
                                  _ul_filename_timestamp))
      <= TIMESTAMPTZ '2024-01-01 00:11:00+10:00'
QUALIFY row_number() OVER (
    PARTITION BY REGIONID, DISPATCHINTERVAL, INTERVENTION, SETTLEMENTDATE
    ORDER BY RUNNO DESC NULLS LAST, LASTCHANGED DESC NULLS LAST,
             _ul_server_last_modified DESC NULLS LAST, _ul_source DESC) = 1
ORDER BY SETTLEMENTDATE
```

Which evidence to trust is your choice; the ranking of copies stays by
the server's time either way, so the choice changes what is known, never
which copy of a revision wins.

## The version in force: standing data

A `from` table (DUDETAIL, a unit's registration) holds versions, each in
force from its valid time until the next version of the same entity.
Take the latest revision of each version, give each its end with
`lead`, and keep the one whose span holds the time. Read every file of
the table: the version that ends one may sit in a later file than the one
it ends, and leaving that file out would leave the earlier version open.

<!-- example: in-force -->
```sql
WITH versions AS (
    SELECT DUID, EFFECTIVEDATE, VERSIONNO, REGISTEREDCAPACITY
    FROM read_parquet('lake/nem/dudetail/**/*.parquet', hive_partitioning = true, union_by_name = true)
    QUALIFY row_number() OVER (
        PARTITION BY DUID, EFFECTIVEDATE
        ORDER BY VERSIONNO DESC NULLS LAST, LASTCHANGED DESC NULLS LAST,
                 _ul_server_last_modified DESC NULLS LAST, _ul_source DESC) = 1
), spans AS (
    SELECT *, lead(EFFECTIVEDATE) OVER (PARTITION BY DUID ORDER BY EFFECTIVEDATE) AS valid_to
    FROM versions
)
SELECT DUID, EFFECTIVEDATE, VERSIONNO, REGISTEREDCAPACITY, valid_to
FROM spans
WHERE EFFECTIVEDATE <= TIMESTAMPTZ '2024-01-15 00:00:00+10:00'
  AND (valid_to IS NULL OR valid_to > TIMESTAMPTZ '2024-01-15 00:00:00+10:00')
ORDER BY DUID
```

AEMO's "no end" dates (31 December 2999 and 9999) are open ends. A table
with an end column (DUDETAILSUMMARY's `END_DATE`) ends a version at the
earlier of that and the next version. A join of interval data to
standing data is an as-of join over `[EFFECTIVEDATE, valid_to)`.

## A NaN order value

Some order columns are doubles (`VERSIONNO` of the bid tables). DuckDB
sorts NaN above every number, so a NaN revision would win the ranking.
Leave it out before ranking:

<!-- example: order-nan -->
```sql
SELECT SETTLEMENTDATE, DUID, BIDTYPE, VERSIONNO, PRICEBAND1
FROM read_parquet('lake/nem/biddayoffer_d/**/*.parquet', hive_partitioning = true, union_by_name = true)
WHERE VERSIONNO IS DISTINCT FROM CAST('NaN' AS DOUBLE)
QUALIFY row_number() OVER (
    PARTITION BY DUID, BIDTYPE, SETTLEMENTDATE
    ORDER BY OFFERDATE DESC NULLS LAST, VERSIONNO DESC NULLS LAST, LASTCHANGED DESC NULLS LAST,
             _ul_server_last_modified DESC NULLS LAST, _ul_source DESC) = 1
ORDER BY SETTLEMENTDATE, DUID, BIDTYPE
```

## A trading day

A `day` table (the bid tables) labels each row with its trading day,
written at midnight. The NEM's trading day runs from 04:00 to 04:00 the
next day (the WEM's from 08:00), so the label 2024-01-01 covers 04:00
on the 1st to 04:00 on the 2nd. Select a trading day by its label:

<!-- example: trading-day -->
```sql
SELECT DUID, BIDTYPE, count(*) AS revisions
FROM read_parquet('lake/nem/biddayoffer_d/**/*.parquet', hive_partitioning = true, union_by_name = true)
WHERE SETTLEMENTDATE = TIMESTAMPTZ '2024-01-01 00:00:00+10:00'
GROUP BY DUID, BIDTYPE
ORDER BY DUID, BIDTYPE
```

## A table with no key

A table with no entity and no valid time (SSM_ENABLEMENT_COSTS) has no
facts to rank: two rows are the same only when every column is. Collapse
copies with `DISTINCT` over the table's own columns:

<!-- example: keyless -->
```sql
SELECT DISTINCT * EXCLUDE (_ul_source, _ul_server_last_modified, _ul_zip_entry_modified,
                           _ul_filename_timestamp)
FROM read_parquet('lake/nem/ssm_enablement_costs/**/*.parquet', union_by_name = true)
ORDER BY ENABLEMENT_REASON
```

## Where a result came from

Every row names its source, so any result can say which files it came
from and when each was public:

<!-- example: sources -->
```sql
SELECT _ul_source, count(*) AS rows, any_value(_ul_server_last_modified) AS public_by_server
FROM read_parquet('lake/nem/dispatchprice/**/*.parquet', hive_partitioning = true, union_by_name = true)
GROUP BY _ul_source
ORDER BY _ul_source
```

## The values of an entity column

<!-- example: entity-values -->
```sql
SELECT DUID, count(*) AS rows, min(EFFECTIVEDATE) AS first, max(EFFECTIVEDATE) AS last
FROM read_parquet('lake/nem/dudetail/**/*.parquet', hive_partitioning = true, union_by_name = true)
GROUP BY DUID
ORDER BY DUID
```

## The step between intervals

The most common gap between distinct valid times is a table's interval
(five minutes for dispatch, thirty for trading):

<!-- example: interval-step -->
```sql
WITH times AS (
    SELECT DISTINCT SETTLEMENTDATE
    FROM read_parquet('lake/nem/dispatchprice/**/*.parquet', hive_partitioning = true, union_by_name = true)
), gaps AS (
    SELECT SETTLEMENTDATE - lag(SETTLEMENTDATE) OVER (ORDER BY SETTLEMENTDATE) AS step FROM times
)
SELECT step, count(*) AS seen FROM gaps WHERE step IS NOT NULL GROUP BY step ORDER BY seen DESC, step
```

## Market time

Times in the lake are instants (`timestamptz`). DuckDB shows them in the
session's zone; to see them as the market does:

```sql
SET TimeZone = 'Australia/Brisbane';
```

## What the lake leaves to you

- Joins: an as-of join of a fact table to its standing data by shared
  entity columns, or an exact join on a version pointer
  (`GENCONID_EFFECTIVEDATE` + `GENCONID_VERSIONNO`).
- A full grid of every interval × every entity, with nulls where nothing
  was published.
- Which intervention run to use: both are kept; filter on `INTERVENTION`.
- Aliases (a battery's charge and discharge units, a renamed DUID) and
  shifts in time.
- Writing a result down: the lake is the only Parquet utilake writes. A
  saved result goes stale when the lake gains a revision, so derived
  datasets belong to a layer of their own that records which lake
  snapshot each was built from.
