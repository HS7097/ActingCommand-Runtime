// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #448 SBUD-1 (register §1.333; contracts/business-identity-consensus.md, the select
// step's budget): a candidate projection whose layout has consensus, or a feature that reads an
// identity or an integer, checks in a run that what is left of its step's deadline holds the
// layout's declared provider time and waits; admission compares the same need, taken from the
// same function (`selection::candidate_provider_need_ms`), with the step's limit, for a
// single-frame select, a select over a scan (its `max_ms`) and a list reading (its scan's
// `max_ms`), and refuses a larger need with `recognition_sample_budget_invalid`, its
// `required_ms`, `limit_ms` and `pack_operation_id`. A need equal to the limit is admitted. A list
// reading in a pack with target consensus has its page recognition compared with the same limit.
//
// Every package is a synthetic `neutral` package in a 400 x 600 frame: pixel (0, 0) names the page,
// pixel (3, 0) is the guard on the list. The count layout holds four slots, each count read by its
// own OCR target of 2 000 ms and no consensus: its need is 8 000 ms, over the 5 000 ms a package
// without a step timeout gets. The card layout is the list of the SEL-L4b and S168-4 gates: an
// anchor template, a name read by OCR (2 000 ms) and an open mark, 64 instances at most; one
// projection evaluates 16 of them within its 16 provider evaluations, so its need is 32 000 ms.
// The run's check runs on the scripted clock (EXEC-I3), which moves only where a script says: a
// fresh step's window is whole when its projection starts.

use super::tests::scripted::{ScriptedFrame, ScriptedRuntime};
use super::*;
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition_pack::{
    NnProviderRequest, NnProviderResult, OcrProviderRequest, OcrProviderResult,
    OcrProviderTextBlock, VisionProviderError, VisionProviderErrorCode,
};
use serde_json::{Value, json};
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

const WIDTH: u32 = 400;
const HEIGHT: u32 = 600;
const TASK_TIMEOUT_MS: u64 = 300_000;
const POLICY_ENTRY: &str = "resources/operations/task/policies/select.json";
const LIST: &str = "neutral/list";
const MEMBERS: [&str; 3] = ["member.a", "member.b", "member.c"];
const LIST_COLOUR: [u8; 3] = [255, 0, 0];
const DETAIL_COLOUR: [u8; 3] = [0, 0, 255];
const GREEN: [u8; 3] = [0, 255, 0];
const BACKGROUND: [u8; 3] = [30, 31, 32];
// The count layout: four slots, each read by an OCR target of 2 000 ms.
const SLOTS: u32 = 4;
const COUNT_TIMEOUT_MS: u64 = 2_000;
const COUNT_NEED_MS: u64 = 8_000;
// The step timeout of a package that declares none.
const DEFAULT_STEP_MS: u64 = 5_000;
// The card layout: 16 instances of one 2 000 ms name read.
const NAME_TIMEOUT_MS: u64 = 2_000;
const CARD_NEED_MS: u64 = 32_000;
// The page consensus of the list reading's second case: three frames 1 000 ms apart, so one page
// recognition waits 2 000 ms (its pages are told by colour, no provider time).
const PAGE_NEED_MS: u64 = 2_000;

fn color(id: &str, x: u32, expected: [u8; 3]) -> Value {
    json!({"type": "color", "id": id, "region": {"x": x, "y": 0, "width": 1, "height": 1},
           "expected": expected})
}

fn ocr(id: &str, region: Value, timeout_ms: u64) -> Value {
    json!({"type": "ocr", "id": id, "region": region, "languages": ["en"],
           "timeout_ms": timeout_ms, "match_mode": "exact", "expected": ["7"],
           "case_sensitive": false, "minimum_confidence": 0.8,
           "model_ref": "PP-OCRv6_medium", "model_sha256": "a".repeat(64)})
}

// The count layout: slot k at x 20 + 90 k, its count read by `count/<k>` inside it.
fn count_layout() -> Value {
    let slots = (0..SLOTS)
        .map(|slot| {
            let left = 20 + 90 * slot;
            json!({"rect": {"x": left, "y": 100, "width": 80, "height": 60},
                   "click": {"x": left + 10, "y": 120, "width": 40, "height": 30},
                   "targets": {"count": format!("count/{slot}")}})
        })
        .collect::<Vec<_>>();
    json!({
        "id": "layout/counts", "page_id": LIST, "kind": "fixed_slots",
        "features": [{"name": "count", "value": "ocr_integer",
                      "integer": {"min": 0, "max": 999, "minimum_confidence_milli": 800}}],
        "slots": slots
    })
}

