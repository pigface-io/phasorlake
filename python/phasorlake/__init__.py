"""AEMO market data, read as `phasor ingest` writes it.

Every function here is a thin call into the native module, which runs the same
crates as the `phasor` command: the kits, the decoders and the registry. What
`decode` returns of a file is what `phasor ingest` writes of it, and `schema`
is the shape it writes a table as. Nothing here reads the lake: that is your
own engine's work (`docs/reading.md`).

The examples read the excerpts of AEMO's files under `testdata/`, so they run
from the repository's root, where `just py-test` runs them.

Examples:
    >>> import phasorlake as pl
    >>> sorted(pl.decode("testdata/csv/DISPATCHPRICE_v4_head.csv"))
    ['DISPATCHPRICE']
    >>> pl.schema("DISPATCHPRICE").metadata[b"phasor:time_column"]
    b'SETTLEMENTDATE'
"""

from phasorlake._api import __version__, decode, inspect, schema
from phasorlake._cli import run
from phasorlake._native import PhasorError, PhasorWarning
from phasorlake._types import Block

__all__ = [
    "Block",
    "PhasorError",
    "PhasorWarning",
    "__version__",
    "decode",
    "inspect",
    "run",
    "schema",
]
