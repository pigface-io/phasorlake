"""Where a call of the Python package spends its time and memory, in zones.

Needs the module built with the feature `zones` (`just py-zones` builds it).
Each workload runs once to warm up, then RUNS times inside a Python zone,
so the Python a call makes and the Rust beneath it print as one tree. Then
the heap: what each of the module's zones allocated, reallocated (a buffer
that grew: capacity not reserved) and freed, per call; what pyarrow's pool
allocated taking the tables in (nothing, when they cross without a copy);
the bytes allocated for each byte of the tables returned; and the module's
heap left live once the tables are dropped. The workloads: the synthetic
DISPATCH_UNIT_SCADA zip `phasor-bench` ingests (one large table), and a zip
of every head fixture (many small tables); or the files given.

usage: python3 scripts/py-zones.py [FILE...]
       (`just py-zones [FILE...]`)
"""

from __future__ import annotations

import gc
import importlib
import logging
import resource
import sys
import tempfile
import time
import zipfile
from pathlib import Path
from typing import TYPE_CHECKING, Protocol, cast

import pyarrow as pa

import phasorlake as pl

if TYPE_CHECKING:
    from contextlib import AbstractContextManager

ROOT = Path(__file__).resolve().parents[1]
HEADS = ROOT / "testdata" / "csv"

RUNS = 5
"""Timed calls of each workload."""

ROWS = 524_288
"""Rows of the synthetic file, as `phasor-bench` makes it."""

EMPTY = 1_000_000
"""Empty Python zones timed, for what one costs."""


class Zones(Protocol):
    """The functions the feature `zones` adds to the native module."""

    def _zone(self, name: str) -> AbstractContextManager[object]: ...
    def _zones(self) -> str: ...
    def _memory(self) -> tuple[list[dict[str, str | int]], int]: ...


MB = 1024 * 1024

log = logging.getLogger("py-zones")


def synthetic_zip(out: Path) -> Path:
    """The zip `phasor-bench` ingests: the SCADA head's rows repeated, 5 minutes apart."""
    lines = (HEADS / "DISPATCH_UNIT_SCADA_v1_head.csv").read_text().splitlines()
    header, irow = lines[0], lines[1]
    rows = [line for line in lines[2:] if line.startswith("D,")]
    parts = [header, irow]
    for i in range(ROWS):
        prefix = rows[i % len(rows)].rsplit(",", 1)[0]
        minute = (i // len(rows)) * 5
        day, rem = divmod(minute, 1440)
        stamp = f"2024/01/{day % 28 + 1:02} {rem // 60:02}:{rem % 60:02}:00"
        parts.append(f"{prefix.replace('2024/01/01 00:05:00', stamp, 1)},{i * 0.731:.6f}")
    parts.append(f'C,"END OF REPORT",{ROWS + 3}')
    path = out / "PUBLIC_DVD_DISPATCH_UNIT_SCADA_202401010000.zip"
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("PUBLIC_DVD_DISPATCH_UNIT_SCADA_202401010000.CSV", "\r\n".join(parts) + "\r\n")
    return path


def heads_zip(out: Path) -> Path:
    """Every head fixture in one zip, as an AEMO zip holds its CSVs."""
    path = out / "heads.zip"
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        for csv in sorted(HEADS.glob("*_head.csv")):
            z.write(csv, csv.name)
    return path


def heap(zones: Zones, runs: int, out_bytes: int) -> None:
    """The module's heap, per call, by zone; and the bytes allocated per byte returned."""
    spent, _ = zones._memory()  # ruff: ignore[private-member-access] — the feature's own
    row_format = "%-12s %9.0f %9.0f %9.0f %9.2f %9.2f%s"
    log.info(
        "%-12s %9s %9s %9s %9s %9s", "zone", "allocs", "reallocs", "frees", "alloc MB", "grown MB"
    )
    for row in spent:
        calls = int(row["calls"]) / runs
        log.info(
            row_format,
            row["zone"],
            int(row["allocations"]) / runs,
            int(row["reallocations"]) / runs,
            int(row["deallocations"]) / runs,
            int(row["bytes_allocated"]) / runs / MB,
            int(row["bytes_reallocated"]) / runs / MB,
            f"   ({calls:.0f} calls a run)" if calls != 1 else "",
        )
        if row["zone"] == "py.decode" and out_bytes:
            allocated = int(row["bytes_allocated"]) / runs
            log.info("  %.2f bytes allocated a byte returned", allocated / out_bytes)


def workload(zones: Zones, f: Path) -> None:
    """Decode `f` RUNS times in a zone; print the tree, the heap, and what is held."""
    pl.decode(f)
    gc.collect()
    zones._zones()  # ruff: ignore[private-member-access] — the warm-up's zones dropped
    _, live_before = zones._memory()  # ruff: ignore[private-member-access] — the feature's own
    pool_before = pa.total_allocated_bytes()
    zone = zones._zone(f"python.decode {f.name}")  # ruff: ignore[private-member-access] — the feature's own
    start = time.perf_counter_ns()
    pool = 0
    for _ in range(RUNS):
        with zone:
            tables = pl.decode(f)
        pool = max(pool, pa.total_allocated_bytes() - pool_before)
        del tables
    ms = (time.perf_counter_ns() - start) / RUNS / 1e6
    tables = pl.decode(f)
    gc.collect()
    _, live_held = zones._memory()  # ruff: ignore[private-member-access] — the feature's own
    rows = sum(t.num_rows for t in tables.values())
    out_bytes = sum(t.nbytes for t in tables.values())
    chunks = max((c.num_chunks for t in tables.values() for c in t.columns), default=0)
    del tables
    gc.collect()
    log.info("%s: %.2f ms a call, %d rows, %.2f MB of tables", f.name, ms, rows, out_bytes / MB)
    log.info("%s", zones._zones())  # ruff: ignore[private-member-access] — the feature's own
    heap(zones, RUNS + 1, out_bytes)
    _, live_after = zones._memory()  # ruff: ignore[private-member-access] — the feature's own
    held = live_held - live_before
    log.info("pyarrow's pool, taking the tables in: %.2f MB", pool / MB)
    log.info(
        "the module's heap held while the tables are kept: %.2f MB, %.2f bytes a byte; "
        "at most %d chunk(s) a column",
        held / MB,
        held / max(out_bytes, 1),
        chunks,
    )
    left = (live_after - live_before) / MB
    log.info("the module's heap left live after the tables are dropped: %.3f MB", left)
    peak = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    log.info("peak RSS so far: %.0f MB\n", peak / (MB if sys.platform == "darwin" else 1024))


def main(argv: list[str]) -> None:
    """Time each workload and print the tree, then its memory."""
    zones = cast("Zones", importlib.import_module("phasorlake._native"))
    if not hasattr(zones, "_zones"):
        sys.exit("the module was built without the feature zones; `just py-zones` builds it")
    empty = zones._zone("python.empty")  # ruff: ignore[private-member-access] — the feature's own
    start = time.perf_counter_ns()
    for _ in range(EMPTY):
        with empty:
            pass
    per_zone = (time.perf_counter_ns() - start) / EMPTY
    with tempfile.TemporaryDirectory() as tmp:
        files = [Path(a) for a in argv] or [synthetic_zip(Path(tmp)), heads_zip(Path(tmp))]
        for f in files:
            workload(zones, f)
    log.info("an empty Python zone: %.0f ns, enter and exit", per_zone)


if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO, format="%(message)s", stream=sys.stdout)
    main(sys.argv[1:])
