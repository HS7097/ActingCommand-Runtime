// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 B12 (exec key 126; B-train plan §2.9, D8, Q7, Q9; test rows BT-B12 T1 to T3): a
// click's input recheck `still_frames` waits until its guard passed on a number of consecutive
// frames that also stayed still, by the Runtime's one frame-change measure
// (contracts/color-digest.md, section Frame change); a frame that changed restarts the count; the
// input binds to the last frame; a screen that never stills within the step timeout refuses the
// input with `contained_task_input_still_unconfirmed`. An input recheck of any kind is admitted
// and run under `linear_steps`.
//
// The screen is three pixels: the page's colour, the guard's colour (ready on every screen here)
// and a grey shade that a looping animation changes. The observation clock is the runtime's
// (`observation_now`); the fake below moves it by one second at every capture, so the step
// timeout of ten seconds ends after a known number of frames. Every expected value comes from the
// plan's §2.9 and its test rows.

use super::*;
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition_pack::RecognitionPack;
use serde_json::{Value, json};
use std::path::PathBuf;

const STEP_TIMEOUT: Duration = Duration::from_secs(10);
const TASK_TIMEOUT: Duration = Duration::from_secs(60);
const TICK: Duration = Duration::from_secs(1);

// The two shades of the animated pixel: black and white differ by every quantized level.
const DARK: u8 = 0;
const LIGHT: u8 = 255;

// What the screen shows: the step's page with its guard ready and the animated pixel's shade, or
// the destination page.
#[derive(Debug, Clone, Copy)]
enum Screen {
    Home(u8),
    Terminal,
}

fn frame(screen: Screen) -> Frame {
    let (page, shade) = match screen {
        Screen::Home(shade) => ([255, 0, 0], shade),
        Screen::Terminal => ([0, 0, 255], DARK),
    };
    Frame::from_pixels(
        3,
        1,
        [page, [0, 255, 0], [shade, shade, shade]].concat(),
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Event {
    Capture,
    StepStarted,
    Guard,
    Intent,
    Input,
}

// The Runtime as the kernel sees it: every capture shows the next screen of the script of the
// inputs made so far (its last screen repeats) and moves the observation clock by one second.
struct ScriptedRuntime {
    scripts: Vec<Vec<Screen>>,
    shown: Vec<usize>,
    origin: Option<Instant>,
    offset: Duration,
    inputs: usize,
    events: Vec<Event>,
}

impl ScriptedRuntime {
    fn new(scripts: Vec<Vec<Screen>>) -> Self {
        Self {
            shown: vec![0; scripts.len()],
            scripts,
            origin: None,
            offset: Duration::ZERO,
            inputs: 0,
            events: Vec::new(),
        }
    }

    fn count(&self, event: &Event) -> usize {
        self.events.iter().filter(|seen| *seen == event).count()
    }

    // The events from the first `StepStarted` to the first input.
    fn before_first_input(&self) -> Vec<Event> {
        let start = self
            .events
            .iter()
            .position(|event| *event == Event::StepStarted)
            .expect("a step started");
        let end = self
            .events
            .iter()
            .position(|event| *event == Event::Input)
            .map_or(self.events.len(), |input| input + 1);
        self.events[start..end].to_vec()
    }
}

impl ContainedTaskRuntime for ScriptedRuntime {
    type Error = &'static str;

    fn observe_task_timing(&mut self, context: ContainedTaskTimingContext) {
        if self.origin.is_none() {
            self.origin = Some(context.deadline() - TASK_TIMEOUT);
        }
    }

    fn observation_now(&mut self) -> Instant {
        self.origin
            .expect("the run gives its timing before it observes")
            + self.offset
    }

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        self.offset += TICK;
        let script = &self.scripts[self.inputs.min(self.scripts.len() - 1)];
        let shown = &mut self.shown[self.inputs.min(self.scripts.len() - 1)];
        let screen = script[(*shown).min(script.len() - 1)];
        *shown += 1;
        Ok(frame(screen).into())
    }

    fn input(
        &mut self,
        _action: InputAction,
        _frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        self.inputs += 1;
        self.events.push(Event::Input);
        Ok(())
    }

    fn record_guard_evaluation(
        &mut self,
        _target_id: Option<&str>,
        _result: Option<&actingcommand_recognition_pack::RecognitionPackResult<TargetEvaluation>>,
        _evaluation: ContainedTaskGuardEvaluation,
    ) -> Result<(), Self::Error> {
        self.events.push(Event::Guard);
        Ok(())
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        match trace {
            ContainedTaskTrace::CaptureCompleted { .. } => self.events.push(Event::Capture),
            ContainedTaskTrace::StepStarted { .. } => self.events.push(Event::StepStarted),
            ContainedTaskTrace::EffectIntent { .. } => self.events.push(Event::Intent),
            _ => {}
        }
        Ok(())
    }
}

fn control_json(mode: &str) -> Value {
    let mut control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.still.task",
        "execution_mode": mode,
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 3, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": STEP_TIMEOUT.as_millis() as u64,
        "timeout_ms": TASK_TIMEOUT.as_millis() as u64
    });
    if mode == "linear_steps" {
        control["max_steps"] = json!(1);
    }
    control
}

