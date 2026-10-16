// SPDX-License-Identifier: AGPL-3.0-only

//! The scan session of a select step (Workflow #308 slice SEL-L3; frozen selector model S0 v2.1
//! §2.1–§2.6).
//!
//! A scan select reads a list that does not fit one screen. The session settles the step's
//! dispatched frame, confirms the start of the list (rewinding toward it when the declaration
//! allows), and then reads one coverage viewport after another, each brought by one declared
//! advance drag, until the declared end check passes. Every viewport after the first is aligned
//! against the coverage viewport before it: anchors pair by their signatures, the pairs' offsets
//! cluster, and exactly one consistent cluster must lie inside the window the motion admits.
//! Positions are the tracked instances in content coordinates; the first complete reading of a
//! position fixes it, and each viewport evaluates the positions it fixed with the one pinned
//! fact snapshot and instant. Coverage is proven, never assumed: the bands overlap (one
//! corrective reverse advance per gap), the spacing checks find missed anchors, and every
//! position that was seen was also read. A scan that declares `early_stop` stops before the end
//! once no unknown position and no position not yet seen could outrank the best member (§3.3).
//!
//! Every number comes from the declaration; the Runtime has no default. Every failure is a
//! registered code (§2.7). Slice SEL-L4b (`select`) adds what follows the coverage checks: the
//! decision over the members, the unknown-instance rules of §3.2, the return to the chosen member,
//! its confirmation on a fresh frame and the v2 projection of the record; it also admits the
//! declaration from a package. Every drag is a real input, recorded as a `task.effect_intent` of
//! the select step and followed by no `task.effect_completed`; the one completed effect of the
//! step follows its selecting tap.
//!
//! Slice SEL-L3b (Workflow #308; forced compromises E-10, E-11, E-12) widens how a scan moves.
//! The list may run along either axis: every rule reads positions along the declared axis and
//! across it. A drag may end with a pause, a brake across the axis, or both, so that the list
//! keeps no fling (the input `single_touch_drag_with_pause_and_brake_v1`). A list may also be
//! paged by a tap (`rect`, with `back_rect` for the reverse move): its pages align by their
//! overlap within the declared steps, as drags do, or, when they do not overlap, by page-order
//! evidence: one declared check per page, of which exactly one passes on each viewport, and each
//! move shows the same page or the next one.

mod admission;
mod online;
mod select;

pub(super) use online::RuntimeScanSource;
#[cfg(test)]
pub(super) use online::run_scan;
pub use online::{dry_run_scan, replay_scan};
pub(super) use select::{
    IdentityState, ScanSelectEnd, ScanSelection, bound_inputs, decision_blocking, record_bound,
    result_blocked,
};

use super::{ClickRect, ContainedTaskError, ContainedTaskRunError};
use crate::codes::{ExecutionKernelCode, ExecutionKernelLocation};
use actingcommand_contract::{
    CandidateFeature, CandidateFeatureMap, CandidateRect, CandidateUnknownReason, FrameId,
    ScanAxis, ScanCoverageState, ScanStartEvidence, ScanUnknownCause, ScanViewportMotion,
};
use actingcommand_recognition::color_digest::{self, ColorDigest};
use actingcommand_recognition_pack::CandidateLayout;
use actingcommand_selection_policy::{
    Candidate, CandidateStatus, FieldDomain, ScalarValue, SelectionDecision, SelectionFactSnapshot,
    SelectionOutcome, SelectionPolicy, SortDirection, TieBreakKey, UpperBound, evaluate,
    upper_bound,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

const SELECT_INVALID: ExecutionKernelCode = ExecutionKernelCode::ContainedTaskSelectInvalid;

/// A select step's `scan` (frozen model S0 v2.1 §2.1): how the list moves, how a viewport
/// settles, which anchors are tracked and how they align, how the start and the end of the list
/// are proven, and the bounds of the session. Every member is required; the Runtime has no
/// default. Slice SEL-L4b reads it from the step's `select` and admits it (the static checks of
/// §2.1); the session takes it as admitted.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScanDeclaration {
    /// The layout's identity feature whose reading identifies a position.
    identity_feature: String,
    axis: ScanAxis,
    advance: DeclaredAdvance,
    reposition: DeclaredReposition,
    settle: DeclaredSettle,
    track: DeclaredTrack,
    start: DeclaredStart,
    end: DeclaredEnd,
    bounds: DeclaredBounds,
    /// The one optional member: stop before the end once no position that was not read could
    /// outrank the best member (S0 §3.3).
    #[serde(default)]
    early_stop: Option<DeclaredEarlyStop>,
}

/// How a scan may stop before the end of its list (S0 §3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DeclaredEarlyStop {
    UpperBound,
}

/// A rectangle of the declaration, in frame pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredRect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

impl DeclaredRect {
    const fn click(self) -> ClickRect {
        ClickRect {
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
        }
    }

    const fn candidate(self) -> CandidateRect {
        CandidateRect {
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
        }
    }
}

/// A drag's brake (Workflow #308 SEL-L3b): after the drag's end and its pause, the touch moves
/// `cross_px` across the scan's axis (signed, in frame pixels) over `duration_ms` and is then
/// released, so that the list keeps no fling along the axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredBrake {
    cross_px: i32,
    duration_ms: u64,
}

/// The evidence a page-flip advance aligns by when its pages do not overlap (Workflow #308
/// SEL-L3b): `checks[k]` is the check target that passes on page k, page 0 being the list's
/// start, and page k's content starts `page_step_px` times k along the axis.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredPageOrder {
    checks: Vec<String>,
    page_step_px: u32,
}

/// How a scan moves the list forward. `drag` (S0 §2.1): a drag between random points of two
/// rectangles, with the measured smallest and largest content displacement of a full drag
/// mid-list; a corrective move is this drag with `from` and `to` swapped. SEL-L3b lets it end
/// with a pause (`pause_ms`), a brake, or both. `rect` (SEL-L3b, the page flip of S0 §8 item 3):
/// a tap in `rect`, whose corrective is a tap in `back_rect`; its pages align by their overlap
/// within the declared steps, or, with `page_order` and no steps, by page-order evidence.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum DeclaredAdvance {
    Drag {
        from: DeclaredRect,
        to: DeclaredRect,
        duration_ms: u64,
        #[serde(default)]
        pause_ms: Option<u64>,
        #[serde(default)]
        brake: Option<DeclaredBrake>,
        min_content_step_px: u32,
        max_content_step_px: u32,
    },
    Rect {
        rect: DeclaredRect,
        back_rect: DeclaredRect,
        #[serde(default)]
        min_content_step_px: Option<u32>,
        #[serde(default)]
        max_content_step_px: Option<u32>,
        #[serde(default)]
        page_order: Option<DeclaredPageOrder>,
    },
}

/// The longer move a rewind takes reversed (and SEL-L4b's return in either direction): a drag,
/// optionally with a pause and a brake, or (SEL-L3b) a tap in `rect`, whose reverse is a tap in
/// `back_rect`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum DeclaredReposition {
    Drag {
        from: DeclaredRect,
        to: DeclaredRect,
        duration_ms: u64,
        #[serde(default)]
        pause_ms: Option<u64>,
        #[serde(default)]
        brake: Option<DeclaredBrake>,
        max_content_step_px: u32,
    },
    Rect {
        /// The forward move, which only a return toward the end takes (SEL-L4b).
        rect: DeclaredRect,
        back_rect: DeclaredRect,
        max_content_step_px: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredSettle {
    interval_ms: u64,
    timeout_ms: u64,
}

/// The color digest of a rectangle relative to the anchor match: an instance's signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredSignature {
    rect: DeclaredRect,
    columns: u32,
    rows: u32,
}

/// The distances that find a missed anchor (§2.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredSpacing {
    max_axis_gap_px: u32,
    columns: u32,
    max_trailing_gap_px: u32,
    max_cross_lead_px: u32,
    max_cross_gap_px: u32,
}

/// Which anchors are tracked and how they pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredTrack {
    area: DeclaredRect,
    edge_margin_px: u32,
    signature: DeclaredSignature,
    signature_max_distance_milli: u32,
    position_tolerance_px: u32,
    min_overlap_instances: u32,
    spacing: DeclaredSpacing,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredStart {
    check: String,
    rewind: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredEnd {
    check: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredBounds {
    max_viewports: u32,
    max_reposition_viewports: u32,
    max_ms: u64,
}

actingcommand_contract::outcome_vocabulary! {
    /// Why an alignment found no shift (the vocabulary `scan_alignment_failure`, S0 §2.4 step 5).
    pub(super) enum ScanAlignmentFailure: "scan_alignment_failure" {
        /// No cluster of pairs reaches `min_overlap_instances`.
        TooFewMatches => "too_few_matches",
        /// No consistent cluster, for any other reason.
        NoConsistentShift => "no_consistent_shift",
        /// Two or more consistent clusters lie inside the motion's window.
        AmbiguousShift => "ambiguous_shift",
        /// No consistent cluster lies inside the window, but one lies outside it.
        UnexpectedDisplacement => "unexpected_displacement",
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// Which spacing check found a missed anchor (the vocabulary `scan_spacing_failure`, S0
    /// §2.5).
    pub(super) enum ScanSpacingFailure: "scan_spacing_failure" {
        /// Two consecutive rows lie further apart than `max_axis_gap_px`.
        AxisGap => "axis_gap",
        /// A row holds another number of positions than `spacing.columns`.
        RowShort => "row_short",
        /// The first row starts further from the band's leading edge than `max_axis_gap_px`.
        LeadingGap => "leading_gap",
        /// The last row starts further from the band's trailing edge than `max_trailing_gap_px`.
        TrailingGap => "trailing_gap",
        /// A row's first position or the next one lies too far across the axis.
        CrossGap => "cross_gap",
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// Which bound of a scan ran out (the vocabulary `scan_bound`, S0 §2.5).
    pub(super) enum ScanBound: "scan_bound" {
        /// `max_viewports`: the coverage viewports.
        CoverageViewports => "coverage_viewports",
        /// `max_reposition_viewports`: the rewind (and return) viewports.
        RepositionViewports => "reposition_viewports",
        /// `max_ms`, or the task's own deadline when it comes first.
        Duration => "duration",
    }
}

/// One instance of a viewport's G7 projection: its instance rectangle in frame coordinates, its
/// signature when the signature rectangle lies in the frame, whether it is actionable, and its
/// features (none when it is not actionable).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ScanReadInstance {
    pub(super) rect: CandidateRect,
    pub(super) signature: Option<ColorDigest>,
    pub(super) actionable: bool,
    pub(super) features: CandidateFeatureMap,
}

/// A settled viewport frame, read once: its instances, the start and end checks, and the frame's
/// identity when the runtime gave one.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ScanReading {
    pub(super) instances: Vec<ScanReadInstance>,
    pub(super) start_check: bool,
    pub(super) end_check: bool,
    pub(super) frame_id: Option<FrameId>,
}

/// Why a move is made: a forward advance, the corrective reverse advance of a gap, a reversed
/// reposition toward the start, or (SEL-L4b, S0 §2.2 `Return`) a move toward the chosen member:
/// the declared `reposition` or `advance`, toward the start or the end of the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ScanDragMotion {
    Advance,
    Corrective,
    Rewind,
    Return {
        reposition: bool,
        toward_start: bool,
    },
}

/// A brake across the scan's axis (Workflow #308 SEL-L3b): the displacement (x, y) in frame
/// pixels from the drag's end, and its duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ScanBrake {
    pub(super) displacement: (i32, i32),
    pub(super) duration_ms: u64,
}

