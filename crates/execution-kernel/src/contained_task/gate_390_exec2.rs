// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #390 EXEC-2 (forced-compromises mapping rows E-05 and E-06; FCM §2.4 row EXEC-2). An
// operation may declare an observation window of at most 60 s after its destination passed:
// until one destination held on a number of frames, or for the whole window so that a late popup
// is seen, a frame without a destination going to the shared interrupt pages (NAV-6). A click may
// declare a recheck before its input: one fresh frame, or fresh frames until its decision held on
// a number of consecutive frames, at the place of SR-1's one sampling deadline check before the
// effect intent, with SR-1's observe-only retries. The post-input delay keeps its 5 s bound.
//
// The observation clock is the runtime's (`observation_now`): the fake below moves it by `tick`
// at every capture, and by scripted jumps, so a window ends exactly where a test says. Every
// expected value comes from the FCM rows and the declaration rules of the contract.

use super::*;
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition_pack::RecognitionPack;
use serde_json::{Value, json};
use std::path::PathBuf;

const STEP_TIMEOUT: Duration = Duration::from_secs(10);
const TASK_TIMEOUT: Duration = Duration::from_secs(60);
const TICK: Duration = Duration::from_secs(1);
// One move of the observation clock by more than a step's window ends the window in progress.
const PAST_WINDOW: Duration = Duration::from_secs(11);

// What the screen shows: the left pixel names the page (none for `Blank`), the right pixel is
// the guard's colour, ready (green) except on `HomeBusy`.
#[derive(Debug, Clone, Copy)]
enum Screen {
    Home,
    HomeBusy,
    Terminal,
    Popup,
    Blank,
}

fn frame(screen: Screen) -> Frame {
    let (page, guard) = match screen {
        Screen::Home => ([255, 0, 0], [0, 255, 0]),
        Screen::HomeBusy => ([255, 0, 0], [128, 128, 128]),
        Screen::Terminal => ([0, 0, 255], [0, 255, 0]),
        Screen::Popup => ([255, 255, 0], [0, 255, 0]),
        Screen::Blank => ([0, 0, 0], [0, 255, 0]),
    };
    Frame::from_pixels(
        2,
        1,
        [page, guard].concat(),
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
    Interrupt,
    StepFinished(String),
}

// The Runtime as the kernel sees it: every capture shows the next screen of the script of the
// inputs made so far (its last screen repeats) and moves the observation clock by `tick`; it
// samples, gives `retries` observe-only retries per step, and handles every frame offered to its
// shared interrupt pages.
struct ScriptedRuntime {
    scripts: Vec<Vec<Screen>>,
    shown: Vec<usize>,
    tick: Duration,
    retries: u32,
    origin: Option<Instant>,
    offset: Duration,
    capture_jumps: BTreeMap<usize, Duration>,
    captures: usize,
    inputs: usize,
    events: Vec<Event>,
}

impl ScriptedRuntime {
    fn new(scripts: Vec<Vec<Screen>>) -> Self {
        Self {
            shown: vec![0; scripts.len()],
            scripts,
            tick: TICK,
            retries: 0,
            origin: None,
            offset: Duration::ZERO,
            capture_jumps: BTreeMap::new(),
            captures: 0,
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

    // The pages the steps finished on.
    fn finished(&self) -> Vec<String> {
        self.events
            .iter()
            .filter_map(|event| match event {
                Event::StepFinished(page) => Some(page.clone()),
                _ => None,
            })
            .collect()
    }

    // Captures made after the first input and before the second.
    fn captures_after_first_input(&self) -> usize {
        self.events
            .iter()
            .skip_while(|event| **event != Event::Input)
            .skip(1)
            .take_while(|event| **event != Event::Input)
            .filter(|event| **event == Event::Capture)
            .count()
    }
}

impl ContainedTaskRuntime for ScriptedRuntime {
    type Error = &'static str;

    fn observe_task_timing(&mut self, context: ContainedTaskTimingContext) {
        if self.origin.is_none() {
            self.origin = Some(context.deadline() - TASK_TIMEOUT);
        }
    }

    fn sampling_supported(&self) -> bool {
        true
    }

    fn step_pacing(&mut self) -> Result<StepPacing, Self::Error> {
        Ok(StepPacing::new(self.retries))
    }

    fn observation_now(&mut self) -> Instant {
        self.origin
            .expect("the run gives its timing before it observes")
            + self.offset
    }

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        self.captures += 1;
        self.offset += self.tick;
        if let Some(jump) = self.capture_jumps.remove(&self.captures) {
            self.offset += jump;
        }
        let script = &self.scripts[self.inputs];
        let shown = &mut self.shown[self.inputs];
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
            ContainedTaskTrace::StepFinished { page_label, .. } => {
                self.events.push(Event::StepFinished(page_label));
            }
            _ => {}
        }
        Ok(())
    }

    fn shared_interrupt(
        &mut self,
        _frame: SharedInterruptFrame<'_>,
    ) -> Result<SharedInterruptVerdict, ContainedTaskRunError<Self::Error>> {
        self.events.push(Event::Interrupt);
        Ok(SharedInterruptVerdict::Handled)
    }
}

fn control_json() -> Value {
    json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.window.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": STEP_TIMEOUT.as_millis() as u64,
        "timeout_ms": TASK_TIMEOUT.as_millis() as u64
    })
}

