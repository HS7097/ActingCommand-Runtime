// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #453 S8 (register §1.350; contracts/package-reference.md, the answer of `actinglab
// standard-pack check`; coordinator work order rd-1114/brief-453-s7s8-vision-manifest.md): the
// check of an installed game root answers `models`, the models its packs need: the union of the
// models their recognition packs' OCR and NN targets name, each `{model_ref, model_sha256,
// packages}`, a model several packs name listed once with each of them, in byte order of
// `model_ref`, then of `model_sha256`, each model's `packages` in byte order of `package_id`.
// - `s8_two_packs_that_share_a_model_list_it_once_with_both_packages_in_order`: two packs name
//   one model and one of them a second; a third names none: two models, the second's reference
//   first by its bytes, the shared one once with both packages;
// - `s8_a_root_without_recognition_targets_answers_no_models`: `models: []`;
// - `s8_the_other_members_are_as_before_on_an_existing_fixture`: on the root of `gate_428_cfl2`
//   (bundle.json only), the answer's bytes before `models`, its last member, are the members it
//   had before, in their order, and `models` names the fixture's one model and pack.
// The case of an OCR and an NN target of different models in one pack is held on the package's
// descriptor (`execution-kernel`, `contained_task::gate_453_s8`): a task source declares no NN
// target, so no game root can carry one. Every pack is made up; no model file exists.

use crate::{StandardPackCheckRequest, StandardPackCheckResponse, check_standard_pack_root};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const GAME: &str = "neutral";
const SERVER: &str = "test";
const ALPHA: &str = "neutral.test.alpha";
const BETA: &str = "neutral.test.beta";
const PLAIN: &str = "neutral.test.plain";
/// The model two packs share and the model one pack adds; the second's reference sorts first.
const SHARED_MODEL: (&str, &str) = (
    "neutral-ocr-zeta",
    "1111111111111111111111111111111111111111111111111111111111111111",
);
const ADDED_MODEL: (&str, &str) = (
    "neutral-ocr-eta",
    "2222222222222222222222222222222222222222222222222222222222222222",
);

fn rect(x: u32, y: u32, width: u32, height: u32) -> Value {
    json!({"x": x, "y": y, "width": width, "height": height})
}

fn probe(page: &str, expected: [u8; 3]) -> Value {
    json!({
        "id": format!("{page}_colour"),
        "region": {"mode": "rect", "rect": rect(0, 0, 1, 1)},
        "expected": expected
    })
}

/// An OCR target of the model `(model_ref, model_sha256)`.
fn ocr(id: &str, (model_ref, model_sha256): (&str, &str)) -> Value {
    json!({
        "id": id, "region": {"mode": "rect", "rect": rect(34, 2, 12, 8)}, "languages": ["en"],
        "timeout_ms": 1000, "match_mode": "exact", "expected": ["7"], "case_sensitive": false,
        "minimum_confidence": 0.9, "model_ref": model_ref, "model_sha256": model_sha256
    })
}

