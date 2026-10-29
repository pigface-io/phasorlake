# The lake as Iceberg tables

Every table the lake holds is an Apache Iceberg table (format version 3).
One model runs through the census, the registry, the lake and the export:

| Term | What it is | Where |
|---|---|---|
| **Field** | An Iceberg field: an id never minted twice, a name, one type, whether it is required | `phasor-iceberg` (phasor-schemas) `src/schema.rs` |
| **Schema** | The fields a file of the table holds, in file order, and the key; an id per distinct schema | `iceberg.schemas` of `schemas/nem/pulled/<TABLE>/table.json` |
| **Block** | One block of one AEMO file: its header, rows, data span and the profile of its values | the census of phasor-schemas (`evidence/census/`) |
| **Run** | Consecutive blocks under one schema, AEMO version and report pair | the history's `runs` |
| **Snapshot** | One run of the lake's files, committed to the table | `{lake}/nem/<table>/metadata/` |

## The history: from the census to schemas

The fold (phasor-schemas, `crates/phasor-schemas/src/history.rs`) reads
every census block and writes each table's history.

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
- **Never required**: every field is optional. Which columns may be null,
  and what one row is, are a reader's decisions, not the lake's.
- **Interned.** A schema with the same fields, order and key as an earlier
  one is that one, with its id.

Each run records what changed at its start (`schema`, `version`,
`report`), the updates from the run before (`add`, `restore`, `delete`,
`move`, `make-optional`, `set-identifier-fields`), the kinds of file it
came in, and whether its data overlaps the run before.

## Resolution and the lake

phasor-schemas publishes each table as Iceberg table metadata
(`nem/<TABLE>/metadata/v1.metadata.json`: its schemas, its runs as
snapshots, its declaration and the data model's words as properties; the
format is described in that repository's README); phasor keeps a copy at
one commit in `schemas/nem/pulled/`, held to the manifest and to the
`phasor-iceberg` rev of `Cargo.lock` by `just schemas-check`, and the
registry embeds what it reads from it through
`phasor_iceberg::from_metadata`. A table's columns are
the history's fields by id, typed by their Iceberg types, so a plan writes
every value under its Iceberg field id (`PARQUET:field_id`). A block
resolves to the schema `History::resolve` names for its report pair, AEMO
version, columns in order and source month, and conforms; the fold
guarantees that every block of the census resolves to its run's fields. A
block no run fits deviates. Every ingest records its schema id
(`phasor.schema_version`) and the census's id of its shape
(`phasor.shape_id`) in the Parquet footer and the manifest.

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

`phasor iceberg` (`crates/phasor-writer/src/iceberg.rs`) builds each
table's metadata from two facts: the history's schemas and the manifest's
live outputs. A reingest or a refolded history is a rebuild, never an
edit. Every schema is added in id order, so the table's ids are the
history's; each run of outputs under one schema and version is one
snapshot, with its first and last source file, months and version in the
summary. After the runs the union of the history's schemas becomes
current: every field by id, under its newest name, which is what every
lake file holds. The table's properties are the ones phasor-schemas
publishes (declaration, `with-columns`, breaks, the data
model's words) and `phasor.lake.format`; the union's fields carry the
data model's description of each column. The store (`crates/phasor-iceberg-store`) is a single writer:
versioned metadata files and a `version-hint.text` swapped by rename. The
bytes come from `phasor_iceberg::table`, written by hand to the spec: the
metadata as JSON, manifests and manifest lists as Avro object container
files with the spec's field ids, no compression, and a sync marker hashed
from the content, so the same facts write the same bytes. The tables are
not partitioned: lake files split by market-time month, and Iceberg's
month transform is the UTC month.

The pointer in `version-hint.text` is the catalog: it needs no server, and
the tables publish as plain files.

pyiceberg (`StaticTable`) and DuckDB (`iceberg_scan('<dir>')`) read them
by field id with the union current, so every column of every file reads;
`scripts/iceberg-readers.py` checks a table the store's test keeps with
both, and every manifest with fastavro.

## Reading an interval

`phasor_iceberg::plan` picks the files whose data overlaps an interval
and projects each onto one target schema by field id: a field the file has
is read under its name there (cast where the target's type is a promotion),
a field it lacks is null of the target's type, a field the target lacks is
left out. phasor itself does not read the lake: `docs/reading.md` gives
the reads as SQL over the Iceberg tables or the Parquet files.

## The export

`phasor-schemas export` (format 3, in phasor-schemas) writes, per folded
table, its Iceberg table metadata with one empty manifest list per run,
and, derived from it, one JSON Schema per Iceberg schema
(`nem/<TABLE>/schema-<id>.json`) and `nem/headers.json`. A lake table
`phasor iceberg` writes relates to the published one by its uuid and its
schema ids, which are the same; its snapshots carry data files, the
published one's none. The layout is described in phasor-schemas' README.
