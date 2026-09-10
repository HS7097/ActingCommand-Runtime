// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn shutdown_records_lifecycle_failures_before_writer_close() {
    let root = TempDir::new().expect("tempdir");
    let first = instance_id();
    let second = instance_id();
    let successful = instance_id();
    let states = [
        Arc::new(FakeState::default()),
        Arc::new(FakeState::default()),
        Arc::new(FakeState::default()),
    ];
    for state in &states[..2] {
        *state.close_error.lock().expect("close error") = Some(
            DeviceError::aggregate_close(
                "maatouch",
                [
                    Some(DeviceError::transient("reset native detail")),
                    Some(DeviceError::fatal("child stop detail")),
                    Some(DeviceError::fatal("reader join detail")),
                    Some(DeviceError::fatal("stderr C:\\private\\backend.log")),
                ],
            )
            .expect_err("phase failures"),
        );
    }
    let provider = FakeProvider::from_entries([
        ("node.a".to_owned(), first, Arc::clone(&states[0])),
        ("node.b".to_owned(), second, Arc::clone(&states[1])),
        ("node.c".to_owned(), successful, Arc::clone(&states[2])),
    ]);
    let host = RuntimeHost::start(config(&root), Arc::new(provider)).expect("host");
    let mut client = TestClient::connect(&host);
    for alias in ["node.a", "node.b", "node.c"] {
        let (_, token) = client.acquire(alias);
        let request = client.request(RuntimeOperation::Input {
            token,
            action: InputAction::Reset,
        });
        assert_eq!(
            client.send(&request).state(),
            RuntimeReceiptState::Completed
        );
    }
    drop(client);
    let error = host.close().expect_err("failed session drain");
    assert_eq!(error.code(), "input_backend_close_failed");
    assert!(error.is_fatal());
    for state in &states {
        assert_eq!(state.close_count.load(Ordering::Acquire), 1);
    }
    let ledger = GlobalLedger::open_evidence(
        actingcommand_ledger::GlobalLedgerEvidenceConfig::new(root.path()),
        |_| None,
    )
    .expect("authoritative read-only ledger");
    assert!(ledger.corrupt_tail().is_none());
    let events = ledger.query(&EventQuery::default());
    let failures = events
        .iter()
        .filter(|event| event.event_type() == EventType::RuntimeFailed)
        .collect::<Vec<_>>();
    assert_eq!(
        failures.len(),
        10,
        "four actual phases plus one typed resource-close cause for each failed instance"
    );
    for instance in [first, second] {
        let actual = failures
            .iter()
            .filter_map(|event| {
                let EventPayload::Runtime(actingcommand_contract::RuntimePayload::Failed(payload)) =
                    event.payload()
                else {
                    panic!("runtime failure");
                };
                let failure = payload.lifecycle_failure().expect("typed lifecycle");
                (failure.instance_id() == Some(instance)).then_some((event, failure))
            })
            .collect::<Vec<_>>();
        assert_eq!(actual.len(), 5);
        assert_eq!(
            actual
                .iter()
                .filter(|(_, failure)| failure.cause().expect("phase").resource().is_some())
                .count(),
            1
        );
        for (event, failure) in actual {
            assert_eq!(failure.stage(), "runtime.lifecycle.session_close");
            assert_eq!(
                event.links(),
                &actingcommand_contract::EventLinks::default()
            );
            assert!(failure.entered_event_id().is_none());
            assert!(failure.cause().expect("phase").native_detail().is_some());
            let public = serde_json::to_string(&event.payload().public_projection())
                .expect("public projection");
            assert!(!public.contains("native detail"));
            assert!(!public.contains("private"));
        }
    }
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type() == EventType::InputCommitted)
            .count(),
        3
    );
    let releases = events
        .iter()
        .filter(|event| event.event_type() == EventType::LeaseReleased)
        .collect::<Vec<_>>();
    assert!(releases.len() <= 1);
    assert!(
        releases
            .iter()
            .all(|event| event.links().instance_id() == Some(&successful))
    );
    assert!(events.iter().any(
        |event| event.event_type() == EventType::RuntimeLifecycleObserved
            && event.sequence() < failures[0].sequence()
    ));
    let summaries = events.iter().filter(|event| matches!(
        event.payload(),
        EventPayload::Runtime(actingcommand_contract::RuntimePayload::LifecycleObserved(value))
            if value.phase() == actingcommand_contract::RuntimeLifecyclePhase::DeviceDiagnosticSummary
    )).collect::<Vec<_>>();
    assert_eq!(
        summaries.len(),
        1,
        "one epoch summary after all close facts"
    );
    summaries[0]
        .payload()
        .validate()
        .expect("valid close budget");
    assert!(
        failures
            .iter()
            .all(|failure| failure.sequence() < summaries[0].sequence())
    );
    assert_eq!(
        events.last().expect("last ledger fact").event_id(),
        summaries[0].event_id()
    );
}

