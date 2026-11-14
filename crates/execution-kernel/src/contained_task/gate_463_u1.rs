// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 U1 (register §1.372; contracts/linear-steps.md, "Failure codes" and "A screen the
// package never recorded"; coordinator work orders `rd-1114/brief-463-u1-unrecorded-screen.md`,
// `rd-1114/brief-1715-fix1.md`, `rd-1114/brief-1715-fix2.md` and `rd-1114/brief-1715-fix3.md`):
// where a linear step's failed wait
// ends the run with `page_confirmation_failed`, a task's own run outside a restart segment first
// makes one closing decision. It observes for the step timeout and only classifies: no input, no
// further attempt, no frame handed to the shared interrupt pages, and a page that passes on it
// leaves the run failed as before. The run ends with `contained_task_linear_unrecorded_screen`,
// with the detail, the timing and the values of that end, only when from the decision's first frame
// to its bound's end no frame passed an awaited page or the step's own page, none was a loading
// frame or showed a shared interrupt page, its first and last frames and its last two captures are
// alike by the frame-change measure, and no other page of the package passes on its last frame
// (fix round 2, rules 1 to 3). Every other end keeps the code, the detail and the timing it had:
// - the step's own page or an awaited page on a frame of the decision;
// - a screen still changing, or whose last two captures, or whose first and last frames, differ;
// - another page of the package on the last frame;
// - a shared interrupt page on a frame of the decision, recognised and never pressed, and an error
//   of the decision (its recognising check of those pages failed, its capture failed, the task's
//   deadline; fix round 3: whatever its kind, a failure of the runtime's bookkeeping alone passed
//   on);
// - no closing decision at all after a missed intermediate page (rule 1 (a)), after a wait that
//   ended on its loading allowance, in a run the host starts itself (rule 5), and when the run's
//   time left is shorter than twice the decision's bound (rule 6, fix round 3).
// A retry decision that sees neither page raises nothing itself: the closing decision follows it
// (rule 2); the last attempt of a step that declares retries gets one too (rule 4).
//
// Every expected value comes from those lines and the package values below, never from the code.
// The runs play scripted frames on the scripted clock (EXEC-I4) at the factor 1000: the device
// shows `screens[n]` after `n` inputs, each segment's last frame repeating, a frame is delivered at
// once, and every pause moves the clock by its length. A frame is two pixels: the first names the
// page (`home` red, `result` green, `wait` blue; dark and grey are no page), a white second is the
// loading page `spinner`. The package declares no capture interval, so a wait takes a frame every
// 50 ms without back-off. The step `open` taps `home` and expects `result` within 600 ms (13
// frames); where it declares a retry it may be attempted twice, 100 ms apart, and its retry
// decision waits the step timeout of 1 000 ms (21 frames); the closing decision waits the step
// timeout too (21 frames). Captures are counted from the run's first frame, which shows `home`.

use super::tests::scripted::{ScriptedFrame, ScriptedRuntime};
use super::*;
use actingcommand_page_detector::LoadingPageSet;
use actingcommand_recognition_pack::RecognitionPack;
use serde_json::{Value, json};
use std::path::PathBuf;

const UNRECORDED_SCREEN: &str = ExecutionKernelCode::ContainedTaskLinearUnrecordedScreen.as_str();
const CONFIRMATION_FAILED: &str = ExecutionKernelCode::PageConfirmationFailed.as_str();

