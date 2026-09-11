// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn one_correlation_queries_the_complete_lease_input_release_sequence() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", state);
    let mut client = TestClient::connect(&host);
    let correlation_id = client.ids.mint_correlation_id().expect("correlation id");
    let correlation_transport = *correlation_id.transport();
    let acquire = client.request_with_correlation(
        correlation_id,
        RuntimeOperation::acquire_lease("node.a", client.ids.mint_holder_id().expect("holder id")),
    );
    let acquire_id = acquire.request_id();
    let receipt = client.send(&acquire);
    let RuntimeResult::LeaseGranted { token } = receipt.result().expect("lease result") else {
        panic!("expected lease grant");
    };
    let token = token.clone();

    let input = client.request_with_correlation(
        correlation_id,
        RuntimeOperation::Input {
            token: token.clone(),
            action: InputAction::Tap { x: 10, y: 20 },
        },
    );
    let input_id = input.request_id();
    assert_eq!(client.send(&input).state(), RuntimeReceiptState::Completed);

    let release =
        client.request_with_correlation(correlation_id, RuntimeOperation::ReleaseLease { token });
    let release_id = release.request_id();
    assert_eq!(
        client.send(&release).state(),
        RuntimeReceiptState::Completed
    );

    let query = client.request(RuntimeOperation::QueryEvents {
        query: EventQuery {
            correlation_id: Some(correlation_transport),
            ..EventQuery::default()
        },
        profile: ProjectionProfile::Forensic,
        page: RuntimeEventQueryPageRequest::default(),
    });
    let receipt = client.send(&query);
    let RuntimeResult::EventPage { page } = receipt.result().expect("events result") else {
        panic!("expected event projection");
    };
    let events = page.events();
    assert_eq!(
        events
            .iter()
            .map(|event| event.event_type)
            .collect::<Vec<_>>(),
        vec![
            EventType::LeaseRequested,
            EventType::SchedulerAdmitted,
            EventType::LeaseTransitionIntent,
            EventType::LeaseGranted,
            EventType::SchedulerAdmitted,
            EventType::InputIntent,
            EventType::InputCommitted,
            EventType::SchedulerAdmitted,
            EventType::LeaseTransitionIntent,
            EventType::LeaseReleased,
        ]
    );
    assert!(
        events[..4]
            .iter()
            .all(|event| event.links.request_id() == Some(&acquire_id))
    );
    assert!(
        events[4..7]
            .iter()
            .all(|event| event.links.request_id() == Some(&input_id))
    );
    assert!(
        events[7..]
            .iter()
            .all(|event| event.links.request_id() == Some(&release_id))
    );
    assert!(
        events
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence)
    );
    drop(client);
    host.close().expect("close host");
}

