// SPDX-License-Identifier: AGPL-3.0-only

use super::{
    CapturePolicyReason, CapturePressureState, DiagnosticCode, EventAction, EventFamily, EventType,
    EvidenceCompleteness, PolicyFailureClass, PolicyFailureDisposition, PolicyPlanningSignalKind,
    RecognitionVerdict, RecoveryReason, ResourceAuthoringPhase, RetentionClass, SanitizationError,
    Sensitivity, TaskOutcome,
};
use crate::{
    HolderId, InputAction, LeaseId, LeasePriority, MonitorDecision, MonitorDiagnosis,
    MonitorDisposition, MonitorObservation, MonitorRecoveryCoordinationReason, MonitorRecoveryKind,
    RequestId,
};
use serde::de;
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;

pub const COMMAND_PAYLOAD_SCHEMA: &str = "actingcommand.payload.command.v2";
pub const RUNTIME_PAYLOAD_SCHEMA: &str = "actingcommand.payload.runtime.v1";
pub const MONITOR_PAYLOAD_SCHEMA: &str = "actingcommand.payload.monitor.v1";
pub const SCHEDULER_PAYLOAD_SCHEMA: &str = "actingcommand.payload.scheduler.v3";
pub const POLICY_PAYLOAD_SCHEMA: &str = "actingcommand.payload.policy.v1";
pub const CATALOG_PAYLOAD_SCHEMA: &str = "actingcommand.payload.catalog.v1";
pub const LEASE_PAYLOAD_SCHEMA: &str = "actingcommand.payload.lease.v3";
pub const TASK_PAYLOAD_SCHEMA: &str = "actingcommand.payload.task.v3";
pub const APPLICATION_PAYLOAD_SCHEMA: &str = "actingcommand.payload.application.v1";
pub const INPUT_PAYLOAD_SCHEMA: &str = "actingcommand.payload.input.v2";
pub const CAPTURE_PAYLOAD_SCHEMA: &str = "actingcommand.payload.capture.v1";
pub const RECOGNITION_PAYLOAD_SCHEMA: &str = "actingcommand.payload.recognition.v1";
pub const ARTIFACT_PAYLOAD_SCHEMA: &str = "actingcommand.payload.artifact.v1";
pub const RESOURCE_AUTHORING_PAYLOAD_SCHEMA: &str = "actingcommand.payload.resource_authoring.v1";
pub const CLIENT_PAYLOAD_SCHEMA: &str = "actingcommand.payload.client.v2";
pub const LEDGER_PAYLOAD_SCHEMA: &str = "actingcommand.payload.ledger.v2";

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
            machine_path: self.machine_path.map(|_| "[redacted]".to_string()),
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
                .is_some_and(|value| value != "[redacted]")
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
    audit: SanitizedAudit,
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
    pub reason_chain_id: String,
    pub reasons: Vec<PolicyReasonRecord>,
    pub catalog_hash: String,
    pub catalog_version: u64,
    pub input_ledger_position: u64,
    pub fact_snapshot_id: String,
    pub approval_fact_ids: Vec<String>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyFailureRecord {
    pub error_code: String,
    pub reported_success: bool,
    pub original_class: PolicyFailureClass,
    pub effective_class: PolicyFailureClass,
    pub consecutive_same_error: u16,
    pub retry_attempt: u16,
    pub disposition: PolicyFailureDisposition,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_at_unix_ms: Option<u64>,
    pub runtime_ms: u64,
    pub sensitive: bool,
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
    reason_chain_id: String,
    reasons: Vec<PolicyReasonRecord>,
    catalog_hash: String,
    catalog_version: u64,
    input_ledger_position: u64,
    fact_snapshot_id: String,
    approval_fact_ids: Vec<String>,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogTransitionEventData {
    pub catalog_id: String,
    pub catalog_version: u64,
    pub catalog_hash: String,
    pub previous_catalog_hash: Option<String>,
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TaskSemanticFact {
    PackageAdmitted {
        package_label: String,
        task_label: String,
        package_sha256: String,
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
    },
    TerminalRejected {
        committed_outcome: TaskOutcome,
        attempted_outcome: TaskOutcome,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSemanticPayload {
    action: EventAction,
    fact: TaskSemanticFact,
    audit: SanitizedAudit,
}

impl TaskSemanticPayload {
    pub const fn action(&self) -> EventAction {
        self.action
    }

    pub const fn fact(&self) -> &TaskSemanticFact {
        &self.fact
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
        self.fact.validate()
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
                    (TaskOutcome::Success, None) => {}
                    (TaskOutcome::Failure | TaskOutcome::Cancelled, Some(code)) => {
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
            | Self::TerminalRejected { .. } => Some(DiagnosticCode::RuntimeDiagnostic),
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
common_detail_accessors!(MonitorOutcomePayload);
common_detail_accessors!(MonitorRecoveryCoordinationPayload);
common_detail_accessors!(SchedulerQueuePayload);
common_detail_accessors!(SchedulerPreemptionPayload);
common_detail_accessors!(LeaseTransferPayload);
common_detail_accessors!(ObservationResultPayload);
common_detail_accessors!(CapturePressurePayload);
common_detail_accessors!(CaptureDedupWindowPayload);
common_detail_accessors!(CapturePolicyPayload);
common_detail_accessors!(ArtifactExportPayload);
common_detail_accessors!(ArtifactExportFailurePayload);

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

struct TaskSemanticDraft {
    fact: TaskSemanticFact,
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
        Ok(TaskSemanticPayload {
            action: EventAction::RuntimeTaskRun,
            fact,
            audit: self.audit.sanitize(fingerprinter)?,
        })
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
            reason_chain_id: self.data.reason_chain_id,
            reasons: self.data.reasons,
            catalog_hash: self.data.catalog_hash,
            catalog_version: self.data.catalog_version,
            input_ledger_position: self.data.input_ledger_position,
            fact_snapshot_id: self.data.fact_snapshot_id,
            approval_fact_ids: self.data.approval_fact_ids,
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
    validate_policy_token(&data.reason_chain_id, "reason_chain_id")?;
    validate_policy_token(&data.fact_snapshot_id, "fact_snapshot_id")?;
    validate_catalog_hash(&data.catalog_hash, "catalog_hash")?;
    if data.catalog_version == 0 || data.input_ledger_position == 0 {
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
        let retry_scheduled = failure.disposition == PolicyFailureDisposition::RetryScheduled;
        if failure.consecutive_same_error == 0
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
    Ok(())
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
        reason_chain_id: value.reason_chain_id.clone(),
        reasons: value.reasons.clone(),
        catalog_hash: value.catalog_hash.clone(),
        catalog_version: value.catalog_version,
        input_ledger_position: value.input_ledger_position,
        fact_snapshot_id: value.fact_snapshot_id.clone(),
        approval_fact_ids: value.approval_fact_ids.clone(),
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
    })
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
}

pub struct RuntimePayloadDraft(RuntimeDraftKind);

impl RuntimePayloadDraft {
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
        Self(TaskDraftKind::Semantic(TaskSemanticDraft { fact, audit }))
    }
}

enum InputDraftKind {
    Intent(ObservationDraft),
    Committed(OutcomeDraft),
    Completed(ObservationDraft),
    Failed(DiagnosticOutcomeDraft),
}

pub struct InputPayloadDraft(InputDraftKind);

impl InputPayloadDraft {
    pub fn intent(action: EventAction, audit: AuditInput) -> Self {
        Self(InputDraftKind::Intent(ObservationDraft::new(action, audit)))
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
    UiAction(ObservationDraft),
    CliCommand(ObservationDraft),
    LabRequest(ObservationDraft),
}

pub struct ClientPayloadDraft(ClientDraftKind);

impl ClientPayloadDraft {
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

pub enum EventPayloadDraft {
    Runtime(RuntimePayloadDraft),
    Monitor(MonitorPayloadDraft),
    Command(CommandPayloadDraft),
    Scheduler(SchedulerPayloadDraft),
    Policy(PolicyPayloadDraft),
    Catalog(CatalogPayloadDraft),
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
payload_draft_from!(SchedulerPayloadDraft, Scheduler);
payload_draft_from!(PolicyPayloadDraft, Policy);
payload_draft_from!(CatalogPayloadDraft, Catalog);
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
    Intent(ObservationPayload),
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
});
family_payload!(MonitorPayload, {
    Requested => EventType::MonitorProbeRequested,
    Started => EventType::MonitorProbeStarted,
    Completed => EventType::MonitorProbeCompleted,
    Failed => EventType::MonitorProbeFailed,
    RecoveryAdmitted => EventType::MonitorRecoveryAdmitted,
    RecoveryDeferred => EventType::MonitorRecoveryDeferred,
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
    Command(CommandPayload),
    Scheduler(SchedulerPayload),
    Policy(PolicyPayload),
    Catalog(CatalogPayload),
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
            Self::Command(_) => COMMAND_PAYLOAD_SCHEMA,
            Self::Scheduler(_) => SCHEDULER_PAYLOAD_SCHEMA,
            Self::Policy(_) => POLICY_PAYLOAD_SCHEMA,
            Self::Catalog(_) => CATALOG_PAYLOAD_SCHEMA,
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
        if let Self::Catalog(value) = self {
            validate_catalog_payload(value)?;
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
        };
        match self {
            Self::Runtime(_) => PublicEventPayload::Runtime(payload),
            Self::Monitor(_) => PublicEventPayload::Monitor(payload),
            Self::Command(_) => PublicEventPayload::Command(payload),
            Self::Scheduler(_) => PublicEventPayload::Scheduler(payload),
            Self::Policy(_) => PublicEventPayload::Policy(payload),
            Self::Catalog(_) => PublicEventPayload::Catalog(payload),
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
            Self::Command(value) => value,
            Self::Scheduler(value) => value,
            Self::Policy(value) => value,
            Self::Catalog(value) => value,
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
    Command(PublicPayload),
    Scheduler(PublicPayload),
    Policy(PublicPayload),
    Catalog(PublicPayload),
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
    Public(PublicEventPayload),
    Full(EventPayload),
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
