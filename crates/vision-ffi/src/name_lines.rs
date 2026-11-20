// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #463 SEL-GRID G2 (`contracts/selection-graph.md`, "Shared OCR read", "The lines of a
//! name box"; `contracts/vision-model-folders.md`, the `detector.max_boxes` row): the lines a
//! layout's shared read declares its name boxes to hold, and how the text provider reads a served
//! block by them, between the cap's count and the recognizer. A block lower than half a line is
//! not read; a block at least 1.25 lines high and of no more lines than declared is cut into its
//! lines, each recognized on its own; every other block is recognized whole, as before.
//!
//! The two factors are constants measured on 11-19 on the list that started the work (a shop of
//! four columns whose card names hold one line or two): a name of two lines detected as one block
//! is 42 to 43 px high where one of its lines is 21 to 23 px, the lines 3 to 4 px apart. 1.25 lines
//! lies above any one line and below the block of two; half a line lies below any line of a name.

use crate::{
    VisionFfiError, VisionFfiErrorCode, VisionFfiResult, VisionFrameView, VisionPixelFormat,
    VisionRect,
};

/// The most lines a name box may declare (the contract crate's bound, written once).
pub const OCR_NAME_LINES_MAX: u8 = actingcommand_contract::candidate_projection::NAME_BOX_LINES_MAX;
/// The least line height a name box may declare, in pixels of the picture the provider is given
/// (the contract crate's bound, written once).
pub const OCR_NAME_LINE_HEIGHT_MIN: u32 =
    actingcommand_contract::candidate_projection::NAME_BOX_LINE_HEIGHT_MIN;
/// A served block at least this many thousandths of the line height high is cut into lines.
pub const OCR_NAME_LINE_CUT_MILLI: u64 = 1_250;
/// A served block lower than this many thousandths of the line height is not read.
pub const OCR_NAME_LINE_FLOOR_MILLI: u64 = 500;
/// The lowest strip a cut makes: a cut that would make a lower one is not made, and the block is
/// read whole.
pub const OCR_NAME_LINE_STRIP_MIN: u32 = 8;

/// The lines a shared read's name boxes hold: at most `max` lines of `line_height` pixels of the
/// picture the provider is given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OcrNameLines {
    pub max: u8,
    pub line_height: u32,
}

impl OcrNameLines {
    /// `max` in 1..=[`OCR_NAME_LINES_MAX`] and `line_height` at least
    /// [`OCR_NAME_LINE_HEIGHT_MIN`], else `InvalidRequest`.
    pub fn validate(self) -> VisionFfiResult<()> {
        if !(1..=OCR_NAME_LINES_MAX).contains(&self.max)
            || self.line_height < OCR_NAME_LINE_HEIGHT_MIN
        {
            return Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::InvalidRequest,
                "ocr",
                format!(
                    "OCR request lines must declare max in 1..={OCR_NAME_LINES_MAX} and a \
                     line_height of at least {OCR_NAME_LINE_HEIGHT_MIN}"
                ),
            ));
        }
        Ok(())
    }
}

/// How one served block is read under [`OcrNameLines`] ([`ocr_name_line_plan`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OcrNameLinePlan {
    /// Lower than half a line: not read, and reported apart.
    BelowFloor,
    /// Recognized whole, as without lines.
    Whole,
    /// Cut into these strips, top to bottom, the block's full width each, each recognized on its
    /// own.
    Cut(Vec<VisionRect>),
}

impl OcrNameLinePlan {
    /// The recognitions the plan takes, which count against the folder's `detector.max_boxes`: a
    /// block read whole one, a cut block its strips, a block below the floor none.
    pub fn recognitions(&self) -> u32 {
        match self {
            Self::BelowFloor => 0,
            Self::Whole => 1,
            Self::Cut(strips) => u32::try_from(strips.len()).unwrap_or(u32::MAX),
        }
    }
}

