// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #450 FL-2 (since 0.12.60; `contracts/resource-readings.md`, "Lists from fields", "Kernel
// and host"), the host's halves on a fake device: a field list's traces are the ones the host
// takes from a `read_list` step, so its list is published, kept, refused and read as a
// `read_list`'s, and no reader can tell the producer. The host needed no product change.
//
// Two made-up packages run on one fake device of 64 x 96 pixels, which paints the screen its test
// plans, one more screen after each input:
//
// - `neutral.list.fields` reads one item per detail page. The first row of a page shows its kind at
//   x 0 (`item`, or `last`, the last item), the item's name code at x 1 (its red level; a code past
//   the dictionary reads `unlisted`), its amount at x 2 (red level), the two confidences at x 3 (red
//   for the name, green for the amount, in hundredths), the previous arrow at x 4 and a final mark
//   at x 5. The fields `name` (a dictionary entry, `member-00` to `member-06`) and `amount` (an
//   integer) of the group `row` are read on both pages; the step `next` taps from `item` to the
//   next item or to `last`. Its field list `list_a` is keyed by the name (`id`), with the column
//   `count` of the amount or without columns, a floor of 900, starts where the previous arrow does
//   not show and ends on `last`.
// - `neutral.list.task` is the `read_list` package of the S168-4 host gate (`gate_428_s168_4`): a
//   one-screen list of cards whose names the scripted provider reads from their patches, read by
//   the step `read_members` into keys only, then the tap `leave` to the detail page.
//
// The scripted provider reads a field from the first row and delegates every other read to the
// SEL-L4b gate's card reader. The existing list-reading rows (`gate_428_s168_4`,
// `gate_428_s168_lb1a`) are unchanged: a package without the member runs as before. Every id, key
// and name is made up; nothing waits on a clock of its own.

use super::gate_308_l4b::{ALIAS, CardNames, HEIGHT, WIDTH, member, single_frame_select_package};
use super::*;
use actingcommand_contract::{
    ArtifactRedactionState, ContainedTaskCancellationReason, FactScalar, FrameId,
    MAX_TASK_DIAGNOSTIC_PAGE_RECORDS, RequestId, TaskDiagnosticCursor, TaskDiagnosticErrorData,
    TaskDiagnosticPayload, TaskDiagnosticRecord,
};
use actingcommand_ledger_forensics::{
    ForensicEventFilter, ForensicEventsRequest, ForensicOutput, ForensicReport, ForensicRequest,
    MAX_FORENSIC_EVENTS, TaskDiagnosticPage, TaskRecordsRequest,
};
use actingcommand_recognition_pack::{
    OcrExecutionProviderKind, OcrProviderExecutionEvidence, OcrProviderObservation,
    OcrProviderTextBlock,
};
use serde_json::{Value, json};

const LIST_KEY: &str = "inventory.list.example_kind_a";
const OTHER_KEY: &str = "inventory.list.example_kind_b";
const UNRESOLVED: &str = "contained_task_resource_reading_unresolved";
const VALID_FOR_MS: u64 = 86_400_000;
// The field list's id and the `read_list` step's.
const LIST_ID: &str = "list_a";
const READ_ID: &str = "read_members";

// The first row of a detail page.
const NAME_X: usize = 1;
const AMOUNT_X: usize = 2;
const CONFIDENCE_X: usize = 3;
const PREVIOUS_X: usize = 4;
const FINAL_X: usize = 5;
// Name codes below it read `member-0<code>`; the code `UNLISTED` reads a name out of the
// dictionary.
const KNOWN_NAMES: u8 = 7;
const UNLISTED: u8 = 200;

