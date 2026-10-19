//! `utilake-nem-census pdr-diff`: where AEMO's PDR control files (the loader
//! files of the participant data replication, kept under `reference/pdr/`)
//! and the control files of our increments disagree, table by table.
//!
//! A report only: resolution never reads a PDR control file.

use std::collections::BTreeMap;
use std::path::Path;

use utilake_core::Diagnostic;
use utilake_mms_registry::file_model::Table;
use utilake_mms_registry::file_model::ctl::{CtlFile, parse};
use utilake_mms_registry::file_model::file::CTL_DIR;

/// How a PDR control file differs from one of ours: columns only one of
/// them loads, and columns both load with other loader types.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Difference {
    /// Columns only the PDR control file loads.
    pub pdr_only: Vec<String>,
    /// Columns only ours loads.
    pub ours_only: Vec<String>,
    /// Columns both load, with the PDR type and ours.
    pub types: Vec<(String, String, String)>,
}

impl Difference {
    /// Whether the two lay the table out alike, order aside.
    pub fn is_empty(&self) -> bool {
        self.pdr_only.is_empty() && self.ours_only.is_empty() && self.types.is_empty()
    }
}

impl std::fmt::Display for Difference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts = Vec::new();
        if !self.pdr_only.is_empty() {
            parts.push(format!("PDR only: {}", self.pdr_only.join(", ")));
        }
        if !self.ours_only.is_empty() {
            parts.push(format!("ours only: {}", self.ours_only.join(", ")));
        }
        for (c, pdr, ours) in &self.types {
            parts.push(format!("{c}: PDR {pdr}, ours {ours}"));
        }
        f.write_str(&parts.join("; "))
    }
}

/// The columns and loader types of a control file, upper-case.
fn layout(c: &CtlFile) -> BTreeMap<String, String> {
    c.data_columns()
        .map(|d| {
            (
                d.name.to_ascii_uppercase(),
                d.ty.render().to_ascii_uppercase(),
            )
        })
        .collect()
}

/// How `pdr` differs from `ours`.
pub fn compare(pdr: &CtlFile, ours: &CtlFile) -> Difference {
    let (p, o) = (layout(pdr), layout(ours));
    Difference {
        pdr_only: p.keys().filter(|c| !o.contains_key(*c)).cloned().collect(),
        ours_only: o.keys().filter(|c| !p.contains_key(*c)).cloned().collect(),
        types: p
            .iter()
            .filter_map(|(c, t)| {
                o.get(c)
                    .filter(|u| *u != t)
                    .map(|u| (c.clone(), t.clone(), u.clone()))
            })
            .collect(),
    }
}

/// One line per table where a PDR control file under `pdr` differs from
/// the control file of an increment of our table, or names a table we do
/// not have. `tables` are ours, kept under `schemas/nem/`.
pub fn report(pdr: &Path, nem: &Path, tables: &[Table]) -> Result<Vec<String>, Box<Diagnostic>> {
    let read_err = |path: &Path, e: &std::io::Error| {
        Box::new(
            Diagnostic::error(
                "schema.pdr_read",
                utilake_core::Category::Schema,
                format!("The PDR control file {} could not be read.", path.display()),
            )
            .with_file(path.display().to_string())
            .caused_by(e),
        )
    };
    let mut files: Vec<_> = std::fs::read_dir(pdr)
        .map_err(|e| read_err(pdr, &e))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("ctl")))
        .collect();
    files.sort();
    let mut out = Vec::new();
    for p in files {
        let bytes = std::fs::read(&p).map_err(|e| read_err(&p, &e))?;
        let name = p.display().to_string();
        let ctl = match parse(&name, &String::from_utf8_lossy(&bytes)) {
            Ok(c) => c,
            Err(e) => {
                out.push(format!("{name}: does not parse: {e}"));
                continue;
            }
        };
        let table = ctl.table.to_ascii_uppercase();
        let Some(t) = tables.iter().find(|t| t.name == table) else {
            out.push(format!("{table}: in the PDR, not in our schemas"));
            continue;
        };
        for i in &t.increments {
            let Some(kept) = i.ctl.as_ref().and_then(|f| {
                let text = std::fs::read(nem.join(&t.name).join(CTL_DIR).join(f)).ok()?;
                parse(f, &String::from_utf8_lossy(&text)).ok()
            }) else {
                continue;
            };
            let d = compare(&ctl, &kept);
            if !d.is_empty() {
                out.push(format!("{}@{} (from {}): {d}", t.name, i.n, i.from));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use utilake_mms_registry::file_model::blob::SchemaSet;

    fn root() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    fn reports_where_the_pdr_differs() {
        let schemas = root().join("schemas");
        let tables = SchemaSet::read_dir(&schemas).unwrap().tables;
        let scada =
            std::fs::read_to_string(schemas.join("nem/DISPATCH_UNIT_SCADA/ctl/002.ctl")).unwrap();
        let dir = tempfile::tempdir().unwrap();
        // The same layout as increment 2; increment 1 lacks LASTCHANGED.
        std::fs::write(dir.path().join("a.ctl"), &scada).unwrap();
        // A table we do not have, and one that does not parse.
        std::fs::write(
            dir.path().join("b.ctl"),
            scada.replace("INTO TABLE DISPATCH_UNIT_SCADA", "INTO TABLE NOBODY"),
        )
        .unwrap();
        std::fs::write(dir.path().join("c.ctl"), "nope").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "not a control file").unwrap();
        let lines = report(dir.path(), &schemas.join("nem"), &tables).unwrap();
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert_eq!(
            lines[0],
            "DISPATCH_UNIT_SCADA@1 (from 2015-01): PDR only: LASTCHANGED"
        );
        assert_eq!(lines[1], "NOBODY: in the PDR, not in our schemas");
        assert!(lines[2].contains("does not parse"), "{lines:?}");
        // Types that differ, and columns only ours has.
        let pdr = parse(
            "p",
            &scada
                .replace("DUID CHAR(20)", "DUID CHAR(30)")
                .replace("SCADAVALUE FLOAT EXTERNAL,", ""),
        )
        .unwrap();
        let d = compare(&pdr, &parse("o", &scada).unwrap());
        assert_eq!(d.ours_only, ["SCADAVALUE"]);
        assert_eq!(
            d.to_string(),
            "ours only: SCADAVALUE; DUID: PDR CHAR(30), ours CHAR(20)"
        );
        assert!(report(&dir.path().join("none"), &schemas, &tables).is_err());
    }
}
