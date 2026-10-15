// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #396 REC-4 (REC-M v1.2.1 §6, §7 last item, §11, §12; ADD:529): a Lab recording's click
// may follow a template mark. `record mark --click-follow` makes the package click the box the
// template matches on the frame it runs on (`target`), `--click` with `--click-follow` that box's
// corner plus the rectangle's offset from the crop (`offset`, scaled by the factor a template with
// `scales` matched at); the guard is the followed template and no generated click is ever
// `target_center`. A declaration that cannot hold refuses `record mark` with
// `record_click_follow_invalid` (`not_template`, `not_required`, `guard_conflict`); a live frame of
// the step on which it does not hold is that code's rejection inside `record_mark_rejected`
// (`primary_mismatch`, `click_outside_frame`). Template marks take `scales` (REC-5), `mask` (REC-7)
// and `unique` (REC-2) as the Runtime evaluates them: an own scaled mark must pass at every factor
// on some frame, and a second place within the margin is reported. A recording without a follow
// generates its click step, guard, template target and click provenance as before.
//
// Frames are synthetic: 96x64 pixels of flat gray with neutral noise patches; no OCR model is
// installed and no device is used. Requirement basis: REC-M §6.1-§6.3, §7, §12 row REC-4.

use crate::recording::crosscheck::admit;
use crate::recording::store::{old_record_path, recording_path};
use crate::{
    ClickSpec, LAB_RECORD_MARK_SCHEMA, LabOcr, MarkFamily, MarkOutcome, MarkRequest, MarkSpec,
    RecordRect, RecordStartOptions, RecordStopOptions, RecordingLock, TemplateMaskSpec,
    TemplateScalesSpec, TemplateUniqueSpec, record_mark, record_start, record_stop,
};
use actingcommand_contract::{InputAction, LabError, LabErrorClass};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_execution_kernel::{OfflineDecision, simulate_contained_task};
use actingcommand_pack_containment::{
    ContainmentLimits, ContentContainer, expand_content_container,
};
use actingcommand_recognition::resample::resample_pixels;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_0001";
const TASK_ID: &str = "neutral_task";
const WIDTH: u32 = 96;
const HEIGHT: u32 = 64;
const BACKGROUND: u8 = 60;
const FOLLOW_CODE: &str = "record_click_follow_invalid";
const FOLLOW_STAGE: &str = "record_click_follow_check";

