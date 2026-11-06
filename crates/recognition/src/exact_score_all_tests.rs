// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #404 FS-1 logic tests of the whole-frame enumeration on the exact score map: an
// unmasked, unscaled anchor gives the matches the direct scorer gives, bit for bit, on both dot
// engines, and a full-frame anchor is summed by the transform engine. Frames are synthetic
// noise; the clock never moves, so no test reads the time limit.

use super::*;
use crate::{
    MatchCandidate, Rect, Scene, SearchClock, SearchWindow, TemplateSearch, overlap_suppresses,
    score_window, template_match_from_candidate,
};
use image::{DynamicImage, ImageFormat, Luma};
use std::io::Cursor;
use std::time::Instant;

const METRICS: [MatchMetric; 2] = [
    MatchMetric::CrossCorrelationNormalized,
    MatchMetric::CorrelationCoefficientNormalized,
];

/// A deterministic generator of neutral noise.
struct Noise(u64);

impl Noise {
    fn image(&mut self, width: u32, height: u32) -> GrayImage {
        GrayImage::from_fn(width, height, |_, _| {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            Luma([(self.0 >> 56) as u8])
        })
    }
}

fn gray_png(image: &GrayImage) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    DynamicImage::ImageLuma8(image.clone())
        .write_to(&mut out, ImageFormat::Png)
        .expect("encode PNG");
    out.into_inner()
}

/// A scene whose gray image is `image`.
fn gray_scene(image: &GrayImage) -> Scene {
    let pixels = image
        .as_raw()
        .iter()
        .flat_map(|value| [*value; 3])
        .collect::<Vec<_>>();
    Scene::from_rgb8(image.width(), image.height(), &pixels).expect("scene")
}

/// A clock that never moves: these tests check results, never the time limit.
fn frozen_clock() -> SearchClock {
    let now = Instant::now();
    SearchClock::new(move || now)
}

/// A match as bits: its box, and its raw and normalized scores.
fn bits(found: &TemplateMatch) -> (i32, i32, i32, i32, u32, u32) {
    (
        found.x,
        found.y,
        found.width,
        found.height,
        found.raw_score.to_bits(),
        found.score.to_bits(),
    )
}

/// The anchor enumeration exactly as it was before it moved onto the exact score map: every
/// position scored by the direct scorer in row-major order, the threshold and the predicate
/// asked there, then the matches taken by score, `y` and `x` and suppressed by overlap.
fn matches_before(
    scene: &Scene,
    template_png: &[u8],
    region: Option<Rect>,
    metric: MatchMetric,
    threshold: f32,
    suppress_iou_milli: u16,
    accept: impl Fn(TemplateMatch) -> bool,
) -> Vec<TemplateMatch> {
    let (search, template, offset_x, offset_y) = scene
        .prepare_template_match(template_png, region)
        .expect("prepared search");
    let stats = TemplateStats::new(&template, metric).expect("template stats");
    let integrals = IntegralImages::new(&search);
    let window = SearchWindow::full(&search, &template);
    let mut found = Vec::new();
    for y in window.min_y..=window.max_y {
        for x in window.min_x..=window.max_x {
            let raw_score = score_window(&search, &stats, &integrals, metric, x, y);
            let candidate = template_match_from_candidate(
                MatchCandidate { x, y, raw_score },
                &template,
                offset_x,
                offset_y,
            )
            .expect("candidate");
            if candidate.score >= threshold && accept(candidate) {
                found.push(candidate);
            }
        }
    }
    found.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then(a.y.cmp(&b.y))
            .then(a.x.cmp(&b.x))
    });
    let mut taken: Vec<TemplateMatch> = Vec::new();
    for candidate in found {
        if !taken
            .iter()
            .any(|taken| overlap_suppresses(taken, &candidate, suppress_iou_milli))
        {
            taken.push(candidate);
        }
    }
    taken
}

