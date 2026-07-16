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
    ActionId, ApprovalDecisionRecord, ApprovalDisposition, ArtifactKind, ArtifactLinksDraft,
    ArtifactMediaType, ArtifactRedactionState, CausationId, ClientActionRecord, CorrelationId,
    EffectDisposition, EventActor, EventId, EventLinksDraft, EventQuery, EventSource, EventType,
    EvidenceCompleteness, FrameId, HolderId, IdentifierIssuanceError, IdentifierIssuer, InstanceId,
    IssuedCausationId, IssuedCorrelationId, IssuedFrameId, IssuedHolderId, IssuedRecognitionId,
    IssuedRequestId, IssuedRunId, IssuedTaskId, LeaseId, OwnerEpoch, ProjectedArtifactReference,
    ProjectedEvent, ProjectionProfile, RecognitionId, RecognitionVerdict, RequestId,
    ResourceAuthoringPhase, RunId, RuntimeMonitorInstanceStatus, RuntimeMonitorPolicy,
    RuntimeMonitorRegistryStatus, SubscriptionCursor, TaskOutcome,
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
pub const MAX_RUNTIME_CAPTURE_SEQUENCE_FRAMES: u16 = 60;
pub const MAX_RUNTIME_CAPTURE_SEQUENCE_INTERVAL_MS: u64 = 5_000;
pub const MAX_RUNTIME_CAPTURE_SEQUENCE_WAIT_MS: u64 = 60_000;
pub const MAX_DEBUG_PACKAGE_PATH_BYTES: usize = 32 * 1024;
pub const MAX_CONTAINED_TASK_PATH_BYTES: usize = 32 * 1024;
pub const MAX_EVIDENCE_OUTPUT_PATH_BYTES: usize = 32 * 1024;
pub const MAX_RUNTIME_SUBSCRIPTION_WAIT_MS: u64 = 30_000;
pub const MAX_RUNTIME_SUBSCRIPTION_EVENTS: u16 = 256;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplicationLifecycleAction {
    Launch,
    Stop,
    Restart,
}

