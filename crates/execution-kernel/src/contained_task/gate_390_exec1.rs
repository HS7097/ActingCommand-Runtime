// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #390 EXEC-1 (forced-compromises mapping row E-04, with E-13; FCM §2.4 row EXEC-1).
// One page may carry several ordered candidate operations, each with a `when` (a target present
// or absent); a guard refusal may be declared to fall to the next candidate, and the run fails
// only when no candidate holds. A frame that matches more than one page stays a conflict.
//
// Every expected value comes from the FCM rows and the declaration rules of the contract
// (`contracts/resource-declarations.md`, Task schema `0.9` candidate operations).

use super::*;
use actingcommand_contract::codes::TaskflowCode;
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition_pack::RecognitionPack;
use serde_json::{Value, json};
use std::path::PathBuf;

const STEP_TIMEOUT: Duration = Duration::from_secs(10);
const TASK_TIMEOUT: Duration = Duration::from_secs(60);
const HOME: &str = "neutral/home";

// What the screen shows: the left pixel names the page, the right pixel is the flag, ready
// (green) or busy (grey).
#[derive(Debug, Clone, Copy)]
enum Screen {
    Home,
    HomeBusy,
    Terminal,
    Other,
}

fn frame(screen: Screen) -> Frame {
    let (page, flag) = match screen {
        Screen::Home => ([255, 0, 0], [0, 255, 0]),
        Screen::HomeBusy => ([255, 0, 0], [128, 128, 128]),
        Screen::Terminal => ([0, 0, 255], [0, 255, 0]),
        Screen::Other => ([255, 255, 0], [0, 255, 0]),
    };
    Frame::from_pixels(
        2,
        1,
        [page, flag].concat(),
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Event {
    Capture,
    // A `when` evaluation and its target.
    When(String),
    // Any other guard evaluation.
    Guard,
    StepStarted(String, u32),
    Intent(String),
    Input,
    // The operation and the page its step finished on.
    StepFinished(String, String),
}

// The Runtime as the kernel sees it: every capture shows the screen of the number of inputs made
// so far (the last one repeats).
struct ScriptedRuntime {
    screens: Vec<Screen>,
    inputs: usize,
    events: Vec<Event>,
}

impl ScriptedRuntime {
    fn new(screens: Vec<Screen>) -> Self {
        Self {
            screens,
            inputs: 0,
            events: Vec::new(),
        }
    }

    // The events from the first `When` or `StepStarted` to the first input, or to the end.
    fn before_first_input(&self) -> Vec<Event> {
        let start = self
            .events
            .iter()
            .position(|event| matches!(event, Event::When(_) | Event::StepStarted(..)))
            .expect("a candidate was considered");
        let end = self
            .events
            .iter()
            .position(|event| *event == Event::Input)
            .map_or(self.events.len(), |input| input + 1);
        self.events[start..end].to_vec()
    }

    // The steps that started, with their indexes.
    fn started(&self) -> Vec<(String, u32)> {
        self.events
            .iter()
            .filter_map(|event| match event {
                Event::StepStarted(label, index) => Some((label.clone(), *index)),
                _ => None,
            })
            .collect()
    }
}

impl ContainedTaskRuntime for ScriptedRuntime {
    type Error = &'static str;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        let screen = self.screens[self.inputs.min(self.screens.len() - 1)];
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
        target_id: Option<&str>,
        _result: Option<&actingcommand_recognition_pack::RecognitionPackResult<TargetEvaluation>>,
        evaluation: ContainedTaskGuardEvaluation,
    ) -> Result<(), Self::Error> {
        self.events.push(match evaluation {
            ContainedTaskGuardEvaluation::When => {
                Event::When(target_id.expect("a when names its target").to_owned())
            }
            _ => Event::Guard,
        });
        Ok(())
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        match trace {
            ContainedTaskTrace::CaptureCompleted { .. } => self.events.push(Event::Capture),
            ContainedTaskTrace::StepStarted {
                step_index,
                operation_label,
                ..
            } => self
                .events
                .push(Event::StepStarted(operation_label, step_index)),
            ContainedTaskTrace::EffectIntent {
                operation_label, ..
            } => self.events.push(Event::Intent(operation_label)),
            ContainedTaskTrace::StepFinished {
                operation_label,
                page_label,
                ..
            } => self
                .events
                .push(Event::StepFinished(operation_label, page_label)),
            _ => {}
        }
        Ok(())
    }
}

fn control_json() -> Value {
    json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.candidates.task",
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

// The three pages and the flag; with `consensus`, the flag needs two frames.
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
            {"type": "color", "id": "page/other",
             "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [255, 255, 0]},
            {"type": "color", "id": "flag/ready",
             "region": {"x": 1, "y": 0, "width": 1, "height": 1}, "expected": [0, 255, 0]}
        ]
    });
    if consensus {
        pack["target_consensus"] = json!({
            "flag/ready": {"samples": [{"frame": 0}, {"frame": 1}], "k": 2,
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
            {"id": "neutral/other", "required": ["page/other"]}
        ]
    }))
    .expect("page set")
}

