// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_artifact_store::read_projected_verified;
use actingcommand_contract::{
    ContainedLabOperationRequest, ContainedLabOperationResult, ContainedObservationEvidence,
    ContainedPageObservation, LabOperationFrame, LabOperationPrepared, LabOperationRecord,
    LabOperationStage, MAX_INPUT_DURATION_MS, ONLINE_OBSERVATION_SCHEMA, RecognitionVerdict,
};

/// Ordinary operation failure is retained in the verified receipt and record.
pub struct VerifiedLabOperation {
    receipt: RuntimeReceipt,
    operation: ContainedLabOperationResult,
}

impl VerifiedLabOperation {
    pub fn receipt(&self) -> &RuntimeReceipt {
        &self.receipt
    }
    pub fn operation(&self) -> &ContainedLabOperationResult {
        &self.operation
    }
}

impl fmt::Debug for VerifiedLabOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("VerifiedLabOperation(<verified-artifacts>)")
    }
}

impl RuntimeDebugSession {
    pub fn run_contained_lab_operation(
        &self,
        instance_alias: &str,
        request: ContainedLabOperationRequest,
    ) -> RuntimeClientResult<VerifiedLabOperation> {
        request
            .validate()
            .map_err(|_| lab_error("runtime_lab_operation_request_invalid"))?;
        let registry = self.client.status()?;
        let instance = registry
            .instances()
            .iter()
            .find(|instance| instance.instance_alias() == instance_alias)
            .ok_or_else(|| lab_error("runtime_lab_operation_instance_unknown"))?
            .instance_id();
        let connection = self.client.connection("run_contained_lab_operation")?;
        let holder = connection
            .ids
            .mint_holder_id()
            .map_err(|_| lab_error("runtime_lab_holder_issue_failed"))?;
        let timeout = connection
            .backend_open_timeout
            .checked_mul(4)
            .and_then(|timeout| timeout.checked_add(Duration::from_millis(MAX_INPUT_DURATION_MS)))
            .and_then(|timeout| timeout.checked_add(connection.io_timeout))
            .ok_or_else(|| lab_error("runtime_lab_operation_timeout_overflow"))?;
        drop(connection);
        let receipt = self.client.execute_receipt_with_correlation(
            "run_contained_lab_operation",
            RuntimeOperation::RunContainedLabOperation {
                instance_alias: instance_alias.to_string(),
                holder_id: *holder.transport(),
                request: request.clone(),
            },
            self.correlation,
            Some(timeout),
        )?;
        let verification = (|| {
            let Some(RuntimeResult::ContainedLabOperation { operation }) = receipt.result() else {
                return Err(lab_error("runtime_lab_operation_result_unexpected"));
            };
            let prepared = &operation.record.prepared;
            if prepared.instance_id != instance
                || prepared.expected_package_sha256 != request.expected_sha256
                || prepared.selection != request.selection
                || prepared.projection_hint != request.projection_hint
            {
                return Err(lab_error("runtime_lab_operation_request_mismatch"));
            }
            let events = self.client.query_events(
                EventQuery {
                    request_id: Some(receipt.request_id()),
                    correlation_id: Some(self.correlation_id()),
                    ..EventQuery::default()
                },
                ProjectionProfile::Forensic,
            )?;
            let lease_events = if let Some(lease) = prepared.lease_id {
                self.client.query_events(
                    EventQuery {
                        lease_id: Some(lease),
                        ..EventQuery::default()
                    },
                    ProjectionProfile::Forensic,
                )?
            } else {
                Vec::new()
            };
            let action_events = if let Some(action_id) = operation.record.input_action_id {
                self.client.query_events(
                    EventQuery {
                        action_id: Some(action_id),
                        ..EventQuery::default()
                    },
                    ProjectionProfile::Forensic,
                )?
            } else {
                Vec::new()
            };
            verify_lab_operation(
                &self.client.shared.state_root,
                &receipt,
                &events,
                &lease_events,
                &action_events,
            )?;
            Ok(VerifiedLabOperation {
                operation: operation.as_ref().clone(),
                receipt: receipt.clone(),
            })
        })();
        verification.map_err(
            |error| match self.client.connection("verify_lab_operation") {
                Ok(mut connection) => connection.latch(error),
                Err(error) => error,
            },
        )
    }
}

fn lab_error(code: &'static str) -> RuntimeClientError {
    RuntimeClientError::fatal(code, "verify_contained_lab_operation")
}

