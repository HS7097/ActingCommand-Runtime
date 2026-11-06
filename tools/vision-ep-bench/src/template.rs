// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #407 GPU-0b: the platform-neutral part of the template mode (design §1.2
//! "模板匹配", §2.5 FS-G).
//!
//! The float path is what an accelerator would propose: the Conv graph (`onnx.rs`) sums
//! `sum(T * I)` in `f32`, and with a mask also `sum_m(I)` and `sum_m(I^2)`; without a mask the
//! window sums come from exact integral images on the CPU. The CPU assembles the score with the
//! formula of the exact scorer. The accuracy tally compares it with the exact score map: the
//! largest |Δ| of the raw scores, then, at the margin ε = 4 × that largest |Δ| (design §2.5), the
//! exact matches (normalized exact score at least the threshold) the float path still keeps
//! (normalized float score at least `threshold − ε`), and how many positions it keeps.

use crate::onnx::ConvKernel;
use actingcommand_recognition::{MatchMetric, TemplateMask};
use image::GrayImage;
use serde::Serialize;

/// Design §2.5: the float path's margin is this many times its largest |Δ|.
pub const MARGIN_FACTOR: f64 = 4.0;
/// The contract's smallest mask (`resource-declarations.md`, Task schema `0.9` masks).
pub const MASK_MIN_SELECTED_PIXELS: u64 = 16;

/// The exact integral images of a gray image: every window's `sum(I)` and `sum(I^2)` as exact
/// integers held in `f64`.
#[derive(Debug, Clone)]
pub struct WindowSums {
    stride: usize,
    sum: Vec<f64>,
    squared: Vec<f64>,
}

impl WindowSums {
    pub fn new(image: &GrayImage) -> Self {
        let width = image.width() as usize;
        let height = image.height() as usize;
        let stride = width + 1;
        let mut sum = vec![0.0; stride * (height + 1)];
        let mut squared = vec![0.0; stride * (height + 1)];
        for (y, row) in image.as_raw().chunks_exact(width.max(1)).enumerate() {
            let mut row_sum = 0.0;
            let mut row_squared = 0.0;
            for (x, value) in row.iter().enumerate() {
                let value = f64::from(*value);
                row_sum += value;
                row_squared += value * value;
                let current = (y + 1) * stride + x + 1;
                let above = y * stride + x + 1;
                sum[current] = sum[above] + row_sum;
                squared[current] = squared[above] + row_squared;
            }
        }
        Self {
            stride,
            sum,
            squared,
        }
    }

    pub fn sum(&self, x: usize, y: usize, width: usize, height: usize) -> f64 {
        self.rect(&self.sum, x, y, width, height)
    }

    pub fn squared_sum(&self, x: usize, y: usize, width: usize, height: usize) -> f64 {
        self.rect(&self.squared, x, y, width, height)
    }

    fn rect(&self, data: &[f64], x: usize, y: usize, width: usize, height: usize) -> f64 {
        let (right, bottom) = (x + width, y + height);
        data[bottom * self.stride + right]
            - data[y * self.stride + right]
            - data[bottom * self.stride + x]
            + data[y * self.stride + x]
    }
}

/// A template's sums over its scored pixels, as the exact scorer computes them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TemplateStatistics {
    pub width: u32,
    pub height: u32,
    pub selected_pixels: u64,
    count: f64,
    mean: f64,
    squared_sum: f64,
    centered_norm_sq: f64,
}

impl TemplateStatistics {
    /// Over every pixel, or the pixels `mask` selects.
    pub fn of(template: &GrayImage, mask: Option<TemplateMask>) -> Self {
        let (mut selected, mut sum, mut squared_sum) = (0_u64, 0_u64, 0_u64);
        for value in template.as_raw() {
            if mask.is_none_or(|mask| mask.selects(*value)) {
                let value = u64::from(*value);
                selected += 1;
                sum += value;
                squared_sum += value * value;
            }
        }
        let count = selected as f64;
        let (sum, squared_sum) = (sum as f64, squared_sum as f64);
        let mean = if selected == 0 { 0.0 } else { sum / count };
        let centered_norm_sq = if selected == 0 {
            0.0
        } else {
            squared_sum - (sum * sum / count)
        };
        Self {
            width: template.width(),
            height: template.height(),
            selected_pixels: selected,
            count,
            mean,
            squared_sum,
            centered_norm_sq,
        }
    }
}

