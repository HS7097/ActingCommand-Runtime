// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #448 SBUD-1 (register §1.333): the offline check refuses what admission refuses. A
// package whose single-frame select step `pick` projects a layout of four slots, each count read
// by its own OCR target of 2 000 ms and no consensus, needs 8 000 ms of its step, and it declares
// no step timeout (5 000 ms). `actinglab package check` refuses it as before any run: exit 2,
// `package_invalid`, whose `details.code` is the admission's `recognition_sample_budget_invalid`
// and whose outcome carries the need, the limit and the operation. The package is a synthetic
// `neutral` package, an assembled ZIP in a temporary directory.

use serde_json::{Value, json};
use std::fs;
use std::io::{Cursor, Write};
use tempfile::TempDir;
use zip::{ZipWriter, write::FileOptions};

const BUDGET_INVALID: &str = "recognition_sample_budget_invalid";
const WIDTH: u32 = 400;
const HEIGHT: u32 = 600;
const SLOTS: u32 = 4;

fn color(id: &str, x: u32, expected: [u8; 3]) -> Value {
    json!({"type": "color", "id": id, "region": {"x": x, "y": 0, "width": 1, "height": 1},
           "expected": expected})
}

// The package: `pick` from the list to the detail page over the count layout, its control with
// no step timeout.
fn package_bytes() -> Vec<u8> {
    let slots = (0..SLOTS)
        .map(|slot| {
            let left = 20 + 90 * slot;
            json!({"rect": {"x": left, "y": 100, "width": 80, "height": 60},
                   "click": {"x": left + 10, "y": 120, "width": 40, "height": 30},
                   "targets": {"count": format!("count/{slot}")}})
        })
        .collect::<Vec<_>>();
    let layout = json!({
        "id": "layout/counts", "page_id": "neutral/list", "kind": "fixed_slots",
        "features": [{"name": "count", "value": "ocr_integer",
                      "integer": {"min": 0, "max": 999, "minimum_confidence_milli": 800}}],
        "slots": slots
    });
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.budget.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": WIDTH, "height": HEIGHT},
        "entry_task_id": "task",
        "capture_interval_ms": 100,
        "timeout_ms": 300_000
    });
    let task = json!({
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
        "candidate_layouts": [layout.clone()],
        "operations": [{
            "id": "pick", "from": "list", "to": "detail",
            "expect_after": {"page_id": "detail", "timeout_ms": 5000, "interval_ms": 200},
            "guard": {"page_id": "list", "target_id": "guard/list",
                      "expected_rect": {"x": 3, "y": 0, "width": 1, "height": 1},
                      "color_probe": "guard/list"},
            "select": {"layout_id": "layout/counts", "policy": {"path": "policies/select.json"}}
        }]
    });
    let policy = json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "counts",
        "applies_to": {
            "candidate_layout_id": "layout/counts",
            "outcome_keys": {"selected": "item_selected", "empty": "item_empty",
                             "insufficient": "item_insufficient", "ambiguous": "item_ambiguous",
                             "unknown": "item_unknown"}
        },
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
    });
    let mut targets = vec![
        color("page/list", 0, [255, 0, 0]),
        color("page/detail", 0, [0, 0, 255]),
        color("guard/list", 3, [0, 255, 0]),
    ];
    for slot in 0..SLOTS {
        targets.push(json!({
            "type": "ocr", "id": format!("count/{slot}"),
            "region": {"x": 30 + 90 * slot, "y": 110, "width": 40, "height": 20},
            "languages": ["en"], "timeout_ms": 2000, "match_mode": "exact", "expected": ["7"],
            "case_sensitive": false, "minimum_confidence": 0.8,
            "model_ref": "PP-OCRv6_medium", "model_sha256": "a".repeat(64)
        }));
    }
    let pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "defaults": {"match_metric": "ccoeff_normed", "color_max_distance": 0.0},
        "targets": targets,
        "candidate_layouts": [layout]
    });
    let pages = ["list", "detail"].map(|page| {
        json!({"id": format!("neutral/{page}"), "required": [format!("page/{page}")],
               "optional": [], "forbidden": []})
    });
    let entries = [
        ("control.json", control),
        (
            "resources/manifest.json",
            json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        ("resources/operations/task/task.json", task),
        ("resources/operations/task/policies/select.json", policy),
        ("resources/recognition/neutral.test.pack.json", pack),
        (
            "resources/recognition/neutral.test.pages.json",
            json!({"schema_version": "0.3", "pages": pages}),
        ),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in entries {
        zip.start_file(path, options).expect("zip entry");
        zip.write_all(&serde_json::to_vec(&value).expect("JSON"))
            .expect("zip bytes");
    }
    zip.finish().expect("finish zip").into_inner()
}

#[test]
fn package_check_refuses_integer_layout_over_step_deadline() {
    let temp = TempDir::new().unwrap();
    let package = temp.path().join("neutral.budget.zip");
    fs::write(&package, package_bytes()).unwrap();
    let result = crate::run_cli(
        [
            "--json",
            "package",
            "check",
            "--package",
            package.to_str().unwrap(),
        ],
        true,
    );
    let envelope = result.envelope_json();
    assert_eq!(result.exit_code(), 2, "{envelope}");
    let error = result
        .envelope
        .error
        .as_ref()
        .unwrap_or_else(|| panic!("not refused: {envelope}"));
    assert_eq!(error.code, "package_invalid", "{envelope}");
    let details = error
        .details
        .as_ref()
        .unwrap_or_else(|| panic!("a refusal without details: {envelope}"));
    assert_eq!(details["code"], BUDGET_INVALID, "{envelope}");
    let outcome = &details["outcome"];
    assert_eq!(outcome["code"], BUDGET_INVALID, "{envelope}");
    assert_eq!(outcome["values"]["required_ms"], 8000, "{envelope}");
    assert_eq!(outcome["values"]["limit_ms"], 5000, "{envelope}");
    assert_eq!(outcome["values"]["pack_operation_id"], "pick", "{envelope}");
}
