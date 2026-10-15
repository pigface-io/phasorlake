//! The mirror without a network: reading a big directory listing strictly,
//! and mapping a zip of zips as the daily report archives are.

use std::fmt::Write as _;
use std::hint::black_box;
use std::io::{Cursor, Write as _};

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use nemweb_mirror::net::Archive;
use nemweb_mirror::{Mirror, Remote, RemoteKind};

const LISTING_ROWS: usize = 10_000;
/// A day of five-minute files: one zip holding 288 zips of one CSV each.
const INNER_ZIPS: usize = 288;
const CSV_ROWS: usize = 400;

/// An IIS directory listing of `LISTING_ROWS` zips, as nemweb serves one.
fn listing() -> Vec<u8> {
    let mut out = String::from(
        "<html><body><pre><A HREF=\"/Data_Archive/\">[To Parent Directory]</A><br><br>",
    );
    for i in 0..LISTING_ROWS {
        let _ = write!(
            out,
            "    Tuesday, December 9, 2025 09:05 AM   {size} <A HREF=\"/Data_Archive/Wholesale_Electricity/MMSDM/2024/MMSDM_2024_01/MMSDM_Historical_Data_SQLLoader/DATA/PUBLIC_DVD_TABLE{i}_202401010000.zip\">PUBLIC_DVD_TABLE{i}_202401010000.zip</A><br>",
            size = 1000 + i
        );
    }
    out.push_str("</pre><hr></body></html>");
    out.into_bytes()
}

fn zip_of(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        w.start_file(name.as_str(), zip::write::SimpleFileOptions::default())
            .unwrap();
        w.write_all(bytes).unwrap();
    }
    w.finish().unwrap().into_inner()
}

/// A finished crawl of one day zip, archived.
fn day(home: &std::path::Path) -> (Mirror, Archive, u64) {
    let mut csv = String::new();
    for r in 0..CSV_ROWS {
        let _ = writeln!(
            csv,
            "D,DISPATCH,UNIT_SCADA,1,\"2026/10/01 00:05:00\",DUID{r},{r}.5"
        );
    }
    let inner: Vec<(String, Vec<u8>)> = (0..INNER_ZIPS)
        .map(|i| {
            let name = format!("PUBLIC_DISPATCHSCADA_20261001{i:04}.CSV");
            let z = zip_of(&[(name, csv.clone().into_bytes())]);
            (format!("PUBLIC_DISPATCHSCADA_20261001{i:04}.zip"), z)
        })
        .collect();
    let day = zip_of(&inner);
    let url = "https://h/A/PUBLIC_DISPATCHSCADA_20261001.zip";
    let archive = Archive::new(home.join("archive"));
    let path = archive.path_for(url);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, &day).unwrap();
    let mirror = Mirror::new(home.join("mirror"));
    std::fs::create_dir_all(mirror.crawl_dir()).unwrap();
    let remote = Remote {
        url: url.into(),
        kind: RemoteKind::File,
        size: Some(day.len() as u64),
        modified: "m".into(),
        listed_on: "https://h/A/".into(),
        crawled_at: None,
    };
    std::fs::write(
        mirror.record_path(),
        serde_json::to_string(&remote).unwrap(),
    )
    .unwrap();
    std::fs::write(mirror.done_path(), "").unwrap();
    (mirror, archive, day.len() as u64)
}

/// A zip's bytes in memory, read by span.
struct Mem(Vec<u8>);

impl nemweb_mirror::zipdir::Spans for Mem {
    type Error = nemweb_mirror::zipdir::DirError;
    async fn read(&mut self, start: u64, end: u64) -> Result<Vec<u8>, Self::Error> {
        Ok(self.0[start as usize..end as usize].to_vec())
    }
}

/// A month folder's worth of entries: 300 table zips in one zip.
fn month_zip() -> Vec<u8> {
    let entries: Vec<(String, Vec<u8>)> = (0..300)
        .map(|i| {
            (
                format!("MMSDM_2026_09/MMSDM_Historical_Data_SQLLoader/DATA/PUBLIC_ARCHIVE#TABLE{i}#FILE01#202609010000.zip"),
                vec![b'z'; 64],
            )
        })
        .collect();
    zip_of(&entries)
}

fn bench(c: &mut Criterion) {
    let html = listing();
    let mut g = c.benchmark_group("mirror");
    g.sample_size(10);
    g.throughput(Throughput::Bytes(html.len() as u64));
    g.bench_function("listing_parse", |b| {
        b.iter(|| {
            black_box(
                nemweb_mirror::listing::parse(black_box(&html))
                    .unwrap()
                    .len(),
            )
        });
    });

    let home = tempfile::tempdir().unwrap();
    let (mirror, archive, bytes) = day(home.path());
    let roots = [nemweb_mirror::root("https://h/A/").unwrap()];
    g.throughput(Throughput::Bytes(bytes));
    g.bench_function("map_zip_of_zips", |b| {
        b.iter(|| {
            black_box(
                nemweb_mirror::map(&mirror, &archive, &roots, 1)
                    .unwrap()
                    .zips,
            )
        });
    });

    let zip = Mem(month_zip());
    let len = zip.0.len() as u64;
    let mut zip = zip;
    g.throughput(Throughput::Bytes(
        (zip.0.len() as u64).min(nemweb_mirror::zipdir::TAIL),
    ));
    g.bench_function("zipdir_parse", |b| {
        b.iter(|| {
            let entries =
                futures::executor::block_on(nemweb_mirror::zipdir::read_directory(&mut zip, len))
                    .unwrap();
            black_box(entries.len())
        });
    });
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
