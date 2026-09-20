// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::error::{PpocrFailureSource, RuntimeFailureRelation};
use actingcommand_artifact_store::ArtifactStream;
use actingcommand_contract::{
    PPOCR_MAX_REPORT_JSON_BYTES, PpocrDiagnostics, PpocrNodePlacementDiagnostic,
    SavedArtifactOcrSource,
};
use std::io::{BufWriter, Write};

pub(super) struct PpocrArchiveContext<'a> {
    pub(super) links: EventLinksDraft,
    pub(super) artifact_links: ArtifactLinksDraft,
    pub(super) phase: &'static str,
    pub(super) target: Option<&'a str>,
    pub(super) saved_source: Option<&'a SavedArtifactOcrSource>,
    pub(super) drain: bool,
}

#[derive(serde::Serialize)]
struct PpocrDiagnosticArtifact<'a> {
    schema_version: &'static str,
    request_id: Option<&'a RequestId>,
    correlation_id: Option<&'a CorrelationId>,
    task_id: Option<&'a TaskId>,
    run_id: Option<&'a RunId>,
    instance_id: Option<&'a InstanceId>,
    frame_id: Option<&'a FrameId>,
    saved_source: Option<&'a SavedArtifactOcrSource>,
    phase: &'static str,
    target: Option<&'a str>,
    archived_at_unix_ms: u64,
    report: &'a PpocrNodePlacementDiagnostic,
}

struct ReportWriter<'a> {
    stream: &'a mut ArtifactStream,
    written: usize,
}

impl Write for ReportWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let next = self
            .written
            .checked_add(bytes.len())
            .filter(|value| *value <= PPOCR_MAX_REPORT_JSON_BYTES)
            .ok_or_else(|| std::io::Error::other("PPOCR diagnostic artifact exceeds 112 MiB"))?;
        self.stream.write_all(bytes)?;
        self.written = next;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.stream.flush()
    }
}

/// Retains the current call's reports and full original message on its existing error.
pub(super) fn attach_ppocr_failure(
    mut error: RuntimeHostError,
    reports: &PpocrDiagnostics,
    message: String,
) -> RuntimeHostError {
    if !reports.is_empty() {
        error.lifecycle.ppocr_diagnostics = reports.clone();
        if let Some(complete) = &mut error.lifecycle.complete_failure {
            complete.primary =
                attach_ppocr_failure(complete.primary.clone(), reports, message.clone());
        }
        error.lifecycle.ppocr_message = Some(match error.lifecycle.ppocr_message.take() {
            Some(original) => format!("{message}; {original}"),
            None => message,
        });
    }
    error
}

fn ppocr_artifact_error(error: ArtifactStoreError) -> RuntimeHostError {
    let message = format!("{error}: {}; {error:?}", error.detail());
    let mut failure = RuntimeHostError::artifact(error.clone());
    failure.lifecycle.ppocr_message = Some(message);
    failure.lifecycle.ppocr_artifact_failure = Some(Arc::new(error));
    failure
}

pub(super) fn attach_ppocr_source(
    mut error: RuntimeHostError,
    source: PpocrFailureSource,
) -> RuntimeHostError {
    if error.lifecycle.ppocr_diagnostics.is_empty() {
        return error;
    }
    let encoded = serde_json::to_string(&source);
    let source = Arc::new(source);
    error.lifecycle.ppocr_source = Some(source.clone());
    if let Some(complete) = &mut error.lifecycle.complete_failure {
        complete.primary.lifecycle.ppocr_source = Some(source);
    }
    match encoded {
        Ok(message) => {
            let reports = error.lifecycle.ppocr_diagnostics.clone();
            attach_ppocr_failure(error, &reports, message)
        }
        Err(cause) => {
            let mut secondary = RuntimeHostError::fatal(
                "ppocr_original_error_encode_failed",
                "archive_ppocr_diagnostic",
                RuntimeErrorCode::RuntimeFatal,
            );
            secondary.lifecycle.ppocr_message = Some(cause.to_string());
            error.with_complete_failure(RuntimeFailureRelation::DiagnosticArchive, secondary)
        }
    }
}

