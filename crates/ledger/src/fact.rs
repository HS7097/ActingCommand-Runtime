// SPDX-License-Identifier: AGPL-3.0-only

//! Opaque facts whose sequence and durable identity are owned by the ledger.

use actingcommand_contract::{
    ArtifactReference, EventId, EventLinks, EventOrigin, EventPayload, EventSeverity, EventType,
    GLOBAL_EVENT_SCHEMA_VERSION, SanitizedEventDraft, Sensitivity,
};
use serde::{Deserialize, Serialize};
use std::fmt;

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
    artifacts: Vec<ArtifactReference>,
}

impl PersistedEvent {
    pub(crate) fn from_sanitized(
        sequence: u64,
        draft: SanitizedEventDraft,
    ) -> Result<Self, FactValidationError> {
        let event = Self {
            schema_version: draft.schema_version().to_string(),
            event_id: *draft.event_id(),
            sequence,
            timestamp_unix_ms: draft.timestamp_unix_ms(),
            event_type: draft.event_type(),
            severity: draft.severity(),
            sensitivity: draft.sensitivity(),
            origin: draft.origin().clone(),
            links: draft.links().clone(),
            payload_schema: draft.payload_schema().to_string(),
            payload: draft.payload().clone(),
            artifacts: draft.artifacts().to_vec(),
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

    pub fn artifacts(&self) -> &[ArtifactReference] {
        &self.artifacts
    }

    fn validate(&self) -> Result<(), FactValidationError> {
        if self.schema_version != GLOBAL_EVENT_SCHEMA_VERSION {
            return Err(FactValidationError {
                code: "unsupported_event_schema",
            });
        }
        if self.sequence == 0 {
            return Err(FactValidationError {
                code: "invalid_sequence",
            });
        }
        if self.timestamp_unix_ms == 0 {
            return Err(FactValidationError {
                code: "invalid_timestamp",
            });
        }
        if self.event_type != self.payload.event_type()
            || self.event_type.family() != self.payload.family()
        {
            return Err(FactValidationError {
                code: "payload_type_mismatch",
            });
        }
        if self.payload_schema != self.payload.schema() {
            return Err(FactValidationError {
                code: "payload_schema_mismatch",
            });
        }
        if self.sensitivity != self.payload.sensitivity() || self.payload.validate().is_err() {
            return Err(FactValidationError {
                code: "invalid_typed_payload",
            });
        }
        if self
            .artifacts
            .iter()
            .any(|artifact| artifact.validate().is_err())
        {
            return Err(FactValidationError {
                code: "invalid_artifact_reference",
            });
        }
        Ok(())
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
    artifacts: Vec<ArtifactReference>,
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
            artifacts: event.artifacts.clone(),
        }
    }

    pub(crate) fn into_event(self) -> Result<PersistedEvent, FactValidationError> {
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
            artifacts: self.artifacts,
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
    pub(crate) const fn code(self) -> &'static str {
        self.code
    }
}
