// SPDX-License-Identifier: AGPL-3.0-only

// STILL-1 (Workflow #407; coordinator brief rd-1031/check-speed/brief-slices.md; design
// model-check.md §2, §6, §7; register §1.249): a wait whose frames stay unchanged for
// `execution.capture.still_limit_ms`, none of them what it accepts, ends there through its own
// failure path and code, with `held_ms` and `limit_ms` on the outcome. 0 is off.
//
// - Counted: a page wait's frames of no page, and a confirmation's frames that are no destination
//   and not on the page it holds on.
// - Never counted: a frame of the page the wait holds on (a wait step's own page, a confirmation's
//   wait page). A changed frame restarts the clock.
// - The clock is wall time and does not depend on the back-off's switch or the step's factor.
//
// Every expected value comes from those lines and the package values below, never from the code.
// The runs play scripted frames on the scripted clock (EXEC-I4): the device shows `screens[n]`
// after `n` inputs, each segment's last frame repeating, a frame is delivered at once, and every
// pause moves the clock by its length. A frame's left pixel names its page (`home`, `lobby`,
// `result`; dark and grey are no page) and its right pixel is dark; a dark frame and a grey one
// differ by more than the frame-change threshold. Unless a test says otherwise the runtime hands a
// ceiling of 60 000 ms, back-off on, an idle ceiling of 5 000 ms and a still limit of 30 000 ms, and
// the package declares no interval (50 ms): the pauses over unchanged frames are 50, 100, 200, 400,
// 800, 1 600 and 3 200, then 5 000 each, so unchanged frames come 0, 50, 150, 350, 750, 1 550,
// 3 150, 6 350, 11 350, 16 350, 21 350, 26 350 and 31 350 ms after the stretch's first frame: the
// frame at 31 350 ms is the first at or past the limit.

use super::tests::scripted::{ScriptedEvent, ScriptedFrame, ScriptedRuntime};
use super::*;
use actingcommand_recognition_pack::RecognitionPack;
use serde_json::{Value, json};
use std::path::PathBuf;

const CONFIRMATION_FAILED: &str = ExecutionKernelCode::PageConfirmationFailed.as_str();
const STEP_NOT_CONFIRMED: &str = ExecutionKernelCode::ContainedTaskStepUnconfirmed.as_str();
const PAGE_UNKNOWN: &str = SharedCode::ContainedTaskPageUnknown.as_str();

const STILL_LIMIT_MS: u64 = 30_000;
// A confirmation's own limit, far past the still limit; the task's is longer still. A page wait's
// limit is the step timeout, the longest a control may declare.
const CONFIRMATION_MS: u64 = 600_000;
const TASK_MS: u64 = 1_200_000;
const PAGE_WAIT_MS: u64 = 60_000;
// The pauses over unchanged frames up to the first frame at or past the limit, and their sum.
const STILL_PAUSES: [u64; 12] = [
    50, 100, 200, 400, 800, 1_600, 3_200, 5_000, 5_000, 5_000, 5_000, 5_000,
];
const STILL_HELD_MS: u64 = 31_350;

const HOME: [u8; 3] = [255, 0, 0];
const LOBBY: [u8; 3] = [0, 0, 255];
const RESULT: [u8; 3] = [0, 255, 0];
const DARK: [u8; 3] = [0, 0, 0];
const GREY: [u8; 3] = [128, 128, 128];

type RunResult = Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>;

// The back-off of the configuration's defaults and the limit of `still_limit_ms`.
fn cadence(still_limit_ms: u64) -> CaptureCadence {
    CaptureCadence::new(60_000, true, 5_000).with_still_limit_ms(still_limit_ms)
}

// A frame whose left pixel is `left`.
fn shown(left: [u8; 3]) -> ScriptedFrame {
    ScriptedFrame::rgb(2, 1, &[left, DARK])
}

fn ms(values: &[u64]) -> Vec<Duration> {
    values.iter().copied().map(Duration::from_millis).collect()
}

// Three pages told apart by the left pixel.
fn pack() -> RecognitionPack {
    let target = |id: &str, expected: [u8; 3]| {
        json!({"type": "color", "id": id,
               "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    serde_json::from_value(json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            target("page/home", HOME),
            target("page/lobby", LOBBY),
            target("page/result", RESULT)
        ]
    }))
    .expect("recognition pack")
}

