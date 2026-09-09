// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn zero_stagger_host_requests_produce_one_grant_and_one_busy_denial() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let first = TestClient::connect(&host);
    let second = TestClient::connect(&host);
    let start = Arc::new(Barrier::new(3));
    let completed = Arc::new(Barrier::new(3));
    let first = concurrent_acquire(first, "node.a", Arc::clone(&start), Arc::clone(&completed));
    let second = concurrent_acquire(second, "node.a", Arc::clone(&start), Arc::clone(&completed));

    start.wait();
    completed.wait();
    let receipts = [
        first.join().expect("first client"),
        second.join().expect("second client"),
    ];
    let grants = receipts
        .iter()
        .filter(|receipt| matches!(receipt.result(), Some(RuntimeResult::LeaseGranted { .. })))
        .count();
    let busy = receipts
        .iter()
        .filter(|receipt| {
            receipt
                .error_projection()
                .is_some_and(|error| error.code == RuntimeErrorCode::LeaseBusy)
        })
        .count();
    assert_eq!(grants, 1);
    assert_eq!(busy, 1);
    assert_eq!(state.open_count.load(Ordering::Acquire), 0);
    assert_eq!(state.close_count.load(Ordering::Acquire), 0);
    host.close().expect("close host");
}

#[test]
fn queued_release_transfers_only_after_the_durable_transfer_fact() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut first = TestClient::connect(&host);
    let mut second = TestClient::connect(&host);
    let (_, old_token) = first.acquire("node.a");
    let (queued_request, status) = second.queue("node.a", LeasePriority::Normal, 2_000);
    assert!(!status.preempt_requested());
    let shutdown = first.request(RuntimeOperation::RequestShutdown {
        target: host.runtime_info().shutdown_target(),
    });
    let denied = first.send(&shutdown);
    assert_eq!(
        denied
            .error_projection()
            .expect("queued work prevents shutdown")
            .code,
        RuntimeErrorCode::RuntimeBusy
    );
    assert!(!host.is_shutdown_requested().expect("queue remains live"));

    let release = first.request(RuntimeOperation::ReleaseLease {
        token: old_token.clone(),
    });
    assert_eq!(first.send(&release).state(), RuntimeReceiptState::Completed);
    let poll = second.request(RuntimeOperation::PollQueuedLease {
        queued_request_id: status.request_id(),
    });
    let granted = second.send(&poll);
    let RuntimeResult::LeaseGranted { token: new_token } =
        granted.result().expect("transferred lease")
    else {
        panic!("expected transferred lease, got {:?}", granted.result());
    };
    assert_ne!(new_token.lease_id(), old_token.lease_id());
    assert_eq!(
        event_types_for_correlation(&mut second, queued_request.correlation_id()),
        vec![
            EventType::LeaseRequested,
            EventType::SchedulerQueued,
            EventType::LeaseTransitionIntent,
            EventType::LeaseTransferred,
        ]
    );
    assert_eq!(
        event_types_for_correlation(&mut first, release.correlation_id()),
        vec![
            EventType::SchedulerAdmitted,
            EventType::LeaseTransitionIntent,
            EventType::LeaseReleased,
        ]
    );
    assert_input_denied(&mut first, old_token, RuntimeErrorCode::LeaseMismatch);
    let release = second.request(RuntimeOperation::ReleaseLease {
        token: new_token.clone(),
    });
    assert_eq!(
        second.send(&release).state(),
        RuntimeReceiptState::Completed
    );
    assert_eq!(state.open_count.load(Ordering::Acquire), 0);
    drop(first);
    drop(second);
    host.close().expect("close host");
}

#[test]
fn high_priority_queue_transfers_immediately_at_an_idle_safe_boundary() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut first = TestClient::connect(&host);
    let mut second = TestClient::connect(&host);
    let (_, old_token) = first.acquire("node.a");
    let holder = second.ids.mint_holder_id().expect("holder id");
    let queued_request = second.request(RuntimeOperation::queue_lease(
        "node.a",
        holder,
        LeaseQueuePolicy::new(LeasePriority::High, 2_000).expect("queue policy"),
    ));

    let granted = second.send(&queued_request);
    let RuntimeResult::LeaseGranted { token: new_token } =
        granted.result().expect("idle preemption result")
    else {
        panic!("expected immediate idle transfer");
    };
    assert_eq!(granted.state(), RuntimeReceiptState::Admitted);
    assert_eq!(
        event_types_for_correlation(&mut second, queued_request.correlation_id()),
        vec![
            EventType::LeaseRequested,
            EventType::SchedulerQueued,
            EventType::SchedulerPreempted,
            EventType::LeaseTransitionIntent,
            EventType::LeaseTransferred,
        ]
    );
    assert_input_denied(&mut first, old_token, RuntimeErrorCode::LeaseMismatch);
    assert!(host.fatal_error().expect("runtime health").is_none());
    let release = second.request(RuntimeOperation::ReleaseLease {
        token: new_token.clone(),
    });
    assert_eq!(
        second.send(&release).state(),
        RuntimeReceiptState::Completed
    );
    assert_eq!(state.open_count.load(Ordering::Acquire), 0);
    drop(first);
    drop(second);
    host.close().expect("close host");
}

