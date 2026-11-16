// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #392 DA-5ab fix round 1 (contracts/display-adapter.md section 11, "Presses" and "The
// answers"; register §1.436): a Lab operation speaks its package's base, so the recording takes
// from it only what it can take. Its two tests of DA-5ab's own checks against the package's base
// (`da5ab_fix1_a_recorded_press_takes_a_recording_of_its_packages_base_only`,
// `da5ab_fix1_a_recorded_observation_takes_a_frame_of_its_packages_base_only`) are retired by the
// train of DA-5ab and DA-5cd (register §1.436 and §1.437, "Train"; ci/retired-gates.toml), whose
// rule R4 replaces those checks with the recording's own judgement:
// - `da5_train_a_frame_a_recording_path_took_is_judged_before_anything_is_written`: a recording
//   whose header names the base size 1280x720 and that holds no frame passes the recording's
//   judgement (`record_size_check`); a frame of 1920x1080 a path took before it sends anything
//   (`record_frame_check`) is refused `record_display_not_base` (it would be the first frame),
//   with its size and the base size; a frame of 1280x720 passes. Once the recording holds a frame
//   of 1280x720, a frame of 1920x1080 is refused `record_frame_size_mismatch` (the display changed
//   during the recording; the recording's size expected, the frame's actual) and the recording's
//   judgement still passes. Nothing of the recording is written by any of them.
// - `da5_train_a_recorded_press_on_a_recording_not_at_the_base_sends_nothing`: a recording whose
//   header names no base size (one made before DA-5cd's fix round) and whose frames are 1920x1080,
//   its open step marked so that the frame before a press would not be stored (the case DA-5cd's
//   fix round left, where the press went first): the recording's judgement and a frame of its own
//   size are each refused `record_size_not_base` with the frames' size and the base size, and
//   `tap 0 0 --record` (`record_press`) is refused `record_size_not_base` before anything is
//   captured or sent: the fake capture and the press are never called, and the recording's files
//   are byte for byte as they were.
// The train's fix round 1 (coordinator work order rd-1116/brief-392-train-lab-fix1.md; the two
// reviews of the train at `2ff33cb3`): `tap 0 0 --record` (`record_press`) against a fake Runtime
// whose captures are frames of the sizes the test gives, with the reference of each:
// - `da5_train_fix1_a_first_frame_the_recording_would_refuse_is_refused_before_the_press`: a
//   recording whose header names no base size and that holds no frame, a capture of 1920x1080: it
//   would be the recording's first frame and the recording would then be one `record stop`
//   refuses, so it is refused `record_display_not_base` with its size and the base size, after the
//   one capture and before anything is stored: the press is never called and the recording's files
//   are byte for byte as they were.
// - `da5_train_fix1_the_retry_of_a_recorded_press_judges_the_recording_again_before_it_sends`: a
//   recording whose header names the base size, its first frame 1280x720 and its open step marked;
//   the press is refused because the Runtime no longer holds its frame (nothing sent), and the
//   recording's header has meanwhile been changed by hand to frames of 1920x1080: the retry judges
//   the recording again before it captures, `record_size_not_base`, so the press is called once,
//   one capture is taken, and nothing is written after the change.
// - `da5_train_fix1_a_frame_not_stored_is_checked_against_a_recording_with_a_base`: a recording
//   whose header names the base size, its first frame 1280x720 and its open step marked, so the
//   frame before the press is not stored: a capture of 1920x1080 is refused
//   `record_frame_size_mismatch` (the recording's 1280x720 expected, 1920x1080 actual), the press
//   is never called and the recording's files are as they were.
// Frames are synthetic; no Runtime, no device, no clock read by the tests; the session header is
// written as `record start` writes it.

