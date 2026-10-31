// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #429 R-15b, test E02 of the frozen design (`rd-1031/design-r15b-ab-v4.1.md` §2.3,
// §2.4 and §7 R-15b): install selection v2 refuses each of its rules with its code, round-trips,
// takes 16 KiB and not one byte more, and refuses a Runtime paired with the Tools of another
// build; the slot document likewise, and it matches only the selection of its own slot and
// members. Every expected value comes from the design and `contracts/install-cli.md`, "The A/B
// layout". Neutral repositories and digits only.

use super::*;
use crate::RuntimeContractResult;
use serde_json::{Value, json};

const RUNTIME_REPOSITORY: &str = "neutral/runtime";
const UI_REPOSITORY: &str = "neutral/ui";
const C1: &str = "1111111111111111111111111111111111111111";
const C2: &str = "2222222222222222222222222222222222222222";
const U1: &str = "3333333333333333333333333333333333333333";
const SHA_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SHA_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const SHA_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const SHA_D: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";

/// A valid selection of slot A, as design §2.3 shows it.
fn selection() -> Value {
    json!({
        "schema_version": "actingcommand.install-selection.v2",
        "selection_id": "sel-1730352000000",
        "generation": 3,
        "slot": "A",
        "slot_document": {"path": "A/SLOT.json", "sha256": SHA_A},
        "members": {
            "runtime": {"repository": RUNTIME_REPOSITORY, "commit": C1, "tag": "v0.0.1-test", "manifest_sha256": SHA_B},
            "tools": {"repository": RUNTIME_REPOSITORY, "commit": C1, "tag": "v0.0.1-test", "manifest_sha256": SHA_C},
            "ui": {"repository": UI_REPOSITORY, "commit": U1, "tag": null, "manifest_sha256": SHA_D}
        },
        "made_by": "op-1730352000000"
    })
}

/// A valid slot document of slot A whose members are the selection's.
fn slot_document() -> Value {
    json!({
        "schema_version": "actingcommand.install-slot.v1",
        "slot": "A",
        "prepared_by": "op-1730351000000",
        "members": {
            "runtime": {"repository": RUNTIME_REPOSITORY, "commit": C1, "tag": "v0.0.1-test", "manifest_sha256": SHA_B,
                        "source": format!("install/builds/{C1}/runtime"), "files": 6, "bytes": 600},
            "tools": {"repository": RUNTIME_REPOSITORY, "commit": C1, "tag": "v0.0.1-test", "manifest_sha256": SHA_C,
                      "source": format!("install/builds/{C1}/tools"), "files": 4, "bytes": 400},
            "ui": {"repository": UI_REPOSITORY, "commit": U1, "tag": null, "manifest_sha256": SHA_D,
                   "source": format!("install/ui-builds/{U1}/ui"), "files": 4, "bytes": 300}
        }
    })
}

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("JSON")
}

fn code<T: std::fmt::Debug>(result: RuntimeContractResult<T>) -> &'static str {
    result.expect_err("refused").code().as_str()
}

/// `value` with the member at `pointer` (a JSON pointer) set to `to`.
fn with(mut value: Value, pointer: &str, to: Value) -> Value {
    *value.pointer_mut(pointer).expect("the member exists") = to;
    value
}

/// `value` with the member `name` of the object at `pointer` removed.
fn without(mut value: Value, pointer: &str, name: &str) -> Value {
    value
        .pointer_mut(pointer)
        .and_then(Value::as_object_mut)
        .expect("an object")
        .remove(name)
        .expect("the member exists");
    value
}

/// `value` with the member `name` added to the object at `pointer`.
fn plus(mut value: Value, pointer: &str, name: &str, to: Value) -> Value {
    value
        .pointer_mut(pointer)
        .and_then(Value::as_object_mut)
        .expect("an object")
        .insert(name.to_owned(), to);
    value
}

