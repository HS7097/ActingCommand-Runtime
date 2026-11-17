// SPDX-License-Identifier: AGPL-3.0-only

// #463 SC-1 (contracts/lab-recording.md, "Selecting in a list", the mark-time self-test, and
// "Self-checks", item 8; register §1.469; brief rd-1118/brief-463-sc1-select-uniqueness.md): a
// select step's page is checked for uniqueness on every frame of the recording. The run's select
// confirms on a fresh frame that exactly one page of the whole package is recognised there
// (`contained_task_recognition_conflict` when two are); so where the select step's page passes on
// another step's frame beside that step's page, `record mark` (on the frames of the steps it knows)
// and `record stop` and `record ready` (on every other live frame of the recording) refuse it with
// `select_page_passes_on_other_frame`, naming the select step, the frame and whose it is, and both
// pages; the other way round, another page on the select step's own frames, is refused as before.
// A pair of pages neither of which is a select step's stays the warning it was.
//
// Frames are synthetic: 96x64 pixels of flat gray 60. The scripted OCR provider reads one block
// per marker pixel inside the requested region, its text chosen by the marker's gray value, at
// confidence 0.9; a region without a marker reads nothing. Slot 0 is (8, 8, 32, 32) with its name
// read in (10, 28, 28, 8), slot 1 (48, 8, 32, 32) with (50, 28, 28, 8); a name's marker stands at
// (12 + 40k, 30). Every page is recognised by colour marks on 4x4 areas of row 50: the list's
// `state/page` at x 2 (gray 60 on the list), the busy slot's `ui/busy` at x 60, `ui/other` at x 20,
// `ui/final` at x 80 and a pop-up's or an ending's mark at x 40, each gray 150 where it is shown; a
// screen that is not the list shows gray 170 at x 2. No device is used; the tests read no clock
// and never sleep. Every expected value comes from the brief's lines and the values above.

use crate::codes::LabCode;
use crate::recording::store::{old_record_path, recording_path};
use crate::{
    ClickEffect, CommitClickRequest, LAB_RECORD_MARK_SCHEMA, LabOcr, MarkOutcome, MarkRequest,
    PlanClickRequest, RecordRect, RecordStartOptions, RecordStopOptions, RecordingLock,
    record_commit_click, record_mark, record_plan_click, record_ready, record_start, record_stop,
};
use actingcommand_contract::LabError;
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_recognition_pack::{
    NnProviderRequest, NnProviderResult, OcrProviderRequest, OcrProviderResult,
    OcrProviderTextBlock, PackRect, VisionProvider, VisionProviderError, VisionProviderErrorCode,
};
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_0463_sc1";
const TASK_ID: &str = "neutral_task";
const WIDTH: u32 = 96;
const HEIGHT: u32 = 64;
const BACKGROUND: u8 = 60;
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
// The word of this slice, and the refusal `record stop` answers it with.
const PASSES_ELSEWHERE: &str = "select_page_passes_on_other_frame";
const SELF_MISMATCH: &str = "record_step_self_mismatch";
// The gray of a mark's area where its screen shows it, and of the list's page area on every screen
// that is not the list.
const SHOWN: u8 = 150;
const ELSEWHERE: u8 = 170;

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
// The marks' areas and ids.
const PAGE_AREA: RecordRect = rect(2, 50, 4, 4);
const OTHER_AREA: RecordRect = rect(20, 50, 4, 4);
const MORE_AREA: RecordRect = rect(40, 50, 4, 4);
const BUSY_AREA: RecordRect = rect(60, 50, 4, 4);
const FINAL_AREA: RecordRect = rect(80, 50, 4, 4);
const PAGE: &str = "state/page";
const BUSY: &str = "ui/busy";
const OTHER: &str = "ui/other";
const FINAL: &str = "ui/final";
// A pop-up's close button, pressed on each copy of it.
const CLOSE: RecordRect = rect(60, 20, 16, 12);

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

