// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 B15 R1 (contracts/remedies.md section 10; contracts/package-reference.md, item 5
// of the cross-check; model-b15 section 13 row 2; exec key 102): across the packages of a game
// root, `package bundle` and `standard-pack check` report one `package_bundle_remedy_invalid`
// warning per finding: a capability a remedy names that no package of the root provides
// (`capability_unprovided`), each package on a cycle of remedies and provided capabilities
// (`cycle`), each provider of a capability whose providers declare different parameters
// (`parameters_mismatch`). A root whose remedies are all provided reports nothing new.
//
// A synthetic `neutral` game, server `test`, 16x9: page `home` is told by a blue first pixel and
// page `page_b` by a red one. The feature package recognises both, and the page list holds both,
// so the report "entry page not in the list" is empty. Every other package goes from `home` to
// `page_b`, where its run ends on the key `blocked_k`; its mapping may declare a remedy and its
// control what it provides. Needs, capabilities, parameters and options are neutral names.

use crate::{
    PackageBundleRequest, StandardPackCheckRequest, StandardPackRootFiles,
    check_standard_pack_root, package_bundle,
};
use actingcommand_contract::outcome::Outcome;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

const GAME: &str = "neutral";
const SERVER: &str = "test";
const FEATURES: &str = "neutral.test.features";
const PACK_A: &str = "neutral.test.a";
const PACK_B: &str = "neutral.test.b";
const PACK_C: &str = "neutral.test.c";
const REMEDY_INVALID: &str = "package_bundle_remedy_invalid";

// One package of the root: its id, the remedy of its mapping and what it provides.
struct Pack {
    package_id: &'static str,
    remedy: Option<Value>,
    provides: Option<Value>,
}

fn pack(package_id: &'static str, remedy: Option<Value>, provides: Option<Value>) -> Pack {
    Pack {
        package_id,
        remedy,
        provides,
    }
}

fn probe(page: &str, expected: [u8; 3]) -> Value {
    json!({
        "id": format!("{page}_colour"),
        "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
        "expected": expected
    })
}

// Writes the source package `package_id`: a `recognize_only` feature package of both pages, or a
// route from `home` to `page_b` with its mapping's remedy and its `provides`.
fn write_pack(directory: &Path, pack: &Pack) {
    let features = pack.package_id == FEATURES;
    let mut control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": pack.package_id,
        "execution_mode": if features { "recognize_only" } else { "navigable_route" },
        "game": GAME,
        "server": SERVER,
        "resolution": {"width": 16, "height": 9},
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 2
    });
    if let Some(provides) = &pack.provides {
        control["provides"] = provides.clone();
    }
    let mut task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": GAME,
        "server_scope": [SERVER],
        "locale": "en-US",
        "goal": "B15 R1 fixture",
        "coordinate_space": {"width": 16, "height": 9},
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
        let mut mapping = json!({
            "outcome_key": "blocked_k",
            "effect": "no_designated_effect",
            "terminal_pages": ["page_b"]
        });
        if let Some(remedy) = &pack.remedy {
            mapping["remedy"] = remedy.clone();
        }
        task["target_page"] = json!("page_b");
        task["operations"] = json!([{
            "id": "go", "from": "home", "to": "page_b",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true, "retryable": false
        }]);
        task["scheduling_outcome"] = json!({"mappings": [mapping]});
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

fn pages() -> Vec<u8> {
    serde_json::to_vec_pretty(&json!({
        "schema_version": "actingcommand.pages.v1",
        "game": GAME,
        "features": [FEATURES],
        "pages": [{"id": "home", "kind": "page"}, {"id": "page_b", "kind": "page"}]
    }))
    .expect("page list JSON")
}

