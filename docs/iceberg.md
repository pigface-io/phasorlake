# Iceberg compatibility

The lake is Hive-partitioned Parquet with a SQLite manifest. A later
`LakeWriter` that keeps an Apache Iceberg table is meant to be a metadata-only
migration: the Parquet files already written are registered as the table's
data files, none is rewritten. These are the rules that keep that possible.

## Format version 3

Every timestamp in the lake is microseconds since the epoch in UTC
(`Timestamp(µs, +10:00)` in Arrow, `TIMESTAMP(MICROS, isAdjustedToUTC)` in
Parquet): Iceberg's `timestamptz`. AEMO's times are whole seconds or carry
at most three fractional digits, and every Iceberg reader reads
`timestamptz`; DuckDB's refuses `timestamptz_ns`. The tables are format
version 3.

## Types

A column's type in `table.toml` is exact and names its Arrow and Iceberg
types alike:

| `type` | Arrow | Parquet | Iceberg V3 |
|---|---|---|---|
| `string` | `Utf8` | `BYTE_ARRAY (STRING)` | `string` |
| `double` | `Float64` | `DOUBLE` | `double` |
| `long` | `Int64` | `INT64` | `long` |
| `boolean` | `Boolean` | `BOOLEAN` | `boolean` |
| `date` | `Date32` | `INT32 (DATE)` | `date` |
| `timestamp` | `Timestamp(µs, +10:00)` | `INT64 (TIMESTAMP(MICROS, isAdjustedToUTC))` | `timestamptz` |
| `decimal(P,S)` | `Decimal128(P, S)` | `INT32` (P ≤ 9), `INT64` (P ≤ 18) `(DECIMAL(P, S))` | `decimal(P, S)` |

The contract lives in `crates/utilake-iceberg`. A column's type is chosen
once, from everything observed of it over its whole history and from
AEMO's declaration (`choose`):

- a number declared whole (the data model's `NUMERIC(p,0)`, a control
  file's `INTEGER EXTERNAL`) or in the key, that only held whole numbers:
  `long`;
- a float printed in full (more places than declared, 16 or more
  significant digits, such as `239.99999999999997`): `double`;
- any other number: an exact `decimal(P, S)`, S the larger of the declared
  and the largest observed scale, fixed for the life of the column, while P
  fits 18 digits (stored as a 64-bit integer, no larger than a double);
  past that, `double`. A whole-number column nothing declares whole is a
  `decimal(P, 0)`, so a later fraction is a scale to decide, not a type
  Iceberg cannot change;
- a column never seen with a value: the data model's type, else what it
  was.

A 32-bit float is never chosen, since it would round AEMO's prices and
energies.

## A table as an Iceberg schema

`Table::iceberg_schema` (`crates/utilake-mms-registry/src/file_model/iceberg.rs`)
gives a table's columns as Iceberg fields, with their ids, types and
nullability, and its primary key as identifier fields; `Table::partition_spec`
partitions it by month of its time column. Iceberg refuses a key with a
nullable or float column. Such a key is left out and the reason returned,
until the column's type is chosen from its observed values.

The export (`just schema-export`) writes each declared table's Iceberg
schema and partition spec into its `table.json`, with its eras: one per
column list the archive published, in order, each the `add-column`
updates that take the table from the columns published before it to its
own. A column is never dropped; a list without it leaves it null. A
block whose column list the archive published, and whose columns the
table has, conforms in strict mode whatever increment is in force.

## Field ids

Iceberg identifies a column by its field id, not its name, and never reuses
one. Every column a file holds carries its id in the Parquet field metadata
(`PARQUET:field_id`, the key Iceberg reads): the id its table's `[[column]]`
gives it. An id is never reused, and a renamed column is a new column with
an id of its own. So each file's schema is a subset of its table's, with
matching ids. `just schema-compat` refuses a change that removes a column,
gives its id to another column or moves a name to another id.

A column a file carries that its table does not have is kept as text under
its own name, without an id; so is the text `{column}__raw` of values that
did not fit their column. Adding the column to `table.toml` gives it an id,
and `utilake reingest` writes the files that carry it again.

## Type changes

A column's type changes only by an Iceberg promotion, which Iceberg applies
in metadata alone: int to long, float to double, a decimal to a larger
precision of the same scale. Of our types only the decimal promotes (`long`
to `double` is not a promotion), so any other
type change is refused (`Schema::apply`, `just schema-compat`): values of
another type go to a new column. A column may become optional, never
required; a key column stays required.
