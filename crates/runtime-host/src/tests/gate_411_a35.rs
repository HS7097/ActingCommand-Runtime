// SPDX-License-Identifier: AGPL-3.0-only

//! #411 A35 (slice A35-S2; design `rd-1021/train-0129/a35-design.md` §2.1, §3, §4.3, §5.1 S2 T1
//! to T6, with decision Q8 of 2026-10-21: the colour-digest distance of #411 B12 is the one
//! frame-change measure): after a return-home rung failure whose ladder class
//! `recovery_ladder.busy.screen.classes` lists, the ladder probes the screen with read-only
//! frames. Two consecutive frames are unchanged when their frame-change measure
//! (`contracts/color-digest.md`, "Frame change") is at most `busy.screen.change_max_milli`. A
//! settled screen is climbed past at once, as before. A changing one is recorded `failed` with
//! `recovery_rung_screen_changing`; the ladder holds `busy.screen.recheck_ms` and probes again,
//! runs the return-home rung again once the screen has settled, and at `busy.screen.max_wait_ms`
//! does what `busy.screen.on_exhausted` says. A probe that cannot capture climbs on as before,
//! its capture failure recorded.
//!
//! The fixture is the ladder fixture's policy run (`ladder_setup`) on a manual Runtime clock
//! that only the test and the kernel's waits move: a hold ends only when the test moves the
//! clock past it. Its packages recognise neither the fake's unknown frame nor its second row, so
//! the scheduled run and the return-home run fail `contained_task_page_unknown` (class
//! `pack_logic_mismatch`, the default listed class). The screen "moves" through a scripted
//! second row of the 16x9 frame (outside every page and guard region) that alternates between
//! black and a colour from capture to capture. This is A35's aspect (a still-moving screen the
//! return-home package does not know, escalated to an application stop), which no earlier test
//! scripted: `gate_387_d1c` and the `instance_workers` fixtures only ever show a still screen.

