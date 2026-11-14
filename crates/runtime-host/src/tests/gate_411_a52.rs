// SPDX-License-Identifier: AGPL-3.0-only

//! #411 A52 (0.12.x; FLK-07's cause, board #71; register §1.175): a resource-close lease that
//! the instance's queue refuses must not halt the Runtime. A read-only observe holds the
//! instance's admission guard while it captures, so a Runtime claim that arrives meanwhile waits
//! in the instance's queue (Workflow #369 Q-6: it is granted right after the observe). When that
//! capture fails, its close asks for a dedicated resource-close lease, which a waiting entry
//! refuses (`prepare_resource_close`: `lease_transfer_not_safe`, Workflow #369 §5.1); the close
//! path turned the refusal into the Runtime's fatal (`host_halted=true`). Requirements of the
//! slice brief:
//!
//! - T1 (host 171): that refusal fails only the observe, as a capture failure; it is recorded
//!   once with the scheduler's code, the instance is not fenced (no close was attempted, so
//!   nothing is unconfirmed; Workflow #403 fences an unconfirmed close), the waiting entry gets
//!   the instance and its lease end closes the session that must close, under its fenced close
//!   (`read-session-resource-close.md`, "Device session lifetime"), and the host keeps answering;
//! - T2 (host 172): a close on the same path refused for a reason of the instance's own key.
//!   Before Workflow #456 the contract kept it fatal and T2 pinned that the host halted; since
//!   #456 (register §1.364) the refusal of a close under a lapsed lease fences only the
//!   instance, and T2 pins that instead.
//!
//! FLK-07 met the refusal only when a claim happened to arrive during one of its observes; no
//! test queued one while an observe held the instance. Each test holds the failing capture inside
//! its observe (the fake's capture hold) until the condition is in place. Every wait returns as
//! soon as what it waits for is recorded; the bound only stops a broken run.

use super::gate_403::{fence_records, status_fence};
use super::instance_workers::{all_events, phases};
use super::*;
use actingcommand_contract::{ResourceQuiescence, RuntimeLifecyclePhase, RuntimePayload};
use actingcommand_scheduler::ClaimKind;
use actingcommand_scheduler::codes::SchedulerCode;

const WAIT: Duration = Duration::from_secs(30);
const ALIAS: &str = "a52.instance";

/// A host of the one fixture instance `ALIAS` (`instance`, its fake `state`).
fn start_host(
    instance: InstanceId,
    state: &Arc<FakeState>,
    config: RuntimeHostConfig,
) -> RuntimeHost {
    RuntimeHost::start(
        config,
        Arc::new(FakeProvider::one(ALIAS, instance, Arc::clone(state))),
    )
    .expect("runtime host")
}

fn observe_request(client: &TestClient) -> RuntimeRequest {
    client.request(RuntimeOperation::ObserveReadonly {
        instance_alias: ALIAS.to_owned(),
    })
}

/// Whether `request`'s `capture.requested` is recorded: the observe then holds the instance's
/// admission guard, from before that record until after its capture failure's close.
fn capture_requested(host: &RuntimeHost, request: &RuntimeRequest) -> bool {
    !host
        .query_persisted_events_for_test(EventQuery {
            correlation_id: Some(request.correlation_id()),
            event_type: Some(EventType::CaptureRequested),
            ..EventQuery::default()
        })
        .expect("capture requests")
        .is_empty()
}

/// Lets the fake's held capture go when dropped, also when a wait or `during` panics, so the
/// observe's thread ends and the scope can join it.
struct ReleaseHold<'a>(&'a FakeState);

impl Drop for ReleaseHold<'_> {
    fn drop(&mut self) {
        self.0.capture_hold_after.store(0, Ordering::Release);
    }
}

/// One read-only observe of `ALIAS` whose capture fails. The capture waits in the fake while
/// `during` runs, so `during` acts while the observe holds the instance; then the capture fails
/// and the observe ends. Returns the observe's request, its receipt and what `during` returned.
///
/// The capture failure is transient, so the observe's own failure is not fatal and any fatality
/// its receipt carries comes from its close. A fatal device error would make the receipt fatal on
/// its own (`RuntimeHostError::readonly_capture` keeps the error's fatality) while the Runtime
/// latches nothing; the read-only close test in `teardown.rs` injects a transient failure for the
/// same reason.
fn failing_observe_while<T>(
    host: &RuntimeHost,
    state: &FakeState,
    during: impl FnOnce() -> T,
) -> (RuntimeRequest, RuntimeReceipt, T) {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let request = runtime_request(
        &ids,
        RuntimeOperation::ObserveReadonly {
            instance_alias: ALIAS.to_owned(),
        },
    );
    let done = state.capture_count.load(Ordering::Acquire);
    assert!(
        done > 0,
        "the instance's session is open before the failing observe"
    );
    state.fail_capture.store(true, Ordering::Release);
    state
        .transient_capture_failure
        .store(true, Ordering::Release);
    // The fake's next capture waits while `done` captures are done.
    state.capture_hold_after.store(done, Ordering::Release);
    let (receipt, during) = thread::scope(|scope| {
        let request = &request;
        let observe = scope.spawn(move || {
            host.process_request_for_test(request, ConnectionId::new(171).expect("connection"))
                .expect("observe receipt")
        });
        let hold = ReleaseHold(state);
        wait_until(WAIT, || capture_requested(host, request));
        let during = during();
        drop(hold);
        (observe.join().expect("observe thread"), during)
    });
    (request, receipt, during)
}

