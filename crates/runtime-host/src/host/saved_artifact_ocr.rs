// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_artifact_store::verify_projected_read_only;
use actingcommand_contract::{
    SAVED_ARTIFACT_OCR_DEADLINE_MS, SavedArtifactOcrRequest, SavedArtifactOcrResult,
    SavedArtifactOcrSource,
};
use actingcommand_execution_kernel::{ExternallyVerifiedBundle, evaluate_saved_artifact_ocr};
use actingcommand_ledger::{
    GlobalLedgerEvidence, GlobalLedgerEvidenceConfig, GlobalLedgerReadExtent,
};
use std::time::{Duration, Instant};

impl HostShared {
    pub(super) fn recognize_artifact(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        input: &SavedArtifactOcrRequest,
    ) -> Result<OperationSuccess, RequestFailure> {
        let links = request.event_links(None, None, None);
        let deadline = Instant::now()
            .checked_add(Duration::from_millis(SAVED_ARTIFACT_OCR_DEADLINE_MS))
            .ok_or_else(|| source_error("saved_ocr_deadline_overflow", "deadline overflow"))?;
        self.append_event(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Recognition,
            EventActor::Runtime,
            links.clone(),
            RecognitionPayloadDraft::requested(
                EventAction::RuntimeRecognizeArtifact,
                AuditInput::new(),
            ),
        )?;
        let work = self.recognize_artifact_inner(request, input, deadline);
        match work {
            Ok(result) => Ok(result),
            Err(error) => {
                let ppocr = error.has_ppocr_diagnostics();
                if ppocr && error.projection().code == RuntimeErrorCode::LedgerFailure {
                    return Err(RequestFailure::poison_without_terminal(error));
                }
                let event = self
                    .append_event(
                        EventSeverity::Error,
                        EventSource::Runtime,
                        OriginModule::Recognition,
                        EventActor::Runtime,
                        links.clone(),
                        RecognitionPayloadDraft::failed(
                            EventAction::RuntimeRecognizeArtifact,
                            DiagnosticCode::RuntimeDiagnostic,
                            EffectDisposition::NotPerformed,
                            AuditInput::new(),
                        ),
                    )
                    .map_err(|failure| {
                        if ppocr {
                            RequestFailure::poison_without_terminal(
                                error.clone().with_complete_failure(
                                    crate::error::RuntimeFailureRelation::LifecycleRecord,
                                    *failure.error,
                                ),
                            )
                        } else {
                            failure
                        }
                    })?;
                self.record_required_failure(&error, &event, links)
                    .map_err(|writer| {
                        RequestFailure::poison_without_terminal(if ppocr {
                            error.clone().with_complete_failure(
                                crate::error::RuntimeFailureRelation::LifecycleRecord,
                                writer,
                            )
                        } else {
                            writer
                        })
                    })?;
                if error.is_fatal() {
                    Err(RequestFailure::poison(error, Some(terminal(&event))))
                } else {
                    Err(RequestFailure::request(
                        error,
                        RuntimeReceiptState::Failed,
                        Some(terminal(&event)),
                    ))
                }
            }
        }
    }

