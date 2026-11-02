// SPDX-License-Identifier: AGPL-3.0-only

// LOAD-3 (Workflow #407, register §1.260; design `rd-1031/check-speed/model-loading.md` §3, §4, §6
// and §7; coordinator brief `brief-load-3.md`): a page-graph run's waits under
// `execution.loading.mode`.
//
// - `off` and `shadow` keep every decision; `shadow` records each frame's loading pages and their
//   transitions.
// - Under `on`, one judgement decides each frame of a wait: a recognition conflict, then an error
//   page, then a transition frame (a loading page held, several being no ambiguity), then what the
//   wait waits for. A transition frame is offered to the shared interrupt pages first; otherwise it
//   spends the input's one allowance and pauses the wait, and is never the step's final state, a
//   decision frame, ACT-4 flip evidence or still. A transition frame after the allowance is spent
//   ends the wait through its own path and code with `loading_page_id`, `loading_ms` and
//   `loading_limit_ms`. The task deadline holds; the allowance is shared by the settled path, the
//   confirmation and the failure decision's confirmation, and a page wait has the package's own.
//
// Every expected value comes from those lines and the package values below, never from the code.
// The runs play scripted frames on the scripted clock (EXEC-I4) at the factor 1000: the device
// shows `screens[n]` after `n` inputs, each segment's last frame repeating, a frame is delivered at
// once, and every pause moves the clock by its length. A frame is four pixels: the first names the
// page (`home`, `lobby`, `result`, the error page `fail`; dark is no page), a grey second is the
// target `mark/popup` of `home`, a white third the loading page `spinner` and a yellow fourth the
// loading page `curtain`. The package declares no capture interval, so a wait takes a frame every
// 50 ms, without back-off unless a test says so; its loading pages' allowance is 30 000 ms unless a
// test says otherwise. The operation `open` taps from `home` to `result` with a confirmation of
// 1 000 ms. Captures are counted from the run's first frame, which shows `home`.

use super::loading::LoadingAllowance;
use super::tests::scripted::{ScriptedEvent, ScriptedFrame, ScriptedRuntime};
use super::*;
use crate::codes::LoadingEventKind;
use actingcommand_page_detector::LoadingPageSet;
use actingcommand_recognition_pack::RecognitionPack;
use serde_json::{Value, json};
use std::path::PathBuf;

const CONFIRMATION_FAILED: &str = ExecutionKernelCode::PageConfirmationFailed.as_str();
const STEP_NOT_CONFIRMED: &str = ExecutionKernelCode::ContainedTaskStepUnconfirmed.as_str();
const ERROR_PAGE: &str = ExecutionKernelCode::ContainedTaskErrorPageReached.as_str();
const CONFLICT: &str = ExecutionKernelCode::ContainedTaskRecognitionConflict.as_str();
const TASK_TIMEOUT: &str = ExecutionKernelCode::ContainedTaskTimeout.as_str();
const PAGE_UNKNOWN: &str = SharedCode::ContainedTaskPageUnknown.as_str();

const CONFIRMATION_MS: u64 = 1_000;
const PACKAGE_LOADING_MS: u64 = 30_000;
const TASK_MS: u64 = 600_000;
const PAGE_WAIT_MS: u64 = 60_000;

const HOME: [u8; 3] = [255, 0, 0];
const LOBBY: [u8; 3] = [0, 0, 255];
const RESULT: [u8; 3] = [0, 255, 0];
const FAIL: [u8; 3] = [255, 0, 255];
const DARK: [u8; 3] = [0, 0, 0];
const GREY: [u8; 3] = [128, 128, 128];
const WHITE: [u8; 3] = [255, 255, 255];
const YELLOW: [u8; 3] = [255, 255, 0];

type RunResult = Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>;

// A frame of `page` (dark: none), with the popup, the spinner and the curtain as given.
fn frame(page: [u8; 3], popup: bool, spinner: bool, curtain: bool) -> ScriptedFrame {
    let on = |shown: bool, colour: [u8; 3]| if shown { colour } else { DARK };
    ScriptedFrame::rgb(
        4,
        1,
        &[
            page,
            on(popup, GREY),
            on(spinner, WHITE),
            on(curtain, YELLOW),
        ],
    )
}

// `page` without any mark.
fn plain(page: [u8; 3]) -> ScriptedFrame {
    frame(page, false, false, false)
}

// `page` under the spinner.
fn spinning(page: [u8; 3]) -> ScriptedFrame {
    frame(page, false, true, false)
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
        "coordinate_space": {"width": 4, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            colour("page/home", 0, HOME),
            colour("page/lobby", 0, LOBBY),
            colour("page/result", 0, RESULT),
            colour("page/fail", 0, FAIL),
            colour("page/twin", 0, RESULT),
            colour("mark/popup", 1, GREY),
            colour("loading/spinner", 2, WHITE),
            colour("loading/curtain", 3, YELLOW)
        ]
    }))
    .expect("recognition pack")
}

