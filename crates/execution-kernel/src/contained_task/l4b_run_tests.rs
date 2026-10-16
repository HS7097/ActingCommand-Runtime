// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #308 slice SEL-L4b in a run (frozen selector model S0 v2.1 §1.3, §1.4, §3.2, §3.4 P4
// and P6, row L4b of §6). A single-frame select step that declares `on_empty` ends the run
// Completed on its own page with its key when nobody listed is present and nothing is unknown,
// mid-phase in a phased route, and is blocked when a card it could not read might be listed. A
// destination's `expect_identity` reads the business ID the select step chose in this run. A
// reading mark on the step's page is read from the frame the run ended on.
//
// The tasks are built in memory and prepared by the kernel's own admission steps: the program is
// validated against the page set, its select steps are prepared with their policy document read
// from a package, and the post-admission and reading target checks run. The device shows one
// 7 x 1 frame at a time: pixel 0 is the page's colour, pixel 1 the select step's guard, pixels 2-3
// and 4-5 the list's two cards (a name code and an open mark) and pixel 6 a count code; on the
// detail page pixel 2 shows the name of the card that was opened. Each input moves the device on
// to its next stage of screens. A scripted OCR provider reads the text of the code in the pixel
// its region starts on. The `expect_identity` timeout case runs on the runtime's default clock,
// so the kernel's own pause waits a few milliseconds there.

use super::*;
use actingcommand_contract::{
    InstanceFactContext, InstanceFactSnapshot, TaskSelectionBlocking, TaskSelectionConfirmation,
    TaskSelectionOutcome, TaskSelectionRecord, TaskSelectionUnknownCheck, TaskSelectionUpperBound,
};
use actingcommand_device::CaptureBackendName;
use actingcommand_pack_containment::{Containment, InstanceId};
use actingcommand_recognition_pack::{
    FsAssetResolver, NnProviderRequest, NnProviderResult, OcrExecutionProviderKind,
    OcrProviderObservation, OcrProviderRequest, OcrProviderResult, OcrProviderTextBlock,
    RecognitionPack, VisionProviderError, VisionProviderErrorCode, load_pack_from_json_str,
};
use serde_json::{Value, json};
use std::io::{Cursor, Write};
use std::path::PathBuf;
use std::time::UNIX_EPOCH;
use zip::{ZipWriter, write::FileOptions};

const WIDTH: u32 = 7;
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PAGES: [(&str, [u8; 3]); 3] = [
    ("home", [255, 255, 0]),
    ("list", [255, 0, 0]),
    ("detail", [0, 0, 255]),
];
const GREEN: [u8; 3] = [0, 255, 0];
const BLACK: [u8; 3] = [0, 0, 0];
// The codes a name or count pixel shows, and the text each one reads as: nothing readable, the
// two members' names and a count.
const UNREAD: u8 = 0;
const ALPHA: u8 = 1;
const BRAVO: u8 = 2;
const COUNT: u8 = 3;
const TEXTS: [&str; 4] = ["", "alpha", "bravo", "42"];
const LIST: &str = "neutral/list";
const DETAIL: &str = "neutral/detail";
// The key the select step's `on_empty` names, mapped to the list page.
const NOBODY_OPEN: &str = "nobody_open";

fn colour(page: &str) -> [u8; 3] {
    PAGES
        .iter()
        .find(|(name, _)| *name == page)
        .map(|(_, colour)| *colour)
        .unwrap_or_else(|| panic!("unknown fixture page {page}"))
}

// Reads the text of the code in the pixel its region starts on: an unreadable code gives no text
// and no confidence, every other code its text at confidence 0.99.
#[derive(Debug)]
struct ShownText;

