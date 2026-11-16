// SPDX-License-Identifier: AGPL-3.0-only

// #463 S2-6 (contracts/lab-recording.md, "Marking a choice first", "Selecting in a list" and "What a
// recording still lacks: `record ready`"; register §1.442): a step that chooses one item among
// several is marked first as holding a choice and refined later by its select; the mark is kept in
// the recording, `record ready` lists a choice not refined (`select_unrefined`) and `record stop`
// refuses it as any other missing item, and the refined choice's wanted value is the package's
// `ordered` variable. Recordings are made as stage one makes them: `tap --record` through
// `record_press` with a fake Runtime (its input answer, its captures, the caller's clock moving
// only by its waits), so the press is a point tap (`source: "point"`); they are then marked with
// `record mark` and read with `record_ready`.
// - `s26_a_choice_not_refined_is_missing_and_record_stop_refuses_it`: step 1 marked as a choice
//   with its page mark: `record ready` is `draft` with step 1's `select_unrefined` on f0001 alone,
//   every self-check after it; `record stop` and its dry run refuse `record_select_invalid` with
//   `choice_not_refined`, `missing_total` 1 and the hint `record ready`, and write nothing.
// - `s26_refining_the_choice_ends_its_item_and_the_package_holds_its_variable`: the select declared
//   on the marked step: `ready`, nothing missing; `record stop` generates a package whose control
//   declares the `ordered` variable `list_item` (its options the names titled by their aliases, its
//   default the select's default alone) and whose policy reads it; a run handed nothing taps the
//   default's slot, one handed `["item_a"]` chooses item_a's slot.
// - `s26_the_choice_mark_survives_an_archive_and_a_reopen`: archived, the recording still lists
//   the item; reopened, the step keeps its mark as it was and the item, and refined it is `ready`.
// - `s26_a_point_tap_of_stage_one_is_the_demonstration_of_the_choice`: the recorded point tap is
//   the select's press: a pressed name in another slot is `select_press_mismatch`, nothing
//   written; the name in the tapped slot turns the tap into the select's click, its press and its
//   point as recorded kept beside the select.
// - `s26_a_choice_given_whole_in_one_request_answers_as_before`: a select given whole without the
//   mark: `ready`, no `choice` anywhere in the recording, and the package as R1 generates it (no
//   variable, the one gate keeping the default).
//
// Frames are synthetic: 96x64 pixels of flat gray. The scripted OCR provider reads one block per
// marker pixel inside the requested region, its text chosen by the marker's gray value, at
// confidence 0.9; a region without a marker reads nothing. Slot 0 is (8, 8, 32, 32) with its name
// read in (10, 28, 28, 8), slot 1 (48, 8, 32, 32) with (50, 28, 28, 8); a name's marker stands at
// (12 + 40k, 30): item_a (ALPHA) in slot 0, item_b (BRAVO) in slot 1. The color mark `state/page` on
// a flat area recognizes the list page and guards the select. No device is used; the tests read no
// clock and never sleep. Every expected value comes from the work order's lines and the values
// above.

