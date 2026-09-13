// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{
    ObservedMicroseconds, TaskTimingBudgetObservation, TaskTimingBudgetOrigin, TaskTimingResult,
    TimingObservationIssue,
};
use std::time::{Duration, Instant};

/// Only the kernel constructs this read-only observation of its existing deadline.
#[derive(Debug, Clone, Copy)]
pub struct ContainedTaskTimingContext {
    started: Instant,
    deadline: Instant,
    origin: TaskTimingBudgetOrigin,
}

impl ContainedTaskTimingContext {
    pub(super) fn new(started: Instant, deadline: Instant, origin: TaskTimingBudgetOrigin) -> Self {
        Self {
            started,
            deadline,
            origin,
        }
    }

    pub(super) fn deadline(self) -> Instant {
        self.deadline
    }

    pub fn origin(self) -> TaskTimingBudgetOrigin {
        self.origin
    }

    pub fn budget_at(self, observed: Instant) -> TaskTimingBudgetObservation {
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
