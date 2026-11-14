// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 K1 (register §1.374; contracts/linear-steps.md, "Select steps"; the design note
// `rd-1114/463-l3l5-design.md`, section A, and the coordinator's rulings on board #463): a
// `linear_steps` task admits and runs the select step a page-graph task has. Its attempt is the
// page graph's: the decision on the step's frame, the confirmation on a fresh frame, one tap inside
// the chosen candidate's click box, after which the step goes on as a click step does. An empty
// decision of a step that declares `on_empty` ends the run Completed on the step's page with its
// key, with no input. A select step declares no retry, is not optional, never starts from any
// screen and its `on_empty` page is mapped to its key; an `expect_identity` is refused, and an
// `input_recheck` beside a select stays refused by the common operation check. Every other result
// fails as on the page graph, before any tap. A package with `on_empty` stays no prerequisite and
// no return-home package.
//
// Every expected value comes from those lines and the package values below, never from the code.
// The packages are synthetic `neutral` packages, admitted by the kernel's own admission
// (`PreparedContainedTask::load_with_vision_provider`, or `validate_linear` for a refusal). A frame
// is 6 x 1: pixel 0 names the page (home, list, detail, done; black is no page), pixel 1 is the
// select step's guard, pixels 2-3 and 4-5 are the list's two cards, each a name code and an open
// mark. A scripted OCR provider reads the name in the pixel its region starts on: nothing, `alpha`
// or `bravo`. The path taps from home into the list (`open`), chooses a card into its detail
// (`pick`) and taps from the detail to done (`close`). The runs play scripted frames on the
// kernel's scripted clock (EXEC-I4): the device shows `screens[n]` after `n` inputs, each segment's
// last frame repeating.

use super::tests::scripted::{ScriptedFrame, ScriptedRuntime};
use super::*;
use actingcommand_contract::{
    InstanceFactContext, InstanceFactSnapshot, MaintenanceUse, TaskSelectionBlocking,
    TaskSelectionConfirmation, TaskSelectionOutcome, TaskSelectionRecord,
    TaskSelectionUnknownCheck, TaskSelectionUpperBound,
};
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition_pack::{
    NnProviderRequest, NnProviderResult, OcrProviderRequest, OcrProviderResult,
    OcrProviderTextBlock, VisionProviderError, VisionProviderErrorCode,
};
use serde_json::{Value, json};
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

const LINEAR_INVALID: &str = ExecutionKernelCode::ContainedTaskLinearInvalid.as_str();
const OPERATION_INVALID: &str = ExecutionKernelCode::ContainedTaskOperationInvalid.as_str();

const WIDTH: u32 = 6;
const STEP_MS: u64 = 5_000;
const ARRIVAL_MS: u64 = 1_000;
const TASK_MS: u64 = 60_000;
// One move of the clock by more than the step timeout ends the window in progress.
const PAST_WINDOW: Duration = Duration::from_secs(6);
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
// The key of the usual ending and the `on_empty` key of `pick`, mapped to the list page.
const DONE_KEY: &str = "task_done";
const NOT_FOUND: &str = "task_not_found";
// The detector pages of the path.
const LIST_PAGE: &str = "neutral/list";
const DETAIL_PAGE: &str = "neutral/detail";
const DONE_PAGE: &str = "neutral/done";

const HOME: [u8; 3] = [255, 0, 255];
const LIST: [u8; 3] = [255, 0, 0];
const DETAIL: [u8; 3] = [0, 0, 255];
const DONE: [u8; 3] = [255, 255, 0];
const GREEN: [u8; 3] = [0, 255, 0];
const BLACK: [u8; 3] = [0, 0, 0];
// The name codes a card's name pixel shows, and the text each one reads as.
const UNREAD: u8 = 0;
const ALPHA: u8 = 1;
const BRAVO: u8 = 2;
const TEXTS: [&str; 3] = ["", "alpha", "bravo"];

type RunResult = Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>;

// Reads the text of the name code in the pixel its region starts on: an unreadable code gives no
// text and no confidence, every other code its text at confidence 0.99.
#[derive(Debug)]
struct ShownText;

impl VisionProvider for ShownText {
    fn require_ocr_model(
        &self,
        _model_ref: &str,
        _model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        Ok(())
    }

    fn require_nn_model(
        &self,
        _model_ref: &str,
        _model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "no NN model in this fixture",
        ))
    }

    fn read_text(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderResult, VisionProviderError> {
        let pixel = usize::try_from(request.region.x).expect("a region on the frame") * 3;
        let code = usize::from(request.frame.rgb8_pixels[pixel]);
        let text = TEXTS.get(code).copied().unwrap_or_default();
        let (blocks, confidence) = if text.is_empty() {
            (Vec::new(), None)
        } else {
            (
                vec![OcrProviderTextBlock {
                    text: text.to_owned(),
                    rect: request.region,
                    confidence: Some(0.99),
                }],
                Some(0.99),
            )
        };
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            text: text.to_owned(),
            blocks,
            confidence,
            detection: None,
        })
    }

    fn classify(
        &self,
        _request: NnProviderRequest<'_>,
    ) -> Result<NnProviderResult, VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "no NN model in this fixture",
        ))
    }
}

// One screen: the page's colour, the guard, then each card's name code and open mark.
fn screen(page: [u8; 3], first: (u8, bool), second: (u8, bool)) -> ScriptedFrame {
    let lit = |open: bool| if open { GREEN } else { BLACK };
    ScriptedFrame::rgb(
        WIDTH,
        1,
        &[
            page,
            GREEN,
            [first.0, 0, 0],
            lit(first.1),
            [second.0, 0, 0],
            lit(second.1),
        ],
    )
}

// A screen of `page` with no card on it.
fn plain(page: [u8; 3]) -> ScriptedFrame {
    screen(page, (UNREAD, false), (UNREAD, false))
}

// The list with its two cards, each a name code and whether it is open.
fn list(first: (u8, bool), second: (u8, bool)) -> ScriptedFrame {
    screen(LIST, first, second)
}

