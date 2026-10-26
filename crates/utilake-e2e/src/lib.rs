//! Whole-stack tests, under `tests/`: every market's kit run through the
//! pipeline into a lake, and the lake read back as `docs/reading.md` says.
//! The crate holds no code; it exists so the tests may depend on every
//! crate while each crate's own tests use only what it depends on.