fn control() -> TaskControl {
    serde_json::from_value(control_json()).expect("task control")
}

// The three pages and the guard; with `consensus`, page `home` needs two frames 1 ms apart.
fn pack(consensus: bool) -> RecognitionPack {
    let mut pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            {"type": "color", "id": "page/home",
             "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [255, 0, 0]},
            {"type": "color", "id": "page/terminal",
             "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [0, 0, 255]},
            {"type": "color", "id": "page/popup",
             "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [255, 255, 0]},
            {"type": "color", "id": "guard/ready",
             "region": {"x": 1, "y": 0, "width": 1, "height": 1}, "expected": [0, 255, 0]}
        ]
    });
    if consensus {
        pack["target_consensus"] = json!({
            "page/home": {"samples": [{"frame": 0}, {"frame": 1}], "k": 2,
                          "sample_interval_ms": 1}
        });
    }
    serde_json::from_value(pack).expect("recognition pack")
}

fn page_set() -> PageSet {
    serde_json::from_value(json!({
        "schema_version": "0.3",
        "pages": [
            {"id": "neutral/home", "required": ["page/home"]},
            {"id": "neutral/terminal", "required": ["page/terminal"]},
            {"id": "neutral/popup", "required": ["page/popup"]}
        ]
    }))
    .expect("page set")
}

// The guarded click from `home`, with `extra` members.
fn open(extra: Value) -> Value {
    let mut operation = json!({
        "id": "open",
        "from": "home",
        "to": "terminal",
        "click": {"kind": "point", "x": 1, "y": 0},
        "guard": {"page_id": "home", "target_id": "guard/ready",
                  "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                  "color_probe": "guard/ready"}
    });
    for (member, value) in extra.as_object().expect("extra members") {
        operation[member] = value.clone();
    }
    operation
}

// The package's own handling of the late popup: a trusted click back to `terminal`.
fn close_popup() -> Value {
    json!({
        "id": "close_popup",
        "from": "popup",
        "to": "terminal",
        "click": {"kind": "point", "x": 0, "y": 0},
        "unguarded_trusted_coordinate": true
    })
}

fn task(operations: Vec<Value>, consensus: bool) -> PreparedContainedTask {
    let control = control();
    let program: TaskProgram = serde_json::from_value(json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "target_page": "terminal",
        "operations": operations
    }))
    .expect("task program");
    for operation in &program.operations {
        operation
            .validate(&control, TaskOperationDefaults::default(), "0.9")
            .expect("valid operation");
    }
    let evaluator =
        RecognitionEvaluator::new(PathBuf::new(), pack(consensus)).expect("recognition evaluator");
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
        package_sha256: "fixture-sha256".into(),
        entry_count: 1,
        task_count: 1,
    }
}

