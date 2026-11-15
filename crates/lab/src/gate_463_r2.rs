// SPDX-License-Identifier: AGPL-3.0-only

// #463 R2 (contracts/lab-recording.md, "Endings"; register §1.384; brief
// rd-1115/brief-463-r2-record-ending.md; design rd-1114/463-l3l5-design.md, section C.3 and the
// coordinator's rulings 4 and 5): a stopped recording takes an ending from a frame, a screen that
// ends the business another way after its last press, and record stop generates its page and its
// scheduling_outcome mapping, which the kernel's admission takes and a run of the package ends on
// normally. Self-check item 9 refuses an ending page that passes on the frames of the step before
// the last, of the last step or of another ending, and warns when the usual ending's page passes on
// an ending's frames.
//
// Frames are synthetic: 96x64 pixels of flat gray (60). The list (step 1) is flat; its color mark
// `state/list` (2, 50, 4, 4) recognizes it and guards its click (8, 8, 32, 32). The next screen
// (step 2, the usual ending) carries a noise patch at (40, 46, 12, 12), its template `ui/next`. An
// ending's screen carries a square of gray 180 at (60, 50, 4, 4), its color mark `ending/short`,
// which no step's screen has. There is no OCR model and no device. The tests sleep nowhere and
// read no clock; the kernel run's clock is the scripted clock of `scripted_clock.rs`, which moves
// only by the run's pauses. Every expected value comes from the brief's and the design's lines and
// the values above.

