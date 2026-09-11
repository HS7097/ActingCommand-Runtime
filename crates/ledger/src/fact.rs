// SPDX-License-Identifier: AGPL-3.0-only

//! Opaque facts whose sequence and durable identity are owned by the ledger.

use actingcommand_contract::{
    ArtifactId, ArtifactKind, ArtifactMediaType, ArtifactProducer, ArtifactRedactionState,
    ArtifactReference, CorrelationId, EventId, EventLinks, EventOrigin, EventPayload,
    EventSeverity, EventType, FrameId, ProjectedArtifactReference, RetentionClass, RunId,
    SanitizedEventDraft, Sensitivity, VerifiedArtifactReference,
};
use serde::{Deserialize, Serialize};
use std::fmt;

mod artifact;
pub use artifact::{ArtifactAvailability, LedgerArtifactReference};
mod metadata;
pub(crate) use metadata::{LedgerEventMetadata, LedgerEventRead};

/// A ledger-assigned fact. Consumers can inspect and serialize it, but cannot construct or
/// deserialize one.
///
/// ```compile_fail
/// use actingcommand_ledger::PersistedEvent;
///
/// let _: PersistedEvent = serde_json::from_str("{}").unwrap();
/// ```
///
/// ```compile_fail
/// use actingcommand_ledger::PersistedEvent;
///
/// let _ = PersistedEvent { sequence: 1 };
/// ```
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct PersistedEvent {
    schema_version: String,
    event_id: EventId,
    sequence: u64,
    timestamp_unix_ms: u64,
    event_type: EventType,
    severity: EventSeverity,
    sensitivity: Sensitivity,
    origin: EventOrigin,
    links: EventLinks,
    payload_schema: String,
    payload: EventPayload,
    artifacts: Vec<LedgerArtifactReference>,
    #[serde(skip)]
    artifact_evictions: artifact::ArtifactRetentionReadState,
}

impl PersistedEvent {
    pub(crate) fn from_sanitized(
        sequence: u64,
        draft: SanitizedEventDraft,
    ) -> Result<Self, FactValidationError> {
        let event_id = *draft.event_id();
        Self::from_sanitized_with_event_id(sequence, draft, event_id)
    }

    pub(crate) fn from_sanitized_with_event_id(
        sequence: u64,
        draft: SanitizedEventDraft,
        event_id: EventId,
    ) -> Result<Self, FactValidationError> {
        let event = Self {
            schema_version: draft.schema_version().to_string(),
            event_id,
            sequence,
            timestamp_unix_ms: draft.timestamp_unix_ms(),
            event_type: draft.event_type(),
            severity: draft.severity(),
            sensitivity: draft.sensitivity(),
            origin: draft.origin().clone(),
            links: draft.links().clone(),
            payload_schema: draft.payload_schema().to_string(),
            payload: draft.payload().clone(),
            artifacts: draft
                .artifacts()
                .iter()
                .map(LedgerArtifactReference::from_issued)
                .collect(),
            artifact_evictions: artifact::ArtifactRetentionReadState::default(),
        };
        event.validate()?;
        Ok(event)
    }

    pub fn schema_version(&self) -> &str {
        &self.schema_version
    }

    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    pub const fn event_id(&self) -> &EventId {
        &self.event_id
    }

    pub const fn timestamp_unix_ms(&self) -> u64 {
        self.timestamp_unix_ms
    }

    pub const fn event_type(&self) -> EventType {
        self.event_type
    }

    pub const fn severity(&self) -> EventSeverity {
        self.severity
    }

    pub const fn sensitivity(&self) -> Sensitivity {
        self.sensitivity
    }

    pub const fn origin(&self) -> &EventOrigin {
        &self.origin
    }

    pub const fn links(&self) -> &EventLinks {
        &self.links
    }

    pub fn payload_schema(&self) -> &str {
        &self.payload_schema
    }

    pub const fn payload(&self) -> &EventPayload {
        &self.payload
    }

    pub fn artifacts(&self) -> &[LedgerArtifactReference] {
        &self.artifacts
    }

    pub fn artifact_evictions(&self) -> &[actingcommand_contract::ArtifactEvictionProof] {
        &self.artifact_evictions.0
    }

    pub(crate) fn apply_artifact_evictions(
        &mut self,
        proofs: Vec<actingcommand_contract::ArtifactEvictionProof>,
    ) {
        for artifact in &mut self.artifacts {
            if let Some(proof) = proofs
                .iter()
                .find(|proof| &proof.identity.artifact.artifact_id == artifact.artifact_id())
            {
                artifact.apply_retention_proof(proof);
            }
        }
        self.artifact_evictions.0 = proofs;
    }

    fn validate(&self) -> Result<(), FactValidationError> {
        metadata::validate(self)
    }
}

