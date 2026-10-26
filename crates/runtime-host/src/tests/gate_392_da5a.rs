// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #392 / #401, slice DA-5a (model 392 v1.2: section 0.2 C12 and C16, section 12.1
//! DA-5a row): the task schema checks become one ordered type and no package behaves otherwise.
//! The slice's identity test (merge is deploy, Alice 10-26): a 1280x720 package on a 1280x720
//! frame runs as it declares, at task schema 0.6 under control v1 and at task schema 0.9 under
//! control v2.

use super::gate_392_daa::{assert_run_goes_on, device_host};
use super::*;
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig, RuntimeFlowOutput};

const ALIAS: &str = "neutral.instance";
const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;

/// A navigable route package at 1280x720 whose task is at `task_schema` under `control_schema`:
/// the page colour at (0, 0) and one trusted tap at the frame's centre (640, 360) from `home` to
/// `terminal`.
fn package(task_schema: &str, control_schema: &str) -> Vec<u8> {
    let space = serde_json::json!({"width": WIDTH, "height": HEIGHT});
    let control = serde_json::json!({
        "schema_version": control_schema,
        "package_id": "neutral.semantic.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": space,
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 20_000,
        "timeout_ms": 40_000,
        "max_steps": 2
    });
    let task = serde_json::json!({
        "schema_version": task_schema,
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": space,
        "entry_page": "home",
        "target_page": "terminal",
        "operations": [{
            "id": "open_terminal",
            "from": "home",
            "click": {"kind": "point", "x": 640, "y": 360},
            "unguarded_trusted_coordinate": true,
            "retryable": false
        }]
    });
    let recognition = serde_json::json!({
        "schema_version": "0.3",
        "game": "neutral",
        "server": "test",
        "coordinate_space": space,
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            {"type": "color", "id": "page/home", "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [255, 0, 0]},
            {"type": "color", "id": "page/terminal", "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [0, 0, 255]},
            {"type": "color", "id": "page/error", "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [255, 255, 0]}
        ]
    });
    let pages = serde_json::json!({
        "schema_version": "0.3",
        "pages": [
            {"id": "neutral/home", "required": ["page/home"], "optional": [], "forbidden": []},
            {"id": "neutral/terminal", "required": ["page/terminal"], "optional": [], "forbidden": []},
            {"id": "neutral/error", "required": ["page/error"], "optional": [], "forbidden": []}
        ]
    });
    let manifest = serde_json::json!({"schema_version": "0.3", "entry_task_id": "task"});
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, contents) in [
        ("control.json", control),
        ("resources/manifest.json", manifest),
        ("resources/operations/task/task.json", task),
        ("resources/recognition/neutral.test.pack.json", recognition),
        ("resources/recognition/neutral.test.pages.json", pages),
    ] {
        zip.start_file(path, options).expect("package entry");
        zip.write_all(&serde_json::to_vec(&contents).expect("package JSON"))
            .expect("package content");
    }
    zip.finish().expect("finish package").into_inner()
}

/// Runs the package on the instance through the official client.
fn run(root: &TempDir, task_schema: &str, control_schema: &str) -> RuntimeFlowOutput {
    let path = root.path().join(format!("identity-{task_schema}.zip"));
    fs::write(&path, package(task_schema, control_schema)).expect("write package");
    let request = ContainedTaskRequest::new(path.display().to_string()).expect("task request");
    RuntimeClient::connect(RuntimeClientConfig::new(
        root.path(),
        actingcommand_runtime_client::RuntimeChannel::Cli,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("official client")
    .run_contained_task(ALIAS, request)
    .unwrap_or_else(|error| panic!("task schema {task_schema}: the run is answered: {error:?}"))
}

/// The identity mapping: on a 1280x720 frame, a 1280x720 package at task schema 0.6 (control
/// v1) and at task schema 0.9 (control v2) passes the task geometry, completes on `terminal`,
/// and its one input is the tap it declares, at (640, 360) of the frame.
#[test]
fn da5a_identity_a_1280x720_package_on_a_1280x720_frame_runs_as_declared() {
    for (task_schema, control_schema) in
        [("0.6", "Lab-1y.control.v1"), ("0.9", "Lab-1y.control.v2")]
    {
        let root = TempDir::new().expect("tempdir");
        let (host, state) = device_host(&root, WIDTH, HEIGHT);
        let flow = run(&root, task_schema, control_schema);
        assert_run_goes_on(&flow, WIDTH, HEIGHT);
        assert_eq!(
            state
                .input_actions
                .lock()
                .expect("fake input actions lock")
                .as_slice(),
            [InputAction::Tap { x: 640, y: 360 }],
            "task schema {task_schema}"
        );
        host.close().expect("close host");
    }
}