#[test]
fn r15b_a_selection_v2_round_trips_and_names_its_slot() {
    let parsed = InstallSelectionV2::from_json(&bytes(&selection())).expect("a valid selection");
    assert_eq!(parsed.slot, InstallSlot::A);
    assert_eq!(parsed.generation, 3);
    assert_eq!(parsed.members.ui.tag, None);
    assert_eq!(
        serde_json::to_value(&parsed).expect("JSON"),
        selection(),
        "every member is written back, the null tag included"
    );
    let again = InstallSelectionV2::from_json(&serde_json::to_vec(&parsed).expect("JSON"))
        .expect("the written selection reads");
    assert_eq!(again, parsed);
    assert_eq!(
        parsed.slot_root(std::path::Path::new("root")),
        std::path::Path::new("root").join("A")
    );
    assert_eq!(slot_document_path(InstallSlot::B), "B/SLOT.json");
}

#[test]
fn r15b_each_rule_of_the_selection_refuses_with_its_code() {
    let long_tag = "t".repeat(129);
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "schema v1",
            with(
                selection(),
                "/schema_version",
                json!("actingcommand.install-selection.v1"),
            ),
            "install_selection_v2_invalid",
        ),
        (
            "selection id without digits",
            with(selection(), "/selection_id", json!("sel-")),
            "install_selection_v2_invalid",
        ),
        (
            "selection id with a letter",
            with(selection(), "/selection_id", json!("sel-17a")),
            "install_selection_v2_invalid",
        ),
        (
            "selection id of 21 digits",
            with(
                selection(),
                "/selection_id",
                json!(format!("sel-{}", "1".repeat(21))),
            ),
            "install_selection_v2_invalid",
        ),
        (
            "selection id of another prefix",
            with(selection(), "/selection_id", json!("op-1730352000000")),
            "install_selection_v2_invalid",
        ),
        (
            "generation 0",
            with(selection(), "/generation", json!(0)),
            "install_selection_v2_invalid",
        ),
        (
            "made_by not an operation",
            with(selection(), "/made_by", json!("sel-1730352000000")),
            "install_selection_v2_invalid",
        ),
        (
            "slot document of the other slot",
            with(selection(), "/slot_document/path", json!("B/SLOT.json")),
            "install_slot_document_mismatch",
        ),
        (
            "slot document by the v1 name",
            with(selection(), "/slot_document/path", json!("A/MEMBERS.json")),
            "install_slot_document_mismatch",
        ),
        (
            "slot document path with ..",
            with(selection(), "/slot_document/path", json!("A/../SLOT.json")),
            "invalid_install_file_reference",
        ),
        (
            "slot document digest in upper case",
            with(
                selection(),
                "/slot_document/sha256",
                json!(SHA_A.to_ascii_uppercase()),
            ),
            "invalid_install_file_reference",
        ),
        (
            "repository without owner",
            with(selection(), "/members/ui/repository", json!("ui")),
            "install_member_identity_invalid",
        ),
        (
            "commit of 39 digits",
            with(selection(), "/members/runtime/commit", json!(&C1[..39])),
            "install_member_identity_invalid",
        ),
        (
            "commit in upper case",
            with(
                selection(),
                "/members/ui/commit",
                json!("ABCDEF1111111111111111111111111111111111"),
            ),
            "install_member_identity_invalid",
        ),
        (
            "manifest digest of 63 digits",
            with(
                selection(),
                "/members/tools/manifest_sha256",
                json!(&SHA_C[..63]),
            ),
            "install_member_identity_invalid",
        ),
        (
            "empty tag",
            with(selection(), "/members/runtime/tag", json!("")),
            "install_member_identity_invalid",
        ),
        (
            "tag of 129 characters",
            with(selection(), "/members/ui/tag", json!(long_tag)),
            "install_member_identity_invalid",
        ),
        (
            "tag with a space",
            with(selection(), "/members/ui/tag", json!("v0 1")),
            "install_member_identity_invalid",
        ),
        (
            "an unknown member",
            plus(selection(), "", "note", json!("x")),
            "install_selection_json_invalid",
        ),
        (
            "v1's config member",
            plus(
                selection(),
                "",
                "config",
                json!({"path": "install/generations/3/actingd.config.json", "sha256": SHA_A}),
            ),
            "install_selection_json_invalid",
        ),
        (
            "an unknown member of a member",
            plus(selection(), "/members/runtime", "source", json!("x")),
            "install_selection_json_invalid",
        ),
        (
            "no UI member",
            without(selection(), "/members", "ui"),
            "install_selection_json_invalid",
        ),
        (
            "no made_by",
            without(selection(), "", "made_by"),
            "install_selection_json_invalid",
        ),
        (
            "slot C",
            with(selection(), "/slot", json!("C")),
            "install_selection_json_invalid",
        ),
    ];
    for (name, document, expected) in rows {
        assert_eq!(
            code(InstallSelectionV2::from_json(&bytes(&document))),
            expected,
            "{name}"
        );
    }
    assert_eq!(
        code(InstallSelectionV2::from_json(b"{")),
        "install_selection_json_invalid",
        "not JSON"
    );
}

