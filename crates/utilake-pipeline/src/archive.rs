//! A zip that could not be walked, diagnosed. The zip walker knows no
//! diagnostics; the pipeline, which walks every file it ingests, gives its
//! failures their codes.

use utilake_core::diag::{Category, Diagnostic};
use utilake_kit::container::ArchiveError;

/// An archive that could not be walked, or an entry of it that could not
/// be read, diagnosed.
pub fn archive_diagnostic(e: &ArchiveError) -> Diagnostic {
    let msg = e.to_string();
    match e {
        ArchiveError::Open { path, .. } => {
            Diagnostic::error("zip.open_failed", Category::Zip, msg).with_file(path.clone())
        }
        ArchiveError::Entry { path, entry, .. } => {
            Diagnostic::error("zip.bad_entry", Category::Zip, msg)
                .with_file(path.clone())
                .with_entry(entry.clone())
        }
        ArchiveError::TooDeep { path, entry, .. } => {
            Diagnostic::error("zip.nesting_too_deep", Category::Zip, msg)
                .with_file(path.clone())
                .with_entry(entry.clone())
        }
        ArchiveError::NoEntry { path, .. } => {
            Diagnostic::error("zip.no_entry", Category::Zip, msg).with_file(path.clone())
        }
        ArchiveError::Spool {
            path,
            entry,
            source,
            ..
        } => Diagnostic::error("zip.spool_failed", Category::Zip, msg)
            .with_file(path.clone())
            .with_entry(entry.clone())
            .caused_by(source),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_errors_have_codes() {
        let codes: Vec<String> = [
            ArchiveError::Open {
                path: "a.zip".into(),
                message: "m".into(),
            },
            ArchiveError::Entry {
                path: "a.zip".into(),
                entry: "e".into(),
                message: "m".into(),
            },
            ArchiveError::TooDeep {
                path: "a.zip".into(),
                entry: "e".into(),
                depth: 3,
            },
            ArchiveError::NoEntry {
                path: "a.zip".into(),
                wanted: ".CSV".into(),
            },
            ArchiveError::Spool {
                path: "a.zip".into(),
                entry: "e".into(),
                tmp: "/t".into(),
                source: std::io::Error::other("full"),
            },
        ]
        .iter()
        .map(|e| archive_diagnostic(e).code.into_owned())
        .collect();
        assert_eq!(
            codes,
            [
                "zip.open_failed",
                "zip.bad_entry",
                "zip.nesting_too_deep",
                "zip.no_entry",
                "zip.spool_failed"
            ]
        );
        let spool = archive_diagnostic(&ArchiveError::Spool {
            path: "a.zip".into(),
            entry: "e".into(),
            tmp: "/t".into(),
            source: std::io::Error::other("full"),
        });
        assert_eq!((spool.entry.as_deref(), spool.cause.len()), (Some("e"), 1));
    }
}
