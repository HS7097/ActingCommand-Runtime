// SPDX-License-Identifier: AGPL-3.0-only

// #411 A62 (P57; 0.12.28 LAB-SMALL, coordinator work order rd-1029/brief-wave2d.md): offline
// `observe --targets` says where a `target_center` click on each named target lands on the
// frame: `target_center {x, y}`, the centre of the box its template matched, as the kernel
// presses a `target_center` click (`x + width / 2`, `y + height / 2`); `null` when the target did
// not pass, where no such click is made. The frame and the icon are synthetic and neutral.

use super::*;
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_recognition_pack::{AssetResolver, RecognitionPackError, RecognitionPackResult};
use std::sync::Arc;

const WIDTH: u32 = 96;
const HEIGHT: u32 = 64;
const BACKGROUND: u8 = 60;
const ICON: u32 = 16;
const TARGET: &str = "ui/mark";

/// A deterministic 16×16 gray noise icon, row-major.
fn icon() -> Vec<u8> {
    let mut state: u64 = 0x0411_0057;
    (0..ICON * ICON)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            1 + ((state >> 33) % 255) as u8
        })
        .collect()
}

fn rgb(gray: &[u8]) -> Vec<u8> {
    gray.iter().flat_map(|value| [*value; 3]).collect()
}

/// The flat frame, with the icon's top-left corner at `at` when given.
fn scene(at: Option<(u32, u32)>) -> Scene {
    let mut gray = vec![BACKGROUND; (WIDTH * HEIGHT) as usize];
    if let Some((x, y)) = at {
        let icon = icon();
        for row in 0..ICON {
            for column in 0..ICON {
                gray[((y + row) * WIDTH + x + column) as usize] =
                    icon[(row * ICON + column) as usize];
            }
        }
    }
    Scene::from_rgb8(WIDTH, HEIGHT, &rgb(&gray)).expect("scene")
}

#[derive(Debug)]
struct MarkAsset(Vec<u8>);

impl AssetResolver for MarkAsset {
    fn read_asset(&self, path: &str) -> RecognitionPackResult<Vec<u8>> {
        if path == "mark.png" {
            Ok(self.0.clone())
        } else {
            Err(RecognitionPackError::fatal("no such fixture asset"))
        }
    }
}

/// `ui/mark`, the icon as a full-frame template, scored `ccoeff_normed` (the noise icon's
/// flat-field floor under `ccorr_normed` is above the threshold).
fn evaluator() -> RecognitionEvaluator {
    let pack = load_pack_from_json_str(
        &json!({
            "schema_version": "0.7",
            "game": "neutral",
            "server": "test",
            "coordinate_space": {"width": WIDTH, "height": HEIGHT},
            "defaults": {"match_metric": "ccoeff_normed"},
            "targets": [
                {"type": "template", "id": TARGET, "template_path": "mark.png",
                 "region": "full_frame", "threshold": 0.9}
            ]
        })
        .to_string(),
    )
    .expect("the neutral pack");
    let png = Frame::from_pixels(
        ICON,
        ICON,
        rgb(&icon()),
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("icon frame")
    .encode_png_fast()
    .expect("icon PNG");
    let evaluator = RecognitionEvaluator::with_asset_resolver(pack, Arc::new(MarkAsset(png)))
        .expect("evaluator");
    evaluator
        .admit_template_assets()
        .expect("the pack's template admission");
    evaluator
}

/// The `targets` entry offline `observe --targets ui/mark` answers on `frame`.
fn observed_target(evaluator: &RecognitionEvaluator, frame: &Scene) -> Value {
    let flags = FlagArgs::parse(&["--targets".to_string(), TARGET.to_string()]).expect("flags");
    let outcome = PageDetectionOutcome {
        page: "unknown".to_string(),
        matched: false,
        standby: false,
        evaluations: Vec::new(),
    };
    let (targets, _) =
        super::super::observe_targets(evaluator, &[frame], &flags, &outcome).expect("targets");
    assert_eq!(targets.len(), 1, "{targets:?}");
    targets[0].clone()
}

#[test]
fn p57_observe_targets_names_where_a_target_center_click_lands() {
    let evaluator = evaluator();

    // The icon at (20, 16), 16×16: a `target_center` click presses (28, 24).
    let found = observed_target(&evaluator, &scene(Some((20, 16))));
    assert_eq!(found["id"], TARGET, "{found}");
    assert_eq!(found["passed"], true, "{found}");
    assert_eq!(found["target_center"], json!({"x": 28, "y": 24}), "{found}");

    // Without the icon the target does not pass, and no click lands.
    let missing = observed_target(&evaluator, &scene(None));
    assert_eq!(missing["passed"], false, "{missing}");
    assert_eq!(missing["target_center"], Value::Null, "{missing}");
}
