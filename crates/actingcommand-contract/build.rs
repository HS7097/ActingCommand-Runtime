// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #420 S2: the merged outcome catalog is generated here, never committed.
//!
//! The script reads the header `contracts/outcome-codes/catalog.json` and every owner fragment
//! `contracts/outcome-codes/<owner>.json`, merges them with the outcome guard's own loader and
//! writer (`tools/outcome-guard/src/catalog.rs`, included as source, so `outcome-guard merge`
//! prints the same text), and writes `outcome-codes.json` into `OUT_DIR`, which `outcome::catalog`
//! embeds. A header or fragment that does not load or merge fails the build with every error.
//! The guard's tests check the fragments against the workspace members (G9); the build takes
//! the members the header names.

#[allow(dead_code)]
#[path = "../../tools/outcome-guard/src/catalog.rs"]
mod catalog;

use std::env;
use std::fs;
use std::path::PathBuf;

const MERGED: &str = "outcome-codes.json";

fn main() {
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let root = manifest_dir.join("../..");
    let fragments = root.join(catalog::FRAGMENT_DIR);
    println!("cargo:rerun-if-changed={}", fragments.display());

    let header_path = root.join(catalog::HEADER_FILE);
    let header_text = fs::read_to_string(&header_path)
        .unwrap_or_else(|error| panic!("read {}: {error}", header_path.display()));
    let header: serde_json::Value = serde_json::from_str(&header_text)
        .unwrap_or_else(|error| panic!("parse {}: {error}", header_path.display()));
    let members = header
        .get("fragments")
        .and_then(serde_json::Value::as_object)
        .unwrap_or_else(|| panic!("{}: fragments is not an object", catalog::HEADER_FILE))
        .values()
        .filter_map(serde_json::Value::as_str)
        .map(str::to_owned)
        .collect::<Vec<_>>();

    let loaded = catalog::load(&root, &members).unwrap_or_else(|errors| {
        panic!(
            "the outcome catalog does not load (Workflow #420 S2):\n{}",
            errors.join("\n")
        )
    });
    let text = catalog::merged_text(&loaded)
        .unwrap_or_else(|error| panic!("the outcome catalog does not merge: {error}"));
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let target = out_dir.join(MERGED);
    fs::write(&target, text).unwrap_or_else(|error| panic!("write {}: {error}", target.display()));
}
