// SPDX-License-Identifier: AGPL-3.0-only

// #463 S2-1 (contracts/lab-recording.md, "What a recording still lacks: `record ready`"; register
// §1.425): one read lists everything a recording still lacks before `record stop` can make its
// package. Recordings are made as stage one makes them, `tap --record` through `record_press` with
// a fake Runtime (its input answer, its captures, the caller's clock moving only by its waits),
// then marked with `record mark`; `record_ready` reads them under no lock.
// - `s21_three_things_missing_are_listed_in_order_and_record_stop_answers_the_first`: a recording
//   whose server and locale are not known and whose step 2 has no mark lists `server`, `locale`
//   (the recording's own, with their hints) and then step 2's `page_recognition`, `missing_total`
//   3, every self-check in `checks_after`; `record stop` answers the first pre-check the generator
//   meets, `record_step_without_recognition` for step 2, with `missing_total` 3 and the hint
//   `record ready`.
// - `s21_a_complete_recording_is_ready_and_every_self_check_ran`: both steps marked: `ready`,
//   nothing missing, `checks_after` empty, and `record stop --dry-run` validates the package.
// - `s21_a_recording_that_lacks_something_names_the_self_checks_and_runs_none`: the locale not
//   known and step 1's frame changed on disk: only `locale` is missing and every self-check is in
//   `checks_after`; with the locale given, `record stop --dry-run` is refused by the frames check,
//   which `record ready` did not run.
// - `s21_the_first_stage_two_write_moves_material_to_draft`: presses only: `material`, each step's
//   `page_recognition`; one mark on step 1: `draft`, step 2's alone.
// - `s21_packaged_after_record_stop_and_draft_again_after_a_later_mark`: after `record stop`
//   `packaged` (session `stopped`), nothing missing; the last step marked optional after the stop:
//   `draft` with its `optional_final_step`.
// - `s21_press_point_only_until_a_press_region_is_set`: two taps, the second indeterminate: each
//   step's press is `press_point_only`; the last step reopened and its click declared with a
//   region: step 1's alone.
// - `s21_the_read_writes_nothing_and_takes_no_lock`: while the recording's lock is held, the read
//   answers (its self-checks ran) and every file of the recording keeps its bytes.
// Fix round 2 (the review of pull request #1766): an item carries what its refusal's details say
// beside its own members (the first and third tests: `missing` of the identity parts not known):
// - `s21_an_option_out_of_its_form_after_a_missing_item_does_not_stop_the_count`: step 2 without a
//   mark, step 1 forbidding a mark no step has (written into the step table), `record stop` with
//   `--package-id` and `--requires` out of their form: it answers step 2's
//   `record_step_without_recognition` with `missing_total` 2; `record ready` lists both, step 1's
//   `forbidden_mark_missing` with its mark.
// - `s21_two_items_of_one_word_on_one_step_differ_by_their_details`: step 2 reusing two marks no
//   step has: two `reused_mark_missing` of step 2, each with its own mark.
// - `s21_a_reopened_last_step_names_the_frame_of_its_last_attempt`: the indeterminate press of the
//   last step moved aside by `--reopen-step`: its items and its `press_point_only` name f0003, the
//   frame of the click's last attempt.
// The bench's press is `press` (it was `tap`, a name the architecture guard reads as an input
// port's method); it goes through `record_press` as the parent's gates send theirs.
// #463 stage two, train 1 (`record ready` meets S2-5's boxes), on the recording S2-5's test of the
// whole of stage two makes, with its bench (`gate_463_s25_draft`: 160x90 screens of gray noise):
// - `s21_train_a_press_box_ends_the_press_point_only_of_its_step`: the three recorded taps are
//   `press_point_only`; a press box on step 2 ends step 2's alone.
// - `s21_train_the_draft_boxed_on_every_step_is_ready_and_material_before`: archived and reopened,
//   `material`; boxed on every step as that test boxes it, `ready` with nothing missing, every
//   self-check run and no `press_point_only`.
// Frames are synthetic 1280x720 (a square of one colour at the top left on gray); no Runtime, no
// device; the tests read no clock and never sleep.