// The list's two cards as fixed slots: each reads the business identity `member` of its name and
// whether it is `open`.
fn cards_layout() -> Value {
    let slot = |x: u32, name: &str, open: &str| {
        json!({"rect": {"x": x, "y": 0, "width": 2, "height": 1},
               "click": {"x": x, "y": 0, "width": 2, "height": 1},
               "targets": {"member": name, "open": open}})
    };
    json!({
        "id": "layout/cards", "page_id": LIST_PAGE, "kind": "fixed_slots",
        "features": [
            {"name": "member", "value": "identity", "identity": {
                "entries": [{"id": "member-a", "aliases": ["alpha"]},
                            {"id": "member-b", "aliases": ["bravo"]}],
                "recognition": {"kind": "ocr_aliases", "max_distance": 1, "minimum_margin": 1,
                                "minimum_confidence_milli": 800}}},
            {"name": "open", "value": "passed"}
        ],
        "slots": [slot(2, "card/first_name", "card/first_open"),
                  slot(4, "card/second_name", "card/second_open")]
    })
}

// The first card alone, its open mark read on two frames 1 ms apart (sample consensus).
fn sampled_layout() -> Value {
    json!({
        "id": "layout/sampled", "page_id": LIST_PAGE, "kind": "fixed_slots",
        "sample_interval_ms": 1,
        "features": [{"name": "open", "value": "passed",
                      "consensus": {"samples": [{"frame": 0}, {"frame": 1}],
                                    "aggregate": {"kind": "k_of_n", "k": 2}}}],
        "slots": [{"rect": {"x": 2, "y": 0, "width": 2, "height": 1},
                   "click": {"x": 2, "y": 0, "width": 2, "height": 1},
                   "targets": {"open": "card/first_open"}}]
    })
}

// A member set over `layout_id`: every open card is a member, of equal score, chosen in `mode`
// with the tie-break keys `tie_break`; with `identity`, it also reads the card's identity.
fn policy(layout_id: &str, identity: bool, mode: &str, tie_break: Value) -> Value {
    let mut fields = vec![json!({"name": "open", "value_type": {"type": "boolean"}})];
    if identity {
        fields.insert(
            0,
            json!({"name": "member",
                   "value_type": {"type": "enum_string", "allowed": ["member-a", "member-b"]}}),
        );
    }
    json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "cards",
        "applies_to": {
            "candidate_layout_id": layout_id,
            "outcome_keys": {"selected": "card_selected", "empty": "card_empty",
                             "insufficient": "card_insufficient",
                             "ambiguous": "card_ambiguous", "unknown": "card_unknown"}
        },
        "fields": fields,
        "facts": [],
        "gates": [{"gate_id": "open",
                   "predicate": {"kind": "boolean_equals",
                                 "value": {"source": "field", "field": "open"},
                                 "expected": true},
                   "on_unknown": {"kind": "drop_candidate"}}],
        "scoring": [],
        "selection": {"mode": mode, "required_count": 1},
        "tie_break": tie_break
    })
}

// The base policy: `none_allowed`, the lowest candidate ID last.
fn member_policy() -> Value {
    policy(
        "layout/cards",
        true,
        "none_allowed",
        json!([{"kind": "candidate_id", "direction": "lowest_first"}]),
    )
}

// The linear control of a path of `max_steps` operations.
fn control(max_steps: u32) -> Value {
    json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.linear.select",
        "execution_mode": "linear_steps",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": WIDTH, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 50,
        "step_timeout_ms": STEP_MS,
        "timeout_ms": TASK_MS,
        "max_steps": max_steps
    })
}

// A trusted tap `id` from `from` to `to`, expected within the arrival budget.
fn tap(id: &str, from: &str, to: &str) -> Value {
    json!({"id": id, "from": from, "to": to,
           "click": {"kind": "point", "x": 0, "y": 0},
           "unguarded_trusted_coordinate": true,
           "expect_after": {"page_id": to, "timeout_ms": ARRIVAL_MS}})
}

// The select step `pick`: from the list, guarded, it taps the chosen card of `layout_id` into the
// detail page; with `on_empty`, nobody open ends the run with `NOT_FOUND`.
fn pick(layout_id: &str, on_empty: bool) -> Value {
    let mut select = json!({"layout_id": layout_id, "policy": {"path": "policies/select.json"}});
    if on_empty {
        select["on_empty"] = json!({"outcome_key": NOT_FOUND});
    }
    json!({
        "id": "pick", "from": "list", "to": "detail",
        "expect_after": {"page_id": "detail", "timeout_ms": ARRIVAL_MS},
        "guard": {"page_id": "list", "target_id": "guard/ready",
                  "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                  "color_probe": "guard/ready"},
        "select": select
    })
}

fn mapping(key: &str, page: &str) -> Value {
    json!({"outcome_key": key, "effect": "no_designated_effect", "terminal_pages": [page]})
}

// The base task: `open`, `pick` and `close` from home to done. With `on_empty`, `pick` declares it
// and the scheduling outcome maps the list page to its key beside the usual ending.
fn base_task(on_empty: bool) -> Value {
    let mut mappings = vec![mapping(DONE_KEY, "done")];
    if on_empty {
        mappings.push(mapping(NOT_FOUND, "list"));
    }
    json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": WIDTH, "height": 1},
        "entry_page": "home",
        "target_page": "done",
        "max_steps": 3,
        "candidate_layouts": [cards_layout()],
        "scheduling_outcome": {"mappings": mappings},
        "operations": [tap("open", "home", "list"), pick("layout/cards", on_empty),
                       tap("close", "detail", "done")]
    })
}

// One step `pick` over the sampled card, from the list (the entry) to the detail page.
fn sampled_task() -> Value {
    json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": WIDTH, "height": 1},
        "entry_page": "list",
        "target_page": "detail",
        "max_steps": 1,
        "candidate_layouts": [sampled_layout()],
        "operations": [pick("layout/sampled", false)]
    })
}

// The page set: home, list, detail and done, each told by pixel 0.
fn pages() -> Value {
    let pages = ["home", "list", "detail", "done"].map(|page| {
        json!({"id": format!("neutral/{page}"), "required": [format!("page/{page}")],
               "optional": [], "forbidden": []})
    });
    json!({"schema_version": "0.3", "pages": pages})
}

