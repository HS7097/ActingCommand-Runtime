// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_artifact_store::read_projected_verified;
use actingcommand_contract::{
    ContainedObservationEvidence, ContainedObservationRequest, ContainedPageObservation,
    InstanceId, ONLINE_OBSERVATION_SCHEMA, PageObservationStatus, RecognitionPayload,
    RecognitionVerdict,
};

/// Constructed only after the receipt, native lifecycle and both artifact hashes agree.
pub struct VerifiedPageObservation {
    receipt: RuntimeReceipt,
    observation: ContainedPageObservation,
    png: Vec<u8>,
}
impl VerifiedPageObservation {
    pub fn receipt(&self) -> &RuntimeReceipt {
        &self.receipt
    }
    pub fn observation(&self) -> &ContainedPageObservation {
        &self.observation
    }
    pub fn png(&self) -> &[u8] {
        &self.png
    }
}
impl fmt::Debug for VerifiedPageObservation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("VerifiedPageObservation(<verified-artifacts>)")
    }
}

impl RuntimeDebugSession {
    pub fn observe_contained_page(
        &self,
        instance_alias: &str,
        request: ContainedObservationRequest,
    ) -> RuntimeClientResult<VerifiedPageObservation> {
        request
            .validate()
            .map_err(|_| observation_error("runtime_observation_request_invalid"))?;
        let registry = self.client.status()?;
        let instance = registry
            .instances()
            .iter()
            .find(|instance| instance.instance_alias() == instance_alias)
            .ok_or_else(|| observation_error("runtime_observation_instance_unknown"))?
            .instance_id();
        let expected = request.expected_sha256().to_string();
        let timeout = self
            .client
            .connection("observe_contained_page")?
            .backend_open_timeout;
        let receipt = self.client.execute_receipt_with_correlation(
            "observe_contained_page",
            RuntimeOperation::ObserveContainedPage {
                instance_alias: instance_alias.to_string(),
                request,
            },
            self.correlation,
            Some(timeout),
        )?;
        let events = self.client.query_events(
            EventQuery {
                request_id: Some(receipt.request_id()),
                correlation_id: Some(self.correlation_id()),
                ..EventQuery::default()
            },
            ProjectionProfile::Forensic,
        )?;
        verify_observation(
            &self.client.shared.state_root,
            receipt,
            self.correlation_id(),
            instance,
            &expected,
            &events,
        )
        .map_err(|error| {
            match self.client.connection("verify_contained_page_observation") {
                Ok(mut connection) => connection.latch(error),
                Err(error) => error,
            }
        })
    }
}

fn observation_error(code: &'static str) -> RuntimeClientError {
    RuntimeClientError::fatal(code, "verify_contained_page_observation")
}