use crate::gate_463_s25_draft as draft;
use crate::recording::store::{lab_dir, old_record_path, recording_path};
use crate::{
    AfterCapture, AttachFrameRequest, CapturedFrame, ClickEffect, ClickPlan, ClickSpec,
    CommitClickRequest, LAB_RECORD_MARK_SCHEMA, LabOcr, MarkFamily, MarkRequest, MarkSpec,
    OpaqueJson, PlanClickRequest, PointPress, RecordArchiveOptions, RecordFramePorts, RecordPoint,
    RecordPressOutcome, RecordPressRequest, RecordRect, RecordReopenOptions, RecordStartOptions,
    RecordStopOptions, RecordingLock, StepAction, StepActionKind, record_archive, record_mark,
    record_press, record_ready, record_reopen, record_start, record_stop,
};
use actingcommand_contract::{InputFrameReference, LabError, LabResult};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use serde_json::{Value, json};
use std::cell::Cell;
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_0463_s21";
const TASK_ID: &str = "neutral_task";
const STARTED_AT: u64 = 1_700_000_000_000;
const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;
const GRAY: [u8; 3] = [60, 60, 60];
/// The colour of each screen's square.
const COLORS: [[u8; 3]; 3] = [[230, 20, 20], [20, 230, 20], [20, 20, 230]];
/// Each screen's square, a colour mark's region.
const SQUARE: RecordRect = rect(0, 0, 40, 40);
/// The self-checks of `record stop`, in the order they run.
const EVERY_CHECK: [&str; 7] = [
    "frames",
    "admission",
    "first_decision",
    "cross_check",
    "endings",
    "selects",
    "final_package",
];

const fn rect(x: i32, y: i32, width: i32, height: i32) -> RecordRect {
    RecordRect {
        x,
        y,
        width,
        height,
    }
}

// The PNG of screen `index`: its square of colour at the top left on gray.
fn screen(index: usize) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            pixels.extend(if x < 40 && y < 40 {
                COLORS[index]
            } else {
                GRAY
            });
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

fn opaque(value: Value) -> Option<OpaqueJson> {
    Some(OpaqueJson::from_serializable(&value).expect("opaque value"))
}

// What the command hands the recording for the input receipt: no carrier package, no Runtime
// frames.
fn receipt(effect: ClickEffect) -> CommitClickRequest {
    CommitClickRequest {
        effect,
        has_failure: false,
        carrier_package: None,
        req_id: opaque(json!("request-s21")),
        correlation_id: opaque(json!("request-s21-correlation")),
        action_id: match effect {
            ClickEffect::Performed => opaque(json!("request-s21-action")),
            ClickEffect::Indeterminate => None,
        },
        lease_id: None,
        failure: None,
        before: None,
        after: None,
    }
}

/// The fake Runtime's capture and the caller's clock: the screens its captures take, in order, and
/// how many it took (each one's frame reference its own); the clock moves only by the waits.
struct FakeDevice<'a> {
    screens: VecDeque<usize>,
    count: &'a Cell<u32>,
    now: Duration,
}

impl RecordFramePorts for FakeDevice<'_> {
    fn wait(&mut self, duration: Duration) {
        self.now += duration;
    }

    fn elapsed(&mut self) -> Duration {
        self.now
    }

    fn capture(&mut self) -> LabResult<CapturedFrame> {
        let index = self
            .screens
            .pop_front()
            .expect("a capture the test did not script");
        self.count.set(self.count.get() + 1);
        Ok(CapturedFrame {
            frame: AttachFrameRequest {
                png: screen(index),
                source: "lab_capture".to_owned(),
                runtime_artifact: None,
                capture_backend: Some("adb_screencap".to_owned()),
                freshness: opaque(json!({"required": false})),
            },
            input_frame: Some(InputFrameReference {
                frame_id: serde_json::from_value(json!(format!("frame_{:032x}", self.count.get())))
                    .expect("a frame id"),
                width: WIDTH,
                height: HEIGHT,
            }),
        })
    }
}

