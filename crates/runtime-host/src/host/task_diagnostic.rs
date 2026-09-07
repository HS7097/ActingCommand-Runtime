// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::{
    MAX_TASK_DIAGNOSTIC_RECORD_BYTES, OcrRegionRect, TASK_DIAGNOSTIC_SCHEMA,
    TaskDiagnosticArtifactData, TaskDiagnosticColorData, TaskDiagnosticErrorData,
    TaskDiagnosticHeader, TaskDiagnosticNnData, TaskDiagnosticNnLabel, TaskDiagnosticNnLabelData,
    TaskDiagnosticNnRank, TaskDiagnosticOcrBlock, TaskDiagnosticOcrBlockData,
    TaskDiagnosticOcrData, TaskDiagnosticOcrExecution, TaskDiagnosticPageData,
    TaskDiagnosticPayload as Payload, TaskDiagnosticRecognitionError, TaskDiagnosticRecord,
    TaskDiagnosticStepElapsedData, TaskDiagnosticStepStartedData, TaskDiagnosticTargetData,
    TaskDiagnosticTargetFailure, TaskDiagnosticTargetSource, TaskDiagnosticTemplateData,
    TaskDiagnosticTerminalData, TaskDiagnosticUnexecutedData, TaskDiagnosticUnexecutedPage,
};
use actingcommand_page_detector::{PageBatchResult, PageOutcome, PageTargetEvaluation};
use actingcommand_recognition_pack::{
    OcrObservationEvaluation, RecognitionPackResult, TargetEvaluation,
};
use std::io::Write;

pub(super) struct DiagnosticStep {
    index: u32,
    action_id: ActionId,
    started_ms: u64,
}

struct RecordWriter {
    bytes: Vec<u8>,
}

