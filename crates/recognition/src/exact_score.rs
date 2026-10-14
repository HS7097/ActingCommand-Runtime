// SPDX-License-Identifier: AGPL-3.0-only

//! The exact score map of a template search (Workflow #396 REC-2, `contracts/resource-declarations.md`,
//! section Task schema `0.9` unique matches).
//!
//! Every position of the search region is scored with the same formula as the direct scorer
//! (`score_window`). Its dot product `sum(T * I)` is an integer: 8-bit gray values times integer
//! template values, at most `255^2 * 921,600`, far below `2^53`. The map sums it either directly,
//! in integers, or by an FFT cross-correlation whose result is rounded to the nearest integer.
//! The FFT's absolute error is far below one half for these sizes, so both give the same integer,
//! and the window sums come from the same exact integral images: the raw `f32` score of every
//! position is bit for bit the direct scorer's.
//!
//! Workflow #396 REC-7: a masked template scores only the pixels its mask selects. Its three
//! sums `sum_m(T * I)`, `sum_m(I)` and `sum_m(I^2)` are integers as well (at most
//! `255^2 * 921,600`), summed directly or by three FFT cross-correlations rounded to integers:
//! the masked template and the mask with the region, and the mask with the region's squares.

use super::{
    IntegralImages, MatchMetric, RecognitionError, RecognitionResult, TemplateMask, TemplateMatch,
    TemplateMatchDeadline, TemplateMatchSelection, TemplateMatchTimingStage, TemplateStats,
    normalize_ncc_score, score_from_sums,
};
use image::{GrayImage, Luma};
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::Arc;

/// The direct sums cost one multiply-add per template pixel and position; a transform costs
/// about this many of them per padded point and `log2` step.
const DIRECT_WORK_PER_TRANSFORM_UNIT: u64 = 4;

/// How the dot products of a map are summed. Both give the same integers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DotEngine {
    Direct,
    Transform,
}

/// The cheaper engine for this search; small searches sum directly.
pub(crate) fn choose_engine(search: &GrayImage, template: &GrayImage) -> DotEngine {
    let direct = super::estimated_work(
        search.width(),
        search.height(),
        template.width(),
        template.height(),
    );
    let points = (smooth_size(search.width() as usize) as u64)
        .saturating_mul(smooth_size(search.height() as usize) as u64);
    let steps = u64::from(u64::BITS - points.leading_zeros());
    let transform = points
        .saturating_mul(steps)
        .saturating_mul(DIRECT_WORK_PER_TRANSFORM_UNIT);
    if direct <= transform {
        DotEngine::Direct
    } else {
        DotEngine::Transform
    }
}

/// The smallest size of at least `minimum` whose only prime factors are 2, 3 and 5.
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

/// The forward transforms of the search regions of one frame. Every template searched in the
/// same region of the same frame shares the region's transform; the holder drops it with the
/// frame.
#[derive(Debug, Default)]
pub struct RegionSpectra {
    entries: Vec<RegionSpectrum>,
}

#[derive(Debug)]
struct RegionSpectrum {
    /// The region's left, top, width and height in the frame.
    bounds: (u32, u32, u32, u32),
    plane: Plane,
    values: Vec<Complex<f64>>,
}

/// What a transform reads of each pixel: its gray value, or (Workflow #396 REC-7, the window
/// sums of a masked template) its square.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Plane {
    Gray,
    Squared,
}

impl RegionSpectra {
    fn index_or_insert(
        &mut self,
        bounds: (u32, u32, u32, u32),
        plane: Plane,
        compute: impl FnOnce() -> RecognitionResult<Vec<Complex<f64>>>,
    ) -> RecognitionResult<usize> {
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.bounds == bounds && entry.plane == plane)
        {
            return Ok(index);
        }
        let values = compute()?;
        self.entries.push(RegionSpectrum {
            bounds,
            plane,
            values,
        });
        Ok(self.entries.len() - 1)
    }
}

/// The planned transforms of one padded size: real rows, complex columns.
struct Transforms {
    width: usize,
    height: usize,
    columns: usize,
    row_forward: Arc<dyn RealToComplex<f64>>,
    row_inverse: Arc<dyn ComplexToReal<f64>>,
    column_forward: Arc<dyn Fft<f64>>,
    column_inverse: Arc<dyn Fft<f64>>,
}

