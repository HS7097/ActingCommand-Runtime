// SPDX-License-Identifier: AGPL-3.0-only

//! The resampling primitive (Workflow #396 REC-5, REC-M v1.2.1 §7.1). REC-5 scales a template by
//! a declared factor with it; Workflow #392 converts a captured frame to the package's base
//! coordinate system with the same primitive, so that both round alike.
//!
//! An 8-bit image (gray, RGB or RGBA, each channel on its own) is resampled to a given size,
//! each axis on its own: an axis that shrinks averages areas (an output pixel is the mean of the
//! source pixels it covers, weighted by the covered length), an axis that grows or keeps its
//! length interpolates linearly between pixel centres. Every weight is a 16.16 fixed-point
//! integer, and an output value is rounded half up to `u8`, so the same input and size give the
//! same bytes on every host. The exact arithmetic is spelled out on `axis_weights`.

use super::{RecognitionError, RecognitionResult};
use image::{ImageBuffer, Pixel};

/// One in 16.16 fixed point: the weights of an output pixel along one axis sum to it.
const ONE: u64 = 1 << 16;

/// The factor that keeps a length (REC-5 `factors_milli`).
pub const SCALE_IDENTITY_MILLI: u32 = 1000;

/// `image` resampled to `width` by `height` pixels.
pub fn resample<P>(
    image: &ImageBuffer<P, Vec<u8>>,
    width: u32,
    height: u32,
) -> RecognitionResult<ImageBuffer<P, Vec<u8>>>
where
    P: Pixel<Subpixel = u8>,
{
    let (source_width, source_height) = image.dimensions();
    let out = resample_pixels(
        image.as_raw(),
        (source_width, source_height),
        usize::from(P::CHANNEL_COUNT),
        (width, height),
    )?;
    ImageBuffer::from_raw(width, height, out)
        .ok_or_else(|| RecognitionError::fatal("resampled image buffer has the wrong length"))
}

/// Interleaved 8-bit pixels of `channels` values each, row-major without padding, of an image
/// of `size` (width, height), resampled to `target` (width, height): the same arithmetic as
/// [`resample`], for frames held as raw bytes.
pub fn resample_pixels(
    pixels: &[u8],
    size: (u32, u32),
    channels: usize,
    target: (u32, u32),
) -> RecognitionResult<Vec<u8>> {
    let ((source_width, source_height), (width, height)) = (size, target);
    if source_width == 0 || source_height == 0 || width == 0 || height == 0 || channels == 0 {
        return Err(RecognitionError::fatal(format!(
            "resampling needs non-zero sizes: {source_width}x{source_height} to {width}x{height}, {channels} channels"
        )));
    }
    let expected = (source_width as usize)
        .checked_mul(source_height as usize)
        .and_then(|count| count.checked_mul(channels));
    if expected != Some(pixels.len()) {
        return Err(RecognitionError::fatal(format!(
            "resampling {source_width}x{source_height} pixels of {channels} channels got {} bytes",
            pixels.len()
        )));
    }
    let columns = axis_weights(source_width, width);
    let rows = axis_weights(source_height, height);
    let source = pixels;
    let stride = source_width as usize * channels;
    let mut out = Vec::with_capacity(width as usize * height as usize * channels);
    for row in &rows {
        for column in &columns {
            for channel in 0..channels {
                let mut sum = 0_u64;
                for (y, row_weight) in row {
                    let line = *y as usize * stride;
                    for (x, column_weight) in column {
                        let value = u64::from(source[line + *x as usize * channels + channel]);
                        sum += row_weight * column_weight * value;
                    }
                }
                // The weights of each axis sum to `ONE`, so `sum` is at most 255 * 2^32.
                out.push(((sum + (1 << 31)) >> 32) as u8);
            }
        }
    }
    Ok(out)
}

