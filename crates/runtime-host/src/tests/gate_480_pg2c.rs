// SPDX-License-Identifier: AGPL-3.0-only

// #480 PG-2c (0.12.73; register §1.472; the coordinator's rulings on board #480, the design
// `rd-1118/design-480-pg2.md`, its slice 3): a pause a person asked for (`pause_mode` `stand`, also
// when the member is absent) closes the gate at once and stands until a person lifts it; at the end
// of its caller's time it answers Completed with `scheduling_pause_standing` (its stage and what is
// in the way) and the host carries its stages on until the instance is quiet, a person resumes or
// the Runtime stops; a shutdown within the caller's time answers `scheduling_pause_kept_at_shutdown`
// (Completed); a fatal error of a stage lifts nothing. A tool's hold (`quiet_or_lift`) is all or
// nothing within its caller's time, as before 0.12.72, for every obstacle, the Runtime's own
// included. The design's inventory, each with its `stand` half and its `quiet_or_lift` half: L1 a
// ladder rung and an automatic start past the grace (the `stand` halves are `gate_480_pg2b`'s (3)
// and `gate_480_pg1`'s (7), adapted), L2 a host package past its checkpoint, L3 a client's entry
// queued during the wait, L4 the close lease outside the multi-Nemu gate (`gate_480_pg2b`'s (6) is
// its `stand` half), L5 a shutdown during the drain, L6 a shutdown during the hand-back, L7 a fatal
// error of a stage; then the absent member, a resume in `draining` and in `paused`, the status a
// reader of `runtime-client` 11 reads, and the commit mark. On the fake device, `gate_480_pg2b`'s
// ladder fixture (`start_climb`) and the scripted emulator of `gate_369_s6c`. Every wait returns as
// soon as what it waits for is seen; `GATE_WAIT` only bounds a broken run.

use super::emulator_controller::EmulatorCall;
use super::gate_369_s6c::{
    START, autostart_host, connectivity, head, instance_scope, resume, send, stop,
};
use super::gate_480_pg2b::{
    Climb, assert_cause, assert_one_record_naming, close_lease_passes_what_the_pause_holds,
    hold_next_rung_step, instance_pause_as, names_instance, pause_records_after, pause_stage,
    start_climb,
};
use super::instance_workers::{wait_for_ladder_end, wait_until};
use super::*;
use crate::codes::{HostCode, HostLocation};
use actingcommand_contract::{InstancePauseStage, SchedulingPauseMode};
use actingcommand_host_metrics::NetworkConnectivity;
use actingcommand_scheduler::ClaimKind;
use serde_json::{Value, json};

/// Test plan v2.2 §2.1 guard 3: the one bound of a cross-thread wait in this file.
const GATE_WAIT: Duration = Duration::from_secs(120);
const STAND: SchedulingPauseMode = SchedulingPauseMode::Stand;
const HOLD: SchedulingPauseMode = SchedulingPauseMode::QuietOrLift;
const CLIENT: &str = "pg2c.client";
const AUTOSTART: &str = "pg2c.autostart";

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

// The pause `request` failed (a tool's hold) with `code`.
#[track_caller]
fn assert_failed_with(
    host: &RuntimeHost,
    receipt: &RuntimeReceipt,
    request: &RuntimeRequest,
    code: &str,
) {
    assert_eq!(receipt.state(), RuntimeReceiptState::Failed, "{receipt:?}");
    let outcome = host
        .failure_outcome_for_test(request.request_id())
        .expect("the refused requests")
        .expect("the pause's failure");
    assert_eq!(outcome.code().as_str(), code, "{outcome:?}");
}

// The pause of `alias` is lifted: no pause, the record that set it and the record without it.
#[track_caller]
fn assert_lifted(host: &RuntimeHost, before: u64, alias: &str) {
    assert_eq!(pause_stage(host, alias), None, "the pause is lifted");
    let records = pause_records_after(host, before);
    let [set, lifted] = &records[..] else {
        panic!("the pause's record and its lift's: {records:?}");
    };
    assert!(names_instance(set, alias), "{set:?}");
    assert!(!names_instance(lifted, alias), "{lifted:?}");
}