#[test]
fn runtime_is_the_single_writer_for_one_correlated_resource_authoring_sequence() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", state);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let correlation = ids.mint_correlation_id().expect("correlation id");
    let correlation_transport = *correlation.transport();
    let connection = ConnectionId::new(121).expect("connection");
    let phases = [
        (ResourceAuthoringPhase::AuthoringStarted, None),
        (ResourceAuthoringPhase::DraftBuilt, None),
        (ResourceAuthoringPhase::ValidationCompleted, None),
        (ResourceAuthoringPhase::PromoteIntent, None),
        (ResourceAuthoringPhase::Promoted, None),
    ];

    for (phase, failure_code) in phases {
        let request = RuntimeRequest::new(
            ids.mint_request_id().expect("request id"),
            correlation,
            None,
            EventActor::Lab,
            EventSource::Lab,
            unix_ms_now().expect("wall clock"),
            RuntimeOperation::RecordAuthoringEvent {
                event: ResourceAuthoringEvent::new(
                    phase,
                    "draft-a",
                    "resource-root",
                    "b".repeat(64),
                    vec!["operations/task-a/task.json".to_string()],
                    failure_code,
                )
                .expect("authoring event"),
            },
        )
        .expect("authoring request");
        let receipt = host
            .process_request_for_test(&request, connection)
            .expect("authoring receipt");
        assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
        assert!(receipt.terminal().is_some());
        assert!(matches!(
            receipt.result(),
            Some(RuntimeResult::AuthoringEventRecorded { phase: recorded }) if *recorded == phase
        ));
    }

    let query = runtime_request(
        &ids,
        RuntimeOperation::QueryEvents {
            query: EventQuery {
                correlation_id: Some(correlation_transport),
                ..EventQuery::default()
            },
            profile: ProjectionProfile::Forensic,
            page: RuntimeEventQueryPageRequest::default(),
        },
    );
    let receipt = host
        .process_request_for_test(&query, connection)
        .expect("event query");
    let RuntimeResult::EventPage { page } = receipt.result().expect("events result") else {
        panic!("expected events");
    };
    let events = page.events();
    assert_eq!(
        events
            .iter()
            .map(|event| event.event_type)
            .collect::<Vec<_>>(),
        vec![
            EventType::ResourceAuthoringStarted,
            EventType::ResourceDraftBuilt,
            EventType::ResourceValidationCompleted,
            EventType::ResourcePromoteIntent,
            EventType::ResourcePromoted,
        ]
    );
    for event in events {
        assert_eq!(event.origin.source(), EventSource::Lab);
        assert_eq!(event.origin.module(), OriginModule::ResourceTooling);
        assert_eq!(event.origin.actor(), EventActor::Lab);
        assert!(matches!(
            &event.payload,
            ProjectionPayload::Full(payload)
                if matches!(payload.as_ref(), EventPayload::ResourceAuthoring(_))
        ));
    }
    host.close().expect("close host");
}

#[test]
fn runtime_rejects_forged_non_lab_resource_authoring_ingress_without_ledger_effect() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", state);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let correlation = ids.mint_correlation_id().expect("correlation id");
    let correlation_transport = *correlation.transport();
    let valid = RuntimeRequest::new(
        ids.mint_request_id().expect("request id"),
        correlation,
        None,
        EventActor::Lab,
        EventSource::Lab,
        unix_ms_now().expect("wall clock"),
        RuntimeOperation::RecordAuthoringEvent {
            event: ResourceAuthoringEvent::new(
                ResourceAuthoringPhase::AuthoringStarted,
                "draft-a",
                "resource-root",
                "b".repeat(64),
                vec!["operations/task-a/task.json".to_string()],
                None,
            )
            .expect("authoring event"),
        },
    )
    .expect("Lab request");
    let mut forged = serde_json::to_value(valid).expect("request JSON");
    forged["actor"] = serde_json::json!("cli");
    forged["source"] = serde_json::json!("cli");
    let forged: RuntimeRequest = serde_json::from_value(forged).expect("wire request");
    let connection = ConnectionId::new(122).expect("connection");
    let denied = host
        .process_request_for_test(&forged, connection)
        .expect("denied receipt");
    assert_eq!(denied.state(), RuntimeReceiptState::Denied);

    let query = runtime_request(
        &ids,
        RuntimeOperation::QueryEvents {
            query: EventQuery {
                correlation_id: Some(correlation_transport),
                ..EventQuery::default()
            },
            profile: ProjectionProfile::Forensic,
            page: RuntimeEventQueryPageRequest::default(),
        },
    );
    let receipt = host
        .process_request_for_test(&query, connection)
        .expect("event query");
    let RuntimeResult::EventPage { page } = receipt.result().expect("events result") else {
        panic!("expected events");
    };
    assert!(page.events().is_empty());
    host.close().expect("close host");
}

