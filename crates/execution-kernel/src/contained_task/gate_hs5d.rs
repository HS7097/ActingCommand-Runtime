// SPDX-License-Identifier: AGPL-3.0-only

// #402 HS-5d (register §1.279; coordinator work order rd-1031/backend-switch/brief-hs-5d-flake.md;
// design rd-1031/backend-switch/model-hs5.md §4.1 D3). On a Nemu-touch instance the application's
// display comes up some time after the application starts, so the arrival wait after an
// application effect (a startup package's, a ladder rung's) sees frames of the main display first
// and frames of the application's display after them; the capture that takes the other display
// moves the paired session's geometry on (D3). An observation whose frames do not keep one
// geometry ends with `recognition_sample_geometry_changed` (DA-2a C11). Required: that ends the
// observation, not the wait. The arrival wait, a page graph's (the step confirmation every step
// shares) and a step plan's, counts it as a frame that did not confirm the step and goes on in
// the same window after the same pause; a window it spends ends the wait as before, with the
// wait's own code.
//
// The kernel sees the change only as frames of another geometry. Here the main display's frames
// are 2x1 (the desktop, no page) and the application's 4x2 (its home page; a pure scale of the
// 2x1 package, DA-2a), the one geometry a scripted frame carries. The home page needs two frames
// 100 ms apart (target consensus), so every observation of the wait takes two frames, and the
// change is put before the wait's first frame, inside its first observation, between two
// observations, inside a later one, and inside every one. The scripted runtime's clock moves only
// by the kernel's own waits (EXEC-I3), so the frames each observation takes and where the window
// ends are the same on every run, however loaded the machine is.

use super::tests::scripted::{ScriptedFrame, ScriptedRuntime};
use super::*;
use actingcommand_recognition_pack::RecognitionPack;
use serde_json::{Value, json};
use std::path::PathBuf;

const CAPTURE_INTERVAL_MS: u64 = 100;
// The arrival's window.
const STEP_TIMEOUT_MS: u64 = 1_000;
// A task timeout no case reaches.
const TASK_TIMEOUT_MS: u64 = 60_000;
// The pause between the two frames of the home page's consensus.
const SAMPLE_INTERVAL_MS: u64 = 100;

const HOME_PAGE: &str = "neutral/home";
const HOME: [u8; 3] = [255, 0, 0];
const DARK: [u8; 3] = [0, 0, 0];

const PAGE_NOT_CONFIRMED: &str = ExecutionKernelCode::PageConfirmationFailed.as_str();
const APPLICATION_UNCONFIRMED: &str =
    ExecutionKernelCode::ContainedTaskLinearApplicationUnconfirmed.as_str();

// A frame of the main display: the desktop, no page of the package.
fn main_display() -> ScriptedFrame {
    ScriptedFrame::rgb(2, 1, &[DARK, DARK])
}

// A frame of the application's display: its home page, in another geometry than the main
// display's.
fn application_display() -> ScriptedFrame {
    ScriptedFrame::rgb(4, 2, &[HOME, HOME, DARK, DARK, HOME, HOME, DARK, DARK])
}

// What the device shows once the application started: `main` frames of the main display, then
// the application's display on every later frame.
fn switched_after(main: usize) -> Vec<ScriptedFrame> {
    let mut frames = vec![main_display(); main];
    frames.push(application_display());
    frames
}

// Frames of the two displays in turn: every observation's first frame is the main display's and
// its second the application's.
fn straddling() -> Vec<ScriptedFrame> {
    (0..64)
        .flat_map(|_| [main_display(), application_display()])
        .collect()
}

// The device: the application starts at the package's effect, and `after` is what it shows from
// then on (nothing is captured before the effect).
fn device(after: Vec<ScriptedFrame>) -> ScriptedRuntime {
    ScriptedRuntime::new(vec![vec![main_display()], after]).with_application_effect()
}

// The recognition pack: the home page by its left pixel, on two frames 100 ms apart.
fn pack() -> RecognitionPack {
    serde_json::from_value(json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            {"type": "color", "id": "page/home",
             "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": HOME}
        ],
        "target_consensus": {
            "page/home": {"samples": [{"frame": 0}, {"frame": 1}], "k": 2,
                          "sample_interval_ms": SAMPLE_INTERVAL_MS}
        }
    }))
    .expect("recognition pack")
}

fn page_set() -> PageSet {
    serde_json::from_value(json!({
        "schema_version": "0.3",
        "pages": [{"id": HOME_PAGE, "required": ["page/home"]}]
    }))
    .expect("page set")
}

