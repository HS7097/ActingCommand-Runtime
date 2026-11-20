// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #490 ENTRY-A (`contracts/policy-suspension.md`, "A failure at the entry";
//! `contracts/scheduling-blackout.md`, "Order of the deferrals", position 4b; register §1.494):
//! the instances a scheduled run's failure at its entry blocked.
//!
//! The production defect of 11-20 (v0.12.75): a package left a game on a screen no package knows;
//! every task due after it failed at its entry (its main package executed no step), was dispatched
//! again within seconds by the immediate rerun until its runs of the day were spent, a rule whose
//! `escalation_threshold` is 1 was paused by one such failure, and once the runs were spent nothing
//! failed any more, so nothing triggered a recovery.
//!
//! - **The block.** The live settlement settles such a failure as a condition of the run's instance
//!   (its record's identity marked `i…`, `policy_control::entry_blocked`) and blocks the instance
//!   here, under the policy outcome gate; the evaluation reads the blocks under the same gate
//!   (`entry_block_gates`) and defers every candidate of a blocked instance (position 4b,
//!   `crate::entry_block`).
//! - **Its end.** A recovery ladder of the instance that ends recovered (`recovered` or
//!   `environment_ready`), a run on the instance that succeeds (a scheduled run, a client's or the
//!   Lab's run, a startup package, a rung's run: `append_contained_task_terminal`), a resume that
//!   lifts the instance's pause or the global one, and a restart (memory only) end it.
//! - **Its way out.** The failure that blocks the instance is one the ladder takes at the task stage
//!   (the settlement asks the ladder's own trigger rule), so its lease end entered the ladder as a
//!   failed run's does. A ladder its cool-down suppressed is held with the block and taken up when
//!   the cool-down ends (`take_up_entry_recoveries`, the lease sweep's step after the unavailable
//!   instances' probes; the mechanism Workflow #489 AVAIL-1 built). A recovery that ends without
//!   success (a ladder `exhausted`, or one a blackout or an admission refusal dropped) leaves the
//!   instance blocked and brings it to a person's attention once per block
//!   (`instance_entry_blocked_awaiting_person`, a warning record and the daemon's log).
//! - **Races.** A scheduled run's start keeps its instance's marks (how many blocks ended and how
//!   many recoveries failed so far), and its settlement compares them: a recovery that already
//!   succeeded between the run's lease end and its settlement leaves no block; one that already
//!   failed raises the attention with the block.
//!
//! Memory only: a restart starts with no block (its preparation proves the instance again and its
//! startup package runs where one is configured).

use super::recovery_ladder::PendingRecoveryLadder;
use super::unavailable_probe::ProbeHold;
use super::*;
use crate::codes::{HostCode, HostLocation};
use crate::entry_block::{EntryBlockGate, EntryBlockGates};
use crate::failure_identity::identifier_text;
use actingcommand_contract::outcome::{CodeStr, Outcome};

/// What a scheduled run's start knew of its instance: how many blocks of it ended and how many of
/// its recoveries ended without success since the Runtime started.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct EntryMarks {
    ended: u64,
    unrecovered: u64,
}

/// A recovery the ladder's cool-down held back, and the end of that cool-down.
struct HeldEntryRecovery {
    ladder: PendingRecoveryLadder,
    until_unix_ms: u64,
}

/// One blocked instance.
struct EntryBlock {
    gate: EntryBlockGate,
    /// Its recovery ended without success: a person was told.
    awaiting_person: bool,
    held: Option<HeldEntryRecovery>,
}

/// The blocks and the marks (memory only). A leaf lock: nothing else is locked while it is held.
#[derive(Default)]
pub(super) struct EntryBlocks {
    blocked: BTreeMap<InstanceId, EntryBlock>,
    ended: BTreeMap<InstanceId, u64>,
    unrecovered: BTreeMap<InstanceId, u64>,
    /// The latest ladder of an instance not blocked (yet) that its cool-down suppressed, with the
    /// failed run its trigger names: the settlement of that run, when it blocks the instance, takes
    /// it as its block's held recovery.
    cooled: BTreeMap<InstanceId, (RunId, HeldEntryRecovery)>,
    /// Test only: the next settlement is one of a run that failed at its entry.
    #[cfg(test)]
    settle_at_entry: bool,
}

impl EntryBlocks {
    fn marks(&self, instance_id: InstanceId) -> EntryMarks {
        EntryMarks {
            ended: self.ended.get(&instance_id).copied().unwrap_or(0),
            unrecovered: self.unrecovered.get(&instance_id).copied().unwrap_or(0),
        }
    }
}

/// One blocked instance as a test reads it: its gate, whether a person was told and when its held
/// recovery is due.
#[cfg(test)]
pub(crate) type EntryBlockForTest = (EntryBlockGate, bool, Option<u64>);

impl HostShared {
    /// The marks of `instance_id` now, which a scheduled run's start keeps.
    pub(super) fn entry_marks(&self, instance_id: InstanceId) -> RuntimeHostResult<EntryMarks> {
        Ok(lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?.marks(instance_id))
    }