/// The quiescence of every resource close of `instance` recorded in `events`, in ledger order.
fn closes(events: &[PersistedEvent], instance: InstanceId) -> Vec<ResourceQuiescence> {
    phases(events)
        .into_iter()
        .filter_map(|(_, phase)| match phase {
            RuntimeLifecyclePhase::ResourceQuiescence {
                instance_id,
                quiescence,
                ..
            } if instance_id == instance => Some(quiescence),
            _ => None,
        })
        .collect()
}

/// T1 (host 171): a person's self-check claim arrives while a failing read-only observe holds the
/// instance and waits in its queue, so the observe's close lease is refused
/// `lease_transfer_not_safe`. The observe fails as a capture failure, not fatal; the Runtime is
/// not fatal; the refusal is one `runtime.failed` record of the scheduler's code under the
/// observe's request, naming the instance; nothing was closed and the instance is not fenced.
/// The self-check is granted once the observe lets the instance go, and its lease end closes the
/// session under its fenced close (one confirmed close). The host then answers the next observe,
/// which opens the instance again.
#[test]
fn a52_t1_a_close_lease_the_queue_refuses_fails_only_the_observe_and_the_next_holder_closes() {
    let root = TempDir::new().expect("tempdir");
    let instance = instance_id();
    let state = Arc::new(FakeState::default());
    // Every capture close must run under a fenced resource-close step.
    state
        .require_fenced_capture_close
        .store(true, Ordering::Release);
    let host = start_host(instance, &state, config(&root));
    let mut client = TestClient::connect(&host);
    let opened = client.send(&observe_request(&client));
    assert_eq!(opened.state(), RuntimeReceiptState::Completed, "{opened:?}");

    let (request, failed, claim) = failing_observe_while(&host, &state, || {
        host.request_host_claim_for_test(ALIAS, ClaimKind::SelfCheck)
            .expect("self-check claim")
    });
    assert!(
        claim.queued.is_some() && claim.granted.is_none(),
        "the claim waited while the observe held the instance: {claim:?}"
    );

    assert_eq!(failed.state(), RuntimeReceiptState::Failed, "{failed:?}");
    let projection = failed.error_projection().expect("observe failure");
    // The failure names what the Runtime latched (HARNESS-DIAG's `host_fatal_context`): the
    // receipt's `host_halted` alone does not say which fatal, or whether one was latched.
    assert!(
        !projection.fatal,
        "the Runtime must not halt: {failed:?}; {}",
        client.host_fatal_context()
    );
    assert_eq!(
        projection.code,
        RuntimeErrorCode::CaptureFailed,
        "{failed:?}"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    let events = all_events(&host);
    let refusals = events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::Failed(record))
                if record.outcome().code().as_str()
                    == SchedulerCode::LeaseTransferNotSafe.as_str() =>
            {
                Some((event, record))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let [(refusal, record)] = refusals.as_slice() else {
        panic!("one recorded refusal, found {}", refusals.len());
    };
    assert_eq!(refusal.links().request_id(), Some(&request.request_id()));
    assert_eq!(
        record
            .lifecycle_failure()
            .and_then(|failure| failure.instance_id()),
        Some(instance)
    );
    assert_eq!(state.capture_close_count.load(Ordering::Acquire), 0);
    assert!(closes(&events, instance).is_empty());
    assert!(fence_records(&events).is_empty());
    assert!(status_fence(&host, ALIAS).is_none());

    let token = claim
        .wait_for_grant(WAIT)
        .expect("grant slot")
        .expect("the waiting self-check is granted after the observe");
    host.release_host_claim_for_test(&claim, &token)
        .expect("the self-check's lease end");
    assert_eq!(state.capture_close_count.load(Ordering::Acquire), 1);
    assert_eq!(
        state.unfenced_capture_close_count.load(Ordering::Acquire),
        0
    );
    assert_eq!(
        closes(&all_events(&host), instance),
        vec![ResourceQuiescence::Confirmed]
    );

    state.fail_capture.store(false, Ordering::Release);
    let next = client.send(&observe_request(&client));
    assert_eq!(next.state(), RuntimeReceiptState::Completed, "{next:?}");
    assert_eq!(state.capture_open_count.load(Ordering::Acquire), 2);
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(client);
    host.close().expect("close host");
}

/// T2 (host 172), adapted by Workflow #456 (register §1.364): a client holds the instance's
/// lease and stops renewing it; the lease runs out while a failing read-only observe holds the
/// instance, so the sweep cannot end it (its expiry scan is held until the observe has answered)
/// and the observe's close reuses it. That close under a lapsed lease is refused `lease_expired`
/// (operation `begin_destructive_resource_close`). Before #456 the refusal halted the Runtime:
/// the observe's failure was fatal, the latched fatal was that refusal, no instance was fenced
/// and the host's close failed. Now it fences only the instance: the observe fails as a capture
/// failure (`capture_failed`, not fatal), nothing is latched, the instance is fenced with
/// `lease_expired` as its cause, the refusal is recorded once under the observe's request, and
/// nothing was closed under the lapsed lease. Once the sweep ends the lapsed lease (Workflow
/// #458: the expiry's own close meets the same lapsed lease and the instance is fenced already),
/// the session is closed once, under a fenced close, after the lease expired, the Runtime still
/// runs and the host closes.
#[test]
fn a52_t2_a_close_under_a_lapsed_lease_fences_only_its_instance() {
    let root = TempDir::new().expect("tempdir");
    let instance = instance_id();
    let state = Arc::new(FakeState::default());
    // Every capture close must run under a fenced resource-close step.
    state
        .require_fenced_capture_close
        .store(true, Ordering::Release);
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let host = start_host(
        instance,
        &state,
        config(&root).with_runtime_clock(clock.clone()),
    );
    let mut client = TestClient::connect(&host);
    let opened = client.send(&observe_request(&client));
    assert_eq!(opened.state(), RuntimeReceiptState::Completed, "{opened:?}");
    let mut holder = TestClient::connect(&host);
    let (_, token) = holder.acquire(ALIAS);
    let scan = host
        .hold_lease_expiry_scan_for_test()
        .expect("hold the lease sweep's expiry scan");

    let (request, failed, ()) = failing_observe_while(&host, &state, || {
        clock.advance(
            token
                .expires_at_monotonic_ms()
                .saturating_sub(clock.monotonic_ms())
                + 1,
        );
    });

    assert_eq!(failed.state(), RuntimeReceiptState::Failed, "{failed:?}");
    let projection = failed.error_projection().expect("observe failure");
    assert!(
        !projection.fatal,
        "the Runtime must not halt: {failed:?}; {}",
        client.host_fatal_context()
    );
    assert_eq!(
        projection.code,
        RuntimeErrorCode::CaptureFailed,
        "{failed:?}"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    let lease_expired = RuntimeErrorCode::LeaseExpired.as_str();
    let fence = status_fence(&host, ALIAS).expect("the instance's fence");
    assert_eq!(fence.cause.as_str(), lease_expired, "{fence:?}");
    let events = all_events(&host);
    let fences = fence_records(&events);
    assert_eq!(fences.len(), 1, "one fence record: {fences:#?}");
    let refusals = events
        .iter()
        .filter(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::Failed(record)) => {
                record.outcome().code().as_str() == lease_expired
                    && event.links().request_id() == Some(&request.request_id())
            }
            _ => false,
        })
        .count();
    assert_eq!(
        refusals, 1,
        "the refusal is recorded once under the observe"
    );
    assert_eq!(
        state.capture_close_count.load(Ordering::Acquire),
        0,
        "nothing was closed under the lapsed lease"
    );
    drop(scan);

    host.expire_due_leases_for_test().expect("the sweep's tick");

    assert!(host.fatal_error().expect("runtime health").is_none());
    let events = all_events(&host);
    let expiries = events
        .iter()
        .filter(|event| {
            event.event_type() == EventType::LeaseExpired
                && event.links().lease_id() == Some(&token.lease_id())
        })
        .map(PersistedEvent::sequence)
        .collect::<Vec<_>>();
    let [expiry] = expiries.as_slice() else {
        panic!("the lapsed lease expired once: {expiries:?}");
    };
    assert_eq!(
        closes(&events, instance),
        vec![ResourceQuiescence::Confirmed]
    );
    let closed_at = phases(&events)
        .into_iter()
        .find_map(|(event, phase)| {
            matches!(phase, RuntimeLifecyclePhase::ResourceQuiescence { instance_id, .. }
                if instance_id == instance)
            .then_some(event.sequence())
        })
        .expect("the session's close");
    assert!(
        closed_at > *expiry,
        "the session is closed after the lease expired"
    );
    assert_eq!(state.capture_close_count.load(Ordering::Acquire), 1);
    assert_eq!(
        state.unfenced_capture_close_count.load(Ordering::Acquire),
        0
    );
    drop(holder);
    drop(client);
    host.close().expect("close host");
}
