"""`inspect` lists every block as `phasor inspect` does."""

from __future__ import annotations

from typing import TYPE_CHECKING

import pytest

import phasorlake as pl

from .conftest import HEADS, phasor_json

if TYPE_CHECKING:
    from pathlib import Path


def test_every_block_of_the_zip_is_listed(heads_zip: Path) -> None:
    blocks = pl.inspect(heads_zip)
    entries = {b["entry"] for b in blocks}
    assert entries == {p.name for p in HEADS.glob("*_head.csv")}
    price = next(b for b in blocks if b["table"] == "DISPATCHPRICE")
    assert price["report_type"] == "DISPATCH"
    assert price["report_subtype"] == "PRICE"
    assert price["version"] == 4
    assert price["status"] == "conforms"
    assert price["rows"] > 0
    assert price["line"] == 2
    assert "REGIONID" in price["columns"]
    assert price["fingerprint"].startswith("xxh3:")


def test_inspect_is_what_the_command_prints(heads_zip: Path) -> None:
    (printed,) = phasor_json("inspect", str(heads_zip))
    assert isinstance(printed, dict)
    assert pl.inspect(heads_zip) == printed["blocks"]


def test_a_csv_is_one_entry_named_for_itself() -> None:
    csv = HEADS / "multi_block_dispatchis.csv"
    blocks = pl.inspect(csv)
    assert len(blocks) > 1
    assert {b["entry"] for b in blocks} == {csv.name}


def test_a_missing_file_is_a_phasor_error(tmp_path: Path) -> None:
    with pytest.raises(pl.PhasorError) as e:
        pl.inspect(tmp_path / "gone.csv")
    assert e.value.code == "csv.read_failed"


def test_a_broken_zip_is_a_phasor_error(tmp_path: Path) -> None:
    broken = tmp_path / "broken.zip"
    broken.write_bytes(b"not a zip")
    with pytest.raises(pl.PhasorError) as e:
        pl.inspect(broken)
    assert e.value.code.startswith("zip.")
    assert e.value.file == str(broken)
