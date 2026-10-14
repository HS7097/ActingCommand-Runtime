// SPDX-License-Identifier: AGPL-3.0-only

use super::{
    ArtifactKind, CorrelationId, InstanceId, LeaseId, OwnerEpoch, ProjectedArtifactReference,
    RequestId, RunId, SanitizationError,
};
use crate::TerminalEvent;
use crate::codes::SanitizationCode;
use crate::outcome::{IoOp, Outcome};
use serde::{Deserialize, Serialize};

pub const FRAME_RETENTION_POLICY_VERSION: u16 = 1;
pub const FRAME_RETENTION_BACKTRACE: usize = 8;
pub const RETENTION_ROUND_OBJECTS: usize = 16;
pub const RETENTION_ROUND_BYTES: u64 = 64 * 1024 * 1024;
pub const RETENTION_ROUND_START_BUDGET_MS: u64 = 1_000;

/// Identity and originating authority; no field grants access to object bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRetentionIdentity {
    pub artifact: ProjectedArtifactReference,
    pub owner_epoch: OwnerEpoch,
    pub instance_id: InstanceId,
    pub request_id: RequestId,
    pub correlation_id: CorrelationId,
    pub run_id: Option<RunId>,
    pub lease_id: Option<LeaseId>,
    pub policy_version: u16,
}

