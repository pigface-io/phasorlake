"""The first rows of one real archived file of every table, for fixtures.

Reads the mirror's archive only. For each NEM table, its file in the newest
MMSDM month that has one; for each gas folder the census saw, its newest
file of each report. An MMS file keeps its C row, the table's I row, five D
rows and a footer; a plain CSV its header and five rows. Writes under OUT,
the gas files at their paths below the host, so each feed reads its own.

usage: python3 -I scripts/table-heads.py SCHEMAS ARCHIVE_HOST OUT [gas]
       (gas: the gas feeds only)
       (`just table-heads {home}` writes testdata/heads)
"""

import io
import json
import re
import sys
import zipfile
from pathlib import Path

schemas, host, out = (Path(a) for a in sys.argv[1:4])
only = sys.argv[4] if len(sys.argv) > 4 else None
ROWS = 5


def pairs(market):
    """Report pair -> table name, from the pulled metadata."""
    m = {}
    for md in sorted((schemas / market / "pulled").glob("*/metadata")):
        if not md.is_dir():
            continue
        v = sorted(md.glob("*.metadata.json"))
        if not v:
            continue
        props = json.loads(v[-1].read_text()).get("properties", {})
        t = md.parent.name
        for rt, st in json.loads(props.get("phasor.reports", "[]")):
            m[(rt.upper(), st.upper())] = t
    return m


def lines_of(path):
    """The text lines of a file, or of the first CSV inside a zip."""
    if path.suffix.lower() == ".zip":
        z = zipfile.ZipFile(path)
        names = [n for n in z.namelist() if n.lower().endswith(".csv")]
        if not names:
            return None, None
        return names[0].rsplit("/", 1)[-1], io.TextIOWrapper(z.open(names[0]), "latin-1")
    return path.name, path.open(encoding="latin-1")


def mms_head(path, wanted):
    """Head of each wanted block of an MMS file: {table: (name, text)}."""
    name, f = lines_of(path)
    if f is None:
        return {}
    first = None
    got = {}
    cur = None
    tables = set(wanted.values())
    full = lambda t: len(got.get(t, [])) > ROWS
    for n, line in enumerate(f):
        line = line.rstrip("\r\n")
        if n == 0:
            first = line
            continue
        if line.startswith("I,"):
            p = line.split(",")
            t = wanted.get((p[1].upper(), p[2].upper())) if len(p) > 2 else None
            cur = t if t is not None and not full(t) else None
            if cur is not None and cur not in got:
                got[cur] = [line]
        elif line.startswith("D,") and cur is not None and not full(cur):
            got[cur].append(line)
        if n > 200000 or all(full(t) for t in tables):
            break
    out_ = {}
    for t, body in got.items():
        rows = [first] + body
        rows.append('C,"END OF REPORT",%d' % (len(rows) + 1))
        out_[t] = (name, "\r\n".join(rows) + "\r\n")
    return out_


def dvd_name(table, text):
    """A head's name as its month's DVD file: the month before the C row's
    date, which is when AEMO wrote the archive."""
    c = text.split("\n", 1)[0].split(",")
    y, m = int(c[5][:4]), int(c[5][5:7])
    y, m = (y, m - 1) if m > 1 else (y - 1, 12)
    return "PUBLIC_DVD_%s_%04d%02d010000.CSV" % (table, y, m)


def write(rel, text):
    p = out / rel
    p.parent.mkdir(parents=True, exist_ok=True)
    with p.open("w", encoding="latin-1", newline="") as w:
        w.write(text)


def data_files(d):
    try:
        entries = list(d.iterdir())
    except OSError:
        return []
    return sorted(
        e.name for e in entries
        if not e.name.startswith("._") and not e.name.endswith((".json", ".part", ".resume", ".tmp"))
        and e.is_file()
    )


done = {}
# NEM: the newest MMSDM months, back until every table has a head.
nem = pairs("nem")
mmsdm = host / "Data_Archive/Wholesale_Electricity/MMSDM"
months = []
for y in sorted(mmsdm.iterdir(), reverse=True):
    if not y.name.isdigit():
        continue
    months.extend(sorted(y.iterdir(), reverse=True))
