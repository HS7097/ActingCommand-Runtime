// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::ArtifactEvictionProof;

/// A read projection is excluded from historical fact identity and serialization.
#[derive(Debug, Clone, Default)]
pub(crate) struct ArtifactRetentionReadState(pub(crate) Vec<ArtifactEvictionProof>);

impl PartialEq for ArtifactRetentionReadState {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}
impl Eq for ArtifactRetentionReadState {}

/// Original Ledger metadata. This value is not a material verification capability.
#[derive(Debug, Clone, Serialize)]
pub struct LedgerArtifactReference {
    #[serde(flatten)]
    reference: ProjectedArtifactReference,
    #[serde(skip)]
    availability: ArtifactAvailability,
}

impl PartialEq for LedgerArtifactReference {
    fn eq(&self, other: &Self) -> bool {
        self.reference == other.reference
    }
}

impl Eq for LedgerArtifactReference {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactAvailability {
    Unrecorded,
    Available(VerifiedArtifactReference),
    Evicted(Box<ArtifactEvictionProof>),
    PendingEviction(Box<ArtifactEvictionProof>),
    FailedEviction(Box<ArtifactEvictionProof>),
}

impl LedgerArtifactReference {
    pub(crate) fn apply_retention_proof(&mut self, proof: &ArtifactEvictionProof) {
        match proof.disposition {
            Some(
                actingcommand_contract::ArtifactEvictionDisposition::Deleted
                | actingcommand_contract::ArtifactEvictionDisposition::RecoveryAbsent,
            ) => {
                self.availability = ArtifactAvailability::Evicted(Box::new(proof.clone()));
            }
            None => {
                self.availability = ArtifactAvailability::PendingEviction(Box::new(proof.clone()));
            }
            Some(actingcommand_contract::ArtifactEvictionDisposition::Failed) => {
                self.availability = ArtifactAvailability::FailedEviction(Box::new(proof.clone()));
            }
        }
    }
    pub(crate) fn recorded(
        reference: ProjectedArtifactReference,
    ) -> Result<Self, FactValidationError> {
        reference.validate().map_err(|_| FactValidationError {
            code: "invalid_artifact_reference",
        })?;
        if reference.object_key.is_none() {
            return Err(FactValidationError {
                code: "artifact_object_key_missing",
            });
        }
        Ok(Self {
            reference,
            availability: ArtifactAvailability::Unrecorded,
        })
    }

    pub(crate) fn from_issued(reference: &ArtifactReference) -> Self {
        Self {
            reference: reference.project(true),
            availability: ArtifactAvailability::Unrecorded,
        }
    }

    pub(crate) fn with_availability(mut self, availability: ArtifactAvailability) -> Self {
        self.availability = availability;
        self
    }

    pub(crate) fn restored(
        reference: ProjectedArtifactReference,
        availability: ArtifactAvailability,
    ) -> Result<Self, FactValidationError> {
        if let ArtifactAvailability::Available(verified) = &availability
            && verified.reference().project(true) != reference
        {
            return Err(FactValidationError::new(
                "artifact_store_verification_mismatch",
            ));
        }
        let valid = match &availability {
            ArtifactAvailability::Available(verified) => {
                verified.reference().project(true) == reference
            }
            ArtifactAvailability::Evicted(proof) => {
                proof.identity.artifact == reference
                    && proof.validate().is_ok()
                    && matches!(
                    proof.disposition,
                    Some(
                        actingcommand_contract::ArtifactEvictionDisposition::Deleted
                            | actingcommand_contract::ArtifactEvictionDisposition::RecoveryAbsent
                    )
                )
            }
            ArtifactAvailability::PendingEviction(proof) => {
                proof.identity.artifact == reference
                    && proof.validate().is_ok()
                    && proof.outcome.is_none()
            }
            ArtifactAvailability::Unrecorded | ArtifactAvailability::FailedEviction(_) => false,
        };
        if !valid {
            return Err(FactValidationError {
                code: "artifact_availability_proof_invalid",
            });
        }
        Ok(Self::recorded(reference)?.with_availability(availability))
    }

    pub fn availability(&self) -> &ArtifactAvailability {
        &self.availability
    }
    pub const fn artifact_id(&self) -> &ArtifactId {
        &self.reference.artifact_id
    }
    pub const fn kind(&self) -> ArtifactKind {
        self.reference.kind
    }
    pub const fn run_id(&self) -> Option<&RunId> {
        self.reference.run_id.as_ref()
    }
    pub const fn frame_id(&self) -> Option<&FrameId> {
        self.reference.frame_id.as_ref()
    }
    pub const fn correlation_id(&self) -> Option<&CorrelationId> {
        self.reference.correlation_id.as_ref()
    }
    pub fn object_key(&self) -> &str {
        self.reference
            .object_key
            .as_deref()
            .expect("validated Ledger artifact key")
    }
    pub const fn media_type(&self) -> ArtifactMediaType {
        self.reference.media_type
    }
    pub const fn byte_count(&self) -> u64 {
        self.reference.byte_count
    }
    pub fn sha256(&self) -> &str {
        &self.reference.sha256
    }
    pub const fn created_at_unix_ms(&self) -> u64 {
        self.reference.created_at_unix_ms
    }
    pub const fn producer(&self) -> ArtifactProducer {
        self.reference.producer
    }
    pub const fn retention_class(&self) -> RetentionClass {
        self.reference.retention_class
    }
    pub const fn redaction_state(&self) -> ArtifactRedactionState {
        self.reference.redaction_state
    }
    pub fn project(&self, include_object_key: bool) -> ProjectedArtifactReference {
        let mut reference = self.reference.clone();
        if !include_object_key {
            reference.object_key = None;
        }
        reference
    }
}

impl PartialEq<ArtifactReference> for LedgerArtifactReference {
    fn eq(&self, other: &ArtifactReference) -> bool {
        self.reference == other.project(true)
    }
}