// The warnings of `package bundle` (with the page list as its root file) and of `standard-pack
// check` on the installed root, for the feature package and `packs`.
fn reports(packs: Vec<Pack>) -> [Vec<Outcome>; 2] {
    let packs = std::iter::once(pack(FEATURES, None, None))
        .chain(packs)
        .collect::<Vec<_>>();

    let temp = TempDir::new().expect("temporary directory");
    let applications = temp.path().join("applications.json");
    fs::write(
        &applications,
        serde_json::to_vec_pretty(&json!({
            "schema_version": "actingcommand.applications.v1",
            "game": GAME,
            "servers": {SERVER: {"application_id": "com.example.neutral", "label": "Test"}}
        }))
        .expect("applications JSON"),
    )
    .expect("applications table");
    let packs_root = temp.path().join("sources");
    for pack in &packs {
        write_pack(&packs_root.join(pack.package_id), pack);
    }
    let page_list = temp.path().join("pages.json");
    fs::write(&page_list, pages()).expect("page list");
    let bundled = package_bundle(PackageBundleRequest {
        applications,
        packs_root,
        out: temp.path().join("bundle"),
        source: None,
        maintenance: None,
        root_files: StandardPackRootFiles {
            pages: Some(page_list),
            ..StandardPackRootFiles::default()
        },
    })
    .unwrap_or_else(|error| panic!("package bundle: {}: {}", error.code, error.message));

    let installed = TempDir::new().expect("temporary directory");
    let game_root = installed.path().join("packages").join(GAME);
    fs::create_dir_all(&game_root).expect("game root");
    for pack in &packs {
        write_pack(&game_root.join(pack.package_id), pack);
    }
    let listed = packs
        .iter()
        .map(|pack| {
            json!({
                "package_id": pack.package_id, "server": SERVER, "entry_task_id": "task",
                "path": format!("packs/{}", pack.package_id), "file_count": 3, "byte_count": 64
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
    fs::write(game_root.join("pages.json"), pages()).expect("installed page list");
    let checked = check_standard_pack_root(StandardPackCheckRequest { root: game_root })
        .unwrap_or_else(|error| panic!("standard-pack check: {}: {}", error.code, error.message));

    [bundled.warnings, checked.warnings]
}

// Both tools report exactly the `package_bundle_remedy_invalid` entries given by their values, in
// any order (values compare as maps).
fn assert_reports(packs: Vec<Pack>, entries: &[Value]) {
    let wanted = entries
        .iter()
        .map(|values| json!({"code": REMEDY_INVALID, "values": values}))
        .collect::<Vec<_>>();
    for (tool, warnings) in ["package bundle", "standard-pack check"]
        .into_iter()
        .zip(reports(packs))
    {
        let found = warnings
            .iter()
            .map(|warning| json!({"code": warning.code().as_str(), "values": warning.values()}))
            .collect::<Vec<_>>();
        assert!(
            found.len() == wanted.len() && wanted.iter().all(|entry| found.contains(entry)),
            "{tool}: found {found:#?}, wanted {wanted:#?}"
        );
    }
}

// A remedy that names a capability no package of the root provides is reported with the package,
// `capability_unprovided`, the remedy's need, that capability and the mapping's key; its other
// capability, which another package provides, is not.
#[test]
fn b15b_a_remedy_capability_no_package_provides_is_reported() {
    assert_reports(
        vec![
            pack(
                PACK_A,
                Some(json!({"need": "need_a", "capabilities": ["cap_x", "cap_y"]})),
                None,
            ),
            pack(PACK_B, None, Some(json!([{"capability": "cap_x"}]))),
        ],
        &[json!({
            "package_id": PACK_A, "remedy_problem": "capability_unprovided",
            "need": "need_a", "capability": "cap_y", "outcome_key": "blocked_k"
        })],
    );
}

// Two packages whose remedies each name a capability the other provides form a cycle: each is
// reported with `cycle` and the remedy that leads into the cycle. A third package whose remedy
// leads into the cycle but which no package of the cycle reaches is not on it.
#[test]
fn b15b_a_provider_cycle_over_two_packages_is_reported_for_both() {
    assert_reports(
        vec![
            pack(
                PACK_A,
                Some(json!({"need": "need_a", "capabilities": ["cap_x"]})),
                Some(json!([{"capability": "cap_y"}])),
            ),
            pack(
                PACK_B,
                Some(json!({"need": "need_b", "capabilities": ["cap_y"]})),
                Some(json!([{"capability": "cap_x"}])),
            ),
            pack(
                PACK_C,
                Some(json!({"need": "need_a", "capabilities": ["cap_x"]})),
                None,
            ),
        ],
        &[
            json!({
                "package_id": PACK_A, "remedy_problem": "cycle",
                "need": "need_a", "capability": "cap_x", "outcome_key": "blocked_k"
            }),
            json!({
                "package_id": PACK_B, "remedy_problem": "cycle",
                "need": "need_b", "capability": "cap_y", "outcome_key": "blocked_k"
            }),
        ],
    );
}

// Two providers of one capability whose parameters have the same names but different defaults
// are each reported with `parameters_mismatch` and the capability; their other capability, whose
// parameters they declare alike, is not.
#[test]
fn b15b_providers_disagreeing_on_parameters_are_reported() {
    let provides = |default: &str| {
        json!([
            {"capability": "cap_x", "irreversible": true, "parameters": [
                {"name": "param_p", "options": ["opt_1", "opt_2"], "default": default}]},
            {"capability": "cap_y", "parameters": [
                {"name": "param_q", "options": ["opt_3", "opt_4"], "default": "opt_3"}]}
        ])
    };
    assert_reports(
        vec![
            pack(
                PACK_A,
                Some(json!({"need": "need_a", "capabilities": ["cap_x", "cap_y"]})),
                None,
            ),
            pack(PACK_B, None, Some(provides("opt_1"))),
            pack(PACK_C, None, Some(provides("opt_2"))),
        ],
        &[
            json!({
                "package_id": PACK_B, "remedy_problem": "parameters_mismatch",
                "capability": "cap_x"
            }),
            json!({
                "package_id": PACK_C, "remedy_problem": "parameters_mismatch",
                "capability": "cap_x"
            }),
        ],
    );
}

// A root whose every remedy capability a package provides, along a chain without a cycle, with
// one provider per capability, reports nothing beyond the cross-check's own report, which is
// empty here.
#[test]
fn b15b_a_root_whose_remedies_are_all_provided_reports_nothing_new() {
    assert_reports(
        vec![
            pack(
                PACK_A,
                Some(json!({"need": "need_a", "capabilities": ["cap_x", "cap_y"],
                            "default": ["cap_x"]})),
                None,
            ),
            pack(
                PACK_B,
                Some(json!({"need": "need_b", "capabilities": ["cap_y"]})),
                Some(
                    json!([{"capability": "cap_x", "irreversible": true, "parameters": [
                    {"name": "param_p", "options": ["opt_1", "opt_2"], "default": "opt_1"}]}]),
                ),
            ),
            pack(PACK_C, None, Some(json!([{"capability": "cap_y"}]))),
        ],
        &[],
    );
}
