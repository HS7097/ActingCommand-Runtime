// SPDX-License-Identifier: AGPL-3.0-only

use super::{EventFamily, EventType, SanitizationError, Sensitivity, StaticCode};
use serde::de;
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;

pub const COMMAND_PAYLOAD_SCHEMA: &str = "actingcommand.payload.command.v2";
pub const SCHEDULER_PAYLOAD_SCHEMA: &str = "actingcommand.payload.scheduler.v2";
pub const LEASE_PAYLOAD_SCHEMA: &str = "actingcommand.payload.lease.v2";
pub const TASK_PAYLOAD_SCHEMA: &str = "actingcommand.payload.task.v2";
pub const INPUT_PAYLOAD_SCHEMA: &str = "actingcommand.payload.input.v2";
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
    action: StaticCode,
    audit: SanitizedAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticPayload {
    action: StaticCode,
    diagnostic_code: StaticCode,
    audit: SanitizedAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutcomePayload {
    action: StaticCode,
    effect_disposition: EffectDisposition,
    audit: SanitizedAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticOutcomePayload {
    action: StaticCode,
    diagnostic_code: StaticCode,
    effect_disposition: EffectDisposition,
    audit: SanitizedAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryPayload {
    reason: StaticCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    segment_index: Option<u64>,
    affected_bytes: u64,
    audit: SanitizedAudit,
}

trait PayloadDetail {
    fn action(&self) -> &StaticCode;
    fn diagnostic_code(&self) -> Option<&StaticCode>;
    fn effect_disposition(&self) -> Option<EffectDisposition>;
    fn audit(&self) -> &SanitizedAudit;
}

macro_rules! common_detail_accessors {
    ($type:ty) => {
        impl $type {
            pub fn action(&self) -> &StaticCode {
                &self.action
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

impl DiagnosticPayload {
    pub fn diagnostic_code(&self) -> &StaticCode {
        &self.diagnostic_code
    }
}

impl OutcomePayload {
    pub const fn effect_disposition(&self) -> EffectDisposition {
        self.effect_disposition
    }
}

impl DiagnosticOutcomePayload {
    pub fn diagnostic_code(&self) -> &StaticCode {
        &self.diagnostic_code
    }

    pub const fn effect_disposition(&self) -> EffectDisposition {
        self.effect_disposition
    }
}

impl PayloadDetail for ObservationPayload {
    fn action(&self) -> &StaticCode {
        &self.action
    }

    fn diagnostic_code(&self) -> Option<&StaticCode> {
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
    fn action(&self) -> &StaticCode {
        &self.action
    }

    fn diagnostic_code(&self) -> Option<&StaticCode> {
        Some(&self.diagnostic_code)
    }

    fn effect_disposition(&self) -> Option<EffectDisposition> {
        None
    }

    fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }
}

impl PayloadDetail for OutcomePayload {
    fn action(&self) -> &StaticCode {
        &self.action
    }

    fn diagnostic_code(&self) -> Option<&StaticCode> {
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
    fn action(&self) -> &StaticCode {
        &self.action
    }

    fn diagnostic_code(&self) -> Option<&StaticCode> {
        Some(&self.diagnostic_code)
    }

    fn effect_disposition(&self) -> Option<EffectDisposition> {
        Some(self.effect_disposition)
    }

    fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }
}

impl RecoveryPayload {
    pub fn reason(&self) -> &StaticCode {
        &self.reason
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
    fn action(&self) -> &StaticCode {
        &self.reason
    }

    fn diagnostic_code(&self) -> Option<&StaticCode> {
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
    action: StaticCode,
    audit: AuditInput,
}

struct DiagnosticDraft {
    action: StaticCode,
    diagnostic_code: StaticCode,
    audit: AuditInput,
}

struct OutcomeDraft {
    action: StaticCode,
    effect_disposition: EffectDisposition,
    audit: AuditInput,
}

struct DiagnosticOutcomeDraft {
    action: StaticCode,
    diagnostic_code: StaticCode,
    effect_disposition: EffectDisposition,
    audit: AuditInput,
}

struct RecoveryDraft {
    reason: StaticCode,
    segment_index: Option<u64>,
    affected_bytes: u64,
    audit: AuditInput,
}

impl ObservationDraft {
    fn new(action: StaticCode, audit: AuditInput) -> Self {
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
    fn new(action: StaticCode, diagnostic_code: StaticCode, audit: AuditInput) -> Self {
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
    fn new(action: StaticCode, effect_disposition: EffectDisposition, audit: AuditInput) -> Self {
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
        action: StaticCode,
        diagnostic_code: StaticCode,
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

pub struct CommandPayloadDraft(CommandDraftKind);

impl CommandPayloadDraft {
    pub fn received(action: StaticCode, audit: AuditInput) -> Self {
        Self(CommandDraftKind::Received(ObservationDraft::new(
            action, audit,
        )))
    }

    pub fn validated(action: StaticCode, effect: EffectDisposition, audit: AuditInput) -> Self {
        Self(CommandDraftKind::Validated(OutcomeDraft::new(
            action, effect, audit,
        )))
    }

    pub fn rejected(
        action: StaticCode,
        diagnostic_code: StaticCode,
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
    pub fn admitted(action: StaticCode, audit: AuditInput) -> Self {
        Self(SchedulerDraftKind::Admitted(ObservationDraft::new(
            action, audit,
        )))
    }

    pub fn queued(action: StaticCode, audit: AuditInput) -> Self {
        Self(SchedulerDraftKind::Queued(ObservationDraft::new(
            action, audit,
        )))
    }

    pub fn denied(action: StaticCode, diagnostic_code: StaticCode, audit: AuditInput) -> Self {
        Self(SchedulerDraftKind::Denied(DiagnosticDraft::new(
            action,
            diagnostic_code,
            audit,
        )))
    }

    pub fn preempted(action: StaticCode, diagnostic_code: StaticCode, audit: AuditInput) -> Self {
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
    Released(OutcomeDraft),
    Expired(OutcomeDraft),
    TransitionIntent(ObservationDraft),
    TransitionFailed(DiagnosticOutcomeDraft),
}

pub struct LeasePayloadDraft(LeaseDraftKind);

impl LeasePayloadDraft {
    pub fn requested(action: StaticCode, audit: AuditInput) -> Self {
        Self(LeaseDraftKind::Requested(ObservationDraft::new(
            action, audit,
        )))
    }

    pub fn granted(action: StaticCode, effect: EffectDisposition, audit: AuditInput) -> Self {
        Self(LeaseDraftKind::Granted(OutcomeDraft::new(
            action, effect, audit,
        )))
    }

    pub fn transferred(action: StaticCode, effect: EffectDisposition, audit: AuditInput) -> Self {
        Self(LeaseDraftKind::Transferred(OutcomeDraft::new(
            action, effect, audit,
        )))
    }

    pub fn released(action: StaticCode, effect: EffectDisposition, audit: AuditInput) -> Self {
        Self(LeaseDraftKind::Released(OutcomeDraft::new(
            action, effect, audit,
        )))
    }

    pub fn expired(action: StaticCode, effect: EffectDisposition, audit: AuditInput) -> Self {
        Self(LeaseDraftKind::Expired(OutcomeDraft::new(
            action, effect, audit,
        )))
    }

    pub fn transition_intent(action: StaticCode, audit: AuditInput) -> Self {
        Self(LeaseDraftKind::TransitionIntent(ObservationDraft::new(
            action, audit,
        )))
    }

    pub fn transition_failed(
        action: StaticCode,
        diagnostic_code: StaticCode,
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
    pub fn requested(action: StaticCode, audit: AuditInput) -> Self {
        Self(TaskDraftKind::Requested(ObservationDraft::new(
            action, audit,
        )))
    }

    pub fn started(action: StaticCode, audit: AuditInput) -> Self {
        Self(TaskDraftKind::Started(ObservationDraft::new(action, audit)))
    }

    pub fn step_started(action: StaticCode, audit: AuditInput) -> Self {
        Self(TaskDraftKind::StepStarted(ObservationDraft::new(
            action, audit,
        )))
    }

    pub fn step_finished(action: StaticCode, audit: AuditInput) -> Self {
        Self(TaskDraftKind::StepFinished(ObservationDraft::new(
            action, audit,
        )))
    }

    pub fn completed(action: StaticCode, effect: EffectDisposition, audit: AuditInput) -> Self {
        Self(TaskDraftKind::Completed(OutcomeDraft::new(
            action, effect, audit,
        )))
    }

    pub fn failed(
        action: StaticCode,
        diagnostic_code: StaticCode,
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

    pub fn cancelled(action: StaticCode, effect: EffectDisposition, audit: AuditInput) -> Self {
        Self(TaskDraftKind::Cancelled(OutcomeDraft::new(
            action, effect, audit,
        )))
    }

    pub fn terminal_intent(action: StaticCode, audit: AuditInput) -> Self {
        Self(TaskDraftKind::TerminalIntent(ObservationDraft::new(
            action, audit,
        )))
    }

    pub fn terminal_commit_failed(
        action: StaticCode,
        diagnostic_code: StaticCode,
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
    pub fn intent(action: StaticCode, audit: AuditInput) -> Self {
        Self(InputDraftKind::Intent(ObservationDraft::new(action, audit)))
    }

    pub fn committed(action: StaticCode, effect: EffectDisposition, audit: AuditInput) -> Self {
        Self(InputDraftKind::Committed(OutcomeDraft::new(
            action, effect, audit,
        )))
    }

    pub fn completed(action: StaticCode, audit: AuditInput) -> Self {
        Self(InputDraftKind::Completed(ObservationDraft::new(
            action, audit,
        )))
    }

    pub fn failed(
        action: StaticCode,
        diagnostic_code: StaticCode,
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

enum ClientDraftKind {
    UiAction(ObservationDraft),
    CliCommand(ObservationDraft),
    LabRequest(ObservationDraft),
}

pub struct ClientPayloadDraft(ClientDraftKind);

impl ClientPayloadDraft {
    pub fn ui_action(action: StaticCode, audit: AuditInput) -> Self {
        Self(ClientDraftKind::UiAction(ObservationDraft::new(
            action, audit,
        )))
    }

    pub fn cli_command(action: StaticCode, audit: AuditInput) -> Self {
        Self(ClientDraftKind::CliCommand(ObservationDraft::new(
            action, audit,
        )))
    }

    pub fn lab_request(action: StaticCode, audit: AuditInput) -> Self {
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
        reason: StaticCode,
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
    Command(CommandPayloadDraft),
    Scheduler(SchedulerPayloadDraft),
    Lease(LeasePayloadDraft),
    Task(TaskPayloadDraft),
    Input(InputPayloadDraft),
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
payload_draft_from!(SchedulerPayloadDraft, Scheduler);
payload_draft_from!(LeasePayloadDraft, Lease);
payload_draft_from!(TaskPayloadDraft, Task);
payload_draft_from!(InputPayloadDraft, Input);
payload_draft_from!(ClientPayloadDraft, Client);
payload_draft_from!(LedgerPayloadDraft, Ledger);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum CommandPayload {
    Received(ObservationPayload),
    Validated(OutcomePayload),
    Rejected(DiagnosticOutcomePayload),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum SchedulerPayload {
    Admitted(ObservationPayload),
    Queued(ObservationPayload),
    Denied(DiagnosticPayload),
    Preempted(DiagnosticPayload),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum LeasePayload {
    Requested(ObservationPayload),
    Granted(OutcomePayload),
    Transferred(OutcomePayload),
    Released(OutcomePayload),
    Expired(OutcomePayload),
    TransitionIntent(ObservationPayload),
    TransitionFailed(DiagnosticOutcomePayload),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
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
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum InputPayload {
    Intent(ObservationPayload),
    Committed(OutcomePayload),
    Completed(ObservationPayload),
    Failed(DiagnosticOutcomePayload),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum ClientPayload {
    UiAction(ObservationPayload),
    CliCommand(ObservationPayload),
    LabRequest(ObservationPayload),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
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
family_payload!(ClientPayload, {
    UiAction => EventType::UiAction,
    CliCommand => EventType::CliCommand,
    LabRequest => EventType::LabRequest,
});
family_payload!(LedgerPayload, {
    Recovered => EventType::LedgerRecovered,
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "family", content = "payload", rename_all = "snake_case")]
pub enum EventPayload {
    Command(CommandPayload),
    Scheduler(SchedulerPayload),
    Lease(LeasePayload),
    Task(TaskPayload),
    Input(InputPayload),
    Client(ClientPayload),
    Ledger(LedgerPayload),
}

impl EventPayloadDraft {
    pub(crate) fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<EventPayload, SanitizationError> {
        Ok(match self {
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
            Self::Command(_) => COMMAND_PAYLOAD_SCHEMA,
            Self::Scheduler(_) => SCHEDULER_PAYLOAD_SCHEMA,
            Self::Lease(_) => LEASE_PAYLOAD_SCHEMA,
            Self::Task(_) => TASK_PAYLOAD_SCHEMA,
            Self::Input(_) => INPUT_PAYLOAD_SCHEMA,
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

    pub fn validate(&self) -> Result<(), SanitizationError> {
        let detail = self.family_payload().detail();
        detail.audit().validate()?;
        if let Self::Ledger(LedgerPayload::Recovered(recovery)) = self
            && recovery.segment_index == Some(0)
        {
            return Err(SanitizationError::new(
                "invalid_sanitized_payload",
                "segment_index",
            ));
        }
        Ok(())
    }

    pub fn public_projection(&self) -> PublicEventPayload {
        let event_type = self.event_type();
        let detail = self.family_payload().detail();
        let payload = PublicPayload {
            event_type,
            action: detail.action().clone(),
            effect_disposition: detail.effect_disposition(),
            segment_index: match self {
                Self::Ledger(LedgerPayload::Recovered(value)) => value.segment_index,
                _ => None,
            },
            affected_bytes: match self {
                Self::Ledger(LedgerPayload::Recovered(value)) => Some(value.affected_bytes),
                _ => None,
            },
        };
        match self {
            Self::Command(_) => PublicEventPayload::Command(payload),
            Self::Scheduler(_) => PublicEventPayload::Scheduler(payload),
            Self::Lease(_) => PublicEventPayload::Lease(payload),
            Self::Task(_) => PublicEventPayload::Task(payload),
            Self::Input(_) => PublicEventPayload::Input(payload),
            Self::Client(_) => PublicEventPayload::Client(payload),
            Self::Ledger(_) => PublicEventPayload::Ledger(payload),
        }
    }

    fn family_payload(&self) -> &dyn FamilyPayload {
        match self {
            Self::Command(value) => value,
            Self::Scheduler(value) => value,
            Self::Lease(value) => value,
            Self::Task(value) => value,
            Self::Input(value) => value,
            Self::Client(value) => value,
            Self::Ledger(value) => value,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicPayload {
    event_type: EventType,
    action: StaticCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    effect_disposition: Option<EffectDisposition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    segment_index: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    affected_bytes: Option<u64>,
}

impl PublicPayload {
    pub const fn event_type(&self) -> EventType {
        self.event_type
    }

    pub fn action(&self) -> &StaticCode {
        &self.action
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "family", content = "payload", rename_all = "snake_case")]
pub enum PublicEventPayload {
    Command(PublicPayload),
    Scheduler(PublicPayload),
    Lease(PublicPayload),
    Task(PublicPayload),
    Input(PublicPayload),
    Client(PublicPayload),
    Ledger(PublicPayload),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "detail", content = "payload", rename_all = "snake_case")]
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
