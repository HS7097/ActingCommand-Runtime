// SPDX-License-Identifier: AGPL-3.0-only

// #463 S2-13 (contracts/lab-recording.md, "The frames around a press: `record frames`"; register
// §1.426): `record frames` lists the stored frames just before and just after one recorded step's
// press, read from the step table; it takes no lock and writes nothing.
// - `s213_step_2_answers_its_press_frame_and_the_frames_on_each_side`: on a recording of three
//   presses as stage one stores them (each sent on a frame captured just before it, three or four
//   captures after each), step 2 answers its press (its time, its point, the frame it was sent on),
//   the last three frames captured before that frame and the frames captured after the press, in
//   the order captured and across the step boundaries, each with its path, sha256, capture time,
//   offset from the press and step, and how many each side holds; asked for five before and two
//   after, the frame of step 1 is the first before.
// - `s213_before_0_and_after_0_answer_only_the_press`: step 2 (a tap) and step 3 (a swipe, its two
//   ends) answer their press and no frame on either side; the totals stand.
// - `s213_a_number_above_100_is_refused`: `before` or `after` 101 is `validation_failed` with
//   `details{argument, reason: "range", value, min: 0, max: 100}`; 100 lists every frame there is.
// - `s213_a_step_without_a_press_answers_what_it_has`: the last step, which has no effect, answers
//   no time, no point and no frame; before it the frames stored under the steps below it, after it
//   its own frames, without offsets.
// - `s213_a_dropped_step_or_one_that_does_not_exist_is_refused`: `record_step_not_found`.
// - `s213_a_missing_frame_file_is_listed_without_a_path_and_warned`: two frames whose files are not
//   there are listed with no path and named, in the order listed, in the warning
//   `frame_file_missing`; every other frame is listed as before.
// - `s213_an_archived_and_a_stopped_recording_answer_as_the_active_one_did`: archived by `record
//   archive` (its session header archived, then another recording's) and then stopped, the
//   recording answers by its id, and by the session header that names it, what it answered active.
// - `s213_the_read_writes_nothing_and_answers_while_the_lock_is_held`: while another command holds
//   the recording's lock, reads answer and refusals are refused; every directory and file of the
//   state directory is as it was.
// The step table is written as stage one writes it; each frame's file holds made-up bytes, which
// the read never decodes. No Runtime, no device, no clock read by the tests themselves.

use crate::recording::store::{hex_sha256, lab_dir, old_record_path, recording_path};
use crate::{
    RecordArchiveOptions, RecordFramesFrame, RecordFramesOutcome, RecordFramesPress,
    RecordFramesRequest, RecordFramesWarning, RecordPoint, RecordingLock, record_archive,
    record_frames,
};
use actingcommand_contract::LabError;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_s213";
const OTHER_ID: &str = "record_s213_next";
const TASK_ID: &str = "neutral_task";
const STARTED_AT: u64 = 1_700_000_000_000;
const WRITTEN_BY: &str = "0123456789abcdef0123456789abcdef01234567";

/// One stored frame of the recording: its id, the step it is stored under, when it was captured
/// (milliseconds after the start) and whether a later frame of its step superseded it.
struct Shot {
    id: &'static str,
    step: u32,
    at: u64,
    superseded: bool,
}

const fn shot(id: &'static str, step: u32, at: u64, superseded: bool) -> Shot {
    Shot {
        id,
        step,
        at,
        superseded,
    }
}

/// Step 1: the frame press 1 (150) was sent on. Step 2: the four captures after press 1, then the
/// frame press 2 (1050) was sent on. Step 3: the four captures after press 2, then the frame press
/// 3 (2050) was sent on. Step 4: the three captures after press 3, the final screen.
const SHOTS: [Shot; 14] = [
    shot("f0001", 1, 100, false),
    shot("f0002", 2, 260, true),
    shot("f0003", 2, 360, true),
    shot("f0004", 2, 460, true),
    shot("f0005", 2, 560, true),
    shot("f0006", 2, 1_000, false),
    shot("f0007", 3, 1_160, true),
    shot("f0008", 3, 1_260, true),
    shot("f0009", 3, 1_360, true),
    shot("f0010", 3, 1_460, true),
    shot("f0011", 3, 2_000, false),
    shot("f0012", 4, 2_160, true),
    shot("f0013", 4, 2_260, true),
    shot("f0014", 4, 2_360, false),
];

