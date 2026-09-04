// SPDX-License-Identifier: AGPL-3.0-only

use super::{
    ArtifactRedactionState, CapturePolicyReason, CapturePressureState, DiagnosticCode, EventAction,
    EventFamily, EventType, EvidenceCompleteness, PinnedFrameReason, PolicyFailureClass,
    PolicyFailureDisposition, PolicyPlanningSignalKind, RecognitionVerdict, RecoveryReason,
    ResourceAuthoringPhase, RetentionClass, SanitizationError, Sensitivity, TaskOutcome,
};
use crate::{
    AgentAttentionState, AgentSessionEventData, AgentSessionId, AgentWakeData, AgentWakeId,
    AgentWakeKind, ApprovalDecisionRecord, ApprovalDisposition, ApprovalTarget, ApprovalTargetKind,
    ArtifactKind, CatalogPromotionAuthorization, ClientActionKind, ClientActionRecord,
    CorrelationId, EventId, FactInvalidationEventData, FactRecord, FactScope, HolderId,
    InputAction, InstanceId, LeaseId, LeasePriority, MonitorDecision, MonitorDiagnosis,
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
const MAX_DIAGNOSTIC_DETAIL_MESSAGE_BYTES: usize = 1_024;
const MAX_MACHINE_PATH_BASENAME_BYTES: usize = 255;
const LEGACY_REDACTED_MACHINE_PATH: &str = "[redacted]";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretField {
    AccountIdentity,
    AuthenticationMaterial,
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
                "invalid_fingerprint",
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
                .map_err(|_| SanitizationError::new("fingerprinter_failed", "account_identity"))?;
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
            return Err(SanitizationError::new("invalid_sanitized_payload", "audit"));
        }
        Ok(())
    }
}

fn sanitize_machine_path(original: String) -> Result<String, SanitizationError> {
    let basename = original
        .rsplit(['/', '\\'])
        .next()
        .filter(|value| valid_machine_path_basename(value))
        .ok_or_else(|| SanitizationError::new("invalid_machine_path", "machine_path"))?;
    let encoded_basename = serde_json::to_string(basename)
        .map_err(|_| SanitizationError::new("invalid_machine_path", "machine_path"))?;
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
                "invalid_input_execution_plan",
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
                "invalid_input_execution_plan",
                "events",
            ));
        }
        let hold_index = holds[0];
        if hold_index <= 1 || hold_index + 2 >= self.events.len() {
            return Err(SanitizationError::new(
                "invalid_input_execution_plan",
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
                        "invalid_input_execution_plan",
                        "events",
                    ));
                };
                if *x < 0 || *y < 0 || *delay_before_ms == 0 {
                    return Err(SanitizationError::new(
                        "invalid_input_execution_plan",
                        "events",
                    ));
                }
                duration_ms = duration_ms.checked_add(*delay_before_ms).ok_or_else(|| {
                    SanitizationError::new("invalid_input_execution_plan", "events")
                })?;
            }
            if duration_ms != expected_duration_ms {
                return Err(SanitizationError::new(
                    "invalid_input_execution_plan",
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
                "invalid_input_execution_plan",
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
                "invalid_input_execution_plan",
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
    audit: SanitizedAudit,
}

impl InputIntentPayload {
    pub const fn action(&self) -> EventAction {
        self.action
    }

    pub const fn execution_plan(&self) -> Option<&InputExecutionPlanRecord> {
        self.execution_plan.as_ref()
    }

    pub fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }

    fn validate(&self) -> Result<(), SanitizationError> {
        if let Some(execution_plan) = &self.execution_plan {
            if self.action != EventAction::InputSwipe {
                return Err(SanitizationError::new(
                    "invalid_input_execution_plan",
                    "action",
                ));
            }
            execution_plan.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct DiagnosticDetailDraft {
    category: String,
    stage: String,
    backend: String,
    operation: String,
    message: String,
    declared_sensitivity: Sensitivity,
}

impl DiagnosticDetailDraft {
    pub fn new(
        category: impl Into<String>,
        stage: impl Into<String>,
        backend: impl Into<String>,
        operation: impl Into<String>,
        message: impl Into<String>,
        declared_sensitivity: Sensitivity,
    ) -> Self {
        Self {
            category: category.into(),
            stage: stage.into(),
            backend: backend.into(),
            operation: operation.into(),
            message: message.into(),
            declared_sensitivity,
        }
    }

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

    pub fn message(&self) -> &str {
        &self.message
    }

    pub const fn declared_sensitivity(&self) -> Sensitivity {
        self.declared_sensitivity
    }

    fn sanitize(self) -> Result<DiagnosticDetailRecord, SanitizationError> {
        validate_diagnostic_detail(
            &self.category,
            &self.stage,
            &self.backend,
            &self.operation,
            &self.message,
            self.declared_sensitivity,
        )?;
        Ok(DiagnosticDetailRecord {
            category: self.category,
            stage: self.stage,
            backend: self.backend,
            operation: self.operation,
            message: self.message,
            declared_sensitivity: self.declared_sensitivity,
        })
    }
}

impl fmt::Debug for DiagnosticDetailDraft {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DiagnosticDetailDraft")
            .field("category", &self.category)
            .field("stage", &self.stage)
            .field("backend", &self.backend)
            .field("operation", &self.operation)
            .field("message", &"<redacted>")
            .field("declared_sensitivity", &self.declared_sensitivity)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticDetailRecord {
    category: String,
    stage: String,
    backend: String,
    operation: String,
    message: String,
    declared_sensitivity: Sensitivity,
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

    pub fn message(&self) -> &str {
        &self.message
    }

    pub const fn declared_sensitivity(&self) -> Sensitivity {
        self.declared_sensitivity
    }

    fn validate(&self) -> Result<(), SanitizationError> {
        validate_diagnostic_detail(
            &self.category,
            &self.stage,
            &self.backend,
            &self.operation,
            &self.message,
            self.declared_sensitivity,
        )
    }
}

impl fmt::Debug for DiagnosticDetailRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DiagnosticDetailRecord")
            .field("category", &self.category)
            .field("stage", &self.stage)
            .field("backend", &self.backend)
            .field("operation", &self.operation)
            .field("message", &"<redacted>")
            .field("declared_sensitivity", &self.declared_sensitivity)
            .finish()
    }
}

fn validate_diagnostic_detail(
    category: &str,
    stage: &str,
    backend: &str,
    operation: &str,
    message: &str,
    declared_sensitivity: Sensitivity,
) -> Result<(), SanitizationError> {
    validate_diagnostic_detail_token(category, "category")?;
    validate_diagnostic_detail_stage(stage)?;
    validate_diagnostic_detail_token(backend, "backend")?;
    validate_diagnostic_detail_token(operation, "operation")?;
    if message.is_empty()
        || message.len() > MAX_DIAGNOSTIC_DETAIL_MESSAGE_BYTES
        || message.chars().any(char::is_control)
    {
        return Err(SanitizationError::new(
            "invalid_diagnostic_detail_message",
            "message",
        ));
    }
    if diagnostic_message_has_unsafe_shape(message) {
        return Err(SanitizationError::new(
            "unsafe_diagnostic_detail_message",
            "message",
        ));
    }
    let lower = message.to_ascii_lowercase();
    let carries_native_output = ["stdout=", "stderr=", "exit_status="]
        .iter()
        .any(|marker| lower.contains(marker));
    if declared_sensitivity == Sensitivity::Public
        || (carries_native_output && declared_sensitivity < Sensitivity::Sensitive)
    {
        return Err(SanitizationError::new(
            "invalid_diagnostic_detail_sensitivity",
            "declared_sensitivity",
        ));
    }
    Ok(())
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
            "invalid_diagnostic_detail_token",
            field,
        ))
    } else {
        Ok(())
    }
}