pub(super) fn task_ppocr_failure(
    code: &'static str,
    message: String,
    reports: &PpocrDiagnostics,
) -> RuntimeHostError {
    attach_ppocr_failure(
        RuntimeHostError::request(
            code,
            "run_contained_task",
            RuntimeErrorCode::BackendOperationFailed,
        ),
        reports,
        message,
    )
}

pub(super) fn finish_task_ppocr_record(
    result: Result<(), RequestFailure>,
    reports: &PpocrDiagnostics,
    primary: Option<RuntimeHostError>,
) -> Result<(), RequestFailure> {
    if reports.is_empty() {
        return result;
    }
    result.map_err(|mut failure| {
        let message = failure.error.complete_message();
        let error = attach_ppocr_failure(*failure.error, reports, message);
        failure.error = Box::new(match primary {
            Some(primary) => {
                primary.with_complete_failure(RuntimeFailureRelation::DiagnosticArchive, error)
            }
            None => error,
        });
        failure.poison_runtime |= failure.error.is_fatal();
        failure
    })
}

impl HostShared {
    pub(super) fn archive_online_ppocr_result(
        &self,
        result: Result<
            actingcommand_execution_kernel::EvaluatedPageObservation,
            actingcommand_execution_kernel::OnlineObservationError,
        >,
        links: EventLinksDraft,
        artifact_links: ArtifactLinksDraft,
        phase: &'static str,
    ) -> Result<actingcommand_execution_kernel::EvaluatedPageObservation, RequestFailure> {
        let reports = match &result {
            Ok(value) => &value.ppocr_diagnostics,
            Err(error) => error.ppocr_diagnostics(),
        };
        let archive = self.archive_ppocr_diagnostics(
            reports,
            PpocrArchiveContext {
                links: links.clone(),
                artifact_links,
                phase,
                target: None,
                saved_source: None,
                drain: result.is_err(),
            },
        );
        match result {
            Ok(value) => match archive {
                Ok(()) => Ok(value),
                Err(error) => {
                    let error = attach_ppocr_source(
                        error,
                        PpocrFailureSource::Observation {
                            status: value.status,
                            facts: value.private_facts,
                        },
                    );
                    Err(self.observation_failure(error, links, RuntimeReceiptState::Failed))
                }
            },
            Err(error) => {
                let reports = error.ppocr_diagnostics().clone();
                let message = error.cause().to_owned();
                let primary = attach_ppocr_failure(
                    online_observation::observation_kernel_error(error),
                    &reports,
                    message,
                );
                let error = match archive {
                    Ok(()) => primary,
                    Err(secondary) => primary.with_complete_failure(
                        RuntimeFailureRelation::DiagnosticArchive,
                        secondary,
                    ),
                };
                Err(self.observation_failure(error, links, RuntimeReceiptState::Failed))
            }
        }
    }

