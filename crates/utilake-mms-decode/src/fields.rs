//! Splitting one line into fields without allocating per row.
//!
//! Most MMS lines contain quotes (timestamps are always quoted), so the
//! quote-aware path has to be fast too. A line without quotes is split with
//! `memchr_iter`; a line with quotes is walked once, and only a field with a
//! doubled quote (`""`) is unescaped into a scratch buffer the splitter
//! reuses across rows. Field boundaries go into a reusable `Vec<Span>`.

use memchr::{memchr, memchr_iter};

/// Fields AEMO always writes before the data: `D`, report type, subtype,
/// version.
pub const PREFIX_FIELDS: usize = 4;

/// Where a field's bytes live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    start: u32,
    end: u32,
    /// `false`: into the line; `true`: into the scratch buffer.
    scratch: bool,
}

/// Reusable split state.
#[derive(Debug, Default)]
pub struct Splitter {
    spans: Vec<Span>,
    scratch: Vec<u8>,
}

/// One split line: the fields of a row.
#[derive(Debug, Clone, Copy)]
pub struct Row<'a> {
    line: &'a [u8],
    scratch: &'a [u8],
    spans: &'a [Span],
}

impl Splitter {
    /// A splitter with room for `fields` fields.
    pub fn with_capacity(fields: usize) -> Self {
        Self {
            spans: Vec::with_capacity(fields),
            scratch: Vec::with_capacity(256),
        }
    }

    /// Splits `line` (no trailing newline) into fields.
    pub fn split<'a>(&'a mut self, line: &'a [u8]) -> Row<'a> {
        self.spans.clear();
        self.scratch.clear();
        if memchr(b'"', line).is_none() {
            let mut start = 0u32;
            for i in memchr_iter(b',', line) {
                self.spans.push(Span {
                    start,
                    end: i as u32,
                    scratch: false,
                });
                start = i as u32 + 1;
            }
            self.spans.push(Span {
                start,
                end: line.len() as u32,
                scratch: false,
            });
        } else {
            self.split_quoted(line);
        }
        Row {
            line,
            scratch: &self.scratch,
            spans: &self.spans,
        }
    }

    fn split_quoted(&mut self, line: &[u8]) {
        let mut i = 0usize;
        let n = line.len();
        loop {
            if i < n && line[i] == b'"' {
                // Quoted field: find the closing quote, unescaping "" if present.
                let content_start = i + 1;
                let mut j = content_start;
                let mut escaped = false;
                loop {
                    match memchr(b'"', &line[j..]) {
                        None => {
                            // Unterminated: take the rest as the field.
                            j = n;
                            break;
                        }
                        Some(off) => {
                            let q = j + off;
                            if line.get(q + 1) == Some(&b'"') {
                                escaped = true;
                                j = q + 2;
                            } else {
                                j = q;
                                break;
                            }
                        }
                    }
                }
                if escaped {
                    let s = self.scratch.len();
                    let mut k = content_start;
                    while k < j {
                        if line[k] == b'"' && line.get(k + 1) == Some(&b'"') {
                            self.scratch.push(b'"');
                            k += 2;
                        } else {
                            self.scratch.push(line[k]);
                            k += 1;
                        }
                    }
                    self.spans.push(Span {
                        start: s as u32,
                        end: self.scratch.len() as u32,
                        scratch: true,
                    });
                } else {
                    self.spans.push(Span {
                        start: content_start as u32,
                        end: j as u32,
                        scratch: false,
                    });
                }
                // Skip the closing quote and anything up to the next comma.
                i = (j + 1).min(n);
                match memchr(b',', &line[i..]) {
                    Some(off) => i += off + 1,
                    None => return,
                }
            } else {
                match memchr(b',', &line[i..]) {
                    Some(off) => {
                        self.spans.push(Span {
                            start: i as u32,
                            end: (i + off) as u32,
                            scratch: false,
                        });
                        i += off + 1;
                    }
                    None => {
                        self.spans.push(Span {
                            start: i as u32,
                            end: n as u32,
                            scratch: false,
                        });
                        return;
                    }
                }
            }
            if i == n && line[n - 1] == b',' {
                // A trailing comma means one more, empty, field.
                self.spans.push(Span {
                    start: n as u32,
                    end: n as u32,
                    scratch: false,
                });
                return;
            }
        }
    }
}

