// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_scheduler::ClaimKind;

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
            frame: None,
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
    assert_eq!(state.close_count.load(Ordering::Acquire), 0);
    let events = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("authoritative preemption events");
    let input = events
        .iter()
        .find(|event| event.event_type() == EventType::InputCommitted)
        .expect("durable input");
    let transferred = events
        .iter()
        .find(|event| event.event_type() == EventType::LeaseTransferred)
        .expect("durable transfer");
    assert!(input.sequence() < transferred.sequence());
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

// Workflow #369 S1: one queue per instance (model-369-queue.md v3.1 Q-2a, Q-4 to Q-8).

fn lease_expired_count(client: &mut TestClient, token: &LeaseToken) -> usize {
    projected_events(
        client,
        EventQuery {
            event_type: Some(EventType::LeaseExpired),
            lease_id: Some(token.lease_id()),
            ..EventQuery::default()
        },
    )
    .len()
}

#[test]
fn a_run_linked_backend_failure_hands_the_lease_to_the_queue_under_the_run_links() {
    // Q-4, C1, C9: a backend failure no longer cancels the queue. The lease end transfers to
    // the first eligible entry and writes `release_via_transfer`'s event set, the releaser's
    // records under the failed run's links.
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let claim = host
        .request_host_claim_for_test("node.a", ClaimKind::DirectTaskRun)
        .expect("claim on a free instance");
    let token = claim.granted.clone().expect("granted at once");
    let mut waiter = TestClient::connect(&host);
    let (queued_request, status) = waiter.queue("node.a", LeasePriority::Normal, 2_000);

    let (task_id, run_id) = host
        .fail_scheduled_host_claim_for_test(&claim, &token)
        .expect("run-linked backend failure cleanup");

    let poll = waiter.request(RuntimeOperation::PollQueuedLease {
        queued_request_id: status.request_id(),
    });
    let granted = waiter.send(&poll);
    let RuntimeResult::LeaseGranted { token: next } = granted.result().expect("poll result") else {
        panic!("expected the transferred lease, got {:?}", granted.result());
    };
    assert_ne!(next.lease_id(), token.lease_id());
    assert_eq!(
        event_types_for_correlation(&mut waiter, queued_request.correlation_id()),
        vec![
            EventType::LeaseRequested,
            EventType::SchedulerQueued,
            EventType::LeaseTransitionIntent,
            EventType::LeaseTransferred,
        ],
        "the waiter was handed the lease, not cancelled"
    );
    let run_events = projected_events(
        &mut waiter,
        EventQuery {
            run_id: Some(run_id),
            ..EventQuery::default()
        },
    );
    assert_eq!(
        run_events
            .iter()
            .map(|event| event.event_type)
            .collect::<Vec<_>>(),
        vec![EventType::LeaseTransitionIntent, EventType::LeaseReleased],
        "exactly one run-linked release"
    );
    for event in &run_events {
        assert_eq!(event.links.task_id(), Some(&task_id));
        assert_eq!(event.links.lease_id(), Some(&token.lease_id()));
        assert_eq!(event.severity, EventSeverity::Info);
    }
    let transferred = projected_events(
        &mut waiter,
        EventQuery {
            event_type: Some(EventType::LeaseTransferred),
            lease_id: Some(next.lease_id()),
            ..EventQuery::default()
        },
    );
    let [transferred] = transferred.as_slice() else {
        panic!("one transfer to the waiter");
    };
    assert!(transferred.sequence > run_events[0].sequence);
    assert!(transferred.sequence < run_events[1].sequence);
    assert!(host.fatal_error().expect("runtime health").is_none());
    let release = waiter.request(RuntimeOperation::ReleaseLease {
        token: next.clone(),
    });
    assert_eq!(
        waiter.send(&release).state(),
        RuntimeReceiptState::Completed
    );
    drop(waiter);
    host.close().expect("close host");
}