fn pack() -> RecognitionPack {
    serde_json::from_value(json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 3, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            {"type": "color", "id": "page/home",
             "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [255, 0, 0]},
            {"type": "color", "id": "page/terminal",
             "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [0, 0, 255]},
            {"type": "color", "id": "guard/ready",
             "region": {"x": 1, "y": 0, "width": 1, "height": 1}, "expected": [0, 255, 0]}
        ]
    }))
    .expect("recognition pack")
}

fn page_set() -> PageSet {
    serde_json::from_value(json!({
        "schema_version": "0.3",
        "pages": [
            {"id": "neutral/home", "required": ["page/home"]},
            {"id": "neutral/terminal", "required": ["page/terminal"]}
        ]
    }))
    .expect("page set")
}

// The guarded click from `home` to `terminal` with the input recheck `recheck`.
fn open(recheck: Value) -> Value {
    json!({
        "id": "open",
        "from": "home",
        "to": "terminal",
        "click": {"kind": "point", "x": 1, "y": 0},
        "guard": {"page_id": "home", "target_id": "guard/ready",
                  "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                  "color_probe": "guard/ready"},
        "input_recheck": recheck
    })
}

// A task of the control's `mode` whose one operation is `open(recheck)`.
fn task(mode: &str, recheck: Value) -> PreparedContainedTask {
    let control: TaskControl = serde_json::from_value(control_json(mode)).expect("task control");
    let mut program = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 3, "height": 1},
        "target_page": "terminal",
        "operations": [open(recheck)]
    });
    if mode == "linear_steps" {
        program["entry_page"] = json!("home");
        program["max_steps"] = json!(1);
    }
    let program: TaskProgram = serde_json::from_value(program).expect("task program");
    for operation in &program.operations {
        operation
            .validate(&control, TaskOperationDefaults::default(), "0.9")
            .expect("valid operation");
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

fn succeeded(result: Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>) {
    match result {
        Ok(outcome) => assert_eq!(outcome.outcome, TaskOutcome::Success),
        Err(error) => panic!("expected the run to succeed, got {error:?}"),
    }
}

// The guard of the step's frame, then `fresh` fresh frames each captured and guarded, then the
// effect intent and the input.
fn rechecked_input(fresh: usize) -> Vec<Event> {
    let mut events = vec![Event::StepStarted, Event::Guard];
    for _ in 0..fresh {
        events.extend([Event::Capture, Event::Guard]);
    }
    events.extend([Event::Intent, Event::Input]);
    events
}

// T1 (§2.9 「画面一动就重新数」, "the count restarts when … the colour-digest distance of the region
// between consecutive frames exceeds the threshold; the input binds to the last frame"): with
// three still frames over the animated pixel, the step's frame and the first fresh frame are
// still (two), the second fresh frame changed (the count starts again at one), and the input
// follows the fourth fresh frame, the third still one in a row, drawn from its guard.
#[test]
fn b12_t1_a_frame_that_moved_restarts_the_count_and_the_input_binds_the_last_still_frame() {
    let still = task(
        "navigable_route",
        json!({"kind": "still_frames", "frames": 3,
               "region": {"x": 2, "y": 0, "width": 1, "height": 1}}),
    );
    let mut runtime = ScriptedRuntime::new(vec![
        vec![
            // The step's frame: one.
            Screen::Home(DARK),
            // Still: two.
            Screen::Home(DARK),
            // Moved: one again.
            Screen::Home(LIGHT),
            // Still: two, then three.
            Screen::Home(LIGHT),
            Screen::Home(LIGHT),
        ],
        vec![Screen::Terminal],
    ]);
    succeeded(still.run(&mut runtime));
    assert_eq!(runtime.inputs, 1);
    assert_eq!(runtime.before_first_input(), rechecked_input(4));
}

// T2 (§2.9 "Failure within the step timeout ends with `contained_task_input_still_unconfirmed`",
// Q7; D8 defaults: the whole frame and the default threshold): a looping animation whose guard
// passes on every frame never stills, so the step timeout of ten seconds ends the wait: the input
// is refused with the new code, its operation and the step timeout, and nothing is input.
#[test]
fn b12_t2_a_screen_that_never_stills_refuses_the_input_with_the_still_code() {
    let still = task(
        "navigable_route",
        json!({"kind": "still_frames", "frames": 3}),
    );
    let looping = (0..32)
        .map(|index| Screen::Home(if index % 2 == 0 { DARK } else { LIGHT }))
        .collect::<Vec<_>>();
    let mut runtime = ScriptedRuntime::new(vec![looping]);
    let error = match still.run(&mut runtime) {
        Err(ContainedTaskRunError::Task(error)) => error,
        Err(other) => panic!("expected a task failure, got {other:?}"),
        Ok(outcome) => panic!(
            "expected a task failure, the run ended {:?}",
            outcome.outcome
        ),
    };
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::ContainedTaskInputStillUnconfirmed.as_str()
    );
    assert_eq!(
        Value::Object(error.outcome().values().clone()),
        json!({"pack_operation_id": "open", "timeout_ms": 10_000})
    );
    assert_eq!(runtime.inputs, 0);
    assert_eq!(runtime.count(&Event::Intent), 0);
}

// T3 (§2.9 "`input_recheck` (all kinds) is admitted under `linear_steps`", Q9): a `linear_steps`
// package whose click declares a recheck of each kind is admitted, and its run checks the click
// again on a fresh frame of the step's page before the effect intent and the input.
#[test]
fn b12_t3_a_linear_package_admits_and_runs_every_input_recheck() {
    for recheck in [
        json!({"kind": "fresh_frame"}),
        json!({"kind": "stable_frames", "frames": 2}),
        json!({"kind": "still_frames", "frames": 2}),
    ] {
        let linear = task("linear_steps", recheck.clone());
        if let Err(error) = linear
            .program
            .validate_linear(&linear.control, &linear.detector)
        {
            panic!("{recheck}: a linear task admits the recheck, got {error}");
        }
        let mut runtime =
            ScriptedRuntime::new(vec![vec![Screen::Home(DARK)], vec![Screen::Terminal]]);
        succeeded(linear.run(&mut runtime));
        assert_eq!(runtime.inputs, 1, "{recheck}");
        assert_eq!(
            runtime.before_first_input(),
            rechecked_input(1),
            "{recheck}"
        );
    }
}
