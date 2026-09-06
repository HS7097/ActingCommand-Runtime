// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::page_projection::{FrameIdentity, FrameKind};
use actingcommand_contract::{
    ContainedObservationEvidence, ContainedObservationRequest, ContainedPageObservation,
    MAX_OBSERVATION_ARTIFACT_BYTES, ONLINE_OBSERVATION_SCHEMA, PageObservationStatus,
};
use actingcommand_execution_kernel::{OnlineObservationError, PreparedPageObservation};

impl HostShared {
    pub(super) fn observe_contained_page(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        instance_alias: &str,
        input: &ContainedObservationRequest,
    ) -> Result<OperationSuccess, RequestFailure> {
        let resolved = self.resolve_instance(instance_alias)?;
        self.append_request_lifecycle(
            original,
            request,
            resolved.instance_id(),
            EventAction::RuntimeReadonlyObserve,
            None,
        )?;
        // Admission does not open a capture backend or request an input lease.
        let prepared = (|| {
            let path = Path::new(input.package_path());
            if !path.is_absolute() {
                return Err(observation_admission_error(
                    "observation_path_not_absolute",
                    "open_observation_package",
                    "absolute package path required",
                ));
            }
            let file = fs::File::open(path).map_err(|error| {
                observation_admission_error(
                    "observation_package_open_failed",
                    "open_observation_package",
                    error,
                )
            })?;
            let mut bytes = Vec::new();
            file.take(DEFAULT_MAX_COMPRESSED_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|error| {
                    observation_admission_error(
                        "observation_package_read_failed",
                        "read_observation_package",
                        error,
                    )
                })?;
            if bytes.len() as u64 > DEFAULT_MAX_COMPRESSED_BYTES {
                return Err(observation_admission_error(
                    "observation_package_limit_exceeded",
                    "read_observation_package",
                    "compressed package limit exceeded",
                ));
            }
            let expected =
                ExternalExpectedSha256::parse_hex(input.expected_sha256()).map_err(|error| {
                    observation_admission_error(
                        "observation_hash_invalid",
                        "admit_contained_observation",
                        error,
                    )
                })?;
            PreparedPageObservation::load(
                instance_alias,
                &bytes,
                expected,
                input.targets(),
                self.execution.vision_provider(),
            )
            .map_err(observation_kernel_error)
        })()
        .map_err(|error| {
            self.observation_failure(
                error,
                request.event_links(Some(resolved.instance_id()), None, None),
                RuntimeReceiptState::Denied,
            )
        })?;
        self.append_scheduler_admitted(request, &resolved, None)?;
        let completed = self.capture_readonly_observation(
            request,
            instance_alias,
            resolved.instance_id(),
            true,
        )?;
        let png = read_projected_verified(self.artifacts.root(), completed.observation.artifact())
            .map_err(observation_artifact_failure)?;
        let recognition_id = self.events.issuer().mint_recognition_id().map_err(|_| {
            observation_integrity_failure("observation_recognition_id_issue_failed")
        })?;
        let links = completed.links.with_recognition_id(recognition_id);
        self.append_event(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Recognition,
            EventActor::Runtime,
            links.clone(),
            RecognitionPayloadDraft::requested(EventAction::RecognitionObserve, AuditInput::new()),
        )?;
        let evaluated = prepared
            .evaluate(
                &png,
                FrameIdentity {
                    kind: FrameKind::Artifact,
                    sha256: completed
                        .observation
                        .artifact()
                        .sha256
                        .strip_prefix("sha256:")
                        .ok_or_else(|| {
                            observation_integrity_failure("observation_frame_hash_invalid")
                        })?
                        .to_string(),
                    width: completed.observation.width(),
                    height: completed.observation.height(),
                },
            )
            .map_err(|error| {
                self.observation_failure(
                    observation_kernel_error(error),
                    links.clone(),
                    RuntimeReceiptState::Failed,
                )
            })?;
        let evidence = ContainedObservationEvidence {
            schema_version: ONLINE_OBSERVATION_SCHEMA.to_string(),
            request_id: original.request_id(),
            correlation_id: request.correlation_id(),
            instance_id: resolved.instance_id(),
            expected_package_sha256: input.expected_sha256().to_string(),
            actual_package_sha256: prepared.package_sha256(),
            frame: completed.observation,
            rgb8_sha256: evaluated.rgb8_sha256,
            status: evaluated.status,
            projection: evaluated.projection,
            facts: evaluated.facts,
            private_facts: evaluated.private_facts,
        };
        let bytes = serde_json::to_vec(&evidence)
            .map_err(|_| observation_integrity_failure("observation_evidence_encode_failed"))?;
        if bytes.len() > MAX_OBSERVATION_ARTIFACT_BYTES {
            return Err(observation_integrity_failure(
                "observation_evidence_limit_exceeded",
            ));
        }
        let (artifact, verified) =
            self.persist_observation_artifact(&bytes, completed.artifact_links, links.clone())?;
        let recognized = evidence.status == PageObservationStatus::Recognized;
        let incomplete = matches!(
            evidence.status,
            PageObservationStatus::Partial | PageObservationStatus::Conflict
        );
        let payload = if incomplete {
            RecognitionPayloadDraft::failed(
                EventAction::RecognitionObserve,
                DiagnosticCode::RecognitionFailed,
                EffectDisposition::Performed,
                AuditInput::new(),
            )
        } else {
            RecognitionPayloadDraft::completed(
                EventAction::RecognitionObserve,
                EffectDisposition::Performed,
                evidence.frame.width(),
                evidence.frame.height(),
                if recognized {
                    RecognitionVerdict::PageMatched
                } else {
                    RecognitionVerdict::PageUnmatched
                },
                AuditInput::new(),
            )
        };
        let final_event = self.append_event(
            if incomplete {
                EventSeverity::Error
            } else {
                EventSeverity::Info
            },
            EventSource::Runtime,
            OriginModule::Recognition,
            EventActor::Runtime,
            links,
            payload,
        )?;
        let observation = ContainedPageObservation {
            instance_id: evidence.instance_id,
            expected_package_sha256: evidence.expected_package_sha256,
            actual_package_sha256: evidence.actual_package_sha256,
            frame: evidence.frame,
            status: evidence.status,
            projection: evidence.projection,
            facts: evidence.facts,
            artifact,
            projection_sequence: verified.sequence(),
            projection_event_id: *verified.event_id(),
        };
        observation
            .validate()
            .map_err(|_| observation_integrity_failure("observation_result_invalid"))?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Observed,
            terminal: Some(terminal(&final_event)),
            result: RuntimeResult::ContainedPageObserved {
                observation: Box::new(observation),
            },
        })
    }

    pub(super) fn persist_observation_artifact(
        &self,
        bytes: &[u8],
        artifact_links: ArtifactLinksDraft,
        links: EventLinksDraft,
    ) -> Result<(ProjectedArtifactReference, PersistedEvent), RequestFailure> {
        if bytes.len() > MAX_OBSERVATION_ARTIFACT_BYTES {
            return Err(observation_integrity_failure(
                "observation_evidence_limit_exceeded",
            ));
        }
        let mut sink = ObservationArtifactSink {
            ledger: &self.ledger,
            events: &self.events,
            verified: None,
        };
        let artifact = self
            .artifacts
            .put(
                ArtifactWriteRequest::new(
                    ArtifactKind::DiagnosticJson,
                    bytes,
                    ArtifactWriteContext::new(
                        artifact_links,
                        links,
                        unix_ms_now().map_err(RequestFailure::poison_without_terminal)?,
                    ),
                    ArtifactIssuePolicy::new(
                        ArtifactProducer::CapturePipeline,
                        RetentionClass::DebugFull,
                        ArtifactRedactionState::Pending,
                    ),
                ),
                &mut sink,
            )
            .map_err(observation_artifact_failure)?;
        let verified = sink
            .verified
            .ok_or_else(|| observation_integrity_failure("observation_verified_event_missing"))?;
        if verified.artifacts() != [artifact.reference().clone()] {
            return Err(observation_integrity_failure(
                "observation_verified_artifact_mismatch",
            ));
        }
        Ok((artifact.reference().project(true), verified))
    }

    pub(super) fn observation_failure(
        &self,
        error: RuntimeHostError,
        links: EventLinksDraft,
        state: RuntimeReceiptState,
    ) -> RequestFailure {
        let event = self.append_event(
            EventSeverity::Error,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links.clone(),
            RuntimePayloadDraft::failed(
                DiagnosticCode::RuntimeDiagnostic,
                EffectDisposition::NotPerformed,
                DiagnosticDetailDraft::new(
                    "contained_observation",
                    "observation.failed",
                    "contained_package",
                    error.operation(),
                    error.code(),
                    Sensitivity::Internal,
                ),
                AuditInput::new(),
            ),
        );
        match event {
            Ok(event) => match self.record_required_failure(&error, &event, links) {
                Ok(()) => RequestFailure::request(error, state, Some(terminal(&event))),
                Err(error) => RequestFailure::poison_without_terminal(error),
            },
            Err(failure) => failure,
        }
    }
}

