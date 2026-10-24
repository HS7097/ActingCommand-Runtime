// SPDX-License-Identifier: AGPL-3.0-only

// emu-watch (Alice's ruling 35: starting the emulators belongs to the Runtime, a running one is
// used, a stopped one is started, and a pause holds only dispatches; #414 option 3; review item
// 5): tests (a) to (d) of the slice. On the scripted emulator (HOST-I1), a scripted connectivity
// status and the Runtime's manual clock, with the S6c helpers. Every wait returns as soon as what
// it waits for is polled, reported or recorded; `GATE_WAIT` only bounds a broken run.

use super::emulator_controller::{EmulatorCall, FAKE_EMULATOR_PORT, running_state, stopped_state};
use super::gate_369_s6c::{
    FIRST, GATE_WAIT, START, autostart_host, claims_after, connectivity, head, instance_scope,
    instance_status, of_request, pause, resume, send, starts, stop,
};
use super::instance_workers::phases;
use super::*;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    EmulatorRunningState, RuntimeLifecyclePhase, RuntimeSubscriptionRequest, SubscriptionCursor,
};
use actingcommand_host_metrics::NetworkConnectivity;
use actingcommand_scheduler::ClaimKind;

/// R2: the default polling interval (`emulator_control.watch_interval_ms`).
const WATCH_INTERVAL_MS: u64 = 60_000;
/// R3: the default restart bound, 3 restarts (`emulator_control.restart_attempts`) per 30 min
/// (`emulator_control.restart_window_ms`).
const RESTART_ATTEMPTS: u64 = 3;
const RESTART_WINDOW_MS: u64 = 1_800_000;

fn reported(lines: &[Outcome], code: &str) -> usize {
    lines
        .iter()
        .filter(|line| line.code().as_str() == code)
        .count()
}

// The report lines once `done` holds for them (none is taken).
fn await_reports(host: &RuntimeHost, done: impl Fn(&[Outcome]) -> bool) -> Vec<Outcome> {
    host.emulator_autostart_reports()
        .await_lines_for_test(done, GATE_WAIT)
        .expect("the awaited report lines")
}

// The outcomes of the `device_state_observed` records under `instance` whose code is `code`, in
// order.
fn watch_records(host: &RuntimeHost, instance: InstanceId, code: &str) -> Vec<Outcome> {
    let events = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events");
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

fn count_of(events: &[PersistedEvent], event_type: EventType) -> usize {
    events
        .iter()
        .filter(|event| event.event_type() == event_type)
        .count()
}

fn recorded(host: &RuntimeHost, event_type: EventType) -> usize {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(event_type),
        ..EventQuery::default()
    })
    .expect("events of the type")
    .len()
}

// Returns once an `event_type` is recorded after `after` (a ledger subscription that answers as
// soon as it is).
fn await_recorded(host: &RuntimeHost, event_type: EventType, after: u64) {
    let query = EventQuery {
        event_type: Some(event_type),
        from_sequence: Some(after + 1),
        ..EventQuery::default()
    };
    let subscription = RuntimeSubscriptionRequest::new(
        query,
        ProjectionProfile::Forensic,
        SubscriptionCursor::default(),
        u64::try_from(GATE_WAIT.as_millis()).expect("gate wait"),
        1,
    )
    .expect("subscription request");
    let receipt = send(
        host,
        RuntimeOperation::SubscribeEvents {
            request: subscription,
        },
        171,
    );
    let Some(RuntimeResult::EventBatch { batch }) = receipt.result() else {
        panic!("expected an event batch: {receipt:?}");
    };
    assert!(!batch.timed_out(), "{event_type:?} after {after}");
}

// The Starts the scripted emulator was asked for.
fn control_starts(state: &FakeState) -> usize {
    state
        .emulator
        .calls()
        .iter()
        .filter(|call| **call == EmulatorCall::Control(START))
        .count()
}

fn with_startup_package(
    config: RuntimeHostConfig,
    startup: &ContainedTaskRequest,
) -> RuntimeHostConfig {
    config.with_startup_packages(BTreeMap::from([(FIRST.to_owned(), startup.clone())]))
}

// A host whose one instance names `start_emulator`, on the manual clock and a connectivity status
// that reports internet access; every emulator status read finds the emulator stopped.
fn watched_host(
    root: &TempDir,
    instance: InstanceId,
    state: &Arc<FakeState>,
    clock: &Arc<ManualRuntimeClock>,
    stopped_reads: usize,
) -> RuntimeHost {
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let host = autostart_host(root, &[(FIRST, instance, state)], read, |config| {
        config.with_runtime_clock(clock.clone())
    });
    for _ in 0..stopped_reads {
        state.emulator.script_read(stopped_state());
    }
    host
}

