// SPDX-License-Identifier: AGPL-3.0-only

//! Brightness and contrast of a matched window relative to its template (Workflow #411 B13,
//! B-train plan §2.10; `contracts/resource-declarations.md`, section Task schema `0.9`
//! brightness and contrast floors).
//!
//! Both are read on the gray images a template search reads, over the template's scored pixels:
//! every pixel, or the pixels its mask selects (Workflow #396 REC-7), of the template as searched
//! (resampled by its factor, REC-5). The mean ratio is the window's mean over the template's, the
//! contrast ratio the window's standard deviation over the template's, each times 1000 and
//! rounded down, in integer arithmetic. This is no frame-change measure.

use crate::{
    RecognitionError, RecognitionResult, Rect, Scene, TemplateMask, TemplateMatch, decode_template,
    scaled_template, validate_rect,
};
use image::GrayImage;

/// The template side of the ratios: the template's gray image as searched at one factor, with
/// the count, sum and sum of squares of its scored pixels.
#[derive(Debug, Clone)]
pub struct LumaReference {
    template: GrayImage,
    mask: Option<TemplateMask>,
    sums: PixelSums,
}

/// A window's brightness and contrast relative to its template's, in milli (1000: equal).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LumaRatios {
    pub mean_ratio_milli: u32,
    pub contrast_ratio_milli: u32,
}

#[derive(Debug, Clone, Copy, Default)]
struct PixelSums {
    count: u128,
    sum: u128,
    squared_sum: u128,
}

impl PixelSums {
    fn add(&mut self, value: u8) {
        let value = u128::from(value);
        self.count += 1;
        self.sum += value;
        self.squared_sum += value * value;
    }

    /// `count * sum(v^2) - sum(v)^2`: the variance times `count^2`, never negative.
    fn spread(&self) -> u128 {
        self.count * self.squared_sum - self.sum * self.sum
    }
}

impl LumaReference {
    /// The reference of the template `template_png` decodes to, resampled by `factor_milli`
    /// (1000: as declared) and scored over the pixels `mask` selects (every pixel when `None`).
    /// A template whose scored pixels are all black, or all of one value, has no brightness or
    /// no contrast to compare with, and is refused.
    pub fn of_png_scaled(
        template_png: &[u8],
        mask: Option<TemplateMask>,
        factor_milli: u32,
    ) -> RecognitionResult<Self> {
        let template = scaled_template(&decode_template(template_png)?, factor_milli)?;
        let mut sums = PixelSums::default();
        for value in template.as_raw() {
            if mask.is_none_or(|mask| mask.selects(*value)) {
                sums.add(*value);
            }
        }
        if sums.sum == 0 {
            return Err(RecognitionError::fatal(
                "luma reference template has no brightness over its scored pixels",
            ));
        }
        if sums.spread() == 0 {
            return Err(RecognitionError::fatal(
                "luma reference template has no contrast over its scored pixels",
            ));
        }
        Ok(Self {
            template,
            mask,
            sums,
        })
    }

    /// The template's size as searched.
    pub fn dimensions(&self) -> (u32, u32) {
        self.template.dimensions()
    }
}

impl Scene {
    /// The ratios of the window `matched` covers in this frame, read over `reference`'s scored
    /// pixels. `matched` is a match of `reference`'s template, so its box has the template's size.
    pub fn luma_ratios(
        &self,
        reference: &LumaReference,
        matched: TemplateMatch,
    ) -> RecognitionResult<LumaRatios> {
        let (width, height) = reference.dimensions();
        if i64::from(matched.width) != i64::from(width)
            || i64::from(matched.height) != i64::from(height)
        {
            return Err(RecognitionError::fatal(format!(
                "luma window {}x{} differs from its template {width}x{height}",
                matched.width, matched.height
            )));
        }
        let bounds = validate_rect(
            Rect {
                x: matched.x,
                y: matched.y,
                width: matched.width,
                height: matched.height,
            },
            self.width(),
            self.height(),
        )?;
        let frame_width = self.gray.width() as usize;
        let frame = self.gray.as_raw();
        let template = reference.template.as_raw();
        let mut window = PixelSums::default();
        for row in 0..height as usize {
            let frame_start = (bounds.y as usize + row) * frame_width + bounds.x as usize;
            let template_start = row * width as usize;
            let frame_row = &frame[frame_start..frame_start + width as usize];
            let template_row = &template[template_start..template_start + width as usize];
            for (value, scored) in frame_row.iter().zip(template_row) {
                if reference.mask.is_none_or(|mask| mask.selects(*scored)) {
                    window.add(*value);
                }
            }
        }
        Ok(LumaRatios {
            // Equal counts: the ratio of the means is the ratio of the sums.
            mean_ratio_milli: saturating_u32(window.sum * 1000 / reference.sums.sum),
            // The ratio of the deviations is sqrt(spread / spread) of equal counts; rounding the
            // radicand down first does not change the rounded-down root.
            contrast_ratio_milli: saturating_u32(
                (window.spread() * 1_000_000 / reference.sums.spread()).isqrt(),
            ),
        })
    }
}

fn saturating_u32(value: u128) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}
