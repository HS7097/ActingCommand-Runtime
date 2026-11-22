// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #477 LR-1 (0.12.79; register §1.513; contracts/linear-steps.md, "Pressing on one page";
// contracts/package-variables.md, the root check's `variable_cap_range`): the root check holds a
// variable a press count reads to a cap's range. A press step's count may read a package variable
// (`session.dispatch.parameter.<name>`) as a page graph's fact cap reads one; the Runtime's metadata
// reader (`read_remedy_declarations`) counts that key among the package's cap keys, and the root
// check (the contract crate's `judge_root_variables`, through `standard-pack check`'s
// `variable_report`) then refuses the variable, `variable_cap_range` at its `max`, when that is
// above a cap's 1000, and lets it apply otherwise. Fix round 2: moved here from the execution
// kernel's gate, whose sources touch no file system (an architecture guard), over a real game root
// on disk in a temporary directory, as `standard-pack check` reads one.
// A made-up game `neutral`, server `test`, 16x9; every package, page and variable is a placeholder.

use crate::standard_pack_goals::variable_report;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::{StandardPackRoot, StandardPackRootBytes, read_standard_pack_root};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const GAME: &str = "neutral";
const SERVER: &str = "test";
const PACKAGE: &str = "neutral.linear.press";
// The key of the package variable `turns`, which a run reads as the dispatch's parameter.
const TURNS_KEY: &str = "session.dispatch.parameter.turns";

/// A colour probe of the page `page`, told by its first pixel `expected`.
fn probe(page: &str, expected: [u8; 3]) -> Value {
    json!({
        "id": format!("{page}_colour"),
        "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
        "expected": expected
    })
}

/// A trusted click `id` at pixel 1 from `from` to `to`.
fn click(id: &str, from: &str, to: &str) -> Value {
    json!({"id": id, "from": from, "to": to,
           "click": {"kind": "point", "x": 1, "y": 0},
           "unguarded_trusted_coordinate": true, "retryable": false})
}

/// Writes the press package as a package directory: its `linear_steps` control, which declares
/// the integer variable `turns` from 0 to `max`, and its one task, the press path: `open` from
/// home into the list, `plus` on the list pressing `presses` times, `start` from the list to done.
fn write_pack(directory: &Path, max: i64, presses: Value) {
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": PACKAGE,
        "execution_mode": "linear_steps",
        "game": GAME,
        "server": SERVER,
        "resolution": {"width": 16, "height": 9},
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 3,
        "variables": [{"name": "turns", "type": "integer", "title": {"en": "Turns"},
                       "min": 0, "max": max, "default": 1}]
    });
    let mut plus = click("plus", "list", "list");
    plus["presses"] = presses;
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": GAME,
        "server_scope": [SERVER],
        "locale": "en-US",
        "goal": "LR-1 fixture",
        "coordinate_space": {"width": 16, "height": 9},
        "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
        "color_probes": [
            probe("home", [0, 0, 255]),
            probe("list", [255, 0, 0]),
            probe("done", [255, 255, 0])
        ],
        "page_rules": {
            "home": {"required": ["home_colour"]},
            "list": {"required": ["list_colour"]},
            "done": {"required": ["done_colour"]}
        },
        "entry_page": "home",
        "target_page": "done",
        "max_steps": 3,
        "operations": [click("open", "home", "list"), plus, click("start", "list", "done")]
    });
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

/// A game root in a new temporary directory: `bundle.json` registering the press package, in its
/// directory, written by [`write_pack`] with `max` and `presses`; with the root as the contract
/// crate reads it.
fn installed(max: i64, presses: Value) -> (TempDir, PathBuf, StandardPackRoot) {
    let temp = TempDir::new().expect("temporary directory");
    let game_root = temp.path().join(GAME);
    write_pack(&game_root.join(PACKAGE), max, presses);
    let bundle = serde_json::to_vec_pretty(&json!({
        "schema_version": "actingcommand.bundle.v4",
        "game": GAME,
        "packs": [{"package_id": PACKAGE, "server": SERVER, "entry_task_id": "task",
                   "path": format!("packs/{PACKAGE}"), "file_count": 3, "byte_count": 64}]
    }))
    .expect("bundle JSON");
    fs::write(game_root.join("bundle.json"), &bundle).expect("bundle.json");
    let root = read_standard_pack_root(
        StandardPackRootBytes {
            bundle: Some(bundle.as_slice()),
            ..StandardPackRootBytes::default()
        },
        GAME,
    )
    .unwrap_or_else(|issue| panic!("the root reads: {issue}"))
    .expect("the root holds bundle.json");
    (temp, game_root, root)
}

// Fix round 1, test 7 (the review's missing test; the coordinator's ruling 3), moved here in fix
// round 2: the root check reads a press step's key as a cap's. The press path whose count reads
// the package's integer variable `turns` (`session.dispatch.parameter.turns`, `minus` 1) lets the
// variable apply when its `max` is 1000, no warning; with a `max` of 2000, above a cap's 1000, the
// check warns `standard_pack_root_invalid` with `variable_cap_range` at `/variables/0/max` and the
// variable does not apply. The same variable of 2000 beside a constant count, which reads no key,
// applies with no warning.
#[test]
fn lr1_fix1_the_root_check_holds_a_variable_a_press_count_reads_to_a_caps_range() {
    let reads_turns = json!({"fact_key": TURNS_KEY, "minus": 1});
    for (case, max, presses, refused, applied) in [
        (
            "max 1000",
            1000,
            reads_turns.clone(),
            Vec::new(),
            vec!["turns"],
        ),
        (
            "max 2000",
            2000,
            reads_turns,
            vec![(
                Some(json!("variable_cap_range")),
                Some(json!("/variables/0/max")),
            )],
            Vec::new(),
        ),
        (
            "max 2000, a constant count",
            2000,
            json!(3),
            Vec::new(),
            vec!["turns"],
        ),
    ] {
        let (_temp, game_root, root) = installed(max, presses);
        let (warnings, listing) = variable_report(GAME, &root, None, &|directory: &str| {
            game_root.join(directory)
        })
        .unwrap_or_else(|error| {
            panic!(
                "{case}: the root check runs: {}: {}",
                error.code, error.message
            )
        });
        assert!(
            warnings
                .iter()
                .all(|warning| warning.is(SharedCode::StandardPackRootInvalid)),
            "{case}: {warnings:?}"
        );
        assert_eq!(
            warnings
                .iter()
                .map(|warning| {
                    (
                        warning.value("standard_pack_problem").cloned(),
                        warning.value("json_pointer").cloned(),
                    )
                })
                .collect::<Vec<_>>(),
            refused,
            "{case}"
        );
        assert_eq!(
            serde_json::to_value(&listing).expect("the listing"),
            json!({"pack": applied, "model": []}),
            "{case}"
        );
    }
}