/// How the served block `rect` of `frame`, the picture the provider is given, is read under
/// `lines`. Lower than [`OCR_NAME_LINE_FLOOR_MILLI`] thousandths of the line height: not read.
/// At least [`OCR_NAME_LINE_CUT_MILLI`] thousandths, with `max` of at least 2: cut into
/// `max(round(height / line_height), 2)` strips, unless `round(height / line_height)` exceeds
/// `max`: a block of more lines than the name box declares is not cut and is read whole, exactly
/// as without lines, so that no strip holds a line and a half (its height, which the Lab's refusal
/// lists, tells the author to raise `max`). The cut between strip `k` and `k + 1` of `n` is not
/// made blindly at the equal position `round(k * height / n)`: within a quarter of the line height
/// around it, it goes to the row with the least ink ([`row_ink`]), the equal position when no row
/// has less, and of rows with equally least ink the one closest to it (the upper one at the same
/// distance); the row of a cut is the first row of the lower strip. A cut that would make a strip
/// lower than [`OCR_NAME_LINE_STRIP_MIN`] pixels is not made: the block is read whole. Every other
/// block is read whole, as without lines.
pub fn ocr_name_line_plan(
    frame: &VisionFrameView<'_>,
    rect: VisionRect,
    lines: OcrNameLines,
) -> OcrNameLinePlan {
    let (Ok(height), Ok(width)) = (u32::try_from(rect.height), u32::try_from(rect.width)) else {
        return OcrNameLinePlan::Whole;
    };
    let line = u64::from(lines.line_height);
    let tall = u64::from(height) * 1_000;
    if tall < line * OCR_NAME_LINE_FLOOR_MILLI {
        return OcrNameLinePlan::BelowFloor;
    }
    if line == 0 || tall < line * OCR_NAME_LINE_CUT_MILLI || lines.max < 2 || width == 0 {
        return OcrNameLinePlan::Whole;
    }
    // round(height / line_height): a block of more lines than `max` is read whole, as without
    // lines; otherwise at least 2 strips (1.25 lines round to 1).
    let lines_held = (2 * u64::from(height) + line) / (2 * line);
    if lines_held > u64::from(lines.max) {
        return OcrNameLinePlan::Whole;
    }
    let Some(ink) = row_ink(frame, rect) else {
        return OcrNameLinePlan::Whole;
    };
    let count = lines_held.max(2);
    let reach = u64::from(lines.line_height / 4);
    let mut bounds = vec![0_u64];
    for k in 1..count {
        let equal = (2 * k * u64::from(height) + count) / (2 * count);
        bounds.push(least_ink_row(&ink, equal, reach));
    }
    bounds.push(u64::from(height));
    let mut strips = Vec::with_capacity(bounds.len() - 1);
    for pair in bounds.windows(2) {
        let (top, bottom) = (pair[0], pair[1]);
        if bottom < top + u64::from(OCR_NAME_LINE_STRIP_MIN) {
            return OcrNameLinePlan::Whole;
        }
        let (Ok(top), Ok(strip_height)) = (i32::try_from(top), i32::try_from(bottom - top)) else {
            return OcrNameLinePlan::Whole;
        };
        let Some(y) = rect.y.checked_add(top) else {
            return OcrNameLinePlan::Whole;
        };
        strips.push(VisionRect {
            x: rect.x,
            y,
            width: rect.width,
            height: strip_height,
        });
    }
    OcrNameLinePlan::Cut(strips)
}

/// Of the rows of a block within `reach` of the row `equal`, its first row and the rows past its
/// last left aside (`ink` holds one value per row of the block), the one with the least ink:
/// `equal` itself when no row has less, and of rows with equally least ink the one closest to
/// `equal`, the upper one at the same distance.
fn least_ink_row(ink: &[u64], equal: u64, reach: u64) -> u64 {
    let rows = u64::try_from(ink.len()).unwrap_or(u64::MAX);
    let ink_at = |row: u64| {
        usize::try_from(row)
            .ok()
            .and_then(|row| ink.get(row))
            .copied()
    };
    let mut best = equal;
    let mut least = ink_at(equal).unwrap_or(u64::MAX);
    for distance in 1..=reach {
        for row in [equal.checked_sub(distance), equal.checked_add(distance)]
            .into_iter()
            .flatten()
        {
            if row == 0 || row >= rows {
                continue;
            }
            if let Some(value) = ink_at(row)
                && value < least
            {
                best = row;
                least = value;
            }
        }
    }
    best
}

/// The ink of each row of `rect` in `frame`, top to bottom, on the pixels the recognizer is given
/// (the frame inside `rect`): the sum over the row of the absolute differences of luma between
/// horizontally neighbouring pixels, luma `(299 R + 587 G + 114 B) / 1000` and a gray pixel's own
/// value. It does not depend on the colour of the text: a row of background holds none, whatever
/// its colour or a gradient from top to bottom, and a row through letters holds their edges, dark
/// letters on light and light letters on dark alike. `None` when `rect` leaves the frame.
fn row_ink(frame: &VisionFrameView<'_>, rect: VisionRect) -> Option<Vec<u64>> {
    let x = usize::try_from(rect.x).ok()?;
    let y = usize::try_from(rect.y).ok()?;
    let width = usize::try_from(rect.width).ok()?;
    let height = usize::try_from(rect.height).ok()?;
    let frame_width = usize::try_from(frame.width).ok()?;
    let frame_height = usize::try_from(frame.height).ok()?;
    if x.checked_add(width)? > frame_width || y.checked_add(height)? > frame_height {
        return None;
    }
    let channels = frame.pixel_format.bytes_per_pixel();
    (y..y + height)
        .map(|row| {
            let start = row
                .checked_mul(frame_width)?
                .checked_add(x)?
                .checked_mul(channels)?;
            let end = start.checked_add(width.checked_mul(channels)?)?;
            let lumas = frame
                .pixels
                .get(start..end)?
                .chunks_exact(channels)
                .map(|pixel| luma(frame.pixel_format, pixel))
                .collect::<Vec<u32>>();
            Some(
                lumas
                    .windows(2)
                    .map(|pair| u64::from(pair[0].abs_diff(pair[1])))
                    .sum::<u64>(),
            )
        })
        .collect()
}

/// The luma of one pixel of `format`.
fn luma(format: VisionPixelFormat, pixel: &[u8]) -> u32 {
    match (format, pixel) {
        (VisionPixelFormat::Gray8, [value, ..]) => u32::from(*value),
        (_, [red, green, blue, ..]) => {
            (299 * u32::from(*red) + 587 * u32::from(*green) + 114 * u32::from(*blue)) / 1_000
        }
        _ => 0,
    }
}
