// SPDX-License-Identifier: AGPL-3.0-only

// #463 S2-1c (contracts/lab-recording.md, "Self-checks" and "What a recording still lacks: `record
// ready`"; register §1.443): `record ready` lists every failure of each self-check, not only the
// first, and says for each self-check that ran whether its list is complete. A recording made as
// stage one makes it (three taps through `record_press` with a fake Runtime, its captures taking the
// scripted screens and the caller's clock moving only by its waits; four steps whose live frames are
// f0001, f0003, f0005 and f0006), each step marked with a colour mark on its square; a mark is then
// broken in the step table, as a stage-two write that went wrong would leave it.
// - `s21c_three_frames_that_fail_the_cross_check_are_three_items_and_stop_answers_the_first`: the
//   marks of steps 2, 3 and 4 expect another colour: three `cross_check` items, by step, each with
//   its frame and page, the cross-check complete and the final package not run; `record stop`
//   answers the first, `record_step_self_mismatch` for step 2 on f0003, as before.
// - `s21c_two_self_checks_that_fail_are_listed_in_the_order_the_checks_run`: step 1's mark expects
//   another colour: the first decision's item, then the cross-check's; `record stop` answers the
//   first decision's.
// - `s21c_a_check_that_must_stop_says_so_and_lists_one`: step 2's mark declares a negative colour
//   distance, which the kernel's admission refuses: one `admission` item, `complete: false` with
//   `whole_package`, and no later check run.
// - `s21c_a_recording_that_passes_has_every_check_complete_and_is_ready`.
// Frames are synthetic 160x90 screens (a square of one colour at the top left on gray); no Runtime,
// no device; the tests read no clock and never sleep.
//
// Fix round 1 (brief rd-1116/brief-463-s21c-fix1.md; the review of pull request #1788, its (h)).
// For the endings the recording is stopped with its package and takes two endings from frames (as
// "Marking after stop" takes them): `alpha` (a magenta square, its colour mark, frame f0007) and
// `bravo` (a cyan square, its colour mark and an OCR mark over a patch of the poison colour that
// no other screen shows, frame f0008). The scripted OCR model reads nothing on any frame and fails
// on a frame that shows the poison colour, as a model that cannot read fails.
// - `s21c_fix1_failures_that_arrive_out_of_step_order_are_listed_by_step`: step 4's stored frame
//   and the first ending's are damaged; `frames` meets step 4's first and lists the ending's first
//   (it has no step), and `record stop` refuses step 4's, the first it met.
// - `s21c_fix1_a_check_that_meets_an_error_says_check_error_and_lists_it_last`: the first ending's
//   mark expects white and the second ending's OCR read fails: the first ending's item, then the
//   error, last; `endings` is `complete: false` with `check_error`.
// - `s21c_fix1_a_final_package_that_stops_is_listed_with_whole_package`: the answer of a final
//   package that stopped (no recording reaches it once every check before it passed), built by
//   hand.
// - `s21c_fix1_the_endings_check_lists_every_ending_that_fails`: both endings' marks expect white.
// - `s21c_fix1_the_selects_check_lists_every_unread_slot_of_every_frame`: step 2 takes a sample
//   and a select of two slots; the scripted model reads no name: two items on each of its two
//   frames, each naming the target that reads its slot's name.
// - `s21c_fix1_a_pre_check_item_never_carries_frames_or_target`: a pre-check's refusal whose
//   details name `frames` and `target` gives an item with neither.

use crate::codes::{ReadyCheck, ReadyCheckStop, ReadyMissing};
use crate::recording::crosscheck::{CheckFailure, CheckRun, Locus};
use crate::recording::generate::Lack;
use crate::recording::model::LabRecording;
use crate::recording::ready::{check_items, check_runs, lack_items};
use crate::recording::store::{old_record_path, recording_path};
use crate::{
    AfterCapture, AttachFrameRequest, CapturedFrame, ClickEffect, ClickPlan, CommitClickRequest,
    LAB_RECORD_MARK_SCHEMA, LabOcr, MarkFamily, MarkRequest, MarkSpec, PlanClickRequest,
    PointPress, RecordFramePorts, RecordPoint, RecordPressRequest, RecordRect, RecordStartOptions,
    RecordStopOptions, RecordingLock, record_mark, record_press, record_ready, record_start,
    record_stop,
};
use actingcommand_contract::{InputFrameReference, LabError, LabResult};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_recognition_pack::{
    NnProviderRequest, NnProviderResult, OcrProviderRequest, OcrProviderResult, VisionProvider,
    VisionProviderError, VisionProviderErrorCode,
};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

