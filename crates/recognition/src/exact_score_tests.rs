// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #396 REC-2 logic tests of the exact score map: its integer sums, its scores against
//! the direct scorer, its two engines, the unchanged results of the two searches it took over,
//! and the non-gating timing of a full-frame search. Frames are synthetic noise.

use super::*;
use crate::{
    MatchCandidate, Rect, Scene, SearchClock, SearchWindow, TEMPLATE_MATCH_TIMEOUT,
    exact_metric_match, score_window, template_match_from_candidate,
};
use image::{DynamicImage, ImageFormat, Luma};
use std::io::{Cursor, Write};
use std::time::Instant;

const METRICS: [MatchMetric; 2] = [
    MatchMetric::CrossCorrelationNormalized,
    MatchMetric::CorrelationCoefficientNormalized,
];

/// A deterministic generator of neutral noise.
struct Noise(u64);

impl Noise {
    fn step(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn below(&mut self, limit: u64) -> u64 {
        self.step() % limit
    }

    fn image(&mut self, width: u32, height: u32) -> GrayImage {
        GrayImage::from_fn(width, height, |_, _| Luma([self.below(256) as u8]))
    }
}

/// A deadline whose clock never moves: these tests check results, never the time limit.
fn frozen_deadline() -> TemplateMatchDeadline {
    let now = Instant::now();
    TemplateMatchDeadline::with_clock(TEMPLATE_MATCH_TIMEOUT, SearchClock::new(move || now))
}

fn search<'a>(
    search: &'a GrayImage,
    template: &'a GrayImage,
    metric: MatchMetric,
) -> ScoreSearch<'a> {
    ScoreSearch {
        search,
        template,
        metric,
        offset_x: 0,
        offset_y: 0,
    }
}

/// `sum(T * I)` of the template placed at `(x, y)`, in integers.
fn integer_sum(search: &GrayImage, template: &GrayImage, x: u32, y: u32) -> u64 {
    let mut sum = 0_u64;
    for ty in 0..template.height() {
        for tx in 0..template.width() {
            sum += u64::from(search.get_pixel(x + tx, y + ty)[0])
                * u64::from(template.get_pixel(tx, ty)[0]);
        }
    }
    sum
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

#[test]
fn transformed_sums_are_the_direct_integer_sums_and_scores_are_the_direct_scorers_bits() {
    let mut noise = Noise(0x5eed_0396);
    let frame = noise.image(1280, 720);
    let mut spectra = RegionSpectra::default();
    for (size, reduced) in [(32, (80, 56)), (64, (144, 88)), (256, (272, 264))] {
        let template = noise.image(size, size);

        // Every position of a reduced frame.
        let small = noise.image(reduced.0, reduced.1);
        let sums = dot_sums(
            &search(&small, &template, METRICS[0]),
            DotEngine::Transform,
            None,
            &frozen_deadline(),
            TemplateMatchTimingStage::Exact,
        )
        .expect("transformed sums");
        let columns = small.width() - size + 1;
        let rows = small.height() - size + 1;
        assert_eq!(sums.len(), (columns * rows) as usize);
        for y in 0..rows {
            for x in 0..columns {
                assert_eq!(
                    sums[(y * columns + x) as usize],
                    integer_sum(&small, &template, x, y) as f64,
                    "{size} px template at ({x}, {y})"
                );
            }
        }
        for metric in METRICS {
            let map = ExactScoreMap::compute(
                search(&small, &template, metric),
                DotEngine::Transform,
                None,
                frozen_deadline(),
                TemplateMatchTimingStage::Exact,
            )
            .expect("score map");
            let stats = TemplateStats::new(&template, metric).expect("template stats");
            let integrals = IntegralImages::new(&small);
            for y in 0..rows {
                for x in 0..columns {
                    assert_eq!(
                        map.raw_scores()[(y * columns + x) as usize].to_bits(),
                        score_window(&small, &stats, &integrals, metric, x, y).to_bits(),
                        "{size} px template, {metric:?}, at ({x}, {y})"
                    );
                }
            }
        }

        // Sampled positions of the whole 1280x720 frame.
        let sums = dot_sums(
            &search(&frame, &template, METRICS[0]),
            DotEngine::Transform,
            Some(&mut spectra),
            &frozen_deadline(),
            TemplateMatchTimingStage::Exact,
        )
        .expect("full-frame sums");
        let columns = frame.width() - size + 1;
        let rows = frame.height() - size + 1;
        for _ in 0..48 {
            let x = noise.below(u64::from(columns)) as u32;
            let y = noise.below(u64::from(rows)) as u32;
            assert_eq!(
                sums[(y * columns + x) as usize],
                integer_sum(&frame, &template, x, y) as f64,
                "{size} px template at ({x}, {y}) of the full frame"
            );
        }
    }
}

#[test]
fn the_direct_and_transform_engines_give_the_same_map() {
    let mut noise = Noise(0x0396_0002);
    let frame = noise.image(120, 90);
    let template = noise.image(20, 14);
    let direct = dot_sums(
        &search(&frame, &template, METRICS[0]),
        DotEngine::Direct,
        None,
        &frozen_deadline(),
        TemplateMatchTimingStage::Exact,
    )
    .expect("direct sums");
    let transformed = dot_sums(
        &search(&frame, &template, METRICS[0]),
        DotEngine::Transform,
        None,
        &frozen_deadline(),
        TemplateMatchTimingStage::Exact,
    )
    .expect("transformed sums");
    assert_eq!(direct, transformed);
    for metric in METRICS {
        let map = |engine| {
            ExactScoreMap::compute(
                search(&frame, &template, metric),
                engine,
                None,
                frozen_deadline(),
                TemplateMatchTimingStage::Exact,
            )
            .expect("score map")
        };
        let bits = |map: &ExactScoreMap| {
            map.raw_scores()
                .iter()
                .map(|score| score.to_bits())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            bits(&map(DotEngine::Direct)),
            bits(&map(DotEngine::Transform)),
            "{metric:?}"
        );
    }
}

/// The joint template/color selection exactly as it was before the exact score map took it
/// over: every position scored by the direct scorer, in row-major order.
fn selection_before(
    scene: &Scene,
    template_png: &[u8],
    region: Option<Rect>,
    metric: MatchMetric,
    threshold: f32,
    mut accept: impl FnMut(TemplateMatch) -> RecognitionResult<bool>,
) -> TemplateMatchSelection {
    let (search, template, offset_x, offset_y) = scene
        .prepare_template_match(template_png, region)
        .expect("prepared search");
    let stats = TemplateStats::new(&template, metric).expect("template stats");
    let integrals = IntegralImages::new(&search);
    let window = SearchWindow::full(&search, &template);
    let mut best: Option<TemplateMatch> = None;
    let mut accepted: Option<TemplateMatch> = None;
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
            if best.is_none_or(|best| raw_score > best.raw_score) {
                best = Some(candidate);
            }
            if candidate.score >= threshold
                && accepted.is_none_or(|best| raw_score > best.raw_score)
                && accept(candidate).expect("acceptance")
            {
                accepted = Some(candidate);
            }
        }
    }
    TemplateMatchSelection {
        best_template: best.expect("a candidate"),
        accepted,
    }
}

