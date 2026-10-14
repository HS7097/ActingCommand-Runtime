// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #396 REC-7 logic test of masked template scores (REC-M v1.2.1 §9.1): every position
// of a masked template's exact score map scores the masked formulas over the template pixels the
// mask selects, whichever engine sums them. Frames are synthetic neutral noise.

use super::*;
use crate::{SearchClock, TEMPLATE_MATCH_TIMEOUT};
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

/// A deadline whose clock never moves: this test checks scores, never the time limit.
fn frozen_deadline() -> TemplateMatchDeadline {
    let now = Instant::now();
    TemplateMatchDeadline::with_clock(TEMPLATE_MATCH_TIMEOUT, SearchClock::new(move || now))
}

/// The masked formulas of REC-M §9.1 at the position `(x, y)`, from sums taken pixel by pixel
/// over the selected template pixels:
/// `ccorr_normed = sum_m(T*I) / sqrt(sum_m(T^2) * sum_m(I^2))` and
/// `ccoeff_normed = (sum_m(T*I) - sum_m(T) * sum_m(I) / n)
///   / sqrt((sum_m(T^2) - sum_m(T)^2 / n) * (sum_m(I^2) - sum_m(I)^2 / n))`, 0 when the
/// denominator is 0; evaluated in the order the direct scorer evaluates them.
fn masked_formula(
    search: &GrayImage,
    template: &GrayImage,
    mask: TemplateMask,
    metric: MatchMetric,
    x: u32,
    y: u32,
) -> f32 {
    let (mut n, mut t, mut tt, mut ti, mut i, mut ii) = (0_u64, 0_u64, 0_u64, 0_u64, 0_u64, 0_u64);
    for ty in 0..template.height() {
        for tx in 0..template.width() {
            let value = template.get_pixel(tx, ty)[0];
            if !(mask.lower..=mask.upper).contains(&value) {
                continue;
            }
            let value = u64::from(value);
            let pixel = u64::from(search.get_pixel(x + tx, y + ty)[0]);
            n += 1;
            t += value;
            tt += value * value;
            ti += value * pixel;
            i += pixel;
            ii += pixel * pixel;
        }
    }
    let (n, t, tt, ti, i, ii) = (
        n as f64, t as f64, tt as f64, ti as f64, i as f64, ii as f64,
    );
    let raw = match metric {
        MatchMetric::CrossCorrelationNormalized => {
            let denominator = (ii * tt).sqrt();
            if denominator <= f64::EPSILON {
                0.0
            } else {
                ti / denominator
            }
        }
        MatchMetric::CorrelationCoefficientNormalized => {
            let window = ii - (i * i / n);
            if window <= f64::EPSILON {
                0.0
            } else {
                (ti - (t / n) * i) / (window.sqrt() * (tt - (t * t / n)).sqrt())
            }
        }
    };
    raw as f32
}

#[test]
fn masked_scores_are_the_masked_formulas_at_every_position_on_both_engines() {
    let mut noise = Noise(0x0396_0701);
    let mask = TemplateMask {
        lower: 64,
        upper: 191,
    };
    for (frame, size) in [((96, 72), (20, 14)), ((160, 120), (64, 48))] {
        let search = noise.image(frame.0, frame.1);
        let template = noise.image(size.0, size.1);
        let columns = frame.0 - size.0 + 1;
        let rows = frame.1 - size.1 + 1;
        for metric in [
            MatchMetric::CrossCorrelationNormalized,
            MatchMetric::CorrelationCoefficientNormalized,
        ] {
            for engine in [DotEngine::Direct, DotEngine::Transform] {
                let map = ExactScoreMap::compute(
                    ScoreSearch {
                        search: &search,
                        template: &template,
                        metric,
                        mask: Some(mask),
                        offset_x: 0,
                        offset_y: 0,
                    },
                    engine,
                    None,
                    frozen_deadline(),
                    TemplateMatchTimingStage::Exact,
                )
                .expect("masked score map");
                assert_eq!(map.raw_scores().len(), (columns * rows) as usize);
                for y in 0..rows {
                    for x in 0..columns {
                        assert_eq!(
                            map.raw_scores()[(y * columns + x) as usize].to_bits(),
                            masked_formula(&search, &template, mask, metric, x, y).to_bits(),
                            "{size:?} template, {metric:?}, {engine:?}, at ({x}, {y})"
                        );
                    }
                }
            }
        }
    }
}
