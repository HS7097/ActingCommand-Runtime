// SPDX-License-Identifier: AGPL-3.0-only

//! Typed local Runtime IPC contract shared by resident hosts and disposable clients.
//!
//! Read-only capture authority can only be issued by `IdentifierIssuer`:
//!
//! ```compile_fail
//! use actingcommand_contract::{InstanceId, OwnerEpoch, ReadOnlyCaptureCapability};
//!
//! fn forge(epoch: OwnerEpoch, instance: InstanceId) {
//!     let _ = ReadOnlyCaptureCapability::new(epoch, instance);
//! }
//! ```

use crate::{
    ActionId, ArtifactKind, ArtifactMediaType, ArtifactRedactionState, CausationId, CorrelationId,
    EffectDisposition, EventActor, EventId, EventLinksDraft, EventQuery, EventSource, FrameId,
    HolderId, IdentifierIssuanceError, IdentifierIssuer, InstanceId, IssuedCausationId,
    IssuedCorrelationId, IssuedFrameId, IssuedHolderId, IssuedRecognitionId, IssuedRequestId,
    LeaseId, OwnerEpoch, ProjectedArtifactReference, ProjectedEvent, ProjectionProfile,
    RecognitionId, RecognitionVerdict, RequestId,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::net::{IpAddr, SocketAddr};

pub const RUNTIME_REQUEST_SCHEMA_VERSION: &str = "actingcommand.runtime.request.v1";
pub const RUNTIME_RECEIPT_SCHEMA_VERSION: &str = "actingcommand.runtime.receipt.v1";
pub const RUNTIME_INFO_SCHEMA_VERSION: &str = "actingcommand.runtime.info.v1";
pub const RUNTIME_INFO_FILE: &str = "runtime-info.json";
pub const MAX_INSTANCE_ALIAS_BYTES: usize = 256;
pub const MAX_INPUT_TEXT_BYTES: usize = 4096;
pub const MAX_INPUT_KEY_BYTES: usize = 64;
pub const MAX_INPUT_DURATION_MS: u64 = 60_000;
pub const MAX_LEASE_QUEUE_TIMEOUT_MS: u64 = 3_600_000;
pub const MAX_READONLY_OBSERVATION_ARTIFACT_BYTES: u64 = 64 * 1024 * 1024;

pub type RuntimeContractResult<T> = Result<T, RuntimeContractError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeContractError {
    code: &'static str,
}

impl RuntimeContractError {
    const fn new(code: &'static str) -> Self {
        Self { code }
    }

    pub const fn code(self) -> &'static str {
        self.code
    }
}

impl fmt::Display for RuntimeContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "runtime contract validation failed with {}",
            self.code
        )
    }
}

impl Error for RuntimeContractError {}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InputAction {
    Tap {
        x: i32,
        y: i32,
    },
    LongTap {
        x: i32,
        y: i32,
        duration_ms: u64,
    },
    Swipe {
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        duration_ms: u64,
    },
    Key {
        key: String,
    },
    Text {
        text: String,
    },
    Reset,
}

impl InputAction {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        match self {
            Self::Tap { x, y } => validate_point(*x, *y),
            Self::LongTap { x, y, duration_ms } => {
                validate_point(*x, *y)?;
                validate_duration(*duration_ms)
            }
            Self::Swipe {
                x1,
                y1,
                x2,
                y2,
                duration_ms,
            } => {
                validate_point(*x1, *y1)?;
                validate_point(*x2, *y2)?;
                validate_duration(*duration_ms)
            }
            Self::Key { key } => validate_bounded_text(key, MAX_INPUT_KEY_BYTES, "invalid_key"),
            Self::Text { text } => {
                validate_bounded_text(text, MAX_INPUT_TEXT_BYTES, "invalid_input_text")
            }
            Self::Reset => Ok(()),
        }
    }

    pub const fn effect(&self) -> EffectDisposition {
        EffectDisposition::Performed
    }

    pub const fn event_action(&self) -> crate::EventAction {
        match self {
            Self::Tap { .. } => crate::EventAction::InputTap,
            Self::LongTap { .. } => crate::EventAction::InputLongTap,
            Self::Swipe { .. } => crate::EventAction::InputSwipe,
            Self::Key { .. } => crate::EventAction::InputKey,
            Self::Text { .. } => crate::EventAction::InputText,
            Self::Reset => crate::EventAction::InputReset,
        }
    }
}

