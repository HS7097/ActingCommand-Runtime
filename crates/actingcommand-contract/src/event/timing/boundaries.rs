// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskTimingBoundary {
    CapturePage,
    Capture,
    CaptureActivePressure,
    CaptureBackend,
    CaptureMaterial,
    CaptureCompletedRecord,
    RecognitionStartedRecord,
    Input,
    InputToEffectCompleted,
    EffectCompletedRecord,
    EffectCompletedToPostInputWait,
    EffectCompletedAppend,
    PostInputWait,
    RetryWait,
    PageRecognitionWait,
    PostconditionWait,
    RecognitionPayloadAppend,
    RecognitionTaskAppend,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskTimingAppendStage {
    FactGate,
    Draft,
    WriterResponse,
    LedgerSend,
    LedgerQueue,
    LedgerPersistence,
    LedgerDurable,
    LedgerPublication,
    DeviceDiagnostics,
    FactSync,
    Pipeline,
}

/// Identity and budget sampled from the same call as the parent summary's last sample.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskTimingCallContext {
    pub budget_after: TaskTimingBudgetObservation,
    pub step_index: Option<u32>,
    pub action_id: Option<super::super::ActionId>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskTimingAppendObservations {
    #[serde(default, skip_serializing_if = "TaskTimingSpanSummary::is_unobserved")]
    pub fact_gate: TaskTimingSpanSummary,
    #[serde(default, skip_serializing_if = "TaskTimingSpanSummary::is_unobserved")]
    pub draft: TaskTimingSpanSummary,
    #[serde(default, skip_serializing_if = "TaskTimingSpanSummary::is_unobserved")]
    pub writer_response: TaskTimingSpanSummary,
    #[serde(default, skip_serializing_if = "TaskTimingSpanSummary::is_unobserved")]
    pub ledger_send: TaskTimingSpanSummary,
    #[serde(default, skip_serializing_if = "TaskTimingSpanSummary::is_unobserved")]
    pub ledger_queue: TaskTimingSpanSummary,
    #[serde(default, skip_serializing_if = "TaskTimingSpanSummary::is_unobserved")]
    pub ledger_persistence: TaskTimingSpanSummary,
    #[serde(default, skip_serializing_if = "TaskTimingSpanSummary::is_unobserved")]
    pub ledger_durable: TaskTimingSpanSummary,
    #[serde(default, skip_serializing_if = "TaskTimingSpanSummary::is_unobserved")]
    pub ledger_publication: TaskTimingSpanSummary,
    #[serde(default, skip_serializing_if = "TaskTimingSpanSummary::is_unobserved")]
    pub device_diagnostics: TaskTimingSpanSummary,
    #[serde(default, skip_serializing_if = "TaskTimingSpanSummary::is_unobserved")]
    pub fact_sync: TaskTimingSpanSummary,
    #[serde(default, skip_serializing_if = "TaskTimingSpanSummary::is_unobserved")]
    pub pipeline: TaskTimingSpanSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writer: Option<Box<TaskTimingWriterObservation>>,
}

impl TaskTimingAppendObservations {
    pub fn span_mut(&mut self, stage: TaskTimingAppendStage) -> &mut TaskTimingSpanSummary {
        match stage {
            TaskTimingAppendStage::FactGate => &mut self.fact_gate,
            TaskTimingAppendStage::Draft => &mut self.draft,
            TaskTimingAppendStage::WriterResponse => &mut self.writer_response,
            TaskTimingAppendStage::LedgerSend => &mut self.ledger_send,
            TaskTimingAppendStage::LedgerQueue => &mut self.ledger_queue,
            TaskTimingAppendStage::LedgerPersistence => &mut self.ledger_persistence,
            TaskTimingAppendStage::LedgerDurable => &mut self.ledger_durable,
            TaskTimingAppendStage::LedgerPublication => &mut self.ledger_publication,
            TaskTimingAppendStage::DeviceDiagnostics => &mut self.device_diagnostics,
            TaskTimingAppendStage::FactSync => &mut self.fact_sync,
            TaskTimingAppendStage::Pipeline => &mut self.pipeline,
        }
    }

    pub(super) fn is_unobserved(&self) -> bool {
        self == &Self::default()
    }

