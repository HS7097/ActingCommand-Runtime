// SPDX-License-Identifier: AGPL-3.0-only

use super::{
    CapturePolicyReason, CapturePressureState, DiagnosticCode, EventAction, EventFamily, EventType,
    EvidenceCompleteness, RecognitionVerdict, RecoveryReason, RetentionClass, SanitizationError,
    Sensitivity, TaskOutcome,
};
use serde::de;
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;

pub const COMMAND_PAYLOAD_SCHEMA: &str = "actingcommand.payload.command.v2";
pub const RUNTIME_PAYLOAD_SCHEMA: &str = "actingcommand.payload.runtime.v1";
pub const SCHEDULER_PAYLOAD_SCHEMA: &str = "actingcommand.payload.scheduler.v2";
pub const LEASE_PAYLOAD_SCHEMA: &str = "actingcommand.payload.lease.v2";
pub const TASK_PAYLOAD_SCHEMA: &str = "actingcommand.payload.task.v2";
pub const INPUT_PAYLOAD_SCHEMA: &str = "actingcommand.payload.input.v2";
pub const CAPTURE_PAYLOAD_SCHEMA: &str = "actingcommand.payload.capture.v1";
pub const RECOGNITION_PAYLOAD_SCHEMA: &str = "actingcommand.payload.recognition.v1";
pub const ARTIFACT_PAYLOAD_SCHEMA: &str = "actingcommand.payload.artifact.v1";
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
common_detail_accessors!(ObservationResultPayload);
common_detail_accessors!(CapturePressurePayload);
common_detail_accessors!(CaptureDedupWindowPayload);
common_detail_accessors!(CapturePolicyPayload);
common_detail_accessors!(ArtifactExportPayload);
common_detail_accessors!(ArtifactExportFailurePayload);

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
        if self.memory_budget_bytes == 0 || self.resident_bytes > self.memory_budget_bytes {
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
    Queued(ObservationDraft),
    Denied(DiagnosticDraft),
    Preempted(DiagnosticDraft),
}

pub struct SchedulerPayloadDraft(SchedulerDraftKind);

impl SchedulerPayloadDraft {
    pub fn admitted(action: EventAction, audit: AuditInput) -> Self {
        Self(SchedulerDraftKind::Admitted(ObservationDraft::new(
            action, audit,
        )))
    }

    pub fn queued(action: EventAction, audit: AuditInput) -> Self {
        Self(SchedulerDraftKind::Queued(ObservationDraft::new(
            action, audit,
        )))
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
        diagnostic_code: DiagnosticCode,
        audit: AuditInput,
    ) -> Self {
        Self(SchedulerDraftKind::Preempted(DiagnosticDraft::new(
            action,
            diagnostic_code,
            audit,
        )))
    }
}

enum LeaseDraftKind {
    Requested(ObservationDraft),
    Granted(OutcomeDraft),
    Transferred(OutcomeDraft),
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

