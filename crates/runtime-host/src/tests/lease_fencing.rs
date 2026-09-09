// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn different_instances_acquire_and_execute_independently() {
    let root = TempDir::new().expect("tempdir");
    let state_a = Arc::new(FakeState::default());
    let state_b = Arc::new(FakeState::default());
    let provider = FakeProvider::from_entries([
        ("node.a".to_string(), instance_id(), Arc::clone(&state_a)),
        ("node.c".to_string(), instance_id(), Arc::clone(&state_b)),
    ]);
    let host = RuntimeHost::start(config(&root), Arc::new(provider)).expect("runtime host");
    let first = TestClient::connect(&host);
    let second = TestClient::connect(&host);
    let start = Arc::new(Barrier::new(3));
    let run = |mut client: TestClient, alias: &'static str, start: Arc<Barrier>| {
        thread::spawn(move || {
            start.wait();
            let (_, token) = client.acquire(alias);
            let input = client.request(RuntimeOperation::Input {
                token: token.clone(),
                action: InputAction::Reset,
            });
            assert_eq!(client.send(&input).state(), RuntimeReceiptState::Completed);
            let release = client.request(RuntimeOperation::ReleaseLease { token });
            assert_eq!(
                client.send(&release).state(),
                RuntimeReceiptState::Completed
            );
        })
    };
    let first = run(first, "node.a", Arc::clone(&start));
    let second = run(second, "node.c", Arc::clone(&start));

    start.wait();
    first.join().expect("first instance client");
    second.join().expect("second instance client");
    for state in [&state_a, &state_b] {
        assert_eq!(state.open_count.load(Ordering::Acquire), 1);
        assert_eq!(state.input_count.load(Ordering::Acquire), 1);
        assert_eq!(state.close_count.load(Ordering::Acquire), 1);
    }
    host.close().expect("close host");
    for state in [&state_a, &state_b] {
        assert_eq!(state.close_count.load(Ordering::Acquire), 1);
    }
}

#[test]
fn acquire_idempotency_recovers_its_durable_terminal_without_a_connection_cache() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let request = RuntimeRequest::new(
        ids.mint_request_id().expect("request id"),
        ids.mint_correlation_id().expect("correlation id"),
        None,
        EventActor::Cli,
        EventSource::Cli,
        unix_ms_now().expect("wall clock"),
        RuntimeOperation::acquire_lease("node.a", ids.mint_holder_id().expect("holder id")),
    )
    .expect("runtime request");
    let connection = ConnectionId::new(99).expect("connection id");

    let first = host
        .process_request_for_test(&request, connection)
        .expect("first acquire");
    let repeated = host
        .process_request_for_test(&request, connection)
        .expect("repeated acquire");

    assert_eq!(repeated, first);
    assert!(first.terminal().is_some());
    assert_eq!(state.open_count.load(Ordering::Acquire), 0);

    let query = RuntimeRequest::new(
        ids.mint_request_id().expect("query request id"),
        ids.mint_correlation_id().expect("query correlation id"),
        None,
        EventActor::Cli,
        EventSource::Cli,
        unix_ms_now().expect("wall clock"),
        RuntimeOperation::QueryEvents {
            query: EventQuery {
                request_id: Some(request.request_id()),
                ..EventQuery::default()
            },
            profile: ProjectionProfile::Forensic,
            page: RuntimeEventQueryPageRequest::default(),
        },
    )
    .expect("query request");
    let receipt = host
        .process_request_for_test(&query, connection)
        .expect("query receipt");
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
        ]
    );
    host.close().expect("close host");
}

#[test]
fn renew_and_release_idempotency_survive_connection_cache_loss() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let connection_id = ConnectionId::new(99).expect("connection id");
    let acquire = runtime_request(
        &ids,
        RuntimeOperation::acquire_lease("node.a", ids.mint_holder_id().expect("holder id")),
    );
    let receipt = host
        .process_request_for_test(&acquire, connection_id)
        .expect("acquire");
    let RuntimeResult::LeaseGranted { token } = receipt.result().expect("lease result") else {
        panic!("expected lease grant");
    };

    let renew = runtime_request(
        &ids,
        RuntimeOperation::RenewLease {
            token: token.clone(),
        },
    );
    let first_renew = host
        .process_request_for_test(&renew, connection_id)
        .expect("first renew");
    let repeated_renew = host
        .process_request_for_test(&renew, connection_id)
        .expect("repeated renew");
    assert_eq!(repeated_renew, first_renew);
    assert_eq!(
        event_types_for_request(&host, &ids, connection_id, renew.request_id()),
        vec![
            EventType::SchedulerAdmitted,
            EventType::LeaseTransitionIntent,
            EventType::LeaseRenewed,
        ]
    );
    let RuntimeResult::LeaseRenewed { token } = first_renew.result().expect("renew result") else {
        panic!("expected renewed lease");
    };

    let release = runtime_request(
        &ids,
        RuntimeOperation::ReleaseLease {
            token: token.clone(),
        },
    );
    let first_release = host
        .process_request_for_test(&release, connection_id)
        .expect("first release");
    let repeated_release = host
        .process_request_for_test(&release, connection_id)
        .expect("repeated release");
    assert_eq!(repeated_release, first_release);
    assert_eq!(
        event_types_for_request(&host, &ids, connection_id, release.request_id()),
        vec![
            EventType::SchedulerAdmitted,
            EventType::LeaseTransitionIntent,
            EventType::LeaseReleased,
        ]
    );
    assert_eq!(state.open_count.load(Ordering::Acquire), 0);
    assert_eq!(state.close_count.load(Ordering::Acquire), 0);
    host.close().expect("close host");
}

