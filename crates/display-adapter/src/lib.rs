// SPDX-License-Identifier: AGPL-3.0-only

//! The display adapter's mapping (Workflow #392 DA-1; model 392 v1.2 §4, §6.7, §7, §9; contract
//! `contracts/display-adapter.md`).
//!
//! Pure integer computation, with no I/O, no backend and no game, between three spaces:
//!
//! - base coordinates: the pixels of a package's `coordinate_space`;
//! - the scale frame: the delivered frame brought to the base's scale, where recognition runs
//!   and base regions land;
//! - the device frame: the delivered frame's own pixels, where evidence and input leave.
//!
//! [`DisplayMap::build`] reads one frame's size and geometry ([`DisplayObservation`]), the
//! package's base and the game's declared rule ([`DisplayRule`]) and returns the map M: an
//! identity, a pure scale or a translation. Regions map into the scale frame by the nine-grid,
//! each corner by its own anchor or the whole box by an explicit [`Anchor`]; values leave for
//! the device frame and come back by the rounding table. A cropped frame is mapped on its
//! logical display; a frame in the display's natural orientation is turned upright first. The
//! same inputs give the same map bit for bit on every host: nothing here is floating point.
//!
//! Workflow #392 DA-2a: the kernel's frame gate and the host's geometry records build the map of
//! a delivered frame from the frame's own geometry ([`DisplayObservation::of_frame`]) and the
//! game's declaration ([`DisplayRule`] from the contract's `StandardPackDisplayLayout`), and the
//! host records it ([`DisplayMap::record`]).
//!
//! Workflow #392 DA-3: the input side. An input the kernel made in base coordinates leaves for
//! the device through [`DisplayMap::device_input`]: every point to the nearest device pixel,
//! inside the device frame and inside the device box its input box maps to (inward), lengths
//! scaled, durations untouched; a cropped frame's input gains the crop's offset and a turned
//! frame's has the turn undone for a backend that takes frame coordinates. The host sends only
//! under the maps the capture side admits ([`DisplayMap::admit_input`]) and only when the map in
//! effect is the one the input was decided under ([`DisplayMap::current_for`]).

use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    CaptureExtent, CaptureFrameCrop, CaptureFrameTransform, CaptureGeometryObservation,
    CaptureInsets, CaptureRotation, CaptureRotationObservation, DisplayArea, DisplayInsetsSource,
    DisplayMapKind, DisplayScaleBy, InputAction, InputPathPoint, StandardPackDisplayLayout,
    TaskGeometryDisplayMap,
};
use std::cmp::Ordering;

pub mod codes;

pub use codes::{DisplayCode, DisplayLocation, DisplayProblem};

#[cfg(test)]
mod gate_392_da1;
#[cfg(test)]
mod gate_392_da3;

/// A point in one of the adapter's spaces, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Point {
    pub x: i64,
    pub y: i64,
}

impl Point {
    pub const fn new(x: i64, y: i64) -> Self {
        Self { x, y }
    }
}

/// A box in one of the adapter's spaces, in pixels: `x .. x + width` by `y .. y + height`, its
/// right and bottom edges open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rect {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

impl Rect {
    pub const fn new(x: i64, y: i64, width: i64, height: i64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// The open right edge, `x + width`.
    pub const fn right(self) -> i64 {
        self.x + self.width
    }

    /// The open bottom edge, `y + height`.
    pub const fn bottom(self) -> i64 {
        self.y + self.height
    }
}

/// Which side of an extent is longer (model 392 §7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Orientation {
    Landscape,
    Portrait,
    Square,
}

impl Orientation {
    pub fn of(extent: CaptureExtent) -> Self {
        match extent.width().cmp(&extent.height()) {
            Ordering::Greater => Self::Landscape,
            Ordering::Less => Self::Portrait,
            Ordering::Equal => Self::Square,
        }
    }

    /// Whether a frame of this orientation shows a package of `package`'s: the same
    /// orientation, or either of them square. The ratio itself is never checked.
    pub fn agrees_with(self, package: Self) -> bool {
        self == package || self == Self::Square || package == Self::Square
    }
}

/// Where a coordinate or an anchor lies along one axis: the first, the middle or the last
/// third (the factor 0, ½ or 1 of model 392 §4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Zone {
    Start,
    Middle,
    End,
}

impl Zone {
    /// The zone of the base coordinate `value` on an axis of `base` pixels: `3 · value < base`
    /// is the start, `3 · value > 2 · base` the end, and everything else the middle, the two
    /// third lines included. A box's right or bottom edge is judged by the same rule at its open
    /// end.
    pub fn of(value: i64, base: u32) -> Self {
        let tripled = i128::from(value) * 3;
        let base = i128::from(base);
        if tripled < base {
            Self::Start
        } else if tripled > base * 2 {
            Self::End
        } else {
            Self::Middle
        }
    }
}

/// One of the nine anchors (model 392 §4.3) a box may be mapped with: the whole box then moves
/// with that anchor and keeps its size. A mapping-side value only: the package member `anchor`
/// that names one is read where its slice (DA-2b, DA-3) declares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Anchor {
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl Anchor {
    /// The nine anchors, row by row.
    pub const ALL: [Self; 9] = [
        Self::TopLeft,
        Self::Top,
        Self::TopRight,
        Self::Left,
        Self::Center,
        Self::Right,
        Self::BottomLeft,
        Self::Bottom,
        Self::BottomRight,
    ];

    /// The anchor's horizontal and vertical zones.
    pub const fn zones(self) -> (Zone, Zone) {
        match self {
            Self::TopLeft => (Zone::Start, Zone::Start),
            Self::Top => (Zone::Middle, Zone::Start),
            Self::TopRight => (Zone::End, Zone::Start),
            Self::Left => (Zone::Start, Zone::Middle),
            Self::Center => (Zone::Middle, Zone::Middle),
            Self::Right => (Zone::End, Zone::Middle),
            Self::BottomLeft => (Zone::Start, Zone::End),
            Self::Bottom => (Zone::Middle, Zone::End),
            Self::BottomRight => (Zone::End, Zone::End),
        }
    }
}