#[test]
fn r15b_the_selection_takes_16_kib_and_not_one_byte_more() {
    let compact = bytes(&selection());
    let padded = |length: usize| {
        let mut document = compact.clone();
        document.resize(length, b' ');
        document
    };
    assert_eq!(MAX_INSTALL_SELECTION_BYTES, 16 * 1024);
    InstallSelectionV2::from_json(&padded(MAX_INSTALL_SELECTION_BYTES))
        .expect("16 KiB exactly reads");
    assert_eq!(
        code(InstallSelectionV2::from_json(&padded(
            MAX_INSTALL_SELECTION_BYTES + 1
        ))),
        "install_selection_size_invalid"
    );
    assert_eq!(
        code(InstallSelectionV2::from_json(b"")),
        "install_selection_size_invalid"
    );
}

#[test]
fn r15b_a_runtime_paired_with_the_tools_of_another_build_is_refused() {
    for (name, pointer, to) in [
        ("another commit", "/members/tools/commit", json!(C2)),
        (
            "another repository",
            "/members/tools/repository",
            json!("neutral/other"),
        ),
        (
            "the runtime of another commit",
            "/members/runtime/commit",
            json!(C2),
        ),
    ] {
        assert_eq!(
            code(InstallSelectionV2::from_json(&bytes(&with(
                selection(),
                pointer,
                to
            )))),
            "install_member_pairing_mismatch",
            "{name}"
        );
    }
    // The UI has its own build: another commit than the Runtime's pairs nothing.
    InstallSelectionV2::from_json(&bytes(&with(selection(), "/members/ui/commit", json!(C2))))
        .expect("the UI's commit is its own");
}

#[test]
fn r15b_a_selection_follows_its_baseline_by_one_generation() {
    let at = |generation: u64| {
        InstallSelectionV2::from_json(&bytes(&with(selection(), "/generation", json!(generation))))
            .expect("a valid selection")
    };
    at(1)
        .validate_successor(None)
        .expect("a root's first selection is generation 1");
    assert_eq!(
        code(at(2).validate_successor(None)),
        "install_selection_v2_invalid"
    );
    let baseline = at(3);
    at(4)
        .validate_successor(Some(&baseline))
        .expect("the baseline's plus one");
    for generation in [3, 5] {
        assert_eq!(
            code(at(generation).validate_successor(Some(&baseline))),
            "install_selection_v2_invalid",
            "generation {generation} after 3"
        );
    }
}

