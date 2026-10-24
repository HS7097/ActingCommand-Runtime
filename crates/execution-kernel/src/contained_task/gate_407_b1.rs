// SPDX-License-Identifier: AGPL-3.0-only

// PERF-B1 (Workflow #407 B1; coordinator brief rd-1024/brief-perf-b1-capture.md, requirements R1
// to R4 frozen for the slice; exec keys 181 to 184): the cadence of a step's captures while it
// waits for a page.
//
// - R1: a package declares a capture interval for one wait, its step's `expect_after.interval_ms`
//   (the confirmation after its input, or a wait step's wait), and its other steps keep their own
//   interval.
// - R2: the largest interval a package may declare is the configuration's (default 60 000 ms); a
//   declared interval above it is refused with a registered code.
// - R3: while a step waits for a page and consecutive frames are unchanged by the Runtime's one
//   frame-change measure (#411 B12, the whole frame), the interval doubles after each unchanged
//   frame and stops at the idle ceiling (default 5 000 ms); the first changed frame returns it to
//   the declared interval; a switch turns it off; it never makes a step pass its own timeout or
//   the task's deadline, its pause being cut at the time left.
// - R4: a package that declares nothing keeps the default interval of 50 ms; only R3 applies to
//   it.
//
// Every expected value comes from those lines and the package values below, never from the code.
// The runs play scripted frames on the scripted clock (EXEC-I4) at the factor 1000: the device
// shows `screens[n]` after `n` inputs, each segment's last frame repeating, a frame is delivered at
// once, and every pause moves the clock by its length, so a wait lasts the sum of its pauses. A
// frame's left pixel names its page (`home`, `lobby`, `result`; dark and grey are no page) and its
// right pixel is dark; a dark frame and a grey one differ by more than the frame-change threshold.
// Unless a test says otherwise the runtime hands the configuration's default cadence: a ceiling of
// 60 000 ms, back-off on, an idle ceiling of 5 000 ms.

use super::tests::scripted::{ScriptedEvent, ScriptedFrame, ScriptedRuntime};
use super::*;
use actingcommand_recognition_pack::RecognitionPack;
use serde_json::{Value, json};
use std::path::PathBuf;

const CONFIRMATION_FAILED: &str = ExecutionKernelCode::PageConfirmationFailed.as_str();
const TASK_TIMEOUT: &str = ExecutionKernelCode::ContainedTaskTimeout.as_str();

// The configuration's default cadence (R2, R3).
const CONFIGURED: CaptureCadence = CaptureCadence::new(60_000, true, 5_000);

const HOME: [u8; 3] = [255, 0, 0];
const LOBBY: [u8; 3] = [0, 0, 255];
const RESULT: [u8; 3] = [0, 255, 0];
const DARK: [u8; 3] = [0, 0, 0];
const GREY: [u8; 3] = [128, 128, 128];

type RunResult = Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>;

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

// A page-graph control with `members` added (a capture interval, timeouts).
fn control(members: Value) -> Value {
    let mut control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.cadence.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task"
    });
    for (member, value) in members.as_object().expect("control members") {
        control[member] = value.clone();
    }
    control
}

// A trusted click from `from` to `to`, confirmed in the package's capture interval and step
// timeout.
fn click(id: &str, from: &str, to: &str) -> Value {
    json!({"id": id, "from": from, "to": to, "click": {"kind": "point", "x": 1, "y": 0},
           "unguarded_trusted_coordinate": true})
}

// A trusted click from `from` confirmed as `expect_after` declares.
fn confirmed_click(id: &str, from: &str, expect_after: Value) -> Value {
    json!({"id": id, "from": from, "click": {"kind": "point", "x": 1, "y": 0},
           "unguarded_trusted_coordinate": true, "expect_after": expect_after})
}