want_tables = set(nem.values())
for mdir in ([] if only == "gas" else months[:36]):
    if want_tables <= set(done):
        break
    for sub in ("DATA", "P5MIN_ALL_DATA", "PREDISP_ALL_DATA"):
        d = mdir / "MMSDM_Historical_Data_SQLLoader" / sub
        for n in data_files(d):
            m = re.match(r"PUBLIC_(?:ARCHIVE#|DVD_)([A-Z0-9_]+?)(?:#FILE\d+#|_)\d{12}", n)
            if not m or m.group(1) in done or m.group(1) not in want_tables:
                continue
            table = m.group(1)
            wanted = {k: v for k, v in nem.items() if v == table}
            for t, (name, text) in mms_head(d / n, wanted).items():
                if t not in done:
                    write(Path("nem", dvd_name(t, text)), text)
                    done[t] = n
    print("nem", len(done), "of", len(want_tables), "after", mdir, file=sys.stderr, flush=True)

# Gas: each folder the census saw, newest file per report it carried.
gas = pairs("gas")
folders = json.loads((schemas / "gas/pulled/folders.json").read_text())["folders"]
gas_done = {}
for fo in folders:
    # A folder named as its root's last part is the root itself.
    whole = fo["root"].rsplit("/", 1)[-1] == fo["folder"]
    rel = fo["root"] if whole else f'{fo["root"]}/{fo["folder"]}'
    d = host / rel
    files = data_files(d)
    if not files:
        continue
    if fo["feed"] == "gsh":
        wanted = {tuple(b["report"]): gas.get(tuple(b["report"])) for b in fo["blocks"]}
        wanted = {k: v for k, v in wanted.items() if v and v not in gas_done}
        for n in reversed(files):
            if not wanted:
                break
            heads = mms_head(d / n, wanted)
            for t, (name, text) in heads.items():
                write(Path("gas", fo["root"], fo["folder"], name), text)
                gas_done[t] = n
                wanted = {k: v for k, v in wanted.items() if v != t}
    else:
        # A plain feed: one file of each name stem (its date tail off).
        stems = {}
        for n in files:
            stem = re.sub(r"(_\d{4,14})+(?=\.|~|$)|~.*$", "", n.rsplit(".", 1)[0]).lower()
            stems[stem] = n
        for stem, n in sorted(stems.items()):
            path = d / n
            # A zip of reports (DWGM's CurrentDay.zip): each CSV in it.
            if n.lower().endswith(".zip") or ".zip@" in n.lower():
                try:
                    z = zipfile.ZipFile(path)
                except (zipfile.BadZipFile, OSError):
                    continue
                members = [m for m in z.namelist() if m.lower().endswith(".csv")]
                texts = []
                for m in members:
                    f = io.TextIOWrapper(z.open(m), "latin-1")
                    texts.append((m.rsplit("/", 1)[-1], [l.rstrip("\r\n") for _, l in zip(range(ROWS + 1), f)]))
            else:
                if not n.lower().endswith(".csv"):
                    continue
                with open(path, encoding="latin-1") as f:
                    texts = [(n, [l.rstrip("\r\n") for _, l in zip(range(ROWS + 1), f)])]
            for name, head in texts:
                key = re.sub(r"(_\d{4,14})+(?=\.|~|$)|~.*$", "", name.rsplit(".", 1)[0]).lower()
                if len(head) < 2 or (rel, key) in gas_done:
                    continue
                write(Path("gas", rel, name), "\r\n".join(head) + "\r\n")
                gas_done[(rel, key)] = n
    print("gas", len(gas_done), fo["folder"], file=sys.stderr, flush=True)

print(json.dumps({"nem": len(done), "nem_tables": len(want_tables),
                  "nem_missing": sorted(want_tables - set(done)), "gas_files": len(gas_done)}, default=str))