#[test]
fn runtime_status_lists_configured_instances_and_live_scheduler_state() {
    let root = TempDir::new().expect("tempdir");
    let state_a = Arc::new(FakeState::default());
    let state_b = Arc::new(FakeState::default());
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::from_entries([
            ("node.c".to_string(), instance_id(), Arc::clone(&state_b)),
            ("node.a".to_string(), instance_id(), Arc::clone(&state_a)),
        ])),
    )
    .expect("runtime host");
    let mut owner = TestClient::connect(&host);

    let initial_request = owner.request(RuntimeOperation::Status);
    let initial = owner
        .send_result(&initial_request)
        .expect("initial status receipt");
    let RuntimeResult::Status { status } = initial.result().expect("status result") else {
        panic!("expected runtime status");
    };
    assert_eq!(status.owner_epoch(), host.runtime_info().owner_epoch());
    assert_eq!(status.instances().len(), 2);
    assert_eq!(status.instances()[0].instance_alias(), "node.a");
    assert_eq!(status.instances()[1].instance_alias(), "node.c");
    let initial_source = status.source().expect("committed status source").clone();
    let source_events = projected_events(
        &mut owner,
        EventQuery {
            from_sequence: Some(initial_source.sequence),
            to_sequence: Some(initial_source.sequence),
            ..EventQuery::default()
        },
    );
    assert_eq!(source_events.len(), 1);
    let source_event = &source_events[0];
    assert_eq!(source_event.event_id, initial_source.event_id);
    assert_eq!(
        source_event.links.request_id(),
        Some(&initial_request.request_id())
    );
    let ProjectionPayload::Full(payload) = &source_event.payload else {
        panic!("full source fact");
    };
    let Some(actingcommand_contract::RuntimeStateFact::Observed {
        state: actingcommand_contract::RuntimeObservedState::ControlPlane { status: recorded },
        sampled_started_at_unix_ms,
        sampled_completed_at_unix_ms,
    }) = payload.runtime_state()
    else {
        panic!("typed control-plane source");
    };
    assert_eq!(recorded.instances(), status.instances());
    assert_eq!(recorded.owner_epoch(), status.owner_epoch());
    assert_eq!(
        *sampled_started_at_unix_ms,
        initial_source.sampled_started_at_unix_ms
    );
    assert_eq!(
        *sampled_completed_at_unix_ms,
        initial_source.sampled_completed_at_unix_ms
    );
    assert!(
        status
            .instances()
            .iter()
            .all(|instance| !instance.lease_active())
    );
    assert_eq!(state_a.open_count.load(Ordering::Acquire), 0);
    assert_eq!(state_b.open_count.load(Ordering::Acquire), 0);

    let acquire = owner.request(RuntimeOperation::acquire_lease(
        "node.a",
        owner.ids.mint_holder_id().expect("owner holder"),
    ));
    let acquire = owner.send(&acquire);
    assert!(matches!(
        acquire.result(),
        Some(RuntimeResult::LeaseGranted { .. })
    ));
    let mut waiter = TestClient::connect(&host);
    let queued = waiter.request(RuntimeOperation::queue_lease(
        "node.a",
        waiter.ids.mint_holder_id().expect("waiter holder"),
        LeaseQueuePolicy::new(LeasePriority::Normal, 1_000).expect("queue policy"),
    ));
    let queued = waiter.send(&queued);
    assert!(matches!(
        queued.result(),
        Some(RuntimeResult::LeaseQueued { .. })
    ));

    let live = owner
        .send_result(&owner.request(RuntimeOperation::Status))
        .expect("live status receipt");
    let RuntimeResult::Status { status } = live.result().expect("live status result") else {
        panic!("expected live runtime status");
    };
    let active = &status.instances()[0];
    assert!(status.source().unwrap().sequence > initial_source.sequence);
    assert!(active.lease_active());
    assert_eq!(active.queued_request_count(), 1);
    assert!(!active.takeover_cooldown_active());
    assert_eq!(state_a.open_count.load(Ordering::Acquire), 0);

    drop(waiter);
    drop(owner);
    host.close().expect("close host");
}

