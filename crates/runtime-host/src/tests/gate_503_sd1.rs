// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #503 SD-1 (register §1.525; contracts/runtime-interface.md, "A shutdown request that
// finds work in flight"; the coordinator's rulings 1, 2, 4, 5 and 7 on board #503, tests 1 to 4,
// 6 and 7): a shutdown request of the Runtime's own target that finds nothing refusing it but
// lifecycle work in flight on other threads, still in flight when its settle of two seconds ends,
// opens a settling window and is answered `runtime_shutdown_settling` with `retry_after_ms`, its
// Runtime error code `runtime_busy`. While the window is open no new policy cycle begins
// (`begin_policy_work` gives none; a trigger waiting in `wait_policy_work`, as actingd's driver
// waits with one, is kept), the frame cleaner's round gives way, and every other request is
// admitted; the next ask once the work ended is accepted. The window ends by itself after
// `SHUTDOWN_SETTLING_WINDOW_MS` on the Runtime clock, and at once when a shutdown is accepted.
// Every other refusal is answered at once, `runtime_shutdown_denied` naming what refused it as
// `shutdown_obstacle`, and opens no window. The policy cycle in flight is a second thread holding
// `begin_policy_work`, as the driver holds it for a whole cycle; a manual Runtime clock, a fake
// device, no clock read in this file.

use super::*;
use crate::codes::{HostCode, ShutdownObstacle};
use crate::host::{
    SHUTDOWN_SETTLING_RETRY_AFTER_MS, SHUTDOWN_SETTLING_WINDOW_MS, ShutdownBusy, shutdown_busy,
};
use actingcommand_contract::{RuntimeLifecyclePhase, RuntimePayload, RuntimeShutdownDecision};

const ALIAS: &str = "node.a";
/// Bounds a wait that a broken build would never end; every wait returns as soon as what it
/// waits for happened.
const GATE_WAIT: Duration = Duration::from_secs(30);

/// A host of one instance on a manual Runtime clock, and the clock.
fn start() -> (TempDir, RuntimeHost, Arc<ManualRuntimeClock>) {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock.clone()),
        Arc::new(FakeProvider::one(
            ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    (root, host, clock)
}

/// A shutdown request of `target` on the `nth` asking connection, as a client asks it: a new
/// request each time, on a connection no test client holds.
fn ask_for(
    host: &RuntimeHost,
    ids: &IdentifierIssuer,
    target: actingcommand_contract::RuntimeShutdownTarget,
    nth: u64,
) -> RuntimeReceipt {
    let request = runtime_request(ids, RuntimeOperation::RequestShutdown { target });
    let connection = ConnectionId::new(900 + nth).expect("connection");
    host.process_request_for_test(&request, connection)
        .expect("the shutdown receipt")
}

/// A shutdown request of the host's own target.
fn ask(host: &RuntimeHost, ids: &IdentifierIssuer, nth: u64) -> RuntimeReceipt {
    ask_for(host, ids, host.runtime_info().shutdown_target(), nth)
}

/// Runs `inside` while another thread holds a policy cycle in flight, as the policy driver holds
/// `begin_policy_work` for a whole cycle; the cycle ends once `inside` returned.
fn with_a_cycle_in_flight<T>(host: &RuntimeHost, inside: impl FnOnce() -> T) -> T {
    let (held_tx, held_rx) = mpsc::channel();
    let (end_tx, end_rx) = mpsc::channel::<()>();
    thread::scope(|scope| {
        let holder = scope.spawn(move || {
            let cycle = host
                .begin_policy_work()
                .expect("the policy admission")
                .expect("a cycle begins");
            held_tx.send(()).expect("the cycle is in flight");
            let _ = end_rx.recv();
            drop(cycle);
        });
        held_rx
            .recv_timeout(GATE_WAIT)
            .expect("the cycle began on its own thread");
        let answered = inside();
        end_tx.send(()).expect("the cycle ends");
        holder.join().expect("the cycle's thread");
        answered
    })
}

/// Whether a new policy cycle begins now (it ends at once).
fn a_cycle_begins(host: &RuntimeHost) -> bool {
    host.begin_policy_work()
        .expect("the policy admission")
        .is_some()
}

fn assert_settling(receipt: &RuntimeReceipt) {
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{receipt:?}");
    assert!(receipt.terminal().is_some(), "the request is recorded");
    assert_eq!(
        receipt.outcome().code().as_str(),
        HostCode::RuntimeShutdownSettling.as_str(),
        "{receipt:?}"
    );
    assert_eq!(
        receipt
            .outcome()
            .value("retry_after_ms")
            .and_then(serde_json::Value::as_u64),
        Some(SHUTDOWN_SETTLING_RETRY_AFTER_MS),
        "{receipt:?}"
    );
    let projection = receipt.error_projection().expect("the refusal");
    assert_eq!(projection.code, RuntimeErrorCode::RuntimeBusy);
    assert_eq!(
        projection.retry_after_ms,
        Some(SHUTDOWN_SETTLING_RETRY_AFTER_MS)
    );
    assert!(!projection.fatal);
}

fn assert_refused_busy(receipt: &RuntimeReceipt, obstacle: ShutdownObstacle) {
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{receipt:?}");
    assert_eq!(
        receipt.outcome().code().as_str(),
        HostCode::RuntimeShutdownDenied.as_str(),
        "{receipt:?}"
    );
    assert_eq!(
        receipt
            .outcome()
            .value("shutdown_obstacle")
            .and_then(serde_json::Value::as_str),
        Some(obstacle.as_str()),
        "{receipt:?}"
    );
    let projection = receipt.error_projection().expect("the refusal");
    assert_eq!(projection.code, RuntimeErrorCode::RuntimeBusy);
    assert_eq!(projection.retry_after_ms, None, "{receipt:?}");
}

fn assert_accepted(receipt: &RuntimeReceipt) {
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Admitted,
        "{receipt:?}"
    );
    assert!(
        matches!(
            receipt.result(),
            Some(RuntimeResult::ShutdownAccepted { .. })
        ),
        "{receipt:?}"
    );
}

