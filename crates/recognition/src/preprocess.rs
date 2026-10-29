// SPDX-License-Identifier: AGPL-3.0-only

//! Recognition preprocessing (Workflow #411 A58, #428; design CF §4, slice CFL-1, since 0.12.29):
//! one fixed chain of at most four steps that turns the RGB pixels of a region into a gray or
//! binary region before a recognition reads it.
//!
//! A chain has one of two shapes, and its steps come in one order only:
//! - a color keep, `keep_colors` or `keep_range`, optionally followed by `invert`;
//! - `gray`, optionally followed by `contrast`, `threshold` and `invert`, in that order, each at
//!   most once.
//!
//! A kept pixel becomes 255 and every other pixel 0; `invert` flips them. Every step except `gray`
//! is integer arithmetic. `gray` is the gray every template search reads (the [`Scene`]'s), so a
//! `[gray]` chain gives the scene's gray byte for byte. The chain is a pure function of the pixels
//! and the declaration: the same region and chain always give the same bytes and evidence.
//! `contracts/recognition-preprocess.md` spells out the arithmetic, the evidence and the refusals.

use super::{RecognitionError, RecognitionResult, Rect, Scene, validate_rect};
use image::{DynamicImage, GrayImage, RgbImage, imageops};
use std::error::Error;
use std::fmt;

/// The most steps a chain holds (`gray`, `contrast`, `threshold`, `invert`).
pub const MAX_STEPS: usize = 4;
/// The most colors one `keep_colors` step lists.
pub const MAX_COLORS: usize = 8;
/// The largest hue of the HSV scale (the OpenCV 8-bit scale: degrees halved, 0 to 179).
pub const HUE_MAX: u8 = 179;
/// A processed pixel of at least this value counts as foreground in `foreground_milli`.
pub const FOREGROUND_MIN: u8 = 128;
/// The declared `value` of a `threshold` step that computes its threshold from the region.
pub const OTSU: &str = "otsu";

/// The six step kinds, by their declared `kind` name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PreprocessKind {
    KeepColors,
    KeepRange,
    Gray,
    Contrast,
    Threshold,
    Invert,
}

impl PreprocessKind {
    pub const ALL: [Self; 6] = [
        Self::KeepColors,
        Self::KeepRange,
        Self::Gray,
        Self::Contrast,
        Self::Threshold,
        Self::Invert,
    ];

    /// The kind's declared name (the step's `kind`).
    pub const fn name(self) -> &'static str {
        match self {
            Self::KeepColors => "keep_colors",
            Self::KeepRange => "keep_range",
            Self::Gray => "gray",
            Self::Contrast => "contrast",
            Self::Threshold => "threshold",
            Self::Invert => "invert",
        }
    }

    /// The kind a declared name names; `None` for any other name.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }

    /// The members a step of this kind declares besides `kind`. Every one is required and none
    /// has a default.
    pub const fn members(self) -> &'static [&'static str] {
        match self {
            Self::KeepColors => &["colors", "tolerance"],
            Self::KeepRange => &["space", "lower", "upper"],
            Self::Contrast => &["low", "high"],
            Self::Threshold => &["value"],
            Self::Gray | Self::Invert => &[],
        }
    }

    /// Whether a chain may start with this kind (`keep_colors`, `keep_range`, `gray`).
    pub const fn starts_chain(self) -> bool {
        matches!(self, Self::KeepColors | Self::KeepRange | Self::Gray)
    }

    /// The kinds that may follow this first kind, in the only order they may come in.
    const fn followers(self) -> &'static [Self] {
        match self {
            Self::Gray => &[Self::Contrast, Self::Threshold, Self::Invert],
            _ => &[Self::Invert],
        }
    }
}

impl fmt::Display for PreprocessKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// The color space a `keep_range` step compares in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ColorSpace {
    /// The pixel's own R, G and B, each 0 to 255.
    Rgb,
    /// H 0 to 179, S and V 0 to 255 ([`rgb_to_hsv`]).
    Hsv,
}

