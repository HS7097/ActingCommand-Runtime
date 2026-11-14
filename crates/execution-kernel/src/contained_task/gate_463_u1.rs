// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 U1 (register §1.365; contracts/linear-steps.md, "Failure codes"; coordinator work
// order `rd-1114/brief-463-u1-unrecorded-screen.md`): a linear step's retry decision that saw
// neither its gate nor the step's own page, on a screen whose last two frames were unchanged by the
// Runtime's one frame-change measure, and that did not end on its loading allowance, met a screen
// the package never recorded. A task's own run ends there with
// `contained_task_linear_unrecorded_screen`, the detail `page_confirmation_failed` gives that end,
// and no further input. Every other end keeps the code and the detail it had:
// - the step's own page still shown with no attempt left (the input did nothing);
// - a retry decision that ended on the attempt's loading allowance (LOAD-4);
// - a screen still changing at the decision's end.
//
// Fix round 1 (the coordinator's ruling on board #463; contracts/linear-steps.md, "Retries"): a
// step that declares no retry, whose failed wait ends the run with `page_confirmation_failed`,
// first makes one closing decision, the retry decision's own wait on the pages the failed wait
// awaited, then the step's own page. It classifies and nothing else: no input, no further
// attempt, and a page that passes on it leaves the run failed as before. Only neither page on a
// settled screen gives the new code, with the detail, timing and values of that end; the step's
// own page, an awaited page that shows only then, a screen still changing keep the code and the
// detail of the failed wait, and so does the task's deadline inside it (closing round: the run
// had failed when the wait ended). It makes no input and at most the captures of one retry
// decision.
//
// Every expected value comes from those lines and the package values below, never from the code.
// The runs play scripted frames on the scripted clock (EXEC-I4) at the factor 1000: the device
// shows `screens[n]` after `n` inputs, each segment's last frame repeating, a frame is delivered at
// once, and every pause moves the clock by its length. A frame is two pixels: the first names the
// page (`home`, `result`; dark and grey are no page), a white second is the loading page
// `spinner`. The package declares no capture interval, so a wait takes a frame every 50 ms without
// back-off. The step `open` taps `home` and expects `result` within 1 000 ms; it may be attempted
// twice, 100 ms apart, and its retry decision waits the step timeout of 1 000 ms.

use super::tests::scripted::{ScriptedFrame, ScriptedRuntime};
use super::*;
use actingcommand_page_detector::LoadingPageSet;
use actingcommand_recognition_pack::RecognitionPack;
use serde_json::{Value, json};
use std::path::PathBuf;

const UNRECORDED_SCREEN: &str = ExecutionKernelCode::ContainedTaskLinearUnrecordedScreen.as_str();
const CONFIRMATION_FAILED: &str = ExecutionKernelCode::PageConfirmationFailed.as_str();

const CONFIRMATION_MS: u64 = 1_000;
const STEP_MS: u64 = 1_000;
const TASK_MS: u64 = 600_000;
// Fix round 1: a task timeout whose deadline falls 500 ms into the closing decision, which starts
// when the arrival's 1 000 ms run out.
const DEADLINE_TASK_MS: u64 = 1_500;
const LOADING_MS: u64 = 200;
const TASK_FILE: &str = "resources/operations/task/task.json";
// The detail `page_confirmation_failed` gives a retry decision that saw neither page after the
// first attempt of `open` (contracts/linear-steps.md, "Failure codes" and "Failure detail").
const DECISION_DETAIL: &str =
    "operation=open attempts=1 after_page=<unrecognized> hit_error_page=false transition=none";

const HOME: [u8; 3] = [255, 0, 0];
const RESULT: [u8; 3] = [0, 255, 0];
const DARK: [u8; 3] = [0, 0, 0];
const GREY: [u8; 3] = [128, 128, 128];
const WHITE: [u8; 3] = [255, 255, 255];

type RunResult = Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>;

fn plain(page: [u8; 3]) -> ScriptedFrame {
    ScriptedFrame::rgb(2, 1, &[page, DARK])
}

fn spinning(page: [u8; 3]) -> ScriptedFrame {
    ScriptedFrame::rgb(2, 1, &[page, WHITE])
}