#[test]
fn r15b_a_slot_document_round_trips_and_refuses_each_rule() {
    let parsed =
        InstallSlotDocument::from_json(&bytes(&slot_document())).expect("a valid slot document");
    assert_eq!(
        serde_json::to_value(&parsed).expect("JSON"),
        slot_document()
    );
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "schema",
            with(
                slot_document(),
                "/schema_version",
                json!("actingcommand.install-slot.v2"),
            ),
            "install_slot_document_invalid",
        ),
        (
            "prepared_by",
            with(slot_document(), "/prepared_by", json!("prepare")),
            "install_slot_document_invalid",
        ),
        (
            "an unknown member",
            plus(slot_document(), "/members/ui", "path", json!("ui")),
            "install_slot_document_invalid",
        ),
        (
            "no tools member",
            without(slot_document(), "/members", "tools"),
            "install_slot_document_invalid",
        ),
        (
            "a source of another commit",
            with(
                slot_document(),
                "/members/runtime/source",
                json!(format!("install/builds/{C2}/runtime")),
            ),
            "install_member_identity_invalid",
        ),
        (
            "the UI from the Runtime builds",
            with(
                slot_document(),
                "/members/ui/source",
                json!(format!("install/builds/{U1}/ui")),
            ),
            "install_member_identity_invalid",
        ),
        (
            "a member without files",
            with(slot_document(), "/members/tools/files", json!(0)),
            "install_member_identity_invalid",
        ),
        (
            "a member identity",
            with(
                slot_document(),
                "/members/ui/manifest_sha256",
                json!("sha256"),
            ),
            "install_member_identity_invalid",
        ),
        (
            "the Tools of another build",
            with(
                with(slot_document(), "/members/tools/commit", json!(C2)),
                "/members/tools/source",
                json!(format!("install/builds/{C2}/tools")),
            ),
            "install_member_pairing_mismatch",
        ),
    ];
    for (name, document, expected) in rows {
        assert_eq!(
            code(InstallSlotDocument::from_json(&bytes(&document))),
            expected,
            "{name}"
        );
    }
    let mut large = bytes(&slot_document());
    large.resize(MAX_INSTALL_SLOT_DOCUMENT_BYTES, b' ');
    InstallSlotDocument::from_json(&large).expect("64 KiB exactly reads");
    large.push(b' ');
    assert_eq!(
        code(InstallSlotDocument::from_json(&large)),
        "install_slot_document_invalid"
    );
    assert_eq!(
        code(InstallSlotDocument::from_json(b"")),
        "install_slot_document_invalid"
    );
}

#[test]
fn r15b_a_slot_document_matches_only_the_selection_of_its_slot_and_members() {
    let document =
        InstallSlotDocument::from_json(&bytes(&slot_document())).expect("a valid slot document");
    let selection_of = |value: Value| InstallSelectionV2::from_json(&bytes(&value)).expect("valid");
    document
        .matches_selection(&selection_of(selection()))
        .expect("the selection of its own slot and members");
    let other_slot = with(
        with(selection(), "/slot", json!("B")),
        "/slot_document/path",
        json!("B/SLOT.json"),
    );
    let other_ui = with(selection(), "/members/ui/commit", json!(C2));
    let other_tag = with(selection(), "/members/runtime/tag", json!(null));
    for (name, other) in [
        ("another slot", other_slot),
        ("another UI build", other_ui),
        ("another tag", other_tag),
    ] {
        assert_eq!(
            code(document.matches_selection(&selection_of(other))),
            "install_slot_document_mismatch",
            "{name}"
        );
    }
}

#[test]
fn r15b_the_environment_names_and_bounds_are_fixed() {
    assert_eq!(INSTALL_OPERATION_ENV, "ACTINGCOMMAND_INSTALL_OPERATION");
    assert_eq!(ENTRY_NO_DIALOG_ENV, "ACTINGCOMMAND_ENTRY_NO_DIALOG");
    assert_eq!(
        INSTALL_SELECTION_V2_SCHEMA,
        "actingcommand.install-selection.v2"
    );
    assert_eq!(INSTALL_SLOT_SCHEMA, "actingcommand.install-slot.v1");
    assert_eq!(MAX_INSTALL_SLOT_DOCUMENT_BYTES, 64 * 1024);
    assert!(is_selection_id("sel-1") && !is_selection_id("sel-1-2"));
    assert!(is_operation_id("op-1") && !is_operation_id("op-"));
}
