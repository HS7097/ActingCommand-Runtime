// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #308 SEL-L3b, register §1.58: the drag with a pause and a brake through the host and
// its input provider. An input request that names it is performed as one prepared plan by the
// backend that performs segmented swipes: the press, the drag, the hold of the pause, the brake
// and the release. The input intent keeps the action whole in its provenance and records no
// execution plan, whose `maa_2_0` record is the vertical-brake drag's. A request whose drag has
// neither a pause nor a brake breaks the action's contract and is refused before any input.

use super::*;
use actingcommand_contract::codes::RuntimeContractCode;
use actingcommand_device::{DragWithPauseAndBrakeAction, SegmentedDragAction};

#[test]
fn l3b_the_host_performs_a_drag_with_a_pause_and_a_brake_as_one_prepared_plan() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let (_, token) = client.acquire("node.a");
    let drag = InputAction::SingleTouchDragWithPauseAndBrakeV1 {
        x1: 1000,
        y1: 360,
        x2: 200,
        y2: 360,
        drag_duration_ms: 400,
        pause_ms: 150,
        x3: 200,
        y3: 260,
        brake_duration_ms: 200,
    };
    let request = client.request(RuntimeOperation::Input {
        frame: None,
        token: token.clone(),
        action: drag.clone(),
    });
    let request_id = request.request_id();
    let receipt = client.send(&request);
    assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);

    let plans = state
        .segmented_swipe_plans
        .lock()
        .expect("fake segmented swipe plans lock")
        .clone();
    let [plan] = plans.as_slice() else {
        panic!("one prepared plan, got {plans:?}");
    };
    assert_eq!(
        plan.action(),
        SegmentedDragAction::PauseAndBrake(DragWithPauseAndBrakeAction {
            points: [(1000, 360), (200, 360), (200, 260)],
            drag_duration_ms: 400,
            pause_ms: 150,
            brake_duration_ms: 200,
        })
    );
    let events = plan.events();
    assert_eq!(
        events.first(),
        Some(&SegmentedSwipeEvent::Down((1000, 360)))
    );
    assert_eq!(events.last(), Some(&SegmentedSwipeEvent::Up));
    let hold = events
        .iter()
        .position(|event| *event == SegmentedSwipeEvent::Hold(150))
        .expect("the pause");
    assert!(
        matches!(
            events[hold - 1],
            SegmentedSwipeEvent::Move {
                point: (200, 360),
                ..
            }
        ),
        "the pause holds at the drag's end: {events:?}"
    );
    assert!(
        matches!(
            events[events.len() - 2],
            SegmentedSwipeEvent::Move {
                point: (200, 260),
                ..
            }
        ),
        "the brake ends at its point: {events:?}"
    );

    let intents = host
        .query_persisted_events_for_test(EventQuery {
            request_id: Some(request_id),
            event_type: Some(EventType::InputIntent),
            ..EventQuery::default()
        })
        .expect("query the input intent");
    let [intent] = intents.as_slice() else {
        panic!("one input intent, got {}", intents.len());
    };
    let EventPayload::Input(InputPayload::Intent(intent)) = intent.payload() else {
        panic!("typed input intent");
    };
    assert_eq!(
        intent
            .provenance()
            .map(|provenance| &provenance.input_action),
        Some(&drag)
    );
    assert!(intent.execution_plan().is_none());

    // A drag with neither a pause nor a brake is a plain swipe, not this action: the wire shape
    // decodes, and the request's contract check refuses it before any input.
    let valid = client.request(RuntimeOperation::Input {
        frame: None,
        token,
        action: drag,
    });
    let mut wire = serde_json::to_value(&valid).expect("request wire shape");
    wire["operation"]["action"]["pause_ms"] = serde_json::json!(0);
    wire["operation"]["action"]["y3"] = serde_json::json!(360);
    wire["operation"]["action"]["brake_duration_ms"] = serde_json::json!(0);
    let refused: RuntimeRequest = serde_json::from_value(wire).expect("request wire shape");
    let denied = client.send(&refused);
    assert_eq!(denied.state(), RuntimeReceiptState::Denied);
    assert_eq!(
        denied.outcome().code().as_str(),
        RuntimeContractCode::InvalidSegmentedSwipeContract.as_str()
    );
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    assert_eq!(
        state
            .segmented_swipe_plans
            .lock()
            .expect("fake segmented swipe plans lock")
            .len(),
        1
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(client);
    host.close().expect("close runtime");
}
