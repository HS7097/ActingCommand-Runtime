// SPDX-License-Identifier: AGPL-3.0-only

// SS-5 (register §1.223; design `rd-1029/design-sch-state.md` §4.6, the SS-5 entry of §5, test
// 5; contracts/standard-pack-schedule.md, "Self-read fact probes"): where a rule meets its package,
// `actinglab standard-pack check` and `package bundle --schedule` refuse a probe written on a key
// the rule's own package does not read. A rule's `fact` predicate with `on_unknown: probe` passes
// when its key is one of the reading marks of the rule's package; a key only another package of
// the root reads, or one no package reads, is `standard_pack_root_invalid` with
// `schedule_invalid` at the member. Neutral fixtures only: game `neutral`, two assembled packages
// of server `test`, each reading two marks of its own (the reader fixture of slice E3).

use crate::standard_pack_check::probe_keys_marked;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::{
    LabErrorClass, StandardPackRoot, StandardPackRootBytes, read_standard_pack_root,
};
use serde_json::{Value, json};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use zip::ZipWriter;
use zip::write::FileOptions;

const GAME: &str = "neutral";
const SERVER: &str = "test";
// The rule's own package marks `resource.own_a` and `resource.own_b`; the other package of the
// root marks `resource.other_a` and `resource.other_b`.
const OWN: &str = "neutral.probe.own";
const OTHER: &str = "neutral.probe.other";
const FACT_UNKNOWN_PROBE: &str = "fact_unknown_probe";

#[test]
fn ss5_t5_a_probe_on_a_key_only_another_package_reads_is_refused() {
    let temp = TempDir::new().expect("temporary directory");
    write_package(
        &temp.path().join(format!("{OWN}.zip")),
        OWN,
        ["own_a", "own_b"],
    );
    write_package(
        &temp.path().join(format!("{OTHER}.zip")),
        OTHER,
        ["other_a", "other_b"],
    );
    let root = root();
    let path = Path::new("schedule.json");
    let packs = |directory: &str| -> PathBuf { temp.path().join(format!("{directory}.zip")) };

    probe_keys_marked(GAME, &root, &schedule("resource.own_b"), path, &packs)
        .unwrap_or_else(|error| panic!("a key the rule's own package marks: {}", error.message));

    for key in ["resource.other_a", "resource.unread"] {
        let error = probe_keys_marked(GAME, &root, &schedule(key), path, &packs)
            .expect_err("a key the rule's own package does not mark is refused");
        assert_eq!(
            error.code,
            SharedCode::StandardPackRootInvalid.as_str(),
            "{key}: {}",
            error.message
        );
        assert_eq!(error.class, LabErrorClass::UsageValidation, "{key}");
        let outcome = error
            .details
            .as_ref()
            .and_then(|details| details.get("outcome"))
            .unwrap_or_else(|| panic!("{key}: no details.outcome: {}", error.message));
        assert_eq!(
            outcome["values"]["standard_pack_problem"], "schedule_invalid",
            "{key}: {outcome}"
        );
        assert_eq!(
            outcome["values"]["json_pointer"], "/rules/0/trigger/predicates/1/on_unknown",
            "{key}: {outcome}"
        );
    }
}

// The game root: a bundle index v4 with both packages on the one server.
fn root() -> StandardPackRoot {
    let pack = |package_id: &str| {
        json!({
            "package_id": package_id,
            "server": SERVER,
            "entry_task_id": "task",
            "path": format!("packs/{package_id}"),
            "file_count": 5,
            "byte_count": 64
        })
    };
    let bundle = serde_json::to_vec(&json!({
        "schema_version": "actingcommand.bundle.v4",
        "game": GAME,
        "packs": [pack(OWN), pack(OTHER)]
    }))
    .expect("bundle bytes");
    read_standard_pack_root(
        StandardPackRootBytes {
            bundle: Some(bundle.as_slice()),
            ..StandardPackRootBytes::default()
        },
        GAME,
    )
    .unwrap_or_else(|issue| panic!("the root reads: {issue}"))
    .expect("the root holds bundle.json")
}

