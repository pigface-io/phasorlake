//! `utilake-census sync` over an archive built from the bootstrap months'
//! control files: the committed schemas are in step with them, and a
//! month whose control file adds a column gains an increment.

use std::path::{Path, PathBuf};
use std::process::Command;

use utilake_acquire::archive::{Archive, Sidecar, sha256_hex};
use utilake_nem::MmsdmArchive;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Archives `bytes` as the control file `name` of `month`.
fn archive_ctl(archive: &Archive, month: &str, name: &str, bytes: &[u8]) {
    let nem = MmsdmArchive::new("https://www.nemweb.com.au").unwrap();
    let url = nem
        .month_root(month.parse().unwrap())
        .unwrap()
        .join("CTL/")
        .unwrap()
        .join(&name.replace('#', "%23"))
        .unwrap();
    let path = archive.path_for(url.as_str());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, bytes).unwrap();
    Archive::write_sidecar(
        &path,
        &Sidecar {
            url: url.to_string(),
            size: bytes.len() as u64,
            sha256: sha256_hex(bytes),
            fetched_at: "2026-10-06T00:00:00Z".into(),
            etag: None,
            last_modified: None,
            last_modified_at: None,
            listed_modified: None,
            listed_modified_at: None,
            tool: "test".into(),
        },
    )
    .unwrap();
}

/// A copy of the committed schemas, and a home whose archive holds the
/// control files of both bootstrap months.
fn setup(dir: &Path) -> (PathBuf, PathBuf) {
    let schemas = dir.join("schemas");
    copy_dir(&root().join("schemas"), &schemas);
    let home = dir.join("home");
    let archive = Archive::new(home.join("archive"));
    for (month, fixture) in [("2024-01", "MMSDM_2024_01"), ("2026-08", "MMSDM_2026_08")] {
        let ctl = root().join("testdata/bootstrap").join(fixture).join("CTL");
        for e in std::fs::read_dir(ctl).unwrap() {
            let p = e.unwrap().path();
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            archive_ctl(&archive, month, &name, &std::fs::read(&p).unwrap());
        }
    }
    (schemas, home)
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let p = e.unwrap().path();
        let dst = to.join(p.file_name().unwrap());
        if p.is_dir() {
            copy_dir(&p, &dst);
        } else {
            std::fs::copy(&p, &dst).unwrap();
        }
    }
}

fn sync(schemas: &Path, home: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_utilake-census"))
        .current_dir(root())
        .arg("sync")
        .args(["--schemas", schemas.to_str().unwrap()])
        .args(["--home", home.to_str().unwrap()])
        .args(args)
        .env_remove("UTILAKE_HOME")
        .env_remove("NEMLAKE_HOME")
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn the_committed_schemas_are_in_step_with_the_bootstrap_months() {
    let dir = tempfile::tempdir().unwrap();
    let (schemas, home) = setup(dir.path());
    let (code, out, err) = sync(
        &schemas,
        &home,
        &["--check", "--from", "2024-01", "--to", "2026-08"],
    );
    assert_eq!(code, 0, "{out}{err}");
    assert!(
        err.contains("2026-08: ") && err.contains("control file(s), 0 change(s)"),
        "{err}"
    );
    assert!(
        err.contains("2024-01:") && err.contains("no table would change"),
        "{err}"
    );
    assert!(err.contains("note: 2024-02 skipped"), "{err}");
}

#[test]
fn a_month_whose_control_file_adds_a_column_gains_an_increment() {
    let dir = tempfile::tempdir().unwrap();
    let (schemas, home) = setup(dir.path());
    let name = "PUBLIC_ARCHIVE#DISPATCH_UNIT_SCADA#FILE01#202609.ctl";
    let text = std::fs::read_to_string(root().join(
        "testdata/bootstrap/MMSDM_2026_08/CTL/PUBLIC_ARCHIVE#DISPATCH_UNIT_SCADA#FILE01#202608.ctl",
    ))
    .unwrap()
    .replace(
        "LASTCHANGED DATE \"YYYY/MM/DD HH24:MI:SS\")",
        "LASTCHANGED DATE \"YYYY/MM/DD HH24:MI:SS\",\nQUALITY CHAR(10))",
    );
    archive_ctl(
        &Archive::new(home.join("archive")),
        "2026-09",
        name,
        text.as_bytes(),
    );
    let (code, out, err) = sync(&schemas, &home, &["--check", "--from", "2026-09"]);
    assert_eq!(code, 3, "{out}{err}");
    assert!(
        out.contains("2026-09: DISPATCH_UNIT_SCADA@3: new increment from 2026-09")
            && out.contains("QUALITY"),
        "{out}"
    );
    let table = schemas.join("nem/DISPATCH_UNIT_SCADA");
    let before = std::fs::read_to_string(table.join("table.toml")).unwrap();
    let (code, out, err) = sync(&schemas, &home, &["--from", "2026-09"]);
    assert_eq!(code, 0, "{out}{err}");
    let after = std::fs::read_to_string(table.join("table.toml")).unwrap();
    // What was there stays: every increment, every column.
    let increments = &before[before.find("\n[[increment]]").unwrap()..];
    assert!(after.contains(increments), "{after}");
    let start = before.find("\ncolumn = [").unwrap();
    let columns = &before[start..start + before[start..].find("\n]\n").unwrap()];
    assert!(after.contains(columns), "{after}");
    assert!(
        after.contains("{ id = 5, name = \"QUALITY\", type = \"string\", nullable = true }"),
        "{after}"
    );
    assert!(after.contains(&format!("aemo_ctl = \"{name}\"")), "{after}");
    assert_eq!(
        std::fs::read(table.join("ctl/003.ctl")).unwrap(),
        text.as_bytes()
    );
    // In step again.
    let (code, out, err) = sync(&schemas, &home, &["--check", "--from", "2026-09"]);
    assert_eq!(code, 0, "{out}{err}");
}
