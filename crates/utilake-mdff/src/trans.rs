//! Appendix A: transaction codes, the TransCode of the 500 record and the
//! PreviousTransCode and CurrentTransCode of the 550 record.

code_enum! {
    /// Why the recipient is receiving the metering data (Appendix A, p.21).
    /// Service orders here are work done by an MP, MDP, ENM or LNSP for a
    /// retailer's ServiceOrderRequest or on its own initiative. Exact case
    /// (`MDFF-FILE-016`; §4.6 asks for "capital letter O").
    ///
    /// Example I.4 of the spec (p.44) also sends an obsolete code `T` for
    /// Historical Data; Appendix A neither lists it nor gives its meaning,
    /// so it is not a value here (`MDFF-TRANS-003`).
    pub enum TransCode ("TransCode", case_insensitive = false) {
        /// `A` Alteration: any alteration of the metering installation,
        /// including replacing a meter, every new connection and "Add/Alts"
        /// service orders.
        Alteration => "A",
        /// `C` Meter Reconfiguration: a "Meter Reconfiguration" service
        /// order, including off-peak (controlled load) timing changes; not
        /// the removal of the meter.
        MeterReconfiguration => "C",
        /// `G` Re-energisation: a "Re-energisation" service order.
        ReEnergisation => "G",
        /// `D` De-energisation: a "De-energisation" service order,
        /// including de-energisation for non-payment.
        DeEnergisation => "D",
        /// `E` Estimate: every Estimate.
        Estimate => "E",
        /// `N` Normal Read: scheduled collection of metering data, with its
        /// Substitutions.
        NormalRead => "N",
        /// `O` Other: "Meter Investigation" and "Miscellaneous" service
        /// orders; also used for Historical Data where the TransCode is
        /// unavailable (`MDFF-TRANS-002`).
        Other => "O",
        /// `S` Special Read: a "Special Read" service order.
        SpecialRead => "S",
        /// `R` Removal of meter: meter removed or supply abolished with no
        /// replacement; a changeover or an added meter is an Alteration.
        RemovalOfMeter => "R",
    }
}

impl TransCode {
    /// The code to send with Historical Data when the real one is unknown.
    pub const HISTORICAL_UNKNOWN: Self = Self::Other;

    /// The "Action" column of Appendix A.
    pub const fn action(self) -> &'static str {
        match self {
            Self::Alteration => "Alteration",
            Self::MeterReconfiguration => "Meter Reconfiguration",
            Self::ReEnergisation => "Re-energisation",
            Self::DeEnergisation => "De-energisation",
            Self::Estimate => "Estimate",
            Self::NormalRead => "Normal Read",
            Self::Other => "Other",
            Self::SpecialRead => "Special Read",
            Self::RemovalOfMeter => "Removal of meter",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_code_round_trips_and_case_matters() {
        assert_eq!(TransCode::ALL.len(), 9);
        for &t in TransCode::ALL {
            assert_eq!(t.as_str().parse::<TransCode>(), Ok(t));
            assert_eq!(TransCode::try_from(t.to_string().as_str()), Ok(t));
            assert!(!t.action().is_empty());
            assert!(t.as_str().to_lowercase().parse::<TransCode>().is_err());
        }
        assert_eq!(TransCode::HISTORICAL_UNKNOWN.as_str(), "O");
        assert!("T".parse::<TransCode>().is_err());
        assert!("0".parse::<TransCode>().is_err());
        assert!("".parse::<TransCode>().is_err());
    }
}
