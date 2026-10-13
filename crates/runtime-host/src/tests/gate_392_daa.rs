// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #392 / #401, slice DA-A (model 392 v1.1, frozen: section 12.1 DA-A row, section 17
//! item 3; tests DA-A-T1 and DA-A-T2 of section 13): the aspect ratio is recorded, never
//! refused. A device instance's frame of any aspect ratio, equal to its package's resolution and
//! with a known rotation, passes the task geometry and the run goes on; the instance records its
//! display as the four `display.` facts of model section 8.1.

use super::*;
use actingcommand_contract::{FactScalar, TaskGeometryConclusion};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig, RuntimeFlowOutput};

const ALIAS: &str = "neutral.instance";

/// A host whose one instance is bound to a device endpoint (the neutral loopback endpoint the
/// other fakes use) and whose fake device serves `width`x`height` frames and display, rotation
/// `r0` observed.
fn device_host(root: &TempDir, width: u32, height: u32) -> (RuntimeHost, Arc<FakeState>) {
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    *state
        .physical_task_extent
        .lock()
        .expect("fake physical task extent") = Some((width, height));
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let registered = instance_id();
    let resolved = ResolvedExecutionInstance::new(registered, "127.0.0.1:16384").with_adb_endpoint(
        actingcommand_execution_kernel::ResolvedAdbEndpoint::new("127.0.0.1", 16384, false),
    );
    let host = RuntimeHost::start(
        config(root),
        Arc::new(
            FakeProvider::one(ALIAS, registered, Arc::clone(&state))
                .with_resolved_override(Arc::new(std::sync::Mutex::new(resolved))),
        ),
    )
    .expect("runtime host with a device instance");
    (host, state)
}

/// A navigable route package whose resolution and coordinate spaces are `width`x`height`: the
/// page colour at (0, 0) and one trusted tap at (1, 0) from `home` to `terminal`. Its timeouts
/// leave room for a full-size frame.
fn package_at(width: u32, height: u32) -> Vec<u8> {
    let space = serde_json::json!({"width": width, "height": height});
    let control = serde_json::json!({
        "schema_version": "Lab-1y.control.v1",
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
        "schema_version": "0.6",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": space,
        "entry_page": "home",
        "target_page": "terminal",
        "operations": [{
            "id": "open_terminal",
            "from": "home",
            "click": {"kind": "point", "x": 1, "y": 0},
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

/// Runs the `width`x`height` package on the instance through the official client.
fn run_package_at(root: &TempDir, width: u32, height: u32) -> RuntimeFlowOutput {
    let bytes = package_at(width, height);
    let path = root.path().join(format!("display-{width}x{height}.zip"));
    fs::write(&path, &bytes).expect("write package");
    let request = ContainedTaskRequest::new(
        path.display().to_string(),
        actingcommand_pack_containment::Sha256Hash::digest(&bytes).to_string(),
    )
    .expect("task request");
    RuntimeClient::connect(RuntimeClientConfig::new(
        root.path(),
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("official client")
    .run_contained_task(ALIAS, request)
    .unwrap_or_else(|error| panic!("{width}x{height}: the run is answered: {error:?}"))
}

/// The conclusions of the run's task geometry records, in ledger order.
fn geometry_conclusions(flow: &RuntimeFlowOutput) -> Vec<TaskGeometryConclusion> {
    flow.events()
        .iter()
        .filter_map(projected_task_semantic_fact)
        .filter_map(|fact| match fact {
            TaskSemanticFact::GeometryObserved { observation } => Some(observation.conclusion),
            _ => None,
        })
        .collect()
}

fn assert_run_goes_on(flow: &RuntimeFlowOutput, width: u32, height: u32) {
    assert_eq!(
        flow.receipt().state(),
        RuntimeReceiptState::Completed,
        "{width}x{height}: {:?}",
        flow.receipt()
    );
    assert!(
        matches!(
            flow.receipt().result(),
            Some(RuntimeResult::ContainedTaskCompleted {
                task_result: TaskOutcome::Success,
                final_page: Some(page),
                ..
            }) if page == "neutral/terminal"
        ),
        "{width}x{height}: {:?}",
        flow.receipt()
    );
    let conclusions = geometry_conclusions(flow);
    assert!(
        !conclusions.is_empty()
            && conclusions
                .iter()
                .all(|conclusion| *conclusion == TaskGeometryConclusion::Pass),
        "{width}x{height}: {conclusions:?}"
    );
}

/// One `record_list` row `{width, height}` (model section 8.1).
fn extent_row(width: i64, height: i64) -> ContractFactValue {
    ContractFactValue::RecordList(vec![BTreeMap::from([
        ("width".to_owned(), FactScalar::Integer(width)),
        ("height".to_owned(), FactScalar::Integer(height)),
    ])])
}

/// DA-A-T1: a 1080x2340 frame on a 1080x2340 package with a known rotation passes the task
/// geometry and the run goes on; `display.aspect` holds the frame reduced by its greatest common
/// divisor (2340x1080 reduces to 13:6, so 1080x2340 is width 6, height 13), and the three other
/// display facts record the frame, its rotation and its orientation.
#[test]
fn da_a_t1_a_1080x2340_frame_passes_and_its_aspect_is_recorded() {
    let root = TempDir::new().expect("tempdir");
    let (host, state) = device_host(&root, 1080, 2340);
    let flow = run_package_at(&root, 1080, 2340);
    assert_run_goes_on(&flow, 1080, 2340);
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    let snapshot = host
        .instance_fact_snapshot(InstanceFactContext {
            instance_id: ALIAS.to_owned(),
            server_id: "test".to_owned(),
            game_id: "neutral".to_owned(),
        })
        .expect("instance fact snapshot");
    let display = |key: &str| {
        snapshot
            .records
            .iter()
            .find(|record| {
                record.key == key
                    && record.scope
                        == FactScope::Instance {
                            instance_id: ALIAS.to_owned(),
                        }
            })
            .map(|record| record.content.clone())
    };
    let inline = |value| Some(FactContent::Inline { value });
    assert_eq!(display("display.aspect"), inline(extent_row(6, 13)));
    assert_eq!(
        display("display.frame_extent"),
        inline(extent_row(1080, 2340))
    );
    assert_eq!(
        display("display.rotation"),
        inline(ContractFactValue::String("r0".to_owned()))
    );
    assert_eq!(
        display("display.orientation"),
        inline(ContractFactValue::String("portrait".to_owned()))
    );
    host.close().expect("close host");
}

/// DA-A-T2: 4:3, 16:10, 16:9, 18:9, 19.5:9, 20:9 and 21:9, each landscape and portrait, with the
/// frame equal to the package: every run passes the task geometry and goes on; no aspect ratio
/// is refused.
#[test]
fn da_a_t2_no_aspect_ratio_is_refused_when_the_frame_equals_its_package() {
    const LANDSCAPE: [(u32, u32); 7] = [
        (4, 3),
        (16, 10),
        (16, 9),
        (18, 9),
        (39, 18),
        (20, 9),
        (21, 9),
    ];
    for (long, short) in LANDSCAPE {
        for (width, height) in [(long, short), (short, long)] {
            let root = TempDir::new().expect("tempdir");
            let (host, _) = device_host(&root, width, height);
            let flow = run_package_at(&root, width, height);
            assert_run_goes_on(&flow, width, height);
            host.close().expect("close host");
        }
    }
}
