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
//! - **The block.** The live settlement settles a failure at the entry (a code of class
//!   `pack_logic_mismatch` only) as a condition of the run's instance (its record's identity
//!   marked `i…`, `policy_control::entry_blocked`) and blocks the instance here, under the policy
//!   outcome gate; the evaluation reads the blocks under the same gate (`entry_block_gates`) and
//!   defers every candidate of a blocked instance but a store update task's (position 4b,
//!   `crate::entry_block`).
//! - **Its end.** A recovery ladder of the instance that ends `recovered` or `environment_ready`,
//!   a run on the instance that succeeds (`append_contained_task_terminal`), a resume that lifts the
//!   instance's pause or the global one, a restart (memory only), a blackout that covers the
//!   instance and a fence that takes it. A recovery the block held when it ends, and (fix round 3,
//!   ruling 7) one a newer recovery replaced, is dropped with the record AVAIL-1 writes for every
//!   recovery it drops (`record_dropped_held_recovery`).
//! - **Whose failure** (fix round 3, ruling 1: what a recovery did is no evidence of anything). A
//!   pair (instance, catalog task) is marked while its task's last scheduled run failed at its
//!   entry, with the packages its identity names, the activity window its admission was committed
//!   in and how many scheduled runs the instance had let in (`entered`) when that failure settled.
//!   The pair's next failure at the entry is its own only when its mark has the same packages and
//!   window and another scheduled run was let in on the instance since (its main package started
//!   after an entry check it passed, or it succeeded): the instance let another task in while it
//!   kept this pair out. Every other failure at the entry is a condition. Any settlement of the
//!   pair's run that is not a failure at the entry, and one that paused the pair, clear the mark.
//! - **The pair yields its turn** (fix round 3, ruling 2). When a block ends, the pair whose failure
//!   set it yields its turn on the instance (`EntryBlocks::yielding`, read by the evaluation with the
//!   blocks) until a run of another task starts there or is settled.
//! - **Where a climb starts and when** (fix round 3, rulings 3 and 5). A stretch of the instance
//!   runs from its first block since a run was last let in (a scheduled run's main package started
//!   after its entry check, a scheduled run, a client's or the Lab's run succeeded) until the next
//!   such run. A block set while nothing was let in since the instance's previous block ended climbs
//!   from the first rung that restarts the application. The first climb of a stretch starts as
//!   configured; every later one waits `scheduling.unavailable_probe`'s interval for the number of
//!   climbs the stretch has had, from the previous climb's end, and never before the instance's
//!   cool-down window ends, whatever ended the previous climb (`recovered` or `environment_ready`
//!   with nothing let in, `exhausted`, a holding interruption). A person's pause holds every climb
//!   and its lifting ends the block; after the game's update dialog ended a rung's run (ruling 4)
//!   nothing climbs again in the stretch; after a panicked worker nothing climbs again in the block.
//! - **Its way out.** A held recovery is taken up by the lease sweep (`take_up_entry_recoveries`,
//!   after the unavailable instances' probes; the mechanism of Workflow #489 AVAIL-1) once it is due
//!   and nothing holds the instance. A transient obstacle (the admission's capacity refusal,
//!   another ladder that holds or waits for the instance) is judged before anything is recorded; a
//!   claim the queue refused is tried again on a growing interval, one second at first, doubling to
//!   one minute, its refusal recorded when it first occurs in the block and when its cause changes.
//!   One of the block's own recoveries that ends without recovering tells a person once per block
//!   (`instance_entry_blocked_recovery_failed`).
//! - **The attention.** Once a stretch lasted `scheduling.unavailable_probe.attention_after_ms`,
//!   while the instance is blocked and no pause, fence or idle stop owns it (as AVAIL-1's
//!   attention), a person is told once per stretch (`instance_entry_blocked_awaiting_person`). Each
//!   warning is a record and the daemon's log.
//! - **Races.** A scheduled run's start keeps how many blocks of its instance ended, and its
//!   settlement compares them: anything that ended a block between the run's start and its
//!   settlement leaves no block. A recovery of the run's own failure that ended before the
//!   settlement acts with the block that settlement sets.
//!
//! Memory only: a restart starts with no block, no mark, no yield and no stretch (its preparation
//! proves the instance again and its startup package runs where one is configured). A leaf lock.

use super::recovery_ladder::PendingRecoveryLadder;
use super::unavailable_probe::ProbeHold;
use super::*;
use crate::codes::{EntryRecoveryEnd, HostCode, HostLocation};
use crate::entry_block::{EntryBlockGate, EntryBlockGates};
use crate::failure_identity::{FailureIdentity, identifier_text};
use actingcommand_contract::outcome::{CodeStr, Outcome};
use actingcommand_runtime_config::UnavailableProbeConfig;

/// Fix round 2 (ruling 7): the wait before a block's recovery whose claim the queue refused is
/// tried again, at first; it doubles at each refusal in the block.
const CLAIM_RETRY_FIRST_MS: u64 = 1_000;
/// Fix round 2 (ruling 7): the longest such wait.
const CLAIM_RETRY_MAX_MS: u64 = 60_000;

/// What a scheduled run's start knew of its instance: how many blocks of it ended.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct EntryMarks {
    ended: u64,
}

/// Workflow #490 ENTRY-A: what a scheduled run's settlement tells its instance's block and its
/// pair's mark. A settlement without one (a stop, a reconciled or interrupted run, a run whose
/// chain was not resolved, a failure of another class) is no failure at the entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EntrySettlement {
    /// The run failed at its entry, with what its start knew of its instance; `own` when the
    /// failure is its pair's own ([`HostShared::entry_failure_own`]); `window_end` the end of the
    /// activity window its admission was committed in (`0` when the policy knows none).
    AtEntry {
        marks: EntryMarks,
        own: bool,
        window_end: u64,
    },
    /// The run's main package started, whatever its end; `entered` (fix round 3) when it started
    /// after an entry check it passed (an `EntryTargetDisposition` `Started`), which lets the run
    /// in on its instance.
    TargetStarted { entered: bool },
}

/// Fix round 3 (rulings 3, 4 and 5): how the climb of a block that a scheduled run's failure at
/// its entry would set now goes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct EntryClimb {
    /// From the first rung that restarts the application (nothing let in since the instance's
    /// previous block ended).
    pub(super) restarts: bool,
    /// A later climb of the instance's stretch (or one in a stretch the update dialog stopped):
    /// held, never admitted at once, due no sooner than this time.
    pub(super) paced: Option<u64>,
}

/// A pair's mark: its task's last scheduled run failed at its entry.
struct EntryFailureMark {
    /// Fix round 3 (ruling 1): the scheduled runs the instance had let in when that failure
    /// settled.
    entered: u64,
    /// The packages its failure identity named ([`FailureIdentity::packages`]).
    packages: String,
    /// The end of the activity window its admission was committed in.
    window_end: u64,
}

