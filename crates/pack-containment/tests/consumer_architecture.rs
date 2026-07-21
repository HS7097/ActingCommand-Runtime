// SPDX-License-Identifier: AGPL-3.0-only

use std::fs;
use std::path::{Path, PathBuf};

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("pack-containment must be two directories below the repository root")
        .to_path_buf()
}

fn source(relative: &str) -> String {
    let path = repository_root().join(relative);
    fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "failed to read architecture-guard source {}: {error}",
            path.display()
        )
    })
}

#[test]
fn downstream_effecting_crates_receive_only_the_kernel_capability() {
    for manifest in ["crates/lab/Cargo.toml", "apps/actinglab/Cargo.toml"] {
        let manifest_source = source(manifest);
        let dependencies = manifest_section(&manifest_source, "dependencies");
        assert!(
            !dependencies.contains("actingcommand-pack-containment"),
            "{manifest} must obtain admitted package types only through execution-kernel/Lab"
        );
    }

    for root in ["crates/lab/src", "apps/actinglab/src"] {
        let files = rust_sources(&repository_root().join(root));
        assert!(
            !files.is_empty(),
            "architecture root {root} contained no Rust files"
        );
        for path in files {
            let text = fs::read_to_string(&path).expect("read production Rust source");
            assert!(
                !text.contains("actingcommand_pack_containment"),
                "{} bypasses the execution-kernel admitted capability dependency",
                path.display()
            );
        }
    }
}

#[test]
fn execution_kernel_has_one_raw_package_ingress_owner() {
    let root = repository_root().join("crates/execution-kernel/src");
    let mut ingress_files = Vec::new();
    for path in rust_sources(&root) {
        let text = fs::read_to_string(&path).expect("read kernel Rust source");
        if text.contains("Containment::new") || text.contains("take_loaded(") {
            ingress_files.push(path.strip_prefix(&root).unwrap().to_path_buf());
        }
    }
    assert_eq!(ingress_files, [PathBuf::from("bundle.rs")]);
}

#[test]
fn raw_executable_document_getters_are_not_public() {
    let containment = source("crates/pack-containment/src/lib.rs");
    for getter in [
        "pub fn control(&self)",
        "pub fn operation(&self)",
        "pub fn navigation(&self)",
        "pub fn entry(&self)",
        "pub fn resource_entry(&self)",
    ] {
        assert!(
            !containment.contains(getter),
            "pack-containment reintroduced raw executable getter {getter:?}"
        );
    }

    let admission = source("crates/pack-containment/src/admission.rs");
    assert!(
        !admission.contains("pub fn as_value(&self)"),
        "canonical admission must not expose an underlying serde_json::Value"
    );
}

fn manifest_section<'a>(manifest: &'a str, section: &str) -> &'a str {
    let heading = format!("[{section}]");
    let start = manifest.find(&heading).expect("manifest section") + heading.len();
    let rest = &manifest[start..];
    let end = rest.find("\n[").unwrap_or(rest.len());
    &rest[..end]
}

fn rust_sources(root: &Path) -> Vec<PathBuf> {
    fn visit(path: &Path, files: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(path).expect("read architecture directory") {
            let path = entry.expect("architecture directory entry").path();
            if path.is_dir() {
                visit(&path, files);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
    }

    let mut files = Vec::new();
    visit(root, &mut files);
    files.sort();
    files
}

#[test]
fn actinglab_navigation_entry_uses_the_admitted_capability() {
    let main = source("apps/actinglab/src/main.rs");
    let start = main
        .find("fn load_navigation_graph(")
        .expect("ActingLab navigation entry");
    let end = main[start..]
        .find("fn navigation_graph_from_admitted(")
        .map(|offset| start + offset)
        .expect("admitted navigation adapter");
    let entry = &main[start..end];
    assert!(entry.contains("contained_resources::load"));
    assert!(entry.contains("admitted_package()"));
    for forbidden in ["fs::read", "serde_json::from_", "navigation_path"] {
        assert!(
            !entry.contains(forbidden),
            "ActingLab navigation entry reintroduced raw parsing token {forbidden:?}"
        );
    }

    let lab2 = source("apps/actinglab/src/lab2_cli.rs");
    assert!(lab2.contains("contained_resources::load"));
    assert!(!lab2.contains("LoadedBundle"));
}