// The pages; with `twin` also a page that holds wherever `result` does.
fn page_set(twin: bool) -> PageSet {
    let mut pages = vec![
        json!({"id": "home", "required": ["page/home"], "optional": ["mark/popup"]}),
        json!({"id": "lobby", "required": ["page/lobby"]}),
        json!({"id": "result", "required": ["page/result"]}),
        json!({"id": "fail", "required": ["page/fail"]}),
    ];
    if twin {
        pages.push(json!({"id": "twin", "required": ["page/twin"]}));
    }
    serde_json::from_value(json!({"schema_version": "0.3", "pages": pages})).expect("page set")
}

// The package's loading pages and their allowance `max_ms`.
fn loading_pages(max_ms: u64) -> LoadingPageSet {
    serde_json::from_value(json!({"max_ms": max_ms, "pages": [
        {"id": "spinner", "required": ["loading/spinner"]},
        {"id": "curtain", "required": ["loading/curtain"]}
    ]}))
    .expect("loading pages")
}

// `open`: a trusted tap from `home` to `result`, its `expect_after` with `expect_after` members and
// the operation with `extra` members.
fn open(expect_after: Value, extra: Value) -> Value {
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

// What a task is built of, beyond its operations.
#[derive(Clone, Copy)]
struct Package {
    loading_ms: u64,
    task_ms: u64,
    twin: bool,
}

const PACKAGE: Package = Package {
    loading_ms: PACKAGE_LOADING_MS,
    task_ms: TASK_MS,
    twin: false,
};

// The page-graph task of `operations` that ends on `result`, with the error page `fail`, admitted as
// the package's admission admits it.
fn task_of(operations: Vec<Value>, package: Package) -> PreparedContainedTask {
    let control: TaskControl = serde_json::from_value(json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.loading.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 4, "height": 1},
        "entry_task_id": "task",
        "step_timeout_ms": PAGE_WAIT_MS,
        "timeout_ms": package.task_ms
    }))
    .expect("task control");
    control.validate().expect("an admitted control");
    let program: TaskProgram = serde_json::from_value(json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 4, "height": 1},
        "target_page": "result",
        "error_pages": ["fail"],
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
    let detector = PageDetector::new(page_set(package.twin))
        .expect("page detector")
        .with_loading_pages(loading_pages(package.loading_ms))
        .expect("loading pages");
    detector
        .validate(&evaluator)
        .expect("page detector targets");
    program
        .validate_loading(&control, &detector, "resources/operations/task/task.json")
        .expect("admitted loading declarations");
    program
        .validate_expected_targets(
            &control,
            &detector,
            &evaluator,
            "resources/operations/task/task.json",
        )
        .expect("admitted final-state targets");
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

fn task(operations: Vec<Value>) -> PreparedContainedTask {
    task_of(operations, PACKAGE)
}

// A device playing `screens` under the loading mode `mode`.
fn device(screens: Vec<Vec<ScriptedFrame>>, mode: LoadingMode) -> ScriptedRuntime {
    ScriptedRuntime::new(screens).with_loading_mode(mode)
}

fn run_on(task: &PreparedContainedTask, runtime: &mut ScriptedRuntime) -> RunResult {
    task.run(runtime)
}

