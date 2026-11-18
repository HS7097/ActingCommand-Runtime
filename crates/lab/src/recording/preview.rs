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
//!
//! #463 S1-R (`contracts/lab-recording.md`, "A ruler on the pictures a recording hands back"): a
//! copy may carry a ruler ([`PreviewRuler`]), drawn first, under the boxes and the press, in the
//! stored frame's own pixels, the space `record mark` takes a box's corners in. `edges`: along the
//! top edge and the left edge a tick every 50 pixels, a longer one every 100, and beside each
//! longer one its number in a built-in bitmap font ([`DIGITS`]); `grid`: the same over a line one
//! pixel thin across the frame at every 100 pixels, both ways. Each mark is dark over a light halo
//! (a line's dashes alternate the two), so that it reads on a light and on a dark frame. The ruler
//! measures nothing and proposes nothing; without it the copy is drawn as before, byte for byte.

use super::model::{RecordPoint, RecordRect};
use super::store::invalid;
use actingcommand_contract::LabResult;
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use serde::Serialize;
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

/// #463 S1-R: the ruler's dark tone, each mark's core.
pub(crate) const RULER_DARK: Colour = [0, 0, 0];
/// #463 S1-R: the ruler's light tone, the halo around each mark's core.
pub(crate) const RULER_LIGHT: Colour = [255, 255, 255];
/// #463 S1-R: a tick along the top and the left edge every this many pixels.
pub(crate) const RULER_TICK_PX: i64 = 50;
/// #463 S1-R: a longer tick every this many pixels, and beside it its number.
pub(crate) const RULER_LONG_TICK_PX: i64 = 100;
/// #463 S1-R: `grid`'s lines across the frame, both ways, every this many pixels.
pub(crate) const RULER_LINE_PX: i64 = 100;
/// #463 S1-R: how long a tick is, from its edge into the frame, and a longer one.
pub(crate) const RULER_TICK_LENGTH: i64 = 6;
pub(crate) const RULER_LONG_TICK_LENGTH: i64 = 12;
/// #463 S1-R: a number's first pixel, along its edge, this far past its tick, and this far from
/// its edge.
pub(crate) const RULER_NUMBER_OFFSET: i64 = 4;
pub(crate) const RULER_NUMBER_INSET: i64 = 2;
/// #463 S1-R: a cell of the font is this many pixels square; a digit is three cells wide and five
/// high, and one empty cell parts two digits.
pub(crate) const RULER_FONT_SCALE: i64 = 2;
/// #463 S1-R: a line's dashes, dark and light in turn, are this many pixels long.
pub(crate) const RULER_DASH_PX: i64 = 4;
/// #463 S1-R: the ruler's coordinate space, as the answer names it: the stored frame's own pixels,
/// in which `record mark` takes a box's corners and the step table holds a press's point.
pub(crate) const RULER_SPACE: &str = "frame_pixels";
/// #463 S1-R: the built-in digits, `0` to `9`, each five rows of three cells, `#` drawn.
pub(crate) const DIGITS: [[&str; 5]; 10] = [
    ["###", "#.#", "#.#", "#.#", "###"],
    [".#.", "##.", ".#.", ".#.", "###"],
    ["###", "..#", "###", "#..", "###"],
    ["###", "..#", "###", "..#", "###"],
    ["#.#", "#.#", "###", "..#", "..#"],
    ["###", "#..", "###", "..#", "###"],
    ["###", "#..", "###", "#.#", "###"],
    ["###", "..#", "..#", "..#", "..#"],
    ["###", "#.#", "###", "#.#", "###"],
    ["###", "#.#", "###", "..#", "###"],
];
/// #463 S1-R: a digit's cells across and down.
const GLYPH_COLUMNS: i64 = 3;
const GLYPH_ROWS: i64 = 5;