// #463 stage two, train: the bench, its instance and the checks' order are the train's gate's too
// (`gate_463_s2_train`).
pub(crate) const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_0463_s21c";
const TASK_ID: &str = "neutral_task";
const STARTED_AT: u64 = 1_700_000_000_000;
const WIDTH: u32 = 160;
const HEIGHT: u32 = 90;
const GRAY: [u8; 3] = [60, 60, 60];
/// The colour of each screen's square.
const COLORS: [[u8; 3]; 4] = [[230, 20, 20], [20, 230, 20], [20, 20, 230], [230, 230, 20]];
/// Each screen's square, a colour mark's region.
const SQUARE: RecordRect = RecordRect {
    x: 0,
    y: 0,
    width: 16,
    height: 16,
};
/// The point each tap presses, on the gray.
const POINT: RecordPoint = RecordPoint { x: 100, y: 50 };
/// The self-checks of `record stop`, in the order they run (#463 S2-10, adapted: `cases` is the
/// last).
pub(crate) const EVERY_CHECK: [&str; 8] = [
    "frames",
    "admission",
    "first_decision",
    "cross_check",
    "endings",
    "selects",
    "final_package",
    "cases",
];
/// #463 S2-10 (adapted): the checks a failed check before the final package leaves unrun.
const AFTER_SELECTS: [&str; 2] = ["final_package", "cases"];

// Fix round 1: the endings' squares, the colour a broken mark expects, and the poison colour of
// the second ending's text patch, which only its screen shows and on which the scripted OCR model
// fails.
const MAGENTA: [u8; 3] = [230, 20, 230];
const CYAN: [u8; 3] = [20, 230, 230];
const WHITE: [u8; 3] = [255, 255, 255];
const POISON: [u8; 3] = [7, 7, 7];
const ALPHA_KEY: &str = "neutral_task_alpha";
// #463 stage two, train, fix round: the second ending's key and mark are the train's gate's too.
pub(crate) const BRAVO_KEY: &str = "neutral_task_bravo";
const ALPHA_MARK: &str = "ending/alpha";
pub(crate) const BRAVO_MARK: &str = "ending/bravo";
const TEXT_MARK: &str = "ending/text";
/// The made-up OCR model the marks and the select name; only the scripted model ever reads.
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

const fn rect(x: i32, y: i32, width: i32, height: i32) -> RecordRect {
    RecordRect {
        x,
        y,
        width,
        height,
    }
}

/// The second ending's text patch, its OCR mark's region.
const TEXT_AREA: RecordRect = rect(40, 40, 32, 16);
/// The select's two slots and the regions their names are read in; the press at `POINT` is in
/// slot 1.
const SLOTS: [RecordRect; 2] = [rect(40, 30, 36, 36), rect(90, 30, 36, 36)];
const NAMES: [RecordRect; 2] = [rect(42, 56, 32, 8), rect(92, 56, 32, 8)];

// Whether `area` covers the pixel at `(x, y)`.
fn covers(area: RecordRect, x: u32, y: u32) -> bool {
    let (column, row) = (i64::from(x), i64::from(y));
    column >= i64::from(area.x)
        && row >= i64::from(area.y)
        && column < i64::from(area.x) + i64::from(area.width)
        && row < i64::from(area.y) + i64::from(area.height)
}

// The PNG of screen `index`: its square of colour at the top left on gray.
fn screen(index: usize) -> Vec<u8> {
    painted(COLORS[index], None)
}

// The PNG of a gray screen with a square of `square` at the top left and, when given, an area of
// one colour.
fn painted(square: [u8; 3], patch: Option<(RecordRect, [u8; 3])>) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let patched = patch
                .filter(|(area, _)| covers(*area, x, y))
                .map(|(_, color)| color);
            pixels.extend(patched.unwrap_or(if x < 16 && y < 16 { square } else { GRAY }));
        }
    }
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

/// Fix round 1: the scripted OCR model. It reads nothing (no text, no confidence) on any frame, and
/// fails on a frame that shows the poison colour, as a model that cannot read fails.
#[derive(Debug)]
struct ScriptedOcr;

