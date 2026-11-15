// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #449 REC-ARCH (contracts/lab-recording.md, "Archiving" and "Recorded swipes"; register
// §1.344): a recording can be ended as material only, and a swipe sent with `--record` is a step.
// - `rec_arch_a_swipe_is_a_step_and_record_stop_generates_it_as_a_drag`: a swipe on the open step
//   that declares no effect declares the step's drag between boxes of one pixel at its two points
//   (`source: "swipe"`) and records its execution; marked, the recording's `record stop` generates
//   the standard `drag` click of those two boxes and duration.
// - `rec_arch_steps_of_several_callers_archive_and_the_rows_equal_the_step_table`: a swipe and a tap
//   added by two initiators and a last step without input; `record archive` writes
//   `archive-manifest-1.json`, whose header and rows (kind, frames, the Runtime frames before and
//   after, page, package, tap or swipe, time, request id) are the step table's and whose step table
//   hash is that of the step table file; the step table and every frame keep their bytes; the copy
//   into a directory that does not exist holds them byte for byte and the manifest.
// - `rec_arch_a_step_table_written_by_0_12_54_archives`: a step table in the form 0.12.54 wrote
//   (built from the members the format has at the slice's base) archives, its rows its steps.
// - `rec_arch_a_recorded_frame_tap_and_swipe_name_their_recording_and_step`: the answers of
//   `record_attach_frame` and `record_commit_click` carry the recording's id, when it started and the
//   step the action became.
// - `rec_arch_without_an_active_recording_or_with_an_unknown_id_nothing_is_archived`: without a
//   session, with a stopped one and with an id no recording has, nothing is written.
// #463 S1-5 (contracts/lab-recording.md, "Archiving", "The manifest"; register §1.406): the manifest
// is `actingcommand.lab-recording-archive.v2`. Adapted, as the requirement changes what they pin:
// the two tests of the rows above give the v2 rows (the frames' `superseded` and
// `recorded_at_unix_ms`, `before_frame`, `after_frame` and `derived: true` by the rule for presses
// of `do --capture --record`, `outcome`, `tap.source`), and the copy holds the recording's files
// with its manifest saying `copy: {frames: "material"}`.
// - `s15_a_stage_one_recording_archives_its_presses_their_screens_and_the_final_screen`: a launch on
//   the empty recording, a tap, a long press, a swipe and a restart, each as stage one records it:
//   every row names the frames its effect's step table names (`tap.rect` and `long_press.rect`
//   `null`), `material.clicks` names each press with its frame, the entry's with none and `entry`
//   true, and where it was pressed, `material.final_frame` the screen after the restart, and
//   there is no warning.
// - `s15_a_recording_made_with_do_archives_with_derived_frames`: presses of `do --capture
//   --record`: `before_frame` the step's live primary frame (not its first), `after_frame` the
//   next effective step's first frame or `null` after the last, `derived: true`, the material's
//   frames derived too; no final screen: `final_frame` `null` and `final_frame_missing`.
// - `s15_a_v1_manifest_on_disk_is_named_again_copied_as_it_is_and_reopened`: a v1 manifest an
//   earlier build wrote is read and verified (a frame changed is `record_archive_verify_failed`),
//   warns nothing, is copied with every file it lists and itself byte for byte, and reopens; the
//   next archive writes `archive-manifest-2.json` as v2 and leaves the first as it was.
// - `s15_without_a_screen_after_the_last_press_there_is_no_final_frame`: two taps whose screens
//   after cannot be taken: the first's `after_frame` is the second's capture before, without
//   `after_capture`; the archive stands with `final_frame` `null` and `final_frame_missing`.
// - `s15_the_copy_holds_the_material_unless_every_frame_is_asked_for`: the copy holds the step
//   table, the manifest and the frames the material names, not a superseded frame, its manifest
//   `copy.frames` `material`; with every frame asked for, the superseded one too and `all`; a
//   superseded frame that no longer holds its sha256 fails the archive named again.
// #463 S1-5 fix round 2 (rd-1115/brief-463-s15-fix2.md, after the review of #1763): every
// `material.clicks[].sent_on_unstored_capture` is `null` (adapted: `material_click`).
// - `s15_fix2_a_copy_that_fails_after_its_first_frame_leaves_no_manifest`: the second frame the
//   copy takes no longer holds its sha256: the copy fails `record_archive_verify_failed` and holds
//   the step table and the frames, and no manifest, which is written last.
// - `s15_fix2_an_application_operation_recorded_the_old_way_is_derived`: a completed receipt
//   without frames and time: `before_frame` the live primary frame, `derived: true`, `after_frame`
//   and `at_unix_ms` `null`; the material's press derived, with its action.
// - `s15_fix2_an_indeterminate_press_is_in_the_material`: `outcome: "indeterminate"` in the row and
//   in `material.clicks`; no final screen.
// - `s15_fix2_dropped_and_converted_steps_are_listed_and_not_in_the_material`: both rows listed (the
//   converted step's frame in the transition of the step before it), neither a press of the
//   material, though the dropped step's press was performed.
// - `s15_fix2_a_step_table_without_executed_at_is_written_back_byte_for_byte`: receipts without
//   `executed_at_unix_ms`, as the layer below writes them, load and are written back byte for byte.
// Frames are synthetic (16x9, a coloured square on gray); no Runtime, no device, no clock read by
// the tests themselves; the session header is written as `record start` writes it.

use crate::recording::store::{
    LabFile, hex_sha256, lab_dir, load_lab, old_record_path, read_old_record, recording_path,
    save_recording,
};
use crate::{
    AfterCapture, ApplicationExecution, ArchiveCopyFrames, AttachFrameOutcome, AttachFrameRequest,
    CapturedFrame, ClickEffect, ClickPlan, CommitApplicationRequest, CommitClickOutcome,
    CommitClickRequest, LAB_RECORD_MARK_SCHEMA, LabOcr, MarkFamily, MarkRequest, MarkSpec,
    OpaqueJson, PlanApplicationRequest, PlanClickRequest, PlannedSwipe, PointPress,
    RecordArchiveOptions, RecordFramePorts, RecordPoint, RecordPressOutcome, RecordPressRequest,
    RecordRect, RecordReopenOptions, RecordStartOptions, RecordStopOptions, RecordingLock,
    StepAction, StepActionKind, record_after_frame, record_archive, record_archive_copy,
    record_archive_warnings, record_attach_frame, record_before_frame, record_commit_application,
    record_commit_click, record_mark, record_plan_application, record_plan_click, record_press,
    record_reopen, record_start, record_stop,
};
use actingcommand_contract::{InputFrameReference, LabError, LabResult};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_pack_containment::{
    ContainmentLimits, ContentContainer, expand_content_container,
};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_0449";
const TASK_ID: &str = "neutral_task";
const STARTED_AT: u64 = 1_700_000_000_000;
const WRITTEN_BY: &str = "0123456789abcdef0123456789abcdef01234567";
const RED: [u8; 3] = [230, 20, 20];
const GREEN: [u8; 3] = [20, 230, 20];
const BLUE: [u8; 3] = [20, 20, 230];
const GRAY: [u8; 3] = [60, 60, 60];
const SQUARE: RecordRect = RecordRect {
    x: 0,
    y: 0,
    width: 4,
    height: 4,
};
const TAP: RecordRect = RecordRect {
    x: 8,
    y: 2,
    width: 4,
    height: 4,
};

fn rect(x: i32, y: i32, width: i32, height: i32) -> RecordRect {
    RecordRect {
        x,
        y,
        width,
        height,
    }
}

