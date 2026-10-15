// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #390 LAB-2 (forced-compromises mapping row LAB-2; compromises L-02 and T-03): the Lab
// recording completes its generated package. A step's page may forbid marks of other steps (each
// must not pass on any live frame of the step, and the generated page rule lists them under
// `forbidden`) and may be anchored by one of its own required template marks (the generated task
// declares it as the page's `anchors[]` entry, whose target `page/<page>` the guard names). A step's
// effect may be a drag from its click rectangle to a destination rectangle or a long press at one
// point, each over a duration: the package clicks with `drag` and `long_press`, the default task
// timeout counts the duration, and `do --capture --record` executes the declared gesture as one
// swipe. A stopped recording takes marks on its steps again and the next `record stop` generates
// its package again; it takes no new step and no step operation. (The `--template` search flag and
// the navigation projection's several destinations have their tests in actinglab, pack-containment,
// resource-tooling and the kernel's drive module.)
//
// Frames are synthetic: 96x64 pixels of flat gray with neutral noise patches; no OCR model is
// installed and no device is used. Requirement basis: FCM row LAB-2 and compromise L-02 ("--template
// declares search; generated pages carry forbidden and anchors; swipe, drag and long press are
// recorded; marks after stop and regeneration").

use crate::recording::crosscheck::admit;
use crate::recording::store::{old_record_path, recording_path};
use crate::{
    ClickEffect, ClickSpec, CommitClickRequest, LAB_RECORD_MARK_SCHEMA, LabOcr, MarkFamily,
    MarkOutcome, MarkRequest, MarkSpec, PlanClickRequest, PlannedSwipe, RecordPoint, RecordRect,
    RecordStartOptions, RecordStopOptions, RecordingLock, SelfTestStatus, StepAction,
    StepActionKind, record_commit_click, record_mark, record_plan_click, record_start, record_stop,
};
use actingcommand_contract::{InputAction, LabError};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_execution_kernel::{OfflineDecision, simulate_contained_task};
use actingcommand_pack_containment::{
    ContainmentLimits, ContentContainer, expand_content_container,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_0390";
const TASK_ID: &str = "neutral_task";
const WIDTH: u32 = 96;
const HEIGHT: u32 = 64;
const BACKGROUND: u8 = 60;

// The button of step 1 and the mark of step 2, each searched at its crop.
const BUTTON: RecordRect = RecordRect {
    x: 20,
    y: 20,
    width: 12,
    height: 12,
};
const NEXT: RecordRect = RecordRect {
    x: 40,
    y: 46,
    width: 12,
    height: 12,
};
// The tap of step 1, and a drag's start and destination.
const TAP: RecordRect = RecordRect {
    x: 60,
    y: 10,
    width: 8,
    height: 8,
};
const DRAG_TO: RecordRect = RecordRect {
    x: 60,
    y: 40,
    width: 8,
    height: 8,
};

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

    // `image` drawn with its top-left corner at `(x, y)`.
    fn draw(mut self, x: u32, y: u32, image: &Gray) -> Self {
        for row in 0..image.height {
            for column in 0..image.width {
                self.values[((y + row) * self.width + x + column) as usize] =
                    image.values[(row * image.width + column) as usize];
            }
        }
        self
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
    Gray::noise(12, 12, 0x0390_0201)
}

fn next() -> Gray {
    Gray::noise(12, 12, 0x0390_0202)
}

// Step 1's screen: the button; step 2's screen: the next mark.
fn first() -> Gray {
    Gray::frame().draw(20, 20, &button())
}

fn second() -> Gray {
    Gray::frame().draw(40, 46, &next())
}

fn template(id: &str, crop: RecordRect) -> MarkSpec {
    MarkSpec {
        search: Some(crop),
        ..MarkSpec::region_only(id.to_owned(), MarkFamily::Template, crop)
    }
}

fn tap() -> ClickSpec {
    ClickSpec {
        region: Some(TAP),
        ..ClickSpec::default()
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
        let lock = RecordingLock::acquire(&state, INSTANCE, "gate_lab2").expect("recording lock");
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

    fn state(&self) -> PathBuf {
        self.temp.path().join("state")
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

    // A step with its own frame, marks and click.
    fn step(
        &self,
        name: &str,
        image: &Gray,
        add: Vec<MarkSpec>,
        click: Option<ClickSpec>,
    ) -> MarkRequest {
        MarkRequest {
            schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
            frame: Some(self.frame(name, image)),
            add,
            click,
            ..MarkRequest::default()
        }
    }

    // Step 1: the button with `click`.
    fn mark_first(&self, click: ClickSpec) -> MarkOutcome {
        self.mark(self.step(
            "first",
            &first(),
            vec![template("ui/button", BUTTON)],
            Some(click),
        ))
        .unwrap_or_else(|error| panic!("step 1: {} {:?}", error.code, error.details))
    }

    // Step 2: the final screen with its own mark.
    fn mark_second(&self) -> MarkOutcome {
        self.mark(self.step("second", &second(), vec![template("ui/next", NEXT)], None))
            .unwrap_or_else(|error| panic!("step 2: {} {:?}", error.code, error.details))
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

    // `record stop` as actinglab runs it: the package, then the session file stopped.
    fn stop_session(&self) -> Value {
        let lab = self.stop();
        let path = old_record_path(&self.state(), INSTANCE);
        let mut session: Value =
            serde_json::from_slice(&fs::read(&path).expect("session file")).expect("session");
        session["status"] = json!("stopped");
        fs::write(&path, serde_json::to_vec(&session).expect("session JSON"))
            .expect("session file");
        lab
    }

    // The files of the package `record stop` wrote (a template makes it a ZIP).
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
        fs::read(recording_path(&self.state(), RECORD_ID)).expect("recording file")
    }

    fn plan(&self, request: PlanClickRequest) -> Result<crate::ClickPlan, LabError> {
        record_plan_click(&self.lock, &request)
    }
}

fn task(entries: &BTreeMap<String, Vec<u8>>) -> Value {
    let bytes = entries
        .get(&format!("resources/operations/{TASK_ID}/task.json"))
        .expect("task.json");
    serde_json::from_slice(bytes).expect("task JSON")
}

// The input the admitted package decides on `image` for its first step.
fn simulated(
    recording: &Recording,
    entries: &BTreeMap<String, Vec<u8>>,
    image: &Gray,
) -> InputAction {
    let prepared =
        admit(entries, &recording.ocr).unwrap_or_else(|error| panic!("admission: {}", error.code));
    let frame = Frame::from_png(image.png(), CaptureBackendName::AdbScreencap).expect("frame");
    let result = simulate_contained_task(&prepared, vec![frame]).expect("simulation");
    match result.decision {
        OfflineDecision::WouldClick {
            operation_label,
            action,
            ..
        } => {
            assert_eq!(operation_label, "step_01_click");
            action
        }
        other => panic!("not a click: {other:?}"),
    }
}

fn inside(x: i32, y: i32, rect: RecordRect) -> bool {
    x >= rect.x && y >= rect.y && x < rect.x + rect.width && y < rect.y + rect.height
}

// The refused marks of a `record_mark_rejected`.
fn rejected_marks(error: LabError) -> Value {
    assert_eq!(error.code, "record_mark_rejected", "{}", error.message);
    error.details.expect("details")["marks"].clone()
}

// L-02 "--template declares search", with the coordinator's input of 10-15 (a game update shifted
// a menu bar by about 40 px, and every search area equal to its template box failed): a template
// mark without a declared search area searches its crop grown by 48 px on each side, clamped to
// the frame, so the button moved by 20 px on a sample frame still passes, and the package
// declares that area as the template's region.
#[test]
fn lab2_a_template_without_a_search_area_searches_48_px_around_its_crop() {
    let recording = Recording::new();
    let marked = recording
        .mark(MarkRequest {
            samples: vec![recording.frame("moved", &Gray::frame().draw(40, 24, &button()))],
            ..recording.step(
                "first",
                &first(),
                vec![MarkSpec::region_only(
                    "ui/button".to_owned(),
                    MarkFamily::Template,
                    BUTTON,
                )],
                Some(tap()),
            )
        })
        .unwrap_or_else(|error| panic!("step 1: {} {:?}", error.code, error.details));
    assert_eq!(marked.marks[0].search, Some(rect(0, 0, 80, 64)));
    assert_eq!(marked.marks[0].self_test.status, SelfTestStatus::Passed);
    recording.mark_second();

    let lab = recording.stop();

    assert_eq!(lab["lab_status"], "generated", "{lab}");
    assert_eq!(
        task(&recording.package(&lab))["verify_templates"][0]["region"],
        json!({"mode": "rect", "rect": rect(0, 0, 80, 64)})
    );
}

// L-02 "generated pages carry forbidden": step 2's page forbids step 1's button, which passes on
// none of step 2's frames; the generated page rule lists it under `forbidden`.
#[test]
fn lab2_a_page_forbids_a_mark_of_another_step_that_is_absent_from_its_frames() {
    let recording = Recording::new();
    recording.mark_first(tap());

    let marked = recording
        .mark(MarkRequest {
            forbid: vec!["ui/button".to_owned()],
            ..recording.step("second", &second(), vec![template("ui/next", NEXT)], None)
        })
        .unwrap_or_else(|error| panic!("step 2: {} {:?}", error.code, error.details));

    assert_eq!(
        serde_json::to_value(&marked.forbidden).expect("forbidden JSON"),
        json!([{"id": "ui/button", "family": "template", "status": "absent", "frames": 1}])
    );
    assert_eq!(
        serde_json::to_value(&marked.step_state).expect("step state JSON")["forbidden"],
        json!(["ui/button"])
    );
    let lab = recording.stop();
    assert_eq!(lab["lab_status"], "generated", "{lab}");
    assert_eq!(
        task(&recording.package(&lab))["page_rules"],
        json!({
            "step_01": {"required": ["ui/button"]},
            "step_02": {"required": ["ui/next"], "forbidden": ["ui/button"]}
        })
    );
}

// L-02: a forbidden mark that passes on a live frame of the step would let the page match where it
// must not: the batch is refused with forbidden_mark_passes naming the frame, and nothing is
// written.
#[test]
fn lab2_a_forbidden_mark_that_passes_on_a_live_frame_refuses_the_batch() {
    let recording = Recording::new();
    recording.mark_first(tap());
    let before = recording.recording_bytes();

    let error = recording
        .mark(MarkRequest {
            forbid: vec!["ui/button".to_owned()],
            ..recording.step(
                "second",
                &second().draw(20, 20, &button()),
                vec![template("ui/next", NEXT)],
                None,
            )
        })
        .expect_err("the forbidden button is on the frame");

    assert_eq!(
        rejected_marks(error),
        json!([{"id": "ui/button", "reason": "forbidden_mark_passes", "frame_id": "f0002"}])
    );
    assert_eq!(recording.recording_bytes(), before, "nothing is written");
}

// L-02: a page forbids marks of other steps only (a mark of its own step is refused, an unknown one
// is forbidden_source_missing); a mark another page forbids stays in use on its own step, and
// `remove` on the forbidding step takes it off that page.
#[test]
fn lab2_a_page_forbids_only_marks_of_other_steps_and_keeps_them_in_use() {
    let recording = Recording::new();
    recording.mark_first(tap());

    let own = recording
        .mark(MarkRequest {
            forbid: vec!["ui/next".to_owned()],
            ..recording.step("second", &second(), vec![template("ui/next", NEXT)], None)
        })
        .expect_err("a page requires its own mark");
    assert_eq!(own.code, "validation_failed", "{}", own.message);

    let unknown = recording
        .mark(MarkRequest {
            forbid: vec!["ui/missing".to_owned()],
            ..recording.step("second", &second(), vec![template("ui/next", NEXT)], None)
        })
        .expect_err("no step has the mark");
    assert_eq!(
        rejected_marks(unknown),
        json!([{"id": "ui/missing", "reason": "forbidden_source_missing"}])
    );

    recording
        .mark(MarkRequest {
            forbid: vec!["ui/button".to_owned()],
            ..recording.step("second", &second(), vec![template("ui/next", NEXT)], None)
        })
        .unwrap_or_else(|error| panic!("step 2: {} {:?}", error.code, error.details));
    let in_use = recording
        .mark(MarkRequest {
            schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
            step: Some(1),
            remove: vec!["ui/button".to_owned()],
            ..MarkRequest::default()
        })
        .expect_err("step 2 forbids the button");
    assert_eq!(in_use.code, "record_mark_in_use", "{}", in_use.message);

    let taken_off = recording
        .mark(MarkRequest {
            schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
            step: Some(2),
            remove: vec!["ui/button".to_owned()],
            ..MarkRequest::default()
        })
        .unwrap_or_else(|error| panic!("remove: {} {:?}", error.code, error.details));
    assert_eq!(taken_off.removed, ["ui/button"]);
    assert_eq!(
        serde_json::to_value(&taken_off.step_state).expect("step state JSON")["forbidden"],
        Value::Null
    );
}

// L-02 "generated pages carry anchors": step 1's button anchors its page. The task declares it as
// the page's `anchors[]` entry (id: the page), not as a template target of its own; the page has
// no rule of its own beyond the anchor's requirement, and the guard names the anchor's target
// `page/step_01`; the kernel admits the package and the first decision clicks the tap.
#[test]
fn lab2_an_anchor_declares_its_page_by_the_template() {
    let recording = Recording::new();
    let marked = recording
        .mark(MarkRequest {
            anchor: Some("ui/button".to_owned()),
            ..recording.step(
                "first",
                &first(),
                vec![template("ui/button", BUTTON)],
                Some(tap()),
            )
        })
        .unwrap_or_else(|error| panic!("step 1: {} {:?}", error.code, error.details));
    assert_eq!(
        serde_json::to_value(&marked.step_state).expect("step state JSON")["anchor"],
        "ui/button"
    );
    recording.mark_second();

    let lab = recording.stop();

    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let entries = recording.package(&lab);
    let task = task(&entries);
    assert_eq!(
        task["anchors"],
        json!([{
            "id": "step_01",
            "template": "assets/ui_button.png",
            "region": {"mode": "rect", "rect": BUTTON},
            "provenance": {
                "source": "lab_record",
                "record_id": RECORD_ID,
                "step": 1,
                "record_index": 1,
                "self_test": "passed",
                "mark_id": "ui/button"
            }
        }])
    );
    assert_eq!(
        task["verify_templates"]
            .as_array()
            .expect("templates")
            .iter()
            .map(|target| target["id"].clone())
            .collect::<Vec<_>>(),
        [json!("ui/next")]
    );
    assert_eq!(
        task["page_rules"],
        json!({"step_02": {"required": ["ui/next"]}})
    );
    assert_eq!(
        task["operations"][0]["guard"],
        json!({
            "page_id": "step_01",
            "target_id": "page/step_01",
            "expected_rect": BUTTON,
            "verify_template": "assets/ui_button.png"
        })
    );
    let InputAction::Tap { x, y } = simulated(&recording, &entries, &first()) else {
        panic!("the first decision is not a tap");
    };
    assert!(inside(x, y, TAP), "({x}, {y})");
}

// L-02: the anchor is an own template mark the page requires: a color mark or an unknown id is
// not_own_template, a template only an any_of check names is not_required; naming and clearing
// the anchor together is refused.
#[test]
fn lab2_an_anchor_is_a_required_own_template_mark_of_its_step() {
    let recording = Recording::new();
    let color = MarkSpec::region_only(
        "state/color".to_owned(),
        MarkFamily::Color,
        rect(70, 4, 4, 4),
    );
    for (anchor, problem) in [
        ("state/color", "not_own_template"),
        ("ui/missing", "not_own_template"),
    ] {
        let error = recording
            .mark(MarkRequest {
                anchor: Some(anchor.to_owned()),
                ..recording.step(
                    "first",
                    &first(),
                    vec![template("ui/button", BUTTON), color.clone()],
                    Some(tap()),
                )
            })
            .expect_err("not an own template");
        assert_eq!(
            rejected_marks(error),
            json!([{"id": anchor, "reason": "anchor_invalid", "problem": problem}])
        );
    }

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
        .mark(MarkRequest {
            anchor: Some("ui/button".to_owned()),
            ..recording.step(
                "first",
                &first().draw(70, 40, &next()),
                vec![
                    template("ui/button", BUTTON),
                    template("ui/other", rect(70, 40, 12, 12)),
                    either,
                ],
                Some(tap()),
            )
        })
        .expect_err("an any_of member is not required");
    assert_eq!(
        rejected_marks(error),
        json!([{"id": "ui/button", "reason": "anchor_invalid", "problem": "not_required"}])
    );

    let both = recording
        .mark(MarkRequest {
            anchor: Some("ui/button".to_owned()),
            clear_anchor: true,
            ..recording.step(
                "first",
                &first(),
                vec![template("ui/button", BUTTON)],
                Some(tap()),
            )
        })
        .expect_err("an anchor is named or cleared");
    assert_eq!(both.code, "validation_failed", "{}", both.message);
}

// L-02 "swipe and drag are recorded": a drag from the click rectangle to its destination over 300
// ms is generated as a `drag` click with both rectangles; the first decision swipes from inside
// the one into the other over 300 ms; and the default task timeout counts the 300 ms
// (15000 step timeout + 300 + 200 post delay + 15000 arrival + 10000 margin).
#[test]
fn lab2_a_drag_is_generated_as_a_drag_click_from_its_rectangle_to_its_destination() {
    let recording = Recording::new();
    let marked = recording.mark_first(ClickSpec {
        region: Some(TAP),
        drag_to: Some(DRAG_TO),
        duration_ms: Some(300),
        ..ClickSpec::default()
    });
    assert_eq!(
        serde_json::to_value(&marked.click).expect("click JSON")["gesture"],
        json!({"kind": "drag", "to": DRAG_TO, "duration_ms": 300})
    );
    recording.mark_second();

    let lab = recording.stop();

    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let entries = recording.package(&lab);
    let task = task(&entries);
    assert_eq!(
        task["operations"][0]["click"],
        json!({"kind": "drag", "from": TAP, "to": DRAG_TO, "duration_ms": 300})
    );
    assert_eq!(task["timeout_ms"], 40_500);
    let InputAction::Swipe {
        x1,
        y1,
        x2,
        y2,
        duration_ms,
    } = simulated(&recording, &entries, &first())
    else {
        panic!("the first decision is not a swipe");
    };
    assert!(inside(x1, y1, TAP), "({x1}, {y1})");
    assert!(inside(x2, y2, DRAG_TO), "({x2}, {y2})");
    assert_eq!(duration_ms, 300);
}

// L-02 "long press is recorded": a long press at one point over 800 ms is generated as a
// `long_press` click at that point, which the first decision presses for 800 ms.
#[test]
fn lab2_a_long_press_is_generated_as_a_long_press_click_at_its_point() {
    let recording = Recording::new();
    let marked = recording.mark_first(ClickSpec {
        long_press: Some(RecordPoint { x: 64, y: 14 }),
        duration_ms: Some(800),
        ..ClickSpec::default()
    });
    let click = serde_json::to_value(&marked.click).expect("click JSON");
    assert_eq!(click["rect"], json!(rect(64, 14, 1, 1)));
    assert_eq!(
        click["gesture"],
        json!({"kind": "long_press", "duration_ms": 800})
    );
    recording.mark_second();

    let lab = recording.stop();

    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let entries = recording.package(&lab);
    assert_eq!(
        task(&entries)["operations"][0]["click"],
        json!({"kind": "long_press", "x": 64, "y": 14, "duration_ms": 800})
    );
    assert_eq!(
        simulated(&recording, &entries, &first()),
        InputAction::LongTap {
            x: 64,
            y: 14,
            duration_ms: 800
        }
    );
}

// L-02: a drag needs its rectangle, its destination and a duration; a long press its point and a
// duration, and nothing else; the duration is 1..=60000 ms; neither follows a template; every
// rectangle lies in the frame. Each incomplete declaration is validation_failed and nothing is
// written.
#[test]
fn lab2_a_gesture_without_its_parts_is_refused() {
    let recording = Recording::new();
    let before = recording.recording_bytes();
    let point = Some(RecordPoint { x: 64, y: 14 });
    for (case, click) in [
        (
            "drag without a duration",
            ClickSpec {
                region: Some(TAP),
                drag_to: Some(DRAG_TO),
                ..ClickSpec::default()
            },
        ),
        (
            "drag without its rectangle",
            ClickSpec {
                drag_to: Some(DRAG_TO),
                duration_ms: Some(300),
                ..ClickSpec::default()
            },
        ),
        (
            "long press with a rectangle",
            ClickSpec {
                region: Some(TAP),
                long_press: point,
                duration_ms: Some(800),
                ..ClickSpec::default()
            },
        ),
        (
            "a duration without a gesture",
            ClickSpec {
                region: Some(TAP),
                duration_ms: Some(300),
                ..ClickSpec::default()
            },
        ),
        (
            "a zero duration",
            ClickSpec {
                long_press: point,
                duration_ms: Some(0),
                ..ClickSpec::default()
            },
        ),
        (
            "a duration above 60000",
            ClickSpec {
                long_press: point,
                duration_ms: Some(60_001),
                ..ClickSpec::default()
            },
        ),
        (
            "a drag that follows a template",
            ClickSpec {
                follow: Some("ui/button".to_owned()),
                drag_to: Some(DRAG_TO),
                duration_ms: Some(300),
                ..ClickSpec::default()
            },
        ),
        (
            "a destination outside the frame",
            ClickSpec {
                region: Some(TAP),
                drag_to: Some(rect(90, 60, 10, 10)),
                duration_ms: Some(300),
                ..ClickSpec::default()
            },
        ),
    ] {
        let error = recording
            .mark(recording.step(
                "first",
                &first(),
                vec![template("ui/button", BUTTON)],
                Some(click),
            ))
            .expect_err(case);
        assert_eq!(error.code, "validation_failed", "{case}: {}", error.message);
    }
    assert_eq!(recording.recording_bytes(), before, "nothing is written");
}

// L-02 "swipe is recorded": `do --capture --record` executes a declared drag as one swipe: by
// default from the center of its rectangle to the center of its destination over its duration;
// a given swipe must start in the one, end in the other and last the declared duration; a tap is
// refused on it. The executed drag records where it ended and how long it took.
#[test]
fn lab2_do_capture_record_executes_a_declared_drag_as_one_swipe() {
    let recording = Recording::new();
    recording.mark_first(ClickSpec {
        region: Some(TAP),
        drag_to: Some(DRAG_TO),
        duration_ms: Some(300),
        ..ClickSpec::default()
    });

    let plan = recording
        .plan(PlanClickRequest::default())
        .unwrap_or_else(|error| panic!("plan: {} {:?}", error.code, error.details));
    assert_eq!(plan.point, RecordPoint { x: 64, y: 14 });
    assert_eq!(plan.to_point, Some(RecordPoint { x: 64, y: 44 }));
    assert_eq!(plan.point_rule, "rect_center");
    let swipe = |from: (i32, i32), to: (i32, i32), duration_ms: u64| PlanClickRequest {
        swipe: Some(PlannedSwipe {
            from: RecordPoint {
                x: from.0,
                y: from.1,
            },
            to: RecordPoint { x: to.0, y: to.1 },
            duration_ms,
        }),
        ..PlanClickRequest::default()
    };
    let explicit = recording
        .plan(swipe((61, 11), (66, 46), 300))
        .unwrap_or_else(|error| panic!("plan: {} {:?}", error.code, error.details));
    assert_eq!(explicit.point, RecordPoint { x: 61, y: 11 });
    assert_eq!(explicit.to_point, Some(RecordPoint { x: 66, y: 46 }));
    assert_eq!(explicit.point_rule, "explicit");
    for (request, code) in [
        (
            swipe((50, 11), (66, 46), 300),
            "record_click_outside_step_rect",
        ),
        (
            swipe((61, 11), (66, 30), 300),
            "record_click_outside_step_rect",
        ),
        (swipe((61, 11), (66, 46), 400), "record_click_rect_conflict"),
        (
            PlanClickRequest {
                tap: Some(RecordPoint { x: 64, y: 14 }),
                ..PlanClickRequest::default()
            },
            "record_click_rect_conflict",
        ),
    ] {
        let error = recording.plan(request).expect_err(code);
        assert_eq!(error.code, code, "{}", error.message);
    }

    let committed = record_commit_click(
        &recording.lock,
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
    .unwrap_or_else(|error| panic!("commit: {} {:?}", error.code, error.details));
    let execution = serde_json::to_value(&committed.click.execution).expect("execution JSON");
    assert_eq!(execution["point"], json!({"x": 64, "y": 14}));
    assert_eq!(execution["to_point"], json!({"x": 64, "y": 44}));
    assert_eq!(execution["duration_ms"], 300);
    assert!(committed.step_closed);
}

// L-02 "long press is recorded": `do --capture --record` holds a declared long press at its point
// (a swipe that ends where it began); a swipe is refused on it, and on a tap step; a swipe on a
// step without a declared click asks for the drag first.
#[test]
fn lab2_do_capture_record_holds_a_long_press_and_swipes_only_a_declared_drag() {
    let swipe = PlanClickRequest {
        swipe: Some(PlannedSwipe {
            from: RecordPoint { x: 64, y: 14 },
            to: RecordPoint { x: 64, y: 44 },
            duration_ms: 300,
        }),
        ..PlanClickRequest::default()
    };

    let pressed = Recording::new();
    pressed.mark_first(ClickSpec {
        long_press: Some(RecordPoint { x: 64, y: 14 }),
        duration_ms: Some(800),
        ..ClickSpec::default()
    });
    let plan = pressed
        .plan(PlanClickRequest::default())
        .unwrap_or_else(|error| panic!("plan: {} {:?}", error.code, error.details));
    assert_eq!(plan.point, RecordPoint { x: 64, y: 14 });
    assert_eq!(plan.to_point, Some(RecordPoint { x: 64, y: 14 }));
    assert_eq!(
        serde_json::to_value(plan.gesture).expect("gesture JSON"),
        json!({"kind": "long_press", "duration_ms": 800})
    );
    let error = pressed
        .plan(swipe.clone())
        .expect_err("a long press is no drag");
    assert_eq!(
        error.code, "record_click_rect_conflict",
        "{}",
        error.message
    );

    let tapped = Recording::new();
    tapped.mark_first(tap());
    let error = tapped.plan(swipe.clone()).expect_err("a tap is no drag");
    assert_eq!(
        error.code, "record_click_rect_conflict",
        "{}",
        error.message
    );

    let undeclared = Recording::new();
    undeclared
        .mark(undeclared.step("first", &first(), vec![template("ui/button", BUTTON)], None))
        .unwrap_or_else(|error| panic!("step 1: {} {:?}", error.code, error.details));
    let error = undeclared.plan(swipe).expect_err("no drag is declared");
    assert_eq!(error.code, "record_step_click_missing", "{}", error.message);
}

// L-02 "marks after stop, regeneration": a stopped recording takes a mark on step 2; it records
// the change, and the next `record stop` generates the package again with the new mark
// (`regenerated`, the same package id), after which a further stop has nothing new to generate.
#[test]
fn lab2_a_stopped_recording_marked_again_generates_its_package_again() {
    let recording = Recording::new();
    recording.mark_first(tap());
    recording.mark_second();
    let generated = recording.stop_session();
    assert_eq!(generated["lab_status"], "generated", "{generated}");
    assert!(
        task(&recording.package(&generated))
            .get("color_probes")
            .is_none()
    );

    let marked = recording
        .mark(MarkRequest {
            schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
            step: Some(2),
            add: vec![MarkSpec::region_only(
                "state/next".to_owned(),
                MarkFamily::Color,
                rect(80, 4, 4, 4),
            )],
            ..MarkRequest::default()
        })
        .unwrap_or_else(|error| panic!("mark after stop: {} {:?}", error.code, error.details));
    assert!(marked.after_stop);
    let amended = recording.recording();
    assert_eq!(amended["status"], "stopped");
    assert!(amended["amended_at_unix_ms"].is_u64(), "{amended}");

    let regenerated = recording.stop_session();

    assert_eq!(regenerated["lab_status"], "regenerated", "{regenerated}");
    assert_eq!(regenerated["package_id"], generated["package_id"]);
    let task = task(&recording.package(&regenerated));
    assert_eq!(task["color_probes"][0]["id"], "state/next");
    assert_eq!(
        task["page_rules"]["step_02"],
        json!({"required": ["ui/next", "state/next"]})
    );
    let stopped = recording.recording();
    assert!(stopped.get("amended_at_unix_ms").is_none(), "{stopped}");
    let again = recording.stop_session();
    assert_eq!(again["lab_status"], "already_generated", "{again}");
}

// L-02: a stopped recording takes no new step and no step operation: both need an active
// recording (record_session_not_active, reason recording_stopped), and nothing is written.
#[test]
fn lab2_a_stopped_recording_takes_no_new_step_and_no_step_operation() {
    let recording = Recording::new();
    recording.mark_first(tap());
    recording.mark_second();
    recording.stop_session();
    let before = recording.recording_bytes();

    let new_step = recording
        .mark(recording.step(
            "third",
            &Gray::frame().draw(4, 4, &next()),
            vec![template("ui/third", rect(4, 4, 12, 12))],
            None,
        ))
        .expect_err("a new step needs an active recording");
    let operation = recording
        .mark(MarkRequest {
            schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
            step_action: Some(StepAction {
                kind: StepActionKind::DropStep,
                step: Some(2),
            }),
            ..MarkRequest::default()
        })
        .expect_err("a step operation needs an active recording");

    for error in [new_step, operation] {
        assert_eq!(error.code, "record_session_not_active", "{}", error.message);
        assert_eq!(
            error.details.expect("details")["reason"],
            "recording_stopped"
        );
    }
    assert_eq!(recording.recording_bytes(), before, "nothing is written");
}
