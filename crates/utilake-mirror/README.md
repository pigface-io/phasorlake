# nemweb-mirror

A copy of every file under nemweb's archive folders, and the catalog of it.

[nemweb](https://www.nemweb.com.au) is where the Australian Energy Market
Operator (AEMO) publishes the National Electricity Market's data. Its
`Data_Archive/` (the monthly MMSDM archive, NEMDE, FCAS causer-pays) and
`Reports/ARCHIVE/` (about a year of daily reports) hold some 110,000 files and
about 4.9 TB. Those two folders are the crawl roots; `Reports/CURRENT/`, which
nemweb rewrites daily, is not mirrored. nemweb-mirror downloads all of it,
once, and writes a catalog of every folder, file and zip entry. [nemlake](https://github.com/g0itb4/nemlake)
reads that catalog and the archive; it knows what the data means, this knows
what exists.

## What it does

- **Crawl**: reads every listing page under the roots and records every
  folder and file. Nothing is filtered, and a listing line it cannot read
  stops it, so no link is passed over quietly.
- **Peek**: what every zip holds before it is downloaded: its central
  directory, read by ranged requests from the end of the file. Nothing is
  decompressed.
- **Download**: every listed file, once, paced (one request per
  `request_interval_ms`), stopping when nemweb refuses (HTTP 403/429) and
  waiting it out. Each file is checked against its `Content-Length` and kept
  with a sidecar recording its URL, size and sha256.
- **Hash**: every downloaded file has its CRC-32 (what zips record of
  their entries) and sha256 in its sidecar; `hash` records the CRC-32 of
  files downloaded before downloads did, checking each sha256 on the way.
- **Map**: the catalog, offline: every zip opened as a folder, zips within
  zips too.
- **Compare**: each month after 2014 is published twice, as a folder and as
  one zip; `compare` checks every file of the zip against the folder, from
  the zip or, before it is downloaded, from its listing, and `--delete`
  removes the zips that hold nothing more, so most are never downloaded
  (`run --delete-duplicates` does it as the download reaches each one);
  `--verify` reads the folders again and checks every removal still holds.

## Run

```sh
just build
export NEMWEB_MIRROR_HOME=/data/nemweb   # default ~/.nemweb-mirror
echo 'request_interval_ms = 750' > $NEMWEB_MIRROR_HOME/mirror/config.toml
nohup caffeinate -ims bin/nemweb-mirror run --json > mirror.log 2>&1 &
bin/nemweb-mirror status
bin/nemweb-mirror compare --delete
bin/nemweb-mirror map && bin/nemweb-mirror summary
```

`run` can be stopped and started again at any time: the crawl carries on from
the pages it kept, and archived files are never downloaded again.

## On disk

```
{home}/archive/{host}/{path}             every file, with {path}.json beside it   (docs/archive.md)
{home}/mirror/config.toml                downloads, request_interval_ms, attempts, backoff_ms, read_timeout_secs, progress_every_secs, base_url, user_agent_suffix, jobs
{home}/mirror/crawl/listings/…           each listing page as served
{home}/mirror/crawl/remote.ndjson        every folder and file the crawl found
{home}/mirror/peek.ndjson                every zip's entries, read before download, with when, against which listing, and the server's ETag and Last-Modified
{home}/mirror/removed.ndjson             zips removed as the same as their folders
{home}/mirror/progress.json              a run's counts, which `status` reads while it runs
{home}/mirror/map/map.ndjson, tree.txt   the catalog                              (docs/catalog.md)
```

`nemweb-mirror summary` folds the catalog into patterns as Markdown; its
default output is `nemweb/map.md`, written after `map`, not tracked in git.

Every time field in these records is RFC 3339 with an offset, or has one
beside it (`modified` as listed, `modified_at` as an instant): listing and
zip times in AEST (`+10:00`), the server's and the mirror's own in UTC.

## Licence

Apache-2.0; see `LICENSE` and `NOTICE`.