/// What one move of a scan does on the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ScanInput {
    /// A drag between random points of `from` and `to`, released at its end (a swipe).
    Drag {
        from: ClickRect,
        to: ClickRect,
        duration_ms: u64,
    },
    /// A drag between random points of `from` and `to` that holds still `pause_ms` at its end
    /// and then, with a brake, moves by the brake's displacement before the release (Workflow
    /// #308 SEL-L3b, `single_touch_drag_with_pause_and_brake_v1`).
    HeldDrag {
        from: ClickRect,
        to: ClickRect,
        duration_ms: u64,
        pause_ms: u64,
        brake: Option<ScanBrake>,
    },
    /// A tap at a random point of `rect`: a page flip (Workflow #308 SEL-L3b).
    Tap { rect: ClickRect },
}

/// One move of the attempt; `index` counts the moves of the attempt and varies the action seed.
#[derive(Debug, Clone, Copy)]
pub(super) struct ScanDrag {
    pub(super) motion: ScanDragMotion,
    pub(super) input: ScanInput,
    pub(super) index: u32,
}

/// The moves, steps and page-order evidence a session takes from its declaration, resolved once
/// (S0 §2.1; Workflow #308 SEL-L3b).
#[derive(Debug, Clone, Copy)]
struct Moves<'a> {
    advance: ScanInput,
    corrective: ScanInput,
    rewind: ScanInput,
    /// The reposition toward the end, which only a return takes (SEL-L4b).
    reposition_forward: ScanInput,
    /// `advance`'s smallest and largest content step.
    advance_steps: (i64, i64),
    /// `reposition`'s largest content step.
    reposition_max: i64,
    /// The page checks and the page step, when the pages align by page-order evidence.
    page_order: Option<(&'a [String], i64)>,
}

/// A drag of the declaration as an input: a swipe, or, with a pause or a brake, the held drag
/// whose brake moves across `axis`.
fn drag_input(
    axis: ScanAxis,
    (from, to, duration_ms): (DeclaredRect, DeclaredRect, u64),
    pause_ms: Option<u64>,
    brake: Option<DeclaredBrake>,
) -> ScanInput {
    if pause_ms.is_none() && brake.is_none() {
        return ScanInput::Drag {
            from: from.click(),
            to: to.click(),
            duration_ms,
        };
    }
    ScanInput::HeldDrag {
        from: from.click(),
        to: to.click(),
        duration_ms,
        pause_ms: pause_ms.unwrap_or(0),
        brake: brake.map(|brake| ScanBrake {
            displacement: match axis {
                ScanAxis::Vertical => (brake.cross_px, 0),
                ScanAxis::Horizontal => (0, brake.cross_px),
            },
            duration_ms: brake.duration_ms,
        }),
    }
}

/// Whether a drag's pause and brake are usable: a declared pause holds, and a declared brake
/// moves and takes time.
fn held_drag_valid(pause_ms: Option<u64>, brake: Option<DeclaredBrake>) -> bool {
    pause_ms != Some(0) && brake.is_none_or(|brake| brake.cross_px != 0 && brake.duration_ms > 0)
}

/// Why a declaration's moves cannot be resolved (the detail of `contained_task_select_invalid`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MovesInvalid {
    AdvanceHold,
    AdvanceRectSteps,
    RepositionHold,
}

impl MovesInvalid {
    const fn detail(self) -> &'static str {
        match self {
            Self::AdvanceHold => "scan advance=drag hold=invalid",
            Self::AdvanceRectSteps => "scan advance=rect steps_or_page_order=invalid",
            Self::RepositionHold => "scan reposition=drag hold=invalid",
        }
    }
}

impl<'a> Moves<'a> {
    /// Resolves the declaration's `advance` and `reposition`. A page-flip advance has either
    /// both steps or `page_order` (with at least one check and a page step of at least 1 px),
    /// never both; a declared pause is not 0 and a declared brake moves and takes time.
    fn of(scan: &'a ScanDeclaration) -> Result<Self, MovesInvalid> {
        let axis = scan.axis;
        let (advance, corrective, advance_steps, page_order) = match &scan.advance {
            DeclaredAdvance::Drag {
                from,
                to,
                duration_ms,
                pause_ms,
                brake,
                min_content_step_px,
                max_content_step_px,
            } => {
                if !held_drag_valid(*pause_ms, *brake) {
                    return Err(MovesInvalid::AdvanceHold);
                }
                (
                    drag_input(axis, (*from, *to, *duration_ms), *pause_ms, *brake),
                    drag_input(axis, (*to, *from, *duration_ms), *pause_ms, *brake),
                    (
                        i64::from(*min_content_step_px),
                        i64::from(*max_content_step_px),
                    ),
                    None,
                )
            }
            DeclaredAdvance::Rect {
                rect,
                back_rect,
                min_content_step_px,
                max_content_step_px,
                page_order,
            } => {
                let (steps, order) = match (min_content_step_px, max_content_step_px, page_order) {
                    (Some(min), Some(max), None) => ((i64::from(*min), i64::from(*max)), None),
                    (None, None, Some(order))
                        if !order.checks.is_empty() && order.page_step_px > 0 =>
                    {
                        let step = i64::from(order.page_step_px);
                        ((step, step), Some((order.checks.as_slice(), step)))
                    }
                    _ => return Err(MovesInvalid::AdvanceRectSteps),
                };
                (
                    ScanInput::Tap { rect: rect.click() },
                    ScanInput::Tap {
                        rect: back_rect.click(),
                    },
                    steps,
                    order,
                )
            }
        };
        let (rewind, reposition_forward, reposition_max) = match scan.reposition {
            DeclaredReposition::Drag {
                from,
                to,
                duration_ms,
                pause_ms,
                brake,
                max_content_step_px,
            } => {
                if !held_drag_valid(pause_ms, brake) {
                    return Err(MovesInvalid::RepositionHold);
                }
                (
                    drag_input(axis, (to, from, duration_ms), pause_ms, brake),
                    drag_input(axis, (from, to, duration_ms), pause_ms, brake),
                    max_content_step_px,
                )
            }
            DeclaredReposition::Rect {
                rect,
                back_rect,
                max_content_step_px,
            } => (
                ScanInput::Tap {
                    rect: back_rect.click(),
                },
                ScanInput::Tap { rect: rect.click() },
                max_content_step_px,
            ),
        };
        Ok(Self {
            advance,
            corrective,
            rewind,
            reposition_forward,
            advance_steps,
            reposition_max: i64::from(reposition_max),
            page_order,
        })
    }
}

/// What a scan session reads frames from and drags through: the runtime of a select step, or a
/// saved frame sequence. Every capture keeps the dispatched frame's page and geometry, or fails.
pub(super) trait ScanSource {
    type Error;

    /// The monotonic time since the session began.
    fn elapsed(&mut self) -> Duration;

    /// The instance rectangles of the step's dispatched frame F1.
    fn dispatched(&mut self) -> Result<Vec<CandidateRect>, ContainedTaskRunError<Self::Error>>;

    /// Waits `interval`, captures a fresh frame of the step's page and geometry, and returns
    /// its instance rectangles (anchor matching only).
    fn capture(
        &mut self,
        interval: Duration,
    ) -> Result<Vec<CandidateRect>, ContainedTaskRunError<Self::Error>>;

    /// The latest frame as a viewport: the step's guard on it, the start and end checks and the
    /// layout's projection.
    fn viewport(&mut self) -> Result<ScanReading, ContainedTaskRunError<Self::Error>>;

    /// The step's guard alone on the latest frame (SEL-L4b): `Confirm` judges it before it reads
    /// F2, so a refusal is recorded as the guard's (S0 §3.4 P8). A viewport read after it on the
    /// same frame does not judge it again. A source whose viewports judge no guard passes.
    fn guard(&mut self) -> Result<(), ContainedTaskRunError<Self::Error>> {
        Ok(())
    }

    /// One move, recorded as an effect intent of the step and bound to the latest frame.
    fn drag(&mut self, drag: ScanDrag) -> Result<(), ContainedTaskRunError<Self::Error>>;

    /// The indices of the declared page checks that pass on the latest frame (Workflow #308
    /// SEL-L3b): the evidence a page-flip advance without overlap aligns by. A source that reads
    /// no page checks refuses.
    fn page_checks(
        &mut self,
        _checks: &[String],
    ) -> Result<Vec<u32>, ContainedTaskRunError<Self::Error>> {
        Err(ContainedTaskError::with_detail(
            SELECT_INVALID.code(),
            "scan page_order=unread_by_source",
        )
        .into())
    }
}

/// How a scan session ended without a failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ScanEnd {
    /// The evaluator aborted: at `Pin` over the declared lists (`coverage` `not_started`, path
    /// P2) or in a viewport (`incomplete`, path P3). The decision is the record's.
    Aborted {
        decision: Box<SelectionDecision>,
        coverage: ScanCoverageState,
    },
    /// The start and end checks passed, every gap closed, the spacing checks passed and every
    /// seen position was read: the coverage is complete and `Decide` may evaluate the members.
    Covered,
    /// The early-stop test held at a coverage viewport (S0 §3.3): no unknown position and no
    /// position not yet seen could outrank the best member. Every position before that
    /// viewport's band was read, and the spacing checks passed on the rows before it.
    EarlyStopped,
}