// The card list: an anchor template, the member's name read 50 px below it, an open mark.
fn card_layout() -> Value {
    let entries = MEMBERS.map(|id| json!({"id": id, "aliases": [id.replace('.', " ")]}));
    json!({
        "id": "layout/cards", "page_id": LIST, "kind": "repeated_anchor",
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

// The scan of the SEL-L4b and S168-4 gates, within `max_ms`.
fn scan(max_ms: u64) -> Value {
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
        "bounds": {"max_viewports": 32, "max_reposition_viewports": 8, "max_ms": max_ms}
    })
}

fn guard() -> Value {
    json!({"page_id": "list", "target_id": "guard/list",
           "expected_rect": {"x": 3, "y": 0, "width": 1, "height": 1},
           "color_probe": "guard/list"})
}

// The select step `pick`: guarded, from the list over `select`, a tap into the detail page.
fn pick(select: Value) -> Value {
    json!({"id": "pick", "from": "list", "to": "detail",
           "expect_after": {"page_id": "detail", "timeout_ms": 5000, "interval_ms": 200},
           "guard": guard(), "select": select})
}

// The list reading `read_members` over the card list within `max_ms`, then the tap `leave`.
fn list_operations(max_ms: u64) -> Vec<Value> {
    vec![
        json!({"id": "read_members", "from": "list", "to": "list", "guard": guard(),
               "read_list": {"layout_id": "layout/cards", "key_feature": "member",
                             "key_column": "id",
                             "columns": [{"column": "open", "feature": "open"}],
                             "fact_key": "inventory.list.example_kind_a",
                             "valid_for_ms": 86_400_000, "scan": scan(max_ms)}}),
        json!({"id": "leave", "from": "list", "to": "detail",
               "click": {"kind": "point", "x": 200, "y": 300},
               "unguarded_trusted_coordinate": true, "retryable": false}),
    ]
}

// A page-graph task (schema 0.9) from the list to the detail page, the terminal its one mapping
// names.
fn task(layout: Value, operations: Vec<Value>) -> Value {
    json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "target_page": "detail",
        "scheduling_outcome": {"mappings": [
            {"outcome_key": "item_selected", "effect": "no_designated_effect",
             "terminal_pages": ["detail"]}
        ]},
        "candidate_layouts": [layout],
        "operations": operations
    })
}

fn outcome_keys() -> Value {
    json!({"selected": "item_selected", "empty": "item_empty", "insufficient": "item_insufficient",
           "ambiguous": "item_ambiguous", "unknown": "item_unknown"})
}

// Every slot with a count of at least 1 is a candidate.
fn count_policy() -> Value {
    json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "counts",
        "applies_to": {"candidate_layout_id": "layout/counts", "outcome_keys": outcome_keys()},
        "fields": [{"name": "count", "value_type": {"type": "integer"}}],
        "facts": [],
        "gates": [{"gate_id": "counted",
                   "predicate": {"kind": "integer_at_least",
                                 "value": {"source": "field", "field": "count"},
                                 "threshold": 1},
                   "on_unknown": {"kind": "drop_candidate"}}],
        "scoring": [],
        "selection": {"mode": "none_allowed", "required_count": 1},
        "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
    })
}

