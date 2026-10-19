// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #406 T4 (exec key 112; register §1.138; plan Q20, Q14; requirement basis:
// rd-1019/plan-405-406/brief-T4.md, Requirement 1 and 4, and decisions.md Q13, Q20).
//
// - An offline run draws as the kernel: each step's action seed derived from the run's seed (a
//   domain-separated SHA-256 over the seed, the step index and the operation label), the touch
//   keys of the root configuration the Lab reads (its `touch` section over the defaults file),
//   no landing history, so a single-region press is `center_hotspot_normal_v1`; `WouldClick`
//   carries the sampling evidence and the result the seed.
// - A Lab recording declares a path drag in its click rectangle; `record stop` generates it, the
//   kernel admits it and its first decision is a path inside the box; a recorded tap's
//   `point_rule` names how the Lab pressed: `center_hotspot_normal_v1` with its sampling (seed)
//   when the Lab was given a seed, `rect_center` when it pressed the centre.
//
// Every expected point comes from the reference of "The draw" below, written from the contract
// (`contracts/resource-declarations.md`, "Touch sampling" and "The Lab's draw"), not from the
// kernel's code. Packages, frames and seeds are synthetic and neutral; no device is used.

use crate::recording::store::{old_record_path, recording_path};
use crate::{
    ClickEffect, ClickSpec, CommitClickRequest, DriveDraw, LAB_RECORD_MARK_SCHEMA, LabOcr,
    LabTouch, MarkFamily, MarkRequest, MarkSpec, OfflineDraw, PlanClickRequest, RecordRect,
    RecordStartOptions, RecordStopOptions, RecordingLock, TouchSampling, record_commit_click,
    record_mark, record_plan_click, record_start, record_stop, simulate_contained_task_drawn,
};
use actingcommand_contract::{InputAction, InputSamplingEvidence, InputSamplingRegion};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_execution_kernel::{OfflineDecision, PreparedContainedTask};
use actingcommand_pack_containment::{
    ContainmentLimits, ContentContainer, expand_content_container,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Write};
use std::path::PathBuf;
use tempfile::TempDir;
use zip::{ZipWriter, write::FileOptions};

/// The reference of the draws, from the contract's text.
pub(crate) mod reference {
    use sha2::{Digest, Sha256};

    /// An offline run's (and a Lab press's) action seed: the first eight bytes, big-endian, of
    /// SHA-256 over the domain, the seed, the step index and the label with its length.
    pub(crate) fn action_seed(seed: u64, step_index: u32, label: &str) -> u64 {
        let mut hasher = Sha256::new();
        hasher.update(b"ActingCommand offline action seed v1\0");
        hasher.update(seed.to_be_bytes());
        hasher.update(step_index.to_be_bytes());
        hasher.update((label.len() as u64).to_be_bytes());
        hasher.update(label.as_bytes());
        let digest = hasher.finalize();
        u64::from_be_bytes(digest[..8].try_into().expect("eight bytes"))
    }

    fn next(state: &mut u64) -> u64 {
        let mut value = *state;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        *state = value;
        value
    }

    fn gaussian_milli(state: &mut u64) -> i128 {
        let span = 1_i128 << 24;
        let sum = (0..12).map(|_| i128::from(next(state) >> 40)).sum::<i128>();
        ((2 * sum - 12 * (span - 1)) * 1000) / (2 * span)
    }

    /// The point of the box `(x, y, width, height)` drawn from `action_seed`: one centre-hotspot
    /// point with `(sigma_ratio_milli, max_redraws)`, one uniform point without.
    pub(crate) fn box_point(
        (x, y, width, height): (i32, i32, i32, i32),
        action_seed: u64,
        touch: Option<(u32, u32)>,
    ) -> (i32, i32) {
        let mut state = if action_seed == 0 {
            0x9e37_79b9_7f4a_7c15
        } else {
            action_seed
        };
        let Some((sigma, max_redraws)) = touch else {
            let dx = next(&mut state) % u64::try_from(width).expect("width");
            let dy = next(&mut state) % u64::try_from(height).expect("height");
            return (
                x + i32::try_from(dx).expect("x"),
                y + i32::try_from(dy).expect("y"),
            );
        };
        let (x, y, width, height) = (
            i128::from(x),
            i128::from(y),
            i128::from(width),
            i128::from(height),
        );
        let (cx, cy) = (x + width / 2, y + height / 2);
        let sigma = i128::from(sigma);
        let mut point = (cx, cy);
        for _ in 0..=max_redraws {
            let zx = gaussian_milli(&mut state);
            let zy = gaussian_milli(&mut state);
            let px = cx + zx * sigma * width / 1_000_000;
            let py = cy + zy * sigma * height / 1_000_000;
            if (x..x + width).contains(&px) && (y..y + height).contains(&py) {
                point = (px, py);
                break;
            }
        }
        (
            i32::try_from(point.0).expect("x"),
            i32::try_from(point.1).expect("y"),
        )
    }
}

