// SPDX-License-Identifier: AGPL-3.0-only

//! #433 RH-1 (board #433 A2; register §1.269, since 0.12.39): a request's own entry recovery
//! package is judged as the configured return-home package is. Its `scheduling_outcome` without a
//! designated operation is allowed and ignored on both paths; one with a designated operation is
//! still refused on both, each path with its own code (`contracts/linear-steps.md`, "Ladder and
//! page-graph home entry").
//!
//! The packages are NAV-3a's (`gate_373_nav3a`: the fake device's first pixel is the screen, red
//! `away`, blue `menu`, white `done`). The run's own package is a page graph whose entry is the
//! set {menu}, from `menu` to `done`; it declares no `scheduling_outcome`. The recovery package is
//! listed in the game root as its return-home package and leads from `away` to `menu` in one
//! click (`go`); its `scheduling_outcome` maps `menu`. Each run starts on `away` on a fresh host,
//! so its home entry needs the recovery package: the request binds it (the entry recovery path)
//! or binds nothing (the registry's, the configured path).

use super::gate_373_nav3a::{
    ALIAS, AWAY, DONE, LEAD_STEPS, MAIN, MENU, OPEN, Pack, RETURN_HOME, StandardRoot, route,
    screens, start,
};
use super::*;
use actingcommand_contract::TaskEntryRecoveryRole;
use serde_json::{Value, json};

/// The recovery package's key of its `no_designated_effect` mapping (`menu`).
const RETURNED: &str = "rh1_recovery_returned";
/// The recovery package's key of its `designated_effect_completed` mapping, when `go` is its
/// designated operation.
const OPENED: &str = "rh1_recovery_opened";
/// A request's own recovery package the home entry cannot use (unchanged).
const ENTRY_INCOMPATIBLE: &str = "contained_task_home_recovery_package_incompatible";
/// The configured return-home package that fails a return-home layer's check (unchanged).
const CONFIGURED_INCOMPATIBLE: &str = "contained_task_prerequisite_incompatible";

struct DirectRun {
    receipt: RuntimeReceipt,
    events: Vec<ProjectedEvent>,
    /// The inputs the fake device received during the run.
    inputs: usize,
}

impl DirectRun {
    fn facts(&self) -> Vec<&TaskSemanticFact> {
        self.events
            .iter()
            .filter_map(projected_task_semantic_fact)
            .collect()
    }

    fn succeeded(&self) -> bool {
        self.receipt.state() == RuntimeReceiptState::Completed
            && matches!(
                self.receipt.result(),
                Some(RuntimeResult::ContainedTaskCompleted {
                    task_result: TaskOutcome::Success,
                    ..
                })
            )
    }

    /// The recovery packages the home entry admitted, with their role.
    fn admitted(&self) -> Vec<(String, Option<TaskEntryRecoveryRole>)> {
        self.facts()
            .into_iter()
            .filter_map(|fact| match fact {
                TaskSemanticFact::EntryRecoveryPackageAdmitted {
                    package_label,
                    role,
                    ..
                } => Some((package_label.clone(), *role)),
                _ => None,
            })
            .collect()
    }

    /// The recovery packages that completed, with the page each ended on.
    fn completed(&self) -> Vec<(String, String)> {
        self.facts()
            .into_iter()
            .filter_map(|fact| match fact {
                TaskSemanticFact::EntryRecoveryCompleted {
                    package_label,
                    final_page,
                    ..
                } => Some((package_label.clone(), final_page.clone())),
                _ => None,
            })
            .collect()
    }

    /// The code the home entry closed with, when it failed closed.
    fn fail_closed(&self) -> Option<String> {
        self.facts().into_iter().find_map(|fact| match fact {
            TaskSemanticFact::EntryTargetDisposition {
                disposition: TaskEntryTargetDisposition::FailClosed,
                outcome: Some(outcome),
            } => Some(outcome.code().as_str().to_owned()),
            _ => None,
        })
    }
}