impl ShownText {
    fn result(request: &OcrProviderRequest<'_>) -> OcrProviderResult {
        let pixel = usize::try_from(request.region.x).expect("a region on the frame") * 3;
        let text = TEXTS[usize::from(request.frame.rgb8_pixels[pixel])];
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
        OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            text: text.to_owned(),
            blocks,
            confidence,
            detection: None,
        }
    }
}

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
        Ok(Self::result(&request))
    }

    // A reading mark's recognition asks for the provider's execution evidence.
    fn read_text_with_execution_evidence(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderObservation, VisionProviderError> {
        Ok(OcrProviderObservation {
            result: Self::result(&request),
            execution: Some(OcrProviderExecutionEvidence {
                invocation_id: "reading-ocr".to_owned(),
                session_id: "reading-session".to_owned(),
                session_generation: 1,
                requested_provider: OcrExecutionProviderKind::Cpu,
                resolved_provider: OcrExecutionProviderKind::Cpu,
                requested_cuda_ordinal: None,
                requested_cuda_identity: None,
                resolved_cuda_ordinal: None,
                resolved_cuda_identity: None,
                provider_implementation: "fixture-ocr".to_owned(),
                provider_binary_sha256: "b".repeat(64),
                runtime_version: "fixture-runtime".to_owned(),
                model_ref: request.model_ref.to_owned(),
                model_sha256: request.model_sha256.to_owned(),
                cpu_ep_registered: true,
                cpu_fallback_disabled: false,
                fallback_forbidden: true,
                fallback_observed: None,
                complete: true,
            }),
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

// One screen: its page, the name code of each card (on the detail page, the first is the name
// shown) and whether each card is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Screen {
    page: &'static str,
    names: [u8; 2],
    open: [bool; 2],
}

const HOME: Screen = Screen {
    page: "home",
    names: [UNREAD, UNREAD],
    open: [false, false],
};

const fn list(names: [u8; 2], open: [bool; 2]) -> Screen {
    Screen {
        page: "list",
        names,
        open,
    }
}

const fn detail(name: u8) -> Screen {
    Screen {
        page: "detail",
        names: [name, UNREAD],
        open: [false, false],
    }
}

fn frame(screen: Screen) -> Frame {
    let lit = |on: bool| if on { GREEN } else { BLACK };
    let code = |shown: u8| [shown, 0, 0];
    Frame::from_pixels(
        WIDTH,
        1,
        [
            colour(screen.page),
            GREEN,
            code(screen.names[0]),
            lit(screen.open[0]),
            code(screen.names[1]),
            lit(screen.open[1]),
            code(COUNT),
        ]
        .concat(),
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
}

// The Runtime as the kernel sees it. Stage `n` is what the device shows after its `n`-th input:
// its screens in order, the last one staying. Captures are stamped one second apart from the
// Unix epoch, so each frame's capture time names it. Every trace and input is kept.
struct Device {
    stages: Vec<VecDeque<Screen>>,
    inputs: Vec<InputAction>,
    captures: u64,
    shown: Vec<(SystemTime, &'static str)>,
    traces: Vec<ContainedTaskTrace>,
}

impl Device {
    fn new(stages: Vec<Vec<Screen>>) -> Self {
        Self {
            stages: stages.into_iter().map(VecDeque::from).collect(),
            inputs: Vec::new(),
            captures: 0,
            shown: Vec::new(),
            traces: Vec::new(),
        }
    }

    // The page of every captured frame, in capture order.
    fn pages(&self) -> Vec<&'static str> {
        self.shown.iter().map(|(_, page)| *page).collect()
    }

    // The step, selection, effect, reading and final traces, in order.
    fn kinds(&self) -> Vec<&'static str> {
        self.traces
            .iter()
            .filter_map(|trace| match trace {
                ContainedTaskTrace::StepStarted { .. } => Some("step_started"),
                ContainedTaskTrace::SelectionEvaluated { .. } => Some("selection_evaluated"),
                ContainedTaskTrace::EffectIntent { .. } => Some("effect_intent"),
                ContainedTaskTrace::EffectCompleted { .. } => Some("effect_completed"),
                ContainedTaskTrace::StepFinished { .. } => Some("step_finished"),
                ContainedTaskTrace::ResourceReadingFrame { .. } => Some("reading_frame"),
                ContainedTaskTrace::ResourceReadings { .. } => Some("readings"),
                ContainedTaskTrace::Finalizing { .. } => Some("finalizing"),
                _ => None,
            })
            .collect()
    }

    fn records(&self) -> Vec<&TaskSelectionRecord> {
        self.traces
            .iter()
            .filter_map(|trace| match trace {
                ContainedTaskTrace::SelectionEvaluated { selection, .. } => {
                    Some(selection.as_ref())
                }
                _ => None,
            })
            .collect()
    }
}

impl ContainedTaskRuntime for Device {
    type Error = &'static str;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        let stage = self.inputs.len().min(self.stages.len().saturating_sub(1));
        let screens = &mut self.stages[stage];
        let screen = if screens.len() > 1 {
            screens.pop_front()
        } else {
            screens.front().copied()
        }
        .ok_or("the fixture shows no screen")?;
        self.captures += 1;
        let mut shown = frame(screen);
        shown.captured_at = UNIX_EPOCH + Duration::from_secs(self.captures);
        self.shown.push((shown.captured_at, screen.page));
        Ok(shown.into())
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

    fn input(
        &mut self,
        action: InputAction,
        _frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        self.inputs.push(action);
        Ok(())
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        self.traces.push(trace);
        Ok(())
    }
}

// The pages' colours on pixel 0, the guard on pixel 1, each card's name (OCR) and open mark, the
// detail page's name (OCR, pixel 2) and the count (OCR, pixel 6). The layout `layout/cards` holds
// the two cards of the list as fixed slots; each card is `open`, and with `identity` it also
// reads the business identity `member` of its name.
fn pack(identity: bool) -> RecognitionPack {
    let colour_target = |id: &str, x: u32, expected: [u8; 3]| {
        json!({"type": "color", "id": id,
               "region": {"x": x, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    let text_target = |id: &str, x: u32| {
        json!({"type": "ocr", "id": id, "region": {"x": x, "y": 0, "width": 1, "height": 1},
               "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact",
               "expected": ["0"], "case_sensitive": false, "minimum_confidence": 0.0,
               "model_ref": "PP-OCRv6_medium", "model_sha256": MODEL_SHA256})
    };
    let mut targets = PAGES
        .iter()
        .map(|(page, colour)| colour_target(&format!("page/{page}"), 0, *colour))
        .collect::<Vec<_>>();
    targets.extend([
        colour_target("guard/ready", 1, GREEN),
        text_target("card/first_name", 2),
        colour_target("card/first_open", 3, GREEN),
        text_target("card/second_name", 4),
        colour_target("card/second_open", 5, GREEN),
        text_target("detail/name", 2),
        text_target("count/list", 6),
    ]);
    let slot = |x: u32, name: &str, open: &str| {
        let mut slot_targets = json!({"open": open});
        if identity {
            slot_targets["member"] = json!(name);
        }
        json!({"rect": {"x": x, "y": 0, "width": 2, "height": 1},
               "click": {"x": x, "y": 0, "width": 2, "height": 1},
               "targets": slot_targets})
    };
    let mut features = vec![json!({"name": "open", "value": "passed"})];
    if identity {
        features.insert(
            0,
            json!({"name": "member", "value": "identity", "identity": {
                "entries": [{"id": "member-a", "aliases": ["alpha"]},
                            {"id": "member-b", "aliases": ["bravo"]}],
                "recognition": {"kind": "ocr_aliases", "max_distance": 1, "minimum_margin": 1,
                                "minimum_confidence_milli": 800}}}),
        );
    }
    load_pack_from_json_str(
        &json!({
            "schema_version": "0.7",
            "game": "neutral",
            "server": "test",
            "coordinate_space": {"width": WIDTH, "height": 1},
            "defaults": {"color_max_distance": 0.0},
            "targets": targets,
            "candidate_layouts": [{
                "id": "layout/cards", "page_id": LIST, "kind": "fixed_slots",
                "features": features,
                "slots": [slot(2, "card/first_name", "card/first_open"),
                          slot(4, "card/second_name", "card/second_open")]
            }]
        })
        .to_string(),
    )
    .expect("recognition pack")
}

// The select step's policy document: the open cards are the candidates, nobody is allowed and
// the lowest candidate wins; with `identity`, it also reads the card's business identity.
fn policy(identity: bool) -> Value {
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
            "candidate_layout_id": "layout/cards",
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
        "selection": {"mode": "none_allowed", "required_count": 1},
        "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
    })
}

// A package that holds the task's policy document at its task-local path.
fn policy_package(identity: bool) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, bytes) in [
        (
            "control.json",
            br#"{"game":"neutral","server":"test","entry_task_id":"task"}"#.to_vec(),
        ),
        (
            "resources/manifest.json",
            br#"{"entry_task_id":"task"}"#.to_vec(),
        ),
        (
            "resources/operations/task/task.json",
            br#"{"task_id":"task"}"#.to_vec(),
        ),
        (
            "resources/operations/task/policies/select.json",
            serde_json::to_vec(&policy(identity)).expect("policy document"),
        ),
    ] {
        zip.start_file(path, options).expect("zip entry");
        zip.write_all(&bytes).expect("zip bytes");
    }
    zip.finish().expect("finish zip").into_inner()
}

fn control(phases: Option<Value>) -> Value {
    let mut control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.select.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": WIDTH, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 5000,
        "timeout_ms": 60000
    });
    if let Some(phases) = phases {
        control["phases"] = phases;
    }
    control
}

// A page-graph task to the detail page through `operations`.
fn task_program(operations: Vec<Value>) -> Value {
    json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": WIDTH, "height": 1},
        "target_page": "detail",
        "operations": operations
    })
}