/// The mapping-side input the adapter takes from a game's display declaration
/// (`display-layout.json`, model 392 §4.1): the side and the area the scale factor is measured
/// on, and the area the nine anchors lie on, in SH-2's `DisplayScaleBy` and `DisplayArea`. The
/// declaration's types and its parsing are the contract crate's (DA-4, model 392 §0.2 C9); this
/// is no second copy of that shape: Workflow #392 DA-2a builds it from the contract's
/// `StandardPackDisplayLayout` (`DisplayRule::from`), the one declaration a run keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayRule {
    pub scale_by: DisplayScaleBy,
    pub scale_area: DisplayArea,
    pub anchor_area: DisplayArea,
}

impl From<&StandardPackDisplayLayout> for DisplayRule {
    /// The declaration's `scale.by`, `scale.area` and `anchor_area` (Workflow #392 DA-2a); its
    /// correction radius is the correction window's (DA-2b), not the map's.
    fn from(layout: &StandardPackDisplayLayout) -> Self {
        Self {
            scale_by: layout.scale.by,
            scale_area: layout.scale.area,
            anchor_area: layout.anchor_area,
        }
    }
}

/// What one delivered frame shows (model 392 §2.2, §3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayObservation {
    /// The frame as the backend delivered it.
    pub frame: CaptureExtent,
    /// The logical display the frame was captured from.
    pub display: CaptureExtent,
    /// The display's rotation, `None` when the source does not provide it.
    pub rotation: Option<CaptureRotation>,
    /// The region of the logical display the frame is, when a stream cropped it
    /// (`CaptureFrameTransform::Crop`).
    pub crop: Option<CaptureFrameCrop>,
    /// The insets in use (the device's report or a declaration), in pixels of the logical
    /// display at its current rotation; `None` when none were read.
    pub insets: Option<CaptureInsets>,
}

impl DisplayObservation {
    /// Workflow #392 DA-2a: what a delivered frame of `frame` size shows by the geometry its own
    /// producer recorded with it (`geometry`): the logical display, its rotation when the source
    /// provides it, the crop when a stream recorded one and the insets when they were read. A
    /// frame whose producer recorded no geometry (a snapshot backend, a fixture) shows itself:
    /// its own size is its display, with no known rotation, no crop and no insets.
    pub fn of_frame(frame: CaptureExtent, geometry: &CaptureGeometryObservation) -> Self {
        match geometry {
            CaptureGeometryObservation::Observed(geometry) => Self {
                frame,
                display: geometry.logical_display_extent,
                rotation: match geometry.rotation {
                    CaptureRotationObservation::Observed { rotation, .. } => Some(rotation),
                    CaptureRotationObservation::NotProvidedBySource => None,
                },
                crop: match geometry.frame_transform {
                    Some(CaptureFrameTransform::Crop(crop)) => Some(crop),
                    _ => None,
                },
                insets: geometry.safe_insets,
            },
            CaptureGeometryObservation::Unknown(_)
            | CaptureGeometryObservation::NotApplicable(_) => Self {
                frame,
                display: frame,
                rotation: None,
                crop: None,
                insets: None,
            },
        }
    }
}

/// A quarter turn that brings a frame delivered in the display's natural orientation upright
/// (model 392 §7, row 2): the turn the device layer gives such frames (clockwise for a display
/// at 90°, counterclockwise at 270°).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QuarterTurn {
    Clockwise,
    Counterclockwise,
}

impl QuarterTurn {
    /// The turn a display at `rotation` asks for; none at 0° and 180°.
    pub const fn for_rotation(rotation: CaptureRotation) -> Option<Self> {
        match rotation {
            CaptureRotation::R90 => Some(Self::Clockwise),
            CaptureRotation::R270 => Some(Self::Counterclockwise),
            CaptureRotation::R0 | CaptureRotation::R180 => None,
        }
    }

    /// A pixel of a frame of `delivered` size, after the turn.
    pub fn turn_point(self, point: Point, delivered: CaptureExtent) -> Point {
        let width = i64::from(delivered.width());
        let height = i64::from(delivered.height());
        match self {
            Self::Clockwise => Point::new(height - 1 - point.y, point.x),
            Self::Counterclockwise => Point::new(point.y, width - 1 - point.x),
        }
    }

    /// A pixel of the upright frame, back in the frame of `delivered` size: what a backend that
    /// takes frame coordinates receives (model 392 §6.7).
    pub fn undo_point(self, point: Point, delivered: CaptureExtent) -> Point {
        let width = i64::from(delivered.width());
        let height = i64::from(delivered.height());
        match self {
            Self::Clockwise => Point::new(point.y, height - 1 - point.x),
            Self::Counterclockwise => Point::new(width - 1 - point.y, point.x),
        }
    }
}

/// How a frame's orientation is handled (the four rows of model 392 §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrientationDisposition {
    /// Row 1: the frame agrees with the package's orientation; it is mapped as delivered.
    AsDelivered,
    /// Row 2: the frame is in the display's natural orientation and the display agrees with
    /// the package: the frame is turned upright, then mapped.
    Turn(QuarterTurn),
    /// Row 3: the display disagrees with the package too (the application is not in its
    /// orientation): this frame cannot be mapped, `orientation_mismatch`.
    Mismatch,
    /// Row 4: the frame disagrees with the package and the display's rotation is unknown.
    RotationUnknown,
}

/// The rotation table (model 392 §7) for a package of `base` size. A frame that disagrees with
/// the package while its display agrees, at a rotation that asks for no quarter turn, is left
/// as delivered; [`DisplayMap::build`] then finds that it does not show its display.
pub fn orientation_disposition(
    base: CaptureExtent,
    frame: CaptureExtent,
    display: CaptureExtent,
    rotation: Option<CaptureRotation>,
) -> OrientationDisposition {
    let package = Orientation::of(base);
    if Orientation::of(frame).agrees_with(package) {
        return OrientationDisposition::AsDelivered;
    }
    let Some(rotation) = rotation else {
        return OrientationDisposition::RotationUnknown;
    };
    if !Orientation::of(display).agrees_with(package) {
        return OrientationDisposition::Mismatch;
    }
    match QuarterTurn::for_rotation(rotation) {
        Some(turn) => OrientationDisposition::Turn(turn),
        None => OrientationDisposition::AsDelivered,
    }
}

/// The scale factor `s = num / den` (model 392 §4.2): reduced, and never below 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Scale {
    num: u32,
    den: u32,
}

impl Scale {
    pub const ONE: Self = Self { num: 1, den: 1 };

    pub const fn num(self) -> u32 {
        self.num
    }

    pub const fn den(self) -> u32 {
        self.den
    }

    pub const fn is_one(self) -> bool {
        self.num == self.den
    }

