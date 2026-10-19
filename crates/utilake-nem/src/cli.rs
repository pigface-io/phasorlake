//! The NEM's command line: `utilake fetch nem ...`.

use clap::Args;
use utilake_core::YearMonth;

/// `fetch nem`: the monthly archive, by table and month.
#[derive(Args, Debug, Clone)]
pub struct FetchNem {
    /// Tables, comma separated (default: every table the archive has
    /// for the months asked, declared in the registry or not).
    #[arg(long, value_delimiter = ',')]
    pub table: Vec<String>,
    /// Tables to leave out, comma separated.
    #[arg(long, value_delimiter = ',')]
    pub exclude: Vec<String>,
    /// First month, YYYY-MM.
    #[arg(long)]
    pub from: YearMonth,
    /// Last month, YYYY-MM (default: same as --from).
    #[arg(long)]
    pub to: Option<YearMonth>,
    /// List what would be fetched and stop.
    #[arg(long)]
    pub dry_run: bool,
    /// Concurrent downloads.
    #[arg(long)]
    pub downloads: Option<usize>,
}

impl FetchNem {
    /// The request, as the manifest records it.
    pub fn request(&self, to: YearMonth) -> String {
        format!(
            "fetch nem {}{} {}..{to}",
            if self.table.is_empty() {
                "*".to_owned()
            } else {
                self.table.join(",")
            },
            if self.exclude.is_empty() {
                String::new()
            } else {
                format!(" except {}", self.exclude.join(","))
            },
            self.from
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_names_tables_and_months() {
        let ym = |y, m| YearMonth::new(y, m).unwrap();
        let f = FetchNem {
            table: vec!["A".into(), "B".into()],
            exclude: vec!["C".into()],
            from: ym(2024, 1),
            to: None,
            dry_run: false,
            downloads: None,
        };
        assert_eq!(
            f.request(ym(2024, 2)),
            "fetch nem A,B except C 2024-01..2024-02"
        );
        let all = FetchNem {
            table: Vec::new(),
            exclude: Vec::new(),
            ..f
        };
        assert_eq!(all.request(ym(2024, 1)), "fetch nem * 2024-01..2024-01");
    }
}