fn task_failure(
    result: Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>,
) -> ContainedTaskError {
    match result {
        Err(ContainedTaskRunError::Task(error)) => error,
        Err(other) => panic!("expected a task failure, got {other:?}"),
        Ok(outcome) => panic!(
            "expected a task failure, the run ended {:?}",
            outcome.outcome
        ),
    }
}

fn succeeded(result: Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>) {
    match result {
        Ok(outcome) => assert_eq!(outcome.outcome, TaskOutcome::Success),
        Err(error) => panic!("expected the run to succeed, got {error:?}"),
    }
}

fn values(error: &ContainedTaskError) -> Value {
    Value::Object(error.outcome().values().clone())
}

fn refused_code(result: Result<(), ContainedTaskError>) -> String {
    result
        .expect_err("admission refuses the operation")
        .code()
        .as_str()
        .to_owned()
}

// FCM E-05 (window ≤ 60 s; the post-input delay bound unchanged), E-06 (a declared recheck on a
// click), §2.4 (page-graph declarations): what admission takes and refuses.
#[test]
fn exec2_admission_bounds_the_window_at_60_s_and_keeps_the_post_delay_bound() {
    let control = control();
    let defaults = TaskOperationDefaults::default();
    let invalid = ExecutionKernelCode::ContainedTaskOperationInvalid.as_str();
    let check = |operation: Value, schema: &str| {
        serde_json::from_value::<TaskOperation>(operation)
            .expect("operation")
            .validate(&control, defaults, schema)
    };
    let window = |value: Value| open(json!({"observation_window": value}));
    let recheck = |value: Value| open(json!({"input_recheck": value}));

    for admitted in [
        window(json!({"until": "window_end", "window_ms": 60_000})),
        window(json!({"until": "stable", "window_ms": 1_000, "frames": 3})),
        recheck(json!({"kind": "fresh_frame"})),
        recheck(json!({"kind": "stable_frames", "frames": 3})),
        open(json!({"post_delay_ms": 5_000})),
    ] {
        check(admitted.clone(), "0.9").unwrap_or_else(|error| panic!("{admitted}: {error}"));
    }
    for refused in [
        window(json!({"until": "window_end", "window_ms": 60_001})),
        window(json!({"until": "window_end", "window_ms": 0})),
        window(json!({"until": "stable", "window_ms": 1_000, "frames": 1})),
        window(json!({"until": "stable", "window_ms": 1_000, "frames": 33})),
        // Two intervals of 600 ms do not fit in a window of 1 s.
        open(
            json!({"observation_window": {"until": "stable", "window_ms": 1_000, "frames": 3},
                    "expect_after": {"page_id": "terminal", "interval_ms": 600}}),
        ),
        // A window watches a destination; an operation without one has none.
        json!({"id": "tap", "from": "home", "click": {"kind": "point", "x": 0, "y": 0},
               "unguarded_trusted_coordinate": true,
               "observation_window": {"until": "window_end", "window_ms": 1_000}}),
        recheck(json!({"kind": "stable_frames", "frames": 1})),
        recheck(json!({"kind": "stable_frames", "frames": 33})),
        // A recheck checks a click; an application effect has none.
        json!({"id": "restart", "from": "home", "to": "terminal",
               "application": {"action": "restart"},
               "input_recheck": {"kind": "fresh_frame"}}),
        open(json!({"post_delay_ms": 5_001})),
    ] {
        assert_eq!(
            refused_code(check(refused.clone(), "0.9")),
            invalid,
            "{refused}"
        );
    }
    // An older task schema refuses both declarations.
    for older in [
        window(json!({"until": "window_end", "window_ms": 1_000})),
        recheck(json!({"kind": "fresh_frame"})),
    ] {
        assert_eq!(
            refused_code(check(older.clone(), "0.8")),
            invalid,
            "{older}"
        );
    }
    // Three frames one capture interval of 5 s apart fit in a step timeout of 10 s; four do not.
    let mut slow = control_json();
    slow["capture_interval_ms"] = json!(5_000);
    let slow: TaskControl = serde_json::from_value(slow).expect("slow control");
    let slow_check = |frames: u32| {
        serde_json::from_value::<TaskOperation>(recheck(
            json!({"kind": "stable_frames", "frames": frames}),
        ))
        .expect("operation")
        .validate(&slow, defaults, "0.9")
    };
    slow_check(3).expect("three frames fit");
    assert_eq!(refused_code(slow_check(4)), invalid);
}

