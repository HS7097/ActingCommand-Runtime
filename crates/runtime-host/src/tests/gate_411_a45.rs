// SPDX-License-Identifier: AGPL-3.0-only

//! #411 A45 (0.12.8), T1: an application entry return-home package that entry recovery runs
//! must not end with `contained_task_state_invalid`. On the production host (10-21, v0.12.7) a
//! manual run of a page-graph package whose entry is a home page met another screen; its entry
//! recovery ran the standard pack's return-home package (`role: return_home`, entry `any`, R24),
//! which ended `contained_task_state_invalid` with no step, and the run failed with it.
//!
//! The path, pinned here: the return-home package does not take an entry gate (it has no entry
//! to check) and runs from the current screen, as a top-level run of it does. The screen is a
//! page the package declares as an error page and that no operation of it leaves. The kernel's
//! run transition refuses to continue from such a page, and that refusal was reported as
//! `contained_task_state_invalid` (`contained_task/candidates.rs`, `candidate_directive`). The
//! code the run must end with names what happened: the package reached its declared error page,
//! `contained_task_error_page_reached`. Requirements of the slice brief:
//!
//! - requirement 1: the nested entry-recovery run of an application entry return-home package;
//!   row `registry return-home layer` is the production path (a game root without navigation
//!   data, the registry's return-home package as the home entry's recovery), row `navigation
//!   data present` the same package as the recovery chain's step of a game with navigation data;
//! - requirement 2: in both rows the application entry package takes no entry check of its own
//!   (the run checks only its own entry) and runs from the current screen;
//! - requirement 3: the run and the layer end with the specific registered code, and no record
//!   of the run carries `contained_task_state_invalid`.
//!
//! A synthetic `neutral` game; the fake device's first pixel is the screen (red `away`, blue
//! `menu`, green `shop`, white `done`) and the screen shows `shop` throughout. Existing tests
//! missed it: every entry-recovery test starts its return-home package on a page it has an
//! operation from, and the error page check had a test only after a step's input
//! (`contained_task/gate_382_e3.rs`); no test let a page-graph run observe a declared error page
//! that no operation leaves.

use super::gate_373_nav3a::{ALIAS, Run, StandardRoot, screens, start};
use super::*;
use actingcommand_contract::TaskEntryRecoveryRole;
use actingcommand_contract::codes::SharedCode;
use actingcommand_execution_kernel::ExecutionKernelCode;
use serde_json::{Value, json};

const AWAY_SCREEN: [u8; 3] = [255, 0, 0];
const MENU_SCREEN: [u8; 3] = [0, 0, 255];
const SHOP_SCREEN: [u8; 3] = [0, 255, 0];
const DONE_SCREEN: [u8; 3] = [255, 255, 255];

const MAIN: &str = "neutral.test.main";
const RETURN_HOME: &str = "neutral.test.return_home";
const FEATURES: &str = "neutral.test.features";

// The run's own entry, as the run records it.
const MAIN_ENTRY: &str = "neutral/menu";

fn colour(page: &str) -> [u8; 3] {
    match page {
        "away" => AWAY_SCREEN,
        "menu" => MENU_SCREEN,
        "shop" => SHOP_SCREEN,
        "done" => DONE_SCREEN,
        other => panic!("no screen {other}"),
    }
}

// A package's control and its one task (schema `0.9`, control `Lab-1y.control.v2`): its entry
// (a page, a set of pages or `any`), its target, one click per operation `(id, from, to)` and
// its error pages; its pages are the ones these name, each by its first pixel.
struct Package<'a> {
    package_id: &'a str,
    mode: &'a str,
    entry: Value,
    target: &'a str,
    operations: &'a [(&'a str, &'a str, &'a str)],
    error_pages: &'a [&'a str],
}