    /// Workflow #392 DA-3 fix 1: whether `2 · s` is a whole number. `s` is reduced, so that is a
    /// denominator of 1 or 2: exactly the scales under which every base point of every base box
    /// maps inside that box's inward device box ([`DisplayMap::admit_input`]).
    pub const fn doubled_is_whole(self) -> bool {
        self.den <= 2
    }

    fn reduced(num: u32, den: u32) -> Self {
        let divisor = greatest_common_divisor(num, den).max(1);
        Self {
            num: num / divisor,
            den: den / divisor,
        }
    }

    /// `value · s` to the nearest pixel, halves up.
    fn up_nearest(self, value: i64) -> i64 {
        saturate(round_half_up(
            i128::from(value) * i128::from(self.num),
            i128::from(self.den),
        ))
    }

    /// `value · s` rounded down.
    fn up_floor(self, value: i64) -> i64 {
        saturate((i128::from(value) * i128::from(self.num)).div_euclid(i128::from(self.den)))
    }

    /// `value · s` rounded up.
    fn up_ceil(self, value: i64) -> i64 {
        let negated = -(i128::from(value) * i128::from(self.num));
        saturate(-(negated.div_euclid(i128::from(self.den))))
    }

    /// `value / s` to the nearest pixel, halves up.
    fn down_nearest(self, value: i64) -> i64 {
        saturate(round_half_up(
            i128::from(value) * i128::from(self.den),
            i128::from(self.num),
        ))
    }
}

/// How a region is mapped (model 392 §5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RegionClass {
    /// A search hint (a template's area, a text to locate, a scan's track): each corner by its
    /// own anchor, so it may stretch, which is harmless.
    SearchHint,
    /// A positional box (a plain text area, a colour probe, a slot, a still-frame region):
    /// stretching would change what it means, so under a translation a box that crosses a
    /// third line and names no anchor is refused (coordinator ruling 2026-10-13, item 1).
    Positional,
}

/// A refusal under `display_map_unresolved` (model 392 §9.1): what could not be mapped, at
/// which step, for which delivered frame and which package base.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayMapUnresolved {
    pub problem: DisplayProblem,
    pub stage: DisplayLocation,
    pub frame: CaptureExtent,
    pub base: CaptureExtent,
}

impl DisplayMapUnresolved {
    /// `display_map_unresolved` with its `display_problem`, its `stage` and the frame's and
    /// the base's sizes. The caller adds its own `operation` and the target or operation id.
    pub fn outcome(&self) -> Outcome {
        Outcome::new(DisplayCode::MapUnresolved)
            .display_problem(self.problem)
            .stage(self.stage)
            .frame_width(self.frame.width())
            .frame_height(self.frame.height())
            .base_width(self.base.width())
            .base_height(self.base.height())
    }
}

/// Why [`DisplayMap::build`] refuses a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayMapError {
    /// `display_map_unresolved` (model 392 §9.1).
    Unresolved(DisplayMapUnresolved),
    /// The rule measures on the safe area and no insets were read (model 392 §4.5); the host
    /// reports it under its own `contained_task_geometry_unavailable`.
    SafeAreaUnavailable,
    /// The frame disagrees with the package's orientation and the display's rotation is
    /// unknown (model 392 §7, row 4); the host reports `contained_task_geometry_unknown`.
    RotationUnknown,
    /// The upright frame does not show its logical display: another aspect ratio, or a crop
    /// that is not the frame (model 392 §2.1, row 4); the host reports
    /// `contained_task_geometry_frame_display_mismatch`.
    FrameDisplayMismatch,
}

/// Workflow #392 DA-3 (model 392 v1.2 §6.7): the coordinates a backend that receives an input
/// takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InputCoordinates {
    /// The logical display's, as `adb input` and the touch protocols take them: a cropped frame's
    /// input gains the crop's offset.
    Display,
    /// The delivered frame's, as a backend that presses on a frame it captured takes them: a
    /// turned frame's input has the quarter turn undone.
    DeliveredFrame,
}

/// Workflow #392 DA-3: why an input cannot leave for the device under a map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMapError {
    /// `display_map_unresolved` at stage `display_input_map`: an input box with less than one
    /// device pixel left (`collapsed`), a point or a box outside the scale frame
    /// (`outside_frame`), or a vertical brake a turned frame cannot keep for a backend that takes
    /// frame coordinates (`orientation_mismatch`).
    Unresolved(DisplayMapUnresolved),
    /// A mapped point outside the device frame, or outside the device box its input box maps to
    /// (model 392 v1.2 §6.2, §9.3: the kernel's `contained_task_input_out_of_bounds`). A point is
    /// never clamped.
    OutOfBounds,
}

/// Workflow #392 DA-3 (model 392 v1.2 §6.6): the map in effect when an input is sent was built
/// for a frame of another size than the frame the input was decided on: the geometry changed in
/// between (`display_map_stale`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayMapStale {
    /// The size of the frame the input was decided on.
    pub decided: CaptureExtent,
    /// The size of the frame the map in effect was built for.
    pub current: CaptureExtent,
}

impl DisplayMapStale {
    /// `display_map_stale` at stage `display_input_map` (its one registered value).
    pub fn outcome(&self) -> Outcome {
        Outcome::new(DisplayCode::MapStale).stage(DisplayLocation::InputMap)
    }
}

const NO_INSETS: CaptureInsets = CaptureInsets {
    left: 0,
    top: 0,
    right: 0,
    bottom: 0,
};

/// The display map M of one frame (model 392 §3.2, §4). It holds while the frame's size, the
/// display, the rotation, the crop, the insets, the rule and the base hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayMap {
    kind: DisplayMapKind,
    base: CaptureExtent,
    frame: CaptureExtent,
    turn: Option<QuarterTurn>,
    scale: Scale,
    scale_extent: CaptureExtent,
    anchor_area: Rect,
    frame_offset: Point,
    scale_frame: CaptureExtent,
    crop: Option<CaptureFrameCrop>,
    rule: Option<DisplayRule>,
}

