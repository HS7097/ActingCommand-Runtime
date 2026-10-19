// SPDX-License-Identifier: AGPL-3.0-only

mod retention;
use crate::codes::{ContractCode, RuntimeContractCode, SanitizationCode, SharedCode};
use crate::outcome::{CauseRelation, Code, IoKind, Location, Outcome, RawSource};
pub use retention::*;
mod ledger_migration;
mod signature;
pub use ledger_migration::*;

pub use signature::*;
mod vendor_stdio;
pub use vendor_stdio::*;
mod adb_recovery;
pub use adb_recovery::*;
mod owner_unlock;
pub use owner_unlock::*;
mod governance_identity;
pub use governance_identity::*;
mod recovery_ladder;
pub use recovery_ladder::*;
mod device_self_check;
pub use device_self_check::*;
mod task_selection;
pub use task_selection::*;
// Workflow #383 SH-1 (register §1.1, §1.2, §1.4): the v0.12.0 shapes of the ledger observer,
// the decision memory, the fact-refresh transitions and performance pacing.
mod observer;
pub use observer::*;
mod pacing;
pub use pacing::*;
// Workflow #383 SH-2 (register §1.25 X7): the display map of a task geometry observation.
mod display_map;
pub use display_map::*;
// Workflow #390 OUT-2: designated effect sets.
mod designated_effects;
pub use designated_effects::*;
// Workflow #411 B15 (contracts/remedies.md section 2): declared remedies and provided capabilities.
mod remedies;
pub use remedies::*;

use super::{
    ArtifactRedactionState, CapturePolicyReason, CapturePressureState, DiagnosticCode, EventAction,
    EventFamily, EventType, EvidenceCompleteness, PinnedFrameReason, PolicyFailureClass,
    PolicyFailureDisposition, PolicyPlanningSignalKind, RecognitionVerdict, RecoveryReason,
    ResourceAuthoringPhase, RetentionClass, SanitizationError, Sensitivity, TaskOutcome,
};
use crate::{
    AgentAttentionState, AgentSessionEventData, AgentSessionId, AgentWakeData, AgentWakeId,
    AgentWakeKind, ApprovalDecisionRecord, ApprovalDisposition, ApprovalTarget, ApprovalTargetKind,
    ArbitrationBasis, ArtifactKind, CatalogPromotionAuthorization, ClientActionKind,
    ClientActionRecord, CorrelationId, EventId, FactInvalidationEventData, FactRecord, FactScope,
    HolderId, InputAction, InstanceId, LeaseId, LeasePriority, MonitorDecision, MonitorDiagnosis,
    MonitorDisposition, MonitorObservation, MonitorRecoveryCoordinationReason, MonitorRecoveryKind,
    OwnerEpoch, PerformanceContext, PerformanceControlEventData, PerformanceControlLevel,
    PerformanceControlReason, PerformanceDeadlineDisposition, PerformanceMonitorHealth,
    PerformanceMonitorStateEventData, PerformancePressureEventData, PerformancePressureRecord,
    PerformanceStutterEventData, PerformanceSummaryEventData, ProjectedArtifactReference,
    ReleaseTransitionData, ReleaseTransitionKind, RequestId, RunId, RuntimeReleaseSet,
    SEGMENTED_SWIPE_BRAKE_DISTANCE_PX, SEGMENTED_SWIPE_BRAKE_DURATION_MS,
    SEGMENTED_SWIPE_CORNER_HOLD_MS, SEGMENTED_SWIPE_HORIZONTAL_DURATION_MS, StateMigrationData,
    TaskId, validate_fact_invalidation, validate_performance_control,
    validate_performance_monitor_state, validate_performance_stutter, validate_performance_summary,
};
use serde::de;
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt;

mod provider;
pub use provider::*;

mod device_diagnostic;
pub use device_diagnostic::*;

pub const COMMAND_PAYLOAD_SCHEMA: &str = "actingcommand.payload.command.v2";
pub const RUNTIME_PAYLOAD_SCHEMA: &str = "actingcommand.payload.runtime.v1";
pub const MONITOR_PAYLOAD_SCHEMA: &str = "actingcommand.payload.monitor.v1";
pub const PERFORMANCE_PAYLOAD_SCHEMA: &str = "actingcommand.payload.performance.v1";
pub const FACT_PAYLOAD_SCHEMA: &str = "actingcommand.payload.fact.v1";
pub const APPROVAL_PAYLOAD_SCHEMA: &str = "actingcommand.payload.approval.v1";
pub const SCHEDULER_PAYLOAD_SCHEMA: &str = "actingcommand.payload.scheduler.v3";
pub const POLICY_PAYLOAD_SCHEMA: &str = "actingcommand.payload.policy.v1";
pub const CATALOG_PAYLOAD_SCHEMA: &str = "actingcommand.payload.catalog.v1";
pub const LEASE_PAYLOAD_SCHEMA: &str = "actingcommand.payload.lease.v3";
pub const TASK_PAYLOAD_SCHEMA: &str = "actingcommand.payload.task.v3";
pub const APPLICATION_PAYLOAD_SCHEMA: &str = "actingcommand.payload.application.v1";
pub const INPUT_PAYLOAD_SCHEMA: &str = "actingcommand.payload.input.v2";
pub const INPUT_EXECUTION_PLAN_VERSION: &str = "actingcommand.input.execution_plan.v1";
pub const INPUT_EXECUTION_PLAN_PROFILE_MAA_2_0: &str = "maa_2_0";
pub const MAX_INPUT_EXECUTION_PLAN_EVENTS: usize = 123;
pub const CAPTURE_PAYLOAD_SCHEMA: &str = "actingcommand.payload.capture.v1";
pub const RECOGNITION_PAYLOAD_SCHEMA: &str = "actingcommand.payload.recognition.v1";
pub const ARTIFACT_PAYLOAD_SCHEMA: &str = "actingcommand.payload.artifact.v1";
pub const RESOURCE_AUTHORING_PAYLOAD_SCHEMA: &str = "actingcommand.payload.resource_authoring.v1";
pub const CLIENT_PAYLOAD_SCHEMA: &str = "actingcommand.payload.client.v2";
pub const STATE_PAYLOAD_SCHEMA: &str = "actingcommand.payload.state.v1";
pub const RELEASE_PAYLOAD_SCHEMA: &str = "actingcommand.payload.release.v1";
pub const AGENT_PAYLOAD_SCHEMA: &str = "actingcommand.payload.agent.v1";
pub const LEDGER_PAYLOAD_SCHEMA: &str = "actingcommand.payload.ledger.v2";
pub const MAX_CAPTURE_SUMMARY_COUNT: u64 = 1_000_000;
pub const MAX_CAPTURE_SUMMARY_FRAMES: usize = 16_384;
pub const MAX_CAPTURE_SUMMARY_PINS: usize = 65_536;
const MAX_DIAGNOSTIC_DETAIL_TOKEN_BYTES: usize = 256;
/// A detail's stage is a location: at most 64 bytes (M378 §3.2).
const MAX_DIAGNOSTIC_DETAIL_STAGE_BYTES: usize = 64;
const MAX_DIAGNOSTIC_DETAIL_MESSAGE_BYTES: usize = 1_024;
const MAX_MACHINE_PATH_BASENAME_BYTES: usize = 255;
const LEGACY_REDACTED_MACHINE_PATH: &str = "[redacted]";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretField {
    AccountIdentity,
    AuthenticationMaterial,
    /// Workflow #398 RD-1b (model-398 v1.1 §3.4): a handset's adb serial, recorded only as its
    /// fingerprint.
    DeviceSerial,
}

pub trait SecretFingerprinter {
    fn fingerprint(
        &self,
        field: SecretField,
        original: &str,
    ) -> Result<Sha256Fingerprint, SanitizationError>;
}

#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Sha256Fingerprint(String);

impl Sha256Fingerprint {
    pub fn new(candidate: impl Into<String>, original: &str) -> Result<Self, SanitizationError> {
        let candidate = candidate.into();
        validate_fingerprint(&candidate, original)?;
        Ok(Self(candidate))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn validate_for(&self, original: &str) -> Result<(), SanitizationError> {
        validate_fingerprint(&self.0, original)
    }

    fn validate_stored(&self) -> Result<(), SanitizationError> {
        if is_sha256(&self.0) {
            Ok(())
        } else {
            Err(SanitizationError::new(
                SanitizationCode::InvalidFingerprint,
                "account_identity",
            ))
        }
    }
}

impl fmt::Debug for Sha256Fingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Sha256Fingerprint(<redacted>)")
    }
}

impl<'de> Deserialize<'de> for Sha256Fingerprint {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let fingerprint = Self(value);
        fingerprint.validate_stored().map_err(de::Error::custom)?;
        Ok(fingerprint)
    }
}

#[derive(Default)]
pub struct AuditInput {
    account: Option<String>,
    authentication: Option<String>,
    machine_path: Option<String>,
    device_endpoint: Option<String>,
}

impl AuditInput {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_account(mut self, value: impl Into<String>) -> Self {
        self.account = Some(value.into());
        self
    }

    pub fn with_authentication(mut self, value: impl Into<String>) -> Self {
        self.authentication = Some(value.into());
        self
    }

    pub fn with_machine_path(mut self, value: impl Into<String>) -> Self {
        self.machine_path = Some(value.into());
        self
    }

    pub fn with_device_endpoint(mut self, value: impl Into<String>) -> Self {
        self.device_endpoint = Some(value.into());
        self
    }

    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<SanitizedAudit, SanitizationError> {
        let account_fingerprint = if let Some(original) = self.account {
            let fingerprint = fingerprinter
                .fingerprint(SecretField::AccountIdentity, &original)
                .map_err(|_| {
                    SanitizationError::new(
                        SanitizationCode::FingerprinterFailed,
                        "account_identity",
                    )
                })?;
            fingerprint.validate_for(&original)?;
            Some(fingerprint)
        } else {
            None
        };
        Ok(SanitizedAudit {
            account_fingerprint,
            authentication_redacted: self.authentication.is_some(),
            machine_path: self.machine_path.map(sanitize_machine_path).transpose()?,
            device_endpoint: self.device_endpoint.map(|_| "[redacted]".to_string()),
        })
    }
}

impl fmt::Debug for AuditInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuditInput")
            .field("account", &self.account.is_some())
            .field("authentication", &self.authentication.is_some())
            .field("machine_path", &self.machine_path.is_some())
            .field("device_endpoint", &self.device_endpoint.is_some())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SanitizedAudit {
    #[serde(skip_serializing_if = "Option::is_none")]
    account_fingerprint: Option<Sha256Fingerprint>,
    #[serde(default, skip_serializing_if = "is_false")]
    authentication_redacted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    machine_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    device_endpoint: Option<String>,
}

impl SanitizedAudit {
    pub fn account_fingerprint(&self) -> Option<&Sha256Fingerprint> {
        self.account_fingerprint.as_ref()
    }

    pub const fn authentication_redacted(&self) -> bool {
        self.authentication_redacted
    }

    pub fn machine_path(&self) -> Option<&str> {
        self.machine_path.as_deref()
    }

    pub fn device_endpoint(&self) -> Option<&str> {
        self.device_endpoint.as_deref()
    }

    fn sensitivity(&self) -> Sensitivity {
        if self.account_fingerprint.is_some() || self.authentication_redacted {
            Sensitivity::Secret
        } else if self.machine_path.is_some() || self.device_endpoint.is_some() {
            Sensitivity::Sensitive
        } else {
            Sensitivity::Public
        }
    }

    fn validate(&self) -> Result<(), SanitizationError> {
        if self
            .account_fingerprint
            .as_ref()
            .is_some_and(|value| value.validate_stored().is_err())
            || self
                .machine_path
                .as_deref()
                .is_some_and(|value| !valid_stored_machine_path(value))
            || self
                .device_endpoint
                .as_deref()
                .is_some_and(|value| value != "[redacted]")
        {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidSanitizedPayload,
                "audit",
            ));
        }
        Ok(())
    }
}

fn sanitize_machine_path(original: String) -> Result<String, SanitizationError> {
    let basename = original
        .rsplit(['/', '\\'])
        .next()
        .filter(|value| valid_machine_path_basename(value))
        .ok_or_else(|| {
            SanitizationError::new(SanitizationCode::InvalidMachinePath, "machine_path")
        })?;
    let encoded_basename = serde_json::to_string(basename).map_err(|_| {
        SanitizationError::new(SanitizationCode::InvalidMachinePath, "machine_path")
    })?;
    Ok(format!(
        "basename:{encoded_basename}|sha256:{:x}",
        Sha256::digest(original.as_bytes())
    ))
}

fn valid_stored_machine_path(value: &str) -> bool {
    if value == LEGACY_REDACTED_MACHINE_PATH {
        return true;
    }
    let Some(encoded) = value.strip_prefix("basename:") else {
        return false;
    };
    let Some((encoded_basename, digest)) = encoded.rsplit_once("|sha256:") else {
        return false;
    };
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return false;
    }
    let Ok(basename) = serde_json::from_str::<String>(encoded_basename) else {
        return false;
    };
    valid_machine_path_basename(&basename)
        && serde_json::to_string(&basename).is_ok_and(|canonical| canonical == encoded_basename)
}

fn valid_machine_path_basename(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_MACHINE_PATH_BASENAME_BYTES
        && !matches!(value, "." | "..")
        && !value
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\'))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectDisposition {
    NotPerformed,
    Performed,
    Indeterminate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationPayload {
    action: EventAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    device_diagnostics: Option<DeviceDiagnosticConfig>,
    /// Workflow #389 (register U1): the channel of a command record's connection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    channel: Option<crate::RuntimeChannel>,
    /// Workflow #389 (register U1): the counter that handed a Lab request over, on `lab` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    via: Option<crate::ChannelVia>,
    audit: SanitizedAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InputExecutionPlanEvent {
    Down {
        x: i32,
        y: i32,
    },
    Move {
        x: i32,
        y: i32,
        delay_before_ms: u64,
    },
    Hold {
        duration_ms: u64,
    },
    Up,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputExecutionPlanRecord {
    version: String,
    profile: String,
    declared_sensitivity: Sensitivity,
    events: Vec<InputExecutionPlanEvent>,
}

impl InputExecutionPlanRecord {
    pub fn new(events: Vec<InputExecutionPlanEvent>) -> Result<Self, SanitizationError> {
        let record = Self {
            version: INPUT_EXECUTION_PLAN_VERSION.to_string(),
            profile: INPUT_EXECUTION_PLAN_PROFILE_MAA_2_0.to_string(),
            declared_sensitivity: Sensitivity::Internal,
            events,
        };
        record.validate()?;
        Ok(record)
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn profile(&self) -> &str {
        &self.profile
    }

    pub const fn declared_sensitivity(&self) -> Sensitivity {
        self.declared_sensitivity
    }

    pub fn events(&self) -> &[InputExecutionPlanEvent] {
        &self.events
    }

    fn validate(&self) -> Result<(), SanitizationError> {
        if self.version != INPUT_EXECUTION_PLAN_VERSION
            || self.profile != INPUT_EXECUTION_PLAN_PROFILE_MAA_2_0
            || self.declared_sensitivity != Sensitivity::Internal
            || self.events.len() > MAX_INPUT_EXECUTION_PLAN_EVENTS
            || !matches!(
                self.events.first(),
                Some(InputExecutionPlanEvent::Down { .. })
            )
            || !matches!(self.events.last(), Some(InputExecutionPlanEvent::Up))
        {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidInputExecutionPlan,
                "execution_plan",
            ));
        }

        let holds = self
            .events
            .iter()
            .enumerate()
            .filter_map(|(index, event)| match event {
                InputExecutionPlanEvent::Hold {
                    duration_ms: SEGMENTED_SWIPE_CORNER_HOLD_MS,
                } => Some(index),
                _ => None,
            })
            .collect::<Vec<_>>();
        if holds.len() != 1 {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidInputExecutionPlan,
                "events",
            ));
        }
        let hold_index = holds[0];
        if hold_index <= 1 || hold_index + 2 >= self.events.len() {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidInputExecutionPlan,
                "events",
            ));
        }

        let validate_moves = |events: &[InputExecutionPlanEvent], expected_duration_ms| {
            let mut duration_ms = 0_u64;
            for event in events {
                let InputExecutionPlanEvent::Move {
                    x,
                    y,
                    delay_before_ms,
                } = event
                else {
                    return Err(SanitizationError::new(
                        SanitizationCode::InvalidInputExecutionPlan,
                        "events",
                    ));
                };
                if *x < 0 || *y < 0 || *delay_before_ms == 0 {
                    return Err(SanitizationError::new(
                        SanitizationCode::InvalidInputExecutionPlan,
                        "events",
                    ));
                }
                duration_ms = duration_ms.checked_add(*delay_before_ms).ok_or_else(|| {
                    SanitizationError::new(SanitizationCode::InvalidInputExecutionPlan, "events")
                })?;
            }
            if duration_ms != expected_duration_ms {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidInputExecutionPlan,
                    "events",
                ));
            }
            Ok(())
        };
        validate_moves(
            &self.events[1..hold_index],
            SEGMENTED_SWIPE_HORIZONTAL_DURATION_MS,
        )?;
        validate_moves(
            &self.events[hold_index + 1..self.events.len() - 1],
            SEGMENTED_SWIPE_BRAKE_DURATION_MS,
        )?;

        let InputExecutionPlanEvent::Down { x, y } = self.events[0] else {
            unreachable!();
        };
        if x < 0 || y < 0 {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidInputExecutionPlan,
                "events",
            ));
        }
        let InputExecutionPlanEvent::Move {
            x: corner_x,
            y: corner_y,
            ..
        } = self.events[hold_index - 1]
        else {
            unreachable!();
        };
        let InputExecutionPlanEvent::Move {
            x: end_x, y: end_y, ..
        } = self.events[self.events.len() - 2]
        else {
            unreachable!();
        };
        if end_x != corner_x
            || corner_y.checked_sub(SEGMENTED_SWIPE_BRAKE_DISTANCE_PX) != Some(end_y)
        {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidInputExecutionPlan,
                "events",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputIntentPayload {
    action: EventAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    execution_plan: Option<InputExecutionPlanRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provenance: Option<InputIntentProvenance>,
    audit: SanitizedAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputIntentProvenance {
    pub input_action: InputAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_step_action_id: Option<crate::ActionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_frame_id: Option<crate::FrameId>,
}

impl InputIntentPayload {
    pub const fn action(&self) -> EventAction {
        self.action
    }

    pub const fn execution_plan(&self) -> Option<&InputExecutionPlanRecord> {
        self.execution_plan.as_ref()
    }

    pub const fn provenance(&self) -> Option<&InputIntentProvenance> {
        self.provenance.as_ref()
    }

    pub fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }

    fn validate(&self) -> Result<(), SanitizationError> {
        if let Some(provenance) = &self.provenance {
            provenance.input_action.validate().map_err(|_| {
                SanitizationError::new(SanitizationCode::InvalidInputProvenance, "input_action")
            })?;
            if self.action != provenance.input_action.event_action() {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidInputProvenance,
                    "action",
                ));
            }
        }
        if let Some(execution_plan) = &self.execution_plan {
            if self.action != EventAction::InputSwipe {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidInputExecutionPlan,
                    "action",
                ));
            }
            execution_plan.validate()?;
        }
        Ok(())
    }
}

/// Where a failure arose, as its producer names it: the category, the stage, the backend and
/// the operation. Ledger revision 3 (Workflow #378 A3) persists no failure text: the failure
/// itself is the record's `outcome`, foreign text rides on its `foreign_*` links, and text the
/// Runtime made goes only to the developer log. The stage is a registered location (Workflow
/// #378 A2-P); the category, backend and operation stay typed context beside the outcome until
/// they are registered (the device categories and backends in B5; the operation relays other
/// crates' spellings until the A2b/A2c merge follow-up).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticDetailDraft {
    category: String,
    stage: Location,
    backend: String,
    operation: String,
}

impl DiagnosticDetailDraft {
    pub fn new(
        category: impl Into<String>,
        stage: impl Into<Location>,
        backend: impl Into<String>,
        operation: impl Into<String>,
    ) -> Self {
        Self {
            category: category.into(),
            stage: stage.into(),
            backend: backend.into(),
            operation: operation.into(),
        }
    }

    pub fn category(&self) -> &str {
        &self.category
    }

    pub const fn stage(&self) -> Location {
        self.stage
    }

    pub fn backend(&self) -> &str {
        &self.backend
    }

    pub fn operation(&self) -> &str {
        &self.operation
    }

    fn sanitize(self) -> Result<DiagnosticDetailRecord, SanitizationError> {
        let stage = self.stage.as_str();
        validate_diagnostic_detail(&self.category, stage, &self.backend, &self.operation)?;
        Ok(DiagnosticDetailRecord {
            category: self.category,
            stage: stage.to_owned(),
            backend: self.backend,
            operation: self.operation,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticDetailRecord {
    category: String,
    stage: String,
    backend: String,
    operation: String,
}

impl DiagnosticDetailRecord {
    pub fn category(&self) -> &str {
        &self.category
    }

    pub fn stage(&self) -> &str {
        &self.stage
    }

    pub fn backend(&self) -> &str {
        &self.backend
    }

    pub fn operation(&self) -> &str {
        &self.operation
    }

    fn validate(&self) -> Result<(), SanitizationError> {
        validate_diagnostic_detail(&self.category, &self.stage, &self.backend, &self.operation)
    }
}

fn validate_diagnostic_detail(
    category: &str,
    stage: &str,
    backend: &str,
    operation: &str,
) -> Result<(), SanitizationError> {
    validate_diagnostic_detail_token(category, "category")?;
    validate_diagnostic_detail_stage(stage)?;
    validate_diagnostic_detail_token(backend, "backend")?;
    validate_diagnostic_detail_token(operation, "operation")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupCauseSeverity {
    Transient,
    Fatal,
}

/// A failed cleanup that accompanied a failure. Ledger revision 3 records it as the failure
/// outcome's `cleanup` link (with the cleanup's native text as its foreign link), and its
/// diagnostic detail as the record's `cleanup_detail`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupCauseDraft {
    code: Code,
    severity: CleanupCauseSeverity,
    detail: Option<DiagnosticDetailDraft>,
    native_detail: Option<LifecycleNativeDetail>,
}

impl CleanupCauseDraft {
    pub fn new(
        code: impl Into<Code>,
        severity: CleanupCauseSeverity,
        detail: Option<DiagnosticDetailDraft>,
    ) -> Self {
        Self {
            code: code.into(),
            severity,
            detail,
            native_detail: None,
        }
    }

    /// The cleanup's own native text (a backend's output), carried as a foreign link.
    pub fn with_native_detail(mut self, detail: Option<LifecycleNativeDetail>) -> Self {
        self.native_detail = detail;
        self
    }

    pub const fn code(&self) -> Code {
        self.code
    }

    pub const fn severity(&self) -> CleanupCauseSeverity {
        self.severity
    }

    pub const fn detail(&self) -> Option<&DiagnosticDetailDraft> {
        self.detail.as_ref()
    }

    /// The cleanup's own outcome, attached as the failure's `cleanup` link: its code, with its
    /// native text as a `foreign_other_error` link.
    pub fn outcome(&self) -> Outcome {
        let outcome = Outcome::new(self.code);
        match &self.native_detail {
            Some(native) => outcome.caused_by(native.outcome()),
            None => outcome,
        }
    }

    fn sanitize(self) -> Result<(Outcome, Option<DiagnosticDetailRecord>), SanitizationError> {
        if let Some(native) = &self.native_detail {
            native.validate()?;
        }
        let outcome = self.outcome();
        let detail = self
            .detail
            .map(DiagnosticDetailDraft::sanitize)
            .transpose()?;
        Ok((outcome, detail))
    }
}

fn validate_diagnostic_detail_token(
    value: &str,
    field: &'static str,
) -> Result<(), SanitizationError> {
    if value.is_empty()
        || value.len() > MAX_DIAGNOSTIC_DETAIL_TOKEN_BYTES
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
    {
        Err(SanitizationError::new(
            SanitizationCode::InvalidDiagnosticDetailToken,
            field,
        ))
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleFailurePhase {
    Reset,
    ChildStop,
    StderrReaderJoin,
    UnexpectedStderr,
    Retirement,
    ResourceClose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeResourceKind {
    CaptureBackend,
    InputBackend,
    ProviderConnection,
    VendorStdio,
    ExternalChild,
    InProcessWorker,
    Library,
    FileDescriptor,
    TemporaryPath,
    PipeReader,
    FactoryCandidate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeResourceClosePhase {
    Close,
    AcquisitionCleanup,
    DisconnectSymbol,
    DisconnectCall,
    WorkerSend,
    WorkerReceive,
    WorkerJoin,
    InitialPoll,
    Kill,
    ExitPoll,
    Deadline,
    RestoreFlush,
    RestoreWin32,
    RestoreCrt,
    SnapshotFlush,
    SnapshotRead,
    FileDescriptorClose,
    Unlink,
    LibraryUnload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceQuiescence {
    Confirmed,
    Unconfirmed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceDispositionScope {
    OperationResources,
    SessionBackends,
}

/// The resources described by this lifecycle boundary, grouped by actual ownership.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceDisposition {
    pub scope: ResourceDispositionScope,
    pub resource_count: u16,
    pub quiescence: ResourceQuiescence,
}

fn validate_resource_dispositions(
    groups: &[ResourceDisposition],
) -> Result<(u16, ResourceQuiescence), SanitizationError> {
    if groups.is_empty()
        || groups.len() > 2
        || (groups.len() == 2 && groups[0].scope == groups[1].scope)
    {
        return Err(SanitizationError::new(
            SanitizationCode::InvalidResourceDispositionScopes,
            "resource_dispositions",
        ));
    }
    let mut count = 0_u16;
    let mut quiescence = ResourceQuiescence::Confirmed;
    for group in groups {
        count = count.checked_add(group.resource_count).ok_or_else(|| {
            SanitizationError::new(
                SanitizationCode::ResourceCountOverflow,
                "resource_dispositions",
            )
        })?;
        if group.quiescence == ResourceQuiescence::Unconfirmed {
            quiescence = ResourceQuiescence::Unconfirmed;
        }
    }
    Ok((count, quiescence))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerResourceDisposition {
    None,
    InUse,
    ConfirmedClosed,
    Unconfirmed,
}

/// Text a third party produced (a backend's output, an OS or store error message), bounded to
/// 1 KiB. Ledger revision 3 carries it as `raw_text` on a `foreign_*` link of the failure's
/// outcome ([`LifecycleNativeDetail::outcome`]), with its declared sensitivity; only the typed
/// ADB recovery evidence still holds it as a member (B5 moves device evidence to links).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleNativeDetail {
    text: String,
    truncated: bool,
    declared_sensitivity: Sensitivity,
    /// Which third party wrote the text: it picks the `foreign_*` link [`Self::outcome`]
    /// builds. Not a record member; the typed ADB recovery evidence that keeps the text as a
    /// member carries no other source.
    #[serde(skip)]
    source: NativeTextSource,
}

/// The third party behind a [`LifecycleNativeDetail`] (ledger revision 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum NativeTextSource {
    /// Any other third party: a `foreign_other_error` link.
    #[default]
    Other,
    /// MuMuManager's output once its command exited: a `foreign_mumu_manager_error` link
    /// with the exit status as `exit_code`, never an OS error.
    MumuManager { exit_code: i32 },
}

impl std::fmt::Debug for LifecycleNativeDetail {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LifecycleNativeDetail")
            .field("truncated", &self.truncated)
            .field("declared_sensitivity", &self.declared_sensitivity)
            .finish_non_exhaustive()
    }
}

impl LifecycleNativeDetail {
    pub fn bounded(text: &str) -> Option<Self> {
        if text.is_empty() {
            return None;
        }
        let mut end = text.len().min(MAX_DIAGNOSTIC_DETAIL_MESSAGE_BYTES);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        Some(Self::new(&text[..end], end < text.len()))
    }

    pub fn new(text: impl Into<String>, truncated: bool) -> Self {
        Self {
            text: text.into(),
            truncated,
            declared_sensitivity: Sensitivity::Sensitive,
            source: NativeTextSource::Other,
        }
    }

    /// This text as MuMuManager's output from a command that exited with `exit_code`.
    pub fn with_mumu_manager_exit_code(mut self, exit_code: i32) -> Self {
        self.source = NativeTextSource::MumuManager { exit_code };
        self
    }

    /// Keeps a sensitivity its source declared above `sensitive` (a `secret` device text);
    /// native text is never recorded below `sensitive`.
    pub fn with_declared_sensitivity(mut self, sensitivity: Sensitivity) -> Self {
        self.declared_sensitivity = sensitivity.max(Sensitivity::Sensitive);
        self
    }

    pub fn text(&self) -> &str {
        &self.text
    }
    pub const fn truncated(&self) -> bool {
        self.truncated
    }

    /// The link that carries this text verbatim as `raw_text`: `foreign_mumu_manager_error`
    /// with `exit_code` for MuMuManager's output, else `foreign_other_error` (`raw_source`
    /// `other`; B5 names the other device sources).
    pub fn outcome(&self) -> Outcome {
        match self.source {
            NativeTextSource::Other => foreign_text_outcome(
                ContractCode::ForeignOtherError,
                RawSource::Other,
                &self.text,
                self.truncated,
                self.declared_sensitivity,
            ),
            NativeTextSource::MumuManager { exit_code } => foreign_text_outcome(
                ContractCode::ForeignMumuManagerError,
                RawSource::MumuManager,
                &self.text,
                self.truncated,
                self.declared_sensitivity,
            )
            .exit_code(exit_code),
        }
    }

    /// The `foreign_os_error` link of an OS error number with this text as its `raw_text`.
    pub fn os_outcome(&self, os_error: i32) -> Outcome {
        foreign_text_outcome(
            ContractCode::ForeignOsError,
            RawSource::Os,
            &self.text,
            self.truncated,
            self.declared_sensitivity,
        )
        .os_error(os_error)
        .io_kind(IoKind::from_error_kind(
            std::io::Error::from_raw_os_error(os_error).kind(),
        ))
    }

    pub(crate) fn validate(&self) -> Result<(), SanitizationError> {
        let lower = self.text.to_ascii_lowercase();
        if self.text.is_empty()
            || self.text.len() > MAX_DIAGNOSTIC_DETAIL_MESSAGE_BYTES
            || self
                .text
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
            || self.declared_sensitivity < Sensitivity::Sensitive
            || [
                "authorization:",
                "bearer ",
                "password=",
                "password:",
                "secret=",
                "secret:",
                "token=",
                "token:",
            ]
            .iter()
            .any(|marker| lower.contains(marker))
        {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidLifecycleNativeDetail,
                "native_detail",
            ));
        }
        Ok(())
    }
}

/// A `foreign_*` link carrying third-party text verbatim.
fn foreign_text_outcome(
    code: ContractCode,
    raw_source: RawSource,
    text: &str,
    truncated: bool,
    sensitivity: Sensitivity,
) -> Outcome {
    let outcome = Outcome::new(code)
        .raw_source(raw_source)
        .raw_text(text)
        .raw_text_sensitivity(sensitivity);
    if truncated {
        outcome.raw_text_truncated(true)
    } else {
        outcome
    }
}

/// The `foreign_os_error` link of an OS error number alone: its text is the OS's own message.
pub fn os_error_outcome(os_error: i32) -> Outcome {
    let error = std::io::Error::from_raw_os_error(os_error);
    Outcome::new(ContractCode::ForeignOsError)
        .raw_source(RawSource::Os)
        .raw_text(error.to_string())
        .os_error(os_error)
        .io_kind(IoKind::from_error_kind(error.kind()))
}

/// One cause of a runtime lifecycle failure: which backend phase failed, and the resources it
/// left. Ledger revision 3 records the cause's code and native text as the failure outcome's
/// `caused_by` link ([`LifecycleCauseDraft::link`]) and its typed context as the record's
/// `cause`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleCauseDraft {
    vendor_stdio: Option<Box<VendorStdioFacts>>,
    phase: LifecycleFailurePhase,
    source: String,
    code: Code,
    severity: CleanupCauseSeverity,
    detail: Option<DiagnosticDetailDraft>,
    native_detail: Option<LifecycleNativeDetail>,
    last_native_detail: Option<LifecycleNativeDetail>,
    resource: Option<RuntimeResourceKind>,
    resource_phase: Option<RuntimeResourceClosePhase>,
    candidate_index: Option<u8>,
    native_instance: Option<i32>,
    quiescence: Option<ResourceQuiescence>,
    owner_disposition: Option<OwnerResourceDisposition>,
    observation_count: Option<u16>,
    dropped_count: Option<u16>,
}

impl LifecycleCauseDraft {
    pub fn with_vendor_stdio_facts(mut self, facts: Option<VendorStdioFacts>) -> Self {
        self.vendor_stdio = facts.map(Box::new);
        self
    }

    pub fn new(
        phase: LifecycleFailurePhase,
        source: impl Into<String>,
        code: impl Into<Code>,
        severity: CleanupCauseSeverity,
    ) -> Self {
        Self {
            phase,
            source: source.into(),
            code: code.into(),
            severity,
            vendor_stdio: None,
            detail: None,
            native_detail: None,
            last_native_detail: None,
            resource: None,
            resource_phase: None,
            candidate_index: None,
            native_instance: None,
            quiescence: None,
            owner_disposition: None,
            observation_count: None,
            dropped_count: None,
        }
    }
    pub fn with_detail(mut self, detail: Option<DiagnosticDetailDraft>) -> Self {
        self.detail = detail;
        self
    }
    pub fn with_native_detail(mut self, detail: LifecycleNativeDetail) -> Self {
        self.native_detail = Some(detail);
        self
    }
    pub fn with_last_native_detail(mut self, detail: Option<LifecycleNativeDetail>) -> Self {
        self.last_native_detail = detail;
        self
    }
    #[allow(clippy::too_many_arguments)]
    pub fn with_resource_context(
        mut self,
        resource: RuntimeResourceKind,
        resource_phase: RuntimeResourceClosePhase,
        candidate_index: Option<u8>,
        native_instance: Option<i32>,
        quiescence: ResourceQuiescence,
        owner_disposition: OwnerResourceDisposition,
        observation_count: u16,
        dropped_count: u16,
    ) -> Self {
        self.resource = Some(resource);
        self.resource_phase = Some(resource_phase);
        self.candidate_index = candidate_index;
        self.native_instance = native_instance;
        self.quiescence = Some(quiescence);
        self.owner_disposition = Some(owner_disposition);
        self.observation_count = Some(observation_count);
        self.dropped_count = Some(dropped_count);
        self
    }
    pub const fn code(&self) -> Code {
        self.code
    }
    pub const fn severity(&self) -> CleanupCauseSeverity {
        self.severity
    }
    pub const fn phase(&self) -> LifecycleFailurePhase {
        self.phase
    }
    pub const fn resource(&self) -> Option<RuntimeResourceKind> {
        self.resource
    }
    pub const fn native_detail(&self) -> Option<&LifecycleNativeDetail> {
        self.native_detail.as_ref()
    }

    /// The cause's own outcome: its code, with its native text (and the last native text it
    /// observed) as `foreign_other_error` links.
    pub fn link(&self) -> Outcome {
        let mut outcome = Outcome::new(self.code);
        if let Some(native) = &self.native_detail {
            outcome = outcome.caused_by(native.outcome());
        }
        if let Some(native) = &self.last_native_detail {
            outcome = outcome.link(CauseRelation::Related, native.outcome());
        }
        outcome
    }

    fn sanitize(self) -> Result<(Outcome, LifecycleCauseRecord), SanitizationError> {
        for native in [&self.native_detail, &self.last_native_detail]
            .into_iter()
            .flatten()
        {
            native.validate()?;
        }
        let link = self.link();
        let record = LifecycleCauseRecord {
            vendor_stdio: self.vendor_stdio,
            phase: self.phase,
            source: self.source,
            detail: self
                .detail
                .map(DiagnosticDetailDraft::sanitize)
                .transpose()?,
            resource: self.resource,
            resource_phase: self.resource_phase,
            candidate_index: self.candidate_index,
            native_instance: self.native_instance,
            quiescence: self.quiescence,
            owner_disposition: self.owner_disposition,
            observation_count: self.observation_count,
            dropped_count: self.dropped_count,
        };
        record.validate()?;
        Ok((link, record))
    }
}

/// The typed context of a lifecycle failure's cause; its code and text are the failure
/// outcome's `caused_by` link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleCauseRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    vendor_stdio: Option<Box<VendorStdioFacts>>,
    phase: LifecycleFailurePhase,
    source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    detail: Option<DiagnosticDetailRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resource: Option<RuntimeResourceKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resource_phase: Option<RuntimeResourceClosePhase>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    candidate_index: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    native_instance: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    quiescence: Option<ResourceQuiescence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    owner_disposition: Option<OwnerResourceDisposition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    observation_count: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dropped_count: Option<u16>,
}

impl LifecycleCauseRecord {
    pub fn vendor_stdio(&self) -> Option<&VendorStdioFacts> {
        self.vendor_stdio.as_deref()
    }
    pub const fn phase(&self) -> LifecycleFailurePhase {
        self.phase
    }
    pub fn source(&self) -> &str {
        &self.source
    }
    pub const fn detail(&self) -> Option<&DiagnosticDetailRecord> {
        self.detail.as_ref()
    }
    pub const fn resource(&self) -> Option<RuntimeResourceKind> {
        self.resource
    }
    pub const fn resource_phase(&self) -> Option<RuntimeResourceClosePhase> {
        self.resource_phase
    }
    pub const fn candidate_index(&self) -> Option<u8> {
        self.candidate_index
    }
    pub const fn native_instance(&self) -> Option<i32> {
        self.native_instance
    }
    pub const fn quiescence(&self) -> Option<ResourceQuiescence> {
        self.quiescence
    }
    pub const fn owner_disposition(&self) -> Option<OwnerResourceDisposition> {
        self.owner_disposition
    }
    pub const fn observation_count(&self) -> Option<u16> {
        self.observation_count
    }
    pub const fn dropped_count(&self) -> Option<u16> {
        self.dropped_count
    }
    fn validate(&self) -> Result<(), SanitizationError> {
        validate_diagnostic_detail_token(&self.source, "lifecycle_source")?;
        if let Some(facts) = &self.vendor_stdio {
            facts.validate()?;
            if self.phase != LifecycleFailurePhase::ResourceClose || self.resource.is_none() {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidVendorStdioCause,
                    "vendor_stdio",
                ));
            }
        }
        if let Some(detail) = &self.detail {
            detail.validate()?;
        }
        if self.resource.is_some() != self.resource_phase.is_some()
            || self.resource.is_some() != self.quiescence.is_some()
            || self.resource.is_some() != self.owner_disposition.is_some()
            || self.resource.is_some() != self.observation_count.is_some()
            || self.resource.is_some() != self.dropped_count.is_some()
        {
            return Err(SanitizationError::new(
                SanitizationCode::IncompleteResourceCloseContext,
                "lifecycle_cause",
            ));
        }
        Ok(())
    }
}

/// A runtime lifecycle failure (`runtime.failed`). Its producer builds the failure's outcome
/// (code, `operation`, `stage`); the draft attaches the cleanup, the cause, the OS error and
/// the native text as links, and the record keeps the typed context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeLifecycleFailureDraft {
    record: RuntimeLifecycleFailureRecord,
    outcome: Outcome,
    native_detail: Option<LifecycleNativeDetail>,
    raw_os_error: Option<i32>,
    primary_detail: Option<DiagnosticDetailDraft>,
    cleanup_cause: Option<CleanupCauseDraft>,
    cause: Option<LifecycleCauseDraft>,
}

impl RuntimeLifecycleFailureDraft {
    pub fn with_adb_recovery(mut self, recovery: Option<AdbTargetRecovery>) -> Self {
        self.record.adb_recovery = recovery.map(Box::new);
        self
    }

    /// `source` names the component that reported the failure (`runtime_host`,
    /// `runtime_client` or `actingd`); `outcome` is the failure itself.
    pub fn new(owner_epoch: OwnerEpoch, source: impl Into<String>, outcome: Outcome) -> Self {
        Self {
            record: RuntimeLifecycleFailureRecord {
                resource_dispositions: None,
                adb_recovery: None,
                owner_epoch,
                source: source.into(),
                entered_event_id: None,
                instance_id: None,
                primary_detail: None,
                cleanup_detail: None,
                cause: None,
                capacity: None,
                task_timing: None,
            },
            outcome,
            native_detail: None,
            raw_os_error: None,
            primary_detail: None,
            cleanup_cause: None,
            cause: None,
        }
    }
    pub fn with_resource_dispositions(mut self, groups: Option<Vec<ResourceDisposition>>) -> Self {
        self.record.resource_dispositions = groups;
        self
    }
    /// The Runtime error code a receipt for this failure projects: the outcome's value
    /// `runtime_code` (ledger revision 3, M378 section 8.1).
    pub fn with_runtime_code(mut self, code: Option<crate::RuntimeErrorCode>) -> Self {
        if let Some(code) = code {
            self.outcome = self.outcome.runtime_code(code);
        }
        self
    }
    pub fn with_entered_event_id(mut self, id: Option<EventId>) -> Self {
        self.record.entered_event_id = id;
        self
    }
    pub fn with_instance_id(mut self, id: Option<InstanceId>) -> Self {
        self.record.instance_id = id;
        self
    }
    pub fn with_primary_detail(mut self, detail: Option<DiagnosticDetailDraft>) -> Self {
        self.primary_detail = detail;
        self
    }
    pub fn with_native_detail(mut self, detail: Option<LifecycleNativeDetail>) -> Self {
        self.native_detail = detail;
        self
    }
    pub fn with_capacity(mut self, capacity: Option<crate::CapacityDecision>) -> Self {
        self.record.capacity = capacity;
        self
    }
    pub fn with_task_timing(mut self, timing: Option<Box<crate::TaskTimingObservations>>) -> Self {
        self.record.task_timing = timing;
        self
    }
    pub fn with_raw_os_error(mut self, code: Option<i32>) -> Self {
        self.raw_os_error = code;
        self
    }
    pub fn with_cleanup_cause(mut self, cause: Option<CleanupCauseDraft>) -> Self {
        self.cleanup_cause = cause;
        self
    }
    pub fn with_cause(mut self, cause: Option<LifecycleCauseDraft>) -> Self {
        self.cause = cause;
        self
    }
    /// The failure's outcome as it is so far (without the links the draft adds).
    pub fn outcome(&self) -> &Outcome {
        &self.outcome
    }
    fn sanitize(mut self) -> Result<(RuntimeLifecycleFailureRecord, Outcome), SanitizationError> {
        let mut outcome = self.outcome;
        if let Some(cause) = self.cause {
            let (link, record) = cause.sanitize()?;
            outcome = outcome.caused_by(link);
            self.record.cause = Some(record);
        }
        if let Some(os_error) = self.raw_os_error {
            outcome = outcome.caused_by(match &self.native_detail {
                Some(native) => native.os_outcome(os_error),
                None => os_error_outcome(os_error),
            });
        } else if let Some(native) = &self.native_detail {
            outcome = outcome.caused_by(native.outcome());
        }
        if let Some(native) = &self.native_detail {
            native.validate()?;
        }
        if let Some(cleanup) = self.cleanup_cause {
            let (link, detail) = cleanup.sanitize()?;
            outcome = outcome.link(CauseRelation::Cleanup, link);
            self.record.cleanup_detail = detail;
        }
        self.record.primary_detail = self
            .primary_detail
            .map(DiagnosticDetailDraft::sanitize)
            .transpose()?;
        self.record.validate()?;
        Ok((self.record, outcome))
    }
}

/// The typed context of a `runtime.failed` record; the failure itself is the payload's
/// `outcome` (ledger revision 3), whose values carry the receipt's `runtime_code`. `source`
/// stays a member: of the reporters it names (`runtime_host`, `runtime_client`, `actingd`)
/// only the first has an `origin_module` token (M378 section 8.1; acceptance CL-M3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeLifecycleFailureRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resource_dispositions: Option<Vec<ResourceDisposition>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    task_timing: Option<Box<crate::TaskTimingObservations>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capacity: Option<crate::CapacityDecision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    adb_recovery: Option<Box<AdbTargetRecovery>>,
    owner_epoch: OwnerEpoch,
    source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    entered_event_id: Option<EventId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    instance_id: Option<InstanceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    primary_detail: Option<DiagnosticDetailRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cleanup_detail: Option<DiagnosticDetailRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cause: Option<LifecycleCauseRecord>,
}

impl RuntimeLifecycleFailureRecord {
    pub fn resource_dispositions(&self) -> Option<&[ResourceDisposition]> {
        self.resource_dispositions.as_deref()
    }
    pub fn task_timing(&self) -> Option<&crate::TaskTimingObservations> {
        self.task_timing.as_deref()
    }
    pub const fn capacity(&self) -> Option<&crate::CapacityDecision> {
        self.capacity.as_ref()
    }
    pub fn adb_recovery(&self) -> Option<&AdbTargetRecovery> {
        self.adb_recovery.as_deref()
    }
    pub const fn owner_epoch(&self) -> OwnerEpoch {
        self.owner_epoch
    }
    pub fn source(&self) -> &str {
        &self.source
    }
    pub const fn entered_event_id(&self) -> Option<EventId> {
        self.entered_event_id
    }
    pub const fn instance_id(&self) -> Option<InstanceId> {
        self.instance_id
    }
    pub const fn primary_detail(&self) -> Option<&DiagnosticDetailRecord> {
        self.primary_detail.as_ref()
    }
    pub const fn cleanup_detail(&self) -> Option<&DiagnosticDetailRecord> {
        self.cleanup_detail.as_ref()
    }
    pub const fn cause(&self) -> Option<&LifecycleCauseRecord> {
        self.cause.as_ref()
    }
    fn validate(&self) -> Result<(), SanitizationError> {
        if let Some(groups) = &self.resource_dispositions {
            validate_resource_dispositions(groups)?;
        }
        if let Some(timing) = &self.task_timing {
            timing.validate()?;
        }
        if let Some(capacity) = &self.capacity {
            capacity.validate()?;
        }
        if let Some(recovery) = &self.adb_recovery {
            recovery.validate()?;
        }
        validate_diagnostic_detail_token(&self.source, "lifecycle_source")?;
        for detail in [&self.primary_detail, &self.cleanup_detail]
            .into_iter()
            .flatten()
        {
            detail.validate()?;
        }
        if let Some(cause) = &self.cause {
            cause.validate()?;
        }
        Ok(())
    }
    fn sensitivity(&self) -> Sensitivity {
        let mut sensitivity = Sensitivity::Internal;
        if self
            .capacity
            .as_ref()
            .is_some_and(|capacity| capacity.target_volume.is_some())
        {
            sensitivity = Sensitivity::Sensitive;
        }
        if self.adb_recovery.is_some() {
            sensitivity = Sensitivity::Sensitive;
        }
        // Workflow #383 register X13: foreign text on the ADB evidence's outcome links.
        if let Some(foreign) = self
            .adb_recovery
            .as_deref()
            .and_then(AdbTargetRecovery::foreign_text_sensitivity)
        {
            sensitivity = sensitivity.max(foreign);
        }
        if self
            .cause
            .as_ref()
            .and_then(LifecycleCauseRecord::vendor_stdio)
            .is_some()
        {
            sensitivity = Sensitivity::Sensitive;
        }
        sensitivity
    }
    fn public_summary(&self) -> Self {
        let mut result = self.clone();
        if let Some(capacity) = &mut result.capacity {
            capacity.target_volume = None;
        }
        result.primary_detail = None;
        result.cleanup_detail = None;
        result.adb_recovery = None;
        if let Some(cause) = &mut result.cause {
            cause.detail = None;
            cause.native_instance = None;
            cause.vendor_stdio = None;
        }
        result
    }
}

/// A stage: the form of a registered location, `^[a-z][a-z0-9]*(_[a-z0-9]+)+$` and at most
/// 64 bytes (Workflow #378 A2-P; the dotted producer stages are registered locations now).
fn validate_diagnostic_detail_stage(value: &str) -> Result<(), SanitizationError> {
    let segment_valid = |segment: &str| {
        !segment.is_empty()
            && segment
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    };
    if value.len() > MAX_DIAGNOSTIC_DETAIL_STAGE_BYTES
        || !value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase())
        || !value.contains('_')
        || !value.split('_').all(segment_valid)
    {
        Err(SanitizationError::new(
            SanitizationCode::InvalidDiagnosticDetailStage,
            "stage",
        ))
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticPayload {
    action: EventAction,
    diagnostic_code: DiagnosticCode,
    outcome: Outcome,
    audit: SanitizedAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutcomePayload {
    action: EventAction,
    effect_disposition: EffectDisposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    runtime_state: Option<Box<crate::RuntimeStateFact>>,
    /// Microseconds spent in the input backend call; only `input.committed` may carry it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    touch_response_us: Option<u64>,
    /// Workflow #389 (register U1): the channel of a command record's connection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    channel: Option<crate::RuntimeChannel>,
    /// Workflow #389 (register U1): the counter that handed a Lab request over, on `lab` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    via: Option<crate::ChannelVia>,
    audit: SanitizedAudit,
}

/// A failed or refused operation (`*.failed`, `*.rejected`). `diagnostic_code` classifies it
/// for ledger queries; `outcome` is the failure itself (ledger revision 3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticOutcomePayload {
    action: EventAction,
    diagnostic_code: DiagnosticCode,
    effect_disposition: EffectDisposition,
    outcome: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    detail: Option<DiagnosticDetailRecord>,
    // Boxed like the former `cleanup_cause`: inline it would make this payload the one large
    // variant of every family enum that holds it (clippy `large_enum_variant`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cleanup_detail: Option<Box<DiagnosticDetailRecord>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lifecycle_failure: Option<Box<RuntimeLifecycleFailureRecord>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    artifact_failure: Option<Box<ArtifactFailureRecord>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resource_declaration: Option<Box<crate::ResourceDeclarationRejection>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    runtime_state: Option<Box<crate::RuntimeStateFact>>,
    /// Workflow #389 (register U1): the channel of a command record's connection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    channel: Option<crate::RuntimeChannel>,
    /// Workflow #389 (register U1): the counter that handed a Lab request over, on `lab` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    via: Option<crate::ChannelVia>,
    audit: SanitizedAudit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactFailureStage {
    BeforePublication,
    CreatedRecord,
    PublishedVerification,
    VerifiedRecord,
}

/// One failed step of an artifact store operation with the store's native text. Ledger
/// revision 3 records it as an outcome ([`Self::outcome`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactFailureCause {
    /// The step's code with its `operation`: a registered code and location, or for a relayed
    /// spelling the catalog does not list `outcome_relay_unregistered` with the spelling as
    /// `raw_code` (`Outcome::relayed`).
    pub step: Outcome,
    pub native_detail: LifecycleNativeDetail,
    pub raw_os_error: Option<i32>,
}

impl ArtifactFailureCause {
    /// The step, with its native text as a `foreign_os_error` (with the OS error number) or
    /// `foreign_other_error` link.
    pub fn outcome(&self) -> Outcome {
        self.step.clone().caused_by(match self.raw_os_error {
            Some(os_error) => self.native_detail.os_outcome(os_error),
            None => self.native_detail.outcome(),
        })
    }
}

pub const MAX_ARTIFACT_FAILURE_SECONDARY_CAUSES: usize = 4;

/// An artifact store failure as its producer reports it: the primary step, at most four
/// secondary steps, and the attempted identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactFailureDraft {
    pub artifact_id: crate::ArtifactId,
    pub stage: ArtifactFailureStage,
    pub primary: ArtifactFailureCause,
    pub secondary: Vec<ArtifactFailureCause>,
    pub omitted_secondary_count: u64,
    pub capacity: Option<crate::CapacityDecision>,
}

impl ArtifactFailureDraft {
    /// The failure's outcome: the primary step on top and each secondary step as a
    /// `secondary` link.
    pub fn outcome(&self) -> Outcome {
        self.secondary
            .iter()
            .fold(self.primary.outcome(), |outcome, cause| {
                outcome.link(CauseRelation::Secondary, cause.outcome())
            })
    }

    fn sanitize(self) -> Result<(ArtifactFailureRecord, Outcome), SanitizationError> {
        if self.secondary.len() > MAX_ARTIFACT_FAILURE_SECONDARY_CAUSES {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidArtifactFailureCauses,
                "secondary",
            ));
        }
        for cause in std::iter::once(&self.primary).chain(&self.secondary) {
            cause.native_detail.validate()?;
        }
        let outcome = self.outcome();
        let record = ArtifactFailureRecord {
            artifact_id: self.artifact_id,
            stage: self.stage,
            omitted_secondary_count: self.omitted_secondary_count,
            capacity: self.capacity,
        };
        record.validate()?;
        Ok((record, outcome))
    }
}

/// An attempted identity is diagnostic data, never a readable artifact attachment. The steps
/// that failed are the payload's `outcome` (ledger revision 3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactFailureRecord {
    pub artifact_id: crate::ArtifactId,
    pub stage: ArtifactFailureStage,
    pub omitted_secondary_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity: Option<crate::CapacityDecision>,
}

impl ArtifactFailureRecord {
    fn validate(&self) -> Result<(), SanitizationError> {
        if let Some(capacity) = &self.capacity {
            capacity.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeLifecyclePhase {
    BackendOpenObserved,
    PriorEpochOwnerImported,
    PriorEpochScopeClosed,
    /// Startup took over the previous owner whose last journal record still declared
    /// resources `in_use` or `unconfirmed`, because its process (`pid`, started at
    /// `started_at_unix_ms`) had exited (slice #315-B2c-2). Owner release only: it closes
    /// no scope and authorises nothing on the device side.
    PriorEpochOwnerReleasedByExit {
        pid: u32,
        started_at_unix_ms: u64,
        last_disposition: OwnerResourceDisposition,
    },
    VendorStdioClose {
        instance_id: Option<InstanceId>,
    },
    AdbTargetRecovery,
    /// Per-occurrence device diagnostic detail, decoded from older ledgers; no longer
    /// produced since Workflow #328.
    DeviceDiagnosticDetail,
    DeviceDiagnosticSummary,
    PolicyForwardEntered,
    PolicyForwardReturned {
        entered_event_id: EventId,
    },
    StrategicReportEntered,
    StrategicReportReturned {
        entered_event_id: EventId,
    },
    ShutdownRequested,
    ShutdownRequest {
        target: crate::RuntimeShutdownTarget,
        decision: crate::RuntimeShutdownDecision,
    },
    ResourceQuiescence {
        instance_id: InstanceId,
        resource_count: u16,
        quiescence: ResourceQuiescence,
        owner_disposition: OwnerResourceDisposition,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resource_dispositions: Option<Vec<ResourceDisposition>>,
    },
    /// Emulator instance control handed the instance's configured startup package to the
    /// host's own scheduling point (slice #316-B3); the run itself records `task.*` events
    /// under the same causation id.
    StartupPackageScheduled {
        instance_id: InstanceId,
    },
    /// Stuck-recovery ladder (slice #316-B4), linked to the instance and the trigger run's
    /// correlation id: the ladder was started for the trigger, with its rung plan in the
    /// configured order (`pending` or `skipped` with a reason). Workflow #387 D1c (M387 §4.7,
    /// §6.1): `config_digest` is the SHA-256 of the `recovery_ladder` configuration the ladder
    /// climbs by, taken when it was admitted; `profile` names the ladder profile whose rungs it
    /// climbs (M398 §6.4), absent while only the section's own `rungs` exist.
    RecoveryLadderStarted {
        trigger: RecoveryLadderTrigger,
        rungs: Vec<RecoveryRungPlan>,
        config_digest: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile: Option<String>,
    },
    /// One rung ended; `run_id` names the rung's own contained task run when it ran one.
    /// A failed or skipped rung's `outcome` is why (ledger revision 3; was `reason`).
    RecoveryRungFinished {
        rung: RecoveryRung,
        rung_result: RecoveryRungOutcome,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<RunId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outcome: Option<Outcome>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        environment: Option<crate::TerminalEvent>,
    },
    /// Actual preparation results after safe resource disposition and lease release.
    /// Missing channel reports remain unknown, independently of a failed other channel.
    InstancePreparationFinished {
        stage: RecoveryTriggerStage,
        capture: crate::BackendObservationStatus,
        input: crate::BackendObservationStatus,
        /// The preparation's failure (ledger revision 3; was `failure_code`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outcome: Option<Outcome>,
    },
    /// Confirmed provider Stop, new Start and successful fresh input/capture preparation.
    RecoveryEnvironmentReady {
        stop: crate::TerminalEvent,
        start: crate::TerminalEvent,
        preparation: crate::TerminalEvent,
    },
    /// The provider observed this instance's old process gone; any following Start shares
    /// these ladder links and must still pass admission/fencing.
    RecoveryInstanceStopped {
        stop: crate::TerminalEvent,
    },
    /// The ladder ended; `rungs_tried` counts the rungs that were executed (not skipped).
    /// An `exhausted` climb carries why as its `outcome` (ledger revision 3): its category is
    /// the record's severity, so a climb its rungs failed on their own ends at `error` and one
    /// a holding interruption ended ends at `warning` (review L6, #666).
    RecoveryLadderFinished {
        ladder_result: RecoveryLadderOutcome,
        rungs_tried: u8,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outcome: Option<Outcome>,
    },
    /// A trigger inside the cool-down window, or while a ladder runs, started nothing.
    /// Workflow #387 D1c (M387 §4.5): `rule` names the trigger item that matched, absent for a
    /// resource fence's ladder.
    RecoveryLadderSuppressed {
        reason: RecoveryLadderSuppression,
        until_unix_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rule: Option<String>,
    },
    /// Startup recovery of the runtime fact store (Workflow #308 5d-1) passed over an
    /// incomplete `runtime.fact_snapshot` set: `parts_found` of its `parts` were in the ledger.
    FactSnapshotSetSkipped {
        snapshot_id: u64,
        parts_found: u16,
        parts: u16,
    },
    /// Workflow #403: the resource fence of `instance_id` was lifted because the preparation
    /// `preparation` (its `instance_preparation_finished`) passed its self-check after the
    /// instance's unconfirmed close. `outcome` is the lift's code with the instance.
    ResourceFenceLifted {
        instance_id: InstanceId,
        preparation: crate::TerminalEvent,
        outcome: Outcome,
    },
    /// Workflow #373 NAV-6 (model M-373 v1.1 §8.4): a standard-pack game root was read, at
    /// startup or when a run's preparation found its registered packages changed. `outcome` is
    /// `standard_pack_registered` (info), `standard_pack_root_invalid` or
    /// `standard_pack_root_unreadable` (error); a broken root never stops the Runtime.
    StandardPackRead {
        outcome: Outcome,
    },
    /// Workflow #398 RD-2 (model-398 v1.1 §4.2, §4.3, §4.7): the wired device link of a handset
    /// changed, or the device-link watch did. `outcome` is `device_link_waiting` (warning; the
    /// handset left `device`, linked to its instance), `device_link_restored` (info; it is back
    /// and its preparation passed, linked to its instance), `device_watch_restarted` (warning)
    /// or `device_watch_failed` (error; every handset is then `unwatched`), the last two with
    /// system links. No value names a device serial: a handset is its label `handset:<alias>`.
    DeviceLinkObserved {
        outcome: Outcome,
    },
    /// Workflow #398 RD-5b (model-398 v1.1 §7.2–§7.6): the device state of a handset, observed
    /// by its sampling, its start gate or the check before an input, linked to its instance.
    /// `outcome` is `device_thermal_hold` or `device_battery_low` (warning; a sample found the
    /// handset held), `device_hold_lifted` (info; a hold lifted), `device_screen_woken` (info;
    /// the wake key lit the screen) or `device_state_unreadable` (error; a read failed or
    /// answered an unknown shape, once per failing streak of the sampling). No value names a
    /// device serial: a handset is its label `handset:<alias>`.
    DeviceStateObserved {
        outcome: Outcome,
    },
    /// Workflow #383 (register L5): the stores and the decision memory were loaded from the
    /// tagged records and opened new lineages; the base record of each new lineage, one entry
    /// per store. Register X4: `outcome` is the load's report (`ledger_tagged_state_loaded`,
    /// with a `ledger_tagged_entries_dropped` link; AO-6), an info outcome.
    TaggedStateLoaded {
        stores: Vec<TaggedStateStore>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outcome: Option<Outcome>,
    },
    /// Workflow #383 (register L6): tagged records are enabled on this state root. Register
    /// X4: `outcome` is the seeding's report (`ledger_tagged_state_seeded`; AO-5), an info
    /// outcome.
    TaggedStateEnabled {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outcome: Option<Outcome>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeLifecyclePayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    backend_open: Option<Box<crate::BackendOpenReport>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prior_epoch_close: Option<Box<crate::PriorEpochCloseFact>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    vendor_stdio: Option<Box<VendorStdioFacts>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    adb_recovery: Option<Box<AdbTargetRecovery>>,
    action: EventAction,
    owner_epoch: OwnerEpoch,
    phase: RuntimeLifecyclePhase,
    /// Workflow #390 DRILL-1: the record is a drill's own ladder phase (a person's
    /// `DrillRecoveryLadder`), recorded as a ladder a trigger started records it; absent on
    /// every other record.
    #[serde(default, skip_serializing_if = "is_false")]
    drill: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    device_diagnostics: Option<Box<DeviceDiagnosticBudgetRecord>>,
    audit: SanitizedAudit,
}

impl RuntimeLifecyclePayload {
    /// Workflow #390 DRILL-1: whether the record is tagged `drill`.
    pub const fn drill(&self) -> bool {
        self.drill
    }

    /// The outcome the phase carries (ledger revision 3): why a recovery rung failed or was
    /// skipped, why a recovery ladder ended exhausted, or why an instance preparation failed;
    /// and the report of a tagged state load or seeding (Workflow #383 register X4).
    pub fn outcome(&self) -> Option<&Outcome> {
        match &self.phase {
            RuntimeLifecyclePhase::RecoveryRungFinished { outcome, .. }
            | RuntimeLifecyclePhase::RecoveryLadderFinished { outcome, .. }
            | RuntimeLifecyclePhase::InstancePreparationFinished { outcome, .. }
            | RuntimeLifecyclePhase::TaggedStateLoaded { outcome, .. }
            | RuntimeLifecyclePhase::TaggedStateEnabled { outcome } => outcome.as_ref(),
            RuntimeLifecyclePhase::ResourceFenceLifted { outcome, .. } => Some(outcome),
            RuntimeLifecyclePhase::StandardPackRead { outcome }
            | RuntimeLifecyclePhase::DeviceLinkObserved { outcome }
            | RuntimeLifecyclePhase::DeviceStateObserved { outcome } => Some(outcome),
            _ => None,
        }
    }

    pub fn backend_open(&self) -> Option<&crate::BackendOpenReport> {
        self.backend_open.as_deref()
    }

    pub fn prior_epoch_close(&self) -> Option<&crate::PriorEpochCloseFact> {
        self.prior_epoch_close.as_deref()
    }
    pub fn vendor_stdio(&self) -> Option<&VendorStdioFacts> {
        self.vendor_stdio.as_deref()
    }

    pub fn adb_recovery(&self) -> Option<&AdbTargetRecovery> {
        self.adb_recovery.as_deref()
    }
    pub fn device_diagnostics(&self) -> Option<&DeviceDiagnosticBudgetRecord> {
        self.device_diagnostics.as_deref()
    }
    pub const fn owner_epoch(&self) -> OwnerEpoch {
        self.owner_epoch
    }

    pub fn phase(&self) -> RuntimeLifecyclePhase {
        self.phase.clone()
    }
}

/// How the Runtime obtained the recorded instance binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceBindingSource {
    Explicit,
    Discovered,
}

pub const MAX_DISCOVERED_INSTANCE_NAME_BYTES: usize = 256;
/// Same bound as `EmulatorVersionEvidence` provider versions.
pub const MAX_PROVIDER_VERSION_BYTES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeInstanceBindingPayload {
    action: EventAction,
    instance_alias: String,
    provenance: crate::ExecutionBackendProvenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    adb_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    adb_port: Option<u16>,
    serial_configured: bool,
    binding_source: InstanceBindingSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    discovered_instance_index: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    discovered_instance_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider_version: Option<String>,
    /// Workflow #398 RD-1b (model-398 v1.1 §3.4): the device of a handset instance; absent for
    /// an emulator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    device: Option<RuntimeInstanceDeviceBinding>,
    audit: SanitizedAudit,
}

/// Workflow #398 RD-1b (model-398 v1.1 §3.4): the device a handset instance drives. The
/// device's identity is the fingerprint of its serial (the ledger's salted SHA-256, field
/// `device_serial`), never the serial itself; the fingerprint, the binding time and the channel
/// that wrote the binding are present together, and absent while the private binding file has no
/// binding for the instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeInstanceDeviceBinding {
    kind: crate::InstanceDeviceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fingerprint: Option<Sha256Fingerprint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bound_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bound_via: Option<crate::DeviceBoundVia>,
}

impl RuntimeInstanceDeviceBinding {
    pub const fn kind(&self) -> crate::InstanceDeviceKind {
        self.kind
    }

    /// The device's identity: the fingerprint of its serial.
    pub fn fingerprint(&self) -> Option<&Sha256Fingerprint> {
        self.fingerprint.as_ref()
    }

    pub const fn bound_at_unix_ms(&self) -> Option<u64> {
        self.bound_at_unix_ms
    }

    pub const fn bound_via(&self) -> Option<crate::DeviceBoundVia> {
        self.bound_via
    }
}

/// Workflow #398 RD-1b: the device of a handset instance as the Runtime holds it; sanitization
/// keeps only the serial's fingerprint.
pub struct InstanceDeviceBindingDraft {
    kind: crate::InstanceDeviceKind,
    bound: Option<(String, u64, crate::DeviceBoundVia)>,
}

impl InstanceDeviceBindingDraft {
    /// A handset bound to the device `serial` at `bound_at_unix_ms`, by `bound_via`.
    pub fn handset(
        serial: impl Into<String>,
        bound_at_unix_ms: u64,
        bound_via: crate::DeviceBoundVia,
    ) -> Self {
        Self {
            kind: crate::InstanceDeviceKind::Handset,
            bound: Some((serial.into(), bound_at_unix_ms, bound_via)),
        }
    }

    /// A handset the private binding file has no binding for.
    pub const fn unbound_handset() -> Self {
        Self {
            kind: crate::InstanceDeviceKind::Handset,
            bound: None,
        }
    }

    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<RuntimeInstanceDeviceBinding, SanitizationError> {
        let Some((serial, bound_at_unix_ms, bound_via)) = self.bound else {
            return Ok(RuntimeInstanceDeviceBinding {
                kind: self.kind,
                fingerprint: None,
                bound_at_unix_ms: None,
                bound_via: None,
            });
        };
        let fingerprint = fingerprinter
            .fingerprint(SecretField::DeviceSerial, &serial)
            .map_err(|_| {
                SanitizationError::new(SanitizationCode::FingerprinterFailed, "device_serial")
            })?;
        fingerprint.validate_for(&serial)?;
        Ok(RuntimeInstanceDeviceBinding {
            kind: self.kind,
            fingerprint: Some(fingerprint),
            bound_at_unix_ms: Some(bound_at_unix_ms),
            bound_via: Some(bound_via),
        })
    }
}

impl fmt::Debug for InstanceDeviceBindingDraft {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InstanceDeviceBindingDraft")
            .field("kind", &self.kind)
            .field("bound", &self.bound.is_some())
            .finish()
    }
}

impl RuntimeInstanceBindingPayload {
    pub fn instance_alias(&self) -> &str {
        &self.instance_alias
    }

    pub const fn provenance(&self) -> crate::ExecutionBackendProvenance {
        self.provenance
    }

    pub fn adb_host(&self) -> Option<&str> {
        self.adb_host.as_deref()
    }

    pub const fn adb_port(&self) -> Option<u16> {
        self.adb_port
    }

    pub const fn serial_configured(&self) -> bool {
        self.serial_configured
    }

    pub const fn binding_source(&self) -> InstanceBindingSource {
        self.binding_source
    }

    pub const fn discovered_instance_index(&self) -> Option<u16> {
        self.discovered_instance_index
    }

    pub fn discovered_instance_name(&self) -> Option<&str> {
        self.discovered_instance_name.as_deref()
    }

    pub fn provider_version(&self) -> Option<&str> {
        self.provider_version.as_deref()
    }

    /// Workflow #398 RD-1b: the device of a handset instance; `None` for an emulator.
    pub fn device(&self) -> Option<&RuntimeInstanceDeviceBinding> {
        self.device.as_ref()
    }
}

/// One runtime fact accepted into the Runtime's own fact store (Workflow #313).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeFactRecordedPayload {
    action: EventAction,
    record: crate::RuntimeFactRecord,
    audit: SanitizedAudit,
}

impl RuntimeFactRecordedPayload {
    pub const fn record(&self) -> &crate::RuntimeFactRecord {
        &self.record
    }
}

/// One runtime fact dropped from the Runtime's own fact store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeFactInvalidatedPayload {
    action: EventAction,
    invalidation: crate::RuntimeFactInvalidation,
    audit: SanitizedAudit,
}

impl RuntimeFactInvalidatedPayload {
    pub const fn invalidation(&self) -> &crate::RuntimeFactInvalidation {
        &self.invalidation
    }
}

/// The sealed image of the Runtime's own fact store at one ledger position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeFactSnapshotPayload {
    action: EventAction,
    snapshot: Box<crate::RuntimeFactSnapshot>,
    audit: SanitizedAudit,
}

impl RuntimeFactSnapshotPayload {
    pub fn snapshot(&self) -> &crate::RuntimeFactSnapshot {
        &self.snapshot
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MonitorOutcomePayload {
    action: EventAction,
    effect_disposition: EffectDisposition,
    observation: MonitorObservation,
    decision: MonitorDecision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    runtime_state: Option<Box<crate::RuntimeStateFact>>,
    audit: SanitizedAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MonitorRecoveryCoordinationPayload {
    action: EventAction,
    effect_disposition: EffectDisposition,
    recovery: MonitorRecoveryKind,
    reason: MonitorRecoveryCoordinationReason,
    audit: SanitizedAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformancePressurePayload {
    action: EventAction,
    observed_at_unix_ms: u64,
    pressure: PerformancePressureRecord,
    audit: SanitizedAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceStutterPayload {
    action: EventAction,
    instance_id: String,
    observed_at_unix_ms: u64,
    frame_gap_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    capture_latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recognition_latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    action_effect_latency_ms: Option<u64>,
    audit: SanitizedAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceSummaryPayload {
    action: EventAction,
    context: PerformanceContext,
    #[serde(skip_serializing_if = "Option::is_none")]
    foreground: Option<crate::PerformanceForegroundSummary>,
    owned_processes: Vec<crate::PerformanceProcessSummary>,
    third_party_high_load: Vec<crate::PerformanceProcessSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ledger_commits: Option<crate::PerformanceLedgerSample>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capacity: Option<crate::PerformanceCapacitySample>,
    /// Workflow #385 (register P4): the pacing members, present together from P-m on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pacing: Option<Box<PerformancePacingSummary>>,
    audit: SanitizedAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceMonitorStatePayload {
    action: EventAction,
    observed_at_unix_ms: u64,
    health: PerformanceMonitorHealth,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    outcome: Option<Outcome>,
    consecutive_failures: u16,
    terminal: bool,
    unavailable_metrics: Vec<crate::PerformanceMetric>,
    audit: SanitizedAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceControlPayload {
    action: EventAction,
    observed_at_unix_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    instance_id: Option<String>,
    previous_level: PerformanceControlLevel,
    level: PerformanceControlLevel,
    reason: PerformanceControlReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    host_responsiveness_basis_points: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    third_party_pressure_basis_points: Option<u16>,
    recovery: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    deadline_disposition: Option<PerformanceDeadlineDisposition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    arbitration: Option<ArbitrationBasis>,
    audit: SanitizedAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactPublishedPayload {
    action: EventAction,
    record: Box<FactRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    related_records: Vec<FactRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    scope_instances: Vec<crate::InstanceId>,
    audit: SanitizedAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactInvalidatedPayload {
    action: EventAction,
    invalidation: FactInvalidationEventData,
    audit: SanitizedAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientActionPayload {
    action: EventAction,
    record: ClientActionRecord,
    audit: SanitizedAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalDecisionPayload {
    action: EventAction,
    decision: ApprovalDecisionRecord,
    audit: SanitizedAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchedulerQueuePayload {
    action: EventAction,
    priority: LeasePriority,
    position: u32,
    deadline_monotonic_ms: u64,
    preempt_requested: bool,
    audit: SanitizedAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchedulerPreemptionPayload {
    action: EventAction,
    from_holder_id: HolderId,
    from_lease_id: LeaseId,
    queued_request_id: RequestId,
    queued_priority: LeasePriority,
    deferred_by_destructive_step: bool,
    audit: SanitizedAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseTransferPayload {
    action: EventAction,
    effect_disposition: EffectDisposition,
    from_holder_id: HolderId,
    from_lease_id: LeaseId,
    to_holder_id: HolderId,
    to_lease_id: LeaseId,
    queued_request_id: RequestId,
    priority: LeasePriority,
    audit: SanitizedAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationResultPayload {
    action: EventAction,
    effect_disposition: EffectDisposition,
    frame_width: u32,
    frame_height: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    recognition_verdict: Option<RecognitionVerdict>,
    /// Microseconds spent acquiring the frame from the backend; only `capture.completed` may carry it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capture_acquire_us: Option<u64>,
    audit: SanitizedAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapturePressurePayload {
    action: EventAction,
    state: CapturePressureState,
    memory_budget_bytes: u64,
    resident_bytes: u64,
    audit: SanitizedAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureDedupWindowPayload {
    action: EventAction,
    duplicate_count: u64,
    duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    preserved_frame_id: Option<crate::FrameId>,
    audit: SanitizedAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapturePolicyPayload {
    action: EventAction,
    cadence_ms: u64,
    retention_class: RetentionClass,
    reason: CapturePolicyReason,
    audit: SanitizedAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapturePersistedEvidence {
    frame_index: u64,
    artifact: ProjectedArtifactReference,
}

impl CapturePersistedEvidence {
    pub fn new(
        frame_index: u64,
        artifact: ProjectedArtifactReference,
    ) -> Result<Self, SanitizationError> {
        let value = Self {
            frame_index,
            artifact,
        };
        value.validate()?;
        Ok(value)
    }

    pub const fn frame_index(&self) -> u64 {
        self.frame_index
    }

    pub const fn artifact(&self) -> &ProjectedArtifactReference {
        &self.artifact
    }

    fn validate(&self) -> Result<(), SanitizationError> {
        if self.artifact.kind != ArtifactKind::CaptureFrame
            || self.artifact.run_id.is_none()
            || self.artifact.correlation_id.is_none()
            || self.artifact.frame_id.is_none()
            || self.artifact.object_key.is_none()
            || self.artifact.validate().is_err()
        {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidCaptureSummaryArtifact,
                "frames",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapturePinnedEvidence {
    #[serde(skip_serializing_if = "Option::is_none")]
    frame_index: Option<u64>,
    reason: PinnedFrameReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    artifact: Option<ProjectedArtifactReference>,
}

impl CapturePinnedEvidence {
    pub fn new(
        frame_index: Option<u64>,
        reason: PinnedFrameReason,
        artifact: Option<ProjectedArtifactReference>,
    ) -> Result<Self, SanitizationError> {
        let value = Self {
            frame_index,
            reason,
            artifact,
        };
        value.validate_shape()?;
        Ok(value)
    }

    pub const fn frame_index(&self) -> Option<u64> {
        self.frame_index
    }

    pub const fn reason(&self) -> PinnedFrameReason {
        self.reason
    }

    pub const fn artifact(&self) -> Option<&ProjectedArtifactReference> {
        self.artifact.as_ref()
    }

    fn validate_shape(&self) -> Result<(), SanitizationError> {
        if self.artifact.is_some() && self.frame_index.is_none() {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidCaptureSummaryPin,
                "pinned",
            ));
        }
        if let Some(artifact) = &self.artifact {
            CapturePersistedEvidence {
                frame_index: self.frame_index.expect("checked above"),
                artifact: artifact.clone(),
            }
            .validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureSummaryRecord {
    captured: u64,
    deduplicated: u64,
    dropped: u64,
    persisted: u64,
    evidence_completeness: EvidenceCompleteness,
    frames: Vec<CapturePersistedEvidence>,
    pinned: Vec<CapturePinnedEvidence>,
}

impl CaptureSummaryRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        captured: u64,
        deduplicated: u64,
        dropped: u64,
        persisted: u64,
        evidence_completeness: EvidenceCompleteness,
        frames: Vec<CapturePersistedEvidence>,
        pinned: Vec<CapturePinnedEvidence>,
    ) -> Result<Self, SanitizationError> {
        let value = Self {
            captured,
            deduplicated,
            dropped,
            persisted,
            evidence_completeness,
            frames,
            pinned,
        };
        value.validate()?;
        Ok(value)
    }

    pub const fn captured(&self) -> u64 {
        self.captured
    }

    pub const fn deduplicated(&self) -> u64 {
        self.deduplicated
    }

    pub const fn dropped(&self) -> u64 {
        self.dropped
    }

    pub const fn persisted(&self) -> u64 {
        self.persisted
    }

    pub const fn evidence_completeness(&self) -> EvidenceCompleteness {
        self.evidence_completeness
    }

    pub fn frames(&self) -> &[CapturePersistedEvidence] {
        &self.frames
    }

    pub fn pinned(&self) -> &[CapturePinnedEvidence] {
        &self.pinned
    }

    pub fn validate(&self) -> Result<(), SanitizationError> {
        if [
            self.captured,
            self.deduplicated,
            self.dropped,
            self.persisted,
        ]
        .into_iter()
        .any(|count| count > MAX_CAPTURE_SUMMARY_COUNT)
            || self.frames.len() > MAX_CAPTURE_SUMMARY_FRAMES
            || self.pinned.len() > MAX_CAPTURE_SUMMARY_PINS
        {
            return Err(SanitizationError::new(
                SharedCode::CaptureSummaryBoundExceeded,
                "summary",
            ));
        }
        if u64::try_from(self.frames.len()).ok() != Some(self.persisted) {
            return Err(SanitizationError::new(
                SanitizationCode::CaptureSummaryCountMismatch,
                "persisted",
            ));
        }
        let accounted = self
            .deduplicated
            .checked_add(self.dropped)
            .and_then(|count| count.checked_add(self.persisted))
            .ok_or_else(|| {
                SanitizationError::new(SharedCode::CaptureSummaryCountOverflow, "summary")
            })?;
        if accounted > self.captured {
            return Err(SanitizationError::new(
                SanitizationCode::CaptureSummaryCountMismatch,
                "captured",
            ));
        }

        let mut previous_frame_index = None;
        let mut artifact_ids = BTreeSet::new();
        let mut frame_ids = BTreeSet::new();
        let mut object_keys = BTreeSet::new();
        for frame in &self.frames {
            frame.validate()?;
            if frame.frame_index > MAX_CAPTURE_SUMMARY_COUNT
                || previous_frame_index.is_some_and(|previous| previous >= frame.frame_index)
                || !artifact_ids.insert(frame.artifact.artifact_id)
                || !frame_ids.insert(
                    frame
                        .artifact
                        .frame_id
                        .expect("validated capture artifact has a frame id"),
                )
                || !object_keys.insert(
                    frame
                        .artifact
                        .object_key
                        .as_deref()
                        .expect("validated capture artifact has an object key"),
                )
            {
                return Err(SanitizationError::new(
                    SharedCode::CaptureSummaryFrameConflict,
                    "frames",
                ));
            }
            previous_frame_index = Some(frame.frame_index);
        }

        let mut previous_pin = None;
        let mut missing_pin = false;
        for pin in &self.pinned {
            pin.validate_shape()?;
            let key = (pin.frame_index, pin.reason);
            if previous_pin.is_some_and(|previous| previous >= key) {
                return Err(SanitizationError::new(
                    SharedCode::CaptureSummaryPinConflict,
                    "pinned",
                ));
            }
            if let Some(frame_index) = pin.frame_index {
                if frame_index > MAX_CAPTURE_SUMMARY_COUNT {
                    return Err(SanitizationError::new(
                        SharedCode::CaptureSummaryPinConflict,
                        "pinned",
                    ));
                }
                if let Some(artifact) = &pin.artifact {
                    let persisted = self
                        .frames
                        .binary_search_by_key(&frame_index, |frame| frame.frame_index)
                        .ok()
                        .map(|index| &self.frames[index].artifact);
                    if persisted != Some(artifact) {
                        return Err(SanitizationError::new(
                            SanitizationCode::CaptureSummaryPinArtifactMismatch,
                            "pinned",
                        ));
                    }
                } else {
                    missing_pin = true;
                }
            } else {
                missing_pin = true;
            }
            previous_pin = Some(key);
        }

        let expected = if missing_pin || accounted != self.captured {
            EvidenceCompleteness::Failed
        } else if self.dropped > 0 {
            EvidenceCompleteness::Partial
        } else {
            EvidenceCompleteness::Complete
        };
        if self.evidence_completeness != expected {
            return Err(SanitizationError::new(
                SanitizationCode::CaptureSummaryCompletenessMismatch,
                "evidence_completeness",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureSummaryCommittedPayload {
    action: EventAction,
    summary: CaptureSummaryRecord,
    audit: SanitizedAudit,
}

impl CaptureSummaryCommittedPayload {
    pub const fn summary(&self) -> &CaptureSummaryRecord {
        &self.summary
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactExportPayload {
    action: EventAction,
    effect_disposition: EffectDisposition,
    task_outcome: TaskOutcome,
    evidence_completeness: EvidenceCompleteness,
    artifact_count: u64,
    audit: SanitizedAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactExportFailurePayload {
    action: EventAction,
    diagnostic_code: DiagnosticCode,
    /// The failed export's outcome (ledger revision 3).
    outcome: Outcome,
    effect_disposition: EffectDisposition,
    task_outcome: TaskOutcome,
    evidence_completeness: EvidenceCompleteness,
    artifact_count: u64,
    audit: SanitizedAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceAuthoringPayload {
    phase: ResourceAuthoringPhase,
    draft_id: String,
    target_label: String,
    target_fingerprint: String,
    changed_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    outcome: Option<Outcome>,
    audit: SanitizedAudit,
}

/// One reason of a dispatch's ranking chain or of a task selection's explanation: the reason's
/// outcome, a registered code with its values (ledger revision 3, Workflow #378 A2-P; model
/// section 8.1). The evaluators' English detail goes only to the developer log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PolicyReasonRecord(pub Outcome);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyDispatchEventData {
    pub decision_id: String,
    pub task_id: String,
    pub instance_id: String,
    pub operation_id: String,
    pub procedure_binding_digest: String,
    pub reason_chain_id: String,
    pub reasons: Vec<PolicyReasonRecord>,
    pub catalog_hash: String,
    pub catalog_version: u64,
    pub input_ledger_position: u64,
    pub fact_snapshot_id: String,
    pub approval_fact_ids: Vec<String>,
    pub urgency_milli: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyActivitySample {
    pub profile_id: String,
    pub local_day: i64,
    pub window_id: String,
    pub admitted_at_unix_ms: u64,
    pub seed: u64,
    pub interval_ms: u64,
    pub next_eligible_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyBudgetReceipt {
    pub task_daily_used: u32,
    pub task_daily_limit: u32,
    pub task_window_used: u32,
    pub task_window_limit: u32,
    pub task_runtime_reserved_ms: u64,
    pub task_runtime_limit_ms: u64,
    pub activity_daily_used: u32,
    pub activity_daily_limit: u32,
    pub activity_window_used: u32,
    pub activity_window_limit: u32,
    pub activity_runtime_reserved_ms: u64,
    pub activity_runtime_limit_ms: u64,
}

impl PolicyBudgetReceipt {
    /// Whether a run of `runtime_ms` exceeds this admission's runtime budget: the reserved task
    /// (or activity) runtime less the dispatch's expected duration, plus the run's runtime,
    /// above the task (or activity) limit. `None` when the receipt reserves less than the
    /// expected duration or a total overflows. The policy's classification, the host's recovery
    /// check and the ledger's settlement check use this one rule (Workflow #369 E3); the
    /// ledger, which does not record the expected duration, passes 0 and so checks the bound
    /// every rewrite meets.
    pub fn runtime_exceeded(&self, expected_duration_ms: u64, runtime_ms: u64) -> Option<bool> {
        let task = self
            .task_runtime_reserved_ms
            .checked_sub(expected_duration_ms)?
            .checked_add(runtime_ms)?;
        let activity = self
            .activity_runtime_reserved_ms
            .checked_sub(expected_duration_ms)?
            .checked_add(runtime_ms)?;
        Some(task > self.task_runtime_limit_ms || activity > self.activity_runtime_limit_ms)
    }
}

/// The failure code the policy records, in place of a run's own outcome, for a run that
/// exceeded its runtime budget (class Severe; `reported_success` when the run succeeded).
pub const POLICY_RUNTIME_BUDGET_EXCEEDED: crate::outcome::Code =
    crate::codes::SanitizationCode::PolicyRuntimeBudgetExceeded.code();

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyAdmissionRecord {
    pub activity: PolicyActivitySample,
    pub budget: PolicyBudgetReceipt,
}

/// A failed run as the policy records it (Workflow #378 A2-P). `outcome` is the run's failure:
/// its terminal outcome's top node ([`Outcome::without_causes`]), the top node of the request
/// error of a run with no terminal, or the policy's own [`POLICY_RUNTIME_BUDGET_EXCEEDED`] or
/// `policy_settlement_interrupted`; its category is the record's severity. `failure_identity`
/// is the #336 failure identity a live `linear_steps` settlement computes
/// (`contracts/policy-suspension.md`), and is absent for any other failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PolicyFailureRecord {
    pub outcome: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_identity: Option<String>,
    pub reported_success: bool,
    pub original_class: PolicyFailureClass,
    pub effective_class: PolicyFailureClass,
    pub consecutive_same_error: u16,
    #[serde(default)]
    pub escalation_streak: u16,
    #[serde(default)]
    pub performance_tax_exempt: bool,
    pub retry_attempt: u16,
    pub disposition: PolicyFailureDisposition,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_at_unix_ms: Option<u64>,
    pub runtime_ms: u64,
    pub sensitive: bool,
    #[serde(default = "legacy_performance_context")]
    pub perf_context: Box<PerformanceContext>,
    /// Workflow #386 (register S1): the run ended by a condition (SCHED §4.2): whether its
    /// counts were refunded, the pair's how-many-th condition termination in this occurrence and
    /// when the occurrence's accounting window ends. Absent for an ordinary failure; replay
    /// settles by it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<PolicyConditionSettlement>,
}

/// How a condition-terminated run settled (Workflow #386 SCHED §4.2, register S1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyConditionSettlement {
    pub refunded: bool,
    pub count: u32,
    pub expires_at_unix_ms: u64,
}

/// The policy class of a failed scheduled run from its task terminal (Workflow #378 A3, model
/// section 8.1; coordinator ruling 9, 2026-10-13): `Severe` when the failure point judged the
/// failure severe (`TerminalCommitted.failure_severe`) or its code is one the Runtime only ever
/// raises as fatal ([`is_severe_policy_failure`]), otherwise `Recoverable`. Before ledger
/// revision 3 the class was read from the terminal's severity (`fatal`); a severity is now its
/// record's category. The live settlement, the startup recovery and the ledger's re-derivation
/// all take the class from here, so they agree.
pub fn scheduled_failure_class(
    code: &crate::outcome::CodeStr,
    failure_severe: bool,
) -> PolicyFailureClass {
    if failure_severe || is_severe_policy_failure(code) {
        PolicyFailureClass::Severe
    } else {
        PolicyFailureClass::Recoverable
    }
}

/// The policy failure code of a failed run's outcome (Workflow #378 A3; coordinator ruling 7,
/// 2026-10-13): the outcome's code, except for a relayed spelling the embedded catalog does not
/// register (`outcome_relay_unregistered`), whose spelling the outcome keeps as `raw_code`, so
/// distinct failures keep distinct failure identities and streaks. The live settlement, the
/// startup recovery, the ledger's re-derivation and the recovery ladder's task trigger
/// (acceptance H-L6) all take it from here, and compare a recorded failure by it.
pub fn scheduled_failure_code_str(outcome: &Outcome) -> crate::outcome::CodeStr {
    if outcome.is(ContractCode::OutcomeRelayUnregistered)
        && let Some(spelling) = outcome
            .value("raw_code")
            .and_then(|value| crate::outcome::CodeStr::deserialize(value).ok())
    {
        return spelling;
    }
    outcome.code().clone()
}

/// Whether a failed scheduled run's code is one the Runtime only ever raises as fatal, so its
/// run is `Severe` whatever the failure point judged (Workflow #378 A3, model section 8.1): the
/// static list of the codes whose task terminals the Runtime recorded `fatal` at the model's
/// pin and that no error raises as nonfatal. B4 reviews the list with the policy.
pub fn is_severe_policy_failure(code: &crate::outcome::CodeStr) -> bool {
    severe_policy_failure_codes().contains(&code.as_str())
}

/// The spellings of [`is_severe_policy_failure`]: the codes the runtime host and the execution
/// kernel build only as fatal errors (`RuntimeHostError::fatal`, `relayed_fatal`,
/// `ExecutionKernelError::fatal`) and never as request or device-severity errors, in spelling
/// order. A code whose fatality the pin decided per error (a device error's severity, a store
/// error's flag) is not listed, so its failure stays `Recoverable`. Other owners' codes, as a
/// list B4 reviews.
fn severe_policy_failure_codes() -> &'static [&'static str] {
    &[
        "action_id_issue_failed",
        "agent_wake_instance_unknown",
        "agent_wake_issue_failed",
        "agent_wake_source_invalid",
        "agent_wake_trigger_invalid",
        "application_foreground_unknown",
        "application_lifecycle_action_id_missing",
        "application_lifecycle_durable_state_inconsistent",
        "application_lifecycle_result_invalid",
        "application_resource_close_required",
        "approval_replay_payload_mismatch",
        "backend_selfcheck_fact_overflow",
        "backend_selfcheck_instance_missing",
        "backend_selfcheck_instance_unregistered",
        "capture_backend_missing",
        "capture_sequence_result_invalid",
        "capture_sequence_terminal_missing",
        "catalog_failure_fact_undurable",
        "catalog_migration_source_missing",
        "catalog_plan_generation_mismatch",
        "client_action_replay_payload_mismatch",
        "client_fact_replay_ambiguous",
        "connection_preparation_cooldown_overflow",
        "contained_task_recovery_identity_invalid",
        "duplicate_emulator_capability_requirement",
        "duplicate_instance_alias",
        "duplicate_instance_id",
        "empty_execution_backend_registry",
        "emulator_capability_probe_failed",
        "emulator_capability_unavailable",
        "event_fingerprinter_failed",
        "event_id_issue_failed",
        "event_issuer_failed",
        "evidence_export_terminal_missing",
        "evidence_output_path_invalid",
        "execution_backend_provenance_mismatch",
        "execution_backend_selection_not_explicit",
        "execution_backend_target_mismatch",
        "execution_capability_profile_invalid",
        "execution_capability_profile_merge_invalid",
        "execution_configuration_timeout_overflow",
        "execution_endpoint_rebind_session_open",
        "execution_instance_identity_mismatch",
        "execution_instance_unknown",
        "execution_kernel_closed",
        "execution_kernel_state_poisoned",
        "execution_session_close_in_progress",
        "execution_session_closed",
        "execution_session_generation_exhausted",
        "execution_session_panicked",
        "execution_session_response_lost",
        "execution_session_spawn_failed",
        "execution_session_state_poisoned",
        "execution_session_unavailable",
        "fact_publication_sequence_missing",
        "fatal_state_poisoned",
        "fenced_step_not_reclaimed",
        "fixture_capture_outside_simulation_scope",
        "fixture_monitor_scope_forbidden",
        "governance_connection_state_conflict",
        "input_backend_missing",
        "input_execution_plan_invalid",
        "input_frame_capture_mismatch",
        "input_frame_not_committed",
        "input_frame_session_missing",
        "instance_worker_spawn_failed",
        "invalid_application_identity",
        "invalid_emulator_capability_requirement_count",
        "invalid_instance_alias",
        "ledger_failure",
        "ledger_migration_required",
        "maintenance_evidence_timestamp_missing",
        "material_read_reference_changed",
        "material_read_verification_mismatch",
        "monitor_committed_state_missing",
        "monitor_decision_invalid",
        "monitor_frame_identity_missing",
        "monitor_instance_unknown",
        "monitor_ledger_failure",
        "monitor_observation_invalid",
        "monitor_observation_unavailable",
        "monitor_probe_id_issue_failed",
        "monitor_recovery_fencing_state_invalid",
        "monitor_state_invalid",
        "monotonic_clock_overflow",
        "nemu_app_index_missing",
        "nemu_app_index_requires_paired_input",
        "nemu_application_identity_invalid",
        "nemu_close_instance_missing",
        "nemu_input_deadline_overflow",
        "nemu_input_fencing_required",
        "nemu_input_frame_source_missing",
        "nemu_input_requires_paired_capture",
        "nemu_input_task_deadline_missing",
        "nonfatal_error_marked_fatal",
        "owner_epoch_issue_failed",
        "owner_file_missing",
        "owner_file_open_failed",
        "owner_lock_failed",
        "owner_record_encode_failed",
        "owner_resource_unconfirmed",
        "owner_revision_overflow",
        "owner_seek_failed",
        "owner_sync_failed",
        "owner_unlock_failed",
        "owner_write_failed",
        "performance_pipeline_instance_missing",
        "performance_pipeline_instance_unknown",
        "policy_execution_identity_conflict",
        "policy_instance_seed_encode_failed",
        "policy_instance_seed_identity_conflict",
        "policy_planning_signal_identity_conflict",
        "policy_settlement_fact_overflow",
        "policy_settlement_instance_unknown",
        "ppocr_diagnostic_invalid",
        "ppocr_diagnostic_verified_event_missing",
        "ppocr_diagnostic_write_failed",
        "ppocr_original_error_encode_failed",
        "preparation_deadline_exceeded",
        "project_decision_count_overflow",
        "project_decision_page_missing",
        "project_interface_count_overflow",
        "project_snapshot_position_mismatch",
        "proposal_activation_mismatch",
        "proposal_authorization_invalid",
        "proposal_base_sources_unavailable",
        "proposal_promotion_invalid",
        "proposal_target_hash_missing",
        "readonly_capability_issue_failed",
        "readonly_observation_invalid",
        "recovery_readiness_adb_unconfirmed",
        "release_failure_fact_undurable",
        "resource_close_instance_missing",
        "resource_targets_record_invalid",
        "run_id_issue_failed",
        "runtime_component_already_prepared",
        "runtime_component_not_prepared",
        "runtime_debug_context_missing_after_terminal",
        "runtime_fact_replay_failed",
        "runtime_fact_store_desync",
        "runtime_instance_identity_mismatch",
        "runtime_instance_registry_incomplete",
        "runtime_restart_required",
        "runtime_status_projection_invalid",
        "runtime_status_queue_count_overflow",
        "runtime_task_terminal_missing",
        "safe_reset_action_id_missing",
        "safe_reset_durable_state_inconsistent",
        "safe_reset_input_result_invalid",
        "scheduling_pause_epoch_encode_failed",
        "scheduling_pause_fact_clock_overflow",
        "scheduling_pause_origin_missing",
        "scheduling_pause_state_invalid",
        "scheduling_pause_state_lost",
        "startup_package_claim_request_invalid",
        "startup_package_instance_unknown",
        "startup_package_requires_physical_instance",
        "strategic_report_encode_failed",
        "strategic_report_identity_conflict",
        "strategic_signal_identity_encode_failed",
        "stuck_recovery_instance_unknown",
        "synthetic_request_invalid",
        "system_clock_invalid",
        "system_clock_overflow",
        "task_id_issue_failed",
        "unknown_emulator_capability",
    ]
}

impl PolicyFailureRecord {
    /// Whether this failure is the policy's runtime-budget rewrite of a run that succeeded
    /// (`reported_success`) or failed: [`POLICY_RUNTIME_BUDGET_EXCEEDED`] with no failure
    /// identity, class Severe.
    pub fn is_runtime_budget_rewrite(&self, reported_success: bool) -> bool {
        self.outcome.is(POLICY_RUNTIME_BUDGET_EXCEEDED)
            && self.failure_identity.is_none()
            && self.original_class == PolicyFailureClass::Severe
            && self.reported_success == reported_success
    }
}

fn legacy_performance_context() -> Box<PerformanceContext> {
    Box::new(PerformanceContext::legacy_unavailable())
}

#[derive(Default)]
struct LegacyOptionalU16(Option<u16>);

impl<'de> Deserialize<'de> for LegacyOptionalU16 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        u16::deserialize(deserializer).map(|value| Self(Some(value)))
    }
}

impl<'de> Deserialize<'de> for PolicyFailureRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            outcome: Outcome,
            #[serde(default)]
            failure_identity: Option<String>,
            reported_success: bool,
            original_class: PolicyFailureClass,
            effective_class: PolicyFailureClass,
            consecutive_same_error: u16,
            #[serde(default)]
            escalation_streak: LegacyOptionalU16,
            #[serde(default)]
            performance_tax_exempt: bool,
            retry_attempt: u16,
            disposition: PolicyFailureDisposition,
            retry_at_unix_ms: Option<u64>,
            runtime_ms: u64,
            sensitive: bool,
            #[serde(default = "legacy_performance_context")]
            perf_context: Box<PerformanceContext>,
            #[serde(default)]
            condition: Option<PolicyConditionSettlement>,
        }

        let wire = Wire::deserialize(deserializer)?;
        Ok(Self {
            outcome: wire.outcome,
            failure_identity: wire.failure_identity,
            reported_success: wire.reported_success,
            original_class: wire.original_class,
            effective_class: wire.effective_class,
            consecutive_same_error: wire.consecutive_same_error,
            escalation_streak: wire
                .escalation_streak
                .0
                .unwrap_or(wire.consecutive_same_error),
            performance_tax_exempt: wire.performance_tax_exempt,
            retry_attempt: wire.retry_attempt,
            disposition: wire.disposition,
            retry_at_unix_ms: wire.retry_at_unix_ms,
            runtime_ms: wire.runtime_ms,
            sensitive: wire.sensitive,
            perf_context: wire.perf_context,
            condition: wire.condition,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PolicyExecutionOutcome {
    Succeeded { runtime_ms: u64 },
    Failed { failure: PolicyFailureRecord },
}

impl PolicyExecutionOutcome {
    /// This result as the public projection carries it (Workflow #378 A2-P, register E9): a
    /// failure's outcome is codes only, as the record's own projected `outcome` is.
    fn codes_only(&self) -> Self {
        match self {
            Self::Succeeded { runtime_ms } => Self::Succeeded {
                runtime_ms: *runtime_ms,
            },
            Self::Failed { failure } => Self::Failed {
                failure: PolicyFailureRecord {
                    outcome: failure.outcome.codes_only(),
                    ..failure.clone()
                },
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyExecutionEventData {
    pub decision_id: String,
    pub task_id: String,
    pub instance_id: String,
    pub observed_at_unix_ms: u64,
    pub outcome: PolicyExecutionOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyExecutionPayload {
    action: EventAction,
    decision_id: String,
    task_id: String,
    instance_id: String,
    observed_at_unix_ms: u64,
    policy_result: PolicyExecutionOutcome,
    /// Workflow #385 (register P6): the run's wall-clock time and its budget-clock time (wall
    /// time less held time, divided by each step's factor), which settlement compares.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    wall_elapsed_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    budget_elapsed_ms: Option<u64>,
    audit: SanitizedAudit,
}

impl PolicyExecutionPayload {
    pub fn decision_id(&self) -> &str {
        &self.decision_id
    }

    pub fn task_id(&self) -> &str {
        &self.task_id
    }

    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    pub const fn observed_at_unix_ms(&self) -> u64 {
        self.observed_at_unix_ms
    }

    /// How the policy recorded the run (ledger revision 3; was `outcome`).
    pub const fn policy_result(&self) -> &PolicyExecutionOutcome {
        &self.policy_result
    }

    /// Workflow #385 (register P6): the run's wall-clock time.
    pub const fn wall_elapsed_ms(&self) -> Option<u64> {
        self.wall_elapsed_ms
    }

    /// Workflow #385 (register P6): the run's budget-clock time.
    pub const fn budget_elapsed_ms(&self) -> Option<u64> {
        self.budget_elapsed_ms
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyPlanningSignalEventData {
    pub signal_id: String,
    pub instance_id: String,
    pub task_id: Option<String>,
    pub kind: PolicyPlanningSignalKind,
    /// The signal's outcome: one registered code with its values (Workflow #378 A2-P).
    pub outcome: Outcome,
    pub observed_at_unix_ms: u64,
    pub detection_budget: Option<PolicyDetectionBudgetRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyDetectionBudgetRecord {
    pub catalog_hash: String,
    pub profile_id: String,
    pub window_id: String,
    pub dispatch_used: u32,
    pub dispatch_limit: u32,
    pub runtime_reserved_ms: u64,
    pub runtime_limit_ms: u64,
    pub reservation_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyPlanningSignalPayload {
    action: EventAction,
    signal_id: String,
    instance_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    task_id: Option<String>,
    kind: PolicyPlanningSignalKind,
    outcome: Outcome,
    observed_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    detection_budget: Option<PolicyDetectionBudgetRecord>,
    audit: SanitizedAudit,
}

impl PolicyPlanningSignalPayload {
    pub fn signal_id(&self) -> &str {
        &self.signal_id
    }

    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    pub fn task_id(&self) -> Option<&str> {
        self.task_id.as_deref()
    }

    pub const fn kind(&self) -> PolicyPlanningSignalKind {
        self.kind
    }

    /// The signal's outcome (Workflow #378 A2-P); the record's severity is its category.
    pub const fn outcome(&self) -> &Outcome {
        &self.outcome
    }

    pub const fn observed_at_unix_ms(&self) -> u64 {
        self.observed_at_unix_ms
    }

    pub const fn detection_budget(&self) -> Option<&PolicyDetectionBudgetRecord> {
        self.detection_budget.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyDispatchPayload {
    action: EventAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostic_code: Option<DiagnosticCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    effect_disposition: Option<EffectDisposition>,
    decision_id: String,
    task_id: String,
    instance_id: String,
    operation_id: String,
    #[serde(default)]
    procedure_binding_digest: String,
    reason_chain_id: String,
    reasons: Vec<PolicyReasonRecord>,
    catalog_hash: String,
    catalog_version: u64,
    /// Absent (read as 0) when the record carries `input_fact_revisions` instead (register L9);
    /// a present position is never 0.
    #[serde(
        default,
        skip_serializing_if = "observer::input_ledger_position_absent",
        deserialize_with = "observer::deserialize_input_ledger_position"
    )]
    input_ledger_position: u64,
    /// Workflow #383 (register L9): both stores' (lineage, revision) the decision read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    input_fact_revisions: Option<Box<InputFactRevisions>>,
    fact_snapshot_id: String,
    approval_fact_ids: Vec<String>,
    #[serde(default)]
    urgency_milli: u16,
    /// Reserved (register A4, RT335-S4): the dispatched task's priority is locked. Only a
    /// dispatch intent carries it.
    #[serde(default, skip_serializing_if = "is_false")]
    priority_locked: bool,
    /// Workflow #383 register X14 (first writer #405): the random delay drawn for this
    /// dispatch. Only a dispatch intent carries it; a drawn delay is not kept across a restart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    random_delay: Option<PolicyRandomDelay>,
    #[serde(skip_serializing_if = "Option::is_none")]
    admission: Option<Box<PolicyAdmissionRecord>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rejection: Option<Box<PolicyDispatchRejection>>,
    audit: SanitizedAudit,
}

/// The longest random delay a configuration allows (`scheduling.random_delay.max_ms`, 24 h).
pub const MAX_POLICY_RANDOM_DELAY_MS: u64 = 86_400_000;

/// Workflow #383 register X14 (#405): a dispatched task waits a random delay after it is due
/// before it starts. `delay_ms` is the delay applied, which a short allowed period may have
/// shortened down to 0, so `planned_start_unix_ms` is `due_at_unix_ms + delay_ms`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyRandomDelay {
    pub due_at_unix_ms: u64,
    pub planned_start_unix_ms: u64,
    pub delay_ms: u64,
}

impl PolicyRandomDelay {
    fn validate(&self) -> Result<(), SanitizationError> {
        if self.due_at_unix_ms == 0
            || self.delay_ms > MAX_POLICY_RANDOM_DELAY_MS
            || self.due_at_unix_ms.checked_add(self.delay_ms) != Some(self.planned_start_unix_ms)
        {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidPolicyPayload,
                "random_delay",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyBudgetDimension {
    TaskDaily,
    TaskWindow,
    TaskRuntime,
    ActivityDaily,
    ActivityWindow,
    ActivityRuntime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyBudgetDenial {
    pub dimension: PolicyBudgetDimension,
    pub used: u64,
    pub requested: u64,
    pub limit: u64,
}

/// Bounded Runtime failure facts, separate from the immutable ranking reason chain: the
/// refusal is `outcome` (its code with `operation`; ledger revision 3), the budget it ran out
/// of and when the task is next eligible stay typed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyDispatchRejection {
    pub outcome: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<PolicyBudgetDenial>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_eligible_unix_ms: Option<u64>,
}

impl PolicyDispatchRejection {
    fn validate(&self) -> Result<(), SanitizationError> {
        Ok(())
    }
}

impl PolicyDispatchPayload {
    pub fn rejection(&self) -> Option<&PolicyDispatchRejection> {
        self.rejection.as_deref()
    }

    pub fn decision_id(&self) -> &str {
        &self.decision_id
    }

    pub fn task_id(&self) -> &str {
        &self.task_id
    }

    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    pub fn procedure_binding_digest(&self) -> &str {
        &self.procedure_binding_digest
    }

    pub fn reason_chain_id(&self) -> &str {
        &self.reason_chain_id
    }

    pub fn reasons(&self) -> &[PolicyReasonRecord] {
        &self.reasons
    }

    pub fn catalog_hash(&self) -> &str {
        &self.catalog_hash
    }

    pub const fn catalog_version(&self) -> u64 {
        self.catalog_version
    }

    /// The ledger position the decision read; 0 when the record carries
    /// [`Self::input_fact_revisions`] instead (register L9).
    pub const fn input_ledger_position(&self) -> u64 {
        self.input_ledger_position
    }

    /// Workflow #383 (register L9): both stores' (lineage, revision) the decision read.
    pub fn input_fact_revisions(&self) -> Option<&InputFactRevisions> {
        self.input_fact_revisions.as_deref()
    }

    /// Reserved (register A4): the dispatched task's priority is locked.
    pub const fn priority_locked(&self) -> bool {
        self.priority_locked
    }

    /// Workflow #383 register X14: the random delay drawn for this dispatch intent.
    pub const fn random_delay(&self) -> Option<&PolicyRandomDelay> {
        self.random_delay.as_ref()
    }

    pub fn fact_snapshot_id(&self) -> &str {
        &self.fact_snapshot_id
    }

    pub fn approval_fact_ids(&self) -> &[String] {
        &self.approval_fact_ids
    }

    pub fn admission(&self) -> Option<&PolicyAdmissionRecord> {
        self.admission.as_deref()
    }

    pub const fn urgency_milli(&self) -> u16 {
        self.urgency_milli
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogTransitionEventData {
    pub catalog_id: String,
    pub catalog_version: u64,
    pub catalog_hash: String,
    pub previous_catalog_hash: Option<String>,
    pub promotion: Option<CatalogPromotionAuthorization>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogTransitionPayload {
    action: EventAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostic_code: Option<DiagnosticCode>,
    /// A failed transition's outcome (ledger revision 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    outcome: Option<Outcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    effect_disposition: Option<EffectDisposition>,
    catalog_id: String,
    catalog_version: u64,
    catalog_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_catalog_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    promotion: Option<CatalogPromotionAuthorization>,
    audit: SanitizedAudit,
}

impl CatalogTransitionPayload {
    pub fn catalog_id(&self) -> &str {
        &self.catalog_id
    }

    pub const fn catalog_version(&self) -> u64 {
        self.catalog_version
    }

    pub fn catalog_hash(&self) -> &str {
        &self.catalog_hash
    }

    pub fn previous_catalog_hash(&self) -> Option<&str> {
        self.previous_catalog_hash.as_deref()
    }

    pub const fn promotion(&self) -> Option<&CatalogPromotionAuthorization> {
        self.promotion.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateMigrationPayload {
    action: EventAction,
    migration: StateMigrationData,
    audit: SanitizedAudit,
}

impl StateMigrationPayload {
    pub const fn migration(&self) -> &StateMigrationData {
        &self.migration
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseStagedPayload {
    action: EventAction,
    effect_disposition: EffectDisposition,
    manifest: RuntimeReleaseSet,
    audit: SanitizedAudit,
}

impl ReleaseStagedPayload {
    pub const fn manifest(&self) -> &RuntimeReleaseSet {
        &self.manifest
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseTransitionPayload {
    action: EventAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostic_code: Option<DiagnosticCode>,
    /// A failed transition's outcome (ledger revision 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    outcome: Option<Outcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    effect_disposition: Option<EffectDisposition>,
    transition: ReleaseTransitionData,
    audit: SanitizedAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentWakePayload {
    action: EventAction,
    wake: AgentWakeData,
    audit: SanitizedAudit,
}

impl AgentWakePayload {
    pub const fn wake(&self) -> &AgentWakeData {
        &self.wake
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSessionPayload {
    action: EventAction,
    session: AgentSessionEventData,
    /// Reserved (register A2, AGT-3): the artifact that holds the agent's report body (a ledger
    /// material, never a board post). Only `agent.response_recorded` carries it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    report_artifact: Option<crate::ArtifactId>,
    audit: SanitizedAudit,
}

impl AgentSessionPayload {
    pub const fn session(&self) -> &AgentSessionEventData {
        &self.session
    }

    /// Reserved (register A2): the artifact of the agent's report.
    pub const fn report_artifact(&self) -> Option<&crate::ArtifactId> {
        self.report_artifact.as_ref()
    }
}

impl ReleaseTransitionPayload {
    pub const fn transition(&self) -> &ReleaseTransitionData {
        &self.transition
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskEntryRecognitionPhase {
    Initial,
    PostRecovery,
    /// Workflow #373 NAV-5 (M-373 §8.3): the feature package's check, after an edge's package
    /// ran, that the run reached the edge's `to` page.
    NavigationTarget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskEntryTargetDisposition {
    Started,
    FailClosed,
}

/// Workflow #373 (model M-373 v1.1 §8.3): the part a package run as a layer of the run plays,
/// on its `EntryRecovery*` records. NAV-6 brings the member and its value `interrupt_handler`
/// (a shared interrupt page's handler or its `then.run` package); NAV-3a adds `prerequisite` (a
/// declared prerequisite package) and `return_home` (the registry's return-home package of the
/// package's game and server). A request's own recovery binding carries none; NAV-5 adds its
/// layers' values: `navigation_hop` (an edge's package of `navigation.json`) and
/// `recovery_chain` (a step of its recovery chain).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskEntryRecoveryRole {
    InterruptHandler,
    Prerequisite,
    ReturnHome,
    NavigationHop,
    RecoveryChain,
}

/// Workflow #373 NAV-5 (M-373 §8.3): one hop of a planned route: the edge's pages (ids of
/// `pages.json`) and its package's `package_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskNavigationHop {
    pub from: String,
    pub to: String,
    pub package_label: String,
}

/// Workflow #373 NAV-6 (§6.2): where the run went after a shared interrupt page's handler:
/// back to what it awaited, to its end, or through the named `then.run` package and back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskInterruptThen {
    Resume,
    EndRun,
    Run(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskGeometryPhase {
    Initial,
    Recheck,
    /// Workflow #383 register X8 (model 392 §7.3, §8.3; first writer DA-4): the display
    /// changed during the run and the map was rebuilt; the trigger names the change.
    Changed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskGeometryRecheckTrigger {
    RecognitionFailed,
    InputFailed,
    PageUnknown,
    /// Register X8: the frame's extent changed (a `changed` observation's trigger).
    ExtentChanged,
    /// Register X8: the display's rotation changed (a `changed` observation's trigger).
    RotationChanged,
    /// Register X8: the safe area's insets changed (a `changed` observation's trigger).
    InsetsChanged,
}

impl TaskGeometryRecheckTrigger {
    /// Whether the trigger names a change of the display (register X8) rather than a failure
    /// that asked for a recheck.
    pub const fn is_display_change(self) -> bool {
        matches!(
            self,
            Self::ExtentChanged | Self::RotationChanged | Self::InsetsChanged
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum TaskGeometryConclusion {
    Pass,
    AspectMismatch { reason: TaskGeometryAspectMismatch },
    Unknown { reason: TaskGeometryUnknownReason },
    Unavailable,
    FixtureNotApplicable,
}

/// Workflow #392 DA-A (model 392 v1.1, section 8.3): the frame's own aspect ratio is recorded,
/// never refused, so the one remaining mismatch is a frame whose aspect ratio differs from the
/// display's logical extent (capture correctness, not an aspect policy).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskGeometryAspectMismatch {
    FrameAndLogicalDisplayDiffer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskGeometryUnknownReason {
    BackendUnsupported,
    ProducerObservationAbsent,
    ProducerUnavailable,
    ProducerBindingMismatch,
    RotationUnavailable,
    FixtureSourceMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskGeometryFrame {
    pub frame_id: crate::FrameId,
    pub extent: crate::CaptureExtent,
    pub backend: crate::CaptureBackendName,
    pub captured_at: std::time::SystemTime,
    pub producer: crate::CaptureGeometryObservation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskGeometryFailure {
    /// The failure (ledger revision 3; was `code`).
    pub outcome: Outcome,
    pub event_id: Option<crate::EventId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskGeometryObservation {
    pub phase: TaskGeometryPhase,
    pub frame: Option<TaskGeometryFrame>,
    pub observation: Option<crate::CaptureGeometryObservation>,
    pub conclusion: TaskGeometryConclusion,
    pub trigger: Option<TaskGeometryRecheckTrigger>,
    pub original_failure: Option<TaskGeometryFailure>,
    pub unavailable: Option<TaskGeometryFailure>,
    /// Workflow #383 register X7 (model 392 §8.3; first writer DA-4): the display map this
    /// observation used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_map: Option<TaskGeometryDisplayMap>,
}

impl TaskGeometryObservation {
    /// Classifies ordered, already-delivered frame dimensions without transforming them. The
    /// frame's aspect ratio is not checked (Workflow #392 DA-A): only its agreement with the
    /// display it was captured from.
    pub fn assess(
        frame: &TaskGeometryFrame,
        observation: &crate::CaptureGeometryObservation,
    ) -> TaskGeometryConclusion {
        use crate::{CaptureBackendName, CaptureGeometryObservation, CaptureGeometryUnknownReason};
        if frame.backend == CaptureBackendName::FixtureSimulation {
            return if matches!(observation, CaptureGeometryObservation::NotApplicable(_))
                && matches!(frame.producer, CaptureGeometryObservation::NotApplicable(_))
            {
                TaskGeometryConclusion::FixtureNotApplicable
            } else {
                TaskGeometryConclusion::Unknown {
                    reason: TaskGeometryUnknownReason::FixtureSourceMismatch,
                }
            };
        }
        let geometry = match observation {
            CaptureGeometryObservation::Observed(geometry) => geometry,
            CaptureGeometryObservation::Unknown(reason) => {
                return TaskGeometryConclusion::Unknown {
                    reason: match reason {
                        CaptureGeometryUnknownReason::BackendUnsupported => {
                            TaskGeometryUnknownReason::BackendUnsupported
                        }
                        CaptureGeometryUnknownReason::ProducerObservationAbsent => {
                            TaskGeometryUnknownReason::ProducerObservationAbsent
                        }
                        CaptureGeometryUnknownReason::ProducerUnavailable => {
                            TaskGeometryUnknownReason::ProducerUnavailable
                        }
                    },
                };
            }
            CaptureGeometryObservation::NotApplicable(_) => {
                return TaskGeometryConclusion::Unknown {
                    reason: TaskGeometryUnknownReason::FixtureSourceMismatch,
                };
            }
        };
        let source_matches_backend = matches!(
            (&geometry.source, geometry.backend),
            (
                crate::CaptureGeometrySource::NemuSdkDisplay { .. },
                CaptureBackendName::NemuIpc
            ) | (
                crate::CaptureGeometrySource::AdbDefaultDisplay { .. },
                CaptureBackendName::AdbScreencap
                    | CaptureBackendName::AdbScreencapEncode
                    | CaptureBackendName::AdbScreencapRawGzip
                    | CaptureBackendName::DroidcastRaw
                    | CaptureBackendName::Scrcpy
            )
        );
        if geometry.backend != frame.backend
            || !source_matches_backend
            || matches!(
                &frame.producer,
                CaptureGeometryObservation::Observed(producer)
                    if producer.backend != frame.backend
                        || !same_geometry_source_binding(&producer.source, &geometry.source)
            )
            || matches!(frame.producer, CaptureGeometryObservation::NotApplicable(_))
        {
            return TaskGeometryConclusion::Unknown {
                reason: TaskGeometryUnknownReason::ProducerBindingMismatch,
            };
        }
        if matches!(
            geometry.source,
            crate::CaptureGeometrySource::AdbDefaultDisplay { .. }
        ) && matches!(
            geometry.rotation,
            crate::CaptureRotationObservation::NotProvidedBySource
        ) {
            return TaskGeometryConclusion::Unknown {
                reason: TaskGeometryUnknownReason::RotationUnavailable,
            };
        }
        // #402 RD-3i: a recorded crop names the region the frame shows; any other geometry keeps
        // the proportional comparison.
        if !geometry.frame_matches_display(frame.extent)
            || matches!(
                &frame.producer,
                CaptureGeometryObservation::Observed(producer)
                    if !producer.frame_matches_display(frame.extent)
            )
        {
            return TaskGeometryConclusion::AspectMismatch {
                reason: TaskGeometryAspectMismatch::FrameAndLogicalDisplayDiffer,
            };
        }
        TaskGeometryConclusion::Pass
    }

    fn validate(&self) -> Result<(), SanitizationError> {
        // Register X8: a recheck follows a failure, a change names what changed.
        let phase_valid = match self.phase {
            TaskGeometryPhase::Initial => {
                self.frame.is_some() && self.trigger.is_none() && self.original_failure.is_none()
            }
            TaskGeometryPhase::Recheck => {
                self.trigger
                    .is_some_and(|trigger| !trigger.is_display_change())
                    && self.original_failure.is_some()
            }
            TaskGeometryPhase::Changed => {
                self.frame.is_some()
                    && self
                        .trigger
                        .is_some_and(TaskGeometryRecheckTrigger::is_display_change)
                    && self.original_failure.is_none()
            }
        };
        if !phase_valid {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidTaskGeometryPhase,
                "geometry",
            ));
        }
        if let Some(frame) = &self.frame {
            validate_geometry_source(&frame.producer)?;
        }
        if let Some(observation) = &self.observation {
            validate_geometry_source(observation)?;
        }
        if let Some(map) = &self.display_map {
            map.validate()?;
        }
        let conclusion_valid = if self.unavailable.is_some() {
            self.observation.is_none() && self.conclusion == TaskGeometryConclusion::Unavailable
        } else {
            match (&self.frame, &self.observation) {
                (Some(frame), Some(observation)) => {
                    self.conclusion == Self::assess(frame, observation)
                }
                _ => false,
            }
        };
        if !conclusion_valid {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidTaskGeometryConclusion,
                "geometry",
            ));
        }
        Ok(())
    }
}

fn same_geometry_source_binding(
    left: &crate::CaptureGeometrySource,
    right: &crate::CaptureGeometrySource,
) -> bool {
    match (left, right) {
        (
            crate::CaptureGeometrySource::AdbDefaultDisplay { serial: left, .. },
            crate::CaptureGeometrySource::AdbDefaultDisplay { serial: right, .. },
        ) => left == right,
        (
            crate::CaptureGeometrySource::NemuSdkDisplay {
                sdk_instance_id: left_instance,
                sdk_display_id: left_display,
                ..
            },
            crate::CaptureGeometrySource::NemuSdkDisplay {
                sdk_instance_id: right_instance,
                sdk_display_id: right_display,
                ..
            },
        ) => left_instance == right_instance && left_display == right_display,
        _ => false,
    }
}

fn validate_geometry_source(
    observation: &crate::CaptureGeometryObservation,
) -> Result<(), SanitizationError> {
    if let crate::CaptureGeometryObservation::Observed(geometry) = observation {
        if let crate::CaptureGeometrySource::AdbDefaultDisplay { serial, .. } = &geometry.source {
            validate_task_semantic_label(serial, "geometry_adb_source")?;
        }
        // Workflow #383 register X6: the insets leave a safe area inside the display.
        if !geometry.safe_insets_fit_display() {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidSanitizedPayload,
                "safe_insets",
            ));
        }
    }
    Ok(())
}

/// Upper bound on the recognized targets one `task.recognition_completed` fact may carry.
pub const TASK_RECOGNITION_TARGET_LIMIT: usize = 32;
const TASK_RECOGNITION_TARGET_ID_MAX_BYTES: usize = 128;

/// The page role under which a recognized target was evaluated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecognizedTargetRole {
    Required,
    AnyOf,
    Optional,
    Forbidden,
}

/// One target of the evaluated page; `region` is in frame pixels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecognizedTarget {
    pub target_id: String,
    pub role: RecognizedTargetRole,
    pub passed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<crate::page_projection::Rect>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TaskSemanticFact {
    /// Workflow #336 PKG-S2 (option 丙): the package is named by its `package_id`
    /// (`package_label`); no content digest is recorded.
    PackageAdmitted {
        package_label: String,
        task_label: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        response_deadline_monotonic_ms: Option<u64>,
    },
    RunStarted,
    EvidenceIndexed {
        frame_width: u32,
        frame_height: u32,
    },
    GeometryObserved {
        observation: Box<TaskGeometryObservation>,
    },
    RecognitionStarted {
        candidate_pages: Vec<String>,
        frame_width: u32,
        frame_height: u32,
    },
    RecognitionCompleted {
        candidate_pages: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        matched_page: Option<String>,
        frame_width: u32,
        frame_height: u32,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        targets: Vec<RecognizedTarget>,
    },
    /// Workflow #373 NAV-3b: the entry's pages (one page, or the 1..=16 pages of an entry page
    /// set) and the one page of them the frame matched.
    EntryRecognition {
        phase: TaskEntryRecognitionPhase,
        required_pages: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        matched_page: Option<String>,
        matched: bool,
    },
    EntryRecoveryDecision {
        required: bool,
    },
    /// Workflow #336 PKG-S2: each layer names its package by `package_id` (`package_label`).
    EntryRecoveryPackageAdmitted {
        package_label: String,
        /// Workflow #373 NAV-6: the layer's part; absent on the layers that have none yet.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<TaskEntryRecoveryRole>,
    },
    EntryRecoveryCompleted {
        package_label: String,
        final_page: String,
        executed_steps: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<TaskEntryRecoveryRole>,
    },
    /// The entry recovery package failed; `outcome` is its failure (ledger revision 3).
    /// Workflow #336 PKG-S2: `package_label` is absent only when the layer's package was refused
    /// before its `package_id` was read (a request's own recovery binding names a path only).
    EntryRecoveryFailed {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        package_label: Option<String>,
        outcome: Outcome,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<TaskEntryRecoveryRole>,
    },
    /// Workflow #373 NAV-6 (§8.3): the shared interrupt page `page` (its `pages.json` id) was
    /// handled by the package `handler_package_label`, the `count`-th time this run; `outcome`
    /// is `contained_task_interrupt_handled`.
    InterruptHandled {
        page: String,
        handler_package_label: String,
        then: TaskInterruptThen,
        count: u8,
        outcome: Outcome,
    },
    /// Workflow #373 NAV-6 (§6.3): one frame matched two or more shared interrupt pages, so
    /// none was handled and the run went on; `outcome` is `contained_task_interrupt_ambiguous`.
    InterruptAmbiguous {
        page_ids: Vec<String>,
        outcome: Outcome,
    },
    /// Workflow #373 NAV-5 (M-373 §5.2, §8.3): the `attempt`-th full match of the entry gate on
    /// one frame. `matched_pages` are the pages of `pages.json` the frame matched (at most 8,
    /// `matched_total` counts them all); `interrupt_page` is the one shared interrupt page the
    /// match handed to its handling. `outcome` is `contained_task_navigation_located`,
    /// `_navigation_page_unrecognized`, `_navigation_page_ambiguous`,
    /// `contained_task_interrupt_ambiguous` or (with `interrupt_page`)
    /// `contained_task_interrupt_handled`.
    NavigationLocated {
        attempt: u32,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        matched_pages: Vec<String>,
        matched_total: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        interrupt_page: Option<String>,
        outcome: Outcome,
    },
    /// Workflow #373 NAV-5 (M-373 §5.3, §8.3): the fewest-hops route from the located page
    /// `from_page` to the run's entry, edge by edge (at most 16; none when the located page is
    /// already an entry page).
    NavigationRoutePlanned {
        from_page: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        hops: Vec<TaskNavigationHop>,
    },
    /// Workflow #373 NAV-5 (M-373 §5.6): a warning of the entry gate's navigation that ends no
    /// run: `contained_task_navigation_route_missing`, `_navigation_hop_unconfirmed` or
    /// `_navigation_entry_unlisted`, with its values.
    NavigationWarning {
        outcome: Outcome,
    },
    /// A fail-closed disposition carries the entry's failure as `outcome`.
    EntryTargetDisposition {
        disposition: TaskEntryTargetDisposition,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outcome: Option<Outcome>,
    },
    StepStarted {
        step_index: u32,
        operation_label: String,
        from_page: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        phase: Option<crate::TaskPhaseEvidence>,
    },
    /// Workflow #308: one select step's decision, recorded before any input for a single
    /// select; a multi select records it once its tap sequence ends, fails or stops, before
    /// the step's `task.effect_completed` (M-357b §5.3).
    SelectionEvaluated {
        step_index: u32,
        operation_label: String,
        selection: Box<TaskSelectionRecord>,
    },
    EffectIntent {
        step_index: u32,
        operation_label: String,
        action: InputAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frame_extent: Option<crate::CaptureExtent>,
    },
    EffectCompleted {
        step_index: u32,
        operation_label: String,
    },
    StepFinished {
        step_index: u32,
        operation_label: String,
        page_label: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        phase: Option<crate::TaskPhaseEvidence>,
    },
    Finalizing {
        task_result: TaskOutcome,
    },
    /// A run's terminal: `task_result` says how it ended, and a failed or cancelled run carries
    /// its failure as `outcome` (ledger revision 3; was `failure_code`).
    TerminalCommitted {
        task_result: TaskOutcome,
        #[serde(skip_serializing_if = "Option::is_none")]
        final_page: Option<String>,
        /// Run-owner logical steps; null means progress was unavailable.
        #[serde(default)]
        executed_steps: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outcome: Option<Outcome>,
        /// The failure point's own verdict that this failure must pause the task: the run
        /// poisoned the Runtime or its error was fatal, except a non-poisoning
        /// `input_backend_operation_failed` or `application_backend_operation_failed` of a
        /// `linear_steps` main package (rerun only, R25-2). A first capture the host makes
        /// nonfatal (ADB baseline lost, a stale kept session) is not severe. Only a failed
        /// terminal sets it. The policy class of a failed scheduled run follows it
        /// ([`scheduled_failure_class`]; Workflow #378 A3, model section 8.1).
        #[serde(default, skip_serializing_if = "is_false")]
        failure_severe: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scheduling_disposition: Option<SchedulingDisposition>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        task_timing: Option<Box<crate::TaskTimingObservations>>,
    },
    /// A second terminal for a run that already had one; `outcome` is why it was refused.
    TerminalRejected {
        committed_outcome: TaskOutcome,
        attempted_outcome: TaskOutcome,
        outcome: Outcome,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputSamplingAlgorithm {
    /// A uniform draw in each box; since Workflow #406 T12 only a runtime without the touch
    /// configuration draws it (the Runtime's own runs do not).
    Xorshift64UniformRectV1,
    /// Workflow #383 register X11, written by #406 T12: a truncated normal draw around the
    /// box's centre, redrawn when it falls outside, the centre after the last redraw.
    CenterHotspotNormalV1,
    /// Register X11, written by #406 T12: several centre-hotspot candidates of a single-region
    /// press, the one unused for the longest time chosen.
    CenterHotspotLeastRecentV1,
}

/// Reserved (Workflow #383 register X12; first writers #406 T-1, T-2): the configured values a
/// centre-hotspot draw used, so a replay from the seed draws the same point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputSamplingParameters {
    /// The standard deviation as a share of the box's width and height, in milli.
    pub sigma_ratio_milli: u32,
    /// How often a draw outside the box is drawn again before the centre is taken.
    pub max_redraws: u32,
    /// How many candidates a least-recent draw drew (T-2 only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_count: Option<u32>,
    /// The index of the chosen candidate, in draw order (T-2 only): the choice also depends
    /// on the in-memory landing history, which no record keeps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chosen_candidate: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputSamplingRegion {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

impl InputSamplingRegion {
    pub fn new(x: i32, y: i32, width: i32, height: i32) -> Result<Self, SanitizationError> {
        let value = Self {
            x,
            y,
            width,
            height,
        };
        value.validate()?;
        Ok(value)
    }

    pub const fn x(&self) -> i32 {
        self.x
    }

    pub const fn y(&self) -> i32 {
        self.y
    }

    pub const fn width(&self) -> i32 {
        self.width
    }

    pub const fn height(&self) -> i32 {
        self.height
    }

    fn validate(&self) -> Result<(), SanitizationError> {
        if self.x < 0
            || self.y < 0
            || self.width <= 0
            || self.height <= 0
            || self.x.checked_add(self.width).is_none()
            || self.y.checked_add(self.height).is_none()
        {
            Err(SanitizationError::new(
                SanitizationCode::InvalidInputSamplingRegion,
                "source_regions",
            ))
        } else {
            Ok(())
        }
    }

    fn contains(&self, x: i32, y: i32) -> bool {
        let Some(end_x) = self.x.checked_add(self.width) else {
            return false;
        };
        let Some(end_y) = self.y.checked_add(self.height) else {
            return false;
        };
        x >= self.x && x < end_x && y >= self.y && y < end_y
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputSamplingEvidence {
    algorithm: InputSamplingAlgorithm,
    action_seed: u64,
    source_regions: Vec<InputSamplingRegion>,
    /// Reserved (register X12): present exactly for a centre-hotspot algorithm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parameters: Option<InputSamplingParameters>,
}

impl InputSamplingEvidence {
    pub fn new(
        action_seed: u64,
        source_regions: Vec<InputSamplingRegion>,
    ) -> Result<Self, SanitizationError> {
        let value = Self {
            algorithm: InputSamplingAlgorithm::Xorshift64UniformRectV1,
            action_seed,
            source_regions,
            parameters: None,
        };
        value.validate_regions()?;
        Ok(value)
    }

    /// Workflow #406 T12 (register X11, X12): a `center_hotspot_normal_v1` draw, one
    /// centre-hotspot point in each of `source_regions` in order, with the spread it used
    /// (`sigma_ratio_milli`) and how often a point outside its box was drawn again before the
    /// centre was taken (`max_redraws`).
    pub fn center_hotspot(
        action_seed: u64,
        source_regions: Vec<InputSamplingRegion>,
        sigma_ratio_milli: u32,
        max_redraws: u32,
    ) -> Result<Self, SanitizationError> {
        let value = Self {
            algorithm: InputSamplingAlgorithm::CenterHotspotNormalV1,
            action_seed,
            source_regions,
            parameters: Some(InputSamplingParameters {
                sigma_ratio_milli,
                max_redraws,
                candidate_count: None,
                chosen_candidate: None,
            }),
        };
        value.validate_regions()?;
        value.validate_parameters()?;
        Ok(value)
    }

    /// Workflow #406 T12 (register X11, X12): a `center_hotspot_least_recent_v1` draw of a
    /// single-region press, `candidate_count` centre-hotspot candidates in `source_region` drawn
    /// with the spread `sigma_ratio_milli` and `max_redraws`, of which the landing history chose
    /// `chosen_candidate` (0-based, in draw order).
    pub fn center_hotspot_least_recent(
        action_seed: u64,
        source_region: InputSamplingRegion,
        sigma_ratio_milli: u32,
        max_redraws: u32,
        candidate_count: u32,
        chosen_candidate: u32,
    ) -> Result<Self, SanitizationError> {
        let value = Self {
            algorithm: InputSamplingAlgorithm::CenterHotspotLeastRecentV1,
            action_seed,
            source_regions: vec![source_region],
            parameters: Some(InputSamplingParameters {
                sigma_ratio_milli,
                max_redraws,
                candidate_count: Some(candidate_count),
                chosen_candidate: Some(chosen_candidate),
            }),
        };
        value.validate_regions()?;
        value.validate_parameters()?;
        Ok(value)
    }

    pub const fn algorithm(&self) -> InputSamplingAlgorithm {
        self.algorithm
    }

    pub const fn action_seed(&self) -> u64 {
        self.action_seed
    }

    pub fn source_regions(&self) -> &[InputSamplingRegion] {
        &self.source_regions
    }

    /// Reserved (register X12): the values a centre-hotspot draw used.
    pub const fn parameters(&self) -> Option<&InputSamplingParameters> {
        self.parameters.as_ref()
    }

    /// Register X11, X12: the uniform draw has no parameters; a centre-hotspot draw names its
    /// spread; a least-recent draw also names its candidates and the one it chose.
    fn validate_parameters(&self) -> Result<(), SanitizationError> {
        let valid = match (self.algorithm, &self.parameters) {
            (InputSamplingAlgorithm::Xorshift64UniformRectV1, None) => true,
            (InputSamplingAlgorithm::CenterHotspotNormalV1, Some(parameters)) => {
                parameters.sigma_ratio_milli > 0
                    && parameters.candidate_count.is_none()
                    && parameters.chosen_candidate.is_none()
            }
            (InputSamplingAlgorithm::CenterHotspotLeastRecentV1, Some(parameters)) => {
                parameters.sigma_ratio_milli > 0
                    && matches!(
                        (parameters.candidate_count, parameters.chosen_candidate),
                        (Some(count), Some(chosen)) if chosen < count
                    )
            }
            _ => false,
        };
        if valid {
            Ok(())
        } else {
            Err(SanitizationError::new(
                SanitizationCode::InvalidSanitizedPayload,
                "sampling_parameters",
            ))
        }
    }

    fn validate_regions(&self) -> Result<(), SanitizationError> {
        if !matches!(self.source_regions.len(), 1 | 2) {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidInputSamplingRegion,
                "source_regions",
            ));
        }
        for region in &self.source_regions {
            region.validate()?;
        }
        Ok(())
    }

    fn validate(&self, action: &InputAction) -> Result<(), SanitizationError> {
        self.validate_regions()?;
        self.validate_parameters()?;
        let valid = match (action, self.source_regions.as_slice()) {
            (InputAction::Tap { x, y }, [region]) => region.contains(*x, *y),
            // Workflow #390 lpbox: a long press held at a point drawn in its click box.
            (InputAction::LongTap { x, y, .. }, [region]) => region.contains(*x, *y),
            (InputAction::Swipe { x1, y1, x2, y2, .. }, [from, to]) => {
                from.contains(*x1, *y1) && to.contains(*x2, *y2)
            }
            (
                InputAction::SingleTouchDragWithVerticalBrakeV1 {
                    x1,
                    y1,
                    x2,
                    y2,
                    x3,
                    y3,
                    brake_distance_px,
                    ..
                },
                [from, corner],
            ) => {
                from.contains(*x1, *y1)
                    && corner.contains(*x2, *y2)
                    && *x3 == *x2
                    && y2.checked_sub(*brake_distance_px) == Some(*y3)
            }
            // Workflow #308 SEL-L3b: the press and the drag's end are sampled in their regions;
            // the brake's end follows from the drag's end by the declared brake.
            (
                InputAction::SingleTouchDragWithPauseAndBrakeV1 { x1, y1, x2, y2, .. },
                [from, to],
            ) => from.contains(*x1, *y1) && to.contains(*x2, *y2),
            _ => false,
        };
        if valid {
            Ok(())
        } else {
            Err(SanitizationError::new(
                SanitizationCode::InvalidInputSamplingAction,
                "sampling",
            ))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchedulingEffectCondition {
    NoDesignatedEffect,
    DesignatedEffectCompleted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchedulingOutcomeMapping {
    outcome_key: String,
    effect: SchedulingEffectCondition,
    terminal_pages: Vec<String>,
    /// Workflow #411 B15 (contracts/remedies.md section 2.1): a run that ends on this key is a
    /// success whose business result is not the expected one, and serving the remedy's need may
    /// make it reachable. Absent, the mapping serializes as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    remedy: Option<RemedyDeclaration>,
}

impl SchedulingOutcomeMapping {
    pub fn outcome_key(&self) -> &str {
        &self.outcome_key
    }

    pub const fn effect(&self) -> SchedulingEffectCondition {
        self.effect
    }

    pub fn terminal_pages(&self) -> &[String] {
        &self.terminal_pages
    }

    /// Workflow #411 B15: the remedy the mapping declares, if any.
    pub const fn remedy(&self) -> Option<&RemedyDeclaration> {
        self.remedy.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchedulingOutcomeDeclaration {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    designated_operation: Option<String>,
    /// Workflow #390 OUT-2: a designated effect set (task schema `0.9`), instead of the one
    /// `designated_operation`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    designated_effects: Option<Vec<DesignatedEffectDeclaration>>,
    mappings: Vec<SchedulingOutcomeMapping>,
}

impl SchedulingOutcomeDeclaration {
    pub fn designated_operation(&self) -> Option<&str> {
        self.designated_operation.as_deref()
    }

    /// Workflow #390 OUT-2: the designated effect set, when the declaration names one.
    pub fn designated_effects(&self) -> Option<&[DesignatedEffectDeclaration]> {
        self.designated_effects.as_deref()
    }

    /// Whether the declaration names a designated operation or a designated effect set.
    pub fn has_designated_effect(&self) -> bool {
        self.designated_operation.is_some() || self.designated_effects.is_some()
    }

    /// The designated operations of either form, in declaration order.
    pub fn designated_operations(&self) -> Vec<&str> {
        match (&self.designated_operation, &self.designated_effects) {
            (Some(operation), _) => vec![operation.as_str()],
            (None, Some(effects)) => effects
                .iter()
                .map(DesignatedEffectDeclaration::operation_id)
                .collect(),
            (None, None) => Vec::new(),
        }
    }

    /// Workflow #390 OUT-2: the designated effects as the static reachability checks see them.
    pub const fn designated_effect_reach(&self) -> DesignatedEffectReach<'_> {
        DesignatedEffectReach::new(self)
    }

    pub fn mappings(&self) -> &[SchedulingOutcomeMapping] {
        &self.mappings
    }

    pub fn validate(&self) -> Result<(), SanitizationError> {
        if let Some(operation) = &self.designated_operation {
            validate_task_semantic_label(operation, "designated_operation")?;
        }
        if let Some(effects) = &self.designated_effects {
            if self.designated_operation.is_some() {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidSchedulingOutcomeDeclaration,
                    "designated_effects",
                ));
            }
            validate_designated_effects(effects)?;
        }
        if self.mappings.is_empty() || self.mappings.len() > 64 {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidSchedulingOutcomeDeclaration,
                "mappings",
            ));
        }
        let mut keys = BTreeSet::new();
        let mut conditions = BTreeSet::new();
        for mapping in &self.mappings {
            validate_policy_token(&mapping.outcome_key, "outcome_key")?;
            if !keys.insert(mapping.outcome_key.as_str()) {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidSchedulingOutcomeDeclaration,
                    "outcome_key",
                ));
            }
            if mapping.effect == SchedulingEffectCondition::DesignatedEffectCompleted
                && !self.has_designated_effect()
            {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidSchedulingOutcomeDeclaration,
                    "designated_operation",
                ));
            }
            if mapping.terminal_pages.is_empty() || mapping.terminal_pages.len() > 64 {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidSchedulingOutcomeDeclaration,
                    "terminal_pages",
                ));
            }
            let mut pages = BTreeSet::new();
            for page in &mapping.terminal_pages {
                validate_task_semantic_label(page, "terminal_page")?;
                if !pages.insert(page.as_str()) {
                    return Err(SanitizationError::new(
                        SanitizationCode::InvalidSchedulingOutcomeDeclaration,
                        "terminal_pages",
                    ));
                }
                if !conditions.insert((mapping.effect, page.as_str())) {
                    return Err(SanitizationError::new(
                        SanitizationCode::InvalidSchedulingOutcomeDeclaration,
                        "mapping_overlap",
                    ));
                }
            }
            // Workflow #411 B15: a remedy keeps the rules of contracts/remedies.md section 2.1.
            if let Some(remedy) = &mapping.remedy {
                remedy.validate()?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SchedulingEffectEvidence {
    NoDesignatedEffect,
    DesignatedEffectCompleted {
        step_index: u32,
        operation_label: String,
    },
    /// Workflow #390 OUT-2: the run of a designated effect set; one tally per member, in
    /// declaration order, its count 0 when the member did not complete.
    DesignatedEffects {
        effects: Vec<DesignatedEffectTally>,
    },
}

impl SchedulingEffectEvidence {
    /// The mapping condition the evidence selects: a set's run completed a designated effect
    /// when any member completed.
    pub fn condition(&self) -> SchedulingEffectCondition {
        match self {
            Self::NoDesignatedEffect => SchedulingEffectCondition::NoDesignatedEffect,
            Self::DesignatedEffectCompleted { .. } => {
                SchedulingEffectCondition::DesignatedEffectCompleted
            }
            Self::DesignatedEffects { effects } => tallies_condition(effects),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchedulingDisposition {
    outcome_key: String,
    effect: SchedulingEffectEvidence,
}

impl SchedulingDisposition {
    pub fn new(
        outcome_key: impl Into<String>,
        effect: SchedulingEffectEvidence,
    ) -> Result<Self, SanitizationError> {
        let value = Self {
            outcome_key: outcome_key.into(),
            effect,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn outcome_key(&self) -> &str {
        &self.outcome_key
    }

    pub const fn effect(&self) -> &SchedulingEffectEvidence {
        &self.effect
    }

    fn validate(&self) -> Result<(), SanitizationError> {
        validate_policy_token(&self.outcome_key, "outcome_key")?;
        match &self.effect {
            SchedulingEffectEvidence::NoDesignatedEffect => {}
            SchedulingEffectEvidence::DesignatedEffectCompleted {
                step_index,
                operation_label,
            } => {
                validate_task_step(*step_index)?;
                validate_task_semantic_label(operation_label, "operation_label")?;
            }
            SchedulingEffectEvidence::DesignatedEffects { effects } => {
                validate_designated_effect_tallies(effects)?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchedulingOutcomeIdentity {
    terminal_event_id: EventId,
    terminal_sequence: u64,
    instance_id: InstanceId,
    task_id: TaskId,
    run_id: RunId,
    request_id: RequestId,
    correlation_id: CorrelationId,
    lease_id: LeaseId,
    decision_id: String,
    catalog_task_id: String,
    instance_alias: String,
}

impl SchedulingOutcomeIdentity {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        terminal_event_id: EventId,
        terminal_sequence: u64,
        instance_id: InstanceId,
        task_id: TaskId,
        run_id: RunId,
        request_id: RequestId,
        correlation_id: CorrelationId,
        lease_id: LeaseId,
        decision_id: impl Into<String>,
        catalog_task_id: impl Into<String>,
        instance_alias: impl Into<String>,
    ) -> Result<Self, SanitizationError> {
        let value = Self {
            terminal_event_id,
            terminal_sequence,
            instance_id,
            task_id,
            run_id,
            request_id,
            correlation_id,
            lease_id,
            decision_id: decision_id.into(),
            catalog_task_id: catalog_task_id.into(),
            instance_alias: instance_alias.into(),
        };
        value.validate()?;
        Ok(value)
    }

    pub const fn terminal_event_id(&self) -> EventId {
        self.terminal_event_id
    }

    pub const fn terminal_sequence(&self) -> u64 {
        self.terminal_sequence
    }

    pub const fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    pub const fn task_id(&self) -> TaskId {
        self.task_id
    }

    pub const fn run_id(&self) -> RunId {
        self.run_id
    }

    pub const fn request_id(&self) -> RequestId {
        self.request_id
    }

    pub const fn correlation_id(&self) -> CorrelationId {
        self.correlation_id
    }

    pub const fn lease_id(&self) -> LeaseId {
        self.lease_id
    }

    pub fn decision_id(&self) -> &str {
        &self.decision_id
    }

    pub fn catalog_task_id(&self) -> &str {
        &self.catalog_task_id
    }

    pub fn instance_alias(&self) -> &str {
        &self.instance_alias
    }

    fn validate(&self) -> Result<(), SanitizationError> {
        if self.terminal_sequence == 0 {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidAuthoritativeSchedulingOutcome,
                "position",
            ));
        }
        validate_policy_token(&self.decision_id, "decision_id")?;
        validate_task_semantic_label(&self.catalog_task_id, "catalog_task_id")?;
        crate::validate_instance_alias(&self.instance_alias).map_err(|_| {
            SanitizationError::new(SanitizationCode::InvalidTaskSemanticLabel, "instance_alias")
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthoritativeSchedulingOutcome {
    identity: SchedulingOutcomeIdentity,
    disposition: SchedulingDisposition,
    terminal_timestamp_unix_ms: u64,
}

impl AuthoritativeSchedulingOutcome {
    pub fn new(
        identity: SchedulingOutcomeIdentity,
        disposition: SchedulingDisposition,
        terminal_timestamp_unix_ms: u64,
    ) -> Result<Self, SanitizationError> {
        identity.validate()?;
        disposition.validate()?;
        if terminal_timestamp_unix_ms == 0 {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidAuthoritativeSchedulingOutcome,
                "terminal_timestamp_unix_ms",
            ));
        }
        Ok(Self {
            identity,
            disposition,
            terminal_timestamp_unix_ms,
        })
    }

    pub const fn identity(&self) -> &SchedulingOutcomeIdentity {
        &self.identity
    }

    pub const fn disposition(&self) -> &SchedulingDisposition {
        &self.disposition
    }

    pub const fn terminal_timestamp_unix_ms(&self) -> u64 {
        self.terminal_timestamp_unix_ms
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchedulingOutcomeProjection {
    ledger_position: u64,
    scheduling_result: AuthoritativeSchedulingOutcome,
}

impl SchedulingOutcomeProjection {
    pub fn new(
        ledger_position: u64,
        scheduling_result: AuthoritativeSchedulingOutcome,
    ) -> Result<Self, SanitizationError> {
        if ledger_position == 0
            || scheduling_result.identity().terminal_sequence() > ledger_position
        {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidSchedulingOutcomeProjection,
                "ledger_position",
            ));
        }
        Ok(Self {
            ledger_position,
            scheduling_result,
        })
    }

    pub const fn ledger_position(&self) -> u64 {
        self.ledger_position
    }

    /// The authoritative scheduling outcome (ledger revision 3; was `outcome`).
    pub const fn scheduling_result(&self) -> &AuthoritativeSchedulingOutcome {
        &self.scheduling_result
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSemanticPayload {
    action: EventAction,
    fact: TaskSemanticFact,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lease_expires_at_monotonic_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sampling: Option<InputSamplingEvidence>,
    audit: SanitizedAudit,
}

impl TaskSemanticPayload {
    pub const fn action(&self) -> EventAction {
        self.action
    }

    pub const fn fact(&self) -> &TaskSemanticFact {
        &self.fact
    }

    pub const fn lease_expires_at_monotonic_ms(&self) -> Option<u64> {
        self.lease_expires_at_monotonic_ms
    }

    pub const fn sampling(&self) -> Option<&InputSamplingEvidence> {
        self.sampling.as_ref()
    }

    pub fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }

    fn validate(&self) -> Result<(), SanitizationError> {
        if self.action != EventAction::RuntimeTaskRun {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidTaskSemanticAction,
                "action",
            ));
        }
        self.fact.validate()?;
        if let Some(expires_at) = self.lease_expires_at_monotonic_ms
            && !matches!(
                &self.fact,
                TaskSemanticFact::PackageAdmitted {
                    response_deadline_monotonic_ms: Some(deadline),
                    ..
                } if *deadline < expires_at
            )
        {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidTaskLeaseDeadline,
                "lease_expires_at_monotonic_ms",
            ));
        }
        match (&self.fact, &self.sampling) {
            (TaskSemanticFact::EffectIntent { action, .. }, Some(sampling)) => {
                sampling.validate(action)
            }
            (TaskSemanticFact::EffectIntent { .. }, None) | (_, None) => Ok(()),
            (_, Some(_)) => Err(SanitizationError::new(
                SanitizationCode::InvalidInputSamplingFact,
                "sampling",
            )),
        }
    }
}

fn redact_sensitive_input(action: &mut InputAction) {
    match action {
        InputAction::Key { key } => *key = "[redacted]".to_string(),
        InputAction::Text { text } => *text = "[redacted]".to_string(),
        InputAction::Tap { .. }
        | InputAction::LongTap { .. }
        | InputAction::Swipe { .. }
        | InputAction::SingleTouchDragWithVerticalBrakeV1 { .. }
        | InputAction::SingleTouchPathDragV1 { .. }
        | InputAction::SingleTouchDragWithPauseAndBrakeV1 { .. }
        | InputAction::Reset => {}
    }
}

impl TaskSemanticFact {
    /// The outcome the fact carries (ledger revision 3): a failed or cancelled terminal's
    /// failure, a refused second terminal, a failed entry recovery or fail-closed entry, or a
    /// select step's failed confirmation.
    pub fn outcome(&self) -> Option<&Outcome> {
        match self {
            Self::EntryRecoveryFailed { outcome, .. }
            | Self::TerminalRejected { outcome, .. }
            | Self::InterruptHandled { outcome, .. }
            | Self::InterruptAmbiguous { outcome, .. }
            | Self::NavigationLocated { outcome, .. }
            | Self::NavigationWarning { outcome } => Some(outcome),
            Self::EntryTargetDisposition { outcome, .. }
            | Self::TerminalCommitted { outcome, .. } => outcome.as_ref(),
            Self::SelectionEvaluated { selection, .. } => match &selection.confirmation {
                TaskSelectionConfirmation::GuardFailed { outcome }
                | TaskSelectionConfirmation::CaptureFailed { outcome }
                | TaskSelectionConfirmation::RepositionFailed { outcome, .. } => Some(outcome),
                _ => None,
            },
            _ => None,
        }
    }

    /// The fact as a public projection shows it: each outcome codes only (model section 4.3),
    /// so no data value and no foreign text leaves through it.
    fn public_summary(&self) -> Self {
        let mut fact = self.clone();
        match &mut fact {
            Self::EntryRecoveryFailed { outcome, .. }
            | Self::TerminalRejected { outcome, .. }
            | Self::InterruptHandled { outcome, .. }
            | Self::InterruptAmbiguous { outcome, .. }
            | Self::NavigationLocated { outcome, .. }
            | Self::NavigationWarning { outcome } => {
                *outcome = outcome.codes_only();
            }
            Self::EntryTargetDisposition {
                outcome: Some(outcome),
                ..
            }
            | Self::TerminalCommitted {
                outcome: Some(outcome),
                ..
            } => *outcome = outcome.codes_only(),
            Self::SelectionEvaluated { selection, .. } => {
                // Workflow #378 A2-P: every reason is an outcome; the projection keeps its codes.
                let selection = &mut **selection;
                for reason in selection.reasons.iter_mut().chain(
                    selection
                        .verdicts
                        .iter_mut()
                        .flat_map(|verdict| verdict.reasons.iter_mut()),
                ) {
                    reason.0 = reason.0.codes_only();
                }
                // M-357b MS4: a multi select's picks keep only codes too, in every confirmation
                // and tap check.
                let picks = selection
                    .multi_select
                    .iter_mut()
                    .flat_map(|multi| multi.picks.iter_mut())
                    .flat_map(|pick| {
                        std::iter::once(&mut pick.confirmation).chain(pick.tap_check.as_mut())
                    });
                for confirmation in std::iter::once(&mut selection.confirmation).chain(picks) {
                    if let TaskSelectionConfirmation::GuardFailed { outcome }
                    | TaskSelectionConfirmation::CaptureFailed { outcome }
                    | TaskSelectionConfirmation::RepositionFailed { outcome, .. } = confirmation
                    {
                        *outcome = outcome.codes_only();
                    }
                }
            }
            Self::GeometryObserved { observation } => {
                for failure in observation
                    .original_failure
                    .iter_mut()
                    .chain(observation.unavailable.iter_mut())
                {
                    failure.outcome = failure.outcome.codes_only();
                }
            }
            _ => {}
        }
        fact
    }

    fn redact_sensitive_input(&mut self) {
        let Self::EffectIntent { action, .. } = self else {
            return;
        };
        redact_sensitive_input(action);
    }

    fn event_type(&self) -> EventType {
        match self {
            Self::PackageAdmitted { .. } => EventType::TaskRequested,
            Self::RunStarted => EventType::TaskStarted,
            Self::EvidenceIndexed { .. } => EventType::TaskEvidenceIndexed,
            Self::GeometryObserved { .. } => EventType::TaskGeometryObserved,
            Self::RecognitionStarted { .. } => EventType::TaskRecognitionStarted,
            Self::RecognitionCompleted { .. } => EventType::TaskRecognitionCompleted,
            Self::EntryRecognition { .. }
            | Self::EntryRecoveryDecision { .. }
            | Self::EntryRecoveryPackageAdmitted { .. }
            | Self::EntryRecoveryCompleted { .. }
            | Self::EntryRecoveryFailed { .. }
            | Self::EntryTargetDisposition { .. }
            | Self::InterruptHandled { .. }
            | Self::InterruptAmbiguous { .. }
            | Self::NavigationLocated { .. }
            | Self::NavigationRoutePlanned { .. }
            | Self::NavigationWarning { .. } => EventType::TaskEntryPreflight,
            Self::StepStarted { .. } => EventType::TaskStepStarted,
            Self::SelectionEvaluated { .. } => EventType::TaskSelectionEvaluated,
            Self::EffectIntent { .. } => EventType::TaskEffectIntent,
            Self::EffectCompleted { .. } => EventType::TaskEffectCompleted,
            Self::StepFinished { .. } => EventType::TaskStepFinished,
            Self::Finalizing { .. } => EventType::TaskTerminalIntent,
            Self::TerminalCommitted {
                task_result: TaskOutcome::Success,
                ..
            } => EventType::TaskCompleted,
            Self::TerminalCommitted {
                task_result: TaskOutcome::Failure,
                ..
            } => EventType::TaskFailed,
            Self::TerminalCommitted {
                task_result: TaskOutcome::Cancelled,
                ..
            } => EventType::TaskCancelled,
            Self::TerminalRejected { .. } => EventType::TaskTerminalRejected,
        }
    }

    fn validate(&self) -> Result<(), SanitizationError> {
        match self {
            Self::PackageAdmitted {
                package_label,
                task_label,
                response_deadline_monotonic_ms,
            } => {
                validate_task_semantic_label(package_label, "package_label")?;
                validate_task_semantic_label(task_label, "task_label")?;
                if response_deadline_monotonic_ms.is_some_and(|deadline| deadline == 0) {
                    return Err(SanitizationError::new(
                        SanitizationCode::InvalidTaskResponseDeadline,
                        "response_deadline_monotonic_ms",
                    ));
                }
            }
            Self::RunStarted => {}
            Self::GeometryObserved { observation } => observation.validate()?,
            Self::EvidenceIndexed {
                frame_width,
                frame_height,
            } => validate_task_frame_dimensions(*frame_width, *frame_height)?,
            Self::RecognitionStarted {
                candidate_pages,
                frame_width,
                frame_height,
            } => {
                validate_task_candidate_pages(candidate_pages)?;
                validate_task_frame_dimensions(*frame_width, *frame_height)?;
            }
            Self::RecognitionCompleted {
                candidate_pages,
                matched_page,
                frame_width,
                frame_height,
                targets,
            } => {
                validate_task_candidate_pages(candidate_pages)?;
                validate_task_frame_dimensions(*frame_width, *frame_height)?;
                if let Some(page) = matched_page {
                    validate_task_semantic_label(page, "matched_page")?;
                    if !candidate_pages.contains(page) {
                        return Err(SanitizationError::new(
                            SanitizationCode::InvalidTaskRecognitionResult,
                            "matched_page",
                        ));
                    }
                }
                validate_task_recognition_targets(targets, *frame_width, *frame_height)?;
            }
            Self::EntryRecognition {
                required_pages,
                matched_page,
                matched,
                ..
            } => {
                if required_pages.is_empty() || required_pages.len() > crate::MAX_ENTRY_PAGES {
                    return Err(SanitizationError::new(
                        SanitizationCode::InvalidTaskCandidatePages,
                        "required_pages",
                    ));
                }
                for (index, page) in required_pages.iter().enumerate() {
                    validate_task_semantic_label(page, "required_pages")?;
                    if required_pages[..index].contains(page) {
                        return Err(SanitizationError::new(
                            SanitizationCode::InvalidTaskCandidatePages,
                            "required_pages",
                        ));
                    }
                }
                if matched_page.is_some() != *matched
                    || matched_page
                        .as_ref()
                        .is_some_and(|page| !required_pages.contains(page))
                {
                    return Err(SanitizationError::new(
                        SanitizationCode::InvalidTaskRecognitionResult,
                        "matched_page",
                    ));
                }
            }
            Self::EntryRecoveryDecision { .. } => {}
            Self::EntryRecoveryPackageAdmitted { package_label, .. } => {
                validate_task_semantic_label(package_label, "package_label")?;
            }
            Self::EntryRecoveryCompleted {
                package_label,
                final_page,
                executed_steps,
                ..
            } => {
                validate_task_semantic_label(package_label, "package_label")?;
                validate_task_semantic_label(final_page, "final_page")?;
                if *executed_steps > 1_000 {
                    return Err(SanitizationError::new(
                        SanitizationCode::InvalidTaskEntryRecovery,
                        "executed_steps",
                    ));
                }
            }
            Self::EntryRecoveryFailed { package_label, .. } => {
                if let Some(package_label) = package_label {
                    validate_task_semantic_label(package_label, "package_label")?;
                }
            }
            Self::InterruptHandled {
                page,
                handler_package_label,
                then,
                count,
                ..
            } => {
                validate_task_semantic_label(page, "page")?;
                validate_task_semantic_label(handler_package_label, "handler_package_label")?;
                if let TaskInterruptThen::Run(package_label) = then {
                    validate_task_semantic_label(package_label, "then")?;
                }
                if *count == 0 || *count > crate::MAX_INTERRUPT_HANDLINGS_PER_PAGE {
                    return Err(SanitizationError::new(
                        SanitizationCode::InvalidTaskInterrupt,
                        "count",
                    ));
                }
            }
            Self::InterruptAmbiguous { page_ids, .. } => {
                if page_ids.len() < 2 || page_ids.len() > crate::MAX_STANDARD_PACK_INTERRUPTS {
                    return Err(SanitizationError::new(
                        SanitizationCode::InvalidTaskInterrupt,
                        "page_ids",
                    ));
                }
                for page in page_ids {
                    validate_task_semantic_label(page, "page_ids")?;
                }
            }
            // Workflow #373 NAV-5 (M-373 §3.10, §8.3): a gate's 16 locates and hops, at most
            // 8 matched pages listed of at most 256 counted, each a label.
            Self::NavigationLocated {
                attempt,
                matched_pages,
                matched_total,
                interrupt_page,
                ..
            } => {
                let total = usize::try_from(*matched_total).unwrap_or(usize::MAX);
                if *attempt == 0
                    || *attempt > crate::MAX_NAVIGATION_LOCATES
                    || matched_pages.len() > crate::MAX_NAVIGATION_RECORDED_PAGES
                    || total > crate::MAX_STANDARD_PACK_PAGES
                    || matched_pages.len() != total.min(crate::MAX_NAVIGATION_RECORDED_PAGES)
                {
                    return Err(SanitizationError::new(
                        SanitizationCode::InvalidTaskNavigation,
                        "matched_pages",
                    ));
                }
                for page in matched_pages.iter().chain(interrupt_page) {
                    validate_task_semantic_label(page, "matched_pages")?;
                }
            }
            Self::NavigationRoutePlanned { from_page, hops } => {
                validate_task_semantic_label(from_page, "from_page")?;
                if hops.len() > crate::MAX_NAVIGATION_HOPS as usize {
                    return Err(SanitizationError::new(
                        SanitizationCode::InvalidTaskNavigation,
                        "hops",
                    ));
                }
                for hop in hops {
                    validate_task_semantic_label(&hop.from, "hops")?;
                    validate_task_semantic_label(&hop.to, "hops")?;
                    validate_task_semantic_label(&hop.package_label, "hops")?;
                }
            }
            Self::NavigationWarning { .. } => {}
            Self::EntryTargetDisposition {
                disposition,
                outcome,
            } => match (disposition, outcome) {
                (TaskEntryTargetDisposition::Started, None)
                | (TaskEntryTargetDisposition::FailClosed, Some(_)) => {}
                _ => {
                    return Err(SanitizationError::new(
                        SanitizationCode::InvalidTaskEntryDisposition,
                        "failure_code",
                    ));
                }
            },
            Self::StepStarted {
                step_index,
                operation_label,
                from_page,
                phase,
            } => {
                if let Some(phase) = phase {
                    validate_task_semantic_label(&phase.id, "phase_id")?;
                    if phase.index as usize >= crate::MAX_TASK_PHASES || phase.completed {
                        return Err(SanitizationError::new(
                            SanitizationCode::InvalidTaskPhase,
                            "phase",
                        ));
                    }
                }
                validate_task_step(*step_index)?;
                validate_task_semantic_label(operation_label, "operation_label")?;
                validate_task_semantic_label(from_page, "from_page")?;
            }
            Self::SelectionEvaluated {
                step_index,
                operation_label,
                selection,
            } => {
                validate_task_step(*step_index)?;
                validate_task_semantic_label(operation_label, "operation_label")?;
                selection.validate()?;
            }
            Self::EffectIntent {
                step_index,
                operation_label,
                action,
                frame_extent: _,
            } => {
                validate_task_step(*step_index)?;
                validate_task_semantic_label(operation_label, "operation_label")?;
                action.validate().map_err(|_| {
                    SanitizationError::new(SanitizationCode::InvalidTaskEffect, "input_action")
                })?;
            }
            Self::EffectCompleted {
                step_index,
                operation_label,
            } => {
                validate_task_step(*step_index)?;
                validate_task_semantic_label(operation_label, "operation_label")?;
            }
            Self::StepFinished {
                step_index,
                operation_label,
                page_label,
                phase,
            } => {
                if let Some(phase) = phase {
                    validate_task_semantic_label(&phase.id, "phase_id")?;
                    if phase.index as usize >= crate::MAX_TASK_PHASES {
                        return Err(SanitizationError::new(
                            SanitizationCode::InvalidTaskPhase,
                            "phase",
                        ));
                    }
                }
                validate_task_step(*step_index)?;
                validate_task_semantic_label(operation_label, "operation_label")?;
                validate_task_semantic_label(page_label, "page_label")?;
            }
            Self::Finalizing { .. } => {}
            Self::TerminalCommitted {
                task_result,
                final_page,
                executed_steps,
                outcome,
                failure_severe,
                scheduling_disposition,
                task_timing,
            } => {
                if let Some(timing) = task_timing {
                    timing.validate()?;
                }
                if *failure_severe && *task_result != TaskOutcome::Failure {
                    return Err(SanitizationError::new(
                        SanitizationCode::InvalidTaskTerminal,
                        "failure_severe",
                    ));
                }
                if executed_steps.is_some_and(|steps| steps > 1_000)
                    || (*task_result == TaskOutcome::Success && executed_steps.is_none())
                {
                    return Err(SanitizationError::new(
                        SanitizationCode::InvalidTaskTerminal,
                        "executed_steps",
                    ));
                }
                if let Some(page) = final_page {
                    validate_task_semantic_label(page, "final_page")?;
                }
                match (task_result, outcome) {
                    (TaskOutcome::Success, None) => {
                        if let Some(disposition) = scheduling_disposition {
                            if final_page.is_none() {
                                return Err(SanitizationError::new(
                                    SanitizationCode::InvalidTaskTerminal,
                                    "scheduling_disposition",
                                ));
                            }
                            disposition.validate()?;
                        }
                    }
                    (TaskOutcome::Failure | TaskOutcome::Cancelled, Some(_)) => {
                        if scheduling_disposition.is_some() {
                            return Err(SanitizationError::new(
                                SanitizationCode::InvalidTaskTerminal,
                                "scheduling_disposition",
                            ));
                        }
                    }
                    _ => {
                        return Err(SanitizationError::new(
                            SanitizationCode::InvalidTaskTerminal,
                            "failure_code",
                        ));
                    }
                }
            }
            Self::TerminalRejected { .. } => {}
        }
        Ok(())
    }

    fn diagnostic_code(&self) -> Option<DiagnosticCode> {
        match self {
            Self::TerminalCommitted {
                task_result: TaskOutcome::Failure | TaskOutcome::Cancelled,
                ..
            }
            | Self::TerminalRejected { .. }
            | Self::EntryRecoveryFailed { .. }
            | Self::EntryTargetDisposition {
                disposition: TaskEntryTargetDisposition::FailClosed,
                ..
            } => Some(DiagnosticCode::RuntimeDiagnostic),
            _ => None,
        }
    }

    fn effect_disposition(&self) -> Option<EffectDisposition> {
        match self {
            Self::EffectCompleted { .. }
            | Self::TerminalCommitted {
                task_result: TaskOutcome::Success,
                ..
            } => Some(EffectDisposition::Performed),
            Self::TerminalCommitted {
                task_result: TaskOutcome::Failure | TaskOutcome::Cancelled,
                ..
            } => Some(EffectDisposition::Indeterminate),
            Self::TerminalRejected { .. } => Some(EffectDisposition::NotPerformed),
            _ => None,
        }
    }
}

impl PayloadDetail for TaskSemanticPayload {
    fn action(&self) -> EventAction {
        self.action
    }

    fn diagnostic_code(&self) -> Option<DiagnosticCode> {
        self.fact.diagnostic_code()
    }

    fn effect_disposition(&self) -> Option<EffectDisposition> {
        self.fact.effect_disposition()
    }

    fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }
}

fn validate_task_semantic_label(value: &str, field: &'static str) -> Result<(), SanitizationError> {
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        Err(SanitizationError::new(
            SanitizationCode::InvalidTaskSemanticLabel,
            field,
        ))
    } else {
        Ok(())
    }
}

fn validate_task_frame_dimensions(width: u32, height: u32) -> Result<(), SanitizationError> {
    if width == 0 || height == 0 {
        Err(SanitizationError::new(
            SanitizationCode::InvalidTaskFrameDimensions,
            "frame_dimensions",
        ))
    } else {
        Ok(())
    }
}

fn validate_task_recognition_targets(
    targets: &[RecognizedTarget],
    frame_width: u32,
    frame_height: u32,
) -> Result<(), SanitizationError> {
    if targets.len() > TASK_RECOGNITION_TARGET_LIMIT {
        return Err(SanitizationError::new(
            SanitizationCode::InvalidTaskRecognitionTargets,
            "targets",
        ));
    }
    for (index, target) in targets.iter().enumerate() {
        if target.target_id.is_empty()
            || target.target_id.len() > TASK_RECOGNITION_TARGET_ID_MAX_BYTES
            || targets[..index]
                .iter()
                .any(|prior| prior.target_id == target.target_id)
        {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidTaskRecognitionTargets,
                "target_id",
            ));
        }
        if let Some(region) = target.region
            && (region.width == 0
                || region.height == 0
                || region
                    .x
                    .checked_add(region.width)
                    .is_none_or(|right| right > frame_width)
                || region
                    .y
                    .checked_add(region.height)
                    .is_none_or(|bottom| bottom > frame_height))
        {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidTaskRecognitionTargets,
                "region",
            ));
        }
    }
    Ok(())
}

fn validate_task_candidate_pages(pages: &[String]) -> Result<(), SanitizationError> {
    if pages.is_empty() || pages.len() > 1_024 {
        return Err(SanitizationError::new(
            SanitizationCode::InvalidTaskCandidatePages,
            "candidate_pages",
        ));
    }
    for (index, page) in pages.iter().enumerate() {
        validate_task_semantic_label(page, "candidate_pages")?;
        if pages[..index].contains(page) {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidTaskCandidatePages,
                "candidate_pages",
            ));
        }
    }
    Ok(())
}

fn validate_task_step(step_index: u32) -> Result<(), SanitizationError> {
    if step_index > 1_000 {
        Err(SanitizationError::new(
            SanitizationCode::InvalidTaskStepIndex,
            "step_index",
        ))
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryPayload {
    reason: RecoveryReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    migration: Option<Box<LedgerMigrationRecord>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    segment_index: Option<u64>,
    affected_bytes: u64,
    audit: SanitizedAudit,
}

trait PayloadDetail {
    fn resource_declaration(&self) -> Option<&crate::ResourceDeclarationRejection> {
        None
    }
    fn artifact_failure(&self) -> Option<&ArtifactFailureRecord> {
        None
    }
    fn runtime_state(&self) -> Option<&crate::RuntimeStateFact> {
        None
    }
    fn touch_response_us(&self) -> Option<u64> {
        None
    }
    fn capture_acquire_us(&self) -> Option<u64> {
        None
    }
    fn action(&self) -> EventAction;
    fn diagnostic_code(&self) -> Option<DiagnosticCode>;
    fn effect_disposition(&self) -> Option<EffectDisposition>;
    fn audit(&self) -> &SanitizedAudit;

    fn device_diagnostic_config(&self) -> Option<DeviceDiagnosticConfig> {
        None
    }

    fn diagnostic_detail(&self) -> Option<&DiagnosticDetailRecord> {
        None
    }

    fn cleanup_detail(&self) -> Option<&DiagnosticDetailRecord> {
        None
    }

    fn lifecycle_failure(&self) -> Option<&RuntimeLifecycleFailureRecord> {
        None
    }

    /// The outcome the record carries (ledger revision 3).
    fn outcome(&self) -> Option<&Outcome> {
        None
    }

    /// Workflow #389 (register U1): the channel and `via` of a command record.
    fn command_channel(&self) -> (Option<crate::RuntimeChannel>, Option<crate::ChannelVia>) {
        (None, None)
    }
}

macro_rules! common_detail_accessors {
    ($type:ty) => {
        impl $type {
            pub const fn action(&self) -> EventAction {
                self.action
            }

            pub fn audit(&self) -> &SanitizedAudit {
                &self.audit
            }
        }
    };
}

common_detail_accessors!(ObservationPayload);

/// Workflow #389 (register U1): the channel members of the three command record payloads.
macro_rules! command_channel_accessors {
    ($type:ty) => {
        impl $type {
            /// The channel of the command record's connection.
            pub const fn channel(&self) -> Option<crate::RuntimeChannel> {
                self.channel
            }

            /// The counter that handed a Lab request over.
            pub const fn via(&self) -> Option<crate::ChannelVia> {
                self.via
            }
        }
    };
}

command_channel_accessors!(ObservationPayload);
command_channel_accessors!(OutcomePayload);
command_channel_accessors!(DiagnosticOutcomePayload);
common_detail_accessors!(DiagnosticPayload);
common_detail_accessors!(OutcomePayload);
common_detail_accessors!(DiagnosticOutcomePayload);
common_detail_accessors!(RuntimeLifecyclePayload);
common_detail_accessors!(RuntimeInstanceBindingPayload);
common_detail_accessors!(RuntimeFactRecordedPayload);
common_detail_accessors!(RuntimeFactInvalidatedPayload);
common_detail_accessors!(RuntimeFactSnapshotPayload);
common_detail_accessors!(MonitorOutcomePayload);
common_detail_accessors!(MonitorRecoveryCoordinationPayload);
common_detail_accessors!(PerformancePressurePayload);
common_detail_accessors!(PerformanceStutterPayload);
common_detail_accessors!(PerformanceSummaryPayload);
common_detail_accessors!(PerformanceMonitorStatePayload);
common_detail_accessors!(PerformanceControlPayload);
common_detail_accessors!(FactPublishedPayload);
common_detail_accessors!(FactInvalidatedPayload);
common_detail_accessors!(ClientActionPayload);
common_detail_accessors!(ApprovalDecisionPayload);
common_detail_accessors!(SchedulerQueuePayload);
common_detail_accessors!(SchedulerPreemptionPayload);
common_detail_accessors!(LeaseTransferPayload);
common_detail_accessors!(ObservationResultPayload);
common_detail_accessors!(CapturePressurePayload);
common_detail_accessors!(CaptureDedupWindowPayload);
common_detail_accessors!(CapturePolicyPayload);
common_detail_accessors!(CaptureSummaryCommittedPayload);
common_detail_accessors!(ArtifactExportPayload);
common_detail_accessors!(ArtifactExportFailurePayload);

macro_rules! plain_payload_detail {
    ($($type:ty),+ $(,)?) => {
        $(
            impl PayloadDetail for $type {
                fn action(&self) -> EventAction {
                    self.action
                }

                fn diagnostic_code(&self) -> Option<DiagnosticCode> {
                    None
                }

                fn effect_disposition(&self) -> Option<EffectDisposition> {
                    None
                }

                fn audit(&self) -> &SanitizedAudit {
                    &self.audit
                }
            }
        )+
    };
}

plain_payload_detail!(
    RuntimeLifecyclePayload,
    RuntimeInstanceBindingPayload,
    RuntimeFactRecordedPayload,
    RuntimeFactInvalidatedPayload,
    RuntimeFactSnapshotPayload,
    PerformancePressurePayload,
    PerformanceStutterPayload,
    PerformanceSummaryPayload,
    PerformanceMonitorStatePayload,
    PerformanceControlPayload,
    FactPublishedPayload,
    FactInvalidatedPayload,
    ClientActionPayload,
    ApprovalDecisionPayload,
);

impl PerformancePressurePayload {
    pub const fn observed_at_unix_ms(&self) -> u64 {
        self.observed_at_unix_ms
    }

    pub const fn pressure(&self) -> &PerformancePressureRecord {
        &self.pressure
    }
}

impl PerformanceStutterPayload {
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    pub const fn observed_at_unix_ms(&self) -> u64 {
        self.observed_at_unix_ms
    }

    pub const fn frame_gap_ms(&self) -> u64 {
        self.frame_gap_ms
    }

    pub const fn capture_latency_ms(&self) -> Option<u64> {
        self.capture_latency_ms
    }

    pub const fn recognition_latency_ms(&self) -> Option<u64> {
        self.recognition_latency_ms
    }

    pub const fn action_effect_latency_ms(&self) -> Option<u64> {
        self.action_effect_latency_ms
    }
}

impl PerformanceSummaryPayload {
    pub const fn capacity(&self) -> Option<&crate::PerformanceCapacitySample> {
        self.capacity.as_ref()
    }
    pub const fn context(&self) -> &PerformanceContext {
        &self.context
    }

    pub const fn foreground(&self) -> Option<&crate::PerformanceForegroundSummary> {
        self.foreground.as_ref()
    }

    pub fn owned_processes(&self) -> &[crate::PerformanceProcessSummary] {
        &self.owned_processes
    }

    pub fn third_party_high_load(&self) -> &[crate::PerformanceProcessSummary] {
        &self.third_party_high_load
    }

    pub const fn ledger_commits(&self) -> Option<&crate::PerformanceLedgerSample> {
        self.ledger_commits.as_ref()
    }

    /// Workflow #385 (register P4): the pacing members.
    pub fn pacing(&self) -> Option<&PerformancePacingSummary> {
        self.pacing.as_deref()
    }
}

impl PerformanceMonitorStatePayload {
    pub const fn observed_at_unix_ms(&self) -> u64 {
        self.observed_at_unix_ms
    }

    pub const fn health(&self) -> PerformanceMonitorHealth {
        self.health
    }

    /// The failure that degraded the monitor.
    pub const fn outcome(&self) -> Option<&Outcome> {
        self.outcome.as_ref()
    }
}

impl PerformanceControlPayload {
    pub const fn observed_at_unix_ms(&self) -> u64 {
        self.observed_at_unix_ms
    }

    pub fn instance_id(&self) -> Option<&str> {
        self.instance_id.as_deref()
    }

    pub const fn level(&self) -> PerformanceControlLevel {
        self.level
    }

    pub const fn reason(&self) -> PerformanceControlReason {
        self.reason
    }

    pub const fn host_responsiveness_basis_points(&self) -> Option<u16> {
        self.host_responsiveness_basis_points
    }

    pub const fn third_party_pressure_basis_points(&self) -> Option<u16> {
        self.third_party_pressure_basis_points
    }

    pub const fn deadline_disposition(&self) -> Option<PerformanceDeadlineDisposition> {
        self.deadline_disposition
    }
}

impl FactPublishedPayload {
    pub fn record(&self) -> &FactRecord {
        self.record.as_ref()
    }

    pub fn records(&self) -> impl Iterator<Item = &FactRecord> {
        std::iter::once(self.record()).chain(self.related_records.iter())
    }

    pub fn scope_instances(&self) -> &[crate::InstanceId] {
        &self.scope_instances
    }
}

impl FactInvalidatedPayload {
    pub const fn invalidation(&self) -> &FactInvalidationEventData {
        &self.invalidation
    }
}

impl ClientActionPayload {
    pub const fn record(&self) -> &ClientActionRecord {
        &self.record
    }
}

impl ApprovalDecisionPayload {
    pub const fn decision(&self) -> &ApprovalDecisionRecord {
        &self.decision
    }
}

impl ResourceAuthoringPayload {
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

    /// The failure of a failed promotion.
    pub const fn outcome(&self) -> Option<&Outcome> {
        self.outcome.as_ref()
    }

    pub fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }
}

impl DiagnosticPayload {
    pub const fn diagnostic_code(&self) -> DiagnosticCode {
        self.diagnostic_code
    }

    /// The refusal this record reports (ledger revision 3).
    pub const fn outcome(&self) -> &Outcome {
        &self.outcome
    }
}

impl OutcomePayload {
    pub const fn effect_disposition(&self) -> EffectDisposition {
        self.effect_disposition
    }

    pub const fn touch_response_us(&self) -> Option<u64> {
        self.touch_response_us
    }
}

impl DiagnosticOutcomePayload {
    pub fn resource_declaration(&self) -> Option<&crate::ResourceDeclarationRejection> {
        self.resource_declaration.as_deref()
    }
    pub fn artifact_failure(&self) -> Option<&ArtifactFailureRecord> {
        self.artifact_failure.as_deref()
    }
    pub fn lifecycle_failure(&self) -> Option<&RuntimeLifecycleFailureRecord> {
        self.lifecycle_failure.as_deref()
    }
    pub const fn diagnostic_code(&self) -> DiagnosticCode {
        self.diagnostic_code
    }

    pub const fn effect_disposition(&self) -> EffectDisposition {
        self.effect_disposition
    }

    pub const fn detail(&self) -> Option<&DiagnosticDetailRecord> {
        self.detail.as_ref()
    }

    pub fn cleanup_detail(&self) -> Option<&DiagnosticDetailRecord> {
        self.cleanup_detail.as_deref()
    }

    /// The failure this record reports (ledger revision 3).
    pub const fn outcome(&self) -> &Outcome {
        &self.outcome
    }
}

impl MonitorOutcomePayload {
    pub const fn effect_disposition(&self) -> EffectDisposition {
        self.effect_disposition
    }

    pub const fn observation(&self) -> &MonitorObservation {
        &self.observation
    }

    pub const fn decision(&self) -> &MonitorDecision {
        &self.decision
    }
}

impl MonitorRecoveryCoordinationPayload {
    pub const fn effect_disposition(&self) -> EffectDisposition {
        self.effect_disposition
    }

    pub const fn recovery(&self) -> MonitorRecoveryKind {
        self.recovery
    }

    pub const fn reason(&self) -> MonitorRecoveryCoordinationReason {
        self.reason
    }
}

impl SchedulerQueuePayload {
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

impl SchedulerPreemptionPayload {
    pub const fn from_holder_id(&self) -> HolderId {
        self.from_holder_id
    }

    pub const fn from_lease_id(&self) -> LeaseId {
        self.from_lease_id
    }

    pub const fn queued_request_id(&self) -> RequestId {
        self.queued_request_id
    }

    pub const fn queued_priority(&self) -> LeasePriority {
        self.queued_priority
    }

    pub const fn deferred_by_destructive_step(&self) -> bool {
        self.deferred_by_destructive_step
    }
}

impl LeaseTransferPayload {
    pub const fn effect_disposition(&self) -> EffectDisposition {
        self.effect_disposition
    }

    pub const fn from_holder_id(&self) -> HolderId {
        self.from_holder_id
    }

    pub const fn from_lease_id(&self) -> LeaseId {
        self.from_lease_id
    }

    pub const fn to_holder_id(&self) -> HolderId {
        self.to_holder_id
    }

    pub const fn to_lease_id(&self) -> LeaseId {
        self.to_lease_id
    }

    pub const fn queued_request_id(&self) -> RequestId {
        self.queued_request_id
    }

    pub const fn priority(&self) -> LeasePriority {
        self.priority
    }
}

impl ObservationResultPayload {
    pub const fn effect_disposition(&self) -> EffectDisposition {
        self.effect_disposition
    }

    pub const fn frame_width(&self) -> u32 {
        self.frame_width
    }

    pub const fn frame_height(&self) -> u32 {
        self.frame_height
    }

    pub const fn recognition_verdict(&self) -> Option<RecognitionVerdict> {
        self.recognition_verdict
    }

    pub const fn capture_acquire_us(&self) -> Option<u64> {
        self.capture_acquire_us
    }
}

impl CapturePressurePayload {
    pub const fn state(&self) -> CapturePressureState {
        self.state
    }

    pub const fn memory_budget_bytes(&self) -> u64 {
        self.memory_budget_bytes
    }

    pub const fn resident_bytes(&self) -> u64 {
        self.resident_bytes
    }
}

impl CaptureDedupWindowPayload {
    pub fn preserved_frame_id(&self) -> Option<&crate::FrameId> {
        self.preserved_frame_id.as_ref()
    }

    pub const fn duplicate_count(&self) -> u64 {
        self.duplicate_count
    }

    pub const fn duration_ms(&self) -> u64 {
        self.duration_ms
    }
}

impl CapturePolicyPayload {
    pub const fn cadence_ms(&self) -> u64 {
        self.cadence_ms
    }

    pub const fn retention_class(&self) -> RetentionClass {
        self.retention_class
    }

    pub const fn reason(&self) -> CapturePolicyReason {
        self.reason
    }
}

impl ArtifactExportPayload {
    pub const fn effect_disposition(&self) -> EffectDisposition {
        self.effect_disposition
    }

    pub const fn task_outcome(&self) -> TaskOutcome {
        self.task_outcome
    }

    pub const fn evidence_completeness(&self) -> EvidenceCompleteness {
        self.evidence_completeness
    }

    pub const fn artifact_count(&self) -> u64 {
        self.artifact_count
    }
}

impl ArtifactExportFailurePayload {
    pub const fn diagnostic_code(&self) -> DiagnosticCode {
        self.diagnostic_code
    }

    pub const fn effect_disposition(&self) -> EffectDisposition {
        self.effect_disposition
    }

    pub const fn task_outcome(&self) -> TaskOutcome {
        self.task_outcome
    }

    pub const fn evidence_completeness(&self) -> EvidenceCompleteness {
        self.evidence_completeness
    }

    pub const fn artifact_count(&self) -> u64 {
        self.artifact_count
    }
}

impl PayloadDetail for ObservationPayload {
    fn device_diagnostic_config(&self) -> Option<DeviceDiagnosticConfig> {
        self.device_diagnostics
    }
    fn action(&self) -> EventAction {
        self.action
    }

    fn diagnostic_code(&self) -> Option<DiagnosticCode> {
        None
    }

    fn effect_disposition(&self) -> Option<EffectDisposition> {
        None
    }

    fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }

    fn command_channel(&self) -> (Option<crate::RuntimeChannel>, Option<crate::ChannelVia>) {
        (self.channel, self.via)
    }
}

impl PayloadDetail for InputIntentPayload {
    fn action(&self) -> EventAction {
        self.action
    }

    fn diagnostic_code(&self) -> Option<DiagnosticCode> {
        None
    }

    fn effect_disposition(&self) -> Option<EffectDisposition> {
        None
    }

    fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }
}

impl PayloadDetail for DiagnosticPayload {
    fn outcome(&self) -> Option<&Outcome> {
        Some(&self.outcome)
    }

    fn action(&self) -> EventAction {
        self.action
    }

    fn diagnostic_code(&self) -> Option<DiagnosticCode> {
        Some(self.diagnostic_code)
    }

    fn effect_disposition(&self) -> Option<EffectDisposition> {
        None
    }

    fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }
}

impl PayloadDetail for OutcomePayload {
    fn runtime_state(&self) -> Option<&crate::RuntimeStateFact> {
        self.runtime_state.as_deref()
    }
    fn touch_response_us(&self) -> Option<u64> {
        self.touch_response_us
    }
    fn action(&self) -> EventAction {
        self.action
    }

    fn diagnostic_code(&self) -> Option<DiagnosticCode> {
        None
    }

    fn effect_disposition(&self) -> Option<EffectDisposition> {
        Some(self.effect_disposition)
    }

    fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }

    fn command_channel(&self) -> (Option<crate::RuntimeChannel>, Option<crate::ChannelVia>) {
        (self.channel, self.via)
    }
}

impl PayloadDetail for DiagnosticOutcomePayload {
    fn resource_declaration(&self) -> Option<&crate::ResourceDeclarationRejection> {
        self.resource_declaration.as_deref()
    }
    fn artifact_failure(&self) -> Option<&ArtifactFailureRecord> {
        self.artifact_failure.as_deref()
    }
    fn runtime_state(&self) -> Option<&crate::RuntimeStateFact> {
        self.runtime_state.as_deref()
    }
    fn lifecycle_failure(&self) -> Option<&RuntimeLifecycleFailureRecord> {
        self.lifecycle_failure.as_deref()
    }
    fn action(&self) -> EventAction {
        self.action
    }

    fn diagnostic_code(&self) -> Option<DiagnosticCode> {
        Some(self.diagnostic_code)
    }

    fn effect_disposition(&self) -> Option<EffectDisposition> {
        Some(self.effect_disposition)
    }

    fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }

    fn diagnostic_detail(&self) -> Option<&DiagnosticDetailRecord> {
        self.detail.as_ref()
    }

    fn cleanup_detail(&self) -> Option<&DiagnosticDetailRecord> {
        self.cleanup_detail.as_deref()
    }

    fn outcome(&self) -> Option<&Outcome> {
        Some(&self.outcome)
    }

    fn command_channel(&self) -> (Option<crate::RuntimeChannel>, Option<crate::ChannelVia>) {
        (self.channel, self.via)
    }
}

impl PayloadDetail for MonitorOutcomePayload {
    fn runtime_state(&self) -> Option<&crate::RuntimeStateFact> {
        self.runtime_state.as_deref()
    }
    fn action(&self) -> EventAction {
        self.action
    }

    fn diagnostic_code(&self) -> Option<DiagnosticCode> {
        None
    }

    fn effect_disposition(&self) -> Option<EffectDisposition> {
        Some(self.effect_disposition)
    }

    fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }
}

impl PayloadDetail for MonitorRecoveryCoordinationPayload {
    fn action(&self) -> EventAction {
        self.action
    }

    fn diagnostic_code(&self) -> Option<DiagnosticCode> {
        None
    }

    fn effect_disposition(&self) -> Option<EffectDisposition> {
        Some(self.effect_disposition)
    }

    fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }
}

impl PayloadDetail for ObservationResultPayload {
    fn capture_acquire_us(&self) -> Option<u64> {
        self.capture_acquire_us
    }
    fn action(&self) -> EventAction {
        self.action
    }

    fn diagnostic_code(&self) -> Option<DiagnosticCode> {
        None
    }

    fn effect_disposition(&self) -> Option<EffectDisposition> {
        Some(self.effect_disposition)
    }

    fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }
}

macro_rules! observation_detail {
    ($type:ty) => {
        impl PayloadDetail for $type {
            fn action(&self) -> EventAction {
                self.action
            }

            fn diagnostic_code(&self) -> Option<DiagnosticCode> {
                None
            }

            fn effect_disposition(&self) -> Option<EffectDisposition> {
                None
            }

            fn audit(&self) -> &SanitizedAudit {
                &self.audit
            }
        }
    };
}

observation_detail!(CapturePressurePayload);
observation_detail!(CaptureDedupWindowPayload);
observation_detail!(CapturePolicyPayload);
observation_detail!(CaptureSummaryCommittedPayload);
observation_detail!(SchedulerQueuePayload);
observation_detail!(SchedulerPreemptionPayload);

impl PayloadDetail for PolicyDispatchPayload {
    fn action(&self) -> EventAction {
        self.action
    }

    fn diagnostic_code(&self) -> Option<DiagnosticCode> {
        self.diagnostic_code
    }

    fn effect_disposition(&self) -> Option<EffectDisposition> {
        self.effect_disposition
    }

    fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }
}

impl PayloadDetail for PolicyExecutionPayload {
    fn action(&self) -> EventAction {
        self.action
    }

    fn diagnostic_code(&self) -> Option<DiagnosticCode> {
        None
    }

    fn effect_disposition(&self) -> Option<EffectDisposition> {
        None
    }

    fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }

    /// A failed execution carries its failure (Workflow #378 A2-P), so the record's severity
    /// is the failure's category; a success carries none (its event type's category).
    fn outcome(&self) -> Option<&Outcome> {
        match &self.policy_result {
            PolicyExecutionOutcome::Failed { failure } => Some(&failure.outcome),
            PolicyExecutionOutcome::Succeeded { .. } => None,
        }
    }
}

impl PayloadDetail for PolicyPlanningSignalPayload {
    fn outcome(&self) -> Option<&Outcome> {
        Some(&self.outcome)
    }

    fn action(&self) -> EventAction {
        self.action
    }

    fn diagnostic_code(&self) -> Option<DiagnosticCode> {
        None
    }

    fn effect_disposition(&self) -> Option<EffectDisposition> {
        None
    }

    fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }
}

impl PayloadDetail for CatalogTransitionPayload {
    fn outcome(&self) -> Option<&Outcome> {
        self.outcome.as_ref()
    }

    fn action(&self) -> EventAction {
        self.action
    }

    fn diagnostic_code(&self) -> Option<DiagnosticCode> {
        self.diagnostic_code
    }

    fn effect_disposition(&self) -> Option<EffectDisposition> {
        self.effect_disposition
    }

    fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }
}

impl PayloadDetail for StateMigrationPayload {
    fn action(&self) -> EventAction {
        self.action
    }

    fn diagnostic_code(&self) -> Option<DiagnosticCode> {
        None
    }

    fn effect_disposition(&self) -> Option<EffectDisposition> {
        Some(EffectDisposition::Performed)
    }

    fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }
}

impl PayloadDetail for ReleaseStagedPayload {
    fn action(&self) -> EventAction {
        self.action
    }

    fn diagnostic_code(&self) -> Option<DiagnosticCode> {
        None
    }

    fn effect_disposition(&self) -> Option<EffectDisposition> {
        Some(self.effect_disposition)
    }

    fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }
}

impl PayloadDetail for ReleaseTransitionPayload {
    fn outcome(&self) -> Option<&Outcome> {
        self.outcome.as_ref()
    }

    fn action(&self) -> EventAction {
        self.action
    }

    fn diagnostic_code(&self) -> Option<DiagnosticCode> {
        self.diagnostic_code
    }

    fn effect_disposition(&self) -> Option<EffectDisposition> {
        self.effect_disposition
    }

    fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }
}

impl PayloadDetail for AgentWakePayload {
    fn action(&self) -> EventAction {
        self.action
    }

    fn diagnostic_code(&self) -> Option<DiagnosticCode> {
        None
    }

    fn effect_disposition(&self) -> Option<EffectDisposition> {
        None
    }

    fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }
}

impl PayloadDetail for AgentSessionPayload {
    fn action(&self) -> EventAction {
        self.action
    }

    fn diagnostic_code(&self) -> Option<DiagnosticCode> {
        None
    }

    fn effect_disposition(&self) -> Option<EffectDisposition> {
        None
    }

    fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }
}

impl PayloadDetail for LeaseTransferPayload {
    fn action(&self) -> EventAction {
        self.action
    }

    fn diagnostic_code(&self) -> Option<DiagnosticCode> {
        None
    }

    fn effect_disposition(&self) -> Option<EffectDisposition> {
        Some(self.effect_disposition)
    }

    fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }
}

impl PayloadDetail for ArtifactExportPayload {
    fn action(&self) -> EventAction {
        self.action
    }

    fn diagnostic_code(&self) -> Option<DiagnosticCode> {
        None
    }

    fn effect_disposition(&self) -> Option<EffectDisposition> {
        Some(self.effect_disposition)
    }

    fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }
}

impl PayloadDetail for ArtifactExportFailurePayload {
    fn outcome(&self) -> Option<&Outcome> {
        Some(&self.outcome)
    }

    fn action(&self) -> EventAction {
        self.action
    }

    fn diagnostic_code(&self) -> Option<DiagnosticCode> {
        Some(self.diagnostic_code)
    }

    fn effect_disposition(&self) -> Option<EffectDisposition> {
        Some(self.effect_disposition)
    }

    fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }
}

impl PayloadDetail for ResourceAuthoringPayload {
    fn action(&self) -> EventAction {
        resource_authoring_action(self.phase)
    }

    fn diagnostic_code(&self) -> Option<DiagnosticCode> {
        None
    }

    fn effect_disposition(&self) -> Option<EffectDisposition> {
        Some(match self.phase {
            ResourceAuthoringPhase::Promoted => EffectDisposition::Performed,
            ResourceAuthoringPhase::PromoteFailed => EffectDisposition::Indeterminate,
            _ => EffectDisposition::NotPerformed,
        })
    }

    fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }
}

impl RecoveryPayload {
    pub fn migration(&self) -> Option<&LedgerMigrationRecord> {
        self.migration.as_deref()
    }
    pub const fn reason(&self) -> RecoveryReason {
        self.reason
    }

    pub const fn segment_index(&self) -> Option<u64> {
        self.segment_index
    }

    pub const fn affected_bytes(&self) -> u64 {
        self.affected_bytes
    }

    pub fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }
}

impl PayloadDetail for RecoveryPayload {
    fn action(&self) -> EventAction {
        EventAction::LedgerRecovery
    }

    fn diagnostic_code(&self) -> Option<DiagnosticCode> {
        None
    }

    fn effect_disposition(&self) -> Option<EffectDisposition> {
        None
    }

    fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }
}

struct ObservationDraft {
    action: EventAction,
    device_diagnostics: Option<DeviceDiagnosticConfig>,
    /// Workflow #389 U-2 (register U1): the channel of a `command.received` record.
    channel: Option<crate::ChannelBinding>,
    audit: AuditInput,
}

struct InputIntentDraft {
    action: EventAction,
    execution_plan: Option<InputExecutionPlanRecord>,
    provenance: Option<InputIntentProvenance>,
    audit: AuditInput,
}

struct DiagnosticDraft {
    action: EventAction,
    diagnostic_code: DiagnosticCode,
    audit: AuditInput,
}

/// The outcome a diagnostic record carries when its producer gives none: its diagnostic code.
fn diagnostic_code_outcome(code: DiagnosticCode) -> Outcome {
    Outcome::new(code.code())
}

struct OutcomeDraft {
    action: EventAction,
    effect_disposition: EffectDisposition,
    runtime_state: Option<Box<crate::RuntimeStateFact>>,
    touch_response_us: Option<u64>,
    audit: AuditInput,
}

struct DiagnosticOutcomeDraft {
    action: EventAction,
    diagnostic_code: DiagnosticCode,
    effect_disposition: EffectDisposition,
    outcome: Option<Outcome>,
    detail: Option<DiagnosticDetailDraft>,
    cleanup_cause: Option<Box<CleanupCauseDraft>>,
    lifecycle_failure: Option<Box<RuntimeLifecycleFailureDraft>>,
    artifact_failure: Option<Box<ArtifactFailureDraft>>,
    resource_declaration: Option<Box<crate::ResourceDeclarationRejection>>,
    runtime_state: Option<Box<crate::RuntimeStateFact>>,
    audit: AuditInput,
}

struct MonitorOutcomeDraft {
    action: EventAction,
    effect_disposition: EffectDisposition,
    observation: MonitorObservation,
    decision: MonitorDecision,
    runtime_state: Option<Box<crate::RuntimeStateFact>>,
    audit: AuditInput,
}

struct MonitorRecoveryCoordinationDraft {
    recovery: MonitorRecoveryKind,
    reason: MonitorRecoveryCoordinationReason,
    admitted: bool,
    audit: AuditInput,
}

struct SchedulerQueueDraft {
    action: EventAction,
    priority: LeasePriority,
    position: u32,
    deadline_monotonic_ms: u64,
    preempt_requested: bool,
    audit: AuditInput,
}

struct SchedulerPreemptionDraft {
    action: EventAction,
    from_holder_id: HolderId,
    from_lease_id: LeaseId,
    queued_request_id: RequestId,
    queued_priority: LeasePriority,
    deferred_by_destructive_step: bool,
    audit: AuditInput,
}

struct LeaseTransferDraft {
    action: EventAction,
    effect_disposition: EffectDisposition,
    from_holder_id: HolderId,
    from_lease_id: LeaseId,
    to_holder_id: HolderId,
    to_lease_id: LeaseId,
    queued_request_id: RequestId,
    priority: LeasePriority,
    audit: AuditInput,
}

struct ObservationResultDraft {
    action: EventAction,
    effect_disposition: EffectDisposition,
    frame_width: u32,
    frame_height: u32,
    recognition_verdict: Option<RecognitionVerdict>,
    capture_acquire_us: Option<u64>,
    audit: AuditInput,
}

struct CapturePressureDraft {
    action: EventAction,
    state: CapturePressureState,
    memory_budget_bytes: u64,
    resident_bytes: u64,
    audit: AuditInput,
}

struct CaptureDedupWindowDraft {
    action: EventAction,
    duplicate_count: u64,
    duration_ms: u64,
    preserved_frame_id: Option<crate::FrameId>,
    preserving_material: bool,
    audit: AuditInput,
}

struct CapturePolicyDraft {
    action: EventAction,
    cadence_ms: u64,
    retention_class: RetentionClass,
    reason: CapturePolicyReason,
    audit: AuditInput,
}

struct CaptureSummaryCommittedDraft {
    summary: CaptureSummaryRecord,
    audit: AuditInput,
}

struct ArtifactExportDraft {
    action: EventAction,
    effect_disposition: EffectDisposition,
    task_outcome: TaskOutcome,
    evidence_completeness: EvidenceCompleteness,
    artifact_count: u64,
    audit: AuditInput,
}

struct ArtifactExportFailureDraft {
    action: EventAction,
    diagnostic_code: DiagnosticCode,
    effect_disposition: EffectDisposition,
    task_outcome: TaskOutcome,
    evidence_completeness: EvidenceCompleteness,
    artifact_count: u64,
    audit: AuditInput,
}

struct RecoveryDraft {
    reason: RecoveryReason,
    migration: Option<Box<LedgerMigrationRecord>>,
    segment_index: Option<u64>,
    affected_bytes: u64,
    audit: AuditInput,
}

struct ResourceAuthoringDraft {
    phase: ResourceAuthoringPhase,
    draft_id: String,
    target_label: String,
    target_fingerprint: String,
    changed_paths: Vec<String>,
    outcome: Option<Outcome>,
    audit: AuditInput,
}

struct PolicyDispatchDraft {
    data: PolicyDispatchEventData,
    admission: Option<Box<PolicyAdmissionRecord>>,
    rejection: Option<Box<PolicyDispatchRejection>>,
    diagnostic_code: Option<DiagnosticCode>,
    effect_disposition: Option<EffectDisposition>,
    /// Workflow #405 D1 (register X14): set only by
    /// `PolicyPayloadDraft::dispatch_intent_with_random_delay`.
    random_delay: Option<PolicyRandomDelay>,
    audit: AuditInput,
}

struct PolicyExecutionDraft {
    data: PolicyExecutionEventData,
    /// Workflow #385 P-k (register P6): the run's wall time and budget time, for a run settled
    /// on its budget time.
    elapsed: Option<(u64, u64)>,
    audit: AuditInput,
}

struct PerformancePressureDraft {
    data: PerformancePressureEventData,
    audit: AuditInput,
}

struct PerformanceStutterDraft {
    data: PerformanceStutterEventData,
    audit: AuditInput,
}

struct PerformanceSummaryDraft {
    data: PerformanceSummaryEventData,
    audit: AuditInput,
}

struct PerformanceMonitorStateDraft {
    data: PerformanceMonitorStateEventData,
    audit: AuditInput,
}

struct PerformanceControlDraft {
    data: PerformanceControlEventData,
    audit: AuditInput,
}

/// Workflow #385 P-m: a pacing transition (`perf.pacing_changed`), its code and values in the
/// outcome.
struct PerformancePacingDraft {
    outcome: Outcome,
    audit: AuditInput,
}

struct FactPublishedDraft {
    record: Box<FactRecord>,
    related_records: Vec<FactRecord>,
    scope_instances: Vec<crate::InstanceId>,
    audit: AuditInput,
}

struct FactInvalidatedDraft {
    invalidation: FactInvalidationEventData,
    audit: AuditInput,
}

struct ClientActionDraft {
    record: ClientActionRecord,
    audit: AuditInput,
}

struct ApprovalDecisionDraft {
    decision: ApprovalDecisionRecord,
    audit: AuditInput,
}

struct PolicyPlanningSignalDraft {
    data: PolicyPlanningSignalEventData,
    audit: AuditInput,
}

struct CatalogTransitionDraft {
    action: EventAction,
    data: CatalogTransitionEventData,
    diagnostic_code: Option<DiagnosticCode>,
    effect_disposition: Option<EffectDisposition>,
    audit: AuditInput,
}

struct StateMigrationDraft {
    migration: StateMigrationData,
    audit: AuditInput,
}

struct ReleaseStagedDraft {
    manifest: RuntimeReleaseSet,
    audit: AuditInput,
}

struct ReleaseTransitionDraft {
    action: EventAction,
    transition: ReleaseTransitionData,
    diagnostic_code: Option<DiagnosticCode>,
    effect_disposition: Option<EffectDisposition>,
    audit: AuditInput,
}

struct AgentWakeDraft {
    wake: AgentWakeData,
    audit: AuditInput,
}

struct AgentSessionDraft {
    action: EventAction,
    session: AgentSessionEventData,
    audit: AuditInput,
}

struct TaskSemanticDraft {
    fact: TaskSemanticFact,
    lease_expires_at_monotonic_ms: Option<u64>,
    sampling: Option<InputSamplingEvidence>,
    audit: AuditInput,
}

impl TaskSemanticDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<TaskSemanticPayload, SanitizationError> {
        let mut fact = self.fact;
        fact.validate()?;
        fact.redact_sensitive_input();
        let payload = TaskSemanticPayload {
            action: EventAction::RuntimeTaskRun,
            fact,
            lease_expires_at_monotonic_ms: self.lease_expires_at_monotonic_ms,
            sampling: self.sampling,
            audit: self.audit.sanitize(fingerprinter)?,
        };
        payload.validate()?;
        Ok(payload)
    }
}

impl ResourceAuthoringDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<ResourceAuthoringPayload, SanitizationError> {
        validate_resource_authoring_fields(
            self.phase,
            &self.draft_id,
            &self.target_label,
            &self.target_fingerprint,
            &self.changed_paths,
            self.outcome.as_ref(),
        )?;
        Ok(ResourceAuthoringPayload {
            phase: self.phase,
            draft_id: self.draft_id,
            target_label: self.target_label,
            target_fingerprint: self.target_fingerprint,
            changed_paths: self.changed_paths,
            outcome: self.outcome,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl PolicyDispatchDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<PolicyDispatchPayload, SanitizationError> {
        validate_policy_dispatch_data(&self.data)?;
        if let Some(rejection) = &self.rejection {
            rejection.validate()?;
        }
        if let Some(random_delay) = &self.random_delay {
            random_delay.validate()?;
        }
        Ok(PolicyDispatchPayload {
            action: EventAction::PolicyDispatch,
            diagnostic_code: self.diagnostic_code,
            effect_disposition: self.effect_disposition,
            decision_id: self.data.decision_id,
            task_id: self.data.task_id,
            instance_id: self.data.instance_id,
            operation_id: self.data.operation_id,
            procedure_binding_digest: self.data.procedure_binding_digest,
            reason_chain_id: self.data.reason_chain_id,
            reasons: self.data.reasons,
            catalog_hash: self.data.catalog_hash,
            catalog_version: self.data.catalog_version,
            input_ledger_position: self.data.input_ledger_position,
            input_fact_revisions: None,
            fact_snapshot_id: self.data.fact_snapshot_id,
            approval_fact_ids: self.data.approval_fact_ids,
            urgency_milli: self.data.urgency_milli,
            priority_locked: false,
            random_delay: self.random_delay,
            admission: self.admission,
            rejection: self.rejection,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl PolicyExecutionDraft {
    /// Workflow #385 P-k (register P6): the record of a run settled on its budget time carries
    /// its wall time `wall_elapsed_ms` and that budget time `budget_elapsed_ms`.
    fn with_elapsed(self, wall_elapsed_ms: u64, budget_elapsed_ms: u64) -> Self {
        Self {
            elapsed: Some((wall_elapsed_ms, budget_elapsed_ms)),
            ..self
        }
    }

    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<PolicyExecutionPayload, SanitizationError> {
        validate_policy_execution_data(&self.data)?;
        // A run's budget time never exceeds its wall time (PERF §5.3: pacing only lengthens).
        if self
            .elapsed
            .is_some_and(|(wall_elapsed_ms, budget_elapsed_ms)| budget_elapsed_ms > wall_elapsed_ms)
        {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidPolicyExecutionTime,
                "budget_elapsed_ms",
            ));
        }
        Ok(PolicyExecutionPayload {
            action: EventAction::PolicyExecution,
            decision_id: self.data.decision_id,
            task_id: self.data.task_id,
            instance_id: self.data.instance_id,
            observed_at_unix_ms: self.data.observed_at_unix_ms,
            policy_result: self.data.outcome,
            wall_elapsed_ms: self.elapsed.map(|(wall_elapsed_ms, _)| wall_elapsed_ms),
            budget_elapsed_ms: self.elapsed.map(|(_, budget_elapsed_ms)| budget_elapsed_ms),
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl PerformancePressureDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<PerformancePressurePayload, SanitizationError> {
        if self.data.observed_at_unix_ms == 0
            || self.data.pressure.started_at_unix_ms > self.data.pressure.last_observed_at_unix_ms
            || self.data.pressure.last_observed_at_unix_ms > self.data.observed_at_unix_ms
        {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidPerformancePressureTime,
                "performance_pressure",
            ));
        }
        self.data.pressure.validate()?;
        Ok(PerformancePressurePayload {
            action: EventAction::PerformanceObserve,
            observed_at_unix_ms: self.data.observed_at_unix_ms,
            pressure: self.data.pressure,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl PerformanceStutterDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<PerformanceStutterPayload, SanitizationError> {
        validate_performance_stutter(&self.data)?;
        Ok(PerformanceStutterPayload {
            action: EventAction::PerformanceObserve,
            instance_id: self.data.instance_id,
            observed_at_unix_ms: self.data.observed_at_unix_ms,
            frame_gap_ms: self.data.frame_gap_ms,
            capture_latency_ms: self.data.capture_latency_ms,
            recognition_latency_ms: self.data.recognition_latency_ms,
            action_effect_latency_ms: self.data.action_effect_latency_ms,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl PerformanceSummaryDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<PerformanceSummaryPayload, SanitizationError> {
        validate_performance_summary(&self.data)?;
        Ok(PerformanceSummaryPayload {
            action: EventAction::PerformanceObserve,
            context: self.data.context,
            foreground: self.data.foreground,
            owned_processes: self.data.owned_processes,
            third_party_high_load: self.data.third_party_high_load,
            ledger_commits: self.data.ledger_commits,
            capacity: self.data.capacity,
            // Workflow #385 P-m (register P4): validated with the rest of the summary above.
            pacing: self.data.pacing.map(Box::new),
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl PerformanceMonitorStateDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<PerformanceMonitorStatePayload, SanitizationError> {
        validate_performance_monitor_state(&self.data)?;
        Ok(PerformanceMonitorStatePayload {
            action: EventAction::PerformanceObserve,
            observed_at_unix_ms: self.data.observed_at_unix_ms,
            health: self.data.health,
            outcome: self.data.outcome,
            consecutive_failures: self.data.consecutive_failures,
            terminal: self.data.terminal,
            unavailable_metrics: self.data.unavailable_metrics,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl PerformanceControlDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<PerformanceControlPayload, SanitizationError> {
        validate_performance_control(&self.data)?;
        Ok(PerformanceControlPayload {
            action: EventAction::PerformanceObserve,
            observed_at_unix_ms: self.data.observed_at_unix_ms,
            instance_id: self.data.instance_id,
            previous_level: self.data.previous_level,
            level: self.data.level,
            reason: self.data.reason,
            host_responsiveness_basis_points: self.data.host_responsiveness_basis_points,
            third_party_pressure_basis_points: self.data.third_party_pressure_basis_points,
            recovery: self.data.recovery,
            deadline_disposition: self.data.deadline_disposition,
            arbitration: self.data.arbitration,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl FactPublishedDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<FactPublishedPayload, SanitizationError> {
        crate::fact::validate_fact_observation(
            std::iter::once(self.record.as_ref()).chain(self.related_records.iter()),
        )?;
        Ok(FactPublishedPayload {
            action: EventAction::FactPublish,
            record: self.record,
            related_records: self.related_records,
            scope_instances: self.scope_instances,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl FactInvalidatedDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<FactInvalidatedPayload, SanitizationError> {
        validate_fact_invalidation(&self.invalidation)?;
        Ok(FactInvalidatedPayload {
            action: EventAction::FactInvalidate,
            invalidation: self.invalidation,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl ClientActionDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<ClientActionPayload, SanitizationError> {
        self.record.validate()?;
        Ok(ClientActionPayload {
            action: EventAction::ClientAction,
            record: self.record,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl ApprovalDecisionDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<ApprovalDecisionPayload, SanitizationError> {
        self.decision.validate()?;
        Ok(ApprovalDecisionPayload {
            action: EventAction::ApprovalDecision,
            decision: self.decision,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl PolicyPlanningSignalDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<PolicyPlanningSignalPayload, SanitizationError> {
        validate_policy_planning_signal_data(&self.data)?;
        Ok(PolicyPlanningSignalPayload {
            action: EventAction::PolicyPlanning,
            signal_id: self.data.signal_id,
            instance_id: self.data.instance_id,
            task_id: self.data.task_id,
            kind: self.data.kind,
            outcome: self.data.outcome,
            observed_at_unix_ms: self.data.observed_at_unix_ms,
            detection_budget: self.data.detection_budget,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl CatalogTransitionDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<CatalogTransitionPayload, SanitizationError> {
        validate_catalog_transition_data(&self.data)?;
        Ok(CatalogTransitionPayload {
            action: self.action,
            diagnostic_code: self.diagnostic_code,
            outcome: self.diagnostic_code.map(diagnostic_code_outcome),
            effect_disposition: self.effect_disposition,
            catalog_id: self.data.catalog_id,
            catalog_version: self.data.catalog_version,
            catalog_hash: self.data.catalog_hash,
            previous_catalog_hash: self.data.previous_catalog_hash,
            promotion: self.data.promotion,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl StateMigrationDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<StateMigrationPayload, SanitizationError> {
        self.migration.validate()?;
        Ok(StateMigrationPayload {
            action: EventAction::StateMigrate,
            migration: self.migration,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl ReleaseStagedDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<ReleaseStagedPayload, SanitizationError> {
        self.manifest.validate()?;
        Ok(ReleaseStagedPayload {
            action: EventAction::ReleaseStage,
            effect_disposition: EffectDisposition::Performed,
            manifest: self.manifest,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl ReleaseTransitionDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<ReleaseTransitionPayload, SanitizationError> {
        self.transition.validate()?;
        Ok(ReleaseTransitionPayload {
            action: self.action,
            diagnostic_code: self.diagnostic_code,
            outcome: self.diagnostic_code.map(diagnostic_code_outcome),
            effect_disposition: self.effect_disposition,
            transition: self.transition,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl AgentWakeDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<AgentWakePayload, SanitizationError> {
        self.wake.validate()?;
        Ok(AgentWakePayload {
            action: EventAction::AgentWake,
            wake: self.wake,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl AgentSessionDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<AgentSessionPayload, SanitizationError> {
        self.session.validate()?;
        Ok(AgentSessionPayload {
            action: self.action,
            session: self.session,
            report_artifact: None,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

const fn resource_authoring_action(phase: ResourceAuthoringPhase) -> EventAction {
    match phase {
        ResourceAuthoringPhase::AuthoringStarted => EventAction::ResourceAuthoringStart,
        ResourceAuthoringPhase::DraftBuilt => EventAction::ResourceDraftBuild,
        ResourceAuthoringPhase::ValidationCompleted => EventAction::ResourceValidation,
        ResourceAuthoringPhase::PromoteIntent
        | ResourceAuthoringPhase::Promoted
        | ResourceAuthoringPhase::PromoteFailed => EventAction::ResourcePromote,
    }
}

pub(crate) fn validate_resource_authoring_fields(
    phase: ResourceAuthoringPhase,
    draft_id: &str,
    target_label: &str,
    target_fingerprint: &str,
    changed_paths: &[String],
    outcome: Option<&Outcome>,
) -> Result<(), SanitizationError> {
    validate_resource_authoring_token(draft_id, 128, "draft_id")?;
    validate_resource_authoring_token(target_label, 128, "target_label")?;
    if target_fingerprint.len() != 64
        || !target_fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(SanitizationError::new(
            SanitizationCode::InvalidResourceAuthoringFingerprint,
            "target_fingerprint",
        ));
    }
    if changed_paths.is_empty() || changed_paths.len() > 4_096 {
        return Err(SanitizationError::new(
            SanitizationCode::InvalidResourceAuthoringPaths,
            "changed_paths",
        ));
    }
    for path in changed_paths {
        let valid = path.len() <= 1_024
            && !path.starts_with('/')
            && !path.contains(['\\', ':'])
            && !path.chars().any(char::is_control)
            && path
                .split('/')
                .all(|component| !component.is_empty() && component != "." && component != "..");
        if !valid {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidResourceAuthoringPath,
                "changed_paths",
            ));
        }
    }
    match (phase, outcome) {
        (ResourceAuthoringPhase::PromoteFailed, Some(_)) => {}
        (ResourceAuthoringPhase::PromoteFailed, None) => {
            return Err(SanitizationError::new(
                SanitizationCode::MissingResourceAuthoringFailureCode,
                "failure_code",
            ));
        }
        (_, Some(_)) => {
            return Err(SanitizationError::new(
                SanitizationCode::UnexpectedResourceAuthoringFailureCode,
                "failure_code",
            ));
        }
        (_, None) => {}
    }
    Ok(())
}

/// A client's resource authoring failure code, before the Runtime records it as an outcome.
pub(crate) fn validate_resource_authoring_failure_token(
    value: &str,
) -> Result<(), SanitizationError> {
    validate_resource_authoring_token(value, 128, "failure_code")
}

fn validate_resource_authoring_token(
    value: &str,
    max_bytes: usize,
    field: &'static str,
) -> Result<(), SanitizationError> {
    if value.is_empty()
        || value.len() > max_bytes
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
    {
        return Err(SanitizationError::new(
            SanitizationCode::InvalidResourceAuthoringToken,
            field,
        ));
    }
    Ok(())
}

fn validate_policy_dispatch_data(data: &PolicyDispatchEventData) -> Result<(), SanitizationError> {
    validate_policy_dispatch_fields(data, data.input_ledger_position != 0)
}

/// The dispatch data's checks; `input_read` says the decision names what it read: a ledger
/// position, or (a stored record, register L9) both stores' revisions.
fn validate_policy_dispatch_fields(
    data: &PolicyDispatchEventData,
    input_read: bool,
) -> Result<(), SanitizationError> {
    validate_policy_token(&data.decision_id, "decision_id")?;
    validate_policy_token(&data.task_id, "task_id")?;
    crate::validate_instance_alias(&data.instance_id)
        .map_err(|_| SanitizationError::new(SanitizationCode::InvalidPolicyToken, "instance_id"))?;
    validate_policy_token(&data.operation_id, "operation_id")?;
    validate_policy_digest(&data.procedure_binding_digest, "procedure_binding_digest")?;
    validate_policy_token(&data.reason_chain_id, "reason_chain_id")?;
    validate_policy_token(&data.fact_snapshot_id, "fact_snapshot_id")?;
    validate_catalog_hash(&data.catalog_hash, "catalog_hash")?;
    if data.catalog_version == 0 || !input_read || data.urgency_milli > 1_000 {
        return Err(SanitizationError::new(
            SanitizationCode::InvalidPolicyDispatchPosition,
            "catalog_version_or_input_position",
        ));
    }
    if data.reasons.is_empty() || data.reasons.len() > 128 {
        return Err(SanitizationError::new(
            SanitizationCode::InvalidPolicyReasonChain,
            "reasons",
        ));
    }
    for reason in &data.reasons {
        validate_policy_token(reason.0.code().as_str(), "reason_code")?;
    }
    if data.approval_fact_ids.len() > 64 {
        return Err(SanitizationError::new(
            SanitizationCode::InvalidPolicyApprovalFacts,
            "approval_fact_ids",
        ));
    }
    for approval in &data.approval_fact_ids {
        validate_policy_token(approval, "approval_fact_ids")?;
    }
    Ok(())
}

fn validate_policy_admission(value: &PolicyAdmissionRecord) -> Result<(), SanitizationError> {
    validate_policy_token(&value.activity.profile_id, "activity_profile_id")?;
    validate_policy_token(&value.activity.window_id, "activity_window_id")?;
    if value.activity.admitted_at_unix_ms == 0
        || value.activity.next_eligible_unix_ms < value.activity.admitted_at_unix_ms
        || value.activity.next_eligible_unix_ms - value.activity.admitted_at_unix_ms
            != value.activity.interval_ms
    {
        return Err(SanitizationError::new(
            SanitizationCode::InvalidPolicyActivitySample,
            "activity",
        ));
    }
    let budget = &value.budget;
    if budget.task_daily_used == 0
        || budget.task_daily_used > budget.task_daily_limit
        || budget.task_window_used == 0
        || budget.task_window_used > budget.task_window_limit
        || budget.task_runtime_reserved_ms == 0
        || budget.task_runtime_reserved_ms > budget.task_runtime_limit_ms
        || budget.activity_daily_used == 0
        || budget.activity_daily_used > budget.activity_daily_limit
        || budget.activity_window_used == 0
        || budget.activity_window_used > budget.activity_window_limit
        || budget.activity_runtime_reserved_ms == 0
        || budget.activity_runtime_reserved_ms > budget.activity_runtime_limit_ms
    {
        return Err(SanitizationError::new(
            SanitizationCode::InvalidPolicyBudgetReceipt,
            "budget",
        ));
    }
    Ok(())
}

fn validate_policy_execution_data(
    data: &PolicyExecutionEventData,
) -> Result<(), SanitizationError> {
    validate_policy_token(&data.decision_id, "decision_id")?;
    validate_policy_token(&data.task_id, "task_id")?;
    crate::validate_instance_alias(&data.instance_id)
        .map_err(|_| SanitizationError::new(SanitizationCode::InvalidPolicyToken, "instance_id"))?;
    if data.observed_at_unix_ms == 0 {
        return Err(SanitizationError::new(
            SanitizationCode::InvalidPolicyExecutionTime,
            "observed_at_unix_ms",
        ));
    }
    if let PolicyExecutionOutcome::Failed { failure } = &data.outcome {
        if let Some(identity) = &failure.failure_identity {
            validate_policy_token(identity, "failure_identity")?;
        }
        failure.perf_context.validate()?;
        // Workflow #386 (register S1, SCHED §4.2): a condition termination counts from 1 in its
        // occurrence, keeps the pair's previous counts (0 when none) and its disposition is
        // `continue`.
        if failure.condition.is_some_and(|condition| {
            condition.count == 0
                || condition.expires_at_unix_ms == 0
                || failure.disposition != PolicyFailureDisposition::Continue
        }) {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidPolicyFailureRecord,
                "condition",
            ));
        }
        let counted = failure.condition.is_none();
        let retry_scheduled = failure.disposition == PolicyFailureDisposition::RetryScheduled;
        if (counted && failure.consecutive_same_error == 0)
            || failure.escalation_streak > failure.consecutive_same_error
            || (counted && !failure.performance_tax_exempt && failure.escalation_streak == 0)
            || failure.performance_tax_exempt
                && (failure.original_class != PolicyFailureClass::Recoverable
                    || failure.effective_class != PolicyFailureClass::Recoverable
                    || failure.sensitive
                    || failure.reported_success
                    || !failure.perf_context.pressure_observed())
            || retry_scheduled != failure.retry_at_unix_ms.is_some()
            || retry_scheduled
                && (failure.retry_attempt == 0
                    || failure.effective_class != PolicyFailureClass::Recoverable
                    || failure
                        .retry_at_unix_ms
                        .is_some_and(|retry_at| retry_at <= data.observed_at_unix_ms))
            || !retry_scheduled && failure.retry_attempt != 0
            || failure.original_class == PolicyFailureClass::Severe
                && failure.effective_class != PolicyFailureClass::Severe
            || failure.effective_class == PolicyFailureClass::Severe
                && failure.disposition != PolicyFailureDisposition::PausedTask
            || failure.reported_success
                && (!failure.outcome.is(POLICY_RUNTIME_BUDGET_EXCEEDED)
                    || failure.original_class != PolicyFailureClass::Severe
                    || failure.effective_class != PolicyFailureClass::Severe)
            || failure.sensitive
                && (failure.effective_class != PolicyFailureClass::Severe
                    || failure.disposition != PolicyFailureDisposition::PausedTask)
        {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidPolicyFailureRecord,
                "outcome",
            ));
        }
    }
    Ok(())
}

fn validate_policy_planning_signal_data(
    data: &PolicyPlanningSignalEventData,
) -> Result<(), SanitizationError> {
    validate_policy_token(&data.signal_id, "signal_id")?;
    crate::validate_instance_alias(&data.instance_id)
        .map_err(|_| SanitizationError::new(SanitizationCode::InvalidPolicyToken, "instance_id"))?;
    if let Some(task_id) = &data.task_id {
        validate_policy_token(task_id, "task_id")?;
    }
    validate_policy_token(data.outcome.code().as_str(), "outcome")?;
    if data.observed_at_unix_ms == 0 {
        return Err(SanitizationError::new(
            SanitizationCode::InvalidPolicyPlanningSignalTime,
            "observed_at_unix_ms",
        ));
    }
    let detection_kind = matches!(
        data.kind,
        PolicyPlanningSignalKind::DetectionReserved
            | PolicyPlanningSignalKind::DetectionQuotaExhausted
    );
    if detection_kind != data.detection_budget.is_some() {
        return Err(SanitizationError::new(
            SanitizationCode::InvalidPolicyDetectionBudgetPresence,
            "detection_budget",
        ));
    }
    if let Some(budget) = &data.detection_budget {
        validate_catalog_hash(&budget.catalog_hash, "detection_budget.catalog_hash")?;
        validate_policy_token(&budget.profile_id, "detection_budget.profile_id")?;
        validate_policy_token(&budget.window_id, "detection_budget.window_id")?;
        let exhausted = budget.dispatch_used >= budget.dispatch_limit
            || budget
                .runtime_reserved_ms
                .checked_add(budget.reservation_ms)
                .is_none_or(|next| next > budget.runtime_limit_ms);
        if budget.dispatch_limit == 0
            || budget.runtime_limit_ms == 0
            || budget.reservation_ms == 0
            || data.kind == PolicyPlanningSignalKind::DetectionReserved
                && (budget.dispatch_used == 0
                    || budget.runtime_reserved_ms == 0
                    || budget.runtime_reserved_ms < budget.reservation_ms
                    || budget.dispatch_used > budget.dispatch_limit
                    || budget.runtime_reserved_ms > budget.runtime_limit_ms)
            || data.kind == PolicyPlanningSignalKind::DetectionQuotaExhausted && !exhausted
        {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidPolicyDetectionBudget,
                "detection_budget",
            ));
        }
    }
    Ok(())
}

fn validate_runtime_instance_binding(
    payload: &RuntimeInstanceBindingPayload,
) -> Result<(), SanitizationError> {
    if payload.action != EventAction::RuntimeAction {
        return Err(SanitizationError::new(
            SanitizationCode::InvalidRuntimeInstanceBindingAction,
            "runtime_payload",
        ));
    }
    crate::validate_instance_alias(&payload.instance_alias)
        .map_err(|_| SanitizationError::new(SharedCode::InvalidInstanceAlias, "instance_alias"))?;
    if payload.adb_port == Some(0) {
        return Err(SanitizationError::new(
            SanitizationCode::InvalidAdbPort,
            "adb_port",
        ));
    }
    if payload.adb_port.is_some()
        && !payload
            .adb_host
            .as_ref()
            .is_some_and(|host| !host.trim().is_empty())
    {
        return Err(SanitizationError::new(
            SanitizationCode::InvalidAdbHost,
            "adb_host",
        ));
    }
    let bounded = |value: &Option<String>, max_bytes: usize| {
        value.as_ref().is_none_or(|text| {
            !text.is_empty() && text.len() <= max_bytes && !text.chars().any(char::is_control)
        })
    };
    let discovery_consistent = match payload.binding_source {
        InstanceBindingSource::Explicit => {
            payload.discovered_instance_index.is_none()
                && payload.discovered_instance_name.is_none()
                && payload.provider_version.is_none()
        }
        InstanceBindingSource::Discovered => {
            payload.discovered_instance_index.is_some()
                && payload.discovered_instance_name.is_some()
        }
    };
    if !discovery_consistent
        || !bounded(
            &payload.discovered_instance_name,
            MAX_DISCOVERED_INSTANCE_NAME_BYTES,
        )
        || !bounded(&payload.provider_version, MAX_PROVIDER_VERSION_BYTES)
    {
        return Err(SanitizationError::new(
            SanitizationCode::InvalidInstanceBindingDiscovery,
            "binding_source",
        ));
    }
    // Workflow #398 RD-1b (model-398 v1.1 §3.4): an instance addressed by its serial records no
    // ADB host or port. A handset's device is bound explicitly, on a physical device, and records
    // its fingerprint, binding time and channel together or none of them.
    let device_consistent = payload.device.as_ref().is_none_or(|device| {
        let bound = [
            device.fingerprint.is_some(),
            device.bound_at_unix_ms.is_some(),
            device.bound_via.is_some(),
        ];
        device.kind == crate::InstanceDeviceKind::Handset
            && payload.serial_configured
            && payload.binding_source == InstanceBindingSource::Explicit
            && payload.provenance == crate::ExecutionBackendProvenance::PhysicalDevice
            && (bound.iter().all(|present| *present) || bound.iter().all(|present| !present))
    });
    if (payload.serial_configured && (payload.adb_host.is_some() || payload.adb_port.is_some()))
        || !device_consistent
    {
        return Err(SanitizationError::new(
            SanitizationCode::InvalidInstanceBindingDevice,
            "device",
        ));
    }
    if let Some(fingerprint) = payload
        .device
        .as_ref()
        .and_then(RuntimeInstanceDeviceBinding::fingerprint)
    {
        fingerprint.validate_stored()?;
    }
    Ok(())
}

fn validate_runtime_fact_recorded(
    payload: &RuntimeFactRecordedPayload,
) -> Result<(), SanitizationError> {
    if payload.action != EventAction::FactPublish {
        return Err(SanitizationError::new(
            SanitizationCode::InvalidRuntimeFactRecordedAction,
            "runtime_payload",
        ));
    }
    payload.record.validate()
}

fn validate_runtime_fact_invalidated(
    payload: &RuntimeFactInvalidatedPayload,
) -> Result<(), SanitizationError> {
    if payload.action != EventAction::FactInvalidate {
        return Err(SanitizationError::new(
            SanitizationCode::InvalidRuntimeFactInvalidatedAction,
            "runtime_payload",
        ));
    }
    payload.invalidation.validate()
}

fn validate_runtime_fact_snapshot(
    payload: &RuntimeFactSnapshotPayload,
) -> Result<(), SanitizationError> {
    if payload.action != EventAction::FactSnapshot {
        return Err(SanitizationError::new(
            SanitizationCode::InvalidRuntimeFactSnapshotAction,
            "runtime_payload",
        ));
    }
    payload.snapshot.validate()
}

fn validate_performance_payload(payload: &PerformancePayload) -> Result<(), SanitizationError> {
    match payload {
        PerformancePayload::PressureStarted(value) | PerformancePayload::PressureEnded(value)
            if value.action == EventAction::PerformanceObserve =>
        {
            if value.observed_at_unix_ms == 0
                || value.pressure.started_at_unix_ms > value.pressure.last_observed_at_unix_ms
                || value.pressure.last_observed_at_unix_ms > value.observed_at_unix_ms
            {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidPerformancePressureTime,
                    "performance_pressure",
                ));
            }
            value.pressure.validate()
        }
        PerformancePayload::StutterDetected(value)
            if value.action == EventAction::PerformanceObserve =>
        {
            validate_performance_stutter(&PerformanceStutterEventData {
                instance_id: value.instance_id.clone(),
                observed_at_unix_ms: value.observed_at_unix_ms,
                frame_gap_ms: value.frame_gap_ms,
                capture_latency_ms: value.capture_latency_ms,
                recognition_latency_ms: value.recognition_latency_ms,
                action_effect_latency_ms: value.action_effect_latency_ms,
            })
        }
        PerformancePayload::Summary(value) if value.action == EventAction::PerformanceObserve => {
            validate_performance_summary(&PerformanceSummaryEventData {
                context: value.context.clone(),
                foreground: value.foreground.clone(),
                owned_processes: value.owned_processes.clone(),
                third_party_high_load: value.third_party_high_load.clone(),
                ledger_commits: value.ledger_commits.clone(),
                capacity: value.capacity.clone(),
                pacing: value.pacing.as_deref().cloned(),
            })
        }
        PerformancePayload::MonitorDegraded(value)
            if value.action == EventAction::PerformanceObserve
                && value.health == PerformanceMonitorHealth::Degraded
                && value.outcome.is_some() =>
        {
            validate_performance_monitor_state(&PerformanceMonitorStateEventData {
                observed_at_unix_ms: value.observed_at_unix_ms,
                health: value.health,
                outcome: value.outcome.clone(),
                consecutive_failures: value.consecutive_failures,
                terminal: value.terminal,
                unavailable_metrics: value.unavailable_metrics.clone(),
            })
        }
        PerformancePayload::MonitorRecovered(value)
            if value.action == EventAction::PerformanceObserve
                && matches!(
                    value.health,
                    PerformanceMonitorHealth::Healthy | PerformanceMonitorHealth::Partial
                )
                && value.outcome.is_none()
                && value.consecutive_failures == 0
                && !value.terminal =>
        {
            validate_performance_monitor_state(&PerformanceMonitorStateEventData {
                observed_at_unix_ms: value.observed_at_unix_ms,
                health: value.health,
                outcome: None,
                consecutive_failures: 0,
                terminal: false,
                unavailable_metrics: value.unavailable_metrics.clone(),
            })
        }
        PerformancePayload::BalanceChanged(value)
            if value.action == EventAction::PerformanceObserve =>
        {
            validate_performance_control(&PerformanceControlEventData {
                observed_at_unix_ms: value.observed_at_unix_ms,
                instance_id: value.instance_id.clone(),
                previous_level: value.previous_level,
                level: value.level,
                reason: value.reason,
                host_responsiveness_basis_points: value.host_responsiveness_basis_points,
                third_party_pressure_basis_points: value.third_party_pressure_basis_points,
                recovery: value.recovery,
                deadline_disposition: value.deadline_disposition,
                arbitration: value.arbitration,
            })
        }
        PerformancePayload::PacingChanged(value)
        | PerformancePayload::DispatchShareChanged(value)
        | PerformancePayload::RunHold(value) => value.validate(),
        _ => Err(SanitizationError::new(
            SanitizationCode::InvalidPerformancePayload,
            "performance_payload",
        )),
    }
}

fn validate_fact_payload(payload: &FactPayload) -> Result<(), SanitizationError> {
    match payload {
        FactPayload::Published(value) if value.action == EventAction::FactPublish => {
            crate::fact::validate_fact_observation(value.records())?;
            if value.scope_instances.len() > 1024
                || value
                    .scope_instances
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    != value.scope_instances.len()
            {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidFactScopeBinding,
                    "scope_instances",
                ));
            }
            // The 512 KiB observation bound measures the current encoding: a write-side rule
            // that `EventPayload::validate_write_bounds` checks (Workflow #191 B1-S1).
            Ok(())
        }
        FactPayload::Invalidated(value) if value.action == EventAction::FactInvalidate => {
            validate_fact_invalidation(&value.invalidation)
        }
        _ => Err(SanitizationError::new(
            SanitizationCode::InvalidFactPayload,
            "fact_payload",
        )),
    }
}

fn validate_client_action_payload(payload: &ClientPayload) -> Result<(), SanitizationError> {
    if let ClientPayload::LabPinRelease(value) = payload {
        return value.validate();
    }
    if let ClientPayload::OwnerUnlock(value) = payload {
        return value.validate();
    }
    if let ClientPayload::GovernanceIdentityDeclared(value) = payload {
        return value.validate();
    }
    if let ClientPayload::Action(value) = payload {
        if value.action == EventAction::ClientAction {
            return value.record.validate();
        }
        return Err(SanitizationError::new(
            SanitizationCode::InvalidClientActionPayload,
            "client_action_payload",
        ));
    }
    Ok(())
}

fn validate_approval_payload(payload: &ApprovalPayload) -> Result<(), SanitizationError> {
    match payload {
        ApprovalPayload::Decision(value) if value.action == EventAction::ApprovalDecision => {
            value.decision.validate()
        }
        _ => Err(SanitizationError::new(
            SanitizationCode::InvalidApprovalPayload,
            "approval_payload",
        )),
    }
}

fn validate_policy_payload(payload: &PolicyPayload) -> Result<(), SanitizationError> {
    let dispatch = match payload {
        PolicyPayload::DispatchIntent(value)
            if value.action == EventAction::PolicyDispatch
                && value.diagnostic_code.is_none()
                && value.rejection.is_none()
                && value.effect_disposition.is_none()
                && value.admission.is_none() =>
        {
            Some(value)
        }
        PolicyPayload::DispatchAdmitted(value)
            if value.action == EventAction::PolicyDispatch
                && value.diagnostic_code.is_none()
                && value.rejection.is_none()
                && value.effect_disposition == Some(EffectDisposition::Performed)
                && value.admission.is_some() =>
        {
            Some(value)
        }
        PolicyPayload::DispatchRejected(value)
            if value.action == EventAction::PolicyDispatch
                && value.diagnostic_code == Some(DiagnosticCode::PolicyRejected)
                && matches!(
                    value.effect_disposition,
                    Some(EffectDisposition::NotPerformed | EffectDisposition::Indeterminate)
                )
                && value.admission.is_none() =>
        {
            Some(value)
        }
        PolicyPayload::DispatchCompleted(value)
            if value.action == EventAction::PolicyDispatch
                && value.diagnostic_code.is_none()
                && value.rejection.is_none()
                && value.effect_disposition == Some(EffectDisposition::Performed)
                && value.admission.is_some() =>
        {
            Some(value)
        }
        PolicyPayload::ExecutionRecorded(value) if value.action == EventAction::PolicyExecution => {
            if value
                .wall_elapsed_ms
                .zip(value.budget_elapsed_ms)
                .is_some_and(|(wall, budget)| budget > wall)
            {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidPolicyExecutionTime,
                    "budget_elapsed_ms",
                ));
            }
            validate_policy_execution_data(&PolicyExecutionEventData {
                decision_id: value.decision_id.clone(),
                task_id: value.task_id.clone(),
                instance_id: value.instance_id.clone(),
                observed_at_unix_ms: value.observed_at_unix_ms,
                outcome: value.policy_result.clone(),
            })?;
            None
        }
        PolicyPayload::ControlStateRecorded(value) => {
            value.validate()?;
            None
        }
        PolicyPayload::ActivityStateRecorded(value) => {
            value.validate()?;
            None
        }
        PolicyPayload::FactRefreshTransition(value) => {
            value.validate()?;
            None
        }
        PolicyPayload::PlanningSignalObserved(value)
            if value.action == EventAction::PolicyPlanning =>
        {
            validate_policy_planning_signal_data(&PolicyPlanningSignalEventData {
                signal_id: value.signal_id.clone(),
                instance_id: value.instance_id.clone(),
                task_id: value.task_id.clone(),
                kind: value.kind,
                outcome: value.outcome.clone(),
                observed_at_unix_ms: value.observed_at_unix_ms,
                detection_budget: value.detection_budget.clone(),
            })?;
            None
        }
        _ => {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidPolicyPayload,
                "policy_payload",
            ));
        }
    };
    let Some(value) = dispatch else {
        return Ok(());
    };
    if value.priority_locked && !matches!(payload, PolicyPayload::DispatchIntent(_)) {
        return Err(SanitizationError::new(
            SanitizationCode::InvalidPolicyPayload,
            "priority_locked",
        ));
    }
    // Workflow #383 register X14: only a dispatch intent carries its random delay.
    if let Some(delay) = &value.random_delay {
        if !matches!(payload, PolicyPayload::DispatchIntent(_)) {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidPolicyPayload,
                "random_delay",
            ));
        }
        delay.validate()?;
    }
    validate_policy_dispatch_fields(
        &PolicyDispatchEventData {
            decision_id: value.decision_id.clone(),
            task_id: value.task_id.clone(),
            instance_id: value.instance_id.clone(),
            operation_id: value.operation_id.clone(),
            procedure_binding_digest: value.procedure_binding_digest.clone(),
            reason_chain_id: value.reason_chain_id.clone(),
            reasons: value.reasons.clone(),
            catalog_hash: value.catalog_hash.clone(),
            catalog_version: value.catalog_version,
            input_ledger_position: value.input_ledger_position,
            fact_snapshot_id: value.fact_snapshot_id.clone(),
            approval_fact_ids: value.approval_fact_ids.clone(),
            urgency_milli: value.urgency_milli,
        },
        value.input_ledger_position != 0 || value.input_fact_revisions.is_some(),
    )?;
    if let Some(admission) = &value.admission {
        validate_policy_admission(admission)?;
    }
    if let Some(rejection) = &value.rejection {
        rejection.validate()?;
    }
    Ok(())
}

fn validate_catalog_transition_data(
    data: &CatalogTransitionEventData,
) -> Result<(), SanitizationError> {
    validate_policy_token(&data.catalog_id, "catalog_id")?;
    validate_catalog_hash(&data.catalog_hash, "catalog_hash")?;
    if data.catalog_version == 0 {
        return Err(SanitizationError::new(
            SanitizationCode::InvalidCatalogVersion,
            "catalog_version",
        ));
    }
    if let Some(previous) = &data.previous_catalog_hash {
        validate_catalog_hash(previous, "previous_catalog_hash")?;
        if previous == &data.catalog_hash {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidCatalogTransition,
                "previous_catalog_hash",
            ));
        }
    }
    if let Some(promotion) = &data.promotion {
        promotion.validate()?;
    }
    Ok(())
}

fn validate_catalog_payload(payload: &CatalogPayload) -> Result<(), SanitizationError> {
    let value = match payload {
        CatalogPayload::TransitionIntent(value)
            if matches!(
                value.action,
                EventAction::CatalogActivate | EventAction::CatalogRollback
            ) && value.diagnostic_code.is_none()
                && value.effect_disposition.is_none() =>
        {
            value
        }
        CatalogPayload::Activated(value)
            if value.action == EventAction::CatalogActivate
                && value.diagnostic_code.is_none()
                && value.effect_disposition == Some(EffectDisposition::Performed) =>
        {
            value
        }
        CatalogPayload::RolledBack(value)
            if value.action == EventAction::CatalogRollback
                && value.diagnostic_code.is_none()
                && value.effect_disposition == Some(EffectDisposition::Performed) =>
        {
            value
        }
        CatalogPayload::TransitionFailed(value)
            if matches!(
                value.action,
                EventAction::CatalogActivate | EventAction::CatalogRollback
            ) && value.diagnostic_code == Some(DiagnosticCode::CatalogTransitionFailed)
                && matches!(
                    value.effect_disposition,
                    Some(EffectDisposition::NotPerformed | EffectDisposition::Indeterminate)
                ) =>
        {
            value
        }
        _ => {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidCatalogTransitionLifecycle,
                "catalog_payload",
            ));
        }
    };
    validate_catalog_transition_data(&CatalogTransitionEventData {
        catalog_id: value.catalog_id.clone(),
        catalog_version: value.catalog_version,
        catalog_hash: value.catalog_hash.clone(),
        previous_catalog_hash: value.previous_catalog_hash.clone(),
        promotion: value.promotion.clone(),
    })
}

fn validate_release_payload(payload: &ReleasePayload) -> Result<(), SanitizationError> {
    match payload {
        ReleasePayload::Staged(value) => {
            if value.action != EventAction::ReleaseStage
                || value.effect_disposition != EffectDisposition::Performed
            {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidReleaseStageLifecycle,
                    "release_payload",
                ));
            }
            value.manifest.validate()
        }
        ReleasePayload::TransitionIntent(value)
            if value.action == release_action(value.transition.kind())
                && value.diagnostic_code.is_none()
                && value.effect_disposition.is_none() =>
        {
            value.transition.validate()
        }
        ReleasePayload::Activated(value)
            if value.action == EventAction::ReleaseActivate
                && value.transition.kind() == ReleaseTransitionKind::Activate
                && value.diagnostic_code.is_none()
                && value.effect_disposition == Some(EffectDisposition::Performed) =>
        {
            value.transition.validate()
        }
        ReleasePayload::RolledBack(value)
            if value.action == EventAction::ReleaseRollback
                && value.transition.kind() == ReleaseTransitionKind::Rollback
                && value.diagnostic_code.is_none()
                && value.effect_disposition == Some(EffectDisposition::Performed) =>
        {
            value.transition.validate()
        }
        ReleasePayload::TransitionFailed(value)
            if value.action == release_action(value.transition.kind())
                && value.diagnostic_code == Some(DiagnosticCode::ReleaseTransitionFailed)
                && matches!(
                    value.effect_disposition,
                    Some(EffectDisposition::NotPerformed | EffectDisposition::Indeterminate)
                ) =>
        {
            value.transition.validate()
        }
        _ => Err(SanitizationError::new(
            SanitizationCode::InvalidReleaseTransitionLifecycle,
            "release_payload",
        )),
    }
}

fn validate_agent_payload(payload: &AgentPayload) -> Result<(), SanitizationError> {
    if let AgentPayload::SessionStarted(value)
    | AgentPayload::SessionResumed(value)
    | AgentPayload::SessionCompleted(value)
    | AgentPayload::SessionEscalated(value) = payload
        && value.report_artifact.is_some()
    {
        return Err(SanitizationError::new(
            SanitizationCode::InvalidAgentSessionLifecycle,
            "report_artifact",
        ));
    }
    match payload {
        AgentPayload::WakeRequested(value) if value.action == EventAction::AgentWake => {
            value.wake.validate()
        }
        AgentPayload::SessionStarted(value)
            if value.action == EventAction::AgentSessionStart
                && value.session.response().is_none()
                && value.session.status().state() == AgentAttentionState::Active
                && value.session.status().attempts_used() == 1 =>
        {
            value.session.validate()
        }
        AgentPayload::SessionResumed(value)
            if value.action == EventAction::AgentSessionResume
                && value.session.response().is_none()
                && value.session.status().state() == AgentAttentionState::Active =>
        {
            value.session.validate()
        }
        AgentPayload::WakeObligationRecorded(value) => value.validate(),
        AgentPayload::ResponseRecorded(value)
            if value.action == EventAction::AgentSessionRespond
                && value.session.status().state() == AgentAttentionState::Active
                && value.session.response().is_some_and(|response| {
                    response.disposition() == crate::AgentResponseDisposition::RetryableFailure
                }) =>
        {
            value.session.validate()
        }
        AgentPayload::SessionCompleted(value)
            if value.action == EventAction::AgentSessionComplete
                && value.session.status().state() == AgentAttentionState::Completed
                && value.session.response().is_some_and(|response| {
                    response.disposition() == crate::AgentResponseDisposition::Completed
                }) =>
        {
            value.session.validate()
        }
        AgentPayload::SessionEscalated(value)
            if value.action == EventAction::AgentSessionEscalate
                && value.session.status().state() == AgentAttentionState::PausedNeedsHuman
                && value.session.response().is_some_and(|response| {
                    matches!(
                        response.disposition(),
                        crate::AgentResponseDisposition::RetryableFailure
                            | crate::AgentResponseDisposition::NeedsHuman
                    )
                }) =>
        {
            value.session.validate()
        }
        _ => Err(SanitizationError::new(
            SanitizationCode::InvalidAgentSessionLifecycle,
            "agent_payload",
        )),
    }
}

fn validate_policy_token(value: &str, field: &'static str) -> Result<(), SanitizationError> {
    if value.is_empty()
        || value.len() > 256
        || value.chars().any(char::is_control)
        || value.chars().any(char::is_whitespace)
    {
        Err(SanitizationError::new(
            SanitizationCode::InvalidPolicyToken,
            field,
        ))
    } else {
        Ok(())
    }
}

fn validate_policy_digest(value: &str, field: &'static str) -> Result<(), SanitizationError> {
    let valid = value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    });
    if valid {
        Ok(())
    } else {
        Err(SanitizationError::new(
            SanitizationCode::InvalidPolicyDigest,
            field,
        ))
    }
}

fn validate_catalog_hash(value: &str, field: &'static str) -> Result<(), SanitizationError> {
    let valid = value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    });
    if valid {
        Ok(())
    } else {
        Err(SanitizationError::new(
            SanitizationCode::InvalidCatalogHash,
            field,
        ))
    }
}

impl ObservationDraft {
    fn new(action: EventAction, audit: AuditInput) -> Self {
        Self {
            action,
            device_diagnostics: None,
            channel: None,
            audit,
        }
    }

    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<ObservationPayload, SanitizationError> {
        Ok(ObservationPayload {
            action: self.action,
            device_diagnostics: self.device_diagnostics,
            channel: self.channel.map(|binding| binding.channel),
            via: self.channel.and_then(|binding| binding.via),
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl InputIntentDraft {
    fn new(
        action: EventAction,
        execution_plan: Option<InputExecutionPlanRecord>,
        audit: AuditInput,
    ) -> Self {
        Self {
            action,
            execution_plan,
            provenance: None,
            audit,
        }
    }

    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<InputIntentPayload, SanitizationError> {
        if self.execution_plan.is_some() && self.action != EventAction::InputSwipe {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidInputExecutionPlan,
                "action",
            ));
        }
        if let Some(execution_plan) = &self.execution_plan {
            execution_plan.validate()?;
        }
        let mut provenance = self.provenance;
        if let Some(provenance) = &mut provenance {
            provenance.input_action.validate().map_err(|_| {
                SanitizationError::new(SanitizationCode::InvalidInputProvenance, "input_action")
            })?;
            redact_sensitive_input(&mut provenance.input_action);
        }
        let payload = InputIntentPayload {
            action: self.action,
            execution_plan: self.execution_plan,
            provenance,
            audit: self.audit.sanitize(fingerprinter)?,
        };
        payload.validate()?;
        Ok(payload)
    }
}

impl DiagnosticDraft {
    fn new(action: EventAction, diagnostic_code: DiagnosticCode, audit: AuditInput) -> Self {
        Self {
            action,
            diagnostic_code,
            audit,
        }
    }

    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<DiagnosticPayload, SanitizationError> {
        Ok(DiagnosticPayload {
            action: self.action,
            diagnostic_code: self.diagnostic_code,
            outcome: diagnostic_code_outcome(self.diagnostic_code),
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl OutcomeDraft {
    fn new(action: EventAction, effect_disposition: EffectDisposition, audit: AuditInput) -> Self {
        Self {
            action,
            effect_disposition,
            runtime_state: None,
            touch_response_us: None,
            audit,
        }
    }

    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<OutcomePayload, SanitizationError> {
        Ok(OutcomePayload {
            action: self.action,
            effect_disposition: self.effect_disposition,
            runtime_state: self.runtime_state,
            touch_response_us: self.touch_response_us,
            channel: None,
            via: None,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl DiagnosticOutcomeDraft {
    fn new(
        action: EventAction,
        diagnostic_code: DiagnosticCode,
        effect_disposition: EffectDisposition,
        audit: AuditInput,
    ) -> Self {
        Self {
            action,
            diagnostic_code,
            effect_disposition,
            outcome: None,
            detail: None,
            cleanup_cause: None,
            lifecycle_failure: None,
            artifact_failure: None,
            resource_declaration: None,
            runtime_state: None,
            audit,
        }
    }

    fn new_with_detail(
        action: EventAction,
        diagnostic_code: DiagnosticCode,
        effect_disposition: EffectDisposition,
        outcome: Outcome,
        detail: DiagnosticDetailDraft,
        audit: AuditInput,
    ) -> Self {
        let mut draft = Self::new(action, diagnostic_code, effect_disposition, audit);
        draft.outcome = Some(outcome);
        draft.detail = Some(detail);
        draft
    }

    fn new_with_causes(
        action: EventAction,
        diagnostic_code: DiagnosticCode,
        effect_disposition: EffectDisposition,
        outcome: Outcome,
        detail: Option<DiagnosticDetailDraft>,
        cleanup_cause: Option<CleanupCauseDraft>,
        audit: AuditInput,
    ) -> Self {
        let mut draft = Self::new(action, diagnostic_code, effect_disposition, audit);
        draft.outcome = Some(outcome);
        draft.detail = detail;
        draft.cleanup_cause = cleanup_cause.map(Box::new);
        draft
    }

    /// The record's outcome is, in order: the producer's, the lifecycle failure's, the
    /// artifact failure's, else the diagnostic code itself; a cleanup cause is its `cleanup`
    /// link (ledger revision 3).
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<DiagnosticOutcomePayload, SanitizationError> {
        let (lifecycle_failure, lifecycle_outcome) = match self.lifecycle_failure {
            Some(failure) => {
                let (record, outcome) = failure.sanitize()?;
                (Some(Box::new(record)), Some(outcome))
            }
            None => (None, None),
        };
        let (artifact_failure, artifact_outcome) = match self.artifact_failure {
            Some(failure) => {
                let (record, outcome) = failure.sanitize()?;
                (Some(Box::new(record)), Some(outcome))
            }
            None => (None, None),
        };
        let mut outcome = self
            .outcome
            .or(lifecycle_outcome)
            .or(artifact_outcome)
            .unwrap_or_else(|| diagnostic_code_outcome(self.diagnostic_code));
        let cleanup_detail = match self.cleanup_cause {
            Some(cause) => {
                let (link, detail) = cause.sanitize()?;
                outcome = outcome.link(CauseRelation::Cleanup, link);
                detail.map(Box::new)
            }
            None => None,
        };
        Ok(DiagnosticOutcomePayload {
            action: self.action,
            diagnostic_code: self.diagnostic_code,
            effect_disposition: self.effect_disposition,
            outcome,
            runtime_state: self.runtime_state,
            artifact_failure,
            resource_declaration: self.resource_declaration,
            detail: self
                .detail
                .map(DiagnosticDetailDraft::sanitize)
                .transpose()?,
            cleanup_detail,
            lifecycle_failure,
            channel: None,
            via: None,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl MonitorOutcomeDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<MonitorOutcomePayload, SanitizationError> {
        self.observation.validate().map_err(|_| {
            SanitizationError::new(SanitizationCode::InvalidMonitorOutcome, "observation")
        })?;
        self.decision.validate().map_err(|_| {
            SanitizationError::new(SanitizationCode::InvalidMonitorOutcome, "decision")
        })?;
        if self.observation.diagnosis() != self.decision.diagnosis() {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidMonitorOutcome,
                "diagnosis",
            ));
        }
        Ok(MonitorOutcomePayload {
            action: self.action,
            effect_disposition: self.effect_disposition,
            observation: self.observation,
            decision: self.decision,
            runtime_state: self.runtime_state,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl MonitorRecoveryCoordinationDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<MonitorRecoveryCoordinationPayload, SanitizationError> {
        if self.admitted != (self.reason == MonitorRecoveryCoordinationReason::SchedulerAvailable) {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidMonitorRecoveryCoordination,
                "reason",
            ));
        }
        Ok(MonitorRecoveryCoordinationPayload {
            action: EventAction::MonitorRecovery,
            effect_disposition: EffectDisposition::NotPerformed,
            recovery: self.recovery,
            reason: self.reason,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl SchedulerQueueDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<SchedulerQueuePayload, SanitizationError> {
        if self.position == 0 || self.deadline_monotonic_ms == 0 {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidSchedulerQueue,
                "queue_position_or_deadline",
            ));
        }
        Ok(SchedulerQueuePayload {
            action: self.action,
            priority: self.priority,
            position: self.position,
            deadline_monotonic_ms: self.deadline_monotonic_ms,
            preempt_requested: self.preempt_requested,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl SchedulerPreemptionDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<SchedulerPreemptionPayload, SanitizationError> {
        Ok(SchedulerPreemptionPayload {
            action: self.action,
            from_holder_id: self.from_holder_id,
            from_lease_id: self.from_lease_id,
            queued_request_id: self.queued_request_id,
            queued_priority: self.queued_priority,
            deferred_by_destructive_step: self.deferred_by_destructive_step,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl LeaseTransferDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<LeaseTransferPayload, SanitizationError> {
        if self.from_lease_id == self.to_lease_id {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidLeaseTransfer,
                "lease_identity",
            ));
        }
        Ok(LeaseTransferPayload {
            action: self.action,
            effect_disposition: self.effect_disposition,
            from_holder_id: self.from_holder_id,
            from_lease_id: self.from_lease_id,
            to_holder_id: self.to_holder_id,
            to_lease_id: self.to_lease_id,
            queued_request_id: self.queued_request_id,
            priority: self.priority,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl ObservationResultDraft {
    fn new(
        action: EventAction,
        effect_disposition: EffectDisposition,
        frame_width: u32,
        frame_height: u32,
        recognition_verdict: Option<RecognitionVerdict>,
        capture_acquire_us: Option<u64>,
        audit: AuditInput,
    ) -> Self {
        Self {
            action,
            effect_disposition,
            frame_width,
            frame_height,
            recognition_verdict,
            capture_acquire_us,
            audit,
        }
    }

    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<ObservationResultPayload, SanitizationError> {
        if self.frame_width == 0 || self.frame_height == 0 {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidSanitizedPayload,
                "frame_dimensions",
            ));
        }
        Ok(ObservationResultPayload {
            action: self.action,
            effect_disposition: self.effect_disposition,
            frame_width: self.frame_width,
            frame_height: self.frame_height,
            recognition_verdict: self.recognition_verdict,
            capture_acquire_us: self.capture_acquire_us,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl CapturePressureDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<CapturePressurePayload, SanitizationError> {
        if self.memory_budget_bytes == 0 {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidCapturePressure,
                "memory_budget_bytes",
            ));
        }
        Ok(CapturePressurePayload {
            action: self.action,
            state: self.state,
            memory_budget_bytes: self.memory_budget_bytes,
            resident_bytes: self.resident_bytes,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl CaptureDedupWindowDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<CaptureDedupWindowPayload, SanitizationError> {
        if self.duplicate_count == 0
            || self.duration_ms == 0
            || (self.preserving_material && self.preserved_frame_id.is_none())
        {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidCaptureDedupWindow,
                "duplicate_count",
            ));
        }
        Ok(CaptureDedupWindowPayload {
            action: self.action,
            duplicate_count: self.duplicate_count,
            duration_ms: self.duration_ms,
            preserved_frame_id: self.preserved_frame_id,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl CapturePolicyDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<CapturePolicyPayload, SanitizationError> {
        if self.cadence_ms == 0 {
            return Err(SanitizationError::new(
                SharedCode::InvalidCapturePolicy,
                "cadence_ms",
            ));
        }
        Ok(CapturePolicyPayload {
            action: self.action,
            cadence_ms: self.cadence_ms,
            retention_class: self.retention_class,
            reason: self.reason,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl CaptureSummaryCommittedDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<CaptureSummaryCommittedPayload, SanitizationError> {
        self.summary.validate()?;
        Ok(CaptureSummaryCommittedPayload {
            action: EventAction::CaptureSummaryCommit,
            summary: self.summary,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl ArtifactExportDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<ArtifactExportPayload, SanitizationError> {
        if self.artifact_count == 0 {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidArtifactExport,
                "artifact_count",
            ));
        }
        Ok(ArtifactExportPayload {
            action: self.action,
            effect_disposition: self.effect_disposition,
            task_outcome: self.task_outcome,
            evidence_completeness: self.evidence_completeness,
            artifact_count: self.artifact_count,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl ArtifactExportFailureDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<ArtifactExportFailurePayload, SanitizationError> {
        Ok(ArtifactExportFailurePayload {
            action: self.action,
            diagnostic_code: self.diagnostic_code,
            outcome: diagnostic_code_outcome(self.diagnostic_code),
            effect_disposition: self.effect_disposition,
            task_outcome: self.task_outcome,
            evidence_completeness: self.evidence_completeness,
            artifact_count: self.artifact_count,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl RecoveryDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<RecoveryPayload, SanitizationError> {
        if let Some(record) = &self.migration {
            record.validate()?;
        }
        Ok(RecoveryPayload {
            reason: self.reason,
            migration: self.migration,
            segment_index: self.segment_index,
            affected_bytes: self.affected_bytes,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

enum CommandDraftKind {
    Received(ObservationDraft),
    Validated(OutcomeDraft),
    Rejected(DiagnosticOutcomeDraft),
}

enum RuntimeDraftKind {
    Started(ObservationDraft),
    Takeover(ObservationDraft),
    Failed(DiagnosticOutcomeDraft),
    LifecycleObserved(RuntimeLifecycleDraft),
    InstanceBound(RuntimeInstanceBindingDraft),
    FactRecorded(RuntimeFactRecordedDraft),
    FactInvalidated(RuntimeFactInvalidatedDraft),
    FactSnapshot(RuntimeFactSnapshotDraft),
    DeviceSelfCheck(DeviceSelfCheckDraft),
}

struct RuntimeFactRecordedDraft {
    record: crate::RuntimeFactRecord,
    audit: AuditInput,
}

impl RuntimeFactRecordedDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<RuntimeFactRecordedPayload, SanitizationError> {
        Ok(RuntimeFactRecordedPayload {
            action: EventAction::FactPublish,
            record: self.record,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

struct RuntimeFactInvalidatedDraft {
    invalidation: crate::RuntimeFactInvalidation,
    audit: AuditInput,
}

impl RuntimeFactInvalidatedDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<RuntimeFactInvalidatedPayload, SanitizationError> {
        Ok(RuntimeFactInvalidatedPayload {
            action: EventAction::FactInvalidate,
            invalidation: self.invalidation,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

struct RuntimeFactSnapshotDraft {
    snapshot: Box<crate::RuntimeFactSnapshot>,
    audit: AuditInput,
}

impl RuntimeFactSnapshotDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<RuntimeFactSnapshotPayload, SanitizationError> {
        Ok(RuntimeFactSnapshotPayload {
            action: EventAction::FactSnapshot,
            snapshot: self.snapshot,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

struct RuntimeInstanceBindingDraft {
    instance_alias: String,
    provenance: crate::ExecutionBackendProvenance,
    adb_host: Option<String>,
    adb_port: Option<u16>,
    serial_configured: bool,
    binding_source: InstanceBindingSource,
    discovered_instance_index: Option<u16>,
    discovered_instance_name: Option<String>,
    provider_version: Option<String>,
    device: Option<InstanceDeviceBindingDraft>,
    audit: AuditInput,
}

impl RuntimeInstanceBindingDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<RuntimeInstanceBindingPayload, SanitizationError> {
        Ok(RuntimeInstanceBindingPayload {
            action: EventAction::RuntimeAction,
            instance_alias: self.instance_alias,
            provenance: self.provenance,
            adb_host: self.adb_host,
            adb_port: self.adb_port,
            serial_configured: self.serial_configured,
            binding_source: self.binding_source,
            discovered_instance_index: self.discovered_instance_index,
            discovered_instance_name: self.discovered_instance_name,
            provider_version: self.provider_version,
            device: self
                .device
                .map(|device| device.sanitize(fingerprinter))
                .transpose()?,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

struct RuntimeLifecycleDraft {
    backend_open: Option<Box<crate::BackendOpenReport>>,
    prior_epoch_close: Option<Box<crate::PriorEpochCloseFact>>,
    vendor_stdio: Option<Box<VendorStdioFacts>>,
    adb_recovery: Option<Box<AdbTargetRecovery>>,
    owner_epoch: OwnerEpoch,
    phase: RuntimeLifecyclePhase,
    drill: bool,
    device_diagnostics: Option<Box<DeviceDiagnosticBudgetRecord>>,
    audit: AuditInput,
}

impl RuntimeLifecycleDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<RuntimeLifecyclePayload, SanitizationError> {
        if let Some(facts) = &self.vendor_stdio {
            facts.validate()?;
        }
        Ok(RuntimeLifecyclePayload {
            backend_open: self.backend_open,
            prior_epoch_close: self.prior_epoch_close,
            vendor_stdio: self.vendor_stdio,
            adb_recovery: self.adb_recovery,
            action: EventAction::RuntimeAction,
            owner_epoch: self.owner_epoch,
            phase: self.phase,
            drill: self.drill,
            device_diagnostics: self.device_diagnostics,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

pub struct RuntimePayloadDraft(RuntimeDraftKind);

impl RuntimePayloadDraft {
    pub fn backend_open_observed(
        owner_epoch: OwnerEpoch,
        report: crate::BackendOpenReport,
    ) -> Self {
        Self(RuntimeDraftKind::LifecycleObserved(RuntimeLifecycleDraft {
            backend_open: Some(Box::new(report)),
            prior_epoch_close: None,
            vendor_stdio: None,
            adb_recovery: None,
            owner_epoch,
            phase: RuntimeLifecyclePhase::BackendOpenObserved,
            drill: false,
            device_diagnostics: None,
            audit: AuditInput::new(),
        }))
    }

    pub fn prior_epoch_close(owner_epoch: OwnerEpoch, fact: crate::PriorEpochCloseFact) -> Self {
        let phase = match &fact {
            crate::PriorEpochCloseFact::OwnerImported(_) => {
                RuntimeLifecyclePhase::PriorEpochOwnerImported
            }
            crate::PriorEpochCloseFact::ScopeClosed(_) => {
                RuntimeLifecyclePhase::PriorEpochScopeClosed
            }
        };
        Self(RuntimeDraftKind::LifecycleObserved(RuntimeLifecycleDraft {
            backend_open: None,
            prior_epoch_close: Some(Box::new(fact)),
            vendor_stdio: None,
            adb_recovery: None,
            owner_epoch,
            phase,
            drill: false,
            device_diagnostics: None,
            audit: AuditInput::new(),
        }))
    }
    pub fn resource_declaration_rejected(rejection: crate::ResourceDeclarationRejection) -> Self {
        let mut outcome = DiagnosticOutcomeDraft::new_with_detail(
            EventAction::RuntimeAction,
            DiagnosticCode::RuntimeDiagnostic,
            EffectDisposition::NotPerformed,
            Outcome::new(SharedCode::ResourceDeclarationInvalid)
                .operation(crate::codes::ContractLocation::RunContainedTask),
            DiagnosticDetailDraft::new(
                "resource_declaration",
                crate::codes::ContractLocation::ResourceDeclarationPrepare,
                "pack_containment",
                crate::codes::ContractLocation::RunContainedTask.as_str(),
            ),
            AuditInput::new(),
        );
        outcome.resource_declaration = Some(Box::new(rejection));
        Self(RuntimeDraftKind::Failed(outcome))
    }

    pub fn start_with_device_diagnostics(
        takeover: bool,
        mode: DeviceDiagnosticMode,
        audit: AuditInput,
    ) -> Self {
        let action = if takeover {
            EventAction::RuntimeTakeover
        } else {
            EventAction::RuntimeStart
        };
        let mut draft = ObservationDraft::new(action, audit);
        draft.device_diagnostics = Some(DeviceDiagnosticConfig::new(mode));
        Self(if takeover {
            RuntimeDraftKind::Takeover(draft)
        } else {
            RuntimeDraftKind::Started(draft)
        })
    }

    /// The close summary of one owner epoch's device diagnostic budget.
    pub fn device_diagnostics(
        owner_epoch: OwnerEpoch,
        record: DeviceDiagnosticBudgetRecord,
    ) -> Self {
        Self(RuntimeDraftKind::LifecycleObserved(RuntimeLifecycleDraft {
            backend_open: None,
            prior_epoch_close: None,
            vendor_stdio: None,
            adb_recovery: None,
            owner_epoch,
            phase: RuntimeLifecyclePhase::DeviceDiagnosticSummary,
            drill: false,
            device_diagnostics: Some(Box::new(record)),
            audit: AuditInput::new(),
        }))
    }
    /// `runtime.failed` for a lifecycle failure: the lifecycle draft builds the outcome.
    pub fn failed_with_lifecycle(
        diagnostic_code: DiagnosticCode,
        effect: EffectDisposition,
        detail: DiagnosticDetailDraft,
        lifecycle: RuntimeLifecycleFailureDraft,
        audit: AuditInput,
    ) -> Self {
        let mut outcome =
            DiagnosticOutcomeDraft::new(EventAction::RuntimeAction, diagnostic_code, effect, audit);
        outcome.detail = Some(detail);
        outcome.lifecycle_failure = Some(Box::new(lifecycle));
        Self(RuntimeDraftKind::Failed(outcome))
    }
    pub fn lifecycle_observed(
        owner_epoch: OwnerEpoch,
        phase: RuntimeLifecyclePhase,
        audit: AuditInput,
    ) -> Self {
        Self(RuntimeDraftKind::LifecycleObserved(RuntimeLifecycleDraft {
            backend_open: None,
            prior_epoch_close: None,
            vendor_stdio: None,
            adb_recovery: None,
            owner_epoch,
            phase,
            drill: false,
            device_diagnostics: None,
            audit,
        }))
    }

    /// Workflow #390 DRILL-1: one phase of a drill's own ladder (a person's
    /// `DrillRecoveryLadder`), tagged `drill`. Only the ladder's phases take the tag; the
    /// payload check refuses it on any other.
    pub fn recovery_drill_observed(
        owner_epoch: OwnerEpoch,
        phase: RuntimeLifecyclePhase,
        audit: AuditInput,
    ) -> Self {
        Self(RuntimeDraftKind::LifecycleObserved(RuntimeLifecycleDraft {
            backend_open: None,
            prior_epoch_close: None,
            vendor_stdio: None,
            adb_recovery: None,
            owner_epoch,
            phase,
            drill: true,
            device_diagnostics: None,
            audit,
        }))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn instance_bound(
        instance_alias: impl Into<String>,
        provenance: crate::ExecutionBackendProvenance,
        adb_host: Option<String>,
        adb_port: Option<u16>,
        serial_configured: bool,
        binding_source: InstanceBindingSource,
        discovered_instance_index: Option<u16>,
        discovered_instance_name: Option<String>,
        provider_version: Option<String>,
        device: Option<InstanceDeviceBindingDraft>,
        audit: AuditInput,
    ) -> Self {
        Self(RuntimeDraftKind::InstanceBound(
            RuntimeInstanceBindingDraft {
                instance_alias: instance_alias.into(),
                provenance,
                adb_host,
                adb_port,
                serial_configured,
                binding_source,
                discovered_instance_index,
                discovered_instance_name,
                provider_version,
                device,
                audit,
            },
        ))
    }

    pub fn fact_recorded(record: crate::RuntimeFactRecord, audit: AuditInput) -> Self {
        Self(RuntimeDraftKind::FactRecorded(RuntimeFactRecordedDraft {
            record,
            audit,
        }))
    }

    pub fn fact_invalidated(
        invalidation: crate::RuntimeFactInvalidation,
        audit: AuditInput,
    ) -> Self {
        Self(RuntimeDraftKind::FactInvalidated(
            RuntimeFactInvalidatedDraft {
                invalidation,
                audit,
            },
        ))
    }

    pub fn fact_snapshot(snapshot: crate::RuntimeFactSnapshot, audit: AuditInput) -> Self {
        Self(RuntimeDraftKind::FactSnapshot(RuntimeFactSnapshotDraft {
            snapshot: Box::new(snapshot),
            audit,
        }))
    }

    pub fn started(action: EventAction, audit: AuditInput) -> Self {
        Self(RuntimeDraftKind::Started(ObservationDraft::new(
            action, audit,
        )))
    }

    pub fn adb_target_recovery(owner_epoch: OwnerEpoch, recovery: AdbTargetRecovery) -> Self {
        Self(RuntimeDraftKind::LifecycleObserved(RuntimeLifecycleDraft {
            backend_open: None,
            prior_epoch_close: None,
            vendor_stdio: None,
            owner_epoch,
            phase: RuntimeLifecyclePhase::AdbTargetRecovery,
            adb_recovery: Some(Box::new(recovery)),
            drill: false,
            device_diagnostics: None,
            audit: AuditInput::new(),
        }))
    }

    pub fn vendor_stdio_close(
        owner_epoch: OwnerEpoch,
        instance_id: Option<InstanceId>,
        facts: VendorStdioFacts,
    ) -> Self {
        Self(RuntimeDraftKind::LifecycleObserved(RuntimeLifecycleDraft {
            backend_open: None,
            prior_epoch_close: None,
            vendor_stdio: Some(Box::new(facts)),
            owner_epoch,
            phase: RuntimeLifecyclePhase::VendorStdioClose { instance_id },
            adb_recovery: None,
            drill: false,
            device_diagnostics: None,
            audit: AuditInput::new(),
        }))
    }

    pub fn takeover(action: EventAction, audit: AuditInput) -> Self {
        Self(RuntimeDraftKind::Takeover(ObservationDraft::new(
            action, audit,
        )))
    }

    /// `runtime.failed` with the failure's `outcome` and where it arose.
    pub fn failed(
        diagnostic_code: DiagnosticCode,
        effect: EffectDisposition,
        outcome: Outcome,
        detail: DiagnosticDetailDraft,
        audit: AuditInput,
    ) -> Self {
        Self(RuntimeDraftKind::Failed(
            DiagnosticOutcomeDraft::new_with_detail(
                EventAction::RuntimeAction,
                diagnostic_code,
                effect,
                outcome,
                detail,
                audit,
            ),
        ))
    }
}

enum MonitorDraftKind {
    Requested(ObservationDraft),
    Started(ObservationDraft),
    Completed(MonitorOutcomeDraft),
    Failed(DiagnosticOutcomeDraft),
    RecoveryAdmitted(MonitorRecoveryCoordinationDraft),
    RecoveryDeferred(MonitorRecoveryCoordinationDraft),
}

pub struct MonitorPayloadDraft(MonitorDraftKind);

impl MonitorPayloadDraft {
    pub fn with_runtime_state(
        mut self,
        state: crate::RuntimeStateFact,
    ) -> Result<Self, SanitizationError> {
        state.validate()?;
        match &mut self.0 {
            MonitorDraftKind::Completed(detail) => detail.runtime_state = Some(Box::new(state)),
            MonitorDraftKind::Failed(detail) => detail.runtime_state = Some(Box::new(state)),
            _ => {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidMonitorStateEvent,
                    "runtime_state",
                ));
            }
        }
        Ok(self)
    }
    pub fn requested(audit: AuditInput) -> Self {
        Self(MonitorDraftKind::Requested(ObservationDraft::new(
            EventAction::MonitorProbe,
            audit,
        )))
    }

    pub fn started(audit: AuditInput) -> Self {
        Self(MonitorDraftKind::Started(ObservationDraft::new(
            EventAction::MonitorProbe,
            audit,
        )))
    }

    pub fn completed(
        effect_disposition: EffectDisposition,
        observation: MonitorObservation,
        decision: MonitorDecision,
        audit: AuditInput,
    ) -> Self {
        Self(MonitorDraftKind::Completed(MonitorOutcomeDraft {
            action: EventAction::MonitorProbe,
            effect_disposition,
            observation,
            decision,
            runtime_state: None,
            audit,
        }))
    }

    pub fn failed(
        diagnostic_code: DiagnosticCode,
        effect_disposition: EffectDisposition,
        audit: AuditInput,
    ) -> Self {
        Self(MonitorDraftKind::Failed(DiagnosticOutcomeDraft::new(
            EventAction::MonitorProbe,
            diagnostic_code,
            effect_disposition,
            audit,
        )))
    }

    pub fn recovery_admitted(recovery: MonitorRecoveryKind, audit: AuditInput) -> Self {
        Self(MonitorDraftKind::RecoveryAdmitted(
            MonitorRecoveryCoordinationDraft {
                recovery,
                reason: MonitorRecoveryCoordinationReason::SchedulerAvailable,
                admitted: true,
                audit,
            },
        ))
    }

    pub fn recovery_deferred(
        recovery: MonitorRecoveryKind,
        reason: MonitorRecoveryCoordinationReason,
        audit: AuditInput,
    ) -> Self {
        Self(MonitorDraftKind::RecoveryDeferred(
            MonitorRecoveryCoordinationDraft {
                recovery,
                reason,
                admitted: false,
                audit,
            },
        ))
    }
}

pub struct CommandPayloadDraft(CommandDraftKind);

impl CommandPayloadDraft {
    pub fn validated_runtime_state(
        action: EventAction,
        effect: EffectDisposition,
        state: crate::RuntimeStateFact,
        audit: AuditInput,
    ) -> Self {
        let mut detail = OutcomeDraft::new(action, effect, audit);
        detail.runtime_state = Some(Box::new(state));
        Self(CommandDraftKind::Validated(detail))
    }
    pub fn received(action: EventAction, audit: AuditInput) -> Self {
        Self(CommandDraftKind::Received(ObservationDraft::new(
            action, audit,
        )))
    }

    /// `command.received` of a request the door tagged with its connection's channel (Workflow
    /// #389 U-2, register U1); `None` writes no channel, as for the Runtime's own requests.
    pub fn received_on_channel(
        action: EventAction,
        channel: Option<crate::ChannelBinding>,
        audit: AuditInput,
    ) -> Self {
        let mut draft = ObservationDraft::new(action, audit);
        draft.channel = channel;
        Self(CommandDraftKind::Received(draft))
    }

    pub fn validated(action: EventAction, effect: EffectDisposition, audit: AuditInput) -> Self {
        Self(CommandDraftKind::Validated(OutcomeDraft::new(
            action, effect, audit,
        )))
    }

    pub fn rejected(
        action: EventAction,
        diagnostic_code: DiagnosticCode,
        effect: EffectDisposition,
        audit: AuditInput,
    ) -> Self {
        Self(CommandDraftKind::Rejected(DiagnosticOutcomeDraft::new(
            action,
            diagnostic_code,
            effect,
            audit,
        )))
    }

    /// `command.rejected` whose outcome is the refusal itself rather than the diagnostic code
    /// (ledger revision 3; Workflow #387 D1b, a refused `ConfigureSettings`).
    pub fn rejected_with_outcome(
        action: EventAction,
        diagnostic_code: DiagnosticCode,
        effect: EffectDisposition,
        outcome: Outcome,
        audit: AuditInput,
    ) -> Self {
        let mut draft = DiagnosticOutcomeDraft::new(action, diagnostic_code, effect, audit);
        draft.outcome = Some(outcome);
        Self(CommandDraftKind::Rejected(draft))
    }
}

enum SchedulerDraftKind {
    Admitted(ObservationDraft),
    Queued(SchedulerQueueDraft),
    Denied(DiagnosticDraft),
    Preempted(SchedulerPreemptionDraft),
}

pub struct SchedulerPayloadDraft(SchedulerDraftKind);

impl SchedulerPayloadDraft {
    pub fn admitted(action: EventAction, audit: AuditInput) -> Self {
        Self(SchedulerDraftKind::Admitted(ObservationDraft::new(
            action, audit,
        )))
    }

    pub fn queued(
        action: EventAction,
        priority: LeasePriority,
        position: u32,
        deadline_monotonic_ms: u64,
        preempt_requested: bool,
        audit: AuditInput,
    ) -> Self {
        Self(SchedulerDraftKind::Queued(SchedulerQueueDraft {
            action,
            priority,
            position,
            deadline_monotonic_ms,
            preempt_requested,
            audit,
        }))
    }

    pub fn denied(action: EventAction, diagnostic_code: DiagnosticCode, audit: AuditInput) -> Self {
        Self(SchedulerDraftKind::Denied(DiagnosticDraft::new(
            action,
            diagnostic_code,
            audit,
        )))
    }

    pub fn preempted(
        action: EventAction,
        from_holder_id: HolderId,
        from_lease_id: LeaseId,
        queued_request_id: RequestId,
        queued_priority: LeasePriority,
        deferred_by_destructive_step: bool,
        audit: AuditInput,
    ) -> Self {
        Self(SchedulerDraftKind::Preempted(SchedulerPreemptionDraft {
            action,
            from_holder_id,
            from_lease_id,
            queued_request_id,
            queued_priority,
            deferred_by_destructive_step,
            audit,
        }))
    }
}

enum LeaseDraftKind {
    Requested(ObservationDraft),
    Granted(OutcomeDraft),
    Transferred(LeaseTransferDraft),
    Renewed(OutcomeDraft),
    Released(OutcomeDraft),
    Expired(OutcomeDraft),
    TransitionIntent(ObservationDraft),
    TransitionFailed(DiagnosticOutcomeDraft),
}

pub struct LeasePayloadDraft(LeaseDraftKind);

impl LeasePayloadDraft {
    pub fn requested(action: EventAction, audit: AuditInput) -> Self {
        Self(LeaseDraftKind::Requested(ObservationDraft::new(
            action, audit,
        )))
    }

    pub fn granted(action: EventAction, effect: EffectDisposition, audit: AuditInput) -> Self {
        Self(LeaseDraftKind::Granted(OutcomeDraft::new(
            action, effect, audit,
        )))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn transferred(
        action: EventAction,
        effect_disposition: EffectDisposition,
        from_holder_id: HolderId,
        from_lease_id: LeaseId,
        to_holder_id: HolderId,
        to_lease_id: LeaseId,
        queued_request_id: RequestId,
        priority: LeasePriority,
        audit: AuditInput,
    ) -> Self {
        Self(LeaseDraftKind::Transferred(LeaseTransferDraft {
            action,
            effect_disposition,
            from_holder_id,
            from_lease_id,
            to_holder_id,
            to_lease_id,
            queued_request_id,
            priority,
            audit,
        }))
    }

    pub fn renewed(action: EventAction, effect: EffectDisposition, audit: AuditInput) -> Self {
        Self(LeaseDraftKind::Renewed(OutcomeDraft::new(
            action, effect, audit,
        )))
    }

    pub fn released(action: EventAction, effect: EffectDisposition, audit: AuditInput) -> Self {
        Self(LeaseDraftKind::Released(OutcomeDraft::new(
            action, effect, audit,
        )))
    }

    pub fn expired(action: EventAction, effect: EffectDisposition, audit: AuditInput) -> Self {
        Self(LeaseDraftKind::Expired(OutcomeDraft::new(
            action, effect, audit,
        )))
    }

    pub fn transition_intent(action: EventAction, audit: AuditInput) -> Self {
        Self(LeaseDraftKind::TransitionIntent(ObservationDraft::new(
            action, audit,
        )))
    }

    pub fn transition_failed(
        action: EventAction,
        diagnostic_code: DiagnosticCode,
        effect: EffectDisposition,
        audit: AuditInput,
    ) -> Self {
        Self(LeaseDraftKind::TransitionFailed(
            DiagnosticOutcomeDraft::new(action, diagnostic_code, effect, audit),
        ))
    }
}

enum TaskDraftKind {
    Requested(ObservationDraft),
    Started(ObservationDraft),
    StepStarted(ObservationDraft),
    StepFinished(ObservationDraft),
    Completed(OutcomeDraft),
    Failed(DiagnosticOutcomeDraft),
    Cancelled(OutcomeDraft),
    TerminalIntent(ObservationDraft),
    TerminalCommitFailed(DiagnosticOutcomeDraft),
    Semantic(TaskSemanticDraft),
}

pub struct TaskPayloadDraft(TaskDraftKind);

impl TaskPayloadDraft {
    pub fn requested(action: EventAction, audit: AuditInput) -> Self {
        Self(TaskDraftKind::Requested(ObservationDraft::new(
            action, audit,
        )))
    }

    pub fn started(action: EventAction, audit: AuditInput) -> Self {
        Self(TaskDraftKind::Started(ObservationDraft::new(action, audit)))
    }

    pub fn step_started(action: EventAction, audit: AuditInput) -> Self {
        Self(TaskDraftKind::StepStarted(ObservationDraft::new(
            action, audit,
        )))
    }

    pub fn step_finished(action: EventAction, audit: AuditInput) -> Self {
        Self(TaskDraftKind::StepFinished(ObservationDraft::new(
            action, audit,
        )))
    }

    pub fn completed(action: EventAction, effect: EffectDisposition, audit: AuditInput) -> Self {
        Self(TaskDraftKind::Completed(OutcomeDraft::new(
            action, effect, audit,
        )))
    }

    pub fn failed(
        action: EventAction,
        diagnostic_code: DiagnosticCode,
        effect: EffectDisposition,
        audit: AuditInput,
    ) -> Self {
        Self(TaskDraftKind::Failed(DiagnosticOutcomeDraft::new(
            action,
            diagnostic_code,
            effect,
            audit,
        )))
    }

    pub fn cancelled(action: EventAction, effect: EffectDisposition, audit: AuditInput) -> Self {
        Self(TaskDraftKind::Cancelled(OutcomeDraft::new(
            action, effect, audit,
        )))
    }

    pub fn terminal_intent(action: EventAction, audit: AuditInput) -> Self {
        Self(TaskDraftKind::TerminalIntent(ObservationDraft::new(
            action, audit,
        )))
    }

    pub fn terminal_commit_failed(
        action: EventAction,
        diagnostic_code: DiagnosticCode,
        effect: EffectDisposition,
        audit: AuditInput,
    ) -> Self {
        Self(TaskDraftKind::TerminalCommitFailed(
            DiagnosticOutcomeDraft::new(action, diagnostic_code, effect, audit),
        ))
    }

    pub fn semantic(fact: TaskSemanticFact, audit: AuditInput) -> Self {
        Self(TaskDraftKind::Semantic(TaskSemanticDraft {
            fact,
            lease_expires_at_monotonic_ms: None,
            sampling: None,
            audit,
        }))
    }

    pub fn semantic_with_lease_expiry(
        fact: TaskSemanticFact,
        lease_expires_at_monotonic_ms: u64,
        audit: AuditInput,
    ) -> Self {
        Self(TaskDraftKind::Semantic(TaskSemanticDraft {
            fact,
            lease_expires_at_monotonic_ms: Some(lease_expires_at_monotonic_ms),
            sampling: None,
            audit,
        }))
    }

    pub fn semantic_with_sampling(
        fact: TaskSemanticFact,
        sampling: InputSamplingEvidence,
        audit: AuditInput,
    ) -> Self {
        Self(TaskDraftKind::Semantic(TaskSemanticDraft {
            fact,
            lease_expires_at_monotonic_ms: None,
            sampling: Some(sampling),
            audit,
        }))
    }
}

enum InputDraftKind {
    Intent(InputIntentDraft),
    Committed(OutcomeDraft),
    Completed(ObservationDraft),
    Failed(DiagnosticOutcomeDraft),
}

pub struct InputPayloadDraft(InputDraftKind);

impl InputPayloadDraft {
    pub fn intent_with_provenance(
        input_action: InputAction,
        execution_plan: Option<InputExecutionPlanRecord>,
        source_step_action_id: Option<crate::ActionId>,
        before_frame_id: Option<crate::FrameId>,
        audit: AuditInput,
    ) -> Self {
        Self(InputDraftKind::Intent(InputIntentDraft {
            action: input_action.event_action(),
            execution_plan,
            provenance: Some(InputIntentProvenance {
                input_action,
                source_step_action_id,
                before_frame_id,
            }),
            audit,
        }))
    }

    pub fn intent(action: EventAction, audit: AuditInput) -> Self {
        Self(InputDraftKind::Intent(InputIntentDraft::new(
            action, None, audit,
        )))
    }

    pub fn intent_with_execution_plan(
        action: EventAction,
        execution_plan: InputExecutionPlanRecord,
        audit: AuditInput,
    ) -> Self {
        Self(InputDraftKind::Intent(InputIntentDraft::new(
            action,
            Some(execution_plan),
            audit,
        )))
    }

    pub fn committed(action: EventAction, effect: EffectDisposition, audit: AuditInput) -> Self {
        Self(InputDraftKind::Committed(OutcomeDraft::new(
            action, effect, audit,
        )))
    }

    /// `input.committed` carrying the measured backend response in microseconds (`None` = unmeasured).
    pub fn committed_with_touch_response(
        action: EventAction,
        effect: EffectDisposition,
        touch_response_us: Option<u64>,
        audit: AuditInput,
    ) -> Self {
        let mut detail = OutcomeDraft::new(action, effect, audit);
        detail.touch_response_us = touch_response_us;
        Self(InputDraftKind::Committed(detail))
    }

    pub fn completed(action: EventAction, audit: AuditInput) -> Self {
        Self(InputDraftKind::Completed(ObservationDraft::new(
            action, audit,
        )))
    }

    pub fn failed(
        action: EventAction,
        diagnostic_code: DiagnosticCode,
        effect: EffectDisposition,
        audit: AuditInput,
    ) -> Self {
        Self(InputDraftKind::Failed(DiagnosticOutcomeDraft::new(
            action,
            diagnostic_code,
            effect,
            audit,
        )))
    }

    /// `input.failed` with the failure's `outcome` and where it arose.
    pub fn failed_with_detail(
        action: EventAction,
        diagnostic_code: DiagnosticCode,
        effect: EffectDisposition,
        outcome: Outcome,
        detail: DiagnosticDetailDraft,
        audit: AuditInput,
    ) -> Self {
        Self(InputDraftKind::Failed(
            DiagnosticOutcomeDraft::new_with_detail(
                action,
                diagnostic_code,
                effect,
                outcome,
                detail,
                audit,
            ),
        ))
    }

    /// `input.failed` with the failure's `outcome`, where it arose and its failed cleanup.
    pub fn failed_with_causes(
        action: EventAction,
        diagnostic_code: DiagnosticCode,
        effect: EffectDisposition,
        outcome: Outcome,
        detail: Option<DiagnosticDetailDraft>,
        cleanup_cause: Option<CleanupCauseDraft>,
        audit: AuditInput,
    ) -> Self {
        Self(InputDraftKind::Failed(
            DiagnosticOutcomeDraft::new_with_causes(
                action,
                diagnostic_code,
                effect,
                outcome,
                detail,
                cleanup_cause,
                audit,
            ),
        ))
    }
}

enum ApplicationDraftKind {
    Intent(ObservationDraft),
    Completed(OutcomeDraft),
    Failed(DiagnosticOutcomeDraft),
}

pub struct ApplicationPayloadDraft(ApplicationDraftKind);

impl ApplicationPayloadDraft {
    pub fn intent(action: EventAction, audit: AuditInput) -> Self {
        Self(ApplicationDraftKind::Intent(ObservationDraft::new(
            action, audit,
        )))
    }

    pub fn completed(action: EventAction, effect: EffectDisposition, audit: AuditInput) -> Self {
        Self(ApplicationDraftKind::Completed(OutcomeDraft::new(
            action, effect, audit,
        )))
    }

    pub fn failed(
        action: EventAction,
        diagnostic_code: DiagnosticCode,
        effect: EffectDisposition,
        audit: AuditInput,
    ) -> Self {
        Self(ApplicationDraftKind::Failed(DiagnosticOutcomeDraft::new(
            action,
            diagnostic_code,
            effect,
            audit,
        )))
    }
}

enum CaptureDraftKind {
    Requested(ObservationDraft),
    Completed(ObservationResultDraft),
    Failed(DiagnosticOutcomeDraft),
    PressureChanged(CapturePressureDraft),
    DedupWindow(CaptureDedupWindowDraft),
    PolicyChanged(CapturePolicyDraft),
    SummaryCommitted(CaptureSummaryCommittedDraft),
}

pub struct CapturePayloadDraft(CaptureDraftKind);

impl CapturePayloadDraft {
    pub fn requested(action: EventAction, audit: AuditInput) -> Self {
        Self(CaptureDraftKind::Requested(ObservationDraft::new(
            action, audit,
        )))
    }

    pub fn completed(
        action: EventAction,
        effect: EffectDisposition,
        frame_width: u32,
        frame_height: u32,
        audit: AuditInput,
    ) -> Self {
        Self(CaptureDraftKind::Completed(ObservationResultDraft::new(
            action,
            effect,
            frame_width,
            frame_height,
            None,
            None,
            audit,
        )))
    }

    /// `capture.completed` carrying the measured frame acquisition in microseconds (`None` = unmeasured).
    pub fn completed_with_capture_acquire(
        action: EventAction,
        effect: EffectDisposition,
        frame_width: u32,
        frame_height: u32,
        capture_acquire_us: Option<u64>,
        audit: AuditInput,
    ) -> Self {
        Self(CaptureDraftKind::Completed(ObservationResultDraft::new(
            action,
            effect,
            frame_width,
            frame_height,
            None,
            capture_acquire_us,
            audit,
        )))
    }

    pub fn failed(
        action: EventAction,
        diagnostic_code: DiagnosticCode,
        effect: EffectDisposition,
        audit: AuditInput,
    ) -> Self {
        Self(CaptureDraftKind::Failed(DiagnosticOutcomeDraft::new(
            action,
            diagnostic_code,
            effect,
            audit,
        )))
    }

    /// `capture.failed` with the failure's `outcome` and where it arose.
    pub fn failed_with_detail(
        action: EventAction,
        diagnostic_code: DiagnosticCode,
        effect: EffectDisposition,
        outcome: Outcome,
        detail: DiagnosticDetailDraft,
        audit: AuditInput,
    ) -> Self {
        Self(CaptureDraftKind::Failed(
            DiagnosticOutcomeDraft::new_with_detail(
                action,
                diagnostic_code,
                effect,
                outcome,
                detail,
                audit,
            ),
        ))
    }

    /// `capture.failed` with the failure's `outcome`, where it arose and its failed cleanup.
    pub fn failed_with_causes(
        action: EventAction,
        diagnostic_code: DiagnosticCode,
        effect: EffectDisposition,
        outcome: Outcome,
        detail: Option<DiagnosticDetailDraft>,
        cleanup_cause: Option<CleanupCauseDraft>,
        audit: AuditInput,
    ) -> Self {
        Self(CaptureDraftKind::Failed(
            DiagnosticOutcomeDraft::new_with_causes(
                action,
                diagnostic_code,
                effect,
                outcome,
                detail,
                cleanup_cause,
                audit,
            ),
        ))
    }

    pub fn pressure_changed(
        state: CapturePressureState,
        memory_budget_bytes: u64,
        resident_bytes: u64,
        audit: AuditInput,
    ) -> Self {
        Self(CaptureDraftKind::PressureChanged(CapturePressureDraft {
            action: EventAction::CapturePressure,
            state,
            memory_budget_bytes,
            resident_bytes,
            audit,
        }))
    }

    pub fn dedup_window(duplicate_count: u64, duration_ms: u64, audit: AuditInput) -> Self {
        Self(CaptureDraftKind::DedupWindow(CaptureDedupWindowDraft {
            action: EventAction::CaptureDedup,
            duplicate_count,
            duration_ms,
            preserved_frame_id: None,
            preserving_material: false,
            audit,
        }))
    }

    /// Records a perceptual relation while the original frame remains materialized.
    pub fn dedup_window_preserving_material(
        original_frame: &crate::EventLinksDraft,
        duration_ms: u64,
        audit: AuditInput,
    ) -> Self {
        Self(CaptureDraftKind::DedupWindow(CaptureDedupWindowDraft {
            action: EventAction::CaptureDedup,
            duplicate_count: 1,
            duration_ms,
            preserved_frame_id: original_frame.frame_id().cloned(),
            preserving_material: true,
            audit,
        }))
    }

    pub fn policy_changed(
        cadence_ms: u64,
        retention_class: RetentionClass,
        reason: CapturePolicyReason,
        audit: AuditInput,
    ) -> Self {
        Self(CaptureDraftKind::PolicyChanged(CapturePolicyDraft {
            action: EventAction::CapturePolicy,
            cadence_ms,
            retention_class,
            reason,
            audit,
        }))
    }

    pub fn summary_committed(summary: CaptureSummaryRecord, audit: AuditInput) -> Self {
        Self(CaptureDraftKind::SummaryCommitted(
            CaptureSummaryCommittedDraft { summary, audit },
        ))
    }
}

enum RecognitionDraftKind {
    Requested(ObservationDraft),
    Completed(ObservationResultDraft),
    Failed(DiagnosticOutcomeDraft),
}

pub struct RecognitionPayloadDraft(RecognitionDraftKind);

impl RecognitionPayloadDraft {
    pub fn requested(action: EventAction, audit: AuditInput) -> Self {
        Self(RecognitionDraftKind::Requested(ObservationDraft::new(
            action, audit,
        )))
    }

    pub fn completed(
        action: EventAction,
        effect: EffectDisposition,
        frame_width: u32,
        frame_height: u32,
        verdict: RecognitionVerdict,
        audit: AuditInput,
    ) -> Self {
        Self(RecognitionDraftKind::Completed(
            ObservationResultDraft::new(
                action,
                effect,
                frame_width,
                frame_height,
                Some(verdict),
                None,
                audit,
            ),
        ))
    }

    pub fn failed(
        action: EventAction,
        diagnostic_code: DiagnosticCode,
        effect: EffectDisposition,
        audit: AuditInput,
    ) -> Self {
        Self(RecognitionDraftKind::Failed(DiagnosticOutcomeDraft::new(
            action,
            diagnostic_code,
            effect,
            audit,
        )))
    }
}

enum ArtifactDraftKind {
    Retention(Box<crate::ArtifactRetentionFact>, AuditInput),
    Created(OutcomeDraft),
    Verified(OutcomeDraft),
    StoreFailed(DiagnosticOutcomeDraft),
    VerificationFailed(DiagnosticOutcomeDraft),
    ExportCompleted(ArtifactExportDraft),
    ExportFailed(ArtifactExportFailureDraft),
}

pub struct ArtifactPayloadDraft(ArtifactDraftKind);

impl ArtifactPayloadDraft {
    pub fn persistence_failed(record: ArtifactFailureDraft, audit: AuditInput) -> Self {
        let verification = record.stage == ArtifactFailureStage::PublishedVerification;
        let mut detail = DiagnosticOutcomeDraft::new(
            if verification {
                EventAction::ArtifactVerify
            } else {
                EventAction::ArtifactStore
            },
            if verification {
                DiagnosticCode::ArtifactVerifyFailed
            } else {
                DiagnosticCode::ArtifactWriteFailed
            },
            EffectDisposition::Indeterminate,
            audit,
        );
        detail.artifact_failure = Some(Box::new(record));
        Self(if verification {
            ArtifactDraftKind::VerificationFailed(detail)
        } else {
            ArtifactDraftKind::StoreFailed(detail)
        })
    }
    pub fn created(audit: AuditInput) -> Self {
        Self(ArtifactDraftKind::Created(OutcomeDraft::new(
            EventAction::ArtifactStore,
            EffectDisposition::Performed,
            audit,
        )))
    }

    pub fn verified(audit: AuditInput) -> Self {
        Self(ArtifactDraftKind::Verified(OutcomeDraft::new(
            EventAction::ArtifactVerify,
            EffectDisposition::Performed,
            audit,
        )))
    }

    pub fn store_failed(diagnostic_code: DiagnosticCode, audit: AuditInput) -> Self {
        Self(ArtifactDraftKind::StoreFailed(DiagnosticOutcomeDraft::new(
            EventAction::ArtifactStore,
            diagnostic_code,
            EffectDisposition::Indeterminate,
            audit,
        )))
    }

    pub fn verification_failed(diagnostic_code: DiagnosticCode, audit: AuditInput) -> Self {
        Self(ArtifactDraftKind::VerificationFailed(
            DiagnosticOutcomeDraft::new(
                EventAction::ArtifactVerify,
                diagnostic_code,
                EffectDisposition::Indeterminate,
                audit,
            ),
        ))
    }

    pub fn export_completed(
        task_outcome: TaskOutcome,
        evidence_completeness: EvidenceCompleteness,
        artifact_count: u64,
        audit: AuditInput,
    ) -> Self {
        Self(ArtifactDraftKind::ExportCompleted(ArtifactExportDraft {
            action: EventAction::ArtifactExport,
            effect_disposition: EffectDisposition::Performed,
            task_outcome,
            evidence_completeness,
            artifact_count,
            audit,
        }))
    }

    pub fn export_failed(
        diagnostic_code: DiagnosticCode,
        task_outcome: TaskOutcome,
        evidence_completeness: EvidenceCompleteness,
        artifact_count: u64,
        audit: AuditInput,
    ) -> Self {
        Self(ArtifactDraftKind::ExportFailed(
            ArtifactExportFailureDraft {
                action: EventAction::ArtifactExport,
                diagnostic_code,
                effect_disposition: EffectDisposition::NotPerformed,
                task_outcome,
                evidence_completeness,
                artifact_count,
                audit,
            },
        ))
    }
}

enum ClientDraftKind {
    Action(ClientActionDraft),
    UiAction(ObservationDraft),
    CliCommand(ObservationDraft),
    LabRequest(ObservationDraft),
    LabPinRelease(crate::LabPinReleaseTarget, AuditInput),
    OwnerUnlock(OwnerUnlockDraft),
    GovernanceIdentityDeclared(GovernanceIdentityDeclaredDraft),
}

pub struct ClientPayloadDraft(ClientDraftKind);

impl ClientPayloadDraft {
    pub fn action(record: ClientActionRecord, audit: AuditInput) -> Self {
        Self(ClientDraftKind::Action(ClientActionDraft { record, audit }))
    }

    pub fn ui_action(action: EventAction, audit: AuditInput) -> Self {
        Self(ClientDraftKind::UiAction(ObservationDraft::new(
            action, audit,
        )))
    }

    pub fn cli_command(action: EventAction, audit: AuditInput) -> Self {
        Self(ClientDraftKind::CliCommand(ObservationDraft::new(
            action, audit,
        )))
    }

    pub fn lab_request(action: EventAction, audit: AuditInput) -> Self {
        Self(ClientDraftKind::LabRequest(ObservationDraft::new(
            action, audit,
        )))
    }
}

pub struct ResourceAuthoringPayloadDraft(ResourceAuthoringDraft);

impl ResourceAuthoringPayloadDraft {
    pub fn event(
        phase: ResourceAuthoringPhase,
        draft_id: impl Into<String>,
        target_label: impl Into<String>,
        target_fingerprint: impl Into<String>,
        changed_paths: Vec<String>,
        outcome: Option<Outcome>,
        audit: AuditInput,
    ) -> Self {
        Self(ResourceAuthoringDraft {
            phase,
            draft_id: draft_id.into(),
            target_label: target_label.into(),
            target_fingerprint: target_fingerprint.into(),
            changed_paths,
            outcome,
            audit,
        })
    }
}

enum LedgerDraftKind {
    Recovered(RecoveryDraft),
    Signature(LedgerSignatureEvent, AuditInput),
    /// Workflow #383 AO-1: `ledger.dump_imported` (register L3).
    DumpImported(Outcome, AuditInput),
    /// Workflow #383 AO-1: a damaged dump's gap (register L1).
    DumpGap {
        gap_kind: LedgerGapKind,
        gap_start_unix_ms: u64,
        gap_end_unix_ms: u64,
        outcome: Outcome,
        audit: AuditInput,
    },
    /// Workflow #383 AO-2: the single-record gap of a record the flush thread refused
    /// (register L1, `record_rejected`).
    RejectedGap {
        captured_at_unix_ms: u64,
        event_id: EventId,
        source: Option<SourceRevision>,
        outcome: Outcome,
        audit: AuditInput,
    },
    /// Workflow #383 AO-2: a finding of the capture queue or the flush thread (register L2).
    Health {
        outcome: Outcome,
        segment_edge: Option<LedgerSegmentEdge>,
        audit: AuditInput,
    },
}

pub struct LedgerPayloadDraft(LedgerDraftKind);

impl LedgerPayloadDraft {
    pub fn migrated(record: LedgerMigrationRecord, audit: AuditInput) -> Self {
        Self(LedgerDraftKind::Recovered(RecoveryDraft {
            reason: RecoveryReason::StorageCutover,
            migration: Some(Box::new(record)),
            segment_index: None,
            affected_bytes: 0,
            audit,
        }))
    }
    pub fn signature(event: LedgerSignatureEvent, audit: AuditInput) -> Self {
        Self(LedgerDraftKind::Signature(event, audit))
    }

    pub fn recovered(
        reason: RecoveryReason,
        segment_index: Option<u64>,
        affected_bytes: u64,
        audit: AuditInput,
    ) -> Self {
        Self(LedgerDraftKind::Recovered(RecoveryDraft {
            reason,
            migration: None,
            segment_index,
            affected_bytes,
            audit,
        }))
    }

    /// Workflow #383 AO-1 (`ledger-observer.md` §9.4 step 5): the ledger's own record of a dump
    /// it imported, `outcome` being `ledger_dump_imported` (register L3).
    pub fn dump_imported(outcome: Outcome, audit: AuditInput) -> Self {
        Self(LedgerDraftKind::DumpImported(outcome, audit))
    }

    /// Workflow #383 AO-1 (`ledger-observer.md` §9.4, §10): the gap of a damaged dump
    /// (`dump_incomplete` or `dump_invalid`): its time range, the count unknown and no record
    /// named; `outcome` is the gap's code (register L1).
    pub fn dump_gap(
        gap_kind: LedgerGapKind,
        gap_start_unix_ms: u64,
        gap_end_unix_ms: u64,
        outcome: Outcome,
        audit: AuditInput,
    ) -> Self {
        Self(LedgerDraftKind::DumpGap {
            gap_kind,
            gap_start_unix_ms,
            gap_end_unix_ms,
            outcome,
            audit,
        })
    }

    /// Workflow #383 AO-2 (`ledger-observer.md` §10): the single-record gap of the record
    /// `event_id`, captured at `captured_at_unix_ms`, that failed assembly or sanitizing;
    /// `source` is its revisioned source; `outcome` is `ledger_record_rejected` (register L1).
    pub fn rejected_gap(
        captured_at_unix_ms: u64,
        event_id: EventId,
        source: Option<SourceRevision>,
        outcome: Outcome,
        audit: AuditInput,
    ) -> Self {
        Self(LedgerDraftKind::RejectedGap {
            captured_at_unix_ms,
            event_id,
            source,
            outcome,
            audit,
        })
    }

    /// Workflow #383 AO-2 (`ledger-observer.md` §14.1): one finding, `outcome` carrying its
    /// values; a watermark or spill finding names the edge of its segment (register L2).
    pub fn health(
        outcome: Outcome,
        segment_edge: Option<LedgerSegmentEdge>,
        audit: AuditInput,
    ) -> Self {
        Self(LedgerDraftKind::Health {
            outcome,
            segment_edge,
            audit,
        })
    }
}

enum PerformanceDraftKind {
    PressureStarted(PerformancePressureDraft),
    PressureEnded(PerformancePressureDraft),
    StutterDetected(PerformanceStutterDraft),
    Summary(Box<PerformanceSummaryDraft>),
    MonitorDegraded(PerformanceMonitorStateDraft),
    MonitorRecovered(PerformanceMonitorStateDraft),
    BalanceChanged(PerformanceControlDraft),
    PacingChanged(PerformancePacingDraft),
}

pub struct PerformancePayloadDraft(PerformanceDraftKind);

impl PerformancePayloadDraft {
    pub fn pressure_started(data: PerformancePressureEventData, audit: AuditInput) -> Self {
        Self(PerformanceDraftKind::PressureStarted(
            PerformancePressureDraft { data, audit },
        ))
    }

    pub fn pressure_ended(data: PerformancePressureEventData, audit: AuditInput) -> Self {
        Self(PerformanceDraftKind::PressureEnded(
            PerformancePressureDraft { data, audit },
        ))
    }

    pub fn stutter_detected(data: PerformanceStutterEventData, audit: AuditInput) -> Self {
        Self(PerformanceDraftKind::StutterDetected(
            PerformanceStutterDraft { data, audit },
        ))
    }

    pub fn summary(data: PerformanceSummaryEventData, audit: AuditInput) -> Self {
        Self(PerformanceDraftKind::Summary(Box::new(
            PerformanceSummaryDraft { data, audit },
        )))
    }

    pub fn monitor_degraded(data: PerformanceMonitorStateEventData, audit: AuditInput) -> Self {
        Self(PerformanceDraftKind::MonitorDegraded(
            PerformanceMonitorStateDraft { data, audit },
        ))
    }

    pub fn monitor_recovered(data: PerformanceMonitorStateEventData, audit: AuditInput) -> Self {
        Self(PerformanceDraftKind::MonitorRecovered(
            PerformanceMonitorStateDraft { data, audit },
        ))
    }

    pub fn balance_changed(data: PerformanceControlEventData, audit: AuditInput) -> Self {
        Self(PerformanceDraftKind::BalanceChanged(
            PerformanceControlDraft { data, audit },
        ))
    }

    /// Workflow #385 P-m (register P1): `perf.pacing_changed`, one stage transition of the
    /// shadow measurement; `outcome` is its info code (`perf_stage_changed`) with its values.
    pub fn pacing_changed(outcome: Outcome, audit: AuditInput) -> Self {
        Self(PerformanceDraftKind::PacingChanged(
            PerformancePacingDraft { outcome, audit },
        ))
    }
}

enum FactDraftKind {
    Published(FactPublishedDraft),
    Invalidated(FactInvalidatedDraft),
}

pub struct FactPayloadDraft(FactDraftKind);

impl FactPayloadDraft {
    pub fn published(record: FactRecord, audit: AuditInput) -> Self {
        Self(FactDraftKind::Published(FactPublishedDraft {
            record: Box::new(record),
            related_records: Vec::new(),
            scope_instances: Vec::new(),
            audit,
        }))
    }

    pub fn observation(
        mut observation: crate::FactObservation,
        scope_instances: Vec<crate::InstanceId>,
        audit: AuditInput,
    ) -> Result<Self, SanitizationError> {
        observation.validate()?;
        observation
            .records
            .sort_by(|left, right| left.key.cmp(&right.key));
        let record = observation.records.remove(0);
        Ok(Self(FactDraftKind::Published(FactPublishedDraft {
            record: Box::new(record),
            related_records: observation.records,
            scope_instances,
            audit,
        })))
    }

    pub fn invalidated(invalidation: FactInvalidationEventData, audit: AuditInput) -> Self {
        Self(FactDraftKind::Invalidated(FactInvalidatedDraft {
            invalidation,
            audit,
        }))
    }
}

enum ApprovalDraftKind {
    Decision(ApprovalDecisionDraft),
}

pub struct ApprovalPayloadDraft(ApprovalDraftKind);

impl ApprovalPayloadDraft {
    pub fn decision(decision: ApprovalDecisionRecord, audit: AuditInput) -> Self {
        Self(ApprovalDraftKind::Decision(ApprovalDecisionDraft {
            decision,
            audit,
        }))
    }
}

enum PolicyDraftKind {
    Intent(PolicyDispatchDraft),
    Admitted(PolicyDispatchDraft),
    Rejected(PolicyDispatchDraft),
    Completed(PolicyDispatchDraft),
    Execution(PolicyExecutionDraft),
    PlanningSignal(PolicyPlanningSignalDraft),
}

pub struct PolicyPayloadDraft(PolicyDraftKind);

impl PolicyPayloadDraft {
    pub fn dispatch_intent(data: PolicyDispatchEventData, audit: AuditInput) -> Self {
        Self(PolicyDraftKind::Intent(PolicyDispatchDraft {
            data,
            admission: None,
            rejection: None,
            diagnostic_code: None,
            effect_disposition: None,
            random_delay: None,
            audit,
        }))
    }

    /// Workflow #405 D1 (register X14): the intent of a dispatch made under a random delay,
    /// which records the delay; only an intent can carry it.
    pub fn dispatch_intent_with_random_delay(
        data: PolicyDispatchEventData,
        random_delay: PolicyRandomDelay,
        audit: AuditInput,
    ) -> Self {
        Self(PolicyDraftKind::Intent(PolicyDispatchDraft {
            data,
            admission: None,
            rejection: None,
            diagnostic_code: None,
            effect_disposition: None,
            random_delay: Some(random_delay),
            audit,
        }))
    }

    pub fn dispatch_admitted(
        data: PolicyDispatchEventData,
        admission: PolicyAdmissionRecord,
        audit: AuditInput,
    ) -> Self {
        Self(PolicyDraftKind::Admitted(PolicyDispatchDraft {
            data,
            admission: Some(Box::new(admission)),
            rejection: None,
            diagnostic_code: None,
            effect_disposition: Some(EffectDisposition::Performed),
            random_delay: None,
            audit,
        }))
    }

    pub fn dispatch_rejected(
        data: PolicyDispatchEventData,
        effect_disposition: EffectDisposition,
        audit: AuditInput,
    ) -> Self {
        Self(PolicyDraftKind::Rejected(PolicyDispatchDraft {
            data,
            admission: None,
            rejection: None,
            diagnostic_code: Some(DiagnosticCode::PolicyRejected),
            effect_disposition: Some(effect_disposition),
            random_delay: None,
            audit,
        }))
    }

    pub fn dispatch_completed(
        data: PolicyDispatchEventData,
        admission: PolicyAdmissionRecord,
        audit: AuditInput,
    ) -> Self {
        Self(PolicyDraftKind::Completed(PolicyDispatchDraft {
            data,
            admission: Some(Box::new(admission)),
            rejection: None,
            diagnostic_code: None,
            effect_disposition: Some(EffectDisposition::Performed),
            random_delay: None,
            audit,
        }))
    }

    pub fn dispatch_rejected_with_reason(
        data: PolicyDispatchEventData,
        effect_disposition: EffectDisposition,
        rejection: PolicyDispatchRejection,
        audit: AuditInput,
    ) -> Self {
        Self(PolicyDraftKind::Rejected(PolicyDispatchDraft {
            data,
            admission: None,
            rejection: Some(Box::new(rejection)),
            diagnostic_code: Some(DiagnosticCode::PolicyRejected),
            effect_disposition: Some(effect_disposition),
            random_delay: None,
            audit,
        }))
    }

    pub fn execution_recorded(data: PolicyExecutionEventData, audit: AuditInput) -> Self {
        Self(PolicyDraftKind::Execution(PolicyExecutionDraft {
            data,
            elapsed: None,
            audit,
        }))
    }

    /// Workflow #385 P-k (register P6): the record of a run settled on its budget time, with
    /// its wall time `wall_elapsed_ms` and that budget time `budget_elapsed_ms` (never above
    /// the wall time).
    pub fn execution_recorded_with_elapsed(
        data: PolicyExecutionEventData,
        wall_elapsed_ms: u64,
        budget_elapsed_ms: u64,
        audit: AuditInput,
    ) -> Self {
        Self(PolicyDraftKind::Execution(
            PolicyExecutionDraft {
                data,
                elapsed: None,
                audit,
            }
            .with_elapsed(wall_elapsed_ms, budget_elapsed_ms),
        ))
    }

    pub fn planning_signal_observed(
        data: PolicyPlanningSignalEventData,
        audit: AuditInput,
    ) -> Self {
        Self(PolicyDraftKind::PlanningSignal(PolicyPlanningSignalDraft {
            data,
            audit,
        }))
    }
}

enum CatalogDraftKind {
    TransitionIntent(CatalogTransitionDraft),
    Activated(CatalogTransitionDraft),
    RolledBack(CatalogTransitionDraft),
    TransitionFailed(CatalogTransitionDraft),
}

pub struct CatalogPayloadDraft(CatalogDraftKind);

impl CatalogPayloadDraft {
    pub fn transition_intent(
        action: EventAction,
        data: CatalogTransitionEventData,
        audit: AuditInput,
    ) -> Self {
        Self(CatalogDraftKind::TransitionIntent(CatalogTransitionDraft {
            action,
            data,
            diagnostic_code: None,
            effect_disposition: None,
            audit,
        }))
    }

    pub fn activated(data: CatalogTransitionEventData, audit: AuditInput) -> Self {
        Self(CatalogDraftKind::Activated(CatalogTransitionDraft {
            action: EventAction::CatalogActivate,
            data,
            diagnostic_code: None,
            effect_disposition: Some(EffectDisposition::Performed),
            audit,
        }))
    }

    pub fn rolled_back(data: CatalogTransitionEventData, audit: AuditInput) -> Self {
        Self(CatalogDraftKind::RolledBack(CatalogTransitionDraft {
            action: EventAction::CatalogRollback,
            data,
            diagnostic_code: None,
            effect_disposition: Some(EffectDisposition::Performed),
            audit,
        }))
    }

    pub fn transition_failed(
        action: EventAction,
        data: CatalogTransitionEventData,
        effect_disposition: EffectDisposition,
        audit: AuditInput,
    ) -> Self {
        Self(CatalogDraftKind::TransitionFailed(CatalogTransitionDraft {
            action,
            data,
            diagnostic_code: Some(DiagnosticCode::CatalogTransitionFailed),
            effect_disposition: Some(effect_disposition),
            audit,
        }))
    }
}

enum StateDraftKind {
    Migrated(StateMigrationDraft),
}

pub struct StatePayloadDraft(StateDraftKind);

impl StatePayloadDraft {
    pub fn migrated(migration: StateMigrationData, audit: AuditInput) -> Self {
        Self(StateDraftKind::Migrated(StateMigrationDraft {
            migration,
            audit,
        }))
    }
}

enum ReleaseDraftKind {
    Staged(ReleaseStagedDraft),
    TransitionIntent(ReleaseTransitionDraft),
    Activated(ReleaseTransitionDraft),
    RolledBack(ReleaseTransitionDraft),
    TransitionFailed(ReleaseTransitionDraft),
}

pub struct ReleasePayloadDraft(ReleaseDraftKind);

impl ReleasePayloadDraft {
    pub fn staged(manifest: RuntimeReleaseSet, audit: AuditInput) -> Self {
        Self(ReleaseDraftKind::Staged(ReleaseStagedDraft {
            manifest,
            audit,
        }))
    }

    pub fn transition_intent(transition: ReleaseTransitionData, audit: AuditInput) -> Self {
        Self(ReleaseDraftKind::TransitionIntent(ReleaseTransitionDraft {
            action: release_action(transition.kind()),
            transition,
            diagnostic_code: None,
            effect_disposition: None,
            audit,
        }))
    }

    pub fn activated(transition: ReleaseTransitionData, audit: AuditInput) -> Self {
        Self(ReleaseDraftKind::Activated(ReleaseTransitionDraft {
            action: EventAction::ReleaseActivate,
            transition,
            diagnostic_code: None,
            effect_disposition: Some(EffectDisposition::Performed),
            audit,
        }))
    }

    pub fn rolled_back(transition: ReleaseTransitionData, audit: AuditInput) -> Self {
        Self(ReleaseDraftKind::RolledBack(ReleaseTransitionDraft {
            action: EventAction::ReleaseRollback,
            transition,
            diagnostic_code: None,
            effect_disposition: Some(EffectDisposition::Performed),
            audit,
        }))
    }

    pub fn transition_failed(
        transition: ReleaseTransitionData,
        effect_disposition: EffectDisposition,
        audit: AuditInput,
    ) -> Self {
        Self(ReleaseDraftKind::TransitionFailed(ReleaseTransitionDraft {
            action: release_action(transition.kind()),
            transition,
            diagnostic_code: Some(DiagnosticCode::ReleaseTransitionFailed),
            effect_disposition: Some(effect_disposition),
            audit,
        }))
    }
}

enum AgentDraftKind {
    WakeRequested(AgentWakeDraft),
    SessionStarted(AgentSessionDraft),
    SessionResumed(AgentSessionDraft),
    ResponseRecorded(AgentSessionDraft),
    SessionCompleted(AgentSessionDraft),
    SessionEscalated(AgentSessionDraft),
}

pub struct AgentPayloadDraft(AgentDraftKind);

impl AgentPayloadDraft {
    pub fn wake_requested(wake: AgentWakeData, audit: AuditInput) -> Self {
        Self(AgentDraftKind::WakeRequested(AgentWakeDraft {
            wake,
            audit,
        }))
    }

    pub fn session_started(session: AgentSessionEventData, audit: AuditInput) -> Self {
        Self(AgentDraftKind::SessionStarted(AgentSessionDraft {
            action: EventAction::AgentSessionStart,
            session,
            audit,
        }))
    }

    pub fn session_resumed(session: AgentSessionEventData, audit: AuditInput) -> Self {
        Self(AgentDraftKind::SessionResumed(AgentSessionDraft {
            action: EventAction::AgentSessionResume,
            session,
            audit,
        }))
    }

    pub fn response_recorded(session: AgentSessionEventData, audit: AuditInput) -> Self {
        Self(AgentDraftKind::ResponseRecorded(AgentSessionDraft {
            action: EventAction::AgentSessionRespond,
            session,
            audit,
        }))
    }

    pub fn session_completed(session: AgentSessionEventData, audit: AuditInput) -> Self {
        Self(AgentDraftKind::SessionCompleted(AgentSessionDraft {
            action: EventAction::AgentSessionComplete,
            session,
            audit,
        }))
    }

    pub fn session_escalated(session: AgentSessionEventData, audit: AuditInput) -> Self {
        Self(AgentDraftKind::SessionEscalated(AgentSessionDraft {
            action: EventAction::AgentSessionEscalate,
            session,
            audit,
        }))
    }
}

const fn release_action(kind: ReleaseTransitionKind) -> EventAction {
    match kind {
        ReleaseTransitionKind::Activate => EventAction::ReleaseActivate,
        ReleaseTransitionKind::Rollback => EventAction::ReleaseRollback,
    }
}

pub enum EventPayloadDraft {
    Provider(ProviderPayloadDraft),
    Runtime(RuntimePayloadDraft),
    Monitor(MonitorPayloadDraft),
    Performance(PerformancePayloadDraft),
    Fact(FactPayloadDraft),
    Approval(ApprovalPayloadDraft),
    Command(CommandPayloadDraft),
    Scheduler(SchedulerPayloadDraft),
    Policy(PolicyPayloadDraft),
    Catalog(CatalogPayloadDraft),
    State(StatePayloadDraft),
    Release(ReleasePayloadDraft),
    Agent(AgentPayloadDraft),
    Lease(LeasePayloadDraft),
    Task(TaskPayloadDraft),
    Application(ApplicationPayloadDraft),
    Input(InputPayloadDraft),
    Capture(CapturePayloadDraft),
    Recognition(RecognitionPayloadDraft),
    Artifact(ArtifactPayloadDraft),
    ResourceAuthoring(ResourceAuthoringPayloadDraft),
    Client(ClientPayloadDraft),
    Ledger(LedgerPayloadDraft),
}

macro_rules! payload_draft_from {
    ($type:ty, $variant:ident) => {
        impl From<$type> for EventPayloadDraft {
            fn from(value: $type) -> Self {
                Self::$variant(value)
            }
        }
    };
}

payload_draft_from!(CommandPayloadDraft, Command);
payload_draft_from!(ProviderPayloadDraft, Provider);
payload_draft_from!(RuntimePayloadDraft, Runtime);
payload_draft_from!(MonitorPayloadDraft, Monitor);
payload_draft_from!(PerformancePayloadDraft, Performance);
payload_draft_from!(FactPayloadDraft, Fact);
payload_draft_from!(ApprovalPayloadDraft, Approval);
payload_draft_from!(SchedulerPayloadDraft, Scheduler);
payload_draft_from!(PolicyPayloadDraft, Policy);
payload_draft_from!(CatalogPayloadDraft, Catalog);
payload_draft_from!(StatePayloadDraft, State);
payload_draft_from!(ReleasePayloadDraft, Release);
payload_draft_from!(AgentPayloadDraft, Agent);
payload_draft_from!(LeasePayloadDraft, Lease);
payload_draft_from!(TaskPayloadDraft, Task);
payload_draft_from!(ApplicationPayloadDraft, Application);
payload_draft_from!(InputPayloadDraft, Input);
payload_draft_from!(CapturePayloadDraft, Capture);
payload_draft_from!(RecognitionPayloadDraft, Recognition);
payload_draft_from!(ArtifactPayloadDraft, Artifact);
payload_draft_from!(ResourceAuthoringPayloadDraft, ResourceAuthoring);
payload_draft_from!(ClientPayloadDraft, Client);
payload_draft_from!(LedgerPayloadDraft, Ledger);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum CommandPayload {
    Received(ObservationPayload),
    Validated(OutcomePayload),
    Rejected(DiagnosticOutcomePayload),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum RuntimePayload {
    Started(ObservationPayload),
    Takeover(ObservationPayload),
    Failed(DiagnosticOutcomePayload),
    LifecycleObserved(RuntimeLifecyclePayload),
    InstanceBound(RuntimeInstanceBindingPayload),
    FactRecorded(RuntimeFactRecordedPayload),
    FactInvalidated(RuntimeFactInvalidatedPayload),
    FactSnapshot(RuntimeFactSnapshotPayload),
    /// `device.self_check` (Workflow #317 sc3), a status hint under the lifecycle event type.
    DeviceSelfCheck(DeviceSelfCheckPayload),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum MonitorPayload {
    Requested(ObservationPayload),
    Started(ObservationPayload),
    Completed(MonitorOutcomePayload),
    Failed(DiagnosticOutcomePayload),
    RecoveryAdmitted(MonitorRecoveryCoordinationPayload),
    RecoveryDeferred(MonitorRecoveryCoordinationPayload),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PerformancePayload {
    PressureStarted(PerformancePressurePayload),
    PressureEnded(PerformancePressurePayload),
    StutterDetected(PerformanceStutterPayload),
    Summary(Box<PerformanceSummaryPayload>),
    MonitorDegraded(PerformanceMonitorStatePayload),
    MonitorRecovered(PerformanceMonitorStatePayload),
    BalanceChanged(PerformanceControlPayload),
    PacingChanged(PerformancePacingPayload),
    DispatchShareChanged(PerformancePacingPayload),
    RunHold(PerformancePacingPayload),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum FactPayload {
    Published(FactPublishedPayload),
    Invalidated(FactInvalidatedPayload),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ApprovalPayload {
    Decision(ApprovalDecisionPayload),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum SchedulerPayload {
    Admitted(ObservationPayload),
    Queued(SchedulerQueuePayload),
    Denied(DiagnosticPayload),
    Preempted(SchedulerPreemptionPayload),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PolicyPayload {
    DispatchIntent(PolicyDispatchPayload),
    DispatchAdmitted(PolicyDispatchPayload),
    DispatchRejected(PolicyDispatchPayload),
    DispatchCompleted(PolicyDispatchPayload),
    ExecutionRecorded(PolicyExecutionPayload),
    PlanningSignalObserved(PolicyPlanningSignalPayload),
    ControlStateRecorded(PolicyControlStatePayload),
    ActivityStateRecorded(PolicyActivityStatePayload),
    FactRefreshTransition(PolicyFactRefreshTransitionPayload),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum CatalogPayload {
    TransitionIntent(CatalogTransitionPayload),
    Activated(CatalogTransitionPayload),
    RolledBack(CatalogTransitionPayload),
    TransitionFailed(CatalogTransitionPayload),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum StatePayload {
    Migrated(StateMigrationPayload),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ReleasePayload {
    Staged(ReleaseStagedPayload),
    TransitionIntent(ReleaseTransitionPayload),
    Activated(ReleaseTransitionPayload),
    RolledBack(ReleaseTransitionPayload),
    TransitionFailed(ReleaseTransitionPayload),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum AgentPayload {
    WakeRequested(AgentWakePayload),
    SessionStarted(AgentSessionPayload),
    SessionResumed(AgentSessionPayload),
    ResponseRecorded(AgentSessionPayload),
    SessionCompleted(AgentSessionPayload),
    SessionEscalated(AgentSessionPayload),
    WakeObligationRecorded(AgentWakeObligationPayload),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum LeasePayload {
    Requested(ObservationPayload),
    Granted(OutcomePayload),
    Transferred(LeaseTransferPayload),
    Renewed(OutcomePayload),
    Released(OutcomePayload),
    Expired(OutcomePayload),
    TransitionIntent(ObservationPayload),
    TransitionFailed(DiagnosticOutcomePayload),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum TaskPayload {
    Requested(ObservationPayload),
    Started(ObservationPayload),
    StepStarted(ObservationPayload),
    StepFinished(ObservationPayload),
    Completed(OutcomePayload),
    Failed(DiagnosticOutcomePayload),
    Cancelled(OutcomePayload),
    TerminalIntent(ObservationPayload),
    TerminalCommitFailed(DiagnosticOutcomePayload),
    Semantic(TaskSemanticPayload),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ApplicationPayload {
    Intent(ObservationPayload),
    Completed(OutcomePayload),
    Failed(DiagnosticOutcomePayload),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum InputPayload {
    Intent(InputIntentPayload),
    Committed(OutcomePayload),
    Completed(ObservationPayload),
    Failed(DiagnosticOutcomePayload),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum CapturePayload {
    Requested(ObservationPayload),
    Completed(ObservationResultPayload),
    Failed(DiagnosticOutcomePayload),
    PressureChanged(CapturePressurePayload),
    DedupWindow(CaptureDedupWindowPayload),
    PolicyChanged(CapturePolicyPayload),
    SummaryCommitted(CaptureSummaryCommittedPayload),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum RecognitionPayload {
    Requested(ObservationPayload),
    Completed(ObservationResultPayload),
    Failed(DiagnosticOutcomePayload),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ArtifactPayload {
    Retention(ArtifactRetentionPayload),
    Created(OutcomePayload),
    Verified(OutcomePayload),
    StoreFailed(DiagnosticOutcomePayload),
    VerificationFailed(DiagnosticOutcomePayload),
    ExportCompleted(ArtifactExportPayload),
    ExportFailed(ArtifactExportFailurePayload),
}

impl FamilyPayload for ResourceAuthoringPayload {
    fn event_type(&self) -> EventType {
        match self.phase {
            ResourceAuthoringPhase::AuthoringStarted => EventType::ResourceAuthoringStarted,
            ResourceAuthoringPhase::DraftBuilt => EventType::ResourceDraftBuilt,
            ResourceAuthoringPhase::ValidationCompleted => EventType::ResourceValidationCompleted,
            ResourceAuthoringPhase::PromoteIntent => EventType::ResourcePromoteIntent,
            ResourceAuthoringPhase::Promoted => EventType::ResourcePromoted,
            ResourceAuthoringPhase::PromoteFailed => EventType::ResourcePromoteFailed,
        }
    }

    fn detail(&self) -> &dyn PayloadDetail {
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ClientPayload {
    Action(ClientActionPayload),
    UiAction(ObservationPayload),
    CliCommand(ObservationPayload),
    LabRequest(ObservationPayload),
    LabPinRelease(LabPinReleaseRequestPayload),
    OwnerUnlock(OwnerUnlockPayload),
    GovernanceIdentityDeclared(GovernanceIdentityDeclaredPayload),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum LedgerPayload {
    Recovered(RecoveryPayload),
    Signature(LedgerSignaturePayload),
    GapRecorded(LedgerGapPayload),
    HealthObserved(LedgerHealthPayload),
    DumpImported(LedgerDumpImportedPayload),
}

trait FamilyPayload {
    fn event_type(&self) -> EventType;
    fn detail(&self) -> &dyn PayloadDetail;
}

macro_rules! family_payload {
    ($type:ty, { $($variant:ident => $event:expr),+ $(,)? }) => {
        impl FamilyPayload for $type {
            fn event_type(&self) -> EventType {
                match self {
                    $(Self::$variant(_) => $event),+
                }
            }

            fn detail(&self) -> &dyn PayloadDetail {
                match self {
                    $(Self::$variant(detail) => detail),+
                }
            }
        }
    };
}

family_payload!(CommandPayload, {
    Received => EventType::CommandReceived,
    Validated => EventType::CommandValidated,
    Rejected => EventType::CommandRejected,
});
family_payload!(RuntimePayload, {
    Started => EventType::RuntimeStarted,
    Takeover => EventType::RuntimeTakeover,
    Failed => EventType::RuntimeFailed,
    LifecycleObserved => EventType::RuntimeLifecycleObserved,
    InstanceBound => EventType::RuntimeInstanceBound,
    FactRecorded => EventType::RuntimeFactRecorded,
    FactInvalidated => EventType::RuntimeFactInvalidated,
    FactSnapshot => EventType::RuntimeFactSnapshot,
    DeviceSelfCheck => EventType::RuntimeLifecycleObserved,
});
family_payload!(MonitorPayload, {
    Requested => EventType::MonitorProbeRequested,
    Started => EventType::MonitorProbeStarted,
    Completed => EventType::MonitorProbeCompleted,
    Failed => EventType::MonitorProbeFailed,
    RecoveryAdmitted => EventType::MonitorRecoveryAdmitted,
    RecoveryDeferred => EventType::MonitorRecoveryDeferred,
});
impl FamilyPayload for PerformancePayload {
    fn event_type(&self) -> EventType {
        match self {
            Self::PressureStarted(_) => EventType::PerformancePressureStarted,
            Self::PressureEnded(_) => EventType::PerformancePressureEnded,
            Self::StutterDetected(_) => EventType::PerformanceStutterDetected,
            Self::Summary(_) => EventType::PerformanceSummary,
            Self::MonitorDegraded(_) => EventType::PerformanceMonitorDegraded,
            Self::MonitorRecovered(_) => EventType::PerformanceMonitorRecovered,
            Self::BalanceChanged(_) => EventType::PerformanceBalanceChanged,
            Self::PacingChanged(_) => EventType::PerformancePacingChanged,
            Self::DispatchShareChanged(_) => EventType::PerformanceDispatchShareChanged,
            Self::RunHold(_) => EventType::PerformanceRunHold,
        }
    }

    fn detail(&self) -> &dyn PayloadDetail {
        match self {
            Self::PressureStarted(detail) | Self::PressureEnded(detail) => detail,
            Self::StutterDetected(detail) => detail,
            Self::Summary(detail) => detail.as_ref(),
            Self::MonitorDegraded(detail) | Self::MonitorRecovered(detail) => detail,
            Self::BalanceChanged(detail) => detail,
            Self::PacingChanged(detail)
            | Self::DispatchShareChanged(detail)
            | Self::RunHold(detail) => detail,
        }
    }
}
family_payload!(FactPayload, {
    Published => EventType::FactPublished,
    Invalidated => EventType::FactInvalidated,
});
family_payload!(ApprovalPayload, {
    Decision => EventType::ApprovalDecision,
});
family_payload!(SchedulerPayload, {
    Admitted => EventType::SchedulerAdmitted,
    Queued => EventType::SchedulerQueued,
    Denied => EventType::SchedulerDenied,
    Preempted => EventType::SchedulerPreempted,
});
family_payload!(PolicyPayload, {
    DispatchIntent => EventType::PolicyDispatchIntent,
    DispatchAdmitted => EventType::PolicyDispatchAdmitted,
    DispatchRejected => EventType::PolicyDispatchRejected,
    DispatchCompleted => EventType::PolicyDispatchCompleted,
    ExecutionRecorded => EventType::PolicyExecutionRecorded,
    PlanningSignalObserved => EventType::PolicyPlanningSignalObserved,
    ControlStateRecorded => EventType::PolicyControlStateRecorded,
    ActivityStateRecorded => EventType::PolicyActivityStateRecorded,
    FactRefreshTransition => EventType::PolicyFactRefreshTransition,
});
family_payload!(CatalogPayload, {
    TransitionIntent => EventType::CatalogTransitionIntent,
    Activated => EventType::CatalogActivated,
    RolledBack => EventType::CatalogRolledBack,
    TransitionFailed => EventType::CatalogTransitionFailed,
});
family_payload!(StatePayload, {
    Migrated => EventType::StateMigrated,
});
family_payload!(ReleasePayload, {
    Staged => EventType::ReleaseStaged,
    TransitionIntent => EventType::ReleaseTransitionIntent,
    Activated => EventType::ReleaseActivated,
    RolledBack => EventType::ReleaseRolledBack,
    TransitionFailed => EventType::ReleaseTransitionFailed,
});
family_payload!(AgentPayload, {
    WakeRequested => EventType::AgentWakeRequested,
    SessionStarted => EventType::AgentSessionStarted,
    SessionResumed => EventType::AgentSessionResumed,
    ResponseRecorded => EventType::AgentResponseRecorded,
    SessionCompleted => EventType::AgentSessionCompleted,
    SessionEscalated => EventType::AgentSessionEscalated,
    WakeObligationRecorded => EventType::AgentWakeObligationRecorded,
});
family_payload!(LeasePayload, {
    Requested => EventType::LeaseRequested,
    Granted => EventType::LeaseGranted,
    Transferred => EventType::LeaseTransferred,
    Renewed => EventType::LeaseRenewed,
    Released => EventType::LeaseReleased,
    Expired => EventType::LeaseExpired,
    TransitionIntent => EventType::LeaseTransitionIntent,
    TransitionFailed => EventType::LeaseTransitionFailed,
});
impl FamilyPayload for TaskPayload {
    fn event_type(&self) -> EventType {
        match self {
            Self::Requested(_) => EventType::TaskRequested,
            Self::Started(_) => EventType::TaskStarted,
            Self::StepStarted(_) => EventType::TaskStepStarted,
            Self::StepFinished(_) => EventType::TaskStepFinished,
            Self::Completed(_) => EventType::TaskCompleted,
            Self::Failed(_) => EventType::TaskFailed,
            Self::Cancelled(_) => EventType::TaskCancelled,
            Self::TerminalIntent(_) => EventType::TaskTerminalIntent,
            Self::TerminalCommitFailed(_) => EventType::TaskTerminalCommitFailed,
            Self::Semantic(value) => value.fact.event_type(),
        }
    }

    fn detail(&self) -> &dyn PayloadDetail {
        match self {
            Self::Requested(value)
            | Self::Started(value)
            | Self::StepStarted(value)
            | Self::StepFinished(value)
            | Self::TerminalIntent(value) => value,
            Self::Completed(value) | Self::Cancelled(value) => value,
            Self::Failed(value) | Self::TerminalCommitFailed(value) => value,
            Self::Semantic(value) => value,
        }
    }
}
family_payload!(ApplicationPayload, {
    Intent => EventType::ApplicationIntent,
    Completed => EventType::ApplicationCompleted,
    Failed => EventType::ApplicationFailed,
});
family_payload!(InputPayload, {
    Intent => EventType::InputIntent,
    Committed => EventType::InputCommitted,
    Completed => EventType::InputCompleted,
    Failed => EventType::InputFailed,
});
family_payload!(CapturePayload, {
    Requested => EventType::CaptureRequested,
    Completed => EventType::CaptureCompleted,
    Failed => EventType::CaptureFailed,
    PressureChanged => EventType::CapturePressureChanged,
    DedupWindow => EventType::CaptureDedupWindow,
    PolicyChanged => EventType::CapturePolicyChanged,
    SummaryCommitted => EventType::CaptureSummaryCommitted,
});
family_payload!(RecognitionPayload, {
    Requested => EventType::RecognitionRequested,
    Completed => EventType::RecognitionCompleted,
    Failed => EventType::RecognitionFailed,
});
impl FamilyPayload for ArtifactPayload {
    fn event_type(&self) -> EventType {
        match self {
            Self::Retention(value) => value.event_type(),
            Self::Created(_) => EventType::ArtifactCreated,
            Self::Verified(_) => EventType::ArtifactVerified,
            Self::StoreFailed(_) => EventType::ArtifactStoreFailed,
            Self::VerificationFailed(_) => EventType::ArtifactVerificationFailed,
            Self::ExportCompleted(_) => EventType::ArtifactExportCompleted,
            Self::ExportFailed(_) => EventType::ArtifactExportFailed,
        }
    }
    fn detail(&self) -> &dyn PayloadDetail {
        match self {
            Self::Retention(value) => value,
            Self::Created(value) | Self::Verified(value) => value,
            Self::StoreFailed(value) | Self::VerificationFailed(value) => value,
            Self::ExportCompleted(value) => value,
            Self::ExportFailed(value) => value,
        }
    }
}
family_payload!(ClientPayload, {
    Action => EventType::ClientAction,
    UiAction => EventType::UiAction,
    CliCommand => EventType::CliCommand,
    LabRequest => EventType::LabRequest,
    LabPinRelease => EventType::LabRequest,
    OwnerUnlock => EventType::CliCommand,
    GovernanceIdentityDeclared => EventType::GovernanceIdentityDeclared,
});
impl FamilyPayload for LedgerPayload {
    fn event_type(&self) -> EventType {
        match self {
            Self::Recovered(_) => EventType::LedgerRecovered,
            Self::Signature(payload) => payload.record().event_type(),
            Self::GapRecorded(_) => EventType::LedgerGapRecorded,
            Self::HealthObserved(_) => EventType::LedgerHealthObserved,
            Self::DumpImported(_) => EventType::LedgerDumpImported,
        }
    }
    fn detail(&self) -> &dyn PayloadDetail {
        match self {
            Self::Recovered(payload) => payload,
            Self::Signature(payload) => payload,
            Self::GapRecorded(payload) => payload,
            Self::HealthObserved(payload) => payload,
            Self::DumpImported(payload) => payload,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "family",
    content = "payload",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum EventPayload {
    Provider(ProviderPayload),
    Runtime(RuntimePayload),
    Monitor(MonitorPayload),
    Performance(PerformancePayload),
    Fact(FactPayload),
    Approval(ApprovalPayload),
    Command(CommandPayload),
    Scheduler(SchedulerPayload),
    Policy(PolicyPayload),
    Catalog(CatalogPayload),
    State(StatePayload),
    Release(ReleasePayload),
    Agent(AgentPayload),
    Lease(LeasePayload),
    Task(TaskPayload),
    Application(ApplicationPayload),
    Input(InputPayload),
    Capture(CapturePayload),
    Recognition(RecognitionPayload),
    Artifact(ArtifactPayload),
    ResourceAuthoring(ResourceAuthoringPayload),
    Client(ClientPayload),
    Ledger(LedgerPayload),
}

impl EventPayloadDraft {
    pub(crate) fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<EventPayload, SanitizationError> {
        Ok(match self {
            Self::Provider(value) => EventPayload::Provider(value.sanitize(fingerprinter)?),
            Self::Runtime(value) => EventPayload::Runtime(match value.0 {
                RuntimeDraftKind::Started(detail) => {
                    RuntimePayload::Started(detail.sanitize(fingerprinter)?)
                }
                RuntimeDraftKind::Takeover(detail) => {
                    RuntimePayload::Takeover(detail.sanitize(fingerprinter)?)
                }
                RuntimeDraftKind::Failed(detail) => {
                    RuntimePayload::Failed(detail.sanitize(fingerprinter)?)
                }
                RuntimeDraftKind::LifecycleObserved(detail) => {
                    RuntimePayload::LifecycleObserved(detail.sanitize(fingerprinter)?)
                }
                RuntimeDraftKind::InstanceBound(detail) => {
                    RuntimePayload::InstanceBound(detail.sanitize(fingerprinter)?)
                }
                RuntimeDraftKind::FactRecorded(detail) => {
                    RuntimePayload::FactRecorded(detail.sanitize(fingerprinter)?)
                }
                RuntimeDraftKind::FactInvalidated(detail) => {
                    RuntimePayload::FactInvalidated(detail.sanitize(fingerprinter)?)
                }
                RuntimeDraftKind::FactSnapshot(detail) => {
                    RuntimePayload::FactSnapshot(detail.sanitize(fingerprinter)?)
                }
                RuntimeDraftKind::DeviceSelfCheck(detail) => {
                    RuntimePayload::DeviceSelfCheck(detail.sanitize(fingerprinter)?)
                }
            }),
            Self::Monitor(value) => EventPayload::Monitor(match value.0 {
                MonitorDraftKind::Requested(detail) => {
                    MonitorPayload::Requested(detail.sanitize(fingerprinter)?)
                }
                MonitorDraftKind::Started(detail) => {
                    MonitorPayload::Started(detail.sanitize(fingerprinter)?)
                }
                MonitorDraftKind::Completed(detail) => {
                    MonitorPayload::Completed(detail.sanitize(fingerprinter)?)
                }
                MonitorDraftKind::Failed(detail) => {
                    MonitorPayload::Failed(detail.sanitize(fingerprinter)?)
                }
                MonitorDraftKind::RecoveryAdmitted(detail) => {
                    MonitorPayload::RecoveryAdmitted(detail.sanitize(fingerprinter)?)
                }
                MonitorDraftKind::RecoveryDeferred(detail) => {
                    MonitorPayload::RecoveryDeferred(detail.sanitize(fingerprinter)?)
                }
            }),
            Self::Performance(value) => EventPayload::Performance(match value.0 {
                PerformanceDraftKind::PressureStarted(detail) => {
                    PerformancePayload::PressureStarted(detail.sanitize(fingerprinter)?)
                }
                PerformanceDraftKind::PressureEnded(detail) => {
                    PerformancePayload::PressureEnded(detail.sanitize(fingerprinter)?)
                }
                PerformanceDraftKind::StutterDetected(detail) => {
                    PerformancePayload::StutterDetected(detail.sanitize(fingerprinter)?)
                }
                PerformanceDraftKind::Summary(detail) => {
                    PerformancePayload::Summary(Box::new(detail.sanitize(fingerprinter)?))
                }
                PerformanceDraftKind::MonitorDegraded(detail) => {
                    PerformancePayload::MonitorDegraded(detail.sanitize(fingerprinter)?)
                }
                PerformanceDraftKind::MonitorRecovered(detail) => {
                    PerformancePayload::MonitorRecovered(detail.sanitize(fingerprinter)?)
                }
                PerformanceDraftKind::BalanceChanged(detail) => {
                    PerformancePayload::BalanceChanged(detail.sanitize(fingerprinter)?)
                }
                PerformanceDraftKind::PacingChanged(detail) => {
                    PerformancePayload::PacingChanged(PerformancePacingPayload::observed(
                        detail.outcome,
                        detail.audit.sanitize(fingerprinter)?,
                    )?)
                }
            }),
            Self::Fact(value) => EventPayload::Fact(match value.0 {
                FactDraftKind::Published(detail) => {
                    FactPayload::Published(detail.sanitize(fingerprinter)?)
                }
                FactDraftKind::Invalidated(detail) => {
                    FactPayload::Invalidated(detail.sanitize(fingerprinter)?)
                }
            }),
            Self::Approval(value) => EventPayload::Approval(match value.0 {
                ApprovalDraftKind::Decision(detail) => {
                    ApprovalPayload::Decision(detail.sanitize(fingerprinter)?)
                }
            }),
            Self::Command(value) => EventPayload::Command(match value.0 {
                CommandDraftKind::Received(detail) => {
                    CommandPayload::Received(detail.sanitize(fingerprinter)?)
                }
                CommandDraftKind::Validated(detail) => {
                    CommandPayload::Validated(detail.sanitize(fingerprinter)?)
                }
                CommandDraftKind::Rejected(detail) => {
                    CommandPayload::Rejected(detail.sanitize(fingerprinter)?)
                }
            }),
            Self::Scheduler(value) => EventPayload::Scheduler(match value.0 {
                SchedulerDraftKind::Admitted(detail) => {
                    SchedulerPayload::Admitted(detail.sanitize(fingerprinter)?)
                }
                SchedulerDraftKind::Queued(detail) => {
                    SchedulerPayload::Queued(detail.sanitize(fingerprinter)?)
                }
                SchedulerDraftKind::Denied(detail) => {
                    SchedulerPayload::Denied(detail.sanitize(fingerprinter)?)
                }
                SchedulerDraftKind::Preempted(detail) => {
                    SchedulerPayload::Preempted(detail.sanitize(fingerprinter)?)
                }
            }),
            Self::Policy(value) => EventPayload::Policy(match value.0 {
                PolicyDraftKind::Intent(detail) => {
                    PolicyPayload::DispatchIntent(detail.sanitize(fingerprinter)?)
                }
                PolicyDraftKind::Admitted(detail) => {
                    PolicyPayload::DispatchAdmitted(detail.sanitize(fingerprinter)?)
                }
                PolicyDraftKind::Rejected(detail) => {
                    PolicyPayload::DispatchRejected(detail.sanitize(fingerprinter)?)
                }
                PolicyDraftKind::Completed(detail) => {
                    PolicyPayload::DispatchCompleted(detail.sanitize(fingerprinter)?)
                }
                PolicyDraftKind::Execution(detail) => {
                    PolicyPayload::ExecutionRecorded(detail.sanitize(fingerprinter)?)
                }
                PolicyDraftKind::PlanningSignal(detail) => {
                    PolicyPayload::PlanningSignalObserved(detail.sanitize(fingerprinter)?)
                }
            }),
            Self::Catalog(value) => EventPayload::Catalog(match value.0 {
                CatalogDraftKind::TransitionIntent(detail) => {
                    CatalogPayload::TransitionIntent(detail.sanitize(fingerprinter)?)
                }
                CatalogDraftKind::Activated(detail) => {
                    CatalogPayload::Activated(detail.sanitize(fingerprinter)?)
                }
                CatalogDraftKind::RolledBack(detail) => {
                    CatalogPayload::RolledBack(detail.sanitize(fingerprinter)?)
                }
                CatalogDraftKind::TransitionFailed(detail) => {
                    CatalogPayload::TransitionFailed(detail.sanitize(fingerprinter)?)
                }
            }),
            Self::State(value) => EventPayload::State(match value.0 {
                StateDraftKind::Migrated(detail) => {
                    StatePayload::Migrated(detail.sanitize(fingerprinter)?)
                }
            }),
            Self::Release(value) => EventPayload::Release(match value.0 {
                ReleaseDraftKind::Staged(detail) => {
                    ReleasePayload::Staged(detail.sanitize(fingerprinter)?)
                }
                ReleaseDraftKind::TransitionIntent(detail) => {
                    ReleasePayload::TransitionIntent(detail.sanitize(fingerprinter)?)
                }
                ReleaseDraftKind::Activated(detail) => {
                    ReleasePayload::Activated(detail.sanitize(fingerprinter)?)
                }
                ReleaseDraftKind::RolledBack(detail) => {
                    ReleasePayload::RolledBack(detail.sanitize(fingerprinter)?)
                }
                ReleaseDraftKind::TransitionFailed(detail) => {
                    ReleasePayload::TransitionFailed(detail.sanitize(fingerprinter)?)
                }
            }),
            Self::Agent(value) => EventPayload::Agent(match value.0 {
                AgentDraftKind::WakeRequested(detail) => {
                    AgentPayload::WakeRequested(detail.sanitize(fingerprinter)?)
                }
                AgentDraftKind::SessionStarted(detail) => {
                    AgentPayload::SessionStarted(detail.sanitize(fingerprinter)?)
                }
                AgentDraftKind::SessionResumed(detail) => {
                    AgentPayload::SessionResumed(detail.sanitize(fingerprinter)?)
                }
                AgentDraftKind::ResponseRecorded(detail) => {
                    AgentPayload::ResponseRecorded(detail.sanitize(fingerprinter)?)
                }
                AgentDraftKind::SessionCompleted(detail) => {
                    AgentPayload::SessionCompleted(detail.sanitize(fingerprinter)?)
                }
                AgentDraftKind::SessionEscalated(detail) => {
                    AgentPayload::SessionEscalated(detail.sanitize(fingerprinter)?)
                }
            }),
            Self::Lease(value) => EventPayload::Lease(match value.0 {
                LeaseDraftKind::Requested(detail) => {
                    LeasePayload::Requested(detail.sanitize(fingerprinter)?)
                }
                LeaseDraftKind::Granted(detail) => {
                    LeasePayload::Granted(detail.sanitize(fingerprinter)?)
                }
                LeaseDraftKind::Transferred(detail) => {
                    LeasePayload::Transferred(detail.sanitize(fingerprinter)?)
                }
                LeaseDraftKind::Renewed(detail) => {
                    LeasePayload::Renewed(detail.sanitize(fingerprinter)?)
                }
                LeaseDraftKind::Released(detail) => {
                    LeasePayload::Released(detail.sanitize(fingerprinter)?)
                }
                LeaseDraftKind::Expired(detail) => {
                    LeasePayload::Expired(detail.sanitize(fingerprinter)?)
                }
                LeaseDraftKind::TransitionIntent(detail) => {
                    LeasePayload::TransitionIntent(detail.sanitize(fingerprinter)?)
                }
                LeaseDraftKind::TransitionFailed(detail) => {
                    LeasePayload::TransitionFailed(detail.sanitize(fingerprinter)?)
                }
            }),
            Self::Task(value) => EventPayload::Task(match value.0 {
                TaskDraftKind::Requested(detail) => {
                    TaskPayload::Requested(detail.sanitize(fingerprinter)?)
                }
                TaskDraftKind::Started(detail) => {
                    TaskPayload::Started(detail.sanitize(fingerprinter)?)
                }
                TaskDraftKind::StepStarted(detail) => {
                    TaskPayload::StepStarted(detail.sanitize(fingerprinter)?)
                }
                TaskDraftKind::StepFinished(detail) => {
                    TaskPayload::StepFinished(detail.sanitize(fingerprinter)?)
                }
                TaskDraftKind::Completed(detail) => {
                    TaskPayload::Completed(detail.sanitize(fingerprinter)?)
                }
                TaskDraftKind::Failed(detail) => {
                    TaskPayload::Failed(detail.sanitize(fingerprinter)?)
                }
                TaskDraftKind::Cancelled(detail) => {
                    TaskPayload::Cancelled(detail.sanitize(fingerprinter)?)
                }
                TaskDraftKind::TerminalIntent(detail) => {
                    TaskPayload::TerminalIntent(detail.sanitize(fingerprinter)?)
                }
                TaskDraftKind::TerminalCommitFailed(detail) => {
                    TaskPayload::TerminalCommitFailed(detail.sanitize(fingerprinter)?)
                }
                TaskDraftKind::Semantic(detail) => {
                    TaskPayload::Semantic(detail.sanitize(fingerprinter)?)
                }
            }),
            Self::Application(value) => EventPayload::Application(match value.0 {
                ApplicationDraftKind::Intent(detail) => {
                    ApplicationPayload::Intent(detail.sanitize(fingerprinter)?)
                }
                ApplicationDraftKind::Completed(detail) => {
                    ApplicationPayload::Completed(detail.sanitize(fingerprinter)?)
                }
                ApplicationDraftKind::Failed(detail) => {
                    ApplicationPayload::Failed(detail.sanitize(fingerprinter)?)
                }
            }),
            Self::Input(value) => EventPayload::Input(match value.0 {
                InputDraftKind::Intent(detail) => {
                    InputPayload::Intent(detail.sanitize(fingerprinter)?)
                }
                InputDraftKind::Committed(detail) => {
                    InputPayload::Committed(detail.sanitize(fingerprinter)?)
                }
                InputDraftKind::Completed(detail) => {
                    InputPayload::Completed(detail.sanitize(fingerprinter)?)
                }
                InputDraftKind::Failed(detail) => {
                    InputPayload::Failed(detail.sanitize(fingerprinter)?)
                }
            }),
            Self::Capture(value) => EventPayload::Capture(match value.0 {
                CaptureDraftKind::Requested(detail) => {
                    CapturePayload::Requested(detail.sanitize(fingerprinter)?)
                }
                CaptureDraftKind::Completed(detail) => {
                    CapturePayload::Completed(detail.sanitize(fingerprinter)?)
                }
                CaptureDraftKind::Failed(detail) => {
                    CapturePayload::Failed(detail.sanitize(fingerprinter)?)
                }
                CaptureDraftKind::PressureChanged(detail) => {
                    CapturePayload::PressureChanged(detail.sanitize(fingerprinter)?)
                }
                CaptureDraftKind::DedupWindow(detail) => {
                    CapturePayload::DedupWindow(detail.sanitize(fingerprinter)?)
                }
                CaptureDraftKind::PolicyChanged(detail) => {
                    CapturePayload::PolicyChanged(detail.sanitize(fingerprinter)?)
                }
                CaptureDraftKind::SummaryCommitted(detail) => {
                    CapturePayload::SummaryCommitted(detail.sanitize(fingerprinter)?)
                }
            }),
            Self::Recognition(value) => EventPayload::Recognition(match value.0 {
                RecognitionDraftKind::Requested(detail) => {
                    RecognitionPayload::Requested(detail.sanitize(fingerprinter)?)
                }
                RecognitionDraftKind::Completed(detail) => {
                    RecognitionPayload::Completed(detail.sanitize(fingerprinter)?)
                }
                RecognitionDraftKind::Failed(detail) => {
                    RecognitionPayload::Failed(detail.sanitize(fingerprinter)?)
                }
            }),
            Self::Artifact(value) => EventPayload::Artifact(match value.0 {
                ArtifactDraftKind::Retention(record, audit) => ArtifactPayload::Retention(
                    ArtifactRetentionPayload::sanitize(record, audit, fingerprinter)?,
                ),
                ArtifactDraftKind::Created(detail) => {
                    ArtifactPayload::Created(detail.sanitize(fingerprinter)?)
                }
                ArtifactDraftKind::Verified(detail) => {
                    ArtifactPayload::Verified(detail.sanitize(fingerprinter)?)
                }
                ArtifactDraftKind::StoreFailed(detail) => {
                    ArtifactPayload::StoreFailed(detail.sanitize(fingerprinter)?)
                }
                ArtifactDraftKind::VerificationFailed(detail) => {
                    ArtifactPayload::VerificationFailed(detail.sanitize(fingerprinter)?)
                }
                ArtifactDraftKind::ExportCompleted(detail) => {
                    ArtifactPayload::ExportCompleted(detail.sanitize(fingerprinter)?)
                }
                ArtifactDraftKind::ExportFailed(detail) => {
                    ArtifactPayload::ExportFailed(detail.sanitize(fingerprinter)?)
                }
            }),
            Self::ResourceAuthoring(value) => {
                EventPayload::ResourceAuthoring(value.0.sanitize(fingerprinter)?)
            }
            Self::Client(value) => EventPayload::Client(match value.0 {
                ClientDraftKind::Action(detail) => {
                    ClientPayload::Action(detail.sanitize(fingerprinter)?)
                }
                ClientDraftKind::UiAction(detail) => {
                    ClientPayload::UiAction(detail.sanitize(fingerprinter)?)
                }
                ClientDraftKind::CliCommand(detail) => {
                    ClientPayload::CliCommand(detail.sanitize(fingerprinter)?)
                }
                ClientDraftKind::LabRequest(detail) => {
                    ClientPayload::LabRequest(detail.sanitize(fingerprinter)?)
                }
                ClientDraftKind::LabPinRelease(target, audit) => ClientPayload::LabPinRelease(
                    LabPinReleaseRequestPayload::sanitize(target, audit, fingerprinter)?,
                ),
                ClientDraftKind::OwnerUnlock(draft) => {
                    ClientPayload::OwnerUnlock(draft.sanitize(fingerprinter)?)
                }
                ClientDraftKind::GovernanceIdentityDeclared(draft) => {
                    ClientPayload::GovernanceIdentityDeclared(draft.sanitize(fingerprinter)?)
                }
            }),
            Self::Ledger(value) => EventPayload::Ledger(match value.0 {
                LedgerDraftKind::Recovered(detail) => {
                    LedgerPayload::Recovered(detail.sanitize(fingerprinter)?)
                }
                LedgerDraftKind::Signature(record, audit) => {
                    record.validate()?;
                    LedgerPayload::Signature(LedgerSignaturePayload {
                        record,
                        audit: audit.sanitize(fingerprinter)?,
                    })
                }
                LedgerDraftKind::DumpImported(outcome, audit) => LedgerPayload::DumpImported(
                    LedgerDumpImportedPayload::new(outcome, audit.sanitize(fingerprinter)?)?,
                ),
                LedgerDraftKind::DumpGap {
                    gap_kind,
                    gap_start_unix_ms,
                    gap_end_unix_ms,
                    outcome,
                    audit,
                } => LedgerPayload::GapRecorded(LedgerGapPayload::dump(
                    gap_kind,
                    (gap_start_unix_ms, gap_end_unix_ms),
                    outcome,
                    audit.sanitize(fingerprinter)?,
                )?),
                LedgerDraftKind::RejectedGap {
                    captured_at_unix_ms,
                    event_id,
                    source,
                    outcome,
                    audit,
                } => LedgerPayload::GapRecorded(LedgerGapPayload::rejected(
                    captured_at_unix_ms,
                    event_id,
                    source,
                    outcome,
                    audit.sanitize(fingerprinter)?,
                )?),
                LedgerDraftKind::Health {
                    outcome,
                    segment_edge,
                    audit,
                } => LedgerPayload::HealthObserved(LedgerHealthPayload::new(
                    outcome,
                    segment_edge,
                    audit.sanitize(fingerprinter)?,
                )?),
            }),
        })
    }
}

impl EventPayload {
    pub fn device_diagnostics(&self) -> Option<&DeviceDiagnosticBudgetRecord> {
        match self {
            Self::Runtime(RuntimePayload::LifecycleObserved(value)) => value.device_diagnostics(),
            _ => None,
        }
    }

    pub fn performance_summary(&self) -> Option<&PerformanceSummaryPayload> {
        performance_summary(self)
    }

    pub fn performance_stutter(&self) -> Option<&PerformanceStutterPayload> {
        performance_stutter(self)
    }

    pub fn performance_clock_jump(&self) -> Option<&PerformanceControlPayload> {
        performance_control(self)
            .filter(|value| value.reason() == PerformanceControlReason::ClockJump)
    }

    pub fn event_type(&self) -> EventType {
        self.family_payload().event_type()
    }

    pub fn family(&self) -> EventFamily {
        self.event_type().family()
    }

    pub fn schema(&self) -> &'static str {
        match self {
            Self::Provider(_) => PROVIDER_PAYLOAD_SCHEMA,
            Self::Runtime(_) => RUNTIME_PAYLOAD_SCHEMA,
            Self::Monitor(_) => MONITOR_PAYLOAD_SCHEMA,
            Self::Performance(_) => PERFORMANCE_PAYLOAD_SCHEMA,
            Self::Fact(_) => FACT_PAYLOAD_SCHEMA,
            Self::Approval(_) => APPROVAL_PAYLOAD_SCHEMA,
            Self::Command(_) => COMMAND_PAYLOAD_SCHEMA,
            Self::Scheduler(_) => SCHEDULER_PAYLOAD_SCHEMA,
            Self::Policy(_) => POLICY_PAYLOAD_SCHEMA,
            Self::Catalog(_) => CATALOG_PAYLOAD_SCHEMA,
            Self::State(_) => STATE_PAYLOAD_SCHEMA,
            Self::Release(_) => RELEASE_PAYLOAD_SCHEMA,
            Self::Agent(_) => AGENT_PAYLOAD_SCHEMA,
            Self::Lease(_) => LEASE_PAYLOAD_SCHEMA,
            Self::Task(_) => TASK_PAYLOAD_SCHEMA,
            Self::Application(_) => APPLICATION_PAYLOAD_SCHEMA,
            Self::Input(_) => INPUT_PAYLOAD_SCHEMA,
            Self::Capture(_) => CAPTURE_PAYLOAD_SCHEMA,
            Self::Recognition(_) => RECOGNITION_PAYLOAD_SCHEMA,
            Self::Artifact(ArtifactPayload::Retention(_)) => ARTIFACT_RETENTION_PAYLOAD_SCHEMA,
            Self::Artifact(_) => ARTIFACT_PAYLOAD_SCHEMA,
            Self::ResourceAuthoring(_) => RESOURCE_AUTHORING_PAYLOAD_SCHEMA,
            Self::Client(_) => CLIENT_PAYLOAD_SCHEMA,
            Self::Ledger(_) => LEDGER_PAYLOAD_SCHEMA,
        }
    }

    pub fn sensitivity(&self) -> Sensitivity {
        let detail = self.family_payload().detail();
        let mut sensitivity = detail.audit().sensitivity();
        if matches!(self, Self::Runtime(RuntimePayload::LifecycleObserved(value)) if value.prior_epoch_close.is_some())
        {
            sensitivity = sensitivity.max(Sensitivity::Internal);
        }
        if let Self::Artifact(ArtifactPayload::Retention(value)) = self {
            sensitivity = sensitivity.max(value.sensitivity());
        }
        if matches!(self, Self::Runtime(RuntimePayload::LifecycleObserved(value)) if value.adb_recovery.is_some() || value.vendor_stdio.is_some() || value.backend_open.is_some())
        {
            sensitivity = sensitivity.max(Sensitivity::Sensitive);
        }
        // Workflow #383 register X13: foreign text on the ADB evidence's outcome links.
        if let Self::Runtime(RuntimePayload::LifecycleObserved(value)) = self
            && let Some(foreign) = value
                .adb_recovery()
                .and_then(AdbTargetRecovery::foreign_text_sensitivity)
        {
            sensitivity = sensitivity.max(foreign);
        }
        if self.artifact_failure().is_some() {
            sensitivity = sensitivity.max(Sensitivity::Sensitive);
        }
        if self
            .performance_summary()
            .and_then(PerformanceSummaryPayload::capacity)
            .is_some()
        {
            sensitivity = sensitivity.max(Sensitivity::Sensitive);
        }
        if matches!(self, Self::Provider(_)) {
            sensitivity = sensitivity.max(Sensitivity::Sensitive);
        }
        if matches!(
            self,
            Self::Task(TaskPayload::Semantic(payload))
                if matches!(payload.fact(), TaskSemanticFact::GeometryObserved { .. })
        ) {
            sensitivity = sensitivity.max(Sensitivity::Sensitive);
        }
        if let Some(budget) = self.device_diagnostics() {
            sensitivity = sensitivity.max(budget.declared_sensitivity);
        }
        if let Some(lifecycle) = detail.lifecycle_failure() {
            sensitivity = sensitivity.max(lifecycle.sensitivity());
        }
        if detail.diagnostic_code().is_some()
            || detail.diagnostic_detail().is_some()
            || detail.cleanup_detail().is_some()
        {
            sensitivity = sensitivity.max(Sensitivity::Internal);
        }
        // Foreign text rides on the outcome's links with its declared sensitivity.
        if let Some(foreign) = self.outcome().and_then(Outcome::foreign_text_sensitivity) {
            sensitivity = sensitivity.max(foreign);
        }
        if matches!(
            self,
            Self::Performance(_)
                | Self::Runtime(
                    RuntimePayload::LifecycleObserved(_)
                        | RuntimePayload::InstanceBound(_)
                        | RuntimePayload::FactRecorded(_)
                        | RuntimePayload::FactInvalidated(_)
                        | RuntimePayload::FactSnapshot(_)
                        | RuntimePayload::DeviceSelfCheck(_)
                )
                | Self::Ledger(LedgerPayload::Signature(_))
        ) {
            sensitivity = sensitivity.max(Sensitivity::Internal);
        }
        if matches!(self, Self::Capture(CapturePayload::SummaryCommitted(_))) {
            sensitivity = sensitivity.max(Sensitivity::Internal);
        }
        if let Self::Input(InputPayload::Intent(intent)) = self
            && let Some(execution_plan) = intent.execution_plan()
        {
            sensitivity = sensitivity.max(execution_plan.declared_sensitivity());
        }
        if let Self::Fact(payload) = self {
            sensitivity = sensitivity.max(fact_sensitivity(payload));
        }
        if matches!(
            self,
            Self::Approval(_)
                | Self::Client(
                    ClientPayload::Action(_)
                        | ClientPayload::OwnerUnlock(_)
                        | ClientPayload::GovernanceIdentityDeclared(_)
                )
        ) {
            sensitivity = sensitivity.max(Sensitivity::Internal);
        }
        if matches!(self, Self::State(_) | Self::Release(_)) {
            sensitivity = sensitivity.max(Sensitivity::Internal);
        }
        if matches!(self, Self::Agent(_)) {
            sensitivity = sensitivity.max(Sensitivity::Internal);
        }
        if self.runtime_state().is_some() {
            sensitivity = sensitivity.max(Sensitivity::Internal);
        }
        sensitivity
    }

    pub fn action(&self) -> EventAction {
        self.family_payload().detail().action()
    }

    /// The outcome this record carries (ledger revision 3): the failure, refusal or result it
    /// reports. A record without one is a state change; its severity is its event type's
    /// category.
    pub fn outcome(&self) -> Option<&Outcome> {
        match self {
            Self::Runtime(RuntimePayload::LifecycleObserved(value)) => value.outcome(),
            Self::Task(TaskPayload::Semantic(value)) => value.fact().outcome(),
            Self::Performance(
                PerformancePayload::MonitorDegraded(value)
                | PerformancePayload::MonitorRecovered(value),
            ) => value.outcome(),
            Self::ResourceAuthoring(value) => value.outcome(),
            Self::Policy(
                PolicyPayload::DispatchIntent(value)
                | PolicyPayload::DispatchAdmitted(value)
                | PolicyPayload::DispatchRejected(value)
                | PolicyPayload::DispatchCompleted(value),
            ) => value.rejection().map(|rejection| &rejection.outcome),
            _ => self.family_payload().detail().outcome(),
        }
    }

    pub fn effect_disposition(&self) -> Option<EffectDisposition> {
        self.family_payload().detail().effect_disposition()
    }

    pub fn diagnostic_code(&self) -> Option<DiagnosticCode> {
        self.family_payload().detail().diagnostic_code()
    }

    /// Workflow #389 MCP-2 (`EventQuery.fact_key`): whether this record names the fact `key`.
    /// The fact records name one: a published instance fact (its record or one of its related
    /// records), an invalidated instance fact, a recorded and an invalidated runtime fact. No
    /// other record does.
    pub fn names_fact_key(&self, key: &str) -> bool {
        match self {
            Self::Fact(FactPayload::Published(value)) => {
                value.records().any(|record| record.key == key)
            }
            Self::Fact(FactPayload::Invalidated(value)) => value.invalidation.key == key,
            Self::Runtime(RuntimePayload::FactRecorded(value)) => value.record.key == key,
            Self::Runtime(RuntimePayload::FactInvalidated(value)) => value.invalidation.key == key,
            _ => false,
        }
    }

    pub fn runtime_state(&self) -> Option<&crate::RuntimeStateFact> {
        self.family_payload().detail().runtime_state()
    }

    pub fn artifact_failure(&self) -> Option<&ArtifactFailureRecord> {
        self.family_payload().detail().artifact_failure()
    }

    pub fn task_timing(&self) -> Option<&crate::TaskTimingObservations> {
        match self {
            Self::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
                TaskSemanticFact::TerminalCommitted { task_timing, .. } => task_timing.as_deref(),
                _ => None,
            },
            _ => self
                .family_payload()
                .detail()
                .lifecycle_failure()?
                .task_timing(),
        }
    }

    pub fn resource_declaration(&self) -> Option<&crate::ResourceDeclarationRejection> {
        self.family_payload().detail().resource_declaration()
    }

    pub fn validate(&self) -> Result<(), SanitizationError> {
        let detail = self.family_payload().detail();
        if let Some(retention) = self.artifact_retention() {
            retention.validate()?;
        }
        if let Some(rejection) = self.resource_declaration() {
            if self.event_type() != EventType::RuntimeFailed
                || detail.effect_disposition() != Some(EffectDisposition::NotPerformed)
            {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidResourceDeclarationOwner,
                    "resource_declaration",
                ));
            }
            rejection
                .validate()
                .map_err(|code| SanitizationError::new(code, "resource_declaration"))?;
        }
        if let Some(failure) = self.artifact_failure() {
            if !matches!(
                self.event_type(),
                EventType::ArtifactStoreFailed | EventType::ArtifactVerificationFailed
            ) {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidArtifactFailureOwner,
                    "artifact_failure",
                ));
            }
            if (failure.stage == ArtifactFailureStage::PublishedVerification)
                != (self.event_type() == EventType::ArtifactVerificationFailed)
            {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidArtifactFailureStage,
                    "artifact_failure",
                ));
            }
            failure.validate()?;
        }
        if let Some(state) = detail.runtime_state() {
            state.validate()?;
            let expected_action = match state {
                crate::RuntimeStateFact::Observed { .. }
                | crate::RuntimeStateFact::MonitorImported { .. } => EventAction::RuntimeAction,
                crate::RuntimeStateFact::MonitorChanged { change, .. } => match change.kind {
                    crate::RuntimeMonitorChangeKind::Configure => EventAction::MonitorConfigure,
                    crate::RuntimeMonitorChangeKind::Clear => EventAction::MonitorClear,
                    crate::RuntimeMonitorChangeKind::Probe => EventAction::MonitorProbe,
                },
            };
            let correct_event = if expected_action == EventAction::MonitorProbe {
                matches!(
                    self,
                    Self::Monitor(MonitorPayload::Completed(_) | MonitorPayload::Failed(_))
                )
            } else {
                matches!(self, Self::Command(CommandPayload::Validated(_)))
            };
            if self.action() != expected_action || !correct_event {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidRuntimeStateOwner,
                    "runtime_state",
                ));
            }
        }
        if let Self::Ledger(LedgerPayload::Signature(payload)) = self {
            payload.record().validate()?;
        }
        match self {
            Self::Ledger(LedgerPayload::GapRecorded(payload)) => payload.validate()?,
            Self::Ledger(LedgerPayload::HealthObserved(payload)) => payload.validate()?,
            Self::Ledger(LedgerPayload::DumpImported(payload)) => payload.validate()?,
            _ => {}
        }
        // Workflow #389 (register U1): only a command record names its channel, and `via`
        // only on the Lab channel.
        let (channel, via) = detail.command_channel();
        if ((channel.is_some() || via.is_some()) && self.family() != EventFamily::Command)
            || (via.is_some() && channel != Some(crate::RuntimeChannel::Lab))
        {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidSanitizedPayload,
                "channel",
            ));
        }
        if let Self::Runtime(RuntimePayload::DeviceSelfCheck(payload)) = self {
            payload.validate()?;
        }
        if detail.touch_response_us().is_some()
            && !matches!(self, Self::Input(InputPayload::Committed(_)))
        {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidTouchResponseScope,
                "touch_response_us",
            ));
        }
        if detail.capture_acquire_us().is_some()
            && !matches!(self, Self::Capture(CapturePayload::Completed(_)))
        {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidCaptureAcquireScope,
                "capture_acquire_us",
            ));
        }
        if detail.device_diagnostic_config().is_some()
            && !matches!(
                self.event_type(),
                EventType::RuntimeStarted | EventType::RuntimeTakeover
            )
        {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidDeviceDiagnosticConfigOwner,
                "runtime_payload",
            ));
        }
        if let Self::Runtime(RuntimePayload::LifecycleObserved(value)) = self {
            if let RuntimeLifecyclePhase::ResourceQuiescence {
                resource_count,
                quiescence,
                resource_dispositions: Some(groups),
                ..
            } = &value.phase
                && validate_resource_dispositions(groups)? != (*resource_count, *quiescence)
            {
                return Err(SanitizationError::new(
                    SanitizationCode::ResourceDispositionSummaryMismatch,
                    "runtime_payload",
                ));
            }
            if (value.phase == RuntimeLifecyclePhase::BackendOpenObserved)
                != value.backend_open.is_some()
            {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidBackendOpenPhase,
                    "runtime_payload",
                ));
            }
            if let Some(report) = &value.backend_open {
                report.validate()?;
            }
            let expected = match value.prior_epoch_close.as_deref() {
                Some(crate::PriorEpochCloseFact::OwnerImported(_)) => {
                    Some(RuntimeLifecyclePhase::PriorEpochOwnerImported)
                }
                Some(crate::PriorEpochCloseFact::ScopeClosed(_)) => {
                    Some(RuntimeLifecyclePhase::PriorEpochScopeClosed)
                }
                None => None,
            };
            if expected.as_ref().is_some_and(|phase| *phase != value.phase)
                || expected.is_none()
                    && matches!(
                        value.phase,
                        RuntimeLifecyclePhase::PriorEpochOwnerImported
                            | RuntimeLifecyclePhase::PriorEpochScopeClosed
                    )
            {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidPriorEpochClosePhase,
                    "runtime_payload",
                ));
            }
            if let Some(fact) = &value.prior_epoch_close {
                fact.validate(value.owner_epoch)?;
            }
            if matches!(value.phase, RuntimeLifecyclePhase::VendorStdioClose { .. })
                != value.vendor_stdio.is_some()
            {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidVendorStdioPhase,
                    "runtime_payload",
                ));
            }
            if let Some(facts) = &value.vendor_stdio {
                facts.validate()?;
            }
            if (value.phase == RuntimeLifecyclePhase::AdbTargetRecovery)
                != value.adb_recovery.is_some()
            {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidAdbRecoveryPhase,
                    "runtime_payload",
                ));
            }
            if let Some(recovery) = &value.adb_recovery {
                recovery.validate()?;
            }
            validate_recovery_ladder_phase(&value.phase)?;
            validate_recovery_drill_tag(&value.phase, value.drill)?;
            if let RuntimeLifecyclePhase::TaggedStateLoaded { stores, .. } = &value.phase {
                observer::validate_tagged_state_stores(stores)?;
            }
            if let RuntimeLifecyclePhase::TaggedStateLoaded { outcome, .. }
            | RuntimeLifecyclePhase::TaggedStateEnabled { outcome } = &value.phase
            {
                observer::validate_tagged_state_outcome(outcome.as_ref())?;
            }
            if let RuntimeLifecyclePhase::FactSnapshotSetSkipped {
                parts_found, parts, ..
            } = value.phase
                && (parts_found == 0 || parts_found >= parts)
            {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidFactSnapshotSetSkipped,
                    "runtime_payload",
                ));
            }
            if let RuntimeLifecyclePhase::ShutdownRequest { target, decision } = value.phase
                && (target.validate().is_err()
                    || (decision == crate::RuntimeShutdownDecision::Accepted
                        && target.owner_epoch != value.owner_epoch))
            {
                return Err(SanitizationError::new(
                    RuntimeContractCode::InvalidShutdownTarget,
                    "runtime_payload",
                ));
            }
            let diagnostic = matches!(
                value.phase,
                RuntimeLifecyclePhase::DeviceDiagnosticDetail
                    | RuntimeLifecyclePhase::DeviceDiagnosticSummary
            );
            if diagnostic != value.device_diagnostics.is_some() {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidDeviceDiagnosticPhase,
                    "runtime_payload",
                ));
            }
            if let Some(budget) = &value.device_diagnostics {
                if budget.owner_epoch != value.owner_epoch {
                    return Err(SanitizationError::new(
                        SanitizationCode::DeviceDiagnosticOwnerMismatch,
                        "runtime_payload",
                    ));
                }
                budget.validate(value.phase == RuntimeLifecyclePhase::DeviceDiagnosticSummary)?;
            }
        }
        if let Some(lifecycle) = detail.lifecycle_failure() {
            if self.event_type() != EventType::RuntimeFailed {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidLifecycleFailureOwner,
                    "lifecycle_failure",
                ));
            }
            lifecycle.validate()?;
        }
        if let Self::Provider(value) = self {
            value.record.validate()?;
        }
        detail.audit().validate()?;
        if let Some(diagnostic_detail) = detail.diagnostic_detail() {
            diagnostic_detail.validate()?;
        }
        if let Some(cleanup_detail) = detail.cleanup_detail() {
            if !matches!(
                self.event_type(),
                EventType::InputFailed | EventType::CaptureFailed
            ) {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidCleanupCauseOwner,
                    "cleanup_detail",
                ));
            }
            cleanup_detail.validate()?;
        }
        if let Self::Runtime(RuntimePayload::LifecycleObserved(value)) = self
            && value.action != EventAction::RuntimeAction
        {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidRuntimeLifecycleAction,
                "runtime_payload",
            ));
        }
        if let Self::Runtime(RuntimePayload::InstanceBound(value)) = self {
            validate_runtime_instance_binding(value)?;
        }
        if let Self::Runtime(RuntimePayload::FactRecorded(value)) = self {
            validate_runtime_fact_recorded(value)?;
        }
        if let Self::Runtime(RuntimePayload::FactInvalidated(value)) = self {
            validate_runtime_fact_invalidated(value)?;
        }
        if let Self::Runtime(RuntimePayload::FactSnapshot(value)) = self {
            validate_runtime_fact_snapshot(value)?;
        }
        if let Self::ResourceAuthoring(value) = self {
            validate_resource_authoring_fields(
                value.phase,
                &value.draft_id,
                &value.target_label,
                &value.target_fingerprint,
                &value.changed_paths,
                value.outcome.as_ref(),
            )?;
        }
        if let Self::Policy(value) = self {
            validate_policy_payload(value)?;
        }
        if let Self::Performance(value) = self {
            validate_performance_payload(value)?;
        }
        if let Self::Fact(value) = self {
            validate_fact_payload(value)?;
        }
        if let Self::Approval(value) = self {
            validate_approval_payload(value)?;
        }
        if let Self::Client(value) = self {
            validate_client_action_payload(value)?;
        }
        if let Self::Catalog(value) = self {
            validate_catalog_payload(value)?;
        }
        if let Self::State(StatePayload::Migrated(value)) = self {
            if value.action != EventAction::StateMigrate {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidStateMigrationLifecycle,
                    "state_payload",
                ));
            }
            value.migration.validate()?;
        }
        if let Self::Release(value) = self {
            validate_release_payload(value)?;
        }
        if let Self::Agent(value) = self {
            validate_agent_payload(value)?;
        }
        if let Self::Ledger(LedgerPayload::Recovered(recovery)) = self {
            if (recovery.reason == RecoveryReason::StorageCutover) != recovery.migration.is_some() {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidLedgerMigration,
                    "ledger_migration",
                ));
            }
            if let Some(record) = &recovery.migration {
                record.validate()?;
                if recovery.segment_index.is_some() || recovery.affected_bytes != 0 {
                    return Err(SanitizationError::new(
                        SanitizationCode::InvalidLedgerMigration,
                        "ledger_migration",
                    ));
                }
            }
        }
        if let Self::Input(InputPayload::Intent(intent)) = self {
            intent.validate()?;
        }
        // Register X8 (Workflow #383 SH-2): a stored geometry observation whose phase does not
        // fit its frame, trigger and original failure is refused with the phase rule's own code,
        // as the writer refuses it; any other refusal of a task semantic fact stays the generic one.
        if let Self::Task(TaskPayload::Semantic(value)) = self
            && let Err(error) = value.validate()
        {
            if error.code() == SanitizationCode::InvalidTaskGeometryPhase.code() {
                return Err(error);
            }
            return Err(SanitizationError::new(
                SanitizationCode::InvalidTaskSemanticPayload,
                "task_semantic_fact",
            ));
        }
        match self {
            Self::Ledger(LedgerPayload::Recovered(recovery))
                if recovery.segment_index == Some(0) =>
            {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidSanitizedPayload,
                    "segment_index",
                ));
            }
            Self::Capture(CapturePayload::Completed(result))
            | Self::Recognition(RecognitionPayload::Completed(result))
                if result.frame_width == 0 || result.frame_height == 0 =>
            {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidSanitizedPayload,
                    "frame_dimensions",
                ));
            }
            Self::Monitor(MonitorPayload::Completed(value))
                if value.observation.validate().is_err()
                    || value.decision.validate().is_err()
                    || value.observation.diagnosis() != value.decision.diagnosis() =>
            {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidMonitorOutcome,
                    "diagnosis",
                ));
            }
            Self::Monitor(MonitorPayload::RecoveryAdmitted(value))
                if value.action != EventAction::MonitorRecovery
                    || value.effect_disposition != EffectDisposition::NotPerformed
                    || value.reason != MonitorRecoveryCoordinationReason::SchedulerAvailable =>
            {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidMonitorRecoveryCoordination,
                    "reason",
                ));
            }
            Self::Monitor(MonitorPayload::RecoveryDeferred(value))
                if value.action != EventAction::MonitorRecovery
                    || value.effect_disposition != EffectDisposition::NotPerformed
                    || value.reason == MonitorRecoveryCoordinationReason::SchedulerAvailable =>
            {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidMonitorRecoveryCoordination,
                    "reason",
                ));
            }
            Self::Capture(CapturePayload::PressureChanged(value))
                if value.memory_budget_bytes == 0 =>
            {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidCapturePressure,
                    "memory_budget_bytes",
                ));
            }
            Self::Capture(CapturePayload::DedupWindow(value))
                if value.duplicate_count == 0
                    || value.duration_ms == 0
                    || value.preserved_frame_id.is_some() && value.duplicate_count != 1 =>
            {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidCaptureDedupWindow,
                    "duplicate_count",
                ));
            }
            Self::Capture(CapturePayload::PolicyChanged(value)) if value.cadence_ms == 0 => {
                return Err(SanitizationError::new(
                    SharedCode::InvalidCapturePolicy,
                    "cadence_ms",
                ));
            }
            Self::Capture(CapturePayload::SummaryCommitted(value))
                if value.action != EventAction::CaptureSummaryCommit
                    || value.summary.validate().is_err() =>
            {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidCaptureSummary,
                    "summary",
                ));
            }
            Self::Artifact(ArtifactPayload::ExportCompleted(value))
                if value.artifact_count == 0 =>
            {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidArtifactExport,
                    "artifact_count",
                ));
            }
            _ => {}
        }
        Ok(())
    }

    /// Workflow #191 B1-S1: bounds that measure the current encoding — a `runtime.fact_snapshot`
    /// part and a `fact.published` observation, each at most 512 KiB, and a
    /// `task.selection_evaluated` record, at most 64 KiB (Workflow #308). Write side only:
    /// `EventDraft::sanitize` calls it right after `validate`. A decoded record is never
    /// re-serialized for a bound, so an additive field cannot invalidate a stored record.
    pub(crate) fn validate_write_bounds(&self) -> Result<(), SanitizationError> {
        match self {
            Self::Runtime(RuntimePayload::FactSnapshot(value)) => {
                value.snapshot.validate_for_append()
            }
            Self::Fact(FactPayload::Published(value)) => crate::FactObservation {
                records: value.records().cloned().collect(),
            }
            .validate(),
            Self::Task(TaskPayload::Semantic(value)) => match value.fact() {
                TaskSemanticFact::SelectionEvaluated { selection, .. } => {
                    selection.validate_for_append()
                }
                _ => Ok(()),
            },
            _ => Ok(()),
        }
    }

    pub fn public_projection(&self) -> PublicEventPayload {
        let event_type = self.event_type();
        let detail = self.family_payload().detail();
        let authoring = resource_authoring(self);
        let policy_dispatch = policy_dispatch(self);
        let policy_execution = policy_execution(self);
        let policy_signal = policy_planning_signal(self);
        let catalog_transition = catalog_transition(self);
        let performance_pressure = performance_pressure(self);
        let performance_stutter = performance_stutter(self);
        let performance_summary = performance_summary(self);
        let performance_state = performance_monitor_state(self);
        let performance_control = performance_control(self);
        let fact_identity = fact_identity(self);
        let client_action = client_action(self);
        let approval = approval_decision(self);
        let agent_wake = agent_wake(self);
        let agent_session = agent_session(self);
        let payload = PublicPayload {
            backend_open: match self {
                Self::Runtime(RuntimePayload::LifecycleObserved(value)) => value
                    .backend_open()
                    .map(|report| Box::new(report.public_summary())),
                _ => None,
            },
            prior_epoch_close: match self {
                Self::Runtime(RuntimePayload::LifecycleObserved(value)) => value
                    .prior_epoch_close()
                    .map(|fact| Box::new(fact.public_summary(value.owner_epoch()))),
                _ => None,
            },
            event_type,
            action: detail.action(),
            device_diagnostic_config: detail.device_diagnostic_config(),
            device_diagnostics: self
                .device_diagnostics()
                .map(|record| Box::new(record.public_summary())),
            effect_disposition: detail.effect_disposition(),
            lifecycle_failure: detail
                .lifecycle_failure()
                .map(|failure| Box::new(failure.public_summary())),
            outcome: self.outcome().map(|outcome| Box::new(outcome.codes_only())),
            segment_index: match self {
                Self::Ledger(LedgerPayload::Recovered(value)) => value.segment_index,
                _ => None,
            },
            affected_bytes: match self {
                Self::Ledger(LedgerPayload::Recovered(value)) => Some(value.affected_bytes),
                _ => None,
            },
            frame_width: observation_result(self).map(ObservationResultPayload::frame_width),
            frame_height: observation_result(self).map(ObservationResultPayload::frame_height),
            recognition_verdict: observation_result(self)
                .and_then(ObservationResultPayload::recognition_verdict),
            capture_pressure_state: capture_pressure(self).map(CapturePressurePayload::state),
            memory_budget_bytes: capture_pressure(self)
                .map(CapturePressurePayload::memory_budget_bytes),
            resident_bytes: capture_pressure(self).map(CapturePressurePayload::resident_bytes),
            duplicate_count: capture_dedup(self).map(CaptureDedupWindowPayload::duplicate_count),
            duration_ms: capture_dedup(self).map(CaptureDedupWindowPayload::duration_ms),
            preserved_frame_id: capture_dedup(self)
                .and_then(|value| value.preserved_frame_id)
                .map(Box::new),
            artifact_retention: match self {
                Self::Artifact(ArtifactPayload::Retention(value)) => {
                    Some(Box::new(value.public_summary()))
                }
                _ => None,
            },
            lab_pin_release_request: self.lab_pin_release_request().copied(),
            cadence_ms: capture_policy(self).map(CapturePolicyPayload::cadence_ms),
            retention_class: capture_policy(self).map(CapturePolicyPayload::retention_class),
            capture_policy_reason: capture_policy(self).map(CapturePolicyPayload::reason),
            task_outcome: artifact_export(self).map(|value| value.0),
            task_semantic_fact: task_semantic_fact(self)
                .map(|fact| Box::new(fact.public_summary())),
            evidence_completeness: artifact_export(self).map(|value| value.1),
            artifact_count: artifact_export(self).map(|value| value.2),
            monitor_diagnosis: monitor_outcome(self).map(|value| value.observation.diagnosis()),
            monitor_disposition: monitor_outcome(self).map(|value| value.decision.disposition()),
            monitor_recovery: monitor_recovery(self),
            monitor_recovery_coordination_reason: monitor_recovery_coordination(self)
                .map(MonitorRecoveryCoordinationPayload::reason),
            authoring_phase: authoring.map(ResourceAuthoringPayload::phase),
            draft_id: authoring.map(|value| value.draft_id.clone()),
            target_label: authoring.map(|value| value.target_label.clone()),
            target_fingerprint: authoring.map(|value| value.target_fingerprint.clone()),
            changed_path_count: authoring.map(|value| value.changed_paths.len() as u64),
            decision_id: policy_dispatch.map(|value| value.decision_id.clone().into_boxed_str()),
            reason_chain_id: policy_dispatch
                .map(|value| value.reason_chain_id.clone().into_boxed_str()),
            reason_count: policy_dispatch.map(|value| value.reasons.len() as u64),
            input_ledger_position: policy_dispatch
                .map(|value| value.input_ledger_position)
                .filter(|position| *position != 0),
            fact_snapshot_id: policy_dispatch
                .map(|value| value.fact_snapshot_id.clone().into_boxed_str()),
            approval_fact_count: policy_dispatch.map(|value| value.approval_fact_ids.len() as u64),
            catalog_id: catalog_transition.map(|value| value.catalog_id.clone().into_boxed_str()),
            catalog_hash: policy_dispatch
                .map(|value| value.catalog_hash.clone().into_boxed_str())
                .or_else(|| {
                    catalog_transition.map(|value| value.catalog_hash.clone().into_boxed_str())
                }),
            catalog_version: policy_dispatch
                .map(|value| value.catalog_version)
                .or_else(|| catalog_transition.map(|value| value.catalog_version)),
            previous_catalog_hash: catalog_transition.and_then(|value| {
                value
                    .previous_catalog_hash
                    .clone()
                    .map(String::into_boxed_str)
            }),
            policy_admission: policy_dispatch.and_then(|value| value.admission.clone()),
            policy_execution_outcome: policy_execution
                .map(|value| Box::new(value.policy_result.codes_only())),
            policy_signal_id: policy_signal.map(|value| value.signal_id.clone().into_boxed_str()),
            policy_signal_kind: policy_signal.map(|value| value.kind),
            performance_pressure: performance_pressure
                .map(|value| Box::new(value.pressure.clone())),
            performance_context: performance_summary.map(|value| Box::new(value.context.clone())),
            performance_frame_gap_ms: performance_stutter.map(|value| value.frame_gap_ms),
            performance_monitor_health: performance_state.map(|value| value.health),
            performance_control_level: performance_control.map(|value| value.level),
            performance_control_reason: performance_control.map(|value| value.reason),
            performance_deadline_disposition: performance_control
                .and_then(|value| value.deadline_disposition),
            fact_scope: fact_identity.map(|value| Box::new(value.0.clone())),
            fact_key: fact_identity.map(|value| value.1.to_owned().into_boxed_str()),
            fact_source_snapshot_id: fact_identity.map(|value| value.2.to_owned().into_boxed_str()),
            client_surface_id: client_action
                .map(|value| value.record().surface_id().to_owned().into_boxed_str()),
            client_control_id: client_action
                .map(|value| value.record().control_id().to_owned().into_boxed_str()),
            client_action_kind: client_action.map(|value| value.record().kind()),
            approval_id: approval
                .map(|value| value.decision().approval_id().to_owned().into_boxed_str()),
            approval_disposition: approval.map(|value| value.decision().disposition()),
            approval_target_kind: approval.map(|value| value.decision().target().kind()),
            approval_target_id: approval.and_then(|value| {
                approval_target_id(value.decision().target())
                    .map(|id| id.to_owned().into_boxed_str())
            }),
            approval_catalog_hash: approval.map(|value| {
                value
                    .decision()
                    .target()
                    .catalog_hash()
                    .to_owned()
                    .into_boxed_str()
            }),
            approval_catalog_version: approval
                .map(|value| value.decision().target().catalog_version()),
            agent_wake_id: agent_wake
                .map(AgentWakeData::wake_id)
                .or_else(|| agent_session.map(|value| value.status().wake_id())),
            agent_session_id: agent_session.map(|value| value.status().session_id()),
            agent_wake_kind: agent_wake.map(AgentWakeData::kind),
            agent_attention_state: agent_wake
                .map(AgentWakeData::attention_state)
                .or_else(|| agent_session.map(|value| value.status().state())),
            agent_attempts_used: agent_session.map(|value| value.status().attempts_used()),
            agent_attempt_limit: agent_wake
                .map(|value| value.budget().max_attempts())
                .or_else(|| agent_session.map(|value| value.status().budget().max_attempts())),
        };
        match self {
            Self::Provider(_) => PublicEventPayload::Provider(payload),
            Self::Runtime(_) => PublicEventPayload::Runtime(payload),
            Self::Monitor(_) => PublicEventPayload::Monitor(payload),
            Self::Performance(_) => PublicEventPayload::Performance(payload),
            Self::Fact(_) => PublicEventPayload::Fact(payload),
            Self::Approval(_) => PublicEventPayload::Approval(payload),
            Self::Command(_) => PublicEventPayload::Command(payload),
            Self::Scheduler(_) => PublicEventPayload::Scheduler(payload),
            Self::Policy(_) => PublicEventPayload::Policy(payload),
            Self::Catalog(_) => PublicEventPayload::Catalog(payload),
            Self::State(_) => PublicEventPayload::State(payload),
            Self::Release(_) => PublicEventPayload::Release(payload),
            Self::Agent(_) => PublicEventPayload::Agent(payload),
            Self::Lease(_) => PublicEventPayload::Lease(payload),
            Self::Task(_) => PublicEventPayload::Task(payload),
            Self::Application(_) => PublicEventPayload::Application(payload),
            Self::Input(_) => PublicEventPayload::Input(payload),
            Self::Capture(_) => PublicEventPayload::Capture(payload),
            Self::Recognition(_) => PublicEventPayload::Recognition(payload),
            Self::Artifact(_) => PublicEventPayload::Artifact(payload),
            Self::ResourceAuthoring(_) => PublicEventPayload::ResourceAuthoring(payload),
            Self::Client(_) => PublicEventPayload::Client(payload),
            Self::Ledger(_) => PublicEventPayload::Ledger(payload),
        }
    }

    fn family_payload(&self) -> &dyn FamilyPayload {
        match self {
            Self::Provider(value) => value,
            Self::Runtime(value) => value,
            Self::Monitor(value) => value,
            Self::Performance(value) => value,
            Self::Fact(value) => value,
            Self::Approval(value) => value,
            Self::Command(value) => value,
            Self::Scheduler(value) => value,
            Self::Policy(value) => value,
            Self::Catalog(value) => value,
            Self::State(value) => value,
            Self::Release(value) => value,
            Self::Agent(value) => value,
            Self::Lease(value) => value,
            Self::Task(value) => value,
            Self::Application(value) => value,
            Self::Input(value) => value,
            Self::Capture(value) => value,
            Self::Recognition(value) => value,
            Self::Artifact(value) => value,
            Self::ResourceAuthoring(value) => value,
            Self::Client(value) => value,
            Self::Ledger(value) => value,
        }
    }
}

fn monitor_outcome(payload: &EventPayload) -> Option<&MonitorOutcomePayload> {
    match payload {
        EventPayload::Monitor(MonitorPayload::Completed(value)) => Some(value),
        _ => None,
    }
}

fn performance_pressure(payload: &EventPayload) -> Option<&PerformancePressurePayload> {
    match payload {
        EventPayload::Performance(PerformancePayload::PressureStarted(value))
        | EventPayload::Performance(PerformancePayload::PressureEnded(value)) => Some(value),
        _ => None,
    }
}

fn performance_stutter(payload: &EventPayload) -> Option<&PerformanceStutterPayload> {
    match payload {
        EventPayload::Performance(PerformancePayload::StutterDetected(value)) => Some(value),
        _ => None,
    }
}

fn performance_summary(payload: &EventPayload) -> Option<&PerformanceSummaryPayload> {
    match payload {
        EventPayload::Performance(PerformancePayload::Summary(value)) => Some(value.as_ref()),
        _ => None,
    }
}

fn performance_monitor_state(payload: &EventPayload) -> Option<&PerformanceMonitorStatePayload> {
    match payload {
        EventPayload::Performance(PerformancePayload::MonitorDegraded(value))
        | EventPayload::Performance(PerformancePayload::MonitorRecovered(value)) => Some(value),
        _ => None,
    }
}

fn performance_control(payload: &EventPayload) -> Option<&PerformanceControlPayload> {
    match payload {
        EventPayload::Performance(PerformancePayload::BalanceChanged(value)) => Some(value),
        _ => None,
    }
}

fn fact_identity(payload: &EventPayload) -> Option<(&FactScope, &str, &str)> {
    match payload {
        EventPayload::Fact(FactPayload::Published(value)) => Some((
            &value.record.scope,
            &value.record.key,
            &value.record.source_snapshot_id,
        )),
        EventPayload::Fact(FactPayload::Invalidated(value)) => Some((
            &value.invalidation.scope,
            &value.invalidation.key,
            &value.invalidation.source_snapshot_id,
        )),
        _ => None,
    }
}

fn client_action(payload: &EventPayload) -> Option<&ClientActionPayload> {
    match payload {
        EventPayload::Client(ClientPayload::Action(value)) => Some(value),
        _ => None,
    }
}

fn approval_decision(payload: &EventPayload) -> Option<&ApprovalDecisionPayload> {
    match payload {
        EventPayload::Approval(ApprovalPayload::Decision(value)) => Some(value),
        _ => None,
    }
}

fn agent_wake(payload: &EventPayload) -> Option<&AgentWakeData> {
    match payload {
        EventPayload::Agent(AgentPayload::WakeRequested(value)) => Some(value.wake()),
        _ => None,
    }
}

fn agent_session(payload: &EventPayload) -> Option<&AgentSessionEventData> {
    match payload {
        EventPayload::Agent(
            AgentPayload::SessionStarted(value)
            | AgentPayload::SessionResumed(value)
            | AgentPayload::ResponseRecorded(value)
            | AgentPayload::SessionCompleted(value)
            | AgentPayload::SessionEscalated(value),
        ) => Some(value.session()),
        _ => None,
    }
}

fn approval_target_id(target: &ApprovalTarget) -> Option<&str> {
    match target {
        ApprovalTarget::Catalog { .. } => None,
        ApprovalTarget::Plan { plan_id, .. } => Some(plan_id),
        ApprovalTarget::Decision { decision_id, .. } => Some(decision_id),
    }
}

fn fact_sensitivity(payload: &FactPayload) -> Sensitivity {
    let FactPayload::Published(value) = payload else {
        return Sensitivity::Internal;
    };
    value
        .records()
        .fold(Sensitivity::Internal, |sensitivity, record| {
            let crate::FactContent::Artifact { artifact } = &record.content else {
                return sensitivity;
            };
            sensitivity.max(match artifact.redaction_state() {
                ArtifactRedactionState::Pending => Sensitivity::Secret,
                ArtifactRedactionState::Applied => Sensitivity::Sensitive,
                ArtifactRedactionState::NotRequired => Sensitivity::Internal,
            })
        })
}

fn monitor_recovery(payload: &EventPayload) -> Option<MonitorRecoveryKind> {
    match payload {
        EventPayload::Monitor(MonitorPayload::Completed(value)) => value.decision.recovery(),
        EventPayload::Monitor(MonitorPayload::RecoveryAdmitted(value))
        | EventPayload::Monitor(MonitorPayload::RecoveryDeferred(value)) => Some(value.recovery()),
        _ => None,
    }
}

fn monitor_recovery_coordination(
    payload: &EventPayload,
) -> Option<&MonitorRecoveryCoordinationPayload> {
    match payload {
        EventPayload::Monitor(MonitorPayload::RecoveryAdmitted(value))
        | EventPayload::Monitor(MonitorPayload::RecoveryDeferred(value)) => Some(value),
        _ => None,
    }
}

fn observation_result(payload: &EventPayload) -> Option<&ObservationResultPayload> {
    match payload {
        EventPayload::Capture(CapturePayload::Completed(result))
        | EventPayload::Recognition(RecognitionPayload::Completed(result)) => Some(result),
        _ => None,
    }
}

fn capture_pressure(payload: &EventPayload) -> Option<&CapturePressurePayload> {
    match payload {
        EventPayload::Capture(CapturePayload::PressureChanged(value)) => Some(value),
        _ => None,
    }
}

fn capture_dedup(payload: &EventPayload) -> Option<&CaptureDedupWindowPayload> {
    match payload {
        EventPayload::Capture(CapturePayload::DedupWindow(value)) => Some(value),
        _ => None,
    }
}

fn capture_policy(payload: &EventPayload) -> Option<&CapturePolicyPayload> {
    match payload {
        EventPayload::Capture(CapturePayload::PolicyChanged(value)) => Some(value),
        _ => None,
    }
}

fn artifact_export(payload: &EventPayload) -> Option<(TaskOutcome, EvidenceCompleteness, u64)> {
    match payload {
        EventPayload::Artifact(ArtifactPayload::ExportCompleted(value)) => Some((
            value.task_outcome,
            value.evidence_completeness,
            value.artifact_count,
        )),
        EventPayload::Artifact(ArtifactPayload::ExportFailed(value)) => Some((
            value.task_outcome,
            value.evidence_completeness,
            value.artifact_count,
        )),
        _ => None,
    }
}

fn resource_authoring(payload: &EventPayload) -> Option<&ResourceAuthoringPayload> {
    match payload {
        EventPayload::ResourceAuthoring(value) => Some(value),
        _ => None,
    }
}

fn task_semantic_fact(payload: &EventPayload) -> Option<&TaskSemanticFact> {
    match payload {
        EventPayload::Task(TaskPayload::Semantic(value)) => Some(value.fact()),
        _ => None,
    }
}

fn policy_dispatch(payload: &EventPayload) -> Option<&PolicyDispatchPayload> {
    match payload {
        EventPayload::Policy(PolicyPayload::DispatchIntent(value))
        | EventPayload::Policy(PolicyPayload::DispatchAdmitted(value))
        | EventPayload::Policy(PolicyPayload::DispatchRejected(value))
        | EventPayload::Policy(PolicyPayload::DispatchCompleted(value)) => Some(value),
        _ => None,
    }
}

fn policy_execution(payload: &EventPayload) -> Option<&PolicyExecutionPayload> {
    match payload {
        EventPayload::Policy(PolicyPayload::ExecutionRecorded(value)) => Some(value),
        _ => None,
    }
}

fn policy_planning_signal(payload: &EventPayload) -> Option<&PolicyPlanningSignalPayload> {
    match payload {
        EventPayload::Policy(PolicyPayload::PlanningSignalObserved(value)) => Some(value),
        _ => None,
    }
}

fn catalog_transition(payload: &EventPayload) -> Option<&CatalogTransitionPayload> {
    match payload {
        EventPayload::Catalog(CatalogPayload::TransitionIntent(value))
        | EventPayload::Catalog(CatalogPayload::Activated(value))
        | EventPayload::Catalog(CatalogPayload::RolledBack(value))
        | EventPayload::Catalog(CatalogPayload::TransitionFailed(value)) => Some(value),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    backend_open: Option<Box<crate::BackendOpenSummary>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prior_epoch_close: Option<Box<crate::PriorEpochCloseSummary>>,
    event_type: EventType,
    action: EventAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    device_diagnostic_config: Option<DeviceDiagnosticConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    device_diagnostics: Option<Box<DeviceDiagnosticBudgetRecord>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    effect_disposition: Option<EffectDisposition>,
    /// The record's outcome, codes only (ledger revision 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    outcome: Option<Box<Outcome>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lifecycle_failure: Option<Box<RuntimeLifecycleFailureRecord>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    segment_index: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    affected_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    frame_width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    frame_height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recognition_verdict: Option<RecognitionVerdict>,
    #[serde(skip_serializing_if = "Option::is_none")]
    capture_pressure_state: Option<CapturePressureState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    memory_budget_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resident_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duplicate_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    preserved_frame_id: Option<Box<crate::FrameId>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    artifact_retention: Option<Box<ArtifactRetentionPublicSummary>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lab_pin_release_request: Option<crate::LabPinReleaseTarget>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cadence_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    retention_class: Option<RetentionClass>,
    #[serde(skip_serializing_if = "Option::is_none")]
    capture_policy_reason: Option<CapturePolicyReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    task_outcome: Option<TaskOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    task_semantic_fact: Option<Box<TaskSemanticFact>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    evidence_completeness: Option<EvidenceCompleteness>,
    #[serde(skip_serializing_if = "Option::is_none")]
    artifact_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    monitor_diagnosis: Option<MonitorDiagnosis>,
    #[serde(skip_serializing_if = "Option::is_none")]
    monitor_disposition: Option<MonitorDisposition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    monitor_recovery: Option<MonitorRecoveryKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    monitor_recovery_coordination_reason: Option<MonitorRecoveryCoordinationReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    authoring_phase: Option<ResourceAuthoringPhase>,
    #[serde(skip_serializing_if = "Option::is_none")]
    draft_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_fingerprint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    changed_path_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    decision_id: Option<Box<str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason_chain_id: Option<Box<str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_ledger_position: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fact_snapshot_id: Option<Box<str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    approval_fact_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    catalog_id: Option<Box<str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    catalog_hash: Option<Box<str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    catalog_version: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_catalog_hash: Option<Box<str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    policy_admission: Option<Box<PolicyAdmissionRecord>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    policy_execution_outcome: Option<Box<PolicyExecutionOutcome>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    policy_signal_id: Option<Box<str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    policy_signal_kind: Option<PolicyPlanningSignalKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    performance_pressure: Option<Box<PerformancePressureRecord>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    performance_context: Option<Box<PerformanceContext>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    performance_frame_gap_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    performance_monitor_health: Option<PerformanceMonitorHealth>,
    #[serde(skip_serializing_if = "Option::is_none")]
    performance_control_level: Option<PerformanceControlLevel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    performance_control_reason: Option<PerformanceControlReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    performance_deadline_disposition: Option<PerformanceDeadlineDisposition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fact_scope: Option<Box<FactScope>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fact_key: Option<Box<str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fact_source_snapshot_id: Option<Box<str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_surface_id: Option<Box<str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_control_id: Option<Box<str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_action_kind: Option<ClientActionKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    approval_id: Option<Box<str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    approval_disposition: Option<ApprovalDisposition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    approval_target_kind: Option<ApprovalTargetKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    approval_target_id: Option<Box<str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    approval_catalog_hash: Option<Box<str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    approval_catalog_version: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_wake_id: Option<AgentWakeId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_session_id: Option<AgentSessionId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_wake_kind: Option<AgentWakeKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_attention_state: Option<AgentAttentionState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_attempts_used: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_attempt_limit: Option<u16>,
}

impl PublicPayload {
    pub fn backend_open(&self) -> Option<&crate::BackendOpenSummary> {
        self.backend_open.as_deref()
    }

    pub fn prior_epoch_close(&self) -> Option<&crate::PriorEpochCloseSummary> {
        self.prior_epoch_close.as_deref()
    }
    pub fn device_diagnostics(&self) -> Option<&DeviceDiagnosticBudgetRecord> {
        self.device_diagnostics.as_deref()
    }

    pub fn lifecycle_failure(&self) -> Option<&RuntimeLifecycleFailureRecord> {
        self.lifecycle_failure.as_deref()
    }
    /// The record's outcome, codes only (ledger revision 3).
    pub fn outcome(&self) -> Option<&Outcome> {
        self.outcome.as_deref()
    }

    pub const fn event_type(&self) -> EventType {
        self.event_type
    }

    pub const fn action(&self) -> EventAction {
        self.action
    }

    pub const fn effect_disposition(&self) -> Option<EffectDisposition> {
        self.effect_disposition
    }

    pub const fn segment_index(&self) -> Option<u64> {
        self.segment_index
    }

    pub const fn affected_bytes(&self) -> Option<u64> {
        self.affected_bytes
    }

    pub const fn frame_width(&self) -> Option<u32> {
        self.frame_width
    }

    pub const fn frame_height(&self) -> Option<u32> {
        self.frame_height
    }

    pub const fn recognition_verdict(&self) -> Option<RecognitionVerdict> {
        self.recognition_verdict
    }

    pub const fn capture_pressure_state(&self) -> Option<CapturePressureState> {
        self.capture_pressure_state
    }

    pub const fn memory_budget_bytes(&self) -> Option<u64> {
        self.memory_budget_bytes
    }

    pub const fn resident_bytes(&self) -> Option<u64> {
        self.resident_bytes
    }

    pub const fn duplicate_count(&self) -> Option<u64> {
        self.duplicate_count
    }

    pub const fn duration_ms(&self) -> Option<u64> {
        self.duration_ms
    }

    pub fn preserved_frame_id(&self) -> Option<&crate::FrameId> {
        self.preserved_frame_id.as_deref()
    }

    pub fn artifact_retention(&self) -> Option<&ArtifactRetentionPublicSummary> {
        self.artifact_retention.as_deref()
    }

    pub fn lab_pin_release_request(&self) -> Option<&crate::LabPinReleaseTarget> {
        self.lab_pin_release_request.as_ref()
    }

    pub const fn cadence_ms(&self) -> Option<u64> {
        self.cadence_ms
    }

    pub const fn retention_class(&self) -> Option<RetentionClass> {
        self.retention_class
    }

    pub const fn capture_policy_reason(&self) -> Option<CapturePolicyReason> {
        self.capture_policy_reason
    }

    pub const fn task_outcome(&self) -> Option<TaskOutcome> {
        self.task_outcome
    }

    pub fn task_semantic_fact(&self) -> Option<&TaskSemanticFact> {
        self.task_semantic_fact.as_deref()
    }

    pub const fn evidence_completeness(&self) -> Option<EvidenceCompleteness> {
        self.evidence_completeness
    }

    pub const fn artifact_count(&self) -> Option<u64> {
        self.artifact_count
    }

    pub const fn monitor_diagnosis(&self) -> Option<MonitorDiagnosis> {
        self.monitor_diagnosis
    }

    pub const fn monitor_disposition(&self) -> Option<MonitorDisposition> {
        self.monitor_disposition
    }

    pub const fn monitor_recovery(&self) -> Option<MonitorRecoveryKind> {
        self.monitor_recovery
    }

    pub const fn monitor_recovery_coordination_reason(
        &self,
    ) -> Option<MonitorRecoveryCoordinationReason> {
        self.monitor_recovery_coordination_reason
    }

    pub const fn authoring_phase(&self) -> Option<ResourceAuthoringPhase> {
        self.authoring_phase
    }

    pub fn draft_id(&self) -> Option<&str> {
        self.draft_id.as_deref()
    }

    pub fn target_label(&self) -> Option<&str> {
        self.target_label.as_deref()
    }

    pub fn target_fingerprint(&self) -> Option<&str> {
        self.target_fingerprint.as_deref()
    }

    pub const fn changed_path_count(&self) -> Option<u64> {
        self.changed_path_count
    }

    pub fn decision_id(&self) -> Option<&str> {
        self.decision_id.as_deref()
    }

    pub fn reason_chain_id(&self) -> Option<&str> {
        self.reason_chain_id.as_deref()
    }

    pub const fn reason_count(&self) -> Option<u64> {
        self.reason_count
    }

    pub const fn input_ledger_position(&self) -> Option<u64> {
        self.input_ledger_position
    }

    pub fn fact_snapshot_id(&self) -> Option<&str> {
        self.fact_snapshot_id.as_deref()
    }

    pub const fn approval_fact_count(&self) -> Option<u64> {
        self.approval_fact_count
    }

    pub fn catalog_id(&self) -> Option<&str> {
        self.catalog_id.as_deref()
    }

    pub fn catalog_hash(&self) -> Option<&str> {
        self.catalog_hash.as_deref()
    }

    pub const fn catalog_version(&self) -> Option<u64> {
        self.catalog_version
    }

    pub fn previous_catalog_hash(&self) -> Option<&str> {
        self.previous_catalog_hash.as_deref()
    }

    pub fn policy_admission(&self) -> Option<&PolicyAdmissionRecord> {
        self.policy_admission.as_deref()
    }

    pub fn policy_execution_outcome(&self) -> Option<&PolicyExecutionOutcome> {
        self.policy_execution_outcome.as_deref()
    }

    pub fn policy_signal_id(&self) -> Option<&str> {
        self.policy_signal_id.as_deref()
    }

    pub const fn policy_signal_kind(&self) -> Option<PolicyPlanningSignalKind> {
        self.policy_signal_kind
    }

    pub fn performance_pressure(&self) -> Option<&PerformancePressureRecord> {
        self.performance_pressure.as_deref()
    }

    pub fn performance_context(&self) -> Option<&PerformanceContext> {
        self.performance_context.as_deref()
    }

    pub const fn performance_frame_gap_ms(&self) -> Option<u64> {
        self.performance_frame_gap_ms
    }

    pub const fn performance_monitor_health(&self) -> Option<PerformanceMonitorHealth> {
        self.performance_monitor_health
    }

    pub const fn performance_control_level(&self) -> Option<PerformanceControlLevel> {
        self.performance_control_level
    }

    pub const fn performance_control_reason(&self) -> Option<PerformanceControlReason> {
        self.performance_control_reason
    }

    pub const fn performance_deadline_disposition(&self) -> Option<PerformanceDeadlineDisposition> {
        self.performance_deadline_disposition
    }

    pub fn fact_scope(&self) -> Option<&FactScope> {
        self.fact_scope.as_deref()
    }

    pub fn fact_key(&self) -> Option<&str> {
        self.fact_key.as_deref()
    }

    pub fn fact_source_snapshot_id(&self) -> Option<&str> {
        self.fact_source_snapshot_id.as_deref()
    }

    pub fn client_surface_id(&self) -> Option<&str> {
        self.client_surface_id.as_deref()
    }

    pub fn client_control_id(&self) -> Option<&str> {
        self.client_control_id.as_deref()
    }

    pub const fn client_action_kind(&self) -> Option<ClientActionKind> {
        self.client_action_kind
    }

    pub fn approval_id(&self) -> Option<&str> {
        self.approval_id.as_deref()
    }

    pub const fn approval_disposition(&self) -> Option<ApprovalDisposition> {
        self.approval_disposition
    }

    pub const fn approval_target_kind(&self) -> Option<ApprovalTargetKind> {
        self.approval_target_kind
    }

    pub fn approval_target_id(&self) -> Option<&str> {
        self.approval_target_id.as_deref()
    }

    pub fn approval_catalog_hash(&self) -> Option<&str> {
        self.approval_catalog_hash.as_deref()
    }

    pub const fn approval_catalog_version(&self) -> Option<u64> {
        self.approval_catalog_version
    }

    pub const fn agent_wake_id(&self) -> Option<AgentWakeId> {
        self.agent_wake_id
    }

    pub const fn agent_session_id(&self) -> Option<AgentSessionId> {
        self.agent_session_id
    }

    pub const fn agent_wake_kind(&self) -> Option<AgentWakeKind> {
        self.agent_wake_kind
    }

    pub const fn agent_attention_state(&self) -> Option<AgentAttentionState> {
        self.agent_attention_state
    }

    pub const fn agent_attempts_used(&self) -> Option<u16> {
        self.agent_attempts_used
    }

    pub const fn agent_attempt_limit(&self) -> Option<u16> {
        self.agent_attempt_limit
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "family",
    content = "payload",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PublicEventPayload {
    Provider(PublicPayload),
    Runtime(PublicPayload),
    Monitor(PublicPayload),
    Performance(PublicPayload),
    Fact(PublicPayload),
    Approval(PublicPayload),
    Command(PublicPayload),
    Scheduler(PublicPayload),
    Policy(PublicPayload),
    Catalog(PublicPayload),
    State(PublicPayload),
    Release(PublicPayload),
    Agent(PublicPayload),
    Lease(PublicPayload),
    Task(PublicPayload),
    Application(PublicPayload),
    Input(PublicPayload),
    Capture(PublicPayload),
    Recognition(PublicPayload),
    Artifact(PublicPayload),
    ResourceAuthoring(PublicPayload),
    Client(PublicPayload),
    Ledger(PublicPayload),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "detail",
    content = "payload",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ProjectionPayload {
    Omitted,
    Public(Box<PublicEventPayload>),
    Full(Box<EventPayload>),
}

fn validate_fingerprint(candidate: &str, original: &str) -> Result<(), SanitizationError> {
    let valid = candidate
        .strip_prefix("sha256:")
        .is_some_and(|digest| is_sha256(candidate) && candidate != original && digest != original);
    if valid {
        Ok(())
    } else {
        Err(SanitizationError::new(
            SanitizationCode::InvalidFingerprint,
            "account_identity",
        ))
    }
}

fn is_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

const fn is_false(value: &bool) -> bool {
    !*value
}