use crate::recording::store::{
    LabFile, lab_dir, load_lab, old_record_path, read_old_record, save_recording,
};
use crate::{
    AfterCapture, AttachFrameRequest, CapturedFrame, ClickPlan, CommitClickRequest,
    LAB_RECORD_MARK_SCHEMA, LabOcr, MarkFamily, MarkRequest, MarkSpec, PlanClickRequest,
    PointPress, RECORDING_BASE_SIZE, RecordFramePorts, RecordPoint, RecordPressRequest, RecordRect,
    RecordSize, RecordStartOptions, RecordingLock, record_attach_frame, record_frame_check,
    record_mark, record_press, record_press_frame_unavailable, record_size_check, record_start,
};
use actingcommand_contract::{InputFrameReference, LabError, LabResult};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_0392_da5_train";
const TASK_ID: &str = "neutral_task";
const STARTED_AT: u64 = 1_700_000_000_000;
/// The label the checks name the frame they judge by.
const LABEL: &str = "the frame the path took";

/// A state directory with an active session of `INSTANCE` and its Lab recording, without a frame,
/// and the recording lock held as one command holds it (dropped before the directory).
struct Bench {
    lock: RecordingLock,
    ocr: LabOcr,
    temp: TempDir,
}

impl Bench {
    /// The recording started with `base` in its header (`None`: no base size, as a recording made
    /// before DA-5cd's fix round).
    fn new(base: Option<RecordSize>) -> Self {
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
        let lock = RecordingLock::acquire(&state, INSTANCE, "gate_392_da5ab", "Lab-CLI")
            .expect("recording lock");
        let ocr = LabOcr::from_vision_root(&temp.path().join("no-vision"));
        let started = record_start(
            &lock,
            &RecordStartOptions {
                game: Some("neutral".to_owned()),
                server: Some("test".to_owned()),
                locale: Some("en-US".to_owned()),
                base_size: base,
                ..RecordStartOptions::default()
            },
        )
        .expect("record start");
        assert_eq!(started.status, "active", "{started:?}");
        Self { lock, ocr, temp }
    }

    fn state(&self) -> PathBuf {
        self.temp.path().join("state")
    }

    /// A device frame of `width` x `height`, as `capture --record` attaches it.
    fn attach(&self, width: u32, height: u32) {
        record_attach_frame(
            &self.lock,
            AttachFrameRequest {
                png: frame_png(width, height),
                source: "lab_capture".to_owned(),
                runtime_artifact: None,
                capture_backend: Some("adb_screencap".to_owned()),
                freshness: None,
            },
        )
        .unwrap_or_else(|error| panic!("frame: {}: {}", error.code, error.message));
    }

    /// `record mark --step 1 --color square=0,0,40,40`: the open step marked.
    fn mark_step_one(&self) {
        let request = MarkRequest {
            schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
            step: Some(1),
            add: vec![MarkSpec::region_only(
                "square".to_owned(),
                MarkFamily::Color,
                RecordRect {
                    x: 0,
                    y: 0,
                    width: 40,
                    height: 40,
                },
            )],
            ..MarkRequest::default()
        };
        record_mark(&self.lock, &request, false, &self.ocr)
            .unwrap_or_else(|error| panic!("mark: {}: {:?}", error.code, error.details));
    }

    /// Every file of the recording's directory, by its `/`-separated path relative to it, with its
    /// bytes, in name order.
    fn files(&self) -> Vec<(String, Vec<u8>)> {
        files_under(&lab_dir(&self.state(), RECORD_ID))
    }
}

/// The PNG of a synthetic device frame of `width` x `height`, one colour throughout.
fn frame_png(width: u32, height: u32) -> Vec<u8> {
    let pixels = (0..width * height).flat_map(|_| [200_u8, 40, 40]).collect();
    Frame::from_pixels(
        width,
        height,
        pixels,
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
    .encode_png_fast()
    .expect("fixture PNG")
}

/// Every file under `dir`, by its `/`-separated path relative to it, with its bytes, in name order.
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

/// The values of a refusal's outcome, as its details carry them.
fn values(error: &LabError) -> Value {
    error.details.clone().unwrap_or(Value::Null)["outcome"]["values"].clone()
}

/// The values `record_display_not_base` and `record_size_not_base` carry for frames of `width` x
/// `height` and the base size 1280x720.
fn not_base_values(width: u32, height: u32) -> Value {
    json!({"frame_width": width, "frame_height": height, "base_width": 1280, "base_height": 720})
}

/// The fake Runtime of a recorded press that must not be reached: each capture or wait it is
/// asked for is logged and fails.
struct Untouched<'a> {
    log: &'a RefCell<Vec<String>>,
}

