// SPDX-License-Identifier: AGPL-3.0-only

// #463 L2: a recorded step presses what it finds by its name. An OCR mark of the request form may
// declare `locate` (with `unique`), the task's locate target member for member, and a click may
// follow it: alone the package clicks the found block or the declared cell holding it (`target`),
// with a rectangle that box's corner plus the rectangle's offset from it on the primary frame
// (`offset`). The mark-time self-test evaluates the target with the Lab's OCR provider on every
// live frame and refuses, with the kernel's typed reason, a name on no frame, a name outside every
// cell and a second match within the margin; the follow refuses a click box that leaves a frame
// and an executed press outside the box the primary frame gives. `record stop` generates the
// locate target, the `target` or `offset` click, the `verify_text` guard and the input recheck
// `fresh_frame`, which the daemon's admission takes and whose fresh frame the input is drawn
// from. A recording without a located-text mark generates its package as before.
//
// Fix round 1 (brief rd-1114/brief-1716-fix1.md, the coordinator's rulings 1 to 3): the followed
// located-text mark is the click's guard and target and no mark of the page, which another mark of
// the step recognizes (`page_needs_followed_text` otherwise), so a screen without the name is the
// step's page and the guard refuses it with its typed cause; a request that changes only the
// followed mark's names is not held to the recorded press, is remembered on the click's follow
// (`names_changed_after_press`) and reported by `record stop`'s cross-check; `--click-guard` may name
// the followed located-text mark. Every step 1 of the tests before fix 1 now has the color mark
// `state/page` beside its located-text mark, which recognizes its page.
//
// Frames are synthetic: 96x64 pixels of flat gray. The scripted OCR provider reads one block of
// 8x6 pixels from each marker pixel inside the requested region, its text chosen by the marker's
// gray value. No device is used. Requirement basis: brief rd-1114/brief-463-l2-record-located-
// text.md, "What to build" items 1 to 5 and "Tests".

use crate::codes::{ClickFollowProblem, LabCode, LabLocation};
use crate::recording::crosscheck::admit;
use crate::recording::store::{old_record_path, recording_path};
use crate::{
    ClickEffect, ClickSpec, CommitClickRequest, LAB_RECORD_MARK_SCHEMA, LabOcr, MarkFamily,
    MarkOutcome, MarkRequest, MarkSpec, PlanClickRequest, RecordRect, RecordStartOptions,
    RecordStopOptions, RecordingLock, SelfTestStatus, record_commit_click, record_mark,
    record_plan_click, record_start, record_status, record_stop,
};
use actingcommand_contract::{InputAction, LabError};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_execution_kernel::{
    ExecutionKernelCode, OfflineDecision, simulate_contained_task,
};
use actingcommand_pack_containment::{
    ContainmentLimits, ContentContainer, expand_content_container,
};
use actingcommand_recognition_pack::{
    NnProviderRequest, NnProviderResult, OcrProviderRequest, OcrProviderResult,
    OcrProviderTextBlock, PackRect, RecognitionPackCode, VisionProvider, VisionProviderError,
    VisionProviderErrorCode,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::sync::Arc;
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_l2";
const TASK_ID: &str = "neutral_task";
const WIDTH: u32 = 96;
const HEIGHT: u32 = 64;
const BACKGROUND: u8 = 60;
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const MARK: &str = "text/name";
// Fix 1: the mark that recognizes the page of step 1, a color mark on a flat area of every frame.
const PAGE: &str = "state/page";

// The region the located-text mark reads: a row of four cards.
const NAMES: RecordRect = RecordRect {
    x: 8,
    y: 8,
    width: 80,
    height: 24,
};
// The block the scripted provider reads from a marker pixel.
const BLOCK_WIDTH: i32 = 8;
const BLOCK_HEIGHT: i32 = 6;
// The gray value of a marker pixel and the name the provider reads from it.
const ALPHA: u8 = 211;
const BETA: u8 = 212;
const MARKERS: [(u8, &str); 2] = [(ALPHA, "ALPHA"), (BETA, "BETA")];

// Reads one block per marker pixel of the requested region (the pixel its top-left corner, the
// block clipped to the region), at confidence 0.9; a region without a marker reads nothing.
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
                            width: BLOCK_WIDTH.min(right - x),
                            height: BLOCK_HEIGHT.min(bottom - y),
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
    // A value in `low..=high`.
    fn between(&mut self, low: u8, high: u8) -> u8 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        low + ((self.0 >> 33) % (u64::from(high - low) + 1)) as u8
    }
}

// A gray image, row-major; it is drawn with equal red, green and blue.
#[derive(Clone)]
struct Gray {
    width: u32,
    height: u32,
    values: Vec<u8>,
}

impl Gray {
    fn from_fn(width: u32, height: u32, mut value: impl FnMut(u32, u32) -> u8) -> Self {
        let mut values = Vec::with_capacity((width * height) as usize);
        for y in 0..height {
            for x in 0..width {
                values.push(value(x, y));
            }
        }
        Self {
            width,
            height,
            values,
        }
    }

    // A screen with the named markers: `(marker, x, y)`.
    fn names(markers: &[(u8, u32, u32)]) -> Self {
        let mut screen = Self::from_fn(WIDTH, HEIGHT, |_, _| BACKGROUND);
        for &(value, x, y) in markers {
            screen.values[(y * WIDTH + x) as usize] = value;
        }
        screen
    }

    // The next screen: a noise patch at (40, 46), with no marker value.
    fn next() -> Self {
        let mut noise = Noise(0x0463_0002);
        let patch = Self::from_fn(12, 12, |_, _| noise.between(1, 199));
        let mut screen = Self::from_fn(WIDTH, HEIGHT, |_, _| BACKGROUND);
        for row in 0..12 {
            for column in 0..12 {
                screen.values[((46 + row) * WIDTH + 40 + column) as usize] =
                    patch.values[(row * 12 + column) as usize];
            }
        }
        screen
    }