#[test]
fn high_priority_preemption_waits_for_the_durable_input_outcome() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.block_input.store(true, Ordering::Release);
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut first = TestClient::connect(&host);
    let mut second = TestClient::connect(&host);
    let (_, old_token) = first.acquire("node.a");
    let input_token = old_token.clone();
    let input_thread = thread::spawn(move || {
        let input = first.request(RuntimeOperation::Input {
            token: input_token,
            action: InputAction::Reset,
        });
        let receipt = first.send(&input);
        (first, receipt)
    });
    wait_until(Duration::from_secs(2), || {
        state.input_started.load(Ordering::Acquire)
    });

    let (queued_request, status) = second.queue("node.a", LeasePriority::High, 2_000);
    assert!(status.preempt_requested());
    let poll = second.request(RuntimeOperation::PollQueuedLease {
        queued_request_id: status.request_id(),
    });
    assert!(matches!(
        second.send(&poll).result(),
        Some(RuntimeResult::LeasePending { .. })
    ));
    assert_eq!(
        event_types_for_correlation(&mut second, queued_request.correlation_id()),
        vec![
            EventType::LeaseRequested,
            EventType::SchedulerQueued,
            EventType::SchedulerPreempted,
        ]
    );

    state.block_input.store(false, Ordering::Release);
    let (mut first, input_receipt) = input_thread.join().expect("input thread");
    assert_eq!(input_receipt.state(), RuntimeReceiptState::Completed);
    let poll = second.request(RuntimeOperation::PollQueuedLease {
        queued_request_id: status.request_id(),
    });
    let granted = second.send(&poll);
    let RuntimeResult::LeaseGranted { token: new_token } =
        granted.result().expect("preempted lease")
    else {
        panic!("expected preempted lease");
    };
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    assert_eq!(state.close_count.load(Ordering::Acquire), 1);
    let events = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("authoritative preemption events");
    let input = events
        .iter()
        .find(|event| event.event_type() == EventType::InputCommitted)
        .expect("durable input");
    let closed = events
        .iter()
        .find(|event| matches!(
            event.payload(),
            EventPayload::Runtime(actingcommand_contract::RuntimePayload::LifecycleObserved(payload))
                if matches!(payload.phase(), actingcommand_contract::RuntimeLifecyclePhase::ResourceQuiescence {
                    quiescence: actingcommand_contract::ResourceQuiescence::Confirmed,
                    owner_disposition: actingcommand_contract::OwnerResourceDisposition::ConfirmedClosed,
                    ..
                })
        ))
        .expect("confirmed resource quiescence");
    let transferred = events
        .iter()
        .find(|event| event.event_type() == EventType::LeaseTransferred)
        .expect("durable transfer");
    assert!(input.sequence() < closed.sequence());
    assert!(closed.sequence() < transferred.sequence());
    assert!(host.fatal_error().expect("runtime health").is_none());
    assert_input_denied(&mut first, old_token, RuntimeErrorCode::LeaseMismatch);
    assert_eq!(
        event_types_for_correlation(&mut second, queued_request.correlation_id()),
        vec![
            EventType::LeaseRequested,
            EventType::SchedulerQueued,
            EventType::SchedulerPreempted,
            EventType::LeaseTransitionIntent,
            EventType::LeaseTransferred,
        ]
    );
    let release = second.request(RuntimeOperation::ReleaseLease {
        token: new_token.clone(),
    });
    assert_eq!(
        second.send(&release).state(),
        RuntimeReceiptState::Completed
    );
    drop(first);
    drop(second);
    host.close().expect("close host");
    assert_eq!(state.close_count.load(Ordering::Acquire), 1);
}

