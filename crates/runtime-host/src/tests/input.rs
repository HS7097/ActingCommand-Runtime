// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn typed_ipc_routes_input_once_and_correlates_ledger_events() {
    use actingcommand_device::{
        AdbRecoveryPath, AdbRecoveryPhase, AdbRecoveryStep, AdbRecoveryText, AdbTargetRecovery,
        AdbTransportState,
    };
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    // Workflow #284 ADB-TARGET-RECOVERY-v1: retain the open warning across the
    // existing real RuntimeClient/Kernel/Host path, including cached requests.
    *state.adb_recovery.lock().unwrap() = Some(AdbTargetRecovery {
        endpoint: AdbRecoveryText {
            text: "private-target:5555".into(),
            truncated: false,
        },
        initial_error: AdbRecoveryText {
            text: "original device offline".into(),
            truncated: false,
        },
        path: AdbRecoveryPath::TargetDisconnectConnect,
        budget_ms: 12000,
        steps: vec![AdbRecoveryStep {
            phase: AdbRecoveryPhase::Verify,
            attempt: 1,
            elapsed_ms: 1,
            command: None,
            error: None,
        }],
        final_state: AdbTransportState::Device,
        recovered: true,
        dropped_count: 0,
    });
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let health = client.request(RuntimeOperation::Health);
    let health = client.send_result(&health);
    assert!(
        health.is_ok(),
        "health failed: {health:?}; fatal={:?}",
        host.fatal_error()
    );
    let acquire_request = client.request(RuntimeOperation::acquire_lease(
        "node.a",
        client.ids.mint_holder_id().expect("holder id"),
    ));
    let acquire_receipt = client.send_result(&acquire_request);
    assert!(
        acquire_receipt.is_ok(),
        "acquire failed: {acquire_receipt:?}; fatal={:?}",
        host.fatal_error()
    );
    let acquire_receipt = acquire_receipt.expect("acquire receipt");
    assert_eq!(client.send(&acquire_request), acquire_receipt);
    assert_eq!(state.open_count.load(Ordering::Acquire), 0);
    let RuntimeResult::LeaseGranted { token } = acquire_receipt.result().expect("lease result")
    else {
        panic!("expected lease grant");
    };
    let token = token.clone();
    let renew_request = client.request(RuntimeOperation::RenewLease {
        token: token.clone(),
    });
    let renew_receipt = client.send(&renew_request);
    assert_eq!(client.send(&renew_request), renew_receipt);
    let RuntimeResult::LeaseRenewed { token } = renew_receipt.result().expect("renew result")
    else {
        panic!("expected renewed lease");
    };
    let token = token.clone();

    let actions = vec![
        InputAction::Tap { x: 10, y: 20 },
        InputAction::LongTap {
            x: 30,
            y: 40,
            duration_ms: 100,
        },
        InputAction::Swipe {
            x1: 10,
            y1: 20,
            x2: 30,
            y2: 40,
            duration_ms: 100,
        },
        InputAction::Key {
            key: "BACK".to_string(),
        },
        InputAction::Text {
            text: "highly-secret-input".to_string(),
        },
        InputAction::Reset,
    ];
    let mut text_request = None;
    for action in actions {
        let request = client.request(RuntimeOperation::Input {
            token: token.clone(),
            action: action.clone(),
        });
        let receipt = client.send(&request);
        assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
        if matches!(action, InputAction::Text { .. }) {
            text_request = Some((request, receipt));
        }
    }
    let (text_request, text_receipt) = text_request.expect("text request");
    assert_eq!(client.send(&text_request), text_receipt);
    assert_eq!(state.input_count.load(Ordering::Acquire), 6);
    let recovery_events = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("native recovery facts");
    let recovery_events = recovery_events.iter().filter(|event| {
        matches!(event.payload(), EventPayload::Runtime(actingcommand_contract::RuntimePayload::LifecycleObserved(value))
            if value.adb_recovery().is_some())
    }).collect::<Vec<_>>();
    assert_eq!(
        recovery_events.len(),
        1,
        "one open warning, no repetition on later input or replay"
    );
    let warning = recovery_events[0];
    assert_eq!(warning.severity(), EventSeverity::Warning);
    assert_eq!(warning.sensitivity(), Sensitivity::Sensitive);
    assert_eq!(
        warning.links().instance_id().copied(),
        Some(token.instance_id())
    );
    assert_eq!(warning.links().lease_id().copied(), Some(token.lease_id()));
    let wire = serde_json::to_string(warning.payload()).expect("stored recovery");
    assert!(wire.contains("original device offline"));
    assert!(wire.contains("target_disconnect_connect"));
    let public =
        serde_json::to_string(&warning.payload().public_projection()).expect("public warning");
    assert!(!public.contains("private-target"));
    assert!(!public.contains("original device offline"));

    let query = client.request(RuntimeOperation::QueryEvents {
        query: EventQuery {
            correlation_id: Some(acquire_request.correlation_id()),
            ..EventQuery::default()
        },
        profile: ProjectionProfile::Forensic,
        page: RuntimeEventQueryPageRequest::default(),
    });
    let receipt = client.send(&query);
    let RuntimeResult::EventPage { page } = receipt.result().expect("events result") else {
        panic!("expected event projection");
    };
    let event_types = page
        .events()
        .iter()
        .map(|event| event.event_type)
        .collect::<Vec<_>>();
    assert_eq!(
        event_types,
        vec![
            EventType::LeaseRequested,
            EventType::SchedulerAdmitted,
            EventType::LeaseTransitionIntent,
            EventType::LeaseGranted,
        ]
    );

    let query = client.request(RuntimeOperation::QueryEvents {
        query: EventQuery {
            correlation_id: Some(text_request.correlation_id()),
            ..EventQuery::default()
        },
        profile: ProjectionProfile::Forensic,
        page: RuntimeEventQueryPageRequest::default(),
    });
    let receipt = client.send(&query);
    let RuntimeResult::EventPage { page } = receipt.result().expect("events result") else {
        panic!("expected input event projection");
    };
    assert_eq!(
        page.events()
            .iter()
            .map(|event| event.event_type)
            .collect::<Vec<_>>(),
        vec![
            EventType::SchedulerAdmitted,
            EventType::InputIntent,
            EventType::InputCommitted,
        ]
    );

    let all_events = client.request(RuntimeOperation::QueryEvents {
        query: EventQuery::default(),
        profile: ProjectionProfile::Forensic,
        page: RuntimeEventQueryPageRequest::default(),
    });
    let receipt = client.send(&all_events);
    let encoded = serde_json::to_string(receipt.result().expect("events")).expect("encode events");
    assert!(!encoded.contains("highly-secret-input"));
    assert!(!encoded.contains("127.0.0.1:16384"));

    let release = client.request(RuntimeOperation::ReleaseLease {
        token: token.clone(),
    });
    let receipt = client.send(&release);
    assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
    assert_eq!(client.send(&release), receipt);
    assert_eq!(state.close_count.load(Ordering::Acquire), 1);
    drop(client);
    host.close().expect("close host");
    assert_eq!(state.close_count.load(Ordering::Acquire), 1);
}

