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
//!   (`entry_block_gates`) and defers every candidate of a blocked instance but a store update
//!   task's (position 4b, `crate::entry_block`).
//! - **Its end.** A recovery ladder of the instance that ends `recovered` or `environment_ready`,
//!   a run on the instance that succeeds (a scheduled run, a client's or the Lab's run, a startup
//!   package, a rung's run: `append_contained_task_terminal`), a resume that lifts the instance's
//!   pause or the global one, a restart (memory only), and (fix round 1) a blackout that covers the
//!   instance or a fence that takes it, each of which also drops the block's held recovery.
//! - **Its way out.** The failure that blocks the instance is one its block has a recovery for
//!   (`entry_failure_recovered`): the ladder its lease end entered, or (fix round 1) for a failure
//!   the exclusion list keeps from starting a ladder by itself, the ladder its lease end prepared
//!   under the item that takes its class, which the block takes as its own trigger. A recovery
//!   held with the block (the ladder's cool-down, an approaching blackout, a transient obstacle) is
//!   taken up by the lease sweep (`take_up_entry_recoveries`, after the unavailable instances'
//!   probes; the mechanism Workflow #489 AVAIL-1 built). Fix round 1, no block without an end and
//!   a person told: a transient obstacle (the admission's capacity refusal, another ladder that
//!   holds or waits for the instance, a claim the queue refused, a preparation superseded) keeps
//!   the recovery held and tries it again at the next tick; a recovery of the block that ends
//!   without recovering (its ladder `exhausted`, its worker panicked) leaves the instance blocked
//!   and tells a person once per block; and a block that lasts
//!   `scheduling.unavailable_probe.attention_after_ms` tells a person once per block whatever
//!   holds it (`instance_entry_blocked_awaiting_person`, a warning record and the daemon's log).
//! - **Races.** A scheduled run's start keeps its instance's marks (how many blocks ended so far),
//!   and its settlement compares them: anything that ended a block between the run's start and its
//!   settlement leaves no block. A recovery of the run's own failure that ended without recovering
//!   before the settlement (fix round 1: that run's, not any ladder of the instance) raises the
//!   attention with the block.
//!
//! - **Whose failure (ENTRY-A2).** The coordinator's ruling of 11-20 23:57 JST: a failure at the
//!   entry is the instance's condition only while nothing says it is the pair's own. Each pair
//!   (instance, catalog task) keeps here whether its task's last scheduled run failed at its entry
//!   (its mark: what that run's start knew of the instance's recoveries, the packages its identity
//!   names, and (fix round 1) how many such failures in a row settled as conditions since the
//!   pair last started its main package). A failure at the entry of a pair whose mark is set, with
//!   the same packages, after a recovery of the instance ended `recovered` since that run began,
//!   is the pair's own when no other run started and no person or client held the instance's key
//!   between that recovery and its own start (the ruling of 11-21 00:22 JST; the count
//!   `begin_contained_run` and a manual claim's grant keep here), or (fix round 1, the bound) when
//!   the pair's conditions in a row reached its rule's `escalation_threshold`, whatever ran first.
//!   It settles as a failure that accumulates (`failure_identity::own_entry_identity`) and blocks
//!   the instance all the same. A run of the task whose main package started, a settlement that
//!   pauses the pair and a success clear the mark; packages that changed make it count for
//!   nothing.
//!
//! Memory only: a restart starts with no block and no mark (its preparation proves the instance
//! again and its startup package runs where one is configured).

use super::recovery_ladder::PendingRecoveryLadder;
use super::unavailable_probe::ProbeHold;
use super::*;
use crate::codes::{HostCode, HostLocation};
use crate::entry_block::{EntryBlockGate, EntryBlockGates};
use crate::failure_identity::{FailureIdentity, identifier_text};
use actingcommand_contract::outcome::{CodeStr, Outcome};

