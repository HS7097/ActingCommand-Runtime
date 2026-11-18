// SPDX-License-Identifier: AGPL-3.0-only

// #480 PG-2d (0.12.74; register §1.481; coordinator work order
// `rd-1118/brief-480-pg2d-pause-records-resume-cause.md`; `contracts/scheduling-pause.md`, "A
// pause's life in the ledger"): each moment of an instance pause's life that its receipt and its
// stage do not tell is one `runtime.lifecycle_observed` `device_state_observed` record under the
// instance, with the pause request's links, written once at its moment, never per poll or per try:
// a `stand` pause's caller's time ended with something in the way (`scheduling_pause_standing`,
// the receipt's own cause, the same code and members), that pause became quiet by itself
// (`scheduling_pause_settled`), a resume lifted it before it was quiet
// (`scheduling_resume_without_reconnect`, under the pause's links, not the resume's), a shutdown
// kept it (`scheduling_pause_kept_at_shutdown`, on the request's thread and on the continuation's),
// a `quiet_or_lift` hold was not quiet in its time and was lifted
// (`scheduling_pause_lifted_not_quiet`, a warning with the hold's reason, what was in the way and
// the failure as its cause). A pause quiet within its caller's time and a restored pause write
// none; a check that fails and is tried again writes no second record. And PG-2c's leftover (the
// review of #1826's last round, "第 8 条"): a panic of a standing pause's continuation is caught
// where the thread runs and latches the Runtime at once, before any join. On the fake device of
// `gate_480_pg2c`'s client fixture, built here; every wait returns as soon as what it waits for
// is seen, `GATE_WAIT` only bounds a broken run; no clock is read.

use super::gate_369_s6c::{head, instance_scope, resume, send};
use super::gate_480_pg2b::{assert_cause, instance_pause_as, pause_stage};
use super::instance_workers::{all_events, phases, wait_until};
use super::*;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    InstancePauseStage, RequestId, RuntimeLifecyclePhase, SchedulingPauseMode,
};
use serde_json::json;

/// Test plan v2.2 §2.1 guard 3: the one bound of a cross-thread wait in this file.
const GATE_WAIT: Duration = Duration::from_secs(120);
const STAND: SchedulingPauseMode = SchedulingPauseMode::Stand;
const HOLD: SchedulingPauseMode = SchedulingPauseMode::QuietOrLift;
const CLIENT: &str = "pg2d.client";
/// The codes of the records of a pause's life.
const PAUSE_LIFE: [&str; 5] = [
    "scheduling_pause_standing",
    "scheduling_pause_settled",
    "scheduling_resume_without_reconnect",
    "scheduling_pause_kept_at_shutdown",
    "scheduling_pause_lifted_not_quiet",
];

/// One record of a pause's life as the ledger holds it.
#[derive(Debug)]
struct LifeRecord {
    severity: EventSeverity,
    request_id: Option<RequestId>,
    instance_linked: bool,
    outcome: Outcome,
}

/// The records of pauses' lives after `after`, in the ledger's order.
fn pause_life_after(host: &RuntimeHost, after: u64) -> Vec<LifeRecord> {
    let events = all_events(host);
    phases(&events)
        .into_iter()
        .filter(|(event, _)| event.sequence() > after)
        .filter_map(|(event, phase)| match phase {
            RuntimeLifecyclePhase::DeviceStateObserved { outcome }
                if PAUSE_LIFE
                    .iter()
                    .any(|code| outcome.code().as_str() == *code) =>
            {
                Some(LifeRecord {
                    severity: event.severity(),
                    request_id: event.links().request_id().copied(),
                    instance_linked: event.links().instance_id().is_some(),
                    outcome,
                })
            }
            _ => None,
        })
        .collect()
}

/// The codes of `records`, in order.
fn codes(records: &[LifeRecord]) -> Vec<&str> {
    records
        .iter()
        .map(|record| record.outcome.code().as_str())
        .collect()
}

/// The record carries the links of the pause's own request, which name the instance.
#[track_caller]
fn assert_pause_links(record: &LifeRecord, request: &RuntimeRequest) {
    assert_eq!(record.request_id, Some(request.request_id()), "{record:?}");
    assert!(record.instance_linked, "{record:?}");
}

