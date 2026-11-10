// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #428 S168-5b (register §1.291, its seven named tests; contracts/selection-graph.md,
// "Judge step"): the kernel's halves. A judge step evaluates the candidates of one frame of a
// `fixed_slots` layout, records the decision with the confirmation `judge_only` and ends the run
// Completed on its page with its `outcome_key` or its `unknown_outcome_key`, with no input.
//
// Every package is a synthetic `neutral` package in a 400 x 600 frame, admitted through
// `PreparedContainedTask::load_with_vision_provider`: pixel (0, 0) names the page; the list holds
// three fixed slots 120 px apart, each showing a name code read by a scripted OCR provider and an
// open mark. The ids are made up (`member.a` to `member.e`, `domain.x`, `domain.y`, `state.a`,
// the list `inventory.list.example_kind_a`, the table `example_values`). The runtime is a fake
// device on the scripted clock whose snapshot holds the facts a test gives it, whose tables are
// loaded with `LoadedTable::load`, and whose input port fails the test on any call.

use super::tests::scripted::{ScriptedFrame, ScriptedRuntime};
use super::*;
use actingcommand_contract::codes::{SanitizationCode, TaskflowCode};
use actingcommand_contract::{
    FactContent, FactRecord, FactScalar, FactScope, FactValue, InstanceFactContext,
    InstanceFactSnapshot, SchedulingJudgeResult, SchedulingOutcomeDeclaration,
    TaskSelectionBlocking, TaskSelectionCandidateStatus, TaskSelectionConfirmation,
    TaskSelectionOutcome, TaskSelectionProjection, TaskSelectionRecord, TaskSelectionUnknownCheck,
    TaskSelectionUnknownReason, TaskSelectionUpperBound, parse_standard_pack_table,
};
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition_pack::{
    NnProviderRequest, NnProviderResult, OcrProviderRequest, OcrProviderResult,
    OcrProviderTextBlock, VisionProviderError, VisionProviderErrorCode,
};
use actingcommand_selection_policy::LoadedTable;
use serde_json::{Value, json};
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

const WIDTH: u32 = 400;
const HEIGHT: u32 = 600;
const TASK_TIMEOUT_MS: u64 = 300_000;
// TCLOCK: the instant every snapshot is read at.
const NOW: u64 = 1_752_147_200_000;
const INSTANCE: &str = "neutral.instance";
const LIST_PAGE: &str = "neutral/list";
const JUDGED: &str = "choices_judged";
const UNKNOWN: &str = "choices_unknown";
const LIST_KEY: &str = "inventory.list.example_kind_a";
const MAX_AGE_MS: u64 = 3_600_000;
const TABLE: &str = "example_values";
const TABLE_PATH: &str = "tables/example_values.json";
const POLICY_ENTRY: &str = "resources/operations/task/policies/judge.json";
// The identity domain, and the name each code reads as: the five members' and one name no
// member answers to, which a slot reads out of the dictionary.
const MEMBERS: [&str; 5] = ["member.a", "member.b", "member.c", "member.d", "member.e"];
const NAMES: [&str; 6] = ["alpha", "bravo", "charlie", "delta", "echo", "foxtrot"];
const A: usize = 0;
const B: usize = 1;
const C: usize = 2;
const D: usize = 3;
const E: usize = 4;
const OUT_OF_DICTIONARY: usize = 5;
const BACKGROUND: [u8; 3] = [30, 31, 32];
const GREEN: [u8; 3] = [0, 255, 0];
const HOME_COLOUR: [u8; 3] = [255, 255, 0];
const LIST_COLOUR: [u8; 3] = [255, 0, 0];
const DETAIL_COLOUR: [u8; 3] = [0, 0, 255];

fn put(pixels: &mut [[u8; 3]], x: u32, y: u32, colour: [u8; 3]) {
    pixels[usize::try_from(y * WIDTH + x).expect("pixel index")] = colour;
}

// The list page's pixels: each slot shows the code of its name and, when open, its open mark.
fn list_pixels(slots: [(usize, bool); 3]) -> Vec<[u8; 3]> {
    let mut pixels = vec![BACKGROUND; usize::try_from(WIDTH * HEIGHT).expect("frame size")];
    put(&mut pixels, 0, 0, LIST_COLOUR);
    for (left, (name, open)) in (0..).map(|slot: u32| 120 * slot).zip(slots) {
        if open {
            put(&mut pixels, 22 + left, 102, GREEN);
        }
        let code = u8::try_from(name + 1).expect("name code");
        put(&mut pixels, 24 + left, 102, [code, 0, 0]);
    }
    pixels
}

// The list page as the fake device shows it.
fn list(slots: [(usize, bool); 3]) -> ScriptedFrame {
    ScriptedFrame::rgb(WIDTH, HEIGHT, &list_pixels(slots))
}

