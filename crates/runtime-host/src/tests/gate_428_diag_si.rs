// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #428 DIAG-SI (register §1.320; the coordinator's rulings of 2026-11-11, not Alice's
//! words): `contained_task_state_invalid` says what was invalid, also where the host reads it.
//!
//! The incident's shape (register §1.319): a package declares a phased prerequisite package; the
//! screen shows a page of the prerequisite package that is outside its entry page set and that its
//! first stage has no operation from (a later stage has one). The prerequisite layer starts at its
//! first stage there and ends at once with `contained_task_state_invalid`; the gate closes it
//! (`EntryRecoveryFailed`) and itself (`EntryTargetDisposition { FailClosed }`), and the run fails
//! with that code. Required, besides the code, its category and the receipt state of before: the
//! layer's records and the run's terminal carry `state_problem` `no_move_from_page`, the page
//! (`page_id`) and the stage (`phase_id`); nothing is input.
//!
//! A synthetic `neutral` game; the fake device's first pixel is the screen (red `away`, blue
//! `menu`, green `busy`, yellow `free`, white `done`) and the screen shows `busy` throughout.

use super::gate_373_nav3a::{ALIAS, Run, StandardRoot, screens, start};
use super::*;
use actingcommand_contract::TaskEntryRecoveryRole;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{Category, Outcome, StateProblem};
use serde_json::{Value, json};

const BUSY_SCREEN: [u8; 3] = [0, 255, 0];

const MAIN: &str = "neutral.diag.main";
const LEAD: &str = "neutral.diag.lead";

fn colour(page: &str) -> [u8; 3] {
    match page {
        "away" => [255, 0, 0],
        "menu" => [0, 0, 255],
        "busy" => BUSY_SCREEN,
        "free" => [255, 255, 0],
        "done" => [255, 255, 255],
        other => panic!("no screen {other}"),
    }
}

// A `navigable_route` package's control and its one task (schema `0.9`, control
// `Lab-1y.control.v2`): its entry, its target, one click per operation `(id, from, to)`, its
// prerequisite package and its stages `(id, operations, target pages)`, none for a route without
// stages; a phased task maps its target under `no_designated_effect`. Its pages are the ones these
// name, each by its first pixel.
struct Package<'a> {
    package_id: &'a str,
    prerequisite: Option<&'a str>,
    entry: Value,
    target: &'a str,
    operations: &'a [(&'a str, &'a str, &'a str)],
    stages: &'a [(&'a str, &'a [&'a str], &'a [&'a str])],
}

impl Package<'_> {
    fn write(&self, directory: &Path) {
        let mut pages = BTreeSet::new();
        if let Some(entry) = self.entry.as_str() {
            pages.insert(entry.to_owned());
        }
        for entry in self.entry.as_array().into_iter().flatten() {
            pages.insert(entry.as_str().expect("entry page").to_owned());
        }
        pages.insert(self.target.to_owned());
        for (_, from, to) in self.operations {
            pages.insert((*from).to_owned());
            pages.insert((*to).to_owned());
        }
        let mut control = json!({
            "schema_version": "Lab-1y.control.v2",
            "package_id": self.package_id,
            "execution_mode": "navigable_route",
            "game": "neutral",
            "server": "test",
            "resolution": {"width": 16, "height": 9},
            "entry_task_id": "task",
            "capture_interval_ms": 10,
            "step_timeout_ms": 600,
            "timeout_ms": 8000,
            "max_steps": 2
        });
        if let Some(prerequisite) = self.prerequisite {
            control["prerequisite_package_id"] = json!(prerequisite);
        }
        let mut task = json!({
            "schema_version": "0.9",
            "task_id": "task",
            "game": "neutral",
            "server_scope": ["test"],
            "locale": "en-US",
            "goal": "DIAG-SI fixture",
            "coordinate_space": {"width": 16, "height": 9},
            "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
            "color_probes": pages
                .iter()
                .map(|page| json!({
                    "id": format!("first_{page}"),
                    "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
                    "expected": colour(page)
                }))
                .collect::<Vec<_>>(),
            "page_rules": pages
                .iter()
                .map(|page| (page.clone(), json!({"required": [format!("first_{page}")]})))
                .collect::<serde_json::Map<_, _>>(),
            "entry_page": self.entry,
            "target_page": self.target,
            "operations": self
                .operations
                .iter()
                .map(|(id, from, to)| json!({
                    "id": id,
                    "from": from,
                    "to": to,
                    "click": {"kind": "point", "x": 1, "y": 0},
                    "unguarded_trusted_coordinate": true,
                    "retryable": false
                }))
                .collect::<Vec<_>>()
        });
        if !self.stages.is_empty() {
            let phases = self
                .stages
                .iter()
                .map(|(id, operations, targets)| {
                    json!({"id": id, "operations": operations, "target_pages": targets})
                })
                .collect::<Vec<_>>();
            task["phases"] = json!(phases);
            control["phases"] = json!(phases);
            task["scheduling_outcome"] = json!({"mappings": [{
                "outcome_key": "reached",
                "effect": "no_designated_effect",
                "terminal_pages": [self.target]
            }]});
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
            fs::create_dir_all(path.parent().expect("package file parent")).expect("package dir");
            fs::write(
                path,
                serde_json::to_vec_pretty(&value).expect("package JSON"),
            )
            .expect("write package file");
        }
    }
}

