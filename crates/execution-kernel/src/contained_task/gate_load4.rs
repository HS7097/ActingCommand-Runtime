// SPDX-License-Identifier: AGPL-3.0-only

// LOAD-4 (Workflow #407, register §1.262; design `rd-1031/check-speed/model-loading.md` §5 and §7
// LOAD-4; coordinator brief `brief-load-4.md`): a step plan's waits under
// `execution.loading.mode`, and a loading set with no limit refused at admission.
//
// - Under `off` and `shadow` every decision is the one before; `shadow` records.
// - Under `on` the candidates and the loading pages are evaluated on one frame and a loading page
//   comes before every candidate: a frame on which one holds is a transition frame, whatever
//   candidate passed. It pauses the arrival budget, a skip target's settle and first sighting and
//   an extra round's end; it is never the intermediate page, never proof that the input took
//   effect and never the step's own page. An attempt's waits and its retry decision's share one
//   allowance, so a loading screen adds no input; the entry wait has the package's. A transition
//   frame after the allowance is spent ends the wait: `page_confirmation_failed` with
//   `loading_page_id`, `loading_ms` and `loading_limit_ms` (`intermediate_seen` and the rest as
//   before), at the entry `contained_task_linear_entry_unmatched` with them.
// - An operation whose loading set is not empty while no limit is named for it (no
//   `loading.max_ms`, no wait page, no loading copy in the package) is refused at admission,
//   `contained_task_program_invalid` with `loading_max_missing`, in a page graph and a step plan.
// - LOAD-4b (register §1.265; coordinator brief `brief-load-4b.md`): in a step plan a page
//   `loading.add` names, or the wait page, is no page the step's waits await. Beside its `from`,
//   destination and transition page (LOAD-2), with a run of optional operations (Workflow #339)
//   that is every optional page its input may reach and the run's skip target: under `on` such a
//   page would be a transition frame on every frame it shows, never a candidate. It is refused at
//   admission, `contained_task_linear_invalid` with `reason=loading_page` and the operation; a page
//   outside the step's waits is admitted.
//
// Every expected value comes from those lines and the package values below, never from the code.
// The runs play scripted frames on the scripted clock (EXEC-I4) at the factor 1000: the device
// shows `screens[n]` after `n` inputs, each segment's last frame repeating, a frame is delivered at
// once, and every pause moves the clock by its length. A frame is two pixels: the first names the
// page (`home`, `pop`, `lobby`, `result`; dark is no page), a white second is the loading page
// `spinner`. The package declares no capture interval, so a wait takes a frame every 50 ms without
// back-off; its loading pages' allowance is 30 000 ms unless a test says otherwise. A step taps
// and expects its next page within 1 000 ms. Captures are counted from the run's first frame,
// which its entry wait takes.
//
// The admission tests load assembled packages through `PreparedContainedTask::load`: a 2 x 1 frame
// whose left pixel names the page (`page_a` to `page_d`), with or without a loading copy (the
// page `transition`, `max_ms` 30 000).

use super::tests::scripted::{ScriptedEvent, ScriptedFrame, ScriptedRuntime};
use super::*;
use crate::codes::LoadingEventKind;
use actingcommand_page_detector::LoadingPageSet;
use actingcommand_recognition_pack::RecognitionPack;
use serde_json::{Value, json};
use std::io::{Cursor, Write};
use std::path::PathBuf;
use zip::{ZipWriter, write::FileOptions};

const CONFIRMATION_FAILED: &str = ExecutionKernelCode::PageConfirmationFailed.as_str();
const ENTRY_UNMATCHED: &str = ExecutionKernelCode::ContainedTaskLinearEntryUnmatched.as_str();
const INTERMEDIATE_UNOBSERVED: &str =
    ExecutionKernelCode::ContainedTaskLinearIntermediateUnobserved.as_str();
const LINEAR_INVALID: &str = ExecutionKernelCode::ContainedTaskLinearInvalid.as_str();
const PROGRAM_INVALID: &str = ExecutionKernelCode::ContainedTaskProgramInvalid.as_str();

const CONFIRMATION_MS: u64 = 1_000;
const PACKAGE_LOADING_MS: u64 = 30_000;
const STEP_MS: u64 = 60_000;
const TASK_MS: u64 = 600_000;
const TASK_FILE: &str = "resources/operations/task/task.json";

const HOME: [u8; 3] = [255, 0, 0];
const POP: [u8; 3] = [255, 255, 0];
const LOBBY: [u8; 3] = [0, 0, 255];
const RESULT: [u8; 3] = [0, 255, 0];
const DARK: [u8; 3] = [0, 0, 0];
const WHITE: [u8; 3] = [255, 255, 255];

type RunResult = Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>;

// `page` (dark: none), under the spinner when `spinner` says so.
fn frame(page: [u8; 3], spinner: bool) -> ScriptedFrame {
    ScriptedFrame::rgb(2, 1, &[page, if spinner { WHITE } else { DARK }])
}