// A state directory with an active session of `INSTANCE` (the header `record start` writes) and its
// Lab recording, before any step, started with `start`. The lock is held as one command holds it
// and dropped before the directory.
struct Bench {
    lock: RecordingLock,
    ocr: LabOcr,
    temp: TempDir,
    /// The fake Runtime's captures of this bench, so that each capture's frame reference is its
    /// own across presses.
    captures: Cell<u32>,
}

impl Bench {
    fn new(start: RecordStartOptions) -> Self {
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
        let lock = RecordingLock::acquire(&state, INSTANCE, "gate_463_s21", "Lab-CLI")
            .expect("recording lock");
        let ocr = LabOcr::from_vision_root(&temp.path().join("no-vision"));
        record_start(&lock, &start).expect("record start");
        Self {
            lock,
            ocr,
            temp,
            captures: Cell::new(0),
        }
    }

    // A recording whose game, server and locale are known from `record start`.
    fn known() -> Self {
        Self::new(RecordStartOptions {
            game: Some("neutral".to_owned()),
            server: Some("test".to_owned()),
            locale: Some("en-US".to_owned()),
            ..RecordStartOptions::default()
        })
    }

    fn state(&self) -> PathBuf {
        self.temp.path().join("state")
    }

    // `tap <x> <y> --record`, the fake Runtime answering `effect` to the input and its captures
    // taking `screens`; the frame after it one capture 800 ms after the press (no stillness test).
    // Fix round 2: the press goes through `record_press` as the parent's gates send it, and the
    // bench's method is no longer named as an input port's method is.
    fn press(
        &self,
        x: i32,
        y: i32,
        effect: ClickEffect,
        screens: &[usize],
    ) -> LabResult<RecordPressOutcome> {
        let request = RecordPressRequest {
            click: PlanClickRequest::default(),
            point_press: Some(PointPress::Tap {
                point: RecordPoint { x, y },
            }),
            after: AfterCapture {
                still_ms: Some(0),
                ..AfterCapture::for_press(Some(800))
            },
        };
        let mut device = FakeDevice {
            screens: screens.iter().copied().collect(),
            count: &self.captures,
            now: Duration::ZERO,
        };
        let mut press = |_: &ClickPlan, _: InputFrameReference| -> LabResult<CommitClickRequest> {
            Ok(receipt(effect))
        };
        record_press(&self.lock, &request, &mut press, &mut device)
    }

    // A performed tap at (100, 200): step 1 its screen 0, step 2 the screen 1 after it.
    fn performed_tap(&self) {
        self.press(100, 200, ClickEffect::Performed, &[0, 1])
            .unwrap_or_else(|error| panic!("the tap: {}: {:?}", error.code, error.details));
    }

    // Fix round 2: the step table as `edit` leaves it, written in place, as a stage-two write
    // that dropped the steps whose marks it names would leave it.
    fn edit_recording(&self, edit: impl FnOnce(&mut Value)) {
        let mut recording = self.recording();
        edit(&mut recording);
        fs::write(
            recording_path(&self.state(), RECORD_ID),
            serde_json::to_vec(&recording).expect("step table JSON"),
        )
        .expect("step table");
    }

    fn mark_with(&self, request: MarkRequest) {
        record_mark(&self.lock, &request, false, &self.ocr).unwrap_or_else(|error| {
            panic!(
                "record mark: {}: {} {:?}",
                error.code, error.message, error.details
            )
        });
    }

    // `record mark --step <step> --color <id>=0,0,40,40`.
    fn mark(&self, step: u32, id: &str) {
        self.mark_with(MarkRequest {
            schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
            step: Some(step),
            add: vec![MarkSpec::region_only(
                id.to_owned(),
                MarkFamily::Color,
                SQUARE,
            )],
            ..MarkRequest::default()
        });
    }

    // `record ready`, read as the command reads it: the instance's recording, no option.
    fn ready(&self) -> Value {
        read_ready(&self.state(), INSTANCE, &self.ocr)
    }

    // `record stop` with `options`.
    fn stop(&self, options: &RecordStopOptions) -> LabResult<Value> {
        record_stop(&self.lock, options, &self.ocr).map(|outcome| {
            let mut lab = serde_json::to_value(&outcome.lab).expect("lab JSON");
            lab["lab_status"] = json!(outcome.lab_status);
            lab
        })
    }

