// SPDX-License-Identifier: AGPL-3.0-only

// #412 Lab B-3 S2 (frozen design rd-1021/train-0129/lab-b3-design.md §1.2 "Exits" and §5 "S2",
// T2-6; register §1.162; apps key 91): how online `do --operation` exits. 0 when the operation
// ended `confirmed` or `run_completed`; 3, `safety_blocked` with the failure's code, blocked by
// `page` (a page mismatch, the candidates exhausted), `guard` (the guard refused, stability or
// still unconfirmed) or `arrival_page` (the step unconfirmed, the page confirmation failed, an
// error page reached, `contained_task_timeout` after an input, recovery requested); anything else
// 4, as today. The refusals before any connection are `tests::gate_412_b3_s2`. No Runtime, clock
// or device is involved.

use super::*;
use actingcommand_contract::{RuntimeErrorCode, RuntimeErrorProjection};

fn failure(stage: LabOperationStage, code: &str) -> LabOperationFailure {
    LabOperationFailure {
        stage,
        code: code.to_string(),
        error: RuntimeErrorProjection::new(RuntimeErrorCode::BackendOperationFailed, false),
        event: None,
    }
}

#[test]
fn b3_s2_t2_6_do_operation_exits_by_the_design_table() {
    use LabOperationEnd::{Confirmed, Failed, PassedOver, RecoveryRequested, RunCompleted};
    use LabOperationStage::{AfterFrame, Arrival, Input, Release, Selection};

    // Exit 0: the operation ended confirmed or run_completed, a release failure after it
    // included.
    for (end, failed) in [
        (Confirmed, None),
        (RunCompleted, None),
        (Confirmed, Some(failure(Release, "lease_missing"))),
    ] {
        assert!(
            declared_operation_error(Some(end), failed.as_ref()).is_none(),
            "{end:?} exits 0"
        );
    }

    // Exit 3: stopped on the screen, blocked by what stopped it.
    for (end, stage, code, blocked_by) in [
        (
            Failed,
            Selection,
            "contained_task_declared_operation_page_mismatch",
            "page",
        ),
        (
            Failed,
            Selection,
            "contained_task_operation_candidates_exhausted",
            "page",
        ),
        (
            PassedOver,
            Selection,
            "contained_task_guard_refused",
            "guard",
        ),
        (Failed, Selection, "contained_task_guard_refused", "guard"),
        (
            Failed,
            Selection,
            "contained_task_input_stability_unconfirmed",
            "guard",
        ),
        (
            Failed,
            Selection,
            "contained_task_input_still_unconfirmed",
            "guard",
        ),
        (
            Failed,
            Arrival,
            "contained_task_step_unconfirmed",
            "arrival_page",
        ),
        (Failed, Arrival, "page_confirmation_failed", "arrival_page"),
        (
            Failed,
            Arrival,
            "contained_task_error_page_reached",
            "arrival_page",
        ),
        (Failed, Arrival, "contained_task_timeout", "arrival_page"),
        (
            RecoveryRequested,
            Arrival,
            "page_confirmation_failed",
            "arrival_page",
        ),
    ] {
        let what = format!("{end:?} at {stage:?} with {code}");
        let error = declared_operation_error(Some(end), Some(&failure(stage, code)))
            .unwrap_or_else(|| panic!("{what} is refused"));
        assert_eq!(error.class, ErrorKind::SafetyBlocked, "{what}");
        assert_eq!(error.exit_code(), 3, "{what}");
        assert_eq!(error.code, code, "{what}");
        assert_eq!(error.blocked_by, vec![blocked_by.to_string()], "{what}");
    }

    // Exit 4: anything else, the task's timeout before an input included.
    for (stage, code) in [
        (Selection, "contained_task_timeout"),
        (AfterFrame, "lab_operation_frame_limit_reached"),
        (Input, "backend_operation_failed"),
    ] {
        let what = format!("{stage:?} with {code}");
        let error = declared_operation_error(Some(Failed), Some(&failure(stage, code)))
            .unwrap_or_else(|| panic!("{what} is refused"));
        assert_eq!(error.class, ErrorKind::DeviceInstance, "{what}");
        assert_eq!(error.exit_code(), 4, "{what}");
    }
}
