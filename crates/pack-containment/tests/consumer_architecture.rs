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
fn executable_package_consumers_cannot_reintroduce_raw_package_interpreters() {
    const DIRECT_CONSUMERS: &[&str] = &[
        "crates/execution-kernel/src/contained_task.rs",
        "crates/execution-kernel/src/drive.rs",
        "crates/execution-kernel/src/offline.rs",
        "crates/lab/src/drive.rs",
        "crates/lab/src/readonly.rs",
        "crates/lab/src/lab_run.rs",
        "crates/lab/src/lab_run/api.rs",
        "crates/lab/src/lab_run/bundle.rs",
        "apps/actinglab/src/contained_resources.rs",
    ];
    const FORBIDDEN: &[&str] = &[
        "LoadedBundle",
        "resource_entry(",
        "loaded_bundle(",
        "into_loaded_bundle(",
        "serde_json::from_slice",
        "serde_json::from_str",
        "serde_json::from_value",
    ];

    for relative in DIRECT_CONSUMERS {
        let text = source(relative);
        for forbidden in FORBIDDEN {
            assert!(
                !text.contains(forbidden),
                "{relative} reintroduced forbidden executable-package interpreter token {forbidden:?}"
            );
        }
    }
}

#[test]
fn raw_executable_document_getters_are_not_public() {
    let containment = source("crates/pack-containment/src/lib.rs");
    for getter in [
        "pub fn control(&self)",
        "pub fn operation(&self)",
        "pub fn navigation(&self)",
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