#[test]
fn segmented_swipe_intent_records_the_exact_prepared_plan_before_input() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.block_input.store(true, Ordering::Release);
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let (_, token) = client.acquire("node.a");
    let request = client.request(RuntimeOperation::Input {
        token,
        action: InputAction::SingleTouchDragWithVerticalBrakeV1 {
            x1: 1091,
            y1: 355,
            x2: 119,
            y2: 363,
            x3: 119,
            y3: 263,
            horizontal_duration_ms: 200,
            corner_hold_ms: 150,
            brake_distance_px: 100,
            brake_duration_ms: 200,
            slope_in: 2,
            slope_out: 0,
        },
    });
    let request_id = request.request_id();
    let input_thread = thread::spawn(move || {
        let receipt = client.send(&request);
        (client, receipt)
    });
    wait_until(Duration::from_secs(2), || {
        state.input_started.load(Ordering::Acquire)
    });

    let intent_events = host
        .query_persisted_events_for_test(EventQuery {
            request_id: Some(request_id),
            event_type: Some(EventType::InputIntent),
            ..EventQuery::default()
        })
        .expect("query durable input intent while backend is blocked");
    assert_eq!(intent_events.len(), 1);
    assert!(
        host.query_persisted_events_for_test(EventQuery {
            request_id: Some(request_id),
            event_type: Some(EventType::InputCommitted),
            ..EventQuery::default()
        })
        .expect("query input outcome while backend is blocked")
        .is_empty()
    );
    let EventPayload::Input(InputPayload::Intent(intent)) = intent_events[0].payload() else {
        panic!("typed input intent")
    };
    let recorded_plan = intent
        .execution_plan()
        .expect("durable prepared plan")
        .clone();
    assert_eq!(recorded_plan.version(), INPUT_EXECUTION_PLAN_VERSION);
    assert_eq!(
        recorded_plan.profile(),
        INPUT_EXECUTION_PLAN_PROFILE_MAA_2_0
    );

    let consumed_plan = state
        .segmented_swipe_plans
        .lock()
        .expect("fake segmented swipe plans lock")
        .first()
        .cloned()
        .expect("backend consumed prepared plan");
    assert_eq!(recorded_plan.events().len(), consumed_plan.events().len());
    for (recorded, consumed) in recorded_plan.events().iter().zip(consumed_plan.events()) {
        match (recorded, consumed) {
            (
                InputExecutionPlanEvent::Down {
                    x: recorded_x,
                    y: recorded_y,
                },
                SegmentedSwipeEvent::Down((consumed_x, consumed_y)),
            ) => assert_eq!((*recorded_x, *recorded_y), (*consumed_x, *consumed_y)),
            (
                InputExecutionPlanEvent::Move {
                    x: recorded_x,
                    y: recorded_y,
                    delay_before_ms: recorded_delay,
                },
                SegmentedSwipeEvent::Move {
                    point: (consumed_x, consumed_y),
                    delay_before_ms: consumed_delay,
                },
            ) => assert_eq!(
                (*recorded_x, *recorded_y, *recorded_delay),
                (*consumed_x, *consumed_y, *consumed_delay)
            ),
            (
                InputExecutionPlanEvent::Hold {
                    duration_ms: recorded_duration,
                },
                SegmentedSwipeEvent::Hold(consumed_duration),
            ) => assert_eq!(*recorded_duration, *consumed_duration),
            (InputExecutionPlanEvent::Up, SegmentedSwipeEvent::Up) => {}
            _ => panic!("ledger and backend prepared plans differ"),
        }
    }

    state.block_input.store(false, Ordering::Release);
    let (client, receipt) = input_thread.join().expect("input thread");
    assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    let events = host
        .query_persisted_events_for_test(EventQuery {
            request_id: Some(request_id),
            ..EventQuery::default()
        })
        .expect("query complete input sequence");
    assert_eq!(
        events
            .iter()
            .map(PersistedEvent::event_type)
            .collect::<Vec<_>>(),
        vec![
            EventType::SchedulerAdmitted,
            EventType::InputIntent,
            EventType::InputCommitted,
        ]
    );
    drop(client);
    host.close().expect("close host");
}