impl Write for RecordWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_TASK_DIAGNOSTIC_RECORD_BYTES - self.bytes.len() {
            return Err(std::io::Error::other(
                "task diagnostic record size exceeded",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn failure(error: impl std::fmt::Display) -> RequestFailure {
    RequestFailure::poison_without_terminal(
        artifact_store_error("task_diagnostic_failed").with_native_detail(error.to_string()),
    )
}

fn region_rect(value: actingcommand_recognition_pack::PackRect) -> OcrRegionRect {
    OcrRegionRect {
        x: value.x,
        y: value.y,
        width: value.width,
        height: value.height,
    }
}

fn page_role(value: actingcommand_page_detector::PageTargetRole) -> &'static str {
    use actingcommand_page_detector::PageTargetRole;
    match value {
        PageTargetRole::Required => "Required",
        PageTargetRole::AnyOf => "AnyOf",
        PageTargetRole::Optional => "Optional",
        PageTargetRole::Forbidden => "Forbidden",
    }
}

fn recognition_error(
    error: &actingcommand_recognition_pack::RecognitionPackError,
) -> TaskDiagnosticRecognitionError {
    use actingcommand_recognition_pack::{RecognitionPackErrorCode, RecognitionPackErrorSeverity};
    TaskDiagnosticRecognitionError {
        severity: match error.severity() {
            RecognitionPackErrorSeverity::Fatal => "Fatal",
            RecognitionPackErrorSeverity::Unresolved => "Unresolved",
        }
        .to_owned(),
        code: match error.code() {
            RecognitionPackErrorCode::InvalidPackage => "InvalidPackage",
            RecognitionPackErrorCode::UnsupportedTarget => "UnsupportedTarget",
            RecognitionPackErrorCode::VisionProviderMissing => "VisionProviderMissing",
            RecognitionPackErrorCode::VisionProviderFailure => "VisionProviderFailure",
            RecognitionPackErrorCode::VisionProviderInvalidResponse => {
                "VisionProviderInvalidResponse"
            }
            RecognitionPackErrorCode::RegionUnresolved => "RegionUnresolved",
        }
        .to_owned(),
        message: error.message().to_owned(),
        region: error.region().cloned().map(Box::new),
    }
}

fn ocr_provider(value: actingcommand_recognition_pack::OcrExecutionProviderKind) -> &'static str {
    match value {
        actingcommand_recognition_pack::OcrExecutionProviderKind::Cpu => "cpu",
        actingcommand_recognition_pack::OcrExecutionProviderKind::Cuda => "cuda",
    }
}

fn ocr_execution(
    value: &actingcommand_recognition_pack::OcrProviderExecutionEvidence,
) -> TaskDiagnosticOcrExecution {
    TaskDiagnosticOcrExecution {
        invocation_id: value.invocation_id.clone(),
        session_id: value.session_id.clone(),
        session_generation: value.session_generation,
        requested_provider: ocr_provider(value.requested_provider).to_owned(),
        resolved_provider: ocr_provider(value.resolved_provider).to_owned(),
        requested_cuda_ordinal: value.requested_cuda_ordinal,
        requested_cuda_identity: value.requested_cuda_identity.clone(),
        resolved_cuda_ordinal: value.resolved_cuda_ordinal,
        resolved_cuda_identity: value.resolved_cuda_identity.clone(),
        provider_implementation: value.provider_implementation.clone(),
        provider_binary_sha256: value.provider_binary_sha256.clone(),
        runtime_version: value.runtime_version.clone(),
        model_ref: value.model_ref.clone(),
        model_sha256: value.model_sha256.clone(),
        cpu_ep_registered: value.cpu_ep_registered,
        cpu_fallback_disabled: value.cpu_fallback_disabled,
        fallback_forbidden: value.fallback_forbidden,
        fallback_observed: value.fallback_observed,
        complete: value.complete,
    }
}

impl RuntimeContainedTask<'_> {
    pub(super) fn begin_diagnostic(&mut self) -> Result<(), RequestFailure> {
        let header = TaskDiagnosticHeader {
            schema_version: TASK_DIAGNOSTIC_SCHEMA.to_owned(),
            request_id: self.control.request_id,
            correlation_id: self.request.correlation_id(),
            task_id: *self.task_id.transport(),
            run_id: *self.run_id.transport(),
            instance_id: self.token.instance_id(),
            lease_id: self.token.lease_id(),
        };
        let bytes = serde_json::to_vec(&header).map_err(failure)?;
        self.diagnostic_stream = Some(
            self.host
                .artifacts
                .begin_stream(
                    ArtifactKind::DiagnosticJson,
                    ArtifactWriteContext::new(
                        self.request.task_artifact_links(self.run_id),
                        self.links(),
                        unix_ms_now().map_err(RequestFailure::poison_without_terminal)?,
                    ),
                    ArtifactIssuePolicy::new(
                        ArtifactProducer::ArtifactStore,
                        RetentionClass::DebugFull,
                        ArtifactRedactionState::Pending,
                    ),
                )
                .map_err(failure)?,
        );
        let stream = self.diagnostic_stream.as_mut().expect("created stream");
        stream
            .append(&bytes[..bytes.len() - 1])
            .and_then(|()| stream.append(b",\"records\":[\n"))
            .map_err(failure)
    }

    fn diagnostic_record(
        &self,
        parent_index: Option<u64>,
        payload: Payload,
    ) -> TaskDiagnosticRecord {
        TaskDiagnosticRecord {
            index: 0,
            parent_index,
            payload,
            frame_id: self.last_frame_id.map(|id| *id.transport()),
            step_action_id: self.diagnostic_step.as_ref().map(|step| step.action_id),
            physical_action_id: self.diagnostic_physical,
        }
    }

