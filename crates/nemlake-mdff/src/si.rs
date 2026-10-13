//! Conversion of MDFF values to SI units and back.
//!
//! Energy is converted from watt hours to joules: the hour is not an SI
//! unit, and the joule is the SI unit of energy (1 Wh = 3600 J). Power,
//! voltage and current are SI already (W, V, A) and lose only their
//! prefix.
//!
//! The var and the volt ampere are not SI units: they are the names IEC
//! 60050 and ISO 80000-6 give to the watt when it measures reactive and
//! apparent power, so that the three powers are not confused. They keep
//! their names here (`var`, `VA`); reactive and apparent energy become
//! `var·s` and `VA·s`, the hour turned into seconds as for active energy.
//! Power factor is a ratio and has no unit.
//!
//! The factors are exact decimals (3600, 3 600 000, 1000 …), all exactly
//! representable in `f64`, so a conversion rounds once. Going there and
//! back may differ from the start in the last bit; written in the unit's
//! [`NumberFormat`](crate::NumberFormat) it is the same text.

use crate::{Error, Uom};

/// What a unit measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QuantityKind {
    /// Active energy: Wh, kWh, MWh.
    ActiveEnergy,
    /// Reactive energy: VArh, kVArh, MVArh.
    ReactiveEnergy,
    /// Apparent energy: VAh, kVAh, MVAh.
    ApparentEnergy,
    /// Active power: W, kW, MW.
    ActivePower,
    /// Reactive power: VAr, kVAr, MVAr.
    ReactivePower,
    /// Apparent power: VA, kVA, MVA.
    ApparentPower,
    /// Voltage: V, kV.
    Voltage,
    /// Current: A, kA.
    Current,
    /// Power factor: pf.
    PowerFactor,
}

impl QuantityKind {
    /// The SI unit (or IEC special name) this kind converts to.
    pub const fn si_unit(self) -> SiUnit {
        match self {
            Self::ActiveEnergy => SiUnit::Joule,
            Self::ReactiveEnergy => SiUnit::VarSecond,
            Self::ApparentEnergy => SiUnit::VoltAmpereSecond,
            Self::ActivePower => SiUnit::Watt,
            Self::ReactivePower => SiUnit::Var,
            Self::ApparentPower => SiUnit::VoltAmpere,
            Self::Voltage => SiUnit::Volt,
            Self::Current => SiUnit::Ampere,
            Self::PowerFactor => SiUnit::One,
        }
    }

    /// Whether the kind is an energy (a quantity over time).
    pub const fn is_energy(self) -> bool {
        matches!(
            self,
            Self::ActiveEnergy | Self::ReactiveEnergy | Self::ApparentEnergy
        )
    }
}

/// The unit of an [`SiQuantity`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SiUnit {
    /// joule, J
    Joule,
    /// var second, var·s (reactive energy)
    VarSecond,
    /// volt ampere second, VA·s (apparent energy)
    VoltAmpereSecond,
    /// watt, W
    Watt,
    /// var (reactive power; the IEC name of the watt for it)
    Var,
    /// volt ampere, VA (apparent power; the IEC name of the watt for it)
    VoltAmpere,
    /// volt, V
    Volt,
    /// ampere, A
    Ampere,
    /// dimensionless, 1
    One,
}

impl SiUnit {
    /// The unit's symbol.
    pub const fn symbol(self) -> &'static str {
        match self {
            Self::Joule => "J",
            Self::VarSecond => "var·s",
            Self::VoltAmpereSecond => "VA·s",
            Self::Watt => "W",
            Self::Var => "var",
            Self::VoltAmpere => "VA",
            Self::Volt => "V",
            Self::Ampere => "A",
            Self::One => "1",
        }
    }
}

/// A value in an SI unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SiQuantity {
    /// The number.
    pub value: f64,
    /// Its unit.
    pub unit: SiUnit,
}

impl Uom {
    /// How many SI units one of this unit is: 3600 J in a Wh, 3.6 × 10⁹ J
    /// in a MWh, 1000 W in a kW, 1 for pf. Exact in `f64`.
    pub const fn si_factor(self) -> f64 {
        let per_base = if self.kind().is_energy() { 3600.0 } else { 1.0 };
        self.prefix_factor() * per_base
    }

    /// The SI unit this unit converts to.
    pub const fn si_unit(self) -> SiUnit {
        self.kind().si_unit()
    }