impl fmt::Debug for InputAction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Tap { .. } => "InputAction::Tap(<redacted-coordinates>)",
            Self::LongTap { .. } => "InputAction::LongTap(<redacted-coordinates>)",
            Self::Swipe { .. } => "InputAction::Swipe(<redacted-coordinates>)",
            Self::Key { .. } => "InputAction::Key(<redacted-key>)",
            Self::Text { .. } => "InputAction::Text(<redacted-text>)",
            Self::Reset => "InputAction::Reset",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseToken {
    owner_epoch: OwnerEpoch,
    lease_id: LeaseId,
    instance_id: InstanceId,
    holder_id: HolderId,
    expires_at_monotonic_ms: u64,
}

impl LeaseToken {
    pub fn new(
        owner_epoch: OwnerEpoch,
        lease_id: LeaseId,
        instance_id: InstanceId,
        holder_id: HolderId,
        expires_at_monotonic_ms: u64,
    ) -> RuntimeContractResult<Self> {
        let token = Self {
            owner_epoch,
            lease_id,
            instance_id,
            holder_id,
            expires_at_monotonic_ms,
        };
        token.validate()?;
        Ok(token)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.expires_at_monotonic_ms == 0 {
            return Err(RuntimeContractError::new("invalid_lease_expiry"));
        }
        Ok(())
    }

    pub const fn owner_epoch(&self) -> OwnerEpoch {
        self.owner_epoch
    }

    pub const fn lease_id(&self) -> LeaseId {
        self.lease_id
    }

    pub const fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    pub const fn holder_id(&self) -> HolderId {
        self.holder_id
    }

