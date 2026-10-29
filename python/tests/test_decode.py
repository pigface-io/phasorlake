"""`decode` gives what `phasor ingest` writes: the same rows, columns, types and field ids."""

from __future__ import annotations

import warnings
import zipfile
from concurrent.futures import ThreadPoolExecutor
from importlib import metadata
from typing import TYPE_CHECKING, cast

import pyarrow as pa
import pyarrow.parquet as pq
import pytest

import phasorlake as pl
from phasorlake import _native

from .conftest import HEADS

if TYPE_CHECKING:
    from collections.abc import Iterable
    from pathlib import Path

FIELD_ID = b"PARQUET:field_id"

CLAMPED = {"parse.timestamp_clamped"}
"""AEMO writes 9999-12-31 for no end; the lake keeps the latest instant nanoseconds hold."""


def decode(
    path: Path,
    allowed: set[str] = CLAMPED,
    *,
    kit: str | None = None,
    strict: bool = False,
    max_bad_rows: int = 0,
) -> dict[str, pa.Table]:
    """`pl.decode`, every warning it raises checked to be one of `allowed`."""
    with warnings.catch_warnings(record=True) as rec:
        warnings.simplefilter("always")
        tables = pl.decode(path, kit=kit, strict=strict, max_bad_rows=max_bad_rows)
    codes = {str(w.message).split(":", 1)[0] for w in rec}
    assert all(issubclass(w.category, pl.PhasorWarning) for w in rec)
    assert codes <= allowed, codes
    return tables


def rows(t: pa.Table, columns: list[str]) -> list[str]:
    """The rows of `columns`, each as text, sorted: the same rows in any order compare equal."""
    return sorted(repr(r) for r in t.select(columns).to_pylist())


def test_every_head_decodes_to_its_table(heads_zip: Path) -> None:
    tables = decode(heads_zip)
    blocks = pl.inspect(heads_zip)
    assert {b["entry"] for b in blocks} == {p.name for p in HEADS.glob("*_head.csv")}
    assert set(tables) == {b["table"] for b in blocks}
    for name, t in tables.items():
        assert isinstance(t, pa.Table)
        assert t.num_rows > 0, name
        assert t.schema.metadata[b"phasor:table"] == name.encode()
        for f in t.schema:
            if not f.name.endswith("__raw"):
                assert f.metadata is not None, f.name
                assert FIELD_ID in f.metadata, f.name


def test_a_csv_decodes_as_its_zip_entry_does(heads_zip: Path) -> None:
    alone = pl.decode(HEADS / "DISPATCHPRICE_v4_head.csv")["DISPATCHPRICE"]
    zipped = decode(heads_zip)["DISPATCHPRICE"]
    assert alone.schema == zipped.schema
    assert alone.to_pylist() == zipped.to_pylist()


def test_decode_matches_what_ingest_writes(heads_zip: Path, tmp_path: Path) -> None:
    home = tmp_path / "home"
    code = pl.run(["--home", str(home), "--no-progress", "import", str(heads_zip)])
    assert code == 0
    tables = decode(heads_zip)
    for name, got in tables.items():
        files = sorted((home / "lake" / "nem" / name.lower()).glob("data/**/*.parquet"))
        assert files, name
        written = pa.concat_tables([pq.read_table(f) for f in files], promote_options="default")
        columns = [c for c in written.column_names if not c.startswith("_phasor_")]
        # The same columns in the same order, the lake's provenance columns aside.
        assert [c for c in got.column_names if c in columns] == columns, name
        assert set(columns) <= set(got.column_names), name
        for c in columns:
            assert got.schema.field(c).type == written.schema.field(c).type, (name, c)
            if not c.endswith("__raw"):
                parquet = pq.read_schema(files[0]).field(c).metadata
                ours = got.schema.field(c).metadata
                assert parquet is not None, (name, c)
                assert ours is not None, (name, c)
                assert ours[FIELD_ID] == parquet[FIELD_ID], (name, c)
        assert rows(got, columns) == rows(written, columns), name


def test_an_unreadable_file_is_a_phasor_error(tmp_path: Path) -> None:
    missing = tmp_path / "DISPATCHPRICE.csv"
    with pytest.raises(pl.PhasorError) as e:
        pl.decode(missing)
    assert e.value.code == "csv.read_failed"
    assert e.value.file == str(missing)
    assert "could not be opened" in e.value.message
    assert e.value.entry is None
    assert e.value.table is None
    assert e.value.line is None


def test_a_file_no_kit_reads_is_a_phasor_error(tmp_path: Path) -> None:
    other = tmp_path / "notes.txt"
    other.write_text("hello")
    with pytest.raises(pl.PhasorError) as e:
        pl.decode(other)
    assert e.value.code == "config.no_decoder"


def test_a_named_kit_reads_the_file(heads_zip: Path) -> None:
    assert decode(heads_zip, kit="nem").keys() == decode(heads_zip).keys()
    with pytest.raises(pl.PhasorError) as e:
        pl.decode(heads_zip, kit="no-such-kit")
    assert e.value.code == "config.no_decoder"


