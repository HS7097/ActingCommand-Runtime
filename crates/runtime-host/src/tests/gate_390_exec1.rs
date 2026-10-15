// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #390 EXEC-1 (forced-compromises mapping row E-04; FCM §2.4 row EXEC-1), the host: a
// guard that refuses on the step's frame falls, as the operation declares, to the page's next
// candidate. The run's ledger shows the step passed over as a step that finished on the page
// without an input, the next candidate as the next step on the same frame, and the run ends as
// that candidate's run would. Every expected value comes from the FCM row and the contract
// (`contracts/resource-declarations.md`, Task schema `0.9` candidate operations; register EC2).

use super::*;

const EXEC1_ALIAS: &str = "neutral.instance";

// `claim` from `home` to `terminal`, guarded by the ready flag and declared to fall to the next
// candidate when its guard refuses; `skip` from `home` to `terminal`, a trusted tap. The outcome
// is "claimed" when `claim` completed and "skipped" when it did not.
fn exec1_package() -> Vec<u8> {
    let task = serde_json::json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "entry_page": "home",
        "target_page": "terminal",
        "scheduling_outcome": {
            "designated_operation": "claim",
            "mappings": [
                {"outcome_key": "claimed", "effect": "designated_effect_completed",
                 "terminal_pages": ["terminal"]},
                {"outcome_key": "skipped", "effect": "no_designated_effect",
                 "terminal_pages": ["terminal"]}
            ]
        },
        "operations": [
            {
                "id": "claim",
                "from": "home",
                "to": "terminal",
                "click": {"kind": "point", "x": 1, "y": 0},
                "guard": {
                    "page_id": "home",
                    "target_id": "guard/ready",
                    "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                    "color_probe": "guard/ready"
                },
                "retryable": false,
                "on_guard_refused": "next_candidate"
            },
            {
                "id": "skip",
                "from": "home",
                "to": "terminal",
                "click": {"kind": "point", "x": 0, "y": 0},
                "unguarded_trusted_coordinate": true,
                "retryable": false
            }
        ]
    });
    let control = serde_json::json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.exec1.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 50,
        "timeout_ms": 5000,
        "max_steps": 10
    });
    let pack = serde_json::json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            {"type": "color", "id": "page/home",
             "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [255, 0, 0]},
            {"type": "color", "id": "page/terminal",
             "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [0, 0, 255]},
            {"type": "color", "id": "guard/ready",
             "region": {"x": 1, "y": 0, "width": 1, "height": 1}, "expected": [0, 255, 0]}
        ]
    });
    let pages = serde_json::json!({
        "schema_version": "0.3",
        "pages": [
            {"id": "neutral/home", "required": ["page/home"], "optional": [], "forbidden": []},
            {"id": "neutral/terminal", "required": ["page/terminal"], "optional": [],
             "forbidden": []}
        ]
    });
    let files = [
        ("control.json", control),
        (
            "resources/manifest.json",
            serde_json::json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        ("resources/operations/task/task.json", task),
        ("resources/recognition/neutral.test.pack.json", pack),
        ("resources/recognition/neutral.test.pages.json", pages),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, contents) in files {
        zip.start_file(path, options).expect("package entry");
        zip.write_all(&serde_json::to_vec(&contents).expect("package JSON"))
            .expect("package content");
    }
    zip.finish().expect("finish package").into_inner()
}

// A step record of the run's ledger: its kind, step index and operation, and for a finished
// step the page it finished on.
#[derive(Debug, Clone, PartialEq, Eq)]
enum StepRecord {
    Started(u32, String),
    Intent(u32, String),
    Completed(u32, String),
    Finished(u32, String, String),
}

// FCM E-04 「守卫拒绝时可以声明落到下一条候选」, end to end: on a screen where `claim`'s guard refuses,
// `claim`'s step finishes on `home` without an input, `skip` runs as the next step on the same
// frame, and the run succeeds with `claim`'s designated effect not completed.
#[test]
fn exec1_a_guard_refusal_falls_to_the_next_candidate_in_the_ledger() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("exec1.zip");
    fs::write(&package, exec1_package()).expect("write package");
    let state = Arc::new(FakeState::default());
    // A direct run needs a device instance whose display is observed (Workflow #392 DA-A).
    state.physical_task_geometry.store(true, Ordering::Release);
    *state
        .physical_task_extent
        .lock()
        .expect("fake physical task extent") = Some((2, 1));
    // The guard's flag is never ready; the first input shows the terminal page.
    state.refuse_guard_capture.store(true, Ordering::Release);
    state
        .transition_capture_after_inputs
        .store(1, Ordering::Release);
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            EXEC1_ALIAS,
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    client.set_receipt_read_timeout();
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            EXEC1_ALIAS,
            client.ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.display().to_string()).expect("task request"),
        ),
    );
    let _receipt = client.send(&request);
    let events = projected_events(
        &mut client,
        EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        },
    );
    let steps = events
        .iter()
        .filter_map(|event| match projected_task_semantic_fact(event)? {
            TaskSemanticFact::StepStarted {
                step_index,
                operation_label,
                ..
            } => Some(StepRecord::Started(*step_index, operation_label.clone())),
            TaskSemanticFact::EffectIntent {
                step_index,
                operation_label,
                ..
            } => Some(StepRecord::Intent(*step_index, operation_label.clone())),
            TaskSemanticFact::EffectCompleted {
                step_index,
                operation_label,
                ..
            } => Some(StepRecord::Completed(*step_index, operation_label.clone())),
            TaskSemanticFact::StepFinished {
                step_index,
                operation_label,
                page_label,
                ..
            } => Some(StepRecord::Finished(
                *step_index,
                operation_label.clone(),
                page_label.clone(),
            )),
            _ => None,
        })
        .collect::<Vec<_>>();
    let terminal = events
        .iter()
        .find_map(|event| match projected_task_semantic_fact(event) {
            Some(TaskSemanticFact::TerminalCommitted {
                task_result,
                executed_steps,
                scheduling_disposition,
                outcome,
                ..
            }) => Some((
                *task_result,
                *executed_steps,
                scheduling_disposition.clone(),
                outcome
                    .as_ref()
                    .map(|outcome| outcome.code().as_str().to_owned()),
            )),
            _ => None,
        })
        .unwrap_or_else(|| {
            panic!(
                "the run's terminal; the Runtime's fatal error: {:?}",
                host.fatal_error()
            )
        });
    let inputs = state.input_count.load(Ordering::Acquire);
    drop(client);
    host.close().expect("close runtime host");

    assert_eq!(terminal.0, TaskOutcome::Success, "{:?}", terminal.3);
    assert_eq!(inputs, 1);
    assert_eq!(
        steps,
        vec![
            StepRecord::Started(0, "claim".to_owned()),
            StepRecord::Finished(0, "claim".to_owned(), "neutral/home".to_owned()),
            StepRecord::Started(1, "skip".to_owned()),
            StepRecord::Intent(1, "skip".to_owned()),
            StepRecord::Completed(1, "skip".to_owned()),
            StepRecord::Finished(1, "skip".to_owned(), "neutral/terminal".to_owned()),
        ]
    );
    // The step passed over stays counted.
    assert_eq!(terminal.1, Some(2));
    let disposition = terminal.2.expect("a scheduling disposition");
    assert_eq!(disposition.outcome_key(), "skipped");
}
