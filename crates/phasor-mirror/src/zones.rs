//! The zones of the mirror, for `zonetrace` (feature `zones`): 600 to
//! 699. The numbers are fixed, so a report from one revision diffs against
//! the next. A command's root and its network stages are stretches (they
//! cross awaits and threads), roots of their own; the page parses, the
//! writes and the map's workers are zones on their threads.

use zonetrace::Zone;

/// `crawl`.
pub static CRAWL: Zone = Zone::new(600, "cmd.crawl");
/// `download`.
pub static DOWNLOAD: Zone = Zone::new(601, "cmd.download");
/// `peek`.
pub static PEEK: Zone = Zone::new(602, "cmd.peek");
/// `run`.
pub static RUN: Zone = Zone::new(603, "cmd.run");
/// `watch`.
pub static WATCH: Zone = Zone::new(604, "cmd.watch");
/// `map`.
pub static MAP: Zone = Zone::new(605, "cmd.map");
/// One folder's listing page fetched.
pub static CRAWL_PAGE: Zone = Zone::new(610, "crawl.page");
/// One listing page read into links.
pub static CRAWL_READ_PAGE: Zone = Zone::new(611, "crawl.read_page");
/// One listing page written to the state.
pub static CRAWL_WRITE_PAGE: Zone = Zone::new(612, "crawl.write_page");
/// One file dealt with by a download pass.
pub static DOWNLOAD_FILE: Zone = Zone::new(620, "download.file");
/// The wait for a host's download permit.
pub static DOWNLOAD_PERMIT: Zone = Zone::new(621, "download.permit");
/// One zip's directory peeked by ranged requests.
pub static PEEK_ZIP: Zone = Zone::new(630, "peek.zip");
/// One round of a watch: the due folders read and their new files downloaded.
pub static WATCH_ROUND: Zone = Zone::new(640, "watch.round");
/// The due folders read again.
pub static WATCH_REFRESH: Zone = Zone::new(641, "watch.refresh");
/// The round's new files downloaded.
pub static WATCH_DOWNLOAD: Zone = Zone::new(642, "watch.download");
/// The sleep until the next folder is due.
pub static WATCH_IDLE: Zone = Zone::new(643, "watch.idle");
/// One top folder of the catalog listed by a map worker.
pub static MAP_TOP: Zone = Zone::new(650, "map.top");
/// The path of a [`CRAWL`] stretch.
pub static CRAWL_PATH: [&Zone; 1] = [&CRAWL];
/// The path of a [`DOWNLOAD`] stretch.
pub static DOWNLOAD_PATH: [&Zone; 1] = [&DOWNLOAD];
/// The path of a [`PEEK`] stretch.
pub static PEEK_PATH: [&Zone; 1] = [&PEEK];
/// The path of a [`RUN`] stretch.
pub static RUN_PATH: [&Zone; 1] = [&RUN];
/// The path of a [`WATCH`] stretch.
pub static WATCH_PATH: [&Zone; 1] = [&WATCH];
/// The path of a [`CRAWL_PAGE`] stretch, under its crawl.
pub static CRAWL_PAGE_PATH: [&Zone; 2] = [&CRAWL, &CRAWL_PAGE];
/// The path of a [`DOWNLOAD_FILE`] stretch.
pub static DOWNLOAD_FILE_PATH: [&Zone; 1] = [&DOWNLOAD_FILE];
/// The path of a [`DOWNLOAD_PERMIT`] stretch, under its file.
pub static DOWNLOAD_PERMIT_PATH: [&Zone; 2] = [&DOWNLOAD_FILE, &DOWNLOAD_PERMIT];
/// The path of a [`PEEK_ZIP`] stretch, under its peek.
pub static PEEK_ZIP_PATH: [&Zone; 2] = [&PEEK, &PEEK_ZIP];
/// The path of a [`WATCH_ROUND`] stretch, under its watch.
pub static WATCH_ROUND_PATH: [&Zone; 2] = [&WATCH, &WATCH_ROUND];
/// The path of a [`WATCH_REFRESH`] stretch, under its round.
pub static WATCH_REFRESH_PATH: [&Zone; 3] = [&WATCH, &WATCH_ROUND, &WATCH_REFRESH];
/// The path of a [`WATCH_DOWNLOAD`] stretch, under its round.
pub static WATCH_DOWNLOAD_PATH: [&Zone; 3] = [&WATCH, &WATCH_ROUND, &WATCH_DOWNLOAD];
/// The path of a [`WATCH_IDLE`] stretch, under its watch.
pub static WATCH_IDLE_PATH: [&Zone; 2] = [&WATCH, &WATCH_IDLE];
