// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

impl HostShared {
    pub(super) fn export_evidence(
        &self,
        validated: &ValidatedRuntimeRequest<'_>,
        request: &RuntimeEvidenceExportRequest,
    ) -> Result<OperationSuccess, RequestFailure> {
        let mut debug_runs = lock(&self.debug_runs, "lock_runtime_debug_runs")?;
        let context = debug_runs
            .get_mut(&validated.correlation_id())
            .ok_or_else(|| {
                RequestFailure::request(
                    RuntimeHostError::request(
                        "runtime_debug_context_missing",
                        "export_evidence",
                        RuntimeErrorCode::EvidenceExportFailed,
                    ),
                    RuntimeReceiptState::Denied,
                    None,
                )
            })?;
        if let Some(completed) = &context.completed_export {
            if completed.request_output_path == request.output_path()
                && completed.task_outcome == request.task_outcome()
            {
                return Ok(OperationSuccess {
                    state: RuntimeReceiptState::Completed,
                    terminal: Some(completed.response_terminal),
                    result: RuntimeResult::EvidenceExportCompleted {
                        summary: Box::new(completed.summary.clone()),
                    },
                });
            }
            return Err(RequestFailure::request(
                RuntimeHostError::request(
                    "runtime_evidence_export_conflict",
                    "export_evidence",
                    RuntimeErrorCode::EvidenceExportFailed,
                ),
                RuntimeReceiptState::Denied,
                None,
            ));
        }
        if context
            .terminal_outcome
            .is_some_and(|outcome| outcome != request.task_outcome())
        {
            return Err(RequestFailure::request(
                RuntimeHostError::request(
                    "runtime_task_outcome_conflict",
                    "export_evidence",
                    RuntimeErrorCode::EvidenceExportFailed,
                ),
                RuntimeReceiptState::Denied,
                None,
            ));
        }

        let task_links = validated.task_event_links(context.task_id, context.run_id);
        if context.terminal_outcome.is_none() {
            self.append_event(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
                task_links.clone(),
                TaskPayloadDraft::terminal_intent(EventAction::ArtifactExport, AuditInput::new()),
            )?;
            self.append_event(
                task_outcome_severity(request.task_outcome()),
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
                task_links.clone(),
                task_outcome_payload(request.task_outcome()),
            )?;
            context.terminal_outcome = Some(request.task_outcome());
        }

        let events = self
            .ledger
            .project(
                EventQuery {
                    correlation_id: Some(validated.correlation_id()),
                    ..EventQuery::default()
                },
                actingcommand_contract::ProjectionProfile::Forensic,
            )
            .map_err(|_| RequestFailure::poison(ledger_error("project_evidence_events"), None))?;
        let terminal_receipt = events
            .iter()
            .rev()
            .find(|event| {
                event.links.run_id() == Some(context.run_id.transport())
                    && event.event_type == task_outcome_event_type(request.task_outcome())
            })
            .cloned()
            .ok_or_else(|| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "runtime_task_terminal_missing",
                    "export_evidence",
                    RuntimeErrorCode::RuntimeFatal,
                ))
            })?;
        let (source_capture_summary_sequence, pipeline) = self
            .authoritative_capture_pipeline_summary(
                &events,
                validated.correlation_id(),
                *context.run_id.transport(),
                terminal_receipt.sequence,
            )?;
        let documents = runtime_evidence_documents(
            context.run_id,
            context.task_id,
            request.task_outcome(),
            &terminal_receipt,
            &events,
        )?;
        let archive_context = ArtifactWriteContext::new(
            validated.task_artifact_links(context.run_id),
            task_links,
            unix_ms_now().map_err(RequestFailure::poison_without_terminal)?,
        );
        let export_request = EvidenceExportRequest {
            output_path: PathBuf::from(request.output_path()),
            identity: EvidenceExportIdentity {
                run_id: *context.run_id.transport(),
                correlation_id: validated.correlation_id(),
                package: context.package.clone(),
                task_outcome: request.task_outcome(),
                terminal_receipt: terminal_receipt.clone(),
                projection_profile: actingcommand_contract::ProjectionProfile::Forensic,
                retention_class: RetentionClass::DebugFull,
                archive_redaction_state: ArtifactRedactionState::NotRequired,
            },
            events,
            source_capture_summary_sequence,
            pipeline,
            documents,
            archive_context,
        };
        let mut exporter =
            EvidenceExporter::open_with_admission(&self.artifacts).map_err(|error| {
                let mut failure = online_observation::observation_artifact_failure(error);
                failure.terminal = Some(terminal_from_projected(&terminal_receipt));
                failure
            })?;
        let mut sink = RuntimeArtifactEventSink {
            ledger: &self.ledger,
            events: &self.events,
        };
        let receipt = match exporter.export(export_request, &mut sink) {
            Ok(receipt) => receipt,
            Err(error) => {
                let failure_terminal = match self.latest_evidence_export_terminal(
                    validated.correlation_id(),
                    EventType::ArtifactExportFailed,
                ) {
                    Ok(terminal) => terminal,
                    Err(query_failure) => {
                        return Err(RequestFailure::poison(
                            (*query_failure.error).with_related_failure(
                                "export_failure",
                                &RuntimeHostError::artifact(error),
                            ),
                            Some(terminal_from_projected(&terminal_receipt)),
                        ));
                    }
                };
                let mut failure = online_observation::observation_artifact_failure(error);
                failure.terminal =
                    failure_terminal.or_else(|| Some(terminal_from_projected(&terminal_receipt)));
                return Err(failure);
            }
        };
        let response_terminal = self
            .latest_evidence_export_terminal(
                validated.correlation_id(),
                EventType::ArtifactExportCompleted,
            )?
            .ok_or_else(|| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "evidence_export_terminal_missing",
                    "export_evidence",
                    RuntimeErrorCode::RuntimeFatal,
                ))
            })?;
        let output_path = receipt.output_path().to_str().ok_or_else(|| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                "evidence_output_path_invalid",
                "export_evidence",
                RuntimeErrorCode::RuntimeFatal,
            ))
        })?;
        let manifest = receipt.manifest();
        let summary = RuntimeEvidenceExportSummary::new(
            validated.correlation_id(),
            *context.run_id.transport(),
            request.task_outcome(),
            manifest.evidence_completeness,
            output_path,
            receipt.zip_byte_count(),
            receipt.zip_sha256(),
            receipt.manifest_sha256(),
            receipt.archive().project(true),
            RuntimeEvidenceScreenshotCounts {
                captured: manifest.screenshot_counts.captured,
                deduplicated: manifest.screenshot_counts.deduplicated,
                dropped: manifest.screenshot_counts.dropped,
                persisted: manifest.screenshot_counts.persisted,
            },
            terminal_receipt,
        )
        .map_err(|error| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                error.code(),
                "export_evidence",
                RuntimeErrorCode::RuntimeFatal,
            ))
        })?;
        context.completed_export = Some(CompletedEvidenceExport {
            request_output_path: request.output_path().to_string(),
            task_outcome: request.task_outcome(),
            response_terminal,
            summary: summary.clone(),
        });
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(response_terminal),
            result: RuntimeResult::EvidenceExportCompleted {
                summary: Box::new(summary),
            },
        })
    }

    fn authoritative_capture_pipeline_summary(
        &self,
        events: &[actingcommand_contract::ProjectedEvent],
        correlation_id: CorrelationId,
        run_id: actingcommand_contract::RunId,
        terminal_sequence: u64,
    ) -> Result<(u64, CapturePipelineSummary), RequestFailure> {
        let summaries = events
            .iter()
            .filter(|event| event.event_type == EventType::CaptureSummaryCommitted)
            .collect::<Vec<_>>();
        let [event] = summaries.as_slice() else {
            return Err(RequestFailure::request(
                evidence_request_error(if summaries.is_empty() {
                    "evidence_capture_summary_missing"
                } else {
                    "evidence_capture_summary_duplicate"
                }),
                RuntimeReceiptState::Failed,
                None,
            ));
        };
        if event.sequence == 0 || event.sequence >= terminal_sequence {
            return Err(RequestFailure::request(
                evidence_request_error("evidence_capture_summary_not_ready"),
                RuntimeReceiptState::Failed,
                None,
            ));
        }
        if event.links.run_id() != Some(&run_id)
            || event.links.correlation_id() != Some(&correlation_id)
            || event.origin.source() != EventSource::Runtime
            || event.origin.module() != OriginModule::CapturePipeline
            || event.origin.actor() != EventActor::Runtime
        {
            return Err(RequestFailure::request(
                evidence_request_error("evidence_capture_summary_conflict"),
                RuntimeReceiptState::Failed,
                None,
            ));
        }
        let ProjectionPayload::Full(payload) = &event.payload else {
            return Err(RequestFailure::request(
                evidence_request_error("evidence_capture_summary_invalid"),
                RuntimeReceiptState::Failed,
                None,
            ));
        };
        let EventPayload::Capture(CapturePayload::SummaryCommitted(payload)) = payload.as_ref()
        else {
            return Err(RequestFailure::request(
                evidence_request_error("evidence_capture_summary_invalid"),
                RuntimeReceiptState::Failed,
                None,
            ));
        };
        let record = payload.summary();
        record.validate().map_err(|_| {
            RequestFailure::request(
                evidence_request_error("evidence_capture_summary_invalid"),
                RuntimeReceiptState::Failed,
                None,
            )
        })?;
        let mut frames = Vec::new();
        for declared in record.frames() {
            let projected = declared.artifact();
            if projected.correlation_id != Some(correlation_id) || projected.run_id != Some(run_id)
            {
                return Err(RequestFailure::request(
                    evidence_request_error("evidence_capture_identity_mismatch"),
                    RuntimeReceiptState::Failed,
                    None,
                ));
            }
            let verified = self
                .artifacts
                .verify_recovery_reference(projected)
                .map_err(|error| {
                    RequestFailure::request(
                        evidence_request_error(error.code()),
                        RuntimeReceiptState::Failed,
                        None,
                    )
                })?;
            frames.push(PersistedFrameEvidence {
                frame_index: usize::try_from(declared.frame_index()).map_err(|_| {
                    RequestFailure::request(
                        evidence_request_error("evidence_capture_summary_invalid"),
                        RuntimeReceiptState::Failed,
                        None,
                    )
                })?,
                pinned_reason: None,
                artifact: verified.into_reference(),
            });
        }
        let pinned = record
            .pinned()
            .iter()
            .map(|declared| {
                let frame_index = declared
                    .frame_index()
                    .map(usize::try_from)
                    .transpose()
                    .map_err(|_| {
                        RequestFailure::request(
                            evidence_request_error("evidence_capture_summary_invalid"),
                            RuntimeReceiptState::Failed,
                            None,
                        )
                    })?;
                let artifact = match (frame_index, declared.artifact()) {
                    (Some(frame_index), Some(projected)) => {
                        let frame = frames
                            .iter()
                            .find(|frame| frame.frame_index == frame_index)
                            .ok_or_else(|| {
                                RequestFailure::request(
                                    evidence_request_error("evidence_capture_summary_invalid"),
                                    RuntimeReceiptState::Failed,
                                    None,
                                )
                            })?;
                        if frame.artifact.project(true) != *projected {
                            return Err(RequestFailure::request(
                                evidence_request_error("evidence_capture_summary_conflict"),
                                RuntimeReceiptState::Failed,
                                None,
                            ));
                        }
                        Some(frame.artifact.clone())
                    }
                    (_, None) => None,
                    (None, Some(_)) => {
                        return Err(RequestFailure::request(
                            evidence_request_error("evidence_capture_summary_invalid"),
                            RuntimeReceiptState::Failed,
                            None,
                        ));
                    }
                };
                Ok(PinnedFrameEvidence {
                    frame_index,
                    reason: declared.reason(),
                    artifact,
                })
            })
            .collect::<Result<Vec<_>, RequestFailure>>()?;
        let summary = build_capture_pipeline_summary(
            CapturePipelineCounts {
                captured: record.captured(),
                deduplicated: record.deduplicated(),
                dropped: record.dropped(),
                persisted: record.persisted(),
            },
            pinned,
            frames,
        )
        .map_err(|_| {
            RequestFailure::request(
                evidence_request_error("evidence_capture_summary_invalid"),
                RuntimeReceiptState::Failed,
                None,
            )
        })?;
        let rebuilt = capture_summary_record(&summary).map_err(|_| {
            RequestFailure::request(
                evidence_request_error("evidence_capture_summary_invalid"),
                RuntimeReceiptState::Failed,
                None,
            )
        })?;
        if summary.evidence_completeness != record.evidence_completeness() || &rebuilt != record {
            return Err(RequestFailure::request(
                evidence_request_error("evidence_capture_summary_conflict"),
                RuntimeReceiptState::Failed,
                None,
            ));
        }
        Ok((event.sequence, summary))
    }

    fn latest_evidence_export_terminal(
        &self,
        correlation_id: CorrelationId,
        event_type: EventType,
    ) -> Result<Option<TerminalEvent>, RequestFailure> {
        self.ledger
            .project(
                EventQuery {
                    correlation_id: Some(correlation_id),
                    event_type: Some(event_type),
                    ..EventQuery::default()
                },
                actingcommand_contract::ProjectionProfile::Forensic,
            )
            .map(|events| events.last().map(terminal_from_projected))
            .map_err(|_| RequestFailure::poison(ledger_error("query_evidence_terminal"), None))
    }
}

