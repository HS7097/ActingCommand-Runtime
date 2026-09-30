// SPDX-License-Identifier: AGPL-3.0-only

//! `color_digest.v1`: a deterministic integer digest of the color layout of one rectangle.
//!
//! The digest verifies a known region of a frame; it never locates anything. Every step is
//! integer arithmetic, so the same pixels always give the same digest, distance and verdict.
//! The algorithm, its golden values and its error codes are specified in
//! `contracts/color-digest.md`.

use crate::{Rect, Scene, validate_rect};
use std::error::Error;
use std::fmt;

/// The versioned algorithm name declared next to every digest.
pub const COLOR_DIGEST_V1: &str = "color_digest.v1";
/// The largest number of columns, and of rows, in a digest grid.
pub const MAX_GRID_AXIS: u32 = 32;
/// The largest quantized channel value (`255 / 8`, rounded down).
pub const MAX_QUANTIZED_LEVEL: u8 = 31;

const CHANNELS: usize = 3;
const HEX_DIGITS_PER_CELL: usize = 2 * CHANNELS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorDigestErrorCode {
    AlgorithmUnknown,
    GridInvalid,
    RegionInvalid,
    CellsInvalid,
    ExcludeCellsInvalid,
    GridMismatch,
}

impl ColorDigestErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AlgorithmUnknown => "color_digest_algorithm_unknown",
            Self::GridInvalid => "color_digest_grid_invalid",
            Self::RegionInvalid => "color_digest_region_invalid",
            Self::CellsInvalid => "color_digest_cells_invalid",
            Self::ExcludeCellsInvalid => "color_digest_exclude_cells_invalid",
            Self::GridMismatch => "color_digest_grid_mismatch",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColorDigestError {
    code: ColorDigestErrorCode,
    message: String,
}

impl ColorDigestError {
    fn new(code: ColorDigestErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub const fn code(&self) -> ColorDigestErrorCode {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ColorDigestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code.as_str(), self.message)
    }
}

impl Error for ColorDigestError {}

pub type ColorDigestResult<T> = Result<T, ColorDigestError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorDigestAlgorithm {
    V1,
}

impl ColorDigestAlgorithm {
    /// Accepts exactly `color_digest.v1`; any other name is rejected, never defaulted.
    pub fn parse(value: &str) -> ColorDigestResult<Self> {
        match value {
            COLOR_DIGEST_V1 => Ok(Self::V1),
            other => Err(ColorDigestError::new(
                ColorDigestErrorCode::AlgorithmUnknown,
                format!("unknown color digest algorithm {other:?}; expected {COLOR_DIGEST_V1:?}"),
            )),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::V1 => COLOR_DIGEST_V1,
        }
    }
}

/// `columns` x `rows` cells, each axis in `1..=MAX_GRID_AXIS`. The grid must also fit the
/// region it is applied to (at most one cell per pixel on each axis).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorDigestGrid {
    columns: u32,
    rows: u32,
}

impl ColorDigestGrid {
    pub fn new(columns: u32, rows: u32) -> ColorDigestResult<Self> {
        for (axis, value) in [("columns", columns), ("rows", rows)] {
            if !(1..=MAX_GRID_AXIS).contains(&value) {
                return Err(ColorDigestError::new(
                    ColorDigestErrorCode::GridInvalid,
                    format!("{axis} must be in 1..={MAX_GRID_AXIS}, got {value}"),
                ));
            }
        }
        Ok(Self { columns, rows })
    }

    pub const fn columns(self) -> u32 {
        self.columns
    }

    pub const fn rows(self) -> u32 {
        self.rows
    }

    /// `columns * rows`; cell `i` is column `i % columns` of row `i / columns`.
    pub const fn cell_count(self) -> u32 {
        self.columns * self.rows
    }

    /// Rejects a grid with more columns than `width` or more rows than `height` pixels.
    pub fn ensure_fits(self, width: u32, height: u32) -> ColorDigestResult<()> {
        if self.columns > width || self.rows > height {
            return Err(ColorDigestError::new(
                ColorDigestErrorCode::GridInvalid,
                format!(
                    "grid {}x{} exceeds region {width}x{height}; each axis allows at most one cell per pixel",
                    self.columns, self.rows
                ),
            ));
        }
        Ok(())
    }
}

