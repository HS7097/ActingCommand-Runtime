// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 B13 logic tests (B-train plan §2.10 with D9, §5.3 rows "BT-B13" T1 to T3): a
// template target may declare `luma_floor`; the window a candidate matched is read over the
// template's scored pixels, its mean and its standard deviation each divided by the template's
// and times 1000; a candidate below a declared floor is not accepted, so the target does not
// pass although its score does, and the template evidence carries the ratios as `luma` only when
// the target declares the floor. Frames are synthetic grey windows on a flat grey background.

use crate::*;
use serde_json::json;

const TONES: &str = "assets/tones.png";
/// The template's two grey tones, left half and right half: over its 48 pixels its mean is 150
/// and its standard deviation 50.
const LOW: u8 = 100;
const HIGH: u8 = 200;
/// Where the tone window lies in every frame.
const WINDOW: (i32, i32) = (12, 7);
/// The relative color square of T3's checked target and a square of another color.
const MARK: [u8; 3] = [200, 40, 40];
const OTHER: [u8; 3] = [90, 90, 90];

#[derive(Debug)]
struct Assets(Vec<u8>);

impl AssetResolver for Assets {
    fn read_asset(&self, path: &str) -> RecognitionPackResult<Vec<u8>> {
        if path == TONES {
            Ok(self.0.clone())
        } else {
            Err(RecognitionPackError::fatal(format!("no asset {path}")))
        }
    }
}

/// An RGB frame, row-major.
struct Frame {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Frame {
    fn new(width: u32, height: u32, color: [u8; 3]) -> Self {
        Self {
            width,
            height,
            pixels: color.repeat((width * height) as usize),
        }
    }

    fn fill(&mut self, x: u32, y: u32, width: u32, height: u32, color: [u8; 3]) {
        for row in y..y + height {
            for column in x..x + width {
                let start = ((row * self.width + column) * 3) as usize;
                self.pixels[start..start + 3].copy_from_slice(&color);
            }
        }
    }