/// The decisions the host recorded for the shutdown requests, in order.
fn shutdown_decisions(host: &RuntimeHost) -> Vec<RuntimeShutdownDecision> {
    host.query_persisted_events_for_test(EventQuery::default())
        .expect("the records")
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::LifecycleObserved(payload)) => {
                match payload.phase() {
                    RuntimeLifecyclePhase::ShutdownRequest { decision, .. } => Some(decision),
                    _ => None,
                }
            }
            _ => None,
        })
        .collect()
}

// Test 1 (rulings 1 and 2): a cycle held in flight past the request's settle: the first request is
// answered `runtime_shutdown_settling` with `retry_after_ms`, recorded `busy`; inside the window no
// new cycle begins, while the held one still runs and after it ended; once it ended the second
// request is accepted, an ordinary accepted shutdown.
#[test]
fn sd1_1_a_cycle_in_flight_is_answered_settling_and_the_second_ask_is_accepted() {
    let (_root, host, _clock) = start();
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let first = with_a_cycle_in_flight(&host, || {
        let first = ask(&host, &ids, 1);
        assert!(
            !a_cycle_begins(&host),
            "no new cycle begins inside the window"
        );
        first
    });
    assert_settling(&first);
    assert!(!host.is_shutdown_requested().expect("running"));
    assert!(
        !a_cycle_begins(&host),
        "the held cycle ended and the window still holds a new one back"
    );

    assert_accepted(&ask(&host, &ids, 2));
    assert!(host.is_shutdown_requested().expect("stopping"));
    assert_eq!(
        shutdown_decisions(&host),
        [
            RuntimeShutdownDecision::Busy,
            RuntimeShutdownDecision::Accepted
        ]
    );
    host.close().expect("close the host");
}

