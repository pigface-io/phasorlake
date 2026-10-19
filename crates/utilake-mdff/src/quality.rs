//! Appendix C quality flags, Appendix D method flags, and the `QMM`
//! QualityMethod field that joins them (300, 400 and 250 records).

use std::fmt;
use std::str::FromStr;

use crate::{Error, ReasonCode};

/// Whether a field must, may or must not be given.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Presence {
    /// It must be given.
    Required,
    /// It may be given or left out.
    Optional,
    /// It must not be given.
    Forbidden,
}

/// The field a QualityMethod is in, which decides the flags it may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QualityContext {
    /// The QualityMethod of a NEM12 300 record.
    IntervalData,
    /// The QualityMethod of a NEM12 400 record.
    IntervalEvent,
    /// PreviousQualityMethod of a NEM13 250 record.
    PreviousRead,
    /// CurrentQualityMethod of a NEM13 250 record.
    CurrentRead,
}

impl QualityContext {
    /// The field, for messages.
    pub const fn describe(self) -> &'static str {
        match self {
            Self::IntervalData => "the QualityMethod of a 300 record",
            Self::IntervalEvent => "the QualityMethod of a 400 record",
            Self::PreviousRead => "the PreviousQualityMethod of a 250 record",
            Self::CurrentRead => "the CurrentQualityMethod of a 250 record",
        }
    }
}

code_enum! {
    /// The quality flag, the first character of a QualityMethod
    /// (Appendix C, p.23). Version 2.7 has no `N` flag: it was removed in
    /// version 2.2 (ICF_025; `MDFF-QUAL-002`). Exact case.
    pub enum QualityFlag ("quality flag", case_insensitive = false) {
        /// `A`, Actual Metering Data. No method flag is required. A reason
        /// code is mandatory on every interval where the meter recorded a
        /// power outage (79), time reset (89) or tamper (61); other codes
        /// may be given.
        Actual => "A",
        /// `E`, forward estimated data. A method flag is mandatory.
        /// Appendix C says no reason code applies; the record tables say
        /// one is not required but may be given for information, and code
        /// 77 is meant for Estimations (`MDFF-QUAL-007`), so one is
        /// accepted.
        Estimated => "E",
        /// `F`, final substituted data. A method flag and a reason code are
        /// mandatory.
        Final => "F",
        /// `S`, substituted data. A method flag and a reason code are
        /// mandatory.
        Substituted => "S",
        /// `V`, variable data: not a quality flag held against any data
        /// item. Only the QualityMethod of a 300 record may be `V`, meaning
        /// quality, method or reason vary across the day and the 400 records
        /// that follow give them. No method flag, no reason code; never in
        /// NEM13.
        Variable => "V",
    }
}

impl QualityFlag {
    /// The "Meaning of Quality Flag" column of Appendix C, shortened.
    pub const fn meaning(self) -> &'static str {
        match self {
            Self::Actual => "Actual Metering Data",
            Self::Estimated => "Forward estimated data",
            Self::Final => "Final substituted data",
            Self::Substituted => "Substituted data",
            Self::Variable => "Variable data",
        }
    }

    /// Whether a method flag goes with this quality flag (`MDFF-QUAL-004`,
    /// `-005`, `-009`). For `A` the spec says only "not required".
    pub const fn method_flag(self) -> Presence {
        match self {
            Self::Actual => Presence::Optional,
            Self::Estimated | Self::Final | Self::Substituted => Presence::Required,
            Self::Variable => Presence::Forbidden,
        }
    }

    /// Whether a reason code goes with this quality flag (`MDFF-QUAL-006`
    /// to `-009`). For `A` it is optional except on intervals with codes
    /// 61, 79 or 89 ([`ReasonCode::mandatory_with_actual`]), which only the
    /// sender knows.
    pub const fn reason_code(self) -> Presence {
        match self {
            Self::Actual | Self::Estimated => Presence::Optional,
            Self::Final | Self::Substituted => Presence::Required,
            Self::Variable => Presence::Forbidden,
        }
    }

    /// Whether the flag may appear in this field: `V` only in a 300 record
    /// (`MDFF-QUAL-009`), and an Estimate never as the previous read of a
    /// 250 record (`MDFF-250-010`).
    pub const fn allowed_in(self, context: QualityContext) -> bool {
        match self {
            Self::Variable => matches!(context, QualityContext::IntervalData),
            Self::Estimated => !matches!(context, QualityContext::PreviousRead),
            Self::Actual | Self::Final | Self::Substituted => true,
        }
    }
}

