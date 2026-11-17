// SPDX-License-Identifier: AGPL-3.0-only

// #463 S2-10 (contracts/lab-recording.md, "Regression cases written with the package";
// contracts/resource-regression-case.md; register §1.441): `record stop` writes the package's
// regression cases beside it, `lab/out/<package_id>.cases/<case id>/`, each run first on the package
// by the one runner of the case contract. A recording is made as stage one makes it (taps through
// `record_press` with a fake Runtime, its captures taking the scripted screens and the caller's clock
// moving only by its waits; three taps give four steps whose live frames are f0001, f0003, f0005 and
// f0006), then marked with colour marks. Frames are synthetic 160x90 screens of gray with squares of
// one colour; no Runtime, no device; no OCR model but in tests 6 and 11, whose scripted model reads
// its text only on a frame that shows the marker colour anywhere, a model that looks outside the
// region the package declares, and in test 16 (fix round 3); the tests read no clock and never sleep. The coordinator's ruling
// after the first round (R2's ruling 4 as precedent): a page recognised beside another page of the
// package is no miss, the case is written without `expect.observe` and named
// `record_case_page_ambiguous`; only a real miss refuses.
// - `s210_three_steps_and_an_ending_write_five_cases_that_pass_read_from_disk` (1): the stop writes
//   four cases, the ending taken after it a fifth on the regeneration; each in the contract's form,
//   each passing when read from disk and run by `run_regression_case` on the written package.
// - `s210_the_package_is_byte_for_byte_what_it_is_without_the_cases` (2; fix round 3: compared with
//   bytes this slice's path does not make): the file `record stop` wrote is the generator's own
//   rendering and encoding of the recording's plan, the three check results its task's provenance
//   records read back, and that provenance holds exactly the members it held before this slice.
// - `s210_a_scene_holds_no_pixel_of_the_frame_outside_the_regions_the_targets_read` (3): every pixel
//   no target reads is painted a marker colour; no scene holds it, every pixel outside the
//   rectangles is the canvas colour, and the runner's scene rule warns nothing.
// - `s210_a_page_told_apart_by_another_pages_forbidden_target_passes` (4): step 2's page also passes
//   on step 1's screen but forbids step 3's blue mark, which only steps 1 and 3 show; step 1's case
//   holds that mark's region and passes.
// - `s210_the_dry_run_writes_nothing_and_lists_five_and_a_regeneration_replaces_the_cases` (5; fix
//   round 3: `would_write` names the cases' directory and each of the five cases).
// - `s210_cases_whose_page_is_not_recognised_are_listed_and_refuse_record_stop` (6, adjusted to the
//   ruling): steps 2 and 3 are recognised by text the scripted model reads only on a frame showing
//   the marker, which their screens show where no target reads: on the scenes their pages are not
//   recognised: two misses, both listed by `record ready`; `record stop` refuses the first and writes
//   no case.
// - `s210_a_page_that_needs_text_recognition_is_written_and_named_not_proven` (7; fix round 3: every
//   case is not proven, and `record_cases_ocr_unavailable` says once that no page was judged).
// - `s210_a_page_the_bare_canvas_answers_is_written_with_the_warning` (8): step 2's square is black.
// - `s210_a_dropped_middle_step_has_no_case_and_a_source_names_no_path` (9).
// - `s210_a_later_page_matching_an_earlier_screen_writes_the_case_without_observe` (10): step 2's
//   mark holds on step 1's screen too: step 1's case is written without `observe`, passes, and
//   `record stop` and `record ready` name it with the other page.
// - `s210_an_expected_page_not_recognised_refuses_record_stop` (11): one such miss, its checks.
// Fix round 3 (register §1.441, "Fix round 3"; the independent review of #1797):
// - `s210_a_target_that_reads_the_whole_frame_makes_each_scene_the_whole_frame_and_names_it` (12):
//   the final step's colour mark reads the whole frame: every scene is its whole frame, and each case
//   is named `record_case_whole_frame` with that target.
// - `s210_an_ambiguous_case_pins_the_targets_its_page_forbids_failing` (13): step 1's page forbids
//   step 3's mark; its case, written without `observe`, expects that mark not to pass.
// - `s210_a_press_frame_no_longer_live_gives_the_case_of_the_steps_primary_frame` (14): step 2's
//   press names its frame set aside; its case is made of its primary frame.
// - `s210_an_ambiguous_page_with_no_target_to_pin_is_not_written_with_its_own_word` (15): the slice's
//   decision on its own (no recording reaches it: a page set requires a target on every page): no
//   case, the warning `record_case_ambiguous_not_written`; with targets, the expectation.
// - `s210_an_ambiguous_case_that_fails_its_run_again_refuses_record_stop` (16): a scripted OCR model
//   reads a case's scene only once; step 1's case, ambiguous on its first run, fails its run again
//   without the page judgement: `record_case_failed` with that run's one failed check.