// The list page as a saved frame, as `package dry-run` reads one.
fn list_frame(slots: [(usize, bool); 3]) -> Frame {
    Frame::from_pixels(
        WIDTH,
        HEIGHT,
        list_pixels(slots).concat(),
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("list frame")
}

// Reads the name of the code in the pixel its region starts on at confidence 0.99; any other
// pixel reads no text.
#[derive(Debug)]
struct ShownNames;

impl VisionProvider for ShownNames {
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
        let x = u32::try_from(request.region.x).expect("a region on the frame");
        let y = u32::try_from(request.region.y).expect("a region on the frame");
        let index = usize::try_from((y * request.frame.width + x) * 3).expect("pixel index");
        let text = usize::from(request.frame.rgb8_pixels[index])
            .checked_sub(1)
            .and_then(|code| NAMES.get(code).copied())
            .unwrap_or_default();
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

// The Runtime as the kernel sees it: the scripted runtime's frames, traces and clock, a fact
// snapshot of `facts` read at `NOW`, the table `example_values` when given one, and an input port
// that fails the test on any call.
struct Device {
    script: ScriptedRuntime,
    facts: Vec<FactRecord>,
    table: Option<Arc<LoadedTable>>,
}

impl Device {
    fn showing(slots: [(usize, bool); 3]) -> Self {
        Self {
            script: ScriptedRuntime::new(vec![vec![list(slots)]]),
            facts: Vec::new(),
            table: None,
        }
    }

    fn records(&self) -> Vec<&TaskSelectionRecord> {
        self.script
            .traces()
            .iter()
            .filter_map(|trace| match trace {
                ContainedTaskTrace::SelectionEvaluated { selection, .. } => {
                    Some(selection.as_ref())
                }
                _ => None,
            })
            .collect()
    }

    fn decision(&self) -> &TaskSelectionRecord {
        let records = self.records();
        let [record] = records.as_slice() else {
            panic!("one decision record: {:?}", self.script.traces());
        };
        record
    }

    // The step, selection, effect and final traces, in order.
    fn kinds(&self) -> Vec<&'static str> {
        self.script
            .traces()
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
}

impl ContainedTaskRuntime for Device {
    type Error = &'static str;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        self.script.capture()
    }

    fn input(
        &mut self,
        action: InputAction,
        _frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        panic!("a judge step makes no input; the device was handed {action:?}")
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        self.script.record(trace)
    }

    fn selection_state(
        &mut self,
        request: SelectionStateRequest,
    ) -> Result<SelectionState, Self::Error> {
        Ok(SelectionState::Snapshot {
            snapshot: InstanceFactSnapshot {
                snapshot_id: "snapshot:test:judge".to_owned(),
                ledger_position: 1,
                context: InstanceFactContext {
                    instance_id: INSTANCE.to_owned(),
                    server_id: request.server,
                    game_id: request.game,
                },
                records: self.facts.clone(),
            },
            now_unix_ms: NOW,
        })
    }

    fn standard_pack_table(&mut self, path: &str) -> Option<Arc<LoadedTable>> {
        if path == TABLE_PATH {
            self.table.clone()
        } else {
            None
        }
    }

    fn monotonic_now(&mut self) -> Instant {
        self.script.monotonic_now()
    }

    fn pause(&mut self, duration: Duration) {
        self.script.pause(duration);
    }
}

fn rect(x: u32, y: u32, width: u32, height: u32) -> Value {
    json!({"x": x, "y": y, "width": width, "height": height})
}

fn pixel(x: u32, y: u32) -> Value {
    rect(x, y, 1, 1)
}

fn color(id: &str, region: Value, expected: [u8; 3]) -> Value {
    json!({"type": "color", "id": id, "region": region, "expected": expected})
}

fn ocr(id: &str, region: Value) -> Value {
    json!({"type": "ocr", "id": id, "region": region, "languages": ["en"], "timeout_ms": 1000,
           "match_mode": "exact", "expected": ["0"], "case_sensitive": false,
           "minimum_confidence": 0.0, "model_ref": "PP-OCRv6_medium",
           "model_sha256": "a".repeat(64)})
}

// A textured anchor image for the card list (a `repeated_anchor` layout the judge refuses).
fn card_png() -> Vec<u8> {
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
    .expect("card frame")
    .png_for_artifact()
    .expect("card PNG")
}

// The pages, each slot's open mark and name, and the card list's anchor, name and open mark.
fn targets() -> Vec<Value> {
    let mut targets = vec![
        color("page/home", pixel(0, 0), HOME_COLOUR),
        color("page/list", pixel(0, 0), LIST_COLOUR),
        color("page/detail", pixel(0, 0), DETAIL_COLOUR),
        json!({"type": "template", "id": "ui/card", "template_path": "assets/card.png",
               "region": "full_frame", "threshold": 0.9}),
        ocr("card/name", rect(0, 0, 100, 20)),
        color("card/open", rect(0, 0, 20, 20), GREEN),
    ];
    for slot in 0..3 {
        let left = 120 * slot;
        targets.push(color(
            &format!("slot/open_{slot}"),
            pixel(22 + left, 102),
            GREEN,
        ));
        targets.push(ocr(&format!("slot/name_{slot}"), pixel(24 + left, 102)));
    }
    targets
}

fn identity() -> Value {
    let entries = MEMBERS
        .iter()
        .zip(NAMES)
        .map(|(id, name)| json!({"id": id, "aliases": [name]}))
        .collect::<Vec<_>>();
    json!({"entries": entries,
           "recognition": {"kind": "ocr_aliases", "max_distance": 1, "minimum_margin": 1,
                           "minimum_confidence_milli": 800}})
}

// The judge's layout: three fixed slots, each read for its member (a text identity that rejects
// an unknown one) and its open mark.
fn slot_layout() -> Value {
    let slots = (0..3)
        .map(|index| {
            let left = 120 * index;
            json!({"rect": rect(20 + left, 100, 100, 100), "click": rect(30 + left, 160, 40, 30),
                   "targets": {"member": format!("slot/name_{index}"),
                               "open": format!("slot/open_{index}")}})
        })
        .collect::<Vec<_>>();
    json!({
        "id": "layout/slots", "page_id": LIST_PAGE, "kind": "fixed_slots",
        "unknown_identity": "reject",
        "features": [{"name": "member", "value": "identity", "identity": identity()},
                     {"name": "open", "value": "passed"}],
        "slots": slots
    })
}

// The same members as a list read by an anchor (a `repeated_anchor` layout).
fn card_layout() -> Value {
    json!({
        "id": "layout/cards", "page_id": LIST_PAGE, "kind": "repeated_anchor",
        "anchor": "ui/card", "max_instances": 64, "order": "top_to_bottom",
        "instance_rect": {"x": 0, "y": 0, "width": 100, "height": 80},
        "click": {"x": 10, "y": 10, "width": 80, "height": 40},
        "readable_band": {"x": 0, "y": 60, "width": 400, "height": 480},
        "unknown_identity": "reject",
        "features": [
            {"name": "member", "value": "identity", "target": "card/name",
             "offset": {"x": 0, "y": 50}, "identity": identity()},
            {"name": "open", "value": "passed", "target": "card/open",
             "offset": {"x": 80, "y": 0}}
        ]
    })
}

// A judge's document over `layout_id`: the gates `gates`, every survivor chosen (mode
// `none_allowed` with a count of every slot), the lowest candidate ID first.
fn document(layout_id: &str, gates: Value) -> Value {
    json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "choices",
        "applies_to": {
            "candidate_layout_id": layout_id,
            "outcome_keys": {"selected": "choices_selected", "empty": "choices_empty",
                             "insufficient": "choices_insufficient",
                             "ambiguous": "choices_ambiguous", "unknown": "choices_evaluated_unknown"}
        },
        "fields": [{"name": "member", "value_type": {"type": "enum_string", "allowed": MEMBERS}},
                   {"name": "open", "value_type": {"type": "boolean"}}],
        "facts": [],
        "gates": gates,
        "scoring": [],
        "selection": {"mode": "none_allowed", "required_count": 3},
        "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
    })
}

