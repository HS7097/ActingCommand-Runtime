// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::codes::{ClientCode, ClientLocation};
use actingcommand_artifact_store::read_projected_verified;
use actingcommand_contract::{
    ContainedObservationEvidence, ContainedObservationRequest, ContainedPageObservation,
    InstanceId, ONLINE_OBSERVATION_SCHEMA, ObservationSeriesPlan, PageObservationStatus,
    RecognitionPayload, RecognitionVerdict,
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

/// #411 Lab L5: the receipt wait of a contained observation: one backend-open allowance for the
/// current frame, and for every earlier frame of its transaction one more and the interval;
/// #411 Lab L5b: for every frame of its page's candidate layout series one more and the
/// layouts' declared waits. `None` on overflow.
pub(crate) fn observe_response_timeout(
    backend_open: Duration,
    frames: impl Into<ObservationSeriesPlan>,
) -> Option<Duration> {
    frames.into().allowance(backend_open)
}

impl RuntimeDebugSession {
    /// `frames`: what the package makes the observation take, its target consensus's
    /// transaction (#411 Lab L5) and its pages' candidate layout series (#411 Lab L5b), as the
    /// kernel's `PreparedPageObservation::series_plan` computes it from the package sent; the
    /// receipt wait allows for it. A package without such layouts may pass its
    /// `ObservationFramePlan`.
    pub fn observe_contained_page(
        &self,
        instance_alias: &str,
        request: ContainedObservationRequest,
        frames: impl Into<ObservationSeriesPlan>,
    ) -> RuntimeClientResult<VerifiedPageObservation> {
        request
            .validate()
            .map_err(|_| observation_error(ClientCode::RuntimeObservationRequestInvalid))?;
        let registry = self.client.status()?;
        let instance = registry
            .instances()
            .iter()
            .find(|instance| instance.instance_alias() == instance_alias)
            .ok_or_else(|| observation_error(ClientCode::RuntimeObservationInstanceUnknown))?
            .instance_id();
        let timeout = observe_response_timeout(
            self.client
                .connection(ClientLocation::ObserveContainedPage.location())?
                .backend_open_timeout,
            frames,
        )
        .ok_or_else(|| observation_error(ClientCode::RuntimeObservationRequestInvalid))?;
        let receipt = self.client.execute_receipt_with_correlation(
            ClientLocation::ObserveContainedPage.location(),
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
            &events,
        )
        .map_err(|error| {
            match self
                .client
                .connection(ClientLocation::VerifyContainedPageObservation.location())
            {
                Ok(mut connection) => connection.latch(error),
                Err(error) => error,
            }
        })
    }
}

fn observation_error(code: impl Into<Code>) -> RuntimeClientError {
    RuntimeClientError::fatal(
        code,
        ClientLocation::VerifyContainedPageObservation.location(),
    )
}

fn verify_observation(
    root: &Path,
    receipt: RuntimeReceipt,
    correlation: CorrelationId,
    instance: InstanceId,
    events: &[ProjectedEvent],
) -> RuntimeClientResult<VerifiedPageObservation> {
    receipt
        .validate()
        .map_err(|_| observation_error(ClientCode::RuntimeObservationReceiptInvalid))?;
    let Some(RuntimeResult::ContainedPageObserved { observation }) = receipt.result() else {
        return Err(observation_error(
            ClientCode::RuntimeObservationResultUnexpected,
        ));
    };
    // Workflow #336 PKG-S2 (option 丙): the observation names its package by `package_id`;
    // no package digest is compared.
    if receipt.correlation_id() != correlation || observation.instance_id != instance {
        return Err(observation_error(
            ClientCode::RuntimeObservationIdentityMismatch,
        ));
    }
    let terminal = receipt
        .terminal()
        .ok_or_else(|| observation_error(ClientCode::RuntimeObservationTerminalMissing))?;
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
            ClientCode::RuntimeObservationWriteAuthorityPresent,
        ));
    }
    let lifecycle = |reference: &ProjectedArtifactReference| -> RuntimeClientResult<(&ProjectedEvent, &ProjectedEvent)> {
        if reference.correlation_id != Some(correlation) || reference.frame_id != frame.frame_id { return Err(observation_error(ClientCode::RuntimeObservationArtifactIdentityMismatch)); }
        let mut created = None;
        let mut verified = None;
        for event in events.iter().filter(|event| event.artifacts.iter().any(|artifact| artifact.artifact_id == reference.artifact_id)) {
            if !identity(event) || event.artifacts != [reference.clone()] { return Err(observation_error(ClientCode::RuntimeObservationArtifactLifecycleConflict)); }
            let slot = match event.event_type { EventType::ArtifactCreated => &mut created, EventType::ArtifactVerified => &mut verified, _ => return Err(observation_error(ClientCode::RuntimeObservationArtifactLifecycleInvalid)) };
            if slot.replace(event).is_some() { return Err(observation_error(ClientCode::RuntimeObservationArtifactLifecycleDuplicate)); }
        }
        match (created, verified) {
            (Some(created), Some(verified)) if created.sequence < verified.sequence && verified.sequence < terminal.sequence => Ok((created, verified)),
            _ => Err(observation_error(ClientCode::RuntimeObservationArtifactLifecycleIncomplete)),
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
            ClientCode::RuntimeObservationProjectionSequenceMismatch,
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
            ClientCode::RuntimeObservationFrameLifecycleMismatch,
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
        return Err(observation_error(
            ClientCode::RuntimeObservationTerminalMismatch,
        ));
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
            ClientCode::RuntimeObservationRecognitionLifecycleMismatch,
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
            ClientCode::RuntimeObservationTerminalOutcomeMismatch,
        ));
    }
    let bytes = read_projected_verified(root, &observation.artifact)
        .map_err(|_| observation_error(ClientCode::RuntimeObservationArtifactVerificationFailed))?;
    let evidence: ContainedObservationEvidence = serde_json::from_slice(&bytes)
        .map_err(|_| observation_error(ClientCode::RuntimeObservationArtifactDecodeFailed))?;
    evidence
        .private_facts
        .validate()
        .map_err(|_| observation_error(ClientCode::RuntimeObservationPrivateFactsInvalid))?;
    if evidence.schema_version != ONLINE_OBSERVATION_SCHEMA
        || evidence.request_id != receipt.request_id()
        || evidence.correlation_id != correlation
        || evidence.instance_id != instance
        || evidence.package_label != observation.package_label
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
            ClientCode::RuntimeObservationArtifactReceiptMismatch,
        ));
    }
    let png = read_projected_verified(root, frame)
        .map_err(|_| observation_error(ClientCode::RuntimeObservationFrameVerificationFailed))?;
    let observation = observation.as_ref().clone();
    Ok(VerifiedPageObservation {
        receipt,
        observation,
        png,
    })
}
