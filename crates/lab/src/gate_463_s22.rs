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
// Fix round 1 (the independent review of #1786; coordinator work order
// rd-1116/brief-463-s22-fix1.md); the last test adapted (a frame's `path` may be `null` now):
// - `s22_fix1_an_application_without_a_completed_receipt_is_indeterminate_as_in_the_manifest`: an
//   application operation with attempts and no completed receipt answers `indeterminate`, its
//   manifest row's outcome.
// - `s22_fix1_a_press_and_a_box_at_the_frames_edges_are_drawn_inside_it`: a tap on the last pixel
//   with a box past that corner, a long press on the first pixel with a box of the whole frame:
//   drawn, each copy of the frame's size, nothing at the start of a row.
// - `s22_fix1_a_tap_has_a_cross_and_a_ring_and_a_long_press_its_ring_alone`: the cross at the tap's
//   point and none at the long press's; both rings.
// - `s22_fix1_a_marks_region_is_drawn_in_the_marks_colour`: a colour mark's region on the copy.
// - `s22_fix1_an_application_step_answers_its_receipts_screens`: kind, press, before and after of an
//   application operation with its completed receipt.
// - `s22_fix1_a_dropped_step_is_not_listed`: the dropped last step is neither listed nor counted,
//   and `step` naming it is `record_step_not_found`.
// - `s22_fix1_between_leaves_out_a_frame_of_another_step`: a frame between the two screens stored
//   under another step is not in `between`.
// - `s22_fix1_a_self_checks_item_is_on_its_step_when_the_locale_is_given`: a recording without a
//   locale of its own: no step's `missing` has an item; given the locale, the cross-check's refusal
//   is step 2's item, as `record ready` lists it.
// Fix round of the train of #463 stage two (register §1.440, "Fix round of the train"):
// - `s22_train_fix_a_frame_whose_file_is_gone_has_no_path_and_one_warning`: with `no_preview`, a
//   frame whose file is gone (step 3's screen after, step 4's screen before) answers `path` `null`
//   on both steps and the warning `frame_file_missing` names it once. Fix round of the 0.12.68
//   train: f0003, the same screen stored in the same content-addressed file, is gone with it and
//   named too.
// No Runtime, no device; the tests read no clock and never sleep.