fn plain(page: [u8; 3]) -> ScriptedFrame {
    frame(page, false)
}

fn spinning(page: [u8; 3]) -> ScriptedFrame {
    frame(page, true)
}

fn ms(values: &[u64]) -> Vec<Duration> {
    values.iter().copied().map(Duration::from_millis).collect()
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
            colour("page/pop", 0, POP),
            colour("page/lobby", 0, LOBBY),
            colour("page/result", 0, RESULT),
            colour("loading/spinner", 1, WHITE)
        ]
    }))
    .expect("recognition pack")
}

fn page_set() -> PageSet {
    serde_json::from_value(json!({"schema_version": "0.3", "pages": [
        {"id": "home", "required": ["page/home"]},
        {"id": "pop", "required": ["page/pop"]},
        {"id": "lobby", "required": ["page/lobby"]},
        {"id": "result", "required": ["page/result"]}
    ]}))
    .expect("page set")
}

// The package's loading page `spinner` and its allowance `max_ms`.
fn loading_pages(max_ms: u64) -> LoadingPageSet {
    serde_json::from_value(json!({"max_ms": max_ms, "pages": [
        {"id": "spinner", "required": ["loading/spinner"]}
    ]}))
    .expect("loading pages")
}

// A trusted tap `id` from `from` expecting `to`, its `expect_after` with `expect_after` members and
// the operation with `extra` members.
fn tap(id: &str, from: &str, to: &str, expect_after: Value, extra: Value) -> Value {
    let mut expectation = json!({"page_id": to, "timeout_ms": CONFIRMATION_MS});
    for (member, value) in expect_after.as_object().expect("expect_after members") {
        expectation[member] = value.clone();
    }
    let mut operation = json!({"id": id, "from": from,
                               "click": {"kind": "point", "x": 1, "y": 0},
                               "unguarded_trusted_coordinate": true,
                               "expect_after": expectation});
    for (member, value) in extra.as_object().expect("operation members") {
        operation[member] = value.clone();
    }
    operation
}

// `open`: the tap from `home` to `result`.
fn open(expect_after: Value, extra: Value) -> Value {
    tap("open", "home", "result", expect_after, extra)
}

// The step plan of `operations` from `home` to `result`, its package's loading allowance
// `loading_ms`, admitted as the package's admission admits it.
fn plan_of(operations: Vec<Value>, loading_ms: u64) -> PreparedContainedTask {
    let steps = operations.len();
    let control: TaskControl = serde_json::from_value(json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.loading.plan",
        "execution_mode": "linear_steps",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "step_timeout_ms": STEP_MS,
        "timeout_ms": TASK_MS,
        "max_steps": steps
    }))
    .expect("task control");
    control.validate().expect("an admitted control");
    let program: TaskProgram = serde_json::from_value(json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "entry_page": "home",
        "target_page": "result",
        "max_steps": steps,
        "operations": operations
    }))
    .expect("task program");
    let evaluator =
        RecognitionEvaluator::new(PathBuf::new(), pack()).expect("recognition evaluator");
    let detector = PageDetector::new(page_set())
        .expect("page detector")
        .with_loading_pages(loading_pages(loading_ms))
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

fn plan(operations: Vec<Value>) -> PreparedContainedTask {
    plan_of(operations, PACKAGE_LOADING_MS)
}

// `open` from `home` to `pop`, then `dismiss`, optional with a settle of 200 ms, from `pop` to
// `result`: the candidates after `open` are `pop`, then the skip target `result`.
fn optional_plan() -> PreparedContainedTask {
    plan(vec![
        tap("open", "home", "pop", json!({}), json!({})),
        tap(
            "dismiss",
            "pop",
            "result",
            json!({}),
            json!({"optional": {"settle_ms": 200}}),
        ),
    ])
}

fn device(screens: Vec<Vec<ScriptedFrame>>, mode: LoadingMode) -> ScriptedRuntime {
    ScriptedRuntime::new(screens).with_loading_mode(mode)
}

fn run(
    task: &PreparedContainedTask,
    screens: Vec<Vec<ScriptedFrame>>,
    mode: LoadingMode,
) -> (RunResult, ScriptedRuntime) {
    let mut runtime = device(screens, mode);
    let result = task.run(&mut runtime);
    (result, runtime)
}

fn failed(case: &str, result: RunResult) -> ContainedTaskError {
    match result {
        Err(ContainedTaskRunError::Task(error)) => error,
        other => panic!("{case}: expected a task failure, got {other:?}"),
    }
}

fn succeeded(case: &str, result: &RunResult) {
    assert!(
        matches!(result, Ok(outcome) if matches!(outcome.outcome, TaskOutcome::Success)),
        "{case}: expected the run to succeed: {result:?}"
    );
}