impl DisplayMap {
    /// Builds the map of one frame for a package of `base` size under the game's `rule`
    /// (`None` when the game declares no display layout).
    ///
    /// - The rotation table comes first; a turned frame swaps its sides.
    /// - The layout is measured on the logical display when the frame is a crop of it, else on
    ///   the upright frame, which must show its display.
    /// - Without a rule only a pure scale of the base with no insets in use maps; with one, `s`
    ///   is the scale area's height or width over the base's.
    /// - `s` below 1 is refused. The scale space is the layout's size divided by `s`, to the
    ///   nearest; insets and the crop's offset are divided the same way.
    pub fn build(
        base: CaptureExtent,
        observation: DisplayObservation,
        rule: Option<DisplayRule>,
    ) -> Result<Self, DisplayMapError> {
        let refuse = |problem: DisplayProblem| {
            DisplayMapError::Unresolved(DisplayMapUnresolved {
                problem,
                stage: DisplayLocation::MapBuild,
                frame: observation.frame,
                base,
            })
        };
        let turn = match orientation_disposition(
            base,
            observation.frame,
            observation.display,
            observation.rotation,
        ) {
            OrientationDisposition::AsDelivered => None,
            OrientationDisposition::Turn(turn) => Some(turn),
            OrientationDisposition::Mismatch => {
                return Err(refuse(DisplayProblem::OrientationMismatch));
            }
            OrientationDisposition::RotationUnknown => {
                return Err(DisplayMapError::RotationUnknown);
            }
        };
        let upright = match turn {
            Some(_) => swapped(observation.frame),
            None => observation.frame,
        };
        let source = match observation.crop {
            Some(crop) if crop_shows(crop, upright, observation.display) => observation.display,
            None if same_aspect(upright, observation.display) => upright,
            _ => return Err(DisplayMapError::FrameDisplayMismatch),
        };
        let insets_in_use = observation.insets.is_some_and(|insets| insets != NO_INSETS);
        let (scale, anchor_insets) = match rule {
            None => {
                if insets_in_use || !same_aspect(source, base) {
                    return Err(refuse(DisplayProblem::LayoutUndeclared));
                }
                (Scale::reduced(source.width(), base.width()), NO_INSETS)
            }
            Some(rule) => {
                let collapsed = refuse(DisplayProblem::Collapsed);
                let scale_insets =
                    area_insets(rule.scale_area, observation.insets, source, collapsed)?;
                let anchor_insets =
                    area_insets(rule.anchor_area, observation.insets, source, collapsed)?;
                let (measured, base_side) = match rule.scale_by {
                    DisplayScaleBy::Height => (
                        source
                            .height()
                            .checked_sub(scale_insets.top)
                            .and_then(|rest| rest.checked_sub(scale_insets.bottom)),
                        base.height(),
                    ),
                    DisplayScaleBy::Width => (
                        source
                            .width()
                            .checked_sub(scale_insets.left)
                            .and_then(|rest| rest.checked_sub(scale_insets.right)),
                        base.width(),
                    ),
                };
                let measured = measured.filter(|side| *side > 0).ok_or(collapsed)?;
                (Scale::reduced(measured, base_side), anchor_insets)
            }
        };
        if scale.num < scale.den {
            return Err(refuse(DisplayProblem::ScaleBelowBase));
        }
        let scale_width = scale.down_nearest(i64::from(source.width()));
        let scale_height = scale.down_nearest(i64::from(source.height()));
        let scale_extent = extent_of(scale_width, scale_height)
            .ok_or_else(|| refuse(DisplayProblem::Collapsed))?;
        let left = scale.down_nearest(i64::from(anchor_insets.left));
        let top = scale.down_nearest(i64::from(anchor_insets.top));
        let right = scale.down_nearest(i64::from(anchor_insets.right));
        let bottom = scale.down_nearest(i64::from(anchor_insets.bottom));
        let anchor_area = Rect::new(
            left,
            top,
            scale_width - left - right,
            scale_height - top - bottom,
        );
        if anchor_area.width < 1 || anchor_area.height < 1 {
            return Err(refuse(DisplayProblem::Collapsed));
        }
        let (frame_offset, frame_width, frame_height) = match observation.crop {
            Some(crop) => (
                Point::new(
                    scale.down_nearest(i64::from(crop.x)),
                    scale.down_nearest(i64::from(crop.y)),
                ),
                scale.down_nearest(i64::from(crop.width)),
                scale.down_nearest(i64::from(crop.height)),
            ),
            None => (Point::new(0, 0), scale_width, scale_height),
        };
        let scale_frame = extent_of(frame_width, frame_height)
            .ok_or_else(|| refuse(DisplayProblem::Collapsed))?;
        let base_area = Rect::new(0, 0, i64::from(base.width()), i64::from(base.height()));
        let kind = if scale_extent != base || anchor_area != base_area {
            DisplayMapKind::Translate
        } else if scale.is_one() {
            DisplayMapKind::Identity
        } else {
            DisplayMapKind::Scale
        };
        Ok(Self {
            kind,
            base,
            frame: observation.frame,
            turn,
            scale,
            scale_extent,
            anchor_area,
            frame_offset,
            scale_frame,
            crop: observation.crop,
            rule,
        })
    }

    pub const fn kind(&self) -> DisplayMapKind {
        self.kind
    }

    /// The package's base size.
    pub const fn base(&self) -> CaptureExtent {
        self.base
    }

    /// The frame's size as the backend delivered it.
    pub const fn frame(&self) -> CaptureExtent {
        self.frame
    }

    /// The quarter turn that brought the frame upright, if any.
    pub const fn turn(&self) -> Option<QuarterTurn> {
        self.turn
    }

    pub const fn scale(&self) -> Scale {
        self.scale
    }

    /// The scale space: the whole layout (the logical display of a crop) divided by `s`.
    pub const fn scale_extent(&self) -> CaptureExtent {
        self.scale_extent
    }

    /// The area the nine anchors lie on, in the scale space.
    pub const fn anchor_area(&self) -> Rect {
        self.anchor_area
    }

    /// The scale frame's size: the frame divided by `s`, the size recognition sees.
    pub const fn scale_frame_extent(&self) -> CaptureExtent {
        self.scale_frame
    }

    /// The part of the scale space the frame covers (a crop's offset, divided by `s`).
    pub fn frame_area(&self) -> Rect {
        Rect::new(
            self.frame_offset.x,
            self.frame_offset.y,
            i64::from(self.scale_frame.width()),
            i64::from(self.scale_frame.height()),
        )
    }

    /// The whole scale frame, the region of `full_frame`.
    pub fn full_frame(&self) -> Rect {
        Rect::new(
            0,
            0,
            i64::from(self.scale_frame.width()),
            i64::from(self.scale_frame.height()),
        )
    }

