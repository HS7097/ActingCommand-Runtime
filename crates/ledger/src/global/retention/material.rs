// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #375 H1: within one opening, each distinct artifact reference without an
//! eviction proof is verified exactly once, before the records are restored. A verifier
//! that can be shared runs on a bounded worker pool. Restore then applies the verdicts in
//! sequence order, so the first failing reference keeps today's error and position.

use super::RetentionIndex;
use crate::fact::StoredEventRecord;
use crate::global::{GlobalLedgerError, GlobalLedgerResult};
use actingcommand_contract::{ProjectedArtifactReference, VerifiedArtifactReference};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

/// The bound on the threads that verify artifact material for one opening.
pub(crate) const MATERIAL_WORKERS_MAX: usize = 8;

/// Timings of one complete formal opening (never persisted). The startup writer open prints
/// them on one stdout line, and a failed `ledger-maintenance verify` reports them in its
/// failure cause. A phase that did not run stays 0.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LedgerOpenTiming {
    /// Records authenticated and restored.
    pub events: u64,
    /// Distinct artifact references verified (those without an eviction proof).
    pub artifacts: u64,
    /// Their declared bytes.
    pub artifact_bytes: u64,
    /// Workers that verified the material.
    pub workers: u64,
    pub sql_read_ms: u64,
    /// Row, relation, head and marker authentication, plus the retention index build.
    pub verify_ms: u64,
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
    bytes: u64,
}

impl PendingMaterial {
    pub(in crate::global) fn collect(
        retention: &RetentionIndex,
        records: &[StoredEventRecord],
    ) -> Self {
        let mut pending = Self {
            references: Vec::new(),
            index: HashMap::new(),
            bytes: 0,
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
                pending.bytes = pending.bytes.saturating_add(reference.byte_count);
                pending.references.push((position + 1, reference));
            }
        }
        pending
    }

    pub(in crate::global) fn count(&self) -> u64 {
        self.references.len() as u64
    }

    pub(in crate::global) fn bytes(&self) -> u64 {
        self.bytes
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

    /// On min(available parallelism, `MATERIAL_WORKERS_MAX`) workers. Each worker takes the
    /// next reference in first-reference order and runs `check` (the opening's deadline)
    /// before every file; the first check that fails stops all workers, and of the failed
    /// checks the one for the earliest reference is returned. Memory is one verdict per
    /// distinct reference.
    pub(in crate::global) fn verify_parallel<F, C>(
        self,
        verify: &F,
        check: &C,
        workers_used: &mut u64,
    ) -> GlobalLedgerResult<MaterialVerdicts>
    where
        F: Fn(&ProjectedArtifactReference) -> Option<VerifiedArtifactReference> + Sync,
        C: Fn(usize) -> GlobalLedgerResult<()> + Sync,
    {
        let total = self.references.len();
        let workers = std::thread::available_parallelism()
            .map_or(1, std::num::NonZeroUsize::get)
            .clamp(1, MATERIAL_WORKERS_MAX)
            .min(total);
        if workers <= 1 {
            *workers_used = workers as u64;
            let mut verdicts = Vec::with_capacity(total);
            for (position, reference) in &self.references {
                check(*position)?;
                verdicts.push(verify(reference));
            }
            return Ok(MaterialVerdicts {
                pending: self,
                verdicts,
            });
        }
        let next = AtomicUsize::new(0);
        let stop = AtomicBool::new(false);
        let references = &self.references;
        let verdicts = std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(workers);
            let mut spawn_failure = None;
            for _ in 0..workers {
                let spawned = std::thread::Builder::new()
                    .name("actingcommand-ledger-material".to_string())
                    .spawn_scoped(scope, || {
                        let mut verified = Vec::new();
                        while !stop.load(Ordering::Relaxed) {
                            let index = next.fetch_add(1, Ordering::Relaxed);
                            let Some((position, reference)) = references.get(index) else {
                                break;
                            };
                            if let Err(error) = check(*position) {
                                stop.store(true, Ordering::Relaxed);
                                return Err((index, error));
                            }
                            verified.push((index, verify(reference)));
                        }
                        Ok(verified)
                    });
                match spawned {
                    Ok(handle) => handles.push(handle),
                    Err(error) => {
                        stop.store(true, Ordering::Relaxed);
                        spawn_failure = Some(GlobalLedgerError::io(
                            "ledger_material_worker_spawn_failed",
                            "verify_artifact_material",
                            &error,
                        ));
                        break;
                    }
                }
            }
            *workers_used = handles.len() as u64;
            let mut verdicts = vec![None; total];
            let mut first_failure: Option<(usize, GlobalLedgerError)> = None;
            let mut panicked = false;
            for handle in handles {
                match handle.join() {
                    Ok(Ok(verified)) => {
                        for (index, verdict) in verified {
                            verdicts[index] = verdict;
                        }
                    }
                    Ok(Err((index, error))) => {
                        if first_failure
                            .as_ref()
                            .is_none_or(|(first, _)| index < *first)
                        {
                            first_failure = Some((index, error));
                        }
                    }
                    Err(_) => panicked = true,
                }
            }
            if let Some(error) = spawn_failure {
                return Err(error);
            }
            if panicked {
                return Err(GlobalLedgerError::fatal(
                    "ledger_material_worker_panicked",
                    "verify_artifact_material",
                ));
            }
            match first_failure {
                Some((_, error)) => Err(error),
                None => Ok(verdicts),
            }
        })?;
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
