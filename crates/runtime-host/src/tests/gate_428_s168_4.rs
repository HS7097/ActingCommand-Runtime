// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #428 S168-4 (`contracts/resource-readings.md`, "List readings"; register §1.291, the
// slice's named tests), the host's halves. A package's `read_list` step reads the neutral list
// of the SEL-L4b host gate (`gate_308_l4b`): a 64 x 96 frame showing one column of cards, card k
// a noise marker (the anchor template, 8 x 6 px) at x 4 and y 10 + 12 k with a 4 x 4 patch of its
// own colour to its right, from which the scripted OCR provider reads its name `member-0k`. Pixel
// (0, 0) is red on the list page and blue on the detail page, (1, 0) green (the list's start),
// (2, 0) blue (its end) and (3, 0) white (the step's guard). The step reads the cards' `member`
// as the key `id` and, as the column `open`, whether a card's patch is card 0's colour. Its scan
// is the SEL-L4b gate's; both checks pass on the first viewport, so a one-screen list makes no
// move. The tap `leave` then goes to the detail page. A fake device paints the screen the test
// plans, by the inputs it took. The ids are made up; the keys are
// `inventory.list.example_kind_a` and `inventory.list.example_kind_b`.

use super::gate_308_l4b::{ALIAS, CardNames, HEIGHT, WIDTH, member, single_frame_select_package};
use super::*;

const LIST_KEY: &str = "inventory.list.example_kind_a";
const OTHER_KEY: &str = "inventory.list.example_kind_b";
const UNRESOLVED: &str = "contained_task_resource_reading_unresolved";
const RESERVED: &str = "list_reading_key_reserved";
const VALID_FOR_MS: u64 = 86_400_000;
// The second instance of the full-store test (`TESTSERIAL0002` in the register's words).
const OTHER_ALIAS: &str = "neutral.other";
// The anchor template's size.
const MARKER: (u32, u32) = (8, 6);

