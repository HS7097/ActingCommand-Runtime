// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 K2 (register §1.380; contracts/linear-steps.md, "Endings"; the design note
// `rd-1114/463-l3l5-design.md`, section B, and the coordinator's rulings 4 and 5 on board #463): a
// `linear_steps` task names more than one normal end. An ending is a `scheduling_outcome` mapping
// whose terminal page is neither the target page nor a select step's `on_empty` page; its page is
// no page of the path. Wherever the target page is a candidate at the path's end, the ending pages
// are candidates right before it, in mapping order; one that passes passes at once, and the run
// finishes Completed on it with no key of the kernel's. The retry decision and the closing decision
// await the same list, then the step's own page: a late ending continues a retried attempt and
// still fails under the closing decision; a still screen that shows none of them is U1's code with
// the endings in its `awaited=`. A package with an ending is no prerequisite and no return-home
// package (`linear_endings`); a package without one awaits and fails as before. Fix round 1
// (`rd-1115/brief-1727-fix1.md`, items 3 and 6): an ending page a confirmation adds as a loading
// page or waits on is `ending_page`, a `store_update` package declares no ending page
// (`store_update_endings`), and a select step's empty end keeps its `on_empty` key beside an ending.
//
// Every expected value comes from those lines and the package values below, never from the code.
// The packages are synthetic `neutral` packages, admitted by the kernel's own admission
// (`PreparedContainedTask::load_with_vision_provider`, or `validate_linear` for a refusal). A frame
// is 6 x 1: pixel 0 names the path's page (home, list, popup, done; black is none of them), pixel 1
// is the select step's guard, pixels 2-3 and 4-5 are the list's two cards, each a name code and an
// open mark; a white pixel 4 marks the ending page `short`, a white pixel 5 the ending page `spare`,
// so an ending can show on the frame the target page shows on. A scripted OCR provider reads a
// card's name in the pixel its region starts on. The path taps from home into the list (`open`)
// and from the list to done (`buy`); variants add an optional step, a retry or a select step. The
// runs play scripted frames on the kernel's scripted clock (EXEC-I4): the device shows `screens[n]`
// after `n` inputs, each segment's last frame repeating.

use super::tests::scripted::{ScriptedFrame, ScriptedRuntime};
use super::*;
use actingcommand_contract::{
    InstanceFactContext, InstanceFactSnapshot, MaintenanceUse, TaskSelectionOutcome,
    TaskSelectionRecord,
};
use actingcommand_recognition_pack::{
    NnProviderRequest, NnProviderResult, OcrProviderRequest, OcrProviderResult,
    OcrProviderTextBlock, VisionProviderError, VisionProviderErrorCode,
};
use serde_json::{Value, json};
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

const LINEAR_INVALID: &str = ExecutionKernelCode::ContainedTaskLinearInvalid.as_str();
const DECLARATION_INVALID: &str =
    ExecutionKernelCode::ContainedTaskOutcomeDeclarationInvalid.as_str();
const CONFIRMATION_FAILED: &str = ExecutionKernelCode::PageConfirmationFailed.as_str();
const UNRECORDED_SCREEN: &str = ExecutionKernelCode::ContainedTaskLinearUnrecordedScreen.as_str();

const WIDTH: u32 = 6;
const STEP_MS: u64 = 5_000;
const ARRIVAL_MS: u64 = 1_000;
const TASK_MS: u64 = 60_000;
// A wait of 1 000 ms and one of 5 000 ms at 50 ms, each from a first frame taken at once.
const ARRIVAL_FRAMES: usize = 21;
const CLOSING_FRAMES: usize = 101;
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
// The key of the usual ending, the keys of the two endings, and the `on_empty` key of `pick`.
const DONE_KEY: &str = "task_done";
const SHORT_KEY: &str = "task_short";
const SPARE_KEY: &str = "task_spare";
const NOT_FOUND: &str = "task_not_found";
// The detector pages.
const LIST_PAGE: &str = "neutral/list";
const POPUP_PAGE: &str = "neutral/popup";
const DONE_PAGE: &str = "neutral/done";
const SHORT_PAGE: &str = "neutral/short";
const SPARE_PAGE: &str = "neutral/spare";
const UNRECOGNIZED: &str = "<unrecognized>";

const HOME: [u8; 3] = [255, 0, 255];
const LIST: [u8; 3] = [255, 0, 0];
const POPUP: [u8; 3] = [0, 0, 255];
const DONE: [u8; 3] = [255, 255, 0];
const GREEN: [u8; 3] = [0, 255, 0];
const BLACK: [u8; 3] = [0, 0, 0];
const WHITE: [u8; 3] = [255, 255, 255];
// The name codes a card's name pixel shows (0 reads nothing), and the text each one reads as.
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