// The select step `pick`: from the list, guarded, it taps the chosen card into the detail page
// (`expect_after`); with `on_empty`, nobody open ends the run with `NOBODY_OPEN`.
fn pick(on_empty: bool, expect_after: Value) -> Value {
    let mut select = json!({"layout_id": "layout/cards",
                            "policy": {"path": "policies/select.json"}});
    if on_empty {
        select["on_empty"] = json!({"outcome_key": NOBODY_OPEN});
    }
    json!({
        "id": "pick",
        "from": "list",
        "guard": {"page_id": "list", "target_id": "guard/ready",
                  "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                  "color_probe": "guard/ready"},
        "select": select,
        "expect_after": expect_after
    })
}

// A tap `id` from the home page, its destination declared by `field` (`to` or `expect_after`).
fn tap_from_home(id: &str, field: &str, destination: Value) -> Value {
    let mut operation = json!({
        "id": id,
        "from": "home",
        "click": {"kind": "point", "x": 0, "y": 0},
        "unguarded_trusted_coordinate": true,
        "retryable": false
    });
    operation[field] = destination;
    operation
}

fn to_detail() -> Value {
    json!({"page_id": "detail"})
}

// The detail page, whose name must read the business ID `pick` chose, polled every millisecond
// for up to `timeout_ms`.
fn expecting_pick(timeout_ms: u64) -> Value {
    json!({"page_id": "detail", "timeout_ms": timeout_ms, "interval_ms": 1,
           "expect_identity": {"target_id": "detail/name", "choice_of": "pick"}})
}

// Ending on the list (nobody open, the `on_empty` key) or on the detail page; no designated
// effect.
fn scheduling_outcome() -> Value {
    json!({"mappings": [
        {"outcome_key": NOBODY_OPEN, "effect": "no_designated_effect", "terminal_pages": ["list"]},
        {"outcome_key": "card_opened", "effect": "no_designated_effect",
         "terminal_pages": ["detail"]}
    ]})
}

// The task whose one step `pick` declares `on_empty`.
fn on_empty_program() -> Value {
    let mut program = task_program(vec![pick(true, to_detail())]);
    program["scheduling_outcome"] = scheduling_outcome();
    program
}

// Two stages: `arrive` taps from the home page into the list, `choose` picks a card.
fn phases() -> Value {
    json!([{"id": "arrive", "operations": ["open_list"], "target_pages": ["list"]},
           {"id": "choose", "operations": ["pick"], "target_pages": ["detail"]}])
}

// The task as contained task admission prepares it (`prepare_bundle`): the control is parsed, the
// page set is checked against the pack, the program is validated, its select steps are prepared
// with their policy document read from a package, and the post-admission and reading target
// checks run.
fn admit(control: Value, program: Value, identity: bool) -> PreparedContainedTask {
    let control = parse_task_control_declaration(control).expect("a valid control");
    let mut program: TaskProgram = serde_json::from_value(program).expect("a task program");
    let evaluator = RecognitionEvaluator::with_vision_provider(
        pack(identity),
        Arc::new(FsAssetResolver::new(PathBuf::new())),
        Arc::new(ShownText),
    )
    .expect("recognition evaluator");
    let pages = PAGES
        .iter()
        .map(|(page, _)| {
            json!({"id": format!("neutral/{page}"), "required": [format!("page/{page}")]})
        })
        .collect::<Vec<_>>();
    let detector = PageDetector::new(
        serde_json::from_value(json!({"schema_version": "0.3", "pages": pages})).expect("page set"),
    )
    .expect("page detector");
    detector
        .validate(&evaluator)
        .expect("the pages' targets are in the pack");
    evaluator
        .pack()
        .validate_candidate_layout_pages(|page_id| detector.contains_page(page_id))
        .expect("the layout's page is in the page set");
    let mut containment = Containment::new();
    let bundle = containment
        .load(
            &InstanceId::new("neutral-select").expect("instance"),
            &policy_package(identity),
        )
        .expect("the policy document's package");
    program
        .validate(&control, bundle, &detector)
        .expect("the program is admitted");
    selection::prepare_select_steps(&mut program, &control, bundle, &evaluator)
        .expect("the select steps are admitted");
    program
        .validate_on_empty_post_admission(&control, false, None)
        .expect("no post-admission OCR supplies a key");
    program
        .validate_resource_reading_targets(bundle, &detector, &evaluator)
        .expect("the reading targets are OCR targets");
    let entry_page = program
        .required_entry(&control, &detector)
        .expect("the entry page");
    let scheduling_outcome = program.scheduling_outcome.clone();
    PreparedContainedTask {
        control,
        program,
        evaluator,
        detector,
        entry_page,
        scheduling_outcome,
        post_admission_ocr: None,
        post_admission_fields: None,
        entry_count: 1,
        task_count: 1,
    }
}

fn succeeded(
    result: Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>,
) -> ContainedTaskOutcome {
    result.unwrap_or_else(|error| panic!("expected the run to end normally, got {error:?}"))
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

fn value(error: &ContainedTaskError, key: &str) -> Option<Value> {
    error.outcome().value(key).cloned()
}

fn finished(
    step_index: u32,
    operation: &str,
    page: &str,
    phase: Option<TaskPhaseEvidence>,
) -> ContainedTaskTrace {
    ContainedTaskTrace::StepFinished {
        step_index,
        operation_label: operation.to_owned(),
        page_label: page.to_owned(),
        phase,
    }
}

// The run's normal end on `page`, after `executed_steps`, with the scheduling outcome `key`.
fn completed(page: &str, executed_steps: u32, key: Option<&str>) -> ContainedTaskOutcome {
    ContainedTaskOutcome {
        outcome: TaskOutcome::Success,
        final_page: Some(page.to_owned()),
        executed_steps,
        selected_scheduling_outcome: key.map(str::to_owned),
        designated_effect_caps: Vec::new(),
    }
}

// P4 (S0 §3.2 rule 1, §1.4): both cards are closed, so the policy (`none_allowed`) admits nobody
// and decides `empty`, with nothing unknown on the step's one frame. The step that declares
// `on_empty` ends the run Completed on its own page with its key: its record holds the decision,
// `unknown_check` with no unknown instance and no confirmation; its `StepFinished` names the list,
// and no input, effect intent or completed effect follows.
#[test]
fn l4b_run_an_on_empty_end_completes_the_run_on_the_step_page_with_its_key() {
    let task = admit(control(None), on_empty_program(), false);
    let mut device = Device::new(vec![vec![list([ALPHA, BRAVO], [false, false])]]);

    let outcome = succeeded(task.run(&mut device));

    assert_eq!(outcome, completed(LIST, 1, Some(NOBODY_OPEN)));
    assert_eq!(
        device.kinds(),
        [
            "step_started",
            "selection_evaluated",
            "step_finished",
            "finalizing"
        ]
    );
    let records = device.records();
    let [record] = records.as_slice() else {
        panic!("one decision record: {:?}", device.kinds());
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
    assert!(
        device.traces.contains(&finished(0, "pick", LIST, None)),
        "{:?}",
        device.traces
    );
    assert!(device.traces.contains(&ContainedTaskTrace::Finalizing {
        outcome: TaskOutcome::Success
    }));
    assert!(device.inputs.is_empty(), "{:?}", device.inputs);
}

// S0 §1.4 (run) for a phased route: the first stage taps from the home page into the list and
// completes; the select step of the second stage ends empty. Its `StepFinished` names the list
// with its stage not completed, and the run is Completed there, mid-stage, with the `on_empty`
// key and both steps counted; the only input is the first stage's tap.
#[test]
fn l4b_run_an_on_empty_end_completes_a_phased_route_mid_stage() {
    let mut program = task_program(vec![
        tap_from_home("open_list", "to", json!("list")),
        pick(true, to_detail()),
    ]);
    program["entry_page"] = json!("home");
    program["phases"] = phases();
    program["scheduling_outcome"] = scheduling_outcome();
    let task = admit(control(Some(phases())), program, false);
    let mut device = Device::new(vec![vec![HOME], vec![list([ALPHA, BRAVO], [false, false])]]);

    let outcome = succeeded(task.run(&mut device));

    assert_eq!(outcome, completed(LIST, 2, Some(NOBODY_OPEN)));
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
    let stage = |index: u32, id: &str, done: bool| TaskPhaseEvidence {
        index,
        id: id.to_owned(),
        completed: done,
    };
    assert!(
        device.traces.contains(&finished(
            0,
            "open_list",
            LIST,
            Some(stage(0, "arrive", true))
        )),
        "{:?}",
        device.traces
    );
    assert!(
        device
            .traces
            .contains(&finished(1, "pick", LIST, Some(stage(1, "choose", false)))),
        "{:?}",
        device.traces
    );
    assert_eq!(device.inputs.len(), 1, "{:?}", device.inputs);
}

// P6 (S0 §3.2 rule 2, which also applies to a single-frame select that declares `on_empty`): the
// first card reads `member-a` and is closed; the second card is open but its name reads no text,
// so its identity is Unknown and it is an unknown instance. The `empty` decision does not stand:
// the step fails `selection_result_blocked` with the decision's kind and key, `unknown_instances`
// and its count, the record keeps `empty` with its `unknown_check`, and nothing is tapped.
#[test]
fn l4b_run_an_unreadable_card_blocks_the_empty_end() {
    let task = admit(control(None), on_empty_program(), true);
    let mut device = Device::new(vec![vec![list([ALPHA, UNREAD], [false, true])]]);

    let error = failed(task.run(&mut device));

    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::SelectionResultBlocked.as_str()
    );
    assert_eq!(value(&error, "stage"), Some(json!("select_decide")));
    assert_eq!(value(&error, "selection_kind"), Some(json!("empty")));
    assert_eq!(value(&error, "outcome_key"), Some(json!("card_empty")));
    assert_eq!(
        value(&error, "selection_blocking"),
        Some(json!("unknown_instances"))
    );
    assert_eq!(value(&error, "count"), Some(json!(1)));
    let records = device.records();
    let [record] = records.as_slice() else {
        panic!("one decision record: {:?}", device.kinds());
    };
    assert_eq!(record.selection_result, TaskSelectionOutcome::Empty);
    let check = record.unknown_check.expect("unknown_check");
    assert_eq!(check.unknown_instances, 1);
    assert_eq!(check.blocking, TaskSelectionBlocking::UnknownInstances);
    assert_ne!(check.upper_bound, TaskSelectionUpperBound::NoneUnknown);
    assert_eq!(record.confirmation, TaskSelectionConfirmation::NotAttempted);
    assert!(device.inputs.is_empty(), "{:?}", device.inputs);
    assert!(
        !device
            .kinds()
            .iter()
            .any(|kind| ["effect_intent", "effect_completed"].contains(kind)),
        "{:?}",
        device.kinds()
    );
}

// The list of the `expect_identity` tasks: the first card, `member-a`, is the open one.
fn member_a_open() -> Screen {
    list([ALPHA, BRAVO], [true, false])
}

// S0 §1.3 `expect_identity`, late render: the select step chooses `member-a` and taps its card.
// The detail page's first frame shows no name yet (Unknown); the next poll reads `member-a`, so
// the step finishes there and the run succeeds, with the one tap inside the first card.
#[test]
fn l4b_run_expect_identity_passes_once_the_chosen_name_renders() {
    let task = admit(
        control(None),
        task_program(vec![pick(false, expecting_pick(5_000))]),
        true,
    );
    let mut device = Device::new(vec![
        vec![member_a_open()],
        vec![detail(UNREAD), detail(ALPHA)],
    ]);

    let outcome = succeeded(task.run(&mut device));

    assert_eq!(outcome, completed(DETAIL, 1, None));
    assert_eq!(device.pages(), ["list", "list", "detail", "detail"]);
    assert!(
        matches!(
            device.inputs.as_slice(),
            [InputAction::Tap { x: 2..=3, y: 0 }]
        ),
        "{:?}",
        device.inputs
    );
    assert_eq!(
        device.kinds(),
        [
            "step_started",
            "selection_evaluated",
            "effect_intent",
            "effect_completed",
            "step_finished",
            "finalizing"
        ]
    );
    assert!(
        device.traces.contains(&finished(0, "pick", DETAIL, None)),
        "{:?}",
        device.traces
    );
}

// S0 §1.3 `expect_identity`, mismatch: the detail page reads another Known identity, `member-b`.
// The step fails at once, on the first destination frame, with `selection_identity_mismatch`
// (`different`, the chosen and the observed ID) after its `StepFinished`; there is one input and
// no retry.
#[test]
fn l4b_run_expect_identity_fails_at_once_on_another_identity() {
    let task = admit(
        control(None),
        task_program(vec![pick(false, expecting_pick(5_000))]),
        true,
    );
    let mut device = Device::new(vec![vec![member_a_open()], vec![detail(BRAVO)]]);

    let error = failed(task.run(&mut device));

    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::SelectionIdentityMismatch.as_str()
    );
    assert_eq!(
        value(&error, "stage"),
        Some(json!("select_expect_identity"))
    );
    assert_eq!(value(&error, "identity_state"), Some(json!("different")));
    assert_eq!(value(&error, "expected_identity"), Some(json!("member-a")));
    assert_eq!(value(&error, "observed_identity"), Some(json!("member-b")));
    assert_eq!(device.pages(), ["list", "list", "detail"]);
    assert_eq!(device.inputs.len(), 1, "{:?}", device.inputs);
    assert_eq!(
        device.kinds(),
        [
            "step_started",
            "selection_evaluated",
            "effect_intent",
            "effect_completed",
            "step_finished"
        ]
    );
    assert!(
        device.traces.contains(&finished(0, "pick", DETAIL, None)),
        "{:?}",
        device.traces
    );
}

// S0 §1.3 `expect_identity`, timeout: the detail page's name never reads (Unknown on every poll
// at 1 ms) until `expect_after.timeout_ms` (5 ms). The step fails `selection_identity_mismatch`
// (`unknown`, the chosen ID, nothing observed) after its `StepFinished`, with its one input.
#[test]
fn l4b_run_expect_identity_fails_after_its_timeout_with_unknown_readings() {
    let task = admit(
        control(None),
        task_program(vec![pick(false, expecting_pick(5))]),
        true,
    );
    let mut device = Device::new(vec![vec![member_a_open()], vec![detail(UNREAD)]]);

    let error = failed(task.run(&mut device));

    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::SelectionIdentityMismatch.as_str()
    );
    assert_eq!(
        value(&error, "stage"),
        Some(json!("select_expect_identity"))
    );
    assert_eq!(value(&error, "identity_state"), Some(json!("unknown")));
    assert_eq!(value(&error, "expected_identity"), Some(json!("member-a")));
    assert_eq!(value(&error, "observed_identity"), None);
    assert_eq!(device.inputs.len(), 1, "{:?}", device.inputs);
    assert_eq!(
        device.kinds(),
        [
            "step_started",
            "selection_evaluated",
            "effect_intent",
            "effect_completed",
            "step_finished"
        ]
    );
    assert!(
        device.traces.contains(&finished(0, "pick", DETAIL, None)),
        "{:?}",
        device.traces
    );
}

