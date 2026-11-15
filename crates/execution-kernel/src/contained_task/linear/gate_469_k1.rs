// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #469 K1 (register §1.395; contracts/linear-steps.md, "What the program saw"; the owner's
// option A on board #469; the coordinator's work order `rd-1115/brief-469-word-slices.md` and the
// accepted design `rd-1115/status-codes-a-design.md`, place 1, the linear parts): every end of a
// `linear_steps` step that the closing decision of Workflow #463 U1 classifies carries
// `confirmation_problem`, the word of what the program saw there, in its outcome's values and never
// in its detail. Where the closing decision is made, its verdict: the step's own page on one of its
// frames (`input_not_taken`), an awaited page (`awaited_page_late`), a transition frame
// (`transition_shown`), a shared interrupt page (`interrupt_shown`), frames that changed at its
// bound (`screen_moving`), another page of the package on its last frame (`other_page_shown`) or a
// loading page there (`loading_page_shown`), the task's deadline (`deadline_reached`), an error of
// its own check or capture (`check_failed`), and otherwise the screen the package never recorded
// (`unrecorded_screen`, which ends the run with `contained_task_linear_unrecorded_screen` as before
// and always carries that word). Where it is not made: a wait that ended on its loading allowance
// (`loading_limit`), a retry decision that saw neither page (`neither_page`), and otherwise the
// condition that kept it from being made (`unjudged_layer_run`, `unjudged_restart_segment`,
// `unjudged_consensus`, `unjudged_time_short`). Every end keeps its code, its detail byte for byte
// (the failure identity reads the code, the scope, the step and fragments of the detail, and the
// streak compares the code, the identity and the class), its timing and the other values it had;
// the host's half (`runtime-host`, `gate_469_k1`) pins the identity string and the streak.
//
// Every expected value comes from those lines and the package values below, never from the code.
// The fixture is U1's (`gate_463_u1`): scripted frames on the scripted clock at the factor 1000;
// the device shows `screens[n]` after `n` effects, each segment's last frame repeating, a frame is
// delivered at once, and every pause moves the clock by its length. A frame is two pixels: the
// first names the page (`home` red, `result` green, `wait` blue; dark and grey are no page), a white
// second is the loading page `spinner`. A wait takes a frame every 50 ms. The step `open` taps its
// page and expects `result` within 600 ms (13 frames); where it declares a retry it may be
// attempted twice, 100 ms apart, and its retry decision waits the step timeout of 1 000 ms; the
// closing decision waits the step timeout too. Captures are counted from the run's first frame.

use super::super::tests::scripted::{ScriptedFrame, ScriptedRuntime};
use super::super::*;
use actingcommand_page_detector::LoadingPageSet;
use actingcommand_recognition_pack::RecognitionPack;
use serde_json::{Map, Value, json};
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
// The pause between the two frames of a page target's sample consensus.
const SAMPLE_INTERVAL_MS: u64 = 100;
const TASK_FILE: &str = "resources/operations/task/task.json";
// The frames a wait of 600 ms, the closing decision's wait of 1 000 ms and the intermediate page's
// wait of 300 ms take, one every 50 ms from its first at once.
const ARRIVAL_FRAMES: usize = 13;
const DECISION_FRAMES: usize = 21;
const INTERMEDIATE_FRAMES: usize = 7;
// The capture number of the closing decision's first frame after the arrival of a step without a
// retry: the run's first frame, then the arrival's.
const FIRST_CLOSING_CAPTURE: usize = 1 + ARRIVAL_FRAMES + 1;

// The details `page_confirmation_failed` gives these ends (contracts/linear-steps.md, "Failure
// codes"): the first attempt, the second, the first after an intermediate page that was not seen,
// and the first after one that was.
const DETAIL: &str =
    "operation=open attempts=1 after_page=<unrecognized> hit_error_page=false transition=none";
const SECOND_ATTEMPT_DETAIL: &str =
    "operation=open attempts=2 after_page=<unrecognized> hit_error_page=false transition=none";
const INTERMEDIATE_DETAIL: &str =
    "operation=open attempts=1 after_page=<unrecognized> hit_error_page=false transition=page";