impl Transforms {
    /// Transforms padded to the smallest 2-3-5 smooth sizes that hold the region: on every
    /// position where the template lies inside the region, the circular correlation is the
    /// linear one.
    fn new(region_width: u32, region_height: u32) -> Self {
        let width = smooth_size(region_width as usize);
        let height = smooth_size(region_height as usize);
        let mut real = RealFftPlanner::<f64>::new();
        let mut complex = FftPlanner::<f64>::new();
        Self {
            width,
            height,
            columns: width / 2 + 1,
            row_forward: real.plan_fft_forward(width),
            row_inverse: real.plan_fft_inverse(width),
            column_forward: complex.plan_fft_forward(height),
            column_inverse: complex.plan_fft_inverse(height),
        }
    }

    /// The forward transform of `plane` of `image` padded with zeros, column after column: the
    /// value of frequency column `k` and row `y` is at `k * height + y`.
    fn forward(
        &self,
        image: &GrayImage,
        plane: Plane,
        deadline: &TemplateMatchDeadline,
        stage: TemplateMatchTimingStage,
    ) -> RecognitionResult<Vec<Complex<f64>>> {
        let label = deadline_label(stage);
        let image_width = image.width() as usize;
        let mut spectrum = vec![Complex::new(0.0_f64, 0.0); self.columns * self.height];
        let mut row = vec![0.0_f64; self.width];
        let mut row_spectrum = vec![Complex::new(0.0_f64, 0.0); self.columns];
        for (y, pixels) in image.as_raw().chunks_exact(image_width).enumerate() {
            deadline.check(label, stage)?;
            row.fill(0.0);
            for (slot, value) in row.iter_mut().zip(pixels) {
                let value = f64::from(*value);
                *slot = match plane {
                    Plane::Gray => value,
                    Plane::Squared => value * value,
                };
            }
            self.row_forward
                .process(&mut row, &mut row_spectrum)
                .map_err(transform_error)?;
            for (k, value) in row_spectrum.iter().enumerate() {
                spectrum[k * self.height + y] = *value;
            }
        }
        deadline.check(label, stage)?;
        self.column_forward.process(&mut spectrum);
        Ok(spectrum)
    }

    /// The correlation sums `sum(T * I)` of every valid position, row-major, each rounded to
    /// the nearest integer.
    fn correlate(
        &self,
        region: &[Complex<f64>],
        template: &[Complex<f64>],
        positions: (usize, usize),
        deadline: &TemplateMatchDeadline,
        stage: TemplateMatchTimingStage,
    ) -> RecognitionResult<Vec<f64>> {
        let label = deadline_label(stage);
        let (columns, rows) = positions;
        let mut product = region
            .iter()
            .zip(template)
            .map(|(region, template)| *region * template.conj())
            .collect::<Vec<_>>();
        deadline.check(label, stage)?;
        self.column_inverse.process(&mut product);
        // Both directions are unnormalized.
        let scale = 1.0 / (self.width as f64 * self.height as f64);
        let mut row_spectrum = vec![Complex::new(0.0_f64, 0.0); self.columns];
        let mut row = vec![0.0_f64; self.width];
        let mut sums = Vec::with_capacity(columns * rows);
        for y in 0..rows {
            deadline.check(label, stage)?;
            for (k, slot) in row_spectrum.iter_mut().enumerate() {
                *slot = product[k * self.height + y];
            }
            // The sums are real; these imaginary parts hold only the transform's rounding.
            row_spectrum[0].im = 0.0;
            if self.width.is_multiple_of(2) {
                row_spectrum[self.columns - 1].im = 0.0;
            }
            self.row_inverse
                .process(&mut row_spectrum, &mut row)
                .map_err(transform_error)?;
            sums.extend(row[..columns].iter().map(|value| {
                let sum = (value * scale).round();
                // Every sum is a non-negative integer; rounding noise never makes it -0.0.
                if sum > 0.0 { sum } else { 0.0 }
            }));
        }
        Ok(sums)
    }
}

fn transform_error(error: realfft::FftError) -> RecognitionError {
    RecognitionError::fatal(format!("template correlation transform failed: {error:?}"))
}