// The arrival's budget and the step timeout differ, so `limit_ms` tells which timer a failure's
// timing names.
const CONFIRMATION_MS: u64 = 600;
const STEP_MS: u64 = 1_000;
const TASK_MS: u64 = 600_000;
// A task timeout whose deadline falls 500 ms into the closing decision of a step without a retry,
// which starts when the arrival's 600 ms run out.
const DEADLINE_TASK_MS: u64 = 1_100;
const LOADING_MS: u64 = 200;
const TASK_FILE: &str = "resources/operations/task/task.json";
// The frames a wait of 600 ms and one of 1 000 ms take, one every 50 ms from its first at once.
const ARRIVAL_FRAMES: usize = 13;
const DECISION_FRAMES: usize = 21;
// Fix round 3: the frames the wait of 300 ms for the intermediate page takes, likewise.
const INTERMEDIATE_FRAMES: usize = 7;
// Fix round 3: the frames the arrival takes on the spinner under `on`: its first at once, one every
// 50 ms; the allowance of 200 ms is spent at its fifth, so its sixth, a transition frame, ends it.
const LOADING_ARRIVAL_FRAMES: usize = 6;
// The capture number of the closing decision's first frame after the arrival of a step without a
// retry: the run's first frame, then the arrival's.
const FIRST_CLOSING_CAPTURE: usize = 1 + ARRIVAL_FRAMES + 1;
// The detail `page_confirmation_failed` gives the first attempt of `open` that saw neither page
// (contracts/linear-steps.md, "Failure codes" and "Failure detail").
const DECISION_DETAIL: &str =
    "operation=open attempts=1 after_page=<unrecognized> hit_error_page=false transition=none";

const HOME: [u8; 3] = [255, 0, 0];
const RESULT: [u8; 3] = [0, 255, 0];
const WAIT: [u8; 3] = [0, 0, 255];
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
            colour("page/wait", 0, WAIT),
            colour("loading/spinner", 1, WHITE)
        ]
    }))
    .expect("recognition pack")
}

// The package's pages: `home` and `result` are the step's, `wait` a page of the package that no
// wait of `open` awaits unless `open` declares it as its intermediate page.
fn page_set() -> PageSet {
    serde_json::from_value(json!({"schema_version": "0.3", "pages": [
        {"id": "home", "required": ["page/home"]},
        {"id": "result", "required": ["page/result"]},
        {"id": "wait", "required": ["page/wait"]}
    ]}))
    .expect("page set")
}

fn loading_pages() -> LoadingPageSet {
    serde_json::from_value(json!({"max_ms": LOADING_MS, "pages": [
        {"id": "spinner", "required": ["loading/spinner"]}
    ]}))
    .expect("loading pages")
}

// Whether `open` declares its retry (twice, 100 ms apart) or none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Retry {
    Declared,
    Undeclared,
}

// The step plan `home` -> `result` of the one tap `open` with a retry, its `expect_after` with
// `expect_after` members, admitted as the package's admission admits it.
fn plan(expect_after: Value) -> PreparedContainedTask {
    plan_of(expect_after, json!({}), Retry::Declared, TASK_MS)
}

// The same step plan, `open` with the members `members` and `retry`, the task timeout `task_ms`.
fn plan_of(
    expect_after: Value,
    members: Value,
    retry: Retry,
    task_ms: u64,
) -> PreparedContainedTask {
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
    for (member, value) in members.as_object().expect("operation members") {
        operation[member] = value.clone();
    }
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
    run_on(task, ScriptedRuntime::new(screens).with_loading_mode(mode))
}

fn run_on(
    task: &PreparedContainedTask,
    mut runtime: ScriptedRuntime,
) -> (RunResult, ScriptedRuntime) {
    let result = task.run(&mut runtime);
    (result, runtime)
}

// The device of a run of `open` without a retry: `home`, then `after_tap`.
fn device_without_retry(after_tap: Vec<ScriptedFrame>) -> ScriptedRuntime {
    ScriptedRuntime::new(vec![vec![plain(HOME)], after_tap])
}

// The run of `open` without a retry on `runtime`, its task timeout `task_ms`.
fn run_without_retry_on(runtime: ScriptedRuntime, task_ms: u64) -> (RunResult, ScriptedRuntime) {
    run_on(
        &plan_of(json!({}), json!({}), Retry::Undeclared, task_ms),
        runtime,
    )
}

