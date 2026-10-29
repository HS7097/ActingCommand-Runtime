// SPDX-License-Identifier: AGPL-3.0-only

// EMU-BIND (0.12.28; production 10-29 after the 05:23 reboot): a configured start's readiness
// wait failed while its emulator came up, the autostart tried once, and the watch never bound the
// running emulator, so the instance kept its endpoint pending for five hours. The watch now
// recognizes a running emulator whose endpoint the Runtime holds pending or bound to another
// port, on an autostart claim marked `recognize` that never launches or stops anything. On the
// scripted emulator (HOST-I1), a scripted connectivity status and the Runtime's manual clock.
// Every wait returns as soon as what it waits for is polled or reported; `GATE_WAIT` only bounds a
// broken run.
//
// Fix round 1 (0.12.28, rd-1029/brief-emu-bind-fix1.md; review M1, L2, L3): a Start that bound the
// reported port and then failed at its ADB baseline is recognized at the next poll (M1); the
// recognition claim reads the status once more on its key (L2), so each recognition below is
// scripted one more read (T1 to T4 adapted for it); the port-shift branch, the idle wake's route
// and T3's fence are pinned (L3).

use super::emulator_controller::{
    EmulatorCall, FAKE_EMULATOR_PORT, FakeEmulatorController, ScriptedControl, running_state,
    stopped_state,
};
use super::gate_369_s6c::{
    GATE_WAIT, START, autostart_host, connectivity, head, instance_status, stop,
};
use super::gate_cap_idx::{base_port, discovered_registration, fake_reader};
use super::gate_idle1p::{NOW_UNIX_MS, WAKE_UNIX_MS, no_plan, x_plan_at, x_state};
use super::instance_workers::phases;
use super::*;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{EmulatorRunningState, RuntimeLifecyclePhase, RuntimePayload};
use actingcommand_device::DeviceResourceQuiescence;
use actingcommand_host_metrics::NetworkConnectivity;

/// R2: the default polling interval (`emulator_control.watch_interval_ms`).
const WATCH_INTERVAL_MS: u64 = 60_000;
/// The bound recognitions share with restarts, counted apart: 3 (`restart_attempts`) per 30 min
/// (`restart_window_ms`).
const RESTART_ATTEMPTS: u64 = 3;
const RESTART_WINDOW_MS: u64 = 1_800_000;
const FIRST: &str = "emu_bind.first";
const SECOND: &str = "emu_bind.second";
/// Fix 1, L3: the device registry entry CAP-IDX's check runs on in the port-shift test.
const REGISTRY_ALIAS: &str = "emu-bind-shifted";

// An emulator whose process runs but whose Android has not started and which reports no port.
fn booting_state() -> crate::InstanceState {
    crate::InstanceState {
        process_started: true,
        android_started: false,
        adb_port: None,
        player_state: None,
        launch_err_code: 0,
        launch_err_msg: String::new(),
    }
}

// A discovery-bound fake whose emulator was stopped when the host started: the Runtime holds it
// pending, and its opens are refused until a port is bound (#411 A48).
fn pending_state() -> Arc<FakeState> {
    let state = Arc::new(FakeState {
        emulator: FakeEmulatorController::pending(),
        ..FakeState::default()
    });
    state.emulator_control.store(true, Ordering::Release);
    state.refuse_pending_opens.store(true, Ordering::Release);
    state.physical_task_geometry.store(true, Ordering::Release);
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    state.observed_open.store(true, Ordering::Release);
    state
}

fn reported(lines: &[Outcome], code: &str) -> usize {
    lines
        .iter()
        .filter(|line| line.code().as_str() == code)
        .count()
}

// The lines of `code` that carry `watch_kind` `recognize`.
fn recognized<'a>(lines: &'a [Outcome], code: &str) -> Vec<&'a Outcome> {
    lines
        .iter()
        .filter(|line| {
            line.code().as_str() == code
                && line.value("watch_kind") == Some(&serde_json::json!("recognize"))
        })
        .collect()
}

