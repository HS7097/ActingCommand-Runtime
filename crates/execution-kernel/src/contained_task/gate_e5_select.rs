// SPDX-License-Identifier: AGPL-3.0-only

// Test plan v2.2 E-5 G7 (exec key 4): a single-frame select step, as contracts/selection-graph.md
// "Select step", "Execution" states it: the step's frame F1 is projected and the policy decides
// over its actionable candidates; only a `selected` outcome captures a confirmation frame F2,
// which must be the step's page, pass the step's guard and project to the same candidate set
// (P2 hashes to H1); the decision is recorded once as `task.selection_evaluated`; on a match one
// tap is sampled inside the chosen candidate's `click` rectangle; any other outcome aborts with
// `selection_not_selected` and detail `<kind>:<outcome_key>`, and every selection failure comes
// before the input, with neither retry nor recovery. Requirement basis:
// rd-1009/tests/req-basis-el.md section E-5, row "G7 select" (single-frame select only; the scan
// form's "confirm only the chosen item" belongs to list selection). The policy-hash admission row
// of the plan is not written: Workflow #336 PKG-S2 (option 丙) ignores a policy's `sha256`.
//
// A frame is four pixels: the page (red `home`, blue `terminal`), the guard (green when it holds)
// and the two slots of the layout, each green when its candidate is ready. The policy selects the
// one ready candidate.

use super::*;
use actingcommand_contract::{
    InstanceFactContext, InstanceFactSnapshot, TaskSelectionConfirmation, TaskSelectionOutcome,
    TaskSelectionRecord,
};
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition_pack::RecognitionPack;
use serde_json::json;
use std::path::PathBuf;

const RED: [u8; 3] = [255, 0, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const GREEN: [u8; 3] = [0, 255, 0];
const BLACK: [u8; 3] = [0, 0, 0];

// One screen: its page, whether the guard holds, and which of the two slots are ready.
#[derive(Debug, Clone, Copy)]
struct Screen {
    page: [u8; 3],
    guard: bool,
    ready: [bool; 2],
}

const fn home(ready: [bool; 2]) -> Screen {
    Screen {
        page: RED,
        guard: true,
        ready,
    }
}

const TERMINAL: Screen = Screen {
    page: BLUE,
    guard: true,
    ready: [false, false],
};

fn frame(screen: Screen) -> Frame {
    let lit = |on: bool| if on { GREEN } else { BLACK };
    Frame::from_pixels(
        4,
        1,
        [
            screen.page,
            lit(screen.guard),
            lit(screen.ready[0]),
            lit(screen.ready[1]),
        ]
        .concat(),
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
}

#[derive(Debug)]
enum Event {
    Capture,
    StateRead,
    StepStarted,
    Selection(Box<TaskSelectionRecord>),
    Intent,
    Input(InputAction),
}

// The Runtime as the kernel sees it: it shows F1 until the select step reads its fact snapshot,
// then F2 until the first input, then `terminal`.
struct Device {
    first: Screen,
    confirmation: Screen,
    state_read: bool,
    inputs: usize,
    events: Vec<Event>,
}

impl Device {
    fn new(first: Screen, confirmation: Screen) -> Self {
        Self {
            first,
            confirmation,
            state_read: false,
            inputs: 0,
            events: Vec::new(),
        }
    }

    fn records(&self) -> Vec<&TaskSelectionRecord> {
        self.events
            .iter()
            .filter_map(|event| match event {
                Event::Selection(record) => Some(record.as_ref()),
                _ => None,
            })
            .collect()
    }

    fn taps(&self) -> Vec<&InputAction> {
        self.events
            .iter()
            .filter_map(|event| match event {
                Event::Input(action) => Some(action),
                _ => None,
            })
            .collect()
    }

    fn count(&self, wanted: fn(&Event) -> bool) -> usize {
        self.events.iter().filter(|event| wanted(event)).count()
    }

    // The events from the snapshot read on, without their contents.
    fn after_state_read(&self) -> Vec<&'static str> {
        self.events
            .iter()
            .skip_while(|event| !matches!(event, Event::StateRead))
            .map(|event| match event {
                Event::Capture => "capture",
                Event::StateRead => "state read",
                Event::StepStarted => "step started",
                Event::Selection(_) => "selection evaluated",
                Event::Intent => "effect intent",
                Event::Input(_) => "input",
            })
            .collect()
    }
}

impl ContainedTaskRuntime for Device {
    type Error = &'static str;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        self.events.push(Event::Capture);
        let screen = if self.inputs > 0 {
            TERMINAL
        } else if self.state_read {
            self.confirmation
        } else {
            self.first
        };
        Ok(frame(screen).into())
    }

    fn selection_state(
        &mut self,
        request: SelectionStateRequest,
    ) -> Result<SelectionState, Self::Error> {
        self.state_read = true;
        self.events.push(Event::StateRead);
        Ok(SelectionState::Snapshot {
            snapshot: InstanceFactSnapshot {
                snapshot_id: "snapshot:test:empty".to_owned(),
                ledger_position: 1,
                context: InstanceFactContext {
                    instance_id: "neutral.instance".to_owned(),
                    server_id: request.server,
                    game_id: request.game,
                },
                records: Vec::new(),
            },
            now_unix_ms: 1_000_000,
        })
    }

    fn input(
        &mut self,
        action: InputAction,
        _frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        self.inputs += 1;
        self.events.push(Event::Input(action));
        Ok(())
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        let event = match trace {
            ContainedTaskTrace::StepStarted { .. } => Event::StepStarted,
            ContainedTaskTrace::SelectionEvaluated { selection, .. } => Event::Selection(selection),
            ContainedTaskTrace::EffectIntent { .. } => Event::Intent,
            _ => return Ok(()),
        };
        self.events.push(event);
        Ok(())
    }
}

