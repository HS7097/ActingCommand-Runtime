// SPDX-License-Identifier: AGPL-3.0-only

use super::online_observation::{
    observation_admission_error, observation_artifact_failure, observation_integrity_failure,
    observation_kernel_error,
};
use super::*;
use actingcommand_contract::page_projection::{FrameIdentity, FrameKind};
use actingcommand_contract::{
    ContainedLabOperationRequest, ContainedLabOperationResult, ContainedObservationEvidence,
    ContainedPageObservation, LAB_OPERATION_PREPARED_SCHEMA, LAB_OPERATION_TERMINAL_SCHEMA,
    LabEvidenceReference, LabOperationFailure, LabOperationFrame, LabOperationPrepared,
    LabOperationRecord, LabOperationStage, ONLINE_OBSERVATION_SCHEMA, PageObservationStatus,
};
use actingcommand_execution_kernel::PreparedPageObservation;

impl HostShared {
    pub(super) fn run_contained_lab_operation(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        instance_alias: &str,
        holder_id: actingcommand_contract::HolderId,
        input: &ContainedLabOperationRequest,
        connection_id: ConnectionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        let resolved = self.resolve_instance(instance_alias)?;
        let run_links = lock(&self.debug_runs, "read_lab_operation_run")?
            .get(&request.correlation_id())
            .map(|run| RuntimeRunLinks::new(run.task_id, run.run_id));
        self.append_request_lifecycle(
            original,
            request,
            resolved.instance_id(),
            RuntimeDebugOperation::Do.event_action(),
            run_links,
        )?;
        let observer = (|| {
            let path = Path::new(&input.package_path);
            if !path.is_absolute() {
                return Err(observation_admission_error(
                    "observation_path_not_absolute",
                    "open_lab_operation_package",
                    "absolute package path required",
                ));
            }
            let file = fs::File::open(path).map_err(|error| {
                observation_admission_error(
                    "observation_package_open_failed",
                    "open_lab_operation_package",
                    error,
                )
            })?;
            let mut bytes = Vec::new();
            file.take(DEFAULT_MAX_COMPRESSED_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|error| {
                    observation_admission_error(
                        "observation_package_read_failed",
                        "read_lab_operation_package",
                        error,
                    )
                })?;
            if bytes.len() as u64 > DEFAULT_MAX_COMPRESSED_BYTES {
                return Err(observation_admission_error(
                    "observation_package_limit_exceeded",
                    "read_lab_operation_package",
                    "compressed package limit exceeded",
                ));
            }
            let expected =
                ExternalExpectedSha256::parse_hex(&input.expected_sha256).map_err(|error| {
                    observation_admission_error(
                        "observation_hash_invalid",
                        "admit_lab_operation",
                        error,
                    )
                })?;
            PreparedPageObservation::load(
                instance_alias,
                &bytes,
                expected,
                &[],
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

        let mut prepared = LabOperationPrepared {
            schema_version: LAB_OPERATION_PREPARED_SCHEMA.to_string(),
            request_id: original.request_id(),
            correlation_id: request.correlation_id(),
            instance_id: resolved.instance_id(),
            expected_package_sha256: input.expected_sha256.clone(),
            actual_package_sha256: observer.package_sha256(),
            lease_id: None,
            selection: input.selection.clone(),
            projection_hint: input.projection_hint.clone(),
            before_frame: None,
            before_projection: None,
            selected_element: None,
            geometry: None,
            action: None,
        };
        let mut token = None;
        let mut stage = LabOperationStage::Lease;
        let preparation = (|| -> Result<(), RequestFailure> {
            let lease = self.acquire_lease(RuntimeLeaseAcquisition {
                request,
                request_id: original.request_id(),
                instance_alias,
                holder_id,
                connection_id,
                run_links,
                lease_ttl_ms: None,
            })?;
            let RuntimeResult::LeaseGranted { token: acquired } = lease.result else {
                return Err(observation_integrity_failure(
                    "lab_operation_lease_result_invalid",
                ));
            };
            prepared.lease_id = Some(acquired.lease_id());
            token = Some(acquired);
            let held = token.as_ref().expect("lease acquired");
            stage = LabOperationStage::BeforeFrame;
            let (frame, fence) =
                self.capture_lab_operation_frame(request, held, connection_id, run_links)?;
            prepared.before_frame = Some(LabOperationFrame {
                observation: frame.observation.clone(),
                verified: frame.verified,
                lease_valid_after_capture: fence.is_ok(),
            });
            fence?;
            stage = LabOperationStage::BeforeProjection;
            let projection = self.evaluate_lab_operation_frame(
                original,
                request,
                input,
                &observer,
                &frame,
                resolved.instance_id(),
            )?;
            prepared.before_projection = Some(projection);
            stage = LabOperationStage::Selection;
            let selected = input
                .selection
                .resolve(
                    &prepared
                        .before_projection
                        .as_ref()
                        .expect("projection evaluated")
                        .projection,
                )
                .map_err(|error| {
                    RequestFailure::request(
                        RuntimeHostError::request(
                            error.code(),
                            "resolve_current_lab_operation",
                            RuntimeErrorCode::RecognitionFailed,
                        ),
                        RuntimeReceiptState::Denied,
                        None,
                    )
                })?;
            prepared.selected_element = selected.0;
            prepared.geometry = Some(selected.1);
            prepared.action = Some(selected.2);
            Ok(())
        })();
        let failure = match preparation {
            Ok(()) => None,
            Err(failure) => Some(lab_operation_failure(stage, failure)?),
        };
        let mut links = request.event_links(Some(resolved.instance_id()), prepared.lease_id, None);
        let mut artifact_links = request.artifact_links();
        if let Some(run_links) = run_links {
            links = run_links.apply(links);
            artifact_links = artifact_links.with_run_id(run_links.run_id);
        }
        let bytes = serde_json::to_vec(&prepared)
            .map_err(|_| observation_integrity_failure("lab_prepared_encode_failed"))?;
        let (artifact, verified) =
            self.persist_observation_artifact(&bytes, artifact_links.clone(), links.clone())?;
        let mut record = LabOperationRecord {
            schema_version: LAB_OPERATION_TERMINAL_SCHEMA.to_string(),
            prepared,
            prepared_artifact: LabEvidenceReference {
                artifact,
                verified: terminal(&verified),
            },
            input_returned: false,
            input_action_id: None,
            input_intent: None,
            input_event: None,
            effect: EffectDisposition::NotPerformed,
            after_frame: None,
            after_projection: None,
            failure,
            cleanup_failure: None,
        };
        if record.failure.is_none() {
            let held = token.as_ref().expect("prepared action holds lease");
            // No instance guard or destructive step is held across this owner call.
            let (input_terminal, input_failure) = match self.input(
                request,
                held,
                record.prepared.action.as_ref().expect("prepared action"),
                connection_id,
                resolved.provenance(),
                RuntimeInputContext {
                    run_links,
                    source_step_action_id: None,
                    before_frame_id: record
                        .prepared
                        .before_frame
                        .as_ref()
                        .and_then(|frame| frame.observation.artifact().frame_id().copied()),
                },
            ) {
                Ok((success, _)) => (success.terminal, None),
                Err(failure) if failure.poison_runtime || failure.error.is_fatal() => {
                    return Err(failure);
                }
                Err(failure) => (failure.terminal, Some(failure)),
            };
            record.input_returned = true;
            record.effect = EffectDisposition::Indeterminate;
            if let Err(mut evidence_failure) = self.resolve_lab_input_evidence(
                &mut record,
                input_terminal,
                input_failure.is_some(),
            ) {
                if let Some(original_failure) = input_failure {
                    evidence_failure.error =
                        Box::new(evidence_failure.error.as_ref().clone().with_native_detail(
                            format!(
                                "input_failure={}; evidence_failure={}",
                                original_failure.error.code(),
                                evidence_failure.error.code(),
                            ),
                        ));
                    evidence_failure.terminal = original_failure.terminal;
                }
                return Err(evidence_failure);
            }
            record.failure = match input_failure {
                Some(failure) => Some(lab_operation_failure(LabOperationStage::Input, failure)?),
                None if record.input_event.is_none() => Some(lab_operation_failure(
                    LabOperationStage::Input,
                    RequestFailure::request(
                        RuntimeHostError::request(
                            "lab_input_outcome_missing",
                            "resolve_lab_input_evidence",
                            RuntimeErrorCode::BackendOperationFailed,
                        ),
                        RuntimeReceiptState::Failed,
                        None,
                    ),
                )?),
                None => None,
            };
            if record.failure.is_none() {
                let post = (|| -> Result<(), RequestFailure> {
                    stage = LabOperationStage::AfterFrame;
                    // This re-fences the original token, including any safe-boundary transfer.
                    let (frame, fence) =
                        self.capture_lab_operation_frame(request, held, connection_id, run_links)?;
                    record.after_frame = Some(LabOperationFrame {
                        observation: frame.observation.clone(),
                        verified: frame.verified,
                        lease_valid_after_capture: fence.is_ok(),
                    });
                    fence?;
                    stage = LabOperationStage::AfterProjection;
                    record.after_projection = Some(self.evaluate_lab_operation_frame(
                        original,
                        request,
                        input,
                        &observer,
                        &frame,
                        resolved.instance_id(),
                    )?);
                    let instance_guard = self.instance_guard(held.instance_id())?;
                    let _admission = lock(&instance_guard, "validate_lab_after_projection")?;
                    self.validated_instance(request, held, connection_id)?;
                    Ok(())
                })();
                if let Err(failure) = post {
                    record.failure = Some(lab_operation_failure(stage, failure)?);
                }
            }
        }
        if let Some(held) = token.as_ref() {
            let still_owned = lock(&self.scheduler, "read_lab_operation_cleanup_lease")?
                .active_tokens()
                .iter()
                .any(|active| active == held);
            if still_owned
                && let Err(failure) = self.release_lease(
                    request,
                    original.request_id(),
                    held,
                    connection_id,
                    run_links,
                )
            {
                let cleanup = lab_operation_failure(LabOperationStage::Release, failure)?;
                if record.failure.is_none() {
                    record.failure = Some(cleanup);
                } else {
                    record.cleanup_failure = Some(cleanup);
                }
            }
        }
        let bytes = serde_json::to_vec(&record)
            .map_err(|_| observation_integrity_failure("lab_terminal_encode_failed"))?;
        let (artifact, verified) =
            self.persist_observation_artifact(&bytes, artifact_links, links.clone())?;
        let payload = if record.failure.is_some() {
            CommandPayloadDraft::rejected(
                RuntimeDebugOperation::Do.event_action(),
                DiagnosticCode::CommandRejected,
                record.effect,
                AuditInput::new(),
            )
        } else {
            CommandPayloadDraft::validated(
                RuntimeDebugOperation::Do.event_action(),
                record.effect,
                AuditInput::new(),
            )
        };
        let final_event = self.append_event(
            if record.failure.is_some() {
                EventSeverity::Error
            } else {
                EventSeverity::Info
            },
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links,
            payload,
        )?;
        let operation = ContainedLabOperationResult {
            record,
            terminal_artifact: LabEvidenceReference {
                artifact,
                verified: terminal(&verified),
            },
        };
        operation
            .validate()
            .map_err(|_| observation_integrity_failure("lab_operation_result_invalid"))?;
        Ok(OperationSuccess {
            state: if operation.record.failure.is_some() {
                RuntimeReceiptState::Failed
            } else {
                RuntimeReceiptState::Completed
            },
            terminal: Some(terminal(&final_event)),
            result: RuntimeResult::ContainedLabOperation {
                operation: Box::new(operation),
            },
        })
    }

    fn resolve_lab_input_evidence(
        &self,
        record: &mut LabOperationRecord,
        returned_terminal: Option<TerminalEvent>,
        input_failed: bool,
    ) -> Result<(), RequestFailure> {
        let prepared = &record.prepared;
        let identity_matches = |event: &PersistedEvent| {
            event.links().request_id() == Some(&prepared.request_id)
                && event.links().correlation_id() == Some(&prepared.correlation_id)
                && event.links().instance_id() == Some(&prepared.instance_id)
                && event.links().lease_id() == prepared.lease_id.as_ref()
        };
        // Input has returned. These bounded native queries never select by proximity or latest.
        let intents = self
            .ledger
            .query_page(
                EventQuery {
                    event_type: Some(EventType::InputIntent),
                    request_id: Some(prepared.request_id),
                    correlation_id: Some(prepared.correlation_id),
                    instance_id: Some(prepared.instance_id),
                    lease_id: prepared.lease_id,
                    ..EventQuery::default()
                },
                0,
                u64::MAX,
                2,
            )
            .map_err(|error| {
                RequestFailure::poison_without_terminal(
                    ledger_error("query_lab_input_intent").with_native_detail(error.to_string()),
                )
            })?;
        if intents.len() > 1 {
            return Err(observation_integrity_failure("lab_input_intent_not_unique"));
        }
        let mut outcome = None;
        if let Some(intent) = intents.first() {
            let action_id = *intent
                .links()
                .action_id()
                .ok_or_else(|| observation_integrity_failure("lab_input_action_missing"))?;
            if !identity_matches(intent)
                || intent.sequence() <= record.prepared_artifact.verified.sequence
                || Some(intent.payload().action())
                    != prepared.action.as_ref().map(InputAction::event_action)
            {
                return Err(observation_integrity_failure("lab_input_intent_mismatch"));
            }
            record.input_action_id = Some(action_id);
            record.input_intent = Some(terminal(intent));
            for event_type in [EventType::InputCommitted, EventType::InputFailed] {
                let events = self
                    .ledger
                    .query_page(
                        EventQuery {
                            event_type: Some(event_type),
                            action_id: Some(action_id),
                            ..EventQuery::default()
                        },
                        0,
                        u64::MAX,
                        2,
                    )
                    .map_err(|error| {
                        RequestFailure::poison_without_terminal(
                            ledger_error("query_lab_input_outcome")
                                .with_native_detail(error.to_string()),
                        )
                    })?;
                if events.len() > 1 || (!events.is_empty() && outcome.is_some()) {
                    return Err(observation_integrity_failure("lab_input_outcome_conflict"));
                }
                if let Some(event) = events.into_iter().next() {
                    if !identity_matches(&event)
                        || event.sequence() <= intent.sequence()
                        || event.payload().action() != intent.payload().action()
                    {
                        return Err(observation_integrity_failure("lab_input_outcome_mismatch"));
                    }
                    outcome = Some(event);
                }
            }
        }
        if let Some(reference) = returned_terminal {
            let events = self
                .ledger
                .query_page(
                    EventQuery {
                        from_sequence: Some(reference.sequence),
                        to_sequence: Some(reference.sequence),
                        ..EventQuery::default()
                    },
                    0,
                    reference.sequence,
                    2,
                )
                .map_err(|error| {
                    RequestFailure::poison_without_terminal(
                        ledger_error("query_lab_returned_input_terminal")
                            .with_native_detail(error.to_string()),
                    )
                })?;
            let [event] = events.as_slice() else {
                return Err(observation_integrity_failure(
                    "lab_returned_input_terminal_missing",
                ));
            };
            if terminal(event) != reference
                || !identity_matches(event)
                || (matches!(
                    event.event_type(),
                    EventType::InputCommitted | EventType::InputFailed
                ) && outcome
                    .as_ref()
                    .is_none_or(|outcome| terminal(outcome) != reference))
                || (!input_failed && event.event_type() != EventType::InputCommitted)
            {
                return Err(observation_integrity_failure(
                    "lab_returned_input_terminal_mismatch",
                ));
            }
        }
        if let Some(outcome) = outcome {
            let effect = outcome
                .payload()
                .effect_disposition()
                .ok_or_else(|| observation_integrity_failure("lab_input_effect_missing"))?;
            if (outcome.event_type() == EventType::InputCommitted
                && effect != EffectDisposition::Performed)
                || (outcome.event_type() == EventType::InputFailed
                    && (!input_failed || effect == EffectDisposition::Performed))
            {
                return Err(observation_integrity_failure(
                    "lab_input_outcome_effect_mismatch",
                ));
            }
            record.input_event = Some(terminal(&outcome));
            record.effect = effect;
        }
        Ok(())
    }

    fn capture_lab_operation_frame(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        token: &LeaseToken,
        connection_id: ConnectionId,
        run_links: Option<RuntimeRunLinks>,
    ) -> Result<(CompletedReadonlyObservation, Result<(), RequestFailure>), RequestFailure> {
        let instance_guard = self.instance_guard(token.instance_id())?;
        let _admission = lock(&instance_guard, "lock_lab_operation_capture")?;
        let instance = self.validated_instance(request, token, connection_id)?;
        let frame_id = self
            .events
            .issuer()
            .mint_frame_id()
            .map_err(|_| observation_integrity_failure("lab_frame_id_issue_failed"))?;
        let recognition_id = self
            .events
            .issuer()
            .mint_recognition_id()
            .map_err(|_| observation_integrity_failure("lab_recognition_id_issue_failed"))?;
        let mut links = request
            .event_links(Some(token.instance_id()), Some(token.lease_id()), None)
            .with_frame_id(frame_id)
            .with_recognition_id(recognition_id);
        let mut artifact_links = request.artifact_links().with_frame_id(frame_id);
        if let Some(run_links) = run_links {
            links = run_links.apply(links);
            artifact_links = artifact_links.with_run_id(run_links.run_id);
        }
        let completed = self.capture_observation_with_links(
            request,
            &instance.instance_alias,
            links,
            artifact_links,
            true,
        )?;
        let fence = self
            .validated_instance(request, token, connection_id)
            .map(|_| ());
        Ok((completed, fence))
    }

    fn evaluate_lab_operation_frame(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        input: &ContainedLabOperationRequest,
        observer: &PreparedPageObservation,
        captured: &CompletedReadonlyObservation,
        instance_id: InstanceId,
    ) -> Result<ContainedPageObservation, RequestFailure> {
        let png = read_projected_verified(self.artifacts.root(), captured.observation.artifact())
            .map_err(observation_artifact_failure)?;
        let recognition_id = self
            .events
            .issuer()
            .mint_recognition_id()
            .map_err(|_| observation_integrity_failure("lab_projection_id_issue_failed"))?;
        let links = captured.links.clone().with_recognition_id(recognition_id);
        self.append_event(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Recognition,
            EventActor::Runtime,
            links.clone(),
            RecognitionPayloadDraft::requested(EventAction::RecognitionObserve, AuditInput::new()),
        )?;
        let evaluated = observer
            .evaluate(
                &png,
                FrameIdentity {
                    kind: FrameKind::Artifact,
                    sha256: captured
                        .observation
                        .artifact()
                        .sha256
                        .strip_prefix("sha256:")
                        .ok_or_else(|| observation_integrity_failure("lab_frame_hash_invalid"))?
                        .to_string(),
                    width: captured.observation.width(),
                    height: captured.observation.height(),
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
            instance_id,
            expected_package_sha256: input.expected_sha256.clone(),
            actual_package_sha256: observer.package_sha256(),
            frame: captured.observation.clone(),
            rgb8_sha256: evaluated.rgb8_sha256,
            status: evaluated.status,
            projection: evaluated.projection,
            facts: evaluated.facts,
            private_facts: evaluated.private_facts,
        };
        let bytes = serde_json::to_vec(&evidence)
            .map_err(|_| observation_integrity_failure("lab_projection_encode_failed"))?;
        let (artifact, verified) = self.persist_observation_artifact(
            &bytes,
            captured.artifact_links.clone(),
            links.clone(),
        )?;
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
                if evidence.status == PageObservationStatus::Recognized {
                    RecognitionVerdict::PageMatched
                } else {
                    RecognitionVerdict::PageUnmatched
                },
                AuditInput::new(),
            )
        };
        self.append_event(
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
        Ok(ContainedPageObservation {
            instance_id,
            expected_package_sha256: evidence.expected_package_sha256,
            actual_package_sha256: evidence.actual_package_sha256,
            frame: evidence.frame,
            status: evidence.status,
            projection: evidence.projection,
            facts: evidence.facts,
            artifact,
            projection_sequence: verified.sequence(),
            projection_event_id: *verified.event_id(),
        })
    }
}

fn lab_operation_failure(
    stage: LabOperationStage,
    failure: RequestFailure,
) -> Result<LabOperationFailure, RequestFailure> {
    if failure.poison_runtime || failure.error.is_fatal() {
        return Err(failure);
    }
    Ok(LabOperationFailure {
        stage,
        code: failure.error.code().to_string(),
        error: failure.error.projection().clone(),
        event: failure.terminal,
    })
}
