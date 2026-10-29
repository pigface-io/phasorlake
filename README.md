# phasor

Downloads AEMO market data and writes it to a local Parquet lake.
Covers the NEM, the WEM and the east-coast gas markets. One Rust binary.

## Quick start

```sh
cargo install --path crates/phasor-cli --locked
phasor fetch DISPATCH_UNIT_SCADA --from 2024-01-15 --to 2024-01-18
```

```sql
select count(*)
from read_parquet('~/.phasor/lake/nem/dispatch_unit_scada/data/**/*.parquet',
                  hive_partitioning = true);
```

Run `fetch` again and it does nothing: the file is archived and the
manifest knows it is in the lake.

## Pipeline

```
 AEMO hosts                   nemweb: MMSDM monthly archive, Reports/CURRENT, Reports/ARCHIVE
     |                        WEM datafiles and WEMDE, gas feeds (GBB, GSH, STTM, DWGM)
     |  fetch: 600 ms between requests, stop on refusal,
     |  each file once, only the zip entries asked for
     v
 archive/                     bytes as served + .provenance.json (sha256, URL, ETag, time)
     |
     |  ingest (no network)
     v
 decode                       MMS CSV: C/I/D rows, one block per I row
     |                        plain CSV and WEMDE through the same sink
     v
 registry                     block (type, subtype, version, columns) -> table schema
     |                        conforms | deviates (extra columns kept as text)
     |                        | derived (unknown table, every column text)
     v
 conform                      text -> typed Arrow, exact decimals, UTC microseconds
     |                        bad value: null + {col}__raw   bad row: quarantine/
     v
 writer                       month partitions, write to tmp, rename when done
     |
     +--> lake/{market}/{table}/data/year=YYYY/month=MM/*.parquet
     +--> lake/{market}/{table}/metadata/       Iceberg v3
     +--> manifest.sqlite                       runs, files, ingests, outputs, errors
```

Months come from the monthly archive. Days and five-minute files come
from the report folders. `fetch` never mixes the two within a month.

phasor writes the lake and does not read it. Every revision is kept, so
"latest" and "as known at" are your SQL. See `docs/reading.md`.

## Commands

| command | does |
|---|---|
| `fetch T --from A --to B` | download what covers the window and ingest it. `--no-ingest`, `--dry-run`, `--day D` |
| `fetch --documents` | AEMO's registration list, a new version kept beside the old |
| `ingest [T --from A --to B]` | archive to lake, offline. No args: everything not yet ingested or out of date |
| `import <zip\|csv\|dir>` | files you already have |
| `list [T]` | tables, time columns, coverage, files, manifest |
| `inspect <file>` | blocks in a file and how each resolves |
| `errors` | diagnostics from past runs |
| `verify [--archive]` | lake files exist; `--archive` rehashes downloads |
| `schema list\|show\|history\|validate\|plan` | the registry |
| `iceberg` | rebuild Iceberg metadata from the manifest |
| `wem manifest <dataset>` | a WEM dataset's file list |

Every `--help` ends with examples. Global flags: `--home DIR`
(`PHASOR_HOME`), `--json`, `--offline`, `--jobs N`.

## Layout

```
~/.phasor/
  archive/{host}/{url path}                   downloads, never changed
  archive/{host}/{zip}!{entry}                one zip entry, fetched by range
  lake/{market}/{table}/data/year=/month=/    appended tables
  lake/{market}/{table}/data/snapshot=YYYYMM/ standing data, whole table per month
  lake/nem/_unregistered/{type}.{subtype}/    tables no schema declares
  quarantine/{table}/{source}.jsonl           malformed rows
  manifest.sqlite
  config.toml
```

Each Parquet footer records its source URL and sha256, the block's shape,
and a hash of the plan used to read it. When a schema changes the plan
hash changes, and `phasor ingest` reads that source again from the archive.

How files are cut is `[lake]` in `config.toml`; it never changes what they
hold, and each footer records it (`phasor.write_options`). The same rows
and layout always give the same bytes.

```toml
[lake]
file_size_mb = 128         # a file is closed for the next at about this size
row_group_rows = 262144
row_group_cells = 4194304  # or this many values, so a wide table's are shorter
page_kb = 1024
page_rows = 100000
dictionary_page_kb = 1024
compression = "zstd"       # zstd, lz4, snappy or none
zstd_level = 3
statistics = "page"        # none, chunk or page
```

## Types

| schema | Arrow | Iceberg |
|---|---|---|
| `string` | `Utf8` | `string` |
| `double` | `Float64` | `double` |
| `long` | `Int64` | `long` |
| `boolean` | `Boolean` | `boolean` |
| `date` | `Date32` | `date` |
| `timestamp` | `Timestamp(µs, +10:00)` | `timestamptz` |
| `decimal(P,S)` | `Decimal128(P,S)` | `decimal(P,S)` |

Times are UTC microseconds with the market offset in the type. AEMO's
"no end" dates (`9999/12/31`) are clamped to 2262-04-11 and reported.

## Schemas

Table histories come from phasor-schemas, pinned by rev and copied into
`schemas/nem/pulled/` (`just schemas-pull REV`). Put your own table in
`~/.phasor/schemas/nem/{TABLE}/table.toml`; `phasor schema show T` prints
one to start from. Details: `docs/iceberg.md`.

## Errors

Each diagnostic has a stable code and a full sentence, one JSON line on
stderr. Exit codes: 0 all files ok, 3 some, 1 none, 4 not archived while
offline, 5 unresolved schema with `--strict`. Codes: `docs/errors.md`.

## Python

`pip install phasorlake` gives Python the `phasor` and `phasor-mirror`
commands and the decoder: `phasorlake.decode(path)` returns every table in
a file as a `pyarrow.Table`, exactly as `phasor ingest` writes it; `inspect`
and `table` are `phasor inspect` and `phasor list TABLE`. See `docs/python.md`.

## Mirror

`phasor-mirror` keeps a full copy of nemweb's archive folders, peeking
into zips before it downloads them. See `crates/phasor-mirror/README.md`.

## Developing

```sh
just setup   # pinned tools and git hooks
just check   # every hook, as CI runs it
just bench   # benchmarks against bench/budgets.json
```

Layers and rules: `CONTRIBUTING.md`. Other docs: `docs/semantics.md`,
`docs/layout.md`.

## Licence

Apache 2.0, see `LICENSE` and `NOTICE`. AEMO excerpts under `testdata/`
are AEMO's, see `testdata/LICENCE.md`.
