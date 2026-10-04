//! Which tables are standing data: tables whose monthly archive file is the
//! whole table as it stood, not the rows of that month.
//!
//! Loading two months of such a table without replacing the first repeats
//! every row, so the lake keeps each month's file as a snapshot of its
//! own. Which tables these are is not a judgment made here. It comes from
//! two places, both of which can be checked:
//!
//! 1. The `README.txt` AEMO ships in the `DOCUMENTATION` directory of every
//!    archive month, which lists the "Static/Standing data tables". The
//!    file is committed as a fixture and read by [`readme_tables`].
//! 2. A short list of tables the README does not name, each of which the
//!    committed samples show to behave the same way: every sampled row of
//!    an earlier month is there again in a later one. A test holds each
//!    entry to that, by [`repeats`].

/// The tables AEMO's archive README lists as static or standing data, in
/// the order it lists them, upper-case.
///
/// The list is the run of lines, each holding one table name and nothing
/// else, that follows the sentence introducing it.
pub fn readme_tables(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut listing = false;
    for line in text.lines() {
        let line = line.trim();
        if line
            .to_ascii_lowercase()
            .contains("the following tables are the static/standing data tables")
        {
            listing = true;
            continue;
        }
        if !listing || line.is_empty() {
            continue;
        }
        let is_name = line.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && line.chars().any(|c| c.is_ascii_alphabetic());
        if !is_name {
            // The list ends at the first line that is not a name.
            break;
        }
        out.push(line.to_ascii_uppercase());
    }
    out
}

/// One sampled file as rows of named values: the column names of its I
/// row and the fields of each D row after the four-field prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rows {
    /// Column names, in file order.
    pub columns: Vec<String>,
    /// Data rows, in file order.
    pub rows: Vec<Vec<String>>,
}