const RED: [u8; 3] = [255, 0, 0];
const GREEN: [u8; 3] = [0, 255, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const WHITE: [u8; 3] = [255, 255, 255];
const BACKGROUND: [u8; 3] = [30, 31, 32];
// A patch colour no card has: its name reads as nothing.
const UNREADABLE: [u8; 3] = [250, 100, 90];

// Deterministic pixel noise, the SEL-L3 gate's: a shifted copy of it correlates with it near
// zero.
fn noise(x: u32, y: u32) -> [u8; 3] {
    let mut state = u64::from(y * 64 + x).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x2545_f491;
    let mut next = || {
        state ^= state >> 33;
        state = state.wrapping_mul(0xff51_afd7_ed55_8ccd);
        state ^= state >> 33;
        state.to_le_bytes()[0]
    };
    [next(), next(), next()]
}

// Card k's patch: the red level the provider reads its name from, and the green and blue that
// follow from it.
fn patch(card: u32) -> [u8; 3] {
    let red = u8::try_from(30 + 36 * card).expect("level");
    [red, 220 - red / 2, 90]
}

// One screen: its page colour, whether the list's end shows, the content moved up by `shift`
// pixels, and its cards, each in its slot with its patch.
#[derive(Debug, Clone)]
struct Screen {
    page: [u8; 3],
    end: bool,
    shift: u32,
    cards: Vec<(u32, [u8; 3])>,
}

// The list page showing `cards` down its slots in that order, each with its own patch, its end
// showing.
fn list(cards: impl IntoIterator<Item = u32>) -> Screen {
    Screen {
        page: RED,
        end: true,
        shift: 0,
        cards: cards
            .into_iter()
            .enumerate()
            .map(|(slot, card)| (u32::try_from(slot).expect("slot"), patch(card)))
            .collect(),
    }
}

fn detail() -> Screen {
    Screen {
        page: BLUE,
        end: false,
        shift: 0,
        cards: Vec::new(),
    }
}

fn pixel(screen: &Screen, x: u32, y: u32) -> [u8; 3] {
    match (x, y) {
        (0, 0) => return screen.page,
        (1, 0) => return GREEN,
        (2, 0) => return if screen.end { BLUE } else { BACKGROUND },
        (3, 0) => return WHITE,
        _ => {}
    }
    for (slot, color) in &screen.cards {
        let top = 10 + 12 * slot - screen.shift;
        if (top..top + MARKER.1).contains(&y) && (4..4 + MARKER.0).contains(&x) {
            return noise(x - 4, y - top);
        }
        if (top..top + 4).contains(&y) && (12..16).contains(&x) {
            return *color;
        }
    }
    BACKGROUND
}

// Every pixel of `screen`, by its row-major index, as the fake device paints it.
fn render(screen: &Screen) -> Vec<(usize, [u8; 3])> {
    (0..HEIGHT)
        .flat_map(|y| (0..WIDTH).map(move |x| pixel(screen, x, y)))
        .enumerate()
        .collect()
}

fn frame(screen: &Screen) -> Frame {
    Frame::from_pixels(
        WIDTH,
        HEIGHT,
        render(screen)
            .into_iter()
            .flat_map(|(_, rgb)| rgb)
            .collect(),
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("list frame")
}

// What the device shows for the run about to start: the screens in order, one more after each
// input the run makes, the last one staying; `base` is the device's input count at the start.
struct Plan {
    base: usize,
    screens: Vec<Screen>,
}

impl Plan {
    fn pixels(&self, inputs: usize) -> Vec<(usize, [u8; 3])> {
        let index = inputs
            .saturating_sub(self.base)
            .min(self.screens.len().saturating_sub(1));
        render(&self.screens[index])
    }
}

// One device instance: a display of the package's resolution observed (Workflow #392 DA-A), on
// which the fake paints its plan.
struct Device {
    state: Arc<FakeState>,
    plan: Arc<std::sync::Mutex<Plan>>,
}

impl Device {
    fn new() -> Self {
        let state = Arc::new(FakeState::default());
        state.physical_task_geometry.store(true, Ordering::Release);
        *state
            .physical_task_extent
            .lock()
            .expect("fake physical task extent") = Some((WIDTH, HEIGHT));
        let plan = Arc::new(std::sync::Mutex::new(Plan {
            base: 0,
            screens: vec![list(0..6), detail()],
        }));
        let painted = Arc::clone(&plan);
        *state.pixel_script.lock().expect("pixel script") =
            Some(Arc::new(move |_: usize, inputs: usize| {
                painted.lock().expect("plan").pixels(inputs)
            }));
        Self { state, plan }
    }

    // The next run sees `screens`, one more after each of its inputs.
    fn shows(&self, screens: Vec<Screen>) {
        let mut plan = self.plan.lock().expect("plan");
        plan.base = self.state.input_count.load(Ordering::Acquire);
        plan.screens = screens;
    }
}

// The scan over the one-screen list (the SEL-L4b host gate's): the identity feature `member`, a
// drag forward and a reposition that admission's static checks accept in this frame, and start
// and end checks that both pass on a list whose end shows.
fn scan() -> serde_json::Value {
    serde_json::json!({
        "identity_feature": "member",
        "axis": "vertical",
        "advance": {"kind": "drag",
                    "from": {"x": 20, "y": 70, "width": 20, "height": 10},
                    "to": {"x": 20, "y": 30, "width": 20, "height": 10},
                    "duration_ms": 300, "min_content_step_px": 12,
                    "max_content_step_px": 60},
        "reposition": {"kind": "drag",
                       "from": {"x": 20, "y": 80, "width": 20, "height": 10},
                       "to": {"x": 20, "y": 10, "width": 20, "height": 10},
                       "duration_ms": 300, "max_content_step_px": 90},
        "settle": {"interval_ms": 1, "timeout_ms": 1000},
        "track": {"area": {"x": 0, "y": 0, "width": WIDTH, "height": HEIGHT},
                  "edge_margin_px": 2,
                  "signature": {"rect": {"x": 8, "y": 0, "width": 4, "height": 4},
                                "columns": 1, "rows": 1},
                  "signature_max_distance_milli": 500,
                  "position_tolerance_px": 1,
                  "min_overlap_instances": 2,
                  "spacing": {"max_axis_gap_px": 20, "columns": 1,
                              "max_trailing_gap_px": 100, "max_cross_lead_px": 10,
                              "max_cross_gap_px": 10}},
        "start": {"check": "check/list_start", "rewind": false},
        "end": {"check": "check/list_end"},
        "bounds": {"max_viewports": 8, "max_reposition_viewports": 4, "max_ms": 20_000}
    })
}

// The package `package_id`: the step `read_members` reads the list into `fact_key`, and `leave`
// taps to the detail page, the terminal its one mapping names. The task declares the card layout
// and the recognition pack holds it for the run.
fn list_package(package_id: &str, fact_key: &str) -> Vec<u8> {
    let identities = (0..=6).map(member).collect::<Vec<_>>();
    let space = serde_json::json!({"width": WIDTH, "height": HEIGHT});
    let layout = serde_json::json!({
        "id": "layout/cards", "page_id": "neutral/list", "kind": "repeated_anchor",
        "anchor": "ui/card", "max_instances": 8, "order": "top_to_bottom",
        "instance_rect": {"x": 0, "y": 0, "width": 16, "height": 8},
        "click": {"x": 2, "y": 1, "width": 4, "height": 4},
        "readable_band": {"x": 0, "y": 0, "width": WIDTH, "height": HEIGHT},
        "unknown_identity": "reject",
        "features": [
            {"name": "member", "value": "identity", "target": "card/name",
             "offset": {"x": 0, "y": 0},
             "identity": {
                 "entries": identities.iter()
                     .map(|id| serde_json::json!({"id": id, "aliases": [id]}))
                     .collect::<Vec<_>>(),
                 "recognition": {"kind": "ocr_aliases", "max_distance": 1, "minimum_margin": 1,
                                 "minimum_confidence_milli": 800}}},
            {"name": "open", "value": "passed", "target": "card/open",
             "offset": {"x": 8, "y": 0}}
        ]
    });
    let control = serde_json::json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": package_id,
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": space,
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 10_000,
        "timeout_ms": 40_000,
        "max_steps": 2
    });
    let task = serde_json::json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": space,
        "target_page": "detail",
        "scheduling_outcome": {"mappings": [
            {"outcome_key": "listed", "effect": "no_designated_effect",
             "terminal_pages": ["detail"]}
        ]},
        "candidate_layouts": [layout],
        "operations": [
            {
                "id": "read_members",
                "from": "list",
                "to": "list",
                "guard": {
                    "page_id": "list",
                    "target_id": "guard/list",
                    "expected_rect": {"x": 3, "y": 0, "width": 1, "height": 1},
                    "color_probe": "guard/list"
                },
                "read_list": {"layout_id": "layout/cards", "key_feature": "member",
                              "key_column": "id",
                              "columns": [{"column": "open", "feature": "open"}],
                              "fact_key": fact_key, "valid_for_ms": VALID_FOR_MS,
                              "scan": scan()}
            },
            {
                "id": "leave",
                "from": "list",
                "to": "detail",
                "click": {"kind": "point", "x": 40, "y": 50},
                "unguarded_trusted_coordinate": true,
                "retryable": false
            }
        ]
    });
    let color = |id: &str, x: u32, expected: [u8; 3]| {
        serde_json::json!({"type": "color", "id": id,
                           "region": {"x": x, "y": 0, "width": 1, "height": 1},
                           "expected": expected})
    };
    let pack = serde_json::json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": space,
        "defaults": {"match_metric": "ccoeff_normed", "color_max_distance": 0.0},
        "targets": [
            color("page/list", 0, RED),
            color("page/detail", 0, BLUE),
            color("check/list_start", 1, GREEN),
            color("check/list_end", 2, BLUE),
            color("guard/list", 3, WHITE),
            {"type": "template", "id": "ui/card", "template_path": "recognition/assets/card.png",
             "region": "full_frame", "threshold": 0.95},
            {"type": "ocr", "id": "card/name",
             "region": {"x": 0, "y": 0, "width": 12, "height": 4},
             "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact",
             "expected": [member(0)], "case_sensitive": true, "minimum_confidence": 0.8,
             "model_ref": "PP-OCRv6_medium", "model_sha256": "a".repeat(64)},
            {"type": "color", "id": "card/open",
             "region": {"x": 0, "y": 0, "width": 4, "height": 4}, "expected": patch(0)}
        ],
        "candidate_layouts": [layout]
    });
    let pages = serde_json::json!({
        "schema_version": "0.3",
        "pages": [
            {"id": "neutral/list", "required": ["page/list"], "optional": [], "forbidden": []},
            {"id": "neutral/detail", "required": ["page/detail"], "optional": [],
             "forbidden": []}
        ]
    });
    let marker = Frame::from_pixels(
        MARKER.0,
        MARKER.1,
        (0..MARKER.1)
            .flat_map(|y| (0..MARKER.0).flat_map(move |x| noise(x, y)))
            .collect(),
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("marker frame")
    .png_for_artifact()
    .expect("marker PNG");
    let json = |value: serde_json::Value| serde_json::to_vec(&value).expect("package JSON");
    let entries = [
        ("control.json", json(control)),
        (
            "resources/manifest.json",
            json(serde_json::json!({"schema_version": "0.3", "entry_task_id": "task"})),
        ),
        ("resources/operations/task/task.json", json(task)),
        ("resources/recognition/neutral.test.pack.json", json(pack)),
        ("resources/recognition/assets/card.png", marker),
        ("resources/recognition/neutral.test.pages.json", json(pages)),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, bytes) in entries {
        zip.start_file(path, options).expect("package entry");
        zip.write_all(&bytes).expect("package content");
    }
    zip.finish().expect("finish package").into_inner()
}