#[test]
fn runtime_registry_is_immutable_and_rejects_duplicate_instance_ids() {
    let root = TempDir::new().expect("tempdir");
    let hidden_state = Arc::new(FakeState::default());
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(
            FakeProvider::from_entries([
                (
                    "node.a".to_string(),
                    instance_id(),
                    Arc::new(FakeState::default()),
                ),
                (
                    "hidden.node".to_string(),
                    instance_id(),
                    Arc::clone(&hidden_state),
                ),
            ])
            .with_inventory(["node.a".to_string()]),
        ),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    let hidden = client.request(RuntimeOperation::acquire_lease(
        "hidden.node",
        client.ids.mint_holder_id().expect("hidden holder"),
    ));
    let hidden = client.send(&hidden);
    assert_eq!(hidden.state(), RuntimeReceiptState::Denied);
    assert_eq!(
        hidden.error_projection().expect("hidden denial").code,
        RuntimeErrorCode::InstanceUnknown
    );
    assert_eq!(hidden_state.open_count.load(Ordering::Acquire), 0);
    drop(client);
    host.close().expect("close host");

    let duplicate_root = TempDir::new().expect("duplicate tempdir");
    let duplicate_id = instance_id();
    let duplicate = RuntimeHost::start(
        config(&duplicate_root),
        Arc::new(FakeProvider::from_entries([
            (
                "node.a".to_string(),
                duplicate_id,
                Arc::new(FakeState::default()),
            ),
            (
                "node.c".to_string(),
                duplicate_id,
                Arc::new(FakeState::default()),
            ),
        ])),
    );
    let error = match duplicate {
        Ok(host) => {
            host.close().expect("close unexpected host");
            panic!("duplicate instance IDs must fail startup");
        }
        Err(error) => error,
    };
    assert_eq!(error.code(), "duplicate_runtime_instance_id");
    assert!(error.is_fatal());
}

#[test]
fn safe_reset_owns_lease_input_and_release_under_one_correlation() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::safe_reset("node.a", client.ids.mint_holder_id().expect("holder")),
    );
    let receipt = client.send(&request);
    assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
    assert!(matches!(
        receipt.result(),
        Some(RuntimeResult::SafeResetCompleted { .. })
    ));
    assert_eq!(state.open_count.load(Ordering::Acquire), 1);
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    assert_eq!(state.close_count.load(Ordering::Acquire), 1);
    assert_eq!(
        event_types_for_correlation(&mut client, correlation_id),
        vec![
            EventType::CliCommand,
            EventType::CommandReceived,
            EventType::CommandValidated,
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
    drop(client);
    host.close().expect("close host");
    assert_eq!(state.close_count.load(Ordering::Acquire), 1);
}

#[test]
fn application_lifecycle_owns_lease_effect_and_release_under_one_correlation() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "neutral.instance", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::application_lifecycle(
            "neutral.instance",
            client.ids.mint_holder_id().expect("holder"),
            ApplicationLifecycleAction::Restart,
        ),
    );

    let receipt = client.send(&request);
    assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
    assert!(matches!(
        receipt.result(),
        Some(RuntimeResult::ApplicationLifecycleCompleted {
            action: ApplicationLifecycleAction::Restart,
            ..
        })
    ));
    assert_eq!(state.application_count.load(Ordering::Acquire), 1);
    assert_eq!(
        event_types_for_correlation(&mut client, correlation_id),
        vec![
            EventType::CliCommand,
            EventType::CommandReceived,
            EventType::CommandValidated,
            EventType::LeaseRequested,
            EventType::SchedulerAdmitted,
            EventType::LeaseTransitionIntent,
            EventType::LeaseGranted,
            EventType::SchedulerAdmitted,
            EventType::ApplicationIntent,
            EventType::ApplicationCompleted,
            EventType::SchedulerAdmitted,
            EventType::LeaseTransitionIntent,
            EventType::LeaseReleased,
        ]
    );
    drop(client);
    host.close().expect("close host");
}