// The run of `open` without a retry on `after_tap`.
fn run_without_retry(after_tap: Vec<ScriptedFrame>) -> (RunResult, ScriptedRuntime) {
    run_without_retry_on(device_without_retry(after_tap), TASK_MS)
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

// The elapsed time and the limit of a failure's timing, in milliseconds.
fn spent(error: &ContainedTaskError) -> Option<(u64, u64)> {
    error
        .timing()
        .map(|failure| (failure.elapsed_ms, failure.limit_ms))
}

// The postcondition timing whose limit is `limit_ms`.
fn postcondition(limit_ms: u64) -> Option<(TaskTimingScope, TaskTimingStage, u64)> {
    Some((
        TaskTimingScope::Postcondition,
        TaskTimingStage::Postcondition,
        limit_ms,
    ))
}

// A screen that changes on every frame, dark and grey, neither a page of the package, for more
// frames than the waits of these runs take; the last one repeats.
fn changing_screen() -> Vec<ScriptedFrame> {
    (0..80)
        .map(|index| plain(if index % 2 == 0 { DARK } else { GREY }))
        .collect()
}

// What the failure of `open` without a retry carries when it ends as its failed arrival ended it:
// `page_confirmation_failed` with the arrival's detail and timing (its 600 ms of 600) and no
// values, after one input, its one `StepFinished` naming `<unrecognized>`.
fn assert_ends_as_the_arrival(case: &str, result: RunResult, runtime: &ScriptedRuntime) {
    let error = failed(case, result);
    assert_eq!(
        error.code().as_str(),
        CONFIRMATION_FAILED,
        "{case}: {error}"
    );
    assert_eq!(error.detail(), Some(DECISION_DETAIL), "{case}: {error}");
    assert_eq!(timing(&error), postcondition(CONFIRMATION_MS), "{case}");
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

// Rule 2: `open` declares no retry; after the tap the device shows a dark screen, neither `result`
// nor `home`, on every frame. The arrival runs out; the closing decision, made at once on `result`
// and then `home`, sees neither page, no loading frame and no shared interrupt page on any of its
// frames until its bound, its frames unchanged, and no other page of the package (`wait`, the
// loading page) on its last: the run fails with the new code, the detail and the timing
// `page_confirmation_failed` gives the arrival's end, the values naming the operation, the pages
// the decision awaited (the arrival's page first, the step's own page last) and the attempts. One
// input in the whole run; the closing decision takes its 21 frames after the arrival's 13, asks
// on each whether a shared interrupt page is shown, and offers none of them to be handled.
#[test]
fn u1_without_a_retry_a_screen_unknown_and_still_for_the_whole_bound_gets_the_code() {
    let (result, runtime) = run_without_retry(vec![plain(DARK)]);
    let error = failed("a still dark screen", result);
    assert_eq!(error.code().as_str(), UNRECORDED_SCREEN, "{error}");
    assert_eq!(error.detail(), Some(DECISION_DETAIL), "{error}");
    assert_eq!(value(&error, "pack_operation_id"), json!("open"));
    assert_eq!(value(&error, "page_ids"), json!(["result", "home"]));
    assert_eq!(value(&error, "attempts"), json!(1));
    assert_eq!(timing(&error), postcondition(CONFIRMATION_MS));
    assert_eq!(spent(&error), Some((CONFIRMATION_MS, CONFIRMATION_MS)));
    assert_eq!(runtime.inputs().len(), 1, "one input in the whole run");
    assert_eq!(finished(&runtime), ["<unrecognized>"]);
    assert_eq!(runtime.captures_after(1), ARRIVAL_FRAMES + DECISION_FRAMES);
    assert_eq!(runtime.popup_checks(), DECISION_FRAMES);
    assert_eq!(runtime.interrupt_offers(), ARRIVAL_FRAMES, "none offered");
}

// Rule 2: the same screen gives way to an awaited page (`result`) or to the step's own page
// (`home`) on the closing decision's eighth frame, before its bound ends. The closing decision ends
// there with nothing: the run fails as the arrival ended it, and no frame is taken after that one.
#[test]
fn u1_a_page_that_shows_before_the_closing_bound_ends_keeps_page_confirmation_failed() {
    for (case, page) in [("the awaited page", RESULT), ("the step's own page", HOME)] {
        let mut after_tap = vec![plain(DARK); ARRIVAL_FRAMES + 7];
        after_tap.push(plain(page));
        let (result, runtime) = run_without_retry(after_tap);
        assert_ends_as_the_arrival(case, result, &runtime);
        assert_eq!(runtime.captures_after(1), ARRIVAL_FRAMES + 8, "{case}");
    }
}

// Rule 2: `open` declares a retry; after the tap the dark screen on every frame. The arrival runs
// out, the retry decision 100 ms later sees neither page and raises nothing itself, and the closing
// decision that follows it at once sees the still unknown screen to its bound: the new code, with
// the detail and the timing `page_confirmation_failed` gives the retry decision's end (its 1 000 ms
// of the step timeout), one input, and the arrival's, the retry decision's and the closing
// decision's frames.
#[test]
fn u1_with_a_retry_the_code_comes_from_the_closing_decision_after_the_retry_decision() {
    let (result, runtime) = run(
        &plan(json!({})),
        vec![vec![plain(HOME)], vec![plain(DARK)]],
        LoadingMode::Off,
    );
    let error = failed("a still dark screen", result);
    assert_eq!(error.code().as_str(), UNRECORDED_SCREEN, "{error}");
    assert_eq!(error.detail(), Some(DECISION_DETAIL), "{error}");
    assert_eq!(value(&error, "pack_operation_id"), json!("open"));
    assert_eq!(value(&error, "page_ids"), json!(["result", "home"]));
    assert_eq!(value(&error, "attempts"), json!(1));
    assert_eq!(timing(&error), postcondition(STEP_MS));
    assert_eq!(spent(&error), Some((STEP_MS, STEP_MS)));
    assert_eq!(runtime.inputs().len(), 1, "no input after either decision");
    assert_eq!(finished(&runtime), ["<unrecognized>"]);
    assert_eq!(
        runtime.captures_after(1),
        ARRIVAL_FRAMES + DECISION_FRAMES + DECISION_FRAMES
    );
    assert_eq!(runtime.popup_checks(), DECISION_FRAMES);
}

// Rule 4: the last attempt of a step that declares retries. The first tap is swallowed (`home`),
// the retry decision takes the step's own page and the step taps again; after the second tap the
// dark screen on every frame. The second arrival runs out with no attempt left, and the closing
// decision sees the still unknown screen to its bound: the new code with the second attempt's
// detail, the arrival's timing and `attempts` 2, two inputs.
#[test]
fn u1_the_last_attempt_of_a_step_with_retries_gets_the_code_from_its_closing_decision() {
    let (result, runtime) = run(
        &plan(json!({})),
        vec![vec![plain(HOME)], vec![plain(HOME)], vec![plain(DARK)]],
        LoadingMode::Off,
    );
    let error = failed("the last attempt", result);
    assert_eq!(error.code().as_str(), UNRECORDED_SCREEN, "{error}");
    assert_eq!(
        error.detail(),
        Some(
            "operation=open attempts=2 after_page=<unrecognized> hit_error_page=false transition=none"
        ),
        "{error}"
    );
    assert_eq!(value(&error, "page_ids"), json!(["result", "home"]));
    assert_eq!(value(&error, "attempts"), json!(2));
    assert_eq!(timing(&error), postcondition(CONFIRMATION_MS));
    assert_eq!(runtime.inputs().len(), 2);
    assert_eq!(finished(&runtime), ["home", "<unrecognized>"]);
    assert_eq!(runtime.captures_after(2), ARRIVAL_FRAMES + DECISION_FRAMES);
}

// Rule 1 (a): `open` declares the intermediate page `wait` (300 ms) and a retry. After the tap
// `wait` flashed past between two frames and the device rests on a dark screen, no page of the
// package, unchanged on every frame (fix round 3: a screen a closing decision would name). The
// intermediate wait misses `wait` in its 7 frames, and the retry decision on `wait` and `home` sees
// neither in its 21; a missed intermediate page is never classified, so no closing decision follows:
// the run fails `page_confirmation_failed` with the retry decision's detail and timing, no values,
// one input, and no frame after the retry decision's (a closing decision would take 21 more and
// name the dark screen).
#[test]
fn u1_an_intermediate_page_that_flashed_past_keeps_page_confirmation_failed() {
    let task = plan_of(
        json!({}),
        json!({"transition": {"kind": "page", "page_id": "wait", "timeout_ms": 300}}),
        Retry::Declared,
        TASK_MS,
    );
    let (result, runtime) = run(
        &task,
        vec![vec![plain(HOME)], vec![plain(DARK)]],
        LoadingMode::Off,
    );
    let error = failed("a missed intermediate page", result);
    assert_eq!(error.code().as_str(), CONFIRMATION_FAILED, "{error}");
    assert_eq!(
        error.detail(),
        Some(
            "operation=open attempts=1 after_page=<unrecognized> hit_error_page=false transition=page"
        ),
        "{error}"
    );
    assert_eq!(timing(&error), postcondition(STEP_MS));
    assert_eq!(value(&error, "page_ids"), Value::Null, "{error}");
    assert_eq!(runtime.inputs().len(), 1);
    assert_eq!(
        runtime.captures_after(1),
        INTERMEDIATE_FRAMES + DECISION_FRAMES,
        "no closing decision"
    );
    assert_eq!(runtime.popup_checks(), 0, "no closing decision");
}

// Rule 1 (c) and rule 2: the closing decision compares its last two captures, in capture order,
// and its first and its last frame. Its second-to-last frame grey, every other dark: the first and
// the last frame are alike, the last two captures are not. Its first frame grey, every later one
// dark: the last two captures are alike, the first and the last frame are not. Neither is a still
// screen: the run fails as the arrival ended it, after the closing decision's whole bound.
#[test]
fn u1_the_closing_decision_compares_its_last_two_captures_and_its_first_and_last_frame() {
    for (case, grey) in [
        (
            "the last two captures differ",
            ARRIVAL_FRAMES + DECISION_FRAMES - 2,
        ),
        ("the first and the last frame differ", ARRIVAL_FRAMES),
    ] {
        let mut after_tap = vec![plain(DARK); grey];
        after_tap.push(plain(GREY));
        after_tap.push(plain(DARK));
        let (result, runtime) = run_without_retry(after_tap);
        assert_ends_as_the_arrival(case, result, &runtime);
        assert_eq!(
            runtime.captures_after(1),
            ARRIVAL_FRAMES + DECISION_FRAMES,
            "{case}"
        );
    }
}

// Rule 1 (d): after the tap the device shows `wait`, a page the package declares that no wait of
// `open` awaits, unchanged on every frame. The closing decision sees neither `result` nor `home`
// to its bound on a still screen, and its last frame, evaluated against the package's other pages,
// shows `wait`: no screen the package never recorded, and the run fails as the arrival ended it.
#[test]
fn u1_another_page_of_the_package_on_the_last_frame_keeps_page_confirmation_failed() {
    let (result, runtime) = run_without_retry(vec![plain(WAIT)]);
    assert_ends_as_the_arrival("the package's page wait", result, &runtime);
    assert_eq!(runtime.captures_after(1), ARRIVAL_FRAMES + DECISION_FRAMES);
}

// Rule 3: from the closing decision's first frame on, the device shows a shared interrupt page
// over the dark screen, which an offer would press. The closing decision recognises it on that
// frame and ends there with nothing pressed: one input, its one recognising check, no offer but
// the arrival's 13, and the run fails as the arrival ended it.
#[test]
fn u1_a_shared_interrupt_page_in_the_closing_decision_is_recognised_and_never_pressed() {
    let device =
        device_without_retry(vec![plain(DARK)]).with_shared_popup_from(FIRST_CLOSING_CAPTURE);
    let (result, runtime) = run_without_retry_on(device, TASK_MS);
    assert_ends_as_the_arrival("a shared interrupt page", result, &runtime);
    assert_eq!(runtime.popup_checks(), 1);
    assert_eq!(runtime.interrupt_offers(), ARRIVAL_FRAMES, "none offered");
    assert_eq!(runtime.captures_after(1), ARRIVAL_FRAMES + 1);
}

// Rule 3: the closing decision's check of the shared interrupt pages fails with
// `contained_task_recognition_failed` on its first frame. An error inside the closing decision
// ends it with nothing: the run fails as the arrival ended it, never with the error's own code.
#[test]
fn u1_a_recognition_error_inside_the_closing_decision_keeps_page_confirmation_failed() {
    let device = device_without_retry(vec![plain(DARK)])
        .with_failing_popup_check_from(FIRST_CLOSING_CAPTURE);
    let (result, runtime) = run_without_retry_on(device, TASK_MS);
    assert_ends_as_the_arrival("a failed recognition", result, &runtime);
    assert_eq!(runtime.captures_after(1), ARRIVAL_FRAMES + 1);
}

// Fix round 3 (item 3): the closing decision's first capture fails with an operation error of the
// device, which the runtime does not count as a failure of its bookkeeping. An error of the closing
// decision's own capture ends it with nothing, whatever its kind: the run fails as the arrival
// ended it, a task failure, never the capture's error; the device showed the arrival's frames only.
#[test]
fn u1_a_capture_error_inside_the_closing_decision_keeps_page_confirmation_failed() {
    let device =
        device_without_retry(vec![plain(DARK)]).with_failing_capture_from(FIRST_CLOSING_CAPTURE);
    let (result, runtime) = run_without_retry_on(device, TASK_MS);
    assert_ends_as_the_arrival("a failed capture", result, &runtime);
    assert_eq!(runtime.captures(), FIRST_CLOSING_CAPTURE);
    assert_eq!(runtime.captures_after(1), ARRIVAL_FRAMES);
    assert_eq!(runtime.popup_checks(), 0);
}

// Rule 3: the task's deadline 500 ms into the closing decision (a task timeout of 1 100 ms) on the
// still dark screen. The deadline that cuts the closing decision leaves the end the arrival gave,
// neither the new code nor the task timeout, after fewer frames than a whole decision.
#[test]
fn u1_the_task_deadline_inside_the_closing_decision_keeps_page_confirmation_failed() {
    let (result, runtime) =
        run_without_retry_on(device_without_retry(vec![plain(DARK)]), DEADLINE_TASK_MS);
    assert_ends_as_the_arrival("the task's deadline", result, &runtime);
    assert!(
        runtime.captures_after(1) < ARRIVAL_FRAMES + DECISION_FRAMES,
        "the deadline cut the closing decision: {} frames",
        runtime.captures_after(1)
    );
}

// Rule 5: the same still dark screen in a run the runtime started itself (a startup package, a
// ladder rung's package). Such a run never names a screen its package never recorded: no closing
// decision is made, and the run fails as the arrival ended it after the arrival's frames alone.
#[test]
fn u1_a_run_the_runtime_started_itself_makes_no_closing_decision() {
    let device = device_without_retry(vec![plain(DARK)]).host_started();
    let (result, runtime) = run_without_retry_on(device, TASK_MS);
    assert_ends_as_the_arrival("a run the runtime started", result, &runtime);
    assert_eq!(runtime.captures_after(1), ARRIVAL_FRAMES);
    assert_eq!(runtime.popup_checks(), 0);
}

// Rule 6, fix round 3 (item 1): the closing decision runs only when the run has at least twice its
// bound left (it lasts its bound, and the capture and recognition of its last frame, the evaluation
// of the package's other pages and the work before the settlement come after it). The same still
// dark screen when the runtime answers that the run has 1 999 ms left, just below twice the bound
// of 1 000 ms: no closing decision, the arrival's end and frames. With 2 000 ms left, just at twice
// the bound, the closing decision is made and the run gets the new code.
#[test]
fn u1_a_run_with_less_time_left_than_the_closing_bound_makes_no_closing_decision() {
    let short = device_without_retry(vec![plain(DARK)])
        .with_time_left(Duration::from_millis(2 * STEP_MS - 1));
    let (result, runtime) = run_without_retry_on(short, TASK_MS);
    assert_ends_as_the_arrival("1 999 ms left", result, &runtime);
    assert_eq!(runtime.captures_after(1), ARRIVAL_FRAMES);
    assert_eq!(runtime.popup_checks(), 0);

    let enough =
        device_without_retry(vec![plain(DARK)]).with_time_left(Duration::from_millis(2 * STEP_MS));
    let (result, runtime) = run_without_retry_on(enough, TASK_MS);
    let error = failed("2 000 ms left", result);
    assert_eq!(error.code().as_str(), UNRECORDED_SCREEN, "{error}");
    assert_eq!(runtime.captures_after(1), ARRIVAL_FRAMES + DECISION_FRAMES);
}

// Not raised, as before: the tap is swallowed every time, `home` shows throughout. The retry
// decision sees the step's own page and taps again; the second attempt has no attempt left, and
// its closing decision sees the step's own page on its first frame: `page_confirmation_failed`,
// two attempts, the arrival's budget.
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
    assert_eq!(timing(&error), postcondition(CONFIRMATION_MS));
    assert_eq!(runtime.inputs().len(), 2);
    assert_eq!(finished(&runtime), ["home", "<unrecognized>"]);
    assert_eq!(runtime.captures_after(2), ARRIVAL_FRAMES + 1);
}

// Not raised, as before: under `execution.loading.mode` `on`, the spinner over a dark screen after
// the tap and an allowance of 200 ms (`loading.max_ms`). The arrival spends the allowance and ends
// in its 6 frames; the retry decision shares it, and its first frame, a transition frame after the
// allowance is spent, ends it: `page_confirmation_failed` with the loading values and the same
// detail, one input, and no closing decision after a decision that ended on its loading allowance:
// no frame after the retry decision's one (fix round 3: a closing decision would take one more, its
// first, a transition frame, before it ended).
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
    assert_eq!(
        runtime.captures_after(1),
        LOADING_ARRIVAL_FRAMES + 1,
        "no closing decision"
    );
    assert_eq!(runtime.popup_checks(), 0, "no closing decision");
}