    /// A value in this unit, in SI: `1.0` kWh is 3 600 000 J.
    pub fn to_si(self, value: f64) -> SiQuantity {
        SiQuantity {
            value: value * self.si_factor(),
            unit: self.si_unit(),
        }
    }

    /// An SI quantity, in this unit: 3 600 000 J is `1.0` kWh. Fails if
    /// the quantity is of another kind (joules into kW).
    pub fn from_si(self, quantity: SiQuantity) -> Result<f64, Error> {
        if quantity.unit == self.si_unit() {
            Ok(quantity.value / self.si_factor())
        } else {
            Err(Error::UnitMismatch {
                expected: self.si_unit(),
                found: quantity.unit,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factors() {
        let cases = [
            ("Wh", 3600.0, SiUnit::Joule),
            ("kWh", 3_600_000.0, SiUnit::Joule),
            ("MWh", 3_600_000_000.0, SiUnit::Joule),
            ("VArh", 3600.0, SiUnit::VarSecond),
            ("kVArh", 3_600_000.0, SiUnit::VarSecond),
            ("MVArh", 3_600_000_000.0, SiUnit::VarSecond),
            ("VAh", 3600.0, SiUnit::VoltAmpereSecond),
            ("kVAh", 3_600_000.0, SiUnit::VoltAmpereSecond),
            ("MVAh", 3_600_000_000.0, SiUnit::VoltAmpereSecond),
            ("W", 1.0, SiUnit::Watt),
            ("kW", 1000.0, SiUnit::Watt),
            ("MW", 1_000_000.0, SiUnit::Watt),
            ("VAr", 1.0, SiUnit::Var),
            ("kVAr", 1000.0, SiUnit::Var),
            ("MVAr", 1_000_000.0, SiUnit::Var),
            ("VA", 1.0, SiUnit::VoltAmpere),
            ("kVA", 1000.0, SiUnit::VoltAmpere),
            ("MVA", 1_000_000.0, SiUnit::VoltAmpere),
            ("V", 1.0, SiUnit::Volt),
            ("kV", 1000.0, SiUnit::Volt),
            ("A", 1.0, SiUnit::Ampere),
            ("kA", 1000.0, SiUnit::Ampere),
            ("pf", 1.0, SiUnit::One),
        ];
        assert_eq!(cases.len(), Uom::ALL.len());
        for (text, factor, unit) in cases {
            let u: Uom = text.parse().unwrap();
            assert_eq!(u.si_factor(), factor, "{text}");
            assert_eq!(u.si_unit(), unit, "{text}");
            assert_eq!(
                u.to_si(1.0),
                SiQuantity {
                    value: factor,
                    unit
                }
            );
            assert!(!unit.symbol().is_empty());
        }
    }

    #[test]
    fn examples() {
        assert_eq!(Uom::KilowattHour.to_si(1.5).value, 5_400_000.0);
        assert_eq!(Uom::WattHour.to_si(1.0).value, 3600.0);
        let j = SiQuantity {
            value: 3_600_000.0,
            unit: SiUnit::Joule,
        };
        assert_eq!(Uom::KilowattHour.from_si(j), Ok(1.0));
        assert_eq!(Uom::MegawattHour.from_si(j), Ok(0.001));
        assert_eq!(
            Uom::Kilowatt.from_si(j),
            Err(Error::UnitMismatch {
                expected: SiUnit::Watt,
                found: SiUnit::Joule
            })
        );
        assert!(!QuantityKind::Voltage.is_energy());
    }

    #[test]
    fn round_trips_keep_the_written_value() {
        // Values as they appear in MDFF files, through SI and back, then
        // written in the unit's format: the text is unchanged.
        let samples = [
            "0",
            "0.1",
            "1.1112",
            "2.312",
            "343.5",
            "2311.765",
            "0.955",
            "1312.1",
            "1234567.1234567",
        ];
        for &u in Uom::ALL {
            let f = u.number_format();
            for text in samples {
                let Ok(v) = f.parse(text) else { continue };
                let back = u.from_si(u.to_si(v)).unwrap();
                assert_eq!(f.format(back).unwrap(), f.format(v).unwrap(), "{u} {text}");
                assert!((back - v).abs() <= v.abs() * f64::EPSILON, "{u} {text}");
                let (base, bu) = u.to_base(v);
                assert_eq!(bu, u.base());
                assert!(
                    (u.from_base(base) - v).abs() <= v.abs() * f64::EPSILON,
                    "{u} {text}"
                );
            }
        }
    }
}
