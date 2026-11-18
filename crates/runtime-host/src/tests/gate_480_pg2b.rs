// SPDX-License-Identifier: AGPL-3.0-only

// #480 PG-2b (0.12.73; register §1.471; the owner's requirement on board #480: a pause a person
// asked for closes the gate at once and stands, and the Runtime never takes it back for work of
// its own). Three places where the Runtime's own recovery or its own queue took a pause back or
// started something after it: (1) a rung of the stuck-recovery ladder makes its holding check
// before the application rung's stop and before each rung's startup package run, as it does
// before the emulator rung's Stop and Start; (2) an instance pause's hand-back waits for the
// Runtime's own ladder that holds the instance as it waits for an automatic emulator start: past
// the caller's grace, with no bound of its own, and a shutdown during that wait keeps the pause;
// (3) outside the multi-Nemu gate, the dedicated close lease of that hand-back is not held up by
// the instance's own entries that the pause holds back. On the fake device, the ladder fixture of
// `instance_workers` (a failed policy run hands its key to its ladder), the drill
// (`DrillRecoveryLadder`) and two Nemu instances as PG-1's (4b). Fix round 1 (coordinator work
// order `rd-1118/brief-480-pg2b-fix1.md`): (7) the settle gate's rerun of the return-home package
// asks first, on #411 A35's settle fixture built here (a manual Runtime clock, a moving screen);
// (8) a rung that a pause interrupts after its stop hands its startup run off, and the run goes
// after the resume, once. Fix round 2 (coordinator work order `rd-1118/brief-480-pg2b-fix2.md`):
// (9) in a device group of several, a pause recorded at the application rung's boundary stops no
// other member, on Workflow #437 ORD-1c's grouped fixture built here. Every wait returns as soon
// as what it waits for is seen; `GATE_WAIT` only bounds a broken run.

use super::gate_369_s6c::{head, instance_scope, pause, resume, send};
use super::gate_387_d1c::ladder_manifest;
use super::instance_workers::{
    admit_ladder_run, all_events, ladder_end_code, ladder_finished, ladder_setup, phases,
    rung_outcomes, wait_for_ladder_end, wait_until,
};
use super::*;
use crate::codes::HostCode;
use actingcommand_contract::{
    ConfigParameter, ConfigParameterSource, EmulatorInstanceAction, FactScalar, FactValue,
    InstancePauseStage, RecoveryDrillMode, RecoveryLadderOutcome, RecoveryRung,
    RecoveryRungOutcome, RuntimeConfigManifest, RuntimeLifecyclePhase, RuntimePayload,
    SchedulingPauseReason, SchedulingPauseScope, StartupPackageDisposition,
};
use actingcommand_runtime_config::scheduling::{
    DEVICE_GROUP_MAX_QUEUE_WAIT_MS, DEVICE_GROUP_SWITCH_FAILURE_COOLDOWN_MS,
    EXPECTED_RUNTIME_ENABLED,
};
use actingcommand_scheduler::ClaimKind;
use serde_json::{Value, json};

/// Test plan v2.2 §2.1 guard 3: the one bound of a cross-thread wait in this file; it bounds a
/// broken run's wait for the ladder (its first rung's run and settle gate come first).
const GATE_WAIT: Duration = Duration::from_secs(120);
/// How many checks of the hand-back that met the ladder's key a test waits for: the last of them
/// began after the earlier ones had each passed the grace, which a zero deadline ends at once.
const CHECKS_PAST: u64 = 3;
/// The runtime fact of the held pauses (`host.scheduling_pause`).
const PAUSE_FACT_KEY: &str = "host.scheduling_pause";
/// A ladder's holding interruption by a scheduling pause.
const ADMISSION_DENIED: &str = "recovery_admission_denied";
const DRILL: &str = "pg2b.drill";
const NEMU_FIRST: &str = "pg2b.first";
const NEMU_SECOND: &str = "pg2b.second";

fn completed(receipt: &RuntimeReceipt) {
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
}

// A person's pause of `alias`, with the smallest drain timeout a request may name.
fn instance_pause(alias: &str) -> RuntimeRequest {
    runtime_request(
        &IdentifierIssuer::new().expect("identifier issuer"),
        RuntimeOperation::PauseScheduling {
            scope: instance_scope(alias),
            pause_reason: SchedulingPauseReason::new("pg2b_pause").expect("pause reason"),
            drain_timeout_ms: 1_000,
        },
    )
}

// The stage of the pause `alias` holds of its own, if it holds one.
fn pause_stage(host: &RuntimeHost, alias: &str) -> Option<InstancePauseStage> {
    host.scheduling_pauses_for_test()
        .expect("the held pauses")
        .1
        .get(alias)
        .map(|pause| pause.stage)
}

// The rows of every record of `host.scheduling_pause` after `after`, in order.
fn pause_records_after(host: &RuntimeHost, after: u64) -> Vec<Vec<BTreeMap<String, FactScalar>>> {
    all_events(host)
        .iter()
        .filter(|event| event.sequence() > after)
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::FactRecorded(recorded))
                if recorded.record().key == PAUSE_FACT_KEY =>
            {
                match &recorded.record().value {
                    FactValue::RecordList(rows) => Some(rows.clone()),
                    _ => None,
                }
            }
            _ => None,
        })
        .collect()
}

// Whether a record of the held pauses names a pause of `alias`.
fn names_instance(rows: &[BTreeMap<String, FactScalar>], alias: &str) -> bool {
    rows.iter()
        .any(|row| row.get("instance_alias") == Some(&FactScalar::String(alias.to_owned())))
}

// After `before`, the pause's one record of the held pauses, naming `alias`: nothing was lifted.
#[track_caller]
fn assert_one_record_naming(host: &RuntimeHost, before: u64, alias: &str) {
    let records = pause_records_after(host, before);
    let [record] = &records[..] else {
        panic!("the pause's one record of the held pauses, nothing lifted: {records:?}");
    };
    assert!(names_instance(record, alias), "{record:?}");
}

// After `after`, how many startup packages were scheduled (`startup_package_scheduled`) and how
// many contained runs were requested (`task.requested`).
fn package_runs_after(host: &RuntimeHost, after: u64) -> (usize, usize) {
    let events = all_events(host);
    let scheduled = phases(&events)
        .into_iter()
        .filter(|(event, phase)| {
            event.sequence() > after
                && matches!(phase, RuntimeLifecyclePhase::StartupPackageScheduled { .. })
        })
        .count();
    let requested = events
        .iter()
        .filter(|event| event.sequence() > after && event.event_type() == EventType::TaskRequested)
        .count();
    (scheduled, requested)
}

// The result and the reason of the one finished record of `rung`.
fn rung_outcome(
    host: &RuntimeHost,
    rung: RecoveryRung,
) -> Option<(RecoveryRungOutcome, Option<String>)> {
    rung_outcomes(&all_events(host))
        .into_iter()
        .find(|(finished, _, _)| *finished == rung)
        .map(|(_, outcome, reason)| (outcome, reason))
}