// A slot is judged worth when it is open.
fn open_policy() -> Value {
    document(
        "layout/slots",
        json!([{"gate_id": "open",
                "predicate": {"kind": "boolean_equals",
                              "value": {"source": "field", "field": "open"},
                              "expected": true},
                "on_unknown": {"kind": "drop_candidate"}}]),
    )
}

// A slot is judged worth while the list lists its member with `open` (`keyed_fact`), within
// `MAX_AGE_MS` of the list's read; a gate that reads `keyed_fact` aborts on an unknown.
fn listed_policy() -> Value {
    let mut policy = document(
        "layout/slots",
        json!([{"gate_id": "listed_open",
                "predicate": {"kind": "boolean_equals",
                              "value": {"source": "keyed_fact", "fact_key": LIST_KEY,
                                        "key_field": "member", "value_column": "open",
                                        "absent": {"type": "boolean", "value": false}},
                              "expected": true},
                "on_unknown": {"kind": "abort_evaluation"}}]),
    );
    policy["facts"] = json!([{"fact_key": LIST_KEY,
                              "value_type": {"type": "record_list", "key_column": "id",
                                             "columns": {"open": {"type": "boolean"}}},
                              "max_age_ms": MAX_AGE_MS, "minimum_confidence_milli": 500}]);
    policy
}

// One read of the table's row of class `domain.x` for the slot's member, `absent` for a member
// that has rows, none of the class.
fn table_read(value_column: &str, absent: Value) -> Value {
    json!({"source": "table", "table_id": TABLE, "key_field": "member",
           "pins": [{"column": "domain", "equals": {"type": "string", "value": "domain.x"}}],
           "value_column": value_column, "absent": absent})
}

// A slot is judged worth when its member's row of class `domain.x` is listed (the class gate,
// `absent` false) and its value is above its floor (`integer_compare`, both operands of the
// row), both under `drop_candidate`.
fn table_policy() -> Value {
    let mut policy = document(
        "layout/slots",
        json!([
            {"gate_id": "listed",
             "predicate": {"kind": "boolean_equals",
                           "value": table_read("listed", json!({"type": "boolean", "value": false})),
                           "expected": true},
             "on_unknown": {"kind": "drop_candidate"}},
            {"gate_id": "above_floor",
             "predicate": {"kind": "integer_compare",
                           "left": table_read("value", json!({"type": "integer", "value": 0})),
                           "op": "gt",
                           "right": table_read("floor", json!({"type": "integer", "value": 0}))},
             "on_unknown": {"kind": "drop_candidate"}}
        ]),
    );
    policy["fields"] = json!([{"name": "member",
                               "value_type": {"type": "enum_string", "allowed": MEMBERS}}]);
    policy["tables"] = json!([{"table_id": TABLE, "path": TABLE_PATH,
    "row_key": ["member", "domain"],
    "columns": {
        "domain": {"type": "enum_string",
                   "allowed": ["domain.x", "domain.y"]},
        "listed": {"type": "boolean"},
        "value": {"type": "integer"},
        "floor": {"type": "integer"}
    }}]);
    policy
}

// The table: `member.a` (5, floor 6) and `member.b` (9, floor 6) of `domain.x`, `member.c` of
// `domain.y` only, `member.d` of `domain.x` with a null value; `member.e` has no row.
fn table() -> Arc<LoadedTable> {
    let value = json!({
        "schema_version": "actingcommand.table.v1",
        "game": "neutral",
        "table_id": TABLE,
        "row_key": ["member", "domain"],
        "columns": [
            {"name": "member", "type": "key"},
            {"name": "domain", "type": "enum_string", "allowed": ["domain.x", "domain.y"]},
            {"name": "listed", "type": "boolean"},
            {"name": "value", "type": "integer", "min": -1000, "max": 1000, "nullable": true},
            {"name": "floor", "type": "integer", "min": -1000, "max": 1000}
        ],
        "rows": [
            ["member.a", "domain.x", true, 5, 6],
            ["member.b", "domain.x", true, 9, 6],
            ["member.c", "domain.y", true, 7, 6],
            ["member.d", "domain.x", true, null, 6]
        ]
    });
    let bytes = serde_json::to_vec(&value).expect("table bytes");
    let table = parse_standard_pack_table(&bytes, Some("neutral"), "example_values.json")
        .expect("a valid table");
    Arc::new(LoadedTable::load(table, NOW))
}

// The list `inventory.list.example_kind_a`, `member.a` and `member.b` open, read at
// `observed_at_unix_ms`.
fn list_fact(observed_at_unix_ms: u64) -> FactRecord {
    let row = |id: &str, open: bool| {
        BTreeMap::from([
            ("id".to_owned(), FactScalar::String(id.to_owned())),
            ("open".to_owned(), FactScalar::Boolean(open)),
        ])
    };
    FactRecord {
        scope: FactScope::Instance {
            instance_id: INSTANCE.to_owned(),
        },
        key: LIST_KEY.to_owned(),
        content: FactContent::Inline {
            value: FactValue::RecordList(vec![row(MEMBERS[A], true), row(MEMBERS[B], true)]),
        },
        observed_at_unix_ms,
        expires_at_unix_ms: None,
        ttl_policy: None,
        confidence_milli: 1000,
        source_detector: "list_reading".to_owned(),
        source_snapshot_id: "snapshot-list-a".to_owned(),
        schema_version: "fact.v1".to_owned(),
        resource_bundle_hash: "a".repeat(64),
        invalidate_on: Vec::new(),
    }
}