#[test]
fn application_lifecycle_is_denied_while_another_client_holds_the_instance() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "neutral.instance", Arc::clone(&state));
    let mut owner = TestClient::connect(&host);
    let mut contender = TestClient::connect(&host);
    let (_request, token) = owner.acquire("neutral.instance");
    let request = contender.request(RuntimeOperation::application_lifecycle(
        "neutral.instance",
        contender.ids.mint_holder_id().expect("holder"),
        ApplicationLifecycleAction::Stop,
    ));

    let receipt = contender.send(&request);
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied);
    assert_eq!(state.application_count.load(Ordering::Acquire), 0);

    let release = owner.request(RuntimeOperation::ReleaseLease { token });
    assert_eq!(owner.send(&release).state(), RuntimeReceiptState::Completed);
    drop(owner);
    drop(contender);
    host.close().expect("close host");
}

#[test]
fn safe_reset_replay_without_connection_cache_does_not_repeat_input() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let request = runtime_request(
        &ids,
        RuntimeOperation::safe_reset("node.a", ids.mint_holder_id().expect("holder")),
    );
    let connection = ConnectionId::new(77).expect("connection");

    let first = host
        .process_request_for_test(&request, connection)
        .expect("first safe reset");
    let replayed = host
        .process_request_for_test(&request, connection)
        .expect("replayed safe reset");

    assert_eq!(replayed, first);
    assert_eq!(state.open_count.load(Ordering::Acquire), 1);
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    assert_eq!(state.close_count.load(Ordering::Acquire), 1);
    assert_eq!(
        event_types_for_request(&host, &ids, connection, request.request_id()).len(),
        13
    );
    host.close().expect("close host");
    assert_eq!(state.close_count.load(Ordering::Acquire), 1);
}