impl Package<'_> {
    fn write(&self, directory: &Path) {
        let mut pages = BTreeSet::new();
        match &self.entry {
            Value::String(page) => {
                pages.insert(page.clone());
            }
            Value::Array(entries) => {
                pages.extend(entries.iter().filter_map(Value::as_str).map(str::to_owned));
            }
            _ => {}
        }
        pages.insert(self.target.to_owned());
        for (_, from, to) in self.operations {
            pages.insert((*from).to_owned());
            pages.insert((*to).to_owned());
        }
        pages.extend(self.error_pages.iter().map(|page| (*page).to_owned()));
        // An application entry names no page.
        pages.remove("any");
        let control = json!({
            "schema_version": "Lab-1y.control.v2",
            "package_id": self.package_id,
            "execution_mode": self.mode,
            "game": "neutral",
            "server": "test",
            "resolution": {"width": 16, "height": 9},
            "entry_task_id": "task",
            "capture_interval_ms": 10,
            "step_timeout_ms": 600,
            "timeout_ms": 8000,
            "max_steps": 2
        });
        let mut task = json!({
            "schema_version": "0.9",
            "task_id": "task",
            "game": "neutral",
            "server_scope": ["test"],
            "locale": "en-US",
            "goal": "A45 fixture",
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
        if !self.error_pages.is_empty() {
            task["error_pages"] = json!(self.error_pages);
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

// The run's own package: a page graph from its entry set {menu} to `done`.
fn main_package() -> Package<'static> {
    Package {
        package_id: MAIN,
        mode: "navigable_route",
        entry: json!(["menu"]),
        target: "done",
        operations: &[("open", "menu", "done")],
        error_pages: &[],
    }
}

// The return-home package: an application entry (`any`) that leaves `away` for `menu`, the
// run's entry, and declares `shop` its error page, which no operation of it leaves.
fn return_home_package() -> Package<'static> {
    Package {
        package_id: RETURN_HOME,
        mode: "navigable_route",
        entry: json!("any"),
        target: "menu",
        operations: &[("back", "away", "menu")],
        error_pages: &["shop"],
    }
}

// The feature package of `pages.json`: a `recognize_only` package with exactly its pages.
fn features_package() -> Package<'static> {
    Package {
        package_id: FEATURES,
        mode: "recognize_only",
        entry: json!("menu"),
        target: "menu",
        operations: &[("see_shop", "shop", "menu")],
        error_pages: &[],
    }
}

// The game root: the return-home package, named the server's return-home package; with
// `navigation`, also the feature package, `pages.json` ({menu, shop}) and `navigation.json` (no
// edges, the return-home package the recovery chain's one step).
fn game_root(base: &Path, navigation: bool) -> StandardRoot {
    let mut root = StandardRoot::new(base);
    return_home_package().write(&root.list(RETURN_HOME));
    root.return_home(RETURN_HOME);
    if navigation {
        features_package().write(&root.list(FEATURES));
        let game = root.root.join("neutral");
        for (file, value) in [
            (
                "pages.json",
                json!({
                    "schema_version": "actingcommand.pages.v1",
                    "game": "neutral",
                    "features": [FEATURES],
                    "pages": [
                        {"id": "menu", "kind": "page"},
                        {"id": "shop", "kind": "page"}
                    ]
                }),
            ),
            (
                "navigation.json",
                json!({
                    "schema_version": "actingcommand.navigation.v1",
                    "game": "neutral",
                    "edges": [],
                    "recovery_chain": [RETURN_HOME]
                }),
            ),
        ] {
            fs::write(
                game.join(file),
                serde_json::to_vec_pretty(&value).expect("root file"),
            )
            .expect("write root file");
        }
    }
    root.write_bundle();
    root
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

// T1: in both rows the run starts on `shop`. Its entry {menu} does not pass; the application
// entry return-home package is admitted as a layer (`role` of the row) and runs from the screen
// as it is, without an entry check of its own; it observes `shop`, its declared error page that
// no operation of it leaves, and ends with `contained_task_error_page_reached`, before any input;
// the run ends with that code; no record of the run carries `contained_task_state_invalid`.
#[test]
fn a45_an_application_entry_return_home_layer_runs_and_never_ends_state_invalid() {
    let error_page_reached = ExecutionKernelCode::ContainedTaskErrorPageReached.as_str();
    let state_invalid = SharedCode::ContainedTaskStateInvalid.as_str();
    for (row, navigation, role) in [
        (
            "registry return-home layer",
            false,
            TaskEntryRecoveryRole::ReturnHome,
        ),
        (
            "navigation data present",
            true,
            TaskEntryRecoveryRole::RecoveryChain,
        ),
    ] {
        let state = Arc::new(FakeState::default());
        screens(&state, vec![SHOP_SCREEN]);
        let temp = TempDir::new().expect("tempdir");
        let root = game_root(temp.path(), navigation);
        let host = start(&temp, &state, Some(root.root.clone()));

        let run = run_directly(&host, &temp, &main_package());
        host.close().expect("close runtime host");

        assert_eq!(
            run.receipt.state(),
            RuntimeReceiptState::Failed,
            "{row}: {:?}",
            run.receipt
        );
        let facts = run.facts();
        // Requirement 1: the return-home package runs as the run's one layer.
        let admitted = facts
            .iter()
            .filter_map(|fact| match fact {
                TaskSemanticFact::EntryRecoveryPackageAdmitted {
                    package_label,
                    role,
                } => Some((package_label.clone(), *role)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            admitted,
            vec![(RETURN_HOME.to_owned(), Some(role))],
            "{row}"
        );
        // Requirement 3: the layer ends with the specific code.
        let failed = facts
            .iter()
            .filter_map(|fact| match fact {
                TaskSemanticFact::EntryRecoveryFailed {
                    package_label,
                    outcome,
                    role,
                } => Some((
                    package_label.clone(),
                    outcome.code().as_str().to_owned(),
                    *role,
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            failed,
            vec![(
                Some(RETURN_HOME.to_owned()),
                error_page_reached.to_owned(),
                Some(role)
            )],
            "{row}"
        );
        let terminal = facts
            .iter()
            .find_map(|fact| match fact {
                TaskSemanticFact::TerminalCommitted {
                    outcome: Some(outcome),
                    ..
                } => Some(outcome.code().as_str().to_owned()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{row}: the run's failed terminal"));
        assert_eq!(terminal, error_page_reached, "{row}");
        // Requirement 2: only the run's own entry is checked; the application entry package
        // takes no entry check of its own.
        let recognitions = facts
            .iter()
            .filter_map(|fact| match fact {
                TaskSemanticFact::EntryRecognition {
                    phase,
                    required_pages,
                    ..
                } => Some((*phase, required_pages.clone())),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            recognitions,
            vec![(
                TaskEntryRecognitionPhase::Initial,
                vec![MAIN_ENTRY.to_owned()]
            )],
            "{row}"
        );
        assert_eq!(
            state.input_count.load(Ordering::Acquire),
            0,
            "{row}: nothing is input from the error page"
        );
        for event in &run.events {
            let payload = serde_json::to_string(event.payload()).expect("payload JSON");
            assert!(
                !payload.contains(state_invalid),
                "{row}: no record carries {state_invalid}: {payload}"
            );
        }
    }
}