// A 16x9 frame: a 4x4 square of `color` at the top left on gray.
fn frame_png(color: [u8; 3]) -> Vec<u8> {
    let mut pixels = Vec::new();
    for y in 0..9 {
        for x in 0..16 {
            pixels.extend(if x < 4 && y < 4 { color } else { GRAY });
        }
    }
    Frame::from_pixels(
        16,
        9,
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

// What `do --capture --record` hands the recording for a Performed input of request `request`.
fn performed(request: &str) -> CommitClickRequest {
    CommitClickRequest {
        effect: ClickEffect::Performed,
        has_failure: false,
        carrier_package: opaque(json!({"package_label": "neutral.package"})),
        req_id: opaque(json!(request)),
        correlation_id: opaque(json!(format!("{request}-correlation"))),
        action_id: opaque(json!(format!("{request}-action"))),
        lease_id: opaque(json!("lease-0449")),
        failure: None,
        before: opaque(json!({
            "frame_id": format!("{request}-before"),
            "frame_sha256": hex_sha256(request.as_bytes()),
            "page": "neutral_page",
            "informational": true
        })),
        after: opaque(json!({
            "frame_id": format!("{request}-after"),
            "frame_sha256": null,
            "page": null,
            "informational": true
        })),
    }
}

// #463 S1-5: screen `number` of the fake Runtime, a square of its own colour, so that no two
// screens share a file.
fn screen_png(number: u8) -> Vec<u8> {
    frame_png([20 + 25 * number, 200 - 20 * number, 90 + 15 * number])
}

// #463 S1-5: what `tap`, `swipe` or `long-tap --record` hands the recording for a performed input
// of request `request`: no carrier package, no Runtime frames.
fn sent(request: &str) -> CommitClickRequest {
    CommitClickRequest {
        effect: ClickEffect::Performed,
        has_failure: false,
        carrier_package: None,
        req_id: opaque(json!(request)),
        correlation_id: opaque(json!(format!("{request}-correlation"))),
        action_id: opaque(json!(format!("{request}-action"))),
        lease_id: None,
        failure: None,
        before: None,
        after: None,
    }
}

// #463 S1-5: the completed receipt `session app <verb>` hands the recording: its screens and its
// time are the recording's to write.
fn completed(verb: &str) -> ApplicationExecution {
    ApplicationExecution {
        request_id: format!("request-{verb}"),
        correlation_id: format!("request-{verb}-correlation"),
        action_id: format!("request-{verb}-action"),
        receipt_state: "completed".to_owned(),
        application_event_ids: Vec::new(),
        before_frame: None,
        after_frame: None,
        after_capture: None,
        executed_at_unix_ms: None,
    }
}

// #463 S1-5: the fake Runtime's captures, each the screen its script names (`None`: that capture
// fails), with the reference of the frame it committed; the caller's clock waits for nothing.
struct Screens {
    script: VecDeque<Option<u8>>,
    taken: u32,
}

impl Screens {
    fn new<const N: usize>(script: [Option<u8>; N]) -> Self {
        Self {
            script: VecDeque::from(script),
            taken: 0,
        }
    }
}

impl RecordFramePorts for Screens {
    fn wait(&mut self, _duration: Duration) {}

    fn capture(&mut self) -> LabResult<CapturedFrame> {
        let number = self
            .script
            .pop_front()
            .expect("a capture the test scripted")
            .ok_or_else(|| LabError::device("the fake capture took no frame"))?;
        self.taken += 1;
        Ok(CapturedFrame {
            frame: AttachFrameRequest {
                png: screen_png(number),
                source: "lab_capture".to_owned(),
                runtime_artifact: None,
                capture_backend: Some("adb_screencap".to_owned()),
                freshness: None,
            },
            input_frame: Some(InputFrameReference {
                frame_id: serde_json::from_value(json!(format!("frame_{:032x}", self.taken)))
                    .expect("a frame id"),
                width: 16,
                height: 9,
            }),
        })
    }
}

// #463 S1-5: `tap <x> <y> --record`.
fn tap_at(x: i32, y: i32) -> RecordPressRequest {
    RecordPressRequest {
        click: PlanClickRequest::default(),
        point_press: Some(PointPress::Tap {
            point: RecordPoint { x, y },
        }),
        after: AfterCapture::for_press(None),
    }
}

// A state directory with an active session of `INSTANCE` (the header `record start` writes) and
// its Lab recording. Each action takes the recording lock as `initiator` and drops it.
struct Bench {
    ocr: LabOcr,
    temp: TempDir,
}

impl Bench {
    fn new() -> Self {
        let temp = TempDir::new().expect("temporary directory");
        let bench = Self {
            ocr: LabOcr::from_vision_root(&temp.path().join("no-vision")),
            temp,
        };
        fs::create_dir_all(bench.state()).expect("state directory");
        bench.header("active");
        record_start(
            &bench.lock("Lab-CLI"),
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

    // The session header with `status`, as the `record` command writes it.
    fn header(&self, status: &str) {
        fs::write(
            old_record_path(&self.state(), INSTANCE),
            serde_json::to_vec(&json!({
                "record_id": RECORD_ID,
                "task_id": TASK_ID,
                "instance": INSTANCE,
                "status": status,
                "started_at_unix_ms": STARTED_AT
            }))
            .expect("session JSON"),
        )
        .expect("session file");
    }

    fn lock(&self, initiator: &str) -> RecordingLock {
        RecordingLock::acquire(&self.state(), INSTANCE, "gate_449", initiator)
            .expect("recording lock")
    }

    // `observe --capture --record`: a Runtime frame showing `color`.
    fn observe(&self, initiator: &str, color: [u8; 3]) -> AttachFrameOutcome {
        record_attach_frame(
            &self.lock(initiator),
            AttachFrameRequest {
                png: frame_png(color),
                source: "runtime_observation".to_owned(),
                runtime_artifact: opaque(json!({"frame_id": "runtime-frame"})),
                capture_backend: Some("adb_screencap".to_owned()),
                freshness: None,
            },
        )
        .unwrap_or_else(|error| panic!("frame: {}: {}", error.code, error.message))
    }

    // `do --capture --record` with `request`, Performed: the recording plans the step's input
    // (`record_plan_click`), the Runtime presses it, and the recording takes the Runtime's answer
    // (`record_commit_click`), as `do --capture --record` and the crate's other recording gates
    // do; no input port is called here. Fix 2 (rd-1113/brief-1675-fix2.md): the two helpers
    // below are `commit_tap` and `commit_swipe`, since the guard
    // `lab_input_face_is_semantic_input_executor` reads every method call named like a
    // `LabInputPort` write (`tap`, `swipe`, ...) as one.
    fn press(
        &self,
        initiator: &str,
        request: PlanClickRequest,
        id: &str,
    ) -> Result<CommitClickOutcome, LabError> {
        let lock = self.lock(initiator);
        let plan = record_plan_click(&lock, &request)?;
        record_commit_click(&lock, &plan, &performed(id))
    }

    // `do --capture --record --tap-rect` of `TAP` at its centre.
    fn commit_tap(&self, initiator: &str, id: &str) -> CommitClickOutcome {
        self.press(
            initiator,
            PlanClickRequest {
                tap_rect: Some(TAP),
                ..PlanClickRequest::default()
            },
            id,
        )
        .unwrap_or_else(|error| panic!("tap: {}: {}", error.code, error.message))
    }

    // `do --capture --record --swipe 9,3,13,6,300`.
    fn commit_swipe(&self, initiator: &str, id: &str) -> CommitClickOutcome {
        self.press(
            initiator,
            PlanClickRequest {
                swipe: Some(PlannedSwipe {
                    from: RecordPoint { x: 9, y: 3 },
                    to: RecordPoint { x: 13, y: 6 },
                    duration_ms: 300,
                }),
                ..PlanClickRequest::default()
            },
            id,
        )
        .unwrap_or_else(|error| panic!("swipe: {}: {}", error.code, error.message))
    }

    // `record mark --step <step> --color <id>=0,0,4,4`.
    fn mark(&self, step: u32, id: &str) {
        let request = MarkRequest {
            schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
            step: Some(step),
            add: vec![MarkSpec::region_only(
                id.to_owned(),
                MarkFamily::Color,
                SQUARE,
            )],
            ..MarkRequest::default()
        };
        record_mark(&self.lock("Lab-CLI"), &request, false, &self.ocr)
            .unwrap_or_else(|error| panic!("mark {step}: {}: {}", error.code, error.message));
    }

    fn archive(
        &self,
        options: &RecordArchiveOptions,
    ) -> Result<crate::RecordArchiveOutcome, LabError> {
        record_archive(&self.lock("Lab-CLI"), options)
    }

    // #463 S1-5: `record archive` of the active recording, or with `record_id` of the archived one
    // named again; it must stand.
    fn archived(&self, record_id: Option<&str>) -> crate::RecordArchiveOutcome {
        self.archive(&RecordArchiveOptions {
            record_id: record_id.map(str::to_owned),
            copy_to: None,
            written_by: WRITTEN_BY.to_owned(),
        })
        .unwrap_or_else(|error| panic!("archive: {}: {:?}", error.code, error.details))
    }

    // #463 S1-5: `tap`, `swipe` or `long-tap --record` of `request`, as `record_press` records it:
    // the fake Runtime performs the input, and its captures take `screens` in order.
    fn press_recorded<const N: usize>(
        &self,
        request: &RecordPressRequest,
        screens: [Option<u8>; N],
    ) -> LabResult<RecordPressOutcome> {
        let mut device = Screens::new(screens);
        let mut press = |_: &ClickPlan, _: InputFrameReference| -> LabResult<CommitClickRequest> {
            Ok(sent("request-s15"))
        };
        record_press(&self.lock("Lab-CLI"), request, &mut press, &mut device)
    }

    // #463 S1-5: as `press_recorded`, a press that must stand.
    fn pressed<const N: usize>(&self, request: &RecordPressRequest, screens: [Option<u8>; N]) {
        self.press_recorded(request, screens)
            .unwrap_or_else(|error| panic!("press: {}: {:?}", error.code, error.details));
    }

    // #463 S1-5: `session app launch --record` on the recording without a step: the application
    // entry step, with a completed receipt and no frame; no device is asked.
    fn entry_launch(&self) {
        let lock = self.lock("Lab-CLI");
        let plan = record_plan_application(
            &lock,
            &PlanApplicationRequest {
                verb: "launch".to_owned(),
            },
        )
        .unwrap_or_else(|error| panic!("plan: {}: {:?}", error.code, error.details));
        assert!(plan.opens_entry_step, "{plan:?}");
        record_commit_application(
            &lock,
            &plan,
            &CommitApplicationRequest::Performed(completed("launch")),
        )
        .unwrap_or_else(|error| panic!("launch: {}: {:?}", error.code, error.details));
    }

    // #463 S1-5: `session app <verb> --record` on the open step, with a completed receipt, as it
    // records it: the capture before it (screen `before`), the operation, then the wait and the
    // capture after it (screen `after`).
    fn operated(&self, verb: &str, before: u8, after: u8) {
        let lock = self.lock("Lab-CLI");
        let mut device = Screens::new([Some(before), Some(after)]);
        record_before_frame(&lock, &mut device)
            .unwrap_or_else(|error| panic!("before: {}: {:?}", error.code, error.details));
        let plan = record_plan_application(
            &lock,
            &PlanApplicationRequest {
                verb: verb.to_owned(),
            },
        )
        .unwrap_or_else(|error| panic!("plan: {}: {:?}", error.code, error.details));
        record_commit_application(
            &lock,
            &plan,
            &CommitApplicationRequest::Performed(completed(verb)),
        )
        .unwrap_or_else(|error| panic!("{verb}: {}: {:?}", error.code, error.details));
        record_after_frame(&lock, &mut device, &AfterCapture::for_application(None))
            .unwrap_or_else(|error| panic!("after: {}: {:?}", error.code, error.details));
    }

    // #463 S1-5 fix round 2: `record mark --drop-step <step>` or `--to-transition <step>`.
    fn step_action(&self, kind: StepActionKind, step: u32) {
        let request = MarkRequest {
            schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
            step_action: Some(StepAction {
                kind,
                step: Some(step),
            }),
            ..MarkRequest::default()
        };
        record_mark(&self.lock("Lab-CLI"), &request, false, &self.ocr)
            .unwrap_or_else(|error| panic!("{kind:?} {step}: {}: {:?}", error.code, error.details));
    }

    // #463 S1-5 fix round 2: the step table loaded as the Lab loads it, `change` applied to each
    // completed application receipt, and written back as the Lab writes it; with `change` taking
    // out what a later build added, the step table as an earlier build wrote it.
    fn rewrite_receipts(&self, change: impl Fn(&mut ApplicationExecution)) {
        let state = self.state();
        let header = read_old_record(&state, INSTANCE)
            .expect("the session header")
            .expect("a session header");
        let LabFile::Ready(mut recording) = load_lab(&state, &header).expect("the step table")
        else {
            panic!("the step table does not load");
        };
        for step in &mut recording.steps {
            if let Some(executed) = step
                .application
                .as_mut()
                .and_then(|application| application.executed.as_mut())
            {
                change(executed);
            }
        }
        save_recording(&state, &recording).expect("the step table written");
    }

    fn recording_bytes(&self) -> Vec<u8> {
        fs::read(recording_path(&self.state(), RECORD_ID)).expect("step table")
    }

    fn recording(&self) -> Value {
        serde_json::from_slice(&self.recording_bytes()).expect("step table JSON")
    }

    // Every file of the recording's directory, by its path relative to it, with its bytes.
    fn files(&self) -> Vec<(String, Vec<u8>)> {
        files_under(&lab_dir(&self.state(), RECORD_ID))
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

fn manifest_of(outcome: &crate::RecordArchiveOutcome) -> Value {
    serde_json::from_slice(&fs::read(&outcome.manifest.path).expect("manifest")).expect("JSON")
}

// #463 S1-5: a JSON object of `members`, built without the `json!` macro's recursion.
fn object<const N: usize>(members: [(&str, Value); N]) -> Value {
    Value::Object(
        members
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect(),
    )
}

// #463 S1-5: the frames of a step of the step table as its row of the manifest (v2) lists them.
fn expected_frames(step: &Value) -> Value {
    Value::Array(
        step["frames"]
            .as_array()
            .expect("frames")
            .iter()
            .map(|frame| {
                object([
                    ("frame_id", frame["frame_id"].clone()),
                    ("role", frame["role"].clone()),
                    (
                        "file",
                        json!(format!(
                            "frames/{}.png",
                            frame["sha256"].as_str().expect("sha256")
                        )),
                    ),
                    ("sha256", frame["sha256"].clone()),
                    ("superseded", frame["superseded"].clone()),
                    ("recorded_at_unix_ms", frame["recorded_at_unix_ms"].clone()),
                ])
            })
            .collect(),
    )
}

// #463 S1-5: the row the manifest (v2) gives a step of the step table, as the contract states it,
// the frames of its effect given: `before_frame`, `after_frame` (`{step, frame_id}` or null) and
// whether the two were derived.
fn expected_row(step: &Value, kind: &str, effect_frames: [Value; 3]) -> Value {
    let [before_frame, after_frame, derived] = effect_frames;
    let click = &step["click"];
    let execution = &click["execution"];
    let gesture = &click["gesture"];
    let application = &step["application"];
    let receipt = &application["executed"];
    let input = |summary: &Value| {
        if summary.is_null() {
            Value::Null
        } else {
            json!({"frame_id": summary["frame_id"], "sha256": summary["frame_sha256"]})
        }
    };
    let rect = if click["source"] == "point" {
        Value::Null
    } else {
        click["rect"].clone()
    };
    let outcome = if !execution.is_null() {
        execution["effect"].clone()
    } else if !receipt.is_null() {
        json!("performed")
    } else if application["attempts"]
        .as_array()
        .is_some_and(|attempts| !attempts.is_empty())
    {
        json!("indeterminate")
    } else {
        Value::Null
    };
    let at_unix_ms = if !execution.is_null() {
        execution["executed_at_unix_ms"].clone()
    } else if !outcome.is_null() {
        receipt["executed_at_unix_ms"].clone()
    } else {
        step["frames"][0]["recorded_at_unix_ms"].clone()
    };
    let after_capture = if execution.is_null() {
        receipt["after_capture"].clone()
    } else {
        execution["after_capture"].clone()
    };
    let request_id = if execution["req_id"].is_null() {
        receipt["request_id"].clone()
    } else {
        execution["req_id"].clone()
    };
    let tap = if kind == "tap" {
        json!({"rect": rect, "point": execution["point"], "source": click["source"]})
    } else {
        Value::Null
    };
    let swipe = if kind == "swipe" {
        json!({
            "from_rect": click["rect"],
            "to_rect": gesture["to"],
            "from": execution["point"],
            "to": execution["to_point"],
            "duration_ms": gesture["duration_ms"]
        })
    } else {
        Value::Null
    };
    let long_press = if kind == "long_press" {
        json!({"point": execution["point"], "duration_ms": gesture["duration_ms"], "rect": rect})
    } else {
        Value::Null
    };
    let application_op = if application.is_null() {
        Value::Null
    } else {
        json!({
            "action": application["action"],
            "cli_verb": application["cli_verb"],
            "executed": !receipt.is_null()
        })
    };
    object([
        ("step", step["index"].clone()),
        ("kind", json!(kind)),
        ("dropped", step["dropped"].clone()),
        (
            "converted_to_transition",
            step["converted_to_transition"].clone(),
        ),
        ("frames", expected_frames(step)),
        ("before_frame", before_frame),
        ("after_frame", after_frame),
        ("after_capture", after_capture),
        ("derived", derived),
        ("outcome", outcome),
        ("tap", tap),
        ("swipe", swipe),
        ("long_press", long_press),
        ("application_op", application_op),
        ("before", input(&execution["before"])),
        ("after", input(&execution["after"])),
        ("page", execution["before"]["page"].clone()),
        (
            "package",
            execution["carrier_package"]["package_label"].clone(),
        ),
        ("note", Value::Null),
        ("at_unix_ms", at_unix_ms),
        ("request_id", request_id),
    ])
}

// #463 S1-5: the frame `frame_id` of step `step` of the step table `recording`, as the manifest
// names it with its row: `{step, frame_id, file, sha256}`.
fn named_frame(recording: &Value, step: u64, frame_id: &str) -> Value {
    let frame = recording["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .find(|candidate| candidate["index"] == step)
        .expect("the step")["frames"]
        .as_array()
        .expect("frames")
        .iter()
        .find(|frame| frame["frame_id"] == frame_id)
        .expect("the frame");
    let sha256 = frame["sha256"].as_str().expect("sha256");
    json!({
        "step": step,
        "frame_id": frame_id,
        "file": format!("frames/{sha256}.png"),
        "sha256": sha256
    })
}

// #463 S1-5: a press of the manifest's material: its step and kind, its frame and whether it was
// derived, whether it opened the entry step, where it was pressed (`point`, `to`, `duration_ms`,
// `action`) and its outcome, `performed`. Fix round 2: `sent_on_unstored_capture` `null`, since
// the step table this build reads does not say it.
fn material_click(
    step: u64,
    kind: &str,
    frame: Value,
    derived: bool,
    entry: bool,
    pressed: [Value; 4],
) -> Value {
    let [point, to, duration_ms, action] = pressed;
    object([
        ("step", json!(step)),
        ("kind", json!(kind)),
        ("frame", frame),
        ("derived", json!(derived)),
        ("sent_on_unstored_capture", Value::Null),
        ("entry", json!(entry)),
        ("point", point),
        ("to", to),
        ("duration_ms", duration_ms),
        ("action", action),
        ("outcome", json!("performed")),
    ])
}

// #463 S1-5: the copy in `copy` holds `held` (the recording's files but its manifest, by name with
// their bytes, in name order) and the manifest `manifest`, which is the recording's manifest
// `recorded` saying `copy: {frames}`.
fn assert_copy(
    copy: &Path,
    held: &[(String, Vec<u8>)],
    manifest: &str,
    recorded: &Value,
    frames: &str,
) {
    let (manifests, others): (Vec<_>, Vec<_>) = files_under(copy)
        .into_iter()
        .partition(|(name, _)| name == manifest);
    assert_eq!(others, held);
    let [(_, bytes)] = manifests.as_slice() else {
        panic!("the copy holds one manifest {manifest}: {manifests:?}");
    };
    let mut expected = recorded.clone();
    expected["copy"] = json!({"frames": frames});
    assert_eq!(
        serde_json::from_slice::<Value>(bytes).expect("the copy's manifest"),
        expected
    );
}

// The files of `files` but `manifest`, in name order.
fn without(files: &[(String, Vec<u8>)], manifest: &str) -> Vec<(String, Vec<u8>)> {
    files
        .iter()
        .filter(|(name, _)| name != manifest)
        .cloned()
        .collect()
}

// The row a v1 manifest (Workflow #449 REC-ARCH, written by the builds before #463 S1-5) gives a
// step of the step table, as its contract stated it.
fn v1_row(step: &Value, kind: &str) -> Value {
    let execution = &step["click"]["execution"];
    let frames = step["frames"]
        .as_array()
        .expect("frames")
        .iter()
        .map(|frame| {
            json!({
                "frame_id": frame["frame_id"],
                "role": frame["role"],
                "file": format!("frames/{}.png", frame["sha256"].as_str().expect("sha256")),
                "sha256": frame["sha256"]
            })
        })
        .collect::<Vec<_>>();
    let input = |summary: &Value| {
        if summary.is_null() {
            Value::Null
        } else {
            json!({"frame_id": summary["frame_id"], "sha256": summary["frame_sha256"]})
        }
    };
    let click = &step["click"];
    json!({
        "step": step["index"],
        "kind": kind,
        "frames": frames,
        "before": input(&execution["before"]),
        "after": input(&execution["after"]),
        "page": execution["before"]["page"],
        "package": execution["carrier_package"]["package_label"],
        "tap": if kind == "tap" {
            json!({"rect": click["rect"], "point": execution["point"]})
        } else {
            Value::Null
        },
        "swipe": if kind == "swipe" {
            json!({
                "from_rect": click["rect"],
                "to_rect": click["gesture"]["to"],
                "from": execution["point"],
                "to": execution["to_point"],
                "duration_ms": click["gesture"]["duration_ms"]
            })
        } else {
            Value::Null
        },
        "at_unix_ms": if execution.is_null() {
            step["frames"][0]["recorded_at_unix_ms"].clone()
        } else {
            execution["executed_at_unix_ms"].clone()
        },
        "request_id": execution["req_id"]
    })
}

// Item 6: a swipe sent with `--record` on the open step that declares no effect is the step's
// effect, a drag between boxes of one pixel at its two points over its duration, recorded with
// its execution; `record stop` generates it as the `drag` click of the standard task schema.
#[test]
fn rec_arch_a_swipe_is_a_step_and_record_stop_generates_it_as_a_drag() {
    let bench = Bench::new();
    bench.observe("Lab-CLI", RED);
    let swiped = bench.commit_swipe("Lab-CLI", "request-swipe");
    assert!(swiped.step_closed);

    let recording = bench.recording();
    let click = &recording["steps"][0]["click"];
    assert_eq!(click["source"], "swipe");
    assert_eq!(click["rect"], json!(rect(9, 3, 1, 1)));
    assert_eq!(
        click["gesture"],
        json!({"kind": "drag", "to": rect(13, 6, 1, 1), "duration_ms": 300})
    );
    assert_eq!(click["execution"]["point"], json!({"x": 9, "y": 3}));
    assert_eq!(click["execution"]["to_point"], json!({"x": 13, "y": 6}));
    assert_eq!(click["execution"]["duration_ms"], 300);
    assert_eq!(click["execution"]["point_rule"], "explicit");
    assert_eq!(click["execution"]["req_id"], "request-swipe");

    bench.observe("Lab-CLI", BLUE);
    bench.mark(1, "first_color");
    bench.mark(2, "last_color");
    let outcome = record_stop(
        &bench.lock("Lab-CLI"),
        &RecordStopOptions::default(),
        &bench.ocr,
    )
    .unwrap_or_else(|error| panic!("record stop: {}: {:?}", error.code, error.details));
    assert_eq!(outcome.lab_status.as_deref(), Some("generated"));
    let lab = serde_json::to_value(&outcome.lab).expect("lab JSON");
    let bytes = fs::read(lab["path"].as_str().expect("package path")).expect("package");
    let entries =
        expand_content_container(&bytes, ContentContainer::Json, ContainmentLimits::default())
            .expect("package container");
    let task: Value = serde_json::from_slice(
        entries
            .get(&format!("resources/operations/{TASK_ID}/task.json"))
            .expect("task.json"),
    )
    .expect("task JSON");
    assert_eq!(
        task["operations"][0]["click"],
        json!({
            "kind": "drag",
            "from": rect(9, 3, 1, 1),
            "to": rect(13, 6, 1, 1),
            "duration_ms": 300
        })
    );
}

// Items 2 and 3: steps a swipe and a tap of two initiators added, and a last step without input;
// the manifest is the step table's, nothing of the recording changes, and the copy into a
// directory that does not exist holds every file byte for byte.
#[test]
fn rec_arch_steps_of_several_callers_archive_and_the_rows_equal_the_step_table() {
    let bench = Bench::new();
    bench.observe("Lab-CLI", RED);
    bench.commit_swipe("Lab-CLI", "request-cli");
    bench.observe("Lab-MCP", GREEN);
    bench.commit_tap("Lab-MCP", "request-mcp");
    bench.observe("Lab-CLI", BLUE);
    let table = bench.recording_bytes();
    let before = bench.files();
    let recording = bench.recording();
    let initiators = recording["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .map(|step| step["initiator"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        initiators,
        [json!("Lab-CLI"), json!("Lab-MCP"), json!("Lab-CLI")]
    );

    let copy = bench.temp.path().join("copy").join("of-the-recording");
    let archived = bench
        .archive(&RecordArchiveOptions {
            record_id: None,
            copy_to: Some(copy.display().to_string()),
            written_by: WRITTEN_BY.to_owned(),
        })
        .unwrap_or_else(|error| panic!("archive: {}: {:?}", error.code, error.details));
    assert_eq!(archived.status, "archived");
    assert!(archived.ends_session);
    assert_eq!(archived.record_id, RECORD_ID);
    assert_eq!(archived.record_started_at_unix_ms, STARTED_AT);
    assert_eq!(archived.manifest.file, "archive-manifest-1.json");
    assert!(archived.manifest.written);
    assert_eq!(archived.manifest.steps, 3);

    let manifest = manifest_of(&archived);
    // #463 S1-5: the manifest is v2, with the step table's locale and coordinate space, no note,
    // and no copy (the recording's own directory).
    assert_eq!(
        manifest["schema_version"],
        "actingcommand.lab-recording-archive.v2"
    );
    assert_eq!(manifest["record_id"], RECORD_ID);
    assert_eq!(manifest["instance"], INSTANCE);
    assert_eq!(manifest["task_id"], TASK_ID);
    assert_eq!(
        manifest["application"],
        json!({"game": "neutral", "server": "test"})
    );
    assert_eq!(manifest["locale"], "en-US");
    assert_eq!(
        manifest["coordinate_space"],
        json!({"width": 16, "height": 9})
    );
    assert_eq!(manifest["note"], Value::Null);
    assert_eq!(manifest["archive_note"], Value::Null);
    assert_eq!(manifest["copy"], Value::Null);
    assert_eq!(manifest["started_at_unix_ms"], STARTED_AT);
    assert!(manifest["archived_at_unix_ms"].is_u64());
    assert_eq!(manifest["step_count"], 3);
    assert_eq!(manifest["written_by"], WRITTEN_BY);
    assert_eq!(
        manifest["step_table"],
        json!({"file": "recording.json", "sha256": hex_sha256(&table)})
    );
    let steps = recording["steps"].as_array().expect("steps");
    let rows = manifest["steps"].as_array().expect("rows");
    assert_eq!(rows.len(), steps.len());
    // #463 S1-5: presses of `do --capture --record`: their frames derived by the rule, the frame
    // before each the step's live primary frame, the frame after it the next step's first.
    let effect_frames = [
        [
            json!("f0001"),
            json!({"step": 2, "frame_id": "f0002"}),
            json!(true),
        ],
        [
            json!("f0002"),
            json!({"step": 3, "frame_id": "f0003"}),
            json!(true),
        ],
        [Value::Null, Value::Null, json!(false)],
    ];
    for ((row, step), (kind, frames)) in rows
        .iter()
        .zip(steps)
        .zip(["swipe", "tap", "none"].into_iter().zip(effect_frames))
    {
        assert_eq!(
            *row,
            expected_row(step, kind, frames),
            "step {}",
            step["index"]
        );
    }
    assert_eq!(
        rows[1]["tap"],
        json!({"rect": TAP, "point": {"x": 10, "y": 4}, "source": "tap_rect"})
    );
    assert_eq!(
        manifest["material"]["final_frame"],
        named_frame(&recording, 3, "f0003")
    );
    assert_eq!(
        rows[0]["swipe"],
        json!({
            "from_rect": rect(9, 3, 1, 1),
            "to_rect": rect(13, 6, 1, 1),
            "from": {"x": 9, "y": 3},
            "to": {"x": 13, "y": 6},
            "duration_ms": 300
        })
    );
    assert_eq!(rows[1]["request_id"], "request-mcp");
    assert_eq!(rows[1]["page"], "neutral_page");
    assert_eq!(rows[1]["package"], "neutral.package");
    assert_eq!(rows[2]["request_id"], Value::Null);

    // Nothing of the recording changed: its files are those before, and the manifest beside them.
    let after = bench.files();
    let manifest_bytes = fs::read(&archived.manifest.path).expect("manifest");
    let mut expected = before.clone();
    expected.push(("archive-manifest-1.json".to_owned(), manifest_bytes.clone()));
    expected.sort();
    assert_eq!(after, expected);
    assert_eq!(bench.recording_bytes(), table);

    // The copy into a directory that does not exist: the step table, the manifest and the frames
    // (#463 S1-5: the frames the material names, here every frame; the copy's manifest says so).
    let copied = record_archive_copy(
        &bench.lock("Lab-CLI"),
        &archived,
        &copy.display().to_string(),
        ArchiveCopyFrames::Material,
    )
    .unwrap_or_else(|error| panic!("copy: {}: {:?}", error.code, error.details));
    let frames = before
        .iter()
        .filter(|(name, _)| name.starts_with("frames/"))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(frames.len(), 3);
    let mut held = frames.clone();
    held.push(("recording.json".to_owned(), table.clone()));
    held.sort();
    assert_copy(
        &copy,
        &held,
        "archive-manifest-1.json",
        &serde_json::from_slice(&manifest_bytes).expect("manifest JSON"),
        "material",
    );
    let mut listed = copied.files.clone();
    listed.sort();
    let mut names = held
        .iter()
        .map(|(name, _)| name.clone())
        .chain(["archive-manifest-1.json".to_owned()])
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(listed, names);
}

// Item 2: a step table in the form release 0.12.54 wrote (the members the format has at the
// slice's base, a tap step and a step without input, no swipe) archives; its rows are its steps.
#[test]
fn rec_arch_a_step_table_written_by_0_12_54_archives() {
    let temp = TempDir::new().expect("temporary directory");
    let state = temp.path().join("state");
    let dir = lab_dir(&state, RECORD_ID);
    fs::create_dir_all(dir.join("frames")).expect("frames directory");
    fs::write(
        old_record_path(&state, INSTANCE),
        serde_json::to_vec(&json!({
            "schema_version": "session-record-v0",
            "record_id": RECORD_ID,
            "task_id": TASK_ID,
            "instance": INSTANCE,
            "status": "active",
            "holder": null,
            "lease_id": null,
            "started_at_unix_ms": STARTED_AT,
            "updated_at_unix_ms": STARTED_AT,
            "steps": []
        }))
        .expect("session JSON"),
    )
    .expect("session file");
    let frame = |color: [u8; 3], id: &str, at: u64| {
        let png = frame_png(color);
        let sha256 = hex_sha256(&png);
        let path = dir.join("frames").join(format!("{sha256}.png"));
        fs::write(&path, &png).expect("frame file");
        json!({
            "frame_id": id, "role": "primary", "path": path.display().to_string(),
            "sha256": sha256, "width": 16, "height": 9, "byte_count": png.len(),
            "source": "runtime_observation", "runtime_artifact": {"frame_id": format!("runtime-{id}")},
            "capture_backend": "adb_screencap", "freshness": null, "recorded_at_unix_ms": at,
            "superseded": false
        })
    };
    let first = frame(RED, "f0001", STARTED_AT + 100);
    let second = frame(BLUE, "f0002", STARTED_AT + 300);
    // Fix 2: the step table is built from its parts, each its own `json!`, so that no literal
    // nests deeper than the macro's recursion limit allows (one literal of it all did not build).
    let before = json!({
        "frame_id": "before-0154", "frame_sha256": hex_sha256(b"before"),
        "page": "neutral_page", "informational": true
    });
    let after = json!({
        "frame_id": "after-0154", "frame_sha256": hex_sha256(b"after"),
        "page": null, "informational": true
    });
    let execution = json!({
        "point": {"x": 10, "y": 4}, "point_rule": "rect_center", "effect": "performed",
        "carrier_package": {"package_label": "neutral.package"},
        "req_id": "request-0154", "correlation_id": "correlation-0154",
        "action_id": "action-0154", "lease_id": "lease-0154", "failure": null,
        "before": before, "after": after, "executed_at_unix_ms": STARTED_AT + 200
    });
    let click = json!({
        "rect": TAP, "source": "tap_rect", "from": null, "retry": null,
        "declared_at_unix_ms": STARTED_AT + 200, "execution": execution,
        "attempts": [], "needs_review": false
    });
    let tapped = json!({
        "index": 1, "page": null, "dropped": false, "converted_to_transition": false,
        "frames": [first], "marks": [], "reused": [], "click": click, "click_guard": null,
        "transition": null, "closed": true, "closed_by": "click", "initiator": "Lab-MCP"
    });
    let last = json!({
        "index": 2, "page": null, "dropped": false, "converted_to_transition": false,
        "frames": [second], "marks": [], "reused": [], "click": null, "click_guard": null,
        "transition": null, "closed": false, "closed_by": null, "initiator": "Lab-CLI"
    });
    let defaults = json!({
        "template_threshold": 0.95, "color_max_distance": 20, "match_metric": "ccoeff_normed"
    });
    let table = json!({
        "schema_version": "actingcommand.lab-recording.v1",
        "record_id": RECORD_ID,
        "record_started_at_unix_ms": STARTED_AT,
        "task_id": TASK_ID,
        "instance": INSTANCE,
        "status": "active",
        "game": null, "server": null, "locale": null,
        "defaults": defaults,
        "coordinate_space": {"width": 16, "height": 9},
        "created_at_unix_ms": STARTED_AT,
        "updated_at_unix_ms": STARTED_AT + 300,
        "steps": [tapped, last],
        "artifact": null
    });
    let bytes = serde_json::to_vec_pretty(&table).expect("step table JSON");
    fs::write(recording_path(&state, RECORD_ID), &bytes).expect("step table");

    let lock = RecordingLock::acquire(&state, INSTANCE, "gate_449", "Lab-CLI").expect("lock");
    let archived = record_archive(
        &lock,
        &RecordArchiveOptions {
            written_by: WRITTEN_BY.to_owned(),
            ..RecordArchiveOptions::default()
        },
    )
    .unwrap_or_else(|error| panic!("archive: {}: {:?}", error.code, error.details));
    assert_eq!(archived.status, "archived");
    let manifest = manifest_of(&archived);
    assert_eq!(manifest["step_count"], 2);
    assert_eq!(
        manifest["application"],
        json!({"game": null, "server": null})
    );
    assert_eq!(manifest["step_table"]["sha256"], hex_sha256(&bytes));
    let steps = table["steps"].as_array().expect("steps");
    let rows = manifest["steps"].as_array().expect("rows");
    assert_eq!(rows.len(), 2);
    // #463 S1-5: v2 rows; the tap's frames derived by the rule (f0002 was recorded after it).
    assert_eq!(
        manifest["schema_version"],
        "actingcommand.lab-recording-archive.v2"
    );
    assert_eq!(
        rows[0],
        expected_row(
            &steps[0],
            "tap",
            [
                json!("f0001"),
                json!({"step": 2, "frame_id": "f0002"}),
                json!(true)
            ]
        )
    );
    assert_eq!(
        rows[1],
        expected_row(&steps[1], "none", [Value::Null, Value::Null, json!(false)])
    );
    assert_eq!(
        manifest["material"]["final_frame"],
        named_frame(&table, 2, "f0002")
    );
    assert_eq!(
        fs::read(recording_path(&state, RECORD_ID)).expect("step table"),
        bytes
    );
}

// Item 7: the answer of a recorded observation, tap and swipe names the recording it entered (its
// id and when it started) and the step the action became.
#[test]
fn rec_arch_a_recorded_frame_tap_and_swipe_name_their_recording_and_step() {
    let bench = Bench::new();
    let observed = bench.observe("Lab-CLI", RED);
    let tapped = bench.commit_tap("Lab-MCP", "request-tap");
    let second = bench.observe("Lab-MCP", GREEN);
    let swiped = bench.commit_swipe("Lab-CLI", "request-swipe");
    for (answer, step) in [
        (serde_json::to_value(&observed).expect("JSON"), 1),
        (serde_json::to_value(&tapped).expect("JSON"), 1),
        (serde_json::to_value(&second).expect("JSON"), 2),
        (serde_json::to_value(&swiped).expect("JSON"), 2),
    ] {
        assert_eq!(answer["record_id"], RECORD_ID, "{answer}");
        assert_eq!(answer["record_started_at_unix_ms"], STARTED_AT, "{answer}");
        assert_eq!(answer["step"], step, "{answer}");
    }
}

// Item 9: without a session, with a stopped one and with an id no recording has, nothing is
// archived: no active recording is `record_session_not_active`, an unknown id
// `record_not_archived` (state `missing`); no manifest is written.
#[test]
fn rec_arch_without_an_active_recording_or_with_an_unknown_id_nothing_is_archived() {
    let options = |record_id: Option<&str>| RecordArchiveOptions {
        record_id: record_id.map(str::to_owned),
        copy_to: None,
        written_by: WRITTEN_BY.to_owned(),
    };
    let empty = TempDir::new().expect("temporary directory");
    let lock = RecordingLock::acquire(empty.path(), INSTANCE, "gate_449", "Lab-CLI").expect("lock");
    let error = record_archive(&lock, &options(None)).expect_err("no session");
    assert_eq!(error.code, "record_session_not_active", "{}", error.message);
    drop(lock);

    let bench = Bench::new();
    bench.observe("Lab-CLI", RED);
    let before = bench.files();
    let error = bench
        .archive(&options(Some("record_none")))
        .expect_err("no such recording");
    assert_eq!(error.code, "record_not_archived", "{}", error.message);
    assert_eq!(
        error.details.expect("details"),
        json!({"record_id": "record_none", "state": "missing"})
    );
    bench.header("stopped");
    let error = bench.archive(&options(None)).expect_err("stopped");
    assert_eq!(error.code, "record_session_not_active", "{}", error.message);
    assert_eq!(bench.files(), before, "nothing was written");
}

// #463 S1-5: a business as stage one records it (an application launch on the empty recording, a
// tap, a long press, a swipe and a restart, each with the screens before and after it): each row
// names the frames the step table names for its effect, and the material is each press with the
// frame it happened on and where it was pressed, and the screen after the restart.
#[test]
fn s15_a_stage_one_recording_archives_its_presses_their_screens_and_the_final_screen() {
    let bench = Bench::new();
    bench.entry_launch();
    bench.pressed(&tap_at(3, 4), [Some(0), Some(1)]);
    bench.pressed(
        &RecordPressRequest {
            click: PlanClickRequest::default(),
            point_press: Some(PointPress::LongPress {
                point: RecordPoint { x: 8, y: 5 },
                duration_ms: 800,
            }),
            after: AfterCapture::for_press(None),
        },
        [Some(2), Some(3)],
    );
    bench.pressed(
        &RecordPressRequest {
            click: PlanClickRequest {
                swipe: Some(PlannedSwipe {
                    from: RecordPoint { x: 2, y: 2 },
                    to: RecordPoint { x: 12, y: 7 },
                    duration_ms: 300,
                }),
                ..PlanClickRequest::default()
            },
            point_press: None,
            after: AfterCapture::for_press(None),
        },
        [Some(4), Some(5)],
    );
    bench.operated("restart", 6, 7);
    let recording = bench.recording();

    let archived = bench.archived(None);
    assert_eq!(archived.status, "archived");
    assert!(
        record_archive_warnings(&archived)
            .expect("warnings")
            .is_empty()
    );
    let manifest = manifest_of(&archived);
    assert_eq!(
        manifest["schema_version"],
        "actingcommand.lab-recording-archive.v2"
    );
    let steps = recording["steps"].as_array().expect("steps");
    let rows = manifest["steps"].as_array().expect("rows");
    assert_eq!(rows.len(), 6, "{manifest}");
    let after = |step: u64, frame_id: &str| json!({"step": step, "frame_id": frame_id});
    let expected = [
        ("application", [Value::Null, Value::Null, json!(false)]),
        ("tap", [json!("f0001"), after(3, "f0002"), json!(false)]),
        (
            "long_press",
            [json!("f0003"), after(4, "f0004"), json!(false)],
        ),
        ("swipe", [json!("f0005"), after(5, "f0006"), json!(false)]),
        (
            "application",
            [json!("f0007"), after(6, "f0008"), json!(false)],
        ),
        ("none", [Value::Null, Value::Null, json!(false)]),
    ];
    for ((row, step), (kind, frames)) in rows.iter().zip(steps).zip(expected) {
        assert_eq!(
            *row,
            expected_row(step, kind, frames),
            "step {}",
            step["index"]
        );
    }
    // The entry step: no frame, the launch performed at the time its receipt was recorded.
    assert_eq!(rows[0]["frames"], json!([]));
    assert_eq!(
        rows[0]["application_op"],
        json!({"action": "launch", "cli_verb": "launch", "executed": true})
    );
    assert_eq!(rows[0]["outcome"], "performed");
    assert!(rows[0]["at_unix_ms"].is_u64(), "{}", rows[0]);
    // A press declared by its point has no rectangle of its own.
    assert_eq!(
        rows[1]["tap"],
        json!({"rect": null, "point": {"x": 3, "y": 4}, "source": "point"})
    );
    assert_eq!(
        rows[2]["long_press"],
        json!({"point": {"x": 8, "y": 5}, "duration_ms": 800, "rect": null})
    );
    assert_eq!(rows[2]["tap"], Value::Null);
    assert_eq!(
        rows[3]["swipe"],
        json!({
            "from_rect": rect(2, 2, 1, 1),
            "to_rect": rect(12, 7, 1, 1),
            "from": {"x": 2, "y": 2},
            "to": {"x": 12, "y": 7},
            "duration_ms": 300
        })
    );
    // The capture before a press supersedes the screen after the one before it, which stays the
    // earlier press's `after_frame`: a step's frames in the order they were recorded.
    let listed = rows[2]["frames"]
        .as_array()
        .expect("frames")
        .iter()
        .map(|frame| (frame["frame_id"].clone(), frame["superseded"].clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        listed,
        [
            (json!("f0002"), json!(true)),
            (json!("f0003"), json!(false))
        ]
    );
    assert_eq!(rows[1]["after_capture"]["after_ms"], 800);
    assert_eq!(rows[4]["after_capture"]["after_ms"], 2000);
    assert!(rows[4]["at_unix_ms"].is_u64(), "{}", rows[4]);

    // What stage one hands over.
    let frame = |step: u64, frame_id: &str| named_frame(&recording, step, frame_id);
    let point = |x: i32, y: i32| json!({"x": x, "y": y});
    assert_eq!(
        manifest["material"],
        object([
            (
                "clicks",
                Value::Array(vec![
                    material_click(
                        1,
                        "application",
                        Value::Null,
                        false,
                        true,
                        [Value::Null, Value::Null, Value::Null, json!("launch")],
                    ),
                    material_click(
                        2,
                        "tap",
                        frame(2, "f0001"),
                        false,
                        false,
                        [point(3, 4), Value::Null, Value::Null, Value::Null],
                    ),
                    material_click(
                        3,
                        "long_press",
                        frame(3, "f0003"),
                        false,
                        false,
                        [point(8, 5), Value::Null, json!(800), Value::Null],
                    ),
                    material_click(
                        4,
                        "swipe",
                        frame(4, "f0005"),
                        false,
                        false,
                        [point(2, 2), point(12, 7), json!(300), Value::Null],
                    ),
                    material_click(
                        5,
                        "application",
                        frame(5, "f0007"),
                        false,
                        false,
                        [Value::Null, Value::Null, Value::Null, json!("restart")],
                    ),
                ]),
            ),
            ("final_frame", frame(6, "f0008")),
        ])
    );
}

// #463 S1-5: presses of `do --capture --record`, whose step table names no frame of them: the rows
// give the rule's, `derived: true`, the frame before each the step's live primary frame (not its
// first) and the frame after it the next effective step's first frame, none after the last; the
// last step has an effect, so there is no final screen and the archive warns.
#[test]
fn s15_a_recording_made_with_do_archives_with_derived_frames() {
    let bench = Bench::new();
    bench.observe("Lab-CLI", RED);
    bench.observe("Lab-CLI", GREEN);
    bench.commit_tap("Lab-CLI", "request-tap");
    bench.observe("Lab-CLI", BLUE);
    bench.commit_swipe("Lab-CLI", "request-swipe");
    let recording = bench.recording();

    let archived = bench.archived(None);
    assert_eq!(archived.status, "archived");
    assert_eq!(
        record_archive_warnings(&archived).expect("warnings"),
        ["final_frame_missing"]
    );
    let manifest = manifest_of(&archived);
    let steps = recording["steps"].as_array().expect("steps");
    let rows = manifest["steps"].as_array().expect("rows");
    assert_eq!(rows.len(), 2, "{manifest}");
    assert_eq!(
        rows[0],
        expected_row(
            &steps[0],
            "tap",
            [
                json!("f0002"),
                json!({"step": 2, "frame_id": "f0003"}),
                json!(true)
            ]
        )
    );
    assert_eq!(rows[0]["frames"][0]["frame_id"], "f0001");
    assert_eq!(rows[0]["frames"][0]["superseded"], true);
    assert_eq!(
        rows[1],
        expected_row(
            &steps[1],
            "swipe",
            [json!("f0003"), Value::Null, json!(true)]
        )
    );
    assert_eq!(
        manifest["material"],
        object([
            (
                "clicks",
                Value::Array(vec![
                    material_click(
                        1,
                        "tap",
                        named_frame(&recording, 1, "f0002"),
                        true,
                        false,
                        [
                            json!({"x": 10, "y": 4}),
                            Value::Null,
                            Value::Null,
                            Value::Null
                        ],
                    ),
                    material_click(
                        2,
                        "swipe",
                        named_frame(&recording, 2, "f0003"),
                        true,
                        false,
                        [
                            json!({"x": 9, "y": 3}),
                            json!({"x": 13, "y": 6}),
                            json!(300),
                            Value::Null
                        ],
                    ),
                ]),
            ),
            ("final_frame", Value::Null),
        ])
    );
}

// #463 S1-5: a v1 manifest an earlier build wrote, beside its recording's frames and step table:
// named again it is read and verified and warns nothing, copied it gives every file it lists and
// itself byte for byte, and its recording reopens; the next archive writes the next manifest as
// v2 and leaves the first as it was.
#[test]
fn s15_a_v1_manifest_on_disk_is_named_again_copied_as_it_is_and_reopened() {
    let bench = Bench::new();
    bench.observe("Lab-CLI", RED);
    bench.commit_tap("Lab-CLI", "request-v1");
    bench.observe("Lab-CLI", BLUE);
    let table = bench.recording_bytes();
    let recording = bench.recording();
    let steps = recording["steps"].as_array().expect("steps");
    let v1 = object([
        (
            "schema_version",
            json!("actingcommand.lab-recording-archive.v1"),
        ),
        ("record_id", json!(RECORD_ID)),
        ("instance", json!(INSTANCE)),
        ("task_id", json!(TASK_ID)),
        ("application", json!({"game": "neutral", "server": "test"})),
        ("started_at_unix_ms", json!(STARTED_AT)),
        ("archived_at_unix_ms", json!(STARTED_AT + 1)),
        ("step_count", json!(2)),
        ("written_by", json!(WRITTEN_BY)),
        (
            "step_table",
            json!({"file": "recording.json", "sha256": hex_sha256(&table)}),
        ),
        (
            "steps",
            Value::Array(vec![v1_row(&steps[0], "tap"), v1_row(&steps[1], "none")]),
        ),
    ]);
    let mut v1_bytes = serde_json::to_vec_pretty(&v1).expect("v1 manifest JSON");
    v1_bytes.push(b'\n');
    let dir = lab_dir(&bench.state(), RECORD_ID);
    fs::write(dir.join("archive-manifest-1.json"), &v1_bytes).expect("v1 manifest");
    bench.header("archived");
    let files = bench.files();

    // Named again: read, every file it lists hashed, nothing written, no warning.
    let again = bench.archived(Some(RECORD_ID));
    assert_eq!(again.status, "already_archived");
    assert_eq!(again.manifest.file, "archive-manifest-1.json");
    assert!(!again.manifest.written);
    assert_eq!(again.manifest.steps, 2);
    assert_eq!(again.manifest.sha256, hex_sha256(&v1_bytes));
    assert!(
        record_archive_warnings(&again)
            .expect("warnings")
            .is_empty()
    );
    assert_eq!(bench.files(), files);

    // Copied: it names no material, so every file it lists, and itself byte for byte.
    let copy = bench.temp.path().join("copy-of-v1");
    record_archive_copy(
        &bench.lock("Lab-CLI"),
        &again,
        &copy.display().to_string(),
        ArchiveCopyFrames::Material,
    )
    .unwrap_or_else(|error| panic!("copy: {}: {:?}", error.code, error.details));
    assert_eq!(files_under(&copy), files);

    // A frame it lists that no longer holds its sha256: named again, the check names it.
    let sha256 = steps[0]["frames"][0]["sha256"]
        .as_str()
        .expect("sha256")
        .to_owned();
    let frame = dir.join("frames").join(format!("{sha256}.png"));
    let original = fs::read(&frame).expect("frame");
    fs::write(&frame, b"not the recorded frame").expect("edited frame");
    let error = bench
        .archive(&RecordArchiveOptions {
            record_id: Some(RECORD_ID.to_owned()),
            copy_to: None,
            written_by: WRITTEN_BY.to_owned(),
        })
        .expect_err("a frame that does not hold its sha256");
    assert_eq!(
        error.code, "record_archive_verify_failed",
        "{}",
        error.message
    );
    let details = error.details.expect("details");
    assert_eq!(details["file"], format!("frames/{sha256}.png"));
    assert_eq!(details["checked"], "recording");
    fs::write(&frame, &original).expect("frame put back");

    // Reopened by its session header; a frame replaces the last step's; archived again.
    let reopened = record_reopen(&bench.lock("Lab-CLI"), &RecordReopenOptions::default())
        .unwrap_or_else(|error| panic!("reopen: {}: {:?}", error.code, error.details));
    assert_eq!(reopened.status, "reopened");
    assert_eq!(reopened.manifest, "archive-manifest-1.json");
    bench.header("active");
    bench.observe("Lab-CLI", GREEN);
    let next = bench.archived(None);
    assert_eq!(next.manifest.file, "archive-manifest-2.json");
    assert!(next.manifest.written);
    assert_eq!(
        fs::read(dir.join("archive-manifest-1.json")).expect("first manifest"),
        v1_bytes
    );
    let manifest = manifest_of(&next);
    assert_eq!(
        manifest["schema_version"],
        "actingcommand.lab-recording-archive.v2"
    );
    let now = bench.recording();
    let rows = manifest["steps"].as_array().expect("rows");
    // The tap's frame after is the first frame of step 2, since superseded: an `after_frame` may
    // name a superseded frame.
    assert_eq!(
        rows[0],
        expected_row(
            &now["steps"][0],
            "tap",
            [
                json!("f0001"),
                json!({"step": 2, "frame_id": "f0002"}),
                json!(true)
            ]
        )
    );
    assert_eq!(rows[1]["frames"][0]["frame_id"], "f0002");
    assert_eq!(rows[1]["frames"][0]["superseded"], true);
    assert_eq!(
        manifest["material"]["final_frame"],
        named_frame(&now, 2, "f0003")
    );
}

// #463 S1-5: two taps whose screens after cannot be taken. The capture before the second is the
// first's `after_frame`, noted without `after_capture`; after the second nothing arrived, so the
// archive stands without a final screen and warns.
#[test]
fn s15_without_a_screen_after_the_last_press_there_is_no_final_frame() {
    let bench = Bench::new();
    for (request, screen) in [(tap_at(3, 4), 0), (tap_at(5, 6), 1)] {
        let error = bench
            .press_recorded(&request, [Some(screen), None])
            .expect_err("the screen after cannot be taken");
        assert_eq!(
            error
                .details
                .as_ref()
                .map(|details| details["after"]["taken"].clone()),
            Some(json!(false)),
            "{:?}",
            error.details
        );
    }
    let recording = bench.recording();

    let archived = bench.archived(None);
    assert_eq!(archived.status, "archived");
    assert_eq!(
        record_archive_warnings(&archived).expect("warnings"),
        ["final_frame_missing"]
    );
    let manifest = manifest_of(&archived);
    let rows = manifest["steps"].as_array().expect("rows");
    assert_eq!(rows.len(), 2, "{manifest}");
    assert_eq!(rows[0]["before_frame"], "f0001");
    assert_eq!(
        rows[0]["after_frame"],
        json!({"step": 2, "frame_id": "f0002"})
    );
    assert_eq!(rows[0]["after_capture"], Value::Null);
    assert_eq!(rows[0]["derived"], false);
    assert_eq!(rows[1]["before_frame"], "f0002");
    assert_eq!(rows[1]["after_frame"], Value::Null);
    assert_eq!(rows[1]["after_capture"], Value::Null);
    assert_eq!(
        manifest["material"],
        object([
            (
                "clicks",
                Value::Array(vec![
                    material_click(
                        1,
                        "tap",
                        named_frame(&recording, 1, "f0001"),
                        false,
                        false,
                        [
                            json!({"x": 3, "y": 4}),
                            Value::Null,
                            Value::Null,
                            Value::Null
                        ],
                    ),
                    material_click(
                        2,
                        "tap",
                        named_frame(&recording, 2, "f0002"),
                        false,
                        false,
                        [
                            json!({"x": 5, "y": 6}),
                            Value::Null,
                            Value::Null,
                            Value::Null
                        ],
                    ),
                ]),
            ),
            ("final_frame", Value::Null),
        ])
    );
}

// #463 S1-5: the copy holds the step table, the manifest and the frames the material names, not
// the frame the capture before the second press superseded, and its manifest says `material`;
// with every frame asked for, it holds that frame too and says `all`. A superseded frame is
// hashed as every frame the manifest lists: changed, the archive named again fails on it.
#[test]
fn s15_the_copy_holds_the_material_unless_every_frame_is_asked_for() {
    let bench = Bench::new();
    bench.pressed(&tap_at(3, 4), [Some(0), Some(1)]);
    bench.pressed(&tap_at(5, 6), [Some(2), Some(3)]);
    let recording = bench.recording();
    let replaced = &recording["steps"][1]["frames"][0];
    assert_eq!(replaced["frame_id"], "f0002", "{recording}");
    assert_eq!(replaced["superseded"], true, "{recording}");
    let name = format!("{}.png", replaced["sha256"].as_str().expect("sha256"));
    let superseded = format!("frames/{name}");

    let archived = bench.archived(None);
    let files = bench.files();
    let manifest = manifest_of(&archived);
    let listed = without(&files, "archive-manifest-1.json");
    let material = bench.temp.path().join("material-copy");
    let copied = record_archive_copy(
        &bench.lock("Lab-CLI"),
        &archived,
        &material.display().to_string(),
        ArchiveCopyFrames::Material,
    )
    .unwrap_or_else(|error| panic!("copy: {}: {:?}", error.code, error.details));
    let held = listed
        .iter()
        .filter(|(file, _)| *file != superseded)
        .cloned()
        .collect::<Vec<_>>();
    // The step table and the frames of the two presses and of the final screen.
    assert_eq!(held.len(), 4, "{held:?}");
    assert_copy(
        &material,
        &held,
        "archive-manifest-1.json",
        &manifest,
        "material",
    );
    assert_eq!(copied.files.len(), 5, "{:?}", copied.files);

    // Named again, with every frame asked for.
    bench.header("archived");
    let again = bench.archived(Some(RECORD_ID));
    assert_eq!(again.status, "already_archived");
    let every = bench.temp.path().join("every-frame-copy");
    record_archive_copy(
        &bench.lock("Lab-CLI"),
        &again,
        &every.display().to_string(),
        ArchiveCopyFrames::All,
    )
    .unwrap_or_else(|error| panic!("copy: {}: {:?}", error.code, error.details));
    assert_copy(&every, &listed, "archive-manifest-1.json", &manifest, "all");

    // The superseded frame changed: the archive named again names it.
    let path = lab_dir(&bench.state(), RECORD_ID).join("frames").join(name);
    fs::write(path, b"not the recorded frame").expect("edited frame");
    let error = bench
        .archive(&RecordArchiveOptions {
            record_id: Some(RECORD_ID.to_owned()),
            copy_to: None,
            written_by: WRITTEN_BY.to_owned(),
        })
        .expect_err("a superseded frame that does not hold its sha256");
    assert_eq!(
        error.code, "record_archive_verify_failed",
        "{}",
        error.message
    );
    assert_eq!(error.details.expect("details")["file"], superseded.as_str());
}

// #463 S1-5 fix round 2 (the review of #1763, a): the copy's manifest is written last. A copy that
// fails after its first frame (the copies' tests' seam: a stored frame that no longer holds its
// sha256, here the second frame the copy takes) keeps the step table and the frames it copied, and
// holds no manifest.
#[test]
fn s15_fix2_a_copy_that_fails_after_its_first_frame_leaves_no_manifest() {
    let bench = Bench::new();
    bench.pressed(&tap_at(3, 4), [Some(0), Some(1)]);
    bench.pressed(&tap_at(5, 6), [Some(2), Some(3)]);
    let archived = bench.archived(None);
    let manifest = manifest_of(&archived);
    // The frames the copy takes, in its order: each press's, then the final screen.
    let material = &manifest["material"];
    let taken = [
        &material["clicks"][0]["frame"],
        &material["clicks"][1]["frame"],
        &material["final_frame"],
    ]
    .map(|frame| frame["file"].as_str().expect("a frame file").to_owned());
    let second = taken[1].strip_prefix("frames/").expect("a frame file");
    fs::write(
        lab_dir(&bench.state(), RECORD_ID)
            .join("frames")
            .join(second),
        b"not the recorded frame",
    )
    .expect("edited frame");

    let target = bench.temp.path().join("failed-copy");
    let error = record_archive_copy(
        &bench.lock("Lab-CLI"),
        &archived,
        &target.display().to_string(),
        ArchiveCopyFrames::Material,
    )
    .expect_err("a frame that does not hold its sha256");
    assert_eq!(
        error.code, "record_archive_verify_failed",
        "{}",
        error.message
    );
    let details = error.details.expect("details");
    assert_eq!(details["checked"], "copy", "{details}");
    assert_eq!(details["file"], taken[1].as_str(), "{details}");
    assert_eq!(
        details["copied"],
        json!(["recording.json", taken[0], taken[1], taken[2]]),
        "{details}"
    );
    // What was copied stays; no manifest names a file the copy does not hold.
    let names = files_under(&target)
        .into_iter()
        .map(|(name, _)| name)
        .collect::<Vec<_>>();
    assert!(
        !names
            .iter()
            .any(|name| name.starts_with("archive-manifest-")),
        "{names:?}"
    );
    let mut held = taken.to_vec();
    held.push("recording.json".to_owned());
    held.sort();
    assert_eq!(names, held);
}

// #463 S1-5 fix round 2 (review d): an application operation recorded the old way, its completed
// receipt naming no frame and no time (as before #463 S1-3 and S1-5): its row's `before_frame` is
// the step's live primary frame by the rule and `derived: true`, its `after_frame` and
// `at_unix_ms` `null`; the material names that frame, derived, with the operation's action.
#[test]
fn s15_fix2_an_application_operation_recorded_the_old_way_is_derived() {
    let bench = Bench::new();
    bench.operated("restart", 0, 1);
    bench.rewrite_receipts(|executed| {
        executed.before_frame = None;
        executed.after_frame = None;
        executed.after_capture = None;
        executed.executed_at_unix_ms = None;
    });
    let recording = bench.recording();
    let steps = recording["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 2, "{recording}");
    let receipt = &steps[0]["application"]["executed"];
    assert!(receipt.is_object(), "{recording}");
    for member in [
        "before_frame",
        "after_frame",
        "after_capture",
        "executed_at_unix_ms",
    ] {
        assert!(receipt.get(member).is_none(), "{member}: {receipt}");
    }

    let archived = bench.archived(None);
    assert!(
        record_archive_warnings(&archived)
            .expect("warnings")
            .is_empty()
    );
    let manifest = manifest_of(&archived);
    let rows = manifest["steps"].as_array().expect("rows");
    assert_eq!(rows.len(), 2, "{manifest}");
    assert_eq!(
        rows[0],
        expected_row(
            &steps[0],
            "application",
            [json!("f0001"), Value::Null, json!(true)]
        )
    );
    assert_eq!(rows[0]["after_frame"], Value::Null);
    assert_eq!(rows[0]["at_unix_ms"], Value::Null);
    assert_eq!(
        manifest["material"],
        object([
            (
                "clicks",
                Value::Array(vec![material_click(
                    1,
                    "application",
                    named_frame(&recording, 1, "f0001"),
                    true,
                    false,
                    [Value::Null, Value::Null, Value::Null, json!("restart")],
                )]),
            ),
            ("final_frame", named_frame(&recording, 2, "f0002")),
        ])
    );
}

// #463 S1-5 fix round 2 (review d): a press whose outcome is indeterminate (`do --capture
// --record` with an indeterminate answer: recorded, its step left open) is in `material.clicks`
// with `outcome: "indeterminate"`, after the performed press before it; the last step has an
// effect, so there is no final screen.
#[test]
fn s15_fix2_an_indeterminate_press_is_in_the_material() {
    let bench = Bench::new();
    bench.observe("Lab-CLI", RED);
    bench.commit_tap("Lab-CLI", "request-tap");
    bench.observe("Lab-CLI", GREEN);
    let lock = bench.lock("Lab-CLI");
    let plan = record_plan_click(
        &lock,
        &PlanClickRequest {
            tap_rect: Some(TAP),
            ..PlanClickRequest::default()
        },
    )
    .unwrap_or_else(|error| panic!("plan: {}: {}", error.code, error.message));
    let error = record_commit_click(
        &lock,
        &plan,
        &CommitClickRequest {
            effect: ClickEffect::Indeterminate,
            ..performed("request-indeterminate")
        },
    )
    .expect_err("an indeterminate press");
    assert_eq!(
        error.code, "record_click_indeterminate",
        "{}",
        error.message
    );
    drop(lock);
    let recording = bench.recording();
    let steps = recording["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 2, "{recording}");
    assert_eq!(
        steps[1]["click"]["execution"]["effect"], "indeterminate",
        "{recording}"
    );

    let archived = bench.archived(None);
    assert_eq!(
        record_archive_warnings(&archived).expect("warnings"),
        ["final_frame_missing"]
    );
    let manifest = manifest_of(&archived);
    let rows = manifest["steps"].as_array().expect("rows");
    assert_eq!(rows.len(), 2, "{manifest}");
    assert_eq!(
        rows[1],
        expected_row(&steps[1], "tap", [json!("f0002"), Value::Null, json!(true)])
    );
    assert_eq!(rows[1]["outcome"], "indeterminate");
    let point = json!({"x": 10, "y": 4});
    let mut indeterminate = material_click(
        2,
        "tap",
        named_frame(&recording, 2, "f0002"),
        true,
        false,
        [point.clone(), Value::Null, Value::Null, Value::Null],
    );
    indeterminate["outcome"] = json!("indeterminate");
    assert_eq!(
        manifest["material"],
        object([
            (
                "clicks",
                Value::Array(vec![
                    material_click(
                        1,
                        "tap",
                        named_frame(&recording, 1, "f0001"),
                        true,
                        false,
                        [point, Value::Null, Value::Null, Value::Null],
                    ),
                    indeterminate,
                ]),
            ),
            ("final_frame", Value::Null),
        ])
    );
}

// #463 S1-5 fix round 2 (review d): a step converted into the transition of the step before it
// (`--to-transition`) and a dropped step that had a performed press are listed as rows, the
// converted step's frame in the row of the step it became the transition of, and neither is a
// press of the material; the final screen is the step after them.
#[test]
fn s15_fix2_dropped_and_converted_steps_are_listed_and_not_in_the_material() {
    let bench = Bench::new();
    bench.observe("Lab-CLI", RED);
    bench.commit_tap("Lab-CLI", "request-first");
    bench.observe("Lab-CLI", GREEN);
    bench.mark(2, "green_color");
    bench.step_action(StepActionKind::ToTransition, 2);
    bench.observe("Lab-CLI", BLUE);
    bench.commit_tap("Lab-CLI", "request-dropped");
    bench.step_action(StepActionKind::DropStep, 3);
    bench.observe("Lab-CLI", GRAY);
    let recording = bench.recording();
    let steps = recording["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 4, "{recording}");
    assert_eq!(steps[1]["converted_to_transition"], true, "{recording}");
    assert_eq!(steps[2]["dropped"], true, "{recording}");

    let archived = bench.archived(None);
    assert!(
        record_archive_warnings(&archived)
            .expect("warnings")
            .is_empty()
    );
    let manifest = manifest_of(&archived);
    let rows = manifest["steps"].as_array().expect("rows");
    assert_eq!(rows.len(), 4, "{manifest}");
    let first = rows[0]["frames"]
        .as_array()
        .expect("frames")
        .iter()
        .map(|frame| (frame["frame_id"].clone(), frame["role"].clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        first,
        [
            (json!("f0001"), json!("primary")),
            (json!("f0002"), json!("transition"))
        ]
    );
    assert_eq!(
        rows[1],
        expected_row(&steps[1], "none", [Value::Null, Value::Null, json!(false)])
    );
    assert_eq!(
        rows[2],
        expected_row(
            &steps[2],
            "tap",
            [
                json!("f0003"),
                json!({"step": 4, "frame_id": "f0004"}),
                json!(true)
            ]
        )
    );
    assert_eq!(rows[2]["outcome"], "performed");
    assert_eq!(
        manifest["material"],
        object([
            (
                "clicks",
                Value::Array(vec![material_click(
                    1,
                    "tap",
                    named_frame(&recording, 1, "f0001"),
                    true,
                    false,
                    [
                        json!({"x": 10, "y": 4}),
                        Value::Null,
                        Value::Null,
                        Value::Null
                    ],
                )]),
            ),
            ("final_frame", named_frame(&recording, 4, "f0004")),
        ])
    );
}

// #463 S1-5 fix round 2 (review d): a step table as the layer below (#463 S1-3) writes it, its
// completed application receipts without `executed_at_unix_ms`, loads and is written back byte for
// byte: the member stays absent where it is `None`.
#[test]
fn s15_fix2_a_step_table_without_executed_at_is_written_back_byte_for_byte() {
    let bench = Bench::new();
    bench.entry_launch();
    bench.operated("restart", 0, 1);
    let receipts = |recording: &Value| -> Vec<Value> {
        recording["steps"]
            .as_array()
            .expect("steps")
            .iter()
            .map(|step| step["application"]["executed"].clone())
            .filter(|executed| !executed.is_null())
            .collect()
    };
    let stamped = receipts(&bench.recording());
    assert_eq!(stamped.len(), 2, "{stamped:?}");
    assert!(
        stamped
            .iter()
            .all(|executed| executed["executed_at_unix_ms"].is_u64()),
        "{stamped:?}"
    );
    bench.rewrite_receipts(|executed| executed.executed_at_unix_ms = None);
    let below = bench.recording_bytes();
    let unstamped = receipts(&bench.recording());
    assert_eq!(unstamped.len(), 2, "{unstamped:?}");
    assert!(
        unstamped
            .iter()
            .all(|executed| executed.get("executed_at_unix_ms").is_none()),
        "{unstamped:?}"
    );

    // Loaded and written back as it is.
    bench.rewrite_receipts(|_| {});
    assert_eq!(bench.recording_bytes(), below);
}
