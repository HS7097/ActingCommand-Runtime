// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #383 AO-7, test row AO7-b (model-ledger-observer v1.4.1 §3.7, §14.2): the host
//! raises `ResourcesChanged` for the policy driver when a lease ends (released, transferred or
//! expired), the mapping the daemon's monitor applied to ledger events (`m:847-856`), and
//! raises nothing for the other lease records (requested, transition intent, granted,
//! renewed).
//!
//! A step's records are committed, and its wake raised, before its receipt is sent, so each
//! step's wakes are taken at once after its receipt. The expiry comes from a sweep and is taken
//! within `GATE_WAIT`, which only bounds a broken run. No performance thread runs, so only the
//! lease steps record anything.

use super::*;

const GATE_WAIT: Duration = Duration::from_secs(30);

fn no_wake() -> Vec<PolicyTrigger> {
    Vec::new()
}

fn lease_released_count(client: &mut TestClient) -> usize {
    projected_events(
        client,
        EventQuery {
            event_type: Some(EventType::LeaseReleased),
            ..EventQuery::default()
        },
    )
    .len()
}

#[test]
fn gate_ao7b_a_lease_end_raises_resources_changed_and_other_lease_records_raise_nothing() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let host = RuntimeHost::start(
        config(&root)
            .with_frame_retention_enabled(false)
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
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    let wakes = host.open_policy_wakes().expect("open the policy wakes");
    let mut first = TestClient::connect(&host);
    let mut second = TestClient::connect(&host);

    // A grant and a renewal raise nothing.
    let (_, granted) = first.acquire("node.a");
    let renew = first.request(RuntimeOperation::RenewLease { token: granted });
    let receipt = first.send(&renew);
    let Some(RuntimeResult::LeaseRenewed { token }) = receipt.result() else {
        panic!("expected a renewed lease, got {:?}", receipt.result());
    };
    let token = token.clone();
    assert_eq!(
        wakes.take(Duration::ZERO),
        no_wake(),
        "lease grant and renewal"
    );

    // An explicit release raises `ResourcesChanged`.
    let release = first.request(RuntimeOperation::ReleaseLease { token });
    assert_eq!(first.send(&release).state(), RuntimeReceiptState::Completed);
    assert_eq!(
        wakes.take(Duration::ZERO),
        vec![PolicyTrigger::ResourcesChanged],
        "lease.released"
    );

    // A queued request (`lease.requested`) and its cancellation raise nothing.
    let _held = first.acquire("node.a");
    assert_eq!(wakes.take(Duration::ZERO), no_wake(), "lease grant");
    let (_, status) = second.queue("node.a", LeasePriority::Normal, 2_000);
    let cancel = second.request(RuntimeOperation::CancelQueuedLease {
        queued_request_id: status.request_id(),
    });
    assert_eq!(second.send(&cancel).state(), RuntimeReceiptState::Cancelled);
    assert_eq!(
        wakes.take(Duration::ZERO),
        no_wake(),
        "lease.requested and its cancellation"
    );

    // A transfer: a high-priority queue takes the idle holder's lease at once; the transfer is
    // the only lease end of the step.
    let released = lease_released_count(&mut second);
    let holder = second.ids.mint_holder_id().expect("holder id");
    let queued = second.request(RuntimeOperation::queue_lease(
        "node.a",
        holder,
        LeaseQueuePolicy::new(LeasePriority::High, 2_000).expect("queue policy"),
    ));
    let receipt = second.send(&queued);
    let Some(RuntimeResult::LeaseGranted { token }) = receipt.result() else {
        panic!("expected an immediate transfer, got {:?}", receipt.result());
    };
    let transferred = token.clone();
    let records = event_types_for_correlation(&mut second, queued.correlation_id());
    assert!(
        records.contains(&EventType::LeaseTransferred),
        "the queue's records: {records:?}"
    );
    assert_eq!(
        lease_released_count(&mut second),
        released,
        "no lease release in the transfer step"
    );
    assert_eq!(
        wakes.take(Duration::ZERO),
        vec![PolicyTrigger::ResourcesChanged],
        "lease.transferred"
    );

    // An expiry: the transferred lease is not renewed, and past its TTL the sweep expires it.
    clock.advance(250);
    host.expire_due_leases_for_test()
        .expect("sweep the due leases");
    assert_eq!(
        wakes.take(GATE_WAIT),
        vec![PolicyTrigger::ResourcesChanged],
        "lease.expired"
    );
    assert_eq!(
        projected_events(
            &mut second,
            EventQuery {
                event_type: Some(EventType::LeaseExpired),
                lease_id: Some(transferred.lease_id()),
                ..EventQuery::default()
            },
        )
        .len(),
        1
    );
    assert_eq!(
        lease_released_count(&mut second),
        released,
        "no lease release in the expiry step"
    );

    drop(first);
    drop(second);
    host.close().expect("close host");
}