// `claim`, the click from `home` to `terminal` guarded by the flag, with `extra` members.
fn claim(extra: Value) -> Value {
    let mut operation = json!({
        "id": "claim",
        "from": "home",
        "to": "terminal",
        "click": {"kind": "point", "x": 1, "y": 0},
        "guard": {"page_id": "home", "target_id": "flag/ready",
                  "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                  "color_probe": "flag/ready"}
    });
    with(&mut operation, extra);
    operation
}

// `skip`, a trusted tap from `home` to `other`, with `extra` members.
fn skip(extra: Value) -> Value {
    let mut operation = json!({
        "id": "skip",
        "from": "home",
        "to": "other",
        "click": {"kind": "point", "x": 0, "y": 0},
        "unguarded_trusted_coordinate": true
    });
    with(&mut operation, extra);
    operation
}

fn with(operation: &mut Value, extra: Value) {
    for (member, value) in extra.as_object().expect("extra members") {
        operation[member] = value.clone();
    }
}

fn program(operations: Vec<Value>) -> TaskProgram {
    serde_json::from_value(json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "target_page": ["terminal", "other"],
        "operations": operations
    }))
    .expect("task program")
}

fn task(operations: Vec<Value>) -> PreparedContainedTask {
    let control = control();
    let program = program(operations);
    for operation in &program.operations {
        operation
            .validate(&control, TaskOperationDefaults::default(), "0.9")
            .expect("valid operation");
    }
    let evaluator = RecognitionEvaluator::new(PathBuf::new(), pack(false)).expect("evaluator");
    program
        .validate_when_targets("tasks/task/task.json", &evaluator)
        .expect("when targets");
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

fn refused_code(result: Result<(), ContainedTaskError>) -> String {
    result
        .expect_err("admission refuses the declaration")
        .code()
        .as_str()
        .to_owned()
}

fn started(label: &str, index: u32) -> Event {
    Event::StepStarted(label.to_owned(), index)
}

fn finished(label: &str, page: &str) -> Event {
    Event::StepFinished(label.to_owned(), page.to_owned())
}

// FCM E-04 「每条带 when」「守卫拒绝时可以声明落到下一条候选」, §2.4 (page-graph declarations): what
// admission takes and refuses.
#[test]
fn exec1_admission_takes_when_and_a_declared_fall_through_on_page_graph_operations() {
    let control = control();
    let defaults = TaskOperationDefaults::default();
    let invalid = ExecutionKernelCode::ContainedTaskOperationInvalid.as_str();
    let check = |operation: Value, schema: &str| {
        serde_json::from_value::<TaskOperation>(operation)
            .expect("operation")
            .validate(&control, defaults, schema)
    };
    let restart = |extra: Value| {
        let mut operation = json!({"id": "restart", "from": "home", "to": "terminal",
                                   "application": {"action": "restart"}});
        with(&mut operation, extra);
        operation
    };
    for admitted in [
        claim(json!({"when": {"present": "flag/ready"}})),
        claim(json!({"when": {"absent": "flag/ready"}})),
        claim(json!({"on_guard_refused": "next_candidate"})),
        claim(json!({"when": {"present": "page/home"}, "on_guard_refused": "next_candidate"})),
        skip(json!({"when": {"absent": "flag/ready"}})),
        restart(json!({"when": {"present": "flag/ready"}})),
    ] {
        check(admitted.clone(), "0.9").unwrap_or_else(|error| panic!("{admitted}: {error}"));
    }
    for refused in [
        // A guard refusal falls through only from a click with a declared guard.
        skip(json!({"on_guard_refused": "next_candidate"})),
        restart(json!({"on_guard_refused": "next_candidate"})),
        // A `when` names a target.
        claim(json!({"when": {"present": " "}})),
    ] {
        assert_eq!(
            refused_code(check(refused.clone(), "0.9")),
            invalid,
            "{refused}"
        );
    }
    // An older task schema refuses both declarations.
    for older in [
        claim(json!({"when": {"present": "flag/ready"}})),
        claim(json!({"on_guard_refused": "next_candidate"})),
    ] {
        assert_eq!(
            refused_code(check(older.clone(), "0.8")),
            invalid,
            "{older}"
        );
    }

    // A `when` names a target of the pack judged on one frame, and is not declared on the entry
    // application, which runs before any page is known.
    let when_targets = |operations: Vec<Value>, consensus: bool| {
        let evaluator =
            RecognitionEvaluator::new(PathBuf::new(), pack(consensus)).expect("evaluator");
        program(operations).validate_when_targets("tasks/task/task.json", &evaluator)
    };
    when_targets(
        vec![
            claim(json!({"when": {"present": "flag/ready"}})),
            skip(json!({})),
        ],
        false,
    )
    .expect("a target of the pack");
    for (case, operations, consensus) in [
        (
            "an unknown target",
            vec![claim(json!({"when": {"present": "flag/missing"}}))],
            false,
        ),
        (
            "a target under consensus",
            vec![claim(json!({"when": {"present": "flag/ready"}}))],
            true,
        ),
        (
            "the entry application",
            vec![
                json!({"id": "launch", "from": "any", "to": "home",
                       "application": {"action": "restart"},
                       "when": {"absent": "page/home"}}),
                claim(json!({})),
            ],
            false,
        ),
    ] {
        assert_eq!(
            refused_code(when_targets(operations, consensus)),
            invalid,
            "{case}"
        );
    }

    // Both are page-graph declarations; a linear task refuses them.
    let mut linear = control_json();
    linear["execution_mode"] = json!("linear_steps");
    linear["max_steps"] = json!(1);
    let linear: TaskControl = serde_json::from_value(linear).expect("linear control");
    let detector = PageDetector::new(page_set()).expect("page detector");
    for (member, value) in [
        ("when", json!({"present": "flag/ready"})),
        ("on_guard_refused", json!("next_candidate")),
    ] {
        let mut operation = claim(json!({}));
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
            Some(format!("reason={member} operation=claim").as_str())
        );
    }
}