#[test]
fn queued_request_is_connection_bound_and_cancellation_is_ledger_visible() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", state);
    let mut first = TestClient::connect(&host);
    let mut second = TestClient::connect(&host);
    let mut intruder = TestClient::connect(&host);
    let (_, token) = first.acquire("node.a");
    let (queued_request, status) = second.queue("node.a", LeasePriority::Normal, 2_000);

    let poll = intruder.request(RuntimeOperation::PollQueuedLease {
        queued_request_id: status.request_id(),
    });
    let denied = intruder.send(&poll);
    assert_eq!(denied.state(), RuntimeReceiptState::Denied);
    assert_eq!(
        denied.error_projection().expect("poll denial").code,
        RuntimeErrorCode::QueueConnectionMismatch
    );
    let cancel = second.request(RuntimeOperation::CancelQueuedLease {
        queued_request_id: status.request_id(),
    });
    let cancelled = second.send(&cancel);
    assert_eq!(cancelled.state(), RuntimeReceiptState::Cancelled);
    assert!(matches!(
        cancelled.result(),
        Some(RuntimeResult::LeaseQueueCancelled { request_id, .. })
            if *request_id == status.request_id()
    ));
    assert_eq!(
        event_types_for_correlation(&mut second, queued_request.correlation_id()),
        vec![
            EventType::LeaseRequested,
            EventType::SchedulerQueued,
            EventType::SchedulerDenied,
        ]
    );
    let release = first.request(RuntimeOperation::ReleaseLease { token });
    assert_eq!(first.send(&release).state(), RuntimeReceiptState::Completed);
    drop(first);
    drop(second);
    drop(intruder);
    host.close().expect("close host");
}

#[test]
fn disconnect_promotes_another_connections_queue_without_opening_a_backend() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut first = TestClient::connect(&host);
    let mut second = TestClient::connect(&host);
    let _ = first.acquire("node.a");
    let (queued_request, status) = second.queue("node.a", LeasePriority::Normal, 2_000);
    drop(first);

    let started = Instant::now();
    let new_token = loop {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "transfer timed out"
        );
        let poll = second.request(RuntimeOperation::PollQueuedLease {
            queued_request_id: status.request_id(),
        });
        let receipt = second.send(&poll);
        match receipt.result() {
            Some(RuntimeResult::LeaseGranted { token }) => break token.clone(),
            Some(RuntimeResult::LeasePending { .. }) => thread::sleep(Duration::from_millis(10)),
            other => panic!("unexpected disconnect transfer result: {other:?}"),
        }
    };
    assert_eq!(
        event_types_for_correlation(&mut second, queued_request.correlation_id()),
        vec![
            EventType::LeaseRequested,
            EventType::SchedulerQueued,
            EventType::LeaseTransitionIntent,
            EventType::LeaseTransferred,
        ]
    );
    let release = second.request(RuntimeOperation::ReleaseLease { token: new_token });
    assert_eq!(
        second.send(&release).state(),
        RuntimeReceiptState::Completed
    );
    assert_eq!(state.open_count.load(Ordering::Acquire), 0);
    drop(second);
    host.close().expect("close host");
}

#[test]
fn lease_expiry_promotes_the_queue_and_fences_the_expired_token() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let provider = Arc::new(FakeProvider::one(
        "node.a",
        instance_id(),
        Arc::clone(&state),
    ));
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock.clone())
            .with_scheduler(SchedulerConfig {
                maximum_client_heartbeat_interval_ms: 20,
                takeover_cooldown_ms: 40,
                lease_ttl_ms: 200,
                ..SchedulerConfig::default()
            }),
        provider,
    )
    .expect("runtime host");
    let mut first = TestClient::connect(&host);
    let mut second = TestClient::connect(&host);
    let (_, expired_token) = first.acquire("node.a");
    let (queued_request, status) = second.queue("node.a", LeasePriority::Normal, 1_000);
    clock.advance(250);

    let started = Instant::now();
    let new_token = loop {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "expiry transfer timed out"
        );
        let poll = second.request(RuntimeOperation::PollQueuedLease {
            queued_request_id: status.request_id(),
        });
        let receipt = second.send(&poll);
        match receipt.result() {
            Some(RuntimeResult::LeaseGranted { token }) => break token.clone(),
            Some(RuntimeResult::LeasePending { .. }) => thread::sleep(Duration::from_millis(10)),
            other => panic!("unexpected expiry transfer result: {other:?}"),
        }
    };
    assert_input_denied(&mut first, expired_token, RuntimeErrorCode::LeaseMismatch);
    assert_eq!(
        event_types_for_correlation(&mut second, queued_request.correlation_id()),
        vec![
            EventType::LeaseRequested,
            EventType::SchedulerQueued,
            EventType::LeaseTransitionIntent,
            EventType::LeaseTransferred,
        ]
    );
    let release = second.request(RuntimeOperation::ReleaseLease { token: new_token });
    assert_eq!(
        second.send(&release).state(),
        RuntimeReceiptState::Completed
    );
    drop(first);
    drop(second);
    host.close().expect("close host");
    assert_eq!(state.open_count.load(Ordering::Acquire), 0);
}

