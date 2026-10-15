// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #396 REC-2 logic tests of the kernel: a guard on a template that declares `unique`
//! refuses its input while a second place exists, and a search past its time limit fails the
//! guard evaluation; neither taps. Frames are synthetic and neutral.

use super::*;
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition_pack::{
    AssetResolver, RecognitionPackResult, SearchClock, load_pack_from_json_str,
};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};

const ICON: u32 = 24;

#[derive(Debug)]
struct IconAsset(Vec<u8>);

impl AssetResolver for IconAsset {
    fn read_asset(&self, path: &str) -> RecognitionPackResult<Vec<u8>> {
        if path == "icon.png" {
            Ok(self.0.clone())
        } else {
            Err(actingcommand_recognition_pack::RecognitionPackError::fatal(
                "no such fixture asset",
            ))
        }
    }
}

/// A neutral 24 by 24 pattern; no pixel is black.
fn icon_pixel(x: u32, y: u32) -> [u8; 3] {
    [
        ((x * 37 + y * 11) % 250 + 5) as u8,
        ((x * 13 + y * 41 + 7) % 250 + 5) as u8,
        ((x * 29 + y * 17 + 3) % 250 + 5) as u8,
    ]
}

fn control() -> TaskControl {
    serde_json::from_value(json!({
        "schema_version": "Lab-1y.control.v1",
        "package_id": "neutral.semantic.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 640, "height": 360},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 5000
    }))
    .expect("task control")
}

/// One operation from the home page to the terminal page that taps the unique icon it guards.
fn operation() -> Value {
    json!({
        "id": "open_detail",
        "from": "home",
        "to": "terminal",
        "click": {"kind": "target", "target_id": "icon/unique"},
        "guard": {
            "page_id": "home",
            "target_id": "icon/unique",
            "expected_rect": {"x": 0, "y": 0, "width": 640, "height": 360},
            "verify_template": "icon/unique"
        }
    })
}

