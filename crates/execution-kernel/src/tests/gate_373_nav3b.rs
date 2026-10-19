// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #373 NAV-3b (model M-373 v1.1 §4.1; test rows N3b-1 and N3b-2): a page-graph
//! task's `entry_page` may be a set of 1–16 pages. Admission refuses a set that breaks the shape
//! with `contained_task_entry_pages_invalid` and its `entry_problem`; in a phased route each
//! page of the set starts the one stage that lists an operation from it, and the route must be
//! reachable from every such start. The packages are synthetic `neutral` packages admitted
//! through `PreparedContainedTask::load`.

use crate::{ContainedTaskCode, ContainedTaskError, ExecutionKernelCode, PreparedContainedTask};
use actingcommand_contract::codes::TaskflowCode;
use actingcommand_contract::outcome::Category;
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
    PreparedContainedTask::load("neutral.instance", &package(task))
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
    for (task, problem, detail) in refused_entry_sets() {
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
        assert_eq!(error.detail(), Some(detail.as_str()), "{problem}");
    }
}

/// The refused sets of N3b-1, one per `entry_problem`, each with the refusal's developer detail.
/// #411 A17: the detail of a refused stage start also names the entry page as admission resolved
/// it and, when more than one stage starts that page, those stages in declaration order.
fn refused_entry_sets() -> Vec<(Value, &'static str, String)> {
    let mut seventeen = page_colors()
        .into_iter()
        .map(|(page, _)| page)
        .collect::<Vec<_>>();
    seventeen.push("spare13".to_owned());
    let bare = |problem: &str| format!("entry_problem={problem}");
    vec![
        (unphased_task(json!([])), "empty", bare("empty")),
        (
            unphased_task(json!(seventeen)),
            "too_many",
            bare("too_many"),
        ),
        (
            unphased_task(json!(["home", "neutral/home"])),
            "duplicate",
            bare("duplicate"),
        ),
        (
            unphased_task(json!(["home", "any"])),
            "any_in_set",
            bare("any_in_set"),
        ),
        (
            unphased_task(json!(["home", "missing"])),
            "unresolved",
            bare("unresolved"),
        ),
        (
            phased_task(json!(["home", "other"]), &[]),
            "phase_start_missing",
            "entry_problem=phase_start_missing page_id=neutral/other".to_owned(),
        ),
        (
            phased_task(
                json!(["home", "panel"]),
                &[("panel_back", "panel", "home", 2)],
            ),
            "phase_start_ambiguous",
            "entry_problem=phase_start_ambiguous page_id=neutral/panel phase_ids=business,return"
                .to_owned(),
        ),
    ]
}

/// The values of a refusal's outcome.
fn values(error: &ContainedTaskError) -> Value {
    Value::Object(error.outcome().values().clone())
}

/// #411 A17 (requirement 1): an entry page no stage's operation leaves from is refused with
/// `entry_problem` `phase_start_missing` and that page, as admission resolved it, as `page_id`.
#[test]
fn a17_a_phase_start_missing_refusal_names_its_entry_page() {
    let Err(error) = admit(phased_task(json!(["home", "other"]), &[])) else {
        panic!("a page that starts no stage was admitted");
    };
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::ContainedTaskEntryPagesInvalid.as_str(),
        "{error}"
    );
    assert_eq!(
        values(&error),
        json!({"entry_problem": "phase_start_missing", "page_id": "neutral/other"})
    );
}

/// #411 A17 (requirements 1 and 2): an entry page that operations of two stages leave from is
/// refused with `entry_problem` `phase_start_ambiguous` and the resolved page as `page_id`; the
/// developer detail names the page and both stages in declaration order.
#[test]
fn a17_a_phase_start_ambiguous_refusal_names_its_entry_page_and_stages() {
    let task = phased_task(
        json!(["home", "panel"]),
        &[("panel_back", "panel", "home", 2)],
    );
    let Err(error) = admit(task) else {
        panic!("a page that starts two stages was admitted");
    };
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::ContainedTaskEntryPagesInvalid.as_str(),
        "{error}"
    );
    assert_eq!(
        values(&error),
        json!({"entry_problem": "phase_start_ambiguous", "page_id": "neutral/panel"})
    );
    assert_eq!(
        error.detail(),
        Some("entry_problem=phase_start_ambiguous page_id=neutral/panel phase_ids=business,return")
    );
}