    pub(super) fn archive_ppocr_diagnostics(
        &self,
        reports: &PpocrDiagnostics,
        context: PpocrArchiveContext<'_>,
    ) -> RuntimeHostResult<()> {
        for (index, report) in reports.iter().enumerate() {
            // Only repeated references within this one returned batch share a report.
            if reports[..index]
                .iter()
                .any(|prior| Arc::ptr_eq(prior, report))
            {
                continue;
            }
            let result = (|| {
                report.validate().map_err(|message| {
                    RuntimeHostError::fatal(
                        "ppocr_diagnostic_invalid",
                        "archive_ppocr_diagnostic",
                        RuntimeErrorCode::RuntimeFatal,
                    )
                    .with_native_detail(message)
                })?;
                let timestamp = unix_ms_now()?;
                let mut write_context = ArtifactWriteContext::new(
                    context.artifact_links.clone(),
                    context.links.clone(),
                    timestamp,
                );
                if context.drain {
                    write_context = write_context.for_drain();
                }
                let mut stream = self
                    .artifacts
                    .begin_stream(
                        ArtifactKind::DiagnosticJson,
                        write_context,
                        ArtifactIssuePolicy::new(
                            ArtifactProducer::ArtifactStore,
                            RetentionClass::DebugFull,
                            ArtifactRedactionState::Pending,
                        ),
                    )
                    .map_err(ppocr_artifact_error)?;
                let document = PpocrDiagnosticArtifact {
                    schema_version: "actingcommand.runtime.ppocr-diagnostic.v1",
                    request_id: context.links.request_id(),
                    correlation_id: context.links.correlation_id(),
                    task_id: context.links.task_id(),
                    run_id: context.links.run_id(),
                    instance_id: context.links.instance_id(),
                    frame_id: context.links.frame_id(),
                    saved_source: context.saved_source,
                    phase: context.phase,
                    target: context.target,
                    archived_at_unix_ms: timestamp,
                    report: report.as_ref(),
                };
                let encoded = {
                    let mut bounded = ReportWriter {
                        stream: &mut stream,
                        written: 0,
                    };
                    let mut buffered = BufWriter::with_capacity(64 * 1024, &mut bounded);
                    let result = serde_json::to_writer(&mut buffered, &document)
                        .map_err(|error| error.to_string())
                        .and_then(|()| buffered.flush().map_err(|error| error.to_string()));
                    // Do not retry a failed write during BufWriter::drop.
                    let (_, pending) = buffered.into_parts();
                    drop(pending);
                    result
                };
                if let Err(message) = encoded {
                    let mut error = RuntimeHostError::fatal(
                        "ppocr_diagnostic_write_failed",
                        "archive_ppocr_diagnostic",
                        RuntimeErrorCode::RuntimeFatal,
                    )
                    .with_native_detail(message.clone());
                    error.lifecycle.ppocr_message = Some(message);
                    return Err(match stream.abort() {
                        Ok(()) => error,
                        Err(cleanup) => error.with_complete_failure(
                            RuntimeFailureRelation::DiagnosticArchive,
                            ppocr_artifact_error(cleanup),
                        ),
                    });
                }
                let mut sink = online_observation::ObservationArtifactSink {
                    ledger: &self.ledger,
                    events: &self.events,
                    verified: None,
                    frame_retention: None,
                };
                let artifact = self
                    .artifacts
                    .seal_stream(stream, &mut sink)
                    .map_err(|error| {
                        if error.code() == "artifact_event_append_failed" {
                            ledger_error("archive_ppocr_diagnostic").with_complete_failure(
                                RuntimeFailureRelation::DiagnosticArchive,
                                ppocr_artifact_error(error),
                            )
                        } else {
                            ppocr_artifact_error(error)
                        }
                    })?;
                if sink
                    .verified
                    .as_ref()
                    .is_none_or(|event| event.artifacts() != [artifact.reference().clone()])
                {
                    return Err(RuntimeHostError::fatal(
                        "ppocr_diagnostic_verified_event_missing",
                        "archive_ppocr_diagnostic",
                        RuntimeErrorCode::RuntimeFatal,
                    ));
                }
                Ok(())
            })();
            if let Err(error) = result {
                let message = format!(
                    "PPOCR diagnostic archive failed: report {}/{}, record_type={}, model_role={}; {}",
                    index + 1,
                    reports.len(),
                    report.record_type,
                    report.model_role,
                    error.complete_message()
                );
                return Err(attach_ppocr_failure(error.into_fatal(), reports, message));
            }
        }
        Ok(())
    }
}

impl RuntimeContainedTask<'_> {
    pub(super) fn archive_task_ppocr_diagnostics(
        &self,
        reports: &PpocrDiagnostics,
        phase: &'static str,
        target: Option<&str>,
        primary: Option<RuntimeHostError>,
    ) -> Result<(), RequestFailure> {
        let mut links = self.links();
        let mut artifact_links = self.request.task_artifact_links(self.run_id);
        if let Some(frame) = self.last_frame_id {
            links = links.with_frame_id(frame);
            artifact_links = artifact_links.with_frame_id(frame);
        }
        self.host
            .archive_ppocr_diagnostics(
                reports,
                PpocrArchiveContext {
                    links,
                    artifact_links,
                    phase,
                    target,
                    saved_source: None,
                    drain: primary.is_some(),
                },
            )
            .map_err(|failure| {
                RequestFailure::poison_without_terminal(match primary {
                    Some(primary) => primary
                        .with_complete_failure(RuntimeFailureRelation::DiagnosticArchive, failure),
                    None => failure,
                })
            })
    }
}
