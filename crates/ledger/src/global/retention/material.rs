// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #375 H1: within one opening that verifies material (the candidate and Segment
//! openings), each distinct artifact reference without an eviction proof is verified exactly
//! once, before the records are restored. Restore then applies the verdicts in sequence
//! order, so the first failing reference keeps its error and position. Workflow #375 R5a:
//! the formal openings (the writer open and every `ledger-maintenance` read) read no
//! material at all; only their timing is kept here.

use super::RetentionIndex;
use crate::codes::{LedgerCode, LedgerLocation};
use crate::fact::StoredEventRecord;
use crate::global::GlobalLedgerResult;
use actingcommand_contract::outcome::{Code, Location, Outcome};
use actingcommand_contract::{ProjectedArtifactReference, VerifiedArtifactReference};
use std::collections::HashMap;
use std::time::Instant;

/// Timings of one formal opening (never persisted). The startup writer open reports them as
/// `ledger_opened`, and a failed `ledger-maintenance verify` reports them in its failure cause.
/// A phase that did not run stays 0. Workflow #375 R5a: a formal opening reads no artifact
/// material, so `artifacts`, `artifact_bytes`, `workers` and `material_ms` are always 0; they
/// stay in the line so that its format is unchanged. LN-8 (#422, LEAN-L §3.2 step 2): the writer
/// open runs the phases `meta_ms`, `tail_ms`, `index_ms` and `import_ms`; the complete read of
/// `ledger-maintenance verify` runs `sql_read_ms`, `verify_ms` and `restore_ms`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LedgerOpenTiming {
    /// Records authenticated (and, for a complete read, restored): the writer open's tail.
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
    /// LN-8: the format, the marker, the keyed meta row and `MAX(sequence)`.
    pub meta_ms: u64,
    /// LN-8: the newest `ledger_observer.open_verify_tail_records` rows read and authenticated.
    pub tail_ms: u64,
    /// LN-8: the views and indexes a database lacks created, the retention state read by type
    /// and by index.
    pub index_ms: u64,
    /// LN-8: a dump an earlier run left imported, the previous epoch's tail gap and the stale
    /// owner's recovery record written.
    pub import_ms: u64,
    /// #422 LN-7c O: why the writer open read every owner record of the history instead of those
    /// of the two latest owner epochs; `None` when its owner read was bounded (or the history
    /// has fewer than two starts). Not part of the line above.
    pub owner_read_full: Option<OwnerReadFull>,
    /// #422 LN-7c R: what the open found of the frame index `ledger_frame_refs` when it had to
    /// rebuild it, fill a gap of records appended without rows, or stop its fill; `None` when the
    /// index was complete or was built for the first time. It never fails the open.
    pub frame_refs: Option<LedgerFrameRefsNote>,
}

/// #422 LN-7c R (`contracts/ledger-observer.md` §11.7): one warning of the writer open about the
/// derived frame index, which the start reports as a `note` link of `ledger_opened`
/// ([`Self::outcome`]: `ledger_index_mismatch` of `build_ledger_frame_refs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedgerFrameRefsNote {
    /// What was found: `verify_ledger_frame_refs`, the extent was untrusted (missing on an
    /// existing table, past the head or malformed), so the rows and the extent were removed and
    /// the index rebuilt; `write_ledger_frame_refs`, records were appended without their rows
    /// (an older build, or a batch the extent did not end before), so the gap was filled;
    /// `build_ledger_frame_refs`, the fill stopped and the extent does not cover the history.
    pub stage: LedgerLocation,
    /// The untrusted extent's end, the end of the extent before the gap, or the record the fill
    /// stopped at.
    pub sequence: Option<u64>,
    /// The failure that stopped the fill: its code and operation.
    pub cause: Option<(&'static str, &'static str)>,
}

impl LedgerFrameRefsNote {
    /// The note's outcome: `ledger_index_mismatch` with `operation`, `stage` and
    /// `ledger_sequence`, caused by the failure that stopped the fill.
    pub fn outcome(&self) -> Outcome {
        let mut outcome = Outcome::new(LedgerCode::LedgerIndexMismatch)
            .operation(LedgerLocation::BuildLedgerFrameRefs)
            .stage(self.stage);
        if let Some(sequence) = self.sequence {
            outcome = outcome.ledger_sequence(sequence);
        }
        if let Some((code, operation)) = self.cause
            && let Some(code) = Code::relayed(code)
        {
            let mut cause = Outcome::new(code);
            if let Some(operation) = Location::relayed(operation) {
                cause = cause.operation(operation);
            }
            outcome = outcome.caused_by(cause);
        }
        outcome
    }
}

/// #422 LN-7c O (`contracts/ledger-store.md`): why the writer open read every owner record of
/// the history. actingd prints it as one line, `actingd ledger_open owner_read=full
/// reason=<reason>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerReadFull {
    /// The bounded read could not place a retained owner in a start's epoch: its first block
    /// holds no start record (a start that stopped before it wrote its start record), or
    /// reading an owner back reached the start before its own (an epoch with no owner record
    /// after its start record).
    OwnerInNoStartEpoch,
    /// A ledger dump an earlier run left is at the dump path or the fallback path. The open
    /// imports it, and the owner it names may precede the bounded read.
    DumpPresent,
}

impl OwnerReadFull {
    /// The `reason` of actingd's line.
    pub fn reason(self) -> &'static str {
        match self {
            Self::OwnerInNoStartEpoch => "owner_in_no_start_epoch",
            Self::DumpPresent => "dump_present",
        }
    }
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
