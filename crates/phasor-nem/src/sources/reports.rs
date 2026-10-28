//! nemweb's report folders as a source: `Reports/ARCHIVE/{folder}/` holds one zip
//! per day, `Reports/CURRENT/{folder}/` the loose files of the last days. A day is
//! its archive zip, read for the request's instants, else its `CURRENT` files.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use phasor_acquire::listing;
use phasor_acquire::{Archive, FetchError};
use phasor_core::{CivilDay, Period};
use phasor_kit::{Entry, Listings, LocalFile, Pick, Picks, Query, RemoteFile, Source};
use phasor_registry::{Registry, Side};
use url::Url;

/// The source id, as the manifest records it.
pub const REPORTS_SOURCE_ID: &str = "nem-reports";

/// Market time of the NEM, seconds east of UTC.
const AEST: i32 = 36_000;

/// Five minutes in microseconds: a file stamped `T` holds the interval ending at `T`.
const INTERVAL_MICROS: i64 = 5 * 60 * 1_000_000;
/// A day, in microseconds.
const DAY_MICROS: i64 = 86_400 * 1_000_000;

/// What a report file's name says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportName {
    /// The report, as the name writes it: `DISPATCHSCADA`, `NEXT_DAY_DISPATCH`.
    pub report: String,
    /// The day of the stamp.
    pub day: CivilDay,
    /// The stamp's `(hour, minute, second)`; a daily `ARCHIVE` zip carries none.
    pub time: Option<(u8, u8, u8)>,
}

impl ReportName {
    /// The stamp as microseconds since the epoch, in market time.
    pub fn micros(&self) -> i64 {
        let (h, m, s) = self.time.unwrap_or((0, 0, 0));
        self.day.start_micros(AEST)
            + (i64::from(h) * 3600 + i64::from(m) * 60 + i64::from(s)) * 1_000_000
    }
}

/// Whether a file stamped `name` serves `instants`: stamped within them, or the
/// first at or after their end. A daily zip serves any request for its day.
fn within(name: &ReportName, instants: Option<(i64, i64)>) -> bool {
    match (name.time, instants) {
        (Some(_), Some((from, to))) => {
            let at = name.micros();
            at >= from && at <= to + INTERVAL_MICROS
        }
        _ => true,
    }
}

/// Whether `instants` take every file of the daily zip `day` (00:05 to the next 00:00).
fn whole_day(day: &ReportName, instants: Option<(i64, i64)>) -> bool {
    instants.is_none_or(|(from, to)| {
        let start = day.micros();
        from <= start + INTERVAL_MICROS && to + INTERVAL_MICROS >= start + DAY_MICROS
    })
}

/// The day whose intervals a file carries: one stamped 00:00 carries the day before's last.
fn day_of(name: &ReportName) -> CivilDay {
    if name.time == Some((0, 0, 0)) {
        CivilDay::from_days(name.day.days_since_epoch() - 1)
    } else {
        name.day
    }
}

/// Parses `PUBLIC_{REPORT}_{YYYYMMDD}.zip` or `PUBLIC_{REPORT}_{YYYYMMDDHHMM[SS]}_{id}.{zip,CSV}`,
/// from the end of a path or a `!`-joined entry path.
pub fn parse_report_name(name: &str) -> Option<ReportName> {
    let name = name.rsplit(['/', '!']).next()?;
    let stem = match name.rsplit_once('.') {
        Some((stem, ext)) if ext.eq_ignore_ascii_case("zip") || ext.eq_ignore_ascii_case("csv") => {
            stem
        }
        _ => return None,
    };
    let parts: Vec<&str> = stem.split('_').collect();
    if parts.len() < 3 || parts[0] != "PUBLIC" {
        return None;
    }
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    // The stamp is the last part, or the one before a numeric id.
    let last = parts.len() - 1;
    let stamp_at = if digits(parts[last]) && last >= 3 && digits(parts[last - 1]) {
        last - 1
    } else if digits(parts[last]) {
        last
    } else {
        return None;
    };
    let stamp = parts[stamp_at];
    if stamp_at < 2 || !matches!(stamp.len(), 8 | 12 | 14) {
        return None;
    }
    let num = |a: usize, b: usize| stamp[a..b].parse::<u32>().ok();
    let day = CivilDay::new(num(0, 4)? as i32, num(4, 6)? as u8, num(6, 8)? as u8)?;
    let time = match stamp.len() {
        8 => None,
        12 => Some((num(8, 10)? as u8, num(10, 12)? as u8, 0)),
        _ => Some((num(8, 10)? as u8, num(10, 12)? as u8, num(12, 14)? as u8)),
    };
    if let Some((h, m, s)) = time
        && (h > 23 || m > 59 || s > 59)
    {
        return None;
    }
    Some(ReportName {
        report: parts[1..stamp_at].join("_"),
        day,
        time,
    })
}