    fn write_diagnostic_record(
        &mut self,
        mut record: TaskDiagnosticRecord,
    ) -> Result<u64, RequestFailure> {
        let index = self
            .diagnostic_records
            .checked_add(1)
            .ok_or_else(|| failure("record count overflow"))?;
        record.index = index;
        let stream = self
            .diagnostic_stream
            .as_mut()
            .ok_or_else(|| failure("task diagnostic stream missing"))?;
        if self.diagnostic_records != 0 {
            stream.append(b",").map_err(failure)?;
        }
        let mut writer = RecordWriter { bytes: Vec::new() };
        // Preserve the original JSON number precision within this one bounded record.
        let encoded = serde_json::to_value(&record).map_err(failure)?;
        serde_json::to_writer(&mut writer, &encoded).map_err(failure)?;
        stream.append(&writer.bytes).map_err(failure)?;
        stream.append(b"\n").map_err(failure)?;
        self.diagnostic_records = index;
        Ok(index)
    }

    fn diagnostic(&mut self, parent: Option<u64>, payload: Payload) -> Result<u64, RequestFailure> {
        self.write_diagnostic_record(self.diagnostic_record(parent, payload))
    }

    pub(super) fn begin_diagnostic_step(
        &mut self,
        index: u32,
        action_id: ActionId,
        now: u64,
    ) -> Result<(), RequestFailure> {
        self.end_diagnostic_step(now, false)?;
        self.diagnostic_physical = None;
        self.diagnostic_step = Some(DiagnosticStep {
            index,
            action_id,
            started_ms: now,
        });
        self.diagnostic(
            None,
            Payload::StepStarted(TaskDiagnosticStepStartedData {
                step_index: index,
                monotonic_ms: now,
            }),
        )?;
        Ok(())
    }

    pub(super) fn end_diagnostic_step(
        &mut self,
        now: u64,
        completed: bool,
    ) -> Result<(), RequestFailure> {
        if let Some(step) = &self.diagnostic_step {
            let elapsed = now
                .checked_sub(step.started_ms)
                .ok_or_else(|| failure("step monotonic clock regressed"))?;
            self.diagnostic(
                None,
                Payload::StepElapsed(TaskDiagnosticStepElapsedData {
                    step_index: step.index,
                    started_monotonic_ms: step.started_ms,
                    ended_monotonic_ms: now,
                    elapsed_ms: elapsed,
                    completed,
                }),
            )?;
            self.diagnostic_step = None;
        }
        Ok(())
    }

    pub(super) fn diagnostic_pages(
        &mut self,
        phase: &'static str,
        results: &PageBatchResult,
    ) -> Result<(), RequestFailure> {
        let outcomes = match results {
            Ok(outcomes) => outcomes,
            Err(error) => &error.completed,
        };
        for outcome in outcomes {
            self.diagnostic_page(phase, outcome)?;
        }
        if let Err(error) = results {
            self.diagnostic(
                None,
                Payload::Error(Box::new(TaskDiagnosticErrorData::Page {
                    phase: phase.to_owned(),
                    message: error.cause.message().to_owned(),
                })),
            )?;
            for page in &error.unexecuted {
                self.diagnostic(
                    None,
                    Payload::Unexecuted(TaskDiagnosticUnexecutedData::Page {
                        phase: phase.to_owned(),
                        page: TaskDiagnosticUnexecutedPage {
                            index: page.index,
                            page_id: page.page_id.clone(),
                            reason: match page.reason {
                                actingcommand_page_detector::PageUnexecutedReason::BatchValidationFailed => "BatchValidationFailed",
                                actingcommand_page_detector::PageUnexecutedReason::BatchTerminated => "BatchTerminated",
                            }.to_owned(),
                        },
                    }),
                )?;
            }
        }
        Ok(())
    }