fn verify_observation(
    root: &Path,
    receipt: RuntimeReceipt,
    correlation: CorrelationId,
    instance: InstanceId,
    expected: &str,
    events: &[ProjectedEvent],
) -> RuntimeClientResult<VerifiedPageObservation> {
    receipt
        .validate()
        .map_err(|_| observation_error("runtime_observation_receipt_invalid"))?;
    let Some(RuntimeResult::ContainedPageObserved { observation }) = receipt.result() else {
        return Err(observation_error("runtime_observation_result_unexpected"));
    };
    if receipt.correlation_id() != correlation
        || observation.instance_id != instance
        || observation.expected_package_sha256 != expected
    {
        return Err(observation_error("runtime_observation_identity_mismatch"));
    }
    let terminal = receipt
        .terminal()
        .ok_or_else(|| observation_error("runtime_observation_terminal_missing"))?;
    let frame = observation.frame.artifact();
    let identity = |event: &ProjectedEvent| {
        event.links.request_id() == Some(&receipt.request_id())
            && event.links.correlation_id() == Some(&correlation)
            && event.links.instance_id() == Some(&instance)
            && event.links.frame_id() == frame.frame_id.as_ref()
            && event.links.lease_id().is_none()
    };
    if events.iter().any(|event| {
        event.links.lease_id().is_some()
            || matches!(
                event.event_type,
                EventType::InputIntent
                    | EventType::InputCommitted
                    | EventType::InputFailed
                    | EventType::LeaseRequested
                    | EventType::LeaseGranted
            )
    }) {
        return Err(observation_error(
            "runtime_observation_write_authority_present",
        ));
    }
    let lifecycle = |reference: &ProjectedArtifactReference| -> RuntimeClientResult<(&ProjectedEvent, &ProjectedEvent)> {
        if reference.correlation_id != Some(correlation) || reference.frame_id != frame.frame_id { return Err(observation_error("runtime_observation_artifact_identity_mismatch")); }
        let mut created = None;
        let mut verified = None;
        for event in events.iter().filter(|event| event.artifacts.iter().any(|artifact| artifact.artifact_id == reference.artifact_id)) {
            if !identity(event) || event.artifacts != [reference.clone()] { return Err(observation_error("runtime_observation_artifact_lifecycle_conflict")); }
            let slot = match event.event_type { EventType::ArtifactCreated => &mut created, EventType::ArtifactVerified => &mut verified, _ => return Err(observation_error("runtime_observation_artifact_lifecycle_invalid")) };
            if slot.replace(event).is_some() { return Err(observation_error("runtime_observation_artifact_lifecycle_duplicate")); }
        }
        match (created, verified) {
            (Some(created), Some(verified)) if created.sequence < verified.sequence && verified.sequence < terminal.sequence => Ok((created, verified)),
            _ => Err(observation_error("runtime_observation_artifact_lifecycle_incomplete")),
        }
    };
    let (_, frame_verified) = lifecycle(frame)?;
    let (projection_created, projection_verified) = lifecycle(&observation.artifact)?;
    if projection_verified.sequence != observation.projection_sequence
        || projection_verified.event_id != observation.projection_event_id
        || frame_verified.sequence >= projection_created.sequence
        || observation.artifact.producer != ArtifactProducer::CapturePipeline
        || observation.artifact.retention_class != RetentionClass::DebugFull
        || observation.artifact.redaction_state != ArtifactRedactionState::Pending
    {
        return Err(observation_error(
            "runtime_observation_projection_sequence_mismatch",
        ));
    }
    let decoded = events.iter().filter(|event| identity(event) && matches!(&event.payload,
        ProjectionPayload::Full(payload) if matches!(payload.as_ref(), EventPayload::Recognition(RecognitionPayload::Completed(value))
            if value.recognition_verdict() == Some(RecognitionVerdict::FrameDecoded) && value.frame_width() == observation.frame.width() && value.frame_height() == observation.frame.height()))).collect::<Vec<_>>();
    if decoded.len() != 1
        || decoded[0].sequence <= frame_verified.sequence
        || decoded[0].sequence >= projection_created.sequence
    {
        return Err(observation_error(
            "runtime_observation_frame_lifecycle_mismatch",
        ));
    }
    let final_events = events
        .iter()
        .filter(|event| {
            event.sequence == terminal.sequence
                && event.event_id == terminal.event_id
                && identity(event)
        })
        .collect::<Vec<_>>();
    let [final_event] = final_events.as_slice() else {
        return Err(observation_error("runtime_observation_terminal_mismatch"));
    };
    let requested = events
        .iter()
        .filter(|event| {
            identity(event)
                && event.event_type == EventType::RecognitionRequested
                && event.links.recognition_id() == final_event.links.recognition_id()
                && event.sequence > decoded[0].sequence
                && event.sequence < projection_created.sequence
        })
        .count();
    if requested != 1
        || final_event.links.recognition_id().is_none()
        || final_event.links.recognition_id() == decoded[0].links.recognition_id()
        || final_event.links.recognition_id() != projection_verified.links.recognition_id()
    {
        return Err(observation_error(
            "runtime_observation_recognition_lifecycle_mismatch",
        ));
    }
    let valid_terminal = match (&final_event.payload, observation.status) {
        (
            ProjectionPayload::Full(payload),
            PageObservationStatus::Recognized | PageObservationStatus::NoMatch,
        ) => {
            matches!(payload.as_ref(), EventPayload::Recognition(RecognitionPayload::Completed(value))
            if value.recognition_verdict() == Some(if observation.status == PageObservationStatus::Recognized { RecognitionVerdict::PageMatched } else { RecognitionVerdict::PageUnmatched }))
        }
        (
            ProjectionPayload::Full(payload),
            PageObservationStatus::Partial | PageObservationStatus::Conflict,
        ) => matches!(
            payload.as_ref(),
            EventPayload::Recognition(RecognitionPayload::Failed(_))
        ),
        _ => false,
    };
    if !valid_terminal {
        return Err(observation_error(
            "runtime_observation_terminal_outcome_mismatch",
        ));
    }
    let bytes = read_projected_verified(root, &observation.artifact)
        .map_err(|_| observation_error("runtime_observation_artifact_verification_failed"))?;
    let evidence: ContainedObservationEvidence = serde_json::from_slice(&bytes)
        .map_err(|_| observation_error("runtime_observation_artifact_decode_failed"))?;
    evidence
        .private_facts
        .validate()
        .map_err(|_| observation_error("runtime_observation_private_facts_invalid"))?;
    if evidence.schema_version != ONLINE_OBSERVATION_SCHEMA
        || evidence.request_id != receipt.request_id()
        || evidence.correlation_id != correlation
        || evidence.instance_id != instance
        || evidence.expected_package_sha256 != expected
        || evidence.actual_package_sha256 != expected
        || evidence.frame != observation.frame
        || evidence.status != observation.status
        || evidence.projection != observation.projection
        || evidence.facts != observation.facts
        || evidence.rgb8_sha256.len() != 64
        || !evidence
            .rgb8_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(observation_error(
            "runtime_observation_artifact_receipt_mismatch",
        ));
    }
    let png = read_projected_verified(root, frame)
        .map_err(|_| observation_error("runtime_observation_frame_verification_failed"))?;
    let observation = observation.as_ref().clone();
    Ok(VerifiedPageObservation {
        receipt,
        observation,
        png,
    })
}