/// The report folders of nemweb.
pub struct ReportFolders {
    base: Url,
    registry: Arc<Registry>,
}

impl ReportFolders {
    /// The folders under `base_url`, mapped to tables by `registry`.
    pub fn new(base_url: &str, registry: Arc<Registry>) -> Result<Self, FetchError> {
        let mut base = Url::parse(base_url).map_err(FetchError::parse(base_url, "base URL"))?;
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        Ok(Self { base, registry })
    }

    /// The URL of a folder's side, `{base}Reports/ARCHIVE/Dispatch_SCADA/`.
    pub fn folder_url(&self, side: &Side) -> Result<Url, FetchError> {
        self.base
            .join(&format!("{}/", side.path))
            .map_err(FetchError::parse(&side.path, "folder URL"))
    }

    /// The folders that carry the query's tables, each with its `ARCHIVE` and `CURRENT` sides.
    fn folders_for(&self, query: &Query) -> Result<BTreeMap<String, Vec<Side>>, FetchError> {
        let no_match = |table: &str, nearest: &str| FetchError::NoMatch {
            url: self.base.to_string(),
            entries: self.registry.folders().len(),
            table: table.to_owned(),
            period: query.range.to_string(),
            nearest: nearest.to_owned(),
        };
        if query.tables.is_empty() {
            return Err(no_match(
                "*",
                "the report folders are read per table; name one",
            ));
        }
        let mut out: BTreeMap<String, Vec<Side>> = BTreeMap::new();
        for table in &query.tables {
            if query.exclude.iter().any(|x| x.eq_ignore_ascii_case(table)) {
                continue;
            }
            let id = self
                .registry
                .table_by_name(&table.to_ascii_uppercase())
                .ok_or_else(|| no_match(table, "no table of that name"))?;
            let folders = self.registry.folders_of(id);
            if folders.is_empty() {
                return Err(no_match(
                    table,
                    "no report folder carries it; the monthly archive does",
                ));
            }
            for f in folders {
                let sides = out.entry(f.path()).or_default();
                for s in f.sides() {
                    if !sides.contains(&s) {
                        sides.push(s);
                    }
                }
            }
        }
        Ok(out)
    }

    /// One folder's files for the query's days: a day's `ARCHIVE` zip, else its `CURRENT` files.
    async fn folder_files(
        &self,
        sides: &[Side],
        query: &Query,
        fetcher: &dyn Listings,
    ) -> Result<Vec<RemoteFile>, FetchError> {
        let days: Vec<CivilDay> = query.range.days_iter().collect();
        let mut out = Vec::new();
        let mut covered: Vec<CivilDay> = Vec::new();
        let listed = |side: &Side,
                      out: &mut Vec<RemoteFile>,
                      covered: &mut Vec<CivilDay>,
                      body: &[u8],
                      url: &Url|
         -> Result<(), FetchError> {
            let entries = listing::parse(body);
            if entries.is_empty() {
                return Err(FetchError::ListingParse {
                    url: url.to_string(),
                    sample: body[..body.len().min(256)].to_vec(),
                });
            }
            for e in entries {
                if e.is_dir {
                    continue;
                }
                let Some(name) = parse_report_name(&e.name) else {
                    continue;
                };
                let archived_day =
                    side.label.eq_ignore_ascii_case("ARCHIVE") && name.time.is_none();
                let day = day_of(&name);
                if !days.contains(&day)
                    || (!archived_day && covered.contains(&day))
                    || !within(&name, query.instants)
                {
                    continue;
                }
                if archived_day {
                    covered.push(name.day);
                }
                let file_url = url
                    .join(&e.href)
                    .map_err(FetchError::parse(&e.href, "listing href"))?;
                out.push(RemoteFile {
                    url: file_url,
                    source: REPORTS_SOURCE_ID,
                    table: None,
                    period: Some(Period::Day(day_of(&name))),
                    size: e.size,
                    modified: e.modified.map(|m| m.text().to_owned()),
                    modified_at: e.modified.and_then(|m| m.stamp()),
                });
            }
            Ok(())
        };
        // ARCHIVE first, so its days are known before CURRENT is read.
        let mut ordered: Vec<&Side> = sides.iter().collect();
        ordered.sort_by_key(|f| !f.label.eq_ignore_ascii_case("ARCHIVE"));
        for side in ordered {
            let url = self.folder_url(side)?;
            let body = match fetcher.fetch_listing(url.as_str()).await {
                Ok(b) => b,
                // A side the census saw but the server no longer lists.
                Err(FetchError::HttpStatus { status: 404, .. }) => continue,
                Err(e) => return Err(e),
            };
            listed(side, &mut out, &mut covered, &body, &url)?;
        }
        Ok(out)
    }
}

