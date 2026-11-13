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
// Frames are synthetic (16x9, a coloured square on gray); no Runtime, no device, no clock read by
// the tests themselves; the session header is written as `record start` writes it.

use crate::recording::store::{hex_sha256, lab_dir, old_record_path, recording_path};
use crate::{
    AttachFrameOutcome, AttachFrameRequest, ClickEffect, CommitClickOutcome, CommitClickRequest,
    LAB_RECORD_MARK_SCHEMA, LabOcr, MarkFamily, MarkRequest, MarkSpec, OpaqueJson,
    PlanClickRequest, PlannedSwipe, RecordArchiveOptions, RecordPoint, RecordRect,
    RecordStartOptions, RecordStopOptions, RecordingLock, record_archive, record_archive_copy,
    record_attach_frame, record_commit_click, record_mark, record_plan_click, record_start,
    record_stop,
};
use actingcommand_contract::LabError;
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_pack_containment::{
    ContainmentLimits, ContentContainer, expand_content_container,
};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
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

// The row the manifest gives a step of the step table, as the contract states it.
fn expected_row(step: &Value, kind: &str) -> Value {
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
    assert_eq!(
        manifest["schema_version"],
        "actingcommand.lab-recording-archive.v1"
    );
    assert_eq!(manifest["record_id"], RECORD_ID);
    assert_eq!(manifest["instance"], INSTANCE);
    assert_eq!(manifest["task_id"], TASK_ID);
    assert_eq!(
        manifest["application"],
        json!({"game": "neutral", "server": "test"})
    );
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
    for ((row, step), kind) in rows.iter().zip(steps).zip(["swipe", "tap", "none"]) {
        assert_eq!(*row, expected_row(step, kind), "step {}", step["index"]);
    }
    assert_eq!(
        rows[1]["tap"],
        json!({"rect": TAP, "point": {"x": 10, "y": 4}})
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

    // The copy into a directory that does not exist: the step table, the manifest and the frames.
    let copied = record_archive_copy(
        &bench.lock("Lab-CLI"),
        &archived,
        &copy.display().to_string(),
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
    held.push(("archive-manifest-1.json".to_owned(), manifest_bytes));
    held.sort();
    assert_eq!(files_under(&copy), held);
    let mut listed = copied.files.clone();
    listed.sort();
    let mut names = held
        .iter()
        .map(|(name, _)| name.clone())
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
    assert_eq!(rows[0], expected_row(&steps[0], "tap"));
    assert_eq!(rows[1], expected_row(&steps[1], "none"));
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