#[test]
fn exact_lease_expiry_checkpoint_promotes_queue_once_and_replays() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock.clone())
            .with_scheduler(SchedulerConfig {
                maximum_client_heartbeat_interval_ms: 20,
                takeover_cooldown_ms: 40,
                lease_ttl_ms: 200,
                ..SchedulerConfig::default()
            }),
        Arc::new(FakeProvider::one(
            "node.a",
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    let mut owner = TestClient::connect(&host);
    let mut waiter = TestClient::connect(&host);
    let (_, expired_token) = owner.acquire("node.a");
    let (queued_request, status) = waiter.queue("node.a", LeasePriority::Normal, 1_000);

    clock.advance(250);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let mismatched_token = LeaseToken::new(
        expired_token.owner_epoch(),
        expired_token.lease_id(),
        expired_token.instance_id(),
        *ids.mint_holder_id().expect("mismatched holder").transport(),
        expired_token.expires_at_monotonic_ms(),
    )
    .expect("mismatched exact-lease token");
    let mismatch = host
        .expire_lease_once_for_test(&mismatched_token)
        .expect_err("mismatched exact-lease identity must not consume the checkpoint");
    assert_eq!(mismatch.code(), "test_lease_expiry_token_identity_mismatch");
    for (identity, mismatched_token) in [
        (
            "owner epoch",
            LeaseToken::new(
                *ids.mint_owner_epoch()
                    .expect("mismatched owner epoch")
                    .transport(),
                expired_token.lease_id(),
                expired_token.instance_id(),
                expired_token.holder_id(),
                expired_token.expires_at_monotonic_ms(),
            )
            .expect("mismatched owner-epoch token"),
        ),
        (
            "lease",
            LeaseToken::new(
                expired_token.owner_epoch(),
                *ids.mint_lease_id().expect("mismatched lease").transport(),
                expired_token.instance_id(),
                expired_token.holder_id(),
                expired_token.expires_at_monotonic_ms(),
            )
            .expect("mismatched lease token"),
        ),
        (
            "instance",
            LeaseToken::new(
                expired_token.owner_epoch(),
                expired_token.lease_id(),
                *ids.mint_instance_id()
                    .expect("mismatched instance")
                    .transport(),
                expired_token.holder_id(),
                expired_token.expires_at_monotonic_ms(),
            )
            .expect("mismatched instance token"),
        ),
        (
            "expiry",
            LeaseToken::new(
                expired_token.owner_epoch(),
                expired_token.lease_id(),
                expired_token.instance_id(),
                expired_token.holder_id(),
                expired_token.expires_at_monotonic_ms() + 1,
            )
            .expect("mismatched expiry token"),
        ),
    ] {
        let mismatch = host
            .expire_lease_once_for_test(&mismatched_token)
            .expect_err("mismatched exact-lease identity must not consume the checkpoint");
        assert_eq!(
            mismatch.code(),
            "test_lease_expiry_token_identity_mismatch",
            "{identity}"
        );
    }
    let terminal = host
        .expire_lease_once_for_test(&expired_token)
        .expect("expire exact queued-owner lease");
    assert_eq!(
        host.expire_lease_once_for_test(&expired_token)
            .expect("replay exact queued-owner expiry"),
        terminal
    );

    let poll = waiter.request(RuntimeOperation::PollQueuedLease {
        queued_request_id: status.request_id(),
    });
    let promoted = waiter.send(&poll);
    let RuntimeResult::LeaseGranted { token: new_token } =
        promoted.result().expect("promoted lease result")
    else {
        panic!("queue promotion must complete before the checkpoint returns")
    };
    let new_token = new_token.clone();
    assert_input_denied(
        &mut owner,
        expired_token.clone(),
        RuntimeErrorCode::LeaseMismatch,
    );
    let expiry_events = projected_events(
        &mut owner,
        EventQuery {
            event_type: Some(EventType::LeaseExpired),
            instance_id: Some(expired_token.instance_id()),
            lease_id: Some(expired_token.lease_id()),
            ..EventQuery::default()
        },
    );
    let [expired] = expiry_events.as_slice() else {
        panic!("the exact expired lease must have one durable terminal")
    };
    assert_eq!(expired.sequence, terminal.sequence);
    assert_eq!(expired.event_id, terminal.event_id);
    assert_eq!(
        event_types_for_correlation(&mut waiter, queued_request.correlation_id()),
        vec![
            EventType::LeaseRequested,
            EventType::SchedulerQueued,
            EventType::LeaseTransitionIntent,
            EventType::LeaseTransferred,
        ]
    );
    let release = waiter.request(RuntimeOperation::ReleaseLease { token: new_token });
    assert_eq!(
        waiter.send(&release).state(),
        RuntimeReceiptState::Completed
    );
    drop(owner);
    drop(waiter);
    host.close().expect("close host");
    assert_eq!(state.open_count.load(Ordering::Acquire), 0);
}

#[test]
fn queued_timeout_is_a_visible_terminal_denial() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", state);
    let mut first = TestClient::connect(&host);
    let mut second = TestClient::connect(&host);
    let (_, token) = first.acquire("node.a");
    let (queued_request, status) = second.queue("node.a", LeasePriority::Normal, 50);
    thread::sleep(Duration::from_millis(100));

    let poll = second.request(RuntimeOperation::PollQueuedLease {
        queued_request_id: status.request_id(),
    });
    let expired = second.send(&poll);
    assert_eq!(expired.state(), RuntimeReceiptState::Denied);
    assert_eq!(
        expired.error_projection().expect("queue expiry").code,
        RuntimeErrorCode::QueueExpired
    );
    assert_eq!(
        event_types_for_correlation(&mut second, queued_request.correlation_id()),
        vec![
            EventType::LeaseRequested,
            EventType::SchedulerQueued,
            EventType::SchedulerDenied,
        ]
    );
    let release = first.request(RuntimeOperation::ReleaseLease { token });
    assert_eq!(first.send(&release).state(), RuntimeReceiptState::Completed);
    drop(first);
    drop(second);
    host.close().expect("close host");
}

