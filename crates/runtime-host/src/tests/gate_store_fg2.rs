// SPDX-License-Identifier: AGPL-3.0-only

// STORE-FG-2 (0.12.29; design `rd-1029/design-store-fg.md` §2, §5 rows T1 to T9; Alice's ruling
// of 2026-10-29 10:1x and her questions 95 to 97, option A; register §1.206, provisional): a
// store update package's own run taps its declared store step while exactly its store is in
// front, and only when `store_foreground.allowed_application_ids` lists that store; every other
// input expects the assigned application, as before.
//
// - T1 to T5, the resource side's five realistic scenarios, end to end through the
//   contained-task port: the right listing; another application's listing; the store gone (the
//   game in front) or a sign-in page (or another package in front); the update finishing and the
//   game relaunched; no update needed.
// - T6: a store the list does not name ends the run `application_store_not_allowed` before any
//   capture or input.
// - T7: an ordinary package's tap, a Lab operation of the store step and a client `Input`, each
//   with the store in front, are refused `application_not_foreground` (today's guard).
// - T8: inside a store run, a step that is not a declared store step, with the store in front,
//   is refused (the non-`Tap` half is `host::foreground_gate::store_fg2_tests`).
// - T9: on a bound handset under `device_state.in_use_gate`, the declared store in front does
//   not hold a store run (`device_in_use`); it still holds an ordinary run.
//
// A synthetic `neutral` game, 16 x 9 frames on the manual Runtime clock (every kernel wait moves
// it and returns at once). Pixel 0 names the page, pixel 1 is the guard's ready flag (the fake's
// green), pixel 2 carries the text a fake OCR provider reads: the package's own listing, another
// listing, or nothing. The device shows segment `n` of a scene once `n` effects (inputs and
// application commands) happened, each segment's frames one per capture (the last repeating),
// and Android reports the segment's package in front. The store is a made-up id; the assigned
// application is the fake's `neutral.application`.

use super::gate_398_rd1b::{bound, resolved_handset};
use super::*;
use crate::codes::DeviceStateHostCode;
use actingcommand_contract::{
    APPLICATION_FOREGROUND_FACT_KEY, ContainedLabOperationRequest, LabOperationSelection,
    LabProjectionHint, ObservationFramePlan, RuntimeConfigManifest, RuntimePayload,
};
use actingcommand_recognition_pack::OcrProviderTextBlock;
use actingcommand_runtime_client::{RuntimeChannel, RuntimeClient, RuntimeClientConfig};
use actingcommand_runtime_config::{
    DefaultsFile, annotate_settable, mechanism_rows, render_parameters,
};
use serde_json::{Value, json};

const ALIAS: &str = "neutral.instance";
/// The store the package declares.
const STORE: &str = "test.store.alpha";
/// Another store, listed instead of [`STORE`] in T6.
const OTHER_STORE: &str = "test.store.beta";
/// The fake's assigned application.
const GAME: &str = "neutral.application";
/// Another package resumed in front of a listing frame (T3b).
const OTHER_PACKAGE: &str = "neutral.other.signin";
const SERIAL: &str = "TESTSERIAL0429";
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const NOT_FOREGROUND: &str = "application_not_foreground";
const NOT_ALLOWED: &str = "application_store_not_allowed";
const ERROR_PAGE: &str = "contained_task_error_page_reached";

const DIALOG: [u8; 3] = [255, 0, 0];
const LISTING: [u8; 3] = [0, 0, 255];
const UPDATING: [u8; 3] = [255, 255, 0];
const DONE: [u8; 3] = [0, 255, 255];
const HOME: [u8; 3] = [255, 0, 255];
const SIGN_IN: [u8; 3] = [128, 128, 128];
/// Pixel 2 of the package's own listing, read as its title.
const OWN_TITLE: [u8; 3] = [10, 10, 10];
/// Pixel 2 of another application's listing, read as that listing's title.
const OTHER_TITLE: [u8; 3] = [20, 20, 20];
const BLANK: [u8; 3] = [0, 0, 0];
/// The ordinary package's pages (`neutral_contained_task_package`): `home`, then `terminal`.
const ORDINARY_HOME: [u8; 3] = [255, 0, 0];
const ORDINARY_TERMINAL: [u8; 3] = [0, 0, 255];

