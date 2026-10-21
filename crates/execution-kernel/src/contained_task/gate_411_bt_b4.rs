// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 B4, slice BT-B4 (exec key 165; register §1.166; B-train plan §2.4 with D3, test
// rows BT-B4 T1 to T3 of §5.3; ledger revision 4, register §1.153 LR-1). A single-frame select
// that declares `confirm: "decision"` projects its confirmation frame F2, decides it again with
// the same fact snapshot and instant, and compares only the decisions: the actionable
// candidates' IDs, click rectangles and statuses, the selected IDs and the outcome key, never a
// feature value. An equal decision records `decision_matched` and taps; another one records
// `decision_mismatched` and fails `selection_decision_mismatch` before any input. Both are
// written as LR-1 registers them: `{kind, candidate_set_sha256, decision_sha256, projection?}`,
// F2's candidate-set hash, both hashes 64 lowercase hex digits, the top-level confirmation of a
// v1 record without `multi_select`. A step without `confirm` compares the candidate sets as
// before (`matched` / `mismatched`).
//
// A frame is six pixels: the page (red `home`, blue `terminal`), the guard (green when it
// holds), the two slots' `ready` marks (green when ready) and their `lit` marks (white when
// lit). The policy gates on `ready` and reads nothing else, so a slot that is not ready is
// gate-rejected whatever its `lit` mark shows; the candidate-set hash covers `lit` as every
// feature value.

use super::*;
use actingcommand_contract::{InstanceFactContext, InstanceFactSnapshot, TaskSelectionRecord};
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition_pack::RecognitionPack;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::PathBuf;

const RED: [u8; 3] = [255, 0, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const GREEN: [u8; 3] = [0, 255, 0];
const WHITE: [u8; 3] = [255, 255, 255];
const BLACK: [u8; 3] = [0, 0, 0];

// One screen: its page, whether the guard holds, which slots are ready and which are lit.
#[derive(Debug, Clone, Copy)]
struct Screen {
    page: [u8; 3],
    guard: bool,
    ready: [bool; 2],
    lit: [bool; 2],
}

const fn home(ready: [bool; 2], lit: [bool; 2]) -> Screen {
    Screen {
        page: RED,
        guard: true,
        ready,
        lit,
    }
}

const TERMINAL: Screen = Screen {
    page: BLUE,
    guard: true,
    ready: [false, false],
    lit: [false, false],
};

// F1 of every case: the first slot is ready, neither slot is lit.
const FIRST_READY: Screen = home([true, false], [false, false]);
// F1 with only the gate-rejected second slot's `lit` reading changed.
const SECOND_LIT: Screen = home([true, false], [false, true]);
// F1 with the other slot ready: the decision chooses the second slot.
const SECOND_READY: Screen = home([false, true], [false, false]);

fn frame(screen: Screen) -> Frame {
    let mark = |on: bool, colour: [u8; 3]| if on { colour } else { BLACK };
    Frame::from_pixels(
        6,
        1,
        [
            screen.page,
            mark(screen.guard, GREEN),
            mark(screen.ready[0], GREEN),
            mark(screen.ready[1], GREEN),
            mark(screen.lit[0], WHITE),
            mark(screen.lit[1], WHITE),
        ]
        .concat(),
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
}

#[derive(Debug)]
enum Event {
    StateRead,
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

    // The one decision record of the run.
    fn decision_record(&self) -> &TaskSelectionRecord {
        let records = self
            .events
            .iter()
            .filter_map(|event| match event {
                Event::Selection(record) => Some(record.as_ref()),
                _ => None,
            })
            .collect::<Vec<_>>();
        match records.as_slice() {
            [record] => *record,
            _ => panic!("one decision record: {:?}", self.events),
        }
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

    fn intents(&self) -> usize {
        self.events
            .iter()
            .filter(|event| matches!(event, Event::Intent))
            .count()
    }
}

impl ContainedTaskRuntime for Device {
    type Error = &'static str;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
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
    let slot = |x: u32, ready: &str, lit: &str| {
        json!({"rect": {"x": x, "y": 0, "width": 1, "height": 1},
               "click": {"x": x, "y": 0, "width": 1, "height": 1},
               "targets": {"ready": ready, "lit": lit}})
    };
    serde_json::from_value(json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 6, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            color("page/home", 0, RED),
            color("page/terminal", 0, BLUE),
            color("guard/ready", 1, GREEN),
            color("slot/first_ready", 2, GREEN),
            color("slot/second_ready", 3, GREEN),
            color("slot/first_lit", 4, WHITE),
            color("slot/second_lit", 5, WHITE)
        ],
        "candidate_layouts": [{
            "id": "choices",
            "page_id": "neutral/home",
            "kind": "fixed_slots",
            "features": [{"name": "ready", "value": "passed"},
                         {"name": "lit", "value": "passed"}],
            "slots": [slot(2, "slot/first_ready", "slot/first_lit"),
                      slot(3, "slot/second_ready", "slot/second_lit")]
        }]
    }))
    .expect("recognition pack")
}

