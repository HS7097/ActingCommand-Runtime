// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::codes::HostLocation;
use actingcommand_contract::codes::{ContractLocation, SharedCode};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    MAX_TASK_DIAGNOSTIC_RECORD_BYTES, OcrRegionRect, TASK_DIAGNOSTIC_SCHEMA_V2,
    TaskDiagnosticArtifactData, TaskDiagnosticColorData, TaskDiagnosticColorDigestData,
    TaskDiagnosticErrorData, TaskDiagnosticHeader, TaskDiagnosticNnData, TaskDiagnosticNnLabel,
    TaskDiagnosticNnLabelData, TaskDiagnosticNnRank, TaskDiagnosticOcrBlock,
    TaskDiagnosticOcrBlockData, TaskDiagnosticOcrData, TaskDiagnosticOcrExecution,
    TaskDiagnosticOcrLocatedData, TaskDiagnosticPageData, TaskDiagnosticPayload as Payload,
    TaskDiagnosticRecognitionError, TaskDiagnosticRecord, TaskDiagnosticStepElapsedData,
    TaskDiagnosticStepStartedData, TaskDiagnosticTargetData, TaskDiagnosticTargetFailure,
    TaskDiagnosticTargetSource, TaskDiagnosticTemplateData, TaskDiagnosticTerminalData,
    TaskDiagnosticUnexecutedData, TaskDiagnosticUnexecutedPage, TaskDiagnosticUniquenessData,
    TaskRecordSubphases, TaskTimingResult,
};
use actingcommand_execution_kernel::ContainedTaskGuardEvaluation;
use actingcommand_page_detector::{PageBatchResult, PageOutcome, PageTargetEvaluation};
use actingcommand_recognition_pack::{
    OcrObservationEvaluation, RecognitionPackResult, TargetEvaluation,
};
use std::io::Write;