// The ladder ended interrupted: `exhausted` at Warning with `recovery_ladder_interrupted`, no
// error point.
#[track_caller]
fn assert_ladder_interrupted(host: &RuntimeHost) {
    let events = all_events(host);
    let (finished, outcome, _) = ladder_finished(&events).expect("the ladder finished");
    assert_eq!(outcome, RecoveryLadderOutcome::Exhausted);
    assert_eq!(
        ladder_end_code(finished),
        Some("recovery_ladder_interrupted")
    );
    assert_eq!(finished.severity(), EventSeverity::Warning);
}

fn emulator_actions(state: &FakeState) -> Vec<EmulatorInstanceAction> {
    state
        .emulator_actions
        .lock()
        .expect("fake emulator actions lock")
        .clone()
}

// Holds the next holding check a rung makes before its application stop or its startup package run
// (`run_before_ladder_step_check_for_test`): the ladder's worker, holding the ladder's key, sends
// on the returned receiver's channel and waits for the returned sender before it makes the check.
fn hold_next_rung_step(host: &RuntimeHost) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (at_tx, at_rx) = mpsc::channel::<()>();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    host.run_before_ladder_step_check_for_test(Box::new(move || {
        let _ = at_tx.send(());
        let _ = go_rx.recv_timeout(GATE_WAIT);
    }))
    .expect("the rung's hook");
    (at_rx, go_tx)
}

// The ladder fixture of `instance_workers` (`ladder_setup`): a physical policy run that binds a
// return-home package, on an instance whose startup package is configured, admitted and then left
// with captures that stay unknown, so that it and every rung run fail.
struct Climb {
    _root: TempDir,
    config: RuntimeHostConfig,
    registered: InstanceId,
    host: RuntimeHost,
    state: Arc<FakeState>,
    context: Box<PolicyRunContext>,
    request: ContainedTaskRequest,
}

// The started fixture, its configuration `configure`d. With `emulator`, the instance has emulator
// control and a discovery binding, so the emulator rung runs its Stop and Start, and Android
// reports no foreground activity after the Start until a test lets it boot.
fn start_climb(
    emulator: bool,
    configure: impl FnOnce(RuntimeHostConfig) -> RuntimeHostConfig,
) -> Climb {
    let root = TempDir::new().expect("tempdir");
    let (host_config, request) = ladder_setup(root.path());
    let host_config = configure(host_config);
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    let registered = instance_id();
    let host = RuntimeHost::start(
        host_config.clone(),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            registered,
            Arc::clone(&state),
        )),
    )
    .expect("physical runtime host");
    if emulator {
        state.hold_android_boot.store(true, Ordering::Release);
        bind_fake_emulator(&host, POLICY_INSTANCE_ALIAS, registered, &state);
    }
    let context = admit_ladder_run(&host, &request);
    state.unknown_capture.store(true, Ordering::Release);
    Climb {
        _root: root,
        config: host_config,
        registered,
        host,
        state,
        context,
        request,
    }
}

impl Climb {
    // Runs the scheduled task; its failure hands the key to the ladder, which climbs on the
    // instance's worker.
    fn fail_scheduled_run(&self) {
        let error = self
            .host
            .run_scheduled_contained_task(&self.context, &self.request)
            .expect_err("the scheduled run fails");
        assert!(
            !error.is_fatal(),
            "a stuck-recovery trigger: {}",
            error.code()
        );
    }
}

// (1) Item 1, the application rung: the ladder, holding its key after rung 1's failed run, is about
// to stop the assigned application, and a person pauses the instance. The rung's holding check
// before the stop sees the pause: no stop and no startup run (no `startup_package_scheduled`, no
// `task.requested`, the game untouched), the rung failed with `recovery_admission_denied`, and the
// ladder finished interrupted (`exhausted`, Warning). The pause answers success, `released`.
#[test]
fn pg2b_1_a_pause_before_the_application_rungs_stop_stops_nothing_and_runs_nothing() {
    let climb = start_climb(false, |config| config);
    let (at, go) = hold_next_rung_step(&climb.host);
    climb.fail_scheduled_run();
    at.recv_timeout(GATE_WAIT)
        .expect("the application rung is about to stop");
    let before = head(&climb.host);
    let pause_request = instance_pause(POLICY_INSTANCE_ALIAS);

    let paused = thread::scope(|scope| {
        let pausing = scope.spawn(|| {
            climb.host.process_request_for_test(
                &pause_request,
                ConnectionId::new(1011).expect("connection"),
            )
        });
        wait_until("the pause holds the instance", || {
            pause_stage(&climb.host, POLICY_INSTANCE_ALIAS).is_some()
        });
        go.send(()).expect("the rung goes on");
        pausing
            .join()
            .expect("the pause's thread")
            .expect("the pause's receipt")
    });

    completed(&paused);
    wait_for_ladder_end(&climb.host, POLICY_INSTANCE_ALIAS);
    assert_eq!(
        climb.state.application_count.load(Ordering::Acquire),
        0,
        "no application stopped or launched"
    );
    assert_eq!(
        package_runs_after(&climb.host, before),
        (0, 0),
        "no startup run"
    );
    assert_eq!(
        rung_outcome(&climb.host, RecoveryRung::ApplicationRestart),
        Some((
            RecoveryRungOutcome::Failed,
            Some(ADMISSION_DENIED.to_owned())
        ))
    );
    assert_ladder_interrupted(&climb.host);
    assert_eq!(
        pause_stage(&climb.host, POLICY_INSTANCE_ALIAS),
        Some(InstancePauseStage::Released),
        "the pause stands"
    );
    assert!(climb.host.fatal_error().expect("runtime health").is_none());
    climb.host.close().expect("close host");
}