// The pauses of the run after its `n`-th input and before the next one.
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

fn value(error: &ContainedTaskError, key: &str) -> Value {
    error.outcome().value(key).cloned().unwrap_or(Value::Null)
}

// The loading values of a failure: `loading_page_id`, `loading_ms`, `loading_limit_ms`.
fn loading_values(error: &ContainedTaskError) -> (Value, Value, Value) {
    (
        value(error, "loading_page_id"),
        value(error, "loading_ms"),
        value(error, "loading_limit_ms"),
    )
}

fn no_loading_values() -> (Value, Value, Value) {
    (Value::Null, Value::Null, Value::Null)
}

// One transition: the capture it came on, its kind, its loading page, the milliseconds spent, the
// page it names and the exit gap in milliseconds.
type Transition = (
    usize,
    LoadingEventKind,
    String,
    u64,
    Option<String>,
    Option<u64>,
);

fn transitions(runtime: &ScriptedRuntime) -> Vec<Transition> {
    runtime
        .loading_events()
        .iter()
        .map(|(capture, event)| {
            (
                *capture,
                event.kind,
                event.loading_page_id.clone(),
                u64::try_from(event.spent.as_millis()).expect("milliseconds"),
                event.page_id.clone(),
                event
                    .exit_gap
                    .map(|gap| u64::try_from(gap.as_millis()).expect("milliseconds")),
            )
        })
        .collect()
}

// An expected transition.
fn seen(
    capture: usize,
    kind: LoadingEventKind,
    spent_ms: u64,
    page: Option<&str>,
    exit_gap_ms: Option<u64>,
) -> Transition {
    (
        capture,
        kind,
        "spinner".to_owned(),
        spent_ms,
        page.map(str::to_owned),
        exit_gap_ms,
    )
}

// On: forty spinner frames over no page (1 950 ms of transition frames, past the arrival budget of
// 1 000 ms) pause it, and `result` 50 ms later ends the step on the 41st frame; `left` names
// `result`, the 1 950 ms spent and the 50 ms since the last spinner frame. Off: the frames are frames
// of no candidate and the arrival runs out at 1 000 ms, twenty pauses, without loading values.
// Shadow: the same end and pauses as off; the entry frame and every frame after the input are
// evaluated, and the stretch is recorded as entered.
//
// Workflow #463 U1, fix round 1 (contracts/linear-steps.md, "Retries"): `open` declares no retry,
// so under off and shadow the arrival's end is followed by one closing decision on `result` and
// `home`, made at once and sharing the attempt's allowance. Its frames 22 to 40 after the input
// are spinner frames of no page (nineteen more pauses of 50 ms), and `result` passes on the 41st:
// the run ends as the arrival ended it, with no further input. Under shadow those frames are
// evaluated too, and `result` leaves the stretch: 1 900 ms spent (the arrival's twenty pauses and
// the decision's eighteen before its last spinner frame), 50 ms after the last spinner frame.
#[test]
fn linear_loading_pauses_the_arrival_budget() {
    let task = plan(vec![open(json!({}), json!({}))]);
    let screens = || {
        let mut after_tap = vec![spinning(DARK); 40];
        after_tap.push(plain(RESULT));
        vec![vec![plain(HOME)], after_tap]
    };
    let (result, runtime) = run(&task, screens(), LoadingMode::On);
    succeeded("on", &result);
    assert_eq!(runtime.captures_after(1), 41);
    assert_eq!(pauses_after_input(&runtime, 1), ms(&[50; 40]));
    assert_eq!(
        transitions(&runtime),
        [
            seen(2, LoadingEventKind::Entered, 0, None, None),
            seen(42, LoadingEventKind::Left, 1_950, Some("result"), Some(50)),
        ]
    );
    assert!(
        runtime
            .loading_events()
            .iter()
            .all(
                |(_, event)| event.pack_operation_id.as_deref() == Some("open")
                    && event.limit == Duration::from_millis(PACKAGE_LOADING_MS)
            )
    );

    for mode in [LoadingMode::Off, LoadingMode::Shadow] {
        let (result, runtime) = run(&task, screens(), mode);
        let error = failed("the arrival runs out", result);
        assert_eq!(error.code().as_str(), CONFIRMATION_FAILED, "{mode:?}");
        let timing = error.timing().expect("the wait's timing");
        assert_eq!(
            (timing.elapsed_ms, timing.limit_ms),
            (1_000, 1_000),
            "{mode:?}"
        );
        assert_eq!(loading_values(&error), no_loading_values(), "{mode:?}");
        assert_eq!(runtime.inputs().len(), 1, "{mode:?}");
        // The arrival's twenty pauses, then the closing decision's nineteen.
        assert_eq!(pauses_after_input(&runtime, 1), ms(&[50; 39]), "{mode:?}");
        assert_eq!(runtime.captures_after(1), 41, "{mode:?}");
        if mode == LoadingMode::Off {
            assert!(runtime.loading_evaluations().is_empty());
            assert!(runtime.loading_events().is_empty());
        } else {
            assert_eq!(
                runtime.loading_evaluations(),
                (1..=42).collect::<Vec<_>>().as_slice()
            );
            assert_eq!(
                transitions(&runtime),
                [
                    seen(2, LoadingEventKind::Entered, 0, None, None),
                    seen(42, LoadingEventKind::Left, 1_900, Some("result"), Some(50)),
                ]
            );
        }
    }
}