fn run(
    task: &PreparedContainedTask,
    screens: Vec<Vec<ScriptedFrame>>,
    mode: LoadingMode,
) -> (RunResult, ScriptedRuntime) {
    let mut runtime = device(screens, mode);
    let result = run_on(task, &mut runtime);
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

fn no_loading_values(case: &str, error: &ContainedTaskError) {
    assert_eq!(
        loading_values(error),
        (Value::Null, Value::Null, Value::Null),
        "{case}"
    );
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
    loading_page: &str,
    spent_ms: u64,
    page: Option<&str>,
    exit_gap_ms: Option<u64>,
) -> Transition {
    (
        capture,
        kind,
        loading_page.to_owned(),
        spent_ms,
        page.map(str::to_owned),
        exit_gap_ms,
    )
}

// The run's frames after its input: four spinner frames over no page, one plain frame of no page,
// four curtain frames, then frames of both, forever.
fn reentering() -> Vec<ScriptedFrame> {
    let mut frames = vec![spinning(DARK); 4];
    frames.push(plain(DARK));
    frames.extend(vec![frame(DARK, false, false, true); 4]);
    frames.push(frame(DARK, false, true, true));
    frames
}

// Off: no loading page is evaluated and nothing is recorded. Spinner frames over no page are frames
// of no page: the confirmation runs out at 1 000 ms (twenty pauses of 50 ms). A spinner over
// `result` is `result`: the step goes on the first frame. A wait page `lobby` of 200 ms pauses the
// confirmation for 200 ms and then counts as no destination: the wait fails at 1 200 ms, 24 pauses,
// without loading values.
#[test]
fn loading_mode_off_keeps_todays_wait() {
    let open_task = task(vec![open(json!({}), json!({}))]);
    let (result, runtime) = run(
        &open_task,
        vec![vec![plain(HOME)], vec![spinning(DARK)]],
        LoadingMode::Off,
    );
    let error = failed("spinner over no page", result);
    assert_eq!(error.code().as_str(), CONFIRMATION_FAILED);
    let timing = error.timing().expect("the wait's timing");
    assert_eq!((timing.elapsed_ms, timing.limit_ms), (1_000, 1_000));
    no_loading_values("spinner over no page", &error);
    assert_eq!(pauses_after_input(&runtime, 1), ms(&[50; 20]));
    assert!(runtime.loading_evaluations().is_empty());
    assert!(runtime.loading_events().is_empty());

    let (result, runtime) = run(
        &open_task,
        vec![
            vec![plain(HOME)],
            vec![spinning(RESULT)],
            vec![plain(RESULT)],
        ],
        LoadingMode::Off,
    );
    succeeded("spinner over the destination", &result);
    assert_eq!(runtime.captures_after(1), 1);
    assert!(runtime.loading_evaluations().is_empty());

    let wait_page = task(vec![open(
        json!({"wait_page": {"page_id": "lobby", "max_ms": 200}}),
        json!({}),
    )]);
    let (result, runtime) = run(
        &wait_page,
        vec![vec![plain(HOME)], vec![plain(LOBBY)]],
        LoadingMode::Off,
    );
    let error = failed("the wait page", result);
    assert_eq!(error.code().as_str(), CONFIRMATION_FAILED);
    no_loading_values("the wait page", &error);
    assert_eq!(pauses_after_input(&runtime, 1), ms(&[50; 24]));
    assert!(runtime.loading_events().is_empty());
}

// Shadow: the same three runs end as under `off`, with the same pauses; each frame's loading pages
// are evaluated (the run's first frame, of its page wait, too) and the transitions recorded. The
// spinner over no page enters at the first frame after the input. The spinner over `result` enters
// there too, naming `result`, the page it would suppress. The wait page is one of the input's
// loading pages with its 200 ms: it enters on its first frame, and the frame after 200 ms spent
// records `limit_reached` once while the wait goes on as before.
#[test]
fn loading_shadow_records_without_changing_a_decision() {
    let open_task = task(vec![open(json!({}), json!({}))]);
    let (result, runtime) = run(
        &open_task,
        vec![vec![plain(HOME)], vec![spinning(DARK)]],
        LoadingMode::Shadow,
    );
    let error = failed("spinner over no page", result);
    assert_eq!(error.code().as_str(), CONFIRMATION_FAILED);
    let timing = error.timing().expect("the wait's timing");
    assert_eq!((timing.elapsed_ms, timing.limit_ms), (1_000, 1_000));
    no_loading_values("spinner over no page", &error);
    assert_eq!(pauses_after_input(&runtime, 1), ms(&[50; 20]));
    assert_eq!(
        runtime.loading_evaluations(),
        (1..=22).collect::<Vec<_>>().as_slice()
    );
    assert_eq!(
        transitions(&runtime),
        [seen(2, LoadingEventKind::Entered, "spinner", 0, None, None)]
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

    let (result, runtime) = run(
        &open_task,
        vec![
            vec![plain(HOME)],
            vec![spinning(RESULT)],
            vec![plain(RESULT)],
        ],
        LoadingMode::Shadow,
    );
    succeeded("spinner over the destination", &result);
    assert_eq!(runtime.captures_after(1), 1);
    assert_eq!(
        transitions(&runtime),
        [seen(
            2,
            LoadingEventKind::Entered,
            "spinner",
            0,
            Some("result"),
            None
        )]
    );

    let wait_page = task(vec![open(
        json!({"wait_page": {"page_id": "lobby", "max_ms": 200}}),
        json!({}),
    )]);
    let (result, runtime) = run(
        &wait_page,
        vec![vec![plain(HOME)], vec![plain(LOBBY)]],
        LoadingMode::Shadow,
    );
    let error = failed("the wait page", result);
    assert_eq!(error.code().as_str(), CONFIRMATION_FAILED);
    no_loading_values("the wait page", &error);
    assert_eq!(pauses_after_input(&runtime, 1), ms(&[50; 24]));
    assert_eq!(
        transitions(&runtime),
        [
            seen(2, LoadingEventKind::Entered, "lobby", 0, None, None),
            seen(7, LoadingEventKind::LimitReached, "lobby", 200, None, None),
        ]
    );
}

// On: forty spinner frames over no page (1 950 ms of transition frames, past the 1 000 ms
// confirmation) pause the confirmation, and `result` 50 ms later ends it: the step goes on the 41st
// frame. `left` names `result`, the 1 950 ms spent and the 50 ms since the last spinner frame.
#[test]
fn loading_frame_pauses_the_confirmation_until_the_destination() {
    let task = task(vec![open(json!({}), json!({}))]);
    let mut after_tap = vec![spinning(DARK); 40];
    after_tap.push(plain(RESULT));
    let (result, runtime) = run(
        &task,
        vec![vec![plain(HOME)], after_tap.clone()],
        LoadingMode::On,
    );
    succeeded("on", &result);
    assert_eq!(runtime.captures_after(1), 41);
    assert_eq!(pauses_after_input(&runtime, 1), ms(&[50; 40]));
    assert_eq!(
        transitions(&runtime),
        [
            seen(2, LoadingEventKind::Entered, "spinner", 0, None, None),
            seen(
                42,
                LoadingEventKind::Left,
                "spinner",
                1_950,
                Some("result"),
                Some(50)
            ),
        ]
    );

    let (result, _) = run(&task, vec![vec![plain(HOME)], after_tap], LoadingMode::Off);
    assert_eq!(
        failed("off", result).code().as_str(),
        CONFIRMATION_FAILED,
        "under off the confirmation runs out"
    );
}

// On, an allowance of 500 ms (`loading.max_ms`): four spinner frames spend 150 ms; a plain frame
// leaves; four curtain frames enter again and spend 200 ms; frames of both loading pages (no
// ambiguity) go on in the same stretch and spend 50 ms each up to 500 ms; the next one, at 600 ms,
// ends the wait with the stretch's page, 500 ms of 500.
#[test]
fn several_loading_pages_and_reentries_share_one_allowance() {
    let task = task(vec![open(json!({"loading": {"max_ms": 500}}), json!({}))]);
    let (result, runtime) = run(
        &task,
        vec![vec![plain(HOME)], reentering()],
        LoadingMode::On,
    );
    let error = failed("on", result);
    assert_eq!(error.code().as_str(), CONFIRMATION_FAILED);
    assert_eq!(
        loading_values(&error),
        (json!("curtain"), json!(500), json!(500))
    );
    assert_eq!(pauses_after_input(&runtime, 1), ms(&[50; 12]));
    assert_eq!(
        transitions(&runtime),
        [
            seen(2, LoadingEventKind::Entered, "spinner", 0, None, None),
            seen(6, LoadingEventKind::Left, "spinner", 150, None, Some(50)),
            seen(7, LoadingEventKind::Entered, "curtain", 150, None, None),
            seen(
                14,
                LoadingEventKind::LimitReached,
                "curtain",
                500,
                None,
                None
            ),
        ]
    );
}

// On: a transition frame after the allowance is spent ends the wait on it with no further time and
// no extra round, through the wait's own path and code, with its loading values:
// - no retry policy: `page_confirmation_failed`, 200 ms of 200 after five pauses;
// - an operation that decides to fail: `contained_task_step_unconfirmed`, with its own values and
//   `page_id` the page the spinner suppressed;
// - a wait page of 200 ms is one of the loading pages: no silent fall-back under `on`;
// - the run's page wait, the package's 300 ms: `contained_task_page_unknown`, before any input.
#[test]
fn loading_allowance_spent_ends_the_wait_with_its_values() {
    let limited = task(vec![open(json!({"loading": {"max_ms": 200}}), json!({}))]);
    let mut runtime = device(
        vec![vec![plain(HOME)], vec![spinning(DARK)]],
        LoadingMode::On,
    )
    .with_step_extras(1, Duration::ZERO);
    let error = failed("no retry policy", run_on(&limited, &mut runtime));
    assert_eq!(error.code().as_str(), CONFIRMATION_FAILED);
    assert_eq!(
        loading_values(&error),
        (json!("spinner"), json!(200), json!(200))
    );
    assert_eq!(pauses_after_input(&runtime, 1), ms(&[50; 5]));
    assert_eq!(runtime.extra_rounds_started(), 0);
    assert_eq!(
        transitions(&runtime).last(),
        Some(&seen(
            7,
            LoadingEventKind::LimitReached,
            "spinner",
            200,
            None,
            None
        ))
    );

    let deciding = task(vec![open(
        json!({"loading": {"max_ms": 200}}),
        json!({"retryable": false}),
    )]);
    let (result, runtime) = run(
        &deciding,
        vec![vec![plain(HOME)], vec![spinning(LOBBY)]],
        LoadingMode::On,
    );
    let error = failed("an operation that decides to fail", result);
    assert_eq!(error.code().as_str(), STEP_NOT_CONFIRMED);
    assert_eq!(
        loading_values(&error),
        (json!("spinner"), json!(200), json!(200))
    );
    assert_eq!(value(&error, "pack_operation_id"), json!("open"));
    assert_eq!(value(&error, "page_id"), json!("lobby"));
    assert_eq!(runtime.inputs().len(), 1);

    let wait_page = task(vec![open(
        json!({"wait_page": {"page_id": "lobby", "max_ms": 200}}),
        json!({}),
    )]);
    let (result, runtime) = run(
        &wait_page,
        vec![vec![plain(HOME)], vec![plain(LOBBY)]],
        LoadingMode::On,
    );
    let error = failed("the wait page", result);
    assert_eq!(error.code().as_str(), CONFIRMATION_FAILED);
    assert_eq!(
        loading_values(&error),
        (json!("lobby"), json!(200), json!(200))
    );
    assert_eq!(pauses_after_input(&runtime, 1), ms(&[50; 5]));

    let page_wait = task_of(
        vec![open(json!({}), json!({}))],
        Package {
            loading_ms: 300,
            ..PACKAGE
        },
    );
    let (result, runtime) = run(&page_wait, vec![vec![spinning(DARK)]], LoadingMode::On);
    let error = failed("the page wait", result);
    assert_eq!(error.code().as_str(), PAGE_UNKNOWN);
    assert_eq!(
        loading_values(&error),
        (json!("spinner"), json!(300), json!(300))
    );
    assert!(runtime.inputs().is_empty());
    assert_eq!(pauses_after_input(&runtime, 0), ms(&[50; 7]));
    assert_eq!(
        transitions(&runtime),
        [
            seen(1, LoadingEventKind::Entered, "spinner", 0, None, None),
            seen(
                8,
                LoadingEventKind::LimitReached,
                "spinner",
                300,
                None,
                None
            ),
        ]
    );
}

// On: three spinner frames over `result` are no destination; the plain `result` 150 ms after the
// first ends the step, on the fourth frame. Under shadow the step goes on the first, as before.
#[test]
fn loading_over_a_destination_suppresses_it() {
    let task = task(vec![open(json!({}), json!({}))]);
    let screens = || {
        let mut after_tap = vec![spinning(RESULT); 3];
        after_tap.push(plain(RESULT));
        vec![vec![plain(HOME)], after_tap, vec![plain(RESULT)]]
    };
    let (result, runtime) = run(&task, screens(), LoadingMode::On);
    succeeded("on", &result);
    assert_eq!(runtime.captures_after(1), 4);
    assert_eq!(
        transitions(&runtime),
        [
            seen(
                2,
                LoadingEventKind::Entered,
                "spinner",
                0,
                Some("result"),
                None
            ),
            seen(
                5,
                LoadingEventKind::Left,
                "spinner",
                100,
                Some("result"),
                Some(50)
            ),
        ]
    );

    let (result, runtime) = run(&task, screens(), LoadingMode::Shadow);
    succeeded("shadow", &result);
    assert_eq!(runtime.captures_after(1), 1);
}

// On: the error page under the spinner is the error page (row 2 before row 3): an operation that
// decides to fail ends on it at once, and no transition is recorded.
#[test]
fn loading_never_hides_an_error_page() {
    let task = task(vec![open(json!({}), json!({"retryable": false}))]);
    let (result, runtime) = run(
        &task,
        vec![vec![plain(HOME)], vec![spinning(FAIL)]],
        LoadingMode::On,
    );
    assert_eq!(failed("on", result).code().as_str(), ERROR_PAGE);
    assert_eq!(runtime.captures_after(1), 1);
    assert!(runtime.loading_events().is_empty());
}

// On: `result` and `twin` both hold under the spinner: the recognition conflict, at once.
#[test]
fn loading_never_hides_a_recognition_conflict() {
    let task = task_of(
        vec![open(json!({}), json!({}))],
        Package {
            twin: true,
            ..PACKAGE
        },
    );
    let (result, runtime) = run(
        &task,
        vec![vec![plain(HOME)], vec![spinning(RESULT)]],
        LoadingMode::On,
    );
    assert_eq!(failed("on", result).code().as_str(), CONFLICT);
    assert_eq!(runtime.captures_after(1), 1);
}

// On, an allowance of 100 ms: the shared interrupt pages handle the first four spinner frames, each
// handling taking 40 ms; those frames spend nothing and enter no stretch. The fifth, at 160 ms, is
// not handled: it enters with nothing spent, and the frames 50, 100 and 150 ms on spend 100 ms and
// end the wait. Every transition frame was offered first: eight offers.
#[test]
fn loading_frame_is_offered_to_the_shared_interrupt_pages_first() {
    let task = task(vec![open(json!({"loading": {"max_ms": 100}}), json!({}))]);
    let mut runtime = device(
        vec![vec![plain(HOME)], vec![spinning(DARK)]],
        LoadingMode::On,
    )
    .with_shared_interrupts(vec![true; 4], Duration::from_millis(40));
    let error = failed("on", run_on(&task, &mut runtime));
    assert_eq!(error.code().as_str(), CONFIRMATION_FAILED);
    assert_eq!(
        loading_values(&error),
        (json!("spinner"), json!(100), json!(100))
    );
    assert_eq!(runtime.interrupt_offers(), 8);
    assert_eq!(
        transitions(&runtime),
        [
            seen(6, LoadingEventKind::Entered, "spinner", 0, None, None),
            seen(
                9,
                LoadingEventKind::LimitReached,
                "spinner",
                100,
                None,
                None
            ),
        ]
    );
}

// On, a task of 2 000 ms and an allowance of 600 000 ms: spinner frames pause the confirmation, never
// the task: the run ends with `contained_task_timeout` at its deadline, 2 000 ms after the input.
// Under off the confirmation runs out first, at 1 000 ms.
#[test]
fn loading_pause_never_passes_the_task_deadline() {
    let task = task_of(
        vec![open(json!({"loading": {"max_ms": 600_000}}), json!({}))],
        Package {
            task_ms: 2_000,
            ..PACKAGE
        },
    );
    let screens = || vec![vec![plain(HOME)], vec![spinning(DARK)]];
    let (result, runtime) = run(&task, screens(), LoadingMode::On);
    assert_eq!(failed("on", result).code().as_str(), TASK_TIMEOUT);
    assert_eq!(
        pauses_after_input(&runtime, 1).iter().sum::<Duration>(),
        Duration::from_millis(2_000)
    );
    let (result, _) = run(&task, screens(), LoadingMode::Off);
    assert_eq!(failed("off", result).code().as_str(), CONFIRMATION_FAILED);
}

// On, a still limit of 30 000 ms and the back-off on: fourteen unchanged spinner frames (36 350 ms,
// the pauses doubling up to 5 000) never count as still, and `result` ends the step. Under off the
// same frames are a still screen of no page and end on the limit at 31 350 ms.
#[test]
fn loading_frame_never_counts_as_still() {
    let task = task(vec![open(
        json!({"timeout_ms": 600_000, "loading": {"max_ms": 120_000}}),
        json!({}),
    )]);
    let cadence = CaptureCadence::new(60_000, true, 5_000).with_still_limit_ms(30_000);
    let screens = || {
        let mut after_tap = vec![spinning(DARK); 14];
        after_tap.push(plain(RESULT));
        vec![vec![plain(HOME)], after_tap]
    };
    let mut runtime = device(screens(), LoadingMode::On).with_capture_cadence(cadence);
    succeeded("on", &run_on(&task, &mut runtime));
    assert_eq!(runtime.captures_after(1), 15);

    let mut runtime = device(screens(), LoadingMode::Off).with_capture_cadence(cadence);
    let error = failed("off", run_on(&task, &mut runtime));
    assert_eq!(value(&error, "held_ms"), json!(31_350));
}

// On, a still limit of 30 000 ms: a black screen and a grey one, neither showing a loading page, end
// the confirmation on the limit as without loading pages, with `held_ms` and no loading values.
#[test]
fn plain_still_screen_gets_no_loading_exemption() {
    let task = task(vec![open(json!({"timeout_ms": 600_000}), json!({}))]);
    let cadence = CaptureCadence::new(60_000, true, 5_000).with_still_limit_ms(30_000);
    for (case, screen) in [
        ("black", plain(DARK)),
        ("grey", ScriptedFrame::rgb(4, 1, &[GREY; 4])),
    ] {
        let mut runtime = device(vec![vec![plain(HOME)], vec![screen]], LoadingMode::On)
            .with_capture_cadence(cadence);
        let error = failed(case, run_on(&task, &mut runtime));
        assert_eq!(error.code().as_str(), CONFIRMATION_FAILED, "{case}");
        assert_eq!(
            (value(&error, "held_ms"), value(&error, "limit_ms")),
            (json!(31_350), json!(30_000)),
            "{case}"
        );
        no_loading_values(case, &error);
        assert!(runtime.loading_events().is_empty(), "{case}");
    }
}

// On, the settled path (poll 50 ms) and an allowance of 300 ms: the reading's first frame shows
// `lobby`; the spinner 50 ms later is a transition frame, which spends 50 ms and hands the step to
// its confirmation at once. The confirmation spends the rest of the same allowance in five frames
// and its sixth ends the wait: one stretch, 300 ms of 300.
#[test]
fn settled_path_hands_a_loading_frame_over_with_its_allowance() {
    let task = task(vec![open(
        json!({"loading": {"max_ms": 300}}),
        json!({"post_delay_ms": 200}),
    )]);
    let mut runtime = device(
        vec![vec![plain(HOME)], vec![plain(LOBBY), spinning(DARK)]],
        LoadingMode::On,
    )
    .with_settled_path(
        SettledPath::new(Duration::from_millis(50), 1, 500),
        Duration::from_millis(60_000),
    );
    let error = failed("on", run_on(&task, &mut runtime));
    assert_eq!(error.code().as_str(), CONFIRMATION_FAILED);
    assert_eq!(
        loading_values(&error),
        (json!("spinner"), json!(300), json!(300))
    );
    assert_eq!(
        transitions(&runtime),
        [
            seen(3, LoadingEventKind::Entered, "spinner", 0, None, None),
            seen(
                10,
                LoadingEventKind::LimitReached,
                "spinner",
                300,
                None,
                None
            ),
        ]
    );
    assert_eq!(pauses_after_input(&runtime, 1), ms(&[50; 7]));
}

// On, a retry policy of two attempts and an allowance of 200 ms: the confirmation spends it and
// ends; the failure decision's confirmation, 100 ms later, shares it and ends on its first frame. No
// second input, one `limit_reached`, 200 ms of 200 on the failure.
#[test]
fn failure_decision_shares_the_attempts_allowance() {
    let task = task(vec![open(
        json!({"loading": {"max_ms": 200}}),
        json!({"retryable": true, "max_attempts": 2, "retry_interval_ms": 100}),
    )]);
    let (result, runtime) = run(
        &task,
        vec![vec![plain(HOME)], vec![spinning(DARK)]],
        LoadingMode::On,
    );
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
}

// The confirmation's loading pages and allowance, in the order of their rules: the mode first
// (`off` takes none), then `loading: "off"`, then the package's pages less `except`, with `add` and
// the wait page; the allowance is `loading.max_ms`, else the wait page's, else the package's. A
// confirmation left with no page has none. `shadow` takes the same pages and decides nothing.
#[test]
fn operation_off_except_add_and_max_apply_in_priority_order() {
    let declared = [
        ("none", open(json!({}), json!({}))),
        ("off", open(json!({"loading": "off"}), json!({}))),
        (
            "except",
            open(json!({"loading": {"except": ["curtain"]}}), json!({})),
        ),
        (
            "add",
            open(json!({"loading": {"add": ["lobby"]}}), json!({})),
        ),
        (
            "wait page",
            open(
                json!({"wait_page": {"page_id": "lobby", "max_ms": 200}}),
                json!({}),
            ),
        ),
        ("max", open(json!({"loading": {"max_ms": 500}}), json!({}))),
        (
            "all",
            open(
                json!({"loading": {"except": ["spinner", "curtain"], "add": ["lobby"],
                                   "max_ms": 700}}),
                json!({}),
            ),
        ),
        (
            "nothing left",
            open(
                json!({"loading": {"except": ["spinner", "curtain"]}}),
                json!({}),
            ),
        ),
    ];
    let both = vec!["spinner".to_owned(), "curtain".to_owned()];
    let lobby = vec!["lobby".to_owned()];
    let none = Vec::<String>::new();
    let expected = [
        (both.clone(), none.clone(), PACKAGE_LOADING_MS),
        (none.clone(), none.clone(), 0),
        (vec!["spinner".to_owned()], none.clone(), PACKAGE_LOADING_MS),
        (both.clone(), lobby.clone(), PACKAGE_LOADING_MS),
        (both.clone(), lobby.clone(), 200),
        (both.clone(), none.clone(), 500),
        (none.clone(), lobby.clone(), 700),
        (none.clone(), none.clone(), 0),
    ];
    for ((case, operation), (probed, added, limit)) in declared.into_iter().zip(expected) {
        let task = task(vec![operation]);
        let operation = &task.program.operations[0];
        for mode in [LoadingMode::Shadow, LoadingMode::On] {
            let allowance = LoadingAllowance::for_input(
                &task,
                operation,
                StepPacing::default().with_loading(mode),
            );
            assert_eq!(allowance.probed(), probed.as_slice(), "{case} {mode:?}");
            assert_eq!(allowance.added(), added.as_slice(), "{case} {mode:?}");
            assert_eq!(
                allowance.limit(),
                Duration::from_millis(limit),
                "{case} {mode:?}"
            );
            let decides = mode == LoadingMode::On && (!probed.is_empty() || !added.is_empty());
            assert_eq!(allowance.decides(), decides, "{case} {mode:?}");
        }
        let off = LoadingAllowance::for_input(&task, operation, StepPacing::default());
        assert!(
            off.probed().is_empty() && off.added().is_empty() && !off.decides(),
            "{case}: the mode off takes none"
        );
    }
}

// On, a transition frame is never the frame a step decides on: the run's page wait passes three
// spinner frames over `home` and the input follows the plain `home` 150 ms later; after an
// action gap of 100 ms its page wait does the same. Under shadow the first frame is taken.
#[test]
fn loading_frame_is_never_a_decision_frame() {
    let task = task(vec![open(json!({}), json!({}))]);
    let mut before = vec![spinning(HOME); 3];
    before.push(plain(HOME));
    let screens = |before: &[ScriptedFrame]| vec![before.to_vec(), vec![plain(RESULT)]];
    for (mode, captures) in [(LoadingMode::On, 4), (LoadingMode::Shadow, 1)] {
        let (result, runtime) = run(&task, screens(before.as_slice()), mode);
        succeeded("the page wait", &result);
        assert_eq!(runtime.captures_after(0), captures, "{mode:?}");
        assert_eq!(runtime.inputs().len(), 1, "{mode:?}");
    }

    let gap_before = [plain(HOME), spinning(HOME), spinning(HOME), plain(HOME)];
    for (mode, captures) in [(LoadingMode::On, 4), (LoadingMode::Shadow, 2)] {
        let mut runtime = device(screens(gap_before.as_slice()), mode)
            .with_action_gap(Duration::from_millis(100));
        succeeded("the action gap", &run_on(&task, &mut runtime));
        assert_eq!(runtime.captures_after(0), captures, "{mode:?}");
        assert_eq!(runtime.inputs().len(), 1, "{mode:?}");
    }
}

// A retry policy of two attempts and an allowance of 200 ms, the spinner over `home` after the tap.
// Under on the step never taps again: its confirmation and its failure decision's spend the one
// allowance and the run ends with one input. Under off the frames are `home`, the failure decision
// takes the last one and the step taps a second time.
#[test]
fn loading_never_adds_an_input() {
    let task = task(vec![open(
        json!({"loading": {"max_ms": 200}}),
        json!({"retryable": true, "max_attempts": 2, "retry_interval_ms": 100}),
    )]);
    let screens = || vec![vec![plain(HOME)], vec![spinning(HOME)]];
    let (result, runtime) = run(&task, screens(), LoadingMode::On);
    let error = failed("on", result);
    assert_eq!(error.code().as_str(), CONFIRMATION_FAILED);
    assert_eq!(value(&error, "loading_ms"), json!(200));
    assert_eq!(runtime.inputs().len(), 1);

    let (result, runtime) = run(&task, screens(), LoadingMode::Off);
    let error = failed("off", result);
    assert_eq!(error.code().as_str(), STEP_NOT_CONFIRMED);
    assert_eq!(runtime.inputs().len(), 2);
}

// ACT-4 under on, the settled path: `close` taps on `home` with the popup and expects `home` without
// it (`target_gone`). The first frame after the tap shows `home` without the popup under the
// spinner: a transition frame, no final state and no flip evidence, so the step is handed to its
// confirmation, which goes on the plain `home` after it. Under shadow the first frame turns the
// target and the step goes on it.
#[test]
fn a_transition_frame_is_never_act4_flip_evidence() {
    let close = json!({"id": "close", "from": "home",
                       "click": {"kind": "point", "x": 1, "y": 0},
                       "unguarded_trusted_coordinate": true,
                       "post_delay_ms": 200,
                       "max_runs": 1, "on_max_runs": "next_candidate",
                       "expect_after": {"page_id": "home", "timeout_ms": CONFIRMATION_MS,
                                        "target_gone": ["mark/popup"]}});
    let task = task(vec![close, open(json!({}), json!({}))]);
    let screens = || {
        vec![
            vec![frame(HOME, true, false, false)],
            vec![spinning(HOME), plain(HOME)],
            vec![plain(RESULT)],
        ]
    };
    for (mode, captures) in [(LoadingMode::On, 2), (LoadingMode::Shadow, 1)] {
        let mut runtime = device(screens(), mode).with_settled_path(
            SettledPath::new(Duration::from_millis(50), 1, 500),
            Duration::from_millis(60_000),
        );
        succeeded("close then open", &run_on(&task, &mut runtime));
        assert_eq!(runtime.inputs().len(), 2, "{mode:?}");
        assert_eq!(runtime.captures_after(1), captures, "{mode:?}");
    }
}
