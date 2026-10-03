// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #336 L6 (§12.5, `contracts/policy-suspension.md`): whether the error frames of two
//! failed runs show the same screen. A pure function over PNG bytes: the Runtime reads the
//! frames, this module only decodes and compares them.
//!
//! Both frames get one full-frame `color_digest.v1` digest on a grid of `min(32, width)`
//! columns and `min(18, height)` rows. A cell whose `|dR| + |dG| + |dB|` exceeds
//! [`FAILURE_FRAME_CELL_DELTA`] has changed; more than [`FAILURE_FRAME_MAX_CHANGED_MILLI`]
//! changed cells per thousand make the frames different. Frames of different sizes are
//! different. The digest mean and the template correlation are reported for display only.

use actingcommand_recognition::color_digest::{self, ColorDigest, ColorDigestGrid};
use actingcommand_recognition::{MatchMetric, Rect, Scene};

/// A cell whose quantized `|dR| + |dG| + |dB|` exceeds this value has changed.
pub const FAILURE_FRAME_CELL_DELTA: u32 = 6;
/// More changed cells than this, per thousand cells, make two frames different.
pub const FAILURE_FRAME_MAX_CHANGED_MILLI: u32 = 250;
const MAX_GRID_COLUMNS: u32 = 32;
const MAX_GRID_ROWS: u32 = 18;

/// The verdict of [`compare_failure_frames`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureFrameVerdict {
    Similar,
    Different,
}

impl FailureFrameVerdict {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Similar => "similar",
            Self::Different => "different",
        }
    }
}

/// Two frames compared: the verdict, why they differ, and the digest figures when both frames
/// have the same size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FailureFrameComparison {
    pub verdict: FailureFrameVerdict,
    /// `size_changed` or `digest_cells_changed` for different frames; `None` when similar.
    pub reason: Option<&'static str>,
    /// Changed cells per thousand cells; `None` when the sizes differ.
    pub changed_cells_milli: Option<u32>,
    /// The `color_digest.v1` mean distance of the two digests, for display only.
    pub digest_mean_milli: Option<u32>,
}

/// Frames that could not be compared; `reason()` is `png_invalid`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailureFrameCompareError {
    reason: &'static str,
    message: String,
}

impl FailureFrameCompareError {
    fn png_invalid(message: impl Into<String>) -> Self {
        Self {
            reason: "png_invalid",
            message: message.into(),
        }
    }

    pub const fn reason(&self) -> &'static str {
        self.reason
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for FailureFrameCompareError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.reason, self.message)
    }
}

impl std::error::Error for FailureFrameCompareError {}

/// Compares the error frame of an earlier failed run with that of the current one (§12.5).
pub fn compare_failure_frames(
    previous_png: &[u8],
    current_png: &[u8],
) -> Result<FailureFrameComparison, FailureFrameCompareError> {
    let previous = decode(previous_png, "previous")?;
    let current = decode(current_png, "current")?;
    if (previous.width(), previous.height()) != (current.width(), current.height()) {
        return Ok(FailureFrameComparison {
            verdict: FailureFrameVerdict::Different,
            reason: Some("size_changed"),
            changed_cells_milli: None,
            digest_mean_milli: None,
        });
    }
    let previous = frame_digest(&previous)?;
    let current = frame_digest(&current)?;
    let cells = previous.cells().len();
    let changed = previous
        .cells()
        .iter()
        .zip(current.cells())
        .filter(|(left, right)| {
            left.iter()
                .zip(right.iter())
                .map(|(left, right)| u32::from(left.abs_diff(*right)))
                .sum::<u32>()
                > FAILURE_FRAME_CELL_DELTA
        })
        .count();
    let changed_cells_milli = u32::try_from(changed.saturating_mul(1000) / cells.max(1))
        .map_err(|_| FailureFrameCompareError::png_invalid("changed cell ratio overflowed"))?;
    let mean = color_digest::distance(&previous, &current, &[])
        .map_err(|error| FailureFrameCompareError::png_invalid(error.message().to_owned()))?;
    let different = changed_cells_milli > FAILURE_FRAME_MAX_CHANGED_MILLI;
    Ok(FailureFrameComparison {
        verdict: if different {
            FailureFrameVerdict::Different
        } else {
            FailureFrameVerdict::Similar
        },
        reason: different.then_some("digest_cells_changed"),
        changed_cells_milli: Some(changed_cells_milli),
        digest_mean_milli: Some(mean.mean_milli),
    })
}

/// The normalized correlation coefficient of two frames of the same size, for display only
/// (`actingd suspended`); it never decides whether two failures are the same.
pub fn failure_frame_ccoeff(
    previous_png: &[u8],
    current_png: &[u8],
) -> Result<f64, FailureFrameCompareError> {
    let current = decode(current_png, "current")?;
    current
        .match_template_with_metric(
            previous_png,
            None,
            MatchMetric::CorrelationCoefficientNormalized,
        )
        .map(|matched| f64::from(matched.raw_score))
        .map_err(|error| FailureFrameCompareError::png_invalid(error.to_string()))
}

fn decode(png: &[u8], which: &str) -> Result<Scene, FailureFrameCompareError> {
    Scene::from_png(png)
        .map_err(|error| FailureFrameCompareError::png_invalid(format!("{which} frame: {error}")))
}

fn frame_digest(scene: &Scene) -> Result<ColorDigest, FailureFrameCompareError> {
    let (width, height) = (scene.width(), scene.height());
    let region = Rect {
        x: 0,
        y: 0,
        width: i32::try_from(width)
            .map_err(|_| FailureFrameCompareError::png_invalid("frame width overflowed"))?,
        height: i32::try_from(height)
            .map_err(|_| FailureFrameCompareError::png_invalid("frame height overflowed"))?,
    };
    let grid = ColorDigestGrid::new(width.min(MAX_GRID_COLUMNS), height.min(MAX_GRID_ROWS))
        .map_err(|error| FailureFrameCompareError::png_invalid(error.message().to_owned()))?;
    ColorDigest::compute(scene, region, grid)
        .map_err(|error| FailureFrameCompareError::png_invalid(error.message().to_owned()))
}