    fn is_valid(&self) -> bool {
        [
            &self.fact_gate,
            &self.draft,
            &self.writer_response,
            &self.ledger_send,
            &self.ledger_queue,
            &self.ledger_persistence,
            &self.ledger_durable,
            &self.ledger_publication,
            &self.device_diagnostics,
            &self.fact_sync,
            &self.pipeline,
        ]
        .into_iter()
        .all(valid_boundary_span)
            && self.writer.as_ref().is_none_or(|writer| writer.is_valid())
    }
}

/// Fixed call sites; nested append spans overlap their containing calls.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskTimingBoundaryObservations {
    #[serde(default, skip_serializing_if = "TaskTimingSpanSummary::is_unobserved")]
    pub capture_page: TaskTimingSpanSummary,
    #[serde(default, skip_serializing_if = "TaskTimingSpanSummary::is_unobserved")]
    pub capture: TaskTimingSpanSummary,
    #[serde(default, skip_serializing_if = "TaskTimingSpanSummary::is_unobserved")]
    pub capture_active_pressure: TaskTimingSpanSummary,
    #[serde(default, skip_serializing_if = "TaskTimingSpanSummary::is_unobserved")]
    pub capture_backend: TaskTimingSpanSummary,
    #[serde(default, skip_serializing_if = "TaskTimingSpanSummary::is_unobserved")]
    pub capture_material: TaskTimingSpanSummary,
    #[serde(default, skip_serializing_if = "TaskTimingSpanSummary::is_unobserved")]
    pub capture_completed_record: TaskTimingSpanSummary,
    #[serde(default, skip_serializing_if = "TaskTimingSpanSummary::is_unobserved")]
    pub recognition_started_record: TaskTimingSpanSummary,
    #[serde(default, skip_serializing_if = "TaskTimingSpanSummary::is_unobserved")]
    pub input: TaskTimingSpanSummary,
    #[serde(default, skip_serializing_if = "TaskTimingSpanSummary::is_unobserved")]
    pub input_to_effect_completed: TaskTimingSpanSummary,
    #[serde(default, skip_serializing_if = "TaskTimingSpanSummary::is_unobserved")]
    pub effect_completed_record: TaskTimingSpanSummary,
    #[serde(default, skip_serializing_if = "TaskTimingSpanSummary::is_unobserved")]
    pub effect_completed_to_post_input_wait: TaskTimingSpanSummary,
    #[serde(default, skip_serializing_if = "TaskTimingSpanSummary::is_unobserved")]
    pub effect_completed_append: TaskTimingSpanSummary,
    #[serde(default, skip_serializing_if = "TaskTimingSpanSummary::is_unobserved")]
    pub post_input_wait: TaskTimingSpanSummary,
    #[serde(default, skip_serializing_if = "TaskTimingSpanSummary::is_unobserved")]
    pub retry_wait: TaskTimingSpanSummary,
    #[serde(default, skip_serializing_if = "TaskTimingSpanSummary::is_unobserved")]
    pub page_recognition_wait: TaskTimingSpanSummary,
    #[serde(default, skip_serializing_if = "TaskTimingSpanSummary::is_unobserved")]
    pub postcondition_wait: TaskTimingSpanSummary,
    #[serde(default, skip_serializing_if = "TaskTimingSpanSummary::is_unobserved")]
    pub recognition_payload_append: TaskTimingSpanSummary,
    #[serde(default, skip_serializing_if = "TaskTimingSpanSummary::is_unobserved")]
    pub recognition_task_append: TaskTimingSpanSummary,
    #[serde(
        default,
        skip_serializing_if = "TaskTimingAppendObservations::is_unobserved"
    )]
    pub recognition_payload_stages: TaskTimingAppendObservations,
    #[serde(
        default,
        skip_serializing_if = "TaskTimingAppendObservations::is_unobserved"
    )]
    pub recognition_task_stages: TaskTimingAppendObservations,
    #[serde(
        default,
        skip_serializing_if = "TaskTimingAppendObservations::is_unobserved"
    )]
    pub effect_completed_stages: TaskTimingAppendObservations,
}