impl VisionProvider for ScriptedOcr {
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
        if request
            .frame
            .rgb8_pixels
            .chunks(3)
            .any(|pixel| *pixel == POISON)
        {
            return Err(VisionProviderError::new(
                VisionProviderErrorCode::Unavailable,
                "the scripted model cannot read this frame",
            ));
        }
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            text: String::new(),
            blocks: Vec::new(),
            confidence: None,
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

// A state directory with an active session of `INSTANCE` and its Lab recording, its game, server
// and locale known, without an OCR model (`ocr`; fix round 1: `read`, the scripted OCR model). The
// lock is held as one command holds it and dropped before the directory.
pub(crate) struct Bench {
    lock: RecordingLock,
    pub(crate) ocr: LabOcr,
    read: LabOcr,
    temp: TempDir,
}

impl Bench {
    // The recording of four screens: three taps, each on the screen before it, the screen after it
    // one capture later, then each step marked with a colour mark on its square.
    pub(crate) fn marked() -> Self {
        let temp = TempDir::new().expect("temporary directory");
        let state = temp.path().join("state");
        fs::create_dir_all(&state).expect("state directory");
        fs::write(
            old_record_path(&state, INSTANCE),
            serde_json::to_vec(&json!({
                "record_id": RECORD_ID,
                "task_id": TASK_ID,
                "instance": INSTANCE,
                "status": "active",
                "started_at_unix_ms": STARTED_AT
            }))
            .expect("session JSON"),
        )
        .expect("session file");
        let lock = RecordingLock::acquire(&state, INSTANCE, "gate_463_s21c", "Lab-CLI")
            .expect("recording lock");
        record_start(
            &lock,
            &RecordStartOptions {
                game: Some("neutral".to_owned()),
                server: Some("test".to_owned()),
                locale: Some("en-US".to_owned()),
                ..RecordStartOptions::default()
            },
        )
        .expect("record start");
        let bench = Self {
            lock,
            ocr: LabOcr::from_vision_root(&temp.path().join("no-vision")),
            read: LabOcr::with_provider(Arc::new(ScriptedOcr)),
            temp,
        };
        for index in 0..3 {
            bench.press(vec![screen(index), screen(index + 1)]);
        }
        for step in 1..=4 {
            bench.mark(step);
        }
        bench
    }

    pub(crate) fn state(&self) -> PathBuf {
        self.temp.path().join("state")
    }

