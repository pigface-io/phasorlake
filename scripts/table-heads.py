"""The first rows of one real archived file of every table, for fixtures.

Reads the mirror's archive only. For each NEM table, its file in the newest
MMSDM month that has one; for each gas folder the census saw, its newest
file of each report; for each WEM or New Zealand folder the census saw, its
newest file. An MMS file keeps its C row, the table's I row, five D rows and
a footer; a plain CSV its header and five rows; a WEMDE document its scalars
and the first five elements of each array. Writes under OUT, the gas, WEM and
NZ files at their paths below their hosts, so each feed reads its own.

usage: python3 -I scripts/table-heads.py SCHEMAS ARCHIVE_HOST OUT [gas|wem|nz|all]
                                         [WA_HOST] [NZ_HOST]
       (gas, wem, nz: that market's feeds only; WA_HOST and NZ_HOST: those hosts'
       directories in the mirror archive, `{ARCHIVE_HOST}/../data.wa.aemo.com.au`
       and `{ARCHIVE_HOST}/../emidatasets.blob.core.windows.net` unless given)
       (`just table-heads {home}` writes testdata/heads)
"""

from __future__ import annotations

import io
import json
import logging
import re
import sys
import zipfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import TextIO

ROWS = 5
"""Data rows kept of each block."""

log = logging.getLogger("table-heads")

LINES_SCANNED = 200_000
"""Lines read of one file before giving up on the blocks still wanted."""

MONTHS_SEARCHED = 36
"""MMSDM months looked through, newest first."""

Pair = tuple[str, str]
"""A report pair: `(REPORT_TYPE, REPORT_SUBTYPE)`."""

STEM_TAIL = re.compile(r"(_\d{4,14})+(?=\.|~|$)|~.*$")
"""The dated tail of a plain file's name, so one file of each stem is kept."""

ARCHIVE_NAME = re.compile(r"PUBLIC_(?:ARCHIVE#|DVD_)([A-Z0-9_]+?)(?:#FILE\d+#|_)\d{12}")


@dataclass
class Args:
    """The command line."""

    schemas: Path
    host: Path
    out: Path
    only: str | None
    wa: Path
    nz: Path

    @classmethod
    def parse(cls, argv: list[str]) -> Args:
        """The arguments.

        Returns:
            The three paths, the optional market and the WA and NZ hosts' directories.

        Raises:
            SystemExit: Too few arguments; the usage is the message.
        """
        if len(argv) < 4:  # ruff: ignore[magic-value-comparison] — the three paths
            raise SystemExit(__doc__)
        schemas, host, out = (Path(a) for a in argv[1:4])
        only = argv[4] if len(argv) > 4 else None  # ruff: ignore[magic-value-comparison] — the optional fourth
        only = None if only == "all" else only
        wa = Path(argv[5]) if len(argv) > 5 else host.parent / "data.wa.aemo.com.au"  # ruff: ignore[magic-value-comparison] — the optional fifth
        nz = Path(argv[6]) if len(argv) > 6 else host.parent / "emidatasets.blob.core.windows.net"  # ruff: ignore[magic-value-comparison] — the optional sixth
        return cls(schemas, host, out, only, wa, nz)

    def wants(self, market: str) -> bool:
        """Whether `market`'s heads are asked for: every market, or the one named."""
        return self.only is None or self.only == market


def pairs(schemas: Path, market: str) -> dict[Pair, str]:
    """Report pair to table name, from the pulled metadata."""
    m: dict[Pair, str] = {}
    for md in sorted((schemas / market / "pulled").glob("*/metadata")):
        if not md.is_dir():
            continue
        v = sorted(md.glob("*.metadata.json"))
        if not v:
            continue
        props = json.loads(v[-1].read_text()).get("properties", {})
        t = md.parent.name
        for rt, st in json.loads(props.get("phasor.reports", "[]")):
            m[str(rt).upper(), str(st).upper()] = t
    return m


