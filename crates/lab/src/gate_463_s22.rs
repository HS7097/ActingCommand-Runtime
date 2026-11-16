// SPDX-License-Identifier: AGPL-3.0-only

// #463 S2-2 (contracts/lab-recording.md, "Reviewing a recording step by step: `record review`";
// register §1.439): each step of a recording as the screen before, the press and the screen after,
// with previews drawn on copies of the stored frames. The recording is made as stage one makes it,
// with S2-5's bench (`gate_463_s25_draft`: `record_press` with a fake Runtime performing the input,
// its captures taking the scripted 160x90 screens of gray with a square of noise, the caller's
// clock moving only by its waits): a tap whose captures after it take two other screens before
// the settled one, a swipe, a long press, and the final screen.
// - `s22_three_steps_answer_their_before_press_and_after`: each step's before, press and after as
//   the step table holds them, the frames between the tap and its screen after, and the last step
//   with its screen and no press.
// - `s22_step_answers_that_step_alone`: `step` answers the one step as the whole answer has it,
//   and the count of the steps; a step that does not exist is `record_step_not_found`.
// - `s22_previews_are_written_under_lab_preview_and_nothing_recorded_changes`: the previews are the
//   files under `lab/preview/` the answer names, each of its frame's size, and every other file of
//   the recording keeps its bytes.
// - `s22_no_preview_writes_no_file`: every preview `null`, every file of the state directory as it
//   was.
// - `s22_a_preview_shows_the_press_and_the_declared_boxes`: the pixels of the previews where the
//   press is drawn (a tap's point, a swipe's start, a long press's ring) and on the outline of a
//   press box and of a recognize box, and on the screen after the next step's box; the stored
//   frame differs there, and a spot away from them is the stored frame's.
// - `s22_a_steps_missing_is_record_readys_items_for_that_step`.
// - `s22_an_archived_recording_is_reviewed_the_same`: after `record archive`, by the session
//   header and by its id.
// - `s22_a_frame_that_no_longer_matches_its_sha256_is_refused_and_no_preview_is_written`: the
//   refusal `record_frame_hash_mismatch`, no file under `lab/preview/`; the step whose frames are
//   intact is reviewed with its preview.
// No Runtime, no device; the tests read no clock and never sleep.

use crate::gate_463_s25_draft::{self as draft, Bench, INSTANCE, POINTS};
use crate::recording::frames::decode_frame;
use crate::recording::preview::{
    PRESS_BOX_COLOUR, PRESS_COLOUR, RECOGNIZE_BOX_COLOUR, press_radius,
};
use crate::recording::store::{hex_sha256, lab_dir, recording_path};
use crate::{
    AfterCapture, PlanClickRequest, PlannedSwipe, PointPress, RecordArchiveOptions, RecordPoint,
    RecordPressRequest, RecordReviewOutcome, RecordReviewRequest, RecordStopOptions,
    record_archive, record_ready, record_review,
};
use actingcommand_device::PixelFormat;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

// The swipe of step 2, on the second screen, from its button to the left.
const SWIPE_FROM: RecordPoint = RecordPoint { x: 78, y: 48 };
const SWIPE_TO: RecordPoint = RecordPoint { x: 20, y: 48 };
// The long press of step 3, on the third screen's button.
const LONG_PRESS: RecordPoint = RecordPoint { x: 128, y: 28 };

// The recording of the work order: step 1 a tap on screen 0, whose captures after it take screen 2,
// screen 3 and then screen 1 (one every 100 ms up to 200 ms after the first: the bound is reached
// and the screen was not still); step 2 a swipe on screen 1, step 3 a long press on screen 2, each
// with one capture 800 ms after it; step 4 the final screen 3, without a press.
fn three_presses() -> Bench {
    let bench = Bench::new();
    bench.press(
        &draft::tap_at(
            POINTS[0],
            AfterCapture {
                after_ms: 100,
                interval_ms: Some(100),
                still_ms: None,
                still_max_ms: 200,
            },
        ),
        vec![
            draft::screen(0),
            draft::screen(2),
            draft::screen(3),
            draft::screen(1),
        ],
    );
    bench.press(
        &RecordPressRequest {
            click: PlanClickRequest {
                swipe: Some(PlannedSwipe {
                    from: SWIPE_FROM,
                    to: SWIPE_TO,
                    duration_ms: 300,
                }),
                ..PlanClickRequest::default()
            },
            point_press: None,
            after: draft::one_capture_after(),
        },
        vec![draft::screen(1), draft::screen(2)],
    );
    bench.press(
        &RecordPressRequest {
            click: PlanClickRequest::default(),
            point_press: Some(PointPress::LongPress {
                point: LONG_PRESS,
                duration_ms: 800,
            }),
            after: draft::one_capture_after(),
        },
        vec![draft::screen(2), draft::screen(3)],
    );
    bench
}

