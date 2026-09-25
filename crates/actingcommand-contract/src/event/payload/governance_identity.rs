// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::{GovernanceIdentityCard, GovernanceIdentityRefusal};

/// Where a governance connection came from. The Runtime only binds a loopback address, so
/// every declaration it records is from this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GovernancePeer {
    Loopback,
}

/// The Runtime's verdict on one governance identity card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum GovernanceIdentityVerdict {
    Accepted,
    Refused { code: GovernanceIdentityRefusal },
}

/// `governance.identity_declared` (Workflow #318 cfg4): one governance identity card as the
/// connection declared it, the peer and the Runtime's verdict. The event origin carries the
/// declaring request's actor and source; accepted and refused declarations are both recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GovernanceIdentityDeclaredPayload {
    card: GovernanceIdentityCard,
    peer: GovernancePeer,
    verdict: GovernanceIdentityVerdict,
    audit: SanitizedAudit,
}

impl GovernanceIdentityDeclaredPayload {
    pub const fn card(&self) -> &GovernanceIdentityCard {
        &self.card
    }

    pub const fn peer(&self) -> GovernancePeer {
        self.peer
    }

    pub const fn verdict(&self) -> GovernanceIdentityVerdict {
        self.verdict
    }

    pub(super) fn validate(&self) -> Result<(), SanitizationError> {
        self.card.validate().map_err(|_| {
            SanitizationError::new("invalid_governance_identity_card", "governance_identity")
        })
    }
}

impl PayloadDetail for GovernanceIdentityDeclaredPayload {
    fn action(&self) -> EventAction {
        EventAction::GovernanceIdentityDeclare
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

pub(super) struct GovernanceIdentityDeclaredDraft {
    card: GovernanceIdentityCard,
    peer: GovernancePeer,
    verdict: GovernanceIdentityVerdict,
    audit: AuditInput,
}

impl ClientPayloadDraft {
    /// One declared governance identity card with the Runtime's verdict.
    pub fn governance_identity_declared(
        card: GovernanceIdentityCard,
        peer: GovernancePeer,
        verdict: GovernanceIdentityVerdict,
        audit: AuditInput,
    ) -> Self {
        Self(ClientDraftKind::GovernanceIdentityDeclared(
            GovernanceIdentityDeclaredDraft {
                card,
                peer,
                verdict,
                audit,
            },
        ))
    }
}

impl GovernanceIdentityDeclaredDraft {
    pub(super) fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<GovernanceIdentityDeclaredPayload, SanitizationError> {
        let payload = GovernanceIdentityDeclaredPayload {
            card: self.card,
            peer: self.peer,
            verdict: self.verdict,
            audit: self.audit.sanitize(fingerprinter)?,
        };
        payload.validate()?;
        Ok(payload)
    }
}