const ITEM: [u8; 3] = [200, 0, 200];
const LAST: [u8; 3] = [0, 200, 200];
const DONE: [u8; 3] = [200, 200, 0];
const ARROW: [u8; 3] = [0, 255, 0];
const RED: [u8; 3] = [255, 0, 0];
const GREEN: [u8; 3] = [0, 255, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const WHITE: [u8; 3] = [255, 255, 255];
const BACKGROUND: [u8; 3] = [30, 31, 32];
// A patch colour no card has: its name reads as nothing.
const UNREADABLE: [u8; 3] = [250, 100, 90];
// The anchor template's size.
const MARKER: (u32, u32) = (8, 6);

// The connection each run is processed on, one per run.
static NEXT_CONNECTION: AtomicU64 = AtomicU64::new(4_501);
// How long a test waits for the stop's answer, which the host gives on another thread.
const GATE_WAIT: Duration = Duration::from_secs(30);

// ---- The screens.

// One detail page of the field list's package.
#[derive(Debug, Clone, Copy)]
struct Item {
    last: bool,
    name: u8,
    amount: u8,
    confidence: [u8; 2],
    previous: bool,
    final_mark: bool,
}

// An item page with nothing marked, its name and amount read at 0.99.
const SHOWN: Item = Item {
    last: false,
    name: 0,
    amount: 0,
    confidence: [99, 99],
    previous: false,
    final_mark: false,
};

#[derive(Debug, Clone)]
enum Screen {
    Item(Item),
    // The `read_list` package's list page, its end showing, with one card per patch.
    Cards(Vec<[u8; 3]>),
    // The `read_list` package's detail page.
    Detail,
}

// The items `(name code, amount)` in visit order: the first without the previous arrow, every
// later one with it, the last one on `last`.
fn items(shown: &[(u8, u8)]) -> Vec<Screen> {
    let last = shown.len().saturating_sub(1);
    shown
        .iter()
        .enumerate()
        .map(|(index, &(name, amount))| {
            Screen::Item(Item {
                last: index == last,
                name,
                amount,
                previous: index > 0,
                ..SHOWN
            })
        })
        .collect()
}

// Three items, `member-00`, `member-01` and `member-02`.
fn three_items() -> Vec<Screen> {
    items(&[(0, 5), (1, 7), (2, 9)])
}

// `screens` with the item at `index` changed.
fn changed(mut screens: Vec<Screen>, index: usize, change: impl FnOnce(&mut Item)) -> Vec<Screen> {
    match screens.get_mut(index) {
        Some(Screen::Item(item)) => change(item),
        other => panic!("an item at {index}, got {other:?}"),
    }
    screens
}

// Card k's patch: the red level the card reader reads its name from (`gate_308_l4b`).
fn patch(card: u32) -> [u8; 3] {
    let red = u8::try_from(30 + 36 * card).expect("level");
    [red, 220 - red / 2, 90]
}

// The list page showing `shown` down its slots in that order.
fn cards(shown: impl IntoIterator<Item = u32>) -> Screen {
    Screen::Cards(shown.into_iter().map(patch).collect())
}

// Deterministic pixel noise, the SEL-L3 gate's: the cards' anchor template.
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

// A pixel of the `read_list` package's pages (the S168-4 gate's): (0, 0) the page, (1, 0) the
// list's start, (2, 0) its end, (3, 0) the step's guard, then the cards.
fn card_pixel(page: [u8; 3], end: bool, patches: &[[u8; 3]], x: u32, y: u32) -> [u8; 3] {
    match (x, y) {
        (0, 0) => return page,
        (1, 0) => return GREEN,
        (2, 0) => return if end { BLUE } else { BACKGROUND },
        (3, 0) => return WHITE,
        _ => {}
    }
    for (slot, color) in (0_u32..).zip(patches) {
        let top = 10 + 12 * slot;
        if (top..top + MARKER.1).contains(&y) && (4..4 + MARKER.0).contains(&x) {
            return noise(x - 4, y - top);
        }
        if (top..top + 4).contains(&y) && (12..16).contains(&x) {
            return *color;
        }
    }
    BACKGROUND
}

// A pixel of an item page: its first row as the header says, the rest background.
fn item_pixel(item: &Item, x: u32, y: u32) -> [u8; 3] {
    let mut row = [BACKGROUND; 6];
    row[0] = if item.last { LAST } else { ITEM };
    row[NAME_X] = [item.name, 0, 0];
    row[AMOUNT_X] = [item.amount, 0, 0];
    row[CONFIDENCE_X] = [item.confidence[0], item.confidence[1], 0];
    if item.previous {
        row[PREVIOUS_X] = ARROW;
    }
    if item.final_mark {
        row[FINAL_X] = ARROW;
    }
    match usize::try_from(x) {
        Ok(column) if y == 0 && column < row.len() => row[column],
        _ => BACKGROUND,
    }
}

fn pixel(screen: &Screen, x: u32, y: u32) -> [u8; 3] {
    match screen {
        Screen::Item(item) => item_pixel(item, x, y),
        Screen::Cards(patches) => card_pixel(RED, true, patches, x, y),
        Screen::Detail => card_pixel(BLUE, false, &[], x, y),
    }
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

// ---- The scripted provider.

// Reads a field from the first row of an item page, each read its own invocation of one CPU
// stream (the fields collector requires the execution evidence); every other read, a card's
// name, is the SEL-L4b gate's card reader's.
#[derive(Debug, Default)]
struct ShownFields {
    reads: AtomicU64,
}

// A field's target is one pixel of the first row; a card's name region is 12 x 4.
fn is_field_read(request: &OcrProviderRequest<'_>) -> bool {
    request.region.y == 0 && request.region.width == 1 && request.region.height == 1
}

impl ShownFields {
    fn field_read(&self, request: OcrProviderRequest<'_>) -> OcrProviderObservation {
        let column = usize::try_from(request.region.x).expect("a column of the first row");
        let (text, hundredths) = if column == NAME_X {
            let code = request.frame.rgb8_pixels[NAME_X * 3];
            let name = if code < KNOWN_NAMES {
                member(u32::from(code))
            } else {
                "unlisted".to_owned()
            };
            (name, request.frame.rgb8_pixels[CONFIDENCE_X * 3])
        } else {
            (
                request.frame.rgb8_pixels[AMOUNT_X * 3].to_string(),
                request.frame.rgb8_pixels[CONFIDENCE_X * 3 + 1],
            )
        };
        let confidence = Some(f32::from(hundredths) / 100.0);
        OcrProviderObservation {
            result: OcrProviderResult {
                ppocr_diagnostics: Vec::new(),
                text: text.clone(),
                blocks: vec![OcrProviderTextBlock {
                    text,
                    rect: request.region,
                    confidence,
                }],
                confidence,
                detection: None,
            },
            execution: Some(OcrProviderExecutionEvidence {
                invocation_id: format!("list-ocr-{}", self.reads.fetch_add(1, Ordering::Relaxed)),
                session_id: "list-session".to_owned(),
                session_generation: 1,
                requested_provider: OcrExecutionProviderKind::Cpu,
                resolved_provider: OcrExecutionProviderKind::Cpu,
                requested_cuda_ordinal: None,
                requested_cuda_identity: None,
                resolved_cuda_ordinal: None,
                resolved_cuda_identity: None,
                provider_implementation: "fixture-ocr".to_owned(),
                provider_binary_sha256: "b".repeat(64),
                runtime_version: "fixture-runtime".to_owned(),
                model_ref: request.model_ref.to_owned(),
                model_sha256: request.model_sha256.to_owned(),
                cpu_ep_registered: true,
                cpu_fallback_disabled: false,
                fallback_forbidden: true,
                fallback_observed: None,
                complete: true,
            }),
        }
    }
}

impl VisionProvider for ShownFields {
    fn require_ocr_model(
        &self,
        model_ref: &str,
        model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        CardNames.require_ocr_model(model_ref, model_sha256)
    }

    fn require_nn_model(
        &self,
        model_ref: &str,
        model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        CardNames.require_nn_model(model_ref, model_sha256)
    }

    fn read_text(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderResult, VisionProviderError> {
        self.read_text_with_execution_evidence(request)
            .map(|observation| observation.result)
    }

    fn read_text_with_execution_evidence(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderObservation, VisionProviderError> {
        if is_field_read(&request) {
            Ok(self.field_read(request))
        } else {
            CardNames.read_text_with_execution_evidence(request)
        }
    }

    fn classify(
        &self,
        request: NnProviderRequest<'_>,
    ) -> Result<NnProviderResult, VisionProviderError> {
        CardNames.classify(request)
    }
}

// ---- The device and the host.

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

// One device instance: a display of the packages' resolution observed (Workflow #392 DA-A), on
// which the fake paints its plan; its id stays the same across a restart.
struct Device {
    id: InstanceId,
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
            screens: vec![Screen::Detail],
        }));
        let painted = Arc::clone(&plan);
        *state.pixel_script.lock().expect("pixel script") =
            Some(Arc::new(move |_: usize, inputs: usize| {
                painted.lock().expect("plan").pixels(inputs)
            }));
        Self {
            id: instance_id(),
            state,
            plan,
        }
    }

    // The next run sees `screens`, one more after each of its inputs.
    fn shows(&self, screens: Vec<Screen>) {
        let mut plan = self.plan.lock().expect("plan");
        plan.base = self.state.input_count.load(Ordering::Acquire);
        plan.screens = screens;
    }
}

fn start(root: &TempDir, device: &Device) -> RuntimeHost {
    let provider = FakeProvider::one(ALIAS, device.id, Arc::clone(&device.state))
        .with_vision_provider(Arc::new(ShownFields::default()));
    RuntimeHost::start(config(root), Arc::new(provider)).expect("runtime host")
}

// One manual run: its receipt, correlation and request.
struct ListRun {
    receipt: RuntimeReceipt,
    correlation_id: CorrelationId,
    request_id: RequestId,
}

// The request of one manual run of `package` on the device, with its correlation.
fn run_request(package: &Path) -> (RuntimeRequest, CorrelationId) {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let correlation = ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = RuntimeRequest::new(
        ids.mint_request_id().expect("request id"),
        correlation,
        None,
        EventActor::Cli,
        EventSource::Cli,
        unix_ms_now().expect("wall clock"),
        RuntimeOperation::run_contained_task(
            ALIAS,
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.display().to_string()).expect("task request"),
        ),
    )
    .expect("runtime request");
    (request, correlation_id)
}

fn process(host: &RuntimeHost, request: &RuntimeRequest) -> RuntimeReceipt {
    let connection =
        ConnectionId::new(NEXT_CONNECTION.fetch_add(1, Ordering::Relaxed)).expect("connection");
    host.process_request_for_test(request, connection)
        .expect("the run's receipt")
}

// One manual run of `package` on the device.
fn run(host: &RuntimeHost, package: &Path) -> ListRun {
    let (request, correlation_id) = run_request(package);
    let receipt = process(host, &request);
    ListRun {
        receipt,
        correlation_id,
        request_id: request.request_id(),
    }
}