fn pack() -> RecognitionPack {
    let colour = |id: &str, x: u32, expected: [u8; 3]| {
        json!({"type": "color", "id": id,
               "region": {"x": x, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    serde_json::from_value(json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            colour("page/home", 0, HOME),
            colour("page/result", 0, RESULT),
            colour("loading/spinner", 1, WHITE)
        ]
    }))
    .expect("recognition pack")
}

fn page_set() -> PageSet {
    serde_json::from_value(json!({"schema_version": "0.3", "pages": [
        {"id": "home", "required": ["page/home"]},
        {"id": "result", "required": ["page/result"]}
    ]}))
    .expect("page set")
}

fn loading_pages() -> LoadingPageSet {
    serde_json::from_value(json!({"max_ms": LOADING_MS, "pages": [
        {"id": "spinner", "required": ["loading/spinner"]}
    ]}))
    .expect("loading pages")
}

// The step plan `home` -> `result` of the one tap `open`, attempted twice 100 ms apart, its
// `expect_after` with `expect_after` members, admitted as the package's admission admits it.
fn plan(expect_after: Value) -> PreparedContainedTask {
    plan_of(expect_after, Retry::Declared, TASK_MS)
}

// Fix round 1: whether `open` declares its retry (twice, 100 ms apart) or none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Retry {
    Declared,
    Undeclared,
}

// The same step plan with `retry` and the task timeout `task_ms`.
fn plan_of(expect_after: Value, retry: Retry, task_ms: u64) -> PreparedContainedTask {
    let mut expectation = json!({"page_id": "result", "timeout_ms": CONFIRMATION_MS});
    for (member, value) in expect_after.as_object().expect("expect_after members") {
        expectation[member] = value.clone();
    }
    let control: TaskControl = serde_json::from_value(json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.unrecorded.plan",
        "execution_mode": "linear_steps",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "step_timeout_ms": STEP_MS,
        "timeout_ms": task_ms,
        "max_steps": 1
    }))
    .expect("task control");
    control.validate().expect("an admitted control");
    let mut operation = json!({
        "id": "open",
        "from": "home",
        "click": {"kind": "point", "x": 1, "y": 0},
        "unguarded_trusted_coordinate": true,
        "expect_after": expectation
    });
    if retry == Retry::Declared {
        operation["retryable"] = json!(true);
        operation["max_attempts"] = json!(2);
        operation["retry_interval_ms"] = json!(100);
    }
    let program: TaskProgram = serde_json::from_value(json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "entry_page": "home",
        "target_page": "result",
        "max_steps": 1,
        "operations": [operation]
    }))
    .expect("task program");
    let evaluator =
        RecognitionEvaluator::new(PathBuf::new(), pack()).expect("recognition evaluator");
    let detector = PageDetector::new(page_set())
        .expect("page detector")
        .with_loading_pages(loading_pages())
        .expect("loading pages");
    detector
        .validate(&evaluator)
        .expect("page detector targets");
    program
        .validate_loading(&control, &detector, TASK_FILE)
        .expect("admitted loading declarations");
    program
        .validate_linear(&control, &detector)
        .map(drop)
        .expect("an admitted step plan");
    let scheduling_outcome = program.scheduling_outcome.clone();
    PreparedContainedTask {
        control,
        program,
        evaluator,
        detector,
        entry_page: None,
        scheduling_outcome,
        post_admission_ocr: None,
        post_admission_fields: None,
        entry_count: 1,
        task_count: 1,
    }
}

fn run(
    task: &PreparedContainedTask,
    screens: Vec<Vec<ScriptedFrame>>,
    mode: LoadingMode,
) -> (RunResult, ScriptedRuntime) {
    let mut runtime = ScriptedRuntime::new(screens).with_loading_mode(mode);
    let result = task.run(&mut runtime);
    (result, runtime)
}

fn failed(case: &str, result: RunResult) -> ContainedTaskError {
    match result {
        Err(ContainedTaskRunError::Task(error)) => error,
        other => panic!("{case}: expected a task failure, got {other:?}"),
    }
}

fn value(error: &ContainedTaskError, key: &str) -> Value {
    error.outcome().value(key).cloned().unwrap_or(Value::Null)
}

fn timing(error: &ContainedTaskError) -> Option<(TaskTimingScope, TaskTimingStage, u64)> {
    error
        .timing()
        .map(|failure| (failure.scope, failure.stage, failure.limit_ms))
}

// Fix round 1: the elapsed time and the limit of a failure's timing, in milliseconds.
fn spent(error: &ContainedTaskError) -> Option<(u64, u64)> {
    error
        .timing()
        .map(|failure| (failure.elapsed_ms, failure.limit_ms))
}

