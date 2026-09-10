// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_artifact_store::verify_projected_read_only;
use actingcommand_contract::{
    SAVED_ARTIFACT_OCR_DEADLINE_MS, SavedArtifactOcrRequest, SavedArtifactOcrResult,
    SavedArtifactOcrSource,
};
use actingcommand_execution_kernel::{ExternallyVerifiedBundle, evaluate_saved_artifact_ocr};
use actingcommand_ledger::{GlobalLedgerEvidence, GlobalLedgerEvidenceConfig};
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
                let event = self.append_event(
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
                )?;
                self.record_required_failure(&error, &event, links)
                    .map_err(RequestFailure::poison_without_terminal)?;
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
        let snapshot = GlobalLedger::open_evidence(
            GlobalLedgerEvidenceConfig::new(&root).with_budget(64 * 1024 * 1024, 100_000, deadline),
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
            source_error(error.code(), detail)
        })?;
        if let Some(error) = verification_error {
            return Err(error);
        }
        if snapshot.corrupt_tail().is_some()
            || !snapshot.read_complete()
            || snapshot
                .writer_metadata()
                .readable()
                .is_none_or(|writer| writer.active())
            || input.source.through_sequence > snapshot.latest_sequence()
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
        check_deadline(deadline)?;
        let image = read_projected_verified(&root, &input.source.artifact)
            .map_err(|error| source_error(error.code(), error))?;
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
        let expected = ExternalExpectedSha256::parse_hex(&input.expected_sha256)
            .map_err(|error| source_error("saved_ocr_package_hash_invalid", error))?;
        let provider = self.execution.vision_provider().ok_or_else(|| {
            source_error(
                "saved_ocr_provider_unavailable",
                "Runtime OCR provider is not configured",
            )
        })?;
        let bundle = ExternallyVerifiedBundle::load_with_vision_provider(
            "saved_artifact_ocr",
            &bytes,
            expected,
            provider,
        )
        .map_err(|error| source_error("saved_ocr_package_invalid", error))?;
        check_deadline(deadline)?;
        let observation =
            evaluate_saved_artifact_ocr(&bundle, &image, dimensions, &input.target_id, deadline)
                .map_err(|error| source_error("saved_ocr_evaluation_failed", error))?;
        check_deadline(deadline)?;
        let links = request.event_links(None, None, None);
        let report = serde_json::json!({
            "schema_version": "actingcommand.runtime.saved-artifact-ocr.v1",
            "request_id": request.request_id(), "correlation_id": request.correlation_id(),
            "source": input.source, "source_sensitivity": source_sensitivity,
            "source_ledger": {
                "canonical_root": source_ledger,
                "writer_owner_id": snapshot.writer_metadata().readable().map(|writer| writer.owner_id()),
                "through_event_id": snapshot.events()[usize::try_from(input.source.through_sequence - 1)
                    .map_err(|error| source_error("saved_source_position_invalid", error))?].event_id(),
                "storage_backend": snapshot.backend(),
                "read_complete": snapshot.read_complete(),
                "storage_snapshot": snapshot.segment().map(|source| source.storage_snapshot()),
            },
            "package_sha256": input.expected_sha256, "target_id": input.target_id,
            "observation": observation,
        });
        let report_bytes = serde_json::to_vec(&report)
            .map_err(|error| source_error("saved_ocr_serialization_failed", error))?;
        let mut sink = online_observation::ObservationArtifactSink {
            ledger: &self.ledger,
            events: &self.events,
            verified: None,
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
        if !event
            .artifacts()
            .iter()
            .any(|artifact| artifact.project(true) == source.artifact)
            || event.links().run_id() != captured.links().run_id()
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