    // The session file's status as the `record` command writes it after `record stop`, so that the
    // stopped recording takes `record mark` ("Marking after stop").
    fn session_status(&self, status: &str) {
        let path = old_record_path(&self.state(), INSTANCE);
        let mut session: Value =
            serde_json::from_slice(&fs::read(&path).expect("session file")).expect("session");
        session["status"] = json!(status);
        fs::write(&path, serde_json::to_vec(&session).expect("session JSON"))
            .expect("session file");
    }

    fn recording(&self) -> Value {
        serde_json::from_slice(
            &fs::read(recording_path(&self.state(), RECORD_ID)).expect("step table"),
        )
        .expect("step table JSON")
    }

    // Every file of the recording's directory, by its `/`-separated path relative to it, with its
    // bytes, in name order.
    fn files(&self) -> Vec<(String, Vec<u8>)> {
        let mut files = Vec::new();
        let mut pending = vec![(lab_dir(&self.state(), RECORD_ID), String::new())];
        while let Some((path, prefix)) = pending.pop() {
            for entry in fs::read_dir(&path).expect("directory") {
                let entry = entry.expect("entry");
                let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
                if entry.file_type().expect("file type").is_dir() {
                    pending.push((entry.path(), format!("{name}/")));
                } else {
                    files.push((name, fs::read(entry.path()).expect("file")));
                }
            }
        }
        files.sort();
        files
    }
}

// `record ready` of the recording the session header of `instance` names under `state`, read as the
// command reads it: no option. #463 stage two, train 1: S2-5's bench reads it so too.
fn read_ready(state: &Path, instance: &str, ocr: &LabOcr) -> Value {
    let outcome = record_ready(state, instance, None, &RecordStopOptions::default(), ocr)
        .unwrap_or_else(|error| {
            panic!(
                "record ready: {}: {} {:?}",
                error.code, error.message, error.details
            )
        });
    serde_json::to_value(outcome).expect("the answer as JSON")
}

// The refusal of `record stop`, or the test's panic with what it answered instead.
fn refused(stopped: LabResult<Value>) -> LabError {
    stopped.expect_err("record stop refuses")
}

// `press_point_only` for the step `step`, its number in the package `step` too, pressed on
// `frame_id`.
fn point_only(step: u32, frame_id: &str) -> Value {
    json!({"step": step, "artifact_step": step, "frame_id": frame_id, "what": "press_point_only"})
}

#[test]
fn s21_three_things_missing_are_listed_in_order_and_record_stop_answers_the_first() {
    let bench = Bench::new(RecordStartOptions {
        game: Some("neutral".to_owned()),
        ..RecordStartOptions::default()
    });
    bench.performed_tap();
    bench.mark(1, "first_square");

    let ready = bench.ready();
    assert_eq!(ready["status"], "draft", "{ready}");
    assert_eq!(ready["record_id"], RECORD_ID, "{ready}");
    assert_eq!(ready["session"], "active", "{ready}");
    assert_eq!(
        ready["missing"],
        json!([
            {"order": 1, "step": null, "artifact_step": null, "frame_id": null, "what": "server",
             "code": "record_locale_missing", "hint": "record stop --server <s>",
             "details": {"missing": ["server", "locale"]}},
            {"order": 2, "step": null, "artifact_step": null, "frame_id": null, "what": "locale",
             "code": "record_locale_missing", "hint": "record stop --locale <l>",
             "details": {"missing": ["server", "locale"]}},
            {"order": 3, "step": 2, "artifact_step": 2, "frame_id": null,
             "what": "page_recognition", "code": "record_step_without_recognition",
             "hint": "record mark --step 2 --template <id>=x,y,w,h"}
        ]),
        "{ready}"
    );
    assert_eq!(ready["missing_total"], 3, "{ready}");
    assert_eq!(ready["checks_after"], json!(EVERY_CHECK), "{ready}");
    assert_eq!(
        ready["warnings"],
        json!([point_only(1, "f0001")]),
        "{ready}"
    );

    // `record stop` answers the first pre-check the generator meets, as before: step 2's marks
    // are checked before the package's identity.
    let error = refused(bench.stop(&RecordStopOptions::default()));
    assert_eq!(
        error.code, "record_step_without_recognition",
        "{}",
        error.message
    );
    let details = error.details.clone().expect("details");
    assert_eq!(details["step"], 2, "{details}");
    assert_eq!(details["missing_total"], 3, "{details}");
    assert_eq!(details["hint"], "record ready", "{details}");
}