// The judge step `judge_choices` on the list page over `layout_id`.
fn judge_step(layout_id: &str) -> Value {
    json!({"id": "judge_choices", "from": "list", "to": "list",
           "judge": {"layout_id": layout_id, "policy": {"path": "policies/judge.json"},
                     "outcome_key": JUDGED, "unknown_outcome_key": UNKNOWN}})
}

// The two mappings of the judge's keys on the list page, and the detail page's.
fn mappings() -> Value {
    json!([
        {"outcome_key": JUDGED, "effect": "no_designated_effect", "terminal_pages": ["list"],
         "judge_result": "judged"},
        {"outcome_key": UNKNOWN, "effect": "no_designated_effect", "terminal_pages": ["list"],
         "judge_result": "unknown"},
        {"outcome_key": "detail_reached", "effect": "no_designated_effect",
         "terminal_pages": ["detail"]}
    ])
}

// The task whose one step is the judge over the fixed slots.
fn task() -> Value {
    json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "target_page": "detail",
        "scheduling_outcome": {"mappings": mappings()},
        "candidate_layouts": [slot_layout(), card_layout()],
        "operations": [judge_step("layout/slots")]
    })
}

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("JSON bytes")
}

// The package of `task` with `policy` as its judge's document; its recognition pack holds every
// target and the task's candidate layouts. A phased task's `phases` are copied into the control,
// as admission requires (`contained_task_phases_invalid` when control and program differ).
fn package(task: &Value, policy: &Value) -> Vec<u8> {
    let mut control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.judge.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": WIDTH, "height": HEIGHT},
        "entry_task_id": "task",
        "capture_interval_ms": 100,
        "step_timeout_ms": 10_000,
        "timeout_ms": TASK_TIMEOUT_MS
    });
    if let Some(phases) = task.get("phases") {
        control["phases"] = phases.clone();
    }
    let pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "defaults": {"match_metric": "ccoeff_normed", "color_max_distance": 0.0},
        "targets": targets(),
        "candidate_layouts": task["candidate_layouts"].clone()
    });
    let pages = ["home", "list", "detail"].map(|page| {
        json!({"id": format!("neutral/{page}"), "required": [format!("page/{page}")],
               "optional": [], "forbidden": []})
    });
    let entries = [
        ("control.json", bytes(&control)),
        (
            "resources/manifest.json",
            bytes(&json!({"schema_version": "0.3", "entry_task_id": "task"})),
        ),
        ("resources/operations/task/task.json", bytes(task)),
        (POLICY_ENTRY, bytes(policy)),
        ("resources/assets/card.png", card_png()),
        ("resources/recognition/neutral.test.pack.json", bytes(&pack)),
        (
            "resources/recognition/neutral.test.pages.json",
            bytes(&json!({"schema_version": "0.3", "pages": pages})),
        ),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, content) in entries {
        zip.start_file(path, options).expect("zip entry");
        zip.write_all(&content).expect("zip bytes");
    }
    zip.finish().expect("finish zip").into_inner()
}

fn admit(task: &Value, policy: &Value) -> Result<PreparedContainedTask, ContainedTaskError> {
    PreparedContainedTask::load_with_vision_provider(
        INSTANCE,
        &package(task, policy),
        Arc::new(ShownNames),
    )
}

fn admitted(policy: &Value) -> PreparedContainedTask {
    admit(&task(), policy).unwrap_or_else(|error| panic!("the judge is admitted: {error}"))
}

fn run(task: &PreparedContainedTask, device: &mut Device) -> ContainedTaskOutcome {
    task.run(device)
        .unwrap_or_else(|error| panic!("the judge's run ends Completed: {error:?}"))
}

// The candidate ID of slot `slot` in the record's projection.
fn slot_id(record: &TaskSelectionRecord, slot: u32) -> String {
    let TaskSelectionProjection::V1(projection) = &record.projection else {
        panic!("a judge records the projection of one frame");
    };
    projection
        .candidates()
        .iter()
        .find(|candidate| candidate.instance_index == slot)
        .map(|candidate| candidate.id.clone())
        .expect("the slot's candidate")
}

// The status of slot `slot`'s verdict, if the evaluator judged it.
fn status(record: &TaskSelectionRecord, slot: u32) -> Option<TaskSelectionCandidateStatus> {
    let id = slot_id(record, slot);
    record
        .verdicts
        .iter()
        .find(|verdict| verdict.candidate_id == id)
        .map(|verdict| verdict.status)
}

// The result of the mapping that lists `page` with the key `key`, as the host matches a judge's
// end (`contracts/selection-graph.md`, "Mapping the two keys").
fn mapped(task: &PreparedContainedTask, key: &str) -> Option<SchedulingJudgeResult> {
    task.scheduling_outcome()?
        .mappings()
        .iter()
        .find(|mapping| {
            mapping.outcome_key() == key
                && mapping
                    .terminal_pages()
                    .iter()
                    .any(|page| crate::page_anchor_matches("neutral", LIST_PAGE, page))
        })
        .and_then(|mapping| mapping.judge_result())
}

// The run ended Completed on the list page with `key`, after one step, mapped by `result`.
fn assert_ended(
    case: &str,
    task: &PreparedContainedTask,
    outcome: &ContainedTaskOutcome,
    key: &str,
    result: SchedulingJudgeResult,
) {
    assert_eq!(outcome.outcome, TaskOutcome::Success, "{case}");
    assert_eq!(outcome.final_page.as_deref(), Some(LIST_PAGE), "{case}");
    assert_eq!(outcome.executed_steps, 1, "{case}");
    assert_eq!(
        outcome.selected_scheduling_outcome.as_deref(),
        Some(key),
        "{case}"
    );
    assert_eq!(mapped(task, key), Some(result), "{case}");
}

