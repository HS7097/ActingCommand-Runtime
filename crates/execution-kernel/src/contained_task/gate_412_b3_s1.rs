// SPDX-License-Identifier: AGPL-3.0-only

// #412 Lab B-3 S1 (Workflow #412 B-3; frozen design rd-1021/train-0129/lab-b3-design.md §1.1,
// §2.1, §2.2 and §5 "S1", tests T1-1 to T1-9; register §1.161; exec keys 136 to 143 and 180): the
// kernel runs one operation a package declares as a run runs it, and returns after that one step.
// Every expected value comes from the design: the six points of §2.1, what runs unchanged (§2.2),
// the admission refusals of §1.1 and the codes of §4.
//
// The device is the kernel's scripted-frame runtime (`tests::scripted`): it shows `screens[n]`
// after `n` inputs, one frame per capture, and has a clock of its own that only pauses move. The
// Lab's runtime around it answers the step's draw (action seed, touch keys, the landing history's
// choice), counts the fact reads of `selection_state`, and keeps what the kernel did in order.

use super::tests::scripted::{ScriptedFrame, ScriptedRuntime};
use super::*;
use actingcommand_contract::{InputSamplingAlgorithm, InputSamplingParameters};
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition_pack::{
    AssetResolver, RecognitionPack, RecognitionPackResult, load_pack_from_json_str,
};
use serde_json::{Value, json};
use std::path::PathBuf;

const HOME: &str = "neutral/home";
const TERMINAL: &str = "neutral/terminal";
const OTHER: &str = "neutral/other";

const RED: [u8; 3] = [255, 0, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const YELLOW: [u8; 3] = [255, 255, 0];
const GREEN: [u8; 3] = [0, 255, 0];
const BLACK: [u8; 3] = [0, 0, 0];

// What the kernel did, in order, as the Lab's runtime saw it.
#[derive(Debug, Clone, PartialEq)]
enum Seen {
    Capture,
    Pause(Duration),
    // A guard evaluation of the given kind (a `when` included).
    Guard(ContainedTaskGuardEvaluation),
    // `StepStarted`: the operation and its step index.
    StepStarted(String, u32),
    // `EffectIntent` of the operation.
    Intent(String),
    Input(InputAction),
    // `StepFinished`: the operation and the page it finished on.
    StepFinished(String, String),
    // A reading or stability trace, by its variant's name.
    Fact(&'static str),
}

// The Lab's runtime around the scripted device: the step's action seed and touch keys, the index
// its landing history chooses among a press's candidates, the candidates it was offered, the
// fact reads, the effect intents' actions and draws, and everything in order.
struct LabRuntime {
    device: ScriptedRuntime,
    seed: Option<u64>,
    touch: Option<TouchSampling>,
    choice: usize,
    offered: Vec<(InputSamplingRegion, Vec<(i32, i32)>)>,
    fact_reads: usize,
    intents: Vec<(InputAction, Option<InputSamplingEvidence>)>,
    seen: Vec<Seen>,
}

impl LabRuntime {
    fn new(screens: Vec<Vec<ScriptedFrame>>) -> Self {
        Self {
            device: ScriptedRuntime::new(screens),
            seed: None,
            touch: None,
            choice: 0,
            offered: Vec::new(),
            fact_reads: 0,
            intents: Vec::new(),
            seen: Vec::new(),
        }
    }

    fn inputs(&self) -> Vec<InputAction> {
        self.seen
            .iter()
            .filter_map(|seen| match seen {
                Seen::Input(action) => Some(action.clone()),
                _ => None,
            })
            .collect()
    }

    fn after_first_input(&self) -> Vec<Seen> {
        let input = self
            .seen
            .iter()
            .position(|seen| matches!(seen, Seen::Input(_)))
            .expect("an input was made");
        self.seen[input + 1..].to_vec()
    }
}

impl ContainedTaskRuntime for LabRuntime {
    type Error = &'static str;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        self.seen.push(Seen::Capture);
        self.device.capture()
    }

    fn action_seed(
        &mut self,
        _step_index: u32,
        _operation_label: &str,
    ) -> Result<Option<u64>, Self::Error> {
        Ok(self.seed)
    }

    fn touch_sampling(&mut self) -> Result<Option<TouchSampling>, Self::Error> {
        Ok(self.touch)
    }

    fn choose_touch_candidate(
        &mut self,
        region: &InputSamplingRegion,
        candidates: &[(i32, i32)],
    ) -> Result<usize, Self::Error> {
        self.offered.push((*region, candidates.to_vec()));
        Ok(self.choice)
    }

    fn input(
        &mut self,
        action: InputAction,
        frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        self.seen.push(Seen::Input(action.clone()));
        self.device.input(action, frame)
    }

    fn selection_state(
        &mut self,
        _request: SelectionStateRequest,
    ) -> Result<SelectionState, Self::Error> {
        self.fact_reads += 1;
        Ok(SelectionState::Unavailable)
    }

    fn record_guard_evaluation(
        &mut self,
        _target_id: Option<&str>,
        _result: Option<&actingcommand_recognition_pack::RecognitionPackResult<TargetEvaluation>>,
        evaluation: ContainedTaskGuardEvaluation,
    ) -> Result<(), Self::Error> {
        self.seen.push(Seen::Guard(evaluation));
        Ok(())
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        let seen = match &trace {
            ContainedTaskTrace::StepStarted {
                step_index,
                operation_label,
                ..
            } => Some(Seen::StepStarted(operation_label.clone(), *step_index)),
            ContainedTaskTrace::EffectIntent {
                operation_label,
                action,
                sampling,
                ..
            } => {
                self.intents.push((action.clone(), sampling.clone()));
                Some(Seen::Intent(operation_label.clone()))
            }
            ContainedTaskTrace::StepFinished {
                operation_label,
                page_label,
                ..
            } => Some(Seen::StepFinished(
                operation_label.clone(),
                page_label.clone(),
            )),
            ContainedTaskTrace::ResourceReadingFrame { .. } => {
                Some(Seen::Fact("ResourceReadingFrame"))
            }
            ContainedTaskTrace::ResourceReadings { .. } => Some(Seen::Fact("ResourceReadings")),
            ContainedTaskTrace::ResourceAdjustmentConfirmed { .. } => {
                Some(Seen::Fact("ResourceAdjustmentConfirmed"))
            }
            ContainedTaskTrace::ResourceAdjustments { .. } => {
                Some(Seen::Fact("ResourceAdjustments"))
            }
            ContainedTaskTrace::StabilityBaseline { .. } => Some(Seen::Fact("StabilityBaseline")),
            ContainedTaskTrace::StabilityComparison { .. } => {
                Some(Seen::Fact("StabilityComparison"))
            }
            ContainedTaskTrace::StabilityTerminal { .. } => Some(Seen::Fact("StabilityTerminal")),
            _ => None,
        };
        if let Some(seen) = seen {
            self.seen.push(seen);
        }
        self.device.record(trace)
    }

    fn sampling_supported(&self) -> bool {
        self.device.sampling_supported()
    }

    fn step_pacing(&mut self) -> Result<StepPacing, Self::Error> {
        self.device.step_pacing()
    }

    fn monotonic_now(&mut self) -> Instant {
        self.device.monotonic_now()
    }

    fn pause(&mut self, duration: Duration) {
        self.seen.push(Seen::Pause(duration));
        self.device.pause(duration);
    }
}

// A page-graph control of `mode` for frames of `width` × `height`.
fn control(mode: &str, width: u32, height: u32) -> TaskControl {
    let mut control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.declared.task",
        "execution_mode": mode,
        "game": "neutral",
        "server": "test",
        "resolution": {"width": width, "height": height},
        "entry_task_id": "task",
        "capture_interval_ms": 50,
        "step_timeout_ms": 10_000,
        "timeout_ms": 60_000
    });
    if mode == "linear_steps" {
        control["max_steps"] = json!(1);
    }
    serde_json::from_value(control).expect("task control")
}