// Test 2 (ruling 4), the one order: each refusal is refused at once whatever else is in flight,
// and only lifecycle work in flight on other threads alone settles.
#[test]
fn sd1_2_each_refusal_is_decided_at_once_and_opens_no_window() {
    let refused = |obstacle| Some(ShutdownBusy::Refused(obstacle));
    for other_work in [false, true] {
        for own_work in [false, true] {
            assert_eq!(
                shutdown_busy(true, None, own_work, other_work),
                refused(ShutdownObstacle::StartPreparing),
                "a start that prepares"
            );
            for obstacle in [
                ShutdownObstacle::Lease,
                ShutdownObstacle::WaitingEntry,
                ShutdownObstacle::ClientReset,
                ShutdownObstacle::PolicyRun,
            ] {
                assert_eq!(
                    shutdown_busy(false, Some(obstacle), own_work, other_work),
                    refused(obstacle),
                    "{obstacle}"
                );
                assert_eq!(
                    shutdown_busy(true, Some(obstacle), own_work, other_work),
                    refused(ShutdownObstacle::StartPreparing),
                    "{obstacle} while the start prepares"
                );
            }
        }
        assert_eq!(
            shutdown_busy(false, None, true, other_work),
            refused(ShutdownObstacle::RequestingThreadWork),
            "the requesting thread's own work"
        );
    }
    assert_eq!(
        shutdown_busy(false, None, false, true),
        Some(ShutdownBusy::Settling),
        "work in flight on other threads alone"
    );
    assert_eq!(shutdown_busy(false, None, false, false), None, "idle");
}

// Test 2 (ruling 4), on the host: with a cycle in flight on another thread, a held lease refuses
// the request at once, `runtime_shutdown_denied` with `lease`, and another owner's target is
// refused `runtime_owner_mismatch`; neither opens a window, so a new cycle still begins.
#[test]
fn sd1_2_a_held_lease_and_another_owner_are_refused_at_once_with_no_window() {
    let (_root, host, _clock) = start();
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let mut client = TestClient::connect(&host);
    let _lease = client.acquire(ALIAS);
    let live = host.runtime_info().shutdown_target();
    let other = actingcommand_contract::RuntimeShutdownTarget {
        pid: live.pid.wrapping_add(1),
        ..live
    };
    with_a_cycle_in_flight(&host, || {
        assert_refused_busy(&ask(&host, &ids, 1), ShutdownObstacle::Lease);
        assert!(a_cycle_begins(&host), "no window opened");
        let mismatch = ask_for(&host, &ids, other, 2);
        assert_eq!(
            mismatch.outcome().code().as_str(),
            HostCode::RuntimeShutdownDenied.as_str(),
            "{mismatch:?}"
        );
        assert_eq!(
            mismatch.error_projection().expect("the refusal").code,
            RuntimeErrorCode::RuntimeOwnerMismatch
        );
        assert!(a_cycle_begins(&host), "no window opened");
    });
    assert!(!host.is_shutdown_requested().expect("running"));
    assert!(!host.retention_gives_way_for_test(), "no window, no stop");
    host.close().expect("close the host");
}

// Test 3 (ruling 5): a window no ask follows ends by itself. A trigger that arrives inside it
// waits in `wait_policy_work` as the driver waits with one; a millisecond before the window's end
// no new cycle begins and the trigger still waits; at its end the trigger runs and cycles go on,
// the Runtime running on as if never asked.
#[test]
fn sd1_3_a_window_no_ask_follows_ends_by_itself_and_the_trigger_it_held_runs() {
    let (_root, host, clock) = start();
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    assert_settling(&with_a_cycle_in_flight(&host, || ask(&host, &ids, 1)));
    let (entered_tx, entered_rx) = mpsc::channel();
    let (began_tx, began_rx) = mpsc::channel();
    thread::scope(|scope| {
        let host = &host;
        let trigger = scope.spawn(move || {
            entered_tx.send(()).expect("the trigger arrived");
            let began = host
                .wait_policy_work()
                .expect("the policy admission")
                .is_some();
            began_tx.send(began).expect("the trigger's cycle");
        });
        entered_rx
            .recv_timeout(GATE_WAIT)
            .expect("the trigger arrived inside the window");
        clock.advance(SHUTDOWN_SETTLING_WINDOW_MS - 1);
        assert!(
            !a_cycle_begins(host),
            "a millisecond before its end the window holds a new cycle back"
        );
        assert!(
            matches!(began_rx.try_recv(), Err(mpsc::TryRecvError::Empty)),
            "the trigger waits inside the window"
        );
        clock.advance(1);
        assert!(
            began_rx
                .recv_timeout(GATE_WAIT)
                .expect("the trigger's cycle"),
            "the trigger kept inside the window runs once it ended"
        );
        trigger.join().expect("the trigger's thread");
    });
    assert!(a_cycle_begins(&host), "cycles go on");
    assert!(!host.retention_gives_way_for_test(), "nothing held back");
    assert!(!host.is_shutdown_requested().expect("running"));
    assert_eq!(shutdown_decisions(&host), [RuntimeShutdownDecision::Busy]);
    host.close().expect("close the host");
}