const INTERMEDIATE_SEEN_DETAIL: &str = "operation=open attempts=1 after_page=<unrecognized> \
     hit_error_page=false transition=page intermediate_seen=true";

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

// Whether the package's page targets declare a sample consensus (both of two frames 100 ms apart).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sampling {
    SingleFrame,
    TwoFrames,
}

fn pack(sampling: Sampling) -> RecognitionPack {
    let colour = |id: &str, x: u32, expected: [u8; 3]| {
        json!({"type": "color", "id": id,
               "region": {"x": x, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    let mut pack = json!({
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
    });
    if sampling == Sampling::TwoFrames {
        let consensus = ["page/home", "page/result", "page/wait"]
            .into_iter()
            .map(|id| {
                let two_frames = json!({"samples": [{"frame": 0}, {"frame": 1}], "k": 2,
                                        "sample_interval_ms": SAMPLE_INTERVAL_MS});
                (id.to_owned(), two_frames)
            })
            .collect::<Map<String, Value>>();
        pack["target_consensus"] = Value::Object(consensus);
    }
    serde_json::from_value(pack).expect("recognition pack")
}

// The package's pages: `home` and `result` are the step's, `wait` a page of the package that no
// wait of `open` awaits unless `open` declares it as its intermediate page or leaves from it.
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

// The control of a step plan of `max_steps` operations within `task_ms`.
fn control(task_ms: u64, max_steps: usize) -> Value {
    json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.words.plan",
        "execution_mode": "linear_steps",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "step_timeout_ms": STEP_MS,
        "timeout_ms": task_ms,
        "max_steps": max_steps
    })
}

// The tap `open` from `from` that expects `result` within `CONFIRMATION_MS`, with the
// `expect_after` members `expect_after` and the operation's members `members`, and `retry`.
fn open(from: &str, expect_after: Value, members: Value, retry: Retry) -> Value {
    let mut expectation = json!({"page_id": "result", "timeout_ms": CONFIRMATION_MS});
    for (member, value) in expect_after.as_object().expect("expect_after members") {
        expectation[member] = value.clone();
    }
    let mut operation = json!({
        "id": "open",
        "from": from,
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
    operation
}

// The task of `operations` from `entry_page` to `result`.
fn program(entry_page: &str, operations: Vec<Value>) -> Value {
    let max_steps = operations.len();
    json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "entry_page": entry_page,
        "target_page": "result",
        "max_steps": max_steps,
        "operations": operations
    })
}

// The step plan of `control` and `program`, its recognition pack's targets sampled as `sampling`
// says, admitted as the package's admission admits it.
fn admitted(control: Value, program: Value, sampling: Sampling) -> PreparedContainedTask {
    let control: TaskControl = serde_json::from_value(control).expect("task control");
    control.validate().expect("an admitted control");
    let program: TaskProgram = serde_json::from_value(program).expect("task program");
    let evaluator =
        RecognitionEvaluator::new(PathBuf::new(), pack(sampling)).expect("recognition evaluator");
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

// The step plan `home` -> `result` of the one tap `open` with `expect_after` members, the
// operation's `members` and `retry`, the task timeout `task_ms`.
fn plan_of(
    expect_after: Value,
    members: Value,
    retry: Retry,
    task_ms: u64,
) -> PreparedContainedTask {
    admitted(
        control(task_ms, 1),
        program("home", vec![open("home", expect_after, members, retry)]),
        Sampling::SingleFrame,
    )
}

// The step plan of `open` with a retry.
fn with_retry() -> PreparedContainedTask {
    plan_of(json!({}), json!({}), Retry::Declared, TASK_MS)
}

// The intermediate page `wait` (300 ms) declared by `open`, with a retry.
fn with_intermediate_page() -> PreparedContainedTask {
    plan_of(
        json!({}),
        json!({"transition": {"kind": "page", "page_id": "wait", "timeout_ms": 300}}),
        Retry::Declared,
        TASK_MS,
    )
}

// The loading allowance of 200 ms declared by `open`'s confirmation, with `retry`.
fn with_loading(retry: Retry) -> PreparedContainedTask {
    plan_of(
        json!({"loading": {"max_ms": LOADING_MS}}),
        json!({}),
        retry,
        TASK_MS,
    )
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

// The values of a failure's outcome but `confirmation_problem`: what the end carried before the
// word.
fn other_values(error: &ContainedTaskError) -> Value {
    Value::Object(
        error
            .outcome()
            .values()
            .iter()
            .filter(|(key, _)| key.as_str() != "confirmation_problem")
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    )
}

// What every end keeps and gains: its code, its detail byte for byte (which, with the scope and
// the step, is all the failure identity reads of the failure), the postcondition timing whose limit
// is `limit_ms`, and the word `word` in its values.
fn assert_end(
    case: &str,
    error: &ContainedTaskError,
    (code, detail, limit_ms): (&str, &str, u64),
    word: &str,
) {
    assert_eq!(error.code().as_str(), code, "{case}: {error}");
    assert_eq!(error.detail(), Some(detail), "{case}: {error}");
    assert_eq!(timing(error), postcondition(limit_ms), "{case}");
    assert_eq!(
        value(error, "confirmation_problem"),
        json!(word),
        "{case}: {:?}",
        error.outcome()
    );
}

// A screen that changes on every frame, dark and grey, neither a page of the package, for more
// frames than the waits of these runs take; the last one repeats.
fn changing_screen() -> Vec<ScriptedFrame> {
    (0..80)
        .map(|index| plain(if index % 2 == 0 { DARK } else { GREY }))
        .collect()
}

// A dark screen for the arrival and the closing decision's first seven frames, then `page`.
fn dark_then(page: [u8; 3]) -> Vec<ScriptedFrame> {
    let mut after_tap = vec![plain(DARK); ARRIVAL_FRAMES + 7];
    after_tap.push(plain(page));
    after_tap
}

// The closing decision sees the step's own page: without a retry, `home` on its eighth frame after
// the arrival ran out; with a retry, the first tap swallowed and `home` throughout, on the first
// frame of the second attempt's closing decision. Each keeps the end the failed wait gave, now with
// `input_not_taken`, and no other value.
#[test]
fn k1_the_step_s_own_page_in_the_closing_decision_is_input_not_taken() {
    let (result, runtime) = run_without_retry(dark_then(HOME));
    let error = failed("without a retry", result);
    assert_end(
        "without a retry",
        &error,
        (CONFIRMATION_FAILED, DETAIL, CONFIRMATION_MS),
        "input_not_taken",
    );
    assert_eq!(spent(&error), Some((CONFIRMATION_MS, CONFIRMATION_MS)));
    assert_eq!(other_values(&error), json!({}));
    assert_eq!(runtime.inputs().len(), 1);
    assert_eq!(runtime.captures_after(1), ARRIVAL_FRAMES + 8);

    let (result, runtime) = run(
        &with_retry(),
        vec![vec![plain(HOME)], vec![plain(HOME)], vec![plain(HOME)]],
        LoadingMode::Off,
    );
    let error = failed("the last attempt", result);
    assert_end(
        "the last attempt",
        &error,
        (CONFIRMATION_FAILED, SECOND_ATTEMPT_DETAIL, CONFIRMATION_MS),
        "input_not_taken",
    );
    assert_eq!(other_values(&error), json!({}));
    assert_eq!(runtime.inputs().len(), 2);
}

// The closing decision sees the awaited page `result` on its eighth frame: `awaited_page_late`,
// and the end the failed arrival gave.
#[test]
fn k1_an_awaited_page_in_the_closing_decision_is_awaited_page_late() {
    let (result, runtime) = run_without_retry(dark_then(RESULT));
    let error = failed("the awaited page", result);
    assert_end(
        "the awaited page",
        &error,
        (CONFIRMATION_FAILED, DETAIL, CONFIRMATION_MS),
        "awaited_page_late",
    );
    assert_eq!(spent(&error), Some((CONFIRMATION_MS, CONFIRMATION_MS)));
    assert_eq!(other_values(&error), json!({}));
    assert_eq!(runtime.inputs().len(), 1);
    assert_eq!(runtime.captures_after(1), ARRIVAL_FRAMES + 8);
}

// The page `wait`, which the package declares and the closing decision does not await, still on
// every frame up to its bound: on its last frame it passes, `other_page_shown`. So after the
// arrival of a step without a retry, and after the continued wait that follows a late gate: `open`
// declares `wait` as its intermediate page and a retry, `wait` shows only after its wait, the retry
// decision takes it as the gate, and the next page does not follow.
#[test]
fn k1_another_page_of_the_package_on_the_last_frame_is_other_page_shown() {
    let (result, runtime) = run_without_retry(vec![plain(WAIT)]);
    let error = failed("after the arrival", result);
    assert_end(
        "after the arrival",
        &error,
        (CONFIRMATION_FAILED, DETAIL, CONFIRMATION_MS),
        "other_page_shown",
    );
    assert_eq!(other_values(&error), json!({}));
    assert_eq!(runtime.inputs().len(), 1);

    let mut after_tap = vec![plain(DARK); INTERMEDIATE_FRAMES];
    after_tap.push(plain(WAIT));
    let (result, runtime) = run(
        &with_intermediate_page(),
        vec![vec![plain(HOME)], after_tap],
        LoadingMode::Off,
    );
    let error = failed("after a late gate", result);
    assert_end(
        "after a late gate",
        &error,
        (
            CONFIRMATION_FAILED,
            INTERMEDIATE_SEEN_DETAIL,
            CONFIRMATION_MS,
        ),
        "other_page_shown",
    );
    assert_eq!(other_values(&error), json!({}));
    assert_eq!(runtime.inputs().len(), 1);
}

// Under `execution.loading.mode` `off`, the loading page `spinner` over a dark screen on every frame
// after the tap: no frame is a transition frame, and on the closing decision's last frame the
// loading page passes: `loading_page_shown`, and no loading values.
#[test]
fn k1_a_loading_page_on_the_last_frame_is_loading_page_shown() {
    let (result, runtime) = run_without_retry(vec![spinning(DARK)]);
    let error = failed("the spinner", result);
    assert_end(
        "the spinner",
        &error,
        (CONFIRMATION_FAILED, DETAIL, CONFIRMATION_MS),
        "loading_page_shown",
    );
    assert_eq!(spent(&error), Some((CONFIRMATION_MS, CONFIRMATION_MS)));
    assert_eq!(other_values(&error), json!({}));
    assert_eq!(runtime.inputs().len(), 1);
}

// Under `on`, an allowance of 200 ms: a dark screen for the arrival's 13 frames, which spend none of
// it, then the spinner from the closing decision's first frame, a transition frame: the decision
// ends there, `transition_shown`, and the end the arrival gave, with no loading values.
#[test]
fn k1_a_transition_frame_in_the_closing_decision_is_transition_shown() {
    let mut after_tap = vec![plain(DARK); ARRIVAL_FRAMES];
    after_tap.push(spinning(DARK));
    let (result, runtime) = run(
        &with_loading(Retry::Undeclared),
        vec![vec![plain(HOME)], after_tap],
        LoadingMode::On,
    );
    let error = failed("a transition frame", result);
    assert_end(
        "a transition frame",
        &error,
        (CONFIRMATION_FAILED, DETAIL, CONFIRMATION_MS),
        "transition_shown",
    );
    assert_eq!(spent(&error), Some((CONFIRMATION_MS, CONFIRMATION_MS)));
    assert_eq!(other_values(&error), json!({}));
    assert_eq!(runtime.captures_after(1), ARRIVAL_FRAMES + 1);
}

// A shared interrupt page from the closing decision's first frame on, recognised and never pressed:
// `interrupt_shown`, one input.
#[test]
fn k1_a_shared_interrupt_page_in_the_closing_decision_is_interrupt_shown() {
    let device =
        device_without_retry(vec![plain(DARK)]).with_shared_popup_from(FIRST_CLOSING_CAPTURE);
    let (result, runtime) = run_without_retry_on(device, TASK_MS);
    let error = failed("a shared interrupt page", result);
    assert_end(
        "a shared interrupt page",
        &error,
        (CONFIRMATION_FAILED, DETAIL, CONFIRMATION_MS),
        "interrupt_shown",
    );
    assert_eq!(other_values(&error), json!({}));
    assert_eq!(runtime.inputs().len(), 1);
    assert_eq!(runtime.popup_checks(), 1);
}

// At the closing decision's bound its frames changed: a screen that changes on every frame, its
// last two captures that differ, its first and last frame that differ; and the changing screen
// after a retry decision that saw neither page, whose end has the step timeout's limit. Each is
// `screen_moving`.
#[test]
fn k1_frames_that_changed_up_to_the_closing_bound_are_screen_moving() {
    let (result, _) = run_without_retry(changing_screen());
    let error = failed("a changing screen", result);
    assert_end(
        "a changing screen",
        &error,
        (CONFIRMATION_FAILED, DETAIL, CONFIRMATION_MS),
        "screen_moving",
    );
    assert_eq!(other_values(&error), json!({}));

    // The closing decision's second-to-last frame grey, or its first.
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
        let (result, _) = run_without_retry(after_tap);
        let error = failed(case, result);
        assert_end(
            case,
            &error,
            (CONFIRMATION_FAILED, DETAIL, CONFIRMATION_MS),
            "screen_moving",
        );
        assert_eq!(other_values(&error), json!({}), "{case}");
    }

    let (result, _) = run(
        &with_retry(),
        vec![vec![plain(HOME)], changing_screen()],
        LoadingMode::Off,
    );
    let error = failed("after a retry decision", result);
    assert_end(
        "after a retry decision",
        &error,
        (CONFIRMATION_FAILED, DETAIL, STEP_MS),
        "screen_moving",
    );
    assert_eq!(other_values(&error), json!({}));
}

// A dark screen, still for the whole closing decision and no page of the package: the run ends with
// `contained_task_linear_unrecorded_screen` as before, its detail, timing and values the ones it had
// (the operation, the pages the decision awaited, the attempts), and the word `unrecorded_screen`;
// after the arrival of a step without a retry and after a retry decision that saw neither page.
#[test]
fn k1_the_unrecorded_screen_code_always_carries_unrecorded_screen() {
    let values = json!({"pack_operation_id": "open", "page_ids": ["result", "home"],
                        "attempts": 1});
    let (result, runtime) = run_without_retry(vec![plain(DARK)]);
    let error = failed("without a retry", result);
    assert_end(
        "without a retry",
        &error,
        (UNRECORDED_SCREEN, DETAIL, CONFIRMATION_MS),
        "unrecorded_screen",
    );
    assert_eq!(spent(&error), Some((CONFIRMATION_MS, CONFIRMATION_MS)));
    assert_eq!(other_values(&error), values);
    assert_eq!(runtime.inputs().len(), 1);

    let (result, runtime) = run(
        &with_retry(),
        vec![vec![plain(HOME)], vec![plain(DARK)]],
        LoadingMode::Off,
    );
    let error = failed("after a retry decision", result);
    assert_end(
        "after a retry decision",
        &error,
        (UNRECORDED_SCREEN, DETAIL, STEP_MS),
        "unrecorded_screen",
    );
    assert_eq!(spent(&error), Some((STEP_MS, STEP_MS)));
    assert_eq!(other_values(&error), values);
    assert_eq!(runtime.inputs().len(), 1);
}

// The task's deadline 500 ms into the closing decision on the still dark screen cuts it:
// `deadline_reached`, and the end the arrival gave.
#[test]
fn k1_the_task_deadline_inside_the_closing_decision_is_deadline_reached() {
    let (result, runtime) =
        run_without_retry_on(device_without_retry(vec![plain(DARK)]), DEADLINE_TASK_MS);
    let error = failed("the task's deadline", result);
    assert_end(
        "the task's deadline",
        &error,
        (CONFIRMATION_FAILED, DETAIL, CONFIRMATION_MS),
        "deadline_reached",
    );
    assert_eq!(spent(&error), Some((CONFIRMATION_MS, CONFIRMATION_MS)));
    assert_eq!(other_values(&error), json!({}));
    assert_eq!(runtime.inputs().len(), 1);
}

// The closing decision's own check fails on its first frame: its check of the shared interrupt
// pages (`contained_task_recognition_failed`), or its capture (an operation error of the device).
// Each is `check_failed`, and the end the arrival gave, never the error's own code.
#[test]
fn k1_an_error_of_the_closing_decision_s_own_check_is_check_failed() {
    for (case, device) in [
        (
            "a failed recognising check",
            device_without_retry(vec![plain(DARK)])
                .with_failing_popup_check_from(FIRST_CLOSING_CAPTURE),
        ),
        (
            "a failed capture",
            device_without_retry(vec![plain(DARK)])
                .with_failing_capture_from(FIRST_CLOSING_CAPTURE),
        ),
    ] {
        let (result, runtime) = run_without_retry_on(device, TASK_MS);
        let error = failed(case, result);
        assert_end(
            case,
            &error,
            (CONFIRMATION_FAILED, DETAIL, CONFIRMATION_MS),
            "check_failed",
        );
        assert_eq!(other_values(&error), json!({}), "{case}");
        assert_eq!(runtime.inputs().len(), 1, "{case}");
    }
}

// Under `on`, the spinner over a dark screen after the tap and an allowance of 200 ms: with a retry,
// the arrival spends the allowance and the retry decision's first frame ends it; without one, the
// arrival ends on it. No closing decision is made: `loading_limit`, beside the loading values the
// end had.
#[test]
fn k1_a_wait_that_ended_on_its_loading_allowance_is_loading_limit() {
    let loading = json!({"loading_page_id": "spinner", "loading_ms": LOADING_MS,
                         "loading_limit_ms": LOADING_MS});
    for (case, retry, limit_ms) in [
        ("the retry decision", Retry::Declared, STEP_MS),
        ("the arrival", Retry::Undeclared, CONFIRMATION_MS),
    ] {
        let (result, runtime) = run(
            &with_loading(retry),
            vec![vec![plain(HOME)], vec![spinning(DARK)]],
            LoadingMode::On,
        );
        let error = failed(case, result);
        assert_end(
            case,
            &error,
            (CONFIRMATION_FAILED, DETAIL, limit_ms),
            "loading_limit",
        );
        assert_eq!(other_values(&error), loading, "{case}");
        assert_eq!(runtime.inputs().len(), 1, "{case}");
        assert_eq!(runtime.popup_checks(), 0, "{case}: no closing decision");
    }
}

// A retry decision that saw neither page and is followed by no closing decision: after the
// intermediate page `wait` was missed (it flashed past between two frames), and in a run the
// runtime started itself. Each is `neither_page`, with the retry decision's limit.
#[test]
fn k1_a_retry_decision_that_saw_neither_page_without_a_closing_decision_is_neither_page() {
    let (result, runtime) = run(
        &with_intermediate_page(),
        vec![vec![plain(HOME)], vec![plain(DARK)]],
        LoadingMode::Off,
    );
    let error = failed("a missed intermediate page", result);
    assert_end(
        "a missed intermediate page",
        &error,
        (CONFIRMATION_FAILED, INTERMEDIATE_DETAIL, STEP_MS),
        "neither_page",
    );
    assert_eq!(other_values(&error), json!({}));
    assert_eq!(runtime.popup_checks(), 0, "no closing decision");

    let device = ScriptedRuntime::new(vec![vec![plain(HOME)], vec![plain(DARK)]]).host_started();
    let (result, runtime) = run_on(&with_retry(), device);
    let error = failed("a run the runtime started", result);
    assert_end(
        "a run the runtime started",
        &error,
        (CONFIRMATION_FAILED, DETAIL, STEP_MS),
        "neither_page",
    );
    assert_eq!(other_values(&error), json!({}));
    assert_eq!(runtime.popup_checks(), 0, "no closing decision");
}

// The still dark screen in a run the runtime started itself (a startup package, a ladder rung's
// package), a step without a retry: no closing decision, `unjudged_layer_run`.
#[test]
fn k1_a_run_the_runtime_started_itself_is_unjudged_layer_run() {
    let device = device_without_retry(vec![plain(DARK)]).host_started();
    let (result, runtime) = run_without_retry_on(device, TASK_MS);
    let error = failed("a run the runtime started", result);
    assert_end(
        "a run the runtime started",
        &error,
        (CONFIRMATION_FAILED, DETAIL, CONFIRMATION_MS),
        "unjudged_layer_run",
    );
    assert_eq!(spent(&error), Some((CONFIRMATION_MS, CONFIRMATION_MS)));
    assert_eq!(other_values(&error), json!({}));
    assert_eq!(runtime.popup_checks(), 0, "no closing decision");
}

// The step plan restarts the application (`start`, from any screen to `wait`, which is not the main
// interface), then taps `open` from `wait`, without a retry, and the still dark screen follows: the
// tap is inside the restart segment, so no closing decision, `unjudged_restart_segment`.
#[test]
fn k1_a_step_inside_a_restart_segment_is_unjudged_restart_segment() {
    let start = json!({"id": "start", "from": "any", "to": "wait",
                       "application": {"action": "restart"}});
    let task = admitted(
        control(TASK_MS, 2),
        program(
            "any",
            vec![start, open("wait", json!({}), json!({}), Retry::Undeclared)],
        ),
        Sampling::SingleFrame,
    );
    let device = ScriptedRuntime::new(vec![
        vec![plain(HOME)],
        vec![plain(WAIT)],
        vec![plain(DARK)],
    ])
    .with_application_effect();
    let (result, runtime) = run_on(&task, device);
    let error = failed("a restart segment", result);
    assert_end(
        "a restart segment",
        &error,
        (CONFIRMATION_FAILED, DETAIL, CONFIRMATION_MS),
        "unjudged_restart_segment",
    );
    assert_eq!(other_values(&error), json!({}));
    assert_eq!(
        runtime.applications(),
        [ApplicationLifecycleAction::Restart]
    );
    assert_eq!(runtime.inputs().len(), 1);
    assert_eq!(runtime.popup_checks(), 0, "no closing decision");
}

// The package's page targets declare a sample consensus (both of two frames 100 ms apart), a step
// without a retry, the dark screen after the tap: no closing decision, `unjudged_consensus`.
#[test]
fn k1_a_package_with_sample_consensus_is_unjudged_consensus() {
    let task = admitted(
        control(TASK_MS, 1),
        program(
            "home",
            vec![open("home", json!({}), json!({}), Retry::Undeclared)],
        ),
        Sampling::TwoFrames,
    );
    let (result, runtime) = run_on(&task, device_without_retry(vec![plain(DARK)]));
    let error = failed("sample consensus", result);
    assert_end(
        "sample consensus",
        &error,
        (CONFIRMATION_FAILED, DETAIL, CONFIRMATION_MS),
        "unjudged_consensus",
    );
    assert_eq!(other_values(&error), json!({}));
    assert_eq!(runtime.inputs().len(), 1);
    assert_eq!(runtime.popup_checks(), 0, "no closing decision");
}

// The runtime answers that the run has 1 999 ms left, just below twice the closing decision's bound
// of 1 000 ms: no closing decision, `unjudged_time_short`.
#[test]
fn k1_too_little_time_left_is_unjudged_time_short() {
    let device = device_without_retry(vec![plain(DARK)])
        .with_time_left(Duration::from_millis(2 * STEP_MS - 1));
    let (result, runtime) = run_without_retry_on(device, TASK_MS);
    let error = failed("1 999 ms left", result);
    assert_end(
        "1 999 ms left",
        &error,
        (CONFIRMATION_FAILED, DETAIL, CONFIRMATION_MS),
        "unjudged_time_short",
    );
    assert_eq!(spent(&error), Some((CONFIRMATION_MS, CONFIRMATION_MS)));
    assert_eq!(other_values(&error), json!({}));
    assert_eq!(runtime.popup_checks(), 0, "no closing decision");
}
