// SPDX-License-Identifier: AGPL-3.0-only

//! Typed global event contracts shared by Runtime producers and ledger adapters.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

pub const GLOBAL_EVENT_SCHEMA_VERSION: &str = "actingcommand.event.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventSeverity {
    Debug,
    Info,
    Warning,
    Error,
    Fatal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sensitivity {
    Public,
    Internal,
    Sensitive,
    Secret,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedactionPolicy {
    Keep,
    Mask,
    Fingerprint,
    Drop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventFamily {
    Command,
    Scheduler,
    Lease,
    Task,
    Capture,
    Recognition,
    Input,
    Module,
    Fallback,
    Artifact,
    Client,
    Runtime,
    Ledger,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventType {
    #[serde(rename = "command.received")]
    CommandReceived,
    #[serde(rename = "command.validated")]
    CommandValidated,
    #[serde(rename = "command.rejected")]
    CommandRejected,
    #[serde(rename = "scheduler.admitted")]
    SchedulerAdmitted,
    #[serde(rename = "scheduler.queued")]
    SchedulerQueued,
    #[serde(rename = "scheduler.denied")]
    SchedulerDenied,
    #[serde(rename = "scheduler.preempted")]
    SchedulerPreempted,
    #[serde(rename = "lease.requested")]
    LeaseRequested,
    #[serde(rename = "lease.granted")]
    LeaseGranted,
    #[serde(rename = "lease.transferred")]
    LeaseTransferred,
    #[serde(rename = "lease.released")]
    LeaseReleased,
    #[serde(rename = "lease.expired")]
    LeaseExpired,
    #[serde(rename = "task.requested")]
    TaskRequested,
    #[serde(rename = "task.started")]
    TaskStarted,
    #[serde(rename = "task.step_started")]
    TaskStepStarted,
    #[serde(rename = "task.step_finished")]
    TaskStepFinished,
    #[serde(rename = "task.completed")]
    TaskCompleted,
    #[serde(rename = "task.failed")]
    TaskFailed,
    #[serde(rename = "task.cancelled")]
    TaskCancelled,
    #[serde(rename = "capture.requested")]
    CaptureRequested,
    #[serde(rename = "capture.completed")]
    CaptureCompleted,
    #[serde(rename = "capture.failed")]
    CaptureFailed,
    #[serde(rename = "capture.stale_detected")]
    CaptureStaleDetected,
    #[serde(rename = "capture.pressure_changed")]
    CapturePressureChanged,
    #[serde(rename = "capture.dedup_window")]
    CaptureDedupWindow,
    #[serde(rename = "capture.policy_changed")]
    CapturePolicyChanged,
    #[serde(rename = "recognition.requested")]
    RecognitionRequested,
    #[serde(rename = "recognition.completed")]
    RecognitionCompleted,
    #[serde(rename = "recognition.failed")]
    RecognitionFailed,
    #[serde(rename = "input.intent")]
    InputIntent,
    #[serde(rename = "input.committed")]
    InputCommitted,
    #[serde(rename = "input.completed")]
    InputCompleted,
    #[serde(rename = "input.failed")]
    InputFailed,
    #[serde(rename = "module.health_changed")]
    ModuleHealthChanged,
    #[serde(rename = "fallback.started")]
    FallbackStarted,
    #[serde(rename = "fallback.completed")]
    FallbackCompleted,
    #[serde(rename = "fallback.escalated")]
    FallbackEscalated,
    #[serde(rename = "artifact.created")]
    ArtifactCreated,
    #[serde(rename = "artifact.verified")]
    ArtifactVerified,
    #[serde(rename = "artifact.export_completed")]
    ArtifactExportCompleted,
    #[serde(rename = "artifact.export_failed")]
    ArtifactExportFailed,
    #[serde(rename = "ui.action")]
    UiAction,
    #[serde(rename = "cli.command")]
    CliCommand,
    #[serde(rename = "lab.request")]
    LabRequest,
    #[serde(rename = "runtime.started")]
    RuntimeStarted,
    #[serde(rename = "runtime.stopped")]
    RuntimeStopped,
    #[serde(rename = "runtime.recovered")]
    RuntimeRecovered,
    #[serde(rename = "runtime.owner_takeover")]
    RuntimeOwnerTakeover,
    #[serde(rename = "runtime.config_changed")]
    RuntimeConfigChanged,
    #[serde(rename = "ledger.recovered")]
    LedgerRecovered,
}

impl EventType {
    pub fn family(self) -> EventFamily {
        match self {
            Self::CommandReceived | Self::CommandValidated | Self::CommandRejected => {
                EventFamily::Command
            }
            Self::SchedulerAdmitted
            | Self::SchedulerQueued
            | Self::SchedulerDenied
            | Self::SchedulerPreempted => EventFamily::Scheduler,
            Self::LeaseRequested
            | Self::LeaseGranted
            | Self::LeaseTransferred
            | Self::LeaseReleased
            | Self::LeaseExpired => EventFamily::Lease,
            Self::TaskRequested
            | Self::TaskStarted
            | Self::TaskStepStarted
            | Self::TaskStepFinished
            | Self::TaskCompleted
            | Self::TaskFailed
            | Self::TaskCancelled => EventFamily::Task,
            Self::CaptureRequested
            | Self::CaptureCompleted
            | Self::CaptureFailed
            | Self::CaptureStaleDetected
            | Self::CapturePressureChanged
            | Self::CaptureDedupWindow
            | Self::CapturePolicyChanged => EventFamily::Capture,
            Self::RecognitionRequested | Self::RecognitionCompleted | Self::RecognitionFailed => {
                EventFamily::Recognition
            }
            Self::InputIntent | Self::InputCommitted | Self::InputCompleted | Self::InputFailed => {
                EventFamily::Input
            }
            Self::ModuleHealthChanged => EventFamily::Module,
            Self::FallbackStarted | Self::FallbackCompleted | Self::FallbackEscalated => {
                EventFamily::Fallback
            }
            Self::ArtifactCreated
            | Self::ArtifactVerified
            | Self::ArtifactExportCompleted
            | Self::ArtifactExportFailed => EventFamily::Artifact,
            Self::UiAction | Self::CliCommand | Self::LabRequest => EventFamily::Client,
            Self::RuntimeStarted
            | Self::RuntimeStopped
            | Self::RuntimeRecovered
            | Self::RuntimeOwnerTakeover
            | Self::RuntimeConfigChanged => EventFamily::Runtime,
            Self::LedgerRecovered => EventFamily::Ledger,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventSource {
    Runtime,
    Scheduler,
    Device,
    Cli,
    Ui,
    Lab,
    System,
    Adapter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventActor {
    User,
    Runtime,
    Scheduler,
    Cli,
    Ui,
    Lab,
    Agent,
    System,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventOrigin {
    source: EventSource,
    module: String,
    actor: EventActor,
}

impl EventOrigin {
    pub fn new(
        source: EventSource,
        module: impl Into<String>,
        actor: EventActor,
    ) -> Result<Self, SanitizationError> {
        let module = module.into();
        validate_identifier("module", &module)?;
        Ok(Self {
            source,
            module,
            actor,
        })
    }

    pub fn source(&self) -> EventSource {
        self.source
    }

    pub fn module(&self) -> &str {
        &self.module
    }

    pub fn actor(&self) -> EventActor {
        self.actor
    }

    fn validate(&self) -> Result<(), SanitizationError> {
        validate_identifier("module", &self.module)
    }
}

impl fmt::Debug for EventOrigin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EventOrigin")
            .field("source", &self.source)
            .field("module", &"<validated-module>")
            .field("actor", &self.actor)
            .finish()
    }
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventLinks {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub causation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reco_id: Option<String>,
}

impl fmt::Debug for EventLinks {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EventLinks")
            .field("instance_id", &self.instance_id.is_some())
            .field("request_id", &self.request_id.is_some())
            .field("correlation_id", &self.correlation_id.is_some())
            .field("causation_id", &self.causation_id.is_some())
            .field("task_id", &self.task_id.is_some())
            .field("run_id", &self.run_id.is_some())
            .field("lease_id", &self.lease_id.is_some())
            .field("frame_id", &self.frame_id.is_some())
            .field("action_id", &self.action_id.is_some())
            .field("reco_id", &self.reco_id.is_some())
            .finish()
    }
}

impl EventLinks {
    fn validate(&self) -> Result<(), SanitizationError> {
        for (name, value) in [
            ("instance_id", self.instance_id.as_deref()),
            ("request_id", self.request_id.as_deref()),
            ("correlation_id", self.correlation_id.as_deref()),
            ("causation_id", self.causation_id.as_deref()),
            ("task_id", self.task_id.as_deref()),
            ("run_id", self.run_id.as_deref()),
            ("lease_id", self.lease_id.as_deref()),
            ("frame_id", self.frame_id.as_deref()),
            ("action_id", self.action_id.as_deref()),
            ("reco_id", self.reco_id.as_deref()),
        ] {
            if let Some(value) = value {
                validate_identifier(name, value)?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactRedactionState {
    NotRequired,
    Applied,
    Pending,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactReference {
    pub artifact_id: String,
    pub kind: String,
    pub relative_ref: String,
    pub sha256: String,
    pub redaction_state: ArtifactRedactionState,
}

impl fmt::Debug for ArtifactReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ArtifactReference")
            .field("identity", &"<unvalidated-artifact>")
            .field("redaction_state", &self.redaction_state)
            .finish()
    }
}

impl ArtifactReference {
    fn validate(&self) -> Result<(), SanitizationError> {
        validate_identifier("artifact_id", &self.artifact_id)?;
        validate_identifier("artifact_kind", &self.kind)?;
        if !is_safe_relative_ref(&self.relative_ref) {
            return Err(SanitizationError::new(
                "invalid_artifact_reference",
                "relative_ref",
            ));
        }
        if !is_sha256(&self.sha256) {
            return Err(SanitizationError::new("invalid_artifact_hash", "sha256"));
        }
        Ok(())
    }
}

pub trait FieldRedactor {
    fn fingerprint(&self, field_name: &str, value: &str) -> Result<String, SanitizationError>;
}

#[derive(Clone)]
pub struct ClassifiedField {
    name: String,
    value: String,
    sensitivity: Sensitivity,
    policy: RedactionPolicy,
}

impl fmt::Debug for ClassifiedField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClassifiedField")
            .field("name", &self.name)
            .field("value", &"<redacted>")
            .field("sensitivity", &self.sensitivity)
            .field("policy", &self.policy)
            .finish()
    }
}

impl ClassifiedField {
    pub fn new(
        name: impl Into<String>,
        value: impl Into<String>,
        sensitivity: Sensitivity,
        policy: RedactionPolicy,
    ) -> Result<Self, SanitizationError> {
        let name = name.into();
        validate_identifier("field_name", &name)?;
        if !policy_allowed(sensitivity, policy) {
            return Err(SanitizationError::new("invalid_redaction_policy", &name));
        }
        Ok(Self {
            name,
            value: value.into(),
            sensitivity,
            policy,
        })
    }

    pub fn public(
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<Self, SanitizationError> {
        Self::new(name, value, Sensitivity::Public, RedactionPolicy::Keep)
    }

    pub fn internal(
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<Self, SanitizationError> {
        Self::new(name, value, Sensitivity::Internal, RedactionPolicy::Keep)
    }

    pub fn sensitive_mask(
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<Self, SanitizationError> {
        Self::new(name, value, Sensitivity::Sensitive, RedactionPolicy::Mask)
    }

    pub fn secret_fingerprint(
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<Self, SanitizationError> {
        Self::new(
            name,
            value,
            Sensitivity::Secret,
            RedactionPolicy::Fingerprint,
        )
    }

    pub fn secret_drop(
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<Self, SanitizationError> {
        Self::new(name, value, Sensitivity::Secret, RedactionPolicy::Drop)
    }

    fn sanitize(self, redactor: &dyn FieldRedactor) -> Result<SanitizedField, SanitizationError> {
        let (value, fingerprint) = match self.policy {
            RedactionPolicy::Keep => (Some(self.value), None),
            RedactionPolicy::Mask => (Some("[redacted]".to_string()), None),
            RedactionPolicy::Fingerprint => {
                let fingerprint = redactor
                    .fingerprint(&self.name, &self.value)
                    .map_err(|_| SanitizationError::new("redactor_failed", &self.name))?;
                let echoed_original = fingerprint
                    .strip_prefix("sha256:")
                    .is_some_and(|digest| digest == self.value);
                if !is_sha256(&fingerprint) || echoed_original {
                    return Err(SanitizationError::new("invalid_fingerprint", &self.name));
                }
                (None, Some(fingerprint))
            }
            RedactionPolicy::Drop => (None, None),
        };
        Ok(SanitizedField {
            name: self.name,
            sensitivity: self.sensitivity,
            policy: self.policy,
            value,
            fingerprint,
            redacted: self.policy != RedactionPolicy::Keep,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SanitizedField {
    name: String,
    sensitivity: Sensitivity,
    policy: RedactionPolicy,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fingerprint: Option<String>,
    redacted: bool,
}

mod sealed {
    pub trait PayloadKind {}
    pub trait SanitizedPayload {}
}

pub trait PayloadKind: sealed::PayloadKind + Serialize + Clone + Send + Sync + 'static {
    const SCHEMA: &'static str;
    const FAMILY: EventFamily;

    fn event_type(&self) -> EventType;
}

pub trait RedactablePayload {
    type Sanitized: SanitizedPayload;

    fn event_type(&self) -> EventType;
    fn sanitize(self, redactor: &dyn FieldRedactor) -> Result<Self::Sanitized, SanitizationError>;
}

pub trait SanitizedPayload:
    sealed::SanitizedPayload + Serialize + Clone + Send + Sync + 'static
{
    const SCHEMA: &'static str;
    const FAMILY: EventFamily;

    fn sensitivity(&self) -> Sensitivity;
}

#[derive(Clone)]
pub struct StructuredPayloadDraft<K> {
    kind: K,
    subject: String,
    fields: Vec<ClassifiedField>,
}

impl<K: fmt::Debug> fmt::Debug for StructuredPayloadDraft<K> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StructuredPayloadDraft")
            .field("kind", &self.kind)
            .field("subject", &self.subject)
            .field("fields", &self.fields)
            .finish()
    }
}

impl<K: PayloadKind> StructuredPayloadDraft<K> {
    pub fn new(
        kind: K,
        subject: impl Into<String>,
        fields: Vec<ClassifiedField>,
    ) -> Result<Self, SanitizationError> {
        let subject = subject.into();
        validate_identifier("payload_subject", &subject)?;
        let mut names = BTreeSet::new();
        for field in &fields {
            if !names.insert(field.name.as_str()) {
                return Err(SanitizationError::new(
                    "duplicate_payload_field",
                    &field.name,
                ));
            }
        }
        Ok(Self {
            kind,
            subject,
            fields,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StructuredPayload<K> {
    kind: K,
    subject: String,
    fields: Vec<SanitizedField>,
    sensitivity: Sensitivity,
}

impl<K: PayloadKind> RedactablePayload for StructuredPayloadDraft<K> {
    type Sanitized = StructuredPayload<K>;

    fn event_type(&self) -> EventType {
        self.kind.event_type()
    }

    fn sanitize(self, redactor: &dyn FieldRedactor) -> Result<Self::Sanitized, SanitizationError> {
        let mut sensitivity = Sensitivity::Public;
        let mut fields = Vec::with_capacity(self.fields.len());
        for field in self.fields {
            sensitivity = sensitivity.max(field.sensitivity);
            fields.push(field.sanitize(redactor)?);
        }
        Ok(StructuredPayload {
            kind: self.kind,
            subject: self.subject,
            fields,
            sensitivity,
        })
    }
}

impl<K: PayloadKind> sealed::SanitizedPayload for StructuredPayload<K> {}

impl<K: PayloadKind> SanitizedPayload for StructuredPayload<K> {
    const SCHEMA: &'static str = K::SCHEMA;
    const FAMILY: EventFamily = K::FAMILY;

    fn sensitivity(&self) -> Sensitivity {
        self.sensitivity
    }
}

macro_rules! payload_kind {
    ($name:ident, $schema:literal, $family:expr, { $($variant:ident => $event:expr),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $($variant),+
        }

        impl PayloadKind for $name {
            const SCHEMA: &'static str = $schema;
            const FAMILY: EventFamily = $family;

            fn event_type(&self) -> EventType {
                match self {
                    $(Self::$variant => $event),+
                }
            }
        }

        impl sealed::PayloadKind for $name {}
    };
}

payload_kind!(CommandStage, "actingcommand.payload.command.v1", EventFamily::Command, {
    Received => EventType::CommandReceived,
    Validated => EventType::CommandValidated,
    Rejected => EventType::CommandRejected,
});
payload_kind!(SchedulerDecision, "actingcommand.payload.scheduler.v1", EventFamily::Scheduler, {
    Admitted => EventType::SchedulerAdmitted,
    Queued => EventType::SchedulerQueued,
    Denied => EventType::SchedulerDenied,
    Preempted => EventType::SchedulerPreempted,
});
payload_kind!(LeaseTransition, "actingcommand.payload.lease.v1", EventFamily::Lease, {
    Requested => EventType::LeaseRequested,
    Granted => EventType::LeaseGranted,
    Transferred => EventType::LeaseTransferred,
    Released => EventType::LeaseReleased,
    Expired => EventType::LeaseExpired,
});
payload_kind!(TaskTransition, "actingcommand.payload.task.v1", EventFamily::Task, {
    Requested => EventType::TaskRequested,
    Started => EventType::TaskStarted,
    StepStarted => EventType::TaskStepStarted,
    StepFinished => EventType::TaskStepFinished,
    Completed => EventType::TaskCompleted,
    Failed => EventType::TaskFailed,
    Cancelled => EventType::TaskCancelled,
});
payload_kind!(InputTransition, "actingcommand.payload.input.v1", EventFamily::Input, {
    Intent => EventType::InputIntent,
    Committed => EventType::InputCommitted,
    Completed => EventType::InputCompleted,
    Failed => EventType::InputFailed,
});
payload_kind!(ClientActionKind, "actingcommand.payload.client.v1", EventFamily::Client, {
    UiAction => EventType::UiAction,
    CliCommand => EventType::CliCommand,
    LabRequest => EventType::LabRequest,
});
payload_kind!(LedgerTransition, "actingcommand.payload.ledger.v1", EventFamily::Ledger, {
    Recovered => EventType::LedgerRecovered,
});

pub type CommandPayloadDraft = StructuredPayloadDraft<CommandStage>;
pub type CommandPayload = StructuredPayload<CommandStage>;
pub type SchedulerPayloadDraft = StructuredPayloadDraft<SchedulerDecision>;
pub type SchedulerPayload = StructuredPayload<SchedulerDecision>;
pub type LeasePayloadDraft = StructuredPayloadDraft<LeaseTransition>;
pub type LeasePayload = StructuredPayload<LeaseTransition>;
pub type TaskPayloadDraft = StructuredPayloadDraft<TaskTransition>;
pub type TaskPayload = StructuredPayload<TaskTransition>;
pub type InputPayloadDraft = StructuredPayloadDraft<InputTransition>;
pub type InputPayload = StructuredPayload<InputTransition>;
pub type ClientPayloadDraft = StructuredPayloadDraft<ClientActionKind>;
pub type ClientPayload = StructuredPayload<ClientActionKind>;
pub type LedgerPayloadDraft = StructuredPayloadDraft<LedgerTransition>;
pub type LedgerPayload = StructuredPayload<LedgerTransition>;

#[derive(Clone)]
pub struct EventDraft<P> {
    event_id: String,
    timestamp_unix_ms: u64,
    event_type: EventType,
    severity: EventSeverity,
    origin: EventOrigin,
    links: EventLinks,
    artifacts: Vec<ArtifactReference>,
    payload: P,
}

impl<P> fmt::Debug for EventDraft<P> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EventDraft")
            .field("event_id", &"<unvalidated-event-id>")
            .field("timestamp_unix_ms", &self.timestamp_unix_ms)
            .field("event_type", &self.event_type)
            .field("severity", &self.severity)
            .field("origin", &self.origin)
            .field("links", &"<unvalidated-links>")
            .field("artifact_count", &self.artifacts.len())
            .field("payload", &"<redacted-payload>")
            .finish()
    }
}

impl<P> EventDraft<P> {
    pub fn new(
        event_id: impl Into<String>,
        timestamp_unix_ms: u64,
        event_type: EventType,
        severity: EventSeverity,
        origin: EventOrigin,
        links: EventLinks,
        payload: P,
    ) -> Self {
        Self {
            event_id: event_id.into(),
            timestamp_unix_ms,
            event_type,
            severity,
            origin,
            links,
            artifacts: Vec::new(),
            payload,
        }
    }

    pub fn with_artifacts(mut self, artifacts: Vec<ArtifactReference>) -> Self {
        self.artifacts = artifacts;
        self
    }
}

impl<P: RedactablePayload> EventDraft<P> {
    pub fn sanitize(
        self,
        redactor: &dyn FieldRedactor,
    ) -> Result<SanitizedEventDraft<P::Sanitized>, SanitizationError> {
        validate_identifier("event_id", &self.event_id)?;
        if self.timestamp_unix_ms == 0 {
            return Err(SanitizationError::new(
                "invalid_timestamp",
                "timestamp_unix_ms",
            ));
        }
        self.origin.validate()?;
        self.links.validate()?;
        for artifact in &self.artifacts {
            artifact.validate()?;
        }
        if self.payload.event_type() != self.event_type
            || P::Sanitized::FAMILY != self.event_type.family()
        {
            return Err(SanitizationError::new(
                "payload_family_mismatch",
                "event_type",
            ));
        }
        let payload = self.payload.sanitize(redactor)?;
        Ok(SanitizedEventDraft {
            schema_version: GLOBAL_EVENT_SCHEMA_VERSION.to_string(),
            event_id: self.event_id,
            timestamp_unix_ms: self.timestamp_unix_ms,
            event_type: self.event_type,
            severity: self.severity,
            sensitivity: payload.sensitivity(),
            origin: self.origin,
            links: self.links,
            payload_schema: P::Sanitized::SCHEMA.to_string(),
            payload,
            artifacts: self.artifacts,
        })
    }
}

/// A draft that has crossed the declared redaction schema.
///
/// Callers cannot rewrite its fields or construct it directly.
///
/// ```compile_fail
/// use actingcommand_contract::SanitizedEventDraft;
///
/// fn forge<P>(mut draft: SanitizedEventDraft<P>) {
///     draft.payload_schema = "unreviewed.payload".to_string();
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SanitizedEventDraft<P> {
    schema_version: String,
    event_id: String,
    timestamp_unix_ms: u64,
    event_type: EventType,
    severity: EventSeverity,
    sensitivity: Sensitivity,
    origin: EventOrigin,
    links: EventLinks,
    payload_schema: String,
    payload: P,
    artifacts: Vec<ArtifactReference>,
}

impl<P: SanitizedPayload> SanitizedEventDraft<P> {
    pub fn event_id(&self) -> &str {
        &self.event_id
    }

    pub fn event_type(&self) -> EventType {
        self.event_type
    }

    pub fn links(&self) -> &EventLinks {
        &self.links
    }

    pub fn sensitivity(&self) -> Sensitivity {
        self.sensitivity
    }

    pub fn erase(self) -> Result<ErasedSanitizedEventDraft, EventContractError> {
        let payload = serde_json::to_value(self.payload)
            .map_err(|_| EventContractError::new("payload_serialization_failed"))?;
        Ok(ErasedSanitizedEventDraft {
            schema_version: self.schema_version,
            event_id: self.event_id,
            timestamp_unix_ms: self.timestamp_unix_ms,
            event_type: self.event_type,
            severity: self.severity,
            sensitivity: self.sensitivity,
            origin: self.origin,
            links: self.links,
            payload_schema: self.payload_schema,
            payload,
            artifacts: self.artifacts,
        })
    }
}

/// Type-erased sanitized ingress produced only by `SanitizedEventDraft::erase`.
///
/// ```compile_fail
/// use actingcommand_contract::ErasedSanitizedEventDraft;
///
/// let _: ErasedSanitizedEventDraft = serde_json::from_str("{}").unwrap();
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct ErasedSanitizedEventDraft {
    schema_version: String,
    event_id: String,
    timestamp_unix_ms: u64,
    event_type: EventType,
    severity: EventSeverity,
    sensitivity: Sensitivity,
    origin: EventOrigin,
    links: EventLinks,
    payload_schema: String,
    payload: Value,
    artifacts: Vec<ArtifactReference>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistedEvent {
    pub schema_version: String,
    pub event_id: String,
    pub sequence: u64,
    pub timestamp_unix_ms: u64,
    pub event_type: EventType,
    pub severity: EventSeverity,
    pub sensitivity: Sensitivity,
    pub origin: EventOrigin,
    pub links: EventLinks,
    pub payload_schema: String,
    pub payload: Value,
    pub artifacts: Vec<ArtifactReference>,
}

impl PersistedEvent {
    pub fn from_draft(sequence: u64, draft: ErasedSanitizedEventDraft) -> Self {
        Self {
            schema_version: draft.schema_version,
            event_id: draft.event_id,
            sequence,
            timestamp_unix_ms: draft.timestamp_unix_ms,
            event_type: draft.event_type,
            severity: draft.severity,
            sensitivity: draft.sensitivity,
            origin: draft.origin,
            links: draft.links,
            payload_schema: draft.payload_schema,
            payload: draft.payload,
            artifacts: draft.artifacts,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventQuery {
    pub from_sequence: Option<u64>,
    pub to_sequence: Option<u64>,
    pub event_type: Option<EventType>,
    pub minimum_severity: Option<EventSeverity>,
    pub source: Option<EventSource>,
    pub instance_id: Option<String>,
    pub request_id: Option<String>,
    pub correlation_id: Option<String>,
    pub causation_id: Option<String>,
    pub task_id: Option<String>,
    pub run_id: Option<String>,
    pub lease_id: Option<String>,
    pub frame_id: Option<String>,
    pub action_id: Option<String>,
    pub reco_id: Option<String>,
}

impl EventQuery {
    pub fn matches(&self, event: &PersistedEvent) -> bool {
        self.from_sequence
            .is_none_or(|value| event.sequence >= value)
            && self.to_sequence.is_none_or(|value| event.sequence <= value)
            && self
                .event_type
                .is_none_or(|value| event.event_type == value)
            && self
                .minimum_severity
                .is_none_or(|value| event.severity >= value)
            && self.source.is_none_or(|value| event.origin.source == value)
            && link_matches(&self.instance_id, &event.links.instance_id)
            && link_matches(&self.request_id, &event.links.request_id)
            && link_matches(&self.correlation_id, &event.links.correlation_id)
            && link_matches(&self.causation_id, &event.links.causation_id)
            && link_matches(&self.task_id, &event.links.task_id)
            && link_matches(&self.run_id, &event.links.run_id)
            && link_matches(&self.lease_id, &event.links.lease_id)
            && link_matches(&self.frame_id, &event.links.frame_id)
            && link_matches(&self.action_id, &event.links.action_id)
            && link_matches(&self.reco_id, &event.links.reco_id)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscriptionCursor {
    pub after_sequence: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionProfile {
    Cli,
    Ui,
    Lab,
    Concise,
    Normal,
    Verbose,
    Forensic,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectedEvent {
    pub sequence: u64,
    pub event_id: String,
    pub timestamp_unix_ms: u64,
    pub event_type: EventType,
    pub severity: EventSeverity,
    pub origin: EventOrigin,
    pub links: EventLinks,
    pub payload_schema: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
    pub artifacts: Vec<ArtifactReference>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SanitizationError {
    code: &'static str,
    field: String,
}

impl SanitizationError {
    fn new(code: &'static str, field: &str) -> Self {
        Self {
            code,
            field: field.to_string(),
        }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn redactor_failure() -> Self {
        Self::new("redactor_failed", "redactor")
    }
}

impl fmt::Display for SanitizationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "event sanitization failed with {} at {}",
            self.code, self.field
        )
    }
}

impl Error for SanitizationError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventContractError {
    code: &'static str,
}

impl EventContractError {
    fn new(code: &'static str) -> Self {
        Self { code }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }
}

impl fmt::Display for EventContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "event contract failed with {}", self.code)
    }
}

impl Error for EventContractError {}

fn validate_identifier(field: &str, value: &str) -> Result<(), SanitizationError> {
    if is_identifier(value) {
        Ok(())
    } else {
        Err(SanitizationError::new("invalid_identifier", field))
    }
}

fn is_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn policy_allowed(sensitivity: Sensitivity, policy: RedactionPolicy) -> bool {
    match sensitivity {
        Sensitivity::Public => policy == RedactionPolicy::Keep,
        Sensitivity::Internal => true,
        Sensitivity::Sensitive => policy != RedactionPolicy::Keep,
        Sensitivity::Secret => {
            matches!(policy, RedactionPolicy::Fingerprint | RedactionPolicy::Drop)
        }
    }
}

fn link_matches(expected: &Option<String>, actual: &Option<String>) -> bool {
    expected
        .as_ref()
        .is_none_or(|expected| actual.as_ref() == Some(expected))
}

fn is_safe_relative_ref(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && !value.starts_with('/')
        && !value.starts_with('\\')
        && !value.contains(':')
        && !value.contains('\\')
        && !value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
}

fn is_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestRedactor;

    impl FieldRedactor for TestRedactor {
        fn fingerprint(&self, field_name: &str, value: &str) -> Result<String, SanitizationError> {
            let byte = if field_name == "account" { 'a' } else { 'b' };
            let _ = value;
            Ok(format!("sha256:{}", byte.to_string().repeat(64)))
        }
    }

    struct EchoRedactor;

    impl FieldRedactor for EchoRedactor {
        fn fingerprint(&self, _field_name: &str, value: &str) -> Result<String, SanitizationError> {
            Ok(format!("sha256:{value}"))
        }
    }

    struct LeakingErrorRedactor;

    impl FieldRedactor for LeakingErrorRedactor {
        fn fingerprint(&self, _field_name: &str, value: &str) -> Result<String, SanitizationError> {
            Err(SanitizationError::new("redactor_failed", value))
        }
    }

    struct ValidShortSecretRedactor;

    impl FieldRedactor for ValidShortSecretRedactor {
        fn fingerprint(
            &self,
            _field_name: &str,
            _value: &str,
        ) -> Result<String, SanitizationError> {
            Ok(format!("sha256:{}", "a1".repeat(32)))
        }
    }

    #[derive(Debug, Clone, Serialize)]
    enum MismatchedKind {
        Command,
    }

    impl sealed::PayloadKind for MismatchedKind {}

    impl PayloadKind for MismatchedKind {
        const SCHEMA: &'static str = "actingcommand.payload.mismatch.v1";
        const FAMILY: EventFamily = EventFamily::Input;

        fn event_type(&self) -> EventType {
            EventType::CommandReceived
        }
    }

    fn command_draft(fields: Vec<ClassifiedField>) -> EventDraft<CommandPayloadDraft> {
        let payload = CommandPayloadDraft::new(CommandStage::Received, "runtime.start", fields)
            .expect("command payload");
        EventDraft::new(
            "evt-command-1",
            1_752_147_200_000,
            EventType::CommandReceived,
            EventSeverity::Info,
            EventOrigin::new(EventSource::Cli, "actingctl", EventActor::User).expect("origin"),
            EventLinks {
                request_id: Some("req-1".to_string()),
                correlation_id: Some("corr-1".to_string()),
                ..EventLinks::default()
            },
            payload,
        )
    }

    #[test]
    fn raw_event_draft_is_sanitized_before_serialization() {
        let draft = command_draft(vec![
            ClassifiedField::public("mode", "manual").expect("public field"),
        ]);

        let sanitized = draft.sanitize(&TestRedactor).expect("sanitize");
        let json = serde_json::to_string(&sanitized).expect("serialize sanitized event");

        assert!(json.contains("command.received"));
        assert!(json.contains("runtime.start"));
        assert!(json.contains("manual"));
    }

    #[test]
    fn secret_and_sensitive_fields_never_survive_sanitization() {
        let token = "token-c1-negative-7ad1";
        let account = "alice@example.invalid";
        let machine_path = r"C:\Users\Alice\private\config.json";
        let endpoint = "127.0.0.1:16384";
        let draft = command_draft(vec![
            ClassifiedField::secret_fingerprint("account", account).expect("account field"),
            ClassifiedField::secret_drop("token", token).expect("token field"),
            ClassifiedField::sensitive_mask("machine_path", machine_path).expect("path field"),
            ClassifiedField::sensitive_mask("device_endpoint", endpoint).expect("endpoint field"),
        ]);

        let sanitized = draft.sanitize(&TestRedactor).expect("sanitize");
        let json = serde_json::to_string(&sanitized).expect("serialize sanitized event");

        for original in [token, account, machine_path, endpoint] {
            assert!(
                !json.contains(original),
                "secret original leaked: {original}"
            );
        }
        assert!(json.contains("sha256:"));
        assert!(json.contains("[redacted]"));
    }

    #[test]
    fn invalid_keep_policy_for_secret_is_rejected_without_value_in_error() {
        let secret = "must-not-appear-in-error-2f90";

        let error =
            ClassifiedField::new("token", secret, Sensitivity::Secret, RedactionPolicy::Keep)
                .expect_err("secret keep must fail");

        assert_eq!(error.code(), "invalid_redaction_policy");
        assert!(!error.to_string().contains(secret));
    }

    #[test]
    fn redactor_failure_does_not_disclose_a_secret_value() {
        let secret = "redactor-secret-c8e1";

        let error = SanitizationError::redactor_failure();

        assert_eq!(error.code(), "redactor_failed");
        assert!(!error.to_string().contains(secret));
    }

    #[test]
    fn origin_is_revalidated_during_sanitization() {
        let injected = r"C:\Users\Alice\private";
        let origin: EventOrigin = serde_json::from_value(serde_json::json!({
            "source": "cli",
            "module": injected,
            "actor": "user"
        }))
        .expect("deserialize forged origin");
        let payload = CommandPayloadDraft::new(CommandStage::Received, "runtime.start", vec![])
            .expect("payload");
        let draft = EventDraft::new(
            "evt-origin-1",
            1_752_147_200_000,
            EventType::CommandReceived,
            EventSeverity::Info,
            origin,
            EventLinks::default(),
            payload,
        );

        let error = draft
            .sanitize(&TestRedactor)
            .expect_err("forged origin must fail");

        assert_eq!(error.code(), "invalid_identifier");
        assert!(!error.to_string().contains(injected));
    }

    #[test]
    fn fingerprint_must_be_fixed_sha256_and_must_not_embed_original() {
        let secret = "a".repeat(64);
        let draft = command_draft(vec![
            ClassifiedField::secret_fingerprint("token", &secret).expect("secret field"),
        ]);

        let error = draft
            .sanitize(&EchoRedactor)
            .expect_err("echo fingerprint must fail");

        assert_eq!(error.code(), "invalid_fingerprint");
        assert!(!error.to_string().contains(&secret));
    }

    #[test]
    fn valid_sha256_is_not_rejected_for_containing_a_short_secret() {
        let draft = command_draft(vec![
            ClassifiedField::secret_fingerprint("token", "a").expect("secret field"),
        ]);

        let sanitized = draft
            .sanitize(&ValidShortSecretRedactor)
            .expect("valid fixed fingerprint");
        let json = serde_json::to_string(&sanitized).expect("serialize");

        assert!(json.contains("sha256:"));
    }

    #[test]
    fn payload_declared_family_must_match_its_event_type_family() {
        let payload = StructuredPayloadDraft::new(MismatchedKind::Command, "runtime.start", vec![])
            .expect("payload");
        let draft = EventDraft::new(
            "evt-mismatch-1",
            1_752_147_200_000,
            EventType::CommandReceived,
            EventSeverity::Info,
            EventOrigin::new(EventSource::Cli, "actingctl", EventActor::User).expect("origin"),
            EventLinks::default(),
            payload,
        );

        let error = draft
            .sanitize(&TestRedactor)
            .expect_err("declared family mismatch must fail");

        assert_eq!(error.code(), "payload_family_mismatch");
    }

    #[test]
    fn redactor_error_is_mapped_to_the_declared_field_without_original() {
        let secret = "leaking-error-secret-7f3c";
        let draft = command_draft(vec![
            ClassifiedField::secret_fingerprint("token", secret).expect("secret field"),
        ]);

        let error = draft
            .sanitize(&LeakingErrorRedactor)
            .expect_err("redactor failure must fail");

        assert_eq!(error.code(), "redactor_failed");
        assert!(!error.to_string().contains(secret));
    }

    #[test]
    fn raw_debug_does_not_disclose_classified_values() {
        let secret = "debug-secret-9b2a";
        let path = r"C:\private\debug.json";
        let draft = command_draft(vec![
            ClassifiedField::secret_drop("token", secret).expect("secret field"),
            ClassifiedField::sensitive_mask("machine_path", path).expect("path field"),
        ]);

        let debug = format!("{draft:?}");

        assert!(!debug.contains(secret));
        assert!(!debug.contains(path));
        assert!(debug.contains("<redacted-payload>"));
    }

    #[test]
    fn raw_debug_hides_unvalidated_event_links_and_artifact_values() {
        let event_secret = "event-secret-a102";
        let link_secret = "link-secret-b203";
        let artifact_secret = "artifact-secret-c304";
        let payload = CommandPayloadDraft::new(CommandStage::Received, "runtime.start", vec![])
            .expect("payload");
        let links = EventLinks {
            request_id: Some(link_secret.to_string()),
            ..EventLinks::default()
        };
        let artifact = ArtifactReference {
            artifact_id: artifact_secret.to_string(),
            kind: "capture".to_string(),
            relative_ref: artifact_secret.to_string(),
            sha256: format!("sha256:{}", "c".repeat(64)),
            redaction_state: ArtifactRedactionState::Pending,
        };
        let draft = EventDraft::new(
            event_secret,
            1_752_147_200_000,
            EventType::CommandReceived,
            EventSeverity::Info,
            EventOrigin::new(EventSource::Cli, "actingctl", EventActor::User).expect("origin"),
            links.clone(),
            payload,
        )
        .with_artifacts(vec![artifact.clone()]);

        let event_debug = format!("{draft:?}");
        let links_debug = format!("{links:?}");
        let artifact_debug = format!("{artifact:?}");

        for original in [event_secret, link_secret, artifact_secret] {
            assert!(!event_debug.contains(original));
            assert!(!links_debug.contains(original));
            assert!(!artifact_debug.contains(original));
        }
    }

    #[test]
    fn event_family_mismatch_is_rejected() {
        let payload = CommandPayloadDraft::new(CommandStage::Received, "runtime.start", vec![])
            .expect("payload");
        let draft = EventDraft::new(
            "evt-family-1",
            1_752_147_200_000,
            EventType::InputIntent,
            EventSeverity::Info,
            EventOrigin::new(EventSource::Cli, "actingctl", EventActor::User).expect("origin"),
            EventLinks::default(),
            payload,
        );

        let error = draft
            .sanitize(&TestRedactor)
            .expect_err("family mismatch must fail");

        assert_eq!(error.code(), "payload_family_mismatch");
    }

    #[test]
    fn identifier_fields_reject_paths_and_endpoints() {
        for invalid in [r"C:\private\request", "127.0.0.1:16384"] {
            let payload = CommandPayloadDraft::new(CommandStage::Received, "runtime.start", vec![])
                .expect("payload");
            let draft = EventDraft::new(
                "evt-invalid-id",
                1_752_147_200_000,
                EventType::CommandReceived,
                EventSeverity::Info,
                EventOrigin::new(EventSource::Cli, "actingctl", EventActor::User).expect("origin"),
                EventLinks {
                    request_id: Some(invalid.to_string()),
                    ..EventLinks::default()
                },
                payload,
            );

            let error = draft
                .sanitize(&TestRedactor)
                .expect_err("unsafe identifier must fail");
            assert_eq!(error.code(), "invalid_identifier");
            assert!(!error.to_string().contains(invalid));
        }
    }
}
