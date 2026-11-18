// SPDX-License-Identifier: AGPL-3.0-only

// #480 PG-2e (0.12.75; register §1.485; coordinator work order
// `rd-1119/brief-480-pg2e-status-answers-the-pause.md`; `contracts/scheduling-pause.md`,
// "Status" and "A pause's life in the ledger"): the status answers each instance pause's
// `pause_mode`, the mode its request set (`stand` for a pause no request set), and before stage
// `released` its `in_the_way`, what the pause's own latest check read, kept with the pause; a status
// request runs no check of its own. And PG-2d's leftover: the continuation's records are decided
// under the persist gate as the request's are, so a lift that takes the gate first leaves them
// unwritten.
//
// - (1) a person's pause whose caller's time ended with a client's lease in the way (the request
//   held by its hook before its record and before its continuation starts): the status answers
//   `stand` and `lease`, the receipt's and the standing record's `in_the_way` word for word; once
//   the lease is gone and the pause `released`, `stand` and nothing in its way;
// - (2) a tool's hold in `draining` behind a rung run held in a capture answers `quiet_or_lift` and
//   what its drain read, `contained_run`; a hold quiet in its time answers `quiet_or_lift`, and the
//   same pause restored at the next start `stand`; a pause held at a first start (`instance.added`)
//   `stand`; an iOS handset's pause, a person's and a tool's, `released` at once with its mode;
//   none of them anything in its way;
// - (3) while the continuation of a standing pause is held inside its check (the hand-back's hook
//   at its meeting with the lease), twenty status reads each answer what that check noted and the
//   count of the hand-back's checks does not move: a read runs no check;
// - (7) a standing pause becomes quiet; its continuation, its stages ended `released`, is held by
//   the hook before the persist gate, and a person's resume lifts the pause first (the reconnect's
//   path): no `scheduling_pause_settled` is written after it, the standing record comes before the
//   lift's record and is the pause's one record of its life.
//
// On the fake device of one physical instance (`gate_480_pg2d`'s fixture, built here), the ladder
// fixture of `gate_480_pg2c` (`climb_with_a_held_rung_run`) and `gate_398_rd1b`'s registration of
// an iOS handset. Every wait returns as soon as what it waits for is seen; `GATE_WAIT` only bounds a
// broken run; no clock is read.

use super::gate_369_s6c::{head, instance_scope, instance_status, resume, send};
use super::gate_398_rd1b::resolved_handset;
use super::gate_480_pg2b::{assert_cause, instance_pause_as, names_instance, pause_stage};
use super::gate_480_pg2c::{climb_with_a_held_rung_run, let_the_run_go};
use super::instance_workers::{all_events, phases, wait_for_ladder_end, wait_until};
use super::*;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    FactValue, InstancePauseStage, InstancePauseState, PauseObstacle, RuntimeLifecyclePhase,
    RuntimePayload, SchedulingPauseMode,
};
use serde_json::json;

/// Test plan v2.2 §2.1 guard 3: the one bound of a cross-thread wait in this file.
const GATE_WAIT: Duration = Duration::from_secs(120);
const STAND: SchedulingPauseMode = SchedulingPauseMode::Stand;
const HOLD: SchedulingPauseMode = SchedulingPauseMode::QuietOrLift;
const CLIENT: &str = "pg2e.client";
const IPHONE: &str = "pg2e.iphone";
/// The runtime fact of the held pauses.
const PAUSE_FACT_KEY: &str = "host.scheduling_pause";
/// How many times test (3) reads the status.
const READS: usize = 20;
/// The codes of the records of a pause's life.
const PAUSE_LIFE: [&str; 5] = [
    "scheduling_pause_standing",
    "scheduling_pause_settled",
    "scheduling_resume_without_reconnect",
    "scheduling_pause_kept_at_shutdown",
    "scheduling_pause_lifted_not_quiet",
];

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

// The instance pause of `alias` as the status answers it.
fn answered(host: &RuntimeHost, alias: &str) -> Option<InstancePauseState> {
    instance_status(host, alias).pause().cloned()
}

// The records of pauses' lives after `after`, in the ledger's order, each with its sequence.
fn life_after(host: &RuntimeHost, after: u64) -> Vec<(u64, Outcome)> {
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
                Some((event.sequence(), outcome))
            }
            _ => None,
        })
        .collect()
}