// A deterministic generator of neutral gray noise.
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

    fn frame() -> Self {
        Self::from_fn(WIDTH, HEIGHT, |_, _| BACKGROUND)
    }

    fn noise(width: u32, height: u32, seed: u64) -> Self {
        let mut noise = Noise(seed);
        Self::from_fn(width, height, |_, _| noise.between(1, 255))
    }

    // This image resampled by `factor_milli` with the primitive every scaled search uses.
    fn scaled(&self, factor_milli: u32) -> Self {
        let width = (self.width * factor_milli + 500) / 1000;
        let height = (self.height * factor_milli + 500) / 1000;
        Self {
            width,
            height,
            values: resample_pixels(&self.values, (self.width, self.height), 1, (width, height))
                .expect("resampled"),
        }
    }

    // `image` drawn with its top-left corner at `(x, y)`; with `only`, the pixels it admits.
    fn draw_where(mut self, x: u32, y: u32, image: &Gray, only: impl Fn(u8) -> bool) -> Self {
        for row in 0..image.height {
            for column in 0..image.width {
                let value = image.values[(row * image.width + column) as usize];
                if only(value) {
                    self.values[((y + row) * self.width + x + column) as usize] = value;
                }
            }
        }
        self
    }

    fn draw(self, x: u32, y: u32, image: &Gray) -> Self {
        self.draw_where(x, y, image, |_| true)
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

fn button() -> Gray {
    Gray::noise(12, 12, 0x0396_0401)
}

fn next() -> Gray {
    Gray::noise(12, 12, 0x0396_0402)
}

// The button's crop on the primary frame and the search area that covers where it moves.
const BUTTON_CROP: RecordRect = RecordRect {
    x: 20,
    y: 20,
    width: 12,
    height: 12,
};
const BUTTON_SEARCH: RecordRect = RecordRect {
    x: 0,
    y: 10,
    width: 96,
    height: 30,
};

fn template(id: &str, crop: RecordRect, search: RecordRect) -> MarkSpec {
    MarkSpec {
        search: Some(search),
        ..MarkSpec::region_only(id.to_owned(), MarkFamily::Template, crop)
    }
}

// A state directory with an active session of `INSTANCE` and its Lab recording, before any
// step. The lock is dropped before the directory.
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
        let lock = RecordingLock::acquire(&state, INSTANCE, "gate_rec4").expect("recording lock");
        let ocr = LabOcr::from_vision_root(&temp.path().join("no-vision"));
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
        Self { lock, ocr, temp }
    }

    // Writes `image` as the frame file `name` and returns its path.
    fn frame(&self, name: &str, image: &Gray) -> String {
        let path = self.temp.path().join(format!("{name}.png"));
        fs::write(&path, image.png()).expect("frame file");
        path.display().to_string()
    }

    fn mark(&self, request: MarkRequest) -> Result<MarkOutcome, LabError> {
        record_mark(&self.lock, &request, false, &self.ocr)
    }

    // The final step: the next screen with its own template mark.
    fn mark_next_step(&self) {
        let frame = self.frame("next", &Gray::frame().draw(40, 46, &next()));
        self.mark(MarkRequest {
            schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
            frame: Some(frame),
            add: vec![template(
                "ui/next",
                rect(40, 46, 12, 12),
                rect(40, 46, 12, 12),
            )],
            ..MarkRequest::default()
        })
        .unwrap_or_else(|error| panic!("record mark of the next step: {}", error.code));
    }

    // `record stop`: the `lab` output with its status.
    fn stop(&self) -> Value {
        let outcome = record_stop(&self.lock, &RecordStopOptions::default(), &self.ocr)
            .unwrap_or_else(|error| {
                panic!(
                    "record stop: {}: {} {:?}",
                    error.code, error.message, error.details
                )
            });
        let mut lab = serde_json::to_value(&outcome.lab).expect("lab JSON");
        lab["lab_status"] = json!(outcome.lab_status);
        lab
    }

    // The files of the package `record stop` wrote.
    fn package(&self, lab: &Value) -> BTreeMap<String, Vec<u8>> {
        let path = lab["path"].as_str().expect("package path");
        let bytes = fs::read(path).expect("package file");
        expand_content_container(&bytes, ContentContainer::Zip, ContainmentLimits::default())
            .expect("package container")
    }

    fn recording(&self) -> Value {
        let file = recording_path(&self.temp.path().join("state"), RECORD_ID);
        serde_json::from_slice(&fs::read(file).expect("recording file")).expect("recording")
    }

    fn recording_bytes(&self) -> Vec<u8> {
        fs::read(recording_path(&self.temp.path().join("state"), RECORD_ID))
            .expect("recording file")
    }
}

fn task(entries: &BTreeMap<String, Vec<u8>>) -> Value {
    let bytes = entries
        .get(&format!("resources/operations/{TASK_ID}/task.json"))
        .expect("task.json");
    serde_json::from_slice(bytes).expect("task JSON")
}

fn never_target_center(entries: &BTreeMap<String, Vec<u8>>) {
    for (path, bytes) in entries {
        assert!(
            !String::from_utf8_lossy(bytes).contains("target_center"),
            "{path} names target_center"
        );
    }
}

// The step-1 request: the primary frame and its samples, the button template, and `click`.
fn step_one(
    recording: &Recording,
    primary: &Gray,
    samples: &[&Gray],
    add: Vec<MarkSpec>,
    click: ClickSpec,
) -> MarkRequest {
    MarkRequest {
        schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
        frame: Some(recording.frame("primary", primary)),
        samples: samples
            .iter()
            .enumerate()
            .map(|(index, sample)| recording.frame(&format!("sample_{index}"), sample))
            .collect(),
        add,
        click: Some(click),
        ..MarkRequest::default()
    }
}

