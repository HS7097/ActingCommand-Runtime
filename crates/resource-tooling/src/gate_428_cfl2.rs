// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #428 CFL-2 (design CF v2 §5.6; contracts/recognition-preprocess.md, "The authoring
// warning"; outcome-code spec §12, one test per new emission path): `standard-pack check` reports
// one `package_bundle_preprocess_divergent` warning, category `warning`, with the package's
// `package_id`, the OCR target's `target_id` and the shared read's `json_pointer` in the
// recognition pack, for an OCR target that a layout's shared read would serve but whose chain
// differs from the read's. It does so on a game root with `pages.json` and on one with
// `bundle.json` only, the cross-check's branch without the page list.
//
// A synthetic `neutral` game, server `test`, 64x24: page `home` is told by a blue first pixel and
// page `page_b` by a red one. The pack under test goes from `home` to `page_b` and declares, on
// `home`, a `fixed_slots` layout of two slots whose shared read keeps near-white and inverts:
// `slot/a` declares the same chain and takes its text from the read, `slot/b` declares the same
// colors in the other order, another chain, and reads on its own. With the page list, a feature
// package recognises both pages.

use crate::{StandardPackCheckRequest, check_standard_pack_root};
use actingcommand_contract::outcome::{Category, Outcome};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

const GAME: &str = "neutral";
const SERVER: &str = "test";
const FEATURES: &str = "neutral.test.features";
const SLOTS: &str = "neutral.test.slots";
const PREPROCESS_DIVERGENT: &str = "package_bundle_preprocess_divergent";
const MODEL_REF: &str = "neutral-ocr-model";
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn rect(x: u32, y: u32, width: u32, height: u32) -> Value {
    json!({"x": x, "y": y, "width": width, "height": height})
}

// Keep near-white in the colors' order `colors`, then invert.
fn chain(colors: Value) -> Value {
    json!({"steps": [
        {"kind": "keep_colors", "colors": colors, "tolerance": 40},
        {"kind": "invert"}
    ]})
}

fn probe(page: &str, expected: [u8; 3]) -> Value {
    json!({
        "id": format!("{page}_colour"),
        "region": {"mode": "rect", "rect": rect(0, 0, 1, 1)},
        "expected": expected
    })
}

fn ocr(id: &str, region: Value, preprocess: Value) -> Value {
    json!({
        "id": id, "region": {"mode": "rect", "rect": region}, "languages": ["en"],
        "timeout_ms": 1000, "match_mode": "exact", "expected": ["7"], "case_sensitive": false,
        "minimum_confidence": 0.9, "model_ref": MODEL_REF, "model_sha256": MODEL_SHA256,
        "preprocess": preprocess
    })
}

