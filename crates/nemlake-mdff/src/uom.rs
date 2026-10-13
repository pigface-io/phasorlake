//! Appendix B: units of measure, and the number format of IntervalValue
//! (300 record) and Quantity (250 record) for each unit.

use crate::NumberError;
use crate::si::QuantityKind;

code_enum! {
    /// The UOM of a 200 or 250 record (Appendix B, p.22; `MDFF-UOM-001`).
    /// Not case sensitive (`MDFF-UOM-002`): `kwh` reads as kWh and `mwh` as
    /// MWh; there are no milli units, so case-insensitive reading is never
    /// ambiguous.
    pub enum Uom ("UOM", case_insensitive = true) {
        /// megawatt hour
        MegawattHour => "MWh",
        /// kilowatt hour
        KilowattHour => "kWh",
        /// watt hour
        WattHour => "Wh",
        /// megavolt ampere reactive hour (megavar hour)
        MegavarHour => "MVArh",
        /// kilovolt ampere reactive hour
        KilovarHour => "kVArh",
        /// volt ampere reactive hour
        VarHour => "VArh",
        /// megavolt ampere reactive
        Megavar => "MVAr",
        /// kilovolt ampere reactive
        Kilovar => "kVAr",
        /// volt ampere reactive
        Var => "VAr",
        /// megawatt
        Megawatt => "MW",
        /// kilowatt
        Kilowatt => "kW",
        /// watt
        Watt => "W",
        /// megavolt ampere hour
        MegavoltAmpereHour => "MVAh",
        /// kilovolt ampere hour
        KilovoltAmpereHour => "kVAh",
        /// volt ampere hour
        VoltAmpereHour => "VAh",
        /// megavolt ampere
        MegavoltAmpere => "MVA",
        /// kilovolt ampere
        KilovoltAmpere => "kVA",
        /// volt ampere
        VoltAmpere => "VA",
        /// kilovolt
        Kilovolt => "kV",
        /// volt
        Volt => "V",
        /// kiloampere
        Kiloampere => "kA",
        /// ampere
        Ampere => "A",
        /// Power Factor (dimensionless)
        PowerFactor => "pf",
    }
}

