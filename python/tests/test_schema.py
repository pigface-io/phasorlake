"""`schema` is a table's lake schema: what `phasor ingest` writes its rows as."""

from __future__ import annotations

import pyarrow as pa
import pytest

import phasorlake as pl

from .conftest import phasor_json


def test_dispatchprice_schema_names_its_table_and_carries_field_ids() -> None:
    s = pl.schema("dispatchprice")
    assert isinstance(s, pa.Schema)
    assert s.metadata[b"phasor:table"] == b"DISPATCHPRICE"
    assert s.metadata[b"phasor:market"] == b"nem"
    assert s.metadata[b"phasor:load_mode"] == b"append"
    assert s.metadata[b"phasor:time_column"] == b"SETTLEMENTDATE"
    assert s.field("SETTLEMENTDATE").type == pa.timestamp("us", "+10:00")
    assert s.field("RRP").type == pa.decimal128(15, 5)
    rrp = s.field("RRP").metadata
    assert rrp is not None
    assert int(rrp[b"PARQUET:field_id"]) > 0
    # The lake derives the UTC sibling of every timestamp, after the table's own columns.
    sibling = s.field("SETTLEMENTDATE_UTC_US")
    assert sibling.type == pa.int64()
    assert sibling.metadata is not None
    assert b"utc_us" in sibling.metadata[b"phasor:derived"]
    assert s.names.index("SETTLEMENTDATE_UTC_US") > s.names.index("RRP")


def field_id(s: pa.Schema, name: str) -> int:
    metadata = s.field(name).metadata
    assert metadata is not None
    return int(metadata[b"PARQUET:field_id"])


def test_the_schema_is_what_the_command_lists() -> None:
    s = pl.schema("DISPATCHPRICE")
    printed = [p for p in phasor_json("list", "DISPATCHPRICE") if isinstance(p, dict)]
    assert s.names == [p["column"] for p in printed]
    assert [field_id(s, n) for n in s.names] == [p["field_id"] for p in printed]
    assert [p["role"] for p in printed if p["column"].endswith("_UTC_US")] == ["derived"] * 2


def test_a_report_pair_names_its_table() -> None:
    assert pl.schema("DISPATCH.PRICE").metadata[b"phasor:table"] == b"DISPATCHPRICE"


def test_an_unknown_table_is_a_phasor_error() -> None:
    with pytest.raises(pl.PhasorError) as e:
        pl.schema("NO_SUCH_TABLE")
    assert e.value.code == "schema.unknown_table"
    assert e.value.table == "NO_SUCH_TABLE"
    assert "phasor list" in str(e.value)


def test_standing_data_is_a_snapshot_with_no_time_column() -> None:
    s = pl.schema("DUDETAILSUMMARY")
    assert s.metadata[b"phasor:load_mode"] == b"snapshot"
    assert b"phasor:time_column" not in s.metadata