// Every open card is a member, the lowest candidate ID last.
fn card_policy() -> Value {
    json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "cards",
        "applies_to": {"candidate_layout_id": "layout/cards", "outcome_keys": outcome_keys()},
        "fields": [
            {"name": "member", "value_type": {"type": "enum_string", "allowed": MEMBERS}},
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

// A textured template image.
fn template_png() -> Vec<u8> {
    let pixels = (0..16_u32 * 12)
        .flat_map(|index| {
            let [red, green, blue, _] = index.wrapping_mul(2_654_435_761).to_le_bytes();
            [red, green, blue]
        })
        .collect::<Vec<u8>>();
    Frame::from_pixels(
        16,
        12,
        pixels,
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("template frame")
    .png_for_artifact()
    .expect("template PNG")
}

// What one package declares beyond its task: its step timeout (none: the default), its select
// step's policy, and whether its list page is told on three frames 1 000 ms apart.
struct Declared {
    step_timeout_ms: Option<u64>,
    policy: Option<Value>,
    page_consensus: bool,
}

// The package of `task`: the recognition pack holds the task's candidate layouts and every target
// the layouts, the guard and the scan checks read.
fn package(task: &Value, declared: Declared) -> Vec<u8> {
    let mut control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.budget.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": WIDTH, "height": HEIGHT},
        "entry_task_id": "task",
        "capture_interval_ms": 100,
        "timeout_ms": TASK_TIMEOUT_MS
    });
    if let Some(step_timeout_ms) = declared.step_timeout_ms {
        control["step_timeout_ms"] = json!(step_timeout_ms);
    }
    let mut targets = vec![
        color("page/list", 0, LIST_COLOUR),
        color("page/detail", 0, DETAIL_COLOUR),
        color("check/list_start", 1, GREEN),
        color("check/list_end", 2, DETAIL_COLOUR),
        color("guard/list", 3, GREEN),
        json!({"type": "template", "id": "ui/card", "template_path": "assets/card.png",
               "region": "full_frame", "threshold": 0.9}),
        ocr(
            "card/name",
            json!({"x": 0, "y": 0, "width": 100, "height": 20}),
            NAME_TIMEOUT_MS,
        ),
        json!({"type": "color", "id": "card/open",
               "region": {"x": 0, "y": 0, "width": 20, "height": 20}, "expected": GREEN}),
    ];
    for slot in 0..SLOTS {
        targets.push(ocr(
            &format!("count/{slot}"),
            json!({"x": 30 + 90 * slot, "y": 110, "width": 40, "height": 20}),
            COUNT_TIMEOUT_MS,
        ));
    }
    let mut pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "defaults": {"match_metric": "ccoeff_normed", "color_max_distance": 0.0},
        "targets": targets,
        "candidate_layouts": task["candidate_layouts"].clone()
    });
    if declared.page_consensus {
        pack["target_consensus"] = json!({
            "page/list": {"samples": [{"frame": 0}, {"frame": 1}, {"frame": 2}], "k": 3,
                          "sample_interval_ms": 1000}
        });
    }
    let pages = ["list", "detail"].map(|page| {
        json!({"id": format!("neutral/{page}"), "required": [format!("page/{page}")],
               "optional": [], "forbidden": []})
    });
    let bytes = |value: &Value| serde_json::to_vec(value).expect("JSON bytes");
    let mut entries = vec![
        ("control.json", bytes(&control)),
        (
            "resources/manifest.json",
            bytes(&json!({"schema_version": "0.3", "entry_task_id": "task"})),
        ),
        ("resources/operations/task/task.json", bytes(task)),
        ("resources/assets/card.png", template_png()),
        ("resources/recognition/neutral.test.pack.json", bytes(&pack)),
        (
            "resources/recognition/neutral.test.pages.json",
            bytes(&json!({"schema_version": "0.3", "pages": pages})),
        ),
    ];
    if let Some(policy) = &declared.policy {
        entries.push((POLICY_ENTRY, bytes(policy)));
    }
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, content) in entries {
        zip.start_file(path, options).expect("zip entry");
        zip.write_all(&content).expect("zip bytes");
    }
    zip.finish().expect("finish zip").into_inner()
}

// The package whose one step `pick` is a single-frame select over the count layout, with the
// step timeout `step_timeout_ms` (none: the default).
fn count_package(step_timeout_ms: Option<u64>) -> Vec<u8> {
    let select = json!({"layout_id": "layout/counts", "policy": {"path": "policies/select.json"}});
    package(
        &task(count_layout(), vec![pick(select)]),
        Declared {
            step_timeout_ms,
            policy: Some(count_policy()),
            page_consensus: false,
        },
    )
}

// The package whose one step `pick` is a select over the card list's scan within `max_ms`; its
// step timeout, 10 000 ms, does not apply to that step.
fn scan_package(max_ms: u64) -> Vec<u8> {
    let select = json!({"layout_id": "layout/cards", "policy": {"path": "policies/select.json"},
                        "scan": scan(max_ms)});
    package(
        &task(card_layout(), vec![pick(select)]),
        Declared {
            step_timeout_ms: Some(10_000),
            policy: Some(card_policy()),
            page_consensus: false,
        },
    )
}