/// Test only: what the next settlement is armed to be.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettleForTest {
    /// One of a run that failed at its entry, judged against the instance's marks then.
    AtEntry,
    /// One of a run whose main package started after its entry check.
    PastEntry,
}

/// A recovery held with a block (or for a scheduled run's settlement), and when it is due.
struct HeldEntryRecovery {
    ladder: PendingRecoveryLadder,
    until_unix_ms: u64,
    /// The block's own climb after a climb that ended without recovering or was interrupted,
    /// which waits for the instance's cool-down window whatever its trigger item says of it.
    retry: bool,
    /// For a recovery held for a settlement still to come, the cause of the queue's refusal of
    /// its claim that was recorded, which the block it joins remembers.
    refused: Option<&'static str>,
    /// Fix round 3 (ruling 3): for a recovery held for a settlement still to come after its climb
    /// ended interrupted while the instance had no stretch, when that climb ended: the block the
    /// settlement sets counts it.
    climbed: Option<u64>,
}

impl HeldEntryRecovery {
    fn new(ladder: PendingRecoveryLadder, until_unix_ms: u64) -> Self {
        Self {
            ladder,
            until_unix_ms,
            retry: false,
            refused: None,
            climbed: None,
        }
    }
}

/// How one of a scheduled run's own recoveries ended without recovering before its settlement.
struct FailedEntryRecovery {
    end: EntryRecoveryEnd,
    ladder: PendingRecoveryLadder,
    ended_at_unix_ms: u64,
    /// Fix round 3 (ruling 3): its climb was counted in the instance's stretch when it ended.
    counted: bool,
}

/// One blocked instance.
struct EntryBlock {
    /// The latest failure at the entry that blocked it.
    gate: EntryBlockGate,
    /// A person was told one of the block's own recoveries ended without recovering
    /// (`instance_entry_blocked_recovery_failed`), once per block.
    recovery_failed: bool,
    held: Option<HeldEntryRecovery>,
    /// The trigger correlations of the block's own recoveries (the blocking runs' lease ends, the
    /// recoveries it held or took over): only one of these ending without recovering tells a
    /// person; another trigger's ladder does not.
    recoveries: BTreeSet<CorrelationId>,
    /// Set while no run was let in on the instance since its previous block ended: the block's
    /// recoveries climb from the application restart.
    restarts: bool,
    /// A worker climbing one of its recoveries panicked: no further climb in the block.
    panicked: bool,
    /// The queue's refusals of its recovery's claims, and the cause of the latest one recorded.
    refusals: u32,
    refused: Option<&'static str>,
}

impl EntryBlock {
    /// Holds `held` as one of the block's own recoveries; returns the recovery it replaced, which
    /// the caller records as dropped (fix round 3, ruling 7).
    fn hold(&mut self, held: HeldEntryRecovery) -> Option<HeldEntryRecovery> {
        self.recoveries.insert(held.ladder.correlation());
        self.held.replace(held)
    }
}

/// A stretch of blocks with nothing let in between them.
struct Stretch {
    /// When its first block was set (the Runtime's clock).
    since_unix_ms: u64,
    /// A person was told (`instance_entry_blocked_awaiting_person`), once per stretch.
    told: bool,
    /// Fix round 3 (ruling 3): the climbs of its blocks' recoveries that ended, and when the
    /// latest ended.
    climbs: u32,
    last_end_unix_ms: u64,
    /// Fix round 3 (ruling 4): the game's update dialog ended one of its climbs: no climb again.
    update_dialog: bool,
}

impl Stretch {
    const fn new(since_unix_ms: u64) -> Self {
        Self {
            since_unix_ms,
            told: false,
            climbs: 0,
            last_end_unix_ms: 0,
            update_dialog: false,
        }
    }

    /// Fix round 3 (ruling 3): when its next climb is due at the earliest, after one climb at
    /// least: `config`'s interval for the climbs it had, from the latest one's end.
    fn paced_until(&self, config: UnavailableProbeConfig) -> Option<u64> {
        (self.climbs > 0).then(|| {
            self.last_end_unix_ms
                .saturating_add(config.interval_ms(self.climbs))
        })
    }
}

/// The blocks, the marks, the yields and the stretches (memory only). A leaf lock: nothing else is
/// locked while it is held.
#[derive(Default)]
pub(super) struct EntryBlocks {
    blocked: BTreeMap<InstanceId, EntryBlock>,
    ended: BTreeMap<InstanceId, u64>,
    /// Fix round 3 (ruling 1): the scheduled runs let in on the instance, as their settlements
    /// tell (a success, a main package started after an entry check it passed).
    entered: BTreeMap<InstanceId, u64>,
    /// Fix round 3 (rulings 3 and 5): the runs let in on the instance: those of `entered`, and a
    /// run that succeeded there that is not the Runtime's own (a client's or the Lab's).
    let_in: BTreeMap<InstanceId, u64>,
    /// `let_in` when the instance's latest block ended.
    ended_let_in: BTreeMap<InstanceId, u64>,
    /// The instance's stretch of blocks, while one runs.
    stretches: BTreeMap<InstanceId, Stretch>,
    /// Fix round 3 (ruling 2): the gate of the instance's latest block that ended, while its pair
    /// (`task_id`) yields its turn there.
    yielding: BTreeMap<InstanceId, EntryBlockGate>,
    /// The pairs' marks, by instance alias and catalog task.
    entry_failed: BTreeMap<(String, String), EntryFailureMark>,
    /// The scheduled runs started whose settlement has not come yet, with their dispatch.
    settling: BTreeMap<RunId, String>,
    /// A recovery held for a scheduled run whose settlement has not come yet (its instance not
    /// blocked yet), by the run: the settlement takes it when it blocks the instance and lets it go
    /// otherwise.
    cooled: BTreeMap<RunId, HeldEntryRecovery>,
    /// The scheduled runs whose own recovery ended without recovering before their settlement,
    /// and how: the block that settlement sets acts on it at once.
    unrecovered: BTreeMap<RunId, FailedEntryRecovery>,
    /// Test only: what the next settlement is armed to be.
    #[cfg(test)]
    settle_for_test: Option<SettleForTest>,
}

/// The count `counts` holds for `instance_id`.
fn count(counts: &BTreeMap<InstanceId, u64>, instance_id: InstanceId) -> u64 {
    counts.get(&instance_id).copied().unwrap_or(0)
}

/// One more in `counts` for `instance_id`.
fn bump(counts: &mut BTreeMap<InstanceId, u64>, instance_id: InstanceId) {
    let counted = counts.entry(instance_id).or_insert(0);
    *counted = counted.saturating_add(1);
}

impl EntryBlocks {
    fn marks(&self, instance_id: InstanceId) -> EntryMarks {
        EntryMarks {
            ended: count(&self.ended, instance_id),
        }
    }