    fn diagnostic_page(
        &mut self,
        phase: &'static str,
        outcome: &PageOutcome,
    ) -> Result<(), RequestFailure> {
        let parent = match &outcome.result {
            Ok(page) => self.diagnostic(
                None,
                Payload::Page(Box::new(TaskDiagnosticPageData::Evaluated {
                    phase: phase.to_owned(),
                    index: outcome.index,
                    page_id: page.page_id.clone(),
                    matched: page.matched,
                    message: page.message.clone(),
                    required_passed: page.required_passed,
                    required_total: page.required_total,
                    any_of_passed: page.any_of_passed,
                    any_of_total: page.any_of_total,
                    optional_passed: page.optional_passed,
                    optional_total: page.optional_total,
                    forbidden_passed: page.forbidden_passed,
                    forbidden_total: page.forbidden_total,
                })),
            )?,
            Err(error) => self.diagnostic(
                None,
                Payload::Page(Box::new(TaskDiagnosticPageData::Failed {
                    phase: phase.to_owned(),
                    index: outcome.index,
                    page_id: outcome.page_id.clone(),
                    error: error.message().to_owned(),
                    failed_target: error.failed_target.as_ref().map(|target| {
                        TaskDiagnosticTargetFailure {
                            target_id: target.target_id.clone(),
                            role: page_role(target.role).to_owned(),
                            group_index: target.group_index,
                            target_index: target.target_index,
                            cause: recognition_error(&target.cause),
                        }
                    }),
                })),
            )?,
        };
        let targets = match &outcome.result {
            Ok(page) => &page.target_results,
            Err(error) => &error.completed_targets,
        };
        for target in targets {
            self.diagnostic_page_target(parent, target)?;
        }
        Ok(())
    }

    fn diagnostic_page_target(
        &mut self,
        parent: u64,
        target: &PageTargetEvaluation,
    ) -> Result<(), RequestFailure> {
        self.diagnostic_target(
            Some(parent),
            &target.evaluation,
            TaskDiagnosticTargetSource::Page {
                role: page_role(target.role).to_owned(),
                group_index: target.group_index,
                target_index: target.target_index,
                target_id: target.target_id.clone(),
                passed: target.passed,
                message: target.message.clone(),
            },
        )
    }

    pub(super) fn diagnostic_guard(
        &mut self,
        target: Option<&str>,
        result: Option<&RecognitionPackResult<TargetEvaluation>>,
        reason: &'static str,
    ) -> Result<(), RequestFailure> {
        match result {
            Some(Ok(value)) => self.diagnostic_target(
                None,
                value,
                TaskDiagnosticTargetSource::Guard {
                    phase: "guard".to_owned(),
                },
            ),
            Some(Err(error)) => self
                .diagnostic(
                    None,
                    Payload::Error(Box::new(TaskDiagnosticErrorData::Recognition {
                        phase: "guard".to_owned(),
                        target_id: target.map(str::to_owned),
                        error: recognition_error(error),
                    })),
                )
                .map(|_| ()),
            None => self
                .diagnostic(
                    None,
                    Payload::Unexecuted(TaskDiagnosticUnexecutedData::Guard {
                        phase: "guard".to_owned(),
                        target_id: target.map(str::to_owned),
                        reason: reason.to_owned(),
                    }),
                )
                .map(|_| ()),
        }
    }