#[test]
fn typed_client_action_is_idempotent_and_public_projection_hides_the_value() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, "node.alpha", Arc::new(FakeState::default()));
    let mut client = TestClient::connect(&host);
    let secret_hash = format!("sha256:{}", "e".repeat(64));
    let request = client.request(RuntimeOperation::RecordClientAction {
        action: ClientActionRecord::new(
            "settings",
            "account_token",
            ClientActionKind::Input,
            Some("node.alpha".to_owned()),
            Some(ClientActionValue::Redacted {
                sha256: secret_hash.clone(),
                byte_count: 24,
            }),
        )
        .expect("client action"),
    });
    let first = client.send(&request);
    let replay = client.send(&request);
    assert_eq!(first, replay);
    assert!(matches!(
        first.result(),
        Some(RuntimeResult::ClientActionRecorded)
    ));

    let query = client.request(RuntimeOperation::QueryEvents {
        query: EventQuery {
            event_type: Some(EventType::ClientAction),
            ..EventQuery::default()
        },
        profile: ProjectionProfile::Ui,
        page: RuntimeEventQueryPageRequest::default(),
    });
    let receipt = client.send(&query);
    let RuntimeResult::EventPage { page } = receipt.result().expect("events") else {
        panic!("expected events")
    };
    let events = page.events();
    assert_eq!(events.len(), 1);
    let ProjectionPayload::Public(payload) = &events[0].payload else {
        panic!("expected public projection")
    };
    let PublicEventPayload::Client(payload) = payload.as_ref() else {
        panic!("expected client projection")
    };
    assert_eq!(payload.client_surface_id(), Some("settings"));
    assert_eq!(payload.client_control_id(), Some("account_token"));
    assert!(
        !serde_json::to_string(&events)
            .expect("events JSON")
            .contains(&secret_hash)
    );
    drop(client);
    host.close().expect("close host");
}

#[test]
fn client_fact_request_id_cannot_cross_typed_operation_boundaries() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, "fixture-instance-a", Arc::new(FakeState::default()));
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let request_id = ids.mint_request_id().expect("request id");
    let action = RuntimeRequest::new(
        request_id,
        ids.mint_correlation_id().expect("action correlation"),
        None,
        EventActor::Cli,
        EventSource::Cli,
        unix_ms_now().expect("wall clock"),
        RuntimeOperation::RecordClientAction {
            action: ClientActionRecord::new(
                "settings",
                "refresh",
                ClientActionKind::Button,
                None,
                None,
            )
            .expect("client action"),
        },
    )
    .expect("action request");
    let action_other_correlation = RuntimeRequest::new(
        request_id,
        ids.mint_correlation_id().expect("other correlation"),
        None,
        EventActor::Cli,
        EventSource::Cli,
        unix_ms_now().expect("wall clock"),
        action.operation().clone(),
    )
    .expect("action request with another correlation");
    let approval = RuntimeRequest::new(
        request_id,
        ids.mint_correlation_id().expect("approval correlation"),
        None,
        EventActor::User,
        EventSource::Ui,
        unix_ms_now().expect("wall clock"),
        RuntimeOperation::RecordApprovalDecision {
            decision: ApprovalDecisionRecord::new(
                "approval:request-boundary",
                ApprovalDisposition::Approved,
                ApprovalTarget::Catalog {
                    catalog_hash: format!("sha256:{}", "a".repeat(64)),
                    catalog_version: 1,
                },
                "user_confirmed",
            )
            .expect("approval decision"),
        },
    )
    .expect("approval request");
    let connection = ConnectionId::new(99).expect("connection id");
    let authentication = RuntimeRequest::new(
        ids.mint_request_id().expect("authentication request"),
        ids.mint_correlation_id()
            .expect("authentication correlation"),
        None,
        EventActor::User,
        EventSource::Ui,
        unix_ms_now().expect("wall clock"),
        RuntimeOperation::AuthenticateGovernance {
            capability: TEST_GOVERNANCE_CAPABILITY.to_owned(),
        },
    )
    .expect("authentication request");
    assert_eq!(
        host.process_request_for_test(&authentication, connection)
            .expect("authentication receipt")
            .state(),
        RuntimeReceiptState::Completed
    );

    assert_eq!(
        host.process_request_for_test(&action, connection)
            .expect("action receipt")
            .state(),
        RuntimeReceiptState::Completed
    );
    let correlation_collision = host
        .process_request_for_test(&action_other_correlation, connection)
        .expect("correlation collision receipt");
    assert_eq!(correlation_collision.state(), RuntimeReceiptState::Denied);
    let collision = host
        .process_request_for_test(&approval, connection)
        .expect("collision receipt");
    assert_eq!(collision.state(), RuntimeReceiptState::Denied);
    assert_eq!(
        collision.error_projection().expect("collision error").code,
        RuntimeErrorCode::InvalidRequest
    );
    assert_eq!(
        event_types_for_request(&host, &ids, connection, action.request_id()),
        vec![EventType::ClientAction]
    );
    host.close().expect("close host");
}

