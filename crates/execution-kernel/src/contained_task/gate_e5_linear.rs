// SPDX-License-Identifier: AGPL-3.0-only

// Test plan v2.2 E-5 G5 (exec key 3): the `linear_steps` execution mode. Requirement sources:
// contracts/linear-steps.md, sections "Admission", "Execution", "Intermediate states", "Retries",
// "Optional steps", "Application steps", "Return-home fallback", "Failure codes" and "Ledger";
// contracts/package-reference.md:384-387 (which packages select the return-home fallback); the
// requirement-basis row rd-1009/tests/req-basis-el.md §E-5 G5.
//
// The device shows one page at a time: a frame's left pixel is the colour of its page (black on a
// screen that is no page of the package). Script `n` is what the device shows after `n` effects
// (inputs and application actions), one screen per capture, its last screen repeating. The
// runtime keeps the traces, its effects and every timing boundary the kernel hands it in one log,
// so the order of captures, recognitions, effects and waits is read from that log, never from a
// clock. A budget a test lets run out is tens of milliseconds; a budget no wait may reach is
// seconds.

use super::*;
use actingcommand_contract::{TaskTimingBoundary, TaskTimingBudgetOrigin};
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition_pack::RecognitionPack;
use serde_json::{Value, json};
use std::path::PathBuf;

const PACKAGE: &str = "neutral.test.linear";
const RETURN_HOME: &str = "neutral.test.return_home";
const LEAD_IN: &str = "neutral.test.lead_in";
// A step timeout that no wait of these runs reaches.
const STEP_TIMEOUT_MS: u64 = 5_000;
// The step timeout of the package whose first step never passes.
const ENTRY_MS: u64 = 30;
// An arrival or intermediate budget a test lets run out.
const SPENT_MS: u64 = 40;

#[derive(Debug, Clone, Copy)]
enum Screen {
    Home,
    List,
    Detail,
    Loading,
    Notice,
    Blank,
}

// The package's pages; `Blank` is none of them.
const PAGES: [Screen; 5] = [
    Screen::Home,
    Screen::List,
    Screen::Detail,
    Screen::Loading,
    Screen::Notice,
];

impl Screen {
    const fn name(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::List => "list",
            Self::Detail => "detail",
            Self::Loading => "loading",
            Self::Notice => "notice",
            Self::Blank => "blank",
        }
    }

    const fn colour(self) -> [u8; 3] {
        match self {
            Self::Home => [255, 0, 0],
            Self::List => [0, 0, 255],
            Self::Detail => [0, 255, 0],
            Self::Loading => [255, 255, 0],
            Self::Notice => [255, 0, 255],
            Self::Blank => [0, 0, 0],
        }
    }
}

