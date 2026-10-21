// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 B3 logic tests (B-train plan §2.3, §5.3 rows T1 to T3): a color probe, a
// template's relative `color_check` in the joint search and the `repeated_anchor` color predicate
// accept the nearest of `expected` and its `alternatives` within the one `max_distance`, and the
// color evidence names the matched color as `expected`. Frames are synthetic: flat neutral
// backgrounds, a noise marker and flat color squares.

use crate::*;
use serde_json::json;

const MARKER: &str = "assets/marker.png";
const BACKGROUND: [u8; 3] = [30, 31, 32];
/// The `expected` color of the marker's check.
const FIRST: [u8; 3] = [200, 40, 40];
/// The marker check's one alternative.
const SECOND: [u8; 3] = [40, 40, 200];
/// A square of neither color.
const NEITHER: [u8; 3] = [90, 90, 90];

#[derive(Debug)]
struct Assets(Vec<u8>);

impl AssetResolver for Assets {
    fn read_asset(&self, path: &str) -> RecognitionPackResult<Vec<u8>> {
        if path == MARKER {
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

    /// `image` drawn with its top-left corner at `(x, y)`.
    fn draw(&mut self, x: u32, y: u32, image: &Frame) {
        for row in 0..image.height {
            let from = (row * image.width * 3) as usize;
            let to = (((y + row) * self.width + x) * 3) as usize;
            let length = (image.width * 3) as usize;
            self.pixels[to..to + length].copy_from_slice(&image.pixels[from..from + length]);
        }
    }

    fn scene(&self) -> Scene {
        Scene::from_rgb8(self.width, self.height, &self.pixels).expect("scene")
    }
}

/// An 8x6 block of deterministic noise: a shifted copy correlates with it near zero.
fn marker() -> Frame {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut frame = Frame::new(8, 6, [0, 0, 0]);
    for value in &mut frame.pixels {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        *value = (state >> 56) as u8;
    }
    frame
}

/// A template target on the marker whose relative check expects `FIRST` with `SECOND` as its
/// one alternative, 4x4 at 10 to the right of and 1 below the match.
fn marked_template() -> Value {
    json!({
        "type": "template", "id": "ui/marker", "template_path": MARKER, "region": "full_frame",
        "threshold": 0.95,
        "color_check": {
            "region": {"mode": "template_relative", "anchor_target_id": "ui/marker",
                       "offset": {"x": 10, "y": 1}, "width": 4, "height": 4},
            "expected": FIRST, "alternatives": [SECOND], "max_distance": 20.0
        }
    })
}

fn evaluator(pack: Value) -> RecognitionEvaluator {
    let marker = marker();
    RecognitionEvaluator::with_asset_resolver(
        load_pack_from_json_str(&pack.to_string()).expect("the pack loads"),
        Arc::new(Assets(encode_png(
            marker.width,
            marker.height,
            &marker.pixels,
        ))),
    )
    .expect("evaluator")
}

/// A schema 0.7 pack of 64x40 with `targets` and, when given, one candidate layout.
fn pack(targets: Vec<Value>, layout: Option<Value>) -> Value {
    let mut pack = json!({
        "schema_version": "0.7",
        "coordinate_space": {"width": 64, "height": 40},
        "defaults": {"match_metric": "ccoeff_normed"},
        "targets": targets
    });
    if let Some(layout) = layout {
        pack["candidate_layouts"] = json!([layout]);
    }
    pack
}

#[test]
fn b3_t1_a_probe_passes_on_its_nearest_alternative_and_names_it_as_expected() {
    // Left: a gray 8 from `expected` and 2 from the second alternative. Right: a blue far from
    // `expected` and 10 from the second alternative.
    let mut frame = Frame::new(8, 4, [108, 100, 100]);
    frame.fill(4, 0, 4, 4, [0, 0, 200]);
    let probe = |id: &str, x: i32, alternatives: Value| {
        json!({
            "type": "color", "id": id, "region": {"x": x, "y": 0, "width": 4, "height": 4},
            "expected": [100, 100, 100], "alternatives": alternatives, "max_distance": 20.0
        })
    };
    let evaluator = evaluator(json!({
        "schema_version": "0.7",
        "coordinate_space": {"width": 8, "height": 4},
        "targets": [
            probe("state/near", 0, json!([[200, 0, 0], [110, 100, 100]])),
            probe("state/far", 4, json!([[200, 0, 0], [0, 0, 210]]))
        ]
    }));

    for (id, matched, distance) in [
        ("state/near", [110_u8, 100, 100], 2.0_f32),
        ("state/far", [0, 0, 210], 10.0),
    ] {
        let evaluation = evaluator
            .evaluate_target(&frame.scene(), id)
            .expect("evaluated");
        assert!(evaluation.passed, "{id}: {evaluation:?}");
        let color = evaluation.color.expect("color evidence");
        assert_eq!(color.expected, matched, "{id}");
        assert_eq!(color.distance, distance, "{id}");
    }
}

#[test]
fn b3_t2_the_joint_search_accepts_a_template_candidate_on_an_alternative() {
    // Two equal template candidates: the first one's square holds neither color, the second
    // one's the alternative.
    let mut frame = Frame::new(64, 40, BACKGROUND);
    for (x, square) in [(4, NEITHER), (34, SECOND)] {
        frame.draw(x, 4, &marker());
        frame.fill(x + 10, 5, 4, 4, square);
    }

    let evaluation = evaluator(pack(vec![marked_template()], None))
        .evaluate_target(&frame.scene(), "ui/marker")
        .expect("evaluated");

    assert!(evaluation.passed, "{evaluation:?}");
    let template = evaluation.template.expect("template evidence");
    assert_eq!((template.x, template.y), (34, 4));
    let color = evaluation.color.expect("color evidence");
    assert_eq!(
        color.region,
        Some(PackRect {
            x: 44,
            y: 5,
            width: 4,
            height: 4
        })
    );
    assert_eq!(color.expected, SECOND);
}

#[test]
fn b3_t3_a_repeated_anchor_takes_every_position_whose_square_has_a_declared_color() {
    // Markers with `expected`'s square, a square of neither color and the alternative's square.
    let mut frame = Frame::new(64, 40, BACKGROUND);
    for (x, y, square) in [(4, 4, FIRST), (34, 4, NEITHER), (4, 20, SECOND)] {
        frame.draw(x, y, &marker());
        frame.fill(x + 10, y + 1, 4, 4, square);
    }
    let square = json!({
        "type": "color", "id": "state/square",
        "region": {"x": 0, "y": 0, "width": 4, "height": 4}, "expected": FIRST
    });
    let layout = json!({
        "id": "layout/rows", "page_id": "list_page", "kind": "repeated_anchor",
        "anchor": "ui/marker", "max_instances": 4, "order": "top_to_bottom",
        "instance_rect": {"x": -2, "y": -2, "width": 20, "height": 10},
        "click": {"x": 0, "y": 0, "width": 8, "height": 6},
        "features": [{"name": "marked", "value": "passed", "target": "state/square",
                      "offset": {"x": 10, "y": 1}}]
    });
    let evaluator = evaluator(pack(vec![marked_template(), square], Some(layout)));

    let projection = evaluator
        .scene_context(&frame.scene())
        .project_candidates("layout/rows")
        .expect("projection");

    let origins = projection
        .candidates()
        .iter()
        .map(|candidate| (candidate.rect.x, candidate.rect.y))
        .collect::<Vec<_>>();
    assert_eq!(origins, [(2, 2), (2, 18)]);
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