fn page_set() -> PageSet {
    serde_json::from_value(json!({
        "schema_version": "0.3",
        "pages": [
            {"id": "home", "required": ["page/home"]},
            {"id": "lobby", "required": ["page/lobby"]},
            {"id": "result", "required": ["page/result"]}
        ]
    }))
    .expect("page set")
}

// A trusted click from `home`, confirmed on `result` within `CONFIRMATION_MS`, with the wait's
// `expect_after` members and the operation's `extra` members added.
fn click(expect_after: Value, extra: Value) -> Value {
    let mut expectation = json!({"page_id": "result", "timeout_ms": CONFIRMATION_MS});
    for (member, value) in expect_after.as_object().expect("expect_after members") {
        expectation[member] = value.clone();
    }
    let mut operation = json!({"id": "open", "from": "home",
                               "click": {"kind": "point", "x": 1, "y": 0},
                               "unguarded_trusted_coordinate": true,
                               "expect_after": expectation});
    for (member, value) in extra.as_object().expect("operation members") {
        operation[member] = value.clone();
    }
    operation
}

// The page-graph task of `operations` that ends on `result`, within `TASK_MS`, admitted as the
// package's admission admits it.
fn task(operations: Vec<Value>) -> PreparedContainedTask {
    let control: TaskControl = serde_json::from_value(json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.still.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "step_timeout_ms": PAGE_WAIT_MS,
        "timeout_ms": TASK_MS
    }))
    .expect("task control");
    control.validate().expect("an admitted control");
    let program: TaskProgram = serde_json::from_value(json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "target_page": "result",
        "operations": operations
    }))
    .expect("task program");
    for operation in &program.operations {
        operation
            .validate(&control, TaskOperationDefaults::default(), "0.9")
            .expect("an admitted operation");
    }
    let evaluator =
        RecognitionEvaluator::new(PathBuf::new(), pack()).expect("recognition evaluator");
    let detector = PageDetector::new(page_set()).expect("page detector");
    detector
        .validate(&evaluator)
        .expect("page detector targets");
    PreparedContainedTask {
        control,
        program,
        evaluator,
        detector,
        entry_page: None,
        scheduling_outcome: None,
        post_admission_ocr: None,
        post_admission_fields: None,
        entry_count: 1,
        task_count: 1,
    }
}

// Runs `task` on `screens`, the runtime handing `cadence` at every step.
fn run(
    task: &PreparedContainedTask,
    screens: Vec<Vec<ScriptedFrame>>,
    cadence: CaptureCadence,
) -> (RunResult, ScriptedRuntime) {
    let mut runtime = ScriptedRuntime::new(screens).with_capture_cadence(cadence);
    let result = task.run(&mut runtime);
    (result, runtime)
}

fn failed(result: RunResult) -> ContainedTaskError {
    match result {
        Err(ContainedTaskRunError::Task(error)) => error,
        other => panic!("expected a task failure, got {other:?}"),
    }
}

fn succeeded(result: RunResult) {
    assert!(
        matches!(&result, Ok(outcome) if matches!(outcome.outcome, TaskOutcome::Success)),
        "expected the run to succeed: {result:?}"
    );
}

// The pauses of the run after its `n`-th input and before the next one (`0`, before any input).
fn pauses_after_input(runtime: &ScriptedRuntime, n: usize) -> Vec<Duration> {
    let mut inputs = 0;
    let mut pauses = Vec::new();
    for event in runtime.events() {
        match event {
            ScriptedEvent::Input => inputs += 1,
            ScriptedEvent::Paused(pause) if inputs == n => pauses.push(*pause),
            _ => {}
        }
    }
    pauses
}

// The `held_ms` and `limit_ms` on a failure's outcome.
fn still_values(error: &ContainedTaskError) -> (Value, Value) {
    let outcome = error.outcome();
    let value = |key: &str| outcome.value(key).cloned().unwrap_or(Value::Null);
    (value("held_ms"), value("limit_ms"))
}

fn assert_ended_on_the_limit(error: &ContainedTaskError, held_ms: u64, case: &str) {
    assert_eq!(
        still_values(error),
        (json!(held_ms), json!(STILL_LIMIT_MS)),
        "{case}"
    );
}