    fn recognize_artifact_inner(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        input: &SavedArtifactOcrRequest,
        deadline: Instant,
    ) -> RuntimeHostResult<OperationSuccess> {
        let root = fs::canonicalize(&input.source.state_root)
            .map_err(|error| source_error("saved_source_root_unavailable", error))?;
        let source_ledger = fs::canonicalize(root.join("ledger"))
            .map_err(|error| source_error("saved_source_ledger_unavailable", error))?;
        let destination_ledger = fs::canonicalize(self.artifacts.root().join("ledger"))
            .map_err(|error| source_error("saved_target_ledger_unavailable", error))?;
        if same_root(&source_ledger, &destination_ledger) {
            return Err(source_error(
                "saved_source_is_target",
                "source and target ledger must differ",
            ));
        }
        // The existing writer lock prevents another Runtime from mutating this source.
        let source_lock = fs::File::open(source_ledger.join("writer.lock"))
            .map_err(|error| source_error("saved_source_lock_unavailable", error))?;
        source_lock
            .try_lock_shared()
            .map_err(|error| source_error("saved_source_not_frozen", error))?;
        check_deadline(deadline)?;
        let mut verification_error = None;
        let mut verified_bytes = 0_u64;
        // Workflow #363: an SQLite source is read only through the declared prefix, bounded
        // by that prefix and this request's deadline, and no artifact is read at open; the
        // selected frame is the only material read (below). A segment source is read whole.
        let ledger_started = Instant::now();
        let snapshot = GlobalLedger::open_evidence(
            GlobalLedgerEvidenceConfig::new(&root)
                .with_deadline(deadline)
                .sqlite_material_not_read()
                .sqlite_prefix(input.source.through_sequence),
            // Called only for a segment source: the SQLite record path reads no material.
            |reference| {
                let result = (|| {
                    check_deadline(deadline)?;
                    verified_bytes = verified_bytes
                        .checked_add(reference.byte_count)
                        .filter(|total| *total <= 4 * 1024 * 1024 * 1024)
                        .ok_or_else(|| {
                            source_error(
                                "saved_source_artifact_budget",
                                "source verification byte budget exceeded",
                            )
                        })?;
                    verify_projected_read_only(&root, reference)
                        .map_err(|error| source_error(error.code(), error))
                })();
                match result {
                    Ok(verified) => Some(verified),
                    Err(error) => {
                        verification_error = Some(error);
                        None
                    }
                }
            },
        )
        .map_err(|error| {
            let detail = match error.detail() {
                Some(detail) => format!("{error}: {detail}"),
                None => error.to_string(),
            };
            // A declared prefix beyond the authenticated source head is an invalid prefix.
            let code = match error.code() {
                "ledger_prefix_beyond_head" => "saved_source_incomplete",
                code => code,
            };
            source_error(code, detail)
        })?;
        let ledger_open = ledger_started.elapsed();
        if let Some(error) = verification_error {
            return Err(error);
        }
        let extent = snapshot.read_extent();
        if snapshot.corrupt_tail().is_some()
            || !snapshot.read_complete()
            || snapshot
                .writer_metadata()
                .readable()
                .is_none_or(|writer| writer.active())
            || input.source.through_sequence > extent.head_sequence
        {
            return Err(source_error(
                "saved_source_incomplete",
                "source must have a complete closed ledger and a valid frozen prefix",
            ));
        }
        let captured = prove_source(&snapshot, &input.source)?;
        let EventPayload::Capture(CapturePayload::Completed(frame)) = captured.payload() else {
            return Err(source_error(
                "saved_source_capture_invalid",
                "capture completion payload missing",
            ));
        };
        let dimensions = (frame.frame_width(), frame.frame_height());
        let source_sensitivity = captured.sensitivity();
        // Workflow #363 (review F5): take out what the report needs, then release the prefix
        // evidence before the PNG read, so its memory does not stay through package load and
        // OCR.
        let writer_owner_id = snapshot
            .writer_metadata()
            .readable()
            .map(|writer| writer.owner_id().to_owned());
        let through_event_id =
            *snapshot.events()[usize::try_from(input.source.through_sequence - 1)
                .map_err(|error| source_error("saved_source_position_invalid", error))?]
            .event_id();
        let storage_backend = snapshot.backend();
        let read_complete = snapshot.read_complete();
        let storage_snapshot = snapshot
            .segment()
            .map(|source| source.storage_snapshot().clone());
        let material_checked = snapshot.material_checked();
        drop(snapshot);
        check_deadline(deadline)?;
        // The only material read: length and SHA-256 are verified as it is read.
        let png_started = Instant::now();
        let image = read_projected_verified(&root, &input.source.artifact).map_err(|error| {
            unavailable_source_artifact(&root, &input.source, extent, &error, deadline)
        })?;
        let png_read = png_started.elapsed();
        let package_started = Instant::now();
        let bundle = if input.expected_sha256.is_directory_source() {
            let provider = self.execution()?.vision_provider().ok_or_else(|| {
                source_error(
                    "saved_ocr_provider_unavailable",
                    "Runtime OCR provider is not configured",
                )
            })?;
            ExternallyVerifiedBundle::load_path(
                "saved_artifact_ocr",
                Path::new(&input.package_path),
                &input.expected_sha256,
                false,
                Some(provider),
                deadline,
            )
            .map_err(|error| source_error("saved_ocr_package_invalid", error))?
        } else {
            let package = fs::File::open(&input.package_path)
                .map_err(|error| source_error("saved_ocr_package_open_failed", error))?;
            let mut bytes = Vec::new();
            package
                .take(DEFAULT_MAX_COMPRESSED_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|error| source_error("saved_ocr_package_read_failed", error))?;
            if bytes.len() as u64 > DEFAULT_MAX_COMPRESSED_BYTES {
                return Err(source_error(
                    "saved_ocr_package_limit",
                    "compressed package limit exceeded",
                ));
            }
            let expected = ExternalExpectedSha256::parse_hex(
                input.expected_sha256.legacy_sha256().ok_or_else(|| {
                    source_error(
                        "saved_ocr_package_hash_invalid",
                        "legacy package identity missing",
                    )
                })?,
            )
            .map_err(|error| source_error("saved_ocr_package_hash_invalid", error))?;
            let provider = self.execution()?.vision_provider().ok_or_else(|| {
                source_error(
                    "saved_ocr_provider_unavailable",
                    "Runtime OCR provider is not configured",
                )
            })?;
            ExternallyVerifiedBundle::load_with_vision_provider(
                "saved_artifact_ocr",
                &bytes,
                expected,
                provider,
            )
            .map_err(|error| source_error("saved_ocr_package_invalid", error))?
        };
        let package_load = package_started.elapsed();
        check_deadline(deadline)?;
        let ocr_started = Instant::now();
        let evaluated =
            evaluate_saved_artifact_ocr(&bundle, &image, dimensions, &input.target_id, deadline);
        let ocr = ocr_started.elapsed();
        let reports = match &evaluated {
            Ok(value) => &value.ppocr_diagnostics,
            Err(error) => error.ppocr_diagnostics(),
        };
        let archive = self.archive_ppocr_diagnostics(
            reports,
            ppocr_diagnostic::PpocrArchiveContext {
                links: request.event_links(None, None, None),
                artifact_links: request.artifact_links(),
                phase: "saved_artifact_ocr",
                target: Some(&input.target_id),
                saved_source: Some(&input.source),
                drain: evaluated.is_err(),
            },
        );
        let observation = match evaluated {
            Ok(value) => {
                archive?;
                value
            }
            Err(error) => {
                let primary = ppocr_diagnostic::attach_ppocr_source(
                    ppocr_diagnostic::attach_ppocr_failure(
                        source_error("saved_ocr_evaluation_failed", &error),
                        error.ppocr_diagnostics(),
                        error.to_string(),
                    ),
                    crate::error::PpocrFailureSource::Saved {
                        message: error.message().to_owned(),
                        conflicting_pages: error.conflicting_pages().map(<[_]>::to_vec),
                    },
                );
                return Err(match archive {
                    Ok(()) => primary,
                    Err(secondary) => primary.with_complete_failure(
                        crate::error::RuntimeFailureRelation::DiagnosticArchive,
                        secondary,
                    ),
                });
            }
        };
        check_deadline(deadline)?;
        let links = request.event_links(None, None, None);
        // A segment source verified all referenced material at open.
        let material_scope = if material_checked {
            "all_referenced"
        } else {
            "requested_frame"
        };
        let report = serde_json::json!({
            "schema_version": "actingcommand.runtime.saved-artifact-ocr.v1",
            "request_id": request.request_id(), "correlation_id": request.correlation_id(),
            "source": input.source, "source_sensitivity": source_sensitivity,
            "source_ledger": {
                "canonical_root": source_ledger,
                "writer_owner_id": writer_owner_id,
                "through_event_id": through_event_id,
                "storage_backend": storage_backend,
                // Physical completeness of segment files; an SQLite source reports true.
                "read_complete": read_complete,
                "storage_snapshot": storage_snapshot,
                // Workflow #363: what this request read from the source.
                "source_read": {
                    "declared_through_sequence": input.source.through_sequence,
                    "read_through_sequence": extent.through_sequence,
                    "head_sequence": extent.head_sequence,
                    "events_read": extent.event_count,
                    "ledger_bytes_read": extent.ledger_bytes,
                    "material_scope": material_scope,
                },
            },
            "package_sha256": input.expected_sha256, "target_id": input.target_id,
            "observation": observation,
            // Workflow #363: wall time per phase, recorded for the deadline horizon.
            "phase_ms": {
                "ledger_open": milliseconds(ledger_open),
                "ledger_sql_read": extent.phases.map(|phases| milliseconds(phases.sql_read)),
                "ledger_verify": extent.phases.map(|phases| milliseconds(phases.verify)),
                "ledger_retention_restore":
                    extent.phases.map(|phases| milliseconds(phases.retention_restore)),
                "png_read": milliseconds(png_read),
                "package_load": milliseconds(package_load),
                "ocr": milliseconds(ocr),
            },
        });
        let report_bytes = serde_json::to_vec(&report)
            .map_err(|error| source_error("saved_ocr_serialization_failed", error))?;
        let mut sink = online_observation::ObservationArtifactSink {
            ledger: &self.ledger,
            events: &self.events,
            verified: None,
            frame_retention: None,
        };
        let stored = self
            .artifacts
            .put(
                ArtifactWriteRequest::new(
                    ArtifactKind::DiagnosticJson,
                    &report_bytes,
                    ArtifactWriteContext::new(
                        request.artifact_links(),
                        links.clone(),
                        unix_ms_now()?,
                    ),
                    ArtifactIssuePolicy::new(
                        ArtifactProducer::ArtifactStore,
                        RetentionClass::DebugFull,
                        ArtifactRedactionState::Pending,
                    ),
                ),
                &mut sink,
            )
            .map_err(RuntimeHostError::artifact)?;
        let verified = sink.verified.ok_or_else(|| {
            source_error(
                "saved_ocr_verified_event_missing",
                "artifact verification event missing",
            )
        })?;
        let event = self.append_event_raw(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Recognition,
            EventActor::Runtime,
            links,
            RecognitionPayloadDraft::completed(
                EventAction::RuntimeRecognizeArtifact,
                EffectDisposition::Performed,
                dimensions.0,
                dimensions.1,
                RecognitionVerdict::FrameDecoded,
                AuditInput::new(),
            ),
        )?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(terminal(&event)),
            result: RuntimeResult::ArtifactRecognized {
                result: Box::new(SavedArtifactOcrResult {
                    source: input.source.clone(),
                    target_id: input.target_id.clone(),
                    artifact: stored.reference().project(true),
                    verified: terminal(&verified),
                }),
            },
        })
    }
}

