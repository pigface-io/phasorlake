//! The one shape every error and warning in utilake is reported as.
//!
//! A `Diagnostic` carries a stable machine-readable code, a category, a
//! severity, a full-sentence message, and every piece of context the layer
//! that raised it knew: the request being served, the URL and file, the zip
//! entry, the table and version, the line and byte offset, the column, what
//! was expected and observed, and a bounded sample of the offending input.
//! Outer layers fill in the fields they know without touching the inner ones.

use std::borrow::Cow;
use std::fmt;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

/// A stable diagnostic code such as `csv.field_count_mismatch`. Codes are
/// `&'static str` when raised and owned when read back from JSON.
pub type Code = Cow<'static, str>;

/// The area of the system a diagnostic belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// Configuration file, flags, environment.
    Config,
    /// HTTP transport.
    Net,
    /// Directory listings and file-name resolution.
    Listing,
    /// The local download cache.
    Cache,
    /// Zip archives.
    Zip,
    /// The C/I/D row structure of an MMS CSV, or the header and rows of a
    /// plain one.
    Csv,
    /// A WEMDE JSON document's structure.
    Json,
    /// A field's value failing its column type.
    Parse,
    /// The schema registry: resolution, drift, validation, import.
    Schema,
    /// Writing Parquet.
    Store,
    /// The SQLite manifest.
    Manifest,
    /// `utilake verify` findings.
    Verify,
    /// Reading the lake: a query's window, roles, projection and files.
    Query,
    /// The mirror's own records: crawl, peek, removed duplicates, map.
    Mirror,
    /// A bug in utilake (panic, broken invariant).
    Internal,
}

impl Category {
    /// The lower-case name used as the first segment of a code.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Net => "net",
            Self::Listing => "listing",
            Self::Cache => "cache",
            Self::Zip => "zip",
            Self::Csv => "csv",
            Self::Json => "json",
            Self::Parse => "parse",
            Self::Schema => "schema",
            Self::Store => "store",
            Self::Manifest => "manifest",
            Self::Verify => "verify",
            Self::Query => "query",
            Self::Mirror => "mirror",
            Self::Internal => "internal",
        }
    }

    /// Every category, for documentation and exhaustiveness tests.
    pub const ALL: [Self; 15] = [
        Self::Config,
        Self::Net,
        Self::Listing,
        Self::Cache,
        Self::Zip,
        Self::Csv,
        Self::Json,
        Self::Parse,
        Self::Schema,
        Self::Store,
        Self::Manifest,
        Self::Verify,
        Self::Query,
        Self::Mirror,
        Self::Internal,
    ];
}

impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Whether a diagnostic failed something or only noted something.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Worth knowing; the work continued.
    Warning,
    /// The unit of work (row, block, file, run) failed.
    Error,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Warning => "warning",
            Self::Error => "error",
        })
    }
}

/// A bounded excerpt of the input that caused a diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sample {
    /// Up to [`Sample::MAX_BYTES`] of the input, lossily decoded as UTF-8,
    /// with control characters escaped.
    pub text: String,
    /// Whether the input was longer than what `text` holds.
    pub truncated: bool,
    /// The input's full length in bytes.
    pub original_len: usize,
}

impl Sample {
    /// How much of the input a sample keeps.
    pub const MAX_BYTES: usize = 256;

    /// Takes a sample of raw bytes.
    pub fn of(raw: &[u8]) -> Self {
        let truncated = raw.len() > Self::MAX_BYTES;
        let mut end = raw.len().min(Self::MAX_BYTES);
        // Never cut a multi-byte character in half: back off to a boundary.
        while end > 0 && end < raw.len() && (raw[end] & 0xC0) == 0x80 {
            end -= 1;
        }
        let mut text = String::with_capacity(end);
        for ch in String::from_utf8_lossy(&raw[..end]).chars() {
            match ch {
                '\n' => text.push_str("\\n"),
                '\r' => text.push_str("\\r"),
                '\t' => text.push_str("\\t"),
                c if c.is_control() => {
                    let _ = write!(text, "\\x{:02x}", c as u32);
                }
                c => text.push(c),
            }
        }
        Self {
            text,
            truncated,
            original_len: raw.len(),
        }
    }
}

