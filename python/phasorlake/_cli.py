"""The `phasor` and `phasor-mirror` commands: the real binaries, shipped in the wheel."""

from __future__ import annotations

import os
import subprocess  # ruff: ignore[suspicious-subprocess-import] — the package runs its own binaries, nothing else
import sys
from pathlib import Path
from typing import TYPE_CHECKING, NoReturn

if TYPE_CHECKING:
    from collections.abc import Sequence

_BIN = Path(__file__).parent / "bin"


def binary(name: str, bin_dir: Path | None = None) -> Path:
    """The path of the bundled binary `name`.

    Args:
        name: `phasor` or `phasor-mirror`; `.exe` is added on Windows.
        bin_dir: Where to look; the package's `bin/` when not given.

    Returns:
        The binary, which exists.

    Raises:
        FileNotFoundError: The binary is not in this install (a development
            install before `just py-bin`).

    Examples:
        >>> from phasorlake._cli import binary
        >>> binary("phasor").name
        'phasor'
    """
    path = (bin_dir or _BIN) / (f"{name}.exe" if os.name == "nt" else name)
    if not path.is_file():
        msg = f"{path} is not in this install; `just py-bin` builds it"
        raise FileNotFoundError(msg)
    return path


def run(args: Sequence[str], *, binary_name: str = "phasor") -> int:
    """Run the bundled `phasor` (or `phasor-mirror`) with `args`.

    The command writes to this process's standard output and error, and
    reads its standard input.

    Args:
        args: The command line after the binary's name.
        binary_name: `phasor` or `phasor-mirror`.

    Returns:
        The exit code.

    Raises:
        FileNotFoundError: The binary is not in this install.

    Examples:
        >>> import phasorlake as pl
        >>> pl.run(["--version"])
        0
    """
    argv = [str(binary(binary_name)), *args]
    return subprocess.run(argv, check=False).returncode  # ruff: ignore[subprocess-without-shell-equals-true] — our own binary, args as given


def _exec(name: str) -> NoReturn:
    """Replace this process with the bundled binary, this process's arguments passed on.

    Raises:
        SystemExit: On Windows, where a process cannot be replaced, with the binary's exit code.
    """
    path = binary(name)
    argv = [str(path), *sys.argv[1:]]
    if os.name == "nt":
        # Windows cannot replace a process: run it and leave with its code.
        raise SystemExit(subprocess.run(argv, check=False).returncode)  # ruff: ignore[subprocess-without-shell-equals-true] — our own binary
    os.execv(path, argv)  # ruff: ignore[start-process-with-no-shell] — no shell; our own binary


def phasor() -> NoReturn:
    """The `phasor` console script."""
    _exec("phasor")


def phasor_mirror() -> NoReturn:
    """The `phasor-mirror` console script."""
    _exec("phasor-mirror")