/// Reads the text pixel 2 shows: the package's own listing title, another listing's, or `blank`.
#[derive(Debug)]
struct ListingText;

impl VisionProvider for ListingText {
    fn require_ocr_model(
        &self,
        _model_ref: &str,
        _model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        Ok(())
    }

    fn require_nn_model(
        &self,
        _model_ref: &str,
        _model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "no NN model in this fixture",
        ))
    }

    fn read_text(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderResult, VisionProviderError> {
        let width = usize::try_from(request.frame.width).expect("frame width");
        let x = usize::try_from(request.region.x).expect("region x");
        let y = usize::try_from(request.region.y).expect("region y");
        let at = (y * width + x) * 3;
        let text = match request.frame.rgb8_pixels.get(at..at + 3) {
            Some(rgb) if rgb == OWN_TITLE.as_slice() => "Neutral listing",
            Some(rgb) if rgb == OTHER_TITLE.as_slice() => "Another listing",
            _ => "blank",
        };
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            text: text.to_owned(),
            blocks: vec![OcrProviderTextBlock {
                text: text.to_owned(),
                rect: request.region,
                confidence: Some(0.99),
            }],
            confidence: Some(0.99),
            detection: None,
        })
    }

    fn classify(
        &self,
        _request: NnProviderRequest<'_>,
    ) -> Result<NnProviderResult, VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "no NN model in this fixture",
        ))
    }
}

/// The pixels a screen shows: its page colour on pixel 0, its text on pixel 2.
fn pixels(screen: &str) -> Vec<(usize, [u8; 3])> {
    let (page, text) = match screen {
        "dialog" => (DIALOG, BLANK),
        "listing" => (LISTING, OWN_TITLE),
        "another_listing" => (LISTING, OTHER_TITLE),
        "sign_in" => (SIGN_IN, BLANK),
        "updating" => (UPDATING, BLANK),
        "done" => (DONE, BLANK),
        "home" => (HOME, BLANK),
        other => panic!("unknown fixture screen {other}"),
    };
    vec![(0, page), (2, text)]
}

/// What the device shows once a number of effects happened: these screens, one per capture, the
/// last repeating, with `front` the package Android reports in front.
struct Segment {
    screens: &'static [&'static str],
    front: Option<&'static str>,
}

const fn segment(screens: &'static [&'static str], front: &'static str) -> Segment {
    Segment {
        screens,
        front: Some(front),
    }
}

/// Scripts the device: segment `n` of `segments` once `n` effects happened (inputs and
/// application commands from now on), the last segment for every later effect.
fn scene(state: &Arc<FakeState>, segments: Vec<Segment>) {
    let base =
        state.input_count.load(Ordering::Acquire) + state.application_count.load(Ordering::Acquire);
    let segments = Arc::new(segments);
    let device = Arc::downgrade(state);
    let shown = std::sync::Mutex::new((usize::MAX, 0_usize));
    let frames = Arc::clone(&segments);
    *state.pixel_script.lock().expect("pixel script") =
        Some(Arc::new(move |_capture: usize, inputs: usize| {
            let applications = device
                .upgrade()
                .map_or(0, |state| state.application_count.load(Ordering::Acquire));
            let effects = (inputs + applications).saturating_sub(base);
            let segment = &frames[effects.min(frames.len() - 1)];
            let mut shown = shown.lock().expect("shown screens");
            if shown.0 != effects {
                *shown = (effects, 0);
            }
            let screen = segment.screens[shown.1.min(segment.screens.len() - 1)];
            shown.1 += 1;
            pixels(screen)
        }));
    *state.foreground_script.lock().expect("foreground script") =
        Some(Arc::new(move |inputs: usize, applications: usize| {
            let effects = (inputs + applications).saturating_sub(base);
            segments[effects.min(segments.len() - 1)]
                .front
                .map(str::to_owned)
        }));
}

/// Scripts the ordinary package's device: `home` until it takes one more input, then `terminal`,
/// with `front` in front throughout.
fn ordinary_scene(state: &Arc<FakeState>, front: &'static str) {
    let base = state.input_count.load(Ordering::Acquire);
    *state.pixel_script.lock().expect("pixel script") =
        Some(Arc::new(move |_capture: usize, inputs: usize| {
            vec![(
                0,
                if inputs > base {
                    ORDINARY_TERMINAL
                } else {
                    ORDINARY_HOME
                },
            )]
        }));
    *state.foreground_script.lock().expect("foreground script") =
        Some(Arc::new(move |_inputs: usize, _applications: usize| {
            Some(front.to_owned())
        }));
}

