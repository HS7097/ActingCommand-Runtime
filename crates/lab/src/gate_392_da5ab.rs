// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #392 DA-5ab fix round 1 (contracts/display-adapter.md section 11, "Presses" and "The
// answers"; register §1.436; the coordinator's work order of 11-16 after the review of pull
// request #1780, points 3 and 5): a Lab operation speaks its package's base, so the recording
// takes from it only what is in the base.
// - `da5ab_fix1_a_recorded_press_takes_a_recording_of_its_packages_base_only`: the check
//   `do --capture --record` (and `observe --capture --record`, before it captures) makes of the
//   recording: a recording without a frame passes; one whose frames are 3x3 is refused for a 2x2
//   base, `record_frame_size_mismatch`, its details naming the recording's size as expected and
//   the base as actual, and the recording's file is not written; for a 3x3 base it passes.
// - `da5ab_fix1_a_recorded_observation_takes_a_frame_of_its_packages_base_only`: the check
//   `observe --capture --record` makes of the Runtime's frame after it observed: a frame of the
//   base's size passes; a 3x3 or a 1920x1080 frame for a 2x2 or a 1280x720 base is refused,
//   `record_frame_size_mismatch`, its details naming the base as expected and the device frame as
//   actual.
// Frames are synthetic; no Runtime, no device, no clock read by the tests; the session header is
// written as `record start` writes it.

use crate::recording::store::{old_record_path, recording_path};
use crate::{
    AttachFrameRequest, RecordStartOptions, RecordingLock, record_attach_frame,
    record_observation_base_check, record_press_base_check, record_start,
};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_0392_da5ab";
const TASK_ID: &str = "neutral_task";
const STARTED_AT: u64 = 1_700_000_000_000;

/// A state directory with an active session of `INSTANCE` and its Lab recording, without a frame.
struct Bench {
    temp: TempDir,
}

impl Bench {
    fn new() -> Self {
        let bench = Self {
            temp: TempDir::new().expect("temporary directory"),
        };
        fs::create_dir_all(bench.state()).expect("state directory");
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
        let started = record_start(
            &bench.lock(),
            &RecordStartOptions {
                game: Some("neutral".to_owned()),
                server: Some("test".to_owned()),
                locale: Some("en-US".to_owned()),
                ..RecordStartOptions::default()
            },
        )
        .expect("record start");
        assert_eq!(started.status, "active", "{started:?}");
        bench
    }

    fn state(&self) -> PathBuf {
        self.temp.path().join("state")
    }

    fn lock(&self) -> RecordingLock {
        RecordingLock::acquire(&self.state(), INSTANCE, "gate_392_da5ab", "Lab-CLI")
            .expect("recording lock")
    }

    fn recording_bytes(&self) -> Vec<u8> {
        fs::read(recording_path(&self.state(), RECORD_ID)).expect("the recording's file")
    }

    /// A device frame of `size` x `size`, as `capture --record` attaches it.
    fn attach(&self, size: u32) {
        let pixels = (0..size * size).flat_map(|_| [200_u8, 40, 40]).collect();
        let png = Frame::from_pixels(
            size,
            size,
            pixels,
            PixelFormat::Rgb8,
            CaptureBackendName::FixtureSimulation,
        )
        .expect("fixture frame")
        .encode_png_fast()
        .expect("fixture PNG");
        record_attach_frame(
            &self.lock(),
            AttachFrameRequest {
                png,
                source: "lab_capture".to_owned(),
                runtime_artifact: None,
                capture_backend: Some("adb_screencap".to_owned()),
                freshness: None,
            },
        )
        .unwrap_or_else(|error| panic!("frame: {}: {}", error.code, error.message));
    }
}

#[test]
fn da5ab_fix1_a_recorded_press_takes_a_recording_of_its_packages_base_only() {
    let bench = Bench::new();
    record_press_base_check(&bench.lock(), (2, 2))
        .unwrap_or_else(|error| panic!("a recording without a frame: {}", error.message));
    bench.attach(3);
    let before = bench.recording_bytes();
    let error =
        record_press_base_check(&bench.lock(), (2, 2)).expect_err("a 3x3 recording for a 2x2 base");
    assert_eq!(
        error.code, "record_frame_size_mismatch",
        "{}",
        error.message
    );
    assert_eq!(
        error.details,
        Some(json!({
            "frame": "the package's base",
            "expected": {"width": 3, "height": 3},
            "actual": {"width": 2, "height": 2},
        })),
        "{}",
        error.message
    );
    assert_eq!(bench.recording_bytes(), before, "nothing written");
    record_press_base_check(&bench.lock(), (3, 3))
        .unwrap_or_else(|error| panic!("a 3x3 recording for a 3x3 base: {}", error.message));
}

#[test]
fn da5ab_fix1_a_recorded_observation_takes_a_frame_of_its_packages_base_only() {
    for base in [(2, 2), (1280, 720)] {
        record_observation_base_check(base, base)
            .unwrap_or_else(|error| panic!("{base:?} on {base:?}: {}", error.message));
    }
    for (base, device) in [((2, 2), (3, 3)), ((1280, 720), (1920, 1080))] {
        let error = record_observation_base_check(base, device)
            .expect_err("a device frame of another size than the base");
        assert_eq!(
            error.code, "record_frame_size_mismatch",
            "{base:?} on {device:?}: {}",
            error.message
        );
        assert_eq!(
            error.details,
            Some(json!({
                "frame": "the Runtime observation's frame",
                "expected": {"width": base.0, "height": base.1},
                "actual": {"width": device.0, "height": device.1},
            })),
            "{base:?} on {device:?}"
        );
    }
}