    /// Whether a recovery of `instance_id`'s block climbs from the application restart: the
    /// instance's block says so, or with no block, its previous block ended and no run was let in
    /// on it since.
    fn restart_due(&self, instance_id: InstanceId) -> bool {
        match self.blocked.get(&instance_id) {
            Some(block) => block.restarts,
            None => self.ended_let_in.get(&instance_id) == Some(&count(&self.let_in, instance_id)),
        }
    }

    /// Ends the block of `instance_id`, if it has one, and returns the recovery it held; counted
    /// for a settlement still to come. Fix round 3 (ruling 2): the pair whose failure set it yields
    /// its turn there.
    fn end(&mut self, instance_id: InstanceId) -> Option<HeldEntryRecovery> {
        bump(&mut self.ended, instance_id);
        let block = self.blocked.remove(&instance_id)?;
        self.ended_let_in
            .insert(instance_id, count(&self.let_in, instance_id));
        self.yielding.insert(instance_id, block.gate);
        block.held
    }

    /// A run was let in on `instance_id` (`scheduled`: a scheduled run, which is evidence against a
    /// pair kept out): its stretch ends unless the instance is still blocked.
    fn note_let_in(&mut self, instance_id: InstanceId, scheduled: bool) {
        bump(&mut self.let_in, instance_id);
        if scheduled {
            bump(&mut self.entered, instance_id);
        }
        if !self.blocked.contains_key(&instance_id) {
            self.stretches.remove(&instance_id);
        }
    }

    /// Fix round 3 (ruling 2): a run of the task `task_id` started or was settled on
    /// `instance_id`: the yield of another task's pair there ends.
    fn note_task_run(&mut self, instance_id: InstanceId, task_id: &str) {
        if self
            .yielding
            .get(&instance_id)
            .is_some_and(|gate| gate.task_id != task_id)
        {
            self.yielding.remove(&instance_id);
        }
    }

    /// Fix round 3 (ruling 3): whether the end of `ladder`'s climb counts in its instance's
    /// stretch: one runs, and the ladder is a scheduled run's (its failure's own) or one of the
    /// block's own recoveries.
    fn counts_climb(&self, instance_id: InstanceId, ladder: &PendingRecoveryLadder) -> bool {
        self.stretches.contains_key(&instance_id)
            && (ladder.task_run().is_some()
                || self
                    .blocked
                    .get(&instance_id)
                    .is_some_and(|block| block.recoveries.contains(&ladder.correlation())))
    }

    /// Fix round 3 (ruling 3): a climb of `instance_id`'s stretch ended at `at`.
    fn climb_ended(&mut self, instance_id: InstanceId, at: u64) {
        if let Some(stretch) = self.stretches.get_mut(&instance_id) {
            stretch.climbs = stretch.climbs.saturating_add(1);
            stretch.last_end_unix_ms = at;
        }
    }

    /// One of `instance_id`'s block's own recoveries, `ladder`, ended without recovering as `end`
    /// says at `ended_at_unix_ms`, its climb counted already. `exhausted`: the next climb is held
    /// from the application restart until the stretch's paced time (`config`; fix round 3, ruling
    /// 3), unless a recovery is held already; the update dialog (ruling 4): nothing climbs again in
    /// the stretch, a held recovery is let go; a panicked worker: nothing climbs again in the
    /// block, a held recovery is let go. Returns when the next climb is due, if one is, and the
    /// recovery let go.
    fn recovery_ended(
        &mut self,
        instance_id: InstanceId,
        end: EntryRecoveryEnd,
        ladder: &PendingRecoveryLadder,
        ended_at_unix_ms: u64,
        config: UnavailableProbeConfig,
    ) -> (Option<u64>, Option<HeldEntryRecovery>) {
        let stretch = self.stretches.get_mut(&instance_id);
        let paced = stretch
            .as_ref()
            .and_then(|stretch| stretch.paced_until(config))
            .unwrap_or_else(|| ended_at_unix_ms.saturating_add(config.interval_ms(1)));
        if let Some(stretch) = stretch
            && matches!(end, EntryRecoveryEnd::UpdateDialog)
        {
            stretch.update_dialog = true;
        }
        let Some(block) = self.blocked.get_mut(&instance_id) else {
            return (None, None);
        };
        match end {
            EntryRecoveryEnd::Exhausted => {
                if block.panicked {
                    return (None, None);
                }
                if let Some(held) = &block.held {
                    return (Some(held.until_unix_ms.max(paced)), None);
                }
                let mut again = ladder.clone();
                again.start_at_application_restart();
                let replaced = block.hold(HeldEntryRecovery {
                    retry: true,
                    ..HeldEntryRecovery::new(again, paced)
                });
                (Some(paced), replaced)
            }
            EntryRecoveryEnd::UpdateDialog => (None, block.held.take()),
            EntryRecoveryEnd::WorkerPanicked => {
                block.panicked = true;
                (None, block.held.take())
            }
        }
    }
}

/// The wait before the claim of a block's recovery is tried again after the block's
/// `refusals`-th refusal (from 0): one second, doubling, at most one minute.
fn claim_retry_wait_ms(refusals: u32) -> u64 {
    CLAIM_RETRY_FIRST_MS
        .saturating_mul(1_u64 << refusals.min(6))
        .min(CLAIM_RETRY_MAX_MS)
}

/// One blocked instance as a test reads it: its gate, whether a person was told of it (either
/// warning) and when its held recovery is due.
#[cfg(test)]
pub(crate) type EntryBlockForTest = (EntryBlockGate, bool, Option<u64>);

impl HostShared {
    /// The marks of `instance_id` now.
    #[cfg(test)]
    pub(super) fn entry_marks(&self, instance_id: InstanceId) -> RuntimeHostResult<EntryMarks> {
        Ok(lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?.marks(instance_id))
    }