// A screen of the path's `page` with no card on it and no ending mark.
fn plain(page: [u8; 3]) -> ScriptedFrame {
    ScriptedFrame::rgb(WIDTH, 1, &[page, GREEN, BLACK, BLACK, BLACK, BLACK])
}

// A screen of the path's `page` (black: none) with the ending marks `short` and `spare`.
fn ending(page: [u8; 3], short: bool, spare: bool) -> ScriptedFrame {
    let mark = |shown: bool| if shown { WHITE } else { BLACK };
    ScriptedFrame::rgb(
        WIDTH,
        1,
        &[page, GREEN, BLACK, BLACK, mark(short), mark(spare)],
    )
}

// The list with its two cards, each a name code and whether it is open.
fn list(first: (u8, bool), second: (u8, bool)) -> ScriptedFrame {
    let lit = |open: bool| if open { GREEN } else { BLACK };
    ScriptedFrame::rgb(
        WIDTH,
        1,
        &[
            LIST,
            GREEN,
            [first.0, 0, 0],
            lit(first.1),
            [second.0, 0, 0],
            lit(second.1),
        ],
    )
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

// A member set over the cards: every open card is a member, of equal score, `none_allowed`, the
// lowest candidate ID first.
fn member_policy() -> Value {
    json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "cards",
        "applies_to": {
            "candidate_layout_id": "layout/cards",
            "outcome_keys": {"selected": "card_selected", "empty": "card_empty",
                             "insufficient": "card_insufficient",
                             "ambiguous": "card_ambiguous", "unknown": "card_unknown"}
        },
        "fields": [
            {"name": "member",
             "value_type": {"type": "enum_string", "allowed": ["member-a", "member-b"]}},
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
    })
}

// The linear control of a path of `max_steps` operations.
fn control(max_steps: u32) -> Value {
    json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.linear.endings",
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

// The select step `pick`: from the list, guarded, it taps the chosen card into done; nobody open
// ends the run on the list with `NOT_FOUND`.
fn pick() -> Value {
    json!({
        "id": "pick", "from": "list", "to": "done",
        "expect_after": {"page_id": "done", "timeout_ms": ARRIVAL_MS},
        "guard": {"page_id": "list", "target_id": "guard/ready",
                  "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                  "color_probe": "guard/ready"},
        "select": {"layout_id": "layout/cards", "policy": {"path": "policies/select.json"},
                   "on_empty": {"outcome_key": NOT_FOUND}}
    })
}

fn mapping(key: &str, page: &str) -> Value {
    json!({"outcome_key": key, "effect": "no_designated_effect", "terminal_pages": [page]})
}

// The usual ending and the two endings, in that order.
fn endings() -> Vec<Value> {
    vec![
        mapping(DONE_KEY, "done"),
        mapping(SHORT_KEY, "short"),
        mapping(SPARE_KEY, "spare"),
    ]
}

// A task from home to done over `operations`, with the scheduling outcome `mappings`.
fn task(operations: Vec<Value>, mappings: Vec<Value>) -> Value {
    let max_steps = operations.len();
    json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": WIDTH, "height": 1},
        "entry_page": "home",
        "target_page": "done",
        "max_steps": max_steps,
        "scheduling_outcome": {"mappings": mappings},
        "operations": operations
    })
}

// The base task: `open` and `buy`, from home to done.
fn buy_task(mappings: Vec<Value>) -> Value {
    task(
        vec![tap("open", "home", "list"), tap("buy", "list", "done")],
        mappings,
    )
}

// The page set: home, list, popup and done told by pixel 0; short and spare by pixels 4 and 5.
fn pages() -> Value {
    let pages = ["home", "list", "popup", "done", "short", "spare"].map(|page| {
        json!({"id": format!("neutral/{page}"), "required": [format!("page/{page}")],
               "optional": [], "forbidden": []})
    });
    json!({"schema_version": "0.3", "pages": pages})
}

// The recognition pack: the pages' marks, the guard on pixel 1 and, with `cards`, each card's name
// (OCR) and open mark and the task's candidate layouts.
fn pack(cards: Option<&Value>) -> Value {
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
    let mut targets = vec![
        colour("page/home", 0, HOME),
        colour("page/list", 0, LIST),
        colour("page/popup", 0, POPUP),
        colour("page/done", 0, DONE),
        colour("page/short", 4, WHITE),
        colour("page/spare", 5, WHITE),
        colour("guard/ready", 1, GREEN),
    ];
    if cards.is_some() {
        targets.extend([
            text("card/first_name", 2),
            colour("card/first_open", 3, GREEN),
            text("card/second_name", 4),
            colour("card/second_open", 5, GREEN),
        ]);
    }
    let mut pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": WIDTH, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": targets
    });
    if let Some(layouts) = cards {
        pack["candidate_layouts"] = layouts.clone();
    }
    pack
}

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("JSON bytes")
}

