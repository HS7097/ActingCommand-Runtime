// SPDX-License-Identifier: AGPL-3.0-only

// #411 A45 (0.12.8), T2: an entry check that needs the package's entry pages, asked of a package
// that declares none it can check, ends with the specific `contained_task_entry_pages_missing`
// (error), not the generic `contained_task_state_invalid`. Requirement 3 of the slice brief:
// `contained_task_state_invalid` becomes specific where it is raised for a missing entry-page set
// (a code that names the missing declaration), apart from an illegal pacing factor; Workflow
// #411's A45 ruling 「至少分清『入口页声明取不到』和『pacing 系数非法』」.
//
// The packages are application entry packages (R24, `entry_page` / first step `any`), a
// page-graph one and a `linear_steps` one, built as admission builds them. Each entry check a
// prerequisite gate or the navigated entry gate asks (`recognize_gate_entry`,
// `recognize_navigation_entry`, `await_gate_entry`, `await_navigation_entry`) runs on a fresh
// runtime that counts what the kernel asks of it: the check fails before any capture or effect.

use super::*;
use actingcommand_contract::TaskTimingBudgetOrigin;
use actingcommand_contract::outcome::Category;
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition_pack::RecognitionPack;
use serde_json::{Value, json};
use std::path::PathBuf;

const STEP_TIMEOUT_MS: u64 = 5_000;
const ENTRY_PAGES_MISSING: &str = "contained_task_entry_pages_missing";

// The runtime as an entry check sees it: one `home` frame per capture, every capture, input and
// application effect counted, every trace accepted.
#[derive(Default)]
struct CountingRuntime {
    captures: usize,
    inputs: usize,
    applications: usize,
}

impl ContainedTaskRuntime for CountingRuntime {
    type Error = &'static str;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        self.captures += 1;
        let frame = Frame::from_pixels(
            2,
            1,
            [[255, 0, 0], [0, 0, 0]].concat(),
            PixelFormat::Rgb8,
            CaptureBackendName::FixtureSimulation,
        )
        .expect("fixture frame");
        Ok(frame.into())
    }

    fn input(
        &mut self,
        _action: InputAction,
        _frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        self.inputs += 1;
        Ok(())
    }

    fn supports_application_effect(&self) -> bool {
        true
    }

    fn control_application(
        &mut self,
        _action: ApplicationLifecycleAction,
    ) -> Result<ApplicationEffectSupport, Self::Error> {
        self.applications += 1;
        Ok(ApplicationEffectSupport::Performed)
    }

    fn record(&mut self, _trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        Ok(())
    }
}

// The package's control; a `linear_steps` package declares its one step as its step count.
fn control(mode: &str) -> TaskControl {
    let max_steps = if mode == linear::LINEAR_STEPS { 1 } else { 2 };
    serde_json::from_value(json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.test.application_entry",
        "execution_mode": mode,
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": STEP_TIMEOUT_MS,
        "timeout_ms": 60_000,
        "max_steps": max_steps
    }))
    .expect("task control")
}

fn pack() -> RecognitionPack {
    serde_json::from_value(json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            {"type": "color", "id": "page/home",
             "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [255, 0, 0]},
            {"type": "color", "id": "page/title",
             "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [255, 255, 0]}
        ]
    }))
    .expect("recognition pack")
}

fn page_set() -> PageSet {
    serde_json::from_value(json!({
        "schema_version": "0.3",
        "pages": [
            {"id": "neutral/home", "required": ["page/home"]},
            {"id": "neutral/title", "required": ["page/title"]}
        ]
    }))
    .expect("page set")
}