// Test 4 (rulings 1 and 4): a client's request admitted inside the window that takes a lease makes
// the next ask refused at once, `lease`, as it would be without a window.
#[test]
fn sd1_4_a_lease_taken_inside_the_window_makes_the_next_ask_refused_as_without_one() {
    let (_root, host, _clock) = start();
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let mut client = TestClient::connect(&host);
    let first = with_a_cycle_in_flight(&host, || {
        let first = ask(&host, &ids, 1);
        // Admitted inside the window: the lease is granted (`acquire` asserts the grant).
        let _lease = client.acquire(ALIAS);
        first
    });
    assert_settling(&first);
    assert_refused_busy(&ask(&host, &ids, 2), ShutdownObstacle::Lease);
    assert!(!host.is_shutdown_requested().expect("running"));
    host.close().expect("close the host");
}

// Test 6 (ruling 2): the settling answer reads `runtime_busy` to a client that knows only the wire
// code, as a client of an earlier version reads it: its receipt as sent decodes and validates, and
// its projection is `runtime_busy`, not fatal.
#[test]
fn sd1_6_a_settling_answer_reads_runtime_busy_to_a_client_that_knows_only_the_wire_code() {
    let (_root, host, _clock) = start();
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let first = with_a_cycle_in_flight(&host, || ask(&host, &ids, 1));
    assert_settling(&first);
    let wire = serde_json::to_value(&first).expect("the receipt as sent");
    assert_eq!(wire["state"], "denied", "{wire}");
    assert_eq!(
        wire["outcome"]["values"]["runtime_code"], "runtime_busy",
        "{wire}"
    );
    let read: RuntimeReceipt = serde_json::from_value(wire).expect("a client reads the receipt");
    read.validate().expect("the receipt is valid to a client");
    let projection = read.error_projection().expect("the refusal");
    assert_eq!(projection.code, RuntimeErrorCode::RuntimeBusy);
    assert!(!projection.fatal);
    host.close().expect("close the host");
}

// Test 7 (ruling 7): the frame cleaner's round gives way to a waiting shutdown: its stop is raised
// while a window is open (a tick that began before it ends its round there), lowered once the
// window ended by itself, and raised for good by an accepted shutdown.
#[test]
fn sd1_7_a_retention_round_gives_way_while_a_window_is_open_and_not_after_it() {
    let (_root, host, clock) = start();
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    assert!(!host.retention_gives_way_for_test(), "no window, no stop");
    let first = with_a_cycle_in_flight(&host, || {
        let first = ask(&host, &ids, 1);
        assert!(
            host.retention_gives_way_for_test(),
            "a round of the tick in flight gives way to the waiting shutdown"
        );
        first
    });
    assert_settling(&first);
    assert!(host.retention_gives_way_for_test(), "the window is open");
    clock.advance(SHUTDOWN_SETTLING_WINDOW_MS);
    assert!(a_cycle_begins(&host), "the window ended by itself");
    assert!(
        !host.retention_gives_way_for_test(),
        "a round after the window does not give way"
    );
    assert_accepted(&ask(&host, &ids, 2));
    assert!(host.retention_gives_way_for_test(), "the stop");
    host.close().expect("close the host");
}
