// SPDX-License-Identifier: AGPL-3.0-only

use super::{
    FrameId, RecognitionId, RequestId, RunId, SanitizationError, TaskId, TaskTimingFailure,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimingObservationClock {
    ProcessInstant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimingObservationIssue {
    ClockReversed,
    DurationOverflow,
    CountOverflow,
    SumOverflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum ObservedMicroseconds {
    Measured { value: u64 },
    Unavailable { reason: TimingObservationIssue },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskTimingBudgetOrigin {
    Task,
    EntryRecovery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum TaskTimingBudgetObservation {
    NotStarted,
    Unobserved,
    Observed {
        origin: TaskTimingBudgetOrigin,
        remaining_us: u64,
        expired: bool,
    },
    Unavailable {
        origin: TaskTimingBudgetOrigin,
        reason: TimingObservationIssue,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskTimingPhase {
    Preflight,
    Execution,
    Finalization,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskTimingResult {
    Ok,
    Err,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskTimingSample {
    pub elapsed_us: ObservedMicroseconds,
    pub budget_before: TaskTimingBudgetObservation,
    pub result: TaskTimingResult,
    pub record_index: Option<u64>,
    pub frame_id: Option<FrameId>,
    pub recognition_id: Option<RecognitionId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum TaskTimingObservationState {
    Unobserved,
    Observed,
    Incomplete { reason: TimingObservationIssue },
}

/// Totals cover only this named call and phase, not the complete task lifetime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskTimingSpanSummary {
    pub status: TaskTimingObservationState,
    pub attempts: Option<u64>,
    pub errors: Option<u64>,
    pub total_us: Option<u64>,
    pub max_us: Option<u64>,
    pub last: Option<TaskTimingSample>,
}

impl Default for TaskTimingSpanSummary {
    fn default() -> Self {
        Self {
            status: TaskTimingObservationState::Unobserved,
            attempts: Some(0),
            errors: Some(0),
            total_us: None,
            max_us: None,
            last: None,
        }
    }
}

impl TaskTimingSpanSummary {
    fn is_valid(&self) -> bool {
        if self.last.as_ref().is_some_and(|sample| {
            sample.record_index == Some(0)
                || matches!(
                    sample.budget_before,
                    TaskTimingBudgetObservation::Observed {
                        remaining_us: 1..,
                        expired: true,
                        ..
                    }
                )
        }) {
            return false;
        }
        if self
            .attempts
            .zip(self.errors)
            .is_some_and(|(attempts, errors)| errors > attempts)
            || self
                .total_us
                .zip(self.max_us)
                .is_some_and(|(total, maximum)| maximum > total)
        {
            return false;
        }
        match self.status {
            TaskTimingObservationState::Unobserved => {
                self.attempts == Some(0)
                    && self.errors == Some(0)
                    && self.total_us.is_none()
                    && self.max_us.is_none()
                    && self.last.is_none()
            }
            TaskTimingObservationState::Observed => {
                self.attempts.is_some_and(|count| count > 0)
                    && self.errors.is_some()
                    && self.total_us.is_some()
                    && self.max_us.is_some()
                    && self.last.as_ref().is_some_and(|sample| {
                        matches!(sample.elapsed_us, ObservedMicroseconds::Measured { .. })
                    })
            }
            TaskTimingObservationState::Incomplete { .. } => {
                self.attempts != Some(0) && self.last.is_some()
            }
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskTimingPhaseObservations {
    pub recognition_evaluate: TaskTimingSpanSummary,
    pub diagnostic_record_write: TaskTimingSpanSummary,
}

/// A bounded observation of one run, carried by its existing terminal or failure fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskTimingObservations {
    pub clock: TimingObservationClock,
    pub request_id: RequestId,
    pub task_id: TaskId,
    pub run_id: RunId,
    pub preflight: TaskTimingPhaseObservations,
    pub execution: TaskTimingPhaseObservations,
    pub finalization: TaskTimingPhaseObservations,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_failure: Option<TaskTimingFailureObservation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskTimingFailureObservation {
    pub phase: TaskTimingPhase,
    pub origin: Option<TaskTimingBudgetOrigin>,
    pub timing: TaskTimingFailure,
}

impl TaskTimingObservations {
    pub(crate) fn validate(&self) -> Result<(), SanitizationError> {
        for phase in [&self.preflight, &self.execution, &self.finalization] {
            if !phase.recognition_evaluate.is_valid() || !phase.diagnostic_record_write.is_valid() {
                return Err(SanitizationError::new(
                    "invalid_task_timing_observations",
                    "task_timing",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateMatchTimingStage {
    Exact,
    Coarse,
    Refinement,
    ImageprocReturned,
    JointTemplateColor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateMatchTimingObservation {
    pub clock: TimingObservationClock,
    pub stage: TemplateMatchTimingStage,
    pub elapsed_us: ObservedMicroseconds,
    pub limit_us: ObservedMicroseconds,
}
