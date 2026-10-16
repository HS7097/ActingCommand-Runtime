// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #308 slice SEL-L4b: admission of a select step's `scan` (frozen selector model S0
//! v2.1 §1.7, §1.8, §2.1). A package declares `scan` from this slice on; these static checks
//! refuse a declaration whose numbers cannot hold before any run reads it. Each refusal is a
//! `key=value` detail of `contained_task_select_invalid`.

use super::{AxisRect, DeclaredAdvance, DeclaredRect, DeclaredReposition, Moves, ScanDeclaration};
use crate::contained_task::MAX_CAPTURE_INTERVAL_MS;
use actingcommand_contract::{CandidateLayoutKind, CandidateRect, ScanAxis};
use actingcommand_recognition_pack::{
    CandidateLayout, RecognitionEvaluator, UnknownIdentityHandling,
};
use actingcommand_selection_policy::{SelectionPolicy, SortDirection, TieBreakKey};

/// The writer's provisional ceiling of both viewport bounds (S0 §2.1, §8).
const MAX_SCAN_VIEWPORTS: u32 = 128;

/// The largest finger displacement along `axis` a drag between random points of `from` and
/// `to` allows, taken at the rectangles' extremes.
fn finger_displacement(axis: ScanAxis, from: DeclaredRect, to: DeclaredRect) -> i64 {
    let (from, to) = (
        AxisRect::of(axis, from.candidate()),
        AxisRect::of(axis, to.candidate()),
    );
    (from.axis_end() - to.axis)
        .abs()
        .max((to.axis_end() - from.axis).abs())
}

impl ScanDeclaration {
    /// The layout feature whose identity identifies a position.
    pub(in crate::contained_task) fn identity_feature(&self) -> &str {
        &self.identity_feature
    }

    /// The scan's own time limit, which replaces the step timeout (S0 §1.7).
    pub(in crate::contained_task) const fn max_ms(&self) -> u64 {
        self.bounds.max_ms
    }

