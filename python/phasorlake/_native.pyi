# The native module: the `phasor` command's crates, called from Python.
# `mypy.stubtest` holds this file to the built module (`just py-stubs`).

from os import PathLike
from typing import final

from phasorlake._types import Block, NativeTable

__all__ = [
    "ArrowSchema",
    "ArrowTable",
    "PhasorError",
    "PhasorWarning",
    "decode",
    "inspect",
    "table",
    "version",
]

# A failure of phasor; `code` is its diagnostic code (docs/errors.md).
class PhasorError(Exception):
    code: str
    message: str
    file: str | None
    entry: str | None
    table: str | None
    line: int | None

# Something phasor noted while reading, prefixed with its diagnostic code.
class PhasorWarning(UserWarning): ...

# A table on its way to Python: pass it to `pyarrow.table`.
@final
class ArrowTable:
    def __arrow_c_schema__(self) -> object: ...
    def __arrow_c_stream__(self, requested_schema: object | None = None) -> object: ...
    def __len__(self) -> int: ...

# A schema on its way to Python: pass it to `pyarrow.schema`.
@final
class ArrowSchema:
    def __arrow_c_schema__(self) -> object: ...

def decode(
    path: str | PathLike[str],
    *,
    kit: str | None = None,
    strict: bool = False,
    max_bad_rows: int = 0,
) -> dict[str, ArrowTable]: ...
def inspect(path: str | PathLike[str]) -> list[Block]: ...
def table(name: str) -> NativeTable: ...
def version() -> str: ...
