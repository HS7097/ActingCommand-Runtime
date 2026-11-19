// SPDX-License-Identifier: AGPL-3.0-only

// #463 PRIV-SCAN (contracts/lab-recording.md, "What a public recording records of a select";
// register §1.492; the owner's rule of 11-18 that whatever goes to GitHub is public, a private
// repository too, and that what an account holds does not enter a repository; the rules below are
// the coordinator's rulings of 11-19): a public recording's provenance records each select entry as
// a new object of named members only, never the answers' entry with members taken out; a local
// recording's records the answers' entries as before; the answers are not narrowed. Recordings are
// made as S2-9a's gate makes them (taps and swipes through `record_press` with a fake Runtime, its
// captures taking the scripted screens and the caller's clock moving only by its waits), on S2-9a's
// synthetic list: 128x128 screens, a page square of one colour at (0, 0, 16, 16) and, on the list
// screens, a list area at rows 20 to 99 that shows the content rows `offset` to `offset + 79`;
// eleven content rows 12 px apart (row k at content row 4 + 12k), each an anchor of noise (8x6 at
// x 4) and a name marker pixel at (20, top + 2) whose grey value the stand-in name reader reads as
// the row's name; a start mark of noise at content rows 0 to 3 and an end mark of other noise at
// content rows 136 to 141, both at x 60 to 75. Two demonstrations: S2-9a's "three forward, two
// back, press" (the press on `delta`, row 3, on the screen at offset 24), read to the list's end,
// and S2-9b's "two forward, press" (the press on `india`, row 8, on the screen at offset 48),
// straight to the wanted item. No Runtime, no device, no OCR model but the stand-in reader, no
// frame or name of a game; the tests read no clock and never sleep. Every expected number comes
// from the geometry above, as S2-9a's gate has it: the offline entry reads 11 positions over 4
// forward screens of 6 screens in all, 3 forward swipes of 24 px, `delta` at position 3, and
// `lima`, which no row shows, empty over complete coverage.
// - `privscan_a_public_package_holds_none_of_the_left_out_members` (1)
// - `privscan_a_local_package_records_the_answers_entries_as_before` (2)
// - `privscan_adopt_its_check_and_regression_pass_the_public_package` (3)
// - `privscan_the_self_check_refuses_the_same_case_public_and_local` (4)
// - `privscan_a_public_record_holds_no_member_off_the_named_list` (5)