// Fix round 1: a screen that changes on every frame, dark and grey, neither a page of the
// package, for more frames than the arrival (21) and a decision (21) take; the last one repeats.
fn changing_screen() -> Vec<ScriptedFrame> {
    (0..80)
        .map(|index| plain(if index % 2 == 0 { DARK } else { GREY }))
        .collect()
}

// Fix round 1: the run of `open` without a retry on `after_tap`, its task timeout `task_ms`.
fn run_without_retry(after_tap: Vec<ScriptedFrame>, task_ms: u64) -> (RunResult, ScriptedRuntime) {
    run(
        &plan_of(json!({}), Retry::Undeclared, task_ms),
        vec![vec![plain(HOME)], after_tap],
        LoadingMode::Off,
    )
}

// Fix round 1: what the failure of a step without a retry carries when it ends as the failed
// arrival ended it before the closing decision: `page_confirmation_failed` with the arrival's
// detail and timing (its 1 000 ms of 1 000) and no values, after one input, its one
// `StepFinished` naming `<unrecognized>`.
fn assert_ends_as_the_arrival(case: &str, result: RunResult, runtime: &ScriptedRuntime) {
    let error = failed(case, result);
    assert_eq!(
        error.code().as_str(),
        CONFIRMATION_FAILED,
        "{case}: {error}"
    );
    assert_eq!(error.detail(), Some(DECISION_DETAIL), "{case}: {error}");
    assert_eq!(
        timing(&error),
        Some((
            TaskTimingScope::Postcondition,
            TaskTimingStage::Postcondition,
            CONFIRMATION_MS
        )),
        "{case}"
    );
    assert_eq!(
        spent(&error),
        Some((CONFIRMATION_MS, CONFIRMATION_MS)),
        "{case}"
    );
    assert_eq!(value(&error, "page_ids"), Value::Null, "{case}: {error}");
    assert_eq!(runtime.inputs().len(), 1, "{case}: no second input");
    assert_eq!(finished(runtime), ["<unrecognized>"], "{case}");
}

// The page each attempt of `open` finished on (its `StepFinished`), in order.
fn finished(runtime: &ScriptedRuntime) -> Vec<String> {
    runtime
        .traces()
        .iter()
        .filter_map(|trace| match trace {
            ContainedTaskTrace::StepFinished { page_label, .. } => Some(page_label.clone()),
            _ => None,
        })
        .collect()
}

// Raised: after the tap the device shows a dark screen, which is neither `result` nor `home`, on
// every frame. The arrival does not see `result`; the retry decision, 100 ms later, sees neither
// page on frames that do not change, and its budget ends: the run fails with the new code, the
// detail `page_confirmation_failed` gives that end, the values naming the operation, the pages the
// decision awaited (the gate first, the step's own page last) and the attempts, the decision's
// timing, and no second input.
#[test]
fn u1_a_settled_screen_of_neither_page_ends_with_the_unrecorded_screen_code() {
    let task = plan(json!({}));
    let (result, runtime) = run(
        &task,
        vec![vec![plain(HOME)], vec![plain(DARK)]],
        LoadingMode::Off,
    );
    let error = failed("a settled dark screen", result);
    assert_eq!(error.code().as_str(), UNRECORDED_SCREEN, "{error}");
    assert_eq!(error.detail(), Some(DECISION_DETAIL), "{error}");
    assert_eq!(value(&error, "pack_operation_id"), json!("open"));
    assert_eq!(value(&error, "page_ids"), json!(["result", "home"]));
    assert_eq!(value(&error, "attempts"), json!(1));
    assert_eq!(
        timing(&error),
        Some((
            TaskTimingScope::Postcondition,
            TaskTimingStage::Postcondition,
            STEP_MS
        ))
    );
    assert_eq!(runtime.inputs().len(), 1, "no input after the decision");
    assert_eq!(finished(&runtime), ["<unrecognized>"]);
}