    /// Blocks the instance of `data`, a run's execution record settled now as failed at its entry
    /// (`failure_code` its failure's code, `run_id` the run when the settlement had its context,
    /// `marks` what the run's start knew of the instance). A block of the instance that ended since
    /// the run's start leaves it unblocked; a recovery that failed since then raises the attention
    /// at once; a recovery the cool-down held back for this run is the block's.
    pub(super) fn block_instance_entry(
        &self,
        data: &PolicyExecutionEventData,
        failure_code: CodeStr,
        run_id: Option<RunId>,
        marks: EntryMarks,
    ) -> RuntimeHostResult<()> {
        let instance_id = self
            .resolve_instance(&data.instance_id)
            .map_err(|failure| *failure.error)?
            .instance_id();
        let gate = EntryBlockGate {
            task_id: data.task_id.clone(),
            decision_id: data.decision_id.clone(),
            failure_code,
            since_unix_ms: data.observed_at_unix_ms,
        };
        let attention = {
            let mut blocks = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
            let now = blocks.marks(instance_id);
            let cooled = blocks
                .cooled
                .remove(&instance_id)
                .and_then(|(cooled_run, held)| (Some(cooled_run) == run_id).then_some(held));
            if now.ended != marks.ended {
                // A recovery or a run of the instance succeeded, or a resume came, after the run
                // began: nothing is left to block.
                return Ok(());
            }
            let previous = blocks.blocked.remove(&instance_id);
            let told = previous.as_ref().is_some_and(|block| block.awaiting_person);
            let awaiting_person = told || now.unrecovered != marks.unrecovered;
            let held = cooled.or_else(|| previous.and_then(|block| block.held));
            let attention = (awaiting_person && !told).then(|| gate.clone());
            blocks.blocked.insert(
                instance_id,
                EntryBlock {
                    gate,
                    awaiting_person,
                    held,
                },
            );
            attention
        };
        match attention {
            Some(gate) => self.record_entry_attention(instance_id, &gate),
            None => Ok(()),
        }
    }

    /// Ends the entry block of `instance_id`, if it has one: a recovery of it or a run on it
    /// succeeded, or a resume lifted a pause of it. Counted for a settlement still to come.
    pub(super) fn end_entry_block(&self, instance_id: InstanceId) -> RuntimeHostResult<()> {
        let mut blocks = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
        let ended = blocks.ended.entry(instance_id).or_insert(0);
        *ended = ended.saturating_add(1);
        blocks.blocked.remove(&instance_id);
        Ok(())
    }

    /// A resume of `scope` was answered: it ends the entry block of the instance it resumed, or of
    /// every instance for the global scope.
    pub(super) fn end_entry_blocks_at_resume(
        &self,
        scope: &actingcommand_contract::SchedulingPauseScope,
    ) -> RuntimeHostResult<()> {
        let instances = match scope.instance_alias() {
            Some(instance_alias) => vec![
                self.resolve_instance(instance_alias)
                    .map_err(|failure| *failure.error)?
                    .instance_id(),
            ],
            None => lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?
                .blocked
                .keys()
                .copied()
                .collect(),
        };
        for instance_id in instances {
            self.end_entry_block(instance_id)?;
        }
        Ok(())
    }