// ---- The store update package (design §6, the resource side's sketch).

fn guard(page: &str) -> Value {
    json!({"page_id": page, "target_id": "flag/ready",
           "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
           "color_probe": "flag/ready"})
}

fn colour(page: &str, expected: [u8; 3]) -> Value {
    json!({"type": "color", "id": format!("page/{page}"),
           "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": expected})
}

// An OCR target of pixel 2; §8.2: `expected` is a list matched with `contains`.
fn title(id: &str, expected: &str) -> Value {
    json!({"type": "ocr", "id": id, "region": {"x": 2, "y": 0, "width": 1, "height": 1},
           "languages": ["en"], "timeout_ms": 1000, "match_mode": "contains",
           "expected": [expected], "case_sensitive": false, "minimum_confidence": 0.5,
           "model_ref": "PP-OCRv6_medium", "model_sha256": MODEL_SHA256})
}

fn page(id: &str, required: &[&str]) -> Value {
    json!({"id": format!("neutral/{id}"), "required": required, "optional": [], "forbidden": []})
}

/// `neutral.store.update`: entry the game's dialog (or the listing, for a run that starts with
/// the store in front); `open_store` taps the dialog (an ordinary step), `tap_update` is the
/// store step (a guarded tap on the listing, its proof the listing's title), `wait_download`
/// waits on the updating page with no input, `restart` brings the game back; another listing and
/// a sign-in page are error pages.
fn store_package() -> Vec<u8> {
    let space = json!({"width": 16, "height": 9});
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.store.update",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": space.clone(),
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 8,
        "store_update": {
            "store_application_id": STORE,
            "store_operations": [{"operation": "tap_update", "proof": ["proof/title"]}]
        }
    });
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": space.clone(),
        "entry_page": ["dialog", "listing"],
        "target_page": "home",
        "error_pages": ["another_listing", "sign_in"],
        "operations": [
            {"id": "open_store", "from": "dialog", "to": ["listing", "done"],
             "click": {"kind": "point", "x": 1, "y": 0}, "guard": guard("dialog"),
             "retryable": false},
            {"id": "tap_update", "from": "listing", "to": "updating",
             "click": {"kind": "point", "x": 1, "y": 0}, "guard": guard("listing"),
             "retryable": false},
            {"id": "wait_download", "from": "updating", "wait": {"max_ms": 5000},
             "expect_after": {"page_id": "done", "timeout_ms": 2000, "interval_ms": 100}},
            {"id": "restart", "from": "done", "to": "home",
             "application": {"action": "restart"}}
        ]
    });
    let pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": space,
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            colour("dialog", DIALOG),
            colour("listing", LISTING),
            colour("updating", UPDATING),
            colour("done", DONE),
            colour("home", HOME),
            colour("sign_in", SIGN_IN),
            {"type": "color", "id": "flag/ready",
             "region": {"x": 1, "y": 0, "width": 1, "height": 1}, "expected": [0, 255, 0]},
            title("proof/title", "Neutral listing"),
            title("proof/other", "Another listing")
        ]
    });
    let pages = json!({"schema_version": "0.3", "pages": [
        page("dialog", &["page/dialog"]),
        page("listing", &["page/listing", "proof/title"]),
        page("another_listing", &["page/listing", "proof/other"]),
        page("sign_in", &["page/sign_in"]),
        page("updating", &["page/updating"]),
        page("done", &["page/done"]),
        page("home", &["page/home"])
    ]});
    let files: [(&str, Value); 5] = [
        ("control.json", control),
        (
            "resources/manifest.json",
            json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        ("resources/operations/task/task.json", task),
        ("resources/recognition/neutral.test.pack.json", pack),
        ("resources/recognition/neutral.test.pages.json", pages),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in files {
        zip.start_file(path, options).expect("zip entry");
        zip.write_all(&serde_json::to_vec(&value).expect("package JSON"))
            .expect("zip content");
    }
    zip.finish().expect("finish zip").into_inner()
}

// ---- The host.

/// The configuration manifest of a start whose file names `file`'s sections, as actingd
/// assembles it.
fn manifest(file: &Value) -> RuntimeConfigManifest {
    let (_, defaults) = DefaultsFile::shipped().expect("the defaults file parses");
    let mut parameters = render_parameters(&mechanism_rows(), &[], &defaults, file, &|_| None)
        .expect("the sections resolve");
    annotate_settable(&mut parameters).expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

fn allowing(stores: &[&str]) -> Value {
    json!({"store_foreground": {"allowed_application_ids": stores}})
}

struct Fixture {
    root: TempDir,
    host: RuntimeHost,
    state: Arc<FakeState>,
    store: PathBuf,
    ordinary: PathBuf,
}

/// A host whose emulator instance [`ALIAS`] reads text with [`ListingText`], on the manual
/// Runtime clock, its store allowlist `stores`.
fn fixture(stores: &[&str]) -> Fixture {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0)))
            .with_config_manifest(manifest(&allowing(stores))),
        Arc::new(
            FakeProvider::one(ALIAS, instance_id(), Arc::clone(&state))
                .with_vision_provider(Arc::new(ListingText)),
        ),
    )
    .expect("runtime host");
    packages(root, host, state)
}

