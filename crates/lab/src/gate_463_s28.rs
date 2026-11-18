// SPDX-License-Identifier: AGPL-3.0-only

// #463 S2-8' (contracts/lab-recording.md, "Folding swipes into a scrolling search"; register
// §1.456): the swipes recorded before a press folded into one scrolling search of that press
// (`fold_scan`), and the fold undone (`unfold_scan`). Recordings are made as stage one makes them
// (taps and swipes through `record_press` with a fake Runtime, its captures taking the scripted
// screens and the caller's clock moving only by its waits): a tap on the start screen opens a list,
// swipes scroll it (each a step with its screen before and after), a tap on a row opens the detail
// screen. With three swipes the steps are 1 (the tap), 2 to 4 (the swipes), 5 (the press on the
// row) and 6 (the detail screen). Frames are synthetic 160x90 screens of gray with squares of one
// colour; no Runtime, no device, no OCR model; the tests read no clock and never sleep.
// - `s28_three_forward_swipes_and_a_tap_are_folded_into_the_press` (1): the members the fold
//   writes, every other member as it was; the effective steps by `record status`; the press step's
//   sheet in `record review`; `record frames` and `record review` of a folded swipe.
// - `s28_forward_and_back_is_folded_and_forward_after_back_is_refused` (2).
// - `s28_each_cause_of_a_range_and_of_a_loss_is_refused_and_nothing_is_written` (3).
// - `s28_swipes_of_different_lengths_fold_and_an_unclear_axis_is_refused` (4).
// - `s28_unfold_gives_the_step_table_back_and_the_package_is_the_one_without_the_fold` (5).
// - `s28_a_fold_without_a_scrolling_select_is_listed_and_refuses_record_stop` (6).
// - `s28_a_folded_swipe_has_no_page_and_no_case_and_unfolded_the_cases_are_as_before` (7).
// - `s28_each_step_action_on_a_step_of_a_fold_is_refused` (8).
// Fix round 1 (rd-1117/brief-463-s28-fix1.md, after the independent review; the helpers that press
// are `Bench::tapped` and `Bench::swiped`, since the workspace guard finds a call of a method named
// `tap` or `swipe` by its name):
// - `s28_fix1_a_stopped_recording_folds_as_an_amendment_and_unfolded_generates_again`.
// - `s28_fix1_a_fold_after_the_stop_is_refused_what_only_a_stopped_recording_holds`.
// - `s28_fix1_every_other_step_operation_stays_refused_on_a_stopped_recording`.
// - `s28_fix1_the_axis_rule_at_its_edges_and_a_fold_along_the_horizontal_axis`.
// - `s28_fix1_the_archive_keeps_the_folded_swipes_in_its_material`.

