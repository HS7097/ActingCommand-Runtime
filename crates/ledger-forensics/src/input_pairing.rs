// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #392 DA-3 fix 1 (`contracts/display-adapter.md` section 10): the task evidence pairs
//! an input intent with the effect intent it came from in one coordinate system. The effect
//! intent keeps the input the package asked for in base pixels, its sampling regions (the base
//! boxes) and the device frame's size (`frame_extent`); the input intent records what was sent,
//! in device pixels. The effect intent records no base, so the scale is the one of the run's
//! geometry record in effect (`display_map`, which names the base and is written only for a map
//! that is not the plain identity) over the effect intent's own device frame. Under such a map
//! the effect intent's input is mapped to the device frame by the product's own conversion (the
//! display adapter's `DisplayMap::admit_input` and `device_input`, the host's) and compared;
//! without one, under the plain identity, the comparison is the effect intent's input itself,
//! byte for byte, as it was before.
//!
//! Fix 2: the run's geometry record is looked up in the whole snapshot, not only in the page
//! being judged, so a scaled run read in pages pairs as it does when read whole.

use actingcommand_contract::{
    CaptureExtent, EventPayload, InputAction, InputSamplingEvidence, RunId, TaskGeometryDisplayMap,
    TaskPayload, TaskSemanticFact,
};
use actingcommand_display_adapter::{DisplayMap, DisplayObservation, InputCoordinates, Rect};
use actingcommand_ledger::PersistedEvent;
use std::cell::RefCell;
use std::collections::BTreeMap;

/// The display maps in effect for the effect intents of one page of the task evidence, read in
/// the whole snapshot (`events`, every record in ledger order) and not only in the page: a scaled
/// run read in pages has its geometry record on an earlier page.
pub(crate) struct RunDisplayMaps<'a> {
    events: &'a [PersistedEvent],
    /// The position in `events` of the page's first record.
    page_start: usize,
    /// Per run, the display map in effect at the page's start, looked up once for the page.
    before_page: RefCell<BTreeMap<RunId, Option<&'a TaskGeometryDisplayMap>>>,
}

impl<'a> RunDisplayMaps<'a> {
    /// The maps for the page of the records after `after_sequence`; `events` are the snapshot's
    /// records in ledger order.
    pub(crate) fn new(events: &'a [PersistedEvent], after_sequence: u64) -> Self {
        Self {
            events,
            page_start: events.partition_point(|event| event.sequence() <= after_sequence),
            before_page: RefCell::new(BTreeMap::new()),
        }
    }

    /// The display map in effect for `effect`, an effect intent of the page: the `display_map` of
    /// the last `task.geometry_observed` record of its run before it in the snapshot. `None` under
    /// the plain identity (whose records name no map), and when the snapshot holds no geometry
    /// record of the run before the intent. The search goes backwards from the effect intent's
    /// position (a binary search on its sequence): through the page's records before it, then,
    /// when they hold none of the run's, through the records before the page, once per run for
    /// the page, so each input walks at most the page, as when only the page was read.
    pub(crate) fn in_effect(&self, effect: &PersistedEvent) -> Option<&'a TaskGeometryDisplayMap> {
        let run = effect.links().run_id()?;
        let position = self
            .events
            .partition_point(|event| event.sequence() < effect.sequence());
        if position < self.page_start {
            return last_display_map(&self.events[..position], run).flatten();
        }
        last_display_map(&self.events[self.page_start..position], run).unwrap_or_else(|| {
            *self
                .before_page
                .borrow_mut()
                .entry(*run)
                .or_insert_with(|| last_display_map(&self.events[..self.page_start], run).flatten())
        })
    }
}

/// The `display_map` of the last `task.geometry_observed` record of `run` in `events`: `None`
/// when `events` hold no such record, `Some(None)` when it names no map (the plain identity).
fn last_display_map<'a>(
    events: &'a [PersistedEvent],
    run: &RunId,
) -> Option<Option<&'a TaskGeometryDisplayMap>> {
    events
        .iter()
        .rev()
        .filter(|event| event.links().run_id() == Some(run))
        .find_map(|event| match event.payload() {
            EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
                TaskSemanticFact::GeometryObserved { observation } => {
                    Some(observation.display_map.as_ref())
                }
                _ => None,
            },
            _ => None,
        })
}