impl ColorSpace {
    /// The space's declared name (`space`).
    pub const fn name(self) -> &'static str {
        match self {
            Self::Rgb => "rgb",
            Self::Hsv => "hsv",
        }
    }

    /// The space a declared name names; `None` for any other name.
    pub fn from_name(name: &str) -> Option<Self> {
        [Self::Rgb, Self::Hsv]
            .into_iter()
            .find(|space| space.name() == name)
    }
}

/// The threshold of a `threshold` step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThresholdValue {
    /// A declared threshold, 1 to 255: a value at least this high becomes 255.
    Fixed(u8),
    /// Otsu's threshold of the region (`"otsu"`).
    Otsu,
}

/// One step of a chain, as declared.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PreprocessStep {
    /// Keeps a pixel that is within `tolerance` of one of `colors` on each of R, G and B.
    KeepColors { colors: Vec<[u8; 3]>, tolerance: u8 },
    /// Keeps a pixel whose components in `space` lie in `lower..=upper`; in HSV a hue range whose
    /// `lower` is above its `upper` wraps through 0.
    KeepRange {
        space: ColorSpace,
        lower: [u8; 3],
        upper: [u8; 3],
    },
    /// The gray of the pixel, as every template search reads it.
    Gray,
    /// Stretches `low..=high` to `0..=255`.
    Contrast { low: u8, high: u8 },
    /// A value at least the threshold becomes 255, any other 0.
    Threshold { value: ThresholdValue },
    /// `255 - v`.
    Invert,
}

impl PreprocessStep {
    pub fn kind(&self) -> PreprocessKind {
        match self {
            Self::KeepColors { .. } => PreprocessKind::KeepColors,
            Self::KeepRange { .. } => PreprocessKind::KeepRange,
            Self::Gray => PreprocessKind::Gray,
            Self::Contrast { .. } => PreprocessKind::Contrast,
            Self::Threshold { .. } => PreprocessKind::Threshold,
            Self::Invert => PreprocessKind::Invert,
        }
    }
}

/// A chain that holds the syntax: one of the two shapes, at most [`MAX_STEPS`] steps, every
/// member in its range. Two chains are the same exactly when their steps are equal.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Preprocess {
    steps: Vec<PreprocessStep>,
}

/// What a chain gave for one region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreprocessOutput {
    /// The processed region, the size of the input region.
    pub image: GrayImage,
    pub evidence: PreprocessEvidence,
}

/// The evidence of one application, for read-only and Lab output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreprocessEvidence {
    /// The share of processed pixels of at least [`FOREGROUND_MIN`], in thousandths, rounded down.
    pub foreground_milli: u32,
    /// The threshold the `threshold` step used (Otsu's, as computed); `None` without a `threshold`
    /// step and for Otsu on a region of one value.
    pub threshold_value: Option<u8>,
}

impl Preprocess {
    /// The chain of `steps`, refused by its first step that breaks the syntax.
    pub fn new(steps: Vec<PreprocessStep>) -> Result<Self, PreprocessError> {
        validate(&steps)?;
        Ok(Self { steps })
    }

    pub fn steps(&self) -> &[PreprocessStep] {
        &self.steps
    }