// Not raised: the tap is swallowed every time, `home` shows throughout. The retry decision sees the
// step's own page and taps again; the second attempt has no attempt left and ends as before:
// `page_confirmation_failed`, two attempts, the arrival's budget.
#[test]
fn u1_the_own_page_still_shown_with_no_attempt_left_keeps_page_confirmation_failed() {
    let task = plan(json!({}));
    let (result, runtime) = run(
        &task,
        vec![vec![plain(HOME)], vec![plain(HOME)], vec![plain(HOME)]],
        LoadingMode::Off,
    );
    let error = failed("the step's own page", result);
    assert_eq!(error.code().as_str(), CONFIRMATION_FAILED, "{error}");
    assert_eq!(
        error.detail(),
        Some(
            "operation=open attempts=2 after_page=<unrecognized> hit_error_page=false transition=none"
        ),
        "{error}"
    );
    assert_eq!(
        timing(&error),
        Some((
            TaskTimingScope::Postcondition,
            TaskTimingStage::Postcondition,
            CONFIRMATION_MS
        ))
    );
    assert_eq!(runtime.inputs().len(), 2);
    assert_eq!(finished(&runtime), ["home", "<unrecognized>"]);
}

// Not raised: under `execution.loading.mode` `on`, the spinner over a dark screen after the tap and
// an allowance of 200 ms (`loading.max_ms`). The arrival spends the allowance and ends; the retry
// decision shares it, and its first frame, a transition frame after the allowance is spent, ends
// it: `page_confirmation_failed` with the loading values and the same detail, one input.
#[test]
fn u1_a_decision_ended_on_its_loading_allowance_keeps_page_confirmation_failed() {
    let task = plan(json!({"loading": {"max_ms": LOADING_MS}}));
    let (result, runtime) = run(
        &task,
        vec![vec![plain(HOME)], vec![spinning(DARK)]],
        LoadingMode::On,
    );
    let error = failed("the loading allowance", result);
    assert_eq!(error.code().as_str(), CONFIRMATION_FAILED, "{error}");
    assert_eq!(error.detail(), Some(DECISION_DETAIL), "{error}");
    assert_eq!(
        (
            value(&error, "loading_page_id"),
            value(&error, "loading_ms"),
            value(&error, "loading_limit_ms")
        ),
        (json!("spinner"), json!(LOADING_MS), json!(LOADING_MS))
    );
    assert_eq!(runtime.inputs().len(), 1);
}

// Not raised: after the tap the screen keeps changing, dark and grey frame by frame, neither a page
// of the package. The retry decision sees neither page and its budget ends on a frame that differs
// from the one before it: `page_confirmation_failed` with the same detail and timing, one input.
#[test]
fn u1_a_screen_still_changing_at_the_decision_s_end_keeps_page_confirmation_failed() {
    let task = plan(json!({}));
    // More frames than the arrival (21) and the decision (21) take; the last one repeats.
    let changing = (0..80)
        .map(|index| plain(if index % 2 == 0 { DARK } else { GREY }))
        .collect::<Vec<_>>();
    let (result, runtime) = run(&task, vec![vec![plain(HOME)], changing], LoadingMode::Off);
    let error = failed("a changing screen", result);
    assert_eq!(error.code().as_str(), CONFIRMATION_FAILED, "{error}");
    assert_eq!(error.detail(), Some(DECISION_DETAIL), "{error}");
    assert_eq!(
        timing(&error),
        Some((
            TaskTimingScope::Postcondition,
            TaskTimingStage::Postcondition,
            STEP_MS
        ))
    );
    assert_eq!(runtime.inputs().len(), 1);
    assert!(
        runtime.captures_after(1) < 80,
        "the script outlasted the waits: {} frames",
        runtime.captures_after(1)
    );
}

// Fix round 1, raised: `open` declares no retry; after the tap the device shows a dark screen,
// neither `result` nor `home`, on every frame. The arrival runs out; the closing decision, made at
// once on `result` and then `home`, sees neither page on frames that do not change, and its budget
// ends: the run fails with the new code, the detail and the timing `page_confirmation_failed`
// gives the arrival's end (its 1 000 ms of 1 000), the values naming the operation, the pages the
// decision awaited (the arrival's page first, the step's own page last) and the attempts, one input
// in the whole run and one `StepFinished` naming `<unrecognized>`.
#[test]
fn u1_without_a_retry_a_settled_screen_of_neither_page_ends_with_the_unrecorded_screen_code() {
    let (result, runtime) = run_without_retry(vec![plain(DARK)], TASK_MS);
    let error = failed("a settled dark screen", result);
    assert_eq!(error.code().as_str(), UNRECORDED_SCREEN, "{error}");
    assert_eq!(error.detail(), Some(DECISION_DETAIL), "{error}");
    assert_eq!(value(&error, "pack_operation_id"), json!("open"));
    assert_eq!(value(&error, "page_ids"), json!(["result", "home"]));
    assert_eq!(value(&error, "attempts"), json!(1));
    assert_eq!(
        timing(&error),
        Some((
            TaskTimingScope::Postcondition,
            TaskTimingStage::Postcondition,
            CONFIRMATION_MS
        ))
    );
    assert_eq!(spent(&error), Some((CONFIRMATION_MS, CONFIRMATION_MS)));
    assert_eq!(runtime.inputs().len(), 1, "one input in the whole run");
    assert_eq!(finished(&runtime), ["<unrecognized>"]);
}