impl ArtifactRetentionIdentity {
    pub fn validate(&self) -> Result<(), SanitizationError> {
        self.artifact.validate()?;
        if self.artifact.kind != ArtifactKind::CaptureFrame
            || self.artifact.object_key.is_none()
            || self.artifact.frame_id.is_none()
            || self.artifact.run_id != self.run_id
            || self.artifact.correlation_id != Some(self.correlation_id)
            || self.policy_version != FRAME_RETENTION_POLICY_VERSION
        {
            return Err(invalid("identity"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactPinReason {
    Lab,
    WarningOrHigher,
    DirectEvidence,
    Explicit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactPinRecord {
    pub identity: ArtifactRetentionIdentity,
    pub reason: ArtifactPinReason,
    pub trigger: TerminalEvent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactPinReleaseRecord {
    pub identity: ArtifactRetentionIdentity,
    pub pin: TerminalEvent,
    pub release: TerminalEvent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LabPinReleaseTarget {
    pub artifact_id: super::ArtifactId,
    pub pin: TerminalEvent,
}

impl LabPinReleaseTarget {
    pub fn validate(&self) -> Result<(), SanitizationError> {
        event(&self.pin)
    }
}

/// A durable release receipt proves only the named pin release, not eviction eligibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LabPinReleaseResult {
    pub identity: ArtifactRetentionIdentity,
    pub pin: TerminalEvent,
    pub release_request: TerminalEvent,
    pub released: TerminalEvent,
    pub already_released: bool,
}

impl LabPinReleaseResult {
    pub fn validate(&self) -> Result<(), SanitizationError> {
        self.identity.validate()?;
        for source in [&self.pin, &self.release_request, &self.released] {
            event(source)?;
        }
        if self.pin.sequence >= self.release_request.sequence
            || self.release_request.sequence >= self.released.sequence
        {
            return Err(invalid("lab_release_order"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactEvictionIntentRecord {
    pub identity: ArtifactRetentionIdentity,
    pub verified: TerminalEvent,
    /// The original success the intent is sourced from. Workflow #382 L-5b: the `failed_run`
    /// evidence of the retired K/T policy is no member any more, so a stored intent that still
    /// carries it does not decode and its replay refuses it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success: Option<TerminalEvent>,
    pub close: TerminalEvent,
    pub capture_summary: Option<TerminalEvent>,
    pub settlement: Option<TerminalEvent>,
    pub through_sequence: u64,
}

/// The `foreign_os_error` of a failed eviction's file removal (ledger revision 3, model
/// section 8.1; was `ArtifactEvictionIo {kind, raw_os_error, message, message_truncated}`).
pub fn artifact_eviction_io_outcome(error: &std::io::Error) -> Outcome {
    Outcome::from_io_error(error, IoOp::Remove)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactEvictionDisposition {
    Deleted,
    Failed,
    RecoveryAbsent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactEvictionOutcomeRecord {
    pub identity: ArtifactRetentionIdentity,
    pub intent: TerminalEvent,
    pub disposition: ArtifactEvictionDisposition,
    /// A failed eviction's I/O failure (ledger revision 3; was `io`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Outcome>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ArtifactRetentionFact {
    PinRecorded(ArtifactPinRecord),
    PinReleased(ArtifactPinReleaseRecord),
    EvictionIntent(ArtifactEvictionIntentRecord),
    EvictionOutcome(ArtifactEvictionOutcomeRecord),
}

impl ArtifactRetentionFact {
    pub fn identity(&self) -> &ArtifactRetentionIdentity {
        match self {
            Self::PinRecorded(value) => &value.identity,
            Self::PinReleased(value) => &value.identity,
            Self::EvictionIntent(value) => &value.identity,
            Self::EvictionOutcome(value) => &value.identity,
        }
    }

    pub fn validate(&self) -> Result<(), SanitizationError> {
        self.identity().validate()?;
        match self {
            Self::PinRecorded(value) => event(&value.trigger),
            Self::PinReleased(value) => {
                event(&value.pin)?;
                event(&value.release)?;
                if value.release.sequence <= value.pin.sequence {
                    return Err(invalid("pin_release_order"));
                }
                Ok(())
            }
            Self::EvictionIntent(value) => {
                let Some(terminal) = &value.success else {
                    return Err(invalid("intent_terminal"));
                };
                for source in [&value.verified, terminal, &value.close]
                    .into_iter()
                    .chain(value.capture_summary.as_ref())
                    .chain(value.settlement.as_ref())
                {
                    event(source)?;
                    if source.sequence > value.through_sequence {
                        return Err(invalid("intent_snapshot"));
                    }
                }
                if value.verified.sequence > terminal.sequence
                    || terminal.sequence >= value.close.sequence
                {
                    return Err(invalid("success_close_order"));
                }
                Ok(())
            }
            Self::EvictionOutcome(value) => {
                event(&value.intent)?;
                if (value.disposition == ArtifactEvictionDisposition::Failed)
                    != value.outcome.is_some()
                {
                    return Err(invalid("outcome_io"));
                }
                Ok(())
            }
        }
    }
}

/// A validated Ledger proof never substitutes for a material verification capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactEvictionProof {
    pub identity: ArtifactRetentionIdentity,
    pub intent: TerminalEvent,
    pub terminal_event: Option<TerminalEvent>,
    pub disposition: Option<ArtifactEvictionDisposition>,
    pub through_sequence: u64,
}

/// Read-time source positions for the original artifact metadata in a projected event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactEvictionObservation {
    pub artifact_id: super::ArtifactId,
    pub intent: TerminalEvent,
    pub terminal_event: Option<TerminalEvent>,
    pub disposition: Option<ArtifactEvictionDisposition>,
    pub through_sequence: u64,
}

impl ArtifactEvictionObservation {
    pub fn validate(&self) -> Result<(), SanitizationError> {
        event(&self.intent)?;
        if self.intent.sequence > self.through_sequence
            || self.terminal_event.is_some() != self.disposition.is_some()
        {
            return Err(invalid("observation"));
        }
        if let Some(terminal) = &self.terminal_event {
            event(terminal)?;
            if terminal.sequence <= self.intent.sequence
                || terminal.sequence > self.through_sequence
            {
                return Err(invalid("observation_order"));
            }
        }
        Ok(())
    }
}

impl ArtifactEvictionProof {
    pub fn observation(&self, through_sequence: u64) -> Option<ArtifactEvictionObservation> {
        let through_sequence = through_sequence.min(self.through_sequence);
        if self.intent.sequence > through_sequence {
            return None;
        }
        let terminal_event = self
            .terminal_event
            .as_ref()
            .filter(|terminal| terminal.sequence <= through_sequence)
            .cloned();
        Some(ArtifactEvictionObservation {
            artifact_id: self.identity.artifact.artifact_id,
            intent: self.intent,
            disposition: terminal_event.as_ref().and(self.disposition),
            terminal_event,
            through_sequence,
        })
    }
    pub fn validate(&self) -> Result<(), SanitizationError> {
        self.identity.validate()?;
        event(&self.intent)?;
        if self.intent.sequence > self.through_sequence
            || self.terminal_event.is_some() != self.disposition.is_some()
        {
            return Err(invalid("proof"));
        }
        if let Some(terminal) = &self.terminal_event {
            event(terminal)?;
            if terminal.sequence <= self.intent.sequence
                || terminal.sequence > self.through_sequence
            {
                return Err(invalid("proof_order"));
            }
        }
        Ok(())
    }
}

fn event(value: &TerminalEvent) -> Result<(), SanitizationError> {
    if value.sequence == 0 {
        Err(invalid("event_sequence"))
    } else {
        Ok(())
    }
}

fn invalid(field: &'static str) -> SanitizationError {
    SanitizationError::new(SanitizationCode::InvalidArtifactRetention, field)
}