    /// The chain applied to the RGB pixels of `region` (at least one pixel).
    pub fn apply(&self, region: &RgbImage) -> RecognitionResult<PreprocessOutput> {
        let (width, height) = region.dimensions();
        if width == 0 || height == 0 {
            return Err(RecognitionError::fatal(format!(
                "preprocessing needs a region of at least one pixel: {width}x{height}"
            )));
        }
        let (first, rest) = self
            .steps
            .split_first()
            .ok_or_else(|| RecognitionError::fatal("preprocessing chain has no step"))?;
        let mut values = match first {
            PreprocessStep::KeepColors { colors, tolerance } => {
                keep(region, |pixel| keeps_color(pixel, colors, *tolerance))
            }
            PreprocessStep::KeepRange {
                space,
                lower,
                upper,
            } => keep(region, |pixel| in_range(pixel, *space, *lower, *upper)),
            // The conversion `Scene` makes its gray with (image 0.25 `to_luma8`), on the region's
            // own pixels and color space.
            PreprocessStep::Gray => DynamicImage::ImageRgb8(region.clone())
                .to_luma8()
                .into_raw(),
            other => {
                return Err(RecognitionError::fatal(format!(
                    "preprocessing chain cannot start with {}",
                    other.kind()
                )));
            }
        };
        let mut threshold_value = None;
        for step in rest {
            match step {
                PreprocessStep::Contrast { low, high } => {
                    for pixel in &mut values {
                        *pixel = stretch(*pixel, *low, *high);
                    }
                }
                PreprocessStep::Threshold { value } => {
                    threshold_value = match value {
                        ThresholdValue::Fixed(threshold) => Some(*threshold),
                        ThresholdValue::Otsu => otsu_threshold(&values)?,
                    };
                    // Otsu on a region of one value has no threshold: the whole region is 0.
                    for pixel in &mut values {
                        *pixel = match threshold_value {
                            Some(threshold) if *pixel >= threshold => 255,
                            _ => 0,
                        };
                    }
                }
                PreprocessStep::Invert => {
                    for pixel in &mut values {
                        *pixel = 255 - *pixel;
                    }
                }
                other => {
                    return Err(RecognitionError::fatal(format!(
                        "preprocessing step {} cannot follow {}",
                        other.kind(),
                        first.kind()
                    )));
                }
            }
        }
        let foreground = values
            .iter()
            .filter(|value| **value >= FOREGROUND_MIN)
            .count() as u64;
        let pixels = u64::from(width) * u64::from(height);
        let foreground_milli = u32::try_from(foreground * 1000 / pixels)
            .map_err(|_| RecognitionError::fatal("foreground share leaves the u32 range"))?;
        let image = GrayImage::from_raw(width, height, values).ok_or_else(|| {
            RecognitionError::fatal("preprocessed region has the wrong number of pixels")
        })?;
        Ok(PreprocessOutput {
            image,
            evidence: PreprocessEvidence {
                foreground_milli,
                threshold_value,
            },
        })
    }
}

impl Scene {
    /// The chain applied to the RGB pixels of `region` of this frame (CFL-1). The region is
    /// checked as every region of a scene is, and never clipped.
    pub fn preprocess_region(
        &self,
        region: Rect,
        preprocess: &Preprocess,
    ) -> RecognitionResult<PreprocessOutput> {
        let bounds = validate_rect(region, self.width(), self.height())?;
        let pixels = imageops::crop_imm(&self.rgb, bounds.x, bounds.y, bounds.width, bounds.height)
            .to_image();
        preprocess.apply(&pixels)
    }
}

/// HSV on the OpenCV 8-bit scale, in integers: `V = max`, `d = V - min`,
/// `S = (2 * 255 * d + V) div (2 * V)` (0 when `V = 0`), and, when `d > 0`, with `n` and `base`
/// `(G - B, 0)`, `(B - R, 60)` or `(R - G, 120)` as V is R, else G, else B,
/// `H = floor((2 * (30 * n + base * d) + d) / (2 * d)) mod 180` (0 when `d = 0`).
pub fn rgb_to_hsv([red, green, blue]: [u8; 3]) -> [u8; 3] {
    let (red, green, blue) = (i32::from(red), i32::from(green), i32::from(blue));
    let value = red.max(green).max(blue);
    let delta = value - red.min(green).min(blue);
    let saturation = if value == 0 {
        0
    } else {
        (2 * 255 * delta + value) / (2 * value)
    };
    let hue = if delta == 0 {
        0
    } else {
        let (numerator, base) = if value == red {
            (green - blue, 0)
        } else if value == green {
            (blue - red, 60)
        } else {
            (red - green, 120)
        };
        (2 * (30 * numerator + base * delta) + delta)
            .div_euclid(2 * delta)
            .rem_euclid(180)
    };
    [hue as u8, saturation as u8, value as u8]
}

