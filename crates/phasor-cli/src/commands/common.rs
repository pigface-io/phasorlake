//! What `fetch` and `ingest` share: which tables of which market, over what window.

use anyhow::{Context as _, Result, bail};
use phasor_core::timetext;
use phasor_core::{Diagnose, TimeRange, YearMonth};
use phasor_pipeline::days_of;
use phasor_registry::file_model::MarketDef;
use phasor_registry::{Market, Registry, TableId};

use crate::cli::{TablesArg, WindowArgs};
use crate::context::Context;

/// A diagnostic as an error.
pub fn fail(e: &impl Diagnose) -> anyhow::Error {
    anyhow::anyhow!("{}", e.diagnostic().render())
}

/// The tables a command works on, and the one market they are of.
pub struct Selection {
    /// The tables in the order asked, less the excluded, upper case.
    pub names: Vec<String>,
    /// The market's constants.
    pub def: MarketDef,
    /// The kit that reads the market: its id is the market's.
    pub kit: String,
}

/// The table a name means: by name, or as a `TYPE.SUBTYPE` pair.
pub fn table_id(r: &Registry, name: &str) -> Option<TableId> {
    let upper = name.to_ascii_uppercase();
    r.table_by_name(&upper).or_else(|| {
        let (rt, st) = upper.split_once('.')?;
        Market::ALL
            .into_iter()
            .find_map(|m| r.table_by_pair(m, rt, st))
    })
}

/// Resolves the tables asked for; `market` is needed only when none is named or a name is unknown.
pub fn select(ctx: &Context, args: &TablesArg) -> Result<Selection> {
    let r = &ctx.registry;
    let mut ids = Vec::new();
    for t in &args.tables {
        let Some(id) = table_id(r, t) else {
            bail!("No table is named {t}; `phasor list` names them.");
        };
        if !args
            .exclude
            .iter()
            .any(|x| x.eq_ignore_ascii_case(&r.table(id).name))
        {
            ids.push(id);
        }
    }
    let market = match (&args.market, ids.first()) {
        (Some(m), _) => Market::parse(&m.to_ascii_lowercase()).ok_or_else(|| {
            anyhow::anyhow!("No market named {m}; the markets are nem, wem and gas.")
        })?,
        (None, Some(id)) => r.table(*id).market,
        (None, None) => bail!("Name a table, or --market for every table of a market."),
    };
    if let Some(odd) = ids.iter().find(|id| r.table(**id).market != market) {
        bail!(
            "{} is of the {} market, the others of {market}; one market a command.",
            r.table(*odd).name,
            r.table(*odd).market
        );
    }
    let def = r
        .market(market)
        .cloned()
        .context("the registry defines no such market")?;
    Ok(Selection {
        names: ids.iter().map(|id| r.table(*id).name.clone()).collect(),
        def,
        kit: market.to_string(),
    })
}

/// An instant from `YYYY-MM`, `YYYY-MM-DD` or `YYYY-MM-DD[T ]HH:MM[:SS]` at `offset_secs`,
/// or RFC 3339.
/// As an end, a month or day means the instant after its last; as a start, its first.
pub fn parse_when(text: &str, offset_secs: i32, end: bool) -> Result<i64> {
    let text = text.trim();
    let bad =
        || anyhow::anyhow!("{text:?} is not a month, a day, a local time or an RFC 3339 time");
    if let Ok(ym) = text.parse::<YearMonth>() {
        let m = if end { ym.next() } else { ym };
        return Ok(phasor_core::Period::Month(m)
            .first_day()
            .start_micros(offset_secs));
    }
    if let Ok(day) = text.parse::<phasor_core::CivilDay>() {
        let d = if end { day.next() } else { day };
        return Ok(d.start_micros(offset_secs));
    }
    let padded = match text.len() {
        10 => format!("{text}T00:00:00"),
        16 => format!("{text}:00"),
        _ => text.to_owned(),
    };
    timetext::parse_rfc3339(text)
        .or_else(|| timetext::parse_naive(&padded, offset_secs))
        .map(|s| s.secs_utc * 1_000_000 + i64::from(s.nanos / 1_000))
        .ok_or_else(bad)
}

/// An instant as RFC 3339 in an offset, to the second.
pub fn rfc3339(us: i64, offset_secs: i32) -> String {
    timetext::format_rfc3339(us.div_euclid(1_000_000), offset_secs)
}

/// A window in the market's clock, with the days and instants a fetch must cover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    /// `[from, to)` as microseconds since the epoch, for the sources.
    pub instants: (i64, i64),
}

impl Window {
    /// The days the window touches, in the market's clock.
    pub fn days(&self, def: &MarketDef) -> TimeRange {
        days_of(self.instants, def.utc_offset_secs)
    }
}

