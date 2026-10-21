# nemweb-mirror: index for agents

A copy of every file under nemweb's Data_Archive, Reports/ARCHIVE and Reports/CURRENT folders, and the catalog of it that nemlake reads. One crate, `crates/nemweb-mirror` (a library and the binary), and `crates/xtask` for the gates. Async at the edges (HTTP), sync in the zip reading.
Each entry names where a thing starts. Read those files before you edit.

## Rules a hook or CI enforces

- Run tools through `just`, never with hand-picked flags. `just check` runs every hook, as CI does.
- `unsafe` is forbidden; zips are read through `BufReader<File>` or from memory.
- Every line of `crates/nemweb-mirror/src` is covered (`coverage.json`, 100%): new code comes with its tests, and code no test can reach is removed. The HTTP paths are tested against a local server (`crates/nemweb-mirror/src/net/testserver.rs`), never nemweb.
- A coverage floor in `coverage.json` only goes up; a budget in `bench/budgets.json` only tightens. Loosening one needs a `coverage-floor:` or `bench-budget:` line in the commit message (`just budget-check`).
- Every failure mode is a `Diagnostic` (`crates/nemweb-mirror/src/diag.rs`) with a stable `category.snake_name` code and a full-sentence message. Codes are never renamed; `docs/errors.md` is generated from them (`just errors-doc`).
- Every HTTP request goes through `Fetcher` (`crates/nemweb-mirror/src/net/fetcher.rs`): pacing, retries, the 403/429 breaker. Never download from nemweb in a shell loop.
- The archive layout and sidecar are a contract with nemlake (`docs/archive.md`); the catalog format is versioned (`docs/catalog.md`).
- Every time field is RFC 3339 with an offset (`Stamp`, `crates/nemweb-mirror/src/stamp.rs`); a time kept as text (`modified`, `last_modified`) has its `_at` instant beside it. Listing and zip times are AEST (`+10:00`), HTTP dates and the mirror's clock UTC. A naive time is a defect.
- Nothing is filtered: every folder under a root is followed and every file is recorded. The listing reader stops on any line it cannot read.
- An archived file is removed only by `compare --delete`, only when found the same as its folder, and only after it is recorded among the removed.
- Fixtures under `testdata/` are AEMO pages kept byte for byte (`testdata/LICENCE.md`). Unit tests use a temp home; nothing touches the network.
- Commits follow Conventional Commits with an imperative subject and no tool attribution.

## Features

- **Home and config**: the archive and the mirror's records under the home, the config file in the records (`crates/nemweb-mirror/src/config.rs`).
- **Net**: transports (reqwest, offline, canned for tests), the paced fetcher with ranged requests, the archive with sidecars. `crates/nemweb-mirror/src/net/`.
- **Time**: `Stamp`, the one type every time field is written and read as, and the readers of the listing, DOS and HTTP date forms (`crates/nemweb-mirror/src/stamp.rs`).
- **Zip directory**: a zip's entries from byte spans of it, zip64 too, nothing decompressed (`crates/nemweb-mirror/src/zipdir.rs`).
- **Crawl**: the strict IIS listing reader (`crates/nemweb-mirror/src/listing.rs`) and the breadth-first crawl that keeps every page and writes the crawl record (`crates/nemweb-mirror/src/crawl.rs`).
- **Peek**: every zip not yet downloaded, its directory by ranged requests, the whole-month zips first, into the peek record (`crates/nemweb-mirror/src/peek.rs`); `run` peeks only the zips beside a folder of the same name, and downloads, together on one fetcher.
- **Download**: every crawled file through the fetcher, skipping archived and removed ones; a refusal or a full disk stops the pass (`crates/nemweb-mirror/src/download.rs`); one run at a time (`crates/nemweb-mirror/src/lock.rs`).
- **Map**: the catalog, every folder, file and zip entry, zips within zips (`crates/nemweb-mirror/src/map.rs`); the committed pattern summary (`crates/nemweb-mirror/src/summary.rs`).
- **Compare**: each zip beside a folder of the same name against that folder, from the zip or its peeked listing; web pages in the zip (readme, disclaimer, index) are not data and do not keep it; `--delete` removes the same ones, downloaded or not; `--verify` checks them again (`crates/nemweb-mirror/src/compare.rs`).
- **Watch**: each folder's listing read again on its own schedule, faster while it changes, and what is new or rewritten downloaded (`crates/nemweb-mirror/src/watch.rs`; the loop in `crates/nemweb-mirror/src/commands.rs`).
- **Hash**: every downloaded file's CRC-32 in its sidecar, recorded at download or by `hash`, which reads a file once and checks its sha256 (`crates/nemweb-mirror/src/hash.rs`).
- **Status**: pages read, files archived out of those listed; a run keeps these counts in progress.json for `status` to read while it runs (`crates/nemweb-mirror/src/status.rs`).
- **Command**: `crawl`, `peek`, `download`, `run`, `watch`, `hash`, `map`, `compare`, `summary`, `status`. `crates/nemweb-mirror/src/cli.rs`, `crates/nemweb-mirror/src/commands.rs`.
- **Gates**: coverage (`just cover`), benchmarks (`just bench`), ratchet (`just budget-check`), agent index (`just llm-check`), error catalogue (`just errors-check`). `crates/xtask/src/`, `bench/budgets.json`, `coverage.json`.

## Documents

- `README.md`: what it is, how to run it, where things are on disk.
- `docs/archive.md`: the archive layout and sidecar, as nemlake reads them.
- `docs/catalog.md`: the catalog format.
- `docs/errors.md`: every diagnostic code.