// One manual run of `package` whose first input carries a stop of the run (`StopInstanceRun`,
// answered at once): the run and the stop's answer.
fn run_stopped_at_first_input(
    host: &RuntimeHost,
    device: &Device,
    package: &Path,
) -> (ListRun, RuntimeReceipt) {
    let address = host.runtime_info().socket_addr().expect("runtime address");
    let (request, correlation_id) = run_request(package);
    let request_id = request.request_id();
    let (sent, answered) = mpsc::channel();
    let hook: Box<dyn FnOnce() + Send> = Box::new(move || {
        let mut client = TestClient::connect_address(address);
        client.set_receipt_read_timeout();
        let stop = client.request(RuntimeOperation::StopInstanceRun {
            instance_alias: ALIAS.to_owned(),
            request_id,
            wait_ms: 0,
        });
        sent.send(client.send(&stop)).expect("the stop's answer");
    });
    *device.state.input_hook.lock().expect("input hook") = Some(hook);
    let receipt = process(host, &request);
    let answer = answered.recv_timeout(GATE_WAIT).expect("the stop's answer");
    (
        ListRun {
            receipt,
            correlation_id,
            request_id,
        },
        answer,
    )
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

fn ended_on_success(receipt: &RuntimeReceipt) -> bool {
    matches!(
        receipt.result(),
        Some(RuntimeResult::ContainedTaskCompleted {
            task_result: TaskOutcome::Success,
            ..
        })
    )
}

fn cancelled_by_its_client(receipt: &RuntimeReceipt) -> bool {
    matches!(
        receipt.result(),
        Some(RuntimeResult::ContainedTaskCancelled {
            reason: ContainedTaskCancellationReason::ClientRequested,
            ..
        })
    )
}

// ---- The packages.

// What a field list's package declares.
#[derive(Debug, Clone, Copy)]
struct Fields {
    fact_key: &'static str,
    // The column `count` of the field `amount`; without it the list holds its keys only.
    count: bool,
    // The fields' `max_frames`.
    max_frames: u32,
    // The list ends on `last` only with the final mark, the task's target is `done`, which no
    // screen shows, and the step `back` taps from `last` to an item: the run collects a frame of
    // the list after its end.
    back_after_end: bool,
}

impl Fields {
    fn list(fact_key: &'static str) -> Self {
        Self {
            fact_key,
            count: false,
            max_frames: 256,
            back_after_end: false,
        }
    }
}

fn zip_package(entries: Vec<(&str, Vec<u8>)>) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, bytes) in entries {
        zip.start_file(path, options).expect("package entry");
        zip.write_all(&bytes).expect("package content");
    }
    zip.finish().expect("finish package").into_inner()
}

fn json_bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("package JSON")
}

// The package `neutral.list.fields` (task schema `0.9`, `fields_v1` on `item` and `last`), with
// its one field list as `fields` declares it.
fn fields_package(fields: Fields) -> Vec<u8> {
    let space = json!({"width": WIDTH, "height": HEIGHT});
    let names = json_bytes(&json!({
        "schema_version": "actingcommand.ocr-truth-set.v2",
        "items": (0..u32::from(KNOWN_NAMES)).map(member).collect::<Vec<_>>(),
        "aliases": []
    }));
    let names_sha256 = format!("{:x}", Sha256::digest(&names));
    let field = |id: &str, value: Value| {
        json!({"id": id, "group": "row", "target_id": format!("text/{id}"), "required": false,
               "privacy": "public", "trim": "whitespace_v1", "value": value})
    };
    let columns = if fields.count {
        json!([{"column": "count", "field": "amount"}])
    } else {
        json!([])
    };
    let mut list = json!({
        "id": LIST_ID, "group": "row", "key_field": "name", "key_column": "id",
        "columns": columns, "minimum_confidence_milli": 900,
        "fact_key": fields.fact_key, "valid_for_ms": VALID_FOR_MS,
        "start": {"forbidden": ["mark/previous"]},
        "end": {"page_ids": ["last"]}
    });
    let mut operations = vec![json!({
        "id": "next", "from": "item", "to": ["item", "last"],
        "click": {"kind": "point", "x": 40, "y": 50}, "unguarded_trusted_coordinate": true
    })];
    let mut target = "last";
    if fields.back_after_end {
        list["end"] = json!({"page_ids": ["last"], "required": ["mark/final"]});
        operations.push(json!({
            "id": "back", "from": "last", "to": "item",
            "click": {"kind": "point", "x": 40, "y": 60}, "unguarded_trusted_coordinate": true
        }));
        target = "done";
    }
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.list.fields",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": space,
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 10_000,
        "timeout_ms": 60_000,
        "max_steps": 80
    });
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": space,
        "target_page": target,
        "post_admission_ocr": {
            "mode": "fields_v1", "page_ids": ["item", "last"],
            "fields": [
                field("name", json!({"type": "dictionary_entry",
                                     "dictionary": {"path": "names.json",
                                                    "sha256": names_sha256}})),
                field("amount", json!({"type": "unsigned_integer", "min": 0, "max": 999}))
            ],
            "limits": {"max_frames": fields.max_frames, "max_items": 512, "max_string_bytes": 64,
                       "max_total_bytes": 65_536, "max_truth_entries": 16},
            "outcome_key": "fields_recorded"
        },
        "scheduling_outcome": {"mappings": [
            {"outcome_key": "fields_recorded", "effect": "no_designated_effect",
             "terminal_pages": ["last"]}
        ]},
        "operations": operations,
        "field_lists": [list]
    });
    let color = |id: &str, x: usize, expected: [u8; 3]| {
        json!({"type": "color", "id": id, "region": {"x": x, "y": 0, "width": 1, "height": 1},
               "expected": expected})
    };
    let ocr = |id: &str, x: usize| {
        json!({"type": "ocr", "id": id, "region": {"x": x, "y": 0, "width": 1, "height": 1},
               "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact",
               "expected": ["unused"], "case_sensitive": false, "minimum_confidence": 0.1,
               "model_ref": "PP-OCRv6_medium", "model_sha256": "a".repeat(64)})
    };
    let pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": space,
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            color("page/item", 0, ITEM),
            color("page/last", 0, LAST),
            color("page/done", 0, DONE),
            color("mark/previous", PREVIOUS_X, ARROW),
            color("mark/final", FINAL_X, ARROW),
            ocr("text/name", NAME_X),
            ocr("text/amount", AMOUNT_X)
        ]
    });
    let page = |id: &str, target: &str| json!({"id": id, "required": [target], "optional": [], "forbidden": []});
    let pages = json!({
        "schema_version": "0.3",
        "pages": [
            page("neutral/item", "page/item"),
            page("neutral/last", "page/last"),
            page("neutral/done", "page/done")
        ]
    });
    let manifest = json!({
        "schema_version": "0.3",
        "entry_task_id": "task",
        "files": [{"path": "operations/task/names.json", "sha256": names_sha256}]
    });
    zip_package(vec![
        ("control.json", json_bytes(&control)),
        ("resources/manifest.json", json_bytes(&manifest)),
        ("resources/operations/task/task.json", json_bytes(&task)),
        ("resources/operations/task/names.json", names),
        (
            "resources/recognition/neutral.test.pack.json",
            json_bytes(&pack),
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            json_bytes(&pages),
        ),
    ])
}