// Writes the source package `package_id`: a `recognize_only` feature package of both pages, or
// the pack under test (module doc).
fn write_pack(directory: &Path, package_id: &str) {
    let features = package_id == FEATURES;
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": package_id,
        "execution_mode": if features { "recognize_only" } else { "navigable_route" },
        "game": GAME,
        "server": SERVER,
        "resolution": {"width": 64, "height": 24},
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 2
    });
    let mut task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": GAME,
        "server_scope": [SERVER],
        "locale": "en-US",
        "goal": "CFL-2 fixture",
        "coordinate_space": {"width": 64, "height": 24},
        "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
        "color_probes": [probe("home", [0, 0, 255]), probe("page_b", [255, 0, 0])],
        "page_rules": {
            "home": {"required": ["home_colour"]},
            "page_b": {"required": ["page_b_colour"]}
        },
        "entry_page": "home"
    });
    if features {
        task["target_page"] = json!("home");
        task["operations"] = json!([{
            "id": "see_b", "from": "page_b", "to": "home",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true, "retryable": false
        }]);
    } else {
        let white = json!([[255, 255, 255], [240, 240, 232]]);
        let reordered = json!([[240, 240, 232], [255, 255, 255]]);
        task["target_page"] = json!("page_b");
        task["operations"] = json!([{
            "id": "go", "from": "home", "to": "page_b",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true, "retryable": false
        }]);
        task["ocr_targets"] = json!([
            ocr("slot/a", rect(34, 2, 12, 8), chain(white.clone())),
            ocr("slot/b", rect(50, 2, 12, 8), chain(reordered))
        ]);
        task["candidate_layouts"] = json!([{
            "id": "layout/cells", "page_id": "home", "kind": "fixed_slots",
            "features": [{"name": "count", "value": "ocr_integer",
                          "integer": {"min": 0, "max": 999, "minimum_confidence_milli": 800}}],
            "slots": [
                {"rect": rect(32, 0, 16, 12), "click": rect(32, 0, 16, 12),
                 "targets": {"count": "slot/a"}},
                {"rect": rect(48, 0, 16, 12), "click": rect(48, 0, 16, 12),
                 "targets": {"count": "slot/b"}}
            ],
            "shared_ocr": {
                "region": {"mode": "rect", "rect": rect(32, 0, 32, 24)}, "languages": ["en"],
                "timeout_ms": 5000, "model_ref": MODEL_REF, "model_sha256": MODEL_SHA256,
                "blocks": "detect", "line_merge": {"gap_factor": 0}, "preprocess": chain(white)
            }
        }]);
    }
    for (path, value) in [
        ("control.json", control),
        (
            "resources/operations/resources.json",
            json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
        ),
        ("resources/operations/task/task.json", task),
    ] {
        let path = directory.join(path);
        fs::create_dir_all(path.parent().expect("pack file parent")).expect("pack directory");
        fs::write(path, serde_json::to_vec_pretty(&value).expect("pack JSON")).expect("pack file");
    }
}

// The warnings of `standard-pack check` on an installed game root of `packs`, with the page list
// beside `bundle.json` when `page_list`.
fn check(packs: &[&str], page_list: bool) -> Vec<Outcome> {
    let installed = TempDir::new().expect("temporary directory");
    let game_root = installed.path().join("packages").join(GAME);
    fs::create_dir_all(&game_root).expect("game root");
    for package_id in packs {
        write_pack(&game_root.join(package_id), package_id);
    }
    let listed = packs
        .iter()
        .map(|package_id| {
            json!({
                "package_id": package_id, "server": SERVER, "entry_task_id": "task",
                "path": format!("packs/{package_id}"), "file_count": 3, "byte_count": 64
            })
        })
        .collect::<Vec<_>>();
    fs::write(
        game_root.join("bundle.json"),
        serde_json::to_vec_pretty(&json!({
            "schema_version": "actingcommand.bundle.v4", "game": GAME, "packs": listed
        }))
        .expect("bundle JSON"),
    )
    .expect("bundle index");
    if page_list {
        let pages = json!({
            "schema_version": "actingcommand.pages.v1",
            "game": GAME,
            "features": [FEATURES],
            "pages": [{"id": "home", "kind": "page"}, {"id": "page_b", "kind": "page"}]
        });
        fs::write(
            game_root.join("pages.json"),
            serde_json::to_vec_pretty(&pages).expect("page list JSON"),
        )
        .expect("page list");
    }
    check_standard_pack_root(StandardPackCheckRequest { root: game_root })
        .unwrap_or_else(|error| panic!("standard-pack check: {}: {}", error.code, error.message))
        .warnings
}

// One warning, and only it, on either root: the code, its category and every required value.
#[test]
fn cfl2_standard_pack_check_names_a_target_whose_chain_differs_from_its_shared_read() {
    for (root, packs, page_list) in [
        ("with pages.json", &[FEATURES, SLOTS][..], true),
        ("with bundle.json only", &[SLOTS][..], false),
    ] {
        let warnings = check(packs, page_list);
        assert_eq!(warnings.len(), 1, "{root}: {warnings:#?}");
        let warning = &warnings[0];
        assert_eq!(warning.code().as_str(), PREPROCESS_DIVERGENT, "{root}");
        assert_eq!(warning.category(), Category::Warning, "{root}");
        assert_eq!(
            Value::Object(warning.values().clone()),
            json!({
                "package_id": SLOTS,
                "target_id": "slot/b",
                "json_pointer": "/candidate_layouts/0/shared_ocr"
            }),
            "{root}"
        );
    }
}
