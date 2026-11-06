//! The strict reading of an IIS directory listing: every folder and file,
//! or an [`Unparsed`] error naming the first line that is neither, from
//! `phasor_listing::strict`.

pub use phasor_listing::strict::{Link, LinkKind, Unparsed, href_text, parse, parse_in};
