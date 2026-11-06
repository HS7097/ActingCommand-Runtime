// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #404 logic tests of the whole-frame anchor enumeration (design rd-1031/perf-407
// model-gpu0-404.md §2.2, §2.3). FS-1: a masked anchor declared at several factors is admitted
// and enumerated on the exact score map, its instance rectangle, click and feature offsets
// scaled by the factor each match hit; one object found at several factors is one instance; and
// sorting and suppression count against the 5 s search limit. FS-2: an `anchor_milli` feature
// carries each instance's own match score in milli, rounded down. Frames are synthetic: flat
// neutral backgrounds, noise icons, a smooth bump and flat color squares.

use crate::codes::{RecognitionPackCode, RecognitionPackLocation};
use crate::*;
use actingcommand_contract::CandidateFeature;
use actingcommand_contract::candidate_projection::{CandidateProjection, CandidateRect};
use actingcommand_recognition as recognition;
use actingcommand_recognition::resample::{resample_pixels, scaled_length};
use serde_json::{Value, json};
use std::cell::Cell;
use std::time::{Duration, Instant};

const ICON: &str = "assets/icon.png";
const GREEN: [u8; 3] = [0, 200, 0];
const LAYOUT: &str = "layout/marks";

/// A gray image, row-major.
#[derive(Clone)]
struct Gray {
    width: u32,
    height: u32,
    values: Vec<u8>,
}

impl Gray {
    fn from_fn(width: u32, height: u32, mut value: impl FnMut(u32, u32) -> u8) -> Self {
        let mut values = Vec::with_capacity((width * height) as usize);
        for y in 0..height {
            for x in 0..width {
                values.push(value(x, y));
            }
        }
        Self {
            width,
            height,
            values,
        }
    }

    /// Deterministic noise in 1..=255.
    fn noise(width: u32, height: u32, seed: u64) -> Self {
        let mut state = seed;
        Self::from_fn(width, height, |_, _| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            1 + ((state >> 33) % 255) as u8
        })
    }

    /// A smooth bump `size` pixels square: a Gaussian of a quarter of its size over 20.
    fn bump(size: u32) -> Self {
        let center = (f64::from(size) - 1.0) / 2.0;
        let sigma = f64::from(size) / 4.0;
        Self::from_fn(size, size, |x, y| {
            let (dx, dy) = (f64::from(x) - center, f64::from(y) - center);
            let falloff = (-(dx * dx + dy * dy) / (2.0 * sigma * sigma)).exp();
            (20.0 + 235.0 * falloff).round() as u8
        })
    }

    /// This image resampled by `factor_milli` with the primitive every search uses.
    fn scaled(&self, factor_milli: u32) -> Self {
        let (width, height) = (
            scaled_length(self.width, factor_milli),
            scaled_length(self.height, factor_milli),
        );
        Self {
            width,
            height,
            values: resample_pixels(&self.values, (self.width, self.height), 1, (width, height))
                .expect("resampled"),
        }
    }

    /// Equal red, green and blue, whose gray value is exactly the image's.
    fn png(&self) -> Vec<u8> {
        let rgb = self
            .values
            .iter()
            .flat_map(|value| [*value; 3])
            .collect::<Vec<_>>();
        encode_png(self.width, self.height, &rgb)
    }
}

/// An RGB frame.
struct Frame {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Frame {
    fn new(width: u32, height: u32, gray: u8) -> Self {
        Self {
            width,
            height,
            pixels: vec![gray; (width * height * 3) as usize],
        }
    }

    /// `image` drawn in gray with its top-left corner at `(x, y)`, each value passed through
    /// `paint`.
    fn draw(&mut self, x: u32, y: u32, image: &Gray, paint: impl Fn(u8) -> u8) {
        for row in 0..image.height {
            for column in 0..image.width {
                let value = paint(image.values[(row * image.width + column) as usize]);
                let at = (((y + row) * self.width + x + column) * 3) as usize;
                self.pixels[at..at + 3].copy_from_slice(&[value; 3]);
            }
        }
    }

    fn fill(&mut self, x: u32, y: u32, width: u32, height: u32, color: [u8; 3]) {
        for row in y..y + height {
            for column in x..x + width {
                let at = ((row * self.width + column) * 3) as usize;
                self.pixels[at..at + 3].copy_from_slice(&color);
            }
        }
    }