use crate::codes::{EndingAgainst, EndingProblem, LabCode, SelectProblem};
use crate::recording::crosscheck::admit;
use crate::recording::store::{hex_sha256, old_record_path, recording_path};
use crate::scripted_clock::ScriptedClock;
use crate::{
    ClickEffect, CommitClickRequest, LAB_RECORD_MARK_SCHEMA, LabOcr, MarkOutcome, MarkRequest,
    PlanClickRequest, RecordArchiveOptions, RecordRect, RecordReopenOptions, RecordStartOptions,
    RecordStopOptions, RecordingLock, record_archive, record_commit_click, record_mark,
    record_plan_click, record_reopen, record_start, record_stop,
};
use actingcommand_contract::{InputAction, LabError, TaskOutcome};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_execution_kernel::{
    ContainedTaskRuntime, ContainedTaskTrace, InputFrameContext, ObservedFrame,
};
use actingcommand_pack_containment::{
    ContainmentLimits, ContentContainer, expand_content_container,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::time::{Duration, Instant};
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_r2";
const TASK_ID: &str = "neutral_task";
const WIDTH: u32 = 96;
const HEIGHT: u32 = 64;
const BACKGROUND: u8 = 60;
const NEXT_SEED: u64 = 0x0463_0002;
const DONE_KEY: &str = "neutral_task_done";
const SHORT_KEY: &str = "neutral_task_short";
const SHORT_PAGE: &str = "ending_01_short";
const SHORT_MARK: &str = "ending/short";
const SHORT_GRAY: u8 = 180;
const SPARE_KEY: &str = "neutral_task_spare";
const SPARE_GRAY: u8 = 200;
// Fix 1: the gray of an optional pop-up's square and of a page transition's square; the made-up
// OCR model of an OCR mark, which no test loads; the commit an archive's manifest names.
const POPUP_GRAY: u8 = 120;
const LOADING_GRAY: u8 = 140;
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const WRITTEN_BY: &str = "0123456789abcdef0123456789abcdef01234567";

const fn rect(x: i32, y: i32, width: i32, height: i32) -> RecordRect {
    RecordRect {
        x,
        y,
        width,
        height,
    }
}

// The list's page area, its click, the next screen's noise patch, the ending's square and a second
// ending's square.
const LIST_AREA: RecordRect = rect(2, 50, 4, 4);
const CLICK: RecordRect = rect(8, 8, 32, 32);
const PATCH: RecordRect = rect(40, 46, 12, 12);
const SHORT_AREA: RecordRect = rect(60, 50, 4, 4);
const SPARE_AREA: RecordRect = rect(70, 50, 4, 4);
// Fix 1: an optional pop-up's square and its close click, a page transition's square, and the
// region of an ending's OCR mark.
const POPUP_AREA: RecordRect = rect(20, 50, 4, 4);
const POPUP_CLICK: RecordRect = rect(16, 20, 24, 16);
const LOADING_AREA: RecordRect = rect(30, 50, 4, 4);
const TEXT_AREA: RecordRect = rect(56, 40, 32, 16);

// A deterministic generator of neutral gray noise.
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

// A flat screen with, when `patch`, the noise patch, and each of `squares` in its gray.
fn screen(patch: bool, squares: &[(RecordRect, u8)]) -> Vec<u8> {
    let width = WIDTH as usize;
    let mut values = vec![BACKGROUND; width * HEIGHT as usize];
    if patch {
        let mut noise = Noise(NEXT_SEED);
        for row in 0..12 {
            for column in 0..12 {
                values[(46 + row) * width + 40 + column] = noise.between(1, 199);
            }
        }
    }
    for (area, gray) in squares {
        for row in area.y..area.y + area.height {
            for column in area.x..area.x + area.width {
                values[row as usize * width + column as usize] = *gray;
            }
        }
    }
    png(&values)
}

fn list() -> Vec<u8> {
    screen(false, &[])
}

fn next() -> Vec<u8> {
    screen(true, &[])
}

fn short() -> Vec<u8> {
    screen(false, &[(SHORT_AREA, SHORT_GRAY)])
}

fn color(id: &str, area: RecordRect) -> Value {
    json!({"id": id, "family": "color", "region": area})
}

fn template(id: &str, area: RecordRect) -> Value {
    json!({"id": id, "family": "template", "region": area, "search": area})
}

// Fix 1: an OCR mark of a made-up model; without an OCR model it is never evaluated.
fn ocr(id: &str, area: RecordRect) -> Value {
    json!({"id": id, "family": "ocr", "region": area, "languages": ["en"], "timeout_ms": 1000,
           "match_mode": "exact", "expected": ["SHORT"], "case_sensitive": false,
           "minimum_confidence": 0.8, "model_ref": "neutral-ocr", "model_sha256": MODEL_SHA256})
}

// Fix 1: the PNG of the screen `bytes` shows at twice its size, each pixel four times, as an
// instance of twice the recording's resolution captures it.
fn doubled(bytes: &[u8]) -> Vec<u8> {
    let frame = Frame::from_png(bytes.to_vec(), CaptureBackendName::FixtureSimulation)
        .expect("fixture PNG");
    let (width, height) = (frame.width as usize, frame.height as usize);
    let mut pixels = Vec::with_capacity(width * height * 16);
    for row in 0..2 * height {
        for column in 0..2 * width {
            let at = ((row / 2) * width + column / 2) * 4;
            pixels.extend_from_slice(&frame.pixels[at..at + 4]);
        }
    }
    Frame::from_pixels(
        2 * frame.width,
        2 * frame.height,
        pixels,
        PixelFormat::Rgba8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("doubled frame")
    .encode_png_fast()
    .expect("doubled PNG")
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

// A state directory with an active session of `INSTANCE` and its Lab recording, before any step,
// without an OCR model. The lock is dropped before the directory.
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
        let lock =
            RecordingLock::acquire(&state, INSTANCE, "gate_r2", "Lab-CLI").expect("recording lock");
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
            ocr: LabOcr::from_vision_root(&temp.path().join("no-vision")),
            temp,
        }
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

    // The path: step 1, the list with its color mark and its click (`pressed`: recorded as
    // performed); step 2, the next screen with its template, the usual ending.
    fn record_path(&self, pressed: bool) {
        self.mark(json!({
            "frame": self.frame("list", &list()),
            "add": [color("state/list", LIST_AREA)],
            "click": {"region": CLICK}
        }))
        .unwrap_or_else(|error| {
            panic!("record mark of step 1: {} {:?}", error.code, error.details)
        });
        if pressed {
            let plan = record_plan_click(&self.lock, &PlanClickRequest::default())
                .unwrap_or_else(|error| panic!("plan click: {}", error.code));
            record_commit_click(&self.lock, &plan, &performed())
                .unwrap_or_else(|error| panic!("commit click: {}", error.code));
        }
        self.mark(json!({
            "frame": self.frame("next", &next()),
            "add": [template("ui/next", PATCH)]
        }))
        .unwrap_or_else(|error| {
            panic!("record mark of step 2: {} {:?}", error.code, error.details)
        });
    }

    // `record stop`: the `lab` output with its status.
    fn stop(&self) -> Result<Value, LabError> {
        record_stop(&self.lock, &RecordStopOptions::default(), &self.ocr).map(|outcome| {
            let mut lab = serde_json::to_value(&outcome.lab).expect("lab JSON");
            lab["lab_status"] = json!(outcome.lab_status);
            lab
        })
    }

    fn stopped(&self) -> Value {
        self.stopped_path(false)
    }

    // The path recorded and stopped, then the session file stopped as the command leaves it, so
    // that the stopped recording takes `record mark` ("Marking after stop").
    fn stopped_path(&self, pressed: bool) -> Value {
        self.record_path(pressed);
        self.stopped_as_recorded()
    }

    // Fix 1: `record stop` of the path recorded so far, generated, and the session file stopped as
    // the command leaves it.
    fn stopped_as_recorded(&self) -> Value {
        let lab = self
            .stop()
            .unwrap_or_else(|error| panic!("record stop: {} {:?}", error.code, error.details));
        assert_eq!(lab["lab_status"], "generated", "{lab}");
        self.session_status("stopped");
        lab
    }

    // The session file's status as the `record` command writes it (stop, archive, reopen).
    fn session_status(&self, status: &str) {
        let path = old_record_path(&self.temp.path().join("state"), INSTANCE);
        let mut session: Value =
            serde_json::from_slice(&fs::read(&path).expect("session file")).expect("session");
        session["status"] = json!(status);
        fs::write(&path, serde_json::to_vec(&session).expect("session JSON"))
            .expect("session file");
    }

    // The request of an ending from the frame file `name` holding `bytes`.
    fn ending(&self, name: &str, bytes: &[u8], key: &str, page: Option<&str>, add: Value) -> Value {
        let mut ending = json!({"frame": self.frame(name, bytes), "outcome_key": key, "add": add});
        if let Some(page) = page {
            ending["page"] = json!(page);
        }
        json!({"ending": ending})
    }

    // The files of the package `record stop` wrote.
    fn package(&self, lab: &Value) -> BTreeMap<String, Vec<u8>> {
        let bytes = fs::read(lab["path"].as_str().expect("package path")).expect("package");
        expand_content_container(&bytes, ContentContainer::Zip, ContainmentLimits::default())
            .expect("package container")
    }

    fn package_sha256(&self, lab: &Value) -> String {
        hex_sha256(&fs::read(lab["path"].as_str().expect("package path")).expect("package"))
    }

    fn recording_bytes(&self) -> Vec<u8> {
        fs::read(recording_path(&self.temp.path().join("state"), RECORD_ID))
            .expect("recording file")
    }

    fn recording(&self) -> Value {
        serde_json::from_slice(&self.recording_bytes()).expect("recording")
    }
}

fn task(entries: &BTreeMap<String, Vec<u8>>) -> Value {
    let bytes = entries
        .get(&format!("resources/operations/{TASK_ID}/task.json"))
        .expect("task.json");
    serde_json::from_slice(bytes).expect("task JSON")
}

// The files of a package, the task's provenance without its generation time (the time the
// recording was last written, which every `record mark` changes).
fn without_generation_time(mut entries: BTreeMap<String, Vec<u8>>) -> BTreeMap<String, Vec<u8>> {
    for (name, bytes) in &mut entries {
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

// The `ending_problem` of a `record_ending_invalid` refusal, its outcome carrying the same word.
fn ending_problem(error: LabError) -> String {
    assert_eq!(
        error.code,
        LabCode::RecordEndingInvalid.as_str(),
        "{}",
        error.message
    );
    let details = error.details.expect("details");
    assert_eq!(
        details["outcome"]["values"]["ending_problem"], details["ending_problem"],
        "{details}"
    );
    details["ending_problem"]
        .as_str()
        .expect("a word")
        .to_owned()
}

// The details of a `record_ending_ambiguous` refusal, its outcome carrying the same `against`.
fn ambiguity(error: LabError) -> Value {
    assert_eq!(
        error.code,
        LabCode::RecordEndingAmbiguous.as_str(),
        "{}: {:?}",
        error.message,
        error.details
    );
    let details = error.details.expect("details");
    assert_eq!(
        details["outcome"]["values"]["against"], details["against"],
        "{details}"
    );
    details
}

// A device for the kernel's interpreter (`PreparedContainedTask::run`): each capture shows the
// next of `screens`, the last one again once they are spent; each input is kept and performs
// nothing. Its clock is the scripted clock of the Lab's support module (as the kernel's gate tests
// take the kernel's scripted runtime's): it moves only by the run's pauses, so that the run never
// sleeps, and this file reads no clock.
struct Device {
    screens: Vec<Vec<u8>>,
    shown: usize,
    inputs: Vec<InputAction>,
    clock: ScriptedClock,
}

impl Device {
    fn showing(screens: Vec<Vec<u8>>) -> Self {
        Self {
            screens,
            shown: 0,
            inputs: Vec::new(),
            clock: ScriptedClock::started(),
        }
    }
}

impl ContainedTaskRuntime for Device {
    type Error = &'static str;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        let index = self.shown.min(self.screens.len().saturating_sub(1));
        self.shown += 1;
        let png = self.screens.get(index).ok_or("no screen")?.clone();
        Frame::from_png(png, CaptureBackendName::AdbScreencap)
            .map(ObservedFrame::from)
            .map_err(|_| "a screen that does not decode")
    }

    fn input(
        &mut self,
        action: InputAction,
        _frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        self.inputs.push(action);
        Ok(())
    }

    fn record(&mut self, _trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        Ok(())
    }

    fn monotonic_now(&mut self) -> Instant {
        self.clock.now()
    }

    fn pause(&mut self, duration: Duration) {
        self.clock.pause(duration);
    }
}

// The recording stopped, the ending `short` taken from its frame (the caller's file
// `caller_short.png`) and the package generated again.
fn with_short_ending() -> (Recording, Value) {
    let recording = Recording::new();
    recording.stopped();
    let ending = recording.ending(
        "caller_short",
        &short(),
        SHORT_KEY,
        Some("short"),
        json!([color(SHORT_MARK, SHORT_AREA)]),
    );
    recording
        .mark(ending)
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    let lab = recording
        .stop()
        .unwrap_or_else(|error| panic!("record stop: {} {:?}", error.code, error.details));
    (recording, lab)
}

// Items 1, 3 and 4 of the brief and the design's R2 tests: an ending taken from a PNG on a stopped
// recording, its marks self-tested on its frame, kept in recording.json with the time it was
// amended, its frame copied into the recording; record stop regenerates the package (another
// content hash) with the ending's page and rule, its mapping after the usual ending's, the mark's
// provenance naming the ending, the ending's provenance and item 9's result, no path of the
// caller's machine anywhere in the package, and the kernel's admission takes it.
#[test]
fn r2_an_ending_from_a_frame_is_taken_after_stop_regenerated_and_admitted() {
    let recording = Recording::new();
    let generated = recording.stopped();
    let before = recording.package_sha256(&generated);
    let caller = recording.frame("caller_short", &short());

    let marked = recording
        .mark(
            json!({"ending": {"frame": caller, "page": "short", "outcome_key": SHORT_KEY,
                                "add": [color(SHORT_MARK, SHORT_AREA)]}}),
        )
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));

    assert_eq!(marked.status, "ending_recorded");
    assert!(marked.after_stop);
    assert_eq!(marked.step, None, "an ending is no step");
    let ending = serde_json::to_value(&marked.ending).expect("ending JSON");
    assert_eq!(ending["outcome_key"], SHORT_KEY, "{ending}");
    assert_eq!(ending["page"], "short", "{ending}");
    assert_eq!(ending["marks"][0]["id"], SHORT_MARK, "{ending}");
    assert_eq!(ending["marks"][0]["step"], 0, "{ending}");
    assert_eq!(
        ending["marks"][0]["self_test"]["status"], "passed",
        "{ending}"
    );
    let stored = recording.recording();
    assert_eq!(stored["endings"], json!([ending]));
    let frame = &stored["endings"][0]["frames"][0];
    assert_eq!(frame["frame_id"], "f0003", "{frame}");
    assert_eq!(frame["role"], "primary", "{frame}");
    assert_eq!(frame["sha256"], hex_sha256(&short()), "{frame}");
    assert_ne!(
        frame["path"],
        json!(caller),
        "the frame is copied into the recording"
    );
    assert!(stored["amended_at_unix_ms"].is_u64(), "{stored}");

    let lab = recording
        .stop()
        .unwrap_or_else(|error| panic!("record stop: {} {:?}", error.code, error.details));

    assert_eq!(lab["lab_status"], "regenerated", "{lab}");
    assert_ne!(
        recording.package_sha256(&lab),
        before,
        "the package's content changed"
    );
    let entries = recording.package(&lab);
    let task = task(&entries);
    assert_eq!(task["target_page"], "step_02", "{task}");
    assert_eq!(
        task["scheduling_outcome"]["mappings"],
        json!([
            {"outcome_key": DONE_KEY, "effect": "no_designated_effect",
             "terminal_pages": ["step_02"]},
            {"outcome_key": SHORT_KEY, "effect": "no_designated_effect",
             "terminal_pages": [SHORT_PAGE]}
        ]),
        "{task}"
    );
    assert_eq!(
        task["page_rules"][SHORT_PAGE],
        json!({"required": [SHORT_MARK]}),
        "{task}"
    );
    let probe = task["color_probes"]
        .as_array()
        .and_then(|probes| probes.iter().find(|probe| probe["id"] == SHORT_MARK))
        .cloned()
        .unwrap_or_else(|| panic!("no probe {SHORT_MARK} in {task}"));
    assert_eq!(
        probe,
        json!({"id": SHORT_MARK, "region": {"mode": "rect", "rect": SHORT_AREA},
               "expected": [SHORT_GRAY, SHORT_GRAY, SHORT_GRAY],
               "provenance": {"source": "lab_record", "record_id": RECORD_ID,
                              "ending": SHORT_KEY, "self_test": "passed"}})
    );
    let provenance = &task["provenance"]["endings"][0];
    assert_eq!(provenance["page"], SHORT_PAGE, "{provenance}");
    assert_eq!(provenance["outcome_key"], SHORT_KEY, "{provenance}");
    assert_eq!(
        provenance["frame_sha256"],
        hex_sha256(&short()),
        "{provenance}"
    );
    let checked = json!([{"ending": 1, "outcome_key": SHORT_KEY, "page": SHORT_PAGE,
                          "frames": [{"frame_id": "f0003", "result": "match"}],
                          "usual": "distinct"}]);
    assert_eq!(lab["endings"], checked, "{lab}");
    assert_eq!(lab["cross_check"]["endings"], checked, "{lab}");
    assert_eq!(
        lab["pages"],
        json!(["step_01", "step_02", SHORT_PAGE]),
        "{lab}"
    );
    for (name, bytes) in &entries {
        assert!(
            !String::from_utf8_lossy(bytes).contains("caller_short"),
            "{name} names the caller's frame file"
        );
    }
    admit(&entries, &recording.ocr)
        .unwrap_or_else(|error| panic!("admission: {} {:?}", error.code, error.details));
}