impl fmt::Debug for PersistedEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PersistedEvent")
            .field("sequence", &self.sequence)
            .field("event_id", &self.event_id)
            .field("event_type", &self.event_type)
            .field("severity", &self.severity)
            .field("sensitivity", &self.sensitivity)
            .field("payload", &"<sanitized-payload>")
            .field("artifact_count", &self.artifacts.len())
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredEventRecord {
    schema_version: String,
    event_id: EventId,
    sequence: u64,
    timestamp_unix_ms: u64,
    event_type: EventType,
    severity: EventSeverity,
    sensitivity: Sensitivity,
    origin: EventOrigin,
    links: EventLinks,
    payload_schema: String,
    payload: EventPayload,
    artifacts: Vec<StoredArtifactRecord>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredArtifactRecord {
    artifact_id: ArtifactId,
    kind: ArtifactKind,
    run_id: Option<RunId>,
    frame_id: Option<FrameId>,
    correlation_id: Option<CorrelationId>,
    object_key: String,
    media_type: ArtifactMediaType,
    byte_count: u64,
    sha256: String,
    created_at_unix_ms: u64,
    producer: ArtifactProducer,
    retention_class: RetentionClass,
    redaction_state: ArtifactRedactionState,
}

impl StoredArtifactRecord {
    fn from_reference(reference: &LedgerArtifactReference) -> Self {
        Self {
            artifact_id: *reference.artifact_id(),
            kind: reference.kind(),
            run_id: reference.run_id().copied(),
            frame_id: reference.frame_id().copied(),
            correlation_id: reference.correlation_id().copied(),
            object_key: reference.object_key().to_string(),
            media_type: reference.media_type(),
            byte_count: reference.byte_count(),
            sha256: reference.sha256().to_string(),
            created_at_unix_ms: reference.created_at_unix_ms(),
            producer: reference.producer(),
            retention_class: reference.retention_class(),
            redaction_state: reference.redaction_state(),
        }
    }

    fn projected(&self) -> ProjectedArtifactReference {
        ProjectedArtifactReference {
            artifact_id: self.artifact_id,
            kind: self.kind,
            run_id: self.run_id,
            frame_id: self.frame_id,
            correlation_id: self.correlation_id,
            object_key: Some(self.object_key.clone()),
            media_type: self.media_type,
            byte_count: self.byte_count,
            sha256: self.sha256.clone(),
            created_at_unix_ms: self.created_at_unix_ms,
            producer: self.producer,
            retention_class: self.retention_class,
            redaction_state: self.redaction_state,
        }
    }
}

impl StoredEventRecord {
    pub(crate) fn from_event(event: &PersistedEvent) -> Self {
        Self {
            schema_version: event.schema_version.clone(),
            event_id: event.event_id,
            sequence: event.sequence,
            timestamp_unix_ms: event.timestamp_unix_ms,
            event_type: event.event_type,
            severity: event.severity,
            sensitivity: event.sensitivity,
            origin: event.origin.clone(),
            links: event.links.clone(),
            payload_schema: event.payload_schema.clone(),
            payload: event.payload.clone(),
            artifacts: event
                .artifacts
                .iter()
                .map(StoredArtifactRecord::from_reference)
                .collect(),
        }
    }

    pub(crate) fn into_event(self) -> Result<PersistedEvent, FactValidationError> {
        // C1 cannot authenticate public artifact metadata without the C2 store owner. Recovery
        // therefore fails closed instead of promoting a syntactically coherent record.
        if !self.artifacts.is_empty() {
            return Err(FactValidationError {
                code: "artifact_store_verification_unavailable",
            });
        }
        self.into_event_with_artifacts(Vec::new())
    }

    pub(crate) fn into_event_with_artifact_verifier<F>(
        self,
        verifier: &mut F,
    ) -> Result<PersistedEvent, FactValidationError>
    where
        F: FnMut(&ProjectedArtifactReference) -> Option<VerifiedArtifactReference> + ?Sized,
    {
        self.into_event_with_artifact_availability(&mut |projected| {
            verifier(projected)
                .map(ArtifactAvailability::Available)
                .ok_or(FactValidationError {
                    code: "artifact_store_verification_failed",
                })
        })
    }

    pub(crate) fn into_event_with_artifact_availability<F>(
        self,
        availability: &mut F,
    ) -> Result<PersistedEvent, FactValidationError>
    where
        F: FnMut(&ProjectedArtifactReference) -> Result<ArtifactAvailability, FactValidationError>
            + ?Sized,
    {
        let mut artifacts = Vec::with_capacity(self.artifacts.len());
        for stored in &self.artifacts {
            let reference = stored.projected();
            artifacts.push(LedgerArtifactReference::restored(
                reference.clone(),
                availability(&reference)?,
            )?);
        }
        self.into_event_with_artifacts(artifacts)
    }

    fn into_event_with_artifacts(
        self,
        artifacts: Vec<LedgerArtifactReference>,
    ) -> Result<PersistedEvent, FactValidationError> {
        let event = PersistedEvent {
            schema_version: self.schema_version,
            event_id: self.event_id,
            sequence: self.sequence,
            timestamp_unix_ms: self.timestamp_unix_ms,
            event_type: self.event_type,
            severity: self.severity,
            sensitivity: self.sensitivity,
            origin: self.origin,
            links: self.links,
            payload_schema: self.payload_schema,
            payload: self.payload,
            artifacts,
            artifact_evictions: artifact::ArtifactRetentionReadState::default(),
        };
        event.validate()?;
        Ok(event)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FactValidationError {
    code: &'static str,
}

impl FactValidationError {
    pub(crate) const fn new(code: &'static str) -> Self {
        Self { code }
    }
    pub(crate) const fn code(self) -> &'static str {
        self.code
    }
}
