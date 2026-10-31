// SPDX-License-Identifier: AGPL-3.0-only

// PAUSE-AUTO (0.12.34, Alice's ruling 117 A): while an instance carries its own scheduling pause,
// neither the configured emulator start at daemon start nor IDLE-1's wake after `stop_emulator`
// starts its emulator, and the pause is asked again on the start's key right up to the Start, so a
// start queued before the pause began starts nothing either; neither do the emulator watch's
// restart and recognition claims requested before the pause, which also give the place they took
// in the watch's bound back. Each skip is `emulator_autostart_skipped` with `skip_reason`
// `instance_paused` (an idle wake's with `idle_action` `stop_emulator`, the watch's with its
// `watch_kind`), once per pause revision and start. After the resume all behave as before; an
// explicit emulator control is unchanged. On the scripted emulator (HOST-I1), a
// scripted connectivity status and, for IDLE-1, the Runtime's manual clock, with the S6c and
// IDLE-1P helpers. Every wait returns as soon as what it waits for is reported or recorded;
// `GATE_WAIT` only bounds a broken run.

use super::emulator_controller::{
    EmulatorCall, FAKE_EMULATOR_PORT, FakeEmulatorController, running_state, stopped_state,
};
use super::gate_369_s6c::{
    GATE_WAIT, START, autostart_host, claims_after, connectivity, control, head, instance_scope,
    of_request, pause, resume, stop,
};
use super::gate_idle1p::{
    NOW_UNIX_MS, WAKE_UNIX_MS, X_ALIAS, emulator_starts, idle_records, kept_rows, no_plan, pause_x,
    start, start_with, stops, wake_records, wake_skips, x_plan_at, x_row, x_state,
};
use super::instance_workers::phases;
use super::*;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{RuntimeConfigManifest, RuntimeLifecyclePhase};
use actingcommand_host_metrics::NetworkConnectivity;
use actingcommand_runtime_config::{
    DefaultsFile, annotate_settable, mechanism_rows, render_parameters,
};
use serde_json::{Value, json};

const FIRST: &str = "pause_auto.first";
const SECOND: &str = "pause_auto.second";
/// R2: the default polling interval of the emulator watch (`emulator_control.watch_interval_ms`).
const WATCH_INTERVAL_MS: u64 = 60_000;

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