impl RecordFramePorts for Untouched<'_> {
    fn wait(&mut self, duration: Duration) {
        self.log
            .borrow_mut()
            .push(format!("wait {}", duration.as_millis()));
    }

    fn capture(&mut self) -> LabResult<CapturedFrame> {
        self.log.borrow_mut().push("capture".to_owned());
        Err(LabError::device("the test's fake Runtime captures nothing"))
    }

    fn elapsed(&mut self) -> Duration {
        Duration::ZERO
    }
}

#[test]
fn da5_train_a_frame_a_recording_path_took_is_judged_before_anything_is_written() {
    let bench = Bench::new(Some(RECORDING_BASE_SIZE));
    record_size_check(&bench.lock)
        .unwrap_or_else(|error| panic!("a recording without a frame: {}", error.message));
    let before = bench.files();
    let first = record_frame_check(&bench.lock, (1920, 1080), LABEL)
        .expect_err("a first frame of another size than the base");
    assert_eq!(first.code, "record_display_not_base", "{}", first.message);
    assert_eq!(
        values(&first),
        not_base_values(1920, 1080),
        "{}",
        first.message
    );
    record_frame_check(&bench.lock, (1280, 720), LABEL)
        .unwrap_or_else(|error| panic!("a first frame of the base size: {}", error.message));
    assert_eq!(bench.files(), before, "nothing written");

    bench.attach(1280, 720);
    let before = bench.files();
    let changed = record_frame_check(&bench.lock, (1920, 1080), LABEL)
        .expect_err("a frame of another size than the recording");
    assert_eq!(
        changed.code, "record_frame_size_mismatch",
        "{}",
        changed.message
    );
    assert_eq!(
        changed.details,
        Some(json!({
            "frame": LABEL,
            "expected": {"width": 1280, "height": 720},
            "actual": {"width": 1920, "height": 1080},
        })),
        "{}",
        changed.message
    );
    record_size_check(&bench.lock)
        .unwrap_or_else(|error| panic!("a recording of the base size: {}", error.message));
    record_frame_check(&bench.lock, (1280, 720), LABEL)
        .unwrap_or_else(|error| panic!("a frame of the recording's size: {}", error.message));
    assert_eq!(bench.files(), before, "nothing written");
}

#[test]
fn da5_train_a_recorded_press_on_a_recording_not_at_the_base_sends_nothing() {
    let bench = Bench::new(None);
    // A recording whose header names no base size takes its first frame's size, as one made on a
    // display of 1920x1080 before DA-5cd's fix round did; its open step is marked, so the frame
    // before a press would not be stored.
    bench.attach(1920, 1080);
    bench.mark_step_one();
    let before = bench.files();

    let judged = record_size_check(&bench.lock).expect_err("a recording not at the base size");
    assert_eq!(judged.code, "record_size_not_base", "{}", judged.message);
    assert_eq!(
        values(&judged),
        not_base_values(1920, 1080),
        "{}",
        judged.message
    );
    let framed = record_frame_check(&bench.lock, (1920, 1080), LABEL)
        .expect_err("a frame of a recording not at the base size");
    assert_eq!(framed.code, "record_size_not_base", "{}", framed.message);

    let log = RefCell::new(Vec::new());
    let mut ports = Untouched { log: &log };
    let mut press =
        |_plan: &ClickPlan, _frame: InputFrameReference| -> LabResult<CommitClickRequest> {
            log.borrow_mut().push("press".to_owned());
            Err(LabError::device("the test's fake Runtime presses nothing"))
        };
    let request = RecordPressRequest {
        click: PlanClickRequest::default(),
        point_press: Some(PointPress::Tap {
            point: RecordPoint { x: 0, y: 0 },
        }),
        after: AfterCapture {
            still_ms: Some(0),
            ..AfterCapture::for_press(Some(0))
        },
    };
    let pressed = record_press(&bench.lock, &request, &mut press, &mut ports)
        .expect_err("a recorded press on a recording not at the base size");
    assert_eq!(pressed.code, "record_size_not_base", "{}", pressed.message);
    assert_eq!(
        values(&pressed),
        not_base_values(1920, 1080),
        "{}",
        pressed.message
    );
    assert!(
        log.borrow().is_empty(),
        "nothing captured or sent: {:?}",
        log.borrow()
    );
    assert_eq!(bench.files(), before, "nothing written");
}