#[test]
fn s21_a_complete_recording_is_ready_and_every_self_check_ran() {
    let bench = Bench::known();
    bench.performed_tap();
    bench.mark(1, "first_square");
    bench.mark(2, "last_square");

    let ready = bench.ready();
    assert_eq!(ready["status"], "ready", "{ready}");
    assert_eq!(ready["session"], "active", "{ready}");
    assert_eq!(ready["missing"], json!([]), "{ready}");
    assert_eq!(ready["missing_total"], 0, "{ready}");
    assert_eq!(ready["checks_after"], json!([]), "{ready}");
    assert_eq!(ready["warnings"][0], point_only(1, "f0001"), "{ready}");

    // `record stop` would write the package.
    let checked = bench
        .stop(&RecordStopOptions {
            dry_run: true,
            ..RecordStopOptions::default()
        })
        .unwrap_or_else(|error| panic!("record stop: {}: {}", error.code, error.message));
    assert_eq!(checked["lab_status"], "validated", "{checked}");
}

#[test]
fn s21_a_recording_that_lacks_something_names_the_self_checks_and_runs_none() {
    let bench = Bench::new(RecordStartOptions {
        game: Some("neutral".to_owned()),
        server: Some("test".to_owned()),
        ..RecordStartOptions::default()
    });
    bench.performed_tap();
    bench.mark(1, "first_square");
    bench.mark(2, "last_square");
    // Step 1's frame no longer holds its recorded bytes, which the frames check would refuse.
    let recording = bench.recording();
    let path = recording["steps"][0]["frames"]
        .as_array()
        .expect("step 1's frames")
        .iter()
        .find(|frame| frame["frame_id"] == "f0001")
        .and_then(|frame| frame["path"].as_str())
        .map(PathBuf::from)
        .expect("step 1's frame f0001");
    fs::write(&path, screen(2)).expect("a changed frame");

    let ready = bench.ready();
    assert_eq!(ready["status"], "draft", "{ready}");
    assert_eq!(
        ready["missing"],
        json!([
            {"order": 1, "step": null, "artifact_step": null, "frame_id": null, "what": "locale",
             "code": "record_locale_missing", "hint": "record stop --locale <l>",
             "details": {"missing": ["locale"]}}
        ]),
        "{ready}"
    );
    assert_eq!(ready["missing_total"], 1, "{ready}");
    assert_eq!(ready["checks_after"], json!(EVERY_CHECK), "{ready}");

    // With the locale given, the frames check runs and refuses: `record ready` ran none.
    let error = refused(bench.stop(&RecordStopOptions {
        locale: Some("en-US".to_owned()),
        dry_run: true,
        ..RecordStopOptions::default()
    }));
    assert_eq!(
        error.code, "record_frame_hash_mismatch",
        "{}",
        error.message
    );
}

#[test]
fn s21_the_first_stage_two_write_moves_material_to_draft() {
    let bench = Bench::known();
    bench.performed_tap();

    let material = bench.ready();
    assert_eq!(material["status"], "material", "{material}");
    assert_eq!(
        material["missing"],
        json!([
            {"order": 1, "step": 1, "artifact_step": 1, "frame_id": "f0001",
             "what": "page_recognition", "code": "record_step_without_recognition",
             "hint": "record mark --step 1 --template <id>=x,y,w,h"},
            {"order": 2, "step": 2, "artifact_step": 2, "frame_id": null,
             "what": "page_recognition", "code": "record_step_without_recognition",
             "hint": "record mark --step 2 --template <id>=x,y,w,h"}
        ]),
        "{material}"
    );
    assert_eq!(material["checks_after"], json!(EVERY_CHECK), "{material}");

    bench.mark(1, "first_square");
    let draft = bench.ready();
    assert_eq!(draft["status"], "draft", "{draft}");
    assert_eq!(draft["missing_total"], 1, "{draft}");
    assert_eq!(draft["missing"][0]["step"], 2, "{draft}");
    assert_eq!(draft["missing"][0]["what"], "page_recognition", "{draft}");
}

