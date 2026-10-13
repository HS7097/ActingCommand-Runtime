// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #373 NAV-3b (model M-373 v1.1 §4.1 and §8.3; test rows N3b-3, N3b-4, N3b-5, N3b-7
//! and N3b-8): a page-graph package's entry may be a set of pages, or `any`. The host checks a
//! set first on one frame of its pages only; a phased route enters at the stage of the page it
//! matched; an entry recovery package must end on a page of the set, and the recheck accepts any
//! page of it. `EntryRecognition` records carry `required_pages` and `matched_page`.
//!
//! The synthetic `neutral` packages read the fake device's two pixels: the first turns from red
//! to blue on a capture transition, the second from green to blue on the stability-region
//! transition, so the device walks `away` (red, green), `home` (blue, green), `done` (blue,
//! blue).

use super::*;

const ALIAS: &str = "neutral.instance";

/// The page rules over the fake device's two pixels.
const AWAY: &[&str] = &["pixel/first_red"];
const HOME: &[&str] = &["pixel/first_blue", "pixel/second_green"];
const DONE: &[&str] = &["pixel/first_blue", "pixel/second_blue"];
/// The second pixel alone: green on the first frames.
const SECOND_GREEN: &[&str] = &["pixel/second_green"];

fn entry_operation(id: &str, from: &str, to: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "from": from,
        "to": to,
        "click": {"kind": "point", "x": 1, "y": 0},
        "unguarded_trusted_coordinate": true,
        "retryable": false
    })
}

/// A `navigable_route` package around `task` with the named page rules (control v2 for task
/// schema 0.9, v1 otherwise; a phased task's `phases` are copied into the control).
fn entry_package(package_id: &str, task: serde_json::Value, pages: &[(&str, &[&str])]) -> Vec<u8> {
    let mut control = serde_json::json!({
        "schema_version": if task["schema_version"] == "0.9" {
            "Lab-1y.control.v2"
        } else {
            "Lab-1y.control.v1"
        },
        "package_id": package_id,
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 16, "height": 9},
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 200,
        "timeout_ms": 5000,
        "max_steps": 2
    });
    if let Some(phases) = task.get("phases") {
        control["phases"] = phases.clone();
    }
    let recognition = serde_json::json!({
        "schema_version": "0.3",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 16, "height": 9},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            {"type": "color", "id": "pixel/first_red", "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [255, 0, 0]},
            {"type": "color", "id": "pixel/first_blue", "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [0, 0, 255]},
            {"type": "color", "id": "pixel/second_green", "region": {"x": 1, "y": 0, "width": 1, "height": 1}, "expected": [0, 255, 0]},
            {"type": "color", "id": "pixel/second_blue", "region": {"x": 1, "y": 0, "width": 1, "height": 1}, "expected": [0, 0, 255]}
        ]
    });
    let pages = serde_json::json!({
        "schema_version": "0.3",
        "pages": pages
            .iter()
            .map(|(page, required)| serde_json::json!({
                "id": format!("neutral/{page}"),
                "required": required,
                "optional": [],
                "forbidden": []
            }))
            .collect::<Vec<_>>()
    });
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in [
        ("control.json", control),
        (
            "resources/manifest.json",
            serde_json::json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        ("resources/operations/task/task.json", task),
        ("resources/recognition/neutral.test.pack.json", recognition),
        ("resources/recognition/neutral.test.pages.json", pages),
    ] {
        zip.start_file(path, options).expect("zip entry");
        zip.write_all(&serde_json::to_vec(&value).expect("package JSON"))
            .expect("zip content");
    }
    zip.finish().expect("finish zip").into_inner()
}

/// A two-stage route: `rewind` (from `rewind_from`) leads to `home` in the first stage,
/// `finish` leads from `home` to `done` in the second.
fn two_stage_task(entry_page: serde_json::Value, rewind_from: &str) -> serde_json::Value {
    serde_json::json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 16, "height": 9},
        "entry_page": entry_page,
        "phases": [
            {"id": "first", "operations": ["rewind"], "target_pages": ["home"]},
            {"id": "second", "operations": ["finish"], "target_pages": ["done"]}
        ],
        "target_page": "done",
        "scheduling_outcome": {"mappings": [{
            "outcome_key": "finished",
            "effect": "no_designated_effect",
            "terminal_pages": ["done"]
        }]},
        "operations": [
            entry_operation("rewind", rewind_from, "home"),
            entry_operation("finish", "home", "done")
        ]
    })
}

/// An entry recovery package: one step from `away` to `landing` (operation schema 0.6).
fn recovery_package(landing: &str) -> Vec<u8> {
    entry_package(
        "neutral.entry.recovery",
        serde_json::json!({
            "schema_version": "0.6",
            "task_id": "task",
            "game": "neutral",
            "server_scope": ["test"],
            "coordinate_space": {"width": 16, "height": 9},
            "target_page": landing,
            "operations": [entry_operation("go", "away", landing)]
        }),
        &[("away", AWAY), (landing, HOME)],
    )
}