/// The train's fix round 1: the fake Runtime of a recorded press that is reached. Each capture is a
/// frame of the next size of `sizes` (the last again once they run out), with the reference of the
/// frame it committed, of that size; each capture and wait is logged.
struct Captures<'a> {
    sizes: Vec<(u32, u32)>,
    taken: usize,
    log: &'a RefCell<Vec<String>>,
}

impl RecordFramePorts for Captures<'_> {
    fn wait(&mut self, duration: Duration) {
        self.log
            .borrow_mut()
            .push(format!("wait {}", duration.as_millis()));
    }

    fn capture(&mut self) -> LabResult<CapturedFrame> {
        self.log.borrow_mut().push("capture".to_owned());
        let last = self.sizes.len().saturating_sub(1);
        let (width, height) = self.sizes[self.taken.min(last)];
        self.taken += 1;
        Ok(CapturedFrame {
            frame: AttachFrameRequest {
                png: frame_png(width, height),
                source: "lab_capture".to_owned(),
                runtime_artifact: None,
                capture_backend: Some("adb_screencap".to_owned()),
                freshness: None,
            },
            input_frame: Some(InputFrameReference {
                frame_id: serde_json::from_value(json!(format!("frame_{:032x}", self.taken)))
                    .expect("a frame id"),
                width,
                height,
            }),
        })
    }

    fn elapsed(&mut self) -> Duration {
        Duration::ZERO
    }
}

/// `tap 0 0 --record`, with no wait and no stillness test after the press.
fn tap_at_origin() -> RecordPressRequest {
    RecordPressRequest {
        click: PlanClickRequest::default(),
        point_press: Some(PointPress::Tap {
            point: RecordPoint { x: 0, y: 0 },
        }),
        after: AfterCapture {
            still_ms: Some(0),
            ..AfterCapture::for_press(Some(0))
        },
    }
}

/// The recording's header changed by hand, as no command writes it, so that it says its frames
/// are of `size`: the step table loaded as the Lab loads it and written back as the Lab writes it.
fn change_frames_size(state: &Path, size: RecordSize) {
    let header = read_old_record(state, INSTANCE)
        .expect("the session header")
        .expect("a session header");
    let LabFile::Ready(mut recording) = load_lab(state, &header).expect("the step table") else {
        panic!("the step table does not load");
    };
    recording.coordinate_space = Some(size);
    save_recording(state, &recording).expect("the step table written");
}

