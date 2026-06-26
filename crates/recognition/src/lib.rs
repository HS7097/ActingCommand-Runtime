// SPDX-License-Identifier: AGPL-3.0-only

use image::{DynamicImage, GrayImage, ImageError, ImageFormat, RgbImage, RgbaImage, imageops};
use imageproc::template_matching::{
    MatchTemplateMethod, find_extremes, match_template as match_template_map,
};
use std::error::Error;
use std::fmt;
use std::time::{Duration, Instant};

pub type RecognitionResult<T> = Result<T, RecognitionError>;

const TEMPLATE_MATCH_TIMEOUT: Duration = Duration::from_secs(5);
const FULL_FRAME_FAST_PATH_WORK_THRESHOLD: u64 = 50_000_000;
const FULL_FRAME_COARSE_WORK_TARGET: u64 = 5_000_000;
const FULL_FRAME_MAX_DOWNSAMPLE: u32 = 8;
const FULL_FRAME_TOP_CANDIDATES: usize = 4;
const FULL_FRAME_REFINE_RADIUS_MULTIPLIER: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecognitionErrorSeverity {
    Fatal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecognitionError {
    severity: RecognitionErrorSeverity,
    message: String,
}

impl RecognitionError {
    pub fn fatal(message: impl Into<String>) -> Self {
        Self {
            severity: RecognitionErrorSeverity::Fatal,
            message: message.into(),
        }
    }

    pub fn severity(&self) -> RecognitionErrorSeverity {
        self.severity
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for RecognitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.severity {
            RecognitionErrorSeverity::Fatal => {
                write!(f, "fatal recognition error: {}", self.message)
            }
        }
    }
}

impl Error for RecognitionError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TemplateMatch {
    pub x: i32,
    pub y: i32,

    /// Raw score returned by the current template matching method.
    pub raw_score: f32,

    /// Normalized score in 0.0..=1.0 for rule-layer thresholding.
    /// This is not a probability.
    pub score: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchMetric {
    CrossCorrelationNormalized,
    CorrelationCoefficientNormalized,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorMatch {
    pub distance: f32,
    pub mean: [u8; 3],
}

#[derive(Debug)]
pub struct Scene {
    rgb: RgbImage,
    gray: GrayImage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScenePixelFormat {
    Rgb8,
    Rgba8,
}

impl Scene {
    pub fn from_png(png: &[u8]) -> RecognitionResult<Scene> {
        let image = image::load_from_memory_with_format(png, ImageFormat::Png)
            .map_err(|err| decode_error("scene PNG", err))?;
        Ok(Scene {
            rgb: image.to_rgb8(),
            gray: image.to_luma8(),
        })
    }

    pub fn from_pixels(
        width: u32,
        height: u32,
        pixels: &[u8],
        pixel_format: ScenePixelFormat,
    ) -> RecognitionResult<Scene> {
        match pixel_format {
            ScenePixelFormat::Rgb8 => Self::from_rgb8(width, height, pixels),
            ScenePixelFormat::Rgba8 => Self::from_rgba8(width, height, pixels),
        }
    }

    pub fn from_rgb8(width: u32, height: u32, pixels: &[u8]) -> RecognitionResult<Scene> {
        validate_pixels(width, height, pixels.len(), 3, "RGB8")?;
        let rgb = RgbImage::from_raw(width, height, pixels.to_vec())
            .ok_or_else(|| RecognitionError::fatal("failed to build RGB8 scene from raw pixels"))?;
        let gray = DynamicImage::ImageRgb8(rgb.clone()).to_luma8();
        Ok(Scene { rgb, gray })
    }

    pub fn from_rgba8(width: u32, height: u32, pixels: &[u8]) -> RecognitionResult<Scene> {
        validate_pixels(width, height, pixels.len(), 4, "RGBA8")?;
        let rgba = RgbaImage::from_raw(width, height, pixels.to_vec()).ok_or_else(|| {
            RecognitionError::fatal("failed to build RGBA8 scene from raw pixels")
        })?;
        let image = DynamicImage::ImageRgba8(rgba);
        Ok(Scene {
            rgb: image.to_rgb8(),
            gray: image.to_luma8(),
        })
    }

    pub fn width(&self) -> u32 {
        self.rgb.width()
    }

    pub fn height(&self) -> u32 {
        self.rgb.height()
    }

    pub fn match_template(
        &self,
        template_png: &[u8],
        region: Option<Rect>,
    ) -> RecognitionResult<TemplateMatch> {
        self.match_template_with_metric(
            template_png,
            region,
            MatchMetric::CrossCorrelationNormalized,
        )
    }

    pub fn match_template_with_metric(
        &self,
        template_png: &[u8],
        region: Option<Rect>,
        metric: MatchMetric,
    ) -> RecognitionResult<TemplateMatch> {
        let template = image::load_from_memory_with_format(template_png, ImageFormat::Png)
            .map_err(|err| decode_error("template PNG", err))?
            .to_luma8();
        if template.width() == 0 || template.height() == 0 {
            return Err(RecognitionError::fatal(
                "template dimensions must be non-zero",
            ));
        }

        let (search, offset_x, offset_y) = match region {
            Some(rect) => {
                let bounds = validate_rect(rect, self.width(), self.height())?;
                (
                    imageops::crop_imm(&self.gray, bounds.x, bounds.y, bounds.width, bounds.height)
                        .to_image(),
                    bounds.x,
                    bounds.y,
                )
            }
            None => (self.gray.clone(), 0, 0),
        };

        if template.width() > search.width() || template.height() > search.height() {
            return Err(RecognitionError::fatal(format!(
                "template {}x{} exceeds search area {}x{}",
                template.width(),
                template.height(),
                search.width(),
                search.height()
            )));
        }

        let use_fast_path = should_use_fast_path(&search, &template);
        let deadline = TemplateMatchDeadline::new(TEMPLATE_MATCH_TIMEOUT);

        match metric {
            MatchMetric::CrossCorrelationNormalized => {
                if use_fast_path {
                    full_frame_pyramid_match(&search, &template, metric, offset_x, offset_y)
                } else {
                    // The bounded ccorr path preserves the existing imageproc semantics.
                    let response = match_template_map(
                        &search,
                        &template,
                        MatchTemplateMethod::CrossCorrelationNormalized,
                    );
                    deadline.check("ccorr_normed template match")?;
                    let extremes = find_extremes(&response);
                    let raw_score = extremes.max_value;
                    let score = normalize_ncc_score(raw_score);
                    let x =
                        i32::try_from(offset_x + extremes.max_value_location.0).map_err(|_| {
                            RecognitionError::fatal("template match x coordinate exceeds i32 range")
                        })?;
                    let y =
                        i32::try_from(offset_y + extremes.max_value_location.1).map_err(|_| {
                            RecognitionError::fatal("template match y coordinate exceeds i32 range")
                        })?;

                    Ok(TemplateMatch {
                        x,
                        y,
                        raw_score,
                        score,
                    })
                }
            }
            MatchMetric::CorrelationCoefficientNormalized => {
                if use_fast_path {
                    full_frame_pyramid_match(&search, &template, metric, offset_x, offset_y)
                } else {
                    exact_metric_match(
                        &search,
                        &template,
                        metric,
                        offset_x,
                        offset_y,
                        SearchWindow::full(&search, &template),
                        &deadline,
                    )
                }
            }
        }
    }

    pub fn compare_color(&self, region: Rect, expected: [u8; 3]) -> RecognitionResult<ColorMatch> {
        let bounds = validate_rect(region, self.width(), self.height())?;
        let mut sum = [0_u64; 3];

        for y in bounds.y..bounds.y + bounds.height {
            for x in bounds.x..bounds.x + bounds.width {
                let pixel = self.rgb.get_pixel(x, y);
                sum[0] += u64::from(pixel[0]);
                sum[1] += u64::from(pixel[1]);
                sum[2] += u64::from(pixel[2]);
            }
        }

        let count = u64::from(bounds.width) * u64::from(bounds.height);
        let mean = [
            (sum[0] / count) as u8,
            (sum[1] / count) as u8,
            (sum[2] / count) as u8,
        ];
        let distance = euclidean_distance(mean, expected);

        Ok(ColorMatch { distance, mean })
    }
}

fn exact_metric_match(
    search: &GrayImage,
    template: &GrayImage,
    metric: MatchMetric,
    offset_x: u32,
    offset_y: u32,
    window: SearchWindow,
    deadline: &TemplateMatchDeadline,
) -> RecognitionResult<TemplateMatch> {
    let candidate = exact_metric_candidates(search, template, metric, window, 1, deadline)?
        .into_iter()
        .next()
        .ok_or_else(|| RecognitionError::fatal("template match produced no candidates"))?;
    template_match_from_candidate(candidate, offset_x, offset_y)
}

fn full_frame_pyramid_match(
    search: &GrayImage,
    template: &GrayImage,
    metric: MatchMetric,
    offset_x: u32,
    offset_y: u32,
) -> RecognitionResult<TemplateMatch> {
    let deadline = TemplateMatchDeadline::new(TEMPLATE_MATCH_TIMEOUT);
    let factor = choose_downsample_factor(search, template);
    if factor <= 1 {
        return exact_metric_match(
            search,
            template,
            metric,
            offset_x,
            offset_y,
            SearchWindow::full(search, template),
            &deadline,
        );
    }

    let coarse_search = downsample_gray(search, factor);
    let coarse_template = downsample_gray(template, factor);
    if coarse_template.width() > coarse_search.width()
        || coarse_template.height() > coarse_search.height()
    {
        return exact_metric_match(
            search,
            template,
            metric,
            offset_x,
            offset_y,
            SearchWindow::full(search, template),
            &deadline,
        );
    }

    let coarse_candidates = exact_metric_candidates(
        &coarse_search,
        &coarse_template,
        metric,
        SearchWindow::full(&coarse_search, &coarse_template),
        FULL_FRAME_TOP_CANDIDATES,
        &deadline,
    )?;
    let full_window = SearchWindow::full(search, template);
    let radius = factor * FULL_FRAME_REFINE_RADIUS_MULTIPLIER;
    let mut refined = Vec::new();
    for candidate in coarse_candidates {
        let approx_x = candidate.x.saturating_mul(factor);
        let approx_y = candidate.y.saturating_mul(factor);
        let window = full_window.around(approx_x, approx_y, radius);
        let best = exact_metric_candidates(search, template, metric, window, 1, &deadline)?;
        refined.extend(best);
    }

    let candidate = refined
        .into_iter()
        .max_by(|a, b| a.raw_score.total_cmp(&b.raw_score))
        .ok_or_else(|| {
            RecognitionError::fatal("full-frame template match produced no candidates")
        })?;
    template_match_from_candidate(candidate, offset_x, offset_y)
}

fn template_match_from_candidate(
    candidate: MatchCandidate,
    offset_x: u32,
    offset_y: u32,
) -> RecognitionResult<TemplateMatch> {
    let x = i32::try_from(offset_x + candidate.x)
        .map_err(|_| RecognitionError::fatal("template match x coordinate exceeds i32 range"))?;
    let y = i32::try_from(offset_y + candidate.y)
        .map_err(|_| RecognitionError::fatal("template match y coordinate exceeds i32 range"))?;

    Ok(TemplateMatch {
        x,
        y,
        raw_score: candidate.raw_score,
        score: normalize_ncc_score(candidate.raw_score),
    })
}

fn exact_metric_candidates(
    search: &GrayImage,
    template: &GrayImage,
    metric: MatchMetric,
    window: SearchWindow,
    limit: usize,
    deadline: &TemplateMatchDeadline,
) -> RecognitionResult<Vec<MatchCandidate>> {
    let limit = limit.max(1);
    let template_stats = TemplateStats::new(template, metric)?;
    let integrals = IntegralImages::new(search);
    let mut candidates = Vec::new();

    for y in window.min_y..=window.max_y {
        deadline.check("template match")?;
        for x in window.min_x..=window.max_x {
            let raw_score = score_window(search, &template_stats, &integrals, metric, x, y);
            push_candidate(&mut candidates, MatchCandidate { x, y, raw_score }, limit);
        }
    }

    if candidates.is_empty() {
        candidates.push(MatchCandidate {
            x: window.min_x,
            y: window.min_y,
            raw_score: 0.0,
        });
    }
    Ok(candidates)
}

fn score_window(
    search: &GrayImage,
    template: &TemplateStats,
    integrals: &IntegralImages,
    metric: MatchMetric,
    x: u32,
    y: u32,
) -> f32 {
    let image_sum = integrals.sum(x, y, template.width, template.height);
    let image_sq_sum = integrals.squared_sum(x, y, template.width, template.height);
    let dot = dot_product(search, template, x, y);

    let raw = match metric {
        MatchMetric::CrossCorrelationNormalized => {
            let denominator = (image_sq_sum * template.squared_sum).sqrt();
            if denominator <= f64::EPSILON {
                0.0
            } else {
                dot / denominator
            }
        }
        MatchMetric::CorrelationCoefficientNormalized => {
            let count = template.count;
            let window_norm_sq = image_sq_sum - (image_sum * image_sum / count);
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

fn dot_product(search: &GrayImage, template: &TemplateStats, x: u32, y: u32) -> f64 {
    let search_width = search.width() as usize;
    let x = x as usize;
    let y = y as usize;
    let template_width = template.width as usize;
    let template_height = template.height as usize;
    let search_pixels = search.as_raw();
    let mut dot = 0.0;

    for ty in 0..template_height {
        let search_start = (y + ty) * search_width + x;
        let template_start = ty * template_width;
        let search_row = &search_pixels[search_start..search_start + template_width];
        let template_row = &template.values[template_start..template_start + template_width];
        for (image, template) in search_row.iter().zip(template_row.iter()) {
            dot += f64::from(*image) * template;
        }
    }
    dot
}

fn push_candidate(candidates: &mut Vec<MatchCandidate>, candidate: MatchCandidate, limit: usize) {
    candidates.push(candidate);
    candidates.sort_by(|a, b| b.raw_score.total_cmp(&a.raw_score));
    candidates.truncate(limit);
}

fn should_use_fast_path(search: &GrayImage, template: &GrayImage) -> bool {
    estimated_work(
        search.width(),
        search.height(),
        template.width(),
        template.height(),
    ) > FULL_FRAME_FAST_PATH_WORK_THRESHOLD
}

fn choose_downsample_factor(search: &GrayImage, template: &GrayImage) -> u32 {
    let mut factor = 1;
    while factor < FULL_FRAME_MAX_DOWNSAMPLE {
        let next = factor * 2;
        let work = estimated_work(
            scaled_dim(search.width(), next),
            scaled_dim(search.height(), next),
            scaled_dim(template.width(), next),
            scaled_dim(template.height(), next),
        );
        factor = next;
        if work <= FULL_FRAME_COARSE_WORK_TARGET {
            break;
        }
    }
    factor
}

fn downsample_gray(image: &GrayImage, factor: u32) -> GrayImage {
    imageops::resize(
        image,
        scaled_dim(image.width(), factor),
        scaled_dim(image.height(), factor),
        imageops::FilterType::Triangle,
    )
}

fn scaled_dim(value: u32, factor: u32) -> u32 {
    value.div_ceil(factor).max(1)
}

fn estimated_work(
    search_width: u32,
    search_height: u32,
    template_width: u32,
    template_height: u32,
) -> u64 {
    if template_width > search_width || template_height > search_height {
        return 0;
    }
    let output_width = u64::from(search_width - template_width + 1);
    let output_height = u64::from(search_height - template_height + 1);
    output_width * output_height * u64::from(template_width) * u64::from(template_height)
}

#[derive(Debug, Clone, Copy)]
struct SearchWindow {
    min_x: u32,
    max_x: u32,
    min_y: u32,
    max_y: u32,
}

impl SearchWindow {
    fn full(search: &GrayImage, template: &GrayImage) -> Self {
        Self {
            min_x: 0,
            max_x: search.width() - template.width(),
            min_y: 0,
            max_y: search.height() - template.height(),
        }
    }

    fn around(self, x: u32, y: u32, radius: u32) -> Self {
        Self {
            min_x: x.saturating_sub(radius).max(self.min_x),
            max_x: x.saturating_add(radius).min(self.max_x),
            min_y: y.saturating_sub(radius).max(self.min_y),
            max_y: y.saturating_add(radius).min(self.max_y),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct MatchCandidate {
    x: u32,
    y: u32,
    raw_score: f32,
}

struct TemplateStats {
    width: u32,
    height: u32,
    count: f64,
    values: Vec<f64>,
    mean: f64,
    squared_sum: f64,
    centered_norm_sq: f64,
}

impl TemplateStats {
    fn new(template: &GrayImage, metric: MatchMetric) -> RecognitionResult<Self> {
        let values = template
            .as_raw()
            .iter()
            .map(|value| f64::from(*value))
            .collect::<Vec<_>>();
        let count = values.len() as f64;
        let sum: f64 = values.iter().sum();
        let squared_sum: f64 = values.iter().map(|value| value * value).sum();
        let mean = sum / count;
        let centered_norm_sq = squared_sum - (sum * sum / count);
        if metric == MatchMetric::CorrelationCoefficientNormalized
            && centered_norm_sq <= f64::EPSILON
        {
            return Err(RecognitionError::fatal(
                "ccoeff_normed template must have non-zero variance",
            ));
        }
        Ok(Self {
            width: template.width(),
            height: template.height(),
            count,
            values,
            mean,
            squared_sum,
            centered_norm_sq,
        })
    }
}

struct IntegralImages {
    stride: usize,
    sum: Vec<f64>,
    squared_sum: Vec<f64>,
}

impl IntegralImages {
    fn new(image: &GrayImage) -> Self {
        let width = image.width() as usize;
        let height = image.height() as usize;
        let stride = width + 1;
        let mut sum = vec![0.0; stride * (height + 1)];
        let mut squared_sum = vec![0.0; stride * (height + 1)];
        let pixels = image.as_raw();

        for y in 0..height {
            let mut row_sum = 0.0;
            let mut row_squared_sum = 0.0;
            for x in 0..width {
                let value = f64::from(pixels[y * width + x]);
                row_sum += value;
                row_squared_sum += value * value;
                let current = (y + 1) * stride + x + 1;
                let above = y * stride + x + 1;
                sum[current] = sum[above] + row_sum;
                squared_sum[current] = squared_sum[above] + row_squared_sum;
            }
        }

        Self {
            stride,
            sum,
            squared_sum,
        }
    }

    fn sum(&self, x: u32, y: u32, width: u32, height: u32) -> f64 {
        self.rect_sum(&self.sum, x, y, width, height)
    }

    fn squared_sum(&self, x: u32, y: u32, width: u32, height: u32) -> f64 {
        self.rect_sum(&self.squared_sum, x, y, width, height)
    }

    fn rect_sum(&self, data: &[f64], x: u32, y: u32, width: u32, height: u32) -> f64 {
        let x1 = x as usize;
        let y1 = y as usize;
        let x2 = x1 + width as usize;
        let y2 = y1 + height as usize;
        data[y2 * self.stride + x2] - data[y1 * self.stride + x2] - data[y2 * self.stride + x1]
            + data[y1 * self.stride + x1]
    }
}

struct TemplateMatchDeadline {
    started: Instant,
    timeout: Duration,
}

impl TemplateMatchDeadline {
    fn new(timeout: Duration) -> Self {
        Self {
            started: Instant::now(),
            timeout,
        }
    }

    fn check(&self, label: &str) -> RecognitionResult<()> {
        if self.started.elapsed() > self.timeout {
            return Err(RecognitionError::fatal(format!(
                "{label} exceeded {} ms deadline",
                self.timeout.as_millis()
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
struct Bounds {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

fn validate_rect(rect: Rect, frame_width: u32, frame_height: u32) -> RecognitionResult<Bounds> {
    if rect.x < 0 || rect.y < 0 {
        return Err(RecognitionError::fatal(format!(
            "rect coordinates must be non-negative: ({}, {})",
            rect.x, rect.y
        )));
    }
    if rect.width <= 0 || rect.height <= 0 {
        return Err(RecognitionError::fatal(format!(
            "rect dimensions must be positive: {}x{}",
            rect.width, rect.height
        )));
    }

    let x = rect.x as u32;
    let y = rect.y as u32;
    let width = rect.width as u32;
    let height = rect.height as u32;
    let right = x
        .checked_add(width)
        .ok_or_else(|| RecognitionError::fatal("rect x + width overflows u32"))?;
    let bottom = y
        .checked_add(height)
        .ok_or_else(|| RecognitionError::fatal("rect y + height overflows u32"))?;

    if right > frame_width || bottom > frame_height {
        return Err(RecognitionError::fatal(format!(
            "rect {}x{} at ({}, {}) exceeds frame {}x{}",
            width, height, x, y, frame_width, frame_height
        )));
    }

    Ok(Bounds {
        x,
        y,
        width,
        height,
    })
}

fn validate_pixels(
    width: u32,
    height: u32,
    len: usize,
    bytes_per_pixel: usize,
    label: &str,
) -> RecognitionResult<()> {
    if width == 0 || height == 0 {
        return Err(RecognitionError::fatal(format!(
            "{label} scene dimensions must be non-zero: {width}x{height}"
        )));
    }
    let expected = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(bytes_per_pixel))
        .ok_or_else(|| {
            RecognitionError::fatal(format!(
                "{label} scene dimensions overflow: {width}x{height}"
            ))
        })?;
    if len != expected {
        return Err(RecognitionError::fatal(format!(
            "{label} scene pixel length mismatch for {width}x{height}: got {len}, expected {expected}"
        )));
    }
    Ok(())
}

fn decode_error(label: &str, err: ImageError) -> RecognitionError {
    RecognitionError::fatal(format!("failed to decode {label}: {err}"))
}

fn euclidean_distance(mean: [u8; 3], expected: [u8; 3]) -> f32 {
    let dr = f32::from(mean[0]) - f32::from(expected[0]);
    let dg = f32::from(mean[1]) - f32::from(expected[1]);
    let db = f32::from(mean[2]) - f32::from(expected[2]);
    (dr * dr + dg * dg + db * db).sqrt()
}

// CrossCorrelationNormalized on non-negative pixels is already a [0, 1] metric; P4a only normalizes scores and leaves thresholds to callers.
fn normalize_ncc_score(raw: f32) -> f32 {
    if raw.is_nan() {
        0.0
    } else {
        raw.clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageBuffer, Rgb};
    use std::io::Cursor;

    #[test]
    fn scene_from_rgb8_uses_raw_pixels() {
        let scene = Scene::from_rgb8(2, 1, &[255, 0, 0, 0, 255, 0]).expect("rgb scene");

        assert_eq!(scene.width(), 2);
        assert_eq!(scene.height(), 1);
    }

    #[test]
    fn scene_from_rgba8_uses_raw_pixels() {
        let scene = Scene::from_rgba8(1, 2, &[255, 0, 0, 255, 0, 255, 0, 128]).expect("rgba scene");

        assert_eq!(scene.width(), 1);
        assert_eq!(scene.height(), 2);
    }

    #[test]
    fn scene_from_pixels_rejects_bad_length() {
        let err =
            Scene::from_pixels(2, 2, &[0, 1, 2], ScenePixelFormat::Rgb8).expect_err("bad length");

        assert_eq!(err.severity(), RecognitionErrorSeverity::Fatal);
    }

    #[test]
    fn template_match_finds_non_uniform_template() {
        let template = template_image();
        let scene = scene_with_template(200, 150, &template);

        let matched = scene
            .match_template(&encode_png(&template), None)
            .expect("template match");

        assert_eq!((matched.x, matched.y), (200, 150));
        assert!(
            matched.raw_score >= 0.99,
            "raw_score was {}",
            matched.raw_score
        );
        assert!(matched.score >= 0.99, "score was {}", matched.score);
        assert_score_is_normalized(matched.score);
    }

    #[test]
    fn frame_without_template_scores_lower() {
        let scene =
            Scene::from_png(&encode_png(&blank_image(320, 240, [30, 31, 32]))).expect("scene");
        let template = template_image();

        let matched = scene
            .match_template(&encode_png(&template), None)
            .expect("template match");

        assert!(matched.score < 0.95, "score was {}", matched.score);
        assert_score_is_normalized(matched.score);
    }

    #[test]
    fn template_match_respects_region_and_returns_full_frame_coordinates() {
        let template = template_image();
        let scene = scene_with_template(200, 150, &template);
        let region = Rect {
            x: 180,
            y: 130,
            width: 80,
            height: 70,
        };

        let matched = scene
            .match_template(&encode_png(&template), Some(region))
            .expect("template match");

        assert_eq!((matched.x, matched.y), (200, 150));
        assert!(
            matched.raw_score >= 0.99,
            "raw_score was {}",
            matched.raw_score
        );
        assert!(matched.score >= 0.99, "score was {}", matched.score);
        assert_score_is_normalized(matched.score);
    }

    #[test]
    fn ccoeff_template_match_finds_non_uniform_template() {
        let template = template_image();
        let scene = scene_with_template(200, 150, &template);

        let matched = scene
            .match_template_with_metric(
                &encode_png(&template),
                None,
                MatchMetric::CorrelationCoefficientNormalized,
            )
            .expect("template match");

        assert_eq!((matched.x, matched.y), (200, 150));
        assert!(
            matched.raw_score >= 0.99,
            "raw_score was {}",
            matched.raw_score
        );
        assert!(matched.score >= 0.99, "score was {}", matched.score);
        assert_score_is_normalized(matched.score);
    }

    #[test]
    fn full_frame_ccoeff_large_template_returns_exact_match() {
        let template = patterned_image(180, 28);
        let scene = scene_with_large_template(270, 190, &template);
        let started = Instant::now();

        let matched = scene
            .match_template_with_metric(
                &encode_png(&template),
                None,
                MatchMetric::CorrelationCoefficientNormalized,
            )
            .expect("template match");

        assert_eq!((matched.x, matched.y), (270, 190));
        assert!(matched.score >= 0.99, "score was {}", matched.score);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "full-frame ccoeff match took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn full_frame_ccorr_large_template_returns_exact_match() {
        let template = patterned_image(180, 28);
        let scene = scene_with_large_template(270, 190, &template);
        let started = Instant::now();

        let matched = scene
            .match_template_with_metric(
                &encode_png(&template),
                None,
                MatchMetric::CrossCorrelationNormalized,
            )
            .expect("template match");

        assert_eq!((matched.x, matched.y), (270, 190));
        assert!(matched.score >= 0.99, "score was {}", matched.score);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "full-frame ccorr match took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn bounded_ccoeff_template_match_returns_full_frame_coordinates() {
        let template = template_image();
        let scene = scene_with_template(200, 150, &template);
        let region = Rect {
            x: 180,
            y: 130,
            width: 80,
            height: 70,
        };

        let matched = scene
            .match_template_with_metric(
                &encode_png(&template),
                Some(region),
                MatchMetric::CorrelationCoefficientNormalized,
            )
            .expect("template match");

        assert_eq!((matched.x, matched.y), (200, 150));
        assert!(matched.score >= 0.99, "score was {}", matched.score);
    }

    #[test]
    fn large_explicit_region_ccoeff_uses_fast_path() {
        let template = patterned_image(180, 28);
        let scene = scene_with_large_template(270, 190, &template);
        let region = Rect {
            x: 0,
            y: 0,
            width: 640,
            height: 360,
        };
        let started = Instant::now();

        let matched = scene
            .match_template_with_metric(
                &encode_png(&template),
                Some(region),
                MatchMetric::CorrelationCoefficientNormalized,
            )
            .expect("template match");

        assert_eq!((matched.x, matched.y), (270, 190));
        assert!(matched.score >= 0.99, "score was {}", matched.score);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "large explicit-region ccoeff match took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn ccoeff_rejects_zero_variance_template() {
        let scene =
            Scene::from_png(&encode_png(&blank_image(40, 40, [30, 31, 32]))).expect("scene");
        let template = blank_image(8, 8, [255, 255, 255]);

        let err = scene
            .match_template_with_metric(
                &encode_png(&template),
                None,
                MatchMetric::CorrelationCoefficientNormalized,
            )
            .expect_err("zero variance template rejected");

        assert_eq!(err.severity(), RecognitionErrorSeverity::Fatal);
        assert!(err.message().contains("non-zero variance"));
    }

    #[test]
    fn template_match_excluded_region_returns_point_inside_region() {
        let template = template_image();
        let scene = scene_with_template(200, 150, &template);
        let region = Rect {
            x: 10,
            y: 12,
            width: 100,
            height: 90,
        };

        let matched = scene
            .match_template(&encode_png(&template), Some(region))
            .expect("template match");

        assert!(matched.x >= region.x);
        assert!(matched.y >= region.y);
        assert!(matched.x < region.x + region.width);
        assert!(matched.y < region.y + region.height);
        assert_score_is_normalized(matched.score);
    }

    #[test]
    fn normalize_ncc_score_uses_identity_clamp_semantics() {
        assert_eq!(normalize_ncc_score(0.0), 0.0);
        assert_eq!(normalize_ncc_score(0.5), 0.5);
        assert_eq!(normalize_ncc_score(1.0), 1.0);
        assert_eq!(normalize_ncc_score(-0.2), 0.0);
        assert_eq!(normalize_ncc_score(1.3), 1.0);
        assert_eq!(normalize_ncc_score(f32::NAN), 0.0);
    }

    #[test]
    fn invalid_rects_are_fatal() {
        let scene = Scene::from_png(&encode_png(&blank_image(100, 80, [0, 0, 0]))).expect("scene");
        let invalid = [
            Rect {
                x: -1,
                y: 0,
                width: 10,
                height: 10,
            },
            Rect {
                x: 0,
                y: -1,
                width: 10,
                height: 10,
            },
            Rect {
                x: 0,
                y: 0,
                width: 0,
                height: 10,
            },
            Rect {
                x: 0,
                y: 0,
                width: 10,
                height: 0,
            },
            Rect {
                x: 95,
                y: 0,
                width: 10,
                height: 10,
            },
            Rect {
                x: 0,
                y: 75,
                width: 10,
                height: 10,
            },
        ];

        for rect in invalid {
            let err = scene
                .compare_color(rect, [0, 0, 0])
                .expect_err("invalid rect rejected");
            assert_eq!(err.severity(), RecognitionErrorSeverity::Fatal);
        }
    }

    #[test]
    fn template_larger_than_frame_or_region_is_fatal() {
        let scene = Scene::from_png(&encode_png(&blank_image(40, 40, [0, 0, 0]))).expect("scene");
        let template = blank_image(50, 50, [255, 255, 255]);

        let err = scene
            .match_template(&encode_png(&template), None)
            .expect_err("large template rejected");
        assert_eq!(err.severity(), RecognitionErrorSeverity::Fatal);

        let scene = Scene::from_png(&encode_png(&blank_image(100, 100, [0, 0, 0]))).expect("scene");
        let err = scene
            .match_template(
                &encode_png(&template),
                Some(Rect {
                    x: 0,
                    y: 0,
                    width: 30,
                    height: 30,
                }),
            )
            .expect_err("large template rejected for region");
        assert_eq!(err.severity(), RecognitionErrorSeverity::Fatal);
    }

    #[test]
    fn bad_png_is_fatal() {
        let err = Scene::from_png(b"not png").expect_err("bad scene rejected");
        assert_eq!(err.severity(), RecognitionErrorSeverity::Fatal);

        let scene = Scene::from_png(&encode_png(&blank_image(100, 100, [0, 0, 0]))).expect("scene");
        let err = scene
            .match_template(b"not png", None)
            .expect_err("bad template rejected");
        assert_eq!(err.severity(), RecognitionErrorSeverity::Fatal);
    }

    #[test]
    fn compare_color_returns_mean_and_distance() {
        let scene = Scene::from_png(&encode_png(&blank_image(20, 20, [255, 0, 0]))).expect("scene");
        let region = Rect {
            x: 0,
            y: 0,
            width: 20,
            height: 20,
        };

        let red = scene.compare_color(region, [255, 0, 0]).expect("red");
        assert_eq!(red.mean, [255, 0, 0]);
        assert!(red.distance <= f32::EPSILON);

        let blue = scene.compare_color(region, [0, 0, 255]).expect("blue");
        assert!(blue.distance > 300.0, "distance was {}", blue.distance);
    }

    fn blank_image(width: u32, height: u32, color: [u8; 3]) -> RgbImage {
        ImageBuffer::from_pixel(width, height, Rgb(color))
    }

    fn template_image() -> RgbImage {
        patterned_image(24, 18)
    }

    fn patterned_image(width: u32, height: u32) -> RgbImage {
        let mut image = RgbImage::new(width, height);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            *pixel = Rgb([
                ((x * 9 + y * 3) % 251) as u8,
                ((x * 5 + y * 13 + 17) % 239) as u8,
                ((x * 7 + y * 11 + 29) % 227) as u8,
            ]);
        }
        image
    }

    fn scene_with_template(x: u32, y: u32, template: &RgbImage) -> Scene {
        let mut frame = blank_image(320, 240, [30, 31, 32]);
        imageops::replace(&mut frame, template, i64::from(x), i64::from(y));
        Scene::from_png(&encode_png(&frame)).expect("scene")
    }

    fn scene_with_large_template(x: u32, y: u32, template: &RgbImage) -> Scene {
        let mut frame = blank_image(640, 360, [30, 31, 32]);
        imageops::replace(&mut frame, template, i64::from(x), i64::from(y));
        Scene::from_png(&encode_png(&frame)).expect("scene")
    }

    fn encode_png(image: &RgbImage) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(image.clone())
            .write_to(&mut out, ImageFormat::Png)
            .expect("encode PNG");
        out.into_inner()
    }

    fn assert_score_is_normalized(score: f32) {
        assert!((0.0..=1.0).contains(&score), "score was {score}");
    }
}