/// Splits one CSV line into fields, honouring double quotes.
fn fields(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// Reads the first block of an MMS CSV as [`Rows`]; `None` when it has no
/// I row.
pub fn first_block(text: &str) -> Option<Rows> {
    let mut columns: Option<Vec<String>> = None;
    let mut rows = Vec::new();
    for line in text.lines() {
        let f = fields(line.trim_end_matches('\r'));
        match f.first().map(String::as_str) {
            Some("I") if columns.is_none() => {
                columns = Some(
                    f.into_iter()
                        .skip(4)
                        .map(|c| c.to_ascii_uppercase())
                        .collect(),
                );
            }
            // A second block is another table or another version.
            Some("I") => break,
            Some("D") if columns.is_some() => rows.push(f.into_iter().skip(4).collect()),
            _ => {}
        }
    }
    columns.map(|columns| Rows { columns, rows })
}

/// Whether every row sampled from the earlier file is among the rows
/// sampled from the later one, compared on the columns both have, the
/// change stamp aside. This is what a standing-data table looks like from
/// two samples: the later file starts with the same rows the earlier one
/// did. A table of monthly rows never passes, since its rows are dated.
///
/// Fewer than `at_least` rows in the earlier sample is no evidence either
/// way and gives `false`.
pub fn repeats(earlier: &Rows, later: &Rows, at_least: usize) -> bool {
    if earlier.rows.len() < at_least {
        return false;
    }
    let common: Vec<(usize, usize)> = earlier
        .columns
        .iter()
        .enumerate()
        .filter(|(_, c)| *c != "LASTCHANGED")
        .filter_map(|(i, c)| later.columns.iter().position(|d| d == c).map(|j| (i, j)))
        .collect();
    if common.is_empty() {
        return false;
    }
    let pick = |row: &[String], side: fn(&(usize, usize)) -> usize| -> Vec<String> {
        common
            .iter()
            .map(|pair| row.get(side(pair)).cloned().unwrap_or_default())
            .collect()
    };
    let later_rows: std::collections::BTreeSet<Vec<String>> =
        later.rows.iter().map(|r| pick(r, |p| p.1)).collect();
    earlier
        .rows
        .iter()
        .all(|r| later_rows.contains(&pick(r, |p| p.0)))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    use super::*;

    fn bootstrap() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/bootstrap")
    }

    /// The first block of the head of `table` in a month's fixtures.
    fn head(month: &str, table: &str) -> Option<Rows> {
        let dir = bootstrap().join(month).join("heads");
        let path = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .find(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .and_then(nemlake_core::archive_name::parse)
                    .is_some_and(|n| n.table == table && n.file.is_none_or(|f| f == 1))
            })?;
        first_block(&String::from_utf8_lossy(&std::fs::read(path).unwrap()))
    }

    fn observed() -> Vec<String> {
        #[derive(serde::Deserialize)]
        struct List {
            observed: Vec<String>,
        }
        let text = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("snapshot_tables.toml"),
        )
        .unwrap();
        toml::from_str::<List>(&text).unwrap().observed
    }

    #[test]
    fn reads_the_list_out_of_aemos_readme() {
        let text = std::fs::read(bootstrap().join("MMSDM_2026_08/README.txt")).unwrap();
        let tables = readme_tables(&String::from_utf8_lossy(&text));
        assert_eq!(tables.len(), 48, "{tables:?}");
        assert_eq!(tables[0], "ANCILLARY_RECOVERY_SPLIT");
        assert_eq!(tables[47], "STATION");
        for t in [
            "DUDETAIL",
            "REGION",
            "PARTICIPANT",
            "GST_RATE",
            "OVERRIDERRP",
        ] {
            assert!(tables.iter().any(|x| x == t), "{t}");
        }
        // Text that is not the list gives nothing; the list ends at the
        // first line that is not a name.
        assert!(readme_tables("no list here\nTABLE_A\n").is_empty());
        assert_eq!(
            readme_tables(
                "The following tables are the Static/Standing data tables\r\n\r\n\tone_a\r\n\tTWO\r\n\r\nREM * end\r\n\tTHREE\r\n"
            ),
            ["ONE_A", "TWO"]
        );
        assert!(
            readme_tables("The following tables are the Static/Standing data tables\n123\n")
                .is_empty()
        );
    }

    #[test]
    fn splits_quoted_fields_and_reads_one_block() {
        assert_eq!(
            fields(r#"D,A,,1,"x, y","say ""hi""",2"#),
            ["D", "A", "", "1", "x, y", "say \"hi\"", "2"]
        );
        let rows = first_block(
            "C,HEAD\r\nD,stray,row,1,x\r\nI,T,S,1,a,B\r\nD,T,S,1,1,\"2\"\r\nD,T,S,1,3,4\r\nI,T,OTHER,1,C\r\nD,T,OTHER,1,9\r\n",
        )
        .unwrap();
        assert_eq!(rows.columns, ["A", "B"]);
        assert_eq!(rows.rows, [["1", "2"], ["3", "4"]]);
        assert!(first_block("C,only a comment\n").is_none());
    }

    #[test]
    fn repeating_rows_are_told_from_dated_ones() {
        let rows = |columns: &[&str], rows: &[&[&str]]| Rows {
            columns: columns.iter().map(|c| (*c).to_owned()).collect(),
            rows: rows
                .iter()
                .map(|r| r.iter().map(|f| (*f).to_owned()).collect())
                .collect(),
        };
        let earlier = rows(
            &["ID", "NAME", "LASTCHANGED"],
            &[&["1", "a", "t1"], &["2", "b", "t1"]],
        );
        // The same rows later, in another order, with a column more and
        // another change stamp: the table stands.
        let later = rows(
            &["NAME", "ID", "EXTRA", "LASTCHANGED"],
            &[
                &["b", "2", "x", "t2"],
                &["a", "1", "y", "t2"],
                &["c", "3", "z", "t2"],
            ],
        );
        assert!(repeats(&earlier, &later, 2));
        assert!(
            !repeats(&later, &earlier, 2),
            "the later file has a row more"
        );
        // Too few rows to say.
        assert!(!repeats(&earlier, &later, 3));
        // Rows of another month.
        let dated = rows(&["ID", "NAME"], &[&["7", "g"], &["8", "h"]]);
        assert!(!repeats(&earlier, &dated, 2));
        // Nothing in common to compare on.
        let other = rows(&["X"], &[&["1"], &["2"]]);
        assert!(!repeats(&earlier, &other, 2));
        // A short row reads as empty fields.
        let short = rows(&["ID", "NAME"], &[&["1"]]);
        assert!(!repeats(&short, &later, 1));
    }

    /// Every table on the observed list earns its place: the committed
    /// samples of two months, thirty-one months apart, hold the same rows.
    /// And the list holds nothing AEMO's README already names.
    #[test]
    fn the_observed_list_is_borne_out_by_the_fixtures() {
        let readme: BTreeSet<String> = readme_tables(&String::from_utf8_lossy(
            &std::fs::read(bootstrap().join("MMSDM_2026_08/README.txt")).unwrap(),
        ))
        .into_iter()
        .collect();
        let observed = observed();
        assert!(!observed.is_empty());
        for table in &observed {
            assert!(!readme.contains(table), "{table} is in the README already");
            let earlier = head("MMSDM_2024_01", table)
                .unwrap_or_else(|| panic!("no 2024-01 head of {table}"));
            let later = head("MMSDM_2026_08", table)
                .unwrap_or_else(|| panic!("no 2026-08 head of {table}"));
            assert!(
                repeats(&earlier, &later, 5),
                "{table}: the rows sampled in 2024-01 are not all in the 2026-08 sample"
            );
        }
        // The README's own tables show the same where the samples can
        // show it at all, and a table of monthly rows does not.
        for table in ["REGION", "PARTICIPANT", "GST_RATE", "STATION"] {
            assert!(repeats(
                &head("MMSDM_2024_01", table).unwrap(),
                &head("MMSDM_2026_08", table).unwrap(),
                4
            ));
        }
        for table in [
            "DISPATCHPRICE",
            "GENCONDATA",
            "SPDCONNECTIONPOINTCONSTRAINT",
        ] {
            assert!(!repeats(
                &head("MMSDM_2024_01", table).unwrap(),
                &head("MMSDM_2026_08", table).unwrap(),
                1
            ));
        }
    }
}