// The page set: `home`, `terminal` and `other`, each by its colour target.
fn page_set() -> PageSet {
    serde_json::from_value(json!({
        "schema_version": "0.3",
        "pages": [
            {"id": HOME, "required": ["page/home"]},
            {"id": TERMINAL, "required": ["page/terminal"]},
            {"id": OTHER, "required": ["page/other"]}
        ]
    }))
    .expect("page set")
}

// The three page colours on the frame's first pixel, and `extra` targets.
fn pack_json(width: u32, height: u32, extra: Vec<Value>) -> Value {
    let mut targets = vec![
        json!({"type": "color", "id": "page/home",
               "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": RED}),
        json!({"type": "color", "id": "page/terminal",
               "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": BLUE}),
        json!({"type": "color", "id": "page/other",
               "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": YELLOW}),
    ];
    targets.extend(extra);
    json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": width, "height": height},
        "defaults": {"color_max_distance": 0.0},
        "targets": targets
    })
}

// A task schema `0.9` program of `operations`, with `extra` members.
fn program(width: u32, height: u32, operations: Vec<Value>, extra: Value) -> TaskProgram {
    let mut program = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": width, "height": height},
        "target_page": "terminal",
        "operations": operations
    });
    for (member, value) in extra.as_object().expect("extra members") {
        program[member] = value.clone();
    }
    serde_json::from_value(program).expect("task program")
}

