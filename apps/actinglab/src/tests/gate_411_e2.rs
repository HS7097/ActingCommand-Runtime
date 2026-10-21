// SPDX-License-Identifier: AGPL-3.0-only

// #411 E2 T3 (A37, 0.12.9), actinglab's part: online `do --capture` (`ac_lab_do` with capture)
// whose element id is one of the package's recognition target ids answers the Runtime's
// `lab_target_type_mismatch`, with the expected and the received target type the receipt's
// outcome names (`expected_target_type`, `received_target_type`), not `capability_insufficient`
// ("element unavailable"). The answer keeps `arrival` last (#411 A19). Every expected value comes
// from the slice brief's A37 requirement line; no Runtime, clock or device is involved.

use super::*;
use actingcommand_contract::LabTargetType;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{RuntimeErrorCode, RuntimeErrorProjection};

const TARGET_TYPE_MISMATCH: &str = "lab_target_type_mismatch";

/// The failed operation's answer: refused at selection, its arrival never started.
fn refused_answer() -> Value {
    json!({
        "req_id": "req_0001",
        "correlation_id": "cor_0001",
        "state": "failed",
        "instance": "node.a",
        "lease_id": "lease_0001",
        "action_id": null,
        "executed": false,
        "effect": "not_performed",
        "ledger": {"authority": "runtime_global_ledger", "prepared_sequence": 11, "input_sequence": null, "terminal_sequence": 14},
        "failure": {
            "stage": "selection",
            "code": TARGET_TYPE_MISMATCH,
            "error": {"code": RuntimeErrorCode::RecognitionFailed.as_str(), "fatal": false},
            "event": null
        },
        "after_condition": {"page_id": "neutral/next", "timeout_ms": 1000},
        "arrival": {"status": "aborted", "elapsed_ms": 0, "samples": []},
        "frame_coordinates": {"device_pixels": [2, 1], "orientation": "landscape", "applies_to": "before"}
    })
}

#[test]
fn e2_t3_a_target_id_where_the_element_id_belongs_answers_the_type_mismatch_with_both_types() {
    let receipt = Outcome::new(SharedCode::LabTargetTypeMismatch)
        .expected_target_type(LabTargetType::ElementId)
        .received_target_type(LabTargetType::TargetId);
    let failure = LabOperationFailure {
        stage: LabOperationStage::Selection,
        code: TARGET_TYPE_MISMATCH.to_string(),
        error: RuntimeErrorProjection::new(RuntimeErrorCode::RecognitionFailed, false),
        event: None,
    };

    let error = operation_failure_error(
        &failure,
        with_target_types(arrival_last(refused_answer()), &receipt),
    );

    assert_eq!(error.code, TARGET_TYPE_MISMATCH);
    let details = error.details.expect("the operation's answer");
    assert_eq!(details["expected_target_type"], json!("element_id"));
    assert_eq!(details["received_target_type"], json!("target_id"));
    assert_eq!(
        details
            .as_object()
            .and_then(|members| members.keys().last())
            .map(String::as_str),
        Some("arrival")
    );
}
