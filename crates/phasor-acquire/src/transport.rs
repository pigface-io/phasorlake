//! Moving bytes over HTTP, behind a trait so tests and `--offline` need no
//! network: the family's transports ([`phasor_net::transport`]).

pub use phasor_net::transport::*;
