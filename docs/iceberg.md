# Iceberg compatibility

The lake is Hive-partitioned Parquet with a SQLite manifest. A later
`LakeWriter` that keeps an Apache Iceberg table is meant to be a metadata-only
migration: the Parquet files already written are registered as the table's
data files, none is rewritten. These are the rules that keep that possible.

## Format version 3

Every timestamp in the lake is nanoseconds since the epoch in UTC
(`Timestamp(ns, +10:00)` in Arrow, `TIMESTAMP(NANOS, isAdjustedToUTC)` in
Parquet). Iceberg's `timestamp_ns` and `timestamptz_ns` types exist only from
format version 3; version 2 knows microseconds only. The target is therefore
Iceberg format version 3. The time unit is not changed to suit version 2:
AEMO publishes fractional seconds, and a microsecond lake would drop them.

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
| `timestamp_ns` | `Timestamp(ns, +10:00)` | `INT64 (TIMESTAMP(NANOS, isAdjustedToUTC))` | `timestamptz_ns` |
| `decimal(P, S)` | `Decimal128(P, S)` | `FIXED_LEN_BYTE_ARRAY (DECIMAL(P, S))` | `decimal(P, S)` |

The contract lives in `crates/utilake-iceberg`. A column's type is chosen
once, from everything observed of it over its whole history (`type_for`):
whole numbers are `long`; numbers of up to 15 significant digits are
`double`, which holds them exactly; longer ones are `decimal(P, S)` with S
the largest scale seen, fixed for the life of the column. A 32-bit float is
never chosen, since it would round AEMO's prices and energies. A column
never seen with a value keeps its declared type.

## A table as an Iceberg schema

`Table::iceberg_schema` (`crates/utilake-mms-registry/src/file_model/iceberg.rs`)
gives a table's columns as Iceberg fields, with their ids, types and
nullability, and its primary key as identifier fields; `Table::partition_spec`
partitions it by month of its time column. Iceberg refuses a key with a
nullable or float column. Such a key is left out and the reason returned,
until the column's type is chosen from its observed values.

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
to `double` is not a promotion, nor `date` to `timestamp_ns`), so any other
type change is refused (`Schema::apply`, `just schema-compat`): values of
another type go to a new column. A column may become optional, never
required; a key column stays required.
