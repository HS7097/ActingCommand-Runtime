// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::codes::SharedCode;
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
            SanitizationError::new(
                SharedCode::InvalidGovernanceIdentityCard,
                "governance_identity",
            )
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

// Workflow #434 LAB-6S4a (0.12.62): no card is declared any more, so nothing writes this record;
// its draft and constructor went, and the records a ledger already holds decode and validate as
// before.
