// SPDX-License-Identifier: AGPL-3.0-only

// #411 A19 (#411 comment 6258457612; register §1.109; exec gate key 93): online
// `do --capture --after-page` whose arrival wait times out answers "the page did not arrive" in
// its own code, `lab_arrival_timeout`, exit 3, blocked by the arrival page, as a navigation step
// whose page did not arrive answers it, not `device_error` (contained-lab-operation.md: a timeout
// produces a failed receipt and a nonzero CLI result). Every other failed record keeps its
// answer. In every online `do` answer `arrival` is the last member, so a reader that cuts the
// text loses the arrival's samples first. Every expected value comes from the slice's
// requirement; no Runtime, clock or device is involved.

use super::*;
use actingcommand_contract::codes::LabErrorCode;
use actingcommand_contract::{RuntimeErrorCode, RuntimeErrorProjection};
use actingcommand_runtime_host::HostCode;

fn failure(stage: LabOperationStage, code: &str, runtime: RuntimeErrorCode) -> LabOperationFailure {
    LabOperationFailure {
        stage,
        code: code.to_string(),
        error: RuntimeErrorProjection::new(runtime, false),
        event: None,
    }
}

/// An online `do` answer with today's member order: the operation's members, then `arrival`
/// before `frame_coordinates`, `projection_schema_version` and `after_attachment`.
fn answer_with_arrival() -> Value {
    json!({
        "req_id": "req_0001",
        "correlation_id": "cor_0001",
        "state": "failed",
        "instance": "node.a",
        "lease_id": "lease_0001",
        "action_id": "act_0001",
        "executed": true,
        "effect": "performed",
        "ledger": {"authority": "runtime_global_ledger", "prepared_sequence": 11, "input_sequence": 12, "terminal_sequence": 19},
        "failure": {
            "stage": "arrival",
            "code": SharedCode::LabArrivalTimeout.as_str(),
            "error": {"code": RuntimeErrorCode::RecognitionFailed.as_str(), "fatal": false},
            "event": null
        },
        "after_condition": {"page_id": "home", "timeout_ms": 1000},
        "arrival": {
            "status": "timed_out",
            "elapsed_ms": 1000,
            "samples": [
                {"frame": {"frame_id": "frame_0002"}, "projection": {"artifact": "projection_0002"}},
                {"frame": {"frame_id": "frame_0003"}, "projection": {"artifact": "projection_0003"}},
                {"frame": {"frame_id": "frame_0004"}, "projection": null}
            ]
        },
        "frame_coordinates": {"device_pixels": [1280, 720], "orientation": "landscape", "applies_to": "before"},
        "projection_schema_version": "projection.v1",
        "after_attachment": {"kind": "frame", "frame_path": "frame_0004.png"}
    })
}

fn member_names(value: &Value) -> Vec<String> {
    value.as_object().unwrap().keys().cloned().collect()
}

#[test]
fn a19_a_timed_out_arrival_is_refused_as_not_reached_naming_lab_arrival_timeout() {
    let details = answer_with_arrival();
    let error = operation_failure_error(
        &failure(
            LabOperationStage::Arrival,
            SharedCode::LabArrivalTimeout.as_str(),
            RuntimeErrorCode::RecognitionFailed,
        ),
        details.clone(),
    );
    assert_eq!(error.class, ErrorKind::SafetyBlocked);
    assert_eq!(error.exit_code(), 3);
    assert_eq!(error.code, SharedCode::LabArrivalTimeout.as_str());
    assert_eq!(error.blocked_by, vec!["arrival_page".to_string()]);
    assert_eq!(error.details, Some(details));
}

#[test]
fn a19_every_other_operation_failure_keeps_its_answer() {
    let device_error = LabErrorCode::DeviceError.as_str();
    let cases = [
        (
            LabOperationStage::Selection,
            RuntimeContractCode::LabElementUnavailable.as_str(),
            RuntimeErrorCode::RecognitionFailed,
            ErrorKind::SafetyBlocked,
            "capability_insufficient",
            3,
        ),
        (
            LabOperationStage::AfterFrame,
            HostCode::LabArrivalShutdown.as_str(),
            RuntimeErrorCode::RuntimeUnavailable,
            ErrorKind::DeviceInstance,
            device_error,
            4,
        ),
        (
            LabOperationStage::AfterProjection,
            HostCode::LabArrivalObservationFailed.as_str(),
            RuntimeErrorCode::RecognitionFailed,
            ErrorKind::DeviceInstance,
            device_error,
            4,
        ),
        (
            LabOperationStage::Input,
            HostCode::LabInputOutcomeMissing.as_str(),
            RuntimeErrorCode::BackendOperationFailed,
            ErrorKind::DeviceInstance,
            device_error,
            4,
        ),
    ];
    for (stage, code, runtime, class, expected_code, exit) in cases {
        let details = answer_with_arrival();
        let error = operation_failure_error(&failure(stage, code, runtime), details.clone());
        assert_eq!(error.class, class, "{stage:?} {code}");
        assert_eq!(error.exit_code(), exit, "{stage:?} {code}");
        assert_eq!(error.code, expected_code, "{stage:?} {code}");
        assert_eq!(error.details, Some(details), "{stage:?} {code}");
    }
}

#[test]
fn a19_the_answer_puts_arrival_last_and_changes_nothing_else() {
    let answer = answer_with_arrival();
    let ordered = arrival_last(answer.clone());
    let mut expected_order = member_names(&answer);
    expected_order.retain(|name| name != "arrival");
    expected_order.push("arrival".to_string());
    assert_eq!(member_names(&ordered), expected_order);
    assert_eq!(
        member_names(&ordered).last().map(String::as_str),
        Some("arrival")
    );
    for name in member_names(&answer) {
        assert_eq!(
            ordered[name.as_str()],
            answer[name.as_str()],
            "member {name}"
        );
    }
    assert_eq!(ordered, answer);
    assert_eq!(
        member_names(&ordered["arrival"]),
        ["status", "elapsed_ms", "samples"]
    );

    let mut without = answer_with_arrival();
    without.as_object_mut().unwrap().shift_remove("arrival");
    let unchanged = arrival_last(without.clone());
    assert_eq!(member_names(&unchanged), member_names(&without));
    assert_eq!(unchanged, without);
}
