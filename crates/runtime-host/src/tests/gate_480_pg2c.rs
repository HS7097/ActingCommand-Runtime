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
//
// Fix round 1 (coordinator work order `rd-1118/brief-480-pg2c-fix1.md`): on an instance outside the
// multi-Nemu gate (`nemu_host`), item 3: a resume while the continuation waits for an admission
// guard another party holds, while the close runs, and while the stage reaches `released` between
// the resume's stage read and its persist gate; a pause lifted and asked again while the first
// continuation waits for the guard; a shutdown then; a tool's hold whose guard stays taken past its
// grace. Item 4: a check that fails once on each thread, and one that stays failing. Item 6: L4's
// refused close lease, takeover cooldown and unconfirmed close under each mode, and the
// `quiet_or_lift` twins of the shutdowns with only the Runtime's own work in the way.

use super::emulator_controller::EmulatorCall;
use super::gate_369_s6c::{
    START, autostart_host, connectivity, head, instance_scope, resume, send, stop,
};
use super::gate_480_pg1::{FIRST as PG1_FIRST, automatic_claim_takes_the_key_then_shutdown};
use super::gate_480_pg2b::{
    Climb, assert_cause, assert_one_record_naming, close_lease_passes_what_the_pause_holds,
    hold_next_rung_step, instance_pause_as, names_instance, pause_records_after, pause_stage,
    start_climb,
};
use super::instance_workers::{wait_for_ladder_end, wait_until};
use super::*;
use crate::codes::{HostCode, HostLocation};
use actingcommand_contract::{InstancePauseStage, PauseObstacle, SchedulingPauseMode};
use actingcommand_host_metrics::NetworkConnectivity;
use actingcommand_scheduler::{ClaimKind, DEFAULT_MAXIMUM_QUEUE_TIMEOUT_MS, SchedulerError};
use serde_json::{Value, json};

/// Test plan v2.2 §2.1 guard 3: the one bound of a cross-thread wait in this file.
const GATE_WAIT: Duration = Duration::from_secs(120);
const STAND: SchedulingPauseMode = SchedulingPauseMode::Stand;
const HOLD: SchedulingPauseMode = SchedulingPauseMode::QuietOrLift;
const CLIENT: &str = "pg2c.client";
const AUTOSTART: &str = "pg2c.autostart";
/// Fix round 1: the first of two instances that capture through the Nemu IPC (outside the
/// multi-Nemu gate), and the second.
const NEMU: &str = "pg2c.first";
const NEMU_OTHER: &str = "pg2c.second";
/// Fix round 1: the code of a check made to fail (`fail_next_pause_checks_for_test`).
const CHECK_FAILED_FOR_TEST: &str = "pause_check_failed_for_test";

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
    client_host_with(root, |config| {
        if zero_grace {
            config.with_scheduling_pause_deadlines_for_test(Duration::ZERO)
        } else {
            config
        }
    })
}

// `client_host` with its configuration `configure`d.
fn client_host_with(
    root: &TempDir,
    configure: impl FnOnce(RuntimeHostConfig) -> RuntimeHostConfig,
) -> RuntimeHost {
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    RuntimeHost::start(
        configure(config(root)),
        Arc::new(FakeProvider::one(CLIENT, instance_id(), state)),
    )
    .expect("runtime host")
}

// Fix round 1: every pause's grace already past, and a refused close or a failed check tried again
// at the next poll.
fn at_once(config: RuntimeHostConfig) -> RuntimeHostConfig {
    config
        .with_scheduling_pause_deadlines_for_test(Duration::ZERO)
        .with_scheduling_pause_retry_for_test(Duration::ZERO)
}

// Fix round 1: a started host with two instances that capture through the Nemu IPC (`gate_480_pg2b`'s
// (6)), so `NEMU` is outside the multi-Nemu gate and a pause's hand-back closes its device session,
// which a keyless observation opened; its configuration `configure`d. The host and `NEMU`'s fake.
fn nemu_host(
    root: &TempDir,
    configure: impl FnOnce(RuntimeHostConfig) -> RuntimeHostConfig,
) -> (RuntimeHost, Arc<FakeState>) {
    let state = Arc::new(FakeState::default());
    let other_state = Arc::new(FakeState::default());
    for nemu in [&state, &other_state] {
        *nemu.backends.lock().expect("fake backends") =
            Some(("adb_shell_input".to_owned(), "nemu_ipc".to_owned()));
        nemu.physical_task_geometry.store(true, Ordering::Release);
        nemu.transition_capture_after_input
            .store(true, Ordering::Release);
        nemu.observed_open.store(true, Ordering::Release);
    }
    let host = RuntimeHost::start(
        configure(config(root)),
        Arc::new(FakeProvider::from_entries([
            (NEMU.to_owned(), instance_id(), Arc::clone(&state)),
            (NEMU_OTHER.to_owned(), instance_id(), other_state),
        ])),
    )
    .expect("runtime host with two Nemu instances");
    assert!(
        !host
            .keeps_device_session_for_test(NEMU)
            .expect("the session gate"),
        "outside the multi-Nemu gate"
    );
    completed(&send(
        &host,
        RuntimeOperation::ObserveReadonly {
            instance_alias: NEMU.to_owned(),
        },
        3001,
    ));
    assert!(
        host.holds_device_session_for_test(NEMU)
            .expect("the session"),
        "the observation left the session open"
    );
    (host, state)
}

// Fix round 1: holds a standing pause's continuation the first time it finds its instance's
// admission guard taken, until the returned sender is used; the returned receiver hears of it.
fn hold_next_taken_guard(host: &RuntimeHost) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (met_tx, met_rx) = mpsc::channel::<()>();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    host.run_when_pause_continuation_meets_a_taken_guard_for_test(Box::new(move || {
        let _ = met_tx.send(());
        let _ = go_rx.recv_timeout(GATE_WAIT);
    }))
    .expect("the continuation's hook");
    (met_rx, go_tx)
}

