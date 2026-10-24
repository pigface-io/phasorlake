# The catalog

`utilake-mirror map` writes the catalog of the mirror to
`{home}/state/cache/catalog.ndjson`: one JSON object per line for every folder and
file nemweb lists under the roots, every archived file nemweb no longer
lists, and every folder and file inside every zip, zips within zips too.
`catalog.txt` beside it is the same nodes as an indented tree.

The first line is a header, `{"catalog_format":3,"roots":[…]}`: the format
version, raised on any change a reader would notice, and the roots crawled.
Format 2 adds the `_at` fields (see Times) to format 1 and changes nothing
else. Format 3 adds the `version` state.
The record of the listings the catalog reads is every folder's
`{home}/state/folders/{host}/{folder}/listing.ndjson`: what the crawl read,
each folder a watch has read since as it read it, and nothing under a
folder gone from nemweb since.
Every other line is a node, in tree order: a folder before what it holds,
names in byte order. The same archive and records give the same bytes.

A zip not downloaded (or removed as a duplicate) is still listed with what
it holds, from its central directory read by ranged requests (`peek`): its
folders and files with their sizes and CRC-32. A zip inside it shows as a
`zip` with `contents: "needs_download"`: its own entries are in the outer
zip's compressed bytes.

## Node

| field | when | meaning |
|---|---|---|
| `file` | always | the file or folder nemweb lists, at its path under the archive (`docs/archive.md`); for an entry inside a zip, the zip's. Both listings of a report folder are one folder, so two rows of files can share a `file` only when their bytes are one file; a file kept beside another URL's as a version has its own `{path}@{at}` |
| `url` | all but the folders above the roots | the exact URL of `file`, as crawled or as its record names it |
| `inside` | entries of zips | where inside `file`: one path per zip, outermost first, folders separated by `/` |
| `kind` | always | `dir`, `file` or `zip` (a file that is also a folder of what it holds) |
| `size` | files, zips | bytes; uncompressed inside a zip |
| `compressed` | inside a zip | packed bytes |
| `crc32` | inside a zip; downloaded files | CRC-32: as the zip records it, or from the file's record |
| `sha256` | downloaded files | from the file's record |
| `etag`, `last_modified` | downloaded files; zips peeked | the server's, from the record or the peek; `last_modified` as the server sent it |
| `last_modified_at` | with `last_modified` | the same time, UTC |
| `peeked_at` | zips not downloaded, or removed | when their listing was read, UTC |
| `matched`, `removed_at` | removed zips | how many of its files were found the same as the folder's, and when (UTC) |
| `modified` | when known | as nemweb lists it (`Monday, October 12, 2026 10:45 AM`), as the zip records it (`YYYY-MM-DD HH:MM:SS`), or, for a file nemweb no longer lists, its sidecar's `last_modified` |
| `modified_at` | with `modified` | the same time with its offset: `+10:00` for a listing's or a zip's time, `Z` for a `Last-Modified` |
| `state` | files nemweb lists or listed | `archived`, `not_archived`, `gone_upstream` (archived, no longer listed), `removed` (a duplicate, see `docs/archive.md`) or `version` (a later version of a file nemweb rewrote in place, at `{path}@{at}`, see `docs/archive.md`; its `modified` is the time nemweb listed it with) |
| `same_as` | removed | the folder holding the same files |
| `contents` | a zip known only from a listing | `needs_download`: its entries need the outer zip |
| `error` | could not be read | `{code, message}`, the code from `docs/errors.md` |

## Times

Every time field is RFC 3339 with an offset, or stands beside one that is.
A time taken from outside as text is kept as written (`modified` as nemweb
lists it or as a zip records it, `last_modified` as the server sent it),
and `modified_at` or `last_modified_at` beside it is the same time with
its offset. A listing's time and a zip entry's DOS time are AEST
(`+10:00`): nemweb lists and AEMO packs in Australian Eastern Standard
Time, which has no daylight saving. An HTTP date, `peeked_at` and
`removed_at` are UTC (`Z`). A peek or crawl record written before the
`_at` fields were kept is read with them derived from its text fields, so
a `map` over older records gives the same catalog as one over newer ones.
A `modified_at` is absent only where the text could not be read as a time
(a zip entry whose DOS fields are out of range).

## Provenance and the tree

Every row says where it came from in full: `file` is a real path under the
archive, `url` the exact URL that file was downloaded from (the archive
path is derived from it, but cannot always be turned back into it: `%23`
becomes `#`, and what a file system cannot hold is replaced), and `inside`
the path within each zip, outermost first:

```json
{"file":"www.nemweb.com.au/Data_Archive/Wholesale_Electricity/MMSDM/2009/MMSDM_2009_07.zip",
 "url":"https://www.nemweb.com.au/Data_Archive/Wholesale_Electricity/MMSDM/2009/MMSDM_2009_07.zip",
 "inside":["MMSDM_2009_07/MMSDM_Historical_Data_SQLLoader/DATA/PUBLIC_DVD_REGION_200907010000.zip",
           "PUBLIC_DVD_REGION_200907010000.CSV"],
 "kind":"file","size":1234,"compressed":321,"crc32":1234567890,
 "modified":"2009-08-03 10:00:00","modified_at":"2009-08-03T10:00:00+10:00"}
```

No separator needs escaping and nothing is unsafe in a shell. A row's
parent is the row with the same `file` and `inside` minus the last name of
its last path (or minus that path, when it is a single name); a top-level
row's parent drops the last name of `file`. Rows come parent first, so one
pass rebuilds the tree. Names inside zips use `/` even where the zip, made
on Windows, used `\`.

## Zips

An entry of a downloaded zip is a zip when its bytes start as one, whatever
its name; an entry known only from a listing, when its name ends in `.zip`.

Zips are unpacked only as far as listing them needs, and never beyond these
limits, each reported on the node: zips nested more than 8 deep
(`zip.nesting_too_deep`), a zip inside a zip declaring more than 16 GiB
(`zip.too_big`), an entry holding more than it declares
(`zip.size_mismatch`), and more than 10 million entries in one downloaded
file (`zip.too_many_entries`).