#[test]
fn s21_packaged_after_record_stop_and_draft_again_after_a_later_mark() {
    let bench = Bench::known();
    bench.performed_tap();
    bench.mark(1, "first_square");
    bench.mark(2, "last_square");
    let generated = bench
        .stop(&RecordStopOptions::default())
        .unwrap_or_else(|error| panic!("record stop: {}: {}", error.code, error.message));
    assert_eq!(generated["lab_status"], "generated", "{generated}");
    bench.session_status("stopped");

    let packaged = bench.ready();
    assert_eq!(packaged["status"], "packaged", "{packaged}");
    assert_eq!(packaged["session"], "stopped", "{packaged}");
    assert_eq!(packaged["missing"], json!([]), "{packaged}");
    assert_eq!(packaged["checks_after"], json!([]), "{packaged}");
    assert_eq!(
        packaged["warnings"],
        json!([point_only(1, "f0001")]),
        "{packaged}"
    );

    // Marked again after the stop: the last step made optional, a place it cannot be.
    bench.mark_with(MarkRequest {
        schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
        step: Some(2),
        optional: Some(true),
        ..MarkRequest::default()
    });
    let draft = bench.ready();
    assert_eq!(draft["status"], "draft", "{draft}");
    assert_eq!(draft["session"], "stopped", "{draft}");
    assert_eq!(
        draft["missing"],
        json!([
            {"order": 1, "step": 2, "artifact_step": 2, "frame_id": null,
             "what": "optional_final_step", "code": "record_optional_final_step",
             "hint": "record mark --step 2 --not-optional"}
        ]),
        "{draft}"
    );
    assert_eq!(draft["checks_after"], json!(EVERY_CHECK), "{draft}");
}

#[test]
fn s21_press_point_only_until_a_press_region_is_set() {
    let bench = Bench::known();
    bench.performed_tap();
    // A second tap on step 2 whose outcome is not known: the step stays open with its click.
    let error = bench
        .press(300, 400, ClickEffect::Indeterminate, &[2])
        .expect_err("an indeterminate press");
    assert_eq!(
        error.code, "record_click_indeterminate",
        "{}",
        error.message
    );

    let pressed = bench.ready();
    assert_eq!(
        pressed["warnings"],
        json!([point_only(1, "f0001"), point_only(2, "f0003")]),
        "{pressed}"
    );

    // The last step reopened, and its click declared with a region.
    bench.mark_with(MarkRequest {
        schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
        step_action: Some(StepAction {
            kind: StepActionKind::ReopenStep,
            step: Some(2),
        }),
        ..MarkRequest::default()
    });
    bench.mark_with(MarkRequest {
        schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
        step: Some(2),
        add: vec![MarkSpec::region_only(
            "pressed_square".to_owned(),
            MarkFamily::Color,
            SQUARE,
        )],
        click: Some(ClickSpec {
            region: Some(rect(280, 390, 40, 20)),
            ..ClickSpec::default()
        }),
        replace_click: true,
        ..MarkRequest::default()
    });
    let region = bench.ready();
    assert_eq!(
        region["warnings"],
        json!([point_only(1, "f0001")]),
        "{region}"
    );
}

#[test]
fn s21_the_read_writes_nothing_and_takes_no_lock() {
    // The bench holds the recording's lock, as a command that writes holds it, all along.
    let bench = Bench::known();
    bench.performed_tap();
    bench.mark(1, "first_square");
    bench.mark(2, "last_square");
    let before = bench.files();

    let ready = bench.ready();
    assert_eq!(ready["status"], "ready", "{ready}");
    assert_eq!(ready["checks_after"], json!([]), "{ready}");
    assert_eq!(bench.files(), before, "a file of the recording changed");
}

