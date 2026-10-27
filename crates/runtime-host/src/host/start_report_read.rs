// SPDX-License-Identifier: AGPL-3.0-only

//! `RuntimeOperation::StartReport` (Workflow #421 INST-2; #421 deploy design §4): what this
//! Runtime knows about its own start, kept in memory as the start goes and answered from there,
//! so the installer's post-start checks read no ledger and parse no log. Like a status read it
//! reads and appends nothing in the ledger.
//!
//! The start notes each part when it has it: the ledger open (its phases, the deadline, whether
//! it was slow, the owner read and the frame index note), how the tagged state was restored, the
//! recovery's time, the scheduling pauses restored and dropped, the run-time instructions
//! restored and those not restored as they were, and its readiness. Every admitted dispatch is
//! counted per instance. The instances bound and the standard packs registered are read when the
//! report is answered.

use super::*;
use actingcommand_contract::{
    RuntimeStartReport, StartReportDispatches, StartReportOpen, StartReportPacks,
    StartReportPhases, StartReportTaggedState,
};

/// What this start noted about itself (memory only).
pub(super) struct StartFacts {
    started_at_unix_ms: u64,
    open: StartReportOpen,
    ready_at_unix_ms: Option<u64>,
    tagged_state: Option<StartReportTaggedState>,
    recovery_ms: Option<u64>,
    pauses_restored: u64,
    pauses_dropped: Vec<String>,
    instructions_restored: u64,
    instructions_dropped: Vec<String>,
    dispatches: BTreeMap<String, u64>,
}

impl StartFacts {
    /// The facts of a start that began at `started_at_unix_ms` and opened its ledger as `timing`
    /// says, within `deadline_ms`; `slow` when the open passed its warning line.
    pub(super) fn new(
        started_at_unix_ms: u64,
        timing: &actingcommand_ledger::LedgerOpenTiming,
        deadline_ms: u64,
        slow: bool,
    ) -> Self {
        let phases = StartReportPhases {
            sql_read_ms: timing.sql_read_ms,
            verify_ms: timing.verify_ms,
            material_ms: timing.material_ms,
            restore_ms: timing.restore_ms,
            meta_ms: timing.meta_ms,
            tail_ms: timing.tail_ms,
            index_ms: timing.index_ms,
            import_ms: timing.import_ms,
        };
        Self {
            started_at_unix_ms,
            open: StartReportOpen {
                phases,
                sum_ms: phases.sum_ms(),
                deadline_ms,
                slow,
                owner_read_full: timing.owner_read_full.map(|full| full.as_str().to_owned()),
                index_note: timing
                    .frame_refs
                    .map(|note| note.outcome().code().as_str().to_owned()),
            },
            ready_at_unix_ms: None,
            tagged_state: None,
            recovery_ms: None,
            pauses_restored: 0,
            pauses_dropped: Vec::new(),
            instructions_restored: 0,
            instructions_dropped: Vec::new(),
            dispatches: BTreeMap::new(),
        }
    }
}

impl HostShared {
    /// Notes how this start restored the tagged state (Workflow #422 LN-4).
    pub(super) fn note_start_tagged_state(&self, seeded: bool) -> RuntimeHostResult<()> {
        lock(&self.start_facts, HostLocation::StartRuntimeHost)?.tagged_state = Some(if seeded {
            StartReportTaggedState::Seed
        } else {
            StartReportTaggedState::Load
        });
        Ok(())
    }

    /// Notes the recovery's time (Workflow #381 R6).
    pub(super) fn note_start_recovery(&self, recovery_ms: Option<u64>) -> RuntimeHostResult<()> {
        lock(&self.start_facts, HostLocation::StartRuntimeHost)?.recovery_ms = recovery_ms;
        Ok(())
    }

