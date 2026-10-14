// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #383 SH-1, register §1.18 (#406, approved 2026-10-14): this build decodes the
// single-touch path drag but performs it only from #406 T-3 on. The host refuses an input
// request that names it, with `input_action_unsupported`, before any effect: nothing reaches the
// input backend and the Runtime goes on.

use super::*;
use crate::codes::ShapeHostCode;
use actingcommand_contract::InputPathPoint;
use actingcommand_contract::outcome::Category;
use serde_json::json;

#[test]
fn h1_the_host_refuses_the_reserved_path_drag_before_any_effect() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let acquire = client.request(RuntimeOperation::acquire_lease(
        "node.a",
        client.ids.mint_holder_id().expect("holder id"),
    ));
    let granted = client.send(&acquire);
    let Some(RuntimeResult::LeaseGranted { token }) = granted.result() else {
        panic!("expected a lease grant");
    };
    let token = token.clone();

    let drag = InputAction::SingleTouchPathDragV1 {
        box_x: 100,
        box_y: 200,
        box_width: 400,
        box_height: 300,
        points: vec![
            InputPathPoint { x: 120, y: 220 },
            InputPathPoint { x: 480, y: 260 },
            InputPathPoint { x: 150, y: 480 },
        ],
        segment_duration_ms: vec![200, 250],
        corner_hold_ms: 80,
    };
    drag.validate().expect("the request names a valid action");
    let input = client.request(RuntimeOperation::Input {
        frame: None,
        token: token.clone(),
        action: drag,
    });
    let denied = client.send(&input);
    assert_eq!(denied.state(), RuntimeReceiptState::Denied);
    assert_eq!(
        denied.outcome().code().as_str(),
        ShapeHostCode::InputActionUnsupported.as_str()
    );
    assert_eq!(denied.outcome().category(), Category::Error);
    assert_eq!(
        serde_json::Value::Object(denied.outcome().values().clone()),
        json!({
            "operation": "execute_input",
            "runtime_code": "invalid_request",
            "host_halted": false
        })
    );
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    assert!(host.fatal_error().expect("runtime health").is_none());

    // The Runtime goes on: the same lease still performs a supported action.
    let tap = client.request(RuntimeOperation::Input {
        frame: None,
        token,
        action: InputAction::Tap { x: 10, y: 20 },
    });
    assert_eq!(client.send(&tap).state(), RuntimeReceiptState::Completed);
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    drop(client);
    host.close().expect("close runtime");
}