    /// A scheduled run `run_id` of the dispatch `decision_id` of the catalog task `task_id` on
    /// `instance_id` starts (its resolution is registered): the marks of its instance now, which
    /// its settlement compares; its settlement is due, so a recovery held for it is kept until
    /// then; and (fix round 3, ruling 2) a run of another task starts there, so the pair that
    /// yields its turn there yields no longer.
    pub(super) fn entry_settlement_pending(
        &self,
        run_id: RunId,
        decision_id: &str,
        task_id: &str,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<EntryMarks> {
        let mut blocks = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
        blocks.settling.insert(run_id, decision_id.to_owned());
        blocks.note_task_run(instance_id, task_id);
        Ok(blocks.marks(instance_id))
    }

    /// The dispatch of the scheduled run `run_id`, while its settlement is still to come.
    pub(super) fn entry_settling_dispatch(
        &self,
        run_id: RunId,
    ) -> RuntimeHostResult<Option<String>> {
        Ok(lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?
            .settling
            .get(&run_id)
            .cloned())
    }

    /// Fix round 3 (rulings 3, 4 and 5): how the climb of a block of `instance_id` set now goes:
    /// from the application restart when nothing was let in since its previous block ended; held
    /// until the stretch's paced time when the stretch had a climb already (or the update dialog
    /// stopped its climbs).
    pub(super) fn entry_climb(&self, instance_id: InstanceId) -> RuntimeHostResult<EntryClimb> {
        let config = self.unavailable_probe_config()?;
        let blocks = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
        let paced = blocks
            .stretches
            .get(&instance_id)
            .filter(|stretch| stretch.climbs > 0 || stretch.update_dialog)
            .map(|stretch| stretch.paced_until(config).unwrap_or(0));
        Ok(EntryClimb {
            restarts: blocks.restart_due(instance_id),
            paced,
        })
    }

    /// Fix round 3 (ruling 1; `contracts/policy-suspension.md`, "Whose failure"): whether a failure
    /// at the entry of `task_id` on `instance_alias` (`instance_id`), whose identity names
    /// `packages` and whose admission was committed in the activity window ending at
    /// `window_end`, is the pair's own: the pair's mark is set (its task's last scheduled run
    /// failed at its entry) with the same packages, in the same window, and the instance let
    /// another scheduled run in since that failure. Nothing else is evidence: not a recovery of
    /// any kind, not a count of conditions, not who held the instance's key.
    pub(super) fn entry_failure_own(
        &self,
        instance_id: InstanceId,
        instance_alias: &str,
        task_id: &str,
        packages: &str,
        window_end: u64,
    ) -> RuntimeHostResult<bool> {
        let blocks = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
        let entered = count(&blocks.entered, instance_id);
        Ok(blocks
            .entry_failed
            .get(&(instance_alias.to_owned(), task_id.to_owned()))
            .is_some_and(|mark| {
                mark.packages == packages && mark.window_end == window_end && entered > mark.entered
            }))
    }

    /// Workflow #490 ENTRY-A: what the execution record `data`, recorded now, tells its pair's mark
    /// and its instance's block (`entry` what the settlement knew of the run, `context` the run
    /// when the settlement had it). A success or a main package started after an entry check lets
    /// a run in on the instance (fix round 3: the evidence count); a run of another task ends the
    /// yield of the pair there; a failure at the entry sets the pair's mark unless it paused the
    /// pair, and every other settlement clears it; a failure at the entry blocks the instance when
    /// it settled as a condition or as the pair's own failure (a configured condition of
    /// `scheduling.rerun` blocks nothing).
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
        let let_in = matches!(data.outcome, PolicyExecutionOutcome::Succeeded { .. })
            || entry == Some(EntrySettlement::TargetStarted { entered: true });
        let at_entry = match entry {
            Some(EntrySettlement::AtEntry {
                marks,
                own,
                window_end,
            }) => Some((marks, own, window_end)),
            Some(EntrySettlement::TargetStarted { .. }) | None => None,
        };
        let blocks_instance = match (at_entry, failure) {
            (Some((_, own, _)), Some(failure)) => {
                crate::policy_control::entry_blocked(failure)
                    || (own && failure.condition.is_none())
            }
            _ => false,
        };
        let run = context.map(|context| (context.run_id(), context.correlation_id()));
        let instance = self.settled_instance(data, context)?;
        {
            let mut guard = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
            let blocks = &mut *guard;
            if let Some(instance_id) = instance {
                if let_in {
                    blocks.note_let_in(instance_id, true);
                }
                blocks.note_task_run(instance_id, &data.task_id);
            }
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
            match at_entry {
                Some((_, _, window_end)) if !paused => {
                    let packages = failure
                        .and_then(|failure| failure.failure_identity.as_deref())
                        .and_then(FailureIdentity::parse)
                        .map(|identity| identity.packages())
                        .unwrap_or_default();
                    let entered =
                        instance.map_or(0, |instance_id| count(&blocks.entered, instance_id));
                    blocks.entry_failed.insert(
                        key,
                        EntryFailureMark {
                            entered,
                            packages,
                            window_end,
                        },
                    );
                }
                _ => {
                    blocks.entry_failed.remove(&key);
                }
            }
        }
        let (true, Some((marks, _, _)), Some(failure)) = (blocks_instance, at_entry, failure)
        else {
            return Ok(());
        };
        self.block_instance_entry(
            data,
            actingcommand_contract::scheduled_failure_code_str(&failure.outcome),
            run,
            marks,
        )
    }

    /// The instance a settled execution record `data` ran on: its run's lease's when the
    /// settlement had the run (`context`), else the instance its alias names now; `None` for an
    /// alias no instance has (nothing to count on it).
    fn settled_instance(
        &self,
        data: &PolicyExecutionEventData,
        context: Option<&PolicyRunContext>,
    ) -> RuntimeHostResult<Option<InstanceId>> {
        if let Some(context) = context {
            return Ok(Some(context.lease_token().instance_id()));
        }
        match self.resolve_instance(&data.instance_id) {
            Ok(resolved) => Ok(Some(resolved.instance_id())),
            Err(failure) if failure.poison_runtime || failure.error.is_fatal() => {
                Err(*failure.error)
            }
            Err(_) => Ok(None),
        }
    }