// The recognition pack: the pages' colours on pixel 0, the guard on pixel 1, each card's name
// (OCR) and open mark, and the task's candidate layouts.
fn pack(layouts: &Value) -> Value {
    let colour = |id: &str, x: u32, expected: [u8; 3]| {
        json!({"type": "color", "id": id,
               "region": {"x": x, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    let text = |id: &str, x: u32| {
        json!({"type": "ocr", "id": id, "region": {"x": x, "y": 0, "width": 1, "height": 1},
               "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact",
               "expected": ["0"], "case_sensitive": false, "minimum_confidence": 0.0,
               "model_ref": "PP-OCRv6_medium", "model_sha256": MODEL_SHA256})
    };
    json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": WIDTH, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            colour("page/home", 0, HOME),
            colour("page/list", 0, LIST),
            colour("page/detail", 0, DETAIL),
            colour("page/done", 0, DONE),
            colour("guard/ready", 1, GREEN),
            text("card/first_name", 2),
            colour("card/first_open", 3, GREEN),
            text("card/second_name", 4),
            colour("card/second_open", 5, GREEN)
        ],
        "candidate_layouts": layouts
    })
}

fn zip_entries(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, bytes) in entries {
        zip.start_file(*path, options).expect("zip entry");
        zip.write_all(bytes).expect("zip bytes");
    }
    zip.finish().expect("finish zip").into_inner()
}

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("JSON bytes")
}

// The package of `task` under `control`, with `policy` as its select step's document.
fn package(control: &Value, task: &Value, policy: &Value) -> Vec<u8> {
    zip_entries(&[
        ("control.json", bytes(control)),
        (
            "resources/manifest.json",
            bytes(&json!({"schema_version": "0.3", "entry_task_id": "task"})),
        ),
        ("resources/operations/task/task.json", bytes(task)),
        (
            "resources/operations/task/policies/select.json",
            bytes(policy),
        ),
        (
            "resources/recognition/neutral.test.pack.json",
            bytes(&pack(&task["candidate_layouts"])),
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            bytes(&pages()),
        ),
    ])
}

// The package as contained task admission prepares it, with the scripted OCR provider.
fn admitted(case: &str, control: &Value, task: &Value, policy: &Value) -> PreparedContainedTask {
    PreparedContainedTask::load_with_vision_provider(
        "neutral.instance",
        &package(control, task, policy),
        Arc::new(ShownText),
    )
    .unwrap_or_else(|error| panic!("{case}: refused: {error}"))
}

// The base package, `pick` declaring `on_empty`, with the base policy.
fn base_package() -> PreparedContainedTask {
    admitted(
        "the base package",
        &control(3),
        &base_task(true),
        &member_policy(),
    )
}

// The step plan of `task` under `control` as linear admission reads it: its refusal, if any.
fn linear_admission(control: Value, task: Value) -> Result<(), ContainedTaskError> {
    let control = parse_task_control_declaration(control).expect("a valid control");
    let program: TaskProgram = serde_json::from_value(task).expect("a task program");
    let page_set: PageSet = serde_json::from_value(pages()).expect("page set");
    let detector = PageDetector::new(page_set).expect("page detector");
    program.validate_linear(&control, &detector).map(drop)
}

// The device of the runs: the kernel's scripted runtime, with the instance facts and instant a
// select step asks for. With `jump`, the first decision record moves its clock that far.
struct Device {
    script: ScriptedRuntime,
    jump: Option<Duration>,
}

impl Device {
    fn new(screens: Vec<Vec<ScriptedFrame>>) -> Self {
        Self {
            script: ScriptedRuntime::new(screens),
            jump: None,
        }
    }

    // The same device, giving each step `retries` observe-only retries, whose first decision
    // record moves its clock past the step's window.
    fn jumping_with_retries(screens: Vec<Vec<ScriptedFrame>>, retries: u32) -> Self {
        Self {
            script: ScriptedRuntime::new(screens).with_sample_timeout_retries(retries),
            jump: Some(PAST_WINDOW),
        }
    }

    fn inputs(&self) -> &[InputAction] {
        self.script.inputs()
    }

    fn traces(&self) -> &[ContainedTaskTrace] {
        self.script.traces()
    }

    // The step, selection, effect and final traces, in order.
    fn kinds(&self) -> Vec<&'static str> {
        self.traces()
            .iter()
            .filter_map(|trace| match trace {
                ContainedTaskTrace::StepStarted { .. } => Some("step_started"),
                ContainedTaskTrace::SelectionEvaluated { .. } => Some("selection_evaluated"),
                ContainedTaskTrace::EffectIntent { .. } => Some("effect_intent"),
                ContainedTaskTrace::EffectCompleted { .. } => Some("effect_completed"),
                ContainedTaskTrace::StepFinished { .. } => Some("step_finished"),
                ContainedTaskTrace::Finalizing { .. } => Some("finalizing"),
                _ => None,
            })
            .collect()
    }

    fn count(&self, kind: &str) -> usize {
        self.kinds().iter().filter(|seen| **seen == kind).count()
    }

    // Each decision record, with its step index.
    fn records(&self) -> Vec<(u32, &TaskSelectionRecord)> {
        self.traces()
            .iter()
            .filter_map(|trace| match trace {
                ContainedTaskTrace::SelectionEvaluated {
                    step_index,
                    selection,
                    ..
                } => Some((*step_index, selection.as_ref())),
                _ => None,
            })
            .collect()
    }

    // Each `StepFinished`: its step index, its operation and its page.
    fn finished(&self) -> Vec<(u32, &str, &str)> {
        self.traces()
            .iter()
            .filter_map(|trace| match trace {
                ContainedTaskTrace::StepFinished {
                    step_index,
                    operation_label,
                    page_label,
                    ..
                } => Some((*step_index, operation_label.as_str(), page_label.as_str())),
                _ => None,
            })
            .collect()
    }

    // Each effect intent: its step index, its operation and its action.
    fn intents(&self) -> Vec<(u32, &str, &InputAction)> {
        self.traces()
            .iter()
            .filter_map(|trace| match trace {
                ContainedTaskTrace::EffectIntent {
                    step_index,
                    operation_label,
                    action,
                    ..
                } => Some((*step_index, operation_label.as_str(), action)),
                _ => None,
            })
            .collect()
    }
}

