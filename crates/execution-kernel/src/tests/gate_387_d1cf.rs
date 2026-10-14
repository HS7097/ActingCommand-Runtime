// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #387 D1c-F, the residual of `input_plan_preparation_failed`. Every producer refuses an
// invalid segmented swipe under its own code before the session (a pack at admission and when
// its action is built, a request by its contract check), so a plan that still fails to prepare
// is a Runtime-internal defect of one action: class `none`, so it never feeds direction ①, with
// no device severity and no resource disposition, and no backend opened or touched.

use super::{FakeState, instance, kernel, step_witness};
use crate::ExecutionKernelCode;
use actingcommand_contract::InputAction;
use actingcommand_contract::outcome::LadderClass;
use std::sync::{Arc, Mutex};

#[test]
fn d1cf_a_swipe_that_reaches_preparation_unvalidated_fails_as_a_runtime_internal_error() {
    let state = Arc::new(Mutex::new(FakeState::default()));
    let kernel = kernel(Arc::clone(&state), &[("node.a", instance(), "private-a")]);
    // The brake end lies 50 px above the corner instead of 100: the contract refuses this
    // action, so only Runtime code that skipped validation hands it to preparation.
    let action = InputAction::SingleTouchDragWithVerticalBrakeV1 {
        x1: 10,
        y1: 200,
        x2: 100,
        y2: 200,
        x3: 100,
        y3: 150,
        horizontal_duration_ms: 200,
        corner_hold_ms: 150,
        brake_distance_px: 100,
        brake_duration_ms: 200,
        slope_in: 2,
        slope_out: 0,
    };
    assert!(action.validate().is_err());

    let prepared = kernel
        .prepare_input(action.clone())
        .expect_err("preparation refuses the plan");
    let input = kernel
        .input("node.a", action, step_witness())
        .expect_err("the input entry refuses the plan");
    for error in [prepared, input] {
        assert_eq!(
            error.code().as_str(),
            ExecutionKernelCode::InputPlanPreparationFailed.as_str()
        );
        assert!(error.device_severity().is_none());
        assert!(error.failure_context().resource_dispositions().is_none());
        assert!(error.resource_quiescence().is_none());
        assert_eq!(error.resource_count(), 0);
    }
    assert_eq!(
        ExecutionKernelCode::InputPlanPreparationFailed.ladder_class(),
        LadderClass::NotApplicable
    );
    let snapshot = state.lock().expect("state");
    assert_eq!((snapshot.input_opens, snapshot.input_calls), (0, 0));
}
