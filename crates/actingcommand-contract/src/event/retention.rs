// SPDX-License-Identifier: AGPL-3.0-only

use super::{
    ArtifactKind, CorrelationId, InstanceId, LeaseId, OwnerEpoch, ProjectedArtifactReference,
    RequestId, RunId, SanitizationError,
};
use crate::TerminalEvent;
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactEvictionIntentRecord {
    pub identity: ArtifactRetentionIdentity,
    pub verified: TerminalEvent,
    pub success: TerminalEvent,
    pub close: TerminalEvent,
    pub capture_summary: Option<TerminalEvent>,
    pub settlement: Option<TerminalEvent>,
    pub through_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactEvictionIo {
    pub kind: String,
    pub raw_os_error: Option<i32>,
    pub message: String,
    pub message_truncated: bool,
}

impl ArtifactEvictionIo {
    pub fn from_io(error: &std::io::Error) -> Self {
        let message = error.to_string();
        let mut chars = message.chars();
        let bounded = chars.by_ref().take(256).collect();
        Self {
            kind: format!("{:?}", error.kind()),
            raw_os_error: error.raw_os_error(),
            message: bounded,
            message_truncated: chars.next().is_some(),
        }
    }
    pub fn validate(&self) -> Result<(), SanitizationError> {
        if self.kind.is_empty()
            || self.kind.len() > 64
            || !self.kind.bytes().all(|b| b.is_ascii_alphanumeric())
            || self.message.chars().count() > 256
        {
            return Err(invalid("io"));
        }
        Ok(())
    }
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
    pub io: Option<ArtifactEvictionIo>,
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
                for source in [&value.verified, &value.success, &value.close]
                    .into_iter()
                    .chain(value.capture_summary.as_ref())
                    .chain(value.settlement.as_ref())
                {
                    event(source)?;
                    if source.sequence > value.through_sequence {
                        return Err(invalid("intent_snapshot"));
                    }
                }
                if value.verified.sequence > value.success.sequence
                    || value.success.sequence >= value.close.sequence
                {
                    return Err(invalid("success_close_order"));
                }
                Ok(())
            }
            Self::EvictionOutcome(value) => {
                event(&value.intent)?;
                if (value.disposition == ArtifactEvictionDisposition::Failed) != value.io.is_some()
                {
                    return Err(invalid("outcome_io"));
                }
                if let Some(io) = &value.io {
                    io.validate()?;
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
    pub outcome: Option<TerminalEvent>,
    pub disposition: Option<ArtifactEvictionDisposition>,
    pub through_sequence: u64,
}

/// Read-time source positions for the original artifact metadata in a projected event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactEvictionObservation {
    pub artifact_id: super::ArtifactId,
    pub intent: TerminalEvent,
    pub outcome: Option<TerminalEvent>,
    pub disposition: Option<ArtifactEvictionDisposition>,
    pub through_sequence: u64,
}

impl ArtifactEvictionObservation {
    pub fn validate(&self) -> Result<(), SanitizationError> {
        event(&self.intent)?;
        if self.intent.sequence > self.through_sequence
            || self.outcome.is_some() != self.disposition.is_some()
        {
            return Err(invalid("observation"));
        }
        if let Some(outcome) = &self.outcome {
            event(outcome)?;
            if outcome.sequence <= self.intent.sequence || outcome.sequence > self.through_sequence
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
        let outcome = self
            .outcome
            .as_ref()
            .filter(|outcome| outcome.sequence <= through_sequence)
            .cloned();
        Some(ArtifactEvictionObservation {
            artifact_id: self.identity.artifact.artifact_id,
            intent: self.intent.clone(),
            disposition: outcome.as_ref().and(self.disposition),
            outcome,
            through_sequence,
        })
    }
    pub fn validate(&self) -> Result<(), SanitizationError> {
        self.identity.validate()?;
        event(&self.intent)?;
        if self.intent.sequence > self.through_sequence
            || self.outcome.is_some() != self.disposition.is_some()
        {
            return Err(invalid("proof"));
        }
        if let Some(outcome) = &self.outcome {
            event(outcome)?;
            if outcome.sequence <= self.intent.sequence || outcome.sequence > self.through_sequence
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
    SanitizationError::new("invalid_artifact_retention", field)
}