impl ContainedTaskRuntime for Device {
    type Error = &'static str;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        self.script.capture()
    }

    fn input(
        &mut self,
        action: InputAction,
        frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        self.script.input(action, frame)
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        let decided = matches!(trace, ContainedTaskTrace::SelectionEvaluated { .. });
        self.script.record(trace)?;
        if decided && let Some(jump) = self.jump.take() {
            self.script.pause(jump);
        }
        Ok(())
    }

    fn sampling_supported(&self) -> bool {
        self.script.sampling_supported()
    }

    fn step_pacing(&mut self) -> Result<StepPacing, Self::Error> {
        self.script.step_pacing()
    }

    fn step_action_gap(&mut self) -> Result<Duration, Self::Error> {
        self.script.step_action_gap()
    }

    fn monotonic_now(&mut self) -> Instant {
        self.script.monotonic_now()
    }

    fn pause(&mut self, duration: Duration) {
        self.script.pause(duration);
    }

    fn selection_state(
        &mut self,
        request: SelectionStateRequest,
    ) -> Result<SelectionState, Self::Error> {
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
}

// The base path's screens: home, then `chooser` (the list `pick` decides on, its frames in order),
// then `after_pick` (what the device shows after `pick`'s tap), then done.
fn path(chooser: Vec<ScriptedFrame>, after_pick: Vec<ScriptedFrame>) -> Vec<Vec<ScriptedFrame>> {
    vec![vec![plain(HOME)], chooser, after_pick, vec![plain(DONE)]]
}

fn failed(case: &str, result: RunResult) -> ContainedTaskError {
    match result {
        Err(ContainedTaskRunError::Task(error)) => error,
        Err(other) => panic!("{case}: expected a task failure, got {other:?}"),
        Ok(outcome) => panic!("{case}: expected a task failure, the run ended {outcome:?}"),
    }
}

fn value(error: &ContainedTaskError, key: &str) -> Value {
    error.outcome().value(key).cloned().unwrap_or(Value::Null)
}

// The run's normal end on `page` after `executed_steps`, with the scheduling outcome `key`.
fn completed(page: &str, executed_steps: u32, key: Option<&str>) -> ContainedTaskOutcome {
    ContainedTaskOutcome {
        outcome: TaskOutcome::Success,
        final_page: Some(page.to_owned()),
        executed_steps,
        selected_scheduling_outcome: key.map(str::to_owned),
        designated_effect_caps: Vec::new(),
    }
}

// The one input the failed runs below make: `open`'s tap; `pick` taps nothing, so no effect intent
// names it.
fn assert_no_pick_input(case: &str, device: &Device) {
    assert_eq!(device.inputs().len(), 1, "{case}: {:?}", device.inputs());
    assert!(
        device
            .intents()
            .iter()
            .all(|(_, operation, _)| *operation == "open"),
        "{case}: {:?}",
        device.intents()
    );
}

// A.1: a linear package whose second step is a single-frame select over the fixed slots of the
// list, with `on_empty` and its page mapped to its key, is admitted by the whole admission; the
// step is prepared with its layout, its identity feature and its key.
#[test]
fn k1_a_fixed_slots_select_with_on_empty_is_admitted_under_linear_steps() {
    let task = base_package();
    assert!(task.has_select_steps());
    let operation = &task.program.operations[1];
    let select = operation
        .prepared_select
        .as_deref()
        .expect("the select step is prepared");
    assert_eq!(select.layout_id(), "layout/cards");
    assert_eq!(select.identity_feature(), Some("member"));
    assert_eq!(operation.on_empty_key(), Some(NOT_FOUND));
    assert_eq!(operation.scan_max_ms(), None);
}

// A.1: each rule a linear task's select step is held to, on the base task changed in the one
// declaration under test; the base task itself is admitted. A refusal is
// `contained_task_linear_invalid` with its reason and the operation at fault.
#[test]
fn k1_linear_admission_refuses_a_select_step_by_each_rule_with_its_reason() {
    linear_admission(control(3), base_task(true)).expect("the base task is admitted");
    let with = |change: &dyn Fn(&mut Value)| {
        let mut task = base_task(true);
        change(&mut task);
        task
    };
    let identity = json!({"target_id": "card/first_name", "choice_of": "pick"});
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "an optional select step",
            with(&|task: &mut Value| {
                task["operations"][1]["optional"] = json!({"settle_ms": 100});
            }),
            "reason=select_optional operation=pick",
        ),
        (
            "a select step that declares retryable false",
            with(&|task: &mut Value| task["operations"][1]["retryable"] = json!(false)),
            "reason=select_retry operation=pick",
        ),
        (
            "a select step that declares a retry",
            with(&|task: &mut Value| {
                task["operations"][1]["retryable"] = json!(true);
                task["operations"][1]["max_attempts"] = json!(2);
                task["operations"][1]["retry_interval_ms"] = json!(100);
            }),
            "reason=select_retry operation=pick",
        ),
        (
            "the select step's own expect_identity",
            with(&|task: &mut Value| {
                task["operations"][1]["expect_after"]["expect_identity"] = identity.clone();
            }),
            "reason=expect_identity operation=pick",
        ),
        (
            "a later step's expect_identity of the select step",
            with(&|task: &mut Value| {
                task["operations"][2]["expect_after"]["expect_identity"] = identity.clone();
            }),
            "reason=expect_identity operation=close",
        ),
        (
            "the on_empty page unmapped",
            with(&|task: &mut Value| {
                task["scheduling_outcome"]["mappings"] = json!([mapping(DONE_KEY, "done")]);
            }),
            "reason=select_on_empty_terminal operation=pick",
        ),
        (
            "the on_empty page mapped to another key",
            with(&|task: &mut Value| {
                task["scheduling_outcome"]["mappings"][1]["outcome_key"] = json!("task_other");
            }),
            "reason=select_on_empty_terminal operation=pick",
        ),
        (
            "a terminal page that is neither the target nor an on_empty page",
            with(&|task: &mut Value| {
                task["scheduling_outcome"]["mappings"]
                    .as_array_mut()
                    .expect("mappings")
                    .push(mapping("task_detail", "detail"));
            }),
            "reason=terminal_page",
        ),
    ];
    for (case, task, detail) in cases {
        let error = linear_admission(control(3), task)
            .err()
            .unwrap_or_else(|| panic!("{case}: admitted"));
        assert_eq!(error.code().as_str(), LINEAR_INVALID, "{case}: {error}");
        assert_eq!(error.detail(), Some(detail), "{case}: {error}");
    }

    // The list page mapped while no select step declares `on_empty`: the target page is the only
    // terminal page, as before.
    let mut unmapped = base_task(false);
    unmapped["scheduling_outcome"]["mappings"]
        .as_array_mut()
        .expect("mappings")
        .push(mapping(NOT_FOUND, "list"));
    let error = linear_admission(control(3), unmapped)
        .err()
        .expect("a list page without on_empty is refused");
    assert_eq!(error.code().as_str(), LINEAR_INVALID, "{error}");
    assert_eq!(error.detail(), Some("reason=terminal_page"), "{error}");

    // A select step chooses on a page: an application entry's `any` is refused for it.
    let mut from_any = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": WIDTH, "height": 1},
        "entry_page": "any",
        "target_page": "detail",
        "max_steps": 1,
        "operations": [pick("layout/cards", false)]
    });
    from_any["operations"][0]["from"] = json!("any");
    let error = linear_admission(control(1), from_any)
        .err()
        .expect("a select step from any is refused");
    assert_eq!(error.code().as_str(), LINEAR_INVALID, "{error}");
    assert_eq!(
        error.detail(),
        Some("reason=any_requires_application operation=pick"),
        "{error}"
    );

    // A select step's confirmation is its recheck: an `input_recheck` beside it stays refused by
    // the common operation check, as on the page graph.
    let recheck = with(&|task: &mut Value| {
        task["operations"][1]["input_recheck"] = json!({"kind": "fresh_frame"});
    });
    let error = linear_admission(control(3), recheck)
        .err()
        .expect("an input recheck beside a select is refused");
    assert_eq!(error.code().as_str(), OPERATION_INVALID, "{error}");
    assert_eq!(
        error.detail(),
        Some("operation=pick field=input_recheck"),
        "{error}"
    );
}