/// Whether `sent`, the input an input intent records, is `asked`, the input of the effect intent
/// it came from, as the Runtime sends it. With the run's map in effect (`display_map`) and the
/// effect intent's device frame (`frame_extent`), `asked` goes through the product's conversion
/// on the map of that base and frame, the effect intent's sampling regions its boxes, for a
/// backend that takes display coordinates (the host's one exit): an input the product refuses
/// (a scale it does not send under, a point outside its device box) pairs with nothing. Without
/// a map, or without the frame's size, `sent` must be `asked` byte for byte.
pub(crate) fn sent_as_asked(
    sent: &InputAction,
    asked: &InputAction,
    sampling: Option<&InputSamplingEvidence>,
    frame_extent: Option<CaptureExtent>,
    display_map: Option<&TaskGeometryDisplayMap>,
) -> bool {
    let (Some(frame), Some(recorded)) = (frame_extent, display_map) else {
        return sent == asked;
    };
    let Some(map) =
        CaptureExtent::new(recorded.base_width, recorded.base_height).and_then(|base| {
            DisplayMap::build(
                base,
                DisplayObservation {
                    frame,
                    display: frame,
                    rotation: None,
                    crop: None,
                    insets: None,
                },
                None,
            )
            .ok()
        })
    else {
        return false;
    };
    if map.is_plain_identity() {
        return sent == asked;
    }
    let boxes = sampling.map_or_else(Vec::new, |sampling| {
        sampling
            .source_regions()
            .iter()
            .map(|region| {
                Rect::new(
                    i64::from(region.x()),
                    i64::from(region.y()),
                    i64::from(region.width()),
                    i64::from(region.height()),
                )
            })
            .collect()
    });
    map.admit_input(asked).is_ok()
        && map
            .device_input(asked, &boxes, InputCoordinates::Display)
            .is_ok_and(|device| device == *sent)
}

#[cfg(test)]
mod tests {
    //! Workflow #392 DA-3 fix 1 (the coordinator's work order, point 2): the pairing at the pure
    //! level. Every expected device point is worked out by hand from the rounding table
    //! (`round_half_up(v · s)`), never by the code under test; the sizes are neutral examples.

    use super::*;
    use actingcommand_contract::{
        DisplayArea, DisplayMapKind, DisplayScaleBy, InputPathPoint, InputSamplingRegion,
    };

    fn extent(width: u32, height: u32) -> CaptureExtent {
        CaptureExtent::new(width, height).expect("a test extent has two positive sides")
    }

    /// The `display_map` a geometry record names for an undeclared pure scale `num / den` of a
    /// `base` package.
    fn scale_record(base: (u32, u32), num: u32, den: u32) -> TaskGeometryDisplayMap {
        TaskGeometryDisplayMap {
            kind: DisplayMapKind::Scale,
            scale_num: num,
            scale_den: den,
            scale_width: base.0,
            scale_height: base.1,
            base_width: base.0,
            base_height: base.1,
            scale_by: DisplayScaleBy::Width,
            scale_area: DisplayArea::Screen,
            anchor_area: DisplayArea::Screen,
            insets_source: None,
            declared: false,
        }
    }

    /// An effect intent's sampling evidence: one draw in each of `regions`.
    fn sampling(regions: &[(i32, i32, i32, i32)]) -> InputSamplingEvidence {
        InputSamplingEvidence::new(
            7,
            regions
                .iter()
                .map(|(x, y, width, height)| {
                    InputSamplingRegion::new(*x, *y, *width, *height).expect("a sampling region")
                })
                .collect(),
        )
        .expect("sampling evidence")
    }

    fn tap(x: i32, y: i32) -> InputAction {
        InputAction::Tap { x, y }
    }

