//! The MMS `I` row: the schema header that starts every block of data rows.
//!
//! ```text
//! I,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID,SCADAVALUE
//! ```
//!
//! Fields 1..=3 are the report type, report subtype and version; the rest are
//! column names. Data rows repeat the first three fields before their values.

use utilake_core::{Fingerprint, normalize_name};

/// A parsed `I` row, borrowing from the line it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IRow<'a> {
    /// Field 1, e.g. `DISPATCH`.
    pub report_type: &'a str,
    /// Field 2, e.g. `UNIT_SCADA`.
    pub report_subtype: &'a str,
    /// Field 3, the schema version AEMO stamps on the block.
    pub version: u16,
    /// Fields 4.., already normalised with [`normalize_name`], trailing
    /// empty names dropped (AEMO leaves a trailing comma on some I rows).
    pub columns: &'a [&'a str],
}

impl IRow<'_> {
    /// The fingerprint of this row's column list.
    pub fn fingerprint(&self) -> Fingerprint {
        Fingerprint::of_names(self.columns.iter().copied())
    }
}

/// Why an `I` row could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IRowError {
    /// Fewer than five fields.
    #[error(
        "it has {0} fields; an I row needs a report type, a subtype, a version and at least one column"
    )]
    TooShort(usize),
    /// Field 3 is not a small integer.
    #[error("its version field {0:?} is not an integer between 0 and 65535")]
    Version(String),
    /// A field failed [`normalize_name`] (or is not UTF-8).
    #[error("its field {index} {raw:?} is not a name")]
    Name {
        /// 0-based field index in the row.
        index: usize,
        /// The offending text.
        raw: String,
    },
    /// An empty column name before the last named column.
    #[error("its field {0} is an empty column name that is not at the end of the row")]
    EmptyColumn(usize),
}

/// Reusable buffers for reading `I` rows.
///
/// A name already in canonical form (upper-case ASCII, no quotes, no
/// surrounding space; nearly every AEMO name) is borrowed from the line, not
/// copied. Any other is normalised once into an arena kept between rows. The
/// column list reuses one allocation too, so a file of many blocks reads
/// every I row after the first without allocating.
#[derive(Debug, Default)]
pub struct IRowScratch {
    /// Normalised names that could not be borrowed, end to end.
    text: String,
    /// One name being normalised.
    name: String,
    /// Allocation of the per-row list of names, empty between rows.
    names: Vec<Name<'static>>,
    /// Allocation of the per-row column list, empty between rows.
    columns: Vec<&'static str>,
}

/// A name of an I row: borrowed from the line, or a span of the arena.
#[derive(Debug, Clone, Copy)]
enum Name<'a> {
    Line(&'a str),
    Arena(u32, u32),
    Empty,
}

impl<'a> Name<'a> {
    fn resolve<'t>(self, arena: &'t str) -> &'t str
    where
        'a: 't,
    {
        match self {
            Self::Line(s) => s,
            Self::Arena(a, b) => &arena[a as usize..b as usize],
            Self::Empty => "",
        }
    }
}

/// Empties `v` and gives back its allocation for elements of another
/// lifetime. Same-layout `into_iter().collect()` is done in place, so this
/// never allocates.
fn recycle<A, B>(mut v: Vec<A>) -> Vec<B> {
    v.clear();
    v.into_iter()
        .map(|_| unreachable!("the vector is empty"))
        .collect()
}

/// Whether `raw` already is what [`normalize_name`] would make of it.
#[inline]
fn is_canonical(raw: &str) -> bool {
    !raw.is_empty()
        && raw
            .bytes()
            .all(|b| b.is_ascii_graphic() && !b.is_ascii_lowercase() && b != b'"' && b != b',')
}

impl IRowScratch {
    /// Reads an `I` row from its fields, field 0 (`I`) included, and hands
    /// it to `f`. A field that is not UTF-8 is `None`. Every reader of I
    /// rows goes through this, so two readers never disagree on a shape.
    pub fn with_header<'a, R>(
        &mut self,
        fields: impl ExactSizeIterator<Item = Option<&'a str>>,
        f: impl FnOnce(&IRow<'_>) -> R,
    ) -> Result<R, IRowError> {
        let n = fields.len();
        if n < 5 {
            return Err(IRowError::TooShort(n));
        }
        self.text.clear();
        let mut names: Vec<Name<'a>> = recycle(std::mem::take(&mut self.names));
        let result = self.read(fields, &mut names).map(|version| {
            let text = self.text.as_str();
            let get = |n: Name<'a>| -> &str { n.resolve(text) };
            let mut columns: Vec<&str> = recycle(std::mem::take(&mut self.columns));
            columns.extend(names[2..].iter().map(|&n| get(n)));
            let r = f(&IRow {
                report_type: get(names[0]),
                report_subtype: get(names[1]),
                version,
                columns: &columns,
            });
            (r, recycle(columns))
        });
        self.names = recycle(names);
        result.map(|(r, columns)| {
            self.columns = columns;
            r
        })
    }