use super::gate_387_d1c::ladder_manifest;
use super::instance_workers::{
    admit_ladder_run, all_events, ladder_end_code, ladder_finished, ladder_setup, phases,
    wait_for_ladder_end, wait_until,
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
/// Non-default values of the settle gate, each the configuration's (design §3).
const POLL_MS: u64 = 100;
const PROBE_FRAMES: u64 = 3;
const PROBE_INTERVAL_MS: u64 = 500;
const RECHECK_MS: u64 = 40_000;
const MAX_WAIT_MS: u64 = 100_000;
/// The code of the return-home run's failure on the fake's unknown frame.
const PAGE_UNKNOWN: &str = "contained_task_page_unknown";
/// The fake's frame is 16 pixels wide (`physical_task_extent`): its second row starts here. The
/// frame-change grid of a 16x9 frame has one cell per pixel (`contracts/color-digest.md`).
const SECOND_ROW: usize = 16;
const FRAME_CELLS: u64 = 16 * 9;
/// A moving screen: eight cells alternate between black and white (31 quantized levels per
/// channel), a change of `1000 * 8 * 93 / 144` milli between two consecutive frames, well above
/// the default threshold (1500).
const MOVING: Movement = Movement {
    cells: 8,
    colour: [248, 248, 248],
};
const MOVING_MILLI: u64 = 1000 * 8 * 3 * (248 / 8) / FRAME_CELLS;
/// A screen whose consecutive frames change by exactly `BOUNDARY_MILLI`: four cells alternate
/// between black and red 216 (27 quantized levels), `1000 * 4 * 27 / 144` = 750 milli.
const BOUNDARY: Movement = Movement {
    cells: 4,
    colour: [216, 0, 0],
};
const BOUNDARY_MILLI: u64 = 1000 * 4 * (216 / 8) / FRAME_CELLS;

/// Scripted movement of the fake's frame: `cells` pixels of its second row show `colour` on
/// every odd capture and stay black on every even one, while the movement is on.
#[derive(Clone, Copy)]
struct Movement {
    cells: usize,
    colour: [u8; 3],
}

struct SettleLadder {
    root: TempDir,
    host: RuntimeHost,
    state: Arc<FakeState>,
    clock: Arc<ManualRuntimeClock>,
    /// While true, the frames move as scripted.
    moving: Arc<AtomicBool>,
}

/// The settle gate's configuration of these rows: the test values above, every other member
/// (the classes, the threshold, `on_exhausted`) from the defaults file unless a row sets it.
fn screen_config() -> Value {
    json!({
        "probe_frames": PROBE_FRAMES,
        "probe_interval_ms": PROBE_INTERVAL_MS,
        "recheck_ms": RECHECK_MS,
        "max_wait_ms": MAX_WAIT_MS,
    })
}

/// The fixture with `recovery_ladder.busy.screen` set to `screen` and the frames moving by
/// `movement` (still when `None`): the scheduled run has failed and its ladder climbs; its
/// return-home run fails `contained_task_page_unknown`.
fn settle_ladder(screen: Value, movement: Option<Movement>) -> SettleLadder {
    let root = TempDir::new().expect("tempdir");
    let (host_config, request) = ladder_setup(root.path());
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    let moving = Arc::new(AtomicBool::new(movement.is_some()));
    if let Some(Movement { cells, colour }) = movement {
        let moving = Arc::clone(&moving);
        *state.pixel_script.lock().expect("pixel script") = Some(Arc::new(
            move |capture: usize, _inputs: usize| -> Vec<(usize, [u8; 3])> {
                if !moving.load(Ordering::Acquire) || capture.is_multiple_of(2) {
                    return Vec::new();
                }
                (SECOND_ROW..SECOND_ROW + cells)
                    .map(|index| (index, colour))
                    .collect()
            },
        ));
    }
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = RuntimeHost::start(
        host_config
            .with_runtime_clock(clock.clone())
            .with_config_manifest(ladder_manifest(
                json!({ "busy": { "poll_ms": POLL_MS, "screen": screen } }),
            )),
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
    SettleLadder {
        root,
        host,
        state,
        clock,
        moving,
    }
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

fn code_of(record: &RungRecord<'_>) -> Option<&str> {
    record
        .outcome
        .as_ref()
        .map(|outcome| outcome.code().as_str())
}

fn is_changing(record: &RungRecord<'_>) -> bool {
    code_of(record) == Some(HostCode::RecoveryRungScreenChanging.as_str())
}

fn changing_records(events: &[PersistedEvent]) -> usize {
    rung_records(events)
        .iter()
        .filter(|record| is_changing(record))
        .count()
}

fn value(outcome: &Outcome, key: &str) -> Value {
    outcome.value(key).cloned().unwrap_or(Value::Null)
}

fn millis(outcome: &Outcome, key: &str) -> u64 {
    value(outcome, key)
        .as_u64()
        .unwrap_or_else(|| panic!("{key} in {outcome:?}"))
}

fn first_cause(outcome: &Outcome) -> Option<&str> {
    outcome
        .causes()
        .first()
        .map(|link| link.outcome().code().as_str())
}

/// The records of the ladder's settle probes of `event_type`: capture records under the ladder's
/// causation id (its `recovery_ladder_started` record's) that belong to no run, so neither to a
/// rung's package run nor to the scheduled run.
fn probe_records(events: &[PersistedEvent], event_type: EventType) -> Vec<&PersistedEvent> {
    let ladder = phases(events)
        .into_iter()
        .find_map(|(event, phase)| match phase {
            RuntimeLifecyclePhase::RecoveryLadderStarted { .. } => event.links().causation_id(),
            _ => None,
        });
    events
        .iter()
        .filter(|event| {
            ladder.is_some()
                && event.event_type() == event_type
                && event.links().causation_id() == ladder
                && event.links().run_id().is_none()
        })
        .collect()
}

/// The application commands the fake received (the application restart rung's stop is one).
fn application_commands(state: &FakeState) -> usize {
    state.application_count.load(Ordering::Acquire)
}

/// Waits until `ready` holds for the host's records.
fn wait_for(host: &RuntimeHost, what: &str, ready: impl Fn(&[PersistedEvent]) -> bool) {
    wait_until(what, || ready(&all_events(host)));
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

/// T1 (design §2.1 A35-a "changing", §4.3 table row 3, §5.3): the return-home run fails
/// `contained_task_page_unknown` while the probe frames differ. The rung is recorded `failed`
/// with `recovery_rung_screen_changing` (info; caused by the page-unknown code; the frames
/// probed, the largest change between two consecutive ones, the time held since the first
/// "changing" verdict (0), the configured limit and the wait before the next probe), and no
/// application stop is issued while the screen changes. Once the scripted frames are identical
/// (the screen has settled on home), the return-home rung runs again after the hold and
/// recovers: two return-home records at least `screen.recheck_ms` apart on the Runtime clock,
/// `rungs_tried` 1, no application stop.
#[test]
fn a35_t1_a_changing_screen_holds_the_ladder_and_return_home_runs_again_once_it_settles() {
    let ladder = settle_ladder(screen_config(), Some(MOVING));
    wait_for(&ladder.host, "the screen-changing record", |events| {
        !rung_records(events).is_empty()
    });
    let events = all_events(&ladder.host);
    let records = rung_records(&events);
    let [record] = records.as_slice() else {
        panic!("one rung record so far");
    };
    assert_eq!(
        (record.rung, record.rung_result, code_of(record)),
        (
            RecoveryRung::ReturnHome,
            RecoveryRungOutcome::Failed,
            Some(HostCode::RecoveryRungScreenChanging.as_str())
        ),
        "a changing screen holds the ladder"
    );
    let outcome = record
        .outcome
        .as_ref()
        .expect("the changing rung's outcome");
    assert_eq!(outcome.category(), Category::Info);
    assert_eq!(record.event.severity(), EventSeverity::Info);
    assert_eq!(first_cause(outcome), Some(PAGE_UNKNOWN));
    assert_eq!(
        (
            value(outcome, "count"),
            value(outcome, "score_milli"),
            value(outcome, "held_ms"),
            value(outcome, "limit_ms"),
            value(outcome, "retry_after_ms"),
        ),
        (
            json!(PROBE_FRAMES),
            json!(MOVING_MILLI),
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
        "no application stop while the screen changes"
    );

    // The activity ended: the screen stops moving on home, and the return-home package's tap
    // leads to its target page.
    ladder.moving.store(false, Ordering::Release);
    ladder.state.unknown_capture.store(false, Ordering::Release);
    ladder
        .state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    ladder.clock.advance(RECHECK_MS);
    wait_for_ladder_end(&ladder.host, POLICY_INSTANCE_ALIAS);

    let events = all_events(&ladder.host);
    let records = rung_records(&events);
    let [changing, recovered] = records.as_slice() else {
        panic!(
            "two rung records: {:?}",
            records
                .iter()
                .map(|record| (record.rung, record.rung_result, code_of(record)))
                .collect::<Vec<_>>()
        );
    };
    assert!(is_changing(changing));
    assert_eq!(
        (recovered.rung, recovered.rung_result),
        (RecoveryRung::ReturnHome, RecoveryRungOutcome::Recovered),
        "the return-home rung ran again and recovered"
    );
    assert!(recovered.run_id.is_some(), "a run of its own");
    assert!(
        recovered.event.timestamp_unix_ms() - changing.event.timestamp_unix_ms() >= RECHECK_MS,
        "the rerun came after the hold"
    );
    let (_, ladder_result, rungs_tried) = ladder_finished(&events).expect("the ladder's end");
    assert_eq!(
        (ladder_result, rungs_tried),
        (RecoveryLadderOutcome::Recovered, 1)
    );
    assert_eq!(
        probe_records(&events, EventType::CaptureCompleted).len() as u64,
        2 * PROBE_FRAMES,
        "a probe before the hold and one after it"
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

/// T2 (design §2.1 A35-a "settled", §4.4 item 4): the same failure on identical probe frames (a
/// still unknown screen). The probe's frames are taken, then the return-home rung is recorded
/// failed with its run's own code, as before, and the application restart runs right after,
/// with no hold: the test never moves the Runtime clock, so a hold would never end.
#[test]
fn a35_t2_a_still_unknown_screen_is_restarted_right_after_the_probe() {
    let ladder = settle_ladder(screen_config(), None);
    wait_for(
        &ladder.host,
        "the ladder's end, or a screen-changing record",
        |events| ladder_finished(events).is_some() || changing_records(events) > 0,
    );
    let events = all_events(&ladder.host);
    assert_eq!(changing_records(&events), 0, "a still screen is not held");
    let records = rung_records(&events);
    let [return_home, restart, ..] = records.as_slice() else {
        panic!("at least two rung records");
    };
    assert_eq!(
        (
            return_home.rung,
            return_home.rung_result,
            code_of(return_home)
        ),
        (
            RecoveryRung::ReturnHome,
            RecoveryRungOutcome::Failed,
            Some(PAGE_UNKNOWN)
        ),
        "the return-home failure is recorded as before"
    );
    assert_eq!(restart.rung, RecoveryRung::ApplicationRestart);
    assert_ne!(restart.rung_result, RecoveryRungOutcome::Skipped);
    let probe = probe_records(&events, EventType::CaptureCompleted);
    assert_eq!(probe.len() as u64, PROBE_FRAMES, "one probe");
    assert!(
        probe
            .iter()
            .all(|frame| frame.sequence() < return_home.event.sequence()),
        "the probe came between the return-home run and the rung's record"
    );
    assert_eq!(
        application_commands(&ladder.state),
        1,
        "the application restart stops the application"
    );
    wait_for_ladder_end(&ladder.host, POLICY_INSTANCE_ALIAS);
    assert!(ladder.host.fatal_error().expect("runtime health").is_none());
    ladder.host.close().expect("close host");
    drop(ladder.root);
}

/// T3 (design §3, §4.3 "end states"; Q3): the screen keeps changing. Each hold waits
/// `screen.recheck_ms`, no longer than what is left of `screen.max_wait_ms` (both non-default
/// values from the configuration), and each probe takes `(probe_frames - 1)` intervals of the
/// Runtime clock. With 40 000, 100 000 and 1 000 ms the climb records four changing verdicts:
/// held 0, 41 000 and 82 000 ms with their next wait, then 101 000 ms at the limit without one.
/// With `on_exhausted = stop` the ladder ends `exhausted` with `recovery_ladder_hold_exhausted`
/// (warning; `held_ms` at least `limit_ms`; caused by the last `recovery_rung_screen_changing`)
/// and no application stop; with `escalate` the application restart rung runs and stops the
/// application.
#[test]
fn a35_t3_at_the_screen_limit_on_exhausted_stops_or_escalates() {
    for on_exhausted in ["stop", "escalate"] {
        let mut screen = screen_config();
        screen["on_exhausted"] = json!(on_exhausted);
        let ladder = settle_ladder(screen, Some(MOVING));
        let mut verdicts = 0;
        loop {
            verdicts += 1;
            wait_for(&ladder.host, "the next screen-changing record", |events| {
                changing_records(events) >= verdicts
            });
            let events = all_events(&ladder.host);
            let records = rung_records(&events);
            let last = records
                .iter()
                .rfind(|record| is_changing(record))
                .and_then(|record| record.outcome.as_ref())
                .expect("the last changing outcome");
            let Some(wait_ms) = value(last, "retry_after_ms").as_u64() else {
                break;
            };
            assert!(
                verdicts < 4,
                "{on_exhausted}: a fourth hold beyond max_wait_ms: {last:?}"
            );
            ladder.clock.advance(wait_ms);
        }
        assert_eq!(
            verdicts, 4,
            "{on_exhausted}: changing verdicts until the limit"
        );
        wait_for_ladder_end(&ladder.host, POLICY_INSTANCE_ALIAS);

        let events = all_events(&ladder.host);
        let records = rung_records(&events);
        let changing = records
            .iter()
            .filter(|record| is_changing(record))
            .map(|record| {
                let outcome = record.outcome.as_ref().expect("changing outcome");
                (
                    millis(outcome, "held_ms"),
                    millis(outcome, "limit_ms"),
                    value(outcome, "retry_after_ms").as_u64(),
                )
            })
            .collect::<Vec<_>>();
        // Each verdict: the configured limit; the time held since the first one (0 at the
        // first, then at least the holds before it); while below the limit the next wait,
        // `recheck_ms` no longer than what is left, and at the limit none.
        let mut held_before = None::<u64>;
        for (held_ms, limit_ms, retry_after_ms) in &changing {
            assert_eq!(*limit_ms, MAX_WAIT_MS, "{on_exhausted}: {changing:?}");
            match held_before {
                None => assert_eq!(*held_ms, 0, "{on_exhausted}: {changing:?}"),
                Some(at_least) => assert!(*held_ms >= at_least, "{on_exhausted}: {changing:?}"),
            }
            let expected = (*held_ms < MAX_WAIT_MS).then(|| RECHECK_MS.min(MAX_WAIT_MS - *held_ms));
            assert_eq!(*retry_after_ms, expected, "{on_exhausted}: {changing:?}");
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
                first_cause(ended),
                Some(HostCode::RecoveryRungScreenChanging.as_str()),
                "caused by the last changing verdict"
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

/// T4 (design §2.1, §3 `screen.classes`): a return-home failure whose ladder class the
/// configured `screen.classes` does not list is not probed, however the screen moves: no probe
/// frame is taken, the rung is recorded failed with its run's own code and the application
/// restart runs at once, as before. The configuration lists `environment_fault` only, so the
/// run's `contained_task_page_unknown` (`pack_logic_mismatch`) is of an unlisted class.
#[test]
fn a35_t4_a_return_home_failure_of_an_unlisted_class_is_not_probed() {
    let mut screen = screen_config();
    screen["classes"] = json!(["environment_fault"]);
    let ladder = settle_ladder(screen, Some(MOVING));
    wait_for(
        &ladder.host,
        "the ladder's end, or a screen-changing record",
        |events| ladder_finished(events).is_some() || changing_records(events) > 0,
    );
    let events = all_events(&ladder.host);
    assert_eq!(
        changing_records(&events),
        0,
        "an unlisted class is not held"
    );
    assert!(
        probe_records(&events, EventType::CaptureRequested).is_empty(),
        "no probe frame"
    );
    let records = rung_records(&events);
    let [return_home, restart, ..] = records.as_slice() else {
        panic!("at least two rung records");
    };
    assert_eq!(
        (
            return_home.rung,
            return_home.rung_result,
            code_of(return_home)
        ),
        (
            RecoveryRung::ReturnHome,
            RecoveryRungOutcome::Failed,
            Some(PAGE_UNKNOWN)
        )
    );
    assert_eq!(restart.rung, RecoveryRung::ApplicationRestart);
    assert_ne!(restart.rung_result, RecoveryRungOutcome::Skipped);
    assert_eq!(application_commands(&ladder.state), 1);
    wait_for_ladder_end(&ladder.host, POLICY_INSTANCE_ALIAS);
    assert!(ladder.host.fatal_error().expect("runtime health").is_none());
    ladder.host.close().expect("close host");
    drop(ladder.root);
}

/// T5 (design §4.3 table row 3, "probe unavailable"): the screen changes, the ladder holds, and
/// the next probe's first capture fails. The probe is neither a changing screen (no second hold)
/// nor a settled one (no return-home run again): the climb goes on to the next rung, as without
/// the gate, and the probe's `capture.failed` record exists, under the ladder's causation id.
#[test]
fn a35_t5_a_probe_whose_capture_fails_climbs_on_and_keeps_its_failure_record() {
    let ladder = settle_ladder(screen_config(), Some(MOVING));
    wait_for(&ladder.host, "the screen-changing record", |events| {
        changing_records(events) >= 1
    });
    let events = all_events(&ladder.host);
    let held = rung_records(&events)
        .iter()
        .find(|record| is_changing(record))
        .map(|record| record.event.sequence())
        .expect("the changing record");
    // Nothing captures while the ladder holds: the next capture is the next probe's first.
    ladder.state.fail_capture_on.store(
        ladder.state.capture_count.load(Ordering::Acquire) + 1,
        Ordering::Release,
    );
    ladder.clock.advance(RECHECK_MS);
    wait_for(
        &ladder.host,
        "the next rung, another hold, or the return-home rung again",
        |events| {
            let records = rung_records(events);
            records.len() > 1 || ladder_finished(events).is_some()
        },
    );
    let events = all_events(&ladder.host);
    assert_eq!(
        changing_records(&events),
        1,
        "a failed probe is not a changing screen"
    );
    let records = rung_records(&events);
    let next = records
        .iter()
        .find(|record| record.event.sequence() > held)
        .expect("a rung record after the hold");
    assert_eq!(
        next.rung,
        RecoveryRung::ApplicationRestart,
        "the climb goes on to the next rung"
    );
    let failed = probe_records(&events, EventType::CaptureFailed);
    assert!(
        failed
            .iter()
            .any(|record| record.sequence() > held && record.sequence() < next.event.sequence()),
        "the probe's capture failure is recorded: {:?}",
        failed
            .iter()
            .map(|record| record.sequence())
            .collect::<Vec<_>>()
    );
    wait_for_ladder_end(&ladder.host, POLICY_INSTANCE_ALIAS);
    assert!(ladder.host.fatal_error().expect("runtime health").is_none());
    ladder.host.close().expect("close host");
    drop(ladder.root);
}

/// T6 (design §5.1 S2 T6 as changed by decision Q8; `contracts/color-digest.md`, "Frame
/// change": two frames are still when the change is at most the threshold): consecutive probe
/// frames change by exactly `BOUNDARY_MILLI`. With `screen.change_max_milli` at that value the
/// screen is settled (the return-home failure is recorded with its own code and the climb goes
/// on); one milli below it, from the configuration too, the same frames are changing
/// (`recovery_rung_screen_changing` with `score_milli` the change). Neither value is the default.
#[test]
fn a35_t6_the_gate_compares_the_frame_change_with_the_configured_threshold_inclusively() {
    for (change_max_milli, changing) in [(BOUNDARY_MILLI, false), (BOUNDARY_MILLI - 1, true)] {
        let mut screen = screen_config();
        screen["change_max_milli"] = json!(change_max_milli);
        let ladder = settle_ladder(screen, Some(BOUNDARY));
        wait_for(&ladder.host, "the return-home rung's record", |events| {
            !rung_records(events).is_empty()
        });
        let events = all_events(&ladder.host);
        let records = rung_records(&events);
        let record = records.first().expect("the return-home rung's record");
        assert_eq!(record.rung, RecoveryRung::ReturnHome);
        if changing {
            assert_eq!(
                code_of(record),
                Some(HostCode::RecoveryRungScreenChanging.as_str()),
                "{change_max_milli}: one milli above the threshold is a change"
            );
            let outcome = record.outcome.as_ref().expect("the changing outcome");
            assert_eq!(millis(outcome, "score_milli"), BOUNDARY_MILLI);
            close_while_holding(ladder.host);
        } else {
            assert_eq!(
                code_of(record),
                Some(PAGE_UNKNOWN),
                "{change_max_milli}: a change at the threshold is still"
            );
            wait_for_ladder_end(&ladder.host, POLICY_INSTANCE_ALIAS);
            assert!(ladder.host.fatal_error().expect("runtime health").is_none());
            ladder.host.close().expect("close host");
        }
        drop(ladder.root);
    }
}