// A select step with `on_empty` may end its run on the step's page, short of a dependent package's
// entry page: the package is no prerequisite package and no return-home package, configured or
// not, as a page-graph one is; the same package without `on_empty` is either.
#[test]
fn k1_a_linear_package_with_on_empty_is_no_prerequisite_and_no_return_home_package() {
    let with = base_package();
    assert_eq!(with.prerequisite_incompatibility(), Some("select_on_empty"));
    let descriptor = with.package_descriptor();
    for configured_return_home in [false, true] {
        assert_eq!(
            descriptor.prerequisite_incompatibility_with(
                "neutral",
                "test",
                (WIDTH, 1),
                configured_return_home
            ),
            Some("select_on_empty"),
            "configured return home {configured_return_home}"
        );
    }
    assert_eq!(
        descriptor.use_incompatibility(MaintenanceUse::ReturnHome),
        Some("select_on_empty")
    );
    let without = admitted(
        "without on_empty",
        &control(3),
        &base_task(false),
        &member_policy(),
    );
    assert_eq!(without.prerequisite_incompatibility(), None);
}

// A.2: the first card reads `alpha` and is open, the second `bravo` and closed. `pick` decides on
// the frame the list arrived on, confirms on one fresh frame and taps once inside the first card's
// click box (x 2 or 3), with nothing captured between the confirmation frame and the tap; its one
// decision record is `selected`, confirmed `matched`. The step goes on as a click: the detail page
// passes, `close` runs, and the run finishes on done with every step counted.
#[test]
fn k1_a_selected_card_is_tapped_once_after_its_confirmation_and_the_next_step_runs() {
    let task = base_package();
    let mut device = Device::new(path(
        vec![list((ALPHA, true), (BRAVO, false))],
        vec![plain(DETAIL)],
    ));

    let outcome = task
        .run(&mut device)
        .unwrap_or_else(|error| panic!("expected the run to end normally, got {error:?}"));

    assert_eq!(outcome, completed(DONE_PAGE, 3, None));
    let records = device.records();
    let [(1, record)] = records.as_slice() else {
        panic!("one decision record of step 1: {:?}", device.kinds());
    };
    assert_eq!(
        record.selection_result,
        TaskSelectionOutcome::Selected { count: 1 }
    );
    assert!(
        matches!(
            record.confirmation,
            TaskSelectionConfirmation::Matched { .. }
        ),
        "{:?}",
        record.confirmation
    );
    assert_eq!(device.inputs().len(), 3, "{:?}", device.inputs());
    assert!(
        matches!(device.inputs()[1], InputAction::Tap { x: 2..=3, y: 0 }),
        "{:?}",
        device.inputs()[1]
    );
    let intents = device.intents();
    assert_eq!(
        intents
            .iter()
            .map(|(step, operation, _)| (*step, *operation))
            .collect::<Vec<_>>(),
        [(0, "open"), (1, "pick"), (2, "close")]
    );
    assert_eq!(intents[1].2, &device.inputs()[1]);
    // The frame the list arrived on and the confirmation frame, then the tap.
    assert_eq!(device.script.captures_after(1), 2);
    assert_eq!(
        device.finished(),
        [
            (0, "open", LIST_PAGE),
            (1, "pick", DETAIL_PAGE),
            (2, "close", DONE_PAGE)
        ]
    );
}

// A.3: both cards are closed, both read, so the policy (`none_allowed`) admits nobody and decides
// `empty` with nothing unknown. `pick` declares `on_empty`: no confirmation, no input, no effect
// intent of its own; its record holds the decision with `unknown_check` and no confirmation, its
// `StepFinished` names the list, and the run is Completed on the list with `NOT_FOUND`, both steps
// dispatched counted.
#[test]
fn k1_an_empty_decision_with_on_empty_completes_the_run_on_the_list_with_its_key() {
    let task = base_package();
    let mut device = Device::new(path(
        vec![list((ALPHA, false), (BRAVO, false))],
        vec![plain(DETAIL)],
    ));

    let outcome = task
        .run(&mut device)
        .unwrap_or_else(|error| panic!("expected the run to end normally, got {error:?}"));

    assert_eq!(outcome, completed(LIST_PAGE, 2, Some(NOT_FOUND)));
    assert_eq!(
        device.kinds(),
        [
            "step_started",
            "effect_intent",
            "effect_completed",
            "step_finished",
            "step_started",
            "selection_evaluated",
            "step_finished",
            "finalizing"
        ]
    );
    let records = device.records();
    let [(1, record)] = records.as_slice() else {
        panic!("one decision record of step 1: {:?}", device.kinds());
    };
    assert_eq!(record.selection_result, TaskSelectionOutcome::Empty);
    assert_eq!(record.outcome_key, "card_empty");
    assert_eq!(
        record.unknown_check,
        Some(TaskSelectionUnknownCheck {
            unknown_instances: 0,
            blocking: TaskSelectionBlocking::None,
            upper_bound: TaskSelectionUpperBound::NoneUnknown,
        })
    );
    assert_eq!(record.confirmation, TaskSelectionConfirmation::NotAttempted);
    assert_eq!(
        device.finished(),
        [(0, "open", LIST_PAGE), (1, "pick", LIST_PAGE)]
    );
    assert_no_pick_input("an empty end", &device);
    // Only the frame the list arrived on: no confirmation frame.
    assert_eq!(device.script.captures_after(1), 1);
}

