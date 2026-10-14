// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #336 PKG-S2 (option 丙), model M-PKG v1.1 §4.3 (contract gate K22): the ledger
// shapes name a package by its `package_id` (`package_label`) and carry no package digest. A
// record this build writes reads back unchanged; a stored record that still carries a removed
// digest member is not read (no old-version compatibility, register §2).

use super::sh1::{holder, read_back, written};
use super::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn semantic(fact: TaskSemanticFact) -> Value {
    written(TaskPayloadDraft::semantic(fact, AuditInput::new()).into())
}

fn members(object: &serde_json::Map<String, Value>) -> BTreeSet<&str> {
    object.keys().map(String::as_str).collect()
}

// The stored record with `member` added to the object that holds `holding`.
fn with_member(mut stored: Value, holding: &str, member: &str, value: Value) -> Value {
    holder(&mut stored, holding)
        .expect("the holding object")
        .insert(member.to_owned(), value);
    stored
}

fn not_read(stored: Value) -> bool {
    serde_json::from_value::<EventPayload>(stored).is_err()
}

// S2-T1: `PackageAdmitted` has only `package_label` and `task_label` (with its response
// deadline); every entry-recovery fact names its layer's package by `package_label`. Each
// reads back unchanged, and none is read with a `package_sha256` member.
#[test]
fn s2_t1_package_facts_name_their_package_by_label_and_refuse_a_digest() {
    let mut admitted = semantic(TaskSemanticFact::PackageAdmitted {
        package_label: "neutral.main".to_owned(),
        task_label: "task".to_owned(),
        response_deadline_monotonic_ms: Some(60_000),
    });
    read_back(&admitted);
    assert_eq!(
        members(holder(&mut admitted, "package_label").expect("the admission fact")),
        BTreeSet::from([
            "kind",
            "package_label",
            "task_label",
            "response_deadline_monotonic_ms",
        ])
    );

    let layers = [
        semantic(TaskSemanticFact::EntryRecoveryPackageAdmitted {
            package_label: "neutral.outer".to_owned(),
            role: None,
        }),
        semantic(TaskSemanticFact::EntryRecoveryCompleted {
            package_label: "neutral.outer".to_owned(),
            final_page: "home".to_owned(),
            executed_steps: 1,
            role: None,
        }),
        semantic(TaskSemanticFact::EntryRecoveryFailed {
            package_label: Some("neutral.inner".to_owned()),
            outcome: Outcome::relayed("contained_task_page_unknown"),
            role: None,
        }),
    ];
    for stored in std::iter::once(&admitted).chain(&layers) {
        read_back(stored);
        assert!(
            not_read(with_member(
                stored.clone(),
                "package_label",
                "package_sha256",
                json!("c".repeat(64))
            )),
            "{stored}"
        );
    }

    // A layer refused before its `package_id` was read records its failure without one.
    read_back(&semantic(TaskSemanticFact::EntryRecoveryFailed {
        package_label: None,
        outcome: Outcome::relayed("content_directory_missing"),
        role: None,
    }));
}

// S2-T1 (a refusal record): a resource declaration refusal names the refused package by an
// optional `package_label`; a stored refusal that still carries the declared or the verified
// package reference is not read.
#[test]
fn s2_t1_a_declaration_refusal_names_its_package_by_label_only() {
    let refusal = json!({
        "package_label": "neutral.main",
        "program_version": "actingcommand-runtime-host/0.12.0",
        "issue": {
            "declaration_file": "resources/operations/task/task.json",
            "field_path": "/operations/0",
            "schema_version": null,
            "reason": "unknown_field"
        }
    });
    let decoded: crate::ResourceDeclarationRejection =
        serde_json::from_value(refusal.clone()).expect("the refusal decodes");
    decoded.validate().expect("the refusal validates");
    assert_eq!(serde_json::to_value(&decoded).expect("encodes"), refusal);
    let mut unnamed = refusal.clone();
    unnamed
        .as_object_mut()
        .expect("refusal object")
        .remove("package_label");
    let decoded: crate::ResourceDeclarationRejection =
        serde_json::from_value(unnamed).expect("a refusal without a label decodes");
    decoded
        .validate()
        .expect("a refusal without a label validates");
    for member in ["declared_package", "verified_package"] {
        let mut stored = refusal.clone();
        stored.as_object_mut().expect("refusal object").insert(
            member.to_owned(),
            json!(format!("sha256:{}", "c".repeat(64))),
        );
        assert!(
            serde_json::from_value::<crate::ResourceDeclarationRejection>(stored).is_err(),
            "{member}"
        );
    }
}

// S2-T7 (the record): a select step's `task.selection_evaluated` names its policy by path and
// the evaluator's identities only; it reads back unchanged, and a stored record that carries
// `policy.package_sha256` is not read.
#[test]
fn s2_t7_a_selection_record_carries_no_package_pin() {
    let stored = written(super::revision_3_selection(
        TaskSelectionConfirmation::CaptureFailed {
            outcome: Outcome::relayed("selection_confirmation_capture_failed"),
        },
    ));
    read_back(&stored);
    let mut policy_record = stored.clone();
    assert_eq!(
        members(holder(&mut policy_record, "policy_sha256").expect("the record's policy")),
        BTreeSet::from(["path", "policy_sha256", "policy_id"])
    );
    assert!(not_read(with_member(
        stored,
        "policy_sha256",
        "package_sha256",
        json!("c".repeat(64))
    )));
}