// The design's R2 test "admitted", run: the kernel's interpreter on the admitted package, over the
// list and then the ending's screen, presses step 1 once and finishes Completed on the ending's
// page, with no key of its own (the host keys the run by the one mapping of the page it finished
// on, which is the ending's key).
#[test]
fn r2_a_run_that_reaches_the_endings_screen_completes_on_it() {
    let (recording, lab) = with_short_ending();
    let entries = recording.package(&lab);
    let prepared = admit(&entries, &recording.ocr)
        .unwrap_or_else(|error| panic!("admission: {} {:?}", error.code, error.details));
    let mut device = Device::showing(vec![list(), short()]);

    let outcome = prepared
        .run(&mut device)
        .unwrap_or_else(|error| panic!("the run: {error:?}"));

    assert_eq!(outcome.outcome, TaskOutcome::Success, "{outcome:?}");
    assert_eq!(
        outcome.final_page.as_deref(),
        Some("neutral/ending_01_short"),
        "{outcome:?}"
    );
    assert_eq!(outcome.executed_steps, 1, "{outcome:?}");
    assert_eq!(outcome.selected_scheduling_outcome, None, "{outcome:?}");
    assert_eq!(device.inputs.len(), 1, "{:?}", device.inputs);
    let mappings = task(&entries)["scheduling_outcome"]["mappings"].clone();
    let keyed = mappings
        .as_array()
        .and_then(|mappings| {
            mappings
                .iter()
                .find(|mapping| mapping["terminal_pages"] == json!([SHORT_PAGE]))
        })
        .cloned()
        .unwrap_or_else(|| panic!("no mapping of {SHORT_PAGE} in {mappings}"));
    assert_eq!(keyed["outcome_key"], SHORT_KEY);
}