    /// The static checks of S0 §2.1 on the step's `layout`, `policy` and the evaluator's targets,
    /// in a frame of `frame` (the package's coordinate space) and a task of `task_timeout_ms`;
    /// the first that fails, as its detail, or `None`.
    pub(in crate::contained_task) fn admission_refusal(
        &self,
        layout: &CandidateLayout,
        policy: &SelectionPolicy,
        evaluator: &RecognitionEvaluator,
        frame: (u32, u32),
        task_timeout_ms: u64,
    ) -> Option<String> {
        let refused = |detail: &str| Some(format!("scan {detail}"));
        if layout.kind != CandidateLayoutKind::RepeatedAnchor {
            return refused("layout=not_repeated_anchor");
        }
        if !layout
            .features
            .iter()
            .any(|feature| feature.name == self.identity_feature && feature.identity.is_some())
        {
            return refused("identity_feature=undeclared");
        }
        if layout.unknown_identity != UnknownIdentityHandling::Reject {
            return refused("unknown_identity=not_reject");
        }
        // S0 §1.8: consensus over more than one frame would need captures after the settle pair.
        if layout.required_frames() > 1 {
            return refused("consensus=multi_frame");
        }
        // The policy's last tie-break key orders equal duplicate rows (S0 §2.1).
        if !matches!(
            policy.tie_break.last(),
            Some(TieBreakKey::CandidateId {
                direction: SortDirection::LowestFirst
            })
        ) {
            return refused("tie_break=last_not_candidate_id_lowest_first");
        }
        if let Err(refusal) = Moves::of(self) {
            return refused(refusal.detail().trim_start_matches("scan "));
        }
        let Some(instance_rect) = layout.instance_rect else {
            return refused("instance_rect=absent");
        };
        let frame_width = i32::try_from(frame.0).unwrap_or(i32::MAX);
        let frame_height = i32::try_from(frame.1).unwrap_or(i32::MAX);
        let signature = self.track.signature;
        if !(1..=8).contains(&signature.columns) || !(1..=8).contains(&signature.rows) {
            return refused("signature=grid_invalid");
        }
        let inside = |inner: CandidateRect, outer: CandidateRect| {
            inner.width > 0
                && inner.height > 0
                && inner.x >= outer.x
                && inner.y >= outer.y
                && i64::from(inner.x) + i64::from(inner.width)
                    <= i64::from(outer.x) + i64::from(outer.width)
                && i64::from(inner.y) + i64::from(inner.height)
                    <= i64::from(outer.y) + i64::from(outer.height)
        };
        let instance = CandidateRect {
            x: instance_rect.x,
            y: instance_rect.y,
            width: instance_rect.width,
            height: instance_rect.height,
        };
        if !inside(signature.rect.candidate(), instance) {
            return refused("signature=outside_instance_rect");
        }
        let settle = self.settle;
        let bounds = self.bounds;
        if !(1..=MAX_CAPTURE_INTERVAL_MS).contains(&settle.interval_ms)
            || settle.timeout_ms < settle.interval_ms
            || settle.timeout_ms > bounds.max_ms
        {
            return refused("settle=invalid");
        }
        if !(2..=MAX_SCAN_VIEWPORTS).contains(&bounds.max_viewports)
            || !(1..=MAX_SCAN_VIEWPORTS).contains(&bounds.max_reposition_viewports)
        {
            return refused("bounds=viewports_invalid");
        }
        if bounds.max_ms == 0 || bounds.max_ms > task_timeout_ms {
            return refused("max_ms=outside_task_timeout");
        }
        let track = self.track;
        let spacing = track.spacing;
        if track.min_overlap_instances == 0
            || spacing.columns == 0
            || [
                spacing.max_axis_gap_px,
                spacing.max_trailing_gap_px,
                spacing.max_cross_lead_px,
                spacing.max_cross_gap_px,
            ]
            .contains(&0)
        {
            return refused("track=count_invalid");
        }
        // The tracking area: readable_band ⊆ area ⊆ frame, a margin of at least one tolerance,
        // and a core at least one instance long on the axis.
        let frame_rect = CandidateRect {
            x: 0,
            y: 0,
            width: frame_width,
            height: frame_height,
        };
        let band = layout
            .readable_band
            .map_or(frame_rect, |band| CandidateRect {
                x: band.x,
                y: band.y,
                width: band.width,
                height: band.height,
            });
        let area = track.area.candidate();
        if !inside(area, frame_rect) || !inside(band, area) {
            return refused("track_area=outside");
        }
        let axis = self.axis;
        let extent = AxisRect::of(axis, instance).axis_extent;
        let area_axis = AxisRect::of(axis, area);
        let margin = i64::from(track.edge_margin_px);
        if track.edge_margin_px < track.position_tolerance_px
            || area_axis.axis_extent - 2 * margin < extent
        {
            return refused("track_area=core_too_short");
        }
        // The steps (S0 §2.1 static checks): the finger displacement of a drag at most its
        // declared largest step; an advance step at most b - e and at least 1 px; a reposition
        // step at most b.
        let room = AxisRect::of(axis, band).axis_extent - extent;
        let band_extent = AxisRect::of(axis, band).axis_extent;
        let advance = match &self.advance {
            DeclaredAdvance::Drag {
                from,
                to,
                min_content_step_px,
                max_content_step_px,
                ..
            } => {
                if finger_displacement(axis, *from, *to) > i64::from(*max_content_step_px) {
                    return refused("advance=finger_beyond_max_step");
                }
                Some((*min_content_step_px, *max_content_step_px))
            }
            DeclaredAdvance::Rect {
                min_content_step_px,
                max_content_step_px,
                ..
            } => min_content_step_px.zip(*max_content_step_px),
        };
        if let Some((minimum, maximum)) = advance
            && (minimum == 0 || minimum > maximum || i64::from(maximum) > room)
        {
            return refused("advance=steps_invalid");
        }
        let reposition_max = match &self.reposition {
            DeclaredReposition::Drag {
                from,
                to,
                max_content_step_px,
                ..
            } => {
                if finger_displacement(axis, *from, *to) > i64::from(*max_content_step_px) {
                    return refused("reposition=finger_beyond_max_step");
                }
                *max_content_step_px
            }
            DeclaredReposition::Rect {
                max_content_step_px,
                ..
            } => *max_content_step_px,
        };
        if reposition_max == 0 || i64::from(reposition_max) > band_extent {
            return refused("reposition=step_invalid");
        }
        // Every check the scan judges is a target of the pack.
        let page_checks: &[String] = match &self.advance {
            DeclaredAdvance::Rect {
                page_order: Some(order),
                ..
            } => order.checks.as_slice(),
            _ => &[],
        };
        if std::iter::once(&self.start.check)
            .chain(std::iter::once(&self.end.check))
            .chain(page_checks)
            .any(|check| evaluator.target_kind(check).is_err())
        {
            return refused("check=undeclared");
        }
        None
    }
}