/// The bytes of a frame's file: made up, never decoded.
fn file_bytes(id: &str) -> Vec<u8> {
    format!("the stored frame {id}").into_bytes()
}

/// The absolute path of a frame's stored file, as the answer gives it.
fn stored_path(state: &Path, id: &str) -> String {
    let file = format!("{}.png", hex_sha256(&file_bytes(id)));
    std::path::absolute(lab_dir(state, RECORD_ID).join("frames").join(file))
        .expect("an absolute path")
        .display()
        .to_string()
}

fn frame_entry(state: &Path, shot: &Shot) -> Value {
    let bytes = file_bytes(shot.id);
    let sha256 = hex_sha256(&bytes);
    let path = lab_dir(state, RECORD_ID)
        .join("frames")
        .join(format!("{sha256}.png"));
    json!({
        "frame_id": shot.id, "role": "primary", "path": path.display().to_string(),
        "sha256": sha256, "width": 16, "height": 9, "byte_count": bytes.len(),
        "source": "runtime_observation", "runtime_artifact": null,
        "capture_backend": "adb_screencap", "freshness": null,
        "recorded_at_unix_ms": STARTED_AT + shot.at, "superseded": shot.superseded
    })
}

/// A press recorded by the command that makes it: performed at `at`, sent on `before`, its screen
/// after `after`, the last of `captures` captures.
fn execution(point: Value, end: Value, at: u64, before: &str, after: &str, captures: u32) -> Value {
    let after_capture = json!({
        "after_ms": 100, "interval_ms": 100, "still_ms": 300, "still_max_ms": 5000,
        "still": true, "waited_ms": 400, "captures": captures, "kept": captures - 1
    });
    json!({
        "point": point, "point_rule": "explicit", "effect": "performed",
        "carrier_package": null, "req_id": null, "correlation_id": null, "action_id": null,
        "lease_id": null, "failure": null, "before": null, "after": null,
        "executed_at_unix_ms": STARTED_AT + at, "to_point": end,
        "before_frame": before, "after_frame": after, "after_capture": after_capture
    })
}

fn click(rect: Value, source: &str, gesture: Value, execution: Value) -> Value {
    json!({
        "rect": rect, "source": source, "from": null, "gesture": gesture, "retry": null,
        "declared_at_unix_ms": STARTED_AT, "execution": execution, "attempts": [],
        "needs_review": false
    })
}

/// Step `index` with its frames of [`SHOTS`] and `click` (none: a step without an effect, open).
fn step(state: &Path, index: u32, click: Value) -> Value {
    let frames = SHOTS
        .iter()
        .filter(|shot| shot.step == index)
        .map(|shot| frame_entry(state, shot))
        .collect::<Vec<_>>();
    let closed = !click.is_null();
    let closed_by = if closed { json!("click") } else { Value::Null };
    json!({
        "index": index, "page": null, "dropped": false, "converted_to_transition": false,
        "frames": frames, "marks": [], "reused": [], "click": click, "click_guard": null,
        "transition": null, "closed": closed, "closed_by": closed_by, "initiator": "Lab-CLI"
    })
}

/// The step table of three presses (two taps and a swipe) and a last step without an effect.
fn table(state: &Path) -> Value {
    let point = |x: i32, y: i32| json!({"x": x, "y": y});
    let pixel = |x: i32, y: i32| json!({"x": x, "y": y, "width": 1, "height": 1});
    let first = execution(point(10, 4), Value::Null, 150, "f0001", "f0005", 4);
    let second = execution(point(12, 5), Value::Null, 1_050, "f0006", "f0010", 4);
    let third = execution(point(2, 2), point(14, 2), 2_050, "f0011", "f0014", 3);
    let to = pixel(14, 2);
    let drag = json!({"kind": "drag", "to": to, "duration_ms": 300});
    let steps = vec![
        step(state, 1, click(pixel(10, 4), "point", Value::Null, first)),
        step(state, 2, click(pixel(12, 5), "point", Value::Null, second)),
        step(state, 3, click(pixel(2, 2), "swipe", drag, third)),
        step(state, 4, Value::Null),
    ];
    let defaults = json!({
        "template_threshold": 0.95, "color_max_distance": 20, "match_metric": "ccoeff_normed"
    });
    json!({
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
        "updated_at_unix_ms": STARTED_AT + 2_360,
        "steps": steps,
        "artifact": null
    })
}