fn packages(root: TempDir, host: RuntimeHost, state: Arc<FakeState>) -> Fixture {
    let store = root.path().join("store-update.zip");
    fs::write(&store, store_package()).expect("write the store package");
    let ordinary = root.path().join("ordinary.zip");
    fs::write(&ordinary, neutral_contained_task_package(true)).expect("write the ordinary package");
    Fixture {
        root,
        host,
        state,
        store,
        ordinary,
    }
}

impl Fixture {
    fn run_on(&self, alias: &str, package: &Path) -> RuntimeReceipt {
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        self.host
            .process_request_for_test(
                &runtime_request(
                    &ids,
                    RuntimeOperation::run_contained_task(
                        alias,
                        ids.mint_holder_id().expect("holder"),
                        ContainedTaskRequest::new(package.display().to_string())
                            .expect("task request"),
                    ),
                ),
                ConnectionId::new(1429).expect("connection"),
            )
            .expect("the run's receipt")
    }

    fn run_store(&self) -> RuntimeReceipt {
        self.run_on(ALIAS, &self.store)
    }

    fn run_ordinary(&self) -> RuntimeReceipt {
        self.run_on(ALIAS, &self.ordinary)
    }

    fn inputs(&self) -> usize {
        self.state.input_count.load(Ordering::Acquire)
    }

    fn applications(&self) -> usize {
        self.state.application_count.load(Ordering::Acquire)
    }

    fn captures(&self) -> usize {
        self.state.capture_count.load(Ordering::Acquire)
    }

    /// Every `application.foreground` the Runtime recorded, in order.
    fn foreground_records(&self) -> Vec<String> {
        self.host
            .query_persisted_events_for_test(EventQuery {
                event_type: Some(EventType::RuntimeFactRecorded),
                ..EventQuery::default()
            })
            .expect("runtime facts")
            .iter()
            .filter_map(|event| match event.payload() {
                EventPayload::Runtime(RuntimePayload::FactRecorded(recorded))
                    if recorded.record().key == APPLICATION_FOREGROUND_FACT_KEY =>
                {
                    match &recorded.record().value {
                        ContractFactValue::String(package) => Some(package.clone()),
                        _ => None,
                    }
                }
                _ => None,
            })
            .collect()
    }

    /// The `runtime.failed` records of `code`.
    fn failures(&self, code: &str) -> usize {
        self.host
            .query_persisted_events_for_test(EventQuery {
                event_type: Some(EventType::RuntimeFailed),
                ..EventQuery::default()
            })
            .expect("runtime failures")
            .iter()
            .filter(|event| {
                matches!(
                    event.payload(),
                    EventPayload::Runtime(RuntimePayload::Failed(payload))
                        if payload.outcome().code().as_str() == code
                )
            })
            .count()
    }

    fn close(self) {
        assert!(self.host.fatal_error().expect("runtime health").is_none());
        self.host.close().expect("close host");
    }
}

