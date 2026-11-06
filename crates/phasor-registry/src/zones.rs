//! The zones of the registry, for `zonetrace` (feature `zones`): 750 to
//! 799. The numbers are fixed, so a report from one revision diffs against
//! the next.

use zonetrace::Zone;

/// The embedded registry decoded once.
pub static LOAD: Zone = Zone::new(750, "registry.load");
