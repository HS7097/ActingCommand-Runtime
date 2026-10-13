// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #373 NAV-3b (model M-373 v1.1 §4.1; test rows N3b-1 and N3b-2): a page-graph
//! task's `entry_page` may be a set of 1–16 pages. Admission refuses a set that breaks the shape
//! with `contained_task_entry_pages_invalid` and its `entry_problem`; in a phased route each
//! page of the set starts the one stage that lists an operation from it, and the route must be
//! reachable from every such start. The packages are synthetic `neutral` packages admitted
//! through `PreparedContainedTask::load`.

use crate::{ContainedTaskCode, ContainedTaskError, PreparedContainedTask};
use actingcommand_contract::PackageRef;
use actingcommand_contract::outcome::Category;
use actingcommand_pack_containment::Sha256Hash;
use serde_json::{Value, json};
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

/// The package's named pages; a frame's left pixel names its page.
const PAGES: [(&str, [u8; 3]); 4] = [
    ("home", [255, 0, 0]),
    ("panel", [0, 0, 255]),
    ("result", [255, 0, 255]),
    ("other", [255, 255, 0]),
];
/// Twelve more pages (`spare1`..`spare12`), so that a set of 16 pages resolves.
const SPARE_PAGES: u8 = 12;

/// Every page of the package with its colour.
fn page_colors() -> Vec<(String, [u8; 3])> {
    PAGES
        .iter()
        .map(|(page, color)| ((*page).to_owned(), *color))
        .chain((1..=SPARE_PAGES).map(|index| (format!("spare{index}"), [index, 7, 7])))
        .collect()
}

fn zip_entries(entries: &[(&str, Value)]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in entries {
        zip.start_file(*path, options).expect("zip entry");
        serde_json::to_writer(&mut zip, value).expect("zip JSON");
        zip.write_all(b"\n").expect("zip newline");
    }
    zip.finish().expect("finish zip").into_inner()
}

/// A `navigable_route` package (control v2, operation schema 0.9) around `task`; a phased
/// task's `phases` are copied into the control, as the contract requires.
fn package(task: Value) -> Vec<u8> {
    let mut control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.entry.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 50,
        "timeout_ms": 5000,
        "max_steps": 4
    });
    if let Some(phases) = task.get("phases") {
        control["phases"] = phases.clone();
    }
    let targets = page_colors()
        .iter()
        .map(|(page, color)| {
            json!({
                "type": "color",
                "id": format!("page/{page}"),
                "region": {"x": 0, "y": 0, "width": 1, "height": 1},
                "expected": color
            })
        })
        .collect::<Vec<_>>();
    let pages = page_colors()
        .iter()
        .map(|(page, _)| {
            json!({
                "id": format!("neutral/{page}"),
                "required": [format!("page/{page}")],
                "optional": [],
                "forbidden": []
            })
        })
        .collect::<Vec<_>>();
    zip_entries(&[
        ("control.json", control),
        (
            "resources/manifest.json",
            json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        ("resources/operations/task/task.json", task),
        (
            "resources/recognition/neutral.test.pack.json",
            json!({
                "schema_version": "0.3",
                "game": "neutral",
                "server": "test",
                "coordinate_space": {"width": 2, "height": 1},
                "defaults": {"color_max_distance": 0.0},
                "targets": targets
            }),
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            json!({"schema_version": "0.3", "pages": pages}),
        ),
    ])
}

fn operation(id: &str, from: &str, to: &str) -> Value {
    json!({
        "id": id,
        "from": from,
        "to": to,
        "click": {"kind": "point", "x": 1, "y": 0},
        "unguarded_trusted_coordinate": true,
        "retryable": false
    })
}

/// A route without stages: every page of the set reaches `result` in one step.
fn unphased_task(entry_page: Value) -> Value {
    json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "entry_page": entry_page,
        "target_page": "result",
        "operations": [
            operation("from_home", "home", "result"),
            operation("from_panel", "panel", "result")
        ]
    })
}