// The prepared task of `control` and `program` over `evaluator`, every operation admitted as
// admission admits it.
fn prepared(
    control: TaskControl,
    program: TaskProgram,
    evaluator: RecognitionEvaluator,
) -> PreparedContainedTask {
    for operation in &program.operations {
        operation
            .validate(&control, TaskOperationDefaults::default(), "0.9")
            .unwrap_or_else(|error| panic!("operation {}: {error}", operation.id));
    }
    let detector = PageDetector::new(page_set()).expect("page detector");
    detector
        .validate(&evaluator)
        .expect("page detector targets");
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

// --- Two-pixel screens: the page, and a flag that is ready (green). ---

fn flag_pack() -> RecognitionPack {
    serde_json::from_value(pack_json(
        2,
        1,
        vec![json!({"type": "color", "id": "flag/ready",
                    "region": {"x": 1, "y": 0, "width": 1, "height": 1}, "expected": GREEN})],
    ))
    .expect("recognition pack")
}

fn flag_screen(page: [u8; 3], flag: [u8; 3]) -> ScriptedFrame {
    ScriptedFrame::rgb(2, 1, &[page, flag])
}

// `claim`: the click from `home` to `terminal` guarded by the ready flag, with `extra` members.
fn claim(extra: Value) -> Value {
    with(
        json!({
            "id": "claim",
            "from": "home",
            "to": "terminal",
            "click": {"kind": "point", "x": 1, "y": 0},
            "guard": {"page_id": "home", "target_id": "flag/ready",
                      "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                      "color_probe": "flag/ready"}
        }),
        extra,
    )
}

// `skip`: a trusted tap from `home` to `other`.
fn skip() -> Value {
    json!({
        "id": "skip",
        "from": "home",
        "to": "other",
        "click": {"kind": "point", "x": 0, "y": 0},
        "unguarded_trusted_coordinate": true
    })
}

fn with(mut operation: Value, extra: Value) -> Value {
    for (member, value) in extra.as_object().expect("extra members") {
        operation[member] = value.clone();
    }
    operation
}

fn flag_task(operations: Vec<Value>, extra: Value) -> PreparedContainedTask {
    prepared(
        control("navigable_route", 2, 1),
        program(2, 1, operations, extra),
        RecognitionEvaluator::new(PathBuf::new(), flag_pack()).expect("evaluator"),
    )
}

// --- Larger screens for the gestures: the page on the first pixel, black elsewhere. ---

const WIDE: (u32, u32) = (320, 240);

fn wide_screen(page: [u8; 3]) -> ScriptedFrame {
    let (width, height) = WIDE;
    let mut pixels = vec![BLACK; (width * height) as usize];
    pixels[0] = page;
    ScriptedFrame::rgb(width, height, &pixels)
}

fn wide_task(operations: Vec<Value>) -> PreparedContainedTask {
    let (width, height) = WIDE;
    let pack: RecognitionPack =
        serde_json::from_value(pack_json(width, height, Vec::new())).expect("recognition pack");
    prepared(
        control("navigable_route", width, height),
        program(width, height, operations, json!({})),
        RecognitionEvaluator::new(PathBuf::new(), pack).expect("evaluator"),
    )
}

fn confirmed(
    result: Result<DeclaredOperationOutcome, ContainedTaskRunError<&'static str>>,
) -> String {
    match result {
        Ok(DeclaredOperationOutcome::Confirmed { page }) => page,
        Ok(other) => panic!("expected the step to be confirmed, it ended {other:?}"),
        Err(error) => panic!("expected the step to be confirmed, got {error:?}"),
    }
}

fn values(error: &ContainedTaskError) -> Value {
    Value::Object(error.outcome().values().clone())
}

// T1-1 (exec key 136; design §2.1 point 3, "Candidates are only the declared operation"): with two
// operations on one page, the one the Lab names runs although the page's first candidate is the
// other: only it is pressed, its step is the run's step 0, and the run returns `Confirmed` on its
// destination after that step.
#[test]
fn b3_s1_t1_1_only_the_named_operation_of_the_page_runs_and_is_confirmed() {
    let task = flag_task(vec![claim(json!({})), skip()], json!({}));
    let mut runtime = LabRuntime::new(vec![
        vec![flag_screen(RED, GREEN)],
        vec![flag_screen(YELLOW, GREEN)],
    ]);

    let page = confirmed(task.run_declared_operation(&mut runtime, "skip"));

    assert_eq!(page, OTHER);
    assert_eq!(
        runtime.seen,
        vec![
            Seen::Capture,
            Seen::StepStarted("skip".to_owned(), 0),
            Seen::Guard(ContainedTaskGuardEvaluation::TrustedCoordinate),
            Seen::Intent("skip".to_owned()),
            Seen::Input(InputAction::Tap { x: 0, y: 0 }),
            Seen::Capture,
            Seen::StepFinished("skip".to_owned(), OTHER.to_owned()),
        ]
    );
}

// T1-2 (exec key 137; design §2.1 point 2, §4): a first frame of another page than the operation's
// `from` ends the run before any step and any input with
// `contained_task_declared_operation_page_mismatch`, naming the operation, the page the frame
// showed and the declared `from`.
#[test]
fn b3_s1_t1_2_another_page_ends_the_run_without_an_input() {
    let task = flag_task(vec![claim(json!({})), skip()], json!({}));
    let mut runtime = LabRuntime::new(vec![vec![flag_screen(YELLOW, GREEN)]]);

    let error = match task.run_declared_operation(&mut runtime, "claim") {
        Err(ContainedTaskRunError::Task(error)) => error,
        Err(other) => panic!("expected a task failure, got {other:?}"),
        Ok(end) => panic!("expected a task failure, the run ended {end:?}"),
    };

    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::ContainedTaskDeclaredOperationPageMismatch.as_str()
    );
    assert_eq!(
        values(&error),
        json!({"pack_operation_id": "claim", "page_id": OTHER, "page_ids": ["home"]})
    );
    assert_eq!(runtime.seen, vec![Seen::Capture]);
}

// T1-3 (exec key 138; design §2.2 "point", decision Q1): a `rect` click, with the runtime's touch
// keys at `candidate_count` 3, offers the runtime's landing history three candidates of its box
// and presses the one it chooses (the third); the effect intent's draw names the step's action
// seed, the box, the least-recent algorithm and the chosen candidate.
#[test]
fn b3_s1_t1_3_a_box_press_lands_on_the_candidate_the_runtime_chooses() {
    const SEED: u64 = 0x0123_4567_89ab_cdef;
    let task = wide_task(vec![json!({
        "id": "press",
        "from": "home",
        "to": "terminal",
        "click": {"kind": "rect", "x": 100, "y": 50, "width": 120, "height": 80},
        "unguarded_trusted_coordinate": true
    })]);
    let mut runtime = LabRuntime::new(vec![vec![wide_screen(RED)], vec![wide_screen(BLUE)]]);
    runtime.seed = Some(SEED);
    runtime.touch = Some(TouchSampling {
        sigma_ratio_milli: 250,
        max_redraws: 8,
        candidate_count: 3,
    });
    runtime.choice = 2;

    let page = confirmed(task.run_declared_operation(&mut runtime, "press"));

    assert_eq!(page, TERMINAL);
    let region = InputSamplingRegion::new(100, 50, 120, 80).expect("sampling region");
    assert_eq!(runtime.offered.len(), 1, "one choice is asked");
    let (offered_region, candidates) = &runtime.offered[0];
    assert_eq!(*offered_region, region);
    assert_eq!(candidates.len(), 3);
    let (x, y) = candidates[2];
    assert_eq!(runtime.inputs(), vec![InputAction::Tap { x, y }]);
    let [(action, Some(sampling))] = runtime.intents.as_slice() else {
        panic!("one effect intent with its draw: {:?}", runtime.intents);
    };
    assert_eq!(*action, InputAction::Tap { x, y });
    assert_eq!(sampling.action_seed(), SEED);
    assert_eq!(sampling.source_regions(), &[region]);
    assert_eq!(
        sampling.algorithm(),
        InputSamplingAlgorithm::CenterHotspotLeastRecentV1
    );
    assert_eq!(
        sampling.parameters(),
        Some(&InputSamplingParameters {
            sigma_ratio_milli: 250,
            max_redraws: 8,
            candidate_count: Some(3),
            chosen_candidate: Some(2),
        })
    );
}

// T1-4 (exec key 139; design §2.2 "gesture"): each gesture of the table is input as the action its
// click declares, and the effect intent carries that same action. Without an action seed a box
// point is its box's centre: the vertical-brake drag runs from the centre of `from_rect` to the
// centre of `corner_rect` and brakes up by its distance; the long press holds the centre of its box
// for its duration; the path drag stays in its box with `turns + 2` points, one duration per
// segment, the durations summing to its own.
#[test]
fn b3_s1_t1_4_brake_drag_path_drag_and_long_press_are_input_as_declared() {
    let gesture = |id: &str, click: Value| {
        json!({"id": id, "from": "home", "to": "terminal", "click": click,
               "unguarded_trusted_coordinate": true})
    };
    let operations = || {
        vec![
            gesture(
                "pull",
                json!({"kind": "single_touch_drag_with_vertical_brake_v1",
                       "from_rect": {"x": 20, "y": 150, "width": 40, "height": 40},
                       "corner_rect": {"x": 200, "y": 150, "width": 40, "height": 40},
                       "horizontal_duration_ms": 200, "corner_hold_ms": 150,
                       "brake_distance_px": 100, "brake_duration_ms": 200}),
            ),
            gesture(
                "trace",
                json!({"kind": "single_touch_path_drag_v1",
                       "box_rect": {"x": 40, "y": 40, "width": 200, "height": 120},
                       "turns": 3, "min_segment_px": 20, "duration_ms": 800,
                       "corner_hold_ms": 50}),
            ),
            gesture(
                "hold",
                json!({"kind": "long_press", "x": 100, "y": 100, "width": 40, "height": 20,
                       "duration_ms": 600}),
            ),
        ]
    };
    let run = |id: &str| {
        let task = wide_task(operations());
        let mut runtime = LabRuntime::new(vec![vec![wide_screen(RED)], vec![wide_screen(BLUE)]]);
        let page = confirmed(task.run_declared_operation(&mut runtime, id));
        assert_eq!(page, TERMINAL, "{id}");
        let inputs = runtime.inputs();
        let [input] = inputs.as_slice() else {
            panic!("{id}: one input, got {inputs:?}");
        };
        let intended = runtime
            .intents
            .iter()
            .map(|(action, _)| action.clone())
            .collect::<Vec<_>>();
        assert_eq!(intended, vec![input.clone()], "{id}: the intent's action");
        input.clone()
    };

    assert_eq!(
        run("pull"),
        InputAction::SingleTouchDragWithVerticalBrakeV1 {
            x1: 40,
            y1: 170,
            x2: 220,
            y2: 170,
            x3: 220,
            y3: 70,
            horizontal_duration_ms: actingcommand_contract::SEGMENTED_SWIPE_HORIZONTAL_DURATION_MS,
            corner_hold_ms: actingcommand_contract::SEGMENTED_SWIPE_CORNER_HOLD_MS,
            brake_distance_px: actingcommand_contract::SEGMENTED_SWIPE_BRAKE_DISTANCE_PX,
            brake_duration_ms: actingcommand_contract::SEGMENTED_SWIPE_BRAKE_DURATION_MS,
            slope_in: actingcommand_contract::SEGMENTED_SWIPE_SLOPE_IN,
            slope_out: actingcommand_contract::SEGMENTED_SWIPE_SLOPE_OUT,
        }
    );
    assert_eq!(
        run("hold"),
        InputAction::LongTap {
            x: 120,
            y: 110,
            duration_ms: 600
        }
    );
    match run("trace") {
        InputAction::SingleTouchPathDragV1 {
            box_x,
            box_y,
            box_width,
            box_height,
            points,
            segment_duration_ms,
            corner_hold_ms,
        } => {
            assert_eq!((box_x, box_y, box_width, box_height), (40, 40, 200, 120));
            assert_eq!(corner_hold_ms, 50);
            assert_eq!(points.len(), 5, "turns + 2 points");
            assert!(
                points
                    .iter()
                    .all(|point| (40..240).contains(&point.x) && (40..160).contains(&point.y)),
                "every point in the box: {points:?}"
            );
            assert_eq!(segment_duration_ms.len(), 4, "one duration per segment");
            assert_eq!(segment_duration_ms.iter().sum::<u64>(), 800);
        }
        other => panic!("trace: a path drag, got {other:?}"),
    }
}

// T1-5 (exec key 140; design §2.2 "`post_delay`" and "arrival"): on the runtime's clock, the first
// confirmation frame is captured `post_delay_ms` after the input, and each next one an
// `expect_after.interval_ms` after the one before, until the destination shows.
#[test]
fn b3_s1_t1_5_the_confirmation_waits_post_delay_then_the_declared_interval() {
    const POST_DELAY: Duration = Duration::from_millis(300);
    const INTERVAL: Duration = Duration::from_millis(700);
    let task = flag_task(
        vec![json!({
            "id": "open",
            "from": "home",
            "to": "terminal",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true,
            "post_delay_ms": 300,
            "expect_after": {"page_id": "terminal", "timeout_ms": 5_000, "interval_ms": 700}
        })],
        json!({}),
    );
    let mut runtime = LabRuntime::new(vec![
        vec![flag_screen(RED, GREEN)],
        vec![
            flag_screen(RED, GREEN),
            flag_screen(RED, GREEN),
            flag_screen(BLUE, GREEN),
        ],
    ]);

    let page = confirmed(task.run_declared_operation(&mut runtime, "open"));

    assert_eq!(page, TERMINAL);
    assert_eq!(
        runtime.after_first_input(),
        vec![
            Seen::Pause(POST_DELAY),
            Seen::Capture,
            Seen::Pause(INTERVAL),
            Seen::Capture,
            Seen::Pause(INTERVAL),
            Seen::Capture,
            Seen::StepFinished("open".to_owned(), TERMINAL.to_owned()),
        ]
    );
}

// T1-6 (exec key 141; design §2.2 "retries", "every attempt is recorded"): with `max_attempts` 2,
// a confirmation that fails after the first input is followed by the retry delay, the step's
// second attempt is guarded again on a frame captured after that delay, its input is made, and the
// run returns `Confirmed` on the destination.
#[test]
fn b3_s1_t1_6_a_failed_confirmation_is_retried_with_a_fresh_guard_and_a_second_input() {
    const RETRY_DELAY: Duration = Duration::from_millis(300);
    const INTERVAL: Duration = Duration::from_millis(500);
    let task = flag_task(
        vec![claim(json!({
            "retryable": true,
            "max_attempts": 2,
            "retry_interval_ms": 300,
            "expect_after": {"page_id": "terminal", "timeout_ms": 1_000, "interval_ms": 500}
        }))],
        json!({}),
    );
    let mut runtime = LabRuntime::new(vec![
        vec![flag_screen(RED, GREEN)],
        vec![flag_screen(RED, GREEN)],
        vec![flag_screen(BLUE, GREEN)],
    ]);

    let page = confirmed(task.run_declared_operation(&mut runtime, "claim"));

    assert_eq!(page, TERMINAL);
    let press = || Seen::Input(InputAction::Tap { x: 1, y: 0 });
    // What happened, without the confirmation's frames and the waits between them.
    let milestones = runtime
        .seen
        .iter()
        .filter(|seen| !matches!(seen, Seen::Capture) && **seen != Seen::Pause(INTERVAL))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        milestones,
        vec![
            Seen::StepStarted("claim".to_owned(), 0),
            Seen::Guard(ContainedTaskGuardEvaluation::Evaluated),
            Seen::Intent("claim".to_owned()),
            press(),
            Seen::Pause(RETRY_DELAY),
            Seen::StepFinished("claim".to_owned(), HOME.to_owned()),
            Seen::StepStarted("claim".to_owned(), 0),
            Seen::Guard(ContainedTaskGuardEvaluation::Evaluated),
            Seen::Intent("claim".to_owned()),
            press(),
            Seen::StepFinished("claim".to_owned(), TERMINAL.to_owned()),
        ]
    );
    // The second attempt's guard judges a frame captured after the retry delay.
    let delay = runtime
        .seen
        .iter()
        .position(|seen| *seen == Seen::Pause(RETRY_DELAY))
        .expect("the retry delay");
    let second = runtime
        .seen
        .iter()
        .rposition(|seen| matches!(seen, Seen::StepStarted(..)))
        .expect("the second attempt");
    assert!(
        runtime.seen[delay..second].contains(&Seen::Capture),
        "a frame between the delay and the second attempt: {:?}",
        runtime.seen
    );
}