/// Whether `url` is under a report folder, and which side.
fn report_side(url: &Url) -> Option<&'static str> {
    let path = url.path();
    if path.contains("/Reports/ARCHIVE/") {
        Some("ARCHIVE")
    } else if path.contains("/Reports/CURRENT/") {
        Some("CURRENT")
    } else {
        None
    }
}

#[async_trait]
impl Source for ReportFolders {
    fn id(&self) -> &'static str {
        REPORTS_SOURCE_ID
    }

    fn granularity(&self) -> phasor_kit::Granularity {
        phasor_kit::Granularity::Day
    }

    async fn list(
        &self,
        query: &Query,
        fetcher: &dyn Listings,
    ) -> Result<Vec<RemoteFile>, FetchError> {
        let mut out = Vec::new();
        for sides in self.folders_for(query)?.values() {
            out.extend(self.folder_files(sides, query, fetcher).await?);
        }
        out.sort_by(|a, b| (a.period, a.url.as_str()).cmp(&(b.period, b.url.as_str())));
        out.dedup_by(|a, b| a.url == b.url);
        Ok(out)
    }

    /// The archived files of the query's folders and days. A day whose `ARCHIVE` zip
    /// is held is read from it, not again from its `CURRENT` files.
    fn archived(&self, archive: &Archive, query: &Query) -> Result<Vec<LocalFile>, FetchError> {
        let days: Vec<CivilDay> = query.range.days_iter().collect();
        let mut out = Vec::new();
        for sides in self.folders_for(query)?.values() {
            let mut dirs: Vec<std::path::PathBuf> = Vec::new();
            for side in sides {
                let dir = archive.path_for(self.folder_url(side)?.as_str());
                if !dirs.contains(&dir) {
                    dirs.push(dir);
                }
            }
            let held: Vec<_> = dirs
                .iter()
                .flat_map(|d| phasor_acquire::archive::entries_under(archive, Some(d)))
                .filter_map(|a| Some((parse_report_name(a.provenance.url())?, a)))
                .filter(|(name, _)| days.contains(&day_of(name)) && within(name, query.instants))
                .collect();
            let whole_days: Vec<CivilDay> = held
                .iter()
                .filter(|(name, a)| {
                    name.time.is_none() && a.provenance.url().contains("/Reports/ARCHIVE/")
                })
                .map(|(name, _)| name.day)
                .collect();
            for (name, a) in held {
                {
                    if name.time.is_some() && whole_days.contains(&day_of(&name)) {
                        continue;
                    }
                    let p = &a.provenance;
                    let Ok(url) = Url::parse(p.url()) else {
                        continue;
                    };
                    let remote = RemoteFile {
                        url,
                        source: REPORTS_SOURCE_ID,
                        table: None,
                        period: Some(Period::Day(day_of(&name))),
                        size: Some(p.content.size),
                        modified: None,
                        modified_at: p.listed(p.url()).and_then(|l| l.at),
                    };
                    out.push(LocalFile::from_held(&remote, a, true));
                }
            }
        }
        out.sort_by(|a, b| {
            (a.remote.period, a.remote.url.as_str()).cmp(&(b.remote.period, b.remote.url.as_str()))
        });
        Ok(out)
    }

    fn describe(&self, url: &Url) -> Option<RemoteFile> {
        report_side(url)?;
        let name = parse_report_name(url.path())?;
        Some(RemoteFile {
            url: url.clone(),
            source: REPORTS_SOURCE_ID,
            table: None,
            period: Some(Period::Day(day_of(&name))),
            size: None,
            modified: None,
            modified_at: None,
        })
    }

    /// A day's `ARCHIVE` zip is read by its directory when the request wants less than the day.
    fn wants_ranged(&self, remote: &RemoteFile, query: &Query) -> bool {
        report_side(&remote.url) == Some("ARCHIVE")
            && parse_report_name(remote.url.path())
                .is_some_and(|n| n.time.is_none() && !whole_day(&n, query.instants))
    }

    /// The entries within the request's instants plus the first at or after their end;
    /// the whole zip when there are no instants.
    fn pick(&self, remote: &RemoteFile, entries: &[Entry], query: &Query) -> Picks {
        let Some((from, to)) = query.instants else {
            return Picks::Whole;
        };
        let picks = entries
            .iter()
            .enumerate()
            .filter(|(_, e)| !e.is_dir())
            .filter_map(|(index, e)| {
                let name = parse_report_name(&e.name)?;
                within(&name, Some((from, to))).then(|| {
                    let url = Url::parse(&format!("{}!{}", remote.url, e.name)).ok()?;
                    Some(Pick {
                        index,
                        remote: RemoteFile {
                            url,
                            source: REPORTS_SOURCE_ID,
                            table: None,
                            period: Some(Period::Day(day_of(&name))),
                            size: Some(e.size),
                            modified: remote.modified.clone(),
                            modified_at: remote.modified_at,
                        },
                    })
                })?
            })
            .collect();
        Picks::Entries(picks)
    }
}