struct EntryRun {
    receipt: RuntimeReceipt,
    events: Vec<ProjectedEvent>,
}

/// Runs `target` once on a fresh host, with `recovery` bound when given.
fn run_entry(state: &Arc<FakeState>, target: &[u8], recovery: Option<&[u8]>) -> EntryRun {
    let root = TempDir::new().expect("tempdir");
    let target_path = root.path().join("entry-target.zip");
    fs::write(&target_path, target).expect("write target package");
    let mut task_request = ContainedTaskRequest::new(
        target_path.display().to_string(),
        format!("{:x}", Sha256::digest(target)),
    )
    .expect("target request");
    if let Some(recovery) = recovery {
        let recovery_path = root.path().join("entry-recovery.zip");
        fs::write(&recovery_path, recovery).expect("write recovery package");
        task_request = task_request
            .with_recovery(
                ContainedTaskRecoveryBinding::new(
                    recovery_path.display().to_string(),
                    format!("{:x}", Sha256::digest(recovery)),
                )
                .expect("recovery binding"),
            )
            .expect("target request with recovery");
    }
    state.physical_task_geometry.store(true, Ordering::Release);
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(ALIAS, instance_id(), Arc::clone(state))),
    )
    .expect("runtime host");
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
    EntryRun { receipt, events }
}

fn task_facts(events: &[ProjectedEvent]) -> Vec<&TaskSemanticFact> {
    events
        .iter()
        .filter_map(projected_task_semantic_fact)
        .collect()
}

/// The started steps of a phased run: operation and stage index.
fn staged_steps(facts: &[&TaskSemanticFact]) -> Vec<(String, u32)> {
    facts
        .iter()
        .filter_map(|fact| match fact {
            TaskSemanticFact::StepStarted {
                operation_label,
                phase: Some(phase),
                ..
            } => Some((operation_label.clone(), phase.index)),
            _ => None,
        })
        .collect()
}

/// N3b-8: every `EntryRecognition` record, as the ledger holds it (serialized and read back
/// unchanged): its phase name, `required_pages` and `matched_page`.
fn entry_recognitions(
    facts: &[&TaskSemanticFact],
) -> Vec<(String, serde_json::Value, serde_json::Value)> {
    facts
        .iter()
        .filter(|fact| matches!(fact, TaskSemanticFact::EntryRecognition { .. }))
        .map(|fact| {
            let value = serde_json::to_value(fact).expect("serialize EntryRecognition");
            let read: TaskSemanticFact =
                serde_json::from_value(value.clone()).expect("read EntryRecognition back");
            assert_eq!(&&read, fact, "the record shape round-trips");
            (
                value["phase"].as_str().expect("phase").to_owned(),
                value["required_pages"].clone(),
                value
                    .get("matched_page")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            )
        })
        .collect()
}

/// The code of the terminal's outcome (ledger revision 3, #378 A3; was its `failure_code`).
fn terminal_failure_code(facts: &[&TaskSemanticFact]) -> Option<String> {
    facts.iter().find_map(|fact| match fact {
        TaskSemanticFact::TerminalCommitted { outcome, .. } => outcome
            .as_ref()
            .map(|outcome| outcome.code().as_str().to_owned()),
        _ => None,
    })
}

/// N3b-3 and N3b-8: entry set {away, home}, `home` starts the second stage, the first frame is
/// `home`: the run starts at the second stage (`StepStarted.phase`), with no recovery package,
/// and its entry records carry the set and the matched page.
#[test]
fn phased_route_enters_at_the_stage_of_its_matched_entry_page_without_recovery() {
    let state = Arc::new(FakeState::default());
    state
        .transition_capture_after_capture
        .store(1, Ordering::Release);
    state
        .stability_region_transition_after_inputs
        .store(1, Ordering::Release);
    let target = entry_package(
        "neutral.entry.target",
        two_stage_task(serde_json::json!(["away", "home"]), "away"),
        &[("away", AWAY), ("home", HOME), ("done", DONE)],
    );
    let run = run_entry(&state, &target, None);
    assert_eq!(run.receipt.state(), RuntimeReceiptState::Completed);
    assert!(matches!(
        run.receipt.result(),
        Some(RuntimeResult::ContainedTaskCompleted {
            task_result: TaskOutcome::Success,
            executed_steps: 1,
            final_page: Some(page),
            ..
        }) if page == "neutral/done"
    ));
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    let facts = task_facts(&run.events);
    assert_eq!(staged_steps(&facts), [("finish".to_owned(), 1)]);
    assert!(
        !facts
            .iter()
            .any(|fact| matches!(fact, TaskSemanticFact::EntryRecoveryPackageAdmitted { .. })),
        "no recovery package runs when the set matched"
    );
    let recognitions = entry_recognitions(&facts);
    assert!(!recognitions.is_empty());
    for (phase, required_pages, matched_page) in recognitions {
        assert_eq!(phase, "initial");
        assert_eq!(
            required_pages,
            serde_json::json!(["neutral/away", "neutral/home"])
        );
        assert_eq!(matched_page, serde_json::json!("neutral/home"));
    }
}