#[test]
fn an_enqueue_racing_a_lease_end_is_recorded_before_its_transfer() {
    // Q-6 (B2): an enqueue on one thread interleaved with a lease end on another never meets
    // `lease_transfer_context_missing`, and the request's `lease.requested` and
    // `scheduler.queued` come before its `lease.transferred`.
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    for round in 0..12 {
        let mut owner = TestClient::connect(&host);
        let mut waiter = TestClient::connect(&host);
        let (_, owner_token) = owner.acquire("node.a");
        let queue = waiter.request(RuntimeOperation::queue_lease(
            "node.a",
            waiter.ids.mint_holder_id().expect("waiter holder"),
            LeaseQueuePolicy::new(LeasePriority::Normal, 2_000).expect("queue policy"),
        ));
        let release = owner.request(RuntimeOperation::ReleaseLease { token: owner_token });
        let start = Arc::new(Barrier::new(2));
        let (waiter_receipt, owner_receipt) = thread::scope(|scope| {
            let queue_start = Arc::clone(&start);
            let queue_request = queue.clone();
            let queued = scope.spawn(move || {
                queue_start.wait();
                let receipt = waiter.send(&queue_request);
                (waiter, receipt)
            });
            start.wait();
            let released = owner.send(&release);
            (queued.join().expect("queue thread"), released)
        });
        let (mut waiter, queued) = waiter_receipt;
        assert_eq!(
            owner_receipt.state(),
            RuntimeReceiptState::Completed,
            "round {round}"
        );
        let token = match queued.result() {
            Some(RuntimeResult::LeaseGranted { token }) => token.clone(),
            Some(RuntimeResult::LeaseQueued { status }) => {
                let started = Instant::now();
                loop {
                    assert!(
                        started.elapsed() < Duration::from_secs(5),
                        "round {round}: the queued request was never granted"
                    );
                    let poll = waiter.request(RuntimeOperation::PollQueuedLease {
                        queued_request_id: status.request_id(),
                    });
                    match waiter.send(&poll).result() {
                        Some(RuntimeResult::LeaseGranted { token }) => break token.clone(),
                        Some(RuntimeResult::LeasePending { .. }) => {
                            thread::sleep(Duration::from_millis(5));
                        }
                        other => panic!("round {round}: unexpected poll result {other:?}"),
                    }
                }
            }
            other => panic!("round {round}: unexpected queue result {other:?}"),
        };
        assert!(
            host.fatal_error().expect("runtime health").is_none(),
            "round {round}"
        );
        let types = event_types_for_correlation(&mut waiter, queue.correlation_id());
        let position = |wanted: EventType| types.iter().position(|kind| *kind == wanted);
        if let Some(transferred) = position(EventType::LeaseTransferred) {
            let requested = position(EventType::LeaseRequested).expect("requested");
            let queued = position(EventType::SchedulerQueued).expect("queued");
            assert!(
                requested < queued && queued < transferred,
                "round {round}: {types:?}"
            );
        } else {
            assert!(
                types.contains(&EventType::LeaseGranted),
                "round {round}: {types:?}"
            );
        }
        let release = waiter.request(RuntimeOperation::ReleaseLease { token });
        assert_eq!(
            waiter.send(&release).state(),
            RuntimeReceiptState::Completed,
            "round {round}"
        );
    }
    host.close().expect("close host");
}

#[test]
fn a_sweep_tick_completes_while_another_instance_guard_is_held() {
    // Q-8: the sweep's queue expiry takes no admission guard, and a lapsed holder's cleanup only
    // tries it, so one instance held inside a device step stalls no other instance.
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let provider = FakeProvider::from_entries([
        (
            "node.a".to_string(),
            instance_id(),
            Arc::new(FakeState::default()),
        ),
        (
            "node.c".to_string(),
            instance_id(),
            Arc::new(FakeState::default()),
        ),
    ]);
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock.clone())
            .with_scheduler(SchedulerConfig {
                maximum_client_heartbeat_interval_ms: 20,
                takeover_cooldown_ms: 40,
                lease_ttl_ms: 200,
                ..SchedulerConfig::default()
            }),
        Arc::new(provider),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    let (_, held_token) = client.acquire("node.a");
    let (_, free_token) = client.acquire("node.c");
    let admission = host
        .instance_admission_for_test("node.a")
        .expect("node.a admission guard");
    let (held_tx, held_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let holder = thread::spawn(move || {
        let _guard = admission.lock().expect("hold node.a admission");
        held_tx.send(()).expect("report the held guard");
        release_rx.recv().expect("release signal");
    });
    held_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("node.a guard held");
    clock.advance(250);

    let (swept, held_expired) = thread::scope(|scope| {
        let (done_tx, done_rx) = mpsc::channel();
        let host = &host;
        scope.spawn(move || {
            done_tx
                .send(
                    host.expire_due_leases_for_test()
                        .map_err(|error| error.code()),
                )
                .expect("report the sweep");
        });
        let swept = done_rx.recv_timeout(Duration::from_secs(5));
        let held_expired = lease_expired_count(&mut client, &held_token);
        release_tx.send(()).expect("let node.a go");
        (swept, held_expired)
    });
    holder.join().expect("guard holder");
    assert_eq!(
        swept.expect("the sweep tick must not wait for node.a"),
        Ok(())
    );
    assert_eq!(held_expired, 0, "node.a is retried at a later tick");
    wait_until(Duration::from_secs(5), || {
        lease_expired_count(&mut client, &free_token) == 1
    });
    host.expire_due_leases_for_test()
        .expect("the next tick cleans node.a up");
    wait_until(Duration::from_secs(5), || {
        lease_expired_count(&mut client, &held_token) == 1
    });
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(client);
    host.close().expect("close host");
}

