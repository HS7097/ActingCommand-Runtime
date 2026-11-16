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
//
// Fix round 1 (brief rd-1116/brief-463-s26-fix1.md, the coordinator's rulings R1, R2 and R5 on the
// independent review): the wanted value of every select is the package's `ordered` variable whose
// default is the recorded one, marked or not, so the mark means only "not refined yet"; `choice:
// false` on a step without the mark is refused (`no_choice`).
// - `s26_a_select_given_whole_without_the_mark_holds_the_same_variable` (the slice's test 5, turned
//   the other way round by R1): a select given whole without the mark: `ready`, no `choice` in the
//   recording, the package's variable and policy those of the same select marked first; a run handed
//   no value taps the default's slot, as the constant of before did.
// - `s26_fix1_the_first_name_of_the_order_on_the_screen_is_chosen`: handed `["item_a", "item_b"]`
//   the select chooses item_a's slot, handed `["item_b", "item_a"]` item_b's (the term's sign).
// - `s26_fix1_no_name_of_the_order_on_the_screen_ends_on_the_on_empty_key`: on a list showing
//   item_a in both slots, the order `["item_b"]` selects nothing and decides the `on_empty` key.
// - `s26_fix1_choice_false_refuses_an_unmarked_step_and_takes_off_only_the_mark`: on a step without
//   the mark `record_select_invalid` with `no_choice`, nothing written; on a marked step not refined
//   the mark goes and nothing is missing; on a refined step the mark goes and the select stays, the
//   package holding its variable.
// - `s26_fix1_a_marked_step_keeps_each_rule_of_a_select_step`: on a step marked as a choice an
//   optional flag, an application operation, a click retry, a drag and a click that follows a mark
//   are each refused with their words, nothing written.
// - `s26_fix1_a_choice_is_marked_and_refined_on_a_stopped_recording`: on the stopped recording the
//   last step takes no mark (`on_final_step`); step 1 takes it, is listed and refused by `record
//   stop`, the package generated before kept; refined, it is `ready` and regenerated.
// - `s26_fix1_a_step_inserted_beside_a_marked_step_leaves_the_mark_where_it_was`: an optional step
//   inserted after the marked step 1 of the stopped recording takes the next serial number and no
//   mark; step 1 keeps its mark as it was and is listed as before.
//
// Frames are synthetic: 96x64 pixels of flat gray. The scripted OCR provider reads one block per
// marker pixel inside the requested region, its text chosen by the marker's gray value, at
// confidence 0.9; a region without a marker reads nothing. Slot 0 is (8, 8, 32, 32) with its name
// read in (10, 28, 28, 8), slot 1 (48, 8, 32, 32) with (50, 28, 28, 8); a name's marker stands at
// (12 + 40k, 30): item_a (ALPHA) in slot 0, item_b (BRAVO) in slot 1. The color mark `state/page` on
// a flat area recognizes the list page and guards the select. Fix round 1: the pop-up inserted after
// the stop is the flat screen with a square of gray 120 at (20, 50, 4, 4), its color mark
// `popup/notice`, closed at (16, 20, 24, 16). No device is used; the tests read no clock and never
// sleep. Every expected value comes from the work order's lines and the values above.

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
    OfflineDecision, PreparedContainedTask, SelectionDryRun, dry_run_select,
    simulate_contained_task_at,
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
// Fix round 1: the key of the select's empty end, `<task_id>_not_found` by default.
const NOT_FOUND: &str = "neutral_task_not_found";
// Fix round 1: the pop-up's gray and its color mark.
const POPUP_GRAY: u8 = 120;
const POPUP_MARK: &str = "popup/notice";
// The self-checks of `record stop`, in the order they run (#463 S2-10, adapted: `cases` is the
// last).
const EVERY_CHECK: [&str; 8] = [
    "frames",
    "admission",
    "first_decision",
    "cross_check",
    "endings",
    "selects",
    "final_package",
    "cases",
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
// Fix round 1: the next screen's noise patch, the pop-up's square and its close click.
const PATCH: RecordRect = rect(40, 46, 12, 12);
const POPUP_AREA: RecordRect = rect(20, 50, 4, 4);
const POPUP_CLICK: RecordRect = rect(16, 20, 24, 16);

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
    list_of([ALPHA, BRAVO])
}

