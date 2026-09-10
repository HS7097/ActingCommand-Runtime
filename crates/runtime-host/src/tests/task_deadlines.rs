// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

// Task Contract: Workflow #241 / direct contained-task lease deadline v1.
// Test class: specification criterion.
#[test]
fn direct_contained_task_max_deadline_survives_past_default_lease_boundary() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("long-contained-task.zip");
    let bytes = neutral_contained_task_package_with_execution_timeout(
        ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS,
    );
    fs::write(&package, &bytes).expect("write package");
    let expected = actingcommand_pack_containment::Sha256Hash::digest(&bytes).to_string();
    let state = Arc::new(FakeState::default());
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let stable_instance_id = instance_id();
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock.clone()),
        Arc::new(FakeProvider::one(
            "neutral.instance",
            stable_instance_id,
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let request = runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            "neutral.instance",
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.display().to_string(), expected)
                .expect("contained task request")
                .with_response_deadline_ms(ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS)
                .expect("bounded response deadline"),
        ),
    );
    let advance_past_default_lease_boundary =
        DEFAULT_LEASE_TTL_MS - DEFAULT_MAX_CLIENT_HEARTBEAT_INTERVAL_MS + 1;
    let checkpoint_clock = Arc::clone(&clock);
    let sample_capacity = host.capacity_sampler_for_test().expect("capacity owner");
    let checkpoint = host
        .run_at_contained_task_checkpoint_for_test(
            request.request_id(),
            stable_instance_id,
            None,
            move |_| {
                checkpoint_clock.advance(advance_past_default_lease_boundary);
                sample_capacity().expect("capacity sample at task checkpoint time");
            },
        )
        .expect("install authoritative deadline checkpoint");

    let receipt = host
        .process_request_for_test(&request, ConnectionId::new(170).expect("connection"))
        .expect("long task receipt");
    let deadline = match receipt.result() {
        Some(RuntimeResult::ContainedTaskCompleted {
            response_deadline_monotonic_ms: Some(deadline),
            outcome: TaskOutcome::Success,
            ..
        }) => *deadline,
        result => panic!("unexpected long task result: {result:?}"),
    };
    assert_eq!(deadline, ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS);
    assert_eq!(clock.monotonic_ms(), advance_past_default_lease_boundary);
    assert_eq!(checkpoint.consumed(), 1);
    let checkpoint_identity = checkpoint
        .observed()
        .expect("read authoritative deadline checkpoint")
        .expect("checkpoint identity");
    assert_eq!(checkpoint_identity.request_id(), request.request_id());
    assert_eq!(checkpoint_identity.instance_id(), stable_instance_id);
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    let persisted = host
        .query_persisted_events_for_test(EventQuery {
            request_id: Some(request.request_id()),
            ..EventQuery::default()
        })
        .expect("long task events");
    assert_eq!(
        persisted
            .iter()
            .filter_map(|event| event.links().lease_id().copied())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([checkpoint_identity.lease_id()])
    );
    for (event_type, expected_count) in [
        (EventType::TaskCompleted, 1),
        (EventType::TaskCancelled, 0),
        (EventType::LeaseReleased, 1),
        (EventType::TaskEffectIntent, 1),
        (EventType::TaskEffectCompleted, 1),
    ] {
        assert_eq!(
            persisted
                .iter()
                .filter(|event| event.event_type() == event_type)
                .count(),
            expected_count,
            "{event_type:?} count"
        );
    }
    let admitted = persisted
        .iter()
        .find(|event| {
            matches!(
                event.payload(),
                EventPayload::Task(TaskPayload::Semantic(payload))
                    if matches!(payload.fact(), TaskSemanticFact::PackageAdmitted { .. })
            )
        })
        .expect("package admitted event");
    assert!(matches!(
        admitted.payload(),
        EventPayload::Task(TaskPayload::Semantic(payload))
            if matches!(payload.fact(), TaskSemanticFact::PackageAdmitted {
                response_deadline_monotonic_ms: Some(recorded),
                ..
            } if *recorded == deadline)
    ));
    assert_eq!(
        admitted.links().lease_id(),
        Some(&checkpoint_identity.lease_id())
    );
    host.close().expect("close host");
}