fn keep(region: &RgbImage, keeps: impl Fn([u8; 3]) -> bool) -> Vec<u8> {
    region
        .pixels()
        .map(|pixel| if keeps(pixel.0) { 255 } else { 0 })
        .collect()
}

fn keeps_color(pixel: [u8; 3], colors: &[[u8; 3]], tolerance: u8) -> bool {
    colors.iter().any(|color| {
        pixel
            .iter()
            .zip(color)
            .all(|(component, wanted)| component.abs_diff(*wanted) <= tolerance)
    })
}

fn in_range(pixel: [u8; 3], space: ColorSpace, lower: [u8; 3], upper: [u8; 3]) -> bool {
    let within = |channel: usize, value: u8| (lower[channel]..=upper[channel]).contains(&value);
    match space {
        ColorSpace::Rgb => (0..3).all(|channel| within(channel, pixel[channel])),
        ColorSpace::Hsv => {
            let [hue, saturation, value] = rgb_to_hsv(pixel);
            let hue_kept = if lower[0] <= upper[0] {
                within(0, hue)
            } else {
                hue >= lower[0] || hue <= upper[0]
            };
            hue_kept && within(1, saturation) && within(2, value)
        }
    }
}

/// `0` up to `low`, `255` from `high`, else `(2 * 255 * (v - low) + (high - low)) div
/// (2 * (high - low))`.
fn stretch(value: u8, low: u8, high: u8) -> u8 {
    if value <= low {
        return 0;
    }
    if value >= high {
        return 255;
    }
    let span = u32::from(high - low);
    ((2 * 255 * u32::from(value - low) + span) / (2 * span)) as u8
}

/// Otsu's threshold of `values`, `t + 1` for the smallest `t` in `0..=254` whose split
/// (`v <= t` against `v > t`, both non-empty) has the greatest between-class variance; `None`
/// when no split has both classes (every value the same).
///
/// For a split with `n0`, `s0` (count and sum of the low class) of `N` values summing to `S`, the
/// variance times `N^2` is `a^2 / (n0 * n1)` with `a = |s0 * N - S * n0|`. Two splits are
/// compared exactly in `u128` by quotient and remainder, never in floating point.
fn otsu_threshold(values: &[u8]) -> RecognitionResult<Option<u8>> {
    let overflow = || RecognitionError::fatal("otsu threshold overflows over this region");
    let mut histogram = [0_u128; 256];
    for value in values {
        histogram[usize::from(*value)] += 1;
    }
    let total = values.len() as u128;
    let sum = (0..=255_u8)
        .map(|value| u128::from(value) * histogram[usize::from(value)])
        .sum::<u128>();
    let mut best: Option<(u8, SplitScore)> = None;
    let (mut low_count, mut low_sum) = (0_u128, 0_u128);
    for threshold in 0..=254_u8 {
        low_count += histogram[usize::from(threshold)];
        low_sum += u128::from(threshold) * histogram[usize::from(threshold)];
        let high_count = total - low_count;
        if low_count == 0 || high_count == 0 {
            continue;
        }
        let spread = (low_sum * total).abs_diff(sum * low_count);
        let numerator = spread.checked_mul(spread).ok_or_else(overflow)?;
        let denominator = low_count * high_count;
        let score = SplitScore {
            quotient: numerator / denominator,
            remainder: numerator % denominator,
            denominator,
        };
        let better = match &best {
            None => true,
            Some((_, best)) => score.exceeds(best).ok_or_else(overflow)?,
        };
        if better {
            best = Some((threshold, score));
        }
    }
    Ok(best.map(|(threshold, _)| threshold + 1))
}

/// `quotient + remainder / denominator`, an exact non-negative rational.
struct SplitScore {
    quotient: u128,
    remainder: u128,
    denominator: u128,
}

impl SplitScore {
    /// Whether `self` is strictly greater than `other`; `None` on overflow.
    fn exceeds(&self, other: &Self) -> Option<bool> {
        if self.quotient != other.quotient {
            return Some(self.quotient > other.quotient);
        }
        Some(
            self.remainder.checked_mul(other.denominator)?
                > other.remainder.checked_mul(self.denominator)?,
        )
    }
}