fn frame(screen: Screen) -> Frame {
    Frame::from_pixels(
        2,
        1,
        [screen.colour(), [0, 0, 0]].concat(),
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
}

#[derive(Debug)]
enum Event {
    Admitted,
    RunStarted,
    Capture(Screen),
    // The candidate pages of a recognition.
    Recognition(Vec<String>),
    // `<step_index> <operation> <from_page>`.
    StepStarted(String),
    Intent(String),
    Input,
    Application(ApplicationLifecycleAction),
    EffectCompleted(String),
    // `<step_index> <operation> <page>`.
    StepFinished(String),
    Finalizing,
    Boundary {
        boundary: TaskTimingBoundary,
        started: Instant,
        ended: Instant,
    },
}

// The Runtime as the kernel sees it: it shows the scripted screens, moves to the next script at
// each effect, drives an assigned application only when it has one, and logs what the kernel
// hands it.
struct Device {
    scripts: Vec<Vec<Screen>>,
    shown: Vec<usize>,
    effects: usize,
    application: bool,
    progress: Vec<u32>,
    events: Vec<Event>,
}

impl Device {
    fn new(scripts: Vec<Vec<Screen>>) -> Self {
        Self {
            shown: vec![0; scripts.len()],
            scripts,
            effects: 0,
            application: false,
            progress: Vec::new(),
            events: Vec::new(),
        }
    }

    // A device whose instance has an assigned application.
    fn with_application(scripts: Vec<Vec<Screen>>) -> Self {
        Self {
            application: true,
            ..Self::new(scripts)
        }
    }

    fn screen(&mut self) -> Screen {
        let index = self.effects.min(self.scripts.len() - 1);
        let script = &self.scripts[index];
        let shown = &mut self.shown[index];
        let screen = script[(*shown).min(script.len() - 1)];
        *shown += 1;
        screen
    }

    // The log without the timing boundaries.
    fn log(&self) -> Vec<String> {
        self.events
            .iter()
            .filter_map(|event| match event {
                Event::Admitted => Some("admitted".to_owned()),
                Event::RunStarted => Some("run started".to_owned()),
                Event::Capture(screen) => Some(format!("capture {}", screen.name())),
                Event::Recognition(pages) => Some(format!("recognize {}", pages.join(","))),
                Event::StepStarted(step) => Some(format!("start {step}")),
                Event::Intent(operation) => Some(format!("intent {operation}")),
                Event::Input => Some("input".to_owned()),
                Event::Application(action) => Some(format!("application {action:?}")),
                Event::EffectCompleted(operation) => Some(format!("effect {operation}")),
                Event::StepFinished(step) => Some(format!("finish {step}")),
                Event::Finalizing => Some("finalizing".to_owned()),
                Event::Boundary { .. } => None,
            })
            .collect()
    }

    fn after_first_input(&self) -> Vec<String> {
        self.log()
            .into_iter()
            .skip_while(|entry| entry != "input")
            .skip(1)
            .collect()
    }

    fn started(&self) -> Vec<String> {
        self.events
            .iter()
            .filter_map(|event| match event {
                Event::StepStarted(step) => Some(step.clone()),
                _ => None,
            })
            .collect()
    }

    fn finished(&self) -> Vec<String> {
        self.events
            .iter()
            .filter_map(|event| match event {
                Event::StepFinished(step) => Some(step.clone()),
                _ => None,
            })
            .collect()
    }

    fn inputs(&self) -> usize {
        self.events
            .iter()
            .filter(|event| matches!(event, Event::Input))
            .count()
    }

    fn applications(&self) -> Vec<ApplicationLifecycleAction> {
        self.events
            .iter()
            .filter_map(|event| match event {
                Event::Application(action) => Some(*action),
                _ => None,
            })
            .collect()
    }

    // The candidate pages of every recognition after the first input.
    fn recognitions_after_first_input(&self) -> Vec<Vec<String>> {
        self.events
            .iter()
            .skip_while(|event| !matches!(event, Event::Input))
            .filter_map(|event| match event {
                Event::Recognition(pages) => Some(pages.clone()),
                _ => None,
            })
            .collect()
    }

    // `(started, ended)` of each boundary `wanted`, in order.
    fn boundaries(&self, wanted: TaskTimingBoundary) -> Vec<(Instant, Instant)> {
        self.events
            .iter()
            .filter_map(|event| match event {
                Event::Boundary {
                    boundary,
                    started,
                    ended,
                } if *boundary == wanted => Some((*started, *ended)),
                _ => None,
            })
            .collect()
    }
}

impl ContainedTaskRuntime for Device {
    type Error = &'static str;

    fn update_run_progress(&mut self, executed_steps: u32) {
        self.progress.push(executed_steps);
    }

    fn observe_task_boundary(&mut self, timing: ContainedTaskBoundaryTiming) {
        self.events.push(Event::Boundary {
            boundary: timing.boundary,
            started: timing.started,
            ended: timing.ended,
        });
    }

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        let screen = self.screen();
        self.events.push(Event::Capture(screen));
        Ok(frame(screen).into())
    }

    fn input(
        &mut self,
        _action: InputAction,
        _frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        self.effects += 1;
        self.events.push(Event::Input);
        Ok(())
    }

    fn supports_application_effect(&self) -> bool {
        self.application
    }

    fn control_application(
        &mut self,
        action: ApplicationLifecycleAction,
    ) -> Result<ApplicationEffectSupport, Self::Error> {
        if !self.application {
            return Ok(ApplicationEffectSupport::Unsupported);
        }
        self.effects += 1;
        self.events.push(Event::Application(action));
        Ok(ApplicationEffectSupport::Performed)
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        let event = match trace {
            ContainedTaskTrace::PackageAdmitted { .. } => Event::Admitted,
            ContainedTaskTrace::RunStarted => Event::RunStarted,
            ContainedTaskTrace::RecognitionStarted {
                candidate_pages, ..
            } => Event::Recognition(candidate_pages),
            ContainedTaskTrace::StepStarted {
                step_index,
                operation_label,
                from_page,
                ..
            } => Event::StepStarted(format!("{step_index} {operation_label} {from_page}")),
            ContainedTaskTrace::EffectIntent {
                operation_label, ..
            } => Event::Intent(operation_label),
            ContainedTaskTrace::EffectCompleted {
                operation_label, ..
            } => Event::EffectCompleted(operation_label),
            ContainedTaskTrace::StepFinished {
                step_index,
                operation_label,
                page_label,
                ..
            } => Event::StepFinished(format!("{step_index} {operation_label} {page_label}")),
            ContainedTaskTrace::Finalizing { .. } => Event::Finalizing,
            _ => return Ok(()),
        };
        self.events.push(event);
        Ok(())
    }
}

// One colour target per page on the left pixel.
fn pack() -> RecognitionPack {
    let targets = PAGES
        .iter()
        .map(|screen| {
            json!({"type": "color", "id": format!("page/{}", screen.name()),
                   "region": {"x": 0, "y": 0, "width": 1, "height": 1},
                   "expected": screen.colour()})
        })
        .collect::<Vec<_>>();
    serde_json::from_value(json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": targets
    }))
    .expect("recognition pack")
}

fn page_set() -> PageSet {
    let pages = PAGES
        .iter()
        .map(|screen| {
            json!({"id": format!("neutral/{}", screen.name()),
                   "required": [format!("page/{}", screen.name())]})
        })
        .collect::<Vec<_>>();
    serde_json::from_value(json!({"schema_version": "0.3", "pages": pages})).expect("page set")
}

fn control(max_steps: usize, step_timeout_ms: u64) -> Value {
    json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": PACKAGE,
        "execution_mode": "linear_steps",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 5,
        "step_timeout_ms": step_timeout_ms,
        "timeout_ms": 60_000,
        "max_steps": max_steps
    })
}

fn program(entry: &str, target: &str, operations: Vec<Value>) -> Value {
    json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "entry_page": entry,
        "target_page": target,
        "max_steps": operations.len(),
        "operations": operations
    })
}

// A trusted click from `from` to `to`.
fn click(id: &str, from: &str, to: &str) -> Value {
    json!({"id": id, "from": from, "to": to, "click": {"kind": "point", "x": 1, "y": 0},
           "unguarded_trusted_coordinate": true})
}

// An application step from `from` to `to`.
fn app(id: &str, from: &str, to: &str, action: &str) -> Value {
    json!({"id": id, "from": from, "to": to, "application": {"action": action}})
}

