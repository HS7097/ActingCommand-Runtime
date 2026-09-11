// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::{ArtifactEvictionDisposition, ArtifactRetentionFact};

pub const ARTIFACT_RETENTION_PAYLOAD_SCHEMA: &str = "actingcommand.payload.artifact_retention.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRetentionPayload {
    record: ArtifactRetentionFact,
    audit: SanitizedAudit,
}

/// Public read data omits material paths and native IO messages; it grants no material capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRetentionPublicSummary {
    pub artifact: ProjectedArtifactReference,
    pub owner_epoch: OwnerEpoch,
    pub policy_version: u16,
    pub fact: ArtifactRetentionPublicFact,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ArtifactRetentionPublicFact {
    PinRecorded {
        reason: crate::ArtifactPinReason,
        trigger: crate::TerminalEvent,
    },
    PinReleased {
        pin: crate::TerminalEvent,
        release: crate::TerminalEvent,
    },
    EvictionIntent {
        verified: crate::TerminalEvent,
        success: crate::TerminalEvent,
        close: crate::TerminalEvent,
        capture_summary: Option<crate::TerminalEvent>,
        settlement: Option<crate::TerminalEvent>,
        through_sequence: u64,
    },
    EvictionOutcome {
        intent: crate::TerminalEvent,
        disposition: ArtifactEvictionDisposition,
        io_kind: Option<String>,
        raw_os_error: Option<i32>,
    },
}

impl ArtifactRetentionPayload {
    pub fn record(&self) -> &ArtifactRetentionFact {
        &self.record
    }

    pub(super) fn public_summary(&self) -> ArtifactRetentionPublicSummary {
        let mut artifact = self.record.identity().artifact.clone();
        artifact.object_key = None;
        let fact = match &self.record {
            ArtifactRetentionFact::PinRecorded(value) => ArtifactRetentionPublicFact::PinRecorded {
                reason: value.reason,
                trigger: value.trigger.clone(),
            },
            ArtifactRetentionFact::PinReleased(value) => ArtifactRetentionPublicFact::PinReleased {
                pin: value.pin.clone(),
                release: value.release.clone(),
            },
            ArtifactRetentionFact::EvictionIntent(value) => {
                ArtifactRetentionPublicFact::EvictionIntent {
                    verified: value.verified.clone(),
                    success: value.success.clone(),
                    close: value.close.clone(),
                    capture_summary: value.capture_summary.clone(),
                    settlement: value.settlement.clone(),
                    through_sequence: value.through_sequence,
                }
            }
            ArtifactRetentionFact::EvictionOutcome(value) => {
                ArtifactRetentionPublicFact::EvictionOutcome {
                    intent: value.intent.clone(),
                    disposition: value.disposition,
                    io_kind: value.io.as_ref().map(|io| io.kind.clone()),
                    raw_os_error: value.io.as_ref().and_then(|io| io.raw_os_error),
                }
            }
        };
        ArtifactRetentionPublicSummary {
            artifact,
            owner_epoch: self.record.identity().owner_epoch,
            policy_version: self.record.identity().policy_version,
            fact,
        }
    }

    pub(super) fn sensitivity(&self) -> Sensitivity {
        match self.record.identity().artifact.redaction_state {
            ArtifactRedactionState::Pending => Sensitivity::Secret,
            ArtifactRedactionState::Applied => Sensitivity::Sensitive,
            ArtifactRedactionState::NotRequired => {
                if matches!(&self.record, ArtifactRetentionFact::EvictionOutcome(value) if value.io.is_some())
                {
                    Sensitivity::Sensitive
                } else {
                    Sensitivity::Internal
                }
            }
        }
    }

    pub(super) fn sanitize(
        record: ArtifactRetentionFact,
        audit: AuditInput,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<Self, SanitizationError> {
        record.validate()?;
        Ok(Self {
            record,
            audit: audit.sanitize(fingerprinter)?,
        })
    }

    pub(super) fn event_type(&self) -> EventType {
        match self.record {
            ArtifactRetentionFact::PinRecorded(_) => EventType::ArtifactPinRecorded,
            ArtifactRetentionFact::PinReleased(_) => EventType::ArtifactPinReleased,
            ArtifactRetentionFact::EvictionIntent(_) => EventType::ArtifactEvictionIntent,
            ArtifactRetentionFact::EvictionOutcome(_) => EventType::ArtifactEvictionOutcome,
        }
    }
}

impl PayloadDetail for ArtifactRetentionPayload {
    fn action(&self) -> EventAction {
        EventAction::ArtifactRetention
    }
    fn diagnostic_code(&self) -> Option<DiagnosticCode> {
        match &self.record {
            ArtifactRetentionFact::EvictionOutcome(outcome)
                if outcome.disposition == ArtifactEvictionDisposition::Failed =>
            {
                Some(DiagnosticCode::ArtifactWriteFailed)
            }
            _ => None,
        }
    }
    fn effect_disposition(&self) -> Option<EffectDisposition> {
        match &self.record {
            ArtifactRetentionFact::EvictionOutcome(outcome) => match outcome.disposition {
                ArtifactEvictionDisposition::Deleted => Some(EffectDisposition::Performed),
                ArtifactEvictionDisposition::Failed => Some(EffectDisposition::Indeterminate),
                ArtifactEvictionDisposition::RecoveryAbsent => None,
            },
            _ => None,
        }
    }
    fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }
}

impl ArtifactPayloadDraft {
    pub fn retention(record: ArtifactRetentionFact, audit: AuditInput) -> Self {
        Self(ArtifactDraftKind::Retention(record, audit))
    }
}

impl EventPayload {
    pub fn artifact_retention(&self) -> Option<&ArtifactRetentionFact> {
        match self {
            Self::Artifact(ArtifactPayload::Retention(payload)) => Some(payload.record()),
            _ => None,
        }
    }
}