#[test]
fn a_renewal_of_a_runtime_hold_writes_the_renew_set_while_the_guard_is_held() {
    // Q-5: a Runtime holder renews under the queue-order lock, without the admission guard it
    // may hold across a device step, with today's renew set under its own request links.
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let claim = host
        .request_host_claim_for_test("node.a", ClaimKind::EmulatorControl)
        .expect("claim on a free instance");
    let token = claim.granted.clone().expect("granted at once");
    let admission = host
        .instance_admission_for_test("node.a")
        .expect("node.a admission guard");
    let (held_tx, held_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let holder = thread::spawn(move || {
        let _guard = admission.lock().expect("hold node.a admission");
        held_tx.send(()).expect("report the held guard");
        release_rx.recv().expect("release signal");
    });
    held_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("node.a guard held");
    let renewed = thread::scope(|scope| {
        let (done_tx, done_rx) = mpsc::channel();
        let host = &host;
        let claim = &claim;
        let token = &token;
        scope.spawn(move || {
            done_tx
                .send(
                    host.renew_host_claim_for_test(claim, token, 90_000)
                        .map_err(|error| error.code()),
                )
                .expect("report the renewal");
        });
        let renewed = done_rx.recv_timeout(Duration::from_secs(5));
        release_tx.send(()).expect("let node.a go");
        renewed
    });
    holder.join().expect("guard holder");
    let renewed = renewed
        .expect("the renewal must not wait for the admission guard")
        .expect("renewal");
    assert_eq!(renewed.lease_id(), token.lease_id());
    assert_eq!(renewed.holder_id(), token.holder_id());
    assert!(renewed.expires_at_monotonic_ms() > token.expires_at_monotonic_ms());

    let mut client = TestClient::connect(&host);
    let events = projected_events(
        &mut client,
        EventQuery {
            correlation_id: Some(claim.request.correlation_id()),
            ..EventQuery::default()
        },
    );
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
            EventType::LeaseTransitionIntent,
            EventType::LeaseRenewed,
        ]
    );
    assert!(
        events
            .iter()
            .all(|event| event.severity == EventSeverity::Info)
    );
    host.release_host_claim_for_test(&claim, &renewed)
        .expect("release the hold");
    drop(client);
    host.close().expect("close host");
}