fn follow(id: &str) -> ClickSpec {
    ClickSpec {
        follow: Some(id.to_owned()),
        ..ClickSpec::default()
    }
}

fn primary_frame() -> Gray {
    Gray::frame().draw(20, 20, &button())
}

fn moved_frame() -> Gray {
    Gray::frame().draw(44, 20, &button())
}

// The decision of the admitted package on `image`, as `record stop`'s first decision simulates it.
fn simulated_tap(
    recording: &Recording,
    lab: &Value,
    entries: &BTreeMap<String, Vec<u8>>,
    image: &Gray,
) -> (i32, i32) {
    // The answer names the package's files; `entries` is the package written at its path.
    assert_eq!(
        lab["entries"],
        json!(entries.keys().collect::<Vec<_>>()),
        "{lab}"
    );
    let prepared =
        admit(entries, &recording.ocr).unwrap_or_else(|error| panic!("admission: {}", error.code));
    let frame = Frame::from_png(image.png(), CaptureBackendName::AdbScreencap).expect("frame");
    let result = simulate_contained_task(&prepared, vec![frame]).expect("simulation");
    match result.decision {
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

fn inside(point: (i32, i32), rect: RecordRect) -> bool {
    point.0 >= rect.x
        && point.1 >= rect.y
        && point.0 < rect.x + rect.width
        && point.1 < rect.y + rect.height
}

// The follow outcome of a refusal or a rejection: the code, its category and its values.
fn assert_follow_outcome(outcome: &Value, mark_id: &str, problem: &str) {
    assert_eq!(outcome["code"], FOLLOW_CODE, "{outcome}");
    assert_eq!(outcome["category"], "error", "{outcome}");
    assert_eq!(
        outcome["values"],
        json!({"mark_id": mark_id, "follow_problem": problem, "stage": FOLLOW_STAGE}),
        "{outcome}"
    );
}

// A declaration that cannot hold: the command is refused (exit 3) and nothing is written.
fn assert_refused(
    recording: &Recording,
    before: &[u8],
    error: LabError,
    mark_id: &str,
    problem: &str,
) {
    assert_eq!(error.code, FOLLOW_CODE, "{}", error.message);
    assert_eq!(error.class, LabErrorClass::SafetyBlocked);
    let details = error.details.expect("details");
    assert_follow_outcome(&details["outcome"], mark_id, problem);
    assert_eq!(recording.recording_bytes(), before, "nothing is written");
}

// A frame on which the click does not hold: the batch is refused under record_mark_rejected
// with the followed template's rejection naming the frame.
fn assert_rejected(error: LabError, mark_id: &str, problem: &str, frame_id: &str) {
    assert_eq!(error.code, "record_mark_rejected", "{}", error.message);
    let details = error.details.expect("details");
    let rejection = details["marks"]
        .as_array()
        .and_then(|marks| marks.iter().find(|mark| mark["reason"] == FOLLOW_CODE))
        .unwrap_or_else(|| panic!("no follow rejection in {details}"));
    assert_eq!(rejection["id"], mark_id, "{rejection}");
    assert_eq!(rejection["frame_id"], frame_id, "{rejection}");
    assert_follow_outcome(&rejection["outcome"], mark_id, problem);
}

// REC-M §6.1, §6.3, §12: `--click-follow` generates a `target` click on the followed template,
// guarded by that template (its asset and search area), which the kernel admits; the recording
// keeps the crop as the click rectangle.
#[test]
fn rec4_a_click_following_a_template_generates_a_target_click_guarded_by_it() {
    let recording = Recording::new();
    let marked = recording
        .mark(step_one(
            &recording,
            &primary_frame(),
            &[&moved_frame()],
            vec![template("ui/button", BUTTON_CROP, BUTTON_SEARCH)],
            follow("ui/button"),
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    let click = serde_json::to_value(&marked.click).expect("click JSON");
    assert_eq!(
        click["follow"],
        json!({"mark_id": "ui/button", "form": "target"})
    );
    assert_eq!(click["rect"], json!(BUTTON_CROP));
    assert_eq!(click["source"], "followed_mark");
    recording.mark_next_step();

    let lab = recording.stop();

    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let entries = recording.package(&lab);
    let operation = &task(&entries)["operations"][0];
    assert_eq!(
        operation["click"],
        json!({"kind": "target", "target_id": "ui/button"})
    );
    assert_eq!(
        operation["guard"],
        json!({
            "page_id": "step_01",
            "target_id": "ui/button",
            "expected_rect": BUTTON_SEARCH,
            "verify_template": "assets/ui_button.png"
        })
    );
    never_target_center(&entries);
}

// REC-M §6.1, §12: `--click` with `--click-follow` generates an `offset` click whose offset is the
// rectangle minus the crop's top-left corner.
#[test]
fn rec4_a_rectangle_with_a_follow_generates_its_offset_from_the_crop() {
    let recording = Recording::new();
    let marked = recording
        .mark(step_one(
            &recording,
            &primary_frame(),
            &[&moved_frame()],
            vec![template("ui/button", BUTTON_CROP, BUTTON_SEARCH)],
            ClickSpec {
                region: Some(rect(24, 24, 4, 4)),
                follow: Some("ui/button".to_owned()),
                ..ClickSpec::default()
            },
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    let click = serde_json::to_value(&marked.click).expect("click JSON");
    assert_eq!(
        click["follow"],
        json!({"mark_id": "ui/button", "form": "offset", "offset": rect(4, 4, 4, 4)})
    );
    assert_eq!(click["rect"], json!(rect(24, 24, 4, 4)));
    recording.mark_next_step();

    let lab = recording.stop();

    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let entries = recording.package(&lab);
    assert_eq!(
        task(&entries)["operations"][0]["click"],
        json!({"kind": "offset", "target_id": "ui/button", "offset": rect(4, 4, 4, 4)})
    );
    never_target_center(&entries);
}

// REC-M §6.2, §12: on a sample frame where the button moved to the right edge, the followed box
// (the match plus the offset) leaves the frame: click_outside_frame, naming the sample.
#[test]
fn rec4_a_followed_box_that_leaves_a_sample_frame_is_click_outside_frame() {
    let recording = Recording::new();
    let edge = Gray::frame().draw(80, 20, &button());

    let error = recording
        .mark(step_one(
            &recording,
            &primary_frame(),
            &[&edge],
            vec![template("ui/button", BUTTON_CROP, BUTTON_SEARCH)],
            ClickSpec {
                region: Some(rect(32, 20, 8, 8)),
                follow: Some("ui/button".to_owned()),
                ..ClickSpec::default()
            },
        ))
        .expect_err("the followed box leaves the sample frame");

    assert_rejected(error, "ui/button", "click_outside_frame", "f0002");
    assert_eq!(
        recording.recording()["steps"],
        json!([]),
        "nothing is written"
    );
}

// REC-M §6.2, §12: a click follows a template mark; following a color mark or a check is
// not_template.
#[test]
fn rec4_following_a_color_mark_or_a_check_is_not_template() {
    let recording = Recording::new();
    let before = recording.recording_bytes();
    let color = MarkSpec::region_only(
        "state/color".to_owned(),
        MarkFamily::Color,
        rect(70, 40, 4, 4),
    );
    let check = MarkSpec {
        region: None,
        all_of: Some(vec!["ui/button".to_owned(), "state/color".to_owned()]),
        ..MarkSpec::region_only("check/both".to_owned(), MarkFamily::Check, rect(0, 0, 1, 1))
    };

    for followed in ["state/color", "check/both"] {
        let error = recording
            .mark(step_one(
                &recording,
                &primary_frame(),
                &[],
                vec![
                    template("ui/button", BUTTON_CROP, BUTTON_SEARCH),
                    color.clone(),
                    check.clone(),
                ],
                follow(followed),
            ))
            .expect_err("only a template can be followed");

        assert_refused(&recording, &before, error, followed, "not_template");
    }
}

// REC-M §6.2, §12: a template that only an any_of check of the step names is not a required
// mark of the page, so it cannot be the guard: not_required.
#[test]
fn rec4_following_a_template_only_an_any_of_check_names_is_not_required() {
    let recording = Recording::new();
    let before = recording.recording_bytes();
    let primary = primary_frame().draw(70, 40, &next());
    let either = MarkSpec {
        region: None,
        any_of: Some(vec!["ui/button".to_owned(), "ui/other".to_owned()]),
        ..MarkSpec::region_only(
            "check/either".to_owned(),
            MarkFamily::Check,
            rect(0, 0, 1, 1),
        )
    };

    let error = recording
        .mark(step_one(
            &recording,
            &primary,
            &[],
            vec![
                template("ui/button", BUTTON_CROP, BUTTON_SEARCH),
                template("ui/other", rect(70, 40, 12, 12), rect(70, 40, 12, 12)),
                either,
            ],
            follow("ui/button"),
        ))
        .expect_err("an any_of member is not required");

    assert_refused(&recording, &before, error, "ui/button", "not_required");
}

// REC-M §6.2, §12: the guard of a following click is the followed template; naming another
// guard is guard_conflict.
#[test]
fn rec4_another_click_guard_than_the_followed_template_is_guard_conflict() {
    let recording = Recording::new();
    let before = recording.recording_bytes();
    let mut request = step_one(
        &recording,
        &primary_frame(),
        &[],
        vec![
            template("ui/button", BUTTON_CROP, BUTTON_SEARCH),
            MarkSpec::region_only(
                "state/color".to_owned(),
                MarkFamily::Color,
                rect(70, 40, 4, 4),
            ),
        ],
        follow("ui/button"),
    );
    request.click_guard = Some("state/color".to_owned());

    let error = recording
        .mark(request)
        .expect_err("the guard must be the followed template");

    assert_refused(&recording, &before, error, "ui/button", "guard_conflict");
}

// REC-M §6.2, §12: an equal match earlier in the search area wins on the primary frame, so the
// offset would be taken from another place than the crop: primary_mismatch.
#[test]
fn rec4_an_earlier_equal_match_on_the_primary_frame_is_primary_mismatch() {
    let recording = Recording::new();
    let twice = Gray::frame().draw(8, 20, &button()).draw(40, 20, &button());

    let error = recording
        .mark(step_one(
            &recording,
            &twice,
            &[],
            vec![template("ui/button", rect(40, 20, 12, 12), BUTTON_SEARCH)],
            follow("ui/button"),
        ))
        .expect_err("the best match on the primary frame is not the crop");

    assert_rejected(error, "ui/button", "primary_mismatch", "f0001");
}

// REC-M §6.3, §12: a recording without a follow generates its click step, its guard, its template
// target and its click provenance as before (rect click, guard on the first template), and no
// document names target_center or follow.
#[test]
fn rec4_without_a_follow_the_click_step_is_generated_as_before() {
    let recording = Recording::new();
    let marked = recording
        .mark(step_one(
            &recording,
            &primary_frame(),
            &[],
            vec![template("ui/button", BUTTON_CROP, BUTTON_SEARCH)],
            ClickSpec {
                region: Some(rect(24, 24, 4, 4)),
                ..ClickSpec::default()
            },
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    let frame_sha256 = marked.frame.expect("primary frame").sha256;
    recording.mark_next_step();

    let lab = recording.stop();

    let entries = recording.package(&lab);
    let task = task(&entries);
    assert_eq!(
        task["operations"][0],
        json!({
            "id": "step_01_click",
            "purpose": "Lab recording step 1",
            "from": "step_01",
            "to": "step_02",
            "click": {"kind": "rect", "x": 24, "y": 24, "width": 4, "height": 4},
            "guard": {
                "page_id": "step_01",
                "target_id": "ui/button",
                "expected_rect": BUTTON_SEARCH,
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
    let button_target = task["verify_templates"]
        .as_array()
        .and_then(|targets| targets.iter().find(|target| target["id"] == "ui/button"))
        .expect("the button's template target");
    assert_eq!(
        *button_target,
        json!({
            "id": "ui/button",
            "template": "assets/ui_button.png",
            "region": {"mode": "rect", "rect": BUTTON_SEARCH},
            "provenance": {
                "source": "lab_record",
                "record_id": RECORD_ID,
                "step": 1,
                "record_index": 1,
                "self_test": "passed"
            }
        })
    );
    assert_eq!(
        task["provenance"]["steps"][0]["click"],
        json!({
            "rect": rect(24, 24, 4, 4),
            "source": "declared",
            "from": null,
            "retry": null,
            "executed": null,
            "attempts": 0,
            "needs_review": false
        })
    );
    never_target_center(&entries);
    for (path, bytes) in &entries {
        assert!(
            !String::from_utf8_lossy(bytes).contains("\"follow\""),
            "{path} names follow"
        );
    }
}

// REC-M §6.3, §12: step 1's click follows the template, so the first decision simulates its
// sample frame too; there the package clicks inside the moved box, not the primary rectangle.
#[test]
fn rec4_on_a_moved_sample_frame_the_first_decision_clicks_inside_the_new_box() {
    let recording = Recording::new();
    recording
        .mark(step_one(
            &recording,
            &primary_frame(),
            &[&moved_frame()],
            vec![template("ui/button", BUTTON_CROP, BUTTON_SEARCH)],
            follow("ui/button"),
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    recording.mark_next_step();

    let lab = recording.stop();

    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let entries = recording.package(&lab);
    let primary = simulated_tap(&recording, &lab, &entries, &primary_frame());
    assert!(inside(primary, BUTTON_CROP), "{primary:?}");
    let moved = simulated_tap(&recording, &lab, &entries, &moved_frame());
    assert!(inside(moved, rect(44, 20, 12, 12)), "{moved:?}");
    assert!(!inside(moved, BUTTON_CROP), "{moved:?}");
}

// REC-M §6.1, §7, §12: a followed template with scales that matches a sample frame at 750 milli
// gives the offset scaled by 750: (4, 4, 8, 8) becomes (3, 3, 6, 6) from the scaled match.
#[test]
fn rec4_a_followed_template_matched_at_750_scales_its_offset() {
    let recording = Recording::new();
    let icon = Gray::noise(16, 16, 0x0396_0403);
    let primary = Gray::frame().draw(20, 16, &icon);
    let zoomed = Gray::frame().draw(52, 16, &icon.scaled(750));
    let zoom = MarkSpec {
        scales: Some(TemplateScalesSpec {
            factors_milli: vec![1000, 750],
        }),
        ..template("ui/zoom", rect(20, 16, 16, 16), rect(0, 8, 96, 32))
    };

    let marked = recording
        .mark(step_one(
            &recording,
            &primary,
            &[&zoomed],
            vec![zoom],
            ClickSpec {
                region: Some(rect(24, 20, 8, 8)),
                follow: Some("ui/zoom".to_owned()),
                ..ClickSpec::default()
            },
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    assert_eq!(
        marked.marks[0].self_test.matched_scales_milli,
        Some(vec![1000, 750])
    );
    recording.mark_next_step();
    let lab = recording.stop();

    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let entries = recording.package(&lab);
    let task = task(&entries);
    assert_eq!(
        task["verify_templates"][0]["scales"],
        json!({"factors_milli": [1000, 750]})
    );
    assert_eq!(
        task["operations"][0]["click"],
        json!({"kind": "offset", "target_id": "ui/zoom", "offset": rect(4, 4, 8, 8)})
    );
    let tap = simulated_tap(&recording, &lab, &entries, &zoomed);
    assert!(inside(tap, rect(55, 19, 6, 6)), "{tap:?}");
}

// REC-M §7 (last item): an own template mark with scales must pass at every factor on at least
// one frame of its step; with only the primary frame, 750 never matches.
#[test]
fn rec4_a_scaled_template_mark_needs_every_factor_on_some_frame() {
    let recording = Recording::new();
    let icon = Gray::noise(16, 16, 0x0396_0403);
    let zoom = MarkSpec {
        scales: Some(TemplateScalesSpec {
            factors_milli: vec![1000, 750],
        }),
        ..template("ui/zoom", rect(20, 16, 16, 16), rect(0, 8, 96, 32))
    };

    let error = recording
        .mark(MarkRequest {
            schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
            frame: Some(recording.frame("primary", &Gray::frame().draw(20, 16, &icon))),
            add: vec![zoom],
            ..MarkRequest::default()
        })
        .expect_err("750 matches on no frame");

    assert_eq!(error.code, "record_mark_rejected", "{}", error.message);
    let details = error.details.expect("details");
    assert_eq!(
        details["marks"],
        json!([{
            "id": "ui/zoom",
            "reason": "self_test_failed",
            "factors_milli": [1000, 750],
            "matched_scales_milli": [1000]
        }])
    );
}

// REC-M §6.1, §9.1, §11: a template mark with a gray-range mask is self-tested with its mask, so
// its masked glyph matches over another background on a sample frame, and a click can follow it;
// the package declares the mask.
#[test]
fn rec4_a_masked_template_mark_is_self_tested_with_its_mask_and_can_be_followed() {
    let recording = Recording::new();
    let mut noise = Noise(0x0396_0404);
    let glyph = Gray::from_fn(16, 16, |x, y| {
        if (x + 2 * y) % 3 == 0 {
            noise.between(150, 255)
        } else {
            40
        }
    });
    let primary = Gray::frame().draw(20, 16, &glyph);
    let other_background =
        Gray::from_fn(WIDTH, HEIGHT, |_, _| 100).draw_where(48, 16, &glyph, |value| value >= 150);
    let masked = MarkSpec {
        mask: Some(TemplateMaskSpec::Range {
            lower: 150,
            upper: 255,
        }),
        ..template("ui/glyph", rect(20, 16, 16, 16), rect(0, 8, 96, 32))
    };

    recording
        .mark(step_one(
            &recording,
            &primary,
            &[&other_background],
            vec![masked],
            follow("ui/glyph"),
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    recording.mark_next_step();
    let lab = recording.stop();

    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let entries = recording.package(&lab);
    let task = task(&entries);
    assert_eq!(
        task["verify_templates"][0]["mask"],
        json!({"type": "range", "lower": 150, "upper": 255})
    );
    assert_eq!(
        task["operations"][0]["click"],
        json!({"kind": "target", "target_id": "ui/glyph"})
    );
}

// REC-M §6.1, D10: a template mark that declares unique reports the second place within its
// margin, and that refuses the mark.
#[test]
fn rec4_a_unique_template_mark_with_a_second_place_in_its_margin_is_refused_with_it() {
    let recording = Recording::new();
    let twice = primary_frame().draw(60, 20, &button());
    let unique = MarkSpec {
        unique: Some(TemplateUniqueSpec { margin_milli: 50 }),
        ..template("ui/button", BUTTON_CROP, BUTTON_SEARCH)
    };

    let error = recording
        .mark(MarkRequest {
            schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
            frame: Some(recording.frame("primary", &twice)),
            add: vec![unique],
            ..MarkRequest::default()
        })
        .expect_err("a second copy lies within the margin");

    assert_eq!(error.code, "record_mark_rejected", "{}", error.message);
    let details = error.details.expect("details");
    let rejection = &details["marks"][0];
    assert_eq!(rejection["id"], "ui/button");
    assert_eq!(rejection["reason"], "self_test_failed");
    let uniqueness = &rejection["uniqueness"];
    assert_eq!(uniqueness["margin_milli"], 50, "{rejection}");
    assert_eq!(
        uniqueness["second_score_milli"], uniqueness["score_milli"],
        "{rejection}"
    );
    assert_eq!(
        uniqueness["second_rect"],
        json!(rect(60, 20, 12, 12)),
        "{rejection}"
    );
}
