// SPDX-License-Identifier: AGPL-3.0-only

//! #463 S2-2 (`contracts/lab-recording.md`, "Reviewing a recording step by step: `record
//! review`"): a preview is a copy of a stored frame with a press and boxes drawn on it. The owner's
//! ruling (board #463): a preview's boxes are rendered onto a copy, and the stored frames carry no
//! box. The copy is drawn by plain pixel writes on the decoded frame's own buffer (a line by
//! Bresenham's steps, a rectangle's outline by four lines, a ring by the midpoint circle's steps)
//! and encoded with the device crate's PNG encoder, which the Lab already uses for its crops: no
//! drawing crate is added.
//!
//! What is drawn, in this order, the later over the earlier: each box with its colour, then the
//! press: a cross and a ring at a tap's point, an arrow from a swipe's start to its end, a ring at
//! a long press's point. The ring's radius is a fortieth of the frame's shorter side, at least six
//! pixels ([`press_radius`]).

use super::model::{RecordPoint, RecordRect};
use super::store::invalid;
use actingcommand_contract::LabResult;
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use std::f64::consts::FRAC_PI_6;

/// A colour, red, green and blue.
pub(crate) type Colour = [u8; 3];

/// The press itself: the cross, the ring and the arrow.
pub(crate) const PRESS_COLOUR: Colour = [255, 0, 255];
/// A step's press box: the rectangle its click presses in, when it is no recorded point.
pub(crate) const PRESS_BOX_COLOUR: Colour = [255, 128, 0];
/// A step's recognize box: the region of a template mark a box drew (`box/<nn>/...`).
pub(crate) const RECOGNIZE_BOX_COLOUR: Colour = [0, 255, 0];
/// The region of every other mark of a step.
pub(crate) const MARK_COLOUR: Colour = [0, 160, 255];

/// The smallest radius of a press's ring, in pixels.
const MIN_RADIUS: i32 = 6;
/// The frame's shorter side over the radius of a press's ring.
const RADIUS_PARTS: u32 = 40;
/// How many pixels thick a ring, a box's outline and a press's line are.
const THICKNESS: i64 = 2;

/// What is drawn of a press, at the points the step table holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Glyph {
    /// A tap: a cross and a ring at its point.
    Tap(RecordPoint),
    /// A swipe: an arrow from its start to its end.
    Swipe { from: RecordPoint, to: RecordPoint },
    /// A long press: a ring at its point.
    LongPress(RecordPoint),
}

/// What one preview draws on its frame: the boxes, in order, each with its colour, then the press.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Overlay {
    pub(crate) boxes: Vec<(RecordRect, Colour)>,
    pub(crate) glyph: Option<Glyph>,
}

/// The radius of a press's ring on a frame of `width` by `height`: a fortieth of the shorter side,
/// at least [`MIN_RADIUS`].
pub(crate) fn press_radius(width: u32, height: u32) -> i32 {
    i32::try_from(width.min(height) / RADIUS_PARTS)
        .unwrap_or(i32::MAX)
        .max(MIN_RADIUS)
}

/// The PNG of the stored frame `png` (its bytes, already checked against its sha256) with
/// `overlay` drawn on a copy of its pixels; `label` names the frame in a failure. A frame that does
/// not decode, and a copy that does not encode, are `record_frame_unreadable`, as a crop's are.
pub(crate) fn render(png: Vec<u8>, overlay: &Overlay, label: &str) -> LabResult<Vec<u8>> {
    let frame = Frame::from_png(png, CaptureBackendName::AdbScreencap).map_err(|error| {
        invalid(
            "record_frame_unreadable",
            format!("failed to decode {label} as PNG: {error}"),
        )
    })?;
    let (width, height, format) = (frame.width, frame.height, frame.pixel_format);
    let channels = match format {
        PixelFormat::Rgb8 => 3,
        PixelFormat::Rgba8 => 4,
    };
    let mut canvas = Canvas {
        width: usize::try_from(width).unwrap_or(0),
        height: usize::try_from(height).unwrap_or(0),
        channels,
        pixels: frame.pixels,
    };
    for (rect, colour) in &overlay.boxes {
        canvas.outline(*rect, *colour);
    }
    let radius = press_radius(width, height);
    match overlay.glyph {
        Some(Glyph::Tap(point)) if canvas.holds(point) => {
            let arm = i64::from(radius) + i64::from(radius) / 2;
            canvas.cross(point, arm, PRESS_COLOUR);
            canvas.ring(point, i64::from(radius), PRESS_COLOUR);
        }
        Some(Glyph::Swipe { from, to }) if canvas.holds(from) && canvas.holds(to) => {
            canvas.arrow(from, to, radius, PRESS_COLOUR);
        }
        Some(Glyph::LongPress(point)) if canvas.holds(point) => {
            canvas.ring(point, i64::from(radius), PRESS_COLOUR);
        }
        _ => {}
    }
    let encode_failed = |error: String| {
        invalid(
            "record_frame_unreadable",
            format!("failed to encode the preview of {label}: {error}"),
        )
    };
    Frame::from_pixels(
        width,
        height,
        canvas.pixels,
        format,
        CaptureBackendName::AdbScreencap,
    )
    .and_then(|copy| copy.encode_png_fast())
    .map_err(|error| encode_failed(error.to_string()))
}

/// A frame's pixels being drawn on: `channels` bytes a pixel, row after row, the first three red,
/// green and blue.
struct Canvas {
    width: usize,
    height: usize,
    channels: usize,
    pixels: Vec<u8>,
}

impl Canvas {
    /// Whether `point` is a pixel of the frame.
    fn holds(&self, point: RecordPoint) -> bool {
        usize::try_from(point.x).is_ok_and(|x| x < self.width)
            && usize::try_from(point.y).is_ok_and(|y| y < self.height)
    }