fn write_table(state: &Path, table: &Value) {
    let bytes = serde_json::to_vec_pretty(table).expect("step table JSON");
    fs::write(recording_path(state, RECORD_ID), bytes).expect("step table");
}

/// The session header of the instance, naming `record_id` with `status`, as the `record` command
/// writes it.
fn header(state: &Path, record_id: &str, status: &str) {
    let header = json!({
        "schema_version": "session-record-v0", "record_id": record_id, "task_id": TASK_ID,
        "instance": INSTANCE, "status": status, "holder": null, "lease_id": null,
        "started_at_unix_ms": STARTED_AT, "updated_at_unix_ms": STARTED_AT, "steps": []
    });
    fs::write(
        old_record_path(state, INSTANCE),
        serde_json::to_vec(&header).expect("session JSON"),
    )
    .expect("session file");
}

struct Bench {
    temp: TempDir,
}

impl Bench {
    fn new() -> Self {
        Self {
            temp: TempDir::new().expect("temporary directory"),
        }
    }

    fn state(&self) -> PathBuf {
        self.temp.path().join("state")
    }

    fn table(&self) -> Value {
        table(&self.state())
    }

    /// The recording `table` with every frame's file but those of `missing`, and the session
    /// header naming it with `status`.
    fn write(&self, table: &Value, status: &str, missing: &[&str]) {
        let state = self.state();
        let frames = lab_dir(&state, RECORD_ID).join("frames");
        fs::create_dir_all(&frames).expect("frames directory");
        for shot in SHOTS.iter().filter(|shot| !missing.contains(&shot.id)) {
            let bytes = file_bytes(shot.id);
            let name = format!("{}.png", hex_sha256(&bytes));
            fs::write(frames.join(name), bytes).expect("frame file");
        }
        write_table(&state, table);
        header(&state, RECORD_ID, status);
    }

    fn read(&self, request: &RecordFramesRequest) -> RecordFramesOutcome {
        record_frames(&self.state(), INSTANCE, request).unwrap_or_else(|error| {
            panic!(
                "record frames: {}: {} {:?}",
                error.code, error.message, error.details
            )
        })
    }

    fn refused(&self, request: &RecordFramesRequest) -> LabError {
        record_frames(&self.state(), INSTANCE, request).expect_err("record frames refuses")
    }
}

/// `record frames --step <step>` with `--before` and `--after` as given.
fn asked(step: u32, before: Option<u64>, after: Option<u64>) -> RecordFramesRequest {
    RecordFramesRequest {
        record_id: None,
        step,
        before,
        after,
    }
}

/// The same, naming the recording by its id.
fn by_id(step: u32) -> RecordFramesRequest {
    RecordFramesRequest {
        record_id: Some(RECORD_ID.to_owned()),
        ..asked(step, None, None)
    }
}

/// Each frame of a side as (id, offset from the press, step).
fn sides(frames: &[RecordFramesFrame]) -> Vec<(&str, Option<i64>, u32)> {
    frames
        .iter()
        .map(|frame| {
            (
                frame.frame_id.as_str(),
                frame.offset_ms,
                frame.stored_under_step,
            )
        })
        .collect()
}

/// Expected frames of a side, each with its offset.
fn timed<'a>(rows: &[(&'a str, i64, u32)]) -> Vec<(&'a str, Option<i64>, u32)> {
    rows.iter()
        .map(|(id, offset, step)| (*id, Some(*offset), *step))
        .collect()
}