    pub const fn crop(&self) -> Option<CaptureFrameCrop> {
        self.crop
    }

    pub const fn rule(&self) -> Option<DisplayRule> {
        self.rule
    }

    /// Whether the game declared a display layout.
    pub const fn declared(&self) -> bool {
        self.rule.is_some()
    }

    /// Whether the scale frame is the delivered frame itself (Workflow #392 DA-2a): the
    /// identity, with no crop and no turn. Only then are the frame's pixels, base coordinates
    /// and input coordinates one space.
    pub fn is_plain_identity(&self) -> bool {
        self.kind == DisplayMapKind::Identity && self.crop.is_none() && self.turn.is_none()
    }

    /// The map as a task geometry record's `display_map` (Workflow #392 DA-2a, register §1.25
    /// X7): its kind, `s`, the scale space, the base, and the rule it was measured by. Without a
    /// declaration that is the rule the map was built by: the width over the base's, on the
    /// screen, the anchors on the screen. A rule that measures on the safe area names a
    /// declaration as its insets' source: Workflow #392 DA-4b-D (Alice's ruling 69) reads no
    /// device's cutout, so the only insets a frame's geometry carries are the declared ones (an
    /// emulator's 0 included).
    pub fn record(&self) -> TaskGeometryDisplayMap {
        let rule = self.rule.unwrap_or(DisplayRule {
            scale_by: DisplayScaleBy::Width,
            scale_area: DisplayArea::Screen,
            anchor_area: DisplayArea::Screen,
        });
        let safe_area =
            rule.scale_area == DisplayArea::SafeArea || rule.anchor_area == DisplayArea::SafeArea;
        TaskGeometryDisplayMap {
            kind: self.kind,
            scale_num: self.scale.num,
            scale_den: self.scale.den,
            scale_width: self.scale_extent.width(),
            scale_height: self.scale_extent.height(),
            base_width: self.base.width(),
            base_height: self.base.height(),
            scale_by: rule.scale_by,
            scale_area: rule.scale_area,
            anchor_area: rule.anchor_area,
            insets_source: safe_area.then_some(DisplayInsetsSource::Declared),
            declared: self.rule.is_some(),
        }
    }

    /// A base region in the scale frame (model 392 §4.4, §5.2): each corner by the anchor of
    /// its own zone, or the whole region with `anchor`, then cut to the frame.
    ///
    /// Refused as `collapsed` when no width or height is left, as `outside_frame` when nothing
    /// of it lies in the frame, and, for a positional region, as `straddles_zones` when it
    /// crosses a third line under a translation and names no anchor.
    pub fn map_region(
        &self,
        region: Rect,
        anchor: Option<Anchor>,
        class: RegionClass,
    ) -> Result<Rect, DisplayMapUnresolved> {
        self.region_in_frame(
            region,
            anchor,
            class == RegionClass::Positional,
            DisplayLocation::RegionMap,
        )
    }

    /// A base input box in device-frame pixels (model 392 §6.1): mapped as a positional region,
    /// then rounded inward so it never grows on the device. Every refusal is at the stage
    /// `display_input_map`.
    pub fn map_input_box(
        &self,
        region: Rect,
        anchor: Option<Anchor>,
    ) -> Result<Rect, DisplayMapUnresolved> {
        let mapped = self.region_in_frame(region, anchor, true, DisplayLocation::InputMap)?;
        self.device_input_box(mapped)
    }

    /// A base point in the scale frame, by the anchor of its own zone or by `anchor`; refused
    /// as `outside_frame` (stage `display_input_map`) when it falls outside the frame.
    pub fn map_point(
        &self,
        point: Point,
        anchor: Option<Anchor>,
    ) -> Result<Point, DisplayMapUnresolved> {
        let (zone_x, zone_y) = match anchor {
            Some(anchor) => anchor.zones(),
            None => (
                Zone::of(point.x, self.base.width()),
                Zone::of(point.y, self.base.height()),
            ),
        };
        let x = self.place_x(point.x, zone_x) - self.frame_offset.x;
        let y = self.place_y(point.y, zone_y) - self.frame_offset.y;
        let inside = (0..i64::from(self.scale_frame.width())).contains(&x)
            && (0..i64::from(self.scale_frame.height())).contains(&y);
        if inside {
            Ok(Point::new(x, y))
        } else {
            Err(self.unresolved(DisplayProblem::OutsideFrame, DisplayLocation::InputMap))
        }
    }

    /// A scale-frame point in device-frame pixels, to the nearest (model 392 §4.6).
    pub fn device_point(&self, point: Point) -> Point {
        Point::new(
            self.scale.up_nearest(point.x),
            self.scale.up_nearest(point.y),
        )
    }

    /// A length in base pixels (a brake, `cross_px`, `delta`, `min_segment_px`) in device-frame
    /// pixels, to the nearest; the caller then holds it to its own bounds.
    pub fn device_length(&self, length: i64) -> i64 {
        self.scale.up_nearest(length)
    }

    /// A scale-frame input box in device-frame pixels, rounded inward: the left and top edges
    /// up, the right and bottom edges down. Refused as `collapsed` (stage `display_input_map`)
    /// when less than one pixel of width or height is left.
    pub fn device_input_box(&self, rect: Rect) -> Result<Rect, DisplayMapUnresolved> {
        let left = self.scale.up_ceil(rect.x);
        let top = self.scale.up_ceil(rect.y);
        let right = self.scale.up_floor(rect.right());
        let bottom = self.scale.up_floor(rect.bottom());
        if right <= left || bottom <= top {
            return Err(self.unresolved(DisplayProblem::Collapsed, DisplayLocation::InputMap));
        }
        Ok(Rect::new(left, top, right - left, bottom - top))
    }

    /// A scale-frame evidence or projection box in device-frame pixels, rounded outward: the
    /// left and top edges down, the right and bottom edges up, so it covers what it found.
    pub fn device_evidence_box(&self, rect: Rect) -> Rect {
        let left = self.scale.up_floor(rect.x);
        let top = self.scale.up_floor(rect.y);
        let right = self.scale.up_ceil(rect.right());
        let bottom = self.scale.up_ceil(rect.bottom());
        Rect::new(left, top, right - left, bottom - top)
    }

    /// A device-frame point back in the scale frame, to the nearest (the reverse mapping).
    pub fn scale_frame_point(&self, point: Point) -> Point {
        Point::new(
            self.scale.down_nearest(point.x),
            self.scale.down_nearest(point.y),
        )
    }