fn evaluator() -> RecognitionEvaluator {
    RecognitionEvaluator::new(PathBuf::new(), pack()).expect("recognition evaluator")
}

// The candidate-set hash of the layout's projection on `screen`.
fn candidate_set_sha256(screen: Screen) -> String {
    let evaluator = evaluator();
    let scene = scene_from_frame(&frame(screen)).expect("fixture scene");
    evaluator
        .scene_context(&scene)
        .project_candidates("choices")
        .expect("fixture projection")
        .candidate_set_sha256()
        .to_owned()
}

// One select step from `home` to `terminal` over the layout above, declaring `confirm` when it
// is given; the policy selects the one ready slot.
fn select_task(confirm: Option<&str>) -> PreparedContainedTask {
    let control: TaskControl = serde_json::from_value(json!({
        "schema_version": "Lab-1y.control.v1",
        "package_id": "neutral.test.select",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 6, "height": 1},
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
    let mut select = json!({"layout_id": "choices", "policy": {"path": "policies/select.json"}});
    if let Some(confirm) = confirm {
        select["confirm"] = json!(confirm);
    }
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
        "select": select
    }))
    .expect("select operation");
    let declared = operation.select.as_ref().expect("declared select");
    operation.prepared_select = Some(Box::new(selection::PreparedSelect::for_test_declared(
        declared, policy,
    )));
    let mut program: TaskProgram = serde_json::from_value(json!({
        "schema_version": "0.6",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 6, "height": 1},
        "target_page": "terminal",
        "operations": []
    }))
    .expect("task program");
    program.operations.push(operation);
    let page_set: PageSet = serde_json::from_value(json!({
        "schema_version": "0.3",
        "pages": [
            {"id": "neutral/home", "required": ["page/home"]},
            {"id": "neutral/terminal", "required": ["page/terminal"]}
        ]
    }))
    .expect("page set");
    let detector = PageDetector::new(page_set).expect("page detector");
    let evaluator = evaluator();
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

fn failed(
    result: Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>,
) -> ContainedTaskError {
    match result {
        Err(ContainedTaskRunError::Task(error)) => error,
        Err(other) => panic!("expected a task failure, got {other:?}"),
        Ok(outcome) => panic!("expected a task failure, the run ended {outcome:?}"),
    }
}

fn is_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

// The members of a serialized confirmation but its optional `projection`, F2's projection when
// it carries recognition evidence.
fn members(confirmation: &Value) -> BTreeSet<&str> {
    confirmation
        .as_object()
        .expect("confirmation object")
        .keys()
        .map(String::as_str)
        .filter(|member| *member != "projection")
        .collect()
}

// The record's confirmation as LR-1 registers it (register §1.153): `kind` and the two hashes,
// `projection` only beside them; both hashes 64 lowercase hex digits; the top-level
// confirmation of a v1 record without `multi_select`, which validates for append and reads back
// to itself. Returns its `candidate_set_sha256` and `decision_sha256`.
fn lr1_confirmation(record: &TaskSelectionRecord, kind: &str) -> (String, String) {
    let confirmation = serde_json::to_value(&record.confirmation).expect("confirmation json");
    assert_eq!(
        members(&confirmation),
        BTreeSet::from(["kind", "candidate_set_sha256", "decision_sha256"]),
        "LR-1 members: {confirmation}"
    );
    assert_eq!(confirmation["kind"], kind, "{confirmation}");
    let candidate_set = confirmation["candidate_set_sha256"]
        .as_str()
        .expect("candidate_set_sha256")
        .to_owned();
    let decision = confirmation["decision_sha256"]
        .as_str()
        .expect("decision_sha256")
        .to_owned();
    assert!(
        is_hash(&candidate_set) && is_hash(&decision),
        "{confirmation}"
    );
    if let Some(projection) = confirmation.get("projection") {
        assert_eq!(
            projection["candidate_set_sha256"], candidate_set,
            "{confirmation}"
        );
    }
    assert_eq!(
        record.projection.schema_version(),
        actingcommand_contract::CANDIDATE_PROJECTION_SCHEMA_VERSION
    );
    assert!(record.multi_select.is_none());
    record
        .validate_for_append()
        .expect("the record validates for append");
    let stored = serde_json::to_value(record).expect("record json");
    let read: TaskSelectionRecord =
        serde_json::from_value(stored.clone()).expect("the record reads back");
    assert_eq!(&read, record, "{stored}");
    (candidate_set, decision)
}

