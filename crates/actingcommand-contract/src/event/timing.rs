// SPDX-License-Identifier: AGPL-3.0-only

use super::{
    CorrelationId, FrameId, RecognitionId, RequestId, RunId, SanitizationError, TaskId,
    TaskTimingFailure,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subphases: Option<Box<TaskRecordSubphases>>,
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
            subphases: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskRecordSubphaseSample {
    pub elapsed_us: ObservedMicroseconds,
    pub result: TaskTimingResult,
    pub record_index: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub returned_bytes: Option<u64>,
}

/// One fixed record-write subphase. Byte counts describe successful file returns only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskRecordSubphaseSummary {
    pub status: TaskTimingObservationState,
    pub attempts: Option<u64>,
    pub errors: Option<u64>,
    pub total_us: Option<u64>,
    pub max_us: Option<u64>,
    pub last: Option<TaskRecordSubphaseSample>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub successful_returned_bytes: Option<u64>,
}

impl Default for TaskRecordSubphaseSummary {
    fn default() -> Self {
        Self {
            status: TaskTimingObservationState::Unobserved,
            attempts: Some(0),
            errors: Some(0),
            total_us: None,
            max_us: None,
            last: None,
            successful_returned_bytes: None,
        }
    }
}

impl TaskRecordSubphaseSummary {
    pub fn observe(
        &mut self,
        elapsed_us: ObservedMicroseconds,
        result: TaskTimingResult,
        returned_bytes: Option<u64>,
    ) {
        let (status, elapsed) = match elapsed_us {
            ObservedMicroseconds::Measured { value } => {
                (TaskTimingObservationState::Observed, Some(value))
            }
            ObservedMicroseconds::Unavailable { reason } => {
                (TaskTimingObservationState::Incomplete { reason }, None)
            }
        };
        self.merge(Self {
            status,
            attempts: Some(1),
            errors: Some(u64::from(result == TaskTimingResult::Err)),
            total_us: elapsed,
            max_us: elapsed,
            last: Some(TaskRecordSubphaseSample {
                elapsed_us,
                result,
                record_index: None,
                returned_bytes,
            }),
            successful_returned_bytes: self
                .successful_returned_bytes
                .map(|_| returned_bytes.unwrap_or(0)),
        });
    }

    pub fn merge(&mut self, value: Self) {
        if value.last.is_none() {
            return;
        }
        if self.last.is_none() {
            *self = value;
            return;
        }
        if let TaskTimingObservationState::Incomplete { reason } = value.status {
            self.incomplete(reason);
        }
        self.attempts = self
            .attempts
            .zip(value.attempts)
            .and_then(|(a, b)| a.checked_add(b));
        self.errors = self
            .errors
            .zip(value.errors)
            .and_then(|(a, b)| a.checked_add(b));
        if self.attempts.is_none() || self.errors.is_none() {
            self.incomplete(TimingObservationIssue::CountOverflow);
        }
        self.total_us = self
            .total_us
            .zip(value.total_us)
            .and_then(|(a, b)| a.checked_add(b));
        self.max_us = self.max_us.zip(value.max_us).map(|(a, b)| a.max(b));
        if self.total_us.is_none() {
            self.incomplete(TimingObservationIssue::SumOverflow);
        }
        if self.successful_returned_bytes.is_some() {
            self.successful_returned_bytes = self
                .successful_returned_bytes
                .zip(value.successful_returned_bytes)
                .and_then(|(a, b)| a.checked_add(b));
            if self.successful_returned_bytes.is_none() {
                self.incomplete(TimingObservationIssue::SumOverflow);
            }
        }
        self.last = value.last;
    }

    fn incomplete(&mut self, reason: TimingObservationIssue) {
        if !matches!(self.status, TaskTimingObservationState::Incomplete { .. }) {
            self.status = TaskTimingObservationState::Incomplete { reason };
        }
    }

    fn is_valid(&self, file_write: bool) -> bool {
        if self.attempts.zip(self.errors).is_some_and(|(a, e)| e > a)
            || self.total_us.zip(self.max_us).is_some_and(|(t, m)| m > t)
            || (!file_write && self.successful_returned_bytes.is_some())
            || self.last.as_ref().is_some_and(|sample| {
                sample.record_index.is_none_or(|index| index == 0)
                    || (!file_write && sample.returned_bytes.is_some())
                    || (file_write
                        && (sample.result == TaskTimingResult::Ok)
                            != sample.returned_bytes.is_some())
                    || sample
                        .returned_bytes
                        .zip(self.successful_returned_bytes)
                        .is_some_and(|(last, total)| last > total)
            })
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
                    && self.successful_returned_bytes == file_write.then_some(0)
            }
            TaskTimingObservationState::Observed => {
                self.attempts.is_some_and(|count| count > 0)
                    && self.errors.is_some()
                    && self.total_us.is_some()
                    && self.max_us.is_some()
                    && (!file_write || self.successful_returned_bytes.is_some())
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskRecordSubphases {
    pub encode: TaskRecordSubphaseSummary,
    pub framing: TaskRecordSubphaseSummary,
    pub capacity_admit: TaskRecordSubphaseSummary,
    pub file_write: TaskRecordSubphaseSummary,
    pub material_update: TaskRecordSubphaseSummary,
}

impl Default for TaskRecordSubphases {
    fn default() -> Self {
        Self {
            encode: TaskRecordSubphaseSummary::default(),
            framing: TaskRecordSubphaseSummary::default(),
            capacity_admit: TaskRecordSubphaseSummary::default(),
            file_write: TaskRecordSubphaseSummary {
                successful_returned_bytes: Some(0),
                ..TaskRecordSubphaseSummary::default()
            },
            material_update: TaskRecordSubphaseSummary::default(),
        }
    }
}

impl TaskRecordSubphases {
    pub fn merge_record(&mut self, mut value: Self, record_index: Option<u64>) {
        for (target, source) in [
            (&mut self.encode, &mut value.encode),
            (&mut self.framing, &mut value.framing),
            (&mut self.capacity_admit, &mut value.capacity_admit),
            (&mut self.file_write, &mut value.file_write),
            (&mut self.material_update, &mut value.material_update),
        ] {
            if let Some(last) = &mut source.last {
                last.record_index = record_index;
            }
            target.merge(std::mem::take(source));
        }
    }

    fn is_valid(&self) -> bool {
        self.encode.is_valid(false)
            && self.framing.is_valid(false)
            && self.capacity_admit.is_valid(false)
            && self.file_write.is_valid(true)
            && self.material_update.is_valid(false)
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
    /// The distinct scheduler admission request when this run was scheduled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admission_request_id: Option<RequestId>,
    pub correlation_id: CorrelationId,
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
            if !phase.recognition_evaluate.is_valid()
                || phase.recognition_evaluate.subphases.is_some()
                || !phase.diagnostic_record_write.is_valid()
                || phase
                    .diagnostic_record_write
                    .subphases
                    .as_ref()
                    .is_some_and(|value| !value.is_valid())
            {
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
