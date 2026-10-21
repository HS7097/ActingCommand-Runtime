// SPDX-License-Identifier: AGPL-3.0-only

//! #411 B37 (slice A35-S1; design `rd-1021/train-0129/a35-design.md` §2.2, §4.3, §5.1 S1 T1 to
//! T4; coordinator decisions Q1 to Q3 of 10-21): a return-home rung run that completes on a
//! mapping its package marks `busy` is not recovered. The rung is recorded `failed` with
//! `recovery_rung_busy_outcome`; the ladder holds its key for `busy.outcome.recheck_ms` and runs
//! the return-home rung again, within `busy.outcome.max_wait_ms` of the first busy end, and at
//! that limit `busy.outcome.on_exhausted` ends the ladder without a restart (`stop`,
//! `recovery_ladder_hold_exhausted`) or goes on to the next rung (`escalate`). A hold makes the
//! ladder's holding check at every `busy.poll_ms`.
//!
//! The fixture is the ladder fixture's policy run (`ladder_setup`) on a manual Runtime clock that
//! only the test moves; a hold therefore ends only when the test moves the clock past it. Its
//! return-home package recognises the fake's unknown frame as its activity page, mapped `busy`,
//! and its red frame as home; the scheduled package recognises neither, so the scheduled run
//! fails `contained_task_page_unknown` and the ladder starts at the return-home rung. This is
//! A35's aspect (an instance in a long activity whose return-home run ends on the activity page),
//! which no earlier test scripted: `gate_387_d1c` and the `instance_workers` fixtures only script
//! return-home runs that fail or recover, never one that completes on a busy key.

use super::gate_387_d1c::ladder_manifest;
use super::instance_workers::{
    admit_ladder_run, all_events, closed_ledger_events, ladder_end_code, ladder_finished,
    ladder_setup, phases, wait_for_ladder_end, wait_until,
};
use super::*;
use crate::codes::HostCode;
use actingcommand_contract::outcome::{Category, Outcome};
use actingcommand_contract::{
    RecoveryLadderOutcome, RecoveryRung, RecoveryRungOutcome, RuntimeLifecyclePhase, RuntimePayload,
};
use serde_json::{Value, json};

/// Test plan v2.2 §2.1 guard 3: the one bound of a cross-thread wait in this file.
const GATE_WAIT: Duration = Duration::from_secs(120);
/// The return-home package's two outcome keys: home reached, and the activity still running.
const RETURNED: &str = "returned";
const STILL_RUNNING: &str = "still_running";
/// Non-default values of the busy hold, each the configuration's (design §3).
const POLL_MS: u64 = 100;
const RECHECK_MS: u64 = 40_000;
const MAX_WAIT_MS: u64 = 100_000;

/// The return-home package of these rows (`neutral`, no entry page, page-graph): its targets are
/// `home` (the fake's red frame) and `terminal`, the activity page, recognised by the fake's
/// unknown frame (first pixel `[1, 2, 3]`). Only `terminal`'s mapping is `busy`.
fn busy_return_home_package() -> Vec<u8> {
    let region = json!({"x": 0, "y": 0, "width": 1, "height": 1});
    let task = json!({
        "schema_version": "0.6",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "target_page": ["home", "terminal"],
        "scheduling_outcome": {"mappings": [
            {"outcome_key": RETURNED, "effect": "no_designated_effect",
             "terminal_pages": ["home"]},
            {"outcome_key": STILL_RUNNING, "effect": "no_designated_effect",
             "terminal_pages": ["terminal"], "busy": true}
        ]},
        "operations": [{
            "id": "leave_error",
            "from": "error",
            "to": "home",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true,
            "retryable": false
        }]
    });
    let recognition = json!({
        "schema_version": "0.3",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            {"type": "color", "id": "page/home", "region": region, "expected": [255, 0, 0]},
            {"type": "color", "id": "page/terminal", "region": region, "expected": [1, 2, 3]},
            {"type": "color", "id": "page/error", "region": region, "expected": [255, 255, 0]},
            {"type": "color", "id": "guard/ready",
             "region": {"x": 1, "y": 0, "width": 1, "height": 1}, "expected": [0, 255, 0]}
        ]
    });
    neutral_contained_task_package_with_task_and_recognition(
        true,
        &serde_json::to_vec(&task).expect("busy return-home task JSON"),
        &serde_json::to_vec(&recognition).expect("busy return-home recognition JSON"),
    )
}

