//! Appendix E reason codes (p.25-30) and the obsolete codes of Appendix F
//! (p.31). Names are the spec's; descriptions are paraphrased.

use std::fmt;
use std::str::FromStr;

use crate::Error;

macro_rules! reason_codes {
    (
        current { $( $cvar:ident = $ccode:literal, $cname:literal, $cdesc:literal; )+ }
        obsolete { $( $ovar:ident = $ocode:literal, $oname:literal; )+ }
    ) => {
        /// A ReasonCode: why metering data was substituted or estimated, or
        /// information about it (`MDFF-REASON-001`). The obsolete codes of
        /// Appendix F are here too, for reading Historical Data
        /// ([`ReasonCode::is_obsolete`], `MDFF-REASON-002`).
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum ReasonCode {
            $(
                #[doc = concat!("`", stringify!($ccode), "` ", $cname, ": ", $cdesc)]
                $cvar,
            )+
            $(
                #[doc = concat!("`", stringify!($ocode), "` ", $oname, ". Obsolete (Appendix F): only for Historical Data.")]
                $ovar,
            )+
        }

        impl ReasonCode {
            /// Every code: Appendix E in order, then Appendix F in order.
            pub const ALL: &'static [Self] = &[$(Self::$cvar,)+ $(Self::$ovar,)+];

            /// The code's number.
            pub const fn code(self) -> u8 {
                match self {
                    $(Self::$cvar => $ccode,)+
                    $(Self::$ovar => $ocode,)+
                }
            }

            /// The code as written in the field, without leading zeros.
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$cvar => stringify!($ccode),)+
                    $(Self::$ovar => stringify!($ocode),)+
                }
            }

            /// The "Reason Code Description" column: the code's name.
            pub const fn name(self) -> &'static str {
                match self {
                    $(Self::$cvar => $cname,)+
                    $(Self::$ovar => $oname,)+
                }
            }

            /// The "Detailed Description" of Appendix E, paraphrased;
            /// `None` for an obsolete code, which Appendix F does not
            /// describe.
            pub const fn description(self) -> Option<&'static str> {
                match self {
                    $(Self::$cvar => Some($cdesc),)+
                    $(Self::$ovar => None,)+
                }
            }

            /// Whether the code is obsolete (Appendix F).
            pub const fn is_obsolete(self) -> bool {
                match self {
                    $(Self::$cvar => false,)+
                    $(Self::$ovar => true,)+
                }
            }

            /// The code with this number, current or obsolete.
            pub const fn from_code(code: u8) -> Option<Self> {
                match code {
                    $($ccode => Some(Self::$cvar),)+
                    $($ocode => Some(Self::$ovar),)+
                    _ => None,
                }
            }
        }
    };
}