/// What a scheduled run's start knew of its instance: how many blocks of it ended and (ENTRY-A2)
/// how many of its recoveries ended `recovered` since the Runtime started, and the runs started and
/// manual keys granted on it since the latest of those recoveries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct EntryMarks {
    ended: u64,
    recovered: u64,
    /// ENTRY-A2 (the coordinator's ruling of 11-21 00:22 JST; fix round 1): the runs started on
    /// the instance since its latest successful recovery (a scheduled run, a client's or the Lab's;
    /// not the Runtime's own startup or recovery package's run), and the keys of it granted to a
    /// person's or a client's claim since then (a client's lease, a Lab operation's, a direct run's,
    /// an input lifecycle's).
    started: u64,
}

/// Workflow #490 ENTRY-A and A2: what a scheduled run's settlement tells its instance's block and
/// its pair's mark. A settlement without one (a stop, a reconciled or interrupted run, a run whose
/// chain was not resolved) leaves the mark as it is, but for a success.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EntrySettlement {
    /// The run failed at its entry (ENTRY-A's set), with what its start knew of its instance;
    /// `own` (ENTRY-A2) when the failure is its pair's own ([`HostShared::entry_failure_own`]).
    AtEntry { marks: EntryMarks, own: bool },
    /// The run's main package started: its entry worked, whatever its end.
    TargetStarted,
}

/// ENTRY-A2: a pair's mark: its task's last scheduled run failed at its entry.
struct EntryFailureMark {
    /// The instance's recoveries that ended `recovered` when that run began.
    recovered: u64,
    /// The packages its failure identity named ([`FailureIdentity::packages`]).
    packages: String,
    /// Fix round 1 (the bound): the pair's failures at the entry in a row settled as conditions
    /// since it last started its main package.
    conditions: u16,
}

/// Test only: what the next settlement is armed to be.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettleForTest {
    /// One of a run that failed at its entry, judged against the instance's marks then.
    AtEntry,
    /// One of a run whose main package started.
    PastEntry,
}

/// A recovery held with a block (or for a scheduled run's settlement), and when it is due.
struct HeldEntryRecovery {
    ladder: PendingRecoveryLadder,
    until_unix_ms: u64,
}

/// One blocked instance.
struct EntryBlock {
    /// The latest failure at the entry that blocked it.
    gate: EntryBlockGate,
    /// Since when the instance is blocked without a break (the attention's clock).
    since_unix_ms: u64,
    /// A person was told (`instance_entry_blocked_awaiting_person`), once per block.
    awaiting_person: bool,
    held: Option<HeldEntryRecovery>,
    /// Fix round 1: the trigger correlations of the block's own recoveries (the blocking runs'
    /// lease ends, the recoveries it held or took over): only one of these ending without
    /// recovering tells a person; another trigger's ladder does not.
    recoveries: BTreeSet<CorrelationId>,
}

/// The blocks and the marks (memory only). A leaf lock: nothing else is locked while it is held.
#[derive(Default)]
pub(super) struct EntryBlocks {
    blocked: BTreeMap<InstanceId, EntryBlock>,
    ended: BTreeMap<InstanceId, u64>,
    /// ENTRY-A2: the instance's recovery ladders that ended `recovered` (fix round 1: not
    /// `environment_ready`, which proved nothing of the screen).
    recovered: BTreeMap<InstanceId, u64>,
    /// ENTRY-A2: the runs started and the manual keys granted on the instance since its latest
    /// successful recovery.
    started: BTreeMap<InstanceId, u64>,
    /// ENTRY-A2: the pairs' marks, by instance alias and catalog task.
    entry_failed: BTreeMap<(String, String), EntryFailureMark>,
    /// Fix round 1: the scheduled runs started whose settlement has not come yet.
    settling: BTreeSet<RunId>,
    /// A recovery held for a scheduled run whose settlement has not come yet (its instance not
    /// blocked yet), by the run (fix round 1: not by the instance, so no other run's recovery
    /// displaces it): the settlement takes it when it blocks the instance and lets it go
    /// otherwise.
    cooled: BTreeMap<RunId, HeldEntryRecovery>,
    /// Fix round 1: the scheduled runs whose own recovery ended without recovering before their
    /// settlement: the block that settlement sets tells a person at once.
    unrecovered: BTreeSet<RunId>,
    /// Test only: what the next settlement is armed to be.
    #[cfg(test)]
    settle_for_test: Option<SettleForTest>,
}

