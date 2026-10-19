//! Record indicators, the VersionHeader and the IntervalLength.

code_enum! {
    /// The format of a file, from the VersionHeader of its 100 record
    /// (§3.1, p.6; `MDFF-100-002`). Exact case.
    pub enum VersionHeader ("VersionHeader", case_insensitive = false) {
        /// `NEM12`: interval metering data.
        Nem12 => "NEM12",
        /// `NEM13`: accumulated metering data.
        Nem13 => "NEM13",
    }
}

impl VersionHeader {
    /// Whether a record of this kind may appear in a file of this format
    /// (`MDFF-FILE-004`).
    pub const fn allows(self, record: RecordIndicator) -> bool {
        match record {
            RecordIndicator::Header | RecordIndicator::End => true,
            RecordIndicator::NmiDataDetails
            | RecordIndicator::IntervalData
            | RecordIndicator::IntervalEvent
            | RecordIndicator::B2bDetails => matches!(self, Self::Nem12),
            RecordIndicator::AccumulationMeterData | RecordIndicator::AccumulationB2bDetails => {
                matches!(self, Self::Nem13)
            }
        }
    }
}

code_enum! {
    /// The RecordIndicator, the first field of every record.
    pub enum RecordIndicator ("RecordIndicator", case_insensitive = false) {
        /// `100` header, NEM12 and NEM13 (§4.2, §5.2).
        Header => "100",
        /// `200` NMI data details, NEM12 (§4.3).
        NmiDataDetails => "200",
        /// `300` interval data, NEM12 (§4.4).
        IntervalData => "300",
        /// `400` interval event, NEM12 (§4.5).
        IntervalEvent => "400",
        /// `500` B2B details, NEM12 (§4.6).
        B2bDetails => "500",
        /// `250` accumulation meter data, NEM13 (§5.3).
        AccumulationMeterData => "250",
        /// `550` B2B details, NEM13 (§5.4).
        AccumulationB2bDetails => "550",
        /// `900` end of data, NEM12 and NEM13 (§4.7, §5.5).
        End => "900",
    }
}

impl RecordIndicator {
    /// The indicator as a number.
    pub const fn code(self) -> u16 {
        match self {
            Self::Header => 100,
            Self::NmiDataDetails => 200,
            Self::IntervalData => 300,
            Self::IntervalEvent => 400,
            Self::B2bDetails => 500,
            Self::AccumulationMeterData => 250,
            Self::AccumulationB2bDetails => 550,
            Self::End => 900,
        }
    }

    /// The number of fields in the record, including the indicator. A 300
    /// record's count depends on its IntervalLength
    /// ([`IntervalLength::fields_in_300`]), so it gives `None`.
    pub const fn field_count(self) -> Option<usize> {
        match self {
            Self::Header | Self::B2bDetails | Self::AccumulationB2bDetails => Some(5),
            Self::NmiDataDetails => Some(10),
            Self::IntervalData => None,
            Self::IntervalEvent => Some(6),
            Self::AccumulationMeterData => Some(23),
            Self::End => Some(1),
        }
    }
}

/// The minutes of each interval (200 record, `MDFF-200-011`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IntervalLength {
    /// 5 minutes: 288 intervals a day.
    Five,
    /// 15 minutes: 96 intervals a day.
    Fifteen,
    /// 30 minutes: 48 intervals a day.
    Thirty,
}

impl IntervalLength {
    /// Every value.
    pub const ALL: &'static [Self] = &[Self::Five, Self::Fifteen, Self::Thirty];

    /// The length in minutes.
    pub const fn minutes(self) -> u32 {
        match self {
            Self::Five => 5,
            Self::Fifteen => 15,
            Self::Thirty => 30,
        }
    }