// FCM §2.4 row EXEC-2: both declarations belong to page-graph tasks; a linear task refuses them.
#[test]
fn exec2_a_linear_task_refuses_the_window_and_the_recheck() {
    let mut linear = control_json();
    linear["execution_mode"] = json!("linear_steps");
    linear["max_steps"] = json!(1);
    let linear: TaskControl = serde_json::from_value(linear).expect("linear control");
    let detector = PageDetector::new(page_set()).expect("page detector");
    for (member, value) in [
        (
            "observation_window",
            json!({"until": "window_end", "window_ms": 1_000}),
        ),
        ("input_recheck", json!({"kind": "fresh_frame"})),
    ] {
        let mut operation = open(json!({}));
        operation[member] = value;
        let program: TaskProgram = serde_json::from_value(json!({
            "schema_version": "0.9",
            "task_id": "task",
            "game": "neutral",
            "server_scope": ["test"],
            "coordinate_space": {"width": 2, "height": 1},
            "entry_page": "home",
            "target_page": "terminal",
            "max_steps": 1,
            "operations": [operation]
        }))
        .expect("linear program");
        let error = program
            .validate_linear(&linear, &detector)
            .err()
            .expect("a linear task refuses the declaration");
        assert_eq!(
            error.code().as_str(),
            ExecutionKernelCode::ContainedTaskLinearInvalid.as_str()
        );
        assert_eq!(
            error.detail(),
            Some(format!("reason={member} operation=open").as_str())
        );
    }
}

// FCM E-05 「等迟到的弹层出现」: a popup that is a destination of the operation and appears two
// seconds after the arrival, inside a window of 5 s, is where the step arrives; the package then
// handles it with its own operation.
#[test]
fn exec2_a_late_popup_inside_the_window_is_where_the_step_arrives() {
    let task = task(
        vec![
            open(json!({"to": ["terminal", "popup"],
                        "observation_window": {"until": "window_end", "window_ms": 5_000}})),
            close_popup(),
        ],
        false,
    );
    let mut runtime = ScriptedRuntime::new(vec![
        vec![Screen::Home],
        vec![Screen::Terminal, Screen::Terminal, Screen::Popup],
        vec![Screen::Terminal],
    ]);
    succeeded(task.run(&mut runtime));
    assert_eq!(runtime.inputs, 2);
    assert_eq!(
        runtime.finished(),
        vec!["neutral/popup".to_owned(), "neutral/terminal".to_owned()]
    );
    // The arrival, one more frame of `terminal`, then the popup ends the window.
    assert_eq!(runtime.captures_after_first_input(), 3);
}