    pub const fn expires_at_monotonic_ms(&self) -> u64 {
        self.expires_at_monotonic_ms
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeasePriority {
    Normal,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseQueuePolicy {
    priority: LeasePriority,
    timeout_ms: u64,
}

impl LeaseQueuePolicy {
    pub fn new(priority: LeasePriority, timeout_ms: u64) -> RuntimeContractResult<Self> {
        let policy = Self {
            priority,
            timeout_ms,
        };
        policy.validate()?;
        Ok(policy)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.timeout_ms == 0 || self.timeout_ms > MAX_LEASE_QUEUE_TIMEOUT_MS {
            return Err(RuntimeContractError::new("invalid_lease_queue_timeout"));
        }
        Ok(())
    }

    pub const fn priority(self) -> LeasePriority {
        self.priority
    }

    pub const fn timeout_ms(self) -> u64 {
        self.timeout_ms
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseQueueStatus {
    request_id: RequestId,
    instance_id: InstanceId,
    priority: LeasePriority,
    position: u32,
    deadline_monotonic_ms: u64,
    preempt_requested: bool,
}

impl LeaseQueueStatus {
    pub fn new(
        request_id: RequestId,
        instance_id: InstanceId,
        priority: LeasePriority,
        position: u32,
        deadline_monotonic_ms: u64,
        preempt_requested: bool,
    ) -> RuntimeContractResult<Self> {
        let status = Self {
            request_id,
            instance_id,
            priority,
            position,
            deadline_monotonic_ms,
            preempt_requested,
        };
        status.validate()?;
        Ok(status)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.position == 0 || self.deadline_monotonic_ms == 0 {
            return Err(RuntimeContractError::new("invalid_lease_queue_status"));
        }
        Ok(())
    }

    pub const fn request_id(&self) -> RequestId {
        self.request_id
    }

    pub const fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    pub const fn priority(&self) -> LeasePriority {
        self.priority
    }

    pub const fn position(&self) -> u32 {
        self.position
    }

    pub const fn deadline_monotonic_ms(&self) -> u64 {
        self.deadline_monotonic_ms
    }

    pub const fn preempt_requested(&self) -> bool {
        self.preempt_requested
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeCaptureBackend {
    AdbScreencap,
    AdbScreencapEncode,
    AdbScreencapRawGzip,
    DroidcastRaw,
    NemuIpc,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadonlyObservation {
    width: u32,
    height: u32,
    verdict: RecognitionVerdict,
    capture_backend: RuntimeCaptureBackend,
    artifact: ProjectedArtifactReference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadonlyFrame {
    width: u32,
    height: u32,
}

impl ReadonlyFrame {
    pub fn new(width: u32, height: u32) -> RuntimeContractResult<Self> {
        let frame = Self { width, height };
        frame.validate()?;
        Ok(frame)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.width == 0 || self.height == 0 {
            return Err(RuntimeContractError::new("invalid_frame_dimensions"));
        }
        Ok(())
    }

    pub const fn width(&self) -> u32 {
        self.width
    }

    pub const fn height(&self) -> u32 {
        self.height
    }
}

impl ReadonlyObservation {
    pub fn new(
        width: u32,
        height: u32,
        verdict: RecognitionVerdict,
        capture_backend: RuntimeCaptureBackend,
        artifact: ProjectedArtifactReference,
    ) -> RuntimeContractResult<Self> {
        let observation = Self {
            width,
            height,
            verdict,
            capture_backend,
            artifact,
        };
        observation.validate()?;
        Ok(observation)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.width == 0 || self.height == 0 {
            return Err(RuntimeContractError::new("invalid_observation_dimensions"));
        }
        if self.artifact.validate().is_err()
            || self.artifact.object_key().is_none()
            || self.artifact.kind() != ArtifactKind::CaptureFrame
            || self.artifact.media_type() != ArtifactMediaType::ImagePng
            || self.artifact.frame_id().is_none()
            || self.artifact.redaction_state() == ArtifactRedactionState::Pending
            || self.artifact.byte_count() > MAX_READONLY_OBSERVATION_ARTIFACT_BYTES
        {
            return Err(RuntimeContractError::new("invalid_observation_artifact"));
        }
        Ok(())
    }

    pub const fn width(&self) -> u32 {
        self.width
    }

    pub const fn height(&self) -> u32 {
        self.height
    }

    pub const fn verdict(&self) -> RecognitionVerdict {
        self.verdict
    }

    pub const fn capture_backend(&self) -> RuntimeCaptureBackend {
        self.capture_backend
    }

    pub const fn artifact(&self) -> &ProjectedArtifactReference {
        &self.artifact
    }
}

impl fmt::Debug for ReadonlyObservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReadonlyObservation")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("verdict", &self.verdict)
            .field("capture_backend", &self.capture_backend)
            .field("artifact", &"<redacted-artifact-reference>")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeInstanceStatus {
    instance_alias: String,
    instance_id: InstanceId,
    lease_active: bool,
    queued_request_count: u32,
    takeover_cooldown_active: bool,
    destructive_step_active: bool,
    preempt_requested: bool,
}

impl RuntimeInstanceStatus {
    pub fn new(
        instance_alias: impl Into<String>,
        instance_id: InstanceId,
        lease_active: bool,
        queued_request_count: u32,
        takeover_cooldown_active: bool,
        destructive_step_active: bool,
        preempt_requested: bool,
    ) -> RuntimeContractResult<Self> {
        let status = Self {
            instance_alias: instance_alias.into(),
            instance_id,
            lease_active,
            queued_request_count,
            takeover_cooldown_active,
            destructive_step_active,
            preempt_requested,
        };
        status.validate()?;
        Ok(status)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        validate_instance_alias(&self.instance_alias)?;
        if (self.destructive_step_active || self.preempt_requested) && !self.lease_active {
            return Err(RuntimeContractError::new("invalid_runtime_instance_status"));
        }
        if self.lease_active && self.takeover_cooldown_active {
            return Err(RuntimeContractError::new("invalid_runtime_instance_status"));
        }
        Ok(())
    }

    pub fn instance_alias(&self) -> &str {
        &self.instance_alias
    }

    pub const fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    pub const fn lease_active(&self) -> bool {
        self.lease_active
    }

    pub const fn queued_request_count(&self) -> u32 {
        self.queued_request_count
    }

    pub const fn takeover_cooldown_active(&self) -> bool {
        self.takeover_cooldown_active
    }

    pub const fn destructive_step_active(&self) -> bool {
        self.destructive_step_active
    }

    pub const fn preempt_requested(&self) -> bool {
        self.preempt_requested
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeControlPlaneStatus {
    owner_epoch: OwnerEpoch,
    instances: Vec<RuntimeInstanceStatus>,
}

impl RuntimeControlPlaneStatus {
    pub fn new(
        owner_epoch: OwnerEpoch,
        mut instances: Vec<RuntimeInstanceStatus>,
    ) -> RuntimeContractResult<Self> {
        instances.sort_by(|left, right| left.instance_alias.cmp(&right.instance_alias));
        let status = Self {
            owner_epoch,
            instances,
        };
        status.validate()?;
        Ok(status)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        let mut aliases = BTreeSet::new();
        let mut instance_ids = BTreeSet::new();
        let mut previous_alias = None;
        for instance in &self.instances {
            instance.validate()?;
            if !aliases.insert(instance.instance_alias.as_str())
                || !instance_ids.insert(instance.instance_id)
                || previous_alias.is_some_and(|previous| previous >= instance.instance_alias())
            {
                return Err(RuntimeContractError::new("invalid_runtime_status_registry"));
            }
            previous_alias = Some(instance.instance_alias());
        }
        Ok(())
    }

    pub const fn owner_epoch(&self) -> OwnerEpoch {
        self.owner_epoch
    }

    pub fn instances(&self) -> &[RuntimeInstanceStatus] {
        &self.instances
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadonlyObservationStage {
    Capture,
    Recognition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReadonlyObservationOutcome {
    Completed {
        observation: ReadonlyObservation,
    },
    Failed {
        stage: ReadonlyObservationStage,
        #[serde(skip_serializing_if = "Option::is_none")]
        captured_frame: Option<ReadonlyFrame>,
    },
}

impl ReadonlyObservationOutcome {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        match self {
            Self::Completed { observation } => observation.validate(),
            Self::Failed {
                stage: ReadonlyObservationStage::Capture,
                captured_frame: None,
            } => Ok(()),
            Self::Failed {
                stage: ReadonlyObservationStage::Recognition,
                captured_frame: Some(frame),
            } => frame.validate(),
            Self::Failed { .. } => Err(RuntimeContractError::new(
                "invalid_observation_failure_context",
            )),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeOperation {
    Health,
    Status,
    AcquireLease {
        instance_alias: String,
        holder_id: HolderId,
    },
    QueueLease {
        instance_alias: String,
        holder_id: HolderId,
        policy: LeaseQueuePolicy,
    },
    PollQueuedLease {
        queued_request_id: RequestId,
    },
    CancelQueuedLease {
        queued_request_id: RequestId,
    },
    RenewLease {
        token: LeaseToken,
    },
    ReleaseLease {
        token: LeaseToken,
    },
    ObserveReadonly {
        instance_alias: String,
    },
    SafeReset {
        instance_alias: String,
        holder_id: HolderId,
    },
    Input {
        token: LeaseToken,
        action: InputAction,
    },
    QueryEvents {
        query: EventQuery,
        profile: ProjectionProfile,
    },
}

impl RuntimeOperation {
    pub fn acquire_lease(instance_alias: impl Into<String>, holder_id: IssuedHolderId) -> Self {
        Self::AcquireLease {
            instance_alias: instance_alias.into(),
            holder_id: *holder_id.transport(),
        }
    }

    pub fn queue_lease(
        instance_alias: impl Into<String>,
        holder_id: IssuedHolderId,
        policy: LeaseQueuePolicy,
    ) -> Self {
        Self::QueueLease {
            instance_alias: instance_alias.into(),
            holder_id: *holder_id.transport(),
            policy,
        }
    }

    pub fn safe_reset(instance_alias: impl Into<String>, holder_id: IssuedHolderId) -> Self {
        Self::SafeReset {
            instance_alias: instance_alias.into(),
            holder_id: *holder_id.transport(),
        }
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        match self {
            Self::Health
            | Self::Status
            | Self::PollQueuedLease { .. }
            | Self::CancelQueuedLease { .. }
            | Self::QueryEvents { .. } => Ok(()),
            Self::AcquireLease { instance_alias, .. }
            | Self::ObserveReadonly { instance_alias }
            | Self::SafeReset { instance_alias, .. } => validate_instance_alias(instance_alias),
            Self::QueueLease {
                instance_alias,
                policy,
                ..
            } => {
                validate_instance_alias(instance_alias)?;
                policy.validate()
            }
            Self::RenewLease { token } | Self::ReleaseLease { token } => token.validate(),
            Self::Input { token, action } => {
                token.validate()?;
                action.validate()
            }
        }
    }

    pub fn instance_alias(&self) -> Option<&str> {
        match self {
            Self::AcquireLease { instance_alias, .. }
            | Self::QueueLease { instance_alias, .. }
            | Self::ObserveReadonly { instance_alias }
            | Self::SafeReset { instance_alias, .. } => Some(instance_alias),
            _ => None,
        }
    }

    pub const fn lease_token(&self) -> Option<&LeaseToken> {
        match self {
            Self::RenewLease { token }
            | Self::ReleaseLease { token }
            | Self::Input { token, .. } => Some(token),
            _ => None,
        }
    }
}

impl fmt::Debug for RuntimeOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Health => "RuntimeOperation::Health",
            Self::Status => "RuntimeOperation::Status",
            Self::AcquireLease { .. } => "RuntimeOperation::AcquireLease(<redacted>)",
            Self::QueueLease { .. } => "RuntimeOperation::QueueLease(<redacted>)",
            Self::PollQueuedLease { .. } => "RuntimeOperation::PollQueuedLease(<opaque-request>)",
            Self::CancelQueuedLease { .. } => {
                "RuntimeOperation::CancelQueuedLease(<opaque-request>)"
            }
            Self::RenewLease { .. } => "RuntimeOperation::RenewLease(<opaque-token>)",
            Self::ReleaseLease { .. } => "RuntimeOperation::ReleaseLease(<opaque-token>)",
            Self::ObserveReadonly { .. } => "RuntimeOperation::ObserveReadonly(<redacted>)",
            Self::SafeReset { .. } => "RuntimeOperation::SafeReset(<redacted>)",
            Self::Input { .. } => "RuntimeOperation::Input(<redacted>)",
            Self::QueryEvents { .. } => "RuntimeOperation::QueryEvents(<typed-query>)",
        })
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeRequest {
    schema_version: String,
    request_id: RequestId,
    correlation_id: CorrelationId,
    #[serde(skip_serializing_if = "Option::is_none")]
    causation_id: Option<CausationId>,
    actor: EventActor,
    source: EventSource,
    submitted_at_unix_ms: u64,
    operation: RuntimeOperation,
}

impl RuntimeRequest {
    pub fn new(
        request_id: IssuedRequestId,
        correlation_id: IssuedCorrelationId,
        causation_id: Option<IssuedCausationId>,
        actor: EventActor,
        source: EventSource,
        submitted_at_unix_ms: u64,
        operation: RuntimeOperation,
    ) -> RuntimeContractResult<Self> {
        let request = Self {
            schema_version: RUNTIME_REQUEST_SCHEMA_VERSION.to_string(),
            request_id: *request_id.transport(),
            correlation_id: *correlation_id.transport(),
            causation_id: causation_id.map(|value| *value.transport()),
            actor,
            source,
            submitted_at_unix_ms,
            operation,
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> RuntimeContractResult<ValidatedRuntimeRequest<'_>> {
        if self.schema_version != RUNTIME_REQUEST_SCHEMA_VERSION {
            return Err(RuntimeContractError::new("unsupported_request_schema"));
        }
        if self.submitted_at_unix_ms == 0 {
            return Err(RuntimeContractError::new("invalid_request_timestamp"));
        }
        if !valid_client_origin(self.actor, self.source) {
            return Err(RuntimeContractError::new("invalid_client_origin"));
        }
        self.operation.validate()?;
        Ok(ValidatedRuntimeRequest { request: self })
    }

    pub const fn request_id(&self) -> RequestId {
        self.request_id
    }

    pub const fn correlation_id(&self) -> CorrelationId {
        self.correlation_id
    }

    pub const fn actor(&self) -> EventActor {
        self.actor
    }

    pub const fn source(&self) -> EventSource {
        self.source
    }

    pub const fn submitted_at_unix_ms(&self) -> u64 {
        self.submitted_at_unix_ms
    }

    pub const fn operation(&self) -> &RuntimeOperation {
        &self.operation
    }
}

impl fmt::Debug for RuntimeRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeRequest")
            .field("schema_version", &self.schema_version)
            .field("request_id", &self.request_id)
            .field("correlation_id", &self.correlation_id)
            .field("actor", &self.actor)
            .field("source", &self.source)
            .field("submitted_at_unix_ms", &self.submitted_at_unix_ms)
            .field("operation", &self.operation)
            .finish()
    }
}

#[derive(Debug)]
pub struct ValidatedRuntimeRequest<'a> {
    request: &'a RuntimeRequest,
}

impl ValidatedRuntimeRequest<'_> {
    pub fn event_links(
        &self,
        instance_id: Option<InstanceId>,
        lease_id: Option<LeaseId>,
        action_id: Option<ActionId>,
    ) -> EventLinksDraft {
        EventLinksDraft::from_verified_runtime(
            instance_id,
            self.request.request_id,
            self.request.correlation_id,
            self.request.causation_id,
            lease_id,
            action_id,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeReceiptState {
    Admitted,
    Queued,
    Denied,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalEvent {
    pub sequence: u64,
    pub event_id: EventId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeErrorCode {
    InvalidRequest,
    RuntimeUnavailable,
    RuntimeFatal,
    OwnerConflict,
    ProtocolInvalid,
    InstanceUnknown,
    LeaseBusy,
    LeaseCooldown,
    LeaseExpired,
    LeaseMissing,
    StaleOwnerEpoch,
    LeaseMismatch,
    QueueFull,
    QueueExpired,
    QueueMissing,
    QueueConnectionMismatch,
    TransferNotSafe,
    InstanceMismatch,
    HolderMismatch,
    ConnectionMismatch,
    ReadonlyCapabilityInvalid,
    CaptureFailed,
    RecognitionFailed,
    BackendOpenFailed,
    BackendOperationFailed,
    LedgerFailure,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeErrorProjection {
    pub code: RuntimeErrorCode,
    pub fatal: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_holder_id: Option<HolderId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_lease_id: Option<LeaseId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}

impl RuntimeErrorProjection {
    pub const fn new(code: RuntimeErrorCode, fatal: bool) -> Self {
        Self {
            code,
            fatal,
            current_holder_id: None,
            current_lease_id: None,
            retry_after_ms: None,
        }
    }

    pub const fn with_holder(mut self, holder_id: HolderId, lease_id: LeaseId) -> Self {
        self.current_holder_id = Some(holder_id);
        self.current_lease_id = Some(lease_id);
        self
    }

    pub const fn with_retry_after(mut self, retry_after_ms: u64) -> Self {
        self.retry_after_ms = Some(retry_after_ms);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadOnlyCaptureCapability {
    owner_epoch: OwnerEpoch,
    instance_id: InstanceId,
    frame_id: FrameId,
    recognition_id: RecognitionId,
}

impl ReadOnlyCaptureCapability {
    pub const fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    pub const fn recognition_id(&self) -> RecognitionId {
        self.recognition_id
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IssuedReadOnlyCaptureCapability {
    transport: ReadOnlyCaptureCapability,
    frame_id: IssuedFrameId,
    recognition_id: IssuedRecognitionId,
}

impl IssuedReadOnlyCaptureCapability {
    pub const fn transport(&self) -> &ReadOnlyCaptureCapability {
        &self.transport
    }

    pub fn event_links(&self, request: &ValidatedRuntimeRequest<'_>) -> EventLinksDraft {
        request
            .event_links(Some(self.transport.instance_id), None, None)
            .with_frame_id(self.frame_id)
            .with_recognition_id(self.recognition_id)
    }

    pub fn artifact_links(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
    ) -> crate::ArtifactLinksDraft {
        crate::ArtifactLinksDraft::default()
            .with_frame_id(self.frame_id)
            .with_correlation_id(IssuedCorrelationId::from_verified_transport(
                request.request.correlation_id,
            ))
    }
}

impl IdentifierIssuer {
    pub fn issue_readonly_capture_capability(
        &self,
        owner_epoch: OwnerEpoch,
        instance_id: InstanceId,
    ) -> Result<IssuedReadOnlyCaptureCapability, IdentifierIssuanceError> {
        let frame_id = self.mint_frame_id()?;
        let recognition_id = self.mint_recognition_id()?;
        Ok(IssuedReadOnlyCaptureCapability {
            transport: ReadOnlyCaptureCapability {
                owner_epoch,
                instance_id,
                frame_id: *frame_id.transport(),
                recognition_id: *recognition_id.transport(),
            },
            frame_id,
            recognition_id,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeResult {
    Health {
        owner_epoch: OwnerEpoch,
    },
    Status {
        status: RuntimeControlPlaneStatus,
    },
    LeaseGranted {
        token: LeaseToken,
    },
    LeaseRenewed {
        token: LeaseToken,
    },
    LeaseReleased {
        instance_id: InstanceId,
        lease_id: LeaseId,
    },
    LeaseQueued {
        status: LeaseQueueStatus,
    },
    LeasePending {
        status: LeaseQueueStatus,
    },
    LeaseQueueCancelled {
        request_id: RequestId,
        instance_id: InstanceId,
    },
    ReadonlyObservationCompleted {
        observation: ReadonlyObservation,
    },
    SafeResetCompleted {
        action_id: ActionId,
    },
    InputCommitted {
        action_id: ActionId,
    },
    Events {
        events: Vec<ProjectedEvent>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeReceipt {
    schema_version: String,
    request_id: RequestId,
    correlation_id: CorrelationId,
    state: RuntimeReceiptState,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal: Option<TerminalEvent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<RuntimeResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RuntimeErrorProjection>,
}

impl RuntimeReceipt {
    pub fn success(
        request: &RuntimeRequest,
        state: RuntimeReceiptState,
        terminal: Option<TerminalEvent>,
        result: RuntimeResult,
    ) -> RuntimeContractResult<Self> {
        let receipt = Self {
            schema_version: RUNTIME_RECEIPT_SCHEMA_VERSION.to_string(),
            request_id: request.request_id,
            correlation_id: request.correlation_id,
            state,
            terminal,
            result: Some(result),
            error: None,
        };
        receipt.validate()?;
        Ok(receipt)
    }

    pub fn error(
        request: &RuntimeRequest,
        state: RuntimeReceiptState,
        terminal: Option<TerminalEvent>,
        error: RuntimeErrorProjection,
    ) -> RuntimeContractResult<Self> {
        let receipt = Self {
            schema_version: RUNTIME_RECEIPT_SCHEMA_VERSION.to_string(),
            request_id: request.request_id,
            correlation_id: request.correlation_id,
            state,
            terminal,
            result: None,
            error: Some(error),
        };
        receipt.validate()?;
        Ok(receipt)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.schema_version != RUNTIME_RECEIPT_SCHEMA_VERSION {
            return Err(RuntimeContractError::new("unsupported_receipt_schema"));
        }
        let success_state = matches!(
            self.state,
            RuntimeReceiptState::Admitted
                | RuntimeReceiptState::Queued
                | RuntimeReceiptState::Completed
                | RuntimeReceiptState::Cancelled
        );
        if success_state != (self.result.is_some() && self.error.is_none()) {
            return Err(RuntimeContractError::new("invalid_receipt_outcome"));
        }
        if !success_state && (self.error.is_none() || self.result.is_some()) {
            return Err(RuntimeContractError::new("invalid_receipt_outcome"));
        }
        if self.terminal.is_some_and(|terminal| terminal.sequence == 0) {
            return Err(RuntimeContractError::new("invalid_terminal_event"));
        }
        if let Some(RuntimeResult::LeaseGranted { token } | RuntimeResult::LeaseRenewed { token }) =
            &self.result
        {
            token.validate()?;
        }
        match &self.result {
            Some(RuntimeResult::Status { status }) => status.validate()?,
            Some(
                RuntimeResult::LeaseQueued { status } | RuntimeResult::LeasePending { status },
            ) => status.validate()?,
            Some(RuntimeResult::ReadonlyObservationCompleted { observation }) => {
                observation.validate()?
            }
            _ => {}
        }
        Ok(())
    }

    pub const fn request_id(&self) -> RequestId {
        self.request_id
    }

    pub const fn correlation_id(&self) -> CorrelationId {
        self.correlation_id
    }

    pub const fn state(&self) -> RuntimeReceiptState {
        self.state
    }

    pub const fn terminal(&self) -> Option<TerminalEvent> {
        self.terminal
    }

    pub const fn result(&self) -> Option<&RuntimeResult> {
        self.result.as_ref()
    }

    pub const fn error_projection(&self) -> Option<&RuntimeErrorProjection> {
        self.error.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeInfo {
    schema_version: String,
    pid: u32,
    host: String,
    port: u16,
    owner_epoch: OwnerEpoch,
    started_at_unix_ms: u64,
}

impl RuntimeInfo {
    pub fn new(
        pid: u32,
        host: impl Into<String>,
        port: u16,
        owner_epoch: OwnerEpoch,
        started_at_unix_ms: u64,
    ) -> RuntimeContractResult<Self> {
        let info = Self {
            schema_version: RUNTIME_INFO_SCHEMA_VERSION.to_string(),
            pid,
            host: host.into(),
            port,
            owner_epoch,
            started_at_unix_ms,
        };
        info.validate()?;
        Ok(info)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.schema_version != RUNTIME_INFO_SCHEMA_VERSION {
            return Err(RuntimeContractError::new("unsupported_runtime_info_schema"));
        }
        let host = self
            .host
            .parse::<IpAddr>()
            .map_err(|_| RuntimeContractError::new("invalid_runtime_host"))?;
        if self.pid == 0 || self.port == 0 || self.started_at_unix_ms == 0 || !host.is_loopback() {
            return Err(RuntimeContractError::new("invalid_runtime_info"));
        }
        Ok(())
    }

    pub fn socket_addr(&self) -> RuntimeContractResult<SocketAddr> {
        self.validate()?;
        let host = self
            .host
            .parse::<IpAddr>()
            .map_err(|_| RuntimeContractError::new("invalid_runtime_host"))?;
        Ok(SocketAddr::new(host, self.port))
    }

    pub const fn pid(&self) -> u32 {
        self.pid
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub const fn port(&self) -> u16 {
        self.port
    }

    pub const fn owner_epoch(&self) -> OwnerEpoch {
        self.owner_epoch
    }
}

fn validate_point(x: i32, y: i32) -> RuntimeContractResult<()> {
    if x < 0 || y < 0 {
        return Err(RuntimeContractError::new("invalid_input_coordinate"));
    }
    Ok(())
}

fn validate_duration(duration_ms: u64) -> RuntimeContractResult<()> {
    if !(1..=MAX_INPUT_DURATION_MS).contains(&duration_ms) {
        return Err(RuntimeContractError::new("invalid_input_duration"));
    }
    Ok(())
}

fn validate_instance_alias(value: &str) -> RuntimeContractResult<()> {
    validate_bounded_text(value, MAX_INSTANCE_ALIAS_BYTES, "invalid_instance_alias")
}

fn validate_bounded_text(
    value: &str,
    max_bytes: usize,
    code: &'static str,
) -> RuntimeContractResult<()> {
    if value.is_empty()
        || value.len() > max_bytes
        || value.chars().any(|character| character == '\0')
    {
        return Err(RuntimeContractError::new(code));
    }
    Ok(())
}

fn valid_client_origin(actor: EventActor, source: EventSource) -> bool {
    matches!(
        source,
        EventSource::Cli | EventSource::Ui | EventSource::Lab | EventSource::Adapter
    ) && matches!(
        actor,
        EventActor::User | EventActor::Cli | EventActor::Ui | EventActor::Lab | EventActor::Agent
    )
}

#[cfg(test)]
#[path = "runtime/tests.rs"]
mod tests;