// `schedule.json`: one rule running the own package, due at every whole hour while its count `key`
// is at least 1, the count probed while it is unknown; `requires` names `fact_unknown_probe`.
fn schedule(key: &str) -> Vec<u8> {
    let document: Value = json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": [FACT_UNKNOWN_PROBE],
        "rules": [{
            "id": OWN,
            "package_id": OWN,
            "priority": 100,
            "trigger": {
                "kind": "all",
                "predicates": [
                    {
                        "kind": "clock",
                        "schedule": {
                            "kind": "interval",
                            "clock_source": {"kind": "local"},
                            "every_ms": 3600000,
                            "anchor_ms": 0
                        }
                    },
                    {
                        "kind": "fact",
                        "fact_key": key,
                        "comparison": "greater_than_or_equal",
                        "value": {"type": "integer", "value": 1},
                        "max_age_ms": null,
                        "on_unknown": "probe"
                    }
                ]
            },
            "feedback_stop": {
                "kind": "outcome",
                "task_id": OWN,
                "outcome_key": "completed",
                "comparison": "eq",
                "value": {"type": "boolean", "value": true}
            },
            "consumes": [],
            "produces": [],
            "on_failure": {
                "action": "continue",
                "retry_limit": 2,
                "retry_backoff_ms": 30000,
                "escalation_threshold": 3
            },
            "sensitive": false,
            "next_run_clamp_ms": 86400000,
            "yield_points": [],
            "expected_duration_ms": 60000,
            "cooldown_ms": 1000,
            "load_profile": {"kind": "weighted", "cpu_milli": 200, "gpu_milli": 100, "io_milli": 300},
            "loop_budget": {"daily_limit": 24, "window_iteration_limit": 4, "max_runtime_ms": 300000},
            "strategic_weight_milli": 1000
        }]
    });
    serde_json::to_vec_pretty(&document).expect("schedule bytes")
}

fn ocr_target(id: &str, region: Value) -> Value {
    json!({
        "type": "ocr", "id": id, "region": region, "languages": ["en"], "timeout_ms": 1000,
        "match_mode": "exact", "expected": ["0"], "case_sensitive": false,
        "minimum_confidence": 0.0, "model_ref": "PP-OCRv6_medium",
        "model_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    })
}

// A reading mark `resource.<id>` of `page`, read by `target`.
fn mark(id: &str, page: &str, target: &str) -> Value {
    json!({
        "id": id, "fact_key": format!("resource.{id}"), "page_id": page, "target_id": target,
        "trim": "whitespace_v1",
        "value": {"type": "unsigned_integer", "min": 0, "max": 100000},
        "minimum_confidence_milli": 900, "valid_for_ms": 60000
    })
}

/// Writes the assembled package `package_id` of server `SERVER` at `path` (slice E3's reader): a
/// guarded tap from `home` to `terminal`, a scheduling outcome without a designated operation,
/// and the two marks `marks`, one on each page.
fn write_package(path: &Path, package_id: &str, marks: [&str; 2]) {
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": GAME,
        "server_scope": [SERVER],
        "coordinate_space": {"width": 2, "height": 1},
        "entry_page": "home",
        "target_page": "terminal",
        "scheduling_outcome": {"mappings": [
            {"outcome_key": "read", "effect": "no_designated_effect",
             "terminal_pages": ["terminal"]}
        ]},
        "operations": [{
            "id": "open_terminal",
            "from": "home",
            "to": "terminal",
            "click": {"kind": "point", "x": 1, "y": 0},
            "guard": {
                "page_id": "home",
                "target_id": "guard/ready",
                "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                "color_probe": "guard/ready"
            },
            "retryable": false
        }],
        "resource_readings": [
            mark(marks[0], "home", "count/head"),
            mark(marks[1], "terminal", "count/page")
        ]
    });
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": package_id,
        "execution_mode": "navigable_route",
        "game": GAME,
        "server": SERVER,
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 50,
        "timeout_ms": 5000,
        "max_steps": 2
    });
    let pack = json!({
        "schema_version": "0.7",
        "game": GAME,
        "server": SERVER,
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            {"type": "color", "id": "page/home",
             "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [255, 0, 0]},
            {"type": "color", "id": "page/terminal",
             "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [0, 0, 255]},
            {"type": "color", "id": "guard/ready",
             "region": {"x": 1, "y": 0, "width": 1, "height": 1}, "expected": [0, 255, 0]},
            ocr_target("count/head", json!({"x": 1, "y": 0, "width": 1, "height": 1})),
            ocr_target("count/page", json!({"x": 0, "y": 0, "width": 2, "height": 1}))
        ]
    });
    let pages = json!({
        "schema_version": "0.3",
        "pages": [
            {"id": "neutral/home", "required": ["page/home"], "optional": [], "forbidden": []},
            {"id": "neutral/terminal", "required": ["page/terminal"], "optional": [],
             "forbidden": []}
        ]
    });
    let files = [
        ("control.json", control),
        (
            "resources/manifest.json",
            json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        ("resources/operations/task/task.json", task),
        ("resources/recognition/neutral.test.pack.json", pack),
        ("resources/recognition/neutral.test.pages.json", pages),
    ];
    let mut zip = ZipWriter::new(File::create(path).expect("package file"));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, contents) in files {
        zip.start_file(name, options).expect("package entry");
        zip.write_all(&serde_json::to_vec(&contents).expect("package JSON"))
            .expect("package content");
    }
    zip.finish().expect("finish package");
}
