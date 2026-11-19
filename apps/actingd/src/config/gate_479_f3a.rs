// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #479 F-3a fix round 1 (register §1.486; contracts/device-binding.md, "An iOS handset",
// "Where `acib` is"; work order rd-1119/brief-479-f3a-fix1.md, item 3): check-config of a
// configuration that names a bound iOS handset, judged from no install root (so no `acib` is where
// that handset's captures run it from), passes: its outcome's exit status is 0, and it notes
// `device_acib_missing`, a warning, with the handset's alias, never a refusal. A configuration
// without an iOS handset notes nothing of it. The ids, aliases, paths and keys are made up.

use super::device_binding::write_private_file_for_test;
use super::*;
use actingcommand_contract::IdentifierIssuer;
use actingcommand_contract::outcome::Category;
use serde_json::{Value, json};
use std::ffi::OsString;
use std::fs;
use tempfile::TempDir;

const IPHONE: &str = "neutral.iphone";
const FIXTURE: &str = "neutral.fixture";
/// The phone's key the private file holds, made under the installation's own secret.
const KEY: &str = "sha256:4790f2000000000000000000000000000000000000000000000000000000f2a1";
const ACIB_MISSING: &str = "device_acib_missing";

fn instance_id() -> InstanceId {
    *IdentifierIssuer::new()
        .expect("issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport()
}

/// check-config of the configuration of `instances` written into `root`, beside the private
/// binding file of `rows` when there are any; it must pass: its outcome and its notes, in order.
fn check(root: &TempDir, instances: Vec<Value>, rows: Vec<Value>) -> (Outcome, Vec<Outcome>) {
    if !rows.is_empty() {
        write_private_file_for_test(
            root.path(),
            &serde_json::to_vec(
                &json!({"schema": "actingcommand.device-bindings.v2", "bindings": rows}),
            )
            .expect("the private file"),
        );
    }
    let path = root.path().join("actingd.json");
    fs::write(
        &path,
        serde_json::to_vec(&json!({
            "schema_version": CONFIG_SCHEMA_VERSION,
            "state_root": root.path(),
            "bind_host": "127.0.0.1",
            "bind_port": 0,
            "secret_fingerprint_salt": "0123456789abcdef",
            "instances": instances
        }))
        .expect("the configuration JSON"),
    )
    .expect("write the configuration");
    let reported = crate::check_config::run(vec![
        OsString::from("check-config"),
        OsString::from("--config"),
        path.into_os_string(),
    ])
    .unwrap_or_else(|error| panic!("check-config passes: {}", error.outcome()));
    let notes = reported
        .outcome
        .causes()
        .iter()
        .filter(|link| link.relation().as_str() == "note")
        .map(|link| link.outcome().clone())
        .collect();
    (reported.outcome, notes)
}

/// The notes of `device_acib_missing`.
fn acib_notes(notes: &[Outcome]) -> Vec<&Outcome> {
    notes
        .iter()
        .filter(|note| note.code().as_str() == ACIB_MISSING)
        .collect()
}

// A bound iOS handset and no `acib` where its captures run it from: check-config passes with exit
// status 0 and one note `device_acib_missing`, a warning, naming the handset (`aliases`). A
// configuration with a fixture alone passes with no such note.
#[test]
fn f3a_check_config_notes_device_acib_missing_and_passes() {
    let root = TempDir::new().expect("tempdir");
    let (outcome, notes) = check(
        &root,
        vec![json!({
            "alias": IPHONE,
            "instance_id": instance_id(),
            "application_id": "neutral.application",
            "device": {"kind": "handset", "system": "ios"}
        })],
        vec![json!({
            "alias": IPHONE,
            "system": "ios",
            "device_key": KEY,
            "bound_at_unix_ms": 1_700_000_000_479_u64,
            "bound_via": "cli"
        })],
    );
    assert_eq!(outcome.category().exit_code(), 0, "{outcome}");
    let missing = acib_notes(&notes);
    let [note] = missing.as_slice() else {
        panic!("one note {ACIB_MISSING}: {notes:?}");
    };
    assert_eq!(note.category(), Category::Warning);
    assert_eq!(note.value("aliases"), Some(&json!([IPHONE])), "{note:?}");

    let fixture_root = TempDir::new().expect("tempdir");
    let (outcome, notes) = check(
        &fixture_root,
        vec![json!({
            "alias": FIXTURE,
            "instance_id": instance_id(),
            "fixture_backend": {
                "frames": [{"width": 1, "height": 1, "rgb": [1, 2, 3]}],
                "max_inputs": 1
            }
        })],
        Vec::new(),
    );
    assert_eq!(outcome.category().exit_code(), 0, "{outcome}");
    assert!(acib_notes(&notes).is_empty(), "{notes:?}");
}