fn task(clock: Option<SearchClock>) -> PreparedContainedTask {
    let pack = load_pack_from_json_str(
        &json!({
            "schema_version": "0.7",
            "game": "neutral",
            "server": "test",
            "coordinate_space": {"width": 640, "height": 360},
            "defaults": {"color_max_distance": 0.0},
            "targets": [
                {"type": "color", "id": "page/home",
                 "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [255, 0, 0]},
                {"type": "color", "id": "page/terminal",
                 "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [0, 0, 255]},
                {"type": "template", "id": "icon/unique", "template_path": "icon.png",
                 "region": "full_frame", "threshold": 0.9, "unique": {"margin_milli": 50}}
            ]
        })
        .to_string(),
    )
    .expect("recognition pack");
    let mut evaluator = RecognitionEvaluator::with_asset_resolver(
        pack,
        Arc::new(IconAsset(encode_png(ICON, ICON, icon_pixel))),
    )
    .expect("recognition evaluator");
    if let Some(clock) = clock {
        evaluator = evaluator.with_search_clock(clock);
    }
    let detector = PageDetector::new(
        serde_json::from_value(json!({
            "schema_version": "0.6",
            "pages": [
                {"id": "neutral/home", "required": ["page/home"]},
                {"id": "neutral/terminal", "required": ["page/terminal"]}
            ]
        }))
        .expect("page set"),
    )
    .expect("page detector");
    let control = control();
    let program = TaskProgram {
        schema_version: "0.9".to_string(),
        task_id: "task".to_string(),
        game: "neutral".to_string(),
        server_scope: vec!["test".to_string()],
        coordinate_space: control.resolution,
        entry_page: None,
        phases: None,
        timeout_ms: None,
        max_steps: None,
        target_page: Some(PageDeclaration::Singleton("terminal".to_string())),
        error_pages: Vec::new(),
        scheduling_outcome: None,
        post_admission_ocr: None,
        resource_readings: None,
        resource_adjustments: None,
        stability_termination: None,
        recovery: None,
        defaults: TaskOperationDefaults::default(),
        operations: vec![serde_json::from_value(operation()).expect("task operation")],
    };
    PreparedContainedTask {
        control,
        program,
        evaluator,
        detector,
        entry_page: None,
        scheduling_outcome: None,
        post_admission_ocr: None,
        post_admission_fields: None,
        entry_count: 5,
        task_count: 1,
    }
}

/// The home page (a red pixel at the origin) with the icon at each of `copies`.
fn home_frame(copies: &[(u32, u32)]) -> Frame {
    let mut pixels = vec![0_u8; 640 * 360 * 3];
    pixels[0..3].copy_from_slice(&[255, 0, 0]);
    for (left, top) in copies {
        for y in 0..ICON {
            for x in 0..ICON {
                let start = (((top + y) * 640 + left + x) * 3) as usize;
                pixels[start..start + 3].copy_from_slice(&icon_pixel(x, y));
            }
        }
    }
    Frame::from_pixels(
        640,
        360,
        pixels,
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("home frame")
}

/// A device that always shows one frame and records every input.
struct TapRuntime {
    frame: Frame,
    inputs: Vec<InputAction>,
}

impl ContainedTaskRuntime for TapRuntime {
    type Error = &'static str;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        Ok(self
            .frame
            .try_clone()
            .expect("copy the fixture frame")
            .into())
    }

    fn input(
        &mut self,
        action: InputAction,
        _frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        self.inputs.push(action);
        Ok(())
    }

    fn record(&mut self, _trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[test]
fn a_second_place_refuses_the_template_guard_and_taps_nothing() {
    let task = task(None);
    let mut runtime = TapRuntime {
        frame: home_frame(&[(100, 80), (400, 200)]),
        inputs: Vec::new(),
    };

    let Err(ContainedTaskRunError::Task(error)) = task.run(&mut runtime) else {
        panic!("the unique guard refuses the input");
    };

    assert_eq!(error.code().as_str(), "contained_task_guard_refused");
    assert_eq!(
        error.detail(),
        Some("operation=open_detail target=icon/unique cause=target_match_not_unique")
    );
    assert!(runtime.inputs.is_empty(), "zero inputs");
}

#[test]
fn a_search_past_its_time_limit_fails_the_guard_evaluation_and_taps_nothing() {
    // Every reading of the search clock is 6 s after the previous one.
    let base = Instant::now();
    let readings = Arc::new(AtomicU64::new(0));
    let task = task(Some(SearchClock::new(move || {
        base + Duration::from_secs(6 * readings.fetch_add(1, Ordering::SeqCst))
    })));
    let mut runtime = TapRuntime {
        frame: home_frame(&[(100, 80)]),
        inputs: Vec::new(),
    };

    let Err(ContainedTaskRunError::Task(error)) = task.run(&mut runtime) else {
        panic!("the guard evaluation fails");
    };

    assert_eq!(
        error.code().as_str(),
        "contained_task_guard_evaluation_failed"
    );
    assert!(
        error
            .detail()
            .is_some_and(|detail| detail.ends_with(" cause=target_match_search_incomplete")),
        "detail: {:?}",
        error.detail()
    );
    assert!(runtime.inputs.is_empty(), "zero inputs, never a pass");
}

/// An uncompressed RGB PNG of one stored deflate block.
fn encode_png(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
    let mut scanlines = Vec::new();
    for y in 0..height {
        scanlines.push(0);
        for x in 0..width {
            scanlines.extend_from_slice(&pixel(x, y));
        }
    }
    let length = u16::try_from(scanlines.len()).expect("one stored block");
    let mut zlib = vec![0x78, 0x01, 0x01];
    zlib.extend_from_slice(&length.to_le_bytes());
    zlib.extend_from_slice(&(!length).to_le_bytes());
    zlib.extend_from_slice(&scanlines);
    let (mut a, mut b) = (1_u32, 0_u32);
    for byte in &scanlines {
        a = (a + u32::from(*byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::new();
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &header);
    chunk(&mut png, b"IDAT", &zlib);
    chunk(&mut png, b"IEND", &[]);
    png
}

fn chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    png.extend_from_slice(&(data.len() as u32).to_be_bytes());
    png.extend_from_slice(kind);
    png.extend_from_slice(data);
    let mut crc = 0xffff_ffff_u32;
    for byte in kind.iter().chain(data) {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0_u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    png.extend_from_slice(&(!crc).to_be_bytes());
}