#[test]
fn concurrent_approval_targets_commit_exactly_one_authoritative_fact() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, "fixture-instance-a", Arc::new(FakeState::default()));
    let first = TestClient::connect(&host);
    let second = TestClient::connect(&host);
    let start = Arc::new(Barrier::new(3));
    let run = |mut client: TestClient, marker: char, start: Arc<Barrier>| {
        thread::spawn(move || {
            client.authenticate_governance();
            let request = client.governance_request(RuntimeOperation::RecordApprovalDecision {
                decision: ApprovalDecisionRecord::new(
                    "approval:concurrent-target",
                    ApprovalDisposition::Approved,
                    ApprovalTarget::Catalog {
                        catalog_hash: format!("sha256:{}", marker.to_string().repeat(64)),
                        catalog_version: 1,
                    },
                    "user_confirmed",
                )
                .expect("approval decision"),
            });
            start.wait();
            client.send(&request)
        })
    };
    let first = run(first, 'a', Arc::clone(&start));
    let second = run(second, 'b', Arc::clone(&start));
    start.wait();
    let receipts = [
        first.join().expect("first approval writer"),
        second.join().expect("second approval writer"),
    ];
    assert_eq!(
        receipts
            .iter()
            .filter(|receipt| receipt.state() == RuntimeReceiptState::Completed)
            .count(),
        1
    );
    assert_eq!(
        receipts
            .iter()
            .filter(|receipt| receipt.state() == RuntimeReceiptState::Denied)
            .count(),
        1
    );
    assert!(receipts.iter().any(|receipt| {
        receipt
            .error_projection()
            .is_some_and(|error| error.code == RuntimeErrorCode::InvalidRequest)
    }));

    let mut client = TestClient::connect(&host);
    let events = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::ApprovalDecision),
            ..EventQuery::default()
        },
    );
    assert_eq!(events.len(), 1);
    drop(client);
    host.close().expect("close host");
}

#[test]
fn information_planning_signals_are_queryable_and_subscription_pages_are_lossless() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    for index in 0..5_u64 {
        host.record_policy_planning_signal(PolicyPlanningSignalEventData {
            signal_id: format!("signal:projection-{index}"),
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
            task_id: None,
            kind: PolicyPlanningSignalKind::GoalMissed,
            fact_code: format!("goal.fixture.projection-{index}"),
            observed_at_unix_ms: POLICY_NOW_UNIX_MS + index,
            detection_budget: None,
        })
        .expect("record planning signal");
    }

    let query = EventQuery {
        event_type: Some(EventType::PolicyPlanningSignalObserved),
        minimum_severity: Some(EventSeverity::Info),
        ..EventQuery::default()
    };
    let mut client = TestClient::connect(&host);
    let expected = projected_events(&mut client, query.clone())
        .into_iter()
        .map(|event| event.sequence)
        .collect::<Vec<_>>();
    assert_eq!(expected.len(), 5);

    let mut cursor = actingcommand_contract::SubscriptionCursor::default();
    let mut observed = Vec::new();
    for _ in 0..8 {
        let subscription = actingcommand_contract::RuntimeSubscriptionRequest::new(
            query.clone(),
            ProjectionProfile::Forensic,
            cursor,
            100,
            2,
        )
        .expect("subscription request");
        let request = client.request(RuntimeOperation::SubscribeEvents {
            request: subscription,
        });
        let receipt = client.send(&request);
        let RuntimeResult::EventBatch { batch } = receipt.result().expect("event batch") else {
            panic!("expected event batch")
        };
        assert!(!batch.timed_out(), "planning signal page must not be empty");
        assert!(batch.events().len() <= 2);
        assert!(batch.events().iter().all(|event| {
            event.event_type == EventType::PolicyPlanningSignalObserved
                && event.severity == EventSeverity::Info
        }));
        observed.extend(batch.events().iter().map(|event| event.sequence));
        cursor = batch.next_cursor();
        if observed.len() == expected.len() {
            break;
        }
    }

    assert_eq!(observed, expected);
    drop(client);
    host.close().expect("close host");
}