    fn png(&self) -> Vec<u8> {
        let pixels = self
            .values
            .iter()
            .flat_map(|value| [*value; 3])
            .collect::<Vec<_>>();
        Frame::from_pixels(
            self.width,
            self.height,
            pixels,
            PixelFormat::Rgb8,
            CaptureBackendName::FixtureSimulation,
        )
        .expect("fixture frame")
        .encode_png_fast()
        .expect("fixture PNG")
    }
}

fn rect(x: i32, y: i32, width: i32, height: i32) -> RecordRect {
    RecordRect {
        x,
        y,
        width,
        height,
    }
}

// The cell `k` of the row of four cards: 18 by 24 pixels, 20 pixels apart, from the region's
// corner.
fn cell(k: i32) -> RecordRect {
    rect(NAMES.x + 20 * k, NAMES.y, 18, 24)
}

fn cells() -> Value {
    json!({"x": 0, "y": 0, "width": 18, "height": 24, "pitch_x": 20, "pitch_y": 24,
           "columns": 4, "rows": 1})
}

fn locate(with_cells: bool) -> Value {
    let mut locate =
        json!({"normalize": "identity_v1", "blocks": "detect", "line_merge": {"gap_factor": 0}});
    if with_cells {
        locate["cells"] = cells();
    }
    locate
}

// A located-text mark of the request form that finds one of `names` in the region.
fn located(names: &[&str], with_cells: bool, unique_milli: Option<u32>) -> MarkSpec {
    let mut spec = json!({
        "id": MARK, "family": "ocr", "region": NAMES,
        "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact", "expected": names,
        "case_sensitive": false, "minimum_confidence": 0.8,
        "model_ref": "neutral-ocr", "model_sha256": MODEL_SHA256,
        "locate": locate(with_cells)
    });
    if let Some(margin_milli) = unique_milli {
        spec["unique"] = json!({"margin_milli": margin_milli});
    }
    serde_json::from_value(spec).expect("a located-text mark request")
}

// Fix 1: a color mark on the flat area (2, 50, 4, 4), which every frame of step 1 shows alike and
// no marker or noise patch reaches: it recognizes step 1's page without the located text.
fn page_mark() -> MarkSpec {
    MarkSpec::region_only(PAGE.to_owned(), MarkFamily::Color, rect(2, 50, 4, 4))
}

fn template(id: &str, crop: RecordRect) -> MarkSpec {
    MarkSpec {
        search: Some(crop),
        ..MarkSpec::region_only(id.to_owned(), MarkFamily::Template, crop)
    }
}

fn follow(region: Option<RecordRect>) -> ClickSpec {
    ClickSpec {
        region,
        follow: Some(MARK.to_owned()),
        ..ClickSpec::default()
    }
}

// The card row with ALPHA in cell 0 (its block's center in that cell), and others as given.
fn alpha_in_cell_0() -> Gray {
    Gray::names(&[(ALPHA, 10, 12)])
}

// ALPHA in cell 0 and BETA in cell 1 (the block (30, 12, 8, 6), its center (34, 15)).
fn alpha_and_beta() -> Gray {
    Gray::names(&[(ALPHA, 10, 12), (BETA, 30, 12)])
}

// Fix 1: the card row with no name on it; the page mark holds there as on every flat frame.
fn no_name() -> Gray {
    Gray::names(&[])
}

// Fix 1: the decision of a package whose step page was recognized and whose guard on the located
// text refused: the kernel's guard refusal naming the target and its typed cause, and no input.
fn assert_guard_refused(decision: &OfflineDecision, cause: RecognitionPackCode) {
    match decision {
        OfflineDecision::Refused { code, detail } => {
            assert_eq!(
                code.as_str(),
                ExecutionKernelCode::ContainedTaskGuardRefused.as_str(),
                "{decision:?}"
            );
            let text = detail.as_deref().unwrap_or_default();
            assert!(text.contains(&format!("target={MARK}")), "{decision:?}");
            assert!(
                text.contains(&format!("cause={}", cause.as_str())),
                "{decision:?}"
            );
        }
        other => panic!("not the guard's refusal: {other:?}"),
    }
}

// A state directory with an active session of `INSTANCE` and its Lab recording, before any
// step, with the scripted OCR provider (`ocr`) and without an OCR model (`none`). The lock is
// dropped before the directory.
struct Recording {
    lock: RecordingLock,
    ocr: LabOcr,
    none: LabOcr,
    temp: TempDir,
}

impl Recording {
    fn new() -> Self {
        let temp = TempDir::new().expect("temporary directory");
        let state = temp.path().join("state");
        fs::create_dir_all(&state).expect("state directory");
        // The session file `record start` writes before the Lab recording is created.
        fs::write(
            old_record_path(&state, INSTANCE),
            serde_json::to_vec(&json!({
                "record_id": RECORD_ID,
                "task_id": TASK_ID,
                "instance": INSTANCE,
                "status": "active",
                "started_at_unix_ms": 1_700_000_000_000_u64
            }))
            .expect("session JSON"),
        )
        .expect("session file");
        let lock =
            RecordingLock::acquire(&state, INSTANCE, "gate_l2", "Lab-CLI").expect("recording lock");
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
        Self {
            lock,
            ocr: LabOcr::with_provider(Arc::new(NamesOcr)),
            none: LabOcr::from_vision_root(&temp.path().join("no-vision")),
            temp,
        }
    }

    // Writes `image` as the frame file `name` and returns its path.
    fn frame(&self, name: &str, image: &Gray) -> String {
        let path = self.temp.path().join(format!("{name}.png"));
        fs::write(&path, image.png()).expect("frame file");
        path.display().to_string()
    }