    /// Fills `names` with the report type, the subtype and the columns
    /// (trailing empty ones dropped); returns the version.
    fn read<'a>(
        &mut self,
        fields: impl Iterator<Item = Option<&'a str>>,
        names: &mut Vec<Name<'a>>,
    ) -> Result<u16, IRowError> {
        let mut version = 0;
        for (index, field) in fields.enumerate().skip(1) {
            if index == 3 {
                version = parse_version(field.unwrap_or(""))?;
                continue;
            }
            // An empty subtype is a name (`I,GENCONDATA,,6,...`); an empty
            // column is one only at the end of the row.
            let blank = field.is_some_and(|s| s.trim().is_empty());
            let name = match field {
                Some(s) if is_canonical(s) => Name::Line(s),
                _ if blank && index >= 2 => Name::Empty,
                Some(s) => match normalize_name(s, &mut self.name) {
                    Some(t) => {
                        let start = self.text.len() as u32;
                        self.text.push_str(t);
                        Name::Arena(start, self.text.len() as u32)
                    }
                    None => {
                        return Err(IRowError::Name {
                            index,
                            raw: s.to_owned(),
                        });
                    }
                },
                None => {
                    return Err(IRowError::Name {
                        index,
                        raw: "(not UTF-8)".to_owned(),
                    });
                }
            };
            names.push(name);
        }
        while names.len() > 2 && matches!(names.last(), Some(Name::Empty)) {
            names.pop();
        }
        if let Some(i) = names[2..].iter().position(|n| matches!(n, Name::Empty)) {
            return Err(IRowError::EmptyColumn(i + 4));
        }
        Ok(version)
    }
}

/// Parses the version field of an `I` or `D` row.
pub fn parse_version(raw: &str) -> Result<u16, IRowError> {
    raw.trim()
        .parse()
        .map_err(|_| IRowError::Version(raw.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn irow_fingerprint_and_version() {
        let cols = ["SETTLEMENTDATE", "DUID", "SCADAVALUE"];
        let row = IRow {
            report_type: "DISPATCH",
            report_subtype: "UNIT_SCADA",
            version: 1,
            columns: &cols,
        };
        assert_eq!(row.fingerprint(), Fingerprint::of_names(cols));
        assert_eq!(parse_version(" 4 ").unwrap(), 4);
        assert!(matches!(parse_version("x"), Err(IRowError::Version(_))));
        assert!(matches!(parse_version("70000"), Err(IRowError::Version(_))));
        let e = IRowError::Name {
            index: 5,
            raw: String::new(),
        };
        assert!(e.to_string().contains("field 5"));
        assert!(IRowError::TooShort(2).to_string().contains('2'));
    }

    /// The owned fields of a header, for comparing.
    type Owned = (String, String, u16, Vec<String>);

    fn header_in(scratch: &mut IRowScratch, line: &str) -> Result<Owned, IRowError> {
        scratch.with_header(
            line.split(',').map(Some).collect::<Vec<_>>().into_iter(),
            |h| {
                (
                    h.report_type.to_owned(),
                    h.report_subtype.to_owned(),
                    h.version,
                    h.columns.iter().map(|c| (*c).to_owned()).collect(),
                )
            },
        )
    }

    fn header(line: &str) -> Result<Owned, IRowError> {
        header_in(&mut IRowScratch::default(), line)
    }

    #[test]
    fn parses_and_refuses_headers() {
        let (rt, st, v, cols) =
            header("I,dispatch,UNIT_SCADA, 2 ,settlementdate,\"DUID\",SCADAVALUE,,").unwrap();
        assert_eq!((rt.as_str(), st.as_str(), v), ("DISPATCH", "UNIT_SCADA", 2));
        assert_eq!(cols, ["SETTLEMENTDATE", "DUID", "SCADAVALUE"]);
        assert_eq!(header("I,GENCONDATA,,6,X").unwrap().1, "");
        assert_eq!(header("I,A,B,1").unwrap_err(), IRowError::TooShort(4));
        assert!(matches!(header("I,A,B,x,C"), Err(IRowError::Version(_))));
        assert!(matches!(
            header("I,,B,1,C"),
            Err(IRowError::Name { index: 1, .. })
        ));
        assert!(matches!(
            header("I,A,\u{1},1,C"),
            Err(IRowError::Name { index: 2, .. })
        ));
        assert!(matches!(
            header("I,A,B,1,C,\"\"\""),
            Err(IRowError::Name { index: 5, .. })
        ));
        assert_eq!(
            header("I,A,B,1,C,,D").unwrap_err(),
            IRowError::EmptyColumn(5)
        );
        let not_utf8 = IRowScratch::default().with_header(
            [Some("I"), Some("A"), Some("B"), Some("1"), None].into_iter(),
            |_| (),
        );
        assert!(not_utf8.unwrap_err().to_string().contains("not UTF-8"));
    }

    #[test]
    fn canonical_names_are_borrowed_and_buffers_are_reused() {
        let line = "I,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID";
        let fields: Vec<&str> = line.split(',').collect();
        let mut scratch = IRowScratch::default();
        let borrowed = scratch
            .with_header(fields.iter().map(|f| Some(*f)), |h| {
                h.columns
                    .iter()
                    .all(|c| line.as_bytes().as_ptr_range().contains(&c.as_ptr()))
            })
            .unwrap();
        assert!(borrowed, "canonical names point into the line");
        assert!(scratch.text.is_empty(), "nothing was copied");
        let cap = (scratch.names.capacity(), scratch.columns.capacity());
        assert!(cap.0 >= 4 && cap.1 >= 2);
        // A second row, one name to normalise: the same allocations, the
        // arena holds only that name, and an error leaves the buffers usable.
        let again = header_in(&mut scratch, "I,DISPATCH,unit_scada,2,A,B").unwrap();
        assert_eq!(again.1, "UNIT_SCADA");
        assert_eq!(scratch.text, "UNIT_SCADA");
        assert_eq!((scratch.names.capacity(), scratch.columns.capacity()), cap);
        // A longer row grows the list once; rows after it reuse it.
        assert!(header_in(&mut scratch, "I,A,B,1,C,,D").is_err());
        let grown = scratch.names.capacity();
        assert_eq!(header_in(&mut scratch, "I,A,B,1,C").unwrap().3, ["C"]);
        assert_eq!(scratch.names.capacity(), grown);
    }
}