/// The raw score of one position from its sums, with the exact scorer's formula
/// (`score_from_sums` in `crates/recognition/src/lib.rs`): 0 where the denominator vanishes or
/// the value is not finite.
pub fn raw_score(
    template: &TemplateStatistics,
    metric: MatchMetric,
    dot: f64,
    image_sum: f64,
    image_squared_sum: f64,
) -> f32 {
    let raw = match metric {
        MatchMetric::CrossCorrelationNormalized => {
            let denominator = (image_squared_sum * template.squared_sum).sqrt();
            if denominator <= f64::EPSILON {
                0.0
            } else {
                dot / denominator
            }
        }
        MatchMetric::CorrelationCoefficientNormalized => {
            let window_norm_sq = image_squared_sum - (image_sum * image_sum / template.count);
            if window_norm_sq <= f64::EPSILON {
                0.0
            } else {
                let numerator = dot - template.mean * image_sum;
                numerator / (window_norm_sq.sqrt() * template.centered_norm_sq.sqrt())
            }
        }
    };
    if raw.is_finite() { raw as f32 } else { 0.0 }
}

/// The normalized score a threshold is compared with: the raw score clamped to `0..=1`, 0 for
/// NaN (as the recognition crate normalizes it).
pub fn normalized(raw: f32) -> f32 {
    if raw.is_nan() {
        0.0
    } else {
        raw.clamp(0.0, 1.0)
    }
}

/// The Conv graph's sums of one scale, row-major over its positions.
#[derive(Debug, Clone, Copy)]
pub struct ScaleSums<'a> {
    pub products: &'a [f32],
    /// `sum_m(I)` and `sum_m(I^2)` of a masked template; `None` takes the window sums from the
    /// exact integral images.
    pub masked: Option<(&'a [f32], &'a [f32])>,
}

/// Appends the float raw score of every position of one scale to `scores`, row-major.
pub fn assemble_scores(
    template: &TemplateStatistics,
    metric: MatchMetric,
    sums: ScaleSums<'_>,
    window: Option<&WindowSums>,
    positions: (usize, usize),
    scores: &mut Vec<f32>,
) -> Result<(), String> {
    let (columns, rows) = positions;
    let count = columns * rows;
    let lengths_match = sums.products.len() == count
        && sums
            .masked
            .is_none_or(|(image, squared)| image.len() == count && squared.len() == count);
    if !lengths_match {
        return Err(format!(
            "the Conv outputs do not hold {columns}x{rows} positions"
        ));
    }
    let (width, height) = (template.width as usize, template.height as usize);
    scores.reserve(count);
    match sums.masked {
        Some((image, squared)) => {
            for ((dot, image), squared) in sums.products.iter().zip(image).zip(squared) {
                scores.push(raw_score(
                    template,
                    metric,
                    f64::from(*dot),
                    f64::from(*image),
                    f64::from(*squared),
                ));
            }
        }
        None => {
            let window = window.ok_or("an unmasked scale needs the exact window sums")?;
            for (y, row) in sums.products.chunks_exact(columns.max(1)).enumerate() {
                for (x, dot) in row.iter().enumerate() {
                    scores.push(raw_score(
                        template,
                        metric,
                        f64::from(*dot),
                        window.sum(x, y, width, height),
                        window.squared_sum(x, y, width, height),
                    ));
                }
            }
        }
    }
    Ok(())
}

/// The kernels of one scale of the Conv graph: the template's gray values (masked: the masked
/// template, and the mask as 0 and 1).
pub fn conv_kernel(template: &GrayImage, mask: Option<TemplateMask>) -> ConvKernel {
    let selected = |value: u8| mask.is_none_or(|mask| mask.selects(value));
    ConvKernel {
        width: template.width(),
        height: template.height(),
        products: template
            .as_raw()
            .iter()
            .map(|value| {
                if selected(*value) {
                    f32::from(*value)
                } else {
                    0.0
                }
            })
            .collect(),
        mask: mask.map(|_| {
            template
                .as_raw()
                .iter()
                .map(|value| if selected(*value) { 1.0 } else { 0.0 })
                .collect()
        }),
    }
}