fn deadline_label(stage: TemplateMatchTimingStage) -> &'static str {
    match stage {
        TemplateMatchTimingStage::JointTemplateColor => "joint template/color match",
        _ => "template match",
    }
}

/// The integer sums `sum(T * I)` of every valid position, row-major, summed directly.
fn direct_sums(
    search: &GrayImage,
    template: &GrayImage,
    positions: (usize, usize),
    deadline: &TemplateMatchDeadline,
    stage: TemplateMatchTimingStage,
) -> RecognitionResult<Vec<f64>> {
    let label = deadline_label(stage);
    let (columns, rows) = positions;
    let search_width = search.width() as usize;
    let template_width = template.width() as usize;
    let search_pixels = search.as_raw();
    let mut sums = Vec::with_capacity(columns * rows);
    for y in 0..rows {
        deadline.check(label, stage)?;
        for x in 0..columns {
            let mut sum = 0_u64;
            for (ty, template_row) in template.as_raw().chunks_exact(template_width).enumerate() {
                let start = (y + ty) * search_width + x;
                for (image, value) in search_pixels[start..start + template_width]
                    .iter()
                    .zip(template_row)
                {
                    sum += u64::from(*image) * u64::from(*value);
                }
            }
            sums.push(sum as f64);
        }
    }
    Ok(sums)
}

/// Workflow #396 REC-7: the three sums of a masked template at every valid position,
/// row-major, over the template pixels the mask selects.
pub(crate) struct MaskedSums {
    /// `sum_m(T * I)`.
    pub(crate) products: Vec<f64>,
    /// `sum_m(I)`.
    pub(crate) image: Vec<f64>,
    /// `sum_m(I^2)`.
    pub(crate) squared: Vec<f64>,
}

/// The masked sums of every valid position, summed directly in integers.
fn direct_masked_sums(
    search: &GrayImage,
    template: &GrayImage,
    mask: TemplateMask,
    positions: (usize, usize),
    deadline: &TemplateMatchDeadline,
    stage: TemplateMatchTimingStage,
) -> RecognitionResult<MaskedSums> {
    let label = deadline_label(stage);
    let (columns, rows) = positions;
    let search_width = search.width() as usize;
    let template_width = template.width() as usize;
    // Each selected template pixel: its offset inside a window of the search, and its value.
    let mut selected = Vec::new();
    for (ty, template_row) in template.as_raw().chunks_exact(template_width).enumerate() {
        for (tx, value) in template_row.iter().enumerate() {
            if mask.selects(*value) {
                selected.push((ty * search_width + tx, u64::from(*value)));
            }
        }
    }
    let search_pixels = search.as_raw();
    let mut sums = MaskedSums {
        products: Vec::with_capacity(columns * rows),
        image: Vec::with_capacity(columns * rows),
        squared: Vec::with_capacity(columns * rows),
    };
    for y in 0..rows {
        deadline.check(label, stage)?;
        for x in 0..columns {
            let origin = y * search_width + x;
            let (mut products, mut image, mut squared) = (0_u64, 0_u64, 0_u64);
            for (offset, value) in &selected {
                let pixel = u64::from(search_pixels[origin + offset]);
                products += pixel * value;
                image += pixel;
                squared += pixel * pixel;
            }
            sums.products.push(products as f64);
            sums.image.push(image as f64);
            sums.squared.push(squared as f64);
        }
    }
    Ok(sums)
}

/// One template search: the region's gray image and its place in the frame.
pub(crate) struct ScoreSearch<'a> {
    pub(crate) search: &'a GrayImage,
    pub(crate) template: &'a GrayImage,
    pub(crate) metric: MatchMetric,
    /// Workflow #396 REC-7: the template pixels scored; all when `None`.
    pub(crate) mask: Option<TemplateMask>,
    pub(crate) offset_x: u32,
    pub(crate) offset_y: u32,
}

impl ScoreSearch<'_> {
    fn positions(&self) -> (usize, usize) {
        (
            (self.search.width() - self.template.width() + 1) as usize,
            (self.search.height() - self.template.height() + 1) as usize,
        )
    }
}