def lines_of(path: Path) -> tuple[str, TextIO] | None:
    """The name and text lines of a file, or of the first CSV inside a zip."""
    if path.suffix.lower() == ".zip":
        z = zipfile.ZipFile(path)
        names = [n for n in z.namelist() if n.lower().endswith(".csv")]
        if not names:
            return None
        return names[0].rsplit("/", 1)[-1], io.TextIOWrapper(z.open(names[0]), "latin-1")
    return path.name, path.open(encoding="latin-1")


@dataclass
class Heads:
    """The head of each wanted block of one MMS file, as it is read."""

    wanted: dict[Pair, str]
    first: str = ""
    got: dict[str, list[str]] = field(default_factory=dict)
    current: str | None = None

    def full(self, table: str) -> bool:
        """Whether `table` has its rows."""
        return len(self.got.get(table, [])) > ROWS

    def done(self) -> bool:
        """Whether every wanted table has its rows."""
        return all(self.full(t) for t in set(self.wanted.values()))

    def read(self, n: int, line: str) -> None:
        """Takes line `n` of the file."""
        if n == 0:
            self.first = line
            return
        if line.startswith("I,"):
            p = line.split(",")
            t = self.wanted.get((p[1].upper(), p[2].upper())) if len(p) > 2 else None  # ruff: ignore[magic-value-comparison] — type and subtype
            self.current = t if t is not None and not self.full(t) else None
            if self.current is not None and self.current not in self.got:
                self.got[self.current] = [line]
        elif line.startswith("D,") and self.current is not None and not self.full(self.current):
            self.got[self.current].append(line)

    def files(self, name: str) -> dict[str, tuple[str, str]]:
        """Each table's head as a whole MMS file: `{table: (name, text)}`."""
        out: dict[str, tuple[str, str]] = {}
        for t, body in self.got.items():
            rows = [self.first, *body]
            rows.append(f'C,"END OF REPORT",{len(rows) + 1}')
            out[t] = (name, "\r\n".join(rows) + "\r\n")
        return out


def mms_head(path: Path, wanted: dict[Pair, str]) -> dict[str, tuple[str, str]]:
    """Head of each wanted block of an MMS file: `{table: (name, text)}`."""
    opened = lines_of(path)
    if opened is None:
        return {}
    name, f = opened
    heads = Heads(wanted)
    with f:
        for n, raw in enumerate(f):
            heads.read(n, raw.rstrip("\r\n"))
            if n > LINES_SCANNED or heads.done():
                break
    return heads.files(name)


def dvd_name(table: str, text: str) -> str:
    """A head's name as its month's DVD file.

    Returns:
        `PUBLIC_DVD_{table}_{yyyymm}010000.CSV` for the month before the C
        row's date, which is when AEMO wrote the archive.
    """
    c = text.split("\n", 1)[0].split(",")
    y, m = int(c[5][:4]), int(c[5][5:7])
    y, m = (y, m - 1) if m > 1 else (y - 1, 12)
    return f"PUBLIC_DVD_{table}_{y:04d}{m:02d}010000.CSV"


def write(out: Path, rel: Path, text: str) -> None:
    """Writes `text` at `rel` under `out`, bytes as they came."""
    p = out / rel
    p.parent.mkdir(parents=True, exist_ok=True)
    with p.open("w", encoding="latin-1", newline="") as w:
        w.write(text)


def data_files(d: Path) -> list[str]:
    """The data files of a mirror folder, by name; none for a folder that is not there."""
    try:
        entries = list(d.iterdir())
    except OSError:
        return []
    return sorted(
        e.name
        for e in entries
        if not e.name.startswith("._")
        and not e.name.endswith((".json", ".part", ".resume", ".tmp"))
        and e.is_file()
    )


def stem_of(name: str) -> str:
    """A plain file's name without its extension and dated tail, lower case."""
    return STEM_TAIL.sub("", name.rsplit(".", 1)[0]).lower()


def head_lines(f: TextIO) -> list[str]:
    """The header and the first rows of a plain CSV."""
    return [line.rstrip("\r\n") for _, line in zip(range(ROWS + 1), f, strict=False)]