/// #411 A17 (requirement 1): every refused entry page set carries `entry_problem`, which the
/// catalog marks required, as a value of the refusal.
#[test]
fn a17_every_entry_problem_is_a_value_of_the_refusal() {
    for (task, problem, _) in refused_entry_sets() {
        let Err(error) = admit(task) else {
            panic!("{problem}: refused set admitted");
        };
        assert!(error.has_values(), "{problem}: the refusal has no values");
        assert_eq!(
            error.outcome().value("entry_problem"),
            Some(&json!(problem)),
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

/// #411 A20 (requirements 1–4): a stage that reaches a page from which only another stage has
/// an operation is refused `task_phases_unreachable` naming the stage (`json_pointer`), the page
/// it cannot leave (`page_id`) and the operation whose destination led there
/// (`pack_operation_id`); the developer detail adds the stage id and the start.
#[test]
fn a20_an_unreachable_stage_names_the_stage_the_page_and_the_operation_that_led_there() {
    // `other` starts the second stage, whose only operation from it leads back to `home`,
    // where the second stage has no operation.
    let task = phased_task(json!(["home", "other"]), &[("stray", "other", "home", 1)]);
    let Err(error) = admit(task) else {
        panic!("an unreachable start was admitted");
    };
    assert_eq!(
        error.code().as_str(),
        TaskflowCode::TaskPhasesUnreachable.as_str(),
        "{error}"
    );
    assert_eq!(
        values(&error),
        json!({"json_pointer": "/phases/1", "page_id": "home", "pack_operation_id": "stray"})
    );
    assert_eq!(
        error.detail(),
        Some(
            "phase_problem=no_operation json_pointer=/phases/1 phase_id=business page_id=home \
             pack_operation_id=stray entry_page=other"
        )
    );
}

/// #411 A20 (requirements 1, 2 and 4): a stage that lists an operation the task does not have
/// is refused `task_phases_invalid` naming that member of the stage's operations
/// (`json_pointer`) and the operation (`pack_operation_id`), with no page; the developer detail
/// adds the rule and the stage id.
#[test]
fn a20_a_stage_listing_an_operation_the_task_lacks_names_it() {
    let mut task = phased_task(json!("home"), &[]);
    task["phases"][1]["operations"]
        .as_array_mut()
        .expect("stage operations")
        .push(json!("missing"));
    let Err(error) = admit(task) else {
        panic!("a stage listing an operation the task lacks was admitted");
    };
    assert_eq!(
        error.code().as_str(),
        TaskflowCode::TaskPhasesInvalid.as_str(),
        "{error}"
    );
    assert_eq!(
        values(&error),
        json!({"json_pointer": "/phases/1/operations/1", "pack_operation_id": "missing"})
    );
    assert_eq!(
        error.detail(),
        Some(
            "phase_problem=operation_unknown json_pointer=/phases/1/operations/1 \
             phase_id=business pack_operation_id=missing"
        )
    );
}

/// #411 A20 (requirements 1, 2 and 4): a route whose return stage loops between two pages it
/// does not target reaches no terminal from its start; the refusal `task_phases_unreachable`
/// names the start's stage (`json_pointer`) and its entry page (`page_id`), with no operation.
#[test]
fn a20_a_start_from_which_no_terminal_is_reached_names_its_stage_and_page() {
    let mut task = phased_task(
        json!("home"),
        &[
            ("back_panel", "result", "panel", 2),
            ("reopen", "panel", "result", 2),
        ],
    );
    // `back_home` stays declared, in no stage.
    task["phases"][2]["operations"] = json!(["back_panel", "reopen"]);
    let Err(error) = admit(task) else {
        panic!("a route without a terminal was admitted");
    };
    assert_eq!(
        error.code().as_str(),
        TaskflowCode::TaskPhasesUnreachable.as_str(),
        "{error}"
    );
    assert_eq!(
        values(&error),
        json!({"json_pointer": "/phases/0", "page_id": "home"})
    );
    assert_eq!(
        error.detail(),
        Some("phase_problem=no_terminal json_pointer=/phases/0 phase_id=depart page_id=home")
    );
}