fn pack() -> RecognitionPack {
    let color = |id: &str, x: u32, expected: [u8; 3]| {
        json!({"type": "color", "id": id,
               "region": {"x": x, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    let slot = |x: u32, target: &str| {
        json!({"rect": {"x": x, "y": 0, "width": 1, "height": 1},
               "click": {"x": x, "y": 0, "width": 1, "height": 1},
               "targets": {"ready": target}})
    };
    serde_json::from_value(json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 4, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            color("page/home", 0, RED),
            color("page/terminal", 0, BLUE),
            color("guard/ready", 1, GREEN),
            color("slot/first_ready", 2, GREEN),
            color("slot/second_ready", 3, GREEN)
        ],
        "candidate_layouts": [{
            "id": "choices",
            "page_id": "neutral/home",
            "kind": "fixed_slots",
            "features": [{"name": "ready", "value": "passed"}],
            "slots": [slot(2, "slot/first_ready"), slot(3, "slot/second_ready")]
        }]
    }))
    .expect("recognition pack")
}

// One select step from `home` to `terminal` that declares a retry, over the layout above.
fn select_task() -> PreparedContainedTask {
    let control: TaskControl = serde_json::from_value(json!({
        "schema_version": "Lab-1y.control.v1",
        "package_id": "neutral.test.select",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 4, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 5_000,
        "timeout_ms": 30_000
    }))
    .expect("task control");
    let policy: actingcommand_selection_policy::SelectionPolicy = serde_json::from_value(json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "policy.ready",
        "applies_to": {
            "candidate_layout_id": "choices",
            "outcome_keys": {"selected": "ready_selected", "empty": "ready_empty",
                             "insufficient": "ready_insufficient",
                             "ambiguous": "ready_ambiguous", "unknown": "ready_unknown"}
        },
        "fields": [{"name": "ready", "value_type": {"type": "boolean"}}],
        "facts": [],
        "gates": [{"gate_id": "ready",
                   "predicate": {"kind": "boolean_equals",
                                 "value": {"source": "field", "field": "ready"},
                                 "expected": true},
                   "on_unknown": {"kind": "drop_candidate"}}],
        "scoring": [],
        "selection": {"mode": "exactly_one", "required_count": 1},
        "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
    }))
    .expect("selection policy");
    policy.validate().expect("valid selection policy");
    let mut operation: TaskOperation = serde_json::from_value(json!({
        "id": "pick",
        "from": "home",
        "to": "terminal",
        "guard": {
            "page_id": "home",
            "target_id": "guard/ready",
            "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
            "color_probe": "guard/ready"
        },
        "select": {"layout_id": "choices", "policy": {"path": "policies/select.json"}},
        "retryable": true,
        "max_attempts": 2,
        "retry_interval_ms": 1
    }))
    .expect("select operation");
    operation.prepared_select = Some(Box::new(selection::PreparedSelect::for_test(
        "choices", policy,
    )));
    let mut program: TaskProgram = serde_json::from_value(json!({
        "schema_version": "0.6",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 4, "height": 1},
        "target_page": "terminal",
        "operations": []
    }))
    .expect("task program");
    program.operations.push(operation);
    let evaluator =
        RecognitionEvaluator::new(PathBuf::new(), pack()).expect("recognition evaluator");
    let page_set: PageSet = serde_json::from_value(json!({
        "schema_version": "0.3",
        "pages": [
            {"id": "neutral/home", "required": ["page/home"]},
            {"id": "neutral/terminal", "required": ["page/terminal"]}
        ]
    }))
    .expect("page set");
    let detector = PageDetector::new(page_set).expect("page detector");
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