fn prepared(control: Value, program: Value) -> PreparedContainedTask {
    let control: TaskControl = serde_json::from_value(control).expect("task control");
    let program: TaskProgram = serde_json::from_value(program).expect("task program");
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

// The application entry package of a page graph (EXEC-3): from any screen it restarts the
// application, to its home page.
fn graph_task() -> PreparedContainedTask {
    let task = prepared(
        json!({
            "schema_version": "Lab-1y.control.v2",
            "package_id": "neutral.application.entry",
            "execution_mode": "navigable_route",
            "game": "neutral",
            "server": "test",
            "resolution": {"width": 2, "height": 1},
            "entry_task_id": "task",
            "capture_interval_ms": CAPTURE_INTERVAL_MS,
            "step_timeout_ms": STEP_TIMEOUT_MS,
            "timeout_ms": TASK_TIMEOUT_MS
        }),
        json!({
            "schema_version": "0.9",
            "task_id": "task",
            "game": "neutral",
            "server_scope": ["test"],
            "coordinate_space": {"width": 2, "height": 1},
            "entry_page": "any",
            "target_page": "home",
            "operations": [{"id": "start", "from": "any", "to": "home",
                            "application": {"action": "restart"}}]
        }),
    );
    for operation in &task.program.operations {
        operation
            .validate(&task.control, TaskOperationDefaults::default(), "0.9")
            .expect("an admitted operation");
    }
    task
}

// The same entry as a step plan (`linear_steps`, R24): its one step restarts the application and
// its next page is home.
fn linear_task() -> PreparedContainedTask {
    let task = prepared(
        json!({
            "schema_version": "Lab-1y.control.v2",
            "package_id": "neutral.application.entry",
            "execution_mode": "linear_steps",
            "game": "neutral",
            "server": "test",
            "resolution": {"width": 2, "height": 1},
            "entry_task_id": "task",
            "capture_interval_ms": CAPTURE_INTERVAL_MS,
            "step_timeout_ms": STEP_TIMEOUT_MS,
            "timeout_ms": TASK_TIMEOUT_MS,
            "max_steps": 1
        }),
        json!({
            "schema_version": "0.9",
            "task_id": "task",
            "game": "neutral",
            "server_scope": ["test"],
            "coordinate_space": {"width": 2, "height": 1},
            "entry_page": "any",
            "target_page": "home",
            "max_steps": 1,
            "operations": [{"id": "start", "from": "any", "to": "home",
                            "application": {"action": "restart"}}]
        }),
    );
    if let Err(error) = task.program.validate_linear(&task.control, &task.detector) {
        panic!("the fixture is no admitted linear task: {error}");
    }
    task
}

fn arrived(
    case: &str,
    result: Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>,
    runtime: &ScriptedRuntime,
) {
    match result {
        Ok(outcome) => {
            assert_eq!(outcome.outcome, TaskOutcome::Success, "{case}");
            assert_eq!(outcome.final_page.as_deref(), Some(HOME_PAGE), "{case}");
        }
        Err(error) => panic!("{case}: expected the arrival on home, got {error:?}"),
    }
    assert_eq!(
        runtime.applications(),
        [ApplicationLifecycleAction::Restart],
        "{case}: one restart"
    );
    assert!(runtime.inputs().is_empty(), "{case}: no input");
}

fn failure(
    case: &str,
    result: Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>,
) -> ContainedTaskError {
    match result {
        Err(ContainedTaskRunError::Task(error)) => error,
        Err(other) => panic!("{case}: expected a task failure, got {other:?}"),
        Ok(outcome) => panic!("{case}: expected a task failure, the run ended {outcome:?}"),
    }
}

// Where the application's display comes up after the start, by the main display's frames before
// it: before the wait's first frame (0), inside its first observation (1), between its first two
// observations (2), inside its second observation (3).
const SWITCHES: [(usize, &str); 4] = [
    (0, "before the first frame"),
    (1, "inside the first observation"),
    (2, "between two observations"),
    (3, "inside a later observation"),
];

// A page graph's application entry, as a startup package or a ladder rung runs it: wherever the
// display changes, the wait goes on and arrives on home.
#[test]
fn hs5d_graph_arrival_goes_on_across_the_display_change() {
    for (main, case) in SWITCHES {
        let mut runtime = device(switched_after(main));
        let result = graph_task().run_host_package(&mut runtime);
        arrived(case, result, &runtime);
    }
}

// A step plan's application entry: the same.
#[test]
fn hs5d_linear_arrival_goes_on_across_the_display_change() {
    for (main, case) in SWITCHES {
        let mut runtime = device(switched_after(main));
        let result = linear_task().run(&mut runtime);
        arrived(case, result, &runtime);
    }
}

// Every observation takes one frame of each display: none confirms the step, and the wait ends
// where its window ends, with its own code and its 1 000 ms limit (never a longer wait, never the
// geometry's code).
#[test]
fn hs5d_a_window_of_changes_ends_the_wait_with_its_own_code() {
    let mut runtime = device(straddling());
    let error = failure("graph", graph_task().run_host_package(&mut runtime));
    assert_eq!(error.code().as_str(), PAGE_NOT_CONFIRMED, "graph: {error}");
    assert_eq!(
        error.timing().map(|timing| timing.limit_ms),
        Some(STEP_TIMEOUT_MS),
        "graph: {error}"
    );

    let mut runtime = device(straddling());
    let error = failure("linear", linear_task().run(&mut runtime));
    assert_eq!(
        error.code().as_str(),
        APPLICATION_UNCONFIRMED,
        "linear: {error}"
    );
    assert_eq!(
        error.timing().map(|timing| timing.limit_ms),
        Some(STEP_TIMEOUT_MS),
        "linear: {error}"
    );
}
