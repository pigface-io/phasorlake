"""The shapes of what the native module returns, as `TypedDict`s."""

from __future__ import annotations

from typing import TYPE_CHECKING, Literal, TypedDict

if TYPE_CHECKING:
    import pyarrow as pa

    from phasorlake._native import ArrowSchema


class Block(TypedDict):
    """One block of an MMS file, as `phasor inspect` shows it."""

    entry: str
    """The zip entry it is in, nested names joined with `!`; the file's name for a CSV."""
    line: int
    """The line of its I row."""
    report_type: str
    """The first half of its report pair (`DISPATCH`)."""
    report_subtype: str
    """The second half of its report pair (`PRICE`)."""
    version: int
    """The version its I row names."""
    columns: list[str]
    """Its columns, as its I row names them."""
    fingerprint: str
    """The fingerprint of its column list."""
    rows: int
    """Its D rows."""
    status: str
    """`conforms`, `deviates` or `derived`, else the resolve error's code."""
    table: str | None
    """The table that claims its report pair."""


class Column(TypedDict):
    """One lake column."""

    name: str
    """The column's name, as AEMO's files spell it."""
    type: str
    """Its lake type: `timestamp`, `decimal(15,5)`, `string`, and so on."""
    role: str
    """`time` for the table's time column, else empty."""
    field_id: int
    """The Iceberg and Parquet field id."""
    nullable: bool
    """Whether it takes nulls: every column does, so that no row is refused for one."""
    description: str | None
    """AEMO's description of it, where the data model has one."""


class _TableBase(TypedDict):
    """What a table is, as the registry says it: everything but its schema."""

    name: str
    """The table's name (`DISPATCHPRICE`)."""
    market: str
    """`nem`, `wem` or `gas`."""
    description: str | None
    """AEMO's description of it, where the data model has one."""
    load_mode: Literal["append", "snapshot"]
    """`append`: rows by year and month; `snapshot`: standing data, the whole table per month."""
    time_column: str | None
    """The column whose month a row is filed under; none for standing data."""
    columns: list[Column]
    """Its current columns, in order."""


class NativeTable(_TableBase):
    """A table as the native module describes it, its schema still to be imported."""

    schema: ArrowSchema


class Table(_TableBase):
    """A table as the registry describes it."""

    schema: pa.Schema
    """The lake schema: every column the table ever had, each with its field id."""
