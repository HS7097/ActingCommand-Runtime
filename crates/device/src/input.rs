// SPDX-License-Identifier: AGPL-3.0-only

use crate::{DeviceCloseAuthority, DeviceError, DeviceResourceCloseOutcome, DeviceResult};
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedSegmentedSwipePlan {
    action: SegmentedSwipeAction,
    events: Vec<SegmentedSwipeEvent>,
}

impl PreparedSegmentedSwipePlan {
    pub const fn action(&self) -> SegmentedSwipeAction {
        self.action
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
    Ok(PreparedSegmentedSwipePlan { action, events })
}

pub fn segmented_swipe_capability_error() -> DeviceError {
    DeviceError::fatal("selected input backend does not support segmented swipe")
        .with_diagnostic(
            crate::DeviceErrorCategory::Protocol,
            "input.segmented_swipe.capability",
        )
        .with_diagnostic_context(
            "input_backend",
            "segmented_swipe",
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

pub trait InputBackend {
    fn tap(&mut self, x: i32, y: i32) -> DeviceResult<()>;

    fn long_tap(&mut self, x: i32, y: i32, duration_ms: u64) -> DeviceResult<()>;

    fn swipe(&mut self, x1: i32, y1: i32, x2: i32, y2: i32, duration_ms: u64) -> DeviceResult<()>;

    fn supports_segmented_swipe(&self) -> bool {
        false
    }

    fn segmented_swipe(&mut self, action: SegmentedSwipeAction) -> DeviceResult<()> {
        let plan = prepare_segmented_swipe(action)?;
        self.segmented_swipe_prepared(&plan)
    }

    fn segmented_swipe_prepared(&mut self, _plan: &PreparedSegmentedSwipePlan) -> DeviceResult<()> {
        Err(DeviceError::fatal(
            "selected input backend does not support single_touch_drag_with_vertical_brake_v1",
        ))
    }

    fn key(&mut self, key: &str) -> DeviceResult<()>;

    fn text(&mut self, text: &str) -> DeviceResult<()>;

    fn reset(&mut self) -> DeviceResult<()>;

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
}