#[derive(Clone, Copy)]
enum QueueExpiryContextCase {
    StaleSome,
    Missing,
}

fn queue_expiry_operation(
    client: &TestClient,
    operation: QueueOperationTestKind,
    queued_request_id: actingcommand_contract::RequestId,
) -> RuntimeRequest {
    match operation {
        QueueOperationTestKind::Poll => {
            client.request(RuntimeOperation::PollQueuedLease { queued_request_id })
        }
        QueueOperationTestKind::Cancel => {
            client.request(RuntimeOperation::CancelQueuedLease { queued_request_id })
        }
    }
}

fn assert_queue_expired(receipt: &RuntimeReceipt) -> TerminalEvent {
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied);
    assert_eq!(
        receipt.error_projection().expect("queue expiry").code,
        RuntimeErrorCode::QueueExpired
    );
    receipt.terminal().expect("queue expiry terminal")
}

fn assert_queue_cancelled(
    receipt: &RuntimeReceipt,
    queued_request_id: actingcommand_contract::RequestId,
) -> TerminalEvent {
    assert_eq!(receipt.state(), RuntimeReceiptState::Cancelled);
    assert!(matches!(
        receipt.result(),
        Some(RuntimeResult::LeaseQueueCancelled { request_id, .. })
            if *request_id == queued_request_id
    ));
    receipt.terminal().expect("queue cancellation terminal")
}