// Fix round 1, not raised: without a retry the tap is swallowed, `home` shows throughout. The
// closing decision's first frame shows the step's own page: the run ends as the arrival ended it.
#[test]
fn u1_without_a_retry_the_own_page_still_shown_keeps_page_confirmation_failed() {
    let (result, runtime) = run_without_retry(vec![plain(HOME)], TASK_MS);
    assert_ends_as_the_arrival("the step's own page", result, &runtime);
    assert_eq!(
        runtime.captures_after(1),
        22,
        "the arrival's 21 frames and the closing decision's first"
    );
}

// Fix round 1, still a failure: without a retry the screen is dark after the tap for longer than
// the arrival waits, then `result` (the 23rd frame). `result` passes only on the closing decision,
// which goes on with nothing: the run fails as the arrival ended it, and the closing decision
// takes no frame after the one `result` passed on.
#[test]
fn u1_without_a_retry_an_awaited_page_only_on_the_closing_decision_still_fails_as_before() {
    let mut late = vec![plain(DARK); 22];
    late.push(plain(RESULT));
    let (result, runtime) = run_without_retry(late, TASK_MS);
    assert_ends_as_the_arrival("the next page on the closing decision", result, &runtime);
    assert_eq!(runtime.captures_after(1), 23);
}

// Fix round 1, not raised: without a retry the screen changes on every frame after the tap. The
// closing decision sees neither page and its budget ends on a frame that differs from the one
// before it: the run ends as the arrival ended it.
#[test]
fn u1_without_a_retry_a_screen_still_changing_keeps_page_confirmation_failed() {
    let (result, runtime) = run_without_retry(changing_screen(), TASK_MS);
    assert_ends_as_the_arrival("a changing screen", result, &runtime);
}

// Closing round: without a retry, the task's deadline 500 ms into the closing decision (a task
// timeout of 1 500 ms), the dark screen of the raised case. The run had failed when the arrival
// ended, and the closing decision only classifies that failure: the deadline that cuts it, before
// its own budget ends, leaves the end the arrival gave, `page_confirmation_failed` with its detail
// and timing, neither the new code nor the task timeout, after one input.
#[test]
fn u1_without_a_retry_the_task_deadline_inside_the_closing_decision_keeps_page_confirmation_failed()
{
    let (result, runtime) = run_without_retry(vec![plain(DARK)], DEADLINE_TASK_MS);
    assert_ends_as_the_arrival("the task's deadline", result, &runtime);
    assert!(
        runtime.captures_after(1) < 42,
        "the deadline cut the closing decision: {} frames",
        runtime.captures_after(1)
    );
}

// Fix round 1: the closing decision makes no input and at most the captures of one retry decision.
// On the settled dark screen and on the changing one, a run of `open` without a retry makes one
// input and takes after it exactly the frames a run with a retry takes, the arrival's and one
// decision's (its retry decision sees neither page either, so it does not tap again).
#[test]
fn u1_the_closing_decision_inputs_nothing_and_takes_the_captures_of_one_retry_decision() {
    for (case, after_tap) in [
        ("a settled screen", vec![plain(DARK)]),
        ("a changing screen", changing_screen()),
    ] {
        let (_, closing) = run_without_retry(after_tap.clone(), TASK_MS);
        let (_, retried) = run(
            &plan(json!({})),
            vec![vec![plain(HOME)], after_tap],
            LoadingMode::Off,
        );
        assert_eq!(closing.inputs().len(), 1, "{case}: no input");
        assert_eq!(retried.inputs().len(), 1, "{case}");
        assert_eq!(
            closing.captures_after(1),
            retried.captures_after(1),
            "{case}: the arrival's frames and one decision's"
        );
    }
}
