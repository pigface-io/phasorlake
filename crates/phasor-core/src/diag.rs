//! The one shape every error and warning is reported as: [`phasor_diag`] re-exported,
//! with phasor's categories. Outer layers fill in fields without touching inner ones.

pub use phasor_diag::{Category, Code, Context, Diagnose, Diagnostic, Sample, Severity, exit};

/// The mirror's own records: crawl, peek, removed duplicates, map.
pub const MIRROR: Category = Category::new("mirror");

/// Every category phasor raises, in catalogue order.
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
    fn phasor_declares_the_common_categories_and_the_mirror() {
        for c in Category::COMMON {
            assert!(CATEGORIES.contains(&c), "{c}");
        }
        assert_eq!(MIRROR.as_str(), "mirror");
        assert_eq!(CATEGORIES.len(), Category::COMMON.len() + 1);
    }
}
