//! A parser for the subset of Oracle SQL*Loader control-file syntax AEMO
//! uses in its monthly archives.
//!
//! ```text
//! LOAD DATA
//! INFILE PUBLIC_DVD_DISPATCH_UNIT_SCADA_202401010000.CSV
//! APPEND INTO TABLE DISPATCH_UNIT_SCADA
//! WHEN (1:1) = 'D'
//! FIELDS TERMINATED BY ','
//! OPTIONALLY ENCLOSED BY '"'
//! TRAILING NULLCOLS
//! (row_type FILLER,
//! report_type FILLER,
//! report_subtype FILLER,
//! report_version FILLER,
//! SETTLEMENTDATE DATE "yyyy/mm/dd hh24:mi:ss",
//! DUID CHAR(20),
//! SCADAVALUE FLOAT EXTERNAL)
//! ```

use nemlake_core::{Category, Diagnose, Diagnostic};

use crate::file_model::model::LakeType;

/// SHA-256 of a control file's bytes, lower-case hex: what an increment
/// records of the file it keeps.
pub fn sha256(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    use std::fmt::Write as _;
    sha2::Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// A SQL*Loader column datatype.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoaderType {
    /// `FILLER`: present in the file, not loaded (the row prefix).
    Filler,
    /// `CHAR(n)`.
    Char(u32),
    /// `DATE "fmt"`.
    Date(String),
    /// `TIMESTAMP "fmt"`, `TIMESTAMP(n) "fmt"` or bare `TIMESTAMP(n)`.
    Timestamp(Option<String>),
    /// `FLOAT EXTERNAL`.
    FloatExternal,
    /// `INTEGER EXTERNAL`.
    IntegerExternal,
    /// `DECIMAL EXTERNAL`.
    DecimalExternal,
}

impl LoaderType {
    /// The lake type a column this loader type brings in starts with;
    /// `None` for a filler. Every number is a `double`; a date with an
    /// hour is a `timestamp_ns`.
    pub fn default_type(&self) -> Option<LakeType> {
        Some(match self {
            Self::Filler => return None,
            Self::Char(_) => LakeType::String,
            Self::FloatExternal | Self::DecimalExternal | Self::IntegerExternal => LakeType::Double,
            Self::Date(f) | Self::Timestamp(Some(f)) => {
                if f.to_ascii_lowercase().contains("hh") {
                    LakeType::TimestampNs
                } else {
                    LakeType::Date
                }
            }
            Self::Timestamp(None) => LakeType::TimestampNs,
        })
    }

    /// The class of type the loader type loads, for a shape's canonical
    /// form; `None` for a filler.
    pub fn type_class(&self) -> Option<nemlake_core::TypeClass> {
        use nemlake_core::TypeClass;
        Some(match self {
            Self::Filler => return None,
            Self::Char(_) => TypeClass::Varchar,
            Self::FloatExternal | Self::DecimalExternal => TypeClass::Number,
            Self::IntegerExternal => TypeClass::Integer,
            Self::Date(_) | Self::Timestamp(_) => match self.default_type() {
                Some(LakeType::Date) => TypeClass::Date,
                _ => TypeClass::Timestamp,
            },
        })
    }

    /// The type as written in a CTL, for the `loader` field of a schema.
    pub fn render(&self) -> String {
        match self {
            Self::Filler => "FILLER".into(),
            Self::Char(n) => format!("CHAR({n})"),
            Self::Date(f) => format!("DATE \"{f}\""),
            Self::Timestamp(Some(f)) => format!("TIMESTAMP \"{f}\""),
            Self::Timestamp(None) => "TIMESTAMP".into(),
            Self::FloatExternal => "FLOAT EXTERNAL".into(),
            Self::IntegerExternal => "INTEGER EXTERNAL".into(),
            Self::DecimalExternal => "DECIMAL EXTERNAL".into(),
        }
    }
}

/// One column clause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CtlColumn {
    /// Column name as written.
    pub name: String,
    /// Its loader type.
    pub ty: LoaderType,
}

/// A parsed control file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CtlFile {
    /// `INFILE` name.
    pub infile: String,
    /// `INTO TABLE` name.
    pub table: String,
    /// Columns in file order, fillers included.
    pub columns: Vec<CtlColumn>,
}

