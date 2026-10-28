# Reads a table the store wrote with readers that share no code with it:
# pyiceberg (scan, and time travel to every snapshot), DuckDB's iceberg_scan,
# and fastavro over every manifest and manifest list.
#
#   PHASOR_KEEP_TABLE=$dir cargo llvm-cov -p phasor-iceberg-store ...
#   uv run --with 'pyiceberg[pyarrow]' --with duckdb --with fastavro \
#     python scripts/iceberg-readers.py $dir/dispatchprice
#
# Exits non-zero when a reader disagrees with what the store's test wrote.

import json
import sys
from pathlib import Path

import duckdb
import fastavro
from pyiceberg.table import StaticTable

meta_dir = Path(sys.argv[1]) / "metadata"
version = (meta_dir / "version-hint.text").read_text().strip()
location = str(meta_dir / f"v{version}.metadata.json")

for path in sorted(meta_dir.glob("*.avro")):
    with path.open("rb") as f:
        reader = fastavro.reader(f)
        records = list(reader)
        schema = json.loads(reader.metadata["avro.schema"])
    print(f"fastavro {path.name}: {schema['name']}, {len(records)} records")

t = StaticTable.from_metadata(location)
print("pyiceberg: format", t.metadata.format_version, "schema", t.metadata.current_schema_id)
rows = t.scan().to_arrow().sort_by("settlementdate")
names = rows.schema.names
assert names == ["settlementdate", "regionid", "raise1secrrp", "rrp"], names
assert rows.num_rows == 6, rows.num_rows
assert rows.column("raise1secrrp").null_count == 3
assert str(rows.schema.field("rrp").type) == "decimal128(18, 5)"
snapshots = sorted(t.metadata.snapshots, key=lambda s: s.sequence_number)
seen = [t.scan(snapshot_id=s.snapshot_id).to_arrow().num_rows for s in snapshots]
assert seen == [2, 4, 5, 6], seen
print("pyiceberg: rows", rows.num_rows, "per snapshot", seen)

con = duckdb.connect()
con.execute("INSTALL iceberg; LOAD iceberg;")
got = con.execute(
    f"SELECT count(*), count(raise1secrrp), max(rrp) FROM iceberg_scan('{location}')"
).fetchone()
assert got[0] == 6 and got[1] == 3, got
print("duckdb:", got)
print("all readers agree")