/// The source pixels and 16.16 weights of each output pixel along an axis of `source` pixels
/// resampled to `target` pixels (both non-zero). The weights of every output pixel sum to
/// exactly 65536.
///
/// - **Shrinking** (`target < source`): output pixel `i` covers the source interval
///   `[i * source / target, (i + 1) * source / target)`. In units of `1 / target` source pixel it
///   is `[i * source, (i + 1) * source)`, and source pixel `j` is `[j * target, (j + 1) * target)`;
///   their overlap `c_j` is an integer and the overlaps sum to `source`. With the running sums
///   `C_j = c_0 + ... + c_j` over the covered pixels, the weight of pixel `j` is
///   `R(C_j) - R(C_(j-1))`, where `R(C) = (2 * C * 65536 + source) div (2 * source)` is
///   `C * 65536 / source` rounded half up and `R` of the empty sum is 0.
/// - **Growing or keeping** (`target >= source`): the centre of output pixel `i` lies at source
///   position `p = ((2i + 1) * source - target) / (2 * target)`. In 16.16 it is
///   `P = (2 * N + 2 * target) div (4 * target)` with `N = ((2i + 1) * source - target) * 65536`
///   (`p` rounded half up), 0 when `N <= 0`, and at most `(source - 1) * 65536`. With
///   `j = P div 65536` and `f = P mod 65536`, pixel `j` weighs `65536 - f` and pixel `j + 1`
///   weighs `f`; `f = 0` gives pixel `j` alone.
///
/// An output value is `(sum(w_y * w_x * v) + 2^31) div 2^32` over the pixels of its row and
/// column lists: the weighted mean rounded half up.
fn axis_weights(source: u32, target: u32) -> Vec<Vec<(u32, u64)>> {
    let (source, target) = (u64::from(source), u64::from(target));
    let rounded = |running: u64| (2 * running * ONE + source) / (2 * source);
    (0..target)
        .map(|i| {
            if target < source {
                let start = i * source;
                let end = (i + 1) * source;
                let mut running = 0_u64;
                let mut weights = Vec::new();
                for j in start / target..end.div_ceil(target) {
                    let overlap = end.min((j + 1) * target) - start.max(j * target);
                    let before = rounded(running);
                    running += overlap;
                    weights.push((j as u32, rounded(running) - before));
                }
                weights
            } else {
                let numerator = (2 * i128::from(i) + 1) * i128::from(source) - i128::from(target);
                let position = if numerator <= 0 {
                    0
                } else {
                    let numerator = numerator * i128::from(ONE);
                    let target = i128::from(target);
                    let half_up = (2 * numerator + 2 * target) / (4 * target);
                    u64::try_from(half_up)
                        .unwrap_or(u64::MAX)
                        .min((source - 1) * ONE)
                };
                let (pixel, fraction) = (position / ONE, position % ONE);
                if fraction == 0 {
                    vec![(pixel as u32, ONE)]
                } else {
                    vec![(pixel as u32, ONE - fraction), (pixel as u32 + 1, fraction)]
                }
            }
        })
        .collect()
}

/// A length scaled by `factor_milli`, rounded half up: `(length * factor_milli + 500) div 1000`.
pub fn scaled_length(length: u32, factor_milli: u32) -> u32 {
    let scaled = (u64::from(length) * u64::from(factor_milli) + 500) / 1000;
    u32::try_from(scaled).unwrap_or(u32::MAX)
}

/// A signed package length (an offset, a width or a height) scaled by `factor_milli`:
/// `(|v| * factor_milli + 500) div 1000` with the sign of `v`, so that a negative value scales
/// as its absolute value does. `None` when the result leaves the `i32` range.
pub fn scale_signed(value: i32, factor_milli: u32) -> Option<i32> {
    let magnitude = (i64::from(value).unsigned_abs() * u64::from(factor_milli) + 500) / 1000;
    let magnitude = i64::try_from(magnitude).ok()?;
    i32::try_from(if value < 0 { -magnitude } else { magnitude }).ok()
}

#[cfg(test)]
#[path = "resample_tests.rs"]
mod tests;
