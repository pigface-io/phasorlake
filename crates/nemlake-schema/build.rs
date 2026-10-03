//! Validates `schemas/` and packs it into `$OUT_DIR/registry.bin`.
//!
//! A schema file that breaks an invariant fails the build with the issue
//! list, so no binary ever ships a registry the validator rejects.

use std::path::{Path, PathBuf};

use nemlake_schema_model::blob::SchemaSet;

fn watch(dir: &Path) {
    println!("cargo:rerun-if-changed={}", dir.display());
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.filter_map(Result::ok) {
            let p = e.path();
            if p.is_dir() {
                watch(&p);
            } else {
                println!("cargo:rerun-if-changed={}", p.display());
            }
        }
    }
}

fn main() {
    let manifest =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    let schemas = manifest.join("../../schemas");
    watch(&schemas);
    let set = match SchemaSet::read_dir(&schemas) {
        Ok(s) => s,
        Err(e) => panic!("schema registry: {e}"),
    };
    let issues = set.validate();
    if !issues.is_empty() {
        let list: Vec<String> = issues.iter().map(|i| format!("  - {i}")).collect();
        panic!(
            "schema registry: {} invalid schema file issue(s):\n{}",
            issues.len(),
            list.join("\n")
        );
    }
    let out =
        PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("registry.bin");
    std::fs::write(&out, set.encode()).expect("write registry.bin");
}
