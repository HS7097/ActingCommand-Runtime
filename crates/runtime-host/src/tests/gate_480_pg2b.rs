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
// (`DrillRecoveryLadder`) and two Nemu instances as PG-1's (4b). Every wait returns as soon as
// what it waits for is seen; `CLIMB_WAIT` only bounds a broken run.

use super::gate_369_s6c::{head, instance_scope, pause, resume, send};
use super::instance_workers::{
    admit_ladder_run, all_events, ladder_end_code, ladder_finished, ladder_setup, phases,
    rung_outcomes, wait_for_ladder_end, wait_until,
};
use super::*;
use actingcommand_contract::{
    EmulatorInstanceAction, FactScalar, FactValue, InstancePauseStage, RecoveryDrillMode,
    RecoveryLadderOutcome, RecoveryRung, RecoveryRungOutcome, RuntimeLifecyclePhase,
    RuntimePayload, SchedulingPauseReason, SchedulingPauseScope, StartupPackageDisposition,
};
use actingcommand_scheduler::ClaimKind;

/// Bounds a broken run's wait for the ladder (its first rung's run and settle gate come first).
const CLIMB_WAIT: Duration = Duration::from_secs(120);
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
        let _ = go_rx.recv_timeout(CLIMB_WAIT);
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
    at.recv_timeout(CLIMB_WAIT)
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
// package run (no `startup_package_scheduled`, no `task.requested`), rung 3 failed with
// `recovery_admission_denied`, the ladder finished interrupted, and the emulator saw nothing but
// that Stop and that Start.
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
    at.recv_timeout(CLIMB_WAIT)
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
        (0, 0),
        "no package run"
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
    at.recv_timeout(CLIMB_WAIT)
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
    at.recv_timeout(CLIMB_WAIT)
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