// Fix round 1: the code of the one cause of `scheduling_pause_standing`, the one cause of the
// completed pause `receipt`.
#[track_caller]
fn standing_failure_code(receipt: &RuntimeReceipt) -> String {
    let [standing] = receipt.outcome().causes() else {
        panic!("one cause of the completed pause: {receipt:?}");
    };
    let [failure] = standing.outcome().causes() else {
        panic!("the failed check as the cause of scheduling_pause_standing: {receipt:?}");
    };
    failure.outcome().code().as_str().to_owned()
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
    // Fix round 1 (review A, point 8): the rung's run, a package the host started itself, is in the
    // run table before any pause is asked, so a pause's drain finds it in flight.
    wait_until("the rung's run in the run table", || {
        climb
            .host
            .contained_runs_on_for_test(POLICY_INSTANCE_ALIAS)
            .expect("the run table")
            > 0
    });
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

// The connection of the queued entry of `lease_with_a_queued_entry`.
const QUEUED_CONNECTION: u64 = 2052;

// A client's lease of `CLIENT` and, behind it, another client's lease queued for the longest the
// scheduler takes, a minute (`DEFAULT_MAXIMUM_QUEUE_TIMEOUT_MS`; fix round 2: fix round 1's ten
// minutes were past that bound, `lease_queue_timeout_invalid`). Fix round 1: the entry ends only
// when its connection ends (`end_connection_for_test`), so neither half of L3 depends on its
// expiry.
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
            LeaseQueuePolicy::new(LeasePriority::Normal, DEFAULT_MAXIMUM_QUEUE_TIMEOUT_MS)
                .expect("queue policy"),
        ),
        QUEUED_CONNECTION,
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
// Once the queued entry's connection has ended and the lease is released, the stage becomes
// `released`.
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
    host.end_connection_for_test(ConnectionId::new(QUEUED_CONNECTION).expect("connection"))
        .expect("the queued entry's connection ends");
    assert!(
        host.instance_claims_for_test(CLIENT)
            .expect("claims")
            .1
            .is_empty(),
        "the queued entry is gone"
    );
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
    host.end_connection_for_test(ConnectionId::new(QUEUED_CONNECTION).expect("connection"))
        .expect("the queued entry's connection ends");
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

// Ruling 7 (withdrawn by fix round 2, item 6: the commit mark is gone): a configured start asked its
// last question and its Start runs (blocked in the fake), holding the instance's admission guard
// and its claim's lease; a person's pause recorded then finds that lease and lists the automatic
// start in its way (`scheduling_pause_standing`, `automatic_start`), as the lease alone says it
// (`HostShared::automatic_start_hold` names the three lines this rests on). The pause takes no
// admission guard, and the Start that had begun is finished as before: one provider Start; once the
// claim has ended the pause reaches `released` by itself. Adapted in fix round 2: the mark's two
// reads are gone.
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

