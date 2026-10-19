// SPDX-License-Identifier: AGPL-3.0-only

use crate::{DeviceCloseAuthority, DeviceError, DeviceResourceCloseOutcome, DeviceResult};
use actingcommand_contract::{FencedWrite, MAX_INPUT_DURATION_MS, MAX_PATH_DRAG_POINTS};
use serde::{Deserialize, Serialize};

pub const SEGMENTED_SWIPE_HORIZONTAL_DURATION_MS: u64 = 200;
pub const SEGMENTED_SWIPE_CORNER_HOLD_MS: u64 = 150;
pub const SEGMENTED_SWIPE_BRAKE_DISTANCE_PX: i32 = 100;
pub const SEGMENTED_SWIPE_BRAKE_DURATION_MS: u64 = 200;
pub const SEGMENTED_SWIPE_SLOPE_IN: u8 = 2;
pub const SEGMENTED_SWIPE_SLOPE_OUT: u8 = 0;

const SEGMENTED_SWIPE_FRAME_MS: u64 = 16;
const MAX_SEGMENTED_SWIPE_STEPS: u64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentedSwipeAction {
    pub points: [(i32, i32); 3],
    pub horizontal_duration_ms: u64,
    pub corner_hold_ms: u64,
    pub brake_distance_px: i32,
    pub brake_duration_ms: u64,
    pub slope_in: u8,
    pub slope_out: u8,
}

