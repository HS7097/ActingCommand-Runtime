// SPDX-License-Identifier: AGPL-3.0-only

// #463 R4 and #464 P4 (contracts/lab-recording.md, "Inserting an optional step after stop",
// "Writing" and "Output"; register §1.388; brief rd-1115/brief-463-r4p4-after-stop.md; design
// rd-1114/464-pack-build-design.md, the slice row P4 and its tests): a stopped recording takes an
// optional step after the effect of one of its steps from a frame, a pop-up after which the path
// goes on, and record stop generates it in its place and checks it as every optional step; giving it
// up gives back the package generated before. record stop keeps the output an earlier stop wrote,
// refuses a --lab-dir inside an installed root and answers the skeleton of the adoption.
//
// Frames are synthetic: 96x64 pixels of flat gray (60). The list (step 1) is flat; its color mark
// `state/list` (2, 50, 4, 4) recognizes it and guards its click (8, 8, 32, 32). The next screen
// (step 2, the usual ending) carries a noise patch at (40, 46, 12, 12), its template `ui/next`. The
// pop-up carries a square of gray 120 at (20, 50, 4, 4), its color mark `popup/notice`, and is
// closed at (16, 20, 24, 16). Fix round 2: a second pop-up carries a square of gray 180 at
// (70, 10, 4, 4), its color mark `popup/reward`, its page `reward`, and is closed at
// (56, 24, 24, 16). There is no OCR model and no device. The tests sleep nowhere and read no clock;
// the kernel run's clock is the scripted clock of `scripted_clock.rs`, which moves only by the
// run's pauses. Every expected value comes from the brief's and the contract's lines and the values
// above.

use crate::codes::{LabCode, OptionalAfterProblem};
use crate::recording::crosscheck::admit;
use crate::recording::store::{hex_sha256, old_record_path, recording_path};
use crate::scripted_clock::ScriptedClock;
use crate::{
    LAB_RECORD_MARK_SCHEMA, LabOcr, MarkOutcome, MarkRequest, RecordRect, RecordStartOptions,
    RecordStopOptions, RecordingLock, record_mark, record_start, record_status, record_stop,
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
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_r4";
const TASK_ID: &str = "neutral_task";
const WIDTH: u32 = 96;
const HEIGHT: u32 = 64;
const BACKGROUND: u8 = 60;
const NEXT_SEED: u64 = 0x0463_0004;
const POPUP_GRAY: u8 = 120;
const POPUP_MARK: &str = "popup/notice";
const POPUP_PAGE: &str = "step_02_notice";
// Fix round 2: the second pop-up's gray and mark.
const REWARD_GRAY: u8 = 180;
const REWARD_MARK: &str = "popup/reward";

const fn rect(x: i32, y: i32, width: i32, height: i32) -> RecordRect {
    RecordRect {
        x,
        y,
        width,
        height,
    }
}

// The list's page area, its click, the next screen's noise patch, the pop-up's square and its close
// click.
const LIST_AREA: RecordRect = rect(2, 50, 4, 4);
const CLICK: RecordRect = rect(8, 8, 32, 32);
const PATCH: RecordRect = rect(40, 46, 12, 12);
const POPUP_AREA: RecordRect = rect(20, 50, 4, 4);
const POPUP_CLICK: RecordRect = rect(16, 20, 24, 16);
// Fix round 2: the second pop-up's square and its close click.
const REWARD_AREA: RecordRect = rect(70, 10, 4, 4);
const REWARD_CLICK: RecordRect = rect(56, 24, 24, 16);

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

fn popup() -> Vec<u8> {
    screen(false, &[(POPUP_AREA, POPUP_GRAY)])
}

// Fix round 2: the second pop-up, over the flat screen.
fn reward() -> Vec<u8> {
    screen(false, &[(REWARD_AREA, REWARD_GRAY)])
}

// Whether `action` is a tap inside `area`.
fn tapped_inside(action: &InputAction, area: RecordRect) -> bool {
    matches!(action, InputAction::Tap { x, y }
        if (area.x..area.x + area.width).contains(x) && (area.y..area.y + area.height).contains(y))
}

fn color(id: &str, area: RecordRect) -> Value {
    json!({"id": id, "family": "color", "region": area})
}

fn template(id: &str, area: RecordRect) -> Value {
    json!({"id": id, "family": "template", "region": area, "search": area})
}

// The PNG of the screen `bytes` shows at twice its size, each pixel four times, as an instance of
// twice the recording's resolution captures it.
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

fn outcome_json(outcome: &MarkOutcome) -> Value {
    serde_json::to_value(outcome).expect("outcome JSON")
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
            RecordingLock::acquire(&state, INSTANCE, "gate_r4", "Lab-CLI").expect("recording lock");
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

    // The path: step 1, the list with its color mark and its click; step 2, the next screen with
    // its template, the usual ending.
    fn record_path(&self) {
        self.mark(json!({
            "frame": self.frame("list", &list()),
            "add": [color("state/list", LIST_AREA)],
            "click": {"region": CLICK}
        }))
        .unwrap_or_else(|error| {
            panic!("record mark of step 1: {} {:?}", error.code, error.details)
        });
        self.mark(json!({
            "frame": self.frame("next", &next()),
            "add": [template("ui/next", PATCH)]
        }))
        .unwrap_or_else(|error| {
            panic!("record mark of step 2: {} {:?}", error.code, error.details)
        });
    }

    // `record stop` with `options`: the `lab` output with its status.
    fn stop_with(&self, options: &RecordStopOptions) -> Result<Value, LabError> {
        record_stop(&self.lock, options, &self.ocr).map(|outcome| {
            let mut lab = serde_json::to_value(&outcome.lab).expect("lab JSON");
            lab["lab_status"] = json!(outcome.lab_status);
            lab
        })
    }

    fn stop(&self) -> Result<Value, LabError> {
        self.stop_with(&RecordStopOptions::default())
    }

    // The path recorded and stopped (generated), then the session file stopped as the command
    // leaves it, so that the stopped recording takes `record mark` ("Marking after stop").
    fn stopped(&self) -> Value {
        self.record_path();
        let lab = self
            .stop()
            .unwrap_or_else(|error| panic!("record stop: {} {:?}", error.code, error.details));
        assert_eq!(lab["lab_status"], "generated", "{lab}");
        self.session_status("stopped");
        lab
    }

    // The session file's status as the `record` command writes it.
    fn session_status(&self, status: &str) {
        let path = old_record_path(&self.state(), INSTANCE);
        let mut session: Value =
            serde_json::from_slice(&fs::read(&path).expect("session file")).expect("session");
        session["status"] = json!(status);
        fs::write(&path, serde_json::to_vec(&session).expect("session JSON"))
            .expect("session file");
    }

    // The request of the pop-up from the frame file `name` holding `bytes`, inserted after `step`
    // with the marks `add` and its close click.
    fn insert(&self, step: u32, name: &str, bytes: &[u8], add: Value) -> Value {
        json!({"optional_after": {"step": step, "frame": self.frame(name, bytes), "page": "notice",
                                   "add": add, "click": {"region": POPUP_CLICK},
                                   "settle_ms": 1000}})
    }

    // The pop-up of the fixture, inserted after step 1.
    fn insert_popup(&self) -> MarkOutcome {
        self.mark(self.insert(
            1,
            "caller_popup",
            &popup(),
            json!([color(POPUP_MARK, POPUP_AREA)]),
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details))
    }

    // Fix round 2: the second pop-up of the fixture, inserted after `step` with its own mark, page
    // and close click.
    fn insert_reward(&self, step: u32) -> MarkOutcome {
        self.mark(json!({"optional_after": {"step": step,
                                            "frame": self.frame("caller_reward", &reward()),
                                            "page": "reward",
                                            "add": [color(REWARD_MARK, REWARD_AREA)],
                                            "click": {"region": REWARD_CLICK},
                                            "settle_ms": 1000}}))
            .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details))
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
        fs::read(recording_path(&self.state(), RECORD_ID)).expect("recording file")
    }

    fn recording(&self) -> Value {
        serde_json::from_slice(&self.recording_bytes()).expect("recording")
    }

    // The serial numbers of the steps `record status` lists, in its order, with their package
    // numbers.
    fn status_steps(&self) -> Vec<(Value, Value)> {
        let status = record_status(&self.state(), INSTANCE)
            .unwrap_or_else(|error| panic!("record status: {} {:?}", error.code, error.details));
        let status = serde_json::to_value(status).expect("status JSON");
        status["steps"]
            .as_array()
            .unwrap_or_else(|| panic!("no steps in {status}"))
            .iter()
            .map(|step| (step["index"].clone(), step["artifact_step"].clone()))
            .collect()
    }
}