const NOTHING_UNKNOWN: TaskSelectionUnknownCheck = TaskSelectionUnknownCheck {
    unknown_instances: 0,
    blocking: TaskSelectionBlocking::None,
    upper_bound: TaskSelectionUpperBound::NoneUnknown,
};

// S168-5b test 1: three slots all known, `member.a` and `member.b` open and `member.c` not: one
// record with `judge_only`, the three verdicts (two ranked, one gate-rejected), `selected` the
// two worth ones in rank order and nothing unknown; the run is Completed on the list page with
// the step's `outcome_key`, which the mapping `judged` maps.
#[test]
fn judge_known_ends_with_outcome_key() {
    let task = admitted(&open_policy());
    let mut device = Device::showing([(A, true), (B, true), (C, false)]);

    let outcome = run(&task, &mut device);

    assert_ended(
        "known",
        &task,
        &outcome,
        JUDGED,
        SchedulingJudgeResult::Judged,
    );
    let record = device.decision();
    assert_eq!(record.confirmation, TaskSelectionConfirmation::JudgeOnly);
    assert_eq!(
        (status(record, 0), status(record, 1), status(record, 2)),
        (
            Some(TaskSelectionCandidateStatus::Ranked),
            Some(TaskSelectionCandidateStatus::Ranked),
            Some(TaskSelectionCandidateStatus::GateRejected)
        )
    );
    assert_eq!(
        record.selection_result,
        TaskSelectionOutcome::Selected { count: 2 }
    );
    assert_eq!(record.selected, [slot_id(record, 0), slot_id(record, 1)]);
    assert_eq!(record.unknown_check, Some(NOTHING_UNKNOWN));
    record.validate().expect("the reader takes the record");
    assert_eq!(
        device.kinds(),
        [
            "step_started",
            "selection_evaluated",
            "step_finished",
            "finalizing"
        ]
    );
}

// S168-5b test 2: the same judge with the third slot's name out of the dictionary: the record
// keeps every verdict it has (the two open slots ranked; the third, kept from the evaluator, has
// none), `selected` the two worth ones, and the unknown check blocks with one unknown instance;
// the run is Completed with the step's `unknown_outcome_key`, which the mapping `unknown` maps.
#[test]
fn judge_unknown_slot_ends_with_unknown_key() {
    let task = admitted(&open_policy());
    let mut device = Device::showing([(A, true), (B, true), (OUT_OF_DICTIONARY, true)]);

    let outcome = run(&task, &mut device);

    assert_ended(
        "out of the dictionary",
        &task,
        &outcome,
        UNKNOWN,
        SchedulingJudgeResult::Unknown,
    );
    let record = device.decision();
    assert_eq!(record.confirmation, TaskSelectionConfirmation::JudgeOnly);
    assert_eq!(
        (status(record, 0), status(record, 1), status(record, 2)),
        (
            Some(TaskSelectionCandidateStatus::Ranked),
            Some(TaskSelectionCandidateStatus::Ranked),
            None
        )
    );
    assert_eq!(record.selected, [slot_id(record, 0), slot_id(record, 1)]);
    let check = record
        .unknown_check
        .expect("a judge's record has its check");
    assert_eq!(check.unknown_instances, 1);
    assert_eq!(check.blocking, TaskSelectionBlocking::UnknownInstances);
    assert_ne!(check.upper_bound, TaskSelectionUpperBound::NoneUnknown);
    record.validate().expect("the reader takes the record");
}

// S168-5b test 3: the list the document declares, read two hours before the snapshot, is older
// than its `max_age_ms`: the record holds `unknown` with `fact_stale` and no verdict, and the run
// is Completed with the step's `unknown_outcome_key`, never `selection_not_selected`.
#[test]
fn judge_stale_list_ends_with_unknown_key() {
    let task = admitted(&listed_policy());
    let mut device = Device::showing([(A, true), (B, true), (C, true)]);
    device.facts = vec![list_fact(NOW - 2 * MAX_AGE_MS)];

    let outcome = run(&task, &mut device);

    assert_ended(
        "a stale list",
        &task,
        &outcome,
        UNKNOWN,
        SchedulingJudgeResult::Unknown,
    );
    let record = device.decision();
    assert_eq!(record.confirmation, TaskSelectionConfirmation::JudgeOnly);
    assert_eq!(
        record.selection_result,
        TaskSelectionOutcome::Unknown {
            reason: TaskSelectionUnknownReason::FactStale
        }
    );
    assert!(record.verdicts.is_empty(), "{:?}", record.verdicts);
    assert!(record.selected.is_empty());
    assert_eq!(record.unknown_check, Some(NOTHING_UNKNOWN));
    record.validate().expect("the reader takes the record");

    // The same list read within the bound judges every slot: the listed open members are worth.
    let mut fresh = Device::showing([(A, true), (B, true), (C, true)]);
    fresh.facts = vec![list_fact(NOW - 1_000)];
    let outcome = run(&task, &mut fresh);
    assert_ended(
        "a fresh list",
        &task,
        &outcome,
        JUDGED,
        SchedulingJudgeResult::Judged,
    );
    assert_eq!(
        fresh.decision().selected,
        [slot_id(fresh.decision(), 0), slot_id(fresh.decision(), 1)]
    );
}