fn lab_event(
    events: &[ProjectedEvent],
    terminal: TerminalEvent,
) -> RuntimeClientResult<&ProjectedEvent> {
    let mut matches = events
        .iter()
        .filter(|event| event.sequence == terminal.sequence && event.event_id == terminal.event_id);
    let event = matches
        .next()
        .ok_or_else(|| lab_error("runtime_lab_event_missing"))?;
    if matches.next().is_some() {
        return Err(lab_error("runtime_lab_event_duplicate"));
    }
    Ok(event)
}

fn verify_lab_artifact(
    root: &Path,
    receipt: &RuntimeReceipt,
    events: &[ProjectedEvent],
    reference: &ProjectedArtifactReference,
    verified: TerminalEvent,
    prepared: &LabOperationPrepared,
) -> RuntimeClientResult<Vec<u8>> {
    if reference.correlation_id != Some(receipt.correlation_id()) {
        return Err(lab_error("runtime_lab_artifact_correlation_mismatch"));
    }
    let mut created = None;
    let mut completed = None;
    for event in events.iter().filter(|event| {
        event
            .artifacts
            .iter()
            .any(|artifact| artifact.artifact_id == reference.artifact_id)
    }) {
        if event.links.request_id() != Some(&receipt.request_id())
            || event.links.correlation_id() != Some(&receipt.correlation_id())
            || event.links.instance_id() != Some(&prepared.instance_id)
            || event.links.lease_id() != prepared.lease_id.as_ref()
            || event.links.frame_id() != reference.frame_id.as_ref()
            || event.artifacts != [reference.clone()]
        {
            return Err(lab_error("runtime_lab_artifact_identity_mismatch"));
        }
        let slot = match event.event_type {
            EventType::ArtifactCreated => &mut created,
            EventType::ArtifactVerified => &mut completed,
            _ => return Err(lab_error("runtime_lab_artifact_event_invalid")),
        };
        if slot.replace(event).is_some() {
            return Err(lab_error("runtime_lab_artifact_event_duplicate"));
        }
    }
    let (Some(created), Some(completed)) = (created, completed) else {
        return Err(lab_error("runtime_lab_artifact_lifecycle_missing"));
    };
    if created.sequence >= completed.sequence
        || completed.sequence != verified.sequence
        || completed.event_id != verified.event_id
        || completed.sequence
            >= receipt
                .terminal()
                .ok_or_else(|| lab_error("runtime_lab_terminal_missing"))?
                .sequence
        || reference.retention_class != RetentionClass::DebugFull
        || (reference.kind == ArtifactKind::DiagnosticJson
            && (reference.producer != ArtifactProducer::CapturePipeline
                || reference.redaction_state != ArtifactRedactionState::Pending))
        || (reference.kind == ArtifactKind::CaptureFrame
            && (reference.producer != ArtifactProducer::CaptureStore
                || reference.redaction_state != ArtifactRedactionState::NotRequired))
    {
        return Err(lab_error("runtime_lab_artifact_lifecycle_mismatch"));
    }
    read_projected_verified(root, reference)
        .map_err(|_| lab_error("runtime_lab_artifact_hash_mismatch"))
}