// S0 §1.3 `expect_identity`, no choice: the run starts on the home page, so the select step
// `pick` of the list never runs on this route; the later tap `open` from the home page into the
// detail page expects `pick`'s choice. Although the detail page shows a Known name, the step
// fails `selection_identity_mismatch` with `no_choice` and no expected or observed ID, after its
// `StepFinished`, with its one input.
#[test]
fn l4b_run_expect_identity_of_a_step_that_did_not_choose_fails_no_choice() {
    let task = admit(
        control(None),
        task_program(vec![
            pick(false, to_detail()),
            tap_from_home("open", "expect_after", expecting_pick(5_000)),
        ]),
        true,
    );
    let mut device = Device::new(vec![vec![HOME], vec![detail(ALPHA)]]);

    let error = failed(task.run(&mut device));

    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::SelectionIdentityMismatch.as_str()
    );
    assert_eq!(
        value(&error, "stage"),
        Some(json!("select_expect_identity"))
    );
    assert_eq!(value(&error, "identity_state"), Some(json!("no_choice")));
    assert_eq!(value(&error, "expected_identity"), None);
    assert_eq!(value(&error, "observed_identity"), None);
    assert_eq!(device.inputs.len(), 1, "{:?}", device.inputs);
    assert_eq!(
        device.kinds(),
        [
            "step_started",
            "effect_intent",
            "effect_completed",
            "step_finished"
        ]
    );
    assert!(
        device.traces.contains(&finished(0, "open", DETAIL, None)),
        "{:?}",
        device.traces
    );
}

