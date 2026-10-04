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

## Field ids

Iceberg identifies a column by its field id, not its name, and never reuses
one. Every column a file holds carries its id in the Parquet field metadata
(`PARQUET:field_id`, the key Iceberg reads):

- a declared column has the id its table file gives it (`[[column]] id`);
- a column a shape of the census carries beside the declared ones is declared
  by `just schema-regen` as nullable text with `src = ["file"]` and an id of
  its own, so its files hold it under that id;
- a column `{column}__raw` that keeps, as text, values of a column whose
  control-file class does not fit its type is declared the same way with
  `src = ["raw"]`, and conform writes those values into it.

So each file's schema is a subset of its table's, with matching ids.
`just schema-compat` refuses a change that deletes a column, gives its id to
another column or moves a name to another id, for these columns as for every
other. A column no table and no census knows yet, as in a file of a month the
census has not seen, is still kept as text under its own name without an id;
the next census and regeneration declare it.

## Type changes

`is_widening` (`crates/nemlake-registry/src/file_model/compat.rs`) says which
type changes the lake survives. Iceberg promotes some types in metadata alone
(int to long, float to double, a decimal to a larger precision) but not a
date to a timestamp, nor a float to a decimal. nemlake does not rely on
either: any type change alters the plan of every shape that carries the
column, and `nemlake reingest` writes those files again.