def nem_heads(args: Args) -> tuple[dict[str, str], set[str]]:
    """The NEM tables' heads from the newest MMSDM months, back until every table has one."""
    nem = pairs(args.schemas, "nem")
    mmsdm = args.host / "Data_Archive/Wholesale_Electricity/MMSDM"
    months: list[Path] = []
    for y in sorted(mmsdm.iterdir(), reverse=True):
        if y.name.isdigit():
            months.extend(sorted(y.iterdir(), reverse=True))
    want_tables = set(nem.values()) if args.wants("nem") else set()
    done: dict[str, str] = {}
    for mdir in months[: 0 if not want_tables else MONTHS_SEARCHED]:
        if want_tables <= set(done):
            break
        for sub in ("DATA", "P5MIN_ALL_DATA", "PREDISP_ALL_DATA"):
            d = mdir / "MMSDM_Historical_Data_SQLLoader" / sub
            for n in data_files(d):
                m = ARCHIVE_NAME.match(n)
                if not m or m.group(1) in done or m.group(1) not in want_tables:
                    continue
                table = m.group(1)
                wanted = {k: v for k, v in nem.items() if v == table}
                for t, (_, text) in mms_head(d / n, wanted).items():
                    if t not in done:
                        write(args.out, Path("nem", dvd_name(t, text)), text)
                        done[t] = n
        log.info("nem %d of %d after %s", len(done), len(want_tables), mdir)
    return done, want_tables


def plain_texts(d: Path, n: str) -> list[tuple[str, list[str]]]:
    """The heads of a plain file: each CSV of a zip of reports, or the CSV itself."""
    path = d / n
    if n.lower().endswith(".zip") or ".zip@" in n.lower():
        try:
            z = zipfile.ZipFile(path)
        except (zipfile.BadZipFile, OSError):
            return []
        members = [m for m in z.namelist() if m.lower().endswith(".csv")]
        return [
            (m.rsplit("/", 1)[-1], head_lines(io.TextIOWrapper(z.open(m), "latin-1")))
            for m in members
        ]
    if not n.lower().endswith(".csv"):
        return []
    with path.open(encoding="latin-1") as f:
        return [(n, head_lines(f))]


def gas_heads(args: Args) -> dict[str | tuple[str, str], str]:
    """Each gas folder the census saw: the newest file of each report it carried."""
    gas = pairs(args.schemas, "gas")
    folders = json.loads((args.schemas / "gas/pulled/folders.json").read_text())["folders"]
    gas_done: dict[str | tuple[str, str], str] = {}
    for fo in folders:
        # A folder named as its root's last part is the root itself.
        root, folder = str(fo["root"]), str(fo["folder"])
        rel = root if root.rsplit("/", 1)[-1] == folder else f"{root}/{folder}"
        d = args.host / rel
        files = data_files(d)
        if not files:
            continue
        if fo["feed"] == "gsh":
            reports: list[Pair] = [(str(b["report"][0]), str(b["report"][1])) for b in fo["blocks"]]
            wanted: dict[Pair, str] = {
                r: gas[r] for r in reports if r in gas and gas[r] not in gas_done
            }
            for n in reversed(files):
                if not wanted:
                    break
                for t, (name, text) in mms_head(d / n, wanted).items():
                    write(args.out, Path("gas", root, folder, name), text)
                    gas_done[t] = n
                    wanted = {k: v for k, v in wanted.items() if v != t}
        else:
            # A plain feed: one file of each name stem (its date tail off).
            stems = {stem_of(n): n for n in files}
            for _, n in sorted(stems.items()):
                for name, head in plain_texts(d, n):
                    key = stem_of(name)
                    if len(head) < 2 or (rel, key) in gas_done:  # ruff: ignore[magic-value-comparison] — a header and a row
                        continue
                    write(args.out, Path("gas", rel, name), "\r\n".join(head) + "\r\n")
                    gas_done[rel, key] = n
        log.info("gas %d %s", len(gas_done), folder)
    return gas_done


