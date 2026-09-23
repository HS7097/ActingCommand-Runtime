// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::OwnerUnlockActor;

/// `owner.unlock`: the operator confirmation recorded by the offline `actingd unlock-owner`
/// (`contracts/actingd-unlock-owner.md`). It is a `cli.command` fact, never an owner-epoch
/// record; the owner journal alone carries the close evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerUnlockPayload {
    owner_epoch: OwnerEpoch,
    previous_resource_disposition: OwnerResourceDisposition,
    actor: OwnerUnlockActor,
    audit: SanitizedAudit,
}

impl PayloadDetail for OwnerUnlockPayload {
    fn action(&self) -> EventAction {
        EventAction::OwnerUnlock
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

pub(super) struct OwnerUnlockDraft {
    owner_epoch: OwnerEpoch,
    previous_resource_disposition: OwnerResourceDisposition,
    actor: OwnerUnlockActor,
    audit: AuditInput,
}

impl ClientPayloadDraft {
    /// The unlocked epoch and the disposition its last journal record carried before.
    pub fn owner_unlock(
        owner_epoch: OwnerEpoch,
        previous_resource_disposition: OwnerResourceDisposition,
        actor: OwnerUnlockActor,
        audit: AuditInput,
    ) -> Self {
        Self(ClientDraftKind::OwnerUnlock(OwnerUnlockDraft {
            owner_epoch,
            previous_resource_disposition,
            actor,
            audit,
        }))
    }
}

impl OwnerUnlockDraft {
    pub(super) fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<OwnerUnlockPayload, SanitizationError> {
        let payload = OwnerUnlockPayload {
            owner_epoch: self.owner_epoch,
            previous_resource_disposition: self.previous_resource_disposition,
            actor: self.actor,
            audit: self.audit.sanitize(fingerprinter)?,
        };
        payload.validate()?;
        Ok(payload)
    }
}

impl OwnerUnlockPayload {
    pub(super) fn validate(&self) -> Result<(), SanitizationError> {
        if !matches!(
            self.previous_resource_disposition,
            OwnerResourceDisposition::InUse | OwnerResourceDisposition::Unconfirmed
        ) {
            return Err(SanitizationError::new(
                "invalid_owner_unlock_disposition",
                "owner_unlock",
            ));
        }
        self.actor.validate()
    }
}