/// The largest |exact − float| of two raw score lists of one length; a NaN difference counts as
/// infinite.
pub fn max_abs_delta(exact: &[f32], float: &[f32]) -> f64 {
    exact
        .iter()
        .zip(float)
        .map(|(exact, float)| {
            let delta = (f64::from(*exact) - f64::from(*float)).abs();
            if delta.is_nan() { f64::INFINITY } else { delta }
        })
        .fold(0.0, f64::max)
}

/// The recall and keep counts of design §2.5 over some positions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct RecallCounts {
    pub positions: u64,
    /// Positions whose normalized exact score reaches the threshold.
    pub exact_matches: u64,
    /// Exact matches whose normalized float score reaches `threshold − margin`.
    pub recalled: u64,
    /// Positions whose normalized float score reaches `threshold − margin`.
    pub kept: u64,
}

impl RecallCounts {
    pub fn add(&mut self, other: RecallCounts) {
        self.positions += other.positions;
        self.exact_matches += other.exact_matches;
        self.recalled += other.recalled;
        self.kept += other.kept;
    }

    /// `recalled / exact_matches`; none without an exact match.
    pub fn recall(&self) -> Option<f64> {
        (self.exact_matches > 0).then(|| self.recalled as f64 / self.exact_matches as f64)
    }
}

/// The positions whose normalized exact score reaches `threshold`.
pub fn exact_match_indices(exact: &[f32], threshold: f32) -> Vec<u32> {
    exact
        .iter()
        .enumerate()
        .filter(|(_, raw)| normalized(**raw) >= threshold)
        .filter_map(|(index, _)| u32::try_from(index).ok())
        .collect()
}

/// The counts at `margin` from the normalized float scores and the exact matches' positions.
pub fn count_kept(
    float_normalized: &[f32],
    exact_matches: &[u32],
    threshold: f32,
    margin: f64,
) -> RecallCounts {
    let floor = f64::from(threshold) - margin;
    let keeps = |score: f32| f64::from(score) >= floor;
    RecallCounts {
        positions: float_normalized.len() as u64,
        exact_matches: exact_matches.len() as u64,
        recalled: exact_matches
            .iter()
            .filter(|index| {
                float_normalized
                    .get(**index as usize)
                    .is_some_and(|score| keeps(*score))
            })
            .count() as u64,
        kept: float_normalized
            .iter()
            .filter(|score| keeps(**score))
            .count() as u64,
    }
}

/// The counts at `margin` of two raw score lists of one length.
pub fn count_at_margin(exact: &[f32], float: &[f32], threshold: f32, margin: f64) -> RecallCounts {
    let float_normalized: Vec<f32> = float.iter().copied().map(normalized).collect();
    count_kept(
        &float_normalized,
        &exact_match_indices(exact, threshold),
        threshold,
        margin,
    )
}

/// How the exact score map sums its dot products.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExactEngine {
    Direct,
    Transform,
}

/// The engine `ExactScoreMap` chooses for a search of this size: the rule of `choose_engine` in
/// `crates/recognition/src/exact_score.rs` on this branch's base (main `b9b3cac5`), repeated here
/// because the map does not expose the engine it chose.
pub fn exact_map_engine(
    search_width: u32,
    search_height: u32,
    template_width: u32,
    template_height: u32,
) -> ExactEngine {
    let direct = if template_width > search_width || template_height > search_height {
        0
    } else {
        u64::from(search_width - template_width + 1)
            * u64::from(search_height - template_height + 1)
            * u64::from(template_width)
            * u64::from(template_height)
    };
    let points = (smooth_size(search_width as usize) as u64)
        .saturating_mul(smooth_size(search_height as usize) as u64);
    let steps = u64::from(u64::BITS - points.leading_zeros());
    // `DIRECT_WORK_PER_TRANSFORM_UNIT` of the recognition crate.
    let transform = points.saturating_mul(steps).saturating_mul(4);
    if direct <= transform {
        ExactEngine::Direct
    } else {
        ExactEngine::Transform
    }
}

fn smooth_size(minimum: usize) -> usize {
    let mut size = minimum.max(1);
    loop {
        let mut rest = size;
        for factor in [2, 3, 5] {
            while rest.is_multiple_of(factor) {
                rest /= factor;
            }
        }
        if rest == 1 {
            return size;
        }
        size += 1;
    }
}
