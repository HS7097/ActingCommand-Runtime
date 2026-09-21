// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{
    CorrelationId, FrameId, ObservedMicroseconds, RecognitionId, RequestId, RunId, TaskId,
    TaskRecordSubphases, TaskTimingAppendStage, TaskTimingBoundary, TaskTimingBudgetObservation,
    TaskTimingCallContext, TaskTimingCheckPosition, TaskTimingFailure,
    TaskTimingFailureObservation, TaskTimingObservationState, TaskTimingObservations,
    TaskTimingObservedExpiry, TaskTimingPhase, TaskTimingPhaseObservations,
    TaskTimingPreviousWorkRelation, TaskTimingProjectViewCount, TaskTimingProjectViewObservation,
    TaskTimingProjectViewReadBudget, TaskTimingResult, TaskTimingSample, TaskTimingSpanSummary,
    TaskTimingWriterCommand, TaskTimingWriterEndpoint, TaskTimingWriterObservation,
    TaskTimingWriterReceiveOrder, TaskTimingWriterSpan, TimingObservationClock,
    TimingObservationIssue,
};
use actingcommand_execution_kernel::{ContainedTaskEvaluationTiming, ContainedTaskTimingContext};
use std::time::Instant;

#[derive(Clone)]
pub(super) struct TaskTimingObserver {
    value: Box<TaskTimingObservations>,
    phase: TaskTimingPhase,
    context: Option<ContainedTaskTimingContext>,
    input_completion: Option<BoundaryStart>,
    effect_completion: Option<BoundaryStart>,
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
            input_completion: None,
            effect_completion: None,
        }
    }

    pub(super) fn begin_execution(&mut self, context: ContainedTaskTimingContext) {
        self.incomplete_effect_bridges();
        self.context = Some(context);
        self.phase = TaskTimingPhase::Execution;
    }

    pub(super) fn replace_context(&mut self, context: Option<ContainedTaskTimingContext>) {
        self.incomplete_effect_bridges();
        self.context = context;
    }

    pub(super) fn context(&self) -> Option<ContainedTaskTimingContext> {
        self.context
    }

    pub(super) fn begin_finalization(&mut self) {
        self.incomplete_effect_bridges();
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
        let mut snapshot = self.clone();
        snapshot.incomplete_effect_bridges();
        snapshot.value
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
pub(super) enum TaskAppend {
    RecognitionPayload,
    RecognitionTask,
    EffectCompleted,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct AppendStart {
    start: BoundaryStart,
    kind: TaskAppend,
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
        match timing.boundary {
            TaskTimingBoundary::EffectCompletedRecord => {
                if let Some(start) = self.input_completion.take() {
                    self.complete_effect_bridge(start, timing, timing.ended, timing.succeeded);
                }
                self.incomplete_effect_bridges();
                if timing.succeeded {
                    self.effect_completion = Some(BoundaryStart {
                        boundary: TaskTimingBoundary::EffectCompletedToPostInputWait,
                        phase: self.phase,
                        context: Some(timing.context),
                        identity: timing.identity,
                        started: timing.ended,
                    });
                }
            }
            TaskTimingBoundary::PostInputWait => {
                if let Some(start) = self.effect_completion.take() {
                    self.complete_effect_bridge(start, timing, timing.started, true);
                }
            }
            _ => {}
        }
    }

    fn complete_effect_bridge(
        &mut self,
        start: BoundaryStart,
        timing: actingcommand_execution_kernel::ContainedTaskBoundaryTiming,
        ended: Instant,
        succeeded: bool,
    ) {
        let same_call = start.phase == self.phase
            && start.context == Some(timing.context)
            && start.identity.step_index.is_some()
            && start.identity.action_id.is_some()
            && start.identity == timing.identity;
        self.boundary_span(
            start,
            None,
            AppendCallSpan {
                present: true,
                started: Some(start.started),
                ended: same_call.then_some(ended),
                result: same_call.then_some(if succeeded {
                    TaskTimingResult::Ok
                } else {
                    TaskTimingResult::Err
                }),
            },
        );
    }

    fn incomplete_effect_bridges(&mut self) {
        for start in [self.input_completion.take(), self.effect_completion.take()]
            .into_iter()
            .flatten()
        {
            self.boundary_span(
                start,
                None,
                AppendCallSpan {
                    present: true,
                    started: Some(start.started),
                    ended: None,
                    result: None,
                },
            );
        }
    }

    pub(super) fn begin_append(&self, kind: TaskAppend, identity: BoundaryIdentity) -> AppendStart {
        AppendStart {
            start: self.begin_boundary(
                match kind {
                    TaskAppend::RecognitionPayload => TaskTimingBoundary::RecognitionPayloadAppend,
                    TaskAppend::RecognitionTask => TaskTimingBoundary::RecognitionTaskAppend,
                    TaskAppend::EffectCompleted => TaskTimingBoundary::EffectCompletedAppend,
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

    /// Returns the boundary's own span so a caller can carry it without a second clock read.
    pub(super) fn finish_boundary(
        &mut self,
        start: BoundaryStart,
        succeeded: bool,
    ) -> ObservedMicroseconds {
        let ended = Instant::now();
        if start.boundary == TaskTimingBoundary::Input {
            self.incomplete_effect_bridges();
        }
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
        if start.boundary == TaskTimingBoundary::Input && succeeded {
            self.input_completion = Some(BoundaryStart {
                boundary: TaskTimingBoundary::InputToEffectCompleted,
                started: ended,
                ..start
            });
        }
        actingcommand_execution_kernel::observe_instant_span(start.started, ended)
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
            self.ledger_span(append, TaskTimingAppendStage::LedgerSend, ledger.send);
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
        // Replace the last same-append snapshot even when this call has no Ledger reply.
        // Previous writer work carries no observing-task identity or task-budget sample.
        let writer = observation.ledger.map(writer_observation).map(Box::new);
        let phase = match start.phase {
            TaskTimingPhase::Preflight => &mut self.value.preflight,
            TaskTimingPhase::Execution => &mut self.value.execution,
            TaskTimingPhase::Finalization => &mut self.value.finalization,
        };
        let boundaries = phase.boundaries.get_or_insert_with(Default::default);
        match append.kind {
            TaskAppend::RecognitionPayload => boundaries.recognition_payload_stages.writer = writer,
            TaskAppend::RecognitionTask => boundaries.recognition_task_stages.writer = writer,
            TaskAppend::EffectCompleted => boundaries.effect_completed_stages.writer = writer,
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
        append_stage: Option<(TaskTimingAppendStage, TaskAppend)>,
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
                TaskAppend::RecognitionPayload => {
                    boundaries.recognition_payload_stages.span_mut(stage)
                }
                TaskAppend::RecognitionTask => boundaries.recognition_task_stages.span_mut(stage),
                TaskAppend::EffectCompleted => boundaries.effect_completed_stages.span_mut(stage),
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

fn writer_endpoint(anchor: Option<Instant>, endpoint: Option<Instant>) -> TaskTimingWriterEndpoint {
    use TaskTimingWriterEndpoint as Endpoint;
    let Some(endpoint) = endpoint else {
        return Endpoint::Unobserved;
    };
    let Some(anchor) = anchor else {
        return Endpoint::Unavailable {
            reason: TimingObservationIssue::CallIncomplete,
        };
    };
    match endpoint.cmp(&anchor) {
        std::cmp::Ordering::Less => Endpoint::BeforeSendStart {
            distance_us: actingcommand_execution_kernel::observe_instant_span(endpoint, anchor),
        },
        std::cmp::Ordering::Equal => Endpoint::AtSendStart,
        std::cmp::Ordering::Greater => Endpoint::AfterSendStart {
            distance_us: actingcommand_execution_kernel::observe_instant_span(anchor, endpoint),
        },
    }
}

fn writer_result(
    result: Option<actingcommand_ledger::LedgerAppendStageResult>,
) -> TaskTimingResult {
    match result {
        Some(actingcommand_ledger::LedgerAppendStageResult::Ok) => TaskTimingResult::Ok,
        Some(actingcommand_ledger::LedgerAppendStageResult::Err) => TaskTimingResult::Err,
        None => TaskTimingResult::Unobserved,
    }
}

fn writer_span(
    anchor: Option<Instant>,
    span: actingcommand_ledger::LedgerAppendSpan,
) -> TaskTimingWriterSpan {
    use actingcommand_ledger::LedgerAppendObservationState as State;
    let elapsed_us = if span.state == State::Unobserved {
        None
    } else {
        Some(span.started_at.zip(span.finished_at).map_or(
            ObservedMicroseconds::Unavailable {
                reason: TimingObservationIssue::CallIncomplete,
            },
            |(started, finished)| {
                actingcommand_execution_kernel::observe_instant_span(started, finished)
            },
        ))
    };
    let status = match (span.state, elapsed_us) {
        (State::Unobserved, _) => TaskTimingObservationState::Unobserved,
        (_, Some(ObservedMicroseconds::Unavailable { reason })) => {
            TaskTimingObservationState::Incomplete { reason }
        }
        (State::Observed, Some(ObservedMicroseconds::Measured { .. })) if span.result.is_some() => {
            TaskTimingObservationState::Observed
        }
        _ => TaskTimingObservationState::Incomplete {
            reason: TimingObservationIssue::CallIncomplete,
        },
    };
    TaskTimingWriterSpan {
        status,
        started: writer_endpoint(anchor, span.started_at),
        finished: writer_endpoint(anchor, span.finished_at),
        elapsed_us,
        result: writer_result(span.result),
    }
}

fn writer_observation(
    ledger: actingcommand_ledger::LedgerAppendObservation,
) -> TaskTimingWriterObservation {
    use actingcommand_ledger::{
        LedgerPreviousWorkRelation as Relation, LedgerWriterCommandKind as Command,
        LedgerWriterReceiveOrder as Order,
    };
    let previous = ledger.previous_writer_work;
    let anchor = ledger.send.started_at;
    TaskTimingWriterObservation {
        send_returned: writer_endpoint(anchor, ledger.send.finished_at),
        writer_received: writer_endpoint(anchor, ledger.queue.finished_at),
        receive_order: match ledger.writer_receive_order {
            Order::Unobserved => TaskTimingWriterReceiveOrder::Unobserved,
            Order::Incomplete => TaskTimingWriterReceiveOrder::Incomplete,
            Order::BeforeSendReturned => TaskTimingWriterReceiveOrder::BeforeSendReturned,
            Order::AtSendReturn => TaskTimingWriterReceiveOrder::AtSendReturn,
            Order::AfterSendReturned => TaskTimingWriterReceiveOrder::AfterSendReturned,
        },
        previous_work_relation: match ledger.previous_work_relation {
            Relation::Unobserved => TaskTimingPreviousWorkRelation::Unobserved,
            Relation::Incomplete => TaskTimingPreviousWorkRelation::Incomplete,
            Relation::CompletedBySendStart => TaskTimingPreviousWorkRelation::CompletedBySendStart,
            Relation::OverlapsSend => TaskTimingPreviousWorkRelation::OverlapsSend,
            Relation::StartedAtOrAfterSendReturn => {
                TaskTimingPreviousWorkRelation::StartedAtOrAfterSendReturn
            }
        },
        previous_command: previous.command.map(|command| match command {
            Command::ReleaseLabPin => TaskTimingWriterCommand::ReleaseLabPin,
            Command::RetentionCandidates => TaskTimingWriterCommand::RetentionCandidates,
            Command::AdmitArtifactEviction => TaskTimingWriterCommand::AdmitArtifactEviction,
            Command::FinishArtifactEviction => TaskTimingWriterCommand::FinishArtifactEviction,
            Command::AppendTransaction => TaskTimingWriterCommand::AppendTransaction,
            Command::Append => TaskTimingWriterCommand::Append,
            Command::ReconcileScheduledPolicySettlement => {
                TaskTimingWriterCommand::ReconcileScheduledPolicySettlement
            }
            Command::Query => TaskTimingWriterCommand::Query,
            Command::QueryPage => TaskTimingWriterCommand::QueryPage,
            Command::ProjectViewPage => TaskTimingWriterCommand::ProjectViewPage,
            Command::ResolveArtifact => TaskTimingWriterCommand::ResolveArtifact,
            Command::ProjectSchedulingOutcomes => {
                TaskTimingWriterCommand::ProjectSchedulingOutcomes
            }
            Command::LatestSequence => TaskTimingWriterCommand::LatestSequence,
            Command::Subscribe => TaskTimingWriterCommand::Subscribe,
            Command::ReplayPage => TaskTimingWriterCommand::ReplayPage,
            Command::Project => TaskTimingWriterCommand::Project,
            Command::ProjectPage => TaskTimingWriterCommand::ProjectPage,
            Command::Shutdown => TaskTimingWriterCommand::Shutdown,
        }),
        previous_processing: writer_span(anchor, previous.processing),
        previous_after_reply: writer_span(anchor, previous.after_reply),
        previous_reply_result: writer_result(previous.reply_result),
        previous_project_view: previous
            .project_view
            .map(|view| Box::new(project_view_observation(anchor, view))),
    }
}

fn project_view_count(
    count: actingcommand_ledger::LedgerProjectViewCount,
) -> TaskTimingProjectViewCount {
    use actingcommand_ledger::LedgerProjectViewCount as Count;
    match count {
        Count::Unobserved => TaskTimingProjectViewCount::Unobserved,
        Count::Observed(value) => TaskTimingProjectViewCount::Measured { value },
        Count::Incomplete => TaskTimingProjectViewCount::Unavailable {
            reason: TimingObservationIssue::CountOverflow,
        },
    }
}

fn project_view_observation(
    anchor: Option<Instant>,
    view: actingcommand_ledger::LedgerProjectViewObservation,
) -> TaskTimingProjectViewObservation {
    TaskTimingProjectViewObservation {
        admission: writer_span(anchor, view.admission),
        connection: writer_span(anchor, view.connection),
        with_connection: writer_span(anchor, view.with_connection),
        begin_transaction: writer_span(anchor, view.begin_transaction),
        read_snapshot: writer_span(anchor, view.read_snapshot),
        verify_snapshot: writer_span(anchor, view.verify_snapshot),
        prepare_events: writer_span(anchor, view.prepare_events),
        select_sequences: writer_span(anchor, view.select_sequences),
        project_page: writer_span(anchor, view.project_page),
        commit: writer_span(anchor, view.commit),
        rollback: writer_span(anchor, view.rollback),
        read_budget: view
            .read_budget
            .map(|budget| TaskTimingProjectViewReadBudget {
                max_bytes: budget.max_bytes,
                max_events: project_view_count(budget.max_events),
                deadline: writer_endpoint(anchor, Some(budget.deadline)),
            }),
        requested_limit: project_view_count(view.requested_limit),
        selection_limit: project_view_count(view.selection_limit),
        max_page_events: project_view_count(view.max_page_events),
        max_response_bytes: project_view_count(view.max_response_bytes),
        max_recovery_context_events: project_view_count(view.max_recovery_context_events),
        raw_bytes: project_view_count(view.raw_bytes),
        raw_event_rows: project_view_count(view.raw_event_rows),
        raw_link_rows: project_view_count(view.raw_link_rows),
        raw_artifact_rows: project_view_count(view.raw_artifact_rows),
        verified_records: project_view_count(view.verified_records),
        prepared_events: project_view_count(view.prepared_events),
        selected_sequences: project_view_count(view.selected_sequences),
        returned_events: project_view_count(view.returned_events),
        returned_recovery_groups: project_view_count(view.returned_recovery_groups),
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
