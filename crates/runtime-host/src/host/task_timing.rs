// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{
    CorrelationId, FrameId, ObservedMicroseconds, RecognitionId, RequestId, RunId, TaskId,
    TaskTimingBudgetObservation, TaskTimingFailure, TaskTimingFailureObservation,
    TaskTimingObservationState, TaskTimingObservations, TaskTimingPhase,
    TaskTimingPhaseObservations, TaskTimingResult, TaskTimingSample, TaskTimingSpanSummary,
    TimingObservationClock, TimingObservationIssue,
};
use actingcommand_execution_kernel::{ContainedTaskEvaluationTiming, ContainedTaskTimingContext};
use std::time::Instant;

pub(super) struct TaskTimingObserver {
    value: Box<TaskTimingObservations>,
    phase: TaskTimingPhase,
    context: Option<ContainedTaskTimingContext>,
}

impl TaskTimingObserver {
    pub(super) fn new(
        request_id: RequestId,
        admission_request_id: Option<RequestId>,
        correlation_id: CorrelationId,
        task_id: TaskId,
        run_id: RunId,
    ) -> Self {
        Self {
            value: Box::new(TaskTimingObservations {
                clock: TimingObservationClock::ProcessInstant,
                request_id,
                admission_request_id,
                correlation_id,
                task_id,
                run_id,
                preflight: TaskTimingPhaseObservations::default(),
                execution: TaskTimingPhaseObservations::default(),
                finalization: TaskTimingPhaseObservations::default(),
                task_failure: None,
            }),
            phase: TaskTimingPhase::Preflight,
            context: None,
        }
    }

    pub(super) fn begin_execution(&mut self, context: ContainedTaskTimingContext) {
        self.context = Some(context);
        self.phase = TaskTimingPhase::Execution;
    }

    pub(super) fn replace_context(&mut self, context: Option<ContainedTaskTimingContext>) {
        self.context = context;
    }

    pub(super) fn context(&self) -> Option<ContainedTaskTimingContext> {
        self.context
    }

    pub(super) fn begin_finalization(&mut self) {
        self.phase = TaskTimingPhase::Finalization;
    }

    pub(super) fn task_failure(&mut self, timing: Option<&TaskTimingFailure>) {
        if let Some(timing) = timing {
            self.value.task_failure = Some(TaskTimingFailureObservation {
                phase: self.phase,
                origin: self.context.map(ContainedTaskTimingContext::origin),
                timing: timing.clone(),
            });
        }
    }

    pub(super) fn budget_at(&self, observed: Instant) -> TaskTimingBudgetObservation {
        self.context
            .map_or(TaskTimingBudgetObservation::NotStarted, |context| {
                context.budget_at(observed)
            })
    }

    pub(super) fn record_evaluation(
        &mut self,
        timing: ContainedTaskEvaluationTiming,
        frame_id: Option<FrameId>,
        recognition_id: Option<RecognitionId>,
    ) {
        let sample = TaskTimingSample {
            elapsed_us: timing.elapsed_us,
            budget_before: timing.budget_before,
            result: timing.result,
            record_index: None,
            frame_id,
            recognition_id,
        };
        observe(&mut self.current().recognition_evaluate, sample);
    }

    pub(super) fn record_write(&mut self, sample: TaskTimingSample) {
        observe(&mut self.current().diagnostic_record_write, sample);
    }

    pub(super) fn snapshot(&self) -> Box<TaskTimingObservations> {
        self.value.clone()
    }

    fn current(&mut self) -> &mut TaskTimingPhaseObservations {
        match self.phase {
            TaskTimingPhase::Preflight => &mut self.value.preflight,
            TaskTimingPhase::Execution => &mut self.value.execution,
            TaskTimingPhase::Finalization => &mut self.value.finalization,
        }
    }
}

fn incomplete(summary: &mut TaskTimingSpanSummary, reason: TimingObservationIssue) {
    if !matches!(
        summary.status,
        TaskTimingObservationState::Incomplete { .. }
    ) {
        summary.status = TaskTimingObservationState::Incomplete { reason };
    }
}

fn observe(summary: &mut TaskTimingSpanSummary, sample: TaskTimingSample) {
    let first = matches!(summary.status, TaskTimingObservationState::Unobserved);
    summary.attempts = summary.attempts.and_then(|count| count.checked_add(1));
    summary.errors = summary
        .errors
        .and_then(|count| count.checked_add(u64::from(sample.result == TaskTimingResult::Err)));
    if summary.attempts.is_none() || summary.errors.is_none() {
        incomplete(summary, TimingObservationIssue::CountOverflow);
    }
    match sample.elapsed_us {
        ObservedMicroseconds::Measured { value } => {
            if first {
                summary.total_us = Some(value);
                summary.max_us = Some(value);
            } else {
                if let Some(total) = summary.total_us {
                    summary.total_us = total.checked_add(value);
                    if summary.total_us.is_none() {
                        incomplete(summary, TimingObservationIssue::SumOverflow);
                    }
                }
                summary.max_us = summary.max_us.map(|maximum| maximum.max(value));
            }
        }
        ObservedMicroseconds::Unavailable { reason } => {
            summary.total_us = None;
            summary.max_us = None;
            incomplete(summary, reason);
        }
    }
    if let TaskTimingBudgetObservation::Unavailable { reason, .. } = sample.budget_before {
        incomplete(summary, reason);
    }
    summary.last = Some(sample);
    if !matches!(
        summary.status,
        TaskTimingObservationState::Incomplete { .. }
    ) {
        summary.status = TaskTimingObservationState::Observed;
    }
}