def wemde_head(path: Path) -> tuple[str, str] | None:
    """A WEMDE zip's document cut to the first rows of each array under `data`.

    Returns:
        The document's name and text; `None` for a zip with no document.
    """
    try:
        z = zipfile.ZipFile(path)
    except (zipfile.BadZipFile, OSError):
        return None
    members = [m for m in z.namelist() if m.lower().endswith(".json")]
    if not members:
        return None
    doc = json.loads(z.read(members[0]))
    # A day published again comes as an array of documents: the first is one.
    if isinstance(doc, list):
        doc = doc[0]
    data = doc.get("data", {})
    for key, value in list(data.items()):
        if isinstance(value, list):
            data[key] = value[:ROWS]
    return members[0].rsplit("/", 1)[-1], json.dumps(doc, separators=(",", ":"))


def wem_heads(args: Args) -> dict[tuple[str, str], str]:
    """Each WEM folder the census saw: its newest file, a CSV's head or a WEMDE document's."""
    wem_done: dict[tuple[str, str], str] = {}
    folders = json.loads((args.schemas / "wem/pulled/folders.json").read_text())["folders"]
    for fo in folders:
        root, folder, feed = str(fo["root"]), str(fo["folder"]), str(fo["feed"])
        d = args.wa / root / folder
        files = data_files(d)
        if not files:
            continue
        newest = files[-1]
        if feed == "wemde":
            head = wemde_head(d / newest)
            if head is not None:
                write(args.out, Path("wem", root, folder, head[0]), head[1])
                wem_done[root, folder] = newest
        else:
            for name, lines in plain_texts(d, newest):
                if len(lines) < 2:  # ruff: ignore[magic-value-comparison] — a header and a row
                    continue
                write(args.out, Path("wem", root, folder, name), "\r\n".join(lines) + "\r\n")
                wem_done[root, folder] = newest
        log.info("wem %d %s", len(wem_done), folder)
    return wem_done


def nz_heads(args: Args) -> dict[tuple[str, str], str]:
    """Each New Zealand folder the census saw: its newest CSV's head.

    A folder's files are a day's or a month's and all one shape, so the
    newest one stands for the folder; the head keeps the file's name, which
    names the table (`20261104_FinalEnergyPrices.csv`).

    Returns:
        `{(root, folder): newest file name}` for each folder that had a head.
    """
    nz_done: dict[tuple[str, str], str] = {}
    folders = json.loads((args.schemas / "nz/pulled/folders.json").read_text())["folders"]
    for fo in folders:
        root, folder = str(fo["root"]), str(fo["folder"])
        d = args.nz / root / folder
        files = data_files(d)
        if not files:
            continue
        newest = files[-1]
        for name, lines in plain_texts(d, newest):
            if len(lines) < 2:  # ruff: ignore[magic-value-comparison] — a header and a row
                continue
            write(args.out, Path("nz", root, folder, name), "\n".join(lines) + "\n")
            nz_done[root, folder] = newest
        log.info("nz %d %s", len(nz_done), folder)
    return nz_done


def main(argv: list[str]) -> None:
    """Write the heads; log what was found as JSON."""
    args = Args.parse(argv)
    done, want_tables = nem_heads(args)
    gas_done = gas_heads(args) if args.wants("gas") else {}
    wem_done = wem_heads(args) if args.wants("wem") else {}
    nz_done = nz_heads(args) if args.wants("nz") else {}
    found = {
        "nem": len(done),
        "nem_tables": len(want_tables),
        "nem_missing": sorted(want_tables - set(done)),
        "gas_files": len(gas_done),
        "wem_files": len(wem_done),
        "nz_files": len(nz_done),
    }
    log.info("%s", json.dumps(found, default=str))


if __name__ == "__main__":
    # Progress as it goes, then what was found, on stderr.
    logging.basicConfig(level=logging.INFO, format="%(message)s")
    main(sys.argv)