// S168-5b test 4: a document that reads the table under `drop_candidate`, every slot read. A
// member without a row (`member.e`) and one whose value cell is null with no fallback
// (`member.d`) are each `unknown_dropped`, a static table drop that the unknown check does not
// count, and each run ends with the step's `unknown_outcome_key`. A run in which the gates decide
// against members on known values (`member.a`'s value below its floor, `member.c`'s class gate
// reading `absent`) and nothing is dropped ends with the step's `outcome_key`.
#[test]
fn judge_unknown_dropped_candidate_ends_with_unknown_key() {
    let task = admitted(&table_policy());
    for (case, third) in [("no row", E), ("a null cell", D)] {
        let mut device = Device::showing([(A, true), (B, true), (third, true)]);
        device.table = Some(table());

        let outcome = run(&task, &mut device);

        assert_ended(
            case,
            &task,
            &outcome,
            UNKNOWN,
            SchedulingJudgeResult::Unknown,
        );
        let record = device.decision();
        assert_eq!(
            (status(record, 0), status(record, 1), status(record, 2)),
            (
                Some(TaskSelectionCandidateStatus::GateRejected),
                Some(TaskSelectionCandidateStatus::Ranked),
                Some(TaskSelectionCandidateStatus::UnknownDropped)
            ),
            "{case}"
        );
        assert_eq!(record.selected, [slot_id(record, 1)], "{case}");
        assert_eq!(record.unknown_check, Some(NOTHING_UNKNOWN), "{case}");
        record
            .validate()
            .unwrap_or_else(|error| panic!("{case}: the reader takes the record: {error}"));
    }

    let mut device = Device::showing([(A, true), (B, true), (C, true)]);
    device.table = Some(table());
    let outcome = run(&task, &mut device);
    assert_ended(
        "known decisions",
        &task,
        &outcome,
        JUDGED,
        SchedulingJudgeResult::Judged,
    );
    let record = device.decision();
    assert_eq!(
        (status(record, 0), status(record, 1), status(record, 2)),
        (
            Some(TaskSelectionCandidateStatus::GateRejected),
            Some(TaskSelectionCandidateStatus::Ranked),
            Some(TaskSelectionCandidateStatus::GateRejected)
        )
    );
    assert_eq!(record.unknown_check, Some(NOTHING_UNKNOWN));
}

// S168-5b test 5, the kernel's half (the host's half is the runtime-host test of the same
// name): in the judge's runs the device's input port receives no call (it fails the test on
// one) and no effect intent or completed effect is recorded for the step; and an architecture
// guard finds no type or call of the device side in the kernel's judge module.
#[test]
fn judge_performs_no_input() {
    for (case, policy, slots, table) in [
        (
            "known",
            open_policy(),
            [(A, true), (B, true), (C, false)],
            None,
        ),
        (
            "an unknown slot",
            open_policy(),
            [(A, true), (B, true), (OUT_OF_DICTIONARY, true)],
            None,
        ),
        (
            "a dropped candidate",
            table_policy(),
            [(A, true), (B, true), (E, true)],
            Some(table()),
        ),
    ] {
        let task = admitted(&policy);
        let mut device = Device::showing(slots);
        device.table = table;
        run(&task, &mut device);
        assert!(device.script.inputs().is_empty(), "{case}");
        assert!(
            !device
                .kinds()
                .iter()
                .any(|kind| matches!(*kind, "effect_intent" | "effect_completed")),
            "{case}: {:?}",
            device.kinds()
        );
    }

    // The judge path holds no type of the device side and calls no input: it is handed neither
    // the runtime nor the step's seed.
    let module = include_str!("judge.rs");
    for name in DEVICE_SIDE {
        assert!(
            !module.contains(name),
            "the kernel's judge module names {name}"
        );
    }
    // Fix 1: `judge_attempt` (in `selection.rs`), the part of the judge path that holds the run's
    // runtime, names no type or call of the device side in its body either, and hands the runtime
    // only to the five callees below, which a select step's attempt shares. What those callees do
    // with it is outside this guard: the fake device's failing input port pins it.
    let body = judge_attempt_body(include_str!("selection.rs"));
    for name in DEVICE_SIDE {
        assert!(!body.contains(name), "judge_attempt names {name}");
    }
    let compact = body.split_whitespace().collect::<String>();
    let handed = [
        "budget_observation_now(runtime,",
        "project_transaction(runtime,",
        "pinned_state(runtime,",
        "step_tables(runtime,",
        "runtime.record(",
    ];
    for callee in handed {
        assert_eq!(
            compact.matches(callee).count(),
            1,
            "judge_attempt: {callee}"
        );
    }
    assert_eq!(
        compact.matches("runtime").count(),
        handed.len(),
        "judge_attempt hands the runtime only to {handed:?}"
    );
}

// The names of the device side the judge path must not hold: its input types, the point draw,
// the runtime trait, the step's seed and the input call.
const DEVICE_SIDE: [&str; 11] = [
    "InputAction",
    "InputFrameContext",
    "InputSamplingEvidence",
    "PointDraw",
    "TouchChoice",
    "ClickRect",
    "ContainedTaskRuntime",
    "step_point_draw",
    "drawn_tap",
    "action_seed",
    ".input(",
];

// The body of `judge_attempt` in `source`: from the brace that opens it to the brace that closes
// it (the signature names the runtime's trait, the body must not).
fn judge_attempt_body(source: &str) -> &str {
    let start = source
        .find("fn judge_attempt<")
        .expect("selection.rs defines judge_attempt");
    let open = start + source[start..].find('{').expect("judge_attempt has a body");
    let mut depth = 0_usize;
    for (offset, character) in source[open..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[open..=open + offset];
                }
            }
            _ => {}
        }
    }
    panic!("judge_attempt's body is not closed")
}

// Admission's own member checks of a judge step, as `prepare_bundle` runs them after the
// declaration gate (which refuses these members first, at their pointers).
fn member_refusal(task: &Value) -> ContainedTaskError {
    let program: TaskProgram = serde_json::from_value(task.clone()).expect("task program");
    program
        .validate_judge_members()
        .expect_err("admission refuses the step")
}

fn with_step(patch: impl FnOnce(&mut Value)) -> Value {
    let mut task = task();
    patch(&mut task["operations"][0]);
    task
}

fn assert_select_invalid(case: &str, error: &ContainedTaskError, token: &str) {
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::ContainedTaskSelectInvalid.as_str(),
        "{case}: {error}"
    );
    let detail = error.detail().unwrap_or_default();
    assert!(
        detail.contains(&format!("judge {token}")),
        "{case}: the detail {detail:?} does not name judge {token}"
    );
}