// With no limit the wait runs its whole time as before: the pauses reach 6 350 ms, then 5 000 each
// up to 56 350 ms, and the next is cut at the 3 650 ms left of a 60 000 ms confirmation. It ends
// with its own code at 60 000 ms of 60 000 ms, and the outcome is the code alone. 0 and a cadence
// that names no limit behave alike.
#[test]
fn still_limit_off_keeps_todays_wait() {
    const WAIT_MS: u64 = 60_000;
    let task = task(vec![click(json!({"timeout_ms": WAIT_MS}), json!({}))]);
    let expected = ms(&[
        50, 100, 200, 400, 800, 1_600, 3_200, 5_000, 5_000, 5_000, 5_000, 5_000, 5_000, 5_000,
        5_000, 5_000, 5_000, 3_650,
    ]);
    for (case, cadence) in [
        ("no limit named", CaptureCadence::new(60_000, true, 5_000)),
        ("a limit of 0", cadence(0)),
    ] {
        let (result, runtime) = run(&task, vec![vec![shown(HOME)], vec![shown(DARK)]], cadence);
        let error = failed(result);
        assert_eq!(error.code().as_str(), CONFIRMATION_FAILED, "{case}");
        let timing = error.timing().expect("the wait's timing");
        assert_eq!(
            (timing.elapsed_ms, timing.limit_ms),
            (WAIT_MS, WAIT_MS),
            "{case}"
        );
        assert!(!error.has_values(), "{case}: {:?}", error.outcome());
        assert_eq!(pauses_after_input(&runtime, 1), expected, "{case}");
    }
}

// A confirmation on a still screen of no page ends at the first frame at or past the limit, with
// its own code on each failure path: an operation without a retry policy
// (`page_confirmation_failed`), and one that decides to fail (`contained_task_step_unconfirmed`,
// which keeps its other values). A screen of a page that is no destination counts as well. The
// page wait of a run that shows no page ends the same way with `contained_task_page_unknown`.
#[test]
fn still_limit_ends_a_confirmation_on_a_still_unknown_screen() {
    for (case, operation, screen, code) in [
        (
            "without a retry policy",
            click(json!({}), json!({})),
            DARK,
            CONFIRMATION_FAILED,
        ),
        (
            "an operation that decides to fail",
            click(json!({}), json!({"retryable": false})),
            DARK,
            STEP_NOT_CONFIRMED,
        ),
        (
            "a page that is no destination",
            click(json!({}), json!({})),
            LOBBY,
            CONFIRMATION_FAILED,
        ),
    ] {
        let task = task(vec![operation]);
        let (result, runtime) = run(
            &task,
            vec![vec![shown(HOME)], vec![shown(screen)]],
            cadence(STILL_LIMIT_MS),
        );
        let error = failed(result);
        assert_eq!(error.code().as_str(), code, "{case}");
        assert_ended_on_the_limit(&error, STILL_HELD_MS, case);
        let timing = error.timing().expect("the wait's timing");
        assert_eq!(
            (timing.elapsed_ms, timing.limit_ms),
            (STILL_HELD_MS, CONFIRMATION_MS),
            "{case}: the timing stays the wait's own"
        );
        assert_eq!(pauses_after_input(&runtime, 1), ms(&STILL_PAUSES), "{case}");
        assert_eq!(runtime.inputs().len(), 1, "{case}: no further input");
    }

    let task_with_retry = task(vec![click(json!({}), json!({"retryable": false}))]);
    let (result, _) = run(
        &task_with_retry,
        vec![vec![shown(HOME)], vec![shown(DARK)]],
        cadence(STILL_LIMIT_MS),
    );
    assert_eq!(
        failed(result).outcome().value("pack_operation_id"),
        Some(&json!("open")),
        "the step's own values stay"
    );

    // The page wait: no frame shows a page of the package.
    let page_wait = task(vec![click(json!({}), json!({}))]);
    let (result, runtime) = run(&page_wait, vec![vec![shown(DARK)]], cadence(STILL_LIMIT_MS));
    let error = failed(result);
    assert_eq!(error.code().as_str(), PAGE_UNKNOWN);
    assert_ended_on_the_limit(&error, STILL_HELD_MS, "the page wait");
    assert_eq!(pauses_after_input(&runtime, 0), ms(&STILL_PAUSES));
    assert!(runtime.inputs().is_empty());
}