// The report lines once `done` holds for them (none is taken).
fn await_reports(host: &RuntimeHost, done: impl Fn(&[Outcome]) -> bool) -> Vec<Outcome> {
    host.emulator_autostart_reports()
        .await_lines_for_test(done, GATE_WAIT)
        .expect("the awaited report lines")
}

fn events(host: &RuntimeHost) -> Vec<PersistedEvent> {
    host.query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events")
}

// The outcomes of the `device_state_observed` records under `instance` whose code is `code`.
fn watch_records(host: &RuntimeHost, instance: InstanceId, code: &str) -> Vec<Outcome> {
    let events = events(host);
    phases(&events)
        .into_iter()
        .filter(|(event, _)| event.links().instance_id() == Some(&instance))
        .filter_map(|(_, phase)| match phase {
            RuntimeLifecyclePhase::DeviceStateObserved { outcome }
                if outcome.code().as_str() == code =>
            {
                Some(outcome)
            }
            _ => None,
        })
        .collect()
}

// The events of `event_type` under `instance` after `after`.
fn recorded_after(
    host: &RuntimeHost,
    instance: InstanceId,
    event_type: EventType,
    after: u64,
) -> usize {
    events(host)
        .iter()
        .filter(|event| {
            event.sequence() > after
                && event.event_type() == event_type
                && event.links().instance_id() == Some(&instance)
        })
        .count()
}

// The codes of the `runtime.failed` records under `instance`.
fn failure_codes(host: &RuntimeHost, instance: InstanceId) -> Vec<String> {
    events(host)
        .iter()
        .filter(|event| event.links().instance_id() == Some(&instance))
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::Failed(record)) => {
                Some(record.outcome().code().as_str().to_owned())
            }
            _ => None,
        })
        .collect()
}

// The startup packages whose scheduling intent was recorded for `instance`.
fn startup_packages_scheduled(host: &RuntimeHost, instance: InstanceId) -> usize {
    let events = events(host);
    phases(&events)
        .into_iter()
        .filter(|(_, phase)| {
            matches!(
                phase,
                RuntimeLifecyclePhase::StartupPackageScheduled { instance_id } if *instance_id == instance
            )
        })
        .count()
}

// The emulator control actions the scripted emulator was asked for.
fn control_calls(state: &FakeState) -> Vec<EmulatorCall> {
    state
        .emulator
        .calls()
        .into_iter()
        .filter(|call| matches!(call, EmulatorCall::Control(_)))
        .collect()
}

fn control_starts(state: &FakeState) -> usize {
    control_calls(state)
        .iter()
        .filter(|call| **call == EmulatorCall::Control(START))
        .count()
}

fn unconfirmed_adb_probe() -> DeviceError {
    DeviceError::fatal("synthetic adb probe left its child unconfirmed")
        .with_resource_summary(DeviceResourceQuiescence::Unconfirmed, 1)
}