// A.2 (selection-graph.md, "Record per path"): the first card reads `alpha` and is closed; the
// second is open but its name reads no text, so its identity is unknown. The `empty` decision does
// not stand: `pick` fails `selection_result_blocked` with the decision's kind and key, the blocking
// and the count of unknown instances; its record keeps `empty` with its `unknown_check`, and
// nothing is tapped.
#[test]
fn k1_an_unreadable_card_blocks_the_empty_end_with_no_input() {
    let task = base_package();
    let mut device = Device::new(path(
        vec![list((ALPHA, false), (UNREAD, true))],
        vec![plain(DETAIL)],
    ));

    let error = failed("an unreadable card", task.run(&mut device));

    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::SelectionResultBlocked.as_str(),
        "{error}"
    );
    assert_eq!(value(&error, "selection_kind"), json!("empty"));
    assert_eq!(
        value(&error, "selection_blocking"),
        json!("unknown_instances")
    );
    assert_eq!(value(&error, "count"), json!(1));
    let records = device.records();
    let [(1, record)] = records.as_slice() else {
        panic!("one decision record of step 1: {:?}", device.kinds());
    };
    assert_eq!(record.selection_result, TaskSelectionOutcome::Empty);
    let check = record.unknown_check.expect("unknown_check");
    assert_eq!(check.unknown_instances, 1);
    assert_eq!(check.blocking, TaskSelectionBlocking::UnknownInstances);
    assert_no_pick_input("an unreadable card", &device);
}

// A.2: under an `exactly_one` policy without a tie-break key and without `on_empty`, both cards
// closed leave no candidate (`insufficient`) and both open leave two of equal rank (`ambiguous`):
// each fails `selection_not_selected` with the decision's kind and key, its one record, and no
// input.
#[test]
fn k1_an_insufficient_or_ambiguous_decision_fails_selection_not_selected_with_no_input() {
    let task = admitted(
        "exactly one, no tie-break",
        &control(3),
        &base_task(false),
        &policy("layout/cards", true, "exactly_one", json!([])),
    );
    for (case, chooser, detail) in [
        (
            "both cards closed",
            list((ALPHA, false), (BRAVO, false)),
            "insufficient:card_insufficient",
        ),
        (
            "both cards open",
            list((ALPHA, true), (BRAVO, true)),
            "ambiguous:card_ambiguous",
        ),
    ] {
        let mut device = Device::new(path(vec![chooser], vec![plain(DETAIL)]));
        let error = failed(case, task.run(&mut device));
        assert_eq!(
            error.code().as_str(),
            ExecutionKernelCode::SelectionNotSelected.as_str(),
            "{case}: {error}"
        );
        assert_eq!(error.detail(), Some(detail), "{case}");
        let records = device.records();
        let [(1, record)] = records.as_slice() else {
            panic!(
                "{case}: one decision record of step 1: {:?}",
                device.kinds()
            );
        };
        assert_eq!(
            record.confirmation,
            TaskSelectionConfirmation::NotAttempted,
            "{case}"
        );
        assert_no_pick_input(case, &device);
    }
}

// A.2: the first card is chosen on the frame the list arrived on, but on the confirmation frame
// the second card is open too, so the candidate sets differ: `pick` fails
// `selection_projection_mismatch` after its record (`mismatched`), and nothing is tapped.
#[test]
fn k1_a_changed_confirmation_frame_fails_selection_projection_mismatch_with_no_input() {
    let task = base_package();
    let mut device = Device::new(path(
        vec![
            list((ALPHA, true), (BRAVO, false)),
            list((ALPHA, true), (BRAVO, true)),
        ],
        vec![plain(DETAIL)],
    ));

    let error = failed("a changed confirmation frame", task.run(&mut device));

    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::SelectionProjectionMismatch.as_str(),
        "{error}"
    );
    let records = device.records();
    let [(1, record)] = records.as_slice() else {
        panic!("one decision record of step 1: {:?}", device.kinds());
    };
    assert!(
        matches!(
            record.confirmation,
            TaskSelectionConfirmation::Mismatched { .. }
        ),
        "{:?}",
        record.confirmation
    );
    assert_no_pick_input("a changed confirmation frame", &device);
}

// A.2 (Workflow #386 SR-1): the card's open mark is sampled on two frames, so the selection's
// sampling has a deadline; the first decision record moves the clock past the step's window. With
// one observe-only retry the selection is observed again on a fresh frame and made again (a second
// record, one `StepStarted`), and the one tap follows; with none the run ends with
// `candidate_sample_deadline_exceeded`, its attempts, window and stage, one record and no input.
#[test]
fn k1_a_selection_sampling_timeout_is_observed_again_without_an_input() {
    let task = admitted(
        "a sampled select step",
        &control(1),
        &sampled_task(),
        &policy(
            "layout/sampled",
            false,
            "exactly_one",
            json!([{"kind": "candidate_id", "direction": "lowest_first"}]),
        ),
    );
    let screens = || {
        vec![
            vec![list((ALPHA, true), (BRAVO, false))],
            vec![plain(DETAIL)],
        ]
    };

    let mut device = Device::jumping_with_retries(screens(), 1);
    let outcome = task
        .run(&mut device)
        .unwrap_or_else(|error| panic!("expected the run to end normally, got {error:?}"));
    assert_eq!(outcome, completed(DETAIL_PAGE, 1, None));
    assert_eq!(device.count("selection_evaluated"), 2);
    assert_eq!(device.count("step_started"), 1);
    assert_eq!(device.inputs().len(), 1);

    let mut device = Device::jumping_with_retries(screens(), 0);
    let error = failed("no retry", task.run(&mut device));
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::CandidateSampleDeadlineExceeded.as_str(),
        "{error}"
    );
    assert_eq!(
        Value::Object(error.outcome().values().clone()),
        json!({"attempts": 0, "timeout_ms": STEP_MS, "stage": "observation_before_input"})
    );
    assert_eq!(device.count("selection_evaluated"), 1);
    assert!(device.inputs().is_empty(), "{:?}", device.inputs());
}

