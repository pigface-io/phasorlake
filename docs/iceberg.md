# The lake as Iceberg tables

Every table the lake holds is an Apache Iceberg table (format version 3).
One model runs through the census, the registry, the lake and the export:

| Term | What it is | Where |
|---|---|---|
| **Field** | An Iceberg field: an id never minted twice, a name, one type, whether it is required | `crates/utilake-iceberg/src/schema.rs` |
| **Schema** | The fields a file of the table holds, in file order, and the key; an id per distinct schema | `schemas/nem/iceberg/<TABLE>.json` |
| **Block** | One block of one AEMO file: its header, rows, data span and the profile of its values | `schemas/nem/census/` |
| **Run** | Consecutive blocks under one schema, AEMO version and report pair | the history's `runs` |
| **Snapshot** | One run of the lake's files, committed to the table | `{lake}/nem/<table>/metadata/` |

## The history: from the census to schemas

The fold (`crates/utilake-nem-census/src/history.rs`) reads every census
block and writes each table's history.

- **Order.** Blocks go in the order AEMO released them: release month (the
  MMSDM archive month, a report CSV's month), then where the block's data
  ends, then file, then block. Data time alone interleaves files that
  restate the past, and a file's recorded publication time is when the
  server last touched it.
- **One field per lineage.** A column's lineage is its blocks until its
  values turn from a number or a date to text. A lineage is one field: its
  id holds across files that leave the column out, so a table that
  alternates between two column lists has two schemas, not one per file.
  A new lineage is a new field, recorded as breaking with its file and
  time.
- **One type per field**, chosen from everything its lineage holds and the
  widest declaration of its months (control files, data-model releases):
  `long` for whole numbers declared whole or in the key, `decimal(P, S)`
  while 18 digits hold it, `double` past that or for floats printed in
  full, `date`, `timestamptz`, `string`; a column that only ever held
  nulls takes its declared type, else `string`. Choosing from the values
  seen so far would re-type a column whose first months were null and turn
  more digits after the point into a new field, since Iceberg cannot widen
  a scale.
- **Required** only for a key column, until a block holds a row without it.
- **Interned.** A schema with the same fields, order and key as an earlier
  one is that one, with its id.

Each run records what changed at its start (`schema`, `version`,
`report`), the updates from the run before (`add`, `restore`, `delete`,
`move`, `make-optional`, `set-identifier-fields`), the kinds of file it
came in, and whether its data overlaps the run before.

## Resolution and the lake

The registry embeds the histories (`iceberg.bin`). A block resolves to the
schema whose columns, in order, are the block's, and conforms;
`table.toml`'s columns are the history's fields, so a plan writes every
value under its Iceberg field id (`PARQUET:field_id`), and validation
refuses a column whose id or type is not its field's. Every ingest records
its schema id in the Parquet footer and the manifest.

Timestamps are microseconds in UTC (`Timestamp(µs, +10:00)` in Arrow,
`TIMESTAMP(MICROS, isAdjustedToUTC)` in Parquet): Iceberg's `timestamptz`,
which every Iceberg reader reads; DuckDB's refuses `timestamptz_ns`.

| `type` | Arrow | Iceberg |
|---|---|---|
| `string` | `Utf8` | `string` |
| `double` | `Float64` | `double` |
| `long` | `Int64` | `long` |
| `boolean` | `Boolean` | `boolean` |
| `date` | `Date32` | `date` |
| `timestamp` | `Timestamp(µs, +10:00)` | `timestamptz` |
| `decimal(P,S)` | `Decimal128(P, S)` | `decimal(P, S)` |

## The Iceberg tables

`utilake iceberg` (`crates/utilake-writer/src/iceberg.rs`) builds each
table's metadata from two facts: the history's schemas and the manifest's
live outputs. A reingest or a refolded history is a rebuild, never an
edit. Every schema is added in id order, so the table's ids are the
history's; each run of outputs under one schema and version is one
snapshot, with its first and last source file, months and version in the
summary. The store (`crates/utilake-iceberg-store`) is a single writer:
versioned metadata files and a `version-hint.text` swapped by rename,
written with the library's manifest writers; the same facts write the same
bytes. The tables are not partitioned: lake files split by market-time
month, and Iceberg's month transform is the UTC month.

pyiceberg (`StaticTable`) and DuckDB (`iceberg_scan('<dir>')`) read them,
across schemas, by field id.

## Reading an interval

`utilake_iceberg::plan` picks the files whose data overlaps an interval
and projects each onto one target schema by field id: a field the file has
is read under its name there (cast where the target's type is a promotion),
a field it lacks is null of the target's type, a field the target lacks is
left out. The target is the latest schema or any schema of the history.
`utilake sql TABLE --from --to [--as-of N]` prints that plan as one DuckDB
query over the lake's files.

## The export

`utilake-nem-census export` (format 2) writes, per folded table, one JSON
Schema per Iceberg schema (`nem/<TABLE>/schema-<hash>.json`, named by its
fields and key so the `$id` never changes) with each field's id and type
and the runs written under it, and the history in `table.json`'s `iceberg`
block.