// The package of the list reading of `fact_key`, written under `root`.
fn write_package(root: &TempDir, name: &str, fact_key: &str) -> PathBuf {
    let path = root.path().join(name);
    fs::write(&path, list_package("neutral.list.task", fact_key)).expect("write the package");
    path
}

fn start(root: &TempDir, devices: &[(&str, &Device)]) -> RuntimeHost {
    let provider = FakeProvider::from_entries(devices.iter().map(|(alias, device)| {
        (
            (*alias).to_owned(),
            instance_id(),
            Arc::clone(&device.state),
        )
    }))
    .with_vision_provider(Arc::new(CardNames));
    RuntimeHost::start(config(root), Arc::new(provider)).expect("runtime host")
}

// One manual run of `package` on `alias`; its receipt and correlation.
fn run(host: &RuntimeHost, alias: &str, package: &Path) -> (RuntimeReceipt, CorrelationId) {
    let mut client = TestClient::connect(host);
    client.set_receipt_read_timeout();
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            alias,
            client.ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.display().to_string()).expect("task request"),
        ),
    );
    (client.send(&request), correlation_id)
}

fn completed(host: &RuntimeHost, receipt: &RuntimeReceipt) {
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "the run's outcome: {:?}; the Runtime's fatal error: {:?}",
        receipt.outcome(),
        host.fatal_error()
    );
}

// Every `fact.published` record of `key`, in ledger order.
fn publications(host: &RuntimeHost, key: &str) -> Vec<FactRecord> {
    let mut events = host
        .query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::FactPublished),
            ..EventQuery::default()
        })
        .expect("the published facts");
    events.sort_by_key(PersistedEvent::sequence);
    events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Fact(actingcommand_contract::FactPayload::Published(payload)) => {
                Some(payload.record().clone())
            }
            _ => None,
        })
        .filter(|record| record.key == key)
        .collect()
}

fn context(alias: &str) -> InstanceFactContext {
    InstanceFactContext {
        instance_id: alias.to_owned(),
        server_id: "test".to_owned(),
        game_id: "neutral".to_owned(),
    }
}

fn records(host: &RuntimeHost, alias: &str) -> Vec<FactRecord> {
    host.instance_fact_snapshot(context(alias))
        .expect("instance facts")
        .records
}

fn record(host: &RuntimeHost, alias: &str, key: &str) -> Option<FactRecord> {
    records(host, alias)
        .into_iter()
        .find(|record| record.key == key)
}

// One row of a list: its key and its `open` column.
fn row(card: u32, open: bool) -> BTreeMap<String, actingcommand_contract::FactScalar> {
    BTreeMap::from([
        (
            "id".to_owned(),
            actingcommand_contract::FactScalar::String(member(card)),
        ),
        (
            "open".to_owned(),
            actingcommand_contract::FactScalar::Boolean(open),
        ),
    ])
}

fn rows_of(record: &FactRecord) -> Vec<BTreeMap<String, actingcommand_contract::FactScalar>> {
    match &record.content {
        FactContent::Inline {
            value: ContractFactValue::RecordList(rows),
        } => rows.clone(),
        other => panic!("a record list, got {other:?}"),
    }
}

// The run and the frame a list's `source_snapshot_id` (`run:<run>/frame:<frame>/<operation>`)
// names.
fn snapshot_parts(record: &FactRecord) -> (String, String, String) {
    let mut parts = record.source_snapshot_id.split('/');
    let run = parts.next().expect("run part").to_owned();
    let frame = parts.next().expect("frame part").to_owned();
    let operation = parts.next().expect("operation part").to_owned();
    assert!(
        run.starts_with("run:run_") && frame.starts_with("frame:frame_"),
        "{}",
        record.source_snapshot_id
    );
    (run, frame, operation)
}