fn failed(
    result: Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>,
) -> ContainedTaskError {
    match result {
        Err(ContainedTaskRunError::Task(error)) => error,
        Err(other) => panic!("expected a task failure, got {other:?}"),
        Ok(outcome) => panic!("expected a task failure, the run ended {outcome:?}"),
    }
}

// A failed attempt: one step start, one decision, no effect intent and no input.
fn assert_aborted_before_the_input(device: &Device) {
    assert_eq!(
        device.count(|event| matches!(event, Event::StepStarted)),
        1,
        "no retry: {:?}",
        device.events
    );
    assert_eq!(device.records().len(), 1, "{:?}", device.events);
    assert_eq!(
        device.count(|event| matches!(event, Event::Intent)),
        0,
        "{:?}",
        device.events
    );
    assert!(device.taps().is_empty(), "{:?}", device.events);
}

// selection-graph.md "Execution" steps 1-6: the ready candidate is chosen on F1, F2 is captured
// after the decision's inputs were read, P2 hashes to H1 (`matched` with F1's candidate set), the
// decision is recorded once before the effect intent, and the one tap lies inside the chosen
// candidate's click rectangle (pixel 2 for the first slot, pixel 3 for the second).
#[test]
fn gate_select_a_selected_candidate_is_confirmed_on_a_fresh_frame_and_tapped_inside_its_click() {
    for (ready, x) in [([true, false], 2), ([false, true], 3)] {
        let mut device = Device::new(home(ready), home(ready));

        let outcome = select_task()
            .run(&mut device)
            .unwrap_or_else(|error| panic!("{ready:?}: the run fails: {error:?}"));

        assert_eq!(outcome.outcome, TaskOutcome::Success, "{ready:?}");
        assert_eq!(
            device.after_state_read()[..5],
            [
                "state read",
                "capture",
                "selection evaluated",
                "effect intent",
                "input"
            ],
            "{ready:?}: {:?}",
            device.events
        );
        let records = device.records();
        let [record] = records.as_slice() else {
            panic!("{ready:?}: one decision record: {:?}", device.events);
        };
        assert_eq!(
            record.selection_result,
            TaskSelectionOutcome::Selected { count: 1 },
            "{ready:?}"
        );
        assert_eq!(record.outcome_key, "ready_selected");
        assert_eq!(record.selected.len(), 1, "{ready:?}");
        match &record.confirmation {
            TaskSelectionConfirmation::Matched {
                candidate_set_sha256,
                ..
            } => assert_eq!(
                candidate_set_sha256,
                record.projection.candidate_set_sha256(),
                "{ready:?}: P2 hashes to H1"
            ),
            other => panic!("{ready:?}: a matched confirmation, got {other:?}"),
        }
        assert_eq!(device.taps(), [&InputAction::Tap { x, y: 0 }], "{ready:?}");
    }
}