// On: after `open` the candidates are `pop`, then the skip target `result` with a settle of 200 ms.
// `result` is seen on the first frame; eight spinner frames over it (400 ms) pause the settle and
// the first sighting, so `pop` 50 ms after them is still in time and `dismiss` runs: two inputs.
// Off and shadow: the spinner frames are `result`, which settles 200 ms after its first sighting
// and skips `dismiss`: one input.
#[test]
fn linear_loading_pauses_a_skip_targets_settle() {
    let task = optional_plan();
    let screens = || {
        let mut after_tap = vec![plain(RESULT)];
        after_tap.resize(9, spinning(RESULT));
        after_tap.push(plain(POP));
        vec![vec![plain(HOME)], after_tap, vec![plain(RESULT)]]
    };
    let (result, runtime) = run(&task, screens(), LoadingMode::On);
    succeeded("on", &result);
    assert_eq!(runtime.inputs().len(), 2);
    assert_eq!(runtime.captures_after(1), 10);
    assert_eq!(
        transitions(&runtime),
        [
            seen(3, LoadingEventKind::Entered, 0, Some("result"), None),
            seen(11, LoadingEventKind::Left, 400, Some("pop"), Some(50)),
        ]
    );

    for mode in [LoadingMode::Off, LoadingMode::Shadow] {
        let (result, runtime) = run(&task, screens(), mode);
        succeeded("the skip target settles", &result);
        assert_eq!(runtime.inputs().len(), 1, "{mode:?}");
        assert_eq!(runtime.captures_after(1), 5, "{mode:?}");
    }
}

// On: `open` declares the intermediate page `lobby` (500 ms). Four spinner frames over `lobby` are
// no sight of it; `result` follows, which the intermediate wait does not await, and the wait runs
// out: `contained_task_linear_intermediate_unobserved`, the step never trusted. Off and shadow: the
// first spinner frame is `lobby`, and the step goes on `result` on the fifth frame.
#[test]
fn linear_loading_is_not_the_intermediate_page() {
    let task = plan(vec![open(
        json!({}),
        json!({"transition": {"kind": "page", "page_id": "lobby", "timeout_ms": 500}}),
    )]);
    let screens = || {
        let mut after_tap = vec![spinning(LOBBY); 4];
        after_tap.push(plain(RESULT));
        vec![vec![plain(HOME)], after_tap]
    };
    let (result, runtime) = run(&task, screens(), LoadingMode::On);
    let error = failed("on", result);
    assert_eq!(error.code().as_str(), INTERMEDIATE_UNOBSERVED);
    assert_eq!(runtime.inputs().len(), 1);
    assert_eq!(
        transitions(&runtime),
        [
            seen(2, LoadingEventKind::Entered, 0, Some("lobby"), None),
            seen(6, LoadingEventKind::Left, 150, None, Some(50)),
        ]
    );

    for mode in [LoadingMode::Off, LoadingMode::Shadow] {
        let (result, runtime) = run(&task, screens(), mode);
        succeeded("the intermediate page seen", &result);
        assert_eq!(runtime.captures_after(1), 5, "{mode:?}");
    }
}