/// The pixels of cell `index` when an axis of `length` pixels from `start` is split into
/// `parts` cells: from `start + floor(index * length / parts)` up to, but excluding,
/// `start + floor((index + 1) * length / parts)`.
pub fn cell_span(start: u32, length: u32, parts: u32, index: u32) -> ColorDigestResult<(u32, u32)> {
    if parts == 0 || parts > MAX_GRID_AXIS || parts > length {
        return Err(ColorDigestError::new(
            ColorDigestErrorCode::GridInvalid,
            format!("{parts} cells do not fit an axis of {length} pixels"),
        ));
    }
    if index >= parts {
        return Err(ColorDigestError::new(
            ColorDigestErrorCode::GridInvalid,
            format!("cell index {index} is outside {parts} cells"),
        ));
    }
    start.checked_add(length).ok_or_else(|| {
        ColorDigestError::new(
            ColorDigestErrorCode::RegionInvalid,
            format!("axis start {start} + length {length} overflows u32"),
        )
    })?;
    let offset = |part: u32| (u64::from(part) * u64::from(length) / u64::from(parts)) as u32;
    // Both offsets are at most `length`, and `start + length` fits in u32.
    Ok((start + offset(index), start + offset(index + 1)))
}

/// A `color_digest.v1` digest: three quantized channels `[R, G, B]` per cell, row-major.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColorDigest {
    grid: ColorDigestGrid,
    cells: Vec<[u8; 3]>,
}

impl ColorDigest {
    /// Digests `region` of `scene`. The region must lie entirely inside the frame and hold
    /// at least one pixel per cell on each axis.
    pub fn compute(scene: &Scene, region: Rect, grid: ColorDigestGrid) -> ColorDigestResult<Self> {
        let bounds = validate_rect(region, scene.width(), scene.height()).map_err(|error| {
            ColorDigestError::new(ColorDigestErrorCode::RegionInvalid, error.message())
        })?;
        grid.ensure_fits(bounds.width, bounds.height)?;
        let pixels = scene.rgb.as_raw();
        let stride = scene.width() as usize * CHANNELS;
        let mut cells = Vec::with_capacity(grid.cell_count() as usize);
        for row in 0..grid.rows {
            let (y0, y1) = cell_span(bounds.y, bounds.height, grid.rows, row)?;
            for column in 0..grid.columns {
                let (x0, x1) = cell_span(bounds.x, bounds.width, grid.columns, column)?;
                let mut sum = [0_u64; 3];
                for y in y0..y1 {
                    let line = y as usize * stride;
                    let (span, _) = pixels
                        [line + x0 as usize * CHANNELS..line + x1 as usize * CHANNELS]
                        .as_chunks::<CHANNELS>();
                    for &[red, green, blue] in span {
                        sum[0] += u64::from(red);
                        sum[1] += u64::from(green);
                        sum[2] += u64::from(blue);
                    }
                }
                let divisor = 8 * u64::from(x1 - x0) * u64::from(y1 - y0);
                // `sum <= 255 * n`, so every quotient is at most 31 and fits in u8.
                cells.push(sum.map(|channel| (channel / divisor) as u8));
            }
        }
        Ok(Self { grid, cells })
    }

    /// Parses the lowercase hex encoding: exactly `6 * columns * rows` digits `0-9a-f`,
    /// every byte at most `0x1f`.
    pub fn from_hex(grid: ColorDigestGrid, hex: &str) -> ColorDigestResult<Self> {
        let cell_count = grid.cell_count() as usize;
        let expected_len = HEX_DIGITS_PER_CELL * cell_count;
        if hex.len() != expected_len {
            return Err(ColorDigestError::new(
                ColorDigestErrorCode::CellsInvalid,
                format!(
                    "cells has {} hex digits; grid {}x{} requires {expected_len}",
                    hex.len(),
                    grid.columns,
                    grid.rows
                ),
            ));
        }
        // The length check above leaves no partial cell.
        let (digits, _) = hex.as_bytes().as_chunks::<HEX_DIGITS_PER_CELL>();
        let mut cells = Vec::with_capacity(cell_count);
        for cell in digits {
            let mut channels = [0_u8; 3];
            let (pairs, _) = cell.as_chunks::<2>();
            for (channel, &[high, low]) in channels.iter_mut().zip(pairs) {
                let byte = (hex_nibble(high)? << 4) | hex_nibble(low)?;
                if byte > MAX_QUANTIZED_LEVEL {
                    return Err(ColorDigestError::new(
                        ColorDigestErrorCode::CellsInvalid,
                        format!(
                            "cells byte {byte:#04x} of cell {} exceeds {MAX_QUANTIZED_LEVEL:#04x}",
                            cells.len()
                        ),
                    ));
                }
                *channel = byte;
            }
            cells.push(channels);
        }
        Ok(Self { grid, cells })
    }

    /// The lowercase hex encoding, `6 * columns * rows` digits.
    pub fn to_hex(&self) -> String {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut hex = String::with_capacity(self.cells.len() * HEX_DIGITS_PER_CELL);
        for byte in self.cells.iter().flatten() {
            hex.push(char::from(DIGITS[usize::from(byte >> 4)]));
            hex.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
        }
        hex
    }

    pub const fn grid(&self) -> ColorDigestGrid {
        self.grid
    }