/// Test 2: the record carries the same code and members as the one cause of the completed
/// `receipt`, and the same causes under it.
#[track_caller]
fn assert_receipts_cause(record: &LifeRecord, receipt: &RuntimeReceipt) {
    let [cause] = receipt.outcome().causes() else {
        panic!("one cause of the completed pause: {receipt:?}");
    };
    let cause = cause.outcome();
    assert_eq!(
        record.outcome.code().as_str(),
        cause.code().as_str(),
        "{record:?}"
    );
    for key in ["pause_stage", "in_the_way"] {
        assert_eq!(
            record.outcome.value(key),
            cause.value(key),
            "{key}: {record:?}"
        );
    }
    let under = |outcome: &Outcome| {
        outcome
            .causes()
            .iter()
            .map(|link| link.outcome().code().as_str().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(under(&record.outcome), under(cause), "{record:?}");
}

fn completed(receipt: &RuntimeReceipt) {
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
}

fn process(host: &RuntimeHost, request: &RuntimeRequest, connection: u64) -> RuntimeReceipt {
    host.process_request_for_test(request, ConnectionId::new(connection).expect("connection"))
        .expect("the receipt")
}

// A started host with one physical instance `CLIENT`; with `at_once`, every pause's caller's time
// already past and a failed check tried again at the next poll.
fn client_host(root: &TempDir, at_once: bool) -> RuntimeHost {
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    let config = if at_once {
        config(root)
            .with_scheduling_pause_deadlines_for_test(Duration::ZERO)
            .with_scheduling_pause_retry_for_test(Duration::ZERO)
    } else {
        config(root)
    };
    RuntimeHost::start(
        config,
        Arc::new(FakeProvider::one(CLIENT, instance_id(), state)),
    )
    .expect("runtime host")
}

// A client's lease of `CLIENT` on `connection`.
fn acquire(host: &RuntimeHost, connection: u64) -> LeaseToken {
    let holder = IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_holder_id()
        .expect("holder id");
    let acquired = send(
        host,
        RuntimeOperation::acquire_lease(CLIENT, holder),
        connection,
    );
    let Some(RuntimeResult::LeaseGranted { token }) = acquired.result() else {
        panic!("the client's lease: {acquired:?}");
    };
    token.clone()
}

fn release(host: &RuntimeHost, token: LeaseToken, connection: u64) {
    completed(&send(
        host,
        RuntimeOperation::ReleaseLease { token },
        connection,
    ));
}

// The host's continuations of standing pauses have ended.
fn wait_for_no_continuation(host: &RuntimeHost) {
    wait_until("the continuations end", || {
        host.pause_carry_ons_running_for_test()
            .expect("the continuations")
            == 0
    });
}

// A person's pause stands behind a client's lease: its caller's time ends with the lease in the
// way. One record at that moment, the receipt's own cause (test 2: the same code, `pause_stage`
// `paused`, `in_the_way` `lease`), under the pause request's links, info; nothing more while the
// lease holds the instance. Once the lease is released the host's continuation brings the stage to
// `released` by itself: one more record, `scheduling_pause_settled`, under the same links.
#[test]
fn pg2d_a_standing_pause_records_standing_then_settled_once_each() {
    let root = TempDir::new().expect("tempdir");
    let host = client_host(&root, true);
    let token = acquire(&host, 4001);
    let before = head(&host);
    let request = instance_pause_as(CLIENT, STAND);

    let paused = process(&host, &request, 4002);

    completed(&paused);
    assert_cause(
        &paused,
        "scheduling_pause_standing",
        "paused",
        Some(&["lease"]),
    );
    let records = pause_life_after(&host, before);
    let [standing] = &records[..] else {
        panic!("the standing pause's one record: {records:?}");
    };
    assert_receipts_cause(standing, &paused);
    assert_pause_links(standing, &request);
    assert_eq!(standing.severity, EventSeverity::Info);

    release(&host, token, 4001);
    wait_until("the pause reaches released by itself", || {
        pause_stage(&host, CLIENT) == Some(InstancePauseStage::Released)
    });
    wait_for_no_continuation(&host);
    let records = pause_life_after(&host, before);
    assert_eq!(
        codes(&records),
        ["scheduling_pause_standing", "scheduling_pause_settled"]
    );
    assert_pause_links(&records[1], &request);
    assert_eq!(records[1].severity, EventSeverity::Info);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// A person's pause whose checks fail three times as a failure that is not fatal
// (`fail_next_pause_checks_for_test`), each tried again at the next poll: the first on the
// request's thread, which answers `scheduling_pause_standing` with `check_failed` and that failure
// as its cause, the next two on the continuation's. One standing record, with the receipt's cause
// and the failure under it; no record for any try; one settled record once a check passes.
#[test]
fn pg2d_a_check_tried_again_writes_no_second_record() {
    let root = TempDir::new().expect("tempdir");
    let host = client_host(&root, true);
    host.fail_next_pause_checks_for_test(3)
        .expect("the failing checks");
    let before = head(&host);
    let request = instance_pause_as(CLIENT, STAND);

    let paused = process(&host, &request, 4011);

    completed(&paused);
    assert_cause(
        &paused,
        "scheduling_pause_standing",
        "paused",
        Some(&["check_failed"]),
    );
    wait_until("the pause reaches released by itself", || {
        pause_stage(&host, CLIENT) == Some(InstancePauseStage::Released)
    });
    wait_for_no_continuation(&host);
    let records = pause_life_after(&host, before);
    assert_eq!(
        codes(&records),
        ["scheduling_pause_standing", "scheduling_pause_settled"]
    );
    assert_receipts_cause(&records[0], &paused);
    for record in &records {
        assert_pause_links(record, &request);
    }
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// A person's pause stands behind a client's lease; a person's resume lifts it in stage `paused`
// without a reconnect (`scheduling_resume_without_reconnect` on the resume's receipt). One record of
// that cause, written by the resume under the pause request's links (not the resume's own); the
// continuation ends at its next check and writes nothing (no settled record).
#[test]
fn pg2d_a_resume_before_quiet_records_its_cause_once_under_the_pauses_links() {
    let root = TempDir::new().expect("tempdir");
    let host = client_host(&root, true);
    let token = acquire(&host, 4021);
    let before = head(&host);
    let request = instance_pause_as(CLIENT, STAND);
    completed(&process(&host, &request, 4022));

    let resumed = resume(&host, instance_scope(CLIENT), 4023);

    completed(&resumed);
    let [cause] = resumed.outcome().causes() else {
        panic!("one cause of the resume: {resumed:?}");
    };
    assert_eq!(
        cause.outcome().code().as_str(),
        "scheduling_resume_without_reconnect"
    );
    wait_for_no_continuation(&host);
    let records = pause_life_after(&host, before);
    assert_eq!(
        codes(&records),
        [
            "scheduling_pause_standing",
            "scheduling_resume_without_reconnect"
        ]
    );
    assert_pause_links(&records[1], &request);
    assert_eq!(records[1].severity, EventSeverity::Info);
    assert_eq!(pause_stage(&host, CLIENT), None, "the pause is lifted");
    release(&host, token, 4021);
    host.close().expect("close host");
}

// A person's pause stands behind a client's lease and the Runtime is asked to stop: the
// continuation keeps the pause and writes one record of the code a request answers then,
// `scheduling_pause_kept_at_shutdown` (`paused`), under the pause request's links.
#[test]
fn pg2d_a_shutdown_that_keeps_a_standing_pause_records_it_once() {
    let root = TempDir::new().expect("tempdir");
    let host = client_host(&root, true);
    let _token = acquire(&host, 4031);
    let before = head(&host);
    let request = instance_pause_as(CLIENT, STAND);
    completed(&process(&host, &request, 4032));

    host.request_shutdown_for_test()
        .expect("the Runtime is asked to stop");
    wait_for_no_continuation(&host);

    let records = pause_life_after(&host, before);
    assert_eq!(
        codes(&records),
        [
            "scheduling_pause_standing",
            "scheduling_pause_kept_at_shutdown"
        ]
    );
    assert_eq!(
        records[1].outcome.value("pause_stage"),
        Some(&json!("paused"))
    );
    assert_pause_links(&records[1], &request);
    assert_eq!(
        pause_stage(&host, CLIENT),
        Some(InstancePauseStage::Paused),
        "the pause is kept"
    );
    host.close().expect("close the stopping host");
}

// A person's pause waits for a client's lease within its caller's time (held at its hand-back's
// meeting with the lease) and the Runtime is asked to stop: the request answers
// `scheduling_pause_kept_at_shutdown` (`paused`) and writes one record of that same cause; nothing
// else of the pause is recorded (it never stood, no continuation).
#[test]
fn pg2d_a_shutdown_within_the_callers_time_records_the_answers_cause_once() {
    let root = TempDir::new().expect("tempdir");
    let host = client_host(&root, false);
    let _token = acquire(&host, 4041);
    let (met_tx, met_rx) = mpsc::channel::<()>();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    host.run_when_pause_release_meets_a_lease_for_test(Box::new(move || {
        let _ = met_tx.send(());
        let _ = go_rx.recv_timeout(GATE_WAIT);
    }))
    .expect("the hand-back's hook");
    let before = head(&host);
    let request = instance_pause_as(CLIENT, STAND);

    let kept = thread::scope(|scope| {
        let pausing = scope.spawn(|| process(&host, &request, 4042));
        met_rx
            .recv_timeout(GATE_WAIT)
            .expect("the hand-back meets the client's lease");
        host.request_shutdown_for_test()
            .expect("the Runtime is asked to stop");
        go_tx.send(()).expect("the hand-back goes on");
        pausing.join().expect("the pause's thread")
    });

    completed(&kept);
    assert_cause(&kept, "scheduling_pause_kept_at_shutdown", "paused", None);
    let records = pause_life_after(&host, before);
    let [record] = &records[..] else {
        panic!("the kept pause's one record: {records:?}");
    };
    assert_receipts_cause(record, &kept);
    assert_pause_links(record, &request);
    host.close().expect("close the stopping host");
}

// A tool's hold (`quiet_or_lift`) meets a client's lease past its grace: it fails
// `scheduling_pause_release_busy` and is lifted, as before. One record after the lift,
// `scheduling_pause_lifted_not_quiet`, a warning: the hold's reason, what was in the way (`lease`),
// the failure's own code as its cause, under the hold request's links.
#[test]
fn pg2d_a_tools_hold_not_quiet_in_its_time_records_its_lift_as_a_warning() {
    let root = TempDir::new().expect("tempdir");
    let host = client_host(&root, true);
    let token = acquire(&host, 4051);
    let before = head(&host);
    let request = instance_pause_as(CLIENT, HOLD);

    let failed = process(&host, &request, 4052);

    assert_eq!(failed.state(), RuntimeReceiptState::Failed, "{failed:?}");
    let failure = host
        .failure_outcome_for_test(request.request_id())
        .expect("the refused requests")
        .expect("the hold's failure");
    assert_eq!(failure.code().as_str(), "scheduling_pause_release_busy");
    assert_eq!(pause_stage(&host, CLIENT), None, "the hold is lifted");
    let records = pause_life_after(&host, before);
    let [lifted] = &records[..] else {
        panic!("the lifted hold's one record: {records:?}");
    };
    assert_eq!(
        lifted.outcome.code().as_str(),
        "scheduling_pause_lifted_not_quiet"
    );
    assert_eq!(lifted.severity, EventSeverity::Warning);
    assert_eq!(
        lifted.outcome.value("pause_reason"),
        Some(&json!("pg2b_pause"))
    );
    assert_eq!(lifted.outcome.value("in_the_way"), Some(&json!(["lease"])));
    let [cause] = lifted.outcome.causes() else {
        panic!("the failure as its cause: {lifted:?}");
    };
    assert_eq!(
        cause.outcome().code().as_str(),
        "scheduling_pause_release_busy"
    );
    assert_pause_links(lifted, &request);
    release(&host, token, 4051);
    host.close().expect("close host");
}

// A person's pause with nothing in the way is quiet within its caller's time: Completed with no
// cause, and no record of its life. The next start restores it (stage `released`) and writes none
// either.
#[test]
fn pg2d_a_pause_quiet_in_its_time_and_a_restored_pause_write_none() {
    let root = TempDir::new().expect("tempdir");
    let host = client_host(&root, true);
    let request = instance_pause_as(CLIENT, STAND);

    let paused = process(&host, &request, 4061);

    completed(&paused);
    assert!(paused.outcome().causes().is_empty(), "{paused:?}");
    assert_eq!(
        pause_stage(&host, CLIENT),
        Some(InstancePauseStage::Released)
    );
    host.close().expect("close host");

    let host = client_host(&root, true);
    assert_eq!(
        pause_stage(&host, CLIENT),
        Some(InstancePauseStage::Released),
        "the next start restores the pause"
    );
    let records = pause_life_after(&host, 0);
    assert!(records.is_empty(), "{records:?}");
    host.close().expect("close host");
}

/// The hook that panics in a standing pause's continuation (PG-2c's leftover test).
fn panic_in_the_continuation() {
    panic!("#480 PG-2d test: a panic of a standing pause's continuation");
}

// PG-2c's leftover (fix round 2, item 8; the review of #1826's last round, "第 8 条"): a person's
// pause stands behind a client's lease, and its continuation panics at its next check that meets
// the lease (the hand-back's hook). The panic is caught where the thread runs and latches the Runtime
// at once, `runtime_thread_panicked`: seen while the host still runs, before any join of the
// thread; the pause is not lifted.
#[test]
fn pg2d_a_panic_of_the_continuation_is_caught_where_it_runs_and_latches_at_once() {
    let root = TempDir::new().expect("tempdir");
    let host = client_host(&root, true);
    let _token = acquire(&host, 4071);
    let paused = process(&host, &instance_pause_as(CLIENT, STAND), 4072);
    completed(&paused);
    assert_cause(
        &paused,
        "scheduling_pause_standing",
        "paused",
        Some(&["lease"]),
    );

    host.run_when_pause_release_meets_a_lease_for_test(Box::new(panic_in_the_continuation))
        .expect("the hand-back's hook");

    wait_until("the Runtime latches the continuation's panic", || {
        host.fatal_error().expect("runtime health").is_some()
    });
    let latched = host
        .fatal_error()
        .expect("runtime health")
        .expect("the latched failure");
    assert_eq!(latched.code(), "runtime_thread_panicked", "{latched:?}");
    wait_for_no_continuation(&host);
    assert_eq!(
        pause_stage(&host, CLIENT),
        Some(InstancePauseStage::Paused),
        "the pause is not lifted"
    );
    assert!(host.close().is_err(), "the Runtime latched");
}
