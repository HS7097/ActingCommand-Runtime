// SPDX-License-Identifier: AGPL-3.0-only

//! #411 Lab L5a (host key 120; register §1.130 L5-4): the receipt waits of `observe --capture`
//! and `do --capture` allow, for each observation the request can make, the frames its
//! package's target consensus needs: one more backend-open allowance and one more interval for
//! every frame before the current one.

use crate::client::{lab_operation_response_timeout, observe_response_timeout};
use actingcommand_contract::{
    ContainedLabOperationRequest, InputAction, LabArrivalCondition, LabOperationSelection,
    LabProjectionHint, MAX_INPUT_DURATION_MS, ObservationFramePlan,
};
use std::time::Duration;

// Three frames 200 ms apart: observe allows three backend opens and the two waits; a do with an
// arrival condition allows today's wait and, for each of its 21 observations, two more backend
// opens and the two waits.
#[test]
fn l5_the_client_allows_for_the_transaction() {
    let frames = ObservationFramePlan::new(3, 200).expect("three frames");
    let backend_open = Duration::from_secs(10);
    let io_timeout = Duration::from_millis(500);
    assert_eq!(
        observe_response_timeout(backend_open, frames),
        Some(backend_open * 3 + Duration::from_millis(400))
    );
    let arrival_timeout_ms = 5_000;
    let request = ContainedLabOperationRequest {
        package_path: "/neutral/package.zip".to_owned(),
        selection: LabOperationSelection::Coordinates {
            action: InputAction::Tap { x: 0, y: 0 },
        },
        projection_hint: LabProjectionHint {
            sequence: None,
            content_sha256: None,
        },
        after: Some(LabArrivalCondition {
            page_id: "neutral/home".to_owned(),
            timeout_ms: arrival_timeout_ms,
        }),
    };
    let today = backend_open * 23
        + Duration::from_millis(MAX_INPUT_DURATION_MS)
        + Duration::from_millis(arrival_timeout_ms)
        + io_timeout;
    assert_eq!(
        lab_operation_response_timeout(backend_open, io_timeout, &request, frames),
        Some(today + (backend_open * 2 + Duration::from_millis(400)) * 21)
    );
}