impl<'a> Row<'a> {
    /// Number of fields.
    pub fn len(&self) -> usize {
        self.spans.len()
    }

    /// Whether the row has no fields (never, for a non-empty line).
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    /// The bytes of field `i`.
    #[inline]
    pub fn get(&self, i: usize) -> Option<&'a [u8]> {
        let s = self.spans.get(i)?;
        let src = if s.scratch { self.scratch } else { self.line };
        Some(&src[s.start as usize..s.end as usize])
    }

    /// Field `i` as text, if it is UTF-8.
    pub fn str(&self, i: usize) -> Option<&'a str> {
        self.get(i).and_then(|b| std::str::from_utf8(b).ok())
    }

    /// The whole line.
    #[cfg(test)]
    pub(crate) const fn line(&self) -> &'a [u8] {
        self.line
    }

    /// Every field.
    pub fn iter(&self) -> impl Iterator<Item = &'a [u8]> + '_ {
        (0..self.len()).filter_map(move |i| self.get(i))
    }

    /// The number of fields once trailing empty ones are dropped.
    pub fn len_trimmed(&self) -> usize {
        let mut n = self.len();
        while n > 0 && self.get(n - 1).is_some_and(<[u8]>::is_empty) {
            n -= 1;
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(line: &[u8]) -> Vec<String> {
        let mut s = Splitter::default();
        let r = s.split(line);
        r.iter()
            .map(|f| String::from_utf8_lossy(f).into_owned())
            .collect()
    }

    #[test]
    fn unquoted() {
        assert_eq!(fields(b"D,a,b,c"), ["D", "a", "b", "c"]);
        assert_eq!(fields(b"a,,b,"), ["a", "", "b", ""]);
        assert_eq!(fields(b""), [""]);
        assert_eq!(fields(b","), ["", ""]);
    }

    #[test]
    fn quoted() {
        assert_eq!(
            fields(br#"D,"2024/01/01 00:05:00",BUTLERSG,9.199999"#),
            ["D", "2024/01/01 00:05:00", "BUTLERSG", "9.199999"]
        );
        assert_eq!(fields(br#"a,"b, c",d"#), ["a", "b, c", "d"]);
        assert_eq!(
            fields(br#"a,"he said ""hi""",d"#),
            ["a", "he said \"hi\"", "d"]
        );
        assert_eq!(fields(br#""only""#), ["only"]);
        assert_eq!(fields(br#"a,"""#), ["a", ""]);
        assert_eq!(fields(br#"a,"b","#), ["a", "b", ""]);
        assert_eq!(fields(br#""a","b""#), ["a", "b"]);
        assert_eq!(fields(br#"x,"a""b""c",y"#), ["x", "a\"b\"c", "y"]);
        assert_eq!(
            fields(br#"C,"END OF REPORT",9"#),
            ["C", "END OF REPORT", "9"]
        );
    }

    #[test]
    fn unterminated_quote_takes_rest() {
        let mut s = Splitter::default();
        let r = s.split(br#"a,"open,b"#);
        assert_eq!(r.len(), 2);
        assert_eq!(r.get(1), Some(b"open,b".as_slice()));
    }

    #[test]
    fn accessors() {
        let mut s = Splitter::with_capacity(4);
        let r = s.split(b"D,x,\xff,y,,");
        assert_eq!(r.len(), 6);
        assert!(!r.is_empty());
        assert_eq!(r.str(1), Some("x"));
        assert_eq!(r.str(2), None);
        assert_eq!(r.get(9), None);
        assert_eq!(r.line(), b"D,x,\xff,y,,");
        assert_eq!(r.len_trimmed(), 4);
        let r = s.split(b",,");
        assert_eq!(r.len_trimmed(), 0);
    }
}
