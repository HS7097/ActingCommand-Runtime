// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #390 EXEC-3 (forced-compromises mapping row E-02; FCM §2.4 row EXEC-3; coordinator
// ruling 5 of wave 14): a graph-mode recovery or startup package may use the R24 `application`
// effect. In a run the host starts itself (a startup package, a recovery ladder rung), a
// page-graph package whose `entry_page` is `any` and whose first operation is a `from: "any"`
// application effect begins with that effect, before any capture, as a `linear_steps`
// application entry does (contracts/linear-steps.md, "Application steps"): a page the screen
// already shows, the terminal page included, does not skip it, and such a rung needs no capture
// before it. Every other run keeps the page-graph rule of slice #316-B3
// (contracts/application-lifecycle.md): the run takes its initial frame first, and an already
// terminal page ends the run without the effect.
//
// Every expected value comes from FCM E-02, the R24 rows of contracts/linear-steps.md and the
// #316-B3 rule of contracts/application-lifecycle.md.

use super::*;
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition_pack::RecognitionPack;
use serde_json::{Value, json};
use std::path::PathBuf;

const STEP_TIMEOUT: Duration = Duration::from_secs(10);
const TASK_TIMEOUT: Duration = Duration::from_secs(60);

// What the screen shows: the terminal page `home`, or the `title` the application shows after a
// restart. The left pixel names the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Home,
    Title,
}

fn frame(screen: Screen) -> Frame {
    let page = match screen {
        Screen::Home => [255, 0, 0],
        Screen::Title => [255, 255, 0],
    };
    Frame::from_pixels(
        2,
        1,
        [page, [0, 0, 0]].concat(),
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Event {
    Capture,
    // The operation and the page its step started from.
    StepStarted(String, String),
    Application(ApplicationLifecycleAction),
    Input,
    // The operation and the page its step finished on.
    StepFinished(String, String),
}

// The device as the kernel sees it: `home` until the application restarts, `title` after the
// restart, `home` again after the next input.
struct ScriptedRuntime {
    screen: Screen,
    events: Vec<Event>,
}

impl ScriptedRuntime {
    fn showing(screen: Screen) -> Self {
        Self {
            screen,
            events: Vec::new(),
        }
    }

    // The events before the first capture.
    fn before_first_capture(&self) -> Vec<Event> {
        let end = self
            .events
            .iter()
            .position(|event| *event == Event::Capture)
            .unwrap_or(self.events.len());
        self.events[..end].to_vec()
    }
}

impl ContainedTaskRuntime for ScriptedRuntime {
    type Error = &'static str;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        Ok(frame(self.screen).into())
    }

    fn input(
        &mut self,
        _action: InputAction,
        _frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        self.events.push(Event::Input);
        self.screen = Screen::Home;
        Ok(())
    }

    fn supports_application_effect(&self) -> bool {
        true
    }

    fn control_application(
        &mut self,
        action: ApplicationLifecycleAction,
    ) -> Result<ApplicationEffectSupport, Self::Error> {
        self.events.push(Event::Application(action));
        self.screen = Screen::Title;
        Ok(ApplicationEffectSupport::Performed)
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        match trace {
            ContainedTaskTrace::CaptureCompleted { .. } => self.events.push(Event::Capture),
            ContainedTaskTrace::StepStarted {
                operation_label,
                from_page,
                ..
            } => self
                .events
                .push(Event::StepStarted(operation_label, from_page)),
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

fn control() -> TaskControl {
    serde_json::from_value(json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.application.entry",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": STEP_TIMEOUT.as_millis() as u64,
        "timeout_ms": TASK_TIMEOUT.as_millis() as u64
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

// The recovery package: restart the assigned application from any screen, then enter from its
// title to the terminal page `home`; its entry page is `entry_page`.
fn program(entry_page: &str) -> TaskProgram {
    let operations: Vec<Value> = vec![
        json!({"id": "restart", "from": "any", "to": "title",
               "application": {"action": "restart"}}),
        json!({"id": "enter", "from": "title", "to": "home",
               "click": {"kind": "point", "x": 0, "y": 0},
               "unguarded_trusted_coordinate": true}),
    ];
    serde_json::from_value(json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "entry_page": entry_page,
        "target_page": "home",
        "operations": operations
    }))
    .expect("task program")
}

fn task(entry_page: &str) -> PreparedContainedTask {
    let control = control();
    let program = program(entry_page);
    for operation in &program.operations {
        operation
            .validate(&control, TaskOperationDefaults::default(), "0.9")
            .expect("valid operation");
    }
    let evaluator = RecognitionEvaluator::new(PathBuf::new(), pack()).expect("evaluator");
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

// FCM E-02 「图模式的恢复包、启动包首个操作可以用 R24 的 application 效果（restart）」; R24 (linear-steps.md,
// "Application steps"): the application entry recognizes nothing before its effect. A run the
// host starts itself restarts the application although the screen already shows the terminal
// page, and the run's first frame is the one after the restart.
#[test]
fn exec3_a_host_package_run_begins_with_its_application_entry_whatever_the_screen_shows() {
    let task = task("any");
    let mut runtime = ScriptedRuntime::showing(Screen::Home);
    let outcome = succeeded(task.run_host_package(&mut runtime));

    assert_eq!(
        runtime.before_first_capture(),
        vec![
            Event::StepStarted("restart".to_owned(), "<unrecognized>".to_owned()),
            Event::Application(ApplicationLifecycleAction::Restart),
        ],
        "the effect comes before any capture: {:?}",
        runtime.events
    );
    // The run's pages and operations take over from the frame the restart arrived on.
    let steps = runtime
        .events
        .iter()
        .filter(|event| matches!(event, Event::StepFinished(..) | Event::Input))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        steps,
        vec![
            Event::StepFinished("restart".to_owned(), "neutral/title".to_owned()),
            Event::Input,
            Event::StepFinished("enter".to_owned(), "neutral/home".to_owned()),
        ]
    );
    assert_eq!(outcome.executed_steps, 2);
    assert_eq!(outcome.final_page.as_deref(), Some("neutral/home"));
}

// contracts/application-lifecycle.md (#316-B3): every other run takes its initial frame first,
// and a terminal page already shown ends the run there, without the effect.
#[test]
fn exec3_another_run_keeps_the_initial_frame_before_the_application_entry() {
    let task = task("any");
    let mut runtime = ScriptedRuntime::showing(Screen::Home);
    let outcome = succeeded(task.run(&mut runtime));

    assert_eq!(runtime.events.first(), Some(&Event::Capture));
    assert!(
        !runtime
            .events
            .iter()
            .any(|event| matches!(event, Event::Application(_) | Event::Input)),
        "an already terminal page ends the run without the effect: {:?}",
        runtime.events
    );
    assert_eq!(outcome.executed_steps, 0);
    assert_eq!(outcome.final_page.as_deref(), Some("neutral/home"));
}

// R24 for a rung (contracts/emulator-control.md, R1/R2 channel eligibility): an application
// entry needs neither capture nor input before its effect, a page-graph one as a linear one;
// a package with a page entry still needs capture for it.
#[test]
fn exec3_a_rung_needs_no_capture_before_a_page_graph_application_entry() {
    assert_eq!(task("any").recovery_entry_channels(), (false, false));
    let (capture, _) = task("title").recovery_entry_channels();
    assert!(capture, "a page entry still needs capture");
}