pub(super) struct DiagnosticStep {
    pub(super) index: u32,
    pub(super) action_id: ActionId,
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
        self.bytes
            .try_reserve(bytes.len())
            .map_err(std::io::Error::other)?;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn failure(error: impl std::fmt::Display) -> RequestFailure {
    RequestFailure::poison_without_terminal(
        artifact_store_error(HostLocation::TaskDiagnosticFailed)
            .with_native_detail(error.to_string()),
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

/// A recognition error's row (Workflow #378 A2-P, M378 section 8.1): its outcome replaces the
/// copied severity and the recognition pack's code spelling. It is the registered outcome of
/// an evaluation that refused to conclude (Workflow #396 REC-1, REC-2) when the error has one,
/// else the error's own registered code.
fn recognition_error(
    error: &actingcommand_recognition_pack::RecognitionPackError,
) -> TaskDiagnosticRecognitionError {
    TaskDiagnosticRecognitionError {
        region: error.region().cloned().map(Box::new),
        timing: error.timing().copied(),
        sample: error.sample().cloned().map(Box::new),
        outcome: error
            .outcome()
            .cloned()
            .unwrap_or_else(|| Outcome::new(error.code().code())),
    }
}

/// The target row of one evaluation; its OCR, NN, digest and member rows are written
/// separately.
fn target_payload(
    target: &TargetEvaluation,
    source: TaskDiagnosticTargetSource,
) -> TaskDiagnosticTargetData {
    TaskDiagnosticTargetData {
        id: target.id.clone(),
        kind: match target.kind {
            actingcommand_recognition_pack::TargetKind::Template => "template",
            actingcommand_recognition_pack::TargetKind::Color => "color",
            actingcommand_recognition_pack::TargetKind::ClickOnly => "click_only",
            actingcommand_recognition_pack::TargetKind::Ocr => "ocr",
            actingcommand_recognition_pack::TargetKind::Nn => "nn",
            actingcommand_recognition_pack::TargetKind::ColorDigest => "color_digest",
            actingcommand_recognition_pack::TargetKind::Composite => "composite",
        }
        .to_owned(),
        passed: target.passed,
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
            region: value.region.map(Into::into),
        }),
        source,
        sampling: target.sampling.clone(),
        failure: target.failure.clone(),
        uniqueness: target.uniqueness.map(|value| TaskDiagnosticUniquenessData {
            score_milli: value.score_milli,
            second_score_milli: value.second_score_milli,
            margin_milli: value.margin_milli,
            second_rect: value.second_rect.map(region_rect),
        }),
    }
}

/// The OCR row under a target row.
fn ocr_payload(ocr: &actingcommand_recognition_pack::OcrEvaluation) -> TaskDiagnosticOcrData {
    TaskDiagnosticOcrData::Evaluated {
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
        located: ocr.located.map(|located| TaskDiagnosticOcrLocatedData {
            block_index: located.block_index,
            rect: region_rect(located.rect),
            confidence_milli: located.confidence_milli,
            cell_index: located.cell_index,
            cell_rect: located.cell_rect.map(region_rect),
            truncated: located.truncated,
        }),
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
            schema_version: TASK_DIAGNOSTIC_SCHEMA_V2.to_owned(),
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
                .map_err(online_observation::observation_artifact_failure)?,
        );
        let stream = self.diagnostic_stream.as_mut().expect("created stream");
        stream
            .append(&bytes[..bytes.len() - 1])
            .and_then(|()| stream.append(b",\"records\":[\n"))
            .map_err(online_observation::observation_artifact_failure)
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
        let started = std::time::Instant::now();
        let budget_before = self.task_timing.budget_at(started);
        let frame_id = record.frame_id;
        let recognition_id = (frame_id == self.last_frame_id.map(|id| *id.transport()))
            .then(|| self.current_recognition_id.map(|id| *id.transport()))
            .flatten();
        let mut record_index = None;
        let mut subphases = TaskRecordSubphases::default();
        let result = (|| {
            let index = self
                .diagnostic_records
                .checked_add(1)
                .ok_or_else(|| failure("record count overflow"))?;
            record.index = index;
            record_index = Some(index);
            let stream = self
                .diagnostic_stream
                .as_mut()
                .ok_or_else(|| failure("task diagnostic stream missing"))?;
            let mut writer = RecordWriter { bytes: Vec::new() };
            let encode_started = std::time::Instant::now();
            let encoded = serde_json::to_writer(&mut writer, &record);
            subphases.encode.observe(
                actingcommand_execution_kernel::observe_instant_span(
                    encode_started,
                    std::time::Instant::now(),
                ),
                if encoded.is_ok() {
                    TaskTimingResult::Ok
                } else {
                    TaskTimingResult::Err
                },
                None,
            );
            encoded.map_err(failure)?;
            let framing_started = std::time::Instant::now();
            let framed: Result<(), RequestFailure> = (|| {
                let has_separator = self.diagnostic_records != 0;
                let json_len = writer.bytes.len();
                let framed_len = json_len
                    .checked_add(usize::from(has_separator))
                    .and_then(|len| len.checked_add(1))
                    .ok_or_else(|| failure("record framing length overflow"))?;
                writer
                    .bytes
                    .try_reserve_exact(framed_len - json_len)
                    .map_err(failure)?;
                if has_separator {
                    writer.bytes.insert(0, b',');
                }
                writer.bytes.push(b'\n');
                Ok(())
            })();
            subphases.framing.observe(
                actingcommand_execution_kernel::observe_instant_span(
                    framing_started,
                    std::time::Instant::now(),
                ),
                if framed.is_ok() {
                    TaskTimingResult::Ok
                } else {
                    TaskTimingResult::Err
                },
                None,
            );
            framed?;
            let (appended, timing) = stream.append_observed(&writer.bytes);
            subphases.capacity_admit = timing.capacity_admit;
            subphases.file_write = timing.file_write;
            subphases.material_update = timing.material_update;
            appended.map_err(online_observation::observation_artifact_failure)?;
            self.diagnostic_records = index;
            Ok(index)
        })();
        let elapsed_us = actingcommand_execution_kernel::observe_instant_span(
            started,
            std::time::Instant::now(),
        );
        self.task_timing.record_write(
            actingcommand_contract::TaskTimingSample {
                elapsed_us,
                budget_before,
                result: if result.is_ok() {
                    actingcommand_contract::TaskTimingResult::Ok
                } else {
                    actingcommand_contract::TaskTimingResult::Err
                },
                record_index,
                frame_id,
                recognition_id,
            },
            subphases,
        );
        result
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
        let reports = match results {
            Ok(outcomes) => outcomes
                .iter()
                .flat_map(|outcome| match &outcome.result {
                    Ok(page) => page.ppocr_diagnostics(),
                    Err(error) => error.ppocr_diagnostics(),
                })
                .collect(),
            Err(error) => error.ppocr_diagnostics(),
        };
        let original_error = match results {
            Ok(outcomes) => outcomes
                .iter()
                .find_map(|outcome| outcome.result.as_ref().err()),
            Err(error) => Some(error.cause.as_ref()),
        };
        let primary = original_error.map(|error| {
            ppocr_diagnostic::task_ppocr_failure(
                SharedCode::ContainedTaskRecognitionFailed,
                error.to_string(),
                &reports,
            )
        });
        let preserve = |mut failure: RequestFailure| {
            if !reports.is_empty() {
                failure.error = Box::new(ppocr_diagnostic::attach_ppocr_source(
                    *failure.error,
                    crate::error::PpocrFailureSource::Pages(Box::new(results.clone())),
                ));
            }
            failure
        };
        self.archive_task_ppocr_diagnostics(&reports, phase, None, primary.clone())
            .map_err(preserve)?;
        let recorded = (|| {
            let outcomes = match results {
                Ok(outcomes) => outcomes,
                Err(error) => &error.completed,
            };
            for outcome in outcomes {
                self.diagnostic_page(phase, outcome)?;
            }
            if let Err(error) = results {
                // Ledger revision 3: the batch failure is the run's recognition failure; the
                // detector's text is not recorded.
                self.diagnostic(
                    None,
                    Payload::Error(Box::new(TaskDiagnosticErrorData::Page {
                        phase: phase.to_owned(),
                        outcome: Outcome::new(SharedCode::ContainedTaskRecognitionFailed),
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
                                outcome: Outcome::new(page.reason.code()),
                            },
                        }),
                    )?;
                }
            }
            Ok(())
        })();
        ppocr_diagnostic::finish_task_ppocr_record(recorded, &reports, primary).map_err(preserve)
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
            },
        )
    }

    pub(super) fn diagnostic_guard(
        &mut self,
        target: Option<&str>,
        result: Option<&RecognitionPackResult<TargetEvaluation>>,
        evaluation: ContainedTaskGuardEvaluation,
    ) -> Result<(), RequestFailure> {
        let sampled = match result {
            Some(Ok(value)) => matches!(
                value.sampling,
                Some(actingcommand_contract::TaskDiagnosticSamplingData::Sample { .. })
            ),
            Some(Err(error)) => error.sample().is_some(),
            None => false,
        };
        if sampled {
            // Sample rows carry their own immutable frame hash. During serialization their
            // OCR children and provider artifacts must not claim the latest capture ID.
            let frame = self.last_frame_id.take();
            let recognition = self.current_recognition_id.take();
            let recorded = self.diagnostic_guard_inner(target, result, evaluation);
            self.last_frame_id = frame;
            self.current_recognition_id = recognition;
            recorded
        } else {
            self.diagnostic_guard_inner(target, result, evaluation)
        }
    }

    fn diagnostic_guard_inner(
        &mut self,
        target: Option<&str>,
        result: Option<&RecognitionPackResult<TargetEvaluation>>,
        evaluation: ContainedTaskGuardEvaluation,
    ) -> Result<(), RequestFailure> {
        let phase = evaluation.phase();
        let reports = match result {
            Some(Ok(value)) => value.ppocr_diagnostics().to_vec(),
            Some(Err(error)) => error.ppocr_diagnostics().clone(),
            None => Vec::new(),
        };
        let primary = result
            .and_then(|result| result.as_ref().err())
            .map(|error| {
                ppocr_diagnostic::attach_ppocr_source(
                    ppocr_diagnostic::task_ppocr_failure(
                        SharedCode::ContainedTaskGuardEvaluationFailed,
                        error.to_string(),
                        &reports,
                    ),
                    crate::error::PpocrFailureSource::Recognition(Box::new(error.clone())),
                )
            });
        self.archive_task_ppocr_diagnostics(&reports, phase, target, primary.clone())?;
        let recorded = match result {
            Some(Ok(value)) => self.diagnostic_target(
                None,
                value,
                TaskDiagnosticTargetSource::Guard {
                    phase: phase.to_owned(),
                },
            ),
            Some(Err(error)) => self
                .diagnostic(
                    None,
                    Payload::Error(Box::new(TaskDiagnosticErrorData::Recognition {
                        phase: phase.to_owned(),
                        target_id: target.map(str::to_owned),
                        error: recognition_error(error),
                    })),
                )
                .map(|_| ()),
            // A guard target that was not evaluated: the kernel's code says why
            // (`contained_task_guard_skipped`, `contained_task_guard_page_mismatch`). An
            // evaluation that names no such code always comes with its result.
            None => match evaluation.unexecuted_code() {
                Some(code) => self
                    .diagnostic(
                        None,
                        Payload::Unexecuted(TaskDiagnosticUnexecutedData::Guard {
                            phase: phase.to_owned(),
                            target_id: target.map(str::to_owned),
                            outcome: Outcome::new(code),
                        }),
                    )
                    .map(|_| ()),
                None => Err(failure(format!(
                    "guard evaluation {evaluation:?} recorded without its result"
                ))),
            },
        };
        ppocr_diagnostic::finish_task_ppocr_record(recorded, &reports, primary)
    }

    fn diagnostic_target(
        &mut self,
        parent: Option<u64>,
        target: &TargetEvaluation,
        source: TaskDiagnosticTargetSource,
    ) -> Result<(), RequestFailure> {
        let mut record = self.diagnostic_record(
            parent,
            Payload::Target(Box::new(target_payload(target, source))),
        );
        if matches!(
            target.sampling,
            Some(actingcommand_contract::TaskDiagnosticSamplingData::Sample { .. })
        ) {
            record.frame_id = None;
        }
        let index = self.write_diagnostic_record(record)?;
        if let Some(ocr) = &target.ocr {
            let ocr_index =
                self.diagnostic(Some(index), Payload::Ocr(Box::new(ocr_payload(ocr))))?;
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
        if let Some(digest) = &target.color_digest {
            self.diagnostic(
                Some(index),
                Payload::ColorDigest(Box::new(TaskDiagnosticColorDigestData {
                    algorithm: digest.algorithm.clone(),
                    columns: digest.columns,
                    rows: digest.rows,
                    active_cells: digest.active_cells,
                    mean_milli: digest.mean_milli,
                    max_cell: digest.max_cell,
                    worst_cell: digest.worst_cell,
                    max_mean_milli: digest.max_mean_milli,
                    max_cell_threshold: digest.max_cell_threshold,
                    observed_cells: digest.observed_cells.clone(),
                })),
            )?;
        }
        // Every member in declaration order, each with its own child rows.
        if let Some(composite) = &target.composite {
            for (member_index, member) in composite.members.iter().enumerate() {
                self.diagnostic_target(
                    Some(index),
                    &member.evaluation,
                    TaskDiagnosticTargetSource::CompositeMember {
                        composite_target_id: target.id.clone(),
                        member_index,
                    },
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
        let reports = match result {
            Ok(ocr) => &ocr.ppocr_diagnostics,
            Err(error) => error.ppocr_diagnostics(),
        };
        let primary = result.as_ref().err().map(|error| {
            ppocr_diagnostic::attach_ppocr_source(
                ppocr_diagnostic::task_ppocr_failure(
                    SharedCode::ContainedTaskPostAdmissionOcrFailed,
                    error.to_string(),
                    reports,
                ),
                crate::error::PpocrFailureSource::Recognition(Box::new(error.clone())),
            )
        });
        self.archive_task_ppocr_diagnostics(
            reports,
            "post_admission",
            Some(target),
            primary.clone(),
        )?;
        let recorded = (|| match result {
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
        })();
        ppocr_diagnostic::finish_task_ppocr_record(recorded, reports, primary)
    }

    pub(super) fn finish_diagnostic(
        &mut self,
        result: &Result<ContainedTaskOutcome, ContainedTaskRunError<RequestFailure>>,
    ) -> Result<(), RequestFailure> {
        if let Some(stream) = &mut self.diagnostic_stream {
            stream.for_drain();
        }
        let terminal_step_action_id = self.diagnostic_step.as_ref().map(|step| step.action_id);
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
                task_result: outcome.outcome,
                executed_steps: outcome.executed_steps,
                final_page: outcome.final_page.clone(),
            },
            // Ledger revision 3: the task failure as the run's terminal records it (the
            // kernel's detail text is not kept).
            Err(ContainedTaskRunError::Task(error)) => TaskDiagnosticTerminalData::TaskError {
                outcome: Outcome::relayed(error.code().as_str())
                    .operation(ContractLocation::RunContainedTask),
                executed_steps: self.executed_steps,
                timing: error.timing().cloned(),
            },
            Err(
                ContainedTaskRunError::Boundary(error)
                | ContainedTaskRunError::NonfatalOperation(error),
            ) => TaskDiagnosticTerminalData::OperationError {
                outcome: error.error.outcome(),
                executed_steps: self.executed_steps,
            },
        };
        let mut terminal = self.diagnostic_record(None, Payload::Terminal(data));
        terminal.step_action_id = terminal_step_action_id;
        self.write_diagnostic_record(terminal)?;
        self.diagnostic_stream
            .as_mut()
            .ok_or_else(|| failure("stream missing at seal"))?
            .append(b"]}\n")
            .map_err(online_observation::observation_artifact_failure)?;
        let stream = self.diagnostic_stream.take().expect("stream ready to seal");
        let mut sink = RuntimeArtifactEventSink {
            ledger: &self.host.ledger,
            events: &self.host.events,
        };
        self.host
            .artifacts
            .seal_stream(stream, &mut sink)
            .map_err(online_observation::observation_artifact_failure)?;
        Ok(())
    }

    pub(super) fn abort_diagnostic(&mut self) -> Result<(), RequestFailure> {
        self.diagnostic_stream.take().map_or(Ok(()), |stream| {
            stream
                .abort()
                .map_err(online_observation::observation_artifact_failure)
        })
    }
}

// Workflow #396 REC-1: the guard rows of a locate OCR target.
#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_contract::OcrRegionEvidence;
    use actingcommand_contract::outcome::Outcome;
    use actingcommand_recognition_pack::{
        OcrEvaluation, OcrLocatedEvidence, OcrMatchMode, OcrTextEvidence, PackRect,
        RecognitionPackCode, RecognitionPackLocation, TargetKind, TargetMatchKind,
        TargetUniquenessEvidence, TemplateEvaluation,
    };

    fn rect(x: i32, y: i32, width: i32, height: i32) -> PackRect {
        PackRect {
            x,
            y,
            width,
            height,
        }
    }

    fn region_rect_of(rect: PackRect) -> OcrRegionRect {
        OcrRegionRect {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
        }
    }

    fn located_target(
        located: Option<OcrLocatedEvidence>,
        failure: Option<Outcome>,
    ) -> TargetEvaluation {
        let region = rect(40, 120, 600, 480);
        TargetEvaluation {
            id: "text/entry".to_owned(),
            kind: TargetKind::Ocr,
            passed: located.is_some(),
            template: None,
            color: None,
            ocr: Some(Box::new(OcrEvaluation {
                ppocr_diagnostics: Vec::new(),
                region: Box::new(OcrRegionEvidence {
                    frame_width: 1280,
                    frame_height: 720,
                    anchor_target_id: None,
                    anchor_match: None,
                    offset: None,
                    width: region.width,
                    height: region.height,
                    roi: Some(region_rect_of(region)),
                    unresolved: None,
                }),
                raw_text: "GAMMA\nALPHA".to_owned(),
                block_source_order: vec![0, 1],
                text: "GAMMA\nALPHA".to_owned(),
                confidence: Some(0.9),
                matched_expected: located.map(|_| "ALPHA".to_owned()),
                match_mode: OcrMatchMode::Exact,
                blocks: vec![
                    OcrTextEvidence {
                        text: "GAMMA".to_owned(),
                        rect: rect(60, 130, 80, 30),
                        confidence: Some(0.9),
                    },
                    OcrTextEvidence {
                        text: "ALPHA".to_owned(),
                        rect: rect(200, 400, 80, 30),
                        confidence: Some(0.93),
                    },
                ],
                located,
            })),
            nn: None,
            color_digest: None,
            composite: None,
            failure,
            uniqueness: None,
            message: "ocr text located".to_owned(),
            sampling: None,
            sample_evaluations: Vec::new(),
        }
    }

    /// Workflow #378 A3 (acceptance CL-L6): the error page, the unexecuted page and the
    /// unexecuted guard rows round-trip with their outcomes (ledger revision 3).
    #[test]
    fn error_and_unexecuted_rows_round_trip_with_their_outcome() {
        let rows = [
            record(
                1,
                None,
                Payload::Error(Box::new(TaskDiagnosticErrorData::Page {
                    phase: "recognition".to_owned(),
                    outcome: Outcome::new(SharedCode::ContainedTaskRecognitionFailed),
                })),
            ),
            record(
                2,
                None,
                Payload::Unexecuted(TaskDiagnosticUnexecutedData::Page {
                    phase: "recognition".to_owned(),
                    page: TaskDiagnosticUnexecutedPage {
                        index: 1,
                        page_id: "page/b".to_owned(),
                        outcome: Outcome::new(
                            actingcommand_page_detector::PageUnexecutedReason::BatchTerminated
                                .code(),
                        ),
                    },
                }),
            ),
            record(
                3,
                None,
                Payload::Unexecuted(TaskDiagnosticUnexecutedData::Guard {
                    phase: "guard".to_owned(),
                    target_id: Some("text/entry".to_owned()),
                    outcome: Outcome::new(
                        actingcommand_execution_kernel::ExecutionKernelCode::ContainedTaskGuardPageMismatch,
                    ),
                }),
            ),
        ];
        for row in &rows {
            assert_eq!(&round_trip(row), row);
        }
    }

    fn round_trip(record: &TaskDiagnosticRecord) -> TaskDiagnosticRecord {
        let bytes = serde_json::to_vec(record).expect("serialize the record");
        serde_json::from_slice(&bytes).expect("read the record back")
    }

    fn record(index: u64, parent_index: Option<u64>, payload: Payload) -> TaskDiagnosticRecord {
        TaskDiagnosticRecord {
            index,
            frame_id: None,
            step_action_id: None,
            physical_action_id: None,
            parent_index,
            payload,
        }
    }

    #[test]
    fn guard_rows_carry_the_located_block_and_cell_and_the_typed_failure_and_round_trip() {
        let found = located_target(
            Some(OcrLocatedEvidence {
                block_index: 1,
                rect: rect(200, 400, 80, 30),
                confidence_milli: 930,
                cell_index: Some(5),
                cell_rect: Some(rect(190, 360, 140, 230)),
                truncated: false,
            }),
            None,
        );
        let guard = || TaskDiagnosticTargetSource::Guard {
            phase: "guard".to_owned(),
        };
        let target_row = record(
            1,
            None,
            Payload::Target(Box::new(target_payload(&found, guard()))),
        );
        let ocr_row = record(
            2,
            Some(1),
            Payload::Ocr(Box::new(ocr_payload(
                found.ocr.as_deref().expect("OCR evidence"),
            ))),
        );
        for row in [&target_row, &ocr_row] {
            assert_eq!(&round_trip(row), row);
        }
        let Payload::Ocr(ocr) = &round_trip(&ocr_row).payload else {
            panic!("an OCR row");
        };
        let TaskDiagnosticOcrData::Evaluated {
            located: Some(located),
            matched_expected,
            ..
        } = ocr.as_ref()
        else {
            panic!("the OCR row carries located");
        };
        assert_eq!(matched_expected.as_deref(), Some("ALPHA"));
        assert_eq!(located.block_index, 1);
        assert_eq!(located.rect, region_rect_of(rect(200, 400, 80, 30)));
        assert_eq!(located.confidence_milli, 930);
        assert_eq!(located.cell_index, Some(5));
        assert_eq!(
            located.cell_rect,
            Some(region_rect_of(rect(190, 360, 140, 230)))
        );
        assert!(!located.truncated);

        let failure = Outcome::new(RecognitionPackCode::TextLocateNotFound)
            .target_id("text/entry")
            .count(2_u32)
            .stage(RecognitionPackLocation::TextLocateEvaluate);
        let missed = located_target(None, Some(failure.clone()));
        let missed_row = record(
            3,
            None,
            Payload::Target(Box::new(target_payload(&missed, guard()))),
        );
        let read = round_trip(&missed_row);
        assert_eq!(read, missed_row);
        let Payload::Target(target) = &read.payload else {
            panic!("a target row");
        };
        assert!(!target.passed);
        let read_failure = target.failure.as_ref().expect("the typed failure");
        assert!(read_failure.is(RecognitionPackCode::TextLocateNotFound));
        assert_eq!(
            read_failure.value("target_id"),
            Some(&serde_json::json!("text/entry"))
        );
        assert_eq!(read_failure.value("count"), Some(&serde_json::json!(2)));
        let Payload::Ocr(ocr) = record(
            4,
            Some(3),
            Payload::Ocr(Box::new(ocr_payload(
                missed.ocr.as_deref().expect("OCR evidence"),
            ))),
        )
        .payload
        else {
            panic!("an OCR row");
        };
        assert!(matches!(
            ocr.as_ref(),
            TaskDiagnosticOcrData::Evaluated { located: None, .. }
        ));
    }

    // Workflow #396 REC-2: the guard row of a unique target with a second place.
    #[test]
    fn a_guard_row_carries_the_uniqueness_check_and_round_trips() {
        let failure = Outcome::new(RecognitionPackCode::TargetMatchNotUnique)
            .target_id("icon/unique")
            .target_kind(TargetMatchKind::Template)
            .score_milli(1000_u32)
            .second_score_milli(1000_u32)
            .margin_milli(50_u32)
            .stage(RecognitionPackLocation::TargetMatchUnique);
        let target = TargetEvaluation {
            id: "icon/unique".to_owned(),
            kind: TargetKind::Template,
            passed: false,
            template: Some(TemplateEvaluation {
                x: 40,
                y: 30,
                width: 24,
                height: 18,
                raw_score: 1.0,
                score: 1.0,
                threshold: 0.9,
            }),
            color: None,
            ocr: None,
            nn: None,
            color_digest: None,
            composite: None,
            failure: Some(failure),
            uniqueness: Some(TargetUniquenessEvidence {
                score_milli: 1000,
                second_score_milli: Some(1000),
                margin_milli: 50,
                second_rect: Some(rect(200, 150, 24, 18)),
            }),
            message: "target match not unique".to_owned(),
            sampling: None,
            sample_evaluations: Vec::new(),
        };
        let row = record(
            1,
            None,
            Payload::Target(Box::new(target_payload(
                &target,
                TaskDiagnosticTargetSource::Guard {
                    phase: "guard".to_owned(),
                },
            ))),
        );

        let read = round_trip(&row);

        assert_eq!(read, row);
        let Payload::Target(data) = &read.payload else {
            panic!("a target row");
        };
        assert_eq!(
            data.uniqueness,
            Some(TaskDiagnosticUniquenessData {
                score_milli: 1000,
                second_score_milli: Some(1000),
                margin_milli: 50,
                second_rect: Some(region_rect_of(rect(200, 150, 24, 18))),
            })
        );
        let failure = data.failure.as_ref().expect("the typed failure");
        assert!(failure.is(RecognitionPackCode::TargetMatchNotUnique));
        assert_eq!(
            failure.value("target_kind"),
            Some(&serde_json::json!("template"))
        );
        assert_eq!(failure.value("margin_milli"), Some(&serde_json::json!(50)));
    }
}
