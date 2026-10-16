// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #308 slice SEL-L4b, admission (frozen selector model S0 v2.1 §1.3, §1.4 and §2.1;
// §6 row L4b): a package whose select step declares `scan` or `on_empty`, or whose operation
// declares `expect_after.expect_identity`, is admitted through `PreparedContainedTask::load`
// exactly when the model allows it. Every package is a synthetic `neutral` package: a list page
// whose cards are the SEL-L4b gate's 400 x 600 grid (a template anchor, an OCR identity read
// through `ocr_aliases`, `unknown_identity: reject`, a readable band) read by that gate's scan,
// and a detail page. Each refusal changes the admitted base package in the one declaration under
// test; the same package with that rule satisfied is admitted.

use super::scan::bound_inputs;
use super::*;
use actingcommand_contract::ResourceDeclarationReason;
use actingcommand_contract::codes::TaskflowCode;
use actingcommand_device::CaptureBackendName;
use actingcommand_selection_policy::{FieldDomain, SelectionFactSnapshot, UpperBound, upper_bound};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

const WIDTH: u32 = 400;
const HEIGHT: u32 = 600;
// The package's task timeout; the scan's `max_ms` lies within it.
const TASK_TIMEOUT_MS: u64 = 300_000;
const SCAN_MAX_MS: u64 = 240_000;
const POLICY_ENTRY: &str = "resources/operations/task/policies/select.json";
const EMPTY_KEY: &str = "card_empty";

fn identities() -> [&'static str; 4] {
    ["member-00", "member-01", "member-02", "member-03"]
}