struct BusyLadder {
    root: TempDir,
    host: RuntimeHost,
    state: Arc<FakeState>,
    clock: Arc<ManualRuntimeClock>,
}

/// The fixture with `recovery_ladder.busy` set to `busy`: the scheduled run has failed and its
/// ladder climbs; its first return-home run shows the activity page.
fn busy_ladder(busy: &Value) -> BusyLadder {
    let root = TempDir::new().expect("tempdir");
    let (host_config, request) = ladder_setup(root.path());
    // The scheduled run's request binds `return-home.zip` (`ladder_setup`).
    fs::write(
        root.path().join("return-home.zip"),
        busy_return_home_package(),
    )
    .expect("write the busy return-home package");
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = RuntimeHost::start(
        host_config
            .with_runtime_clock(clock.clone())
            .with_config_manifest(ladder_manifest(json!({ "busy": busy }))),
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
    BusyLadder {
        root,
        host,
        state,
        clock,
    }
}

fn busy_config(recheck_ms: u64, max_wait_ms: u64, on_exhausted: &str) -> Value {
    json!({
        "poll_ms": POLL_MS,
        "outcome": {
            "recheck_ms": recheck_ms,
            "max_wait_ms": max_wait_ms,
            "on_exhausted": on_exhausted
        }
    })
}

/// One `recovery_rung_finished` record: its record, rung, result, run and outcome.
struct RungRecord<'a> {
    event: &'a PersistedEvent,
    rung: RecoveryRung,
    rung_result: RecoveryRungOutcome,
    run_id: Option<RunId>,
    outcome: Option<Outcome>,
}

fn rung_records(events: &[PersistedEvent]) -> Vec<RungRecord<'_>> {
    phases(events)
        .into_iter()
        .filter_map(|(event, phase)| match phase {
            RuntimeLifecyclePhase::RecoveryRungFinished {
                rung,
                rung_result,
                run_id,
                outcome,
                ..
            } => Some(RungRecord {
                event,
                rung,
                rung_result,
                run_id,
                outcome,
            }),
            _ => None,
        })
        .collect()
}

fn code_of(outcome: Option<&Outcome>) -> Option<&str> {
    outcome.map(|outcome| outcome.code().as_str())
}

fn is_busy(record: &RungRecord<'_>) -> bool {
    code_of(record.outcome.as_ref()) == Some(HostCode::RecoveryRungBusyOutcome.as_str())
}

fn value(outcome: &Outcome, key: &str) -> Value {
    outcome.value(key).cloned().unwrap_or(Value::Null)
}

fn millis(outcome: &Outcome, key: &str) -> u64 {
    value(outcome, key)
        .as_u64()
        .unwrap_or_else(|| panic!("{key} in {outcome:?}"))
}

/// Waits for the `count`-th busy rung record.
fn wait_for_busy_records(host: &RuntimeHost, count: usize) {
    wait_until("the busy rung's record", || {
        rung_records(&all_events(host))
            .iter()
            .filter(|record| is_busy(record))
            .count()
            >= count
    });
}

/// Waits for the first rung record of the return-home rung, whatever its result.
fn wait_for_return_home_record(host: &RuntimeHost) {
    wait_until("the return-home rung's record", || {
        rung_records(&all_events(host))
            .iter()
            .any(|record| record.rung == RecoveryRung::ReturnHome)
    });
}

/// The application commands the fake received (the application restart rung's stop is one).
fn application_commands(state: &FakeState) -> usize {
    state.application_count.load(Ordering::Acquire)
}

/// Closes `host` while its ladder holds, on a thread of its own, waiting at most `GATE_WAIT`: the
/// hold makes its holding check at every poll, so the shutdown ends it at the next one and the
/// close returns.
fn close_while_holding(host: RuntimeHost) {
    let (closed_tx, closed_rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = closed_tx.send(host.close());
    });
    closed_rx
        .recv_timeout(GATE_WAIT)
        .expect("the close returns at the hold's next poll")
        .expect("close host");
}