impl Uom {
    /// The Appendix B description.
    pub const fn description(self) -> &'static str {
        match self {
            Self::MegawattHour => "megawatt hour",
            Self::KilowattHour => "kilowatt hour",
            Self::WattHour => "watt hour",
            Self::MegavarHour => "megavolt ampere reactive hour (megavar hour)",
            Self::KilovarHour => "kilovolt ampere reactive hour",
            Self::VarHour => "volt ampere reactive hour",
            Self::Megavar => "megavolt ampere reactive",
            Self::Kilovar => "kilovolt ampere reactive",
            Self::Var => "volt ampere reactive",
            Self::Megawatt => "megawatt",
            Self::Kilowatt => "kilowatt",
            Self::Watt => "watt",
            Self::MegavoltAmpereHour => "megavolt ampere hour",
            Self::KilovoltAmpereHour => "kilovolt ampere hour",
            Self::VoltAmpereHour => "volt ampere hour",
            Self::MegavoltAmpere => "megavolt ampere",
            Self::KilovoltAmpere => "kilovolt ampere",
            Self::VoltAmpere => "volt ampere",
            Self::Kilovolt => "kilovolt",
            Self::Volt => "volt",
            Self::Kiloampere => "kiloampere",
            Self::Ampere => "ampere",
            Self::PowerFactor => "Power Factor",
        }
    }

    /// The unprefixed unit the industry works in: `Wh` for `kWh` and `MWh`,
    /// `VArh` for `kVArh`, `W` for `MW`, and so on. A base unit is its own
    /// base.
    pub const fn base(self) -> Self {
        match self {
            Self::MegawattHour | Self::KilowattHour | Self::WattHour => Self::WattHour,
            Self::MegavarHour | Self::KilovarHour | Self::VarHour => Self::VarHour,
            Self::Megavar | Self::Kilovar | Self::Var => Self::Var,
            Self::Megawatt | Self::Kilowatt | Self::Watt => Self::Watt,
            Self::MegavoltAmpereHour | Self::KilovoltAmpereHour | Self::VoltAmpereHour => {
                Self::VoltAmpereHour
            }
            Self::MegavoltAmpere | Self::KilovoltAmpere | Self::VoltAmpere => Self::VoltAmpere,
            Self::Kilovolt | Self::Volt => Self::Volt,
            Self::Kiloampere | Self::Ampere => Self::Ampere,
            Self::PowerFactor => Self::PowerFactor,
        }
    }

    /// The decimal prefix: how many base units one of this unit is
    /// (1 000 000 for mega, 1 000 for kilo, 1 otherwise). Exact in `f64`.
    pub const fn prefix_factor(self) -> f64 {
        match self {
            Self::MegawattHour
            | Self::MegavarHour
            | Self::Megavar
            | Self::Megawatt
            | Self::MegavoltAmpereHour
            | Self::MegavoltAmpere => 1_000_000.0,
            Self::KilowattHour
            | Self::KilovarHour
            | Self::Kilovar
            | Self::Kilowatt
            | Self::KilovoltAmpereHour
            | Self::KilovoltAmpere
            | Self::Kilovolt
            | Self::Kiloampere => 1_000.0,
            Self::WattHour
            | Self::VarHour
            | Self::Var
            | Self::Watt
            | Self::VoltAmpereHour
            | Self::VoltAmpere
            | Self::Volt
            | Self::Ampere
            | Self::PowerFactor => 1.0,
        }
    }

    /// What the unit measures.
    pub const fn kind(self) -> QuantityKind {
        match self.base() {
            Self::WattHour => QuantityKind::ActiveEnergy,
            Self::VarHour => QuantityKind::ReactiveEnergy,
            Self::VoltAmpereHour => QuantityKind::ApparentEnergy,
            Self::Watt => QuantityKind::ActivePower,
            Self::Var => QuantityKind::ReactivePower,
            Self::VoltAmpere => QuantityKind::ApparentPower,
            Self::Volt => QuantityKind::Voltage,
            Self::Ampere => QuantityKind::Current,
            _ => QuantityKind::PowerFactor,
        }
    }

    /// The Appendix B format of an IntervalValue or Quantity in this unit
    /// (`MDFF-NUM-001`): `M…` units 15.7, `k…` units 15.4, `pf` 15.3, the
    /// rest 15.1.
    pub const fn number_format(self) -> NumberFormat {
        if matches!(self, Self::PowerFactor) {
            NumberFormat::PF
        } else if self.prefix_factor() == 1_000_000.0 {
            NumberFormat::MEGA
        } else if self.prefix_factor() == 1_000.0 {
            NumberFormat::KILO
        } else {
            NumberFormat::BASE
        }
    }

    /// A value in this unit, in the unprefixed base unit ([`Uom::base`]):
    /// `1.5` kWh is `(1500.0, Wh)`.
    pub fn to_base(self, value: f64) -> (f64, Self) {
        (value * self.prefix_factor(), self.base())
    }

    /// A value in the base unit ([`Uom::base`]), in this unit: `1500.0` Wh
    /// is `1.5` kWh.
    pub fn from_base(self, value: f64) -> f64 {
        value / self.prefix_factor()
    }
}

/// The format of an IntervalValue or Quantity: at most `max_len`
/// characters and at most `max_decimals` digits after the decimal point,
/// no sign, no exponent (Appendix B, spec §4.4 and §5.3).
///
/// Appendix B writes these as "15.7" under "CHARACTER LENGTH", and the
/// record tables as `Numeric(sx.y)`, without defining either. Two readings
/// fit: 15 characters including the decimal point, or 15 digits (as SQL's
/// `NUMERIC(15,7)`). This type takes the first, the stricter: every value it
/// writes or accepts fits both. Decimals are a maximum, not an exact count;
/// the spec's own examples use fewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NumberFormat {
    /// The most characters the field may hold.
    pub max_len: u8,
    /// The most digits after the decimal point.
    pub max_decimals: u8,
}