/// Why a chain was refused, with the step and member it points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreprocessError {
    step: Option<usize>,
    member: Option<&'static str>,
    reason: PreprocessErrorReason,
}

/// The refusals of [`Preprocess::new`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreprocessErrorReason {
    /// The chain has no step.
    NoSteps,
    /// The chain has more than [`MAX_STEPS`] steps.
    TooManySteps { count: usize },
    /// The first step is not `keep_colors`, `keep_range` or `gray`.
    FirstStepNotAllowed { kind: PreprocessKind },
    /// A kind appears a second time.
    StepRepeated { kind: PreprocessKind },
    /// A kind the chain's shape does not allow at this place, after the step before it.
    StepOutOfOrder {
        kind: PreprocessKind,
        after: PreprocessKind,
    },
    /// `colors` lists no color or more than [`MAX_COLORS`].
    ColorCount { count: usize },
    /// `colors[index]` repeats an earlier color.
    ColorRepeated { index: usize },
    /// An HSV hue bound above [`HUE_MAX`].
    HueOutOfRange { value: u8 },
    /// `lower` is above `upper` on `channel` (0, 1, 2), where only an HSV hue may wrap.
    BoundsReversed { channel: usize },
    /// `contrast` with `low >= high`.
    ContrastBoundsReversed { low: u8, high: u8 },
    /// A fixed `threshold` of 0 (a fixed threshold is 1 to 255).
    ThresholdZero,
}

impl PreprocessError {
    fn of_chain(reason: PreprocessErrorReason) -> Self {
        Self {
            step: None,
            member: None,
            reason,
        }
    }

    fn at(step: usize, member: &'static str, reason: PreprocessErrorReason) -> Self {
        Self {
            step: Some(step),
            member: Some(member),
            reason,
        }
    }

    /// The index of the refused step; `None` for the step count.
    pub fn step(&self) -> Option<usize> {
        self.step
    }

    /// The refused member of that step (`kind` for the chain's shape); `None` for the step count.
    pub fn member(&self) -> Option<&'static str> {
        self.member
    }

    pub fn reason(&self) -> PreprocessErrorReason {
        self.reason
    }

    /// The JSON pointer of the refusal below the `preprocess` object: `/steps` for the step
    /// count, `/steps/<k>/<member>` otherwise.
    pub fn pointer(&self) -> String {
        match (self.step, self.member) {
            (Some(step), Some(member)) => format!("/steps/{step}/{member}"),
            (Some(step), None) => format!("/steps/{step}"),
            _ => "/steps".to_owned(),
        }
    }
}

impl fmt::Display for PreprocessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "preprocess {}: ", self.pointer())?;
        match self.reason {
            PreprocessErrorReason::NoSteps => formatter.write_str("the chain has no step"),
            PreprocessErrorReason::TooManySteps { count } => {
                write!(formatter, "{count} steps, at most {MAX_STEPS}")
            }
            PreprocessErrorReason::FirstStepNotAllowed { kind } => write!(
                formatter,
                "a chain starts with keep_colors, keep_range or gray, not {kind}"
            ),
            PreprocessErrorReason::StepRepeated { kind } => {
                write!(formatter, "{kind} appears more than once")
            }
            PreprocessErrorReason::StepOutOfOrder { kind, after } => {
                write!(formatter, "{kind} cannot follow {after}")
            }
            PreprocessErrorReason::ColorCount { count } => {
                write!(formatter, "{count} colors, 1 to {MAX_COLORS}")
            }
            PreprocessErrorReason::ColorRepeated { index } => {
                write!(formatter, "color {index} repeats an earlier color")
            }
            PreprocessErrorReason::HueOutOfRange { value } => {
                write!(formatter, "hue {value} above {HUE_MAX}")
            }
            PreprocessErrorReason::BoundsReversed { channel } => {
                write!(formatter, "lower above upper on channel {channel}")
            }
            PreprocessErrorReason::ContrastBoundsReversed { low, high } => {
                write!(formatter, "low {low} is not below high {high}")
            }
            PreprocessErrorReason::ThresholdZero => {
                formatter.write_str("a fixed threshold is 1 to 255, not 0")
            }
        }
    }
}