// FCM E-04 「同一页可以挂多条有序的候选操作，每条带 when（某目标在或不在）」: the first candidate whose
// `when` holds on the frame runs; one whose `when` does not hold starts no step and takes no
// step index.
#[test]
fn exec1_the_first_candidate_whose_when_holds_runs() {
    let operations = || {
        vec![
            claim(json!({"when": {"present": "flag/ready"}})),
            skip(json!({"when": {"absent": "flag/ready"}})),
        ]
    };

    let mut runtime = ScriptedRuntime::new(vec![Screen::Home, Screen::Terminal]);
    let outcome = succeeded(task(operations()).run(&mut runtime));
    assert_eq!(outcome.final_page.as_deref(), Some("neutral/terminal"));
    assert_eq!(outcome.executed_steps, 1);
    assert_eq!(runtime.inputs, 1);
    assert_eq!(
        runtime.before_first_input(),
        vec![
            Event::When("flag/ready".to_owned()),
            started("claim", 0),
            Event::Guard,
            Event::Intent("claim".to_owned()),
            Event::Input,
        ]
    );

    let mut runtime = ScriptedRuntime::new(vec![Screen::HomeBusy, Screen::Other]);
    let outcome = succeeded(task(operations()).run(&mut runtime));
    assert_eq!(outcome.final_page.as_deref(), Some("neutral/other"));
    assert_eq!(outcome.executed_steps, 1);
    assert_eq!(runtime.inputs, 1);
    assert_eq!(
        runtime.before_first_input(),
        vec![
            Event::When("flag/ready".to_owned()),
            Event::When("flag/ready".to_owned()),
            started("skip", 0),
            Event::Guard,
            Event::Intent("skip".to_owned()),
            Event::Input,
        ]
    );
}

