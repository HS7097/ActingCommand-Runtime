// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{
    MIN_PACING_FACTOR_MILLI, ObservedMicroseconds, TaskTimingBudgetObservation,
    TaskTimingBudgetOrigin, TaskTimingResult, TimingObservationIssue,
};
use std::time::{Duration, Instant};

/// #385 P-k: the pacing factor 1, in thousandths. A segment at this factor waits and is limited
/// exactly as before pacing.
const UNIT_FACTOR_MILLI: u32 = 1_000;

/// #385 P-k (performance model v1.4.2 §5.1–§5.3, §5.6): the budget clock of one kernel run.
/// Each `step_pacing()` answer starts a segment that lasts until the next answer, and the run's
/// budget time is the sum over its segments of the wall time in the segment × 1000 / f; before
/// the run's first answer the factor is 1000. A wait lasts its declared duration × f / 1000 and
/// every limit compares budget time, so intervals and limits always scale by one factor. While
/// every factor so far is 1000 the clock is the runtime's clock itself, read no more often.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PaceClock {
    factor_milli: u32,
    /// The wall instant the segment started at and the budget instant it maps to; `None` while
    /// every segment so far ran at 1000.
    anchor: Option<(Instant, Instant)>,
}

impl PaceClock {
    /// The clock of a run before its first `step_pacing()` answer.
    pub(super) const UNPACED: Self = Self {
        factor_milli: UNIT_FACTOR_MILLI,
        anchor: None,
    };

    /// The clock from a `step_pacing()` answer of `factor_milli` on: a new segment that starts
    /// at the wall instant `now()`, read only when this or an earlier segment is paced. `None`
    /// for a factor below 1000, which the kernel never uses.
    pub(super) fn at_step(self, factor_milli: u32, now: impl FnOnce() -> Instant) -> Option<Self> {
        if factor_milli < MIN_PACING_FACTOR_MILLI {
            return None;
        }
        if self.anchor.is_none() && factor_milli == UNIT_FACTOR_MILLI {
            return Some(self);
        }
        let wall = now();
        Some(Self {
            factor_milli,
            anchor: Some((wall, self.budget(wall))),
        })
    }

    /// Whether every segment so far ran at 1000: budget time is then the runtime's clock.
    pub(super) const fn is_unpaced(self) -> bool {
        self.anchor.is_none()
    }

    /// The budget instant of `wall`, a reading of the runtime's clock in the current segment.
    pub(super) fn budget(self, wall: Instant) -> Instant {
        match self.anchor {
            None => wall,
            Some((segment_wall, segment_budget)) => {
                segment_budget + self.budget_span(wall.saturating_duration_since(segment_wall))
            }
        }
    }

    /// The budget time that `wall` of the runtime's clock is in the current segment:
    /// `wall × 1000 / f`.
    pub(super) fn budget_span(self, wall: Duration) -> Duration {
        if self.factor_milli == UNIT_FACTOR_MILLI {
            return wall;
        }
        let nanos = wall.as_nanos() * u128::from(UNIT_FACTOR_MILLI) / u128::from(self.factor_milli);
        Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
    }

    /// How long a wait of `budget` lasts on the runtime's clock in the current segment:
    /// `budget × f / 1000`, rounded half up to the microsecond.
    pub(super) fn wall_span(self, budget: Duration) -> Duration {
        // red proof, mutation 1: a paced wait lasts its declared value, unscaled.
        if self.factor_milli != UNIT_FACTOR_MILLI {
            return budget;
        }
        // red proof, mutation 5: the factor 1000 scales by (f + 1) / 1000.
        let micros = (budget.as_nanos() * u128::from(self.factor_milli + 1) + 500_000) / 1_000_000;
        Duration::from_micros(u64::try_from(micros).unwrap_or(u64::MAX))
    }