/// T1 (design §2.2, §4.3 table row 2): the return-home run completes `success` on the `busy`
/// mapping. The rung is recorded `failed` with `recovery_rung_busy_outcome` (info), carrying the
/// mapping's key, the time held since the first busy end (0) and the configured limit, and the
/// wait before the return-home rung runs again; it is not recovered, and while the budget lasts
/// no application stop is issued and no further rung runs.
#[test]
fn b37_t1_a_busy_return_home_end_is_recorded_failed_and_not_recovered() {
    let ladder = busy_ladder(&busy_config(RECHECK_MS, MAX_WAIT_MS, "stop"));
    wait_for_return_home_record(&ladder.host);
    let events = all_events(&ladder.host);
    let records = rung_records(&events);
    let [record] = records.as_slice() else {
        panic!("one rung record so far");
    };
    assert_eq!(
        (record.rung, record.rung_result),
        (RecoveryRung::ReturnHome, RecoveryRungOutcome::Failed),
        "a busy end is not recovered"
    );
    assert!(record.run_id.is_some(), "the record names the busy run");
    let outcome = record.outcome.as_ref().expect("the busy rung's outcome");
    assert_eq!(
        outcome.code().as_str(),
        HostCode::RecoveryRungBusyOutcome.as_str()
    );
    assert_eq!(outcome.category(), Category::Info);
    assert_eq!(record.event.severity(), EventSeverity::Info);
    assert_eq!(
        (
            value(outcome, "outcome_key"),
            value(outcome, "held_ms"),
            value(outcome, "limit_ms"),
            value(outcome, "retry_after_ms"),
        ),
        (
            json!(STILL_RUNNING),
            json!(0),
            json!(MAX_WAIT_MS),
            json!(RECHECK_MS)
        )
    );
    assert!(
        ladder_finished(&events).is_none(),
        "the ladder holds the instance"
    );
    assert_eq!(
        application_commands(&ladder.state),
        0,
        "no application stop while the budget lasts"
    );
    assert!(ladder.host.fatal_error().expect("runtime health").is_none());
    close_while_holding(ladder.host);
    drop(ladder.root);
}

/// T2 (design §4.3 "re-runs", "end states: recovered"; Q1, Q2): busy once, then, after the
/// test moved the Runtime clock by `recheck_ms`, the return-home rung runs again and completes on
/// the mapping that is not busy. The ladder ends `recovered` with `rungs_tried` 1 (distinct
/// rungs), two return-home records at least `recheck_ms` apart on the Runtime clock, and no
/// application stop.
#[test]
fn b37_t2_after_the_hold_the_return_home_rung_runs_again_and_recovers() {
    let ladder = busy_ladder(&busy_config(RECHECK_MS, MAX_WAIT_MS, "stop"));
    wait_for_busy_records(&ladder.host, 1);
    // The activity ended: the fake shows home.
    ladder.state.unknown_capture.store(false, Ordering::Release);
    ladder.clock.advance(RECHECK_MS);
    wait_for_ladder_end(&ladder.host, POLICY_INSTANCE_ALIAS);

    let events = all_events(&ladder.host);
    let records = rung_records(&events);
    let [busy, recovered] = records.as_slice() else {
        panic!(
            "two rung records: {:?}",
            records
                .iter()
                .map(|record| (record.rung, record.rung_result))
                .collect::<Vec<_>>()
        );
    };
    assert!(is_busy(busy), "the first return-home run ended busy");
    assert_eq!(busy.rung, RecoveryRung::ReturnHome);
    assert_eq!(
        (recovered.rung, recovered.rung_result),
        (RecoveryRung::ReturnHome, RecoveryRungOutcome::Recovered),
        "the return-home rung ran again and recovered"
    );
    assert_ne!(
        busy.run_id, recovered.run_id,
        "the second record is a run of its own"
    );
    assert!(
        recovered.event.timestamp_unix_ms() - busy.event.timestamp_unix_ms() >= RECHECK_MS,
        "the rerun came after the hold"
    );
    let (_, ladder_result, rungs_tried) = ladder_finished(&events).expect("the ladder's end");
    assert_eq!(
        (ladder_result, rungs_tried),
        (RecoveryLadderOutcome::Recovered, 1)
    );
    assert_eq!(
        application_commands(&ladder.state),
        0,
        "no application stop"
    );
    assert!(ladder.host.fatal_error().expect("runtime health").is_none());
    ladder.host.close().expect("close host");
    drop(ladder.root);
}