// T1-7 (exec key 142; design §2.1 points 1, 3 and 5, §2.5): a package that reads marks, declares
// stability termination and a designated set whose cap is a fact: the declared operation's run
// records no reading and no stability trace, and asks no `selection_state` (it reads no fact).
#[test]
fn b3_s1_t1_7_readings_stability_and_designated_caps_stay_out_of_the_run() {
    const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let stability = json!({
        "region": {"x": 1, "y": 0, "width": 1, "height": 1},
        "comparison": {"mode": "exact_pixels_v1", "parameters": {}},
        "consecutive_unchanged_threshold": 2,
        "max_steps": 10
    });
    let mut control = control("in_page_guard", 2, 1);
    control.max_steps = Some(10);
    control.stability_termination =
        Some(serde_json::from_value(stability.clone()).expect("stability termination"));
    let program = program(
        2,
        1,
        vec![json!({
            "id": "collect",
            "from": "home",
            "to": "home",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true
        })],
        json!({
            "target_page": null,
            "stability_termination": stability,
            "scheduling_outcome": {
                "designated_effects": [
                    {"operation_id": "collect",
                     "max_repeats": {"fact_key": "session.example.collect.remaining"}}
                ],
                "mappings": [
                    {"outcome_key": "collected", "effect": "designated_effect_completed",
                     "terminal_pages": ["home"]},
                    {"outcome_key": "nothing", "effect": "no_designated_effect",
                     "terminal_pages": ["home"]}
                ]
            },
            "resource_readings": [{
                "id": "count", "fact_key": "resource.count", "page_id": "home",
                "target_id": "count/value", "trim": "whitespace_v1",
                "value": {"type": "unsigned_integer", "min": 0, "max": 999},
                "minimum_confidence_milli": 900, "valid_for_ms": 60_000
            }]
        }),
    );
    let pack = load_pack_from_json_str(
        &pack_json(
            2,
            1,
            vec![json!({
                "type": "ocr", "id": "count/value",
                "region": {"x": 1, "y": 0, "width": 1, "height": 1},
                "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact", "expected": ["0"],
                "case_sensitive": false, "minimum_confidence": 0.9,
                "model_ref": "PP-OCRv6_medium", "model_sha256": MODEL_SHA256
            })],
        )
        .to_string(),
    )
    .expect("recognition pack");
    let task = prepared(
        control,
        program,
        RecognitionEvaluator::new(PathBuf::new(), pack).expect("evaluator"),
    );
    let mut runtime = LabRuntime::new(vec![vec![flag_screen(RED, GREEN)]]);

    let page = confirmed(task.run_declared_operation(&mut runtime, "collect"));

    assert_eq!(page, HOME);
    assert_eq!(runtime.inputs(), vec![InputAction::Tap { x: 1, y: 0 }]);
    let facts = runtime
        .seen
        .iter()
        .filter(|seen| matches!(seen, Seen::Fact(_)))
        .collect::<Vec<_>>();
    assert!(facts.is_empty(), "no reading or stability trace: {facts:?}");
    assert_eq!(runtime.fact_reads, 0, "no selection_state");
}