// The cards of the list page (the SEL-L4b gate's layout): an anchor template, the member's name
// read by OCR 50 px below it, and an `open` mark at its top right.
fn card_layout() -> Value {
    let entries = identities().map(|id| json!({"id": id, "aliases": [id.replace('-', " ")]}));
    json!({
        "id": "layout/cards", "page_id": "neutral/list", "kind": "repeated_anchor",
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

// One slot of the list page whose member is told by its icon (S0 §1.3: an identity that is not
// text-based).
fn icon_layout() -> Value {
    json!({
        "id": "layout/icons", "page_id": "neutral/list", "kind": "fixed_slots",
        "unknown_identity": "reject",
        "features": [{"name": "member", "value": "identity",
                      "identity": {"entries": [{"id": "member-00"}, {"id": "member-01"}],
                                   "recognition": {"kind": "icon_templates",
                                                   "minimum_score_milli": 900,
                                                   "minimum_margin_milli": 50}}}],
        "slots": [{"rect": {"x": 0, "y": 60, "width": 400, "height": 100},
                   "click": {"x": 10, "y": 70, "width": 80, "height": 40},
                   "targets": {},
                   "identity_templates": {"member": [
                       {"id": "member-00", "target_id": "icon/member-00"},
                       {"id": "member-01", "target_id": "icon/member-01"}
                   ]}}]
    })
}

// The scan of the SEL-L4b gate: a vertical list over the readable band, from its start check to
// its end check, within `SCAN_MAX_MS`.
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

// The select step: from the list to the detail page of the chosen card, over a scan, ending the
// run with `EMPTY_KEY` when nobody listed is present.
fn pick() -> Value {
    json!({
        "id": "pick", "from": "list", "to": "detail",
        "expect_after": {"page_id": "detail", "timeout_ms": 5000, "interval_ms": 200},
        "guard": {"page_id": "list", "target_id": "guard/list",
                  "expected_rect": {"x": 3, "y": 0, "width": 1, "height": 1},
                  "color_probe": "guard/list"},
        "select": {"layout_id": "layout/cards", "policy": {"path": "policies/select.json"},
                   "scan": scan_declaration(), "on_empty": {"outcome_key": EMPTY_KEY}}
    })
}

// The admitted base task (schema 0.6, page graph): `pick`, and a scheduling outcome that maps
// the detail page and, with the `on_empty` key, the step's `from` page.
fn task() -> Value {
    json!({
        "schema_version": "0.6",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "target_page": "detail",
        "scheduling_outcome": {"mappings": [
            {"outcome_key": "card_selected", "effect": "no_designated_effect",
             "terminal_pages": ["detail"]},
            {"outcome_key": EMPTY_KEY, "effect": "no_designated_effect",
             "terminal_pages": ["list"]}
        ]},
        "candidate_layouts": [card_layout()],
        "operations": [pick()]
    })
}

// The base task as a phased route (schema 0.9): one stage from its entry page `list` to
// `detail`.
fn phased_task() -> Value {
    let mut task = task();
    task["schema_version"] = json!("0.9");
    task["entry_page"] = json!("list");
    task["phases"] = json!([{"id": "choose", "operations": ["pick"], "target_pages": ["detail"]}]);
    task
}

// A phased route from `home`: `claim`, the designated operation, runs while `guard/claim`
// passes and is passed over otherwise, and `skip` reaches the list without it; so `pick` is
// reached before any designated effect and after `claim` completed. The list is mapped under
// both effect conditions, each with its own key, as keys are unique.
fn claim_task() -> Value {
    let mut task = phased_task();
    task["entry_page"] = json!("home");
    task["phases"][0]["operations"] = json!(["claim", "skip", "pick"]);
    task["scheduling_outcome"] = json!({
        "designated_operation": "claim",
        "mappings": [
            {"outcome_key": "card_selected", "effect": "no_designated_effect",
             "terminal_pages": ["detail"]},
            {"outcome_key": "card_claimed", "effect": "designated_effect_completed",
             "terminal_pages": ["detail"]},
            {"outcome_key": EMPTY_KEY, "effect": "no_designated_effect",
             "terminal_pages": ["list"]},
            {"outcome_key": "card_claimed_empty", "effect": "designated_effect_completed",
             "terminal_pages": ["list"]}
        ]
    });
    task["operations"] = json!([
        {"id": "claim", "from": "home", "to": "list",
         "click": {"kind": "point", "x": 200, "y": 300},
         "unguarded_trusted_coordinate": true, "retryable": false,
         "when": {"present": "guard/claim"}},
        {"id": "skip", "from": "home", "to": "list",
         "click": {"kind": "point", "x": 200, "y": 500},
         "unguarded_trusted_coordinate": true, "retryable": false},
        pick()
    ]);
    task
}

// The base task with a post-admission OCR comparison of the detail page's name (schema 0.7),
// whose outcome key names the detail page's mapping.
fn comparison_task() -> Value {
    let mut task = task();
    task["schema_version"] = json!("0.7");
    task["post_admission_ocr"] = json!({
        "page_id": "detail", "target_id": "detail/name",
        "truth_set": {"path": "truth.json"},
        "normalization": "trim_lowercase_v1", "comparison": "exact_set_v1",
        "limits": {"max_frames": 2, "max_items": 8, "max_string_bytes": 64,
                   "max_total_bytes": 4096, "max_truth_entries": 8},
        "outcome_key": "card_selected"
    });
    task
}

// The base task with post-admission fields on `page_ids` (schema 0.8): the detail page's level.
fn fields_task(page_ids: &[&str]) -> Value {
    let mut task = task();
    task["schema_version"] = json!("0.8");
    task["scheduling_outcome"]["mappings"][0]["outcome_key"] = json!("fields_recorded");
    task["post_admission_ocr"] = json!({
        "mode": "fields_v1", "page_ids": page_ids,
        "fields": [{"id": "level", "group": "card", "target_id": "detail/level",
                    "required": true, "privacy": "public", "trim": "whitespace_v1",
                    "value": {"type": "unsigned_integer", "min": 0, "max": 999}}],
        "limits": {"max_frames": 2, "max_items": 8, "max_string_bytes": 64,
                   "max_total_bytes": 4096, "max_truth_entries": 8},
        "outcome_key": "fields_recorded"
    });
    task
}

// The base task whose `pick` is a single-frame select over the icon slot, still declaring
// `on_empty` (its layout is `fixed_slots`).
fn icon_task() -> Value {
    let mut task = task();
    task["candidate_layouts"] = json!([icon_layout()]);
    let select = &mut task["operations"][0]["select"];
    select["layout_id"] = json!("layout/icons");
    select.as_object_mut().expect("select").remove("scan");
    task
}

// `task` with no select step declaring `on_empty`.
fn without_on_empty(task: &Value) -> Value {
    let mut task = task.clone();
    for operation in task["operations"].as_array_mut().expect("operations") {
        if let Some(select) = operation.get_mut("select").and_then(Value::as_object_mut) {
            select.remove("on_empty");
        }
    }
    task
}

fn outcome_keys() -> Value {
    json!({"selected": "card_selected", "empty": EMPTY_KEY, "insufficient": "card_insufficient",
           "ambiguous": "card_ambiguous", "unknown": "card_unknown"})
}

// The base policy over the cards: every open card is a member, of equal score; mode
// `none_allowed` with one candidate; the lowest candidate ID last.
fn policy() -> Value {
    json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "cards",
        "applies_to": {"candidate_layout_id": "layout/cards", "outcome_keys": outcome_keys()},
        "fields": [
            {"name": "member", "value_type": {"type": "enum_string", "allowed": identities()}},
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

// The base policy reading a record list of listed members through `keyed_fact` (S0 §4.2): a gate
// that keeps listed members and a term that ranks them, with the given `on_unknown` kinds.
fn keyed_policy(gate: &str, term: &str) -> Value {
    let listed = json!({"source": "keyed_fact", "fact_key": "session.example.list.targets",
                        "key_field": "member", "value_column": "rank",
                        "absent": {"type": "integer", "value": 0}});
    let mut policy = policy();
    policy["facts"] = json!([{"fact_key": "session.example.list.targets",
                              "value_type": {"type": "record_list", "key_column": "id",
                                             "columns": {"rank": {"type": "integer"}}},
                              "max_age_ms": 3_600_000, "minimum_confidence_milli": 1_000}]);
    policy["gates"]
        .as_array_mut()
        .expect("gates")
        .push(json!({"gate_id": "listed",
                     "predicate": {"kind": "integer_at_least", "value": listed.clone(),
                                   "threshold": 1},
                     "on_unknown": {"kind": gate}}));
    policy["scoring"] = json!([{"term_id": "rank", "value": listed,
                                "transform": {"kind": "identity"}, "weight_milli": 1_000,
                                "on_unknown": {"kind": term}}]);
    policy
}

// The policy over the icon slot.
fn icon_policy() -> Value {
    let mut policy = policy();
    policy["policy_id"] = json!("icons");
    policy["applies_to"]["candidate_layout_id"] = json!("layout/icons");
    policy["fields"] = json!([{"name": "member",
                               "value_type": {"type": "enum_string",
                                              "allowed": ["member-00", "member-01"]}}]);
    policy["gates"] = json!([]);
    policy
}

// One slot of the list page whose layout declares no identity feature: the card is told only by
// its `open` mark.
fn open_slot_layout() -> Value {
    json!({
        "id": "layout/slots", "page_id": "neutral/list", "kind": "fixed_slots",
        "features": [{"name": "open", "value": "passed"}],
        "slots": [{"rect": {"x": 0, "y": 60, "width": 400, "height": 100},
                   "click": {"x": 10, "y": 70, "width": 80, "height": 40},
                   "targets": {"open": "card/open"}}]
    })
}

// The base task whose `pick` is a single-frame select over the identity-free slot, still
// declaring `on_empty`.
fn open_slot_task() -> Value {
    let mut task = task();
    task["candidate_layouts"] = json!([open_slot_layout()]);
    let select = &mut task["operations"][0]["select"];
    select["layout_id"] = json!("layout/slots");
    select.as_object_mut().expect("select").remove("scan");
    task
}

// The base policy over the identity-free slot: the open card is a member.
fn open_slot_policy() -> Value {
    let mut policy = policy();
    policy["policy_id"] = json!("slots");
    policy["applies_to"]["candidate_layout_id"] = json!("layout/slots");
    policy["fields"] = json!([{"name": "open", "value_type": {"type": "boolean"}}]);
    policy
}

fn pixel(x: i32) -> Value {
    json!({"x": x, "y": 0, "width": 1, "height": 1})
}

fn color(id: &str, region: Value, expected: [u8; 3]) -> Value {
    json!({"type": "color", "id": id, "region": region, "expected": expected})
}

fn ocr(id: &str, region: Value, expected: &str) -> Value {
    json!({"type": "ocr", "id": id, "region": region, "languages": ["en"], "timeout_ms": 2000,
           "match_mode": "exact", "expected": [expected], "case_sensitive": false,
           "minimum_confidence": 0.8, "model_ref": "PP-OCRv6_medium",
           "model_sha256": "a".repeat(64)})
}

fn template(id: &str, path: &str, region: Value) -> Value {
    json!({"type": "template", "id": id, "template_path": path, "region": region,
           "threshold": 0.9})
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

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("JSON bytes")
}

fn zip_entries(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, content) in entries {
        zip.start_file(*path, options).expect("zip entry");
        zip.write_all(content).expect("zip bytes");
    }
    zip.finish().expect("finish zip").into_inner()
}

// The package of `task` with `policy` as its select steps' document. A task of schema 0.9 has a
// control of `Lab-1y.control.v2` holding the task's phases. The recognition pack declares the
// task's candidate layouts and their targets; the template images and a declared truth set are
// entries of the package.
fn package(task: &Value, policy: &Value) -> Vec<u8> {
    let mut control = json!({
        "schema_version": "Lab-1y.control.v1",
        "package_id": "neutral.list.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": WIDTH, "height": HEIGHT},
        "entry_task_id": "task",
        "capture_interval_ms": 100,
        "step_timeout_ms": 10_000,
        "timeout_ms": TASK_TIMEOUT_MS
    });
    if task["schema_version"] == "0.9" {
        control["schema_version"] = json!("Lab-1y.control.v2");
        if let Some(phases) = task.get("phases") {
            control["phases"] = phases.clone();
        }
    }
    let layouts = task["candidate_layouts"].clone();
    let icons = layouts
        .as_array()
        .is_some_and(|layouts| layouts.iter().any(|layout| layout["kind"] == "fixed_slots"));
    let mut targets = vec![
        color("page/home", pixel(0), [255, 255, 0]),
        color("page/list", pixel(0), [255, 0, 0]),
        color("page/detail", pixel(0), [0, 0, 255]),
        color("check/list_start", pixel(1), [0, 255, 0]),
        color("check/list_end", pixel(2), [0, 0, 255]),
        color("guard/list", pixel(3), [0, 255, 0]),
        color("guard/claim", pixel(4), [255, 0, 255]),
        template("ui/card", "assets/card.png", json!("full_frame")),
        ocr(
            "card/name",
            json!({"x": 0, "y": 0, "width": 100, "height": 20}),
            "member 00",
        ),
        color(
            "card/open",
            json!({"x": 0, "y": 0, "width": 20, "height": 20}),
            [0, 255, 0],
        ),
        ocr(
            "detail/name",
            json!({"x": 100, "y": 100, "width": 200, "height": 40}),
            "member 00",
        ),
        ocr(
            "detail/level",
            json!({"x": 100, "y": 160, "width": 80, "height": 30}),
            "500",
        ),
    ];
    let mut entries = vec![
        ("control.json", bytes(&control)),
        (
            "resources/manifest.json",
            bytes(&json!({"schema_version": "0.3", "entry_task_id": "task"})),
        ),
        ("resources/operations/task/task.json", bytes(task)),
        (POLICY_ENTRY, bytes(policy)),
        ("resources/assets/card.png", template_png(16, 12, 1)),
    ];
    if icons {
        let slot = json!({"x": 10, "y": 70, "width": 40, "height": 40});
        targets.push(template(
            "icon/member-00",
            "assets/icon-00.png",
            slot.clone(),
        ));
        targets.push(template("icon/member-01", "assets/icon-01.png", slot));
        entries.push(("resources/assets/icon-00.png", template_png(24, 24, 2)));
        entries.push(("resources/assets/icon-01.png", template_png(24, 24, 3)));
    }
    if task.pointer("/post_admission_ocr/truth_set").is_some() {
        entries.push((
            "resources/operations/task/truth.json",
            bytes(&json!({"schema_version": "actingcommand.ocr-truth-set.v1",
                          "items": identities().map(|id| id.replace('-', " "))})),
        ));
    }
    let pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "defaults": {"match_metric": "ccoeff_normed", "color_max_distance": 0.0},
        "targets": targets,
        "candidate_layouts": layouts
    });
    let pages = ["home", "list", "detail"].map(|page| {
        json!({
            "id": format!("neutral/{page}"),
            "required": [format!("page/{page}")],
            "optional": [],
            "forbidden": []
        })
    });
    let pages = json!({"schema_version": "0.3", "pages": pages});
    entries.push(("resources/recognition/neutral.test.pack.json", bytes(&pack)));
    entries.push((
        "resources/recognition/neutral.test.pages.json",
        bytes(&pages),
    ));
    zip_entries(&entries)
}

fn admit(task: &Value, policy: &Value) -> Result<PreparedContainedTask, ContainedTaskError> {
    PreparedContainedTask::load("neutral.instance", &package(task, policy))
}

fn admitted(case: &str, task: &Value, policy: &Value) -> PreparedContainedTask {
    admit(task, policy).unwrap_or_else(|error| panic!("{case}: refused: {error}"))
}

// The refusal of the package, which must carry `code`.
fn refusal(case: &str, task: &Value, policy: &Value, code: &str) -> ContainedTaskError {
    let error = match admit(task, policy) {
        Ok(_) => panic!("{case}: admitted"),
        Err(error) => error,
    };
    assert_eq!(error.code().as_str(), code, "{case}: {error}");
    error
}

fn assert_detail(case: &str, error: &ContainedTaskError, token: &str) {
    let detail = error.detail().unwrap_or_default();
    assert!(
        detail.contains(token),
        "{case}: the detail {detail:?} does not name {token:?}"
    );
}

// S0 §2.1 and §1.4: a scan select over the card layout (a `repeated_anchor` layout with a
// template anchor, an `ocr_aliases` identity and `unknown_identity: reject`) whose scan passes
// the static checks, and whose `on_empty` names the key of the one mapping of its `from` page
// under no designated effect, with a `none_allowed` policy, is admitted; the step is prepared
// with its layout, its identity feature, its key and its scan's time limit.
#[test]
fn l4b_admission_a_scan_select_with_on_empty_is_admitted() {
    let prepared = admitted("scan select with on_empty", &task(), &policy());
    let [operation] = prepared.program.operations.as_slice() else {
        panic!("one operation");
    };
    let select = operation
        .prepared_select
        .as_deref()
        .expect("the select step is prepared");
    assert_eq!(select.layout_id(), "layout/cards");
    assert_eq!(select.identity_feature(), Some("member"));
    assert_eq!(operation.on_empty_key(), Some(EMPTY_KEY));
    assert_eq!(operation.scan_max_ms(), Some(SCAN_MAX_MS));
    assert!(prepared.has_select_steps());
}

// S0 §1.4 (a), (b) and §1.2: `on_empty` needs a `none_allowed` policy, a task that declares
// `scheduling_outcome`, and, on a single-frame select, a `fixed_slots` layout (a one-screen G7
// list declares a scan instead). Each package is admitted without `on_empty`.
#[test]
fn l4b_admission_refuses_on_empty_where_the_step_cannot_end_the_run_normally() {
    let mut exactly_one = policy();
    exactly_one["selection"]["mode"] = json!("exactly_one");
    let mut unscheduled = task();
    unscheduled
        .as_object_mut()
        .expect("task")
        .remove("scheduling_outcome");
    let mut single_frame = task();
    single_frame["operations"][0]["select"]
        .as_object_mut()
        .expect("select")
        .remove("scan");
    for (case, broken, document, token) in [
        (
            "an exactly_one policy",
            task(),
            exactly_one,
            "operation=pick on_empty mode=not_none_allowed",
        ),
        (
            "no scheduling_outcome",
            unscheduled,
            policy(),
            "operation=pick on_empty scheduling_outcome=absent",
        ),
        (
            "a single-frame select over a repeated_anchor layout",
            single_frame,
            policy(),
            "operation=pick on_empty layout=not_fixed_slots",
        ),
    ] {
        admitted(case, &without_on_empty(&broken), &document);
        let error = refusal(
            case,
            &broken,
            &document,
            ExecutionKernelCode::ContainedTaskSelectInvalid.as_str(),
        );
        assert_detail(case, &error, token);
    }
}

// S0 §1.7 and §2.1: a scan's `max_ms` lies within the task's `timeout_ms` (here up to the
// timeout itself), and the policy of a scan select ends its tie-break keys with `candidate_id
// lowest_first`.
#[test]
fn l4b_admission_refuses_a_scan_past_the_task_timeout_or_without_the_lowest_id_last() {
    let mut whole_task = task();
    whole_task["operations"][0]["select"]["scan"]["bounds"]["max_ms"] = json!(TASK_TIMEOUT_MS);
    admitted("max_ms equal to the task timeout", &whole_task, &policy());
    let mut late = task();
    late["operations"][0]["select"]["scan"]["bounds"]["max_ms"] = json!(TASK_TIMEOUT_MS + 60_000);
    let mut highest = policy();
    highest["tie_break"] = json!([{"kind": "candidate_id", "direction": "highest_first"}]);
    for (case, broken, document, token) in [
        (
            "max_ms past the task timeout",
            late,
            policy(),
            "operation=pick scan max_ms=outside_task_timeout",
        ),
        (
            "candidate_id highest_first last",
            task(),
            highest,
            "operation=pick scan tie_break=last_not_candidate_id_lowest_first",
        ),
    ] {
        let error = refusal(
            case,
            &broken,
            &document,
            ExecutionKernelCode::ContainedTaskSelectInvalid.as_str(),
        );
        assert_detail(case, &error, token);
    }
}

// S0 §1.4 (d): in a page graph, an `on_empty` step's state is a terminal too; exactly one
// mapping with its effect condition lists its page, and its key is the step's `on_empty` key.
// Each package is admitted without `on_empty`.
#[test]
fn l4b_admission_a_page_graph_on_empty_step_needs_its_page_mapped_to_its_key() {
    let mut unmapped = task();
    unmapped["scheduling_outcome"]["mappings"]
        .as_array_mut()
        .expect("mappings")
        .truncate(1);
    let mut other_key = task();
    other_key["scheduling_outcome"]["mappings"][1]["outcome_key"] = json!("card_none");
    for (case, broken) in [
        ("the from page unmapped", unmapped),
        ("the from page mapped to another key", other_key),
    ] {
        admitted(case, &without_on_empty(&broken), &policy());
        let error = refusal(
            case,
            &broken,
            &policy(),
            ExecutionKernelCode::ContainedTaskOutcomeDeclarationIncomplete.as_str(),
        );
        assert_detail(case, &error, "operation=pick on_empty page=neutral/list");
    }
}

// S0 §1.4 (c): in a phased route the same terminal is checked by the route's walk. An `on_empty`
// step whose page has no mapping is refused, and so is one reached both before and after a
// designated effect completed: the two effect conditions would need two mappings with the one
// key, which unique keys forbid. Each package is admitted without `on_empty`, and the phased
// route with its page mapped is admitted with it.
#[test]
fn l4b_admission_a_phased_on_empty_step_needs_one_mapping_under_one_effect_condition() {
    admitted("phased, the from page mapped", &phased_task(), &policy());
    let mut unmapped = phased_task();
    unmapped["scheduling_outcome"]["mappings"]
        .as_array_mut()
        .expect("mappings")
        .truncate(1);
    for (case, broken) in [
        ("the from page unmapped", unmapped),
        ("reached under both effect conditions", claim_task()),
    ] {
        admitted(case, &without_on_empty(&broken), &policy());
        refusal(
            case,
            &broken,
            &policy(),
            TaskflowCode::TaskPhasesOutcomeIncomplete.as_str(),
        );
    }
}

// S0 §1.4 (e): a task whose post-admission OCR comparison supplies the run's key declares no
// `on_empty`, and a task with post-admission fields declares one only when the fields' pages
// include the step's `from` page.
#[test]
fn l4b_admission_on_empty_stays_the_run_s_one_key_beside_post_admission_ocr() {
    admitted(
        "a comparison without on_empty",
        &without_on_empty(&comparison_task()),
        &policy(),
    );
    admitted(
        "fields that read the from page",
        &fields_task(&["list", "detail"]),
        &policy(),
    );
    for (case, broken, token) in [
        (
            "a post-admission OCR comparison",
            comparison_task(),
            "operation=pick on_empty post_admission_ocr=comparison",
        ),
        (
            "post-admission fields without the from page",
            fields_task(&["detail"]),
            "operation=pick on_empty post_admission_fields=from_page_absent",
        ),
    ] {
        let error = refusal(
            case,
            &broken,
            &policy(),
            ExecutionKernelCode::ContainedTaskSelectInvalid.as_str(),
        );
        assert_detail(case, &error, token);
    }
}

// S0 §4.2: every gate and term that reads `keyed_fact` declares `on_unknown: abort_evaluation`,
// the policy document's own validation. The declaration gate validates the sealed document
// before the select step is prepared (`contracts/selection-graph.md`, Select step: the document
// "validates"), so the refusal is the gate's `resource_declaration_invalid` at the policy file.
#[test]
fn l4b_admission_refuses_a_keyed_fact_reader_that_does_not_abort_the_evaluation() {
    admitted(
        "a keyed gate and term that abort",
        &task(),
        &keyed_policy("abort_evaluation", "abort_evaluation"),
    );
    for (case, document, token) in [
        (
            "a keyed gate that drops the candidate",
            keyed_policy("drop_candidate", "abort_evaluation"),
            "gate=listed reads=keyed_fact on_unknown",
        ),
        (
            "a keyed term that drops the candidate",
            keyed_policy("abort_evaluation", "drop_candidate"),
            "term=rank reads=keyed_fact on_unknown",
        ),
    ] {
        let error = refusal(
            case,
            &task(),
            &document,
            SharedCode::ResourceDeclarationInvalid.as_str(),
        );
        assert_detail(case, &error, token);
        let issue = error
            .declaration_issue()
            .unwrap_or_else(|| panic!("{case}: the refusal names no declaration"));
        assert_eq!(issue.declaration_file, POLICY_ENTRY, "{case}");
        assert_eq!(
            issue.reason,
            ResourceDeclarationReason::InvalidValue,
            "{case}"
        );
    }
}

// S0 §1.3, admission of `expect_identity`: `choice_of` names a select step at or before the
// declaring one whose identity is text-based (`ocr_aliases`), the declaring operation is not
// retryable, and `target_id` is an OCR target. A retryable step, and a step whose identity is
// read from icons, are refused; each package is admitted without `expect_identity`.
#[test]
fn l4b_admission_expect_identity_maps_a_text_identity_on_a_step_that_never_retries() {
    let expecting = |mut task: Value| {
        task["operations"][0]["expect_after"]["expect_identity"] =
            json!({"target_id": "detail/name", "choice_of": "pick"});
        task
    };
    let prepared = admitted("the step's own choice", &expecting(task()), &policy());
    assert!(prepared.program.operations[0].prepared_select.is_some());

    let mut retryable = task();
    retryable["operations"][0]["retryable"] = json!(true);
    retryable["operations"][0]["max_attempts"] = json!(2);
    retryable["operations"][0]["retry_interval_ms"] = json!(500);
    for (case, base, document, token) in [
        (
            "a retryable step",
            retryable,
            policy(),
            "operation=pick expect_identity operation=retryable",
        ),
        (
            "an icon identity",
            icon_task(),
            icon_policy(),
            "operation=pick expect_identity choice_of=identity_not_text",
        ),
    ] {
        admitted(case, &base, &document);
        let error = refusal(
            case,
            &expecting(base),
            &document,
            ExecutionKernelCode::ContainedTaskSelectInvalid.as_str(),
        );
        assert_detail(case, &error, token);
    }
}

// S0 §1.4 (g): an `on_empty` step may end the run on its `from` page, short of a dependent
// package's entry page, so its package is no prerequisite package; without `on_empty` the same
// package (no stability termination, post-admission OCR, readings or designated operation) is.
#[test]
fn l4b_admission_an_on_empty_package_is_no_prerequisite_package() {
    let with = admitted("with on_empty", &task(), &policy());
    assert_eq!(with.prerequisite_incompatibility(), Some("select_on_empty"));
    let without = admitted("without on_empty", &without_on_empty(&task()), &policy());
    assert_eq!(without.prerequisite_incompatibility(), None);
}

// The extra admission checks of this slice (PR deviation 6, wave-17 acceptance ruling 6) follow.

// Extra admission check 1: every check a scan judges is a target of the pack, its `start.check`,
// its `end.check` and each `page_order` check of a page-flip advance. A check the pack does not
// declare is refused; the base scan and a page flip whose page checks are declared are admitted.
#[test]
fn l4b_admission_refuses_a_scan_check_the_pack_does_not_declare() {
    let page_flip = |checks: Value| {
        let mut task = task();
        task["operations"][0]["select"]["scan"]["advance"] = json!({
            "kind": "rect",
            "rect": {"x": 300, "y": 540, "width": 60, "height": 30},
            "back_rect": {"x": 40, "y": 540, "width": 60, "height": 30},
            "page_order": {"checks": checks, "page_step_px": 300}
        });
        task
    };
    admitted("the base scan", &task(), &policy());
    admitted(
        "a page flip whose page checks are declared",
        &page_flip(json!(["check/list_start", "check/list_end"])),
        &policy(),
    );
    let mut start = task();
    start["operations"][0]["select"]["scan"]["start"]["check"] = json!("check/list_top");
    let mut end = task();
    end["operations"][0]["select"]["scan"]["end"]["check"] = json!("check/list_bottom");
    for (case, broken) in [
        ("an undeclared start check", start),
        ("an undeclared end check", end),
        (
            "an undeclared page check",
            page_flip(json!(["check/list_start", "check/page_1"])),
        ),
    ] {
        let error = refusal(
            case,
            &broken,
            &policy(),
            ExecutionKernelCode::ContainedTaskSelectInvalid.as_str(),
        );
        assert_detail(case, &error, "operation=pick scan check=undeclared");
    }
}

// Extra admission check 2: the upper bound of S0 §3.3 needs a domain for every policy field, and
// a position without a Known identity is bounded over the step's own identity only. A policy
// field that reads another identity feature of the layout has no domain there, so a scan select
// whose policy reads one is refused (`upper_bound`); the same layout under the base policy, which
// reads only the step's identity, is admitted.
#[test]
fn l4b_admission_refuses_a_policy_field_of_another_identity_than_the_step_s() {
    let mut task = task();
    task["candidate_layouts"][0]["features"]
        .as_array_mut()
        .expect("features")
        .push(json!({
            "name": "group", "value": "identity", "target": "card/name",
            "offset": {"x": 0, "y": 30},
            "identity": {
                "entries": [{"id": "group-a", "aliases": ["group a"]},
                            {"id": "group-b", "aliases": ["group b"]}],
                "recognition": {"kind": "ocr_aliases", "max_distance": 1, "minimum_margin": 1,
                                "minimum_confidence_milli": 800}}
        }));
    admitted(
        "the policy reads the step's identity only",
        &task,
        &policy(),
    );
    let mut grouped = policy();
    grouped["fields"]
        .as_array_mut()
        .expect("fields")
        .push(json!({"name": "group",
                     "value_type": {"type": "enum_string", "allowed": ["group-a", "group-b"]}}));
    let case = "the policy reads another identity";
    let error = refusal(
        case,
        &task,
        &grouped,
        ExecutionKernelCode::ContainedTaskSelectInvalid.as_str(),
    );
    assert_detail(
        case,
        &error,
        "operation=pick upper_bound field=group identity=not_the_step_identity",
    );
}

// Extra admission rule 3: a single-frame select that declares `on_empty` over a `fixed_slots`
// layout with no identity feature is admitted, and an unknown slot of it is bounded with one
// neutral hypothesis over its field domains. The open mark may still pass, so the slot may be a
// member scoring 0: the bound is `Max(0)`, never none-eligible.
#[test]
fn l4b_admission_an_identity_free_on_empty_layout_is_bounded_with_one_neutral_hypothesis() {
    let prepared = admitted(
        "an identity-free single-frame on_empty select",
        &open_slot_task(),
        &open_slot_policy(),
    );
    let [operation] = prepared.program.operations.as_slice() else {
        panic!("one operation");
    };
    let select = operation
        .prepared_select
        .as_deref()
        .expect("the select step is prepared");
    assert_eq!(select.identity_feature(), None);
    assert_eq!(operation.on_empty_key(), Some(EMPTY_KEY));
    let layout = prepared
        .evaluator
        .candidate_layout("layout/slots")
        .expect("the slot layout");
    let (domains, hypotheses) = bound_inputs(layout, select.policy(), select.identity_feature())
        .expect("the bound's inputs");
    assert_eq!(
        domains,
        BTreeMap::from([("open".to_owned(), FieldDomain::Boolean)])
    );
    assert_eq!(hypotheses.len(), 1, "{hypotheses:?}");
    let facts = SelectionFactSnapshot {
        snapshot_id: "snapshot-empty".into(),
        snapshot_at_unix_ms: 0,
        facts: BTreeMap::new(),
    };
    let bound = upper_bound(select.policy(), &facts, 0, &hypotheses, &domains).expect("the bound");
    assert_eq!(bound, UpperBound::Max(0));
}

// Extra admission check 4: `expect_identity` reads `target_id` by OCR (S0 §1.3), so a target the
// pack declares with another kind, or does not declare, is refused; the same step reading the
// detail page's OCR name is admitted.
#[test]
fn l4b_admission_refuses_an_expect_identity_target_that_is_no_ocr_target() {
    let expecting = |target: &str| {
        let mut task = task();
        task["operations"][0]["expect_after"]["expect_identity"] =
            json!({"target_id": target, "choice_of": "pick"});
        task
    };
    admitted(
        "the detail page's OCR name",
        &expecting("detail/name"),
        &policy(),
    );
    for (case, target) in [
        ("a colour target", "page/detail"),
        ("an undeclared target", "detail/title"),
    ] {
        let error = refusal(
            case,
            &expecting(target),
            &policy(),
            ExecutionKernelCode::ContainedTaskSelectInvalid.as_str(),
        );
        assert_detail(
            case,
            &error,
            "operation=pick expect_identity target=not_ocr",
        );
    }
}