reason_codes! {
    current {
        FreeText = 0, "Free text description", "When no other code can reasonably be used, e.g. system issues preventing collection or delivery of market data. Needs a ReasonDescription.";
        MeterEquipmentChanged = 1, "Meter/equipment changed", "The metering installation has changed.";
        ExtremeWeatherConditions = 2, "Extreme weather conditions", "Extreme weather (floods, storms, bushfires) prevented data collection.";
        QuarantinedPremises = 3, "Quarantined premises", "Premises under quarantine or lockdown prevent access (e.g. pandemic).";
        BlankScreen = 5, "Blank screen", "Electronic meter display is blank; may be powered off or faulty, cannot tell which.";
        DeEnergisedPremises = 6, "De-energised premises", "Blank electronic meter where the reader can tell the Site is de-energised, or a de-energised interval metered Site with Datastreams left active, for which the MDP substitutes.";
        UnableToLocateMeter = 7, "Unable to locate meter", "Site found, metering installation not.";
        VacantPremises = 8, "Vacant premises", "Reader believes the Site is vacant and could not access the meter.";
        UnderInvestigation = 9, "Under investigation", "An issue with the metering installation is under investigation.";
        LockDamagedUnableToOpen = 10, "Lock damaged unable to open", "A damaged lock prevents access to the metering installation.";
        InWrongRoute = 11, "In wrong route", "No reading because the installation is in the wrong route.";
        LockedPremises = 12, "Locked premises", "Site locked, no access to the metering installation.";
        LockedGate = 13, "Locked gate", "A locked gate prevents access to the metering installation.";
        LockedMeterBox = 14, "Locked meter box", "A locked meter box prevents access to the metering installation.";
        OvergrownVegetation = 15, "Overgrown vegetation", "Overgrown vegetation prevents access to the metering installation.";
        UnsafeEquipmentLocation = 17, "Unsafe equipment/location", "Equipment or location identified as unsafe (other than a high meter).";
        ReadLessThanPrevious = 18, "Read less than previous", "Current reading below the previous one, no sign of tampering, no reverse energy observed.";
        DamagedEquipmentPanel = 20, "Damaged equipment/panel", "Equipment or panel damaged but not identified as unsafe.";
        MainSwitchOff = 21, "Main switch off", "Blank electronic meter where the reader can tell the main switch is off, or an interval metered Site with the main switch off and Datastreams left active, for which the MDP substitutes.";
        MeterEquipmentSealsMissing = 22, "Meter/equipment seals missing", "One or more seals missing, no tampering identified.";
        ReaderError = 23, "Reader error", "The MDP found the reader's Meter Reading incorrect.";
        SubstitutedReplacedData = 24, "Substituted/replaced data (data correction)", "Interval Meter Reading: the MDP replaced erroneous data for specific intervals.";
        UnableToLocatePremises = 25, "Unable to locate premises", "The Site could not be found.";
        NegativeConsumption = 26, "Negative consumption (generation)", "Accumulation meter whose previous reading exceeds the current one; the Site usually has generation.";
        Rolr = 27, "RoLR", "Transfer of the End User because of a RoLR Event.";
        CtVtFault = 28, "CT/VT fault", "The MDP corrected data for a known instrument transformer (CT/VT) fault.";
        RelayFaultyDamaged = 29, "Relay faulty/damaged", "The reader found the relay device faulty.";
        NotAllMetersRead = 31, "Not all meters read", "The MDP has not received readings for every meter of the Site (typically a non-scheduled reading).";
        ReEnergisedWithoutReadings = 32, "Re-energised without readings", "No readings because of exceptional circumstances when the Site was re-energised outside standard practice.";
        DeEnergisedWithoutReadings = 33, "De-energised without readings", "No readings at de-energisation, including disconnection for non-payment.";
        MeterNotInHandheld = 34, "Meter not in handheld", "Unexpected (new or additional) meter found on Site.";
        TimeswitchFaultyResetRequired = 35, "Timeswitch faulty/reset required", "The reader found the time switching device faulty and needing a reset.";
        MeterHighLadderRequired = 36, "Meter high/ladder required", "The meter is too high to read without a ladder.";
        MeterUnderChurn = 37, "Meter under churn", "The MDP substituted because data was not received from the relevant MDP.";
        UnmarriedLock = 38, "Unmarried lock", "Two or more locks, one a power industry lock, not interlocked to allow access.";
        ReverseEnergyObserved = 39, "Reverse energy observed", "Reverse energy where the Site is not expected to have it.";
        UnrestrainedLivestock = 40, "Unrestrained livestock", "Roaming farm animals are a hazard, or access was not taken because they might escape.";
        FaultyMeterDisplayDials = 41, "Faulty Meter display/dials", "Display or dials faulty; the Site is not de-energised and the display is not blank.";
        ChannelAddedRemoved = 42, "Channel added/removed", "Data for an added channel, or substitutes for a removed channel whose Datastream is still active in MSATS.";
        PowerOutage = 43, "Power outage", "Interval meter: intervals substituted because power was not available at the installation.";
        MeterTesting = 44, "Meter testing", "Substitutes reflecting consumption while the meter was under test.";
        ReadingsFailedToValidate = 45, "Readings failed to validate", "Readings loaded but failed Validation and were substituted.";
        RefusedAccess = 47, "Refused access", "The End User refused access.";
        DogOnPremises = 48, "Dog on premises", "A dog on Site; the reader could not tell whether it is dangerous.";
        InstallationDemolished = 51, "Installation demolished", "The metering installation no longer exists.";
        AccessBlocked = 52, "Access – blocked", "Items block safe access to the meter or Site.";
        PestsInMeterBox = 53, "Pests in meter box", "Pests in the meter box risk accuracy, safety or the reader.";
        MeterBoxDamagedFaulty = 54, "Meter box damaged/faulty", "Meter box damaged or faulty; mechanical protection or weather proofing compromised.";
        DialsObscured = 55, "Dials obscured", "Dials obscured (painted over, misted, faded or mouldy panel); no evidence of tampering.";
        IllegalConnection = 60, "Illegal connection", "The Site has been illegally connected.";
        EquipmentTampered = 61, "Equipment tampered", "The installation was tampered with; recorded consumption may be affected.";
        NsrdWindowExpired = 62, "NSRD window expired", "The NSRD window expired without Actual Meter Readings.";
        KeyRequired = 64, "Key required", "The reader usually has the key but could not get it at the time.";
        WrongKeyProvided = 65, "Wrong key provided", "The key provided no longer opens the lock.";
        Transfer = 67, "Transfer", "Reading provided to enable a customer transfer.";
        ZeroConsumption = 68, "Zero consumption", "Known zero consumption; the Site is energised in MSATS but no energy flows to the meter.";
        ReadingExceedsSubstitute = 69, "Reading exceeds Substitute", "Re-substituted data smoothed to align with the next Actual Meter Reading.";
        ProbeReadError = 71, "Probe read error", "The meter probe could not extract the data.";
        RecalculatedBasedOnActual = 72, "Re-calculated based on Actual Metering Data", "Actual readings received; prior Substitutes amended.";
        LowConsumption = 73, "Low consumption", "Reading failed Validation as too low against Historical Data; kept as actual or substituted.";
        HighConsumption = 74, "High consumption", "Reading failed Validation as too high against Historical Data; kept as actual or substituted.";
        CustomerRead = 75, "Customer read", "Reading provided by the End User, where the jurisdiction allows it.";
        CommunicationsFault = 76, "Communications fault", "Remote communication with the meter failed.";
        EstimationForecast = 77, "Estimation Forecast", "Optional code for Estimations.";
        NullData = 78, "Null Data", "Interval meter: no data received; Substitutes cover the period.";
        PowerOutageAlarm = 79, "Power Outage Alarm", "The interval meter detected a power outage.";
        ShortIntervalAlarm = 80, "Short Interval Alarm", "The meter clock was slow and was corrected, so interval data is shorter than a full interval.";
        LongIntervalAlarm = 81, "Long Interval Alarm", "The meter clock was fast and was corrected, so interval data exceeds a full interval.";
        ResetOccurred = 87, "Reset occurred", "Meter reset for reprogramming, a configuration change, a firmware upgrade or the like.";
        TimeResetOccurred = 89, "Time reset occurred", "A time reset occurred within the metering installation.";
        IncorrectMeterMultiplier = 100, "Incorrect Meter Multiplier", "Correcting data after the wrong meter multiplier was applied.";
        TemporarilyUnmetered = 101, "Temporarily Connection Point unmetered", "The connection point was temporarily unmetered (e.g. mains by-pass).";
        CustomerBypass = 102, "Customer By-Pass", "The customer by-passed the meter.";
        NetworkBypass = 103, "Network By-Pass", "The network by-passed the meter, believing it faulty, to supply the customer.";
        TransposedChannel = 104, "Transposed Channel", "Meter data streams transposed (e.g. ToU with controlled load).";
        TransposedChannelUomCorrection = 105, "Transposed Channel - UoM Correction", "Data channels transposed across units (e.g. kWh with kvarh).";
        TransposedChannelReversePolarity = 106, "Transposed Channel – Reverse Polarity", "Meter wired in reverse, or a reverse polarity alarm, swapping export and import registers.";
        TransposedMeter = 107, "Transposed Meter", "Correcting data after crossed meters.";
        NetworkBypassExtremeWeather = 108, "Network by-pass extreme weather", "The network by-passed the meter because extreme weather affected it.";
        DefinedLoadMethod = 109, "Defined load method", "Retailer or LNSP provides profile data from an off-market meter or other measured data that best represents the load.";
    }
    obsolete {
        DangerousDog = 4, "Dangerous dog";
        NoxiousWeeds = 16, "Noxious weeds at premises are preventing access to metering installation";
        ConsumerWanted = 19, "Consumer wanted";
        MeterStopSwitchOn = 30, "Meter stop switch on";
        ExtremeWeatherHot = 46, "Extreme weather/hot";
        WetPaint = 49, "Wet paint";
        WrongTariff = 50, "Wrong tariff";
        MeterOkSupplyFailure = 58, "Meter ok – supply failure";
        ProbeReportsTampering = 70, "Probe reports tampering";
        CrcError = 82, "CRC error";
        RamChecksumError = 83, "RAM checksum error";
        RomChecksumError = 84, "ROM checksum error";
        DataMissingAlarm = 85, "Data missing alarm";
        ClockErrorAlarm = 86, "Clock error alarm";
        WatchdogTimeoutAlarm = 88, "Watchdog timeout alarm";
        TestMode = 90, "Test mode";
        LoadControl = 91, "Load control";
        AddedInterval = 92, "Added interval (data correction)";
        ReplacedInterval = 93, "Replaced interval (data correction)";
        EstimatedInterval = 94, "Estimated interval (data correction)";
        PulseOverflowAlarm = 95, "Pulse overflow alarm";
        DataOutOfLimits = 96, "Data out of limits";
        ExcludedData = 97, "Excluded data";
        ParityError = 98, "Parity error";
        EnergyTypeRegisterChanged = 99, "Energy type (register changed)";
    }
}

