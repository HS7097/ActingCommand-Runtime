// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #387 D1c-F, the direct-input producer of the segmented swipe. An input request whose
// single-touch drag breaks its contract is refused by the request's contract check with
// `invalid_segmented_swipe_contract` (class `none`): the receipt is denied, nothing reaches the
// input backend, no plan is prepared and the Runtime goes on.

use super::*;
use actingcommand_contract::codes::RuntimeContractCode;
use actingcommand_contract::outcome::LadderClass;

#[test]
fn d1cf_a_direct_input_swipe_that_breaks_its_contract_is_refused_before_any_input() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let (_, token) = client.acquire("node.a");
    // The brake end lies 50 px above the corner instead of the contract's 100.
    let request = client.request(RuntimeOperation::Input {
        frame: None,
        token,
        action: InputAction::SingleTouchDragWithVerticalBrakeV1 {
            x1: 1091,
            y1: 355,
            x2: 119,
            y2: 363,
            x3: 119,
            y3: 313,
            horizontal_duration_ms: 200,
            corner_hold_ms: 150,
            brake_distance_px: 100,
            brake_duration_ms: 200,
            slope_in: 2,
            slope_out: 0,
        },
    });
    let denied = client.send(&request);
    assert_eq!(denied.state(), RuntimeReceiptState::Denied);
    assert_eq!(
        denied.outcome().code().as_str(),
        RuntimeContractCode::InvalidSegmentedSwipeContract.as_str()
    );
    assert_eq!(
        RuntimeContractCode::InvalidSegmentedSwipeContract.ladder_class(),
        LadderClass::NotApplicable
    );
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    assert!(
        state
            .segmented_swipe_plans
            .lock()
            .expect("fake segmented swipe plans lock")
            .is_empty()
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(client);
    host.close().expect("close runtime");
}