impl fmt::Display for Sample {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "`{}`", self.text)?;
        if self.truncated {
            write!(f, "… ({} bytes)", self.original_len)?;
        }
        Ok(())
    }
}

/// Everything known about where and why something went wrong.
///
/// Fields are `Option` because each layer knows a different subset; see
/// [`Diagnostic::fill_from`] for how they combine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    /// Stable, machine-readable, e.g. `csv.field_count_mismatch`. Never
    /// renamed or reused; `docs/errors.md` lists every one.
    pub code: Code,
    /// The area of the system.
    pub category: Category,
    /// Error or warning.
    pub severity: Severity,
    /// A full sentence naming the thing that is wrong.
    pub message: String,
    /// The user's request, e.g. `fetch nem DISPATCH_UNIT_SCADA 2024-01..2024-01`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
    /// The remote URL involved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The local file involved (cache path or user path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// The entry inside the zip, nested entries joined with `!`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
    /// The table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<String>,
    /// The schema version from the I row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u16>,
    /// 1-based line in the CSV.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    /// 0-based byte offset of that line in the CSV.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_offset: Option<u64>,
    /// The column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    /// What was expected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    /// What was observed instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed: Option<String>,
    /// A bounded excerpt of the offending input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample: Option<Sample>,
    /// The chain of underlying causes, outermost first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cause: Vec<String>,
}

impl Diagnostic {
    /// Starts a diagnostic with the three things every one has.
    pub fn new(
        code: &'static str,
        category: Category,
        severity: Severity,
        message: impl Into<String>,
    ) -> Self {
        debug_assert!(
            code.starts_with(category.as_str())
                && code.as_bytes().get(category.as_str().len()) == Some(&b'.'),
            "code {code} must start with {}.",
            category.as_str()
        );
        Self {
            code: Cow::Borrowed(code),
            category,
            severity,
            message: message.into(),
            request: None,
            url: None,
            file: None,
            entry: None,
            table: None,
            version: None,
            line: None,
            byte_offset: None,
            column: None,
            expected: None,
            observed: None,
            sample: None,
            cause: Vec::new(),
        }
    }

    /// An error.
    pub fn error(code: &'static str, category: Category, message: impl Into<String>) -> Self {
        Self::new(code, category, Severity::Error, message)
    }

    /// A warning.
    pub fn warning(code: &'static str, category: Category, message: impl Into<String>) -> Self {
        Self::new(code, category, Severity::Warning, message)
    }