// T1-8 (exec key 143; design §1.1 "Refused by the kernel before any lease", §4, decision Q2):
// admission of the declared operation. An id the entry task does not declare is
// `contained_task_declared_operation_unknown`; an application effect, a select step, an operation
// whose `expect_after` reads an earlier select step's choice, and any operation of a
// `linear_steps` or `recognize_only` package are `contained_task_declared_operation_unsupported`
// with their `lab_operation_problem`. A page-graph click is admitted and stated as the prepared
// record (LR-10) states it: its stage, its `from` as declared, its destination, and its attempts
// (1 without a retry policy).
#[test]
fn b3_s1_t1_8_admission_refuses_unknown_ids_and_unsupported_operations() {
    let guard = json!({"page_id": "home", "target_id": "flag/ready",
                       "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                       "color_probe": "flag/ready"});
    let operations = || {
        vec![
            claim(json!({})),
            json!({"id": "launch", "from": "home", "to": "terminal",
                   "application": {"action": "restart"}}),
            json!({"id": "choose", "from": "home", "to": "terminal",
                   "select": {"layout_id": "choices", "policy": {"path": "policies/select.json"}},
                   "guard": guard.clone(), "expect_after": {"page_id": "terminal"}}),
            json!({"id": "confirm", "from": "home",
                   "click": {"kind": "point", "x": 0, "y": 0},
                   "unguarded_trusted_coordinate": true,
                   "expect_after": {"page_id": "terminal",
                                    "expect_identity": {"target_id": "flag/ready",
                                                        "choice_of": "choose"}}}),
        ]
    };
    let task = |mode: &str| {
        let mut task = flag_task(operations(), json!({}));
        task.control = control(mode, 2, 1);
        task
    };
    let unknown = ExecutionKernelCode::ContainedTaskDeclaredOperationUnknown.as_str();
    let unsupported = ExecutionKernelCode::ContainedTaskDeclaredOperationUnsupported.as_str();
    let refused = |problem: &str, id: &str| {
        (
            unsupported,
            json!({"pack_operation_id": id, "lab_operation_problem": problem}),
        )
    };
    for (mode, id, (code, expected)) in [
        (
            "navigable_route",
            "missing",
            (unknown, json!({"pack_operation_id": "missing"})),
        ),
        (
            "navigable_route",
            "launch",
            refused("application_effect", "launch"),
        ),
        (
            "navigable_route",
            "choose",
            refused("select_step", "choose"),
        ),
        (
            "navigable_route",
            "confirm",
            refused("chosen_identity", "confirm"),
        ),
        ("linear_steps", "claim", refused("linear_steps", "claim")),
        (
            "recognize_only",
            "claim",
            refused("recognize_only", "claim"),
        ),
    ] {
        let error = task(mode)
            .declared_operation(id)
            .err()
            .unwrap_or_else(|| panic!("{mode} {id}: admission refuses it"));
        assert_eq!(error.code().as_str(), code, "{mode} {id}");
        assert_eq!(values(&error), expected, "{mode} {id}");
    }

    assert_eq!(
        task("navigable_route")
            .declared_operation("claim")
            .expect("a page-graph click is admitted"),
        DeclaredOperation {
            phase_id: None,
            from_page: "home".to_owned(),
            destination_pages: vec!["terminal".to_owned()],
            max_attempts: 1,
        }
    );
    let mut staged = flag_task(
        vec![claim(
            json!({"retryable": true, "max_attempts": 3, "retry_interval_ms": 100}),
        )],
        json!({}),
    );
    staged.program.phases = Some(vec![TaskPhase {
        id: "stage".to_owned(),
        operations: vec!["claim".to_owned()],
        target_pages: vec!["terminal".to_owned()],
        end_pages: None,
        entry_pages: None,
    }]);
    assert_eq!(
        staged
            .declared_operation("claim")
            .expect("a staged click with retries is admitted"),
        DeclaredOperation {
            phase_id: Some("stage".to_owned()),
            from_page: "home".to_owned(),
            destination_pages: vec!["terminal".to_owned()],
            max_attempts: 3,
        }
    );
}

// --- T1-9: a template guard on screens whose mark moves while a strip of the screen stays. ---

const MARKED: (u32, u32) = (64, 48);
const MARK: u32 = 12;

#[derive(Debug)]
struct MarkAsset(Vec<u8>);

impl AssetResolver for MarkAsset {
    fn read_asset(&self, path: &str) -> RecognitionPackResult<Vec<u8>> {
        if path == "mark.png" {
            Ok(self.0.clone())
        } else {
            Err(actingcommand_recognition_pack::RecognitionPackError::fatal(
                "no such fixture asset",
            ))
        }
    }
}

// A neutral 12 × 12 pattern; no pixel is black.
fn mark_pixel(x: u32, y: u32) -> [u8; 3] {
    [
        ((x * 37 + y * 11) % 200 + 40) as u8,
        ((x * 13 + y * 41 + 7) % 200 + 40) as u8,
        ((x * 29 + y * 17 + 3) % 200 + 40) as u8,
    ]
}

fn mark_png() -> Vec<u8> {
    let pixels = (0..MARK)
        .flat_map(|y| (0..MARK).flat_map(move |x| mark_pixel(x, y)))
        .collect::<Vec<_>>();
    Frame::from_pixels(
        MARK,
        MARK,
        pixels,
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("mark frame")
    .encode_png_fast()
    .expect("mark PNG")
}

// `page` on the first pixel and the mark with its top-left corner at `(left, top)`; black
// elsewhere, the bottom strip included.
fn marked_screen(page: [u8; 3], (left, top): (u32, u32)) -> ScriptedFrame {
    let (width, height) = MARKED;
    let mut pixels = vec![BLACK; (width * height) as usize];
    pixels[0] = page;
    for y in 0..MARK {
        for x in 0..MARK {
            pixels[((top + y) * width + left + x) as usize] = mark_pixel(x, y);
        }
    }
    ScriptedFrame::rgb(width, height, &pixels)
}

// T1-9 (exec key 180; design §2.2 "input recheck", BT-B12 `still_frames`): a click on its guard's
// template with the recheck `still_frames` (2 frames, the bottom strip) is drawn from and bound to
// the last frame that held: the mark moved between the step's frame and that frame while the strip
// stayed still, and the press lands on the mark's centre in the last frame.
#[test]
fn b3_s1_t1_9_the_input_binds_to_the_last_still_frame() {
    let (width, height) = MARKED;
    let pack = load_pack_from_json_str(
        &pack_json(
            width,
            height,
            vec![
                json!({"type": "template", "id": "mark/target", "template_path": "mark.png",
                        "region": "full_frame", "threshold": 0.9}),
            ],
        )
        .to_string(),
    )
    .expect("recognition pack");
    let evaluator =
        RecognitionEvaluator::with_asset_resolver(pack, Arc::new(MarkAsset(mark_png())))
            .expect("evaluator");
    let task = prepared(
        control("navigable_route", width, height),
        program(
            width,
            height,
            vec![json!({
                "id": "open",
                "from": "home",
                "to": "terminal",
                "click": {"kind": "target", "target_id": "mark/target"},
                "guard": {"page_id": "home", "target_id": "mark/target",
                          "expected_rect": {"x": 0, "y": 0, "width": width, "height": height},
                          "verify_template": "mark/target"},
                "input_recheck": {"kind": "still_frames", "frames": 2,
                                  "region": {"x": 0, "y": 40, "width": width, "height": 8}}
            })],
            json!({}),
        ),
        evaluator,
    );
    // The step's frame shows the mark at (40, 6), the recheck's frame at (12, 22).
    let mut runtime = LabRuntime::new(vec![
        vec![marked_screen(RED, (40, 6)), marked_screen(RED, (12, 22))],
        vec![marked_screen(BLUE, (12, 22))],
    ]);

    let page = confirmed(task.run_declared_operation(&mut runtime, "open"));

    assert_eq!(page, TERMINAL);
    // The mark's centre in the last still frame: (12 + 12 / 2, 22 + 12 / 2).
    assert_eq!(runtime.inputs(), vec![InputAction::Tap { x: 18, y: 28 }]);
    let before_input = runtime
        .seen
        .iter()
        .take_while(|seen| !matches!(seen, Seen::Input(_)))
        .filter(|seen| matches!(seen, Seen::Capture | Seen::Guard(_)))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        before_input,
        vec![
            Seen::Capture,
            Seen::Guard(ContainedTaskGuardEvaluation::Evaluated),
            Seen::Capture,
            Seen::Guard(ContainedTaskGuardEvaluation::Evaluated),
        ],
        "the step's frame and one recheck frame, each guarded"
    );
}