    fn mark_with(&self, request: MarkRequest, ocr: &LabOcr) -> Result<MarkOutcome, LabError> {
        record_mark(&self.lock, &request, false, ocr)
    }

    fn mark(&self, request: MarkRequest) -> Result<MarkOutcome, LabError> {
        self.mark_with(request, &self.ocr)
    }

    // Step 1: the primary frame and its samples, the marks and the click.
    fn step_one(
        &self,
        primary: &Gray,
        samples: &[&Gray],
        add: Vec<MarkSpec>,
        click: Option<ClickSpec>,
    ) -> MarkRequest {
        MarkRequest {
            schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
            frame: Some(self.frame("primary", primary)),
            samples: samples
                .iter()
                .enumerate()
                .map(|(index, sample)| self.frame(&format!("sample_{index}"), sample))
                .collect(),
            add,
            click,
            ..MarkRequest::default()
        }
    }

    // The final step: the next screen with its own template mark.
    fn mark_next_step(&self) {
        let frame = self.frame("next", &Gray::next());
        self.mark(MarkRequest {
            schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
            frame: Some(frame),
            add: vec![template("ui/next", rect(40, 46, 12, 12))],
            ..MarkRequest::default()
        })
        .unwrap_or_else(|error| panic!("record mark of the next step: {}", error.code));
    }

    // `record stop` with `ocr`: the `lab` output with its status.
    fn stop_with(&self, ocr: &LabOcr) -> Value {
        let outcome =
            record_stop(&self.lock, &RecordStopOptions::default(), ocr).unwrap_or_else(|error| {
                panic!(
                    "record stop: {}: {} {:?}",
                    error.code, error.message, error.details
                )
            });
        let mut lab = serde_json::to_value(&outcome.lab).expect("lab JSON");
        lab["lab_status"] = json!(outcome.lab_status);
        lab
    }

    fn stop(&self) -> Value {
        self.stop_with(&self.ocr)
    }

    // `record stop`, then the session file stopped as the command leaves it, so the stopped
    // recording takes `record mark` ("Marking after stop").
    fn stop_session(&self) -> Value {
        let lab = self.stop();
        let path = old_record_path(&self.temp.path().join("state"), INSTANCE);
        let mut session: Value =
            serde_json::from_slice(&fs::read(&path).expect("session file")).expect("session");
        session["status"] = json!("stopped");
        fs::write(&path, serde_json::to_vec(&session).expect("session JSON"))
            .expect("session file");
        lab
    }

    // `record status`'s view of the recording.
    fn status(&self) -> Value {
        let status = record_status(&self.temp.path().join("state"), INSTANCE)
            .unwrap_or_else(|error| panic!("record status: {}", error.code))
            .expect("a Lab recording");
        serde_json::to_value(status).expect("status JSON")
    }