    /// Sets the request.
    pub fn with_request(mut self, v: impl Into<String>) -> Self {
        self.request = Some(v.into());
        self
    }
    /// Sets the URL.
    pub fn with_url(mut self, v: impl Into<String>) -> Self {
        self.url = Some(v.into());
        self
    }
    /// Sets the file.
    pub fn with_file(mut self, v: impl Into<String>) -> Self {
        self.file = Some(v.into());
        self
    }
    /// Sets the entry.
    pub fn with_entry(mut self, v: impl Into<String>) -> Self {
        self.entry = Some(v.into());
        self
    }
    /// Sets the table.
    pub fn with_table(mut self, v: impl Into<String>) -> Self {
        self.table = Some(v.into());
        self
    }
    /// Sets the version.
    pub fn with_version(mut self, v: u16) -> Self {
        self.version = Some(v);
        self
    }
    /// Sets the line and byte offset.
    pub fn at(mut self, line: u64, byte_offset: u64) -> Self {
        self.line = Some(line);
        self.byte_offset = Some(byte_offset);
        self
    }
    /// Sets the column.
    pub fn with_column(mut self, v: impl Into<String>) -> Self {
        self.column = Some(v.into());
        self
    }
    /// Sets expected and observed.
    pub fn expected_observed(
        mut self,
        expected: impl Into<String>,
        observed: impl Into<String>,
    ) -> Self {
        self.expected = Some(expected.into());
        self.observed = Some(observed.into());
        self
    }
    /// Takes a sample of raw input.
    pub fn with_sample(mut self, raw: &[u8]) -> Self {
        self.sample = Some(Sample::of(raw));
        self
    }
    /// Appends an underlying cause and its own source chain.
    pub fn caused_by(mut self, err: &(dyn std::error::Error + 'static)) -> Self {
        self.cause.push(err.to_string());
        let mut src = err.source();
        while let Some(s) = src {
            self.cause.push(s.to_string());
            src = s.source();
        }
        self
    }

    /// Fills every field that is still `None` from `outer`.
    ///
    /// The inner layer knows the most precise context (line, sample), so its
    /// values win; the outer layer adds what it alone knows (request, URL).
    pub fn fill_from(&mut self, outer: &Context) {
        fill_option(&mut self.request, outer.request.as_ref());
        fill_option(&mut self.url, outer.url.as_ref());
        fill_option(&mut self.file, outer.file.as_ref());
        join_entry(&mut self.entry, outer.entry.as_ref());
        fill_option(&mut self.table, outer.table.as_ref());
        if self.version.is_none() {
            self.version = outer.version;
        }
    }

    /// Whether this is an error.
    pub const fn is_error(&self) -> bool {
        matches!(self.severity, Severity::Error)
    }

    /// Renders every known field as one paragraph of full sentences, the
    /// human form used in terminals and logs.
    pub fn render(&self) -> String {
        let mut s = format!("{} [{}]", self.message, self.code);
        let mut where_ = Vec::new();
        if let Some(r) = &self.request {
            where_.push(format!("while running `{r}`"));
        }
        if let Some(u) = &self.url {
            where_.push(format!("from {u}"));
        }
        if let Some(f) = &self.file {
            where_.push(format!("in {f}"));
        }
        if let Some(e) = &self.entry {
            where_.push(format!("entry {e}"));
        }
        match (&self.table, self.version) {
            (Some(t), Some(v)) => where_.push(format!("table {t} version {v}")),
            (Some(t), None) => where_.push(format!("table {t}")),
            _ => {}
        }
        match (self.line, self.byte_offset) {
            (Some(l), Some(b)) => where_.push(format!("line {l} (byte {b})")),
            (Some(l), None) => where_.push(format!("line {l}")),
            _ => {}
        }
        if let Some(c) = &self.column {
            where_.push(format!("column {c}"));
        }
        if !where_.is_empty() {
            s.push_str(" This happened ");
            s.push_str(&where_.join(", "));
            s.push('.');
        }
        match (&self.expected, &self.observed) {
            (Some(e), Some(o)) => {
                let _ = write!(s, " Expected {e}; observed {o}.");
            }
            (Some(e), None) => {
                let _ = write!(s, " Expected {e}.");
            }
            (None, Some(o)) => {
                let _ = write!(s, " Observed {o}.");
            }
            _ => {}
        }
        if let Some(sample) = &self.sample {
            let _ = write!(s, " Sample: {sample}.");
        }
        if !self.cause.is_empty() {
            let _ = write!(s, " Caused by: {}.", self.cause.join("; "));
        }
        s
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render())
    }
}

impl std::error::Error for Diagnostic {}

/// The context an outer layer adds to diagnostics raised below it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Context {
    /// The user's request.
    pub request: Option<String>,
    /// The remote URL.
    pub url: Option<String>,
    /// The local file.
    pub file: Option<String>,
    /// The zip entry (outermost first, `!`-joined).
    pub entry: Option<String>,
    /// The table.
    pub table: Option<String>,
    /// The schema version.
    pub version: Option<u16>,
}

impl Context {
    /// Fills every field that is still `None` from `outer`; see
    /// [`Diagnostic::fill_from`] for the entry-joining rule.
    pub fn fill_from(&mut self, outer: &Context) {
        fill_option(&mut self.request, outer.request.as_ref());
        fill_option(&mut self.url, outer.url.as_ref());
        fill_option(&mut self.file, outer.file.as_ref());
        join_entry(&mut self.entry, outer.entry.as_ref());
        fill_option(&mut self.table, outer.table.as_ref());
        if self.version.is_none() {
            self.version = outer.version;
        }
    }
}

fn fill_option(slot: &mut Option<String>, outer: Option<&String>) {
    if slot.is_none() {
        *slot = outer.cloned();
    }
}