    /// A device-frame box back in the scale frame, each edge to the nearest.
    pub fn scale_frame_box(&self, rect: Rect) -> Rect {
        let left = self.scale.down_nearest(rect.x);
        let top = self.scale.down_nearest(rect.y);
        let right = self.scale.down_nearest(rect.right());
        let bottom = self.scale.down_nearest(rect.bottom());
        Rect::new(left, top, right - left, bottom - top)
    }

    /// A device-frame point in logical-display coordinates, what a backend that takes display
    /// coordinates receives (model 392 §4.7, §6.7): the crop's offset added back.
    pub fn display_point(&self, point: Point) -> Point {
        match self.crop {
            Some(crop) => Point::new(point.x + i64::from(crop.x), point.y + i64::from(crop.y)),
            None => point,
        }
    }

    /// A device-frame point of the upright frame in the frame as delivered, what a backend
    /// that takes frame coordinates receives (model 392 §6.7): the quarter turn undone.
    pub fn delivered_point(&self, point: Point) -> Point {
        match self.turn {
            Some(turn) => turn.undo_point(point, self.frame),
            None => point,
        }
    }

    /// Workflow #392 DA-3 (`contracts/display-adapter.md` section 10): whether this Runtime sends
    /// `action` under this map. The input side sends under the maps the capture side admits
    /// (section 8), the identity and a pure scale; under any other map it refuses with the token
    /// the frame gate gives that map: a turned frame `orientation_mismatch`, a translation or a
    /// cropped frame `translation_unsupported` (until DA-2b admits them). Workflow #392 DA-3 fix
    /// 1: of the pure scales it sends under those whose double is a whole number
    /// ([`Scale::doubled_is_whole`]), the scales under which every base point of every base box
    /// lands inside that box's inward device box; any other pure scale refuses every input
    /// `input_scale_unsupported`, whatever its point, so the refusal never depends on a draw. A
    /// vertical-brake drag whose brake the map changes (a scale other than 1) is refused
    /// `input_scale_unsupported` too: the contract holds that drag's brake at exactly 100
    /// pixels. Every refusal is at stage `display_input_map`, with the frame's and the base's
    /// sizes.
    pub fn admit_input(&self, action: &InputAction) -> Result<(), DisplayMapUnresolved> {
        let problem = if self.turn.is_some() {
            Some(DisplayProblem::OrientationMismatch)
        } else if self.kind == DisplayMapKind::Translate || self.crop.is_some() {
            Some(DisplayProblem::TranslationUnsupported)
        } else if !self.scale.doubled_is_whole() {
            Some(DisplayProblem::InputScaleUnsupported)
        } else if let InputAction::SingleTouchDragWithVerticalBrakeV1 {
            brake_distance_px, ..
        } = action
            && self.device_length(i64::from(*brake_distance_px)) != i64::from(*brake_distance_px)
        {
            Some(DisplayProblem::InputScaleUnsupported)
        } else {
            None
        };
        match problem {
            Some(problem) => Err(self.unresolved(problem, DisplayLocation::InputMap)),
            None => Ok(()),
        }
    }

    /// Workflow #392 DA-3 (model 392 v1.2 §6.6): whether this map, in effect when an input is
    /// sent, is the map of the frame the input was decided on, a frame of the size `decided`.
    /// Under the maps the input side sends (the identity and a pure scale) a frame's size decides
    /// its map, so a frame of another size means the geometry changed after that frame was taken:
    /// `display_map_stale`, and the input is not sent.
    pub fn current_for(&self, decided: CaptureExtent) -> Result<(), DisplayMapStale> {
        if decided == self.frame {
            Ok(())
        } else {
            Err(DisplayMapStale {
                decided,
                current: self.frame,
            })
        }
    }