use crate::gate_463_s25_draft::{self as draft, Bench, INSTANCE, POINTS};
use crate::recording::frames::decode_frame;
use crate::recording::preview::{
    Glyph, MARK_COLOUR, Overlay, PRESS_BOX_COLOUR, PRESS_COLOUR, RECOGNIZE_BOX_COLOUR,
    press_radius, render,
};
use crate::recording::store::{hex_sha256, lab_dir, old_record_path, recording_path};
use crate::{
    AfterCapture, PlanClickRequest, PlannedSwipe, PointPress, RecordArchiveOptions, RecordPoint,
    RecordPressRequest, RecordRect, RecordReviewOutcome, RecordReviewRequest, RecordStopOptions,
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
        .and_then(|frame| frame.path.as_deref())
        .expect("step 2's screen before and its file");
    let tampered = draft::screen(0);
    fs::write(before, &tampered).expect("the frame's file");
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
    assert_eq!(fs::read(before).expect("the frame's file"), tampered);
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

// Fix round 1: the captures after a tap that take two other screens before the settled one, one
// every 100 ms up to 200 ms after the first, as step 1 of `three_presses` takes them.
fn settling_after() -> AfterCapture {
    AfterCapture {
        after_ms: 100,
        interval_ms: Some(100),
        still_ms: None,
        still_max_ms: 200,
    }
}

// Fix round 1: the id of the bench's recording, as its session header names it.
fn record_id_of(bench: &Bench) -> String {
    let header: Value = serde_json::from_slice(
        &fs::read(old_record_path(&bench.state(), INSTANCE)).expect("session header"),
    )
    .expect("session header JSON");
    header["record_id"]
        .as_str()
        .expect("the header's record_id")
        .to_owned()
}

// Fix round 1: the step table of the recording `record_id` as `edit` leaves it, written in place.
fn edit_table(bench: &Bench, record_id: &str, edit: impl FnOnce(&mut Value)) {
    let mut table = step_table(bench, record_id);
    edit(&mut table);
    fs::write(
        recording_path(&bench.state(), record_id),
        serde_json::to_vec_pretty(&table).expect("step table JSON"),
    )
    .expect("step table");
}

// Fix round 1: step `serial` of a step table, to change it.
fn step_mut(table: &mut Value, serial: u64) -> &mut Value {
    table["steps"]
        .as_array_mut()
        .expect("steps")
        .iter_mut()
        .find(|step| step["index"] == serial)
        .unwrap_or_else(|| panic!("step {serial}"))
}

// Fix round 1, the review's change: an application operation with attempts and no completed receipt
// may have run, so it answers `indeterminate`, the judgement the archive's manifest gives its row,
// not `declared`. The last step is given such an operation in the step table, as `session app
// --record` leaves one whose receipt did not complete (an attempt, the step left open).
#[test]
fn s22_fix1_an_application_without_a_completed_receipt_is_indeterminate_as_in_the_manifest() {
    let bench = three_presses();
    let record_id = record_id_of(&bench);
    edit_table(&bench, &record_id, |table| {
        step_mut(table, 4)["application"] = json!({
            "action": "restart", "cli_verb": "restart", "source": "declared", "executed": null,
            "attempts": [{"receipt_state": "none", "runtime_code": null,
                          "request_id": "request-s22-attempt"}],
            "needs_review": false
        });
    });
    let json = serde_json::to_value(review(&bench, &without_previews())).expect("answer JSON");
    let last = &json["steps"][3];
    assert_eq!(last["step"], 4, "{json}");
    assert_eq!(last["kind"], "application", "{json}");
    assert_eq!(
        last["press"],
        json!({"application": "restart", "source": "declared", "outcome": "indeterminate"}),
        "{json}"
    );
    assert_eq!(last["before"]["frame_id"], "f0008", "{json}");
    assert!(last.get("after").is_none(), "{json}");
    // The archive's manifest gives the step's row the same judgement.
    let archived = record_archive(
        &bench.lock,
        &RecordArchiveOptions {
            record_id: None,
            copy_to: None,
            written_by: "gate_463_s22".to_owned(),
        },
    )
    .unwrap_or_else(|error| panic!("record archive: {}: {:?}", error.code, error.details));
    let manifest: Value =
        serde_json::from_slice(&fs::read(&archived.manifest.path).expect("manifest"))
            .expect("manifest JSON");
    let row = manifest["steps"]
        .as_array()
        .expect("the manifest's rows")
        .iter()
        .find(|row| row["step"] == 4)
        .unwrap_or_else(|| panic!("row 4: {manifest}"));
    assert_eq!(row["outcome"], "indeterminate", "{manifest}");
    assert_eq!(row["outcome"], last["press"]["outcome"], "{manifest}");
}

// Fix round 1 (the review's h): a press and a box at the frame's edges are drawn without a panic
// and inside the frame. On a stored screen of 160x90: a tap on its bottom-right pixel with a box
// that runs past that corner, then a long press on its top-left pixel with a box that is the whole
// frame. Each copy is of the frame's size; the press and the sides of each box that lie in the frame
// carry their colours; nothing comes out at the start of a row, where a pixel written past the right
// edge would land: the first columns are the frame's.
#[test]
fn s22_fix1_a_press_and_a_box_at_the_frames_edges_are_drawn_inside_it() {
    let png = draft::screen(0);
    let frame = Image::of(png.clone());
    let right = i32::try_from(frame.width).expect("width") - 1;
    let bottom = i32::try_from(frame.height).expect("height") - 1;
    let drawn = |overlay: &Overlay| {
        let copy = render(png.clone(), overlay, None, "the stored screen")
            .unwrap_or_else(|error| panic!("render: {}: {}", error.code, error.message));
        let image = Image::of(copy);
        assert_eq!((image.width, image.height), (frame.width, frame.height));
        image
    };
    // The bottom-right corner.
    let corner = drawn(&Overlay {
        boxes: vec![(
            RecordRect {
                x: right - 9,
                y: bottom - 9,
                width: 20,
                height: 20,
            },
            PRESS_BOX_COLOUR,
        )],
        glyph: Some(Glyph::Tap(RecordPoint {
            x: right,
            y: bottom,
        })),
    });
    assert_eq!(corner.at(right, bottom), PRESS_COLOUR, "the tap's point");
    assert_eq!(
        corner.at(right - 5, bottom - 9),
        PRESS_BOX_COLOUR,
        "the box's top side"
    );
    assert_eq!(
        corner.at(right - 9, bottom - 5),
        PRESS_BOX_COLOUR,
        "the box's left side"
    );
    for y in 0..=bottom {
        for x in 0..3 {
            assert_eq!(corner.at(x, y), frame.at(x, y), "{x},{y}");
        }
    }
    // The top-left corner, and a box of the whole frame.
    let whole = drawn(&Overlay {
        boxes: vec![(
            RecordRect {
                x: 0,
                y: 0,
                width: right + 1,
                height: bottom + 1,
            },
            RECOGNIZE_BOX_COLOUR,
        )],
        glyph: Some(Glyph::LongPress(RecordPoint { x: 0, y: 0 })),
    });
    for (x, y) in [(0, 0), (right, 0), (0, bottom), (right, bottom)] {
        assert_eq!(whole.at(x, y), RECOGNIZE_BOX_COLOUR, "the corner {x},{y}");
    }
    let radius = press_radius(frame.width, frame.height);
    assert_eq!(whole.at(radius, 0), PRESS_COLOUR, "the ring in the frame");
    assert_eq!(whole.at(0, radius), PRESS_COLOUR, "the ring in the frame");
    assert_eq!(
        whole.at(right / 2, bottom / 2),
        frame.at(right / 2, bottom / 2)
    );
}

// Fix round 1 (the review's h): a tap and a long press are drawn as the contract says: a cross and
// a ring at a tap's point, a ring alone at a long press's. On the previews of the three presses: at
// the tap's point, and half a radius to its left on the cross's arm, the press's colour; at the long
// press's point and half a radius to its left the copy is its frame; a radius to the right of each
// point is on its ring.
#[test]
fn s22_fix1_a_tap_has_a_cross_and_a_ring_and_a_long_press_its_ring_alone() {
    let bench = three_presses();
    let json =
        serde_json::to_value(review(&bench, &RecordReviewRequest::default())).expect("answer JSON");
    let steps = json["steps"].as_array().expect("steps");
    let (tap_x, tap_y) = POINTS[0];
    for (index, (x, y), crossed) in [
        (0, (tap_x, tap_y), true),
        (2, (LONG_PRESS.x, LONG_PRESS.y), false),
    ] {
        let side = &steps[index]["before"];
        let preview = Image::read(&side["preview"]);
        let frame = Image::read(&side["path"]);
        let radius = press_radius(preview.width, preview.height);
        assert_eq!(
            preview.at(x + radius, y),
            PRESS_COLOUR,
            "step {}: its ring",
            index + 1
        );
        for (at_x, at_y) in [(x, y), (x - radius / 2, y)] {
            let expected = if crossed {
                PRESS_COLOUR
            } else {
                frame.at(at_x, at_y)
            };
            assert_eq!(
                preview.at(at_x, at_y),
                expected,
                "step {} at {at_x},{at_y}",
                index + 1
            );
        }
        assert_ne!(frame.at(x, y), PRESS_COLOUR, "the stored frame at {x},{y}");
    }
}

// Fix round 1 (the review's h): the region of a mark that is neither a recognize box nor the press
// box is drawn in the marks' colour. Step 1 takes a colour mark on a flat part of its screen: its
// region's two opposite corners carry that colour on the copy and not on the stored frame, and the
// answer lists the mark.
#[test]
fn s22_fix1_a_marks_region_is_drawn_in_the_marks_colour() {
    let bench = three_presses();
    bench.marked(json!({"step": 1, "add": [{
        "id": "first_corner", "family": "color",
        "region": {"x": 100, "y": 60, "width": 16, "height": 16}
    }]}));
    let json =
        serde_json::to_value(review(&bench, &RecordReviewRequest::default())).expect("answer JSON");
    let first = &json["steps"][0];
    assert!(
        first["marks"]
            .as_array()
            .expect("marks")
            .iter()
            .any(|mark| mark["id"] == "first_corner"),
        "{json}"
    );
    let preview = Image::read(&first["before"]["preview"]);
    let frame = Image::read(&first["before"]["path"]);
    for (x, y) in [(100, 60), (115, 75)] {
        assert_eq!(preview.at(x, y), MARK_COLOUR, "{x},{y}");
        assert_ne!(frame.at(x, y), MARK_COLOUR, "the stored frame at {x},{y}");
    }
}

// Fix round 1 (the review's h): an application step. Step 2's swipe is replaced in the step table by
// an application operation with its completed receipt, as `session app --record` records one in the
// middle of a business: the screen it ran on (f0005), the last capture of its loop after it (f0006)
// and how that was taken. Its kind is `application`; its press the operation's action, its source
// and `executed`, without a point, a region or a gesture; its screens before and after the
// receipt's, with the stillness and the wait of the receipt's capture after; no frame between.
#[test]
fn s22_fix1_an_application_step_answers_its_receipts_screens() {
    let bench = three_presses();
    let record_id = record_id_of(&bench);
    let execution = step_of(&step_table(&bench, &record_id), 2)["click"]["execution"].clone();
    let capture = execution["after_capture"].clone();
    edit_table(&bench, &record_id, |table| {
        let second = step_mut(table, 2);
        second["click"] = Value::Null;
        second["closed_by"] = json!("application");
        second["application"] = json!({
            "action": "restart", "cli_verb": "restart", "source": "executed",
            "executed": {
                "request_id": "request-s22", "correlation_id": "correlation-s22",
                "action_id": "action-s22", "receipt_state": "completed",
                "application_event_ids": [], "before_frame": execution["before_frame"],
                "after_frame": execution["after_frame"],
                "after_capture": execution["after_capture"],
                "executed_at_unix_ms": execution["executed_at_unix_ms"]
            },
            "attempts": [], "needs_review": false
        });
    });
    let json = serde_json::to_value(review(&bench, &without_previews())).expect("answer JSON");
    let second = &json["steps"][1];
    assert_eq!(second["step"], 2, "{json}");
    assert_eq!(second["kind"], "application", "{json}");
    assert_eq!(
        second["press"],
        json!({"application": "restart", "source": "executed", "outcome": "executed"}),
        "{json}"
    );
    assert_eq!(second["before"]["frame_id"], "f0005", "{json}");
    let after = &second["after"];
    assert_eq!(after["step"], 3, "{json}");
    assert_eq!(after["frame_id"], "f0006", "{json}");
    assert_eq!(after["waited_ms"], capture["waited_ms"], "{json}");
    assert_eq!(
        after.get("still").cloned().unwrap_or(Value::Null),
        capture["still"],
        "{json}"
    );
    assert_eq!(second["between"], json!([]), "{json}");
}

// Fix round 1 (the review's h): a dropped step is not listed. The last step dropped with `record
// mark` (`drop_step`): the answer lists steps 1 to 3, numbered 1 to 3 in the package,
// `steps_total` 3, and `step` naming the dropped one is `record_step_not_found`; the step table
// still holds it.
#[test]
fn s22_fix1_a_dropped_step_is_not_listed() {
    let bench = three_presses();
    bench.marked(json!({"step_action": {"kind": "drop_step", "step": 4}}));
    let answer = review(&bench, &without_previews());
    assert_eq!(answer.steps_total, 3);
    assert_eq!(
        answer
            .steps
            .iter()
            .map(|step| (step.step, step.artifact_step))
            .collect::<Vec<_>>(),
        [(1, 1), (2, 2), (3, 3)]
    );
    let refused = record_review(
        &bench.state(),
        INSTANCE,
        &RecordReviewRequest {
            step: Some(4),
            no_preview: true,
            ..RecordReviewRequest::default()
        },
        &RecordStopOptions::default(),
        &bench.ocr,
    )
    .expect_err("a dropped step");
    assert_eq!(refused.code, "record_step_not_found", "{}", refused.message);
    let table = step_table(&bench, &answer.record_id);
    assert_eq!(step_of(&table, 4)["dropped"], true, "{table}");
}

// Fix round 1 (the review's h): `between` lists only the frames stored under the step the screen
// after opened. The tap's second capture after it (f0003) is moved in the step table from step 2 to
// step 1, its place in the order of arrival kept by its id: step 1's screens are as before and its
// `between` is f0002 alone.
#[test]
fn s22_fix1_between_leaves_out_a_frame_of_another_step() {
    let bench = three_presses();
    let record_id = record_id_of(&bench);
    edit_table(&bench, &record_id, |table| {
        let frames = step_mut(table, 2)["frames"]
            .as_array_mut()
            .expect("step 2's frames");
        let at = frames
            .iter()
            .position(|frame| frame["frame_id"] == "f0003")
            .expect("f0003 under step 2");
        let moved = frames.remove(at);
        step_mut(table, 1)["frames"]
            .as_array_mut()
            .expect("step 1's frames")
            .push(moved);
    });
    let answer = review(&bench, &without_previews());
    let first = &answer.steps[0];
    assert_eq!(
        first.before.as_ref().map(|frame| frame.frame_id.as_str()),
        Some("f0001")
    );
    assert_eq!(
        first.after.as_ref().map(|after| after.frame_id.as_str()),
        Some("f0004")
    );
    let between = first
        .between
        .iter()
        .map(|frame| frame.frame_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(between, ["f0002"]);
}

// Fix round 1 (the review's k): `record review` takes the locale `record ready` takes, so that a
// self-check's refusal reaches the step it names. Three taps (the first one's captures after it
// take two other screens before the settled one), every step boxed with a page and a recognize box
// (each tap a press box too), the recording's locale removed from the step table and step 2's
// capture of another screen (f0002) made a live sample of the step. Without a locale the whole list
// stops at it and no step's `missing` has an item; with `en-US` the self-checks run, the
// cross-check refuses step 2's page on that sample, and step 2's `missing` is that one item, as
// `record ready` with the same locale lists it; no other step has one.
#[test]
fn s22_fix1_a_self_checks_item_is_on_its_step_when_the_locale_is_given() {
    let bench = Bench::new();
    bench.press(
        &draft::tap_at(POINTS[0], settling_after()),
        vec![
            draft::screen(0),
            draft::screen(2),
            draft::screen(3),
            draft::screen(1),
        ],
    );
    for (index, point) in POINTS.into_iter().enumerate().skip(1) {
        bench.press(
            &draft::tap_at(point, draft::one_capture_after()),
            vec![draft::screen(index), draft::screen(index + 1)],
        );
    }
    for (index, page) in ["first", "second", "third", "done"].into_iter().enumerate() {
        let step = u32::try_from(index + 1).expect("step");
        let mut boxes = vec![draft::button_box("recognize", index)];
        if index < 3 {
            boxes.push(draft::button_box("press", index));
        }
        bench.marked(json!({"step": step, "page": page, "boxes": boxes}));
    }
    let record_id = record_id_of(&bench);
    edit_table(&bench, &record_id, |table| {
        table["locale"] = Value::Null;
        let sample = step_mut(table, 2)["frames"]
            .as_array_mut()
            .expect("step 2's frames")
            .iter_mut()
            .find(|frame| frame["frame_id"] == "f0002")
            .expect("f0002 under step 2");
        sample["superseded"] = json!(false);
        sample["role"] = json!("sample");
    });

    // Without a locale: an item of the whole recording alone.
    let unknown = review(&bench, &without_previews());
    assert!(
        unknown.steps.iter().all(|step| step.missing.is_empty()),
        "{:?}",
        unknown.steps
    );

    let options = RecordStopOptions {
        locale: Some("en-US".to_owned()),
        ..RecordStopOptions::default()
    };
    let answer = record_review(
        &bench.state(),
        INSTANCE,
        &without_previews(),
        &options,
        &bench.ocr,
    )
    .unwrap_or_else(|error| panic!("record review: {}: {}", error.code, error.message));
    let ready = record_ready(&bench.state(), INSTANCE, None, &options, &bench.ocr)
        .unwrap_or_else(|error| panic!("record ready: {}: {}", error.code, error.message));
    let second = &answer.steps[1];
    assert_eq!(second.step, 2);
    assert_eq!(
        second
            .missing
            .iter()
            .map(|item| (item.step, item.what.as_str(), item.code.as_str()))
            .collect::<Vec<_>>(),
        [(Some(2), "cross_check", "record_step_self_mismatch")],
        "{:?}",
        ready.missing
    );
    let listed = ready
        .missing
        .iter()
        .filter(|item| item.step == Some(2))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(second.missing, listed);
    for step in answer.steps.iter().filter(|step| step.step != 2) {
        assert!(
            step.missing.is_empty(),
            "step {}: {:?}",
            step.step,
            step.missing
        );
    }
}

// The fix round of the train of #463 stage two (register §1.440, "Fix round of the train"; the
// review of the train: fix round 1's third item had no test): with `no_preview`, a frame whose file
// is gone is answered without a path and named once in the warning `frame_file_missing`. The file
// of f0008, step 3's screen after and step 4's screen before, is moved out of the recording.
// Fix round of the 0.12.68 train: frames are stored by their content (`lab/frames/<sha256>.png`),
// and f0003, the second capture between step 1's press and its screen after, is screen 3 as f0008
// is, so the two frames are one file and f0003's is gone with it (the test first expected f0008
// alone). Both answer `path` `null` wherever they are answered, every other frame its file, and the
// warning names f0003 and f0008 once each, in the order the answer names them; `step` 4 alone
// answers f0008 without its path and names it alone.
#[test]
fn s22_train_fix_a_frame_whose_file_is_gone_has_no_path_and_one_warning() {
    let bench = three_presses();
    let intact = review(&bench, &without_previews());
    assert!(intact.warnings.is_empty(), "{:?}", intact.warnings);
    let last = intact.steps[3]
        .before
        .as_ref()
        .expect("step 4's screen before");
    assert_eq!(last.frame_id, "f0008");
    let file = last.path.clone().expect("the frame's file");
    // f0003 is stored in the one file of its content, f0008's.
    let same_screen = &intact.steps[0].between[1];
    assert_eq!(same_screen.frame_id, "f0003");
    assert_eq!(
        same_screen.path.as_ref(),
        Some(&file),
        "f0003 and f0008 are one file"
    );
    fs::rename(&file, bench.state().join("f0008-moved-away.png")).expect("the file moved away");

    let gone = ["f0003", "f0008"];
    let json = serde_json::to_value(review(&bench, &without_previews())).expect("answer JSON");
    assert_eq!(
        json["warnings"],
        json!([{"code": "frame_file_missing", "frame_ids": gone}]),
        "{json}"
    );
    assert_eq!(
        json["steps"][0]["between"][1]["frame_id"], "f0003",
        "{json}"
    );
    assert_eq!(json["steps"][2]["after"]["frame_id"], "f0008", "{json}");
    assert_eq!(json["steps"][3]["before"]["frame_id"], "f0008", "{json}");
    for step in json["steps"].as_array().expect("steps") {
        let sides = [&step["before"], &step["after"]]
            .into_iter()
            .filter(|side| !side.is_null());
        for frame in sides.chain(step["between"].as_array().expect("between")) {
            let frame_id = frame["frame_id"].as_str().expect("a frame id");
            assert_eq!(
                frame["path"].is_null(),
                gone.contains(&frame_id),
                "{frame}: {json}"
            );
        }
        assert!(step["before"]["preview"].is_null(), "{json}");
    }
    let one = serde_json::to_value(review(
        &bench,
        &RecordReviewRequest {
            step: Some(4),
            no_preview: true,
            ..RecordReviewRequest::default()
        },
    ))
    .expect("answer JSON");
    assert_eq!(one["steps"][0]["before"]["frame_id"], "f0008", "{one}");
    assert_eq!(one["steps"][0]["before"]["path"], Value::Null, "{one}");
    assert_eq!(
        one["warnings"],
        json!([{"code": "frame_file_missing", "frame_ids": ["f0008"]}]),
        "{one}"
    );
}