    fn scene(&self) -> Scene {
        Scene::from_rgb8(self.width, self.height, &self.pixels).expect("scene")
    }
}

#[derive(Debug)]
struct Assets(Vec<u8>);

impl AssetResolver for Assets {
    fn read_asset(&self, path: &str) -> RecognitionPackResult<Vec<u8>> {
        if path == ICON {
            Ok(self.0.clone())
        } else {
            Err(RecognitionPackError::fatal(format!("no asset {path}")))
        }
    }
}

fn evaluator(pack: Value, icon: &Gray) -> RecognitionEvaluator {
    RecognitionEvaluator::with_asset_resolver(
        load_pack_from_json_str(&pack.to_string()).expect("the pack loads"),
        Arc::new(Assets(icon.png())),
    )
    .expect("evaluator")
}

/// A template target on the icon over `region`.
fn icon_target(id: &str, region: Value, threshold: f64) -> Value {
    json!({"type": "template", "id": id, "template_path": ICON, "region": region,
           "threshold": threshold})
}

/// A 4x4 color target expecting `GREEN`.
fn square() -> Value {
    json!({"type": "color", "id": "state/square",
           "region": {"x": 0, "y": 0, "width": 4, "height": 4}, "expected": GREEN})
}

/// A schema 0.7 `ccoeff_normed` pack of `width` by `height` with `targets` and `layout`.
fn pack(width: u32, height: u32, targets: Vec<Value>, layout: Value) -> Value {
    json!({
        "schema_version": "0.7",
        "coordinate_space": {"width": width, "height": height},
        "defaults": {"match_metric": "ccoeff_normed"},
        "targets": targets,
        "candidate_layouts": [layout]
    })
}

/// A `repeated_anchor` layout on `anchor` that suppresses any overlap.
fn layout(
    anchor: &str,
    instance_rect: CandidateRect,
    click: CandidateRect,
    features: Value,
) -> Value {
    let rect = |rect: CandidateRect| json!({"x": rect.x, "y": rect.y, "width": rect.width, "height": rect.height});
    json!({
        "id": LAYOUT, "page_id": "room", "kind": "repeated_anchor", "anchor": anchor,
        "max_instances": 8, "order": "top_to_bottom", "suppress_iou_milli": 0,
        "instance_rect": rect(instance_rect), "click": rect(click), "features": features
    })
}

fn rect(x: i32, y: i32, width: i32, height: i32) -> CandidateRect {
    CandidateRect {
        x,
        y,
        width,
        height,
    }
}

/// Each candidate's rectangle and click.
fn boxes(projection: &CandidateProjection) -> Vec<(CandidateRect, CandidateRect)> {
    projection
        .candidates()
        .iter()
        .map(|candidate| (candidate.rect, candidate.click))
        .collect()
}

#[test]
fn masked_and_scaled_anchors_enumerate_on_the_exact_map() {
    let icon = Gray::noise(24, 18, 0x0404_0101);
    // The mask scores the gray values 64..=255; the frame shows the icon with every pixel outside
    // the mask inverted, so only the masked search matches it.
    let shown = |value: u8| if value >= 64 { value } else { 255 - value };
    let mut frame = Frame::new(320, 240, 30);
    frame.draw(40, 30, &icon, shown);
    frame.draw(200, 150, &icon.scaled(750), shown);
    // The squares at each copy's offset (24, 0), scaled: (24, 0) at 1000, (18, 0) at 750.
    frame.fill(64, 30, 4, 4, GREEN);
    frame.fill(218, 150, 4, 4, GREEN);
    let masked = |id: &str, region: Value| {
        let mut target = icon_target(id, region, 0.9);
        target["mask"] = json!({"type": "range", "lower": 64, "upper": 255});
        target["scales"] = json!({"factors_milli": [1000, 750]});
        target
    };
    let mut plain = icon_target("ui/plain", json!("full_frame"), 0.9);
    plain["scales"] = json!({"factors_milli": [1000, 750]});
    let evaluator = evaluator(
        pack(
            320,
            240,
            vec![
                masked("ui/mark", json!("full_frame")),
                // The oracle: the exact search of each copy's neighbourhood alone.
                masked(
                    "probe/a",
                    json!({"x": 30, "y": 20, "width": 50, "height": 40}),
                ),
                masked(
                    "probe/b",
                    json!({"x": 190, "y": 140, "width": 40, "height": 34}),
                ),
                plain,
                square(),
            ],
            layout(
                "ui/mark",
                rect(-4, -4, 32, 26),
                rect(4, 4, 16, 10),
                json!([{"name": "open", "value": "passed", "target": "state/square",
                        "offset": {"x": 24, "y": 0}}]),
            ),
        ),
        &icon,
    );
    evaluator
        .admit_template_assets()
        .expect("admission takes a masked anchor declared at two factors");
    let scene = frame.scene();
    let context = evaluator.scene_context(&scene);

    let projection = context.project_candidates(LAYOUT).expect("projection");

    let probe = |id: &str| {
        let template = context
            .evaluate_target(id)
            .expect("probe")
            .template
            .expect("template evidence");
        (template.x, template.y, template.scale_milli)
    };
    assert_eq!(probe("probe/a"), (40, 30, Some(1000)));
    assert_eq!(probe("probe/b"), (200, 150, Some(750)));
    // Each instance stands at its exact match: at 750, (-4, -4, 32, 26) is (-3, -3, 24, 20) and
    // (4, 4, 16, 10) is (3, 3, 12, 8), and the feature reads its square at (18, 0).
    assert_eq!(
        boxes(&projection),
        [
            (rect(36, 26, 32, 26), rect(44, 34, 16, 10)),
            (rect(197, 147, 24, 20), rect(203, 153, 12, 8)),
        ]
    );
    for candidate in projection.candidates() {
        assert!(
            matches!(
                candidate.features.get("open"),
                Some(CandidateFeature::Boolean { value: true, .. })
            ),
            "{candidate:?}"
        );
    }
    // The same template without its mask scores the inverted pixels too and matches neither.
    assert_eq!(
        context
            .evaluate_target_all("ui/plain", 0)
            .expect("enumeration"),
        TargetSearch::Complete(Vec::new())
    );
}

#[test]
fn one_object_across_scales_is_one_instance() {
    let bump = Gray::bump(20);
    let mut frame = Frame::new(160, 120, 20);
    frame.draw(60, 40, &bump, |value| value);
    let scaled = |id: &str, factors: &[u32]| {
        let mut target = icon_target(id, json!("full_frame"), 0.8);
        target["scales"] = json!({"factors_milli": factors});
        target
    };
    let evaluator = evaluator(
        pack(
            160,
            120,
            vec![
                scaled("ui/mark", &[1000, 900, 1100]),
                scaled("ui/at_900", &[900]),
                scaled("ui/at_1100", &[1100]),
                square(),
            ],
            layout(
                "ui/mark",
                rect(-2, -2, 24, 24),
                rect(4, 4, 12, 12),
                json!([{"name": "lit", "value": "passed", "target": "state/square",
                        "offset": {"x": 8, "y": 8}}]),
            ),
        ),
        &bump,
    );
    evaluator
        .admit_template_assets()
        .expect("admission takes the anchor's factors");
    let scene = frame.scene();
    let context = evaluator.scene_context(&scene);
    let factors = |id: &str| match context.evaluate_target_all(id, 0).expect("enumeration") {
        TargetSearch::Complete(matches) => matches
            .iter()
            .map(|matched| matched.scale_milli)
            .collect::<Vec<_>>(),
        TargetSearch::Incomplete { .. } => panic!("the system clock passed the limit"),
    };

    // Each factor alone finds the bump once.
    assert_eq!(factors("ui/at_900"), [Some(900)]);
    assert_eq!(factors("ui/at_1100"), [Some(1100)]);
    // All three find it once, at the declared size, whose copy scores highest.
    assert_eq!(factors("ui/mark"), [Some(1000)]);
    let projection = context.project_candidates(LAYOUT).expect("projection");
    assert_eq!(
        boxes(&projection),
        [(rect(58, 38, 24, 24), rect(64, 44, 12, 12))]
    );
}

thread_local! {
    static START: Instant = Instant::now();
    static READS: Cell<u64> = const { Cell::new(0) };
    static LATE_FROM: Cell<u64> = const { Cell::new(u64::MAX) };
}

/// Counts its readings and reads the start instant before reading `LATE_FROM`, then a time past
/// the 5 s limit.
fn counted_clock() -> Instant {
    let read = READS.with(|reads| {
        let read = reads.get();
        reads.set(read + 1);
        read
    });
    let start = START.with(|start| *start);
    if read >= LATE_FROM.with(Cell::get) {
        start + Duration::from_secs(6)
    } else {
        start
    }
}

#[test]
fn sorting_and_suppression_count_against_the_search_limit() {
    let bump = Gray::bump(20);
    let mut frame = Frame::new(160, 120, 20);
    frame.draw(30, 30, &bump, |value| value);
    frame.draw(100, 60, &bump, |value| value);
    let evaluator = evaluator(
        pack(
            160,
            120,
            vec![icon_target("ui/mark", json!("full_frame"), 0.8), square()],
            layout(
                "ui/mark",
                rect(-2, -2, 24, 24),
                rect(4, 4, 12, 12),
                json!([{"name": "lit", "value": "passed", "target": "state/square",
                        "offset": {"x": 8, "y": 8}}]),
            ),
        ),
        &bump,
    );
    let scene = frame.scene();
    // The positions at or above the threshold on the exact score map: a smooth bump passes at
    // its neighbours too, so suppression keeps one of each copy's.
    let found = scene
        .exact_score_maps(
            &bump.png(),
            None,
            recognition::MatchMetric::CorrelationCoefficientNormalized,
            recognition::TemplateMatchTimingStage::Exact,
            None,
            &[1000],
            &mut recognition::RegionSpectra::default(),
            &recognition::SearchClock::system(),
        )
        .expect("score map")
        .matches_at_least(0.8, |_| Ok(true))
        .expect("matches")
        .len();
    assert!(found > 2, "{found} positions");

    // The whole projection reads the clock `reads` times; the search is its only reader.
    READS.with(|reads| reads.set(0));
    let projection = evaluator
        .scene_context(&scene)
        .with_search_clock(counted_clock)
        .project_candidates(LAYOUT)
        .expect("a clock that stays at its start passes no limit");
    assert_eq!(projection.candidates().len(), 2);
    let reads = READS.with(Cell::get);

    // Past the limit at the last reading only, after every position is scored, collected, sorted
    // and suppressed: the search fails, and names every position it had found.
    READS.with(|reads| reads.set(0));
    LATE_FROM.with(|late| late.set(reads - 1));
    let incomplete = evaluator
        .scene_context(&scene)
        .with_search_clock(counted_clock)
        .project_candidates(LAYOUT)
        .expect_err("suppression ran past the limit");
    assert_eq!(
        incomplete.code(),
        RecognitionPackCode::CandidateSearchIncomplete.code()
    );
    assert_eq!(incomplete.item(), "anchor");
    assert_eq!(
        incomplete.detail(),
        format!(
            "stage={} timeout_ms=5000 count={found}",
            RecognitionPackLocation::CandidateSearchRepeatedAnchor.as_str()
        )
    );
}

#[test]
fn anchor_milli_carries_the_match_score_floor_milli() {
    let icon = Gray::noise(16, 12, 0x0404_0201);
    // An exact copy and two copies with some pixels brightened, which score below 1.
    let brighten = |every: usize, by: u8| {
        let mut copy = icon.clone();
        for value in copy.values.iter_mut().step_by(every) {
            *value = value.saturating_add(by);
        }
        copy
    };
    let mut frame = Frame::new(200, 120, 30);
    frame.draw(20, 20, &icon, |value| value);
    frame.draw(100, 20, &brighten(5, 50), |value| value);
    frame.draw(60, 80, &brighten(3, 60), |value| value);
    let mut anchored = layout(
        "ui/mark",
        rect(-2, -2, 20, 16),
        rect(2, 2, 12, 8),
        json!([{"name": "match", "value": "anchor_milli"}]),
    );
    // The third copy lies below the readable band.
    anchored["readable_band"] = json!({"x": 0, "y": 0, "width": 200, "height": 60});
    let probe = |id: &str, x: i32, y: i32| {
        icon_target(
            id,
            json!({"x": x - 8, "y": y - 8, "width": 32, "height": 28}),
            0.8,
        )
    };
    let evaluator = evaluator(
        pack(
            200,
            120,
            vec![
                icon_target("ui/mark", json!("full_frame"), 0.8),
                // The oracle: the exact search of each copy's neighbourhood alone.
                probe("probe/a", 20, 20),
                probe("probe/b", 100, 20),
            ],
            anchored,
        ),
        &icon,
    );
    let scene = frame.scene();
    let context = evaluator.scene_context(&scene);

    let projection = context.project_candidates(LAYOUT).expect("projection");

    let floor_milli = |id: &str| {
        let template = context
            .evaluate_target(id)
            .expect("probe")
            .template
            .expect("template evidence");
        (f64::from(template.score) * 1000.0).floor() as i64
    };
    let (exact, brightened) = (floor_milli("probe/a"), floor_milli("probe/b"));
    assert!(
        (800..exact).contains(&brightened),
        "{brightened} below {exact}"
    );
    let candidates = projection.candidates();
    assert_eq!(
        candidates
            .iter()
            .map(|candidate| (candidate.rect.x, candidate.rect.y, candidate.actionable))
            .collect::<Vec<_>>(),
        [(18, 18, true), (98, 18, true), (58, 78, false)]
    );
    for (candidate, milli) in candidates.iter().zip([exact, brightened]) {
        // The feature is the score in milli, rounded down, and its own confidence.
        assert_eq!(
            candidate.features.get("match"),
            Some(&CandidateFeature::Integer {
                value: milli,
                confidence: Some(milli),
            })
        );
        assert_eq!(
            serde_json::to_value(&candidate.features["match"]).expect("feature JSON"),
            json!({"type": "integer", "value": milli, "confidence": milli})
        );
    }
    // An instance that is not actionable carries no feature.
    assert!(candidates[2].features.is_empty());
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