pub(super) fn observation_admission_error(
    code: &'static str,
    stage: &'static str,
    cause: impl ToString,
) -> RuntimeHostError {
    RuntimeHostError::request(code, stage, RuntimeErrorCode::InvalidRequest)
        .with_native_detail(cause.to_string())
}
pub(super) fn observation_kernel_error(error: OnlineObservationError) -> RuntimeHostError {
    RuntimeHostError::request(
        error.code(),
        error.stage(),
        if matches!(
            error.stage(),
            "admit_contained_observation" | "load_observation_resources"
        ) {
            RuntimeErrorCode::InvalidRequest
        } else {
            RuntimeErrorCode::RecognitionFailed
        },
    )
    .with_native_detail(error.cause().to_string())
}
pub(super) fn observation_integrity_failure(code: &'static str) -> RequestFailure {
    RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
        code,
        "persist_contained_page_observation",
        RuntimeErrorCode::RuntimeFatal,
    ))
}
pub(super) fn observation_artifact_failure(error: ArtifactStoreError) -> RequestFailure {
    RequestFailure::poison_without_terminal(
        RuntimeHostError::fatal(
            error.code(),
            error.operation(),
            RuntimeErrorCode::RuntimeFatal,
        )
        .with_native_detail(error.to_string()),
    )
}

/// This request retains the sequence returned by its own native append.
pub(super) struct ObservationArtifactSink<'a> {
    pub(super) ledger: &'a GlobalLedger,
    pub(super) events: &'a RuntimeEvents,
    pub(super) verified: Option<PersistedEvent>,
}
impl ArtifactEventSink for ObservationArtifactSink<'_> {
    fn append(&mut self, draft: EventDraft) -> ArtifactStoreResult<()> {
        let sanitized = self.events.sanitize(draft).map_err(|error| {
            ArtifactStoreError::fatal(
                "artifact_event_sanitize_failed",
                "append_observation_artifact_event",
                error.to_string(),
            )
        })?;
        let event = self.ledger.append(sanitized).map_err(|error| {
            ArtifactStoreError::fatal(
                "artifact_event_append_failed",
                "append_observation_artifact_event",
                error.to_string(),
            )
        })?;
        if event.event_type() == EventType::ArtifactVerified
            && self.verified.replace(event).is_some()
        {
            return Err(ArtifactStoreError::fatal(
                "observation_duplicate_verified_event",
                "append_observation_artifact_event",
                "one diagnostic artifact per request",
            ));
        }
        Ok(())
    }
}