/// Expected frames of a side, without offsets.
fn untimed<'a>(rows: &[(&'a str, u32)]) -> Vec<(&'a str, Option<i64>, u32)> {
    rows.iter().map(|(id, step)| (*id, None, *step)).collect()
}

/// Every directory and file under `dir`, each file with its bytes; the lock file, which the lock's
/// holder keeps open, by its name only.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    let mut found = BTreeMap::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in fs::read_dir(next).expect("a directory of the state") {
            let path = entry.expect("an entry of the state").path();
            if path.is_dir() {
                found.insert(path.clone(), None);
                pending.push(path);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "lock")
            {
                found.insert(path, None);
            } else {
                let bytes = fs::read(&path).expect("a file of the state");
                found.insert(path, Some(bytes));
            }
        }
    }
    found
}

#[test]
fn s213_step_2_answers_its_press_frame_and_the_frames_on_each_side() {
    let bench = Bench::new();
    bench.write(&bench.table(), "active", &[]);
    let state = bench.state();
    let answered = bench.read(&asked(2, None, None));
    assert_eq!(answered.record_id, RECORD_ID);
    assert_eq!(
        (
            answered.step,
            answered.artifact_step,
            answered.kind.as_str()
        ),
        (2, Some(2), "tap")
    );
    assert_eq!(answered.press.at_unix_ms, Some(STARTED_AT + 1_050));
    assert_eq!(answered.press.point, Some(RecordPoint { x: 12, y: 5 }));
    assert_eq!(answered.press.to_point, None);
    let frame = answered
        .press
        .frame
        .as_ref()
        .expect("the frame the press was sent on");
    assert_eq!(frame.frame_id, "f0006");
    assert_eq!(
        frame.path.as_deref(),
        Some(stored_path(&state, "f0006").as_str())
    );
    assert_eq!(frame.sha256, hex_sha256(&file_bytes("f0006")));
    assert_eq!(frame.captured_at_unix_ms, STARTED_AT + 1_000);
    assert_eq!(frame.stored_under_step, 2);
    // The last three captured before the frame the press was sent on, the after loop of press 1.
    assert_eq!(
        sides(&answered.before),
        timed(&[("f0003", -690, 2), ("f0004", -590, 2), ("f0005", -490, 2)])
    );
    // Every frame captured after the press: the after loop of press 2, the frame press 3 was sent
    // on and the after loop of press 3, across the steps they are stored under.
    assert_eq!(
        sides(&answered.after),
        timed(&[
            ("f0007", 110, 3),
            ("f0008", 210, 3),
            ("f0009", 310, 3),
            ("f0010", 410, 3),
            ("f0011", 950, 3),
            ("f0012", 1_110, 4),
            ("f0013", 1_210, 4),
            ("f0014", 1_310, 4),
        ])
    );
    assert_eq!((answered.before_total, answered.after_total), (5, 8));
    assert!(answered.warnings.is_empty(), "{:?}", answered.warnings);
    for listed in answered.before.iter().chain(&answered.after) {
        let id = listed.frame_id.as_str();
        assert_eq!(
            listed.path.as_deref(),
            Some(stored_path(&state, id).as_str())
        );
        assert_eq!(listed.sha256, hex_sha256(&file_bytes(id)));
        let shot = SHOTS
            .iter()
            .find(|shot| shot.id == id)
            .expect("a frame of the step table");
        assert_eq!(listed.captured_at_unix_ms, STARTED_AT + shot.at, "{id}");
    }
    let shown = serde_json::to_value(&answered).expect("the answer's JSON");
    assert_eq!(shown["press"]["point"], json!({"x": 12, "y": 5}));
    assert!(shown["press"].get("to_point").is_none(), "{shown}");
    assert_eq!(shown["press"]["frame"]["frame_id"], "f0006");
    assert_eq!(shown["before"][0]["offset_ms"], -690);
    assert_eq!(shown["after"][7]["stored_under_step"], 4);

    // Five before and two after: the frame of step 1 is the first before.
    let wider = bench.read(&asked(2, Some(5), Some(2)));
    assert_eq!(
        sides(&wider.before),
        timed(&[
            ("f0001", -950, 1),
            ("f0002", -790, 2),
            ("f0003", -690, 2),
            ("f0004", -590, 2),
            ("f0005", -490, 2),
        ])
    );
    assert_eq!(
        sides(&wider.after),
        timed(&[("f0007", 110, 3), ("f0008", 210, 3)])
    );
    assert_eq!((wider.before_total, wider.after_total), (5, 8));
    assert_eq!(wider.press, answered.press);
}

