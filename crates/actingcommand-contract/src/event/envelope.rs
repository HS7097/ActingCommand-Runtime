// SPDX-License-Identifier: AGPL-3.0-only

use super::{
    ActionId, ArtifactReference, CausationId, CorrelationId, EventActor, EventId, EventPayload,
    EventPayloadDraft, EventSeverity, EventSource, EventType, FrameId, GLOBAL_EVENT_SCHEMA_VERSION,
    InstanceId, LeaseId, RecognitionId, RequestId, RunId, SanitizationError, SecretFingerprinter,
    Sensitivity, StaticCode, TaskId,
};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventOrigin {
    source: EventSource,
    module: StaticCode,
    actor: EventActor,
}

impl EventOrigin {
    pub const fn new(source: EventSource, module: StaticCode, actor: EventActor) -> Self {
        Self {
            source,
            module,
            actor,
        }
    }

    pub const fn source(&self) -> EventSource {
        self.source
    }

    pub const fn actor(&self) -> EventActor {
        self.actor
    }

    pub fn module(&self) -> &StaticCode {
        &self.module
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventLinks {
    #[serde(skip_serializing_if = "Option::is_none")]
    instance_id: Option<InstanceId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_id: Option<RequestId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    correlation_id: Option<CorrelationId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    causation_id: Option<CausationId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    task_id: Option<TaskId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_id: Option<RunId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lease_id: Option<LeaseId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    frame_id: Option<FrameId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    action_id: Option<ActionId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recognition_id: Option<RecognitionId>,
}

impl EventLinks {
    pub fn with_instance_id(mut self, value: InstanceId) -> Self {
        self.instance_id = Some(value);
        self
    }

    pub fn with_request_id(mut self, value: RequestId) -> Self {
        self.request_id = Some(value);
        self
    }

    pub fn with_correlation_id(mut self, value: CorrelationId) -> Self {
        self.correlation_id = Some(value);
        self
    }

    pub fn with_causation_id(mut self, value: CausationId) -> Self {
        self.causation_id = Some(value);
        self
    }

    pub fn with_task_id(mut self, value: TaskId) -> Self {
        self.task_id = Some(value);
        self
    }

    pub fn with_run_id(mut self, value: RunId) -> Self {
        self.run_id = Some(value);
        self
    }

    pub fn with_lease_id(mut self, value: LeaseId) -> Self {
        self.lease_id = Some(value);
        self
    }

    pub fn with_frame_id(mut self, value: FrameId) -> Self {
        self.frame_id = Some(value);
        self
    }

    pub fn with_action_id(mut self, value: ActionId) -> Self {
        self.action_id = Some(value);
        self
    }

    pub fn with_recognition_id(mut self, value: RecognitionId) -> Self {
        self.recognition_id = Some(value);
        self
    }

    pub const fn instance_id(&self) -> Option<&InstanceId> {
        self.instance_id.as_ref()
    }

    pub const fn request_id(&self) -> Option<&RequestId> {
        self.request_id.as_ref()
    }

    pub const fn correlation_id(&self) -> Option<&CorrelationId> {
        self.correlation_id.as_ref()
    }

    pub const fn causation_id(&self) -> Option<&CausationId> {
        self.causation_id.as_ref()
    }

    pub const fn task_id(&self) -> Option<&TaskId> {
        self.task_id.as_ref()
    }

    pub const fn run_id(&self) -> Option<&RunId> {
        self.run_id.as_ref()
    }

    pub const fn lease_id(&self) -> Option<&LeaseId> {
        self.lease_id.as_ref()
    }

    pub const fn frame_id(&self) -> Option<&FrameId> {
        self.frame_id.as_ref()
    }

    pub const fn action_id(&self) -> Option<&ActionId> {
        self.action_id.as_ref()
    }

    pub const fn recognition_id(&self) -> Option<&RecognitionId> {
        self.recognition_id.as_ref()
    }
}

pub struct EventDraft {
    event_id: EventId,
    timestamp_unix_ms: u64,
    severity: EventSeverity,
    origin: EventOrigin,
    links: EventLinks,
    payload: EventPayloadDraft,
    artifacts: Vec<ArtifactReference>,
}

impl EventDraft {
    pub fn new(
        event_id: EventId,
        timestamp_unix_ms: u64,
        severity: EventSeverity,
        origin: EventOrigin,
        links: EventLinks,
        payload: EventPayloadDraft,
    ) -> Self {
        Self {
            event_id,
            timestamp_unix_ms,
            severity,
            origin,
            links,
            payload,
            artifacts: Vec::new(),
        }
    }

    pub fn with_artifacts(mut self, artifacts: Vec<ArtifactReference>) -> Self {
        self.artifacts = artifacts;
        self
    }

    pub fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<SanitizedEventDraft, SanitizationError> {
        if self.timestamp_unix_ms == 0 {
            return Err(SanitizationError::new(
                "invalid_timestamp",
                "timestamp_unix_ms",
            ));
        }
        for artifact in &self.artifacts {
            artifact.validate()?;
        }
        let payload = self.payload.sanitize(fingerprinter)?;
        payload.validate()?;
        Ok(SanitizedEventDraft {
            schema_version: GLOBAL_EVENT_SCHEMA_VERSION.to_string(),
            event_id: self.event_id,
            timestamp_unix_ms: self.timestamp_unix_ms,
            event_type: payload.event_type(),
            severity: self.severity,
            sensitivity: payload.sensitivity(),
            origin: self.origin,
            links: self.links,
            payload_schema: payload.schema().to_string(),
            payload,
            artifacts: self.artifacts,
        })
    }
}

impl fmt::Debug for EventDraft {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EventDraft")
            .field("event_id", &self.event_id)
            .field("timestamp_unix_ms", &self.timestamp_unix_ms)
            .field("severity", &self.severity)
            .field("origin", &self.origin)
            .field("links", &self.links)
            .field("payload", &"<redacted-raw-payload>")
            .field("artifact_count", &self.artifacts.len())
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct SanitizedEventDraft {
    schema_version: String,
    event_id: EventId,
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

impl SanitizedEventDraft {
    pub fn schema_version(&self) -> &str {
        &self.schema_version
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
}

impl fmt::Debug for SanitizedEventDraft {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SanitizedEventDraft")
            .field("event_id", &self.event_id)
            .field("event_type", &self.event_type)
            .field("severity", &self.severity)
            .field("sensitivity", &self.sensitivity)
            .field("payload", &"<sanitized-payload>")
            .field("artifact_count", &self.artifacts.len())
            .finish()
    }
}
