//! The Bureau's real FTP server, opt-in: `PHASOR_NET_TESTS=1 cargo test -p phasor-ftp --test net`.
//! One small product header is listed, asked for and fetched, so the live
//! session's connection, listing, time, size and retrieval are seen to work;
//! each step's time is printed. The Bureau serves no `MLSD`, so the folder
//! is listed by name with only the kept files asked their time and size.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use futures::StreamExt as _;
use phasor_ftp::FtpTransport;
use phasor_ftp::listing::Entry;
use phasor_net::Transport;

const FOLDER: &str = "ftp://ftp.bom.gov.au/anon/gen/fwo/";
const FILE: &str = "ftp://ftp.bom.gov.au/anon/gen/fwo/IDN11060.amoc.xml";

#[tokio::test]
async fn the_bureaus_server_lists_a_folder_and_serves_a_header_file() {
    if std::env::var("PHASOR_NET_TESTS").is_err() {
        eprintln!("skipped: set PHASOR_NET_TESTS=1 to reach ftp.bom.gov.au");
        return;
    }
    let t = FtpTransport::new(Duration::from_secs(60)).with_names_kept(HashMap::from([(
        "ftp.bom.gov.au".to_owned(),
        vec!["IDN11060*".to_owned()],
    )]));
    let at = Instant::now();
    let r = t.head(FILE).await.unwrap();
    eprintln!("head: {:?}", at.elapsed());
    assert_eq!(r.status, 200);
    assert!(
        r.content_length.is_some_and(|n| n > 0),
        "{:?}",
        r.content_length
    );
    assert!(r.last_modified.is_some(), "MDTM gave no time");
    let at = Instant::now();
    let r = t.get(FILE).await.unwrap();
    let body: Vec<u8> = r.body.map(|c| c.unwrap().to_vec()).concat().await;
    eprintln!("get file: {:?}", at.elapsed());
    assert!(
        body.starts_with(b"<?xml") || body.starts_with(b"<amoc"),
        "{}",
        String::from_utf8_lossy(&body[..40.min(body.len())])
    );
    let at = Instant::now();
    let r = t.get(FOLDER).await.unwrap();
    let page: Vec<u8> = r.body.map(|c| c.unwrap().to_vec()).concat().await;
    let listed = at.elapsed();
    eprintln!("list folder: {listed:?}");
    let entries = Entry::read(&page).expect("entries as lines");
    assert!(entries.len() > 100, "{} entries", entries.len());
    let header = entries
        .iter()
        .find(|e| e.name == "IDN11060.amoc.xml")
        .expect("the header file is listed");
    assert!(
        header.size.is_some() && header.modified.is_some(),
        "a kept file is asked"
    );
    assert!(
        entries.iter().filter(|e| e.size.is_some()).count() < 20,
        "only the kept names are asked"
    );
    assert!(
        listed < Duration::from_secs(60),
        "the listing took {listed:?}"
    );
    eprintln!(
        "{} entries in fwo/, {} bytes in the header file",
        entries.len(),
        body.len()
    );
}