impl ReasonCode {
    /// Whether a ReasonDescription must accompany the code: only code 0,
    /// free text (`MDFF-REASON-004`).
    pub const fn requires_description(self) -> bool {
        matches!(self, Self::FreeText)
    }

    /// The meter events whose code is mandatory on Actual (`A`) intervals:
    /// tamper (61), power outage (79) and time reset (89). When they cover
    /// only part of an actual day, the 300 record is `V` with 400 records
    /// (`MDFF-QUAL-008`, `MDFF-300-011`).
    pub const fn mandatory_with_actual(self) -> bool {
        matches!(
            self,
            Self::EquipmentTampered | Self::PowerOutageAlarm | Self::TimeResetOccurred
        )
    }
}

impl FromStr for ReasonCode {
    type Err = Error;

    /// One to three ASCII digits naming a current or obsolete code. The
    /// spec does not say whether `Numeric(3)` allows leading zeros; `079`
    /// is read as 79.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let unknown = || Error::Unknown {
            what: "ReasonCode",
            text: s.to_owned(),
        };
        if s.is_empty() || s.len() > 3 || !s.bytes().all(|b| b.is_ascii_digit()) {
            return Err(unknown());
        }
        s.parse::<u8>()
            .ok()
            .and_then(Self::from_code)
            .ok_or_else(unknown)
    }
}

