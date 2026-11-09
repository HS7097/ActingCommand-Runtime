// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #390 WD-1 (deploy plan Q1 option (b); Alice 2026-10-16; register §1.96; apps gate 70),
// req. 6: `actingctl start` / `restart` without `--config` still need an A/B selection. Workflow
// #434 LAB-4 moved the watchdog's gates to `crates/watchdog` (`gate_wd1`); this one tests
// actingctl's `start`, and it moved with it to `crates/lab-rescue` (LAB-3), on the same
// single-location root:
// `runtime\BUILD-MANIFEST.json`, a stand-in fixed entry and `actingd.config.json` naming an
// absolute state root outside the root; no `install\`.

use crate::watchdog::observe;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

// `actingctl start`'s developer hint for a root without a selection.
const NO_SELECTION_HINT: &str = "pass --config <file> to start a root without one";

// Req. 6: `actingctl start` (and `restart`) without `--config` on a single-location root keep
// today's refusal: `watchdog_layout_unknown` with the root as `path` and the hint to pass
// `--config`.
#[test]
fn wd1_start_without_config_still_needs_an_ab_selection() {
    let temp = TempDir::new().expect("tempdir");
    let root = temp.path().join("single");
    let state = temp.path().join("single-state");
    for directory in [root.join("runtime"), state.clone()] {
        fs::create_dir_all(directory).expect("create a fixture directory");
    }
    fs::write(
        root.join("runtime").join("BUILD-MANIFEST.json"),
        br#"{"fixture":"watchdog-single"}"#,
    )
    .expect("write the build manifest");
    fs::write(root.join("runtime").join("actingcommand-actingd.exe"), b"")
        .expect("write the fixed entry");
    let config = serde_json::to_vec(&json!({
        "schema_version": "actingcommand.actingd.config.v1",
        "state_root": state,
        "instances": []
    }))
    .expect("the config JSON");
    fs::write(root.join("actingd.config.json"), config).expect("write the config");

    let failed = crate::runtime_start::ab_installation(&root)
        .err()
        .expect("start without --config refuses a single-location root");

    assert_eq!(failed.outcome.code().as_str(), "watchdog_layout_unknown");
    assert_eq!(
        failed.outcome.value("path"),
        Some(&json!(text(&plain(&root))))
    );
    assert_eq!(failed.text.as_deref(), Some(NO_SELECTION_HINT));
}

// The plain spelling of a fixture path's canonical form.
fn plain(path: &Path) -> PathBuf {
    observe::plain_path(&fs::canonicalize(path).expect("canonicalize a fixture path"))
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
