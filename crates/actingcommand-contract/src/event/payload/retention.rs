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

impl ArtifactRetentionPayload {
    pub fn record(&self) -> &ArtifactRetentionFact {
        &self.record
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
