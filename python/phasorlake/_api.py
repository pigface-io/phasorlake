"""The package's functions: thin calls into the native module, pyarrow at the edge."""

from __future__ import annotations

from importlib.metadata import version as _version
from typing import TYPE_CHECKING, cast

import pyarrow as pa

from phasorlake import _native
from phasorlake._types import Block, Table

if TYPE_CHECKING:
    from collections.abc import Iterable
    from os import PathLike

__version__ = _version("phasorlake")


def decode(
    path: str | PathLike[str],
    *,
    kit: str | None = None,
    strict: bool = False,
    max_bad_rows: int = 0,
) -> dict[str, pa.Table]:
    """Every table in the file at `path`, as `phasor ingest` would write it.

    A zip is read entry by entry, nested zips included. Each table comes back
    under the registry's lake schema, every field with its Iceberg field id in
    its metadata, plus any `{column}__raw` column holding a value that did not
    fit its type.

    Args:
        path: A CSV, a WEMDE JSON document, or a zip of them.
        kit: The kit that reads the file (`nem`, `wem`, `gas`) when its name
            alone does not say which.
        strict: Refuse a block the catalog does not describe exactly, instead
            of keeping its rows as text.
        max_bad_rows: Malformed rows an entry may have before it fails.

    Returns:
        The tables by name.

    Raises:
        PhasorError: The file could not be read; `code` says why.

    Warns:
        PhasorWarning: A deviation from the catalog, a value kept as text, a
            quarantined row, a footer that disagrees with the rows read.
    """
    tables = _native.decode(path, kit=kit, strict=strict, max_bad_rows=max_bad_rows)
    return {name: pa.table(t) for name, t in tables.items()}


def inspect(path: str | PathLike[str]) -> list[Block]:
    """Every block of every CSV in the file at `path`, as `phasor inspect` lists them.

    Args:
        path: A CSV, or a zip of them.

    Returns:
        The blocks in file order, each with the table it resolved to.

    Raises:
        PhasorError: The file could not be read; `code` says why.
    """
    return _native.inspect(path)


def table(name: str) -> Table:
    """What the registry says of a table, as `phasor list TABLE --json` does.

    Args:
        name: The table's name (`DISPATCHPRICE`), or its report pair (`DISPATCH.PRICE`).

    Returns:
        The table's roles, key, columns and its lake schema.

    Raises:
        PhasorError: No table has that name (`schema.unknown_table`).
    """
    t = _native.table(name)
    return Table(
        name=t["name"],
        market=t["market"],
        description=t["description"],
        load_mode=t["load_mode"],
        key=t["key"],
        time_column=t["time_column"],
        roles=t["roles"],
        columns=t["columns"],
        # The native schema speaks the Arrow C data interface, which pyarrow's stubs do not name.
        schema=pa.schema(cast("Iterable[pa.Field[pa.DataType]]", t["schema"])),
    )
