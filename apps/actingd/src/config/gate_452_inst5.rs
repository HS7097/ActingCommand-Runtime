// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #452 (register §1.300, "Fix #452"; contracts/install-cli.md, "Instances, the first
// slice" ("The application id"); the coordinator's ruling on the work order's point 6, option C):
// the real configuration assembly judges the document the Lab's rescue module's `instance add`
// composes. The document is the checked-in fixture
// `crates/lab-rescue/tests/fixtures/inst5-application-id.json`, read here by its path relative to
// the workspace (no dependency between the two crates); the rescue module's INST-5 gate
// (`install::gate_421_inst5::inst5_452_compose_yields_the_fixtures_candidate`) holds `compose` to
// the same candidate, so neither side can drift from it unnoticed.
// - `inst5_452_the_real_assembly_takes_the_composed_candidate`: the candidate, written as the
//   configuration of a temporary root that holds what it names (its state root and its
//   standard-pack root with the game's root), is loaded and assembled as actingd's start does it
//   (`load`, then `assemble_or_refusal`): it passes; the same document with the added entry's
//   `application_id` removed, as `instance add` composed it before Workflow #452, is refused at
//   the assembly with `application_identity_missing`.
// Workflow #453 S1 (register §1.357; contracts/install-cli.md, "Instances, the first slice" ("A
// first row")): the same judge for the candidate of a first row, the checked-in fixture
// `crates/lab-rescue/tests/fixtures/inst5-first-row.json`, which the rescue module's INST-5 gate
// (`install::gate_421_inst5::inst5_453_s1_a_first_row_takes_the_one_host_and_the_standard_pack_operation`)
// holds `compose` to.
// - `inst5_453_s1_the_real_assembly_takes_a_first_rows_candidate`: the candidate (composer mode,
//   one entry, one row on the one declared host with the standard-pack operation) passes `load`
//   and `assemble_or_refusal`, whose policy check (`PolicyAssembly::validate_instance_identities`)
//   holds the set of `policy.facts.instances[].instance_id` equal to the configured aliases and
//   every row's `host_id` a declared host; the same document with the row's host undeclared is
//   refused there with `policy_instance_host_unknown`, and without the row with
//   `policy_instance_set_mismatch`.
// No device, no Runtime, no clock: the assembly only reads the file.

use super::*;
use crate::codes::ActingdLocation;
use serde_json::Value;
use tempfile::TempDir;

/// The candidate `instance add` composes, with its inputs (crates/lab-rescue's fixture).
const FIXTURE: &str =
    include_str!("../../../../crates/lab-rescue/tests/fixtures/inst5-application-id.json");

/// The code and the stage of a start refusal; `None` when the start goes on.
type Refusal = Option<(&'static str, ActingdLocation)>;

/// Where actingd's start refuses `document`, written as `actingd.config.json` of a temporary root
/// that holds what it names: the state root and, under the standard-pack root, the game root's
/// `bundle.json` and `applications.json` from `fixture`. As the start runs: `load`, then
/// `assemble`.
fn start_refusal(fixture: &Value, document: &Value) -> Refusal {
    let root = TempDir::new().expect("tempdir");
    let named = |pointer: &str| {
        document
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("the document names {pointer}"))
            .to_owned()
    };
    fs::create_dir_all(root.path().join(named("/state_root"))).expect("the state root");
    let game = fixture["game"].as_str().expect("the fixture's game");
    let game_root = root.path().join(named("/standard_packs/root")).join(game);
    fs::create_dir_all(&game_root).expect("the game root");
    for name in ["bundle.json", "applications.json"] {
        fs::write(
            game_root.join(name),
            serde_json::to_vec_pretty(&fixture["game_root"][name]).expect("JSON"),
        )
        .expect("a game root file");
    }
    let path = root.path().join("actingd.config.json");
    fs::write(
        &path,
        serde_json::to_vec_pretty(document).expect("configuration JSON"),
    )
    .expect("write the configuration");
    let file = match load(&path) {
        Ok(file) => file,
        Err(code) => return Some((code.as_str(), ActingdLocation::ConfigLoad)),
    };
    file.assemble_or_refusal()
        .err()
        .map(|(code, _)| (code.as_str(), ActingdLocation::ConfigAssemble))
}

// The coordinator's ruling on point 6 (option C): the fixture's candidate passes the real
// assembly, and the same document without the added entry's `application_id` is refused as the
// candidate of `instance add` before Workflow #452 was.
#[test]
fn inst5_452_the_real_assembly_takes_the_composed_candidate() {
    let fixture: Value = serde_json::from_str(FIXTURE).expect("the fixture's JSON");
    let candidate = &fixture["candidate"];
    assert_eq!(start_refusal(&fixture, candidate), None);

    // As composed before Workflow #452: the added entry (the last) without `application_id`.
    let mut before = candidate.clone();
    let removed = before["instances"]
        .as_array_mut()
        .and_then(|entries| entries.last_mut())
        .and_then(Value::as_object_mut)
        .and_then(|entry| entry.remove("application_id"));
    assert!(
        removed.is_some(),
        "the added entry names its application id"
    );
    assert_eq!(
        start_refusal(&fixture, &before),
        Some((
            "application_identity_missing",
            ActingdLocation::ConfigAssemble
        ))
    );
}

/// Workflow #453 S1: the candidate `instance add` composes for a first row, with its inputs
/// (crates/lab-rescue's fixture).
const FIRST_ROW_FIXTURE: &str =
    include_str!("../../../../crates/lab-rescue/tests/fixtures/inst5-first-row.json");

// Workflow #453 S1, the work order's "Tests": the first row's candidate passes the configuration
// assembly's own rule on the policy's instances and hosts.
#[test]
fn inst5_453_s1_the_real_assembly_takes_a_first_rows_candidate() {
    let fixture: Value = serde_json::from_str(FIRST_ROW_FIXTURE).expect("the fixture's JSON");
    let candidate = &fixture["candidate"];
    assert!(
        candidate.pointer("/policy/catalog").is_none(),
        "composer mode"
    );
    assert_eq!(start_refusal(&fixture, candidate), None);

    // The rule is in force on this document: the row's host undeclared, and the row gone.
    let mut undeclared = candidate.clone();
    undeclared["policy"]["facts"]["instances"][0]["host_id"] =
        Value::from("neutral.host.undeclared");
    assert_eq!(
        start_refusal(&fixture, &undeclared),
        Some((
            "policy_instance_host_unknown",
            ActingdLocation::ConfigAssemble
        ))
    );
    let mut rowless = candidate.clone();
    rowless["policy"]["facts"]["instances"] = Value::Array(Vec::new());
    assert_eq!(
        start_refusal(&fixture, &rowless),
        Some((
            "policy_instance_set_mismatch",
            ActingdLocation::ConfigAssemble
        ))
    );
}