#[test]
fn contained_task_deadline_checkpoint_fails_closed_on_lease_identity_mismatch() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("identity-mismatch-task.zip");
    let bytes = neutral_contained_task_package();
    fs::write(&package, &bytes).expect("write package");
    let expected = actingcommand_pack_containment::Sha256Hash::digest(&bytes).to_string();
    let state = Arc::new(FakeState::default());
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let stable_instance_id = instance_id();
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock.clone()),
        Arc::new(FakeProvider::one(
            "neutral.instance",
            stable_instance_id,
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let mismatched_lease_id = *ids.mint_lease_id().expect("mismatched lease").transport();
    let request = runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            "neutral.instance",
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.display().to_string(), expected)
                .expect("contained task request")
                .with_response_deadline_ms(7_000)
                .expect("bounded response deadline"),
        ),
    );
    let checkpoint_clock = Arc::clone(&clock);
    let checkpoint = host
        .run_at_contained_task_checkpoint_for_test(
            request.request_id(),
            stable_instance_id,
            Some(mismatched_lease_id),
            move |_| checkpoint_clock.advance(7_000),
        )
        .expect("install mismatched deadline checkpoint");

    let receipt = host
        .process_request_for_test(&request, ConnectionId::new(171).expect("connection"))
        .expect("identity mismatch receipt");
    assert!(matches!(
        receipt.result(),
        Some(RuntimeResult::ContainedTaskCompleted {
            outcome: TaskOutcome::Success,
            ..
        })
    ));
    assert_eq!(checkpoint.consumed(), 0);
    assert_eq!(
        checkpoint
            .observed()
            .expect("read mismatched deadline checkpoint"),
        None
    );
    assert_eq!(clock.monotonic_ms(), 0);
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    let persisted = host
        .query_persisted_events_for_test(EventQuery {
            request_id: Some(request.request_id()),
            ..EventQuery::default()
        })
        .expect("identity mismatch events");
    let actual_lease_id = persisted
        .iter()
        .find_map(|event| event.links().lease_id().copied())
        .expect("actual lease identity");
    assert_ne!(actual_lease_id, mismatched_lease_id);
    for (event_type, expected_count) in [
        (EventType::TaskCompleted, 1),
        (EventType::TaskCancelled, 0),
        (EventType::LeaseReleased, 1),
        (EventType::TaskEffectIntent, 1),
        (EventType::TaskEffectCompleted, 1),
    ] {
        assert_eq!(
            persisted
                .iter()
                .filter(|event| event.event_type() == event_type)
                .count(),
            expected_count,
            "{event_type:?} count"
        );
    }
    host.close().expect("close host");
}

