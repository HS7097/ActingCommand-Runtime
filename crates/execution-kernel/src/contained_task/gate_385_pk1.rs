// SPDX-License-Identifier: AGPL-3.0-only

// #385 P-k1a (exec gate key 91; performance model v1.4.2 §5.1–§5.3, §5.5, §5.6; brief PK1a): the
// kernel paces each step by the factor the runtime hands it at the step's start. Every wait lasts
// its declared value × f / 1000; every limit is compared on the run's budget clock, which runs
// 1000 / f as fast as the runtime's clock, so intervals and limits scale together; a factor holds
// from one `step_pacing()` answer to the next; at the factor 1000 every wait is the package's
// value; gesture durations are never scaled. Every expected value comes from those requirement
// lines and the package values below, never from the code.
//
// The runs play scripted frames on the scripted clock (EXEC-I4): a frame's left pixel names its
// page (none for a dark one), a frame the device is slow to deliver moves the clock by its
// latency, every pause moves it by its length and is recorded, and the n-th `step_pacing()` call
// is answered with the n-th scripted factor (after the list, its last one). A page-graph run asks
// once at its start (the factor of its first observations) and once at each step's start.
//
// #385 P-k2 (exec gate key 92; register §1.108; performance model v1.4.2 §5.2, §5.4; test rows
// Tk-6, Tk-7, Tk-8, Tk-12; brief PK2 requirements 2–6): the runtime may also hand each step extra
// wait rounds and a pre-input wait (`with_step_extras`). A wait without an input that reached its
// limit starts one more round of the same length, captures only, at most that many times; the
// confirmation's rounds come before the failure decision; rounds never add an input; the
// observe-only resamples are the base plus the rounds; the pre-input wait lies between the step's
// recognition and its effect intent, as given. Expected values come from those lines and the
// package values below.

use super::tests::scripted::{ScriptedEvent, ScriptedFrame, ScriptedRuntime};
use super::*;
use actingcommand_recognition_pack::RecognitionPack;
use serde_json::{Map, Value, json};
use std::path::PathBuf;

const PAGE_UNKNOWN: &str = SharedCode::ContainedTaskPageUnknown.as_str();
const TASK_TIMEOUT: &str = ExecutionKernelCode::ContainedTaskTimeout.as_str();
const CONFIRMATION_FAILED: &str = ExecutionKernelCode::PageConfirmationFailed.as_str();

const TASK_TIMEOUT_MS: u64 = 60_000;
const STEP_TIMEOUT_MS: u64 = 5_000;
// The package's page wait interval (`capture_interval_ms`).
const CAPTURE_INTERVAL_MS: u64 = 300;
// The wait after a click (`post_delay_ms`).
const POST_DELAY_MS: u64 = 700;
// The confirmation of `open` (`expect_after.timeout_ms`, `expect_after.interval_ms`).
const CONFIRM_TIMEOUT_MS: u64 = 800;
const CONFIRM_INTERVAL_MS: u64 = 400;
// The wait before a retry (`retry_interval_ms`).
const RETRY_INTERVAL_MS: u64 = 900;
// The pause between the two frames of a page target's consensus (`sample_interval_ms`).
const SAMPLE_INTERVAL_MS: u64 = 100;
// The linear path's intermediate states: a window whose lower bound is the post-input delay, and
// a page awaited every `TRANSITION_INTERVAL_MS` for `TRANSITION_TIMEOUT_MS`.
const WINDOW_MS: u64 = 600;
const TRANSITION_TIMEOUT_MS: u64 = 1_000;
const TRANSITION_INTERVAL_MS: u64 = 500;
// The gestures' own durations.
const SWIPE_MS: u64 = 450;
const LONG_PRESS_MS: u64 = 800;

const HOME: [u8; 3] = [255, 0, 0];
const MENU: [u8; 3] = [0, 0, 255];
const RESULT: [u8; 3] = [0, 255, 0];
const AWAY: [u8; 3] = [255, 255, 0];
const DARK: [u8; 3] = [0, 0, 0];

// What the device shows.
#[derive(Debug, Clone, Copy)]
enum Screen {
    Home,
    Menu,
    Result,
    Away,
    // No page of the package.
    Blank,
}