impl TaskTimingBoundaryObservations {
    pub fn span_mut(&mut self, boundary: TaskTimingBoundary) -> &mut TaskTimingSpanSummary {
        match boundary {
            TaskTimingBoundary::CapturePage => &mut self.capture_page,
            TaskTimingBoundary::Capture => &mut self.capture,
            TaskTimingBoundary::CaptureActivePressure => &mut self.capture_active_pressure,
            TaskTimingBoundary::CaptureBackend => &mut self.capture_backend,
            TaskTimingBoundary::CaptureMaterial => &mut self.capture_material,
            TaskTimingBoundary::CaptureCompletedRecord => &mut self.capture_completed_record,
            TaskTimingBoundary::RecognitionStartedRecord => &mut self.recognition_started_record,
            TaskTimingBoundary::Input => &mut self.input,
            TaskTimingBoundary::InputToEffectCompleted => &mut self.input_to_effect_completed,
            TaskTimingBoundary::EffectCompletedRecord => &mut self.effect_completed_record,
            TaskTimingBoundary::EffectCompletedToPostInputWait => {
                &mut self.effect_completed_to_post_input_wait
            }
            TaskTimingBoundary::EffectCompletedAppend => &mut self.effect_completed_append,
            TaskTimingBoundary::PostInputWait => &mut self.post_input_wait,
            TaskTimingBoundary::RetryWait => &mut self.retry_wait,
            TaskTimingBoundary::PageRecognitionWait => &mut self.page_recognition_wait,
            TaskTimingBoundary::PostconditionWait => &mut self.postcondition_wait,
            TaskTimingBoundary::RecognitionPayloadAppend => &mut self.recognition_payload_append,
            TaskTimingBoundary::RecognitionTaskAppend => &mut self.recognition_task_append,
        }
    }

    pub(super) fn is_valid(&self) -> bool {
        [
            &self.capture_page,
            &self.capture,
            &self.capture_active_pressure,
            &self.capture_backend,
            &self.capture_material,
            &self.capture_completed_record,
            &self.recognition_started_record,
            &self.input,
            &self.input_to_effect_completed,
            &self.effect_completed_record,
            &self.effect_completed_to_post_input_wait,
            &self.effect_completed_append,
            &self.post_input_wait,
            &self.retry_wait,
            &self.page_recognition_wait,
            &self.postcondition_wait,
            &self.recognition_payload_append,
            &self.recognition_task_append,
        ]
        .into_iter()
        .all(valid_boundary_span)
            && self.recognition_payload_stages.is_valid()
            && self.recognition_task_stages.is_valid()
            && self.effect_completed_stages.is_valid()
    }
}

fn valid_boundary_span(span: &TaskTimingSpanSummary) -> bool {
    span.is_valid()
        && span.subphases.is_none()
        && span
            .last
            .as_ref()
            .is_none_or(|sample| sample.record_index.is_none())
        && (span.last.is_some() == span.last_call.is_some())
}

/// First completed, directly measured call interval observed to cross the task budget.
/// It is not an inferred instant or an assertion about unmeasured gaps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskTimingObservedExpiry {
    pub phase: TaskTimingPhase,
    pub boundary: TaskTimingBoundary,
    pub append_stage: Option<TaskTimingAppendStage>,
    pub sample: TaskTimingSample,
    pub context: TaskTimingCallContext,
}

impl TaskTimingObservedExpiry {
    pub(super) fn is_valid(&self) -> bool {
        matches!(
            self.sample.elapsed_us,
            ObservedMicroseconds::Measured { .. }
        ) && self.sample.record_index.is_none()
            && self.sample.result != TaskTimingResult::Unobserved
            && matches!(
                self.sample.budget_before,
                TaskTimingBudgetObservation::Observed {
                    origin: TaskTimingBudgetOrigin::Task,
                    expired: false,
                    ..
                }
            )
            && matches!(
                self.context.budget_after,
                TaskTimingBudgetObservation::Observed {
                    origin: TaskTimingBudgetOrigin::Task,
                    expired: true,
                    remaining_us: 0,
                }
            )
            && (self.append_stage.is_none()
                || matches!(
                    self.boundary,
                    TaskTimingBoundary::RecognitionPayloadAppend
                        | TaskTimingBoundary::RecognitionTaskAppend
                        | TaskTimingBoundary::EffectCompletedAppend
                ))
    }
}