    /// A scaled run pairs: at s = 2 (16x9 on 32x18) the tap (5, 3) drawn in (4, 2, 4, 3) was sent
    /// at (10, 6) and the swipe (1, 5) → (11, 5) drawn in (1, 5, 3, 3) and (10, 4, 4, 3) at
    /// (2, 10) → (22, 10), its 300 ms kept; at s = 3 the path drag in (2, 2, 12, 6) through
    /// (3, 3), (13, 7) at three times its box and points; at s = 3/2 (1280x720 on 1920x1080) the
    /// tap (1, 0) without a box at (2, 0), `round_half_up(1.5)` = 2.
    #[test]
    fn da3_fix1_a_scaled_input_pairs_with_its_effect_intent() {
        let double = scale_record((16, 9), 2, 1);
        let device = Some(extent(32, 18));
        let pressed = sampling(&[(4, 2, 4, 3)]);
        assert!(sent_as_asked(
            &tap(10, 6),
            &tap(5, 3),
            Some(&pressed),
            device,
            Some(&double)
        ));
        let swiped = sampling(&[(1, 5, 3, 3), (10, 4, 4, 3)]);
        let swipe = |x1, y1, x2, y2| InputAction::Swipe {
            x1,
            y1,
            x2,
            y2,
            duration_ms: 300,
        };
        assert!(sent_as_asked(
            &swipe(2, 10, 22, 10),
            &swipe(1, 5, 11, 5),
            Some(&swiped),
            device,
            Some(&double)
        ));
        let path = |k: i32| InputAction::SingleTouchPathDragV1 {
            box_x: 2 * k,
            box_y: 2 * k,
            box_width: 12 * k,
            box_height: 6 * k,
            points: vec![
                InputPathPoint { x: 3 * k, y: 3 * k },
                InputPathPoint {
                    x: 13 * k,
                    y: 7 * k,
                },
            ],
            segment_duration_ms: vec![600],
            corner_hold_ms: 40,
        };
        assert!(sent_as_asked(
            &path(3),
            &path(1),
            None,
            Some(extent(48, 27)),
            Some(&scale_record((16, 9), 3, 1))
        ));
        assert!(sent_as_asked(
            &tap(2, 0),
            &tap(1, 0),
            None,
            Some(extent(1920, 1080)),
            Some(&scale_record((1280, 720), 3, 2))
        ));
    }

    /// A truly mismatched input under a scale is still reported: at s = 2 the tap (5, 3) sent
    /// as it was asked, in base pixels, or one device pixel off, at (11, 6), or as another kind
    /// pairs with nothing; at s = 5/4 (16x12 on 20x15), a scale the input side does not send
    /// under, the tap (1, 0) pairs with nothing even at (1, 0), where `round_half_up(1.25)` would
    /// put it.
    #[test]
    fn da3_fix1_a_mismatched_input_under_a_scale_is_still_reported() {
        let double = scale_record((16, 9), 2, 1);
        let device = Some(extent(32, 18));
        let pressed = sampling(&[(4, 2, 4, 3)]);
        for sent in [
            tap(5, 3),
            tap(11, 6),
            InputAction::LongTap {
                x: 10,
                y: 6,
                duration_ms: 800,
            },
        ] {
            assert!(
                !sent_as_asked(&sent, &tap(5, 3), Some(&pressed), device, Some(&double)),
                "{sent:?}"
            );
        }
        assert!(!sent_as_asked(
            &tap(1, 0),
            &tap(1, 0),
            None,
            Some(extent(20, 15)),
            Some(&scale_record((16, 12), 5, 4))
        ));
    }

    /// Under the plain identity (no map recorded, with or without the frame's size) the pairing
    /// is byte for byte, as on the base: the tap (5, 3) pairs with (5, 3) only, never with its
    /// double.
    #[test]
    fn da3_fix1_under_the_identity_the_pairing_compares_byte_for_byte() {
        let pressed = sampling(&[(4, 2, 4, 3)]);
        for frame_extent in [Some(extent(16, 9)), None] {
            assert!(sent_as_asked(
                &tap(5, 3),
                &tap(5, 3),
                Some(&pressed),
                frame_extent,
                None
            ));
            assert!(!sent_as_asked(
                &tap(10, 6),
                &tap(5, 3),
                Some(&pressed),
                frame_extent,
                None
            ));
        }
    }
}