// The package's task: entry `any`, the application restarted from any screen to `title`, then
// (page graph only) a click from `title` to the terminal page `home`.
fn program(linear: bool) -> Value {
    let mut operations = vec![json!({
        "id": "restart", "from": "any", "to": "title",
        "application": {"action": "restart"}
    })];
    if !linear {
        operations.push(json!({
            "id": "enter", "from": "title", "to": "home",
            "click": {"kind": "point", "x": 0, "y": 0},
            "unguarded_trusted_coordinate": true
        }));
    }
    let target = if linear { "title" } else { "home" };
    let mut task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "entry_page": "any",
        "target_page": target,
        "operations": operations
    });
    if linear {
        task["max_steps"] = json!(1);
    }
    task
}

// An application entry package of `mode` as admission prepares it: no required entry pages.
fn package(mode: &str) -> PreparedContainedTask {
    let control = control(mode);
    let program: TaskProgram =
        serde_json::from_value(program(mode == linear::LINEAR_STEPS)).expect("task program");
    let evaluator = RecognitionEvaluator::new(PathBuf::new(), pack()).expect("evaluator");
    let detector = PageDetector::new(page_set()).expect("page detector");
    detector
        .validate(&evaluator)
        .expect("page detector targets");
    if mode == linear::LINEAR_STEPS {
        program
            .validate_linear(&control, &detector)
            .expect("an admitted linear application entry");
    } else {
        for operation in &program.operations {
            operation
                .validate(&control, TaskOperationDefaults::default(), "0.9")
                .expect("valid operation");
        }
    }
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

type CheckResult = Result<(), ContainedTaskRunError<&'static str>>;

// The four entry checks of a gate and of the navigated entry gate, each on a fresh runtime: its
// name, its result and the runtime it ran on.
fn entry_checks(
    package: &PreparedContainedTask,
) -> Vec<(&'static str, CheckResult, CountingRuntime)> {
    let budget = Duration::from_millis(STEP_TIMEOUT_MS);
    let mut checks = Vec::new();

    let mut runtime = CountingRuntime::default();
    let result = package.recognize_gate_entry(&mut runtime).map(|_| ());
    checks.push(("recognize_gate_entry", result, runtime));

    let mut runtime = CountingRuntime::default();
    let result = package.recognize_navigation_entry(&mut runtime).map(|_| ());
    checks.push(("recognize_navigation_entry", result, runtime));

    let mut runtime = CountingRuntime::default();
    let result = package
        .await_gate_entry(&mut runtime, budget, TaskTimingBudgetOrigin::Task, 0, None)
        .map(|_| ());
    checks.push(("await_gate_entry", result, runtime));

    let mut runtime = CountingRuntime::default();
    let result = package
        .await_navigation_entry(&mut runtime, budget, TaskTimingBudgetOrigin::Task)
        .map(|_| ());
    checks.push(("await_navigation_entry", result, runtime));

    checks
}

// T2: each entry check of an application entry package, page graph or `linear_steps`, fails
// with `contained_task_entry_pages_missing` (error), never `contained_task_state_invalid`, and
// asks nothing of the runtime.
#[test]
fn a45_an_entry_check_of_a_package_without_entry_pages_names_the_missing_entry_pages() {
    for mode in ["navigable_route", linear::LINEAR_STEPS] {
        let prepared = package(mode);
        assert!(
            prepared.has_application_entry(),
            "{mode}: the fixture is an application entry package"
        );
        for (check, result, runtime) in entry_checks(&prepared) {
            let error = match result {
                Err(ContainedTaskRunError::Task(error)) => error,
                other => panic!("{mode} {check}: expected a task failure, got {other:?}"),
            };
            assert_eq!(
                error.code().as_str(),
                ENTRY_PAGES_MISSING,
                "{mode} {check}: {error}"
            );
            let ContainedTaskCode::Registered(code) = error.code() else {
                panic!("{mode} {check}: an unregistered code");
            };
            assert_eq!(code.category(), Category::Error, "{mode} {check}");
            assert_eq!(
                (runtime.captures, runtime.inputs, runtime.applications),
                (0, 0, 0),
                "{mode} {check}: nothing is captured, input or effected"
            );
        }
    }
}
