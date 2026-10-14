# The catalog

`nemweb-mirror map` writes the catalog of the mirror to
`{home}/mirror/map/map.ndjson`: one JSON object per line for every folder and
file nemweb lists under the roots, every archived file nemweb no longer
lists, and every folder and file inside every zip, zips within zips too.
`tree.txt` beside it is the same nodes as an indented tree.

Lines come in tree order: a folder before what it holds, names in byte
order. The same archive gives the same bytes.

## Node

| field | when | meaning |
|---|---|---|
| `path` | always | the archive path (`docs/archive.md`) for what nemweb lists; then `!` and the entry's path inside each zip |
| `kind` | always | `dir`, `file` or `zip` (a file that is also a folder of what it holds) |
| `size` | files, zips | bytes; uncompressed inside a zip |
| `compressed` | inside a zip | packed bytes |
| `crc32` | inside a zip | CRC-32 as the zip records it |
| `modified` | when known | as nemweb lists it, or as the zip records it (`YYYY-MM-DD HH:MM:SS`) |
| `state` | files nemweb lists or listed | `archived`, `not_archived`, `gone_upstream` (archived, no longer listed) or `removed` (a duplicate, see `docs/archive.md`) |
| `same_as` | removed | the folder holding the same files |
| `error` | could not be read | `{code, message}`, the code from `docs/errors.md` |

An entry is a zip when its bytes start as one, whatever its name.