#[test]
fn connection_drop_revokes_lease_without_opening_the_lazy_backend() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let _ = client.acquire("node.a");
    drop(client);
    assert_eq!(state.open_count.load(Ordering::Acquire), 0);

    let mut replacement = TestClient::connect(&host);
    let (_, token) = replacement.acquire("node.a");
    let release = replacement.request(RuntimeOperation::ReleaseLease { token });
    assert_eq!(
        replacement.send(&release).state(),
        RuntimeReceiptState::Completed
    );
    drop(replacement);
    host.close().expect("close host");
    assert_eq!(state.close_count.load(Ordering::Acquire), 0);
}

#[test]
fn every_fencing_field_is_checked_before_backend_use() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let (_, token) = client.acquire("node.a");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let stale_epoch = LeaseToken::new(
        *ids.mint_owner_epoch().expect("owner epoch").transport(),
        token.lease_id(),
        token.instance_id(),
        token.holder_id(),
        token.expires_at_monotonic_ms(),
    )
    .expect("stale epoch token");
    assert_input_denied(&mut client, stale_epoch, RuntimeErrorCode::StaleOwnerEpoch);

    let wrong_lease = LeaseToken::new(
        token.owner_epoch(),
        *ids.mint_lease_id().expect("lease id").transport(),
        token.instance_id(),
        token.holder_id(),
        token.expires_at_monotonic_ms(),
    )
    .expect("wrong lease token");
    assert_input_denied(&mut client, wrong_lease, RuntimeErrorCode::LeaseMismatch);

    let wrong_instance = LeaseToken::new(
        token.owner_epoch(),
        token.lease_id(),
        *ids.mint_instance_id().expect("instance id").transport(),
        token.holder_id(),
        token.expires_at_monotonic_ms(),
    )
    .expect("wrong instance token");
    assert_input_denied(
        &mut client,
        wrong_instance,
        RuntimeErrorCode::InstanceMismatch,
    );

    let wrong_holder = LeaseToken::new(
        token.owner_epoch(),
        token.lease_id(),
        token.instance_id(),
        *ids.mint_holder_id().expect("holder id").transport(),
        token.expires_at_monotonic_ms(),
    )
    .expect("wrong holder token");
    assert_input_denied(&mut client, wrong_holder, RuntimeErrorCode::HolderMismatch);

    let mut intruder = TestClient::connect(&host);
    let cross_connection = intruder.request(RuntimeOperation::Input {
        token: token.clone(),
        action: InputAction::Tap { x: 10, y: 20 },
    });
    let receipt = intruder.send(&cross_connection);
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied);
    assert_eq!(
        receipt.error_projection().expect("denial").code,
        RuntimeErrorCode::ConnectionMismatch
    );
    drop(intruder);
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);

    let release = client.request(RuntimeOperation::ReleaseLease { token });
    client.send(&release);
    drop(client);
    host.close().expect("close host");
}

#[test]
fn expired_unopened_lease_is_reclaimed_before_a_new_grant() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let provider = Arc::new(FakeProvider::one(
        "node.a",
        instance_id(),
        Arc::clone(&state),
    ));
    let host = RuntimeHost::start(
        config(&root).with_scheduler(SchedulerConfig {
            maximum_client_heartbeat_interval_ms: 100,
            takeover_cooldown_ms: 200,
            lease_ttl_ms: 1_000,
            ..SchedulerConfig::default()
        }),
        provider,
    )
    .expect("runtime host");
    let mut first = TestClient::connect(&host);
    let _ = first.acquire("node.a");
    thread::sleep(Duration::from_millis(1_100));
    assert_eq!(state.open_count.load(Ordering::Acquire), 0);
    let mut second = TestClient::connect(&host);
    let (_, token) = second.acquire("node.a");
    let release = second.request(RuntimeOperation::ReleaseLease { token });
    second.send(&release);
    drop(first);
    drop(second);
    host.close().expect("close host");
}