fn refused(case: &str, task: &Value, policy: &Value) -> ContainedTaskError {
    match admit(task, policy) {
        Ok(_) => panic!("{case}: admitted"),
        Err(error) => error,
    }
}

fn assert_code(case: &str, error: &ContainedTaskError, code: &str) {
    assert_eq!(error.code().as_str(), code, "{case}: {error}");
}

fn declaration(mappings: Value) -> SchedulingOutcomeDeclaration {
    serde_json::from_value(json!({"mappings": mappings})).expect("a scheduling outcome")
}

// S168-5b test 6, the kernel's half (the declaration gate's half is the pack-containment test of
// the same name): the base package (the pair `judged` and `unknown` on one condition and page) is
// admitted and prepared; each rule of the declaration tables is refused with its code and detail
// token, admission's own member checks called as admission calls them where the gate refuses the
// member first, and the whole admission otherwise.
#[test]
fn judge_declaration_refusals() {
    let base = admitted(&open_policy());
    let [step] = base.program.operations.as_slice() else {
        panic!("one operation");
    };
    assert!(step.prepared_judge.is_some(), "the step is prepared");
    assert_eq!(
        base.judge_operation_ids(),
        BTreeSet::from(["judge_choices".to_owned()])
    );
    // A package with a judge step is no prerequisite or return-home layer.
    assert_eq!(base.prerequisite_incompatibility(), Some("judge_end"));

    // Members: admission's own checks, the first refused member in the table's order.
    for (case, task, token) in [
        (
            "an older schema",
            {
                let mut task = task();
                task["schema_version"] = json!("0.8");
                task
            },
            "schema=before_0_9",
        ),
        (
            "the two keys equal",
            with_step(|step| step["judge"]["unknown_outcome_key"] = json!(JUDGED)),
            "keys=equal",
        ),
        (
            "a scan",
            with_step(|step| step["judge"]["scan"] = json!({"identity_feature": "member"})),
            "scan=declared",
        ),
        (
            "a `to` that leaves the page",
            with_step(|step| step["to"] = json!("detail")),
            "to=not_from",
        ),
        (
            "a guard",
            with_step(|step| {
                step["guard"] = json!({"page_id": "list", "target_id": "page/list",
                                       "expected_rect": pixel(0, 0),
                                       "color_probe": "page/list"});
            }),
            "guard=declared",
        ),
        (
            "an expect_after",
            with_step(|step| step["expect_after"] = json!({"page_id": "list"})),
            "expect_after=declared",
        ),
        (
            "a retry",
            with_step(|step| step["retryable"] = json!(true)),
            "retryable=declared",
        ),
    ] {
        assert_select_invalid(case, &member_refusal(&task), token);
    }
    let mut unscheduled = task();
    unscheduled
        .as_object_mut()
        .expect("task")
        .remove("scheduling_outcome");
    assert_code(
        "no scheduling outcome",
        &member_refusal(&unscheduled),
        ExecutionKernelCode::ContainedTaskOutcomeDeclarationIncomplete.as_str(),
    );

    // The layout and the document, through the whole admission.
    let mut cards_policy = open_policy();
    cards_policy["applies_to"]["candidate_layout_id"] = json!("layout/cards");
    let mut top_k = open_policy();
    top_k["selection"] = json!({"mode": "top_k", "required_count": 3});
    let mut below = open_policy();
    below["selection"]["required_count"] = json!(2);
    for (case, task, policy, token) in [
        (
            "a repeated_anchor layout",
            with_step(|step| step["judge"]["layout_id"] = json!("layout/cards")),
            cards_policy,
            "layout=not_fixed_slots",
        ),
        (
            "a mode other than none_allowed",
            task(),
            top_k,
            "mode=not_none_allowed",
        ),
        (
            "a required_count below the slots",
            task(),
            below,
            "required_count=below_slots",
        ),
    ] {
        assert_select_invalid(case, &refused(case, &task, &policy), token);
    }

    // The mappings: each result's state needs its one mapping with the step's key.
    let incomplete = ExecutionKernelCode::ContainedTaskOutcomeDeclarationIncomplete.as_str();
    for (case, kept) in [
        ("no mapping for judged", vec![1, 2]),
        ("no mapping for unknown", vec![0, 2]),
    ] {
        let mut task = task();
        task["scheduling_outcome"]["mappings"] = json!(
            kept.iter()
                .map(|index| mappings()[*index].clone())
                .collect::<Vec<_>>()
        );
        assert_code(case, &refused(case, &task, &open_policy()), incomplete);
    }
    let mut wrong_key = task();
    wrong_key["scheduling_outcome"]["mappings"][0]["outcome_key"] = json!("choices_other");
    assert_code(
        "a mapping with the wrong key",
        &refused("a mapping with the wrong key", &wrong_key, &open_policy()),
        incomplete,
    );
    // A judge state reachable under both effect conditions: a designated tap from the home page
    // reaches the list page, where no mapping can carry the step's keys a second time.
    let mut both = task();
    both["scheduling_outcome"]["designated_operation"] = json!("back_to_list");
    both["operations"].as_array_mut().expect("operations").push(
        json!({"id": "back_to_list", "from": "home", "to": "list",
                     "click": {"kind": "point", "x": 200, "y": 300},
                     "unguarded_trusted_coordinate": true, "retryable": false}),
    );
    assert_code(
        "a state under both effect conditions",
        &refused(
            "a state under both effect conditions",
            &both,
            &open_policy(),
        ),
        incomplete,
    );
    // Fix 1, the phased route: the same task as one stage from the list page to the detail page
    // is admitted, its judge state a terminal of the route's walk; without the mapping of either
    // result, or with the wrong key, the walk refuses it `task_phases_outcome_incomplete`.
    let phased = |mut task: Value| {
        task["entry_page"] = json!("list");
        task["phases"] = json!([{"id": "judge", "operations": ["judge_choices"],
                                 "target_pages": ["detail"]}]);
        task
    };
    let phased_base = admit(&phased(task()), &open_policy())
        .unwrap_or_else(|error| panic!("the phased judge is admitted: {error}"));
    assert!(phased_base.program.phases.is_some(), "one stage");
    let phased_incomplete = TaskflowCode::TaskPhasesOutcomeIncomplete.as_str();
    for (case, kept) in [
        ("phased, no mapping for judged", vec![1, 2]),
        ("phased, no mapping for unknown", vec![0, 2]),
    ] {
        let mut task = phased(task());
        task["scheduling_outcome"]["mappings"] = json!(
            kept.iter()
                .map(|index| mappings()[*index].clone())
                .collect::<Vec<_>>()
        );
        assert_code(
            case,
            &refused(case, &task, &open_policy()),
            phased_incomplete,
        );
    }
    let mut phased_wrong_key = phased(task());
    phased_wrong_key["scheduling_outcome"]["mappings"][1]["outcome_key"] = json!("choices_other");
    assert_code(
        "phased, a mapping with the wrong key",
        &refused(
            "phased, a mapping with the wrong key",
            &phased_wrong_key,
            &open_policy(),
        ),
        phased_incomplete,
    );

    // `judge_result`: refused by the declaration gate, which admission runs first, as another
    // word, an older schema, a task without a judge step or a page that is no judge step's page.
    let gate = "resource_declaration_invalid";
    let mut another_word = task();
    another_word["scheduling_outcome"]["mappings"][0]["judge_result"] = json!("decided");
    let mut older = task();
    older["schema_version"] = json!("0.8");
    let mut without_judge = task();
    without_judge["operations"] = json!([{"id": "leave", "from": "list", "to": "detail",
                                          "click": {"kind": "point", "x": 200, "y": 300},
                                          "unguarded_trusted_coordinate": true,
                                          "retryable": false}]);
    let mut other_page = task();
    other_page["scheduling_outcome"]["mappings"][0]["terminal_pages"] = json!(["list", "detail"]);
    for (case, task) in [
        ("another judge_result", another_word),
        ("an older schema", older),
        ("a task without a judge step", without_judge),
        ("a page that is no judge step's", other_page),
    ] {
        assert_code(case, &refused(case, &task, &open_policy()), gate);
    }

    // The declaration's own check: never with `busy`, and two mappings overlap only with the same
    // condition, page and `judge_result`.
    let mut busy = mappings();
    busy[0]["busy"] = json!(true);
    let error = declaration(busy)
        .validate()
        .expect_err("judge_result with busy is refused");
    assert_eq!(
        (error.code(), error.field()),
        (
            SanitizationCode::InvalidSchedulingOutcomeDeclaration.code(),
            "judge_result"
        )
    );
    let mut overlapping = mappings();
    overlapping[1]["judge_result"] = json!("judged");
    let error = declaration(overlapping)
        .validate()
        .expect_err("two judged mappings of one page overlap");
    assert_eq!(
        (error.code(), error.field()),
        (
            SanitizationCode::InvalidSchedulingOutcomeDeclaration.code(),
            "mapping_overlap"
        )
    );
    declaration(mappings())
        .validate()
        .expect("the pair judged and unknown on one condition and page is admitted");
}