#[test]
fn contained_task_deadline_commits_cancelled_terminal_and_releases_lease() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("deadline-task.zip");
    let bytes = neutral_contained_task_package();
    fs::write(&package, &bytes).expect("write package");
    let expected = actingcommand_pack_containment::Sha256Hash::digest(&bytes).to_string();
    let state = Arc::new(FakeState::default());
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let stable_instance_id = instance_id();
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock.clone()),
        Arc::new(FakeProvider::one(
            "neutral.instance",
            stable_instance_id,
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let task_request = ContainedTaskRequest::new(package.display().to_string(), expected)
        .expect("contained task request")
        .with_response_deadline_ms(25)
        .expect("bounded task deadline");
    let request = runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            "neutral.instance",
            ids.mint_holder_id().expect("holder"),
            task_request,
        ),
    );
    let checkpoint_clock = Arc::clone(&clock);
    let checkpoint = host
        .run_at_contained_task_checkpoint_for_test(
            request.request_id(),
            stable_instance_id,
            None,
            move |_| checkpoint_clock.advance(25),
        )
        .expect("install exceeded deadline checkpoint");
    let receipt = host
        .process_request_for_test(&request, ConnectionId::new(172).expect("connection"))
        .expect("deadline receipt");

    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Cancelled,
        "{receipt:#?}"
    );
    assert!(matches!(
        receipt.result(),
        Some(RuntimeResult::ContainedTaskCancelled {
            task_request_id,
            reason: actingcommand_contract::ContainedTaskCancellationReason::DeadlineExceeded,
            lease_terminal: actingcommand_contract::ContainedTaskLeaseTerminal::Released,
            ..
        }) if task_request_id == &request.request_id()
    ));
    assert_eq!(checkpoint.consumed(), 1);
    let checkpoint_identity = checkpoint
        .observed()
        .expect("read exceeded deadline checkpoint")
        .expect("exceeded deadline identity");
    assert_eq!(checkpoint_identity.request_id(), request.request_id());
    assert_eq!(checkpoint_identity.instance_id(), stable_instance_id);
    assert_eq!(clock.monotonic_ms(), 25);
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    let persisted = host
        .query_persisted_events_for_test(EventQuery {
            request_id: Some(request.request_id()),
            ..EventQuery::default()
        })
        .expect("deadline events");
    assert_eq!(
        persisted
            .iter()
            .filter(|event| event.event_type() == EventType::TaskCancelled)
            .count(),
        1
    );
    for event_type in [
        EventType::TaskCompleted,
        EventType::TaskEffectIntent,
        EventType::TaskEffectCompleted,
    ] {
        assert_eq!(
            persisted
                .iter()
                .filter(|event| event.event_type() == event_type)
                .count(),
            0,
            "{event_type:?} count"
        );
    }
    assert!(persisted.iter().any(|event| {
        event.event_type() == EventType::LeaseReleased
            && event.links().lease_id() == Some(&checkpoint_identity.lease_id())
    }));
    assert_eq!(
        persisted
            .iter()
            .filter(|event| event.event_type() == EventType::LeaseReleased)
            .count(),
        1
    );
    let deadline = persisted.iter().find_map(|event| match event.payload() {
        EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
            TaskSemanticFact::PackageAdmitted {
                response_deadline_monotonic_ms,
                ..
            } => *response_deadline_monotonic_ms,
            _ => None,
        },
        _ => None,
    });
    assert_eq!(deadline, Some(25));
    assert!(persisted.iter().any(|event| matches!(
        event.payload(),
        EventPayload::Task(TaskPayload::Semantic(payload))
            if matches!(payload.fact(), TaskSemanticFact::TerminalCommitted {
                outcome: TaskOutcome::Cancelled,
                executed_steps: Some(0),
                ..
            })
    )));

    let cancel = runtime_request(
        &ids,
        RuntimeOperation::CancelContainedTask {
            task_request_id: request.request_id(),
        },
    );
    let status = host
        .process_request_for_test(&cancel, ConnectionId::new(173).expect("connection"))
        .expect("cancellation status");
    assert!(matches!(
        status.result(),
        Some(RuntimeResult::ContainedTaskCancellation {
            task_request_id,
            status: actingcommand_contract::ContainedTaskCancellationStatus::Terminal {
                outcome: TaskOutcome::Cancelled,
                lease_disposition:
                    actingcommand_contract::ContainedTaskLeaseTerminal::Released,
                ..
            },
        }) if task_request_id == &request.request_id()
    ));
    host.close().expect("close host");

    let root = TempDir::new().expect("mid-step tempdir");
    let package = root.path().join("deadline-task.zip");
    fs::write(&package, &bytes).expect("write mid-step package");
    let state = Arc::new(FakeState::default());
    state.block_input.store(true, Ordering::Release);
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock.clone()),
        Arc::new(FakeProvider::one(
            "neutral.instance",
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .expect("mid-step host");
    let task_request = ContainedTaskRequest::new(
        package.display().to_string(),
        actingcommand_pack_containment::Sha256Hash::digest(&bytes).to_string(),
    )
    .unwrap()
    .with_response_deadline_ms(25)
    .unwrap();
    let request = runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            "neutral.instance",
            ids.mint_holder_id().unwrap(),
            task_request,
        ),
    );
    let receipt = thread::scope(|scope| {
        let worker = scope
            .spawn(|| host.process_request_for_test(&request, ConnectionId::new(174).unwrap()));
        let wait_deadline = Instant::now() + Duration::from_secs(5);
        while !state.input_started.load(Ordering::Acquire) && Instant::now() < wait_deadline {
            thread::sleep(Duration::from_millis(1));
        }
        let input_started = state.input_started.load(Ordering::Acquire);
        clock.advance(25);
        state.block_input.store(false, Ordering::Release);
        assert!(
            input_started,
            "mid-step input reached its existing boundary"
        );
        worker
            .join()
            .expect("mid-step worker")
            .expect("cancelled receipt")
    });
    assert_eq!(receipt.state(), RuntimeReceiptState::Cancelled);
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    let events = host
        .query_persisted_events_for_test(EventQuery {
            request_id: Some(request.request_id()),
            ..EventQuery::default()
        })
        .unwrap();
    assert!(events.iter().any(|event| matches!(
        event.payload(),
        EventPayload::Task(TaskPayload::Semantic(payload))
            if matches!(payload.fact(), TaskSemanticFact::TerminalCommitted {
                outcome: TaskOutcome::Cancelled,
                executed_steps: Some(1),
                ..
            })
    )));
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type() == EventType::InputCommitted)
            .count(),
        1
    );
    host.close().expect("close mid-step host");
}