/// #463 S1-R: the ruler a copy carries, the parameter `ruler` of `record review` and `record
/// frames` (a closed list; absent: no ruler).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewRuler {
    /// Ticks and numbers along the top edge and the left edge.
    Edges,
    /// The same, over a thin line across the frame at every 100 pixels, both ways.
    Grid,
}

impl PreviewRuler {
    /// The words the parameter takes, as the operation table lists them.
    pub const WORDS: [&'static str; 2] = ["edges", "grid"];

    /// The ruler `word` names; `None` for every other word.
    pub fn from_word(word: &str) -> Option<Self> {
        match word {
            "edges" => Some(Self::Edges),
            "grid" => Some(Self::Grid),
            _ => None,
        }
    }

    /// Its word, as a ruled copy's name carries it.
    pub fn word(self) -> &'static str {
        match self {
            Self::Edges => "edges",
            Self::Grid => "grid",
        }
    }

    /// What the answer says of it ([`RulerView`]).
    pub fn view(self) -> RulerView {
        RulerView {
            kind: self,
            space: RULER_SPACE,
            tick_px: RULER_TICK_PX,
            long_tick_px: RULER_LONG_TICK_PX,
            number_px: RULER_LONG_TICK_PX,
            top_numbers: "x",
            left_numbers: "y",
            line_px: (self == Self::Grid).then_some(RULER_LINE_PX),
        }
    }
}