#[test]
fn stopped_instance_claims_skip_the_endpoint_and_capacity_checks_other_kinds_keep_them() {
    // Q-2a: emulator control, autostart, resume reconnect and self-check claims are granted on
    // a stopped (pending) instance and with drain capacity; every other kind keeps the
    // bound-endpoint check and business capacity.
    use actingcommand_contract::CapacityThresholds;
    use actingcommand_execution_kernel::{
        DiscoveredInstanceBinding, PendingAdbEndpoint, ResolvedInstanceEndpoint,
    };
    use actingcommand_host_metrics::{
        CapacitySample, CapacityTarget, HostSample, HostSampler, ProcessLoadThresholds,
    };

    struct HardPressureSampler;

    impl HostSampler for HardPressureSampler {
        fn sample_capacity(&mut self, targets: &[CapacityTarget]) -> Vec<CapacitySample> {
            actingcommand_host_metrics::sample_capacity(targets)
                .into_iter()
                .map(|mut sample| {
                    if sample.available_bytes.is_ok() {
                        sample.available_bytes = Ok(0);
                    }
                    sample
                })
                .collect()
        }

        fn sample(
            &mut self,
            _observed_at_unix_ms: u64,
            _owned_processes: &BTreeMap<u32, String>,
            _top_process_count: usize,
            _thresholds: ProcessLoadThresholds,
        ) -> Result<HostSample, &'static str> {
            panic!("capacity-only specification does not enable performance counters")
        }
    }

    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = RuntimeHost::start(
        config(&root).with_capacity_thresholds(CapacityThresholds::default()),
        Arc::new(FakeProvider::one("node.a", instance_id(), state)),
    )
    .expect("runtime host");
    host.replace_capacity_sampler_for_test(Box::new(HardPressureSampler))
        .expect("hard capacity pressure");
    let bound = host
        .replace_instance_endpoint_for_test(
            "node.a",
            Some(ResolvedInstanceEndpoint::Pending(PendingAdbEndpoint::new(
                "127.0.0.1",
                DiscoveredInstanceBinding::new(0, "fixture", "1.0.0", "C:/fixture/MuMuManager.exe"),
            ))),
        )
        .expect("stop node.a");

    let control = host
        .request_host_claim_for_test("node.a", ClaimKind::EmulatorControl)
        .expect("emulator control on a stopped instance under hard pressure");
    let control_token = control.granted.clone().expect("granted at once");
    host.release_host_claim_for_test(&control, &control_token)
        .expect("release the control");
    let refused = host
        .request_host_claim_for_test("node.a", ClaimKind::DirectTaskRun)
        .expect_err("a direct run keeps the bound-endpoint check");
    assert_eq!(refused.code(), "instance_not_running");

    host.replace_instance_endpoint_for_test("node.a", bound)
        .expect("start node.a");
    let selfcheck = host
        .request_host_claim_for_test("node.a", ClaimKind::SelfCheck)
        .expect("a self-check takes drain capacity");
    let selfcheck_token = selfcheck.granted.clone().expect("granted at once");
    host.release_host_claim_for_test(&selfcheck, &selfcheck_token)
        .expect("release the self-check");
    // Review L5 (#369 S2+S3b): a direct run keeps business capacity; refused now, it is queued
    // and written nothing more, and the pump grants it only once capacity recovers.
    let direct = host
        .request_host_claim_for_test("node.a", ClaimKind::DirectTaskRun)
        .expect("a direct run under capacity pressure is queued");
    assert!(
        direct.granted.is_none() && direct.queued.is_some(),
        "{direct:?}"
    );
    host.expire_due_leases_for_test().expect("sweep tick");
    assert_eq!(
        direct
            .wait_for_grant(Duration::from_millis(50))
            .expect("grant slot"),
        None,
        "no grant while capacity refuses"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

#[test]
fn a_queued_runtime_claim_is_granted_when_the_holder_releases() {
    // Q-4, Q-6, Q-7: a claim on a held instance is queued with no deadline and never preempts;
    // the holder's release hands it the lease, and the grant reaches the claimant.
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut owner = TestClient::connect(&host);
    let (_, owner_token) = owner.acquire("node.a");
    let claim = host
        .request_host_claim_for_test("node.a", ClaimKind::StartupPackage)
        .expect("claim on a held instance");
    assert!(claim.granted.is_none());
    let queued = claim.queued.clone().expect("queued");
    assert_eq!(queued.kind(), ClaimKind::StartupPackage);
    assert_eq!(queued.deadline_monotonic_ms(), u64::MAX);
    assert!(!queued.preempt_requested());
    assert_eq!(
        claim
            .wait_for_grant(Duration::from_millis(20))
            .expect("grant slot"),
        None
    );
    let release = owner.request(RuntimeOperation::ReleaseLease { token: owner_token });
    assert_eq!(owner.send(&release).state(), RuntimeReceiptState::Completed);
    let granted = claim
        .wait_for_grant(Duration::from_secs(5))
        .expect("grant slot")
        .expect("granted at the release");
    assert_eq!(
        event_types_for_correlation(&mut owner, claim.request.correlation_id()),
        vec![
            EventType::LeaseRequested,
            EventType::SchedulerQueued,
            EventType::LeaseTransitionIntent,
            EventType::LeaseTransferred,
        ]
    );
    host.release_host_claim_for_test(&claim, &granted)
        .expect("release the claim");
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(owner);
    host.close().expect("close host");
}