    pub fn transferred(action: EventAction, effect: EffectDisposition, audit: AuditInput) -> Self {
        Self(LeaseDraftKind::Transferred(OutcomeDraft::new(
            action, effect, audit,
        )))
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

pub enum EventPayloadDraft {
    Runtime(RuntimePayloadDraft),
    Command(CommandPayloadDraft),
    Scheduler(SchedulerPayloadDraft),
    Lease(LeasePayloadDraft),
    Task(TaskPayloadDraft),
    Input(InputPayloadDraft),
    Capture(CapturePayloadDraft),
    Recognition(RecognitionPayloadDraft),
    Artifact(ArtifactPayloadDraft),
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
payload_draft_from!(SchedulerPayloadDraft, Scheduler);
payload_draft_from!(LeasePayloadDraft, Lease);
payload_draft_from!(TaskPayloadDraft, Task);
payload_draft_from!(InputPayloadDraft, Input);
payload_draft_from!(CapturePayloadDraft, Capture);
payload_draft_from!(RecognitionPayloadDraft, Recognition);
payload_draft_from!(ArtifactPayloadDraft, Artifact);
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
pub enum SchedulerPayload {
    Admitted(ObservationPayload),
    Queued(ObservationPayload),
    Denied(DiagnosticPayload),
    Preempted(DiagnosticPayload),
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
    Transferred(OutcomePayload),
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
    ExportCompleted(ArtifactExportPayload),
    ExportFailed(ArtifactExportFailurePayload),
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
family_payload!(SchedulerPayload, {
    Admitted => EventType::SchedulerAdmitted,
    Queued => EventType::SchedulerQueued,
    Denied => EventType::SchedulerDenied,
    Preempted => EventType::SchedulerPreempted,
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
family_payload!(TaskPayload, {
    Requested => EventType::TaskRequested,
    Started => EventType::TaskStarted,
    StepStarted => EventType::TaskStepStarted,
    StepFinished => EventType::TaskStepFinished,
    Completed => EventType::TaskCompleted,
    Failed => EventType::TaskFailed,
    Cancelled => EventType::TaskCancelled,
    TerminalIntent => EventType::TaskTerminalIntent,
    TerminalCommitFailed => EventType::TaskTerminalCommitFailed,
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
    Command(CommandPayload),
    Scheduler(SchedulerPayload),
    Lease(LeasePayload),
    Task(TaskPayload),
    Input(InputPayload),
    Capture(CapturePayload),
    Recognition(RecognitionPayload),
    Artifact(ArtifactPayload),
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
                ArtifactDraftKind::ExportCompleted(detail) => {
                    ArtifactPayload::ExportCompleted(detail.sanitize(fingerprinter)?)
                }
                ArtifactDraftKind::ExportFailed(detail) => {
                    ArtifactPayload::ExportFailed(detail.sanitize(fingerprinter)?)
                }
            }),
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
            Self::Command(_) => COMMAND_PAYLOAD_SCHEMA,
            Self::Scheduler(_) => SCHEDULER_PAYLOAD_SCHEMA,
            Self::Lease(_) => LEASE_PAYLOAD_SCHEMA,
            Self::Task(_) => TASK_PAYLOAD_SCHEMA,
            Self::Input(_) => INPUT_PAYLOAD_SCHEMA,
            Self::Capture(_) => CAPTURE_PAYLOAD_SCHEMA,
            Self::Recognition(_) => RECOGNITION_PAYLOAD_SCHEMA,
            Self::Artifact(_) => ARTIFACT_PAYLOAD_SCHEMA,
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
        match self {
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
            Self::Capture(CapturePayload::PressureChanged(value))
                if value.memory_budget_bytes == 0
                    || value.resident_bytes > value.memory_budget_bytes =>
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
            evidence_completeness: artifact_export(self).map(|value| value.1),
            artifact_count: artifact_export(self).map(|value| value.2),
        };
        match self {
            Self::Runtime(_) => PublicEventPayload::Runtime(payload),
            Self::Command(_) => PublicEventPayload::Command(payload),
            Self::Scheduler(_) => PublicEventPayload::Scheduler(payload),
            Self::Lease(_) => PublicEventPayload::Lease(payload),
            Self::Task(_) => PublicEventPayload::Task(payload),
            Self::Input(_) => PublicEventPayload::Input(payload),
            Self::Capture(_) => PublicEventPayload::Capture(payload),
            Self::Recognition(_) => PublicEventPayload::Recognition(payload),
            Self::Artifact(_) => PublicEventPayload::Artifact(payload),
            Self::Client(_) => PublicEventPayload::Client(payload),
            Self::Ledger(_) => PublicEventPayload::Ledger(payload),
        }
    }

    fn family_payload(&self) -> &dyn FamilyPayload {
        match self {
            Self::Runtime(value) => value,
            Self::Command(value) => value,
            Self::Scheduler(value) => value,
            Self::Lease(value) => value,
            Self::Task(value) => value,
            Self::Input(value) => value,
            Self::Capture(value) => value,
            Self::Recognition(value) => value,
            Self::Artifact(value) => value,
            Self::Client(value) => value,
            Self::Ledger(value) => value,
        }
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
    evidence_completeness: Option<EvidenceCompleteness>,
    #[serde(skip_serializing_if = "Option::is_none")]
    artifact_count: Option<u64>,
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

    pub const fn evidence_completeness(&self) -> Option<EvidenceCompleteness> {
        self.evidence_completeness
    }

    pub const fn artifact_count(&self) -> Option<u64> {
        self.artifact_count
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
    Command(PublicPayload),
    Scheduler(PublicPayload),
    Lease(PublicPayload),
    Task(PublicPayload),
    Input(PublicPayload),
    Capture(PublicPayload),
    Recognition(PublicPayload),
    Artifact(PublicPayload),
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