fn run_queue_expiry_context_case(
    operation: QueueOperationTestKind,
    context_case: QueueExpiryContextCase,
) {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock.clone()),
        Arc::new(FakeProvider::one("node.a", instance_id(), state)),
    )
    .expect("runtime host");
    let mut owner = TestClient::connect(&host);
    let mut waiter = TestClient::connect(&host);
    let (_, token) = owner.acquire("node.a");
    let (queued_request, status) = waiter.queue("node.a", LeasePriority::Normal, 50);
    let queued_request_id = status.request_id();

    let expired = match context_case {
        QueueExpiryContextCase::StaleSome => {
            let control = host
                .pause_queue_operation_after_snapshot_for_test(operation, queued_request_id)
                .expect("install queue race hook");
            let operation_thread = thread::spawn(move || {
                let request = queue_expiry_operation(&waiter, operation, queued_request_id);
                let receipt = waiter.send(&request);
                (waiter, receipt)
            });
            control.wait_until_paused();
            clock.advance(100);
            host.expire_all_queued_for_test()
                .expect("expire queued request");
            control.resume();
            let (returned_waiter, receipt) = operation_thread.join().expect("queue operation");
            waiter = returned_waiter;
            receipt
        }
        QueueExpiryContextCase::Missing => {
            clock.advance(100);
            host.expire_all_queued_for_test()
                .expect("expire queued request");
            let request = queue_expiry_operation(&waiter, operation, queued_request_id);
            waiter.send(&request)
        }
    };
    let terminal = assert_queue_expired(&expired);

    let repeated_poll = waiter.request(RuntimeOperation::PollQueuedLease { queued_request_id });
    let repeated = waiter.send(&repeated_poll);
    assert_eq!(assert_queue_expired(&repeated), terminal);
    assert_eq!(
        event_types_for_correlation(&mut waiter, queued_request.correlation_id()),
        vec![
            EventType::LeaseRequested,
            EventType::SchedulerQueued,
            EventType::SchedulerDenied,
        ]
    );
    assert_eq!(
        projected_events(
            &mut waiter,
            EventQuery {
                event_type: Some(EventType::SchedulerDenied),
                ..EventQuery::default()
            }
        )
        .len(),
        1
    );

    let release = owner.request(RuntimeOperation::ReleaseLease { token });
    assert_eq!(owner.send(&release).state(), RuntimeReceiptState::Completed);
    drop(owner);
    drop(waiter);
    host.close().expect("close host");
}

#[test]
fn poll_vs_expiry_sweep_recovers_one_terminal_for_stale_and_missing_context() {
    for context_case in [
        QueueExpiryContextCase::StaleSome,
        QueueExpiryContextCase::Missing,
    ] {
        run_queue_expiry_context_case(QueueOperationTestKind::Poll, context_case);
    }
}

#[test]
fn cancel_vs_expiry_sweep_recovers_one_terminal_for_stale_and_missing_context() {
    for context_case in [
        QueueExpiryContextCase::StaleSome,
        QueueExpiryContextCase::Missing,
    ] {
        run_queue_expiry_context_case(QueueOperationTestKind::Cancel, context_case);
    }
}

#[test]
fn cancel_before_expiry_sweep_replays_one_cancelled_terminal() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock.clone()),
        Arc::new(FakeProvider::one("node.a", instance_id(), state)),
    )
    .expect("runtime host");
    let mut owner = TestClient::connect(&host);
    let mut waiter = TestClient::connect(&host);
    let (_, token) = owner.acquire("node.a");
    let (queued_request, status) = waiter.queue("node.a", LeasePriority::Normal, 50);
    let queued_request_id = status.request_id();

    let cancel = waiter.request(RuntimeOperation::CancelQueuedLease { queued_request_id });
    let cancelled = waiter.send(&cancel);
    let terminal = assert_queue_cancelled(&cancelled, queued_request_id);

    clock.advance(100);
    host.expire_all_queued_for_test()
        .expect("sweep after queue cancellation");

    let poll = waiter.request(RuntimeOperation::PollQueuedLease { queued_request_id });
    assert_eq!(
        assert_queue_cancelled(&waiter.send(&poll), queued_request_id),
        terminal
    );
    let repeated_cancel = waiter.request(RuntimeOperation::CancelQueuedLease { queued_request_id });
    assert_eq!(
        assert_queue_cancelled(&waiter.send(&repeated_cancel), queued_request_id),
        terminal
    );
    assert_eq!(
        event_types_for_correlation(&mut waiter, queued_request.correlation_id()),
        vec![
            EventType::LeaseRequested,
            EventType::SchedulerQueued,
            EventType::SchedulerDenied,
        ]
    );
    assert_eq!(
        projected_events(
            &mut waiter,
            EventQuery {
                event_type: Some(EventType::SchedulerDenied),
                ..EventQuery::default()
            }
        )
        .len(),
        1
    );

    let release = owner.request(RuntimeOperation::ReleaseLease { token });
    assert_eq!(owner.send(&release).state(), RuntimeReceiptState::Completed);
    drop(owner);
    drop(waiter);
    host.close().expect("close host");
}