use crate::recording::store::{lab_dir, old_record_path, recording_path};
use crate::{
    AfterCapture, AttachFrameRequest, CapturedFrame, ClickEffect, ClickPlan, CommitClickRequest,
    LAB_RECORD_MARK_SCHEMA, LabCode, LabOcr, MarkOutcome, MarkRequest, PlanClickRequest,
    PlannedSwipe, PointPress, RecordArchiveOptions, RecordFramePorts, RecordFramesRequest,
    RecordPoint, RecordPressRequest, RecordRect, RecordReviewRequest, RecordStartOptions,
    RecordStopOptions, RecordingLock, record_archive, record_frames, record_mark, record_press,
    record_ready, record_review, record_start, record_status, record_stop,
};
use actingcommand_contract::{InputFrameReference, LabError, LabResult};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_pack_containment::{
    ContainmentLimits, ContentContainer, expand_content_container,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_0463_s28";
const TASK_ID: &str = "neutral_task";
const STARTED_AT: u64 = 1_700_000_000_000;
const WIDTH: u32 = 160;
const HEIGHT: u32 = 90;
const GRAY: [u8; 3] = [60, 60, 60];
const RED: [u8; 3] = [230, 20, 20];
const GREEN: [u8; 3] = [20, 230, 20];
const BLUE: [u8; 3] = [20, 20, 230];
const YELLOW: [u8; 3] = [230, 230, 20];
const CYAN: [u8; 3] = [20, 230, 230];
/// The self-checks of `record stop`, in the order they run.
const EVERY_CHECK: [&str; 8] = [
    "frames",
    "admission",
    "first_decision",
    "cross_check",
    "endings",
    "selects",
    "final_package",
    "cases",
];

const fn rect(x: i32, y: i32, width: i32, height: i32) -> RecordRect {
    RecordRect {
        x,
        y,
        width,
        height,
    }
}

/// Each screen's page square, a colour mark's region: red on the start screen, green on the list,
/// yellow on the detail screen, cyan on a loading screen.
const SQUARE: RecordRect = rect(0, 0, 16, 16);
/// The tap on the start screen and the press on the wanted row, on no square.
const START_POINT: (i32, i32) = (130, 70);
const ROW_POINT: (i32, i32) = (56, 60);

/// A swipe by its two points.
type Swipe = ((i32, i32), (i32, i32));
/// Up the list by 40 px, and back down by as much.
const FORWARD: Swipe = ((80, 70), (80, 30));
const BACK: Swipe = ((80, 30), (80, 70));
/// Up by 20 px, and up by 70 px with 2 px across: along the same axis in the same direction.
const SHORT_FORWARD: Swipe = ((80, 70), (80, 50));
const LONG_FORWARD: Swipe = ((79, 80), (81, 10));
/// 30 px across and 30 px up: along neither axis clearly.
const DIAGONAL: Swipe = ((80, 70), (110, 40));
/// 80 px to the left: along the other axis.
const ACROSS: Swipe = ((120, 45), (40, 45));
/// Fix round 1: 80 px to the right, back along the axis of `ACROSS`.
const ACROSS_BACK: Swipe = ((40, 45), (120, 45));
/// Fix round 1: 40 px up and 20 px across, its move along the axis exactly twice its move across.
const TWICE: Swipe = ((80, 70), (100, 30));
/// Fix round 1: 40 px up and 21 px across, its move along the axis less than twice its move across.
const NEARLY_TWICE: Swipe = ((80, 70), (101, 30));
/// Fix round 1: a swipe that ends where it began, no move at all.
const STILL: Swipe = ((80, 50), (80, 50));
/// Fix round 1: a pop-up's square and its close click, of an optional step inserted after the stop.
const MAGENTA: [u8; 3] = [230, 20, 230];
const NOTICE_CLICK: RecordRect = rect(60, 40, 20, 10);

// Whether `area` covers the pixel at `(x, y)`.
fn covers(area: RecordRect, x: u32, y: u32) -> bool {
    let (column, row) = (i64::from(x), i64::from(y));
    column >= i64::from(area.x)
        && row >= i64::from(area.y)
        && column < i64::from(area.x) + i64::from(area.width)
        && row < i64::from(area.y) + i64::from(area.height)
}

// The PNG of a gray screen with each of `squares` in its colour.
fn screen(squares: &[(RecordRect, [u8; 3])]) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let color = squares
                .iter()
                .find(|(area, _)| covers(*area, x, y))
                .map_or(GRAY, |(_, color)| *color);
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

fn start() -> Vec<u8> {
    screen(&[(SQUARE, RED)])
}

// The list scrolled to `position`: its green square, and a blue row 10 px lower each position.
fn list(position: i32) -> Vec<u8> {
    screen(&[(SQUARE, GREEN), (rect(40, 20 + 10 * position, 32, 8), BLUE)])
}

fn detail() -> Vec<u8> {
    screen(&[(SQUARE, YELLOW)])
}

fn loading() -> Vec<u8> {
    screen(&[(SQUARE, CYAN)])
}

// Fix round 1: a pop-up a run met, its magenta square.
fn notice() -> Vec<u8> {
    screen(&[(SQUARE, MAGENTA)])
}

// The colour mark `id` over `area`.
fn color(id: &str, area: RecordRect) -> Value {
    json!({"id": id, "family": "color", "region": area})
}

fn request(mut value: Value) -> MarkRequest {
    value["schema_version"] = json!(LAB_RECORD_MARK_SCHEMA);
    serde_json::from_value(value).expect("a record mark request")
}

// The step action `fold_scan` of the press `step` and its first swipe `first`.
fn fold(step: u32, first: u32) -> Value {
    json!({"step_action": {"kind": "fold_scan", "step": step, "first": first}})
}

// The step action `kind` on `step`.
fn action(kind: &str, step: u32) -> Value {
    json!({"step_action": {"kind": kind, "step": step}})
}

fn point(at: (i32, i32)) -> RecordPoint {
    RecordPoint { x: at.0, y: at.1 }
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

// The screen after a press as one capture 800 ms after it, without a stillness test.
fn one_capture_after() -> AfterCapture {
    AfterCapture {
        still_ms: Some(0),
        ..AfterCapture::for_press(Some(800))
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

// The step table's bytes with the digits of its `updated_at_unix_ms`, the time it was last written,
// which every write sets, replaced by `0`; every other byte as it is.
fn but_update_time(bytes: &[u8]) -> Vec<u8> {
    let text = std::str::from_utf8(bytes).expect("a step table of UTF-8");
    let key = "\"updated_at_unix_ms\":";
    assert_eq!(text.matches(key).count(), 1, "{text}");
    let at = text.find(key).expect("the update time") + key.len();
    let (head, rest) = text.split_at(at);
    let value = rest.trim_start();
    let spaces = &rest[..rest.len() - value.len()];
    let digits = value.bytes().take_while(u8::is_ascii_digit).count();
    assert!(digits > 0, "{text}");
    format!("{head}{spaces}0{}", &value[digits..]).into_bytes()
}

// The entries of the package `record stop` wrote.
fn package(lab: &Value) -> BTreeMap<String, Vec<u8>> {
    let bytes = fs::read(lab["path"].as_str().expect("package path")).expect("package");
    let container = if lab["container"] == "zip" {
        ContentContainer::Zip
    } else {
        ContentContainer::Json
    };
    expand_content_container(&bytes, container, ContainmentLimits::default())
        .expect("package container")
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

// The row of `step` among `rows` (the steps of the step table, of `record status` or of `record
// review`), by the member `key` that holds its serial number.
fn row<'v>(rows: &'v Value, key: &str, step: u32) -> &'v Value {
    rows.as_array()
        .unwrap_or_else(|| panic!("no rows in {rows}"))
        .iter()
        .find(|item| item[key] == step)
        .unwrap_or_else(|| panic!("no step {step} in {rows}"))
}

fn step_of(recording: &Value, step: u32) -> &Value {
    row(&recording["steps"], "index", step)
}

// The details of a `record_scan_invalid` refusal whose `scan_problem` is `word`, in the details and
// in the outcome's values.
fn scan_refusal(error: LabError, word: &str) -> Value {
    assert_eq!(
        error.code,
        LabCode::RecordScanInvalid.as_str(),
        "{}: {:?}",
        error.message,
        error.details
    );
    let details = error.details.expect("details");
    assert_eq!(details["scan_problem"], word, "{details}");
    assert_eq!(
        details["outcome"]["values"]["scan_problem"], word,
        "{details}"
    );
    details
}

// A state directory with an active session of `INSTANCE` and its Lab recording, its game, server
// and locale known, without an OCR model. The lock is held as one command holds it and dropped
// before the directory.
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
        let lock = RecordingLock::acquire(&state, INSTANCE, "gate_463_s28", "Lab-CLI")
            .expect("recording lock");
        let bench = Self {
            lock,
            ocr: LabOcr::from_vision_root(&temp.path().join("no-vision")),
            temp,
        };
        fs::write(
            old_record_path(&bench.state(), INSTANCE),
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

    fn state(&self) -> PathBuf {
        self.temp.path().join("state")
    }

    // `record_press` of `request`, the fake Runtime performing the input and its captures taking
    // `shots`: the screen before the press, then the screen after it; the press must be recorded.
    fn press(&self, request: &RecordPressRequest, shots: [Vec<u8>; 2]) {
        let mut device = Screens {
            shots: shots.into(),
            taken: 0,
            now: Duration::ZERO,
        };
        let mut send = |_: &ClickPlan, _: InputFrameReference| -> LabResult<CommitClickRequest> {
            Ok(receipt())
        };
        if let Err(error) = record_press(&self.lock, request, &mut send, &mut device) {
            panic!(
                "the press: {}: {} {:?}",
                error.code, error.message, error.details
            );
        }
    }

    // `tap <x> <y> --record` on the first of `shots`. (Fix round 1: not named `tap`, a write
    // method of `LabInputPort` that the workspace guard finds by name.)
    fn tapped(&self, at: (i32, i32), shots: [Vec<u8>; 2]) {
        let request = RecordPressRequest {
            click: PlanClickRequest::default(),
            point_press: Some(PointPress::Tap { point: point(at) }),
            after: one_capture_after(),
        };
        self.press(&request, shots);
    }

    // `swipe <x1> <y1> <x2> <y2> 300 --record` on the first of `shots`. (Fix round 1: not named
    // `swipe`, as `tapped` is not named `tap`.)
    fn swiped(&self, (from, to): Swipe, shots: [Vec<u8>; 2]) {
        let request = RecordPressRequest {
            click: PlanClickRequest {
                swipe: Some(PlannedSwipe {
                    from: point(from),
                    to: point(to),
                    duration_ms: 300,
                }),
                ..PlanClickRequest::default()
            },
            point_press: None,
            after: one_capture_after(),
        };
        self.press(&request, shots);
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

    // `record mark` of `value`, which must be refused `record_scan_invalid` with `word`, every file
    // as it was; the refusal's details.
    fn refused(&self, value: Value, word: &str) -> Value {
        let before = self.files();
        let error = self.mark(value.clone()).expect_err("a refused record mark");
        assert_eq!(self.files(), before, "a file changed: {value}");
        scan_refusal(error, word)
    }

    // `record mark` of `value`, which must be refused `validation_failed`, every file as it was.
    fn invalid(&self, value: Value) {
        let before = self.files();
        let error = self.mark(value.clone()).expect_err("a refused record mark");
        assert_eq!(error.code, "validation_failed", "{}", error.message);
        assert_eq!(self.files(), before, "a file changed: {value}");
    }

    fn recording_bytes(&self) -> Vec<u8> {
        fs::read(recording_path(&self.state(), RECORD_ID)).expect("step table")
    }

    fn recording(&self) -> Value {
        serde_json::from_slice(&self.recording_bytes()).expect("step table JSON")
    }

    fn files(&self) -> Vec<(String, Vec<u8>)> {
        files_under(&lab_dir(&self.state(), RECORD_ID))
    }

    // `record status`'s `lab`.
    fn status(&self) -> Value {
        let status = record_status(&self.state(), INSTANCE)
            .unwrap_or_else(|error| panic!("record status: {}: {}", error.code, error.message));
        serde_json::to_value(&status).expect("record status JSON")
    }

    // `record review [--step <k>] --no-preview`.
    fn review(&self, step: Option<u32>) -> Result<Value, LabError> {
        let request = RecordReviewRequest {
            record_id: None,
            step,
            no_preview: true,
            ruler: None,
        };
        record_review(
            &self.state(),
            INSTANCE,
            &request,
            &RecordStopOptions::default(),
            &self.ocr,
        )
        .map(|outcome| serde_json::to_value(&outcome).expect("record review JSON"))
    }

    // `record frames --step <k>`.
    fn frames(&self, step: u32) -> Result<Value, LabError> {
        let request = RecordFramesRequest {
            record_id: None,
            step,
            before: None,
            after: None,
        };
        record_frames(&self.state(), INSTANCE, &request)
            .map(|outcome| serde_json::to_value(&outcome).expect("record frames JSON"))
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

    // `record stop`, which must stand.
    fn stopped(&self) -> Value {
        self.stop(false).unwrap_or_else(|error| {
            panic!(
                "record stop: {}: {} {:?}",
                error.code, error.message, error.details
            )
        })
    }

    // Fix round 1: `record stop`, which must generate the package, and the session header then
    // stopped as the `record` command writes it, so that the recording is stopped with its package
    // as `record mark` after stop finds it; the stop's `lab`.
    fn stopped_session(&self) -> Value {
        let lab = self.stopped();
        assert_eq!(lab["lab_status"], "generated", "{lab}");
        let path = old_record_path(&self.state(), INSTANCE);
        let mut header: Value =
            serde_json::from_slice(&fs::read(&path).expect("session header")).expect("header");
        header["status"] = json!("stopped");
        fs::write(&path, serde_json::to_vec(&header).expect("header bytes"))
            .expect("the stopped session header");
        lab
    }

    // Fix round 1: a frame file on the caller's machine, outside the recording's directory, holding
    // `bytes`; its path.
    fn caller_frame(&self, name: &str, bytes: &[u8]) -> String {
        let path = self.temp.path().join(format!("{name}.png"));
        fs::write(&path, bytes).expect("frame file");
        path.display().to_string()
    }

    // Rewrites the step table with `member` of step `step` set to `value`, a recording file changed
    // by hand; the bytes it had before.
    fn edit_step(&self, step: u32, member: &str, value: Value) -> Vec<u8> {
        let path = recording_path(&self.state(), RECORD_ID);
        let original = fs::read(&path).expect("step table");
        let mut table: Value = serde_json::from_slice(&original).expect("step table JSON");
        let entry = table["steps"]
            .as_array_mut()
            .expect("steps")
            .iter_mut()
            .find(|entry| entry["index"] == step)
            .expect("the step");
        entry[member] = value;
        fs::write(
            &path,
            serde_json::to_vec_pretty(&table).expect("step table bytes"),
        )
        .expect("the edited step table");
        original
    }
}

// A recording of a list scrolled by `swipes`: a tap on the start screen opens the list at
// `positions[0]`, each swipe moves it from one position of `positions` to the next, and a tap on a
// row of the last opens the detail screen. Its steps: 1 the tap, 2 to `swipes.len() + 1` the
// swipes, then the press on the row, then the detail screen.
fn scrolled(swipes: &[Swipe], positions: &[i32]) -> Bench {
    assert_eq!(
        positions.len(),
        swipes.len() + 1,
        "a position around each swipe"
    );
    let bench = Bench::new();
    bench.tapped(START_POINT, [start(), list(positions[0])]);
    for (swipe, pair) in swipes.iter().zip(positions.windows(2)) {
        bench.swiped(*swipe, [list(pair[0]), list(pair[1])]);
    }
    bench.tapped(ROW_POINT, [list(positions[swipes.len()]), detail()]);
    bench
}

// Three swipes forward: steps 2, 3 and 4 the swipes, step 5 the press on the row.
fn three_forward() -> Bench {
    scrolled(&[FORWARD, FORWARD, FORWARD], &[0, 1, 2, 3])
}

// `three_forward` with a page on every step: step 1's red square, step 5's green one (the list),
// which the swipes' pages reuse, and step 6's yellow one; every pre-check of `record stop` passes.
fn paged() -> Bench {
    let bench = three_forward();
    bench.marked(json!({"step": 1, "add": [color("ui/start", SQUARE)]}));
    bench.marked(json!({"step": 5, "add": [color("ui/list", SQUARE)]}));
    for step in 2..5 {
        bench.marked(json!({"step": step, "reuse": ["ui/list"]}));
    }
    bench.marked(json!({"step": 6, "add": [color("ui/detail", SQUARE)]}));
    bench
}

// The steps `record review` answers, by their serial numbers.
fn reviewed_steps(review: &Value) -> Vec<Value> {
    review["steps"]
        .as_array()
        .expect("reviewed steps")
        .iter()
        .map(|sheet| sheet["step"].clone())
        .collect()
}

// The items of `record ready` that name step `step`, by their word.
fn items_of(ready: &Value, step: u32) -> Vec<Value> {
    ready["missing"]
        .as_array()
        .expect("missing")
        .iter()
        .filter(|item| item["step"] == step)
        .map(|item| item["what"].clone())
        .collect()
}

// The work order's test 1: three swipes forward and a tap, folded into the tap (`fold_scan` of step
// 5 from step 2). The swipes stay in the file whole, each with `folded_into: 5` and every other
// member as it was; the press step holds the fold (its first step, the vertical axis, each swipe
// forward, its time) and every other member as it was; nothing is moved or renumbered. `record
// status` numbers the effective steps 1, 5 and 6 as 1 to 3, the swipes none, and shows the fold on
// both sides. `record review` answers the effective steps; the press step's sheet lists the three
// swipes in order, each with its screen before (the list's first screen, then the screen after
// each swipe) and after and its press. A folded swipe is no effective step for `record review
// --step` (`step_in_fold`) and is listed by `record frames`, with no number in the package.
#[test]
fn s28_three_forward_swipes_and_a_tap_are_folded_into_the_press() {
    let bench = three_forward();
    let before = bench.recording();
    let marked = bench.marked(fold(5, 2));
    assert_eq!(marked.status, "scan_folded");
    assert_eq!(marked.step, Some(5));
    let folded = bench.recording();
    assert_eq!(
        folded["steps"].as_array().map(Vec::len),
        before["steps"].as_array().map(Vec::len),
        "{folded}"
    );
    for step in [2, 3, 4] {
        let mut kept = step_of(&folded, step).clone();
        let member = kept.as_object_mut().expect("a step").remove("folded_into");
        assert_eq!(member, Some(json!(5)), "{folded}");
        assert_eq!(&kept, step_of(&before, step), "step {step} stays whole");
    }
    let mut press = step_of(&folded, 5).clone();
    let fold_member = press
        .as_object_mut()
        .expect("a step")
        .remove("scan_fold")
        .expect("the fold on the press step");
    assert_eq!(&press, step_of(&before, 5), "the press step stays whole");
    assert!(fold_member["folded_at_unix_ms"].is_u64(), "{fold_member}");
    let mut written = fold_member.clone();
    written
        .as_object_mut()
        .expect("the fold")
        .remove("folded_at_unix_ms");
    assert_eq!(
        written,
        json!({"first": 2, "axis": "vertical", "moves": [
            {"step": 2, "direction": "forward"},
            {"step": 3, "direction": "forward"},
            {"step": 4, "direction": "forward"}
        ]}),
        "{folded}"
    );
    assert_eq!(
        serde_json::to_value(&marked.scan_fold).expect("the answer's fold"),
        fold_member
    );
    for step in [1, 6] {
        assert_eq!(
            step_of(&folded, step),
            step_of(&before, step),
            "step {step}"
        );
    }

    let status = bench.status();
    let numbered = status["steps"]
        .as_array()
        .expect("status steps")
        .iter()
        .map(|item| (item["index"].clone(), item["artifact_step"].clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        numbered,
        [
            (json!(1), json!(1)),
            (json!(2), Value::Null),
            (json!(3), Value::Null),
            (json!(4), Value::Null),
            (json!(5), json!(2)),
            (json!(6), json!(3))
        ],
        "{status}"
    );
    for step in [2, 3, 4] {
        let item = row(&status["steps"], "index", step);
        assert_eq!(item["folded_into"], 5, "{item}");
        assert!(item.get("scan_fold").is_none(), "{item}");
    }
    let item = row(&status["steps"], "index", 5);
    assert_eq!(item["scan_fold"], fold_member, "{item}");
    assert!(item.get("folded_into").is_none(), "{item}");
    for step in [1, 6] {
        let item = row(&status["steps"], "index", step);
        assert!(item.get("folded_into").is_none(), "{item}");
        assert!(item.get("scan_fold").is_none(), "{item}");
    }

    let review = bench.review(None).expect("record review");
    assert_eq!(review["steps_total"], 3, "{review}");
    assert_eq!(reviewed_steps(&review), [json!(1), json!(5), json!(6)]);
    for step in [1, 6] {
        assert!(
            row(&review["steps"], "step", step).get("folded").is_none(),
            "{review}"
        );
    }
    let sheet = row(&review["steps"], "step", 5);
    assert_eq!(sheet["artifact_step"], 2, "{sheet}");
    let swipes = sheet["folded"].as_array().expect("the folded swipes");
    assert_eq!(swipes.len(), 3, "{sheet}");
    for ((entry, step), (shown_before, shown_after)) in
        swipes.iter().zip([2, 3, 4]).zip([(0, 1), (1, 2), (2, 3)])
    {
        let execution = &step_of(&folded, step)["click"]["execution"];
        assert_eq!(entry["step"], step, "{entry}");
        assert_eq!(entry["direction"], "forward", "{entry}");
        assert_eq!(
            entry["before"]["frame_id"], execution["before_frame"],
            "{entry}"
        );
        assert_eq!(entry["before"]["preview"], Value::Null, "{entry}");
        assert_eq!(
            entry["after"]["frame_id"], execution["after_frame"],
            "{entry}"
        );
        assert_eq!(entry["after"]["step"], step + 1, "{entry}");
        assert_eq!(entry["press"]["source"], "swipe", "{entry}");
        assert_eq!(entry["press"]["gesture"]["kind"], "drag", "{entry}");
        assert_eq!(
            entry["press"]["point"],
            json!({"x": 80, "y": 70}),
            "{entry}"
        );
        assert_eq!(
            entry["press"]["to_point"],
            json!({"x": 80, "y": 30}),
            "{entry}"
        );
        let shown = |side: &str| {
            fs::read(entry[side]["path"].as_str().expect("a frame's path")).expect("the frame")
        };
        assert_eq!(shown("before"), list(shown_before), "{entry}");
        assert_eq!(shown("after"), list(shown_after), "{entry}");
    }

    let details = scan_refusal(
        bench.review(Some(3)).expect_err("a folded swipe"),
        "step_in_fold",
    );
    assert_eq!(details["step"], 3, "{details}");
    assert_eq!(details["fold_step"], 5, "{details}");
    let frames = bench.frames(3).expect("record frames of a folded swipe");
    assert_eq!(frames["step"], 3, "{frames}");
    assert_eq!(frames["artifact_step"], Value::Null, "{frames}");
    assert_eq!(frames["kind"], "swipe", "{frames}");
    assert_eq!(
        frames["press"]["frame"]["frame_id"],
        step_of(&folded, 3)["click"]["execution"]["before_frame"],
        "{frames}"
    );
}

// The work order's test 2: forward, forward, back and the tap (the first kind's demonstration: to
// the end and back) fold, the third swipe `back`; forward, back, forward is `move_after_back` on
// the third swipe, nothing written.
#[test]
fn s28_forward_and_back_is_folded_and_forward_after_back_is_refused() {
    let bench = scrolled(&[FORWARD, FORWARD, BACK], &[0, 1, 2, 1]);
    bench.marked(fold(5, 2));
    let recorded = bench.recording();
    assert_eq!(
        step_of(&recorded, 5)["scan_fold"]["moves"],
        json!([
            {"step": 2, "direction": "forward"},
            {"step": 3, "direction": "forward"},
            {"step": 4, "direction": "back"}
        ]),
        "{recorded}"
    );
    assert_eq!(step_of(&recorded, 5)["scan_fold"]["axis"], "vertical");

    let other = scrolled(&[FORWARD, BACK, FORWARD], &[0, 1, 0, 1]);
    let details = other.refused(fold(5, 2), "move_after_back");
    assert_eq!(details["step"], 5, "{details}");
    assert_eq!(details["first"], 2, "{details}");
    assert_eq!(details["at"], 4, "{details}");
}

// The work order's test 3: every cause of `fold_range_invalid` this build can meet, and every loss
// a folded step would suffer, each refused with nothing written: the first step not below the
// press; a step that does not exist; a press that is a swipe; a step between that is a tap; a step
// folded or holding a fold already; a dropped step and a step converted into a transition in the
// range (each on a recording of its own). Then marks of its own, `optional`, a transition, a choice
// (written by hand: a swipe takes none) and a page name on a swipe of the range. A request that
// misses `first`, or gives it to another step action, is `validation_failed`.
#[test]
fn s28_each_cause_of_a_range_and_of_a_loss_is_refused_and_nothing_is_written() {
    let bench = three_forward();
    for (value, at) in [
        (fold(5, 5), Value::Null),
        (fold(9, 2), json!(9)),
        (fold(4, 2), json!(4)),
        (fold(5, 1), json!(1)),
    ] {
        let details = bench.refused(value.clone(), "fold_range_invalid");
        assert_eq!(details["at"], at, "{value}: {details}");
    }
    bench.invalid(json!({"step_action": {"kind": "fold_scan", "step": 5}}));
    bench.invalid(json!({"step_action": {"kind": "drop_step", "step": 3, "first": 2}}));

    bench.marked(json!({"step": 3, "add": [color("list/moved", SQUARE)]}));
    assert_eq!(bench.refused(fold(5, 2), "would_lose_marks")["at"], 3);
    bench.marked(json!({"step": 3, "remove": ["list/moved"]}));
    bench.marked(json!({"step": 3, "optional": true}));
    assert_eq!(bench.refused(fold(5, 2), "would_lose_optional")["at"], 3);
    bench.marked(json!({"step": 3, "optional": false}));
    bench
        .marked(json!({"step": 3, "transition": {"kind": "window", "min_ms": 100, "max_ms": 500}}));
    assert_eq!(bench.refused(fold(5, 2), "would_lose_transition")["at"], 3);
    bench.marked(json!({"step": 3, "transition": {"kind": "none"}}));
    let original = bench.edit_step(3, "choice", json!({"marked_at_unix_ms": 1}));
    assert_eq!(bench.refused(fold(5, 2), "would_lose_choice")["at"], 3);
    fs::write(recording_path(&bench.state(), RECORD_ID), original).expect("the step table");
    bench.marked(json!({"step": 3, "page": "list_moved"}));
    assert_eq!(bench.refused(fold(5, 2), "would_lose_page")["at"], 3);

    let twice = three_forward();
    twice.marked(fold(5, 3));
    assert_eq!(twice.refused(fold(5, 2), "fold_range_invalid")["at"], 3);
    assert_eq!(twice.refused(fold(6, 5), "fold_range_invalid")["at"], 5);

    // A swipe that moved nothing, dropped (#463 S2-7): steps 2 to 4 the swipes, 3 the dropped one.
    let dropped = Bench::new();
    dropped.tapped(START_POINT, [start(), list(0)]);
    dropped.swiped(FORWARD, [list(0), list(1)]);
    dropped.swiped(FORWARD, [list(1), list(1)]);
    dropped.swiped(FORWARD, [list(1), list(2)]);
    dropped.tapped(ROW_POINT, [list(2), detail()]);
    assert_eq!(
        dropped.marked(action("drop_step", 3)).status,
        "step_dropped"
    );
    assert_eq!(dropped.refused(fold(5, 2), "fold_range_invalid")["at"], 3);

    // A loading screen after the first swipe converted into its transition: step 3 converted, the
    // next swipe step 4.
    let converted = Bench::new();
    converted.tapped(START_POINT, [start(), list(0)]);
    converted.swiped(FORWARD, [list(0), loading()]);
    converted.marked(json!({"step": 3, "add": [color("load/square", SQUARE)]}));
    assert_eq!(
        converted.marked(action("to_transition", 3)).status,
        "step_converted_to_transition"
    );
    converted.swiped(FORWARD, [list(1), list(2)]);
    converted.tapped(ROW_POINT, [list(2), detail()]);
    assert_eq!(converted.refused(fold(5, 2), "fold_range_invalid")["at"], 3);
}

// The work order's test 4: swipes of different lengths along one axis in one direction fold (40
// px, 20 px, and 70 px with 2 px across); a diagonal swipe, and a swipe across after one along, are
// `move_axis_unclear` on that swipe, naming its two points, nothing written.
#[test]
fn s28_swipes_of_different_lengths_fold_and_an_unclear_axis_is_refused() {
    let bench = scrolled(&[FORWARD, SHORT_FORWARD, LONG_FORWARD], &[0, 1, 2, 3]);
    bench.marked(fold(5, 2));
    let recorded = bench.recording();
    assert_eq!(step_of(&recorded, 5)["scan_fold"]["axis"], "vertical");
    assert_eq!(
        step_of(&recorded, 5)["scan_fold"]["moves"],
        json!([
            {"step": 2, "direction": "forward"},
            {"step": 3, "direction": "forward"},
            {"step": 4, "direction": "forward"}
        ]),
        "{recorded}"
    );

    for (swipe, from, to) in [
        (
            DIAGONAL,
            json!({"x": 80, "y": 70}),
            json!({"x": 110, "y": 40}),
        ),
        (
            ACROSS,
            json!({"x": 120, "y": 45}),
            json!({"x": 40, "y": 45}),
        ),
    ] {
        let other = scrolled(&[FORWARD, swipe, FORWARD], &[0, 1, 2, 3]);
        let details = other.refused(fold(5, 2), "move_axis_unclear");
        assert_eq!(details["at"], 3, "{details}");
        assert_eq!(details["from"], from, "{details}");
        assert_eq!(details["to"], to, "{details}");
    }
}

// The work order's test 5: `unfold_scan` gives the step table back byte for byte (but for the
// time it was last written, which every write sets), and `record stop` then writes the package a
// recording made the same way and never folded writes, but for the generation time. `unfold_scan`
// of a folded swipe, and of a step of no fold, is `not_folded`, nothing written.
#[test]
fn s28_unfold_gives_the_step_table_back_and_the_package_is_the_one_without_the_fold() {
    let plain = paged();
    let bench = paged();
    let before = bench.recording_bytes();
    bench.marked(fold(5, 2));
    assert_ne!(
        but_update_time(&bench.recording_bytes()),
        but_update_time(&before)
    );
    let details = bench.refused(action("unfold_scan", 4), "not_folded");
    assert_eq!(details["folded_into"], 5, "{details}");
    let details = bench.refused(action("unfold_scan", 1), "not_folded");
    assert_eq!(details["folded_into"], Value::Null, "{details}");
    let unfolded = bench.marked(action("unfold_scan", 5));
    assert_eq!(unfolded.status, "scan_unfolded");
    assert_eq!(unfolded.step, Some(5));
    assert_eq!(unfolded.scan_fold, None);
    assert_eq!(
        but_update_time(&bench.recording_bytes()),
        but_update_time(&before)
    );

    let generated = bench.stopped();
    let expected = plain.stopped();
    assert_eq!(generated["lab_status"], "generated", "{generated}");
    assert_eq!(expected["lab_status"], "generated", "{expected}");
    assert_eq!(
        without_generation_time(package(&generated)),
        without_generation_time(package(&expected))
    );
}

// The work order's test 6: a recording whose every pre-check but the fold passes. `record ready`
// lists the press step with `scan_without_select` (code `record_scan_invalid`), its one item, and
// runs no self-check; `record stop` and its dry run refuse with `scan_not_selected`, the item's
// count and the hint, and nothing is written: every file of the recording and the session header
// stay as they were, the recording active.
#[test]
fn s28_a_fold_without_a_scrolling_select_is_listed_and_refuses_record_stop() {
    let bench = paged();
    bench.marked(fold(5, 2));
    let recorded = bench.recording();
    let ready = bench.ready();
    assert_eq!(ready["status"], "draft", "{ready}");
    assert_eq!(ready["missing_total"], 1, "{ready}");
    let item = &ready["missing"][0];
    assert_eq!(item["what"], "scan_without_select", "{item}");
    assert_eq!(item["code"], LabCode::RecordScanInvalid.as_str(), "{item}");
    assert_eq!(item["step"], 5, "{item}");
    assert_eq!(item["artifact_step"], 2, "{item}");
    assert_eq!(
        item["frame_id"],
        step_of(&recorded, 5)["click"]["execution"]["before_frame"],
        "{item}"
    );
    assert_eq!(ready["checks"], json!([]), "{ready}");
    assert_eq!(ready["checks_after"], json!(EVERY_CHECK), "{ready}");

    let header = old_record_path(&bench.state(), INSTANCE);
    let header_before = fs::read(&header).expect("session header");
    let files_before = bench.files();
    for dry_run in [false, true] {
        let details = scan_refusal(
            bench.stop(dry_run).expect_err("a fold without a select"),
            "scan_not_selected",
        );
        assert_eq!(details["step"], 5, "{details}");
        assert_eq!(details["first"], 2, "{details}");
        assert_eq!(details["missing_total"], 1, "{details}");
        assert_eq!(details["hint"], "record ready", "{details}");
        assert_eq!(
            bench.files(),
            files_before,
            "dry run {dry_run}: a file changed"
        );
        assert_eq!(
            fs::read(&header).expect("session header"),
            header_before,
            "dry run {dry_run}"
        );
    }
    assert_eq!(bench.recording()["status"], "active");
}

// The work order's test 7: a folded swipe has no page and no case. On a recording whose steps carry
// no mark, `record ready` lists each swipe's missing page (`page_recognition`) before the fold,
// none of a folded swipe after it (the press step its own items and `scan_without_select`, number 2
// in the package), and after `unfold_scan` the same answer as before the fold. On a recording whose
// pre-checks pass but the fold's, no self-check runs while it is folded, so no case is made; folded
// and unfolded again, `record stop` writes the cases a recording made the same way and never folded
// writes, byte for byte, one per step 1 to 5 and the final screen.
#[test]
fn s28_a_folded_swipe_has_no_page_and_no_case_and_unfolded_the_cases_are_as_before() {
    let bench = three_forward();
    let ready_before = bench.ready();
    for step in [2, 3, 4] {
        assert_eq!(
            items_of(&ready_before, step),
            [json!("page_recognition")],
            "{ready_before}"
        );
    }
    bench.marked(fold(5, 2));
    let ready = bench.ready();
    for step in [2, 3, 4] {
        assert_eq!(items_of(&ready, step), Vec::<Value>::new(), "{ready}");
    }
    assert_eq!(
        items_of(&ready, 5),
        [json!("page_recognition"), json!("scan_without_select")],
        "{ready}"
    );
    assert!(
        ready["missing"]
            .as_array()
            .expect("missing")
            .iter()
            .filter(|item| item["step"] == 5)
            .all(|item| item["artifact_step"] == 2),
        "{ready}"
    );
    bench.marked(action("unfold_scan", 5));
    assert_eq!(bench.ready(), ready_before);

    let plain = paged();
    let folded = paged();
    folded.marked(fold(5, 2));
    let ready = folded.ready();
    assert_eq!(ready["checks"], json!([]), "{ready}");
    assert_eq!(ready["checks_after"], json!(EVERY_CHECK), "{ready}");
    folded.marked(action("unfold_scan", 5));
    let generated = folded.stopped();
    let expected = plain.stopped();
    assert_eq!(
        generated["cases"]["ids"],
        json!([
            "step_01", "step_02", "step_03", "step_04", "step_05", "final"
        ]),
        "{generated}"
    );
    assert_eq!(generated["cases"]["ids"], expected["cases"]["ids"]);
    let cases = |lab: &Value| {
        files_under(Path::new(
            lab["cases"]["directory"]
                .as_str()
                .unwrap_or_else(|| panic!("no cases directory in {lab}")),
        ))
    };
    assert_eq!(cases(&generated), cases(&expected));
}

// The work order's test 8: on a recording folded from step 2 into step 5, every step action on a
// folded swipe (`drop_step`, `restore_step`, `reopen_step`, `close_step` naming it,
// `to_transition`) and on the press step that holds the fold (`drop_step`), and marks and a
// transition on a folded swipe, are `step_in_fold` naming the fold's press step, nothing written.
#[test]
fn s28_each_step_action_on_a_step_of_a_fold_is_refused() {
    let bench = three_forward();
    bench.marked(fold(5, 2));
    for (value, step) in [
        (action("drop_step", 3), 3),
        (action("restore_step", 3), 3),
        (action("reopen_step", 3), 3),
        (action("close_step", 3), 3),
        (action("to_transition", 3), 3),
        (action("drop_step", 5), 5),
        (json!({"step": 3, "add": [color("list/moved", SQUARE)]}), 3),
        (
            json!({"step": 3, "transition": {"kind": "window", "min_ms": 100, "max_ms": 500}}),
            3,
        ),
    ] {
        let details = bench.refused(value.clone(), "step_in_fold");
        assert_eq!(details["step"], step, "{value}: {details}");
        assert_eq!(details["fold_step"], 5, "{value}: {details}");
    }
}

// Fix round 1 (the coordinator's ruling on the review's answer 1): a stopped recording takes a fold
// and its undoing as an amendment, as it takes a mark. Folded after its package was generated, it
// stays stopped and is marked again (`amended_at_unix_ms`; the answer `after_stop`); `record ready`
// lists `scan_without_select`, its one item; `record stop` refuses `scan_not_selected`, nothing
// written. Unfolded, `record stop` generates the package again (`regenerated`, its package id), the
// one it generated before the fold but for its generation time, and clears the mark.
#[test]
fn s28_fix1_a_stopped_recording_folds_as_an_amendment_and_unfolded_generates_again() {
    let bench = paged();
    let generated = bench.stopped_session();
    let package_before = without_generation_time(package(&generated));
    let stopped = bench.recording();
    assert_eq!(stopped["status"], "stopped", "{stopped}");
    assert!(stopped.get("amended_at_unix_ms").is_none(), "{stopped}");

    let folded = bench.marked(fold(5, 2));
    assert_eq!(folded.status, "scan_folded");
    assert!(folded.after_stop);
    let amended = bench.recording();
    assert_eq!(amended["status"], "stopped", "{amended}");
    assert!(amended["amended_at_unix_ms"].is_u64(), "{amended}");
    assert_eq!(step_of(&amended, 5)["scan_fold"]["first"], 2, "{amended}");
    for step in [2, 3, 4] {
        assert_eq!(step_of(&amended, step)["folded_into"], 5, "{amended}");
    }

    let ready = bench.ready();
    assert_eq!(ready["session"], "stopped", "{ready}");
    assert_eq!(ready["status"], "draft", "{ready}");
    assert_eq!(ready["missing_total"], 1, "{ready}");
    assert_eq!(
        ready["missing"][0]["what"], "scan_without_select",
        "{ready}"
    );
    assert_eq!(ready["missing"][0]["step"], 5, "{ready}");

    let files_before = bench.files();
    let details = scan_refusal(
        bench.stop(false).expect_err("a fold without a select"),
        "scan_not_selected",
    );
    assert_eq!(details["step"], 5, "{details}");
    assert_eq!(details["missing_total"], 1, "{details}");
    assert_eq!(bench.files(), files_before, "a refused stop writes nothing");

    let unfolded = bench.marked(action("unfold_scan", 5));
    assert_eq!(unfolded.status, "scan_unfolded");
    assert!(unfolded.after_stop);
    assert!(bench.recording()["amended_at_unix_ms"].is_u64());
    let regenerated = bench.stopped();
    assert_eq!(regenerated["lab_status"], "regenerated", "{regenerated}");
    assert_eq!(regenerated["package_id"], generated["package_id"]);
    assert_eq!(
        without_generation_time(package(&regenerated)),
        package_before
    );
    let after = bench.recording();
    assert!(after.get("amended_at_unix_ms").is_none(), "{after}");
}

// Fix round 1: on a stopped recording a fold meets what only a stopped recording holds. An optional
// step inserted after the stop after the second swipe, step 3 (it takes the serial number 7):
// folding the swipes is `would_lose_inserted` on step 3, naming step 7; a fold whose press step is
// that inserted step is `inserted_in_range` on it; nothing is written either time. Given up
// (`remove_inserted`), the inserted step holds nothing back and the swipes fold.
#[test]
fn s28_fix1_a_fold_after_the_stop_is_refused_what_only_a_stopped_recording_holds() {
    let bench = paged();
    bench.stopped_session();
    let inserted = bench.marked(json!({"optional_after": {
        "step": 3,
        "frame": bench.caller_frame("caller_notice", &notice()),
        "page": "notice",
        "add": [color("popup/notice", SQUARE)],
        "click": {"region": NOTICE_CLICK},
        "settle_ms": 1000
    }}));
    assert_eq!(inserted.step, Some(7));
    assert_eq!(step_of(&bench.recording(), 7)["inserted_after"], 3);

    let details = bench.refused(fold(5, 2), "would_lose_inserted");
    assert_eq!(details["step"], 5, "{details}");
    assert_eq!(details["at"], 3, "{details}");
    assert_eq!(details["inserted"], 7, "{details}");
    let details = bench.refused(fold(7, 2), "inserted_in_range");
    assert_eq!(details["step"], 7, "{details}");
    assert_eq!(details["at"], 7, "{details}");
    assert_eq!(details["inserted_after"], 3, "{details}");

    bench.marked(json!({"remove_inserted": 7}));
    assert_eq!(bench.marked(fold(5, 2)).status, "scan_folded");
}

// Fix round 1: every other step operation stays refused on a stopped recording, as before
// (`record_session_not_active`, reason `recording_stopped`), before and after a fold made after the
// stop, on a step of no fold and on a folded swipe; nothing is written.
#[test]
fn s28_fix1_every_other_step_operation_stays_refused_on_a_stopped_recording() {
    let bench = paged();
    bench.stopped_session();
    for folded in [false, true] {
        if folded {
            bench.marked(fold(5, 2));
        }
        let before = bench.files();
        for value in [
            action("drop_step", 6),
            action("drop_step", 3),
            action("restore_step", 3),
            action("reopen_step", 6),
            json!({"step_action": {"kind": "close_step"}}),
            action("to_transition", 3),
        ] {
            let error = bench
                .mark(value.clone())
                .expect_err("a step operation on a stopped recording");
            assert_eq!(
                error.code, "record_session_not_active",
                "{value}: {}",
                error.message
            );
            assert_eq!(
                error.details.expect("details")["reason"],
                "recording_stopped",
                "{value}"
            );
        }
        assert_eq!(bench.files(), before, "folded {folded}: a file changed");
    }
}

// Fix round 1, the review's (h): the rule of one axis at its edges and along the other axis. A
// swipe 40 px up and 20 px across, its move along the axis exactly twice its move across it, runs
// along the vertical axis and folds; 40 px up and 21 px across, and a swipe that ends where it
// began, are `move_axis_unclear` with their two points, nothing written. Two swipes to the left and
// one back to the right fold along the horizontal axis, the last `back`.
#[test]
fn s28_fix1_the_axis_rule_at_its_edges_and_a_fold_along_the_horizontal_axis() {
    let forward = json!([
        {"step": 2, "direction": "forward"},
        {"step": 3, "direction": "forward"},
        {"step": 4, "direction": "forward"}
    ]);
    let twice = scrolled(&[FORWARD, TWICE, FORWARD], &[0, 1, 2, 3]);
    twice.marked(fold(5, 2));
    let recorded = twice.recording();
    assert_eq!(
        step_of(&recorded, 5)["scan_fold"]["axis"],
        "vertical",
        "{recorded}"
    );
    assert_eq!(
        step_of(&recorded, 5)["scan_fold"]["moves"],
        forward,
        "{recorded}"
    );

    for (swipe, from, to) in [
        (
            NEARLY_TWICE,
            json!({"x": 80, "y": 70}),
            json!({"x": 101, "y": 30}),
        ),
        (STILL, json!({"x": 80, "y": 50}), json!({"x": 80, "y": 50})),
    ] {
        let other = scrolled(&[FORWARD, swipe, FORWARD], &[0, 1, 2, 3]);
        let details = other.refused(fold(5, 2), "move_axis_unclear");
        assert_eq!(details["at"], 3, "{details}");
        assert_eq!(details["from"], from, "{details}");
        assert_eq!(details["to"], to, "{details}");
    }

    let across = scrolled(&[ACROSS, ACROSS, ACROSS_BACK], &[0, 1, 2, 1]);
    across.marked(fold(5, 2));
    let recorded = across.recording();
    let mut written = step_of(&recorded, 5)["scan_fold"].clone();
    written
        .as_object_mut()
        .expect("the fold")
        .remove("folded_at_unix_ms");
    assert_eq!(
        written,
        json!({"first": 2, "axis": "horizontal", "moves": [
            {"step": 2, "direction": "forward"},
            {"step": 3, "direction": "forward"},
            {"step": 4, "direction": "back"}
        ]}),
        "{recorded}"
    );
}

// Fix round 1, the review's (h): the archive's material keeps a folded recording's swipes as
// recorded. Three swipes folded into the press of step 5, then archived: the manifest's material
// names the press of every step 1 to 5 (each swipe with the frame it was sent on and where it began
// and ended) and the final screen, step 6's; its rows are the six steps; the step table keeps its
// bytes, the fold in it.
#[test]
fn s28_fix1_the_archive_keeps_the_folded_swipes_in_its_material() {
    let bench = three_forward();
    bench.marked(fold(5, 2));
    let table = bench.recording_bytes();
    let folded = bench.recording();
    let archived = record_archive(
        &bench.lock,
        &RecordArchiveOptions {
            written_by: "gate_463_s28".to_owned(),
            ..RecordArchiveOptions::default()
        },
    )
    .unwrap_or_else(|error| panic!("record archive: {}: {:?}", error.code, error.details));
    assert_eq!(archived.status, "archived");
    let manifest: Value =
        serde_json::from_slice(&fs::read(Path::new(&archived.manifest.path)).expect("manifest"))
            .expect("manifest JSON");
    let material = &manifest["material"];
    let pressed = material["clicks"]
        .as_array()
        .expect("the material's presses")
        .iter()
        .map(|click| (click["step"].clone(), click["kind"].clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        pressed,
        [
            (json!(1), json!("tap")),
            (json!(2), json!("swipe")),
            (json!(3), json!("swipe")),
            (json!(4), json!("swipe")),
            (json!(5), json!("tap"))
        ],
        "{material}"
    );
    for step in [2, 3, 4] {
        let click = row(&material["clicks"], "step", step);
        assert_eq!(
            click["frame"]["frame_id"],
            step_of(&folded, step)["click"]["execution"]["before_frame"],
            "{click}"
        );
        assert_eq!(click["point"], json!({"x": 80, "y": 70}), "{click}");
        assert_eq!(click["to"], json!({"x": 80, "y": 30}), "{click}");
    }
    assert_eq!(material["final_frame"]["step"], 6, "{material}");
    assert_eq!(
        manifest["steps"].as_array().map(Vec::len),
        Some(6),
        "{manifest}"
    );
    assert_eq!(
        row(&manifest["steps"], "step", 3)["kind"],
        "swipe",
        "{manifest}"
    );
    assert_eq!(
        bench.recording_bytes(),
        table,
        "the step table keeps its bytes"
    );
}