// On: three spinner frames over `result` are no candidate; the plain `result` 100 ms of transition
// frames later ends the step on the fourth frame. The same over the optional page `pop`, the first
// of the candidates after `open` of the optional plan: `dismiss` runs on the fourth frame. Shadow
// and off: the step goes on the first frame, as before.
#[test]
fn linear_loading_over_a_candidate_suppresses_it() {
    let task = plan(vec![open(json!({}), json!({}))]);
    let screens = || {
        let mut after_tap = vec![spinning(RESULT); 3];
        after_tap.push(plain(RESULT));
        vec![vec![plain(HOME)], after_tap]
    };
    let (result, runtime) = run(&task, screens(), LoadingMode::On);
    succeeded("on", &result);
    assert_eq!(runtime.captures_after(1), 4);
    assert_eq!(
        transitions(&runtime),
        [
            seen(2, LoadingEventKind::Entered, 0, Some("result"), None),
            seen(5, LoadingEventKind::Left, 100, Some("result"), Some(50)),
        ]
    );
    let (result, runtime) = run(&task, screens(), LoadingMode::Shadow);
    succeeded("shadow", &result);
    assert_eq!(runtime.captures_after(1), 1);
    assert_eq!(
        transitions(&runtime),
        [seen(2, LoadingEventKind::Entered, 0, Some("result"), None)]
    );
    let (result, runtime) = run(&task, screens(), LoadingMode::Off);
    succeeded("off", &result);
    assert_eq!(runtime.captures_after(1), 1);
    assert!(runtime.loading_events().is_empty());

    let optional = optional_plan();
    let screens = || {
        let mut after_tap = vec![spinning(POP); 3];
        after_tap.push(plain(POP));
        vec![vec![plain(HOME)], after_tap, vec![plain(RESULT)]]
    };
    for (mode, captures) in [
        (LoadingMode::On, 4),
        (LoadingMode::Shadow, 1),
        (LoadingMode::Off, 1),
    ] {
        let (result, runtime) = run(&optional, screens(), mode);
        succeeded("the optional page", &result);
        assert_eq!(runtime.inputs().len(), 2, "{mode:?}");
        assert_eq!(runtime.captures_after(1), captures, "{mode:?}");
    }
}

// A retry policy of two attempts (100 ms apart) and an allowance of 200 ms, the spinner over `home`
// after the tap. On: the arrival spends the allowance in five pauses and ends; the retry decision,
// 100 ms later, shares it, and its first frame (`home` under the spinner, never the step's own page)
// ends it: `page_confirmation_failed` with 200 ms of 200, one input, one `limit_reached`. Off: the
// frames are `home`, the retry decision takes the step's own page and the step taps again.
#[test]
fn linear_retry_decision_on_loading_adds_no_input() {
    let task = plan(vec![open(
        json!({"loading": {"max_ms": 200}}),
        json!({"retryable": true, "max_attempts": 2, "retry_interval_ms": 100}),
    )]);
    let screens = || vec![vec![plain(HOME)], vec![spinning(HOME)]];
    let (result, runtime) = run(&task, screens(), LoadingMode::On);
    let error = failed("on", result);
    assert_eq!(error.code().as_str(), CONFIRMATION_FAILED);
    assert_eq!(
        loading_values(&error),
        (json!("spinner"), json!(200), json!(200))
    );
    assert_eq!(runtime.inputs().len(), 1);
    assert_eq!(
        pauses_after_input(&runtime, 1),
        ms(&[50, 50, 50, 50, 50, 100])
    );
    assert_eq!(
        transitions(&runtime)
            .iter()
            .filter(|transition| transition.1 == LoadingEventKind::LimitReached)
            .count(),
        1
    );

    let (result, runtime) = run(&task, screens(), LoadingMode::Off);
    let error = failed("off", result);
    assert_eq!(error.code().as_str(), CONFIRMATION_FAILED);
    assert_eq!(loading_values(&error), no_loading_values());
    assert_eq!(runtime.inputs().len(), 2);
}

// On, an allowance of 200 ms (`loading.max_ms`), spinner frames over no page after the tap; a
// transition frame after the allowance is spent ends the wait on it with no further time and no
// extra round:
// - the arrival: `page_confirmation_failed`, 200 ms of 200 after five pauses, `limit_reached` on the
//   sixth frame after the input;
// - after the intermediate page `lobby` was seen: the same, its detail with `intermediate_seen=true`;
// - the entry wait, the package's allowance of 300 ms: `contained_task_linear_entry_unmatched` with
//   300 ms of 300 after seven pauses, before any input.
#[test]
fn linear_loading_limit_ends_with_page_confirmation_failed_and_its_values() {
    let limited = plan(vec![open(json!({"loading": {"max_ms": 200}}), json!({}))]);
    let mut runtime = device(
        vec![vec![plain(HOME)], vec![spinning(DARK)]],
        LoadingMode::On,
    )
    .with_step_extras(1, Duration::ZERO);
    let error = failed("the arrival", limited.run(&mut runtime));
    assert_eq!(error.code().as_str(), CONFIRMATION_FAILED);
    assert_eq!(
        loading_values(&error),
        (json!("spinner"), json!(200), json!(200))
    );
    assert_eq!(pauses_after_input(&runtime, 1), ms(&[50; 5]));
    assert_eq!(runtime.extra_rounds_started(), 0);
    assert_eq!(
        transitions(&runtime),
        [
            seen(2, LoadingEventKind::Entered, 0, None, None),
            seen(7, LoadingEventKind::LimitReached, 200, None, None),
        ]
    );

    let through_lobby = plan(vec![open(
        json!({"loading": {"max_ms": 200}}),
        json!({"transition": {"kind": "page", "page_id": "lobby", "timeout_ms": 500}}),
    )]);
    let (result, runtime) = run(
        &through_lobby,
        vec![vec![plain(HOME)], vec![plain(LOBBY), spinning(DARK)]],
        LoadingMode::On,
    );
    let error = failed("after the intermediate page", result);
    assert_eq!(error.code().as_str(), CONFIRMATION_FAILED);
    assert_eq!(
        loading_values(&error),
        (json!("spinner"), json!(200), json!(200))
    );
    assert!(
        error.detail().is_some_and(|detail| detail
            .split(' ')
            .any(|token| token == "intermediate_seen=true")),
        "{error}"
    );
    assert_eq!(pauses_after_input(&runtime, 1), ms(&[50; 5]));

    let entry = plan_of(vec![open(json!({}), json!({}))], 300);
    let (result, runtime) = run(&entry, vec![vec![spinning(DARK)]], LoadingMode::On);
    let error = failed("the entry", result);
    assert_eq!(error.code().as_str(), ENTRY_UNMATCHED);
    assert_eq!(
        loading_values(&error),
        (json!("spinner"), json!(300), json!(300))
    );
    assert!(runtime.inputs().is_empty());
    assert_eq!(pauses_after_input(&runtime, 0), ms(&[50; 7]));
    assert_eq!(
        transitions(&runtime),
        [
            seen(1, LoadingEventKind::Entered, 0, None, None),
            seen(8, LoadingEventKind::LimitReached, 300, None, None),
        ]
    );
    assert!(
        runtime
            .loading_events()
            .iter()
            .all(|(_, event)| event.pack_operation_id.is_none())
    );
}