// The outcomes of the `device_state_observed` records under `instance` whose code is `code`, in
// order.
fn observed(host: &RuntimeHost, instance: InstanceId, code: &str) -> Vec<Outcome> {
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

// The `emulator_autostart_skipped` records under `instance` that a pause made, in order.
fn paused_skips(host: &RuntimeHost, instance: InstanceId) -> Vec<Outcome> {
    observed(host, instance, "emulator_autostart_skipped")
        .into_iter()
        .filter(|outcome| outcome.value("skip_reason").is_some())
        .collect()
}

fn count(events: &[PersistedEvent], event_type: EventType) -> usize {
    events
        .iter()
        .filter(|event| event.event_type() == event_type)
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
    state
        .emulator
        .calls()
        .iter()
        .filter(|call| **call == EmulatorCall::Control(START))
        .count()
}

fn completed(receipt: &RuntimeReceipt) {
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
}

// The daemon start's skip of `alias`: the pause's reason, and neither the watch's `watch_kind` nor
// the idle wake's `idle_action`.
fn assert_daemon_start_skip(skip: &Outcome, alias: &str) {
    assert_eq!(skip.value("alias"), Some(&serde_json::json!(alias)));
    assert_eq!(
        skip.value("skip_reason"),
        Some(&serde_json::json!("instance_paused"))
    );
    assert_eq!(skip.value("watch_kind"), None);
    assert_eq!(skip.value("idle_action"), None);
}

// The idle wake's skip of X.
fn assert_idle_wake_skip(skip: &Outcome) {
    assert_eq!(skip.value("alias"), Some(&serde_json::json!(X_ALIAS)));
    assert_eq!(
        skip.value("skip_reason"),
        Some(&serde_json::json!("instance_paused"))
    );
    assert_eq!(
        skip.value("idle_action"),
        Some(&serde_json::json!("stop_emulator"))
    );
    assert_eq!(skip.value("watch_kind"), None);
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

// Daemon start: of two configured instances whose emulators are stopped, the one its own pause
// holds gets no claim and no Start, and one skip; the unpaused one is counted, claimed and started
// as before.
#[test]
fn pause_auto_1_the_daemon_start_starts_no_paused_instance_and_an_unpaused_one_as_before() {
    let root = TempDir::new().expect("tempdir");
    let first_state = Arc::new(FakeState::default());
    let second_state = Arc::new(FakeState::default());
    let (first, second) = (instance_id(), instance_id());
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let host = autostart_host(
        &root,
        &[
            (FIRST, first, &first_state),
            (SECOND, second, &second_state),
        ],
        read,
        |config| config,
    );
    stop(&host, FIRST, 701);
    stop(&host, SECOND, 702);
    completed(&pause(&host, instance_scope(FIRST), 703));
    let before = head(&host);

    assert_eq!(
        host.enqueue_emulator_autostart().expect("enqueue"),
        1,
        "the unpaused instance alone"
    );
    let lines = await_reports(&host, |lines| {
        reported(lines, "emulator_autostart_started") == 1
    });
    let queued = lines
        .iter()
        .filter(|line| line.code().as_str() == "emulator_autostart_queued")
        .collect::<Vec<_>>();
    let [queued] = &queued[..] else {
        panic!("one count: {lines:?}");
    };
    assert_eq!(queued.value("count"), Some(&serde_json::json!(1)));
    let started = lines
        .iter()
        .find(|line| line.code().as_str() == "emulator_autostart_started")
        .expect("the start");
    assert_eq!(started.value("alias"), Some(&serde_json::json!(SECOND)));
    assert_eq!(reported(&lines, "emulator_autostart_failed"), 0);

    // The unpaused one, as before.
    assert_eq!(control_starts(&second_state), 1, "the unpaused one starts");
    assert_eq!(claims_after(&host, second, before).len(), 1);
    assert!(paused_skips(&host, second).is_empty());

    // The paused one.
    assert_eq!(control_starts(&first_state), 0, "the paused one does not");
    assert!(
        claims_after(&host, first, before).is_empty(),
        "no claim for the paused one"
    );
    let skips = paused_skips(&host, first);
    let [skip] = &skips[..] else {
        panic!("one skip of the paused one: {skips:?}");
    };
    assert_daemon_start_skip(skip, FIRST);
    assert_eq!(reported(&lines, "emulator_autostart_skipped"), 1);
    assert_eq!(
        host.instance_claims_for_test(FIRST).expect("claims"),
        (None, vec![]),
        "nothing holds or waits for the paused one"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// Idle wake: X's `stop_emulator` wake falls due while its own pause holds X. No wake is queued,
// nothing claims X's key and nothing starts its emulator, one skip is recorded over two checks, and
// X stays kept closed owing its wake. After the resume the next check wakes X and its configured
// start starts the emulator.
#[test]
fn pause_auto_2_a_paused_wake_starts_no_emulator_and_the_wake_after_the_resume_does() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(NOW_UNIX_MS, 0));
    let x = instance_id();
    let state = x_state();
    let host = start(&root, &clock, x, &state, false).expect("the start");
    close_x(&host, &state);
    pause_x(&host, "pause_auto_hold");
    let paused_at = head(&host);

    // The wake's time while the pause holds X (the lease sweep's check may come first).
    clock.set_unix_ms(WAKE_UNIX_MS);
    for _ in 0..2 {
        host.check_idle_instances_for_test(&no_plan())
            .expect("a check under the pause");
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
        panic!("one skip for the pause revision: {skips:?}");
    };
    assert_idle_wake_skip(skip);
    assert_eq!(
        kept_rows(&host),
        Some(vec![x_row(Some(WAKE_UNIX_MS))]),
        "X stays kept closed, owing its wake"
    );

    // The resume (its reconnect reads the emulator stopped), then the next check: the wake, whose
    // configured start reads the emulator stopped and starts it.
    state.emulator.script_read(stopped_state());
    state.emulator.script_read(stopped_state());
    completed(&resume(&host, instance_scope(X_ALIAS), 4_502));
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

// Re-check, daemon start: a configured start claimed before the pause, holding its key while it
// waits for the network, meets the pause on its key: it releases the key without its command or a
// failure pair, starts nothing and records the skip, and the pause's hand-back completes.
#[test]
fn pause_auto_3_a_configured_start_queued_before_the_pause_is_skipped_on_its_key() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance = instance_id();
    let (read, reads) = connectivity(Vec::new(), NetworkConnectivity::NoInternet);
    let host = autostart_host(&root, &[(FIRST, instance, &state)], read, |config| config);
    stop(&host, FIRST, 711);
    let before = head(&host);
    assert_eq!(host.enqueue_emulator_autostart().expect("enqueue"), 1);
    assert_eq!(
        reads
            .recv_timeout(GATE_WAIT)
            .expect("the claim's first read"),
        NetworkConnectivity::NoInternet
    );
    let [claim] = claims_after(&host, instance, before)[..] else {
        panic!("one autostart claim");
    };

    // The pause's hand-back waits for the claim's key, which the claim's next check gives up.
    completed(&pause(&host, instance_scope(FIRST), 712));
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
    assert_daemon_start_skip(skip, FIRST);
    assert_eq!(
        host.instance_claims_for_test(FIRST).expect("claims"),
        (None, vec![])
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// Re-check, idle wake: X's wake is queued before the pause, and its configured start holds X's key
// waiting for the network when the pause arrives. It starts nothing, records the skip, and X owes
// its wake again (kept closed, not woken); checks under the pause queue nothing and record nothing
// more. After the resume the next check wakes X again and its start starts the emulator.
#[test]
fn pause_auto_4_an_idle_wake_queued_before_the_pause_starts_nothing_and_stays_owed() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(NOW_UNIX_MS, 0));
    let x = instance_id();
    let state = x_state();
    // The first configured start waits without internet access; the next one has it.
    let (read, reads) = connectivity(
        vec![NetworkConnectivity::NoInternet],
        NetworkConnectivity::Internet,
    );
    let host = start_with(&root, &clock, x, &state, |config| {
        config.with_network_connectivity_for_test(read)
    })
    .expect("the start");
    close_x(&host, &state);

    // The wake, before any pause (the lease sweep's check may take it): its configured start holds
    // X's key and waits for the network.
    let woken = head(&host);
    clock.set_unix_ms(WAKE_UNIX_MS);
    host.check_idle_instances_for_test(&no_plan())
        .expect("the wake's check");
    assert_eq!(idle_records(&host, x, woken), wake_records(), "X is woken");
    assert_eq!(
        reads
            .recv_timeout(GATE_WAIT)
            .expect("the claim's first read"),
        NetworkConnectivity::NoInternet
    );

    // The pause arrives while that claim waits: the claim's next check gives the key up.
    pause_x(&host, "pause_auto_recheck");
    await_reports(&host, skip_reported);
    assert_eq!(
        emulator_starts(&state),
        0,
        "the queued start starts nothing"
    );
    let skips = wake_skips(&host, x, woken);
    let [skip] = &skips[..] else {
        panic!("one skip on the claim's key: {skips:?}");
    };
    assert_idle_wake_skip(skip);
    assert_eq!(
        kept_rows(&host),
        Some(vec![x_row(Some(WAKE_UNIX_MS))]),
        "X owes its wake again"
    );
    host.check_idle_instances_for_test(&no_plan())
        .expect("a check under the pause");
    assert_eq!(
        idle_records(&host, x, woken),
        wake_records(),
        "no second wake under the pause"
    );
    assert_eq!(wake_skips(&host, x, woken).len(), 1, "and no second skip");

    // The resume (its reconnect reads the emulator stopped), then the next check: X is woken
    // again, and its configured start reads the emulator stopped and starts it.
    state.emulator.script_read(stopped_state());
    state.emulator.script_read(stopped_state());
    completed(&resume(&host, instance_scope(X_ALIAS), 4_503));
    host.check_idle_instances_for_test(&no_plan())
        .expect("the check after the resume");
    let mut woken_twice = wake_records();
    woken_twice.extend(wake_records());
    assert_eq!(
        idle_records(&host, x, woken),
        woken_twice,
        "X is woken again after the resume"
    );
    await_reports(&host, start_ended);
    assert_eq!(
        emulator_starts(&state),
        1,
        "the wake after the resume starts X's emulator"
    );
    assert_eq!(wake_skips(&host, x, 0).len(), 1);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// Explicit: a person's emulator-control start of an instance its own pause holds, whose daemon
// start was skipped, still runs and starts the emulator; the pause stays.
#[test]
fn pause_auto_5_an_emulator_control_start_of_a_paused_instance_still_runs() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance = instance_id();
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let host = autostart_host(&root, &[(FIRST, instance, &state)], read, |config| config);
    stop(&host, FIRST, 721);
    completed(&pause(&host, instance_scope(FIRST), 722));
    assert_eq!(
        host.enqueue_emulator_autostart().expect("enqueue"),
        0,
        "the daemon start leaves the paused instance"
    );
    assert_eq!(control_starts(&state), 0);

    completed(&control(&host, FIRST, START, 723));
    assert_eq!(
        control_starts(&state),
        1,
        "the person's Start runs under the pause"
    );
    let skips = paused_skips(&host, instance);
    let [skip] = &skips[..] else {
        panic!("the daemon start's skip alone: {skips:?}");
    };
    assert_daemon_start_skip(skip, FIRST);
    let (_, paused) = host.scheduling_pauses_for_test().expect("pauses");
    assert!(paused.contains_key(FIRST), "the pause stays");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// Re-check, emulator watch (the coordinator's ruling of 10-31 on rulings 112 and 117 A): a poll
// finds the configured instance's emulator stopped and requests its restart claim (R3) before any
// pause; the claim holds the key, waiting for the network, when the instance's own pause arrives.
// On its key the claim releases the key with no command record and no failure pair, starts
// nothing and records the skip (`watch_kind` `restart`); a poll under the same pause requests
// nothing and records no second skip. After the resume the watch's next poll restarts it as
// before.
#[test]
fn pause_auto_6_a_watch_restart_queued_before_the_pause_is_skipped_on_its_key() {
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

    // The pause's hand-back waits for the claim's key, which the claim's next check gives up.
    completed(&pause(&host, instance_scope(FIRST), 731));
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
    assert_eq!(skip.value("alias"), Some(&serde_json::json!(FIRST)));
    assert_eq!(
        skip.value("watch_kind"),
        Some(&serde_json::json!("restart"))
    );
    assert_eq!(
        skip.value("skip_reason"),
        Some(&serde_json::json!("instance_paused"))
    );
    assert_eq!(skip.value("idle_action"), None);

    // A poll under the same pause finds the emulator stopped again: no claim, no second skip.
    state.emulator.script_read(stopped_state());
    clock.advance(WATCH_INTERVAL_MS);
    host.await_emulator_watch_polls_for_test(2, GATE_WAIT)
        .expect("a poll under the pause");
    assert!(
        claims_after(&host, instance, paused_at).is_empty(),
        "no restart claim under the pause"
    );
    assert_eq!(
        paused_skips(&host, instance).len(),
        1,
        "one skip for the pause revision"
    );
    assert_eq!(control_starts(&state), 0, "no Start under the pause");

    // The resume's reconnect reads the emulator running; the next poll finds it stopped and
    // restarts it, its claim reading the status once more before the Start.
    state
        .emulator
        .script_read(running_state(FAKE_EMULATOR_PORT));
    completed(&resume(&host, instance_scope(FIRST), 732));
    state.emulator.script_read(stopped_state());
    state.emulator.script_read(stopped_state());
    clock.advance(WATCH_INTERVAL_MS);
    host.await_emulator_watch_polls_for_test(3, GATE_WAIT)
        .expect("the first poll after the resume");
    await_reports(&host, |lines| {
        reported(lines, "emulator_autostart_started") == 1
    });
    assert_eq!(control_starts(&state), 1, "one Start after the resume");
    assert_eq!(
        observed(&host, instance, "emulator_autostart_queued").len(),
        2,
        "the restart requested before the pause and the one after the resume"
    );
    assert_eq!(paused_skips(&host, instance).len(), 1);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// The configuration actingd assembles over the defaults file with `instance`'s own blackout list
// holding one `once` entry `[from_unix_ms, until_unix_ms)`.
fn blackout_manifest(
    instance: InstanceId,
    from_unix_ms: u64,
    until_unix_ms: u64,
) -> RuntimeConfigManifest {
    let id = match serde_json::to_value(instance).expect("instance id encodes") {
        Value::String(text) => text,
        other => panic!("an instance id is a string: {other}"),
    };
    let entry = json!({
        "kind": "once",
        "id": "pause_auto.recognize",
        "from_unix_ms": from_unix_ms,
        "until_unix_ms": until_unix_ms,
    });
    let file = json!({
        "instances": [{"instance_id": id, "scheduling": {"blackout": [entry]}}],
    });
    let (_, defaults) = DefaultsFile::shipped().expect("the defaults file parses");
    let mut parameters = render_parameters(
        &mechanism_rows(),
        std::slice::from_ref(&id),
        &defaults,
        &file,
        &|_| None,
    )
    .expect("the sections resolve");
    annotate_settable(&mut parameters).expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
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

// The events of `event_type` under `instance` after `after`.
fn recorded_after(
    host: &RuntimeHost,
    instance: InstanceId,
    event_type: EventType,
    after: u64,
) -> usize {
    host.query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events")
        .iter()
        .filter(|event| {
            event.sequence() > after
                && event.event_type() == event_type
                && event.links().instance_id() == Some(&instance)
        })
        .count()
}

// Re-check, the watch's recognition (the coordinator's ruling of 10-31 on ruling 112: a paused
// instance is not recognized): inside a blackout of the instance a poll finds its emulator
// running with Android and a port while the Runtime holds its endpoint pending, and requests a
// recognition claim, which the blackout holds queued without the key. The instance's own pause
// comes, then the blackout ends: the claim gets the key, meets the pause on it and releases it,
// binding, preparing and starting nothing, and records the skip (`watch_kind` `recognize`); a
// poll under the same pause requests nothing and records no second skip. After the resume the
// next poll recognizes and binds it, the first recognition of the window: the skipped one gave
// its place in the bound back.
#[test]
fn pause_auto_7_a_watch_recognition_queued_before_the_pause_is_skipped_on_its_key() {
    let root = TempDir::new().expect("tempdir");
    let state = pending_state();
    let instance = instance_id();
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let until = POLICY_NOW_UNIX_MS + 100_000;
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock.clone())
            .with_config_manifest(blackout_manifest(
                instance,
                POLICY_NOW_UNIX_MS - 1_000,
                until,
            )),
        Arc::new(FakeProvider::one(FIRST, instance, Arc::clone(&state))),
    )
    .expect("runtime host started before its emulator, inside a blackout");
    let before = head(&host);
    host.start_emulator_watch().expect("the watch starts");

    // The first poll, inside the blackout: the recognition claim waits queued, without the key.
    state
        .emulator
        .script_read(running_state(FAKE_EMULATOR_PORT));
    clock.advance(WATCH_INTERVAL_MS);
    host.await_emulator_watch_polls_for_test(1, GATE_WAIT)
        .expect("the first poll");
    let [claim] = claims_after(&host, instance, before)[..] else {
        panic!("one recognition claim");
    };
    assert_eq!(
        host.instance_claims_for_test(FIRST).expect("claims"),
        (None, vec![actingcommand_scheduler::ClaimKind::Autostart]),
        "the blackout holds the claim without the key"
    );

    // The pause, then the blackout's end: the claim gets the key and meets the pause on it.
    completed(&pause(&host, instance_scope(FIRST), 741));
    let paused_at = head(&host);
    clock.set_unix_ms(until);
    let lines = await_reports(&host, skip_reported);
    assert_eq!(reported(&lines, "emulator_autostart_failed"), 0);
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
    let skips = paused_skips(&host, instance);
    let [skip] = &skips[..] else {
        panic!("one skip on the claim's key: {skips:?}");
    };
    assert_eq!(skip.value("alias"), Some(&serde_json::json!(FIRST)));
    assert_eq!(
        skip.value("watch_kind"),
        Some(&serde_json::json!("recognize"))
    );
    assert_eq!(
        skip.value("skip_reason"),
        Some(&serde_json::json!("instance_paused"))
    );
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

    // A poll under the same pause: no claim and no second skip.
    state
        .emulator
        .script_read(running_state(FAKE_EMULATOR_PORT));
    clock.advance(WATCH_INTERVAL_MS);
    host.await_emulator_watch_polls_for_test(2, GATE_WAIT)
        .expect("a poll under the pause");
    assert!(claims_after(&host, instance, paused_at).is_empty());
    assert_eq!(paused_skips(&host, instance).len(), 1);

    // The resume's reconnect reads the emulator before its Android has started and binds nothing;
    // the next poll reads it running, then the claim's read on its key and the recognition's own.
    state.emulator.script_read(booting_state());
    completed(&resume(&host, instance_scope(FIRST), 742));
    assert_eq!(
        state.emulator.bound_port(),
        None,
        "the reconnect bound nothing"
    );
    let resumed = head(&host);
    for _ in 0..3 {
        state
            .emulator
            .script_read(running_state(FAKE_EMULATOR_PORT));
    }
    clock.advance(WATCH_INTERVAL_MS);
    host.await_emulator_watch_polls_for_test(3, GATE_WAIT)
        .expect("the first poll after the resume");
    await_reports(&host, |lines| {
        lines.iter().any(|line| {
            line.code().as_str() == "emulator_autostart_skipped"
                && line.value("watch_kind") == Some(&serde_json::json!("recognize"))
                && line.value("skip_reason").is_none()
        })
    });
    assert_eq!(
        state.emulator.bound_port(),
        Some(FAKE_EMULATOR_PORT),
        "recognized and bound after the resume"
    );
    assert_eq!(
        recorded_after(&host, instance, EventType::RuntimeInstanceBound, resumed),
        1
    );
    let queued = observed(&host, instance, "emulator_autostart_queued");
    let [_, after_resume] = &queued[..] else {
        panic!("the recognition before the pause and the one after the resume: {queued:?}");
    };
    assert_eq!(
        after_resume.value("attempts"),
        Some(&serde_json::json!(1)),
        "the skipped recognition gave its place in the bound back"
    );
    assert_eq!(paused_skips(&host, instance).len(), 1);
    assert!(
        control_calls(&state).is_empty(),
        "nothing started or stopped"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// The bound (the coordinator's ruling of 10-31): three times a poll requests the restart of the
// configured instance found stopped, and each time the claim meets the instance's own pause on
// its key and starts nothing. Each skipped restart gives its place in the bound back
// (`emulator_control.restart_attempts`, 3 within `restart_window_ms`): after the third resume the
// fourth restart is still allowed, the first of its window, and starts the emulator; no used-up
// bound is recorded.
#[test]
fn pause_auto_8_restarts_skipped_on_their_key_give_their_places_in_the_bound_back() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance = instance_id();
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    // The three skipped restarts' claims wait without internet access; the fourth has it.
    let (read, reads) = connectivity(
        vec![NetworkConnectivity::NoInternet; 3],
        NetworkConnectivity::Internet,
    );
    let host = autostart_host(&root, &[(FIRST, instance, &state)], read, |config| {
        config.with_runtime_clock(clock.clone())
    });
    host.start_emulator_watch().expect("the watch starts");

    for round in 1..=3_u64 {
        // The poll finds the emulator stopped and requests its restart; the claim waits for the
        // network holding its key.
        let before = head(&host);
        state.emulator.script_read(stopped_state());
        clock.advance(WATCH_INTERVAL_MS);
        host.await_emulator_watch_polls_for_test(round, GATE_WAIT)
            .expect("the round's poll");
        assert_eq!(
            reads
                .recv_timeout(GATE_WAIT)
                .expect("the claim's first read"),
            NetworkConnectivity::NoInternet,
            "round {round}"
        );
        assert_eq!(
            claims_after(&host, instance, before).len(),
            1,
            "round {round}: one restart claim"
        );
        // The pause ends the claim on its key; the resume's reconnect reads the emulator running.
        completed(&pause(&host, instance_scope(FIRST), 750 + 2 * round));
        await_reports(&host, |lines| {
            lines
                .iter()
                .filter(|line| {
                    line.code().as_str() == "emulator_autostart_skipped"
                        && line.value("skip_reason").is_some()
                })
                .count()
                == usize::try_from(round).expect("round")
        });
        state
            .emulator
            .script_read(running_state(FAKE_EMULATOR_PORT));
        completed(&resume(&host, instance_scope(FIRST), 751 + 2 * round));
    }
    assert_eq!(control_starts(&state), 0, "no Start in the three rounds");
    assert_eq!(
        paused_skips(&host, instance).len(),
        3,
        "one skip per pause revision"
    );

    // The fourth poll: the bound has room, and the restart starts the emulator.
    state.emulator.script_read(stopped_state());
    state.emulator.script_read(stopped_state());
    clock.advance(WATCH_INTERVAL_MS);
    host.await_emulator_watch_polls_for_test(4, GATE_WAIT)
        .expect("the fourth poll");
    await_reports(&host, |lines| {
        reported(lines, "emulator_autostart_started") == 1
    });
    assert_eq!(control_starts(&state), 1, "one Start");
    let queued = observed(&host, instance, "emulator_autostart_queued");
    assert_eq!(queued.len(), 4, "four restarts requested: {queued:?}");
    for request in &queued {
        assert_eq!(
            request.value("attempts"),
            Some(&serde_json::json!(1)),
            "each restart is the first of its window: {queued:?}"
        );
    }
    assert!(
        observed(&host, instance, "emulator_watch_restart_exhausted").is_empty(),
        "the skipped restarts used up no bound"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}