fn shown(screen: Screen) -> ScriptedFrame {
    let page = match screen {
        Screen::Home => HOME,
        Screen::Menu => MENU,
        Screen::Result => RESULT,
        Screen::Away => AWAY,
        Screen::Blank => DARK,
    };
    ScriptedFrame::rgb(2, 1, &[page, DARK])
}

fn frames(screens: &[Screen]) -> Vec<ScriptedFrame> {
    screens.iter().copied().map(shown).collect()
}

fn ms(values: &[u64]) -> Vec<Duration> {
    values.iter().copied().map(Duration::from_millis).collect()
}

// Each value of `values` multiplied by `factor_milli / 1000`.
fn scaled(values: &[u64], factor_milli: u64) -> Vec<Duration> {
    values
        .iter()
        .map(|value| Duration::from_millis(value * factor_milli / 1_000))
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sampling {
    // The pack declares no target consensus.
    SingleFrame,
    // Every page target needs both of two frames `SAMPLE_INTERVAL_MS` apart.
    TwoFrames,
}

// Four pages told apart by the left pixel.
fn pack(sampling: Sampling) -> RecognitionPack {
    let pages = [
        ("page/home", HOME),
        ("page/menu", MENU),
        ("page/result", RESULT),
        ("page/away", AWAY),
    ];
    let targets = pages
        .iter()
        .map(|&(id, expected)| {
            json!({"type": "color", "id": id,
                   "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": expected})
        })
        .collect::<Vec<_>>();
    let mut pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": targets
    });
    if sampling == Sampling::TwoFrames {
        let consensus = pages
            .iter()
            .map(|&(id, _)| {
                let two_frames = json!({"samples": [{"frame": 0}, {"frame": 1}], "k": 2,
                                        "sample_interval_ms": SAMPLE_INTERVAL_MS});
                (id.to_owned(), two_frames)
            })
            .collect::<Map<String, Value>>();
        pack["target_consensus"] = Value::Object(consensus);
    }
    serde_json::from_value(pack).expect("recognition pack")
}

fn page_set() -> PageSet {
    serde_json::from_value(json!({
        "schema_version": "0.3",
        "pages": [
            {"id": "home", "required": ["page/home"]},
            {"id": "menu", "required": ["page/menu"]},
            {"id": "result", "required": ["page/result"]},
            {"id": "away", "required": ["page/away"]}
        ]
    }))
    .expect("page set")
}

fn prepared(
    control: TaskControl,
    program: TaskProgram,
    sampling: Sampling,
) -> PreparedContainedTask {
    let evaluator =
        RecognitionEvaluator::new(PathBuf::new(), pack(sampling)).expect("recognition evaluator");
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

// A page-graph task of `operations` that ends on `result`, waiting for a page every
// `capture_interval_ms` for at most `step_timeout_ms`, within `TASK_TIMEOUT_MS`.
fn page_graph_task(
    sampling: Sampling,
    (capture_interval_ms, step_timeout_ms): (u64, u64),
    operations: Value,
) -> PreparedContainedTask {
    let control: TaskControl = serde_json::from_value(json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.paced.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": capture_interval_ms,
        "step_timeout_ms": step_timeout_ms,
        "timeout_ms": TASK_TIMEOUT_MS
    }))
    .expect("task control");
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
    prepared(control, program, sampling)
}

// Two clicks: `open` from `home` to `menu`, waited `POST_DELAY_MS` after, confirmed every
// `CONFIRM_INTERVAL_MS` for `CONFIRM_TIMEOUT_MS` and attempted twice, `RETRY_INTERVAL_MS` after
// a confirmation that failed; then `go` from `menu` to `result`, waited `POST_DELAY_MS` after.
fn two_steps(sampling: Sampling) -> PreparedContainedTask {
    page_graph_task(
        sampling,
        (CAPTURE_INTERVAL_MS, STEP_TIMEOUT_MS),
        json!([
            {
                "id": "open",
                "from": "home",
                "to": "menu",
                "click": {"kind": "point", "x": 1, "y": 0},
                "unguarded_trusted_coordinate": true,
                "post_delay_ms": POST_DELAY_MS,
                "expect_after": {"page_id": "menu", "timeout_ms": CONFIRM_TIMEOUT_MS,
                                 "interval_ms": CONFIRM_INTERVAL_MS},
                "retryable": true,
                "max_attempts": 2,
                "retry_interval_ms": RETRY_INTERVAL_MS
            },
            {
                "id": "go",
                "from": "menu",
                "to": "result",
                "click": {"kind": "point", "x": 1, "y": 0},
                "unguarded_trusted_coordinate": true,
                "post_delay_ms": POST_DELAY_MS
            }
        ]),
    )
}