// FCM E-04 「守卫拒绝时可以声明落到下一条候选」: a guard that refuses on the step's frame ends that
// step without an input, and the page's next candidate runs on the same frame as the next step;
// without the declaration the refusal still ends the run.
#[test]
fn exec1_a_declared_guard_refusal_falls_to_the_next_candidate_on_the_same_frame() {
    let operations = |extra: Value| vec![claim(extra), skip(json!({}))];

    let mut runtime = ScriptedRuntime::new(vec![Screen::HomeBusy, Screen::Other]);
    let outcome = succeeded(
        task(operations(json!({"on_guard_refused": "next_candidate"}))).run(&mut runtime),
    );
    assert_eq!(outcome.final_page.as_deref(), Some("neutral/other"));
    // The step passed over stays counted.
    assert_eq!(outcome.executed_steps, 2);
    assert_eq!(runtime.inputs, 1);
    // No frame is captured between the refusal and the next candidate's step.
    assert_eq!(
        runtime.before_first_input(),
        vec![
            started("claim", 0),
            Event::Guard,
            finished("claim", HOME),
            started("skip", 1),
            Event::Guard,
            Event::Intent("skip".to_owned()),
            Event::Input,
        ]
    );
    assert_eq!(
        runtime.started(),
        vec![("claim".to_owned(), 0), ("skip".to_owned(), 1)]
    );

    // On a frame where the guard passes, the first candidate runs.
    let mut runtime = ScriptedRuntime::new(vec![Screen::Home, Screen::Terminal]);
    let outcome = succeeded(
        task(operations(json!({"on_guard_refused": "next_candidate"}))).run(&mut runtime),
    );
    assert_eq!(outcome.final_page.as_deref(), Some("neutral/terminal"));
    assert_eq!(runtime.started(), vec![("claim".to_owned(), 0)]);

    // Without the declaration a guard refusal ends the run before any input.
    let mut runtime = ScriptedRuntime::new(vec![Screen::HomeBusy, Screen::Other]);
    let error = task_failure(task(operations(json!({}))).run(&mut runtime));
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::ContainedTaskGuardRefused.as_str()
    );
    assert_eq!(runtime.inputs, 0);
}

// FCM E-04 「全部不成立才失败」: a page whose candidates were all passed over on the frame ends the run
// with `contained_task_operation_candidates_exhausted`, its page and how many were passed over,
// and no input.
#[test]
fn exec1_a_page_whose_candidates_all_fail_ends_the_run_without_an_input() {
    let operations = vec![
        claim(json!({"when": {"present": "page/home"}, "on_guard_refused": "next_candidate"})),
        skip(json!({"when": {"present": "flag/ready"}})),
    ];
    let mut runtime = ScriptedRuntime::new(vec![Screen::HomeBusy]);
    let error = task_failure(task(operations).run(&mut runtime));
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::ContainedTaskOperationCandidatesExhausted.as_str()
    );
    assert_eq!(
        Value::Object(error.outcome().values().clone()),
        json!({"page_id": HOME, "count": 2})
    );
    assert_eq!(runtime.inputs, 0);
    assert_eq!(
        runtime.before_first_input(),
        vec![
            Event::When("page/home".to_owned()),
            started("claim", 0),
            Event::Guard,
            finished("claim", HOME),
            Event::When("flag/ready".to_owned()),
        ]
    );
}