// (a) R1: an instance whose configuration names `start_emulator` and whose own scheduling pause
// is restored at the daemon's start is started at once; the pause holds only its startup package,
// which runs after the resume.
#[test]
fn emu_w_a_a_paused_instance_is_started_and_its_startup_package_waits_for_the_resume() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance = instance_id();
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let package_path = root.path().join("startup-package.zip");
    fs::write(&package_path, neutral_contained_task_package(true)).expect("write startup package");
    let startup = ContainedTaskRequest::new(package_path.to_string_lossy().into_owned())
        .and_then(|request| request.with_response_deadline_ms(60_000))
        .expect("startup package request");
    let host = autostart_host(
        &root,
        &[(FIRST, instance, &state)],
        read.clone(),
        |config| with_startup_package(config, &startup),
    );
    stop(&host, FIRST, 301);
    assert_eq!(
        pause(&host, instance_scope(FIRST), 302).state(),
        RuntimeReceiptState::Completed
    );
    host.close().expect("close the paused host");

    let host = autostart_host(&root, &[(FIRST, instance, &state)], read, |config| {
        with_startup_package(config, &startup)
    });
    let (_, paused) = host.scheduling_pauses_for_test().expect("pauses");
    assert!(
        paused.contains_key(FIRST),
        "the instance's pause is restored"
    );
    let calls = state.emulator.listen();
    let before = head(&host);

    assert_eq!(host.enqueue_emulator_autostart().expect("enqueue"), 1);
    let lines = await_reports(&host, |lines| {
        reported(lines, "emulator_autostart_started") == 1
    });
    assert_eq!(reported(&lines, "emulator_autostart_yielded"), 0);
    assert_eq!(starts(&calls), 1, "the Start under the pause");
    let claim = of_request(&host, claims_after(&host, instance, before)[0]);
    assert_eq!(
        count_of(&claim, EventType::SchedulerQueued),
        0,
        "{claim:#?}"
    );
    assert_eq!(count_of(&claim, EventType::LeaseGranted), 1, "{claim:#?}");
    assert_eq!(
        host.instance_claims_for_test(FIRST).expect("claims"),
        (None, vec![ClaimKind::StartupPackage]),
        "only the startup package waits behind the pause"
    );
    assert_eq!(
        recorded(&host, EventType::TaskRequested),
        0,
        "no startup run under the pause"
    );

    // The resume's reconnect reads the emulator running; its release hands the key on and the
    // startup package runs.
    state
        .emulator
        .script_read(running_state(FAKE_EMULATOR_PORT));
    let resumed_at = head(&host);
    assert_eq!(
        resume(&host, instance_scope(FIRST), 303).state(),
        RuntimeReceiptState::Completed
    );
    await_recorded(&host, EventType::TaskRequested, resumed_at);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// (b) R2, R3: a poll finds the configured instance's emulator stopped while the Runtime still held