/// The window the flags ask for; none when none was given.
pub fn window(args: &WindowArgs, def: &MarketDef) -> Result<Option<Window>> {
    let off = def.utc_offset_secs;
    let w = match (&args.from, &args.to, &args.at, &args.day) {
        (Some(from), Some(to), _, _) => {
            let (a, b) = (parse_when(from, off, false)?, parse_when(to, off, true)?);
            if b <= a {
                bail!("--to {to} is not after --from {from}.");
            }
            Window { instants: (a, b) }
        }
        (_, _, Some(at), _) => {
            let t = parse_when(at, off, false)?;
            Window {
                instants: (t, t + 1),
            }
        }
        (_, _, _, Some(day)) => {
            let d = day.parse::<phasor_core::CivilDay>().map_err(|_| {
                anyhow::anyhow!("--day takes a date, YYYY-MM-DD; {day:?} is not one")
            })?;
            let start = d.start_micros(off) + i64::from(def.trading_day_start_secs) * 1_000_000;
            let end = start + 86_400_000_000;
            Window {
                instants: (start, end),
            }
        }
        _ => return Ok(None),
    };
    Ok(Some(w))
}

/// The request line the manifest records.
pub fn request(verb: &str, sel: &Selection, w: Option<&Window>, def: &MarketDef) -> String {
    let tables = if sel.names.is_empty() {
        "*".to_owned()
    } else {
        sel.names.join(",")
    };
    match w {
        Some(w) => format!(
            "{verb} {} {} {}..{}",
            sel.kit,
            tables,
            rfc3339(w.instants.0, def.utc_offset_secs),
            rfc3339(w.instants.1, def.utc_offset_secs)
        ),
        None => format!("{verb} {} {tables}", sel.kit),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phasor_registry::file_model::TimestampFormat;

    fn nem() -> MarketDef {
        MarketDef {
            market: Market::Nem,
            utc_offset_secs: 36_000,
            timestamp_format: TimestampFormat::AemoSlash,
            trading_day_start_secs: 14_400,
        }
    }

    #[test]
    fn every_form_of_when_reads_in_market_time() {
        let aest = 36_000;
        let day = |s: &str| s.parse::<phasor_core::CivilDay>().unwrap();
        assert_eq!(
            parse_when("2024-01", aest, false).unwrap(),
            day("2024-01-01").start_micros(aest)
        );
        assert_eq!(
            parse_when("2024-01", aest, true).unwrap(),
            day("2024-02-01").start_micros(aest)
        );
        assert_eq!(
            parse_when("2024-01-15", aest, false).unwrap(),
            day("2024-01-15").start_micros(aest)
        );
        assert_eq!(
            parse_when("2024-01-15", aest, true).unwrap(),
            day("2024-01-16").start_micros(aest)
        );
        // A local time in market time; the same as an end.
        let ten = day("2024-01-15").start_micros(aest) + 10 * 3_600_000_000;
        assert_eq!(parse_when("2024-01-15T10:00", aest, false).unwrap(), ten);
        assert_eq!(parse_when("2024-01-15 10:00:00", aest, true).unwrap(), ten);
        // RFC 3339 carries its own offset.
        assert_eq!(
            parse_when("2024-01-15T00:00:00Z", aest, false).unwrap(),
            day("2024-01-15").start_micros(0)
        );
        assert_eq!(
            parse_when("2024-01-15T10:00:00+10:00", aest, true).unwrap(),
            ten
        );
        for bad in ["yesterday", "2024-13-01", "2024-01-15T25:00", ""] {
            assert!(parse_when(bad, aest, false).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_window_is_a_range_an_instant_or_a_trading_day() {
        let def = nem();
        let args = |from: Option<&str>, to: Option<&str>, at: Option<&str>, day: Option<&str>| {
            WindowArgs {
                from: from.map(str::to_owned),
                to: to.map(str::to_owned),
                at: at.map(str::to_owned),
                day: day.map(str::to_owned),
            }
        };
        assert_eq!(window(&args(None, None, None, None), &def).unwrap(), None);
        let w = window(
            &args(Some("2024-01-01"), Some("2024-01-01"), None, None),
            &def,
        )
        .unwrap()
        .unwrap();
        let midnight = 1_704_031_200_000_000;
        assert_eq!(w.instants, (midnight, midnight + 86_400_000_000));
        assert_eq!(w.days(&def).to_string(), "2024-01-01");
        let w = window(&args(None, None, Some("2024-01-01T02:00"), None), &def)
            .unwrap()
            .unwrap();
        assert_eq!(
            w.instants,
            (midnight + 7_200_000_000, midnight + 7_200_000_001)
        );
        // A trading day starts at 04:00.
        let w = window(&args(None, None, None, Some("2024-01-01")), &def)
            .unwrap()
            .unwrap();
        assert_eq!(
            w.instants,
            (
                midnight + 14_400_000_000,
                midnight + 14_400_000_000 + 86_400_000_000
            )
        );
        assert_eq!(w.days(&def).to_string(), "2024-01-01..2024-01-02");
        assert!(window(&args(Some("2024-02"), Some("2024-01"), None, None), &def).is_err());
        assert!(window(&args(None, None, None, Some("soon")), &def).is_err());
    }
}
