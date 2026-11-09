// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #434 LAB-1 (register §1.287, "Slice LAB-1"): the slice's named tests.
//!
//! - `lab_ops_table_complete`: the operation table file, read itself, holds what
//!   `contracts/lab-two-entries.md` §9.2 requires.
//! - `lab_ops_locate_defaults`: on a fake installed root, with neither a root nor a state root
//!   given, the layer finds both (§9.3).

use crate::codes::ActingctlCode;
use crate::locate::locate_from;
use crate::packs::ROOT_CONFIG_FILE;
use crate::table::{
    Entry, OperationModule, OperationStatus, TABLE_PATH, read_operation_table, request_kind_name,
};
use actingcommand_contract::RuntimeOperationKind;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// The table file of this repository.
fn table_file() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join(TABLE_PATH)
}

// §9.2: names and MCP tool names are unique; every operation has params, output, writes,
// irreversible, runtime, entries, requests, source or new, slice and status; every kind of
// `RuntimeOperationKind::ALL` is in some operation's requests or once in `not_exposed`, and every
// kind the file names exists; `entries` follows its rule; a `reserved` row has `opens_with`.
#[test]
fn lab_ops_table_complete() {
    // Reading the file into typed rows refuses a row without one of the members §9.1 lists (or
    // with another), and a kind the file names that `RuntimeOperationKind` does not have.
    let table = read_operation_table(&table_file())
        .unwrap_or_else(|outcome| panic!("the operation table reads: {outcome}"));
    let mut violations = Vec::new();

    let mut names = BTreeSet::new();
    let mut tools = BTreeSet::new();
    for operation in &table.operations {
        if !names.insert(operation.name.as_str()) {
            violations.push(format!("the name {} is not unique", operation.name));
        }
        let tool = operation.mcp_tool();
        if !tools.insert(tool.clone()) {
            violations.push(format!(
                "the MCP tool {tool} ({}) is not unique",
                operation.name
            ));
        }
        if operation.output.trim().is_empty() {
            violations.push(format!("{} has no output", operation.name));
        }
        if operation.source.is_empty() && operation.new.is_none() {
            violations.push(format!("{} has neither a source nor new", operation.name));
        }
        if operation.slice.trim().is_empty() {
            violations.push(format!("{} has no slice", operation.name));
        }
        if operation.status == OperationStatus::Reserved && operation.opens_with.is_none() {
            violations.push(format!("{} is reserved without opens_with", operation.name));
        }
    }

    // Every kind is sent by some operation, or is once in `not_exposed`, the kinds no operation
    // sends.
    let sent: BTreeSet<RuntimeOperationKind> = table
        .operations
        .iter()
        .flat_map(|operation| operation.requests.iter().copied())
        .collect();
    let mut not_exposed = BTreeMap::<RuntimeOperationKind, usize>::new();
    for row in &table.not_exposed {
        *not_exposed.entry(row.kind).or_default() += 1;
    }
    for kind in RuntimeOperationKind::ALL {
        let listed = not_exposed.get(kind).copied().unwrap_or(0);
        let name = request_kind_name(*kind);
        match (sent.contains(kind), listed) {
            (true, 0) | (false, 1) => {}
            (false, 0) => violations.push(format!(
                "{name} is in no operation's requests and not in not_exposed"
            )),
            (true, _) => violations.push(format!(
                "{name} is in an operation's requests and in not_exposed"
            )),
            (false, _) => violations.push(format!("{name} is in not_exposed {listed} times")),
        }
    }

    // `entries`: `["lab", "ui"]` for a core operation that sends requests, each of them on the UI
    // list (the transitional kind, the row with `until`, not counted); `["lab"]` for every other.
    let ui: BTreeSet<RuntimeOperationKind> = table
        .ui_kinds
        .iter()
        .filter(|row| row.until.is_none())
        .map(|row| row.kind)
        .collect();
    for operation in &table.operations {
        let on_ui = operation.module == OperationModule::Core
            && !operation.requests.is_empty()
            && operation.requests.iter().all(|kind| ui.contains(kind));
        let expected = if on_ui {
            vec![Entry::Lab, Entry::Ui]
        } else {
            vec![Entry::Lab]
        };
        if operation.entries != expected {
            violations.push(format!(
                "{} has entries {:?}, its rule gives {expected:?}",
                operation.name, operation.entries
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "the operation table is incomplete:\n{}",
        violations.join("\n")
    );
}

// §9.3: the root from the program's own place, the state root from the selected configuration,
// with neither given: a single-location root's `runtime\` and a kept build's
// `install\builds\<commit>\runtime\` find the same root and state root; a place that is no
// installed root's is refused by name.
#[test]
fn lab_ops_locate_defaults() {
    let install = TempDir::new().expect("tempdir");
    let root = install.path();
    let state_root = root.join("state");
    fs::create_dir_all(root.join("runtime")).expect("the runtime folder");
    fs::write(root.join("runtime").join("BUILD-MANIFEST.json"), b"{}").expect("the manifest");
    fs::write(
        root.join(ROOT_CONFIG_FILE),
        json!({"state_root": &state_root, "instances": []}).to_string(),
    )
    .expect("the configuration");
    let kept = root
        .join("install")
        .join("builds")
        .join("0".repeat(40))
        .join("runtime");
    fs::create_dir_all(&kept).expect("the kept build");

    for program in [
        root.join("runtime").join("actinglab.exe"),
        kept.join("actinglab.exe"),
    ] {
        let location = locate_from(&program, None)
            .unwrap_or_else(|outcome| panic!("{} is located: {outcome}", program.display()));
        assert_eq!(location.root, root, "the root of {}", program.display());
        assert!(location.installation.is_none(), "a single-location root");
        assert_eq!(location.config, root.join(ROOT_CONFIG_FILE));
        assert_eq!(
            location.state_root,
            state_root,
            "the state root of {}",
            program.display()
        );
    }

    let elsewhere = root.join("elsewhere").join("actinglab.exe");
    let refused = locate_from(&elsewhere, None).expect_err("no installed root holds it");
    assert!(
        refused.is(ActingctlCode::InstallRootUnresolved),
        "{refused}"
    );
}