#[test]
fn safe_reset_replay_recovers_from_durable_ledger_after_host_restart() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let fixed_instance = instance_id();
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let request = runtime_request(
        &ids,
        RuntimeOperation::safe_reset("node.a", ids.mint_holder_id().expect("holder")),
    );
    let connection = ConnectionId::new(88).expect("connection");
    let first = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            "node.a",
            fixed_instance,
            Arc::clone(&state),
        )),
    )
    .expect("first Runtime host");
    let first_receipt = first
        .process_request_for_test(&request, connection)
        .expect("first safe reset");
    first.close().expect("close first host");

    let second = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            "node.a",
            fixed_instance,
            Arc::clone(&state),
        )),
    )
    .expect("second Runtime host");
    let replayed = second
        .process_request_for_test(&request, connection)
        .expect("durable replay");

    assert_eq!(replayed, first_receipt);
    assert_eq!(state.open_count.load(Ordering::Acquire), 1);
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    assert_eq!(state.close_count.load(Ordering::Acquire), 1);
    second.close().expect("close second host");

    // S3 extends the existing owner/startup specification through the formal offline entry.
    let source_root = TempDir::new().expect("legacy runtime root");
    drop(
        RuntimeStateStore::open(source_root.path(), b"runtime-host-test-salt")
            .expect("existing State material"),
    );
    let segment = GlobalLedger::open(GlobalLedgerConfig::new(
        source_root.path().join("ledger"),
        "legacy-source",
    ))
    .expect("legacy source");
    segment.close().expect("source closed");
    let original_writer =
        std::fs::read(source_root.path().join("ledger/writer.lock")).expect("source writer bytes");
    let external = TempDir::new().expect("maintenance destinations");
    let backup = external.path().join("backup");
    let maintenance = |operation, target| {
        RuntimeHost::maintain_ledger(
            config(&source_root),
            crate::LedgerMaintenanceRequest {
                operation,
                backup: Some(backup.clone()),
                target,
                artifact_root: None,
                limits: Default::default(),
            },
        )
    };
    let refused = RuntimeHost::start(
        config(&source_root),
        Arc::new(FakeProvider::one(
            "node.a",
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .err()
    .expect("legacy startup requires migration");
    assert_eq!(refused.code(), "ledger_migration_required");
    let frozen =
        maintenance(crate::LedgerMaintenanceOperation::Backup, None).expect("formal frozen backup");
    assert_eq!(frozen.status, "backed-up");
    let preview =
        maintenance(crate::LedgerMaintenanceOperation::DryRun, None).expect("formal dry-run");
    assert_eq!(preview.status, "dry-run");
    assert_eq!(
        preview.ledger,
        actingcommand_ledger::LedgerStorageStatus::Missing
    );
    assert!(!preview.activated);
    let delivered =
        maintenance(crate::LedgerMaintenanceOperation::Import, None).expect("formal import");
    assert_eq!(delivered.status, "imported");
    assert_eq!(delivered.backup_id, frozen.backup_id);
    let repeated = maintenance(crate::LedgerMaintenanceOperation::Import, None)
        .expect("formal idempotent import");
    assert_eq!(repeated.status, "already-imported");
    assert_eq!(repeated.ledger, delivered.ledger);
    assert_eq!(
        std::fs::read(source_root.path().join("ledger/writer.lock")).unwrap(),
        original_writer
    );
    let restore_target = external.path().join("restored");
    let restored = maintenance(
        crate::LedgerMaintenanceOperation::Restore,
        Some(restore_target.clone()),
    )
    .expect("exact pre-cutover restore before later events");
    assert_eq!(restored.status, "restored");
    assert!(!restored.activated);
    assert_eq!(
        restored.ledger,
        actingcommand_ledger::LedgerStorageStatus::Missing
    );
    let migrated = host_with_state(&source_root, "node.a", Arc::new(FakeState::default()));
    migrated
        .close()
        .expect("normal SQLite startup after cutover");
    let refused = maintenance(
        crate::LedgerMaintenanceOperation::Restore,
        Some(external.path().join("discard-forbidden")),
    )
    .expect_err("new Runtime facts cannot be lost");
    assert!(matches!(
        refused.code.as_str(),
        "restore_would_discard_new_events" | "restore_state_has_advanced"
    ));
    assert!(!external.path().join("discard-forbidden").exists());
}

#[test]
fn second_owner_is_rejected_and_clean_restart_gets_a_new_epoch() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let first = host_with_state(&root, "node.a", Arc::clone(&state));
    assert!(root.path().join(RUNTIME_INFO_FILE).is_file());
    let first_epoch = first.runtime_info().owner_epoch();
    let error = match RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            "node.a",
            instance_id(),
            Arc::clone(&state),
        )),
    ) {
        Ok(host) => {
            host.close().expect("close unexpected host");
            panic!("second owner must fail");
        }
        Err(error) => error,
    };
    assert_eq!(error.code(), "owner_conflict");
    assert_eq!(error.projection().code, RuntimeErrorCode::OwnerConflict);
    // C-SHUTDOWN-v1 specification: exact owner, scoped work, and one admission.
    let ids = IdentifierIssuer::new().expect("shutdown ids");
    let connection = ConnectionId::new(999).expect("shutdown connection");
    let target = first.runtime_info().shutdown_target();
    for wrong in [
        actingcommand_contract::RuntimeShutdownTarget {
            pid: target.pid + 1,
            ..target
        },
        actingcommand_contract::RuntimeShutdownTarget {
            started_at_unix_ms: target.started_at_unix_ms + 1,
            ..target
        },
    ] {
        let request = runtime_request(&ids, RuntimeOperation::RequestShutdown { target: wrong });
        let denied = first
            .process_request_for_test(&request, connection)
            .expect("owner rejection");
        assert_eq!(
            denied.error_projection().expect("typed mismatch").code,
            RuntimeErrorCode::RuntimeOwnerMismatch
        );
        assert!(denied.terminal().is_some());
        assert!(!first.is_shutdown_requested().expect("still running"));
    }
    let request = runtime_request(&ids, RuntimeOperation::RequestShutdown { target });
    let mut forged = serde_json::to_value(&request).expect("request JSON");
    forged["actor"] = serde_json::json!("agent");
    forged["source"] = serde_json::json!("adapter");
    let forged = serde_json::from_value(forged).expect("unvalidated origin");
    let denied = first
        .process_request_for_test(&forged, connection)
        .expect("origin rejection");
    assert_eq!(
        denied
            .error_projection()
            .expect("typed origin rejection")
            .code,
        RuntimeErrorCode::InvalidRequest
    );
    assert!(denied.terminal().is_none());
    {
        let _work = first
            .begin_policy_work()
            .expect("policy admission")
            .expect("running");
        let busy = first
            .process_request_for_test(&request, connection)
            .expect("work rejection");
        assert_eq!(
            busy.error_projection().expect("typed busy").code,
            RuntimeErrorCode::RuntimeBusy
        );
        assert!(!first.is_shutdown_requested().expect("still running"));
    }
    let started = Instant::now();
    let accepted = loop {
        let receipt = first
            .process_request_for_test(&request, connection)
            .expect("shutdown receipt");
        if receipt.state() == RuntimeReceiptState::Admitted {
            break receipt;
        }
        assert_eq!(
            receipt.error_projection().expect("only busy").code,
            RuntimeErrorCode::RuntimeBusy
        );
        eprintln!("WARNING shutdown specification: RuntimeBusy; waiting for existing sweep");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "shutdown remained busy"
        );
        thread::sleep(Duration::from_millis(10));
    };
    assert!(
        matches!(accepted.result(), Some(RuntimeResult::ShutdownAccepted { target: actual }) if *actual == target)
    );
    assert!(
        first
            .begin_policy_work()
            .expect("closed admission")
            .is_none()
    );
    let repeated = first
        .process_request_for_test(&request, connection)
        .expect("second shutdown");
    assert_eq!(
        repeated.error_projection().expect("already stopping").code,
        RuntimeErrorCode::RuntimeUnavailable
    );
    let events = first
        .query_persisted_events_for_test(EventQuery::default())
        .expect("shutdown facts");
    assert_eq!(events.iter().filter(|event| matches!(event.payload(), EventPayload::Runtime(actingcommand_contract::RuntimePayload::LifecycleObserved(payload)) if matches!(payload.phase(), actingcommand_contract::RuntimeLifecyclePhase::ShutdownRequest { decision: actingcommand_contract::RuntimeShutdownDecision::Accepted, .. }))).count(), 1);
    assert!(!events.iter().any(|event| matches!(
        event.event_type(),
        EventType::LeaseGranted | EventType::InputCommitted
    )));
    first.close().expect("close first host");
    assert!(!root.path().join(RUNTIME_INFO_FILE).exists());

    let second = host_with_state(&root, "node.a", state);
    assert_ne!(second.runtime_info().owner_epoch(), first_epoch);
    let denied = second
        .process_request_for_test(&request, connection)
        .expect("old epoch rejected");
    assert_eq!(
        denied.error_projection().expect("epoch mismatch").code,
        RuntimeErrorCode::RuntimeOwnerMismatch
    );
    assert!(
        !second
            .is_shutdown_requested()
            .expect("new owner stays running")
    );
    second.close().expect("close second host");
}