impl EntryBlocks {
    fn marks(&self, instance_id: InstanceId) -> EntryMarks {
        EntryMarks {
            ended: self.ended.get(&instance_id).copied().unwrap_or(0),
            recovered: self.recovered.get(&instance_id).copied().unwrap_or(0),
            started: self.started.get(&instance_id).copied().unwrap_or(0),
        }
    }

    /// Ends the block of `instance_id`, if it has one, its held recovery dropped; counted for a
    /// settlement still to come.
    fn end(&mut self, instance_id: InstanceId) {
        let ended = self.ended.entry(instance_id).or_insert(0);
        *ended = ended.saturating_add(1);
        self.blocked.remove(&instance_id);
    }
}

/// One blocked instance as a test reads it: its gate, whether a person was told and when its held
/// recovery is due.
#[cfg(test)]
pub(crate) type EntryBlockForTest = (EntryBlockGate, bool, Option<u64>);

impl HostShared {
    /// The marks of `instance_id` now.
    #[cfg(test)]
    pub(super) fn entry_marks(&self, instance_id: InstanceId) -> RuntimeHostResult<EntryMarks> {
        Ok(lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?.marks(instance_id))
    }

    /// A scheduled run `run_id` on `instance_id` starts (its resolution is registered, before
    /// `begin_contained_run` counts it): the marks of its instance now, which its settlement
    /// compares, and (fix round 1) its settlement is due, so a recovery held for it is kept until
    /// then.
    pub(super) fn entry_settlement_pending(
        &self,
        run_id: RunId,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<EntryMarks> {
        let mut blocks = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
        blocks.settling.insert(run_id);
        Ok(blocks.marks(instance_id))
    }

    /// ENTRY-A2: whether a failure at the entry of `task_id` on `instance_alias`, whose run's start
    /// knew `marks` and whose identity names `packages`, is the pair's own: the pair's mark is set
    /// (its task's last scheduled run failed at its entry) with the same packages, a recovery of the
    /// instance ended `recovered` since that run began, and either (the coordinator's ruling of
    /// 11-21 00:22 JST) no other run started and no manual key was granted on the instance between
    /// the latest such recovery and this run's start (the run started from a recovered instance),
    /// or (fix round 1, the bound) the pair's failures at the entry in a row settled as conditions
    /// reached `escalation_threshold`, its rule's, whatever ran first. A block ended by anything
    /// else (a run's success, a resume, `environment_ready`) is no evidence against the pair.
    pub(super) fn entry_failure_own(
        &self,
        instance_alias: &str,
        task_id: &str,
        marks: EntryMarks,
        packages: &str,
        escalation_threshold: Option<u16>,
    ) -> RuntimeHostResult<bool> {
        Ok(lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?
            .entry_failed
            .get(&(instance_alias.to_owned(), task_id.to_owned()))
            .is_some_and(|mark| {
                mark.packages == packages
                    && marks.recovered > mark.recovered
                    && (marks.started == 0
                        || escalation_threshold
                            .is_some_and(|threshold| mark.conditions >= threshold))
            }))
    }

    /// ENTRY-A2: a run started on `instance_id` (a scheduled run, a client's or the Lab's run;
    /// `begin_contained_run` calls it for every run but the Runtime's own startup or recovery
    /// package's), or (fix round 1) a key of it was granted to a person's or a client's claim
    /// (`grant_prepared_lease_with_links`): counted until the instance's next successful recovery.
    pub(super) fn note_entry_run_started(&self, instance_id: InstanceId) -> RuntimeHostResult<()> {
        let mut blocks = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
        let started = blocks.started.entry(instance_id).or_insert(0);
        *started = started.saturating_add(1);
        Ok(())
    }

    /// Workflow #490 ENTRY-A and A2: what the execution record `data`, recorded now, tells its
    /// pair's mark and its instance's block (`entry` what the settlement knew of the run, `context`
    /// the run when the settlement had it). A success, a run whose main package started and a
    /// settlement that paused the pair clear the mark; a failure at the entry sets it (unless it
    /// paused the pair), counting a condition in a row, and blocks the instance when it settled as
    /// a condition at the entry or as the pair's own failure (a configured condition of
    /// `scheduling.rerun` blocks nothing, as before ENTRY-A2).
    pub(super) fn settle_entry(
        &self,
        data: &PolicyExecutionEventData,
        entry: Option<EntrySettlement>,
        context: Option<&PolicyRunContext>,
    ) -> RuntimeHostResult<()> {
        let key = (data.instance_id.clone(), data.task_id.clone());
        let failure = match &data.outcome {
            PolicyExecutionOutcome::Failed { failure } => Some(failure),
            PolicyExecutionOutcome::Succeeded { .. } | PolicyExecutionOutcome::Stopped { .. } => {
                None
            }
        };
        let paused = failure.is_some_and(|failure| {
            failure.disposition == actingcommand_contract::PolicyFailureDisposition::PausedTask
        });
        let succeeded = matches!(data.outcome, PolicyExecutionOutcome::Succeeded { .. });
        let at_entry = match entry {
            Some(EntrySettlement::AtEntry { marks, own }) => Some((marks, own)),
            Some(EntrySettlement::TargetStarted) | None => None,
        };
        let blocks_instance = match (at_entry, failure) {
            (Some((_, own)), Some(failure)) => {
                crate::policy_control::entry_blocked(failure)
                    || (own && failure.condition.is_none())
            }
            _ => false,
        };
        let run = context.map(|context| (context.run_id(), context.correlation_id()));
        {
            let mut blocks = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
            if let Some((run_id, _)) = run {
                blocks.settling.remove(&run_id);
                // A recovery held for this run, or its own that ended without recovering, is the
                // block's only when this settlement blocks the instance (`block_instance_entry`);
                // any other lets it go, as it went before ENTRY-A.
                if !blocks_instance {
                    blocks.cooled.remove(&run_id);
                    blocks.unrecovered.remove(&run_id);
                }
            }
            if succeeded || paused || entry == Some(EntrySettlement::TargetStarted) {
                blocks.entry_failed.remove(&key);
            } else if let Some((marks, own)) = at_entry {
                let packages = failure
                    .and_then(|failure| failure.failure_identity.as_deref())
                    .and_then(FailureIdentity::parse)
                    .map(|identity| identity.packages())
                    .unwrap_or_default();
                // Fix round 1 (the bound): a condition adds one to the pair's conditions in a row;
                // the pair's own failure leaves them as they are; changed packages start again.
                let conditions = match blocks.entry_failed.get(&key) {
                    Some(mark) if mark.packages == packages && own => mark.conditions,
                    Some(mark) if mark.packages == packages => mark.conditions.saturating_add(1),
                    _ => 1,
                };
                blocks.entry_failed.insert(
                    key,
                    EntryFailureMark {
                        recovered: marks.recovered,
                        packages,
                        conditions,
                    },
                );
            }
        }
        let (true, Some((marks, _)), Some(failure)) = (blocks_instance, at_entry, failure) else {
            return Ok(());
        };
        self.block_instance_entry(
            data,
            actingcommand_contract::scheduled_failure_code_str(&failure.outcome),
            run,
            marks,
        )
    }

    /// Blocks the instance of `data`, a run's execution record settled now as failed at its entry
    /// (`failure_code` its failure's code, `run` the run and its correlation when the settlement
    /// had its context, `marks` what the run's start knew of the instance). A block of the
    /// instance that ended since the run's start leaves it unblocked; the run's own recovery that
    /// ended without recovering since then raises the attention at once; a recovery held for this
    /// run is the block's.
    fn block_instance_entry(
        &self,
        data: &PolicyExecutionEventData,
        failure_code: CodeStr,
        run: Option<(RunId, CorrelationId)>,
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
            let (cooled, failed) = match run {
                Some((run_id, _)) => (
                    blocks.cooled.remove(&run_id),
                    blocks.unrecovered.remove(&run_id),
                ),
                None => (None, false),
            };
            if now.ended != marks.ended {
                // A recovery or a run of the instance succeeded, a resume came, a blackout covered
                // it or a fence took it after the run began: nothing is left to block.
                return Ok(());
            }
            let (since_unix_ms, told, mut recoveries, previous_held) =
                match blocks.blocked.remove(&instance_id) {
                    Some(block) => (
                        block.since_unix_ms,
                        block.awaiting_person,
                        block.recoveries,
                        block.held,
                    ),
                    None => (gate.since_unix_ms, false, BTreeSet::new(), None),
                };
            if let Some((_, correlation)) = run {
                recoveries.insert(correlation);
            }
            if let Some(held) = &cooled {
                recoveries.insert(held.ladder.correlation());
            }
            let awaiting_person = told || failed;
            let attention = (awaiting_person && !told).then(|| gate.clone());
            blocks.blocked.insert(
                instance_id,
                EntryBlock {
                    gate,
                    since_unix_ms,
                    awaiting_person,
                    held: cooled.or(previous_held),
                    recoveries,
                },
            );
            attention
        };
        match attention {
            Some(gate) => self.record_entry_attention(instance_id, &gate, None),
            None => Ok(()),
        }
    }