    /// The pixel at `x`, `y` in `colour`; one outside the frame is left.
    fn set(&mut self, x: i64, y: i64, colour: Colour) {
        let (Ok(x), Ok(y)) = (usize::try_from(x), usize::try_from(y)) else {
            return;
        };
        if x >= self.width || y >= self.height {
            return;
        }
        let start = (y * self.width + x) * self.channels;
        if let Some(pixel) = self.pixels.get_mut(start..start + 3) {
            pixel.copy_from_slice(&colour);
        }
    }

    /// A line one pixel thick from `from` to `to`, both ends included (Bresenham's steps).
    fn line(&mut self, from: (i64, i64), to: (i64, i64), colour: Colour) {
        let (mut x, mut y) = from;
        let step_x = if x < to.0 { 1 } else { -1 };
        let step_y = if y < to.1 { 1 } else { -1 };
        let across = (to.0 - x).abs();
        let down = -(to.1 - y).abs();
        let mut error = across + down;
        loop {
            self.set(x, y, colour);
            if x == to.0 && y == to.1 {
                break;
            }
            let doubled = 2 * error;
            if doubled >= down {
                error += down;
                x += step_x;
            }
            if doubled <= across {
                error += across;
                y += step_y;
            }
        }
    }

    /// A line [`THICKNESS`] pixels thick: the line, and beside it the same line one pixel right and
    /// one pixel down.
    fn thick_line(&mut self, from: (i64, i64), to: (i64, i64), colour: Colour) {
        for (right, down) in [(0, 0), (1, 0), (0, 1)] {
            self.line(
                (from.0 + right, from.1 + down),
                (to.0 + right, to.1 + down),
                colour,
            );
        }
    }

    /// The outline of `rect`, [`THICKNESS`] pixels thick inside it, its corners included. Its sides
    /// are drawn within one pixel around the frame, so that a rectangle larger than the frame takes
    /// no longer than the frame's own outline.
    fn outline(&mut self, rect: RecordRect, colour: Colour) {
        let width = i64::try_from(self.width).unwrap_or(i64::MAX);
        let height = i64::try_from(self.height).unwrap_or(i64::MAX);
        let (left, top) = (i64::from(rect.x), i64::from(rect.y));
        let right = left + i64::from(rect.width) - 1;
        let bottom = top + i64::from(rect.height) - 1;
        for inset in 0..THICKNESS {
            let (x0, y0, x1, y1) = (left + inset, top + inset, right - inset, bottom - inset);
            if x0 > x1 || y0 > y1 {
                break;
            }
            let (x0, x1) = (x0.clamp(-1, width), x1.clamp(-1, width));
            let (y0, y1) = (y0.clamp(-1, height), y1.clamp(-1, height));
            self.line((x0, y0), (x1, y0), colour);
            self.line((x0, y1), (x1, y1), colour);
            self.line((x0, y0), (x0, y1), colour);
            self.line((x1, y0), (x1, y1), colour);
        }
    }

    /// A ring of `radius` around `centre`, [`THICKNESS`] pixels thick inside it (the midpoint
    /// circle's steps); the pixel `radius` to the right of the centre is on it.
    fn ring(&mut self, centre: RecordPoint, radius: i64, colour: Colour) {
        let (centre_x, centre_y) = (i64::from(centre.x), i64::from(centre.y));
        for inset in 0..THICKNESS {
            let mut x = radius - inset;
            let mut y = 0;
            let mut error = 1 - x;
            while x >= y {
                for (dx, dy) in [
                    (x, y),
                    (y, x),
                    (-y, x),
                    (-x, y),
                    (-x, -y),
                    (-y, -x),
                    (y, -x),
                    (x, -y),
                ] {
                    self.set(centre_x + dx, centre_y + dy, colour);
                }
                y += 1;
                if error < 0 {
                    error += 2 * y + 1;
                } else {
                    x -= 1;
                    error += 2 * (y - x) + 1;
                }
            }
        }
    }

    /// A cross through `centre`, each of its arms `arm` long; the centre is on it.
    fn cross(&mut self, centre: RecordPoint, arm: i64, colour: Colour) {
        let (x, y) = (i64::from(centre.x), i64::from(centre.y));
        self.thick_line((x - arm, y), (x + arm, y), colour);
        self.thick_line((x, y - arm), (x, y + arm), colour);
    }

    /// An arrow from `from` to `to`, its head at `to`: two strokes twice the ring's radius long, at
    /// thirty degrees on each side of the line. A swipe of no length is a ring at its point.
    fn arrow(&mut self, from: RecordPoint, to: RecordPoint, radius: i32, colour: Colour) {
        let start = (i64::from(from.x), i64::from(from.y));
        let end = (i64::from(to.x), i64::from(to.y));
        let along_x = f64::from(to.x) - f64::from(from.x);
        let along_y = f64::from(to.y) - f64::from(from.y);
        let length = along_x.hypot(along_y);
        if length < 1.0 {
            self.ring(to, i64::from(radius), colour);
            return;
        }
        self.thick_line(start, end, colour);
        let (back_x, back_y) = (-along_x / length, -along_y / length);
        let head = 2.0 * f64::from(radius);
        let (sin, cos) = FRAC_PI_6.sin_cos();
        for side in [1.0, -1.0] {
            let stroke_x = back_x * cos - side * back_y * sin;
            let stroke_y = side * back_x * sin + back_y * cos;
            let tip = (
                end.0 + (stroke_x * head).round() as i64,
                end.1 + (stroke_y * head).round() as i64,
            );
            self.thick_line(end, tip, colour);
        }
    }
}
