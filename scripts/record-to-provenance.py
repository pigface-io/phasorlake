"""Give each file of an exogen raw tree the sidecar phasor's archive reads.

exogen's fetchers wrote `{file}.record.json` beside each file: the same
facts as phasor's `{file}.provenance.json` (size, sha256, tool, the
addresses the bytes were seen at) less the CRC-32 the archive also checks,
and with a `listed.catalog` the provenance record has no field for. This
reads each record, computes the CRC-32 of the file, and writes the
provenance record beside it; the record stays. A file whose size or
sha256 is not the record's is reported and gets no sidecar.

Nothing is written without `--write`; `--sha256` also recomputes the
hash (a second read of every file).

usage: python3 -I scripts/record-to-provenance.py RAW_DIR [--write] [--sha256]
"""

from __future__ import annotations

import hashlib
import json
import logging
import sys
import zlib
from dataclasses import dataclass
from pathlib import Path

RECORD = ".record.json"
PROVENANCE = ".provenance.json"
CHUNK = 1 << 20

log = logging.getLogger("record-to-provenance")


@dataclass(frozen=True)
class Args:
    """The command line."""

    raw: Path
    write: bool
    sha256: bool

    @classmethod
    def parse(cls, argv: list[str]) -> Args:
        """The arguments.

        Returns:
            The tree and the flags.

        Raises:
            SystemExit: No tree given; the usage is the message.
        """
        if len(argv) < 2:  # ruff: ignore[magic-value-comparison] — the tree
            raise SystemExit(__doc__)
        return cls(Path(argv[1]), "--write" in argv[2:], "--sha256" in argv[2:])


@dataclass(frozen=True)
class Outcome:
    """One record's result."""

    path: Path
    problem: str | None


@dataclass(frozen=True)
class Digests:
    """What one read of a file gives."""

    size: int
    crc32: int
    sha256: str | None


def digests(path: Path, *, with_sha256: bool) -> Digests:
    """The file's size, CRC-32 and (when wanted) sha256, in one read.

    Returns:
        The digests; the sha256 is `None` unless asked for.
    """
    size = 0
    crc = 0
    sha = hashlib.sha256() if with_sha256 else None
    with path.open("rb") as f:
        while chunk := f.read(CHUNK):
            size += len(chunk)
            crc = zlib.crc32(chunk, crc)
            if sha is not None:
                sha.update(chunk)
    return Digests(size, crc, sha.hexdigest() if sha is not None else None)


def address(a: dict[str, object]) -> dict[str, object]:
    """An exogen address as a provenance address: `listed.catalog` has no field, so it goes.

    Returns:
        The address with `url`, `seen`, `via`, and `listed`/`served` where they hold a field.
    """
    out: dict[str, object] = {"url": a["url"], "seen": a["seen"], "via": a.get("via", "fetched")}
    listed = a.get("listed")
    if isinstance(listed, dict):
        kept = {k: v for k, v in listed.items() if k in {"size", "at", "modified"}}
        if "modified" in kept:
            kept["at"] = kept.pop("modified")
        if kept:
            out["listed"] = kept
    served = a.get("served")
    if isinstance(served, dict):
        kept = {k: v for k, v in served.items() if k in {"etag", "modified"}}
        if kept:
            out["served"] = kept
    return out


def convert(record_path: Path, args: Args) -> Outcome:
    """One record: checked against its file, its provenance written when asked.

    Returns:
        What happened to it.
    """
    file = record_path.with_name(record_path.name[: -len(RECORD)])
    sidecar = file.with_name(file.name + PROVENANCE)
    if not file.is_file():
        return Outcome(record_path, "no file beside the record")
    if sidecar.exists():
        return Outcome(record_path, None)
    record = json.loads(record_path.read_text(encoding="utf-8"))
    d = digests(file, with_sha256=args.sha256)
    if d.size != record.get("size"):
        return Outcome(record_path, f"size {d.size} is not the record's {record.get('size')}")
    if d.sha256 is not None and d.sha256 != record.get("sha256"):
        return Outcome(record_path, "sha256 is not the record's")
    provenance = {
        "size": d.size,
        "sha256": record["sha256"],
        "crc32": d.crc32,
        "tool": record.get("tool", "exogen"),
        "addresses": [address(a) for a in record.get("addresses", [])],
    }
    if args.write:
        tmp = sidecar.with_name(sidecar.name + ".tmp")
        tmp.write_text(json.dumps(provenance, indent=2) + "\n", encoding="utf-8")
        tmp.replace(sidecar)
    return Outcome(record_path, None)


def main(argv: list[str]) -> int:
    """Converts every record under the tree; logs a summary and each problem.

    Returns:
        1 when any record had a problem, else 0.
    """
    args = Args.parse(argv)
    records = sorted(args.raw.rglob(f"*{RECORD}"))
    outcomes = [convert(r, args) for r in records]
    bad = [o for o in outcomes if o.problem is not None]
    for o in bad:
        log.warning("%s: %s", o.path, o.problem)
    verb = "written" if args.write else "would be written (pass --write)"
    log.info(
        "%d records, %d provenance sidecars %s, %d bad",
        len(records),
        len(records) - len(bad),
        verb,
        len(bad),
    )
    return 1 if bad else 0


if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO, format="%(message)s")
    sys.exit(main(sys.argv))