// The assembled packages of the admission tests.

const ADMITTED_PAGES: [(&str, [u8; 3]); 4] = [
    ("page_a", [255, 0, 0]),
    ("page_b", [0, 255, 0]),
    ("page_c", [0, 0, 255]),
    ("page_d", [255, 255, 0]),
];

// A trusted tap `id` from `from` confirmed on `to`, its `expect_after` with `extra` members.
fn admitted_tap(id: &str, from: &str, to: &str, extra: Value) -> Value {
    let mut expect_after = json!({"page_id": to, "timeout_ms": 2_000, "interval_ms": 500});
    for (member, value) in extra.as_object().expect("expect_after members") {
        expect_after[member] = value.clone();
    }
    json!({
        "id": id, "from": from,
        "click": {"kind": "point", "x": 1, "y": 0},
        "unguarded_trusted_coordinate": true, "retryable": false,
        "expect_after": expect_after
    })
}

fn admitted_task(operations: Vec<Value>, target: &str) -> Value {
    json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "entry_page": "page_a",
        "target_page": target,
        "operations": operations
    })
}

// The page-graph task of the one tap `op_tap` from `page_a` to `page_b`, its `expect_after` with
// `extra`, and the error page `page_c`.
fn page_graph(extra: Value) -> Value {
    let mut task = admitted_task(
        vec![admitted_tap("op_tap", "page_a", "page_b", extra)],
        "page_b",
    );
    task["error_pages"] = json!(["page_c"]);
    task
}

// The step plan `page_a` -> `page_b` -> `page_c`, its two taps' `expect_after` with `first` and
// `second`.
fn step_plan(first: Value, second: Value) -> Value {
    let mut plan = admitted_task(
        vec![
            admitted_tap("op_1", "page_a", "page_b", first),
            admitted_tap("op_2", "page_b", "page_c", second),
        ],
        "page_c",
    );
    plan["max_steps"] = json!(2);
    plan
}

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("JSON bytes")
}

fn zip_entries(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, content) in entries {
        zip.start_file(*path, options).expect("zip entry");
        zip.write_all(content).expect("zip bytes");
    }
    zip.finish().expect("finish zip").into_inner()
}

// The assembled package of `task` under the execution mode `mode`; with `copy`, its page set
// carries the loading page `transition` of a loading copy (`max_ms` 30 000), else none.
fn package(task: &Value, mode: &str, copy: bool) -> Vec<u8> {
    let mut control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.loading.task",
        "execution_mode": mode,
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 1_000,
        "step_timeout_ms": 3_000,
        "timeout_ms": 30_000
    });
    if let Some(max_steps) = task.get("max_steps") {
        control["max_steps"] = max_steps.clone();
    }
    let mut targets = ADMITTED_PAGES
        .iter()
        .map(|(page, colour)| {
            json!({"type": "color", "id": format!("page/{page}"),
                   "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": colour})
        })
        .collect::<Vec<_>>();
    let pages = ADMITTED_PAGES
        .iter()
        .map(|(page, _)| {
            json!({"id": format!("neutral/{page}"), "required": [format!("page/{page}")],
                   "optional": [], "forbidden": []})
        })
        .collect::<Vec<_>>();
    let mut page_set = json!({"schema_version": "0.3", "pages": pages});
    if copy {
        targets.push(json!({"type": "color", "id": "loading/spinner",
                            "region": {"x": 1, "y": 0, "width": 1, "height": 1},
                            "expected": WHITE}));
        page_set["loading"] = json!({"max_ms": 30_000, "pages": [
            {"id": "neutral/transition", "required": ["loading/spinner"],
             "optional": [], "forbidden": []}
        ]});
    }
    let pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": targets
    });
    zip_entries(&[
        ("control.json", bytes(&control)),
        (
            "resources/manifest.json",
            bytes(&json!({"schema_version": "0.3", "entry_task_id": "task"})),
        ),
        (TASK_FILE, bytes(task)),
        ("resources/recognition/neutral.test.pack.json", bytes(&pack)),
        (
            "resources/recognition/neutral.test.pages.json",
            bytes(&page_set),
        ),
    ])
}