// The scan over the one-screen list (the SEL-L4b host gate's), whose start and end checks both pass
// on a list whose end shows.
fn scan() -> Value {
    json!({
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

// The package `neutral.list.task` of the S168-4 host gate, its step `read_members` reading the
// cards' keys only into `fact_key`, then `leave` to the detail page.
fn read_list_package(fact_key: &str) -> Vec<u8> {
    let identities = (0..=6).map(member).collect::<Vec<_>>();
    let space = json!({"width": WIDTH, "height": HEIGHT});
    let layout = json!({
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
                     .map(|id| json!({"id": id, "aliases": [id]}))
                     .collect::<Vec<_>>(),
                 "recognition": {"kind": "ocr_aliases", "max_distance": 1, "minimum_margin": 1,
                                 "minimum_confidence_milli": 800}}},
            {"name": "open", "value": "passed", "target": "card/open",
             "offset": {"x": 8, "y": 0}}
        ]
    });
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.list.task",
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
    let task = json!({
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
                "id": READ_ID,
                "from": "list",
                "to": "list",
                "guard": {
                    "page_id": "list",
                    "target_id": "guard/list",
                    "expected_rect": {"x": 3, "y": 0, "width": 1, "height": 1},
                    "color_probe": "guard/list"
                },
                "read_list": {"layout_id": "layout/cards", "key_feature": "member",
                              "key_column": "id", "columns": [],
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
        json!({"type": "color", "id": id, "region": {"x": x, "y": 0, "width": 1, "height": 1},
               "expected": expected})
    };
    let pack = json!({
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
    let pages = json!({
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
    zip_package(vec![
        ("control.json", json_bytes(&control)),
        (
            "resources/manifest.json",
            json_bytes(&json!({"schema_version": "0.3", "entry_task_id": "task"})),
        ),
        ("resources/operations/task/task.json", json_bytes(&task)),
        (
            "resources/recognition/neutral.test.pack.json",
            json_bytes(&pack),
        ),
        ("resources/recognition/assets/card.png", marker),
        (
            "resources/recognition/neutral.test.pages.json",
            json_bytes(&pages),
        ),
    ])
}

fn write_package(root: &TempDir, name: &str, bytes: Vec<u8>) -> PathBuf {
    let path = root.path().join(name);
    fs::write(&path, bytes).expect("write the package");
    path
}

// ---- What the host recorded.

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

fn records(host: &RuntimeHost) -> Vec<FactRecord> {
    host.instance_fact_snapshot(InstanceFactContext {
        instance_id: ALIAS.to_owned(),
        server_id: "test".to_owned(),
        game_id: "neutral".to_owned(),
    })
    .expect("instance facts")
    .records
}

fn record(host: &RuntimeHost, key: &str) -> Option<FactRecord> {
    records(host).into_iter().find(|record| record.key == key)
}

// The key's record from before is the key's record, byte for byte.
fn assert_kept(host: &RuntimeHost, key: &str, before: &FactRecord) {
    let now = record(host, key).expect("the key's record");
    assert_eq!(&now, before);
    assert_eq!(
        serde_json::to_vec(&now).expect("record JSON"),
        serde_json::to_vec(before).expect("record JSON"),
        "byte for byte"
    );
}

fn rows_of(record: &FactRecord) -> Vec<BTreeMap<String, FactScalar>> {
    match &record.content {
        FactContent::Inline {
            value: ContractFactValue::RecordList(rows),
        } => rows.clone(),
        other => panic!("a record list, got {other:?}"),
    }
}

// The row of `member-0<card>`, keys only.
fn key_row(card: u32) -> BTreeMap<String, FactScalar> {
    BTreeMap::from([("id".to_owned(), FactScalar::String(member(card)))])
}

// An identifier's canonical text, the one a `source_snapshot_id` names it by.
fn id_text(id: &impl serde::Serialize) -> String {
    serde_json::to_value(id)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .expect("an identifier's text")
}

// The run, the frame and the list a list's `source_snapshot_id`
// (`run:<run>/frame:<frame>/<list>`) names.
fn snapshot_parts(record: &FactRecord) -> (String, String, String) {
    let mut parts = record.source_snapshot_id.split('/');
    let run = parts.next().expect("run part").to_owned();
    let frame = parts.next().expect("frame part").to_owned();
    let list = parts.next().expect("list part").to_owned();
    assert!(
        run.starts_with("run:run_") && frame.starts_with("frame:frame_"),
        "{}",
        record.source_snapshot_id
    );
    (run, frame, list)
}

// The run `correlation_id` and the frame it captured last, the end frame of a field list.
fn end_frame(host: &RuntimeHost, correlation_id: CorrelationId) -> (RunId, FrameId) {
    let records = run_records(host, correlation_id);
    let frame = records
        .iter()
        .rev()
        .find(|event| event.event_type() == EventType::CaptureCompleted)
        .and_then(|event| event.links().frame_id())
        .copied()
        .expect("a frame the run captured");
    let run = records
        .iter()
        .find_map(|event| event.links().run_id())
        .copied()
        .expect("the run's id");
    (run, frame)
}

// The times of the records `capture.requested` and `capture.completed` of `frame` in the run
// `correlation_id`: the frame's device capture lies between them.
fn capture_window(host: &RuntimeHost, correlation_id: CorrelationId, frame: FrameId) -> (u64, u64) {
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

// The rows of the task diagnostic streams of the run `correlation_id` whose outcome is the warning
// of a list that gave no reading, as the host wrote them.
fn miss_rows(root: &TempDir, host: &RuntimeHost, correlation_id: CorrelationId) -> Vec<Value> {
    let mut rows = Vec::new();
    for event in run_records(host, correlation_id) {
        if event.event_type() != EventType::ArtifactVerified {
            continue;
        }
        for artifact in event.artifacts() {
            let projected = artifact.project(true);
            if projected.kind != ArtifactKind::DiagnosticJson
                || projected.producer != ArtifactProducer::ArtifactStore
                || projected.redaction_state != ArtifactRedactionState::Pending
            {
                continue;
            }
            let document: Value = serde_json::from_slice(
                &read_projected_verified(root.path(), &projected).expect("verified stream bytes"),
            )
            .expect("diagnostic JSON");
            let Some(records) = document["records"].as_array() else {
                continue;
            };
            rows.extend(
                records
                    .iter()
                    .filter(|record| {
                        record["kind"] == "error"
                            && record["data"]["error"]["outcome"]["code"] == UNRESOLVED
                    })
                    .cloned(),
            );
        }
    }
    rows
}

// The one miss row of the run `correlation_id`.
fn one_miss_row(root: &TempDir, host: &RuntimeHost, correlation_id: CorrelationId) -> Value {
    let rows = miss_rows(root, host, correlation_id);
    let [row] = rows.as_slice() else {
        panic!("exactly one miss row: {rows:?}");
    };
    row.clone()
}

// A miss row's outcome values without `keys`.
fn values_without(row: &Value, keys: &[&str]) -> serde_json::Map<String, Value> {
    let mut values = row["data"]["error"]["outcome"]["values"]
        .as_object()
        .cloned()
        .expect("the outcome's values");
    for key in keys {
        values.remove(*key);
    }
    values
}

// A warning row of the field list `list_a` of `LIST_KEY`: the error row of the post-admission phase
// without a target, the warning with `problem`, `count` and `frame_indices`.
fn assert_field_list_miss(row: &Value, problem: &str, count: u32, frame_indices: &[u32]) {
    let outcome = &row["data"]["error"]["outcome"];
    assert_eq!(outcome["category"], "warning", "{problem}: {row}");
    assert_eq!(outcome["values"]["reading_id"], LIST_ID, "{problem}");
    assert_eq!(outcome["values"]["fact_key"], LIST_KEY, "{problem}");
    assert_eq!(outcome["values"]["reading_problem"], problem);
    assert_eq!(outcome["values"]["count"], count, "{problem}");
    assert_eq!(
        outcome["values"]["frame_indices"],
        json!(frame_indices),
        "{problem}"
    );
    assert_eq!(row["data"]["phase"], "post_admission", "{problem}");
    assert!(row["data"]["target_id"].is_null(), "{problem}");
}

// ---- The tests.

// FL-2 test 1: a run of the field list over six items, their names out of key order, is whole: the
// host publishes after its successful terminal one `fact.published` of the list's key with one
// `record_list` record of the run's instance: the six keys in ascending byte order (the order and
// the row cap are a `read_list`'s; the 1024-row cap cannot be reached by a field list, whose rows
// are at most its 256 frames), expiring `valid_for_ms` after its observation with that lifetime
// from the detector contract, at the lowest confidence of the fields read (990),
// `source_detector` `list_reading:task/list_a`, and `source_snapshot_id` naming the run, its end
// frame (the frame it captured last, when the list was observed) and the list. In the same host a
// `read_list` step read the same keys under the same key just before: the two records are equal
// in every member but the producer's list id (`source_detector`), the run, frame and list id
// (`source_snapshot_id`), the end frame's time (`observed_at_unix_ms`, `expires_at_unix_ms`) and
// the producing package (`resource_bundle_hash`). The list is the key's instance record, and a
// restart loads it again.
#[test]
fn field_list_published_like_a_list() {
    let root = TempDir::new().expect("tempdir");
    let fields = write_package(&root, "fields.zip", fields_package(Fields::list(LIST_KEY)));
    let read_list = write_package(&root, "read-list.zip", read_list_package(LIST_KEY));
    let device = Device::new();
    let host = start(&root, &device);

    device.shows(vec![cards([3, 0, 5, 1, 4, 2]), Screen::Detail]);
    let listed = run(&host, &read_list);
    completed(&host, &listed.receipt);
    device.shows(items(&[(3, 5), (0, 6), (5, 7), (1, 8), (4, 9), (2, 10)]));
    let inputs = device.state.input_count.load(Ordering::Acquire);
    let read = run(&host, &fields);
    completed(&host, &read.receipt);
    assert!(ended_on_success(&read.receipt), "{:?}", read.receipt);
    assert_eq!(
        device.state.input_count.load(Ordering::Acquire) - inputs,
        5,
        "one tap from each item but the last"
    );

    let published = publications(&host, LIST_KEY);
    let [by_read_list, by_fields] = published.as_slice() else {
        panic!("one publication by each producer, got {published:#?}");
    };
    assert_eq!(
        by_fields.scope,
        FactScope::Instance {
            instance_id: ALIAS.to_owned()
        }
    );
    assert_eq!(rows_of(by_fields), (0..6).map(key_row).collect::<Vec<_>>());
    assert_eq!(
        by_fields.expires_at_unix_ms,
        Some(by_fields.observed_at_unix_ms + VALID_FOR_MS)
    );
    assert_eq!(
        by_fields.ttl_policy,
        Some(FactTtlPolicy {
            minimum_ms: VALID_FOR_MS,
            maximum_ms: VALID_FOR_MS,
            source: FactTtlSource::DetectorContract,
        })
    );
    assert_eq!(by_fields.confidence_milli, 990);
    assert_eq!(by_fields.source_detector, "list_reading:task/list_a");
    assert_eq!(by_fields.schema_version, "fact.v1");
    assert!(by_fields.invalidate_on.is_empty());
    let (run_id, end) = end_frame(&host, read.correlation_id);
    let (requested_at, completed_at) = capture_window(&host, read.correlation_id, end);
    assert!(
        (requested_at..=completed_at).contains(&by_fields.observed_at_unix_ms),
        "observed at {}, the end frame captured within {requested_at}..={completed_at}",
        by_fields.observed_at_unix_ms
    );
    assert_eq!(
        snapshot_parts(by_fields),
        (
            format!("run:{}", id_text(&run_id)),
            format!("frame:{}", id_text(&end)),
            LIST_ID.to_owned()
        )
    );

    // Member by member against the `read_list` step's record of the same keys.
    assert_eq!(
        by_read_list.source_detector,
        "list_reading:task/read_members"
    );
    assert_eq!(snapshot_parts(by_read_list).2, READ_ID);
    assert!(by_fields.observed_at_unix_ms > by_read_list.observed_at_unix_ms);
    assert_ne!(
        by_fields.resource_bundle_hash,
        by_read_list.resource_bundle_hash
    );
    let mut as_read_list = by_fields.clone();
    as_read_list
        .source_detector
        .clone_from(&by_read_list.source_detector);
    as_read_list
        .source_snapshot_id
        .clone_from(&by_read_list.source_snapshot_id);
    as_read_list.observed_at_unix_ms = by_read_list.observed_at_unix_ms;
    as_read_list.expires_at_unix_ms = by_read_list.expires_at_unix_ms;
    as_read_list
        .resource_bundle_hash
        .clone_from(&by_read_list.resource_bundle_hash);
    assert_eq!(
        &as_read_list, by_read_list,
        "every other member is the read_list's"
    );

    // The key's instance record, also after a restart.
    assert_eq!(record(&host, LIST_KEY).as_ref(), Some(by_fields));
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
    let host = start(&root, &device);
    assert_eq!(
        record(&host, LIST_KEY).as_ref(),
        Some(by_fields),
        "loaded at start"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

// FL-2 test 2: after a run published the list, each way a list is not whole succeeds as a
// `read_list` miss does (in the same host, a `read_list` step whose card reads as nothing), publishes
// nothing, keeps the key's record byte for byte and writes exactly one warning row,
// `contained_task_resource_reading_unresolved` with the list's id and key, its `reading_problem`,
// `count` and `frame_indices` (the fields report's frame indices): an item whose name is out of the
// dictionary (`list_position_unread`, 1, [1]; its values are the `read_list` miss's plus
// `frame_indices`), a name read at 0.85, under the list's floor of 900 though over its target's
// own (`list_position_unread`, 1, [2]), a key read twice (`list_duplicate_key`, 1, [0, 2]), a first
// item that shows the previous arrow (`list_start_unseen`, 3, [0]), and a package whose fields stop
// at two frames before the last item (`list_end_unseen`, 2, [1]). Sixty-five unresolved items name
// the first 64 frames and `frame_indices_total` 65.
#[test]
fn field_list_miss_keeps_record() {
    let root = TempDir::new().expect("tempdir");
    let list = write_package(&root, "fields.zip", fields_package(Fields::list(LIST_KEY)));
    let short = write_package(
        &root,
        "fields-short.zip",
        fields_package(Fields {
            max_frames: 2,
            ..Fields::list(LIST_KEY)
        }),
    );
    let read_list = write_package(&root, "read-list.zip", read_list_package(OTHER_KEY));
    let device = Device::new();
    let host = start(&root, &device);
    device.shows(three_items());
    let whole = run(&host, &list);
    completed(&host, &whole.receipt);
    let before = record(&host, LIST_KEY).expect("the list's record");

    // What a `read_list` run writes for an unread position.
    let mut unread = (0..6).map(patch).collect::<Vec<_>>();
    unread[2] = UNREADABLE;
    device.shows(vec![Screen::Cards(unread), Screen::Detail]);
    let listed = run(&host, &read_list);
    completed(&host, &listed.receipt);
    assert!(ended_on_success(&listed.receipt), "{:?}", listed.receipt);
    let read_list_miss = one_miss_row(&root, &host, listed.correlation_id);
    assert_eq!(
        read_list_miss["data"]["error"]["outcome"]["values"]["reading_problem"],
        "list_position_unread"
    );
    assert!(publications(&host, OTHER_KEY).is_empty());

    let cases: [(&Path, Vec<Screen>, &str, u32, Vec<u32>); 5] = [
        (
            list.as_path(),
            changed(three_items(), 1, |item| item.name = UNLISTED),
            "list_position_unread",
            1,
            vec![1],
        ),
        (
            list.as_path(),
            changed(three_items(), 2, |item| item.confidence = [85, 99]),
            "list_position_unread",
            1,
            vec![2],
        ),
        (
            list.as_path(),
            items(&[(0, 5), (1, 7), (0, 9)]),
            "list_duplicate_key",
            1,
            vec![0, 2],
        ),
        (
            list.as_path(),
            changed(three_items(), 0, |item| item.previous = true),
            "list_start_unseen",
            3,
            vec![0],
        ),
        (
            short.as_path(),
            three_items(),
            "list_end_unseen",
            2,
            vec![1],
        ),
    ];
    for (package, screens, problem, count, frame_indices) in cases {
        device.shows(screens);
        let miss = run(&host, package);
        completed(&host, &miss.receipt);
        assert!(
            ended_on_success(&miss.receipt),
            "{problem}: {:?}",
            miss.receipt
        );
        assert_kept(&host, LIST_KEY, &before);
        assert_eq!(publications(&host, LIST_KEY).len(), 1, "{problem}");
        let row = one_miss_row(&root, &host, miss.correlation_id);
        assert_field_list_miss(&row, problem, count, &frame_indices);
        assert!(
            row["data"]["error"]["outcome"]["values"]
                .get("frame_indices_total")
                .is_none(),
            "{problem}"
        );
        if problem == "list_position_unread" {
            assert_eq!(
                values_without(&row, &["reading_id", "fact_key", "frame_indices"]),
                values_without(&read_list_miss, &["reading_id", "fact_key"]),
                "{problem}: the read_list miss's values"
            );
        }
    }

    // Sixty-five items, none in the dictionary.
    let shown = (0..65_u8)
        .map(|index| (UNLISTED, index))
        .collect::<Vec<_>>();
    device.shows(items(&shown));
    let long = run(&host, &list);
    completed(&host, &long.receipt);
    assert_kept(&host, LIST_KEY, &before);
    let row = one_miss_row(&root, &host, long.correlation_id);
    assert_field_list_miss(
        &row,
        "list_position_unread",
        65,
        &(0..64).collect::<Vec<_>>(),
    );
    assert_eq!(
        row["data"]["error"]["outcome"]["values"]["frame_indices_total"],
        65
    );
    assert_eq!(publications(&host, LIST_KEY).len(), 1);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

// FL-2 test 3: after a run published the list, a run stopped at its first tap (after its first
// item, `StopInstanceRun` answered at once) is cancelled by its client, and a run whose list was
// decided whole on `last` and which then collects a frame of the list after its end fails
// `contained_task_state_invalid`: neither publishes, the key's record stays byte for byte, and
// neither writes a miss row. A `read_list` run stopped at its one input, after its list step,
// publishes nothing and writes no miss row either.
#[test]
fn field_list_failed_or_cancelled_run_publishes_nothing() {
    let root = TempDir::new().expect("tempdir");
    let list = write_package(&root, "fields.zip", fields_package(Fields::list(LIST_KEY)));
    let failing = write_package(
        &root,
        "fields-back.zip",
        fields_package(Fields {
            back_after_end: true,
            ..Fields::list(LIST_KEY)
        }),
    );
    let read_list = write_package(&root, "read-list.zip", read_list_package(OTHER_KEY));
    let device = Device::new();
    let host = start(&root, &device);
    device.shows(three_items());
    let whole = run(&host, &list);
    completed(&host, &whole.receipt);
    let before = record(&host, LIST_KEY).expect("the list's record");

    // Stopped in mid-list.
    device.shows(three_items());
    let (cancelled, answer) = run_stopped_at_first_input(&host, &device, &list);
    assert_eq!(answer.state(), RuntimeReceiptState::Completed, "{answer:?}");
    assert!(
        cancelled_by_its_client(&cancelled.receipt),
        "{:?}",
        cancelled.receipt
    );
    assert_kept(&host, LIST_KEY, &before);
    assert_eq!(publications(&host, LIST_KEY).len(), 1);
    let field_rows = miss_rows(&root, &host, cancelled.correlation_id);

    // A `read_list` run stopped the same way, at its tap after the list step.
    device.shows(vec![cards(0..6), Screen::Detail]);
    let (read_cancelled, read_answer) = run_stopped_at_first_input(&host, &device, &read_list);
    assert_eq!(
        read_answer.state(),
        RuntimeReceiptState::Completed,
        "{read_answer:?}"
    );
    assert!(
        cancelled_by_its_client(&read_cancelled.receipt),
        "{:?}",
        read_cancelled.receipt
    );
    assert!(publications(&host, OTHER_KEY).is_empty());
    let read_rows = miss_rows(&root, &host, read_cancelled.correlation_id);
    assert!(read_rows.is_empty(), "{read_rows:?}");
    assert_eq!(field_rows.len(), read_rows.len(), "{field_rows:?}");

    // Decided whole on `last`, then a frame of the list after its end.
    device.shows(vec![
        Screen::Item(Item {
            name: 0,
            amount: 5,
            ..SHOWN
        }),
        Screen::Item(Item {
            last: true,
            name: 1,
            amount: 7,
            previous: true,
            final_mark: true,
            ..SHOWN
        }),
        Screen::Item(Item {
            name: 2,
            amount: 9,
            previous: true,
            ..SHOWN
        }),
    ]);
    let failed = run(&host, &failing);
    assert_eq!(
        failed.receipt.state(),
        RuntimeReceiptState::Failed,
        "{:?}",
        failed.receipt.outcome()
    );
    assert_eq!(
        failed.receipt.outcome().code().as_str(),
        "contained_task_state_invalid",
        "{:?}",
        failed.receipt.outcome()
    );
    assert_kept(&host, LIST_KEY, &before);
    assert_eq!(publications(&host, LIST_KEY).len(), 1);
    let failed_rows = miss_rows(&root, &host, failed.correlation_id);
    assert!(failed_rows.is_empty(), "{failed_rows:?}");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

// FL-2 test 4, the store's refusals as for a `read_list` step, both producers in one host. The byte
// bound (lowered for each producer's own key, as S168-LB1a's host test lowers it): after each
// published its list of six rows, a list over the bound succeeds, publishes nothing, keeps the
// record from before and writes one warning row `list_bytes_exceeded` with `count` 6, and the two
// rows' values are the same but the producer's id and key: the field list's names no frame. A full
// store: each producer's list under a new key is refused at its publication, the run fails
// `contained_task_resource_reading_rejected`, the reason is the failure's native detail
// `<list id>:fact_store_capacity_exceeded` (as the contract keeps it), nothing is published or
// evicted, and the two receipts' outcomes carry the same values (no value is added). The row bound
// of a record list (1024) cannot be reached by a field list, whose rows are at most its 256 frames.
#[test]
fn field_list_full_store_refused() {
    const FIELD_BYTES_KEY: &str = "inventory.list.example_kind_f";
    const READ_BYTES_KEY: &str = "inventory.list.example_kind_g";
    const THIRD_KEY: &str = "inventory.list.example_kind_c";
    let root = TempDir::new().expect("tempdir");
    let field_bytes = write_package(
        &root,
        "fields-bytes.zip",
        fields_package(Fields::list(FIELD_BYTES_KEY)),
    );
    let read_bytes = write_package(&root, "read-bytes.zip", read_list_package(READ_BYTES_KEY));
    let field_new = write_package(
        &root,
        "fields-new.zip",
        fields_package(Fields::list(OTHER_KEY)),
    );
    let read_new = write_package(&root, "read-new.zip", read_list_package(THIRD_KEY));
    let device = Device::new();
    let host = start(&root, &device);
    let six_items = || items(&[(0, 5), (1, 5), (2, 5), (3, 5), (4, 5), (5, 5)]);
    let six_cards = || vec![cards(0..6), Screen::Detail];

    // The byte bound.
    for (package, screens, key) in [
        (&field_bytes, six_items(), FIELD_BYTES_KEY),
        (&read_bytes, six_cards(), READ_BYTES_KEY),
    ] {
        device.shows(screens);
        let first = run(&host, package);
        completed(&host, &first.receipt);
        assert_eq!(publications(&host, key).len(), 1, "{key}");
    }
    crate::host::set_list_observation_bytes_for_test(FIELD_BYTES_KEY, 64);
    crate::host::set_list_observation_bytes_for_test(READ_BYTES_KEY, 64);
    let mut misses = Vec::new();
    for (package, screens, key, id) in [
        (&field_bytes, six_items(), FIELD_BYTES_KEY, LIST_ID),
        (&read_bytes, six_cards(), READ_BYTES_KEY, READ_ID),
    ] {
        let before = record(&host, key).expect("the list's record");
        device.shows(screens);
        let over = run(&host, package);
        completed(&host, &over.receipt);
        assert_kept(&host, key, &before);
        assert_eq!(
            publications(&host, key).len(),
            1,
            "{key}: nothing is published"
        );
        let row = one_miss_row(&root, &host, over.correlation_id);
        let outcome = &row["data"]["error"]["outcome"];
        assert_eq!(outcome["category"], "warning", "{key}");
        assert_eq!(outcome["values"]["reading_id"], id);
        assert_eq!(outcome["values"]["fact_key"], key);
        assert_eq!(outcome["values"]["reading_problem"], "list_bytes_exceeded");
        assert_eq!(outcome["values"]["count"], 6, "{key}");
        assert_eq!(row["data"]["phase"], "post_admission", "{key}");
        assert!(row["data"]["target_id"].is_null(), "{key}");
        misses.push(row);
    }
    let [field_miss, read_list_miss] = misses.as_slice() else {
        unreachable!("two misses")
    };
    assert_eq!(
        values_without(field_miss, &["reading_id", "fact_key"]),
        values_without(read_list_miss, &["reading_id", "fact_key"]),
        "the same values: no frame is named"
    );

    // A full store.
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
            source_snapshot_id: format!("snapshot:fl-2-fill-{index:03}"),
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
    let before = records(&host);
    let mut refusals = Vec::new();
    for (package, screens, key, id) in [
        (&field_new, six_items(), OTHER_KEY, LIST_ID),
        (&read_new, six_cards(), THIRD_KEY, READ_ID),
    ] {
        device.shows(screens);
        let refused = run(&host, package);
        assert_eq!(
            refused.receipt.state(),
            RuntimeReceiptState::Failed,
            "{key}: {:?}",
            refused.receipt.outcome()
        );
        assert_eq!(
            refused.receipt.outcome().code().as_str(),
            "contained_task_resource_reading_rejected",
            "{key}: {:?}",
            refused.receipt.outcome()
        );
        assert_eq!(
            host.failure_detail_for_test(refused.request_id)
                .expect("the failure's detail"),
            Some(format!("{id}:fact_store_capacity_exceeded"))
        );
        assert!(publications(&host, key).is_empty(), "{key}");
        assert_eq!(records(&host), before, "{key}: no active fact is evicted");
        assert!(host.fatal_error().expect("runtime health").is_none());
        let outcome = serde_json::to_value(refused.receipt.outcome()).expect("outcome JSON");
        refusals.push(outcome["values"].clone());
    }
    assert_eq!(refusals[0], refusals[1], "no value is added");
    assert!(refusals[0].get("frame_indices").is_none());
    host.close().expect("close the host");
}

// FL-2 test 5: the ledger reader of this build (`actingledger`'s task evidence, `ledger` 8) reads
// the task diagnostic rows of a run whose list started inside it and of a run whose list saw no
// end: each miss row is an error row of the post-admission phase without a target whose outcome,
// `contained_task_resource_reading_unresolved`, carries `list_start_unseen` (3, [0]) or
// `list_end_unseen` (2, [1]) with `frame_indices`, and its values are, member for member, the
// values the host wrote.
#[test]
fn field_list_row_read_by_ledger_8() {
    let root = TempDir::new().expect("tempdir");
    let list = write_package(&root, "fields.zip", fields_package(Fields::list(LIST_KEY)));
    let short = write_package(
        &root,
        "fields-short.zip",
        fields_package(Fields {
            max_frames: 2,
            ..Fields::list(LIST_KEY)
        }),
    );
    let device = Device::new();
    let host = start(&root, &device);
    device.shows(changed(three_items(), 0, |item| item.previous = true));
    let started_inside = run(&host, &list);
    completed(&host, &started_inside.receipt);
    device.shows(three_items());
    let no_end = run(&host, &short);
    completed(&host, &no_end.receipt);
    let written = [
        (
            "list_start_unseen",
            3,
            vec![0_u32],
            one_miss_row(&root, &host, started_inside.correlation_id),
        ),
        (
            "list_end_unseen",
            2,
            vec![1],
            one_miss_row(&root, &host, no_end.correlation_id),
        ),
    ];
    let after_sequence = run_records(&host, started_inside.correlation_id)
        .first()
        .map(PersistedEvent::sequence)
        .expect("the first run's first record")
        - 1;
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");

    let rows = ledger_rows(root.path(), after_sequence);
    for (problem, count, frame_indices, row_written) in written {
        let read = rows
            .iter()
            .filter_map(|record| match &record.payload {
                TaskDiagnosticPayload::Error(error) => match error.as_ref() {
                    TaskDiagnosticErrorData::Recognition {
                        phase,
                        target_id,
                        error: recognition,
                    } if recognition.outcome.value("reading_problem") == Some(&json!(problem)) => {
                        Some((phase, target_id, &recognition.outcome))
                    }
                    _ => None,
                },
                _ => None,
            })
            .collect::<Vec<_>>();
        let [(phase, target_id, outcome)] = read.as_slice() else {
            panic!("{problem}: one row read, got {read:?}");
        };
        assert_eq!(phase.as_str(), "post_admission", "{problem}");
        assert!(target_id.is_none(), "{problem}");
        assert_eq!(outcome.code().as_str(), UNRESOLVED, "{problem}");
        assert_eq!(outcome.value("reading_id"), Some(&json!(LIST_ID)));
        assert_eq!(outcome.value("count"), Some(&json!(count)), "{problem}");
        assert_eq!(
            outcome.value("frame_indices"),
            Some(&json!(frame_indices)),
            "{problem}"
        );
        assert_eq!(
            serde_json::to_value(outcome).expect("outcome JSON")["values"],
            row_written["data"]["error"]["outcome"]["values"],
            "{problem}: relayed unchanged"
        );
    }
}

// Every task diagnostic row the ledger reader reads from the window after `after_sequence`, each
// stream read to its end page by page.
fn ledger_rows(root: &Path, after_sequence: u64) -> Vec<TaskDiagnosticRecord> {
    let read = |cursor: Option<TaskDiagnosticCursor>| -> Vec<TaskDiagnosticPage> {
        let request = ForensicRequest::task_evidence(
            root,
            ForensicEventsRequest::new(
                ForensicEventFilter::default(),
                after_sequence,
                None,
                MAX_FORENSIC_EVENTS,
            )
            .expect("a task evidence window"),
        )
        .with_task_records(TaskRecordsRequest {
            cursor,
            limit: MAX_TASK_DIAGNOSTIC_PAGE_RECORDS,
            include_private: true,
        })
        .expect("a task records page");
        let ForensicOutput::Machine(ForensicReport::TaskEvidence(report)) =
            actingcommand_ledger_forensics::run(request).expect("the task evidence")
        else {
            panic!("a task evidence report");
        };
        report
            .diagnostics
            .iter()
            .filter(|page| page.state == "verified")
            .cloned()
            .collect()
    };
    let mut rows = Vec::new();
    for page in read(None) {
        rows.extend(page.records.iter().cloned());
        let mut next = page.next_cursor.clone();
        while let Some(cursor) = next.take() {
            let pages = read(Some(cursor));
            let [more] = pages.as_slice() else {
                panic!("one page of the stream, got {pages:?}");
            };
            rows.extend(more.records.iter().cloned());
            next = more.next_cursor.clone();
        }
    }
    rows
}

// A select step's document over the card layout that keeps a card whose `count` in the published
// list is at least 5 (`keyed_fact`; an unlisted card reads `absent`, 0), within `max_age_ms` of the
// list's read; a gate that reads `keyed_fact` declares `on_unknown: abort_evaluation`.
fn counted_policy(max_age_ms: u64) -> Value {
    let identities = (0..=6).map(member).collect::<Vec<_>>();
    json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "cards-counted",
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
                                  "columns": {"count": {"type": "integer"}}},
                   "max_age_ms": max_age_ms, "minimum_confidence_milli": 500}],
        "gates": [{"gate_id": "counted",
                   "predicate": {"kind": "integer_at_least",
                                 "value": {"source": "keyed_fact", "fact_key": LIST_KEY,
                                           "key_field": "member", "value_column": "count",
                                           "absent": {"type": "integer", "value": 0}},
                                 "threshold": 5},
                   "on_unknown": {"kind": "abort_evaluation"}}],
        "scoring": [],
        "selection": {"mode": "none_allowed", "required_count": 1},
        "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
    })
}

// FL-2 test 6: the reader the list-reading slices built, a select step's `keyed_fact`, is given the
// list a field list published (`member-00` count 7, `member-01` 3, `member-02` 2) and behaves as
// S168-4 pins it on a `read_list`'s: evaluated on a frame of `member-00` to `member-05` with the
// instance's snapshot, the listed `member-00` reads its column and is the one member, the listed
// `member-01` and `member-02` read theirs and are rejected by the gate, and the unlisted `member-05`
// reads `absent` (0), a known value, so it is rejected by the gate, never dropped as unknown. The
// same list older than the document's `max_age_ms` ends the evaluation `unknown` with
// `fact_stale`, before any candidate.
#[test]
fn field_list_read_by_the_lists_reader() {
    const MAX_AGE_MS: u64 = 60_000;
    let root = TempDir::new().expect("tempdir");
    let list = write_package(
        &root,
        "fields.zip",
        fields_package(Fields {
            count: true,
            ..Fields::list(LIST_KEY)
        }),
    );
    let device = Device::new();
    let host = start(&root, &device);
    device.shows(items(&[(0, 7), (1, 3), (2, 2)]));
    let read = run(&host, &list);
    completed(&host, &read.receipt);
    let list_record = record(&host, LIST_KEY).expect("the list's record");
    let counted = |card: u32, count: i64| {
        BTreeMap::from([
            ("id".to_owned(), FactScalar::String(member(card))),
            ("count".to_owned(), FactScalar::Integer(count)),
        ])
    };
    assert_eq!(
        rows_of(&list_record),
        vec![counted(0, 7), counted(1, 3), counted(2, 2)]
    );
    let snapshot = host
        .instance_fact_snapshot(InstanceFactContext {
            instance_id: ALIAS.to_owned(),
            server_id: "test".to_owned(),
            game_id: "neutral".to_owned(),
        })
        .expect("instance facts");

    let select = PreparedContainedTask::load_with_vision_provider(
        ALIAS,
        &single_frame_select_package(counted_policy(MAX_AGE_MS), false),
        Arc::new(CardNames),
    )
    .expect("the select package is admitted");
    let evaluate = |now_unix_ms: u64| {
        let dry_run = actingcommand_execution_kernel::dry_run_select(
            &select,
            "pick",
            &frame(&cards(0..6)),
            (&snapshot, now_unix_ms),
            &select.default_run_parameters(),
            None,
        )
        .expect("the select step evaluates on the frame");
        serde_json::to_value(&dry_run.decision).expect("decision JSON")
    };
    let status = |decision: &Value, candidate: &str| {
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
        json!({"kind": "selected", "count": 1}),
        "{fresh}"
    );
    assert_eq!(fresh["selected"], json!(["layout/cards#00"]));
    assert_eq!(status(&fresh, "layout/cards#00"), "ranked");
    assert_eq!(status(&fresh, "layout/cards#01"), "gate_rejected");
    assert_eq!(status(&fresh, "layout/cards#02"), "gate_rejected");
    assert_eq!(status(&fresh, "layout/cards#05"), "gate_rejected");

    let stale = evaluate(list_record.observed_at_unix_ms + MAX_AGE_MS + 1);
    assert_eq!(stale["outcome"]["kind"], "unknown", "{stale}");
    assert_eq!(stale["outcome"]["reason"], "fact_stale", "{stale}");
    assert_eq!(stale["candidates"], json!([]), "{stale}");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}