/// T3 (design §3, §4.3 "end states"; Q3): the return-home run stays busy. Each hold waits
/// `recheck_ms`, no longer than what is left of `max_wait_ms` (both non-default values from the
/// configuration), so with 40 000 and 100 000 ms the climb records three busy ends with their
/// next wait (40 000, 40 000, 20 000 ms) and a fourth at the limit without one. With
/// `on_exhausted = stop` the ladder ends `exhausted` with `recovery_ladder_hold_exhausted`
/// (warning; `held_ms` at least `limit_ms`; caused by the last busy outcome) and no application
/// stop; with `escalate` the application restart rung runs and stops the application.
#[test]
fn b37_t3_at_the_limit_on_exhausted_stops_or_escalates() {
    for on_exhausted in ["stop", "escalate"] {
        let ladder = busy_ladder(&busy_config(RECHECK_MS, MAX_WAIT_MS, on_exhausted));
        let mut busy_ends = 0;
        loop {
            busy_ends += 1;
            wait_for_busy_records(&ladder.host, busy_ends);
            let events = all_events(&ladder.host);
            let records = rung_records(&events);
            let last = records
                .iter()
                .rfind(|record| is_busy(record))
                .and_then(|record| record.outcome.as_ref())
                .expect("the last busy outcome");
            let Some(wait_ms) = value(last, "retry_after_ms").as_u64() else {
                break;
            };
            assert!(
                busy_ends < 4,
                "{on_exhausted}: a fourth hold beyond max_wait_ms: {last:?}"
            );
            ladder.clock.advance(wait_ms);
        }
        assert_eq!(busy_ends, 4, "{on_exhausted}: busy ends until the limit");
        wait_for_ladder_end(&ladder.host, POLICY_INSTANCE_ALIAS);

        let events = all_events(&ladder.host);
        let records = rung_records(&events);
        let busy = records
            .iter()
            .filter(|record| is_busy(record))
            .map(|record| {
                let outcome = record.outcome.as_ref().expect("busy outcome");
                (
                    millis(outcome, "held_ms"),
                    millis(outcome, "limit_ms"),
                    value(outcome, "retry_after_ms").as_u64(),
                )
            })
            .collect::<Vec<_>>();
        // Each busy end: the configured limit; the time held since the first one (0 at the
        // first, then at least the holds before it); while below the limit the next wait,
        // `recheck_ms` no longer than what is left, and at the limit none.
        let mut held_before = None::<u64>;
        for (held_ms, limit_ms, retry_after_ms) in &busy {
            assert_eq!(*limit_ms, MAX_WAIT_MS, "{on_exhausted}: {busy:?}");
            match held_before {
                None => assert_eq!(*held_ms, 0, "{on_exhausted}: {busy:?}"),
                Some(at_least) => assert!(*held_ms >= at_least, "{on_exhausted}: {busy:?}"),
            }
            let expected = (*held_ms < MAX_WAIT_MS).then(|| RECHECK_MS.min(MAX_WAIT_MS - *held_ms));
            assert_eq!(*retry_after_ms, expected, "{on_exhausted}: {busy:?}");
            held_before = Some(held_ms + retry_after_ms.unwrap_or(0));
        }
        let restarted = records
            .iter()
            .any(|record| record.rung == RecoveryRung::ApplicationRestart);
        let (finished, ladder_result, _) = ladder_finished(&events).expect("the ladder's end");
        assert_eq!(
            ladder_result,
            RecoveryLadderOutcome::Exhausted,
            "{on_exhausted}"
        );
        if on_exhausted == "stop" {
            assert!(!restarted, "stop: no application restart rung");
            assert_eq!(application_commands(&ladder.state), 0, "stop: no stop");
            assert_eq!(
                ladder_end_code(finished),
                Some(HostCode::RecoveryLadderHoldExhausted.as_str())
            );
            assert_eq!(finished.severity(), EventSeverity::Warning);
            let EventPayload::Runtime(RuntimePayload::LifecycleObserved(observed)) =
                finished.payload()
            else {
                panic!("the ladder's end record");
            };
            let ended = observed.outcome().expect("the exhausted outcome");
            assert_eq!(millis(ended, "limit_ms"), MAX_WAIT_MS);
            assert!(
                millis(ended, "held_ms") >= MAX_WAIT_MS,
                "held to the limit: {ended:?}"
            );
            assert_eq!(
                ended
                    .causes()
                    .first()
                    .map(|link| link.outcome().code().as_str()),
                Some(HostCode::RecoveryRungBusyOutcome.as_str()),
                "caused by the last busy outcome"
            );
        } else {
            assert!(
                records.iter().any(|record| {
                    record.rung == RecoveryRung::ApplicationRestart
                        && record.rung_result != RecoveryRungOutcome::Skipped
                }),
                "escalate: the application restart rung runs"
            );
            assert_eq!(
                application_commands(&ladder.state),
                1,
                "escalate: the rung stops the application"
            );
        }
        assert!(ladder.host.fatal_error().expect("runtime health").is_none());
        ladder.host.close().expect("close host");
        drop(ladder.root);
    }
}