// `operation` with `members` added.
fn with(mut operation: Value, members: Value) -> Value {
    for (member, value) in members.as_object().expect("members to add") {
        operation[member] = value.clone();
    }
    operation
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

// A linear task that the linear admission rules admit.
fn admitted(control: Value, program: Value) -> PreparedContainedTask {
    let package = prepared(control, program);
    if let Err(error) = package
        .program
        .validate_linear(&package.control, &package.detector)
    {
        panic!("the fixture is no admitted linear task: {error}");
    }
    package
}

fn succeeded(
    result: Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>,
) -> ContainedTaskOutcome {
    match result {
        Ok(outcome) => {
            assert_eq!(outcome.outcome, TaskOutcome::Success);
            outcome
        }
        Err(error) => panic!("expected the run to succeed, got {error:?}"),
    }
}

fn failed(
    result: Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>,
) -> ContainedTaskError {
    match result {
        Err(ContainedTaskRunError::Task(error)) => error,
        Err(other) => panic!("expected a task failure, got {other:?}"),
        Ok(outcome) => panic!("expected a task failure, the run ended {outcome:?}"),
    }
}

// The `key=value` tokens of a failure's detail.
fn detail(error: &ContainedTaskError) -> BTreeMap<&str, &str> {
    error
        .detail()
        .unwrap_or_default()
        .split(' ')
        .filter_map(|token| token.split_once('='))
        .collect()
}

fn timing(error: &ContainedTaskError) -> Option<(TaskTimingScope, TaskTimingStage)> {
    error.timing().map(|failure| (failure.scope, failure.stage))
}

fn limit_ms(error: &ContainedTaskError) -> Option<u64> {
    error.timing().map(|failure| failure.limit_ms)
}

// Which operation an admission refusal names.
enum Fault {
    // No operation is at fault: the detail is `reason=<reason>` alone.
    Task,
    // This operation is at fault.
    Operation(&'static str),
    // The contract does not say which of the operations involved is named; only the reason is
    // asserted.
    Unstated,
}

// contracts/linear-steps.md "Admission" (:57-60): a violation is `contained_task_linear_invalid`
// with detail `reason=<reason>`, plus `operation=<id>` when one operation is at fault.
fn assert_refused(case: &str, control: Value, program: Value, reason: &str, fault: Fault) {
    let package = prepared(control, program);
    let error = match package
        .program
        .validate_linear(&package.control, &package.detector)
    {
        Ok(_) => panic!("{case}: admission admits the task"),
        Err(error) => error,
    };
    assert_eq!(
        error.code().as_str(),
        "contained_task_linear_invalid",
        "{case}: {error}"
    );
    let tokens = detail(&error);
    assert_eq!(
        tokens.get("reason").copied(),
        Some(reason),
        "{case}: {error}"
    );
    match fault {
        Fault::Task => assert_eq!(
            tokens.keys().copied().collect::<Vec<_>>(),
            ["reason"],
            "{case}: {error}"
        ),
        Fault::Operation(operation) => assert_eq!(
            tokens.get("operation").copied(),
            Some(operation),
            "{case}: {error}"
        ),
        Fault::Unstated => {}
    }
}

// contracts/linear-steps.md "Admission", table :62-84 (rows `max_steps`, the field-name row of
// `error_pages`, `operation_effect`, `to_equals_from`, `entry_page`, `chain`, `target_page`,
// `transition_page`, `transition_window`, `terminal_page`): each case is the admitted two-step
// path below with one mistake an author makes when editing a recorded package by hand.
#[test]
fn gate_linear_admission_refuses_a_malformed_path() {
    let path = || {
        vec![
            click("open", "home", "list"),
            click("show", "list", "detail"),
        ]
    };
    let base = || program("home", "detail", path());
    admitted(control(2, STEP_TIMEOUT_MS), base());

    let mut stale_count = base();
    stale_count["max_steps"] = json!(3);
    let mut error_pages = base();
    error_pages["error_pages"] = json!(["notice"]);
    let mut other_terminal = base();
    other_terminal["scheduling_outcome"] = json!({"mappings": [{
        "outcome_key": "done", "effect": "no_designated_effect", "terminal_pages": ["list"]
    }]});
    let cases = [
        (
            "a step was removed and max_steps still counts it",
            control(3, STEP_TIMEOUT_MS),
            stale_count,
            "max_steps",
            Fault::Task,
        ),
        (
            "error pages copied from a page-graph task",
            control(2, STEP_TIMEOUT_MS),
            error_pages,
            "error_pages",
            Fault::Task,
        ),
        (
            "an outcome whose terminal page is not the target page",
            control(2, STEP_TIMEOUT_MS),
            other_terminal,
            "terminal_page",
            Fault::Task,
        ),
        (
            "the entry page is not the first step's page",
            control(2, STEP_TIMEOUT_MS),
            program("list", "detail", path()),
            "entry_page",
            Fault::Unstated,
        ),
        (
            "the target page is not the last step's destination",
            control(2, STEP_TIMEOUT_MS),
            program("home", "list", path()),
            "target_page",
            Fault::Unstated,
        ),
        (
            "a step leaves another page than the step before it reached",
            control(2, STEP_TIMEOUT_MS),
            program(
                "home",
                "detail",
                vec![
                    click("open", "home", "list"),
                    click("show", "loading", "detail"),
                ],
            ),
            "chain",
            Fault::Unstated,
        ),
        (
            "a step whose destination is its own page",
            control(3, STEP_TIMEOUT_MS),
            program(
                "home",
                "detail",
                vec![
                    click("open", "home", "list"),
                    click("stay", "list", "list"),
                    click("show", "list", "detail"),
                ],
            ),
            "to_equals_from",
            Fault::Operation("stay"),
        ),
        (
            "the intermediate page is the destination itself",
            control(2, STEP_TIMEOUT_MS),
            program(
                "home",
                "detail",
                vec![
                    with(
                        click("open", "home", "list"),
                        json!({"transition": {"kind": "page", "page_id": "list"}}),
                    ),
                    click("show", "list", "detail"),
                ],
            ),
            "transition_page",
            Fault::Operation("open"),
        ),
        (
            "a window whose bounds are swapped",
            control(2, STEP_TIMEOUT_MS),
            program(
                "home",
                "detail",
                vec![
                    with(
                        click("open", "home", "list"),
                        json!({"transition": {"kind": "window", "min_ms": 3_000, "max_ms": 1_000}}),
                    ),
                    click("show", "list", "detail"),
                ],
            ),
            "transition_window",
            Fault::Operation("open"),
        ),
        (
            "an error handler copied from a page-graph task",
            control(2, STEP_TIMEOUT_MS),
            program(
                "home",
                "detail",
                vec![
                    with(
                        click("open", "home", "list"),
                        json!({"on_error": "return_home"}),
                    ),
                    click("show", "list", "detail"),
                ],
            ),
            "operation_effect",
            Fault::Operation("open"),
        ),
    ];
    for (case, control, program, reason, fault) in cases {
        assert_refused(case, control, program, reason, fault);
    }
}

// contracts/linear-steps.md "Optional steps", admission table :257-266 (`optional_first_step`,
// `optional_application`, `optional_restart_segment_end`), and "Application steps", admission
// table :294-304 (`application_retry`, `any_from`, `any_requires_application`,
// `input_after_application_stop`, `application_without_home`), with the admission row :83
// (`prerequisite_with_application_entry`, "Prerequisite packages" :375-380): each case is one
// misplaced optional or application step in an otherwise admitted path.
#[test]
fn gate_linear_admission_refuses_misplaced_optional_and_application_steps() {
    let optional = || json!({"optional": {"settle_ms": 2_000}});
    admitted(
        control(2, STEP_TIMEOUT_MS),
        program(
            "home",
            "list",
            vec![
                click("open", "home", "notice"),
                with(click("close", "notice", "list"), optional()),
            ],
        ),
    );
    let restart_then_open = || {
        program(
            "any",
            "list",
            vec![
                app("start", "any", "home", "restart"),
                click("open", "home", "list"),
            ],
        )
    };
    admitted(control(2, STEP_TIMEOUT_MS), restart_then_open());

    let mut lead_in = control(2, STEP_TIMEOUT_MS);
    lead_in["prerequisite_package_id"] = json!(LEAD_IN);
    let cases = [
        (
            "the first step marked optional",
            control(2, STEP_TIMEOUT_MS),
            program(
                "home",
                "list",
                vec![
                    with(click("open", "home", "notice"), optional()),
                    click("close", "notice", "list"),
                ],
            ),
            "optional_first_step",
            Fault::Operation("open"),
        ),
        (
            "a restart marked optional",
            control(3, STEP_TIMEOUT_MS),
            program(
                "home",
                "detail",
                vec![
                    click("open", "home", "list"),
                    with(app("restart", "list", "home", "restart"), optional()),
                    click("show", "home", "detail"),
                ],
            ),
            "optional_application",
            Fault::Operation("restart"),
        ),
        (
            "the main interface after a restart is an optional step's page",
            control(2, STEP_TIMEOUT_MS),
            program(
                "any",
                "list",
                vec![
                    app("start", "any", "home", "restart"),
                    with(click("greet", "home", "list"), optional()),
                ],
            ),
            "optional_restart_segment_end",
            Fault::Operation("greet"),
        ),
        (
            "a restart declared with retries",
            control(2, STEP_TIMEOUT_MS),
            program(
                "any",
                "list",
                vec![
                    with(
                        app("start", "any", "home", "restart"),
                        json!({"retryable": true, "max_attempts": 2, "retry_interval_ms": 1_000}),
                    ),
                    click("open", "home", "list"),
                ],
            ),
            "application_retry",
            Fault::Operation("start"),
        ),
        (
            "a restart from any screen in the middle of the path",
            control(2, STEP_TIMEOUT_MS),
            program(
                "home",
                "home",
                vec![
                    click("open", "home", "list"),
                    app("start", "any", "home", "restart"),
                ],
            ),
            "any_from",
            Fault::Operation("start"),
        ),
        (
            "a click from any screen",
            control(1, STEP_TIMEOUT_MS),
            program("any", "list", vec![click("open", "any", "list")]),
            "any_requires_application",
            Fault::Operation("open"),
        ),
        (
            "a click after the application was stopped",
            control(3, STEP_TIMEOUT_MS),
            program(
                "home",
                "detail",
                vec![
                    click("open", "home", "list"),
                    app("stop", "list", "home", "stop"),
                    click("show", "home", "detail"),
                ],
            ),
            "input_after_application_stop",
            Fault::Unstated,
        ),
        (
            "a restart whose path never reaches the main interface",
            control(2, STEP_TIMEOUT_MS),
            program(
                "any",
                "detail",
                vec![
                    app("start", "any", "list", "restart"),
                    click("show", "list", "detail"),
                ],
            ),
            "application_without_home",
            Fault::Unstated,
        ),
        (
            "an application entry that names a prerequisite package",
            lead_in,
            restart_then_open(),
            "prerequisite_with_application_entry",
            Fault::Unstated,
        ),
    ];
    for (case, control, program, reason, fault) in cases {
        assert_refused(case, control, program, reason, fault);
    }
}

// contracts/linear-steps.md "Execution" (:100-112, steps 1-6 at :138-153, :154-159) and "Ledger"
// (:659-667): after `PackageAdmitted` and `RunStarted` the first step's page is awaited; each step
// then starts on the frame on which its page passed (`StepStarted` naming the operation and that
// page, no new capture), its effect intent, input and effect completion follow, only the next
// step's page is evaluated, and `StepFinished` names it. The path passes its target page after its
// first step and still runs on to its last operation (:157-159); the run finishes on the target
// page with `executed_steps` the operation count (:154-156), and the run progress becomes `d + 1`
// at each step (:144).
#[test]
fn gate_linear_each_step_leaves_its_page_and_reaches_the_next() {
    let package = admitted(
        control(3, STEP_TIMEOUT_MS),
        program(
            "home",
            "list",
            vec![
                click("open", "home", "list"),
                click("show", "list", "detail"),
                click("back", "detail", "list"),
            ],
        ),
    );
    let mut device = Device::new(vec![
        vec![Screen::Home],
        vec![Screen::List],
        vec![Screen::Detail],
        vec![Screen::List],
    ]);

    let outcome = succeeded(package.run(&mut device));

    assert_eq!(outcome.final_page.as_deref(), Some("neutral/list"));
    assert_eq!(outcome.executed_steps, 3);
    assert_eq!(
        device.log(),
        [
            "admitted",
            "run started",
            "capture home",
            "recognize neutral/home",
            "start 0 open neutral/home",
            "intent open",
            "input",
            "effect open",
            "capture list",
            "recognize neutral/list",
            "finish 0 open neutral/list",
            "start 1 show neutral/list",
            "intent show",
            "input",
            "effect show",
            "capture detail",
            "recognize neutral/detail",
            "finish 1 show neutral/detail",
            "start 2 back neutral/detail",
            "intent back",
            "input",
            "effect back",
            "capture list",
            "recognize neutral/list",
            "finish 2 back neutral/list",
            "finalizing",
        ]
    );
    let mut progress = device.progress.clone();
    progress.retain(|steps| *steps > 0);
    progress.dedup();
    assert_eq!(progress, [1, 2, 3]);
}

// contracts/linear-steps.md "Intermediate states" (:177-182), "Retries" (:193-196), "Failure
// codes" (:617-618) and "Ledger" (:664): a declared intermediate page is awaited alone after the
// input and must be seen before the next step's page is evaluated. A next step's page that already
// shows never skips it: without an attempt left the task fails with
// `contained_task_linear_intermediate_unobserved` (`postcondition` / `postcondition`) and
// `StepFinished` names `<unrecognized>`. Once the intermediate page was seen, a next step's page
// that does not follow fails the task with `page_confirmation_failed`, `transition=page` and
// `intermediate_seen=true`, `limit_ms` the spent arrival budget, with no retry decision and no
// further input although the operation declares a retry.
#[test]
fn gate_linear_an_intermediate_page_must_be_seen_before_the_next_step() {
    let transition =
        json!({"kind": "page", "page_id": "loading", "timeout_ms": SPENT_MS, "interval_ms": 5});
    let plain = || {
        admitted(
            control(1, STEP_TIMEOUT_MS),
            program(
                "home",
                "list",
                vec![with(
                    click("open", "home", "list"),
                    json!({"transition": transition.clone()}),
                )],
            ),
        )
    };

    let mut device = Device::new(vec![
        vec![Screen::Home],
        vec![Screen::Loading, Screen::List],
    ]);
    let outcome = succeeded(plain().run(&mut device));
    assert_eq!(outcome.final_page.as_deref(), Some("neutral/list"));
    assert_eq!(
        device.after_first_input(),
        [
            "effect open",
            "capture loading",
            "recognize neutral/loading",
            "capture list",
            "recognize neutral/list",
            "finish 0 open neutral/list",
            "finalizing",
        ]
    );

    let mut device = Device::new(vec![vec![Screen::Home], vec![Screen::List]]);
    let error = failed(plain().run(&mut device));
    assert_eq!(
        error.code().as_str(),
        "contained_task_linear_intermediate_unobserved",
        "{error}"
    );
    assert_eq!(
        timing(&error),
        Some((
            TaskTimingScope::Postcondition,
            TaskTimingStage::Postcondition
        ))
    );
    assert_eq!(device.inputs(), 1);
    assert_eq!(device.finished(), ["0 open <unrecognized>"]);
    let awaited = device.recognitions_after_first_input();
    assert!(!awaited.is_empty());
    assert!(
        awaited.iter().all(|pages| *pages == ["neutral/loading"]),
        "{awaited:?}"
    );

    let retried = admitted(
        control(1, STEP_TIMEOUT_MS),
        program(
            "home",
            "list",
            vec![with(
                click("open", "home", "list"),
                json!({"transition": transition.clone(),
                       "expect_after": {"page_id": "list", "timeout_ms": SPENT_MS},
                       "retryable": true, "max_attempts": 2, "retry_interval_ms": 5}),
            )],
        ),
    );
    let mut device = Device::new(vec![
        vec![Screen::Home],
        vec![Screen::Loading, Screen::Blank],
    ]);
    let error = failed(retried.run(&mut device));
    assert_eq!(error.code().as_str(), "page_confirmation_failed", "{error}");
    let tokens = detail(&error);
    assert_eq!(tokens.get("operation").copied(), Some("open"), "{error}");
    assert_eq!(tokens.get("transition").copied(), Some("page"), "{error}");
    assert_eq!(
        tokens.get("intermediate_seen").copied(),
        Some("true"),
        "{error}"
    );
    assert_eq!(
        timing(&error),
        Some((
            TaskTimingScope::Postcondition,
            TaskTimingStage::Postcondition
        ))
    );
    assert_eq!(limit_ms(&error), Some(SPENT_MS));
    assert_eq!(device.inputs(), 1);
    assert_eq!(device.started(), ["0 open neutral/home"]);
    assert_eq!(device.finished(), ["0 open <unrecognized>"]);
    let awaited = device.recognitions_after_first_input();
    assert!(
        awaited.iter().flatten().all(|page| page != "neutral/home"),
        "a retry decision awaited the step's own page: {awaited:?}"
    );
}

// contracts/linear-steps.md "Execution" step 4 (:149) and "Intermediate states", Window (:183):
// the wait after an input is `post_delay_ms`, or the window's `min_ms` when larger, and nothing is
// evaluated before it. Read through the runtime's callbacks: the one `PostInputWait` boundary lies
// between the input and the next capture, no recognition comes before its end, the next capture's
// boundary starts after it ended, and it lasted at least the larger of the two values.
#[test]
fn gate_linear_the_post_input_wait_is_the_larger_of_post_delay_and_the_window_minimum() {
    for (post_delay_ms, min_ms) in [(20_u64, 50_u64), (50, 20)] {
        let required = Duration::from_millis(post_delay_ms.max(min_ms));
        let package = admitted(
            control(1, STEP_TIMEOUT_MS),
            program(
                "home",
                "list",
                vec![with(
                    click("open", "home", "list"),
                    json!({"post_delay_ms": post_delay_ms,
                           "transition": {"kind": "window", "min_ms": min_ms, "max_ms": 60}}),
                )],
            ),
        );
        let mut device = Device::new(vec![vec![Screen::Home], vec![Screen::List]]);

        let outcome = succeeded(package.run(&mut device));

        assert_eq!(outcome.final_page.as_deref(), Some("neutral/list"));
        let events = &device.events;
        let input = events
            .iter()
            .position(|event| matches!(event, Event::Input))
            .expect("the step's input");
        let waits = events
            .iter()
            .enumerate()
            .filter_map(|(index, event)| match event {
                Event::Boundary {
                    boundary: TaskTimingBoundary::PostInputWait,
                    started,
                    ended,
                } => Some((index, *started, *ended)),
                _ => None,
            })
            .collect::<Vec<_>>();
        let &[(wait, started, ended)] = waits.as_slice() else {
            panic!("one post-input wait: {waits:?}");
        };
        let capture = events[input..]
            .iter()
            .position(|event| matches!(event, Event::Capture(_)))
            .map(|offset| input + offset)
            .expect("a capture after the input");
        assert!(
            input < wait && wait < capture,
            "input {input}, post-input wait {wait}, next capture {capture}: {events:?}"
        );
        assert!(
            !events[input..wait]
                .iter()
                .any(|event| matches!(event, Event::Recognition(_))),
            "a recognition before the post-input wait ended: {events:?}"
        );
        let waited = ended.duration_since(started);
        assert!(
            waited >= required,
            "waited {waited:?}, required {required:?}"
        );
        let capture_started = device
            .boundaries(TaskTimingBoundary::CapturePage)
            .into_iter()
            .map(|(capture_started, _)| capture_started)
            .find(|capture_started| *capture_started >= started)
            .expect("the next capture's boundary");
        assert!(capture_started >= ended);
    }
}

// contracts/linear-steps.md "Optional steps" (:212-246) and "Ledger" (:659-661, :667): after the
// operation before a run of optional steps, the run's optional pages and then its skip target `N`
// are the candidates, evaluated together. Absent page: `N` is reached only on a frame captured at
// least the settle after the one on which it first passed (so on a second frame at the earliest);
// the skip shows as the preceding `StepFinished` naming `N` and no `StepStarted` of the optional
// operation, and `executed_steps` counts the operations dispatched. Present page: it passes at
// once and its operation runs next, from that page, and awaits only `N`; an optional page that
// appears after `N` within the settle is still run.
#[test]
fn gate_linear_an_optional_step_is_skipped_when_its_page_is_absent_and_run_when_present() {
    let popup = |settle_ms: u64| {
        admitted(
            control(2, STEP_TIMEOUT_MS),
            program(
                "home",
                "list",
                vec![
                    click("open", "home", "notice"),
                    with(
                        click("close", "notice", "list"),
                        json!({"optional": {"settle_ms": settle_ms}}),
                    ),
                ],
            ),
        )
    };

    let mut device = Device::new(vec![vec![Screen::Home], vec![Screen::List]]);
    let outcome = succeeded(popup(20).run(&mut device));
    assert_eq!(outcome.final_page.as_deref(), Some("neutral/list"));
    assert_eq!(outcome.executed_steps, 1);
    assert_eq!(device.started(), ["0 open neutral/home"]);
    assert_eq!(device.finished(), ["0 open neutral/list"]);
    assert_eq!(device.inputs(), 1);
    let awaited = device.recognitions_after_first_input();
    assert!(awaited.len() >= 2, "{awaited:?}");
    assert!(
        awaited
            .iter()
            .all(|pages| *pages == ["neutral/notice", "neutral/list"]),
        "{awaited:?}"
    );

    let mut device = Device::new(vec![
        vec![Screen::Home],
        vec![Screen::Notice],
        vec![Screen::List],
    ]);
    let outcome = succeeded(popup(20).run(&mut device));
    assert_eq!(outcome.final_page.as_deref(), Some("neutral/list"));
    assert_eq!(outcome.executed_steps, 2);
    assert_eq!(
        device.started(),
        ["0 open neutral/home", "1 close neutral/notice"]
    );
    assert_eq!(
        device.finished(),
        ["0 open neutral/notice", "1 close neutral/list"]
    );
    let awaited = device.recognitions_after_first_input();
    assert_eq!(awaited.len(), 2, "{awaited:?}");
    assert_eq!(awaited[0], ["neutral/notice", "neutral/list"]);
    assert_eq!(awaited[1], ["neutral/list"]);

    let mut device = Device::new(vec![
        vec![Screen::Home],
        vec![Screen::List, Screen::Notice],
        vec![Screen::List],
    ]);
    let outcome = succeeded(popup(5_000).run(&mut device));
    assert_eq!(outcome.executed_steps, 2);
    assert_eq!(
        device.started(),
        ["0 open neutral/home", "1 close neutral/notice"]
    );
    assert_eq!(
        device.finished(),
        ["0 open neutral/notice", "1 close neutral/list"]
    );
}

// contracts/linear-steps.md "Retries" (:188-208) and "Failure codes" (:618): a retry exists only
// for a swallowed input. Swallowed (the step's own page shows again): after `retry_interval_ms`
// the retry decision awaits the next step's page first, then the step's own page; the own page
// passes, `StepFinished` names it, and the next attempt starts on that frame with another
// `StepStarted` of the same `step_index`; a retry adds no executed step. Late (the input took
// effect after the arrival budget): the next step's page passes in the retry decision and the
// attempt goes on with no other input. An operation that declares no retry is not retried: the
// task fails with `page_confirmation_failed`, `transition=none`, `limit_ms` the spent budget, and
// `StepFinished` names `<unrecognized>`.
#[test]
fn gate_linear_only_a_swallowed_input_is_retried() {
    let open = |retry: bool| {
        let mut operation = with(
            click("open", "home", "list"),
            json!({"expect_after": {"page_id": "list", "timeout_ms": SPENT_MS}}),
        );
        if retry {
            operation = with(
                operation,
                json!({"retryable": true, "max_attempts": 2, "retry_interval_ms": 5}),
            );
        }
        admitted(
            control(1, STEP_TIMEOUT_MS),
            program("home", "list", vec![operation]),
        )
    };

    let mut device = Device::new(vec![
        vec![Screen::Home],
        vec![Screen::Home],
        vec![Screen::List],
    ]);
    let outcome = succeeded(open(true).run(&mut device));
    assert_eq!(outcome.final_page.as_deref(), Some("neutral/list"));
    assert_eq!(outcome.executed_steps, 1);
    assert_eq!(device.inputs(), 2);
    assert_eq!(
        device.started(),
        ["0 open neutral/home", "0 open neutral/home"]
    );
    assert_eq!(
        device.finished(),
        ["0 open neutral/home", "0 open neutral/list"]
    );
    let log = device.log();
    let swallowed = log
        .iter()
        .position(|entry| entry == "finish 0 open neutral/home")
        .expect("the retry decision saw the step's own page");
    assert_eq!(log[swallowed - 1], "recognize neutral/list,neutral/home");
    assert_eq!(
        log[swallowed + 1..],
        [
            "start 0 open neutral/home",
            "intent open",
            "input",
            "effect open",
            "capture list",
            "recognize neutral/list",
            "finish 0 open neutral/list",
            "finalizing",
        ]
    );
    let retry_waits = device.boundaries(TaskTimingBoundary::RetryWait);
    let &[(retry_started, retry_ended)] = retry_waits.as_slice() else {
        panic!("one retry wait: {retry_waits:?}");
    };
    assert!(retry_ended.duration_since(retry_started) >= Duration::from_millis(5));

    // At most ten frames fit in the arrival budget; the next step's page shows on the 21st.
    let late = [vec![Screen::Blank; 20], vec![Screen::List]].concat();
    let mut device = Device::new(vec![vec![Screen::Home], late]);
    let outcome = succeeded(open(true).run(&mut device));
    assert_eq!(outcome.final_page.as_deref(), Some("neutral/list"));
    assert_eq!(outcome.executed_steps, 1);
    assert_eq!(device.inputs(), 1);
    assert_eq!(device.started(), ["0 open neutral/home"]);
    assert_eq!(device.finished(), ["0 open neutral/list"]);
    let awaited = device.recognitions_after_first_input();
    assert!(
        awaited
            .iter()
            .any(|pages| *pages == ["neutral/list", "neutral/home"]),
        "{awaited:?}"
    );

    let mut device = Device::new(vec![vec![Screen::Home], vec![Screen::Home]]);
    let error = failed(open(false).run(&mut device));
    assert_eq!(error.code().as_str(), "page_confirmation_failed", "{error}");
    let tokens = detail(&error);
    assert_eq!(tokens.get("operation").copied(), Some("open"), "{error}");
    assert_eq!(tokens.get("transition").copied(), Some("none"), "{error}");
    assert_ne!(
        tokens.get("intermediate_seen").copied(),
        Some("true"),
        "{error}"
    );
    assert_eq!(
        timing(&error),
        Some((
            TaskTimingScope::Postcondition,
            TaskTimingStage::Postcondition
        ))
    );
    assert_eq!(limit_ms(&error), Some(SPENT_MS));
    assert_eq!(device.inputs(), 1);
    assert_eq!(device.started(), ["0 open neutral/home"]);
    assert_eq!(device.finished(), ["0 open <unrecognized>"]);
    let awaited = device.recognitions_after_first_input();
    assert!(
        awaited.iter().all(|pages| *pages == ["neutral/list"]),
        "a retry decision ran: {awaited:?}"
    );
}

// contracts/linear-steps.md "Application steps" (:281-291, :318-322, :324-348), "Failure codes"
// (:619-620) and "Ledger" (:660, :663): the application entry captures and recognizes nothing
// before its effect; its `StepStarted` names `<unrecognized>`; it has no effect intent, its
// application action is followed by `EffectCompleted`, and the run's first capture is the arrival
// wait after it; the next click starts on the frame its page passed on. When the next step's page
// does not pass, the task fails with `contained_task_linear_application_unconfirmed`
// (`operation=<id> application=<action> attempts=1 transition=none intermediate_seen=false`,
// `postcondition` / `postcondition`, `limit_ms` the spent budget) after one action, never retried.
// An instance without an assigned application refuses the package with
// `application_effect_requires_assigned_application` after `RunStarted` and before any capture.
#[test]
fn gate_linear_an_application_entry_starts_from_any_screen_and_is_never_retried() {
    let restart = || {
        admitted(
            control(2, STEP_TIMEOUT_MS),
            program(
                "any",
                "list",
                vec![
                    with(
                        app("start", "any", "home", "restart"),
                        json!({"expect_after": {"page_id": "home", "timeout_ms": SPENT_MS}}),
                    ),
                    click("open", "home", "list"),
                ],
            ),
        )
    };

    let mut device = Device::with_application(vec![
        vec![Screen::Blank],
        vec![Screen::Home],
        vec![Screen::List],
    ]);
    let outcome = succeeded(restart().run(&mut device));
    assert_eq!(outcome.final_page.as_deref(), Some("neutral/list"));
    assert_eq!(outcome.executed_steps, 2);
    assert_eq!(
        device.log(),
        [
            "admitted",
            "run started",
            "start 0 start <unrecognized>",
            "application Restart",
            "effect start",
            "capture home",
            "recognize neutral/home",
            "finish 0 start neutral/home",
            "start 1 open neutral/home",
            "intent open",
            "input",
            "effect open",
            "capture list",
            "recognize neutral/list",
            "finish 1 open neutral/list",
            "finalizing",
        ]
    );

    let mut device = Device::with_application(vec![vec![Screen::Blank]]);
    let error = failed(restart().run(&mut device));
    assert_eq!(
        error.code().as_str(),
        "contained_task_linear_application_unconfirmed",
        "{error}"
    );
    let tokens = detail(&error);
    for (key, value) in [
        ("operation", "start"),
        ("application", "restart"),
        ("attempts", "1"),
        ("transition", "none"),
        ("intermediate_seen", "false"),
    ] {
        assert_eq!(tokens.get(key).copied(), Some(value), "{key}: {error}");
    }
    assert_eq!(
        timing(&error),
        Some((
            TaskTimingScope::Postcondition,
            TaskTimingStage::Postcondition
        ))
    );
    assert_eq!(limit_ms(&error), Some(SPENT_MS));
    assert_eq!(device.applications(), [ApplicationLifecycleAction::Restart]);
    assert_eq!(device.inputs(), 0);
    assert_eq!(device.started(), ["0 start <unrecognized>"]);
    assert_eq!(device.finished(), ["0 start <unrecognized>"]);

    let mut device = Device::new(vec![vec![Screen::Home]]);
    let error = failed(restart().run(&mut device));
    assert_eq!(
        error.code().as_str(),
        "application_effect_requires_assigned_application",
        "{error}"
    );
    assert_eq!(device.log(), ["admitted", "run started"]);
}

// The first layer the kernel's shared prerequisite-chain predicate gives a package, as
// `(package id, from the return-home configuration)`.
fn first_layer(
    package: &PreparedContainedTask,
    return_home: Option<&str>,
) -> Option<(String, bool)> {
    crate::PrerequisiteChain::new(package.package_descriptor())
        .next(return_home)
        .map(|link| (link.package_id, link.return_home))
}

// contracts/package-reference.md:384-387 and contracts/linear-steps.md "Return-home fallback"
// (:511-525): only a `linear_steps` package whose first step is a page and that declares no
// prerequisite package falls back to the configured return-home package; there is none when the
// configuration names none for its game and server or names the package itself (already in the
// chain); a package that declares a prerequisite package runs that one first; an application
// entry and a page-graph package never fall back.
#[test]
fn gate_linear_only_a_page_entry_without_a_prerequisite_falls_back_to_return_home() {
    let path = || program("home", "list", vec![click("open", "home", "list")]);
    let page_entry = admitted(control(1, ENTRY_MS), path());
    assert_eq!(
        first_layer(&page_entry, Some(RETURN_HOME)),
        Some((RETURN_HOME.to_owned(), true))
    );
    assert_eq!(first_layer(&page_entry, None), None);
    assert_eq!(first_layer(&page_entry, Some(PACKAGE)), None);

    let mut declares = control(1, ENTRY_MS);
    declares["prerequisite_package_id"] = json!(LEAD_IN);
    let declared = admitted(declares, path());
    assert_eq!(
        first_layer(&declared, Some(RETURN_HOME)),
        Some((LEAD_IN.to_owned(), false))
    );

    let application_entry = admitted(
        control(1, ENTRY_MS),
        program("any", "home", vec![app("start", "any", "home", "restart")]),
    );
    assert_eq!(first_layer(&application_entry, Some(RETURN_HOME)), None);

    let mut page_graph = control(1, ENTRY_MS);
    page_graph["execution_mode"] = json!("navigable_route");
    let page_graph = prepared(page_graph, path());
    assert_eq!(first_layer(&page_graph, Some(RETURN_HOME)), None);
}

// contracts/linear-steps.md "Return-home fallback" (:530-537), "Prerequisite packages" step 3
// (:435-446) and "Failure codes" (:616, :629): after the return-home package ran, the kernel
// evaluates the first step's page alone until it passes or the step timeout is spent; spent, it
// is `contained_task_return_home_entry_unmatched` with detail `layer=<i> package_id=<X_i>
// required_page=<page> return_home=<return-home package id>` and timing `page_recognition` /
// `entry_recognition` with `limit_ms` the step timeout. A package started directly, with neither a
// prerequisite package nor a fallback, that does not start on its first step's page fails with
// `contained_task_linear_entry_unmatched` (`page_recognition` / `entry_recognition`) before any
// step.
#[test]
fn gate_linear_the_first_step_after_return_home_and_without_a_fallback() {
    let package = admitted(
        control(1, ENTRY_MS),
        program("home", "list", vec![click("open", "home", "list")]),
    );
    let step_timeout = Duration::from_millis(ENTRY_MS);

    let mut device = Device::new(vec![vec![Screen::Blank]]);
    let awaited = package
        .await_gate_entry(
            &mut device,
            step_timeout,
            TaskTimingBudgetOrigin::Task,
            0,
            Some(RETURN_HOME),
        )
        .expect("the recheck runs");
    let error = awaited
        .unmatched
        .expect("the first step's page never passed");
    assert_eq!(
        error.code().as_str(),
        "contained_task_return_home_entry_unmatched",
        "{error}"
    );
    assert_eq!(
        error.detail(),
        Some(
            format!(
                "layer=0 package_id={PACKAGE} required_page=neutral/home return_home={RETURN_HOME}"
            )
            .as_str()
        )
    );
    assert_eq!(
        timing(&error),
        Some((
            TaskTimingScope::PageRecognition,
            TaskTimingStage::EntryRecognition
        ))
    );
    assert_eq!(limit_ms(&error), Some(ENTRY_MS));
    let recognitions = device.log();
    assert!(
        recognitions
            .iter()
            .filter(|entry| entry.starts_with("recognize"))
            .all(|entry| entry == "recognize neutral/home"),
        "{recognitions:?}"
    );
    assert_eq!(device.inputs(), 0);

    let mut device = Device::new(vec![vec![Screen::Home]]);
    let awaited = package
        .await_gate_entry(
            &mut device,
            step_timeout,
            TaskTimingBudgetOrigin::Task,
            0,
            Some(RETURN_HOME),
        )
        .expect("the recheck runs");
    assert!(awaited.unmatched.is_none(), "{:?}", awaited.unmatched);

    let mut device = Device::new(vec![vec![Screen::Blank]]);
    let error = failed(package.run(&mut device));
    assert_eq!(
        error.code().as_str(),
        "contained_task_linear_entry_unmatched",
        "{error}"
    );
    assert_eq!(
        timing(&error),
        Some((
            TaskTimingScope::PageRecognition,
            TaskTimingStage::EntryRecognition
        ))
    );
    assert!(device.started().is_empty(), "{:?}", device.started());
    assert_eq!(device.inputs(), 0);
}