#[test]
fn an_unmasked_unscaled_anchor_projects_byte_identical_to_the_direct_scorer() {
    let mut noise = Noise(0x0404_0001);
    let mut frame = noise.image(200, 150);
    let large = noise.image(18, 12);
    let small = noise.image(5, 4);
    // Whole copies of both templates inside the region at an `x` the predicate below admits, so
    // every combination has copies the direct scorer finds; one copy of the large one at an `x`
    // it refuses; one copy of the large one cut by the region's edge; one of the small one
    // outside the region.
    for (x, y) in [(31, 20), (140, 100), (61, 110)] {
        image::imageops::replace(&mut frame, &large, x, y);
    }
    for (x, y) in [(101, 30), (12, 80)] {
        image::imageops::replace(&mut frame, &small, x, y);
    }
    let scene = gray_scene(&frame);
    let region = Rect {
        x: 20,
        y: 10,
        width: 160,
        height: 110,
    };
    // A deterministic stand-in for the relative color predicate, asked per position.
    let accept = |candidate: TemplateMatch| candidate.x.rem_euclid(5) != 0;

    let mut engines = Vec::new();
    for template in [&large, &small] {
        let png = gray_png(template);
        for region in [None, Some(region)] {
            let (search, prepared, _, _) = scene
                .prepare_template_match(&png, region)
                .expect("prepared search");
            engines.push(choose_engine(&search, &prepared));
            for metric in METRICS {
                let thresholds = match metric {
                    MatchMetric::CrossCorrelationNormalized => [0.85, 0.99],
                    MatchMetric::CorrelationCoefficientNormalized => [0.5, 0.99],
                };
                for threshold in thresholds {
                    for suppress_iou_milli in [0, 500] {
                        let label = format!(
                            "{}x{} {metric:?} {region:?} {threshold} {suppress_iou_milli}",
                            template.width(),
                            template.height()
                        );
                        let after = match scene
                            .match_template_all(
                                &png,
                                region,
                                metric,
                                threshold,
                                suppress_iou_milli,
                                |candidate| Ok(accept(candidate)),
                            )
                            .expect("enumeration")
                        {
                            TemplateSearch::Complete(matches) => matches,
                            TemplateSearch::Incomplete { .. } => {
                                panic!("{label}: the system clock passed the limit")
                            }
                        };
                        let before = matches_before(
                            &scene,
                            &png,
                            region,
                            metric,
                            threshold,
                            suppress_iou_milli,
                            accept,
                        );
                        assert!(!before.is_empty(), "{label}: the copies match");
                        assert_eq!(
                            after.iter().map(bits).collect::<Vec<_>>(),
                            before.iter().map(bits).collect::<Vec<_>>(),
                            "{label}"
                        );
                    }
                }
            }
        }
    }
    // Both dot engines took part.
    assert!(engines.contains(&DotEngine::Direct), "{engines:?}");
    assert!(engines.contains(&DotEngine::Transform), "{engines:?}");
}

#[test]
fn a_full_frame_anchor_uses_the_transform_engine() {
    let mut noise = Noise(0x0404_0002);
    let mut frame = noise.image(1280, 720);
    let template = noise.image(48, 48);
    let copies = [(900, 80), (64, 300), (640, 600)];
    for (x, y) in copies {
        image::imageops::replace(&mut frame, &template, x, y);
    }
    assert_eq!(choose_engine(&frame, &template), DotEngine::Transform);
    let scene = gray_scene(&frame);
    let mut spectra = RegionSpectra::default();

    let search = scene
        .match_template_all_scaled(
            &gray_png(&template),
            None,
            MatchMetric::CorrelationCoefficientNormalized,
            None,
            &[1000],
            0.9,
            0,
            |_| Ok(true),
            &mut spectra,
            &frozen_clock(),
        )
        .expect("enumeration");

    // The frame's transform was computed, and kept for the next template of this frame.
    assert!(
        spectra
            .entries
            .iter()
            .any(|entry| entry.bounds == (0, 0, 1280, 720) && entry.plane == Plane::Gray),
        "the whole frame's transform"
    );
    let TemplateSearch::Complete(matches) = search else {
        panic!("a frozen clock never passes the limit");
    };
    // Exactly the three copies, each scoring 1 up to the rounding of its normalization.
    let mut found = matches
        .iter()
        .map(|found| {
            (
                found.matched.x,
                found.matched.y,
                found.matched.width,
                found.scale_milli,
            )
        })
        .collect::<Vec<_>>();
    found.sort();
    assert_eq!(
        found,
        [
            (64, 300, 48, 1000),
            (640, 600, 48, 1000),
            (900, 80, 48, 1000)
        ]
    );
    assert!(matches.iter().all(|found| found.matched.score >= 0.999));
}
