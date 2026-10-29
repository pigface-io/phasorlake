r"""Reads a table the store wrote with readers that share no code with it.

pyiceberg (a scan, and time travel to every snapshot), DuckDB's `iceberg_scan`,
and fastavro over every manifest and manifest list.

    PHASOR_KEEP_TABLE=$dir cargo llvm-cov -p phasor-iceberg-store ...
    uv run --with 'pyiceberg[pyarrow]' --with duckdb --with fastavro \\
        python scripts/iceberg-readers.py $dir/dispatchprice

Exits non-zero when a reader disagrees with what the store's test wrote.
"""

from __future__ import annotations

import json
import logging
import sys
from pathlib import Path
from typing import NoReturn

import duckdb
import fastavro
from pyiceberg.table import StaticTable

COLUMNS = ["settlementdate", "regionid", "raise1secrrp", "rrp"]
ROWS = 6
NULLS = 3
ROWS_PER_SNAPSHOT = [2, 4, 5, 6]

log = logging.getLogger("iceberg-readers")


def disagree(reader: str, what: str, got: object) -> NoReturn:
    """Leave: `reader` read something other than what was written.

    Raises:
        SystemExit: Always, naming the reader and what it read.
    """
    msg = f"{reader}: {what}: {got!r}"
    raise SystemExit(msg)


def main(table_dir: Path) -> None:
    """Read `table_dir` with the three readers.

    Raises:
        SystemExit: A reader disagrees with what the store's test wrote.
    """
    meta_dir = table_dir / "metadata"
    version = (meta_dir / "version-hint.text").read_text().strip()
    location = str(meta_dir / f"v{version}.metadata.json")

    for path in sorted(meta_dir.glob("*.avro")):
        with path.open("rb") as f:
            reader = fastavro.reader(f)
            records = list(reader)
            schema = json.loads(reader.metadata["avro.schema"])
        log.info("fastavro %s: %s, %d records", path.name, schema["name"], len(records))

    t = StaticTable.from_metadata(location)
    log.info(
        "pyiceberg: format %s schema %s", t.metadata.format_version, t.metadata.current_schema_id
    )
    rows = t.scan().to_arrow().sort_by("settlementdate")
    if rows.schema.names != COLUMNS:
        disagree("pyiceberg", "columns", rows.schema.names)
    if rows.num_rows != ROWS:
        disagree("pyiceberg", "rows", rows.num_rows)
    if rows.column("raise1secrrp").null_count != NULLS:
        disagree("pyiceberg", "nulls", rows.column("raise1secrrp").null_count)
    if str(rows.schema.field("rrp").type) != "decimal128(18, 5)":
        disagree("pyiceberg", "rrp type", rows.schema.field("rrp").type)
    snapshots = sorted(t.metadata.snapshots, key=lambda s: s.sequence_number or 0)
    seen = [t.scan(snapshot_id=s.snapshot_id).to_arrow().num_rows for s in snapshots]
    if seen != ROWS_PER_SNAPSHOT:
        disagree("pyiceberg", "rows per snapshot", seen)
    log.info("pyiceberg: rows %d per snapshot %s", rows.num_rows, seen)

    con = duckdb.connect()
    con.execute("INSTALL iceberg; LOAD iceberg;")
    got = con.execute(
        f"SELECT count(*), count(raise1secrrp), max(rrp) FROM iceberg_scan('{location}')"  # ruff: ignore[hardcoded-sql-expression] — the path is this script's own argument
    ).fetchone()
    if got is None or got[0] != ROWS or got[1] != NULLS:
        disagree("duckdb", "counts", got)
    log.info("duckdb: %s", got)
    log.info("all readers agree")


if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO, format="%(message)s", stream=sys.stdout)
    main(Path(sys.argv[1]))
