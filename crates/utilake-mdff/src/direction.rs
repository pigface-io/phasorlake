//! The DirectionIndicator of the NEM13 250 record (§5.3, p.17).

code_enum! {
    /// Whether a register records import or export, **said from the grid's
    /// side** (`MDFF-250-008`). It is the reverse of the customer's view:
    /// an ordinary consumption register is `E`, energy exported *from* the
    /// grid to the connection point. Exact case.
    pub enum DirectionIndicator ("DirectionIndicator", case_insensitive = false) {
        /// `I`, "Import to grid": energy normally flows from the
        /// connection point to the grid (e.g. a generation register).
        Import => "I",
        /// `E`, "Export from grid": energy normally flows from the grid to
        /// the connection point (e.g. a consumption register).
        Export => "E",
    }
}

impl DirectionIndicator {
    /// The spec's wording of the value.
    pub const fn meaning(self) -> &'static str {
        match self {
            Self::Import => "Import to grid",
            Self::Export => "Export from grid",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values() {
        assert_eq!("I".parse(), Ok(DirectionIndicator::Import));
        assert_eq!("E".parse(), Ok(DirectionIndicator::Export));
        assert!("e".parse::<DirectionIndicator>().is_err());
        assert!("X".parse::<DirectionIndicator>().is_err());
        assert_eq!(DirectionIndicator::Import.meaning(), "Import to grid");
        assert_eq!(DirectionIndicator::Export.meaning(), "Export from grid");
        assert_eq!(DirectionIndicator::ALL.len(), 2);
    }
}