fn verify_lab_projection(
    root: &Path,
    receipt: &RuntimeReceipt,
    events: &[ProjectedEvent],
    prepared: &LabOperationPrepared,
    frame: &LabOperationFrame,
    observation: &ContainedPageObservation,
) -> RuntimeClientResult<()> {
    let verified = TerminalEvent {
        sequence: observation.projection_sequence,
        event_id: observation.projection_event_id,
    };
    let bytes = verify_lab_artifact(
        root,
        receipt,
        events,
        &observation.artifact,
        verified,
        prepared,
    )?;
    let evidence: ContainedObservationEvidence = serde_json::from_slice(&bytes)
        .map_err(|_| lab_error("runtime_lab_projection_decode_failed"))?;
    evidence
        .private_facts
        .validate()
        .map_err(|_| lab_error("runtime_lab_private_facts_invalid"))?;
    if evidence.schema_version != ONLINE_OBSERVATION_SCHEMA
        || evidence.request_id != receipt.request_id()
        || evidence.correlation_id != receipt.correlation_id()
        || evidence.instance_id != prepared.instance_id
        || evidence.expected_package_sha256 != prepared.expected_package_sha256
        || evidence.actual_package_sha256 != prepared.actual_package_sha256
        || evidence.frame != frame.observation
        || evidence.frame != observation.frame
        || evidence.projection != observation.projection
        || evidence.facts != observation.facts
        || evidence.status != observation.status
        || frame.verified.sequence >= verified.sequence
        || observation.artifact.frame_id != frame.observation.artifact().frame_id
    {
        return Err(lab_error("runtime_lab_projection_content_mismatch"));
    }
    let projection_event = lab_event(events, verified)?;
    let decoded = events.iter().filter(|event| event.links.frame_id() == frame.observation.artifact().frame_id.as_ref()
        && event.sequence > frame.verified.sequence && event.sequence < verified.sequence
        && matches!(&event.payload, ProjectionPayload::Full(payload)
            if matches!(payload.as_ref(), EventPayload::Recognition(actingcommand_contract::RecognitionPayload::Completed(value))
                if value.recognition_verdict() == Some(RecognitionVerdict::FrameDecoded)
                    && value.frame_width() == frame.observation.width() && value.frame_height() == frame.observation.height()))).collect::<Vec<_>>();
    let recognition = events
        .iter()
        .filter(|event| {
            event.links.recognition_id() == projection_event.links.recognition_id()
                && event.links.frame_id() == frame.observation.artifact().frame_id.as_ref()
        })
        .collect::<Vec<_>>();
    if decoded.len() != 1
        || projection_event.links.recognition_id().is_none()
        || recognition.iter().chain(decoded.iter()).any(|event| {
            event.links.instance_id() != Some(&prepared.instance_id)
                || event.links.lease_id() != prepared.lease_id.as_ref()
        })
        || events
            .iter()
            .filter(|event| {
                event.event_type == EventType::CaptureCompleted
                    && event.links.frame_id() == frame.observation.artifact().frame_id.as_ref()
                    && event.links.lease_id() == prepared.lease_id.as_ref()
                    && event.sequence > frame.verified.sequence
                    && event.sequence < verified.sequence
            })
            .count()
            != 1
        || recognition
            .iter()
            .filter(|event| {
                event.event_type == EventType::RecognitionRequested
                    && event.sequence > decoded[0].sequence
                    && event.sequence < verified.sequence
            })
            .count()
            != 1
        || recognition
            .iter()
            .filter(|event| {
                matches!(
                    event.event_type,
                    EventType::RecognitionCompleted | EventType::RecognitionFailed
                ) && event.sequence > verified.sequence
            })
            .count()
            != 1
    {
        return Err(lab_error("runtime_lab_recognition_lifecycle_mismatch"));
    }
    Ok(())
}