// The package that reads the card list within `max_ms` and leaves to the detail page; with
// `page_consensus`, its list page is told on three frames.
fn list_package(max_ms: u64, page_consensus: bool) -> Vec<u8> {
    package(
        &task(card_layout(), list_operations(max_ms)),
        Declared {
            step_timeout_ms: Some(10_000),
            policy: None,
            page_consensus,
        },
    )
}

fn admitted(case: &str, bytes: &[u8]) -> PreparedContainedTask {
    PreparedContainedTask::load_with_vision_provider("neutral.instance", bytes, Arc::new(Sevens))
        .unwrap_or_else(|error| panic!("{case}: refused: {error}"))
}

fn refusal(case: &str, bytes: &[u8]) -> ContainedTaskError {
    match PreparedContainedTask::load("neutral.instance", bytes) {
        Ok(_) => panic!("{case}: admitted"),
        Err(error) => error,
    }
}

fn values(error: &ContainedTaskError) -> Value {
    Value::Object(error.outcome().values().clone())
}

// The refusal is `recognition_sample_budget_invalid` with the need, the limit and the operation.
#[track_caller]
fn assert_budget_invalid(
    case: &str,
    error: &ContainedTaskError,
    (required_ms, limit_ms): (u64, u64),
    operation: &str,
) {
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::RecognitionSampleBudgetInvalid.as_str(),
        "{case}: {error}"
    );
    assert_eq!(
        values(error),
        json!({"required_ms": required_ms, "limit_ms": limit_ms, "pack_operation_id": operation}),
        "{case}: {error}"
    );
}

// Reads "7" at confidence 0.99 wherever it is asked: every count reads 7.
#[derive(Debug)]
struct Sevens;

impl VisionProvider for Sevens {
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
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            text: "7".to_owned(),
            blocks: vec![OcrProviderTextBlock {
                text: "7".to_owned(),
                rect: request.region,
                confidence: Some(0.99),
            }],
            confidence: Some(0.99),
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

// The list page with its guard.
fn list_frame() -> ScriptedFrame {
    let mut pixels = vec![BACKGROUND; usize::try_from(WIDTH * HEIGHT).expect("frame size")];
    pixels[0] = LIST_COLOUR;
    pixels[3] = GREEN;
    ScriptedFrame::rgb(WIDTH, HEIGHT, &pixels)
}

// The run's check at the start of a fresh step of `window_ms`: the step's frame captured, then
// `pick`'s projection over the count layout with the step's deadline, as `select_attempt` takes it.
// The scripted clock does not move, so the whole window is left when the projection starts.
fn fresh_step(
    task: &PreparedContainedTask,
    window_ms: u64,
) -> Result<(), ContainedTaskRunError<&'static str>> {
    let mut runtime = ScriptedRuntime::new(vec![vec![list_frame()]]);
    let started = runtime.monotonic_now();
    let timing = ContainedTaskTimingContext::new(
        started,
        started + Duration::from_millis(window_ms),
        actingcommand_contract::TaskTimingBudgetOrigin::Task,
    );
    let observed = task
        .capture_frame(&mut runtime, None, None, timing, None, None)?
        .expect("the step's frame shows the list page");
    let operation = &task.program.operations[0];
    let layout = task
        .evaluator
        .candidate_layout("layout/counts")
        .expect("the count layout");
    task.project_transaction(
        &mut runtime,
        operation,
        &observed,
        layout,
        timing,
        timing.deadline(),
    )
    .map(|_| ())
}

// SBUD-1 test 1 (the production case): a single-frame select over a layout whose four slots each
// read an integer by OCR, without consensus, needs 8 000 ms. Declaring no step timeout (5 000 ms)
// or one of 7 999 ms, the package is refused at load with the need, the limit and the operation;
// declaring 8 000 ms it is admitted, and the run's check at the start of a fresh step of 8 000 ms
// does not refuse it: the projection goes on and reads the counts.
#[test]
fn admission_refuses_integer_layout_over_step_deadline() {
    for (case, step_timeout_ms, limit_ms) in [
        ("no step timeout", None, DEFAULT_STEP_MS),
        (
            "a step timeout 1 ms short",
            Some(COUNT_NEED_MS - 1),
            COUNT_NEED_MS - 1,
        ),
    ] {
        let error = refusal(case, &count_package(step_timeout_ms));
        assert_budget_invalid(case, &error, (COUNT_NEED_MS, limit_ms), "pick");
    }

    let task = admitted(
        "a step timeout equal to the need",
        &count_package(Some(COUNT_NEED_MS)),
    );
    if let Err(error) = fresh_step(&task, COUNT_NEED_MS) {
        panic!("the run's check refuses a need equal to the step's window: {error:?}");
    }
}