// The page-graph task of `control` that ends on `result` through `operations`, each operation
// admitted as the package's admission admits it.
fn task(control: Value, operations: Vec<Value>) -> PreparedContainedTask {
    let control: TaskControl = serde_json::from_value(control).expect("task control");
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

// The pauses of the run, split at its inputs: `[n]` holds the pauses after the `n`-th input and
// before the next one, which are the waits of step `n` (`[0]`, before any input).
fn pauses_by_input(runtime: &ScriptedRuntime) -> Vec<Vec<Duration>> {
    let mut segments = vec![Vec::new()];
    for event in runtime.events() {
        match event {
            ScriptedEvent::Input => segments.push(Vec::new()),
            ScriptedEvent::Paused(pause) => segments
                .last_mut()
                .expect("a segment of pauses")
                .push(*pause),
            ScriptedEvent::Recognized | ScriptedEvent::EffectIntent => {}
        }
    }
    segments
}

fn total_ms(pauses: &[Duration]) -> u64 {
    pauses
        .iter()
        .map(|pause| u64::try_from(pause.as_millis()).expect("a pause in milliseconds"))
        .sum()
}

// (a) R1 (exec key 181): `open` keeps the package's interval of 100 ms; `start` declares a slow
// wait, a frame every 30 000 ms for at most 100 000 ms, after which the device still shows `lobby`.
// The slow wait captures at most ⌈100 000 / 30 000⌉ + 1 = 5 frames, at 0, 30 000, 60 000, 90 000
// and 100 000 ms of the wait: three pauses of its interval and one cut at its end, 10 000 ms. Its
// frames are unchanged, and an interval above the idle ceiling does not change by back-off. The
// confirmation of `open` sees a dark frame, a grey one (changed) and `lobby`: two pauses of
// 100 ms. The run ends with the slow wait's own code and its time: 100 000 ms of 100 000 ms.
#[test]
fn b1_a_a_slow_wait_keeps_its_declared_interval_and_the_other_steps_keep_theirs() {
    const SLOW_INTERVAL_MS: u64 = 30_000;
    const SLOW_WAIT_MS: u64 = 100_000;
    let task = task(
        control(json!({"capture_interval_ms": 100, "timeout_ms": 600_000})),
        vec![
            click("open", "home", "lobby"),
            confirmed_click(
                "start",
                "lobby",
                json!({"page_id": "result", "timeout_ms": SLOW_WAIT_MS,
                       "interval_ms": SLOW_INTERVAL_MS}),
            ),
        ],
    );
    let (result, runtime) = run(
        &task,
        vec![
            vec![shown(HOME)],
            vec![shown(DARK), shown(GREY), shown(LOBBY)],
            vec![shown(LOBBY)],
        ],
        CONFIGURED,
    );
    let error = failed(result);
    assert_eq!(error.code().as_str(), CONFIRMATION_FAILED);
    let timing = error.timing().expect("the slow wait's timing");
    assert_eq!(
        (timing.elapsed_ms, timing.limit_ms),
        (SLOW_WAIT_MS, SLOW_WAIT_MS),
        "the slow wait ran its whole time and no longer"
    );
    let pauses = pauses_by_input(&runtime);
    assert_eq!(pauses.len(), 3, "two inputs: {pauses:?}");
    assert_eq!(
        pauses[1],
        ms(&[100, 100]),
        "the other step keeps the package's interval"
    );
    assert_eq!(
        pauses[2],
        ms(&[SLOW_INTERVAL_MS, SLOW_INTERVAL_MS, SLOW_INTERVAL_MS, 10_000]),
        "the slow wait takes a frame every declared interval"
    );
    let duration_ms = total_ms(&pauses[2]);
    assert_eq!(duration_ms, SLOW_WAIT_MS);
    let captures = u64::try_from(runtime.captures_after(2)).expect("a frame count");
    assert!(
        captures <= duration_ms.div_ceil(SLOW_INTERVAL_MS) + 1,
        "{captures} frames in {duration_ms} ms at one per {SLOW_INTERVAL_MS} ms"
    );
}

// (b) R3, R4 (exec key 182): a package that declares no interval (R4: 50 ms) confirms its one
// click on `result` within 60 000 ms. After the click the device shows nine unchanged dark frames,
// a grey one (changed) and `result`. With the configuration's cadence the pauses double from
// 50 ms after each unchanged frame, 50, 100, 200, 400, 800, 1 600, 3 200, then stop at the idle
// ceiling, 5 000 and 5 000; the grey frame returns the interval to 50 ms. With an idle ceiling of
// 1 000 ms they stop at 1 000 ms instead. With back-off switched off every pause is 50 ms. Each
// run succeeds on `result`.
#[test]
fn b1_b_unchanged_frames_back_off_to_the_idle_ceiling_and_a_change_returns_to_the_declared_interval()
 {
    let task = task(
        control(json!({})),
        vec![confirmed_click(
            "open",
            "home",
            json!({"page_id": "result", "timeout_ms": 60_000}),
        )],
    );
    let mut after_click = vec![shown(DARK); 9];
    after_click.extend([shown(GREY), shown(RESULT)]);
    for (case, cadence, expected) in [
        (
            "the configuration's cadence",
            CONFIGURED,
            [50, 100, 200, 400, 800, 1_600, 3_200, 5_000, 5_000, 50],
        ),
        (
            "an idle ceiling of 1 000 ms",
            CaptureCadence::new(60_000, true, 1_000),
            [50, 100, 200, 400, 800, 1_000, 1_000, 1_000, 1_000, 50],
        ),
        (
            "back-off switched off",
            CaptureCadence::new(60_000, false, 5_000),
            [50; 10],
        ),
    ] {
        let (result, runtime) = run(&task, vec![vec![shown(HOME)], after_click.clone()], cadence);
        succeeded(result);
        let pauses = pauses_by_input(&runtime);
        assert_eq!(pauses.len(), 2, "{case}: one input: {pauses:?}");
        assert_eq!(pauses[1], ms(&expected), "{case}");
    }
}

// (c) R3 (exec key 183): the back-off never passes the step's remaining time. The device shows
// one unchanged dark frame after the click, and the package declares no interval (50 ms).
// - The confirmation's own time is 4 000 ms: the pauses 50, 100, 200, 400, 800 and 1 600 reach
//   3 150 ms, and the next, 3 200 by back-off, is cut at the 850 ms left. The wait ends at
//   4 000 ms of its 4 000 ms, with the confirmation's own code.
// - The confirmation's own time is 60 000 ms but the task's is 3 000 ms: the pauses reach
//   1 550 ms, and the next, 1 600 by back-off, is cut at the 1 450 ms left before the task's
//   deadline. The run ends at 3 000 ms of its 3 000 ms with the task's own code.
#[test]
fn b1_c_the_back_off_never_passes_the_steps_remaining_time() {
    for (case, control_members, confirmation_ms, expected, code, limit_ms) in [
        (
            "the confirmation's time",
            json!({}),
            4_000,
            vec![50, 100, 200, 400, 800, 1_600, 850],
            CONFIRMATION_FAILED,
            4_000,
        ),
        (
            "the task's deadline",
            json!({"timeout_ms": 3_000}),
            60_000,
            vec![50, 100, 200, 400, 800, 1_450],
            TASK_TIMEOUT,
            3_000,
        ),
    ] {
        let task = task(
            control(control_members),
            vec![confirmed_click(
                "open",
                "home",
                json!({"page_id": "result", "timeout_ms": confirmation_ms}),
            )],
        );
        let (result, runtime) = run(
            &task,
            vec![vec![shown(HOME)], vec![shown(DARK)]],
            CONFIGURED,
        );
        let error = failed(result);
        assert_eq!(error.code().as_str(), code, "{case}");
        let timing = error.timing().expect("the wait's timing");
        assert_eq!(
            (timing.elapsed_ms, timing.limit_ms),
            (limit_ms, limit_ms),
            "{case}: the wait ends at its limit, not past it"
        );
        let pauses = pauses_by_input(&runtime);
        assert_eq!(pauses.len(), 2, "{case}: one input: {pauses:?}");
        assert_eq!(pauses[1], ms(&expected), "{case}");
        assert_eq!(total_ms(&pauses[1]), limit_ms, "{case}");
    }
}

// The `linear_steps` task of one click from `home` to `result` through the intermediate page
// `lobby`, whose wait takes a frame every `transition_interval_ms`, admitted as the linear
// admission admits it.
fn linear_task(transition_interval_ms: u64) -> PreparedContainedTask {
    let control: TaskControl = serde_json::from_value(control(
        json!({"execution_mode": "linear_steps", "max_steps": 1}),
    ))
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
        "max_steps": 1,
        "operations": [{
            "id": "open", "from": "home", "to": "result",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true,
            "transition": {"kind": "page", "page_id": "lobby",
                           "interval_ms": transition_interval_ms}
        }]
    }))
    .expect("task program");
    let evaluator =
        RecognitionEvaluator::new(PathBuf::new(), pack()).expect("recognition evaluator");
    let detector = PageDetector::new(page_set()).expect("page detector");
    detector
        .validate(&evaluator)
        .expect("page detector targets");
    let task = PreparedContainedTask {
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
    };
    task.program
        .validate_linear(&task.control, &task.detector)
        .map(drop)
        .expect("an admitted linear task");
    task
}