impl CtlFile {
    /// The loaded (non-filler) columns.
    pub fn data_columns(&self) -> impl Iterator<Item = &CtlColumn> {
        self.columns.iter().filter(|c| c.ty != LoaderType::Filler)
    }

    /// The loaded columns' names, upper-case, in file order.
    pub fn column_names(&self) -> Vec<String> {
        self.data_columns()
            .map(|c| c.name.to_ascii_uppercase())
            .collect()
    }

    /// Whether `other` loads the same columns, in the same order, with the
    /// same types. Two months' files of one table differ in their title and
    /// file name even when nothing else changed; only a layout that differs
    /// starts a new increment.
    pub fn same_layout(&self, other: &Self) -> bool {
        let layout = |f: &Self| -> Vec<(String, String)> {
            f.data_columns()
                .map(|c| {
                    (
                        c.name.to_ascii_uppercase(),
                        c.ty.render().to_ascii_uppercase(),
                    )
                })
                .collect()
        };
        layout(self) == layout(other)
    }
}

/// Why a control file could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{path}:{line}:{col}: {message}")]
pub struct CtlError {
    /// The file.
    pub path: String,
    /// 1-based line.
    pub line: u64,
    /// 1-based column.
    pub col: u64,
    /// What was wrong, naming the token.
    pub message: String,
    /// The offending text.
    pub sample: String,
}

impl Diagnose for CtlError {
    fn diagnostic(&self) -> Diagnostic {
        Diagnostic::error("schema.ctl_parse", Category::Schema, self.to_string())
            .with_file(self.path.clone())
            .at(self.line, 0)
            .with_sample(self.sample.as_bytes())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Word(String),
    Quoted(String),
    LParen,
    RParen,
    Comma,
    Equals,
    Colon,
    Number(String),
}

struct Lexer<'a> {
    path: &'a str,
    src: &'a str,
    pos: usize,
    line: u64,
    line_start: usize,
}

impl<'a> Lexer<'a> {
    fn err(&self, message: impl Into<String>) -> CtlError {
        let end = self.src[self.pos..]
            .find('\n')
            .map_or(self.src.len(), |i| self.pos + i);
        CtlError {
            path: self.path.to_owned(),
            line: self.line,
            col: (self.pos - self.line_start) as u64 + 1,
            message: message.into(),
            sample: self.src[self.line_start..end].trim_end().to_owned(),
        }
    }

    fn next(&mut self) -> Result<Option<(Tok, u64)>, CtlError> {
        let b = self.src.as_bytes();
        loop {
            while self.pos < b.len() && (b[self.pos] as char).is_ascii_whitespace() {
                if b[self.pos] == b'\n' {
                    self.line += 1;
                    self.line_start = self.pos + 1;
                }
                self.pos += 1;
            }
            if self.pos >= b.len() {
                return Ok(None);
            }
            if b[self.pos] == b'-' && b.get(self.pos + 1) == Some(&b'-') {
                while self.pos < b.len() && b[self.pos] != b'\n' {
                    self.pos += 1;
                }
                continue;
            }
            break;
        }
        let line = self.line;
        let c = b[self.pos];
        let tok = match c {
            b'(' => {
                self.pos += 1;
                Tok::LParen
            }
            b')' => {
                self.pos += 1;
                Tok::RParen
            }
            b',' => {
                self.pos += 1;
                Tok::Comma
            }
            b'=' => {
                self.pos += 1;
                Tok::Equals
            }
            b':' => {
                self.pos += 1;
                Tok::Colon
            }
            b'"' | b'\'' => {
                let start = self.pos + 1;
                let end = self.src[start..]
                    .find(c as char)
                    .ok_or_else(|| self.err("unterminated quoted string"))?
                    + start;
                self.pos = end + 1;
                Tok::Quoted(self.src[start..end].to_owned())
            }
            b'0'..=b'9' => {
                let start = self.pos;
                while self.pos < b.len() && b[self.pos].is_ascii_digit() {
                    self.pos += 1;
                }
                Tok::Number(self.src[start..self.pos].to_owned())
            }
            c if c.is_ascii_alphabetic() || c == b'_' || c == b'.' => {
                let start = self.pos;
                while self.pos < b.len()
                    && (b[self.pos].is_ascii_alphanumeric()
                        || matches!(b[self.pos], b'_' | b'.' | b'$' | b'#'))
                {
                    self.pos += 1;
                }
                Tok::Word(self.src[start..self.pos].to_owned())
            }
            other => return Err(self.err(format!("unexpected character {:?}", other as char))),
        };
        Ok(Some((tok, line)))
    }
}