def test_a_malformed_row_is_warned_of_then_refused(tmp_path: Path) -> None:
    text = (HEADS / "DISPATCHPRICE_v4_head.csv").read_text(encoding="latin-1")
    lines = text.splitlines()
    first_d = next(i for i, line in enumerate(lines) if line.startswith("D,"))
    # Too many fields is a malformed row; too few is a row with nulls, kept.
    lines.insert(first_d + 1, "D,DISPATCH,PRICE,4," + ",".join(["x"] * 80))
    bad = tmp_path / "DISPATCHPRICE_v4_bad.csv"
    bad.write_text("\r\n".join(lines) + "\r\n", encoding="latin-1")
    with pytest.warns(pl.PhasorWarning) as rec:
        kept = pl.decode(bad, max_bad_rows=1)["DISPATCHPRICE"]
    assert any(str(w.message).startswith("csv.field_count_mismatch") for w in rec)
    good = pl.decode(HEADS / "DISPATCHPRICE_v4_head.csv")["DISPATCHPRICE"]
    assert kept.num_rows == good.num_rows
    with pytest.raises(pl.PhasorError) as e:
        pl.decode(bad)
    assert e.value.code.startswith("csv.")
    assert e.value.line is not None


def test_strict_refuses_a_block_the_catalog_does_not_describe(tmp_path: Path) -> None:
    text = (HEADS / "DISPATCHPRICE_v4_head.csv").read_text(encoding="latin-1")
    odd = tmp_path / "DISPATCHPRICE_v4_odd.csv"
    shifted = text.replace("I,DISPATCH,PRICE,4,", "I,DISPATCH,PRICE,4,EXTRA,", 1)
    odd.write_text(shifted, encoding="latin-1")
    with pytest.warns(pl.PhasorWarning) as rec:
        loose = pl.decode(odd)
    assert "DISPATCHPRICE" in loose
    assert "SETTLEMENTDATE__raw" in loose["DISPATCHPRICE"].column_names
    assert any("parse.value_kept_raw" in str(w.message) for w in rec)
    with pytest.raises(pl.PhasorError):
        pl.decode(odd, strict=True)


def test_an_entry_that_cannot_be_read_is_warned_of_and_skipped(tmp_path: Path) -> None:
    head = HEADS / "DISPATCHPRICE_v4_head.csv"
    path = tmp_path / "mixed.zip"
    with zipfile.ZipFile(path, "w") as z:
        z.write(head, "A.CSV", zipfile.ZIP_DEFLATED)
        z.write(head, "B.CSV", zipfile.ZIP_LZMA)
    tables = decode(path, allowed={*CLAMPED, "zip.bad_entry"})
    assert tables["DISPATCHPRICE"].num_rows == decode(head)["DISPATCHPRICE"].num_rows


def test_entries_kept_raw_in_different_columns_share_one_table(tmp_path: Path) -> None:
    # Each entry has a value that does not fit in a column of its own: the table
    # has both text columns, each null in the rows of the other entry.
    lines = (HEADS / "DISPATCHPRICE_v4_head.csv").read_text(encoding="latin-1").splitlines()
    first_d = next(i for i, line in enumerate(lines) if line.startswith("D,"))
    path = tmp_path / "PUBLIC_DISPATCHIS.zip"
    with zipfile.ZipFile(path, "w") as z:
        for name, at in (("A.CSV", 9), ("B.CSV", 10)):  # RRP, then EEP
            fields = lines[first_d].split(",")
            fields[at] = "not-a-number"
            odd = [*lines[:first_d], ",".join(fields), *lines[first_d + 1 :]]
            z.writestr(name, "\r\n".join(odd) + "\r\n")
    price = decode(path, allowed={*CLAMPED, "parse.value_kept_raw"})["DISPATCHPRICE"]
    rows = price.num_rows // 2
    assert price.column("RRP__raw").to_pylist() == ["not-a-number"] + [None] * (2 * rows - 1)
    eep = price.column("EEP__raw").to_pylist()
    assert eep == [None] * rows + ["not-a-number"] + [None] * (rows - 1)
    assert {c.num_chunks for c in price.columns} == {1}


def test_the_version_is_the_crate_version() -> None:
    assert pl.__version__ == _native.version() == metadata.version("phasorlake")


def test_a_native_table_speaks_the_arrow_protocols() -> None:
    native = _native.decode(HEADS / "DISPATCHPRICE_v4_head.csv")["DISPATCHPRICE"]
    table = pa.table(native)
    assert len(native) == table.num_rows
    # pyarrow's stubs do not name the Arrow C data interface; `pyarrow.schema` takes it.
    assert pa.schema(cast("Iterable[pa.Field[pa.DataType]]", native)) == table.schema


def test_small_blocks_come_back_as_one_chunk(tmp_path: Path) -> None:
    # A zip of 5-minute files is many small blocks of a table: one chunk, each row once.
    head = HEADS / "DISPATCHPRICE_v4_head.csv"
    path = tmp_path / "PUBLIC_DISPATCHIS.zip"
    with zipfile.ZipFile(path, "w") as z:
        for i in range(12):
            z.write(head, f"PUBLIC_DISPATCHIS_{i:02}.CSV")
    alone = decode(head)["DISPATCHPRICE"]
    price = decode(path)["DISPATCHPRICE"]
    assert price.num_rows == 12 * alone.num_rows
    assert {c.num_chunks for c in price.columns} == {1}
    assert price.slice(0, alone.num_rows).equals(alone)


def test_threads_decode_files_side_by_side(heads_zip: Path) -> None:
    # The GIL is released while a file is read: each thread gets the whole file.
    alone = decode(heads_zip)
    with ThreadPoolExecutor(4) as pool:
        side_by_side = list(pool.map(lambda _: decode(heads_zip), range(8)))
    for tables in side_by_side:
        assert tables.keys() == alone.keys()
        for name, t in tables.items():
            assert t.equals(alone[name]), name
