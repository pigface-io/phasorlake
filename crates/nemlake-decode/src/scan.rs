//! The line scanner: bytes in, `Sink` calls out.

use std::io::Read;

use memchr::memchr;
use nemlake_core::{IRow, IRowScratch};

use crate::error::{RowError, RowFailure, ScanError, SinkError};
use crate::fields::{Row, Splitter};

pub use crate::error::Pos;

/// What to do after a bad row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    /// Keep scanning.
    Continue,
    /// Stop now.
    Stop,
}

/// Receives the rows of one file.
pub trait Sink {
    /// A `C` row that is not the footer.
    fn comment(&mut self, row: &Row<'_>, pos: Pos) -> Result<(), SinkError>;
    /// An `I` row: a new block begins. `irow.columns` are normalised and
    /// trailing empty names are dropped.
    fn header(&mut self, irow: &IRow<'_>, pos: Pos) -> Result<(), SinkError>;
    /// A `D` row (fields 0..4 are the prefix, data starts at 4).
    fn row(&mut self, row: &Row<'_>, pos: Pos) -> Result<(), RowFailure>;
    /// A `D` row that [`Sink::row`] rejected, or a line of unknown kind.
    fn bad_row(&mut self, raw: &[u8], pos: Pos, err: &RowError) -> Flow;
    /// The footer `C,"END OF REPORT",n`, or end of input without one.
    fn footer(&mut self, declared: Option<u64>, lines_read: u64, pos: Pos)
    -> Result<(), SinkError>;
    /// The number of `D` rows of the block that just ended, in a scan of
    /// [`ScanOptions::headers_only`]; called before the next
    /// [`Sink::header`] and before [`Sink::footer`].
    fn block_rows(&mut self, _rows: u64) {}
}

/// Limits for a scan.
#[derive(Debug, Clone, Copy)]
pub struct ScanOptions {
    /// Longest line accepted; longer is a corrupt file.
    pub max_line_bytes: usize,
    /// Bad rows tolerated before the file fails.
    pub max_bad_rows: u64,
    /// Read only the I rows: a D row is counted against its block and
    /// never split or handed to [`Sink::row`]. What a census needs, at the
    /// speed of finding line ends.
    pub headers_only: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            max_line_bytes: 64 << 20,
            max_bad_rows: 0,
            headers_only: false,
        }
    }
}

/// Counts from one scan.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanStats {
    /// Lines, including header, headers and footer.
    pub lines: u64,
    /// Bytes consumed.
    pub bytes: u64,
    /// `I` rows.
    pub blocks: u64,
    /// `D` rows accepted.
    pub rows: u64,
    /// `D` rows rejected.
    pub bad_rows: u64,
    /// The footer's declared line count, when present.
    pub footer_declared: Option<u64>,
}

impl ScanStats {
    /// Whether the footer's count disagrees with the lines read.
    pub fn footer_mismatch(&self) -> bool {
        self.footer_declared.is_some_and(|d| d != self.lines)
    }
}

const BUF: usize = 1 << 20;

/// Scans `reader` to the end, delivering rows to `sink`.
pub fn scan<R: Read, S: Sink>(
    reader: R,
    sink: &mut S,
    opts: &ScanOptions,
) -> Result<ScanStats, ScanError> {
    scan_with(reader, sink, opts, &mut Vec::new())
}

