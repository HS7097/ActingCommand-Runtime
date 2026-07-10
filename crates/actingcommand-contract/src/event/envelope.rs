// SPDX-License-Identifier: AGPL-3.0-only

use super::{
    ActionId, ArtifactReference, CausationId, CorrelationId, EventActor, EventId, EventPayload,
    EventPayloadDraft, EventSeverity, EventSource, EventType, FrameId, GLOBAL_EVENT_SCHEMA_VERSION,
    InstanceId, IssuedActionId, IssuedCausationId, IssuedCorrelationId, IssuedEventId,
    IssuedFrameId, IssuedInstanceId, IssuedLeaseId, IssuedRecognitionId, IssuedRequestId,
    IssuedRunId, IssuedTaskId, LeaseId, OriginModule, RecognitionId, RequestId, RunId,
    SanitizationError, SecretFingerprinter, Sensitivity, StoreIssuedArtifact, TaskId,
};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventOrigin {
    source: EventSource,
    module: OriginModule,
    actor: EventActor,
}

impl EventOrigin {
    pub const fn new(source: EventSource, module: OriginModule, actor: EventActor) -> Self {
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

    pub const fn module(&self) -> OriginModule {
        self.module
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

/// Producer-only links whose values can only come from an identifier issuer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventLinksDraft {
    instance_id: Option<IssuedInstanceId>,
    request_id: Option<IssuedRequestId>,
    correlation_id: Option<IssuedCorrelationId>,
    causation_id: Option<IssuedCausationId>,
    task_id: Option<IssuedTaskId>,
    run_id: Option<IssuedRunId>,
    lease_id: Option<IssuedLeaseId>,
    frame_id: Option<IssuedFrameId>,
    action_id: Option<IssuedActionId>,
    recognition_id: Option<IssuedRecognitionId>,
}

impl EventLinksDraft {
    pub(crate) fn from_verified_runtime(
        instance_id: Option<InstanceId>,
        request_id: RequestId,
        correlation_id: CorrelationId,
        causation_id: Option<CausationId>,
        lease_id: Option<LeaseId>,
        action_id: Option<ActionId>,
    ) -> Self {
        Self {
            instance_id: instance_id.map(IssuedInstanceId::from_verified_transport),
            request_id: Some(IssuedRequestId::from_verified_transport(request_id)),
            correlation_id: Some(IssuedCorrelationId::from_verified_transport(correlation_id)),
            causation_id: causation_id.map(IssuedCausationId::from_verified_transport),
            task_id: None,
            run_id: None,
            lease_id: lease_id.map(IssuedLeaseId::from_verified_transport),
            frame_id: None,
            action_id: action_id.map(IssuedActionId::from_verified_transport),
            recognition_id: None,
        }
    }

    pub fn with_instance_id(mut self, value: IssuedInstanceId) -> Self {
        self.instance_id = Some(value);
        self
    }

    pub fn with_request_id(mut self, value: IssuedRequestId) -> Self {
        self.request_id = Some(value);
        self
    }

    pub fn with_correlation_id(mut self, value: IssuedCorrelationId) -> Self {
        self.correlation_id = Some(value);
        self
    }

    pub fn with_causation_id(mut self, value: IssuedCausationId) -> Self {
        self.causation_id = Some(value);
        self
    }

    pub fn with_task_id(mut self, value: IssuedTaskId) -> Self {
        self.task_id = Some(value);
        self
    }

    pub fn with_run_id(mut self, value: IssuedRunId) -> Self {
        self.run_id = Some(value);
        self
    }

    pub fn with_lease_id(mut self, value: IssuedLeaseId) -> Self {
        self.lease_id = Some(value);
        self
    }

    pub fn with_frame_id(mut self, value: IssuedFrameId) -> Self {
        self.frame_id = Some(value);
        self
    }

    pub fn with_action_id(mut self, value: IssuedActionId) -> Self {
        self.action_id = Some(value);
        self
    }

    pub fn with_recognition_id(mut self, value: IssuedRecognitionId) -> Self {
        self.recognition_id = Some(value);
        self
    }

    pub fn instance_id(&self) -> Option<&InstanceId> {
        self.instance_id.as_ref().map(IssuedInstanceId::transport)
    }

    pub fn request_id(&self) -> Option<&RequestId> {
        self.request_id.as_ref().map(IssuedRequestId::transport)
    }

    pub fn correlation_id(&self) -> Option<&CorrelationId> {
        self.correlation_id
            .as_ref()
            .map(IssuedCorrelationId::transport)
    }

    pub fn causation_id(&self) -> Option<&CausationId> {
        self.causation_id.as_ref().map(IssuedCausationId::transport)
    }

    pub fn task_id(&self) -> Option<&TaskId> {
        self.task_id.as_ref().map(IssuedTaskId::transport)
    }

    pub fn run_id(&self) -> Option<&RunId> {
        self.run_id.as_ref().map(IssuedRunId::transport)
    }

    pub fn lease_id(&self) -> Option<&LeaseId> {
        self.lease_id.as_ref().map(IssuedLeaseId::transport)
    }

    pub fn frame_id(&self) -> Option<&FrameId> {
        self.frame_id.as_ref().map(IssuedFrameId::transport)
    }

    pub fn action_id(&self) -> Option<&ActionId> {
        self.action_id.as_ref().map(IssuedActionId::transport)
    }

    pub fn recognition_id(&self) -> Option<&RecognitionId> {
        self.recognition_id
            .as_ref()
            .map(IssuedRecognitionId::transport)
    }

    pub(crate) fn into_transport(self) -> EventLinks {
        EventLinks {
            instance_id: self.instance_id.map(IssuedInstanceId::into_transport),
            request_id: self.request_id.map(IssuedRequestId::into_transport),
            correlation_id: self.correlation_id.map(IssuedCorrelationId::into_transport),
            causation_id: self.causation_id.map(IssuedCausationId::into_transport),
            task_id: self.task_id.map(IssuedTaskId::into_transport),
            run_id: self.run_id.map(IssuedRunId::into_transport),
            lease_id: self.lease_id.map(IssuedLeaseId::into_transport),
            frame_id: self.frame_id.map(IssuedFrameId::into_transport),
            action_id: self.action_id.map(IssuedActionId::into_transport),
            recognition_id: self.recognition_id.map(IssuedRecognitionId::into_transport),
        }
    }
}

pub struct EventDraft {
    event_id: IssuedEventId,
    timestamp_unix_ms: u64,
    severity: EventSeverity,
    origin: EventOrigin,
    links: EventLinksDraft,
    payload: EventPayloadDraft,
    artifacts: Vec<StoreIssuedArtifact>,
}

impl EventDraft {
    pub fn new(
        event_id: IssuedEventId,
        timestamp_unix_ms: u64,
        severity: EventSeverity,
        origin: EventOrigin,
        links: EventLinksDraft,
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

    pub fn with_artifacts(mut self, artifacts: Vec<StoreIssuedArtifact>) -> Self {
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
        let artifacts = self
            .artifacts
            .into_iter()
            .map(StoreIssuedArtifact::into_reference)
            .collect::<Vec<_>>();
        for artifact in &artifacts {
            artifact.validate()?;
        }
        let payload = self.payload.sanitize(fingerprinter)?;
        payload.validate()?;
        let sensitivity = artifacts
            .iter()
            .fold(payload.sensitivity(), |current, artifact| {
                current.max(artifact.sensitivity())
            });
        Ok(SanitizedEventDraft {
            schema_version: GLOBAL_EVENT_SCHEMA_VERSION.to_string(),
            event_id: self.event_id.into_transport(),
            timestamp_unix_ms: self.timestamp_unix_ms,
            event_type: payload.event_type(),
            severity: self.severity,
            sensitivity,
            origin: self.origin,
            links: self.links.into_transport(),
            payload_schema: payload.schema().to_string(),
            payload,
            artifacts,
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
