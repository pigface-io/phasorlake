//! Validates `schemas/` and packs it into `$OUT_DIR/registry.bin`, and the
//! census's shapes into `$OUT_DIR/shapes.bin`.
//!
//! A schema file that breaks an invariant fails the build with the issue
//! list, so no binary ever ships a registry the validator rejects.

use std::path::{Path, PathBuf};

// The build script reads and validates the schema files with the crate's own
// file model, compiled here a second time. It calls only the reader, the
// validator and the encoder, so the rest of the module is unused in this
// compilation unit and nowhere else.
#[allow(dead_code, unused_imports)]
#[path = "src/file_model/mod.rs"]
mod file_model;
#[allow(dead_code, unused_imports)]
#[path = "src/shape.rs"]
mod shape;

use file_model::blob::SchemaSet;

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
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    std::fs::write(out_dir.join("registry.bin"), set.encode()).expect("write registry.bin");
    std::fs::write(out_dir.join("shapes.bin"), set.encode_shapes()).expect("write shapes.bin");
}
