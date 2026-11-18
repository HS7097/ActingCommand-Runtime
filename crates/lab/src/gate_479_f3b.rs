// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #479 F-3b (`contracts/display-adapter.md` section 13; register §1.487): the kernel's
//! frame gate and the Lab on a display within a pixel of the package's base's shape. A frame of
//! 1334x750 or 1366x768 for a 1280x720 package is brought to the base by the frame gate's own map
//! and its one resample (`lab_scene_on_base`, which a Lab operation's live frames, the client's
//! `--capture` forms and the stored scenes all take, on the gate a run's frame takes), and a Lab
//! press on such a frame takes the same map (`lab_display_map`); a stored scene of 1334x750 answers
//! on the base, naming `frame_coordinates` in the base form; a recording on such a display is
//! refused as before. The sizes' arithmetic is the adapter gate's (`gate_479_f3b` of the display
//! adapter): 1334x750 is 667/640 across and 25/24 down, 1366x768 683/640 and 16/15. Frames are
//! synthetic, one colour throughout; no Runtime, no device, no clock read by the tests; the
//! session header is written as `record start` writes it.

use crate::recording::store::old_record_path;
use crate::{
    FrameCoordinates, RECORDING_BASE_SIZE, RecordStartOptions, RecordingLock, is_base_size,
    lab_display_map, lab_scene_on_base, record_frame_check, record_start, scene_in_base,
};
use actingcommand_contract::DisplayMapKind;
use actingcommand_recognition::Scene;
use serde_json::{Value, json};
use std::fs;
use tempfile::TempDir;

const BASE: (u32, u32) = (1280, 720);
const COLOUR: [u8; 3] = [200, 40, 40];
const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_0479_f3b";
const TASK_ID: &str = "neutral_task";
const STARTED_AT: u64 = 1_700_000_000_000;
/// The label the check names the frame it judges by.
const LABEL: &str = "the frame the path took";

/// A synthetic scene of `width` x `height`, `COLOUR` throughout.
fn scene(width: u32, height: u32) -> Scene {
    let pixels = (0..width * height)
        .flat_map(|_| COLOUR)
        .collect::<Vec<u8>>();
    Scene::from_rgb8(width, height, &pixels).expect("a synthetic scene")
}

/// Test 1, the resample: a 1334x750 and a 1366x768 frame of a 1280x720 package pass the frame
/// gate as a pure scale with the two factors and are brought to 1280x720 in one area-average
/// resample (a frame of one colour stays that colour); the map a Lab press on such a frame leaves
/// under is that map.
#[test]
fn f3b_the_frame_gate_brings_a_frame_within_a_pixel_of_the_base_to_the_base() {
    for (frame, across, down) in [
        ((1334, 750), (667, 640), (25, 24)),
        ((1366, 768), (683, 640), (16, 15)),
    ] {
        let reduced = lab_scene_on_base(BASE, &scene(frame.0, frame.1))
            .expect("the frame gate takes the frame")
            .expect("a frame of another size than the base is reduced");
        assert_eq!(reduced.map.kind(), DisplayMapKind::Scale, "{frame:?}");
        assert_eq!(
            (reduced.map.scale().num(), reduced.map.scale().den()),
            across,
            "{frame:?}"
        );
        assert_eq!(
            (
                reduced.map.height_scale().num(),
                reduced.map.height_scale().den()
            ),
            down,
            "{frame:?}"
        );
        assert_eq!(
            (reduced.scene.width(), reduced.scene.height()),
            BASE,
            "{frame:?}"
        );
        assert!(
            reduced
                .scene
                .rgb8_pixels()
                .chunks(3)
                .all(|pixel| pixel == COLOUR.as_slice()),
            "{frame:?}"
        );
        assert_eq!(lab_display_map(BASE, frame), Ok(reduced.map), "{frame:?}");
    }
}

/// Test 5: the Lab's reading of a stored scene of 1334x750 for a 1280x720 package
/// (`scene_in_base`, which every offline command takes) answers on the base, where it was refused
/// `lab_scene_size_unsupported` before: the scene reduced to 1280x720 and `frame_coordinates` in
/// the base form, reduced from 1334x750; the Lab's live frame of that size is the frame gate's
/// (test 1). A recording on such a display is refused as before: `record start`'s display check
/// (`is_base_size`) refuses it, and the judgement every recording path asks of a frame before it
/// sends or writes anything (`record_frame_check`) refuses the first frame of a recording made at
/// the base size, `record_display_not_base` with the frame's and the base's sizes.
#[test]
fn f3b_a_stored_scene_of_1334x750_answers_on_the_base_and_a_recording_still_refuses_it() {
    let (reduced, coordinates) = scene_in_base(Some(BASE), scene(1334, 750))
        .unwrap_or_else(|error| panic!("{}: {}", error.code, error.message));
    assert_eq!((reduced.width(), reduced.height()), BASE);
    assert_eq!(
        coordinates,
        Some(FrameCoordinates::observed(BASE, (1334, 750)))
    );
    assert_eq!(
        serde_json::to_value(coordinates).expect("frame_coordinates JSON"),
        json!({
            "space": "base",
            "base_pixels": [1280, 720],
            "reduced_from": [1334, 750],
            "orientation": "landscape",
            "applies_to": ["tap", "tap_rect", "swipe"]
        })
    );

    assert!(!is_base_size(1334, 750));
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
    let lock = RecordingLock::acquire(&state, INSTANCE, "gate_479_f3b", "Lab-CLI")
        .expect("recording lock");
    let started = record_start(
        &lock,
        &RecordStartOptions {
            game: Some("neutral".to_owned()),
            server: Some("test".to_owned()),
            locale: Some("en-US".to_owned()),
            base_size: Some(RECORDING_BASE_SIZE),
            ..RecordStartOptions::default()
        },
    )
    .expect("record start");
    assert_eq!(started.status, "active", "{started:?}");
    let refused = record_frame_check(&lock, (1334, 750), LABEL)
        .expect_err("a first frame of 1334x750 for a recording at the base size");
    assert_eq!(
        refused.code, "record_display_not_base",
        "{}",
        refused.message
    );
    assert_eq!(
        refused.details.clone().unwrap_or(Value::Null)["outcome"]["values"],
        json!({"frame_width": 1334, "frame_height": 750, "base_width": 1280, "base_height": 720}),
        "{}",
        refused.message
    );
}