/// The dot products of every valid position, row-major, from `engine`.
pub(crate) fn dot_sums(
    search: &ScoreSearch<'_>,
    engine: DotEngine,
    spectra: Option<&mut RegionSpectra>,
    deadline: &TemplateMatchDeadline,
    stage: TemplateMatchTimingStage,
) -> RecognitionResult<Vec<f64>> {
    let positions = search.positions();
    match engine {
        DotEngine::Direct => {
            direct_sums(search.search, search.template, positions, deadline, stage)
        }
        DotEngine::Transform => {
            let transforms = Transforms::new(search.search.width(), search.search.height());
            let bounds = (
                search.offset_x,
                search.offset_y,
                search.search.width(),
                search.search.height(),
            );
            let owned;
            let region: &[Complex<f64>] = match spectra {
                Some(cache) => {
                    let index = cache.index_or_insert(bounds, Plane::Gray, || {
                        transforms.forward(search.search, Plane::Gray, deadline, stage)
                    })?;
                    cache.entries[index].values.as_slice()
                }
                None => {
                    owned = transforms.forward(search.search, Plane::Gray, deadline, stage)?;
                    owned.as_slice()
                }
            };
            let template = transforms.forward(search.template, Plane::Gray, deadline, stage)?;
            transforms.correlate(region, &template, positions, deadline, stage)
        }
    }
}

/// Workflow #396 REC-7: the masked sums of every valid position, row-major, from `engine`. The
/// transform engine correlates the masked template and the mask with the region's gray values,
/// and the mask with their squares: two forward transforms of the template, three inverse ones,
/// and the region's two transforms shared through `spectra`.
pub(crate) fn masked_sums(
    search: &ScoreSearch<'_>,
    mask: TemplateMask,
    engine: DotEngine,
    spectra: Option<&mut RegionSpectra>,
    deadline: &TemplateMatchDeadline,
    stage: TemplateMatchTimingStage,
) -> RecognitionResult<MaskedSums> {
    let positions = search.positions();
    match engine {
        DotEngine::Direct => direct_masked_sums(
            search.search,
            search.template,
            mask,
            positions,
            deadline,
            stage,
        ),
        DotEngine::Transform => {
            let transforms = Transforms::new(search.search.width(), search.search.height());
            let bounds = (
                search.offset_x,
                search.offset_y,
                search.search.width(),
                search.search.height(),
            );
            let masked =
                GrayImage::from_fn(search.template.width(), search.template.height(), |x, y| {
                    let value = search.template.get_pixel(x, y)[0];
                    Luma([if mask.selects(value) { value } else { 0 }])
                });
            let selected =
                GrayImage::from_fn(search.template.width(), search.template.height(), |x, y| {
                    Luma([u8::from(mask.selects(search.template.get_pixel(x, y)[0]))])
                });
            let mut owned = RegionSpectra::default();
            let cache = spectra.unwrap_or(&mut owned);
            let gray = cache.index_or_insert(bounds, Plane::Gray, || {
                transforms.forward(search.search, Plane::Gray, deadline, stage)
            })?;
            let squares = cache.index_or_insert(bounds, Plane::Squared, || {
                transforms.forward(search.search, Plane::Squared, deadline, stage)
            })?;
            let masked = transforms.forward(&masked, Plane::Gray, deadline, stage)?;
            let selected = transforms.forward(&selected, Plane::Gray, deadline, stage)?;
            let gray = cache.entries[gray].values.as_slice();
            let squares = cache.entries[squares].values.as_slice();
            Ok(MaskedSums {
                products: transforms.correlate(gray, &masked, positions, deadline, stage)?,
                image: transforms.correlate(gray, &selected, positions, deadline, stage)?,
                squared: transforms.correlate(squares, &selected, positions, deadline, stage)?,
            })
        }
    }
}

/// The raw score of every position of one template search, from exact integer sums.
pub struct ExactScoreMap {
    offset_x: u32,
    offset_y: u32,
    template_width: u32,
    template_height: u32,
    columns: u32,
    raw_scores: Vec<f32>,
    deadline: TemplateMatchDeadline,
    stage: TemplateMatchTimingStage,
}