// FCM E-05 「观察 N 秒」 and 「交给 NAV-6」: without a popup the step finishes on its destination at
// the window's end (3 s, one frame a second after the arrival); a frame with no page of the
// package is offered to the shared interrupt pages and, once handled, the window goes on.
#[test]
fn exec2_the_window_watches_to_its_end_and_hands_a_frame_without_a_page_to_the_interrupts() {
    let window = || {
        task(
            vec![open(
                json!({"observation_window": {"until": "window_end", "window_ms": 3_000}}),
            )],
            false,
        )
    };
    let mut runtime = ScriptedRuntime::new(vec![vec![Screen::Home], vec![Screen::Terminal]]);
    succeeded(window().run(&mut runtime));
    assert_eq!(runtime.captures_after_first_input(), 4);
    assert_eq!(runtime.finished(), vec!["neutral/terminal".to_owned()]);
    assert_eq!(runtime.count(&Event::Interrupt), 0);

    let mut runtime = ScriptedRuntime::new(vec![
        vec![Screen::Home],
        vec![Screen::Terminal, Screen::Blank, Screen::Terminal],
    ]);
    succeeded(window().run(&mut runtime));
    assert_eq!(runtime.count(&Event::Interrupt), 1);
    assert_eq!(runtime.inputs, 1);
    assert_eq!(runtime.finished(), vec!["neutral/terminal".to_owned()]);
    assert!(runtime.captures_after_first_input() >= 4);
}

// FCM E-05 「等画面稳定」: a stable window of three frames ends on the frame that makes three
// consecutive frames of one destination; a screen that never settles within the window ends the
// run with `contained_task_observation_stability_unconfirmed`, its operation and its window, and
// no further input.
#[test]
fn exec2_a_stable_window_ends_once_one_destination_held_and_fails_when_it_never_does() {
    let stable = || {
        task(
            vec![
                open(json!({"to": ["terminal", "popup"],
                            "observation_window": {"until": "stable", "window_ms": 5_000,
                                                   "frames": 3}})),
                close_popup(),
            ],
            false,
        )
    };
    let mut runtime = ScriptedRuntime::new(vec![
        vec![Screen::Home],
        vec![
            Screen::Terminal,
            Screen::Popup,
            Screen::Popup,
            Screen::Popup,
        ],
        vec![Screen::Terminal],
    ]);
    succeeded(stable().run(&mut runtime));
    assert_eq!(runtime.captures_after_first_input(), 4);
    assert_eq!(
        runtime.finished(),
        vec!["neutral/popup".to_owned(), "neutral/terminal".to_owned()]
    );

    let flapping = [
        Screen::Terminal,
        Screen::Popup,
        Screen::Terminal,
        Screen::Popup,
        Screen::Terminal,
        Screen::Popup,
    ];
    let mut runtime = ScriptedRuntime::new(vec![vec![Screen::Home], flapping.to_vec()]);
    let error = task_failure(stable().run(&mut runtime));
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::ContainedTaskObservationStabilityUnconfirmed.as_str()
    );
    assert_eq!(
        values(&error),
        json!({"pack_operation_id": "open", "timeout_ms": 5_000})
    );
    assert_eq!(runtime.inputs, 1);
}

// FCM E-06 「点击前取新帧复核」: the guard passes on the step's frame, then one fresh frame is
// captured and guarded before the effect intent, and the input follows it; a fresh frame on which
// the guard no longer passes refuses the input.
#[test]
fn exec2_a_fresh_frame_recheck_guards_a_new_frame_before_the_intent() {
    let fresh = || {
        task(
            vec![open(json!({"input_recheck": {"kind": "fresh_frame"}}))],
            false,
        )
    };
    let mut runtime = ScriptedRuntime::new(vec![vec![Screen::Home], vec![Screen::Terminal]]);
    succeeded(fresh().run(&mut runtime));
    assert_eq!(
        runtime.before_first_input(),
        vec![
            Event::StepStarted,
            Event::Guard,
            Event::Capture,
            Event::Guard,
            Event::Intent,
            Event::Input,
        ]
    );

    let mut runtime = ScriptedRuntime::new(vec![vec![Screen::Home, Screen::HomeBusy]]);
    let error = task_failure(fresh().run(&mut runtime));
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::ContainedTaskGuardRefused.as_str()
    );
    assert_eq!(runtime.inputs, 0);
    assert_eq!(runtime.count(&Event::Intent), 0);
}