// S168-5b fix 1 (`contracts/selection-graph.md`, "Account data and offline"): `package dry-run` of
// a judge package completes and reports the judge's end, its key and its record, as it reports a
// select step's evaluation: the empty offline snapshot (position 1) at the dry run's instant,
// which the result carries, F1 the one frame captured, nothing confirmed and no input. A document
// over the frame's features judges every slot and ends with the step's `outcome_key`; a document
// that reads a record list finds none in the empty snapshot, so the evaluation is `unknown`
// (`fact_missing`, no verdict) and the end is the step's `unknown_outcome_key`.
#[test]
fn judge_dry_run_reports_the_judge_end() {
    for (case, policy, key, result) in [
        (
            "a document over the frame's features",
            open_policy(),
            JUDGED,
            TaskSelectionOutcome::Selected { count: 2 },
        ),
        (
            "a document that reads a record list",
            listed_policy(),
            UNKNOWN,
            TaskSelectionOutcome::Unknown {
                reason: TaskSelectionUnknownReason::FactMissing,
            },
        ),
    ] {
        let task = admitted(&policy);
        let simulation = crate::simulate_contained_task_at(
            &task,
            vec![list_frame([(A, true), (B, true), (C, false)])],
            NOW,
        )
        .unwrap_or_else(|error| panic!("{case}: the dry run completes: {error}"));

        let crate::OfflineDecision::JudgeCompleted {
            operation_label,
            final_page,
            scheduling_outcome_key,
            selection,
        } = &simulation.decision
        else {
            panic!("{case}: the judge's end: {:?}", simulation.decision);
        };
        assert_eq!(operation_label, "judge_choices", "{case}");
        assert_eq!(final_page.as_deref(), Some(LIST_PAGE), "{case}");
        assert_eq!(scheduling_outcome_key, key, "{case}");
        assert_eq!(
            selection.confirmation,
            TaskSelectionConfirmation::JudgeOnly,
            "{case}"
        );
        assert_eq!(selection.selection_result, result, "{case}");
        assert_eq!(selection.input_ledger_position, 1, "{case}");
        assert_eq!(selection.now_unix_ms, NOW, "{case}");
        assert_eq!(selection.unknown_check, Some(NOTHING_UNKNOWN), "{case}");
        selection
            .validate()
            .unwrap_or_else(|error| panic!("{case}: the reader takes the record: {error}"));
        assert_eq!(simulation.selection_now_unix_ms, Some(NOW), "{case}");
        assert_eq!(
            simulation.capture_count, 1,
            "{case}: F1 only, nothing confirmed"
        );
        assert_eq!(simulation.seed, None, "{case}: no point drawn");
        assert!(!simulation.executed, "{case}");
    }
}