/// An upper bound (S0 §3.3) as an ordered value: `None` (no eligible identity, or nothing to
/// bound) lies below every score and `Unbounded` above every score.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::contained_task) enum Bound {
    None,
    Max(i64),
    Unbounded,
}

impl From<UpperBound> for Bound {
    fn from(bound: UpperBound) -> Self {
        match bound {
            UpperBound::NoneEligible => Self::None,
            UpperBound::Max(milli) => Self::Max(milli),
            UpperBound::Unbounded => Self::Unbounded,
        }
    }
}

/// What the early-stop test reads (S0 §3.3): the domain of every policy field, built from the
/// layout, the identity domain, the bound over the whole domain for positions not yet seen,
/// and whether the policy's first tie-break key is `candidate_id lowest_first`.
#[derive(Debug, Clone)]
struct EarlyStop {
    field_domains: BTreeMap<String, FieldDomain>,
    identities: Vec<String>,
    unseen: Bound,
    first_key_lowest_id: bool,
}

/// One rectangle split along the scan's axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AxisRect {
    axis: i64,
    axis_extent: i64,
    cross: i64,
    cross_extent: i64,
}

impl AxisRect {
    fn of(axis: ScanAxis, rect: CandidateRect) -> Self {
        let (x, y) = (i64::from(rect.x), i64::from(rect.y));
        let (width, height) = (i64::from(rect.width), i64::from(rect.height));
        match axis {
            ScanAxis::Vertical => Self {
                axis: y,
                axis_extent: height,
                cross: x,
                cross_extent: width,
            },
            ScanAxis::Horizontal => Self {
                axis: x,
                axis_extent: width,
                cross: y,
                cross_extent: height,
            },
        }
    }

    const fn axis_end(self) -> i64 {
        self.axis + self.axis_extent
    }

    const fn cross_end(self) -> i64 {
        self.cross + self.cross_extent
    }

    const fn within(self, outer: Self) -> bool {
        self.axis >= outer.axis
            && self.axis_end() <= outer.axis_end()
            && self.cross >= outer.cross
            && self.cross_end() <= outer.cross_end()
    }
}

/// The geometry every viewport shares: the tracking area and its core, the readable band, the
/// instance extent on the axis and the position tolerance.
#[derive(Debug, Clone, Copy)]
struct ScanGeometry {
    area: AxisRect,
    /// The area shrunk by `edge_margin_px` at its two axis edges, on the axis.
    core: (i64, i64),
    band: AxisRect,
    /// `e`: the extent of `instance_rect` on the axis.
    extent: i64,
    /// The frame's extent on the axis.
    frame_axis: i64,
    tolerance: i64,
}

impl ScanGeometry {
    /// Whether an instance that starts at `axis` (frame coordinates) lies inside the core.
    const fn in_core(&self, axis: i64) -> bool {
        axis >= self.core.0 && axis + self.extent <= self.core.1
    }
}

/// One tracked instance of a viewport: inside the tracking area, in frame coordinates.
#[derive(Debug, Clone, PartialEq)]
struct TrackedInstance {
    rect: AxisRect,
    signature: ColorDigest,
    /// The Known identity this viewport read, when the instance is actionable.
    identity: Option<String>,
    actionable: bool,
    features: CandidateFeatureMap,
}

/// One instance an alignment pairs against, in content coordinates: a tracked instance of a
/// coverage viewport, or a position of the content map.
struct Reference<'a> {
    axis: i64,
    cross: i64,
    signature: &'a ColorDigest,
    identity: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Pair {
    reference: usize,
    current: usize,
    /// The content offset of the current frame this pair proposes.
    offset: i64,
}

/// An alignment that found no single shift: why, and the pairs of the best cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AlignmentRejected {
    failure: ScanAlignmentFailure,
    best: usize,
}

/// What the evaluator said about a fixed position in the viewport that fixed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Member { score_milli: Option<i64> },
    GatedOut,
    UnknownDropped,
}

#[derive(Debug, Clone, PartialEq)]
enum PositionState {
    /// Tracked, never actionable.
    Unread,
    /// Read, without a complete reading so far.
    Incomplete,
    /// The first complete reading fixed it in `viewport`; the evaluator's verdict follows.
    Fixed {
        viewport: u32,
        features: CandidateFeatureMap,
        verdict: Option<Verdict>,
    },
    /// A later Known reading of a policy-read feature differed from the fixed one.
    Contradicted,
}

/// One position of the content map (§2.6): a tracked instance's rectangle in content
/// coordinates; rectangles within the tolerance are one position.
#[derive(Debug, Clone, PartialEq)]
struct Position {
    /// First-seen order among all tracked positions of the attempt.
    index: u32,
    rect: AxisRect,
    /// The signature of the position's first tracked occurrence.
    signature: ColorDigest,
    first_seen_viewport: u32,
    first_read_viewport: Option<u32>,
    /// The Known identity: the fixed one, or the one an unknown position kept.
    identity: Option<String>,
    /// Why the identity read Unknown, while no Known identity was read.
    identity_unknown: Option<CandidateUnknownReason>,
    state: PositionState,
}

/// One coverage viewport (start, forward or corrective).
#[derive(Debug, Clone, PartialEq)]
struct CoverageViewport {
    motion: ScanViewportMotion,
    offset: i64,
    tracked: u32,
    readable: u32,
    unknown: u32,
    frame_id: Option<FrameId>,
    end_check: bool,
}

/// A value of a reading compared across viewports: its type and value, never its confidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KnownValue<'a> {
    Boolean(bool),
    Integer(i64),
    Identity(&'a str),
}

fn known_value(feature: &CandidateFeature) -> Option<KnownValue<'_>> {
    match feature {
        CandidateFeature::Boolean { value, .. } => Some(KnownValue::Boolean(*value)),
        CandidateFeature::Integer { value, .. } => Some(KnownValue::Integer(*value)),
        CandidateFeature::Identity { value, .. } => Some(KnownValue::Identity(value.as_str())),
        CandidateFeature::Unknown { .. } => None,
    }
}

/// The evaluator's fields of one reading: a boolean, an integer or an identity as the
/// projection carries it; Unknown features are absent.
fn candidate_fields(
    features: &CandidateFeatureMap,
) -> std::collections::BTreeMap<String, ScalarValue> {
    features
        .iter()
        .filter_map(|(name, feature)| {
            let value = match feature {
                CandidateFeature::Boolean { value, .. } => ScalarValue::Boolean(*value),
                CandidateFeature::Integer { value, .. } => ScalarValue::Integer(*value),
                CandidateFeature::Identity { value, .. } => ScalarValue::String(value.clone()),
                CandidateFeature::Unknown { .. } => return None,
            };
            Some((name.clone(), value))
        })
        .collect()
}

/// The lower median offset of a cluster sorted by offset.
fn median(cluster: &[Pair]) -> i64 {
    cluster
        .get(cluster.len().saturating_sub(1) / 2)
        .map_or(0, |pair| pair.offset)
}

/// Whether `offset` lies in one of the window's closed intervals.
fn in_window(window: &[(i64, i64)], offset: i64) -> bool {
    window
        .iter()
        .any(|&(low, high)| low <= offset && offset <= high)
}