// The decision hash of a decision-mode step whose F2 shows F1 `first` unchanged: the hash of
// the decision taken on `first`, which a matched confirmation repeats.
fn decision_on(first: Screen) -> String {
    let mut device = Device::new(first, first);
    select_task(Some("decision"))
        .run(&mut device)
        .unwrap_or_else(|error| panic!("an unchanged confirmation frame: {error:?}"));
    lr1_confirmation(device.decision_record(), "decision_matched").1
}

// T1: F2 differs from F1 only in the `lit` reading of the gate-rejected second slot. Its
// candidate set hashes differently, its decision is the same: `decision_matched` with F2's
// candidate-set hash and the decision's hash, and the one tap inside the first slot's click.
#[test]
fn bt_b4_t1_a_changed_reading_of_a_gate_rejected_row_keeps_the_decision_and_taps() {
    let mut device = Device::new(FIRST_READY, SECOND_LIT);

    let result = select_task(Some("decision")).run(&mut device);

    let record = device.decision_record();
    let (candidate_set, decision) = lr1_confirmation(record, "decision_matched");
    assert_ne!(
        record.projection.candidate_set_sha256(),
        candidate_set_sha256(SECOND_LIT),
        "the fixture changes F2's candidate set"
    );
    assert_eq!(candidate_set, candidate_set_sha256(SECOND_LIT), "F2's hash");
    assert_eq!(decision, decision_on(FIRST_READY), "the decision's hash");
    assert_eq!(record.selected, ["choices#00"]);
    let outcome = result.unwrap_or_else(|error| panic!("the run fails: {error:?}"));
    assert_eq!(outcome.outcome, TaskOutcome::Success);
    assert_eq!(device.intents(), 1, "{:?}", device.events);
    assert_eq!(device.taps(), [&InputAction::Tap { x: 2, y: 0 }]);
}

// T2: F2 shows the other slot ready, so its decision selects the other candidate although
// every actionable candidate keeps its ID and click rectangle: `decision_mismatched` with F2's
// hashes, `selection_decision_mismatch`, and no input.
#[test]
fn bt_b4_t2_a_confirmation_frame_that_selects_another_candidate_is_a_decision_mismatch() {
    let mut device = Device::new(FIRST_READY, SECOND_READY);

    let result = select_task(Some("decision")).run(&mut device);

    let record = device.decision_record();
    let (candidate_set, decision) = lr1_confirmation(record, "decision_mismatched");
    let error = failed(result);
    assert_eq!(
        error.code().as_str(),
        "selection_decision_mismatch",
        "{error}"
    );
    assert_eq!(
        candidate_set,
        candidate_set_sha256(SECOND_READY),
        "F2's hash"
    );
    assert_ne!(decision, decision_on(FIRST_READY), "F2's decision differs");
    assert_eq!(
        record.selected,
        ["choices#00"],
        "the record keeps F1's decision"
    );
    assert_eq!(device.intents(), 0, "{:?}", device.events);
    assert!(device.taps().is_empty(), "{:?}", device.events);
}

// T3: without `confirm` the step compares the candidate sets as today: an unchanged F2 records
// `matched {candidate_set_sha256}` with the evaluated hash and taps; T1's F2 records
// `mismatched {candidate_set_sha256}` with F2's hash, fails `selection_projection_mismatch` and
// taps nothing.
#[test]
fn bt_b4_t3_without_confirm_the_candidate_sets_compare_as_before() {
    let mut unchanged = Device::new(FIRST_READY, FIRST_READY);
    select_task(None)
        .run(&mut unchanged)
        .unwrap_or_else(|error| panic!("an unchanged confirmation frame: {error:?}"));
    let record = unchanged.decision_record();
    let confirmation = serde_json::to_value(&record.confirmation).expect("confirmation json");
    assert_eq!(confirmation["kind"], "matched", "{confirmation}");
    assert_eq!(
        members(&confirmation),
        BTreeSet::from(["kind", "candidate_set_sha256"])
    );
    assert_eq!(
        confirmation["candidate_set_sha256"],
        record.projection.candidate_set_sha256()
    );
    assert_eq!(unchanged.taps(), [&InputAction::Tap { x: 2, y: 0 }]);

    let mut changed = Device::new(FIRST_READY, SECOND_LIT);
    let error = failed(select_task(None).run(&mut changed));
    let record = changed.decision_record();
    let confirmation = serde_json::to_value(&record.confirmation).expect("confirmation json");
    assert_eq!(confirmation["kind"], "mismatched", "{confirmation}");
    assert_eq!(
        members(&confirmation),
        BTreeSet::from(["kind", "candidate_set_sha256"])
    );
    assert_eq!(
        confirmation["candidate_set_sha256"],
        candidate_set_sha256(SECOND_LIT)
    );
    assert_eq!(
        error.code().as_str(),
        "selection_projection_mismatch",
        "{error}"
    );
    assert!(changed.taps().is_empty(), "{:?}", changed.events);
}