// The PNG of a gray screen: gray 60 with each area of `painted` in its gray and, in slot `k`, the
// marker given for it (`None`: no name).
fn screen(names: [Option<u8>; 2], painted: &[(RecordRect, u8)]) -> Vec<u8> {
    let mut values = vec![BACKGROUND; (WIDTH * HEIGHT) as usize];
    for (area, gray) in painted {
        for row in area.y..area.y + area.height {
            for column in area.x..area.x + area.width {
                values[(row as u32 * WIDTH + column as u32) as usize] = *gray;
            }
        }
    }
    for (slot, marker) in (0_u32..).zip(names) {
        if let Some(marker) = marker {
            values[(30 * WIDTH + 12 + 40 * slot) as usize] = marker;
        }
    }
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

// The list with both slots free: item_a's name in slot 0, item_b's in slot 1, gray 60 everywhere
// else, its page area among it.
fn list() -> Vec<u8> {
    screen([Some(ALPHA), Some(BRAVO)], &[])
}

// The list after the job began: the same slots and page area, a slot busy (`ui/busy` shown).
fn busy_list() -> Vec<u8> {
    screen([Some(ALPHA), Some(BRAVO)], &[(BUSY_AREA, SHOWN)])
}

// A screen that is not the list: no names, its page area gray 170, the areas of `shown` shown.
fn other_screen(shown: &[RecordRect]) -> Vec<u8> {
    let mut painted = vec![(PAGE_AREA, ELSEWHERE)];
    painted.extend(shown.iter().map(|area| (*area, SHOWN)));
    screen([None, None], &painted)
}

// A colour mark on `area`.
fn color(id: &str, area: RecordRect) -> Value {
    json!({"id": id, "family": "color", "region": area})
}

// The select over the two slots with the wanted value's `default` and the name pressed.
fn select(pressed: &str, default: &str) -> Value {
    json!({
        "slots": [{"rect": SLOTS[0], "name": NAMES[0]}, {"rect": SLOTS[1], "name": NAMES[1]}],
        "read": {"languages": ["en"], "timeout_ms": 1000, "minimum_confidence_milli": 800,
                 "model_ref": "neutral-ocr", "model_sha256": MODEL_SHA256},
        "names": [{"id": "item_a", "aliases": ["ALPHA"]}, {"id": "item_b", "aliases": ["BRAVO"]}],
        "match": {"max_distance": 1, "minimum_margin": 1},
        "wanted": {"name": "list_item", "default": default},
        "pressed": pressed
    })
}

fn request(mut value: Value) -> MarkRequest {
    value["schema_version"] = json!(LAB_RECORD_MARK_SCHEMA);
    serde_json::from_value(value).expect("a record mark request")
}

fn performed() -> CommitClickRequest {
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

// The rejection of the select with `reason` in a `record_mark_rejected` refusal, as its marks list
// it.
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

// The warnings of `warnings` with the code `code`.
fn warnings_of(warnings: &Value, code: &str) -> Vec<Value> {
    warnings
        .as_array()
        .unwrap_or_else(|| panic!("no warnings in {warnings}"))
        .iter()
        .filter(|warning| warning["code"] == code)
        .cloned()
        .collect()
}

// `warnings` without each one's message, which it must carry.
fn without_message(warnings: Vec<Value>) -> Vec<Value> {
    warnings
        .into_iter()
        .map(|mut warning| {
            warning
                .as_object_mut()
                .expect("a warning")
                .remove("message")
                .expect("its message");
            warning
        })
        .collect()
}

// A state directory with an active session of `INSTANCE` and its Lab recording, before any step,
// with the scripted OCR provider. The lock is dropped before the directory.
struct Recording {
    lock: RecordingLock,
    ocr: LabOcr,
    temp: TempDir,
}

impl Recording {
    fn new() -> Self {
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
                "started_at_unix_ms": 1_700_000_000_000_u64
            }))
            .expect("session JSON"),
        )
        .expect("session file");
        let lock = RecordingLock::acquire(&state, INSTANCE, "gate_463_sc1", "Lab-CLI")
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
        Self {
            lock,
            ocr: LabOcr::with_provider(Arc::new(NamesOcr)),
            temp,
        }
    }

    fn state(&self) -> PathBuf {
        self.temp.path().join("state")
    }

    // Writes `bytes` as the frame file `name` on the caller's side and returns its path.
    fn frame(&self, name: &str, bytes: &[u8]) -> String {
        let path = self.temp.path().join(format!("{name}.png"));
        fs::write(&path, bytes).expect("frame file");
        path.display().to_string()
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

    // The request `value` of a new step on the screen `bytes` (the frame file `name`).
    fn on(&self, name: &str, bytes: &[u8], mut value: Value) -> Value {
        value["frame"] = json!(self.frame(name, bytes));
        value
    }

    // The click of the open step planned and committed as performed, which closes the step.
    fn press_open_step(&self) {
        let plan = record_plan_click(&self.lock, &PlanClickRequest::default())
            .unwrap_or_else(|error| panic!("plan click: {}", error.code));
        record_commit_click(&self.lock, &plan, &performed())
            .unwrap_or_else(|error| panic!("commit click: {}", error.code));
    }

    // `record stop`: the `lab` output with its status, or the refusal.
    fn stop(&self) -> Result<Value, LabError> {
        record_stop(&self.lock, &RecordStopOptions::default(), &self.ocr).map(|outcome| {
            let mut lab = serde_json::to_value(&outcome.lab).expect("lab JSON");
            lab["lab_status"] = json!(outcome.lab_status);
            lab
        })
    }

    // `record stop`, which must generate.
    fn stopped(&self) -> Value {
        self.stop().unwrap_or_else(|error| {
            panic!(
                "record stop: {}: {} {:?}",
                error.code, error.message, error.details
            )
        })
    }

    // `record stop`, then the session file stopped as the command leaves it, so that the stopped
    // recording takes `record mark` ("Marking after stop").
    fn stop_session(&self) -> Value {
        let lab = self.stopped();
        let path = old_record_path(&self.state(), INSTANCE);
        let mut session: Value =
            serde_json::from_slice(&fs::read(&path).expect("session file")).expect("session");
        session["status"] = json!("stopped");
        fs::write(&path, serde_json::to_vec(&session).expect("session JSON"))
            .expect("session file");
        lab
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

    fn recording_bytes(&self) -> Vec<u8> {
        fs::read(recording_path(&self.state(), RECORD_ID)).expect("recording file")
    }

    // Step 1, the select on the list with item_a pressed (slot 0) and item_b its default, pressed.
    fn select_on_the_list(&self) {
        self.marked(self.on(
            "list",
            &list(),
            json!({"add": [color(PAGE, PAGE_AREA)], "select": select("item_a", "item_b")}),
        ));
        self.press_open_step();
    }

    // The recording the observation found: step 1 the select on the list, step 2 the list after
    // the job began, its page the busy slot; the list's page passes on step 2's frame f0002.
    fn observed() -> Self {
        let recording = Self::new();
        recording.select_on_the_list();
        recording.marked(recording.on(
            "busy",
            &busy_list(),
            json!({"add": [color(BUSY, BUSY_AREA)]}),
        ));
        recording
    }
}

// The work order's test 1, the observed case: the select step's own frame is clean (no other page
// passes on it), and on step 2's frame f0002 the select step's page passes beside step 2's.
// `record ready` lists it as the self-check `selects`' one item, naming the select step, its page
// and the frame; `record stop` refuses `record_step_self_mismatch` with the word, naming the
// select step and its page, the frame and whose it is, the other page and whose it is; nothing is
// written.
#[test]
fn sc1_a_select_page_passing_beside_a_later_steps_page_is_listed_and_refuses_record_stop() {
    let recording = Recording::observed();

    let ready = recording.ready();
    assert_eq!(ready["status"], "draft", "{ready}");
    let missing = ready["missing"].as_array().expect("missing");
    assert_eq!(missing.len(), 1, "{ready}");
    let item = &missing[0];
    assert_eq!(item["what"], "selects", "{item}");
    assert_eq!(item["code"], SELF_MISMATCH, "{item}");
    assert_eq!(item["step"], 1, "{item}");
    assert_eq!(item["details"]["page"], "step_01", "{item}");
    assert_eq!(item["details"]["frames"], json!(["f0002"]), "{item}");

    let before = recording.recording_bytes();
    let error = recording
        .stop()
        .expect_err("the select's page is not alone on step 2's frame");
    assert_eq!(error.code, SELF_MISMATCH, "{}", error.message);
    let details = error.details.expect("details");
    for (member, value) in [
        ("step", json!(1)),
        ("record_index", json!(1)),
        ("frame_id", json!("f0002")),
        ("reason", json!(PASSES_ELSEWHERE)),
        ("page", json!("step_01")),
        ("other_page", json!("step_02")),
        ("other_step", json!(2)),
        ("other_record_index", json!(2)),
        ("other_transition", json!(false)),
        ("frame_step", json!(2)),
        ("frame_record_index", json!(2)),
        ("frame_transition", json!(false)),
    ] {
        assert_eq!(details[member], value, "{member}: {details}");
    }
    assert_eq!(recording.recording_bytes(), before, "nothing written");
}

// The work order's test 1, the same recording told apart. A mark of the later step's page that the
// select step's screen lacks (`ui/busy`) does not by itself take the select step's page off step
// 2's frame, where it still passes beside step 2's; forbidden on the select step's page (`record
// mark --step 1 --forbid ui/busy`), it does: the mark stands, `record ready` is ready and `record
// stop` generates.
#[test]
fn sc1_the_select_page_forbidding_the_later_steps_mark_passes() {
    let recording = Recording::observed();

    recording.marked(json!({"step": 1, "forbid": [BUSY]}));

    let ready = recording.ready();
    assert_eq!(ready["status"], "ready", "{ready}");
    assert_eq!(ready["missing"], json!([]), "{ready}");
    let lab = recording.stopped();
    assert_eq!(lab["lab_status"], "generated", "{lab}");
}

// The work order's test 1 at `record mark`: the recording with step 1 a plain press on the list and
// step 2 the busy list generates; on the stopped recording, which the mark knows whole, a select
// declared on step 1 is refused with the word, naming the select step, the frame f0002 and whose it
// is, and step 2's page; nothing is written.
#[test]
fn sc1_a_select_declared_after_the_stop_on_such_a_list_is_refused_at_the_mark() {
    let recording = Recording::new();
    recording.marked(recording.on(
        "list",
        &list(),
        json!({"add": [color(PAGE, PAGE_AREA)], "click": {"region": SLOTS[0]}}),
    ));
    recording.press_open_step();
    recording.marked(recording.on(
        "busy",
        &busy_list(),
        json!({"add": [color(BUSY, BUSY_AREA)]}),
    ));
    let lab = recording.stop_session();
    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let before = recording.recording_bytes();

    let error = recording
        .mark(json!({"step": 1, "select": select("item_a", "item_b")}))
        .expect_err("the list's page passes on step 2's frame beside step 2's page");

    let refused = select_rejection(error, PASSES_ELSEWHERE);
    assert_eq!(
        refused,
        json!({"id": "select", "reason": PASSES_ELSEWHERE, "frame_id": "f0002",
               "select_step": 1, "step": 2, "transition": false,
               "frame_step": 2, "frame_transition": false}),
        "{refused}"
    );
    assert_eq!(recording.recording_bytes(), before, "nothing written");
}

// The work order's test 2, today's direction as today: step 1 a press on a screen of its own, step
// 2 the select on the list, step 3 a screen whose page (`ui/final`, gray 60 at x 80) passes on the
// list too and on which the list's page does not. `record stop` answers the select step's own
// frame first, as before: `record_step_self_mismatch` with `self_test_error` and the kernel's
// `select_page_mismatch` on f0002; `record ready` lists that one item and nothing the other way
// round.
#[test]
fn sc1_another_page_on_the_select_steps_own_frame_refuses_as_before() {
    let recording = Recording::new();
    recording.marked(recording.on(
        "start",
        &screen(
            [None, None],
            &[
                (PAGE_AREA, ELSEWHERE),
                (OTHER_AREA, SHOWN),
                (FINAL_AREA, ELSEWHERE),
            ],
        ),
        json!({"add": [color(OTHER, OTHER_AREA)], "click": {"region": SLOTS[0]}}),
    ));
    recording.press_open_step();
    recording.marked(recording.on(
        "list",
        &list(),
        json!({"add": [color(PAGE, PAGE_AREA)], "select": select("item_a", "item_b")}),
    ));
    recording.press_open_step();
    recording.marked(recording.on(
        "plain",
        &screen([None, None], &[(PAGE_AREA, ELSEWHERE)]),
        json!({"add": [color(FINAL, FINAL_AREA)]}),
    ));

    let ready = recording.ready();
    let missing = ready["missing"].as_array().expect("missing");
    assert_eq!(missing.len(), 1, "{ready}");
    assert_eq!(missing[0]["what"], "selects", "{ready}");
    assert_eq!(missing[0]["step"], 2, "{ready}");
    assert_eq!(missing[0]["details"]["frames"], json!(["f0002"]), "{ready}");

    let error = recording
        .stop()
        .expect_err("step 3's page passes on the select step's frame");
    assert_eq!(error.code, SELF_MISMATCH, "{}", error.message);
    let details = error.details.expect("details");
    assert_eq!(details["reason"], "self_test_error", "{details}");
    assert_eq!(
        details["failure"]["code"], "select_page_mismatch",
        "{details}"
    );
    assert_eq!(details["step"], 2, "{details}");
    assert_eq!(details["frame_id"], "f0002", "{details}");
}

// The work order's test 3: a recording with a select step whose page is alone on every frame, and
// a pair of pages neither of which is a select step's: step 3's screen shows step 2's mark too, so
// step 2's page passes on it beside step 3's. `record ready` is ready; `record stop` generates, and
// its cases name the final case with `record_case_page_ambiguous` as before, whose text says what
// is so: a select step's confirmation does ask.
#[test]
fn sc1_a_pair_without_a_select_side_is_the_cases_warning_as_before() {
    let recording = Recording::new();
    recording.select_on_the_list();
    recording.marked(recording.on(
        "other",
        &other_screen(&[OTHER_AREA]),
        json!({"add": [color(OTHER, OTHER_AREA)], "click": {"region": SLOTS[0]}}),
    ));
    recording.press_open_step();
    recording.marked(recording.on(
        "final",
        &other_screen(&[OTHER_AREA, FINAL_AREA]),
        json!({"add": [color(FINAL, FINAL_AREA)]}),
    ));

    let ready = recording.ready();
    assert_eq!(ready["status"], "ready", "{ready}");
    assert_eq!(ready["missing"], json!([]), "{ready}");
    let lab = recording.stopped();
    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let ambiguous = warnings_of(
        &lab["cases"]["warnings"],
        LabCode::RecordCasePageAmbiguous.as_str(),
    );
    assert!(
        ambiguous[0]["message"]
            .as_str()
            .is_some_and(|text| text.contains("A select step's confirmation does ask")),
        "{lab}"
    );
    assert_eq!(
        without_message(ambiguous),
        [
            json!({"code": LabCode::RecordCasePageAmbiguous.as_str(), "case": "final",
                "page": "neutral/step_03", "other_pages": ["neutral/step_02"],
                "written": true})
        ],
        "{lab}"
    );
}

// The work order's test 4, the contract's first exemption: two optional steps that record one
// pop-up closed by the same press (section 7c). A select step is never optional
// (`on_optional_step`), so it is never one side of such a pair, and the exemption stands beside a
// select step whose page is alone: step 1 the select, steps 2 and 3 the pop-up twice (step 3 reuses
// step 2's mark), both optional, closed at the same rectangle, step 4 the next screen. `record stop`
// generates, and the pair is the `arrival_unconfirmed` warning with `same_as` naming steps 2 and 3,
// as before.
#[test]
fn sc1_the_optional_steps_closed_by_one_press_stay_one_pop_up_beside_a_select() {
    let recording = Recording::new();
    recording.select_on_the_list();
    let popup = other_screen(&[MORE_AREA]);
    recording.marked(recording.on(
        "popup",
        &popup,
        json!({"add": [color("popup/notice", MORE_AREA)], "click": {"region": CLOSE},
               "optional": true}),
    ));
    recording.press_open_step();
    recording.marked(recording.on(
        "popup_again",
        &popup,
        json!({"reuse": ["popup/notice"], "click": {"region": CLOSE}, "optional": true}),
    ));
    recording.press_open_step();
    recording.marked(recording.on(
        "next",
        &other_screen(&[FINAL_AREA]),
        json!({"add": [color(FINAL, FINAL_AREA)]}),
    ));

    let lab = recording.stopped();

    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let same = warnings_of(&lab["warnings"], "arrival_unconfirmed")
        .into_iter()
        .filter(|warning| warning["same_as"] == json!([2, 3]))
        .count();
    assert_eq!(same, 1, "{lab}");
}

// The work order's test 4, the contract's second exemption: the page of the step after a press
// passing on that step's frames (`arrival_unconfirmed`) is a warning, which stands where neither is
// a select step. A select step can be the step after: on step 1's frame f0001 (gray 60 at x 2) the
// list's page passes beside step 1's, and the run's select would confirm on such a screen, if the
// press of step 1 has not yet changed it, with two pages. The run does not honour the exemption
// there, so neither does the mark: refused with the word, naming the select step 2, step 1's page
// and its frame; nothing written. The same pair with a plain press on the list is the warning.
#[test]
fn sc1_the_page_of_the_step_before_a_press_is_no_exemption_for_a_select_after_it() {
    let recording = Recording::new();
    recording.marked(recording.on(
        "start",
        &screen([None, None], &[(OTHER_AREA, SHOWN)]),
        json!({"add": [color(OTHER, OTHER_AREA)], "click": {"region": SLOTS[0]}}),
    ));
    recording.press_open_step();
    let before = recording.recording_bytes();

    let error = recording
        .mark(recording.on(
            "list",
            &list(),
            json!({"add": [color(PAGE, PAGE_AREA)], "select": select("item_a", "item_b")}),
        ))
        .expect_err("the list's page passes on step 1's frame beside step 1's page");

    let refused = select_rejection(error, PASSES_ELSEWHERE);
    assert_eq!(
        refused,
        json!({"id": "select", "reason": PASSES_ELSEWHERE, "frame_id": "f0001",
               "select_step": 2, "step": 1, "transition": false,
               "frame_step": 1, "frame_transition": false}),
        "{refused}"
    );
    assert_eq!(recording.recording_bytes(), before, "nothing written");

    recording.marked(recording.on(
        "list",
        &list(),
        json!({"add": [color(PAGE, PAGE_AREA)], "click": {"region": SLOTS[0]}}),
    ));
    recording.press_open_step();
    recording.marked(recording.on(
        "final",
        &other_screen(&[FINAL_AREA]),
        json!({"add": [color(FINAL, FINAL_AREA)]}),
    ));
    let lab = recording.stopped();
    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let arrival = warnings_of(&lab["warnings"], "arrival_unconfirmed");
    assert_eq!(arrival.len(), 1, "{lab}");
    assert_eq!(arrival[0]["step"], 1, "{lab}");
    assert_eq!(arrival[0]["gate"], "step_02", "{lab}");
    assert_eq!(arrival[0]["frames"], json!(["f0001"]), "{lab}");
}

// The work order's test 4, the contract's third exemption: an ending told from the usual end, the
// last step's page passing on an ending's frame (`ending_usual_insensitive`, a warning). Its two
// sides are the last step's page, which a select never stands on (`on_final_step`), and an ending's
// page, which is no step's, so a select step is never one of them, and the exemption stands beside
// a select step whose page is alone: step 1 the select, step 2 the last screen, stopped; an ending
// taken from a screen showing the last screen's mark and one of its own. `record stop` regenerates,
// with the warning naming the ending and the usual page, as before.
#[test]
fn sc1_an_ending_the_usual_page_passes_on_stays_a_warning_beside_a_select() {
    let recording = Recording::new();
    recording.select_on_the_list();
    recording.marked(recording.on(
        "final",
        &other_screen(&[FINAL_AREA]),
        json!({"add": [color(FINAL, FINAL_AREA)]}),
    ));
    let lab = recording.stop_session();
    assert_eq!(lab["lab_status"], "generated", "{lab}");
    recording.marked(json!({"ending": {
        "frame": recording.frame("other_end", &other_screen(&[FINAL_AREA, MORE_AREA])),
        "outcome_key": "neutral_task_other",
        "page": "other",
        "add": [color("ending/other", MORE_AREA)]
    }}));

    let lab = recording.stopped();

    assert_eq!(lab["lab_status"], "regenerated", "{lab}");
    let insensitive = warnings_of(&lab["warnings"], "ending_usual_insensitive");
    assert_eq!(insensitive.len(), 1, "{lab}");
    assert_eq!(insensitive[0]["outcome_key"], "neutral_task_other", "{lab}");
    assert_eq!(insensitive[0]["usual_page"], "step_02", "{lab}");
}

// The work order's test 5: a recording without a select step, the observed screens with a plain
// press on the list, generates as before: the list's page passes on step 2's frame beside step 2's
// page, which only the cases name, with `record_case_page_ambiguous` on the final case.
#[test]
fn sc1_a_recording_without_a_select_generates_with_the_cases_warning_as_before() {
    let recording = Recording::new();
    recording.marked(recording.on(
        "list",
        &list(),
        json!({"add": [color(PAGE, PAGE_AREA)], "click": {"region": SLOTS[0]}}),
    ));
    recording.press_open_step();
    recording.marked(recording.on(
        "busy",
        &busy_list(),
        json!({"add": [color(BUSY, BUSY_AREA)]}),
    ));

    let ready = recording.ready();
    assert_eq!(ready["status"], "ready", "{ready}");
    assert_eq!(ready["missing"], json!([]), "{ready}");
    let lab = recording.stopped();
    assert_eq!(lab["lab_status"], "generated", "{lab}");
    assert_eq!(
        without_message(warnings_of(
            &lab["cases"]["warnings"],
            LabCode::RecordCasePageAmbiguous.as_str()
        )),
        [
            json!({"code": LabCode::RecordCasePageAmbiguous.as_str(), "case": "final",
                "page": "neutral/step_02", "other_pages": ["neutral/step_01"],
                "written": true})
        ],
        "{lab}"
    );
}