#[test]
fn da5_train_fix1_a_first_frame_the_recording_would_refuse_is_refused_before_the_press() {
    let bench = Bench::new(None);
    let before = bench.files();
    let log = RefCell::new(Vec::new());
    let mut ports = Captures {
        sizes: vec![(1920, 1080)],
        taken: 0,
        log: &log,
    };
    let mut press =
        |_plan: &ClickPlan, _frame: InputFrameReference| -> LabResult<CommitClickRequest> {
            log.borrow_mut().push("press".to_owned());
            Err(LabError::device("the test's fake Runtime presses nothing"))
        };
    let refused = record_press(&bench.lock, &tap_at_origin(), &mut press, &mut ports)
        .expect_err("a first frame that would make a recording record stop refuses");
    assert_eq!(
        refused.code, "record_display_not_base",
        "{}",
        refused.message
    );
    assert_eq!(
        values(&refused),
        not_base_values(1920, 1080),
        "{}",
        refused.message
    );
    assert_eq!(
        *log.borrow(),
        ["capture"],
        "one capture, and the press never called"
    );
    assert_eq!(bench.files(), before, "nothing stored or written");
}

#[test]
fn da5_train_fix1_the_retry_of_a_recorded_press_judges_the_recording_again_before_it_sends() {
    let bench = Bench::new(Some(RECORDING_BASE_SIZE));
    bench.attach(1280, 720);
    bench.mark_step_one();
    let state = bench.state();
    let log = RefCell::new(Vec::new());
    let changed = RefCell::new(None);
    let mut ports = Captures {
        sizes: vec![(1280, 720)],
        taken: 0,
        log: &log,
    };
    // The Runtime no longer holds the frame the press names, so nothing was sent; meanwhile the
    // recording's header was changed by hand to frames of 1920x1080, not the base size its header
    // names.
    let mut press =
        |_plan: &ClickPlan, _frame: InputFrameReference| -> LabResult<CommitClickRequest> {
            log.borrow_mut().push("press".to_owned());
            change_frames_size(
                &state,
                RecordSize {
                    width: 1920,
                    height: 1080,
                },
            );
            *changed.borrow_mut() = Some(bench.files());
            Err(record_press_frame_unavailable(LabError::device(
                "the test's fake Runtime no longer holds the frame the press names",
            )))
        };
    let refused = record_press(&bench.lock, &tap_at_origin(), &mut press, &mut ports)
        .expect_err("the retry on a recording no longer at the base size");
    assert_eq!(refused.code, "record_size_not_base", "{}", refused.message);
    assert_eq!(
        values(&refused),
        not_base_values(1920, 1080),
        "{}",
        refused.message
    );
    assert_eq!(
        *log.borrow(),
        ["capture", "press"],
        "the press called once, and the retry judged the recording before it captured"
    );
    let changed = changed.borrow().clone().expect("the press was called");
    assert_eq!(bench.files(), changed, "nothing written after the change");
}

#[test]
fn da5_train_fix1_a_frame_not_stored_is_checked_against_a_recording_with_a_base() {
    let bench = Bench::new(Some(RECORDING_BASE_SIZE));
    bench.attach(1280, 720);
    // The open step is marked, so the frame before the press is not stored.
    bench.mark_step_one();
    let before = bench.files();
    let log = RefCell::new(Vec::new());
    let mut ports = Captures {
        sizes: vec![(1920, 1080)],
        taken: 0,
        log: &log,
    };
    let mut press =
        |_plan: &ClickPlan, _frame: InputFrameReference| -> LabResult<CommitClickRequest> {
            log.borrow_mut().push("press".to_owned());
            Err(LabError::device("the test's fake Runtime presses nothing"))
        };
    let refused = record_press(&bench.lock, &tap_at_origin(), &mut press, &mut ports)
        .expect_err("a frame of another size than the recording");
    assert_eq!(
        refused.code, "record_frame_size_mismatch",
        "{}",
        refused.message
    );
    let details = refused.details.clone().unwrap_or(Value::Null);
    assert_eq!(
        details["expected"],
        json!({"width": 1280, "height": 720}),
        "{details}"
    );
    assert_eq!(
        details["actual"],
        json!({"width": 1920, "height": 1080}),
        "{details}"
    );
    assert_eq!(
        *log.borrow(),
        ["capture"],
        "one capture, and the press never called"
    );
    assert_eq!(bench.files(), before, "nothing written");
}