    fn diagnostic_target(
        &mut self,
        parent: Option<u64>,
        target: &TargetEvaluation,
        source: TaskDiagnosticTargetSource,
    ) -> Result<(), RequestFailure> {
        let index = self.diagnostic(
            parent,
            Payload::Target(Box::new(TaskDiagnosticTargetData {
                id: target.id.clone(),
                kind: match target.kind {
                    actingcommand_recognition_pack::TargetKind::Template => "template",
                    actingcommand_recognition_pack::TargetKind::Color => "color",
                    actingcommand_recognition_pack::TargetKind::ClickOnly => "click_only",
                    actingcommand_recognition_pack::TargetKind::Ocr => "ocr",
                    actingcommand_recognition_pack::TargetKind::Nn => "nn",
                }
                .to_owned(),
                passed: target.passed,
                message: target.message.clone(),
                template: target.template.map(|value| TaskDiagnosticTemplateData {
                    x: value.x,
                    y: value.y,
                    width: value.width,
                    height: value.height,
                    raw_score: value.raw_score,
                    score: value.score,
                    threshold: value.threshold,
                }),
                color: target.color.map(|value| TaskDiagnosticColorData {
                    distance: value.distance,
                    max_distance: value.max_distance,
                    mean: value.mean,
                    expected: value.expected,
                }),
                source,
            })),
        )?;
        if let Some(ocr) = &target.ocr {
            let ocr_index = self.diagnostic(
                Some(index),
                Payload::Ocr(Box::new(TaskDiagnosticOcrData::Evaluated {
                    region: ocr.region.as_ref().clone(),
                    raw_text: ocr.raw_text.clone(),
                    derived_text: ocr.text.clone(),
                    confidence: ocr.confidence,
                    matched_expected: ocr.matched_expected.clone(),
                    match_mode: match ocr.match_mode {
                        actingcommand_recognition_pack::OcrMatchMode::Exact => "exact",
                        actingcommand_recognition_pack::OcrMatchMode::Contains => "contains",
                    }
                    .to_owned(),
                    block_count: ocr.blocks.len(),
                })),
            )?;
            self.diagnostic_ocr_blocks(ocr_index, &ocr.blocks, &ocr.block_source_order)?;
        }
        if let Some(nn) = &target.nn {
            let nn_index = self.diagnostic(
                Some(index),
                Payload::Nn(TaskDiagnosticNnData {
                    requested_region: region_rect(nn.requested_region),
                    selected_label: nn.selected_label.clone(),
                    selected_score: nn.selected_score,
                    selection: match nn.selection {
                        actingcommand_recognition_pack::NnSelectionMode::Best => "best",
                        actingcommand_recognition_pack::NnSelectionMode::Label => "label",
                    }
                    .to_owned(),
                    label_count: nn.labels.len(),
                }),
            )?;
            let mut labels = nn.labels.iter().enumerate().collect::<Vec<_>>();
            labels.sort_by_key(|(_, label)| label.source_index);
            for (rank, label) in labels {
                self.diagnostic(
                    Some(nn_index),
                    Payload::NnLabel(TaskDiagnosticNnLabelData {
                        source_index: label.source_index,
                        raw: TaskDiagnosticNnLabel {
                            label: label.label.clone(),
                            score: label.score,
                        },
                        derived: TaskDiagnosticNnRank {
                            candidate: label.candidate,
                            rank,
                        },
                    }),
                )?;
            }
        }
        Ok(())
    }

    fn diagnostic_ocr_blocks(
        &mut self,
        parent: u64,
        blocks: &[actingcommand_recognition_pack::OcrTextEvidence],
        order: &[usize],
    ) -> Result<(), RequestFailure> {
        if blocks.len() != order.len() {
            return Err(failure("OCR source order mismatch"));
        }
        let mut indices = order.iter().copied().enumerate().collect::<Vec<_>>();
        indices.sort_by_key(|(_, source)| *source);
        for (rank, source) in indices {
            self.diagnostic(
                Some(parent),
                Payload::OcrBlock(TaskDiagnosticOcrBlockData {
                    source_index: source,
                    derived_rank: rank,
                    raw: TaskDiagnosticOcrBlock {
                        text: blocks[rank].text.clone(),
                        rect: region_rect(blocks[rank].rect),
                        confidence: blocks[rank].confidence,
                    },
                }),
            )?;
        }
        Ok(())
    }