/// The recovery package `RETURN_HOME` at `directory`: NAV-3a's page graph from `away` to `menu`
/// (`go`), whose `scheduling_outcome` maps `menu` without a designated operation, or, when
/// `designated`, with `go` as its designated operation.
fn write_recovery(directory: &Path, designated: bool) {
    route(RETURN_HOME, "away", "menu", LEAD_STEPS, None).write(directory);
    let path = directory.join("resources/operations/task/task.json");
    let mut task: Value =
        serde_json::from_slice(&fs::read(&path).expect("read recovery task")).expect("task JSON");
    task["scheduling_outcome"] = if designated {
        json!({"designated_operation": "go", "mappings": [
            {"outcome_key": OPENED, "effect": "designated_effect_completed",
             "terminal_pages": ["menu"]},
            {"outcome_key": RETURNED, "effect": "no_designated_effect",
             "terminal_pages": ["menu"]}
        ]})
    } else {
        json!({"mappings": [
            {"outcome_key": RETURNED, "effect": "no_designated_effect",
             "terminal_pages": ["menu"]}
        ]})
    };
    fs::write(
        &path,
        serde_json::to_vec_pretty(&task).expect("recovery task JSON"),
    )
    .expect("write recovery task");
}

/// One run of the run's own package on a fresh host whose game root lists the recovery package
/// (`designated` as [`write_recovery`]) as its return-home package. With `bind`, the request binds
/// that same package directory as its own recovery package; without, it binds none.
fn run_once(designated: bool, bind: bool) -> DirectRun {
    let temp = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    screens(&state, vec![AWAY, MENU, DONE]);
    let mut root = StandardRoot::new(temp.path());
    let recovery = root.list(RETURN_HOME);
    write_recovery(&recovery, designated);
    root.return_home(RETURN_HOME);
    root.write_bundle();
    let package = temp.path().join("main");
    Pack {
        package_id: MAIN,
        mode: "navigable_route",
        prerequisite: None,
        entry: json!(["menu"]),
        target: "done",
        operations: OPEN,
    }
    .write(&package);
    let host = start(&temp, &state, Some(root.root.clone()));

    let mut task_request =
        ContainedTaskRequest::new(package.display().to_string()).expect("task request");
    if bind {
        task_request = task_request
            .with_recovery(
                ContainedTaskRecoveryBinding::new(recovery.display().to_string())
                    .expect("recovery binding"),
            )
            .expect("task request with recovery");
    }
    let mut client = TestClient::connect(&host);
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
    let events = projected_events(
        &mut client,
        EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        },
    );
    drop(client);
    host.close().expect("close runtime host");
    DirectRun {
        receipt,
        events,
        inputs: state.input_count.load(Ordering::Acquire),
    }
}

/// The request's own recovery package declares a `scheduling_outcome` without a designated
/// operation: the home entry admits it (no role: the request's binding), it runs and ends on
/// `menu`, the entry passes and the run's own package runs to `done`. Two inputs: its `go` and
/// the run's `open`.
#[test]
fn entry_recovery_accepts_a_scheduling_outcome_without_a_designated_operation() {
    let run = run_once(false, true);

    assert!(run.succeeded(), "{:?}", run.receipt);
    assert_eq!(run.admitted(), vec![(RETURN_HOME.to_owned(), None)]);
    assert_eq!(
        run.completed(),
        vec![(RETURN_HOME.to_owned(), "neutral/menu".to_owned())]
    );
    assert_eq!(run.fail_closed(), None);
    assert_eq!(run.inputs, 2);
}