struct Parser<'a> {
    lex: Lexer<'a>,
    peeked: Option<Option<(Tok, u64)>>,
}

impl Parser<'_> {
    fn peek(&mut self) -> Result<Option<&Tok>, CtlError> {
        if self.peeked.is_none() {
            self.peeked = Some(self.lex.next()?);
        }
        Ok(self
            .peeked
            .as_ref()
            .and_then(|p| p.as_ref())
            .map(|(t, _)| t))
    }

    fn next(&mut self) -> Result<Option<Tok>, CtlError> {
        match self.peeked.take() {
            Some(p) => Ok(p.map(|(t, _)| t)),
            None => Ok(self.lex.next()?.map(|(t, _)| t)),
        }
    }

    fn expect_word(&mut self, what: &str) -> Result<String, CtlError> {
        match self.next()? {
            Some(Tok::Word(w)) => Ok(w),
            Some(other) => Err(self.lex.err(format!("expected {what}, found {other:?}"))),
            None => Err(self.lex.err(format!("expected {what}, found end of file"))),
        }
    }

    fn expect_keyword(&mut self, kw: &str) -> Result<(), CtlError> {
        let w = self.expect_word(kw)?;
        if w.eq_ignore_ascii_case(kw) {
            Ok(())
        } else {
            Err(self.lex.err(format!("expected {kw}, found {w}")))
        }
    }

    fn expect(&mut self, tok: &Tok) -> Result<(), CtlError> {
        match self.next()? {
            Some(ref t) if t == tok => Ok(()),
            Some(other) => Err(self.lex.err(format!("expected {tok:?}, found {other:?}"))),
            None => Err(self.lex.err(format!("expected {tok:?}, found end of file"))),
        }
    }

    fn word_is(&mut self, kw: &str) -> Result<bool, CtlError> {
        Ok(matches!(self.peek()?, Some(Tok::Word(w)) if w.eq_ignore_ascii_case(kw)))
    }

    fn file(&mut self) -> Result<CtlFile, CtlError> {
        self.expect_keyword("LOAD")?;
        self.expect_keyword("DATA")?;
        self.expect_keyword("INFILE")?;
        let infile = match self.next()? {
            Some(Tok::Word(w) | Tok::Quoted(w)) => w,
            other => {
                return Err(self.lex.err(format!(
                    "expected a file name after INFILE, found {other:?}"
                )));
            }
        };
        // APPEND | INSERT | REPLACE | TRUNCATE
        let mode = self.expect_word("a load mode such as APPEND")?;
        if !["APPEND", "INSERT", "REPLACE", "TRUNCATE"]
            .iter()
            .any(|m| mode.eq_ignore_ascii_case(m))
        {
            return Err(self.lex.err(format!(
                "unsupported load mode {mode}; AEMO files use APPEND"
            )));
        }
        self.expect_keyword("INTO")?;
        self.expect_keyword("TABLE")?;
        let table = self.expect_word("a table name")?;
        // Optional clauses until the column list.
        loop {
            match self.peek()? {
                Some(Tok::LParen) => break,
                Some(Tok::Word(w)) => {
                    let w = w.clone();
                    if w.eq_ignore_ascii_case("WHEN") {
                        self.next()?;
                        self.expect(&Tok::LParen)?;
                        self.next()?; // 1
                        self.expect(&Tok::Colon)?;
                        self.next()?; // 1
                        self.expect(&Tok::RParen)?;
                        self.expect(&Tok::Equals)?;
                        match self.next()? {
                            Some(Tok::Quoted(_)) => {}
                            other => {
                                return Err(self.lex.err(format!(
                                    "expected a quoted row type after WHEN (1:1) =, found {other:?}"
                                )));
                            }
                        }
                    } else if w.eq_ignore_ascii_case("FIELDS") {
                        self.next()?;
                        self.expect_keyword("TERMINATED")?;
                        self.expect_keyword("BY")?;
                        self.next()?;
                    } else if w.eq_ignore_ascii_case("OPTIONALLY") {
                        self.next()?;
                        self.expect_keyword("ENCLOSED")?;
                        self.expect_keyword("BY")?;
                        self.next()?;
                    } else if w.eq_ignore_ascii_case("TRAILING") {
                        self.next()?;
                        self.expect_keyword("NULLCOLS")?;
                    } else {
                        return Err(self
                            .lex
                            .err(format!("unsupported clause {w} before the column list")));
                    }
                }
                other => {
                    let msg = format!("expected a clause or the column list, found {other:?}");
                    return Err(self.lex.err(msg));
                }
            }
        }
        self.expect(&Tok::LParen)?;
        let mut columns = Vec::new();
        loop {
            let name = self.expect_word("a column name")?;
            let ty = self.column_type()?;
            columns.push(CtlColumn { name, ty });
            match self.next()? {
                Some(Tok::Comma) => {}
                Some(Tok::RParen) => break,
                other => {
                    return Err(self
                        .lex
                        .err(format!("expected , or ) after a column, found {other:?}")));
                }
            }
        }
        Ok(CtlFile {
            infile,
            table,
            columns,
        })
    }

    fn column_type(&mut self) -> Result<LoaderType, CtlError> {
        let w = self.expect_word("a column datatype")?;
        let ty = match w.to_ascii_uppercase().as_str() {
            "FILLER" => LoaderType::Filler,
            "CHAR" => {
                self.expect(&Tok::LParen)?;
                let n = match self.next()? {
                    Some(Tok::Number(n)) => n
                        .parse::<u32>()
                        .map_err(|_| self.lex.err(format!("CHAR length {n} is not a number")))?,
                    other => {
                        return Err(self
                            .lex
                            .err(format!("expected a length after CHAR(, found {other:?}")));
                    }
                };
                self.expect(&Tok::RParen)?;
                LoaderType::Char(n)
            }
            "DATE" => LoaderType::Date(self.format_string("DATE")?),
            "TIMESTAMP" => {
                if self.peek()? == Some(&Tok::LParen) {
                    self.next()?;
                    self.next()?;
                    self.expect(&Tok::RParen)?;
                }
                match self.peek()? {
                    Some(Tok::Quoted(_)) => {
                        LoaderType::Timestamp(Some(self.format_string("TIMESTAMP")?))
                    }
                    _ => LoaderType::Timestamp(None),
                }
            }
            "FLOAT" | "INTEGER" | "DECIMAL" => {
                self.expect_keyword("EXTERNAL")?;
                match w.to_ascii_uppercase().as_str() {
                    "FLOAT" => LoaderType::FloatExternal,
                    "INTEGER" => LoaderType::IntegerExternal,
                    _ => LoaderType::DecimalExternal,
                }
            }
            other => return Err(self.lex.err(format!("unsupported column datatype {other}"))),
        };
        // Optional NULLIF x=BLANKS and similar trailing clauses are skipped.
        while self.word_is("NULLIF")? {
            self.next()?;
            self.next()?;
            self.expect(&Tok::Equals)?;
            self.next()?;
        }
        Ok(ty)
    }

    fn format_string(&mut self, kw: &str) -> Result<String, CtlError> {
        match self.next()? {
            Some(Tok::Quoted(f)) => Ok(f),
            other => Err(self.lex.err(format!(
                "expected a quoted format after {kw}, found {other:?}"
            ))),
        }
    }
}

