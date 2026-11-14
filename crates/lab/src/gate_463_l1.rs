// SPDX-License-Identifier: AGPL-3.0-only

// #463 L1 (contracts/lab-recording.md, "One call per step"; register §1.370): `do --capture
// --record` records a whole step in one call. On a recording of device frames, attached as
// `capture --record` attaches them, `record_plan_step` applies the step's mark request with `record
// mark`'s own code and plans the press with `do --capture --record`'s, and `PlannedStep::perform`
// writes the marks, presses through a fake Runtime (`RecordStepPorts`), records its answer as
// `do --capture --record` records it and, when asked, waits on the fake's clock and takes the next
// frame, which arrives as `capture --record`'s does.
// - `l1_one_call_records_the_step_the_three_calls_record`: the step table after one call equals,
//   record for record, the one `capture --record`, `record mark` and `do --capture --record` write
//   for the same step (times and the temporary directory removed), and so do the frame and crop
//   files; the one rectangle given once is the template's crop and the click (`click.from`).
// - `l1_a_refused_mark_presses_nothing_and_writes_nothing`: a mark whose self-test fails, a click
//   outside the frame and a request with an unknown member are refused with `record mark`'s code;
//   the fake Runtime saw no input and every file of the recording keeps its bytes.
// - `l1_a_press_not_performed_with_a_failure_or_indeterminate_keeps_the_marks_and_takes_no_next_frame`:
//   the marks are recorded, the click's outcome is the one "`--record` on device commands" gives,
//   and neither the wait nor the capture is asked for.
// - `l1_the_next_frame_comes_after_the_declared_wait_and_opens_the_next_step`: the fake's clock is
//   asked for the declared wait between the press and the capture, the frame opens the next step
//   and is written to the named file; without the option nothing is captured after the press; a
//   capture that fails leaves the press and the closed step and answers the capture's own error.
// - `l1_a_whole_recording_in_the_fewest_calls_generates_the_three_call_package`: start, the first
//   frame, one call for each of three steps, the final screen's marks and stop generate the package
//   the same recording made with three calls a step generates (the generation time removed).
// Fix round 1 (brief rd-1114/brief-1717-fix1.md, item 1): the mark request is for the open step and
// nothing else. Each of these requests passes `record mark`'s checks and the press is planned, and
// then it is `validation_failed`, its details' `member` naming what it took and its message naming
// `record mark`; the fake Runtime saw no input and every file of the recording keeps its bytes:
// - `l1_fix1_a_step_operation_is_refused`: a request with `step_action` (reopen the closed step 1);
// - `l1_fix1_a_transition_is_refused`: a request with `transition` (none, on step 1, while step 2 is
//   open);
// - `l1_fix1_a_step_other_than_the_open_one_is_refused`: a request whose `step` is 1, a closed step,
//   while step 2 is open;
// - `l1_fix1_a_frame_file_is_refused`: a request whose `frame`, a file of another screen, replaced
//   the open step's frame (its outcome carries the frame).
// Frames are synthetic gray noise; no Runtime, no device; the tests read no clock and never sleep.