// The sequence of the first record of the held pauses after `after` that no longer names `alias`:
// the record of its pause's lift.
fn lift_after(host: &RuntimeHost, after: u64, alias: &str) -> Option<u64> {
    all_events(host)
        .iter()
        .filter(|event| event.sequence() > after)
        .find_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::FactRecorded(recorded))
                if recorded.record().key == PAUSE_FACT_KEY =>
            {
                match &recorded.record().value {
                    FactValue::RecordList(rows) if !names_instance(rows, alias) => {
                        Some(event.sequence())
                    }
                    _ => None,
                }
            }
            _ => None,
        })
}

// (1): a person's pause stands behind a client's lease. Held by its hook after its stages ended and
// before its record (so no continuation runs yet), the request's last check is what the status
// answers: `stand`, stage `paused`, `lease`; that reading is the receipt's `in_the_way` and the
// standing record's. Once the lease is released the continuation brings the pause to `released`:
// `stand` and nothing in its way.
#[test]
fn pg2e_1_a_standing_pause_answers_stand_and_the_receipts_reading_then_nothing_once_released() {
    let root = TempDir::new().expect("tempdir");
    let host = client_host(&root, true);
    let token = acquire(&host, 5001);
    let (decided_tx, decided_rx) = mpsc::channel::<()>();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    host.run_before_held_pause_record_for_test(Box::new(move || {
        let _ = decided_tx.send(());
        let _ = go_rx.recv_timeout(GATE_WAIT);
    }))
    .expect("the request's hook");
    let before = head(&host);
    let request = instance_pause_as(CLIENT, STAND);

    let (paused, at_the_end) = thread::scope(|scope| {
        let pausing = scope.spawn(|| process(&host, &request, 5002));
        decided_rx
            .recv_timeout(GATE_WAIT)
            .expect("the caller's time ended");
        let at_the_end = answered(&host, CLIENT).expect("the pause in the status");
        go_tx.send(()).expect("the request goes on");
        (pausing.join().expect("the pause's thread"), at_the_end)
    });

    completed(&paused);
    assert_cause(
        &paused,
        "scheduling_pause_standing",
        "paused",
        Some(&["lease"]),
    );
    assert_eq!(at_the_end.stage, InstancePauseStage::Paused);
    assert_eq!(at_the_end.pause_mode, Some(STAND));
    assert_eq!(at_the_end.in_the_way, [PauseObstacle::Lease]);
    let reading = json!(at_the_end.in_the_way);
    let [cause] = paused.outcome().causes() else {
        panic!("one cause of the completed pause: {paused:?}");
    };
    assert_eq!(cause.outcome().value("in_the_way"), Some(&reading));
    let records = life_after(&host, before);
    let [(_, standing)] = &records[..] else {
        panic!("the standing pause's one record: {records:?}");
    };
    assert_eq!(standing.code().as_str(), "scheduling_pause_standing");
    assert_eq!(standing.value("in_the_way"), Some(&reading));

    release(&host, token, 5001);
    wait_until("the pause reaches released by itself", || {
        pause_stage(&host, CLIENT) == Some(InstancePauseStage::Released)
    });
    let settled = answered(&host, CLIENT).expect("the pause in the status");
    assert_eq!(settled.stage, InstancePauseStage::Released);
    assert_eq!(settled.pause_mode, Some(STAND));
    assert!(settled.in_the_way.is_empty(), "{settled:?}");
    wait_for_no_continuation(&host);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// (2): a tool's hold of an instance whose rung run is held in a capture (it cannot reach its next
// checkpoint) waits in its drain within its caller's time: the status answers it in `draining`,
// `quiet_or_lift`, and what its drain read, the run in flight. The run then goes on and the hold's
// request ends as its stages end; the ladder ends.
#[test]
fn pg2e_2_a_tools_hold_in_draining_answers_its_mode_and_what_its_drain_read() {
    let climb = climb_with_a_held_rung_run(|config| config);
    let request = instance_pause_as(POLICY_INSTANCE_ALIAS, HOLD);

    let draining = thread::scope(|scope| {
        let holding = scope.spawn(|| process(&climb.host, &request, 5011));
        let mut seen = None;
        wait_until("the hold's drain reads the run in flight", || {
            seen = answered(&climb.host, POLICY_INSTANCE_ALIAS);
            seen.as_ref()
                .is_some_and(|pause| !pause.in_the_way.is_empty())
        });
        let_the_run_go(&climb);
        let _ended = holding.join().expect("the hold's thread");
        seen.expect("the hold in the status")
    });

    assert_eq!(draining.stage, InstancePauseStage::Draining);
    assert_eq!(draining.pause_mode, Some(HOLD));
    assert_eq!(draining.in_the_way, [PauseObstacle::ContainedRun]);
    wait_for_ladder_end(&climb.host, POLICY_INSTANCE_ALIAS);
    assert!(climb.host.fatal_error().expect("runtime health").is_none());
    climb.host.close().expect("close host");
}

// (2): a tool's hold quiet within its time is `released` with its mode, `quiet_or_lift`; the next
// start restores it as every persisted pause (no request set it, so no mode in memory) and the
// status answers it `stand`, the mode the Runtime treats it as. Nothing is in the way of either.
#[test]
fn pg2e_2_a_restored_pause_answers_stand_and_nothing_in_its_way() {
    let root = TempDir::new().expect("tempdir");
    let host = client_host(&root, false);
    let held = process(&host, &instance_pause_as(CLIENT, HOLD), 5021);
    completed(&held);
    assert!(held.outcome().causes().is_empty(), "quiet: {held:?}");
    let quiet = answered(&host, CLIENT).expect("the hold in the status");
    assert_eq!(quiet.stage, InstancePauseStage::Released);
    assert_eq!(quiet.pause_mode, Some(HOLD));
    assert!(quiet.in_the_way.is_empty(), "{quiet:?}");
    host.close().expect("close host");

    let host = client_host(&root, false);
    let restored = answered(&host, CLIENT).expect("the restored pause in the status");
    assert_eq!(restored.stage, InstancePauseStage::Released);
    assert_eq!(restored.pause_mode, Some(STAND));
    assert!(restored.in_the_way.is_empty(), "{restored:?}");
    host.close().expect("close host");
}

// (2): an entry that declares `start_paused` is held at the first start that sees it with the
// pause `instance.added`, stage `released`: no request set it, so the status answers `stand`, and
// nothing in its way.
#[test]
fn pg2e_2_a_pause_held_at_a_first_start_answers_stand_and_nothing_in_its_way() {
    let root = TempDir::new().expect("tempdir");
    let instance = instance_id();
    let host = RuntimeHost::start(
        config(&root).with_start_paused(BTreeSet::from([instance])),
        Arc::new(FakeProvider::one(
            CLIENT,
            instance,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");

    let held = answered(&host, CLIENT).expect("the start's hold in the status");
    assert_eq!(held.pause_reason.as_str(), "instance.added");
    assert_eq!(held.stage, InstancePauseStage::Released);
    assert_eq!(held.pause_mode, Some(STAND));
    assert!(held.in_the_way.is_empty(), "{held:?}");
    host.close().expect("close host");
}

// (2): an iOS handset (here not bound) is inert, so its pause is `released` at once in both modes:
// the status answers a person's pause `stand` and a tool's hold `quiet_or_lift`, each with nothing
// in its way; each is then resumed.
#[test]
fn pg2e_2_an_ios_handsets_pause_answers_its_mode_and_nothing_in_its_way() {
    let root = TempDir::new().expect("tempdir");
    let instance = instance_id();
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(
            FakeProvider::one(IPHONE, instance, Arc::new(FakeState::default())).with_resolved_for(
                IPHONE,
                resolved_handset(IPHONE, instance, HandsetEndpoint::Ios(None)),
            ),
        ),
    )
    .expect("runtime host");

    for (mode, connection) in [(STAND, 5031), (HOLD, 5033)] {
        let paused = process(&host, &instance_pause_as(IPHONE, mode), connection);
        completed(&paused);
        assert!(paused.outcome().causes().is_empty(), "quiet: {paused:?}");
        let pause = answered(&host, IPHONE).expect("the handset's pause in the status");
        assert_eq!(pause.stage, InstancePauseStage::Released, "{mode:?}");
        assert_eq!(pause.pause_mode, Some(mode));
        assert!(pause.in_the_way.is_empty(), "{mode:?}: {pause:?}");
        completed(&resume(&host, instance_scope(IPHONE), connection + 1));
    }
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// (3): a person's pause stands behind a client's lease and its continuation is held inside its next
// check that meets the lease (the hand-back's hook, after that check noted what it read). Twenty
// status reads each answer that reading (`stand`, `paused`, `lease`, the pause's state in memory),
// and the count of the hand-back's checks that met the lease does not move: no read ran a check.
#[test]
fn pg2e_3_reading_the_status_runs_no_check_of_a_pause() {
    let root = TempDir::new().expect("tempdir");
    let host = client_host(&root, true);
    let token = acquire(&host, 5041);
    completed(&process(&host, &instance_pause_as(CLIENT, STAND), 5042));
    let (met_tx, met_rx) = mpsc::channel::<()>();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    host.run_when_pause_release_meets_a_lease_for_test(Box::new(move || {
        let _ = met_tx.send(());
        let _ = go_rx.recv_timeout(GATE_WAIT);
    }))
    .expect("the hand-back's hook");
    met_rx
        .recv_timeout(GATE_WAIT)
        .expect("the continuation's check meets the lease");
    let checks = host
        .pause_release_lease_meetings_for_test()
        .expect("the hand-back's checks");

    for _ in 0..READS {
        let pause = answered(&host, CLIENT).expect("the pause in the status");
        assert_eq!(pause.stage, InstancePauseStage::Paused);
        assert_eq!(pause.pause_mode, Some(STAND));
        assert_eq!(pause.in_the_way, [PauseObstacle::Lease]);
    }

    assert_eq!(
        host.pause_release_lease_meetings_for_test()
            .expect("the hand-back's checks"),
        checks,
        "no status read ran a check"
    );
    assert_eq!(
        host.pause_in_the_way_for_test(CLIENT)
            .expect("the pause's state"),
        [PauseObstacle::Lease]
    );
    go_tx.send(()).expect("the continuation goes on");
    release(&host, token, 5041);
    wait_until("the pause reaches released by itself", || {
        pause_stage(&host, CLIENT) == Some(InstancePauseStage::Released)
    });
    wait_for_no_continuation(&host);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// (7), PG-2d's leftover: a person's pause stands behind a client's lease; the lease is released and
// the continuation's stages end `released`, its record held by the hook before the persist gate. A
// person's resume of the `released` pause lifts it first (the reconnect's path, under the persist
// gate). The continuation then finds under the gate that its pause no longer holds and writes no
// `scheduling_pause_settled`: the standing record is the pause's one record of its life, and it
// comes before the lift's record.
#[test]
fn pg2e_7_a_resume_that_takes_the_gate_first_leaves_the_settled_record_unwritten() {
    let root = TempDir::new().expect("tempdir");
    let host = client_host(&root, true);
    let token = acquire(&host, 5051);
    let before = head(&host);
    let paused = process(&host, &instance_pause_as(CLIENT, STAND), 5052);
    completed(&paused);
    assert_cause(
        &paused,
        "scheduling_pause_standing",
        "paused",
        Some(&["lease"]),
    );
    let (decided_tx, decided_rx) = mpsc::channel::<()>();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    host.run_before_held_pause_record_for_test(Box::new(move || {
        let _ = decided_tx.send(());
        let _ = go_rx.recv_timeout(GATE_WAIT);
    }))
    .expect("the continuation's hook");

    release(&host, token, 5051);
    decided_rx
        .recv_timeout(GATE_WAIT)
        .expect("the continuation's stages ended released");
    assert_eq!(
        pause_stage(&host, CLIENT),
        Some(InstancePauseStage::Released)
    );
    completed(&resume(&host, instance_scope(CLIENT), 5053));
    assert_eq!(
        pause_stage(&host, CLIENT),
        None,
        "the resume lifted the pause"
    );
    go_tx.send(()).expect("the continuation goes on");
    wait_for_no_continuation(&host);

    let records = life_after(&host, before);
    let codes = records
        .iter()
        .map(|(_, outcome)| outcome.code().as_str())
        .collect::<Vec<_>>();
    assert_eq!(codes, ["scheduling_pause_standing"], "{records:?}");
    let lifted = lift_after(&host, before, CLIENT).expect("the lift's record");
    assert!(records[0].0 < lifted, "{records:?}, lifted at {lifted}");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}