fn admit(
    task: &Value,
    mode: &str,
    copy: bool,
) -> Result<PreparedContainedTask, ContainedTaskError> {
    PreparedContainedTask::load("neutral.instance", &package(task, mode, copy))
}

fn admitted(case: &str, task: &Value, mode: &str, copy: bool) {
    if let Err(error) = admit(task, mode, copy) {
        panic!("{case}: admission refused: {error}");
    }
}

// `task` is refused `contained_task_program_invalid` with `loading_max_missing` at `pointer` in its
// task file.
fn refused_without_limit(case: &str, task: &Value, mode: &str, pointer: &str) {
    let error = match admit(task, mode, false) {
        Ok(_) => panic!("{case}: admitted"),
        Err(error) => error,
    };
    let program_value = |key: &str| error.outcome().value(key).cloned().unwrap_or(Value::Null);
    assert_eq!(error.code().as_str(), PROGRAM_INVALID, "{case}: {error}");
    assert_eq!(
        program_value("program_problem"),
        json!("loading_max_missing"),
        "{case}"
    );
    assert_eq!(program_value("json_pointer"), json!(pointer), "{case}");
    assert_eq!(program_value("path"), json!(TASK_FILE), "{case}");
}

fn wait_page(page: &str) -> Value {
    json!({"page_id": page, "max_ms": 10_000})
}

// A page graph whose package carries no loading copy: `loading.add` alone names no limit and is
// refused at the operation's `expect_after.loading`. A `max_ms` of its own, a wait page beside it,
// a wait page alone, `loading: "off"`, and the same `add` in a package whose copy carries `max_ms`
// are admitted.
#[test]
fn a_loading_set_without_any_limit_is_refused_at_admission() {
    let mode = "navigable_route";
    refused_without_limit(
        "pages added without a limit",
        &page_graph(json!({"loading": {"add": ["page_d"]}})),
        mode,
        "/operations/0/expect_after/loading",
    );
    for (case, extra, copy) in [
        (
            "a max_ms of its own",
            json!({"loading": {"add": ["page_d"], "max_ms": 60_000}}),
            false,
        ),
        (
            "pages added beside a wait page",
            json!({"loading": {"add": ["page_d"]}, "wait_page": wait_page("page_d")}),
            false,
        ),
        (
            "a wait page alone",
            json!({"wait_page": wait_page("page_d")}),
            false,
        ),
        ("loading off", json!({"loading": "off"}), false),
        (
            "the copy's max_ms",
            json!({"loading": {"add": ["page_d"]}}),
            true,
        ),
    ] {
        admitted(case, &page_graph(extra), mode, copy);
    }
}

// A step plan whose package carries no loading copy: `loading.add` alone on either step names no
// limit and is refused at that step's `expect_after.loading`. A `max_ms` of its own, a wait page,
// and the same `add` in a package whose copy carries `max_ms` are admitted.
#[test]
fn a_linear_loading_set_without_any_limit_is_refused_at_admission() {
    let mode = "linear_steps";
    refused_without_limit(
        "the first step",
        &step_plan(json!({"loading": {"add": ["page_d"]}}), json!({})),
        mode,
        "/operations/0/expect_after/loading",
    );
    refused_without_limit(
        "the second step",
        &step_plan(json!({}), json!({"loading": {"add": ["page_d"]}})),
        mode,
        "/operations/1/expect_after/loading",
    );
    admitted(
        "a max_ms of its own and a wait page",
        &step_plan(
            json!({"loading": {"add": ["page_d"], "max_ms": 60_000}}),
            json!({"wait_page": wait_page("page_d")}),
        ),
        mode,
        false,
    );
    admitted(
        "the copy's max_ms",
        &step_plan(json!({"loading": {"add": ["page_d"]}}), json!({})),
        mode,
        true,
    );
}

// LOAD-4b: `operation` marked optional, with a settle of 500 ms.
fn optional(mut operation: Value) -> Value {
    operation["optional"] = json!({"settle_ms": 500});
    operation
}