#[test]
fn scheduled_contained_task_deadline_uses_existing_failure_settlement() {
    // Defect regression: Workflow #269 SCHEDULED-LEASE-BUDGET-v1,
    // B16 first red: issuecomment-5591140471. Reuse the existing task and clock.
    {
        let root = TempDir::new().expect("tempdir");
        let budget_ms = 6_000;
        let reserve_ms = 50;
        let short_ttl_ms = 1_200;
        let package = neutral_contained_task_package_with_execution_timeout(budget_ms);
        let package_path = root.path().join("scheduled-task.zip");
        fs::write(&package_path, &package).expect("write scheduled package");
        let request = ContainedTaskRequest::new(
            package_path.to_string_lossy().into_owned(),
            format!("{:x}", Sha256::digest(&package)),
        )
        .expect("scheduled package request")
        .with_response_deadline_ms(budget_ms)
        .expect("bounded scheduled response budget");
        let state = Arc::new(FakeState::default());
        state
            .transition_capture_after_input
            .store(true, Ordering::Release);
        let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
        let host = RuntimeHost::start(
            config(&root)
                .with_runtime_clock(clock.clone())
                .with_scheduler(SchedulerConfig {
                    maximum_client_heartbeat_interval_ms: reserve_ms,
                    takeover_cooldown_ms: 100,
                    lease_ttl_ms: short_ttl_ms,
                    ..SchedulerConfig::default()
                })
                .with_procedure_manifest(procedure_manifest_with_primary(
                    &package,
                    vec!["after_observation".to_owned()],
                )),
            Arc::new(FakeProvider::one(
                POLICY_INSTANCE_ALIAS,
                instance_id(),
                Arc::clone(&state),
            )),
        )
        .expect("scheduled runtime host");
        host.activate_policy_catalog(&policy_sources(1))
            .expect("activate scheduled catalog");
        let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
        record_policy_approval(&host, &intent);
        let PolicyDispatchAdmission::Granted { context } = host
            .admit_scheduled_policy_dispatch(
                &intent,
                &reasons,
                &policy_context(&host, &intent),
                &request,
            )
            .expect("scheduled request-budget admission")
        else {
            panic!("expected scheduled context")
        };
        assert_eq!(
            context.lease_token().expires_at_monotonic_ms(),
            budget_ms + reserve_ms
        );
        clock.advance(short_ttl_ms + 1);
        let receipt = host
            .run_scheduled_contained_task(&context, &request)
            .expect("declared task remains runnable past the default lease TTL");
        assert!(matches!(
            receipt.result(),
            Some(RuntimeResult::ContainedTaskCompleted {
                outcome: TaskOutcome::Success,
                response_deadline_monotonic_ms: Some(deadline),
                ..
            }) if *deadline == budget_ms
        ));
        let (outcome, _) = host
            .complete_scheduled_policy_run(&context, &receipt)
            .expect("settle the actual scheduled execution");
        assert_eq!(
            outcome.outcome,
            PolicyExecutionOutcome::Succeeded {
                runtime_ms: short_ttl_ms + 1
            }
        );
        assert_eq!(state.input_count.load(Ordering::Acquire), 1);
        let events = host
            .query_persisted_events_for_test(EventQuery {
                run_id: Some(context.run_id()),
                ..EventQuery::default()
            })
            .expect("scheduled lease and deadline evidence");
        assert!(events.iter().any(|event| matches!(
            event.payload(),
            EventPayload::Task(TaskPayload::Semantic(payload))
                if payload.lease_expires_at_monotonic_ms() == Some(budget_ms + reserve_ms)
                    && matches!(payload.fact(), TaskSemanticFact::PackageAdmitted {
                        response_deadline_monotonic_ms: Some(deadline),
                        ..
                    } if *deadline == budget_ms)
                    && event.links().lease_id() == Some(&context.lease_token().lease_id())
        )));
        for event_type in [
            EventType::LeaseGranted,
            EventType::TaskCompleted,
            EventType::LeaseReleased,
            EventType::PolicyExecutionRecorded,
            EventType::PolicyDispatchCompleted,
        ] {
            assert_eq!(
                events
                    .iter()
                    .filter(|event| event.event_type() == event_type)
                    .count(),
                1,
                "{event_type:?} must remain unique"
            );
        }
        host.close().expect("close scheduled host");
    }
    let root = TempDir::new().expect("tempdir");
    let (host, state, context, request, _) = admitted_physical_run_fixture(&root);
    state.capture_delay_ms.store(100, Ordering::Release);
    let request = request
        .with_response_deadline_ms(25)
        .expect("bounded scheduled deadline");

    let error = host
        .run_scheduled_contained_task(&context, &request)
        .expect_err("scheduled deadline must use failure settlement");
    assert_eq!(error.code(), "contained_task_deadline_exceeded");
    let events = host
        .query_persisted_events_for_test(EventQuery {
            run_id: Some(context.run_id()),
            ..EventQuery::default()
        })
        .expect("scheduled deadline events");
    for terminal in [
        EventType::TaskFailed,
        EventType::LeaseReleased,
        EventType::PolicyExecutionRecorded,
    ] {
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type() == terminal)
                .count(),
            1,
            "missing or duplicate {terminal:?}"
        );
    }
    host.close().expect("close host");
}
