// SPDX-License-Identifier: AGPL-3.0-only

//! One-off (to be reverted), Workflow #336 L2e evidence item 8 (optional): a linear package with
//! an application entry on the existing fake physical provider (`control_application` returns
//! Ok, the foreground is always the assigned application). Copied into the runtime-host test
//! module by the one-off workflow only; every printed line starts with `HOST|`.

use super::*;

fn oneoff_linear_package(task: &serde_json::Value) -> Vec<u8> {
    let control = serde_json::json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.linear.application",
        "execution_mode": "linear_steps",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 16, "height": 9},
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 500,
        "timeout_ms": 5000,
        "max_steps": 2
    });
    let recognition = serde_json::json!({
        "schema_version": "0.3",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 16, "height": 9},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            {"type": "color", "id": "page/home", "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [255, 0, 0]},
            {"type": "color", "id": "page/terminal", "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [0, 0, 255]}
        ]
    });
    let pages = serde_json::json!({
        "schema_version": "0.3",
        "pages": [
            {"id": "neutral/home", "required": ["page/home"], "optional": [], "forbidden": []},
            {"id": "neutral/terminal", "required": ["page/terminal"], "optional": [], "forbidden": []}
        ]
    });
    let control = serde_json::to_vec(&control).expect("control JSON");
    let task = serde_json::to_vec(task).expect("task JSON");
    let recognition = serde_json::to_vec(&recognition).expect("recognition JSON");
    let pages = serde_json::to_vec(&pages).expect("pages JSON");
    let cursor = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(cursor);
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, contents) in [
        ("control.json", control.as_slice()),
        (
            "resources/manifest.json",
            br#"{"schema_version":"0.3","entry_task_id":"task"}"#.as_slice(),
        ),
        ("resources/operations/task/task.json", task.as_slice()),
        (
            "resources/recognition/neutral.test.pack.json",
            recognition.as_slice(),
        ),
        ("resources/recognition/neutral.test.pages.json", pages.as_slice()),
    ] {
        zip.start_file(path, options).expect("zip entry");
        zip.write_all(contents).expect("zip content");
    }
    zip.finish().expect("finish zip").into_inner()
}

fn oneoff_task(first_to: &str, second_to: &str) -> serde_json::Value {
    let second_from = first_to;
    serde_json::json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 16, "height": 9},
        "timeout_ms": 5000,
        "max_steps": 2,
        "entry_page": "any",
        "target_page": second_to,
        "operations": [
            {
                "id": "step_01_app",
                "from": "any",
                "to": first_to,
                "application": {"action": "restart"},
                "expect_after": {"page_id": first_to, "timeout_ms": 300, "interval_ms": 10},
                "post_delay_ms": 10
            },
            {
                "id": "step_02_click",
                "from": second_from,
                "to": second_to,
                "click": {"kind": "point", "x": 1, "y": 0},
                "unguarded_trusted_coordinate": true,
                "expect_after": {"page_id": second_to, "timeout_ms": 300, "interval_ms": 10},
                "post_delay_ms": 10
            }
        ]
    })
}

fn oneoff_run(label: &str, task: &serde_json::Value, fail_application: bool) {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("linear-application.zip");
    let bytes = oneoff_linear_package(task);
    fs::write(&package, &bytes).expect("write package");
    let expected = actingcommand_pack_containment::Sha256Hash::digest(&bytes).to_string();
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    state
        .fail_application
        .store(fail_application, Ordering::Release);
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            "neutral.instance",
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            "neutral.instance",
            client.ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.display().to_string(), expected).expect("request"),
        ),
    );
    let receipt = client.send(&request);
    println!(
        "HOST|{label}|receipt|{}",
        serde_json::to_string(&receipt).expect("receipt JSON")
    );
    println!(
        "HOST|{label}|counts|application_count={} input_count={} capture_count={}",
        state.application_count.load(Ordering::Acquire),
        state.input_count.load(Ordering::Acquire),
        state.capture_count.load(Ordering::Acquire)
    );
    let events = projected_events(
        &mut client,
        EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        },
    );
    for event in &events {
        let semantic = projected_task_semantic_fact(event)
            .map(|fact| format!("{fact:?}"))
            .unwrap_or_default();
        println!(
            "HOST|{label}|event|{}|{:?}|{}",
            event.sequence, event.event_type, semantic
        );
        if matches!(
            event.event_type,
            EventType::ApplicationIntent
                | EventType::ApplicationCompleted
                | EventType::ApplicationFailed
                | EventType::TaskFailed
                | EventType::TaskCompleted
                | EventType::RuntimeFactRecorded
        ) {
            println!(
                "HOST|{label}|event json|{}|{}",
                event.sequence,
                serde_json::to_string(event).expect("event JSON")
            );
        }
    }
    drop(client);
    host.close().expect("close host");
}

#[test]
fn oneoff_336l2e_linear_application_entry_on_fake_physical_device() {
    // P1 shape: any -> restart -> home, then a click to terminal; the fake frame is home until
    // the first pointer input and terminal after it.
    oneoff_run(
        "P1 performed",
        &oneoff_task("home", "terminal"),
        false,
    );
    // The provider's application call fails with a fatal device error.
    oneoff_run(
        "P1 application failure",
        &oneoff_task("home", "terminal"),
        true,
    );
    // The application step expects terminal, which never shows before an input: unconfirmed.
    oneoff_run(
        "application unconfirmed",
        &oneoff_task("terminal", "home"),
        false,
    );
}