/// N3b-4: one frame matches two pages of the set: `contained_task_recognition_conflict`, and
/// no input.
#[test]
fn entry_set_conflict_fails_before_any_input() {
    let state = Arc::new(FakeState::default());
    let target = entry_package(
        "neutral.entry.target",
        serde_json::json!({
            "schema_version": "0.9",
            "task_id": "task",
            "game": "neutral",
            "server_scope": ["test"],
            "coordinate_space": {"width": 16, "height": 9},
            "entry_page": ["first", "second"],
            "target_page": "done",
            "operations": [
                entry_operation("from_first", "first", "done"),
                entry_operation("from_second", "second", "done")
            ]
        }),
        &[("first", AWAY), ("second", SECOND_GREEN), ("done", DONE)],
    );
    let run = run_entry(&state, &target, None);
    assert_eq!(run.receipt.state(), RuntimeReceiptState::Failed);
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    assert_eq!(
        terminal_failure_code(&task_facts(&run.events)).as_deref(),
        Some("contained_task_recognition_conflict")
    );
}

/// N3b-5: the first frame matches no page of the set {done, home}; the bound recovery package
/// ends on `home`, a page of the set, and the run goes on from `home`'s stage (the second);
/// a recovery package that ends outside the set fails with
/// `contained_task_home_recovery_terminal_non_home`.
#[test]
fn entry_recovery_lands_in_the_set_and_the_run_goes_on_from_that_stage() {
    let target = entry_package(
        "neutral.entry.target",
        two_stage_task(serde_json::json!(["done", "home"]), "done"),
        &[("away", AWAY), ("home", HOME), ("done", DONE)],
    );

    let state = Arc::new(FakeState::default());
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    state
        .stability_region_transition_after_inputs
        .store(2, Ordering::Release);
    let run = run_entry(&state, &target, Some(&recovery_package("home")));
    assert_eq!(run.receipt.state(), RuntimeReceiptState::Completed);
    assert!(matches!(
        run.receipt.result(),
        Some(RuntimeResult::ContainedTaskCompleted {
            task_result: TaskOutcome::Success,
            executed_steps: 2,
            ..
        })
    ));
    assert_eq!(state.input_count.load(Ordering::Acquire), 2);
    let facts = task_facts(&run.events);
    assert!(
        facts
            .iter()
            .any(|fact| matches!(fact, TaskSemanticFact::EntryRecoveryPackageAdmitted { .. }))
    );
    assert_eq!(staged_steps(&facts), [("finish".to_owned(), 1)]);
    let recognitions = entry_recognitions(&facts);
    assert!(recognitions.contains(&(
        "post_recovery".to_owned(),
        serde_json::json!(["neutral/done", "neutral/home"]),
        serde_json::json!("neutral/home"),
    )));
    assert!(recognitions.contains(&(
        "initial".to_owned(),
        serde_json::json!(["neutral/done", "neutral/home"]),
        serde_json::Value::Null,
    )));

    let state = Arc::new(FakeState::default());
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let run = run_entry(&state, &target, Some(&recovery_package("elsewhere")));
    assert_eq!(run.receipt.state(), RuntimeReceiptState::Failed);
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    let facts = task_facts(&run.events);
    // Ledger revision 3 (#378 A3): the fail-closed disposition carries the failure as `outcome`.
    assert!(facts.iter().any(|fact| matches!(
        fact,
        TaskSemanticFact::EntryTargetDisposition {
            disposition: TaskEntryTargetDisposition::FailClosed,
            outcome: Some(outcome),
        } if outcome.code().as_str() == "contained_task_home_recovery_terminal_non_home"
    )));
    assert!(staged_steps(&facts).is_empty());
}

/// N3b-7: a page-graph package with `entry_page: "any"` and no stages has no host entry
/// check; the run goes by its pages.
#[test]
fn any_entry_without_stages_has_no_host_entry_check() {
    let state = Arc::new(FakeState::default());
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let target = entry_package(
        "neutral.entry.target",
        serde_json::json!({
            "schema_version": "0.9",
            "task_id": "task",
            "game": "neutral",
            "server_scope": ["test"],
            "coordinate_space": {"width": 16, "height": 9},
            "entry_page": "any",
            "target_page": "home",
            "operations": [entry_operation("arrive", "away", "home")]
        }),
        &[("away", AWAY), ("home", HOME)],
    );
    let run = run_entry(&state, &target, None);
    assert_eq!(run.receipt.state(), RuntimeReceiptState::Completed);
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    assert!(
        !run.events
            .iter()
            .any(|event| event.event_type == EventType::TaskEntryPreflight),
        "no entry check runs for an `any` entry"
    );
}