#[test]
fn s21_an_option_out_of_its_form_after_a_missing_item_does_not_stop_the_count() {
    let bench = Bench::known();
    bench.performed_tap();
    bench.mark(1, "first_square");
    // Step 1 forbids a mark no effective step has: a pre-check the generator makes after it
    // checks the options' form.
    bench.edit_recording(|recording| {
        recording["steps"][0]["forbidden"] = json!(["gone_mark"]);
    });

    // `--package-id` and `--requires` out of their form, met after step 2's missing mark: that
    // mark is the answer, as before, and the generator goes on past both options.
    let error = refused(bench.stop(&RecordStopOptions {
        package_id: Some("not/one/segment".to_owned()),
        requires: Some(String::new()),
        ..RecordStopOptions::default()
    }));
    assert_eq!(
        error.code, "record_step_without_recognition",
        "{}",
        error.message
    );
    let details = error.details.clone().expect("details");
    assert_eq!(details["step"], 2, "{details}");
    assert_eq!(details["missing_total"], 2, "{details}");
    assert_eq!(details["hint"], "record ready", "{details}");

    // `record ready` lists the same two.
    let ready = bench.ready();
    assert_eq!(
        ready["missing"],
        json!([
            {"order": 1, "step": 1, "artifact_step": 1, "frame_id": "f0001",
             "what": "forbidden_mark_missing", "code": "record_mark_rejected",
             "details": {"marks": [{"id": "gone_mark", "reason": "forbidden_source_missing"}]}},
            {"order": 2, "step": 2, "artifact_step": 2, "frame_id": null,
             "what": "page_recognition", "code": "record_step_without_recognition",
             "hint": "record mark --step 2 --template <id>=x,y,w,h"}
        ]),
        "{ready}"
    );
    assert_eq!(ready["missing_total"], 2, "{ready}");
}

#[test]
fn s21_two_items_of_one_word_on_one_step_differ_by_their_details() {
    let bench = Bench::known();
    bench.performed_tap();
    bench.mark(1, "first_square");
    bench.mark(2, "last_square");
    // Step 2's page reuses two marks no effective step has.
    bench.edit_recording(|recording| {
        recording["steps"][1]["reused"] = json!(["gone_first", "gone_second"]);
    });

    let ready = bench.ready();
    assert_eq!(
        ready["missing"],
        json!([
            {"order": 1, "step": 2, "artifact_step": 2, "frame_id": null,
             "what": "reused_mark_missing", "code": "record_mark_rejected",
             "details": {"marks": [{"id": "gone_first", "reason": "reuse_source_missing"}]}},
            {"order": 2, "step": 2, "artifact_step": 2, "frame_id": null,
             "what": "reused_mark_missing", "code": "record_mark_rejected",
             "details": {"marks": [{"id": "gone_second", "reason": "reuse_source_missing"}]}}
        ]),
        "{ready}"
    );
    assert_ne!(ready["missing"][0], ready["missing"][1], "{ready}");
    assert_eq!(ready["missing_total"], 2, "{ready}");
}

#[test]
fn s21_a_reopened_last_step_names_the_frame_of_its_last_attempt() {
    let bench = Bench::known();
    bench.performed_tap();
    let error = bench
        .press(300, 400, ClickEffect::Indeterminate, &[2])
        .expect_err("an indeterminate press");
    assert_eq!(
        error.code, "record_click_indeterminate",
        "{}",
        error.message
    );
    // The last step reopened: its press, sent on f0003, is moved aside as its click's last attempt.
    bench.mark_with(MarkRequest {
        schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
        step_action: Some(StepAction {
            kind: StepActionKind::ReopenStep,
            step: Some(2),
        }),
        ..MarkRequest::default()
    });
    let recording = bench.recording();
    assert_eq!(
        recording["steps"][1]["click"]["execution"],
        Value::Null,
        "{recording}"
    );

    let ready = bench.ready();
    assert_eq!(
        ready["missing"],
        json!([
            {"order": 1, "step": 1, "artifact_step": 1, "frame_id": "f0001",
             "what": "page_recognition", "code": "record_step_without_recognition",
             "hint": "record mark --step 1 --template <id>=x,y,w,h"},
            {"order": 2, "step": 2, "artifact_step": 2, "frame_id": "f0003",
             "what": "final_has_effect", "code": "record_final_step_has_click"},
            {"order": 3, "step": 2, "artifact_step": 2, "frame_id": "f0003",
             "what": "page_recognition", "code": "record_step_without_recognition",
             "hint": "record mark --step 2 --template <id>=x,y,w,h"}
        ]),
        "{ready}"
    );
    assert_eq!(
        ready["warnings"],
        json!([point_only(1, "f0001"), point_only(2, "f0003")]),
        "{ready}"
    );
}