// ---------------------------------------------------------------- test 1: the offline run

const WIDTH: u32 = 80;
const HEIGHT: u32 = 40;
// The click box of the run's one operation.
const BOX: (i32, i32, i32, i32) = (20, 10, 40, 20);

fn zip_entries(entries: &[(&str, Value)]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in entries {
        zip.start_file(*path, options).expect("zip entry");
        serde_json::to_writer(&mut zip, value).expect("zip JSON");
        zip.write_all(b"\n").expect("zip newline");
    }
    zip.finish().expect("finish zip").into_inner()
}

// A package of one operation from `home` to `terminal` that presses the box BOX, guarded by the
// colour at (1, 0); a page is told by the colour at (0, 0).
fn offline_package() -> PreparedContainedTask {
    let (x, y, width, height) = BOX;
    let color = |id: &str, at: u32, expected: [u8; 3]| {
        json!({"type": "color", "id": id,
               "region": {"x": at, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    let bytes = zip_entries(&[
        (
            "control.json",
            json!({
                "schema_version": "Lab-1y.control.v1",
                "package_id": "neutral.t4.offline",
                "execution_mode": "navigable_route",
                "game": "neutral",
                "server": "test",
                "resolution": {"width": WIDTH, "height": HEIGHT},
                "entry_task_id": "task",
                "capture_interval_ms": 1,
                "step_timeout_ms": 10,
                "timeout_ms": 100,
                "max_steps": 2
            }),
        ),
        (
            "resources/manifest.json",
            json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        (
            "resources/operations/task/task.json",
            json!({
                "schema_version": "0.6",
                "task_id": "task",
                "game": "neutral",
                "server_scope": ["test"],
                "coordinate_space": {"width": WIDTH, "height": HEIGHT},
                "entry_page": "home",
                "target_page": "terminal",
                "operations": [{
                    "id": "open_terminal",
                    "from": "home",
                    "to": "terminal",
                    "click": {"kind": "rect", "x": x, "y": y, "width": width, "height": height},
                    "guard": {
                        "page_id": "home",
                        "target_id": "guard/ready",
                        "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                        "color_probe": "guard/ready"
                    }
                }]
            }),
        ),
        (
            "resources/recognition/neutral.test.pack.json",
            json!({
                "schema_version": "0.3",
                "game": "neutral",
                "server": "test",
                "coordinate_space": {"width": WIDTH, "height": HEIGHT},
                "defaults": {"color_max_distance": 0.0},
                "targets": [
                    color("page/home", 0, [255, 0, 0]),
                    color("page/terminal", 0, [0, 0, 255]),
                    color("guard/ready", 1, [0, 255, 0])
                ]
            }),
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            json!({"schema_version": "0.3", "pages": [
                {"id": "neutral/home", "required": ["page/home"], "optional": [], "forbidden": []},
                {"id": "neutral/terminal", "required": ["page/terminal"], "optional": [],
                 "forbidden": []}
            ]}),
        ),
    ]);
    PreparedContainedTask::load("neutral.instance", &bytes).expect("the package is admitted")
}

// The home page with its guard passing.
fn home_frame() -> Frame {
    let mut pixels = vec![0_u8; (WIDTH * HEIGHT * 3) as usize];
    pixels[..3].copy_from_slice(&[255, 0, 0]);
    pixels[3..6].copy_from_slice(&[0, 255, 0]);
    Frame::from_pixels(
        WIDTH,
        HEIGHT,
        pixels,
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("frame")
}

// Requirement 1: the root configuration names `touch.sampling.sigma_ratio_milli` 300; its other
// keys come from the defaults file (max_redraws 8, candidate_count 4, decisions Q13). The run's
// seed 7 gives the step `0` / `open_terminal` its action seed as the reference derives it; the
// press draws one centre-hotspot point (no landing history: `center_hotspot_normal_v1`, its
// spread and redraws, no candidates), the reference's point, and the result names the seed.
#[test]
fn t4_an_offline_run_draws_as_the_kernel_and_reports_the_sampling() {
    let touch = LabTouch::from_runtime_configuration(
        br#"{"touch": {"sampling": {"sigma_ratio_milli": 300}}}"#,
    )
    .sampling()
    .unwrap_or_else(|error| panic!("touch keys: {} {}", error.code, error.message));
    assert_eq!(
        touch,
        Some(TouchSampling {
            sigma_ratio_milli: 300,
            max_redraws: 8,
            candidate_count: 4,
        })
    );

    let result = simulate_contained_task_drawn(
        &offline_package(),
        vec![home_frame()],
        None,
        OfflineDraw { seed: 7, touch },
    )
    .expect("the offline run");

    let action_seed = reference::action_seed(7, 0, "open_terminal");
    let (x, y) = reference::box_point(BOX, action_seed, Some((300, 8)));
    let (bx, by, bw, bh) = BOX;
    let expected = InputSamplingEvidence::center_hotspot(
        action_seed,
        vec![InputSamplingRegion::new(bx, by, bw, bh).expect("region")],
        300,
        8,
    )
    .expect("evidence");
    let OfflineDecision::WouldClick {
        operation_label,
        action,
        sampling,
        ..
    } = &result.decision
    else {
        panic!("not a click: {:?}", result.decision);
    };
    assert_eq!(operation_label, "open_terminal");
    assert_eq!(action, &InputAction::Tap { x, y });
    assert_eq!(sampling.as_ref(), Some(&expected));
    assert_eq!(result.seed, Some(7));
    let shown = serde_json::to_value(&result).expect("result JSON");
    assert_eq!(shown["seed"], 7, "{shown}");
    assert_eq!(
        shown["decision"]["sampling"]["algorithm"], "center_hotspot_normal_v1",
        "{shown}"
    );
}

// ---------------------------------------------------------------- test 6: the recording

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_t4";
const TASK_ID: &str = "neutral_task";
const FRAME_WIDTH: u32 = 192;
const FRAME_HEIGHT: u32 = 160;
const BACKGROUND: u8 = 60;
const BUTTON: RecordRect = RecordRect {
    x: 20,
    y: 20,
    width: 12,
    height: 12,
};
const NEXT: RecordRect = RecordRect {
    x: 100,
    y: 20,
    width: 12,
    height: 12,
};
// The path drag's box: 120 x 80, so its shortest segment may be up to 59 px.
const PATH_BOX: RecordRect = RecordRect {
    x: 40,
    y: 60,
    width: 120,
    height: 80,
};
// The tap of the point-rule recording.
const TAP: RecordRect = RecordRect {
    x: 140,
    y: 100,
    width: 24,
    height: 16,
};

// A gray frame with neutral noise in `patch`.
fn gray_png(patch: RecordRect, seed: u64) -> Vec<u8> {
    let mut noise = seed;
    let mut pixels = Vec::with_capacity((FRAME_WIDTH * FRAME_HEIGHT * 3) as usize);
    for y in 0..FRAME_HEIGHT as i32 {
        for x in 0..FRAME_WIDTH as i32 {
            let inside = x >= patch.x
                && y >= patch.y
                && x < patch.x + patch.width
                && y < patch.y + patch.height;
            let value = if inside {
                noise = noise
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                1 + ((noise >> 33) % 255) as u8
            } else {
                BACKGROUND
            };
            pixels.extend([value; 3]);
        }
    }
    Frame::from_pixels(
        FRAME_WIDTH,
        FRAME_HEIGHT,
        pixels,
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
    .encode_png_fast()
    .expect("fixture PNG")
}

fn first_png() -> Vec<u8> {
    gray_png(BUTTON, 0x0406_0401)
}

fn second_png() -> Vec<u8> {
    gray_png(NEXT, 0x0406_0402)
}

fn template(id: &str, crop: RecordRect) -> MarkSpec {
    MarkSpec {
        search: Some(crop),
        ..MarkSpec::region_only(id.to_owned(), MarkFamily::Template, crop)
    }
}

// A state directory with an active session of INSTANCE and its Lab recording, before any step,
// without an OCR model.
struct Recording {
    lock: RecordingLock,
    ocr: LabOcr,
    temp: TempDir,
}

impl Recording {
    fn new() -> Self {
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
                "started_at_unix_ms": 1_700_000_000_000_u64
            }))
            .expect("session JSON"),
        )
        .expect("session file");
        let lock = RecordingLock::acquire(&state, INSTANCE, "gate_406_t4").expect("lock");
        let ocr = LabOcr::from_vision_root(&temp.path().join("no-vision"));
        record_start(
            &lock,
            &RecordStartOptions {
                game: Some("neutral".to_owned()),
                server: Some("test".to_owned()),
                locale: Some("en-US".to_owned()),
                ..RecordStartOptions::default()
            },
        )
        .expect("record start");
        Self { lock, ocr, temp }
    }

    fn frame(&self, name: &str, png: Vec<u8>) -> String {
        let path = self.temp.path().join(format!("{name}.png"));
        fs::write(&path, png).expect("frame file");
        path.display().to_string()
    }

    // Step 1: its frame, the button and `click`.
    fn mark_first(&self, click: ClickSpec) -> Value {
        let marked = record_mark(
            &self.lock,
            &MarkRequest {
                schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
                frame: Some(self.frame("first", first_png())),
                add: vec![template("ui/button", BUTTON)],
                click: Some(click),
                ..MarkRequest::default()
            },
            false,
            &self.ocr,
        )
        .unwrap_or_else(|error| panic!("step 1: {} {:?}", error.code, error.details));
        serde_json::to_value(&marked.click).expect("click JSON")
    }

    // Step 2, given offline: the final screen with its own mark.
    fn mark_second(&self) {
        record_mark(
            &self.lock,
            &MarkRequest {
                schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
                frame: Some(self.frame("second", second_png())),
                add: vec![template("ui/next", NEXT)],
                ..MarkRequest::default()
            },
            false,
            &self.ocr,
        )
        .unwrap_or_else(|error| panic!("step 2: {} {:?}", error.code, error.details));
    }

    // `record stop`, and the files of the package it wrote.
    fn stop(&self) -> BTreeMap<String, Vec<u8>> {
        let outcome = record_stop(&self.lock, &RecordStopOptions::default(), &self.ocr)
            .unwrap_or_else(|error| {
                panic!(
                    "record stop: {}: {} {:?}",
                    error.code, error.message, error.details
                )
            });
        let lab = serde_json::to_value(&outcome.lab).expect("lab JSON");
        assert_eq!(outcome.lab_status.as_deref(), Some("generated"), "{lab}");
        let path = lab["path"].as_str().expect("package path");
        let bytes = fs::read(path).expect("package file");
        expand_content_container(&bytes, ContentContainer::Zip, ContainmentLimits::default())
            .expect("package container")
    }

    fn recording(&self) -> Value {
        let path: PathBuf = recording_path(&self.temp.path().join("state"), RECORD_ID);
        serde_json::from_slice(&fs::read(path).expect("recording file")).expect("recording")
    }
}

// Requirement 4: the request form `"click": {"region": PATH_BOX, "path_drag": {...}}` declares a
// path drag in the click rectangle, kept as the click's gesture; `record stop` declares it as
// the source click (`box`, compiled to `box_rect`) with its numbers, and the first decision the
// admitted package gives on step 1's frame is a path in that box (inside the frame): `turns + 2`
// points, all in the box, segment durations summing to `duration_ms`, the corner hold.
// Requirement 4: a recorded tap's `point_rule` names how the Lab pressed: drawn from the seed
// 5 with the touch keys, `center_hotspot_normal_v1` with its sampling and the reference's point
// (the first press, no label); without a seed, `rect_center` at the rectangle's centre.
#[test]
fn t4_recording_writes_a_path_drag_and_names_the_press_rule() {
    let gesture = json!({"kind": "single_touch_path_drag_v1", "turns": 3,
                         "min_segment_px": 20, "duration_ms": 1200, "corner_hold_ms": 40});
    let recording = Recording::new();
    let click = recording.mark_first(
        serde_json::from_value(json!({"region": PATH_BOX, "path_drag": {
            "turns": 3, "min_segment_px": 20, "duration_ms": 1200, "corner_hold_ms": 40
        }}))
        .expect("a path drag click request"),
    );
    assert_eq!(click["rect"], json!(PATH_BOX));
    assert_eq!(click["gesture"], gesture);
    assert_eq!(
        recording.recording()["steps"][0]["click"]["gesture"],
        gesture
    );
    recording.mark_second();

    let entries = recording.stop();

    let task: Value = serde_json::from_slice(
        entries
            .get(&format!("resources/operations/{TASK_ID}/task.json"))
            .expect("task.json"),
    )
    .expect("task JSON");
    assert_eq!(
        task["operations"][0]["click"],
        json!({"kind": "single_touch_path_drag_v1", "box": PATH_BOX, "turns": 3,
               "min_segment_px": 20, "duration_ms": 1200, "corner_hold_ms": 40})
    );
    let prepared = crate::recording::crosscheck::admit(&entries, &recording.ocr)
        .unwrap_or_else(|error| panic!("admission: {}", error.code));
    let frame = Frame::from_png(first_png(), CaptureBackendName::AdbScreencap).expect("frame");
    let result = crate::simulate_contained_task(&prepared, vec![frame]).expect("simulation");
    let OfflineDecision::WouldClick {
        action:
            InputAction::SingleTouchPathDragV1 {
                box_x,
                box_y,
                box_width,
                box_height,
                points,
                segment_duration_ms,
                corner_hold_ms,
            },
        ..
    } = &result.decision
    else {
        panic!("not a path drag: {:?}", result.decision);
    };
    assert_eq!(
        (*box_x, *box_y, *box_width, *box_height),
        (PATH_BOX.x, PATH_BOX.y, PATH_BOX.width, PATH_BOX.height)
    );
    assert_eq!(points.len(), 5);
    for point in points {
        assert!(
            point.x >= PATH_BOX.x
                && point.y >= PATH_BOX.y
                && point.x < PATH_BOX.x + PATH_BOX.width
                && point.y < PATH_BOX.y + PATH_BOX.height,
            "{point:?} outside the box"
        );
    }
    assert_eq!(segment_duration_ms.iter().sum::<u64>(), 1200);
    assert_eq!(*corner_hold_ms, 40);

    // The point rule of a tap the Lab presses with a seed, and of one it presses without.
    let draw = DriveDraw {
        seed: 5,
        touch: Some(TouchSampling {
            sigma_ratio_milli: 250,
            max_redraws: 8,
            candidate_count: 4,
        }),
    };
    let tapped = Recording::new();
    tapped.mark_first(ClickSpec {
        region: Some(TAP),
        ..ClickSpec::default()
    });
    let centre = record_plan_click(&tapped.lock, &PlanClickRequest::default())
        .unwrap_or_else(|error| panic!("plan: {} {:?}", error.code, error.details));
    assert_eq!(centre.point_rule, "rect_center");
    assert_eq!((centre.point.x, centre.point.y), (152, 108));
    assert_eq!(centre.sampling, None);
    let plan = record_plan_click(
        &tapped.lock,
        &PlanClickRequest {
            draw: Some(draw),
            ..PlanClickRequest::default()
        },
    )
    .unwrap_or_else(|error| panic!("plan: {} {:?}", error.code, error.details));
    let action_seed = reference::action_seed(5, 0, "");
    let expected = InputSamplingEvidence::center_hotspot(
        action_seed,
        vec![InputSamplingRegion::new(TAP.x, TAP.y, TAP.width, TAP.height).expect("region")],
        250,
        8,
    )
    .expect("evidence");
    assert_eq!(plan.point_rule, "center_hotspot_normal_v1");
    assert_eq!(
        (plan.point.x, plan.point.y),
        reference::box_point(
            (TAP.x, TAP.y, TAP.width, TAP.height),
            action_seed,
            Some((250, 8))
        )
    );
    assert_eq!(plan.sampling.as_ref(), Some(&expected));
    let committed = record_commit_click(
        &tapped.lock,
        &plan,
        &CommitClickRequest {
            effect: ClickEffect::Performed,
            has_failure: false,
            carrier_package: None,
            req_id: None,
            correlation_id: None,
            action_id: None,
            lease_id: None,
            failure: None,
            before: None,
            after: None,
        },
    )
    .unwrap_or_else(|error| panic!("commit: {} {:?}", error.code, error.details));
    assert!(committed.step_closed);
    let execution = tapped.recording()["steps"][0]["click"]["execution"].clone();
    assert_eq!(execution["point_rule"], "center_hotspot_normal_v1");
    assert_eq!(
        execution["sampling"],
        serde_json::to_value(&expected).expect("evidence JSON")
    );
}