// (d) R2 (exec key 184): with a configured ceiling of 20 000 ms, a run refuses a package that
// declares a longer interval before its first capture, with the registered code its admission
// gives a broken interval: `contained_task_control_invalid` for the control's
// `capture_interval_ms` and for a step's `expect_after.interval_ms`, `contained_task_linear_invalid`
// for a `linear_steps` transition's `interval_ms`. A package that declares 20 000 ms on both runs
// and succeeds. Admission, which knows no configuration, takes a declared interval up to the
// largest ceiling the configuration may set, 1 800 000 ms (the longest wait a step may declare),
// and refuses one above it with the same codes.
#[test]
fn b1_d_a_declared_interval_above_the_configured_ceiling_is_refused() {
    const CEILING_MS: u64 = 20_000;
    const KEY_MAX_MS: u64 = 1_800_000;
    let control_invalid = ExecutionKernelCode::ContainedTaskControlInvalid.as_str();
    let linear_invalid = ExecutionKernelCode::ContainedTaskLinearInvalid.as_str();
    let ceiling = CaptureCadence::new(CEILING_MS, true, 5_000);
    let screens = || vec![vec![shown(HOME)], vec![shown(RESULT)]];

    // Above the ceiling in effect: refused before any capture.
    for (case, package, code) in [
        (
            "the control's capture interval",
            task(
                control(json!({"capture_interval_ms": CEILING_MS + 1})),
                vec![click("open", "home", "result")],
            ),
            control_invalid,
        ),
        (
            "a step's own interval",
            task(
                control(json!({})),
                vec![confirmed_click(
                    "open",
                    "home",
                    json!({"page_id": "result", "timeout_ms": 60_000,
                           "interval_ms": CEILING_MS + 1}),
                )],
            ),
            control_invalid,
        ),
        (
            "a transition's interval",
            linear_task(CEILING_MS + 1),
            linear_invalid,
        ),
    ] {
        let (result, runtime) = run(&package, screens(), ceiling);
        assert_eq!(failed(result).code().as_str(), code, "{case}");
        assert_eq!(runtime.captures(), 0, "{case}: refused before any capture");
    }

    // At the ceiling: the run goes on.
    let at_ceiling = task(
        control(json!({"capture_interval_ms": CEILING_MS})),
        vec![confirmed_click(
            "open",
            "home",
            json!({"page_id": "result", "timeout_ms": 60_000, "interval_ms": CEILING_MS}),
        )],
    );
    let (result, runtime) = run(&at_ceiling, screens(), ceiling);
    succeeded(result);
    assert_eq!(runtime.inputs().len(), 1, "the run clicked once");

    // Admission: up to the largest ceiling, and refused above it.
    for (interval_ms, admitted) in [(KEY_MAX_MS, true), (KEY_MAX_MS + 1, false)] {
        let case = format!("an interval of {interval_ms} ms at admission");
        let declared = control(json!({"capture_interval_ms": interval_ms}));
        let operation: TaskOperation = serde_json::from_value(confirmed_click(
            "open",
            "home",
            json!({"page_id": "result", "timeout_ms": KEY_MAX_MS, "interval_ms": interval_ms}),
        ))
        .expect("task operation");
        let base: TaskControl = serde_json::from_value(control(json!({}))).expect("task control");
        let checks = [
            (
                validate_control_declaration(declared).err(),
                control_invalid,
            ),
            (
                operation
                    .validate(&base, TaskOperationDefaults::default(), "0.9")
                    .err(),
                control_invalid,
            ),
            (linear_admission(interval_ms).err(), linear_invalid),
        ];
        for (refusal, code) in checks {
            match (refusal, admitted) {
                (None, true) => {}
                (Some(error), false) => assert_eq!(error.code().as_str(), code, "{case}"),
                (refusal, _) => panic!("{case}: expected admitted={admitted}, got {refusal:?}"),
            }
        }
    }
}

// The linear admission of `linear_task`'s program with a transition interval of `interval_ms`.
fn linear_admission(interval_ms: u64) -> Result<(), ContainedTaskError> {
    let control: TaskControl = serde_json::from_value(control(
        json!({"execution_mode": "linear_steps", "max_steps": 1}),
    ))
    .expect("task control");
    let program: TaskProgram = serde_json::from_value(json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "entry_page": "home",
        "target_page": "result",
        "max_steps": 1,
        "operations": [{
            "id": "open", "from": "home", "to": "result",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true,
            "transition": {"kind": "page", "page_id": "lobby", "interval_ms": interval_ms}
        }]
    }))
    .expect("task program");
    let detector = PageDetector::new(page_set()).expect("page detector");
    program.validate_linear(&control, &detector).map(drop)
}