use crate::recording::store::{lab_dir, old_record_path, recording_path};
use crate::{
    AttachFrameRequest, ClickEffect, ClickPlan, ClickSpec, CommitClickRequest,
    LAB_RECORD_MARK_SCHEMA, LabOcr, MarkFamily, MarkRequest, MarkSpec, OpaqueJson,
    PlanClickRequest, RecordRect, RecordStartOptions, RecordStepOutcome, RecordStepPorts,
    RecordStepRequest, RecordStopOptions, RecordingLock, StepAction, StepActionKind,
    TransitionSpec, parse_mark_request, record_attach_frame, record_commit_click, record_mark,
    record_plan_click, record_plan_step, record_start, record_stop,
};
use actingcommand_contract::{LabError, LabResult};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_pack_containment::{
    ContainmentLimits, ContentContainer, expand_content_container,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_0463";
const TASK_ID: &str = "neutral_task";
const STARTED_AT: u64 = 1_700_000_000_000;
const WIDTH: u32 = 96;
const HEIGHT: u32 = 64;
const BACKGROUND: u8 = 60;

const fn rect(x: i32, y: i32, width: i32, height: i32) -> RecordRect {
    RecordRect {
        x,
        y,
        width,
        height,
    }
}

/// The button each screen shows, a 12x12 square of noise of its own: one screen a step.
const BUTTONS: [RecordRect; 4] = [
    rect(20, 20, 12, 12),
    rect(40, 24, 12, 12),
    rect(60, 30, 12, 12),
    rect(30, 40, 12, 12),
];

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

fn inside(x: u32, y: u32, area: RecordRect) -> bool {
    let (x, y) = (i64::from(x), i64::from(y));
    x >= i64::from(area.x)
        && y >= i64::from(area.y)
        && x < i64::from(area.x) + i64::from(area.width)
        && y < i64::from(area.y) + i64::from(area.height)
}

// The PNG of a gray frame showing the noise button `button` (none: the background alone).
fn png(button: Option<usize>) -> Vec<u8> {
    let mut noise = Noise(0x0463_0100 + button.unwrap_or(0) as u64);
    let mut pixels = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let value = match button {
                Some(index) if inside(x, y, BUTTONS[index]) => noise.between(1, 255),
                _ => BACKGROUND,
            };
            pixels.extend([value; 3]);
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

// Screen `index`: its button on the background.
fn screen(index: usize) -> Vec<u8> {
    png(Some(index))
}

// Screen `index` as `capture --record` hands it to the recording.
fn captured(index: usize) -> AttachFrameRequest {
    AttachFrameRequest {
        png: screen(index),
        source: "lab_capture".to_owned(),
        runtime_artifact: None,
        capture_backend: Some("adb_screencap".to_owned()),
        freshness: Some(
            OpaqueJson::from_serializable(&json!({"required": false})).expect("freshness"),
        ),
    }
}

fn button_id(index: usize) -> String {
    format!("ui/button_{index}")
}

// The template of screen `index`'s button, searched at its crop.
fn template(index: usize) -> MarkSpec {
    MarkSpec {
        search: Some(BUTTONS[index]),
        ..MarkSpec::region_only(button_id(index), MarkFamily::Template, BUTTONS[index])
    }
}

// The request of a step on screen `index`: one rectangle, given once, the template's crop and the
// click (`click.from`).
fn step_request(index: usize) -> MarkRequest {
    MarkRequest {
        schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
        add: vec![template(index)],
        click: Some(ClickSpec {
            from: Some(button_id(index)),
            ..ClickSpec::default()
        }),
        ..MarkRequest::default()
    }
}

// The final screen's request: its template, no click.
fn final_request(index: usize) -> MarkRequest {
    MarkRequest {
        schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
        add: vec![template(index)],
        ..MarkRequest::default()
    }
}

fn opaque(value: Value) -> Option<OpaqueJson> {
    Some(OpaqueJson::from_serializable(&value).expect("opaque value"))
}

// What `do --capture --record` hands the recording for the Runtime's answer to request `id`.
fn answered(id: &str, effect: ClickEffect, failure: Option<Value>) -> CommitClickRequest {
    CommitClickRequest {
        effect,
        has_failure: failure.is_some(),
        carrier_package: opaque(json!({"package_label": "neutral.package"})),
        req_id: opaque(json!(id)),
        correlation_id: opaque(json!(format!("{id}-correlation"))),
        action_id: opaque(json!(format!("{id}-action"))),
        lease_id: opaque(json!("lease-0463")),
        failure: failure.map(|failure| OpaqueJson::from_serializable(&failure).expect("failure")),
        before: opaque(json!({"frame_id": format!("{id}-before"), "informational": true})),
        after: opaque(json!({"frame_id": format!("{id}-after"), "informational": true})),
    }
}

// The fake capture's own error.
fn capture_failure() -> LabError {
    LabError::device("the fake capture took no frame")
}

/// What the fake Runtime answers to the press.
#[derive(Clone, Copy, Debug)]
enum Answer {
    Performed,
    PerformedWithFailure,
    Indeterminate,
    NotPerformed,
}

/// The fake Runtime and clock of one call: what it answers and everything it was asked, in order.
struct FakeRuntime {
    answer: Answer,
    /// The request id of its answer.
    id: String,
    /// The screen its capture takes; `None`: the capture fails.
    next: Option<usize>,
    /// `press <step>`, `wait <ms>` and `capture`, in the order asked.
    seen: Vec<String>,
}

impl FakeRuntime {
    fn new(answer: Answer, id: &str, next: Option<usize>) -> Self {
        Self {
            answer,
            id: id.to_owned(),
            next,
            seen: Vec::new(),
        }
    }
}

impl RecordStepPorts for FakeRuntime {
    fn press(&mut self, plan: &ClickPlan) -> LabResult<CommitClickRequest> {
        self.seen.push(format!("press {}", plan.step));
        match self.answer {
            Answer::Performed => Ok(answered(&self.id, ClickEffect::Performed, None)),
            Answer::PerformedWithFailure => Ok(answered(
                &self.id,
                ClickEffect::Performed,
                Some(json!({"message": "the fake Runtime reports a failure after the input"})),
            )),
            Answer::Indeterminate => Ok(answered(&self.id, ClickEffect::Indeterminate, None)),
            Answer::NotPerformed => Err(LabError::device(
                "the fake Runtime did not perform the input",
            )),
        }
    }

    fn wait(&mut self, duration: Duration) {
        self.seen.push(format!("wait {}", duration.as_millis()));
    }

    fn capture(&mut self) -> LabResult<AttachFrameRequest> {
        self.seen.push("capture".to_owned());
        self.next.map(captured).ok_or_else(capture_failure)
    }
}

// A state directory with an active session of `INSTANCE` (the header `record start` writes) and
// its Lab recording, before any step. The lock is held as one command holds it and dropped before
// the directory.
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
        let lock = RecordingLock::acquire(&state, INSTANCE, "gate_463", "Lab-CLI")
            .expect("recording lock");
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

    // `capture --record` of screen `index`.
    fn capture(&self, index: usize) {
        record_attach_frame(&self.lock, captured(index))
            .unwrap_or_else(|error| panic!("frame {index}: {}: {}", error.code, error.message));
    }

    // `record mark` of `request` on the open step.
    fn mark(&self, request: &MarkRequest) {
        record_mark(&self.lock, request, false, &self.ocr)
            .unwrap_or_else(|error| panic!("record mark: {}: {:?}", error.code, error.details));
    }

    // The three calls' last two: `record mark` of `request`, then `do --capture --record` at the
    // declared rectangle's centre, which the Runtime performed as request `id`.
    fn three_calls(&self, request: &MarkRequest, id: &str) {
        self.mark(request);
        let plan = record_plan_click(&self.lock, &PlanClickRequest::default())
            .unwrap_or_else(|error| panic!("plan: {}: {}", error.code, error.message));
        record_commit_click(
            &self.lock,
            &plan,
            &answered(id, ClickEffect::Performed, None),
        )
        .unwrap_or_else(|error| panic!("press: {}: {}", error.code, error.message));
    }

    // The one call: `do --capture --record` with `request`, through `runtime`.
    fn one_call(
        &self,
        request: &RecordStepRequest,
        runtime: &mut FakeRuntime,
    ) -> LabResult<RecordStepOutcome> {
        record_plan_step(&self.lock, request, &self.ocr)?.perform(&self.lock, runtime)
    }

    fn recording(&self) -> Value {
        serde_json::from_slice(
            &fs::read(recording_path(&self.state(), RECORD_ID)).expect("step table"),
        )
        .expect("step table JSON")
    }

    // The step table without what differs between two recordings made one after the other in two
    // directories: every time (`*_unix_ms`, but when the session started) and the temporary
    // directory in the paths.
    fn normalised(&self) -> Value {
        let mut recording = self.recording();
        scrub(&mut recording, &self.temp.path().display().to_string());
        recording
    }

    // Every file of the recording's directory, by its `/`-separated path relative to it, with its
    // bytes, in name order.
    fn files(&self) -> Vec<(String, Vec<u8>)> {
        files_under(&lab_dir(&self.state(), RECORD_ID))
    }

    // A PNG file outside the recording's directory.
    fn frame_file(&self, name: &str, bytes: &[u8]) -> String {
        let path = self.temp.path().join(format!("{name}.png"));
        fs::write(&path, bytes).expect("frame file");
        path.display().to_string()
    }

    // `record stop`: the `lab` output with its status.
    fn stop(&self, dry_run: bool) -> Value {
        let outcome = record_stop(
            &self.lock,
            &RecordStopOptions {
                dry_run,
                ..RecordStopOptions::default()
            },
            &self.ocr,
        )
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
}

// `value` without its times and with `temp` in its texts replaced.
fn scrub(value: &mut Value, temp: &str) {
    match value {
        Value::Object(members) => {
            members
                .retain(|key, _| !key.ends_with("_unix_ms") || key == "record_started_at_unix_ms");
            for member in members.values_mut() {
                scrub(member, temp);
            }
        }
        Value::Array(items) => {
            for item in items {
                scrub(item, temp);
            }
        }
        Value::String(text) => *text = text.replace(temp, "<temp>"),
        _ => {}
    }
}

// Every file under `dir`, by its `/`-separated path relative to it, with its bytes, in name order.
fn files_under(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    let mut pending = vec![(dir.to_path_buf(), String::new())];
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

// The files of the package `record stop` wrote, the task's provenance without its generation time
// (the time the step table was last written).
fn package(lab: &Value) -> BTreeMap<String, Vec<u8>> {
    let path = lab["path"].as_str().expect("package path");
    let bytes = fs::read(path).expect("package file");
    let container = if lab["container"] == "zip" {
        ContentContainer::Zip
    } else {
        ContentContainer::Json
    };
    let mut entries = expand_content_container(&bytes, container, ContainmentLimits::default())
        .expect("package container");
    for (name, bytes) in entries.iter_mut() {
        if !name.ends_with("/task.json") {
            continue;
        }
        let mut task: Value = serde_json::from_slice(bytes).expect("task JSON");
        task["provenance"]
            .as_object_mut()
            .expect("provenance")
            .remove("generated_at_unix_ms")
            .expect("the generation time");
        *bytes = serde_json::to_vec(&task).expect("task bytes");
    }
    entries
}

fn one_step(mark: MarkRequest) -> RecordStepRequest {
    RecordStepRequest {
        mark: Some(mark),
        ..RecordStepRequest::default()
    }
}

#[test]
fn l1_one_call_records_the_step_the_three_calls_record() {
    let three = Bench::new();
    three.capture(0);
    three.three_calls(&step_request(0), "req-step-1");

    let one = Bench::new();
    one.capture(0);
    let mut runtime = FakeRuntime::new(Answer::Performed, "req-step-1", Some(1));
    let outcome = one
        .one_call(&one_step(step_request(0)), &mut runtime)
        .unwrap_or_else(|error| panic!("one call: {}: {:?}", error.code, error.details));

    assert_eq!(runtime.seen, ["press 1"]);
    assert_eq!(one.normalised(), three.normalised());
    let without_table = |files: Vec<(String, Vec<u8>)>| {
        files
            .into_iter()
            .filter(|(name, _)| name != "recording.json")
            .collect::<Vec<_>>()
    };
    assert_eq!(without_table(one.files()), without_table(three.files()));
    // One rectangle given once: the template's crop is the click, taken from the mark.
    let step = &one.recording()["steps"][0];
    assert_eq!(step["marks"][0]["region"], json!(BUTTONS[0]), "{step}");
    assert_eq!(step["click"]["rect"], json!(BUTTONS[0]), "{step}");
    assert_eq!(step["click"]["source"], "from_mark", "{step}");
    assert_eq!(step["click"]["from"], json!(button_id(0)), "{step}");
    assert_eq!(step["closed"], true, "{step}");
    // The answer: `record mark`'s output and the press as `do --capture --record` records it.
    let mark = outcome.mark.expect("the mark output");
    assert_eq!(mark.status, "marks_recorded");
    assert_eq!(mark.step, Some(1));
    assert_eq!(outcome.click.step, 1);
    assert!(outcome.click.step_closed);
    assert!(outcome.next_frame.is_none());
}

#[test]
fn l1_a_refused_mark_presses_nothing_and_writes_nothing() {
    let bench = Bench::new();
    bench.capture(0);
    let before = bench.files();
    // A color mark whose sample frame shows no button: its self-test fails there.
    let sample = bench.frame_file("blank", &png(None));
    let failing = MarkRequest {
        samples: vec![sample],
        add: vec![MarkSpec::region_only(
            "state/button".to_owned(),
            MarkFamily::Color,
            BUTTONS[0],
        )],
        click: Some(ClickSpec {
            from: Some("state/button".to_owned()),
            ..ClickSpec::default()
        }),
        ..final_request(0)
    };
    // A click whose rectangle leaves the 96x64 frame.
    let outside = MarkRequest {
        click: Some(ClickSpec {
            region: Some(rect(90, 60, 20, 20)),
            ..ClickSpec::default()
        }),
        ..final_request(0)
    };
    for (case, request, code) in [
        ("a failed self-test", failing, "record_mark_rejected"),
        ("a click outside the frame", outside, "validation_failed"),
    ] {
        let mut runtime = FakeRuntime::new(Answer::Performed, "req-refused", Some(1));
        let error = bench
            .one_call(&one_step(request.clone()), &mut runtime)
            .expect_err(case);
        // `record mark` itself (a dry run, which writes nothing) refuses the same request so.
        let alone = record_mark(&bench.lock, &request, true, &bench.ocr).expect_err(case);
        assert_eq!(error.code, code, "{case}: {}", error.message);
        assert_eq!(error.code, alone.code, "{case}");
        assert!(runtime.seen.is_empty(), "{case}: {:?}", runtime.seen);
        assert_eq!(bench.files(), before, "{case}: a file changed");
    }
    // A member the request does not have: it does not decode, as `record mark --request-json`
    // reads it.
    let text = json!({
        "schema_version": LAB_RECORD_MARK_SCHEMA,
        "add": [],
        "neutral_unknown_member": true
    })
    .to_string();
    let mut runtime = FakeRuntime::new(Answer::Performed, "req-unknown", Some(1));
    let refused = match parse_mark_request(&text) {
        Ok(mark) => bench.one_call(&one_step(mark), &mut runtime),
        Err(error) => Err(error),
    };
    let error = refused.expect_err("an unknown member");
    assert_eq!(error.code, LabError::usage("").code, "{}", error.message);
    assert!(runtime.seen.is_empty(), "{:?}", runtime.seen);
    assert_eq!(bench.files(), before, "an unknown member: a file changed");
}

#[test]
fn l1_a_press_not_performed_with_a_failure_or_indeterminate_keeps_the_marks_and_takes_no_next_frame()
 {
    let not_performed = LabError::device("").code;
    for (case, answer, code, effect, closed, needs_review) in [
        (
            "not performed",
            Answer::NotPerformed,
            not_performed.as_str(),
            Value::Null,
            false,
            false,
        ),
        (
            "performed with a failure",
            Answer::PerformedWithFailure,
            "record_click_performed_with_failure",
            json!("performed"),
            true,
            true,
        ),
        (
            "indeterminate",
            Answer::Indeterminate,
            "record_click_indeterminate",
            json!("indeterminate"),
            false,
            false,
        ),
    ] {
        let bench = Bench::new();
        bench.capture(0);
        let mut runtime = FakeRuntime::new(answer, "req-press", Some(1));
        let error = bench
            .one_call(
                &RecordStepRequest {
                    next_frame_after_ms: Some(500),
                    ..one_step(step_request(0))
                },
                &mut runtime,
            )
            .expect_err(case);

        assert_eq!(error.code, code, "{case}: {}", error.message);
        assert_eq!(runtime.seen, ["press 1"], "{case}: no wait and no capture");
        // The marks are recorded and the error names them.
        let recording = bench.recording();
        let steps = recording["steps"].as_array().expect("steps");
        assert_eq!(steps.len(), 1, "{case}: no next step: {recording}");
        let step = &steps[0];
        assert_eq!(step["marks"][0]["id"], json!(button_id(0)), "{case}");
        assert_eq!(step["click"]["rect"], json!(BUTTONS[0]), "{case}");
        // The click's outcome, as "`--record` on device commands" gives it.
        let execution = &step["click"]["execution"];
        let recorded_effect = if execution.is_null() {
            Value::Null
        } else {
            execution["effect"].clone()
        };
        assert_eq!(recorded_effect, effect, "{case}: {step}");
        assert_eq!(step["closed"], closed, "{case}: {step}");
        assert_eq!(
            step["click"]["needs_review"], needs_review,
            "{case}: {step}"
        );
        let details = error.details.clone().expect("details");
        assert_eq!(details["mark"]["step"], 1, "{case}: {details}");
        assert_eq!(
            details["mark"]["marks"][0]["id"],
            json!(button_id(0)),
            "{case}: {details}"
        );
    }
}

#[test]
fn l1_the_next_frame_comes_after_the_declared_wait_and_opens_the_next_step() {
    // Asked for: the wait between the press and the capture; the frame opens step 2 and is written
    // to the named file.
    let bench = Bench::new();
    bench.capture(0);
    let out = bench.temp.path().join("next").join("frame.png");
    let mut runtime = FakeRuntime::new(Answer::Performed, "req-next", Some(1));
    let outcome = bench
        .one_call(
            &RecordStepRequest {
                next_frame_after_ms: Some(800),
                next_frame_out: Some(out.clone()),
                ..one_step(step_request(0))
            },
            &mut runtime,
        )
        .unwrap_or_else(|error| panic!("one call: {}: {:?}", error.code, error.details));
    assert_eq!(runtime.seen, ["press 1", "wait 800", "capture"]);
    let next = outcome.next_frame.expect("the next frame");
    assert_eq!(next.after_ms, 800);
    assert_eq!(next.record.step, 2);
    assert!(next.record.step_opened);
    assert_eq!(next.record.closed_step, None);
    assert_eq!(next.out, Some(out.display().to_string()));
    assert_eq!(fs::read(&out).expect("the next frame's file"), screen(1));
    let recording = bench.recording();
    assert_eq!(recording["steps"][0]["closed"], true, "{recording}");
    assert_eq!(recording["steps"][1]["index"], 2, "{recording}");
    assert_eq!(
        recording["steps"][1]["frames"][0]["sha256"],
        json!(next.record.frame.sha256),
        "{recording}"
    );
    assert_eq!(
        recording["steps"][1]["frames"][0]["source"], "lab_capture",
        "{recording}"
    );

    // Not asked for: nothing is captured after the press.
    let bench = Bench::new();
    bench.capture(0);
    let mut runtime = FakeRuntime::new(Answer::Performed, "req-none", Some(1));
    let outcome = bench
        .one_call(&one_step(step_request(0)), &mut runtime)
        .unwrap_or_else(|error| panic!("one call: {}: {:?}", error.code, error.details));
    assert_eq!(runtime.seen, ["press 1"]);
    assert!(outcome.next_frame.is_none());
    assert_eq!(bench.recording()["steps"].as_array().map(Vec::len), Some(1));

    // A capture that fails: the press and its closed step stand, and the answer is the capture's
    // own error.
    let bench = Bench::new();
    bench.capture(0);
    let mut runtime = FakeRuntime::new(Answer::Performed, "req-fails", None);
    let error = bench
        .one_call(
            &RecordStepRequest {
                next_frame_after_ms: Some(300),
                ..one_step(step_request(0))
            },
            &mut runtime,
        )
        .expect_err("the capture fails");
    assert_eq!(runtime.seen, ["press 1", "wait 300", "capture"]);
    assert_eq!(error.code, capture_failure().code, "{}", error.message);
    assert_eq!(error.class, capture_failure().class);
    let details = error.details.expect("details");
    assert_eq!(details["record"]["step"], 1, "{details}");
    assert_eq!(details["record"]["step_closed"], true, "{details}");
    assert_eq!(
        details["next_frame"],
        json!({"after_ms": 300, "taken": false}),
        "{details}"
    );
    let recording = bench.recording();
    let steps = recording["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 1, "{recording}");
    assert_eq!(steps[0]["closed"], true, "{recording}");
    assert_eq!(
        steps[0]["click"]["execution"]["effect"], "performed",
        "{recording}"
    );
}

#[test]
fn l1_a_whole_recording_in_the_fewest_calls_generates_the_three_call_package() {
    // Three calls a step: the frame, `record mark`, `do --capture --record`; then the final
    // screen's frame and marks.
    let three = Bench::new();
    three.capture(0);
    for index in 0..3 {
        three.three_calls(&step_request(index), &format!("req-step-{}", index + 1));
        three.capture(index + 1);
    }
    three.mark(&final_request(3));

    // The fewest calls: the first frame, one call a step whose next frame is the next screen, the
    // final screen's marks.
    let one = Bench::new();
    one.capture(0);
    for index in 0..3 {
        let mut runtime = FakeRuntime::new(
            Answer::Performed,
            &format!("req-step-{}", index + 1),
            Some(index + 1),
        );
        one.one_call(
            &RecordStepRequest {
                next_frame_after_ms: Some(0),
                ..one_step(step_request(index))
            },
            &mut runtime,
        )
        .unwrap_or_else(|error| panic!("step {index}: {}: {:?}", error.code, error.details));
        assert_eq!(
            runtime.seen,
            [
                format!("press {}", index + 1),
                "wait 0".to_owned(),
                "capture".to_owned()
            ]
        );
    }
    one.mark(&final_request(3));

    assert_eq!(one.normalised(), three.normalised());
    let checked = one.stop(true);
    assert_eq!(checked["lab_status"], "validated", "{checked}");
    let (one_lab, three_lab) = (one.stop(false), three.stop(false));
    assert_eq!(one_lab["lab_status"], "generated", "{one_lab}");
    assert_eq!(three_lab["lab_status"], "generated", "{three_lab}");
    assert_eq!(one_lab["click_steps"], 3, "{one_lab}");
    assert_eq!(package(&one_lab), package(&three_lab));
}

// Fix round 1, item 1: `request`, which `record mark`'s checks and the press's plan pass, is
// refused because its mark request is not for the open step alone: `validation_failed`, its
// details' `member` naming what it took, its message naming `record mark`; the fake Runtime saw no
// input and every file of the recording keeps its bytes.
#[track_caller]
fn assert_held_to_the_open_step(bench: &Bench, request: &RecordStepRequest, member: &str) {
    let before = bench.files();
    let mut runtime = FakeRuntime::new(Answer::Performed, "req-held", Some(1));
    let error = bench.one_call(request, &mut runtime).expect_err(member);
    assert_eq!(
        error.code,
        LabError::usage("").code,
        "{member}: {}",
        error.message
    );
    let details = error.details.clone().unwrap_or(Value::Null);
    assert_eq!(details["member"], member, "{member}: {details}");
    assert!(
        error.message.contains("record mark"),
        "{member}: {}",
        error.message
    );
    assert!(runtime.seen.is_empty(), "{member}: {:?}", runtime.seen);
    assert_eq!(bench.files(), before, "{member}: a file changed");
}

// A recording whose step 1 was recorded with three calls (its click performed, the step closed)
// and whose step 2, screen 1, is open without a click.
fn second_step_open() -> Bench {
    let bench = Bench::new();
    bench.capture(0);
    bench.three_calls(&step_request(0), "req-step-1");
    bench.capture(1);
    bench
}

// The press of step 2, declared by `--tap-rect` on screen 1's button.
fn tap_rect_on_step_2(mark: MarkRequest) -> RecordStepRequest {
    RecordStepRequest {
        click: PlanClickRequest {
            tap_rect: Some(BUTTONS[1]),
            ..PlanClickRequest::default()
        },
        ..one_step(mark)
    }
}

#[test]
fn l1_fix1_a_step_operation_is_refused() {
    // Step 1 closed and the only step: reopening it leaves an open step with its declared click,
    // which the press would take.
    let bench = Bench::new();
    bench.capture(0);
    bench.three_calls(&step_request(0), "req-step-1");
    let request = one_step(MarkRequest {
        schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
        step_action: Some(StepAction {
            kind: StepActionKind::ReopenStep,
            step: Some(1),
        }),
        ..MarkRequest::default()
    });
    assert_held_to_the_open_step(&bench, &request, "step_action");
}

#[test]
fn l1_fix1_a_transition_is_refused() {
    let bench = second_step_open();
    let request = tap_rect_on_step_2(MarkRequest {
        schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
        step: Some(1),
        transition: Some(TransitionSpec::Clear),
        ..MarkRequest::default()
    });
    assert_held_to_the_open_step(&bench, &request, "transition");
}

#[test]
fn l1_fix1_a_step_other_than_the_open_one_is_refused() {
    // A color mark of the background of step 1's screen, which passes its self-test there.
    let bench = second_step_open();
    let request = tap_rect_on_step_2(MarkRequest {
        schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
        step: Some(1),
        add: vec![MarkSpec::region_only(
            "state/background".to_owned(),
            MarkFamily::Color,
            rect(0, 0, 8, 8),
        )],
        ..MarkRequest::default()
    });
    assert_held_to_the_open_step(&bench, &request, "step");
}

#[test]
fn l1_fix1_a_frame_file_is_refused() {
    // Step 1 open with screen 0 and nothing marked: a file of screen 1 replaces its frame, and the
    // request marks and clicks screen 1's button on it.
    let bench = Bench::new();
    bench.capture(0);
    let file = bench.frame_file("another-screen", &screen(1));
    let request = one_step(MarkRequest {
        frame: Some(file),
        ..step_request(1)
    });
    assert_held_to_the_open_step(&bench, &request, "frame");
}