#[cfg(test)]
mod tests {
    use phasor_acquire::Fetcher;
    use std::fmt::Write as _;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    use phasor_acquire::transport::{Canned, StaticTransport};
    use phasor_acquire::{Archive, FetcherConfig};
    use phasor_core::{TimeRange, YearMonth};

    use super::*;

    fn day(s: &str) -> CivilDay {
        s.parse().unwrap()
    }

    #[test]
    fn report_names_say_their_report_day_and_time() {
        let daily = parse_report_name("PUBLIC_DISPATCHSCADA_20261019.zip").unwrap();
        assert_eq!(daily.report, "DISPATCHSCADA");
        assert_eq!((daily.day, daily.time), (day("2026-10-19"), None));
        let five = parse_report_name(
            "Reports/ARCHIVE/Dispatch_SCADA/PUBLIC_DISPATCHSCADA_20261019.zip!PUBLIC_DISPATCHSCADA_202610190005_0000000543611267.zip",
        )
        .unwrap();
        assert_eq!(five.time, Some((0, 5, 0)));
        assert_eq!(five.micros() - daily.micros(), 5 * 60 * 1_000_000);
        // Midnight in +10:00.
        assert_eq!(daily.micros(), day("2026-10-19").start_micros(36_000));
        // A file stamped at midnight carries the day before's last interval.
        let midnight = parse_report_name("PUBLIC_DISPATCHSCADA_202610200000_1.zip").unwrap();
        assert_eq!(day_of(&midnight), day("2026-10-19"));
        assert_eq!(day_of(&daily), day("2026-10-19"));
        let words = parse_report_name("PUBLIC_NEXT_DAY_DISPATCH_20261019.zip").unwrap();
        assert_eq!(words.report, "NEXT_DAY_DISPATCH");
        let secs = parse_report_name("PUBLIC_P5MIN_20261019000512_0000000543611267.CSV").unwrap();
        assert_eq!(secs.time, Some((0, 5, 12)));
        for bad in [
            "PUBLIC_DISPATCHSCADA_20261019.txt",
            "PRIVATE_X_20261019.zip",
            "PUBLIC_20261019.zip",
            "PUBLIC_X_2026101.zip",
            "PUBLIC_X_20261319.zip",
            "PUBLIC_X_202610192500_1.zip",
            "PUBLIC_X_abc.zip",
            "notes.zip",
        ] {
            assert!(parse_report_name(bad).is_none(), "{bad}");
        }
    }