    // Step 1 with ALPHA in cell 0 and BETA in cell 1 and the page mark, its click following ALPHA
    // with cells, executed (a point in cell 0); the step is closed. Returns the pressed point.
    fn executed_in_cell_0(&self) -> (i32, i32) {
        self.mark(self.step_one(
            &alpha_and_beta(),
            &[],
            vec![located(&["ALPHA"], true, None), page_mark()],
            Some(follow(None)),
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
        let plan = record_plan_click(&self.lock, &PlanClickRequest::default())
            .unwrap_or_else(|error| panic!("plan click: {}", error.code));
        assert!(inside((plan.point.x, plan.point.y), cell(0)), "{plan:?}");
        record_commit_click(
            &self.lock,
            &plan,
            &CommitClickRequest {
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
            },
        )
        .unwrap_or_else(|error| panic!("commit click: {}", error.code));
        (plan.point.x, plan.point.y)
    }

    // A request on step 1 that removes the followed mark and adds `spec` with its id again.
    fn declare_again(&self, spec: MarkSpec) -> Result<MarkOutcome, LabError> {
        self.mark(MarkRequest {
            schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
            step: Some(1),
            remove: vec![MARK.to_owned()],
            add: vec![spec],
            ..MarkRequest::default()
        })
    }

    // The files of the package `record stop` wrote.
    fn package(&self, lab: &Value) -> BTreeMap<String, Vec<u8>> {
        let path = lab["path"].as_str().expect("package path");
        let bytes = fs::read(path).expect("package file");
        expand_content_container(&bytes, ContentContainer::Zip, ContainmentLimits::default())
            .expect("package container")
    }

    fn recording(&self) -> Value {
        serde_json::from_slice(&self.recording_bytes()).expect("recording")
    }

    fn recording_bytes(&self) -> Vec<u8> {
        fs::read(recording_path(&self.temp.path().join("state"), RECORD_ID))
            .expect("recording file")
    }

    // The package's decision on `images`, the step's frame first, as the daemon admits it.
    fn decision(&self, entries: &BTreeMap<String, Vec<u8>>, images: &[&Gray]) -> OfflineDecision {
        let prepared =
            admit(entries, &self.ocr).unwrap_or_else(|error| panic!("admission: {}", error.code));
        let frames = images
            .iter()
            .map(|image| {
                Frame::from_png(image.png(), CaptureBackendName::AdbScreencap).expect("frame")
            })
            .collect();
        simulate_contained_task(&prepared, frames)
            .expect("simulation")
            .decision
    }

    // The tap the package decides on `images`, the step's frame first.
    fn simulated_tap(&self, entries: &BTreeMap<String, Vec<u8>>, images: &[&Gray]) -> (i32, i32) {
        match self.decision(entries, images) {
            OfflineDecision::WouldClick {
                operation_label,
                action: InputAction::Tap { x, y },
                ..
            } => {
                assert_eq!(operation_label, "step_01_click");
                (x, y)
            }
            other => panic!("not a click: {other:?}"),
        }
    }
}

fn task(entries: &BTreeMap<String, Vec<u8>>) -> Value {
    let bytes = entries
        .get(&format!("resources/operations/{TASK_ID}/task.json"))
        .expect("task.json");
    serde_json::from_slice(bytes).expect("task JSON")
}

fn inside(point: (i32, i32), rect: RecordRect) -> bool {
    point.0 >= rect.x
        && point.1 >= rect.y
        && point.0 < rect.x + rect.width
        && point.1 < rect.y + rect.height
}

// The refused mark of `record_mark_rejected` with `reason`, after nothing was written.
fn rejection(error: LabError, reason: &str) -> Value {
    assert_eq!(error.code, "record_mark_rejected", "{}", error.message);
    let details = error.details.expect("details");
    details["marks"]
        .as_array()
        .and_then(|marks| marks.iter().find(|mark| mark["reason"] == reason))
        .cloned()
        .unwrap_or_else(|| panic!("no {reason} rejection in {details}"))
}

// A self-test refusal of the located-text mark on the primary frame with the kernel's `code`.
fn assert_self_test_refusal(error: LabError, code: RecognitionPackCode) -> Value {
    let refused = rejection(error, "self_test_failed");
    assert_eq!(refused["id"], MARK, "{refused}");
    assert_eq!(refused["frame_id"], "f0001", "{refused}");
    assert_eq!(refused["failure"]["code"], code.as_str(), "{refused}");
    assert_eq!(refused["failure"]["values"]["target_id"], MARK, "{refused}");
    refused
}

// The follow outcome of a refusal or a rejection: the code and its values.
fn assert_follow_outcome(outcome: &Value, problem: ClickFollowProblem) {
    assert_eq!(
        outcome["code"],
        LabCode::RecordClickFollowInvalid.as_str(),
        "{outcome}"
    );
    assert_eq!(
        outcome["values"],
        json!({
            "mark_id": MARK,
            "follow_problem": problem.as_str(),
            "stage": LabLocation::RecordClickFollowCheck.as_str()
        }),
        "{outcome}"
    );
}

// Items 1, 2, 4: a click that follows a located-text mark with cells is the cell holding the
// found name; record stop generates the locate target, the target click, the verify_text guard
// and the fresh-frame recheck, which the daemon's admission takes, and presses inside the cell.
// Fix 1: step 1 has the page mark too (a step whose page has no other mark is refused).
#[test]
fn l2_a_followed_located_text_generates_its_target_click_text_guard_and_recheck() {
    let recording = Recording::new();
    let marked = recording
        .mark(recording.step_one(
            &alpha_in_cell_0(),
            &[],
            vec![located(&["ALPHA"], true, None), page_mark()],
            Some(follow(None)),
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    assert_eq!(marked.marks[0].self_test.status, SelfTestStatus::Passed);
    let click = serde_json::to_value(&marked.click).expect("click JSON");
    assert_eq!(click["follow"], json!({"mark_id": MARK, "form": "target"}));
    assert_eq!(click["rect"], json!(cell(0)));
    assert_eq!(click["source"], "followed_mark");
    recording.mark_next_step();

    let lab = recording.stop();

    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let entries = recording.package(&lab);
    let task = task(&entries);
    let operation = &task["operations"][0];
    assert_eq!(
        operation["click"],
        json!({"kind": "target", "target_id": MARK})
    );
    assert_eq!(
        operation["guard"],
        json!({
            "page_id": "step_01",
            "target_id": MARK,
            "expected_rect": NAMES,
            "verify_text": MARK
        })
    );
    assert_eq!(operation["input_recheck"], json!({"kind": "fresh_frame"}));
    assert_eq!(task["ocr_targets"][0]["id"], MARK);
    assert_eq!(task["ocr_targets"][0]["locate"], locate(true));
    assert!(admit(&entries, &recording.ocr).is_ok(), "admitted");
    let tap = recording.simulated_tap(&entries, &[&alpha_in_cell_0(), &alpha_in_cell_0()]);
    assert!(inside(tap, cell(0)), "{tap:?}");
}

// Item 2: with a rectangle the click follows the found block by the rectangle's offset from the
// block's corner on the primary frame. Fix 1: step 1 has the page mark too.
#[test]
fn l2_a_rectangle_with_a_located_text_follow_is_its_offset_from_the_found_block() {
    let recording = Recording::new();
    let marked = recording
        .mark(recording.step_one(
            &alpha_in_cell_0(),
            &[],
            vec![located(&["ALPHA"], false, None), page_mark()],
            Some(follow(Some(rect(12, 20, 6, 6)))),
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    let click = serde_json::to_value(&marked.click).expect("click JSON");
    assert_eq!(
        click["follow"],
        json!({"mark_id": MARK, "form": "offset", "offset": rect(2, 8, 6, 6)})
    );
    assert_eq!(click["rect"], json!(rect(12, 20, 6, 6)));
    assert_eq!(click["source"], "declared");
    recording.mark_next_step();

    let lab = recording.stop();

    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let entries = recording.package(&lab);
    assert_eq!(
        task(&entries)["operations"][0]["click"],
        json!({"kind": "offset", "target_id": MARK, "offset": rect(2, 8, 6, 6)})
    );
    let tap = recording.simulated_tap(&entries, &[&alpha_in_cell_0(), &alpha_in_cell_0()]);
    assert!(inside(tap, rect(12, 20, 6, 6)), "{tap:?}");
}

// Item 3: a name on no frame is the kernel's text_locate_not_found; nothing is written. Fix 1:
// step 1 has the page mark too.
#[test]
fn l2_a_name_on_no_frame_is_refused_text_locate_not_found() {
    let recording = Recording::new();

    let error = recording
        .mark(recording.step_one(
            &alpha_in_cell_0(),
            &[],
            vec![located(&["BETA"], true, None), page_mark()],
            Some(follow(None)),
        ))
        .expect_err("BETA is on no frame");

    assert_self_test_refusal(error, RecognitionPackCode::TextLocateNotFound);
    assert_eq!(recording.recording()["steps"], json!([]), "nothing written");
}

// Item 3: a found name whose block's center lies between two cells is the kernel's
// text_locate_outside_cells; nothing is written. Fix 1: step 1 has the page mark too.
#[test]
fn l2_a_name_outside_every_cell_is_refused_text_locate_outside_cells() {
    let recording = Recording::new();
    // The block (22, 12, 8, 6) has its center at (26, 15), between cell 0 and cell 1.
    let between = Gray::names(&[(ALPHA, 22, 12)]);

    let error = recording
        .mark(recording.step_one(
            &between,
            &[],
            vec![located(&["ALPHA"], true, None), page_mark()],
            Some(follow(None)),
        ))
        .expect_err("the hit lies in no cell");

    assert_self_test_refusal(error, RecognitionPackCode::TextLocateOutsideCells);
    assert_eq!(recording.recording()["steps"], json!([]), "nothing written");
}

// Item 3: a second matching block within the declared margin is the kernel's
// target_match_not_unique, with the second place; nothing is written. Fix 1: step 1 has the page
// mark too.
#[test]
fn l2_two_matches_within_the_margin_are_refused_target_match_not_unique() {
    let recording = Recording::new();
    let twice = Gray::names(&[(ALPHA, 10, 12), (ALPHA, 50, 12)]);

    let error = recording
        .mark(recording.step_one(
            &twice,
            &[],
            vec![located(&["ALPHA"], false, Some(50)), page_mark()],
            Some(follow(None)),
        ))
        .expect_err("a second ALPHA lies within the margin");

    let refused = assert_self_test_refusal(error, RecognitionPackCode::TargetMatchNotUnique);
    let uniqueness = &refused["uniqueness"];
    assert_eq!(uniqueness["margin_milli"], 50, "{refused}");
    assert_eq!(
        uniqueness["second_score_milli"], uniqueness["score_milli"],
        "{refused}"
    );
    assert_eq!(
        uniqueness["second_rect"],
        json!(rect(50, 12, 8, 6)),
        "{refused}"
    );
    assert_eq!(recording.recording()["steps"], json!([]), "nothing written");
}

// Item 3: on a sample frame where the name stands at the region's right edge, the click box (the
// found block plus the offset) leaves the frame: click_outside_frame, naming the sample. Fix 1:
// step 1 has the page mark too.
#[test]
fn l2_a_click_box_that_leaves_a_sample_frame_is_click_outside_frame() {
    let recording = Recording::new();
    let edge = Gray::names(&[(ALPHA, 80, 12)]);

    let error = recording
        .mark(recording.step_one(
            &alpha_in_cell_0(),
            &[&edge],
            vec![located(&["ALPHA"], false, None), page_mark()],
            Some(follow(Some(rect(20, 12, 10, 8)))),
        ))
        .expect_err("the click box leaves the sample frame");

    let refused = rejection(error, LabCode::RecordClickFollowInvalid.as_str());
    assert_eq!(refused["id"], MARK, "{refused}");
    assert_eq!(refused["frame_id"], "f0002", "{refused}");
    assert_eq!(
        refused["click_rect"],
        json!(rect(90, 12, 10, 8)),
        "{refused}"
    );
    assert_follow_outcome(&refused["outcome"], ClickFollowProblem::ClickOutsideFrame);
    assert_eq!(recording.recording()["steps"], json!([]), "nothing written");
}

// Item 3 and the ruling on the names: a step whose click was executed in cell 0 cannot take a
// name the primary frame shows in cell 1 when the followed mark, declared again by the same
// request, also changes another member: press_outside_found_box, and nothing is written. Fix 1:
// the request also widens the cells (18 to 19 pixels), since a request that changes only the names
// is no longer held to the press; step 1 has the page mark too.
#[test]
fn l2_an_executed_press_outside_the_found_box_is_press_outside_found_box() {
    let recording = Recording::new();
    recording.executed_in_cell_0();
    let before = recording.recording_bytes();
    let mut wider = located(&["BETA"], true, None);
    let grid = wider
        .locate
        .as_mut()
        .and_then(|locate| locate.cells.as_mut())
        .expect("cells");
    grid.width = 19;

    let error = recording
        .declare_again(wider)
        .expect_err("the executed press lies in cell 0, BETA in cell 1");

    let refused = rejection(error, LabCode::RecordClickFollowInvalid.as_str());
    assert_eq!(refused["id"], MARK, "{refused}");
    assert_eq!(refused["frame_id"], "f0001", "{refused}");
    // Cell 1 of the wider grid: x 8 + 20, width 19.
    assert_eq!(
        refused["click_rect"],
        json!(rect(28, 8, 19, 24)),
        "{refused}"
    );
    assert_follow_outcome(
        &refused["outcome"],
        ClickFollowProblem::PressOutsideFoundBox,
    );
    assert_eq!(recording.recording_bytes(), before, "nothing written");
}

// Item 4 and the point of the form: a sample on which the name stands in another cell than on
// the primary frame is accepted, and the package presses that cell there; with the step's frame
// first and that sample as the recheck's fresh frame, the press is drawn from the fresh frame.
// Fix 1: step 1 has the page mark too.
#[test]
fn l2_a_sample_with_the_name_in_another_cell_is_accepted_and_the_click_follows_it() {
    let recording = Recording::new();
    let moved = Gray::names(&[(ALPHA, 50, 12)]);
    recording
        .mark(recording.step_one(
            &alpha_in_cell_0(),
            &[&moved],
            vec![located(&["ALPHA"], true, None), page_mark()],
            Some(follow(None)),
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    recording.mark_next_step();

    let lab = recording.stop();

    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let entries = recording.package(&lab);
    let there = recording.simulated_tap(&entries, &[&moved, &moved]);
    assert!(inside(there, cell(2)), "{there:?}");
    let fresh = recording.simulated_tap(&entries, &[&alpha_in_cell_0(), &moved]);
    assert!(inside(fresh, cell(2)), "{fresh:?}");
    assert!(!inside(fresh, cell(0)), "{fresh:?}");
}

// The follow rules: a mark that is neither a template nor a located-text mark is not_template, a
// located-text mark only an any_of check names is not_required, another guard is guard_conflict;
// each refuses the command and writes nothing.
#[test]
fn l2_the_follow_rules_hold_for_a_located_text_mark() {
    let recording = Recording::new();
    let before = recording.recording_bytes();
    let plain = MarkSpec {
        locate: None,
        ..located(&["ALPHA"], false, None)
    };
    let either = MarkSpec {
        region: None,
        any_of: Some(vec![MARK.to_owned(), "ui/other".to_owned()]),
        ..MarkSpec::region_only(
            "check/either".to_owned(),
            MarkFamily::Check,
            rect(0, 0, 1, 1),
        )
    };
    let color = MarkSpec::region_only(
        "state/color".to_owned(),
        MarkFamily::Color,
        rect(70, 40, 4, 4),
    );
    let mut guarded = recording.step_one(
        &alpha_in_cell_0(),
        &[],
        vec![located(&["ALPHA"], true, None), color],
        Some(follow(None)),
    );
    guarded.click_guard = Some("state/color".to_owned());
    let cases = [
        (
            recording.step_one(&alpha_in_cell_0(), &[], vec![plain], Some(follow(None))),
            ClickFollowProblem::NotTemplate,
        ),
        (
            recording.step_one(
                &alpha_in_cell_0(),
                &[],
                vec![
                    located(&["ALPHA"], true, None),
                    template("ui/other", rect(70, 40, 12, 12)),
                    either,
                ],
                Some(follow(None)),
            ),
            ClickFollowProblem::NotRequired,
        ),
        (guarded, ClickFollowProblem::GuardConflict),
    ];

    for (request, problem) in cases {
        let error = recording.mark(request).expect_err("the follow cannot hold");

        assert_eq!(
            error.code,
            LabCode::RecordClickFollowInvalid.as_str(),
            "{}",
            error.message
        );
        assert_follow_outcome(&error.details.expect("details")["outcome"], problem);
        assert_eq!(recording.recording_bytes(), before, "nothing written");
    }
}

// Item 5: a recording with an ordinary OCR mark and a rectangle generates the OCR target, the
// click step and its guard as before: no locate, no text guard, no recheck in any file.
#[test]
fn l2_without_a_located_text_mark_the_package_is_generated_as_before() {
    let recording = Recording::new();
    let button = rect(60, 40, 12, 12);
    let mut noise = Noise(0x0463_0001);
    let patch = Gray::from_fn(12, 12, |_, _| noise.between(1, 199));
    let mut primary = alpha_in_cell_0();
    for row in 0..12 {
        for column in 0..12 {
            primary.values[((40 + row) * WIDTH + 60 + column) as usize] =
                patch.values[(row * 12 + column) as usize];
        }
    }
    let plain = MarkSpec {
        id: "text/plain".to_owned(),
        locate: None,
        match_mode: Some("contains".to_owned()),
        ..located(&["ALPHA"], false, None)
    };
    let marked = recording
        .mark(recording.step_one(
            &primary,
            &[],
            vec![template("ui/button", button), plain],
            Some(ClickSpec {
                region: Some(rect(62, 42, 4, 4)),
                ..ClickSpec::default()
            }),
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    let frame_sha256 = marked.frame.expect("primary frame").sha256;
    recording.mark_next_step();

    let lab = recording.stop();

    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let entries = recording.package(&lab);
    let task = task(&entries);
    assert_eq!(
        task["ocr_targets"],
        json!([{
            "id": "text/plain",
            "region": {"mode": "rect", "rect": NAMES},
            "languages": ["en"],
            "timeout_ms": 1000,
            "match_mode": "contains",
            "expected": ["ALPHA"],
            "case_sensitive": false,
            "minimum_confidence": 0.8,
            "model_ref": "neutral-ocr",
            "model_sha256": MODEL_SHA256
        }])
    );
    assert_eq!(
        task["operations"][0],
        json!({
            "id": "step_01_click",
            "purpose": "Lab recording step 1",
            "from": "step_01",
            "to": "step_02",
            "click": {"kind": "rect", "x": 62, "y": 42, "width": 4, "height": 4},
            "guard": {
                "page_id": "step_01",
                "target_id": "ui/button",
                "expected_rect": button,
                "verify_template": "assets/ui_button.png"
            },
            "expect_after": {"page_id": "step_02", "timeout_ms": 15000, "interval_ms": 500},
            "post_delay_ms": 200,
            "provenance": {
                "source": "lab_record",
                "record_id": RECORD_ID,
                "step": 1,
                "record_index": 1,
                "frame_sha256": frame_sha256
            }
        })
    );
    for (path, bytes) in &entries {
        let text = String::from_utf8_lossy(bytes);
        for member in [
            "\"locate\"",
            "\"verify_text\"",
            "\"input_recheck\"",
            "\"follow\"",
        ] {
            assert!(!text.contains(member), "{path} names {member}");
        }
    }
}

// OCR unavailable: a located-text mark is recorded not evaluated, as an OCR mark is; a click
// cannot follow it without the OCR that finds its box (lab_ocr_unavailable, nothing written).
// Fix 1: step 1 has the page mark too, so the follow's declaration holds and the OCR is what
// refuses it.
#[test]
fn l2_without_an_ocr_model_the_mark_is_not_evaluated_and_cannot_be_followed() {
    let recording = Recording::new();
    let marked = recording
        .mark_with(
            recording.step_one(
                &alpha_in_cell_0(),
                &[],
                vec![located(&["ALPHA"], true, None), page_mark()],
                None,
            ),
            &recording.none,
        )
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    let test = &marked.marks[0].self_test;
    assert_eq!(test.status, SelfTestStatus::NotEvaluated);
    assert_eq!(
        test.reason.as_deref(),
        Some(LabCode::LabOcrUnavailable.as_str())
    );
    let before = recording.recording_bytes();

    let error = recording
        .mark_with(
            MarkRequest {
                schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
                step: Some(1),
                click: Some(follow(None)),
                ..MarkRequest::default()
            },
            &recording.none,
        )
        .expect_err("the click box needs the OCR");

    assert_eq!(
        error.code,
        LabCode::LabOcrUnavailable.as_str(),
        "{}",
        error.message
    );
    assert_eq!(
        error.details.expect("details")["outcome"]["values"]["target_id"],
        MARK
    );
    assert_eq!(recording.recording_bytes(), before, "nothing written");
}

// OCR unavailable at record stop: the package is generated with its locate target, text guard
// and recheck, and its first decision is not evaluated (lab_ocr_unavailable), as for an OCR mark.
// Fix 1: step 1 has the page mark too; its page no longer reads OCR, its guard does.
#[test]
fn l2_record_stop_without_an_ocr_model_reports_the_first_decision_not_evaluated() {
    let recording = Recording::new();
    recording
        .mark(recording.step_one(
            &alpha_in_cell_0(),
            &[],
            vec![located(&["ALPHA"], true, None), page_mark()],
            Some(follow(None)),
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    recording.mark_next_step();

    let lab = recording.stop_with(&recording.none);

    assert_eq!(lab["lab_status"], "generated", "{lab}");
    assert_eq!(
        lab["first_decision"],
        json!({"status": "not_evaluated", "reason": LabCode::LabOcrUnavailable.as_str()})
    );
    assert!(
        lab["warnings"]
            .as_array()
            .is_some_and(|warnings| warnings.iter().any(|warning| {
                warning["code"] == "first_decision_not_evaluated"
                    && warning["reason"] == LabCode::LabOcrUnavailable.as_str()
            })),
        "{lab}"
    );
    let operation = task(&recording.package(&lab))["operations"][0].clone();
    assert_eq!(operation["guard"]["verify_text"], MARK);
    assert_eq!(operation["input_recheck"], json!({"kind": "fresh_frame"}));
}

// Fix 1, item 1: the followed located-text mark is the target of the click and of the guard and of
// no page rule; step 1's page stands on the page mark. On a frame without the name the admitted
// package recognizes step 1's page and its guard refuses with the typed cause, no input; on a fresh
// frame without the name after the step's frame the same.
#[test]
fn l2_fix1_the_followed_located_text_guards_the_click_and_is_no_mark_of_the_page() {
    let recording = Recording::new();
    recording
        .mark(recording.step_one(
            &alpha_in_cell_0(),
            &[],
            vec![located(&["ALPHA"], true, None), page_mark()],
            Some(follow(None)),
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    recording.mark_next_step();

    let lab = recording.stop();

    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let entries = recording.package(&lab);
    let task = task(&entries);
    let operation = &task["operations"][0];
    assert_eq!(operation["click"]["target_id"], MARK, "{operation}");
    assert_eq!(operation["guard"]["target_id"], MARK, "{operation}");
    assert_eq!(operation["guard"]["verify_text"], MARK, "{operation}");
    assert_eq!(task["ocr_targets"][0]["id"], MARK);
    assert_eq!(task["page_rules"]["step_01"], json!({"required": [PAGE]}));
    let rules = task["page_rules"].as_object().expect("page rules");
    for (page, rule) in rules {
        for members in rule.as_object().expect("a page rule").values() {
            let listed = members.as_array().expect("a list of targets");
            assert!(
                !listed.contains(&json!(MARK)),
                "{page} names {MARK}: {rule}"
            );
        }
    }
    assert_guard_refused(
        &recording.decision(&entries, &[&no_name()]),
        RecognitionPackCode::TextLocateNotFound,
    );
    assert_guard_refused(
        &recording.decision(&entries, &[&alpha_in_cell_0(), &no_name()]),
        RecognitionPackCode::TextLocateNotFound,
    );
}

// Fix 1, item 1: a click that follows a located-text mark needs another mark that recognizes the
// step's page. The located-text mark alone, and a check of the page that names it, are refused
// page_needs_followed_text; so is a later record mark that removes the step's only other mark.
// Nothing is written.
#[test]
fn l2_fix1_a_page_that_needs_the_followed_text_is_refused_page_needs_followed_text() {
    let recording = Recording::new();
    let both = MarkSpec {
        region: None,
        all_of: Some(vec![MARK.to_owned(), PAGE.to_owned()]),
        ..MarkSpec::region_only("check/both".to_owned(), MarkFamily::Check, rect(0, 0, 1, 1))
    };
    let alone = recording.step_one(
        &alpha_in_cell_0(),
        &[],
        vec![located(&["ALPHA"], true, None)],
        Some(follow(None)),
    );
    let checked = recording.step_one(
        &alpha_in_cell_0(),
        &[],
        vec![located(&["ALPHA"], true, None), page_mark(), both],
        Some(follow(None)),
    );
    for request in [alone, checked] {
        let error = recording
            .mark(request)
            .expect_err("the page would need the name");

        assert_eq!(
            error.code,
            LabCode::RecordClickFollowInvalid.as_str(),
            "{}",
            error.message
        );
        assert_follow_outcome(
            &error.details.expect("details")["outcome"],
            ClickFollowProblem::PageNeedsFollowedText,
        );
        assert_eq!(recording.recording()["steps"], json!([]), "nothing written");
    }
    recording
        .mark(recording.step_one(
            &alpha_in_cell_0(),
            &[],
            vec![located(&["ALPHA"], true, None), page_mark()],
            Some(follow(None)),
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    let before = recording.recording_bytes();

    let error = recording
        .mark(MarkRequest {
            schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
            step: Some(1),
            remove: vec![PAGE.to_owned()],
            ..MarkRequest::default()
        })
        .expect_err("the page mark is the step's only other mark");

    assert_eq!(
        error.code,
        LabCode::RecordClickFollowInvalid.as_str(),
        "{}",
        error.message
    );
    assert_follow_outcome(
        &error.details.expect("details")["outcome"],
        ClickFollowProblem::PageNeedsFollowedText,
    );
    assert_eq!(recording.recording_bytes(), before, "nothing written");
}

// Fix 1, item 2: after the press in cell 0 and record stop, the request that removes the followed
// mark and adds it again with only its names changed (BETA, which the recorded frame shows in
// cell 1) is accepted without the press check; the click's follow remembers it, the answer and
// record status show it, and the next record stop regenerates the package, whose click follows
// BETA there, its cross-check naming the step as judged by its changed names and passing.
#[test]
fn l2_fix1_names_changed_after_the_press_are_accepted_remembered_and_regenerated() {
    let recording = Recording::new();
    let pressed = recording.executed_in_cell_0();
    let point = json!({"x": pressed.0, "y": pressed.1});
    recording.mark_next_step();
    let generated = recording.stop_session();
    assert_eq!(generated["lab_status"], "generated", "{generated}");
    assert!(
        generated["cross_check"]
            .get("names_changed_after_press")
            .is_none(),
        "{generated}"
    );

    let marked = recording
        .declare_again(located(&["BETA"], true, None))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));

    assert!(marked.after_stop);
    let click = serde_json::to_value(&marked.click).expect("click JSON");
    assert_eq!(
        click["follow"],
        json!({"mark_id": MARK, "form": "target", "names_changed_after_press": true}),
        "{click}"
    );
    assert_eq!(click["rect"], json!(cell(1)), "{click}");
    assert_eq!(click["execution"]["point"], point, "{click}");
    let status = recording.status();
    assert_eq!(
        status["steps"][0]["click"]["names_changed_after_press"], true,
        "{status}"
    );

    let regenerated = recording.stop();

    assert_eq!(regenerated["lab_status"], "regenerated", "{regenerated}");
    let entries = recording.package(&regenerated);
    let task = task(&entries);
    assert_eq!(task["ocr_targets"][0]["expected"], json!(["BETA"]));
    assert_eq!(
        task["operations"][0]["click"],
        json!({"kind": "target", "target_id": MARK})
    );
    let tap = recording.simulated_tap(&entries, &[&alpha_and_beta(), &alpha_and_beta()]);
    assert!(inside(tap, cell(1)), "{tap:?}");
    let cross_check = &regenerated["cross_check"];
    assert_eq!(cross_check["status"], "passed", "{cross_check}");
    assert_eq!(
        cross_check["names_changed_after_press"],
        json!([{"step": 1, "record_index": 1, "mark_id": MARK, "recorded_point": point}]),
        "{cross_check}"
    );
}

// Fix 1, item 2: after the press, a request that changes only the names to one that no recorded
// frame shows is still the mark's self-test refusal text_locate_not_found; nothing is written.
#[test]
fn l2_fix1_after_the_press_a_name_on_no_frame_is_still_refused_text_locate_not_found() {
    let recording = Recording::new();
    recording.executed_in_cell_0();
    let before = recording.recording_bytes();

    let error = recording
        .declare_again(located(&["GAMMA"], true, None))
        .expect_err("GAMMA is on no frame");

    assert_self_test_refusal(error, RecognitionPackCode::TextLocateNotFound);
    assert_eq!(recording.recording_bytes(), before, "nothing written");
}

// Fix 1, item 3: --click-guard may name the followed located-text mark, the guard the package has
// anyway; naming another OCR mark is still refused record_guard_family_invalid, its message naming
// the followed mark as the guard, and nothing is written.
#[test]
fn l2_fix1_the_click_guard_may_name_the_followed_located_text_mark_only() {
    let refused = Recording::new();
    let plain = MarkSpec {
        id: "text/plain".to_owned(),
        locate: None,
        ..located(&["ALPHA"], false, None)
    };
    let mut other = refused.step_one(
        &alpha_in_cell_0(),
        &[],
        vec![located(&["ALPHA"], true, None), plain, page_mark()],
        Some(follow(None)),
    );
    other.click_guard = Some("text/plain".to_owned());

    let error = refused
        .mark(other)
        .expect_err("another OCR mark is no guard");

    assert_eq!(
        error.code, "record_guard_family_invalid",
        "{}",
        error.message
    );
    assert!(
        error
            .message
            .contains(&format!("this click follows '{MARK}', which is its guard")),
        "{}",
        error.message
    );
    assert_eq!(refused.recording()["steps"], json!([]), "nothing written");

    let recording = Recording::new();
    let mut named = recording.step_one(
        &alpha_in_cell_0(),
        &[],
        vec![located(&["ALPHA"], true, None), page_mark()],
        Some(follow(None)),
    );
    named.click_guard = Some(MARK.to_owned());

    recording
        .mark(named)
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));

    assert_eq!(recording.recording()["steps"][0]["click_guard"], MARK);
    recording.mark_next_step();
    let lab = recording.stop();
    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let operation = task(&recording.package(&lab))["operations"][0].clone();
    assert_eq!(operation["guard"]["target_id"], MARK, "{operation}");
    assert_eq!(operation["guard"]["verify_text"], MARK, "{operation}");
}