// Item 5 of the brief: each `ending_problem` but `key_of_select` (the next test), refused at the
// mark with its word, nothing written: an active recording takes no ending and gives none up
// (`recording_active`); an ending beside a step's member, or beside `remove_ending`
// (`with_other_change`, naming them); a key out of its grammar (`key_invalid`), the usual ending's
// key (`key_reserved`); no mark (`no_marks`); a key no ending has (`key_unknown`); once an ending is
// taken, another with its key (`key_exists`) or its page name (`page_exists`).
#[test]
fn r2_each_ending_problem_is_refused_with_its_word() {
    let active = Recording::new();
    active.record_path(false);
    let before = active.recording_bytes();
    let short_ending = |recording: &Recording, key: &str| {
        recording.ending(
            "short",
            &short(),
            key,
            None,
            json!([color(SHORT_MARK, SHORT_AREA)]),
        )
    };
    for value in [
        short_ending(&active, SHORT_KEY),
        json!({"remove_ending": SHORT_KEY}),
    ] {
        let error = active.mark(value).expect_err("an active recording");
        assert_eq!(
            ending_problem(error),
            EndingProblem::RecordingActive.as_str()
        );
        assert_eq!(active.recording_bytes(), before, "nothing written");
    }

    let recording = Recording::new();
    recording.stopped();
    let before = recording.recording_bytes();
    let mut with_step = short_ending(&recording, SHORT_KEY);
    with_step["step"] = json!(1);
    let mut with_removal = short_ending(&recording, SHORT_KEY);
    with_removal["remove_ending"] = json!(SHORT_KEY);
    for (value, members) in [
        (with_step, json!(["step"])),
        (with_removal, json!(["remove_ending"])),
    ] {
        let error = recording.mark(value).expect_err("an ending stands alone");
        let details = error.details.clone().expect("details");
        assert_eq!(
            ending_problem(error),
            EndingProblem::WithOtherChange.as_str()
        );
        assert_eq!(details["members"], members, "{details}");
        assert_eq!(recording.recording_bytes(), before, "nothing written");
    }
    let unmarked = recording.ending("short", &short(), SHORT_KEY, None, json!([]));
    for (value, problem) in [
        (
            short_ending(&recording, "Short Balance"),
            EndingProblem::KeyInvalid,
        ),
        (
            short_ending(&recording, DONE_KEY),
            EndingProblem::KeyReserved,
        ),
        (unmarked, EndingProblem::NoMarks),
        (
            json!({"remove_ending": "neutral_task_nothing"}),
            EndingProblem::KeyUnknown,
        ),
    ] {
        let error = recording.mark(value).expect_err("the ending cannot hold");
        assert_eq!(ending_problem(error), problem.as_str());
        assert_eq!(recording.recording_bytes(), before, "nothing written");
    }

    recording
        .mark(recording.ending(
            "short",
            &short(),
            SHORT_KEY,
            Some("short"),
            json!([color(SHORT_MARK, SHORT_AREA)]),
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    let taken = recording.recording_bytes();
    for (key, page, problem) in [
        (SHORT_KEY, "other", EndingProblem::KeyExists),
        (SPARE_KEY, "short", EndingProblem::PageExists),
    ] {
        let error = recording
            .mark(recording.ending(
                "spare",
                &short(),
                key,
                Some(page),
                json!([color("ending/spare", SPARE_AREA)]),
            ))
            .expect_err("the second ending cannot hold");
        assert_eq!(ending_problem(error), problem.as_str());
        assert_eq!(recording.recording_bytes(), taken, "nothing written");
    }
}

// Item 5 of the brief, `key_of_select`: on a stopped recording whose step 1 (its press recorded)
// chooses in a list, an ending with the select's `on_empty` key (`<task_id>_not_found` by default)
// is refused at the mark. Fix 1, item 4 (adapted: the requirement moved the refusal of a select
// declared after the ending from record stop to the select's own mark): a select declared
// afterwards with an ending's key as its `on_empty` key is refused where it is declared,
// `record_select_invalid` with `on_empty_key_of_ending`, and nothing is written. Without an OCR
// model the select's self-test is not evaluated, which a select over a recorded press admits.
#[test]
fn r2_a_select_steps_on_empty_key_is_no_endings_key() {
    let recording = Recording::new();
    recording.stopped_path(true);
    let select = |on_empty_key: Option<&str>| {
        let mut select = json!({
            "slots": [{"rect": CLICK, "name": rect(10, 28, 28, 8)},
                      {"rect": rect(48, 8, 32, 32), "name": rect(50, 28, 28, 8)}],
            "read": {"languages": ["en"], "timeout_ms": 1000, "minimum_confidence_milli": 800,
                     "model_ref": "neutral-ocr",
                     "model_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
            "names": [{"id": "item_a", "aliases": ["ALPHA"]}, {"id": "item_b", "aliases": ["BRAVO"]}],
            "match": {"max_distance": 1, "minimum_margin": 1},
            "wanted": {"name": "list_item", "default": "item_b"},
            "pressed": "item_a"
        });
        if let Some(key) = on_empty_key {
            select["on_empty_key"] = json!(key);
        }
        json!({"step": 1, "select": select})
    };
    recording
        .mark(select(None))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    let before = recording.recording_bytes();
    let error = recording
        .mark(recording.ending(
            "short",
            &short(),
            "neutral_task_not_found",
            None,
            json!([color(SHORT_MARK, SHORT_AREA)]),
        ))
        .expect_err("the select's on_empty key");
    assert_eq!(ending_problem(error), EndingProblem::KeyOfSelect.as_str());
    assert_eq!(recording.recording_bytes(), before, "nothing written");

    recording
        .mark(recording.ending(
            "short",
            &short(),
            SPARE_KEY,
            None,
            json!([color(SHORT_MARK, SHORT_AREA)]),
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    let marked = recording.recording_bytes();
    let error = recording
        .mark(select(Some(SPARE_KEY)))
        .expect_err("the select's on_empty key is the ending's key");
    assert_eq!(
        error.code,
        LabCode::RecordSelectInvalid.as_str(),
        "{}",
        error.message
    );
    let details = error.details.expect("details");
    assert_eq!(
        details["select_problem"],
        SelectProblem::OnEmptyKeyOfEnding.as_str(),
        "{details}"
    );
    assert_eq!(
        details["outcome"]["values"]["select_problem"], details["select_problem"],
        "{details}"
    );
    assert_eq!(recording.recording_bytes(), marked, "nothing written");
}

// Item 5 of the brief, self-check item 9: an ending page that passes on the frames of the step
// before the last (`previous`, its color mark on the background the list shows too), of the last
// step (`usual`, its template the next screen's patch) or of another ending (`other_ending`, the
// second ending's screen showing the first one's square) is refused by record stop with its
// `against`, naming the screen and the frames; nothing is written.
#[test]
fn r2_each_screen_an_ending_page_passes_on_is_refused_with_its_against() {
    let cases = [
        (
            screen(false, &[(SHORT_AREA, SHORT_GRAY)]),
            color("ending/background", LIST_AREA),
            EndingAgainst::Previous,
            json!({"step": 1, "frames": ["f0001"]}),
        ),
        (
            screen(true, &[(SHORT_AREA, SHORT_GRAY)]),
            template("ending/patch", PATCH),
            EndingAgainst::Usual,
            json!({"step": 2, "frames": ["f0002"]}),
        ),
    ];
    for (bytes, mark, against, named) in cases {
        let recording = Recording::new();
        recording.stopped();
        recording
            .mark(recording.ending("short", &bytes, SHORT_KEY, None, json!([mark])))
            .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
        let marked = recording.recording_bytes();
        let details = ambiguity(recording.stop().expect_err("the ending is ambiguous"));
        assert_eq!(details["against"], against.as_str(), "{details}");
        assert_eq!(details["outcome_key"], SHORT_KEY, "{details}");
        assert_eq!(details["page"], "ending_01", "{details}");
        assert_eq!(details["step"], named["step"], "{details}");
        assert_eq!(details["frames"], named["frames"], "{details}");
        assert_eq!(recording.recording_bytes(), marked, "nothing written");
    }

    let recording = Recording::new();
    recording.stopped();
    recording
        .mark(recording.ending(
            "short",
            &short(),
            SHORT_KEY,
            None,
            json!([color(SHORT_MARK, SHORT_AREA)]),
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    recording
        .mark(recording.ending(
            "spare",
            &screen(false, &[(SHORT_AREA, SHORT_GRAY), (SPARE_AREA, SPARE_GRAY)]),
            SPARE_KEY,
            None,
            json!([color("ending/spare", SPARE_AREA)]),
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    let marked = recording.recording_bytes();
    let details = ambiguity(recording.stop().expect_err("the endings are ambiguous"));
    assert_eq!(
        details["against"],
        EndingAgainst::OtherEnding.as_str(),
        "{details}"
    );
    assert_eq!(details["outcome_key"], SHORT_KEY, "{details}");
    assert_eq!(details["page"], "ending_01", "{details}");
    assert_eq!(details["other_outcome_key"], SPARE_KEY, "{details}");
    assert_eq!(details["frames"], json!(["f0004"]), "{details}");
    assert_eq!(recording.recording_bytes(), marked, "nothing written");
}

// Item 5 of the brief: the usual ending's page passing on an ending's frame (the ending's screen
// shows the next screen's patch beside its square) is no refusal, since the run evaluates the
// endings first, but the warning `ending_usual_insensitive`, and item 9 reports it.
#[test]
fn r2_the_usual_page_passing_on_an_endings_frame_is_a_warning() {
    let recording = Recording::new();
    recording.stopped();
    recording
        .mark(recording.ending(
            "short",
            &screen(true, &[(SHORT_AREA, SHORT_GRAY)]),
            SHORT_KEY,
            None,
            json!([color(SHORT_MARK, SHORT_AREA)]),
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));

    let lab = recording
        .stop()
        .unwrap_or_else(|error| panic!("record stop: {} {:?}", error.code, error.details));

    assert_eq!(lab["lab_status"], "regenerated", "{lab}");
    let warning = lab["warnings"]
        .as_array()
        .and_then(|warnings| {
            warnings
                .iter()
                .find(|warning| warning["code"] == "ending_usual_insensitive")
        })
        .cloned()
        .unwrap_or_else(|| panic!("no ending_usual_insensitive in {lab}"));
    assert_eq!(warning["outcome_key"], SHORT_KEY, "{warning}");
    assert_eq!(warning["page"], "ending_01", "{warning}");
    assert_eq!(warning["usual_page"], "step_02", "{warning}");
    assert_eq!(warning["frames"], json!(["f0003"]), "{warning}");
    assert_eq!(lab["endings"][0]["usual"], "passes", "{lab}");
}

// Item 5 of the brief: `remove_ending` (`--remove-ending`) gives the ending up, the steps as they
// were, and record stop regenerates the package it generated before the ending was taken, but for
// its generation time; the package with the ending had another content hash.
#[test]
fn r2_removing_the_ending_gives_back_the_package_generated_before() {
    let recording = Recording::new();
    let generated = recording.stopped();
    let steps = recording.recording()["steps"].clone();
    let package = without_generation_time(recording.package(&generated));
    let before = recording.package_sha256(&generated);
    recording
        .mark(recording.ending(
            "short",
            &short(),
            SHORT_KEY,
            None,
            json!([color(SHORT_MARK, SHORT_AREA)]),
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    let with_ending = recording
        .stop()
        .unwrap_or_else(|error| panic!("record stop: {} {:?}", error.code, error.details));
    assert_ne!(recording.package_sha256(&with_ending), before);

    let removed = recording
        .mark(json!({"remove_ending": SHORT_KEY}))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));

    assert_eq!(removed.status, "ending_removed");
    assert!(removed.after_stop);
    let given_up = serde_json::to_value(&removed.removed_ending).expect("ending JSON");
    assert_eq!(given_up["outcome_key"], SHORT_KEY, "{given_up}");
    let stored = recording.recording();
    assert_eq!(stored.get("endings"), None, "{stored}");
    assert_eq!(stored["steps"], steps, "the steps as they were");
    let regenerated = recording
        .stop()
        .unwrap_or_else(|error| panic!("record stop: {} {:?}", error.code, error.details));
    assert_eq!(regenerated["lab_status"], "regenerated", "{regenerated}");
    assert_eq!(
        without_generation_time(recording.package(&regenerated)),
        package
    );
}

// Item 3 of the brief: a recording without an ending has no ending anywhere: no `endings` in
// recording.json, one mapping, no ending page, no ending in the provenance, the output or the
// cross-check. (gate_463_r1's constants test pins the bytes of such a package, unchanged.)
#[test]
fn r2_a_recording_without_an_ending_generates_as_before() {
    let recording = Recording::new();
    let lab = recording.stopped();

    let text = String::from_utf8(recording.recording_bytes()).expect("recording text");
    assert!(!text.contains("\"endings\""), "{text}");
    assert_eq!(lab.get("endings"), None, "{lab}");
    assert_eq!(lab["cross_check"].get("endings"), None, "{lab}");
    assert_eq!(lab["pages"], json!(["step_01", "step_02"]), "{lab}");
    let task = task(&recording.package(&lab));
    assert_eq!(
        task["scheduling_outcome"]["mappings"],
        json!([{"outcome_key": DONE_KEY, "effect": "no_designated_effect",
                "terminal_pages": ["step_02"]}]),
        "{task}"
    );
    assert_eq!(task["provenance"].get("endings"), None, "{task}");
    let pages = task["page_rules"]
        .as_object()
        .map(|rules| rules.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    assert_eq!(pages, ["step_01", "step_02"], "{task}");
}

// R1's review: `clear_select` on a step that has no select is refused with its own word,
// `no_select`, and nothing is written, where it answered as if it had taken a select off.
#[test]
fn r2_clear_select_on_a_step_without_a_select_is_refused() {
    let recording = Recording::new();
    recording.record_path(false);
    let before = recording.recording_bytes();

    let error = recording
        .mark(json!({"step": 1, "clear_select": true}))
        .expect_err("no select to take off");

    assert_eq!(
        error.code,
        LabCode::RecordSelectInvalid.as_str(),
        "{}",
        error.message
    );
    let details = error.details.expect("details");
    assert_eq!(
        details["select_problem"],
        SelectProblem::NoSelect.as_str(),
        "{details}"
    );
    assert_eq!(
        details["outcome"]["values"]["select_problem"], details["select_problem"],
        "{details}"
    );
    assert_eq!(recording.recording_bytes(), before, "nothing written");
}

// Fix 1, item 3 (the coordinator's ruling: the Lab scales an ending's frame itself, with the
// Runtime's own conversion): a frame of another ratio than the recording is refused
// `record_frame_size_mismatch` as before, nothing written; a frame twice the recording's size, a
// pure scale, is converted to the recording's 96x64 as the Runtime converts every frame (each
// output pixel the area average of the four it covers, so the screen as recorded here), kept as
// the ending's frame with `scaled_from` 192x128, its mark self-tested on the converted frame, and
// record stop regenerates the package.
#[test]
fn r2_an_endings_frame_of_a_pure_scale_is_converted_to_the_recordings_size() {
    let recording = Recording::new();
    recording.stopped();
    let before = recording.recording_bytes();
    let other_ratio = Frame::from_pixels(
        100,
        64,
        vec![BACKGROUND; 100 * 64 * 3],
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
    .encode_png_fast()
    .expect("fixture PNG");
    let error = recording
        .mark(recording.ending(
            "wide",
            &other_ratio,
            SHORT_KEY,
            None,
            json!([color(SHORT_MARK, SHORT_AREA)]),
        ))
        .expect_err("a frame of another ratio");
    assert_eq!(
        error.code, "record_frame_size_mismatch",
        "{}",
        error.message
    );
    assert_eq!(recording.recording_bytes(), before, "nothing written");

    let marked = recording
        .mark(recording.ending(
            "doubled",
            &doubled(&short()),
            SHORT_KEY,
            None,
            json!([color(SHORT_MARK, SHORT_AREA)]),
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));

    let frame = serde_json::to_value(&marked.frame).expect("frame JSON");
    assert_eq!(frame["width"], WIDTH, "{frame}");
    assert_eq!(frame["height"], HEIGHT, "{frame}");
    assert_eq!(
        frame["scaled_from"],
        json!({"width": 2 * WIDTH, "height": 2 * HEIGHT}),
        "{frame}"
    );
    let stored = recording.recording();
    assert_eq!(stored["endings"][0]["frames"][0], frame, "{stored}");
    assert_eq!(
        stored["endings"][0]["marks"][0]["self_test"]["status"], "passed",
        "{stored}"
    );
    let kept = Frame::from_png(
        fs::read(frame["path"].as_str().expect("frame path")).expect("stored frame"),
        CaptureBackendName::AdbScreencap,
    )
    .expect("stored PNG");
    let recorded = Frame::from_png(short(), CaptureBackendName::AdbScreencap).expect("fixture PNG");
    assert_eq!((kept.width, kept.height), (WIDTH, HEIGHT));
    assert_eq!(
        kept.pixels, recorded.pixels,
        "the converted frame is the screen as recorded"
    );
    let lab = recording
        .stop()
        .unwrap_or_else(|error| panic!("record stop: {} {:?}", error.code, error.details));
    assert_eq!(lab["lab_status"], "regenerated", "{lab}");
}

// Fix 1, item 5: an ending whose page rests on OCR marks alone, which no OCR model can check, is
// refused by record stop's item 9 at the first screen it could not be checked on, the step before
// the last: `record_ending_invalid` with `ocr_alone_unchecked`, naming the screen and its frames;
// nothing is written.
#[test]
fn r2_an_ending_that_rests_on_ocr_alone_is_refused_without_a_model() {
    let recording = Recording::new();
    recording.stopped();
    recording
        .mark(recording.ending(
            "short",
            &short(),
            SHORT_KEY,
            None,
            json!([ocr("ending/text", TEXT_AREA)]),
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    let marked = recording.recording_bytes();

    let error = recording
        .stop()
        .expect_err("no OCR model checks the ending");

    let details = error.details.clone().expect("details");
    assert_eq!(
        ending_problem(error),
        EndingProblem::OcrAloneUnchecked.as_str()
    );
    assert_eq!(details["outcome_key"], SHORT_KEY, "{details}");
    assert_eq!(details["page"], "ending_01", "{details}");
    assert_eq!(
        details["against"],
        EndingAgainst::Previous.as_str(),
        "{details}"
    );
    assert_eq!(details["step"], 1, "{details}");
    assert_eq!(details["frames"], json!(["f0001"]), "{details}");
    assert_eq!(recording.recording_bytes(), marked, "nothing written");
}

// Fix 1, items 5 and 6: an ending with an OCR mark beside a mark of another family (a color the
// screens share) is no refusal without an OCR model: record stop regenerates, with the warning
// `ending_ambiguity_not_evaluated` for each screen the color passes on and the OCR mark could not
// decide (the step before the last, f0001; the last step, f0002), the ending's own frame
// undetermined and the cross-check partially evaluated.
#[test]
fn r2_an_ending_with_a_mark_of_another_family_keeps_the_not_evaluated_warning() {
    let recording = Recording::new();
    recording.stopped();
    recording
        .mark(recording.ending(
            "short",
            &short(),
            SHORT_KEY,
            None,
            json!([
                color("ending/background", LIST_AREA),
                ocr("ending/text", TEXT_AREA)
            ]),
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));

    let lab = recording
        .stop()
        .unwrap_or_else(|error| panic!("record stop: {} {:?}", error.code, error.details));

    assert_eq!(lab["lab_status"], "regenerated", "{lab}");
    let warnings = lab["warnings"]
        .as_array()
        .map(|warnings| {
            warnings
                .iter()
                .filter(|warning| warning["code"] == "ending_ambiguity_not_evaluated")
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    assert_eq!(warnings.len(), 2, "{lab}");
    for (warning, (against, step, frame)) in warnings.iter().zip([
        (EndingAgainst::Previous, 1, "f0001"),
        (EndingAgainst::Usual, 2, "f0002"),
    ]) {
        assert_eq!(
            warning["check"],
            LabCode::RecordEndingAmbiguous.as_str(),
            "{warning}"
        );
        assert_eq!(warning["outcome_key"], SHORT_KEY, "{warning}");
        assert_eq!(warning["page"], "ending_01", "{warning}");
        assert_eq!(warning["against"], against.as_str(), "{warning}");
        assert_eq!(warning["step"], step, "{warning}");
        assert_eq!(warning["frames"], json!([frame]), "{warning}");
    }
    assert_eq!(lab["cross_check"]["status"], "partially_evaluated", "{lab}");
    assert_eq!(
        lab["endings"][0]["frames"],
        json!([{"frame_id": "f0003", "result": "undetermined"}]),
        "{lab}"
    );
    assert_eq!(lab["endings"][0]["usual"], "distinct", "{lab}");
}

// Fix 1, item 6: `previous` through a run of optional steps. The path is the list (step 1), an
// optional pop-up with its square and close click (step 2, the step before the last) and the next
// screen (step 3); the run awaits the endings after step 1's press too, before the pop-up. An
// ending page that passes on step 1's frame (a color of the background, which the pop-up's frame
// shows too) is refused naming step 1, the step before the run; one that passes on the pop-up's
// frame only (the pop-up's square) is refused naming step 2.
#[test]
fn r2_previous_covers_the_step_before_a_run_of_optional_steps_and_its_members() {
    let cases = [
        (short(), color("ending/background", LIST_AREA), 1, "f0001"),
        (
            screen(false, &[(SHORT_AREA, SHORT_GRAY), (POPUP_AREA, POPUP_GRAY)]),
            color("ending/popup", POPUP_AREA),
            2,
            "f0002",
        ),
    ];
    for (bytes, mark, step, frame) in cases {
        let recording = Recording::new();
        for request in [
            json!({"frame": recording.frame("list", &list()),
                   "add": [color("state/list", LIST_AREA)], "click": {"region": CLICK}}),
            json!({"frame": recording.frame("popup", &screen(false, &[(POPUP_AREA, POPUP_GRAY)])),
                   "add": [color("state/popup", POPUP_AREA)], "click": {"region": POPUP_CLICK},
                   "optional": true}),
            json!({"frame": recording.frame("next", &next()),
                   "add": [template("ui/next", PATCH)]}),
        ] {
            recording
                .mark(request)
                .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
        }
        recording.stopped_as_recorded();
        recording
            .mark(recording.ending("short", &bytes, SHORT_KEY, None, json!([mark])))
            .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
        let marked = recording.recording_bytes();

        let details = ambiguity(recording.stop().expect_err("the ending is ambiguous"));

        assert_eq!(
            details["against"],
            EndingAgainst::Previous.as_str(),
            "{details}"
        );
        assert_eq!(details["step"], step, "{details}");
        assert_eq!(details["frames"], json!([frame]), "{details}");
        assert_eq!(recording.recording_bytes(), marked, "nothing written");
    }
}

// Fix 1, item 6: `previous` on a step with a page transition takes the transition's frames, the
// screen left after its press: an ending page that passes on the transition's frame (its square)
// and not on the list is refused naming step 1 and the transition's frame f0003.
#[test]
fn r2_previous_on_a_step_with_a_page_transition_takes_its_transition_frames() {
    let recording = Recording::new();
    recording.stopped();
    let loading = screen(false, &[(LOADING_AREA, LOADING_GRAY)]);
    recording
        .mark(json!({"step": 1, "transition": {"kind": "page",
                     "frame": recording.frame("loading", &loading),
                     "add": [color("loading/state", LOADING_AREA)]}}))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    recording
        .mark(recording.ending(
            "short",
            &screen(
                false,
                &[(SHORT_AREA, SHORT_GRAY), (LOADING_AREA, LOADING_GRAY)],
            ),
            SHORT_KEY,
            None,
            json!([color("ending/loading", LOADING_AREA)]),
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    let marked = recording.recording_bytes();

    let details = ambiguity(
        recording
            .stop()
            .expect_err("the ending passes on the transition"),
    );

    assert_eq!(
        details["against"],
        EndingAgainst::Previous.as_str(),
        "{details}"
    );
    assert_eq!(details["step"], 1, "{details}");
    assert_eq!(details["frames"], json!(["f0003"]), "{details}");
    assert_eq!(recording.recording_bytes(), marked, "nothing written");
}

// Fix 1, item 6: an archived recording takes no ending (`record_session_not_active`, as every
// record mark of a session that is not active); reopened, it is active, and an ending is
// `recording_active` until record stop. Nothing is written either time.
#[test]
fn r2_an_archived_recording_takes_no_ending_and_a_reopened_one_is_active() {
    let recording = Recording::new();
    recording.record_path(false);
    record_archive(
        &recording.lock,
        &RecordArchiveOptions {
            written_by: WRITTEN_BY.to_owned(),
            ..RecordArchiveOptions::default()
        },
    )
    .unwrap_or_else(|error| panic!("record archive: {} {:?}", error.code, error.details));
    recording.session_status("archived");
    let ending = || {
        recording.ending(
            "short",
            &short(),
            SHORT_KEY,
            None,
            json!([color(SHORT_MARK, SHORT_AREA)]),
        )
    };
    let before = recording.recording_bytes();

    let error = recording.mark(ending()).expect_err("an archived recording");
    assert_eq!(error.code, "record_session_not_active", "{}", error.message);
    assert_eq!(recording.recording_bytes(), before, "nothing written");

    record_reopen(&recording.lock, &RecordReopenOptions { record_id: None })
        .unwrap_or_else(|error| panic!("record reopen: {} {:?}", error.code, error.details));
    recording.session_status("active");
    let error = recording
        .mark(ending())
        .expect_err("a reopened recording is active");
    assert_eq!(
        ending_problem(error),
        EndingProblem::RecordingActive.as_str()
    );
    assert_eq!(recording.recording_bytes(), before, "nothing written");
}