use crate::codes::{LabCode, ReadyMissing, SelectProblem};
use crate::recording::crosscheck::admit;
use crate::recording::store::{lab_dir, old_record_path, recording_path};
use crate::{
    AfterCapture, AttachFrameRequest, CapturedFrame, ClickEffect, ClickPlan, CommitClickRequest,
    LAB_RECORD_MARK_SCHEMA, LabOcr, MarkOutcome, MarkRequest, PlanClickRequest, PointPress,
    RecordArchiveOptions, RecordFramePorts, RecordPoint, RecordPressRequest, RecordRect,
    RecordReopenOptions, RecordStartOptions, RecordStopOptions, RecordingLock, record_archive,
    record_mark, record_press, record_ready, record_reopen, record_start, record_stop,
};
use actingcommand_contract::{
    InputAction, InputFrameReference, InstanceFactContext, InstanceFactSnapshot, LabError,
    LabResult,
};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_execution_kernel::{
    OfflineDecision, PreparedContainedTask, dry_run_select, simulate_contained_task_at,
};
use actingcommand_pack_containment::{
    ContainmentLimits, ContentContainer, expand_content_container,
};
use actingcommand_recognition_pack::{
    NnProviderRequest, NnProviderResult, OcrProviderRequest, OcrProviderResult,
    OcrProviderTextBlock, PackRect, VisionProvider, VisionProviderError, VisionProviderErrorCode,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_0463_s26";
const TASK_ID: &str = "neutral_task";
const STARTED_AT: u64 = 1_700_000_000_000;
const WIDTH: u32 = 96;
const HEIGHT: u32 = 64;
const BACKGROUND: u8 = 60;
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PAGE: &str = "state/page";
const OPERATION: &str = "step_01_select";
const POLICY: &str = "resources/operations/neutral_task/policies/step_01_select.json";
const NEXT_SEED: u64 = 0x0463_0026;
// The wanted value's name, and the key under which a run reads the package's variable of it.
const WANTED: &str = "list_item";
const VARIABLE_KEY: &str = "session.dispatch.parameter.list_item";
// The instant of the kernel's dry runs below; the overlay is observed at it.
const DRY_RUN_UNIX_MS: u64 = 1_700_000_100_000;
// The point stage one taps on the list: inside slot 0, where item_a stands.
const TAP: (i32, i32) = (20, 20);
// The self-checks of `record stop`, in the order they run.
const EVERY_CHECK: [&str; 7] = [
    "frames",
    "admission",
    "first_decision",
    "cross_check",
    "endings",
    "selects",
    "final_package",
];

// The gray value of a marker pixel and the name the provider reads from it.
const ALPHA: u8 = 211;
const BRAVO: u8 = 212;
const MARKERS: [(u8, &str); 2] = [(ALPHA, "ALPHA"), (BRAVO, "BRAVO")];

const fn rect(x: i32, y: i32, width: i32, height: i32) -> RecordRect {
    RecordRect {
        x,
        y,
        width,
        height,
    }
}

// The slots' rectangles and the regions their names are read in.
const SLOTS: [RecordRect; 2] = [rect(8, 8, 32, 32), rect(48, 8, 32, 32)];
const NAMES: [RecordRect; 2] = [rect(10, 28, 28, 8), rect(50, 28, 28, 8)];

// Reads one block per marker pixel of the requested region at confidence 0.9; a region without a
// marker reads nothing, with no confidence.
#[derive(Debug)]
struct NamesOcr;

impl VisionProvider for NamesOcr {
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
        let region = request.region;
        let (right, bottom) = (region.x + region.width, region.y + region.height);
        let mut blocks = Vec::new();
        for y in region.y..bottom {
            for x in region.x..right {
                let index = (y as usize * request.frame.width as usize + x as usize) * 3;
                let pixel = request.frame.rgb8_pixels.get(index).copied();
                if let Some((_, name)) = MARKERS.iter().find(|(value, _)| Some(*value) == pixel) {
                    blocks.push(OcrProviderTextBlock {
                        text: (*name).to_owned(),
                        rect: PackRect {
                            x,
                            y,
                            width: 1,
                            height: 1,
                        },
                        confidence: Some(0.9),
                    });
                }
            }
        }
        let text = blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            confidence: (!blocks.is_empty()).then_some(0.9),
            text,
            blocks,
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

// A deterministic generator of neutral gray noise below every marker value.
struct Noise(u64);

impl Noise {
    fn between(&mut self, low: u8, high: u8) -> u8 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        low + ((self.0 >> 33) % (u64::from(high - low) + 1)) as u8
    }
}

// The PNG of a gray screen, row-major values drawn with equal red, green and blue.
fn png(values: &[u8]) -> Vec<u8> {
    let pixels = values
        .iter()
        .flat_map(|value| [*value; 3])
        .collect::<Vec<_>>();
    Frame::from_pixels(
        WIDTH,
        HEIGHT,
        pixels,
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
    .encode_png_fast()
    .expect("fixture PNG")
}

// The list: item_a's marker in slot 0, item_b's in slot 1.
fn list() -> Vec<u8> {
    let mut values = vec![BACKGROUND; (WIDTH * HEIGHT) as usize];
    for (slot, marker) in (0_u32..).zip([ALPHA, BRAVO]) {
        values[(30 * WIDTH + 12 + 40 * slot) as usize] = marker;
    }
    png(&values)
}

// The next screen: a noise patch at (40, 46), with no marker value.
fn next_screen() -> Vec<u8> {
    let mut noise = Noise(NEXT_SEED);
    let mut values = vec![BACKGROUND; (WIDTH * HEIGHT) as usize];
    for row in 0..12 {
        for column in 0..12 {
            values[((46 + row) * WIDTH + 40 + column) as usize] = noise.between(1, 199);
        }
    }
    png(&values)
}

// The page mark of the list: a color mark on a flat area no marker reaches.
fn page_mark() -> Value {
    json!({"id": PAGE, "family": "color", "region": rect(2, 50, 4, 4)})
}

// The select over the two slots with the wanted value's `default` and, when given, `pressed`.
fn select(pressed: Option<&str>, default: &str) -> Value {
    let mut select = json!({
        "slots": [{"rect": SLOTS[0], "name": NAMES[0]}, {"rect": SLOTS[1], "name": NAMES[1]}],
        "read": {"languages": ["en"], "timeout_ms": 1000, "minimum_confidence_milli": 800,
                 "model_ref": "neutral-ocr", "model_sha256": MODEL_SHA256},
        "names": [{"id": "item_a", "aliases": ["ALPHA"]}, {"id": "item_b", "aliases": ["BRAVO"]}],
        "match": {"max_distance": 1, "minimum_margin": 1},
        "wanted": {"name": WANTED, "default": default}
    });
    if let Some(pressed) = pressed {
        select["pressed"] = json!(pressed);
    }
    select
}

fn request(mut value: Value) -> MarkRequest {
    value["schema_version"] = json!(LAB_RECORD_MARK_SCHEMA);
    serde_json::from_value(value).expect("a record mark request")
}

fn inside(point: (i32, i32), area: RecordRect) -> bool {
    point.0 >= area.x
        && point.1 >= area.y
        && point.0 < area.x + area.width
        && point.1 < area.y + area.height
}

// What the command hands the recording for a performed input: no carrier package, no Runtime
// frames.
fn receipt() -> CommitClickRequest {
    CommitClickRequest {
        effect: ClickEffect::Performed,
        has_failure: false,
        carrier_package: None,
        req_id: None,
        correlation_id: None,
        action_id: None,
        lease_id: None,
        failure: None,
        before: None,
        after: None,
    }
}

/// The fake Runtime's captures and the caller's clock: the screens its captures take, in order,
/// and the clock, which moves only by the waits.
struct Screens {
    shots: VecDeque<Vec<u8>>,
    taken: u32,
    now: Duration,
}

impl RecordFramePorts for Screens {
    fn wait(&mut self, duration: Duration) {
        self.now += duration;
    }

    fn elapsed(&mut self) -> Duration {
        self.now
    }

    fn capture(&mut self) -> LabResult<CapturedFrame> {
        let png = self
            .shots
            .pop_front()
            .expect("a capture the test did not script");
        self.taken += 1;
        Ok(CapturedFrame {
            frame: AttachFrameRequest {
                png,
                source: "lab_capture".to_owned(),
                runtime_artifact: None,
                capture_backend: Some("adb_screencap".to_owned()),
                freshness: None,
            },
            input_frame: Some(InputFrameReference {
                frame_id: serde_json::from_value(json!(format!("frame_{:032x}", self.taken)))
                    .expect("a frame id"),
                width: WIDTH,
                height: HEIGHT,
            }),
        })
    }
}

// A state directory with an active session of `INSTANCE` (the header `record start` writes) and
// its Lab recording, before any step, with the scripted OCR provider. The lock is held as one
// command holds it and dropped before the directory.
struct Bench {
    lock: RecordingLock,
    ocr: LabOcr,
    temp: TempDir,
}

impl Bench {
    fn new() -> Self {
        let temp = TempDir::new().expect("temporary directory");
        let state = temp.path().join("state");
        fs::create_dir_all(&state).expect("state directory");
        let lock = RecordingLock::acquire(&state, INSTANCE, "gate_463_s26", "Lab-CLI")
            .expect("recording lock");
        let bench = Self {
            lock,
            ocr: LabOcr::with_provider(Arc::new(NamesOcr)),
            temp,
        };
        bench.header("active");
        record_start(
            &bench.lock,
            &RecordStartOptions {
                game: Some("neutral".to_owned()),
                server: Some("test".to_owned()),
                locale: Some("en-US".to_owned()),
                ..RecordStartOptions::default()
            },
        )
        .expect("record start");
        bench
    }

    fn state(&self) -> PathBuf {
        self.temp.path().join("state")
    }

    // The session header with `status`, as the `record` command writes it.
    fn header(&self, status: &str) {
        fs::write(
            old_record_path(&self.state(), INSTANCE),
            serde_json::to_vec(&json!({
                "record_id": RECORD_ID,
                "task_id": TASK_ID,
                "instance": INSTANCE,
                "status": status,
                "started_at_unix_ms": STARTED_AT
            }))
            .expect("session JSON"),
        )
        .expect("session file");
    }

    // `tap 20 20 --record` on the list, as stage one records it: the fake Runtime performs the
    // input; its first capture is the list (step 1, f0001), its one capture after the press the
    // next screen (step 2, f0002).
    fn press_on_the_list(&self) {
        let request = RecordPressRequest {
            click: PlanClickRequest::default(),
            point_press: Some(PointPress::Tap {
                point: RecordPoint { x: TAP.0, y: TAP.1 },
            }),
            after: AfterCapture {
                still_ms: Some(0),
                ..AfterCapture::for_press(Some(800))
            },
        };
        let mut device = Screens {
            shots: VecDeque::from([list(), next_screen()]),
            taken: 0,
            now: Duration::ZERO,
        };
        let mut send = |_: &ClickPlan, _: InputFrameReference| -> LabResult<CommitClickRequest> {
            Ok(receipt())
        };
        record_press(&self.lock, &request, &mut send, &mut device).unwrap_or_else(|error| {
            panic!(
                "the press: {}: {} {:?}",
                error.code, error.message, error.details
            )
        });
    }

    fn mark(&self, value: Value) -> Result<MarkOutcome, LabError> {
        record_mark(&self.lock, &request(value), false, &self.ocr)
    }

    // `record mark` of `value`, which must stand.
    fn marked(&self, value: Value) -> MarkOutcome {
        self.mark(value).unwrap_or_else(|error| {
            panic!(
                "record mark: {}: {} {:?}",
                error.code, error.message, error.details
            )
        })
    }

    // Step 1 marked as stage two marks the draft: its page mark and its choice.
    fn mark_the_choice(&self) -> MarkOutcome {
        self.marked(json!({"step": 1, "add": [page_mark()], "choice": true}))
    }

    // Step 2, the next screen, recognized by its own template mark.
    fn mark_the_next_screen(&self) {
        self.marked(json!({
            "step": 2,
            "add": [{"id": "ui/next", "family": "template", "region": rect(40, 46, 12, 12),
                     "search": rect(40, 46, 12, 12)}]
        }));
    }

    // `record ready`, read as the command reads it: the instance's recording, no option.
    fn ready(&self) -> Value {
        let outcome = record_ready(
            &self.state(),
            INSTANCE,
            None,
            &RecordStopOptions::default(),
            &self.ocr,
        )
        .unwrap_or_else(|error| {
            panic!(
                "record ready: {}: {} {:?}",
                error.code, error.message, error.details
            )
        });
        serde_json::to_value(outcome).expect("the answer as JSON")
    }

    fn stop_with(&self, dry_run: bool) -> LabResult<Value> {
        record_stop(
            &self.lock,
            &RecordStopOptions {
                dry_run,
                ..RecordStopOptions::default()
            },
            &self.ocr,
        )
        .map(|outcome| {
            let mut lab = serde_json::to_value(&outcome.lab).expect("lab JSON");
            lab["lab_status"] = json!(outcome.lab_status);
            lab
        })
    }

    // `record stop`: the `lab` output with its status.
    fn stop(&self) -> Value {
        self.stop_with(false).unwrap_or_else(|error| {
            panic!(
                "record stop: {}: {} {:?}",
                error.code, error.message, error.details
            )
        })
    }

    fn recording_bytes(&self) -> Vec<u8> {
        fs::read(recording_path(&self.state(), RECORD_ID)).expect("recording file")
    }

    fn recording(&self) -> Value {
        serde_json::from_slice(&self.recording_bytes()).expect("recording")
    }
}

// The entries of the package `record stop` wrote.
fn package(lab: &Value) -> BTreeMap<String, Vec<u8>> {
    let bytes = fs::read(lab["path"].as_str().expect("package path")).expect("package");
    let container = if lab["container"] == "zip" {
        ContentContainer::Zip
    } else {
        ContentContainer::Json
    };
    expand_content_container(&bytes, container, ContainmentLimits::default())
        .expect("package container")
}

fn document(entries: &BTreeMap<String, Vec<u8>>, path: &str) -> Value {
    serde_json::from_slice(entries.get(path).expect("the document")).expect("document JSON")
}

// Step 1's item of `record ready`: the step marked as a choice and not refined, on the frame its
// press was sent on.
fn unrefined_item() -> Value {
    json!({"order": 1, "step": 1, "artifact_step": 1, "frame_id": "f0001",
           "what": ReadyMissing::SelectUnrefined.as_str(),
           "code": LabCode::RecordSelectInvalid.as_str()})
}

// The refused select of `record_mark_rejected` with `reason`.
fn select_rejection(error: LabError, reason: &str) -> Value {
    assert_eq!(error.code, "record_mark_rejected", "{}", error.message);
    let details = error.details.expect("details");
    details["marks"]
        .as_array()
        .and_then(|marks| {
            marks
                .iter()
                .find(|mark| mark["id"] == "select" && mark["reason"] == reason)
        })
        .cloned()
        .unwrap_or_else(|| panic!("no select {reason} rejection in {details}"))
}

// The slot the kernel's dry run of the package's select step chooses on `frame` with the variable
// `list_item` handed `order`, at `DRY_RUN_UNIX_MS` on an instance snapshot without records; `None`
// when it chooses none.
fn chosen_with_order(
    prepared: &PreparedContainedTask,
    frame: &Frame,
    order: &[&str],
) -> Option<u32> {
    let variables = prepared
        .given_variable_values(&[(WANTED.to_owned(), json!(order))])
        .expect("the variable takes the order");
    let parameters = prepared.default_run_parameters().with_variables(variables);
    let snapshot = InstanceFactSnapshot {
        snapshot_id: "gate.s26".to_owned(),
        ledger_position: 0,
        context: InstanceFactContext {
            instance_id: "gate.s26".to_owned(),
            server_id: String::new(),
            game_id: String::new(),
        },
        records: Vec::new(),
    };
    let dry = dry_run_select(
        prepared,
        OPERATION,
        frame,
        (&snapshot, DRY_RUN_UNIX_MS),
        &parameters,
        None,
    )
    .expect("the dry run of the select step");
    dry.decision.selected.first().and_then(|id| {
        dry.candidate_projection
            .candidates()
            .iter()
            .find(|candidate| candidate.id == *id)
            .map(|candidate| candidate.instance_index)
    })
}

#[test]
fn s26_a_choice_not_refined_is_missing_and_record_stop_refuses_it() {
    let bench = Bench::new();
    bench.press_on_the_list();
    let marked = bench.mark_the_choice();
    let mark = serde_json::to_value(&marked).expect("mark JSON");
    assert!(mark["choice"]["marked_at_unix_ms"].is_u64(), "{mark}");
    assert_eq!(mark.get("select"), None, "{mark}");
    bench.mark_the_next_screen();

    let ready = bench.ready();
    assert_eq!(ready["status"], "draft", "{ready}");
    assert_eq!(ready["missing"], json!([unrefined_item()]), "{ready}");
    assert_eq!(ready["missing_total"], 1, "{ready}");
    assert_eq!(ready["checks_after"], json!(EVERY_CHECK), "{ready}");

    // `record stop` and its dry run refuse it as any other missing item, and write nothing.
    for dry_run in [true, false] {
        let before = bench.recording_bytes();
        let error = bench
            .stop_with(dry_run)
            .expect_err("a choice not refined generates no package");
        assert_eq!(
            error.code,
            LabCode::RecordSelectInvalid.as_str(),
            "{}",
            error.message
        );
        let details = error.details.expect("details");
        assert_eq!(
            details["select_problem"],
            SelectProblem::ChoiceNotRefined.as_str(),
            "{details}"
        );
        assert_eq!(
            details["outcome"]["values"]["select_problem"], details["select_problem"],
            "{details}"
        );
        assert_eq!(details["step"], 1, "{details}");
        assert_eq!(details["missing_total"], 1, "{details}");
        assert_eq!(details["hint"], "record ready", "{details}");
        assert_eq!(bench.recording_bytes(), before, "nothing written");
    }
    assert!(
        !lab_dir(&bench.state(), RECORD_ID).join("out").exists(),
        "no package written"
    );
}

#[test]
fn s26_refining_the_choice_ends_its_item_and_the_package_holds_its_variable() {
    let bench = Bench::new();
    bench.press_on_the_list();
    bench.mark_the_choice();
    bench.mark_the_next_screen();

    let refined = bench.marked(json!({"step": 1, "select": select(Some("item_a"), "item_b")}));
    assert!(refined.choice.is_some(), "the mark stays beside the select");
    let refined_select = serde_json::to_value(&refined.select).expect("select JSON");
    assert_eq!(
        refined_select["self_test"]["frames"],
        json!([{"frame_id": "f0001", "pressed": {"result": "selected", "slot": 0},
                "default": {"result": "selected", "slot": 1}}]),
        "{refined_select}"
    );

    let ready = bench.ready();
    assert_eq!(ready["status"], "ready", "{ready}");
    assert_eq!(ready["missing"], json!([]), "{ready}");
    assert_eq!(ready["checks_after"], json!([]), "{ready}");

    let lab = bench.stop();

    assert_eq!(lab["lab_status"], "generated", "{lab}");
    assert_eq!(lab["first_decision"]["status"], "would_click", "{lab}");
    assert_eq!(lab["first_decision"]["slot"], 1, "{lab}");
    let entries = package(&lab);
    let title = |text: &str| json!({"en-US": text});
    assert_eq!(
        document(&entries, "control.json")["variables"],
        json!([{
            "name": WANTED,
            "type": "ordered",
            "title": title(WANTED),
            "options": [{"value": "item_a", "title": title("ALPHA")},
                        {"value": "item_b", "title": title("BRAVO")}],
            "min_count": 1,
            "max_count": 2,
            "default": ["item_b"]
        }])
    );
    let policy = document(&entries, POLICY);
    assert_eq!(policy["facts"][0]["fact_key"], VARIABLE_KEY, "{policy}");
    assert_eq!(
        policy["facts"][0]["value_type"]["type"], "record_list",
        "{policy}"
    );
    assert_eq!(
        policy["gates"][0]["gate_id"], "wanted.list_item",
        "{policy}"
    );
    assert_eq!(
        policy["gates"][0]["predicate"]["value"]["fact_key"], VARIABLE_KEY,
        "{policy}"
    );

    // A run handed nothing reads the default and taps item_b's slot; one handed `["item_a"]`
    // chooses item_a's.
    let prepared =
        admit(&entries, &bench.ocr).unwrap_or_else(|error| panic!("admission: {}", error.code));
    let frame = || Frame::from_png(list(), CaptureBackendName::AdbScreencap).expect("frame");
    let decision = simulate_contained_task_at(&prepared, vec![frame()], 1)
        .expect("simulation")
        .decision;
    match decision {
        OfflineDecision::WouldClick {
            operation_label,
            action: InputAction::Tap { x, y },
            ..
        } => {
            assert_eq!(operation_label, OPERATION);
            assert!(inside((x, y), SLOTS[1]), "{x},{y}");
        }
        other => panic!("not a tap: {other:?}"),
    }
    assert_eq!(chosen_with_order(&prepared, &frame(), &["item_a"]), Some(0));
    assert_eq!(chosen_with_order(&prepared, &frame(), &["item_b"]), Some(1));
}

#[test]
fn s26_the_choice_mark_survives_an_archive_and_a_reopen() {
    let bench = Bench::new();
    bench.press_on_the_list();
    bench.mark_the_choice();
    bench.mark_the_next_screen();
    let mark = bench.recording()["steps"][0]["choice"].clone();
    assert!(mark["marked_at_unix_ms"].is_u64(), "{mark}");

    record_archive(
        &bench.lock,
        &RecordArchiveOptions {
            record_id: None,
            copy_to: None,
            written_by: "gate_463_s26".to_owned(),
        },
    )
    .unwrap_or_else(|error| panic!("record archive: {}: {:?}", error.code, error.details));
    bench.header("archived");
    let archived = bench.ready();
    assert_eq!(archived["session"], "archived", "{archived}");
    assert_eq!(archived["missing"], json!([unrefined_item()]), "{archived}");

    record_reopen(&bench.lock, &RecordReopenOptions::default())
        .unwrap_or_else(|error| panic!("record reopen: {}: {:?}", error.code, error.details));
    bench.header("active");
    assert_eq!(bench.recording()["steps"][0]["choice"], mark);
    let reopened = bench.ready();
    assert_eq!(reopened["session"], "active", "{reopened}");
    assert_eq!(reopened["status"], "draft", "{reopened}");
    assert_eq!(reopened["missing"], json!([unrefined_item()]), "{reopened}");

    bench.marked(json!({"step": 1, "select": select(Some("item_a"), "item_b")}));
    let refined = bench.ready();
    assert_eq!(refined["status"], "ready", "{refined}");
    assert_eq!(refined["missing"], json!([]), "{refined}");
}

#[test]
fn s26_a_point_tap_of_stage_one_is_the_demonstration_of_the_choice() {
    let bench = Bench::new();
    bench.press_on_the_list();
    bench.mark_the_choice();
    bench.mark_the_next_screen();
    let tapped = bench.recording()["steps"][0]["click"].clone();
    assert_eq!(tapped["source"], "point", "{tapped}");
    assert_eq!(tapped["rect"], json!(rect(TAP.0, TAP.1, 1, 1)), "{tapped}");

    // item_b stands in slot 1, and the point was tapped in slot 0: refused, nothing written.
    let before = bench.recording_bytes();
    let error = bench
        .mark(json!({"step": 1, "select": select(Some("item_b"), "item_b")}))
        .expect_err("the point is not in item_b's slot");
    let refused = select_rejection(error, "select_press_mismatch");
    assert_eq!(refused["frame_id"], "f0001", "{refused}");
    assert_eq!(refused["slot"], 1, "{refused}");
    assert_eq!(bench.recording_bytes(), before, "nothing written");

    // item_a stands where the point was tapped: the tap is the select's click, its press as
    // recorded, and the point tap is kept beside the select.
    let refined = bench.marked(json!({"step": 1, "select": select(Some("item_a"), "item_b")}));
    let click = serde_json::to_value(&refined.click).expect("click JSON");
    assert_eq!(click["source"], "select", "{click}");
    assert_eq!(click["rect"], json!(SLOTS[0]), "{click}");
    assert_eq!(
        click["execution"]["point"],
        json!({"x": TAP.0, "y": TAP.1}),
        "{click}"
    );
    assert_eq!(
        bench.recording()["steps"][0]["select"]["click_before"],
        json!({"rect": rect(TAP.0, TAP.1, 1, 1), "source": "point", "from": null})
    );
}

#[test]
fn s26_a_choice_given_whole_in_one_request_answers_as_before() {
    let bench = Bench::new();
    bench.press_on_the_list();
    bench.marked(json!({
        "step": 1,
        "add": [page_mark()],
        "select": select(Some("item_a"), "item_b")
    }));
    bench.mark_the_next_screen();

    let ready = bench.ready();
    assert_eq!(ready["status"], "ready", "{ready}");
    assert_eq!(ready["missing"], json!([]), "{ready}");
    let text = String::from_utf8(bench.recording_bytes()).expect("recording text");
    assert!(!text.contains("\"choice\""), "{text}");

    let lab = bench.stop();

    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let entries = package(&lab);
    assert_eq!(document(&entries, "control.json").get("variables"), None);
    let policy = document(&entries, POLICY);
    assert_eq!(policy["facts"], json!([]), "{policy}");
    assert_eq!(
        policy["gates"],
        json!([{"gate_id": "wanted.list_item",
                "predicate": {"kind": "string_in", "value": {"source": "field", "field": "name"},
                              "allowed": ["item_b"]},
                "on_unknown": {"kind": "drop_candidate"}}]),
        "{policy}"
    );
}