/// T4 (design §4.3 "the hold"): a shutdown requested while the ladder holds (the Runtime clock
/// never reaches the hold's end) ends the climb at the hold's next poll: the return-home rung it
/// was about to run again is recorded failed with `recovery_ladder_interrupted` caused by
/// `recovery_ladder_shutdown_requested`, the ladder ends `exhausted` at warning with the same
/// outcome, no second return-home run starts, and the close returns.
#[test]
fn b37_t4_a_shutdown_during_the_hold_ends_the_climb_at_the_next_poll() {
    // A recheck long enough that a hold without holding checks never ends within GATE_WAIT.
    let ladder = busy_ladder(&busy_config(1_200_000, 3_600_000, "stop"));
    wait_for_busy_records(&ladder.host, 1);
    let BusyLadder { root, host, .. } = ladder;
    close_while_holding(host);

    let events = closed_ledger_events(root.path());
    let records = rung_records(&events);
    let [busy, interrupted] = records.as_slice() else {
        panic!(
            "two rung records: {:?}",
            records
                .iter()
                .map(|record| (record.rung, record.rung_result))
                .collect::<Vec<_>>()
        );
    };
    assert!(is_busy(busy));
    assert_eq!(
        (
            interrupted.rung,
            interrupted.rung_result,
            interrupted.run_id
        ),
        (RecoveryRung::ReturnHome, RecoveryRungOutcome::Failed, None),
        "no second return-home run started"
    );
    let shutdown = |outcome: Option<&Outcome>| {
        outcome.map(|outcome| {
            (
                outcome.code().as_str().to_owned(),
                outcome
                    .causes()
                    .first()
                    .map(|link| link.outcome().code().as_str().to_owned()),
            )
        })
    };
    let expected = Some((
        HostCode::RecoveryLadderInterrupted.as_str().to_owned(),
        Some(
            HostCode::RecoveryLadderShutdownRequested
                .as_str()
                .to_owned(),
        ),
    ));
    assert_eq!(shutdown(interrupted.outcome.as_ref()), expected);
    let (finished, ladder_result, rungs_tried) =
        ladder_finished(&events).expect("the ladder's end");
    assert_eq!(
        (ladder_result, rungs_tried),
        (RecoveryLadderOutcome::Exhausted, 1)
    );
    assert_eq!(finished.severity(), EventSeverity::Warning);
    let EventPayload::Runtime(RuntimePayload::LifecycleObserved(observed)) = finished.payload()
    else {
        panic!("the ladder's end record");
    };
    assert_eq!(shutdown(observed.outcome()), expected);
    assert!(
        finished.timestamp_unix_ms() - busy.event.timestamp_unix_ms() < 1_200_000,
        "the climb ended before the hold's end"
    );
}