impl TryFrom<&str> for ReasonCode {
    type Error = Error;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl fmt::Display for ReasonCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn appendix_e_and_f() {
        let current: BTreeSet<u8> = ReasonCode::ALL
            .iter()
            .filter(|r| !r.is_obsolete())
            .map(|r| r.code())
            .collect();
        let obsolete: BTreeSet<u8> = ReasonCode::ALL
            .iter()
            .filter(|r| r.is_obsolete())
            .map(|r| r.code())
            .collect();
        assert_eq!(current.len(), 80);
        assert_eq!(obsolete.len(), 25);
        assert!(current.is_disjoint(&obsolete));
        let missing: Vec<u8> = (0..=109)
            .filter(|c| !current.contains(c) && !obsolete.contains(c))
            .collect();
        assert_eq!(missing, [56, 57, 59, 63, 66]);
        for &r in ReasonCode::ALL {
            assert_eq!(ReasonCode::from_code(r.code()), Some(r));
            assert_eq!(r.as_str(), r.code().to_string());
            assert_eq!(r.to_string().parse(), Ok(r));
            assert_eq!(ReasonCode::try_from(r.as_str()), Ok(r));
            assert!(!r.name().is_empty());
            assert_eq!(r.description().is_none(), r.is_obsolete());
        }
    }

    #[test]
    fn parsing_and_rules() {
        assert_eq!("079".parse(), Ok(ReasonCode::PowerOutageAlarm));
        assert_eq!("0".parse(), Ok(ReasonCode::FreeText));
        for bad in ["", "56", "110", "1000", "-1", "7a", "255", "256"] {
            assert!(bad.parse::<ReasonCode>().is_err(), "{bad}");
        }
        assert!(ReasonCode::FreeText.requires_description());
        assert!(!ReasonCode::UnderInvestigation.requires_description());
        let events: Vec<u8> = ReasonCode::ALL
            .iter()
            .filter(|r| r.mandatory_with_actual())
            .map(|r| r.code())
            .collect();
        assert_eq!(events, [61, 79, 89]);
        assert_eq!(ReasonCode::AccessBlocked.name(), "Access – blocked");
        assert_eq!(ReasonCode::WetPaint.name(), "Wet paint");
    }
}