/// [`scan`] reading through `buf`, which is grown to the read size once
/// and can be passed again for the next stream: a file of many entries,
/// or many files, reuses one buffer.
pub fn scan_with<R: Read, S: Sink>(
    mut reader: R,
    sink: &mut S,
    opts: &ScanOptions,
    buf: &mut Vec<u8>,
) -> Result<ScanStats, ScanError> {
    let want = BUF.min(opts.max_line_bytes.max(16));
    if buf.len() < want {
        buf.resize(want, 0);
    }
    let mut filled = 0usize;
    let mut stats = ScanStats::default();
    let mut splitter = Splitter::with_capacity(64);
    let mut irows = IRowScratch::default();
    let mut block_rows: u64 = 0;
    let mut seen_header = false;
    let mut eof = false;
    let mut line_start_offset: u64 = 0;
    // The first row that could not be read, so a refusal can say exactly
    // what was wrong with it.
    let mut first_bad: Option<(u64, Box<RowError>)> = None;

    while !eof || filled > 0 {
        // Fill.
        if !eof {
            if filled == buf.len() {
                if buf.len() >= opts.max_line_bytes {
                    return Err(ScanError::LineTooLong {
                        line: stats.lines + 1,
                        limit: opts.max_line_bytes,
                    });
                }
                buf.resize((buf.len() * 2).min(opts.max_line_bytes), 0);
            }
            match reader.read(&mut buf[filled..]) {
                Ok(0) => eof = true,
                Ok(n) => filled += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(source) => {
                    return Err(ScanError::Io {
                        bytes: stats.bytes,
                        source,
                    });
                }
            }
        }
        // Consume whole lines.
        let mut consumed = 0usize;
        loop {
            let rest = &buf[consumed..filled];
            let (line_end, next) = match memchr(b'\n', rest) {
                Some(i) => (i, i + 1),
                None if eof && !rest.is_empty() => (rest.len(), rest.len()),
                None => break,
            };
            let mut line = &rest[..line_end];
            if line.last() == Some(&b'\r') {
                line = &line[..line.len() - 1];
            }
            stats.lines += 1;
            let pos = Pos {
                line: stats.lines,
                byte_offset: line_start_offset,
            };
            let line_len = next as u64;
            line_start_offset += line_len;
            stats.bytes += line_len;
            consumed += next;

            match line.first() {
                Some(b'D') => {
                    if !seen_header {
                        return Err(ScanError::DataBeforeHeader { line: pos.line });
                    }
                    if opts.headers_only {
                        block_rows += 1;
                        stats.rows += 1;
                        continue;
                    }
                    let row = splitter.split(line);
                    match sink.row(&row, pos) {
                        Ok(()) => stats.rows += 1,
                        Err(RowFailure::Sink(e)) => return Err(ScanError::Sink(*e)),
                        Err(RowFailure::Row(e)) => {
                            on_bad_row(sink, &mut stats, &mut first_bad, opts, line, pos, e)?;
                        }
                    }
                }
                Some(b'I') => {
                    if opts.headers_only && seen_header {
                        sink.block_rows(block_rows);
                        block_rows = 0;
                    }
                    let row = splitter.split(line);
                    irows
                        .with_header((0..row.len()).map(|i| row.str(i)), |irow| {
                            sink.header(irow, pos)
                        })
                        .map_err(|e| ScanError::BadHeader {
                            line: pos.line,
                            message: e.to_string(),
                        })??;
                    seen_header = true;
                    stats.blocks += 1;
                }
                Some(b'C') => {
                    let row = splitter.split(line);
                    if row.get(1) == Some(b"END OF REPORT") {
                        stats.footer_declared = row.str(2).and_then(|s| s.trim().parse().ok());
                    } else {
                        sink.comment(&row, pos)?;
                    }
                }
                None => {}
                Some(&b) => {
                    let e = Box::new(RowError::UnknownRecord(char::from(b)));
                    on_bad_row(sink, &mut stats, &mut first_bad, opts, line, pos, e)?;
                }
            }
        }
        // Keep the partial tail.
        if consumed > 0 {
            buf.copy_within(consumed..filled, 0);
            filled -= consumed;
        } else if eof {
            break;
        }
    }
    if opts.headers_only && seen_header {
        sink.block_rows(block_rows);
    }
    sink.footer(
        stats.footer_declared,
        stats.lines,
        Pos {
            line: stats.lines,
            byte_offset: line_start_offset,
        },
    )?;
    Ok(stats)
}