// (2) Item 1, the emulator rung: rung 3 stopped and started the emulator, its readiness wait
// passes once Android has booted, and it records `recovery_environment_ready`; just before its
// startup package's run a person presses "pause all". The rung's holding check sees the pause: no
// package run (no `task.requested`), rung 3 failed with `recovery_admission_denied`, the ladder
// finished interrupted, and the emulator saw nothing but that Stop and that Start. Fix round 1
// (item 4, which changes what this test pinned): the rung brought the emulator up, so it hands its
// startup run off: one `startup_package_scheduled`, and its claim waits queued behind the pause.
#[test]
fn pg2b_2_a_pause_after_the_emulator_rungs_preparation_runs_no_package() {
    let climb = start_climb(true, |config| config);
    climb.fail_scheduled_run();
    wait_until("rung 3's Stop and Start", || {
        emulator_actions(&climb.state)
            == [EmulatorInstanceAction::Stop, EmulatorInstanceAction::Start]
    });
    // Rung 2's checks are behind the ladder now: the next one is rung 3's, before its startup run.
    let (at, go) = hold_next_rung_step(&climb.host);
    // The readiness wait's next attempt sees Android up and passes its preparation.
    climb.state.observed_open.store(true, Ordering::Release);
    climb.state.android_down.store(false, Ordering::Release);
    at.recv_timeout(GATE_WAIT)
        .expect("the emulator rung's preparation passed");
    let before = head(&climb.host);

    completed(&pause(&climb.host, SchedulingPauseScope::Global, 1021));
    go.send(()).expect("the rung goes on");

    wait_for_ladder_end(&climb.host, POLICY_INSTANCE_ALIAS);
    let events = all_events(&climb.host);
    assert!(
        phases(&events).iter().any(|(_, phase)| matches!(
            phase,
            RuntimeLifecyclePhase::RecoveryEnvironmentReady { .. }
        )),
        "rung 3's readiness passed"
    );
    assert_eq!(
        package_runs_after(&climb.host, before),
        (1, 0),
        "no package run: the startup run handed off"
    );
    assert_eq!(
        climb
            .host
            .instance_claims_for_test(POLICY_INSTANCE_ALIAS)
            .expect("claims"),
        (None, vec![ClaimKind::StartupPackage]),
        "the handed-off startup claim waits behind the pause"
    );
    assert_eq!(
        rung_outcome(&climb.host, RecoveryRung::EmulatorRestart),
        Some((
            RecoveryRungOutcome::Failed,
            Some(ADMISSION_DENIED.to_owned())
        ))
    );
    assert_ladder_interrupted(&climb.host);
    assert_eq!(
        emulator_actions(&climb.state),
        [EmulatorInstanceAction::Stop, EmulatorInstanceAction::Start]
    );
    assert!(climb.host.fatal_error().expect("runtime health").is_none());
    climb.host.close().expect("close host");
}

// (3) Item 2, the 11-17 shape with the ladder: the ladder holds the instance by its key, held just
// before the application rung's stop, and a person pauses the instance with the grace already
// past (`with_scheduling_pause_deadlines_for_test`). The hand-back meets the ladder's key check
// after check and fails nothing: the stage stays `paused`, and the one record of the held pauses
// names the instance. Once the rung goes on, its holding check ends the ladder, and the pause
// answers success, `released`, its one record still naming the instance. (Before this slice the
// ladder's key was waited for only until the grace: `scheduling_pause_release_busy`, lifted, and
// the ladder's next holding check passed.)
#[test]
fn pg2b_3_an_instance_pause_waits_for_a_rung_past_its_grace_and_stands() {
    let climb = start_climb(false, |config| {
        config.with_scheduling_pause_deadlines_for_test(Duration::ZERO)
    });
    let (at, go) = hold_next_rung_step(&climb.host);
    climb.fail_scheduled_run();
    at.recv_timeout(GATE_WAIT)
        .expect("the application rung is about to stop");
    let before = head(&climb.host);
    let pause_request = instance_pause(POLICY_INSTANCE_ALIAS);

    let paused = thread::scope(|scope| {
        let pausing = scope.spawn(|| {
            climb.host.process_request_for_test(
                &pause_request,
                ConnectionId::new(1031).expect("connection"),
            )
        });
        wait_until("the hand-back meets the ladder again and again", || {
            climb
                .host
                .pause_release_lease_meetings_for_test()
                .expect("the hand-back's checks")
                >= CHECKS_PAST
        });
        assert_eq!(
            pause_stage(&climb.host, POLICY_INSTANCE_ALIAS),
            Some(InstancePauseStage::Paused),
            "the stage stays where it is"
        );
        assert_eq!(
            climb
                .host
                .instance_claims_for_test(POLICY_INSTANCE_ALIAS)
                .expect("claims")
                .0,
            Some(ClaimKind::Ladder),
            "the ladder holds the instance"
        );
        assert_one_record_naming(&climb.host, before, POLICY_INSTANCE_ALIAS);
        go.send(()).expect("the rung goes on");
        pausing
            .join()
            .expect("the pause's thread")
            .expect("the pause's receipt")
    });

    completed(&paused);
    assert!(
        matches!(
            paused.result(),
            Some(RuntimeResult::SchedulingPaused {
                drained: Some(_),
                ..
            })
        ),
        "{paused:?}"
    );
    assert!(
        ladder_finished(&all_events(&climb.host)).is_some(),
        "the ladder ended before the pause answered"
    );
    assert_eq!(
        pause_stage(&climb.host, POLICY_INSTANCE_ALIAS),
        Some(InstancePauseStage::Released),
        "the pause stands"
    );
    assert_one_record_naming(&climb.host, before, POLICY_INSTANCE_ALIAS);
    wait_for_ladder_end(&climb.host, POLICY_INSTANCE_ALIAS);
    assert_eq!(
        rung_outcome(&climb.host, RecoveryRung::ApplicationRestart),
        Some((
            RecoveryRungOutcome::Failed,
            Some(ADMISSION_DENIED.to_owned())
        ))
    );
    assert_ladder_interrupted(&climb.host);
    assert_eq!(
        climb.state.application_count.load(Ordering::Acquire),
        0,
        "no application stopped or launched"
    );
    assert!(climb.host.fatal_error().expect("runtime health").is_none());
    climb.host.close().expect("close host");
}

