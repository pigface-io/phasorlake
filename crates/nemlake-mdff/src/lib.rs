//! The code tables and value rules of AEMO's Meter Data File Format
//! Specification NEM12 & NEM13, version 2.7 (effective 1 December 2025),
//! as Rust types. Requirement IDs such as `MDFF-QUAL-006` refer to
//! `reference/mdff/requirements.md`, which restates the spec.
//!
//! - [`TransCode`]: Appendix A transaction codes.
//! - [`Uom`] and [`NumberFormat`]: Appendix B units and the number format
//!   of IntervalValue and Quantity; [`SiQuantity`] converts to SI units.
//! - [`QualityFlag`], [`MethodFlag`] and [`QualityMethod`]: Appendices C and
//!   D, and the `QMM` field that joins them.
//! - [`ReasonCode`]: Appendix E reason codes and the obsolete ones of
//!   Appendix F.
//! - [`DirectionIndicator`], [`RecordIndicator`], [`VersionHeader`] and
//!   [`IntervalLength`]: the other enumerated fields.
//!
//! Case: field values are not case sensitive except enumerated lists of
//! allowed values (spec §3.3(a)), so every code here parses exactly as
//! written, with one exception: Appendix B says UOM values are not case
//! sensitive, so [`Uom`] parses without regard to case.
//!
//! Pure data and conversions: no I/O, no dependencies beyond `std`. Errors
//! are plain values; a caller that reports them turns them into a
//! `Diagnostic` with the context it knows (file, line, field).

/// Defines a field enum whose values are fixed strings: the enum, `ALL`,
/// `as_str`, `FromStr` (exact, or ASCII case-insensitive), `TryFrom<&str>`
/// and `Display`.
macro_rules! code_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident ($what:literal, case_insensitive = $ci:literal) {
            $( $(#[$vmeta:meta])* $variant:ident => $text:literal, )+
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum $name {
            $( $(#[$vmeta])* $variant, )+
        }

        impl $name {
            /// Every value, in the order the spec lists them.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// The value exactly as the spec spells it.
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text,)+
                }
            }
        }

        impl ::std::str::FromStr for $name {
            type Err = $crate::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::ALL
                    .iter()
                    .copied()
                    .find(|v| if $ci { v.as_str().eq_ignore_ascii_case(s) } else { v.as_str() == s })
                    .ok_or_else(|| $crate::Error::Unknown { what: $what, text: s.to_owned() })
            }
        }

        impl TryFrom<&str> for $name {
            type Error = $crate::Error;

            fn try_from(s: &str) -> Result<Self, Self::Error> {
                s.parse()
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

mod direction;
mod error;
mod quality;
mod reason;
mod record;
mod si;
mod trans;
mod uom;

pub use direction::DirectionIndicator;
pub use error::{Error, NumberError};
pub use quality::{MethodFlag, Presence, QualityContext, QualityFlag, QualityMethod};
pub use reason::ReasonCode;
pub use record::{IntervalLength, RecordIndicator, VersionHeader};
pub use si::{QuantityKind, SiQuantity, SiUnit};
pub use trans::TransCode;
pub use uom::{NumberFormat, Uom};