// A.2 with Workflow #463 U1 (its fix round 2): after `pick`'s tap the device shows a black
// screen, neither the detail page nor the list, on every frame. The arrival is the click step's
// and runs out; `pick` declares no retry, so that attempt is its last, and the closing decision
// that follows observes the step timeout, a frame every 50 ms: the screen stays still and shows no
// page of the package (not the detail page or the list it awaits, nor home or done on its last
// frame) up to its bound. The run ends with `contained_task_linear_unrecorded_screen`, the detail
// and the timing `page_confirmation_failed` gives the arrival's end (its 1 000 ms of 1 000), the
// values naming the step, the pages the decision awaited (the detail page first, the list last)
// and one attempt; `pick`'s one `StepFinished` names `<unrecognized>`, after its one tap, and the
// black screen was captured for the arrival's 21 frames and the closing decision's 101.
#[test]
fn k1_after_the_select_tap_a_settled_unknown_screen_ends_with_the_unrecorded_screen_code() {
    // A wait of 1 000 ms and one of 5 000 ms at 50 ms, from a first frame taken at once.
    const ARRIVAL_FRAMES: usize = 21;
    const CLOSING_FRAMES: usize = 101;
    let task = base_package();
    let mut device = Device::new(path(
        vec![list((ALPHA, true), (BRAVO, false))],
        vec![plain(BLACK)],
    ));

    let error = failed("a settled black screen", task.run(&mut device));

    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::ContainedTaskLinearUnrecordedScreen.as_str(),
        "{error}"
    );
    assert_eq!(
        error.detail(),
        Some(
            "operation=pick attempts=1 after_page=<unrecognized> hit_error_page=false transition=none"
        ),
        "{error}"
    );
    assert_eq!(value(&error, "pack_operation_id"), json!("pick"));
    assert_eq!(value(&error, "page_ids"), json!([DETAIL_PAGE, LIST_PAGE]));
    assert_eq!(value(&error, "attempts"), json!(1));
    assert_eq!(
        error.timing().map(|timing| (
            timing.scope,
            timing.stage,
            timing.elapsed_ms,
            timing.limit_ms
        )),
        Some((
            TaskTimingScope::Postcondition,
            TaskTimingStage::Postcondition,
            ARRIVAL_MS,
            ARRIVAL_MS
        ))
    );
    assert_eq!(device.inputs().len(), 2, "{:?}", device.inputs());
    assert_eq!(
        device.finished(),
        [(0, "open", LIST_PAGE), (1, "pick", "<unrecognized>")]
    );
    assert_eq!(
        device.script.captures_after(2),
        ARRIVAL_FRAMES + CLOSING_FRAMES
    );
}

// The card list of the SEL-L4b admission gate (`gate_308_l4b_admission`): 400 x 600 cards found by
// an anchor template, each naming its member by OCR and showing an open mark.
const SCAN_WIDTH: u32 = 400;
const SCAN_HEIGHT: u32 = 600;
const SCAN_MAX_MS: u64 = 240_000;

fn scan_identities() -> [&'static str; 4] {
    ["member-00", "member-01", "member-02", "member-03"]
}

fn scan_layout() -> Value {
    let entries = scan_identities().map(|id| json!({"id": id, "aliases": [id.replace('-', " ")]}));
    json!({
        "id": "layout/cards", "page_id": LIST_PAGE, "kind": "repeated_anchor",
        "anchor": "ui/card", "max_instances": 64, "order": "top_to_bottom",
        "instance_rect": {"x": 0, "y": 0, "width": 100, "height": 80},
        "click": {"x": 10, "y": 10, "width": 80, "height": 40},
        "readable_band": {"x": 0, "y": 60, "width": 400, "height": 480},
        "unknown_identity": "reject",
        "features": [
            {"name": "member", "value": "identity", "target": "card/name",
             "offset": {"x": 0, "y": 50},
             "identity": {
                 "entries": entries,
                 "recognition": {"kind": "ocr_aliases", "max_distance": 1, "minimum_margin": 1,
                                 "minimum_confidence_milli": 800}}},
            {"name": "open", "value": "passed", "target": "card/open",
             "offset": {"x": 80, "y": 0}}
        ]
    })
}

// The SEL-L4b gate's scan: a vertical list over the readable band, within `SCAN_MAX_MS`.
fn scan_declaration() -> Value {
    json!({
        "identity_feature": "member",
        "axis": "vertical",
        "advance": {"kind": "drag",
                    "from": {"x": 150, "y": 450, "width": 100, "height": 20},
                    "to": {"x": 150, "y": 150, "width": 100, "height": 20},
                    "duration_ms": 500, "min_content_step_px": 180, "max_content_step_px": 400},
        "reposition": {"kind": "drag",
                       "from": {"x": 150, "y": 500, "width": 100, "height": 20},
                       "to": {"x": 150, "y": 80, "width": 100, "height": 20},
                       "duration_ms": 400, "max_content_step_px": 480},
        "settle": {"interval_ms": 200, "timeout_ms": 1000},
        "track": {"area": {"x": 0, "y": 40, "width": 400, "height": 520},
                  "edge_margin_px": 12,
                  "signature": {"rect": {"x": 0, "y": 0, "width": 20, "height": 20},
                                "columns": 1, "rows": 1},
                  "signature_max_distance_milli": 500,
                  "position_tolerance_px": 4,
                  "min_overlap_instances": 2,
                  "spacing": {"max_axis_gap_px": 120, "columns": 3, "max_trailing_gap_px": 200,
                              "max_cross_lead_px": 40, "max_cross_gap_px": 140}},
        "start": {"check": "check/list_start", "rewind": true},
        "end": {"check": "check/list_end"},
        "bounds": {"max_viewports": 32, "max_reposition_viewports": 8, "max_ms": SCAN_MAX_MS}
    })
}