    /// A recovery of `instance_id` ended without success (a ladder `exhausted`, or one a blackout
    /// or an admission refusal dropped): a blocked instance stays blocked and is brought to a
    /// person's attention, once per block.
    pub(super) fn note_entry_recovery_failed(
        &self,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<()> {
        let attention = {
            let mut blocks = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
            let unrecovered = blocks.unrecovered.entry(instance_id).or_insert(0);
            *unrecovered = unrecovered.saturating_add(1);
            match blocks.blocked.get_mut(&instance_id) {
                Some(block) if !block.awaiting_person => {
                    block.awaiting_person = true;
                    Some(block.gate.clone())
                }
                _ => None,
            }
        };
        match attention {
            Some(gate) => self.record_entry_attention(instance_id, &gate),
            None => Ok(()),
        }
    }

    /// A ladder of `instance_id` its cool-down suppressed until `until_unix_ms` while no probe of
    /// an unavailable instance holds it (Workflow #489 AVAIL-1): a blocked instance holds it until
    /// the cool-down ends; for an instance not blocked, one a failed run's trigger names is kept
    /// for that run's settlement.
    pub(super) fn hold_entry_recovery(
        &self,
        instance_id: InstanceId,
        ladder: PendingRecoveryLadder,
        until_unix_ms: u64,
    ) -> RuntimeHostResult<()> {
        let run_id = ladder.task_run();
        let held = HeldEntryRecovery {
            ladder,
            until_unix_ms,
        };
        let mut blocks = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
        if let Some(block) = blocks.blocked.get_mut(&instance_id) {
            block.held = Some(held);
        } else if let Some(run_id) = run_id {
            blocks.cooled.insert(instance_id, (run_id, held));
        }
        Ok(())
    }

    /// The lease sweep's step after the unavailable instances' probes: each blocked instance whose
    /// held recovery's cool-down has ended has it admitted as it would have been without the
    /// cool-down ([`Self::admit_held_recovery`], every other admission rule as for every ladder),
    /// unless a blackout covers the instance (it waits) or a pause, a fence or an idle stop owns it
    /// (the recovery is dropped: those have their own rules, and a resume ends the block). Only a
    /// failure the Runtime does not survive is returned.
    pub(super) fn take_up_entry_recoveries(&self) -> RuntimeHostResult<()> {
        if self.fatal.is_shutdown_requested() {
            return Ok(());
        }
        let now_unix_ms = self.runtime_clock_sample()?.unix_ms;
        let due = lock(&self.entry_blocks, HostLocation::TakeUpEntryRecoveries)?
            .blocked
            .iter()
            .filter(|(_, block)| {
                block
                    .held
                    .as_ref()
                    .is_some_and(|held| held.until_unix_ms <= now_unix_ms)
            })
            .map(|(instance_id, _)| *instance_id)
            .collect::<Vec<_>>();
        for instance_id in due {
            let alias = lock(
                &self.registered_instances,
                HostLocation::TakeUpEntryRecoveries,
            )?
            .get(&instance_id)
            .map(|instance| instance.instance_alias.clone());
            let Some(alias) = alias else {
                continue;
            };
            let hold = self.probe_hold(instance_id, &alias)?;
            if hold == ProbeHold::Blackout {
                continue;
            }
            let held = lock(&self.entry_blocks, HostLocation::TakeUpEntryRecoveries)?
                .blocked
                .get_mut(&instance_id)
                .and_then(|block| block.held.take_if(|held| held.until_unix_ms <= now_unix_ms));
            if hold == ProbeHold::Owned {
                continue;
            }
            if let Some(held) = held {
                self.admit_held_recovery(held.ladder)?;
            }
        }
        Ok(())
    }

    /// The blocked instances by alias, as the evaluation reads them.
    pub(super) fn entry_block_gates(&self) -> RuntimeHostResult<EntryBlockGates> {
        let blocked = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?
            .blocked
            .iter()
            .map(|(instance_id, block)| (*instance_id, block.gate.clone()))
            .collect::<Vec<_>>();
        if blocked.is_empty() {
            return Ok(EntryBlockGates::new());
        }
        let instances = lock(&self.registered_instances, HostLocation::InstanceEntryBlock)?;
        Ok(blocked
            .into_iter()
            .filter_map(|(instance_id, gate)| {
                instances
                    .get(&instance_id)
                    .map(|instance| (instance.instance_alias.clone(), gate))
            })
            .collect())
    }

    /// `instance_entry_blocked_awaiting_person` of `instance_id` blocked by `gate`: a warning
    /// `runtime.failed` record under the instance's links, handed to the daemon's log too.
    fn record_entry_attention(
        &self,
        instance_id: InstanceId,
        gate: &EntryBlockGate,
    ) -> RuntimeHostResult<()> {
        let operation = HostLocation::InstanceEntryBlock;
        let outcome = Outcome::new(HostCode::InstanceEntryBlockedAwaitingPerson)
            .operation(operation)
            .instance_id(identifier_text(&instance_id))
            .task_id(gate.task_id.as_str())
            .decision_id(gate.decision_id.as_str())
            .observed_at_unix_ms(gate.since_unix_ms);
        let outcome = match gate.failure_code.registered() {
            Some(code) => outcome.failure_code(code),
            None => outcome,
        };
        let links = self
            .events
            .system_links()?
            .with_instance_id(self.events.issuer().issue_registered_instance(instance_id));
        self.append_event_raw(
            EventSeverity::Warning,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links,
            RuntimePayloadDraft::failed(
                DiagnosticCode::RuntimeDiagnostic,
                EffectDisposition::NotPerformed,
                outcome.clone(),
                DiagnosticDetailDraft::new(
                    "entry_block",
                    operation,
                    "runtime_host",
                    operation.as_str(),
                ),
                AuditInput::new(),
            ),
        )?;
        self.fatal.report_warning(&outcome);
        Ok(())
    }

    /// Test only: the next settlement of a scheduled run is one of a run that failed at its entry,
    /// judged against the instance's marks at that settlement.
    #[cfg(test)]
    pub(super) fn settle_next_at_entry_for_test(&self) -> RuntimeHostResult<()> {
        lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?.settle_at_entry = true;
        Ok(())
    }

    /// Test only: whether the next settlement was armed to be one of a run that failed at its
    /// entry; taken once.
    #[cfg(test)]
    pub(super) fn take_settle_at_entry_for_test(&self) -> RuntimeHostResult<bool> {
        Ok(std::mem::take(
            &mut lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?.settle_at_entry,
        ))
    }

    /// Test only: the entry block of `instance_id`, if it has one.
    #[cfg(test)]
    pub(super) fn entry_block_for_test(
        &self,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<Option<EntryBlockForTest>> {
        Ok(lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?
            .blocked
            .get(&instance_id)
            .map(|block| {
                (
                    block.gate.clone(),
                    block.awaiting_person,
                    block.held.as_ref().map(|held| held.until_unix_ms),
                )
            }))
    }
}
