# phasorlake for Python

`pip install phasorlake` gives Python the `phasor` command and its decoder.
The package is the same Rust as the command, built from the same commit: what
`phasorlake.decode` returns of a file is what `phasor ingest` writes of it,
column for column, type for type, field id for field id. The source, the
command's own documentation and the files the examples below read are at
<https://github.com/pigface-io/phasorlake>.

```sh
pip install phasorlake
phasor --version
phasor-mirror --help
```

The package does not read the lake. Every revision of every row is kept, and
"the latest", "every revision" and "as known at a time" are questions you ask
in your own SQL, with DuckDB, pyiceberg or polars:
[`docs/reading.md`](https://github.com/pigface-io/phasorlake/blob/main/docs/reading.md)
gives each as one query.

## Decode a file

A CSV, a WEMDE JSON document or a zip of them, read entry by entry, nested
zips included. Each table comes back as a `pyarrow.Table` under the
registry's lake schema, every field carrying its Iceberg field id in its
metadata (`PARQUET:field_id`).

```python
import phasorlake as pl

tables = pl.decode("testdata/csv/DISPATCHPRICE_v4_head.csv")
price = tables["DISPATCHPRICE"]
assert price.num_rows > 0
assert price.schema.field("RRP").metadata[b"PARQUET:field_id"]
print(price.select(["SETTLEMENTDATE", "REGIONID", "RRP"]).slice(0, 3))
```

A value that does not fit its column's type is kept as null, with its text in
`{column}__raw` beside it, as the lake keeps it; `strict=True` refuses such a
block instead. A malformed row is dropped with a `PhasorWarning`, up to
`max_bad_rows` of them per entry, then the entry fails. `kit="gas"` names the
kit that reads a file whose name alone does not say which (`nem`, `wem`,
`gas`).

## Inspect a file

Every block of every CSV, as `phasor inspect` lists them: its report pair,
version, columns, rows, and how it matched the catalog (`conforms`,
`deviates`, `derived`).

```python
import phasorlake as pl

for block in pl.inspect("testdata/csv/multi_block_dispatchis.csv"):
    print(block["table"], block["version"], block["status"], block["rows"])
```

## Describe a table

What the registry says of a table, as `phasor list TABLE --json` does: its
load mode, time column, columns with their field ids, and the lake schema as
a `pyarrow.Schema`. What one row is (a key) and which restatement is the
latest (temporal roles) are a reader's decisions, so the registry says
neither.

```python
import phasorlake as pl

t = pl.table("DISPATCHPRICE")
assert t["time_column"] == "SETTLEMENTDATE"
print(t["schema"])
```

## Run the command

The wheel ships the real `phasor` and `phasor-mirror` binaries; `pip install`
puts both on your PATH. `run` calls `phasor` from Python and returns its exit
code.

```python
import phasorlake as pl

assert pl.run(["--version"]) == 0
```

## Errors and warnings

A failure raises `PhasorError`. Its `code` is the diagnostic code the command
prints ([`docs/errors.md`](https://github.com/pigface-io/phasorlake/blob/main/docs/errors.md) lists every one), and `message`, `file`, `entry`,
`table` and `line` say where. What phasor noted without failing comes
through Python's `warnings` as `PhasorWarning`, the code first in its text.

```python
import phasorlake as pl

try:
    pl.table("NO_SUCH_TABLE")
except pl.PhasorError as e:
    assert e.code == "schema.unknown_table"
```

## How it stays the command

- One repository, one workspace, one version: the module is the crate
  `crates/phasor-py`, and the wheel's version is Cargo's.
- No logic in Python. `decode` runs the kit decoders through the pipeline's
  own file walk (`crates/phasor-pipeline/src/decode.rs`), the one `phasor
  ingest` uses; `inspect` and `table` call what the command's subcommands
  call.
- A test decodes a zip and ingests it with the bundled `phasor`, then holds
  the two to the same rows, types and field ids.
- The stubs (`python/phasorlake/_native.pyi`) are held to the built module by
  `mypy.stubtest`; the Python is held to `ruff` on every rule, `mypy
  --strict` and `basedpyright` strict; the examples on this page are run by
  the tests.
- A tag `vX.Y.Z` builds the binaries and the wheels together and publishes
  the wheels to PyPI.

Wheels are built for macOS (Apple silicon and Intel), Linux (x86_64 and
aarch64) and Windows (x86_64), for Python 3.10 and later. Tables cross to
Python by the Arrow C data interface, so `pyarrow`, `polars` and `nanoarrow`
all read them; `pyarrow` is the one dependency.

## Developing

`just py-dev` builds the binaries and the module into `.venv`; `just
py-test`, `py-type`, `py-lint` and `py-fmt-check` are the hooks; `just
py-wheel` builds the wheel into `dist/`.
