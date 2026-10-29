from __future__ import annotations

import json
import subprocess  # ruff: ignore[suspicious-subprocess-import] — the tests run the bundled binary
import zipfile
from pathlib import Path

import pytest

from phasorlake._cli import binary

ROOT = Path(__file__).resolve().parents[2]
"""The repository."""

HEADS = ROOT / "testdata" / "csv"
"""AEMO's files, their first rows: one per table and version."""


def phasor_json(*args: str) -> list[object]:
    """What the bundled `phasor ... --json` prints: one JSON value per line."""
    argv = [str(binary("phasor")), "--json", *args]
    out = subprocess.run(argv, check=True, capture_output=True, text=True)  # ruff: ignore[subprocess-without-shell-equals-true] — our own binary
    return [json.loads(line) for line in out.stdout.splitlines() if line]


@pytest.fixture(scope="session")
def heads_zip(tmp_path_factory: pytest.TempPathFactory) -> Path:
    """A zip of every head fixture, as an AEMO zip holds its CSVs."""
    path = tmp_path_factory.mktemp("zips") / "heads.zip"
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        for csv in sorted(HEADS.glob("*_head.csv")):
            z.write(csv, csv.name)
    return path