// FCM E-06 「或等 N 帧稳定」: with three stable frames the input waits until the guard passed on
// the step's page on three consecutive frames; a frame whose guard does not pass and a frame
// without a page restart the count. A screen that never holds within the step timeout refuses the
// input with `contained_task_input_stability_unconfirmed`, its operation and its window.
#[test]
fn exec2_stable_frames_wait_for_consecutive_passing_frames_and_refuse_when_they_never_come() {
    let stable = || {
        task(
            vec![open(
                json!({"input_recheck": {"kind": "stable_frames", "frames": 3}}),
            )],
            false,
        )
    };
    let mut runtime = ScriptedRuntime::new(vec![
        vec![
            Screen::Home,
            Screen::HomeBusy,
            Screen::Blank,
            Screen::Home,
            Screen::Home,
            Screen::Home,
        ],
        vec![Screen::Terminal],
    ]);
    succeeded(stable().run(&mut runtime));
    assert_eq!(
        runtime.before_first_input(),
        vec![
            Event::StepStarted,
            // The step's own frame passes: one.
            Event::Guard,
            // The guard does not pass: none.
            Event::Capture,
            Event::Guard,
            // No page: none.
            Event::Capture,
            // Three passing frames in a row.
            Event::Capture,
            Event::Guard,
            Event::Capture,
            Event::Guard,
            Event::Capture,
            Event::Guard,
            Event::Intent,
            Event::Input,
        ]
    );

    let alternating = (0..12)
        .map(|index| {
            if index % 2 == 0 {
                Screen::Home
            } else {
                Screen::HomeBusy
            }
        })
        .collect::<Vec<_>>();
    let mut runtime = ScriptedRuntime::new(vec![alternating]);
    let error = task_failure(stable().run(&mut runtime));
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::ContainedTaskInputStabilityUnconfirmed.as_str()
    );
    assert_eq!(
        values(&error),
        json!({"pack_operation_id": "open", "timeout_ms": 10_000})
    );
    assert_eq!(runtime.inputs, 0);
    assert_eq!(runtime.count(&Event::Intent), 0);
}

// FCM E-06 (the recheck sits where SR-1's one sampling deadline check before the intent is, and
// uses its seams): the fresh frame of a pack whose page needs two frames runs past its window;
// with one observe-only retry it is observed again in a new window and the click goes on, with
// none the run ends with SR-1's timeout, its attempts, window and stage, and no intent.
#[test]
fn exec2_a_recheck_frame_whose_sampling_timed_out_takes_the_observe_only_retry() {
    let sampled = || {
        task(
            vec![open(json!({"input_recheck": {"kind": "fresh_frame"}}))],
            true,
        )
    };
    let scripted = |retries: u32| {
        let mut runtime = ScriptedRuntime::new(vec![vec![Screen::Home], vec![Screen::Terminal]]);
        runtime.tick = Duration::ZERO;
        runtime.retries = retries;
        // Captures 1 and 2 are the step's frame; capture 3, the first sample of the fresh frame,
        // runs past its window.
        runtime.capture_jumps = BTreeMap::from([(3, PAST_WINDOW)]);
        runtime
    };
    let mut runtime = scripted(1);
    succeeded(sampled().run(&mut runtime));
    assert_eq!(runtime.inputs, 1);
    assert_eq!(runtime.count(&Event::Intent), 1);

    let mut runtime = scripted(0);
    let error = task_failure(sampled().run(&mut runtime));
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::RecognitionSampleDeadlineExceeded.as_str()
    );
    assert_eq!(
        values(&error),
        json!({"attempts": 0, "timeout_ms": 10_000, "stage": "observation_before_input"})
    );
    assert_eq!(runtime.inputs, 0);
    assert_eq!(runtime.count(&Event::Intent), 0);
}
