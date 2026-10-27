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
  with its provenance record, `{name}.provenance.json`: its size, sha256
  and CRC-32, and every address that served those bytes, the one it was
  downloaded from first with its listing line and the server's response.
  A file nemweb rewrites in place is kept in every version with new data:
  a later one beside the first as `{name}@{listed time}`. A download with
  the bytes of a version kept already, or a zip of the same files packed
  again, is not kept; the folder's journal notes it (`docs/archive.md`).
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
  writes what it reads of a folder as that folder's listing, and notes in
  each held file's record when nemweb stops listing its address.
  A refusal
  is waited out as in `run`, and so is an archive linked to a drive not
  mounted.
- **Hash**: every downloaded file has its CRC-32 (what zips record of
  their entries) and sha256 in its record; `hash` records the CRC-32 of
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
  The same goes for each daily zip of a report folder's `ARCHIVE` listing
  (a rollup) against the folder's files of that day held from `CURRENT`.
- **Relayout**: both listings of a report folder are one folder in the
  archive (`docs/archive.md`, Layout); `relayout` moves an archive kept the
  old way, `--dry-run` first. Nothing is written over.
- **Upgrade**: a home kept before `state/` (its records under `mirror/`,
  sidecars at `{name}.json`) is refused until `upgrade` moves it: every
  record to the name below, the crawl record into one listing per folder,
  and every sidecar written again as a provenance record. It reads and
  writes records only, never a downloaded file; `--dry-run` first,
  `--skip-records` to leave the sidecars for later.
- **Reindex**: `state/cache/held.ndjson` written again from the records,
  for a cache deleted or out of step.
- **Keep from**: `keep_from = "2020-01"` in the config, or `--keep-from
  2020-01` on any command, keeps nothing whose data is for a month before
  it: it is not downloaded, peeked, read by a watch or counted by
  `status`. A file's month is read from its name (`MMSDM_2015_01.zip`, the
  first `_YYYYMMDD` stamp of `PUBLIC_…_201612220000_….zip`), else from
  its `/YYYY/` folder; a file with neither is always kept. The listings
  still hold it, so moving the month back downloads it again.
- **Prune**: deletes from the archive what is before `keep_from`, with
  its records, versions and AppleDouble twins: a folder every file of
  which is before goes whole, one removal; a file before in a folder that
  keeps others goes alone. Without `--yes` it only says what would go.
  Each deletion is a line of `state/pruned.ndjson`. Offline.

## Run

```sh
cargo build --release -p utilake-mirror           # target/release/utilake-mirror
export UTILAKE_MIRROR_HOME=/path/to/mirror       # default ~/.utilake-mirror
echo 'request_interval_ms = 750' > $UTILAKE_MIRROR_HOME/config.toml
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
(`{home}/archive/data.wa.aemo.com.au/…`), its folders and the catalog
likewise. A restart takes a file listed in `cache/held.ndjson` as
archived when its size on disk matches, without reading its record.

## On disk

Every file is named for what it holds. The records under `state/` are
the mirror's own; what is under `state/cache/` it derives from them and
writes again, so it may be deleted.

```
{home}/config.toml                         downloads, request_interval_ms, attempts, backoff_ms, read_timeout_secs, progress_every_secs, base_url, user_agent_suffix, jobs; watch_fast, watch_fast_secs, watch_current, watch_current_secs, watch_secs, watch_least_secs; keep_from; [[hosts]] (base_url, listing_offset_secs, request_interval_ms, roots, watch_fast, watch_current)
{home}/archive/{host}/{folder}/{name}      every file, as served; a report folder's listing segment dropped (docs/archive.md)
{home}/archive/…/{name}.provenance.json    its record: content, and every address that served it
{home}/archive/…/{name}@{at}               a later version under the same name, with its own record
{home}/state/lock                          held while a command writes
{home}/state/crawl.json                    written when a crawl has read every page
{home}/state/folders/{host}/{folder}/      one per local folder, both listings of a report folder in one:
    listing.ndjson                         every link its listings show, each with the listing it is on
    pages/{LISTING}.html                   each listing's page as served (CURRENT.html, ARCHIVE.html, index.html)
    events.ndjson                          what no record holds: same, repacked, folder_gone
{home}/state/peeks.ndjson                  zip directories read before download, with when, the listing and the response
{home}/state/removals.ndjson               zips removed as the same as their folders
{home}/state/pruned.ndjson                 folders and files prune deleted, before keep_from
{home}/state/migrations/                   relayout.ndjson, upgrade.ndjson: every move, logged before it was made
{home}/state/cache/held.ndjson             every file a download pass found archived or fetched: its size, listing and version
{home}/state/cache/status.json             a run's counts, which `status` reads while it runs
{home}/state/cache/catalog.ndjson, .txt    the catalog (docs/catalog.md)
{home}/state/tmp/                          scratch
```

`utilake-mirror summary` folds the catalog into patterns as Markdown; its
default output is `nemweb/map.md`, written after `map`, not tracked in git.

Every time field in these records is RFC 3339 with an offset: listing and
zip times in the host's clock (AEST, `+10:00`, for nemweb), the server's
and the mirror's own in UTC. A record of an older mirror that kept a time
as text only has it read from the text.

On a volume without extended attributes (exFAT), macOS keeps a file's
attributes (`com.apple.provenance`, which it gives every file a
downloaded tool writes) in an AppleDouble file beside it, `._{name}`.
They are not downloads: every walk of the archive and of `state/` passes
over them, and deleting them loses nothing the mirror keeps.

## Licence

Apache-2.0; see `LICENSE` and `NOTICE`.