fn task(entries: &BTreeMap<String, Vec<u8>>) -> Value {
    let bytes = entries
        .get(&format!("resources/operations/{TASK_ID}/task.json"))
        .expect("task.json");
    serde_json::from_slice(bytes).expect("task JSON")
}

// Fix round 2: each operation of a task, its id and the page it starts from, in the package's
// order.
fn operation_starts(task: &Value) -> Vec<(Value, Value)> {
    task["operations"]
        .as_array()
        .unwrap_or_else(|| panic!("no operations in {task}"))
        .iter()
        .map(|operation| (operation["id"].clone(), operation["from"].clone()))
        .collect()
}

// Fix round 2: the serial number of each step of a package, in the package's order, as its
// provenance names them.
fn record_indices(task: &Value) -> Vec<Value> {
    task["provenance"]["steps"]
        .as_array()
        .unwrap_or_else(|| panic!("no step provenance in {task}"))
        .iter()
        .map(|step| step["record_index"].clone())
        .collect()
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

// The `optional_after_problem` of a `record_optional_after_invalid` refusal, its outcome carrying
// the same word.
fn problem(error: LabError) -> String {
    assert_eq!(
        error.code,
        LabCode::RecordOptionalAfterInvalid.as_str(),
        "{}: {:?}",
        error.message,
        error.details
    );
    let details = error.details.expect("details");
    assert_eq!(
        details["outcome"]["values"]["optional_after_problem"], details["optional_after_problem"],
        "{details}"
    );
    details["optional_after_problem"]
        .as_str()
        .expect("a word")
        .to_owned()
}

// The file names directly inside `dir`, sorted.
fn names_in(dir: &Path) -> Vec<String> {
    let mut names = fs::read_dir(dir)
        .expect("directory")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    names.sort();
    names
}

// A device for the kernel's interpreter (`PreparedContainedTask::run`): each capture shows the
// next of `screens`, the last one again once they are spent; each input is kept and performs
// nothing. Its clock is the scripted clock of the Lab's support module: it moves only by the run's
// pauses, so that the run never sleeps, and this file reads no clock.
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

// The recording stopped, the fixture's pop-up inserted after step 1 and the package generated
// again.
fn with_popup() -> (Recording, Value) {
    let recording = Recording::new();
    recording.stopped();
    recording.insert_popup();
    let lab = recording
        .stop()
        .unwrap_or_else(|error| panic!("record stop: {} {:?}", error.code, error.details));
    (recording, lab)
}

// R4, items 1 to 3 of the brief: an optional step inserted after step 1 of the stopped recording
// from a PNG: status optional_step_inserted, the next serial number 3, inserted after step 1,
// closed, optional with its settle, its frame f0003 copied into the recording and its mark
// self-tested; record status lists the steps in the order they run (1, 3, 2, numbered 1, 2, 3);
// record stop regenerates the package (another content hash) with the pop-up's page and operation
// between step 1's and the next screen's, which the package numbers 3, its optional settle and the
// skip target, its provenance naming the recorded step it runs after; the kernel's admission takes
// it.
#[test]
fn r4_an_optional_step_inserted_after_stop_is_generated_in_its_place_and_admitted() {
    let recording = Recording::new();
    let generated = recording.stopped();
    let before = recording.package_sha256(&generated);

    let marked = outcome_json(&recording.insert_popup());

    assert_eq!(marked["status"], "optional_step_inserted", "{marked}");
    assert_eq!(marked["after_stop"], json!(true), "{marked}");
    assert_eq!(marked["step"], 3, "{marked}");
    assert_eq!(marked["step_state"]["inserted_after"], 1, "{marked}");
    assert_eq!(marked["step_state"]["closed"], json!(true), "{marked}");
    assert_eq!(marked["step_state"]["effect"], "click_declared", "{marked}");
    assert_eq!(
        marked["step_state"]["optional"],
        json!({"settle_ms": 1000}),
        "{marked}"
    );
    assert_eq!(marked["marks"][0]["id"], POPUP_MARK, "{marked}");
    assert_eq!(
        marked["marks"][0]["self_test"]["status"], "passed",
        "{marked}"
    );
    let stored = recording.recording();
    let step = &stored["steps"][2];
    assert_eq!(step["index"], 3, "{stored}");
    assert_eq!(step["inserted_after"], 1, "{stored}");
    assert_eq!(step["closed_by"], "author", "{stored}");
    assert_eq!(step["page"], "notice", "{stored}");
    assert_eq!(step["optional"]["settle_ms"], 1000, "{stored}");
    assert_eq!(step["frames"][0]["frame_id"], "f0003", "{stored}");
    assert_eq!(
        step["frames"][0]["sha256"],
        hex_sha256(&popup()),
        "{stored}"
    );
    assert!(stored["amended_at_unix_ms"].is_u64(), "{stored}");
    assert_eq!(
        recording.status_steps(),
        [
            (json!(1), json!(1)),
            (json!(3), json!(2)),
            (json!(2), json!(3))
        ]
    );

    let lab = recording
        .stop()
        .unwrap_or_else(|error| panic!("record stop: {} {:?}", error.code, error.details));

    assert_eq!(lab["lab_status"], "regenerated", "{lab}");
    assert_ne!(recording.package_sha256(&lab), before);
    let entries = recording.package(&lab);
    let task = task(&entries);
    assert_eq!(task["target_page"], "step_03", "{task}");
    assert_eq!(
        task["page_rules"][POPUP_PAGE],
        json!({"required": [POPUP_MARK]}),
        "{task}"
    );
    let operations = task["operations"].as_array().expect("operations");
    assert_eq!(operations.len(), 2, "{task}");
    assert_eq!(operations[0]["id"], "step_01_click", "{task}");
    assert_eq!(operations[0]["to"], POPUP_PAGE, "{task}");
    assert_eq!(operations[1]["id"], "step_02_click", "{task}");
    assert_eq!(operations[1]["from"], POPUP_PAGE, "{task}");
    assert_eq!(operations[1]["to"], "step_03", "{task}");
    assert_eq!(
        operations[1]["click"],
        json!({"kind": "rect", "x": 16, "y": 20, "width": 24, "height": 16}),
        "{task}"
    );
    assert_eq!(
        operations[1]["optional"],
        json!({"settle_ms": 1000}),
        "{task}"
    );
    let provenance = &task["provenance"]["steps"][1];
    assert_eq!(provenance["record_index"], 3, "{provenance}");
    assert_eq!(provenance["inserted_after"], 1, "{provenance}");
    assert_eq!(
        lab["optional_steps"],
        json!([{"step": 2, "op": "step_02_click", "settle_ms": 1000, "skip_to": "step_03",
                "same_as": null}]),
        "{lab}"
    );
    assert_eq!(
        lab["pages"],
        json!(["step_01", POPUP_PAGE, "step_03"]),
        "{lab}"
    );
    for (name, bytes) in &entries {
        assert!(
            !String::from_utf8_lossy(bytes).contains("caller_popup"),
            "{name} names the caller's frame file"
        );
    }
    admit(&entries, &recording.ocr)
        .unwrap_or_else(|error| panic!("admission: {} {:?}", error.code, error.details));
}

// R4, the brief's run tests: the kernel's interpreter on the admitted package, over the list, the
// pop-up and the next screen, presses step 1 and then the pop-up's click and finishes Completed on
// the next screen; over the list and the next screen alone it presses step 1 only, waits out the
// settle and finishes Completed on the next screen, as the package without the pop-up does. Fix
// round 2 (item 7): the first press lies inside step 1's click box and the second inside the
// inserted step's.
#[test]
fn r4_a_run_presses_the_inserted_pop_up_when_it_shows_and_goes_on_without_it() {
    let (recording, lab) = with_popup();
    let prepared = admit(&recording.package(&lab), &recording.ocr)
        .unwrap_or_else(|error| panic!("admission: {} {:?}", error.code, error.details));

    for (screens, steps, boxes) in [
        (
            vec![list(), popup(), next()],
            2_u32,
            vec![CLICK, POPUP_CLICK],
        ),
        (vec![list(), next()], 1, vec![CLICK]),
    ] {
        let mut device = Device::showing(screens);

        let outcome = prepared
            .run(&mut device)
            .unwrap_or_else(|error| panic!("the run: {error:?}"));

        assert_eq!(outcome.outcome, TaskOutcome::Success, "{outcome:?}");
        assert_eq!(
            outcome.final_page.as_deref(),
            Some("neutral/step_03"),
            "{outcome:?}"
        );
        assert_eq!(outcome.executed_steps, steps, "{outcome:?}");
        assert_eq!(device.inputs.len(), boxes.len(), "{:?}", device.inputs);
        for (input, area) in device.inputs.iter().zip(boxes) {
            assert!(tapped_inside(input, area), "{input:?} outside {area:?}");
        }
    }
}

// R4, item 5 of the brief: each optional_after_problem, refused with its word and nothing written:
// recording_active (an insertion and a removal on an active recording); with_other_change (beside
// a step's member, beside remove_inserted, naming them); step_unknown, step_last, no_marks,
// no_click (#463 KR-1: now no_effect); step_application (step 1 as `session app --record` leaves an application step, its
// application operation recorded on the step's frame: the test has no device, so the recording's
// file says it); clears_optional (--not-optional on the inserted step); not_inserted (a recorded
// step, a serial number no step has). A frame of another ratio is record_frame_size_mismatch.
#[test]
fn r4_each_optional_after_problem_is_refused_with_its_word() {
    let active = Recording::new();
    active.record_path();
    let before = active.recording_bytes();
    for value in [
        active.insert(1, "popup", &popup(), json!([color(POPUP_MARK, POPUP_AREA)])),
        json!({"remove_inserted": 1}),
    ] {
        let error = active.mark(value).expect_err("an active recording");
        assert_eq!(
            problem(error),
            OptionalAfterProblem::RecordingActive.as_str()
        );
        assert_eq!(active.recording_bytes(), before, "nothing written");
    }

    let recording = Recording::new();
    recording.stopped();
    let before = recording.recording_bytes();
    let popup_request = |step: u32, add: Value| recording.insert(step, "popup", &popup(), add);
    let mut with_step = popup_request(1, json!([color(POPUP_MARK, POPUP_AREA)]));
    with_step["step"] = json!(1);
    let mut with_removal = popup_request(1, json!([color(POPUP_MARK, POPUP_AREA)]));
    with_removal["remove_inserted"] = json!(3);
    for (value, members) in [
        (with_step, json!(["step"])),
        (with_removal, json!(["remove_inserted"])),
    ] {
        let error = recording
            .mark(value)
            .expect_err("an insertion stands alone");
        let details = error.details.clone().expect("details");
        assert_eq!(
            problem(error),
            OptionalAfterProblem::WithOtherChange.as_str()
        );
        assert_eq!(details["members"], members, "{details}");
        assert_eq!(recording.recording_bytes(), before, "nothing written");
    }
    let mut no_click = popup_request(1, json!([color(POPUP_MARK, POPUP_AREA)]));
    let removed = no_click["optional_after"]
        .as_object_mut()
        .expect("optional_after")
        .remove("click");
    assert!(removed.is_some(), "{no_click}");
    for (value, word) in [
        (
            popup_request(9, json!([color(POPUP_MARK, POPUP_AREA)])),
            OptionalAfterProblem::StepUnknown,
        ),
        (
            popup_request(2, json!([color(POPUP_MARK, POPUP_AREA)])),
            OptionalAfterProblem::StepLast,
        ),
        (popup_request(1, json!([])), OptionalAfterProblem::NoMarks),
        // #463 KR-1: the word is `no_effect`, the inserted step declaring neither a click nor a key.
        (no_click, OptionalAfterProblem::NoEffect),
        (
            json!({"remove_inserted": 1}),
            OptionalAfterProblem::NotInserted,
        ),
        (
            json!({"remove_inserted": 9}),
            OptionalAfterProblem::NotInserted,
        ),
    ] {
        let error = recording
            .mark(value)
            .expect_err("the insertion cannot hold");
        assert_eq!(problem(error), word.as_str());
        assert_eq!(recording.recording_bytes(), before, "nothing written");
    }
    let wide = Frame::from_pixels(
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
        .mark(recording.insert(1, "wide", &wide, json!([color(POPUP_MARK, POPUP_AREA)])))
        .expect_err("a frame of another ratio");
    assert_eq!(
        error.code, "record_frame_size_mismatch",
        "{}",
        error.message
    );
    assert_eq!(recording.recording_bytes(), before, "nothing written");

    recording.insert_popup();
    let inserted = recording.recording_bytes();
    let error = recording
        .mark(json!({"step": 3, "optional": false}))
        .expect_err("an inserted step stays optional");
    assert_eq!(
        problem(error),
        OptionalAfterProblem::ClearsOptional.as_str()
    );
    assert_eq!(recording.recording_bytes(), inserted, "nothing written");

    // Step 1 as an application step, its application operation recorded on its frame.
    let path = recording_path(&recording.state(), RECORD_ID);
    let mut file = recording.recording();
    file["steps"][0]["click"] = Value::Null;
    file["steps"][0]["application"] = json!({"action": "launch", "cli_verb": "launch",
        "source": "declared", "executed": null, "attempts": [], "needs_review": false});
    fs::write(&path, serde_json::to_vec(&file).expect("recording JSON")).expect("recording file");
    let application = recording.recording_bytes();
    let error = recording
        .mark(popup_request(1, json!([color("popup/other", POPUP_AREA)])))
        .expect_err("an application step");
    assert_eq!(
        problem(error),
        OptionalAfterProblem::StepApplication.as_str()
    );
    assert_eq!(recording.recording_bytes(), application, "nothing written");
}

// R4, item 3 of the brief, self-check item 7's two checks on an inserted step, each refused by
// record stop with the check's own code and nothing written: a pop-up whose page passes where it
// is absent (its color mark on the background the list shows too: record_optional_step_ambiguous
// against previous, the list's frame f0001), and a next page that passes under the pop-up (the
// pop-up's frame shows the next screen's patch: record_optional_skip_target_insensitive, the next
// screen as package step 3, the pop-up as step 2 and its frame f0003). Fix round 2 (item 1): on
// this recording the two numbers differ, and each refusal names both: the pop-up, package step 2,
// is serial number 3 (record_index, optional_record_index), the next screen, package step 3, is
// serial number 2.
#[test]
fn r4_item_7_refuses_an_inserted_step_that_fails_its_checks() {
    let recording = Recording::new();
    recording.stopped();
    recording
        .mark(recording.insert(
            1,
            "popup",
            &popup(),
            json!([color("popup/background", LIST_AREA)]),
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    let marked = recording.recording_bytes();

    let error = recording
        .stop()
        .expect_err("the pop-up's page passes on the list");

    assert_eq!(
        error.code, "record_optional_step_ambiguous",
        "{}",
        error.message
    );
    let details = error.details.expect("details");
    assert_eq!(details["against"], "previous", "{details}");
    assert_eq!(details["step"], 2, "{details}");
    assert_eq!(details["record_index"], 3, "{details}");
    assert_eq!(details["page"], POPUP_PAGE, "{details}");
    assert_eq!(details["frames"], json!(["f0001"]), "{details}");
    assert_eq!(recording.recording_bytes(), marked, "nothing written");

    let recording = Recording::new();
    recording.stopped();
    recording
        .mark(recording.insert(
            1,
            "popup",
            &screen(true, &[(POPUP_AREA, POPUP_GRAY)]),
            json!([color(POPUP_MARK, POPUP_AREA)]),
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    let marked = recording.recording_bytes();

    let error = recording
        .stop()
        .expect_err("the next page passes under the pop-up");

    assert_eq!(
        error.code, "record_optional_skip_target_insensitive",
        "{}",
        error.message
    );
    let details = error.details.expect("details");
    assert_eq!(details["step"], 3, "{details}");
    assert_eq!(details["record_index"], 2, "{details}");
    assert_eq!(details["optional_step"], 2, "{details}");
    assert_eq!(details["optional_record_index"], 3, "{details}");
    assert_eq!(details["frames"], json!(["f0003"]), "{details}");
    assert_eq!(recording.recording_bytes(), marked, "nothing written");
}

// R4, item 4 of the brief: remove_inserted (--remove-inserted) gives the inserted step up: status
// optional_step_removed, the step dropped and kept in the file, record status listing it where it
// stood without a package number, and record stop regenerates the package it generated before the
// insertion but for its generation time; the package with the pop-up had another content hash.
#[test]
fn r4_giving_up_the_inserted_step_gives_back_the_package_generated_before() {
    let recording = Recording::new();
    let generated = recording.stopped();
    let package = without_generation_time(recording.package(&generated));
    let before = recording.package_sha256(&generated);
    recording.insert_popup();
    let with_popup = recording
        .stop()
        .unwrap_or_else(|error| panic!("record stop: {} {:?}", error.code, error.details));
    assert_ne!(recording.package_sha256(&with_popup), before);

    let removed = outcome_json(
        &recording
            .mark(json!({"remove_inserted": 3}))
            .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details)),
    );

    assert_eq!(removed["status"], "optional_step_removed", "{removed}");
    assert_eq!(removed["step"], 3, "{removed}");
    let stored = recording.recording();
    assert_eq!(stored["steps"][2]["index"], 3, "{stored}");
    assert_eq!(stored["steps"][2]["dropped"], json!(true), "{stored}");
    assert_eq!(
        recording.status_steps(),
        [
            (json!(1), json!(1)),
            (json!(3), Value::Null),
            (json!(2), json!(2))
        ]
    );
    let regenerated = recording
        .stop()
        .unwrap_or_else(|error| panic!("record stop: {} {:?}", error.code, error.details));
    assert_eq!(regenerated["lab_status"], "regenerated", "{regenerated}");
    assert_eq!(
        without_generation_time(recording.package(&regenerated)),
        package
    );
}

// R4, item 2 of the brief: a recording without an inserted step has no inserted_after anywhere:
// not in recording.json, not in record status, not in the package's provenance; record status
// lists its steps by their serial numbers. (gate_463_r1's constants test pins the bytes of such a
// package, unchanged.)
#[test]
fn r4_a_recording_without_an_inserted_step_is_as_before() {
    let recording = Recording::new();
    let lab = recording.stopped();

    let text = String::from_utf8(recording.recording_bytes()).expect("recording text");
    assert!(!text.contains("inserted_after"), "{text}");
    assert_eq!(
        recording.status_steps(),
        [(json!(1), json!(1)), (json!(2), json!(2))]
    );
    let status = serde_json::to_value(
        record_status(&recording.state(), INSTANCE)
            .unwrap_or_else(|error| panic!("record status: {} {:?}", error.code, error.details)),
    )
    .expect("status JSON");
    assert!(!status.to_string().contains("inserted_after"), "{status}");
    let task = task(&recording.package(&lab));
    assert!(!task.to_string().contains("inserted_after"), "{task}");
    assert_eq!(lab["optional_steps"], json!([]), "{lab}");
}

// R4, item 1 of the brief: the pop-up's frame given at twice the recording's size, a pure scale, is
// converted to the recording's 96x64 as the Runtime converts every frame, kept with scaled_from
// 192x128, its mark self-tested on the converted frame, and record stop regenerates the package.
#[test]
fn r4_a_pop_up_frame_of_a_pure_scale_is_converted_to_the_recordings_size() {
    let recording = Recording::new();
    recording.stopped();

    let marked = outcome_json(
        &recording
            .mark(recording.insert(
                1,
                "doubled",
                &doubled(&popup()),
                json!([color(POPUP_MARK, POPUP_AREA)]),
            ))
            .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details)),
    );

    let frame = &marked["frame"];
    assert_eq!(frame["width"], WIDTH, "{frame}");
    assert_eq!(frame["height"], HEIGHT, "{frame}");
    assert_eq!(
        frame["scaled_from"],
        json!({"width": 2 * WIDTH, "height": 2 * HEIGHT}),
        "{frame}"
    );
    assert_eq!(
        marked["marks"][0]["self_test"]["status"], "passed",
        "{marked}"
    );
    let stored = recording.recording();
    assert_eq!(stored["steps"][2]["frames"][0], *frame, "{stored}");
    let kept = Frame::from_png(
        fs::read(frame["path"].as_str().expect("frame path")).expect("stored frame"),
        CaptureBackendName::AdbScreencap,
    )
    .expect("stored PNG");
    let shown = Frame::from_png(popup(), CaptureBackendName::AdbScreencap).expect("fixture PNG");
    assert_eq!(
        kept.pixels, shown.pixels,
        "the converted frame is the screen as recorded"
    );
    let lab = recording
        .stop()
        .unwrap_or_else(|error| panic!("record stop: {} {:?}", error.code, error.details));
    assert_eq!(lab["lab_status"], "regenerated", "{lab}");
}

// Fix round 2, item 2: from a failed run to the step to insert after. On the recording with the
// fixture's pop-up inserted after step 1 (serial number 3, package step 2), a run that stops on an
// unrecorded screen after the pop-up's press names it by pack_operation_id, the package's operation
// that starts on the pop-up's page: step_02_click. In record status the row whose artifact_step is
// that number, 2, has the index 3, another number. The second pop-up inserted after that index
// joins the run of step 1 (inserted_after 1) right after the first: record status lists 1, 3, 4, 2
// numbered 1 to 4; the regenerated package starts step_02_click on the first pop-up's page and the
// new step_03_click on the second's (step_03_reward), before the next screen; and the kernel's
// interpreter over the list, both pop-ups and the next screen presses three times, the second press
// inside the first pop-up's click box and the third inside the second's, and ends Completed on
// neutral/step_04.
#[test]
fn r4_a_runs_package_number_leads_to_the_serial_number_to_insert_after() {
    let (recording, lab) = with_popup();
    let task_ran = task(&recording.package(&lab));
    let stopped_at = task_ran["operations"]
        .as_array()
        .expect("operations")
        .iter()
        .find(|operation| operation["from"] == POPUP_PAGE)
        .and_then(|operation| operation["id"].as_str())
        .expect("the pop-up's operation")
        .to_owned();
    assert_eq!(stopped_at, "step_02_click");
    let number: u32 = stopped_at
        .strip_prefix("step_")
        .and_then(|rest| rest.strip_suffix("_click"))
        .and_then(|digits| digits.parse().ok())
        .expect("the number inside the package");

    let index = recording
        .status_steps()
        .into_iter()
        .find(|(_, artifact_step)| *artifact_step == json!(number))
        .map(|(index, _)| index)
        .expect("the row with that artifact_step");

    assert_eq!(index, json!(3));
    assert_ne!(index, json!(number));
    let serial = index
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .expect("a serial number");
    let inserted = outcome_json(&recording.insert_reward(serial));
    assert_eq!(inserted["status"], "optional_step_inserted", "{inserted}");
    assert_eq!(inserted["step"], 4, "{inserted}");
    assert_eq!(inserted["step_state"]["inserted_after"], 1, "{inserted}");
    assert_eq!(
        recording.status_steps(),
        [
            (json!(1), json!(1)),
            (json!(3), json!(2)),
            (json!(4), json!(3)),
            (json!(2), json!(4))
        ]
    );
    let regenerated = recording
        .stop()
        .unwrap_or_else(|error| panic!("record stop: {} {:?}", error.code, error.details));
    assert_eq!(regenerated["lab_status"], "regenerated", "{regenerated}");
    let entries = recording.package(&regenerated);
    let task = task(&entries);
    assert_eq!(
        operation_starts(&task),
        [
            (json!("step_01_click"), json!("step_01")),
            (json!("step_02_click"), json!(POPUP_PAGE)),
            (json!("step_03_click"), json!("step_03_reward"))
        ]
    );
    assert_eq!(
        record_indices(&task),
        [json!(1), json!(3), json!(4), json!(2)]
    );
    let prepared = admit(&entries, &recording.ocr)
        .unwrap_or_else(|error| panic!("admission: {} {:?}", error.code, error.details));
    let mut device = Device::showing(vec![list(), popup(), reward(), next()]);
    let outcome = prepared
        .run(&mut device)
        .unwrap_or_else(|error| panic!("the run: {error:?}"));
    assert_eq!(outcome.outcome, TaskOutcome::Success, "{outcome:?}");
    assert_eq!(
        outcome.final_page.as_deref(),
        Some("neutral/step_04"),
        "{outcome:?}"
    );
    assert_eq!(outcome.executed_steps, 3, "{outcome:?}");
    assert_eq!(device.inputs.len(), 3, "{:?}", device.inputs);
    for (input, area) in device.inputs.iter().zip([CLICK, POPUP_CLICK, REWARD_CLICK]) {
        assert!(tapped_inside(input, area), "{input:?} outside {area:?}");
    }
}

// Fix round 2, item 7: two pop-ups inserted after the same step 1 (the fixture's, serial number 3,
// then the second, serial number 4): record status lists them after step 1 in the order they were
// inserted (1, 3, 4, 2, numbered 1 to 4), and record stop generates both in that order (pages
// step_02_notice and step_03_reward before the next screen step_04, the provenance naming their
// serial numbers). Giving up the first leaves the other: record status lists 3 without a package
// number, and record stop generates the second pop-up alone after step 1 (step_02_reward before
// step_03).
#[test]
fn r4_two_steps_inserted_after_the_same_step_run_in_the_order_inserted() {
    let recording = Recording::new();
    recording.stopped();
    recording.insert_popup();
    let second = outcome_json(&recording.insert_reward(1));
    assert_eq!(second["step"], 4, "{second}");
    assert_eq!(second["step_state"]["inserted_after"], 1, "{second}");
    assert_eq!(
        recording.status_steps(),
        [
            (json!(1), json!(1)),
            (json!(3), json!(2)),
            (json!(4), json!(3)),
            (json!(2), json!(4))
        ]
    );

    let both = recording
        .stop()
        .unwrap_or_else(|error| panic!("record stop: {} {:?}", error.code, error.details));

    assert_eq!(
        both["pages"],
        json!(["step_01", POPUP_PAGE, "step_03_reward", "step_04"]),
        "{both}"
    );
    let task_both = task(&recording.package(&both));
    assert_eq!(
        operation_starts(&task_both),
        [
            (json!("step_01_click"), json!("step_01")),
            (json!("step_02_click"), json!(POPUP_PAGE)),
            (json!("step_03_click"), json!("step_03_reward"))
        ]
    );
    assert_eq!(
        record_indices(&task_both),
        [json!(1), json!(3), json!(4), json!(2)]
    );

    recording
        .mark(json!({"remove_inserted": 3}))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    assert_eq!(
        recording.status_steps(),
        [
            (json!(1), json!(1)),
            (json!(3), Value::Null),
            (json!(4), json!(2)),
            (json!(2), json!(3))
        ]
    );
    let one = recording
        .stop()
        .unwrap_or_else(|error| panic!("record stop: {} {:?}", error.code, error.details));
    assert_eq!(
        one["pages"],
        json!(["step_01", "step_02_reward", "step_03"]),
        "{one}"
    );
    let task_one = task(&recording.package(&one));
    assert_eq!(
        operation_starts(&task_one),
        [
            (json!("step_01_click"), json!("step_01")),
            (json!("step_02_click"), json!("step_02_reward"))
        ]
    );
    assert_eq!(record_indices(&task_one), [json!(1), json!(4), json!(2)]);
}

// Fix round 2, item 7: giving up an inserted step one of whose marks another page forbids (the next
// screen, step 2, forbids the pop-up's popup/notice) is refused as --drop-step refuses it:
// record_mark_in_use naming the step and the mark, and nothing is written.
#[test]
fn r4_giving_up_an_inserted_step_whose_mark_another_page_forbids_is_refused() {
    let recording = Recording::new();
    recording.stopped();
    recording.insert_popup();
    recording
        .mark(json!({"step": 2, "forbid": [POPUP_MARK]}))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    let forbidden = recording.recording_bytes();

    let error = recording
        .mark(json!({"remove_inserted": 3}))
        .expect_err("a mark the next page forbids");

    assert_eq!(error.code, "record_mark_in_use", "{}", error.message);
    let details = error.details.expect("details");
    assert_eq!(details["step"], 3, "{details}");
    assert_eq!(details["marks"], json!([POPUP_MARK]), "{details}");
    assert_eq!(recording.recording_bytes(), forbidden, "nothing written");
}

// P4, the design's first test: a second record stop of the same recording keeps the output the
// first one wrote, set aside as <name>.old-<ms> beside the new one and named in lab.kept: its bytes
// are the first package's, nothing else is left beside them, and the new package is another.
// Adapted by #463 S2-10 (register §1.441, fix round 2; contracts/lab-recording.md, "Regression
// cases written with the package"): the output directory also holds the package's regression cases,
// <package_id>.cases, and after the second stop the first stop's cases set aside as
// <package_id>.cases.old-<ms> (lab.cases.kept), holding the first stop's cases; the package and the
// cases one stop sets aside carry the same <ms>. The listing stays exact.
#[test]
fn p4_a_second_stop_keeps_the_output_the_first_wrote() {
    let recording = Recording::new();
    let generated = recording.stopped();
    let first_path = PathBuf::from(generated["path"].as_str().expect("package path"));
    let first = fs::read(&first_path).expect("the first package");
    assert_eq!(generated.get("kept"), None, "{generated}");
    let mut first_cases = generated["cases"]["ids"]
        .as_array()
        .unwrap_or_else(|| panic!("no cases in {generated}"))
        .iter()
        .map(|id| id.as_str().expect("a case id").to_owned())
        .collect::<Vec<_>>();
    assert!(!first_cases.is_empty(), "{generated}");
    first_cases.sort();
    recording.insert_popup();

    let regenerated = recording
        .stop()
        .unwrap_or_else(|error| panic!("record stop: {} {:?}", error.code, error.details));

    let name = first_path
        .file_name()
        .and_then(|name| name.to_str())
        .expect("file name")
        .to_owned();
    let kept_files = regenerated["kept"]
        .as_array()
        .unwrap_or_else(|| panic!("no kept in {regenerated}"));
    assert_eq!(kept_files.len(), 1, "{regenerated}");
    let kept = PathBuf::from(kept_files[0].as_str().expect("kept path"));
    let kept_name = kept
        .file_name()
        .and_then(|name| name.to_str())
        .expect("kept name")
        .to_owned();
    assert!(
        kept_name.starts_with(&format!("{name}.old-")),
        "{kept_name}"
    );
    assert_eq!(fs::read(&kept).expect("the kept package"), first);
    let cases = format!(
        "{}.cases",
        regenerated["package_id"].as_str().expect("package id")
    );
    let kept_cases = PathBuf::from(
        regenerated["cases"]["kept"]
            .as_str()
            .unwrap_or_else(|| panic!("no kept cases in {regenerated}")),
    );
    let kept_cases_name = kept_cases
        .file_name()
        .and_then(|name| name.to_str())
        .expect("kept cases name")
        .to_owned();
    // One stop, one suffix: the package and the cases it set aside carry the same <ms>.
    let suffix = kept_name
        .strip_prefix(&format!("{name}.old-"))
        .expect("the kept package's <ms>");
    assert_eq!(
        kept_cases_name,
        format!("{cases}.old-{suffix}"),
        "{regenerated}"
    );
    // The kept cases are the first stop's: a directory for each of its cases, nothing else.
    assert_eq!(names_in(&kept_cases), first_cases, "{regenerated}");
    let directory = first_path.parent().expect("out directory");
    // #463 S2-10 fix round 3 (the review of #1797): both kept members of the answer, whole: lab.kept
    // is the package set aside and nothing else, lab.cases.kept the cases set aside, both in the
    // output directory under the one suffix.
    assert_eq!(
        kept_files
            .iter()
            .map(|path| PathBuf::from(path.as_str().expect("kept path")))
            .collect::<Vec<_>>(),
        [directory.join(format!("{name}.old-{suffix}"))],
        "{regenerated}"
    );
    assert_eq!(
        kept_cases,
        directory.join(format!("{cases}.old-{suffix}")),
        "{regenerated}"
    );
    // The whole output directory, sorted: the cases, the kept cases, the package, the kept package.
    assert_eq!(
        names_in(directory),
        [
            cases.as_str(),
            kept_cases_name.as_str(),
            name.as_str(),
            kept_name.as_str()
        ]
    );
    let second = fs::read(&first_path).expect("the second package");
    assert_ne!(second, first);
    assert_eq!(
        second,
        fs::read(regenerated["path"].as_str().expect("path")).expect("package")
    );
}

// P4, the design's second test: a --lab-dir inside an installed root, or the root itself, is
// refused record_lab_dir_invalid with inside_installed_root and the root, under --dry-run too,
// before anything is written: the recording stays active, no package is written and the directory
// is not created. Fix round 2 (item 3): an installed root is a directory holding the installer's
// state file install\state.json (contracts/install-cli.md, "Layout"); a plain folder named Install
// beside the target (a folder of installers) is none, and a --dry-run stop into that target
// validates.
#[test]
fn p4_a_lab_dir_inside_an_installed_root_is_refused() {
    let recording = Recording::new();
    recording.record_path();
    let root = recording.temp.path().join("installed");
    fs::create_dir_all(root.join("install")).expect("install directory");
    fs::write(root.join("install").join("state.json"), b"{}").expect("install state");
    fs::create_dir_all(root.join("packages")).expect("packages directory");
    let inside = root.join("packages").join("neutral");
    let before = recording.recording_bytes();

    for (dir, dry_run) in [(&inside, false), (&inside, true), (&root, false)] {
        let error = recording
            .stop_with(&RecordStopOptions {
                lab_dir: Some(dir.display().to_string()),
                dry_run,
                ..RecordStopOptions::default()
            })
            .expect_err("a --lab-dir inside an installed root");

        assert_eq!(error.code, "record_lab_dir_invalid", "{}", error.message);
        let details = error.details.expect("details");
        assert_eq!(details["reason"], "inside_installed_root", "{details}");
        assert_eq!(
            details["installed_root"],
            std::path::absolute(&root)
                .expect("absolute root")
                .display()
                .to_string(),
            "{details}"
        );
        assert_eq!(recording.recording_bytes(), before, "nothing written");
        assert_eq!(recording.recording()["status"], "active");
        assert!(!inside.exists(), "the directory is not created");
    }

    let downloads = recording.temp.path().join("downloads");
    fs::create_dir_all(downloads.join("Install")).expect("a folder of installers");
    let beside = downloads.join("packages");
    let lab = recording
        .stop_with(&RecordStopOptions {
            lab_dir: Some(beside.display().to_string()),
            dry_run: true,
            ..RecordStopOptions::default()
        })
        .unwrap_or_else(|error| panic!("record stop: {} {:?}", error.code, error.details));
    assert_eq!(lab["lab_status"], "validated", "{lab}");
    assert_eq!(recording.recording_bytes(), before, "nothing written");
    assert!(!beside.exists(), "the directory is not created");
}

// P4, the design's third test: a recording that meets none of these cases (a first stop, a
// --lab-dir outside every installed root) writes its package as before: one file in the
// recording's out directory and one, the same bytes, in --lab-dir, nothing set aside and no kept;
// the answer keeps binding_example and adds adoption_example, the adopt line with the recording's
// id and state directory and the decision's skeleton. Fix round 2 (item 4, the owner's rule that a
// package not yet run through on a real device is registered and stays held): the skeleton is a
// decision of the class held with its reason blank and nothing else. Adapted by #463 S2-10 (register
// §1.441, fix round 2): the recording's out directory holds, beside the package, its regression
// cases, <package_id>.cases, and nothing else; --lab-dir holds the package alone.
#[test]
fn p4_a_first_stop_writes_as_before_and_answers_the_adoption_skeleton() {
    let recording = Recording::new();
    recording.record_path();
    let lab_dir = recording.temp.path().join("packages");

    let lab = recording
        .stop_with(&RecordStopOptions {
            lab_dir: Some(lab_dir.display().to_string()),
            ..RecordStopOptions::default()
        })
        .unwrap_or_else(|error| panic!("record stop: {} {:?}", error.code, error.details));

    assert_eq!(lab["lab_status"], "generated", "{lab}");
    assert_eq!(lab["lab_dir_status"], "written", "{lab}");
    // #463 S2-10 fix round 3 (the review of #1797): neither kept member of the answer, the
    // package's (lab.kept) nor the cases' (lab.cases.kept), is there.
    assert_eq!(lab.get("kept"), None, "{lab}");
    assert_eq!(lab["cases"].get("kept"), None, "{lab}");
    let copy = PathBuf::from(lab["path"].as_str().expect("package path"));
    let name = copy
        .file_name()
        .and_then(|name| name.to_str())
        .expect("file name")
        .to_owned();
    let cases = format!("{}.cases", lab["package_id"].as_str().expect("package id"));
    assert_eq!(
        names_in(copy.parent().expect("out directory")),
        [cases.as_str(), name.as_str()]
    );
    assert_eq!(names_in(&lab_dir), [name.as_str()]);
    assert_eq!(
        fs::read(lab_dir.join(&name)).expect("--lab-dir package"),
        fs::read(&copy).expect("the recording's copy")
    );
    assert!(lab["binding_example"].is_object(), "{lab}");
    let state = std::path::absolute(recording.state())
        .expect("absolute state")
        .display()
        .to_string();
    assert_eq!(
        lab["adoption_example"],
        json!({
            "command": format!(
                "actinglab standard-pack adopt --repo <checkout> --record-id {RECORD_ID} \
                 --state-dir \"{state}\" --decide <decision.json>"
            ),
            "decision": {
                "schema_version": "actingcommand.pack-decision.v1",
                "class": "held",
                "held": {"reason": ""}
            }
        }),
        "{lab}"
    );
}