fn verify_lab_operation(
    root: &Path,
    receipt: &RuntimeReceipt,
    events: &[ProjectedEvent],
    lease_events: &[ProjectedEvent],
    action_events: &[ProjectedEvent],
) -> RuntimeClientResult<()> {
    receipt
        .validate()
        .map_err(|_| lab_error("runtime_lab_receipt_invalid"))?;
    let Some(RuntimeResult::ContainedLabOperation { operation }) = receipt.result() else {
        return Err(lab_error("runtime_lab_result_missing"));
    };
    let record = &operation.record;
    let prepared = &record.prepared;
    let prepared_bytes = verify_lab_artifact(
        root,
        receipt,
        events,
        &record.prepared_artifact.artifact,
        record.prepared_artifact.verified,
        prepared,
    )?;
    let stored_prepared: LabOperationPrepared = serde_json::from_slice(&prepared_bytes)
        .map_err(|_| lab_error("runtime_lab_prepared_decode_failed"))?;
    let terminal_bytes = verify_lab_artifact(
        root,
        receipt,
        events,
        &operation.terminal_artifact.artifact,
        operation.terminal_artifact.verified,
        prepared,
    )?;
    let stored_record: LabOperationRecord = serde_json::from_slice(&terminal_bytes)
        .map_err(|_| lab_error("runtime_lab_terminal_decode_failed"))?;
    if stored_prepared != *prepared
        || stored_record != *record
        || record.prepared_artifact.artifact.frame_id.is_some()
        || operation.terminal_artifact.artifact.frame_id.is_some()
    {
        return Err(lab_error("runtime_lab_record_content_mismatch"));
    }
    let mut diagnostic_ids = BTreeSet::from([
        record.prepared_artifact.artifact.artifact_id,
        operation.terminal_artifact.artifact.artifact_id,
    ]);
    for projection in [&prepared.before_projection, &record.after_projection]
        .into_iter()
        .flatten()
    {
        diagnostic_ids.insert(projection.artifact.artifact_id);
    }
    let diagnostics = events
        .iter()
        .filter(|event| event.event_type == EventType::ArtifactVerified)
        .flat_map(|event| &event.artifacts)
        .filter(|artifact| artifact.kind == ArtifactKind::DiagnosticJson)
        .collect::<Vec<_>>();
    if diagnostics.len() != diagnostic_ids.len()
        || diagnostics.len() > 4
        || diagnostics
            .iter()
            .any(|artifact| !diagnostic_ids.contains(&artifact.artifact_id))
        || diagnostics
            .iter()
            .map(|artifact| artifact.byte_count)
            .sum::<u64>()
            > 1024 * 1024
    {
        return Err(lab_error("runtime_lab_diagnostic_budget_mismatch"));
    }
    for failure in [&record.failure, &record.cleanup_failure]
        .into_iter()
        .flatten()
    {
        if let Some(reference) = failure.event {
            let event = lab_event(events, reference)?;
            if event.links.instance_id() != Some(&prepared.instance_id)
                || event.links.lease_id() != prepared.lease_id.as_ref()
                || reference.sequence >= operation.terminal_artifact.verified.sequence
            {
                return Err(lab_error("runtime_lab_failure_event_mismatch"));
            }
        }
    }
    let grants = events
        .iter()
        .filter(|event| event.event_type == EventType::LeaseGranted)
        .collect::<Vec<_>>();
    if let Some(lease) = prepared.lease_id {
        if grants.len() != 1
            || grants[0].links.lease_id() != Some(&lease)
            || grants[0].links.instance_id() != Some(&prepared.instance_id)
        {
            return Err(lab_error("runtime_lab_lease_grant_mismatch"));
        }
    } else if !grants.is_empty() || prepared.before_frame.is_some() {
        return Err(lab_error("runtime_lab_unleased_frame"));
    }
    for (frame, projection) in [
        (&prepared.before_frame, &prepared.before_projection),
        (&record.after_frame, &record.after_projection),
    ] {
        if let Some(frame) = frame {
            verify_lab_artifact(
                root,
                receipt,
                events,
                frame.observation.artifact(),
                frame.verified,
                prepared,
            )?;
            if grants
                .first()
                .is_none_or(|grant| grant.sequence >= frame.verified.sequence)
            {
                return Err(lab_error("runtime_lab_frame_before_lease"));
            }
            if let Some(projection) = projection {
                verify_lab_projection(root, receipt, events, prepared, frame, projection)?;
            }
        }
    }
    if let Some(action) = &prepared.action {
        let projection = prepared
            .before_projection
            .as_ref()
            .ok_or_else(|| lab_error("runtime_lab_action_projection_missing"))?;
        let (element, geometry, resolved) = prepared
            .selection
            .resolve(&projection.projection)
            .map_err(|_| lab_error("runtime_lab_action_unresolvable"))?;
        if element != prepared.selected_element
            || Some(&geometry) != prepared.geometry.as_ref()
            || &resolved != action
            || prepared
                .before_frame
                .as_ref()
                .is_none_or(|frame| !frame.lease_valid_after_capture)
            || projection.projection_sequence >= record.prepared_artifact.verified.sequence
        {
            return Err(lab_error("runtime_lab_action_source_mismatch"));
        }
    }
    let intents = events
        .iter()
        .filter(|event| event.event_type == EventType::InputIntent)
        .collect::<Vec<_>>();
    if let Some(reference) = record.input_intent {
        let intent = lab_event(events, reference)?;
        let ProjectionPayload::Full(payload) = &intent.payload else {
            return Err(lab_error("runtime_lab_input_intent_payload_missing"));
        };
        if intents.len() != 1
            || intent.event_type != EventType::InputIntent
            || intent.links.action_id() != record.input_action_id.as_ref()
            || intent.links.instance_id() != Some(&prepared.instance_id)
            || intent.links.lease_id() != prepared.lease_id.as_ref()
            || intent.sequence <= record.prepared_artifact.verified.sequence
            || Some(payload.action()) != prepared.action.as_ref().map(InputAction::event_action)
            || action_events
                .iter()
                .filter(|event| event.event_type == EventType::InputIntent)
                .count()
                != 1
        {
            return Err(lab_error("runtime_lab_input_intent_mismatch"));
        }
        lab_event(action_events, reference)?;
    } else if !intents.is_empty() || !action_events.is_empty() {
        return Err(lab_error("runtime_lab_unreported_input_intent"));
    }
    let input_events = action_events
        .iter()
        .filter(|event| {
            matches!(
                event.event_type,
                EventType::InputCommitted | EventType::InputFailed
            )
        })
        .collect::<Vec<_>>();
    if let Some(terminal) = record.input_event {
        let input = lab_event(events, terminal)?;
        lab_event(action_events, terminal)?;
        if input_events.len() != 1
            || !matches!(
                input.event_type,
                EventType::InputCommitted | EventType::InputFailed
            )
            || input.links.lease_id() != prepared.lease_id.as_ref()
            || input.links.instance_id() != Some(&prepared.instance_id)
            || input.links.action_id() != record.input_action_id.as_ref()
            || prepared.action.is_none()
        {
            return Err(lab_error("runtime_lab_input_identity_mismatch"));
        }
        let ProjectionPayload::Full(payload) = &input.payload else {
            return Err(lab_error("runtime_lab_input_payload_missing"));
        };
        if payload.effect_disposition() != Some(record.effect)
            || Some(payload.action()) != prepared.action.as_ref().map(InputAction::event_action)
        {
            return Err(lab_error("runtime_lab_input_effect_mismatch"));
        }
        if record
            .input_intent
            .is_none_or(|intent| intent.sequence >= input.sequence)
        {
            return Err(lab_error("runtime_lab_input_intent_mismatch"));
        }
        if input.event_type == EventType::InputFailed
            && (record.after_frame.is_some()
                || record
                    .failure
                    .as_ref()
                    .is_none_or(|failure| failure.stage != LabOperationStage::Input))
        {
            return Err(lab_error("runtime_lab_failed_input_reobserved"));
        }
        if record
            .after_frame
            .as_ref()
            .is_some_and(|frame| frame.verified.sequence <= input.sequence)
        {
            return Err(lab_error("runtime_lab_post_frame_order_invalid"));
        }
    } else if !input_events.is_empty()
        || events.iter().any(|event| {
            matches!(
                event.event_type,
                EventType::InputCommitted | EventType::InputFailed
            )
        })
        || record.after_frame.is_some()
        || record.after_projection.is_some()
        || (record.input_returned
            && record
                .failure
                .as_ref()
                .is_none_or(|failure| failure.stage != LabOperationStage::Input))
    {
        return Err(lab_error("runtime_lab_unreported_input"));
    }
    if record.failure.is_none() {
        let before = prepared
            .before_frame
            .as_ref()
            .ok_or_else(|| lab_error("runtime_lab_before_missing"))?;
        let after = record
            .after_frame
            .as_ref()
            .ok_or_else(|| lab_error("runtime_lab_after_missing"))?;
        if before.observation.artifact().frame_id == after.observation.artifact().frame_id
            || lease_events.iter().any(|event| {
                event.sequence > grants[0].sequence
                    && event.sequence
                        < record
                            .after_projection
                            .as_ref()
                            .expect("validated post projection")
                            .projection_sequence
                    && matches!(
                        event.event_type,
                        EventType::LeaseReleased
                            | EventType::LeaseExpired
                            | EventType::LeaseTransferred
                    )
            })
        {
            return Err(lab_error("runtime_lab_success_lease_discontinuous"));
        }
    }
    let final_event = lab_event(
        events,
        receipt
            .terminal()
            .ok_or_else(|| lab_error("runtime_lab_terminal_missing"))?,
    )?;
    let ProjectionPayload::Full(payload) = &final_event.payload else {
        return Err(lab_error("runtime_lab_terminal_payload_missing"));
    };
    if final_event.event_type
        != if record.failure.is_some() {
            EventType::CommandRejected
        } else {
            EventType::CommandValidated
        }
        || payload.effect_disposition() != Some(record.effect)
        || final_event.links.request_id() != Some(&receipt.request_id())
        || final_event.links.correlation_id() != Some(&receipt.correlation_id())
        || final_event.links.instance_id() != Some(&prepared.instance_id)
        || final_event.links.lease_id() != prepared.lease_id.as_ref()
    {
        return Err(lab_error("runtime_lab_terminal_outcome_mismatch"));
    }
    Ok(())
}
