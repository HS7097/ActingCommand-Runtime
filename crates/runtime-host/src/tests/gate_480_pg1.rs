// SPDX-License-Identifier: AGPL-3.0-only

// #480 PG-1 (0.12.72; register §1.468; the owner's requirement on board #480, 11-17: after "pause
// all" the Runtime starts no emulator and launches no game by itself, and a pause she presses
// stands). Production v0.12.70 on 11-17: under the global pause the emulator watch restarted the
// one instance that carried no pause of its own, and the instance pause a person then pressed was
// lifted again when the watch's restart, which held the instance through its readiness wait,
// ended. Under the global pause every automatic emulator start is held as under the instance's
// own pause (PAUSE-AUTO), its skip `emulator_autostart_skipped` with `skip_reason`
// `global_paused`, once per global pause revision, start and instance (the instance's own
// `instance_paused` where both pauses hold); after the global resume all behave as before. An
// instance pause waits beyond its grace for an automatic start's claim that holds its instance,
// up to that claim's own deadline and the grace after it, and fails for anything else as before.
// On the scripted emulator (HOST-I1), a scripted connectivity status and, where time moves, the
// Runtime's manual clock, with the S6c and IDLE-1P helpers. Every wait returns as soon as what it
// waits for is polled, reported, recorded or seen; `GATE_WAIT` only bounds a broken run.

use super::emulator_controller::{
    EmulatorCall, FAKE_EMULATOR_PORT, FakeEmulatorController, running_state, stopped_state,
};
use super::gate_369_s6c::{
    GATE_WAIT, START, autostart_host, claims_after, connectivity, head, instance_scope, of_request,
    pause, resume, send, stop,
};
use super::gate_idle1p::{
    NOW_UNIX_MS, WAKE_UNIX_MS, X_ALIAS, emulator_starts, idle_records, kept_rows, no_plan, start,
    stops, wake_records, wake_skips, x_plan_at, x_row, x_state,
};
use super::instance_workers::{all_events, phases, wait_until};
use super::*;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    FactScalar, FactValue, InstancePauseStage, RuntimeLifecyclePhase, RuntimePayload,
    SchedulingPauseReason, SchedulingPauseScope,
};
use actingcommand_host_metrics::NetworkConnectivity;
use serde_json::json;

const FIRST: &str = "pg1.first";
const SECOND: &str = "pg1.second";
/// R2: the default polling interval of the emulator watch (`emulator_control.watch_interval_ms`).
const WATCH_INTERVAL_MS: u64 = 60_000;
/// R3: the default restart bound (`emulator_control.restart_attempts`).
const RESTART_ATTEMPTS: u64 = 3;
/// The runtime fact of the held pauses (`host.scheduling_pause`).
const PAUSE_FACT_KEY: &str = "host.scheduling_pause";
const GLOBAL_PAUSED: &str = "global_paused";
const INSTANCE_PAUSED: &str = "instance_paused";
/// How far test (9) moves the manual clock: past an autostart claim's key (renewed to 225 s before
/// its Start's provider step) and the 30 s grace after it.
const OUTLASTED_MS: u64 = 10 * 60_000;

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

// Whether a pause's skip has been reported.
fn skip_reported(lines: &[Outcome]) -> bool {
    lines.iter().any(|line| {
        line.code().as_str() == "emulator_autostart_skipped" && line.value("skip_reason").is_some()
    })
}

// Whether a configured start that sent its Start has ended (one of its report lines).
fn start_ended(lines: &[Outcome]) -> bool {
    [
        "emulator_autostart_started",
        "emulator_autostart_preparation_failed",
        "emulator_autostart_failed",
    ]
    .iter()
    .any(|code| reported(lines, code) > 0)
}

fn completed(receipt: &RuntimeReceipt) {
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
}

fn count(events: &[PersistedEvent], event_type: EventType) -> usize {
    events
        .iter()
        .filter(|event| event.event_type() == event_type)
        .count()
}

