// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #396 REC-5 logic test of the scaled score maps (REC-M v1.2.1 §7, §12): a
// `ccorr_normed` template whose only factor is 1000 is scored at every position exactly as the
// direct scorer scores it. Frames are synthetic noise.

use super::*;
use crate::{Rect, Scene, SearchClock, score_window};
use image::{DynamicImage, ImageFormat, Luma};
use std::io::Cursor;
use std::time::Instant;

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

#[test]
fn a_ccorr_template_whose_only_factor_is_1000_scores_every_position_as_the_direct_scorer() {
    let mut noise = Noise(0x0396_0501);
    let frame = noise.image(96, 64);
    let template = noise.image(16, 12);
    let scene = gray_scene(&frame);
    let region = Rect {
        x: 8,
        y: 4,
        width: 80,
        height: 56,
    };
    let now = Instant::now();
    let metric = MatchMetric::CrossCorrelationNormalized;

    let maps = scene
        .exact_score_maps(
            &gray_png(&template),
            Some(region),
            metric,
            TemplateMatchTimingStage::Exact,
            None,
            &[1000],
            &mut RegionSpectra::default(),
            &SearchClock::new(move || now),
        )
        .expect("score maps");

    let [map] = maps.maps() else {
        panic!("one map per factor");
    };
    assert_eq!(map.scale_milli(), 1000);
    let search = image::imageops::crop_imm(&frame, 8, 4, 80, 56).to_image();
    let stats = TemplateStats::new(&template, metric).expect("template stats");
    let integrals = IntegralImages::new(&search);
    let (columns, rows) = (80 - 16 + 1, 56 - 12 + 1);
    assert_eq!(map.raw_scores().len(), (columns * rows) as usize);
    for y in 0..rows {
        for x in 0..columns {
            assert_eq!(
                map.raw_scores()[(y * columns + x) as usize].to_bits(),
                score_window(&search, &stats, &integrals, metric, x, y).to_bits(),
                "({x}, {y})"
            );
        }
    }
}
