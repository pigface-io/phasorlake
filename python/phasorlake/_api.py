"""The package's functions: thin calls into the native module, pyarrow at the edge.

pyarrow is imported by the functions that return its types, not with the
package: the `phasor` and `phasor-mirror` scripts import the package and
start no faster than it does.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, cast

from phasorlake import _native
from phasorlake._types import Block, Table

if TYPE_CHECKING:
    from collections.abc import Iterable
    from os import PathLike

    import pyarrow as pa

__version__ = _native.version()


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
        strict: Refuse a block the catalog does not describe exactly, or a
            value that does not fit its type, instead of keeping it as text.
        max_bad_rows: Malformed rows an entry may drop before it fails.

    Returns:
        The tables by name.

    Raises:
        PhasorError: The file could not be read; `code` says why.

    Warns:
        PhasorWarning: A deviation from the catalog, a value kept as text, a
            malformed row dropped, a footer that disagrees with the rows read.

    Examples:
        >>> import phasorlake as pl
        >>> price = pl.decode("testdata/csv/DISPATCHPRICE_v4_head.csv")["DISPATCHPRICE"]
        >>> price.num_rows
        20
        >>> price.schema.field("SETTLEMENTDATE").type
        TimestampType(timestamp[us, tz=+10:00])
        >>> price.schema.field("RRP").metadata
        {b'PARQUET:field_id': b'6'}
    """
    import pyarrow as pa  # ruff: ignore[import-outside-top-level] — not with the package (above)

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

    Examples:
        >>> import phasorlake as pl
        >>> blocks = pl.inspect("testdata/csv/multi_block_dispatchis.csv")
        >>> [(b["report_subtype"], b["version"], b["status"]) for b in blocks[:3]]
        [('CASE_SOLUTION', 2, 'conforms'), ('LOCAL_PRICE', 1, 'conforms'), ('PRICE', 5, 'conforms')]
        >>> blocks[2]["table"]
        'DISPATCHPRICE'
    """
    return _native.inspect(path)


def table(name: str) -> Table:
    """What the registry says of a table, as `phasor list TABLE --json` does.

    Args:
        name: The table's name (`DISPATCHPRICE`), or its report pair (`DISPATCH.PRICE`).

    Returns:
        The table's name, market, description, load mode, time column,
        columns, and its lake schema.

    Raises:
        PhasorError: No table has that name (`schema.unknown_table`).

    Examples:
        >>> import phasorlake as pl
        >>> t = pl.table("DISPATCH.PRICE")
        >>> t["name"], t["market"], t["load_mode"], t["time_column"]
        ('DISPATCHPRICE', 'nem', 'append', 'SETTLEMENTDATE')
        >>> t["columns"][0]["name"], t["columns"][0]["role"], t["columns"][0]["field_id"]
        ('SETTLEMENTDATE', 'time', 1)
        >>> try:
        ...     pl.table("NOSUCHTABLE")
        ... except pl.PhasorError as e:
        ...     e.code
        'schema.unknown_table'
    """
    import pyarrow as pa  # ruff: ignore[import-outside-top-level] — not with the package (above)

    t = _native.table(name)
    return Table(
        name=t["name"],
        market=t["market"],
        description=t["description"],
        load_mode=t["load_mode"],
        time_column=t["time_column"],
        columns=t["columns"],
        # The native schema speaks the Arrow C data interface, which pyarrow's stubs do not name.
        schema=pa.schema(cast("Iterable[pa.Field[pa.DataType]]", t["schema"])),
    )