// The task diagnostic rows of the run `correlation_id` whose outcome is the warning of a list
// that gave no reading.
fn miss_rows(
    root: &TempDir,
    host: &RuntimeHost,
    correlation_id: CorrelationId,
) -> Vec<serde_json::Value> {
    let mut client = TestClient::connect(host);
    let events = projected_events(
        &mut client,
        EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        },
    );
    let stream = events
        .iter()
        .filter(|event| event.event_type == EventType::ArtifactVerified)
        .flat_map(|event| &event.artifacts)
        .find(|artifact| {
            artifact.kind == ArtifactKind::DiagnosticJson
                && artifact.redaction_state
                    == actingcommand_contract::ArtifactRedactionState::Pending
        })
        .expect("the run's task diagnostic stream");
    let document: serde_json::Value = serde_json::from_slice(
        &read_projected_verified(root.path(), stream).expect("verified stream bytes"),
    )
    .expect("task diagnostic JSON");
    document["records"]
        .as_array()
        .expect("diagnostic records")
        .iter()
        .filter(|record| {
            record["kind"] == "error" && record["data"]["error"]["outcome"]["code"] == UNRESOLVED
        })
        .cloned()
        .collect()
}

// The semantic task facts of the run `correlation_id`, in ledger order.
fn task_facts(host: &RuntimeHost, correlation_id: CorrelationId) -> Vec<TaskSemanticFact> {
    let mut events = host
        .query_persisted_events_for_test(EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        })
        .expect("the run's records");
    events.sort_by_key(PersistedEvent::sequence);
    events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Task(TaskPayload::Semantic(payload)) => Some(payload.fact().clone()),
            _ => None,
        })
        .collect()
}

// Every record of the run `correlation_id`, in ledger order.
fn run_records(host: &RuntimeHost, correlation_id: CorrelationId) -> Vec<PersistedEvent> {
    let mut events = host
        .query_persisted_events_for_test(EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        })
        .expect("the run's records");
    events.sort_by_key(PersistedEvent::sequence);
    events
}

// An identifier's canonical text, the one a `source_snapshot_id` names it by.
fn id_text(id: &impl serde::Serialize) -> String {
    serde_json::to_value(id)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .expect("an identifier's text")
}

// The run `correlation_id`, and the last frame it captured during its step `operation` (between
// the step's `task.step_started` and `task.step_finished`).
fn step_end_frame(
    host: &RuntimeHost,
    correlation_id: CorrelationId,
    operation: &str,
) -> (
    actingcommand_contract::RunId,
    actingcommand_contract::FrameId,
) {
    let records = run_records(host, correlation_id);
    let step = |finished: bool| {
        records
            .iter()
            .position(|event| match event.payload() {
                EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
                    TaskSemanticFact::StepStarted {
                        operation_label, ..
                    } => !finished && operation_label == operation,
                    TaskSemanticFact::StepFinished {
                        operation_label, ..
                    } => finished && operation_label == operation,
                    _ => false,
                },
                _ => false,
            })
            .expect("the step's record")
    };
    let frame = records[step(false)..step(true)]
        .iter()
        .rev()
        .find(|event| event.event_type() == EventType::CaptureCompleted)
        .and_then(|event| event.links().frame_id())
        .copied()
        .expect("a frame the step captured");
    let run = records
        .iter()
        .find_map(|event| event.links().run_id())
        .copied()
        .expect("the run's id");
    (run, frame)
}

// The times of the records `capture.requested` and `capture.completed` of `frame` in the run
// `correlation_id`: the frame's device capture lies between them.
fn capture_window(
    host: &RuntimeHost,
    correlation_id: CorrelationId,
    frame: actingcommand_contract::FrameId,
) -> (u64, u64) {
    let records = run_records(host, correlation_id);
    let at = |event_type: EventType| {
        records
            .iter()
            .find(|event| {
                event.event_type() == event_type && event.links().frame_id() == Some(&frame)
            })
            .map(PersistedEvent::timestamp_unix_ms)
            .expect("the frame's capture record")
    };
    (
        at(EventType::CaptureRequested),
        at(EventType::CaptureCompleted),
    )
}

