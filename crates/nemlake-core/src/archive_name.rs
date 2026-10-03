//! The names AEMO gives the files of the monthly archive.
//!
//! Two schemes exist. Until 2024 every file is
//!
//! ```text
//! PUBLIC_DVD_{TABLE}{split?}[_ALL]_{YYYYMM}010000.zip      the data, with its .CSV inside
//! PUBLIC_DVD_{TABLE}{split?}_{YYYYMM}.ctl                  its control file
//! ```
//!
//! where a big table is split over files by a digit glued to its name
//! (`BIDPEROFFER1`, `BIDPEROFFER2`) and the big-table directories add `_ALL`.
//! The newer scheme separates the parts with `#` and numbers every file:
//!
//! ```text
//! PUBLIC_ARCHIVE#{TABLE}#FILE{NN}#{YYYYMM}010000.zip
//! PUBLIC_ARCHIVE#{TABLE}#FILE{NN}#{YYYYMM}.ctl
//! ```
//!
//! One parser for both lives here so the fetcher, the pipeline and the
//! schema bootstrap agree on what a name means.

use crate::period::YearMonth;

/// What an archive file name says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveName {
    /// The table part exactly as written, upper-case, `_ALL` removed. In the
    /// old scheme it still carries a split digit (`BIDPEROFFER1`), because
    /// only a registry can tell a split digit from a name that ends in one.
    pub table: String,
    /// The file number of the `#` scheme (`FILE02` is 2).
    pub file: Option<u8>,
    /// The archive month.
    pub month: YearMonth,
}

impl ArchiveName {
    /// `table` without a trailing split digit, and the digit: `BIDPEROFFER1`
    /// is `("BIDPEROFFER", Some(1))`. A name that is all digits, or has
    /// none, is returned whole.
    #[must_use]
    pub fn split_table(&self) -> (&str, Option<u8>) {
        let digits = self
            .table
            .bytes()
            .rev()
            .take_while(u8::is_ascii_digit)
            .count();
        if digits == 0 || digits == self.table.len() {
            return (&self.table, None);
        }
        let (base, n) = self.table.split_at(self.table.len() - digits);
        (base, n.parse().ok())
    }
}

/// Parses an archive file name: a data zip, its CSV, a control file, or
/// the bare stem of any of them. `%23` is read as `#`, so a name taken from
/// a URL works. Returns `None` for anything else.
#[must_use]
pub fn parse(name: &str) -> Option<ArchiveName> {
    let name = name.trim();
    let decoded;
    let name = if name.contains("%23") {
        decoded = name.replace("%23", "#");
        decoded.as_str()
    } else {
        name
    };
    let upper = name.to_ascii_uppercase();
    let stem = [".ZIP", ".CSV", ".CTL"]
        .iter()
        .find_map(|ext| upper.strip_suffix(ext))
        .unwrap_or(&upper);
    if let Some(rest) = stem.strip_prefix("PUBLIC_ARCHIVE#") {
        let mut parts = rest.split('#');
        let table = parts.next().filter(|t| !t.is_empty())?;
        let file = parts.next()?.strip_prefix("FILE")?.parse::<u8>().ok()?;
        let month = month_of(parts.next()?)?;
        if parts.next().is_some() {
            return None;
        }
        return Some(ArchiveName {
            table: table.to_owned(),
            file: Some(file),
            month,
        });
    }
    let rest = stem.strip_prefix("PUBLIC_DVD_")?;
    let (table, stamp) = rest.rsplit_once('_')?;
    let month = month_of(stamp)?;
    let table = table.strip_suffix("_ALL").unwrap_or(table);
    if table.is_empty() {
        return None;
    }
    Some(ArchiveName {
        table: table.to_owned(),
        file: None,
        month,
    })
}