// Not raised: after the tap the screen keeps changing, dark and grey frame by frame, neither a page
// of the package. With a retry, the retry decision sees neither page and the closing decision after
// it sees no still screen to its bound: `page_confirmation_failed` with the retry decision's detail
// and timing, one input. Without a retry, the closing decision after the arrival: the arrival's end.
#[test]
fn u1_a_screen_still_changing_keeps_page_confirmation_failed() {
    let (result, runtime) = run(
        &plan(json!({})),
        vec![vec![plain(HOME)], changing_screen()],
        LoadingMode::Off,
    );
    let error = failed("a changing screen after a retry decision", result);
    assert_eq!(error.code().as_str(), CONFIRMATION_FAILED, "{error}");
    assert_eq!(error.detail(), Some(DECISION_DETAIL), "{error}");
    assert_eq!(timing(&error), postcondition(STEP_MS));
    assert_eq!(runtime.inputs().len(), 1);
    assert_eq!(
        runtime.captures_after(1),
        ARRIVAL_FRAMES + DECISION_FRAMES + DECISION_FRAMES
    );

    let (result, runtime) = run_without_retry(changing_screen());
    assert_ends_as_the_arrival("a changing screen after the arrival", result, &runtime);
    assert_eq!(runtime.captures_after(1), ARRIVAL_FRAMES + DECISION_FRAMES);
}