fn assert_completed_on_home(receipt: &RuntimeReceipt) {
    assert!(
        matches!(receipt.result(), Some(RuntimeResult::ContainedTaskCompleted {
            task_result: TaskOutcome::Success, final_page: Some(page), ..
        }) if page == "neutral/home"),
        "the run completes on home: {receipt:?}"
    );
}

/// The run ended without completing, not fatally, with `code`.
fn assert_refused(receipt: &RuntimeReceipt, code: &str) {
    assert_ne!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert!(
        !receipt.error_projection().expect("the run's failure").fatal,
        "{receipt:?}"
    );
    assert_eq!(receipt.outcome().code().as_str(), code, "{receipt:?}");
}

fn assert_ended_on_error_page(receipt: &RuntimeReceipt, page: &str) {
    assert_refused(receipt, ERROR_PAGE);
    assert!(
        outcome_str(receipt.outcome(), "page_id").is_some_and(|id| id.ends_with(page)),
        "the run ends on {page}: {receipt:?}"
    );
}

// T1 (design §5): the store page is the package's own listing. The dialog tap passes with the
// game in front, the listing page passes with its proof, the store tap passes with the store in
// front; the run waits while the store updates and restarts the game. One input each, and
// `application.foreground` records the store once.
#[test]
fn store_fg2_t1_the_right_listing_takes_the_store_tap_with_the_store_in_front() {
    let run = fixture(&[STORE]);
    scene(
        &run.state,
        vec![
            segment(&["dialog"], GAME),
            segment(&["listing"], STORE),
            segment(&["updating", "updating", "done"], STORE),
            segment(&["home"], GAME),
        ],
    );
    assert_completed_on_home(&run.run_store());
    assert_eq!(run.inputs(), 2, "open_store and tap_update");
    assert_eq!(run.applications(), 1, "the way back");
    let records = run.foreground_records();
    assert_eq!(
        records.iter().filter(|package| *package == STORE).count(),
        1,
        "{records:?}"
    );
    assert_eq!(
        records.last().map(String::as_str),
        Some(STORE),
        "{records:?}"
    );
    run.close();
}

// T2: the store page is another application's listing: the proof fails, the listing page does
// not match, no store step is dispatched and nothing is tapped after the dialog; the run ends on
// the package's error page.
#[test]
fn store_fg2_t2_another_listing_ends_on_its_error_page_with_no_store_tap() {
    let run = fixture(&[STORE]);
    scene(
        &run.state,
        vec![
            segment(&["dialog"], GAME),
            segment(&["another_listing"], STORE),
        ],
    );
    assert_ended_on_error_page(&run.run_store(), "another_listing");
    assert_eq!(run.inputs(), 1, "the dialog tap only");
    assert!(
        !run.foreground_records()
            .iter()
            .any(|package| package == STORE)
    );
    run.close();
}

// T3a: the store crashed: the frame still shows the listing, but Android reports the game. The
// store tap is refused `application_not_foreground` (it expects the store), nothing reaches the
// device, and the run fails.
#[test]
fn store_fg2_t3a_a_store_that_crashed_gets_no_tap() {
    let run = fixture(&[STORE]);
    scene(
        &run.state,
        vec![segment(&["dialog"], GAME), segment(&["listing"], GAME)],
    );
    assert_refused(&run.run_store(), NOT_FOREGROUND);
    assert_eq!(run.inputs(), 1, "the dialog tap only");
    assert_ne!(run.failures(NOT_FOREGROUND), 0, "the refused store tap");
    run.close();
}

// T3b: the store asks to sign in: the frame matches the package's sign-in error page and the run
// ends there with no store tap. And another package resumed in front of a listing frame: the
// store tap is refused.
#[test]
fn store_fg2_t3b_a_sign_in_page_ends_the_run_and_another_package_in_front_gets_no_tap() {
    let run = fixture(&[STORE]);
    scene(
        &run.state,
        vec![segment(&["dialog"], GAME), segment(&["sign_in"], STORE)],
    );
    assert_ended_on_error_page(&run.run_store(), "sign_in");
    assert_eq!(run.inputs(), 1, "the dialog tap only");

    scene(
        &run.state,
        vec![
            segment(&["dialog"], GAME),
            segment(&["listing"], OTHER_PACKAGE),
        ],
    );
    assert_refused(&run.run_store(), NOT_FOREGROUND);
    assert_eq!(run.inputs(), 2, "the second run's dialog tap only");
    run.close();
}

