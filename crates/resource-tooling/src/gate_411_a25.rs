// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 A25 (register §1.140; exec key 113; contracts/package-reference.md,
// "Standard-pack root files"): `standard-pack check` admits every package of the registry
// (`bundle.json`) once, by its directory, with or without `pages.json`, `interrupts.json` and
// `navigation.json`, by the admission of the cross-check
// (`PreparedContainedTask::describe_navigation_path`). A package that fails it, or whose
// directory is missing, is refused `package_invalid` naming its directory; the root is
// `checked` only once every package was admitted, and `pack_count` is the registry's length.
//
// A synthetic `neutral` game, server `test`, 16x9, installed as `<temporary>/packages/neutral/`
// with `bundle.json` as its only root file. Each pack goes from page `home` (a blue first pixel)
// to page `page_b` (a red one). A pack may have one defect: its `task.json` cut off half way, so
// it is no JSON document; or its task's coordinate space twice its control's resolution, which
// kernel admission refuses while the remedy report's metadata reader, which reads only the
// control declaration and the scheduling outcome, reads it.

use crate::{StandardPackCheckRequest, StandardPackCheckResponse, check_standard_pack_root};
use actingcommand_contract::LabResult;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const GAME: &str = "neutral";
const SERVER: &str = "test";
const PACK_A: &str = "neutral.test.a";
const PACK_B: &str = "neutral.test.b";
const PACK_C: &str = "neutral.test.c";
const PACKAGE_INVALID: &str = "package_invalid";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Defect {
    Intact,
    TaskCutOff,
    CoordinateSpace,
}

// Writes the pack `package_id` as a package directory: its control, its resource table and its
// one task, with `defect`.
fn write_pack(directory: &Path, package_id: &str, defect: Defect) {
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": package_id,
        "execution_mode": "navigable_route",
        "game": GAME,
        "server": SERVER,
        "resolution": {"width": 16, "height": 9},
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 2
    });
    let (width, height) = if defect == Defect::CoordinateSpace {
        (32, 18)
    } else {
        (16, 9)
    };
    let probe = |page: &str, expected: [u8; 3]| {
        json!({
            "id": format!("{page}_colour"),
            "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
            "expected": expected
        })
    };
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": GAME,
        "server_scope": [SERVER],
        "locale": "en-US",
        "goal": "A25 fixture",
        "coordinate_space": {"width": width, "height": height},
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
    let mut task = serde_json::to_vec_pretty(&task).expect("task JSON");
    if defect == Defect::TaskCutOff {
        task.truncate(task.len() / 2);
    }
    for (path, bytes) in [
        (
            "control.json",
            serde_json::to_vec_pretty(&control).expect("control JSON"),
        ),
        (
            "resources/operations/resources.json",
            serde_json::to_vec_pretty(
                &json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
            )
            .expect("resource table JSON"),
        ),
        ("resources/operations/task/task.json", task),
    ] {
        let path = directory.join(path);
        fs::create_dir_all(path.parent().expect("pack file parent")).expect("pack directory");
        fs::write(path, bytes).expect("pack file");
    }
}

// An installed game root `<temporary>/packages/neutral/` whose only root file is `bundle.json`,
// listing every pack of `packs` (each in its directory, with its defect) and every package of
// `listed_only`, which has no directory.
fn installed(packs: &[(&str, Defect)], listed_only: &[&str]) -> (TempDir, PathBuf) {
    let temp = TempDir::new().expect("temporary directory");
    let game_root = temp.path().join("packages").join(GAME);
    fs::create_dir_all(&game_root).expect("game root");
    for (package_id, defect) in packs {
        write_pack(&game_root.join(package_id), package_id, *defect);
    }
    let listed = packs
        .iter()
        .map(|(package_id, _)| *package_id)
        .chain(listed_only.iter().copied())
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
    (temp, game_root)
}

fn check(game_root: &Path) -> LabResult<StandardPackCheckResponse> {
    check_standard_pack_root(StandardPackCheckRequest {
        root: game_root.to_owned(),
    })
}

// The check of `game_root` is refused `package_invalid`, naming the directory of `package_id`
// below the game root.
fn assert_refused_naming(game_root: &Path, package_id: &str, case: &str) {
    let error = match check(game_root) {
        Ok(checked) => panic!(
            "{case}: answered {} with {} packs",
            checked.status, checked.pack_count
        ),
        Err(error) => error,
    };
    assert_eq!(error.code, PACKAGE_INVALID, "{case}: {}", error.message);
    let directory = std::path::absolute(game_root)
        .expect("absolute game root")
        .join(package_id);
    let directory = directory.display().to_string();
    assert!(
        error.message.contains(&directory),
        "{case}: {} does not name {directory}",
        error.message
    );
}

// Requirement 1: a root with only `bundle.json` whose registry holds a pack whose `task.json` is
// no JSON document is refused, naming that pack's directory.
#[test]
fn a25_a_registry_package_that_fails_admission_is_refused_without_navigation_files() {
    let (_temp, game_root) = installed(
        &[
            (PACK_A, Defect::Intact),
            (PACK_B, Defect::TaskCutOff),
            (PACK_C, Defect::Intact),
        ],
        &[],
    );
    assert_refused_naming(&game_root, PACK_B, "a task.json cut off half way");
}

// Requirements 1 and 2: a root with only `bundle.json` whose packs all pass admission is
// `checked`, with `pack_count` the registry's length and nothing to report; and every package of
// the registry went through admission: whichever of them is one that admission refuses and the
// metadata reader reads, the root is refused, naming that package's directory.
#[test]
fn a25_every_registry_package_is_admitted_without_navigation_files() {
    let ids = [PACK_A, PACK_B, PACK_C];
    let (_temp, game_root) = installed(&ids.map(|id| (id, Defect::Intact)), &[]);
    let checked =
        check(&game_root).unwrap_or_else(|error| panic!("{}: {}", error.code, error.message));
    assert_eq!(checked.status, "checked");
    assert_eq!(checked.game, GAME);
    assert_eq!(checked.root_files, ["bundle.json"]);
    assert_eq!(checked.pack_count, ids.len());
    assert!(checked.warnings.is_empty(), "{:?}", checked.warnings);

    for refused in ids {
        let packs = ids.map(|id| {
            let defect = if id == refused {
                Defect::CoordinateSpace
            } else {
                Defect::Intact
            };
            (id, defect)
        });
        let (_temp, game_root) = installed(&packs, &[]);
        assert_refused_naming(
            &game_root,
            refused,
            &format!("{refused} with a coordinate space other than its resolution"),
        );
    }
}

// Requirement 1: a root with only `bundle.json` whose registry lists a package without its
// directory is refused, naming that directory.
#[test]
fn a25_a_missing_registry_directory_is_refused_without_navigation_files() {
    let (_temp, game_root) = installed(
        &[(PACK_A, Defect::Intact), (PACK_C, Defect::Intact)],
        &[PACK_B],
    );
    assert_refused_naming(&game_root, PACK_B, "a listed package without its directory");
}