// it bound (it stopped behind the Runtime's back): the change is recorded and the emulator is
// started once on one autostart claim. A second poll during that Start starts nothing.
#[test]
fn emu_w_b_a_polled_stopped_instance_is_started_once_and_recorded() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance = instance_id();
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let host = watched_host(&root, instance, &state, &clock, 3);
    state.block_emulator_start.store(true, Ordering::Release);
    let before = head(&host);

    host.start_emulator_watch().expect("the watch starts");
    clock.advance(WATCH_INTERVAL_MS);
    host.await_emulator_watch_polls_for_test(1, GATE_WAIT)
        .expect("the first poll");
    assert_eq!(
        host.instance_claims_for_test(FIRST).expect("claims"),
        (Some(ClaimKind::Autostart), vec![]),
        "the restart holds the instance through its Start"
    );
    clock.advance(WATCH_INTERVAL_MS);
    host.await_emulator_watch_polls_for_test(2, GATE_WAIT)
        .expect("the second poll");
    assert_eq!(
        host.instance_claims_for_test(FIRST).expect("claims"),
        (Some(ClaimKind::Autostart), vec![]),
        "the second poll requests no second start"
    );
    state.block_emulator_start.store(false, Ordering::Release);

    let lines = await_reports(&host, |lines| {
        reported(lines, "emulator_autostart_started") == 1
    });
    assert_eq!(reported(&lines, "emulator_watch_restart_requested"), 1);
    assert_eq!(control_starts(&state), 1, "one Start");
    let changed = watch_records(&host, instance, "emulator_watch_state_changed");
    assert_eq!(changed.len(), 1, "{changed:?}");
    assert_eq!(changed[0].value("alias"), Some(&serde_json::json!(FIRST)));
    assert_eq!(
        changed[0].value("process_started"),
        Some(&serde_json::json!(false))
    );
    let requested = watch_records(&host, instance, "emulator_watch_restart_requested");
    assert_eq!(requested.len(), 1, "{requested:?}");
    assert_eq!(requested[0].value("alias"), Some(&serde_json::json!(FIRST)));
    assert_eq!(requested[0].value("attempts"), Some(&serde_json::json!(1)));
    assert_eq!(
        requested[0].value("limit_count"),
        Some(&serde_json::json!(RESTART_ATTEMPTS))
    );
    let claims = claims_after(&host, instance, before);
    let [claim] = claims[..] else {
        panic!("one claim: {claims:?}");
    };
    let claim = of_request(&host, claim);
    assert_eq!(
        count_of(&claim, EventType::CommandReceived),
        1,
        "{claim:#?}"
    );
    assert_eq!(
        count_of(&claim, EventType::CommandValidated),
        1,
        "{claim:#?}"
    );
    assert!(matches!(
        instance_status(&host, FIRST).emulator_state(),
        EmulatorRunningState::On { .. }
    ));
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// (c) R3: three restarts whose Starts fail within 30 min use the bound up: the next poll records
// the warning once and starts nothing, until the first restart leaves the window.
#[test]
fn emu_w_c_three_failed_starts_in_the_window_warn_and_hold_until_it_passes() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance = instance_id();
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let host = watched_host(&root, instance, &state, &clock, 32);
    state.fail_emulator_start.store(true, Ordering::Release);
    host.start_emulator_watch().expect("the watch starts");

    // Polls 1 to 3, one interval apart: each starts the stopped emulator once, and the Start fails.
    for poll in 1..=RESTART_ATTEMPTS {
        clock.advance(WATCH_INTERVAL_MS);
        host.await_emulator_watch_polls_for_test(poll, GATE_WAIT)
            .expect("the poll");
        let failed = usize::try_from(poll).expect("a count");
        await_reports(&host, |lines| {
            reported(lines, "emulator_autostart_failed") == failed
        });
    }
    assert_eq!(control_starts(&state), 3);

    // Poll 4 meets three restarts in the window: one warning and no Start.
    clock.advance(WATCH_INTERVAL_MS);
    host.await_emulator_watch_polls_for_test(4, GATE_WAIT)
        .expect("poll 4");
    let exhausted = watch_records(&host, instance, "emulator_watch_restart_exhausted");
    assert_eq!(exhausted.len(), 1, "{exhausted:?}");
    assert_eq!(exhausted[0].value("alias"), Some(&serde_json::json!(FIRST)));
    assert_eq!(exhausted[0].value("attempts"), Some(&serde_json::json!(3)));
    assert_eq!(
        exhausted[0].value("limit_count"),
        Some(&serde_json::json!(RESTART_ATTEMPTS))
    );
    // The first restart (at 60 s) leaves the window at 60 s + 30 min; poll 4 is at 240 s.
    assert_eq!(
        exhausted[0].value("retry_after_ms"),
        Some(&serde_json::json!(
            RESTART_WINDOW_MS - 3 * WATCH_INTERVAL_MS
        ))
    );
    assert_eq!(
        reported(
            &await_reports(&host, |_| true),
            "emulator_watch_restart_exhausted"
        ),
        1
    );

    // Poll 5, still in the window: no Start and no second warning.
    clock.advance(WATCH_INTERVAL_MS);
    host.await_emulator_watch_polls_for_test(5, GATE_WAIT)
        .expect("poll 5");
    assert_eq!(
        watch_records(&host, instance, "emulator_watch_restart_exhausted").len(),
        1
    );
    assert_eq!(
        watch_records(&host, instance, "emulator_watch_restart_requested").len(),
        3
    );
    assert_eq!(
        control_starts(&state),
        3,
        "no Start while the bound is used up"
    );

    // At 60 s + 30 min the first restart has left the window: the poll starts again.
    clock.advance(RESTART_WINDOW_MS - 4 * WATCH_INTERVAL_MS);
    host.await_emulator_watch_polls_for_test(6, GATE_WAIT)
        .expect("poll 6");
    await_reports(&host, |lines| {
        reported(lines, "emulator_autostart_failed") == 4
    });
    assert_eq!(control_starts(&state), 4);
    let requested = watch_records(&host, instance, "emulator_watch_restart_requested");
    assert_eq!(requested.len(), 4, "{requested:?}");
    assert_eq!(requested[3].value("attempts"), Some(&serde_json::json!(3)));
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}