    /// Workflow #392 DA-3 (model 392 v1.2 §4.6, §6.1, §6.2, §6.4, §6.7): `action`, made in base
    /// coordinates, as it leaves for the device.
    ///
    /// - Each point goes into the scale frame by the anchor of its own zone (outside it:
    ///   `outside_frame`), then to the device frame to the nearest, `round_half_up(v · s)`.
    /// - A mapped point must lie inside the device frame and, when it was drawn in a box, inside
    ///   the device box that box maps to (inward); otherwise [`InputMapError::OutOfBounds`]: a
    ///   point is never clamped. `boxes` are the input boxes the step drew its points in, in
    ///   order (a tap's or a long press's one, a drag's press and end); none for a point.
    /// - A length (a vertical drag's brake, the cross of a drag's brake) is
    ///   `round_half_up(|v| · s)` with its sign; a path drag's own box maps inward and holds all
    ///   its points.
    /// - Durations, holds, slopes and the number of points never change.
    /// - The points then take the backend's `coordinates`: a cropped frame's gain the crop's
    ///   offset (display coordinates), a turned frame's have the turn undone (frame coordinates).
    ///
    /// Keys, text and the reset carry no coordinate and leave as they are.
    pub fn device_input(
        &self,
        action: &InputAction,
        boxes: &[Rect],
        coordinates: InputCoordinates,
    ) -> Result<InputAction, InputMapError> {
        Ok(match action {
            InputAction::Tap { x, y } => {
                let press = self.device_input_point(*x, *y, self.drawn_box(boxes, 0)?)?;
                let (x, y) = self.sent_point(press, coordinates)?;
                InputAction::Tap { x, y }
            }
            InputAction::LongTap { x, y, duration_ms } => {
                let press = self.device_input_point(*x, *y, self.drawn_box(boxes, 0)?)?;
                let (x, y) = self.sent_point(press, coordinates)?;
                InputAction::LongTap {
                    x,
                    y,
                    duration_ms: *duration_ms,
                }
            }
            InputAction::Swipe {
                x1,
                y1,
                x2,
                y2,
                duration_ms,
            } => {
                let start = self.device_input_point(*x1, *y1, self.drawn_box(boxes, 0)?)?;
                let end = self.device_input_point(*x2, *y2, self.drawn_box(boxes, 1)?)?;
                let (x1, y1) = self.sent_point(start, coordinates)?;
                let (x2, y2) = self.sent_point(end, coordinates)?;
                InputAction::Swipe {
                    x1,
                    y1,
                    x2,
                    y2,
                    duration_ms: *duration_ms,
                }
            }
            InputAction::SingleTouchDragWithVerticalBrakeV1 {
                x1,
                y1,
                x2,
                y2,
                horizontal_duration_ms,
                corner_hold_ms,
                brake_distance_px,
                brake_duration_ms,
                slope_in,
                slope_out,
                ..
            } => {
                // The brake runs up the upright frame; a quarter turn undone for frame
                // coordinates would make it run across, which this drag cannot say.
                if coordinates == InputCoordinates::DeliveredFrame && self.turn.is_some() {
                    return Err(InputMapError::Unresolved(self.unresolved(
                        DisplayProblem::OrientationMismatch,
                        DisplayLocation::InputMap,
                    )));
                }
                let start = self.device_input_point(*x1, *y1, self.drawn_box(boxes, 0)?)?;
                let corner = self.device_input_point(*x2, *y2, self.drawn_box(boxes, 1)?)?;
                let brake = self.device_length(i64::from(*brake_distance_px));
                let end = Point::new(corner.x, corner.y - brake);
                if !self.in_device_frame(end) {
                    return Err(InputMapError::OutOfBounds);
                }
                let (x1, y1) = self.sent_point(start, coordinates)?;
                let (x2, y2) = self.sent_point(corner, coordinates)?;
                let (x3, y3) = self.sent_point(end, coordinates)?;
                InputAction::SingleTouchDragWithVerticalBrakeV1 {
                    x1,
                    y1,
                    x2,
                    y2,
                    x3,
                    y3,
                    horizontal_duration_ms: *horizontal_duration_ms,
                    corner_hold_ms: *corner_hold_ms,
                    brake_distance_px: narrow(brake)?,
                    brake_duration_ms: *brake_duration_ms,
                    slope_in: *slope_in,
                    slope_out: *slope_out,
                }
            }
            InputAction::SingleTouchDragWithPauseAndBrakeV1 {
                x1,
                y1,
                x2,
                y2,
                drag_duration_ms,
                pause_ms,
                x3,
                y3,
                brake_duration_ms,
            } => {
                let start = self.device_input_point(*x1, *y1, self.drawn_box(boxes, 0)?)?;
                let end = self.device_input_point(*x2, *y2, self.drawn_box(boxes, 1)?)?;
                let brake = Point::new(
                    end.x + self.device_offset(i64::from(*x3) - i64::from(*x2)),
                    end.y + self.device_offset(i64::from(*y3) - i64::from(*y2)),
                );
                if !self.in_device_frame(brake) {
                    return Err(InputMapError::OutOfBounds);
                }
                let (x1, y1) = self.sent_point(start, coordinates)?;
                let (x2, y2) = self.sent_point(end, coordinates)?;
                let (x3, y3) = self.sent_point(brake, coordinates)?;
                InputAction::SingleTouchDragWithPauseAndBrakeV1 {
                    x1,
                    y1,
                    x2,
                    y2,
                    drag_duration_ms: *drag_duration_ms,
                    pause_ms: *pause_ms,
                    x3,
                    y3,
                    brake_duration_ms: *brake_duration_ms,
                }
            }
            InputAction::SingleTouchPathDragV1 {
                box_x,
                box_y,
                box_width,
                box_height,
                points,
                segment_duration_ms,
                corner_hold_ms,
            } => {
                let device_box = self
                    .map_input_box(
                        Rect::new(
                            i64::from(*box_x),
                            i64::from(*box_y),
                            i64::from(*box_width),
                            i64::from(*box_height),
                        ),
                        None,
                    )
                    .map_err(InputMapError::Unresolved)?;
                let points = points
                    .iter()
                    .map(|point| {
                        let device = self.device_input_point(point.x, point.y, Some(device_box))?;
                        let (x, y) = self.sent_point(device, coordinates)?;
                        Ok(InputPathPoint { x, y })
                    })
                    .collect::<Result<Vec<_>, InputMapError>>()?;
                // The box's first and last pixels, sent as points: a turn may swap its sides.
                let first = self.sent_point(Point::new(device_box.x, device_box.y), coordinates)?;
                let last = self.sent_point(
                    Point::new(device_box.right() - 1, device_box.bottom() - 1),
                    coordinates,
                )?;
                let (left, top) = (first.0.min(last.0), first.1.min(last.1));
                let (right, bottom) = (first.0.max(last.0), first.1.max(last.1));
                InputAction::SingleTouchPathDragV1 {
                    box_x: left,
                    box_y: top,
                    box_width: right - left + 1,
                    box_height: bottom - top + 1,
                    points,
                    segment_duration_ms: segment_duration_ms.clone(),
                    corner_hold_ms: *corner_hold_ms,
                }
            }
            InputAction::Key { .. } | InputAction::Text { .. } | InputAction::Reset => {
                action.clone()
            }
        })
    }

    /// The device frame upright: the delivered frame, its sides swapped when it was turned.
    fn upright_frame(&self) -> CaptureExtent {
        match self.turn {
            Some(_) => swapped(self.frame),
            None => self.frame,
        }
    }

    fn in_device_frame(&self, point: Point) -> bool {
        let frame = self.upright_frame();
        (0..i64::from(frame.width())).contains(&point.x)
            && (0..i64::from(frame.height())).contains(&point.y)
    }

    /// The device box the base input box `boxes[index]` maps to (inward), when the step drew its
    /// point in one.
    fn drawn_box(&self, boxes: &[Rect], index: usize) -> Result<Option<Rect>, InputMapError> {
        boxes
            .get(index)
            .map(|rect| {
                self.map_input_box(*rect, None)
                    .map_err(InputMapError::Unresolved)
            })
            .transpose()
    }

    /// A base point of an input in device-frame pixels, inside the device frame and inside
    /// `within`, the device box it was drawn in, when it has one.
    fn device_input_point(
        &self,
        x: i32,
        y: i32,
        within: Option<Rect>,
    ) -> Result<Point, InputMapError> {
        let scale_point = self
            .map_point(Point::new(i64::from(x), i64::from(y)), None)
            .map_err(InputMapError::Unresolved)?;
        let device = self.device_point(scale_point);
        if self.in_device_frame(device) && within.is_none_or(|rect| rect_contains(rect, device)) {
            Ok(device)
        } else {
            Err(InputMapError::OutOfBounds)
        }
    }

    /// A signed length in base pixels in device-frame pixels: its size to the nearest, its sign
    /// kept.
    fn device_offset(&self, length: i64) -> i64 {
        let size = self.device_length(length.abs());
        if length < 0 { -size } else { size }
    }