// The script of `two_steps` whose first confirmation fails: the run's first frame shows no page
// and the next `home` (one page wait); after the click of `open` three frames of `home` spend the
// confirmation, and after the retry wait `menu` confirms it; after `go`, `result`.
fn retried_confirmation() -> Vec<Vec<ScriptedFrame>> {
    vec![
        frames(&[Screen::Blank, Screen::Home]),
        frames(&[Screen::Home, Screen::Home, Screen::Home, Screen::Menu]),
        frames(&[Screen::Result]),
    ]
}

// The waits of `retried_confirmation`, as the package declares them: the page wait, the wait
// after `open`, two confirmation intervals, the retry wait, the wait after `go`.
const RETRIED_CONFIRMATION_WAITS: [u64; 6] = [
    CAPTURE_INTERVAL_MS,
    POST_DELAY_MS,
    CONFIRM_INTERVAL_MS,
    CONFIRM_INTERVAL_MS,
    RETRY_INTERVAL_MS,
    POST_DELAY_MS,
];

// The linear path `home` -> `menu` -> `result`: `open` declares a window whose lower bound
// `WINDOW_MS` is its post-input delay, and `menu` is awaited every `CONFIRM_INTERVAL_MS`
// (`expect_after.interval_ms`); `go` declares the intermediate page `away`, awaited every
// `TRANSITION_INTERVAL_MS` for `TRANSITION_TIMEOUT_MS`, and is attempted twice,
// `RETRY_INTERVAL_MS` after an intermediate page that was not seen.
fn linear_task() -> PreparedContainedTask {
    let control: TaskControl = serde_json::from_value(json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.paced.linear",
        "execution_mode": "linear_steps",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": CAPTURE_INTERVAL_MS,
        "step_timeout_ms": STEP_TIMEOUT_MS,
        "timeout_ms": TASK_TIMEOUT_MS,
        "max_steps": 2
    }))
    .expect("task control");
    let program: TaskProgram = serde_json::from_value(json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "entry_page": "home",
        "target_page": "result",
        "max_steps": 2,
        "operations": [
            {
                "id": "open",
                "from": "home",
                "to": "menu",
                "click": {"kind": "point", "x": 1, "y": 0},
                "unguarded_trusted_coordinate": true,
                "transition": {"kind": "window", "min_ms": WINDOW_MS, "max_ms": WINDOW_MS},
                "expect_after": {"page_id": "menu", "interval_ms": CONFIRM_INTERVAL_MS}
            },
            {
                "id": "go",
                "from": "menu",
                "to": "result",
                "click": {"kind": "point", "x": 1, "y": 0},
                "unguarded_trusted_coordinate": true,
                "transition": {"kind": "page", "page_id": "away",
                               "timeout_ms": TRANSITION_TIMEOUT_MS,
                               "interval_ms": TRANSITION_INTERVAL_MS},
                "retryable": true,
                "max_attempts": 2,
                "retry_interval_ms": RETRY_INTERVAL_MS
            }
        ]
    }))
    .expect("task program");
    let task = prepared(control, program, Sampling::SingleFrame);
    if let Err(error) = task.program.validate_linear(&task.control, &task.detector) {
        panic!("the fixture is no admitted linear task: {error}");
    }
    task
}

// Runs `task` on `screens`, the n-th `step_pacing()` call answered with `factors[n]`; the run
// succeeds.
fn succeeding_run(
    task: &PreparedContainedTask,
    screens: Vec<Vec<ScriptedFrame>>,
    factors: Vec<u32>,
) -> ScriptedRuntime {
    let mut runtime = ScriptedRuntime::new(screens).with_step_factors(factors);
    match task.run(&mut runtime) {
        Ok(outcome) => assert!(
            matches!(outcome.outcome, TaskOutcome::Success),
            "the run succeeds: {outcome:?}"
        ),
        Err(error) => panic!("expected the run to succeed, it failed: {error:?}"),
    }
    runtime
}