/// A three-stage route: `home` starts the first stage, `panel` the second; `extra` operations
/// join the named stages.
fn phased_task(entry_page: Value, extra: &[(&str, &str, &str, usize)]) -> Value {
    let mut phases = json!([
        {"id": "depart", "operations": ["open_panel"], "target_pages": ["panel"]},
        {"id": "business", "operations": ["submit"], "target_pages": ["result"]},
        {"id": "return", "operations": ["back_home"], "target_pages": ["home"]}
    ]);
    let mut operations = vec![
        operation("open_panel", "home", "panel"),
        operation("submit", "panel", "result"),
        operation("back_home", "result", "home"),
    ];
    for (id, from, to, phase) in extra {
        operations.push(operation(id, from, to));
        phases[*phase]["operations"]
            .as_array_mut()
            .expect("stage operations")
            .push(json!(id));
    }
    json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "entry_page": entry_page,
        "phases": phases,
        "target_page": "home",
        "scheduling_outcome": {
            "designated_operation": "submit",
            "mappings": [{
                "outcome_key": "returned",
                "effect": "designated_effect_completed",
                "terminal_pages": ["home"]
            }]
        },
        "operations": operations
    })
}

fn admit(task: Value) -> Result<PreparedContainedTask, ContainedTaskError> {
    let bytes = package(task);
    let expected = Sha256Hash::digest(&bytes).to_string();
    PreparedContainedTask::load(
        "neutral.instance",
        &bytes,
        &PackageRef::LegacyZipSha256(expected),
    )
}

/// N3b-1: a set of 1–16 distinct pages, none `any`, each resolving to one page, and in a phased
/// route each starting exactly one stage, is admitted; each violation is refused with
/// `contained_task_entry_pages_invalid` (error) and its `entry_problem`.
#[test]
fn entry_page_sets_are_admitted_or_refused_with_their_entry_problem() {
    let sixteen = page_colors()
        .into_iter()
        .map(|(page, _)| page)
        .collect::<Vec<_>>();
    assert_eq!(sixteen.len(), 16);
    for (case, task) in [
        (
            "set without stages",
            unphased_task(json!(["home", "panel"])),
        ),
        ("set of one page", unphased_task(json!(["panel"]))),
        ("set of sixteen pages", unphased_task(json!(sixteen))),
        ("phased set", phased_task(json!(["home", "panel"]), &[])),
    ] {
        if let Err(error) = admit(task) {
            panic!("{case}: admitted set refused: {error}");
        }
    }
    let mut seventeen = sixteen;
    seventeen.push("spare13".to_owned());
    for (task, problem) in [
        (unphased_task(json!([])), "empty"),
        (unphased_task(json!(seventeen)), "too_many"),
        (unphased_task(json!(["home", "neutral/home"])), "duplicate"),
        (unphased_task(json!(["home", "any"])), "any_in_set"),
        (unphased_task(json!(["home", "missing"])), "unresolved"),
        (
            phased_task(json!(["home", "other"]), &[]),
            "phase_start_missing",
        ),
        (
            phased_task(
                json!(["home", "panel"]),
                &[("panel_back", "panel", "home", 2)],
            ),
            "phase_start_ambiguous",
        ),
    ] {
        let Err(error) = admit(task) else {
            panic!("{problem}: refused set admitted");
        };
        assert_eq!(
            error.code().as_str(),
            "contained_task_entry_pages_invalid",
            "{problem}: {error}"
        );
        let ContainedTaskCode::Registered(code) = error.code() else {
            panic!("{problem}: unregistered code");
        };
        assert_eq!(code.category(), Category::Error, "{problem}");
        assert_eq!(
            error.detail(),
            Some(format!("entry_problem={problem}").as_str()),
            "{problem}"
        );
    }
}

/// N3b-2: the phased route's reachability is checked once from every start of the set; a start
/// from which no terminal is reachable keeps today's unreachable code.
#[test]
fn phased_route_reachability_is_checked_from_every_entry_start() {
    // `other` starts the second stage, whose only operation from it leads back to `home`,
    // where the second stage has no operation.
    let stray = [("stray", "other", "home", 1)];
    if let Err(error) = admit(phased_task(json!(["home", "panel"]), &stray)) {
        panic!("every start reachable, yet refused: {error}");
    }
    let Err(error) = admit(phased_task(json!(["home", "other"]), &stray)) else {
        panic!("an unreachable start was admitted");
    };
    assert_eq!(error.code().as_str(), "task_phases_unreachable", "{error}");
}