/// Counts a row that could not be read and hands it to the sink; an error
/// when the sink stops or there are more such rows than `opts` allows.
#[cold]
fn on_bad_row<S: Sink>(
    sink: &mut S,
    stats: &mut ScanStats,
    first_bad: &mut Option<(u64, Box<RowError>)>,
    opts: &ScanOptions,
    line: &[u8],
    pos: Pos,
    e: Box<RowError>,
) -> Result<(), ScanError> {
    stats.bad_rows += 1;
    if sink.bad_row(line, pos, &e) == Flow::Stop {
        return Err(ScanError::Stopped { line: pos.line });
    }
    let (first_line, first) = first_bad.get_or_insert((pos.line, e));
    if stats.bad_rows > opts.max_bad_rows {
        return Err(ScanError::TooManyBadRows {
            bad: stats.bad_rows,
            limit: opts.max_bad_rows,
            line: *first_line,
            first: first.clone(),
        });
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Collects everything, as owned strings.
    #[derive(Default, Debug)]
    pub(crate) struct Collect {
        pub comments: Vec<Vec<String>>,
        pub headers: Vec<(String, String, u16, Vec<String>)>,
        pub rows: Vec<Vec<String>>,
        pub bad: Vec<(u64, String)>,
        pub footer: Option<(Option<u64>, u64)>,
        pub reject_rows_with: Option<usize>,
        pub stop_on_bad: bool,
        pub block_rows: Vec<u64>,
    }

    impl Sink for Collect {
        fn comment(&mut self, row: &Row<'_>, _: Pos) -> Result<(), SinkError> {
            self.comments.push(
                row.iter()
                    .map(|f| String::from_utf8_lossy(f).into_owned())
                    .collect(),
            );
            Ok(())
        }
        fn header(&mut self, irow: &IRow<'_>, _: Pos) -> Result<(), SinkError> {
            self.headers.push((
                irow.report_type.to_owned(),
                irow.report_subtype.to_owned(),
                irow.version,
                irow.columns.iter().map(|c| (*c).to_owned()).collect(),
            ));
            Ok(())
        }
        fn row(&mut self, row: &Row<'_>, _: Pos) -> Result<(), RowFailure> {
            if let Some(n) = self.reject_rows_with
                && row.len() == n
            {
                return Err(RowError::FieldCount {
                    declared: 0,
                    observed: n,
                }
                .into());
            }
            self.rows.push(
                row.iter()
                    .map(|f| String::from_utf8_lossy(f).into_owned())
                    .collect(),
            );
            Ok(())
        }
        fn block_rows(&mut self, rows: u64) {
            self.block_rows.push(rows);
        }
        fn bad_row(&mut self, raw: &[u8], pos: Pos, _: &RowError) -> Flow {
            self.bad
                .push((pos.line, String::from_utf8_lossy(raw).into_owned()));
            if self.stop_on_bad {
                Flow::Stop
            } else {
                Flow::Continue
            }
        }
        fn footer(&mut self, declared: Option<u64>, lines: u64, _: Pos) -> Result<(), SinkError> {
            self.footer = Some((declared, lines));
            Ok(())
        }
    }

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!(
            "{}/../../testdata/csv/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    #[test]
    fn scans_region_fixture() {
        let mut c = Collect::default();
        let stats = scan(
            fixture("REGION_v1_head.csv").as_slice(),
            &mut c,
            &ScanOptions::default(),
        )
        .unwrap();
        assert_eq!(stats.lines, 9);
        assert_eq!(stats.blocks, 1);
        assert_eq!(stats.rows, 6);
        assert_eq!(stats.bad_rows, 0);
        assert_eq!(stats.footer_declared, Some(9));
        assert!(!stats.footer_mismatch());
        assert_eq!(c.comments.len(), 1);
        assert_eq!(c.comments[0][2], "DVD_REGION");
        assert_eq!(c.headers[0].0, "MARKET_CONFIG");
        assert_eq!(
            c.headers[0].3,
            ["REGIONID", "DESCRIPTION", "REGIONSTATUS", "LASTCHANGED"]
        );
        assert_eq!(
            c.rows[0][4..],
            ["QLD1", "Queensland Region", "Active", "2004/06/11 10:43:05"]
        );
        assert_eq!(c.footer, Some((Some(9), 9)));
    }

    #[test]
    fn headers_only_reads_every_block_and_counts_its_rows() {
        let mut c = Collect::default();
        let stats = scan(
            fixture("multi_block_dispatchis.csv").as_slice(),
            &mut c,
            &ScanOptions {
                headers_only: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(stats.blocks, 7);
        assert_eq!(c.headers.len(), 7);
        assert!(c.rows.is_empty(), "no D row is split or handed over");
        assert_eq!(c.block_rows.len(), 7);
        assert_eq!(c.block_rows.iter().sum::<u64>(), stats.rows);
        assert!(!stats.footer_mismatch());
        // A D row before any I row is still an error.
        assert!(matches!(
            scan(
                b"D,A,B,1,x\n".as_slice(),
                &mut Collect::default(),
                &ScanOptions {
                    headers_only: true,
                    ..Default::default()
                },
            ),
            Err(ScanError::DataBeforeHeader { line: 1 })
        ));
    }

    #[test]
    fn scans_multi_block_and_every_fixture() {
        let mut c = Collect::default();
        let stats = scan(
            fixture("multi_block_dispatchis.csv").as_slice(),
            &mut c,
            &ScanOptions::default(),
        )
        .unwrap();
        assert_eq!(stats.blocks, 7);
        assert_eq!(c.headers[2].1, "PRICE");
        assert_eq!(c.headers[2].2, 5);
        let dir = format!("{}/../../testdata/csv", env!("CARGO_MANIFEST_DIR"));
        for e in std::fs::read_dir(dir).unwrap().filter_map(Result::ok) {
            if e.path().extension().is_some_and(|x| x == "csv") {
                let mut c = Collect::default();
                let stats = scan(
                    std::fs::read(e.path()).unwrap().as_slice(),
                    &mut c,
                    &ScanOptions::default(),
                )
                .unwrap();
                assert!(!stats.footer_mismatch(), "{}", e.path().display());
                assert_eq!(stats.bad_rows, 0, "{}", e.path().display());
            }
        }
    }

    #[test]
    fn small_buffer_boundaries_and_no_trailing_newline() {
        // Reader that yields one byte at a time, exercising the carry-over path.
        struct OneByte<'a>(&'a [u8]);
        impl Read for OneByte<'_> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if self.0.is_empty() {
                    return Ok(0);
                }
                buf[0] = self.0[0];
                self.0 = &self.0[1..];
                Ok(1)
            }
        }
        let text = b"C,x\nI,A,B,1,X,Y\nD,A,B,1,1,2\nD,A,B,1,3,4";
        let mut c = Collect::default();
        let stats = scan(OneByte(text), &mut c, &ScanOptions::default()).unwrap();
        assert_eq!(stats.lines, 4);
        assert_eq!(stats.rows, 2);
        assert_eq!(c.rows[1][5], "4");
        assert_eq!(stats.footer_declared, None);
        assert_eq!(c.footer, Some((None, 4)));
        assert_eq!(stats.bytes, text.len() as u64);
    }

    #[test]
    fn crlf_blank_lines_and_trailing_commas() {
        let text = b"I,A,B,1,X,Y,\r\n\r\nD,A,B,1,1,2,\r\nC,\"END OF REPORT\",4\r\n";
        let mut c = Collect::default();
        let stats = scan(text.as_slice(), &mut c, &ScanOptions::default()).unwrap();
        assert_eq!(c.headers[0].3, ["X", "Y"], "trailing empty name dropped");
        assert_eq!(
            c.rows[0].len(),
            7,
            "trailing empty field kept for the sink to judge"
        );
        assert_eq!(stats.lines, 4);
        assert!(!stats.footer_mismatch());
    }

    #[test]
    fn empty_subtype_is_a_name() {
        // Standing-data tables such as GENCONDATA publish no subtype.
        let mut c = Collect::default();
        let stats = scan(
            b"I,GENCONDATA,,6,GENCONID\nD,GENCONDATA,,6,X1\n".as_slice(),
            &mut c,
            &ScanOptions::default(),
        )
        .unwrap();
        assert_eq!(c.headers[0].0, "GENCONDATA");
        assert_eq!(c.headers[0].1, "");
        assert_eq!(c.headers[0].2, 6);
        assert_eq!(stats.lines, 2);
        // A quoted empty subtype is the same thing.
        let mut c = Collect::default();
        scan(
            b"I,GENCONDATA,\"\",6,GENCONID\n".as_slice(),
            &mut c,
            &ScanOptions::default(),
        )
        .unwrap();
        assert_eq!(c.headers[0].1, "");
    }

    #[test]
    fn errors() {
        let mut c = Collect::default();
        assert!(matches!(
            scan(b"D,A,B,1,x\n".as_slice(), &mut c, &ScanOptions::default()),
            Err(ScanError::DataBeforeHeader { line: 1 })
        ));
        for bad in [
            "I,A,B\n",
            "I,A,B,x,C\n",
            "I,A,B,1,C,,D\n",
            "I,,B,1,C\n",
            "I,A,B,1,\"\"\"\n",
        ] {
            let mut c = Collect::default();
            let e = scan(bad.as_bytes(), &mut c, &ScanOptions::default()).unwrap_err();
            assert!(
                matches!(e, ScanError::BadHeader { line: 1, .. }),
                "{bad}: {e}"
            );
        }
        let mut c = Collect::default();
        let e = scan(
            b"I,A,B,1,X\nxyz\n".as_slice(),
            &mut c,
            &ScanOptions::default(),
        )
        .unwrap_err();
        assert!(
            matches!(
                e,
                ScanError::TooManyBadRows {
                    bad: 1,
                    limit: 0,
                    line: 2,
                    ..
                }
            ),
            "{e}"
        );
        let mut c = Collect {
            stop_on_bad: true,
            ..Default::default()
        };
        let e = scan(
            b"I,A,B,1,X\nxyz\n".as_slice(),
            &mut c,
            &ScanOptions::default(),
        )
        .unwrap_err();
        assert!(matches!(e, ScanError::Stopped { line: 2 }), "{e}");
        let mut c = Collect {
            reject_rows_with: Some(5),
            ..Default::default()
        };
        let stats = scan(
            b"I,A,B,1,X\nD,A,B,1,1\nD,A,B,1,1,2\n".as_slice(),
            &mut c,
            &ScanOptions {
                max_bad_rows: 5,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(stats.bad_rows, 1);
        assert_eq!(stats.rows, 1);
        assert_eq!(c.bad, vec![(2, "D,A,B,1,1".to_owned())]);
        let mut c = Collect {
            reject_rows_with: Some(5),
            stop_on_bad: true,
            ..Default::default()
        };
        assert!(matches!(
            scan(
                b"I,A,B,1,X\nD,A,B,1,1\n".as_slice(),
                &mut c,
                &ScanOptions::default()
            ),
            Err(ScanError::Stopped { line: 2 })
        ));
        let long = vec![b'x'; 100];
        let mut c = Collect::default();
        let e = scan(
            long.as_slice(),
            &mut c,
            &ScanOptions {
                max_line_bytes: 16,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(matches!(e, ScanError::LineTooLong { .. }), "{e}");
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("disk"))
            }
        }
        let mut c = Collect::default();
        assert!(matches!(
            scan(Broken, &mut c, &ScanOptions::default()),
            Err(ScanError::Io { .. })
        ));
        struct Interrupt(u8);
        impl Read for Interrupt {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                self.0 += 1;
                match self.0 {
                    1 => Err(std::io::Error::from(std::io::ErrorKind::Interrupted)),
                    2 => {
                        buf[..4].copy_from_slice(b"C,a\n");
                        Ok(4)
                    }
                    _ => Ok(0),
                }
            }
        }
        let mut c = Collect::default();
        assert_eq!(
            scan(Interrupt(0), &mut c, &ScanOptions::default())
                .unwrap()
                .lines,
            1
        );
    }
}