/// The alignment of `current` (frame coordinates) against `reference` (content coordinates)
/// within `window` (S0 §2.4 steps 3–5): the content offset of the current frame, or why no
/// single offset holds. `region` is the content interval on the axis in which the reference
/// holds every instance (the core of a reference viewport, or the covered extent of the content
/// map). A cluster is consistent when every reference instance its offset places inside the
/// current core, and every current core instance it places inside `region`, has its pair: the
/// overlap is matched completely, and instances that enter the frame for the first time need
/// none.
fn align(
    geometry: &ScanGeometry,
    track: &DeclaredTrack,
    (reference, region): (&[Reference<'_>], (i64, i64)),
    current: &[TrackedInstance],
    window: &[(i64, i64)],
) -> Result<i64, AlignmentRejected> {
    let tolerance = geometry.tolerance;
    let mut pairs = Vec::new();
    for (reference_index, known) in reference.iter().enumerate() {
        for (current_index, instance) in current.iter().enumerate() {
            if (known.cross - instance.rect.cross).abs() > tolerance {
                continue;
            }
            if let (Some(kept), Some(read)) = (known.identity, instance.identity.as_deref())
                && kept != read
            {
                continue;
            }
            let Ok(distance) = color_digest::distance(known.signature, &instance.signature, &[])
            else {
                continue;
            };
            if distance.mean_milli > track.signature_max_distance_milli {
                continue;
            }
            pairs.push(Pair {
                reference: reference_index,
                current: current_index,
                offset: known.axis - instance.rect.axis,
            });
        }
    }
    pairs.sort_by_key(|pair| (pair.offset, pair.reference, pair.current));
    // Split between neighbours more than one tolerance apart, then merge adjacent clusters
    // whose medians lie within two tolerances, so that jitter never splits one shift.
    let mut clusters: Vec<Vec<Pair>> = Vec::new();
    for pair in pairs {
        match clusters.last_mut() {
            Some(cluster)
                if cluster
                    .last()
                    .is_some_and(|last| pair.offset - last.offset <= tolerance) =>
            {
                cluster.push(pair);
            }
            _ => clusters.push(vec![pair]),
        }
    }
    while let Some(index) = (1..clusters.len()).find(|&index| {
        (median(&clusters[index]) - median(&clusters[index - 1])).abs() <= 2 * tolerance
    }) {
        let merged = clusters.remove(index);
        clusters[index - 1].extend(merged);
    }
    let minimum = usize::try_from(track.min_overlap_instances).unwrap_or(usize::MAX);
    let consistent = |cluster: &[Pair]| {
        let offset = median(cluster);
        let mut references = BTreeSet::new();
        let mut currents = BTreeSet::new();
        for pair in cluster {
            if !references.insert(pair.reference) || !currents.insert(pair.current) {
                return false;
            }
        }
        cluster.len() >= minimum
            && reference.iter().enumerate().all(|(index, known)| {
                !geometry.in_core(known.axis - offset) || references.contains(&index)
            })
            && current.iter().enumerate().all(|(index, instance)| {
                let content = instance.rect.axis + offset;
                let referenced = content >= region.0 && content + geometry.extent <= region.1;
                !geometry.in_core(instance.rect.axis) || !referenced || currents.contains(&index)
            })
    };
    let best = clusters.iter().map(Vec::len).max().unwrap_or(0);
    let mut inside = Vec::new();
    let mut outside = 0_usize;
    for cluster in &clusters {
        if !consistent(cluster.as_slice()) {
            continue;
        }
        let offset = median(cluster);
        if in_window(window, offset) {
            inside.push(offset);
        } else {
            outside += 1;
        }
    }
    let failure = match inside.as_slice() {
        [offset] => return Ok(*offset),
        [_, _, ..] => ScanAlignmentFailure::AmbiguousShift,
        [] if outside > 0 => ScanAlignmentFailure::UnexpectedDisplacement,
        [] if best < minimum => ScanAlignmentFailure::TooFewMatches,
        [] => ScanAlignmentFailure::NoConsistentShift,
    };
    Err(AlignmentRejected { failure, best })
}

/// The first gap of the coverage bands (content intervals), as the pixels by which a band
/// starts too late to overlap the union before it by `extent`.
fn coverage_gap(bands: &[(i64, i64)], extent: i64) -> Option<i64> {
    let mut sorted = bands.to_vec();
    sorted.sort_unstable();
    let mut bands = sorted.into_iter();
    let (_, mut reach) = bands.next()?;
    for (start, end) in bands {
        let needed = reach - extent;
        if start > needed {
            return Some(start - needed);
        }
        reach = reach.max(end);
    }
    None
}

fn u32_of(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

/// One scan session over one source (S0 §2.2 from `Pin` to `Decide`'s coverage checks).
pub(super) struct ScanSession<'a> {
    scan: &'a ScanDeclaration,
    /// The declaration's moves, steps and page-order evidence (Workflow #308 SEL-L3b).
    moves: Moves<'a>,
    /// The page of the latest coverage viewport, when the pages align by page-order evidence.
    page: i64,
    layout_id: &'a str,
    /// The layout (SEL-L4b: its page, click and features for the decision and the record).
    layout: &'a CandidateLayout,
    /// The dispatched frame's size.
    frame: (u32, u32),
    policy: &'a SelectionPolicy,
    facts: &'a SelectionFactSnapshot,
    now_unix_ms: u64,
    /// The features the policy reads.
    read_fields: Vec<String>,
    /// The layout's features that declare consensus and its identity features: the kernel
    /// keeps an instance whose consensus is unresolved, or whose identity is not an identity,
    /// out of the evaluator, so such a reading is never complete.
    consensus_features: Vec<String>,
    identity_features: Vec<String>,
    geometry: ScanGeometry,
    /// `max_ms`, or the task's remaining time when that is shorter.
    budget: Duration,
    positions: Vec<Position>,
    viewports: Vec<CoverageViewport>,
    /// The coverage viewports' bands in content coordinates.
    bands: Vec<(i64, i64)>,
    /// The tracked instances of the latest coverage viewport, and its offset.
    previous: Vec<TrackedInstance>,
    previous_offset: i64,
    rewind_viewports: u32,
    /// The viewports a return to the chosen member settled (SEL-L4b); with the rewind viewports
    /// they count toward `max_reposition_viewports`.
    return_viewports: u32,
    drags: u32,
    start: Option<ScanStartEvidence>,
    forward_repeats: u32,
    /// The early-stop test's inputs, when the scan declares `early_stop`.
    early_stop: Option<EarlyStop>,
    /// The coverage viewport at which the early-stop test held.
    stopped_at: Option<u32>,
}

/// The early-stop test's inputs (S0 §3.3): every policy field's domain from the layout
/// (`passed` boolean, `measure_milli` 0..=1000, `ocr_integer` its declared range, the scan's
/// identity feature the identity itself), the identity feature's entries, and the bound over
/// the whole domain, which the pinned facts and instant fix for the attempt.
fn early_stop_inputs(
    scan: &ScanDeclaration,
    layout: &CandidateLayout,
    policy: &SelectionPolicy,
    facts: &SelectionFactSnapshot,
    now_unix_ms: u64,
) -> Result<EarlyStop, ContainedTaskError> {
    let invalid = |detail: String| {
        ContainedTaskError::with_detail(
            SELECT_INVALID.code(),
            format!("layout={} {detail}", layout.id),
        )
    };
    // SEL-L4b: the same inputs bound the unknown instances at `Decide` (S0 §3.2 rule 3).
    let (field_domains, identities) =
        select::bound_inputs(layout, policy, Some(&scan.identity_feature))
            .map_err(|detail| invalid(format!("scan early_stop {detail}")))?;
    let unseen = upper_bound(policy, facts, now_unix_ms, &identities, &field_domains)
        .map_err(|error| invalid(format!("scan early_stop bound={error}")))?;
    Ok(EarlyStop {
        field_domains,
        identities,
        unseen: unseen.into(),
        first_key_lowest_id: matches!(
            policy.tie_break.first(),
            Some(TieBreakKey::CandidateId {
                direction: SortDirection::LowestFirst
            })
        ),
    })
}

impl<'a> ScanSession<'a> {
    /// A session over `layout` (the step's `repeated_anchor` layout) with the step's admitted
    /// `scan`, its sealed `policy`, and the facts and instant pinned for the whole attempt.
    /// `frame` is the dispatched frame's size; `task_remaining`, the task's time left when the
    /// step started, caps `max_ms`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        scan: &'a ScanDeclaration,
        layout: &'a CandidateLayout,
        policy: &'a SelectionPolicy,
        facts: &'a SelectionFactSnapshot,
        now_unix_ms: u64,
        frame: (u32, u32),
        task_remaining: Duration,
    ) -> Result<Self, ContainedTaskError> {
        let invalid = |detail: &str| {
            ContainedTaskError::with_detail(
                SELECT_INVALID.code(),
                format!("layout={} {detail}", layout.id),
            )
        };
        let instance_rect = layout
            .instance_rect
            .ok_or_else(|| invalid("scan=declared instance_rect=absent"))?;
        let frame_rect = CandidateRect {
            x: 0,
            y: 0,
            width: i32::try_from(frame.0).map_err(|_| invalid("frame=oversized"))?,
            height: i32::try_from(frame.1).map_err(|_| invalid("frame=oversized"))?,
        };
        let band = layout
            .readable_band
            .map_or(frame_rect, |band| CandidateRect {
                x: band.x,
                y: band.y,
                width: band.width,
                height: band.height,
            });
        let axis = scan.axis;
        let area = AxisRect::of(axis, scan.track.area.candidate());
        let margin = i64::from(scan.track.edge_margin_px);
        let geometry = ScanGeometry {
            area,
            core: (area.axis + margin, area.axis_end() - margin),
            band: AxisRect::of(axis, band),
            extent: AxisRect::of(
                axis,
                CandidateRect {
                    x: instance_rect.x,
                    y: instance_rect.y,
                    width: instance_rect.width,
                    height: instance_rect.height,
                },
            )
            .axis_extent,
            frame_axis: AxisRect::of(axis, frame_rect).axis_extent,
            tolerance: i64::from(scan.track.position_tolerance_px),
        };
        if !layout
            .features
            .iter()
            .any(|feature| feature.name == scan.identity_feature && feature.identity.is_some())
        {
            return Err(invalid("scan identity_feature=undeclared"));
        }
        let early_stop = match scan.early_stop {
            Some(DeclaredEarlyStop::UpperBound) => {
                Some(early_stop_inputs(scan, layout, policy, facts, now_unix_ms)?)
            }
            None => None,
        };
        let moves = Moves::of(scan).map_err(|refused| invalid(refused.detail()))?;
        Ok(Self {
            scan,
            moves,
            page: 0,
            layout_id: &layout.id,
            layout,
            frame,
            policy,
            facts,
            now_unix_ms,
            read_fields: policy
                .fields
                .iter()
                .map(|field| field.name.clone())
                .collect(),
            consensus_features: layout
                .features
                .iter()
                .filter(|feature| feature.consensus.is_some())
                .map(|feature| feature.name.clone())
                .collect(),
            identity_features: layout
                .features
                .iter()
                .filter(|feature| feature.identity.is_some())
                .map(|feature| feature.name.clone())
                .collect(),
            geometry,
            budget: Duration::from_millis(scan.bounds.max_ms).min(task_remaining),
            positions: Vec::new(),
            viewports: Vec::new(),
            bands: Vec::new(),
            previous: Vec::new(),
            previous_offset: 0,
            rewind_viewports: 0,
            return_viewports: 0,
            drags: 0,
            start: None,
            forward_repeats: 0,
            early_stop,
            stopped_at: None,
        })
    }

    /// Runs the session from `Pin` to `Decide`'s coverage checks (S0 §2.2).
    pub(super) fn run<S: ScanSource>(
        &mut self,
        source: &mut S,
    ) -> Result<ScanEnd, ContainedTaskRunError<S::Error>> {
        // Pin: the declared lists are read once, before any viewport; an unusable list is the
        // decision `unknown` with no coverage (path P2).
        let pinned = self.evaluate_candidates(&[])?;
        if matches!(pinned.outcome, SelectionOutcome::Unknown { .. }) {
            return Ok(ScanEnd::Aborted {
                decision: Box::new(pinned),
                coverage: ScanCoverageState::NotStarted,
            });
        }
        self.check_budget(source, ExecutionKernelLocation::SelectScanSettle)?;
        let dispatched = source.dispatched()?;
        let mut reading = self.settle(source, Some(dispatched), 0)?;
        let mut start = ScanStartEvidence::StartCheck;
        if !reading.start_check {
            if !self.scan.start.rewind {
                return Err(start_unconfirmed(0).into());
            }
            start = ScanStartEvidence::RewindCheck;
            let mut repeats = 0_u32;
            loop {
                self.check_budget(source, ExecutionKernelLocation::SelectScanStart)?;
                if self.rewind_viewports >= self.scan.bounds.max_reposition_viewports {
                    return Err(bounds_exceeded(
                        ExecutionKernelLocation::SelectScanStart,
                        ScanBound::RepositionViewports,
                        self.scan.bounds.max_reposition_viewports,
                    )
                    .into());
                }
                let before = self.tracked(&reading)?;
                self.drag(source, ScanDragMotion::Rewind)?;
                reading = self.settle(source, None, 0)?;
                self.rewind_viewports += 1;
                if reading.start_check {
                    break;
                }
                let after = self.tracked(&reading)?;
                if self.repeats(&before, &after) {
                    repeats += 1;
                    if repeats >= 2 {
                        return Err(start_unconfirmed(0).into());
                    }
                } else {
                    repeats = 0;
                }
            }
        }
        self.start = Some(start);
        let tracked = self.tracked(&reading)?;
        if let Some((checks, _)) = self.moves.page_order {
            self.page = self.place_page(source, checks, None, 0)?;
        }
        if let Some(end) = self.observe(tracked, &reading, ScanViewportMotion::Start, 0)? {
            return Ok(end);
        }
        // The coverage viewport (start or forward) on which the end check passed, and its
        // offset; a corrective that follows it does not move the end.
        let mut end_viewport = reading.end_check.then_some((0_u32, 0_i64));
        loop {
            // EndTest (S0 §2.2), in its order: the end check, the early stop, the repeats.
            let latest = u32_of(self.viewports.len().saturating_sub(1));
            if let Some((index, offset)) = end_viewport {
                self.check_trailing(index, offset)?;
                return self.decide_coverage().map_err(ContainedTaskRunError::Task);
            }
            if self.early_stop_holds()? {
                self.stopped_at = Some(latest);
                return self.decide_coverage().map_err(ContainedTaskRunError::Task);
            }
            if self.forward_repeats >= 2 {
                return Err(ContainedTaskError::valued(
                    ExecutionKernelCode::SelectionScanEndUnconfirmed.code(),
                    |outcome| {
                        outcome
                            .stage(ExecutionKernelLocation::SelectScanEnd)
                            .viewport_index(latest)
                    },
                )
                .into());
            }
            let reading = self.advance(source, ScanDragMotion::Advance)?;
            let index = u32_of(self.viewports.len());
            let current = self.tracked(&reading)?;
            let previous_offset = self.previous_offset;
            let offset = match self.moves.page_order {
                // Workflow #308 SEL-L3b: pages that do not overlap are placed by the page they
                // show, the same page or the next one, at the declared page step.
                Some((checks, step)) => {
                    self.page = self.place_page(source, checks, Some(self.page), index)?;
                    self.page * step
                }
                None => {
                    let window = self.forward_window(previous_offset, reading.end_check);
                    self.align_previous(&current, &window, index)?
                }
            };
            if (offset - previous_offset).abs() <= self.geometry.tolerance {
                self.forward_repeats += 1;
            } else {
                self.forward_repeats = 0;
            }
            if let Some(end) =
                self.observe(current, &reading, ScanViewportMotion::Forward, offset)?
            {
                return Ok(end);
            }
            if reading.end_check {
                end_viewport = Some((index, offset));
            }
            // Pages placed by page-order evidence tile the list by declaration: no gap to close.
            if self.moves.page_order.is_none()
                && coverage_gap(&self.bands, self.geometry.extent).is_some()
            {
                // One corrective reverse advance per gap, aligned against the viewport that
                // opened it; the gap is then checked again against the union.
                let reading = self.advance(source, ScanDragMotion::Corrective)?;
                let index = u32_of(self.viewports.len());
                let current = self.tracked(&reading)?;
                let window = self.corrective_window(self.previous_offset);
                let offset = self.align_previous(&current, &window, index)?;
                if let Some(end) =
                    self.observe(current, &reading, ScanViewportMotion::Corrective, offset)?
                {
                    return Ok(end);
                }
                if let Some(gap_px) = coverage_gap(&self.bands, self.geometry.extent) {
                    return Err(ContainedTaskError::valued(
                        ExecutionKernelCode::SelectionScanGapDetected.code(),
                        |outcome| {
                            outcome
                                .stage(ExecutionKernelLocation::SelectScanAlign)
                                .viewport_index(index)
                                .gap_px(gap_px)
                        },
                    )
                    .into());
                }
            }
        }
    }

    /// Fails the session with `duration` once its time is spent.
    fn check_budget<S: ScanSource>(
        &self,
        source: &mut S,
        stage: ExecutionKernelLocation,
    ) -> Result<(), ContainedTaskError> {
        if source.elapsed() >= self.budget {
            let timeout_ms = u64::try_from(self.budget.as_millis()).unwrap_or(u64::MAX);
            return Err(ContainedTaskError::valued(
                ExecutionKernelCode::SelectionScanBoundsExceeded.code(),
                |outcome| {
                    outcome
                        .stage(stage)
                        .scan_bound(ScanBound::Duration)
                        .timeout_ms(timeout_ms)
                },
            ));
        }
        Ok(())
    }

    /// Settles a frame (S0 §2.3): captures at `settle.interval_ms` until two consecutive captures
    /// give the same core anchor set, then reads the later one as the viewport. `first` is the
    /// dispatched frame's anchors at `Pin`; after an input every capture is fresh.
    fn settle<S: ScanSource>(
        &self,
        source: &mut S,
        first: Option<Vec<CandidateRect>>,
        viewport_index: u32,
    ) -> Result<ScanReading, ContainedTaskRunError<S::Error>> {
        self.settle_capture(source, first, viewport_index)?;
        source.viewport()
    }

    /// The captures of [`Self::settle`] until two consecutive ones give the same core anchor
    /// set; the latest frame is then the settled one, not yet read (SEL-L4b: `Confirm` judges the
    /// guard before it reads F2).
    fn settle_capture<S: ScanSource>(
        &self,
        source: &mut S,
        first: Option<Vec<CandidateRect>>,
        viewport_index: u32,
    ) -> Result<(), ContainedTaskRunError<S::Error>> {
        let interval = Duration::from_millis(self.scan.settle.interval_ms);
        let timeout = Duration::from_millis(self.scan.settle.timeout_ms);
        let started = source.elapsed();
        let mut previous = first;
        loop {
            self.check_budget(source, ExecutionKernelLocation::SelectScanSettle)?;
            let anchors = source.capture(interval)?;
            if previous
                .as_ref()
                .is_some_and(|previous| self.same_core_anchors(previous, &anchors))
            {
                return Ok(());
            }
            if source.elapsed().saturating_sub(started) >= timeout {
                let timeout_ms = self.scan.settle.timeout_ms;
                return Err(ContainedTaskError::valued(
                    ExecutionKernelCode::SelectionScanSettleIncomplete.code(),
                    |outcome| {
                        outcome
                            .stage(ExecutionKernelLocation::SelectScanSettle)
                            .viewport_index(viewport_index)
                            .timeout_ms(timeout_ms)
                    },
                )
                .into());
            }
            previous = Some(anchors);
        }
    }

    /// Whether two captures give the same core anchor set: the anchors whose instance lies
    /// inside the tracking area and the core pair one-to-one within the tolerance; anchors in the
    /// margins are ignored.
    fn same_core_anchors(&self, previous: &[CandidateRect], current: &[CandidateRect]) -> bool {
        let core = |rects: &[CandidateRect]| {
            let mut kept = rects
                .iter()
                .map(|rect| AxisRect::of(self.scan.axis, *rect))
                .filter(|rect| rect.within(self.geometry.area) && self.geometry.in_core(rect.axis))
                .map(|rect| (rect.axis, rect.cross))
                .collect::<Vec<_>>();
            kept.sort_unstable();
            kept
        };
        let (previous, current) = (core(previous), core(current));
        let tolerance = self.geometry.tolerance;
        previous.len() == current.len()
            && previous.iter().zip(&current).all(|(before, after)| {
                (before.0 - after.0).abs() <= tolerance && (before.1 - after.1).abs() <= tolerance
            })
    }

    /// The tracked instances of a viewport: those inside the tracking area, with their
    /// signatures and the identity each actionable one read.
    fn tracked(&self, reading: &ScanReading) -> Result<Vec<TrackedInstance>, ContainedTaskError> {
        reading
            .instances
            .iter()
            .filter_map(|instance| {
                let rect = AxisRect::of(self.scan.axis, instance.rect);
                rect.within(self.geometry.area).then_some((rect, instance))
            })
            .map(|(rect, instance)| {
                let signature = instance.signature.clone().ok_or_else(|| {
                    ContainedTaskError::with_detail(
                        SELECT_INVALID.code(),
                        format!("layout={} scan signature=outside_frame", self.layout_id),
                    )
                })?;
                let identity = match instance.features.get(&self.scan.identity_feature) {
                    Some(CandidateFeature::Identity { value, .. }) if instance.actionable => {
                        Some(value.clone())
                    }
                    _ => None,
                };
                Ok(TrackedInstance {
                    rect,
                    signature,
                    identity,
                    actionable: instance.actionable,
                    features: instance.features.clone(),
                })
            })
            .collect()
    }

    /// Whether a rewind viewport repeats the one before it: every core instance pairs one-to-one
    /// at a shift within the tolerance (S0 §2.4 step 6).
    fn repeats(&self, before: &[TrackedInstance], after: &[TrackedInstance]) -> bool {
        let reference = before
            .iter()
            .map(|instance| Reference {
                axis: instance.rect.axis,
                cross: instance.rect.cross,
                signature: &instance.signature,
                identity: instance.identity.as_deref(),
            })
            .collect::<Vec<_>>();
        let tolerance = self.geometry.tolerance;
        align(
            &self.geometry,
            &self.scan.track,
            (reference.as_slice(), self.geometry.core),
            after,
            &[(-tolerance, tolerance)],
        )
        .is_ok()
    }

    /// The offsets a forward viewport may lie at: no move, or a full step; where the end check
    /// passed, any step up to the largest, because the list stopped at its end.
    fn forward_window(&self, previous: i64, end_passed: bool) -> Vec<(i64, i64)> {
        let tolerance = self.geometry.tolerance;
        let (minimum, maximum) = self.moves.advance_steps;
        if end_passed {
            vec![(previous - tolerance, previous + maximum + tolerance)]
        } else {
            vec![
                (previous - tolerance, previous + tolerance),
                (
                    previous + minimum - tolerance,
                    previous + maximum + tolerance,
                ),
            ]
        }
    }

    /// The offsets a corrective viewport may lie at: no move, or one reversed step.
    fn corrective_window(&self, previous: i64) -> Vec<(i64, i64)> {
        let tolerance = self.geometry.tolerance;
        let (minimum, maximum) = self.moves.advance_steps;
        vec![
            (previous - tolerance, previous + tolerance),
            (
                previous - maximum - tolerance,
                previous - minimum + tolerance,
            ),
        ]
    }

    /// The window of a return move (S0 §2.4 step 5, `Return`): toward the start or the end by
    /// at most the largest step of the motion used, with no minimum.
    fn return_window(&self, last: i64, toward_start: bool, reposition: bool) -> Vec<(i64, i64)> {
        let tolerance = self.geometry.tolerance;
        let step = if reposition {
            self.moves.reposition_max
        } else {
            self.moves.advance_steps.1
        };
        if toward_start {
            vec![(last - step - tolerance, last + tolerance)]
        } else {
            vec![(last - tolerance, last + step + tolerance)]
        }
    }

    /// The content offset of a frame aligned directly against the content map within `window`
    /// (S0 §2.4 step 7): the reference of SEL-L4b's `Return` and `Confirm`. Every tracked
    /// position of the coverage takes part with its stored signature and its Known identity.
    fn align_to_content_map(
        &self,
        reading: &ScanReading,
        window: &[(i64, i64)],
        stage: ExecutionKernelLocation,
        viewport_index: u32,
    ) -> Result<i64, ContainedTaskError> {
        let current = self.tracked(reading)?;
        let reference = self
            .positions
            .iter()
            .map(|position| Reference {
                axis: position.rect.axis,
                cross: position.rect.cross,
                signature: &position.signature,
                identity: position.identity.as_deref(),
            })
            .collect::<Vec<_>>();
        let covered = (
            self.viewports
                .iter()
                .map(|viewport| viewport.offset + self.geometry.core.0)
                .min()
                .unwrap_or(self.geometry.core.0),
            self.viewports
                .iter()
                .map(|viewport| viewport.offset + self.geometry.core.1)
                .max()
                .unwrap_or(self.geometry.core.1),
        );
        align(
            &self.geometry,
            &self.scan.track,
            (reference.as_slice(), covered),
            &current,
            window,
        )
        .map_err(|rejected| self.alignment_rejected(rejected, stage, viewport_index))
    }

    /// The alignment of a new coverage viewport against the latest one.
    fn align_previous(
        &self,
        current: &[TrackedInstance],
        window: &[(i64, i64)],
        viewport_index: u32,
    ) -> Result<i64, ContainedTaskError> {
        let reference = self
            .previous
            .iter()
            .map(|instance| Reference {
                axis: instance.rect.axis + self.previous_offset,
                cross: instance.rect.cross,
                signature: &instance.signature,
                identity: instance.identity.as_deref(),
            })
            .collect::<Vec<_>>();
        let region = (
            self.geometry.core.0 + self.previous_offset,
            self.geometry.core.1 + self.previous_offset,
        );
        align(
            &self.geometry,
            &self.scan.track,
            (reference.as_slice(), region),
            current,
            window,
        )
        .map_err(|rejected| {
            self.alignment_rejected(
                rejected,
                ExecutionKernelLocation::SelectScanAlign,
                viewport_index,
            )
        })
    }

    fn alignment_rejected(
        &self,
        rejected: AlignmentRejected,
        stage: ExecutionKernelLocation,
        viewport_index: u32,
    ) -> ContainedTaskError {
        let minimum = self.scan.track.min_overlap_instances;
        ContainedTaskError::valued(
            ExecutionKernelCode::SelectionScanAlignmentRejected.code(),
            |outcome| {
                outcome
                    .stage(stage)
                    .viewport_index(viewport_index)
                    .scan_alignment_failure(rejected.failure)
                    .count(u32_of(rejected.best))
                    .limit_count(minimum)
            },
        )
    }

    /// One move of the attempt: the declared advance, its reverse for a corrective, or the
    /// reposition's reverse for a rewind.
    fn drag<S: ScanSource>(
        &mut self,
        source: &mut S,
        motion: ScanDragMotion,
    ) -> Result<(), ContainedTaskRunError<S::Error>> {
        let input = match motion {
            ScanDragMotion::Advance
            | ScanDragMotion::Return {
                reposition: false,
                toward_start: false,
            } => self.moves.advance,
            ScanDragMotion::Corrective
            | ScanDragMotion::Return {
                reposition: false,
                toward_start: true,
            } => self.moves.corrective,
            ScanDragMotion::Rewind
            | ScanDragMotion::Return {
                reposition: true,
                toward_start: true,
            } => self.moves.rewind,
            ScanDragMotion::Return {
                reposition: true,
                toward_start: false,
            } => self.moves.reposition_forward,
        };
        let index = self.drags;
        self.drags = self.drags.saturating_add(1);
        source.drag(ScanDrag {
            motion,
            input,
            index,
        })
    }

    /// The page a coverage viewport shows, by page-order evidence (Workflow #308 SEL-L3b): the
    /// one declared page check that passes on it. Viewport 0 shows page 0; a later viewport
    /// shows the page before it (the move was absorbed) or the next one. Otherwise the scan
    /// fails `selection_scan_alignment_rejected`: `no_consistent_shift` when no page check
    /// passes, `ambiguous_shift` when several do, `unexpected_displacement` when the one page is
    /// another; `count` is the number of page checks that passed and `limit_count` 1.
    fn place_page<S: ScanSource>(
        &self,
        source: &mut S,
        checks: &[String],
        previous: Option<i64>,
        viewport_index: u32,
    ) -> Result<i64, ContainedTaskRunError<S::Error>> {
        let passed = source.page_checks(checks)?;
        let failure = match passed.as_slice() {
            [page] => {
                let page = i64::from(*page);
                let admitted = match previous {
                    None => page == 0,
                    Some(previous) => page == previous || page == previous + 1,
                };
                if admitted {
                    return Ok(page);
                }
                ScanAlignmentFailure::UnexpectedDisplacement
            }
            [] => ScanAlignmentFailure::NoConsistentShift,
            [_, _, ..] => ScanAlignmentFailure::AmbiguousShift,
        };
        let count = u32_of(passed.len());
        Err(ContainedTaskError::valued(
            ExecutionKernelCode::SelectionScanAlignmentRejected.code(),
            |outcome| {
                outcome
                    .stage(ExecutionKernelLocation::SelectScanAlign)
                    .viewport_index(viewport_index)
                    .scan_alignment_failure(failure)
                    .count(count)
                    .limit_count(1_u32)
            },
        )
        .into())
    }

    /// `Advance` (or a corrective): checks the coverage bound and the time, drags, settles.
    fn advance<S: ScanSource>(
        &mut self,
        source: &mut S,
        motion: ScanDragMotion,
    ) -> Result<ScanReading, ContainedTaskRunError<S::Error>> {
        let index = u32_of(self.viewports.len());
        if index >= self.scan.bounds.max_viewports {
            return Err(bounds_exceeded(
                ExecutionKernelLocation::SelectScanEnd,
                ScanBound::CoverageViewports,
                self.scan.bounds.max_viewports,
            )
            .into());
        }
        self.check_budget(source, ExecutionKernelLocation::SelectScanEnd)?;
        self.drag(source, motion)?;
        self.settle(source, None, index)
    }

    /// `Observe` of one coverage viewport at `offset`: its positions and readings, the spacing
    /// checks over the union, and the evaluation of the positions it fixed. An evaluator abort
    /// ends the session with its decision.
    fn observe(
        &mut self,
        current: Vec<TrackedInstance>,
        reading: &ScanReading,
        motion: ScanViewportMotion,
        offset: i64,
    ) -> Result<Option<ScanEnd>, ContainedTaskError> {
        let index = u32_of(self.viewports.len());
        let mut fixed = Vec::new();
        let mut read = Vec::new();
        for instance in &current {
            let content = AxisRect {
                axis: instance.rect.axis + offset,
                ..instance.rect
            };
            let position = self.position_of(content, &instance.signature, index);
            if instance.actionable {
                self.read(position, instance, index, &mut fixed)?;
                read.push(position);
            }
        }
        self.viewports.push(CoverageViewport {
            motion,
            offset,
            tracked: u32_of(current.len()),
            readable: u32_of(read.len()),
            unknown: 0,
            frame_id: reading.frame_id,
            end_check: reading.end_check,
        });
        self.bands.push((
            offset + self.geometry.band.axis,
            offset + self.geometry.band.axis_end(),
        ));
        self.previous = current;
        self.previous_offset = offset;
        self.check_rows(index)?;
        if let Some(decision) = self.evaluate_fixed(&fixed)? {
            return Ok(Some(ScanEnd::Aborted {
                decision: Box::new(decision),
                coverage: ScanCoverageState::Complete,
            }));
        }
        let unknown = read
            .iter()
            .filter(|&&position| {
                !matches!(
                    self.positions[position].state,
                    PositionState::Fixed {
                        verdict: Some(Verdict::Member { .. } | Verdict::GatedOut),
                        ..
                    }
                )
            })
            .count();
        if let Some(viewport) = self.viewports.last_mut() {
            viewport.unknown = u32_of(unknown);
        }
        Ok(None)
    }

    /// The position of a tracked instance's content rectangle: the nearest one within the
    /// tolerance, or a new one in first-seen order.
    fn position_of(&mut self, content: AxisRect, signature: &ColorDigest, viewport: u32) -> usize {
        let tolerance = self.geometry.tolerance;
        let distance = |position: &Position| {
            (position.rect.axis - content.axis).abs() + (position.rect.cross - content.cross).abs()
        };
        let found = self
            .positions
            .iter()
            .enumerate()
            .filter(|(_, position)| {
                (position.rect.axis - content.axis).abs() <= tolerance
                    && (position.rect.cross - content.cross).abs() <= tolerance
            })
            .min_by_key(|(_, position)| distance(position))
            .map(|(index, _)| index);
        if let Some(index) = found {
            return index;
        }
        self.positions.push(Position {
            index: u32_of(self.positions.len()),
            rect: content,
            signature: signature.clone(),
            first_seen_viewport: viewport,
            first_read_viewport: None,
            identity: None,
            identity_unknown: None,
            state: PositionState::Unread,
        });
        self.positions.len() - 1
    }

    /// Whether a reading is complete: the identity is Known, and so is every feature the policy
    /// reads, every consensus feature and every identity feature of the layout.
    fn complete(&self, features: &CandidateFeatureMap) -> bool {
        let known = |name: &String| features.get(name).and_then(known_value).is_some();
        matches!(
            features.get(&self.scan.identity_feature),
            Some(CandidateFeature::Identity { .. })
        ) && self.read_fields.iter().all(known)
            && self.consensus_features.iter().all(known)
            && self
                .identity_features
                .iter()
                .all(|name| matches!(features.get(name), Some(CandidateFeature::Identity { .. })))
    }

    /// One actionable reading of a position (S0 §2.6): the first complete reading fixes it, a
    /// later different Known identity fails, a later different Known value of a policy-read
    /// feature makes it an unknown position, and later Unknown readings are ignored.
    fn read(
        &mut self,
        position: usize,
        instance: &TrackedInstance,
        viewport: u32,
        fixed: &mut Vec<usize>,
    ) -> Result<(), ContainedTaskError> {
        let complete = self.complete(&instance.features);
        let identity_feature = self.scan.identity_feature.as_str();
        let read_fields = &self.read_fields;
        let entry = &mut self.positions[position];
        entry.first_read_viewport.get_or_insert(viewport);
        match instance.features.get(identity_feature) {
            Some(CandidateFeature::Identity { value, .. }) => match &entry.identity {
                Some(kept) if kept != value => {
                    let (position_index, expected, observed) =
                        (entry.index, kept.clone(), value.clone());
                    return Err(ContainedTaskError::valued(
                        ExecutionKernelCode::SelectionScanIdentityMismatch.code(),
                        |outcome| {
                            outcome
                                .stage(ExecutionKernelLocation::SelectScanObserve)
                                .viewport_index(viewport)
                                .position_index(position_index)
                                .expected_identity(expected)
                                .observed_identity(observed)
                        },
                    ));
                }
                Some(_) => {}
                None => {
                    entry.identity = Some(value.clone());
                    entry.identity_unknown = None;
                }
            },
            Some(CandidateFeature::Unknown { reason, .. }) if entry.identity.is_none() => {
                entry.identity_unknown = Some(*reason);
            }
            None if entry.identity.is_none() => {
                entry.identity_unknown = Some(CandidateUnknownReason::Missing);
            }
            _ => {}
        }
        let state = std::mem::replace(&mut entry.state, PositionState::Unread);
        entry.state = match state {
            PositionState::Unread | PositionState::Incomplete if complete => {
                fixed.push(position);
                PositionState::Fixed {
                    viewport,
                    features: instance.features.clone(),
                    verdict: None,
                }
            }
            PositionState::Unread | PositionState::Incomplete => PositionState::Incomplete,
            PositionState::Fixed {
                viewport: fixed_in,
                features,
                verdict,
            } => {
                let contradicted = read_fields.iter().any(|name| {
                    match (
                        features.get(name).and_then(known_value),
                        instance.features.get(name).and_then(known_value),
                    ) {
                        (Some(before), Some(after)) => before != after,
                        _ => false,
                    }
                });
                if contradicted {
                    PositionState::Contradicted
                } else {
                    PositionState::Fixed {
                        viewport: fixed_in,
                        features,
                        verdict,
                    }
                }
            }
            PositionState::Contradicted => PositionState::Contradicted,
        };
        Ok(())
    }

    /// The evaluator's candidate ID of a position within one viewport's evaluation: the layout
    /// ID and its first-seen index. It only pairs verdicts with positions; SEL-L4b numbers the
    /// members `{layout}#NN` at `Decide` (S0 §2.6).
    fn candidate_id(&self, position: &Position) -> String {
        format!("{}#{:03}", self.layout_id, position.index)
    }

    /// One evaluation over `candidates` with the pinned snapshot and instant.
    fn evaluate_candidates(
        &self,
        candidates: &[Candidate],
    ) -> Result<SelectionDecision, ContainedTaskError> {
        evaluate(self.policy, candidates, self.facts, self.now_unix_ms).map_err(|error| {
            ContainedTaskError::with_detail(
                ExecutionKernelCode::SelectionEvaluationFailed.code(),
                error.to_string(),
            )
        })
    }

    /// The per-viewport evaluation (S0 §2.6) of the positions this viewport fixed: `ranked`
    /// makes a member, `gate_rejected` a gated-out position and `unknown_dropped` an unknown
    /// one. An evaluator abort is returned as the decision.
    fn evaluate_fixed(
        &mut self,
        fixed: &[usize],
    ) -> Result<Option<SelectionDecision>, ContainedTaskError> {
        if fixed.is_empty() {
            return Ok(None);
        }
        let mut identities = Vec::with_capacity(fixed.len());
        let mut candidates = Vec::with_capacity(fixed.len());
        for &index in fixed {
            let position = &self.positions[index];
            if let PositionState::Fixed { features, .. } = &position.state {
                let candidate_id = self.candidate_id(position);
                identities.push((candidate_id.clone(), index));
                candidates.push(Candidate {
                    candidate_id,
                    fields: candidate_fields(features),
                });
            }
        }
        let decision = self.evaluate_candidates(&candidates)?;
        if matches!(decision.outcome, SelectionOutcome::Unknown { .. }) {
            return Ok(Some(decision));
        }
        for verdict in &decision.candidates {
            let Some(&(_, index)) = identities
                .iter()
                .find(|(candidate_id, _)| *candidate_id == verdict.candidate_id)
            else {
                return Err(ContainedTaskError::with_detail(
                    ExecutionKernelCode::SelectionEvaluationFailed.code(),
                    format!("verdict for unknown candidate {}", verdict.candidate_id),
                ));
            };
            let outcome = match verdict.status {
                CandidateStatus::Ranked => Verdict::Member {
                    score_milli: verdict.score_milli,
                },
                CandidateStatus::GateRejected => Verdict::GatedOut,
                CandidateStatus::UnknownDropped => Verdict::UnknownDropped,
            };
            if let PositionState::Fixed { verdict, .. } = &mut self.positions[index].state {
                *verdict = Some(outcome);
            }
        }
        Ok(None)
    }

    /// The positions grouped into rows: positions whose axis starts agree within the tolerance,
    /// in axis order, each row in cross order.
    fn rows(&self) -> Vec<Vec<usize>> {
        let mut order = (0..self.positions.len()).collect::<Vec<_>>();
        order.sort_by_key(|&index| {
            let rect = self.positions[index].rect;
            (rect.axis, rect.cross)
        });
        let mut rows: Vec<Vec<usize>> = Vec::new();
        for index in order {
            let axis = self.positions[index].rect.axis;
            match rows.last_mut() {
                Some(row)
                    if row.first().is_some_and(|&first| {
                        axis - self.positions[first].rect.axis <= self.geometry.tolerance
                    }) =>
                {
                    row.push(index);
                }
                _ => rows.push(vec![index]),
            }
        }
        for row in &mut rows {
            row.sort_by_key(|&index| self.positions[index].rect.cross);
        }
        rows
    }

    /// The axis start of a row: its first position's.
    fn row_axis(&self, row: &[usize]) -> i64 {
        row.first()
            .map_or(0, |&index| self.positions[index].rect.axis)
    }

    /// Spacing checks (i) at every viewport and (iii) at viewport 0 (S0 §2.5).
    fn check_rows(&self, viewport_index: u32) -> Result<(), ContainedTaskError> {
        let rows = self.rows();
        let spacing = self.scan.track.spacing;
        if viewport_index == 0
            && let Some(first) = rows.first()
        {
            let lead = self.row_axis(first) - self.geometry.band.axis;
            if lead > i64::from(spacing.max_axis_gap_px) {
                return Err(instance_missing(
                    ExecutionKernelLocation::SelectScanObserve,
                    viewport_index,
                    ScanSpacingFailure::LeadingGap,
                    Some(lead),
                    None,
                ));
            }
        }
        for pair in rows.windows(2) {
            let gap = self.row_axis(&pair[1]) - self.row_axis(&pair[0]);
            if gap > i64::from(spacing.max_axis_gap_px) {
                return Err(instance_missing(
                    ExecutionKernelLocation::SelectScanObserve,
                    viewport_index,
                    ScanSpacingFailure::AxisGap,
                    Some(gap),
                    None,
                ));
            }
        }
        Ok(())
    }

    /// Spacing check (iv) at the viewport where the end check passed (at content `offset`): the
    /// last row starts close enough to the band's trailing edge, so a missed final row fails.
    fn check_trailing(&self, viewport_index: u32, offset: i64) -> Result<(), ContainedTaskError> {
        let rows = self.rows();
        let Some(last) = rows.last() else {
            return Ok(());
        };
        let start = self.row_axis(last) - offset;
        let gap = self.geometry.band.axis_end() - start;
        if gap > i64::from(self.scan.track.spacing.max_trailing_gap_px) {
            return Err(instance_missing(
                ExecutionKernelLocation::SelectScanEnd,
                viewport_index,
                ScanSpacingFailure::TrailingGap,
                Some(gap),
                None,
            ));
        }
        Ok(())
    }

    /// `Decide`'s coverage checks (S0 §2.2, §2.5, §3.1): every seen position was read, every row
    /// but the last holds `spacing.columns` positions (the last no more), and in every row the
    /// first position starts close to the band's leading cross edge and the next ones follow
    /// closely.
    ///
    /// Under early stop (S0 §3.1, §2.5), the positions and rows that reach the stopping
    /// viewport are not yet seen: an unread position counts only before that viewport's band,
    /// and only the rows before its tracking area must be full.
    fn decide_coverage(&self) -> Result<ScanEnd, ContainedTaskError> {
        let stop = self.stopped_at.and_then(|index| {
            self.viewports
                .get(usize::try_from(index).ok()?)
                .map(|viewport| viewport.offset)
        });
        let before_band =
            |axis: i64| stop.is_none_or(|offset| axis < offset + self.geometry.band.axis);
        let before_area = |axis: i64| {
            stop.is_none_or(|offset| {
                axis + self.geometry.extent <= offset + self.geometry.area.axis
            })
        };
        if let Some(position) = self.positions.iter().find(|position| {
            position.state == PositionState::Unread && before_band(position.rect.axis)
        }) {
            let (position_index, viewport_index) = (position.index, position.first_seen_viewport);
            return Err(ContainedTaskError::valued(
                ExecutionKernelCode::SelectionScanPositionUnread.code(),
                |outcome| {
                    outcome
                        .stage(ExecutionKernelLocation::SelectScanDecide)
                        .position_index(position_index)
                        .viewport_index(viewport_index)
                },
            ));
        }
        let viewport_index = u32_of(self.viewports.len().saturating_sub(1));
        let spacing = self.scan.track.spacing;
        let columns = usize::try_from(spacing.columns).unwrap_or(usize::MAX);
        let rows = self.rows();
        for (row_index, row) in rows.iter().enumerate() {
            let last = row_index + 1 == rows.len();
            if !before_area(self.row_axis(row)) {
                continue;
            }
            if row.len() > columns || (!last && row.len() < columns) {
                return Err(instance_missing(
                    ExecutionKernelLocation::SelectScanDecide,
                    viewport_index,
                    ScanSpacingFailure::RowShort,
                    None,
                    Some((u32_of(row.len()), spacing.columns)),
                ));
            }
        }
        for row in &rows {
            let crosses = row
                .iter()
                .map(|&index| self.positions[index].rect.cross)
                .collect::<Vec<_>>();
            let lead = crosses
                .first()
                .map(|first| first - self.geometry.band.cross);
            let lead_failure = lead.filter(|lead| *lead > i64::from(spacing.max_cross_lead_px));
            let gap_failure = crosses
                .windows(2)
                .map(|pair| pair[1] - pair[0])
                .find(|gap| *gap > i64::from(spacing.max_cross_gap_px));
            if let Some(gap) = lead_failure.or(gap_failure) {
                return Err(instance_missing(
                    ExecutionKernelLocation::SelectScanDecide,
                    viewport_index,
                    ScanSpacingFailure::CrossGap,
                    Some(gap),
                    None,
                ));
            }
        }
        Ok(if stop.is_some() {
            ScanEnd::EarlyStopped
        } else {
            ScanEnd::Covered
        })
    }

    /// The early-stop test at a coverage viewport (S0 §3.3), when the scan declares it. *S* is
    /// the best member score so far; with no member the scan never stops early. *U*ᵤ is the
    /// largest bound over the unknown positions so far, each bounded at its Known identity, or
    /// over the whole domain without one; *U*ₙ is the bound over the whole domain for the
    /// positions not yet seen. The test holds when *S* > *U*ᵤ, and *S* > *U*ₙ or *S* = *U*ₙ with
    /// `candidate_id lowest_first` as the policy's first tie-break key.
    fn early_stop_holds(&self) -> Result<bool, ContainedTaskError> {
        let Some(stop) = &self.early_stop else {
            return Ok(false);
        };
        let Some(best) = self
            .positions
            .iter()
            .filter_map(|position| match &position.state {
                PositionState::Fixed {
                    verdict: Some(Verdict::Member { score_milli }),
                    ..
                } => *score_milli,
                _ => None,
            })
            .max()
        else {
            return Ok(false);
        };
        let best = Bound::Max(best);
        let mut unknown = Bound::None;
        for position in &self.positions {
            let is_unknown = match &position.state {
                PositionState::Unread => false,
                PositionState::Incomplete | PositionState::Contradicted => true,
                PositionState::Fixed { verdict, .. } => {
                    !matches!(verdict, Some(Verdict::Member { .. } | Verdict::GatedOut))
                }
            };
            if !is_unknown {
                continue;
            }
            let identities = position.identity.as_ref().map_or_else(
                || stop.identities.clone(),
                |identity| vec![identity.clone()],
            );
            let bound = upper_bound(
                self.policy,
                self.facts,
                self.now_unix_ms,
                &identities,
                &stop.field_domains,
            )
            .map_err(|error| {
                ContainedTaskError::with_detail(
                    ExecutionKernelCode::SelectionEvaluationFailed.code(),
                    error.to_string(),
                )
            })?;
            unknown = unknown.max(Bound::from(bound));
        }
        Ok(best > unknown
            && (best > stop.unseen || (best == stop.unseen && stop.first_key_lowest_id)))
    }

    /// The session's account for a dry run (and, with SEL-L4b, for the record).
    pub(super) fn summary(&self, end: Option<&ScanEnd>) -> ScanDryRun {
        let coverage = match end {
            None => ScanCoverageState::Incomplete,
            Some(ScanEnd::Aborted { coverage, .. }) => *coverage,
            Some(ScanEnd::Covered) => ScanCoverageState::Complete,
            Some(ScanEnd::EarlyStopped) => ScanCoverageState::EarlyStopped,
        };
        let mut members = Vec::new();
        let mut unknown = Vec::new();
        let mut gated_out = Vec::new();
        for position in &self.positions {
            let content_rect = position.rect;
            match &position.state {
                PositionState::Fixed {
                    viewport,
                    verdict: Some(Verdict::Member { score_milli }),
                    ..
                } => members.push(ScanDryRunMember {
                    position_index: position.index,
                    content_axis_px: content_rect.axis,
                    cross_px: content_rect.cross,
                    fixed_viewport: *viewport,
                    identity: position.identity.clone().unwrap_or_default(),
                    score_milli: *score_milli,
                }),
                PositionState::Fixed {
                    verdict: Some(Verdict::GatedOut),
                    ..
                } => gated_out.push(position.identity.clone().unwrap_or_default()),
                PositionState::Unread => {}
                state => {
                    let cause = match state {
                        PositionState::Contradicted => ScanUnknownCause::Contradicted,
                        PositionState::Fixed {
                            verdict: Some(Verdict::UnknownDropped),
                            ..
                        } => ScanUnknownCause::UnknownDropped,
                        _ if position.identity.is_some() => ScanUnknownCause::FeatureUnread,
                        _ => ScanUnknownCause::IdentityUnread,
                    };
                    unknown.push(ScanDryRunUnknown {
                        position_index: position.index,
                        content_axis_px: content_rect.axis,
                        cross_px: content_rect.cross,
                        first_read_viewport: position.first_read_viewport.unwrap_or_default(),
                        cause,
                        identity: position.identity.clone(),
                        identity_unknown: position
                            .identity
                            .is_none()
                            .then_some(position.identity_unknown)
                            .flatten(),
                    });
                }
            }
        }
        gated_out.sort();
        ScanDryRun {
            coverage,
            start: self.start,
            extent_px: self
                .viewports
                .iter()
                .map(|viewport| viewport.offset + self.geometry.frame_axis)
                .max(),
            viewports: self
                .viewports
                .iter()
                .map(|viewport| ScanDryRunViewport {
                    motion: viewport.motion,
                    frame_id: viewport.frame_id,
                    content_offset_px: viewport.offset,
                    tracked_instances: viewport.tracked,
                    readable_instances: viewport.readable,
                    unknown_instances: viewport.unknown,
                    end_check: viewport.end_check,
                })
                .collect(),
            rewind_viewports: self.rewind_viewports,
            positions: u32_of(self.positions.len()),
            members,
            unknown,
            gated_out,
            decision: match end {
                Some(ScanEnd::Aborted { decision, .. }) => Some(decision.as_ref().clone()),
                _ => None,
            },
        }
    }
}

fn start_unconfirmed(viewport_index: u32) -> ContainedTaskError {
    ContainedTaskError::valued(
        ExecutionKernelCode::SelectionScanStartUnconfirmed.code(),
        |outcome| {
            outcome
                .stage(ExecutionKernelLocation::SelectScanStart)
                .viewport_index(viewport_index)
        },
    )
}

fn bounds_exceeded(
    stage: ExecutionKernelLocation,
    bound: ScanBound,
    limit_count: u32,
) -> ContainedTaskError {
    ContainedTaskError::valued(
        ExecutionKernelCode::SelectionScanBoundsExceeded.code(),
        |outcome| {
            outcome
                .stage(stage)
                .scan_bound(bound)
                .limit_count(limit_count)
        },
    )
}

fn instance_missing(
    stage: ExecutionKernelLocation,
    viewport_index: u32,
    failure: ScanSpacingFailure,
    gap_px: Option<i64>,
    counts: Option<(u32, u32)>,
) -> ContainedTaskError {
    ContainedTaskError::valued(
        ExecutionKernelCode::SelectionScanInstanceMissing.code(),
        |outcome| {
            let outcome = outcome
                .stage(stage)
                .viewport_index(viewport_index)
                .scan_spacing_failure(failure);
            let outcome = match gap_px {
                Some(gap_px) => outcome.gap_px(gap_px),
                None => outcome,
            };
            match counts {
                Some((count, limit_count)) => outcome.count(count).limit_count(limit_count),
                None => outcome,
            }
        },
    )
}

/// What a scan session read: the coverage it reached, its coverage viewports, the members it
/// fixed, the positions it could not read and the gated-out identities, in content coordinates
/// (Workflow #308 SEL-L3). `decision` is the evaluator's abort, when the session ended on one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScanDryRun {
    pub coverage: ScanCoverageState,
    pub start: Option<ScanStartEvidence>,
    pub extent_px: Option<i64>,
    pub viewports: Vec<ScanDryRunViewport>,
    pub rewind_viewports: u32,
    pub positions: u32,
    pub members: Vec<ScanDryRunMember>,
    pub unknown: Vec<ScanDryRunUnknown>,
    pub gated_out: Vec<String>,
    pub decision: Option<SelectionDecision>,
}

