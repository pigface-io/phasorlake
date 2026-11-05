//! New Zealand's wholesale electricity market kit: the Electricity
//! Authority's EMI datasets, plain CSV files on a blob store, one a day or
//! one a month, as two sources over one feed ([`kit`]), set by
//! [`NzConfig`]. A day's file, its interim and the month's file of a dataset
//! are one table; a file written again is fetched again and kept beside the
//! earlier one.

pub mod config;
pub mod kit;

pub use config::NzConfig;
pub use kit::{DAILY_SOURCE_ID, KIT_ID, MONTHLY_SOURCE_ID, kit};
