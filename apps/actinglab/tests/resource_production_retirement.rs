// SPDX-License-Identifier: AGPL-3.0-only

use serde_json::Value;
use std::{fs, process::Command};
use tempfile::TempDir;

#[test]
fn resource_production_refuses_before_installation_config_and_recording_effects() {
    let root = TempDir::new().expect("tempdir");
    let config = root.path().join("actinglab.json");
    let state_dir = root.path().join("record");
    let repo = root.path().join("resource-repo");
    let output = root.path().join("output.zip");
    // An installation reader or config loader would fail on these inputs before dispatch.
    fs::write(&config, b"{invalid-config").expect("config");
    let routes: &[&[&str]] = &[
        &["package", "build-task"],
        &["package", "build-pack"],
        &["resource", "convert"],
        &["record", "build-task"],
        &["record", "promote"],
        &["record", "publish"],
        &["session", "record", "build-task"],
        &["session", "record", "promote"],
        &["session", "record", "publish"],
    ];
    for route in routes {
        for global in [None, Some("--dry-run"), Some("--version")] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_actinglab"));
            command
                .env("ACTINGLAB_CONFIG", &config)
                .env("ACTINGCOMMAND_INSTALL_ROOT", root.path())
                .env("ACTINGCOMMAND_INSTALL_SELECTION", "{invalid-selection")
                .env("ACTINGCOMMAND_STATE_ROOT", root.path().join("runtime"))
                .env("LOCALAPPDATA", root.path().join("local"))
                .arg("--json")
                .args(*route)
                .arg("--state-dir")
                .arg(&state_dir)
                .arg("--repo")
                .arg(&repo)
                .arg("--out")
                .arg(&output);
            if let Some(global) = global {
                command.arg(global);
            }
            let result = command.output().expect("actinglab process");
            assert_eq!(
                result.status.code(),
                Some(2),
                "{route:?}: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            let envelope: Value = serde_json::from_slice(&result.stdout).expect("error envelope");
            assert_eq!(envelope["ok"], false);
            assert_eq!(envelope["command"], route.join(" "));
            assert_eq!(envelope["error"]["code"], "resource_production_retired");
            assert!(!state_dir.exists() && !repo.exists() && !output.exists());
            assert_eq!(fs::read(&config).unwrap(), b"{invalid-config");
            assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
        }
    }
}
