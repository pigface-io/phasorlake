//! The zones of getting files, for `zonetrace` (feature `zones`): 550 to
//! 599. The numbers are fixed, so a report from one revision diffs against
//! the next. The fetches are asynchronous stretches, roots of their own;
//! the listing parse and the archive check are zones on the thread.

use zonetrace::Zone;

/// One file's picked entries fetched by ranged requests.
pub static FETCH_PICKS: Zone = Zone::new(550, "acquire.fetch_picks");
/// One whole file fetched into the archive, or found there.
pub static FETCH: Zone = Zone::new(551, "acquire.fetch");
/// One download streamed and kept with its record.
pub static DOWNLOAD: Zone = Zone::new(552, "acquire.download");
/// One folder listed from the host.
pub static LISTING: Zone = Zone::new(553, "acquire.listing");
/// The path of a [`FETCH_PICKS`] stretch.
pub static FETCH_PICKS_PATH: [&Zone; 1] = [&FETCH_PICKS];
/// The path of a [`FETCH`] stretch.
pub static FETCH_PATH: [&Zone; 1] = [&FETCH];
/// The path of a [`DOWNLOAD`] stretch, under its fetch.
pub static DOWNLOAD_PATH: [&Zone; 2] = [&FETCH, &DOWNLOAD];
/// The path of a [`LISTING`] stretch.
pub static LISTING_PATH: [&Zone; 1] = [&LISTING];