// FCM E-04 (a later candidate may run instead of an earlier one): admission follows every
// candidate that may run from a page, in the page-graph reachability check and in the phased
// route check, so each terminal page they reach needs its mapping; a page left without a
// candidate after such candidates only ends that path.
#[test]
fn exec1_admission_follows_every_candidate_that_may_run() {
    let claimed_only = json!({
        "designated_operation": "claim",
        "mappings": [
            {"outcome_key": "claimed", "effect": "designated_effect_completed",
             "terminal_pages": ["terminal"]}
        ]
    });
    let both = json!({
        "designated_operation": "claim",
        "mappings": [
            {"outcome_key": "claimed", "effect": "designated_effect_completed",
             "terminal_pages": ["terminal"]},
            {"outcome_key": "skipped", "effect": "no_designated_effect",
             "terminal_pages": ["other"]}
        ]
    });
    let scheduling = |value: &Value| -> SchedulingOutcomeDeclaration {
        serde_json::from_value(value.clone()).expect("scheduling outcome declaration")
    };
    let coverage = |operations: Vec<Value>, outcome: &Value| {
        let operations = operations
            .into_iter()
            .map(|operation| serde_json::from_value::<TaskOperation>(operation).expect("operation"))
            .collect::<Vec<_>>();
        validate_scheduling_outcome_coverage(
            Some(HOME),
            "neutral",
            &["terminal".to_owned(), "other".to_owned()],
            &[
                HOME.to_owned(),
                "neutral/terminal".to_owned(),
                "neutral/other".to_owned(),
            ],
            &operations,
            &scheduling(outcome),
        )
    };
    let incomplete = ExecutionKernelCode::ContainedTaskOutcomeDeclarationIncomplete.as_str();
    for first in [
        claim(json!({"when": {"present": "flag/ready"}})),
        claim(json!({"on_guard_refused": "next_candidate"})),
    ] {
        // `skip` may run instead of `claim`, so `other` without a designated effect is reached.
        assert_eq!(
            refused_code(coverage(
                vec![first.clone(), skip(json!({}))],
                &claimed_only
            )),
            incomplete,
            "{first}"
        );
        coverage(vec![first.clone(), skip(json!({}))], &both).expect("both terminals mapped");
        // Both candidates conditional: a frame on which neither holds ends the run, not a path.
        coverage(
            vec![first, skip(json!({"when": {"absent": "flag/ready"}}))],
            &both,
        )
        .expect("a page that may be left without a candidate");
    }

    // The phased route check, one stage from `home` to `terminal` or `other`.
    let phases = vec![TaskPhase {
        id: "stage".to_owned(),
        operations: vec!["claim".to_owned(), "skip".to_owned()],
        target_pages: vec!["terminal".to_owned(), "other".to_owned()],
    }];
    let phased = |claim_conditional: bool, skip_conditional: bool, outcome: &Value| {
        let operation = |id, to: &str, may_be_passed_over| TaskPhaseOperation {
            id,
            from: "home",
            destinations: vec![to.to_owned()],
            retryable: false,
            may_be_passed_over,
        };
        actingcommand_contract::validate_phased_route(
            "neutral",
            &[TaskPhaseEntry {
                phase: 0,
                page: "home",
            }],
            &phases,
            &["terminal".to_owned(), "other".to_owned()],
            &[
                operation("claim", "terminal", claim_conditional),
                operation("skip", "other", skip_conditional),
            ],
            &scheduling(outcome),
        )
    };
    assert_eq!(
        phased(true, false, &claimed_only).map_err(|code| code.as_str()),
        Err(TaskflowCode::TaskPhasesOutcomeIncomplete.as_str())
    );
    phased(true, false, &both).expect("both terminals mapped");
    phased(true, true, &both).expect("a stage page that may be left without a candidate");
}