use crate::recording::model::RecordDestination;
use crate::recording::store::{lab_dir, old_record_path};
use crate::regression_case::run_regression_case;
use crate::{
    AfterCapture, AttachFrameRequest, CapturedFrame, ClickEffect, ClickPlan, CommitClickRequest,
    LAB_RECORD_MARK_SCHEMA, LabOcr, MarkRequest, PlanClickRequest, PlannedSwipe, PointPress,
    RecordFramePorts, RecordPoint, RecordPressRequest, RecordRect, RecordStartOptions,
    RecordStopOptions, RecordingLock, record_mark, record_press, record_start, record_stop,
};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{InputFrameReference, LabError, LabResult, canonical_json_bytes};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_pack_containment::{
    ContainmentLimits, ContentContainer, expand_content_container,
};
use actingcommand_recognition_pack::{
    NnProviderRequest, NnProviderResult, OcrProviderRequest, OcrProviderResult,
    OcrProviderTextBlock, PackRect, VisionProvider, VisionProviderError, VisionProviderErrorCode,
};
use actingcommand_resource_tooling::{
    AdoptCaseAt, AdoptCasesRun, AdoptDecision, AdoptLab, AdoptSource, AdoptWorktree, ReadyRefused,
    RecordingReady, StandardPackAdoptRequest, standard_pack_adopt,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_0463_privscan";
const TASK_ID: &str = "neutral_task";
const STARTED_AT: u64 = 1_700_000_000_000;
const WIDTH: u32 = 128;
const HEIGHT: u32 = 128;
const GRAY: [u8; 3] = [60, 60, 60];
const RED: [u8; 3] = [230, 20, 20];
const GREEN: [u8; 3] = [20, 230, 20];
const YELLOW: [u8; 3] = [230, 230, 20];
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
/// The members of the answers' entry a public record leaves out, by their names: the names each
/// screen showed, the positions the scan read, the content step of each swipe, the pressed name's
/// position in the list.
const LEFT_OUT: [&str; 4] = ["screens", "positions", "content_step_px", "position"];
/// The time the adoption gives itself, which names its kept places.
const ADOPTED_AT: u64 = 1_000;

const fn rect(x: i32, y: i32, width: i32, height: i32) -> RecordRect {
    RecordRect {
        x,
        y,
        width,
        height,
    }
}

/// Each screen's page square: red on the start screen, green on the list, yellow on the detail.
const SQUARE: RecordRect = rect(0, 0, 16, 16);
/// The list's area: the frame's rows 20 to 99.
const LIST_TOP: u32 = 20;
const LIST_HEIGHT: u32 = 80;
const LIST: RecordRect = rect(0, 20, 128, 80);
/// The rows of the content: eleven, 12 px apart, row 0 at content row 4.
const ROWS: u32 = 11;
const PITCH: u32 = 12;
const FIRST_ROW: u32 = 4;
/// The names the list may show, the eleven of its rows and `lima`, which no row shows.
const NAMES: [&str; 12] = [
    "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india", "juliet",
    "kilo", "lima",
];
const ALIASES: [&str; 12] = [
    "ALPHA", "BRAVO", "CHARLIE", "DELTA", "ECHO", "FOXTROT", "GOLF", "HOTEL", "INDIA", "JULIET",
    "KILO", "LIMA",
];
/// The start and end marks: noise at x 60 to 75, content rows 0 to 3 and 136 to 141.
const MARK_X: u32 = 60;
const MARK_WIDTH: u32 = 16;
const START_ROWS: u32 = 4;
const END_TOP: u32 = 136;
const END_ROWS: u32 = 6;

/// The agent's regions on the list's first screen (offset 0): row 0 at frame row 24.
const ROW: RecordRect = rect(2, 24, 40, 8);
const ANCHOR: RecordRect = rect(4, 24, 8, 6);
const NAME: RecordRect = rect(18, 24, 16, 6);
const START_MARK: RecordRect = rect(60, 20, 16, 4);
/// The end mark on the last screen after a forward swipe (offset 72): content row 136 at 84.
const END_MARK: RecordRect = rect(60, 84, 16, 6);

/// The tap on the start screen; the press on the row `delta` (row 3, frame rows 36 to 43 on the
/// screen at offset 24); the press on the row `india` (row 8, frame rows 72 to 79 on the screen at
/// offset 48).
const START_POINT: (i32, i32) = (100, 110);
const ROW_POINT: (i32, i32) = (10, 40);
const WANTED_POINT: (i32, i32) = (10, 76);

/// A swipe by its two points: up the list by 24 px, and back down by as much.
type Swipe = ((i32, i32), (i32, i32));
const FORWARD: Swipe = ((64, 80), (64, 56));
const BACK: Swipe = ((64, 56), (64, 80));

// Deterministic pixel noise: a shifted copy of it correlates with it near zero.
fn noise(x: u32, y: u32) -> [u8; 3] {
    let mut state = u64::from(y * 256 + x).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x2545_f491;
    let mut next = || {
        state ^= state >> 33;
        state = state.wrapping_mul(0xff51_afd7_ed55_8ccd);
        state ^= state >> 33;
        (state & 0xff) as u8
    };
    [next(), next(), next()]
}

// Whether `area` covers the pixel at `(x, y)`.
fn covers(area: RecordRect, x: u32, y: u32) -> bool {
    let (column, row) = (i64::from(x), i64::from(y));
    column >= i64::from(area.x)
        && row >= i64::from(area.y)
        && column < i64::from(area.x) + i64::from(area.width)
        && row < i64::from(area.y) + i64::from(area.height)
}

// The PNG of a screen whose pixels `pixel` gives.
fn image(pixel: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            pixels.extend(pixel(x, y));
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

// A gray screen with its page square in `color`.
fn squared(color: [u8; 3]) -> Vec<u8> {
    image(|x, y| if covers(SQUARE, x, y) { color } else { GRAY })
}

fn start() -> Vec<u8> {
    squared(RED)
}

fn detail() -> Vec<u8> {
    squared(YELLOW)
}

// The list scrolled by `offset` px.
fn list_at(offset: u32) -> Vec<u8> {
    image(|x, y| {
        if covers(SQUARE, x, y) {
            return GREEN;
        }
        if !(LIST_TOP..LIST_TOP + LIST_HEIGHT).contains(&y) {
            return GRAY;
        }
        let row = y - LIST_TOP + offset;
        if (MARK_X..MARK_X + MARK_WIDTH).contains(&x) {
            if row < START_ROWS {
                return noise(x - MARK_X + 100, row + 100);
            }
            if (END_TOP..END_TOP + END_ROWS).contains(&row) {
                return noise(x - MARK_X + 150, row - END_TOP + 150);
            }
        }
        for k in 0..ROWS {
            let top = FIRST_ROW + PITCH * k;
            if (top..top + 6).contains(&row) && (4..12).contains(&x) {
                return noise(x - 4, row - top);
            }
            if row == top + 2 && x == 20 {
                return [200 + u8::try_from(k).expect("a row"); 3];
            }
        }
        GRAY
    })
}

// Reads one block per name marker pixel inside the requested region, its text the alias of the
// name its grey value stands for, at confidence 0.9; a region without one reads nothing.
#[derive(Debug)]
struct RowNames;

impl VisionProvider for RowNames {
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
                let Some(alias) = request
                    .frame
                    .rgb8_pixels
                    .get(index)
                    .and_then(|red| red.checked_sub(200))
                    .and_then(|k| ALIASES.get(usize::from(k)))
                else {
                    continue;
                };
                blocks.push(OcrProviderTextBlock {
                    text: (*alias).to_owned(),
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

// The colour mark `id` over `area`.
fn color(id: &str, area: RecordRect) -> Value {
    json!({"id": id, "family": "color", "region": area})
}

fn request(mut value: Value) -> MarkRequest {
    value["schema_version"] = json!(LAB_RECORD_MARK_SCHEMA);
    serde_json::from_value(value).expect("a record mark request")
}

// The names a select over the list declares: the eleven of its rows and `lima`, each with its
// alias.
fn names() -> Vec<Value> {
    NAMES
        .iter()
        .zip(ALIASES)
        .map(|(id, alias)| json!({"id": id, "aliases": [alias]}))
        .collect()
}

// The scrolling select of the first kind over the list, the wanted value's default `delta`, the
// row the demonstration pressed.
fn scrolling() -> Value {
    json!({
        "rows": {"row": ROW, "anchor": ANCHOR, "name": NAME},
        "scan": {"list": LIST, "start": START_MARK, "end": END_MARK, "stop": "end"},
        "read": {"languages": ["en"], "timeout_ms": 1000, "minimum_confidence_milli": 800,
                 "model_ref": "neutral-ocr", "model_sha256": MODEL_SHA256},
        "names": names(),
        "match": {"max_distance": 1, "minimum_margin": 1},
        "wanted": {"name": "list_item", "default": "delta"}
    })
}

// The scrolling select of the second kind over the list, straight to the wanted value's default
// `india`, without an end mark.
fn straight() -> Value {
    json!({
        "rows": {"row": ROW, "anchor": ANCHOR, "name": NAME},
        "scan": {"list": LIST, "start": START_MARK, "stop": "wanted"},
        "read": {"languages": ["en"], "timeout_ms": 1000, "minimum_confidence_milli": 800,
                 "model_ref": "neutral-ocr", "model_sha256": MODEL_SHA256},
        "names": names(),
        "match": {"max_distance": 1, "minimum_margin": 1},
        "wanted": {"name": "list_item", "default": "india"}
    })
}

fn point(at: (i32, i32)) -> RecordPoint {
    RecordPoint { x: at.0, y: at.1 }
}

// What the command hands the recording for a performed input.
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

/// The fake Runtime's captures and the caller's clock, which moves only by the waits.
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

fn task_of(entries: &BTreeMap<String, Vec<u8>>) -> Value {
    serde_json::from_slice(
        entries
            .get(&format!("resources/operations/{TASK_ID}/task.json"))
            .expect("task.json"),
    )
    .expect("task JSON")
}

// The directory of the cases `record stop` wrote beside the package.
fn cases_directory(lab: &Value) -> PathBuf {
    PathBuf::from(lab["cases"]["directory"].as_str().expect("cases directory"))
}

// Whether `haystack` holds the bytes `needle`.
fn holds(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

// The members a public record names, as a tree: an object names its members, an array its items by
// its one item, `null` a value. The decisions on a frame by their kind (`{result, slot}`), or a
// frame not evaluated (`result`); of the scan, a decision by its kind and the name it chose.
fn named_list() -> Value {
    let decision = json!({"result": null, "slot": null});
    let chosen = json!({"result": null, "name": null});
    json!({
        "step": null,
        "record_index": null,
        "op": null,
        "layout": null,
        "wanted": {"name": null, "default": null},
        "pressed": null,
        "on_empty_key": null,
        "frames": [{"frame_id": null, "pressed": decision, "default": decision, "result": null}],
        "scan": {
            "coverage": null,
            "pressed": chosen,
            "absent": {"name": null, "coverage": null, "decision": chosen}
        }
    })
}

// The paths of the members of `value` that `list` does not name, each `/member/…` (an array's
// items under the array's own path), without going into them.
fn walk(value: &Value, list: &Value, at: &str, found: &mut Vec<String>) {
    match (value, list) {
        (Value::Object(members), Value::Object(named)) => {
            for (name, member) in members {
                let path = format!("{at}/{name}");
                match named.get(name) {
                    Some(sub) => walk(member, sub, &path, found),
                    None => found.push(path),
                }
            }
        }
        (Value::Array(items), Value::Array(named)) => {
            if let Some(sub) = named.first() {
                for item in items {
                    walk(item, sub, at, found);
                }
            }
        }
        (Value::Object(_) | Value::Array(_), _) => found.push(at.to_owned()),
        _ => {}
    }
}

// The members of the select entry `entry` that are not on the named list.
fn off_the_list(entry: &Value) -> Vec<String> {
    let mut found = Vec::new();
    walk(entry, &named_list(), "", &mut found);
    found
}

/// The version control of a checkout whose every change is committed; the Lab's parts it is never
/// asked for, since an adoption of a package by its path judges no recording and takes no case.
struct Committed;

impl AdoptWorktree for Committed {
    fn changes_under(&self, _repo: &Path, _relative: &str) -> Result<Vec<String>, Outcome> {
        Ok(Vec::new())
    }

    fn lab(&self) -> &dyn AdoptLab {
        &NoRecording
    }
}

/// What an adoption by a package's path never asks of the Lab: each refuses, so that a call would
/// fail the adoption rather than pass unseen.
struct NoRecording;

impl AdoptLab for NoRecording {
    fn recording_ready(
        &self,
        _state_dir: &Path,
        _record_id: &str,
    ) -> Result<RecordingReady, ReadyRefused> {
        Err(ReadyRefused::Failed(LabError::usage(
            "an adoption of a package by its path judges no recording",
        )))
    }

    fn run_cases(&self, _package_dir: &Path, _cases: &[AdoptCaseAt]) -> LabResult<AdoptCasesRun> {
        Err(LabError::usage(
            "an adoption of a package by its path takes no case",
        ))
    }
}

// Writes `value` at `path` in the canonical form.
fn write_json(path: &Path, value: &Value) {
    fs::create_dir_all(path.parent().expect("a parent")).expect("the directory");
    fs::write(path, canonical_json_bytes(value).expect("canonical JSON")).expect("the file");
}

// A made-up checkout of the game `neutral`, server `test` at `repo`: `applications.json`,
// `maintenance.json` (`[]`), an empty `packs/` and `schedule.json` without a rule.
fn checkout(repo: &Path) {
    write_json(
        &repo.join("applications.json"),
        &json!({
            "schema_version": "actingcommand.applications.v1",
            "game": "neutral",
            "servers": {"test": {"application_id": "com.example.neutral", "label": "Test"}}
        }),
    );
    write_json(&repo.join("maintenance.json"), &json!([]));
    fs::create_dir_all(repo.join("packs")).expect("the packs directory");
    write_json(
        &repo.join("schedule.json"),
        &json!({"schema_version": "actingcommand.schedule.v1", "rules": []}),
    );
}

/// A state directory with an active session of `INSTANCE` and its Lab recording, public, its game,
/// server and locale known, with the stand-in name reader. The lock is dropped before the
/// directory.
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
        let lock = RecordingLock::acquire(&state, INSTANCE, "gate_463_priv_scan", "Lab-CLI")
            .expect("recording lock");
        let bench = Self {
            lock,
            ocr: LabOcr::with_provider(Arc::new(RowNames)),
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
                destination: RecordDestination::Public,
                ..RecordStartOptions::default()
            },
        )
        .expect("record start");
        bench
    }

    fn state(&self) -> PathBuf {
        self.temp.path().join("state")
    }

    // `record_press` of `pressed`, the fake Runtime performing the input and its captures taking
    // `shots`: the screen before the press, then the screen after it.
    fn press_with(&self, pressed: &RecordPressRequest, shots: [Vec<u8>; 2]) {
        let mut device = Screens {
            shots: shots.into(),
            taken: 0,
            now: Duration::ZERO,
        };
        let mut send = |_: &ClickPlan, _: InputFrameReference| -> LabResult<CommitClickRequest> {
            Ok(receipt())
        };
        if let Err(error) = record_press(&self.lock, pressed, &mut send, &mut device) {
            panic!(
                "the press: {}: {} {:?}",
                error.code, error.message, error.details
            );
        }
    }

    // A point press recorded at `at` on the first of `shots`.
    fn press_at(&self, at: (i32, i32), shots: [Vec<u8>; 2]) {
        let pressed = RecordPressRequest {
            click: PlanClickRequest::default(),
            point_press: Some(PointPress::Tap { point: point(at) }),
            after: one_capture_after(),
        };
        self.press_with(&pressed, shots);
    }

    // A drag of the list over 300 ms recorded on the first of `shots`.
    fn drag_list(&self, (from, to): Swipe, shots: [Vec<u8>; 2]) {
        let pressed = RecordPressRequest {
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
        self.press_with(&pressed, shots);
    }

    // `record mark` of `value`, which must stand.
    fn marked(&self, value: Value) {
        if let Err(error) = record_mark(&self.lock, &request(value), false, &self.ocr) {
            panic!(
                "record mark: {}: {} {:?}",
                error.code, error.message, error.details
            );
        }
    }

    fn files(&self) -> Vec<(String, Vec<u8>)> {
        files_under(&lab_dir(&self.state(), RECORD_ID))
    }

    // `record stop [--destination <word>]`: the `lab` output with its status.
    fn stop(&self, destination: Option<RecordDestination>) -> Result<Value, LabError> {
        let options = RecordStopOptions {
            destination,
            ..RecordStopOptions::default()
        };
        record_stop(&self.lock, &options, &self.ocr).map(|outcome| {
            let mut lab = serde_json::to_value(&outcome.lab).expect("lab JSON");
            lab["lab_status"] = json!(outcome.lab_status);
            lab
        })
    }

    fn stopped(&self, destination: Option<RecordDestination>) -> Value {
        self.stop(destination).unwrap_or_else(|error| {
            panic!(
                "record stop: {}: {} {:?}",
                error.code, error.message, error.details
            )
        })
    }
}

// S2-9a's demonstration, public: steps 1 (the tap), 2 to 4 (forward to offsets 24, 48 and 72, the
// list's end), 5 and 6 (back to 48 and 24), 7 (the press on `delta`), 8 (the detail screen); each
// page marked (steps 1, 7 and 8), the five swipes folded into the press step 7, and the scrolling
// select of the first kind marked on it.
fn selected() -> Bench {
    let bench = Bench::new();
    bench.press_at(START_POINT, [start(), list_at(0)]);
    for (swipe, (from, to)) in [FORWARD, FORWARD, FORWARD, BACK, BACK].into_iter().zip([
        (0, 24),
        (24, 48),
        (48, 72),
        (72, 48),
        (48, 24),
    ]) {
        bench.drag_list(swipe, [list_at(from), list_at(to)]);
    }
    bench.press_at(ROW_POINT, [list_at(24), detail()]);
    bench.marked(json!({"step": 1, "add": [color("ui/start", SQUARE)]}));
    bench.marked(json!({"step": 7, "add": [color("ui/list", SQUARE)]}));
    bench.marked(json!({"step": 8, "add": [color("ui/detail", SQUARE)]}));
    bench.marked(json!({"step_action": {"kind": "fold_scan", "step": 7, "first": 2}}));
    bench.marked(json!({"step": 7, "select": scrolling()}));
    bench
}

// S2-9b's first demonstration, public: steps 1 (the tap), 2 and 3 (forward to offsets 24 and 48), 4
// (the press on `india`), 5 (the detail screen); each page marked (steps 1, 4 and 5), the two swipes
// folded into the press step 4.
fn two_forward() -> Bench {
    let bench = Bench::new();
    bench.press_at(START_POINT, [start(), list_at(0)]);
    for (from, to) in [(0, 24), (24, 48)] {
        bench.drag_list(FORWARD, [list_at(from), list_at(to)]);
    }
    bench.press_at(WANTED_POINT, [list_at(48), detail()]);
    bench.marked(json!({"step": 1, "add": [color("ui/start", SQUARE)]}));
    bench.marked(json!({"step": 4, "add": [color("ui/list", SQUARE)]}));
    bench.marked(json!({"step": 5, "add": [color("ui/detail", SQUARE)]}));
    bench.marked(json!({"step_action": {"kind": "fold_scan", "step": 4, "first": 2}}));
    bench
}

// The brief's test 1: a public recording of a scrolling select read to the list's end. The answer
// is not narrowed: `lab.selects` (and the cross-check's `selects` in `lab.cross_check`) still says
// what the scan read: 11 positions, 6 screens with their names, 3 swipes and `delta` at position 3.
// The package's entry task records the select's scan as `coverage`, `pressed` without its position
// and `absent`, nothing else; the other members of the entry are the answer's. And no file the
// recording writes into the package, nor any case it writes beside it, holds a left-out member by
// its name, searched in the bytes of every file, not at one member's path.
#[test]
fn privscan_a_public_package_holds_none_of_the_left_out_members() {
    let bench = selected();
    let lab = bench.stopped(None);
    assert_eq!(lab["lab_status"], "generated", "{lab}");
    assert_eq!(lab["destination"], "public", "{lab}");
    let answered = &lab["selects"][0];
    assert_eq!(answered["scan"]["positions"], 11, "{answered}");
    assert_eq!(
        answered["scan"]["screens"].as_array().map(Vec::len),
        Some(6),
        "{answered}"
    );
    assert_eq!(
        answered["scan"]["steps"].as_array().map(Vec::len),
        Some(3),
        "{answered}"
    );
    assert_eq!(
        answered["scan"]["pressed"],
        json!({"result": "selected", "name": "delta", "position": 3}),
        "{answered}"
    );
    assert_eq!(lab["cross_check"]["selects"][0], *answered, "{lab}");

    let entries = package(&lab);
    let task = task_of(&entries);
    let recorded = &task["provenance"]["cross_check"]["selects"][0];
    assert_eq!(
        recorded["scan"],
        json!({
            "coverage": "complete",
            "pressed": {"result": "selected", "name": "delta"},
            "absent": {"name": "lima", "coverage": "complete", "decision": {"result": "empty"}}
        }),
        "{recorded}"
    );
    for member in [
        "step",
        "record_index",
        "op",
        "layout",
        "wanted",
        "pressed",
        "on_empty_key",
        "frames",
    ] {
        assert_eq!(recorded[member], answered[member], "{member}: {recorded}");
    }

    let mut written = entries.into_iter().collect::<Vec<_>>();
    written.extend(files_under(&cases_directory(&lab)));
    assert!(
        written.len() > 3,
        "{:?}",
        written.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
    for (name, bytes) in &written {
        for member in LEFT_OUT {
            let key = format!("\"{member}\":");
            assert!(!holds(bytes, key.as_bytes()), "{name} holds {member}");
        }
    }
}

// The brief's test 2: the same recording stopped again with `--destination local`: it generates
// again; its answer's entries are the public stop's; its entry task carries `provenance.destination:
// "local"` and records the answers' entries, as every recording did before this slice, the scan's
// `positions` among them; every other file of the package is the public package's byte for byte,
// and the entry task is the public one's but for its generation time, the mark and the select
// entries.
#[test]
fn privscan_a_local_package_records_the_answers_entries_as_before() {
    let bench = selected();
    let public = bench.stopped(None);
    assert_eq!(public["lab_status"], "generated", "{public}");
    let public_entries = package(&public);
    let local = bench.stopped(Some(RecordDestination::Local));
    assert_eq!(local["lab_status"], "regenerated", "{local}");
    assert_eq!(local["destination"], "local", "{local}");
    assert_eq!(local["selects"], public["selects"]);
    let local_entries = package(&local);
    let task = task_of(&local_entries);
    assert_eq!(task["provenance"]["destination"], "local", "{task}");
    assert_eq!(
        task["provenance"]["cross_check"]["selects"], local["selects"],
        "{task}"
    );
    assert_eq!(
        task["provenance"]["cross_check"]["selects"][0]["scan"]["positions"], 11,
        "{task}"
    );

    assert_eq!(
        public_entries.keys().collect::<Vec<_>>(),
        local_entries.keys().collect::<Vec<_>>()
    );
    let entry = format!("resources/operations/{TASK_ID}/task.json");
    for (name, bytes) in &public_entries {
        if *name != entry {
            assert!(local_entries.get(name) == Some(bytes), "{name} differs");
        }
    }
    let [mut one, mut other] = [task_of(&public_entries), task];
    for each in [&mut one, &mut other] {
        let provenance = each["provenance"].as_object_mut().expect("provenance");
        provenance.remove("generated_at_unix_ms");
        provenance.remove("destination");
        provenance
            .get_mut("cross_check")
            .and_then(Value::as_object_mut)
            .expect("the cross-check")
            .remove("selects");
    }
    assert_eq!(one, other);
}

// The brief's test 3: the public package made so is adopted by its path into a made-up checkout as
// held, its `standard-pack check` run by the adoption on the tree as it would be written; the
// adoption's review reads the public record as it read every record (`recorded`, cross-check
// `passed`); the adopted package's record is the public one; and every case `record stop` wrote,
// run by the one runner on the adopted package as `resource regression` runs a case of a checkout,
// passes.
#[test]
fn privscan_adopt_its_check_and_regression_pass_the_public_package() {
    let bench = selected();
    let lab = bench.stopped(None);
    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let package_id = lab["package_id"]
        .as_str()
        .expect("the package id")
        .to_owned();
    let repo = bench.temp.path().join("repo");
    checkout(&repo);
    let answer = standard_pack_adopt(
        StandardPackAdoptRequest {
            repo: repo.clone(),
            source: AdoptSource::Package(PathBuf::from(
                lab["path"].as_str().expect("package path"),
            )),
            decision: Some(AdoptDecision::Held {
                rationale: "Not run through on a real device yet.".to_owned(),
                reference: None,
            }),
            replace: false,
            dry_run: false,
            state_dir: bench.temp.path().join("adopt-state"),
            at_unix_ms: ADOPTED_AT,
        },
        &Committed,
    )
    .unwrap_or_else(|error| {
        panic!(
            "the adoption: {}: {} {:?}",
            error.code, error.message, error.details
        )
    });
    let answer = serde_json::to_value(&answer).expect("adoption JSON");
    assert_eq!(answer["status"], "adopted", "{answer}");
    assert_eq!(answer["review"]["recorded"], true, "{answer}");
    assert_eq!(answer["review"]["cross_check"], "passed", "{answer}");

    let adopted = repo.join("packs").join(&package_id);
    let task: Value = serde_json::from_slice(
        &fs::read(adopted.join(format!("resources/operations/{TASK_ID}/task.json")))
            .expect("the adopted entry task"),
    )
    .expect("task JSON");
    let recorded = &task["provenance"]["cross_check"]["selects"][0];
    assert_eq!(recorded["scan"]["coverage"], "complete", "{recorded}");
    assert!(recorded["scan"].get("screens").is_none(), "{recorded}");

    let directory = cases_directory(&lab);
    let ids = lab["cases"]["ids"].as_array().expect("the case ids");
    assert!(!ids.is_empty(), "{lab}");
    for id in ids {
        let case = directory.join(id.as_str().expect("a case id"));
        let verdict = run_regression_case(
            &adopted,
            &fs::read(case.join("case.json")).expect("case.json"),
            &fs::read(case.join("scene.png")).expect("scene.png"),
            &bench.ocr,
        );
        let verdict = serde_json::to_value(&verdict).expect("verdict JSON");
        assert_eq!(verdict["verdict"], "passed", "{id}: {verdict}");
    }
}

// The brief's test 4: S2-9b's refusal, a scan straight to the wanted item whose default `lima` no
// screen shows while the demonstration pressed `india`: record stop's self-check refuses it with
// `record_step_self_mismatch`, reason `scan_default_not_shown`, as before; stopped public and then
// with `--destination local`, the refusal is the same (its step, serial number, frame, reason and
// default), and nothing is written either time.
#[test]
fn privscan_the_self_check_refuses_the_same_case_public_and_local() {
    let bench = two_forward();
    let mut select = straight();
    select["wanted"]["default"] = json!("lima");
    select["pressed"] = json!("india");
    bench.marked(json!({"step": 4, "select": select}));
    let before = bench.files();
    let mut refusals = Vec::new();
    for destination in [None, Some(RecordDestination::Local)] {
        let error = bench
            .stop(destination)
            .expect_err("a default no screen shows");
        assert_eq!(
            error.code, "record_step_self_mismatch",
            "{}: {:?}",
            error.message, error.details
        );
        let details = error.details.expect("details");
        assert_eq!(details["reason"], "scan_default_not_shown", "{details}");
        assert_eq!(details["default"], "lima", "{details}");
        assert_eq!(bench.files(), before, "{destination:?}: a file changed");
        refusals.push(
            ["step", "record_index", "frame_id", "reason", "default"]
                .map(|member| details[member].clone()),
        );
    }
    assert_eq!(refusals[0], refusals[1]);
}

// The coordinator's test of 11-19: every member of a public record's select entry, at every depth,
// is on the named list ([`named_list`]), so that a member a later slice adds to the answers' entry
// fails here the day it reaches the record. The same walk over the answers' entry finds the scan's
// `screens`, `positions`, `steps` and the pressed name's `position`: it would have found them had the
// record been the answers' entry.
#[test]
fn privscan_a_public_record_holds_no_member_off_the_named_list() {
    let bench = selected();
    let lab = bench.stopped(None);
    let task = task_of(&package(&lab));
    let recorded = task["provenance"]["cross_check"]["selects"]
        .as_array()
        .expect("the recorded selects");
    assert_eq!(recorded.len(), 1, "{task}");
    for entry in recorded {
        assert_eq!(off_the_list(entry), Vec::<String>::new(), "{entry}");
    }
    let answered = off_the_list(&lab["selects"][0]);
    for path in [
        "/scan/screens",
        "/scan/positions",
        "/scan/steps",
        "/scan/pressed/position",
    ] {
        assert!(
            answered.iter().any(|found| found == path),
            "{path}: {answered:?}"
        );
    }
}
