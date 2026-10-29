"""AEMO market data, read as `phasor ingest` writes it.

Every function here is a thin call into the native module, which runs the same
crates as the `phasor` command: the kits, the decoders and the registry. What
`decode` returns of a file is what `phasor ingest` writes of it. Nothing here
reads the lake: that is your own engine's work (`docs/reading.md`).
"""

from phasorlake._api import __version__, decode, inspect, table
from phasorlake._cli import run
from phasorlake._native import PhasorError, PhasorWarning
from phasorlake._types import Block, Column, Roles, Table, Valid

__all__ = [
    "Block",
    "Column",
    "PhasorError",
    "PhasorWarning",
    "Roles",
    "Table",
    "Valid",
    "__version__",
    "decode",
    "inspect",
    "run",
    "table",
]