/// `YYYYMM` (control files) or `YYYYMM010000` (data files).
fn month_of(stamp: &str) -> Option<YearMonth> {
    if !stamp.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    match stamp.len() {
        6 => {}
        12 if stamp.ends_with("010000") => {}
        _ => return None,
    }
    YearMonth::new(stamp[..4].parse().ok()?, stamp[4..6].parse().ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ym(y: i32, m: u8) -> YearMonth {
        YearMonth::new(y, m).unwrap()
    }

    #[test]
    fn old_scheme() {
        let n = parse("PUBLIC_DVD_DISPATCH_UNIT_SCADA_202401010000.zip").unwrap();
        assert_eq!(n.table, "DISPATCH_UNIT_SCADA");
        assert_eq!(n.file, None);
        assert_eq!(n.month, ym(2024, 1));
        assert_eq!(n.split_table(), ("DISPATCH_UNIT_SCADA", None));
        let n = parse("PUBLIC_DVD_BIDPEROFFER1_202401010000.CSV").unwrap();
        assert_eq!(n.table, "BIDPEROFFER1");
        assert_eq!(n.split_table(), ("BIDPEROFFER", Some(1)));
        let n = parse("PUBLIC_DVD_P5MIN_CONSTRAINTSOLUTION4_ALL_202401010000").unwrap();
        assert_eq!(n.table, "P5MIN_CONSTRAINTSOLUTION4");
        assert_eq!(n.split_table(), ("P5MIN_CONSTRAINTSOLUTION", Some(4)));
        let n = parse("public_dvd_region_201201.ctl").unwrap();
        assert_eq!((n.table.as_str(), n.month), ("REGION", ym(2012, 1)));
        // A table whose name is only digits after the prefix is left whole.
        assert_eq!(
            parse("PUBLIC_DVD_2024_202401010000.zip")
                .unwrap()
                .split_table(),
            ("2024", None)
        );
    }

    #[test]
    fn hash_scheme() {
        let n = parse("PUBLIC_ARCHIVE#DISPATCHPRICE#FILE01#202608010000.zip").unwrap();
        assert_eq!(n.table, "DISPATCHPRICE");
        assert_eq!(n.file, Some(1));
        assert_eq!(n.month, ym(2026, 8));
        let n = parse("PUBLIC_ARCHIVE%23BIDOFFERPERIOD%23FILE06%23202608.ctl").unwrap();
        assert_eq!((n.table.as_str(), n.file), ("BIDOFFERPERIOD", Some(6)));
        let n = parse(" PUBLIC_ARCHIVE#P5MIN_CONSTRAINTSOLUTION#FILE03#202608010000.CSV ").unwrap();
        assert_eq!(n.table, "P5MIN_CONSTRAINTSOLUTION");
        // In this scheme a trailing digit belongs to the table name.
        assert_eq!(n.file, Some(3));
    }

    #[test]
    fn rejects_other_names() {
        for bad in [
            "",
            "nothing",
            "Readme.htm",
            "PUBLIC_DISPATCHIS_202610011340_0000000540580033.zip",
            "PUBLIC_DVD_X_2024.zip",
            "PUBLIC_DVD_X_202401010001.zip",
            "PUBLIC_DVD_X_2024010100000.zip",
            "PUBLIC_DVD_X_202499010000.zip",
            "PUBLIC_DVD__202401010000.zip",
            "PUBLIC_DVD__ALL_202401010000.zip",
            "PUBLIC_DVD_X_20240a010000.zip",
            "PUBLIC_ARCHIVE#X#FILE01.zip",
            "PUBLIC_ARCHIVE#X#PART01#202608010000.zip",
            "PUBLIC_ARCHIVE#X#FILExx#202608010000.zip",
            "PUBLIC_ARCHIVE##FILE01#202608010000.zip",
            "PUBLIC_ARCHIVE#X#FILE01#202608010000#more.zip",
            "PUBLIC_ARCHIVE#X#FILE01#2026.zip",
        ] {
            assert_eq!(parse(bad), None, "{bad}");
        }
    }
}