// The step plan `page_a` -> `page_b` -> `page_c` with `op_2` optional, its two taps' `expect_after`
// with `first` and `second`: after `op_1` the candidates are the optional page `page_b` and the
// skip target `page_c`.
fn optional_step_plan(first: Value, second: Value) -> Value {
    let mut plan = step_plan(first, second);
    plan["operations"][1] = optional(plan["operations"][1].clone());
    plan
}

// The step plan `page_a` -> `page_b` -> `page_c` -> `page_d`: `op_1`, then the run of the optional
// `op_2` and `op_3`, whose skip target is `page_d`; the taps' `expect_after` with `first`, `second`
// and `third`. The candidates after `op_1` are `page_b`, `page_c` and `page_d`; after `op_2`,
// `page_c` and `page_d`; after `op_3`, reached with `op_2` skipped, `page_b` and `page_d`.
fn run_plan(first: Value, second: Value, third: Value) -> Value {
    let mut plan = admitted_task(
        vec![
            admitted_tap("op_1", "page_a", "page_b", first),
            optional(admitted_tap("op_2", "page_b", "page_c", second)),
            optional(admitted_tap("op_3", "page_c", "page_d", third)),
        ],
        "page_d",
    );
    plan["max_steps"] = json!(3);
    plan
}

// `task`, a step plan in a package with a loading copy, is refused `contained_task_linear_invalid`
// with `reason=loading_page` naming `operation`.
fn refused_as_loading_page(case: &str, task: &Value, operation: &str) {
    let error = match admit(task, "linear_steps", true) {
        Ok(_) => panic!("{case}: admitted"),
        Err(error) => error,
    };
    assert_eq!(error.code().as_str(), LINEAR_INVALID, "{case}: {error}");
    let operation = format!("operation={operation}");
    let tokens = error
        .detail()
        .map(|detail| detail.split(' ').collect::<Vec<_>>())
        .unwrap_or_default();
    assert!(
        tokens.contains(&"reason=loading_page") && tokens.contains(&operation.as_str()),
        "{case}: {error}"
    );
}

// An optional page a step's input may reach, other than its destination, added or as the wait page:
// `page_c` and `page_d` after `op_1` of the run plan, and `page_b`, `op_2`'s page, after `op_3`.
#[test]
fn a_linear_loading_page_that_is_an_optional_candidate_is_refused() {
    refused_as_loading_page(
        "the run's second optional page added on the step before the run",
        &run_plan(
            json!({"loading": {"add": ["page_c"]}}),
            json!({}),
            json!({}),
        ),
        "op_1",
    );
    refused_as_loading_page(
        "the run's second optional page as the wait page of the step before the run",
        &run_plan(
            json!({"wait_page": wait_page("page_c")}),
            json!({}),
            json!({}),
        ),
        "op_1",
    );
    refused_as_loading_page(
        "the skipped member's page added on the run's last member",
        &run_plan(
            json!({}),
            json!({}),
            json!({"loading": {"add": ["page_b"]}}),
        ),
        "op_3",
    );
}

// A run's skip target on a step whose input may reach it and whose destination it is not: `page_c`
// on `op_1` of the optional step plan, added or as the wait page, and `page_d` on `op_2` of the run
// plan.
#[test]
fn a_linear_loading_page_that_is_the_skip_target_is_refused() {
    refused_as_loading_page(
        "the skip target added on the step before the run",
        &optional_step_plan(json!({"loading": {"add": ["page_c"]}}), json!({})),
        "op_1",
    );
    refused_as_loading_page(
        "the skip target as the wait page of the step before the run",
        &optional_step_plan(json!({"wait_page": wait_page("page_c")}), json!({})),
        "op_1",
    );
    refused_as_loading_page(
        "the skip target added on the run's first member",
        &run_plan(
            json!({}),
            json!({"loading": {"add": ["page_d"]}}),
            json!({}),
        ),
        "op_2",
    );
}

// A page no wait of its step awaits is admitted, though another step's waits await it: `page_d` on
// `op_1` and `page_a` on `op_2` of the optional step plan; `page_a` on `op_2` and `op_3` of the run
// plan; and without a run, `op_2`'s destination `page_c` on `op_1` of the step plan.
#[test]
fn a_linear_loading_page_outside_the_waits_is_still_admitted() {
    let mode = "linear_steps";
    admitted(
        "pages outside the waits of a step plan with one optional step",
        &optional_step_plan(
            json!({"loading": {"add": ["page_d"]}}),
            json!({"wait_page": wait_page("page_a")}),
        ),
        mode,
        true,
    );
    admitted(
        "pages outside the waits of the run's members",
        &run_plan(
            json!({}),
            json!({"loading": {"add": ["page_a"]}}),
            json!({"wait_page": wait_page("page_a")}),
        ),
        mode,
        true,
    );
    admitted(
        "the next step's destination without a run",
        &step_plan(json!({"loading": {"add": ["page_c"]}}), json!({})),
        mode,
        true,
    );
}