#[test]
fn owner_journal_recovers_only_an_incomplete_final_record() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    host_with_state(&root, "node.a", Arc::clone(&state))
        .close()
        .expect("close initial host");
    let owner_path = root.path().join(crate::owner::OWNER_FILE_NAME);
    OpenOptions::new()
        .append(true)
        .open(&owner_path)
        .expect("open owner journal")
        .write_all(br#"{"incomplete"#)
        .expect("append incomplete tail");

    let recovered = host_with_state(&root, "node.a", state);
    recovered.close().expect("close recovered host");
    let content = std::fs::read(&owner_path).expect("read owner journal");
    assert!(content.ends_with(b"\n"));
    assert!(!content.windows(10).any(|window| window == b"incomplete"));
}

#[test]
fn complete_owner_journal_corruption_is_fatal() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    host_with_state(&root, "node.a", Arc::clone(&state))
        .close()
        .expect("close initial host");
    let owner_path = root.path().join(crate::owner::OWNER_FILE_NAME);
    OpenOptions::new()
        .append(true)
        .open(owner_path)
        .expect("open owner journal")
        .write_all(b"not-json\n")
        .expect("append corruption");
    let result = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one("node.a", instance_id(), state)),
    );
    let error = match result {
        Ok(host) => {
            host.close().expect("close unexpected host");
            panic!("corrupt owner journal must fail");
        }
        Err(error) => error,
    };
    assert_eq!(error.code(), "owner_record_invalid");
    assert!(error.is_fatal());
}
