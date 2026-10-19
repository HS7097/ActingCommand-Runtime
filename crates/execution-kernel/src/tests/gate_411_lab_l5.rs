// SPDX-License-Identifier: AGPL-3.0-only

// #411 Lab L5a (host key 120; register §1.130 L5-1): an online Lab observation evaluates the
// frames of its transaction as one screen, so frames of different sizes fail it with the
// kernel's `recognition_sample_geometry_changed`, raised by the evaluation itself. The package
// and the frames are synthetic and neutral.

use crate::codes::ExecutionKernelCode;
use crate::{ObservationMode, PreparedPageObservation};
use actingcommand_contract::page_projection::{FrameIdentity, FrameKind};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use serde_json::{Value, json};
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

const HOME: [u8; 3] = [255, 0, 0];

fn json_bytes(value: Value) -> Vec<u8> {
    serde_json::to_vec(&value).expect("JSON")
}

// An observation package (the Lab resource layout) whose one page needs its mark on two frames
// of two, 1 ms apart.
fn package() -> Vec<u8> {
    let entries = [
        (
            "control.json",
            json_bytes(json!({
                "game": "neutral",
                "server": "test",
                "package_id": "neutral.lab_l5",
                "entry_task_id": "task"
            })),
        ),
        (
            "resources/manifest.json",
            json_bytes(json!({"entry_task_id": "task"})),
        ),
        (
            "resources/operations/task/task.json",
            json_bytes(json!({"task_id": "task"})),
        ),
        (
            "resources/recognition/neutral.test.pack.json",
            json_bytes(json!({
                "schema_version": "0.7",
                "game": "neutral",
                "server": "test",
                "coordinate_space": {"width": 2, "height": 1},
                "defaults": {"color_max_distance": 0.0},
                "targets": [
                    {"type": "color", "id": "page/home",
                     "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": HOME}
                ],
                "target_consensus": {
                    "page/home": {"samples": [{"frame": 0}, {"frame": 1}], "k": 2,
                                  "sample_interval_ms": 1}
                }
            })),
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            json_bytes(json!({
                "schema_version": "0.6",
                "pages": [{"id": "neutral/home", "required": ["page/home"]}]
            })),
        ),
        (
            "resources/navigation/neutral.test.navigation.json",
            json_bytes(json!({"navigation": []})),
        ),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, bytes) in entries {
        zip.start_file(path, options).expect("zip entry");
        zip.write_all(&bytes).expect("zip bytes");
    }
    zip.finish().expect("finish zip").into_inner()
}

// A frame `width` pixels wide and one high, every pixel showing the page's mark.
fn png(width: u32) -> Vec<u8> {
    Frame::from_pixels(
        width,
        1,
        HOME.repeat(width as usize),
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
    .encode_png_fast()
    .expect("fixture PNG")
}

// Two frames of one transaction that differ in size fail the observation with the kernel's
// `recognition_sample_geometry_changed`; nothing is recognised from them.
#[test]
fn l5_frames_of_different_sizes_fail_the_observation() {
    let observation = PreparedPageObservation::load("node.a", &package(), &[], None)
        .unwrap_or_else(|error| {
            panic!(
                "the package is admitted for online observation: {error} ({})",
                error.cause()
            )
        });
    assert_eq!(observation.frame_plan().frames(), 2);
    let (earlier, current) = (png(3), png(2));
    let Err(error) = observation.evaluate_frames(
        &[earlier.as_slice(), current.as_slice()],
        FrameIdentity {
            kind: FrameKind::Rgb8,
            sha256: "0".repeat(64),
            width: 2,
            height: 1,
        },
        ObservationMode::Inspect,
    ) else {
        panic!("frames of different sizes are not one screen");
    };
    assert_eq!(
        error.code(),
        ExecutionKernelCode::RecognitionSampleGeometryChanged.code(),
        "{error} ({})",
        error.cause()
    );
}
