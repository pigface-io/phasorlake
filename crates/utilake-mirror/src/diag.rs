//! The one shape every error and warning of nemweb-mirror is reported as.
//!
//! A `Diagnostic` carries a stable machine-readable code, a category, a
//! severity, a full-sentence message, and the context the layer that raised
//! it knew: the URL and file, the line of a record, what was expected and
//! observed, a bounded sample of the offending input, and its causes.

use std::borrow::Cow;
use std::fmt;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

/// A stable diagnostic code such as `zip.bad_entry`. Codes are
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
    /// Directory listings.
    Listing,
    /// The archive of downloads.
    Cache,
    /// Zip archives.
    Zip,
    /// The mirror's own records: crawl, peek, removed duplicates, map.
    Mirror,
    /// A bug in nemweb-mirror (panic, broken invariant).
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
            Self::Mirror => "mirror",
            Self::Internal => "internal",
        }
    }

    /// Every category, for documentation and exhaustiveness tests.
    pub const ALL: [Self; 7] = [
        Self::Config,
        Self::Net,
        Self::Listing,
        Self::Cache,
        Self::Zip,
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
    /// The unit of work failed.
    Error,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    /// Stable, machine-readable, e.g. `zip.bad_entry`. Never renamed or
    /// reused; `docs/errors.md` lists every one.
    pub code: Code,
    /// The area of the system.
    pub category: Category,
    /// Error or warning.
    pub severity: Severity,
    /// A full sentence naming the thing that is wrong.
    pub message: String,
    /// The remote URL involved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The local file involved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// 1-based line of a record file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
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
            url: None,
            file: None,
            line: None,
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

    /// Sets the line.
    pub fn at_line(mut self, line: u64) -> Self {
        self.line = Some(line);
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
        let mut src = Some(err);
        while let Some(s) = src {
            self.cause.push(s.to_string());
            src = s.source();
        }
        self
    }

    /// Every known field as one paragraph of full sentences, the human form
    /// used in terminals and logs.
    pub fn render(&self) -> String {
        let mut s = format!("{} [{}]", self.message, self.code);
        let mut where_ = Vec::new();
        if let Some(u) = &self.url {
            where_.push(format!("from {u}"));
        }
        if let Some(f) = &self.file {
            where_.push(format!("in {f}"));
        }
        if let Some(l) = self.line {
            where_.push(format!("line {l}"));
        }
        if !where_.is_empty() {
            let _ = write!(s, " This happened {}.", where_.join(", "));
        }
        if let Some(e) = &self.expected {
            let _ = write!(s, " Expected {e}.");
        }
        if let Some(o) = &self.observed {
            let _ = write!(s, " Observed {o}.");
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

/// Anything that can describe itself as a [`Diagnostic`].
pub trait Diagnose {
    /// The diagnostic for this error, with whatever context it carries.
    fn diagnostic(&self) -> Diagnostic;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_bounds_and_escapes() {
        let s = Sample::of(b"D,1,2\r\n\t");
        assert_eq!(s.text, "D,1,2\\r\\n\\t");
        assert!(!s.truncated);
        assert_eq!(s.to_string(), "`D,1,2\\r\\n\\t`");
        let long = vec![b'x'; 1000];
        let s = Sample::of(&long);
        assert_eq!(s.text.len(), Sample::MAX_BYTES);
        assert!(s.truncated);
        assert_eq!(s.original_len, 1000);
        assert!(s.to_string().ends_with("(1000 bytes)"));
        // A multi-byte character straddling the cut is dropped, not split.
        let mut raw = vec![b'a'; Sample::MAX_BYTES - 1];
        raw.extend_from_slice("é".as_bytes());
        assert_eq!(Sample::of(&raw).text.len(), Sample::MAX_BYTES - 1);
        assert_eq!(Sample::of(&[0x01, b'a']).text, "\\x01a");
    }

    #[test]
    fn render_includes_every_field() {
        #[derive(Debug, thiserror::Error)]
        #[error("outer")]
        struct Outer(#[source] std::io::Error);
        let d = Diagnostic::error("zip.bad_entry", Category::Zip, "The entry is bad.")
            .with_url("https://h/x.zip")
            .with_file("/a/x.zip")
            .at_line(7)
            .expected_observed("7 fields", "8 fields")
            .with_sample(b"D,x")
            .caused_by(&Outer(std::io::Error::other("disk gone")));
        let r = d.render();
        assert_eq!(
            r,
            "The entry is bad. [zip.bad_entry] This happened from https://h/x.zip, in /a/x.zip, line 7. Expected 7 fields. Observed 8 fields. Sample: `D,x`. Caused by: outer; disk gone."
        );
        assert_eq!(d.to_string(), r);
        let json: serde_json::Value = serde_json::to_value(&d).unwrap();
        assert_eq!(json["code"], "zip.bad_entry");
        assert_eq!(json["category"], "zip");
        assert_eq!(json["severity"], "error");
        let back: Diagnostic = serde_json::from_value(json).unwrap();
        assert_eq!(back, d);
        let w = Diagnostic::warning("net.offline", Category::Net, "Offline.");
        assert_eq!(
            (w.render().as_str(), w.severity),
            ("Offline. [net.offline]", Severity::Warning)
        );
        let names: Vec<String> = Category::ALL.iter().map(ToString::to_string).collect();
        assert_eq!(
            names,
            [
                "config", "net", "listing", "cache", "zip", "mirror", "internal"
            ]
        );
    }

    #[test]
    #[should_panic(expected = "must start with")]
    fn code_must_match_category() {
        let _ = Diagnostic::error("net.x", Category::Zip, "m");
    }
}