use crate::recording::cases::CaseSet;
use crate::recording::container::{container_kind, encode};
use crate::recording::generate::{
    DEFAULT_APPLICATION_ARRIVAL_TIMEOUT_MS, DEFAULT_ARRIVAL_TIMEOUT_MS, StopSettings,
    plan as generate_plan, render,
};
use crate::recording::model::LabRecording;
use crate::recording::store::{hex_sha256, lab_dir, old_record_path, recording_path};
use crate::{
    AfterCapture, AttachFrameRequest, CapturedFrame, ClickEffect, ClickPlan, CommitClickRequest,
    LAB_RECORD_MARK_SCHEMA, LabCode, LabOcr, MarkOutcome, MarkRequest, PlanClickRequest,
    PointPress, REGRESSION_CASE_SCHEMA, RecordFramePorts, RecordPoint, RecordPressRequest,
    RecordRect, RecordStartOptions, RecordStopOptions, RecordingLock, RegressionVerdict,
    record_mark, record_press, record_ready, record_start, record_stop, run_regression_case,
};
use actingcommand_contract::{InputFrameReference, LabError, LabResult};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_pack_containment::{
    ContainmentLimits, ContentContainer, expand_content_container,
};
use actingcommand_recognition::Scene;
use actingcommand_recognition_pack::{
    NnProviderRequest, NnProviderResult, OcrProviderRequest, OcrProviderResult,
    OcrProviderTextBlock, PackRect, VisionProvider, VisionProviderError, VisionProviderErrorCode,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_0463_s210";
const TASK_ID: &str = "neutral_task";
/// The package's id, `<game>.<server>.<task_id>`.
const PACKAGE_ID: &str = "neutral.test.neutral_task";
const STARTED_AT: u64 = 1_700_000_000_000;
const WIDTH: u32 = 160;
const HEIGHT: u32 = 90;
const GRAY: [u8; 3] = [60, 60, 60];
const RED: [u8; 3] = [230, 20, 20];
const GREEN: [u8; 3] = [20, 230, 20];
const BLUE: [u8; 3] = [20, 20, 230];
const YELLOW: [u8; 3] = [230, 230, 20];
const MAGENTA: [u8; 3] = [230, 20, 230];
const CYAN: [u8; 3] = [20, 230, 230];
const BLACK: [u8; 3] = [0, 0, 0];
/// The colour test 3 paints every pixel of a screen that no target reads, and tests 6 and 11 show
/// where no target reads, the colour `MarkerOcr` reads by.
const MARKER: [u8; 3] = [201, 13, 177];
/// The ending taken after the stop, and its mark.
const ALPHA_KEY: &str = "neutral_task_alpha";
const ALPHA_MARK: &str = "ending/alpha";
/// The made-up OCR model of test 7's OCR mark; no test loads it.
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
/// The ids of the five cases of test 1 and test 5, in the order the answer gives them.
const FIVE: [&str; 5] = [
    "step_01",
    "step_02",
    "step_03",
    "final",
    "ending_neutral_task_alpha",
];

const fn rect(x: i32, y: i32, width: i32, height: i32) -> RecordRect {
    RecordRect {
        x,
        y,
        width,
        height,
    }
}

/// Each screen's page square, a colour mark's region.
const SQUARE: RecordRect = rect(0, 0, 16, 16);
/// Test 4's second and third squares.
const SECOND: RecordRect = rect(32, 0, 16, 16);
const THIRD: RecordRect = rect(64, 0, 16, 16);
/// A square no target reads, where tests 6 and 11 show the marker colour.
const UNREAD: RecordRect = rect(100, 40, 16, 16);
/// Test 12's mark: the whole frame.
const WHOLE: RecordRect = rect(0, 0, 160, 90);
/// The OCR marks' region (tests 6, 7 and 11).
const TEXT_AREA: RecordRect = rect(40, 40, 32, 16);
/// The point each tap presses, on no square.
const POINT: RecordPoint = RecordPoint { x: 130, y: 70 };

// Whether `area` covers the pixel at `(x, y)`.
fn covers(area: RecordRect, x: u32, y: u32) -> bool {
    let (column, row) = (i64::from(x), i64::from(y));
    column >= i64::from(area.x)
        && row >= i64::from(area.y)
        && column < i64::from(area.x) + i64::from(area.width)
        && row < i64::from(area.y) + i64::from(area.height)
}

// The PNG of a screen of `background` with each of `squares` in its colour.
fn screen_on(background: [u8; 3], squares: &[(RecordRect, [u8; 3])]) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let color = squares
                .iter()
                .find(|(area, _)| covers(*area, x, y))
                .map_or(background, |(_, color)| *color);
            pixels.extend(color);
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

// A gray screen with each of `squares` in its colour.
fn screen(squares: &[(RecordRect, [u8; 3])]) -> Vec<u8> {
    screen_on(GRAY, squares)
}

// Four screens, each with its page square in its own colour.
fn four_screens() -> [Vec<u8>; 4] {
    [RED, GREEN, BLUE, YELLOW].map(|color| screen(&[(SQUARE, color)]))
}

// The colour mark `id` over `area`.
fn color(id: &str, area: RecordRect) -> Value {
    json!({"id": id, "family": "color", "region": area})
}

// The OCR mark `id` over `area`, of a made-up model: without an OCR model it is never evaluated.
fn ocr(id: &str, area: RecordRect) -> Value {
    json!({"id": id, "family": "ocr", "region": area, "languages": ["en"], "timeout_ms": 1000,
           "match_mode": "exact", "expected": ["TEXT"], "case_sensitive": false,
           "minimum_confidence": 0.8, "model_ref": "neutral-ocr", "model_sha256": MODEL_SHA256})
}

fn request(mut value: Value) -> MarkRequest {
    value["schema_version"] = json!(LAB_RECORD_MARK_SCHEMA);
    serde_json::from_value(value).expect("a record mark request")
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

/// Tests 6 and 11's scripted OCR model: it reads `TEXT` in the requested region, at confidence
/// 0.9, only on a frame that shows the marker colour anywhere, and nothing on any other frame: a
/// model that looks outside the region the package declares for its target.
#[derive(Debug)]
struct MarkerOcr;

impl VisionProvider for MarkerOcr {
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
        let shows_marker = request
            .frame
            .rgb8_pixels
            .chunks(3)
            .any(|pixel| *pixel == MARKER);
        let blocks = if shows_marker {
            vec![OcrProviderTextBlock {
                text: "TEXT".to_owned(),
                rect: request.region,
                confidence: Some(0.9),
            }]
        } else {
            Vec::new()
        };
        let text = blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            text,
            confidence: shows_marker.then_some(0.9),
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

/// What a scripted model reads in `region`: `TEXT` at confidence 0.9 when `reads`, nothing else.
fn text_read(reads: bool, region: PackRect) -> OcrProviderResult {
    let blocks = if reads {
        vec![OcrProviderTextBlock {
            text: "TEXT".to_owned(),
            rect: region,
            confidence: Some(0.9),
        }]
    } else {
        Vec::new()
    };
    let text = blocks
        .iter()
        .map(|block| block.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    OcrProviderResult {
        ppocr_diagnostics: Vec::new(),
        text,
        confidence: reads.then_some(0.9),
        blocks,
        detection: None,
    }
}

/// Test 16's scripted OCR model (fix round 3): it reads `TEXT` in the requested region on every
/// recorded screen, and on a case's scene (a frame showing the canvas colour anywhere, which no
/// recorded screen shows) only the first time it reads that scene: a model whose second reading of
/// one scene is not its first.
#[derive(Debug, Default)]
struct OnceOcr {
    scenes: Mutex<Vec<Vec<u8>>>,
}

impl VisionProvider for OnceOcr {
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
        let pixels = request.frame.rgb8_pixels;
        let scene = pixels.chunks(3).any(|pixel| *pixel == BLACK);
        let reads = !scene || {
            let mut read = self.scenes.lock().expect("the scenes read");
            let first = !read.iter().any(|seen| seen.as_slice() == pixels);
            if first {
                read.push(pixels.to_vec());
            }
            first
        };
        Ok(text_read(reads, request.region))
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

// A state directory with an active session of `INSTANCE` and its Lab recording, its game, server
// and locale known, without an OCR model (or, `Bench::reading`, with the scripted one). The lock is
// held as one command holds it and dropped before the directory.
struct Bench {
    lock: RecordingLock,
    ocr: LabOcr,
    temp: TempDir,
}

impl Bench {
    fn new() -> Self {
        Self::with_ocr(None)
    }

    // Tests 6 and 11: a bench whose Lab reads text with `MarkerOcr`.
    fn reading() -> Self {
        Self::with_ocr(Some(LabOcr::with_provider(Arc::new(MarkerOcr))))
    }

    fn with_ocr(ocr: Option<LabOcr>) -> Self {
        let temp = TempDir::new().expect("temporary directory");
        let state = temp.path().join("state");
        fs::create_dir_all(&state).expect("state directory");
        let lock = RecordingLock::acquire(&state, INSTANCE, "gate_463_s210", "Lab-CLI")
            .expect("recording lock");
        let bench = Self {
            lock,
            ocr: ocr.unwrap_or_else(|| LabOcr::from_vision_root(&temp.path().join("no-vision"))),
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

    // The recording of `screens`: one tap on each screen but the last, the screen after it one
    // capture later.
    fn recorded(screens: &[Vec<u8>]) -> Self {
        Self::new().pressing(screens)
    }

    // This bench with one tap on each of `screens` but the last.
    fn pressing(self, screens: &[Vec<u8>]) -> Self {
        for pair in screens.windows(2) {
            self.press(vec![pair[0].clone(), pair[1].clone()]);
        }
        self
    }

    fn state(&self) -> PathBuf {
        self.temp.path().join("state")
    }

    // The session header, as the `record` command writes it with `status`.
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

    // `tap 130 70 --record`, the fake Runtime performing the input and its captures taking `shots`:
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

    // `record mark` of `value`, which must stand.
    fn marked(&self, value: Value) -> MarkOutcome {
        record_mark(&self.lock, &request(value), false, &self.ocr).unwrap_or_else(|error| {
            panic!(
                "record mark: {}: {} {:?}",
                error.code, error.message, error.details
            )
        })
    }

    // Steps 1 to `steps`, each with the colour mark `square_<step>` on its square.
    fn squares(&self, steps: u32) {
        for step in 1..=steps {
            self.marked(json!({
                "step": step,
                "add": [color(&format!("square_{step}"), SQUARE)]
            }));
        }
    }

    // The ending `ALPHA_KEY` taken from a magenta screen after the stop, the session stopped first
    // as the command leaves it.
    fn take_alpha_ending(&self) {
        self.header("stopped");
        let frame = self.temp.path().join("alpha.png");
        fs::write(&frame, screen(&[(SQUARE, MAGENTA)])).expect("the ending's frame");
        self.marked(json!({"ending": {
            "frame": frame.display().to_string(),
            "outcome_key": ALPHA_KEY,
            "add": [color(ALPHA_MARK, SQUARE)]
        }}));
    }

    // `record stop` (with `dry_run`): the `lab` output with its status.
    fn stop(&self, dry_run: bool) -> Result<Value, LabError> {
        let options = RecordStopOptions {
            dry_run,
            ..RecordStopOptions::default()
        };
        record_stop(&self.lock, &options, &self.ocr).map(|outcome| {
            let mut lab = serde_json::to_value(&outcome.lab).expect("lab JSON");
            lab["lab_status"] = json!(outcome.lab_status);
            lab
        })
    }

    // `record stop` (with `dry_run`), which must stand.
    fn stopped(&self, dry_run: bool) -> Value {
        self.stop(dry_run).unwrap_or_else(|error| {
            panic!(
                "record stop: {}: {} {:?}",
                error.code, error.message, error.details
            )
        })
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
        .unwrap_or_else(|error| panic!("record ready: {}: {}", error.code, error.message));
        serde_json::to_value(&outcome).expect("record ready JSON")
    }

    // The package's output directory, `lab/out`.
    fn out(&self) -> PathBuf {
        lab_dir(&self.state(), RECORD_ID).join("out")
    }

    // The package `record stop` wrote (`lab`), expanded into a checkout's `packs/<package_id>/`.
    fn package(&self, lab: &Value) -> PathBuf {
        let bytes = fs::read(lab["path"].as_str().expect("package path")).expect("package");
        let container = if lab["container"] == "zip" {
            ContentContainer::Zip
        } else {
            ContentContainer::Json
        };
        let entries = expand_content_container(&bytes, container, ContainmentLimits::default())
            .expect("package container");
        let root = self.temp.path().join("repo").join("packs").join(PACKAGE_ID);
        for (name, bytes) in entries {
            let file = name
                .split('/')
                .fold(root.clone(), |path, part| path.join(part));
            fs::create_dir_all(file.parent().expect("a parent")).expect("package directories");
            fs::write(file, bytes).expect("package file");
        }
        root
    }
}

// The cases' directory `record stop` answered (`lab.cases.directory`).
fn cases_dir(lab: &Value) -> PathBuf {
    let directory = PathBuf::from(
        lab["cases"]["directory"]
            .as_str()
            .unwrap_or_else(|| panic!("no cases directory in {lab}")),
    );
    assert!(
        directory.ends_with(format!("out/{PACKAGE_ID}.cases")),
        "{}",
        directory.display()
    );
    directory
}

// Each case of the cases' directory `dir` by its directory's name: its `case.json` and its scene,
// the only two files of its directory.
fn written_cases(dir: &Path) -> BTreeMap<String, (Vec<u8>, Vec<u8>)> {
    let mut cases = BTreeMap::new();
    for entry in fs::read_dir(dir).expect("the cases' directory") {
        let path = entry.expect("an entry").path();
        let names = fs::read_dir(&path)
            .expect("a case's directory")
            .map(|file| {
                file.expect("a file")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(
            names,
            BTreeSet::from(["case.json".to_owned(), "scene.png".to_owned()]),
            "{}",
            path.display()
        );
        let name = path
            .file_name()
            .expect("a name")
            .to_string_lossy()
            .into_owned();
        cases.insert(
            name,
            (
                fs::read(path.join("case.json")).expect("case.json"),
                fs::read(path.join("scene.png")).expect("scene.png"),
            ),
        );
    }
    cases
}

// The names of `cases`.
fn names(cases: &BTreeMap<String, (Vec<u8>, Vec<u8>)>) -> BTreeSet<&str> {
    cases.keys().map(String::as_str).collect()
}

fn case_of(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).expect("case JSON")
}

// The rectangles a case's scene is built of.
fn rects_of(case: &Value) -> Vec<RecordRect> {
    case["scene_construction"]["rects"]
        .as_array()
        .unwrap_or_else(|| panic!("no rects in {case}"))
        .iter()
        .map(|item| {
            let member = |name: &str| {
                item[name]
                    .as_i64()
                    .and_then(|value| i32::try_from(value).ok())
                    .unwrap_or_else(|| panic!("no {name} in {item}"))
            };
            rect(member("x"), member("y"), member("width"), member("height"))
        })
        .collect()
}

// The targets one rectangle of a case's scene names as its content.
fn content_of(case: &Value, index: usize) -> BTreeSet<String> {
    case["scene_construction"]["rects"][index]["content"]
        .as_str()
        .unwrap_or_else(|| panic!("no content in {case}"))
        .split(", ")
        .map(str::to_owned)
        .collect()
}

// The work order's test 1: three steps with a press each and the final screen give four cases at
// the stop; the ending taken after it gives a fifth at the regeneration. Each is in the contract's
// form (its id, its package, its scene and how it was made, its source, what it expects), and each,
// read from the disk, passes when `run_regression_case` runs it on the package written.
#[test]
fn s210_three_steps_and_an_ending_write_five_cases_that_pass_read_from_disk() {
    let bench = Bench::recorded(&four_screens());
    bench.squares(4);
    let generated = bench.stopped(false);
    assert_eq!(generated["lab_status"], "generated", "{generated}");
    assert_eq!(
        generated["cases"]["counts"],
        json!({"steps": 3, "final": 1, "endings": 0}),
        "{generated}"
    );
    assert_eq!(generated["cases"]["status"], "written", "{generated}");

    bench.take_alpha_ending();
    let lab = bench.stopped(false);
    assert_eq!(lab["lab_status"], "regenerated", "{lab}");
    let answer = &lab["cases"];
    assert_eq!(
        answer["counts"],
        json!({"steps": 3, "final": 1, "endings": 1}),
        "{answer}"
    );
    assert_eq!(answer["ids"], json!(FIVE), "{answer}");
    assert_eq!(answer["not_proven"], json!([]), "{answer}");
    assert_eq!(answer["warnings"], json!([]), "{answer}");
    assert_eq!(answer["limits"], json!(["self_agreement"]), "{answer}");
    assert!(
        answer["limit_texts"]["self_agreement"]
            .as_str()
            .is_some_and(|text| text.contains("says nothing about screens it did not meet")),
        "{answer}"
    );

    let package = bench.package(&lab);
    let cases = written_cases(&cases_dir(&lab));
    assert_eq!(names(&cases), FIVE.into_iter().collect(), "{answer}");
    let expected = [
        ("step_01", "neutral/step_01", "square_1", json!({"step": 1})),
        ("step_02", "neutral/step_02", "square_2", json!({"step": 2})),
        ("step_03", "neutral/step_03", "square_3", json!({"step": 3})),
        (
            "final",
            "neutral/step_04",
            "square_4",
            json!({"final": true}),
        ),
        (
            "ending_neutral_task_alpha",
            "neutral/ending_01",
            ALPHA_MARK,
            json!({"ending": ALPHA_KEY}),
        ),
    ];
    for (id, page, target, mut source) in expected {
        let (case_json, scene) = &cases[id];
        let case = case_of(case_json);
        source["record_id"] = json!(RECORD_ID);
        assert_eq!(
            case["schema_version"], REGRESSION_CASE_SCHEMA,
            "{id}: {case}"
        );
        assert_eq!(case["id"], id, "{case}");
        assert_eq!(case["package_id"], PACKAGE_ID, "{case}");
        assert_eq!(case["scene"], "scene.png", "{case}");
        assert_eq!(case["source"], source, "{case}");
        assert_eq!(
            case["expect"],
            json!({"observe": {"page": page},
                   "recognize": [{"target": target, "passed": true}]}),
            "{case}"
        );
        let construction = &case["scene_construction"];
        assert_eq!(construction["kind"], "roi_patch", "{case}");
        assert_eq!(
            construction["canvas"],
            json!({"width": WIDTH, "height": HEIGHT, "fill": [0, 0, 0]}),
            "{case}"
        );
        assert_eq!(
            construction["scene_sha256"],
            json!(hex_sha256(scene)),
            "{case}"
        );
        assert_eq!(rects_of(&case), [SQUARE], "{case}");
        assert_eq!(
            content_of(&case, 0),
            ["square_1", "square_2", "square_3", "square_4", ALPHA_MARK]
                .map(str::to_owned)
                .into_iter()
                .collect(),
            "{case}"
        );
        let verdict = run_regression_case(&package, case_json, scene, &bench.ocr);
        assert_eq!(
            verdict.verdict,
            RegressionVerdict::Passed,
            "{id}: {verdict:?}"
        );
        assert!(verdict.warnings.is_empty(), "{id}: {verdict:?}");
    }
}

// Each warning of `warnings`, a list of `lab.cases.warnings` or of `record ready`'s `warnings`, without
// its text.
fn without_message(warnings: &Value) -> Vec<Value> {
    warnings
        .as_array()
        .unwrap_or_else(|| panic!("no warnings in {warnings}"))
        .iter()
        .map(|warning| {
            let mut warning = warning.clone();
            warning
                .as_object_mut()
                .expect("a warning")
                .remove("message")
                .expect("its message");
            warning
        })
        .collect()
}

// The work order's test 2: the package is byte for byte what it is without this slice. Fix round 3
// (the review of #1797: the earlier test compared the stop with a second call of the slice's own
// path): the file `record stop` wrote is compared with bytes that path does not make, the
// generator's own rendering (`render`) and encoding (`encode`) of the recording's plan as it stands
// before the stop, given the three check results the package's provenance records (`warnings`,
// `arrival_by_time_window`, `cross_check`, read back from the file); and that provenance holds
// exactly the members the generator wrote before this slice, nothing of the cases.
#[test]
fn s210_the_package_is_byte_for_byte_what_it_is_without_the_cases() {
    let bench = Bench::recorded(&four_screens());
    bench.squares(4);
    let recording: LabRecording = serde_json::from_slice(
        &fs::read(recording_path(&bench.state(), RECORD_ID)).expect("step table"),
    )
    .expect("step table JSON");
    let settings = StopSettings {
        game: Some("neutral".to_owned()),
        server: Some("test".to_owned()),
        locale: Some("en-US".to_owned()),
        package_id: None,
        requires: None,
        timeout_ms: None,
        arrival_timeout_ms: DEFAULT_ARRIVAL_TIMEOUT_MS,
        application_arrival_timeout_ms: DEFAULT_APPLICATION_ARRIVAL_TIMEOUT_MS,
    };
    let plan = generate_plan(&recording, &settings)
        .unwrap_or_else(|error| panic!("the plan: {}: {}", error.code, error.message));

    let lab = bench.stopped(false);
    assert_eq!(lab["lab_status"], "generated", "{lab}");
    assert_eq!(lab["container"], "json", "{lab}");
    assert_eq!(
        lab["cases"]["ids"],
        json!(["step_01", "step_02", "step_03", "final"]),
        "{lab}"
    );
    let written = fs::read(lab["path"].as_str().expect("package path")).expect("package");
    let entries = expand_content_container(
        &written,
        ContentContainer::Json,
        ContainmentLimits::default(),
    )
    .expect("package container");
    let task: Value = entries
        .iter()
        .find(|(name, _)| name.ends_with("/task.json"))
        .map(|(_, bytes)| serde_json::from_slice::<Value>(bytes).expect("task JSON"))
        .expect("the package's task.json");
    let provenance = task["provenance"]
        .as_object()
        .unwrap_or_else(|| panic!("no provenance in {task}"));
    assert_eq!(
        provenance
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "arrival_by_time_window",
            "cross_check",
            "generated_at_unix_ms",
            "generator",
            "record_id",
            "recording_schema",
            "source",
            "steps",
            "warnings"
        ]),
        "{task}"
    );
    // In the order the self-checks hand them to the generator.
    let checks = ["warnings", "arrival_by_time_window", "cross_check"]
        .into_iter()
        .map(|key| {
            let value = provenance
                .get(key)
                .cloned()
                .unwrap_or_else(|| panic!("no {key} in {task}"));
            (key.to_owned(), value)
        })
        .collect::<serde_json::Map<String, Value>>();
    let rendered = render(&plan, Some(&checks), &BTreeMap::new())
        .unwrap_or_else(|error| panic!("render: {}: {}", error.code, error.message));
    let rebuilt = encode(&rendered, container_kind(&rendered))
        .unwrap_or_else(|error| panic!("encode: {}: {}", error.code, error.message));
    assert!(
        written == rebuilt,
        "the package record stop wrote is not the generator's package of the recording"
    );
}

// The work order's test 3: every pixel of every screen that no target reads is the marker colour.
// No scene holds a pixel of that colour, every pixel outside its rectangles is the canvas colour,
// and the runner, run on the written package, passes each case with no warning of its scene rule.
#[test]
fn s210_a_scene_holds_no_pixel_of_the_frame_outside_the_regions_the_targets_read() {
    let screens = [RED, GREEN, BLUE, YELLOW].map(|square| screen_on(MARKER, &[(SQUARE, square)]));
    let bench = Bench::recorded(&screens);
    bench.squares(4);
    let lab = bench.stopped(false);
    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let package = bench.package(&lab);
    let cases = written_cases(&cases_dir(&lab));
    assert_eq!(names(&cases).len(), 4, "{lab}");
    for (id, (case_json, scene_png)) in &cases {
        let case = case_of(case_json);
        let rects = rects_of(&case);
        assert_eq!(rects, [SQUARE], "{id}: {case}");
        let scene = Scene::from_png(scene_png).expect("the scene");
        assert_eq!((scene.width(), scene.height()), (WIDTH, HEIGHT), "{id}");
        let (pixels, _) = scene.rgb8_pixels().as_chunks::<3>();
        for (index, pixel) in (0_u32..).zip(pixels) {
            let (x, y) = (index % WIDTH, index / WIDTH);
            assert_ne!(
                *pixel, MARKER,
                "{id}: the scene holds the frame at ({x}, {y})"
            );
            if !rects.iter().any(|area| covers(*area, x, y)) {
                assert_eq!(*pixel, BLACK, "{id}: ({x}, {y}) is not the canvas colour");
            }
        }
        let verdict = run_regression_case(&package, case_json, scene_png, &bench.ocr);
        assert_eq!(
            verdict.verdict,
            RegressionVerdict::Passed,
            "{id}: {verdict:?}"
        );
        assert!(verdict.warnings.is_empty(), "{id}: {verdict:?}");
    }
}

// The work order's test 4: step 2's page requires the red square, which step 1's screen shows too,
// and forbids `blue_3`, step 3's blue mark, which step 1's screen shows and step 2's does not. Step
// 1's page is told apart from step 2's on step 1's screen only by that forbidden target of another
// page: its case's scene holds `blue_3`'s region (the whole package's regions, not only its own
// page's), so step 2's page does not pass there and the case passes.
#[test]
fn s210_a_page_told_apart_by_another_pages_forbidden_target_passes() {
    let bench = Bench::recorded(&[
        screen(&[(SQUARE, RED), (SECOND, BLUE), (THIRD, CYAN)]),
        screen(&[(SQUARE, RED)]),
        screen(&[(SQUARE, GREEN), (SECOND, BLUE)]),
        screen(&[(SQUARE, YELLOW)]),
    ]);
    bench.marked(json!({"step": 3, "add": [color("square_3", SQUARE), color("blue_3", SECOND)]}));
    bench.marked(json!({"step": 1, "add": [color("square_1", SQUARE), color("cyan_1", THIRD)]}));
    bench.marked(json!({"step": 2, "add": [color("square_2", SQUARE)], "forbid": ["blue_3"]}));
    bench.marked(json!({"step": 4, "add": [color("square_4", SQUARE)]}));
    let lab = bench.stopped(false);
    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let package = bench.package(&lab);
    let cases = written_cases(&cases_dir(&lab));
    let (case_json, scene) = &cases["step_01"];
    let case = case_of(case_json);
    let rects = rects_of(&case);
    let second = rects
        .iter()
        .position(|area| *area == SECOND)
        .unwrap_or_else(|| panic!("the scene holds no region of blue_3: {case}"));
    assert_eq!(
        content_of(&case, second),
        BTreeSet::from(["blue_3".to_owned()]),
        "{case}"
    );
    assert_eq!(
        case["expect"]["observe"]["page"], "neutral/step_01",
        "{case}"
    );
    let verdict = run_regression_case(&package, case_json, scene, &bench.ocr);
    assert_eq!(verdict.verdict, RegressionVerdict::Passed, "{verdict:?}");
}

// The work order's test 5: after the stop wrote four cases and the ending was taken, the dry run
// lists the five the regeneration would write (`to_replace`) and writes nothing; the regeneration
// writes the five whole in place of the four, the four set aside and kept as they were.
#[test]
fn s210_the_dry_run_writes_nothing_and_lists_five_and_a_regeneration_replaces_the_cases() {
    let bench = Bench::recorded(&four_screens());
    bench.squares(4);
    let generated = bench.stopped(false);
    let dir = cases_dir(&generated);
    let before = written_cases(&dir);
    assert_eq!(
        names(&before),
        BTreeSet::from(["final", "step_01", "step_02", "step_03"])
    );
    bench.take_alpha_ending();
    let out_before = fs::read_dir(bench.out())
        .expect("the output directory")
        .map(|entry| entry.expect("an entry").file_name())
        .collect::<BTreeSet<_>>();

    let validated = bench.stopped(true);
    assert_eq!(validated["lab_status"], "validated", "{validated}");
    let answer = &validated["cases"];
    assert_eq!(answer["status"], "to_replace", "{answer}");
    assert_eq!(answer["ids"], json!(FIVE), "{answer}");
    assert_eq!(
        answer["counts"],
        json!({"steps": 3, "final": 1, "endings": 1}),
        "{answer}"
    );
    assert!(answer.get("kept").is_none(), "{answer}");
    // Fix round 3: what it would write, whole: the package, the cases' directory and each case.
    let would_write = validated["would_write"]
        .as_array()
        .unwrap_or_else(|| panic!("no would_write in {validated}"))
        .iter()
        .map(|path| PathBuf::from(path.as_str().expect("a path")))
        .collect::<Vec<_>>();
    let mut listed = vec![
        PathBuf::from(validated["path"].as_str().expect("package path")),
        dir.clone(),
    ];
    listed.extend(FIVE.map(|id| dir.join(id)));
    assert_eq!(would_write, listed, "{validated}");
    assert_eq!(written_cases(&dir), before, "the dry run wrote a case");
    let out_after = fs::read_dir(bench.out())
        .expect("the output directory")
        .map(|entry| entry.expect("an entry").file_name())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        out_after, out_before,
        "the dry run wrote beside the package"
    );

    let regenerated = bench.stopped(false);
    assert_eq!(regenerated["lab_status"], "regenerated", "{regenerated}");
    assert_eq!(regenerated["cases"]["status"], "replaced", "{regenerated}");
    assert_eq!(cases_dir(&regenerated), dir);
    let after = written_cases(&dir);
    assert_eq!(names(&after), FIVE.into_iter().collect());
    let kept = PathBuf::from(
        regenerated["cases"]["kept"]
            .as_str()
            .unwrap_or_else(|| panic!("no kept directory in {regenerated}")),
    );
    assert!(
        kept.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(&format!("{PACKAGE_ID}.cases.old-"))),
        "{}",
        kept.display()
    );
    assert_eq!(
        written_cases(&kept),
        before,
        "the earlier set is kept as it was"
    );
}

// The recording of tests 6 and 11, with the scripted OCR model: four screens of a red, a green, a
// blue and a yellow square; each of `reading` (2, 3) shows the marker colour where no target reads,
// and its step's page requires, beside its square, an OCR mark the scripted model reads only on a
// frame that shows the marker. The cross-check passes on the frames; on a case's scene, which holds
// only the regions the package declares, the marker is gone and the page is not recognised.
fn reading_recording(reading: &[u32]) -> Bench {
    let screens = [RED, GREEN, BLUE, YELLOW]
        .into_iter()
        .zip(1_u32..)
        .map(|(square, step)| {
            if reading.contains(&step) {
                screen(&[(SQUARE, square), (UNREAD, MARKER)])
            } else {
                screen(&[(SQUARE, square)])
            }
        })
        .collect::<Vec<_>>();
    let bench = Bench::reading().pressing(&screens);
    for step in 1..=4_u32 {
        let mut marks = vec![color(&format!("square_{step}"), SQUARE)];
        if reading.contains(&step) {
            marks.push(ocr(&format!("text_{step}"), TEXT_AREA));
        }
        bench.marked(json!({"step": step, "add": marks}));
    }
    bench
}

// The work order's test 6, adjusted to the coordinator's ruling: steps 2 and 3 are recognised on
// their frames by text the scripted model reads only where the frame shows the marker, which no
// region of the package holds: on their cases' scenes their pages are not recognised (a real miss,
// `page_mismatch`, and the text target `target_mismatch`). `record ready` lists both as failures of
// the self-check `cases`, the last, every check complete; `record stop` refuses the first,
// `record_case_failed`, and writes no case and no package.
#[test]
fn s210_cases_whose_page_is_not_recognised_are_listed_and_refuse_record_stop() {
    let bench = reading_recording(&[2, 3]);

    let ready = bench.ready();
    assert_eq!(ready["status"], "draft", "{ready}");
    assert_eq!(ready["missing_total"], 2, "{ready}");
    for (item, (step, frame, case)) in ready["missing"]
        .as_array()
        .expect("missing")
        .iter()
        .zip([(2, "f0003", "step_02"), (3, "f0005", "step_03")])
    {
        assert_eq!(item["what"], "cases", "{item}");
        assert_eq!(item["code"], LabCode::RecordCaseFailed.as_str(), "{item}");
        assert_eq!(item["step"], step, "{item}");
        assert_eq!(item["artifact_step"], step, "{item}");
        assert_eq!(item["frame_id"], frame, "{item}");
        let details = &item["details"];
        assert_eq!(details["case"], case, "{item}");
        // Fix round 3: the page as the page detection names it, as the warnings do.
        assert_eq!(details["page"], format!("neutral/{case}"), "{item}");
        assert_eq!(details["frames"], json!([frame]), "{item}");
        let checks = details["checks"].as_array().expect("checks");
        assert_eq!(checks.len(), 2, "{item}");
        assert_eq!(checks[0]["at"], "/expect/observe/page", "{item}");
        assert_eq!(checks[0]["result"], "failed", "{item}");
        assert_eq!(checks[0]["problem"], "page_mismatch", "{item}");
        assert_eq!(
            checks[0]["page"]["expected"],
            format!("neutral/{case}"),
            "{item}"
        );
        assert!(checks[0]["page"]["recognized"].is_null(), "{item}");
        assert_eq!(checks[1]["at"], "/expect/recognize/1", "{item}");
        assert_eq!(checks[1]["problem"], "target_mismatch", "{item}");
        assert_eq!(
            checks[1]["target"],
            json!({"target": format!("text_{step}"), "expected": true, "recognized": false}),
            "{item}"
        );
    }
    let every = [
        "frames",
        "admission",
        "first_decision",
        "cross_check",
        "endings",
        "selects",
        "final_package",
        "cases",
    ]
    .map(|check| json!({"check": check, "complete": true}));
    assert_eq!(ready["checks"], json!(every), "{ready}");
    assert_eq!(ready["checks_after"], json!([]), "{ready}");

    let error = bench.stop(false).expect_err("a case that fails");
    assert_eq!(
        error.code,
        LabCode::RecordCaseFailed.as_str(),
        "{}",
        error.message
    );
    let details = error.details.expect("details");
    assert_eq!(details["case"], "step_02", "{details}");
    assert_eq!(details["step"], 2, "{details}");
    assert_eq!(details["frame_id"], "f0003", "{details}");
    assert_eq!(details["verdict"], "failed", "{details}");
    assert_eq!(
        details["checks"][0]["problem"], "page_mismatch",
        "{details}"
    );
    assert_eq!(
        details["outcome"]["values"]["regression_problem"], "page_mismatch",
        "{details}"
    );
    assert!(
        !bench.out().join(format!("{PACKAGE_ID}.cases")).exists(),
        "a case was written"
    );
    assert!(
        !bench.out().join(format!("{PACKAGE_ID}.json")).exists(),
        "a package was written"
    );
}

// The work order's test 7: step 2's page requires an OCR mark beside its square, and the Lab has no
// OCR model. The cases are written; step 2's is named not proven, its page and its OCR target not
// run with `ocr_unavailable`, the runner's own reason. Fix round 3 (the review of #1797, its (h)):
// the page detection reads that text target on every page, so no case's page is judged: every case
// is not proven, its page check not run, and the answer says so once, `record_cases_ocr_unavailable`,
// its only warning.
#[test]
fn s210_a_page_that_needs_text_recognition_is_written_and_named_not_proven() {
    let bench = Bench::recorded(&four_screens());
    bench.marked(json!({"step": 1, "add": [color("square_1", SQUARE)]}));
    bench.marked(json!({
        "step": 2,
        "add": [color("square_2", SQUARE), ocr("text_2", TEXT_AREA)]
    }));
    bench.marked(json!({"step": 3, "add": [color("square_3", SQUARE)]}));
    bench.marked(json!({"step": 4, "add": [color("square_4", SQUARE)]}));
    let lab = bench.stopped(false);
    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let answer = &lab["cases"];
    let not_proven = ["step_01", "step_02", "step_03", "final"].map(|case| {
        json!({
            "case": case,
            "verdict": "partial",
            "reasons": ["ocr_unavailable"]
        })
    });
    assert_eq!(answer["not_proven"], json!(not_proven), "{answer}");
    assert_eq!(
        without_message(&answer["warnings"]),
        [json!({"code": LabCode::RecordCasesOcrUnavailable.as_str()})],
        "{answer}"
    );
    assert!(
        answer["warnings"][0]["message"]
            .as_str()
            .is_some_and(|text| text.starts_with("No case's page was judged")),
        "{answer}"
    );
    let cases = written_cases(&cases_dir(&lab));
    assert!(cases.contains_key("step_02"), "step_02 is not written");
    let case = case_of(&cases["step_02"].0);
    assert_eq!(
        case["expect"]["recognize"],
        json!([{"target": "square_2", "passed": true}, {"target": "text_2", "passed": true}]),
        "{case}"
    );
    assert!(rects_of(&case).contains(&TEXT_AREA), "{case}");
}

// The work order's test 8: step 2's square is black, so its page passes on the bare canvas too. Its
// case passes and is written, and the answer names it with the warning `record_case_bare_canvas`;
// no other case is named.
#[test]
fn s210_a_page_the_bare_canvas_answers_is_written_with_the_warning() {
    let bench =
        Bench::recorded(&[RED, BLACK, BLUE, YELLOW].map(|square| screen(&[(SQUARE, square)])));
    bench.squares(4);
    let lab = bench.stopped(false);
    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let answer = &lab["cases"];
    let warnings = answer["warnings"].as_array().expect("warnings");
    assert_eq!(warnings.len(), 1, "{answer}");
    assert_eq!(
        warnings[0]["code"],
        LabCode::RecordCaseBareCanvas.as_str(),
        "{answer}"
    );
    assert_eq!(warnings[0]["case"], "step_02", "{answer}");
    assert_eq!(warnings[0]["page"], "neutral/step_02", "{answer}");
    assert_eq!(
        answer["ids"],
        json!(["step_01", "step_02", "step_03", "final"]),
        "{answer}"
    );
    assert!(written_cases(&cases_dir(&lab)).contains_key("step_02"));
}

// The work order's test 9: three taps, the second on a screen it does not change (its screen
// before, its screen after and the third tap's screen before are the same), dropped; steps 1, 3 and
// 4 carry pages. The package's steps 1 to 3 are the recorded steps 1, 3 and 4: two step cases and
// the final screen's, none for the dropped step; each case's `source` is the recording's id and the
// step's serial number, or `final`, and nothing else.
#[test]
fn s210_a_dropped_middle_step_has_no_case_and_a_source_names_no_path() {
    let bench = Bench::new();
    let (first, second, third) = (
        screen(&[(SQUARE, RED)]),
        screen(&[(SQUARE, GREEN)]),
        screen(&[(SQUARE, BLUE)]),
    );
    bench.press(vec![first, second.clone()]);
    bench.press(vec![second.clone(), second.clone()]);
    bench.press(vec![second, third]);
    bench.marked(json!({"step": 1, "add": [color("square_1", SQUARE)]}));
    bench.marked(json!({"step": 3, "add": [color("square_3", SQUARE)]}));
    bench.marked(json!({"step": 4, "add": [color("square_4", SQUARE)]}));
    bench.marked(json!({"step_action": {"kind": "drop_step", "step": 2}}));
    let lab = bench.stopped(false);
    assert_eq!(lab["lab_status"], "generated", "{lab}");
    assert_eq!(
        lab["cases"]["ids"],
        json!(["step_01", "step_02", "final"]),
        "{lab}"
    );
    assert_eq!(
        lab["cases"]["counts"],
        json!({"steps": 2, "final": 1, "endings": 0}),
        "{lab}"
    );
    let cases = written_cases(&cases_dir(&lab));
    for (id, source) in [
        ("step_01", json!({"record_id": RECORD_ID, "step": 1})),
        ("step_02", json!({"record_id": RECORD_ID, "step": 3})),
        ("final", json!({"record_id": RECORD_ID, "final": true})),
    ] {
        let case = case_of(&cases[id].0);
        assert_eq!(case["source"], source, "{id}: {case}");
    }
}

// The warnings `record_case_page_ambiguous` among `warnings`, a list of `record stop`'s
// `lab.cases.warnings` or of `record ready`'s `warnings`.
fn ambiguity_of(warnings: &Value) -> Vec<Value> {
    warnings
        .as_array()
        .unwrap_or_else(|| panic!("no warnings in {warnings}"))
        .iter()
        .filter(|warning| warning["code"] == LabCode::RecordCasePageAmbiguous.as_str())
        .cloned()
        .collect()
}

// The coordinator's ruling after the first round (R2's ruling 4 as precedent): step 2's page, a
// green square beside step 1's red one, also passes on step 1's screen (today's cross-check warns
// `arrival_unconfirmed` for it and refuses nothing). Step 1's case is no miss: it is written without
// `expect.observe`, with step 1's page's target alone, passes when read from disk, and `record
// stop` and `record ready` name it with `record_case_page_ambiguous`, the other page beside it.
// Step 2's case, on whose screen step 1's page does not pass, keeps its `observe`.
#[test]
fn s210_a_later_page_matching_an_earlier_screen_writes_the_case_without_observe() {
    let bench = Bench::recorded(&[
        screen(&[(SQUARE, RED), (SECOND, GREEN)]),
        screen(&[(SQUARE, BLUE), (SECOND, GREEN)]),
        screen(&[(SQUARE, GREEN)]),
        screen(&[(SQUARE, YELLOW)]),
    ]);
    bench.marked(json!({"step": 1, "add": [color("square_1", SQUARE)]}));
    bench.marked(json!({"step": 2, "add": [color("green_2", SECOND)]}));
    bench.marked(json!({"step": 3, "add": [color("square_3", SQUARE)]}));
    bench.marked(json!({"step": 4, "add": [color("square_4", SQUARE)]}));
    let expected = json!({
        "code": LabCode::RecordCasePageAmbiguous.as_str(),
        "case": "step_01",
        "page": "neutral/step_01",
        "other_pages": ["neutral/step_02"],
        "written": true
    });
    let named = |warnings: &Value| {
        ambiguity_of(warnings)
            .into_iter()
            .map(|mut warning| {
                warning
                    .as_object_mut()
                    .expect("a warning")
                    .remove("message")
                    .expect("its message");
                warning
            })
            .collect::<Vec<_>>()
    };

    let ready = bench.ready();
    assert_eq!(ready["status"], "ready", "{ready}");
    assert_eq!(ready["missing"], json!([]), "{ready}");
    assert_eq!(
        named(&ready["warnings"]),
        std::slice::from_ref(&expected),
        "{ready}"
    );

    let lab = bench.stopped(false);
    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let answer = &lab["cases"];
    assert_eq!(
        answer["ids"],
        json!(["step_01", "step_02", "step_03", "final"]),
        "{answer}"
    );
    assert_eq!(named(&answer["warnings"]), [expected], "{answer}");
    let package = bench.package(&lab);
    let cases = written_cases(&cases_dir(&lab));
    let (case_json, scene) = &cases["step_01"];
    let case = case_of(case_json);
    assert_eq!(
        case["expect"],
        json!({"recognize": [{"target": "square_1", "passed": true}]}),
        "{case}"
    );
    let verdict = run_regression_case(&package, case_json, scene, &bench.ocr);
    assert_eq!(verdict.verdict, RegressionVerdict::Passed, "{verdict:?}");
    let second = case_of(&cases["step_02"].0);
    assert_eq!(
        second["expect"]["observe"]["page"], "neutral/step_02",
        "{second}"
    );
}

// The coordinator's ruling after the first round, its first point: step 2's page is recognised on
// its frame by text the scripted model reads only where the frame shows the marker, which no region
// of the package holds; on its case's scene the page is not recognised at all. `record stop`
// refuses `record_case_failed` with `page_mismatch`, the page expected and none recognised, its
// text target failing too; nothing is written and the recording stays active.
#[test]
fn s210_an_expected_page_not_recognised_refuses_record_stop() {
    let bench = reading_recording(&[2]);
    let error = bench.stop(false).expect_err("a page not recognised");
    assert_eq!(
        error.code,
        LabCode::RecordCaseFailed.as_str(),
        "{}",
        error.message
    );
    let details = error.details.expect("details");
    assert_eq!(details["case"], "step_02", "{details}");
    // Fix round 3: the page as the page detection names it, as the warnings do.
    assert_eq!(details["page"], "neutral/step_02", "{details}");
    assert_eq!(
        details["outcome"]["values"]["regression_problem"], "page_mismatch",
        "{details}"
    );
    let checks = details["checks"].as_array().expect("checks");
    assert_eq!(checks.len(), 2, "{details}");
    assert_eq!(checks[0]["problem"], "page_mismatch", "{details}");
    assert_eq!(
        checks[0]["page"],
        json!({"expected": "neutral/step_02", "recognized": null}),
        "{details}"
    );
    assert_eq!(checks[1]["problem"], "target_mismatch", "{details}");
    assert!(
        !bench.out().join(format!("{PACKAGE_ID}.cases")).exists(),
        "a case was written"
    );
    let recording: Value = serde_json::from_slice(
        &fs::read(recording_path(&bench.state(), RECORD_ID)).expect("step table"),
    )
    .expect("step table JSON");
    assert_eq!(recording["status"], "active", "{recording}");
}

// Fix round 3, the review's first point: step 4's colour mark reads the whole frame (a cyan screen
// with a yellow square), so every case's scene is its whole frame: each holds every pixel of its
// screen (no recorded screen shows the canvas colour), its rectangles are the squares' region and the
// whole frame, and the runner passes it with no warning of its scene rule. The answer names each case
// with `record_case_whole_frame` and the target that reads the whole frame, its only warnings, and
// nothing is refused.
#[test]
fn s210_a_target_that_reads_the_whole_frame_makes_each_scene_the_whole_frame_and_names_it() {
    let bench = Bench::recorded(&[
        screen(&[(SQUARE, RED)]),
        screen(&[(SQUARE, GREEN)]),
        screen(&[(SQUARE, BLUE)]),
        screen_on(CYAN, &[(SQUARE, YELLOW)]),
    ]);
    bench.squares(3);
    bench.marked(json!({"step": 4, "add": [color("whole_4", WHOLE)]}));
    let lab = bench.stopped(false);
    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let answer = &lab["cases"];
    let ids = ["step_01", "step_02", "step_03", "final"];
    assert_eq!(answer["ids"], json!(ids), "{answer}");
    let named = ids
        .into_iter()
        .zip(1_u32..)
        .map(|(case, step)| {
            json!({
                "code": LabCode::RecordCaseWholeFrame.as_str(),
                "case": case,
                "page": format!("neutral/step_{step:02}"),
                "targets": ["whole_4"]
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(without_message(&answer["warnings"]), named, "{answer}");
    let package = bench.package(&lab);
    let cases = written_cases(&cases_dir(&lab));
    for id in ids {
        let (case_json, scene_png) = &cases[id];
        let case = case_of(case_json);
        assert_eq!(rects_of(&case), [SQUARE, WHOLE], "{id}: {case}");
        assert_eq!(
            content_of(&case, 1),
            BTreeSet::from(["whole_4".to_owned()]),
            "{id}: {case}"
        );
        let scene = Scene::from_png(scene_png).expect("the scene");
        let (pixels, _) = scene.rgb8_pixels().as_chunks::<3>();
        assert!(
            pixels.iter().all(|pixel| *pixel != BLACK),
            "{id}: the scene is not its whole frame"
        );
        let verdict = run_regression_case(&package, case_json, scene_png, &bench.ocr);
        assert_eq!(
            verdict.verdict,
            RegressionVerdict::Passed,
            "{id}: {verdict:?}"
        );
        assert!(verdict.warnings.is_empty(), "{id}: {verdict:?}");
    }
}

// Fix round 3, the review's (b), its first weak point (the coordinator's ruling): step 1's page
// forbids step 3's square (green, which step 1's red square is not), and step 2's green mark holds on
// step 1's screen too. Step 1's case, written without `observe`, expects its own square to pass and
// the forbidden square not to: it states that step 1's page holds on its screen, and passes when read
// from disk; `record_case_page_ambiguous` names it, the answer's only warning.
#[test]
fn s210_an_ambiguous_case_pins_the_targets_its_page_forbids_failing() {
    let bench = Bench::recorded(&[
        screen(&[(SQUARE, RED), (SECOND, GREEN)]),
        screen(&[(SQUARE, BLUE), (SECOND, GREEN)]),
        screen(&[(SQUARE, GREEN)]),
        screen(&[(SQUARE, YELLOW)]),
    ]);
    bench.marked(json!({"step": 3, "add": [color("square_3", SQUARE)]}));
    bench.marked(json!({"step": 1, "add": [color("square_1", SQUARE)], "forbid": ["square_3"]}));
    bench.marked(json!({"step": 2, "add": [color("green_2", SECOND)]}));
    bench.marked(json!({"step": 4, "add": [color("square_4", SQUARE)]}));
    let lab = bench.stopped(false);
    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let answer = &lab["cases"];
    assert_eq!(
        without_message(&answer["warnings"]),
        [json!({
            "code": LabCode::RecordCasePageAmbiguous.as_str(),
            "case": "step_01",
            "page": "neutral/step_01",
            "other_pages": ["neutral/step_02"],
            "written": true
        })],
        "{answer}"
    );
    let package = bench.package(&lab);
    let cases = written_cases(&cases_dir(&lab));
    let (case_json, scene) = &cases["step_01"];
    let case = case_of(case_json);
    assert_eq!(
        case["expect"],
        json!({"recognize": [
            {"target": "square_1", "passed": true},
            {"target": "square_3", "passed": false}
        ]}),
        "{case}"
    );
    let verdict = run_regression_case(&package, case_json, scene, &bench.ocr);
    assert_eq!(verdict.verdict, RegressionVerdict::Passed, "{verdict:?}");
}

// Fix round 3, the review's (h): a step whose press frame is no longer one of its live frames gives
// the case of its primary frame. Step 2's screen changed between the capture after press 1 (a cyan
// square) and the capture press 2 was sent on (a green square, which set the cyan one aside); the
// step table is made to name the frame set aside as press 2's. Step 2's case is made of its primary
// frame, the green square, and passes.
#[test]
fn s210_a_press_frame_no_longer_live_gives_the_case_of_the_steps_primary_frame() {
    let bench = Bench::new();
    bench.press(vec![screen(&[(SQUARE, RED)]), screen(&[(SQUARE, CYAN)])]);
    bench.press(vec![screen(&[(SQUARE, GREEN)]), screen(&[(SQUARE, BLUE)])]);
    bench.press(vec![screen(&[(SQUARE, BLUE)]), screen(&[(SQUARE, YELLOW)])]);
    bench.squares(4);
    let path = recording_path(&bench.state(), RECORD_ID);
    let mut table: Value =
        serde_json::from_slice(&fs::read(&path).expect("step table")).expect("step table JSON");
    let second = &mut table["steps"][1];
    let set_aside = second["frames"]
        .as_array()
        .and_then(|frames| frames.iter().find(|frame| frame["superseded"] == true))
        .map(|frame| frame["frame_id"].clone())
        .expect("the frame step 2 set aside");
    assert_ne!(
        second["click"]["execution"]["before_frame"], set_aside,
        "{second}"
    );
    second["click"]["execution"]["before_frame"] = set_aside;
    fs::write(
        &path,
        serde_json::to_vec_pretty(&table).expect("step table JSON"),
    )
    .expect("step table");

    let lab = bench.stopped(false);
    assert_eq!(lab["lab_status"], "generated", "{lab}");
    assert_eq!(
        lab["cases"]["ids"],
        json!(["step_01", "step_02", "step_03", "final"]),
        "{lab}"
    );
    let package = bench.package(&lab);
    let cases = written_cases(&cases_dir(&lab));
    let (case_json, scene_png) = &cases["step_02"];
    let scene = Scene::from_png(scene_png).expect("the scene");
    assert_eq!(
        scene.rgb8_pixels()[..3],
        GREEN,
        "step 2's case is not made of its primary frame"
    );
    let verdict = run_regression_case(&package, case_json, scene_png, &bench.ocr);
    assert_eq!(verdict.verdict, RegressionVerdict::Passed, "{verdict:?}");
}

// Fix round 3, the review's (b), its second weak point: a case not written and a case written
// without the page judgement are two consequences, two words. No recording reaches the first (a page
// set requires a target on every page), so the slice's decision is called on its own: a page
// recognised beside another page with no target to pin gives no expectation, no case and the
// warning `record_case_ambiguous_not_written`, which `record ready` names too; with a target it
// requires and one it forbids, the expectation pins both and warns nothing.
#[test]
fn s210_an_ambiguous_page_with_no_target_to_pin_is_not_written_with_its_own_word() {
    let mut set = CaseSet::default();
    let others = ["neutral/step_02".to_owned()];
    assert!(
        set.without_page_judgement("step_01", "neutral/step_01", &others, &[], &[])
            .is_none(),
        "an expectation of nothing"
    );
    let pinned = set.without_page_judgement(
        "step_03",
        "neutral/step_03",
        &others,
        &["square_3".to_owned()],
        &["square_1".to_owned()],
    );
    assert_eq!(
        pinned,
        Some(json!({"recognize": [
            {"target": "square_3", "passed": true},
            {"target": "square_1", "passed": false}
        ]}))
    );
    let expected = json!({
        "code": LabCode::RecordCaseAmbiguousNotWritten.as_str(),
        "case": "step_01",
        "page": "neutral/step_01",
        "other_pages": ["neutral/step_02"],
        "written": false
    });
    assert_eq!(
        without_message(&Value::Array(set.ambiguity_warnings().to_vec())),
        std::slice::from_ref(&expected)
    );
    let answer = set.answer("cases".to_owned(), "written", None);
    assert_eq!(without_message(&answer["warnings"]), [expected], "{answer}");
    assert_eq!(answer["ids"], json!([]), "{answer}");
    assert!(set.files().is_empty(), "a case was made");
}

// Fix round 3, the work order's item 8 (d): a case whose run again without the page judgement fails
// is a real miss. Test 10's screens and marks, step 1's page also requiring a text mark that the
// scripted model `OnceOcr` reads on every recorded screen and on a case's scene only once. On step
// 1's scene the page detection reads it, so step 1's page and step 2's both match (ambiguity, no
// miss); made again without `observe`, the case reads it again and finds nothing: `record stop`
// refuses `record_case_failed` with that run's one failed check, the text target at
// `/expect/recognize/1` (no page check: it is the run without the page judgement), and writes no
// case.
#[test]
fn s210_an_ambiguous_case_that_fails_its_run_again_refuses_record_stop() {
    let bench = Bench::with_ocr(Some(LabOcr::with_provider(Arc::new(OnceOcr::default()))))
        .pressing(&[
            screen(&[(SQUARE, RED), (SECOND, GREEN)]),
            screen(&[(SQUARE, BLUE), (SECOND, GREEN)]),
            screen(&[(SQUARE, GREEN)]),
            screen(&[(SQUARE, YELLOW)]),
        ]);
    bench.marked(json!({
        "step": 1,
        "add": [color("square_1", SQUARE), ocr("text_1", TEXT_AREA)]
    }));
    bench.marked(json!({"step": 2, "add": [color("green_2", SECOND)]}));
    bench.marked(json!({"step": 3, "add": [color("square_3", SQUARE)]}));
    bench.marked(json!({"step": 4, "add": [color("square_4", SQUARE)]}));
    let error = bench
        .stop(false)
        .expect_err("a case that fails its run again");
    assert_eq!(
        error.code,
        LabCode::RecordCaseFailed.as_str(),
        "{}",
        error.message
    );
    let details = error.details.expect("details");
    assert_eq!(details["case"], "step_01", "{details}");
    assert_eq!(details["page"], "neutral/step_01", "{details}");
    assert_eq!(
        details["outcome"]["values"]["regression_problem"], "target_mismatch",
        "{details}"
    );
    let checks = details["checks"].as_array().expect("checks");
    assert_eq!(checks.len(), 1, "{details}");
    assert_eq!(checks[0]["at"], "/expect/recognize/1", "{details}");
    assert_eq!(checks[0]["problem"], "target_mismatch", "{details}");
    assert_eq!(
        checks[0]["target"],
        json!({"target": "text_1", "expected": true, "recognized": false}),
        "{details}"
    );
    assert!(
        !bench.out().join(format!("{PACKAGE_ID}.cases")).exists(),
        "a case was written"
    );
}