#[test]
fn the_filtered_and_exact_matches_keep_their_results_byte_for_byte() {
    let mut noise = Noise(0x0396_0003);
    let mut frame = noise.image(200, 150);
    let template = noise.image(18, 12);
    // Two copies, so that the acceptance decides between them.
    image::imageops::replace(&mut frame, &template, 30, 20);
    image::imageops::replace(&mut frame, &template, 140, 100);
    let scene = gray_scene(&frame);
    let png = gray_png(&template);
    // A deterministic stand-in for a same-frame color check.
    let accept = |candidate: TemplateMatch| Ok(candidate.x.rem_euclid(3) != 0);
    for metric in METRICS {
        for region in [
            None,
            Some(Rect {
                x: 20,
                y: 10,
                width: 160,
                height: 130,
            }),
        ] {
            for threshold in [0.0, 0.5, 0.99] {
                let after = scene
                    .match_template_with_filter(&png, region, metric, threshold, accept)
                    .expect("joint match");
                let before = selection_before(&scene, &png, region, metric, threshold, accept);
                assert_eq!(
                    format!("{after:?}"),
                    format!("{before:?}"),
                    "{metric:?} {region:?} {threshold}"
                );
            }
        }
    }

    let metric = MatchMetric::CorrelationCoefficientNormalized;
    for region in [
        None,
        Some(Rect {
            x: 120,
            y: 80,
            width: 60,
            height: 50,
        }),
    ] {
        let after = scene
            .match_template_with_metric(&png, region, metric)
            .expect("exact match");
        let (search, template, offset_x, offset_y) = scene
            .prepare_template_match(&png, region)
            .expect("prepared search");
        assert!(!crate::should_use_fast_path(&search, &template));
        let before = exact_metric_match(
            &search,
            &template,
            metric,
            offset_x,
            offset_y,
            SearchWindow::full(&search, &template),
            &frozen_deadline(),
        )
        .expect("direct exact match");
        assert_eq!(format!("{after:?}"), format!("{before:?}"), "{region:?}");
    }
}

#[test]
fn a_full_frame_single_template_search_prints_its_time() {
    let mut noise = Noise(0x0396_0004);
    let mut frame = noise.image(1280, 720);
    let mut report = String::new();
    for size in [32, 64] {
        let template = noise.image(size, size);
        image::imageops::replace(&mut frame, &template, 700, 400);
        let scene = gray_scene(&frame);
        let png = gray_png(&template);
        let now = Instant::now();
        let started = Instant::now();
        let map = scene
            .exact_score_map(
                &png,
                None,
                MatchMetric::CrossCorrelationNormalized,
                TemplateMatchTimingStage::Exact,
                &mut RegionSpectra::default(),
                &SearchClock::new(move || now),
            )
            .expect("full-frame score map");
        let best = map.best().expect("best");
        let elapsed = started.elapsed();
        assert_eq!((best.x, best.y), (700, 400));
        report.push_str(&format!(
            "REC-2 benchmark (not a gate): 1280x720 frame, {size} px template, {} ms\n",
            elapsed.as_millis()
        ));
    }
    // Written past the test harness's capture, so that the CI log shows it.
    std::io::stderr()
        .write_all(report.as_bytes())
        .expect("benchmark report");
}