// The list showing in slot `k` the name of the marker given for it.
fn list_of(markers: [u8; 2]) -> Vec<u8> {
    let mut values = vec![BACKGROUND; (WIDTH * HEIGHT) as usize];
    for (slot, marker) in (0_u32..).zip(markers) {
        values[(30 * WIDTH + 12 + 40 * slot) as usize] = marker;
    }
    png(&values)
}

// Fix round 1: the pop-up, the flat screen with the square of `POPUP_GRAY` on `POPUP_AREA`.
fn popup() -> Vec<u8> {
    let mut values = vec![BACKGROUND; (WIDTH * HEIGHT) as usize];
    for row in POPUP_AREA.y..POPUP_AREA.y + POPUP_AREA.height {
        for column in POPUP_AREA.x..POPUP_AREA.x + POPUP_AREA.width {
            values[(row as u32 * WIDTH + column as u32) as usize] = POPUP_GRAY;
        }
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

    // Fix round 1: the session file's status changed as the `record` command changes it, so that
    // a stopped recording takes `record mark` ("Marking after stop").
    fn session_status(&self, status: &str) {
        let path = old_record_path(&self.state(), INSTANCE);
        let mut session: Value =
            serde_json::from_slice(&fs::read(&path).expect("session file")).expect("session");
        session["status"] = json!(status);
        fs::write(&path, serde_json::to_vec(&session).expect("session JSON"))
            .expect("session file");
    }

    // Fix round 1: writes `bytes` as the frame file `name` on the caller's side and returns its
    // path.
    fn frame_file(&self, name: &str, bytes: &[u8]) -> String {
        let path = self.temp.path().join(format!("{name}.png"));
        fs::write(&path, bytes).expect("frame file");
        path.display().to_string()
    }

    // Fix round 1: step 1 with its page mark and no choice, step 2 the next screen, generated by
    // `record stop` and then stopped as the command leaves it.
    fn stopped_without_a_choice(&self) -> Value {
        self.press_on_the_list();
        self.marked(json!({"step": 1, "add": [page_mark()]}));
        self.mark_the_next_screen();
        let generated = self.stop();
        assert_eq!(generated["lab_status"], "generated", "{generated}");
        self.session_status("stopped");
        generated
    }

    // Fix round 1: step 1 marked as a choice and refined by the select with the default item_b,
    // then `record stop`: the entries of the package it generated.
    fn refined_package(&self) -> BTreeMap<String, Vec<u8>> {
        self.press_on_the_list();
        self.mark_the_choice();
        self.mark_the_next_screen();
        self.marked(json!({"step": 1, "select": select(Some("item_a"), "item_b")}));
        let lab = self.stop();
        assert_eq!(lab["lab_status"], "generated", "{lab}");
        package(&lab)
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

// Fix round 1: the `select_problem` of a `record_select_invalid` refusal, its outcome carrying the
// same word.
fn select_problem(error: LabError) -> String {
    assert_eq!(
        error.code,
        LabCode::RecordSelectInvalid.as_str(),
        "{}",
        error.message
    );
    let details = error.details.expect("details");
    assert_eq!(
        details["outcome"]["values"]["select_problem"], details["select_problem"],
        "{details}"
    );
    details["select_problem"]
        .as_str()
        .expect("a word")
        .to_owned()
}

// The kernel's dry run of the package's select step on `frame` with the variable `list_item`
// handed `order`, at `DRY_RUN_UNIX_MS` on an instance snapshot without records.
fn dry_run_with_order(
    prepared: &PreparedContainedTask,
    frame: &Frame,
    order: &[&str],
) -> SelectionDryRun {
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
    dry_run_select(
        prepared,
        OPERATION,
        frame,
        (&snapshot, DRY_RUN_UNIX_MS),
        &parameters,
        None,
    )
    .expect("the dry run of the select step")
}

// The slot the kernel's dry run of the package's select step chooses on `frame` with the variable
// `list_item` handed `order`; `None` when it chooses none.
fn chosen_with_order(
    prepared: &PreparedContainedTask,
    frame: &Frame,
    order: &[&str],
) -> Option<u32> {
    let dry = dry_run_with_order(prepared, frame, order);
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

// Fix round 1 (the coordinator's ruling R1 turns the slice's test 5 the other way round): a select
// given whole in one request, without the mark, holds the variable and the policy of the same
// select marked first and refined; a run handed no value taps the default's slot, as the constant
// of before did.
#[test]
fn s26_a_select_given_whole_without_the_mark_holds_the_same_variable() {
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
    assert_eq!(lab["first_decision"]["status"], "would_click", "{lab}");
    assert_eq!(lab["first_decision"]["slot"], 1, "{lab}");
    let entries = package(&lab);
    let control = document(&entries, "control.json");
    assert_eq!(control["variables"][0]["name"], WANTED, "{control}");
    assert_eq!(
        control["variables"][0]["default"],
        json!(["item_b"]),
        "{control}"
    );
    let policy = document(&entries, POLICY);
    assert_eq!(
        policy["gates"][0]["predicate"]["value"]["fact_key"], VARIABLE_KEY,
        "{policy}"
    );
    let marked = Bench::new().refined_package();
    assert_eq!(
        control["variables"],
        document(&marked, "control.json")["variables"]
    );
    assert_eq!(policy, document(&marked, POLICY));

    // A run handed no value reads the default and taps item_b's slot, the slot R1's constant gate
    // chose.
    let prepared =
        admit(&entries, &bench.ocr).unwrap_or_else(|error| panic!("admission: {}", error.code));
    let frame = Frame::from_png(list(), CaptureBackendName::AdbScreencap).expect("frame");
    let decision = simulate_contained_task_at(&prepared, vec![frame], 1)
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
}

// Fix round 1 (R5, the review's (h)): two names in an order. The select chooses the first of the
// order the screen shows, and with the order reversed the other one, which a term of the other sign
// would not.
#[test]
fn s26_fix1_the_first_name_of_the_order_on_the_screen_is_chosen() {
    let bench = Bench::new();
    let entries = bench.refined_package();
    let prepared =
        admit(&entries, &bench.ocr).unwrap_or_else(|error| panic!("admission: {}", error.code));
    let frame = Frame::from_png(list(), CaptureBackendName::AdbScreencap).expect("frame");

    assert_eq!(
        chosen_with_order(&prepared, &frame, &["item_a", "item_b"]),
        Some(0)
    );
    assert_eq!(
        chosen_with_order(&prepared, &frame, &["item_b", "item_a"]),
        Some(1)
    );
}

// Fix round 1 (R5, the review's (h)): no name of the order on the screen. On a list showing item_a
// in both slots the order `["item_b"]` selects nothing and decides the select's empty end, its
// `on_empty` key, the run's normal end there; the order `["item_a"]` selects on the same frame.
#[test]
fn s26_fix1_no_name_of_the_order_on_the_screen_ends_on_the_on_empty_key() {
    let bench = Bench::new();
    let entries = bench.refined_package();
    let prepared =
        admit(&entries, &bench.ocr).unwrap_or_else(|error| panic!("admission: {}", error.code));
    let frame =
        Frame::from_png(list_of([ALPHA, ALPHA]), CaptureBackendName::AdbScreencap).expect("frame");

    let dry = dry_run_with_order(&prepared, &frame, &["item_b"]);

    assert!(
        dry.decision.selected.is_empty(),
        "{:?}",
        dry.decision.selected
    );
    assert_eq!(dry.decision.outcome_key, NOT_FOUND);
    assert_eq!(
        document(&entries, POLICY)["applies_to"]["outcome_keys"]["empty"],
        NOT_FOUND
    );
    assert_eq!(chosen_with_order(&prepared, &frame, &["item_a"]), Some(0));
}

// Fix round 1 (R2, R5): `choice: false` on a step without the mark is `no_choice` and writes
// nothing; on a step marked and not refined it takes the mark off, and nothing is missing then; on
// a refined step it takes the mark off alone: the select stays and the package holds its variable.
#[test]
fn s26_fix1_choice_false_refuses_an_unmarked_step_and_takes_off_only_the_mark() {
    let bench = Bench::new();
    bench.press_on_the_list();
    bench.marked(json!({"step": 1, "add": [page_mark()]}));
    bench.mark_the_next_screen();

    let before = bench.recording_bytes();
    let error = bench
        .mark(json!({"step": 1, "choice": false}))
        .expect_err("no mark to take off");
    assert_eq!(select_problem(error), SelectProblem::NoChoice.as_str());
    assert_eq!(bench.recording_bytes(), before, "nothing written");

    bench.marked(json!({"step": 1, "choice": true}));
    assert_eq!(bench.ready()["missing"], json!([unrefined_item()]));
    let withdrawn = bench.marked(json!({"step": 1, "choice": false}));
    assert!(withdrawn.choice.is_none(), "the mark is taken off");
    assert_eq!(bench.recording()["steps"][0].get("choice"), None);
    let ready = bench.ready();
    assert_eq!(ready["status"], "ready", "{ready}");
    assert_eq!(ready["missing"], json!([]), "{ready}");

    let refined = Bench::new();
    refined.press_on_the_list();
    refined.mark_the_choice();
    refined.mark_the_next_screen();
    refined.marked(json!({"step": 1, "select": select(Some("item_a"), "item_b")}));
    let withdrawn = refined.marked(json!({"step": 1, "choice": false}));
    assert!(withdrawn.choice.is_none(), "the mark is taken off");
    let step = refined.recording()["steps"][0].clone();
    assert_eq!(step.get("choice"), None, "{step}");
    assert_eq!(
        step["select"]["declaration"],
        select(Some("item_a"), "item_b"),
        "{step}"
    );
    assert_eq!(step["click"]["source"], "select", "{step}");
    let lab = refined.stop();
    assert_eq!(lab["lab_status"], "generated", "{lab}");
    assert_eq!(
        document(&package(&lab), "control.json")["variables"][0]["default"],
        json!(["item_b"])
    );
}

// Fix round 1 (R5, the review's (m)): a step marked as a choice keeps a select step's rules from its
// mark on. Step 2, the open step, is marked; an optional flag, an application operation, a click
// with a retry, a drag and a click that follows its template are then each refused with their
// words, and nothing is written.
#[test]
fn s26_fix1_a_marked_step_keeps_each_rule_of_a_select_step() {
    let bench = Bench::new();
    bench.press_on_the_list();
    bench.marked(json!({"step": 1, "add": [page_mark()]}));
    bench.mark_the_next_screen();
    let marked = bench.marked(json!({"step": 2, "choice": true}));
    assert!(marked.choice.is_some(), "step 2 is marked");

    let retry = json!({"max_attempts": 2, "interval_ms": 1000});
    let cases = [
        (
            json!({"step": 2, "optional": true}),
            SelectProblem::OnOptionalStep,
        ),
        (
            json!({"step": 2, "application": {"action": "restart"}}),
            SelectProblem::WithApplication,
        ),
        (
            json!({"step": 2, "click": {"region": PATCH}, "retry": retry}),
            SelectProblem::WithRetry,
        ),
        (
            json!({"step": 2,
                   "click": {"region": SLOTS[0], "drag_to": SLOTS[1], "duration_ms": 300}}),
            SelectProblem::WithGesture,
        ),
        (
            json!({"step": 2, "click": {"follow": "ui/next"}}),
            SelectProblem::WithFollow,
        ),
    ];
    for (value, problem) in cases {
        let before = bench.recording_bytes();
        let error = bench.mark(value).expect_err("a rule of a select step");
        assert_eq!(select_problem(error), problem.as_str());
        assert_eq!(bench.recording_bytes(), before, "nothing written");
    }
}

// Fix round 1 (R5, the review's (m); "Marking a choice first", "Marking after stop"): on the stopped
// recording the last step takes no mark (`on_final_step`), nothing written; step 1 takes it, is
// listed by `record ready` and refused by `record stop`, which keeps the package it generated
// before; refined on the stopped recording, it is `ready` and generated again with its variable.
#[test]
fn s26_fix1_a_choice_is_marked_and_refined_on_a_stopped_recording() {
    let bench = Bench::new();
    let generated = bench.stopped_without_a_choice();
    let path = generated["path"].as_str().expect("package path").to_owned();
    let generated_bytes = fs::read(&path).expect("package");

    let before = bench.recording_bytes();
    let error = bench
        .mark(json!({"step": 2, "choice": true}))
        .expect_err("the last step of the stopped recording");
    assert_eq!(select_problem(error), SelectProblem::OnFinalStep.as_str());
    assert_eq!(bench.recording_bytes(), before, "nothing written");

    let marked = bench.marked(json!({"step": 1, "choice": true}));
    assert!(marked.after_stop);
    assert!(marked.choice.is_some(), "step 1 is marked");
    let draft = bench.ready();
    assert_eq!(draft["status"], "draft", "{draft}");
    assert_eq!(draft["missing"], json!([unrefined_item()]), "{draft}");
    let error = bench
        .stop_with(false)
        .expect_err("a choice not refined generates no package");
    assert_eq!(
        select_problem(error),
        SelectProblem::ChoiceNotRefined.as_str()
    );
    assert_eq!(
        fs::read(&path).expect("package"),
        generated_bytes,
        "the package generated before is kept"
    );

    let refined = bench.marked(json!({"step": 1, "select": select(Some("item_a"), "item_b")}));
    assert!(refined.after_stop);
    assert!(refined.choice.is_some(), "the mark stays beside the select");
    let ready = bench.ready();
    assert_eq!(ready["status"], "ready", "{ready}");
    let regenerated = bench.stop();
    assert_eq!(regenerated["lab_status"], "regenerated", "{regenerated}");
    assert_eq!(
        document(&package(&regenerated), "control.json")["variables"][0]["default"],
        json!(["item_b"])
    );
}

// Fix round 1 (R5, the review's (m)): an optional step inserted after the marked step 1 of the
// stopped recording takes the next serial number and no mark; step 1 keeps its mark as it was, and
// `record ready` still lists it by its serial number and its place in the package.
#[test]
fn s26_fix1_a_step_inserted_beside_a_marked_step_leaves_the_mark_where_it_was() {
    let bench = Bench::new();
    bench.stopped_without_a_choice();
    bench.marked(json!({"step": 1, "choice": true}));
    let mark = bench.recording()["steps"][0]["choice"].clone();
    assert!(mark["marked_at_unix_ms"].is_u64(), "{mark}");

    let inserted = bench.marked(json!({"optional_after": {
        "step": 1,
        "frame": bench.frame_file("popup", &popup()),
        "page": "notice",
        "add": [{"id": POPUP_MARK, "family": "color", "region": POPUP_AREA}],
        "click": {"region": POPUP_CLICK},
        "settle_ms": 1000
    }}));
    let inserted = serde_json::to_value(&inserted).expect("mark JSON");

    assert_eq!(inserted["status"], "optional_step_inserted", "{inserted}");
    assert_eq!(inserted["step"], 3, "{inserted}");
    assert_eq!(inserted.get("choice"), None, "{inserted}");
    let steps = bench.recording()["steps"].clone();
    assert_eq!(steps[0]["index"], 1, "{steps}");
    assert_eq!(steps[0]["choice"], mark, "{steps}");
    assert_eq!(steps[1]["index"], 2, "{steps}");
    assert_eq!(steps[1].get("choice"), None, "{steps}");
    assert_eq!(steps[2]["index"], 3, "{steps}");
    assert_eq!(steps[2]["inserted_after"], 1, "{steps}");
    assert_eq!(steps[2].get("choice"), None, "{steps}");
    let ready = bench.ready();
    let unrefined = ready["missing"]
        .as_array()
        .expect("missing")
        .iter()
        .filter(|item| item["what"] == ReadyMissing::SelectUnrefined.as_str())
        .collect::<Vec<_>>();
    assert_eq!(unrefined.len(), 1, "{ready}");
    assert_eq!(unrefined[0]["step"], 1, "{ready}");
    assert_eq!(unrefined[0]["artifact_step"], 1, "{ready}");
    assert_eq!(unrefined[0]["frame_id"], "f0001", "{ready}");
}