/// The request's own recovery package declares a `scheduling_outcome` whose designated operation
/// is `go`: it is refused before it runs with `contained_task_home_recovery_package_incompatible`,
/// as before: no input, no `EntryRecoveryPackageAdmitted`, the entry failed closed with the code
/// and the run's terminal carrying it.
#[test]
fn entry_recovery_still_refuses_a_designated_scheduling_outcome() {
    let run = run_once(true, true);

    assert_eq!(
        run.receipt.state(),
        RuntimeReceiptState::Failed,
        "{:?}",
        run.receipt
    );
    assert_eq!(run.inputs, 0, "no input");
    assert!(run.admitted().is_empty(), "{:?}", run.admitted());
    assert!(run.completed().is_empty(), "{:?}", run.completed());
    assert_eq!(run.fail_closed().as_deref(), Some(ENTRY_INCOMPATIBLE));
    let terminal = run
        .facts()
        .into_iter()
        .find_map(|fact| match fact {
            TaskSemanticFact::TerminalCommitted {
                task_result,
                outcome: Some(outcome),
                ..
            } => Some((*task_result, outcome.code().as_str().to_owned())),
            _ => None,
        })
        .expect("the run's failed terminal");
    assert_eq!(
        terminal,
        (TaskOutcome::Failure, ENTRY_INCOMPATIBLE.to_owned())
    );
}

/// One package, on both paths: bound by the request (the entry recovery path) and named by the
/// game root with no binding (the configured return-home path). Without a designated operation
/// both admit it, run it to the same page with the same inputs and run the package after it;
/// with one, both refuse it before any input and before it is admitted, each with its own code.
#[test]
fn entry_recovery_and_configured_return_home_judge_one_package_alike() {
    for designated in [false, true] {
        let entry = run_once(designated, true);
        let configured = run_once(designated, false);

        for (path, run) in [("entry recovery", &entry), ("configured", &configured)] {
            assert_eq!(
                run.succeeded(),
                !designated,
                "{path}, designated {designated}: {:?}",
                run.receipt
            );
        }
        assert_eq!(
            entry.completed(),
            configured.completed(),
            "designated {designated}"
        );
        assert_eq!(entry.inputs, configured.inputs, "designated {designated}");
        assert_eq!(
            (entry.admitted().len(), configured.admitted().len()),
            if designated { (0, 0) } else { (1, 1) },
            "designated {designated}"
        );
        if designated {
            assert_eq!(entry.inputs, 0);
            assert_eq!(entry.fail_closed().as_deref(), Some(ENTRY_INCOMPATIBLE));
            assert_eq!(
                configured.fail_closed().as_deref(),
                Some(CONFIGURED_INCOMPATIBLE)
            );
        } else {
            assert_eq!(entry.inputs, 2);
            assert_eq!(
                entry.completed(),
                vec![(RETURN_HOME.to_owned(), "neutral/menu".to_owned())]
            );
            assert_eq!(entry.admitted(), vec![(RETURN_HOME.to_owned(), None)]);
            assert_eq!(
                configured.admitted(),
                vec![(
                    RETURN_HOME.to_owned(),
                    Some(TaskEntryRecoveryRole::ReturnHome)
                )]
            );
        }
    }
}

/// While the request's own recovery package runs as an entry recovery, nothing of its
/// `scheduling_outcome` is mapped or recorded: the run's terminal carries no scheduling
/// disposition (the run's own package declares no `scheduling_outcome`), and the recovery
/// package's outcome key is in no record of the run nor in its receipt.
#[test]
fn entry_recovery_ignores_the_recovery_packages_scheduling_outcome() {
    let run = run_once(false, true);

    assert!(run.succeeded(), "{:?}", run.receipt);
    assert_eq!(run.completed().len(), 1, "{:?}", run.completed());
    let dispositions = run
        .facts()
        .into_iter()
        .filter_map(|fact| match fact {
            TaskSemanticFact::TerminalCommitted {
                scheduling_disposition,
                ..
            } => Some(scheduling_disposition.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(dispositions, vec![None], "one terminal, nothing mapped");
    let records = serde_json::to_string(&run.events).expect("run records JSON");
    assert!(
        !records.contains(RETURNED),
        "the recovery package's outcome key is recorded"
    );
    assert!(
        !format!("{:?}", run.receipt).contains(RETURNED),
        "the recovery package's outcome key is in the receipt"
    );
}