    /// Blocks the instance of `data`, a run's execution record settled now as failed at its entry
    /// (`failure_code` its failure's code, `run` the run and its correlation when the settlement
    /// had its context, `marks` what the run's start knew of the instance). A block of the
    /// instance that ended since the run's start leaves it unblocked; a recovery held for this run
    /// is the block's; the run's own recovery that ended without recovering since then acts with
    /// the block at once (`instance_entry_blocked_recovery_failed` once per block). A new block
    /// starts the instance's stretch when none runs, counts there a climb of the run's own
    /// recovery that ended before with no stretch to count it in (fix round 3, ruling 3), and
    /// climbs from the application restart when nothing was let in since the previous block ended.
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
        let now_unix_ms = self.runtime_clock_sample()?.unix_ms;
        let config = self.unavailable_probe_config()?;
        let gate = EntryBlockGate {
            task_id: data.task_id.clone(),
            decision_id: data.decision_id.clone(),
            failure_code,
            since_unix_ms: data.observed_at_unix_ms,
        };
        let (told, dropped) = {
            let mut guard = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
            let blocks = &mut *guard;
            let now = blocks.marks(instance_id);
            let (cooled, failed) = match run {
                Some((run_id, _)) => (
                    blocks.cooled.remove(&run_id),
                    blocks.unrecovered.remove(&run_id),
                ),
                None => (None, None),
            };
            if now.ended != marks.ended {
                // A recovery or a run of the instance succeeded, a resume came, a blackout covered
                // it or a fence took it after the run began: nothing is left to block.
                return Ok(());
            }
            let restarts = blocks.restart_due(instance_id);
            blocks
                .stretches
                .entry(instance_id)
                .or_insert_with(|| Stretch::new(now_unix_ms));
            let uncounted = cooled.as_ref().and_then(|held| held.climbed).or_else(|| {
                failed
                    .as_ref()
                    .filter(|failed| {
                        !failed.counted && !matches!(failed.end, EntryRecoveryEnd::WorkerPanicked)
                    })
                    .map(|failed| failed.ended_at_unix_ms)
            });
            if let Some(at) = uncounted {
                blocks.climb_ended(instance_id, at);
            }
            let refused = cooled.as_ref().and_then(|held| held.refused);
            let replaced = {
                let block = blocks
                    .blocked
                    .entry(instance_id)
                    .or_insert_with(|| EntryBlock {
                        gate: gate.clone(),
                        recovery_failed: false,
                        held: None,
                        recoveries: BTreeSet::new(),
                        restarts,
                        panicked: false,
                        refusals: u32::from(refused.is_some()),
                        refused,
                    });
                block.gate = gate.clone();
                if let Some((_, correlation)) = run {
                    block.recoveries.insert(correlation);
                }
                cooled.and_then(|held| block.hold(held))
            };
            match failed {
                Some(failed) => {
                    let (next, let_go) = blocks.recovery_ended(
                        instance_id,
                        failed.end,
                        &failed.ladder,
                        failed.ended_at_unix_ms,
                        config,
                    );
                    let first = blocks
                        .blocked
                        .get_mut(&instance_id)
                        .is_some_and(|block| !std::mem::replace(&mut block.recovery_failed, true));
                    let told = first.then(|| {
                        (
                            gate,
                            failed.end,
                            failed.ladder.rule().map(str::to_owned),
                            next,
                        )
                    });
                    (told, [replaced, let_go])
                }
                None => (None, [replaced, None]),
            }
        };
        self.record_dropped_entry_recoveries(dropped)?;
        match told {
            Some((gate, end, rule, next)) => {
                self.record_entry_recovery_failed(instance_id, &gate, end, rule.as_deref(), next)
            }
            None => Ok(()),
        }
    }

    /// Ends the entry block of `instance_id`, if it has one, and drops its held recovery with
    /// its record: a resume lifted a pause of it, or a blackout covers it or a fence took it.
    /// Counted for a settlement still to come.
    pub(super) fn end_entry_block(&self, instance_id: InstanceId) -> RuntimeHostResult<()> {
        let dropped = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?.end(instance_id);
        self.record_dropped_entry_recoveries([dropped])
    }

    /// A run on `instance_id` succeeded (a scheduled run, a store update task's, a client's or the
    /// Lab's run, a startup package, a rung's run): it ends the instance's entry block; fix round 3
    /// (rulings 3 and 5): with `let_in`, a run that is not the Runtime's own, it lets a run in on
    /// the instance, which ends the instance's stretch, so its next block climbs as configured.
    pub(super) fn note_entry_success(
        &self,
        instance_id: InstanceId,
        let_in: bool,
    ) -> RuntimeHostResult<()> {
        let dropped = {
            let mut blocks = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
            let dropped = blocks.end(instance_id);
            if let_in {
                blocks.note_let_in(instance_id, false);
            }
            dropped
        };
        self.record_dropped_entry_recoveries([dropped])
    }

    /// Recovered recoveries the blocks let go or replaced (fix round 2, ruling 9; fix round 3,
    /// ruling 7) are recorded as Workflow #489 AVAIL-1 records every recovery it drops
    /// (`record_dropped_held_recovery`: its trigger's `recovery_ladder_suppressed` `cooldown` once
    /// more, `until_unix_ms` the time it was held to).
    fn record_dropped_entry_recoveries(
        &self,
        dropped: impl IntoIterator<Item = Option<HeldEntryRecovery>>,
    ) -> RuntimeHostResult<()> {
        for held in dropped.into_iter().flatten() {
            self.record_dropped_held_recovery(&held.ladder, held.until_unix_ms)?;
        }
        Ok(())
    }

    /// A recovery `ladder` ended `recovered` or `environment_ready`: its climb counts in its
    /// instance's stretch (fix round 3, ruling 3: the next climb of the stretch waits its interval
    /// from this end unless a run is let in first), and it ends the instance's entry block. It is
    /// no evidence against a pair (ruling 1).
    pub(super) fn note_entry_climb_ended_block(
        &self,
        ladder: &PendingRecoveryLadder,
    ) -> RuntimeHostResult<()> {
        let instance_id = ladder.instance_id();
        let ended_at_unix_ms = self.runtime_clock_sample()?.unix_ms;
        let dropped = {
            let mut blocks = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
            if blocks.counts_climb(instance_id, ladder) {
                blocks.climb_ended(instance_id, ended_at_unix_ms);
            }
            blocks.end(instance_id)
        };
        self.record_dropped_entry_recoveries([dropped])
    }

    /// Fix round 3, test only: a scheduled run was let in on `instance_id`, as its settlement
    /// tells the blocks.
    #[cfg(test)]
    pub(super) fn note_entry_let_in_for_test(
        &self,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<()> {
        lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?.note_let_in(instance_id, true);
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

    /// A recovery `ladder` ended without recovering, as `end` says (its ladder `exhausted`, the
    /// game's update dialog ended a rung's run, its worker panicked). Fix round 3 (ruling 3): an
    /// exhausted climb or one the update dialog ended counts in its instance's stretch. When it is
    /// one of its blocked instance's own recoveries, the instance stays blocked and
    /// `instance_entry_blocked_recovery_failed` tells a person, once per block; after an exhausted
    /// climb the block holds its next one, paced (`EntryBlocks::recovery_ended`); after the update
    /// dialog nothing climbs again in the stretch (ruling 4); after a panicked worker nothing
    /// climbs again in the block. A scheduled run's own recovery that ended so before the run's
    /// settlement does the same with the block that settlement sets. Any other ladder's end tells
    /// a block nothing.
    pub(super) fn note_entry_recovery_failed(
        &self,
        ladder: &PendingRecoveryLadder,
        end: EntryRecoveryEnd,
    ) -> RuntimeHostResult<()> {
        let instance_id = ladder.instance_id();
        let correlation = ladder.correlation();
        let ended_at_unix_ms = self.runtime_clock_sample()?.unix_ms;
        let config = self.unavailable_probe_config()?;
        let (told, dropped) = {
            let mut guard = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
            let blocks = &mut *guard;
            let counted = !matches!(end, EntryRecoveryEnd::WorkerPanicked)
                && blocks.counts_climb(instance_id, ladder);
            if counted {
                blocks.climb_ended(instance_id, ended_at_unix_ms);
            }
            let own = blocks
                .blocked
                .get(&instance_id)
                .is_some_and(|block| block.recoveries.contains(&correlation));
            if !own {
                if let Some(run_id) = ladder.task_run()
                    && blocks.settling.contains_key(&run_id)
                {
                    blocks.unrecovered.insert(
                        run_id,
                        FailedEntryRecovery {
                            end,
                            ladder: ladder.clone(),
                            ended_at_unix_ms,
                            counted,
                        },
                    );
                }
                return Ok(());
            }
            let (next, dropped) =
                blocks.recovery_ended(instance_id, end, ladder, ended_at_unix_ms, config);
            let told = blocks.blocked.get_mut(&instance_id).and_then(|block| {
                let first = !std::mem::replace(&mut block.recovery_failed, true);
                first.then(|| (block.gate.clone(), next))
            });
            (told, dropped)
        };
        self.record_dropped_entry_recoveries([dropped])?;
        match told {
            Some((gate, next)) => {
                self.record_entry_recovery_failed(instance_id, &gate, end, ladder.rule(), next)
            }
            None => Ok(()),
        }
    }

    /// A recovery `ladder` of `instance_id` held back until `until_unix_ms` (its cool-down's end,
    /// an approaching blackout's end, a transient refusal's next tick, or, fix round 3, the
    /// stretch's paced time of a later climb its lease end did not admit) while no probe of an
    /// unavailable instance holds it (Workflow #489 AVAIL-1): a blocked instance's block holds it
    /// (it becomes one of the block's recoveries); for an instance not blocked, one a scheduled
    /// run's trigger names whose settlement has not come yet is kept for that settlement (by the
    /// run); any other is dropped, as before ENTRY-A. A recovery it replaces is recorded as
    /// dropped (fix round 3, ruling 7).
    pub(super) fn hold_entry_recovery(
        &self,
        instance_id: InstanceId,
        ladder: PendingRecoveryLadder,
        until_unix_ms: u64,
    ) -> RuntimeHostResult<()> {
        let run_id = ladder.task_run();
        let held = HeldEntryRecovery::new(ladder, until_unix_ms);
        let replaced = {
            let mut blocks = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
            if let Some(block) = blocks.blocked.get_mut(&instance_id) {
                block.hold(held)
            } else if let Some(run_id) = run_id
                && blocks.settling.contains_key(&run_id)
            {
                blocks.cooled.insert(run_id, held)
            } else {
                None
            }
        };
        self.record_dropped_entry_recoveries([replaced])
    }

    /// A recovery `ladder` a transient obstacle refused (its admission's capacity refusal,
    /// another ladder that holds or waits for the instance, a later preparation that superseded
    /// its own): no failure of the recovery; held as [`Self::hold_entry_recovery`] holds it, due
    /// at once, so its admission is tried again at the lease sweep's next tick once nothing holds
    /// the instance (a later climb of the stretch still paced there). A person's drill is refused
    /// to them instead.
    pub(super) fn hold_entry_recovery_again(
        &self,
        ladder: PendingRecoveryLadder,
    ) -> RuntimeHostResult<()> {
        if ladder.is_drill() {
            return Ok(());
        }
        self.hold_entry_recovery(ladder.instance_id(), ladder, 0)
    }

    /// A recovery `ladder` whose climb a holding interruption ended (a pause, business capacity, a
    /// drain, a shutdown): no failure of the recovery, so no person is told. Fix round 3 (ruling
    /// 3): its climb counts in the instance's stretch, and it is held as the block's own climb
    /// after another, so it is taken up no sooner than the stretch's interval from this end
    /// allows and never before the instance's cool-down window ends; under a pause nothing climbs
    /// and the resume ends the block. A person's drill is refused to them instead.
    pub(super) fn hold_interrupted_entry_recovery(
        &self,
        ladder: PendingRecoveryLadder,
    ) -> RuntimeHostResult<()> {
        if ladder.is_drill() {
            return Ok(());
        }
        let instance_id = ladder.instance_id();
        let run_id = ladder.task_run();
        let ended_at_unix_ms = self.runtime_clock_sample()?.unix_ms;
        let replaced = {
            let mut guard = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
            let blocks = &mut *guard;
            let counted = blocks.counts_climb(instance_id, &ladder);
            if counted {
                blocks.climb_ended(instance_id, ended_at_unix_ms);
            }
            let held = HeldEntryRecovery {
                retry: true,
                climbed: (!counted).then_some(ended_at_unix_ms),
                ..HeldEntryRecovery::new(ladder, 0)
            };
            if let Some(block) = blocks.blocked.get_mut(&instance_id) {
                block.hold(held)
            } else if let Some(run_id) = run_id
                && blocks.settling.contains_key(&run_id)
            {
                blocks.cooled.insert(run_id, held)
            } else {
                None
            }
        };
        self.record_dropped_entry_recoveries([replaced])
    }

    /// Fix round 2 (ruling 7): the queue refused the claim of a recovery `ladder` for `cause`.
    /// A blocked instance's block holds it again, due after one second at its first refusal in the
    /// block, the wait doubling at each refusal to at most one minute; one held for a scheduled
    /// run's settlement still to come is kept for it, due after a second. Returns whether the
    /// refusal is recorded: when it first occurs in the block and when its cause changes, never
    /// for a retry with the same cause; any other ladder's refusal is recorded as before ENTRY-A
    /// (and a person's drill's, refused to them).
    pub(super) fn hold_refused_entry_recovery(
        &self,
        ladder: PendingRecoveryLadder,
        cause: &'static str,
    ) -> RuntimeHostResult<bool> {
        if ladder.is_drill() {
            return Ok(true);
        }
        let now_unix_ms = self.runtime_clock_sample()?.unix_ms;
        let instance_id = ladder.instance_id();
        let run_id = ladder.task_run();
        let (recorded, replaced) = {
            let mut blocks = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
            if let Some(block) = blocks.blocked.get_mut(&instance_id) {
                let recorded = block.refused != Some(cause);
                block.refused = Some(cause);
                let wait_ms = claim_retry_wait_ms(block.refusals);
                block.refusals = block.refusals.saturating_add(1);
                let replaced = block.hold(HeldEntryRecovery::new(
                    ladder,
                    now_unix_ms.saturating_add(wait_ms),
                ));
                (recorded, replaced)
            } else if let Some(run_id) = run_id
                && blocks.settling.contains_key(&run_id)
            {
                let replaced = blocks.cooled.insert(
                    run_id,
                    HeldEntryRecovery {
                        refused: Some(cause),
                        ..HeldEntryRecovery::new(
                            ladder,
                            now_unix_ms.saturating_add(CLAIM_RETRY_FIRST_MS),
                        )
                    },
                );
                (true, replaced)
            } else {
                (true, None)
            }
        };
        self.record_dropped_entry_recoveries([replaced])?;
        Ok(recorded)
    }

    /// A recovery Workflow #489 AVAIL-1 held back with `instance_id` while it was unavailable,
    /// which the instance's leaving the unavailable set lets go (`leave_held_recovery`): an
    /// entry-blocked instance's block takes it over, to take it up at `until_unix_ms` (the window
    /// in force read again then; a recovery it replaces recorded as dropped, fix round 3, ruling
    /// 7); for any other instance it is returned, for AVAIL-1 to drop.
    pub(super) fn hand_over_held_recovery(
        &self,
        instance_id: InstanceId,
        ladder: PendingRecoveryLadder,
        until_unix_ms: u64,
    ) -> RuntimeHostResult<Option<PendingRecoveryLadder>> {
        let replaced = {
            let mut blocks = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
            let Some(block) = blocks.blocked.get_mut(&instance_id) else {
                return Ok(Some(ladder));
            };
            block.hold(HeldEntryRecovery::new(ladder, until_unix_ms))
        };
        self.record_dropped_entry_recoveries([replaced])?;
        Ok(None)
    }

    /// The lease sweep's step after the unavailable instances' probes, for each blocked instance:
    ///
    /// - a blackout that covers the instance, or a fence that took it, ends the block (its held
    ///   recovery dropped with its record);
    /// - a stretch that lasted `scheduling.unavailable_probe.attention_after_ms` brings the
    ///   instance to a person's attention, once per stretch, unless a pause, a fence or an idle
    ///   stop owns the instance (as AVAIL-1's attention: its clock runs on, and it is told at the
    ///   first tick nothing owns it);
    /// - a held recovery waits while a pause or an idle stop owns the instance (nothing recorded;
    ///   a resume ends the block), and after a panicked climb in the block, or once the update
    ///   dialog stopped the stretch's climbs (fix round 3, ruling 4), no recovery is taken up;
    ///   otherwise a due one is admitted as it would have been without its hold
    ///   ([`Self::admit_held_recovery`]), from the application restart when the block says so. Fix
    ///   round 3 (ruling 3): a later climb of the stretch is due no sooner than
    ///   `scheduling.unavailable_probe`'s interval for the climbs the stretch had, from the latest
    ///   one's end, and, like the block's own climb after another, never before the instance's
    ///   cool-down window in force now ends (read again here, as AVAIL-1's release reads it); one
    ///   that respects the cool-down waits for that window too. While another ladder holds or
    ///   waits for the instance, or business capacity refuses new work, a due recovery stays held,
    ///   nothing recorded, and is tried again at the next tick.
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
        let config = self.unavailable_probe_config()?;
        let attention_after_ms = config.attention_after_ms;
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
            let hold = self.probe_hold(instance_id, &alias)?;
            let fenced = hold == ProbeHold::Owned && self.resource_fenced(instance_id)?;
            if hold == ProbeHold::Blackout || fenced {
                // Nothing is dispatched inside the blackout, and after it the first due task tries
                // its entry anew; a fence and its own recheck own the instance.
                self.end_entry_block(instance_id)?;
                continue;
            }
            let (late, held_until) = {
                let mut guard = lock(&self.entry_blocks, HostLocation::TakeUpEntryRecoveries)?;
                let blocks = &mut *guard;
                let Some(block) = blocks.blocked.get(&instance_id) else {
                    continue;
                };
                let late = blocks
                    .stretches
                    .get_mut(&instance_id)
                    .filter(|stretch| {
                        hold != ProbeHold::Owned
                            && !stretch.told
                            && now_unix_ms.saturating_sub(stretch.since_unix_ms)
                                >= attention_after_ms
                    })
                    .map(|stretch| {
                        stretch.told = true;
                        (
                            block.gate.clone(),
                            now_unix_ms.saturating_sub(stretch.since_unix_ms),
                        )
                    });
                let stretch = blocks.stretches.get(&instance_id);
                let stopped = stretch.is_some_and(|stretch| stretch.update_dialog);
                let paced = stretch.and_then(|stretch| stretch.paced_until(config));
                let held_until = block
                    .held
                    .as_ref()
                    .filter(|_| !block.panicked && !stopped)
                    .map(|held| held.until_unix_ms.max(paced.unwrap_or(0)));
                (late, held_until)
            };
            if let Some((gate, elapsed_ms)) = late {
                self.record_entry_attention(instance_id, &gate, elapsed_ms, attention_after_ms)?;
            }
            let Some(until_unix_ms) = held_until else {
                continue;
            };
            if hold == ProbeHold::Owned || until_unix_ms > now_unix_ms {
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
            let ladder = {
                let mut guard = lock(&self.entry_blocks, HostLocation::TakeUpEntryRecoveries)?;
                let blocks = &mut *guard;
                let (paced, stopped) = blocks
                    .stretches
                    .get(&instance_id)
                    .map_or((None, false), |stretch| {
                        (stretch.paced_until(config), stretch.update_dialog)
                    });
                let Some(block) = blocks.blocked.get_mut(&instance_id) else {
                    continue;
                };
                if block.panicked || stopped {
                    continue;
                }
                let restarts = block.restarts;
                let Some(held) = block.held.as_mut() else {
                    continue;
                };
                let due = held.until_unix_ms.max(paced.unwrap_or(0));
                if due > now_unix_ms {
                    held.until_unix_ms = due;
                    continue;
                }
                if (paced.is_some() || held.retry || held.ladder.respects_cooldown())
                    && now_unix_ms < window_until_unix_ms
                {
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
                let mut ladder = held.ladder;
                if restarts {
                    ladder.start_at_application_restart();
                }
                ladder
            };
            self.admit_held_recovery(ladder)?;
        }
        Ok(())
    }

    /// The blocked instances and (fix round 3, ruling 2) the pairs that yield their turn, by
    /// alias, as the evaluation reads them.
    pub(super) fn entry_block_gates(&self) -> RuntimeHostResult<EntryBlockGates> {
        let (blocked, yielding) = {
            let blocks = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
            (
                blocks
                    .blocked
                    .iter()
                    .map(|(instance_id, block)| (*instance_id, block.gate.clone()))
                    .collect::<Vec<_>>(),
                blocks
                    .yielding
                    .iter()
                    .map(|(instance_id, gate)| (*instance_id, gate.clone()))
                    .collect::<Vec<_>>(),
            )
        };
        if blocked.is_empty() && yielding.is_empty() {
            return Ok(EntryBlockGates::default());
        }
        let instances = lock(&self.registered_instances, HostLocation::InstanceEntryBlock)?;
        let by_alias = |gates: Vec<(InstanceId, EntryBlockGate)>| {
            gates
                .into_iter()
                .filter_map(|(instance_id, gate)| {
                    instances
                        .get(&instance_id)
                        .map(|instance| (instance.instance_alias.clone(), gate))
                })
                .collect::<BTreeMap<_, _>>()
        };
        Ok(EntryBlockGates::new(by_alias(blocked), by_alias(yielding)))
    }

    /// `instance_entry_blocked_awaiting_person` of `instance_id`, blocked by `gate` within a
    /// stretch that lasted `elapsed_ms`, which reached `limit_ms` (`attention_after_ms`).
    fn record_entry_attention(
        &self,
        instance_id: InstanceId,
        gate: &EntryBlockGate,
        elapsed_ms: u64,
        limit_ms: u64,
    ) -> RuntimeHostResult<()> {
        let outcome = entry_block_outcome(
            HostCode::InstanceEntryBlockedAwaitingPerson,
            instance_id,
            gate,
        )
        .elapsed_ms(elapsed_ms)
        .limit_ms(limit_ms);
        self.record_entry_block_warning(instance_id, outcome)
    }

    /// `instance_entry_blocked_recovery_failed` of `instance_id`, blocked by `gate`, one of whose
    /// own recoveries ended without recovering as `end` says, admitted under the trigger item
    /// `rule`; `next` when the block's next climb is due at the earliest (none after the update
    /// dialog or a panicked worker, which end the climbs).
    fn record_entry_recovery_failed(
        &self,
        instance_id: InstanceId,
        gate: &EntryBlockGate,
        end: EntryRecoveryEnd,
        rule: Option<&str>,
        next: Option<u64>,
    ) -> RuntimeHostResult<()> {
        let outcome = entry_block_outcome(
            HostCode::InstanceEntryBlockedRecoveryFailed,
            instance_id,
            gate,
        )
        .entry_recovery_end(end);
        let outcome = match rule {
            Some(rule) => outcome.trigger_rule(rule),
            None => outcome,
        };
        let outcome = match next {
            Some(next) => outcome.next_occurrence_unix_ms(next),
            None => outcome,
        };
        self.record_entry_block_warning(instance_id, outcome)
    }

    /// One warning of an entry-blocked instance, `outcome`: a `runtime.failed` record under the
    /// instance's links, handed to the daemon's log too.
    fn record_entry_block_warning(
        &self,
        instance_id: InstanceId,
        outcome: Outcome,
    ) -> RuntimeHostResult<()> {
        let operation = HostLocation::InstanceEntryBlock;
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
    /// live settlement judges one (fix round 3, ruling 1), against the instance's marks now (the
    /// live settlement reads them at the run's start, which the replays exercise) and the activity
    /// window of its admission: when it is its pair's own, the input returned is `input` with its
    /// identity written as a failure that accumulates (no error frame). A run armed as past its
    /// entry started its main package after its entry check.
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
            return Ok((Some(EntrySettlement::TargetStarted { entered: true }), None));
        }
        let (instance_alias, task_id, previous, window_end) = {
            let policy = lock(self.policy()?, HostLocation::InstanceEntryBlock)?;
            let instance_alias = policy.execution_instance_id(decision_id)?.to_owned();
            let task_id = policy.execution_task_id_for_test(decision_id)?.to_owned();
            let previous = policy
                .latest_failure(&task_id, &instance_alias)
                .and_then(|(identity, _)| identity.map(str::to_owned));
            let window_end = policy.dispatch_window_end(decision_id)?.unwrap_or_default();
            (instance_alias, task_id, previous, window_end)
        };
        let instance_id = self
            .resolve_instance(&instance_alias)
            .map_err(|failure| *failure.error)?
            .instance_id();
        let marks = self.entry_marks(instance_id)?;
        let not_own = Some(EntrySettlement::AtEntry {
            marks,
            own: false,
            window_end,
        });
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
            instance_id,
            &instance_alias,
            &task_id,
            &parsed.packages(),
            window_end,
        )? {
            return Ok((not_own, None));
        }
        Ok((
            Some(EntrySettlement::AtEntry {
                marks,
                own: true,
                window_end,
            }),
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

    /// Test only: the pair of `task_id` on `instance_alias`'s mark, if it is set: the scheduled
    /// runs the instance had let in when the failure that set it settled.
    #[cfg(test)]
    pub(super) fn entry_mark_for_test(
        &self,
        instance_alias: &str,
        task_id: &str,
    ) -> RuntimeHostResult<Option<u64>> {
        Ok(lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?
            .entry_failed
            .get(&(instance_alias.to_owned(), task_id.to_owned()))
            .map(|mark| mark.entered))
    }

    /// Test only: the entry block of `instance_id`, if it has one.
    #[cfg(test)]
    pub(super) fn entry_block_for_test(
        &self,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<Option<EntryBlockForTest>> {
        let blocks = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
        let stretch_told = blocks
            .stretches
            .get(&instance_id)
            .is_some_and(|stretch| stretch.told);
        Ok(blocks.blocked.get(&instance_id).map(|block| {
            (
                block.gate.clone(),
                block.recovery_failed || stretch_told,
                block.held.as_ref().map(|held| held.until_unix_ms),
            )
        }))
    }

    /// Test only: whether the entry block of `instance_id`, if it has one, climbs from the
    /// application restart.
    #[cfg(test)]
    pub(super) fn entry_block_restarts_for_test(
        &self,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<Option<bool>> {
        Ok(lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?
            .blocked
            .get(&instance_id)
            .map(|block| block.restarts))
    }

    /// Test only: the stretch of `instance_id`, if one runs: when it began and whether a person
    /// was told.
    #[cfg(test)]
    pub(super) fn entry_stretch_for_test(
        &self,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<Option<(u64, bool)>> {
        Ok(lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?
            .stretches
            .get(&instance_id)
            .map(|stretch| (stretch.since_unix_ms, stretch.told)))
    }

    /// Fix round 3, test only: the climbs of `instance_id`'s stretch, if one runs, and when the
    /// latest ended (ruling 3).
    #[cfg(test)]
    pub(super) fn entry_climbs_for_test(
        &self,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<Option<(u32, u64)>> {
        Ok(lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?
            .stretches
            .get(&instance_id)
            .map(|stretch| (stretch.climbs, stretch.last_end_unix_ms)))
    }

    /// Fix round 3, test only: the task whose pair yields its turn on `instance_id`, if one does
    /// (ruling 2).
    #[cfg(test)]
    pub(super) fn entry_yielding_for_test(
        &self,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<Option<String>> {
        Ok(lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?
            .yielding
            .get(&instance_id)
            .map(|gate| gate.task_id.clone()))
    }
}

/// The warning `code` of `instance_id`, blocked by `gate`: the values both warnings carry.
fn entry_block_outcome(code: HostCode, instance_id: InstanceId, gate: &EntryBlockGate) -> Outcome {
    let outcome = Outcome::new(code)
        .operation(HostLocation::InstanceEntryBlock)
        .instance_id(identifier_text(&instance_id))
        .task_id(gate.task_id.as_str())
        .decision_id(gate.decision_id.as_str())
        .observed_at_unix_ms(gate.since_unix_ms);
    match gate.failure_code.registered() {
        Some(code) => outcome.failure_code(code),
        None => outcome,
    }
}