// #463 stage two, train 1: the recording S2-5's test of the whole of stage two makes
// (`gate_463_s25_draft`): three taps recorded by their points on its first three screens, each
// screen after one capture, and the fourth screen the final one. Steps 1 to 3 are sent on f0001,
// f0003 and f0005.
fn draft_recording() -> draft::Bench {
    let bench = draft::Bench::new();
    for (index, point) in draft::POINTS.into_iter().enumerate() {
        bench.press(
            &draft::tap_at(point, draft::one_capture_after()),
            vec![draft::screen(index), draft::screen(index + 1)],
        );
    }
    bench
}

// #463 stage two, train 1: `record ready` meets S2-5's boxes. Each recorded tap is
// `press_point_only`; a press box drawn on the middle step (S2-5: the click's rectangle becomes the
// box, the press stays as recorded) ends that step's warning, and only that one.
#[test]
fn s21_train_a_press_box_ends_the_press_point_only_of_its_step() {
    let bench = draft_recording();
    let pressed = read_ready(&bench.state(), draft::INSTANCE, &bench.ocr);
    assert_eq!(
        pressed["warnings"],
        json!([
            point_only(1, "f0001"),
            point_only(2, "f0003"),
            point_only(3, "f0005")
        ]),
        "{pressed}"
    );

    bench.marked(json!({"step": 2, "boxes": [draft::button_box("press", 1)]}));
    let boxed = read_ready(&bench.state(), draft::INSTANCE, &bench.ocr);
    assert_eq!(
        boxed["warnings"],
        json!([point_only(1, "f0001"), point_only(3, "f0005")]),
        "{boxed}"
    );
}

// #463 stage two, train 1: the draft of S2-5's test of the whole of stage two. Archived and
// reopened, before any mark, it is `material`; marked as that test marks it (on each step a page
// name, a recognize box and, on the three taps, a press box), it is `ready`: nothing missing, every
// self-check run, no press left on one pixel.
#[test]
fn s21_train_the_draft_boxed_on_every_step_is_ready_and_material_before() {
    let bench = draft_recording();
    record_archive(
        &bench.lock,
        &RecordArchiveOptions {
            record_id: None,
            copy_to: None,
            written_by: "gate_463_s21".to_owned(),
        },
    )
    .unwrap_or_else(|error| panic!("record archive: {}: {:?}", error.code, error.details));
    bench.header("archived");
    record_reopen(&bench.lock, &RecordReopenOptions::default())
        .unwrap_or_else(|error| panic!("record reopen: {}: {:?}", error.code, error.details));
    bench.header("active");

    let material = read_ready(&bench.state(), draft::INSTANCE, &bench.ocr);
    assert_eq!(material["status"], "material", "{material}");

    for (index, page) in ["first", "second", "third", "done"].into_iter().enumerate() {
        let step = u32::try_from(index + 1).expect("step");
        let mut boxes = vec![draft::button_box("recognize", index)];
        if index < 3 {
            boxes.push(draft::button_box("press", index));
        }
        bench.marked(json!({"step": step, "page": page, "boxes": boxes}));
    }
    let ready = read_ready(&bench.state(), draft::INSTANCE, &bench.ocr);
    assert_eq!(ready["status"], "ready", "{ready}");
    assert_eq!(ready["missing"], json!([]), "{ready}");
    assert_eq!(ready["missing_total"], 0, "{ready}");
    assert_eq!(ready["checks_after"], json!([]), "{ready}");
    let warnings = ready["warnings"].as_array().expect("warnings");
    assert!(
        warnings
            .iter()
            .all(|warning| warning["what"] != "press_point_only"),
        "{ready}"
    );
}