    /// Ends the entry block of `instance_id`, if it has one, and drops its held recovery: a run on
    /// it succeeded, a recovery of it ended `environment_ready`, a resume lifted a pause of it, or
    /// (fix round 1) a blackout covers it or a fence took it. Counted for a settlement still to
    /// come.
    pub(super) fn end_entry_block(&self, instance_id: InstanceId) -> RuntimeHostResult<()> {
        lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?.end(instance_id);
        Ok(())
    }

    /// A recovery ladder of `instance_id` ended `recovered`: it ends the instance's entry block and
    /// (ENTRY-A2) is counted as the one unblocking after which a failure at the entry of a marked
    /// pair may be the pair's own.
    pub(super) fn note_entry_recovered(&self, instance_id: InstanceId) -> RuntimeHostResult<()> {
        let mut blocks = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
        let recovered = blocks.recovered.entry(instance_id).or_insert(0);
        *recovered = recovered.saturating_add(1);
        // No run has started and no manual key was granted since this recovery.
        blocks.started.remove(&instance_id);
        blocks.end(instance_id);
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

    /// A recovery `ladder` ended without recovering (fix round 1: its ladder `exhausted`, its
    /// worker panicked): when it is one of its blocked instance's own recoveries, the instance
    /// stays blocked and is brought to a person's attention, once per block; a scheduled run's own
    /// recovery that ended so before the run's settlement raises it with the block that settlement
    /// sets. Any other ladder's end tells a block nothing.
    pub(super) fn note_entry_recovery_failed(
        &self,
        ladder: &PendingRecoveryLadder,
    ) -> RuntimeHostResult<()> {
        let instance_id = ladder.instance_id();
        let correlation = ladder.correlation();
        let attention = {
            let mut blocks = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
            let own = blocks
                .blocked
                .get(&instance_id)
                .is_some_and(|block| block.recoveries.contains(&correlation));
            if own {
                blocks.blocked.get_mut(&instance_id).and_then(|block| {
                    (!block.awaiting_person).then(|| {
                        block.awaiting_person = true;
                        block.gate.clone()
                    })
                })
            } else {
                if let Some(run_id) = ladder.task_run()
                    && blocks.settling.contains(&run_id)
                {
                    blocks.unrecovered.insert(run_id);
                }
                None
            }
        };
        match attention {
            Some(gate) => self.record_entry_attention(instance_id, &gate, None),
            None => Ok(()),
        }
    }

    /// A recovery `ladder` of `instance_id` held back until `until_unix_ms` (its cool-down's end,
    /// an approaching blackout's end, or fix round 1's next tick) while no probe of an unavailable
    /// instance holds it (Workflow #489 AVAIL-1): a blocked instance's block holds it (it becomes
    /// one of the block's recoveries); for an instance not blocked, one a scheduled run's trigger
    /// names whose settlement has not come yet is kept for that settlement (by the run); any other
    /// is dropped, as before ENTRY-A.
    pub(super) fn hold_entry_recovery(
        &self,
        instance_id: InstanceId,
        ladder: PendingRecoveryLadder,
        until_unix_ms: u64,
    ) -> RuntimeHostResult<()> {
        let correlation = ladder.correlation();
        let run_id = ladder.task_run();
        let held = HeldEntryRecovery {
            ladder,
            until_unix_ms,
        };
        let mut blocks = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
        if let Some(block) = blocks.blocked.get_mut(&instance_id) {
            block.recoveries.insert(correlation);
            block.held = Some(held);
        } else if let Some(run_id) = run_id
            && blocks.settling.contains(&run_id)
        {
            blocks.cooled.insert(run_id, held);
        }
        Ok(())
    }

    /// Fix round 1: a recovery `ladder` a transient obstacle refused (its admission's capacity
    /// refusal, another ladder that holds or waits for the instance, a claim the queue refused, a
    /// later preparation that superseded its own): no failure of the recovery; held as
    /// [`Self::hold_entry_recovery`] holds it, due at once, so its admission is tried again at the
    /// lease sweep's next tick. A person's drill is refused to them instead.
    pub(super) fn hold_entry_recovery_again(
        &self,
        ladder: PendingRecoveryLadder,
    ) -> RuntimeHostResult<()> {
        if ladder.is_drill() {
            return Ok(());
        }
        self.hold_entry_recovery(ladder.instance_id(), ladder, 0)
    }

    /// A recovery Workflow #489 AVAIL-1 held back with `instance_id` while it was unavailable,
    /// which the instance's leaving the unavailable set lets go (`track_unavailability`): an
    /// entry-blocked instance's block takes it over, to take it up at `until_unix_ms` (the window
    /// in force read again then); for any other instance it is dropped, as AVAIL-1 drops it.
    pub(super) fn hand_over_held_recovery(
        &self,
        instance_id: InstanceId,
        ladder: PendingRecoveryLadder,
        until_unix_ms: u64,
    ) -> RuntimeHostResult<()> {
        if let Some(block) = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?
            .blocked
            .get_mut(&instance_id)
        {
            block.recoveries.insert(ladder.correlation());
            block.held = Some(HeldEntryRecovery {
                ladder,
                until_unix_ms,
            });
        }
        Ok(())
    }

    /// The lease sweep's step after the unavailable instances' probes, for each blocked instance:
    ///
    /// - fix round 1, the catch-all: a block that lasted `scheduling.unavailable_probe.
    ///   attention_after_ms` (the one threshold AVAIL-1's attention reads) brings the instance to a
    ///   person's attention, once per block, whatever holds it;
    /// - with a held recovery: a blackout that covers the instance, or a fence that took it, ends
    ///   the block and drops the recovery; a pause or an idle stop that owns the instance drops a
    ///   due one (a resume ends the block); otherwise a due one is admitted as it would have been
    ///   without its hold ([`Self::admit_held_recovery`], every other admission rule as for every
    ///   ladder, with a request id, a causation id, a holder and a claim request of its own). As
    ///   Workflow #489 AVAIL-1's release reads it (fix round 1, item 3), the cool-down that holds
    ///   it is the instance's window in force now, read again here: while a window holds the
    ///   instance (another ladder's climb, a `triggered` admission), a recovery that respects the
    ///   cool-down stays held until its end. Fix round 1: while another ladder holds or waits for
    ///   the instance, or business capacity refuses new work, a due recovery stays held, nothing
    ///   recorded, and is tried again at the next tick.
    ///
    /// Only a failure the Runtime does not survive is returned.
    pub(super) fn take_up_entry_recoveries(&self) -> RuntimeHostResult<()> {
        if self.fatal.is_shutdown_requested() {
            return Ok(());
        }
        let blocked = lock(&self.entry_blocks, HostLocation::TakeUpEntryRecoveries)?
            .blocked
            .keys()
            .copied()
            .collect::<Vec<_>>();
        if blocked.is_empty() {
            return Ok(());
        }
        let now_unix_ms = self.runtime_clock_sample()?.unix_ms;
        let attention_after_ms = self.unavailable_probe_config()?.attention_after_ms;
        for instance_id in blocked {
            let alias = lock(
                &self.registered_instances,
                HostLocation::TakeUpEntryRecoveries,
            )?
            .get(&instance_id)
            .map(|instance| instance.instance_alias.clone());
            let Some(alias) = alias else {
                continue;
            };
            let (late, held_until) = {
                let mut blocks = lock(&self.entry_blocks, HostLocation::TakeUpEntryRecoveries)?;
                let Some(block) = blocks.blocked.get_mut(&instance_id) else {
                    continue;
                };
                let elapsed_ms = now_unix_ms.saturating_sub(block.since_unix_ms);
                let late =
                    (!block.awaiting_person && elapsed_ms >= attention_after_ms).then(|| {
                        block.awaiting_person = true;
                        (block.gate.clone(), elapsed_ms)
                    });
                (late, block.held.as_ref().map(|held| held.until_unix_ms))
            };
            if let Some((gate, elapsed_ms)) = late {
                self.record_entry_attention(
                    instance_id,
                    &gate,
                    Some((elapsed_ms, attention_after_ms)),
                )?;
            }
            let Some(until_unix_ms) = held_until else {
                continue;
            };
            let hold = self.probe_hold(instance_id, &alias)?;
            let fenced = hold == ProbeHold::Owned && self.resource_fenced(instance_id)?;
            if hold == ProbeHold::Blackout || fenced {
                // Nothing is dispatched inside the blackout, and after it the first due task tries
                // its entry anew; a fence and its own recheck own the instance.
                self.end_entry_block(instance_id)?;
                continue;
            }
            if until_unix_ms > now_unix_ms {
                continue;
            }
            if hold == ProbeHold::Owned {
                if let Some(block) = lock(&self.entry_blocks, HostLocation::TakeUpEntryRecoveries)?
                    .blocked
                    .get_mut(&instance_id)
                {
                    block.held = None;
                }
                continue;
            }
            let window_until_unix_ms = self.cooldown_window_until(instance_id)?;
            let ladder_active = lock(&self.scheduler, HostLocation::TakeUpEntryRecoveries)?
                .ladder_active(instance_id);
            let capacity = match self.admit_capacity() {
                Ok(_) => true,
                Err(error) if error.is_fatal() => return Err(error),
                Err(_) => false,
            };
            let held = {
                let mut blocks = lock(&self.entry_blocks, HostLocation::TakeUpEntryRecoveries)?;
                let Some(block) = blocks.blocked.get_mut(&instance_id) else {
                    continue;
                };
                let Some(held) = block.held.as_mut() else {
                    continue;
                };
                if held.until_unix_ms > now_unix_ms {
                    continue;
                }
                if held.ladder.respects_cooldown() && now_unix_ms < window_until_unix_ms {
                    held.until_unix_ms = window_until_unix_ms;
                    continue;
                }
                if ladder_active || !capacity {
                    continue;
                }
                let Some(held) = block.held.take() else {
                    continue;
                };
                block.recoveries.insert(held.ladder.correlation());
                held
            };
            self.admit_held_recovery(held.ladder)?;
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
    /// `runtime.failed` record under the instance's links, handed to the daemon's log too. `late`
    /// (fix round 1): for the block that lasted the threshold, how long it lasted and the
    /// threshold (`elapsed_ms`, `limit_ms`); `None` for a recovery that ended without recovering.
    fn record_entry_attention(
        &self,
        instance_id: InstanceId,
        gate: &EntryBlockGate,
        late: Option<(u64, u64)>,
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
        let outcome = match late {
            Some((elapsed_ms, limit_ms)) => outcome.elapsed_ms(elapsed_ms).limit_ms(limit_ms),
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

    /// Test only: the next settlement of a scheduled run is armed as `armed`.
    #[cfg(test)]
    pub(super) fn settle_next_for_test(&self, armed: SettleForTest) -> RuntimeHostResult<()> {
        lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?.settle_for_test = Some(armed);
        Ok(())
    }

    /// Test only: what the next settlement of the dispatch `decision_id`, failed or succeeded as
    /// `input`, was armed to be; taken once. A run armed as failed at its entry is judged as the
    /// live settlement judges one, against the instance's marks now (the live settlement reads
    /// them at the run's start, which the replays exercise): when it is its pair's own (ENTRY-A2,
    /// the rule's `escalation_threshold` read as the live settlement reads it), the input returned
    /// is `input` with its identity written as a failure that accumulates (no error frame).
    #[cfg(test)]
    pub(super) fn entry_settlement_for_test(
        &self,
        decision_id: &str,
        input: &PolicyExecutionInput,
    ) -> RuntimeHostResult<(Option<EntrySettlement>, Option<PolicyExecutionInput>)> {
        let armed = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?
            .settle_for_test
            .take();
        let Some(armed) = armed else {
            return Ok((None, None));
        };
        if armed == SettleForTest::PastEntry {
            return Ok((Some(EntrySettlement::TargetStarted), None));
        }
        let (instance_alias, task_id, previous, threshold) = {
            let policy = lock(self.policy()?, HostLocation::InstanceEntryBlock)?;
            let instance_alias = policy.execution_instance_id(decision_id)?.to_owned();
            let task_id = policy.execution_task_id_for_test(decision_id)?.to_owned();
            let previous = policy
                .latest_failure(&task_id, &instance_alias)
                .and_then(|(identity, _)| identity.map(str::to_owned));
            let threshold = policy.dispatch_escalation_threshold(decision_id)?;
            (instance_alias, task_id, previous, threshold)
        };
        let instance_id = self
            .resolve_instance(&instance_alias)
            .map_err(|failure| *failure.error)?
            .instance_id();
        let marks = self.entry_marks(instance_id)?;
        let not_own = Some(EntrySettlement::AtEntry { marks, own: false });
        let PolicyExecutionInput::Failed {
            outcome,
            failure_identity: Some(identity),
            class,
        } = input
        else {
            return Ok((not_own, None));
        };
        let Some(parsed) = FailureIdentity::parse(identity) else {
            return Ok((not_own, None));
        };
        if !self.entry_failure_own(
            &instance_alias,
            &task_id,
            marks,
            &parsed.packages(),
            threshold,
        )? {
            return Ok((not_own, None));
        }
        Ok((
            Some(EntrySettlement::AtEntry { marks, own: true }),
            Some(PolicyExecutionInput::Failed {
                outcome: outcome.clone(),
                failure_identity: Some(crate::failure_identity::own_entry_identity(
                    &parsed,
                    None,
                    previous.as_deref(),
                )),
                class: *class,
            }),
        ))
    }

    /// Test only: the runs started and manual keys granted on `instance_id` since its latest
    /// successful recovery.
    #[cfg(test)]
    pub(super) fn entry_runs_started_for_test(
        &self,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<u64> {
        Ok(lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?
            .marks(instance_id)
            .started)
    }

    /// Test only: whether the pair of `task_id` on `instance_alias` is marked as failed at its
    /// entry, and the instance's successful recoveries the mark holds.
    #[cfg(test)]
    pub(super) fn entry_mark_for_test(
        &self,
        instance_alias: &str,
        task_id: &str,
    ) -> RuntimeHostResult<Option<u64>> {
        Ok(lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?
            .entry_failed
            .get(&(instance_alias.to_owned(), task_id.to_owned()))
            .map(|mark| mark.recovered))
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