fn validate_diagnostic_detail_stage(value: &str) -> Result<(), SanitizationError> {
    let segments = value.split('.').collect::<Vec<_>>();
    if value.len() > MAX_DIAGNOSTIC_DETAIL_TOKEN_BYTES
        || !(2..=4).contains(&segments.len())
        || segments.iter().any(|segment| {
            segment.is_empty()
                || !segment.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'_' | b'-')
                })
        })
    {
        Err(SanitizationError::new(
            "invalid_diagnostic_detail_stage",
            "stage",
        ))
    } else {
        Ok(())
    }
}

fn diagnostic_message_has_unsafe_shape(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    if [
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
        || value.contains("\\\\")
        || value.contains("//")
        || value.as_bytes().windows(3).any(|window| {
            window[0].is_ascii_alphabetic()
                && window[1] == b':'
                && matches!(window[2], b'\\' | b'/')
        })
    {
        return true;
    }
    value
        .split(|character: char| {
            character.is_whitespace() || matches!(character, ',' | ';' | '(' | ')' | '"' | '\'')
        })
        .map(|candidate| candidate.trim_matches(|character| matches!(character, '.' | ':')))
        .any(|candidate| {
            (candidate.starts_with('/') && candidate.len() > 1)
                || candidate.parse::<std::net::SocketAddr>().is_ok()
                || candidate.rsplit_once(':').is_some_and(|(host, port)| {
                    !host.is_empty()
                        && port.parse::<u16>().is_ok()
                        && (host == "localhost" || host.parse::<std::net::IpAddr>().is_ok())
                })
        })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticPayload {
    action: EventAction,
    diagnostic_code: DiagnosticCode,
    audit: SanitizedAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutcomePayload {
    action: EventAction,
    effect_disposition: EffectDisposition,
    audit: SanitizedAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticOutcomePayload {
    action: EventAction,
    diagnostic_code: DiagnosticCode,
    effect_disposition: EffectDisposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    detail: Option<DiagnosticDetailRecord>,
    audit: SanitizedAudit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeLifecyclePhase {
    PolicyForwardEntered,
    PolicyForwardReturned { entered_event_id: EventId },
    StrategicReportEntered,
    StrategicReportReturned { entered_event_id: EventId },
    ShutdownRequested,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeLifecyclePayload {
    action: EventAction,
    owner_epoch: OwnerEpoch,
    phase: RuntimeLifecyclePhase,
    audit: SanitizedAudit,
}

impl RuntimeLifecyclePayload {
    pub const fn owner_epoch(&self) -> OwnerEpoch {
        self.owner_epoch
    }

    pub const fn phase(&self) -> RuntimeLifecyclePhase {
        self.phase
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MonitorOutcomePayload {
    action: EventAction,
    effect_disposition: EffectDisposition,
    observation: MonitorObservation,
    decision: MonitorDecision,
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
    audit: SanitizedAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceMonitorStatePayload {
    action: EventAction,
    observed_at_unix_ms: u64,
    health: PerformanceMonitorHealth,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_code: Option<String>,
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
    audit: SanitizedAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactPublishedPayload {
    action: EventAction,
    record: Box<FactRecord>,
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
                "invalid_capture_summary_artifact",
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
                "invalid_capture_summary_pin",
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
                "capture_summary_bound_exceeded",
                "summary",
            ));
        }
        if u64::try_from(self.frames.len()).ok() != Some(self.persisted) {
            return Err(SanitizationError::new(
                "capture_summary_count_mismatch",
                "persisted",
            ));
        }
        let accounted = self
            .deduplicated
            .checked_add(self.dropped)
            .and_then(|count| count.checked_add(self.persisted))
            .ok_or_else(|| SanitizationError::new("capture_summary_count_overflow", "summary"))?;
        if accounted > self.captured {
            return Err(SanitizationError::new(
                "capture_summary_count_mismatch",
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
                    "capture_summary_frame_conflict",
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
                    "capture_summary_pin_conflict",
                    "pinned",
                ));
            }
            if let Some(frame_index) = pin.frame_index {
                if frame_index > MAX_CAPTURE_SUMMARY_COUNT {
                    return Err(SanitizationError::new(
                        "capture_summary_pin_conflict",
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
                            "capture_summary_pin_artifact_mismatch",
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
                "capture_summary_completeness_mismatch",
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
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_code: Option<String>,
    audit: SanitizedAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyReasonRecord {
    pub code: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyDispatchEventData {
    pub decision_id: String,
    pub task_id: String,
    pub instance_id: String,
    pub operation_id: String,
    pub package_digest: String,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyAdmissionRecord {
    pub activity: PolicyActivitySample,
    pub budget: PolicyBudgetReceipt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PolicyFailureRecord {
    pub error_code: String,
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
            error_code: String,
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
        }

        let wire = Wire::deserialize(deserializer)?;
        Ok(Self {
            error_code: wire.error_code,
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
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PolicyExecutionOutcome {
    Succeeded { runtime_ms: u64 },
    Failed { failure: PolicyFailureRecord },
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
    outcome: PolicyExecutionOutcome,
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

    pub const fn outcome(&self) -> &PolicyExecutionOutcome {
        &self.outcome
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyPlanningSignalEventData {
    pub signal_id: String,
    pub instance_id: String,
    pub task_id: Option<String>,
    pub kind: PolicyPlanningSignalKind,
    pub fact_code: String,
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
    fact_code: String,
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

    pub fn fact_code(&self) -> &str {
        &self.fact_code
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
    package_digest: String,
    #[serde(default)]
    procedure_binding_digest: String,
    reason_chain_id: String,
    reasons: Vec<PolicyReasonRecord>,
    catalog_hash: String,
    catalog_version: u64,
    input_ledger_position: u64,
    fact_snapshot_id: String,
    approval_fact_ids: Vec<String>,
    #[serde(default)]
    urgency_milli: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    admission: Option<Box<PolicyAdmissionRecord>>,
    audit: SanitizedAudit,
}

impl PolicyDispatchPayload {
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

    pub fn package_digest(&self) -> &str {
        &self.package_digest
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

    pub const fn input_ledger_position(&self) -> u64 {
        self.input_ledger_position
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
    audit: SanitizedAudit,
}

impl AgentSessionPayload {
    pub const fn session(&self) -> &AgentSessionEventData {
        &self.session
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskEntryTargetDisposition {
    Started,
    FailClosed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TaskSemanticFact {
    PackageAdmitted {
        package_label: String,
        task_label: String,
        package_sha256: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        response_deadline_monotonic_ms: Option<u64>,
    },
    RunStarted,
    EvidenceIndexed {
        frame_width: u32,
        frame_height: u32,
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
    },
    EntryRecognition {
        phase: TaskEntryRecognitionPhase,
        required_page: String,
        matched: bool,
    },
    EntryRecoveryDecision {
        required: bool,
    },
    EntryRecoveryPackageAdmitted {
        package_sha256: String,
    },
    EntryRecoveryCompleted {
        package_sha256: String,
        final_page: String,
        executed_steps: u32,
    },
    EntryRecoveryFailed {
        package_sha256: String,
        failure_code: String,
    },
    EntryTargetDisposition {
        disposition: TaskEntryTargetDisposition,
        #[serde(skip_serializing_if = "Option::is_none")]
        failure_code: Option<String>,
    },
    StepStarted {
        step_index: u32,
        operation_label: String,
        from_page: String,
    },
    EffectIntent {
        step_index: u32,
        operation_label: String,
        action: InputAction,
    },
    EffectCompleted {
        step_index: u32,
        operation_label: String,
    },
    StepFinished {
        step_index: u32,
        operation_label: String,
        page_label: String,
    },
    Finalizing {
        outcome: TaskOutcome,
    },
    TerminalCommitted {
        outcome: TaskOutcome,
        #[serde(skip_serializing_if = "Option::is_none")]
        final_page: Option<String>,
        executed_steps: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        failure_code: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scheduling_disposition: Option<SchedulingDisposition>,
    },
    TerminalRejected {
        committed_outcome: TaskOutcome,
        attempted_outcome: TaskOutcome,
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputSamplingAlgorithm {
    Xorshift64UniformRectV1,
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
                "invalid_input_sampling_region",
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
        };
        value.validate_regions()?;
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

    fn validate_regions(&self) -> Result<(), SanitizationError> {
        if !matches!(self.source_regions.len(), 1 | 2) {
            return Err(SanitizationError::new(
                "invalid_input_sampling_region",
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
        let valid = match (action, self.source_regions.as_slice()) {
            (InputAction::Tap { x, y }, [region]) => region.contains(*x, *y),
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
            _ => false,
        };
        if valid {
            Ok(())
        } else {
            Err(SanitizationError::new(
                "invalid_input_sampling_action",
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchedulingOutcomeDeclaration {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    designated_operation: Option<String>,
    mappings: Vec<SchedulingOutcomeMapping>,
}

impl SchedulingOutcomeDeclaration {
    pub fn designated_operation(&self) -> Option<&str> {
        self.designated_operation.as_deref()
    }

    pub fn mappings(&self) -> &[SchedulingOutcomeMapping] {
        &self.mappings
    }

    pub fn validate(&self) -> Result<(), SanitizationError> {
        if let Some(operation) = &self.designated_operation {
            validate_task_semantic_label(operation, "designated_operation")?;
        }
        if self.mappings.is_empty() || self.mappings.len() > 64 {
            return Err(SanitizationError::new(
                "invalid_scheduling_outcome_declaration",
                "mappings",
            ));
        }
        let mut keys = BTreeSet::new();
        let mut conditions = BTreeSet::new();
        for mapping in &self.mappings {
            validate_policy_token(&mapping.outcome_key, "outcome_key")?;
            if !keys.insert(mapping.outcome_key.as_str()) {
                return Err(SanitizationError::new(
                    "invalid_scheduling_outcome_declaration",
                    "outcome_key",
                ));
            }
            if mapping.effect == SchedulingEffectCondition::DesignatedEffectCompleted
                && self.designated_operation.is_none()
            {
                return Err(SanitizationError::new(
                    "invalid_scheduling_outcome_declaration",
                    "designated_operation",
                ));
            }
            if mapping.terminal_pages.is_empty() || mapping.terminal_pages.len() > 64 {
                return Err(SanitizationError::new(
                    "invalid_scheduling_outcome_declaration",
                    "terminal_pages",
                ));
            }
            let mut pages = BTreeSet::new();
            for page in &mapping.terminal_pages {
                validate_task_semantic_label(page, "terminal_page")?;
                if !pages.insert(page.as_str()) {
                    return Err(SanitizationError::new(
                        "invalid_scheduling_outcome_declaration",
                        "terminal_pages",
                    ));
                }
                if !conditions.insert((mapping.effect, page.as_str())) {
                    return Err(SanitizationError::new(
                        "invalid_scheduling_outcome_declaration",
                        "mapping_overlap",
                    ));
                }
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
        if let SchedulingEffectEvidence::DesignatedEffectCompleted {
            step_index,
            operation_label,
        } = &self.effect
        {
            validate_task_step(*step_index)?;
            validate_task_semantic_label(operation_label, "operation_label")?;
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
                "invalid_authoritative_scheduling_outcome",
                "position",
            ));
        }
        validate_policy_token(&self.decision_id, "decision_id")?;
        validate_task_semantic_label(&self.catalog_task_id, "catalog_task_id")?;
        validate_task_semantic_label(&self.instance_alias, "instance_alias")
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
                "invalid_authoritative_scheduling_outcome",
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
    outcome: AuthoritativeSchedulingOutcome,
}

impl SchedulingOutcomeProjection {
    pub fn new(
        ledger_position: u64,
        outcome: AuthoritativeSchedulingOutcome,
    ) -> Result<Self, SanitizationError> {
        if ledger_position == 0 || outcome.identity().terminal_sequence() > ledger_position {
            return Err(SanitizationError::new(
                "invalid_scheduling_outcome_projection",
                "ledger_position",
            ));
        }
        Ok(Self {
            ledger_position,
            outcome,
        })
    }

    pub const fn ledger_position(&self) -> u64 {
        self.ledger_position
    }

    pub const fn outcome(&self) -> &AuthoritativeSchedulingOutcome {
        &self.outcome
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSemanticPayload {
    action: EventAction,
    fact: TaskSemanticFact,
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

    pub const fn sampling(&self) -> Option<&InputSamplingEvidence> {
        self.sampling.as_ref()
    }

    pub fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }

    fn validate(&self) -> Result<(), SanitizationError> {
        if self.action != EventAction::RuntimeTaskRun {
            return Err(SanitizationError::new(
                "invalid_task_semantic_action",
                "action",
            ));
        }
        self.fact.validate()?;
        match (&self.fact, &self.sampling) {
            (TaskSemanticFact::EffectIntent { action, .. }, Some(sampling)) => {
                sampling.validate(action)
            }
            (TaskSemanticFact::EffectIntent { .. }, None) | (_, None) => Ok(()),
            (_, Some(_)) => Err(SanitizationError::new(
                "invalid_input_sampling_fact",
                "sampling",
            )),
        }
    }
}

impl TaskSemanticFact {
    fn redact_sensitive_input(&mut self) {
        let Self::EffectIntent { action, .. } = self else {
            return;
        };
        match action {
            InputAction::Key { key } => *key = "[redacted]".to_string(),
            InputAction::Text { text } => *text = "[redacted]".to_string(),
            InputAction::Tap { .. }
            | InputAction::LongTap { .. }
            | InputAction::Swipe { .. }
            | InputAction::SingleTouchDragWithVerticalBrakeV1 { .. }
            | InputAction::Reset => {}
        }
    }

    fn event_type(&self) -> EventType {
        match self {
            Self::PackageAdmitted { .. } => EventType::TaskRequested,
            Self::RunStarted => EventType::TaskStarted,
            Self::EvidenceIndexed { .. } => EventType::TaskEvidenceIndexed,
            Self::RecognitionStarted { .. } => EventType::TaskRecognitionStarted,
            Self::RecognitionCompleted { .. } => EventType::TaskRecognitionCompleted,
            Self::EntryRecognition { .. }
            | Self::EntryRecoveryDecision { .. }
            | Self::EntryRecoveryPackageAdmitted { .. }
            | Self::EntryRecoveryCompleted { .. }
            | Self::EntryRecoveryFailed { .. }
            | Self::EntryTargetDisposition { .. } => EventType::TaskEntryPreflight,
            Self::StepStarted { .. } => EventType::TaskStepStarted,
            Self::EffectIntent { .. } => EventType::TaskEffectIntent,
            Self::EffectCompleted { .. } => EventType::TaskEffectCompleted,
            Self::StepFinished { .. } => EventType::TaskStepFinished,
            Self::Finalizing { .. } => EventType::TaskTerminalIntent,
            Self::TerminalCommitted {
                outcome: TaskOutcome::Success,
                ..
            } => EventType::TaskCompleted,
            Self::TerminalCommitted {
                outcome: TaskOutcome::Failure,
                ..
            } => EventType::TaskFailed,
            Self::TerminalCommitted {
                outcome: TaskOutcome::Cancelled,
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
                package_sha256,
                response_deadline_monotonic_ms,
            } => {
                validate_task_semantic_label(package_label, "package_label")?;
                validate_task_semantic_label(task_label, "task_label")?;
                if package_sha256.len() != 64
                    || !package_sha256
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
                {
                    return Err(SanitizationError::new(
                        "invalid_task_package_fingerprint",
                        "package_sha256",
                    ));
                }
                if response_deadline_monotonic_ms.is_some_and(|deadline| deadline == 0) {
                    return Err(SanitizationError::new(
                        "invalid_task_response_deadline",
                        "response_deadline_monotonic_ms",
                    ));
                }
            }
            Self::RunStarted => {}
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
            } => {
                validate_task_candidate_pages(candidate_pages)?;
                validate_task_frame_dimensions(*frame_width, *frame_height)?;
                if let Some(page) = matched_page {
                    validate_task_semantic_label(page, "matched_page")?;
                    if !candidate_pages.contains(page) {
                        return Err(SanitizationError::new(
                            "invalid_task_recognition_result",
                            "matched_page",
                        ));
                    }
                }
            }
            Self::EntryRecognition { required_page, .. } => {
                validate_task_semantic_label(required_page, "required_page")?;
            }
            Self::EntryRecoveryDecision { .. } => {}
            Self::EntryRecoveryPackageAdmitted { package_sha256 } => {
                validate_task_package_sha256(package_sha256)?;
            }
            Self::EntryRecoveryCompleted {
                package_sha256,
                final_page,
                executed_steps,
            } => {
                validate_task_package_sha256(package_sha256)?;
                validate_task_semantic_label(final_page, "final_page")?;
                if *executed_steps > 1_000 {
                    return Err(SanitizationError::new(
                        "invalid_task_entry_recovery",
                        "executed_steps",
                    ));
                }
            }
            Self::EntryRecoveryFailed {
                package_sha256,
                failure_code,
            } => {
                validate_task_package_sha256(package_sha256)?;
                validate_task_semantic_label(failure_code, "failure_code")?;
            }
            Self::EntryTargetDisposition {
                disposition,
                failure_code,
            } => match (disposition, failure_code) {
                (TaskEntryTargetDisposition::Started, None) => {}
                (TaskEntryTargetDisposition::FailClosed, Some(code)) => {
                    validate_task_semantic_label(code, "failure_code")?;
                }
                _ => {
                    return Err(SanitizationError::new(
                        "invalid_task_entry_disposition",
                        "failure_code",
                    ));
                }
            },
            Self::StepStarted {
                step_index,
                operation_label,
                from_page,
            } => {
                validate_task_step(*step_index)?;
                validate_task_semantic_label(operation_label, "operation_label")?;
                validate_task_semantic_label(from_page, "from_page")?;
            }
            Self::EffectIntent {
                step_index,
                operation_label,
                action,
            } => {
                validate_task_step(*step_index)?;
                validate_task_semantic_label(operation_label, "operation_label")?;
                action
                    .validate()
                    .map_err(|_| SanitizationError::new("invalid_task_effect", "input_action"))?;
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
            } => {
                validate_task_step(*step_index)?;
                validate_task_semantic_label(operation_label, "operation_label")?;
                validate_task_semantic_label(page_label, "page_label")?;
            }
            Self::Finalizing { .. } => {}
            Self::TerminalCommitted {
                outcome,
                final_page,
                executed_steps,
                failure_code,
                scheduling_disposition,
            } => {
                if *executed_steps > 1_000 {
                    return Err(SanitizationError::new(
                        "invalid_task_terminal",
                        "executed_steps",
                    ));
                }
                if let Some(page) = final_page {
                    validate_task_semantic_label(page, "final_page")?;
                }
                match (outcome, failure_code) {
                    (TaskOutcome::Success, None) => {
                        if let Some(disposition) = scheduling_disposition {
                            if final_page.is_none() {
                                return Err(SanitizationError::new(
                                    "invalid_task_terminal",
                                    "scheduling_disposition",
                                ));
                            }
                            disposition.validate()?;
                        }
                    }
                    (TaskOutcome::Failure | TaskOutcome::Cancelled, Some(code)) => {
                        if scheduling_disposition.is_some() {
                            return Err(SanitizationError::new(
                                "invalid_task_terminal",
                                "scheduling_disposition",
                            ));
                        }
                        validate_task_semantic_label(code, "failure_code")?;
                    }
                    _ => {
                        return Err(SanitizationError::new(
                            "invalid_task_terminal",
                            "failure_code",
                        ));
                    }
                }
            }
            Self::TerminalRejected { reason, .. } => {
                validate_task_semantic_label(reason, "terminal_rejection_reason")?;
            }
        }
        Ok(())
    }

    fn diagnostic_code(&self) -> Option<DiagnosticCode> {
        match self {
            Self::TerminalCommitted {
                outcome: TaskOutcome::Failure | TaskOutcome::Cancelled,
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
                outcome: TaskOutcome::Success,
                ..
            } => Some(EffectDisposition::Performed),
            Self::TerminalCommitted {
                outcome: TaskOutcome::Failure | TaskOutcome::Cancelled,
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
        Err(SanitizationError::new("invalid_task_semantic_label", field))
    } else {
        Ok(())
    }
}

fn validate_task_package_sha256(value: &str) -> Result<(), SanitizationError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        Ok(())
    } else {
        Err(SanitizationError::new(
            "invalid_task_package_fingerprint",
            "package_sha256",
        ))
    }
}

fn validate_task_frame_dimensions(width: u32, height: u32) -> Result<(), SanitizationError> {
    if width == 0 || height == 0 {
        Err(SanitizationError::new(
            "invalid_task_frame_dimensions",
            "frame_dimensions",
        ))
    } else {
        Ok(())
    }
}

fn validate_task_candidate_pages(pages: &[String]) -> Result<(), SanitizationError> {
    if pages.is_empty() || pages.len() > 1_024 {
        return Err(SanitizationError::new(
            "invalid_task_candidate_pages",
            "candidate_pages",
        ));
    }
    for (index, page) in pages.iter().enumerate() {
        validate_task_semantic_label(page, "candidate_pages")?;
        if pages[..index].contains(page) {
            return Err(SanitizationError::new(
                "invalid_task_candidate_pages",
                "candidate_pages",
            ));
        }
    }
    Ok(())
}

fn validate_task_step(step_index: u32) -> Result<(), SanitizationError> {
    if step_index > 1_000 {
        Err(SanitizationError::new(
            "invalid_task_step_index",
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
    #[serde(skip_serializing_if = "Option::is_none")]
    segment_index: Option<u64>,
    affected_bytes: u64,
    audit: SanitizedAudit,
}

trait PayloadDetail {
    fn action(&self) -> EventAction;
    fn diagnostic_code(&self) -> Option<DiagnosticCode>;
    fn effect_disposition(&self) -> Option<EffectDisposition>;
    fn audit(&self) -> &SanitizedAudit;

    fn diagnostic_detail(&self) -> Option<&DiagnosticDetailRecord> {
        None
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
common_detail_accessors!(DiagnosticPayload);
common_detail_accessors!(OutcomePayload);
common_detail_accessors!(DiagnosticOutcomePayload);
common_detail_accessors!(RuntimeLifecyclePayload);
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
}

impl PerformanceSummaryPayload {
    pub const fn context(&self) -> &PerformanceContext {
        &self.context
    }
}

impl PerformanceMonitorStatePayload {
    pub const fn observed_at_unix_ms(&self) -> u64 {
        self.observed_at_unix_ms
    }

    pub const fn health(&self) -> PerformanceMonitorHealth {
        self.health
    }

    pub fn failure_code(&self) -> Option<&str> {
        self.failure_code.as_deref()
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

    pub const fn deadline_disposition(&self) -> Option<PerformanceDeadlineDisposition> {
        self.deadline_disposition
    }
}

impl FactPublishedPayload {
    pub fn record(&self) -> &FactRecord {
        self.record.as_ref()
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

    pub fn failure_code(&self) -> Option<&str> {
        self.failure_code.as_deref()
    }

    pub fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }
}

impl DiagnosticPayload {
    pub const fn diagnostic_code(&self) -> DiagnosticCode {
        self.diagnostic_code
    }
}

impl OutcomePayload {
    pub const fn effect_disposition(&self) -> EffectDisposition {
        self.effect_disposition
    }
}

impl DiagnosticOutcomePayload {
    pub const fn diagnostic_code(&self) -> DiagnosticCode {
        self.diagnostic_code
    }

    pub const fn effect_disposition(&self) -> EffectDisposition {
        self.effect_disposition
    }

    pub const fn detail(&self) -> Option<&DiagnosticDetailRecord> {
        self.detail.as_ref()
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

impl PayloadDetail for DiagnosticOutcomePayload {
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
}

impl PayloadDetail for MonitorOutcomePayload {
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
}

impl PayloadDetail for PolicyPlanningSignalPayload {
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
    audit: AuditInput,
}

struct InputIntentDraft {
    action: EventAction,
    execution_plan: Option<InputExecutionPlanRecord>,
    audit: AuditInput,
}

struct DiagnosticDraft {
    action: EventAction,
    diagnostic_code: DiagnosticCode,
    audit: AuditInput,
}

struct OutcomeDraft {
    action: EventAction,
    effect_disposition: EffectDisposition,
    audit: AuditInput,
}

struct DiagnosticOutcomeDraft {
    action: EventAction,
    diagnostic_code: DiagnosticCode,
    effect_disposition: EffectDisposition,
    detail: Option<DiagnosticDetailDraft>,
    audit: AuditInput,
}

struct MonitorOutcomeDraft {
    action: EventAction,
    effect_disposition: EffectDisposition,
    observation: MonitorObservation,
    decision: MonitorDecision,
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
    failure_code: Option<String>,
    audit: AuditInput,
}

struct PolicyDispatchDraft {
    data: PolicyDispatchEventData,
    admission: Option<Box<PolicyAdmissionRecord>>,
    diagnostic_code: Option<DiagnosticCode>,
    effect_disposition: Option<EffectDisposition>,
    audit: AuditInput,
}

struct PolicyExecutionDraft {
    data: PolicyExecutionEventData,
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

struct FactPublishedDraft {
    record: Box<FactRecord>,
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
            self.failure_code.as_deref(),
        )?;
        Ok(ResourceAuthoringPayload {
            phase: self.phase,
            draft_id: self.draft_id,
            target_label: self.target_label,
            target_fingerprint: self.target_fingerprint,
            changed_paths: self.changed_paths,
            failure_code: self.failure_code,
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
        Ok(PolicyDispatchPayload {
            action: EventAction::PolicyDispatch,
            diagnostic_code: self.diagnostic_code,
            effect_disposition: self.effect_disposition,
            decision_id: self.data.decision_id,
            task_id: self.data.task_id,
            instance_id: self.data.instance_id,
            operation_id: self.data.operation_id,
            package_digest: self.data.package_digest,
            procedure_binding_digest: self.data.procedure_binding_digest,
            reason_chain_id: self.data.reason_chain_id,
            reasons: self.data.reasons,
            catalog_hash: self.data.catalog_hash,
            catalog_version: self.data.catalog_version,
            input_ledger_position: self.data.input_ledger_position,
            fact_snapshot_id: self.data.fact_snapshot_id,
            approval_fact_ids: self.data.approval_fact_ids,
            urgency_milli: self.data.urgency_milli,
            admission: self.admission,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl PolicyExecutionDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<PolicyExecutionPayload, SanitizationError> {
        validate_policy_execution_data(&self.data)?;
        Ok(PolicyExecutionPayload {
            action: EventAction::PolicyExecution,
            decision_id: self.data.decision_id,
            task_id: self.data.task_id,
            instance_id: self.data.instance_id,
            observed_at_unix_ms: self.data.observed_at_unix_ms,
            outcome: self.data.outcome,
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
            || self.data.pressure.last_observed_at_unix_ms != self.data.observed_at_unix_ms
        {
            return Err(SanitizationError::new(
                "invalid_performance_pressure_time",
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
            failure_code: self.data.failure_code,
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
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl FactPublishedDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<FactPublishedPayload, SanitizationError> {
        self.record.validate()?;
        Ok(FactPublishedPayload {
            action: EventAction::FactPublish,
            record: self.record,
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
            fact_code: self.data.fact_code,
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
    failure_code: Option<&str>,
) -> Result<(), SanitizationError> {
    validate_resource_authoring_token(draft_id, 128, "draft_id")?;
    validate_resource_authoring_token(target_label, 128, "target_label")?;
    if target_fingerprint.len() != 64
        || !target_fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(SanitizationError::new(
            "invalid_resource_authoring_fingerprint",
            "target_fingerprint",
        ));
    }
    if changed_paths.is_empty() || changed_paths.len() > 4_096 {
        return Err(SanitizationError::new(
            "invalid_resource_authoring_paths",
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
                "invalid_resource_authoring_path",
                "changed_paths",
            ));
        }
    }
    match (phase, failure_code) {
        (ResourceAuthoringPhase::PromoteFailed, Some(code)) => {
            validate_resource_authoring_token(code, 128, "failure_code")?;
        }
        (ResourceAuthoringPhase::PromoteFailed, None) => {
            return Err(SanitizationError::new(
                "missing_resource_authoring_failure_code",
                "failure_code",
            ));
        }
        (_, Some(_)) => {
            return Err(SanitizationError::new(
                "unexpected_resource_authoring_failure_code",
                "failure_code",
            ));
        }
        (_, None) => {}
    }
    Ok(())
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
            "invalid_resource_authoring_token",
            field,
        ));
    }
    Ok(())
}

fn validate_policy_dispatch_data(data: &PolicyDispatchEventData) -> Result<(), SanitizationError> {
    validate_policy_token(&data.decision_id, "decision_id")?;
    validate_policy_token(&data.task_id, "task_id")?;
    validate_policy_token(&data.instance_id, "instance_id")?;
    validate_policy_token(&data.operation_id, "operation_id")?;
    validate_policy_digest(&data.package_digest, "package_digest")?;
    validate_policy_digest(&data.procedure_binding_digest, "procedure_binding_digest")?;
    validate_policy_token(&data.reason_chain_id, "reason_chain_id")?;
    validate_policy_token(&data.fact_snapshot_id, "fact_snapshot_id")?;
    validate_catalog_hash(&data.catalog_hash, "catalog_hash")?;
    if data.catalog_version == 0 || data.input_ledger_position == 0 || data.urgency_milli > 1_000 {
        return Err(SanitizationError::new(
            "invalid_policy_dispatch_position",
            "catalog_version_or_input_position",
        ));
    }
    if data.reasons.is_empty() || data.reasons.len() > 128 {
        return Err(SanitizationError::new(
            "invalid_policy_reason_chain",
            "reasons",
        ));
    }
    for reason in &data.reasons {
        validate_policy_token(&reason.code, "reason_code")?;
        validate_policy_text(&reason.detail, "reason_detail")?;
    }
    if data.approval_fact_ids.len() > 64 {
        return Err(SanitizationError::new(
            "invalid_policy_approval_facts",
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
            "invalid_policy_activity_sample",
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
            "invalid_policy_budget_receipt",
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
    validate_policy_token(&data.instance_id, "instance_id")?;
    if data.observed_at_unix_ms == 0 {
        return Err(SanitizationError::new(
            "invalid_policy_execution_time",
            "observed_at_unix_ms",
        ));
    }
    if let PolicyExecutionOutcome::Failed { failure } = &data.outcome {
        validate_policy_token(&failure.error_code, "error_code")?;
        failure.perf_context.validate()?;
        let retry_scheduled = failure.disposition == PolicyFailureDisposition::RetryScheduled;
        if failure.consecutive_same_error == 0
            || failure.escalation_streak > failure.consecutive_same_error
            || (!failure.performance_tax_exempt && failure.escalation_streak == 0)
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
                && (failure.error_code != "policy_runtime_budget_exceeded"
                    || failure.original_class != PolicyFailureClass::Severe
                    || failure.effective_class != PolicyFailureClass::Severe)
            || failure.sensitive
                && (failure.effective_class != PolicyFailureClass::Severe
                    || failure.disposition != PolicyFailureDisposition::PausedTask)
        {
            return Err(SanitizationError::new(
                "invalid_policy_failure_record",
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
    validate_policy_token(&data.instance_id, "instance_id")?;
    if let Some(task_id) = &data.task_id {
        validate_policy_token(task_id, "task_id")?;
    }
    validate_policy_token(&data.fact_code, "fact_code")?;
    if data.observed_at_unix_ms == 0 {
        return Err(SanitizationError::new(
            "invalid_policy_planning_signal_time",
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
            "invalid_policy_detection_budget_presence",
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
                "invalid_policy_detection_budget",
                "detection_budget",
            ));
        }
    }
    Ok(())
}

fn validate_performance_payload(payload: &PerformancePayload) -> Result<(), SanitizationError> {
    match payload {
        PerformancePayload::PressureStarted(value)
            if value.action == EventAction::PerformanceObserve =>
        {
            if value.observed_at_unix_ms == 0
                || value.pressure.last_observed_at_unix_ms != value.observed_at_unix_ms
                || value.pressure.started_at_unix_ms != value.observed_at_unix_ms
            {
                return Err(SanitizationError::new(
                    "invalid_performance_pressure_time",
                    "performance_pressure",
                ));
            }
            value.pressure.validate()
        }
        PerformancePayload::PressureEnded(value)
            if value.action == EventAction::PerformanceObserve =>
        {
            if value.observed_at_unix_ms == 0
                || value.pressure.last_observed_at_unix_ms != value.observed_at_unix_ms
                || value.pressure.started_at_unix_ms >= value.observed_at_unix_ms
            {
                return Err(SanitizationError::new(
                    "invalid_performance_pressure_time",
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
            })
        }
        PerformancePayload::MonitorDegraded(value)
            if value.action == EventAction::PerformanceObserve
                && value.health == PerformanceMonitorHealth::Degraded
                && value.failure_code.is_some() =>
        {
            validate_performance_monitor_state(&PerformanceMonitorStateEventData {
                observed_at_unix_ms: value.observed_at_unix_ms,
                health: value.health,
                failure_code: value.failure_code.clone(),
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
                && value.failure_code.is_none()
                && value.consecutive_failures == 0
                && !value.terminal =>
        {
            validate_performance_monitor_state(&PerformanceMonitorStateEventData {
                observed_at_unix_ms: value.observed_at_unix_ms,
                health: value.health,
                failure_code: None,
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
            })
        }
        _ => Err(SanitizationError::new(
            "invalid_performance_payload",
            "performance_payload",
        )),
    }
}

fn validate_fact_payload(payload: &FactPayload) -> Result<(), SanitizationError> {
    match payload {
        FactPayload::Published(value) if value.action == EventAction::FactPublish => {
            value.record.validate()
        }
        FactPayload::Invalidated(value) if value.action == EventAction::FactInvalidate => {
            validate_fact_invalidation(&value.invalidation)
        }
        _ => Err(SanitizationError::new(
            "invalid_fact_payload",
            "fact_payload",
        )),
    }
}

fn validate_client_action_payload(payload: &ClientPayload) -> Result<(), SanitizationError> {
    if let ClientPayload::Action(value) = payload {
        if value.action == EventAction::ClientAction {
            return value.record.validate();
        }
        return Err(SanitizationError::new(
            "invalid_client_action_payload",
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
            "invalid_approval_payload",
            "approval_payload",
        )),
    }
}

fn validate_policy_payload(payload: &PolicyPayload) -> Result<(), SanitizationError> {
    let dispatch = match payload {
        PolicyPayload::DispatchIntent(value)
            if value.action == EventAction::PolicyDispatch
                && value.diagnostic_code.is_none()
                && value.effect_disposition.is_none()
                && value.admission.is_none() =>
        {
            Some(value)
        }
        PolicyPayload::DispatchAdmitted(value)
            if value.action == EventAction::PolicyDispatch
                && value.diagnostic_code.is_none()
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
                && value.effect_disposition == Some(EffectDisposition::Performed)
                && value.admission.is_some() =>
        {
            Some(value)
        }
        PolicyPayload::ExecutionRecorded(value) if value.action == EventAction::PolicyExecution => {
            validate_policy_execution_data(&PolicyExecutionEventData {
                decision_id: value.decision_id.clone(),
                task_id: value.task_id.clone(),
                instance_id: value.instance_id.clone(),
                observed_at_unix_ms: value.observed_at_unix_ms,
                outcome: value.outcome.clone(),
            })?;
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
                fact_code: value.fact_code.clone(),
                observed_at_unix_ms: value.observed_at_unix_ms,
                detection_budget: value.detection_budget.clone(),
            })?;
            None
        }
        _ => {
            return Err(SanitizationError::new(
                "invalid_policy_payload",
                "policy_payload",
            ));
        }
    };
    let Some(value) = dispatch else {
        return Ok(());
    };
    validate_policy_dispatch_data(&PolicyDispatchEventData {
        decision_id: value.decision_id.clone(),
        task_id: value.task_id.clone(),
        instance_id: value.instance_id.clone(),
        operation_id: value.operation_id.clone(),
        package_digest: value.package_digest.clone(),
        procedure_binding_digest: value.procedure_binding_digest.clone(),
        reason_chain_id: value.reason_chain_id.clone(),
        reasons: value.reasons.clone(),
        catalog_hash: value.catalog_hash.clone(),
        catalog_version: value.catalog_version,
        input_ledger_position: value.input_ledger_position,
        fact_snapshot_id: value.fact_snapshot_id.clone(),
        approval_fact_ids: value.approval_fact_ids.clone(),
        urgency_milli: value.urgency_milli,
    })?;
    if let Some(admission) = &value.admission {
        validate_policy_admission(admission)?;
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
            "invalid_catalog_version",
            "catalog_version",
        ));
    }
    if let Some(previous) = &data.previous_catalog_hash {
        validate_catalog_hash(previous, "previous_catalog_hash")?;
        if previous == &data.catalog_hash {
            return Err(SanitizationError::new(
                "invalid_catalog_transition",
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
                "invalid_catalog_transition_lifecycle",
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
                    "invalid_release_stage_lifecycle",
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
            "invalid_release_transition_lifecycle",
            "release_payload",
        )),
    }
}

fn validate_agent_payload(payload: &AgentPayload) -> Result<(), SanitizationError> {
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
            "invalid_agent_session_lifecycle",
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
        Err(SanitizationError::new("invalid_policy_token", field))
    } else {
        Ok(())
    }
}

fn validate_policy_text(value: &str, field: &'static str) -> Result<(), SanitizationError> {
    if value.is_empty() || value.len() > 1_024 || value.chars().any(char::is_control) {
        Err(SanitizationError::new("invalid_policy_text", field))
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
        Err(SanitizationError::new("invalid_policy_digest", field))
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
        Err(SanitizationError::new("invalid_catalog_hash", field))
    }
}

impl ObservationDraft {
    fn new(action: EventAction, audit: AuditInput) -> Self {
        Self { action, audit }
    }

    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<ObservationPayload, SanitizationError> {
        Ok(ObservationPayload {
            action: self.action,
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
            audit,
        }
    }

    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<InputIntentPayload, SanitizationError> {
        if self.execution_plan.is_some() && self.action != EventAction::InputSwipe {
            return Err(SanitizationError::new(
                "invalid_input_execution_plan",
                "action",
            ));
        }
        if let Some(execution_plan) = &self.execution_plan {
            execution_plan.validate()?;
        }
        Ok(InputIntentPayload {
            action: self.action,
            execution_plan: self.execution_plan,
            audit: self.audit.sanitize(fingerprinter)?,
        })
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
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl OutcomeDraft {
    fn new(action: EventAction, effect_disposition: EffectDisposition, audit: AuditInput) -> Self {
        Self {
            action,
            effect_disposition,
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
            detail: None,
            audit,
        }
    }

    fn new_with_detail(
        action: EventAction,
        diagnostic_code: DiagnosticCode,
        effect_disposition: EffectDisposition,
        detail: DiagnosticDetailDraft,
        audit: AuditInput,
    ) -> Self {
        Self {
            action,
            diagnostic_code,
            effect_disposition,
            detail: Some(detail),
            audit,
        }
    }

    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<DiagnosticOutcomePayload, SanitizationError> {
        Ok(DiagnosticOutcomePayload {
            action: self.action,
            diagnostic_code: self.diagnostic_code,
            effect_disposition: self.effect_disposition,
            detail: self
                .detail
                .map(DiagnosticDetailDraft::sanitize)
                .transpose()?,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

impl MonitorOutcomeDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<MonitorOutcomePayload, SanitizationError> {
        self.observation
            .validate()
            .map_err(|_| SanitizationError::new("invalid_monitor_outcome", "observation"))?;
        self.decision
            .validate()
            .map_err(|_| SanitizationError::new("invalid_monitor_outcome", "decision"))?;
        if self.observation.diagnosis() != self.decision.diagnosis() {
            return Err(SanitizationError::new(
                "invalid_monitor_outcome",
                "diagnosis",
            ));
        }
        Ok(MonitorOutcomePayload {
            action: self.action,
            effect_disposition: self.effect_disposition,
            observation: self.observation,
            decision: self.decision,
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
                "invalid_monitor_recovery_coordination",
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
                "invalid_scheduler_queue",
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
                "invalid_lease_transfer",
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
        audit: AuditInput,
    ) -> Self {
        Self {
            action,
            effect_disposition,
            frame_width,
            frame_height,
            recognition_verdict,
            audit,
        }
    }

    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<ObservationResultPayload, SanitizationError> {
        if self.frame_width == 0 || self.frame_height == 0 {
            return Err(SanitizationError::new(
                "invalid_sanitized_payload",
                "frame_dimensions",
            ));
        }
        Ok(ObservationResultPayload {
            action: self.action,
            effect_disposition: self.effect_disposition,
            frame_width: self.frame_width,
            frame_height: self.frame_height,
            recognition_verdict: self.recognition_verdict,
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
                "invalid_capture_pressure",
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
        if self.duplicate_count == 0 || self.duration_ms == 0 {
            return Err(SanitizationError::new(
                "invalid_capture_dedup_window",
                "duplicate_count",
            ));
        }
        Ok(CaptureDedupWindowPayload {
            action: self.action,
            duplicate_count: self.duplicate_count,
            duration_ms: self.duration_ms,
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
                "invalid_capture_policy",
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
                "invalid_artifact_export",
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
        Ok(RecoveryPayload {
            reason: self.reason,
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
}

struct RuntimeLifecycleDraft {
    owner_epoch: OwnerEpoch,
    phase: RuntimeLifecyclePhase,
    audit: AuditInput,
}

impl RuntimeLifecycleDraft {
    fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<RuntimeLifecyclePayload, SanitizationError> {
        Ok(RuntimeLifecyclePayload {
            action: EventAction::RuntimeAction,
            owner_epoch: self.owner_epoch,
            phase: self.phase,
            audit: self.audit.sanitize(fingerprinter)?,
        })
    }
}

pub struct RuntimePayloadDraft(RuntimeDraftKind);

impl RuntimePayloadDraft {
    pub fn lifecycle_observed(
        owner_epoch: OwnerEpoch,
        phase: RuntimeLifecyclePhase,
        audit: AuditInput,
    ) -> Self {
        Self(RuntimeDraftKind::LifecycleObserved(RuntimeLifecycleDraft {
            owner_epoch,
            phase,
            audit,
        }))
    }

    pub fn started(action: EventAction, audit: AuditInput) -> Self {
        Self(RuntimeDraftKind::Started(ObservationDraft::new(
            action, audit,
        )))
    }

    pub fn takeover(action: EventAction, audit: AuditInput) -> Self {
        Self(RuntimeDraftKind::Takeover(ObservationDraft::new(
            action, audit,
        )))
    }

    pub fn failed(
        diagnostic_code: DiagnosticCode,
        effect: EffectDisposition,
        detail: DiagnosticDetailDraft,
        audit: AuditInput,
    ) -> Self {
        Self(RuntimeDraftKind::Failed(
            DiagnosticOutcomeDraft::new_with_detail(
                EventAction::RuntimeAction,
                diagnostic_code,
                effect,
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
    pub fn received(action: EventAction, audit: AuditInput) -> Self {
        Self(CommandDraftKind::Received(ObservationDraft::new(
            action, audit,
        )))
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

    pub fn failed_with_detail(
        action: EventAction,
        diagnostic_code: DiagnosticCode,
        effect: EffectDisposition,
        detail: DiagnosticDetailDraft,
        audit: AuditInput,
    ) -> Self {
        Self(InputDraftKind::Failed(
            DiagnosticOutcomeDraft::new_with_detail(action, diagnostic_code, effect, detail, audit),
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

    pub fn failed_with_detail(
        action: EventAction,
        diagnostic_code: DiagnosticCode,
        effect: EffectDisposition,
        detail: DiagnosticDetailDraft,
        audit: AuditInput,
    ) -> Self {
        Self(CaptureDraftKind::Failed(
            DiagnosticOutcomeDraft::new_with_detail(action, diagnostic_code, effect, detail, audit),
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
    Created(OutcomeDraft),
    Verified(OutcomeDraft),
    StoreFailed(DiagnosticOutcomeDraft),
    VerificationFailed(DiagnosticOutcomeDraft),
    ExportCompleted(ArtifactExportDraft),
    ExportFailed(ArtifactExportFailureDraft),
}

pub struct ArtifactPayloadDraft(ArtifactDraftKind);

impl ArtifactPayloadDraft {
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
        failure_code: Option<String>,
        audit: AuditInput,
    ) -> Self {
        Self(ResourceAuthoringDraft {
            phase,
            draft_id: draft_id.into(),
            target_label: target_label.into(),
            target_fingerprint: target_fingerprint.into(),
            changed_paths,
            failure_code,
            audit,
        })
    }
}

enum LedgerDraftKind {
    Recovered(RecoveryDraft),
}

pub struct LedgerPayloadDraft(LedgerDraftKind);

impl LedgerPayloadDraft {
    pub fn recovered(
        reason: RecoveryReason,
        segment_index: Option<u64>,
        affected_bytes: u64,
        audit: AuditInput,
    ) -> Self {
        Self(LedgerDraftKind::Recovered(RecoveryDraft {
            reason,
            segment_index,
            affected_bytes,
            audit,
        }))
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
            audit,
        }))
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
            diagnostic_code: None,
            effect_disposition: None,
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
            diagnostic_code: None,
            effect_disposition: Some(EffectDisposition::Performed),
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
            diagnostic_code: Some(DiagnosticCode::PolicyRejected),
            effect_disposition: Some(effect_disposition),
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
            diagnostic_code: None,
            effect_disposition: Some(EffectDisposition::Performed),
            audit,
        }))
    }

    pub fn execution_recorded(data: PolicyExecutionEventData, audit: AuditInput) -> Self {
        Self(PolicyDraftKind::Execution(PolicyExecutionDraft {
            data,
            audit,
        }))
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
        }
    }

    fn detail(&self) -> &dyn PayloadDetail {
        match self {
            Self::PressureStarted(detail) | Self::PressureEnded(detail) => detail,
            Self::StutterDetected(detail) => detail,
            Self::Summary(detail) => detail.as_ref(),
            Self::MonitorDegraded(detail) | Self::MonitorRecovered(detail) => detail,
            Self::BalanceChanged(detail) => detail,
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
family_payload!(ArtifactPayload, {
    Created => EventType::ArtifactCreated,
    Verified => EventType::ArtifactVerified,
    StoreFailed => EventType::ArtifactStoreFailed,
    VerificationFailed => EventType::ArtifactVerificationFailed,
    ExportCompleted => EventType::ArtifactExportCompleted,
    ExportFailed => EventType::ArtifactExportFailed,
});
family_payload!(ClientPayload, {
    Action => EventType::ClientAction,
    UiAction => EventType::UiAction,
    CliCommand => EventType::CliCommand,
    LabRequest => EventType::LabRequest,
});
family_payload!(LedgerPayload, {
    Recovered => EventType::LedgerRecovered,
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "family",
    content = "payload",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum EventPayload {
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
            }),
            Self::Ledger(value) => EventPayload::Ledger(match value.0 {
                LedgerDraftKind::Recovered(detail) => {
                    LedgerPayload::Recovered(detail.sanitize(fingerprinter)?)
                }
            }),
        })
    }
}

impl EventPayload {
    pub fn event_type(&self) -> EventType {
        self.family_payload().event_type()
    }

    pub fn family(&self) -> EventFamily {
        self.event_type().family()
    }

    pub fn schema(&self) -> &'static str {
        match self {
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
            Self::Artifact(_) => ARTIFACT_PAYLOAD_SCHEMA,
            Self::ResourceAuthoring(_) => RESOURCE_AUTHORING_PAYLOAD_SCHEMA,
            Self::Client(_) => CLIENT_PAYLOAD_SCHEMA,
            Self::Ledger(_) => LEDGER_PAYLOAD_SCHEMA,
        }
    }

    pub fn sensitivity(&self) -> Sensitivity {
        let detail = self.family_payload().detail();
        let mut sensitivity = detail.audit().sensitivity();
        if detail.diagnostic_code().is_some() {
            sensitivity = sensitivity.max(Sensitivity::Internal);
        }
        if let Some(diagnostic_detail) = detail.diagnostic_detail() {
            sensitivity = sensitivity.max(diagnostic_detail.declared_sensitivity());
        }
        if matches!(
            self,
            Self::Performance(_) | Self::Runtime(RuntimePayload::LifecycleObserved(_))
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
            Self::Approval(_) | Self::Client(ClientPayload::Action(_))
        ) {
            sensitivity = sensitivity.max(Sensitivity::Internal);
        }
        if matches!(self, Self::State(_) | Self::Release(_)) {
            sensitivity = sensitivity.max(Sensitivity::Internal);
        }
        if matches!(self, Self::Agent(_)) {
            sensitivity = sensitivity.max(Sensitivity::Internal);
        }
        sensitivity
    }

    pub fn action(&self) -> EventAction {
        self.family_payload().detail().action()
    }

    pub fn effect_disposition(&self) -> Option<EffectDisposition> {
        self.family_payload().detail().effect_disposition()
    }

    pub fn validate(&self) -> Result<(), SanitizationError> {
        let detail = self.family_payload().detail();
        detail.audit().validate()?;
        if let Some(diagnostic_detail) = detail.diagnostic_detail() {
            diagnostic_detail.validate()?;
        }
        if let Self::Runtime(RuntimePayload::LifecycleObserved(value)) = self
            && value.action != EventAction::RuntimeAction
        {
            return Err(SanitizationError::new(
                "invalid_runtime_lifecycle_action",
                "runtime_payload",
            ));
        }
        if let Self::ResourceAuthoring(value) = self {
            validate_resource_authoring_fields(
                value.phase,
                &value.draft_id,
                &value.target_label,
                &value.target_fingerprint,
                &value.changed_paths,
                value.failure_code.as_deref(),
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
                    "invalid_state_migration_lifecycle",
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
        if let Self::Input(InputPayload::Intent(intent)) = self {
            intent.validate()?;
        }
        match self {
            Self::Task(TaskPayload::Semantic(value)) if value.validate().is_err() => {
                return Err(SanitizationError::new(
                    "invalid_task_semantic_payload",
                    "task_semantic_fact",
                ));
            }
            Self::Ledger(LedgerPayload::Recovered(recovery))
                if recovery.segment_index == Some(0) =>
            {
                return Err(SanitizationError::new(
                    "invalid_sanitized_payload",
                    "segment_index",
                ));
            }
            Self::Capture(CapturePayload::Completed(result))
            | Self::Recognition(RecognitionPayload::Completed(result))
                if result.frame_width == 0 || result.frame_height == 0 =>
            {
                return Err(SanitizationError::new(
                    "invalid_sanitized_payload",
                    "frame_dimensions",
                ));
            }
            Self::Monitor(MonitorPayload::Completed(value))
                if value.observation.validate().is_err()
                    || value.decision.validate().is_err()
                    || value.observation.diagnosis() != value.decision.diagnosis() =>
            {
                return Err(SanitizationError::new(
                    "invalid_monitor_outcome",
                    "diagnosis",
                ));
            }
            Self::Monitor(MonitorPayload::RecoveryAdmitted(value))
                if value.action != EventAction::MonitorRecovery
                    || value.effect_disposition != EffectDisposition::NotPerformed
                    || value.reason != MonitorRecoveryCoordinationReason::SchedulerAvailable =>
            {
                return Err(SanitizationError::new(
                    "invalid_monitor_recovery_coordination",
                    "reason",
                ));
            }
            Self::Monitor(MonitorPayload::RecoveryDeferred(value))
                if value.action != EventAction::MonitorRecovery
                    || value.effect_disposition != EffectDisposition::NotPerformed
                    || value.reason == MonitorRecoveryCoordinationReason::SchedulerAvailable =>
            {
                return Err(SanitizationError::new(
                    "invalid_monitor_recovery_coordination",
                    "reason",
                ));
            }
            Self::Capture(CapturePayload::PressureChanged(value))
                if value.memory_budget_bytes == 0 =>
            {
                return Err(SanitizationError::new(
                    "invalid_capture_pressure",
                    "memory_budget_bytes",
                ));
            }
            Self::Capture(CapturePayload::DedupWindow(value))
                if value.duplicate_count == 0 || value.duration_ms == 0 =>
            {
                return Err(SanitizationError::new(
                    "invalid_capture_dedup_window",
                    "duplicate_count",
                ));
            }
            Self::Capture(CapturePayload::PolicyChanged(value)) if value.cadence_ms == 0 => {
                return Err(SanitizationError::new(
                    "invalid_capture_policy",
                    "cadence_ms",
                ));
            }
            Self::Capture(CapturePayload::SummaryCommitted(value))
                if value.action != EventAction::CaptureSummaryCommit
                    || value.summary.validate().is_err() =>
            {
                return Err(SanitizationError::new("invalid_capture_summary", "summary"));
            }
            Self::Artifact(ArtifactPayload::ExportCompleted(value))
                if value.artifact_count == 0 =>
            {
                return Err(SanitizationError::new(
                    "invalid_artifact_export",
                    "artifact_count",
                ));
            }
            _ => {}
        }
        Ok(())
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
            event_type,
            action: detail.action(),
            effect_disposition: detail.effect_disposition(),
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
            cadence_ms: capture_policy(self).map(CapturePolicyPayload::cadence_ms),
            retention_class: capture_policy(self).map(CapturePolicyPayload::retention_class),
            capture_policy_reason: capture_policy(self).map(CapturePolicyPayload::reason),
            task_outcome: artifact_export(self).map(|value| value.0),
            task_semantic_fact: task_semantic_fact(self).cloned().map(Box::new),
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
            failure_code: authoring.and_then(|value| value.failure_code.clone()),
            decision_id: policy_dispatch.map(|value| value.decision_id.clone().into_boxed_str()),
            reason_chain_id: policy_dispatch
                .map(|value| value.reason_chain_id.clone().into_boxed_str()),
            reason_count: policy_dispatch.map(|value| value.reasons.len() as u64),
            input_ledger_position: policy_dispatch.map(|value| value.input_ledger_position),
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
            policy_execution_outcome: policy_execution.map(|value| Box::new(value.outcome.clone())),
            policy_signal_id: policy_signal.map(|value| value.signal_id.clone().into_boxed_str()),
            policy_signal_kind: policy_signal.map(|value| value.kind),
            policy_signal_fact_code: policy_signal
                .map(|value| value.fact_code.clone().into_boxed_str()),
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
    let crate::FactContent::Artifact { artifact } = &value.record().content else {
        return Sensitivity::Internal;
    };
    match artifact.redaction_state() {
        ArtifactRedactionState::Pending => Sensitivity::Secret,
        ArtifactRedactionState::Applied => Sensitivity::Sensitive,
        ArtifactRedactionState::NotRequired => Sensitivity::Internal,
    }
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
    event_type: EventType,
    action: EventAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    effect_disposition: Option<EffectDisposition>,
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
    failure_code: Option<String>,
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
    policy_signal_fact_code: Option<Box<str>>,
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

    pub fn failure_code(&self) -> Option<&str> {
        self.failure_code.as_deref()
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

    pub fn policy_signal_fact_code(&self) -> Option<&str> {
        self.policy_signal_fact_code.as_deref()
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
            "invalid_fingerprint",
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
