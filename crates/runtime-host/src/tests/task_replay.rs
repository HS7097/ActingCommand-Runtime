// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn inactive_incomplete_contained_task_replay_recovers_terminal_after_lease_expiry() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("recovered-task.zip");
    let bytes = neutral_contained_task_package();
    fs::write(&package, &bytes).expect("write package");
    let expected = actingcommand_pack_containment::Sha256Hash::digest(&bytes).to_string();
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let state = Arc::new(FakeState::default());
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock.clone())
            .with_scheduler(SchedulerConfig {
                maximum_client_heartbeat_interval_ms: 20,
                takeover_cooldown_ms: 40,
                lease_ttl_ms: 200,
                ..SchedulerConfig::default()
            }),
        Arc::new(FakeProvider::one("neutral.instance", instance_id(), state)),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    let (_, token) = client.acquire("neutral.instance");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let task_id = ids.mint_task_id().expect("task id");
    let run_id = ids.mint_run_id().expect("run id");
    let request = runtime_request(
        &ids,
        RuntimeOperation::RunContainedTask {
            instance_alias: "neutral.instance".to_owned(),
            holder_id: token.holder_id(),
            request: ContainedTaskRequest::new(package.display().to_string(), expected.clone())
                .expect("task request"),
        },
    );
    host.append_contained_task_semantic_for_test(
        &request,
        &token,
        task_id,
        run_id,
        TaskSemanticFact::PackageAdmitted {
            package_label: "neutral.semantic.task".to_owned(),
            task_label: "task".to_owned(),
            package_sha256: expected,
            response_deadline_monotonic_ms: None,
        },
    )
    .expect("append old package admission");
    clock.advance(250);
    host.expire_lease_once_for_test(&token)
        .expect("expire orphan lease");

    let recovered = host
        .process_request_for_test(&request, ConnectionId::new(173).expect("connection"))
        .expect("recover interrupted task");
    assert_eq!(recovered.state(), RuntimeReceiptState::Cancelled);
    assert!(matches!(
        recovered.result(),
        Some(RuntimeResult::ContainedTaskCancelled {
            reason: actingcommand_contract::ContainedTaskCancellationReason::RecoveredAfterRestart,
            lease_terminal: actingcommand_contract::ContainedTaskLeaseTerminal::Expired,
            ..
        })
    ));
    let events = host
        .query_persisted_events_for_test(EventQuery {
            lease_id: Some(token.lease_id()),
            ..EventQuery::default()
        })
        .expect("recovered events");
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type() == EventType::TaskCancelled)
            .count(),
        1
    );
    assert!(events.iter().any(|event| matches!(
        event.payload(),
        EventPayload::Task(TaskPayload::Semantic(payload))
            if matches!(payload.fact(), TaskSemanticFact::TerminalCommitted {
                outcome: TaskOutcome::Cancelled,
                executed_steps: None,
                ..
            })
    )));
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type() == EventType::LeaseExpired)
            .count(),
        1
    );
    drop(client);
    host.close().expect("close host");
}

#[test]
fn contained_task_replay_without_connection_cache_reuses_runtime_terminal() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("neutral-task.zip");
    let bytes = neutral_contained_task_package();
    fs::write(&package, &bytes).expect("write package");
    let expected = actingcommand_pack_containment::Sha256Hash::digest(&bytes).to_string();
    let state = Arc::new(FakeState::default());
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let host = host_with_state(&root, "neutral.instance", Arc::clone(&state));
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let request = runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            "neutral.instance",
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.display().to_string(), expected)
                .expect("task request"),
        ),
    );
    let connection = ConnectionId::new(91).expect("connection");

    let first = host
        .process_request_for_test(&request, connection)
        .expect("first contained task");
    let replayed = host
        .process_request_for_test(&request, connection)
        .expect("replayed contained task");

    assert_eq!(replayed, first);
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    let mut query_client = TestClient::connect(&host);
    let events = projected_events(
        &mut query_client,
        EventQuery {
            request_id: Some(request.request_id()),
            ..EventQuery::default()
        },
    );
    let facts = events.iter().filter_map(projected_task_semantic_fact);
    let mut packages = 0;
    let mut effect_intents = 0;
    let mut terminals = 0;
    for fact in facts {
        match fact {
            TaskSemanticFact::PackageAdmitted { .. } => packages += 1,
            TaskSemanticFact::EffectIntent { .. } => effect_intents += 1,
            TaskSemanticFact::TerminalCommitted { .. } => terminals += 1,
            _ => {}
        }
    }
    assert_eq!((packages, effect_intents, terminals), (1, 1, 1));
    drop(query_client);
    host.close().expect("close host");
}

#[test]
fn contained_task_terminal_is_absorbing_across_request_ids_and_rejections_are_audited() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "neutral.instance", state);
    let mut client = TestClient::connect(&host);
    let (_, token) = client.acquire("neutral.instance");
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let first_request = client.request_with_correlation(correlation, RuntimeOperation::Health);
    let second_request = client.request_with_correlation(correlation, RuntimeOperation::Health);
    assert_ne!(first_request.request_id(), second_request.request_id());
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let task_id = ids.mint_task_id().expect("task id");
    let run_id = ids.mint_run_id().expect("run id");

    let committed = host
        .append_contained_task_terminal_for_test(
            &first_request,
            &token,
            task_id,
            run_id,
            TaskOutcome::Success,
            false,
            Some("neutral/terminal".to_string()),
            1,
            None,
        )
        .expect("first terminal");
    let (rejected_state, rejected_code, rejected_terminal) = host
        .append_contained_task_terminal_for_test(
            &second_request,
            &token,
            task_id,
            run_id,
            TaskOutcome::Failure,
            true,
            None,
            1,
            Some("conflicting_terminal"),
        )
        .expect_err("second terminal must be rejected");

    assert_eq!(rejected_state, RuntimeReceiptState::Denied);
    assert_eq!(rejected_code, RuntimeErrorCode::InvalidRequest);
    assert_ne!(committed, rejected_terminal.expect("rejection terminal"));
    let events = projected_events(
        &mut client,
        EventQuery {
            task_id: Some(*task_id.transport()),
            run_id: Some(*run_id.transport()),
            ..EventQuery::default()
        },
    );
    let facts = events
        .iter()
        .filter_map(projected_task_semantic_fact)
        .collect::<Vec<_>>();
    assert_eq!(
        facts
            .iter()
            .filter(|fact| matches!(fact, TaskSemanticFact::TerminalCommitted { .. }))
            .count(),
        1
    );
    assert_eq!(
        facts
            .iter()
            .filter(|fact| matches!(fact, TaskSemanticFact::TerminalRejected { .. }))
            .count(),
        1
    );
    let release = client.request(RuntimeOperation::ReleaseLease { token });
    assert_eq!(
        client.send(&release).state(),
        RuntimeReceiptState::Completed
    );
    drop(client);
    host.close().expect("close host");
}
