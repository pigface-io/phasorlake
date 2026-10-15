# The catalog

`nemweb-mirror map` writes the catalog of the mirror to
`{home}/mirror/map/map.ndjson`: one JSON object per line for every folder and
file nemweb lists under the roots, every archived file nemweb no longer
lists, and every folder and file inside every zip, zips within zips too.
`tree.txt` beside it is the same nodes as an indented tree.

The first line is a header, `{"catalog_format":1,"roots":[…]}`: the format
version, raised on any change a reader would notice, and the roots crawled.
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
| `path` | always | the archive path (`docs/archive.md`) for what nemweb lists; then `!` and the entry's path inside each zip |
| `url` | what nemweb lists or listed | its exact URL, as crawled or as its sidecar records it |
| `kind` | always | `dir`, `file` or `zip` (a file that is also a folder of what it holds) |
| `size` | files, zips | bytes; uncompressed inside a zip |
| `compressed` | inside a zip | packed bytes |
| `crc32` | inside a zip; downloaded files | CRC-32: as the zip records it, or from the file's sidecar |
| `sha256` | downloaded files | from the file's sidecar |
| `modified` | when known | as nemweb lists it, or as the zip records it (`YYYY-MM-DD HH:MM:SS`) |
| `state` | files nemweb lists or listed | `archived`, `not_archived`, `gone_upstream` (archived, no longer listed) or `removed` (a duplicate, see `docs/archive.md`) |
| `same_as` | removed | the folder holding the same files |
| `contents` | a zip known only from a listing | `needs_download`: its entries need the outer zip |
| `error` | could not be read | `{code, message}`, the code from `docs/errors.md` |

## Paths, URLs and the tree

`/` separates folders and `!` steps into a zip; a `%` or `!` inside a name
is written `%25` or `%21`, so a node's parent is always its path up to the
last `/` or `!`, and the lines rebuild the tree in one pass. Names inside
zips use `/` even where the zip, made on Windows, used `\`.

A path is not a URL (the archive decodes `%23` to `#` and replaces what a
file system cannot hold), so every node nemweb lists carries its `url`; an
entry inside a zip belongs to the node at its path before the first `!`.
Path to URL and URL to path are both exact.

## Zips

An entry of a downloaded zip is a zip when its bytes start as one, whatever
its name; an entry known only from a listing, when its name ends in `.zip`.

Zips are unpacked only as far as listing them needs, and never beyond these
limits, each reported on the node: zips nested more than 8 deep
(`zip.nesting_too_deep`), a zip inside a zip declaring more than 16 GiB
(`zip.too_big`), an entry holding more than it declares
(`zip.size_mismatch`), and more than 10 million entries in one downloaded
file (`zip.too_many_entries`).