// Fix round 1, item 3 (a), (b): outside the multi-Nemu gate, a person's pause with its grace
// already past answers at once while the test's thread holds the instance's admission guard (what
// is in the way is read outside the guard: nothing); its continuation then finds the guard taken
// and, held there by the hook, a person resumes the instance. The resume lifts the pause without a
// reconnect (no close had begun); once the continuation goes on it sees the lift at its loop's top
// check and ends, closing nothing: once the guard is free the session is still open and nothing
// was closed.
#[test]
fn pg2c_fix1_3_a_resume_while_the_continuation_waits_for_the_admission_guard_closes_nothing() {
    let root = TempDir::new().expect("tempdir");
    let (host, state) = nemu_host(&root, at_once);
    let admission = host
        .instance_admission_for_test(NEMU)
        .expect("the admission guard");
    let (met, go) = hold_next_taken_guard(&host);
    let closed_before = state.capture_close_count.load(Ordering::Acquire);
    let before = head(&host);
    let taken = admission.lock().expect("the admission guard");

    let paused = process(&host, &instance_pause_as(NEMU, STAND), 3011);
    completed(&paused);
    assert_cause(&paused, "scheduling_pause_standing", "paused", Some(&[]));
    met.recv_timeout(GATE_WAIT)
        .expect("the continuation finds the guard taken");
    let resumed = resume(&host, instance_scope(NEMU), 3012);
    go.send(()).expect("the continuation goes on");

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
    wait_for_no_continuation(&host);
    drop(taken);
    assert_lifted(&host, before, NEMU);
    assert_eq!(
        state.capture_close_count.load(Ordering::Acquire),
        closed_before,
        "nothing was closed"
    );
    assert!(
        host.holds_device_session_for_test(NEMU)
            .expect("the session"),
        "the session is still open"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// Fix round 1, item 3 (b): outside the multi-Nemu gate a person's pause's hand-back closes the
// session the observation left open, and while that close runs (held in it by the close's hook) a
// person resumes the instance. The close has begun, so the resume takes the path of a `released`
// pause's: its reconnect claim waits behind the close lease, and once the close ended the resume
// answers after its reconnect, with `selfcheck`; the reconnect opened the session again.
#[test]
fn pg2c_fix1_3_a_resume_while_the_hand_back_closes_the_session_reconnects() {
    let root = TempDir::new().expect("tempdir");
    let (host, state) = nemu_host(&root, |config| config);
    let (closing_tx, closing_rx) = mpsc::channel::<()>();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    host.run_before_next_resource_close_for_test(Box::new(move || {
        let _ = closing_tx.send(());
        let _ = go_rx.recv_timeout(GATE_WAIT);
    }))
    .expect("the close's hook");
    let opened_before = state.capture_open_count.load(Ordering::Acquire);
    let before = head(&host);
    let request = instance_pause_as(NEMU, STAND);

    let (paused, resumed) = thread::scope(|scope| {
        let pausing = scope.spawn(|| process(&host, &request, 3021));
        closing_rx
            .recv_timeout(GATE_WAIT)
            .expect("the hand-back closes the session");
        let resuming = scope.spawn(|| resume(&host, instance_scope(NEMU), 3022));
        wait_until(
            "the resume's reconnect claim waits behind the close",
            || {
                host.instance_claims_for_test(NEMU)
                    .expect("claims")
                    .1
                    .contains(&ClaimKind::ResumeReconnect)
            },
        );
        go_tx.send(()).expect("the close goes on");
        (
            pausing.join().expect("the pause's thread"),
            resuming.join().expect("the resume's thread"),
        )
    });

    completed(&paused);
    completed(&resumed);
    assert!(
        matches!(
            resumed.result(),
            Some(RuntimeResult::SchedulingResumed {
                selfcheck: Some(_),
                ..
            })
        ),
        "the resume reconnected: {resumed:?}"
    );
    assert_lifted(&host, before, NEMU);
    assert!(
        state.capture_open_count.load(Ordering::Acquire) > opened_before,
        "the reconnect opened the session again"
    );
    wait_for_no_continuation(&host);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// Fix round 1, item 3 (b): a person's pause stands behind a client's lease outside the multi-Nemu
// gate; a person resumes the instance, and between the resume's read of the stage and its persist
// gate (held there by its hook) the lease is released and the continuation brings the stage to
// `released`. Under the persist gate the resume reads `released`, so it takes that pause's path:
// the reconnect claim, the reconnect, `selfcheck` answered.
#[test]
fn pg2c_fix1_3_a_resume_whose_pause_reaches_released_meanwhile_reconnects() {
    let root = TempDir::new().expect("tempdir");
    let (host, state) = nemu_host(&root, at_once);
    let token = acquire(&host, NEMU, 3031);
    let before = head(&host);
    let paused = process(&host, &instance_pause_as(NEMU, STAND), 3032);
    completed(&paused);
    assert_cause(
        &paused,
        "scheduling_pause_standing",
        "paused",
        Some(&["lease"]),
    );
    let (read_tx, read_rx) = mpsc::channel::<()>();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    host.run_before_unquiet_resume_gate_for_test(Box::new(move || {
        let _ = read_tx.send(());
        let _ = go_rx.recv_timeout(GATE_WAIT);
    }))
    .expect("the resume's hook");

    let (resumed, opened_before) = thread::scope(|scope| {
        let resuming = scope.spawn(|| resume(&host, instance_scope(NEMU), 3033));
        read_rx
            .recv_timeout(GATE_WAIT)
            .expect("the resume read the stage");
        release(&host, token, 3031);
        wait_until("the pause reaches released", || {
            pause_stage(&host, NEMU) == Some(InstancePauseStage::Released)
        });
        let opened_before = state.capture_open_count.load(Ordering::Acquire);
        go_tx.send(()).expect("the resume goes on");
        (resuming.join().expect("the resume's thread"), opened_before)
    });

    completed(&resumed);
    assert!(
        matches!(
            resumed.result(),
            Some(RuntimeResult::SchedulingResumed {
                selfcheck: Some(_),
                ..
            })
        ),
        "the resume reconnected: {resumed:?}"
    );
    assert!(
        state.capture_open_count.load(Ordering::Acquire) > opened_before,
        "the reconnect opened the session again"
    );
    assert_lifted(&host, before, NEMU);
    wait_for_no_continuation(&host);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// Fix round 1, item 3 (a), (c): while the test's thread holds the admission guard, a person's
// pause answers at once and its continuation finds the guard taken; held there, the pause is lifted
// and a person pauses the instance again, whose own continuation waits for the guard too. Let go,
// the first continuation sees at its loop's top check that its pause (its revision) is no longer
// held and ends, closing nothing; once the guard is free, only the newer pause's continuation acts:
// it closes the session and its pause, at its own revision, reaches `released`.
#[test]
fn pg2c_fix1_3_a_continuation_of_a_lifted_pause_does_nothing_beside_the_newer_pause() {
    let root = TempDir::new().expect("tempdir");
    let (host, state) = nemu_host(&root, at_once);
    let admission = host
        .instance_admission_for_test(NEMU)
        .expect("the admission guard");
    let (met, go) = hold_next_taken_guard(&host);
    let closed_before = state.capture_close_count.load(Ordering::Acquire);
    let taken = admission.lock().expect("the admission guard");

    completed(&process(&host, &instance_pause_as(NEMU, STAND), 3041));
    met.recv_timeout(GATE_WAIT)
        .expect("the first continuation finds the guard taken");
    completed(&resume(&host, instance_scope(NEMU), 3042));
    let again = process(&host, &instance_pause_as(NEMU, STAND), 3043);
    completed(&again);
    let Some(RuntimeResult::SchedulingPaused { revision, .. }) = again.result() else {
        panic!("the newer pause: {again:?}");
    };
    let revision = *revision;
    go.send(()).expect("the first continuation goes on");
    wait_until("the first continuation ends", || {
        host.pause_carry_ons_running_for_test()
            .expect("the continuations")
            == 1
    });
    assert_eq!(
        state.capture_close_count.load(Ordering::Acquire),
        closed_before,
        "nothing was closed while the guard was taken"
    );
    drop(taken);

    wait_until("the newer pause reaches released", || {
        pause_stage(&host, NEMU) == Some(InstancePauseStage::Released)
    });
    wait_for_no_continuation(&host);
    assert_eq!(
        host.scheduling_pauses_for_test()
            .expect("the held pauses")
            .1
            .get(NEMU)
            .map(|pause| pause.revision),
        Some(revision),
        "the newer pause"
    );
    assert!(
        state.capture_close_count.load(Ordering::Acquire) > closed_before,
        "the newer pause's hand-back closed the session"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// Fix round 1, item 3 (a): while the test's thread holds the admission guard, a person's pause
// answers at once and its continuation finds the guard taken; held there, the Runtime is asked to
// stop. The continuation sees the shutdown at its loop's top check and ends while the guard is
// still taken (the join at the shutdown no longer waits for it); the pause is kept, still `paused`,
// its one record naming the instance.
#[test]
fn pg2c_fix1_3_a_shutdown_while_the_continuation_waits_for_the_admission_guard_keeps_the_pause() {
    let root = TempDir::new().expect("tempdir");
    let (host, _state) = nemu_host(&root, at_once);
    let admission = host
        .instance_admission_for_test(NEMU)
        .expect("the admission guard");
    let (met, go) = hold_next_taken_guard(&host);
    let before = head(&host);
    let taken = admission.lock().expect("the admission guard");

    completed(&process(&host, &instance_pause_as(NEMU, STAND), 3051));
    met.recv_timeout(GATE_WAIT)
        .expect("the continuation finds the guard taken");
    host.request_shutdown_for_test()
        .expect("the Runtime is asked to stop");
    go.send(()).expect("the continuation goes on");
    wait_for_no_continuation(&host);

    assert_eq!(
        pause_stage(&host, NEMU),
        Some(InstancePauseStage::Paused),
        "the pause is kept"
    );
    assert_one_record_naming(&host, before, NEMU);
    drop(taken);
    host.close().expect("close the stopping host");
}

// Fix round 1, item 3 (a), ruling 3 of the slice: a tool's hold whose instance's admission guard
// another party holds past its grace waits for it only within its caller's time: it fails
// `scheduling_pause_release_busy` and is lifted (before this round it waited for the guard with no
// bound).
#[test]
fn pg2c_fix1_3_a_tools_hold_whose_admission_guard_stays_taken_past_its_grace_fails_and_is_lifted() {
    let root = TempDir::new().expect("tempdir");
    let (host, _state) = nemu_host(&root, at_once);
    let admission = host
        .instance_admission_for_test(NEMU)
        .expect("the admission guard");
    let before = head(&host);
    let request = instance_pause_as(NEMU, HOLD);

    let failed = {
        let _taken = admission.lock().expect("the admission guard");
        process(&host, &request, 3061)
    };

    assert_failed_with(&host, &failed, &request, "scheduling_pause_release_busy");
    assert_lifted(&host, before, NEMU);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// Fix round 1, item 4, the request's thread: a person's pause whose first check fails as a failure
// of the Runtime that is not fatal and concerns only the operation (`fail_next_pause_checks_for_test`)
// answers, its caller's time past, Completed with `scheduling_pause_standing` (`paused`,
// `check_failed`) and that failure as its cause; the check is tried again, passes, and the stage
// becomes `released`; nothing lifted the pause and the Runtime is not latched.
#[test]
fn pg2c_fix1_4_a_failed_check_on_the_requests_thread_is_tried_again_and_settles() {
    let root = TempDir::new().expect("tempdir");
    let host = client_host_with(&root, at_once);
    host.fail_next_pause_checks_for_test(1)
        .expect("the failing check");
    let before = head(&host);

    let paused = process(&host, &instance_pause_as(CLIENT, STAND), 3101);

    completed(&paused);
    assert_cause(
        &paused,
        "scheduling_pause_standing",
        "paused",
        Some(&["check_failed"]),
    );
    assert_eq!(standing_failure_code(&paused), CHECK_FAILED_FOR_TEST);
    wait_until("the pause reaches released by itself", || {
        pause_stage(&host, CLIENT) == Some(InstancePauseStage::Released)
    });
    assert_one_record_naming(&host, before, CLIENT);
    wait_for_no_continuation(&host);
    assert!(
        host.fatal_error().expect("runtime health").is_none(),
        "the Runtime is not latched"
    );
    host.close().expect("close host");
}

// Fix round 1, item 4, the continuation's thread: a person's pause stands behind a client's lease;
// a check of its continuation fails as above. The pause's state in memory says `check_failed` then
// (held there by the hook); the check is tried again, the lease is released, and the stage becomes
// `released`; nothing lifted the pause and the Runtime is not latched.
#[test]
fn pg2c_fix1_4_a_failed_check_of_the_continuation_is_tried_again_and_settles() {
    let root = TempDir::new().expect("tempdir");
    let host = client_host_with(&root, at_once);
    let token = acquire(&host, CLIENT, 3111);
    let before = head(&host);
    let paused = process(&host, &instance_pause_as(CLIENT, STAND), 3112);
    completed(&paused);
    assert_cause(
        &paused,
        "scheduling_pause_standing",
        "paused",
        Some(&["lease"]),
    );
    let (failed_tx, failed_rx) = mpsc::channel::<()>();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    host.run_after_pause_check_failed_for_test(Box::new(move || {
        let _ = failed_tx.send(());
        let _ = go_rx.recv_timeout(GATE_WAIT);
    }))
    .expect("the failed check's hook");
    host.fail_next_pause_checks_for_test(1)
        .expect("the failing check");

    failed_rx
        .recv_timeout(GATE_WAIT)
        .expect("the continuation's check fails");
    assert_eq!(
        host.pause_in_the_way_for_test(CLIENT)
            .expect("the pause's state"),
        vec![PauseObstacle::CheckFailed],
        "the pause's state says so"
    );
    go_tx.send(()).expect("the continuation goes on");
    release(&host, token, 3111);

    wait_until("the pause reaches released by itself", || {
        pause_stage(&host, CLIENT) == Some(InstancePauseStage::Released)
    });
    assert_one_record_naming(&host, before, CLIENT);
    wait_for_no_continuation(&host);
    assert!(
        host.fatal_error().expect("runtime health").is_none(),
        "the Runtime is not latched"
    );
    host.close().expect("close host");
}

// Fix round 1, item 4: every check of a person's pause fails as above. The pause answers
// `scheduling_pause_standing` (`paused`, `check_failed`) and stands, in stage `paused`, tried
// again; a person's resume lifts it, the continuation ends, and the Runtime is not latched.
#[test]
fn pg2c_fix1_4_a_check_that_keeps_failing_leaves_the_pause_standing_until_a_resume() {
    let root = TempDir::new().expect("tempdir");
    let host = client_host_with(&root, at_once);
    host.fail_next_pause_checks_for_test(u64::MAX)
        .expect("the failing checks");
    let before = head(&host);

    let paused = process(&host, &instance_pause_as(CLIENT, STAND), 3121);

    completed(&paused);
    assert_cause(
        &paused,
        "scheduling_pause_standing",
        "paused",
        Some(&["check_failed"]),
    );
    assert_eq!(
        pause_stage(&host, CLIENT),
        Some(InstancePauseStage::Paused),
        "the pause stands"
    );
    let resumed = resume(&host, instance_scope(CLIENT), 3122);
    completed(&resumed);
    assert_lifted(&host, before, CLIENT);
    wait_for_no_continuation(&host);
    assert!(
        host.fatal_error().expect("runtime health").is_none(),
        "the Runtime is not latched"
    );
    host.fail_next_pause_checks_for_test(0)
        .expect("the checks pass again");
    host.close().expect("close host");
}

// Fix round 1, item 6, L4: outside the multi-Nemu gate a pause of `mode` with its grace past whose
// hand-back's close lease is refused as the scheduler refuses it (`refusal`, once): the host,
// `NEMU`'s fake and its count of capture closes before the pause, the ledger's head before the
// pause, the pause's receipt and its request.
struct RefusedClose {
    host: RuntimeHost,
    state: Arc<FakeState>,
    closed_before: usize,
    before: u64,
    answered: RuntimeReceipt,
    request: RuntimeRequest,
}

fn refused_close_lease(
    root: &TempDir,
    mode: SchedulingPauseMode,
    refusal: SchedulerError,
) -> RefusedClose {
    let (host, state) = nemu_host(root, at_once);
    host.refuse_hand_back_closes_for_test(vec![refusal])
        .expect("the refused close lease");
    let closed_before = state.capture_close_count.load(Ordering::Acquire);
    let before = head(&host);
    let request = instance_pause_as(NEMU, mode);
    let answered = process(&host, &request, 3201);
    RefusedClose {
        host,
        state,
        closed_before,
        before,
        answered,
        request,
    }
}

// L4, `stand`: a refused close lease closed nothing: Completed with `scheduling_pause_standing`
// (`paused`, `close_refused`); the close is tried again, the refusal has ended, the session is
// closed and the stage becomes `released`, nothing lifted.
#[track_caller]
fn assert_refused_close_stands_and_settles(refusal: SchedulerError) {
    let root = TempDir::new().expect("tempdir");
    let RefusedClose {
        host,
        state,
        closed_before,
        before,
        answered: paused,
        ..
    } = refused_close_lease(&root, STAND, refusal);

    completed(&paused);
    assert_cause(
        &paused,
        "scheduling_pause_standing",
        "paused",
        Some(&["close_refused"]),
    );
    wait_until("the pause reaches released by itself", || {
        pause_stage(&host, NEMU) == Some(InstancePauseStage::Released)
    });
    assert!(
        state.capture_close_count.load(Ordering::Acquire) > closed_before,
        "the close tried again closed the session"
    );
    assert_one_record_naming(&host, before, NEMU);
    wait_for_no_continuation(&host);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// L4, `quiet_or_lift`: a refused close lease fails a tool's hold with the refusal's code and lifts
// it, as before 0.12.72.
#[track_caller]
fn assert_refused_close_fails_and_lifts(refusal: SchedulerError, code: &str) {
    let root = TempDir::new().expect("tempdir");
    let RefusedClose {
        host,
        before,
        answered: failed,
        request,
        ..
    } = refused_close_lease(&root, HOLD, refusal);

    assert_failed_with(&host, &failed, &request, code);
    assert_lifted(&host, before, NEMU);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// The refusal of a close lease by another member's entry ahead of it.
fn entry_ahead() -> SchedulerError {
    SchedulerError::QueueAhead {
        kind: ClaimKind::ClientLease,
        position: 1,
    }
}

// The refusal of a close lease by the device's takeover cooldown.
fn takeover_cooldown() -> SchedulerError {
    SchedulerError::Cooldown {
        retry_after_ms: 1_000,
    }
}

#[test]
fn pg2c_fix1_l4_a_persons_pause_whose_close_lease_is_refused_stands_and_settles() {
    assert_refused_close_stands_and_settles(entry_ahead());
}

#[test]
fn pg2c_fix1_l4_a_persons_pause_meeting_the_takeover_cooldown_stands_and_settles() {
    assert_refused_close_stands_and_settles(takeover_cooldown());
}

#[test]
fn pg2c_fix1_l4_a_tools_hold_whose_close_lease_is_refused_fails_and_is_lifted() {
    assert_refused_close_fails_and_lifts(entry_ahead(), "lease_busy");
}

#[test]
fn pg2c_fix1_l4_a_tools_hold_meeting_the_takeover_cooldown_fails_and_is_lifted() {
    assert_refused_close_fails_and_lifts(takeover_cooldown(), "lease_cooldown");
}

// The failure of a capture close that leaves its connection unconfirmed (`gate_437_g1`'s).
fn unconfirmed_close() -> DeviceError {
    DeviceError::fatal("synthetic capture close left its connection unconfirmed")
        .with_resource_summary(
            actingcommand_device::DeviceResourceQuiescence::Unconfirmed,
            1,
        )
}

// How far an L4 test moves the manual clock to let the close lease an unconfirmed close keeps lapse:
// past the fixture's lease time (5 000 ms).
const CLOSE_LAPSE_MS: u64 = 10_000;

// L4: outside the multi-Nemu gate, on a manual Runtime clock, a pause of `mode` with its grace past
// whose hand-back's close of the session is left unconfirmed: the host, `NEMU`'s fake, the clock,
// the ledger's head before the pause and the pause's receipt.
fn unconfirmed_hand_back(
    root: &TempDir,
    mode: SchedulingPauseMode,
) -> (
    RuntimeHost,
    Arc<FakeState>,
    Arc<ManualRuntimeClock>,
    u64,
    RuntimeReceipt,
) {
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let runtime_clock = Arc::clone(&clock);
    let (host, state) = nemu_host(root, |config| {
        at_once(config).with_runtime_clock(runtime_clock)
    });
    *state
        .capture_close_error
        .lock()
        .expect("capture close error") = Some(unconfirmed_close());
    let before = head(&host);
    let answered = process(&host, &instance_pause_as(NEMU, mode), 3211);
    (host, state, clock, before, answered)
}

// L4, `stand`: the unconfirmed close fenced the instance: Completed with
// `scheduling_pause_standing` (`paused`, `close_unconfirmed`), no continuation (the close is not
// tried again), the pause still `paused`. Once the close lease that close keeps has lapsed, a
// person's resume of the fenced instance takes the reconnect's path (`selfcheck` answered) and
// lifts the pause.
#[test]
fn pg2c_fix1_l4_a_persons_pause_whose_close_is_unconfirmed_stands_until_a_resume_reconnects() {
    let root = TempDir::new().expect("tempdir");
    let (host, state, clock, before, paused) = unconfirmed_hand_back(&root, STAND);

    completed(&paused);
    assert_cause(
        &paused,
        "scheduling_pause_standing",
        "paused",
        Some(&["close_unconfirmed"]),
    );
    assert_eq!(
        host.pause_carry_ons_running_for_test()
            .expect("the continuations"),
        0,
        "the close is not tried again"
    );
    assert_eq!(
        pause_stage(&host, NEMU),
        Some(InstancePauseStage::Paused),
        "the pause stands"
    );
    *state
        .capture_close_error
        .lock()
        .expect("capture close error") = None;
    clock.advance(CLOSE_LAPSE_MS);
    host.expire_due_leases_for_test()
        .expect("the sweep's tick that ends the kept close lease");
    let resumed = resume(&host, instance_scope(NEMU), 3212);
    completed(&resumed);
    assert!(
        matches!(
            resumed.result(),
            Some(RuntimeResult::SchedulingResumed {
                selfcheck: Some(_),
                ..
            })
        ),
        "the resume reconnected: {resumed:?}"
    );
    assert_lifted(&host, before, NEMU);
    host.close().expect("close host");
}

// L4, `quiet_or_lift`: the unconfirmed close fails a tool's hold (with the fence's failure) and
// lifts it.
#[test]
fn pg2c_fix1_l4_a_tools_hold_whose_close_is_unconfirmed_fails_and_is_lifted() {
    let root = TempDir::new().expect("tempdir");
    let (host, state, _clock, before, failed) = unconfirmed_hand_back(&root, HOLD);

    assert_eq!(failed.state(), RuntimeReceiptState::Failed, "{failed:?}");
    assert_lifted(&host, before, NEMU);
    *state
        .capture_close_error
        .lock()
        .expect("capture close error") = None;
    host.close().expect("close host");
}

// Fix round 1, item 6, the `quiet_or_lift` twin of `gate_480_pg1`'s fix round 1 R4″: an autostart
// claim keeps its key once its Start was driven, and a tool's hold of the instance, within its
// caller's time, meets it; the Runtime is asked to stop while the hand-back waits for the
// Runtime's own automatic start alone. A tool's hold is lifted then:
// `scheduling_pause_release_interrupted`, the record that set it and the record without it.
#[test]
fn pg2c_fix1_a_shutdown_while_a_tools_hold_waits_for_an_automatic_start_lifts_it() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance = instance_id();
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let host = autostart_host(&root, &[(AUTOSTART, instance, &state)], read, |config| {
        config
    });
    stop(&host, AUTOSTART, 3301);
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
    let (met, go) = hold_next_meeting(&host);
    let before = head(&host);
    let request = instance_pause_as(AUTOSTART, HOLD);

    let failed = thread::scope(|scope| {
        let pausing = scope.spawn(|| process(&host, &request, 3302));
        met.recv_timeout(GATE_WAIT)
            .expect("the hand-back meets the automatic claim");
        host.request_shutdown_for_test()
            .expect("the Runtime is asked to stop");
        go.send(()).expect("the hand-back goes on");
        pausing.join().expect("the pause's thread")
    });
    release_tx.send(()).expect("the claim goes on");

    assert_failed_with(
        &host,
        &failed,
        &request,
        "scheduling_pause_release_interrupted",
    );
    assert_lifted(&host, before, AUTOSTART);
    host.close().expect("close the stopping host");
}

// Fix round 1, item 6, the `quiet_or_lift` twin of `gate_480_pg1`'s fix round 2
// `an_automatic_claim_that_took_the_key_from_a_client…`: at the shutdown the Runtime's own restart
// claim alone holds the instance, which keeps a person's pause; a tool's hold is lifted:
// `scheduling_pause_release_interrupted`.
#[test]
fn pg2c_fix1_a_shutdown_with_an_automatic_claim_that_took_the_key_lifts_a_tools_hold() {
    let (_root, host, before, failed, request) = automatic_claim_takes_the_key_then_shutdown(HOLD);

    assert_failed_with(
        &host,
        &failed,
        &request,
        "scheduling_pause_release_interrupted",
    );
    assert_lifted(&host, before, PG1_FIRST);
    host.close().expect("close the stopping host");
}

// Fix round 1, item 6, the `quiet_or_lift` twin of `gate_480_pg2b`'s (4): the ladder holds the
// instance by its key, held just before the application rung's stop, and a tool's hold of the
// instance, within its caller's time, meets it; the Runtime is asked to stop while the hand-back
// waits for the Runtime's own ladder alone. A tool's hold is lifted then:
// `scheduling_pause_release_interrupted`.
#[test]
fn pg2c_fix1_a_shutdown_while_a_tools_hold_waits_for_a_rung_lifts_it() {
    let climb = start_climb(false, |config| config);
    let (at, rung_go) = hold_next_rung_step(&climb.host);
    climb.fail_scheduled_run();
    at.recv_timeout(GATE_WAIT)
        .expect("the application rung is about to stop");
    let (met, go) = hold_next_meeting(&climb.host);
    let before = head(&climb.host);
    let request = instance_pause_as(POLICY_INSTANCE_ALIAS, HOLD);

    let failed = thread::scope(|scope| {
        let pausing = scope.spawn(|| process(&climb.host, &request, 3311));
        met.recv_timeout(GATE_WAIT)
            .expect("the hand-back meets the ladder's key");
        climb
            .host
            .request_shutdown_for_test()
            .expect("the Runtime is asked to stop");
        go.send(()).expect("the hand-back goes on");
        pausing.join().expect("the pause's thread")
    });
    rung_go.send(()).expect("the rung goes on");

    assert_failed_with(
        &climb.host,
        &failed,
        &request,
        "scheduling_pause_release_interrupted",
    );
    assert_lifted(&climb.host, before, POLICY_INSTANCE_ALIAS);
    climb.host.close().expect("close the stopping host");
}

// Fix round 2 (coordinator work order `rd-1118/brief-480-pg2c-fix2.md`): item 4, a person's resume
// seen by a request that waits for the admission guard; item 5, the hand-back's question under the
// pause table's lock pinned against a resume that lifted the pause and one that is lifting it; item
// 6, the `quiet_or_lift` half of the commit mark's test. Each receipt goes through the host's own
// receipt builder (`process`).

// Holds the next `stand` pause's request that finds its instance's admission guard taken with time
// left, until the returned sender is used; the returned receiver hears of it.
fn hold_next_request_wait(host: &RuntimeHost) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (met_tx, met_rx) = mpsc::channel::<()>();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    host.run_when_pause_request_meets_a_taken_guard_for_test(Box::new(move || {
        let _ = met_tx.send(());
        let _ = go_rx.recv_timeout(GATE_WAIT);
    }))
    .expect("the request's hook");
    (met_rx, go_tx)
}

// Holds the next `stand` pause's hand-back that is about to ask whether it may close the session,
// until the returned sender is used; the returned receiver hears of it.
fn hold_next_close_question(host: &RuntimeHost) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (asking_tx, asking_rx) = mpsc::channel::<()>();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    host.run_before_hand_back_close_question_for_test(Box::new(move || {
        let _ = asking_tx.send(());
        let _ = go_rx.recv_timeout(GATE_WAIT);
    }))
    .expect("the hand-back's hook");
    (asking_rx, go_tx)
}

// Whether the resume `resumed` lifted its pause without a reconnect (no `selfcheck`).
fn resumed_without_a_reconnect(resumed: &RuntimeReceipt) -> bool {
    matches!(
        resumed.result(),
        Some(RuntimeResult::SchedulingResumed {
            selfcheck: None,
            ..
        })
    )
}

// Item 4: a person's pause, its caller's time not ended, waits for its instance's admission guard,
// which the test's thread holds; held there by the hook, a person resumes the instance. The wait
// sees the lift (the pause of its revision gone): the request answers Completed with
// `scheduling_pause_lifted_by_resume` (`paused`) and starts no continuation. Before this round it
// waited until its grace and then answered `scheduling_pause_standing` and started one.
#[test]
fn pg2c_fix2_4_a_resume_while_the_request_waits_for_the_admission_guard_answers_lifted() {
    let root = TempDir::new().expect("tempdir");
    let host = client_host(&root, false);
    let admission = host
        .instance_admission_for_test(CLIENT)
        .expect("the admission guard");
    let (met, go) = hold_next_request_wait(&host);
    let before = head(&host);
    let request = instance_pause_as(CLIENT, STAND);
    let taken = admission.lock().expect("the admission guard");

    let (paused, resumed) = thread::scope(|scope| {
        let pausing = scope.spawn(|| process(&host, &request, 3401));
        met.recv_timeout(GATE_WAIT)
            .expect("the request finds the guard taken");
        let resumed = resume(&host, instance_scope(CLIENT), 3402);
        go.send(()).expect("the request goes on");
        (pausing.join().expect("the pause's thread"), resumed)
    });
    drop(taken);

    completed(&resumed);
    assert!(resumed_without_a_reconnect(&resumed), "{resumed:?}");
    completed(&paused);
    assert_cause(&paused, "scheduling_pause_lifted_by_resume", "paused", None);
    assert_eq!(
        host.pause_carry_ons_running_for_test()
            .expect("the continuations"),
        0,
        "no continuation was started"
    );
    assert_lifted(&host, before, CLIENT);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// Item 5, `Lifted`: outside the multi-Nemu gate a person's pause has nothing in the way, and its
// hand-back, the admission guard in hand and its loop's top check passed, is held by the hook just
// before it asks under the pause table's lock whether it may close the session. A person resumes
// the instance then (no close had begun: lifted without a reconnect). The question finds the pause
// lifted: the hand-back ends and closes nothing; the request answers
// `scheduling_pause_lifted_by_resume`, and the session is still open. Without the question's
// re-check the loop's top check would have passed and the session would be closed.
#[test]
fn pg2c_fix2_5_a_resume_between_the_top_check_and_the_close_question_closes_nothing() {
    let root = TempDir::new().expect("tempdir");
    let (host, state) = nemu_host(&root, |config| config);
    let (asking, go) = hold_next_close_question(&host);
    let closed_before = state.capture_close_count.load(Ordering::Acquire);
    let before = head(&host);
    let request = instance_pause_as(NEMU, STAND);

    let (paused, resumed) = thread::scope(|scope| {
        let pausing = scope.spawn(|| process(&host, &request, 3411));
        asking
            .recv_timeout(GATE_WAIT)
            .expect("the hand-back is about to ask");
        let resumed = resume(&host, instance_scope(NEMU), 3412);
        go.send(()).expect("the hand-back asks");
        (pausing.join().expect("the pause's thread"), resumed)
    });

    completed(&resumed);
    assert!(resumed_without_a_reconnect(&resumed), "{resumed:?}");
    completed(&paused);
    assert_cause(&paused, "scheduling_pause_lifted_by_resume", "paused", None);
    assert_eq!(
        state.capture_close_count.load(Ordering::Acquire),
        closed_before,
        "nothing was closed"
    );
    assert!(
        host.holds_device_session_for_test(NEMU)
            .expect("the session"),
        "the session is still open"
    );
    assert_lifted(&host, before, NEMU);
    wait_for_no_continuation(&host);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// Item 5, `ResumeLifting`: as above, but the resume, held by its own hook, has marked that it lifts
// the pause without a reconnect (`Lifting`) and not lifted it yet when the hand-back asks. The
// question finds the mark: the hand-back closes nothing and looks again (the hook meets it a second
// time, so the first question closed nothing); once the resume lifted the pause the hand-back ends,
// the request answers `scheduling_pause_lifted_by_resume`, and the session is still open.
#[test]
fn pg2c_fix2_5_a_resume_lifting_when_the_hand_back_asks_closes_nothing() {
    let root = TempDir::new().expect("tempdir");
    let (host, state) = nemu_host(&root, |config| config);
    let (asking, go) = hold_next_close_question(&host);
    let (marked_tx, marked_rx) = mpsc::channel::<()>();
    let (lift_tx, lift_rx) = mpsc::channel::<()>();
    host.run_after_resume_marks_lifting_for_test(Box::new(move || {
        let _ = marked_tx.send(());
        let _ = lift_rx.recv_timeout(GATE_WAIT);
    }))
    .expect("the resume's hook");
    let closed_before = state.capture_close_count.load(Ordering::Acquire);
    let before = head(&host);
    let request = instance_pause_as(NEMU, STAND);

    let (paused, resumed) = thread::scope(|scope| {
        let pausing = scope.spawn(|| process(&host, &request, 3421));
        asking
            .recv_timeout(GATE_WAIT)
            .expect("the hand-back is about to ask");
        let resuming = scope.spawn(|| resume(&host, instance_scope(NEMU), 3422));
        marked_rx
            .recv_timeout(GATE_WAIT)
            .expect("the resume marked its lift");
        let (asking_again, go_again) = hold_next_close_question(&host);
        go.send(()).expect("the hand-back asks");
        asking_again
            .recv_timeout(GATE_WAIT)
            .expect("the hand-back closed nothing and asks again");
        lift_tx.send(()).expect("the resume lifts");
        let resumed = resuming.join().expect("the resume's thread");
        go_again.send(()).expect("the hand-back asks again");
        (pausing.join().expect("the pause's thread"), resumed)
    });

    completed(&resumed);
    assert!(resumed_without_a_reconnect(&resumed), "{resumed:?}");
    completed(&paused);
    assert_cause(&paused, "scheduling_pause_lifted_by_resume", "paused", None);
    assert_eq!(
        state.capture_close_count.load(Ordering::Acquire),
        closed_before,
        "nothing was closed"
    );
    assert!(
        host.holds_device_session_for_test(NEMU)
            .expect("the session"),
        "the session is still open"
    );
    assert_lifted(&host, before, NEMU);
    wait_for_no_continuation(&host);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// Item 6 (the commit mark withdrawn), the `quiet_or_lift` half of
// `pg2c_a_pause_after_an_automatic_starts_commit_lists_it_and_the_start_is_finished`: a configured
// start asked its last question and its Start runs (blocked in the fake, holding the instance's
// admission guard and its claim's lease); a tool's hold of the instance, its grace already past,
// fails `scheduling_pause_release_busy` and is lifted, and the Start that had begun is finished as
// before: one provider Start.
#[test]
fn pg2c_fix2_6_a_tools_hold_while_an_automatic_start_runs_fails_past_its_grace_and_is_lifted() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance = instance_id();
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let host = autostart_host(&root, &[(AUTOSTART, instance, &state)], read, |config| {
        config.with_scheduling_pause_deadlines_for_test(Duration::ZERO)
    });
    stop(&host, AUTOSTART, 3431);
    // The Start blocks in its readiness wait, which then ends timed out.
    state.block_emulator_start.store(true, Ordering::Release);
    state.fail_emulator_start.store(true, Ordering::Release);
    assert_eq!(host.enqueue_emulator_autostart().expect("enqueue"), 1);
    wait_until("the configured Start", || {
        state.emulator_start_entered.load(Ordering::Acquire)
    });
    let before = head(&host);
    let request = instance_pause_as(AUTOSTART, HOLD);

    let failed = process(&host, &request, 3432);

    assert_failed_with(&host, &failed, &request, "scheduling_pause_release_busy");
    assert_lifted(&host, before, AUTOSTART);
    state.block_emulator_start.store(false, Ordering::Release);
    wait_until("the claim ends", || {
        host.instance_claims_for_test(AUTOSTART).expect("claims") == (None, vec![])
    });
    let starts = state
        .emulator
        .calls()
        .into_iter()
        .filter(|call| *call == EmulatorCall::Control(START))
        .count();
    assert_eq!(starts, 1, "the Start that had begun was finished");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}