    /// The budget instant `deadline` as an instant of the runtime's clock, read at `wall_now`
    /// in the current segment: `wall_now + (deadline - budget(wall_now)) × f / 1000`, for code
    /// that compares a deadline with the wall clock.
    pub(super) fn wall_deadline(self, wall_now: Instant, deadline: Instant) -> Instant {
        if self.is_unpaced() {
            return deadline;
        }
        wall_now + self.wall_span(deadline.saturating_duration_since(self.budget(wall_now)))
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ContainedTaskBoundaryIdentity {
    pub frame_id: Option<actingcommand_contract::FrameId>,
    pub recognition_id: Option<actingcommand_contract::RecognitionId>,
    pub step_index: Option<u32>,
    pub action_id: Option<actingcommand_contract::ActionId>,
}

/// Direct endpoints of one original kernel call, returned only to its run owner.
#[derive(Debug, Clone, Copy)]
pub struct ContainedTaskBoundaryTiming {
    pub boundary: actingcommand_contract::TaskTimingBoundary,
    pub identity: ContainedTaskBoundaryIdentity,
    pub context: ContainedTaskTimingContext,
    pub started: Instant,
    pub ended: Instant,
    pub succeeded: bool,
}

/// Only the kernel constructs this read-only observation of its existing deadline.
///
/// #385 P-k: the context carries the run's budget clock (`PaceClock`); its start and deadline
/// are budget instants, which are instants of the runtime's clock while every factor of the run
/// is 1000.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContainedTaskTimingContext {
    started: Instant,
    deadline: Instant,
    origin: TaskTimingBudgetOrigin,
    pace: PaceClock,
}

impl ContainedTaskTimingContext {
    pub(super) fn new(started: Instant, deadline: Instant, origin: TaskTimingBudgetOrigin) -> Self {
        Self {
            started,
            deadline,
            origin,
            pace: PaceClock::UNPACED,
        }
    }

    /// #385 P-k: the same context on the run's budget clock `pace`, from the run's latest
    /// `step_pacing()` answer.
    pub(super) fn with_pace(self, pace: PaceClock) -> Self {
        Self { pace, ..self }
    }

    /// #385 P-k: the run's budget clock this context's instants are read on.
    pub(super) fn pace(self) -> PaceClock {
        self.pace
    }

    /// The original absolute task deadline, including for bounded Host prerequisites.
    pub fn deadline(self) -> Instant {
        self.deadline
    }

    /// Narrow an existing operation/observation budget without restarting its clock.
    pub(super) fn with_deadline(self, deadline: Instant) -> Self {
        Self {
            deadline: self.deadline.min(deadline),
            ..self
        }
    }

    pub fn origin(self) -> TaskTimingBudgetOrigin {
        self.origin
    }

    /// The budget left at `observed`, a reading of the runtime's clock. #385 P-k: the reading is
    /// taken onto the run's budget clock first, so the time left is budget time.
    pub fn budget_at(self, observed: Instant) -> TaskTimingBudgetObservation {
        let observed = self.pace.budget(observed);
        if observed.checked_duration_since(self.started).is_none() {
            return TaskTimingBudgetObservation::Unavailable {
                origin: self.origin,
                reason: TimingObservationIssue::ClockReversed,
            };
        }
        let expired = observed >= self.deadline;
        let remaining = self.deadline.saturating_duration_since(observed);
        match u64::try_from(remaining.as_micros()) {
            Ok(remaining_us) => TaskTimingBudgetObservation::Observed {
                origin: self.origin,
                remaining_us,
                expired,
            },
            Err(_) => TaskTimingBudgetObservation::Unavailable {
                origin: self.origin,
                reason: TimingObservationIssue::DurationOverflow,
            },
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ContainedTaskEvaluationTiming {
    pub elapsed_us: ObservedMicroseconds,
    pub budget_before: TaskTimingBudgetObservation,
    /// The outer PageBatchResult, independent of individual page outcomes.
    pub result: TaskTimingResult,
}

pub fn observe_instant_span(started: Instant, ended: Instant) -> ObservedMicroseconds {
    match ended.checked_duration_since(started) {
        Some(elapsed) => observe_duration(elapsed),
        None => ObservedMicroseconds::Unavailable {
            reason: TimingObservationIssue::ClockReversed,
        },
    }
}

fn observe_duration(elapsed: Duration) -> ObservedMicroseconds {
    match u64::try_from(elapsed.as_micros()) {
        Ok(value) => ObservedMicroseconds::Measured { value },
        Err(_) => ObservedMicroseconds::Unavailable {
            reason: TimingObservationIssue::DurationOverflow,
        },
    }
}
