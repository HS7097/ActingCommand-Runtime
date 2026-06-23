// SPDX-License-Identifier: AGPL-3.0-only

use image::{GrayImage, ImageError, ImageFormat, RgbImage, imageops};
use imageproc::template_matching::{
    MatchTemplateMethod, find_extremes, match_template as match_template_map,
};
use std::error::Error;
use std::fmt;

pub type RecognitionResult<T> = Result<T, RecognitionError>;

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

impl Scene {
    pub fn from_png(png: &[u8]) -> RecognitionResult<Scene> {
        let image = image::load_from_memory_with_format(png, ImageFormat::Png)
            .map_err(|err| decode_error("scene PNG", err))?;
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

        match metric {
            MatchMetric::CrossCorrelationNormalized => {
                // Grayscale template matching plus independent color comparison mirrors an area+color primitive; thresholds stay with callers.
                let response = match_template_map(
                    &search,
                    &template,
                    MatchTemplateMethod::CrossCorrelationNormalized,
                );
                let extremes = find_extremes(&response);
                let raw_score = extremes.max_value;
                let score = normalize_ncc_score(raw_score);
                let x = i32::try_from(offset_x + extremes.max_value_location.0).map_err(|_| {
                    RecognitionError::fatal("template match x coordinate exceeds i32 range")
                })?;
                let y = i32::try_from(offset_y + extremes.max_value_location.1).map_err(|_| {
                    RecognitionError::fatal("template match y coordinate exceeds i32 range")
                })?;

                Ok(TemplateMatch {
                    x,
                    y,
                    raw_score,
                    score,
                })
            }
            MatchMetric::CorrelationCoefficientNormalized => {
                ccoeff_normed_match(&search, &template, offset_x, offset_y)
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

fn ccoeff_normed_match(
    search: &GrayImage,
    template: &GrayImage,
    offset_x: u32,
    offset_y: u32,
) -> RecognitionResult<TemplateMatch> {
    let template_width = template.width();
    let template_height = template.height();
    let count = (template_width * template_height) as f32;
    let template_sum: f32 = template.pixels().map(|pixel| f32::from(pixel[0])).sum();
    let template_mean = template_sum / count;
    let template_centered = template
        .pixels()
        .map(|pixel| f32::from(pixel[0]) - template_mean)
        .collect::<Vec<_>>();
    let template_norm_sq: f32 = template_centered.iter().map(|value| value * value).sum();
    if template_norm_sq <= f32::EPSILON {
        return Err(RecognitionError::fatal(
            "ccoeff_normed template must have non-zero variance",
        ));
    }
    let template_norm = template_norm_sq.sqrt();

    let max_x = search.width() - template_width;
    let max_y = search.height() - template_height;
    let mut best_raw = f32::NEG_INFINITY;
    let mut best_x = 0_u32;
    let mut best_y = 0_u32;

    for y in 0..=max_y {
        for x in 0..=max_x {
            let mut window_sum = 0_f32;
            for ty in 0..template_height {
                for tx in 0..template_width {
                    window_sum += f32::from(search.get_pixel(x + tx, y + ty)[0]);
                }
            }
            let window_mean = window_sum / count;
            let mut numerator = 0_f32;
            let mut window_norm_sq = 0_f32;
            let mut index = 0_usize;
            for ty in 0..template_height {
                for tx in 0..template_width {
                    let window_value = f32::from(search.get_pixel(x + tx, y + ty)[0]) - window_mean;
                    numerator += window_value * template_centered[index];
                    window_norm_sq += window_value * window_value;
                    index += 1;
                }
            }
            if window_norm_sq <= f32::EPSILON {
                continue;
            }
            let raw = numerator / (window_norm_sq.sqrt() * template_norm);
            if raw > best_raw {
                best_raw = raw;
                best_x = x;
                best_y = y;
            }
        }
    }

    if !best_raw.is_finite() {
        best_raw = 0.0;
    }
    let x = i32::try_from(offset_x + best_x)
        .map_err(|_| RecognitionError::fatal("template match x coordinate exceeds i32 range"))?;
    let y = i32::try_from(offset_y + best_y)
        .map_err(|_| RecognitionError::fatal("template match y coordinate exceeds i32 range"))?;

    Ok(TemplateMatch {
        x,
        y,
        raw_score: best_raw,
        score: normalize_ncc_score(best_raw),
    })
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
        let mut image = RgbImage::new(24, 18);
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