    /// A device-frame point in the coordinates the backend takes.
    fn sent_point(
        &self,
        point: Point,
        coordinates: InputCoordinates,
    ) -> Result<(i32, i32), InputMapError> {
        let sent = match coordinates {
            InputCoordinates::Display => self.display_point(point),
            InputCoordinates::DeliveredFrame => self.delivered_point(point),
        };
        Ok((narrow(sent.x)?, narrow(sent.y)?))
    }

    fn region_in_frame(
        &self,
        region: Rect,
        anchor: Option<Anchor>,
        positional: bool,
        stage: DisplayLocation,
    ) -> Result<Rect, DisplayMapUnresolved> {
        let (start, end) = match anchor {
            Some(anchor) => (anchor.zones(), anchor.zones()),
            None => {
                let start = (
                    Zone::of(region.x, self.base.width()),
                    Zone::of(region.y, self.base.height()),
                );
                let end = (
                    Zone::of(region.right(), self.base.width()),
                    Zone::of(region.bottom(), self.base.height()),
                );
                if positional && self.kind == DisplayMapKind::Translate && start != end {
                    return Err(self.unresolved(DisplayProblem::StraddlesZones, stage));
                }
                (start, end)
            }
        };
        let left = self.place_x(region.x, start.0);
        let top = self.place_y(region.y, start.1);
        let right = self.place_x(region.right(), end.0);
        let bottom = self.place_y(region.bottom(), end.1);
        if right <= left || bottom <= top {
            return Err(self.unresolved(DisplayProblem::Collapsed, stage));
        }
        let left = (left - self.frame_offset.x).max(0);
        let top = (top - self.frame_offset.y).max(0);
        let right = (right - self.frame_offset.x).min(i64::from(self.scale_frame.width()));
        let bottom = (bottom - self.frame_offset.y).min(i64::from(self.scale_frame.height()));
        if right <= left || bottom <= top {
            return Err(self.unresolved(DisplayProblem::OutsideFrame, stage));
        }
        Ok(Rect::new(left, top, right - left, bottom - top))
    }

    fn place_x(&self, value: i64, zone: Zone) -> i64 {
        place(
            value,
            zone,
            self.anchor_area.x,
            self.anchor_area.width,
            self.base.width(),
        )
    }

    fn place_y(&self, value: i64, zone: Zone) -> i64 {
        place(
            value,
            zone,
            self.anchor_area.y,
            self.anchor_area.height,
            self.base.height(),
        )
    }

    fn unresolved(&self, problem: DisplayProblem, stage: DisplayLocation) -> DisplayMapUnresolved {
        DisplayMapUnresolved {
            problem,
            stage,
            frame: self.frame,
            base: self.base,
        }
    }
}

/// A base coordinate in the scale space along one axis (model 392 §4.3): the anchor area's
/// start, plus the coordinate, plus nothing, half (rounded down) or all of the room the area
/// has beyond the base, by zone.
fn place(value: i64, zone: Zone, area_start: i64, area_length: i64, base: u32) -> i64 {
    let room = area_length - i64::from(base);
    let offset = match zone {
        Zone::Start => 0,
        Zone::Middle => room.div_euclid(2),
        Zone::End => room,
    };
    area_start + value + offset
}

/// The insets of `area` (none for the screen); the safe area needs read insets that leave a
/// width and a height inside `source`.
fn area_insets(
    area: DisplayArea,
    observed: Option<CaptureInsets>,
    source: CaptureExtent,
    collapsed: DisplayMapError,
) -> Result<CaptureInsets, DisplayMapError> {
    match area {
        DisplayArea::Screen => Ok(NO_INSETS),
        DisplayArea::SafeArea => match observed {
            None => Err(DisplayMapError::SafeAreaUnavailable),
            Some(insets) if insets_fit(insets, source) => Ok(insets),
            Some(_) => Err(collapsed),
        },
    }
}

fn insets_fit(insets: CaptureInsets, source: CaptureExtent) -> bool {
    u64::from(insets.left) + u64::from(insets.right) < u64::from(source.width())
        && u64::from(insets.top) + u64::from(insets.bottom) < u64::from(source.height())
}

/// Whether a recorded crop is exactly the frame and lies inside the display (#402 RD-3i).
fn crop_shows(crop: CaptureFrameCrop, frame: CaptureExtent, display: CaptureExtent) -> bool {
    crop.width == frame.width()
        && crop.height == frame.height()
        && u64::from(crop.x) + u64::from(crop.width) <= u64::from(display.width())
        && u64::from(crop.y) + u64::from(crop.height) <= u64::from(display.height())
}

/// Whether `point` lies in `rect`, its right and bottom edges open.
fn rect_contains(rect: Rect, point: Point) -> bool {
    (rect.x..rect.right()).contains(&point.x) && (rect.y..rect.bottom()).contains(&point.y)
}

/// A device coordinate as an input's `i32`; one that does not fit lies outside every frame.
fn narrow(value: i64) -> Result<i32, InputMapError> {
    i32::try_from(value).map_err(|_| InputMapError::OutOfBounds)
}

fn same_aspect(left: CaptureExtent, right: CaptureExtent) -> bool {
    u64::from(left.width()) * u64::from(right.height())
        == u64::from(left.height()) * u64::from(right.width())
}

fn swapped(extent: CaptureExtent) -> CaptureExtent {
    CaptureExtent::new(extent.height(), extent.width()).unwrap_or(extent)
}

fn extent_of(width: i64, height: i64) -> Option<CaptureExtent> {
    CaptureExtent::new(u32::try_from(width).ok()?, u32::try_from(height).ok()?)
}

fn greatest_common_divisor(mut left: u32, mut right: u32) -> u32 {
    while right != 0 {
        (left, right) = (right, left % right);
    }
    left
}

/// `numerator / denominator` to the nearest integer, halves up (towards positive infinity),
/// for a positive denominator.
fn round_half_up(numerator: i128, denominator: i128) -> i128 {
    (numerator * 2 + denominator).div_euclid(denominator * 2)
}

/// Narrows a computed coordinate, holding it at the ends of `i64`; no base or device value
/// comes near them.
fn saturate(value: i128) -> i64 {
    match i64::try_from(value) {
        Ok(narrow) => narrow,
        Err(_) if value < 0 => i64::MIN,
        Err(_) => i64::MAX,
    }
}