// The package of `task` under `control`; a task with candidate layouts carries `member_policy` as
// its select step's document.
fn package(control: &Value, task: &Value) -> Vec<u8> {
    let layouts = task.get("candidate_layouts");
    let mut entries = vec![
        ("control.json", bytes(control)),
        (
            "resources/manifest.json",
            bytes(&json!({"schema_version": "0.3", "entry_task_id": "task"})),
        ),
        ("resources/operations/task/task.json", bytes(task)),
        (
            "resources/recognition/neutral.test.pack.json",
            bytes(&pack(layouts)),
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            bytes(&pages()),
        ),
    ];
    if layouts.is_some() {
        entries.push((
            "resources/operations/task/policies/select.json",
            bytes(&member_policy()),
        ));
    }
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, content) in entries {
        zip.start_file(path, options).expect("zip entry");
        zip.write_all(&content).expect("zip bytes");
    }
    zip.finish().expect("finish zip").into_inner()
}

// The package as contained task admission prepares it, with the scripted OCR provider.
fn admitted(case: &str, control: &Value, task: &Value) -> PreparedContainedTask {
    PreparedContainedTask::load_with_vision_provider(
        "neutral.instance",
        &package(control, task),
        Arc::new(ShownText),
    )
    .unwrap_or_else(|error| panic!("{case}: refused: {error}"))
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
// select step asks for, telling the kernel which of its errors are bookkeeping failures as the
// scripted runtime it wraps does.
struct Device {
    script: ScriptedRuntime,
}

impl Device {
    fn new(screens: Vec<Vec<ScriptedFrame>>) -> Self {
        Self {
            script: ScriptedRuntime::new(screens),
        }
    }

    fn inputs(&self) -> &[InputAction] {
        self.script.inputs()
    }

    fn traces(&self) -> &[ContainedTaskTrace] {
        self.script.traces()
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

    // The candidate lists the recognitions after `operation`'s completed effect evaluated, each
    // list once for its consecutive frames, in order.
    fn awaited_after(&self, operation: &str) -> Vec<Vec<String>> {
        let mut lists: Vec<Vec<String>> = Vec::new();
        let mut after = false;
        for trace in self.traces() {
            match trace {
                ContainedTaskTrace::EffectCompleted {
                    operation_label, ..
                } if operation_label == operation => after = true,
                ContainedTaskTrace::RecognitionStarted {
                    candidate_pages, ..
                } if after && lists.last() != Some(candidate_pages) => {
                    lists.push(candidate_pages.clone());
                }
                _ => {}
            }
        }
        lists
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
        self.script.record(trace)
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

    fn is_bookkeeping_failure(error: &Self::Error) -> bool {
        <ScriptedRuntime as ContainedTaskRuntime>::is_bookkeeping_failure(error)
    }
}

// The base path's screens: home, then the list, then `after_buy`, what the device shows after the
// last tap, its frames in order.
fn path(after_buy: Vec<ScriptedFrame>) -> Vec<Vec<ScriptedFrame>> {
    vec![vec![plain(HOME)], vec![plain(LIST)], after_buy]
}

// `count` black frames of no page, then the frame `then`.
fn black_then(count: usize, then: ScriptedFrame) -> Vec<ScriptedFrame> {
    let mut frames = vec![plain(BLACK); count];
    frames.push(then);
    frames
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

// The run's normal end on `page` after `executed_steps`, with no key of the kernel's.
fn completed(page: &str, executed_steps: u32) -> ContainedTaskOutcome {
    ContainedTaskOutcome {
        outcome: TaskOutcome::Success,
        final_page: Some(page.to_owned()),
        executed_steps,
        selected_scheduling_outcome: None,
        designated_effect_caps: Vec::new(),
    }
}

fn pages_of(pages: &[&str]) -> Vec<String> {
    pages.iter().map(|page| (*page).to_owned()).collect()
}

// The failure keeps the arrival's timing: postcondition, its 1 000 ms of 1 000.
fn assert_arrival_timing(error: &ContainedTaskError) {
    let timing = error.timing().expect("the failure's timing");
    assert_eq!(timing.scope, TaskTimingScope::Postcondition, "{error}");
    assert_eq!(timing.stage, TaskTimingStage::Postcondition, "{error}");
    assert_eq!(timing.elapsed_ms, ARRIVAL_MS, "{error}");
    assert_eq!(timing.limit_ms, ARRIVAL_MS, "{error}");
}

// B.1: a linear task whose scheduling outcome names one ending page beside the target page, and
// one that names two, is admitted by linear admission and by the whole admission.
#[test]
fn k2_a_linear_task_with_one_or_two_endings_is_admitted() {
    for (case, mappings) in [
        (
            "one ending",
            vec![mapping(DONE_KEY, "done"), mapping(SHORT_KEY, "short")],
        ),
        ("two endings", endings()),
    ] {
        linear_admission(control(2), buy_task(mappings.clone()))
            .unwrap_or_else(|error| panic!("{case}: refused: {error}"));
        admitted(case, &control(2), &buy_task(mappings));
    }
}

// B.1: each rule an ending is held to, on the base task changed in the one declaration under test,
// with its code and detail. An ending page that an operation's intermediate page is: `ending_page`.
// One detector page in two mappings, written with and without the game prefix (an ending, the
// target page): `terminal_page_duplicate`; written alike, the declaration's own check refuses it
// first (`contained_task_outcome_declaration_invalid`). A page of the path named as a terminal page
// (an optional step's page) stays `terminal_page`.
#[test]
fn k2_linear_admission_refuses_each_ending_rule_with_its_reason() {
    let mut intermediate = buy_task(endings());
    intermediate["operations"][0]["transition"] = json!({"kind": "page", "page_id": "short"});
    let optional_page = task(
        vec![
            tap("open", "home", "list"),
            tap("buy", "list", "popup"),
            optional(tap("dismiss", "popup", "done")),
        ],
        vec![mapping(DONE_KEY, "done"), mapping(SHORT_KEY, "popup")],
    );
    let cases = [
        Refusal {
            case: "an ending page that is an operation's intermediate page",
            task: intermediate,
            answer: LINEAR_INVALID,
            detail: Some("reason=ending_page"),
        },
        Refusal {
            case: "an ending page in two mappings, written two ways",
            task: buy_task(vec![
                mapping(DONE_KEY, "done"),
                mapping(SHORT_KEY, "short"),
                mapping(SPARE_KEY, "neutral/short"),
            ]),
            answer: LINEAR_INVALID,
            detail: Some("reason=terminal_page_duplicate"),
        },
        Refusal {
            case: "the target page in two mappings, written two ways",
            task: buy_task(vec![
                mapping(DONE_KEY, "done"),
                mapping(SHORT_KEY, "neutral/done"),
            ]),
            answer: LINEAR_INVALID,
            detail: Some("reason=terminal_page_duplicate"),
        },
        Refusal {
            case: "an ending page in two mappings, written alike",
            task: buy_task(vec![
                mapping(DONE_KEY, "done"),
                mapping(SHORT_KEY, "short"),
                mapping(SPARE_KEY, "short"),
            ]),
            answer: DECLARATION_INVALID,
            detail: None,
        },
        Refusal {
            case: "an optional step's page named as a terminal page",
            task: optional_page,
            answer: LINEAR_INVALID,
            detail: Some("reason=terminal_page"),
        },
    ];
    for Refusal {
        case,
        task,
        answer,
        detail,
    } in cases
    {
        let steps = u32::try_from(task["operations"].as_array().expect("operations").len())
            .expect("a few operations");
        let error = linear_admission(control(steps), task)
            .err()
            .unwrap_or_else(|| panic!("{case}: admitted"));
        assert_eq!(error.code().as_str(), answer, "{case}: {error}");
        assert_eq!(error.detail(), detail, "{case}: {error}");
    }
}

// Fix round 1 (`rd-1115/brief-1727-fix1.md`, item 6), B.1 as above: an ending page that `buy`'s
// confirmation adds as a loading page (`expect_after.loading.add`), and one that is `buy`'s wait
// page (`expect_after.wait_page`), is a page `buy`'s waits evaluate apart from its candidates, so
// linear admission refuses it `ending_page`, as it refuses an operation's intermediate page.
#[test]
fn k2_an_ending_page_that_a_confirmation_adds_or_waits_on_is_refused_ending_page() {
    for (case, member, declared) in [
        (
            "a loading page buy's confirmation adds",
            "loading",
            json!({"add": ["short"], "max_ms": 60_000}),
        ),
        (
            "buy's wait page",
            "wait_page",
            json!({"page_id": "short", "max_ms": 10_000}),
        ),
    ] {
        let mut adding = buy_task(endings());
        adding["operations"][1]["expect_after"][member] = declared;
        let error = linear_admission(control(2), adding)
            .err()
            .unwrap_or_else(|| panic!("{case}: admitted"));
        assert_eq!(error.code().as_str(), LINEAR_INVALID, "{case}: {error}");
        assert_eq!(
            error.detail(),
            Some("reason=ending_page"),
            "{case}: {error}"
        );
    }
}

// Fix round 1 (`rd-1115/brief-1727-fix1.md`, item 3): any success of a package that declares
// `store_update` clears the instance's update requirement (UPD-1), a run that finished on an
// ending page included, so linear admission refuses an ending page in such a package
// (`store_update_endings`); the same package without its endings passes linear admission.
#[test]
fn k2_a_store_update_package_that_declares_an_ending_page_is_refused() {
    let mut store = control(2);
    store["store_update"] = json!({
        "store_application_id": "test.store.alpha",
        "store_operations": [{"operation": "buy", "proof": ["page/list"]}]
    });
    let error = linear_admission(store.clone(), buy_task(endings()))
        .err()
        .unwrap_or_else(|| panic!("a store update package with endings: admitted"));
    assert_eq!(error.code().as_str(), LINEAR_INVALID, "{error}");
    assert_eq!(
        error.detail(),
        Some("reason=store_update_endings"),
        "{error}"
    );
    linear_admission(store, buy_task(vec![mapping(DONE_KEY, "done")]))
        .unwrap_or_else(|error| panic!("a store update package without endings: refused: {error}"));
}

// One admission case: the task changed in one declaration, and the refusal's code and detail.
struct Refusal {
    case: &'static str,
    task: Value,
    answer: &'static str,
    detail: Option<&'static str>,
}

// B.1: an ending may end the run short of the target page, so short of a dependent package's
// entry page: the package is no prerequisite package, no return-home package, configured or not,
// and no entry recovery package (`linear_endings`); the same package without its endings is each.
#[test]
fn k2_a_package_with_endings_is_no_prerequisite_and_no_return_home_package() {
    let with = admitted("with endings", &control(2), &buy_task(endings()));
    assert_eq!(with.prerequisite_incompatibility(), Some("linear_endings"));
    assert!(!with.is_entry_recovery_compatible());
    let descriptor = with.package_descriptor();
    for configured_return_home in [false, true] {
        assert_eq!(
            descriptor.prerequisite_incompatibility_with(
                "neutral",
                "test",
                (WIDTH, 1),
                configured_return_home
            ),
            Some("linear_endings"),
            "configured return home {configured_return_home}"
        );
    }
    assert_eq!(
        descriptor.use_incompatibility(MaintenanceUse::ReturnHome),
        Some("linear_endings")
    );
    let without = admitted(
        "without endings",
        &control(2),
        &buy_task(vec![mapping(DONE_KEY, "done")]),
    );
    assert_eq!(without.prerequisite_incompatibility(), None);
    assert!(without.is_entry_recovery_compatible());
}

// The operation `operation`, optional with a settle of 100 ms.
fn optional(mut operation: Value) -> Value {
    operation["optional"] = json!({"settle_ms": 100});
    operation
}

// B.2: after `buy`'s tap the device shows the ending `short`. Its arrival awaits the endings in
// mapping order, then the target page; `short` passes at once, `buy`'s `StepFinished` names it and
// the run is Completed on it after both steps, with no key of the kernel's. After the same tap the
// device shows done: the usual ending still ends the run on the target page.
#[test]
fn k2_the_last_input_reaching_an_ending_completes_the_run_on_it() {
    let task = admitted("two endings", &control(2), &buy_task(endings()));
    for (case, after_buy, page) in [
        ("the ending short", ending(BLACK, true, false), SHORT_PAGE),
        ("the usual ending", plain(DONE), DONE_PAGE),
    ] {
        let mut device = Device::new(path(vec![after_buy]));

        let outcome = task
            .run(&mut device)
            .unwrap_or_else(|error| panic!("{case}: expected a normal end, got {error:?}"));

        assert_eq!(outcome, completed(page, 2), "{case}");
        assert_eq!(
            device.finished(),
            [(0, "open", LIST_PAGE), (1, "buy", page)],
            "{case}"
        );
        assert_eq!(device.inputs().len(), 2, "{case}: {:?}", device.inputs());
        assert_eq!(
            device.awaited_after("buy"),
            [pages_of(&[SHORT_PAGE, SPARE_PAGE, DONE_PAGE])],
            "{case}"
        );
    }
}

// The coordinator's ruling 4: after `buy`'s tap one frame shows the target page and the ending
// `short` together. The ending, awaited first, wins: the run is Completed on `short`.
#[test]
fn k2_an_ending_and_the_target_page_on_one_frame_end_on_the_ending() {
    let task = admitted("two endings", &control(2), &buy_task(endings()));
    let mut device = Device::new(path(vec![ending(DONE, true, false)]));

    let outcome = task
        .run(&mut device)
        .unwrap_or_else(|error| panic!("expected a normal end, got {error:?}"));

    assert_eq!(outcome, completed(SHORT_PAGE, 2));
    assert_eq!(
        device.finished(),
        [(0, "open", LIST_PAGE), (1, "buy", SHORT_PAGE)]
    );
}

// B.2: `buy` leads to `popup`, the page of the optional last step `dismiss`, so the run of
// `dismiss` ends the path and its skip target is the target page. `buy`'s candidates are the popup,
// then the endings, then the target page: `short` after `buy`'s tap ends the run on it after two
// steps. When the popup comes, `dismiss` runs, awaiting the endings and the target page; `spare`
// after its tap ends the run on it after three steps.
#[test]
fn k2_endings_after_a_run_of_optional_steps_that_ends_the_path() {
    let task = admitted(
        "an optional last step",
        &control(3),
        &task(
            vec![
                tap("open", "home", "list"),
                tap("buy", "list", "popup"),
                optional(tap("dismiss", "popup", "done")),
            ],
            endings(),
        ),
    );

    let mut device = Device::new(path(vec![ending(BLACK, true, false)]));
    let outcome = task
        .run(&mut device)
        .unwrap_or_else(|error| panic!("short after buy: expected a normal end, got {error:?}"));
    assert_eq!(outcome, completed(SHORT_PAGE, 2));
    assert_eq!(
        device.finished(),
        [(0, "open", LIST_PAGE), (1, "buy", SHORT_PAGE)]
    );
    assert_eq!(
        device.awaited_after("buy"),
        [pages_of(&[POPUP_PAGE, SHORT_PAGE, SPARE_PAGE, DONE_PAGE])]
    );

    let mut screens = path(vec![plain(POPUP)]);
    screens.push(vec![ending(BLACK, false, true)]);
    let mut device = Device::new(screens);
    let outcome = task.run(&mut device).unwrap_or_else(|error| {
        panic!("spare after dismiss: expected a normal end, got {error:?}")
    });
    assert_eq!(outcome, completed(SPARE_PAGE, 3));
    assert_eq!(
        device.finished(),
        [
            (0, "open", LIST_PAGE),
            (1, "buy", POPUP_PAGE),
            (2, "dismiss", SPARE_PAGE)
        ]
    );
    assert_eq!(
        device.awaited_after("dismiss"),
        [pages_of(&[SHORT_PAGE, SPARE_PAGE, DONE_PAGE])]
    );
    assert_eq!(device.inputs().len(), 3, "{:?}", device.inputs());
}

// B.4 item 4: `buy` declares a retry. After its tap the device shows a black screen for the
// arrival's 21 frames, then `short`. The arrival misses; the retry decision, 10 ms later, awaits
// the endings, the target page and then `buy`'s own page, and its first frame shows `short`: the
// input took effect late, the attempt goes on without another input and the run is Completed on
// `short` after both steps, one `StepFinished` of `buy` naming it.
#[test]
fn k2_a_late_ending_in_the_retry_decision_continues_without_another_input() {
    let mut retried = tap("buy", "list", "done");
    retried["retryable"] = json!(true);
    retried["max_attempts"] = json!(2);
    retried["retry_interval_ms"] = json!(10);
    let task = admitted(
        "a retried last step",
        &control(2),
        &task(vec![tap("open", "home", "list"), retried], endings()),
    );
    let mut device = Device::new(path(black_then(ARRIVAL_FRAMES, ending(BLACK, true, false))));

    let outcome = task
        .run(&mut device)
        .unwrap_or_else(|error| panic!("expected a normal end, got {error:?}"));

    assert_eq!(outcome, completed(SHORT_PAGE, 2));
    assert_eq!(
        device.finished(),
        [(0, "open", LIST_PAGE), (1, "buy", SHORT_PAGE)]
    );
    assert_eq!(device.inputs().len(), 2, "{:?}", device.inputs());
    assert_eq!(
        device.awaited_after("buy"),
        [
            pages_of(&[SHORT_PAGE, SPARE_PAGE, DONE_PAGE]),
            pages_of(&[SHORT_PAGE, SPARE_PAGE, DONE_PAGE, LIST_PAGE])
        ]
    );
    assert_eq!(device.script.captures_after(2), ARRIVAL_FRAMES + 1);
}

// B.4 item 4 with Workflow #463 U1's closing decision: `buy` declares no retry, so its arrival is
// its last attempt. After its tap the device shows a black screen for the arrival's 21 frames, then
// `short`. The closing decision awaits the endings, the target page and then `buy`'s own page, and
// its first frame shows `short`: the task still fails with `page_confirmation_failed`, its detail
// naming the awaited endings and the target page, the arrival's timing kept, never a success.
#[test]
fn k2_a_late_ending_in_the_closing_decision_still_fails() {
    let task = admitted("two endings", &control(2), &buy_task(endings()));
    let mut device = Device::new(path(black_then(ARRIVAL_FRAMES, ending(BLACK, true, false))));

    let error = failed("a late ending", task.run(&mut device));

    assert_eq!(error.code().as_str(), CONFIRMATION_FAILED, "{error}");
    assert_eq!(
        error.detail(),
        Some(
            "operation=buy attempts=1 after_page=<unrecognized> hit_error_page=false \
             transition=none awaited=neutral/short,neutral/spare,neutral/done"
        ),
        "{error}"
    );
    assert_arrival_timing(&error);
    assert_eq!(
        device.finished(),
        [(0, "open", LIST_PAGE), (1, "buy", UNRECOGNIZED)]
    );
    assert_eq!(device.inputs().len(), 2, "{:?}", device.inputs());
    assert_eq!(
        device.awaited_after("buy"),
        [
            pages_of(&[SHORT_PAGE, SPARE_PAGE, DONE_PAGE]),
            pages_of(&[SHORT_PAGE, SPARE_PAGE, DONE_PAGE, LIST_PAGE])
        ]
    );
    assert_eq!(device.script.captures_after(2), ARRIVAL_FRAMES + 1);
}

// B.4 item 4 with Workflow #463 U1: after `buy`'s tap the device shows a black screen on every
// frame, neither an ending, the target page nor the list. The arrival runs out and the closing
// decision observes its bound on a still screen: the run ends with
// `contained_task_linear_unrecorded_screen`, the detail `page_confirmation_failed` gives the same
// end with `awaited=` listing the endings and the target page, the values naming `buy`, the pages
// the decision awaited (the endings, the target page, then the list) and one attempt.
#[test]
fn k2_neither_page_on_a_still_screen_is_the_unrecorded_screen_with_the_endings_awaited() {
    let task = admitted("two endings", &control(2), &buy_task(endings()));
    let mut device = Device::new(path(vec![plain(BLACK)]));

    let error = failed("a still black screen", task.run(&mut device));

    assert_eq!(error.code().as_str(), UNRECORDED_SCREEN, "{error}");
    assert_eq!(
        error.detail(),
        Some(
            "operation=buy attempts=1 after_page=<unrecognized> hit_error_page=false \
             transition=none awaited=neutral/short,neutral/spare,neutral/done"
        ),
        "{error}"
    );
    assert_eq!(value(&error, "pack_operation_id"), json!("buy"));
    assert_eq!(
        value(&error, "page_ids"),
        json!([SHORT_PAGE, SPARE_PAGE, DONE_PAGE, LIST_PAGE])
    );
    assert_eq!(value(&error, "attempts"), json!(1));
    assert_arrival_timing(&error);
    assert_eq!(
        device.awaited_after("buy"),
        [
            pages_of(&[SHORT_PAGE, SPARE_PAGE, DONE_PAGE]),
            pages_of(&[SHORT_PAGE, SPARE_PAGE, DONE_PAGE, LIST_PAGE])
        ]
    );
    assert_eq!(
        device.script.captures_after(2),
        ARRIVAL_FRAMES + CLOSING_FRAMES
    );
}

// B.2, the last sentence: the same package without its endings, on the same still black screen.
// The last step awaits the target page alone and its closing decision the target page and the list,
// as before endings existed: U1's code, the detail with no `awaited=`, the pages the target page and
// the list.
#[test]
fn k2_without_endings_the_last_step_awaits_and_fails_as_before() {
    let task = admitted(
        "no ending",
        &control(2),
        &buy_task(vec![mapping(DONE_KEY, "done")]),
    );
    let mut device = Device::new(path(vec![plain(BLACK)]));

    let error = failed("a still black screen", task.run(&mut device));

    assert_eq!(error.code().as_str(), UNRECORDED_SCREEN, "{error}");
    assert_eq!(
        error.detail(),
        Some(
            "operation=buy attempts=1 after_page=<unrecognized> hit_error_page=false transition=none"
        ),
        "{error}"
    );
    assert_eq!(value(&error, "page_ids"), json!([DONE_PAGE, LIST_PAGE]));
    assert_eq!(
        device.awaited_after("buy"),
        [pages_of(&[DONE_PAGE]), pages_of(&[DONE_PAGE, LIST_PAGE])]
    );
    assert_eq!(
        device.script.captures_after(2),
        ARRIVAL_FRAMES + CLOSING_FRAMES
    );
}

// A select step followed by an ending in one task: `pick`, the last step, selects the first card
// (open) of the list and taps it once; the list is its `on_empty` page, mapped to its key, and
// `short` is an ending. After the tap the device shows `short`: `pick`'s arrival awaits the ending,
// then the target page, and the run is Completed on `short` after both steps.
#[test]
fn k2_a_select_step_whose_tap_reaches_an_ending_completes_the_run_on_it() {
    let mut select = task(
        vec![tap("open", "home", "list"), pick()],
        vec![
            mapping(DONE_KEY, "done"),
            mapping(NOT_FOUND, "list"),
            mapping(SHORT_KEY, "short"),
        ],
    );
    select["candidate_layouts"] = json!([cards_layout()]);
    let task = admitted("a select step and an ending", &control(2), &select);
    let mut device = Device::new(vec![
        vec![plain(HOME)],
        vec![list((ALPHA, true), (BRAVO, false))],
        vec![ending(BLACK, true, false)],
    ]);

    let outcome = task
        .run(&mut device)
        .unwrap_or_else(|error| panic!("expected a normal end, got {error:?}"));

    assert_eq!(outcome, completed(SHORT_PAGE, 2));
    let records = device.records();
    let [(1, record)] = records.as_slice() else {
        panic!("one decision record of step 1, got {}", records.len());
    };
    assert_eq!(
        record.selection_result,
        TaskSelectionOutcome::Selected { count: 1 }
    );
    assert_eq!(device.inputs().len(), 2, "{:?}", device.inputs());
    assert!(
        matches!(device.inputs()[1], InputAction::Tap { x: 2..=3, y: 0 }),
        "{:?}",
        device.inputs()[1]
    );
    assert_eq!(
        device.finished(),
        [(0, "open", LIST_PAGE), (1, "pick", SHORT_PAGE)]
    );
    assert_eq!(
        device.awaited_after("pick"),
        [pages_of(&[SHORT_PAGE, DONE_PAGE])]
    );
}

// Fix round 1 (`rd-1115/brief-1727-fix1.md`, item 6), Workflow #463 K1's empty end in a package
// that also names an ending: the same task as above, both cards closed and both read, so `pick`'s
// policy (`none_allowed`) decides `empty` with nothing unknown. `pick` declares `on_empty`: it taps
// nothing, its `StepFinished` names the list and the run is Completed on the list with its
// `on_empty` key `NOT_FOUND`, both steps counted, as without an ending; the ending is no candidate
// of an end that taps nothing.
#[test]
fn k2_a_select_steps_empty_end_in_a_package_with_an_ending_keeps_its_key() {
    let mut select = task(
        vec![tap("open", "home", "list"), pick()],
        vec![
            mapping(DONE_KEY, "done"),
            mapping(NOT_FOUND, "list"),
            mapping(SHORT_KEY, "short"),
        ],
    );
    select["candidate_layouts"] = json!([cards_layout()]);
    let task = admitted("a select step and an ending", &control(2), &select);
    let mut device = Device::new(vec![
        vec![plain(HOME)],
        vec![list((ALPHA, false), (BRAVO, false))],
        vec![plain(DONE)],
    ]);

    let outcome = task
        .run(&mut device)
        .unwrap_or_else(|error| panic!("expected a normal end, got {error:?}"));

    assert_eq!(
        outcome,
        ContainedTaskOutcome {
            outcome: TaskOutcome::Success,
            final_page: Some(LIST_PAGE.to_owned()),
            executed_steps: 2,
            selected_scheduling_outcome: Some(NOT_FOUND.to_owned()),
            designated_effect_caps: Vec::new(),
        }
    );
    let records = device.records();
    let [(1, record)] = records.as_slice() else {
        panic!("one decision record of step 1, got {}", records.len());
    };
    assert_eq!(record.selection_result, TaskSelectionOutcome::Empty);
    assert_eq!(device.inputs().len(), 1, "{:?}", device.inputs());
    assert_eq!(
        device.finished(),
        [(0, "open", LIST_PAGE), (1, "pick", LIST_PAGE)]
    );
}