// The host's continuations of standing pauses have ended.
fn wait_for_no_continuation(host: &RuntimeHost) {
    wait_until("the continuations end", || {
        host.pause_carry_ons_running_for_test()
            .expect("the continuations")
            == 0
    });
}

// A started host with one physical instance `CLIENT`, every pause's grace already past with
// `zero_grace`.
fn client_host(root: &TempDir, zero_grace: bool) -> RuntimeHost {
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    let config = if zero_grace {
        config(root).with_scheduling_pause_deadlines_for_test(Duration::ZERO)
    } else {
        config(root)
    };
    RuntimeHost::start(
        config,
        Arc::new(FakeProvider::one(CLIENT, instance_id(), state)),
    )
    .expect("runtime host")
}

// A client's lease of `alias` on `connection`.
fn acquire(host: &RuntimeHost, alias: &str, connection: u64) -> LeaseToken {
    let holder = IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_holder_id()
        .expect("holder id");
    let acquired = send(
        host,
        RuntimeOperation::acquire_lease(alias, holder),
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

// Holds the next check of a pause's hand-back that finds its instance's key held, until the
// returned sender is used; the returned receiver hears of it.
fn hold_next_meeting(host: &RuntimeHost) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (met_tx, met_rx) = mpsc::channel::<()>();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    host.run_when_pause_release_meets_a_lease_for_test(Box::new(move || {
        let _ = met_tx.send(());
        let _ = go_rx.recv_timeout(GATE_WAIT);
    }))
    .expect("the hand-back's hook");
    (met_rx, go_tx)
}

// `gate_480_pg2b`'s ladder fixture with its first rung run in flight, held in a capture of the fake
// (`capture_hold_after`) until `let_the_run_go`: the run cannot reach its next checkpoint.
fn climb_with_a_held_rung_run(
    configure: impl FnOnce(RuntimeHostConfig) -> RuntimeHostConfig,
) -> Climb {
    let climb = start_climb(false, configure);
    climb.state.capture_delay_ms.store(400, Ordering::Release);
    climb.fail_scheduled_run();
    wait_until("a rung run on the ladder's key", || {
        let (holder, queued) = climb
            .host
            .instance_claims_for_test(POLICY_INSTANCE_ALIAS)
            .expect("claims");
        holder == Some(ClaimKind::Ladder) && queued.contains(&ClaimKind::LadderContinuation)
    });
    let done = climb.state.capture_count.load(Ordering::Acquire).max(1);
    climb
        .state
        .capture_hold_after
        .store(done, Ordering::Release);
    climb
}

fn let_the_run_go(climb: &Climb) {
    climb.state.capture_hold_after.store(0, Ordering::Release);
}

// L1, `quiet_or_lift` (the twin of `gate_480_pg2b`'s (3)): the ladder holds the instance by its key,
// held just before the application rung's stop, and a tool's hold pauses the instance with its grace
// already past. Before 0.12.72 a ladder was waited for only until the grace; so it is for a tool's
// hold again: `scheduling_pause_release_busy`, lifted; the ladder then climbs on and ends.
#[test]
fn pg2c_l1_a_tools_hold_meeting_a_ladder_past_its_grace_fails_and_is_lifted() {
    let climb = start_climb(false, |config| {
        config.with_scheduling_pause_deadlines_for_test(Duration::ZERO)
    });
    let (at, go) = hold_next_rung_step(&climb.host);
    climb.fail_scheduled_run();
    at.recv_timeout(GATE_WAIT)
        .expect("the application rung is about to stop");
    let before = head(&climb.host);
    let request = instance_pause_as(POLICY_INSTANCE_ALIAS, HOLD);

    let failed = process(&climb.host, &request, 2011);
    go.send(()).expect("the rung goes on");

    assert_failed_with(
        &climb.host,
        &failed,
        &request,
        "scheduling_pause_release_busy",
    );
    assert_lifted(&climb.host, before, POLICY_INSTANCE_ALIAS);
    wait_for_ladder_end(&climb.host, POLICY_INSTANCE_ALIAS);
    assert!(climb.host.fatal_error().expect("runtime health").is_none());
    climb.host.close().expect("close host");
}

// L1, the Runtime's own automatic start, `quiet_or_lift` (the twin of `gate_480_pg1`'s (7) and its
// fix round 1's R4′): an autostart claim keeps its key once its Start was driven, and a tool's hold
// pauses the instance with its grace already past: `scheduling_pause_release_busy`, lifted, as
// before 0.12.72.
#[test]
fn pg2c_l1_a_tools_hold_meeting_an_automatic_start_past_its_grace_fails_and_is_lifted() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance = instance_id();
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let host = autostart_host(&root, &[(AUTOSTART, instance, &state)], read, |config| {
        config.with_scheduling_pause_deadlines_for_test(Duration::ZERO)
    });
    stop(&host, AUTOSTART, 2021);
    state.fail_emulator_start.store(true, Ordering::Release);
    let (driven_tx, driven_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    host.run_after_configured_start_drive_for_test(Box::new(move || {
        let _ = driven_tx.send(());
        let _ = release_rx.recv_timeout(GATE_WAIT);
    }))
    .expect("the claim's hook");
    assert_eq!(host.enqueue_emulator_autostart().expect("enqueue"), 1);
    driven_rx
        .recv_timeout(GATE_WAIT)
        .expect("the configured Start was driven");
    let before = head(&host);
    let request = instance_pause_as(AUTOSTART, HOLD);

    let failed = process(&host, &request, 2022);
    release_tx.send(()).expect("the claim goes on");

    assert_failed_with(&host, &failed, &request, "scheduling_pause_release_busy");
    assert_lifted(&host, before, AUTOSTART);
    wait_until("the claim ends", || {
        host.instance_claims_for_test(AUTOSTART).expect("claims") == (None, vec![])
    });
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// L2, `stand`: the ladder's first rung run, a package the host started itself, is held in a capture
// and cannot reach its next checkpoint; a person pauses the instance with the grace already past.
// The pause answers at once, Completed with `scheduling_pause_standing` (`draining`,
// `contained_run`), and is never lifted. Once the run goes on it stops at its checkpoint, the ladder
// ends at its holding check, and the host's continuation brings the stage to `released` by itself.
#[test]
fn pg2c_l2_a_persons_pause_meeting_a_host_package_past_its_checkpoint_stands_and_settles() {
    let climb = climb_with_a_held_rung_run(|config| {
        config.with_scheduling_pause_deadlines_for_test(Duration::ZERO)
    });
    let before = head(&climb.host);
    let request = instance_pause_as(POLICY_INSTANCE_ALIAS, STAND);

    let paused = process(&climb.host, &request, 2031);

    completed(&paused);
    assert_cause(
        &paused,
        "scheduling_pause_standing",
        "draining",
        Some(&["contained_run"]),
    );
    assert_one_record_naming(&climb.host, before, POLICY_INSTANCE_ALIAS);
    let_the_run_go(&climb);
    wait_for_ladder_end(&climb.host, POLICY_INSTANCE_ALIAS);
    wait_until("the pause reaches released by itself", || {
        pause_stage(&climb.host, POLICY_INSTANCE_ALIAS) == Some(InstancePauseStage::Released)
    });
    assert_one_record_naming(&climb.host, before, POLICY_INSTANCE_ALIAS);
    wait_for_no_continuation(&climb.host);
    assert!(climb.host.fatal_error().expect("runtime health").is_none());
    climb.host.close().expect("close host");
}

// L2, `quiet_or_lift`: the same with a tool's hold: `scheduling_pause_drain_incomplete`, lifted, as
// before 0.12.72; the ladder then climbs on and ends.
#[test]
fn pg2c_l2_a_tools_hold_meeting_a_host_package_past_its_checkpoint_fails_and_is_lifted() {
    let climb = climb_with_a_held_rung_run(|config| {
        config.with_scheduling_pause_deadlines_for_test(Duration::ZERO)
    });
    let before = head(&climb.host);
    let request = instance_pause_as(POLICY_INSTANCE_ALIAS, HOLD);

    let failed = process(&climb.host, &request, 2041);
    let_the_run_go(&climb);

    assert_failed_with(
        &climb.host,
        &failed,
        &request,
        "scheduling_pause_drain_incomplete",
    );
    assert_lifted(&climb.host, before, POLICY_INSTANCE_ALIAS);
    wait_for_ladder_end(&climb.host, POLICY_INSTANCE_ALIAS);
    assert!(climb.host.fatal_error().expect("runtime health").is_none());
    climb.host.close().expect("close host");
}

// A client's lease of `CLIENT` and, behind it, another client's lease queued for at most 1 s.
fn lease_with_a_queued_entry(host: &RuntimeHost) -> LeaseToken {
    let token = acquire(host, CLIENT, 2051);
    let holder = IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_holder_id()
        .expect("holder id");
    let queued = send(
        host,
        RuntimeOperation::queue_lease(
            CLIENT,
            holder,
            LeaseQueuePolicy::new(LeasePriority::Normal, 1_000).expect("queue policy"),
        ),
        2052,
    );
    assert_eq!(queued.state(), RuntimeReceiptState::Queued, "{queued:?}");
    assert_eq!(
        host.instance_claims_for_test(CLIENT).expect("claims"),
        (Some(ClaimKind::ClientLease), vec![ClaimKind::ClientLease])
    );
    token
}

// L3, `stand`: a client's lease holds the instance and another client's entry is queued behind it;
// a person pauses the instance with the grace already past. Before 0.12.73 the queued entry failed
// the pause at once (`lease_transfer_not_safe`) and lifted it; a person's pause waits for it:
// Completed with `scheduling_pause_standing` (`paused`, `lease` and `queued_entry`), never lifted.
// Once the queued entry has expired and the lease is released, the stage becomes `released`.
#[test]
fn pg2c_l3_a_persons_pause_meeting_a_queued_client_entry_stands_and_settles() {
    let root = TempDir::new().expect("tempdir");
    let host = client_host(&root, true);
    let token = lease_with_a_queued_entry(&host);
    let before = head(&host);
    let request = instance_pause_as(CLIENT, STAND);

    let paused = process(&host, &request, 2053);

    completed(&paused);
    assert_cause(
        &paused,
        "scheduling_pause_standing",
        "paused",
        Some(&["lease", "queued_entry"]),
    );
    wait_until("the queued entry expires", || {
        host.instance_claims_for_test(CLIENT)
            .expect("claims")
            .1
            .is_empty()
    });
    assert_eq!(
        pause_stage(&host, CLIENT),
        Some(InstancePauseStage::Paused),
        "the lease is still in the way"
    );
    release(&host, token, 2051);
    wait_until("the pause reaches released by itself", || {
        pause_stage(&host, CLIENT) == Some(InstancePauseStage::Released)
    });
    assert_one_record_naming(&host, before, CLIENT);
    wait_for_no_continuation(&host);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// L3, `quiet_or_lift`: the same with a tool's hold: the queued entry fails it at once,
// `lease_transfer_not_safe`, lifted, as before.
#[test]
fn pg2c_l3_a_tools_hold_meeting_a_queued_client_entry_fails_and_is_lifted() {
    let root = TempDir::new().expect("tempdir");
    let host = client_host(&root, true);
    let token = lease_with_a_queued_entry(&host);
    let before = head(&host);
    let request = instance_pause_as(CLIENT, HOLD);

    let failed = process(&host, &request, 2061);

    assert_failed_with(&host, &failed, &request, "lease_transfer_not_safe");
    assert_lifted(&host, before, CLIENT);
    release(&host, token, 2051);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// L4, `quiet_or_lift` (`gate_480_pg2b`'s (6) is its `stand` half): outside the multi-Nemu gate the
// close lease of a tool's hold passes the startup package that hold holds back too: success,
// `released`, nothing lifted, the package still queued, and after the resume it runs.
#[test]
fn pg2c_l4_a_tools_holds_close_lease_passes_the_startup_package_it_holds_back() {
    close_lease_passes_what_the_pause_holds(HOLD);
}

// L5: a pause of `mode` drains a rung run that is held in a capture (the drain timeout 1 s), and
// the Runtime is asked to stop during the drain. What the pause answered and the stage it is in.
fn shutdown_during_the_drain(
    mode: SchedulingPauseMode,
) -> (Climb, u64, RuntimeReceipt, RuntimeRequest) {
    let climb = climb_with_a_held_rung_run(|config| config);
    let before = head(&climb.host);
    let request = instance_pause_as(POLICY_INSTANCE_ALIAS, mode);
    let answered = thread::scope(|scope| {
        let pausing = scope.spawn(|| process(&climb.host, &request, 2071));
        wait_until("the pause drains", || {
            pause_stage(&climb.host, POLICY_INSTANCE_ALIAS) == Some(InstancePauseStage::Draining)
        });
        climb
            .host
            .request_shutdown_for_test()
            .expect("the Runtime is asked to stop");
        pausing.join().expect("the pause's thread")
    });
    let_the_run_go(&climb);
    (climb, before, answered, request)
}

// L5, `stand`: the person's pause is kept: Completed with `scheduling_pause_kept_at_shutdown`
// (`draining`), still draining, its one record naming the instance; the next start restores it.
#[test]
fn pg2c_l5_a_shutdown_during_a_persons_pauses_drain_keeps_it() {
    let (climb, before, kept, _) = shutdown_during_the_drain(STAND);

    completed(&kept);
    assert_cause(&kept, "scheduling_pause_kept_at_shutdown", "draining", None);
    assert_eq!(
        pause_stage(&climb.host, POLICY_INSTANCE_ALIAS),
        Some(InstancePauseStage::Draining),
        "the pause is kept"
    );
    assert_one_record_naming(&climb.host, before, POLICY_INSTANCE_ALIAS);
    climb.host.close().expect("close the stopping host");
}

// L5, `quiet_or_lift`: a tool's hold is lifted: `scheduling_pause_drain_interrupted`, as before.
#[test]
fn pg2c_l5_a_shutdown_during_a_tools_holds_drain_lifts_it() {
    let (climb, before, failed, request) = shutdown_during_the_drain(HOLD);

    assert_failed_with(
        &climb.host,
        &failed,
        &request,
        "scheduling_pause_drain_interrupted",
    );
    assert_lifted(&climb.host, before, POLICY_INSTANCE_ALIAS);
    climb.host.close().expect("close the stopping host");
}

// L6: a client's lease holds the instance, a pause of `mode` (its caller's time not yet ended)
// waits for it, and the Runtime is asked to stop while it waits.
fn shutdown_during_the_hand_back(
    mode: SchedulingPauseMode,
) -> (TempDir, RuntimeHost, u64, RuntimeReceipt, RuntimeRequest) {
    let root = TempDir::new().expect("tempdir");
    let host = client_host(&root, false);
    let _token = acquire(&host, CLIENT, 2081);
    let (met, go) = hold_next_meeting(&host);
    let before = head(&host);
    let request = instance_pause_as(CLIENT, mode);
    let answered = thread::scope(|scope| {
        let pausing = scope.spawn(|| process(&host, &request, 2082));
        met.recv_timeout(GATE_WAIT)
            .expect("the hand-back meets the client's lease");
        host.request_shutdown_for_test()
            .expect("the Runtime is asked to stop");
        go.send(()).expect("the hand-back goes on");
        pausing.join().expect("the pause's thread")
    });
    (root, host, before, answered, request)
}

// L6, `stand`: the person's pause is kept whatever is in the way: Completed with
// `scheduling_pause_kept_at_shutdown` (`paused`), still `paused`, one record naming the instance;
// the next start restores it (stage `released`).
#[test]
fn pg2c_l6_a_shutdown_during_a_persons_pauses_hand_back_keeps_it() {
    let (root, host, before, kept, _) = shutdown_during_the_hand_back(STAND);

    completed(&kept);
    assert_cause(&kept, "scheduling_pause_kept_at_shutdown", "paused", None);
    assert_eq!(
        pause_stage(&host, CLIENT),
        Some(InstancePauseStage::Paused),
        "the pause is kept"
    );
    assert_one_record_naming(&host, before, CLIENT);
    host.close().expect("close the stopping host");

    let host = client_host(&root, false);
    assert_eq!(
        pause_stage(&host, CLIENT),
        Some(InstancePauseStage::Released),
        "the next start restores the pause"
    );
    host.close().expect("close host");
}

// L6, `quiet_or_lift`: a tool's hold is lifted: `scheduling_pause_release_interrupted`.
#[test]
fn pg2c_l6_a_shutdown_during_a_tools_holds_hand_back_lifts_it() {
    let (_root, host, before, failed, request) = shutdown_during_the_hand_back(HOLD);

    assert_failed_with(
        &host,
        &failed,
        &request,
        "scheduling_pause_release_interrupted",
    );
    assert_lifted(&host, before, CLIENT);
    host.close().expect("close the stopping host");
}

// L7: a client's lease holds the instance, a pause of `mode` waits for it, and the Runtime latches
// a fatal error meanwhile. What the pause answered.
fn fatal_during_the_hand_back(
    mode: SchedulingPauseMode,
) -> (TempDir, RuntimeHost, u64, RuntimeReceipt) {
    let root = TempDir::new().expect("tempdir");
    let host = client_host(&root, false);
    let _token = acquire(&host, CLIENT, 2091);
    let (met, go) = hold_next_meeting(&host);
    let before = head(&host);
    let request = instance_pause_as(CLIENT, mode);
    let answered = thread::scope(|scope| {
        let pausing = scope.spawn(|| process(&host, &request, 2092));
        met.recv_timeout(GATE_WAIT)
            .expect("the hand-back meets the client's lease");
        host.latch_fatal_for_test(RuntimeHostError::fatal(
            HostCode::RuntimeStatePoisoned,
            HostLocation::ReleasePausedInstance,
            RuntimeErrorCode::RuntimeHalted,
        ))
        .expect("the Runtime latches");
        go.send(()).expect("the hand-back goes on");
        pausing.join().expect("the pause's thread")
    });
    (root, host, before, answered)
}

// L7, `stand`: a fatal error of a stage lifts a person's pause no more than anything else: the
// request answers the failure (the Runtime is going down for it), and what "kept" can honestly mean
// is that the gate stays closed and the one record of the held pauses still names the instance, so
// the next start restores it; no lift is recorded.
#[test]
fn pg2c_l7_a_fatal_error_in_a_persons_pauses_stage_lifts_nothing() {
    let (_root, host, before, failed) = fatal_during_the_hand_back(STAND);

    assert_eq!(failed.state(), RuntimeReceiptState::Failed, "{failed:?}");
    assert_eq!(
        pause_stage(&host, CLIENT),
        Some(InstancePauseStage::Paused),
        "the gate stays closed"
    );
    assert_one_record_naming(&host, before, CLIENT);
    assert!(host.close().is_err(), "the Runtime latched");
}

// L7, `quiet_or_lift`: a tool's hold is lifted, as before.
#[test]
fn pg2c_l7_a_fatal_error_in_a_tools_holds_stage_lifts_it() {
    let (_root, host, _, failed) = fatal_during_the_hand_back(HOLD);

    assert_eq!(failed.state(), RuntimeReceiptState::Failed, "{failed:?}");
    assert_eq!(pause_stage(&host, CLIENT), None, "the pause is lifted");
    assert!(host.close().is_err(), "the Runtime latched");
}

// Ruling 1: a pause request without `pause_mode` (the console's of today) is `stand`: a client's
// lease past the grace, which fails a tool's hold, answers `scheduling_pause_standing` here.
#[test]
fn pg2c_an_absent_pause_mode_is_stand() {
    let root = TempDir::new().expect("tempdir");
    let host = client_host(&root, true);
    let token = acquire(&host, CLIENT, 2101);
    let mut wire = serde_json::to_value(instance_pause_as(CLIENT, HOLD)).expect("the request");
    let removed = wire["operation"]
        .as_object_mut()
        .and_then(|operation| operation.remove("pause_mode"));
    assert_eq!(removed, Some(json!("quiet_or_lift")));
    let request: RuntimeRequest = serde_json::from_value(wire).expect("the request without it");
    let RuntimeOperation::PauseScheduling { pause_mode, .. } = request.operation() else {
        panic!("a pause request");
    };
    assert_eq!(*pause_mode, STAND);

    let paused = process(&host, &request, 2102);

    completed(&paused);
    assert_cause(
        &paused,
        "scheduling_pause_standing",
        "paused",
        Some(&["lease"]),
    );
    release(&host, token, 2101);
    wait_until("the pause reaches released by itself", || {
        pause_stage(&host, CLIENT) == Some(InstancePauseStage::Released)
    });
    wait_for_no_continuation(&host);
    host.close().expect("close host");
}

// Ruling 5, in `paused`: a person's pause stands behind a client's lease; a person's resume is
// accepted in stage `paused`: the gate is lifted at once, without a reconnect claim (the client's
// lease still holds the instance) and without a reconnect (`selfcheck` absent); the host's
// continuation ends at its next check.
#[test]
fn pg2c_a_resume_in_paused_lifts_a_persons_pause_without_a_reconnect() {
    let root = TempDir::new().expect("tempdir");
    let host = client_host(&root, true);
    let token = acquire(&host, CLIENT, 2111);
    let before = head(&host);
    let paused = process(&host, &instance_pause_as(CLIENT, STAND), 2112);
    completed(&paused);
    assert_eq!(pause_stage(&host, CLIENT), Some(InstancePauseStage::Paused));

    let resumed = resume(&host, instance_scope(CLIENT), 2113);

    completed(&resumed);
    assert!(
        matches!(
            resumed.result(),
            Some(RuntimeResult::SchedulingResumed {
                selfcheck: None,
                ..
            })
        ),
        "{resumed:?}"
    );
    assert_lifted(&host, before, CLIENT);
    assert_eq!(
        host.instance_claims_for_test(CLIENT).expect("claims"),
        (Some(ClaimKind::ClientLease), vec![]),
        "no reconnect claim"
    );
    wait_for_no_continuation(&host);
    release(&host, token, 2111);
    host.close().expect("close host");
}

// Ruling 5, in `draining`: a person's pause stands behind a rung run held in a capture; a person's
// resume is accepted in stage `draining`: lifted at once, `selfcheck` absent. The run, asked to stop,
// stops at its next checkpoint; the ladder, no longer held, climbs on and ends.
#[test]
fn pg2c_a_resume_in_draining_lifts_a_persons_pause_without_a_reconnect() {
    let climb = climb_with_a_held_rung_run(|config| {
        config.with_scheduling_pause_deadlines_for_test(Duration::ZERO)
    });
    let before = head(&climb.host);
    let paused = process(
        &climb.host,
        &instance_pause_as(POLICY_INSTANCE_ALIAS, STAND),
        2121,
    );
    completed(&paused);
    assert_eq!(
        pause_stage(&climb.host, POLICY_INSTANCE_ALIAS),
        Some(InstancePauseStage::Draining)
    );

    let resumed = resume(&climb.host, instance_scope(POLICY_INSTANCE_ALIAS), 2122);

    completed(&resumed);
    assert!(
        matches!(
            resumed.result(),
            Some(RuntimeResult::SchedulingResumed {
                selfcheck: None,
                ..
            })
        ),
        "{resumed:?}"
    );
    assert_lifted(&climb.host, before, POLICY_INSTANCE_ALIAS);
    let_the_run_go(&climb);
    wait_for_no_continuation(&climb.host);
    wait_for_ladder_end(&climb.host, POLICY_INSTANCE_ALIAS);
    assert!(climb.host.fatal_error().expect("runtime health").is_none());
    climb.host.close().expect("close host");
}

// Ruling 4, a reader of `runtime-client` 11: while a person's pause stands with something in the
// way, the status answers the instance's pause with exactly the members of revision 11, so the
// console pinned to v0.12.71 reads it (no Runtime of 0.12.73 answers `pause_mode` or `in_the_way`;
// a reader of revision 12 reads them: the contract crate's `pg2c_status_pause_members_*`).
#[test]
fn pg2c_the_status_of_a_standing_pause_carries_only_revision_11s_members() {
    let root = TempDir::new().expect("tempdir");
    let host = client_host(&root, true);
    let token = acquire(&host, CLIENT, 2131);
    completed(&process(&host, &instance_pause_as(CLIENT, STAND), 2132));

    let read = send(&host, RuntimeOperation::Status, 2133);

    let Some(RuntimeResult::Status { status }) = read.result() else {
        panic!("a status: {read:?}");
    };
    let encoded = serde_json::to_value(status).expect("the status encodes");
    let pause = encoded["instances"]
        .as_array()
        .and_then(|rows| {
            rows.iter()
                .find(|row| row["instance_alias"] == json!(CLIENT))
        })
        .map(|row| row["pause"].clone())
        .expect("the instance's row");
    let Value::Object(members) = pause else {
        panic!("the instance's pause: {encoded}");
    };
    assert_eq!(
        members.keys().map(String::as_str).collect::<BTreeSet<_>>(),
        BTreeSet::from(["pause_reason", "revision", "since_unix_ms", "stage"]),
        "{encoded}"
    );
    assert_eq!(members["stage"], json!("paused"));
    release(&host, token, 2131);
    wait_for_no_continuation(&host);
    host.close().expect("close host");
}

// Ruling 7, the commit mark: a configured start's last question and its mark are one step; while
// its Start runs (blocked in the fake) the mark is set, and a person's pause recorded then lists the
// automatic start in its way (`scheduling_pause_standing`, `automatic_start`). The pause takes no
// admission guard, and the Start that had begun is finished as before: one provider Start; the
// mark is gone once the claim has ended, and the pause reaches `released` by itself.
#[test]
fn pg2c_a_pause_after_an_automatic_starts_commit_lists_it_and_the_start_is_finished() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance = instance_id();
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let host = autostart_host(&root, &[(AUTOSTART, instance, &state)], read, |config| {
        config.with_scheduling_pause_deadlines_for_test(Duration::ZERO)
    });
    stop(&host, AUTOSTART, 2141);
    // The Start blocks in its readiness wait, which then ends timed out.
    state.block_emulator_start.store(true, Ordering::Release);
    state.fail_emulator_start.store(true, Ordering::Release);
    assert_eq!(host.enqueue_emulator_autostart().expect("enqueue"), 1);
    wait_until("the configured Start", || {
        state.emulator_start_entered.load(Ordering::Acquire)
    });
    assert!(
        host.automatic_start_committed_for_test(AUTOSTART)
            .expect("the mark"),
        "the Start is committed"
    );

    let paused = process(&host, &instance_pause_as(AUTOSTART, STAND), 2142);

    completed(&paused);
    assert_cause(
        &paused,
        "scheduling_pause_standing",
        "paused",
        Some(&["automatic_start"]),
    );
    state.block_emulator_start.store(false, Ordering::Release);
    wait_until("the claim ends", || {
        host.instance_claims_for_test(AUTOSTART).expect("claims") == (None, vec![])
    });
    assert!(
        !host
            .automatic_start_committed_for_test(AUTOSTART)
            .expect("the mark"),
        "the mark ends with the drive"
    );
    let starts = state
        .emulator
        .calls()
        .into_iter()
        .filter(|call| *call == EmulatorCall::Control(START))
        .count();
    assert_eq!(starts, 1, "the Start that had begun was finished");
    wait_until("the pause reaches released by itself", || {
        pause_stage(&host, AUTOSTART) == Some(InstancePauseStage::Released)
    });
    wait_for_no_continuation(&host);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}