// `record review` of `request` on the bench's recording, read as the command reads it: the
// instance's recording, `record ready`'s options without flags.
fn review(bench: &Bench, request: &RecordReviewRequest) -> RecordReviewOutcome {
    record_review(
        &bench.state(),
        INSTANCE,
        request,
        &RecordStopOptions::default(),
        &bench.ocr,
    )
    .unwrap_or_else(|error| {
        panic!(
            "record review: {}: {} {:?}",
            error.code, error.message, error.details
        )
    })
}

fn without_previews() -> RecordReviewRequest {
    RecordReviewRequest {
        no_preview: true,
        ..RecordReviewRequest::default()
    }
}

// The step table of the recording `record_id`.
fn step_table(bench: &Bench, record_id: &str) -> Value {
    serde_json::from_slice(
        &fs::read(recording_path(&bench.state(), record_id)).expect("step table"),
    )
    .expect("step table JSON")
}

fn step_of(table: &Value, serial: u64) -> &Value {
    table["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .find(|step| step["index"] == serial)
        .unwrap_or_else(|| panic!("step {serial}: {table}"))
}

// The sha256 the step table records for the frame `frame_id`.
fn recorded_sha(table: &Value, frame_id: &str) -> String {
    table["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .flat_map(|step| step["frames"].as_array().expect("frames").iter())
        .find(|frame| frame["frame_id"] == frame_id)
        .and_then(|frame| frame["sha256"].as_str())
        .unwrap_or_else(|| panic!("frame {frame_id}: {table}"))
        .to_owned()
}

// The sha256 of the file at `path`.
fn sha_at(path: &Value) -> String {
    hex_sha256(&fs::read(path.as_str().expect("a path")).expect("the file"))
}

// Every file under `dir`, by its `/`-separated path relative to it, with its bytes, in name order;
// the recording's lock file, which the bench's lock keeps open, by its name only.
fn files_under(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    let mut pending = vec![(dir.to_path_buf(), String::new())];
    while let Some((path, prefix)) = pending.pop() {
        for entry in fs::read_dir(&path).expect("directory") {
            let entry = entry.expect("entry");
            let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
            if entry.file_type().expect("file type").is_dir() {
                pending.push((entry.path(), format!("{name}/")));
            } else if entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "lock")
            {
                files.push((name, Vec::new()));
            } else {
                files.push((name, fs::read(entry.path()).expect("file")));
            }
        }
    }
    files.sort();
    files
}

// The decoded pixels of a PNG.
struct Image {
    width: u32,
    height: u32,
    channels: usize,
    pixels: Vec<u8>,
}

impl Image {
    fn of(png: Vec<u8>) -> Self {
        let loaded = decode_frame(png, "a PNG of the test").expect("a PNG");
        let frame = loaded.frame;
        let channels = match frame.pixel_format {
            PixelFormat::Rgb8 => 3,
            PixelFormat::Rgba8 => 4,
        };
        Self {
            width: frame.width,
            height: frame.height,
            channels,
            pixels: frame.pixels,
        }
    }

    fn read(path: &Value) -> Self {
        Self::of(fs::read(path.as_str().expect("a path")).expect("the PNG file"))
    }

    // The red, green and blue of the pixel at `x`, `y`.
    fn at(&self, x: i32, y: i32) -> [u8; 3] {
        let x = usize::try_from(x).expect("x");
        let y = usize::try_from(y).expect("y");
        let width = usize::try_from(self.width).expect("width");
        let start = (y * width + x) * self.channels;
        [
            self.pixels[start],
            self.pixels[start + 1],
            self.pixels[start + 2],
        ]
    }
}

// The work order's test of the answer: each step's before, press and after as the step table holds
// them. Step 1's press is the tap at its point (a press region of one pixel, no end, no duration),
// its screen after the last of its three captures (f0004, stored under step 2, the screen not still
// at the bound, waited from the press to that capture), and the two captures before that one
// between; step 2 the swipe with both ends and its duration; step 3 the long press with its
// duration; step 4 the final screen, its live frame, with no press and no screen after.
#[test]
fn s22_three_steps_answer_their_before_press_and_after() {
    let bench = three_presses();
    let answer = review(&bench, &without_previews());
    assert_eq!(answer.session, "active");
    assert_eq!(answer.steps_total, 4);
    let table = step_table(&bench, &answer.record_id);
    let json = serde_json::to_value(&answer).expect("answer JSON");
    let steps = json["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 4, "{json}");
    for (index, kind) in ["tap", "swipe", "long_press", "none"]
        .into_iter()
        .enumerate()
    {
        let serial = u64::try_from(index + 1).expect("serial");
        let step = &steps[index];
        assert_eq!(step["step"], serial, "{json}");
        assert_eq!(step["artifact_step"], serial, "{json}");
        assert_eq!(step["kind"], kind, "{json}");
        assert!(step["before"]["preview"].is_null(), "{json}");
        assert_eq!(
            sha_at(&step["before"]["path"]),
            recorded_sha(&table, step["before"]["frame_id"].as_str().expect("before")),
            "step {serial}: the path is the frame's file"
        );
        assert!(step.get("note").is_none(), "{json}");
        assert!(step.get("same_screen").is_none(), "{json}");
        if serial == 4 {
            continue;
        }
        // The press as the step table holds it.
        let click = &step_of(&table, serial)["click"];
        let execution = &click["execution"];
        assert_eq!(
            step["before"]["frame_id"], execution["before_frame"],
            "{json}"
        );
        let mut press = json!({
            "rect": click["rect"],
            "source": click["source"],
            "outcome": "executed",
            "point": execution["point"],
        });
        for member in ["to_point", "duration_ms"] {
            if !execution[member].is_null() {
                press[member] = execution[member].clone();
            }
        }
        if !click["gesture"].is_null() {
            press["gesture"] = click["gesture"].clone();
        }
        assert_eq!(step["press"], press, "step {serial}");
        // The screen after, as the execution names it.
        let after = &step["after"];
        assert_eq!(after["step"], serial + 1, "{json}");
        assert_eq!(after["frame_id"], execution["after_frame"], "{json}");
        assert!(after["preview"].is_null(), "{json}");
        assert_eq!(
            after["waited_ms"], execution["after_capture"]["waited_ms"],
            "{json}"
        );
        assert_eq!(
            after.get("still").cloned().unwrap_or(Value::Null),
            execution["after_capture"]["still"],
            "{json}"
        );
        assert_eq!(
            sha_at(&after["path"]),
            recorded_sha(&table, after["frame_id"].as_str().expect("after")),
            "step {serial}: the path is the frame's file"
        );
    }
    // The values themselves.
    let step_1 = &steps[0];
    assert_eq!(step_1["before"]["frame_id"], "f0001", "{json}");
    assert_eq!(
        step_1["press"]["point"],
        json!({"x": 28, "y": 28}),
        "{json}"
    );
    assert_eq!(
        step_1["press"]["rect"],
        json!({"x": 28, "y": 28, "width": 1, "height": 1})
    );
    assert!(step_1["press"].get("to_point").is_none(), "{json}");
    assert_eq!(step_1["after"]["frame_id"], "f0004", "{json}");
    assert_eq!(step_1["after"]["still"], json!(false), "{json}");
    let between = step_1["between"]
        .as_array()
        .expect("between")
        .iter()
        .map(|frame| frame["frame_id"].clone())
        .collect::<Vec<_>>();
    assert_eq!(between, [json!("f0002"), json!("f0003")], "{json}");
    for frame in step_1["between"].as_array().expect("between") {
        assert_eq!(
            sha_at(&frame["path"]),
            recorded_sha(&table, frame["frame_id"].as_str().expect("between")),
            "{json}"
        );
    }
    let step_2 = &steps[1];
    assert_eq!(step_2["before"]["frame_id"], "f0005", "{json}");
    assert_eq!(step_2["press"]["point"], json!(SWIPE_FROM), "{json}");
    assert_eq!(step_2["press"]["to_point"], json!(SWIPE_TO), "{json}");
    assert_eq!(step_2["press"]["duration_ms"], 300, "{json}");
    assert_eq!(step_2["press"]["source"], "swipe", "{json}");
    assert_eq!(step_2["after"]["frame_id"], "f0006", "{json}");
    assert_eq!(step_2["between"], json!([]), "{json}");
    let step_3 = &steps[2];
    assert_eq!(step_3["press"]["point"], json!(LONG_PRESS), "{json}");
    assert_eq!(step_3["press"]["duration_ms"], 800, "{json}");
    assert_eq!(
        step_3["press"]["gesture"],
        json!({"kind": "long_press", "duration_ms": 800})
    );
    assert_eq!(step_3["after"]["frame_id"], "f0008", "{json}");
    let step_4 = &steps[3];
    assert_eq!(step_4["before"]["frame_id"], "f0008", "{json}");
    assert!(step_4.get("press").is_none(), "{json}");
    assert!(step_4.get("after").is_none(), "{json}");
    assert_eq!(step_4["between"], json!([]), "{json}");
}

// The work order's test of `--step`: the one step, as the whole answer has it, and the count of
// every step; a step the recording does not have is `record_step_not_found`.
#[test]
fn s22_step_answers_that_step_alone() {
    let bench = three_presses();
    let whole = review(&bench, &without_previews());
    for serial in 1..=4_u32 {
        let one = review(
            &bench,
            &RecordReviewRequest {
                step: Some(serial),
                no_preview: true,
                ..RecordReviewRequest::default()
            },
        );
        assert_eq!(one.steps_total, 4);
        assert_eq!(one.steps.len(), 1, "step {serial}");
        let index = usize::try_from(serial - 1).expect("index");
        assert_eq!(one.steps[0], whole.steps[index], "step {serial}");
    }
    let refused = record_review(
        &bench.state(),
        INSTANCE,
        &RecordReviewRequest {
            step: Some(9),
            no_preview: true,
            ..RecordReviewRequest::default()
        },
        &RecordStopOptions::default(),
        &bench.ocr,
    )
    .expect_err("a step the recording does not have");
    assert_eq!(refused.code, "record_step_not_found", "{}", refused.message);
}

// The work order's test of the previews' files: under `lab/preview/`, named by the step and its
// role as the answer names them (each step's screen before, and the screen after of the three
// presses), each of its frame's size; every other file of the recording, the frames, the step
// table and the session's, keeps its bytes.
#[test]
fn s22_previews_are_written_under_lab_preview_and_nothing_recorded_changes() {
    let bench = three_presses();
    let state = bench.state();
    let before = files_under(&state);
    let answer = review(&bench, &RecordReviewRequest::default());
    let json = serde_json::to_value(&answer).expect("answer JSON");
    let preview_dir = lab_dir(&state, &answer.record_id).join("preview");
    let mut named = BTreeSet::new();
    for step in json["steps"].as_array().expect("steps") {
        for role in ["before", "after"] {
            if step.get(role).is_none() {
                continue;
            }
            let preview = step[role]["preview"].as_str().expect("a preview path");
            let file = PathBuf::from(preview);
            assert_eq!(
                file.parent(),
                Some(preview_dir.as_path()),
                "{preview} is under lab/preview"
            );
            let image = Image::read(&step[role]["preview"]);
            let frame = Image::read(&step[role]["path"]);
            assert_eq!((image.width, image.height), (frame.width, frame.height));
            named.insert(
                file.file_name()
                    .expect("a file name")
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }
    let expected = [
        "s01-after.png",
        "s01-before.png",
        "s02-after.png",
        "s02-before.png",
        "s03-after.png",
        "s03-before.png",
        "s04-before.png",
    ]
    .map(str::to_owned);
    assert_eq!(named, BTreeSet::from(expected.clone()));
    let written = files_under(&preview_dir)
        .into_iter()
        .map(|(name, _)| name)
        .collect::<BTreeSet<_>>();
    assert_eq!(written, BTreeSet::from(expected));
    let preview_prefix = format!("record-artifacts/{}/lab/preview/", answer.record_id);
    let others = files_under(&state)
        .into_iter()
        .filter(|(name, _)| !name.starts_with(&preview_prefix))
        .collect::<Vec<_>>();
    assert_eq!(others, before, "a recorded file changed");
}

// The work order's test of `--no-preview`: every preview `null`, and no file of the state
// directory is written or changed.
#[test]
fn s22_no_preview_writes_no_file() {
    let bench = three_presses();
    let before = files_under(&bench.state());
    let answer = review(&bench, &without_previews());
    for step in &answer.steps {
        assert!(
            step.before
                .as_ref()
                .is_some_and(|frame| frame.preview.is_none())
        );
        assert!(
            step.after
                .as_ref()
                .is_none_or(|after| after.preview.is_none())
        );
    }
    assert_eq!(files_under(&bench.state()), before, "a file was written");
}

// The work order's test of what a preview draws, by its pixels. Step 1 is marked with a page, a
// recognize box and a press box over its button, step 3 with a page and a recognize box over its
// button (as S2-5's test of the whole of stage two marks them). On step 1's screen before: the
// tap's point in the press's colour, the press box's corner in its colour (drawn over the
// recognize box's, the same rectangle); on step 2's: the swipe's start in the press's colour; on
// step 3's: the recognize box's corner in its colour and the long press's ring, its radius to the
// right of the point; on step 2's screen after (step 3's screen): step 3's recognize box; on step
// 1's screen after (step 2's, without a box): no press. The stored frame differs at each drawn
// pixel, and a pixel away from every drawing is the stored frame's.
#[test]
fn s22_a_preview_shows_the_press_and_the_declared_boxes() {
    let bench = three_presses();
    bench.marked(json!({"step": 1, "page": "first",
                        "boxes": [draft::button_box("recognize", 0), draft::button_box("press", 0)]}));
    bench.marked(json!({"step": 3, "page": "third",
                        "boxes": [draft::button_box("recognize", 2)]}));
    let answer = review(&bench, &RecordReviewRequest::default());
    let json = serde_json::to_value(&answer).expect("answer JSON");
    let steps = json["steps"].as_array().expect("steps");
    // The top-left pixel of button `index`, the first corner of a box drawn over it.
    let corner = |index: usize| {
        let boxed = draft::button_box("press", index);
        let first = &boxed["corners"][0];
        (
            i32::try_from(first["x"].as_i64().expect("x")).expect("x"),
            i32::try_from(first["y"].as_i64().expect("y")).expect("y"),
        )
    };
    // Each check: the step, the role, the pixel and the colour drawn there.
    let (tap_x, tap_y) = POINTS[0];
    let first_box = corner(0);
    let third_box = corner(2);
    let checks = [
        (0, "before", (tap_x, tap_y), PRESS_COLOUR),
        (0, "before", first_box, PRESS_BOX_COLOUR),
        (1, "before", (SWIPE_FROM.x, SWIPE_FROM.y), PRESS_COLOUR),
        (2, "before", third_box, RECOGNIZE_BOX_COLOUR),
        (1, "after", third_box, RECOGNIZE_BOX_COLOUR),
    ];
    for (index, role, (x, y), colour) in checks {
        let side = &steps[index][role];
        let preview = Image::read(&side["preview"]);
        let frame = Image::read(&side["path"]);
        assert_eq!(
            preview.at(x, y),
            colour,
            "step {} {role} at {x},{y}",
            index + 1
        );
        assert_ne!(frame.at(x, y), colour, "the stored frame at {x},{y}");
        // Away from every drawing, the preview is its frame.
        assert_eq!(
            preview.at(150, 85),
            frame.at(150, 85),
            "step {} {role}",
            index + 1
        );
    }
    // The long press's ring, its radius to the right of its point.
    let side = &steps[2]["before"];
    let preview = Image::read(&side["preview"]);
    let frame = Image::read(&side["path"]);
    let radius = press_radius(preview.width, preview.height);
    let ring = (LONG_PRESS.x + radius, LONG_PRESS.y);
    assert_eq!(preview.at(ring.0, ring.1), PRESS_COLOUR);
    assert_ne!(frame.at(ring.0, ring.1), PRESS_COLOUR);
    // The screen after shows no press: at the swipe's start step 1's after copy is its frame.
    let after = &steps[0]["after"];
    assert_eq!(
        Image::read(&after["preview"]).at(SWIPE_FROM.x, SWIPE_FROM.y),
        Image::read(&after["path"]).at(SWIPE_FROM.x, SWIPE_FROM.y)
    );
    // The stored frames carry no box: each frame file is what the step table recorded.
    let table = step_table(&bench, &answer.record_id);
    for step in steps {
        assert_eq!(
            sha_at(&step["before"]["path"]),
            recorded_sha(&table, step["before"]["frame_id"].as_str().expect("before"))
        );
    }
}

// The work order's test of `missing`: each step's items are `record ready`'s items of that step,
// as `record ready` answers them for the same recording; step 1 marked, the others not.
#[test]
fn s22_a_steps_missing_is_record_readys_items_for_that_step() {
    let bench = three_presses();
    bench.marked(json!({"step": 1, "page": "first",
                        "boxes": [draft::button_box("recognize", 0)]}));
    let ready = record_ready(
        &bench.state(),
        INSTANCE,
        None,
        &RecordStopOptions::default(),
        &bench.ocr,
    )
    .unwrap_or_else(|error| panic!("record ready: {}: {}", error.code, error.message));
    let answer = review(&bench, &without_previews());
    assert_eq!(answer.steps.len(), 4);
    for step in &answer.steps {
        let items = ready
            .missing
            .iter()
            .filter(|item| item.step == Some(step.step))
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(step.missing, items, "step {}", step.step);
    }
    assert!(
        !answer.steps[1].missing.is_empty(),
        "step 2, unmarked, lacks something: {:?}",
        ready.missing
    );
}

// The work order's test of an archived recording: after `record archive` it is reviewed the same,
// by the session header and by its id; its session is `archived`.
#[test]
fn s22_an_archived_recording_is_reviewed_the_same() {
    let bench = three_presses();
    let active = review(&bench, &without_previews());
    record_archive(
        &bench.lock,
        &RecordArchiveOptions {
            record_id: None,
            copy_to: None,
            written_by: "gate_463_s22".to_owned(),
        },
    )
    .unwrap_or_else(|error| panic!("record archive: {}: {:?}", error.code, error.details));
    bench.header("archived");
    let by_header = review(&bench, &without_previews());
    let by_id = review(
        &bench,
        &RecordReviewRequest {
            record_id: Some(active.record_id.clone()),
            no_preview: true,
            ..RecordReviewRequest::default()
        },
    );
    assert_eq!(active.session, "active");
    for archived in [&by_header, &by_id] {
        assert_eq!(archived.session, "archived");
        assert_eq!(archived.record_id, active.record_id);
        assert_eq!(archived.steps_total, active.steps_total);
        assert_eq!(archived.steps, active.steps);
    }
    let previewed = review(&bench, &RecordReviewRequest::default());
    assert!(
        previewed.steps[0]
            .before
            .as_ref()
            .and_then(|frame| frame.preview.as_ref())
            .is_some_and(|preview| Path::new(preview).is_file())
    );
}

// The work order's test of a frame whose content no longer matches its sha256: the file of step
// 2's screen before (also step 1's screen after) holds another screen's bytes. The review is
// refused with the code a stored frame's read answers, and no preview is written; the frame's file
// is left as it is. Step 4, whose frame is intact, is reviewed with its preview alone.
#[test]
fn s22_a_frame_that_no_longer_matches_its_sha256_is_refused_and_no_preview_is_written() {
    let bench = three_presses();
    let answer = review(&bench, &without_previews());
    let before = answer.steps[1]
        .before
        .as_ref()
        .expect("step 2's screen before");
    let tampered = draft::screen(0);
    fs::write(&before.path, &tampered).expect("the frame's file");
    let preview_dir = lab_dir(&bench.state(), &answer.record_id).join("preview");
    let refused = record_review(
        &bench.state(),
        INSTANCE,
        &RecordReviewRequest::default(),
        &RecordStopOptions::default(),
        &bench.ocr,
    )
    .expect_err("a frame whose bytes are not its sha256");
    assert_eq!(
        refused.code, "record_frame_hash_mismatch",
        "{}",
        refused.message
    );
    assert!(!preview_dir.exists(), "a preview was written");
    assert_eq!(fs::read(&before.path).expect("the frame's file"), tampered);
    let step_4 = review(
        &bench,
        &RecordReviewRequest {
            step: Some(4),
            ..RecordReviewRequest::default()
        },
    );
    let written = files_under(&preview_dir)
        .into_iter()
        .map(|(name, _)| name)
        .collect::<Vec<_>>();
    assert_eq!(written, ["s04-before.png"]);
    assert!(step_4.steps[0].after.is_none());
}
