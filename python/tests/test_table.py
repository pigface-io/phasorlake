"""`table` describes a table as `phasor list TABLE --json` does."""

from __future__ import annotations

import pyarrow as pa
import pytest

import phasorlake as pl

from .conftest import phasor_json


def test_dispatchprice_columns_and_schema() -> None:
    t = pl.table("dispatchprice")
    assert t["name"] == "DISPATCHPRICE"
    assert t["market"] == "nem"
    assert t["load_mode"] == "append"
    # A table's key and roles are a reader's: the registry says neither.
    assert "key" not in t and "roles" not in t
    assert t["time_column"] == "SETTLEMENTDATE"
    by_name = {c["name"]: c for c in t["columns"]}
    assert by_name["SETTLEMENTDATE"]["role"] == "time"
    assert not by_name["RRP"]["role"]
    assert by_name["RRP"]["type"] == "decimal(15,5)"
    assert by_name["RRP"]["field_id"] > 0
    assert isinstance(t["schema"], pa.Schema)
    assert t["schema"].names == [c["name"] for c in t["columns"]]
    assert t["schema"].field("SETTLEMENTDATE").type == pa.timestamp("us", "+10:00")
    metadata = t["schema"].field("RRP").metadata
    assert metadata is not None
    assert metadata[b"PARQUET:field_id"] == str(by_name["RRP"]["field_id"]).encode()


def test_table_columns_are_what_the_command_prints() -> None:
    t = pl.table("DISPATCHPRICE")
    printed = phasor_json("list", "DISPATCHPRICE")
    want = [
        {
            "name": p["column"],
            "type": p["type"],
            "role": p["role"],
            "field_id": p["field_id"],
            "nullable": p["nullable"],
            "description": p["description"],
        }
        for p in printed
        if isinstance(p, dict)
    ]
    assert [dict(c) for c in t["columns"]] == want


def test_a_report_pair_names_its_table() -> None:
    assert pl.table("DISPATCH.PRICE")["name"] == "DISPATCHPRICE"


def test_an_unknown_table_is_a_phasor_error() -> None:
    with pytest.raises(pl.PhasorError) as e:
        pl.table("NO_SUCH_TABLE")
    assert e.value.code == "schema.unknown_table"
    assert e.value.table == "NO_SUCH_TABLE"
    assert "phasor list" in str(e.value)