fn prove_source<'a>(
    snapshot: &'a GlobalLedgerEvidence,
    source: &SavedArtifactOcrSource,
) -> RuntimeHostResult<&'a PersistedEvent> {
    let locate = |position: TerminalEvent, kind: EventType| {
        snapshot
            .events()
            .get(usize::try_from(position.sequence - 1).unwrap_or(usize::MAX))
            .filter(|event| {
                event.sequence() == position.sequence
                    && *event.event_id() == position.event_id
                    && event.event_type() == kind
                    && event.links().frame_id() == Some(&source.frame_id)
            })
            .ok_or_else(|| {
                source_error(
                    "saved_source_event_mismatch",
                    "source event identity, kind or frame does not match",
                )
            })
    };
    let created = locate(source.created, EventType::ArtifactCreated)?;
    let verified = locate(source.verified, EventType::ArtifactVerified)?;
    let captured = locate(source.captured, EventType::CaptureCompleted)?;
    for event in [created, verified] {
        let artifact = event
            .artifacts()
            .iter()
            .find(|artifact| artifact.project(true) == source.artifact)
            .ok_or_else(|| {
                source_error(
                    "saved_source_artifact_mismatch",
                    "source artifact identity does not match",
                )
            })?;
        match artifact.availability() {
            actingcommand_ledger::ArtifactAvailability::Available(_) => {}
            // Workflow #363: material was deliberately not read at open; the selected frame
            // alone is verified by length and SHA-256 when it is read.
            actingcommand_ledger::ArtifactAvailability::Unrecorded
                if !snapshot.material_checked() => {}
            actingcommand_ledger::ArtifactAvailability::Evicted(_) => {
                return Err(source_error(
                    "saved_source_artifact_evicted",
                    "source pixels are unavailable under the recorded eviction proof",
                ));
            }
            actingcommand_ledger::ArtifactAvailability::PendingEviction(_) => {
                return Err(source_error(
                    "saved_source_artifact_pending_eviction",
                    "source pixels await the original Runtime retention recovery",
                ));
            }
            actingcommand_ledger::ArtifactAvailability::FailedEviction(_) => {
                return Err(source_error(
                    "saved_source_artifact_eviction_failed",
                    "source material has an unresolved recorded deletion failure",
                ));
            }
            actingcommand_ledger::ArtifactAvailability::Unrecorded => {
                return Err(source_error(
                    "saved_source_artifact_unverified",
                    "source pixels have not been verified",
                ));
            }
        }
        if event.links().run_id() != captured.links().run_id()
            || event.links().correlation_id() != captured.links().correlation_id()
            || event.links().request_id() != captured.links().request_id()
        {
            return Err(source_error(
                "saved_source_artifact_mismatch",
                "source artifact and frame completion are not the same observation",
            ));
        }
    }
    if source.artifact.run_id.as_ref() != captured.links().run_id()
        || source.artifact.correlation_id.as_ref() != captured.links().correlation_id()
    {
        return Err(source_error(
            "saved_source_links_mismatch",
            "artifact source links do not match capture",
        ));
    }
    Ok(captured)
}

