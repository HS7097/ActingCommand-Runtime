// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #392 / #401, slice DA-2a in the host (model 392 v1.2 §0.2 C2 and C3, §4.5, §6.6,
//! §8.3, §12.1 row DA-2a; test row DA2-T9 of §13; brief rd-1026/brief-da2a): the run keeps its
//! game's display declaration from its preparation, records the display map of every frame its
//! geometry passed, and rebuilds it when the frame changes. A frame of another size no longer
//! stops the run's input: its geometry is read again, the change is recorded (`changed`, trigger
//! `extent_changed`, with its `display_map`), the instance's display facts follow it, and the run
//! goes on. Until the input side maps (DA-3) an input waits for a frame whose map is the plain
//! identity: under a pure scale the run ends `display_map_unresolved` (`input_scale_unsupported`)
//! with no input. A declaration that measures on the safe area finds no insets before DA-4b:
//! `contained_task_geometry_unavailable` (`safe_area_unavailable`), no input.
//!
//! A synthetic `neutral` game, server `test`. The device instance is bound to a neutral loopback
//! endpoint; its frames show `home` (red) until the run's one input and `terminal` (blue) after it,
//! in the frame's 2x2 top-left block, so a pure scale of 3/2 keeps the colour of its first pixel.

use super::*;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    DisplayArea, DisplayMapKind, DisplayScaleBy, FactScalar, TaskGeometryConclusion,
    TaskGeometryDisplayMap, TaskGeometryObservation, TaskGeometryPhase, TaskGeometryRecheckTrigger,
};
use serde_json::{Value, json};

const ALIAS: &str = "neutral.instance";
const GAME: &str = "neutral";
const HOME: [u8; 3] = [255, 0, 0];
const TERMINAL: [u8; 3] = [0, 0, 255];

/// A host whose one instance is bound to a device endpoint whose frames and display are
/// `width`x`height` with rotation `r0` observed (after the run's input `after` when given),
/// reading the standard packs of `root`; every frame shows the page in its 2x2 top-left block.
fn device_host(
    temp: &TempDir,
    root: Option<&Path>,
    (width, height): (u32, u32),
    after: Option<(u32, u32)>,
) -> (RuntimeHost, Arc<FakeState>) {
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    *state
        .physical_task_extent
        .lock()
        .expect("fake physical task extent") = Some((width, height));
    *state
        .physical_task_extent_after_input
        .lock()
        .expect("fake physical task extent after input") = after;
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let (before_width, after_width) = (
        width as usize,
        after.map_or(width, |after| after.0) as usize,
    );
    *state.pixel_script.lock().expect("pixel script") = Some(Arc::new(
        move |_capture: usize, inputs: usize| -> Vec<(usize, [u8; 3])> {
            let (row, page) = if inputs > 0 {
                (after_width, TERMINAL)
            } else {
                (before_width, HOME)
            };
            [0, 1, row, row + 1]
                .into_iter()
                .map(|index| (index, page))
                .collect()
        },
    ));
    let registered = instance_id();
    let resolved = ResolvedExecutionInstance::new(registered, "127.0.0.1:16384").with_adb_endpoint(
        actingcommand_execution_kernel::ResolvedAdbEndpoint::new("127.0.0.1", 16384, false),
    );
    let host = RuntimeHost::start(
        config(temp).with_standard_packs_root(root.map(Path::to_path_buf)),
        Arc::new(
            FakeProvider::one(ALIAS, registered, Arc::clone(&state))
                .with_resolved_override(Arc::new(std::sync::Mutex::new(resolved))),
        ),
    )
    .expect("runtime host with a device instance");
    (host, state)
}