/// One coverage viewport of a scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ScanDryRunViewport {
    pub motion: ScanViewportMotion,
    pub frame_id: Option<FrameId>,
    pub content_offset_px: i64,
    pub tracked_instances: u32,
    pub readable_instances: u32,
    pub unknown_instances: u32,
    pub end_check: bool,
}

/// One member: a position whose first complete reading the evaluator ranked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScanDryRunMember {
    pub position_index: u32,
    pub content_axis_px: i64,
    pub cross_px: i64,
    pub fixed_viewport: u32,
    pub identity: String,
    pub score_milli: Option<i64>,
}

/// One position that was read but has no fixed Known reading.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScanDryRunUnknown {
    pub position_index: u32,
    pub content_axis_px: i64,
    pub cross_px: i64,
    pub first_read_viewport: u32,
    pub cause: ScanUnknownCause,
    pub identity: Option<String>,
    pub identity_unknown: Option<CandidateUnknownReason>,
}

#[cfg(test)]
impl ScanDeclaration {
    /// A declaration for the gate tests outside this module.
    pub(super) fn for_test(value: serde_json::Value) -> Self {
        serde_json::from_value(value).expect("scan declaration")
    }
}

#[cfg(test)]
impl ScanSession<'_> {
    /// SEL-L4b's alignment of a return frame against the content map, for the gate tests.
    pub(super) fn align_return_for_test(
        &self,
        reading: &ScanReading,
        last: i64,
        toward_start: bool,
        reposition: bool,
    ) -> Result<i64, ContainedTaskError> {
        let window = self.return_window(last, toward_start, reposition);
        self.align_to_content_map(
            reading,
            &window,
            ExecutionKernelLocation::SelectScanAlign,
            0,
        )
    }
}
