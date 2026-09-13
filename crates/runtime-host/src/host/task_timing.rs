// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{
    CorrelationId, FrameId, ObservedMicroseconds, RecognitionId, RequestId, RunId, TaskId,
    TaskRecordSubphases, TaskTimingAppendStage, TaskTimingBoundary, TaskTimingBudgetObservation,
    TaskTimingCallContext, TaskTimingCheckPosition, TaskTimingFailure,
    TaskTimingFailureObservation, TaskTimingObservationState, TaskTimingObservations,
    TaskTimingObservedExpiry, TaskTimingPhase, TaskTimingPhaseObservations, TaskTimingResult,
    TaskTimingSample, TaskTimingSpanSummary, TimingObservationClock, TimingObservationIssue,
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
                first_observed_expiry: None,
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

    pub(super) fn task_failure(
        &mut self,
        timing: Option<&TaskTimingFailure>,
        check_position: Option<TaskTimingCheckPosition>,
    ) {
        if let Some(timing) = timing {
            self.value.task_failure = Some(TaskTimingFailureObservation {
                phase: self.phase,
                origin: self.context.map(ContainedTaskTimingContext::origin),
                timing: timing.clone(),
                check_position,
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

    pub(super) fn record_write(
        &mut self,
        sample: TaskTimingSample,
        subphases: TaskRecordSubphases,
    ) {
        let summary = &mut self.current().diagnostic_record_write;
        summary
            .subphases
            .get_or_insert_with(Default::default)
            .merge_record(subphases, sample.record_index);
        observe(summary, sample);
    }

    pub(super) fn snapshot(&self) -> Box<TaskTimingObservations> {
        self.value.clone()
    }

    pub(super) fn capture_recognition(&mut self, sample: TaskTimingSample) {
        observe(&mut self.current().capture_recognition, sample);
    }

    pub(super) fn recognition_completed_record(&mut self, sample: TaskTimingSample) {
        observe(&mut self.current().recognition_completed_record, sample);
    }

    fn current(&mut self) -> &mut TaskTimingPhaseObservations {
        match self.phase {
            TaskTimingPhase::Preflight => &mut self.value.preflight,
            TaskTimingPhase::Execution => &mut self.value.execution,
            TaskTimingPhase::Finalization => &mut self.value.finalization,
        }
    }
}

// Fixed, process-local endpoints. Only the original call result completes a stage.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct AppendCallSpan {
    pub(super) present: bool,
    pub(super) started: Option<Instant>,
    pub(super) ended: Option<Instant>,
    pub(super) result: Option<TaskTimingResult>,
}

impl AppendCallSpan {
    pub(super) fn begin(&mut self) {
        self.present = true;
        self.started = Some(Instant::now());
    }

    pub(super) fn finish(&mut self, succeeded: bool) {
        self.present = true;
        self.ended = Some(Instant::now());
        self.result = Some(if succeeded {
            TaskTimingResult::Ok
        } else {
            TaskTimingResult::Err
        });
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct AppendObservation {
    pub(super) fact_gate: AppendCallSpan,
    pub(super) draft: AppendCallSpan,
    pub(super) writer_response: AppendCallSpan,
    pub(super) device_diagnostics: AppendCallSpan,
    pub(super) fact_sync: AppendCallSpan,
    pub(super) pipeline: AppendCallSpan,
    pub(super) ledger: Option<actingcommand_ledger::LedgerAppendObservation>,
}

pub(super) use actingcommand_execution_kernel::ContainedTaskBoundaryIdentity as BoundaryIdentity;

#[derive(Debug, Clone, Copy)]
pub(super) struct BoundaryStart {
    boundary: TaskTimingBoundary,
    phase: TaskTimingPhase,
    context: Option<ContainedTaskTimingContext>,
    identity: BoundaryIdentity,
    started: Instant,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum RecognitionAppend {
    Payload,
    Task,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct AppendStart {
    start: BoundaryStart,
    kind: RecognitionAppend,
}

impl TaskTimingObserver {
    pub(super) fn kernel_boundary(
        &mut self,
        timing: actingcommand_execution_kernel::ContainedTaskBoundaryTiming,
    ) {
        self.boundary_span(
            BoundaryStart {
                boundary: timing.boundary,
                phase: self.phase,
                context: Some(timing.context),
                identity: timing.identity,
                started: timing.started,
            },
            None,
            AppendCallSpan {
                present: true,
                started: Some(timing.started),
                ended: Some(timing.ended),
                result: Some(if timing.succeeded {
                    TaskTimingResult::Ok
                } else {
                    TaskTimingResult::Err
                }),
            },
        );
    }

    pub(super) fn begin_append(
        &self,
        kind: RecognitionAppend,
        identity: BoundaryIdentity,
    ) -> AppendStart {
        AppendStart {
            start: self.begin_boundary(
                match kind {
                    RecognitionAppend::Payload => TaskTimingBoundary::RecognitionPayloadAppend,
                    RecognitionAppend::Task => TaskTimingBoundary::RecognitionTaskAppend,
                },
                identity,
            ),
            kind,
        }
    }
    pub(super) fn begin_boundary(
        &self,
        boundary: TaskTimingBoundary,
        identity: BoundaryIdentity,
    ) -> BoundaryStart {
        BoundaryStart {
            boundary,
            phase: self.phase,
            context: self.context,
            identity,
            started: Instant::now(),
        }
    }

    pub(super) fn finish_boundary(&mut self, start: BoundaryStart, succeeded: bool) {
        let ended = Instant::now();
        self.boundary_span(
            start,
            None,
            AppendCallSpan {
                present: true,
                started: Some(start.started),
                ended: Some(ended),
                result: Some(if succeeded {
                    TaskTimingResult::Ok
                } else {
                    TaskTimingResult::Err
                }),
            },
        );
    }

    pub(super) fn finish_append(
        &mut self,
        append: AppendStart,
        succeeded: bool,
        observation: AppendObservation,
    ) {
        let start = append.start;
        let ended = Instant::now();
        self.boundary_span(
            start,
            Some((TaskTimingAppendStage::FactGate, append.kind)),
            observation.fact_gate,
        );
        self.boundary_span(
            start,
            Some((TaskTimingAppendStage::Draft, append.kind)),
            observation.draft,
        );
        if let Some(ledger) = observation.ledger {
            self.ledger_span(append, TaskTimingAppendStage::LedgerQueue, ledger.queue);
            if let Some(durable_started) = ledger.durable_started_at {
                self.ledger_span(
                    append,
                    TaskTimingAppendStage::LedgerDurable,
                    actingcommand_ledger::LedgerAppendSpan {
                        started_at: Some(durable_started),
                        ..ledger.persistence
                    },
                );
            }
            self.ledger_span(
                append,
                TaskTimingAppendStage::LedgerPersistence,
                ledger.persistence,
            );
            self.ledger_span(
                append,
                TaskTimingAppendStage::LedgerPublication,
                ledger.publication,
            );
        }
        self.boundary_span(
            start,
            Some((TaskTimingAppendStage::WriterResponse, append.kind)),
            observation.writer_response,
        );
        self.boundary_span(
            start,
            Some((TaskTimingAppendStage::DeviceDiagnostics, append.kind)),
            observation.device_diagnostics,
        );
        self.boundary_span(
            start,
            Some((TaskTimingAppendStage::FactSync, append.kind)),
            observation.fact_sync,
        );
        self.boundary_span(
            start,
            Some((TaskTimingAppendStage::Pipeline, append.kind)),
            observation.pipeline,
        );
        self.boundary_span(
            start,
            None,
            AppendCallSpan {
                present: true,
                started: Some(start.started),
                ended: Some(ended),
                result: Some(if succeeded {
                    TaskTimingResult::Ok
                } else {
                    TaskTimingResult::Err
                }),
            },
        );
    }

    fn ledger_span(
        &mut self,
        append: AppendStart,
        stage: TaskTimingAppendStage,
        span: actingcommand_ledger::LedgerAppendSpan,
    ) {
        if span.state == actingcommand_ledger::LedgerAppendObservationState::Unobserved {
            return;
        }
        self.boundary_span(
            append.start,
            Some((stage, append.kind)),
            AppendCallSpan {
                present: true,
                started: span.started_at,
                ended: span.finished_at,
                result: span.result.map(|result| match result {
                    actingcommand_ledger::LedgerAppendStageResult::Ok => TaskTimingResult::Ok,
                    actingcommand_ledger::LedgerAppendStageResult::Err => TaskTimingResult::Err,
                }),
            },
        );
    }

    fn boundary_span(
        &mut self,
        start: BoundaryStart,
        append_stage: Option<(TaskTimingAppendStage, RecognitionAppend)>,
        span: AppendCallSpan,
    ) {
        if !span.present {
            return;
        }
        let budget = |instant: Option<Instant>| {
            instant.map_or(TaskTimingBudgetObservation::Unobserved, |at| {
                start
                    .context
                    .map_or(TaskTimingBudgetObservation::NotStarted, |context| {
                        context.budget_at(at)
                    })
            })
        };
        let sample = TaskTimingSample {
            elapsed_us: span.started.zip(span.ended).map_or(
                ObservedMicroseconds::Unavailable {
                    reason: TimingObservationIssue::CallIncomplete,
                },
                |(started, ended)| {
                    actingcommand_execution_kernel::observe_instant_span(started, ended)
                },
            ),
            budget_before: budget(span.started),
            result: span.result.unwrap_or(TaskTimingResult::Unobserved),
            record_index: None,
            frame_id: start.identity.frame_id,
            recognition_id: start.identity.recognition_id,
        };
        let context = TaskTimingCallContext {
            budget_after: budget(span.ended),
            step_index: start.identity.step_index,
            action_id: start.identity.action_id,
        };
        let phase = match start.phase {
            TaskTimingPhase::Preflight => &mut self.value.preflight,
            TaskTimingPhase::Execution => &mut self.value.execution,
            TaskTimingPhase::Finalization => &mut self.value.finalization,
        };
        let boundaries = phase.boundaries.get_or_insert_with(Default::default);
        let summary = match append_stage {
            None => boundaries.span_mut(start.boundary),
            Some((stage, kind)) => match kind {
                RecognitionAppend::Payload => boundaries.recognition_payload_stages.span_mut(stage),
                RecognitionAppend::Task => boundaries.recognition_task_stages.span_mut(stage),
            },
        };
        observe(summary, sample.clone());
        summary.last_call = Some(context.clone());
        if span.result.is_none() {
            summary.errors = None;
            incomplete(summary, TimingObservationIssue::CallIncomplete);
        }
        if span.started.is_none() {
            summary.attempts = None;
            incomplete(summary, TimingObservationIssue::CallIncomplete);
        }
        if let TaskTimingBudgetObservation::Unavailable { reason, .. } = context.budget_after {
            incomplete(summary, reason);
        }
        let complete = summary.status == TaskTimingObservationState::Observed;
        if complete
            && self.value.first_observed_expiry.is_none()
            && matches!(
                sample.budget_before,
                TaskTimingBudgetObservation::Observed {
                    origin: actingcommand_contract::TaskTimingBudgetOrigin::Task,
                    expired: false,
                    ..
                }
            )
            && matches!(
                context.budget_after,
                TaskTimingBudgetObservation::Observed {
                    origin: actingcommand_contract::TaskTimingBudgetOrigin::Task,
                    expired: true,
                    remaining_us: 0,
                }
            )
        {
            self.value.first_observed_expiry = Some(Box::new(TaskTimingObservedExpiry {
                phase: start.phase,
                boundary: start.boundary,
                append_stage: append_stage.map(|(stage, _)| stage),
                sample,
                context,
            }));
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