// Eleven dark frames reach 21 350 ms; 5 000 ms on, a grey frame at 26 350 ms is changed and restarts
// the clock, and 50 ms on, a dark frame at 26 400 ms, changed again, starts the stretch that reaches
// the limit. The wait ends 31 350 ms after that frame, 57 750 ms after its first.
#[test]
fn moved_frame_restarts_the_still_clock() {
    let task = task(vec![click(json!({}), json!({}))]);
    let mut after_click = vec![shown(DARK); 11];
    after_click.extend([shown(GREY), shown(DARK)]);
    let (result, runtime) = run(
        &task,
        vec![vec![shown(HOME)], after_click],
        cadence(STILL_LIMIT_MS),
    );
    let error = failed(result);
    assert_eq!(error.code().as_str(), CONFIRMATION_FAILED);
    assert_ended_on_the_limit(&error, STILL_HELD_MS, "after the change");
    let timing = error.timing().expect("the wait's timing");
    assert_eq!(timing.elapsed_ms, 57_750);
    let mut expected = vec![
        50, 100, 200, 400, 800, 1_600, 3_200, 5_000, 5_000, 5_000, 5_000, 50,
    ];
    expected.extend(STILL_PAUSES);
    assert_eq!(pauses_after_input(&runtime, 1), ms(&expected));
}

// Frames of the page a wait holds on never count, past the limit: a confirmation's wait page shows
// for fourteen unchanged frames (36 350 ms) and a wait step's own page for thirteen (31 350 ms; the
// run's first frame is the fourteenth), and the destination then ends the wait as it does without
// a limit. After a wait page's frames, frames of no page count from the first of them, not from the
// wait's start.
#[test]
fn held_page_is_never_counted_as_still() {
    let confirmation = task(vec![click(
        json!({"wait_page": {"page_id": "lobby", "max_ms": CONFIRMATION_MS}}),
        json!({}),
    )]);
    let mut after_click = vec![shown(LOBBY); 14];
    after_click.push(shown(RESULT));
    let (result, _) = run(
        &confirmation,
        vec![vec![shown(HOME)], after_click],
        cadence(STILL_LIMIT_MS),
    );
    succeeded(result);

    let wait_step = task(vec![json!({
        "id": "hold", "from": "home", "wait": {"max_ms": CONFIRMATION_MS},
        "expect_after": {"page_id": "result", "timeout_ms": CONFIRMATION_MS}
    })]);
    let mut frames = vec![shown(HOME); 14];
    frames.push(shown(RESULT));
    let (result, runtime) = run(&wait_step, vec![frames], cadence(STILL_LIMIT_MS));
    succeeded(result);
    assert!(runtime.inputs().is_empty(), "a wait step makes no input");

    let mut after_click = vec![shown(LOBBY); 14];
    after_click.push(shown(DARK));
    let (result, _) = run(
        &confirmation,
        vec![vec![shown(HOME)], after_click],
        cadence(STILL_LIMIT_MS),
    );
    let error = failed(result);
    assert_eq!(error.code().as_str(), CONFIRMATION_FAILED);
    assert_ended_on_the_limit(&error, STILL_HELD_MS, "after the wait page");
}

// The limit does not depend on the factor, nor on the back-off. At the factor 1000 (where the
// unpaced rule of a still and unrecognised screen tracks nothing) the wait ends as everywhere. At
// the factor 2000 the first pause is paced (50 ms × 2) until two frames are still, and the rest
// last their length, so the first frame at or past the limit comes at 31 400 ms of wall time. With
// the back-off switched off every pause is 50 ms and the frame at exactly 30 000 ms ends the wait.
#[test]
fn still_limit_applies_at_factor_1000() {
    let task = task(vec![click(json!({}), json!({}))]);
    let screens = || vec![vec![shown(HOME)], vec![shown(DARK)]];
    for (case, factor, cadence, expected, held_ms) in [
        (
            "the factor 1000",
            1_000,
            cadence(STILL_LIMIT_MS),
            ms(&STILL_PAUSES),
            STILL_HELD_MS,
        ),
        (
            "the factor 2000",
            2_000,
            cadence(STILL_LIMIT_MS),
            ms(&[
                100, 100, 200, 400, 800, 1_600, 3_200, 5_000, 5_000, 5_000, 5_000, 5_000,
            ]),
            31_400,
        ),
        (
            "the factor 1000 with the back-off off",
            1_000,
            CaptureCadence::new(60_000, false, 5_000).with_still_limit_ms(STILL_LIMIT_MS),
            ms(&[50; 600]),
            STILL_LIMIT_MS,
        ),
    ] {
        let mut runtime = ScriptedRuntime::new(screens())
            .with_capture_cadence(cadence)
            .with_step_factors(vec![factor]);
        let error = failed(task.run(&mut runtime));
        assert_eq!(error.code().as_str(), CONFIRMATION_FAILED, "{case}");
        assert_ended_on_the_limit(&error, held_ms, case);
        assert_eq!(pauses_after_input(&runtime, 1), expected, "{case}");
    }
}