/// A method flag: the two-digit number of a substitution or estimation
/// type in table 2.6 of Metrology Procedure: Part B (Appendix D, p.24;
/// `MDFF-METHOD-001`). Method flag 17 is substitution type 17.
///
/// The MDFF spec lists no values; the set of current types lives in the
/// Metrology Procedure, so this type checks only the form (two ASCII
/// digits). Flags that appear in the spec's own examples are 14, 52, 53,
/// 54, 56 and 64.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MethodFlag(u8);

impl MethodFlag {
    /// A method flag from its number, 0 to 99.
    pub const fn new(n: u8) -> Option<Self> {
        if n <= 99 { Some(Self(n)) } else { None }
    }

    /// The number.
    pub const fn value(self) -> u8 {
        self.0
    }
}

impl FromStr for MethodFlag {
    type Err = Error;

    /// Exactly two ASCII digits, e.g. `14` or `05`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.as_bytes() {
            &[a, b] if a.is_ascii_digit() && b.is_ascii_digit() => {
                Ok(Self((a - b'0') * 10 + (b - b'0')))
            }
            _ => Err(Error::Unknown {
                what: "method flag",
                text: s.to_owned(),
            }),
        }
    }
}

impl TryFrom<&str> for MethodFlag {
    type Error = Error;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl fmt::Display for MethodFlag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}", self.0)
    }
}

/// The QualityMethod field, `QMM`: a quality flag and, where it takes one,
/// a two-digit method flag (`MDFF-QUAL-003`). `A` and `V` are written
/// alone, `S14` is substituted by method 14.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct QualityMethod {
    flag: QualityFlag,
    method: Option<MethodFlag>,
}

impl QualityMethod {
    /// Joins a flag and a method flag, refusing a pair the flag's rules
    /// forbid ([`QualityFlag::method_flag`]).
    pub const fn new(flag: QualityFlag, method: Option<MethodFlag>) -> Result<Self, Error> {
        match (flag.method_flag(), method) {
            (Presence::Required, None) => Err(Error::MethodFlagRequired(flag)),
            (Presence::Forbidden, Some(_)) => Err(Error::MethodFlagNotAllowed(flag)),
            _ => Ok(Self { flag, method }),
        }
    }

    /// The quality flag.
    pub const fn flag(self) -> QualityFlag {
        self.flag
    }

    /// The method flag, if any.
    pub const fn method(self) -> Option<MethodFlag> {
        self.method
    }

    /// Checks that the flag may appear in this field
    /// ([`QualityFlag::allowed_in`]).
    pub const fn check_context(self, context: QualityContext) -> Result<(), Error> {
        if self.flag.allowed_in(context) {
            Ok(())
        } else {
            Err(Error::QualityNotAllowed {
                flag: self.flag,
                context,
            })
        }
    }

    /// Checks the reason code that goes with this QualityMethod against the
    /// flag's rule ([`QualityFlag::reason_code`]).
    pub const fn check_reason(self, reason: Option<ReasonCode>) -> Result<(), Error> {
        match (self.flag.reason_code(), reason) {
            (Presence::Required, None) => Err(Error::ReasonCodeRequired(self.flag)),
            (Presence::Forbidden, Some(_)) => Err(Error::ReasonCodeNotAllowed(self.flag)),
            _ => Ok(()),
        }
    }
}

impl FromStr for QualityMethod {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let unknown = || Error::Unknown {
            what: "QualityMethod",
            text: s.to_owned(),
        };
        let (flag, method) = match s.len() {
            1 => (s, None),
            3 if s.is_char_boundary(1) => (&s[..1], Some(&s[1..])),
            _ => return Err(unknown()),
        };
        let flag: QualityFlag = flag.parse().map_err(|_| unknown())?;
        let method = method
            .map(str::parse::<MethodFlag>)
            .transpose()
            .map_err(|_| unknown())?;
        Self::new(flag, method)
    }
}

