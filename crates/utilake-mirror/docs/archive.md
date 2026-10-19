# The archive

The archive is where every downloaded file is kept, untransformed. nemlake
reads it; nemweb-mirror and nemlake's own `fetch` both write it, so a file
either downloads is never downloaded again by the other.

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
sha256}`; a fragment is not part of the file. `Archive::path_for`
(`crates/nemweb-mirror/src/net/archive.rs`) is the reference.

A file is written to `{path}.part` and renamed into place only after its
size matches the server's `Content-Length`; a `.part` file is never part of
the archive.

## Sidecar

Beside every file, `{path}.json`:

```json
{
  "url": "https://www.nemweb.com.au/…/MMSDM_2009_07.zip",
  "size": 1469212391,
  "sha256": "…",
  "crc32": 1234567890,
  "fetched_at": "2026-10-15T09:12:44Z",
  "etag": "\"…\"",
  "last_modified": "Mon, 12 Oct 2026 00:32:30 GMT",
  "last_modified_at": "2026-10-12T00:32:30Z",
  "listed_size": 1469212391,
  "listed_modified": "Monday, October 12, 2026 10:45 AM",
  "listed_modified_at": "2026-10-12T10:45:00+10:00",
  "tool": "nemweb-mirror/0.1.0"
}
```

`etag` and `last_modified` are present when the server sent them (nemweb
sends `Last-Modified` and, for some files, an ETag). `listed_size` and
`listed_modified` are what nemweb's listing said of the file when it was
downloaded. `crc32`
is written by the download, or by `compare` for a file downloaded before
downloads recorded it; it is what zips record of their entries, so a file
can be matched to a zip entry without reading either again. A file is
archived when it and its sidecar exist, the sidecar's `url` is the file's
URL and its `size` is the file's length. Readers ignore fields they do not
know.

### Times

Every time field is RFC 3339 with an offset, or stands beside one that is.
`fetched_at` is when the download completed, UTC. `last_modified` and
`listed_modified` are kept as the server and the listing wrote them;
`last_modified_at` and `listed_modified_at` beside them are the same times
with their offset: the HTTP date in UTC (`Z`), the listing's time in AEST
(`+10:00`: nemweb lists in Australian Eastern Standard Time, which has no
daylight saving). A sidecar written before the `_at` fields were kept
lacks them; `Archive::read_sidecar` derives them from the text beside each
(`Sidecar::derived`), and the sidecar on disk is left as it is. A time
written without an offset is a defect.

## Removed duplicates

A zip removed by `nemweb-mirror compare --delete` is gone from the archive
or was never downloaded, and is recorded in `{home}/mirror/removed.ndjson`,
one line per file: `url`, `size`, `sha256` (absent when it was never
downloaded), `downloaded`, `same_as` (the folder holding the same files)
and `files` (how many of its files matched), `ancillary` (web pages of the
zip left out of the comparison, when there were any) and `removed_at`
(UTC). Its listing is kept in `{home}/mirror/peek.ndjson` and in the
catalog, and each folder file's CRC-32 in its sidecar, so `nemweb-mirror
compare --verify` can read the folder again and check the removal still
holds. Its files are the folder's files: every one of them, by path, size
and CRC-32, from the zip itself or from its listing, apart from web pages.
A file of the zip named `.htm` or `.html` (`Readme.htm`, `disclaimer.htm`,
`Index.htm`) is not data: one the folder lacks, or holds with other bytes,
is listed as `ancillary` and does not keep the zip. Every data file still
has to match.