/// A navigable route package of the game whose resolution and coordinate spaces are
/// `width`x`height`: the page colour at (0, 0) and one trusted tap at (1, 0) from `home` to
/// `terminal`. Its timeouts leave room for full-size frames.
fn package_at(width: u32, height: u32) -> Vec<u8> {
    let space = json!({"width": width, "height": height});
    let control = json!({
        "schema_version": "Lab-1y.control.v1",
        "package_id": "neutral.test.task",
        "execution_mode": "navigable_route",
        "game": GAME,
        "server": "test",
        "resolution": space,
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 20_000,
        "timeout_ms": 40_000,
        "max_steps": 2
    });
    let task = json!({
        "schema_version": "0.6",
        "task_id": "task",
        "game": GAME,
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
    let recognition = json!({
        "schema_version": "0.3",
        "game": GAME,
        "server": "test",
        "coordinate_space": space,
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            {"type": "color", "id": "page/home", "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": HOME},
            {"type": "color", "id": "page/terminal", "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": TERMINAL}
        ]
    });
    let pages = json!({
        "schema_version": "0.3",
        "pages": [
            {"id": "neutral/home", "required": ["page/home"], "optional": [], "forbidden": []},
            {"id": "neutral/terminal", "required": ["page/terminal"], "optional": [], "forbidden": []}
        ]
    });
    let manifest = json!({"schema_version": "0.3", "entry_task_id": "task"});
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

/// The standard-pack root `<temporary>/packs/` with the game root `neutral/`: a `bundle.json`
/// listing one package no run here uses, and `declaration` as `display-layout.json`.
fn pack_root(temp: &TempDir, declaration: &Value) -> PathBuf {
    let root = temp.path().join("packs");
    let game = root.join(GAME);
    fs::create_dir_all(&game).expect("game root");
    fs::write(
        game.join("bundle.json"),
        serde_json::to_vec_pretty(&json!({
            "schema_version": "actingcommand.bundle.v4",
            "game": GAME,
            "packs": [{
                "package_id": "neutral.test.listed",
                "server": "test",
                "entry_task_id": "task",
                "path": "packs/neutral.test.listed",
                "file_count": 3,
                "byte_count": 64
            }]
        }))
        .expect("bundle JSON"),
    )
    .expect("write bundle.json");
    fs::write(
        game.join("display-layout.json"),
        serde_json::to_vec_pretty(declaration).expect("declaration JSON"),
    )
    .expect("write display-layout.json");
    root
}

struct Run {
    receipt: RuntimeReceipt,
    events: Vec<PersistedEvent>,
}

impl Run {
    fn geometry(&self) -> Vec<TaskGeometryObservation> {
        self.events
            .iter()
            .filter_map(|event| match event.payload() {
                EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
                    TaskSemanticFact::GeometryObserved { observation } => {
                        Some(observation.as_ref().clone())
                    }
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }
}

/// Runs `package` (written as `<name>.zip`) on the instance and reads its records back.
fn run(host: &RuntimeHost, temp: &TempDir, name: &str, package: &[u8]) -> Run {
    let path = temp.path().join(format!("{name}.zip"));
    fs::write(&path, package).expect("write run package");
    let task_request = ContainedTaskRequest::new(path.display().to_string()).expect("task request");
    let mut client = TestClient::connect(host);
    // Full-size frames are read, kept and recognised on every capture of an unoptimised build.
    client
        .stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .expect("receipt timeout");
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

fn text<'a>(outcome: &'a Outcome, key: &str) -> Option<&'a str> {
    outcome.value(key).and_then(Value::as_str)
}

fn number(outcome: &Outcome, key: &str) -> Option<u64> {
    outcome.value(key).and_then(Value::as_u64)
}

/// The instance's active `display.frame_extent`.
fn frame_extent_fact(host: &RuntimeHost) -> Option<FactContent> {
    host.instance_fact_snapshot(InstanceFactContext {
        instance_id: ALIAS.to_owned(),
        server_id: "test".to_owned(),
        game_id: GAME.to_owned(),
    })
    .expect("instance fact snapshot")
    .records
    .iter()
    .find(|record| {
        record.key == "display.frame_extent"
            && record.scope
                == FactScope::Instance {
                    instance_id: ALIAS.to_owned(),
                }
    })
    .map(|record| record.content.clone())
}

fn extent_row(width: i64, height: i64) -> FactContent {
    FactContent::Inline {
        value: ContractFactValue::RecordList(vec![BTreeMap::from([
            ("width".to_owned(), FactScalar::Integer(width)),
            ("height".to_owned(), FactScalar::Integer(height)),
        ])]),
    }
}

/// A 9:16 declaration, by width on the screen, or the given `scale_area`.
fn declaration(scale_area: &str) -> Value {
    json!({
        "schema_version": "actingcommand.display-layout.v1",
        "game": GAME,
        "scale": {"by": "width", "area": scale_area},
        "anchor_area": "screen",
        "correction": {"radius_px": 16}
    })
}

/// DA2-T9 (model 392 v1.2 §0.2 C2, C3, §6.6; on a 9:16 emulator display, the identity and a pure
/// scale being this slice's maps): a 720x1280 package on a 720x1280 device whose display becomes
/// 1080x1920 after the run's input. The first frame's record is `initial` with no map (the
/// identity leaves it as it was); the first frame of the new size reads the geometry again and is
/// recorded `changed` with trigger `extent_changed` and the pure scale it was mapped under (3/2,
/// scale frame 720x1280, base 720x1280, by the declaration's width on the screen); every record
/// passes; `display.frame_extent` follows to 1080x1920; the run is not stopped by the size change
/// (no `contained_task_geometry_input_prerequisite_missing`) and completes on its target page.
#[test]
fn da2a_t9_a_frame_that_changes_its_size_rebuilds_the_map_and_the_run_goes_on() {
    let temp = TempDir::new().expect("tempdir");
    let root = pack_root(&temp, &declaration("screen"));
    let (host, state) = device_host(&temp, Some(&root), (720, 1280), Some((1080, 1920)));
    let run = run(&host, &temp, "changed", &package_at(720, 1280));
    assert_eq!(
        run.receipt.state(),
        RuntimeReceiptState::Completed,
        "{:?}",
        run.receipt
    );
    assert!(
        matches!(
            run.receipt.result(),
            Some(RuntimeResult::ContainedTaskCompleted {
                task_result: TaskOutcome::Success,
                final_page: Some(page),
                ..
            }) if page == "neutral/terminal"
        ),
        "{:?}",
        run.receipt
    );
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    let geometry = run.geometry();
    let [initial, changed] = geometry.as_slice() else {
        panic!("an initial and a changed record: {geometry:?}");
    };
    assert_eq!(initial.phase, TaskGeometryPhase::Initial);
    assert_eq!(initial.display_map, None);
    assert_eq!(changed.phase, TaskGeometryPhase::Changed);
    assert_eq!(
        changed.trigger,
        Some(TaskGeometryRecheckTrigger::ExtentChanged)
    );
    assert_eq!(
        changed
            .frame
            .as_ref()
            .map(|frame| (frame.extent.width(), frame.extent.height())),
        Some((1080, 1920))
    );
    assert_eq!(
        changed.display_map,
        Some(TaskGeometryDisplayMap {
            kind: DisplayMapKind::Scale,
            scale_num: 3,
            scale_den: 2,
            scale_width: 720,
            scale_height: 1280,
            base_width: 720,
            base_height: 1280,
            scale_by: DisplayScaleBy::Width,
            scale_area: DisplayArea::Screen,
            anchor_area: DisplayArea::Screen,
            insets_source: None,
            declared: true,
        })
    );
    assert!(
        geometry
            .iter()
            .all(|record| record.conclusion == TaskGeometryConclusion::Pass),
        "{geometry:?}"
    );
    assert_eq!(frame_extent_fact(&host), Some(extent_row(1080, 1920)));
    host.close().expect("close runtime host");
}

/// The host's input prerequisite under a pure scale (Workflow #392 DA-2a; model 392 v1.2 §6.6;
/// the input side maps from DA-3): a 1280x720 package on a 1920x1080 device recognises its home
/// page on the scale frame, but its tap is not sent; the run ends `display_map_unresolved` with
/// `input_scale_unsupported` at `display_input_map`, the frame's and the base's sizes, and the
/// device took no input.
#[test]
fn da2a_an_input_waits_for_the_input_side_under_a_pure_scale() {
    let temp = TempDir::new().expect("tempdir");
    let (host, state) = device_host(&temp, None, (1920, 1080), None);
    let run = run(&host, &temp, "scaled", &package_at(1280, 720));
    host.close().expect("close runtime host");
    assert_eq!(
        run.receipt.state(),
        RuntimeReceiptState::Failed,
        "{:?}",
        run.receipt
    );
    let outcome = run.receipt.outcome();
    assert_eq!(
        outcome.code().as_str(),
        "display_map_unresolved",
        "{outcome:?}"
    );
    assert_eq!(
        text(outcome, "display_problem"),
        Some("input_scale_unsupported")
    );
    assert_eq!(text(outcome, "stage"), Some("display_input_map"));
    assert_eq!(number(outcome, "frame_width"), Some(1920));
    assert_eq!(number(outcome, "frame_height"), Some(1080));
    assert_eq!(number(outcome, "base_width"), Some(1280));
    assert_eq!(number(outcome, "base_height"), Some(720));
    assert_eq!(state.input_count.load(Ordering::Acquire), 0, "no input");
    let geometry = run.geometry();
    assert_eq!(
        geometry
            .first()
            .and_then(|record| record.display_map.map(|map| (map.kind, map.declared))),
        Some((DisplayMapKind::Scale, false)),
        "{geometry:?}"
    );
}

/// The safe area before DA-4b (model 392 v1.2 §4.5): a declaration whose scale is measured on the
/// safe area finds no insets on the first frame, so the run ends
/// `contained_task_geometry_unavailable` with `safe_area_unavailable`, after its geometry record
/// and before any input.
#[test]
fn da2a_a_safe_area_declaration_finds_no_insets_before_da_4b() {
    let temp = TempDir::new().expect("tempdir");
    let root = pack_root(&temp, &declaration("safe_area"));
    let (host, state) = device_host(&temp, Some(&root), (720, 1280), None);
    let run = run(&host, &temp, "safe-area", &package_at(720, 1280));
    host.close().expect("close runtime host");
    assert_eq!(
        run.receipt.state(),
        RuntimeReceiptState::Failed,
        "{:?}",
        run.receipt
    );
    let outcome = run.receipt.outcome();
    assert_eq!(
        outcome.code().as_str(),
        "contained_task_geometry_unavailable",
        "{outcome:?}"
    );
    assert_eq!(
        text(outcome, "display_problem"),
        Some("safe_area_unavailable")
    );
    assert_eq!(state.input_count.load(Ordering::Acquire), 0, "no input");
    assert_eq!(
        run.geometry()
            .iter()
            .map(|record| (record.phase, record.conclusion))
            .collect::<Vec<_>>(),
        [(TaskGeometryPhase::Initial, TaskGeometryConclusion::Pass)]
    );
}