/// Writes the source package `package_id` into `directory`: a 64x24 game whose page `home` is
/// told by a blue first pixel and `page_b` by a red one, one trusted click from `home` to
/// `page_b`; each `(page, target)` of `ocr` is an OCR target the page requires besides its colour.
fn write_pack(directory: &Path, package_id: &str, ocr: &[(&str, Value)]) {
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": package_id,
        "execution_mode": "navigable_route",
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
        "goal": "S8 fixture",
        "coordinate_space": {"width": 64, "height": 24},
        "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
        "color_probes": [probe("home", [0, 0, 255]), probe("page_b", [255, 0, 0])],
        "page_rules": {
            "home": {"required": ["home_colour"]},
            "page_b": {"required": ["page_b_colour"]}
        },
        "entry_page": "home",
        "target_page": "page_b",
        "operations": [{
            "id": "go", "from": "home", "to": "page_b",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true, "retryable": false
        }]
    });
    if !ocr.is_empty() {
        task["ocr_targets"] = Value::Array(ocr.iter().map(|(_, target)| target.clone()).collect());
        for (page, target) in ocr {
            if let Some(required) = task["page_rules"][*page]["required"].as_array_mut() {
                required.push(target["id"].clone());
            }
        }
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

/// One pack of a game root: its package id and what writes it in its directory.
type PackWriter<'a> = (&'a str, &'a dyn Fn(&Path));

/// An installed game root in `installed`: each pack `writer` writes in its directory, listed in
/// `bundle.json` in the order given.
fn game_root(installed: &TempDir, packs: &[PackWriter<'_>]) -> PathBuf {
    let game_root = installed.path().join("packages").join(GAME);
    fs::create_dir_all(&game_root).expect("game root");
    for (package_id, writer) in packs {
        writer(&game_root.join(package_id));
    }
    let listed = packs
        .iter()
        .map(|(package_id, _)| {
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
    game_root
}

fn check(game_root: &Path) -> StandardPackCheckResponse {
    check_standard_pack_root(StandardPackCheckRequest {
        root: game_root.to_path_buf(),
    })
    .unwrap_or_else(|error| panic!("standard-pack check: {}: {}", error.code, error.message))
}

#[test]
fn s8_two_packs_that_share_a_model_list_it_once_with_both_packages_in_order() {
    let installed = TempDir::new().expect("temporary directory");
    let alpha = |directory: &Path| {
        write_pack(
            directory,
            ALPHA,
            &[("page_b", ocr("title/alpha", SHARED_MODEL))],
        );
    };
    let beta = |directory: &Path| {
        write_pack(
            directory,
            BETA,
            &[
                ("home", ocr("title/beta", SHARED_MODEL)),
                ("page_b", ocr("count/beta", ADDED_MODEL)),
            ],
        );
    };
    let plain = |directory: &Path| write_pack(directory, PLAIN, &[]);
    let packs: [PackWriter<'_>; 3] = [(ALPHA, &alpha), (BETA, &beta), (PLAIN, &plain)];
    let game_root = game_root(&installed, &packs);

    let checked = check(&game_root);

    assert_eq!(checked.status, "checked");
    assert_eq!(checked.pack_count, 3);
    let answer = serde_json::to_value(&checked).expect("the answer");
    assert_eq!(
        answer["models"],
        json!([
            {"model_ref": ADDED_MODEL.0, "model_sha256": ADDED_MODEL.1, "packages": [BETA]},
            {"model_ref": SHARED_MODEL.0, "model_sha256": SHARED_MODEL.1,
             "packages": [ALPHA, BETA]}
        ]),
        "each model once, by reference, with every package that names it: {answer}"
    );
}

#[test]
fn s8_a_root_without_recognition_targets_answers_no_models() {
    let installed = TempDir::new().expect("temporary directory");
    let plain = |directory: &Path| write_pack(directory, PLAIN, &[]);
    let packs: [PackWriter<'_>; 1] = [(PLAIN, &plain)];
    let game_root = game_root(&installed, &packs);

    let checked = check(&game_root);

    assert_eq!(checked.status, "checked");
    assert!(checked.models.is_empty(), "{:?}", checked.models);
    let answer = serde_json::to_value(&checked).expect("the answer");
    assert_eq!(
        answer["models"],
        json!([]),
        "the member is there, empty: {answer}"
    );
}

#[test]
fn s8_the_other_members_are_as_before_on_an_existing_fixture() {
    use super::gate_428_cfl2 as fixture;
    let installed = TempDir::new().expect("temporary directory");
    let slots = |directory: &Path| fixture::write_pack(directory, fixture::SLOTS);
    let packs: [PackWriter<'_>; 1] = [(fixture::SLOTS, &slots)];
    let game_root = game_root(&installed, &packs);
    assert_eq!((GAME, SERVER), (fixture::GAME, fixture::SERVER), "setup");

    let checked = check(&game_root);

    // The fixture's one warning, as `gate_428_cfl2` pins it.
    assert_eq!(checked.warnings.len(), 1, "{:?}", checked.warnings);
    assert_eq!(
        checked.warnings[0].code().as_str(),
        "package_bundle_preprocess_divergent"
    );
    let line = serde_json::to_string(&checked).expect("the answer");
    let models = serde_json::to_string(&checked.models).expect("the models");
    let tail = format!(r#","models":{models}}}"#);
    let before = line
        .strip_suffix(tail.as_str())
        .unwrap_or_else(|| panic!("models is the answer's last member: {line}"));
    // The answer this root gave before the slice: its members in their order, those a root
    // without schedule.json or notices/ does not have left out.
    let root = std::path::absolute(&game_root).expect("the absolute game root");
    let expected = format!(
        r#"{{"status":"checked","root":{},"game":"{GAME}","root_files":["bundle.json"],"pack_count":1,"warnings":{}"#,
        serde_json::to_string(&root.display().to_string()).expect("the root"),
        serde_json::to_string(&checked.warnings).expect("the warnings"),
    );
    assert_eq!(before, expected, "every member before models as before");
    assert_eq!(
        serde_json::to_value(&checked.models).expect("the models"),
        json!([{"model_ref": fixture::MODEL_REF, "model_sha256": fixture::MODEL_SHA256,
                "packages": [fixture::SLOTS]}]),
        "the fixture's one model, named by its one pack"
    );
}