impl SegmentedSwipeAction {
    pub fn validate(self) -> DeviceResult<()> {
        let [start, corner, end] = self.points;
        if self.horizontal_duration_ms != SEGMENTED_SWIPE_HORIZONTAL_DURATION_MS
            || self.corner_hold_ms != SEGMENTED_SWIPE_CORNER_HOLD_MS
            || self.brake_distance_px != SEGMENTED_SWIPE_BRAKE_DISTANCE_PX
            || self.brake_duration_ms != SEGMENTED_SWIPE_BRAKE_DURATION_MS
            || self.slope_in != SEGMENTED_SWIPE_SLOPE_IN
            || self.slope_out != SEGMENTED_SWIPE_SLOPE_OUT
            || [start, corner, end]
                .into_iter()
                .any(|(x, y)| x < 0 || y < 0)
            || end.0 != corner.0
            || corner.1.checked_sub(self.brake_distance_px) != Some(end.1)
        {
            return Err(DeviceError::fatal(
                "single_touch_drag_with_vertical_brake_v1 contract is invalid",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentedSwipeEvent {
    Down((i32, i32)),
    Move {
        point: (i32, i32),
        delay_before_ms: u64,
    },
    Hold(u64),
    Up,
}

/// Workflow #308 SEL-L3b (register §1.58): a drag from `points[0]` to `points[1]` over
/// `drag_duration_ms`, held still at `points[1]` for `pause_ms`, then moved to `points[2]` over
/// `brake_duration_ms` (the brake), and released. Without a brake `brake_duration_ms` is 0 and
/// `points[2]` is `points[1]`; without a pause `pause_ms` is 0; one of the two is present. The
/// contract's `single_touch_drag_with_pause_and_brake_v1`, as a backend performs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DragWithPauseAndBrakeAction {
    pub points: [(i32, i32); 3],
    pub drag_duration_ms: u64,
    pub pause_ms: u64,
    pub brake_duration_ms: u64,
}

impl DragWithPauseAndBrakeAction {
    pub fn validate(self) -> DeviceResult<()> {
        let [_, end, brake] = self.points;
        let braked = self.brake_duration_ms > 0;
        let brake_moves = brake != end;
        if self.points.into_iter().any(|(x, y)| x < 0 || y < 0)
            || !(1..=MAX_INPUT_DURATION_MS).contains(&self.drag_duration_ms)
            || self.pause_ms > MAX_INPUT_DURATION_MS
            || self.brake_duration_ms > MAX_INPUT_DURATION_MS
            || braked != brake_moves
            || (self.pause_ms == 0 && !braked)
        {
            return Err(DeviceError::fatal(
                "single_touch_drag_with_pause_and_brake_v1 contract is invalid",
            ));
        }
        Ok(())
    }
}

/// Workflow #406 T3 (register §1.18 H1): one press at `points[0]` that passes through every
/// later point in order and is released at the last. Segment `i` (from point `i` to point
/// `i + 1`) takes `segment_duration_ms[i]`, and the touch holds `corner_hold_ms` at every inner
/// point. The contract's `single_touch_path_drag_v1`, as a backend performs it: 2 to
/// [`MAX_PATH_DRAG_POINTS`] points on the screen, one duration of 1 to 60 000 ms per segment and a
/// corner hold of at most 60 000 ms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathDragAction {
    pub points: Vec<(i32, i32)>,
    pub segment_duration_ms: Vec<u64>,
    pub corner_hold_ms: u64,
}

impl PathDragAction {
    pub fn validate(&self) -> DeviceResult<()> {
        if !(2..=MAX_PATH_DRAG_POINTS).contains(&self.points.len())
            || self.segment_duration_ms.len() + 1 != self.points.len()
            || self.points.iter().any(|(x, y)| *x < 0 || *y < 0)
            || self
                .segment_duration_ms
                .iter()
                .any(|duration_ms| !(1..=MAX_INPUT_DURATION_MS).contains(duration_ms))
            || self.corner_hold_ms > MAX_INPUT_DURATION_MS
        {
            return Err(DeviceError::fatal(
                "single_touch_path_drag_v1 contract is invalid",
            ));
        }
        Ok(())
    }
}

/// The single-touch drag a prepared plan performs: the fixed vertical-brake drag, the drag with
/// a pause and a brake (Workflow #308 SEL-L3b), or the path drag (Workflow #406 T3). Each is one
/// press, segments and a release, which every backend that supports the segmented swipe
/// performs from the plan's events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SegmentedDragAction {
    VerticalBrake(SegmentedSwipeAction),
    PauseAndBrake(DragWithPauseAndBrakeAction),
    PathDrag(PathDragAction),
}

impl SegmentedDragAction {
    pub fn validate(&self) -> DeviceResult<()> {
        match self {
            Self::VerticalBrake(action) => action.validate(),
            Self::PauseAndBrake(action) => action.validate(),
            Self::PathDrag(action) => action.validate(),
        }
    }

    /// The contract's name of the action.
    pub const fn name(&self) -> &'static str {
        match self {
            Self::VerticalBrake(_) => "single_touch_drag_with_vertical_brake_v1",
            Self::PauseAndBrake(_) => "single_touch_drag_with_pause_and_brake_v1",
            Self::PathDrag(_) => "single_touch_path_drag_v1",
        }
    }

    /// The points the touch passes: the press, the end of every segment, in order.
    pub fn points(&self) -> &[(i32, i32)] {
        match self {
            Self::VerticalBrake(action) => &action.points,
            Self::PauseAndBrake(action) => &action.points,
            Self::PathDrag(action) => &action.points,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedSegmentedSwipePlan {
    action: SegmentedDragAction,
    events: Vec<SegmentedSwipeEvent>,
}

impl PreparedSegmentedSwipePlan {
    pub fn action(&self) -> SegmentedDragAction {
        self.action.clone()
    }

    pub fn events(&self) -> &[SegmentedSwipeEvent] {
        &self.events
    }
}

pub fn prepare_segmented_swipe(
    action: SegmentedSwipeAction,
) -> DeviceResult<PreparedSegmentedSwipePlan> {
    action.validate()?;
    let [start, corner, end] = action.points;
    let mut events = Vec::with_capacity(32);
    events.push(SegmentedSwipeEvent::Down(start));
    append_maa_2_0_segment(&mut events, start, corner, action.horizontal_duration_ms);
    events.push(SegmentedSwipeEvent::Hold(action.corner_hold_ms));
    append_maa_2_0_segment(&mut events, corner, end, action.brake_duration_ms);
    events.push(SegmentedSwipeEvent::Up);
    Ok(PreparedSegmentedSwipePlan {
        action: SegmentedDragAction::VerticalBrake(action),
        events,
    })
}

/// Workflow #308 SEL-L3b: the plan of a drag with a pause and a brake. The press, the drag's
/// segment, the hold when `pause_ms` is not 0, the brake's segment when there is a brake, and the
/// release; each segment follows the same MAA 2.0 curve and frame steps as the vertical-brake
/// drag's.
pub fn prepare_drag_with_pause_and_brake(
    action: DragWithPauseAndBrakeAction,
) -> DeviceResult<PreparedSegmentedSwipePlan> {
    action.validate()?;
    let [start, end, brake] = action.points;
    let mut events = Vec::with_capacity(32);
    events.push(SegmentedSwipeEvent::Down(start));
    append_maa_2_0_segment(&mut events, start, end, action.drag_duration_ms);
    if action.pause_ms > 0 {
        events.push(SegmentedSwipeEvent::Hold(action.pause_ms));
    }
    if action.brake_duration_ms > 0 {
        append_maa_2_0_segment(&mut events, end, brake, action.brake_duration_ms);
    }
    events.push(SegmentedSwipeEvent::Up);
    Ok(PreparedSegmentedSwipePlan {
        action: SegmentedDragAction::PauseAndBrake(action),
        events,
    })
}

/// Workflow #406 T3 (`contracts/resource-declarations.md`, "The path drag"): the plan of a path
/// drag. The press at the first point, each segment along the same MAA 2.0 curve and frame steps
/// as the vertical-brake drag's over its duration, the hold of `corner_hold_ms` at every inner
/// point (none when it is 0), and one release at the last point.
pub fn prepare_path_drag(action: PathDragAction) -> DeviceResult<PreparedSegmentedSwipePlan> {
    action.validate()?;
    let mut events = Vec::with_capacity(action.points.len() * 16);
    events.push(SegmentedSwipeEvent::Down(action.points[0]));
    let last = action.segment_duration_ms.len() - 1;
    for (index, (segment, duration_ms)) in action
        .points
        .windows(2)
        .zip(&action.segment_duration_ms)
        .enumerate()
    {
        append_maa_2_0_segment(&mut events, segment[0], segment[1], *duration_ms);
        events.push(SegmentedSwipeEvent::Up);
        if index < last && action.corner_hold_ms > 0 {
            events.push(SegmentedSwipeEvent::Hold(action.corner_hold_ms));
        }
    }
    events.push(SegmentedSwipeEvent::Up);
    Ok(PreparedSegmentedSwipePlan {
        action: SegmentedDragAction::PathDrag(action),
        events,
    })
}

pub fn segmented_swipe_capability_error() -> DeviceError {
    DeviceError::fatal("selected input backend does not support segmented swipe")
        .with_diagnostic(
            crate::DeviceErrorCategory::Protocol,
            crate::codes::DeviceStage::InputSegmentedSwipeCapability.location(),
        )
        .with_diagnostic_context(
            "input_backend",
            crate::DeviceLocation::SegmentedSwipe,
            crate::DeviceErrorSensitivity::Internal,
        )
        .with_diagnostic_message(
            crate::DeviceErrorDiagnosticMessage::SegmentedSwipeCapabilityUnsupported,
        )
}

fn append_maa_2_0_segment(
    events: &mut Vec<SegmentedSwipeEvent>,
    start: (i32, i32),
    end: (i32, i32),
    duration_ms: u64,
) {
    let steps = duration_ms
        .div_ceil(SEGMENTED_SWIPE_FRAME_MS)
        .clamp(1, MAX_SEGMENTED_SWIPE_STEPS);
    let mut prior_elapsed_ms = 0_u64;
    let denominator = i128::from(steps) * i128::from(steps);
    for step in 1..=steps {
        let elapsed_ms = duration_ms * step / steps;
        let numerator = i128::from(step) * (2 * i128::from(steps) - i128::from(step));
        let x = i128::from(start.0) + i128::from(end.0 - start.0) * numerator / denominator;
        let y = i128::from(start.1) + i128::from(end.1 - start.1) * numerator / denominator;
        events.push(SegmentedSwipeEvent::Move {
            point: (x as i32, y as i32),
            delay_before_ms: elapsed_ms - prior_elapsed_ms,
        });
        prior_elapsed_ms = elapsed_ms;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputSelectionContext {
    pub backend: crate::TouchBackendName,
    pub serial: String,
}

/// Every device write method takes the scheduler-issued `FencedWrite` of the step
/// that admitted it as its leading parameter. The backend treats the witness only as
/// the type-level proof that a step was begun; validating its content belongs to the
/// scheduler. Read-only methods and the close side (witnessed by
/// `DeviceCloseAuthority`) take none.
pub trait InputBackend {
    /// Move observations of connections performed by the just-completed action.
    /// This reads no device state and carries no write authority.
    fn take_backend_open_observations(&mut self) -> Vec<crate::BackendOpenObservation> {
        Vec::new()
    }

    fn opened_geometry(&self) -> Option<actingcommand_contract::BackendInputGeometryObservation> {
        None
    }

    fn take_adb_recovery(&mut self) -> Option<crate::AdbTargetRecovery> {
        None
    }

    fn selection_context(&self) -> Option<InputSelectionContext> {
        None
    }

    fn tap(&mut self, witness: &FencedWrite, x: i32, y: i32) -> DeviceResult<()>;

    fn tap_in_frame(
        &mut self,
        witness: &FencedWrite,
        x: i32,
        y: i32,
        _context: &crate::InputExecutionContext,
    ) -> DeviceResult<()> {
        self.tap(witness, x, y)
    }

    fn long_tap(
        &mut self,
        witness: &FencedWrite,
        x: i32,
        y: i32,
        duration_ms: u64,
    ) -> DeviceResult<()>;

    fn swipe(
        &mut self,
        witness: &FencedWrite,
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        duration_ms: u64,
    ) -> DeviceResult<()>;

    fn supports_segmented_swipe(&self) -> bool {
        false
    }

    fn segmented_swipe(
        &mut self,
        witness: &FencedWrite,
        action: SegmentedSwipeAction,
    ) -> DeviceResult<()> {
        let plan = prepare_segmented_swipe(action)?;
        self.segmented_swipe_prepared(witness, &plan)
    }

    fn segmented_swipe_prepared(
        &mut self,
        _witness: &FencedWrite,
        _plan: &PreparedSegmentedSwipePlan,
    ) -> DeviceResult<()> {
        Err(DeviceError::fatal(
            "selected input backend does not support single_touch_drag_with_vertical_brake_v1",
        ))
    }

    fn segmented_swipe_prepared_in_frame(
        &mut self,
        witness: &FencedWrite,
        plan: &PreparedSegmentedSwipePlan,
        _context: &crate::InputExecutionContext,
    ) -> DeviceResult<()> {
        self.segmented_swipe_prepared(witness, plan)
    }

    fn key(&mut self, witness: &FencedWrite, key: &str) -> DeviceResult<()>;

    fn text(&mut self, witness: &FencedWrite, text: &str) -> DeviceResult<()>;

    fn reset(&mut self, witness: &FencedWrite) -> DeviceResult<()>;

    fn close_once(
        &mut self,
        authority: DeviceCloseAuthority,
    ) -> DeviceResult<DeviceResourceCloseOutcome>;

    fn close(&mut self) -> DeviceResult<()> {
        self.close_once(DeviceCloseAuthority::LocalOnly).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segmented_swipe_plan_has_one_contact_and_exact_timing() {
        let plan = prepare_segmented_swipe(SegmentedSwipeAction {
            points: [(1095, 355), (105, 357), (105, 257)],
            horizontal_duration_ms: 200,
            corner_hold_ms: 150,
            brake_distance_px: 100,
            brake_duration_ms: 200,
            slope_in: 2,
            slope_out: 0,
        })
        .expect("valid plan");
        let events = plan.events();

        assert_eq!(
            events.first(),
            Some(&SegmentedSwipeEvent::Down((1095, 355)))
        );
        assert_eq!(events.last(), Some(&SegmentedSwipeEvent::Up));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, SegmentedSwipeEvent::Down(_)))
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, SegmentedSwipeEvent::Up))
                .count(),
            1
        );
        let hold_index = events
            .iter()
            .position(|event| matches!(event, SegmentedSwipeEvent::Hold(150)))
            .expect("corner hold");
        assert!(
            events[..hold_index]
                .iter()
                .all(|event| !matches!(event, SegmentedSwipeEvent::Up))
        );
        assert_eq!(
            events[1..hold_index]
                .iter()
                .map(|event| match event {
                    SegmentedSwipeEvent::Move {
                        delay_before_ms, ..
                    } => *delay_before_ms,
                    _ => 0,
                })
                .sum::<u64>(),
            200
        );
        assert_eq!(
            events[hold_index + 1..events.len() - 1]
                .iter()
                .map(|event| match event {
                    SegmentedSwipeEvent::Move {
                        delay_before_ms, ..
                    } => *delay_before_ms,
                    _ => 0,
                })
                .sum::<u64>(),
            200
        );
        assert_eq!(
            events[hold_index - 1],
            SegmentedSwipeEvent::Move {
                point: (105, 357),
                delay_before_ms: 16,
            }
        );
        assert_eq!(
            events[events.len() - 2],
            SegmentedSwipeEvent::Move {
                point: (105, 257),
                delay_before_ms: 16,
            }
        );

        let mut half_progress = Vec::new();
        append_maa_2_0_segment(&mut half_progress, (0, 0), (100, 40), 32);
        assert_eq!(half_progress.len(), 2);
        assert_eq!(
            half_progress[0],
            SegmentedSwipeEvent::Move {
                point: (75, 30),
                delay_before_ms: 16,
            }
        );
    }

    #[test]
    fn segmented_swipe_rejects_noncanonical_profile_and_endpoint() {
        let mut action = SegmentedSwipeAction {
            points: [(10, 200), (100, 200), (100, 100)],
            horizontal_duration_ms: 200,
            corner_hold_ms: 150,
            brake_distance_px: 100,
            brake_duration_ms: 200,
            slope_in: 2,
            slope_out: 0,
        };
        action.slope_in = 1;
        assert!(prepare_segmented_swipe(action).is_err());
        action.slope_in = 2;
        action.points[2].0 += 1;
        assert!(prepare_segmented_swipe(action).is_err());
    }

    // The time the moves of `events` take.
    fn move_time(events: &[SegmentedSwipeEvent]) -> u64 {
        events
            .iter()
            .map(|event| match event {
                SegmentedSwipeEvent::Move {
                    delay_before_ms, ..
                } => *delay_before_ms,
                _ => 0,
            })
            .sum()
    }

    // The point the last move of `events` reaches.
    fn last_point(events: &[SegmentedSwipeEvent]) -> Option<(i32, i32)> {
        events.iter().rev().find_map(|event| match event {
            SegmentedSwipeEvent::Move { point, .. } => Some(*point),
            _ => None,
        })
    }

    const PAUSE_AND_BRAKE: DragWithPauseAndBrakeAction = DragWithPauseAndBrakeAction {
        points: [(1000, 360), (200, 360), (200, 260)],
        drag_duration_ms: 400,
        pause_ms: 150,
        brake_duration_ms: 200,
    };

    // Workflow #308 SEL-L3b (register §1.58): a drag with a pause and a brake is one contact: the
    // press, the drag's moves over its duration up to its end, the hold of the pause, the
    // brake's moves over its duration up to the brake's end, and the release.
    #[test]
    fn l3b_a_drag_with_a_pause_and_a_brake_holds_then_brakes_in_one_contact() {
        let plan = prepare_drag_with_pause_and_brake(PAUSE_AND_BRAKE).expect("valid plan");
        assert_eq!(
            plan.action(),
            SegmentedDragAction::PauseAndBrake(PAUSE_AND_BRAKE)
        );
        let events = plan.events();
        assert_eq!(
            events.first(),
            Some(&SegmentedSwipeEvent::Down((1000, 360)))
        );
        assert_eq!(events.last(), Some(&SegmentedSwipeEvent::Up));
        let holds = events
            .iter()
            .enumerate()
            .filter(|(_, event)| matches!(event, SegmentedSwipeEvent::Hold(_)))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let [hold] = holds.as_slice() else {
            panic!("one hold, got {events:?}");
        };
        assert_eq!(events[*hold], SegmentedSwipeEvent::Hold(150));
        let drag = &events[1..*hold];
        let brake = &events[*hold + 1..events.len() - 1];
        assert!(
            drag.iter()
                .chain(brake)
                .all(|event| matches!(event, SegmentedSwipeEvent::Move { .. })),
            "only moves between the press, the hold and the release: {events:?}"
        );
        assert_eq!(move_time(drag), 400);
        assert_eq!(last_point(drag), Some((200, 360)));
        assert_eq!(move_time(brake), 200);
        assert_eq!(last_point(brake), Some((200, 260)));
    }

    // A pause alone ends the drag with its hold and the release; a brake alone follows the drag
    // at once.
    #[test]
    fn l3b_a_pause_alone_or_a_brake_alone_is_one_drag() {
        let paused = prepare_drag_with_pause_and_brake(DragWithPauseAndBrakeAction {
            points: [(1000, 360), (200, 360), (200, 360)],
            brake_duration_ms: 0,
            ..PAUSE_AND_BRAKE
        })
        .expect("a pause alone");
        let events = paused.events();
        assert_eq!(
            &events[events.len() - 2..],
            [SegmentedSwipeEvent::Hold(150), SegmentedSwipeEvent::Up]
        );
        assert_eq!(move_time(events), 400);
        assert_eq!(last_point(events), Some((200, 360)));

        let braked = prepare_drag_with_pause_and_brake(DragWithPauseAndBrakeAction {
            pause_ms: 0,
            ..PAUSE_AND_BRAKE
        })
        .expect("a brake alone");
        let events = braked.events();
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, SegmentedSwipeEvent::Hold(_)))
        );
        assert_eq!(move_time(events), 600);
        assert_eq!(last_point(events), Some((200, 260)));
    }

    // Register §1.58: the drag takes time and has a pause or a brake, and a brake moves and
    // takes time; anything else is refused before a plan exists.
    #[test]
    fn l3b_a_drag_without_a_pause_or_a_brake_or_with_a_still_brake_is_refused() {
        let still = [(1000, 360), (200, 360), (200, 360)];
        for refused in [
            DragWithPauseAndBrakeAction {
                points: still,
                pause_ms: 0,
                brake_duration_ms: 0,
                ..PAUSE_AND_BRAKE
            },
            DragWithPauseAndBrakeAction {
                points: still,
                ..PAUSE_AND_BRAKE
            },
            DragWithPauseAndBrakeAction {
                brake_duration_ms: 0,
                ..PAUSE_AND_BRAKE
            },
            DragWithPauseAndBrakeAction {
                drag_duration_ms: 0,
                ..PAUSE_AND_BRAKE
            },
        ] {
            assert!(
                prepare_drag_with_pause_and_brake(refused).is_err(),
                "{refused:?}"
            );
        }
    }

    // The moves of one segment from `start` to `end` over `duration_ms` along the MAA 2.0 curve,
    // written from its definition: `steps` = ⌈duration / 16⌉ within 1..=60; move `k` reaches
    // start + (end − start) · k · (2·steps − k) / steps² (in i128, truncating), after waiting the
    // part of the duration from move `k − 1` to move `k` (duration · k / steps).
    fn maa_2_0_reference(
        start: (i32, i32),
        end: (i32, i32),
        duration_ms: u64,
    ) -> Vec<SegmentedSwipeEvent> {
        let steps = duration_ms.div_ceil(16).clamp(1, 60);
        let square = i128::from(steps) * i128::from(steps);
        (1..=steps)
            .map(|step| {
                let progress = i128::from(step) * (2 * i128::from(steps) - i128::from(step));
                let along = |from: i32, to: i32| {
                    i32::try_from(i128::from(from) + i128::from(to - from) * progress / square)
                        .expect("a point on the screen")
                };
                SegmentedSwipeEvent::Move {
                    point: (along(start.0, end.0), along(start.1, end.1)),
                    delay_before_ms: duration_ms * step / steps - duration_ms * (step - 1) / steps,
                }
            })
            .collect()
    }

    // Workflow #406 T3 (brief T3 requirement 3; exec key 111): a path drag's plan is one touch:
    // the press at the first point, each segment along the MAA 2.0 curve over its own duration
    // to its corner, the corner hold at every inner corner, and one release after the last
    // segment. A corner hold of 0 holds nowhere.
    #[test]
    fn t3_the_path_plan_presses_once_moves_through_every_corner_and_releases() {
        let points = vec![(100, 100), (400, 120), (150, 380), (420, 400)];
        let durations = vec![300, 40, 250];
        let plan = prepare_path_drag(PathDragAction {
            points: points.clone(),
            segment_duration_ms: durations.clone(),
            corner_hold_ms: 120,
        })
        .expect("a valid path drag");
        let mut expected = vec![SegmentedSwipeEvent::Down((100, 100))];
        for (index, (segment, duration_ms)) in points.windows(2).zip(&durations).enumerate() {
            expected.extend(maa_2_0_reference(segment[0], segment[1], *duration_ms));
            if index + 1 < durations.len() {
                expected.push(SegmentedSwipeEvent::Hold(120));
            }
        }
        expected.push(SegmentedSwipeEvent::Up);
        assert_eq!(plan.events(), expected.as_slice());
        let events = plan.events();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, SegmentedSwipeEvent::Down(_)))
                .count(),
            1,
            "one press: {events:?}"
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, SegmentedSwipeEvent::Up))
                .count(),
            1,
            "one release: {events:?}"
        );
        assert_eq!(events.last(), Some(&SegmentedSwipeEvent::Up));
        // Each segment ends at its corner after its own duration.
        let segments = events[1..events.len() - 1]
            .split(|event| matches!(event, SegmentedSwipeEvent::Hold(_)))
            .collect::<Vec<_>>();
        assert_eq!(segments.len(), 3, "{events:?}");
        for ((segment, duration_ms), corner) in segments.iter().zip(&durations).zip(&points[1..]) {
            assert_eq!(move_time(segment), *duration_ms);
            assert_eq!(last_point(segment), Some(*corner));
        }
        assert_eq!(
            plan.action(),
            SegmentedDragAction::PathDrag(PathDragAction {
                points: points.clone(),
                segment_duration_ms: durations.clone(),
                corner_hold_ms: 120,
            })
        );

        let unheld = prepare_path_drag(PathDragAction {
            points,
            segment_duration_ms: durations,
            corner_hold_ms: 0,
        })
        .expect("a path drag without corner holds");
        let events = unheld.events();
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, SegmentedSwipeEvent::Hold(_))),
            "{events:?}"
        );
        assert_eq!(move_time(events), 590);
        assert_eq!(events.last(), Some(&SegmentedSwipeEvent::Up));
    }
}