// The code a run that failed ended with.
fn failure_code(
    result: Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>,
) -> String {
    match result {
        Err(ContainedTaskRunError::Task(error)) => error.code().as_str().to_owned(),
        Err(other) => panic!("expected a task failure, got {other:?}"),
        Ok(outcome) => panic!("expected a task failure, the run ended {outcome:?}"),
    }
}

// Requirements 3 and 7 (Tk-1): at the factor 1000 the page wait, the post-input delays, the
// confirmation intervals and the retry wait are the package's values exactly.
#[test]
fn pk1_at_a_factor_of_one_every_wait_is_the_package_value() {
    let task = two_steps(Sampling::SingleFrame);
    let runtime = succeeding_run(&task, retried_confirmation(), vec![1_000]);
    assert_eq!(runtime.pauses(), ms(&RETRIED_CONFIRMATION_WAITS));
}

// Requirement 3 (Tk-2): at the factor 2000 the same page-graph steps pause twice each wait: the
// page wait, the post-input delays, the confirmation intervals and the retry wait; with a target
// consensus, the pause between a consensus's two frames too.
#[test]
fn pk1_a_factor_of_two_doubles_every_wait_of_a_page_graph_step() {
    let task = two_steps(Sampling::SingleFrame);
    let runtime = succeeding_run(&task, retried_confirmation(), vec![2_000]);
    assert_eq!(
        runtime.pauses(),
        scaled(&RETRIED_CONFIRMATION_WAITS, 2_000),
        "every wait of the steps lasts twice its package value"
    );

    // Each observation of a consensus pack takes two frames one sample interval apart: the
    // run's first page, the confirmation of `open` after its post-input delay, the confirmation
    // of `go` after its own.
    let task = two_steps(Sampling::TwoFrames);
    let runtime = succeeding_run(
        &task,
        vec![
            frames(&[Screen::Home]),
            frames(&[Screen::Menu]),
            frames(&[Screen::Result]),
        ],
        vec![2_000],
    );
    assert_eq!(
        runtime.pauses(),
        scaled(
            &[
                SAMPLE_INTERVAL_MS,
                POST_DELAY_MS,
                SAMPLE_INTERVAL_MS,
                POST_DELAY_MS,
                SAMPLE_INTERVAL_MS,
            ],
            2_000
        ),
        "the consensus sample interval lasts twice its package value"
    );
}

// Requirement 3 (Tk-2): at the factor 2000 a `linear_steps` run pauses twice each wait: the
// post-input delay of a window (its lower bound), the arrival interval, the intermediate page's
// interval and the retry delay.
#[test]
fn pk1_a_factor_of_two_doubles_every_wait_of_a_linear_step() {
    let task = linear_task();
    // `home`; after `open` a frame without a page, then `menu`; after the first click of `go`
    // only `menu` (the intermediate page is not seen and the retry decision finds `menu`, the
    // click swallowed); after the second click `away`, then `result`.
    let runtime = succeeding_run(
        &task,
        vec![
            frames(&[Screen::Home]),
            frames(&[Screen::Blank, Screen::Menu]),
            frames(&[Screen::Menu]),
            frames(&[Screen::Away, Screen::Result]),
        ],
        vec![2_000],
    );
    assert_eq!(
        runtime.pauses(),
        scaled(
            &[
                WINDOW_MS,
                CONFIRM_INTERVAL_MS,
                TRANSITION_INTERVAL_MS,
                TRANSITION_INTERVAL_MS,
                RETRY_INTERVAL_MS,
            ],
            2_000
        )
    );
    assert_eq!(
        runtime.inputs().len(),
        3,
        "two clicks of `go`, one of `open`"
    );
}