// selection-graph.md "Execution", failure table: a confirmation frame whose projection hashes
// differently (the other slot is now the ready one) is `selection_projection_mismatch`, recorded
// `mismatched`; nothing is tapped and the declared retry is not taken.
#[test]
fn gate_select_a_confirmation_frame_with_other_candidates_taps_nothing() {
    let mut device = Device::new(home([true, false]), home([false, true]));

    let error = failed(select_task().run(&mut device));

    assert_eq!(
        error.code().as_str(),
        "selection_projection_mismatch",
        "{error}"
    );
    assert_aborted_before_the_input(&device);
    assert!(
        matches!(
            device.records()[0].confirmation,
            TaskSelectionConfirmation::Mismatched { .. }
        ),
        "{:?}",
        device.records()[0].confirmation
    );
}

// selection-graph.md "Execution", step 4 and the failure table: F2 must be the step's page; a
// confirmation frame of another page is `selection_page_changed`, recorded `page_changed`, and
// nothing is tapped.
#[test]
fn gate_select_a_confirmation_frame_of_another_page_taps_nothing() {
    let mut device = Device::new(home([true, false]), TERMINAL);

    let error = failed(select_task().run(&mut device));

    assert_eq!(error.code().as_str(), "selection_page_changed", "{error}");
    assert_aborted_before_the_input(&device);
    assert_eq!(
        device.records()[0].confirmation,
        TaskSelectionConfirmation::PageChanged
    );
}

// selection-graph.md "Execution", step 4 and the failure table: the step's guard is judged on F2;
// when it fails there the step fails with the guard's own code, recorded `guard_failed` with that
// code, and nothing is tapped.
#[test]
fn gate_select_a_guard_that_fails_on_the_confirmation_frame_taps_nothing() {
    let confirmation = Screen {
        guard: false,
        ..home([true, false])
    };
    let mut device = Device::new(home([true, false]), confirmation);

    let error = failed(select_task().run(&mut device));

    assert_aborted_before_the_input(&device);
    match &device.records()[0].confirmation {
        TaskSelectionConfirmation::GuardFailed { outcome } => {
            assert_eq!(outcome.code().as_str(), error.code().as_str(), "{error}");
        }
        other => panic!("a guard_failed confirmation, got {other:?}"),
    }
}

// selection-graph.md "Execution": "Any outcome other than `selected` ... is the v1 fallback, an
// abort: the step fails with `selection_not_selected` and detail `<kind>:<outcome_key>`", recorded
// `not_attempted`; only a `selected` outcome captures a confirmation frame, and the declared
// retry is not taken. With no slot ready the gate rejects both candidates, so no candidate
// survives; selection-policy.md "Selection modes": `exactly_one` is `top_k` with a required
// count of one, and "Fewer is `insufficient`" (`empty` belongs to `none_allowed`), so the
// outcome is `insufficient` with no survivor of the one required.
#[test]
fn gate_select_an_outcome_other_than_selected_aborts_without_a_confirmation_or_a_retry() {
    let mut device = Device::new(home([false, false]), home([false, false]));

    let error = failed(select_task().run(&mut device));

    assert_eq!(error.code().as_str(), "selection_not_selected", "{error}");
    assert_eq!(error.detail(), Some("insufficient:ready_insufficient"));
    assert_aborted_before_the_input(&device);
    let record = device.records()[0];
    assert_eq!(
        record.selection_result,
        TaskSelectionOutcome::Insufficient {
            surviving: 0,
            required: 1
        }
    );
    assert_eq!(record.confirmation, TaskSelectionConfirmation::NotAttempted);
    // The decision directly follows the snapshot read: no confirmation frame was captured.
    assert_eq!(
        device.after_state_read()[..2],
        ["state read", "selection evaluated"],
        "{:?}",
        device.events
    );
}