// T4: the update finishes and the game is relaunched: the run waits on the updating page and
// restarts the game. The next ordinary tap passes the normal gate and records the assigned
// application in front again; a later ordinary tap with the store still in front is refused as
// today.
#[test]
fn store_fg2_t4_after_the_update_the_game_is_back_and_the_ordinary_guard_holds() {
    let run = fixture(&[STORE]);
    scene(
        &run.state,
        vec![
            segment(&["dialog"], GAME),
            segment(&["listing"], STORE),
            segment(&["updating", "updating", "updating", "done"], STORE),
            segment(&["home"], GAME),
        ],
    );
    assert_completed_on_home(&run.run_store());
    assert_eq!(run.applications(), 1, "restart");
    assert_eq!(run.inputs(), 2, "no input while the store updates");

    ordinary_scene(&run.state, GAME);
    let receipt = run.run_ordinary();
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert_eq!(run.inputs(), 3);
    let records = run.foreground_records();
    assert_eq!(
        records.last().map(String::as_str),
        Some(GAME),
        "{records:?}"
    );

    ordinary_scene(&run.state, STORE);
    assert_refused(&run.run_ordinary(), NOT_FOREGROUND);
    assert_eq!(run.inputs(), 3, "nothing reaches the store");
    run.close();
}

// T5: no update is needed: after the dialog the store shows the listing's current state (its
// open label, page `done`); no store tap is made, the game is restarted and the run completes.
#[test]
fn store_fg2_t5_no_update_needed_makes_no_store_tap() {
    let run = fixture(&[STORE]);
    scene(
        &run.state,
        vec![
            segment(&["dialog"], GAME),
            segment(&["done"], STORE),
            segment(&["home"], GAME),
        ],
    );
    assert_completed_on_home(&run.run_store());
    assert_eq!(run.inputs(), 1, "the dialog tap only");
    assert_eq!(run.applications(), 1, "restart");
    assert!(
        !run.foreground_records()
            .iter()
            .any(|package| package == STORE)
    );
    run.close();
}

// T6: the allowlist. Empty (the default) or naming another store, the store run ends
// `application_store_not_allowed` (values `alias`, `application_id`) before any capture or
// input. A listed store passes (T1).
#[test]
fn store_fg2_t6_a_store_the_allowlist_does_not_name_ends_the_run_before_any_capture() {
    for stores in [&[][..], &[OTHER_STORE][..]] {
        let run = fixture(stores);
        scene(
            &run.state,
            vec![segment(&["dialog"], GAME), segment(&["listing"], STORE)],
        );
        let captures = run.captures();
        let receipt = run.run_store();
        assert_refused(&receipt, NOT_ALLOWED);
        assert_eq!(outcome_str(receipt.outcome(), "alias"), Some(ALIAS));
        assert_eq!(
            outcome_str(receipt.outcome(), "application_id"),
            Some(STORE)
        );
        assert_eq!(run.captures(), captures, "no capture: {stores:?}");
        assert_eq!(run.inputs(), 0, "no input: {stores:?}");
        run.close();
    }
}