    /// Notes the scheduling pauses the start restored and dropped (Workflow #361 B1): one
    /// `scheduling_pause_restored` or `scheduling_pause_dropped` outcome each, the dropped ones
    /// naming their `alias`.
    pub(super) fn note_start_pauses(&self, restore: &[Outcome]) -> RuntimeHostResult<()> {
        let mut facts = lock(&self.start_facts, HostLocation::StartRuntimeHost)?;
        for outcome in restore {
            if outcome.is(HostCode::SchedulingPauseDropped) {
                if let Some(alias) = outcome.value("alias").and_then(serde_json::Value::as_str)
                    && !facts.pauses_dropped.iter().any(|known| known == alias)
                {
                    facts.pauses_dropped.push(alias.to_owned());
                }
            } else {
                facts.pauses_restored += 1;
            }
        }
        Ok(())
    }

    /// Notes the run-time instructions the start restored and the keys it did not restore as
    /// they were (Workflow #387 D1b, §5.7).
    pub(super) fn note_start_instructions(
        &self,
        restored: usize,
        dropped: Vec<String>,
    ) -> RuntimeHostResult<()> {
        let mut facts = lock(&self.start_facts, HostLocation::StartRuntimeHost)?;
        facts.instructions_restored = u64::try_from(restored).unwrap_or(u64::MAX);
        for key in dropped {
            if !facts.instructions_dropped.contains(&key) {
                facts.instructions_dropped.push(key);
            }
        }
        Ok(())
    }

    /// Notes that the start is ready, at `ready_at_unix_ms`.
    pub(super) fn note_start_ready(&self, ready_at_unix_ms: u64) -> RuntimeHostResult<()> {
        lock(&self.start_facts, HostLocation::StartRuntimeHost)?.ready_at_unix_ms =
            Some(ready_at_unix_ms);
        Ok(())
    }

    /// Counts one admitted dispatch on the instance `instance_alias`.
    pub(super) fn count_start_dispatch(&self, instance_alias: &str) -> RuntimeHostResult<()> {
        *lock(&self.start_facts, HostLocation::StartRuntimeHost)?
            .dispatches
            .entry(instance_alias.to_owned())
            .or_insert(0) += 1;
        Ok(())
    }

    /// The answer of `StartReport`: the facts the start noted, with the instances bound and the
    /// standard packs registered as they are now.
    pub(super) fn start_report_read(&self) -> Result<OperationSuccess, RequestFailure> {
        let mut instances_bound = lock(
            &self.registered_instances,
            HostLocation::PrepareRegisteredInstances,
        )?
        .values()
        .map(|instance| instance.instance_alias.clone())
        .collect::<Vec<_>>();
        instances_bound.sort();
        instances_bound.dedup();
        let standard_packs = self
            .standard_packs
            .registered_counts()?
            .into_iter()
            .map(|(game, pack_count)| StartReportPacks {
                game,
                pack_count: u64::try_from(pack_count).unwrap_or(u64::MAX),
            })
            .collect::<Vec<_>>();
        let report = {
            let facts = lock(&self.start_facts, HostLocation::StartRuntimeHost)?;
            let mut pauses_dropped = facts.pauses_dropped.clone();
            pauses_dropped.sort();
            let mut instructions_dropped = facts.instructions_dropped.clone();
            instructions_dropped.sort();
            RuntimeStartReport {
                owner_epoch: self.owner_epoch,
                started_at_unix_ms: facts.started_at_unix_ms,
                ready_at_unix_ms: facts.ready_at_unix_ms,
                open: facts.open.clone(),
                tagged_state: facts.tagged_state,
                recovery_ms: facts.recovery_ms,
                pauses_restored: facts.pauses_restored,
                pauses_dropped,
                instructions_restored: facts.instructions_restored,
                instructions_dropped,
                instances_bound,
                standard_packs,
                dispatches_since_start: facts
                    .dispatches
                    .iter()
                    .map(|(instance_alias, count)| StartReportDispatches {
                        instance_alias: instance_alias.clone(),
                        count: *count,
                    })
                    .collect(),
            }
        };
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::StartReport {
                report: Box::new(report),
            },
            failure_outcome: None,
        })
    }
}
