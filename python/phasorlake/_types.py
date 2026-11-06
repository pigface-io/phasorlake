"""The shapes of what the native module returns, as `TypedDict`s."""

from __future__ import annotations

from typing import TypedDict


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