// S168-4 test 1, the host: a run whose step reads the list of `member-00` to `member-05`
// publishes, after its successful terminal, one `fact.published` of the list's key with one
// `record_list` record: the run's instance's scope; the six rows in ascending byte order of the
// key, each with its `open` column (only card 0's patch is the open colour); expiring
// `valid_for_ms` after its observation, with that lifetime from the detector contract; at the
// lowest confidence of the features the rows read (the identities' 990; a colour target's
// `passed` carries none); `source_detector` `list_reading:task/read_members`; and
// `source_snapshot_id` naming the run, a frame and the step. The step started and finished on
// the list page once, with no input; `leave` then made the run's one input.
#[test]
fn list_reading_publishes_whole_list() {
    let root = TempDir::new().expect("tempdir");
    let package = write_package(&root, "list.zip", LIST_KEY);
    let device = Device::new();
    let host = start(&root, &[(ALIAS, &device)]);
    // The list shows its members out of their key order.
    device.shows(vec![list([3, 0, 5, 1, 4, 2]), detail()]);

    let (receipt, correlation_id) = run(&host, ALIAS, &package);
    completed(&host, &receipt);

    let published = publications(&host, LIST_KEY);
    let [record] = published.as_slice() else {
        panic!("one publication of the list, got {published:#?}");
    };
    assert_eq!(
        record.scope,
        FactScope::Instance {
            instance_id: ALIAS.to_owned()
        }
    );
    assert_eq!(
        rows_of(record),
        vec![
            row(0, true),
            row(1, false),
            row(2, false),
            row(3, false),
            row(4, false),
            row(5, false)
        ]
    );
    assert_eq!(
        record.expires_at_unix_ms,
        Some(record.observed_at_unix_ms + VALID_FOR_MS)
    );
    assert_eq!(
        record.ttl_policy,
        Some(FactTtlPolicy {
            minimum_ms: VALID_FOR_MS,
            maximum_ms: VALID_FOR_MS,
            source: FactTtlSource::DetectorContract,
        })
    );
    assert_eq!(record.confidence_milli, 990);
    assert_eq!(record.source_detector, "list_reading:task/read_members");
    assert_eq!(record.schema_version, "fact.v1");
    assert!(record.invalidate_on.is_empty());
    // The frame the end check passed on: the last frame the step captured, its one viewport. The
    // list is observed when that frame was captured, between the frame's `capture.requested` and
    // `capture.completed`, and its snapshot names this run and that frame.
    let (run_id, end_frame) = step_end_frame(&host, correlation_id, "read_members");
    let (requested_at, completed_at) = capture_window(&host, correlation_id, end_frame);
    assert!(
        (requested_at..=completed_at).contains(&record.observed_at_unix_ms),
        "observed at {}, the end frame captured within {requested_at}..={completed_at}",
        record.observed_at_unix_ms
    );
    assert_eq!(
        snapshot_parts(record),
        (
            format!("run:{}", id_text(&run_id)),
            format!("frame:{}", id_text(&end_frame)),
            "read_members".to_owned()
        )
    );
    assert_eq!(
        record,
        &self::record(&host, ALIAS, LIST_KEY).expect("the active record")
    );

    let steps = task_facts(&host, correlation_id)
        .into_iter()
        .filter_map(|fact| match fact {
            TaskSemanticFact::StepStarted {
                operation_label, ..
            } => Some(("step_started", operation_label, None)),
            TaskSemanticFact::EffectIntent {
                operation_label, ..
            } => Some(("effect_intent", operation_label, None)),
            TaskSemanticFact::StepFinished {
                operation_label,
                page_label,
                ..
            } => Some(("step_finished", operation_label, Some(page_label))),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        steps,
        [
            ("step_started", "read_members".to_owned(), None),
            (
                "step_finished",
                "read_members".to_owned(),
                Some("neutral/list".to_owned())
            ),
            ("step_started", "leave".to_owned(), None),
            ("effect_intent", "leave".to_owned(), None),
            (
                "step_finished",
                "leave".to_owned(),
                Some("neutral/detail".to_owned())
            ),
        ]
    );
    assert_eq!(
        device.state.input_count.load(Ordering::Acquire),
        1,
        "the list made no move: the one input is leave's tap"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

// S168-4 test 2, the host: a list page with no card, both checks passing on the first viewport,
// is a known empty list: one record with zero rows and confidence 1000, and the list step made no
// input (the run's one input is `leave`'s tap).
#[test]
fn list_reading_empty_list_is_known() {
    let root = TempDir::new().expect("tempdir");
    let package = write_package(&root, "list.zip", LIST_KEY);
    let device = Device::new();
    let host = start(&root, &[(ALIAS, &device)]);
    device.shows(vec![list([]), detail()]);

    let (receipt, _) = run(&host, ALIAS, &package);
    completed(&host, &receipt);

    let published = publications(&host, LIST_KEY);
    let [record] = published.as_slice() else {
        panic!("one publication of the list, got {published:#?}");
    };
    assert!(rows_of(record).is_empty());
    assert_eq!(record.confidence_milli, 1000);
    assert_eq!(
        device.state.input_count.load(Ordering::Acquire),
        1,
        "the one input is leave's tap"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

// S168-4 test 3, the host: after a run published the list, a run whose list shows a card whose
// name reads as nothing, and a run whose list shows two cards that read `member-00`, each succeed
// and publish nothing: the key's record from before stays byte for byte, and each writes one
// warning row of the task diagnostic, `contained_task_resource_reading_unresolved` with the
// step's id, the key, `list_position_unread` or `list_duplicate_key` and the count 1, an error row
// of the post-admission phase without a target.
#[test]
fn list_reading_miss_publishes_nothing() {
    let root = TempDir::new().expect("tempdir");
    let package = write_package(&root, "list.zip", LIST_KEY);
    let device = Device::new();
    let host = start(&root, &[(ALIAS, &device)]);
    device.shows(vec![list(0..6), detail()]);
    let (receipt, _) = run(&host, ALIAS, &package);
    completed(&host, &receipt);
    let before = record(&host, ALIAS, LIST_KEY).expect("the list's record");

    let mut unread = list(0..6);
    unread.cards[2].1 = UNREADABLE;
    let mut repeated = list(0..6);
    repeated.cards[1].1 = patch(0);
    for (screen, problem) in [
        (unread, "list_position_unread"),
        (repeated, "list_duplicate_key"),
    ] {
        device.shows(vec![screen, detail()]);
        let (receipt, correlation_id) = run(&host, ALIAS, &package);
        completed(&host, &receipt);
        assert_eq!(
            record(&host, ALIAS, LIST_KEY).as_ref(),
            Some(&before),
            "{problem}: the record from before stays"
        );
        assert_eq!(publications(&host, LIST_KEY).len(), 1, "{problem}");
        let rows = miss_rows(&root, &host, correlation_id);
        let [miss] = rows.as_slice() else {
            panic!("{problem}: exactly one miss row: {rows:?}");
        };
        let outcome = &miss["data"]["error"]["outcome"];
        assert_eq!(outcome["category"], "warning", "{problem}");
        assert_eq!(outcome["values"]["reading_id"], "read_members", "{problem}");
        assert_eq!(outcome["values"]["fact_key"], LIST_KEY, "{problem}");
        assert_eq!(outcome["values"]["reading_problem"], problem);
        assert_eq!(outcome["values"]["count"], 1, "{problem}");
        assert_eq!(miss["data"]["phase"], "post_admission", "{problem}");
        assert!(miss["data"]["target_id"].is_null(), "{problem}");
    }
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

// S168-4 test 4, the host: the list's end does not show, so the step advances, and the next
// viewport shows the list moved 6 px, outside the advance's window: the run fails
// `selection_scan_alignment_rejected` at the stage `read_list`, and publishes no list.
#[test]
fn list_reading_failed_scan_fails_run() {
    let root = TempDir::new().expect("tempdir");
    let package = write_package(&root, "list.zip", LIST_KEY);
    let device = Device::new();
    let host = start(&root, &[(ALIAS, &device)]);
    let mut open = list(0..6);
    open.end = false;
    let mut moved = open.clone();
    moved.shift = 6;
    device.shows(vec![open, moved]);

    let (receipt, _) = run(&host, ALIAS, &package);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Failed,
        "{:?}",
        receipt.outcome()
    );
    let outcome = receipt.outcome();
    assert_eq!(
        outcome.code().as_str(),
        "selection_scan_alignment_rejected",
        "{outcome:?}"
    );
    assert_eq!(
        outcome.value("stage"),
        Some(&serde_json::json!("read_list")),
        "{outcome:?}"
    );
    assert!(publications(&host, LIST_KEY).is_empty());
    assert!(record(&host, ALIAS, LIST_KEY).is_none());
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

// S168-4 test 6: a `PublishFact` and a `PublishFacts` of `inventory.list.example_kind_a` from
// every origin that may publish facts are refused `list_reading_key_reserved` with the key, a
// denied request, and so is the Runtime's own ordinary publication path, non-fatal; nothing is
// appended. A client publishes an ordinary fact from the agent dispatcher's origin (Agent,
// Adapter), as every host test that publishes a fact as a client does: the agent, and the CLI,
// whose `facts publish` sends that origin (lab-ops, `fact_publish`). A CLI or UI origin of its own,
// (Cli, Cli) or (User, Ui), publishes only the families with the origin exception, so either
// request of the key from it is refused earlier, when it is formed
// (`invalid_agent_dispatcher_origin`), as for any publication, and nothing is appended either.
#[test]
fn list_reading_key_reserved() {
    let root = TempDir::new().expect("tempdir");
    let device = Device::new();
    let host = start(&root, &[(ALIAS, &device)]);
    let published = |host: &RuntimeHost| {
        host.query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::FactPublished),
            ..EventQuery::default()
        })
        .expect("the published facts")
        .len()
    };
    let before = published(&host);
    let list = stored_fact(
        FactScope::Instance {
            instance_id: ALIAS.to_owned(),
        },
        LIST_KEY,
        ContractFactValue::RecordList(vec![row(0, true)]),
        "snapshot:s168-4-client",
        Vec::new(),
    );
    let mut client = TestClient::connect(&host);
    // `PublishFact` (`batch` false) or `PublishFacts` (`batch` true) of the list's record.
    let publication = |batch: bool| {
        if batch {
            RuntimeOperation::PublishFacts {
                observation: actingcommand_contract::FactObservation {
                    records: vec![list.clone()],
                },
            }
        } else {
            RuntimeOperation::PublishFact {
                record: list.clone(),
            }
        }
    };
    for batch in [false, true] {
        let request = client.agent_request(publication(batch));
        let receipt = client.send(&request);
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Denied,
            "batch {batch}: {receipt:?}"
        );
        assert_eq!(
            receipt.outcome().code().as_str(),
            RESERVED,
            "batch {batch}: {receipt:?}"
        );
        assert_eq!(
            receipt.outcome().value("fact_key"),
            Some(&serde_json::json!(LIST_KEY)),
            "batch {batch}: {receipt:?}"
        );
        for (origin, actor, source) in [
            ("cli", EventActor::Cli, EventSource::Cli),
            ("ui", EventActor::User, EventSource::Ui),
        ] {
            let formed = RuntimeRequest::new(
                client.ids.mint_request_id().expect("request id"),
                client.ids.mint_correlation_id().expect("correlation id"),
                None,
                actor,
                source,
                unix_ms_now().expect("wall clock"),
                publication(batch),
            );
            let refused = match formed {
                Ok(_) => panic!("{origin}, batch {batch}: a person's origin cannot publish it"),
                Err(refused) => refused,
            };
            assert_eq!(
                refused.code().as_str(),
                "invalid_agent_dispatcher_origin",
                "{origin}, batch {batch}"
            );
        }
    }
    let error = host
        .publish_fact(list)
        .expect_err("only a list reading writes under the prefix");
    assert_eq!(error.code(), RESERVED, "{error:?}");
    assert!(!error.is_fatal(), "{error:?}");
    assert_eq!(published(&host), before, "nothing is appended");
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(client);
    host.close().expect("close the host");
}

// A select step's document over the card layout that keeps a card while the published list
// lists it with `open` (`keyed_fact`; an unlisted card reads `absent`, false), within
// `max_age_ms` of the list's read. A gate that reads `keyed_fact` declares
// `on_unknown: abort_evaluation` (selection-policy.md, "Fail-closed declaration").
fn listed_open_policy(max_age_ms: u64) -> serde_json::Value {
    let identities = (0..=6).map(member).collect::<Vec<_>>();
    serde_json::json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "cards-listed-open",
        "applies_to": {
            "candidate_layout_id": "layout/cards",
            "outcome_keys": {"selected": "card_selected", "empty": "card_empty",
                             "insufficient": "card_insufficient",
                             "ambiguous": "card_ambiguous", "unknown": "card_unknown"}
        },
        "fields": [{"name": "member",
                    "value_type": {"type": "enum_string", "allowed": identities}}],
        "facts": [{"fact_key": LIST_KEY,
                   "value_type": {"type": "record_list", "key_column": "id",
                                  "columns": {"open": {"type": "boolean"}}},
                   "max_age_ms": max_age_ms, "minimum_confidence_milli": 500}],
        "gates": [{"gate_id": "listed_open",
                   "predicate": {"kind": "boolean_equals",
                                 "value": {"source": "keyed_fact", "fact_key": LIST_KEY,
                                           "key_field": "member", "value_column": "open",
                                           "absent": {"type": "boolean", "value": false}},
                                 "expected": true},
                   "on_unknown": {"kind": "abort_evaluation"}}],
        "scoring": [],
        "selection": {"mode": "none_allowed", "required_count": 1},
        "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
    })
}