/// #463 S1-R: what the ruler on an answer's copies is, as the answer says it: its word, the
/// coordinate space of its numbers, its spacing, what its numbers name, and `grid`'s lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RulerView {
    /// `edges` or `grid`, the word given.
    pub kind: PreviewRuler,
    /// `frame_pixels`: the stored frame's own pixels, the space `record mark` takes a box's
    /// corners in; a copy is of its frame's size.
    pub space: &'static str,
    /// A tick along the top and the left edge at every multiple of this many pixels.
    pub tick_px: i64,
    /// A longer tick at every multiple of this many pixels.
    pub long_tick_px: i64,
    /// At every multiple of this many pixels, the number itself beside its longer tick: to the
    /// right of the tick on the top edge, below it on the left edge (one that would not fit wholly
    /// in the frame is left out).
    pub number_px: i64,
    /// What the numbers along the top edge are: `x`.
    pub top_numbers: &'static str,
    /// What the numbers along the left edge are: `y`.
    pub left_numbers: &'static str,
    /// `grid`: a line across the frame at every multiple of this many pixels, both ways; absent on
    /// `edges`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_px: Option<i64>,
}

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
/// #463 S1-R: with `ruler`, the ruler is drawn on the copy first, under the overlay; without it
/// nothing else is drawn.
pub(crate) fn render(
    png: Vec<u8>,
    overlay: &Overlay,
    ruler: Option<PreviewRuler>,
    label: &str,
) -> LabResult<Vec<u8>> {
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
    if let Some(ruler) = ruler {
        canvas.ruler(ruler);
    }
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

    /// #463 S1-R: the ruler `ruler`: `grid`'s lines first, then along the top edge a tick down from
    /// it at every multiple of [`RULER_TICK_PX`] and along the left edge one across from it, a
    /// multiple of [`RULER_LONG_TICK_PX`] longer and with its number beside it. A mark that falls
    /// outside the frame is left out, so a frame under 100 pixels, or under 50, takes what fits.
    fn ruler(&mut self, ruler: PreviewRuler) {
        let width = i64::try_from(self.width).unwrap_or(i64::MAX);
        let height = i64::try_from(self.height).unwrap_or(i64::MAX);
        if ruler == PreviewRuler::Grid {
            for x in multiples(RULER_LINE_PX, width) {
                for y in 0..height {
                    self.set(x, y, dash(y));
                }
            }
            for y in multiples(RULER_LINE_PX, height) {
                for x in 0..width {
                    self.set(x, y, dash(x));
                }
            }
        }
        for x in multiples(RULER_TICK_PX, width) {
            let core = (0..tick_length(x)).map(|y| (x, y)).collect::<Vec<_>>();
            self.two_tone(&core);
        }
        for y in multiples(RULER_TICK_PX, height) {
            let core = (0..tick_length(y)).map(|x| (x, y)).collect::<Vec<_>>();
            self.two_tone(&core);
        }
        for x in multiples(RULER_LONG_TICK_PX, width) {
            self.number(x, (x + RULER_NUMBER_OFFSET, RULER_NUMBER_INSET));
        }
        for y in multiples(RULER_LONG_TICK_PX, height) {
            self.number(y, (RULER_NUMBER_INSET, y + RULER_NUMBER_OFFSET));
        }
    }

    /// #463 S1-R: the pixels `core` in the ruler's dark tone over a halo of its light tone, every
    /// pixel next to one of them (diagonals too) light first, so that the mark reads on a light and
    /// on a dark frame.
    fn two_tone(&mut self, core: &[(i64, i64)]) {
        for &(x, y) in core {
            for down in -1..=1 {
                for right in -1..=1 {
                    self.set(x + right, y + down, RULER_LIGHT);
                }
            }
        }
        for &(x, y) in core {
            self.set(x, y, RULER_DARK);
        }
    }

    /// #463 S1-R: the decimal digits of `value` in the font ([`DIGITS`]), two tones, the top-left
    /// pixel of the first at `at`; a number that would not lie wholly inside the frame is left out,
    /// so that no cut number is read.
    fn number(&mut self, value: i64, at: (i64, i64)) {
        let glyphs = value
            .to_string()
            .bytes()
            .filter_map(|byte| DIGITS.get(usize::from(byte.wrapping_sub(b'0'))))
            .collect::<Vec<_>>();
        let advance = (GLYPH_COLUMNS + 1) * RULER_FONT_SCALE;
        let count = i64::try_from(glyphs.len()).unwrap_or(i64::MAX);
        let right = at.0 + count * advance - RULER_FONT_SCALE;
        let bottom = at.1 + GLYPH_ROWS * RULER_FONT_SCALE;
        let inside = at.0 >= 0
            && at.1 >= 0
            && usize::try_from(right).is_ok_and(|end| end <= self.width)
            && usize::try_from(bottom).is_ok_and(|end| end <= self.height);
        if !inside {
            return;
        }
        let mut core = Vec::new();
        for (left, glyph) in (0..).map(|place| at.0 + place * advance).zip(glyphs) {
            for (top, row) in (0..).map(|line| at.1 + line * RULER_FONT_SCALE).zip(glyph) {
                for (cell, _) in (0..)
                    .map(|column| left + column * RULER_FONT_SCALE)
                    .zip(row.bytes())
                    .filter(|(_, drawn)| *drawn == b'#')
                {
                    for down in 0..RULER_FONT_SCALE {
                        for across in 0..RULER_FONT_SCALE {
                            core.push((cell + across, top + down));
                        }
                    }
                }
            }
        }
        self.two_tone(&core);
    }
}

/// #463 S1-R: the multiples of `step` from `step` on, below `limit`.
fn multiples(step: i64, limit: i64) -> impl Iterator<Item = i64> {
    (step..limit).step_by(usize::try_from(step).unwrap_or(usize::MAX))
}

/// #463 S1-R: how long the tick at `at` pixels along its edge is: longer at a multiple of
/// [`RULER_LONG_TICK_PX`].
fn tick_length(at: i64) -> i64 {
    if at % RULER_LONG_TICK_PX == 0 {
        RULER_LONG_TICK_LENGTH
    } else {
        RULER_TICK_LENGTH
    }
}

/// #463 S1-R: the tone of a line's pixel `along` pixels from its start: dashes of
/// [`RULER_DASH_PX`] pixels, dark first, then light, in turn.
pub(crate) fn dash(along: i64) -> Colour {
    if (along / RULER_DASH_PX) % 2 == 0 {
        RULER_DARK
    } else {
        RULER_LIGHT
    }
}
