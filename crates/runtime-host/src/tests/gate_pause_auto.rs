// SPDX-License-Identifier: AGPL-3.0-only

// PAUSE-AUTO (0.12.34, Alice's ruling 117 A): while an instance carries its own scheduling pause,
// neither the configured emulator start at daemon start nor IDLE-1's wake after `stop_emulator`
// starts its emulator, and the pause is asked again on the start's key right up to the Start, so a
// start queued before the pause began starts nothing either. Each skip is
// `emulator_autostart_skipped` with `skip_reason` `instance_paused` (an idle wake's with
// `idle_action` `stop_emulator`), once per pause revision and start. After the resume both behave
// as before; an explicit emulator control is unchanged. On the scripted emulator (HOST-I1), a
// scripted connectivity status and, for IDLE-1, the Runtime's manual clock, with the S6c and
// IDLE-1P helpers. Every wait returns as soon as what it waits for is reported or recorded;
// `GATE_WAIT` only bounds a broken run.

use super::emulator_controller::{EmulatorCall, stopped_state};
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
use actingcommand_contract::RuntimeLifecyclePhase;
use actingcommand_contract::outcome::Outcome;
use actingcommand_host_metrics::NetworkConnectivity;

const FIRST: &str = "pause_auto.first";
const SECOND: &str = "pause_auto.second";

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

// The `emulator_autostart_skipped` records under `instance` that a pause made, in order.
fn paused_skips(host: &RuntimeHost, instance: InstanceId) -> Vec<Outcome> {
    let events = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events");
    phases(&events)
        .into_iter()
        .filter(|(event, _)| event.links().instance_id() == Some(&instance))
        .filter_map(|(_, phase)| match phase {
            RuntimeLifecyclePhase::DeviceStateObserved { outcome }
                if outcome.code().as_str() == "emulator_autostart_skipped"
                    && outcome.value("skip_reason").is_some() =>
            {
                Some(outcome)
            }
            _ => None,
        })
        .collect()
}

fn count(events: &[PersistedEvent], event_type: EventType) -> usize {
    events
        .iter()
        .filter(|event| event.event_type() == event_type)
        .count()
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
