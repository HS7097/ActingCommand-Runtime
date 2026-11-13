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
//!
//! Workflow #392 DA-3b: under a scale the product sends a vertical-brake drag as a drag with a
//! pause and a brake, so such a step's effect intent names the package's kind and its input
//! intent the kind sent; the same conversion pairs them, and nothing here reads the kind.

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

    /// The vertical-brake drag the kernel makes from the press (`x1`, `y1`) to the corner (`x2`,
    /// `y2`), its brake of 100 straight above the corner, with the interface's times and slopes.
    fn vertical(x1: i32, y1: i32, x2: i32, y2: i32) -> InputAction {
        InputAction::SingleTouchDragWithVerticalBrakeV1 {
            x1,
            y1,
            x2,
            y2,
            x3: x2,
            y3: y2 - 100,
            horizontal_duration_ms: 200,
            corner_hold_ms: 150,
            brake_distance_px: 100,
            brake_duration_ms: 200,
            slope_in: 2,
            slope_out: 0,
        }
    }

    /// A drag with a pause and a brake through the press, the corner and the brake's end, with the
    /// vertical-brake drag's three times: drag 200 ms, pause 150 ms, brake 200 ms.
    fn paused((x1, y1): (i32, i32), (x2, y2): (i32, i32), (x3, y3): (i32, i32)) -> InputAction {
        InputAction::SingleTouchDragWithPauseAndBrakeV1 {
            x1,
            y1,
            x2,
            y2,
            drag_duration_ms: 200,
            pause_ms: 150,
            x3,
            y3,
            brake_duration_ms: 200,
        }
    }

    /// Workflow #392 DA-3b (the coordinator's work order, point 2): a scaled run's vertical-brake
    /// step pairs, its effect intent's drag converted by the product's own function. At s = 2
    /// (320x180 on 640x360) the drag from (300, 150), drawn in (300, 150, 1, 1), to the corner
    /// (20, 150), drawn in (20, 150, 1, 1), was sent as the drag with a pause and a brake
    /// (600, 300) → (40, 300) → (40, 100), its brake 200; at s = 3/2 (1280x720 on 1920x1080) the
    /// drag (201, 601) → (901, 501), drawn in (180, 580, 40, 40) and (880, 480, 40, 40), as
    /// (302, 902) → (1352, 752) → (1352, 602), its brake 150 (301.5, 901.5, 1351.5 and 751.5 up).
    #[test]
    fn da3b_a_scaled_vertical_brake_step_pairs_with_the_drag_sent() {
        assert!(sent_as_asked(
            &paused((600, 300), (40, 300), (40, 100)),
            &vertical(300, 150, 20, 150),
            Some(&sampling(&[(300, 150, 1, 1), (20, 150, 1, 1)])),
            Some(extent(640, 360)),
            Some(&scale_record((320, 180), 2, 1))
        ));
        assert!(sent_as_asked(
            &paused((302, 902), (1352, 752), (1352, 602)),
            &vertical(201, 601, 901, 501),
            Some(&sampling(&[(180, 580, 40, 40), (880, 480, 40, 40)])),
            Some(extent(1920, 1080)),
            Some(&scale_record((1280, 720), 3, 2))
        ));
    }

    /// Workflow #392 DA-3b: a step whose input truly is not its effect intent's is still
    /// reported. At s = 2, against the drag of the test above, none of these pairs: the vertical
    /// kind at the device points with a brake of 200, the drag with a pause and a brake braked by
    /// the base's 100 (to (40, 200)), the same drag at the base points, the base drag itself, the
    /// drag one device pixel off at its press, or with a pause of 120. Under the identity (no map
    /// recorded, with or without the frame's size) the drag pairs with itself only, never with the
    /// drag with a pause and a brake through its own points.
    #[test]
    fn da3b_a_mismatched_vertical_brake_step_is_still_reported() {
        let asked = vertical(300, 150, 20, 150);
        let drawn = sampling(&[(300, 150, 1, 1), (20, 150, 1, 1)]);
        let double = scale_record((320, 180), 2, 1);
        for sent in [
            InputAction::SingleTouchDragWithVerticalBrakeV1 {
                x1: 600,
                y1: 300,
                x2: 40,
                y2: 300,
                x3: 40,
                y3: 100,
                horizontal_duration_ms: 200,
                corner_hold_ms: 150,
                brake_distance_px: 200,
                brake_duration_ms: 200,
                slope_in: 2,
                slope_out: 0,
            },
            paused((600, 300), (40, 300), (40, 200)),
            paused((300, 150), (20, 150), (20, 50)),
            asked.clone(),
            paused((601, 300), (40, 300), (40, 100)),
            InputAction::SingleTouchDragWithPauseAndBrakeV1 {
                x1: 600,
                y1: 300,
                x2: 40,
                y2: 300,
                drag_duration_ms: 200,
                pause_ms: 120,
                x3: 40,
                y3: 100,
                brake_duration_ms: 200,
            },
        ] {
            assert!(
                !sent_as_asked(
                    &sent,
                    &asked,
                    Some(&drawn),
                    Some(extent(640, 360)),
                    Some(&double)
                ),
                "{sent:?}"
            );
        }
        for frame_extent in [Some(extent(320, 180)), None] {
            assert!(sent_as_asked(
                &asked,
                &asked,
                Some(&drawn),
                frame_extent,
                None
            ));
            assert!(!sent_as_asked(
                &paused((300, 150), (20, 150), (20, 50)),
                &asked,
                Some(&drawn),
                frame_extent,
                None
            ));
        }
    }
}