// T7 (today's guard, pinned): with the store in front, an ordinary package's tap, a Lab
// operation of the store step and a client `Input` are each refused `application_not_foreground`,
// and nothing reaches the device.
#[test]
fn store_fg2_t7_ordinary_lab_and_client_inputs_never_tap_the_store() {
    let run = fixture(&[STORE]);
    ordinary_scene(&run.state, STORE);
    assert_refused(&run.run_ordinary(), NOT_FOREGROUND);

    // A client `Input` under the client's own lease, which it then gives back.
    let refused = run.failures(NOT_FOREGROUND);
    {
        let mut client = TestClient::connect(&run.host);
        let (_, token) = client.acquire(ALIAS);
        let request = client.request(RuntimeOperation::Input {
            frame: None,
            token: token.clone(),
            action: InputAction::Tap { x: 1, y: 0 },
        });
        let receipt = client.send(&request);
        assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{receipt:?}");
        assert_eq!(receipt.outcome().code().as_str(), NOT_FOREGROUND);
        let release = client.request(RuntimeOperation::ReleaseLease { token });
        let _ = client.send(&release);
    }
    assert!(run.failures(NOT_FOREGROUND) > refused, "the client's tap");

    // A Lab operation of the declared store step: a declared operation's run never routes it.
    scene(&run.state, vec![segment(&["listing"], STORE)]);
    let refused = run.failures(NOT_FOREGROUND);
    {
        let lab = RuntimeClient::connect(RuntimeClientConfig::new(
            run.root.path(),
            RuntimeChannel::Lab,
            EventActor::Lab,
            EventSource::Lab,
        ))
        .expect("Lab client");
        // The operation's own result is the Lab's (its record and receipt); the refused tap is
        // read from the ledger below.
        let _ = lab.begin_debug_session().map(|session| {
            session.run_contained_lab_declared_operation(
                ALIAS,
                ContainedLabOperationRequest {
                    package_path: run.store.display().to_string(),
                    selection: LabOperationSelection::Operation {
                        operation_id: "tap_update".to_owned(),
                        seed: None,
                    },
                    projection_hint: LabProjectionHint::default(),
                    after: None,
                },
                ObservationFramePlan::SINGLE,
                Duration::from_millis(8_000),
            )
        });
    }
    assert!(
        run.failures(NOT_FOREGROUND) > refused,
        "the Lab operation's tap"
    );
    assert_eq!(run.inputs(), 0, "nothing reached the device");
    run.close();
}

// T8: inside a store run, a step that is no declared store step (the dialog tap) with the store
// already in front is refused, as every ordinary input is.
#[test]
fn store_fg2_t8_a_non_store_step_with_the_store_in_front_is_refused() {
    let run = fixture(&[STORE]);
    scene(&run.state, vec![segment(&["dialog"], STORE)]);
    assert_refused(&run.run_store(), NOT_FOREGROUND);
    assert_eq!(run.inputs(), 0);
    run.close();
}

// T9 (design §2.6): on a bound handset under `device_state.in_use_gate`, the declared store in
// front does not hold the store run (`device_in_use`), which starts on the listing and completes;
// an ordinary run with the store in front is still held.
#[test]
fn store_fg2_t9_the_declared_store_in_front_does_not_hold_a_store_run_on_a_handset() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.observed_open.store(true, Ordering::Release);
    state.physical_task_geometry.store(true, Ordering::Release);
    let handset = Arc::new(FakeHandset::default());
    let handset_id = instance_id();
    let mut file = allowing(&[STORE]);
    file["device_state"] = json!({
        "in_use_gate": true,
        "sample_interval_ms": 3_600_000,
        "fact_ttl_ms": 3_600_000
    });
    let host = RuntimeHost::start(
        config(&root)
            .with_policy_inputs(PolicyInputSnapshot::new(
                pending_policy_facts(),
                pending_policy_resources(),
            ))
            .with_runtime_clock(Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0)))
            .with_config_manifest(manifest(&file)),
        Arc::new(
            FakeProvider::from_entries([(
                POLICY_INSTANCE_ALIAS.to_owned(),
                handset_id,
                Arc::clone(&state),
            )])
            .with_device_endpoints()
            .with_resolved_for(
                POLICY_INSTANCE_ALIAS,
                resolved_handset(
                    POLICY_INSTANCE_ALIAS,
                    handset_id,
                    bound(POLICY_INSTANCE_ALIAS, SERIAL),
                ),
            )
            .with_handset(POLICY_INSTANCE_ALIAS, Arc::clone(&handset))
            .with_vision_provider(Arc::new(ListingText)),
        ),
    )
    .expect("runtime host with a handset");
    let run = packages(root, host, state);
    handset.script(|handset| handset.foreground = Some(STORE));

    scene(
        &run.state,
        vec![
            segment(&["listing"], STORE),
            segment(&["updating", "done"], STORE),
            segment(&["home"], GAME),
        ],
    );
    assert_completed_on_home(&run.run_on(POLICY_INSTANCE_ALIAS, &run.store));
    assert_eq!(run.inputs(), 1, "the store tap");

    ordinary_scene(&run.state, STORE);
    let receipt = run.run_on(POLICY_INSTANCE_ALIAS, &run.ordinary);
    assert_ne!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert!(
        receipt.outcome().is(DeviceStateHostCode::InUse),
        "{receipt:?}"
    );
    assert_eq!(run.inputs(), 1, "the ordinary run did not start");
    run.close();
}