impl ExactScoreMap {
    pub(crate) fn compute(
        search: ScoreSearch<'_>,
        engine: DotEngine,
        spectra: Option<&mut RegionSpectra>,
        deadline: TemplateMatchDeadline,
        stage: TemplateMatchTimingStage,
    ) -> RecognitionResult<Self> {
        let label = deadline_label(stage);
        let (columns, rows) = search.positions();
        let mut raw_scores = Vec::with_capacity(columns * rows);
        let stats = match search.mask {
            // Workflow #396 REC-7: the window sums are those of the selected pixels too.
            Some(mask) => {
                let stats = TemplateStats::masked(search.template, mask, search.metric)?;
                let sums = masked_sums(&search, mask, engine, spectra, &deadline, stage)?;
                for ((products, images), squares) in sums
                    .products
                    .chunks_exact(columns)
                    .zip(sums.image.chunks_exact(columns))
                    .zip(sums.squared.chunks_exact(columns))
                {
                    deadline.check(label, stage)?;
                    for ((product, image), squared) in products.iter().zip(images).zip(squares) {
                        raw_scores.push(score_from_sums(
                            &stats,
                            search.metric,
                            *product,
                            *image,
                            *squared,
                        ));
                    }
                }
                stats
            }
            None => {
                let stats = TemplateStats::new(search.template, search.metric)?;
                let sums = dot_sums(&search, engine, spectra, &deadline, stage)?;
                let integrals = IntegralImages::new(search.search);
                for (y, row) in sums.chunks_exact(columns).enumerate() {
                    deadline.check(label, stage)?;
                    for (x, dot) in row.iter().enumerate() {
                        let (x, y) = (x as u32, y as u32);
                        raw_scores.push(score_from_sums(
                            &stats,
                            search.metric,
                            *dot,
                            integrals.sum(x, y, stats.width, stats.height),
                            integrals.squared_sum(x, y, stats.width, stats.height),
                        ));
                    }
                }
                stats
            }
        };
        Ok(Self {
            offset_x: search.offset_x,
            offset_y: search.offset_y,
            template_width: stats.width,
            template_height: stats.height,
            columns: columns as u32,
            raw_scores,
            deadline,
            stage,
        })
    }

    /// The raw scores, row-major over the positions of the search region.
    #[cfg(test)]
    pub(crate) fn raw_scores(&self) -> &[f32] {
        &self.raw_scores
    }