// Requirement 2 (Tk-3): the factors 1000 for the run's first observations and for `open`, then
// 3000 for `go`. `open` is clicked twice (its retried confirmation fails again, so its second
// attempt clicks again); the factor the runtime would answer during it does not reach it, and
// every wait of `open` is the package's value while the wait after `go` lasts three times its.
#[test]
fn pk1_a_factor_change_takes_effect_at_the_next_step() {
    let task = two_steps(Sampling::SingleFrame);
    let runtime = succeeding_run(
        &task,
        vec![
            frames(&[Screen::Home]),
            frames(&[Screen::Home]),
            frames(&[Screen::Menu]),
            frames(&[Screen::Result]),
        ],
        vec![1_000, 1_000, 3_000],
    );
    assert_eq!(
        runtime.inputs().len(),
        3,
        "two attempts of `open`, one of `go`"
    );
    let mut expected = ms(&[
        POST_DELAY_MS,
        CONFIRM_INTERVAL_MS,
        CONFIRM_INTERVAL_MS,
        RETRY_INTERVAL_MS,
        CONFIRM_INTERVAL_MS,
        CONFIRM_INTERVAL_MS,
        POST_DELAY_MS,
    ]);
    expected.extend(scaled(&[POST_DELAY_MS], 3_000));
    assert_eq!(runtime.pauses(), expected);
}

// Requirement 4 (Tk-4): after the click the device delivers its next frame 8 s later, still
// without a page; the step's page wait is 5 s. At the factor 1000 the wait is spent and the run
// ends `contained_task_page_unknown`; at the factor 2000 those 8 s are 4 s of budget, the wait
// goes on and the target page arrives, with the same one input.
#[test]
fn pk1_a_page_that_arrives_late_is_waited_for_under_a_factor() {
    let task = page_graph_task(
        Sampling::SingleFrame,
        (1_000, STEP_TIMEOUT_MS),
        json!([{
            "id": "open",
            "from": "home",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true
        }]),
    );
    let script = || {
        vec![
            frames(&[Screen::Home]),
            vec![
                shown(Screen::Blank).delivered_after(Duration::from_secs(8)),
                shown(Screen::Result),
            ],
        ]
    };

    let mut unpaced = ScriptedRuntime::new(script()).with_step_factors(vec![1_000]);
    assert_eq!(failure_code(task.run(&mut unpaced)), PAGE_UNKNOWN);
    assert_eq!(unpaced.inputs().len(), 1);

    let paced = succeeding_run(&task, script(), vec![2_000]);
    assert_eq!(paced.inputs().len(), 1, "pacing adds no input");
}

// Requirement 4 (Tk-5, the kernel's half): the task timeout is 60 s and the run takes 90 s of the
// runtime's clock (its two frames each arrive 45 s after their capture starts). At the factor
// 2000 that is 45 s of budget and the run succeeds; at the factor 1000 the same script ends
// `contained_task_timeout`.
#[test]
fn pk1_the_task_timeout_is_compared_on_the_budget_clock() {
    let task = page_graph_task(
        Sampling::SingleFrame,
        (CAPTURE_INTERVAL_MS, STEP_TIMEOUT_MS),
        json!([{
            "id": "open",
            "from": "home",
            "to": "result",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true
        }]),
    );
    let slow = |screen| vec![shown(screen).delivered_after(Duration::from_secs(45))];
    let script = || vec![slow(Screen::Home), slow(Screen::Result)];

    let mut unpaced = ScriptedRuntime::new(script()).with_step_factors(vec![1_000]);
    assert_eq!(failure_code(task.run(&mut unpaced)), TASK_TIMEOUT);

    succeeding_run(&task, script(), vec![2_000]);
}