impl NumberFormat {
    /// `M…` (mega) units: 15.7.
    pub const MEGA: Self = Self {
        max_len: 15,
        max_decimals: 7,
    };
    /// `k…` (kilo) units: 15.4.
    pub const KILO: Self = Self {
        max_len: 15,
        max_decimals: 4,
    };
    /// `pf`: 15.3.
    pub const PF: Self = Self {
        max_len: 15,
        max_decimals: 3,
    };
    /// `Wh`, `VArh`, `VAh`, `VAr`, `VA`, `V`, `A`, `W`: 15.1.
    pub const BASE: Self = Self {
        max_len: 15,
        max_decimals: 1,
    };

    /// Checks a field's text: digits, optionally a decimal point and more
    /// digits, within the length and decimals of the format.
    pub fn validate(self, text: &str) -> Result<(), NumberError> {
        let bytes = text.as_bytes();
        if bytes.is_empty() {
            return Err(NumberError::Empty);
        }
        if bytes[0] == b'-' {
            return Err(NumberError::Negative);
        }
        if bytes.iter().any(|&b| b == b'e' || b == b'E') {
            return Err(NumberError::Exponent);
        }
        let (int, frac) = match text.split_once('.') {
            Some((i, f)) => (i, Some(f)),
            None => (text, None),
        };
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        if !digits(int) || frac.is_some_and(|f| !digits(f)) {
            return Err(NumberError::Invalid);
        }
        if text.len() > usize::from(self.max_len) {
            return Err(NumberError::TooLong {
                max: self.max_len,
                found: text.len(),
            });
        }
        let decimals = frac.map_or(0, str::len);
        if decimals > usize::from(self.max_decimals) {
            return Err(NumberError::TooManyDecimals {
                max: self.max_decimals,
                found: decimals,
            });
        }
        Ok(())
    }

    /// Reads a field's text as a number after [`NumberFormat::validate`].
    pub fn parse(self, text: &str) -> Result<f64, NumberError> {
        self.validate(text)?;
        text.parse().map_err(|_| NumberError::Invalid)
    }