impl Error for PreprocessError {}

/// Refuses `steps` by its first step that breaks the syntax, in step order: the step count, then
/// each step's place in the shape and its members.
fn validate(steps: &[PreprocessStep]) -> Result<(), PreprocessError> {
    if steps.is_empty() {
        return Err(PreprocessError::of_chain(PreprocessErrorReason::NoSteps));
    }
    if steps.len() > MAX_STEPS {
        return Err(PreprocessError::of_chain(
            PreprocessErrorReason::TooManySteps { count: steps.len() },
        ));
    }
    let first = steps[0].kind();
    if !first.starts_chain() {
        return Err(PreprocessError::at(
            0,
            "kind",
            PreprocessErrorReason::FirstStepNotAllowed { kind: first },
        ));
    }
    let followers = first.followers();
    let mut next = 0;
    for (index, step) in steps.iter().enumerate() {
        let kind = step.kind();
        if index > 0 {
            if steps[..index].iter().any(|earlier| earlier.kind() == kind) {
                return Err(PreprocessError::at(
                    index,
                    "kind",
                    PreprocessErrorReason::StepRepeated { kind },
                ));
            }
            match followers[next..]
                .iter()
                .position(|allowed| *allowed == kind)
            {
                Some(offset) => next += offset + 1,
                None => {
                    return Err(PreprocessError::at(
                        index,
                        "kind",
                        PreprocessErrorReason::StepOutOfOrder {
                            kind,
                            after: steps[index - 1].kind(),
                        },
                    ));
                }
            }
        }
        validate_members(index, step)?;
    }
    Ok(())
}

fn validate_members(index: usize, step: &PreprocessStep) -> Result<(), PreprocessError> {
    match step {
        PreprocessStep::KeepColors { colors, .. } => {
            if colors.is_empty() || colors.len() > MAX_COLORS {
                return Err(PreprocessError::at(
                    index,
                    "colors",
                    PreprocessErrorReason::ColorCount {
                        count: colors.len(),
                    },
                ));
            }
            if let Some(repeated) =
                (1..colors.len()).find(|color| colors[..*color].contains(&colors[*color]))
            {
                return Err(PreprocessError::at(
                    index,
                    "colors",
                    PreprocessErrorReason::ColorRepeated { index: repeated },
                ));
            }
        }
        PreprocessStep::KeepRange {
            space,
            lower,
            upper,
        } => {
            let ordered_from = match space {
                ColorSpace::Rgb => 0,
                ColorSpace::Hsv => {
                    for (member, bound) in [("lower", lower), ("upper", upper)] {
                        if bound[0] > HUE_MAX {
                            return Err(PreprocessError::at(
                                index,
                                member,
                                PreprocessErrorReason::HueOutOfRange { value: bound[0] },
                            ));
                        }
                    }
                    // Only the hue may wrap through 0.
                    1
                }
            };
            if let Some(channel) =
                (ordered_from..3).find(|channel| lower[*channel] > upper[*channel])
            {
                return Err(PreprocessError::at(
                    index,
                    "upper",
                    PreprocessErrorReason::BoundsReversed { channel },
                ));
            }
        }
        PreprocessStep::Contrast { low, high } => {
            if low >= high {
                return Err(PreprocessError::at(
                    index,
                    "high",
                    PreprocessErrorReason::ContrastBoundsReversed {
                        low: *low,
                        high: *high,
                    },
                ));
            }
        }
        PreprocessStep::Threshold {
            value: ThresholdValue::Fixed(0),
        } => {
            return Err(PreprocessError::at(
                index,
                "value",
                PreprocessErrorReason::ThresholdZero,
            ));
        }
        PreprocessStep::Threshold { .. } | PreprocessStep::Gray | PreprocessStep::Invert => {}
    }
    Ok(())
}

#[cfg(test)]
#[path = "preprocess_tests.rs"]
mod tests;