/// Parses a control file's text.
pub fn parse(path: &str, text: &str) -> Result<CtlFile, CtlError> {
    let mut p = Parser {
        lex: Lexer {
            path,
            src: text,
            pos: 0,
            line: 1,
            line_start: 0,
        },
        peeked: None,
    };
    let f = p.file()?;
    if let Some(t) = p.next()? {
        return Err(p.lex.err(format!("unexpected {t:?} after the column list")));
    }
    Ok(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCADA: &str = r#"-- comment line
LOAD DATA
INFILE PUBLIC_DVD_DISPATCH_UNIT_SCADA_202401010000.CSV
APPEND INTO TABLE DISPATCH_UNIT_SCADA
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
SETTLEMENTDATE DATE "yyyy/mm/dd hh24:mi:ss",
DUID CHAR(20),
SCADAVALUE FLOAT EXTERNAL)
-- End of Script
"#;

    #[test]
    fn layout_hash_and_default_types() {
        let a = parse("a.ctl", SCADA).unwrap();
        assert_eq!(a.column_names(), ["SETTLEMENTDATE", "DUID", "SCADAVALUE"]);
        // Another month's file: a new name and title, the same layout.
        let next = SCADA
            .replace("202401010000", "202608010000")
            .replace("hh24:mi:ss", "HH24:MI:SS");
        let b = parse("b.ctl", &next).unwrap();
        assert!(a.same_layout(&b));
        assert_ne!(sha256(SCADA.as_bytes()), sha256(next.as_bytes()));
        let wider = SCADA.replace(
            "SCADAVALUE FLOAT EXTERNAL)",
            "SCADAVALUE FLOAT EXTERNAL,\nLASTCHANGED DATE \"yyyy/mm/dd hh24:mi:ss\")",
        );
        assert!(!a.same_layout(&parse("c.ctl", &wider).unwrap()));
        let typed = SCADA.replace("DUID CHAR(20)", "DUID CHAR(30)");
        assert!(!a.same_layout(&parse("d.ctl", &typed).unwrap()));
        assert_eq!(
            sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let ty = |l: LoaderType| l.default_type();
        assert_eq!(ty(LoaderType::Filler), None);
        assert_eq!(ty(LoaderType::Char(3)), Some(LakeType::String));
        for n in [
            LoaderType::FloatExternal,
            LoaderType::DecimalExternal,
            LoaderType::IntegerExternal,
        ] {
            assert_eq!(ty(n), Some(LakeType::Double));
        }
        assert_eq!(
            ty(LoaderType::Date("yyyy/mm/dd hh24:mi:ss".into())),
            Some(LakeType::TimestampNs)
        );
        assert_eq!(
            ty(LoaderType::Date("yyyy/mm/dd".into())),
            Some(LakeType::Date)
        );
        assert_eq!(
            ty(LoaderType::Timestamp(Some("YYYY/MM/DD".into()))),
            Some(LakeType::Date)
        );
        assert_eq!(ty(LoaderType::Timestamp(None)), Some(LakeType::TimestampNs));
    }

    #[test]
    fn parses_aemo_ctl() {
        let f = parse("x.ctl", SCADA).unwrap();
        assert_eq!(f.table, "DISPATCH_UNIT_SCADA");
        assert_eq!(f.infile, "PUBLIC_DVD_DISPATCH_UNIT_SCADA_202401010000.CSV");
        assert_eq!(f.columns.len(), 7);
        let data: Vec<_> = f.data_columns().collect();
        assert_eq!(data.len(), 3);
        assert_eq!(data[0].ty, LoaderType::Date("yyyy/mm/dd hh24:mi:ss".into()));
        assert_eq!(data[1].ty, LoaderType::Char(20));
        assert_eq!(data[2].ty, LoaderType::FloatExternal);
        assert_eq!(data[0].ty.render(), "DATE \"yyyy/mm/dd hh24:mi:ss\"");
        assert_eq!(LoaderType::Filler.render(), "FILLER");
        assert_eq!(LoaderType::Char(3).render(), "CHAR(3)");
        assert_eq!(
            LoaderType::Timestamp(Some("f".into())).render(),
            "TIMESTAMP \"f\""
        );
        assert_eq!(LoaderType::Timestamp(None).render(), "TIMESTAMP");
        assert_eq!(LoaderType::IntegerExternal.render(), "INTEGER EXTERNAL");
        assert_eq!(LoaderType::DecimalExternal.render(), "DECIMAL EXTERNAL");
        use nemlake_core::TypeClass;
        let classes: Vec<_> = f.columns.iter().map(|c| c.ty.type_class()).collect();
        assert_eq!(
            classes[4..],
            [
                Some(TypeClass::Timestamp),
                Some(TypeClass::Varchar),
                Some(TypeClass::Number)
            ]
        );
        assert_eq!(classes[0], None);
        assert_eq!(
            LoaderType::Date("yyyy/mm/dd".into()).type_class(),
            Some(TypeClass::Date)
        );
        assert_eq!(
            LoaderType::IntegerExternal.type_class(),
            Some(TypeClass::Integer)
        );
        assert_eq!(
            LoaderType::DecimalExternal.type_class(),
            Some(TypeClass::Number)
        );
    }

    #[test]
    fn parses_variants() {
        let text = r#"LOAD DATA INFILE "a.CSV" APPEND INTO TABLE T (A INTEGER EXTERNAL NULLIF A=BLANKS, B DECIMAL EXTERNAL, C TIMESTAMP(3) "yyyy", D timestamp "x", E TIMESTAMP(3), F TIMESTAMP)"#;
        let f = parse("v.ctl", text).unwrap();
        assert_eq!(f.columns[0].ty, LoaderType::IntegerExternal);
        assert_eq!(f.columns[1].ty, LoaderType::DecimalExternal);
        assert_eq!(f.columns[2].ty, LoaderType::Timestamp(Some("yyyy".into())));
        assert_eq!(f.columns[3].ty, LoaderType::Timestamp(Some("x".into())));
        assert_eq!(f.columns[4].ty, LoaderType::Timestamp(None));
        assert_eq!(f.columns[5].ty, LoaderType::Timestamp(None));
    }

    #[test]
    fn errors_name_the_token_and_position() {
        let bad = SCADA.replace("FLOAT EXTERNAL", "POSITION(1:10) FLOAT EXTERNAL");
        let e = parse("x.ctl", &bad).unwrap_err();
        assert!(e.message.contains("POSITION"), "{e}");
        assert_eq!(e.line, 15);
        assert!(e.sample.contains("SCADAVALUE"));
        let d = e.diagnostic();
        assert_eq!(d.code, "schema.ctl_parse");
        assert_eq!(d.line, Some(15));
        for (bad, needle) in [
            (
                "LOAD DATA INFILE a MERGE INTO TABLE T (A FILLER)",
                "unsupported load mode",
            ),
            (
                "LOAD DATA INFILE a APPEND INTO TABLE T BOGUS (A FILLER)",
                "unsupported clause",
            ),
            (
                "LOAD DATA INFILE a APPEND INTO TABLE T (A CHAR(x))",
                "expected a length",
            ),
            (
                "LOAD DATA INFILE a APPEND INTO TABLE T (A DATE 3)",
                "expected a quoted format",
            ),
            (
                "LOAD DATA INFILE a APPEND INTO TABLE T (A FILLER) extra",
                "unexpected",
            ),
            (
                "LOAD DATA INFILE a APPEND INTO TABLE T (A FILLER B FILLER)",
                "expected , or )",
            ),
            (
                "LOAD DATA INFILE a APPEND INTO TABLE T (A 'oops')",
                "expected a column datatype",
            ),
            (
                "LOAD DATA INFILE a APPEND INTO TABLE T (A FLOAT)",
                "expected EXTERNAL",
            ),
            (
                "LOAD DATA INFILE a APPEND INTO TABLE T ('q' FILLER)",
                "expected a column name",
            ),
            (
                "LOAD DATA INFILE a APPEND INTO TABLE T WHEN (1:1) = D (A FILLER)",
                "expected a quoted row type",
            ),
            (
                "LOAD DATA INFILE a APPEND INTO TABLE T (A CHAR(1",
                "expected RParen",
            ),
            ("LOAD DATA INFILE 'unterminated", "unterminated"),
            (
                "LOAD DATA INFILE a APPEND INTO TABLE T ;",
                "unexpected character",
            ),
            ("LOAD DATA INFILE a APPEND INTO TABLE", "found end of file"),
            ("LOAD DATA INFILE a APPEND INTO TABLE T", "found None"),
            ("LOAD DATA INFILE ( APPEND", "expected a file name"),
            (
                "LOAD DATA INFILE a APPEND INTO TABLE T (A CHAR(99999999999))",
                "is not a number",
            ),
        ] {
            let e = parse("b.ctl", bad).unwrap_err();
            assert!(e.message.contains(needle), "{bad}: {e}");
        }
    }

    #[test]
    fn parses_every_fixture_ctl() {
        let dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../testdata/bootstrap/MMSDM_2024_01/CTL"
        );
        let mut n = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let p = entry.unwrap().path();
            let text = std::fs::read_to_string(&p).unwrap();
            let f = parse(p.to_str().unwrap(), &text).unwrap_or_else(|e| panic!("{e}"));
            assert!(f.data_columns().count() >= 1, "{}", p.display());
            assert!(
                f.columns
                    .iter()
                    .filter(|c| c.ty == LoaderType::Filler)
                    .count()
                    >= 4,
                "{}",
                p.display()
            );
            n += 1;
        }
        assert!(n >= 20, "only {n} fixtures");
    }
}