/// A nested entry keeps its path as `outer.zip!inner.zip!FILE.CSV`: the outer
/// layer's entry is prefixed unless it is already there.
fn join_entry(slot: &mut Option<String>, outer: Option<&String>) {
    match (slot, outer) {
        (slot @ None, Some(e)) => *slot = Some(e.clone()),
        (Some(inner), Some(e)) if !inner.starts_with(e.as_str()) => *inner = format!("{e}!{inner}"),
        _ => {}
    }
}

/// Anything that can describe itself as a [`Diagnostic`].
///
/// Every library error enum implements this; the pipeline turns errors into
/// diagnostics at the file boundary and the CLI renders them.
pub trait Diagnose {
    /// The diagnostic for this error, with whatever context it carries.
    fn diagnostic(&self) -> Diagnostic;

    /// The stable code of this error.
    fn code(&self) -> Code {
        self.diagnostic().code
    }
}

impl Diagnose for Diagnostic {
    fn diagnostic(&self) -> Diagnostic {
        self.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_bounds_and_escapes() {
        let s = Sample::of(b"D,1,2\r\n");
        assert_eq!(s.text, "D,1,2\\r\\n");
        assert!(!s.truncated);
        let long = vec![b'x'; 1000];
        let s = Sample::of(&long);
        assert_eq!(s.text.len(), Sample::MAX_BYTES);
        assert!(s.truncated);
        assert_eq!(s.original_len, 1000);
        assert!(s.to_string().ends_with("(1000 bytes)"));
        // A multi-byte character straddling the cut is dropped, not split.
        let mut raw = vec![b'a'; Sample::MAX_BYTES - 1];
        raw.extend_from_slice("é".as_bytes());
        let s = Sample::of(&raw);
        assert_eq!(s.text.len(), Sample::MAX_BYTES - 1);
        let s = Sample::of(&[0x01, b'a']);
        assert_eq!(s.text, "\\x01a");
    }

    #[test]
    fn render_includes_every_field() {
        let io = std::io::Error::other("disk gone");
        let d = Diagnostic::error(
            "csv.field_count_mismatch",
            Category::Csv,
            "The row has 8 fields but the I row declared 7.",
        )
        .with_request("fetch nem DISPATCH_UNIT_SCADA 2024-01..2024-01")
        .with_url("https://nemweb.com.au/x.zip")
        .with_file("/cache/x.zip")
        .with_entry("X.CSV")
        .with_table("DISPATCH_UNIT_SCADA")
        .with_version(1)
        .at(48213, 2_097_153)
        .with_column("SCADAVALUE")
        .expected_observed("7 fields", "8 fields")
        .with_sample(b"D,DISPATCH,UNIT_SCADA,1,x")
        .caused_by(&io);
        let r = d.render();
        for needle in [
            "[csv.field_count_mismatch]",
            "while running `fetch nem",
            "from https://nemweb.com.au/x.zip",
            "in /cache/x.zip",
            "entry X.CSV",
            "table DISPATCH_UNIT_SCADA version 1",
            "line 48213 (byte 2097153)",
            "column SCADAVALUE",
            "Expected 7 fields; observed 8 fields.",
            "Sample: `D,DISPATCH,UNIT_SCADA,1,x`.",
            "Caused by: disk gone.",
        ] {
            assert!(r.contains(needle), "{r} lacks {needle}");
        }
        assert_eq!(d.to_string(), r);
        assert!(d.is_error());
        let json: serde_json::Value = serde_json::to_value(&d).unwrap();
        assert_eq!(json["code"], "csv.field_count_mismatch");
        assert_eq!(json["category"], "csv");
        assert_eq!(json["severity"], "error");
        assert_eq!(json["line"], 48213);
        let back: Diagnostic = serde_json::from_value(json).unwrap();
        assert_eq!(back.line, d.line);
        assert_eq!(Category::Csv.to_string(), "csv");
        assert_eq!(Severity::Warning.to_string(), "warning");
        assert_eq!(Category::ALL.len(), 15);
        assert_eq!(Category::Mirror.to_string(), "mirror");
        assert_eq!(Category::Json.to_string(), "json");
    }

    #[test]
    fn render_partial_fields() {
        let d = Diagnostic::warning("csv.footer_count_mismatch", Category::Csv, "Footer says 9.")
            .with_table("REGION")
            .expected_observed("9", "8");
        let r = d.render();
        assert!(r.contains("table REGION."));
        assert!(!r.contains("version"));
        let d = Diagnostic::error("net.offline", Category::Net, "Offline.");
        assert_eq!(d.render(), "Offline. [net.offline]");
        let mut d = Diagnostic::error("net.http_status", Category::Net, "x.").with_sample(b"body");
        d.observed = Some("503".into());
        assert!(d.render().contains(" Observed 503."));
        d.expected = Some("200".into());
        d.observed = None;
        assert!(d.render().contains(" Expected 200."));
        d.line = Some(3);
        assert!(d.render().contains("line 3."));
    }

    #[derive(Debug)]
    struct Inner;
    impl Diagnose for Inner {
        fn diagnostic(&self) -> Diagnostic {
            Diagnostic::error("parse.timestamp", Category::Parse, "Bad.")
                .with_entry("INNER.CSV")
                .at(7, 70)
        }
    }

    #[test]
    fn fill_from_merges_outer_context_without_overwriting_inner() {
        let outer = Context {
            request: Some("ingest a.zip".into()),
            file: Some("a.zip".into()),
            entry: Some("b.zip".into()),
            table: Some("T".into()),
            version: Some(2),
            url: None,
        };
        let mut d = Inner.diagnostic();
        d.fill_from(&outer);
        assert_eq!(d.entry.as_deref(), Some("b.zip!INNER.CSV"));
        assert_eq!(d.line, Some(7));
        assert_eq!(d.request.as_deref(), Some("ingest a.zip"));
        assert_eq!(d.table.as_deref(), Some("T"));
        assert_eq!(d.version, Some(2));
        // A second, outer-most layer adds the URL and keeps everything else.
        let outermost = Context {
            url: Some("https://h/a.zip".into()),
            request: Some("other".into()),
            ..Default::default()
        };
        d.fill_from(&outermost);
        assert_eq!(d.url.as_deref(), Some("https://h/a.zip"));
        assert_eq!(d.request.as_deref(), Some("ingest a.zip"));
        assert_eq!(d.entry.as_deref(), Some("b.zip!INNER.CSV"));
        // Re-applying the same entry prefix does not duplicate it.
        d.fill_from(&outer);
        assert_eq!(d.entry.as_deref(), Some("b.zip!INNER.CSV"));
        assert_eq!(
            Diagnostic::error("net.x", Category::Net, "m")
                .diagnostic()
                .code,
            "net.x"
        );
    }

    #[test]
    fn every_category_names_itself_causes_chain_and_contexts_fill() {
        for c in Category::ALL {
            assert_eq!(c.to_string(), c.as_str());
        }
        assert_eq!(Sample::of(b"a\tb").text, "a\\tb");
        #[derive(Debug, thiserror::Error)]
        #[error("outer")]
        struct Outer(#[source] std::io::Error);
        let d = Diagnostic::error("zip.bad_entry", Category::Zip, "m")
            .caused_by(&Outer(std::io::Error::other("disk gone")));
        assert_eq!(d.cause, ["outer", "disk gone"]);
        let mut inner = Context {
            entry: Some("INNER.CSV".into()),
            ..Context::default()
        };
        let outer = Context {
            request: Some("ingest".into()),
            url: Some("https://h/a.zip".into()),
            file: Some("a.zip".into()),
            entry: Some("b.zip".into()),
            table: Some("T".into()),
            version: Some(2),
        };
        inner.fill_from(&outer);
        assert_eq!(inner.entry.as_deref(), Some("b.zip!INNER.CSV"));
        assert_eq!(
            (inner.table.as_deref(), inner.version),
            (Some("T"), Some(2))
        );
    }

    #[test]
    #[should_panic(expected = "must start with")]
    fn code_must_match_category() {
        let _ = Diagnostic::error("net.x", Category::Csv, "m");
    }
}