// SBUD-1 test 2 (item 2): a select over a scan of the card list, whose need is 32 000 ms, is
// compared with the scan's `max_ms`, not with the step timeout (10 000 ms): within 31 999 ms it is
// refused with the need, the limit and the operation; within 32 000 ms it is admitted.
#[test]
fn admission_refuses_scan_select_over_max_ms() {
    let case = "max_ms 1 ms short of the need";
    let error = refusal(case, &scan_package(CARD_NEED_MS - 1));
    assert_budget_invalid(case, &error, (CARD_NEED_MS, CARD_NEED_MS - 1), "pick");
    admitted("max_ms equal to the need", &scan_package(CARD_NEED_MS));
}

// SBUD-1 test 3 (items 3 and 4): a list reading of the card list (need 32 000 ms) within 31 999 ms
// is refused with the need, the limit and the operation, and within 32 000 ms admitted. In a pack
// whose list page is told on three frames 1 000 ms apart, every capture of the reading recognizes
// its page in 2 000 ms: within 1 999 ms that need is refused first; within 2 000 ms it fits, and the
// refusal is the candidate projection's, 32 000 ms over 2 000 ms.
#[test]
fn admission_refuses_list_reading_over_limit() {
    let case = "a list reading's scan 1 ms short of the projection's need";
    let error = refusal(case, &list_package(CARD_NEED_MS - 1, false));
    assert_budget_invalid(
        case,
        &error,
        (CARD_NEED_MS, CARD_NEED_MS - 1),
        "read_members",
    );
    admitted(
        "a list reading's scan equal to the projection's need",
        &list_package(CARD_NEED_MS, false),
    );

    let case = "a list reading's scan 1 ms short of a page recognition";
    let error = refusal(case, &list_package(PAGE_NEED_MS - 1, true));
    assert_budget_invalid(
        case,
        &error,
        (PAGE_NEED_MS, PAGE_NEED_MS - 1),
        "read_members",
    );
    let case = "a list reading's scan equal to a page recognition";
    let error = refusal(case, &list_package(PAGE_NEED_MS, true));
    assert_budget_invalid(case, &error, (CARD_NEED_MS, PAGE_NEED_MS), "read_members");
}

// SBUD-1 test 4: for the count layout, the need admission compares is the `required_ms` the run's
// refusal reports. Admission of the package without a step timeout refuses 8 000 ms over 5 000 ms;
// the run's check at the start of a fresh step of 5 000 ms ends it before any provider call with
// `candidate_sample_budget_insufficient`, 8 000 ms needed and 5 000 ms left; both are the one
// function's answer for the layout.
#[test]
fn admission_and_run_time_share_one_need() {
    let admission = refusal("no step timeout", &count_package(None));
    let task = admitted(
        "a step timeout equal to the need",
        &count_package(Some(COUNT_NEED_MS)),
    );
    let run = match fresh_step(&task, DEFAULT_STEP_MS) {
        Err(ContainedTaskRunError::Task(error)) => error,
        Err(other) => panic!("expected the run's budget refusal, got {other:?}"),
        Ok(()) => panic!("expected the run's budget refusal, the projection went on"),
    };
    assert_eq!(
        run.code().as_str(),
        ExecutionKernelCode::CandidateSampleBudgetInsufficient.as_str(),
        "{run}"
    );
    assert_eq!(
        values(&run),
        json!({"required_ms": COUNT_NEED_MS, "remaining_ms": DEFAULT_STEP_MS})
    );
    assert_eq!(
        values(&admission)["required_ms"],
        values(&run)["required_ms"]
    );
    let layout = task
        .evaluator
        .candidate_layout("layout/counts")
        .expect("the count layout");
    assert_eq!(
        selection::candidate_provider_need_ms(layout, &task.evaluator),
        Some(COUNT_NEED_MS)
    );
}