    /// A registry of two real tables with a folders.json placing them.
    fn registry() -> (tempfile::TempDir, Arc<Registry>) {
        let dir = tempfile::tempdir().unwrap();
        let real = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas/nem");
        let nem = dir.path().join("nem");
        std::fs::create_dir_all(nem.join("pulled")).unwrap();
        std::fs::copy(real.join("market.toml"), nem.join("market.toml")).unwrap();
        for t in ["DISPATCH_UNIT_SCADA", "TRADINGPRICE", "BILLINGDAYTRK"] {
            let to = nem.join("pulled").join(t).join("metadata");
            std::fs::create_dir_all(&to).unwrap();
            std::fs::copy(
                real.join("pulled")
                    .join(t)
                    .join("metadata/v1.metadata.json"),
                to.join("v1.metadata.json"),
            )
            .unwrap();
        }
        std::fs::write(
            nem.join("pulled/folders.json"),
            r#"{"x-format": 3, "folders": [
              {"feed": "reports", "root": "Reports", "folder": "Dispatch_SCADA", "listings": ["ARCHIVE", "CURRENT"],
               "blocks": [{"report": ["DISPATCH", "UNIT_SCADA"], "version": 1, "first-release": "2024-01", "last-release": "2026-10", "months": 34}]},
              {"feed": "reports", "root": "Reports", "folder": "TradingIS_Reports", "listings": ["ARCHIVE"],
               "blocks": [{"report": ["TRADING", "PRICE"], "version": 3, "first-release": "2024-01", "last-release": "2026-10", "months": 34}]}
            ]}"#,
        )
        .unwrap();
        let r = Registry::load_dir(dir.path()).unwrap();
        (dir, Arc::new(r))
    }

    fn listing(dir: &str, names: &[&str]) -> Vec<u8> {
        let mut html = format!(
            "<html><body><H1>nemweb.com.au - {dir}</H1><hr><pre><A HREF=\"/Reports/\">[To Parent Directory]</A><br><br>"
        );
        // A sub-folder, as IIS lists one: passed over.
        let _ = write!(
            html,
            "   Sunday, October 19, 2026  4:05 AM        &lt;dir&gt; <A HREF=\"{dir}DUPLICATE/\">DUPLICATE</A><br>"
        );
        for n in names {
            let _ = write!(
                html,
                "   Sunday, October 19, 2026  4:05 AM         1194837 <A HREF=\"{dir}{n}\">{n}</A><br>"
            );
        }
        html.push_str("</pre><hr></body></html>");
        html.into_bytes()
    }

    fn query(tables: &[&str], from: &str, to: &str, instants: Option<(i64, i64)>) -> Query {
        Query {
            tables: tables.iter().map(|t| (*t).to_owned()).collect(),
            exclude: Vec::new(),
            range: TimeRange::days(day(from), day(to)),
            instants,
        }
    }

    fn fetcher(t: &Arc<StaticTransport>, dir: &Path) -> Fetcher {
        Fetcher::new(
            Arc::new(t.clone()),
            Archive::new(dir),
            FetcherConfig {
                attempts: 1,
                backoff: Duration::ZERO,
                max_backoff: Duration::ZERO,
                min_interval: Duration::ZERO,
            },
        )
    }

    #[tokio::test]
    async fn a_table_is_listed_from_its_folders_a_day_from_archive_else_current() {
        let (_dir, registry) = registry();
        let src = ReportFolders::new("https://h", registry.clone()).unwrap();
        assert_eq!(src.id(), REPORTS_SOURCE_ID);
        assert_eq!(src.granularity(), phasor_kit::Granularity::Day);
        let t = Arc::new(StaticTransport::default());
        t.add(
            "https://h/Reports/ARCHIVE/Dispatch_SCADA/",
            Canned::ok(&listing(
                "/Reports/ARCHIVE/Dispatch_SCADA/",
                &[
                    "PUBLIC_DISPATCHSCADA_20261017.zip",
                    "PUBLIC_DISPATCHSCADA_20261018.zip",
                    "README.txt",
                ],
            )),
        );
        t.add(
            "https://h/Reports/CURRENT/Dispatch_SCADA/",
            Canned::ok(&listing(
                "/Reports/CURRENT/Dispatch_SCADA/",
                &[
                    "PUBLIC_DISPATCHSCADA_202610182355_0000000543600001.zip",
                    "PUBLIC_DISPATCHSCADA_202610190000_0000000543611000.zip",
                    "PUBLIC_DISPATCHSCADA_202610190005_0000000543611267.zip",
                    "PUBLIC_DISPATCHSCADA_202610190010_0000000543611895.zip",
                    "PUBLIC_DISPATCHSCADA_202610200000_0000000543699999.zip",
                    "PUBLIC_DISPATCHSCADA_202610200005_0000000543700000.zip",
                ],
            )),
        );
        let tmp = tempfile::tempdir().unwrap();
        let f = fetcher(&t, tmp.path());
        let q = query(&["DISPATCH_UNIT_SCADA"], "2026-10-18", "2026-10-19", None);
        let files = src.list(&q, &f).await.unwrap();
        let names: Vec<String> = files.iter().map(Named::remote_name).collect();
        assert_eq!(
            names,
            [
                "PUBLIC_DISPATCHSCADA_20261018.zip",
                "PUBLIC_DISPATCHSCADA_202610190005_0000000543611267.zip",
                "PUBLIC_DISPATCHSCADA_202610190010_0000000543611895.zip",
                "PUBLIC_DISPATCHSCADA_202610200000_0000000543699999.zip",
            ],
            "the 18th from ARCHIVE (its zip holds the 19th's midnight file too), the \
             19th from CURRENT with the file at the 20th's midnight, which carries the \
             19th's last interval; nothing else"
        );
        assert_eq!(files[0].period, Some(Period::Day(day("2026-10-18"))));
        assert_eq!(files[0].source, REPORTS_SOURCE_ID);
        assert!(files[0].table.is_none());
        assert_eq!(files[0].size, Some(1_194_837));
        assert!(files[0].modified_at.is_some());
        assert_eq!(t.request_count(), 2);
        // Finer than a day: CURRENT's files stamped within the instants
        // (and the one just after, for the last interval), not the day's
        // every file. 00:00 to 00:05 on the 19th: the files at 00:05 and
        // 00:10; the 18th's daily zip still serves, read by its directory.
        let start = day("2026-10-19").start_micros(36_000);
        let five = 5 * 60 * 1_000_000;
        let fine = query(
            &["DISPATCH_UNIT_SCADA"],
            "2026-10-18",
            "2026-10-19",
            Some((start, start + five)),
        );
        let names: Vec<String> = src
            .list(&fine, &f)
            .await
            .unwrap()
            .iter()
            .map(Named::remote_name)
            .collect();
        assert_eq!(
            names,
            [
                "PUBLIC_DISPATCHSCADA_20261018.zip",
                "PUBLIC_DISPATCHSCADA_202610190005_0000000543611267.zip",
                "PUBLIC_DISPATCHSCADA_202610190010_0000000543611895.zip",
            ]
        );
        let late = query(
            &["DISPATCH_UNIT_SCADA"],
            "2026-10-19",
            "2026-10-19",
            Some((start + 3 * five, start + 4 * five)),
        );
        let names: Vec<String> = src
            .list(&late, &f)
            .await
            .unwrap()
            .iter()
            .map(Named::remote_name)
            .collect();
        assert!(
            names.is_empty(),
            "nothing stamped 00:15 to 00:25: {names:?}"
        );
        // Only the daily zip wants reading by its directory, and only
        // for less than its whole day: a day's zip comes whole for the day.
        assert!(src.wants_ranged(&files[0], &fine));
        assert!(!src.wants_ranged(&files[1], &fine));
        let day18 = parse_report_name("PUBLIC_DISPATCHSCADA_20261018.zip")
            .unwrap()
            .micros();
        let instants = |from: i64, to: i64| {
            query(
                &["DISPATCH_UNIT_SCADA"],
                "2026-10-18",
                "2026-10-18",
                Some((from, to)),
            )
        };
        let whole = 86_400 * 1_000_000;
        assert!(!src.wants_ranged(&files[0], &instants(day18, day18 + whole)));
        assert!(!src.wants_ranged(&files[0], &instants(day18 - five, day18 + whole + five)));
        assert!(!src.wants_ranged(
            &files[0],
            &query(&["DISPATCH_UNIT_SCADA"], "2026-10-18", "2026-10-18", None)
        ));
        assert!(src.wants_ranged(&files[0], &instants(day18 + 2 * five, day18 + whole)));
        assert!(src.wants_ranged(&files[0], &instants(day18, day18 + whole - 2 * five)));
        // Known by their path alone.
        let d = src.describe(&files[1].url).unwrap();
        assert_eq!(d.period, files[1].period);
        assert!(
            src.describe(
                &Url::parse("https://h/Data_Archive/x/PUBLIC_DVD_REGION_202401010000.zip").unwrap()
            )
            .is_none()
        );
        assert!(
            src.describe(
                &Url::parse("https://h/Reports/CURRENT/Dispatch_SCADA/notes.txt").unwrap()
            )
            .is_none()
        );
        // A table whose folder the server no longer lists: nothing, no error.
        let q2 = query(&["TRADINGPRICE"], "2026-10-18", "2026-10-18", None);
        assert!(src.list(&q2, &f).await.unwrap().is_empty());
        assert_eq!(
            t.request_count(),
            7,
            "two listings for each of the three queries above, one for this"
        );
        // No table, an unknown table, a table of the monthly archive only,
        // and an excluded one.
        let e = src
            .list(&query(&[], "2026-10-18", "2026-10-18", None), &f)
            .await
            .unwrap_err();
        assert!(e.to_string().contains("name one"), "{e}");
        let e = src
            .list(&query(&["NOPE"], "2026-10-18", "2026-10-18", None), &f)
            .await
            .unwrap_err();
        assert!(
            matches!(&e, FetchError::NoMatch { table, .. } if table == "NOPE"),
            "{e}"
        );
        let e = src
            .list(
                &query(&["BILLINGDAYTRK"], "2026-10-18", "2026-10-18", None),
                &f,
            )
            .await
            .unwrap_err();
        assert!(e.to_string().contains("monthly archive"), "{e}");
        let mut q3 = query(&["DISPATCH_UNIT_SCADA"], "2026-10-18", "2026-10-18", None);
        q3.exclude = vec!["dispatch_unit_scada".into()];
        assert!(src.list(&q3, &f).await.unwrap().is_empty());
        // A listing that is not one.
        t.add(
            "https://h/Reports/ARCHIVE/TradingIS_Reports/",
            Canned::ok(b"<html>nothing</html>"),
        );
        let e = src.list(&q2, &f).await.unwrap_err();
        assert!(matches!(e, FetchError::ListingParse { .. }), "{e}");
        // A base the folders cannot be joined to.
        let odd = ReportFolders::new("mailto:x", registry.clone()).unwrap();
        assert!(matches!(
            odd.folder_url(&registry.folders()[0].sides()[0]),
            Err(FetchError::Parse {
                what: "folder URL",
                ..
            })
        ));
        // A server that fails.
        t.add(
            "https://h/Reports/ARCHIVE/TradingIS_Reports/",
            Canned::status(500),
        );
        assert!(src.list(&q2, &f).await.is_err());
        // A base that is not a URL, and one without a trailing slash.
        assert!(ReportFolders::new("not a url", registry.clone()).is_err());
        let s2 = ReportFolders::new("https://h/mirror", registry).unwrap();
        assert_eq!(
            s2.folder_url(&s2.registry.folders()[0].sides()[0])
                .unwrap()
                .as_str(),
            "https://h/mirror/Reports/ARCHIVE/Dispatch_SCADA/"
        );
    }

    trait Named {
        fn remote_name(&self) -> String;
    }

    impl Named for RemoteFile {
        fn remote_name(&self) -> String {
            self.file_name()
        }
    }

    #[test]
    fn picks_are_the_entries_within_the_instants_and_the_archive_is_read_back() {
        let (_dir, registry) = registry();
        let src = ReportFolders::new("https://h", registry).unwrap();
        let daily = RemoteFile {
            url: Url::parse(
                "https://h/Reports/ARCHIVE/Dispatch_SCADA/PUBLIC_DISPATCHSCADA_20261019.zip",
            )
            .unwrap(),
            source: REPORTS_SOURCE_ID,
            table: None,
            period: Some(Period::Day(day("2026-10-19"))),
            size: None,
            modified: Some("m".into()),
            modified_at: None,
        };
        let entry = |name: &str, dir: bool| Entry {
            name: if dir {
                format!("{name}/")
            } else {
                name.to_owned()
            },
            size: 10,
            compressed: 8,
            crc32: 0,
            method: 0,
            dos_date: 0,
            dos_time: 0,
            local_header_offset: 0,
        };
        let entries = [
            entry(
                "PUBLIC_DISPATCHSCADA_202610190005_0000000543611267.zip",
                false,
            ),
            entry(
                "PUBLIC_DISPATCHSCADA_202610190010_0000000543611895.zip",
                false,
            ),
            entry(
                "PUBLIC_DISPATCHSCADA_202610190015_0000000543612430.zip",
                false,
            ),
            entry(
                "PUBLIC_DISPATCHSCADA_202610190020_0000000543612972.zip",
                false,
            ),
            entry("folder", true),
            entry("README.txt", false),
        ];
        // No instants: the whole day.
        assert_eq!(
            src.pick(
                &daily,
                &entries,
                &query(&["DISPATCH_UNIT_SCADA"], "2026-10-19", "2026-10-19", None)
            ),
            Picks::Whole
        );
        // 00:05 to 00:10: the file at 00:05 (its interval ends then), the
        // one at 00:10, and the one at 00:15 for the last interval.
        let start = day("2026-10-19").start_micros(36_000);
        let five = 5 * 60 * 1_000_000;
        let q = query(
            &["DISPATCH_UNIT_SCADA"],
            "2026-10-19",
            "2026-10-19",
            Some((start + five, start + 2 * five)),
        );
        let picked = src.pick(&daily, &entries, &q);
        assert!(matches!(&picked, Picks::Entries(_)), "{picked:?}");
        let p = match picked {
            Picks::Entries(p) => p,
            Picks::Whole => Vec::new(),
        };
        assert_eq!(p.iter().map(|p| p.index).collect::<Vec<_>>(), [0, 1, 2]);
        assert_eq!(
            p[0].remote.url.as_str(),
            "https://h/Reports/ARCHIVE/Dispatch_SCADA/PUBLIC_DISPATCHSCADA_20261019.zip!PUBLIC_DISPATCHSCADA_202610190005_0000000543611267.zip"
        );
        assert_eq!(
            (p[0].remote.size, p[0].remote.modified.as_deref()),
            (Some(10), Some("m"))
        );
        assert_eq!(p[0].remote.period, daily.period);
        // The archive read back: files of the folder for the days asked.
        let home = tempfile::tempdir().unwrap();
        let archive = Archive::new(home.path());
        let put = |url: &str| {
            let path = archive.path_for(url);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"zip").unwrap();
            let record = phasor_acquire::Provenance::new(
                phasor_acquire::Content::of(b"zip"),
                "test",
                phasor_acquire::Address {
                    url: url.into(),
                    seen: phasor_core::timetext::Stamp::new(1_792_454_400, 0),
                    gone: None,
                    via: phasor_acquire::Via::Fetched {
                        listed: None,
                        served: None,
                    },
                },
            );
            Archive::write(&path, &record).unwrap();
        };
        put("https://h/Reports/ARCHIVE/Dispatch_SCADA/PUBLIC_DISPATCHSCADA_20261018.zip");
        put(
            "https://h/Reports/ARCHIVE/Dispatch_SCADA/PUBLIC_DISPATCHSCADA_20261019.zip!PUBLIC_DISPATCHSCADA_202610190005_0000000543611267.zip",
        );
        put(
            "https://h/Reports/CURRENT/Dispatch_SCADA/PUBLIC_DISPATCHSCADA_202610200005_0000000543700000.zip",
        );
        put("https://h/Reports/CURRENT/Dispatch_SCADA/notes.txt");
        // A sidecar whose URL is not one: passed over.
        put("x/PUBLIC_DISPATCHSCADA_20261019.zip");
        let got = src
            .archived(
                &archive,
                &query(&["DISPATCH_UNIT_SCADA"], "2026-10-19", "2026-10-20", None),
            )
            .unwrap();
        let names: Vec<String> = got.iter().map(|l| l.remote.file_name()).collect();
        assert_eq!(
            names,
            [
                "PUBLIC_DISPATCHSCADA_20261019.zip!PUBLIC_DISPATCHSCADA_202610190005_0000000543611267.zip",
                "PUBLIC_DISPATCHSCADA_202610200005_0000000543700000.zip",
            ]
        );
        assert!(got.iter().all(|l| l.cached && l.size == 3));
        assert_eq!(got[0].remote.period, Some(Period::Day(day("2026-10-19"))));
        // Finer than a day: only the files stamped within the instants.
        let got = src
            .archived(
                &archive,
                &query(
                    &["DISPATCH_UNIT_SCADA"],
                    "2026-10-19",
                    "2026-10-20",
                    Some((start, start + five)),
                ),
            )
            .unwrap();
        let names: Vec<String> = got.iter().map(|l| l.remote.file_name()).collect();
        assert_eq!(
            names,
            [
                "PUBLIC_DISPATCHSCADA_20261019.zip!PUBLIC_DISPATCHSCADA_202610190005_0000000543611267.zip"
            ]
        );
        let _ = YearMonth::new(2026, 10);
    }
}
