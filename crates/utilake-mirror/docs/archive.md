# The archive

The archive is where every downloaded file is kept, untransformed. The
lake reads it; utilake-mirror and the lake's own `fetch` both write it,
through the one store of `utilake_provenance::archive` and the one fetcher
core of `utilake-net`. Give the lake and the mirror the same home
(`UTILAKE_HOME` and `UTILAKE_MIRROR_HOME`), and a file either downloads is
never downloaded again by the other: the archive is `{home}/archive/` for
both, beside the lake's `lake/` and the mirror's `state/`.

## Layout

A file's path is its URL's host and path under `{home}/archive/`:

```
https://www.nemweb.com.au/Data_Archive/Wholesale_Electricity/MMSDM/2009/MMSDM_2009_07.zip
{home}/archive/www.nemweb.com.au/Data_Archive/Wholesale_Electricity/MMSDM/2009/MMSDM_2009_07.zip
```

Each path segment is percent-decoded (`%23` is `#`), and the characters no
file system takes (control characters and `/ \ : * ? " < > |`) become `_`;
`.`, `..` and empty segments become `_`. A query string, which no archive
URL has, is folded into the file name as `name@{first 16 hex of its
sha256}`; a fragment is not part of the file.

A report folder is one folder, whichever listing served a file. nemweb
lists a folder's files under `Reports/CURRENT/{folder}/` for a few days,
then the day's zip of them under `Reports/ARCHIVE/{folder}/`; the WA host
lists a WEMDE dataset under `{dataset}/current/` and `{dataset}/previous/`.
The listing segment is dropped from the path, and kept in the record's
addresses:

```
https://www.nemweb.com.au/Reports/CURRENT/Dispatch_SCADA/PUBLIC_DISPATCHSCADA_202610170005_0000000001.zip
https://www.nemweb.com.au/Reports/ARCHIVE/Dispatch_SCADA/PUBLIC_DISPATCHSCADA_20261017.zip
{home}/archive/www.nemweb.com.au/Reports/Dispatch_SCADA/…
```

`utilake_provenance::layout` (in utilake-schemas) holds the listings and
is the reference; `Archive::path_for`
applies it, for the archive and for the mirror's own folders under
`{home}/state/folders/`, where both listings of a report folder share a
`listing.ndjson`, a `pages/` folder (`CURRENT.html`, `ARCHIVE.html`) and
an `events.ndjson`. An archive kept before this layout is moved by
`utilake-mirror relayout` (below).
Files named `._*`, which macOS writes beside files on a disk without
extended attributes, are not part of the archive; every walk skips them.

A file is written to `{path}.part` and renamed into place only after its
size matches the server's `Content-Length`; a `.part` file is never part of
the archive.

## Versions

nemweb rewrites some files in place under the same name: yearly summaries
such as `AS_PAYMENTS_SUMMARY_2026.CSV` and most gas reports under
`Reports/CURRENT/`. The first version downloaded is kept at `{path}`. When
nemweb later lists the file with another size or time, that version is
downloaded beside it as `{path}@{at}`, `at` the listed time in compact RFC
3339 (`AS_PAYMENTS_SUMMARY_2026.CSV@20261021T115300+1000`), with its own
record at `{path}@{at}.provenance.json`. A version is
kept only when its data is new: a download with the bytes of any version
archived already, or a zip that holds the same files as one (names,
CRC-32 and sizes) packed again, is not kept, and its folder's journal
says so. Nothing archived is moved or written over, so a path, once
written, always holds the same bytes.

Two URLs can share a path: the same name under both listings of a report
folder. The same bytes are one file: the second URL joins its record's
addresses (`via = "same"`) and nothing is downloaded twice. Other bytes (a
gas monthly file listed partial under `CURRENT`, final under `ARCHIVE`)
are a version beside the first, `{path}@{at}`, with its own record.
`Archive::lookup` finds a URL's file at the path or among its versions by
the record's addresses. `{home}/state/cache/held.ndjson` names
the version each file was last found or downloaded in. `utilake-mirror
versions` compares each version with the ones before it; `--prune`
removes those kept before downloads compared with every version, noting
each in the journal first.

## Journal

Each local folder has a journal, `{home}/state/folders/{host}/{folder}/events.ndjson`
(both listings of a report folder share it; each line keeps its `url`),
one JSON line per event, appended and never written over. It holds only
what no record does:

| `event` | when |
|---|---|
| `same` | listed again with the bytes of `file`, a version archived already |
| `repacked` | a zip listed again and packed again, the files of `file`; its other bytes (`sha256`) not kept |
| `folder_gone` | the folder no longer served (HTTP 404) |

Fields: `at` (UTC), `url`, `event`, and when known `file` (the archived
file holding the bytes or the files) and `sha256`. A download is in its
file's record (`seen`), and so is a listing that stops naming it (`gone`).
Lines an older mirror wrote (`seen_at`, and the events `new`, `version`
and `dropped`, with sizes and times) still read.

## Provenance record

Beside every file, `{path}.provenance.json`
([`utilake_provenance::provenance`] in utilake-schemas):