    fn label(&self) -> &'static str {
        deadline_label(self.stage)
    }

    fn candidate(&self, index: usize, raw_score: f32) -> RecognitionResult<TemplateMatch> {
        let columns = self.columns as usize;
        let x = i32::try_from(self.offset_x + (index % columns) as u32).map_err(|_| {
            RecognitionError::fatal("template match x coordinate exceeds i32 range")
        })?;
        let y = i32::try_from(self.offset_y + (index / columns) as u32).map_err(|_| {
            RecognitionError::fatal("template match y coordinate exceeds i32 range")
        })?;
        Ok(TemplateMatch {
            x,
            y,
            width: i32::try_from(self.template_width)
                .map_err(|_| RecognitionError::fatal("template match width exceeds i32 range"))?,
            height: i32::try_from(self.template_height)
                .map_err(|_| RecognitionError::fatal("template match height exceeds i32 range"))?,
            raw_score,
            score: normalize_ncc_score(raw_score),
        })
    }

    /// The highest raw score; equal raw scores keep the first `(y, x)` position.
    pub fn best(&self) -> RecognitionResult<TemplateMatch> {
        let mut best: Option<(usize, f32)> = None;
        for (y, row) in self
            .raw_scores
            .chunks_exact(self.columns as usize)
            .enumerate()
        {
            self.deadline.check(self.label(), self.stage)?;
            for (x, raw_score) in row.iter().enumerate() {
                if best.is_none_or(|(_, best)| raw_score.total_cmp(&best) == Ordering::Greater) {
                    best = Some((y * self.columns as usize + x, *raw_score));
                }
            }
        }
        let (index, raw_score) =
            best.ok_or_else(|| RecognitionError::fatal("template match produced no candidates"))?;
        self.candidate(index, raw_score)
    }

    /// The highest raw score overall, and the highest whose normalized score reaches
    /// `threshold` and that `accept` admits (`contracts/template-relative-color.md`). Positions
    /// are visited in row-major order; equal raw scores keep the first `(y, x)` position.
    pub fn select(
        &self,
        threshold: f32,
        mut accept: impl FnMut(TemplateMatch) -> RecognitionResult<bool>,
    ) -> RecognitionResult<TemplateMatchSelection> {
        if !threshold.is_finite() || !(0.0..=1.0).contains(&threshold) {
            return Err(RecognitionError::fatal(
                "template threshold must be in 0..=1",
            ));
        }
        let mut best: Option<TemplateMatch> = None;
        let mut accepted: Option<TemplateMatch> = None;
        for (y, row) in self
            .raw_scores
            .chunks_exact(self.columns as usize)
            .enumerate()
        {
            self.deadline.check(self.label(), self.stage)?;
            for (x, raw_score) in row.iter().copied().enumerate() {
                let candidate = self.candidate(y * self.columns as usize + x, raw_score)?;
                if best.is_none_or(|best| raw_score > best.raw_score) {
                    best = Some(candidate);
                }
                if candidate.score >= threshold
                    && accepted.is_none_or(|best| raw_score > best.raw_score)
                    && accept(candidate)?
                {
                    accepted = Some(candidate);
                }
            }
        }
        self.deadline.check(self.label(), self.stage)?;
        Ok(TemplateMatchSelection {
            best_template: best
                .ok_or_else(|| RecognitionError::fatal("template match produced no candidates"))?,
            accepted,
        })
    }

    /// The best second place to `best`: among the positions whose box does not intersect
    /// `best`'s and whose normalized score in milli is at least `best`'s minus `margin_milli`,
    /// the first that `accept` admits, asked from the highest raw score down (equal raw scores
    /// in `(y, x)` order). A score below the threshold counts when it is within the margin.
    pub fn second_within(
        &self,
        best: TemplateMatch,
        margin_milli: u32,
        mut accept: impl FnMut(TemplateMatch) -> RecognitionResult<bool>,
    ) -> RecognitionResult<Option<TemplateMatch>> {
        let floor = i64::from(score_milli(best.score)) - i64::from(margin_milli);
        let best_x = i64::from(best.x) - i64::from(self.offset_x);
        let best_y = i64::from(best.y) - i64::from(self.offset_y);
        let width = i64::from(self.template_width);
        let height = i64::from(self.template_height);
        let mut eligible = Vec::new();
        for (y, row) in self
            .raw_scores
            .chunks_exact(self.columns as usize)
            .enumerate()
        {
            self.deadline.check(self.label(), self.stage)?;
            let apart_rows = (y as i64 - best_y).abs() >= height;
            for (x, raw_score) in row.iter().copied().enumerate() {
                if !apart_rows && (x as i64 - best_x).abs() < width {
                    continue;
                }
                if i64::from(score_milli(normalize_ncc_score(raw_score))) < floor {
                    continue;
                }
                eligible.push(Ranked {
                    raw_score,
                    index: y * self.columns as usize + x,
                });
            }
        }
        let mut ranked = BinaryHeap::from(eligible);
        let row = self.columns as usize;
        let mut asked = 0_usize;
        while let Some(next) = ranked.pop() {
            if asked.is_multiple_of(row) {
                self.deadline.check(self.label(), self.stage)?;
            }
            asked += 1;
            let candidate = self.candidate(next.index, next.raw_score)?;
            if accept(candidate)? {
                return Ok(Some(candidate));
            }
        }
        Ok(None)
    }
}

/// A position ranked by raw score, higher first; equal scores rank the earlier position first.
struct Ranked {
    raw_score: f32,
    index: usize,
}

impl Ord for Ranked {
    fn cmp(&self, other: &Self) -> Ordering {
        self.raw_score
            .total_cmp(&other.raw_score)
            .then_with(|| other.index.cmp(&self.index))
    }
}

impl PartialOrd for Ranked {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Ranked {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Ranked {}

/// A normalized score (`0..=1`) in milli, rounded down.
pub fn score_milli(score: f32) -> u32 {
    let milli = (f64::from(score) * 1000.0).floor();
    if milli.is_nan() || milli <= 0.0 {
        0
    } else if milli >= 1000.0 {
        1000
    } else {
        milli as u32
    }
}

#[cfg(test)]
#[path = "exact_score_tests.rs"]
mod tests;

// Workflow #396 REC-7.
#[cfg(test)]
#[path = "exact_score_mask_tests.rs"]
mod mask_tests;