    fn scene(&self) -> Scene {
        Scene::from_rgb8(self.width, self.height, &self.pixels).expect("scene")
    }
}

/// An 8x6 grey window: its left four columns `low`, its right four `high`.
fn tones(low: u8, high: u8) -> Frame {
    let mut window = Frame::new(8, 6, [low; 3]);
    window.fill(4, 0, 4, 6, [high; 3]);
    window
}

/// A 32x20 frame with the tone window `low`/`high` at `WINDOW`, on a background halfway between
/// the two tones. Both of the window's side edges then step down, against the template's step up,
/// so no box straddling an edge matches the template and the window is the frame's only full
/// match. (A background darker than `low`, or brighter than `high`, matches as well where it
/// meets that tone.)
fn frame(low: u8, high: u8) -> Frame {
    let mut frame = Frame::new(32, 20, [low + (high - low) / 2; 3]);
    let (x, y) = (WINDOW.0 as u32, WINDOW.1 as u32);
    frame.fill(x, y, 4, 6, [low; 3]);
    frame.fill(x + 4, y, 4, 6, [high; 3]);
    frame
}

/// A template target on the tones, searched over the whole frame, with `luma_floor` when given.
fn tone_target(id: &str, luma_floor: Option<Value>) -> Value {
    let mut target = json!({
        "type": "template", "id": id, "template_path": TONES, "region": "full_frame",
        "threshold": 0.95
    });
    if let Some(floor) = luma_floor {
        target["luma_floor"] = floor;
    }
    target
}

fn evaluator(targets: Vec<Value>) -> RecognitionEvaluator {
    let pack = json!({
        "schema_version": "0.7",
        "coordinate_space": {"width": 32, "height": 20},
        "defaults": {"match_metric": "ccoeff_normed"},
        "targets": targets
    });
    let template = tones(LOW, HIGH);
    RecognitionEvaluator::with_asset_resolver(
        load_pack_from_json_str(&pack.to_string()).expect("the pack loads"),
        Arc::new(Assets(encode_png(
            template.width,
            template.height,
            &template.pixels,
        ))),
    )
    .expect("evaluator")
}

/// The template evidence's `luma` member as the evaluation writes it, absent when not written.
fn luma(evaluation: &TargetEvaluation) -> Option<Value> {
    let written = serde_json::to_value(evaluation).expect("the evaluation serializes");
    written
        .get("template")
        .and_then(|template| template.get("luma"))
        .cloned()
}

/// The template evidence at the tone window, whose score reaches its threshold.
fn assert_scored_at_window(evaluation: &TargetEvaluation) {
    let template = evaluation.template.expect("template evidence");
    assert_eq!((template.x, template.y), WINDOW, "{evaluation:?}");
    assert!(template.score >= template.threshold, "{evaluation:?}");
}

#[test]
fn bt_b13_t1_a_window_darker_than_the_mean_floor_fails_although_its_score_passes() {
    let evaluator = evaluator(vec![tone_target(
        "ui/tones",
        Some(json!({"mean_ratio_milli": 800})),
    )]);

    // The template's own tones: both ratios 1000, the floor holds.
    let same = evaluator
        .evaluate_target(&frame(LOW, HIGH).scene(), "ui/tones")
        .expect("evaluated");
    assert_scored_at_window(&same);
    assert!(same.passed, "{same:?}");
    assert_eq!(
        luma(&same),
        Some(json!({"mean_ratio_milli": 1000, "contrast_ratio_milli": 1000}))
    );

    // Half as bright (50 and 100): mean 75 of 150 and deviation 25 of 50, 500 each, below 800.
    let darker = evaluator
        .evaluate_target(&frame(50, 100).scene(), "ui/tones")
        .expect("evaluated");
    assert_scored_at_window(&darker);
    assert!(!darker.passed, "{darker:?}");
    assert_eq!(
        luma(&darker),
        Some(json!({"mean_ratio_milli": 500, "contrast_ratio_milli": 500}))
    );
}

#[test]
fn bt_b13_t2_a_window_flatter_than_the_contrast_floor_fails_although_its_score_passes() {
    let evaluator = evaluator(vec![tone_target(
        "ui/tones",
        Some(json!({"contrast_ratio_milli": 800})),
    )]);

    // The template's own tones: both ratios 1000, the floor holds.
    let same = evaluator
        .evaluate_target(&frame(LOW, HIGH).scene(), "ui/tones")
        .expect("evaluated");
    assert_scored_at_window(&same);
    assert!(same.passed, "{same:?}");
    assert_eq!(
        luma(&same),
        Some(json!({"mean_ratio_milli": 1000, "contrast_ratio_milli": 1000}))
    );

    // Half the spread around the same mean (125 and 175): mean 150 of 150, 1000; deviation 25
    // of 50, 500, below 800.
    let flatter = evaluator
        .evaluate_target(&frame(125, 175).scene(), "ui/tones")
        .expect("evaluated");
    assert_scored_at_window(&flatter);
    assert!(!flatter.passed, "{flatter:?}");
    assert_eq!(
        luma(&flatter),
        Some(json!({"mean_ratio_milli": 1000, "contrast_ratio_milli": 500}))
    );
}

#[test]
fn bt_b13_t3_without_a_floor_no_luma_member_is_written_and_score_and_color_decide() {
    // A plain target and one whose relative color check expects `MARK` 10 to the right of and 1
    // below the match; neither declares `luma_floor`.
    let mut checked = tone_target("ui/checked", None);
    checked["color_check"] = json!({
        "region": {"mode": "template_relative", "anchor_target_id": "ui/checked",
                   "offset": {"x": 10, "y": 1}, "width": 4, "height": 4},
        "expected": MARK, "max_distance": 20.0
    });
    let evaluator = evaluator(vec![tone_target("ui/plain", None), checked]);
    let square = |color: [u8; 3]| {
        // The darker tones of T1, which no floor refuses here.
        let mut frame = frame(50, 100);
        frame.fill(WINDOW.0 as u32 + 10, WINDOW.1 as u32 + 1, 4, 4, color);
        frame.scene()
    };

    for (id, color, passes) in [
        ("ui/plain", MARK, true),
        ("ui/checked", MARK, true),
        ("ui/checked", OTHER, false),
    ] {
        let evaluation = evaluator
            .evaluate_target(&square(color), id)
            .expect("evaluated");
        assert_scored_at_window(&evaluation);
        assert_eq!(luma(&evaluation), None, "{id} {color:?}");
        assert_eq!(evaluation.passed, passes, "{id} {color:?}: {evaluation:?}");
    }
}

/// An uncompressed RGB PNG.
fn encode_png(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
    let mut scanlines = Vec::new();
    for row in pixels.chunks((width * 3) as usize) {
        scanlines.push(0);
        scanlines.extend_from_slice(row);
    }
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::new();
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &header);
    let mut zlib = vec![0x78, 0x01];
    let blocks = scanlines.chunks(65_535).collect::<Vec<_>>();
    for (index, block) in blocks.iter().enumerate() {
        zlib.push(u8::from(index + 1 == blocks.len()));
        let length = block.len() as u16;
        zlib.extend_from_slice(&length.to_le_bytes());
        zlib.extend_from_slice(&(!length).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    let (mut a, mut b) = (1_u32, 0_u32);
    for byte in &scanlines {
        a = (a + u32::from(*byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());
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