```json
{
  "size": 1469212391,
  "sha256": "…",
  "crc32": 1234567890,
  "tool": "utilake-mirror/0.1.0",
  "addresses": [
    {"url": "https://www.nemweb.com.au/Reports/CURRENT/…/PUBLIC_…_0000000001.zip",
     "seen": "2026-10-15T09:12:44Z", "gone": "2026-10-19T02:00:00Z",
     "via": "fetched", "listed": {}, "served": {"etag": "\"…\"", "modified": "2026-10-15T09:06:31Z"}},
    {"url": "https://www.nemweb.com.au/Reports/ARCHIVE/…/PUBLIC_…_20261015.zip!PUBLIC_…_0000000001.zip",
     "seen": "2026-10-16T03:00:00Z", "via": "entry"}
  ]
}
```

`size`, `sha256` and `crc32` are the bytes: they never change, so the data
file is never read again to know them. `addresses` is every place nemweb
served those bytes, the first where they were obtained:

| `via` | the address | keeps |
|---|---|---|
| `fetched` | downloaded whole from `url` | `listed` (its listing line), `served` (the response's `etag`, `modified`) |
| `range` | one zip entry fetched alone by byte span (the lake's `fetch`) | `span` (zip, entry, zip size, offset, compressed size, method), `listed`, `served` |
| `same` | `url` served identical bytes (sha256 equal) | `listed` |
| `entry` | `{zip URL}!{entry}`, an entry of a daily zip with the file's name, size and CRC-32: the address that serves the file once `CURRENT` stops listing it | |

`seen` is when the bytes were fetched or found there; `gone`, when a watch
saw the listing stop naming the address. A listing's `size` is left out
when it is the file's, and its `at` when it is the response's `modified`
cut to the minute in the host's listing clock (IIS lists a file's last
write time); `Provenance::listed` gives both back exactly. Only the record
grows, rewritten whole (a temporary file, handed to the disk, renamed).
A file is archived when it and its record exist, the record names its URL
and its `size` is the file's length; it is the version nemweb lists when
the listing's size and time are the record's.

### Times

Every time is RFC 3339 with an offset: `seen` and `gone` in UTC, the
response's `modified` in UTC, a listing's `at` in the host's clock (AEST,
`+10:00`, for nemweb, which has no daylight saving). A time written
without an offset is a defect.

### Format 1

Records written before (`{path}.json`, the flat sidecar: `url`,
`fetched_at`, `last_modified` with `last_modified_at`, `listed_size`,
`listed_modified` with `listed_modified_at`, `also`, `gone_at`,
`extracted_from`) are read as format 2, a text time without its `_at` read
from the text in the host's clock. A record is written again only in
format 2, and the format-1 file goes; `utilake-mirror upgrade` writes
every one again.

## Removed duplicates

A **rollup** is a daily zip of an `ARCHIVE` listing,
`{prefix}{YYYYMMDD}.zip`, whose entries are the folder's files of that day
held from `CURRENT`. `compare` pairs it with those files from the archive
on disk (not the crawl, since `CURRENT` lists a file for about four days)
and rules on it as on a whole-month zip. One whose every entry is held is
removed as below, with `same_as` the folder's `CURRENT` URL, and each held
file gains the entry's address in its record; one with any entry
not held is downloaded and kept whole, its held files still gaining the
address (`compare --delete`).

A zip removed by `utilake-mirror compare --delete` is gone from the archive
or was never downloaded, and is recorded in `{home}/state/removals.ndjson`,
one line per file: `url`, `size`, `sha256` (absent when it was never
downloaded), `downloaded`, `same_as` (the folder holding the same files)
and `files` (how many of its files matched), `ancillary` (web pages of the
zip left out of the comparison, when there were any) and `removed_at`
(UTC). Its listing is kept in `{home}/state/peeks.ndjson` and in the
catalog, and each folder file's CRC-32 in its record, so `utilake-mirror
compare --verify` can read the folder again and check the removal still
holds. Its files are the folder's files: every one of them, by path, size
and CRC-32, from the zip itself or from its listing, apart from web pages.
A file of the zip named `.htm` or `.html` (`Readme.htm`, `disclaimer.htm`,
`Index.htm`) is not data: one the folder lacks, or holds with other bytes,
is listed as `ancillary` and does not keep the zip. Every data file still
has to match.

## Relayout

`utilake-mirror relayout [--dry-run]` moves an archive kept with each URL
as its path to the layout above, with the mirror's run lock held. Every
file with a record in a folder under a listing moves to the path of its
record's first address: a rename on one volume, nothing copied, its
record written again there. A file there already with the same bytes
keeps its place and the moving file's addresses join its record; the
moving file is
dropped. A file there with other bytes is never written over: the moving
file becomes `{name}@{at}` beside it (`at` the listed time, else when it
was downloaded). The journals of a folder's two listings merge into one,
lines in time order; `held.ndjson` lines whose file became a version are
rewritten; emptied folders are removed. Each step is logged to
`{home}/state/migrations/relayout.ndjson` before it is taken, and a run cut
off is finished by the next; a second run moves nothing. A file whose
record does not read is left where it is and named, and the run exits 1.