/// Workflow #363: a prefix read does not see an eviction recorded after the prefix. When the
/// selected PNG cannot be read and the source head lies beyond the prefix, the source is
/// reopened once through its authenticated head, still reading no material, so that an
/// authorized eviction is reported as such and not as file loss.
fn unavailable_source_artifact(
    root: &Path,
    source: &SavedArtifactOcrSource,
    extent: GlobalLedgerReadExtent,
    error: &actingcommand_artifact_store::ArtifactStoreError,
    deadline: Instant,
) -> RuntimeHostError {
    let identity = serde_json::to_string(&source.artifact.artifact_id).unwrap_or_default();
    let read = format!(
        "source artifact {identity} ({}): {error}",
        source.artifact.sha256
    );
    let head = extent.head_sequence;
    if extent.through_sequence >= head {
        return source_error(
            error.code(),
            format!("{read}; no eviction recorded through head {head}"),
        );
    }
    let current = match GlobalLedger::open_evidence(
        GlobalLedgerEvidenceConfig::new(root)
            .with_deadline(deadline)
            .sqlite_material_not_read()
            .sqlite_prefix(head),
        |_| None,
    ) {
        Ok(current) => current,
        Err(reopen) => {
            return source_error(
                error.code(),
                format!(
                    "{read}; eviction check through head {head} failed: {reopen}: {}",
                    reopen.detail().unwrap_or("no detail")
                ),
            );
        }
    };
    let availability = source
        .created
        .sequence
        .checked_sub(1)
        .and_then(|index| usize::try_from(index).ok())
        .and_then(|index| current.events().get(index))
        .filter(|event| *event.event_id() == source.created.event_id)
        .and_then(|event| {
            event
                .artifacts()
                .iter()
                .find(|artifact| artifact.project(true) == source.artifact)
        })
        .map(|artifact| artifact.availability());
    let recorded = |position: u64| {
        format!(
            "eviction recorded at {position} after through_sequence {}; {read}",
            source.through_sequence
        )
    };
    match availability {
        Some(actingcommand_ledger::ArtifactAvailability::Evicted(proof)) => source_error(
            "saved_source_artifact_evicted",
            recorded(
                proof
                    .outcome
                    .map_or(proof.intent.sequence, |outcome| outcome.sequence),
            ),
        ),
        Some(actingcommand_ledger::ArtifactAvailability::PendingEviction(proof)) => source_error(
            "saved_source_artifact_pending_eviction",
            recorded(proof.intent.sequence),
        ),
        Some(actingcommand_ledger::ArtifactAvailability::FailedEviction(proof)) => source_error(
            "saved_source_artifact_eviction_failed",
            recorded(
                proof
                    .outcome
                    .map_or(proof.intent.sequence, |outcome| outcome.sequence),
            ),
        ),
        _ => source_error(
            error.code(),
            format!("{read}; no eviction recorded through head {head}"),
        ),
    }
}

fn milliseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn same_root(left: &Path, right: &Path) -> bool {
    if cfg!(windows) {
        left.as_os_str()
            .as_encoded_bytes()
            .eq_ignore_ascii_case(right.as_os_str().as_encoded_bytes())
    } else {
        left == right
    }
}

fn check_deadline(deadline: Instant) -> RuntimeHostResult<()> {
    if Instant::now() >= deadline {
        Err(source_error(
            "saved_ocr_deadline_exceeded",
            "saved artifact recognition deadline exceeded",
        ))
    } else {
        Ok(())
    }
}

fn source_error(code: &'static str, detail: impl ToString) -> RuntimeHostError {
    RuntimeHostError::request(code, "recognize_artifact", RuntimeErrorCode::InvalidRequest)
        .with_native_detail(detail.to_string())
}