// The run's own package: from its entry set {menu} to `done`, with the prerequisite package
// `LEAD`.
fn main_package() -> Package<'static> {
    Package {
        package_id: MAIN,
        prerequisite: Some(LEAD),
        entry: json!(["menu"]),
        target: "done",
        operations: &[("open", "menu", "done")],
        stages: &[],
    }
}

// The prerequisite package: two stages from its entry page `away` to `menu`. `first` leaves
// `away` for `free`; `second` leaves `free`, or `busy`, for `menu`. `first` has no operation from
// `busy` and does not target it, and `busy` is no entry page.
fn lead_package() -> Package<'static> {
    Package {
        package_id: LEAD,
        prerequisite: None,
        entry: json!("away"),
        target: "menu",
        operations: &[
            ("go", "away", "free"),
            ("enter", "free", "menu"),
            ("leave", "busy", "menu"),
        ],
        stages: &[
            ("first", &["go"], &["free"]),
            ("second", &["enter", "leave"], &["menu"]),
        ],
    }
}

// Runs `package`, written at `<temp>/main`, directly.
fn run_directly(host: &RuntimeHost, temp: &TempDir, package: &Package<'_>) -> Run {
    let path = temp.path().join("main");
    package.write(&path);
    let task_request = ContainedTaskRequest::new(path.display().to_string()).expect("task request");
    let mut client = TestClient::connect(host);
    client.set_receipt_read_timeout();
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            ALIAS,
            client.ids.mint_holder_id().expect("holder"),
            task_request,
        ),
    );
    let receipt = client.send(&request);
    drop(client);
    let events = host
        .query_persisted_events_for_test(EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        })
        .expect("run events");
    Run { receipt, events }
}

// The outcome carries the code, category error, `no_move_from_page`, the page `busy` and the
// stage `first` of the prerequisite package.
fn assert_names_the_page_and_the_stage(place: &str, outcome: &Outcome) {
    assert_eq!(
        outcome.code().as_str(),
        SharedCode::ContainedTaskStateInvalid.as_str(),
        "{place}: {outcome:?}"
    );
    assert_eq!(outcome.category(), Category::Error, "{place}");
    let text = |key: &str| outcome.value(key).and_then(Value::as_str);
    assert_eq!(
        text("state_problem"),
        Some(StateProblem::NoMoveFromPage.as_str()),
        "{place}: {outcome:?}"
    );
    assert_eq!(
        text("page_id"),
        Some("neutral/busy"),
        "{place}: {outcome:?}"
    );
    assert_eq!(text("phase_id"), Some("first"), "{place}: {outcome:?}");
}

#[test]
fn diag_si_a_prerequisite_layer_without_a_move_names_the_page_and_the_stage_in_its_records() {
    let state = Arc::new(FakeState::default());
    screens(&state, vec![BUSY_SCREEN]);
    let temp = TempDir::new().expect("tempdir");
    let mut root = StandardRoot::new(temp.path());
    lead_package().write(&root.list(LEAD));
    root.write_bundle();
    let host = start(&temp, &state, Some(root.root.clone()));

    let run = run_directly(&host, &temp, &main_package());
    host.close().expect("close runtime host");

    // The run ends as before: failed, with the code, before any input.
    assert_eq!(
        run.receipt.state(),
        RuntimeReceiptState::Failed,
        "{:?}",
        run.receipt
    );
    assert_eq!(
        run.receipt.outcome().code().as_str(),
        SharedCode::ContainedTaskStateInvalid.as_str(),
        "{:?}",
        run.receipt
    );
    assert_eq!(
        state.input_count.load(Ordering::Acquire),
        0,
        "nothing input"
    );
    let facts = run.facts();
    let admitted = facts
        .iter()
        .filter_map(|fact| match fact {
            TaskSemanticFact::EntryRecoveryPackageAdmitted {
                package_label,
                role,
                ..
            } => Some((package_label.clone(), *role)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        admitted,
        vec![(LEAD.to_owned(), Some(TaskEntryRecoveryRole::Prerequisite))]
    );
    // The layer's `task.entry_preflight` records: its failure and the gate's fail-closed end.
    let failed = facts
        .iter()
        .filter_map(|fact| match fact {
            TaskSemanticFact::EntryRecoveryFailed {
                package_label,
                outcome,
                role,
            } => Some((package_label.clone(), outcome, *role)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(failed.len(), 1, "one layer failed: {failed:?}");
    let (package_label, outcome, role) = &failed[0];
    assert_eq!(package_label.as_deref(), Some(LEAD));
    assert_eq!(*role, Some(TaskEntryRecoveryRole::Prerequisite));
    assert_names_the_page_and_the_stage("entry_recovery_failed", outcome);
    let closed = facts
        .iter()
        .filter_map(|fact| match fact {
            TaskSemanticFact::EntryTargetDisposition {
                disposition: TaskEntryTargetDisposition::FailClosed,
                outcome: Some(outcome),
            } => Some(outcome),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(closed.len(), 1, "the gate closed once: {closed:?}");
    assert_names_the_page_and_the_stage("entry_target_disposition", closed[0]);
    // The run's terminal.
    let terminal = facts
        .iter()
        .find_map(|fact| match fact {
            TaskSemanticFact::TerminalCommitted {
                outcome: Some(outcome),
                ..
            } => Some(outcome),
            _ => None,
        })
        .expect("the run's failed terminal");
    assert_names_the_page_and_the_stage("terminal", terminal);
}