fn runtime_evidence_documents(
    run_id: IssuedRunId,
    task_id: IssuedTaskId,
    task_outcome: TaskOutcome,
    terminal_receipt: &actingcommand_contract::ProjectedEvent,
    events: &[actingcommand_contract::ProjectedEvent],
) -> Result<EvidenceExportDocuments, RequestFailure> {
    let warning_count = events
        .iter()
        .filter(|event| event.severity == EventSeverity::Warning)
        .count();
    let error_count = events
        .iter()
        .filter(|event| matches!(event.severity, EventSeverity::Error | EventSeverity::Fatal))
        .count();
    let result = EvidenceJsonDocument::from_serializable(&serde_json::json!({
        "schema_version": "actingcommand.runtime.evidence-result.v1",
        "run_id": run_id.transport(),
        "task_id": task_id.transport(),
        "task_outcome": task_outcome,
        "terminal_receipt": terminal_receipt,
    }))
    .map_err(|error| {
        RequestFailure::request(
            evidence_request_error(error.code()),
            RuntimeReceiptState::Failed,
            Some(terminal_from_projected(terminal_receipt)),
        )
    })?;
    let diagnostics = EvidenceJsonDocument::from_serializable(&serde_json::json!({
        "schema_version": "actingcommand.runtime.evidence-diagnostics.v1",
        "event_count": events.len(),
        "warning_count": warning_count,
        "error_count": error_count,
    }))
    .map_err(|error| {
        RequestFailure::request(
            evidence_request_error(error.code()),
            RuntimeReceiptState::Failed,
            Some(terminal_from_projected(terminal_receipt)),
        )
    })?;
    EvidenceExportDocuments::new(
        result,
        diagnostics,
        "Runtime-owned Lab debug evidence export",
    )
    .map_err(|error| {
        RequestFailure::request(
            evidence_request_error(error.code()),
            RuntimeReceiptState::Failed,
            Some(terminal_from_projected(terminal_receipt)),
        )
    })
}

fn task_outcome_payload(outcome: TaskOutcome) -> TaskPayloadDraft {
    match outcome {
        TaskOutcome::Success => TaskPayloadDraft::completed(
            EventAction::ArtifactExport,
            EffectDisposition::Performed,
            AuditInput::new(),
        ),
        TaskOutcome::Failure => TaskPayloadDraft::failed(
            EventAction::ArtifactExport,
            DiagnosticCode::RuntimeDiagnostic,
            EffectDisposition::Performed,
            AuditInput::new(),
        ),
        TaskOutcome::Cancelled => TaskPayloadDraft::cancelled(
            EventAction::ArtifactExport,
            EffectDisposition::NotPerformed,
            AuditInput::new(),
        ),
    }
}

fn evidence_request_error(code: &'static str) -> RuntimeHostError {
    RuntimeHostError::request(
        code,
        "export_runtime_evidence",
        RuntimeErrorCode::EvidenceExportFailed,
    )
}