// The outcomes of the `device_state_observed` records under `instance` whose code is `code`, in
// order.
fn watch_records(host: &RuntimeHost, instance: InstanceId, code: &str) -> Vec<Outcome> {
    let events = all_events(host);
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

// The `emulator_autostart_skipped` records under `instance` that a pause made, in order.
fn paused_skips(host: &RuntimeHost, instance: InstanceId) -> Vec<Outcome> {
    watch_records(host, instance, "emulator_autostart_skipped")
        .into_iter()
        .filter(|outcome| outcome.value("skip_reason").is_some())
        .collect()
}

// A pause's skip: its word (`skip_reason`), and the value that names the start (the watch's
// `watch_kind`, the idle wake's `idle_action`, neither for the daemon start's).
fn assert_skip(skip: &Outcome, watch_kind: Option<&str>, idle_action: Option<&str>, word: &str) {
    assert_eq!(skip.value("skip_reason"), Some(&json!(word)), "{skip:?}");
    assert_eq!(
        skip.value("watch_kind"),
        watch_kind.map(|kind| json!(kind)).as_ref(),
        "{skip:?}"
    );
    assert_eq!(
        skip.value("idle_action"),
        idle_action.map(|action| json!(action)).as_ref(),
        "{skip:?}"
    );
}

// The events of `event_type` under `instance` after `after`.
fn recorded_after(
    host: &RuntimeHost,
    instance: InstanceId,
    event_type: EventType,
    after: u64,
) -> usize {
    all_events(host)
        .iter()
        .filter(|event| {
            event.sequence() > after
                && event.event_type() == event_type
                && event.links().instance_id() == Some(&instance)
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

// The Starts the scripted emulator was asked for.
fn control_starts(state: &FakeState) -> usize {
    control_calls(state)
        .iter()
        .filter(|call| **call == EmulatorCall::Control(START))
        .count()
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

// X closed by its idle action `stop_emulator` (its next task four hours away), as the first start
// of `gate_idle1p` closes it: the Stop sent, X kept closed with its wake at `WAKE_UNIX_MS`.
fn close_x(host: &RuntimeHost, state: &FakeState) {
    host.check_idle_instances_for_test(&x_plan_at(NOW_UNIX_MS))
        .expect("the idle check");
    assert_eq!(
        host.await_idle_action_ends_for_test(1, GATE_WAIT),
        Some(1),
        "X's idle action ended"
    );
    assert_eq!(stops(state), 1, "X's action asked for its Stop");
    assert_eq!(
        kept_rows(host),
        Some(vec![x_row(Some(WAKE_UNIX_MS))]),
        "X is kept closed with its wake"
    );
}

// A person's pause of `alias`, with the smallest drain timeout a request may name.
fn instance_pause(alias: &str) -> RuntimeRequest {
    runtime_request(
        &IdentifierIssuer::new().expect("identifier issuer"),
        RuntimeOperation::PauseScheduling {
            scope: instance_scope(alias),
            pause_reason: SchedulingPauseReason::new("pg1_pause").expect("pause reason"),
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

// (1) Ruling 1 and 2, the watch's restart: under the global pause a configured instance with no
// pause of its own is found stopped at every poll, one poll more than the restart bound allows. No
// restart claim, no Start, no queued or used-up record, and one skip, `skip_reason`
// `global_paused` with `watch_kind` `restart`, for the global pause's revision; the polls go on and
// record the reading.
#[test]
fn pg1_1_under_the_global_pause_the_watch_restarts_nothing_and_records_one_skip() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance = instance_id();
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let host = autostart_host(&root, &[(FIRST, instance, &state)], read, |config| {
        config.with_runtime_clock(clock.clone())
    });
    completed(&pause(&host, SchedulingPauseScope::Global, 801));
    let before = head(&host);
    host.start_emulator_watch().expect("the watch starts");

    for poll in 1..=RESTART_ATTEMPTS + 1 {
        state.emulator.script_read(stopped_state());
        clock.advance(WATCH_INTERVAL_MS);
        host.await_emulator_watch_polls_for_test(poll, GATE_WAIT)
            .expect("the poll");
        assert!(
            claims_after(&host, instance, before).is_empty(),
            "no restart claim at poll {poll}"
        );
    }
    let skips = paused_skips(&host, instance);
    let [skip] = &skips[..] else {
        panic!("one skip for the global pause's revision: {skips:?}");
    };
    assert_eq!(skip.value("alias"), Some(&json!(FIRST)));
    assert_skip(skip, Some("restart"), None, GLOBAL_PAUSED);
    assert_eq!(
        reported(
            &await_reports(&host, |_| true),
            "emulator_autostart_skipped"
        ),
        1,
        "the skip is one report line"
    );
    assert!(watch_records(&host, instance, "emulator_autostart_queued").is_empty());
    assert!(
        watch_records(&host, instance, "emulator_watch_restart_exhausted").is_empty(),
        "the skips use up no bound"
    );
    assert_eq!(
        watch_records(&host, instance, "emulator_watch_state_changed").len(),
        1,
        "the polls go on and record the reading"
    );
    assert_eq!(control_starts(&state), 0, "no Start under the global pause");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// (1) The watch's recognition, its first path: under the global pause an instance whose emulator
// runs with Android and its port while the Runtime holds its endpoint pending gets no recognition
// claim at any poll, one skip (`watch_kind` `recognize`, `global_paused`), and stays pending;
// nothing is started or stopped.
#[test]
fn pg1_1_under_the_global_pause_the_watch_recognizes_no_pending_instance() {
    let root = TempDir::new().expect("tempdir");
    let state = pending_state();
    let instance = instance_id();
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock.clone()),
        Arc::new(FakeProvider::one(FIRST, instance, Arc::clone(&state))),
    )
    .expect("runtime host started before its emulator");
    completed(&pause(&host, SchedulingPauseScope::Global, 811));
    let before = head(&host);
    host.start_emulator_watch().expect("the watch starts");

    for poll in 1..=RESTART_ATTEMPTS + 1 {
        state
            .emulator
            .script_read(running_state(FAKE_EMULATOR_PORT));
        clock.advance(WATCH_INTERVAL_MS);
        host.await_emulator_watch_polls_for_test(poll, GATE_WAIT)
            .expect("the poll");
        assert!(
            claims_after(&host, instance, before).is_empty(),
            "no recognition claim at poll {poll}"
        );
    }
    let skips = paused_skips(&host, instance);
    let [skip] = &skips[..] else {
        panic!("one skip for the global pause's revision: {skips:?}");
    };
    assert_skip(skip, Some("recognize"), None, GLOBAL_PAUSED);
    assert!(watch_records(&host, instance, "emulator_autostart_queued").is_empty());
    assert!(watch_records(&host, instance, "emulator_watch_restart_exhausted").is_empty());
    assert_eq!(
        state.emulator.bound_port(),
        None,
        "the endpoint stays pending"
    );
    assert_eq!(
        recorded_after(&host, instance, EventType::RuntimeInstanceBound, before),
        0
    );
    assert!(
        control_calls(&state).is_empty(),
        "nothing started or stopped"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// (1) The watch's recognition, its second path (EMU-BIND fix 1): a configured Start bound the
// reported port and then failed at its ADB baseline, so the instance is bound to that port with no
// self-check fact. Under the global pause the polls that read it running there request no
// recognition claim, and one skip (`watch_kind` `recognize`, `global_paused`) says so.
#[test]
fn pg1_1_under_the_global_pause_the_watch_recognizes_no_unprepared_instance() {
    let root = TempDir::new().expect("tempdir");
    let state = pending_state();
    let instance = instance_id();
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let host = autostart_host(&root, &[(FIRST, instance, &state)], read, |config| {
        config
            .with_runtime_clock(clock.clone())
            // The Start's ADB baseline wait ends at once instead of after 30 s.
            .with_adb_baseline_wait_for_test(Duration::ZERO)
    });
    // adbd stays silent through the Start's ADB baseline wait.
    *state.adb_probe_error.lock().expect("adb probe error") =
        Some(DeviceError::fatal("synthetic adb probe found no device"));
    assert_eq!(host.enqueue_emulator_autostart().expect("enqueue"), 1);
    await_reports(&host, |lines| {
        reported(lines, "emulator_autostart_failed") == 1
    });
    assert_eq!(
        state.emulator.bound_port(),
        Some(FAKE_EMULATOR_PORT),
        "the Start bound the reported port"
    );
    *state.adb_probe_error.lock().expect("adb probe error") = None;
    completed(&pause(&host, SchedulingPauseScope::Global, 821));
    let before = head(&host);
    host.start_emulator_watch().expect("the watch starts");

    for poll in 1..=2 {
        state
            .emulator
            .script_read(running_state(FAKE_EMULATOR_PORT));
        clock.advance(WATCH_INTERVAL_MS);
        host.await_emulator_watch_polls_for_test(poll, GATE_WAIT)
            .expect("the poll");
        assert!(
            claims_after(&host, instance, before).is_empty(),
            "no recognition claim at poll {poll}"
        );
    }
    let skips = paused_skips(&host, instance);
    let [skip] = &skips[..] else {
        panic!("one skip for the global pause's revision: {skips:?}");
    };
    assert_skip(skip, Some("recognize"), None, GLOBAL_PAUSED);
    assert!(watch_records(&host, instance, "emulator_autostart_queued").is_empty());
    assert_eq!(
        recorded_after(&host, instance, EventType::RuntimeInstanceBound, before),
        0
    );
    assert_eq!(control_starts(&state), 1, "the daemon start's Start alone");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// (2) A daemon start under a restored global pause: of two configured instances whose emulators
// are stopped, neither gets a configured start (none counted, no claim, no Start), and each records
// its skip once (`global_paused`, neither `watch_kind` nor `idle_action`).
#[test]
fn pg1_2_a_daemon_start_under_a_restored_global_pause_requests_no_configured_start() {
    let root = TempDir::new().expect("tempdir");
    let first_state = Arc::new(FakeState::default());
    let second_state = Arc::new(FakeState::default());
    let (first, second) = (instance_id(), instance_id());
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let instances = [
        (FIRST, first, &first_state),
        (SECOND, second, &second_state),
    ];
    let host = autostart_host(&root, &instances, read.clone(), |config| config);
    stop(&host, FIRST, 831);
    stop(&host, SECOND, 832);
    completed(&pause(&host, SchedulingPauseScope::Global, 833));
    host.close().expect("close the paused host");

    let host = autostart_host(&root, &instances, read, |config| config);
    let (global, own) = host.scheduling_pauses_for_test().expect("the held pauses");
    assert!(global.is_some(), "the global pause is restored");
    assert!(own.is_empty(), "no instance carries a pause of its own");
    let before = head(&host);

    assert_eq!(
        host.enqueue_emulator_autostart().expect("enqueue"),
        0,
        "no configured start is requested"
    );
    let lines = await_reports(&host, |lines| {
        reported(lines, "emulator_autostart_skipped") == 2
    });
    let queued = lines
        .iter()
        .filter(|line| line.code().as_str() == "emulator_autostart_queued")
        .collect::<Vec<_>>();
    let [queued] = &queued[..] else {
        panic!("one count: {lines:?}");
    };
    assert_eq!(queued.value("count"), Some(&json!(0)));
    for (alias, instance, state) in instances {
        assert!(
            claims_after(&host, instance, before).is_empty(),
            "no claim for {alias}"
        );
        assert_eq!(control_starts(state), 0, "no Start of {alias}");
        let skips = paused_skips(&host, instance);
        let [skip] = &skips[..] else {
            panic!("one skip of {alias}: {skips:?}");
        };
        assert_eq!(skip.value("alias"), Some(&json!(alias)));
        assert_skip(skip, None, None, GLOBAL_PAUSED);
    }
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// (3) A claim requested before the global pause began: the watch's restart claim of a configured
// instance found stopped holds its key, waiting for the network, when the global pause comes. At
// its next tick it releases the key with no command record and no failure pair, starts nothing and
// records the skip (`watch_kind` `restart`, `global_paused`); a poll under the same pause requests
// nothing and records no second skip. After the global resume the next poll restarts the instance
// as the first restart of its window: the skipped request was given back.
#[test]
fn pg1_3_a_watch_restart_requested_before_the_global_pause_ends_on_its_key_refunded() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance = instance_id();
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    // The first restart's claim waits without internet access; the one after the resume has it.
    let (read, reads) = connectivity(
        vec![NetworkConnectivity::NoInternet],
        NetworkConnectivity::Internet,
    );
    let host = autostart_host(&root, &[(FIRST, instance, &state)], read, |config| {
        config.with_runtime_clock(clock.clone())
    });
    let before = head(&host);
    host.start_emulator_watch().expect("the watch starts");

    // The first poll finds the emulator stopped behind the Runtime's back: its restart claim.
    state.emulator.script_read(stopped_state());
    clock.advance(WATCH_INTERVAL_MS);
    host.await_emulator_watch_polls_for_test(1, GATE_WAIT)
        .expect("the first poll");
    assert_eq!(
        reads
            .recv_timeout(GATE_WAIT)
            .expect("the claim's first read"),
        NetworkConnectivity::NoInternet
    );
    let [claim] = claims_after(&host, instance, before)[..] else {
        panic!("one restart claim");
    };

    // The global pause answers at once; the claim's next tick gives the key up.
    completed(&pause(&host, SchedulingPauseScope::Global, 841));
    let paused_at = head(&host);
    let lines = await_reports(&host, skip_reported);
    assert_eq!(reported(&lines, "emulator_autostart_failed"), 0);
    assert_eq!(reported(&lines, "emulator_autostart_started"), 0);
    let claim = of_request(&host, claim);
    assert_eq!(count(&claim, EventType::LeaseGranted), 1, "{claim:#?}");
    assert_eq!(count(&claim, EventType::LeaseReleased), 1, "{claim:#?}");
    for event_type in [
        EventType::CommandReceived,
        EventType::CommandRejected,
        EventType::RuntimeFailed,
    ] {
        assert_eq!(count(&claim, event_type), 0, "{event_type:?}: {claim:#?}");
    }
    assert_eq!(control_starts(&state), 0, "no Start");
    let skips = paused_skips(&host, instance);
    let [skip] = &skips[..] else {
        panic!("one skip on the claim's key: {skips:?}");
    };
    assert_skip(skip, Some("restart"), None, GLOBAL_PAUSED);

    // A poll under the same pause finds the emulator stopped again: no claim, no second skip.
    state.emulator.script_read(stopped_state());
    clock.advance(WATCH_INTERVAL_MS);
    host.await_emulator_watch_polls_for_test(2, GATE_WAIT)
        .expect("a poll under the pause");
    assert!(
        claims_after(&host, instance, paused_at).is_empty(),
        "no restart claim under the pause"
    );
    assert_eq!(paused_skips(&host, instance).len(), 1);

    // The global resume reconnects nothing. The next poll finds the emulator stopped and restarts
    // it, its claim reading the status once more before the Start.
    completed(&resume(&host, SchedulingPauseScope::Global, 842));
    state.emulator.script_read(stopped_state());
    state.emulator.script_read(stopped_state());
    clock.advance(WATCH_INTERVAL_MS);
    host.await_emulator_watch_polls_for_test(3, GATE_WAIT)
        .expect("the first poll after the resume");
    await_reports(&host, |lines| {
        reported(lines, "emulator_autostart_started") == 1
    });
    assert_eq!(control_starts(&state), 1, "one Start after the resume");
    let queued = watch_records(&host, instance, "emulator_autostart_queued");
    let [_, after_resume] = &queued[..] else {
        panic!("the restart before the pause and the one after the resume: {queued:?}");
    };
    assert_eq!(
        after_resume.value("attempts"),
        Some(&json!(1)),
        "the skipped restart gave its place in the bound back"
    );
    assert_eq!(paused_skips(&host, instance).len(), 1);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// (4) IDLE-1's wake: X's `stop_emulator` wake falls due under the global pause. No wake is queued,
// nothing claims X's key and nothing starts its emulator over two checks, one skip
// (`idle_action` `stop_emulator`, `global_paused`) is recorded, and X stays kept closed owing its
// wake. After the global resume the next check wakes X and its configured start starts the
// emulator.
#[test]
fn pg1_4_an_idle_wake_under_the_global_pause_stays_owed_and_is_taken_up_after_the_resume() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(NOW_UNIX_MS, 0));
    let x = instance_id();
    let state = x_state();
    let host = start(&root, &clock, x, &state, false).expect("the start");
    close_x(&host, &state);
    completed(&pause(&host, SchedulingPauseScope::Global, 851));
    let paused_at = head(&host);

    // The wake's time under the global pause (the lease sweep's check may come first).
    clock.set_unix_ms(WAKE_UNIX_MS);
    for _ in 0..2 {
        host.check_idle_instances_for_test(&no_plan())
            .expect("a check under the global pause");
    }
    assert!(
        idle_records(&host, x, paused_at).is_empty(),
        "no wake and no configured start queued"
    );
    assert!(
        claims_after(&host, x, paused_at).is_empty(),
        "nothing claims X's key"
    );
    assert_eq!(emulator_starts(&state), 0, "nothing starts X's emulator");
    let skips = wake_skips(&host, x, paused_at);
    let [skip] = &skips[..] else {
        panic!("one skip for the global pause's revision: {skips:?}");
    };
    assert_eq!(skip.value("alias"), Some(&json!(X_ALIAS)));
    assert_skip(skip, None, Some("stop_emulator"), GLOBAL_PAUSED);
    assert_eq!(
        kept_rows(&host),
        Some(vec![x_row(Some(WAKE_UNIX_MS))]),
        "X stays kept closed, owing its wake"
    );

    // The global resume reconnects nothing. The next check wakes X: its configured start reads
    // the emulator stopped and starts it.
    state.emulator.script_read(stopped_state());
    completed(&resume(&host, SchedulingPauseScope::Global, 852));
    host.check_idle_instances_for_test(&no_plan())
        .expect("the check after the resume");
    assert_eq!(
        idle_records(&host, x, paused_at),
        wake_records(),
        "one wake, after the resume"
    );
    await_reports(&host, start_ended);
    assert_eq!(
        emulator_starts(&state),
        1,
        "the wake after the resume starts X's emulator"
    );
    assert_eq!(wake_skips(&host, x, 0).len(), 1, "no skip after the resume");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// (5) Ruling 3: after the global resume the watch's next poll restarts the stopped emulator as for
// any instance, the first restart of its window. A new global pause is a new revision: its first
// poll records its own skip, its second none.
#[test]
fn pg1_5_after_the_global_resume_the_next_poll_restarts_the_stopped_emulator() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance = instance_id();
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let host = autostart_host(&root, &[(FIRST, instance, &state)], read, |config| {
        config.with_runtime_clock(clock.clone())
    });
    completed(&pause(&host, SchedulingPauseScope::Global, 861));
    let before = head(&host);
    host.start_emulator_watch().expect("the watch starts");
    state.emulator.script_read(stopped_state());
    clock.advance(WATCH_INTERVAL_MS);
    host.await_emulator_watch_polls_for_test(1, GATE_WAIT)
        .expect("a poll under the global pause");
    assert!(claims_after(&host, instance, before).is_empty());
    assert_eq!(paused_skips(&host, instance).len(), 1);

    // The global resume adds nothing: the next poll finds the emulator stopped and restarts it,
    // its claim reading the status once more before the Start.
    completed(&resume(&host, SchedulingPauseScope::Global, 862));
    state.emulator.script_read(stopped_state());
    state.emulator.script_read(stopped_state());
    clock.advance(WATCH_INTERVAL_MS);
    host.await_emulator_watch_polls_for_test(2, GATE_WAIT)
        .expect("the first poll after the resume");
    await_reports(&host, |lines| {
        reported(lines, "emulator_autostart_started") == 1
    });
    assert_eq!(control_starts(&state), 1, "one Start after the resume");
    let queued = watch_records(&host, instance, "emulator_autostart_queued");
    let [queued] = &queued[..] else {
        panic!("one restart after the resume: {queued:?}");
    };
    assert_eq!(queued.value("attempts"), Some(&json!(1)));
    assert_eq!(queued.value("limit_count"), Some(&json!(RESTART_ATTEMPTS)));
    assert_eq!(queued.value("watch_kind"), Some(&json!("restart")));
    assert_eq!(queued.value("skip_reason"), None);

    // A second global pause: its first poll records its own skip, its second none.
    completed(&pause(&host, SchedulingPauseScope::Global, 863));
    for poll in 3..=4 {
        state.emulator.script_read(stopped_state());
        clock.advance(WATCH_INTERVAL_MS);
        host.await_emulator_watch_polls_for_test(poll, GATE_WAIT)
            .expect("a poll of the second global pause");
    }
    let skips = paused_skips(&host, instance);
    let [_, second] = &skips[..] else {
        panic!("one skip per global pause revision: {skips:?}");
    };
    assert_skip(second, Some("restart"), None, GLOBAL_PAUSED);
    assert_eq!(
        watch_records(&host, instance, "emulator_autostart_queued").len(),
        1
    );
    assert_eq!(
        control_starts(&state),
        1,
        "no Start under the second global pause"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// (6) Ruling 2, both pauses on one instance: the watch's skip carries the instance's own word,
// `instance_paused`, once. Once the instance's own pause is resumed while the global one stays, the
// next poll records the global pause's word, once: each pause keeps its own skip memory.
#[test]
fn pg1_6_an_instance_both_pauses_hold_records_its_own_pause_then_the_global_one() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance = instance_id();
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let host = autostart_host(&root, &[(FIRST, instance, &state)], read, |config| {
        config.with_runtime_clock(clock.clone())
    });
    completed(&pause(&host, SchedulingPauseScope::Global, 881));
    completed(&pause(&host, instance_scope(FIRST), 882));
    let before = head(&host);
    host.start_emulator_watch().expect("the watch starts");
    for poll in 1..=2 {
        state.emulator.script_read(stopped_state());
        clock.advance(WATCH_INTERVAL_MS);
        host.await_emulator_watch_polls_for_test(poll, GATE_WAIT)
            .expect("a poll under both pauses");
    }
    let skips = paused_skips(&host, instance);
    let [skip] = &skips[..] else {
        panic!("one skip under both pauses: {skips:?}");
    };
    assert_skip(skip, Some("restart"), None, INSTANCE_PAUSED);
    assert!(
        claims_after(&host, instance, before).is_empty(),
        "no restart claim under both pauses"
    );

    // The resume's reconnect reads the emulator running and passes; the global pause stays.
    state
        .emulator
        .script_read(running_state(FAKE_EMULATOR_PORT));
    completed(&resume(&host, instance_scope(FIRST), 883));
    for poll in 3..=4 {
        state.emulator.script_read(stopped_state());
        clock.advance(WATCH_INTERVAL_MS);
        host.await_emulator_watch_polls_for_test(poll, GATE_WAIT)
            .expect("a poll under the global pause alone");
    }
    let skips = paused_skips(&host, instance);
    let [own, global] = &skips[..] else {
        panic!("the instance pause's skip, then the global pause's: {skips:?}");
    };
    assert_skip(own, Some("restart"), None, INSTANCE_PAUSED);
    assert_skip(global, Some("restart"), None, GLOBAL_PAUSED);
    assert!(
        watch_records(&host, instance, "emulator_autostart_queued").is_empty(),
        "no restart requested under either pause"
    );
    assert_eq!(control_starts(&state), 0, "no Start");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// (7) Ruling 4, the 11-17 case. An autostart claim holds the instance through its Start, whose
// readiness wait ends timed out, and a person pauses the instance meanwhile, the pause's grace
// already past (`with_scheduling_pause_deadlines_for_test`). The pause's hand-back waits for the
// instance, which the Start holds, and then meets the claim's key, which the claim still holds
// once its Start was driven (`run_after_configured_start_drive_for_test`). It waits for that
// automatic claim beyond the grace, ends `released` once the claim has ended and answers success:
// nothing is lifted, and the one record of the held pauses it made names the instance.
#[test]
fn pg1_7_an_instance_pause_stands_while_an_automatic_start_outlasts_its_grace() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance = instance_id();
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let host = autostart_host(&root, &[(FIRST, instance, &state)], read, |config| {
        config.with_scheduling_pause_deadlines_for_test(Duration::ZERO)
    });
    stop(&host, FIRST, 871);
    // The Start blocks in its readiness wait, which then ends timed out.
    state.block_emulator_start.store(true, Ordering::Release);
    state.fail_emulator_start.store(true, Ordering::Release);
    // Once its Start was driven, the claim keeps its key until the hand-back has met it.
    let (driven_tx, driven_rx) = mpsc::channel::<()>();
    let (met_tx, met_rx) = mpsc::channel::<()>();
    host.run_after_configured_start_drive_for_test(Box::new(move || {
        let _ = driven_tx.send(());
        let _ = met_rx.recv_timeout(GATE_WAIT);
    }))
    .expect("the claim's hook");
    host.run_when_pause_release_meets_a_lease_for_test(Box::new(move || {
        let _ = met_tx.send(());
    }))
    .expect("the hand-back's hook");
    assert_eq!(host.enqueue_emulator_autostart().expect("enqueue"), 1);
    wait_until("the configured Start", || {
        state.emulator_start_entered.load(Ordering::Acquire)
    });
    let before = head(&host);
    let pause_request = instance_pause(FIRST);

    let paused = thread::scope(|scope| {
        let pausing = scope.spawn(|| {
            host.process_request_for_test(
                &pause_request,
                ConnectionId::new(872).expect("connection"),
            )
        });
        // The hand-back waits for the instance, which the Start holds.
        wait_until("the pause's hand-back", || {
            pause_stage(&host, FIRST) == Some(InstancePauseStage::Paused)
        });
        state.block_emulator_start.store(false, Ordering::Release);
        driven_rx
            .recv_timeout(GATE_WAIT)
            .expect("the configured Start was driven");
        pausing
            .join()
            .expect("the pause's thread")
            .expect("the pause's receipt")
    });

    assert_eq!(paused.state(), RuntimeReceiptState::Completed, "{paused:?}");
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
    assert_eq!(
        pause_stage(&host, FIRST),
        Some(InstancePauseStage::Released),
        "the pause stands"
    );
    let records = pause_records_after(&host, before);
    let [record] = &records[..] else {
        panic!("the pause's one record of the held pauses, nothing lifted: {records:?}");
    };
    assert!(names_instance(record, FIRST), "{record:?}");
    let lines = await_reports(&host, |lines| {
        reported(lines, "emulator_autostart_failed") == 1
    });
    assert_eq!(reported(&lines, "emulator_autostart_started"), 0);
    assert_eq!(
        host.instance_claims_for_test(FIRST).expect("claims"),
        (None, vec![]),
        "the claim has ended"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// (8) Ruling 4, the grace for everything else: a client's lease, the key a client's run holds its
// instance by, outlasts the grace of an instance pause (already past). The pause fails as before
// with `scheduling_pause_release_busy`, naming no holder, and is lifted: the record that set it and
// the record without it.
#[test]
fn pg1_8_an_instance_pause_still_fails_and_is_lifted_when_a_clients_lease_outlasts_its_grace() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = RuntimeHost::start(
        config(&root).with_scheduling_pause_deadlines_for_test(Duration::ZERO),
        Arc::new(FakeProvider::one(FIRST, instance_id(), Arc::clone(&state))),
    )
    .expect("runtime host");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let acquired = send(
        &host,
        RuntimeOperation::acquire_lease(FIRST, ids.mint_holder_id().expect("holder id")),
        885,
    );
    assert!(
        matches!(acquired.result(), Some(RuntimeResult::LeaseGranted { .. })),
        "{acquired:?}"
    );
    let before = head(&host);
    let pause_request = instance_pause(FIRST);
    let failed = host
        .process_request_for_test(&pause_request, ConnectionId::new(886).expect("connection"))
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
    assert_eq!(
        outcome.value("holder_kind"),
        None,
        "today's word, naming no holder"
    );
    assert_eq!(pause_stage(&host, FIRST), None, "the pause is lifted");
    let records = pause_records_after(&host, before);
    let [set, lifted] = &records[..] else {
        panic!("the pause's record and its lift's: {records:?}");
    };
    assert!(names_instance(set, FIRST), "{set:?}");
    assert!(!names_instance(lifted, FIRST), "{lifted:?}");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// (9) Ruling 4, the bound: an autostart claim still holds the instance once its own deadline (the
// time its key lapses, as its Start renewed it) and the grace after it have passed (the manual
// clock moves on when the hand-back first meets the key; the lease sweep waits, so it does not end
// the lapsed key itself). The pause fails with the existing `scheduling_pause_release_busy`,
// `holder_kind` `autostart` naming the holder, and is lifted.
#[test]
fn pg1_9_an_automatic_start_that_outlasts_its_own_deadline_fails_the_pause_naming_it() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance = instance_id();
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let host = autostart_host(&root, &[(FIRST, instance, &state)], read, |config| {
        config
            .with_runtime_clock(clock.clone())
            .with_scheduling_pause_deadlines_for_test(Duration::ZERO)
    });
    stop(&host, FIRST, 891);
    // The Start's readiness wait ends timed out; then the claim keeps its key until released.
    state.fail_emulator_start.store(true, Ordering::Release);
    let (driven_tx, driven_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    host.run_after_configured_start_drive_for_test(Box::new(move || {
        let _ = driven_tx.send(());
        let _ = release_rx.recv_timeout(GATE_WAIT);
    }))
    .expect("the claim's hook");
    let moved = Arc::clone(&clock);
    host.run_when_pause_release_meets_a_lease_for_test(Box::new(move || {
        moved.advance(OUTLASTED_MS);
    }))
    .expect("the hand-back's hook");
    assert_eq!(host.enqueue_emulator_autostart().expect("enqueue"), 1);
    driven_rx
        .recv_timeout(GATE_WAIT)
        .expect("the configured Start was driven");
    let before = head(&host);
    let pause_request = instance_pause(FIRST);

    let failed = {
        let _sweep = host
            .hold_lease_expiry_scan_for_test()
            .expect("the lease sweep waits");
        let failed = host
            .process_request_for_test(&pause_request, ConnectionId::new(892).expect("connection"))
            .expect("the pause's receipt");
        release_tx.send(()).expect("the claim goes on");
        await_reports(&host, |lines| {
            reported(lines, "emulator_autostart_failed") == 1
        });
        failed
    };

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
    assert_eq!(
        outcome.value("holder_kind"),
        Some(&json!("autostart")),
        "{outcome:?}"
    );
    assert_eq!(pause_stage(&host, FIRST), None, "the pause is lifted");
    let records = pause_records_after(&host, before);
    let [set, lifted] = &records[..] else {
        panic!("the pause's record and its lift's: {records:?}");
    };
    assert!(names_instance(set, FIRST), "{set:?}");
    assert!(!names_instance(lifted, FIRST), "{lifted:?}");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}
