//! The ways a value can fail the spec. Each message is a full sentence.

use std::fmt;

use crate::{QualityContext, QualityFlag, SiUnit};

/// A field value that is not one the spec allows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The text is not one of the field's allowed values.
    Unknown {
        /// The field or code table, e.g. `"TransCode"`.
        what: &'static str,
        /// The text as read.
        text: String,
    },
    /// The quality flag needs a method flag and none was given
    /// (`MDFF-QUAL-005`).
    MethodFlagRequired(QualityFlag),
    /// The quality flag takes no method flag and one was given
    /// (`MDFF-QUAL-009`).
    MethodFlagNotAllowed(QualityFlag),
    /// The quality flag needs a reason code and none was given
    /// (`MDFF-QUAL-006`).
    ReasonCodeRequired(QualityFlag),
    /// The quality flag takes no reason code and one was given
    /// (`MDFF-QUAL-009`, `MDFF-REASON-005`).
    ReasonCodeNotAllowed(QualityFlag),
    /// The quality flag is not allowed in this field (`MDFF-QUAL-009`,
    /// `MDFF-400-006`, `MDFF-250-010`).
    QualityNotAllowed {
        /// The flag found.
        flag: QualityFlag,
        /// Where it was found.
        context: QualityContext,
    },
    /// A number failed its format.
    Number(NumberError),
    /// An SI quantity was converted to a unit of another kind.
    UnitMismatch {
        /// The SI unit the target UOM converts from.
        expected: SiUnit,
        /// The SI unit of the quantity given.
        found: SiUnit,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown { what, text } => {
                write!(f, "`{text}` is not an allowed {what} value.")
            }
            Self::MethodFlagRequired(q) => write!(
                f,
                "Quality flag `{q}` needs a two-digit method flag, and none was given."
            ),
            Self::MethodFlagNotAllowed(q) => {
                write!(
                    f,
                    "Quality flag `{q}` takes no method flag, and one was given."
                )
            }
            Self::ReasonCodeRequired(q) => {
                write!(
                    f,
                    "Quality flag `{q}` needs a reason code, and none was given."
                )
            }
            Self::ReasonCodeNotAllowed(q) => {
                write!(
                    f,
                    "Quality flag `{q}` takes no reason code, and one was given."
                )
            }
            Self::QualityNotAllowed { flag, context } => {
                write!(
                    f,
                    "Quality flag `{flag}` is not allowed in {}.",
                    context.describe()
                )
            }
            Self::Number(e) => e.fmt(f),
            Self::UnitMismatch { expected, found } => write!(
                f,
                "A quantity in {} cannot be converted to a unit measured in {}.",
                found.symbol(),
                expected.symbol()
            ),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Number(e) => Some(e),
            _ => None,
        }
    }
}

impl From<NumberError> for Error {
    fn from(e: NumberError) -> Self {
        Self::Number(e)
    }
}

/// An IntervalValue or Quantity that breaks the rules of spec §4.4 and §5.3
/// or the Appendix B format of its unit (`MDFF-NUM-001`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberError {
    /// The field is empty; these fields are never null (`MDFF-FILE-019`).
    Empty,
    /// The value is negative (`MDFF-300-005`, `MDFF-250-020`).
    Negative,
    /// The value is in exponential notation (`MDFF-300-006`).
    Exponent,
    /// The text is not digits with an optional decimal part.
    Invalid,
    /// The value is not a finite number and cannot be written.
    NotFinite,
    /// More characters than the format allows.
    TooLong {
        /// The most the format allows.
        max: u8,
        /// How many the value has.
        found: usize,
    },
    /// More decimal places than the format allows.
    TooManyDecimals {
        /// The most the format allows.
        max: u8,
        /// How many the value has.
        found: usize,
    },
}

impl fmt::Display for NumberError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("The value is empty, and this field is never null."),
            Self::Negative => f.write_str("The value is negative, and this field never is."),
            Self::Exponent => f.write_str(
                "The value is in exponential notation, which the format does not allow.",
            ),
            Self::Invalid => {
                f.write_str("The value is not a number of digits with an optional decimal part.")
            }
            Self::NotFinite => f.write_str("The value is not a finite number."),
            Self::TooLong { max, found } => write!(
                f,
                "The value has {found} characters, and the format allows at most {max}."
            ),
            Self::TooManyDecimals { max, found } => write!(
                f,
                "The value has {found} decimal places, and the format allows at most {max}."
            ),
        }
    }
}

impl std::error::Error for NumberError {}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::*;

    #[test]
    fn messages_are_sentences() {
        let all = [
            Error::Unknown {
                what: "TransCode",
                text: "X".into(),
            },
            Error::MethodFlagRequired(QualityFlag::Substituted),
            Error::MethodFlagNotAllowed(QualityFlag::Variable),
            Error::ReasonCodeRequired(QualityFlag::Final),
            Error::ReasonCodeNotAllowed(QualityFlag::Variable),
            Error::QualityNotAllowed {
                flag: QualityFlag::Variable,
                context: QualityContext::IntervalEvent,
            },
            Error::Number(NumberError::Empty),
            Error::UnitMismatch {
                expected: SiUnit::Joule,
                found: SiUnit::Watt,
            },
            NumberError::Negative.into(),
            NumberError::Exponent.into(),
            NumberError::Invalid.into(),
            NumberError::NotFinite.into(),
            NumberError::TooLong { max: 15, found: 16 }.into(),
            NumberError::TooManyDecimals { max: 1, found: 2 }.into(),
        ];
        for e in &all {
            let m = e.to_string();
            assert!(
                m.ends_with('.') && m.chars().next().is_some_and(|c| !c.is_lowercase()),
                "{m}"
            );
        }
        assert!(all[6].source().is_some());
        assert!(all[0].source().is_none());
        assert_eq!(
            all[7].to_string(),
            "A quantity in W cannot be converted to a unit measured in J."
        );
    }
}