    pub fn cells(&self) -> &[[u8; 3]] {
        &self.cells
    }
}

fn hex_nibble(digit: u8) -> ColorDigestResult<u8> {
    match digit {
        b'0'..=b'9' => Ok(digit - b'0'),
        b'a'..=b'f' => Ok(digit - b'a' + 10),
        other => Err(ColorDigestError::new(
            ColorDigestErrorCode::CellsInvalid,
            format!("cells must be lowercase hex; found {:?}", char::from(other)),
        )),
    }
}

/// Checks an exclusion list against `grid`: strictly ascending, every index below
/// `cell_count`, and at least one cell left active. Returns the number of active cells.
pub fn validate_exclude_cells(
    grid: ColorDigestGrid,
    exclude_cells: &[u32],
) -> ColorDigestResult<u32> {
    let cell_count = grid.cell_count();
    let mut previous: Option<u32> = None;
    for &index in exclude_cells {
        if index >= cell_count {
            return Err(ColorDigestError::new(
                ColorDigestErrorCode::ExcludeCellsInvalid,
                format!("exclude_cells index {index} is outside {cell_count} cells"),
            ));
        }
        if let Some(previous) = previous
            && index <= previous
        {
            return Err(ColorDigestError::new(
                ColorDigestErrorCode::ExcludeCellsInvalid,
                format!("exclude_cells must be strictly ascending; {index} follows {previous}"),
            ));
        }
        previous = Some(index);
    }
    let excluded = exclude_cells.len() as u32;
    if excluded >= cell_count {
        return Err(ColorDigestError::new(
            ColorDigestErrorCode::ExcludeCellsInvalid,
            format!("exclude_cells leaves no active cell of {cell_count}"),
        ));
    }
    Ok(cell_count - excluded)
}

/// The acceptance limits of one digest. There is no default: `max_mean_milli` is always
/// declared, and `max_cell` applies only when declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorDigestThresholds {
    pub max_mean_milli: u32,
    pub max_cell: Option<u32>,
}

/// The L1 distance of two digests over the active cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorDigestDistance {
    /// Cells compared: all cells minus `exclude_cells`.
    pub active_cells: u32,
    /// `floor(1000 * sum(d_i) / active_cells)` over the active cells.
    pub mean_milli: u32,
    /// The largest active `d_i`.
    pub max_cell: u32,
    /// The smallest active cell index whose `d_i` equals `max_cell`.
    pub worst_cell: u32,
}

impl ColorDigestDistance {
    /// `mean_milli <= max_mean_milli`, and `max_cell <= thresholds.max_cell` when declared.
    pub fn passes(&self, thresholds: ColorDigestThresholds) -> bool {
        self.mean_milli <= thresholds.max_mean_milli
            && thresholds
                .max_cell
                .is_none_or(|max_cell| self.max_cell <= max_cell)
    }
}

/// Compares `observed` with `expected` cell by cell: `d_i = |dR| + |dG| + |dB|`, skipping
/// the cells listed in `exclude_cells`. Both digests must use the same grid.
pub fn distance(
    expected: &ColorDigest,
    observed: &ColorDigest,
    exclude_cells: &[u32],
) -> ColorDigestResult<ColorDigestDistance> {
    if expected.grid != observed.grid {
        return Err(ColorDigestError::new(
            ColorDigestErrorCode::GridMismatch,
            format!(
                "expected grid {}x{} differs from observed grid {}x{}",
                expected.grid.columns,
                expected.grid.rows,
                observed.grid.columns,
                observed.grid.rows
            ),
        ));
    }
    let active_cells = validate_exclude_cells(expected.grid, exclude_cells)?;
    let mut excluded = exclude_cells.iter().copied().peekable();
    let mut total = 0_u64;
    let mut worst: Option<(u32, u32)> = None;
    for (index, (want, seen)) in (0_u32..).zip(expected.cells.iter().zip(&observed.cells)) {
        if excluded.next_if_eq(&index).is_some() {
            continue;
        }
        let cell: u32 = want
            .iter()
            .zip(seen)
            .map(|(want, seen)| u32::from(want.abs_diff(*seen)))
            .sum();
        total += u64::from(cell);
        if worst.is_none_or(|(_, max_cell)| cell > max_cell) {
            worst = Some((index, cell));
        }
    }
    let (worst_cell, max_cell) = worst.ok_or_else(|| {
        ColorDigestError::new(
            ColorDigestErrorCode::ExcludeCellsInvalid,
            "exclude_cells leaves no active cell",
        )
    })?;
    // `total <= 93 * active_cells`, so the mean is at most 93000.
    let mean_milli = (1000 * total / u64::from(active_cells)) as u32;
    Ok(ColorDigestDistance {
        active_cells,
        mean_milli,
        max_cell,
        worst_cell,
    })
}