#[test]
fn s213_before_0_and_after_0_answer_only_the_press() {
    let bench = Bench::new();
    bench.write(&bench.table(), "active", &[]);
    let whole = bench.read(&asked(2, None, None));
    let only = bench.read(&asked(2, Some(0), Some(0)));
    assert!(only.before.is_empty() && only.after.is_empty(), "{only:?}");
    assert_eq!(only.press, whole.press);
    assert_eq!((only.before_total, only.after_total), (5, 8));

    // The swipe: both of its ends, and the frame it was sent on.
    let swiped = bench.read(&asked(3, Some(0), Some(0)));
    assert_eq!(swiped.kind, "swipe");
    assert!(
        swiped.before.is_empty() && swiped.after.is_empty(),
        "{swiped:?}"
    );
    assert_eq!(swiped.press.at_unix_ms, Some(STARTED_AT + 2_050));
    assert_eq!(swiped.press.point, Some(RecordPoint { x: 2, y: 2 }));
    assert_eq!(swiped.press.to_point, Some(RecordPoint { x: 14, y: 2 }));
    assert_eq!(
        swiped
            .press
            .frame
            .as_ref()
            .map(|frame| (frame.frame_id.as_str(), frame.stored_under_step)),
        Some(("f0011", 3))
    );
    assert_eq!((swiped.before_total, swiped.after_total), (10, 3));
}

#[test]
fn s213_a_number_above_100_is_refused() {
    let bench = Bench::new();
    bench.write(&bench.table(), "active", &[]);
    for (before, after, argument) in [(Some(101), None, "before"), (None, Some(101), "after")] {
        let error = bench.refused(&asked(2, before, after));
        assert_eq!(error.code, "validation_failed", "{}", error.message);
        assert_eq!(
            error.details,
            Some(json!({
                "argument": argument, "reason": "range", "value": 101, "min": 0, "max": 100
            })),
            "{argument}"
        );
    }
    let most = bench.read(&asked(2, Some(100), Some(100)));
    assert_eq!((most.before.len(), most.after.len()), (5, 8));
}

#[test]
fn s213_a_step_without_a_press_answers_what_it_has() {
    let bench = Bench::new();
    bench.write(&bench.table(), "active", &[]);
    let last = bench.read(&asked(4, None, None));
    assert_eq!((last.kind.as_str(), last.artifact_step), ("none", Some(4)));
    assert_eq!(
        last.press,
        RecordFramesPress {
            at_unix_ms: None,
            point: None,
            to_point: None,
            frame: None,
        }
    );
    assert_eq!(
        serde_json::to_value(&last.press).expect("the press's JSON"),
        json!({"at_unix_ms": null, "frame": null})
    );
    assert_eq!(
        sides(&last.before),
        untimed(&[("f0009", 3), ("f0010", 3), ("f0011", 3)])
    );
    assert_eq!(
        sides(&last.after),
        untimed(&[("f0012", 4), ("f0013", 4), ("f0014", 4)])
    );
    assert_eq!((last.before_total, last.after_total), (11, 3));
    assert!(last.warnings.is_empty(), "{:?}", last.warnings);
}

#[test]
fn s213_a_dropped_step_or_one_that_does_not_exist_is_refused() {
    let bench = Bench::new();
    let mut table = bench.table();
    table["steps"][3]["dropped"] = json!(true);
    bench.write(&table, "active", &[]);
    for step in [4, 9] {
        let error = bench.refused(&asked(step, None, None));
        assert_eq!(
            error.code, "record_step_not_found",
            "{step}: {}",
            error.message
        );
    }
    // The steps that stand still answer.
    assert_eq!(bench.read(&asked(2, None, None)).after_total, 8);
}