impl TryFrom<&str> for QualityMethod {
    type Error = Error;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl fmt::Display for QualityMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.flag.as_str())?;
        match self.method {
            Some(m) => m.fmt(f),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags() {
        let all: Vec<_> = QualityFlag::ALL.iter().map(|q| q.as_str()).collect();
        assert_eq!(all, ["A", "E", "F", "S", "V"]);
        assert!("N".parse::<QualityFlag>().is_err());
        assert!("a".parse::<QualityFlag>().is_err());
        for &q in QualityFlag::ALL {
            assert!(!q.meaning().is_empty());
            assert_eq!(QualityFlag::try_from(q.as_str()), Ok(q));
        }
        use QualityContext::*;
        assert!(QualityFlag::Variable.allowed_in(IntervalData));
        assert_eq!(IntervalData.describe(), "the QualityMethod of a 300 record");
        for c in [IntervalEvent, PreviousRead, CurrentRead] {
            assert!(!QualityFlag::Variable.allowed_in(c));
            assert!(!c.describe().is_empty());
        }
        assert!(!QualityFlag::Estimated.allowed_in(PreviousRead));
        assert!(QualityFlag::Estimated.allowed_in(CurrentRead));
        assert!(QualityFlag::Final.allowed_in(PreviousRead));
    }

    #[test]
    fn method_flags() {
        assert_eq!("14".parse::<MethodFlag>().map(MethodFlag::value), Ok(14));
        assert_eq!(
            MethodFlag::try_from("05").map(|m| m.to_string()),
            Ok("05".to_owned())
        );
        assert_eq!(MethodFlag::new(99).map(MethodFlag::value), Some(99));
        assert_eq!(MethodFlag::new(100), None);
        for bad in ["5", "123", "1a", "", "é"] {
            assert!(bad.parse::<MethodFlag>().is_err(), "{bad}");
        }
    }

    #[test]
    fn quality_methods() {
        for text in ["A", "V", "S14", "F52", "E64", "A14"] {
            let qm: QualityMethod = text.parse().unwrap();
            assert_eq!(qm.to_string(), text);
            assert_eq!(QualityMethod::try_from(text), Ok(qm));
        }
        let s53: QualityMethod = "S53".parse().unwrap();
        assert_eq!(s53.flag(), QualityFlag::Substituted);
        assert_eq!(s53.method().map(MethodFlag::value), Some(53));
        assert_eq!(
            "S".parse::<QualityMethod>(),
            Err(Error::MethodFlagRequired(QualityFlag::Substituted))
        );
        assert_eq!(
            "V14".parse::<QualityMethod>(),
            Err(Error::MethodFlagNotAllowed(QualityFlag::Variable))
        );
        for bad in ["", "N", "S1", "S123", "X14", "Sab", "é1"] {
            assert!(
                matches!(bad.parse::<QualityMethod>(), Err(Error::Unknown { .. })),
                "{bad}"
            );
        }
    }

    #[test]
    fn reason_rules() {
        let rc = Some(ReasonCode::UnderInvestigation);
        let qm = |s: &str| s.parse::<QualityMethod>().unwrap();
        assert_eq!(
            qm("S53").check_reason(None),
            Err(Error::ReasonCodeRequired(QualityFlag::Substituted))
        );
        assert_eq!(
            qm("F14").check_reason(None),
            Err(Error::ReasonCodeRequired(QualityFlag::Final))
        );
        assert_eq!(qm("S53").check_reason(rc), Ok(()));
        assert_eq!(
            qm("V").check_reason(rc),
            Err(Error::ReasonCodeNotAllowed(QualityFlag::Variable))
        );
        assert_eq!(qm("V").check_reason(None), Ok(()));
        assert_eq!(qm("A").check_reason(None), Ok(()));
        assert_eq!(
            qm("A").check_reason(Some(ReasonCode::PowerOutageAlarm)),
            Ok(())
        );
        assert_eq!(
            qm("E52").check_reason(Some(ReasonCode::EstimationForecast)),
            Ok(())
        );
        assert_eq!(
            qm("V").check_context(QualityContext::IntervalEvent),
            Err(Error::QualityNotAllowed {
                flag: QualityFlag::Variable,
                context: QualityContext::IntervalEvent
            })
        );
        assert_eq!(qm("V").check_context(QualityContext::IntervalData), Ok(()));
        assert!(
            qm("E64")
                .check_context(QualityContext::PreviousRead)
                .is_err()
        );
    }
}
