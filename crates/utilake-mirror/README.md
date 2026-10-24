# utilake-mirror

A copy of every file under nemweb's archive folders, and under any other
AEMO host the config names, and the catalog of it.

[nemweb](https://www.nemweb.com.au) is where the Australian Energy Market
Operator (AEMO) publishes the National Electricity Market's data. Its
`Data_Archive/` (the monthly MMSDM archive, NEMDE, FCAS causer-pays) and
`Reports/ARCHIVE/` (about a year of daily reports) hold some 110,000 files and
about 4.9 TB. `Reports/CURRENT/` holds about 120,000 newer report files,
about 50 GB; nemweb rewrites it daily and drops each file after days or weeks,
some of them without archiving them, and only it gives each file's publish
time. Those three folders are the crawl roots. utilake-mirror, the
`utilake-mirror` crate of this repository, downloads all of it, once, and
writes a catalog of every folder, file and zip entry. utilake's ingest and
the census in utilake-schemas read that catalog and the archive; they know
what the data means, this knows what exists.

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
  with a sidecar recording its URL, size and sha256. A file nemweb rewrites
  in place is kept in every version with new data: a later one beside the
  first as `{name}@{listed time}`. A download with the bytes of a version
  kept already, or a zip of the same files packed again, is not kept; each
  folder's journal notes every download, new data or not (`docs/archive.md`).
- **Watch**: after a finished crawl, `watch` reads each folder's listing
  again on a schedule of its own and downloads what is new or rewritten,
  until stopped: the folders whose files nemweb rewrites in place several
  times a day (`watch_fast`: the gas reports under `Reports/CURRENT/`)
  every 15 minutes, the rest of `Reports/CURRENT/` hourly, every other
  folder daily. A folder found changed is read again in half its interval,
  down to 5 minutes; one found the same goes back towards its own. Rounds
  of reads are at least 5 minutes apart (`watch_least_secs`). A
  version nemweb rewrites twice between two reads is not seen, so a
  folder's interval is what decides how many versions are kept. A watch
  writes what it reads of a folder beside the crawl record, not the record
  again, and notes each file nemweb stops listing in the folder's journal.
  A refusal
  is waited out as in `run`, and so is an archive linked to a drive not
  mounted.
- **Hash**: every downloaded file has its CRC-32 (what zips record of
  their entries) and sha256 in its sidecar; `hash` records the CRC-32 of
  files downloaded before downloads did, checking each sha256 on the way.
- **Map**: the catalog, offline: every zip opened as a folder, zips within
  zips too.
- **Compare**: each month after 2014 is published twice, as a folder and as
  one zip; `compare` checks every file of the zip against the folder, from
  the zip or, before it is downloaded, from its listing, and `--delete`
  removes the zips that hold no data the folder lacks (a readme or
  disclaimer page does not count), so most are never downloaded
  (`run --delete-duplicates` does it as the download reaches each one);
  `--verify` reads the folders again and checks every removal still holds.

## Run

```sh
cargo build --release -p utilake-mirror           # target/release/utilake-mirror
export UTILAKE_MIRROR_HOME=/data/nemweb   # default ~/.utilake-mirror
echo 'request_interval_ms = 750' > $UTILAKE_MIRROR_HOME/mirror/config.toml
m=target/release/utilake-mirror
nohup caffeinate -ims $m run --json > mirror.log 2>&1 &
$m status
$m compare --delete
$m map && $m summary
nohup caffeinate -ims $m watch --json --delete-duplicates >> mirror.log 2>&1 &
```

`run` can be stopped and started again at any time: the crawl carries on from
the pages it kept, and archived files are never downloaded again.

## Other hosts

AEMO publishes the Wholesale Electricity Market (WA) on
`data.wa.aemo.com.au`, an IIS site like nemweb whose listed and packed times
are AWST (UTC+8). A `[[hosts]]` table in `config.toml` adds a host with its
own roots, clock and pace; the crawl, download and watch cover every host,
each through its own paced fetcher, so a refusal by one host stops only
that host, and every `_at` instant is read in its host's clock. Without any
`[[hosts]]` an existing config parses as before.

```toml
[[hosts]]
base_url = "https://data.wa.aemo.com.au"
listing_offset_secs = 28800
request_interval_ms = 1000                       # optional; the top-level value when absent
roots = [
  "public/public-data/datafiles/",
  "public/public-data/manifests/",
  "public/market-data/capo/", "public/market-data/dsp/", "public/market-data/mt-pasa/",
  "public/market-data/outages/", "public/market-data/st-pasa/",
  "public/market-data/wemde/caseInputData/previous/", "public/market-data/wemde/caseInputData/schema/",
  "public/market-data/wemde/dispatchSolution/previous/", "public/market-data/wemde/dispatchSolution/schema/",
  "public/market-data/wemde/facilityScada/previous/", "public/market-data/wemde/facilityScada/schema/",
  "public/market-data/wemde/intermittentLoad/previous/", "public/market-data/wemde/intermittentLoad/schema/",
  "public/market-data/wemde/interruptibleLoad/previous/", "public/market-data/wemde/interruptibleLoad/schema/",
  "public/market-data/wemde/loadShedding/previous/", "public/market-data/wemde/loadShedding/schema/",
  "public/market-data/wemde/marketAdvisory/previous/", "public/market-data/wemde/marketAdvisory/schema/",
  "public/market-data/wemde/ncess/previous/", "public/market-data/wemde/ncess/schema/",
  "public/market-data/wemde/notInServiceCapacity/previous/", "public/market-data/wemde/notInServiceCapacity/schema/",
  "public/market-data/wemde/operationalDemandWithdrawal/previous/", "public/market-data/wemde/operationalDemandWithdrawal/schema/",
  "public/market-data/wemde/referenceTradingPrice/previous/", "public/market-data/wemde/referenceTradingPrice/schema/",
  "public/market-data/wemde/tradingReport/previous/", "public/market-data/wemde/tradingReport/schema/",
]
watch_current = ["public/public-data/datafiles/", "public/market-data/wemde/"]
```

WEMDE: each dataset's `previous/` holds one zip per trading day, written
once and kept, and `current/` the day in progress, rewritten through the
day. Only `previous/` (and `schema/`) are roots, so the crawl records
`current/` as a link out and never fetches a file that will be rewritten.
The archive keeps each host under its own name
(`{home}/archive/data.wa.aemo.com.au/…`), the journal and the catalog
likewise. A restart
takes a file listed in `held.ndjson` as archived when its size on disk
matches, without reading its sidecar.

## On disk

```
{home}/archive/{host}/{path}             every file, with {path}.json beside it   (docs/archive.md)
{home}/mirror/config.toml                downloads, request_interval_ms, attempts, backoff_ms, read_timeout_secs, progress_every_secs, base_url, user_agent_suffix, jobs; watch_fast, watch_fast_secs, watch_current, watch_current_secs, watch_secs, watch_least_secs; [[hosts]] (base_url, listing_offset_secs, request_interval_ms, roots, watch_fast, watch_current)
{home}/mirror/crawl/listings/…           each listing page as served
{home}/mirror/crawl/remote.ndjson        every folder and file the crawl found
{home}/mirror/peek.ndjson                zip entries read before download (`peek`: every zip; `run`: the whole-month zips), with when, against which listing, and the server's ETag and Last-Modified
{home}/mirror/removed.ndjson             zips removed as the same as their folders
{home}/mirror/journal/{host}/{path}/changes.ndjson   each folder's events: new, version, same, repacked, dropped, folder_gone
{home}/mirror/crawl/folders/…            what a watch read of each folder since the crawl
{home}/mirror/held.ndjson                every file a download pass found archived or fetched: its size, listed time and version
{home}/mirror/progress.json              a run's counts, which `status` reads while it runs
{home}/mirror/map/map.ndjson, tree.txt   the catalog                              (docs/catalog.md)
```

`utilake-mirror summary` folds the catalog into patterns as Markdown; its
default output is `nemweb/map.md`, written after `map`, not tracked in git.

Every time field in these records is RFC 3339 with an offset, or has one
beside it (`modified` as listed, `modified_at` as an instant): listing and
zip times in AEST (`+10:00`), the server's and the mirror's own in UTC.

## Licence

Apache-2.0; see `LICENSE` and `NOTICE`.