// Requirement 8 (Tk-11): at the factor 3000 a swipe and a long press carry the package's
// durations, exactly the inputs of the factor 1000.
#[test]
fn pk1_gesture_durations_are_not_scaled() {
    let task = page_graph_task(
        Sampling::SingleFrame,
        (CAPTURE_INTERVAL_MS, STEP_TIMEOUT_MS),
        json!([
            {
                "id": "open",
                "from": "home",
                "to": "menu",
                "click": {"kind": "drag",
                          "from_rect": {"x": 0, "y": 0, "width": 1, "height": 1},
                          "to_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                          "duration_ms": SWIPE_MS},
                "unguarded_trusted_coordinate": true
            },
            {
                "id": "go",
                "from": "menu",
                "to": "result",
                "click": {"kind": "long_press", "x": 1, "y": 0, "duration_ms": LONG_PRESS_MS},
                "unguarded_trusted_coordinate": true
            }
        ]),
    );
    let script = || {
        vec![
            frames(&[Screen::Home]),
            frames(&[Screen::Menu]),
            frames(&[Screen::Result]),
        ]
    };
    let unpaced = succeeding_run(&task, script(), vec![1_000]);
    let paced = succeeding_run(&task, script(), vec![3_000]);
    let durations = paced
        .inputs()
        .iter()
        .map(|input| match input {
            InputAction::Swipe { duration_ms, .. } | InputAction::LongTap { duration_ms, .. } => {
                *duration_ms
            }
            other => panic!("a gesture, got {other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(durations, [SWIPE_MS, LONG_PRESS_MS]);
    assert_eq!(paced.inputs(), unpaced.inputs());
}

// #385 P-k2: a page-graph task whose one click `open` goes from `home` to `result`, confirmed every
// `CONFIRM_INTERVAL_MS` for `CONFIRM_TIMEOUT_MS`, without a retry policy.
fn confirmed_once(sampling: Sampling) -> PreparedContainedTask {
    page_graph_task(
        sampling,
        (CAPTURE_INTERVAL_MS, STEP_TIMEOUT_MS),
        json!([{
            "id": "open",
            "from": "home",
            "to": "result",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true,
            "expect_after": {"page_id": "result", "timeout_ms": CONFIRM_TIMEOUT_MS,
                             "interval_ms": CONFIRM_INTERVAL_MS}
        }]),
    )
}

type RunResult = Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>;

// Runs `task` on `screens` at the factor `factor_milli`, every step handed `extra_wait_rounds` and
// the pre-input wait `pre_input_wait`.
fn paced_run(
    task: &PreparedContainedTask,
    screens: Vec<Vec<ScriptedFrame>>,
    factor_milli: u32,
    (extra_wait_rounds, pre_input_wait): (u32, Duration),
) -> (RunResult, ScriptedRuntime) {
    let mut runtime = ScriptedRuntime::new(screens)
        .with_step_factors(vec![factor_milli])
        .with_step_extras(extra_wait_rounds, pre_input_wait);
    let result = task.run(&mut runtime);
    (result, runtime)
}

fn succeeded(result: &RunResult) -> bool {
    matches!(result, Ok(outcome) if matches!(outcome.outcome, TaskOutcome::Success))
}

// The events of each input: from the recognition last before it to the input itself.
fn before_each_input(events: &[ScriptedEvent]) -> Vec<Vec<ScriptedEvent>> {
    events
        .iter()
        .enumerate()
        .filter(|(_, event)| **event == ScriptedEvent::Input)
        .map(|(input, _)| {
            let recognized = events[..input]
                .iter()
                .rposition(|event| *event == ScriptedEvent::Recognized)
                .expect("a recognition before the input");
            events[recognized..=input].to_vec()
        })
        .collect()
}

// Requirements 3 and 4 (Tk-12): a package without target consensus; after the click of `open` the
// device shows `home` on three frames, which spend the confirmation's window (frames at 0, 400 and
// 800 ms of budget), and `result` on the fourth. At 1 500 with one extra round the fourth frame
// falls in the second round and confirms the step: the run succeeds with one input and reports one
// round. The same frames with no extra round end at the failure decision: the step has no retry
// policy, so `page_confirmation_failed`.
#[test]
fn pk2_an_extra_round_finds_a_late_target_without_a_second_input() {
    let task = confirmed_once(Sampling::SingleFrame);
    let script = || {
        vec![
            frames(&[Screen::Home]),
            frames(&[Screen::Home, Screen::Home, Screen::Home, Screen::Result]),
        ]
    };

    let (result, runtime) = paced_run(&task, script(), 1_500, (1, Duration::ZERO));
    assert!(succeeded(&result), "one extra round: {result:?}");
    assert_eq!(runtime.inputs().len(), 1, "no second input");
    assert_eq!(runtime.extra_rounds_started(), 1, "one round used");

    let (result, runtime) = paced_run(&task, script(), 1_500, (0, Duration::ZERO));
    assert_eq!(failure_code(result), CONFIRMATION_FAILED);
    assert_eq!(runtime.inputs().len(), 1);
    assert_eq!(runtime.extra_rounds_started(), 0);
}

// Requirement 6 (Tk-8): `open` is attempted at most twice (`max_attempts` 2) and its confirmation
// never passes, the device showing `home` throughout. At 3 000 with three extra rounds the run
// makes exactly the inputs it makes at 1 000 without any: the package's `max_attempts`.
#[test]
fn pk2_extra_rounds_never_add_an_input() {
    let task = two_steps(Sampling::SingleFrame);
    let script = || vec![frames(&[Screen::Home])];

    let (paced, paced_runtime) = paced_run(&task, script(), 3_000, (3, Duration::ZERO));
    let (unpaced, unpaced_runtime) = paced_run(&task, script(), 1_000, (0, Duration::ZERO));
    assert!(!succeeded(&paced), "{paced:?}");
    assert!(!succeeded(&unpaced), "{unpaced:?}");
    assert_eq!(
        unpaced_runtime.inputs().len(),
        2,
        "the package's max_attempts"
    );
    assert_eq!(
        paced_runtime.inputs().len(),
        unpaced_runtime.inputs().len(),
        "extra rounds add no input"
    );
}

// Requirement 2 (Tk-7): a package with target consensus (two frames per observation). After the
// click the first frame of the confirmation's first and second observations is delivered 3 s
// late, past the observation's 800 ms window at 2 500, so each one's sampling times out; the step
// is handed a base of 0 observe-only retries and 2 extra rounds. The observation is made again
// twice with no input, and the third one (two frames) confirms the step.
#[test]
fn pk2_extra_sample_retries_resample_without_an_input() {
    let task = confirmed_once(Sampling::TwoFrames);
    let late = || shown(Screen::Result).delivered_after(Duration::from_secs(3));
    let screens = vec![
        frames(&[Screen::Home]),
        vec![late(), late(), shown(Screen::Result), shown(Screen::Result)],
    ];

    let (result, runtime) = paced_run(&task, screens, 2_500, (2, Duration::ZERO));
    assert!(
        succeeded(&result),
        "the third observation confirms: {result:?}"
    );
    assert_eq!(runtime.inputs().len(), 1, "no input while observing again");
    // Two frames of the first page, then one frame of each timed-out observation and two of the
    // third.
    assert_eq!(runtime.captures(), 2 + 1 + 1 + 2);
}

// Requirement 5 (Tk-6): with a pre-input wait of 300 ms (at 2 000), each input of a page-graph
// step and of a `linear_steps` step follows, in order, the step's recognition, one pause of
// exactly 300 ms and the effect intent; without one, the effect intent follows the recognition
// with no pause between.
#[test]
fn pk2_the_pre_input_wait_sits_between_recognition_and_the_effect_intent() {
    const PRE_INPUT_WAIT: Duration = Duration::from_millis(300);
    let page_graph = page_graph_task(
        Sampling::SingleFrame,
        (CAPTURE_INTERVAL_MS, STEP_TIMEOUT_MS),
        json!([{
            "id": "open",
            "from": "home",
            "to": "result",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true
        }]),
    );
    let page_graph_screens = || vec![frames(&[Screen::Home]), frames(&[Screen::Result])];
    let linear = linear_task();
    let linear_screens = || {
        vec![
            frames(&[Screen::Home]),
            frames(&[Screen::Menu]),
            frames(&[Screen::Away, Screen::Result]),
        ]
    };
    let paced = vec![
        ScriptedEvent::Recognized,
        ScriptedEvent::Paused(PRE_INPUT_WAIT),
        ScriptedEvent::EffectIntent,
        ScriptedEvent::Input,
    ];
    let unpaced = vec![
        ScriptedEvent::Recognized,
        ScriptedEvent::EffectIntent,
        ScriptedEvent::Input,
    ];
    for (pre_input_wait, expected) in [(PRE_INPUT_WAIT, &paced), (Duration::ZERO, &unpaced)] {
        for (task, screens, inputs) in [
            (&page_graph, page_graph_screens(), 1),
            (&linear, linear_screens(), 2),
        ] {
            let (result, runtime) = paced_run(task, screens, 2_000, (0, pre_input_wait));
            assert!(succeeded(&result), "{result:?}");
            assert_eq!(
                before_each_input(runtime.events()),
                vec![expected.clone(); inputs],
                "pre-input wait {pre_input_wait:?}"
            );
        }
    }
}