#[test]
fn event_pages_freeze_the_snapshot_and_planning_recovery_uses_a_compact_checkpoint() {
    const SIGNAL_COUNT: u64 = 300;

    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    let first_signal = PolicyPlanningSignalEventData {
        signal_id: "signal:bounded-history-0".to_owned(),
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        task_id: None,
        kind: PolicyPlanningSignalKind::GoalMissed,
        fact_code: "goal.bounded-history.0".to_owned(),
        observed_at_unix_ms: POLICY_NOW_UNIX_MS,
        detection_budget: None,
    };
    host.record_policy_planning_signal(first_signal.clone())
        .expect("record first signal");
    for index in 1..SIGNAL_COUNT {
        host.record_policy_planning_signal(PolicyPlanningSignalEventData {
            signal_id: format!("signal:bounded-history-{index}"),
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
            task_id: None,
            kind: PolicyPlanningSignalKind::GoalMissed,
            fact_code: format!("goal.bounded-history.{index}"),
            observed_at_unix_ms: POLICY_NOW_UNIX_MS + index,
            detection_budget: None,
        })
        .expect("record planning signal");
    }

    let query = EventQuery {
        event_type: Some(EventType::PolicyPlanningSignalObserved),
        ..EventQuery::default()
    };
    let mut client = TestClient::connect(&host);
    let first_request = client.request(RuntimeOperation::QueryEvents {
        query: query.clone(),
        profile: ProjectionProfile::Forensic,
        page: RuntimeEventQueryPageRequest::new(31, None).expect("first page request"),
    });
    let first_receipt = client.send(&first_request);
    let RuntimeResult::EventPage { page } = first_receipt.result().expect("first page") else {
        panic!("expected event page")
    };
    let snapshot = page.snapshot_ledger_position();
    let first_cursor = page.next_cursor().cloned().expect("continuation cursor");
    assert_eq!(
        page.read_scope().unwrap().source,
        actingcommand_contract::LedgerReadSource::Runtime
    );
    assert!(page.read_scope().unwrap().read_complete);
    assert_eq!(
        page.read_scope().unwrap().material_read,
        actingcommand_contract::LedgerMaterialReadState::NotRequested
    );
    assert!(page.events().iter().all(|event| {
        event
            .views
            .contains(&actingcommand_contract::LedgerView::Events)
    }));
    let mut sequences = page
        .events()
        .iter()
        .map(|event| event.sequence)
        .collect::<Vec<_>>();

    let late_signal = PolicyPlanningSignalEventData {
        signal_id: "signal:bounded-history-late".to_owned(),
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        task_id: None,
        kind: PolicyPlanningSignalKind::GoalMissed,
        fact_code: "goal.bounded-history.late".to_owned(),
        observed_at_unix_ms: POLICY_NOW_UNIX_MS + SIGNAL_COUNT,
        detection_budget: None,
    };
    host.record_policy_planning_signal(late_signal)
        .expect("record interleaved signal");

    let historical = client.request(RuntimeOperation::QueryEvents {
        query: EventQuery {
            view: Some(actingcommand_contract::LedgerView::Events),
            ..query.clone()
        },
        profile: ProjectionProfile::Forensic,
        page: RuntimeEventQueryPageRequest::new(31, None)
            .unwrap()
            .at_snapshot(snapshot)
            .unwrap(),
    });
    let historical = client.send(&historical);
    let RuntimeResult::EventPage { page: historical } =
        historical.result().expect("historical first page")
    else {
        panic!("expected historical event page")
    };
    assert_eq!(historical.snapshot_ledger_position(), snapshot);
    assert_eq!(
        historical
            .events()
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        sequences
    );

    let mismatched = client.request(RuntimeOperation::QueryEvents {
        query: EventQuery::default(),
        profile: ProjectionProfile::Forensic,
        page: RuntimeEventQueryPageRequest::new(31, Some(first_cursor.clone()))
            .expect("mismatched page request"),
    });
    let denied = client.send(&mismatched);
    assert_eq!(denied.state(), RuntimeReceiptState::Denied);
    assert_eq!(
        denied.error_projection().expect("typed cursor denial").code,
        RuntimeErrorCode::ProtocolInvalid
    );

    let mut cursor = Some(first_cursor);
    while let Some(current) = cursor {
        let request = client.request(RuntimeOperation::QueryEvents {
            query: query.clone(),
            profile: ProjectionProfile::Forensic,
            page: RuntimeEventQueryPageRequest::new(31, Some(current))
                .expect("continuation request"),
        });
        let receipt = client.send(&request);
        let RuntimeResult::EventPage { page } = receipt.result().expect("continuation page") else {
            panic!("expected event page")
        };
        assert_eq!(page.snapshot_ledger_position(), snapshot);
        assert!(
            serde_json::to_vec(page).expect("page encoding").len()
                <= actingcommand_contract::MAX_RUNTIME_EVENT_QUERY_RESPONSE_BYTES
        );
        sequences.extend(page.events().iter().map(|event| event.sequence));
        cursor = page.next_cursor().cloned();
    }
    assert_eq!(sequences.len(), SIGNAL_COUNT as usize);
    assert!(sequences.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(sequences.iter().all(|sequence| *sequence <= snapshot));

    let all_signals = projected_events(&mut client, query.clone());
    assert_eq!(all_signals.len(), SIGNAL_COUNT as usize + 1);
    let latest_signal_sequence = all_signals.last().expect("latest planning signal").sequence;
    drop(client);
    host.close().expect("close host");

    let state = RuntimeStateStore::open(root.path(), b"runtime-host-test-salt")
        .expect("open compact state");
    let checkpoint = state
        .read_projection_entry("policy.planning-signal.v1", "checkpoint")
        .expect("read checkpoint")
        .expect("planning checkpoint");
    let checkpoint_payload: serde_json::Value =
        serde_json::from_slice(checkpoint.payload()).expect("checkpoint payload");
    assert!(
        checkpoint_payload["through_sequence"]
            .as_u64()
            .expect("checkpoint sequence")
            >= latest_signal_sequence
    );
    drop(state);

    let database =
        actingcommand_runtime_database::RuntimeDatabase::open_existing(root.path(), false)
            .expect("shared database");
    database
        .connection("remove reconstructible projection in existing specification")
        .expect("connection")
        .execute(
            "DELETE FROM projection_entries WHERE namespace='policy.planning-signal.v1'",
            [],
        )
        .expect("remove compact projection");
    drop(database);

    let reopened = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    reopened
        .record_policy_planning_signal(first_signal)
        .expect("replay compacted signal identity");
    let mut client = TestClient::connect(&reopened);
    assert_eq!(
        projected_events(&mut client, query).len(),
        SIGNAL_COUNT as usize + 1
    );
    drop(client);
    reopened.close().expect("close reopened host");
    fs::remove_file(root.path().join(RUNTIME_STATE_DATABASE_FILE))
        .expect("missing physical database case");
    let error = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .err()
    .expect("missing authoritative database must fail");
    assert!(error.is_fatal());
}