// (4) Item 2, R4″ with the ladder: the Runtime is asked to stop while the hand-back waits for the
// ladder alone (as in (3)). The pause is not lifted: the request fails with
// `scheduling_pause_kept_at_shutdown`, the pause stays `paused`, and its one record of the held
// pauses still names the instance. The next start restores it (stage `released`).
#[test]
fn pg2b_4_a_shutdown_while_the_pause_waits_for_a_rung_keeps_the_pause() {
    let climb = start_climb(false, |config| {
        config.with_scheduling_pause_deadlines_for_test(Duration::ZERO)
    });
    let (at, go) = hold_next_rung_step(&climb.host);
    climb.fail_scheduled_run();
    at.recv_timeout(GATE_WAIT)
        .expect("the application rung is about to stop");
    let before = head(&climb.host);
    let pause_request = instance_pause(POLICY_INSTANCE_ALIAS);

    let kept = thread::scope(|scope| {
        let pausing = scope.spawn(|| {
            climb.host.process_request_for_test(
                &pause_request,
                ConnectionId::new(1041).expect("connection"),
            )
        });
        wait_until("the hand-back waits for the ladder", || {
            climb
                .host
                .pause_release_lease_meetings_for_test()
                .expect("the hand-back's checks")
                >= CHECKS_PAST
        });
        climb
            .host
            .request_shutdown_for_test()
            .expect("the Runtime is asked to stop");
        pausing
            .join()
            .expect("the pause's thread")
            .expect("the pause's receipt")
    });
    go.send(()).expect("the rung goes on");

    assert_eq!(kept.state(), RuntimeReceiptState::Failed, "{kept:?}");
    let outcome = climb
        .host
        .failure_outcome_for_test(pause_request.request_id())
        .expect("the refused requests")
        .expect("the pause's failure");
    assert_eq!(
        outcome.code().as_str(),
        "scheduling_pause_kept_at_shutdown",
        "{outcome:?}"
    );
    assert_eq!(
        pause_stage(&climb.host, POLICY_INSTANCE_ALIAS),
        Some(InstancePauseStage::Paused),
        "the pause is not lifted"
    );
    assert_one_record_naming(&climb.host, before, POLICY_INSTANCE_ALIAS);
    let Climb {
        _root,
        config: host_config,
        registered,
        host,
        state,
        ..
    } = climb;
    host.close().expect("close the stopping host");

    let host = RuntimeHost::start(
        host_config,
        Arc::new(FakeProvider::one(POLICY_INSTANCE_ALIAS, registered, state)),
    )
    .expect("the next start");
    assert_eq!(
        pause_stage(&host, POLICY_INSTANCE_ALIAS),
        Some(InstancePauseStage::Released),
        "the next start restores the pause"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// A started host whose one physical instance `DRILL` is discovery-bound to the fake emulator, with
// opens that pass a preparation's self-check and no startup package (`gate_390_drill1`'s), every
// instance pause's grace already past.
fn drill_host(root: &TempDir, state: &Arc<FakeState>, instance: InstanceId) -> RuntimeHost {
    state.physical_task_geometry.store(true, Ordering::Release);
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    state.observed_open.store(true, Ordering::Release);
    let host = RuntimeHost::start(
        config(root).with_scheduling_pause_deadlines_for_test(Duration::ZERO),
        Arc::new(FakeProvider::one(DRILL, instance, Arc::clone(state))),
    )
    .expect("runtime host with the fake emulator");
    bind_fake_emulator(&host, DRILL, instance, state);
    host
}

// (5) Item 2, another party's work together with the ladder: a client's lease holds the instance
// and a person's drill of its ladder (`DrillRecoveryLadder`, `device`) waits queued behind it. A
// person pauses the instance with the grace already past: the client's lease is no work of the
// Runtime's own, so the pause fails as before with `scheduling_pause_release_busy` and is lifted
// (the record that set it and the record without it). The ladder's queued claim, which the pause
// held back, refused nothing; the client's lease and that claim are as they were.
#[test]
fn pg2b_5_a_clients_lease_with_the_ladder_waiting_still_fails_and_lifts_the_pause() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = drill_host(&root, &state, instance_id());
    let holder = IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_holder_id()
        .expect("holder id");
    let acquired = send(&host, RuntimeOperation::acquire_lease(DRILL, holder), 1051);
    assert!(
        matches!(acquired.result(), Some(RuntimeResult::LeaseGranted { .. })),
        "{acquired:?}"
    );
    completed(&send(
        &host,
        RuntimeOperation::DrillRecoveryLadder {
            instance_alias: DRILL.to_owned(),
            mode: RecoveryDrillMode::Device,
        },
        1052,
    ));
    let waiting = (Some(ClaimKind::ClientLease), vec![ClaimKind::Ladder]);
    assert_eq!(
        host.instance_claims_for_test(DRILL).expect("claims"),
        waiting,
        "the drill's ladder waits behind the client's lease"
    );
    let before = head(&host);
    let pause_request = instance_pause(DRILL);

    let failed = host
        .process_request_for_test(&pause_request, ConnectionId::new(1053).expect("connection"))
        .expect("the pause's receipt");

    assert_eq!(failed.state(), RuntimeReceiptState::Failed, "{failed:?}");
    let outcome = host
        .failure_outcome_for_test(pause_request.request_id())
        .expect("the refused requests")
        .expect("the pause's failure");
    assert_eq!(
        outcome.code().as_str(),
        "scheduling_pause_release_busy",
        "{outcome:?}"
    );
    assert_eq!(pause_stage(&host, DRILL), None, "the pause is lifted");
    let records = pause_records_after(&host, before);
    let [set, lifted] = &records[..] else {
        panic!("the pause's record and its lift's: {records:?}");
    };
    assert!(names_instance(set, DRILL), "{set:?}");
    assert!(!names_instance(lifted, DRILL), "{lifted:?}");
    assert_eq!(
        host.instance_claims_for_test(DRILL).expect("claims"),
        waiting,
        "the client's lease and the ladder's claim are as they were"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// The neutral startup package, written under `root`.
fn startup_package(root: &TempDir) -> ContainedTaskRequest {
    let path = root.path().join("startup-package.zip");
    fs::write(&path, neutral_contained_task_package(true)).expect("write startup package");
    ContainedTaskRequest::new(path.to_string_lossy().into_owned())
        .and_then(|request| request.with_response_deadline_ms(60_000))
        .expect("startup package request")
}

// How many contained runs completed and how many failed after `after`.
fn runs_ended_after(host: &RuntimeHost, after: u64) -> (usize, usize) {
    let events = all_events(host);
    let ended = |event_type: EventType| {
        events
            .iter()
            .filter(|event| event.sequence() > after && event.event_type() == event_type)
            .count()
    };
    (
        ended(EventType::TaskCompleted),
        ended(EventType::TaskFailed),
    )
}

// (6) Item 3, outside the multi-Nemu gate: two instances capture through the Nemu IPC, so a pause
// of the first closes its device session on the dedicated close lease; a keyless observation left
// that session open. A person pauses the first; while its hand-back waits for the instance's
// admission guard, which the test's thread holds, the first's startup package is queued, held back
// by the pause. Once the guard is let go the hand-back's close lease is granted past that entry
// (no `lease_queue_ahead`): the session is closed and the pause answers success, `released`,
// nothing lifted, no run requested; the package is still queued. After the resume it runs. (Before
// this slice the close lease read the queue as an open gate does: `lease_queue_ahead`, the pause
// lifted, and the package passed the opened gate.)
#[test]
fn pg2b_6_outside_the_multi_nemu_gate_the_close_lease_passes_the_startup_package_the_pause_holds() {
    let root = TempDir::new().expect("tempdir");
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
        config(&root).with_startup_packages(BTreeMap::from([(
            NEMU_FIRST.to_owned(),
            startup_package(&root),
        )])),
        Arc::new(FakeProvider::from_entries([
            (NEMU_FIRST.to_owned(), instance_id(), Arc::clone(&state)),
            (
                NEMU_SECOND.to_owned(),
                instance_id(),
                Arc::clone(&other_state),
            ),
        ])),
    )
    .expect("runtime host with two Nemu instances");
    assert!(
        !host
            .keeps_device_session_for_test(NEMU_FIRST)
            .expect("the first's session gate"),
        "outside the multi-Nemu gate"
    );
    completed(&send(
        &host,
        RuntimeOperation::ObserveReadonly {
            instance_alias: NEMU_FIRST.to_owned(),
        },
        1061,
    ));
    let admission = host
        .instance_admission_for_test(NEMU_FIRST)
        .expect("the first's admission guard");
    let closed_before = state.capture_close_count.load(Ordering::Acquire);
    let before = head(&host);
    let pause_request = instance_pause(NEMU_FIRST);

    let paused = thread::scope(|scope| {
        let taken = admission.lock().expect("the first's admission guard");
        let pausing = scope.spawn(|| {
            host.process_request_for_test(
                &pause_request,
                ConnectionId::new(1062).expect("connection"),
            )
        });
        // The pause holds the instance; its hand-back comes to wait for the guard.
        wait_until("the pause's hand-back", || {
            pause_stage(&host, NEMU_FIRST) == Some(InstancePauseStage::Paused)
        });
        assert_eq!(
            host.schedule_startup_package_for_test(NEMU_FIRST)
                .expect("the first's startup package"),
            StartupPackageDisposition::Scheduled
        );
        assert_eq!(
            host.instance_claims_for_test(NEMU_FIRST).expect("claims"),
            (None, vec![ClaimKind::StartupPackage]),
            "the startup package waits behind the pause"
        );
        drop(taken);
        pausing
            .join()
            .expect("the pause's thread")
            .expect("the pause's receipt")
    });

    completed(&paused);
    assert_eq!(
        pause_stage(&host, NEMU_FIRST),
        Some(InstancePauseStage::Released),
        "the pause stands"
    );
    assert!(
        state.capture_close_count.load(Ordering::Acquire) > closed_before,
        "the close lease closed the first's session"
    );
    assert_one_record_naming(&host, before, NEMU_FIRST);
    assert_eq!(
        host.instance_claims_for_test(NEMU_FIRST).expect("claims"),
        (None, vec![ClaimKind::StartupPackage]),
        "the startup package is still queued"
    );
    assert_eq!(
        package_runs_after(&host, before),
        (1, 0),
        "the package was scheduled and not run"
    );

    let resumed_at = head(&host);
    completed(&resume(&host, instance_scope(NEMU_FIRST), 1063));
    wait_until("the startup run after the resume and its release", || {
        let (completed_runs, failed_runs) = runs_ended_after(&host, resumed_at);
        completed_runs + failed_runs > 0
            && host.instance_claims_for_test(NEMU_FIRST).expect("claims") == (None, vec![])
    });
    assert_eq!(
        runs_ended_after(&host, resumed_at),
        (1, 0),
        "the startup package ran after the resume"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

/// The settle gate's configuration of (7), as #411 A35's tests set it: a 100 ms poll, three probe
/// frames 500 ms apart, a 40 s recheck within 100 s.
const SETTLE_POLL_MS: u64 = 100;
const SETTLE_RECHECK_MS: u64 = 40_000;
/// The fake frame is 16 pixels wide: its second row starts here; eight of its cells move.
const SECOND_ROW: usize = 16;
const MOVING_CELLS: usize = 8;

// #411 A35's settle fixture (`gate_411_a35`): the ladder fixture's policy run on a manual Runtime
// clock that only the test and the kernel's waits move, the settle gate configured as above, and a
// screen that moves while `moving` holds (eight cells of the fake frame's second row alternate
// between black and white from capture to capture). The scheduled run and the return-home run fail
// `contained_task_page_unknown` (class `pack_logic_mismatch`, a listed class), the gate's first
// probe finds the screen changing, and the ladder holds until the clock passes the recheck.
struct Settle {
    _root: TempDir,
    host: RuntimeHost,
    state: Arc<FakeState>,
    clock: Arc<ManualRuntimeClock>,
    moving: Arc<AtomicBool>,
}

fn start_settle() -> Settle {
    let root = TempDir::new().expect("tempdir");
    let (host_config, request) = ladder_setup(root.path());
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    let moving = Arc::new(AtomicBool::new(true));
    let moves = Arc::clone(&moving);
    *state.pixel_script.lock().expect("pixel script") = Some(Arc::new(
        move |capture: usize, _inputs: usize| -> Vec<(usize, [u8; 3])> {
            if !moves.load(Ordering::Acquire) || capture.is_multiple_of(2) {
                return Vec::new();
            }
            (SECOND_ROW..SECOND_ROW + MOVING_CELLS)
                .map(|index| (index, [248, 248, 248]))
                .collect()
        },
    ));
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = RuntimeHost::start(
        host_config
            .with_runtime_clock(clock.clone())
            .with_config_manifest(ladder_manifest(json!({
                "busy": {
                    "poll_ms": SETTLE_POLL_MS,
                    "screen": {
                        "probe_frames": 3,
                        "probe_interval_ms": 500,
                        "recheck_ms": SETTLE_RECHECK_MS,
                        "max_wait_ms": 100_000,
                    },
                },
            }))),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .expect("physical runtime host");
    let context = admit_ladder_run(&host, &request);
    state.unknown_capture.store(true, Ordering::Release);
    let failed = host
        .run_scheduled_contained_task(&context, &request)
        .expect_err("the scheduled run fails");
    assert!(!failed.is_fatal(), "{}", failed.code());
    Settle {
        _root: root,
        host,
        state,
        clock,
        moving,
    }
}

// (7) Fix round 1, the review's second must-fix: the settle gate's rerun of the return-home
// package asks first. The return-home run fails on a changing screen and the ladder holds; the
// screen then stands still and the clock passes the recheck, so the gate's next probe finds it
// settled, and the ladder is about to run the return-home package again when a person pauses the
// instance. The holding check right before the rerun sees the pause: no rerun (no
// `task.requested`), the return-home rung recorded failed with `recovery_admission_denied` after
// its changing verdict, the ladder finished interrupted (`exhausted`, Warning), no application
// command; the pause answers success, `released`. (Before this round the rerun ran, and only the
// pause's drain stopped it.)
#[test]
fn pg2b_7_a_pause_before_the_settle_gates_rerun_runs_no_return_home_package() {
    let settle = start_settle();
    wait_until("the screen-changing verdict", || {
        !rung_outcomes(&all_events(&settle.host)).is_empty()
    });
    let (at, go) = hold_next_rung_step(&settle.host);
    settle.moving.store(false, Ordering::Release);
    settle.clock.advance(SETTLE_RECHECK_MS);
    at.recv_timeout(GATE_WAIT)
        .expect("the settle gate is about to run the return-home package again");
    let before = head(&settle.host);
    let pause_request = instance_pause(POLICY_INSTANCE_ALIAS);

    let paused = thread::scope(|scope| {
        let pausing = scope.spawn(|| {
            settle.host.process_request_for_test(
                &pause_request,
                ConnectionId::new(1071).expect("connection"),
            )
        });
        wait_until("the pause holds the instance", || {
            pause_stage(&settle.host, POLICY_INSTANCE_ALIAS).is_some()
        });
        go.send(()).expect("the settle gate goes on");
        pausing
            .join()
            .expect("the pause's thread")
            .expect("the pause's receipt")
    });

    completed(&paused);
    wait_for_ladder_end(&settle.host, POLICY_INSTANCE_ALIAS);
    assert_eq!(
        package_runs_after(&settle.host, before),
        (0, 0),
        "no return-home rerun"
    );
    assert_eq!(
        rung_outcomes(&all_events(&settle.host)),
        vec![
            (
                RecoveryRung::ReturnHome,
                RecoveryRungOutcome::Failed,
                Some(HostCode::RecoveryRungScreenChanging.as_str().to_owned())
            ),
            (
                RecoveryRung::ReturnHome,
                RecoveryRungOutcome::Failed,
                Some(ADMISSION_DENIED.to_owned())
            ),
        ]
    );
    assert_ladder_interrupted(&settle.host);
    assert_eq!(
        settle.state.application_count.load(Ordering::Acquire),
        0,
        "no application command"
    );
    assert_eq!(
        pause_stage(&settle.host, POLICY_INSTANCE_ALIAS),
        Some(InstancePauseStage::Released),
        "the pause stands"
    );
    assert!(settle.host.fatal_error().expect("runtime health").is_none());
    settle.host.close().expect("close host");
}

// (8) Fix round 1, item 4 (ruled by the coordinator): a rung that a pause interrupts after its
// stop owes the startup run, by the existing way. The application rung's check before its stop
// passes and it stops the assigned application; just before its startup run a person pauses the
// instance. The rung's holding check sees the pause and hands the run off as an emulator start
// under a pause hands off its own: one `startup_package_scheduled` and no `task.requested`, the
// startup claim queued behind the pause; the rung failed with `recovery_admission_denied`, the
// ladder finished interrupted, nothing launched, the pause success, `released`, the claim still
// queued. After the resume (its reconnect first) the claim is granted and the package runs once:
// one `task.requested`, no second `startup_package_scheduled`, one run ended, the queue empty.
#[test]
fn pg2b_8_a_rung_paused_after_its_stop_hands_its_startup_run_off_and_it_runs_once_after_the_resume()
{
    let climb = start_climb(false, |config| config);
    let (at_stop, go_stop) = hold_next_rung_step(&climb.host);
    climb.fail_scheduled_run();
    at_stop
        .recv_timeout(GATE_WAIT)
        .expect("the application rung is about to stop");
    // The check before the stop is let pass; the next one is the check before the startup run.
    let (at_run, go_run) = hold_next_rung_step(&climb.host);
    go_stop.send(()).expect("the rung goes on to its stop");
    at_run
        .recv_timeout(GATE_WAIT)
        .expect("the application rung is about to run its startup package");
    assert_eq!(
        climb.state.application_count.load(Ordering::Acquire),
        1,
        "the rung stopped the application"
    );
    let before = head(&climb.host);
    let pause_request = instance_pause(POLICY_INSTANCE_ALIAS);

    let paused = thread::scope(|scope| {
        let pausing = scope.spawn(|| {
            climb.host.process_request_for_test(
                &pause_request,
                ConnectionId::new(1081).expect("connection"),
            )
        });
        wait_until("the pause holds the instance", || {
            pause_stage(&climb.host, POLICY_INSTANCE_ALIAS).is_some()
        });
        go_run.send(()).expect("the rung goes on");
        pausing
            .join()
            .expect("the pause's thread")
            .expect("the pause's receipt")
    });

    completed(&paused);
    wait_for_ladder_end(&climb.host, POLICY_INSTANCE_ALIAS);
    assert_eq!(
        package_runs_after(&climb.host, before),
        (1, 0),
        "the startup run handed off, not run"
    );
    assert_eq!(
        climb
            .host
            .instance_claims_for_test(POLICY_INSTANCE_ALIAS)
            .expect("claims"),
        (None, vec![ClaimKind::StartupPackage]),
        "the startup claim waits behind the pause"
    );
    assert_eq!(
        rung_outcome(&climb.host, RecoveryRung::ApplicationRestart),
        Some((
            RecoveryRungOutcome::Failed,
            Some(ADMISSION_DENIED.to_owned())
        ))
    );
    assert_ladder_interrupted(&climb.host);
    assert_eq!(
        climb.state.application_count.load(Ordering::Acquire),
        1,
        "nothing launched"
    );
    assert_eq!(
        pause_stage(&climb.host, POLICY_INSTANCE_ALIAS),
        Some(InstancePauseStage::Released),
        "the pause stands"
    );

    // The screen is known again and the opens pass, as PG-1's (4b) instances' do.
    climb.state.unknown_capture.store(false, Ordering::Release);
    climb
        .state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    climb.state.observed_open.store(true, Ordering::Release);
    let resumed_at = head(&climb.host);
    completed(&resume(
        &climb.host,
        instance_scope(POLICY_INSTANCE_ALIAS),
        1082,
    ));
    wait_until("the startup run after the resume and its release", || {
        let (completed_runs, failed_runs) = runs_ended_after(&climb.host, resumed_at);
        completed_runs + failed_runs > 0
            && climb
                .host
                .instance_claims_for_test(POLICY_INSTANCE_ALIAS)
                .expect("claims")
                == (None, vec![])
    });
    assert_eq!(
        package_runs_after(&climb.host, resumed_at),
        (0, 1),
        "the handed-off claim was granted and its package run once"
    );
    let (completed_runs, failed_runs) = runs_ended_after(&climb.host, resumed_at);
    assert_eq!(
        completed_runs + failed_runs,
        1,
        "the startup run ended once"
    );
    assert!(climb.host.fatal_error().expect("runtime health").is_none());
    climb.host.close().expect("close host");
}

// ---- Fix round 2 (item 4): Workflow #437 ORD-1c's grouped fixture (`gate_ord1c`, its `GROUPED`
// host and its device drill, as its t12 uses them), built here as (7) builds #411 A35's settle
// fixture. A and B are one device group of several and C is alone on its own device, each with its
// own application and the neutral startup package, whose one step restarts the application; the
// host is handed the member → device key map and the group's name as the assembly builds them, on
// a manual Runtime clock, the fixture catalog's generic task active once on A and once on B.

const GROUP_A: &str = POLICY_INSTANCE_ALIAS;
const GROUP_B: &str = POLICY_INSTANCE_ALIAS_B;
/// An instance alone on its own device, beside the group.
const GROUP_C: &str = "fixture-instance-c";
const GROUP_NAME: &str = "device.one";
const GROUP_APP_A: &str = "neutral.member.a";
const GROUP_APP_B: &str = "neutral.member.b";
const GROUP_APP_C: &str = "neutral.member.c";
/// The fake device's first pixel before any input: the startup package's home.
const GROUP_HOME: [u8; 3] = [255, 0, 0];
const GROUP_PACKAGE_ID: &str = "neutral.test.startup";
/// The response deadline of the startup package.
const GROUP_STARTUP_DEADLINE_MS: u64 = 60_000;
/// Every task's declared expected duration, and the switch cooldown the fixture's file sets.
const GROUP_MINUTE_MS: u64 = 60_000;
/// `scheduling.device_group.max_queue_wait_ms`'s default.
const GROUP_DEFAULT_LIMIT_MS: u64 = 1_800_000;

/// One task `id` of `instance` with `priority`, due at every evaluation (a local interval clock of
/// 1 ms), from the fixture catalog's generic task, declaring `GROUP_MINUTE_MS` (ORD-1c's
/// `member_task`, with no wait limit of its own).
fn group_task(base: &Value, id: &str, instance: &str, priority: u32) -> Value {
    let mut spec = base.clone();
    spec["id"] = json!(id);
    spec["scope"] = json!({"kind": "instance", "instance_id": instance});
    spec["priority"] = json!(priority);
    spec["trigger"] = json!({
        "kind": "clock",
        "schedule": {"kind": "interval", "clock_source": {"kind": "local"},
                     "every_ms": 1, "anchor_ms": 0}
    });
    spec["cooldown_ms"] = json!(0);
    spec["expected_duration_ms"] = json!(GROUP_MINUTE_MS);
    spec["instance_overrides"] = json!([]);
    spec["produces"] = json!([]);
    spec["on_failure"]["escalation_threshold"] = json!(1000);
    spec["loop_budget"] = json!({
        "daily_limit": 48,
        "window_iteration_limit": 48,
        "max_runtime_ms": 1_800_000
    });
    spec
}

/// ORD-1c's catalog (`sources` of `one_task_each`): the fixture catalog declared V2 (each timeline
/// event with its explicit unbounded validity), one task on A (priority 300) and one on B (200),
/// under its game-wide activity profile with no interval between dispatches.
fn group_sources() -> CatalogSources {
    let mut sources = policy_sources(1);
    for source in [
        &mut sources.tasks,
        &mut sources.pools,
        &mut sources.activity,
        &mut sources.timeline,
    ] {
        let mut document: Value = serde_json::from_slice(&source.bytes).expect("fixture document");
        document["schema_version"] = json!(actingcommand_policy::SCHEDULING_SCHEMA_VERSION_V2);
        if let Some(events) = document.get_mut("events").and_then(Value::as_array_mut) {
            for event in events {
                event["validity"] = json!({"from_unix_ms": 0, "until_unix_ms": null});
            }
        }
        source.bytes = serde_json::to_vec_pretty(&document).expect("V2 fixture bytes");
    }
    let mut document: Value = serde_json::from_slice(&sources.tasks.bytes).expect("fixture tasks");
    let base = document["tasks"][0].clone();
    document["tasks"] = Value::Array(vec![
        group_task(&base, "fixture.member-a", GROUP_A, 300),
        group_task(&base, "fixture.member-b", GROUP_B, 200),
    ]);
    sources.tasks.bytes = serde_json::to_vec_pretty(&document).expect("task bytes");
    let mut activity: Value =
        serde_json::from_slice(&sources.activity.bytes).expect("fixture activity");
    let mut profile = activity["profiles"][1].clone();
    profile["daily_budget"] = json!(48);
    profile["max_window_iterations"] = json!(48);
    profile["minimum_interval_ms"] = json!(1);
    profile["maximum_interval_ms"] = json!(1);
    activity["profiles"] = json!([profile]);
    sources.activity.bytes = serde_json::to_vec_pretty(&activity).expect("activity bytes");
    sources
}

/// ORD-1c's neutral startup package (an application entry): from any screen its one step restarts
/// the instance's application, to `home`, which its recognition pack knows by pixel 0.
fn group_startup_package(root: &TempDir) -> PathBuf {
    let space = json!({"width": 16, "height": 9});
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": GROUP_PACKAGE_ID,
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": space.clone(),
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 2
    });
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": space.clone(),
        "entry_page": "any",
        "target_page": "home",
        "operations": [
            {"id": "start", "from": "any", "to": "home", "application": {"action": "restart"}}
        ]
    });
    let pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": space,
        "defaults": {"color_max_distance": 0.0},
        "targets": [{"type": "color", "id": "page/home",
                     "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": GROUP_HOME}]
    });
    let pages = json!({"schema_version": "0.3", "pages": [
        {"id": "neutral/home", "required": ["page/home"], "optional": [], "forbidden": []}
    ]});
    let files: [(&str, Value); 6] = [
        ("control.json", control),
        (
            "resources/manifest.json",
            json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        ("resources/operations/task/task.json", task),
        ("resources/recognition/neutral.test.pack.json", pack),
        ("resources/recognition/neutral.test.pages.json", pages),
        (
            "resources/navigation/neutral.test.navigation.json",
            json!({"navigation": []}),
        ),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in files {
        zip.start_file(path, options).expect("zip entry");
        zip.write_all(&serde_json::to_vec(&value).expect("package JSON"))
            .expect("zip content");
    }
    let path = root.path().join("startup.zip");
    fs::write(&path, zip.finish().expect("finish zip").into_inner())
        .expect("write the startup package");
    path
}

/// One parameter of the fixture's configuration file.
fn group_parameter(key: &str, value: FactScalar, source: ConfigParameterSource) -> ConfigParameter {
    ConfigParameter {
        key: key.to_owned(),
        value,
        source,
        settable: false,
    }
}

/// ORD-1c's configuration in effect for `GROUPED`: the expected runtime off, the switch cooldown
/// a minute, and `scheduling.device_group.max_queue_wait_ms` at its default.
fn group_manifest() -> RuntimeConfigManifest {
    let mut parameters = vec![
        group_parameter(
            EXPECTED_RUNTIME_ENABLED,
            FactScalar::Boolean(false),
            ConfigParameterSource::Explicit,
        ),
        group_parameter(
            DEVICE_GROUP_SWITCH_FAILURE_COOLDOWN_MS,
            FactScalar::DurationMs(GROUP_MINUTE_MS),
            ConfigParameterSource::Explicit,
        ),
        group_parameter(
            DEVICE_GROUP_MAX_QUEUE_WAIT_MS,
            FactScalar::DurationMs(GROUP_DEFAULT_LIMIT_MS),
            ConfigParameterSource::Default,
        ),
    ];
    actingcommand_runtime_config::annotate_settable(&mut parameters)
        .expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

/// ORD-1c's `GROUPED` host, and each instance's fake device.
struct Group {
    _root: TempDir,
    host: RuntimeHost,
    states: BTreeMap<&'static str, Arc<FakeState>>,
}

/// The started host of A, B and C, A and B one device group of several, each with its own
/// application and the startup package, on the manual Runtime clock, the catalog active
/// (ORD-1c's `Bench::start(GROUPED)`).
fn start_group() -> Group {
    let root = TempDir::new().expect("tempdir");
    let package = group_startup_package(&root);
    let ids = [GROUP_A, GROUP_B, GROUP_C]
        .into_iter()
        .map(|alias| (alias, instance_id()))
        .collect::<BTreeMap<_, _>>();
    let states = [
        (GROUP_A, GROUP_APP_A),
        (GROUP_B, GROUP_APP_B),
        (GROUP_C, GROUP_APP_C),
    ]
    .into_iter()
    .map(|(alias, application)| {
        let state = Arc::new(FakeState::default());
        state.observed_open.store(true, Ordering::Release);
        state.physical_task_geometry.store(true, Ordering::Release);
        *state.application_id.lock().expect("fake application id") = Some(application.to_owned());
        (alias, state)
    })
    .collect::<BTreeMap<_, _>>();
    let mut facts = policy_facts();
    facts.outcomes.clear();
    for (alias, server, host) in [
        (GROUP_B, "fixture-server-b", "fixture-host-b"),
        (GROUP_C, "fixture-server-c", "fixture-host-c"),
    ] {
        facts.instances.push(InstanceSnapshot {
            instance_id: alias.to_owned(),
            server_id: server.to_owned(),
            game_id: "fixture-game-a".to_owned(),
            host_id: host.to_owned(),
            available: true,
            capability_operation_ids: vec!["operation.observe".to_owned()],
            preferred_task_ids: Vec::new(),
        });
    }
    let mut resources = pending_policy_resources();
    let mut host_c = resources.hosts[0].clone();
    host_c.host_id = "fixture-host-c".to_owned();
    resources.hosts.push(host_c);
    let startup = ContainedTaskRequest::new(package.to_string_lossy().into_owned())
        .and_then(|request| request.with_response_deadline_ms(GROUP_STARTUP_DEADLINE_MS))
        .expect("startup package request");
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let device = ids[GROUP_A].min(ids[GROUP_B]);
    let host_config = config(&root)
        .with_policy_inputs(PolicyInputSnapshot::new(facts, resources))
        .with_runtime_clock(clock)
        .with_startup_packages(
            [GROUP_A, GROUP_B, GROUP_C]
                .into_iter()
                .map(|alias| (alias.to_owned(), startup.clone()))
                .collect(),
        )
        .with_config_manifest(group_manifest())
        .with_device_keys(BTreeMap::from([
            (ids[GROUP_A], device),
            (ids[GROUP_B], device),
        ]))
        .with_device_group_names(BTreeMap::from([
            (GROUP_A.to_owned(), GROUP_NAME.to_owned()),
            (GROUP_B.to_owned(), GROUP_NAME.to_owned()),
        ]));
    let entries = [GROUP_A, GROUP_B, GROUP_C]
        .into_iter()
        .map(|alias| (alias.to_owned(), ids[alias], Arc::clone(&states[alias])))
        .collect::<Vec<_>>();
    let host = RuntimeHost::start(host_config, Arc::new(FakeProvider::from_entries(entries)))
        .expect("start the fixture host");
    host.activate_policy_catalog(&group_sources())
        .expect("activate the catalog");
    Group {
        _root: root,
        host,
        states,
    }
}

impl Group {
    /// Every application command that reached `alias`'s device.
    fn actions(&self, alias: &str) -> Vec<ApplicationLifecycleAction> {
        self.states[alias]
            .application_actions
            .lock()
            .expect("fake application actions")
            .clone()
    }

    /// The member the host holds in front of the group's device.
    fn front(&self) -> Option<String> {
        self.host
            .device_front_for_test(GROUP_B)
            .expect("the member in front")
    }
}

// (9) Fix round 2, item 4 (the second review's first "may change"): in a device group of several,
// a pause recorded at the application rung's boundary stops no other member. On ORD-1c's grouped
// fixture, a `launch` of A puts A in front of the device, and B's device answers A's application
// in front until its own first application command (as ORD-1c's t12 sets it, where the rung, not
// paused, stops A first). A person's device drill of B's ladder comes to the application rung,
// whose first holding check (fix round 1, item 3: just before the switch that stops the member in
// front) the test hook holds, and a person pauses B. The check sees the pause: A receives no stop
// and B none either, the device still has A in front (nothing was stopped, so nothing is
// forgotten), no startup run is handed off or run (no `startup_package_scheduled`, no
// `task.requested`), the rung failed with `recovery_admission_denied`, and the ladder finished
// interrupted (`exhausted`, Warning); the pause answers success, `released`.
#[test]
fn pg2b_9_a_pause_at_a_group_members_application_rung_stops_no_other_member() {
    let group = start_group();
    let holder = IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_holder_id()
        .expect("holder id");
    completed(&send(
        &group.host,
        RuntimeOperation::ApplicationLifecycle {
            instance_alias: GROUP_A.to_owned(),
            holder_id: *holder.transport(),
            action: ApplicationLifecycleAction::Launch,
        },
        1091,
    ));
    assert_eq!(
        group.front().as_deref(),
        Some(GROUP_A),
        "A's launch put A in front"
    );
    *group.states[GROUP_B]
        .foreground_script
        .lock()
        .expect("foreground script") = Some(Arc::new(|_: usize, commands: usize| {
        let front = if commands == 0 {
            GROUP_APP_A
        } else {
            GROUP_APP_B
        };
        Some(front.to_owned())
    }));
    group.states[GROUP_B]
        .unknown_capture
        .store(true, Ordering::Release);
    let a_before = group.actions(GROUP_A);
    let b_before = group.actions(GROUP_B);
    let (at, go) = hold_next_rung_step(&group.host);
    completed(&send(
        &group.host,
        RuntimeOperation::DrillRecoveryLadder {
            instance_alias: GROUP_B.to_owned(),
            mode: RecoveryDrillMode::Device,
        },
        1092,
    ));
    at.recv_timeout(GATE_WAIT)
        .expect("B's application rung is about to stop the member in front");
    let before = head(&group.host);
    let pause_request = instance_pause(GROUP_B);

    let paused = thread::scope(|scope| {
        let pausing = scope.spawn(|| {
            group.host.process_request_for_test(
                &pause_request,
                ConnectionId::new(1093).expect("connection"),
            )
        });
        wait_until("the pause holds B", || {
            pause_stage(&group.host, GROUP_B).is_some()
        });
        go.send(()).expect("the rung goes on");
        pausing
            .join()
            .expect("the pause's thread")
            .expect("the pause's receipt")
    });

    completed(&paused);
    wait_for_ladder_end(&group.host, GROUP_B);
    assert_eq!(
        group.actions(GROUP_A),
        a_before,
        "the other member received no stop"
    );
    assert_eq!(group.actions(GROUP_B), b_before, "the rung stopped nothing");
    assert_eq!(
        group.front().as_deref(),
        Some(GROUP_A),
        "the member in front is not forgotten"
    );
    assert_eq!(
        package_runs_after(&group.host, before),
        (0, 0),
        "no startup run handed off or run"
    );
    assert_eq!(
        rung_outcome(&group.host, RecoveryRung::ApplicationRestart),
        Some((
            RecoveryRungOutcome::Failed,
            Some(ADMISSION_DENIED.to_owned())
        ))
    );
    assert_ladder_interrupted(&group.host);
    assert_eq!(
        pause_stage(&group.host, GROUP_B),
        Some(InstancePauseStage::Released),
        "the pause stands"
    );
    assert!(group.host.fatal_error().expect("runtime health").is_none());
    group.host.close().expect("close host");
}