// S168-4 test 7: a run reads the list of `member-00` to `member-04`. A select step of another
// package then reads the published list through `keyed_fact`, evaluated on a frame of
// `member-00` to `member-05` with the instance's snapshot, as its run would read it: the listed
// `member-00` reads its `open` column (true) and is the one member; the listed `member-01`
// reads false; the unlisted `member-05` reads `absent` (false), a known value, so it is rejected
// by the gate, never dropped as unknown. The same list older than the document's `max_age_ms`
// ends the evaluation `unknown` with `fact_stale`, before any candidate.
#[test]
fn list_reading_read_by_keyed_fact() {
    let root = TempDir::new().expect("tempdir");
    let package = write_package(&root, "list.zip", LIST_KEY);
    let device = Device::new();
    let host = start(&root, &[(ALIAS, &device)]);
    device.shows(vec![list(0..5), detail()]);
    let (receipt, _) = run(&host, ALIAS, &package);
    completed(&host, &receipt);
    let list_record = record(&host, ALIAS, LIST_KEY).expect("the list's record");
    let snapshot = host
        .instance_fact_snapshot(context(ALIAS))
        .expect("instance facts");

    const MAX_AGE_MS: u64 = 60_000;
    let select = PreparedContainedTask::load_with_vision_provider(
        ALIAS,
        &single_frame_select_package(listed_open_policy(MAX_AGE_MS), false),
        Arc::new(CardNames),
    )
    .expect("the select package is admitted");
    let evaluate = |now_unix_ms: u64| {
        let dry_run = actingcommand_execution_kernel::dry_run_select(
            &select,
            "pick",
            &frame(&list(0..6)),
            (&snapshot, now_unix_ms),
            &select.default_run_parameters(),
            None,
        )
        .expect("the select step evaluates on the frame");
        serde_json::to_value(&dry_run.decision).expect("decision JSON")
    };
    let status = |decision: &serde_json::Value, candidate: &str| {
        decision["candidates"]
            .as_array()
            .expect("candidates")
            .iter()
            .find(|verdict| verdict["candidate_id"] == candidate)
            .map(|verdict| verdict["status"].clone())
            .unwrap_or_else(|| panic!("no verdict of {candidate}: {decision}"))
    };

    let fresh = evaluate(list_record.observed_at_unix_ms + 1);
    assert_eq!(
        fresh["outcome"],
        serde_json::json!({"kind": "selected", "count": 1}),
        "{fresh}"
    );
    assert_eq!(fresh["selected"], serde_json::json!(["layout/cards#00"]));
    assert_eq!(status(&fresh, "layout/cards#00"), "ranked");
    assert_eq!(status(&fresh, "layout/cards#01"), "gate_rejected");
    assert_eq!(status(&fresh, "layout/cards#05"), "gate_rejected");

    let stale = evaluate(list_record.observed_at_unix_ms + MAX_AGE_MS + 1);
    assert_eq!(stale["outcome"]["kind"], "unknown", "{stale}");
    assert_eq!(stale["outcome"]["reason"], "fact_stale", "{stale}");
    assert_eq!(stale["candidates"], serde_json::json!([]), "{stale}");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

// S168-4 test 8: a second complete read whose rows did not change publishes a second
// observation of the key, with a `source_snapshot_id` of its own run and a strictly newer
// observation time, and raises no `fact_source_snapshot_identity_conflict` (the Runtime is not
// poisoned); the key's record is the second read's.
#[test]
fn list_reading_unchanged_rows_republished() {
    let root = TempDir::new().expect("tempdir");
    let package = write_package(&root, "list.zip", LIST_KEY);
    let device = Device::new();
    let host = start(&root, &[(ALIAS, &device)]);
    for _ in 0..2 {
        device.shows(vec![list(0..6), detail()]);
        let (receipt, _) = run(&host, ALIAS, &package);
        completed(&host, &receipt);
    }
    let published = publications(&host, LIST_KEY);
    let [first, second] = published.as_slice() else {
        panic!("two publications of the list, got {published:#?}");
    };
    assert_eq!(rows_of(first), rows_of(second), "the rows did not change");
    assert_ne!(first.source_snapshot_id, second.source_snapshot_id);
    let (first_run, _, _) = snapshot_parts(first);
    let (second_run, _, _) = snapshot_parts(second);
    assert_ne!(first_run, second_run, "each read names its own run");
    assert!(second.observed_at_unix_ms > first.observed_at_unix_ms);
    assert_eq!(
        record(&host, ALIAS, LIST_KEY).as_ref(),
        Some(second),
        "the record's age is the second read's"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

// S168-4 test 9: on a store with no free fact identity, a run whose list is a new identity is
// refused at its publication: the run fails `contained_task_resource_reading_rejected`, native
// detail `read_members:fact_store_capacity_exceeded`, no active fact is evicted, and a run of
// another instance goes on. Each instance reads once before the
// store is filled, so its display facts and its own list's identity exist; the store is then
// filled with ordinary facts up to its bound (the first refusal is `fact_store_capacity_exceeded`).
#[test]
fn list_reading_full_store_refused() {
    let root = TempDir::new().expect("tempdir");
    let first_list = write_package(&root, "list-a.zip", LIST_KEY);
    let second_list = write_package(&root, "list-b.zip", OTHER_KEY);
    let device = Device::new();
    let other = Device::new();
    let host = start(&root, &[(ALIAS, &device), (OTHER_ALIAS, &other)]);
    for (alias, instance) in [(ALIAS, &device), (OTHER_ALIAS, &other)] {
        instance.shows(vec![list(0..6), detail()]);
        let (receipt, _) = run(&host, alias, &first_list);
        completed(&host, &receipt);
    }

    let now = unix_ms_now().expect("wall clock");
    let mut filled = false;
    for index in 0..300_u32 {
        let fill = FactRecord {
            scope: FactScope::Instance {
                instance_id: ALIAS.to_owned(),
            },
            key: format!("session.example.fill_{index:03}"),
            content: FactContent::Inline {
                value: ContractFactValue::Integer(1),
            },
            observed_at_unix_ms: now,
            expires_at_unix_ms: None,
            ttl_policy: None,
            confidence_milli: 1_000,
            source_detector: "detector.fixture".to_owned(),
            source_snapshot_id: format!("snapshot:s168-4-fill-{index:03}"),
            schema_version: "fact.v1".to_owned(),
            resource_bundle_hash: "a".repeat(64),
            invalidate_on: Vec::new(),
        };
        match host.publish_fact(fill) {
            Ok(_) => {}
            Err(error) if error.code() == "fact_store_capacity_exceeded" => {
                assert!(!error.is_fatal(), "{error:?}");
                filled = true;
                break;
            }
            Err(error) => panic!("fill fact {index}: {error:?}"),
        }
    }
    assert!(filled, "the store reached its bound");
    let before = records(&host, ALIAS);

    device.shows(vec![list(0..6), detail()]);
    let (receipt, _) = run(&host, ALIAS, &second_list);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Failed,
        "{:?}",
        receipt.outcome()
    );
    assert_eq!(
        receipt.outcome().code().as_str(),
        "contained_task_resource_reading_rejected",
        "{:?}",
        receipt.outcome()
    );
    // The reason is the run failure's native detail, `<operation id>:<fact store code>`, which
    // the receipt does not carry; the host hands it to the test by the request.
    assert_eq!(
        host.failure_detail_for_test(receipt.request_id())
            .expect("the failure's detail"),
        Some("read_members:fact_store_capacity_exceeded".to_owned())
    );
    assert!(publications(&host, OTHER_KEY).is_empty());
    assert_eq!(records(&host, ALIAS), before, "no active fact is evicted");
    assert!(host.fatal_error().expect("runtime health").is_none());

    other.shows(vec![list(0..6), detail()]);
    let (receipt, _) = run(&host, OTHER_ALIAS, &first_list);
    completed(&host, &receipt);
    assert_eq!(
        publications(&host, LIST_KEY)
            .iter()
            .filter(|record| record.scope
                == FactScope::Instance {
                    instance_id: OTHER_ALIAS.to_owned()
                })
            .count(),
        2,
        "the other instance's list is read again in place"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

// Workflow #428 S168-LB1a (since 0.12.55; register §1.309 LB1-5b, §1.311;
// `contracts/resource-readings.md`, "List readings", "Rows"): a list read whole whose record is
// larger than one observation holds is a miss of the list, not a failure of its run. The test
// lowers, for its own key only, the bound the host measures a list's record against, so the fake
// device's six rows meet it (`set_list_observation_bytes_for_test`; the 512 KiB itself is
// `gate_428_s168_lb1a::lb1a_a_list_is_measured_against_the_observation_bytes`). After a run
// published the list, a run whose list is over the bound succeeds and publishes nothing: the key's
// record from before stays byte for byte, and the step writes one warning row,
// `contained_task_resource_reading_unresolved` with the step's id, the key,
// `list_bytes_exceeded` and `count` 6, the rows read, an error row of the post-admission phase.
#[test]
fn list_reading_over_observation_bytes_is_a_miss() {
    const BYTES_KEY: &str = "inventory.list.example_kind_e";
    let root = TempDir::new().expect("tempdir");
    let package = write_package(&root, "list.zip", BYTES_KEY);
    let device = Device::new();
    let host = start(&root, &[(ALIAS, &device)]);
    device.shows(vec![list(0..6), detail()]);
    let (receipt, _) = run(&host, ALIAS, &package);
    completed(&host, &receipt);
    let before = record(&host, ALIAS, BYTES_KEY).expect("the list's record");

    crate::host::set_list_observation_bytes_for_test(BYTES_KEY, 64);
    device.shows(vec![list(0..6), detail()]);
    let (receipt, correlation_id) = run(&host, ALIAS, &package);
    completed(&host, &receipt);
    assert_eq!(
        record(&host, ALIAS, BYTES_KEY).as_ref(),
        Some(&before),
        "the record from before stays"
    );
    assert_eq!(
        publications(&host, BYTES_KEY).len(),
        1,
        "nothing is published"
    );
    let rows = miss_rows(&root, &host, correlation_id);
    let [miss] = rows.as_slice() else {
        panic!("exactly one miss row: {rows:?}");
    };
    let outcome = &miss["data"]["error"]["outcome"];
    assert_eq!(outcome["category"], "warning");
    assert_eq!(outcome["values"]["reading_id"], "read_members");
    assert_eq!(outcome["values"]["fact_key"], BYTES_KEY);
    assert_eq!(outcome["values"]["reading_problem"], "list_bytes_exceeded");
    assert_eq!(outcome["values"]["count"], 6);
    assert_eq!(miss["data"]["phase"], "post_admission");
    assert!(miss["data"]["target_id"].is_null());
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}
