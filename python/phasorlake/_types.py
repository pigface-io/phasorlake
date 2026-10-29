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
    report_subtype: str
    version: int
    columns: list[str]
    fingerprint: str
    """The fingerprint of its column list."""
    rows: int
    """Its D rows."""
    status: str
    """`conforms`, `deviates` or `derived`, else the resolve error's code."""
    table: str | None
    """The table that claims its report pair."""


class Valid(TypedDict):
    """When a row applies."""

    column: str | None
    kind: str
    """`interval_end`, `instant`, `day`, `from` or `timeless`."""


class Roles(TypedDict):
    """A table's temporal model: what a fact is about, when, and which revision is newest."""

    entity: list[str]
    valid: Valid
    order: list[str]
    cutoff: str | None


class Column(TypedDict):
    """One lake column."""

    name: str
    type: str
    role: str
    """`entity`, `valid`, `order`, `order, cutoff`, `cutoff`, or empty."""
    field_id: int
    """The Iceberg and Parquet field id."""
    nullable: bool
    description: str | None


class _TableBase(TypedDict):
    name: str
    market: str
    description: str | None
    load_mode: Literal["append", "snapshot"]
    key: list[str]
    time_column: str | None
    roles: Roles
    columns: list[Column]


class NativeTable(_TableBase):
    """A table as the native module describes it, its schema still to be imported."""

    schema: ArrowSchema


class Table(_TableBase):
    """A table as the registry describes it."""

    schema: pa.Schema
    """The lake schema: every column the table ever had, each with its field id."""