impl ApplicationLifecycleAction {
    pub const fn event_action(self) -> crate::EventAction {
        match self {
            Self::Launch => crate::EventAction::ApplicationLaunch,
            Self::Stop => crate::EventAction::ApplicationStop,
            Self::Restart => crate::EventAction::ApplicationRestart,
        }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureSequenceSpec {
    frame_count: u16,
    interval_ms: u64,
}

impl CaptureSequenceSpec {
    pub fn new(frame_count: u16, interval_ms: u64) -> RuntimeContractResult<Self> {
        let spec = Self {
            frame_count,
            interval_ms,
        };
        spec.validate()?;
        Ok(spec)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.frame_count == 0
            || self.frame_count > MAX_RUNTIME_CAPTURE_SEQUENCE_FRAMES
            || self.interval_ms > MAX_RUNTIME_CAPTURE_SEQUENCE_INTERVAL_MS
            || self.planned_wait_ms()? > MAX_RUNTIME_CAPTURE_SEQUENCE_WAIT_MS
        {
            return Err(RuntimeContractError::new("invalid_capture_sequence_spec"));
        }
        Ok(())
    }

    pub const fn frame_count(&self) -> u16 {
        self.frame_count
    }

    pub const fn interval_ms(&self) -> u64 {
        self.interval_ms
    }

    pub fn planned_wait_ms(&self) -> RuntimeContractResult<u64> {
        u64::from(self.frame_count.saturating_sub(1))
            .checked_mul(self.interval_ms)
            .ok_or_else(|| RuntimeContractError::new("capture_sequence_wait_overflow"))
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureSequence {
    spec: CaptureSequenceSpec,
    observations: Vec<ReadonlyObservation>,
}

impl CaptureSequence {
    pub fn new(
        spec: CaptureSequenceSpec,
        observations: Vec<ReadonlyObservation>,
    ) -> RuntimeContractResult<Self> {
        let sequence = Self { spec, observations };
        sequence.validate()?;
        Ok(sequence)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        self.spec.validate()?;
        if self.observations.len() != usize::from(self.spec.frame_count()) {
            return Err(RuntimeContractError::new(
                "invalid_capture_sequence_observation_count",
            ));
        }
        let mut artifact_ids = BTreeSet::new();
        let mut frame_ids = BTreeSet::new();
        for observation in &self.observations {
            observation.validate()?;
            let artifact = observation.artifact();
            let frame_id = artifact.frame_id().ok_or_else(|| {
                RuntimeContractError::new("invalid_capture_sequence_artifact_identity")
            })?;
            if !artifact_ids.insert(artifact.artifact_id) || !frame_ids.insert(*frame_id) {
                return Err(RuntimeContractError::new(
                    "duplicate_capture_sequence_artifact_identity",
                ));
            }
        }
        Ok(())
    }

    pub const fn spec(&self) -> CaptureSequenceSpec {
        self.spec
    }

    pub fn observations(&self) -> &[ReadonlyObservation] {
        &self.observations
    }
}

impl fmt::Debug for CaptureSequence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CaptureSequence")
            .field("spec", &self.spec)
            .field("observation_count", &self.observations.len())
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

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceAuthoringEvent {
    phase: ResourceAuthoringPhase,
    draft_id: String,
    target_label: String,
    target_fingerprint: String,
    changed_paths: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_code: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeDebugOperation {
    LabRun,
    Observe,
    Do,
    Ensure,
    Wait,
}

impl RuntimeDebugOperation {
    pub const fn event_action(self) -> crate::EventAction {
        match self {
            Self::LabRun => crate::EventAction::RuntimeDebugLabRun,
            Self::Observe => crate::EventAction::RuntimeDebugObserve,
            Self::Do => crate::EventAction::RuntimeDebugDo,
            Self::Ensure => crate::EventAction::RuntimeDebugEnsure,
            Self::Wait => crate::EventAction::RuntimeDebugWait,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeDebugPhase {
    Requested,
    Progress,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeDebugEvent {
    operation: RuntimeDebugOperation,
    phase: RuntimeDebugPhase,
    effect_disposition: EffectDisposition,
}

impl RuntimeDebugEvent {
    pub fn requested(operation: RuntimeDebugOperation) -> Self {
        Self {
            operation,
            phase: RuntimeDebugPhase::Requested,
            effect_disposition: EffectDisposition::NotPerformed,
        }
    }

    pub fn progress(operation: RuntimeDebugOperation) -> Self {
        Self {
            operation,
            phase: RuntimeDebugPhase::Progress,
            effect_disposition: EffectDisposition::NotPerformed,
        }
    }

    pub fn completed(
        operation: RuntimeDebugOperation,
        effect_disposition: EffectDisposition,
    ) -> Self {
        Self {
            operation,
            phase: RuntimeDebugPhase::Completed,
            effect_disposition,
        }
    }

    pub fn failed(operation: RuntimeDebugOperation, effect_disposition: EffectDisposition) -> Self {
        Self {
            operation,
            phase: RuntimeDebugPhase::Failed,
            effect_disposition,
        }
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if matches!(
            self.phase,
            RuntimeDebugPhase::Requested | RuntimeDebugPhase::Progress
        ) && self.effect_disposition != EffectDisposition::NotPerformed
        {
            return Err(RuntimeContractError::new("invalid_runtime_debug_event"));
        }
        if self.phase == RuntimeDebugPhase::Progress
            && self.operation != RuntimeDebugOperation::LabRun
        {
            return Err(RuntimeContractError::new("invalid_runtime_debug_event"));
        }
        Ok(())
    }

    pub const fn operation(&self) -> RuntimeDebugOperation {
        self.operation
    }

    pub const fn phase(&self) -> RuntimeDebugPhase {
        self.phase
    }

    pub const fn effect_disposition(&self) -> EffectDisposition {
        self.effect_disposition
    }
}

impl ResourceAuthoringEvent {
    pub fn new(
        phase: ResourceAuthoringPhase,
        draft_id: impl Into<String>,
        target_label: impl Into<String>,
        target_fingerprint: impl Into<String>,
        changed_paths: Vec<String>,
        failure_code: Option<String>,
    ) -> RuntimeContractResult<Self> {
        let event = Self {
            phase,
            draft_id: draft_id.into(),
            target_label: target_label.into(),
            target_fingerprint: target_fingerprint.into(),
            changed_paths,
            failure_code,
        };
        event.validate()?;
        Ok(event)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        crate::validate_resource_authoring_fields(
            self.phase,
            &self.draft_id,
            &self.target_label,
            &self.target_fingerprint,
            &self.changed_paths,
            self.failure_code.as_deref(),
        )
        .map_err(|_| RuntimeContractError::new("invalid_resource_authoring_event"))
    }

    pub const fn phase(&self) -> ResourceAuthoringPhase {
        self.phase
    }

    pub fn draft_id(&self) -> &str {
        &self.draft_id
    }

    pub fn target_label(&self) -> &str {
        &self.target_label
    }

    pub fn target_fingerprint(&self) -> &str {
        &self.target_fingerprint
    }

    pub fn changed_paths(&self) -> &[String] {
        &self.changed_paths
    }

    pub fn failure_code(&self) -> Option<&str> {
        self.failure_code.as_deref()
    }
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
#[serde(deny_unknown_fields)]
pub struct PackageDebugRequest {
    package_path: String,
    expected_sha256: String,
}

impl PackageDebugRequest {
    pub fn new(
        package_path: impl Into<String>,
        expected_sha256: impl Into<String>,
    ) -> RuntimeContractResult<Self> {
        let request = Self {
            package_path: package_path.into(),
            expected_sha256: expected_sha256.into(),
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.package_path.trim().is_empty()
            || self.package_path.len() > MAX_DEBUG_PACKAGE_PATH_BYTES
            || self.package_path.contains('\0')
        {
            return Err(RuntimeContractError::new("invalid_debug_package_path"));
        }
        validate_sha256_hex(&self.expected_sha256)
            .map_err(|_| RuntimeContractError::new("invalid_debug_package_hash"))
    }

    pub fn package_path(&self) -> &str {
        &self.package_path
    }

    pub fn expected_sha256(&self) -> &str {
        &self.expected_sha256
    }
}

impl fmt::Debug for PackageDebugRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PackageDebugRequest")
            .field("package_path", &"<redacted-path>")
            .field("expected_sha256", &"<redacted-hash>")
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContainedTaskRequest {
    package_path: String,
    expected_sha256: String,
}

impl ContainedTaskRequest {
    pub fn new(
        package_path: impl Into<String>,
        expected_sha256: impl Into<String>,
    ) -> RuntimeContractResult<Self> {
        let request = Self {
            package_path: package_path.into(),
            expected_sha256: expected_sha256.into(),
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.package_path.trim().is_empty()
            || self.package_path.len() > MAX_CONTAINED_TASK_PATH_BYTES
            || self.package_path.contains('\0')
        {
            return Err(RuntimeContractError::new("invalid_contained_task_path"));
        }
        validate_sha256_hex(&self.expected_sha256)
            .map_err(|_| RuntimeContractError::new("invalid_contained_task_hash"))
    }

    pub fn package_path(&self) -> &str {
        &self.package_path
    }

    pub fn expected_sha256(&self) -> &str {
        &self.expected_sha256
    }
}

impl fmt::Debug for ContainedTaskRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ContainedTaskRequest")
            .field("package_path", &"<redacted-path>")
            .field("expected_sha256", &"<redacted-hash>")
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeEvidenceExportRequest {
    output_path: String,
    task_outcome: TaskOutcome,
}

impl RuntimeEvidenceExportRequest {
    pub fn new(
        output_path: impl Into<String>,
        task_outcome: TaskOutcome,
    ) -> RuntimeContractResult<Self> {
        let request = Self {
            output_path: output_path.into(),
            task_outcome,
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.output_path.trim().is_empty()
            || self.output_path.len() > MAX_EVIDENCE_OUTPUT_PATH_BYTES
            || self.output_path.contains('\0')
        {
            return Err(RuntimeContractError::new("invalid_evidence_output_path"));
        }
        Ok(())
    }

    pub fn output_path(&self) -> &str {
        &self.output_path
    }

    pub const fn task_outcome(&self) -> TaskOutcome {
        self.task_outcome
    }
}

impl fmt::Debug for RuntimeEvidenceExportRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeEvidenceExportRequest")
            .field("output_path", &"<redacted-path>")
            .field("task_outcome", &self.task_outcome)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeEvidenceScreenshotCounts {
    pub captured: u64,
    pub deduplicated: u64,
    pub dropped: u64,
    pub persisted: u64,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeEvidenceExportSummary {
    correlation_id: CorrelationId,
    run_id: RunId,
    task_outcome: TaskOutcome,
    evidence_completeness: EvidenceCompleteness,
    normalized_output_path: String,
    zip_byte_count: u64,
    zip_sha256: String,
    manifest_sha256: String,
    archive: ProjectedArtifactReference,
    screenshot_counts: RuntimeEvidenceScreenshotCounts,
    terminal_receipt: ProjectedEvent,
}

impl RuntimeEvidenceExportSummary {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        correlation_id: CorrelationId,
        run_id: RunId,
        task_outcome: TaskOutcome,
        evidence_completeness: EvidenceCompleteness,
        normalized_output_path: impl Into<String>,
        zip_byte_count: u64,
        zip_sha256: impl Into<String>,
        manifest_sha256: impl Into<String>,
        archive: ProjectedArtifactReference,
        screenshot_counts: RuntimeEvidenceScreenshotCounts,
        terminal_receipt: ProjectedEvent,
    ) -> RuntimeContractResult<Self> {
        let summary = Self {
            correlation_id,
            run_id,
            task_outcome,
            evidence_completeness,
            normalized_output_path: normalized_output_path.into(),
            zip_byte_count,
            zip_sha256: zip_sha256.into(),
            manifest_sha256: manifest_sha256.into(),
            archive,
            screenshot_counts,
            terminal_receipt,
        };
        summary.validate()?;
        Ok(summary)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.normalized_output_path.trim().is_empty()
            || self.normalized_output_path.len() > MAX_EVIDENCE_OUTPUT_PATH_BYTES
            || self.normalized_output_path.contains('\0')
        {
            return Err(RuntimeContractError::new("invalid_evidence_output_path"));
        }
        if self.zip_byte_count == 0 {
            return Err(RuntimeContractError::new("invalid_evidence_zip_count"));
        }
        let accounted = self
            .screenshot_counts
            .deduplicated
            .checked_add(self.screenshot_counts.dropped)
            .and_then(|count| count.checked_add(self.screenshot_counts.persisted));
        if accounted.is_none_or(|count| count > self.screenshot_counts.captured) {
            return Err(RuntimeContractError::new(
                "invalid_evidence_screenshot_counts",
            ));
        }
        if self.archive.kind() != ArtifactKind::EvidenceArchive
            || self.archive.byte_count() != self.zip_byte_count
            || self.archive.sha256() != self.zip_sha256
            || self.archive.run_id != Some(self.run_id)
            || self.archive.correlation_id != Some(self.correlation_id)
        {
            return Err(RuntimeContractError::new(
                "invalid_evidence_archive_reference",
            ));
        }
        if self.terminal_receipt.sequence == 0
            || self.terminal_receipt.links.run_id() != Some(&self.run_id)
            || self.terminal_receipt.links.correlation_id() != Some(&self.correlation_id)
            || self.terminal_receipt.event_type != terminal_event_type(self.task_outcome)
        {
            return Err(RuntimeContractError::new(
                "invalid_evidence_terminal_receipt",
            ));
        }
        validate_canonical_sha256(&self.zip_sha256)
            .map_err(|_| RuntimeContractError::new("invalid_evidence_zip_hash"))?;
        validate_canonical_sha256(&self.manifest_sha256)
            .map_err(|_| RuntimeContractError::new("invalid_evidence_manifest_hash"))?;
        self.archive
            .validate()
            .map_err(|_| RuntimeContractError::new("invalid_evidence_archive_reference"))
    }

    pub const fn correlation_id(&self) -> CorrelationId {
        self.correlation_id
    }

    pub const fn run_id(&self) -> RunId {
        self.run_id
    }

    pub const fn task_outcome(&self) -> TaskOutcome {
        self.task_outcome
    }

    pub const fn evidence_completeness(&self) -> EvidenceCompleteness {
        self.evidence_completeness
    }

    pub fn normalized_output_path(&self) -> &str {
        &self.normalized_output_path
    }

    pub const fn zip_byte_count(&self) -> u64 {
        self.zip_byte_count
    }

    pub fn zip_sha256(&self) -> &str {
        &self.zip_sha256
    }

    pub fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }

    pub const fn archive(&self) -> &ProjectedArtifactReference {
        &self.archive
    }

    pub const fn screenshot_counts(&self) -> RuntimeEvidenceScreenshotCounts {
        self.screenshot_counts
    }

    pub const fn terminal_receipt(&self) -> &ProjectedEvent {
        &self.terminal_receipt
    }
}

impl fmt::Debug for RuntimeEvidenceExportSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeEvidenceExportSummary")
            .field("correlation_id", &self.correlation_id)
            .field("run_id", &self.run_id)
            .field("task_outcome", &self.task_outcome)
            .field("evidence_completeness", &self.evidence_completeness)
            .field("normalized_output_path", &"<redacted-path>")
            .field("zip_byte_count", &self.zip_byte_count)
            .field("zip_sha256", &"<redacted-hash>")
            .field("manifest_sha256", &"<redacted-hash>")
            .field("archive", &self.archive)
            .field("screenshot_counts", &self.screenshot_counts)
            .field("terminal_receipt", &self.terminal_receipt)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageDebugLayout {
    Lab,
    Module,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageDebugSummary {
    task_id: String,
    verified_sha256: String,
    layout: PackageDebugLayout,
    entry_count: u32,
    resident_bytes: u64,
    task_count: u32,
    has_recognition_pack: bool,
    has_pages: bool,
    has_navigation: bool,
}

impl PackageDebugSummary {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        task_id: impl Into<String>,
        verified_sha256: impl Into<String>,
        layout: PackageDebugLayout,
        entry_count: u32,
        resident_bytes: u64,
        task_count: u32,
        has_recognition_pack: bool,
        has_pages: bool,
        has_navigation: bool,
    ) -> RuntimeContractResult<Self> {
        let summary = Self {
            task_id: task_id.into(),
            verified_sha256: verified_sha256.into(),
            layout,
            entry_count,
            resident_bytes,
            task_count,
            has_recognition_pack,
            has_pages,
            has_navigation,
        };
        summary.validate()?;
        Ok(summary)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.task_id.trim().is_empty()
            || self.task_id.len() > MAX_INSTANCE_ALIAS_BYTES
            || self.entry_count == 0
            || self.resident_bytes == 0
            || self.task_count == 0
        {
            return Err(RuntimeContractError::new("invalid_debug_package_summary"));
        }
        validate_sha256_hex(&self.verified_sha256)
            .map_err(|_| RuntimeContractError::new("invalid_debug_package_summary"))
    }

    pub fn task_id(&self) -> &str {
        &self.task_id
    }

    pub fn verified_sha256(&self) -> &str {
        &self.verified_sha256
    }

    pub const fn layout(&self) -> PackageDebugLayout {
        self.layout
    }

    pub const fn entry_count(&self) -> u32 {
        self.entry_count
    }

    pub const fn resident_bytes(&self) -> u64 {
        self.resident_bytes
    }

    pub const fn task_count(&self) -> u32 {
        self.task_count
    }

    pub const fn has_recognition_pack(&self) -> bool {
        self.has_recognition_pack
    }

    pub const fn has_pages(&self) -> bool {
        self.has_pages
    }

    pub const fn has_navigation(&self) -> bool {
        self.has_navigation
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSubscriptionRequest {
    query: EventQuery,
    profile: ProjectionProfile,
    cursor: SubscriptionCursor,
    wait_ms: u64,
    max_events: u16,
}

impl RuntimeSubscriptionRequest {
    pub fn new(
        query: EventQuery,
        profile: ProjectionProfile,
        cursor: SubscriptionCursor,
        wait_ms: u64,
        max_events: u16,
    ) -> RuntimeContractResult<Self> {
        let request = Self {
            query,
            profile,
            cursor,
            wait_ms,
            max_events,
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.wait_ms > MAX_RUNTIME_SUBSCRIPTION_WAIT_MS
            || self.max_events == 0
            || self.max_events > MAX_RUNTIME_SUBSCRIPTION_EVENTS
        {
            return Err(RuntimeContractError::new(
                "invalid_runtime_subscription_request",
            ));
        }
        Ok(())
    }

    pub const fn query(&self) -> &EventQuery {
        &self.query
    }

    pub const fn profile(&self) -> ProjectionProfile {
        self.profile
    }

    pub const fn cursor(&self) -> SubscriptionCursor {
        self.cursor
    }

    pub const fn wait_ms(&self) -> u64 {
        self.wait_ms
    }

    pub const fn max_events(&self) -> u16 {
        self.max_events
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeEventBatch {
    events: Vec<ProjectedEvent>,
    next_cursor: SubscriptionCursor,
    timed_out: bool,
}

impl RuntimeEventBatch {
    pub fn new(
        events: Vec<ProjectedEvent>,
        next_cursor: SubscriptionCursor,
        timed_out: bool,
    ) -> RuntimeContractResult<Self> {
        let batch = Self {
            events,
            next_cursor,
            timed_out,
        };
        batch.validate()?;
        Ok(batch)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.events.len() > usize::from(MAX_RUNTIME_SUBSCRIPTION_EVENTS)
            || self.events.is_empty() != self.timed_out
        {
            return Err(RuntimeContractError::new("invalid_runtime_event_batch"));
        }
        let mut previous = 0;
        for event in &self.events {
            if event.sequence <= previous || event.sequence > self.next_cursor.after_sequence {
                return Err(RuntimeContractError::new("invalid_runtime_event_batch"));
            }
            previous = event.sequence;
        }
        Ok(())
    }

    pub fn events(&self) -> &[ProjectedEvent] {
        &self.events
    }

    pub const fn next_cursor(&self) -> SubscriptionCursor {
        self.next_cursor
    }

    pub const fn timed_out(&self) -> bool {
        self.timed_out
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeOperation {
    Health,
    Status,
    MonitorStatus,
    ConfigureMonitor {
        instance_alias: String,
        policy: RuntimeMonitorPolicy,
    },
    ClearMonitor {
        instance_alias: String,
    },
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
    CaptureSequence {
        instance_alias: String,
        spec: CaptureSequenceSpec,
    },
    SafeReset {
        instance_alias: String,
        holder_id: HolderId,
    },
    ApplicationLifecycle {
        instance_alias: String,
        holder_id: HolderId,
        action: ApplicationLifecycleAction,
    },
    RunContainedTask {
        instance_alias: String,
        holder_id: HolderId,
        request: ContainedTaskRequest,
    },
    Input {
        token: LeaseToken,
        action: InputAction,
    },
    QueryEvents {
        query: EventQuery,
        profile: ProjectionProfile,
    },
    SubscribeEvents {
        request: RuntimeSubscriptionRequest,
    },
    DebugPackage {
        request: PackageDebugRequest,
    },
    ExportEvidence {
        request: RuntimeEvidenceExportRequest,
    },
    RecordAuthoringEvent {
        event: ResourceAuthoringEvent,
    },
    RecordDebugEvent {
        event: RuntimeDebugEvent,
    },
    RecordClientAction {
        action: ClientActionRecord,
    },
    RecordApprovalDecision {
        decision: ApprovalDecisionRecord,
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

    pub fn application_lifecycle(
        instance_alias: impl Into<String>,
        holder_id: IssuedHolderId,
        action: ApplicationLifecycleAction,
    ) -> Self {
        Self::ApplicationLifecycle {
            instance_alias: instance_alias.into(),
            holder_id: *holder_id.transport(),
            action,
        }
    }

    pub fn run_contained_task(
        instance_alias: impl Into<String>,
        holder_id: IssuedHolderId,
        request: ContainedTaskRequest,
    ) -> Self {
        Self::RunContainedTask {
            instance_alias: instance_alias.into(),
            holder_id: *holder_id.transport(),
            request,
        }
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        match self {
            Self::Health
            | Self::Status
            | Self::MonitorStatus
            | Self::PollQueuedLease { .. }
            | Self::CancelQueuedLease { .. }
            | Self::QueryEvents { .. } => Ok(()),
            Self::SubscribeEvents { request } => request.validate(),
            Self::DebugPackage { request } => request.validate(),
            Self::ExportEvidence { request } => request.validate(),
            Self::RecordAuthoringEvent { event } => event.validate(),
            Self::RecordDebugEvent { event } => event.validate(),
            Self::RecordClientAction { action } => action
                .validate()
                .map_err(|_| RuntimeContractError::new("invalid_client_action")),
            Self::RecordApprovalDecision { decision } => decision
                .validate()
                .map_err(|_| RuntimeContractError::new("invalid_approval_decision")),
            Self::AcquireLease { instance_alias, .. }
            | Self::ObserveReadonly { instance_alias }
            | Self::SafeReset { instance_alias, .. }
            | Self::ApplicationLifecycle { instance_alias, .. }
            | Self::ClearMonitor { instance_alias } => validate_instance_alias(instance_alias),
            Self::ConfigureMonitor {
                instance_alias,
                policy,
            } => {
                validate_instance_alias(instance_alias)?;
                policy
                    .validate()
                    .map_err(|_| RuntimeContractError::new("invalid_runtime_monitor_policy"))
            }
            Self::QueueLease {
                instance_alias,
                policy,
                ..
            } => {
                validate_instance_alias(instance_alias)?;
                policy.validate()
            }
            Self::RenewLease { token } | Self::ReleaseLease { token } => token.validate(),
            Self::CaptureSequence {
                instance_alias,
                spec,
            } => {
                validate_instance_alias(instance_alias)?;
                spec.validate()
            }
            Self::Input { token, action } => {
                token.validate()?;
                action.validate()
            }
            Self::RunContainedTask {
                instance_alias,
                request,
                ..
            } => {
                validate_instance_alias(instance_alias)?;
                request.validate()
            }
        }
    }

    pub fn instance_alias(&self) -> Option<&str> {
        match self {
            Self::AcquireLease { instance_alias, .. }
            | Self::QueueLease { instance_alias, .. }
            | Self::ObserveReadonly { instance_alias }
            | Self::CaptureSequence { instance_alias, .. }
            | Self::SafeReset { instance_alias, .. }
            | Self::ApplicationLifecycle { instance_alias, .. }
            | Self::RunContainedTask { instance_alias, .. }
            | Self::ConfigureMonitor { instance_alias, .. }
            | Self::ClearMonitor { instance_alias } => Some(instance_alias),
            Self::RecordClientAction { action } => action.instance_alias(),
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
            Self::MonitorStatus => "RuntimeOperation::MonitorStatus",
            Self::ConfigureMonitor { .. } => "RuntimeOperation::ConfigureMonitor(<redacted>)",
            Self::ClearMonitor { .. } => "RuntimeOperation::ClearMonitor(<redacted>)",
            Self::AcquireLease { .. } => "RuntimeOperation::AcquireLease(<redacted>)",
            Self::QueueLease { .. } => "RuntimeOperation::QueueLease(<redacted>)",
            Self::PollQueuedLease { .. } => "RuntimeOperation::PollQueuedLease(<opaque-request>)",
            Self::CancelQueuedLease { .. } => {
                "RuntimeOperation::CancelQueuedLease(<opaque-request>)"
            }
            Self::RenewLease { .. } => "RuntimeOperation::RenewLease(<opaque-token>)",
            Self::ReleaseLease { .. } => "RuntimeOperation::ReleaseLease(<opaque-token>)",
            Self::ObserveReadonly { .. } => "RuntimeOperation::ObserveReadonly(<redacted>)",
            Self::CaptureSequence { .. } => "RuntimeOperation::CaptureSequence(<redacted>)",
            Self::SafeReset { .. } => "RuntimeOperation::SafeReset(<redacted>)",
            Self::ApplicationLifecycle { .. } => {
                "RuntimeOperation::ApplicationLifecycle(<redacted>)"
            }
            Self::RunContainedTask { .. } => "RuntimeOperation::RunContainedTask(<redacted>)",
            Self::Input { .. } => "RuntimeOperation::Input(<redacted>)",
            Self::QueryEvents { .. } => "RuntimeOperation::QueryEvents(<typed-query>)",
            Self::SubscribeEvents { .. } => "RuntimeOperation::SubscribeEvents(<typed-query>)",
            Self::DebugPackage { .. } => "RuntimeOperation::DebugPackage(<redacted>)",
            Self::ExportEvidence { .. } => "RuntimeOperation::ExportEvidence(<redacted>)",
            Self::RecordAuthoringEvent { .. } => {
                "RuntimeOperation::RecordAuthoringEvent(<redacted>)"
            }
            Self::RecordDebugEvent { .. } => {
                "RuntimeOperation::RecordDebugEvent(<typed-debug-event>)"
            }
            Self::RecordClientAction { .. } => {
                "RuntimeOperation::RecordClientAction(<typed-redacted-action>)"
            }
            Self::RecordApprovalDecision { .. } => {
                "RuntimeOperation::RecordApprovalDecision(<typed-approval>)"
            }
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
        if matches!(
            self.operation,
            RuntimeOperation::RecordAuthoringEvent { .. }
        ) && (self.actor != EventActor::Lab || self.source != EventSource::Lab)
        {
            return Err(RuntimeContractError::new(
                "invalid_resource_authoring_origin",
            ));
        }
        if matches!(self.operation, RuntimeOperation::RecordDebugEvent { .. })
            && (self.actor != EventActor::Lab || self.source != EventSource::Lab)
        {
            return Err(RuntimeContractError::new("invalid_runtime_debug_origin"));
        }
        if matches!(
            self.operation,
            RuntimeOperation::DebugPackage { .. } | RuntimeOperation::ExportEvidence { .. }
        ) && (self.actor != EventActor::Lab || self.source != EventSource::Lab)
        {
            return Err(RuntimeContractError::new("invalid_runtime_debug_origin"));
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

    pub fn task_event_links(&self, task_id: IssuedTaskId, run_id: IssuedRunId) -> EventLinksDraft {
        self.event_links(None, None, None)
            .with_task_id(task_id)
            .with_run_id(run_id)
    }

    pub fn task_artifact_links(&self, run_id: IssuedRunId) -> ArtifactLinksDraft {
        ArtifactLinksDraft::default()
            .with_run_id(run_id)
            .with_correlation_id(IssuedCorrelationId::from_verified_transport(
                self.request.correlation_id,
            ))
    }

    pub const fn correlation_id(&self) -> CorrelationId {
        self.request.correlation_id
    }

    pub const fn actor(&self) -> EventActor {
        self.request.actor
    }

    pub const fn source(&self) -> EventSource {
        self.request.source
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
    PackageInvalid,
    EvidenceExportFailed,
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
    MonitorStatus {
        status: RuntimeMonitorRegistryStatus,
    },
    MonitorConfigured {
        status: RuntimeMonitorInstanceStatus,
    },
    MonitorCleared {
        status: RuntimeMonitorInstanceStatus,
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
    CaptureSequenceCompleted {
        sequence: CaptureSequence,
    },
    SafeResetCompleted {
        action_id: ActionId,
    },
    ApplicationLifecycleCompleted {
        action_id: ActionId,
        action: ApplicationLifecycleAction,
    },
    ContainedTaskCompleted {
        run_id: RunId,
        task_id: crate::TaskId,
        outcome: TaskOutcome,
        #[serde(skip_serializing_if = "Option::is_none")]
        final_page: Option<String>,
        executed_steps: u32,
    },
    InputCommitted {
        action_id: ActionId,
    },
    Events {
        events: Vec<ProjectedEvent>,
    },
    EventBatch {
        batch: RuntimeEventBatch,
    },
    PackageDebugCompleted {
        summary: PackageDebugSummary,
    },
    EvidenceExportCompleted {
        summary: Box<RuntimeEvidenceExportSummary>,
    },
    AuthoringEventRecorded {
        phase: ResourceAuthoringPhase,
    },
    DebugEventRecorded {
        phase: RuntimeDebugPhase,
    },
    ClientActionRecorded,
    ApprovalDecisionRecorded {
        approval_id: String,
        disposition: ApprovalDisposition,
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
            Some(RuntimeResult::MonitorStatus { status }) => status
                .validate()
                .map_err(|_| RuntimeContractError::new("invalid_runtime_monitor_status"))?,
            Some(
                RuntimeResult::MonitorConfigured { status }
                | RuntimeResult::MonitorCleared { status },
            ) => status
                .validate()
                .map_err(|_| RuntimeContractError::new("invalid_runtime_monitor_status"))?,
            Some(
                RuntimeResult::LeaseQueued { status } | RuntimeResult::LeasePending { status },
            ) => status.validate()?,
            Some(RuntimeResult::ReadonlyObservationCompleted { observation }) => {
                observation.validate()?
            }
            Some(RuntimeResult::CaptureSequenceCompleted { sequence }) => sequence.validate()?,
            Some(RuntimeResult::EventBatch { batch }) => batch.validate()?,
            Some(RuntimeResult::PackageDebugCompleted { summary }) => summary.validate()?,
            Some(RuntimeResult::EvidenceExportCompleted { summary }) => summary.validate()?,
            Some(RuntimeResult::ContainedTaskCompleted {
                outcome,
                final_page,
                executed_steps,
                ..
            }) if *outcome != TaskOutcome::Success
                || *executed_steps > 1_000
                || final_page
                    .as_deref()
                    .is_some_and(|value| value.is_empty() || value.len() > 256) =>
            {
                return Err(RuntimeContractError::new("invalid_contained_task_result"));
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

    pub const fn started_at_unix_ms(&self) -> u64 {
        self.started_at_unix_ms
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

fn validate_sha256_hex(value: &str) -> RuntimeContractResult<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(RuntimeContractError::new("invalid_sha256"));
    }
    Ok(())
}

fn validate_canonical_sha256(value: &str) -> RuntimeContractResult<()> {
    value
        .strip_prefix("sha256:")
        .ok_or_else(|| RuntimeContractError::new("invalid_sha256"))
        .and_then(validate_sha256_hex)
}

const fn terminal_event_type(outcome: TaskOutcome) -> EventType {
    match outcome {
        TaskOutcome::Success => EventType::TaskCompleted,
        TaskOutcome::Failure => EventType::TaskFailed,
        TaskOutcome::Cancelled => EventType::TaskCancelled,
    }
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
