// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #375 H1: within one opening that verifies material (the candidate and Segment
//! openings), each distinct artifact reference without an eviction proof is verified exactly
//! once, before the records are restored. Restore then applies the verdicts in sequence
//! order, so the first failing reference keeps its error and position. Workflow #375 R5a:
//! the formal openings (the writer open and every `ledger-maintenance` read) read no
//! material at all; only their timing is kept here.

use super::RetentionIndex;
use crate::fact::StoredEventRecord;
use crate::global::GlobalLedgerResult;
use actingcommand_contract::{ProjectedArtifactReference, VerifiedArtifactReference};
use std::collections::HashMap;
use std::time::Instant;

/// Timings of one complete formal opening (never persisted). The startup writer open prints
/// them on one stdout line, and a failed `ledger-maintenance verify` reports them in its
/// failure cause. A phase that did not run stays 0. Workflow #375 R5a: a formal opening
/// reads no artifact material, so `artifacts`, `artifact_bytes`, `workers` and
/// `material_ms` are always 0; they stay in the line so that its format is unchanged.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LedgerOpenTiming {
    /// Records authenticated and restored.
    pub events: u64,
    /// Distinct artifact references verified: 0, since material is never read.
    pub artifacts: u64,
    /// Their declared bytes: 0.
    pub artifact_bytes: u64,
    /// Workers that verified the material: 0.
    pub workers: u64,
    pub sql_read_ms: u64,
    /// Row, relation, head and marker authentication, plus the retention index build.
    pub verify_ms: u64,
    /// Material verification: 0.
    pub material_ms: u64,
    pub restore_ms: u64,
}

impl std::fmt::Display for LedgerOpenTiming {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "events={} artifacts={} artifact_bytes={} workers={} sql_read_ms={} verify_ms={} material_ms={} restore_ms={}",
            self.events,
            self.artifacts,
            self.artifact_bytes,
            self.workers,
            self.sql_read_ms,
            self.verify_ms,
            self.material_ms,
            self.restore_ms
        )
    }
}

pub(in crate::global) fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// The distinct references of a snapshot that carry no eviction proof, in first-reference
/// order, each with the one-based position of the first record that references it.
pub(in crate::global) struct PendingMaterial {
    references: Vec<(usize, ProjectedArtifactReference)>,
    /// sha256 -> indexes into `references`; equality is the complete projected reference.
    index: HashMap<String, Vec<usize>>,
}

impl PendingMaterial {
    pub(in crate::global) fn collect(
        retention: &RetentionIndex,
        records: &[StoredEventRecord],
    ) -> Self {
        let mut pending = Self {
            references: Vec::new(),
            index: HashMap::new(),
        };
        for (position, record) in records.iter().enumerate() {
            for reference in record.projected_artifacts() {
                // A proof, or an identity conflict, is decided by restore in sequence order.
                if !matches!(retention.proof(&reference), Ok(None)) {
                    continue;
                }
                let slot = pending.index.entry(reference.sha256.clone()).or_default();
                if slot
                    .iter()
                    .any(|&known| pending.references[known].1 == reference)
                {
                    continue;
                }
                slot.push(pending.references.len());
                pending.references.push((position + 1, reference));
            }
        }
        pending
    }

    /// One reference after another, for a verifier that cannot be shared. `check` runs
    /// before every reference with the position of its first record.
    pub(in crate::global) fn verify_each<F>(
        self,
        verify: &mut F,
        check: &mut impl FnMut(usize) -> GlobalLedgerResult<()>,
    ) -> GlobalLedgerResult<MaterialVerdicts>
    where
        F: FnMut(&ProjectedArtifactReference) -> Option<VerifiedArtifactReference>,
    {
        let mut verdicts = Vec::with_capacity(self.references.len());
        for (position, reference) in &self.references {
            check(*position)?;
            verdicts.push(verify(reference));
        }
        Ok(MaterialVerdicts {
            pending: self,
            verdicts,
        })
    }
}

/// One verdict per distinct pending reference.
pub(in crate::global) struct MaterialVerdicts {
    pending: PendingMaterial,
    verdicts: Vec<Option<VerifiedArtifactReference>>,
}

impl MaterialVerdicts {
    /// The verdict of a reference that was pending; any other reference has none.
    pub(in crate::global) fn get(
        &self,
        reference: &ProjectedArtifactReference,
    ) -> Option<VerifiedArtifactReference> {
        self.pending
            .index
            .get(&reference.sha256)?
            .iter()
            .find(|&&known| self.pending.references[known].1 == *reference)
            .and_then(|&known| self.verdicts[known].clone())
    }
}
