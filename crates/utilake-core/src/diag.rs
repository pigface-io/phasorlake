//! The one shape every error and warning in utilake is reported as: the
//! family's [`utilake_diag`], re-exported, with the categories utilake
//! declares beside the common ones.
//!
//! A `Diagnostic` carries a stable machine-readable code, a category, a
//! severity, a message, and every piece of context the layer that raised it
//! knew. Outer layers fill in the fields they know without touching the
//! inner ones.

pub use utilake_diag::{Category, Code, Context, Diagnose, Diagnostic, Sample, Severity, exit};

/// The mirror's own records: crawl, peek, removed duplicates, map.
pub const MIRROR: Category = Category::new("mirror");

/// Every category utilake raises: the family's common ones and the
/// mirror's, in the order a catalogue lists them.
pub const CATEGORIES: [Category; 15] = [
    Category::CONFIG,
    Category::NET,
    Category::LISTING,
    Category::CACHE,
    Category::ZIP,
    Category::CSV,
    Category::JSON,
    Category::PARSE,
    Category::SCHEMA,
    Category::STORE,
    Category::MANIFEST,
    Category::VERIFY,
    Category::QUERY,
    MIRROR,
    Category::INTERNAL,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utilake_declares_the_common_categories_and_the_mirror() {
        for c in Category::COMMON {
            assert!(CATEGORIES.contains(&c), "{c}");
        }
        assert_eq!(MIRROR.as_str(), "mirror");
        assert_eq!(CATEGORIES.len(), Category::COMMON.len() + 1);
    }
}