#[test]
fn s213_a_missing_frame_file_is_listed_without_a_path_and_warned() {
    let bench = Bench::new();
    bench.write(&bench.table(), "active", &["f0004", "f0012"]);
    let answered = bench.read(&asked(2, None, None));
    let pathless = answered
        .before
        .iter()
        .chain(&answered.after)
        .filter(|frame| frame.path.is_none())
        .map(|frame| frame.frame_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(pathless, ["f0004", "f0012"]);
    assert_eq!((answered.before.len(), answered.after.len()), (3, 8));
    assert_eq!((answered.before_total, answered.after_total), (5, 8));
    assert!(
        answered
            .press
            .frame
            .as_ref()
            .is_some_and(|frame| frame.path.is_some()),
        "{:?}",
        answered.press
    );
    assert_eq!(
        answered.warnings,
        [RecordFramesWarning {
            code: "frame_file_missing",
            frame_ids: vec!["f0004".to_owned(), "f0012".to_owned()],
        }]
    );
    let shown = serde_json::to_value(&answered).expect("the answer's JSON");
    assert_eq!(shown["before"][1]["frame_id"], "f0004");
    assert_eq!(shown["before"][1]["path"], Value::Null);
    assert_eq!(
        shown["warnings"],
        json!([{"code": "frame_file_missing", "frame_ids": ["f0004", "f0012"]}])
    );
}

#[test]
fn s213_an_archived_and_a_stopped_recording_answer_as_the_active_one_did() {
    let bench = Bench::new();
    let table = bench.table();
    bench.write(&table, "active", &[]);
    let state = bench.state();
    let active = bench.read(&asked(2, None, None));

    // Archived as `record archive` leaves it: its manifest written, its session header archived.
    {
        let lock =
            RecordingLock::acquire(&state, INSTANCE, "gate_463_s213", "Lab-CLI").expect("lock");
        let archived = record_archive(
            &lock,
            &RecordArchiveOptions {
                written_by: WRITTEN_BY.to_owned(),
                ..RecordArchiveOptions::default()
            },
        )
        .unwrap_or_else(|error| panic!("archive: {}: {:?}", error.code, error.details));
        assert_eq!(archived.status, "archived");
    }
    header(&state, RECORD_ID, "archived");
    assert_eq!(bench.read(&asked(2, None, None)), active);
    assert_eq!(bench.read(&by_id(2)), active);
    // A later recording's session header: the archived one is read by its id.
    header(&state, OTHER_ID, "active");
    assert_eq!(bench.read(&by_id(2)), active);

    // Stopped.
    let mut stopped = table;
    stopped["status"] = json!("stopped");
    write_table(&state, &stopped);
    header(&state, RECORD_ID, "stopped");
    assert_eq!(bench.read(&asked(2, None, None)), active);
    assert_eq!(bench.read(&by_id(2)), active);
}

#[test]
fn s213_the_read_writes_nothing_and_answers_while_the_lock_is_held() {
    let bench = Bench::new();
    bench.write(&bench.table(), "active", &[]);
    let state = bench.state();
    let lock = RecordingLock::acquire(&state, INSTANCE, "record mark", "Lab-CLI")
        .expect("another command's lock");
    let before = snapshot(&state);
    assert!(
        before.len() > SHOTS.len(),
        "setup: the state holds the frames"
    );
    let answered = bench.read(&asked(2, None, None));
    assert_eq!(
        answered
            .press
            .frame
            .as_ref()
            .map(|frame| frame.frame_id.as_str()),
        Some("f0006")
    );
    let named = bench.read(&RecordFramesRequest {
        before: Some(0),
        after: Some(100),
        ..by_id(4)
    });
    assert_eq!((named.after.len(), named.after_total), (3, 3));
    assert_eq!(
        bench.refused(&asked(9, None, None)).code,
        "record_step_not_found"
    );
    assert_eq!(
        bench.refused(&asked(2, Some(101), None)).code,
        "validation_failed"
    );
    assert_eq!(snapshot(&state), before);
    drop(lock);
}