    // `tap 100 50 --record`, the fake Runtime performing the input and its captures taking `shots`:
    // the screen before the press, then the screen after it one capture 800 ms later.
    fn press(&self, shots: Vec<Vec<u8>>) {
        let request = RecordPressRequest {
            click: PlanClickRequest::default(),
            point_press: Some(PointPress::Tap { point: POINT }),
            after: AfterCapture {
                still_ms: Some(0),
                ..AfterCapture::for_press(Some(800))
            },
        };
        let mut device = Screens {
            shots: shots.into(),
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

    // `record mark --step <step> --color square_<step>=0,0,16,16`.
    fn mark(&self, step: u32) {
        let request = MarkRequest {
            schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
            step: Some(step),
            add: vec![MarkSpec::region_only(
                format!("square_{step}"),
                MarkFamily::Color,
                SQUARE,
            )],
            ..MarkRequest::default()
        };
        record_mark(&self.lock, &request, false, &self.ocr).unwrap_or_else(|error| {
            panic!(
                "record mark: {}: {} {:?}",
                error.code, error.message, error.details
            )
        });
    }

    // The colour mark of step `step` with its member `member` set to `value`, written into the step
    // table in place.
    pub(crate) fn break_mark(&self, step: u32, member: &str, value: Value) {
        let path = recording_path(&self.state(), RECORD_ID);
        let mut recording: Value =
            serde_json::from_slice(&fs::read(&path).expect("step table")).expect("step table JSON");
        let entry = recording["steps"]
            .as_array_mut()
            .expect("steps")
            .iter_mut()
            .find(|entry| entry["index"] == step)
            .unwrap_or_else(|| panic!("no step {step}"));
        entry["marks"][0][member] = value;
        fs::write(
            &path,
            serde_json::to_vec(&recording).expect("step table JSON"),
        )
        .expect("step table");
    }

    // `record ready`, read as the command reads it: the instance's recording, no option.
    fn ready(&self) -> Value {
        self.ready_with(&self.ocr)
    }

    // Fix round 1: `record ready` with the Lab's OCR `ocr`.
    fn ready_with(&self, ocr: &LabOcr) -> Value {
        let outcome = record_ready(
            &self.state(),
            INSTANCE,
            None,
            &RecordStopOptions::default(),
            ocr,
        )
        .unwrap_or_else(|error| {
            panic!(
                "record ready: {}: {} {:?}",
                error.code, error.message, error.details
            )
        });
        serde_json::to_value(outcome).expect("the answer as JSON")
    }

    // The refusal of `record stop --dry-run`.
    fn stop_refusal(&self) -> LabError {
        self.stop_refusal_with(&self.ocr)
    }

    // Fix round 1: the refusal of `record stop --dry-run` with the Lab's OCR `ocr`.
    fn stop_refusal_with(&self, ocr: &LabOcr) -> LabError {
        record_stop(
            &self.lock,
            &RecordStopOptions {
                dry_run: true,
                ..RecordStopOptions::default()
            },
            ocr,
        )
        .map(|outcome| serde_json::to_value(&outcome.lab).expect("lab JSON"))
        .expect_err("record stop refuses")
    }

    // Fix round 1: the step table as the recording's file holds it.
    pub(crate) fn recording(&self) -> Value {
        let path = recording_path(&self.state(), RECORD_ID);
        serde_json::from_slice(&fs::read(path).expect("step table")).expect("step table JSON")
    }

    // Fix round 1: the step table written back in place.
    pub(crate) fn write_recording(&self, recording: &Value) {
        fs::write(
            recording_path(&self.state(), RECORD_ID),
            serde_json::to_vec(recording).expect("step table JSON"),
        )
        .expect("step table");
    }

    // Fix round 1: the step table read as the Lab reads it.
    fn stored(&self) -> LabRecording {
        serde_json::from_value(self.recording()).expect("the step table as a recording")
    }

    // Fix round 1: `bytes` written as the caller's frame file `name`; its path.
    fn file(&self, name: &str, bytes: &[u8]) -> String {
        let path = self.temp.path().join(format!("{name}.png"));
        fs::write(&path, bytes).expect("frame file");
        path.display().to_string()
    }

    // Fix round 1: `record mark` of the request `value`, without an OCR model.
    pub(crate) fn mark_request(&self, mut value: Value) {
        value["schema_version"] = json!(LAB_RECORD_MARK_SCHEMA);
        let request: MarkRequest = serde_json::from_value(value).expect("a record mark request");
        record_mark(&self.lock, &request, false, &self.ocr).unwrap_or_else(|error| {
            panic!(
                "record mark: {}: {} {:?}",
                error.code, error.message, error.details
            )
        });
    }

    // Fix round 1: the session file's status as the `record` command writes it.
    fn session_status(&self, status: &str) {
        let path = old_record_path(&self.state(), INSTANCE);
        let mut session: Value =
            serde_json::from_slice(&fs::read(&path).expect("session file")).expect("session");
        session["status"] = json!(status);
        fs::write(&path, serde_json::to_vec(&session).expect("session JSON"))
            .expect("session file");
    }

    // Fix round 1: the recording of `marked`, stopped with its package (the session file stopped
    // as the command leaves it), then the endings `alpha` (a magenta square and its colour mark,
    // frame f0007) and `bravo` (a cyan square and its colour mark, and an OCR mark over its patch
    // of the poison colour, frame f0008), taken from frames as "Marking after stop" takes them.
    // #463 stage two, train, fix round: the train's gate reads it too.
    pub(crate) fn with_endings() -> Self {
        let bench = Self::marked();
        let stopped = record_stop(&bench.lock, &RecordStopOptions::default(), &bench.ocr)
            .unwrap_or_else(|error| {
                panic!(
                    "record stop: {}: {} {:?}",
                    error.code, error.message, error.details
                )
            });
        assert_eq!(stopped.lab_status.as_deref(), Some("generated"));
        bench.session_status("stopped");
        let alpha = bench.file("ending_alpha", &painted(MAGENTA, None));
        bench.mark_request(json!({"ending": {
            "frame": alpha, "outcome_key": ALPHA_KEY, "page": "alpha",
            "add": [{"id": ALPHA_MARK, "family": "color", "region": SQUARE}]
        }}));
        let bravo = bench.file("ending_bravo", &painted(CYAN, Some((TEXT_AREA, POISON))));
        bench.mark_request(json!({"ending": {
            "frame": bravo, "outcome_key": BRAVO_KEY, "page": "bravo",
            "add": [
                {"id": BRAVO_MARK, "family": "color", "region": SQUARE},
                {"id": TEXT_MARK, "family": "ocr", "region": TEXT_AREA, "languages": ["en"],
                 "timeout_ms": 1000, "match_mode": "exact", "expected": ["DONE"],
                 "case_sensitive": false, "minimum_confidence": 0.8, "model_ref": "neutral-ocr",
                 "model_sha256": MODEL_SHA256}
            ]
        }}));
        bench
    }

    // Fix round 1: the colour mark `mark` of ending `ending` (0-based, in the order taken)
    // expecting white, written into the step table in place. #463 stage two, train, fix round: the
    // train's gate breaks one too.
    pub(crate) fn break_ending_mark(&self, ending: usize, mark: &str) {
        let mut recording = self.recording();
        let entry = recording["endings"][ending]["marks"]
            .as_array_mut()
            .expect("the ending's marks")
            .iter_mut()
            .find(|entry| entry["id"] == mark)
            .unwrap_or_else(|| panic!("no mark {mark}"));
        entry["expected"] = json!(WHITE);
        self.write_recording(&recording);
    }

    // Fix round 1: the stored frame `frame_id`, a step's or an ending's, overwritten with other
    // bytes, as a damaged file holds them; its entry in the step table.
    fn damage(&self, frame_id: &str) -> Value {
        let recording = self.recording();
        let frame = recording["steps"]
            .as_array()
            .into_iter()
            .chain(recording["endings"].as_array())
            .flatten()
            .flat_map(|entry| entry["frames"].as_array().into_iter().flatten())
            .find(|frame| frame["frame_id"] == frame_id)
            .cloned()
            .unwrap_or_else(|| panic!("no stored frame {frame_id}"));
        fs::write(
            frame["path"].as_str().expect("the frame's path"),
            b"not the recorded frame",
        )
        .expect("the damaged frame");
        frame
    }
}

// `checks` of the self-checks `ran` (each complete), in that order.
pub(crate) fn complete(ran: &[&str]) -> Value {
    json!(
        ran.iter()
            .map(|check| json!({"check": check, "complete": true}))
            .collect::<Vec<_>>()
    )
}

// The work order's first test: the cross-check fails on three frames.
#[test]
fn s21c_three_frames_that_fail_the_cross_check_are_three_items_and_stop_answers_the_first() {
    let bench = Bench::marked();
    for step in 2..=4 {
        bench.break_mark(step, "expected", json!([255, 255, 255]));
    }

    let ready = bench.ready();
    assert_eq!(ready["status"], "draft", "{ready}");
    assert_eq!(
        ready["missing"],
        json!([
            {"order": 1, "step": 2, "artifact_step": 2, "frame_id": "f0003",
             "what": "cross_check", "code": "record_step_self_mismatch",
             "details": {"page": "step_02", "frames": ["f0003"]}},
            {"order": 2, "step": 3, "artifact_step": 3, "frame_id": "f0005",
             "what": "cross_check", "code": "record_step_self_mismatch",
             "details": {"page": "step_03", "frames": ["f0005"]}},
            {"order": 3, "step": 4, "artifact_step": 4, "frame_id": null,
             "what": "cross_check", "code": "record_step_self_mismatch",
             "details": {"page": "step_04", "frames": ["f0006"]}}
        ]),
        "{ready}"
    );
    assert_eq!(ready["missing_total"], 3, "{ready}");
    // Every check before the final package ran to its end; the final package is built of what they
    // gave, so it did not run.
    assert_eq!(ready["checks"], complete(&EVERY_CHECK[..6]), "{ready}");
    assert_eq!(ready["checks_after"], json!(AFTER_SELECTS), "{ready}");

    // `record stop` answers the first failure with the code it answers today.
    let error = bench.stop_refusal();
    assert_eq!(error.code, "record_step_self_mismatch", "{}", error.message);
    let details = error.details.clone().expect("details");
    assert_eq!(details["record_index"], 2, "{details}");
    assert_eq!(details["frame_id"], "f0003", "{details}");
    assert_eq!(details["page"], "step_02", "{details}");
}

// The work order's second test: two self-checks fail, and both are listed in the order they run.
#[test]
fn s21c_two_self_checks_that_fail_are_listed_in_the_order_the_checks_run() {
    let bench = Bench::marked();
    // Step 1's page no longer passes on its frame: the first decision gives no click there, and the
    // cross-check finds the page does not match its own frame.
    bench.break_mark(1, "expected", json!([255, 255, 255]));

    let ready = bench.ready();
    assert_eq!(ready["status"], "draft", "{ready}");
    assert_eq!(
        ready["missing"],
        json!([
            {"order": 1, "step": 1, "artifact_step": 1, "frame_id": "f0001",
             "what": "first_decision", "code": "record_artifact_admission_failed",
             "details": {"frames": ["f0001"]}},
            {"order": 2, "step": 1, "artifact_step": 1, "frame_id": "f0001",
             "what": "cross_check", "code": "record_step_self_mismatch",
             "details": {"page": "step_01", "frames": ["f0001"]}}
        ]),
        "{ready}"
    );
    assert_eq!(ready["missing_total"], 2, "{ready}");
    assert_eq!(ready["checks"], complete(&EVERY_CHECK[..6]), "{ready}");
    assert_eq!(ready["checks_after"], json!(AFTER_SELECTS), "{ready}");

    // `record stop` answers the first, the first decision's refusal.
    let error = bench.stop_refusal();
    assert_eq!(
        error.code, "record_artifact_admission_failed",
        "{}",
        error.message
    );
    let details = error.details.clone().expect("details");
    assert_eq!(details["stage"], "first_decision", "{details}");
}

// The work order's third test: a check that must stop says so with its word and lists one.
#[test]
fn s21c_a_check_that_must_stop_says_so_and_lists_one() {
    let bench = Bench::marked();
    // A colour distance below 0, which the kernel's admission refuses for the package as a whole.
    bench.break_mark(2, "max_distance", json!(-1.0));

    let ready = bench.ready();
    assert_eq!(ready["status"], "draft", "{ready}");
    assert_eq!(
        ready["missing"],
        json!([
            {"order": 1, "step": null, "artifact_step": null, "frame_id": null,
             "what": "admission", "code": "record_artifact_admission_failed"}
        ]),
        "{ready}"
    );
    assert_eq!(ready["missing_total"], 1, "{ready}");
    assert_eq!(
        ready["checks"],
        json!([
            {"check": "frames", "complete": true},
            {"check": "admission", "complete": false, "reason": "whole_package"}
        ]),
        "{ready}"
    );
    assert_eq!(ready["checks_after"], json!(EVERY_CHECK[2..]), "{ready}");

    let error = bench.stop_refusal();
    assert_eq!(
        error.code, "record_artifact_admission_failed",
        "{}",
        error.message
    );
    let details = error.details.clone().expect("details");
    assert_eq!(details["stage"], "admission", "{details}");
}

// The work order's fourth test: a recording that passes.
#[test]
fn s21c_a_recording_that_passes_has_every_check_complete_and_is_ready() {
    let bench = Bench::marked();

    let ready = bench.ready();
    assert_eq!(ready["status"], "ready", "{ready}");
    assert_eq!(ready["missing"], json!([]), "{ready}");
    assert_eq!(ready["missing_total"], 0, "{ready}");
    assert_eq!(ready["checks"], complete(&EVERY_CHECK), "{ready}");
    assert_eq!(ready["checks_after"], json!([]), "{ready}");
}

// Fix round 1, the review's (h) and (k): failures that do not arrive in step order. `frames` reads
// the steps' frames before the endings' frames, so it meets step 4's damaged frame first and the
// first ending's after it; the list sorts by step, items without a step first, so the ending's item
// comes first. `record stop` refuses the one it met first, step 4's.
#[test]
fn s21c_fix1_failures_that_arrive_out_of_step_order_are_listed_by_step() {
    let bench = Bench::with_endings();
    let step_four = bench.damage("f0006");
    bench.damage("f0007");

    let ready = bench.ready();
    assert_eq!(ready["status"], "draft", "{ready}");
    assert_eq!(
        ready["missing"],
        json!([
            {"order": 1, "step": null, "artifact_step": null, "frame_id": null,
             "what": "frames", "code": "record_frame_hash_mismatch",
             "details": {"outcome_key": ALPHA_KEY, "page": "ending_01_alpha",
                         "frames": ["f0007"]}},
            {"order": 2, "step": 4, "artifact_step": 4, "frame_id": null,
             "what": "frames", "code": "record_frame_hash_mismatch",
             "details": {"frames": ["f0006"]}}
        ]),
        "{ready}"
    );
    assert_eq!(ready["missing_total"], 2, "{ready}");
    // Every later check reads every frame, so none ran.
    assert_eq!(ready["checks"], complete(&EVERY_CHECK[..1]), "{ready}");
    assert_eq!(ready["checks_after"], json!(EVERY_CHECK[1..]), "{ready}");

    let error = bench.stop_refusal();
    assert_eq!(
        error.code, "record_frame_hash_mismatch",
        "{}",
        error.message
    );
    let details = error.details.clone().expect("details");
    assert_eq!(details["expected_sha256"], step_four["sha256"], "{details}");
}

// Fix round 1, the review's (h) and its point 1: a check that met an error that is none of its
// verdicts stops there and says `check_error`; the failure it stopped at is its last item, after
// the failures it met before it. With the scripted OCR model the first ending's page (its mark
// expecting white) does not match its own frame, and the second ending's OCR mark cannot be read on
// its own frame.
#[test]
fn s21c_fix1_a_check_that_meets_an_error_says_check_error_and_lists_it_last() {
    let bench = Bench::with_endings();
    bench.break_ending_mark(0, ALPHA_MARK);

    let ready = bench.ready_with(&bench.read);
    assert_eq!(ready["status"], "draft", "{ready}");
    assert_eq!(
        ready["missing"],
        json!([
            {"order": 1, "step": null, "artifact_step": null, "frame_id": null,
             "what": "endings", "code": "record_step_self_mismatch",
             "details": {"outcome_key": ALPHA_KEY, "page": "ending_01_alpha",
                         "frames": ["f0007"]}},
            {"order": 2, "step": null, "artifact_step": null, "frame_id": null,
             "what": "endings", "code": "record_artifact_admission_failed"}
        ]),
        "{ready}"
    );
    assert_eq!(
        ready["checks"],
        json!([
            {"check": "frames", "complete": true},
            {"check": "admission", "complete": true},
            {"check": "first_decision", "complete": true},
            {"check": "cross_check", "complete": true},
            {"check": "endings", "complete": false, "reason": "check_error"},
            {"check": "selects", "complete": true}
        ]),
        "{ready}"
    );
    assert_eq!(ready["checks_after"], json!(AFTER_SELECTS), "{ready}");

    // `record stop` answers the first failure the endings met.
    let error = bench.stop_refusal_with(&bench.read);
    assert_eq!(error.code, "record_step_self_mismatch", "{}", error.message);
    let details = error.details.clone().expect("details");
    assert_eq!(details["outcome_key"], ALPHA_KEY, "{details}");
    assert_eq!(details["frame_id"], "f0007", "{details}");
}

// Fix round 1, the review's (h): the final package's `whole_package`. The final package is built
// of the plan, the frames and the crops every check before it passed, so no recording reaches its
// refusal; its answer is built here from the checks' runs as `record ready` would hold them: every
// check before it complete, and the final package stopped at its refusal, one item of the package
// as a whole.
#[test]
fn s21c_fix1_a_final_package_that_stops_is_listed_with_whole_package() {
    let recording = Bench::marked().stored();
    let mut runs = ReadyCheck::ALL[..6]
        .iter()
        .map(|check| CheckRun {
            check: *check,
            failures: Vec::new(),
            stopped: None,
        })
        .collect::<Vec<_>>();
    runs.push(CheckRun {
        check: ReadyCheck::FinalPackage,
        failures: vec![CheckFailure {
            locus: Locus::default(),
            error: LabError::safety_blocked(
                "record_artifact_admission_failed",
                "the final package is not admitted",
                &["session_record"],
            )
            .with_details(json!({"stage": "admission"})),
        }],
        stopped: Some(ReadyCheckStop::WholePackage),
    });

    let items = serde_json::to_value(check_items(&recording, &runs)).expect("items JSON");
    assert_eq!(
        items,
        json!([
            {"order": 1, "step": null, "artifact_step": null, "frame_id": null,
             "what": "final_package", "code": "record_artifact_admission_failed"}
        ])
    );
    let checks = serde_json::to_value(check_runs(&runs)).expect("checks JSON");
    assert_eq!(
        checks,
        json!([
            {"check": "frames", "complete": true},
            {"check": "admission", "complete": true},
            {"check": "first_decision", "complete": true},
            {"check": "cross_check", "complete": true},
            {"check": "endings", "complete": true},
            {"check": "selects", "complete": true},
            {"check": "final_package", "complete": false, "reason": "whole_package"}
        ])
    );
}

// Fix round 1, the review's (h): the endings check collects. Both endings' marks expect white: two
// items, in the order the recording took the endings, the check complete; `record stop` answers
// the first.
#[test]
fn s21c_fix1_the_endings_check_lists_every_ending_that_fails() {
    let bench = Bench::with_endings();
    bench.break_ending_mark(0, ALPHA_MARK);
    bench.break_ending_mark(1, BRAVO_MARK);

    let ready = bench.ready();
    assert_eq!(ready["status"], "draft", "{ready}");
    assert_eq!(
        ready["missing"],
        json!([
            {"order": 1, "step": null, "artifact_step": null, "frame_id": null,
             "what": "endings", "code": "record_step_self_mismatch",
             "details": {"outcome_key": ALPHA_KEY, "page": "ending_01_alpha",
                         "frames": ["f0007"]}},
            {"order": 2, "step": null, "artifact_step": null, "frame_id": null,
             "what": "endings", "code": "record_step_self_mismatch",
             "details": {"outcome_key": BRAVO_KEY, "page": "ending_02_bravo",
                         "frames": ["f0008"]}}
        ]),
        "{ready}"
    );
    assert_eq!(ready["checks"], complete(&EVERY_CHECK[..6]), "{ready}");
    assert_eq!(ready["checks_after"], json!(AFTER_SELECTS), "{ready}");

    let error = bench.stop_refusal();
    assert_eq!(error.code, "record_step_self_mismatch", "{}", error.message);
    let details = error.details.clone().expect("details");
    assert_eq!(details["outcome_key"], ALPHA_KEY, "{details}");
}

// Fix round 1, the review's (h) and its point 3: the selects check collects, and lists every slot
// of a frame whose name reads no known name. Step 2 takes a sample (f0007, its screen with one
// pixel changed) and a select of two slots, marked without an OCR model; the scripted model reads
// no name: two items on f0003 and two on f0007, each naming the target that reads its slot's name.
// `record stop` answers the first, slot 0 on f0003.
#[test]
fn s21c_fix1_the_selects_check_lists_every_unread_slot_of_every_frame() {
    let bench = Bench::marked();
    let sample = bench.file(
        "sample",
        &painted(COLORS[1], Some((rect(150, 80, 1, 1), [61, 61, 61]))),
    );
    bench.mark_request(json!({"step": 2, "samples": [sample]}));
    bench.mark_request(json!({"step": 2, "select": {
        "slots": [{"rect": SLOTS[0], "name": NAMES[0]}, {"rect": SLOTS[1], "name": NAMES[1]}],
        "read": {"languages": ["en"], "timeout_ms": 1000, "minimum_confidence_milli": 800,
                 "model_ref": "neutral-ocr", "model_sha256": MODEL_SHA256},
        "names": [{"id": "item_a", "aliases": ["ALPHA"]}, {"id": "item_b", "aliases": ["BRAVO"]}],
        "match": {"max_distance": 1, "minimum_margin": 1},
        "wanted": {"name": "list_item", "default": "item_b"}
    }}));

    let ready = bench.ready_with(&bench.read);
    assert_eq!(ready["status"], "draft", "{ready}");
    let item = |order: usize, frame: &str, slot: &str| {
        json!({"order": order, "step": 2, "artifact_step": 2, "frame_id": "f0003",
               "what": "selects", "code": "record_step_self_mismatch",
               "details": {"page": "step_02", "frames": [frame],
                           "target": format!("select/step_02/slot_{slot}")}})
    };
    assert_eq!(
        ready["missing"],
        json!([
            item(1, "f0003", "00"),
            item(2, "f0003", "01"),
            item(3, "f0007", "00"),
            item(4, "f0007", "01")
        ]),
        "{ready}"
    );
    assert_eq!(ready["checks"], complete(&EVERY_CHECK[..6]), "{ready}");
    assert_eq!(ready["checks_after"], json!(AFTER_SELECTS), "{ready}");

    let error = bench.stop_refusal_with(&bench.read);
    assert_eq!(error.code, "record_step_self_mismatch", "{}", error.message);
    let details = error.details.clone().expect("details");
    assert_eq!(details["reason"], "select_name_unread", "{details}");
    assert_eq!(details["frame_id"], "f0003", "{details}");
    assert_eq!(details["slot"], 0, "{details}");
}

// Fix round 1, the review's (h): a pre-check's item never carries the members a self-check's item
// has of its own (`frames`, `target`), whatever its refusal's details name; what a pre-check's
// refusal says of its item (here `page`) it still carries.
#[test]
fn s21c_fix1_a_pre_check_item_never_carries_frames_or_target() {
    let recording = Bench::marked().stored();
    let lack = Lack {
        what: ReadyMissing::PageRecognition,
        step: Some((2, 2)),
        error: LabError::safety_blocked(
            "record_step_without_recognition",
            "step 2 has no mark",
            &["session_record"],
        )
        .with_details(json!({
            "step": 2,
            "record_index": 2,
            "page": "step_02",
            "frames": ["f0003"],
            "target": "square_2"
        })),
        outcome_key: None,
    };

    let items = lack_items(&recording, &[lack]).unwrap_or_else(|error| {
        panic!(
            "the items: {}: {} {:?}",
            error.code, error.message, error.details
        )
    });

    assert_eq!(
        serde_json::to_value(items).expect("items JSON"),
        json!([
            {"order": 1, "step": 2, "artifact_step": 2, "frame_id": "f0003",
             "what": "page_recognition", "code": "record_step_without_recognition",
             "hint": "record mark --step 2 --template <id>=x,y,w,h",
             "details": {"page": "step_02"}}
        ])
    );
}
