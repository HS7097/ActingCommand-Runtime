// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::GLOBAL_EVENT_SCHEMA_VERSION;

/// Ledger metadata carries no artifact-store capability. Readers expose it only after
/// validating the containing ledger snapshot, and apply the requested output profile.
#[derive(Clone)]
pub(crate) struct LedgerEventMetadata {
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
    artifacts: Vec<ProjectedArtifactReference>,
    artifact_evictions: Vec<actingcommand_contract::ArtifactEvictionProof>,
}

/// Read-only fields shared by material-verified facts and ledger-verified metadata.
pub(crate) trait LedgerEventRead: Clone {
    fn schema_version(&self) -> &str;
    fn event_id(&self) -> &EventId;
    fn sequence(&self) -> u64;
    fn timestamp_unix_ms(&self) -> u64;
    fn event_type(&self) -> EventType;
    fn severity(&self) -> EventSeverity;
    fn sensitivity(&self) -> Sensitivity;
    fn origin(&self) -> &EventOrigin;
    fn links(&self) -> &EventLinks;
    fn payload_schema(&self) -> &str;
    fn payload(&self) -> &EventPayload;
    fn projected_artifacts(&self, include_object_key: bool) -> Vec<ProjectedArtifactReference>;
    fn artifact_evictions(&self) -> &[actingcommand_contract::ArtifactEvictionProof];
}

impl LedgerEventRead for PersistedEvent {
    fn artifact_evictions(&self) -> &[actingcommand_contract::ArtifactEvictionProof] {
        self.artifact_evictions()
    }
    fn schema_version(&self) -> &str {
        self.schema_version()
    }
    fn event_id(&self) -> &EventId {
        self.event_id()
    }
    fn sequence(&self) -> u64 {
        self.sequence()
    }
    fn timestamp_unix_ms(&self) -> u64 {
        self.timestamp_unix_ms()
    }
    fn event_type(&self) -> EventType {
        self.event_type()
    }
    fn severity(&self) -> EventSeverity {
        self.severity()
    }
    fn sensitivity(&self) -> Sensitivity {
        self.sensitivity()
    }
    fn origin(&self) -> &EventOrigin {
        self.origin()
    }
    fn links(&self) -> &EventLinks {
        self.links()
    }
    fn payload_schema(&self) -> &str {
        self.payload_schema()
    }
    fn payload(&self) -> &EventPayload {
        self.payload()
    }
    fn projected_artifacts(&self, include_object_key: bool) -> Vec<ProjectedArtifactReference> {
        self.artifacts()
            .iter()
            .map(|artifact| artifact.project(include_object_key))
            .collect()
    }
}

impl LedgerEventRead for LedgerEventMetadata {
    fn artifact_evictions(&self) -> &[actingcommand_contract::ArtifactEvictionProof] {
        &self.artifact_evictions
    }
    fn schema_version(&self) -> &str {
        &self.schema_version
    }
    fn event_id(&self) -> &EventId {
        &self.event_id
    }
    fn sequence(&self) -> u64 {
        self.sequence
    }
    fn timestamp_unix_ms(&self) -> u64 {
        self.timestamp_unix_ms
    }
    fn event_type(&self) -> EventType {
        self.event_type
    }
    fn severity(&self) -> EventSeverity {
        self.severity
    }
    fn sensitivity(&self) -> Sensitivity {
        self.sensitivity
    }
    fn origin(&self) -> &EventOrigin {
        &self.origin
    }
    fn links(&self) -> &EventLinks {
        &self.links
    }
    fn payload_schema(&self) -> &str {
        &self.payload_schema
    }
    fn payload(&self) -> &EventPayload {
        &self.payload
    }
    fn projected_artifacts(&self, include_object_key: bool) -> Vec<ProjectedArtifactReference> {
        self.artifacts
            .iter()
            .cloned()
            .map(|mut artifact| {
                if !include_object_key {
                    artifact.object_key = None;
                }
                artifact
            })
            .collect()
    }
}

impl StoredEventRecord {
    /// Validates record structure only; the caller must authenticate its ledger snapshot.
    pub(crate) fn into_metadata(self) -> Result<LedgerEventMetadata, FactValidationError> {
        let event = LedgerEventMetadata {
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
            artifacts: self
                .artifacts
                .iter()
                .map(StoredArtifactRecord::projected)
                .collect(),
            artifact_evictions: Vec::new(),
        };
        validate(&event)?;
        Ok(event)
    }

    pub(crate) fn sequence(&self) -> u64 {
        self.sequence
    }
    pub(crate) fn timestamp_unix_ms(&self) -> u64 {
        self.timestamp_unix_ms
    }
    pub(crate) fn payload(&self) -> &EventPayload {
        &self.payload
    }
}

pub(super) fn validate(event: &impl LedgerEventRead) -> Result<(), FactValidationError> {
    let invalid = |code| FactValidationError { code };
    if event.schema_version() != GLOBAL_EVENT_SCHEMA_VERSION {
        return Err(invalid("unsupported_event_schema"));
    }
    if event.sequence() == 0 {
        return Err(invalid("invalid_sequence"));
    }
    if event.timestamp_unix_ms() == 0 {
        return Err(invalid("invalid_timestamp"));
    }
    if event.event_type() != event.payload().event_type()
        || event.event_type().family() != event.payload().family()
    {
        return Err(invalid("payload_type_mismatch"));
    }
    if event.payload_schema() != event.payload().schema() {
        return Err(invalid("payload_schema_mismatch"));
    }
    let artifacts = event.projected_artifacts(true);
    let expected_sensitivity =
        artifacts
            .iter()
            .fold(event.payload().sensitivity(), |current, artifact| {
                current.max(match artifact.redaction_state {
                    ArtifactRedactionState::Pending => Sensitivity::Secret,
                    ArtifactRedactionState::Applied => Sensitivity::Sensitive,
                    ArtifactRedactionState::NotRequired => Sensitivity::Internal,
                })
            });
    if event.sensitivity() != expected_sensitivity || event.payload().validate().is_err() {
        return Err(invalid("invalid_typed_payload"));
    }
    if artifacts
        .iter()
        .any(|artifact| artifact.object_key.is_none() || artifact.validate().is_err())
    {
        return Err(invalid("invalid_artifact_reference"));
    }
    Ok(())
}

impl LedgerEventMetadata {
    pub(crate) fn apply_artifact_evictions(
        &mut self,
        proofs: Vec<actingcommand_contract::ArtifactEvictionProof>,
    ) {
        self.artifact_evictions = proofs;
    }
    pub(crate) fn into_record(self) -> StoredEventRecord {
        StoredEventRecord {
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
            artifacts: self
                .artifacts
                .into_iter()
                .map(|reference| StoredArtifactRecord {
                    artifact_id: reference.artifact_id,
                    kind: reference.kind,
                    run_id: reference.run_id,
                    frame_id: reference.frame_id,
                    correlation_id: reference.correlation_id,
                    object_key: reference.object_key.expect("validated metadata object key"),
                    media_type: reference.media_type,
                    byte_count: reference.byte_count,
                    sha256: reference.sha256,
                    created_at_unix_ms: reference.created_at_unix_ms,
                    producer: reference.producer,
                    retention_class: reference.retention_class,
                    redaction_state: reference.redaction_state,
                })
                .collect(),
        }
    }
}
