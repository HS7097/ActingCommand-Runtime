// SPDX-License-Identifier: AGPL-3.0-only

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use actingcommand_actinglab_architecture::{
    contract_dependency_violations, extract_command_inventory, inspect_lab_source,
    inspect_public_api, validate_line_ratchet, workspace_dependency_violations,
};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("architecture tool must live at tools/<name>")
        .to_path_buf()
}

#[test]
fn lab_source_obeys_dependency_law_or_placeholder_is_consistent() {
    let root = workspace_root();
    let lab_root = root.join("crates/lab");
    if !lab_root.exists() {
        let workspace_manifest =
            fs::read_to_string(root.join("Cargo.toml")).expect("read workspace Cargo.toml");
        assert!(
            !workspace_manifest.contains("\"crates/lab\""),
            "workspace registers crates/lab before the crate exists"
        );
        return;
    }

    let mut files = Vec::new();
    collect_rust_files(&lab_root, &mut files);
    assert!(
        !files.is_empty(),
        "crates/lab contains no Rust source files"
    );
    let mut violations = Vec::new();
    for path in files {
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
        let display = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .display()
            .to_string();
        violations.extend(inspect_lab_source(&display, &source).unwrap());
        violations.extend(inspect_public_api(&display, &source).unwrap());
    }
    assert!(
        violations.is_empty(),
        "crates/lab dependency-law violations:\n{}",
        violations.join("\n")
    );
}

fn collect_rust_files(root: &Path, files: &mut Vec<PathBuf>) {
    let entries =
        fs::read_dir(root).unwrap_or_else(|err| panic!("read directory {}: {err}", root.display()));
    for entry in entries {
        let entry = entry.unwrap_or_else(|err| panic!("read {} entry: {err}", root.display()));
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

#[test]
fn command_inventory_matches_checked_in_snapshot() {
    let root = workspace_root();
    let mut paths = Vec::new();
    collect_rust_files(&root.join("apps/actinglab/src"), &mut paths);
    paths.sort();
    let owned_sources = paths
        .iter()
        .map(|path| {
            let relative = path
                .strip_prefix(&root)
                .unwrap_or(path)
                .display()
                .to_string();
            let source = fs::read_to_string(path)
                .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
            (relative, source)
        })
        .collect::<Vec<_>>();
    let sources = owned_sources
        .iter()
        .map(|(path, source)| (path.as_str(), source.as_str()))
        .collect::<Vec<_>>();
    let actual = extract_command_inventory(&sources).unwrap();

    let snapshot_text = fs::read_to_string(root.join("ratchet/actinglab_commands.json"))
        .expect("read ratchet/actinglab_commands.json");
    let snapshot: serde_json::Value =
        serde_json::from_str(&snapshot_text).expect("parse actinglab command snapshot");
    assert_eq!(
        snapshot["schema_version"],
        "actingcommand.command-inventory.v1"
    );
    assert_eq!(snapshot["source"], "apps/actinglab/src/**/*.rs");
    assert_eq!(snapshot["dispatch_function"], "execute");
    assert_eq!(snapshot["denominator_kind"], "top_level_dispatch_arm");
    assert_eq!(
        snapshot["dispatch_arm_count"].as_u64(),
        Some(actual.dispatch_arm_count as u64)
    );
    let expected_dispatch_arms = snapshot["dispatch_arms"]
        .as_array()
        .expect("snapshot dispatch_arms must be an array")
        .iter()
        .map(|value| {
            value
                .as_str()
                .expect("snapshot dispatch arm must be a string")
                .to_string()
        })
        .collect::<Vec<_>>();
    assert_eq!(expected_dispatch_arms, actual.dispatch_arms);
    assert_eq!(
        snapshot["command_count"].as_u64(),
        Some(actual.commands.len() as u64)
    );
    let expected_commands = snapshot["commands"]
        .as_array()
        .expect("snapshot commands must be an array")
        .iter()
        .map(|value| {
            value
                .as_str()
                .expect("snapshot command must be a string")
                .to_string()
        })
        .collect::<Vec<_>>();
    assert_eq!(expected_commands, actual.commands);
    for exemption in snapshot["pipeline_exemptions"]
        .as_array()
        .expect("snapshot pipeline_exemptions must be an array")
    {
        let command = exemption["command"]
            .as_str()
            .expect("pipeline exemption command must be a string");
        assert!(
            actual.commands.iter().any(|candidate| candidate == command),
            "pipeline exemption references unknown command {command}"
        );
        assert!(
            exemption["reason"]
                .as_str()
                .is_some_and(|reason| !reason.trim().is_empty()),
            "pipeline exemption {command} must explain its reason"
        );
    }
}

#[test]
fn contract_dependencies_stay_within_budget() {
    let root = workspace_root();
    let manifest = fs::read_to_string(root.join("crates/actingcommand-contract/Cargo.toml"))
        .expect("read contract Cargo.toml");
    let violations = contract_dependency_violations(&manifest).unwrap();

    assert!(
        violations.is_empty(),
        "contract dependency budget violations:\n{}",
        violations.join("\n")
    );
}

#[test]
fn workspace_packages_do_not_depend_on_apps() {
    let root = workspace_root();
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1"])
        .current_dir(&root)
        .output()
        .expect("run cargo metadata");
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata = String::from_utf8(output.stdout).expect("cargo metadata must emit UTF-8 JSON");
    let violations = workspace_dependency_violations(&metadata).unwrap();

    assert!(
        violations.is_empty(),
        "workspace dependency-law violations:\n{}",
        violations.join("\n")
    );
}

#[test]
fn main_rs_line_ratchet_matches_checked_in_baseline() {
    let root = workspace_root();
    let source = fs::read_to_string(root.join("apps/actinglab/src/main.rs"))
        .expect("read apps/actinglab/src/main.rs");
    let baseline = fs::read_to_string(root.join("ratchet/main_rs_lines.txt"))
        .expect("read ratchet/main_rs_lines.txt")
        .trim()
        .parse::<usize>()
        .expect("ratchet/main_rs_lines.txt must contain one integer");

    validate_line_ratchet(baseline, source.lines().count()).unwrap();
}