// A textured template image, `seed` varying it.
fn template_png(width: u32, height: u32, seed: u32) -> Vec<u8> {
    let pixels = (0..width * height)
        .flat_map(|index| {
            let [red, green, blue, _] = index
                .wrapping_mul(2_654_435_761)
                .wrapping_add(seed.wrapping_mul(40_503))
                .to_le_bytes();
            [red, green, blue]
        })
        .collect::<Vec<u8>>();
    Frame::from_pixels(
        width,
        height,
        pixels,
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("template frame")
    .png_for_artifact()
    .expect("template PNG")
}

// The linear package of one select step over the card list's scan, from the list (the entry) to
// the detail page, its `on_empty` key mapped to the list page.
fn scan_package() -> Vec<u8> {
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.linear.scan",
        "execution_mode": "linear_steps",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": SCAN_WIDTH, "height": SCAN_HEIGHT},
        "entry_task_id": "task",
        "capture_interval_ms": 100,
        "step_timeout_ms": 32_000,
        "timeout_ms": 300_000,
        "max_steps": 1
    });
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": SCAN_WIDTH, "height": SCAN_HEIGHT},
        "entry_page": "list",
        "target_page": "detail",
        "max_steps": 1,
        "scheduling_outcome": {"mappings": [mapping("card_selected", "detail"),
                                            mapping("card_empty", "list")]},
        "candidate_layouts": [scan_layout()],
        "operations": [{
            "id": "pick", "from": "list", "to": "detail",
            "expect_after": {"page_id": "detail", "timeout_ms": 5000, "interval_ms": 200},
            "guard": {"page_id": "list", "target_id": "guard/list",
                      "expected_rect": {"x": 3, "y": 0, "width": 1, "height": 1},
                      "color_probe": "guard/list"},
            "select": {"layout_id": "layout/cards", "policy": {"path": "policies/select.json"},
                       "scan": scan_declaration(), "on_empty": {"outcome_key": "card_empty"}}
        }]
    });
    let policy = json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "cards",
        "applies_to": {
            "candidate_layout_id": "layout/cards",
            "outcome_keys": {"selected": "card_selected", "empty": "card_empty",
                             "insufficient": "card_insufficient",
                             "ambiguous": "card_ambiguous", "unknown": "card_unknown"}
        },
        "fields": [
            {"name": "member", "value_type": {"type": "enum_string", "allowed": scan_identities()}},
            {"name": "open", "value_type": {"type": "boolean"}}
        ],
        "facts": [],
        "gates": [{"gate_id": "open",
                   "predicate": {"kind": "boolean_equals",
                                 "value": {"source": "field", "field": "open"},
                                 "expected": true},
                   "on_unknown": {"kind": "drop_candidate"}}],
        "scoring": [],
        "selection": {"mode": "none_allowed", "required_count": 1},
        "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
    });
    let colour = |id: &str, x: u32, expected: [u8; 3]| {
        json!({"type": "color", "id": id,
               "region": {"x": x, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    let text = |id: &str, region: Value, expected: &str| {
        json!({"type": "ocr", "id": id, "region": region, "languages": ["en"],
               "timeout_ms": 2000, "match_mode": "exact", "expected": [expected],
               "case_sensitive": false, "minimum_confidence": 0.8,
               "model_ref": "PP-OCRv6_medium", "model_sha256": MODEL_SHA256})
    };
    let pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": SCAN_WIDTH, "height": SCAN_HEIGHT},
        "defaults": {"match_metric": "ccoeff_normed", "color_max_distance": 0.0},
        "targets": [
            colour("page/list", 0, [255, 0, 0]),
            colour("page/detail", 0, [0, 0, 255]),
            colour("check/list_start", 1, [0, 255, 0]),
            colour("check/list_end", 2, [0, 0, 255]),
            colour("guard/list", 3, [0, 255, 0]),
            {"type": "template", "id": "ui/card", "template_path": "assets/card.png",
             "region": "full_frame", "threshold": 0.9},
            text("card/name", json!({"x": 0, "y": 0, "width": 100, "height": 20}), "member 00"),
            {"type": "color", "id": "card/open",
             "region": {"x": 0, "y": 0, "width": 20, "height": 20}, "expected": [0, 255, 0]}
        ],
        "candidate_layouts": [scan_layout()]
    });
    let pages = ["list", "detail"].map(|page| {
        json!({"id": format!("neutral/{page}"), "required": [format!("page/{page}")],
               "optional": [], "forbidden": []})
    });
    zip_entries(&[
        ("control.json", bytes(&control)),
        (
            "resources/manifest.json",
            bytes(&json!({"schema_version": "0.3", "entry_task_id": "task"})),
        ),
        ("resources/operations/task/task.json", bytes(&task)),
        (
            "resources/operations/task/policies/select.json",
            bytes(&policy),
        ),
        ("resources/assets/card.png", template_png(16, 12, 1)),
        ("resources/recognition/neutral.test.pack.json", bytes(&pack)),
        (
            "resources/recognition/neutral.test.pages.json",
            bytes(&json!({"schema_version": "0.3", "pages": pages})),
        ),
    ])
}

// A.1: a linear package whose one step is a select over a scan, with `on_empty` and its page mapped
// to its key, is admitted by the whole admission; the step is prepared with its identity feature,
// its key and its scan's time limit, which replaces the step timeout as on the page graph.
#[test]
fn k1_a_scan_select_with_on_empty_is_admitted_under_linear_steps() {
    let task = PreparedContainedTask::load("neutral.instance", &scan_package())
        .unwrap_or_else(|error| panic!("refused: {error}"));
    let [operation] = task.program.operations.as_slice() else {
        panic!("one operation");
    };
    let select = operation
        .prepared_select
        .as_deref()
        .expect("the select step is prepared");
    assert_eq!(select.identity_feature(), Some("member"));
    assert_eq!(operation.on_empty_key(), Some("card_empty"));
    assert_eq!(operation.scan_max_ms(), Some(SCAN_MAX_MS));
}