// S0 §1.4 and row L4b ("an `on_empty` end with resource readings on `from` reads them from the
// last viewport"), with Workflow #384 D2-S2's hand-off: a reading mark on the select step's page.
// The run reaches the list on its one captured frame, hands that frame to recognition, and the
// select step on it ends empty; the frame's reading (the count, 42) is joined before `Finalizing`
// and the run ends with the `on_empty` key.
#[test]
fn l4b_run_an_on_empty_end_publishes_the_reading_of_its_page() {
    let mut program = on_empty_program();
    program["resource_readings"] = json!([{
        "id": "count", "fact_key": "resource.count", "page_id": "list",
        "target_id": "count/list", "trim": "whitespace_v1",
        "value": {"type": "unsigned_integer", "min": 0, "max": 100000},
        "minimum_confidence_milli": 900, "valid_for_ms": 60000
    }]);
    let task = admit(control(None), program, false);
    let mut device = Device::new(vec![vec![list([ALPHA, BRAVO], [false, false])]]);

    let outcome = succeeded(task.run(&mut device));

    assert_eq!(outcome, completed(LIST, 1, Some(NOBODY_OPEN)));
    assert_eq!(
        device.kinds(),
        [
            "reading_frame",
            "step_started",
            "selection_evaluated",
            "step_finished",
            "readings",
            "finalizing"
        ]
    );
    // The step decided on the run's one captured frame, which is the frame handed off.
    assert_eq!(device.pages(), ["list"]);
    let last = device.shown.last().expect("a captured frame").0;
    assert!(
        device
            .traces
            .contains(&ContainedTaskTrace::ResourceReadingFrame { captured_at: last }),
        "{:?}",
        device.traces
    );
    let joined = device
        .traces
        .iter()
        .filter_map(|trace| match trace {
            ContainedTaskTrace::ResourceReadings {
                captured_at,
                readings,
                unresolved,
            } => Some((*captured_at, readings, unresolved.len())),
            _ => None,
        })
        .collect::<Vec<_>>();
    let [(captured_at, readings, misses)] = joined.as_slice() else {
        panic!("one joined frame: {:?}", device.traces);
    };
    assert_eq!(*captured_at, last);
    assert_eq!(*misses, 0);
    assert_eq!(
        readings
            .iter()
            .map(|reading| (
                reading.fact_key.as_str(),
                reading.value,
                reading.confidence_milli
            ))
            .collect::<Vec<_>>(),
        [("resource.count", 42, 990)]
    );
}
