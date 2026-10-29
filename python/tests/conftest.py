from __future__ import annotations

import zipfile
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
"""The repository."""

HEADS = ROOT / "testdata" / "csv"
"""AEMO's files, their first rows: one per table and version."""


@pytest.fixture(scope="session")
def heads_zip(tmp_path_factory: pytest.TempPathFactory) -> Path:
    """A zip of every head fixture, as an AEMO zip holds its CSVs."""
    path = tmp_path_factory.mktemp("zips") / "heads.zip"
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        for csv in sorted(HEADS.glob("*_head.csv")):
            z.write(csv, csv.name)
    return path