    /// The field as written. The spec does not say whether `05` is
    /// allowed; this crate reads and writes only `5`, `15` and `30`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Five => "5",
            Self::Fifteen => "15",
            Self::Thirty => "30",
        }
    }

    /// From a number of minutes.
    pub const fn from_minutes(minutes: u32) -> Option<Self> {
        match minutes {
            5 => Some(Self::Five),
            15 => Some(Self::Fifteen),
            30 => Some(Self::Thirty),
            _ => None,
        }
    }

    /// The number of IntervalValues in a 300 record: 1440 / IntervalLength
    /// (`MDFF-300-003`).
    pub const fn intervals_per_day(self) -> u32 {
        1440 / self.minutes()
    }

    /// The number of fields in a 300 record: indicator, IntervalDate, the
    /// values, then five more (`MDFF-300-018`).
    pub const fn fields_in_300(self) -> usize {
        self.intervals_per_day() as usize + 7
    }

    /// Minutes after 00:00 of the IntervalDate at which interval `n`
    /// (1-based) ends; the last ends at 1440, 00:00 of the next day
    /// (`MDFF-300-007`). `None` outside 1 ..= intervals per day.
    pub const fn interval_end_minute(self, n: u32) -> Option<u32> {
        if n == 0 || n > self.intervals_per_day() {
            None
        } else {
            Some(n * self.minutes())
        }
    }
}

impl std::str::FromStr for IntervalLength {
    type Err = crate::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .copied()
            .find(|v| v.as_str() == s)
            .ok_or_else(|| crate::Error::Unknown {
                what: "IntervalLength",
                text: s.to_owned(),
            })
    }
}

impl TryFrom<&str> for IntervalLength {
    type Error = crate::Error;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl std::fmt::Display for IntervalLength {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_header_and_records() {
        assert_eq!("NEM12".parse(), Ok(VersionHeader::Nem12));
        assert_eq!(VersionHeader::try_from("NEM13"), Ok(VersionHeader::Nem13));
        assert!("nem12".parse::<VersionHeader>().is_err());
        for &r in RecordIndicator::ALL {
            assert_eq!(r.as_str().parse::<RecordIndicator>(), Ok(r));
            assert_eq!(r.as_str(), r.code().to_string());
            let both = VersionHeader::Nem12.allows(r) && VersionHeader::Nem13.allows(r);
            assert_eq!(
                both,
                matches!(r, RecordIndicator::Header | RecordIndicator::End)
            );
            assert!(VersionHeader::Nem12.allows(r) || VersionHeader::Nem13.allows(r));
        }
        assert!(!VersionHeader::Nem12.allows(RecordIndicator::AccumulationMeterData));
        assert!(!VersionHeader::Nem13.allows(RecordIndicator::IntervalData));
        let counts: Vec<_> = RecordIndicator::ALL
            .iter()
            .map(|r| r.field_count())
            .collect();
        assert_eq!(
            counts,
            [
                Some(5),
                Some(10),
                None,
                Some(6),
                Some(5),
                Some(23),
                Some(5),
                Some(1)
            ]
        );
        assert!("150".parse::<RecordIndicator>().is_err());
    }

    #[test]
    fn interval_lengths() {
        let per_day: Vec<_> = IntervalLength::ALL
            .iter()
            .map(|l| l.intervals_per_day())
            .collect();
        assert_eq!(per_day, [288, 96, 48]);
        let fields: Vec<_> = IntervalLength::ALL
            .iter()
            .map(|l| l.fields_in_300())
            .collect();
        assert_eq!(fields, [295, 103, 55]);
        for &l in IntervalLength::ALL {
            assert_eq!(IntervalLength::from_minutes(l.minutes()), Some(l));
            assert_eq!(l.to_string().parse(), Ok(l));
            assert_eq!(IntervalLength::try_from(l.as_str()), Ok(l));
            assert_eq!(l.interval_end_minute(1), Some(l.minutes()));
            assert_eq!(l.interval_end_minute(l.intervals_per_day()), Some(1440));
            assert_eq!(l.interval_end_minute(0), None);
            assert_eq!(l.interval_end_minute(l.intervals_per_day() + 1), None);
        }
        assert_eq!(IntervalLength::from_minutes(10), None);
        assert!("05".parse::<IntervalLength>().is_err());
        assert!("60".parse::<IntervalLength>().is_err());
    }
}