    /// Writes a value in this format: rounded to `max_decimals` places,
    /// trailing zeros and a bare decimal point dropped (`1.5`, `2`, `0`).
    /// Fails if the value is negative, not finite, or too long once
    /// written.
    pub fn format(self, value: f64) -> Result<String, NumberError> {
        if !value.is_finite() {
            return Err(NumberError::NotFinite);
        }
        if value < 0.0 {
            return Err(NumberError::Negative);
        }
        // -0.0 is not below zero but would print a sign.
        let value = value.abs();
        let mut text = format!("{value:.prec$}", prec = usize::from(self.max_decimals));
        if text.contains('.') {
            let kept = text.trim_end_matches('0').trim_end_matches('.').len();
            text.truncate(kept);
        }
        if text.len() > usize::from(self.max_len) {
            return Err(NumberError::TooLong {
                max: self.max_len,
                found: text.len(),
            });
        }
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_round_trip_without_case() {
        assert_eq!(Uom::ALL.len(), 23);
        for &u in Uom::ALL {
            assert_eq!(u.as_str().parse(), Ok(u));
            assert_eq!(u.as_str().to_uppercase().parse(), Ok(u));
            assert_eq!(u.as_str().to_lowercase().parse(), Ok(u));
            assert_eq!(Uom::try_from(u.to_string().as_str()), Ok(u));
            assert!(!u.description().is_empty());
            assert_eq!(u.base().base(), u.base());
            assert_eq!(u.base().prefix_factor(), 1.0);
            assert_eq!(u.base().kind(), u.kind());
        }
        assert_eq!("mwh".parse(), Ok(Uom::MegawattHour));
        assert!("kwhr".parse::<Uom>().is_err());
        assert!("GWh".parse::<Uom>().is_err());
    }

    #[test]
    fn formats_by_unit() {
        let fmt = |s: &str| s.parse::<Uom>().unwrap().number_format();
        for u in ["MWh", "MVArh", "MVAr", "MW", "MVAh", "MVA"] {
            assert_eq!(fmt(u), NumberFormat::MEGA, "{u}");
        }
        for u in ["kWh", "kVArh", "kVAr", "kW", "kVAh", "kVA", "kV", "kA"] {
            assert_eq!(fmt(u), NumberFormat::KILO, "{u}");
        }
        for u in ["Wh", "VArh", "VAh", "VAr", "VA", "V", "A", "W"] {
            assert_eq!(fmt(u), NumberFormat::BASE, "{u}");
        }
        assert_eq!(fmt("pf"), NumberFormat::PF);
    }

    #[test]
    fn base_units() {
        assert_eq!(Uom::KilowattHour.to_base(1.5), (1500.0, Uom::WattHour));
        assert_eq!(Uom::MegavarHour.to_base(2.0), (2_000_000.0, Uom::VarHour));
        assert_eq!(Uom::Kiloampere.to_base(0.25), (250.0, Uom::Ampere));
        assert_eq!(Uom::PowerFactor.to_base(0.95), (0.95, Uom::PowerFactor));
        assert_eq!(Uom::KilowattHour.from_base(1500.0), 1.5);
        assert_eq!(Uom::Megawatt.from_base(2_500_000.0), 2.5);
    }

    #[test]
    fn validate() {
        let k = NumberFormat::KILO;
        for ok in ["0", "1.1112", "123456789.1234", "1234567890.123", "007"] {
            assert_eq!(k.validate(ok), Ok(()), "{ok}");
        }
        assert_eq!(k.validate(""), Err(NumberError::Empty));
        assert_eq!(k.validate("-1"), Err(NumberError::Negative));
        assert_eq!(k.validate("1.5E3"), Err(NumberError::Exponent));
        assert_eq!(k.validate("2e-1"), Err(NumberError::Exponent));
        for bad in ["+1", ".5", "5.", "1.2.3", "1,5", " 1", "abc"] {
            assert_eq!(k.validate(bad), Err(NumberError::Invalid), "{bad}");
        }
        assert_eq!(
            k.validate("1.12345"),
            Err(NumberError::TooManyDecimals { max: 4, found: 5 })
        );
        assert_eq!(
            k.validate("12345678901.1234"),
            Err(NumberError::TooLong { max: 15, found: 16 })
        );
        assert_eq!(
            NumberFormat::BASE.validate("12.34"),
            Err(NumberError::TooManyDecimals { max: 1, found: 2 })
        );
        assert_eq!(NumberFormat::MEGA.validate("0.1234567"), Ok(()));
        assert_eq!(NumberFormat::PF.validate("0.955"), Ok(()));
        assert_eq!(k.parse("1.25"), Ok(1.25));
        assert_eq!(k.parse("x"), Err(NumberError::Invalid));
    }

    #[test]
    fn format() {
        let k = NumberFormat::KILO;
        assert_eq!(k.format(1.5).as_deref(), Ok("1.5"));
        assert_eq!(k.format(2.0).as_deref(), Ok("2"));
        assert_eq!(k.format(0.0).as_deref(), Ok("0"));
        assert_eq!(k.format(-0.0).as_deref(), Ok("0"));
        assert_eq!(k.format(0.00001).as_deref(), Ok("0"));
        assert_eq!(k.format(1.23456).as_deref(), Ok("1.2346"));
        assert_eq!(k.format(100.0).as_deref(), Ok("100"));
        assert_eq!(NumberFormat::BASE.format(12.34).as_deref(), Ok("12.3"));
        assert_eq!(
            NumberFormat::MEGA.format(0.123_456_78).as_deref(),
            Ok("0.1234568")
        );
        assert_eq!(k.format(-1.0), Err(NumberError::Negative));
        assert_eq!(k.format(f64::NAN), Err(NumberError::NotFinite));
        assert_eq!(k.format(f64::INFINITY), Err(NumberError::NotFinite));
        assert_eq!(
            k.format(1e20),
            Err(NumberError::TooLong { max: 15, found: 21 })
        );
        // Every value written is valid in its own format.
        for &u in Uom::ALL {
            let f = u.number_format();
            for v in [0.0, 0.1, 1.1112, 343.5, 2311.765, 123_456.789_012_3] {
                let text = f.format(v).unwrap();
                assert_eq!(f.validate(&text), Ok(()), "{u} {v} {text}");
            }
        }
    }
}