    pub(super) fn diagnostic_ocr(
        &mut self,
        target: &str,
        result: &RecognitionPackResult<OcrObservationEvaluation>,
    ) -> Result<(), RequestFailure> {
        match result {
            Ok(ocr) => {
                let index = self.diagnostic(
                    None,
                    Payload::Ocr(Box::new(TaskDiagnosticOcrData::Observed {
                        phase: "post_admission".to_owned(),
                        target_id: target.to_owned(),
                        region: ocr.region.clone(),
                        raw_text: ocr.raw_text.clone(),
                        derived_text: ocr.text.clone(),
                        confidence: ocr.confidence,
                        block_count: ocr.blocks.len(),
                        execution: Box::new(ocr_execution(&ocr.execution)),
                    })),
                )?;
                self.diagnostic_ocr_blocks(index, &ocr.blocks, &ocr.block_source_order)
            }
            Err(error) => self
                .diagnostic(
                    None,
                    Payload::Error(Box::new(TaskDiagnosticErrorData::Recognition {
                        phase: "post_admission".to_owned(),
                        target_id: Some(target.to_owned()),
                        error: recognition_error(error),
                    })),
                )
                .map(|_| ()),
        }
    }

    pub(super) fn finish_diagnostic(
        &mut self,
        result: &Result<ContainedTaskOutcome, ContainedTaskRunError<RequestFailure>>,
    ) -> Result<(), RequestFailure> {
        self.end_diagnostic_step(
            self.host
                .monotonic_ms()
                .map_err(RequestFailure::poison_without_terminal)?,
            false,
        )?;
        // Read a fixed ledger cut. Each existing verified artifact remains its original source.
        let through = self.host.ledger.latest_sequence().map_err(failure)?;
        let query = EventQuery {
            event_type: Some(EventType::ArtifactVerified),
            request_id: Some(self.control.request_id),
            task_id: Some(*self.task_id.transport()),
            run_id: Some(*self.run_id.transport()),
            ..EventQuery::default()
        };
        let mut after = 0;
        loop {
            let events = self
                .host
                .ledger
                .query_page(query.clone(), after, through, 256)
                .map_err(failure)?;
            if events.is_empty() {
                break;
            }
            for event in &events {
                for artifact in event.artifacts() {
                    self.write_diagnostic_record(TaskDiagnosticRecord {
                        index: 0,
                        parent_index: None,
                        frame_id: event.links().frame_id().copied(),
                        step_action_id: None,
                        physical_action_id: event.links().action_id().copied(),
                        payload: Payload::Artifact(Box::new(TaskDiagnosticArtifactData {
                            source_sequence: event.sequence(),
                            artifact: artifact.project(true),
                        })),
                    })?;
                }
            }
            after = events.last().expect("nonempty page").sequence();
        }
        let data = match result {
            Ok(outcome) => TaskDiagnosticTerminalData::Returned {
                outcome: outcome.outcome,
                executed_steps: outcome.executed_steps,
                final_page: outcome.final_page.clone(),
            },
            Err(ContainedTaskRunError::Task(error)) => TaskDiagnosticTerminalData::TaskError {
                code: error.code().to_owned(),
                detail: error.detail().map(str::to_owned),
                executed_steps: None,
            },
            Err(
                ContainedTaskRunError::Boundary(error)
                | ContainedTaskRunError::NonfatalOperation(error),
            ) => TaskDiagnosticTerminalData::OperationError {
                code: error.error.code(),
                executed_steps: None,
            },
        };
        self.diagnostic(None, Payload::Terminal(data))?;
        self.diagnostic_stream
            .as_mut()
            .ok_or_else(|| failure("stream missing at seal"))?
            .append(b"]}\n")
            .map_err(failure)?;
        let stream = self.diagnostic_stream.take().expect("stream ready to seal");
        let mut sink = RuntimeArtifactEventSink {
            ledger: &self.host.ledger,
            events: &self.host.events,
        };
        self.host
            .artifacts
            .seal_stream(stream, &mut sink)
            .map_err(failure)?;
        Ok(())
    }

    pub(super) fn abort_diagnostic(&mut self) -> Result<(), RequestFailure> {
        self.diagnostic_stream
            .take()
            .map_or(Ok(()), |stream| stream.abort().map_err(failure))
    }
}