// T1, the 10-29 case. The configured start's launch succeeded, but one `info` read of its
// readiness wait exited non-zero, so (without A, which the device gate pins) the Start failed
// with `mumu_manager_exit` and the autostart tried once. The emulator came up: the next poll
// reads it running on its port while the endpoint is pending, recognizes it on one autostart
// claim, binds the port (`runtime.instance_bound`), passes the self-check and queues the startup
// package. The Start was sent exactly once.
#[test]
fn emu_bind_t1_a_start_whose_readiness_read_failed_is_bound_at_the_next_poll() {
    let root = TempDir::new().expect("tempdir");
    let state = pending_state();
    let instance = instance_id();
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let package_path = root.path().join("startup-package.zip");
    fs::write(&package_path, neutral_contained_task_package(true)).expect("write startup package");
    let startup = ContainedTaskRequest::new(package_path.to_string_lossy().into_owned())
        .and_then(|request| request.with_response_deadline_ms(60_000))
        .expect("startup package request");
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let host = autostart_host(&root, &[(FIRST, instance, &state)], read, |config| {
        config
            .with_runtime_clock(clock.clone())
            .with_startup_packages(BTreeMap::from([(FIRST.to_owned(), startup)]))
    });
    state.emulator.script_control(
        START,
        ScriptedControl::Failed {
            stage: actingcommand_device::codes::DeviceStage::MumuManagerExit.location(),
            exit_code: Some(0),
            last_state: None,
            device_process_present: None,
        },
    );
    // The bounded read after the failed control: the VM has just come up.
    state.emulator.script_read(booting_state());

    assert_eq!(host.enqueue_emulator_autostart().expect("enqueue"), 1);
    await_reports(&host, |lines| {
        reported(lines, "emulator_autostart_failed") == 1
    });
    assert_eq!(state.emulator.bound_port(), None, "the endpoint is pending");

    // The poll's read, the claim's read on its key (fix 1, L2), then the recognition's own
    // status read.
    for _ in 0..3 {
        state
            .emulator
            .script_read(running_state(FAKE_EMULATOR_PORT));
    }
    let before = head(&host);
    host.start_emulator_watch().expect("the watch starts");
    clock.advance(WATCH_INTERVAL_MS);
    host.await_emulator_watch_polls_for_test(1, GATE_WAIT)
        .expect("the first poll");
    let lines = await_reports(&host, |lines| {
        !recognized(lines, "emulator_autostart_skipped").is_empty()
    });

    assert_eq!(control_starts(&state), 1, "the Start was sent exactly once");
    assert_eq!(reported(&lines, "emulator_autostart_preparation_failed"), 0);
    assert_eq!(
        state.emulator.bound_port(),
        Some(FAKE_EMULATOR_PORT),
        "the reported port is bound"
    );
    assert!(matches!(
        instance_status(&host, FIRST).emulator_state(),
        EmulatorRunningState::On { .. }
    ));
    assert_eq!(
        recorded_after(&host, instance, EventType::RuntimeInstanceBound, before),
        1
    );
    let queued = watch_records(&host, instance, "emulator_autostart_queued");
    let [queued] = &queued[..] else {
        panic!("one recognition queued: {queued:?}");
    };
    assert_eq!(
        queued.value("watch_kind"),
        Some(&serde_json::json!("recognize"))
    );
    assert_eq!(queued.value("attempts"), Some(&serde_json::json!(1)));
    assert_eq!(
        startup_packages_scheduled(&host, instance),
        1,
        "the startup package is queued as after a start"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// T2: an instance without `start_emulator` whose emulator was started outside the Runtime (it
// reads running while the Runtime holds it pending) is recognized and bound; nothing is started
// or stopped.
#[test]
fn emu_bind_t2_an_emulator_started_outside_the_runtime_is_recognized_and_bound() {
    let root = TempDir::new().expect("tempdir");
    let state = pending_state();
    let instance = instance_id();
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock.clone()),
        Arc::new(FakeProvider::one(FIRST, instance, Arc::clone(&state))),
    )
    .expect("runtime host started before its emulator");
    assert!(matches!(
        instance_status(&host, FIRST).emulator_state(),
        EmulatorRunningState::Off { .. }
    ));
    // The poll's read, the claim's read on its key (fix 1, L2), the recognition's read.
    for _ in 0..3 {
        state
            .emulator
            .script_read(running_state(FAKE_EMULATOR_PORT));
    }
    let before = head(&host);

    host.start_emulator_watch().expect("the watch starts");
    clock.advance(WATCH_INTERVAL_MS);
    host.await_emulator_watch_polls_for_test(1, GATE_WAIT)
        .expect("the first poll");
    let lines = await_reports(&host, |lines| {
        !recognized(lines, "emulator_autostart_skipped").is_empty()
    });

    assert_eq!(
        recognized(&lines, "emulator_autostart_skipped")[0].value("alias"),
        Some(&serde_json::json!(FIRST))
    );
    assert!(
        control_calls(&state).is_empty(),
        "nothing was started or stopped"
    );
    assert_eq!(state.emulator.bound_port(), Some(FAKE_EMULATOR_PORT));
    assert!(matches!(
        instance_status(&host, FIRST).emulator_state(),
        EmulatorRunningState::On { .. }
    ));
    assert_eq!(
        recorded_after(&host, instance, EventType::RuntimeInstanceBound, before),
        1
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// T3 (#419): two instances read running while pending. One's ADB baseline probe leaves its adb
// child unconfirmed: that fences only it, and only it records `emulator_device_unrecognized` and
// a failed recognition. The other is recognized and bound in the same poll; the Runtime goes on.
// Fix 1 (review L3): the failing instance's fence is asserted, and the other has none.
#[test]
fn emu_bind_t3_two_instances_are_recognized_independently() {
    let root = TempDir::new().expect("tempdir");
    let failing_state = pending_state();
    let state = pending_state();
    *failing_state
        .adb_probe_error
        .lock()
        .expect("adb probe error") = Some(unconfirmed_adb_probe());
    let failing = instance_id();
    let other = instance_id();
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let host = autostart_host(
        &root,
        &[(FIRST, failing, &failing_state), (SECOND, other, &state)],
        read,
        |config| config.with_runtime_clock(clock.clone()),
    );
    // Each: the poll's read, the claim's read on its key (fix 1, L2), the recognition's read.
    for fake in [&failing_state, &state] {
        for _ in 0..3 {
            fake.emulator.script_read(running_state(FAKE_EMULATOR_PORT));
        }
    }

    host.start_emulator_watch().expect("the watch starts");
    clock.advance(WATCH_INTERVAL_MS);
    host.await_emulator_watch_polls_for_test(1, GATE_WAIT)
        .expect("the first poll");
    let lines = await_reports(&host, |lines| {
        !recognized(lines, "emulator_autostart_skipped").is_empty()
            && !recognized(lines, "emulator_autostart_preparation_failed").is_empty()
    });

    let passed = recognized(&lines, "emulator_autostart_skipped");
    let failed = recognized(&lines, "emulator_autostart_preparation_failed");
    assert_eq!(passed.len(), 1, "{lines:?}");
    assert_eq!(failed.len(), 1, "{lines:?}");
    assert_eq!(passed[0].value("alias"), Some(&serde_json::json!(SECOND)));
    assert_eq!(failed[0].value("alias"), Some(&serde_json::json!(FIRST)));
    assert!(
        failure_codes(&host, failing)
            .iter()
            .any(|code| code == "emulator_device_unrecognized"),
        "{:?}",
        failure_codes(&host, failing)
    );
    assert!(
        !failure_codes(&host, other)
            .iter()
            .any(|code| code == "emulator_device_unrecognized"),
        "{:?}",
        failure_codes(&host, other)
    );
    assert!(
        instance_status(&host, FIRST).resource_fence().is_some(),
        "the unconfirmed adb child fences the failing instance"
    );
    assert_eq!(
        instance_status(&host, SECOND).resource_fence(),
        None,
        "the other instance is not fenced"
    );
    assert_eq!(state.emulator.bound_port(), Some(FAKE_EMULATOR_PORT));
    assert!(matches!(
        instance_status(&host, SECOND).emulator_state(),
        EmulatorRunningState::On { .. }
    ));
    assert!(
        control_calls(&state).is_empty(),
        "the recognized instance was neither started nor stopped"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// T4: three recognitions whose self-check fails (the device reports no ADB port by the time the
// claim reads it) use the bound up within 30 min: the fourth poll records
// `emulator_watch_restart_exhausted` with `watch_kind` `recognize` once, and neither it nor the
// fifth requests another recognition. Nothing is started.
#[test]
fn emu_bind_t4_used_up_recognitions_warn_once_and_hold_in_the_window() {
    let root = TempDir::new().expect("tempdir");
    let state = pending_state();
    let instance = instance_id();
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock.clone()),
        Arc::new(FakeProvider::one(FIRST, instance, Arc::clone(&state))),
    )
    .expect("runtime host started before its emulator");
    // Each recognition: the poll's read, the claim's read on its key (fix 1, L2), then the
    // self-check's read, which finds no port.
    for _ in 0..RESTART_ATTEMPTS {
        state
            .emulator
            .script_read(running_state(FAKE_EMULATOR_PORT));
        state
            .emulator
            .script_read(running_state(FAKE_EMULATOR_PORT));
        state.emulator.script_read(booting_state());
    }
    for _ in 0..2 {
        state
            .emulator
            .script_read(running_state(FAKE_EMULATOR_PORT));
    }
    host.start_emulator_watch().expect("the watch starts");

    // Polls 1 to 3, one interval apart: each requests one recognition, which fails.
    for poll in 1..=RESTART_ATTEMPTS {
        clock.advance(WATCH_INTERVAL_MS);
        host.await_emulator_watch_polls_for_test(poll, GATE_WAIT)
            .expect("the poll");
        let failed = usize::try_from(poll).expect("a count");
        await_reports(&host, |lines| {
            recognized(lines, "emulator_autostart_preparation_failed").len() == failed
        });
    }

    // Polls 4 and 5 meet three recognitions in the window: one warning, no fourth recognition.
    for poll in 4..=5 {
        clock.advance(WATCH_INTERVAL_MS);
        host.await_emulator_watch_polls_for_test(poll, GATE_WAIT)
            .expect("the poll");
    }
    let exhausted = watch_records(&host, instance, "emulator_watch_restart_exhausted");
    let [exhausted] = &exhausted[..] else {
        panic!("one used-up record: {exhausted:?}");
    };
    assert_eq!(
        exhausted.value("watch_kind"),
        Some(&serde_json::json!("recognize"))
    );
    assert_eq!(exhausted.value("attempts"), Some(&serde_json::json!(3)));
    assert_eq!(
        exhausted.value("limit_count"),
        Some(&serde_json::json!(RESTART_ATTEMPTS))
    );
    // The first recognition (at 60 s) leaves the window at 60 s + 30 min; poll 4 is at 240 s.
    assert_eq!(
        exhausted.value("retry_after_ms"),
        Some(&serde_json::json!(
            RESTART_WINDOW_MS - 3 * WATCH_INTERVAL_MS
        ))
    );
    let queued = watch_records(&host, instance, "emulator_autostart_queued");
    assert_eq!(queued.len(), 3, "{queued:?}");
    assert!(
        queued
            .iter()
            .all(|queued| queued.value("watch_kind") == Some(&serde_json::json!("recognize"))),
        "counted apart from restarts: {queued:?}"
    );
    let lines = await_reports(&host, |_| true);
    assert_eq!(
        recognized(&lines, "emulator_autostart_preparation_failed").len(),
        3
    );
    assert_eq!(reported(&lines, "emulator_watch_restart_exhausted"), 1);
    assert_eq!(
        host.instance_claims_for_test(FIRST).expect("claims"),
        (None, vec![]),
        "no claim in the window"
    );
    assert!(control_calls(&state).is_empty(), "nothing was started");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// T5: a watch claim (R3: the poll read the configured instance stopped) whose read just before its
// Start finds the emulator running while the Runtime holds the endpoint pending no longer ends as
// a plain `emulator_autostart_skipped`: it recognizes the emulator and binds the reported port,
// and no Start is sent.
#[test]
fn emu_bind_t5_a_watch_claim_that_finds_the_emulator_running_while_pending_recognizes_it() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance = instance_id();
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let host = autostart_host(&root, &[(FIRST, instance, &state)], read, |config| {
        config.with_runtime_clock(clock.clone())
    });
    stop(&host, FIRST, 301);
    assert_eq!(state.emulator.bound_port(), None, "the endpoint is pending");
    // The poll's read, the claim's read just before its Start, the recognition's read.
    state.emulator.script_read(stopped_state());
    state
        .emulator
        .script_read(running_state(FAKE_EMULATOR_PORT));
    state
        .emulator
        .script_read(running_state(FAKE_EMULATOR_PORT));

    host.start_emulator_watch().expect("the watch starts");
    clock.advance(WATCH_INTERVAL_MS);
    host.await_emulator_watch_polls_for_test(1, GATE_WAIT)
        .expect("the first poll");
    let lines = await_reports(&host, |lines| {
        reported(lines, "emulator_autostart_skipped") == 1
    });

    assert_eq!(
        recognized(&lines, "emulator_autostart_skipped").len(),
        1,
        "the skipped claim recognized the emulator: {lines:?}"
    );
    assert_eq!(reported(&lines, "emulator_autostart_failed"), 0);
    assert_eq!(control_starts(&state), 0, "no Start");
    assert_eq!(state.emulator.bound_port(), Some(FAKE_EMULATOR_PORT));
    let queued = watch_records(&host, instance, "emulator_autostart_queued");
    let [queued] = &queued[..] else {
        panic!("one restart queued: {queued:?}");
    };
    assert_eq!(
        queued.value("watch_kind"),
        Some(&serde_json::json!("restart"))
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// Fix 1, M1 (0.12.28): a configured Start binds the reported port, drops the self-check facts and
// then fails at its ADB baseline (`emulator_control_adb_not_ready`): the instance stays bound to
// the right port and `on`, with no self-check fact, no startup package and no ladder. The next
// poll recognizes it (bound to the reported port, unprepared, its key idle): the self-check
// passes, the startup package is queued once, and the Start was sent once.
#[test]
fn emu_bind_m1_a_start_that_bound_and_failed_at_its_adb_baseline_is_recognized_at_the_next_poll() {
    let root = TempDir::new().expect("tempdir");
    let state = pending_state();
    let instance = instance_id();
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let package_path = root.path().join("startup-package.zip");
    fs::write(&package_path, neutral_contained_task_package(true)).expect("write startup package");
    let startup = ContainedTaskRequest::new(package_path.to_string_lossy().into_owned())
        .and_then(|request| request.with_response_deadline_ms(60_000))
        .expect("startup package request");
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let host = autostart_host(&root, &[(FIRST, instance, &state)], read, |config| {
        config
            .with_runtime_clock(clock.clone())
            .with_startup_packages(BTreeMap::from([(FIRST.to_owned(), startup)]))
            // The Start's ADB baseline wait ends at once instead of after 30 s.
            .with_adb_baseline_wait_for_test(Duration::ZERO)
    });
    // adbd stays silent through the Start's ADB baseline wait.
    *state.adb_probe_error.lock().expect("adb probe error") =
        Some(DeviceError::fatal("synthetic adb probe found no device"));

    // The Start: the launch reports the emulator running on its port (unscripted), which is
    // bound; the ADB baseline then fails the Start.
    assert_eq!(host.enqueue_emulator_autostart().expect("enqueue"), 1);
    await_reports(&host, |lines| {
        reported(lines, "emulator_autostart_failed") == 1
    });
    assert!(
        failure_codes(&host, instance)
            .iter()
            .any(|code| code == "emulator_control_adb_not_ready"),
        "{:?}",
        failure_codes(&host, instance)
    );
    assert_eq!(
        state.emulator.bound_port(),
        Some(FAKE_EMULATOR_PORT),
        "the Start bound the reported port"
    );
    assert_eq!(
        startup_packages_scheduled(&host, instance),
        0,
        "the failed Start queued no startup package"
    );

    // adbd answers by the next poll. The poll's read, the claim's read on its key, the
    // self-check's read.
    *state.adb_probe_error.lock().expect("adb probe error") = None;
    for _ in 0..3 {
        state
            .emulator
            .script_read(running_state(FAKE_EMULATOR_PORT));
    }
    host.start_emulator_watch().expect("the watch starts");
    clock.advance(WATCH_INTERVAL_MS);
    host.await_emulator_watch_polls_for_test(1, GATE_WAIT)
        .expect("the first poll");
    let lines = await_reports(&host, |lines| {
        !recognized(lines, "emulator_autostart_skipped").is_empty()
    });

    assert_eq!(control_starts(&state), 1, "the Start was sent once");
    assert_eq!(reported(&lines, "emulator_autostart_preparation_failed"), 0);
    let passed = recognized(&lines, "emulator_autostart_skipped");
    let [passed] = &passed[..] else {
        panic!("one recognition: {lines:?}");
    };
    assert_eq!(
        passed.value("process_started"),
        None,
        "the recognition ran (no unneeded skip)"
    );
    let queued = watch_records(&host, instance, "emulator_autostart_queued");
    let [queued] = &queued[..] else {
        panic!("one recognition queued: {queued:?}");
    };
    assert_eq!(
        queued.value("watch_kind"),
        Some(&serde_json::json!("recognize"))
    );
    assert_eq!(state.emulator.bound_port(), Some(FAKE_EMULATOR_PORT));
    assert!(matches!(
        instance_status(&host, FIRST).emulator_state(),
        EmulatorRunningState::On { .. }
    ));
    assert_eq!(
        startup_packages_scheduled(&host, instance),
        1,
        "the startup package is queued once"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// Fix 1, L3: the port-shift branch. The instance is bound through discovery on its index's base
// port and prepared; MuMu relaunched it on the next port behind the Runtime's back. The poll reads
// it running on the shifted port: the recognition rebinds the endpoint (`runtime.instance_bound`
// with the shifted port and the binding's index) and opens capture again on it; nothing is
// started or stopped. CAP-IDX's check accepts that binding: the device registry's capture takes
// the index from it and finds the shifted port reported for the index.
#[test]
fn emu_bind_t2b_a_shifted_port_is_rebound_and_capture_opens_through_the_port_check() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.emulator_control.store(true, Ordering::Release);
    state.physical_task_geometry.store(true, Ordering::Release);
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    state.observed_open.store(true, Ordering::Release);
    let instance = instance_id();
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock.clone()),
        Arc::new(FakeProvider::one(FIRST, instance, Arc::clone(&state))),
    )
    .expect("runtime host started with its emulator bound");
    assert_eq!(
        base_port(FAKE_EMULATOR_INDEX),
        FAKE_EMULATOR_PORT,
        "the fake is bound on its index's base port"
    );
    let shifted = FAKE_EMULATOR_PORT + 1;
    // The poll's read, the claim's read on its key, the recognition's read.
    for _ in 0..3 {
        state.emulator.script_read(running_state(shifted));
    }
    let captures_before = state.capture_open_count.load(Ordering::Acquire);
    let before = head(&host);

    host.start_emulator_watch().expect("the watch starts");
    clock.advance(WATCH_INTERVAL_MS);
    host.await_emulator_watch_polls_for_test(1, GATE_WAIT)
        .expect("the first poll");
    let lines = await_reports(&host, |lines| {
        !recognized(lines, "emulator_autostart_skipped").is_empty()
    });

    assert_eq!(reported(&lines, "emulator_autostart_preparation_failed"), 0);
    assert!(
        control_calls(&state).is_empty(),
        "nothing was started or stopped"
    );
    assert_eq!(
        state.emulator.bound_port(),
        Some(shifted),
        "rebound to the shifted port"
    );
    assert_eq!(instance_status(&host, FIRST).adb_port(), Some(shifted));
    assert!(
        state.capture_open_count.load(Ordering::Acquire) > captures_before,
        "capture opened again on the rebound endpoint"
    );
    let bindings = events(&host)
        .into_iter()
        .filter(|event| {
            event.sequence() > before
                && event.event_type() == EventType::RuntimeInstanceBound
                && event.links().instance_id() == Some(&instance)
        })
        .collect::<Vec<_>>();
    let [bound] = &bindings[..] else {
        panic!("one rebinding: {bindings:?}");
    };
    let EventPayload::Runtime(RuntimePayload::InstanceBound(binding)) = bound.payload() else {
        panic!("an instance binding: {bound:?}");
    };
    assert_eq!(binding.adb_port(), Some(shifted));
    assert_eq!(
        binding.discovered_instance_index(),
        Some(FAKE_EMULATOR_INDEX)
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");

    // CAP-IDX on the binding the recognition recorded: capture takes the binding's index, reads
    // `info` for it once and opens on the shifted port it reports.
    let reads = Arc::new(std::sync::Mutex::new(Vec::new()));
    let registry = ExecutionBackendRegistry::new([discovered_registration(
        REGISTRY_ALIAS,
        FAKE_EMULATOR_INDEX,
    )])
    .expect("registry")
    .with_instance_state_reader(fake_reader(
        BTreeMap::from([(FAKE_EMULATOR_INDEX, shifted)]),
        Arc::clone(&reads),
    ));
    registry
        .rebind_discovered_endpoint(REGISTRY_ALIAS, binding.adb_port())
        .expect("the recognition's binding");
    let capture = registry
        .capture_for_open(REGISTRY_ALIAS)
        .expect("capture on the shifted port");
    assert_eq!(capture.target.port, shifted);
    assert_eq!(
        capture.nemu.instance_id,
        Some(i32::from(FAKE_EMULATOR_INDEX))
    );
    assert_eq!(*reads.lock().expect("reads"), vec![FAKE_EMULATOR_INDEX]);
}

// Fix 1, L3: the idle wake's route. The idle action `stop_emulator` stopped the instance's
// emulator, so its endpoint is pending; a person then started the emulator outside the Runtime.
// At the wake, its configured emulator start reads the emulator running while pending: it sends
// no Start and recognizes it instead (the reported port bound, the self-check passed), and the
// wake's startup package is queued once.
#[test]
fn emu_bind_t5b_an_idle_wake_claim_that_finds_the_emulator_running_while_pending_recognizes_it() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(NOW_UNIX_MS, 0));
    let x = instance_id();
    let state = x_state();
    let host = super::gate_idle1p::start(&root, &clock, x, &state, false).expect("the start");
    host.check_idle_instances_for_test(&x_plan_at(NOW_UNIX_MS))
        .expect("the idle check");
    assert_eq!(
        host.await_idle_action_ends_for_test(1, GATE_WAIT),
        Some(1),
        "the idle action ended"
    );
    assert_eq!(
        state.emulator.bound_port(),
        None,
        "the idle stop left the endpoint pending"
    );

    // A person starts the emulator outside the Runtime. The wake's claim reads it running on its
    // port, then the recognition's self-check reads it.
    state.android_down.store(false, Ordering::Release);
    for _ in 0..2 {
        state
            .emulator
            .script_read(running_state(FAKE_EMULATOR_PORT));
    }
    let before = head(&host);
    clock.set_unix_ms(WAKE_UNIX_MS);
    host.check_idle_instances_for_test(&no_plan())
        .expect("the check at the wake");
    let lines = await_reports(&host, |lines| {
        !recognized(lines, "emulator_autostart_skipped").is_empty()
    });

    assert_eq!(control_starts(&state), 0, "no Start");
    assert_eq!(reported(&lines, "emulator_autostart_preparation_failed"), 0);
    assert_eq!(state.emulator.bound_port(), Some(FAKE_EMULATOR_PORT));
    assert_eq!(
        recorded_after(&host, x, EventType::RuntimeInstanceBound, before),
        1
    );
    let all = events(&host);
    let scheduled = phases(&all)
        .into_iter()
        .filter(|(event, phase)| {
            event.sequence() > before
                && matches!(
                    phase,
                    RuntimeLifecyclePhase::StartupPackageScheduled { instance_id } if *instance_id == x
                )
        })
        .count();
    assert_eq!(scheduled, 1, "the wake's startup package is queued once");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}
