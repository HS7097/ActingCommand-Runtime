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
//! - **The block.** The live settlement settles a failure at the entry (fix round 2, ruling 1: a
//!   code of class `pack_logic_mismatch` only) as a condition of the run's instance (its record's
//!   identity marked `i…`, `policy_control::entry_blocked`) and blocks the instance here, under
//!   the policy outcome gate; the evaluation reads the blocks under the same gate
//!   (`entry_block_gates`) and defers every candidate of a blocked instance but a store update
//!   task's (position 4b, `crate::entry_block`).
//! - **Its end.** A recovery ladder of the instance that ends `recovered` or `environment_ready`,
//!   a run on the instance that succeeds (a scheduled run, a client's or the Lab's run, a startup
//!   package, a rung's run: `append_contained_task_terminal`), a resume that lifts the instance's
//!   pause or the global one, a restart (memory only), a blackout that covers the instance and a
//!   fence that takes it. Fix round 2 (ruling 9): a recovery the block held when it ends is
//!   dropped with the record AVAIL-1 writes for every recovery it drops
//!   (`record_dropped_held_recovery`: the trigger's `recovery_ladder_suppressed` `cooldown` once
//!   more).
//! - **Where its recovery starts** (fix round 2, ruling 2). A block set while no run's main
//!   package has started on the instance since the instance's previous block ended is climbed
//!   from the first rung that restarts the application (`start_at_application_restart`, by the
//!   rung's kind): whatever ended the previous block let nothing in. Any other block (the first
//!   since the Runtime started, one after a run got in) is climbed as its trigger item says. The
//!   Host learns that a run's main package started from a scheduled run's settlement (a success,
//!   or a failure past its entry).
//! - **Its way out.** A held recovery (the ladder's cool-down, an approaching blackout, a
//!   transient obstacle, a climb that was interrupted) is taken up by the lease sweep
//!   (`take_up_entry_recoveries`, after the unavailable instances' probes; the mechanism Workflow
//!   #489 AVAIL-1 built) once nothing holds the instance. A transient obstacle (the admission's
//!   capacity refusal, another ladder that holds or waits for the instance) is judged before
//!   anything is recorded; a claim the queue refused is tried again on a growing interval, one
//!   second at first, doubling to one minute (fix round 2, ruling 7), its refusal recorded when it
//!   first occurs in the block and when its cause changes. Fix round 2 (ruling 5): a recovery of
//!   the block that ends `exhausted` tells a person once per block
//!   (`instance_entry_blocked_recovery_failed`) and the block climbs again by itself, from the
//!   application restart, after the wait `scheduling.unavailable_probe` gives its n-th exhausted
//!   climb (first interval, factor, maximum), counted from that climb's end and never before the
//!   instance's cool-down window ends; one whose worker panicked tells a person and climbs no more
//!   in that block.
//! - **The attention** (fix round 2, ruling 6). A stretch runs from the first block since a run's
//!   main package last started on the instance (or since the Runtime started) until a run's main
//!   package starts there: blocks that something ended and that came back with nothing let in are
//!   one stretch. Once it lasted `scheduling.unavailable_probe.attention_after_ms`, while the
//!   instance is blocked and no pause, fence or idle stop owns it (as AVAIL-1's attention), a
//!   person is told once per stretch (`instance_entry_blocked_awaiting_person`). Each warning is a
//!   record and the daemon's log.
//! - **Races.** A scheduled run's start keeps its instance's marks (how many blocks ended so far),
//!   and its settlement compares them: anything that ended a block between the run's start and its
//!   settlement leaves no block. A recovery of the run's own failure that ended without recovering
//!   before the settlement raises its warning with the block.
//!
//! - **Whose failure (ENTRY-A2).** Each pair (instance, catalog task) keeps here whether its
//!   task's last scheduled run failed at its entry (its mark: what that run's start knew of the
//!   instance, the packages its identity names, its conditions in a row and the activity window
//!   they were settled in). A failure at the entry of a marked pair, with the same packages and in
//!   the same activity window (fix round 2, ruling 4a), is the pair's own when a recovery of the
//!   instance that ended `recovered` and restarted the application (fix round 2, ruling 3) came
//!   since that run began and no other run started and no person or client held the instance's
//!   key between that recovery and its own start; or (the bound) when the pair's conditions in a
//!   row reached its rule's `escalation_threshold` and since its previous failure at the entry
//!   either such a recovery came or another run's main package started on the instance (fix round
//!   2, ruling 4b). It settles as a failure that accumulates
//!   (`failure_identity::own_entry_identity`) and blocks the instance all the same. A run of the
//!   task whose main package started, a settlement that pauses the pair and a success clear the
//!   mark; packages that changed and a window that ended make it count for nothing.
//!
//! Memory only: a restart starts with no block, no mark and no stretch (its preparation proves
//! the instance again and its startup package runs where one is configured). A leaf lock.

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

/// What a scheduled run's start knew of its instance: how many blocks of it ended, (ENTRY-A2) how
/// many of its recoveries ended `recovered` after restarting the application since the Runtime
/// started, the runs started and manual keys granted on it since the latest of those recoveries,
/// and (fix round 2) how many runs' main packages started on it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct EntryMarks {
    ended: u64,
    recovered: u64,
    /// ENTRY-A2 (the coordinator's ruling of 11-21 00:22 JST; fix rounds 1 and 2): the runs
    /// started on the instance since its latest recovery that is evidence (a scheduled run, a
    /// client's or the Lab's; not the Runtime's own startup or recovery package's run), and the
    /// keys of it granted to a person's or a client's claim since then (a client's lease, a Lab
    /// operation's, a direct run's, an input lifecycle's, an emulator control's, the automatic
    /// start's).
    started: u64,
    /// Fix round 2 (rulings 2, 4b and 6): the runs whose main package started on the instance, as
    /// their settlements tell (a success, a failure past the entry).
    let_in: u64,
}

/// Workflow #490 ENTRY-A and A2: what a scheduled run's settlement tells its instance's block and
/// its pair's mark. A settlement without one (a stop, a reconciled or interrupted run, a run whose
/// chain was not resolved) leaves the mark as it is, but for a success.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EntrySettlement {
    /// The run failed at its entry (ENTRY-A's set), with what its start knew of its instance;
    /// `own` (ENTRY-A2) when the failure is its pair's own ([`HostShared::entry_failure_own`]);
    /// `window_end` (fix round 2, ruling 4a) the end of the activity window its admission was
    /// committed in (`0` when the policy knows none).
    AtEntry {
        marks: EntryMarks,
        own: bool,
        window_end: u64,
    },
    /// The run's main package started: its entry worked, whatever its end.
    TargetStarted,
}

/// ENTRY-A2: a pair's mark: its task's last scheduled run failed at its entry.
struct EntryFailureMark {
    /// The instance's recoveries that are evidence when that run began.
    recovered: u64,
    /// Fix round 2 (ruling 4b): the runs whose main package started on the instance when that run
    /// began.
    let_in: u64,
    /// The packages its failure identity named ([`FailureIdentity::packages`]).
    packages: String,
    /// Fix round 1 (the bound): the pair's failures at the entry in a row settled as conditions
    /// since it last started its main package, in the activity window `window_end` ends.
    conditions: u16,
    /// Fix round 2 (ruling 4a): the end of the activity window those conditions were settled in.
    window_end: u64,
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
    /// Fix round 2 (ruling 5): the block's own climb after an exhausted one, which waits for the
    /// instance's cool-down window whatever its trigger item says of the window.
    retry: bool,
    /// Fix round 2 (ruling 7): for a recovery held for a settlement still to come, the cause of
    /// the queue's refusal of its claim that was recorded, which the block it joins remembers.
    refused: Option<&'static str>,
}

impl HeldEntryRecovery {
    fn new(ladder: PendingRecoveryLadder, until_unix_ms: u64) -> Self {
        Self {
            ladder,
            until_unix_ms,
            retry: false,
            refused: None,
        }
    }
}

/// How one of a scheduled run's own recoveries ended without recovering before its settlement.
struct FailedEntryRecovery {
    end: EntryRecoveryEnd,
    ladder: PendingRecoveryLadder,
    ended_at_unix_ms: u64,
}

/// One blocked instance.
struct EntryBlock {
    /// The latest failure at the entry that blocked it.
    gate: EntryBlockGate,
    /// Fix round 1: a person was told one of the block's own recoveries ended without recovering
    /// (`instance_entry_blocked_recovery_failed`), once per block.
    recovery_failed: bool,
    held: Option<HeldEntryRecovery>,
    /// Fix round 1: the trigger correlations of the block's own recoveries (the blocking runs'
    /// lease ends, the recoveries it held or took over): only one of these ending without
    /// recovering tells a person; another trigger's ladder does not.
    recoveries: BTreeSet<CorrelationId>,
    /// Fix round 2 (ruling 2): set while no run's main package had started on the instance since
    /// its previous block ended: the block's recoveries climb from the application restart.
    restarts: bool,
    /// Fix round 2 (ruling 5): the block's own climbs that ended `exhausted`.
    exhausted: u32,
    /// Fix round 2 (ruling 5): a worker climbing one of its recoveries panicked: no further climb.
    panicked: bool,
    /// Fix round 2 (ruling 7): the queue's refusals of its recovery's claims, and the cause of the
    /// latest one recorded.
    refusals: u32,
    refused: Option<&'static str>,
}

impl EntryBlock {
    /// One of the block's own recoveries, `ladder`, ended without recovering as `end` says at
    /// `ended_at_unix_ms` (fix round 2, ruling 5): an exhausted climb is followed by another, held
    /// from the application restart until its wait (`config`'s interval for the block's n-th
    /// exhausted climb) has passed, unless a recovery is already held; a panicked worker's ends
    /// the block's climbs. Returns when the next climb is due, if one is, and a held recovery the
    /// panic let go.
    fn recovery_ended(
        &mut self,
        end: EntryRecoveryEnd,
        ladder: &PendingRecoveryLadder,
        ended_at_unix_ms: u64,
        config: UnavailableProbeConfig,
    ) -> (Option<u64>, Option<HeldEntryRecovery>) {
        match end {
            EntryRecoveryEnd::Exhausted => {
                self.exhausted = self.exhausted.saturating_add(1);
                if self.panicked {
                    return (None, None);
                }
                if let Some(held) = &self.held {
                    return (Some(held.until_unix_ms), None);
                }
                let mut again = ladder.clone();
                again.start_at_application_restart();
                let until_unix_ms =
                    ended_at_unix_ms.saturating_add(config.interval_ms(self.exhausted));
                self.recoveries.insert(again.correlation());
                self.held = Some(HeldEntryRecovery {
                    retry: true,
                    ..HeldEntryRecovery::new(again, until_unix_ms)
                });
                (Some(until_unix_ms), None)
            }
            EntryRecoveryEnd::WorkerPanicked => {
                self.panicked = true;
                (None, self.held.take())
            }
        }
    }
}

/// Fix round 2 (ruling 6): a stretch of blocks with nothing let in between them.
struct Stretch {
    /// When its first block was set (the Runtime's clock).
    since_unix_ms: u64,
    /// A person was told (`instance_entry_blocked_awaiting_person`), once per stretch.
    told: bool,
}

/// The blocks, the marks and the stretches (memory only). A leaf lock: nothing else is locked
/// while it is held.
#[derive(Default)]
pub(super) struct EntryBlocks {
    blocked: BTreeMap<InstanceId, EntryBlock>,
    ended: BTreeMap<InstanceId, u64>,
    /// ENTRY-A2: the instance's recovery ladders that ended `recovered` (fix round 1: not
    /// `environment_ready`; fix round 2, ruling 3: only those that restarted the application).
    recovered: BTreeMap<InstanceId, u64>,
    /// ENTRY-A2: the runs started and the manual keys granted on the instance since its latest
    /// recovery that is evidence.
    started: BTreeMap<InstanceId, u64>,
    /// Fix round 2: the runs whose main package started on the instance.
    let_in: BTreeMap<InstanceId, u64>,
    /// Fix round 2 (ruling 2): `let_in` when the instance's latest block ended.
    ended_let_in: BTreeMap<InstanceId, u64>,
    /// Fix round 2 (ruling 6): the instance's stretch of blocks, while one runs.
    stretches: BTreeMap<InstanceId, Stretch>,
    /// ENTRY-A2: the pairs' marks, by instance alias and catalog task.
    entry_failed: BTreeMap<(String, String), EntryFailureMark>,
    /// Fix round 1: the scheduled runs started whose settlement has not come yet, with (fix round
    /// 2) their dispatch.
    settling: BTreeMap<RunId, String>,
    /// A recovery held for a scheduled run whose settlement has not come yet (its instance not
    /// blocked yet), by the run (fix round 1: not by the instance, so no other run's recovery
    /// displaces it): the settlement takes it when it blocks the instance and lets it go
    /// otherwise.
    cooled: BTreeMap<RunId, HeldEntryRecovery>,
    /// Fix round 1: the scheduled runs whose own recovery ended without recovering before their
    /// settlement, and how: the block that settlement sets tells a person at once.
    unrecovered: BTreeMap<RunId, FailedEntryRecovery>,
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
            let_in: self.let_in(instance_id),
        }
    }

    fn let_in(&self, instance_id: InstanceId) -> u64 {
        self.let_in.get(&instance_id).copied().unwrap_or(0)
    }

    /// Fix round 2 (ruling 2): whether a recovery of `instance_id`'s block climbs from the
    /// application restart: the instance's block says so, or with no block, its previous block
    /// ended and no run's main package started on it since.
    fn restart_due(&self, instance_id: InstanceId) -> bool {
        match self.blocked.get(&instance_id) {
            Some(block) => block.restarts,
            None => self.ended_let_in.get(&instance_id) == Some(&self.let_in(instance_id)),
        }
    }

    /// Ends the block of `instance_id`, if it has one, and returns the recovery it held; counted
    /// for a settlement still to come.
    fn end(&mut self, instance_id: InstanceId) -> Option<HeldEntryRecovery> {
        let ended = self.ended.entry(instance_id).or_insert(0);
        *ended = ended.saturating_add(1);
        let block = self.blocked.remove(&instance_id)?;
        let let_in = self.let_in(instance_id);
        self.ended_let_in.insert(instance_id, let_in);
        block.held
    }

    /// Fix round 2: a run's main package started on `instance_id`: it let a run in, which ends
    /// the instance's stretch.
    fn note_let_in(&mut self, instance_id: InstanceId) {
        let let_in = self.let_in.entry(instance_id).or_insert(0);
        *let_in = let_in.saturating_add(1);
        self.stretches.remove(&instance_id);
    }
}

/// Fix round 2 (ruling 7): the wait before the claim of a block's recovery is tried again after
/// the block's `refusals`-th refusal (from 0): one second, doubling, at most one minute.
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

    /// A scheduled run `run_id` of the dispatch `decision_id` on `instance_id` starts (its
    /// resolution is registered, before `begin_contained_run` counts it): the marks of its instance
    /// now, which its settlement compares, and (fix round 1) its settlement is due, so a recovery
    /// held for it is kept until then.
    pub(super) fn entry_settlement_pending(
        &self,
        run_id: RunId,
        decision_id: &str,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<EntryMarks> {
        let mut blocks = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
        blocks.settling.insert(run_id, decision_id.to_owned());
        Ok(blocks.marks(instance_id))
    }

    /// Fix round 2 (ruling 2): the dispatch of the scheduled run `run_id`, while its settlement is
    /// still to come.
    pub(super) fn entry_settling_dispatch(
        &self,
        run_id: RunId,
    ) -> RuntimeHostResult<Option<String>> {
        Ok(lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?
            .settling
            .get(&run_id)
            .cloned())
    }

    /// Fix round 2 (ruling 2): whether a recovery of a block of `instance_id` set now would climb
    /// from the application restart ([`EntryBlocks::restart_due`]).
    pub(super) fn entry_restart_due(&self, instance_id: InstanceId) -> RuntimeHostResult<bool> {
        Ok(lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?.restart_due(instance_id))
    }

    /// ENTRY-A2: whether a failure at the entry of `task_id` on `instance_alias`, whose run's start
    /// knew `marks`, whose identity names `packages` and whose admission was committed in the
    /// activity window ending at `window_end`, is the pair's own: the pair's mark is set (its
    /// task's last scheduled run failed at its entry) with the same packages, in the same window
    /// (fix round 2, ruling 4a: a mark of an earlier window counts for nothing), and either (the
    /// coordinator's ruling of 11-21 00:22 JST) a recovery that is evidence (fix round 2, ruling 3:
    /// it ended `recovered` and restarted the application) came since that run began and no other
    /// run started and no manual key was granted on the instance between the latest such
    /// recovery and this run's start, or (the bound) the pair's conditions in a row reached
    /// `escalation_threshold`, its rule's, and (fix round 2, ruling 4b) since its previous failure
    /// at the entry such a recovery came or another run's main package started on the instance.
    /// A block ended by anything else (a run's success, a resume, `environment_ready`, a recovery
    /// at the return-home rung) is no evidence against the pair.
    pub(super) fn entry_failure_own(
        &self,
        instance_alias: &str,
        task_id: &str,
        marks: EntryMarks,
        packages: &str,
        escalation_threshold: Option<u16>,
        window_end: u64,
    ) -> RuntimeHostResult<bool> {
        Ok(lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?
            .entry_failed
            .get(&(instance_alias.to_owned(), task_id.to_owned()))
            .is_some_and(|mark| {
                let recovered = marks.recovered > mark.recovered;
                mark.packages == packages
                    && mark.window_end == window_end
                    && ((recovered && marks.started == 0)
                        || (escalation_threshold
                            .is_some_and(|threshold| mark.conditions >= threshold)
                            && (recovered || marks.let_in > mark.let_in)))
            }))
    }

    /// ENTRY-A2: a run started on `instance_id` (a scheduled run, a client's or the Lab's run;
    /// `begin_contained_run` calls it for every run but the Runtime's own startup or recovery
    /// package's), or (fix rounds 1 and 2) a key of it was granted to a person's or a client's
    /// claim or to the automatic start (`grant_prepared_lease_with_links`): counted until the
    /// instance's next recovery that is evidence.
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
    /// paused the pair), counting a condition in a row within its activity window, and blocks the
    /// instance when it settled as a condition at the entry or as the pair's own failure (a
    /// configured condition of `scheduling.rerun` blocks nothing, as before ENTRY-A2). Fix round
    /// 2: a success or a run whose main package started let a run in on its instance.
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
        let target_started = entry == Some(EntrySettlement::TargetStarted);
        let at_entry = match entry {
            Some(EntrySettlement::AtEntry {
                marks,
                own,
                window_end,
            }) => Some((marks, own, window_end)),
            Some(EntrySettlement::TargetStarted) | None => None,
        };
        let blocks_instance = match (at_entry, failure) {
            (Some((_, own, _)), Some(failure)) => {
                crate::policy_control::entry_blocked(failure)
                    || (own && failure.condition.is_none())
            }
            _ => false,
        };
        let run = context.map(|context| (context.run_id(), context.correlation_id()));
        let let_in = if succeeded || target_started {
            self.settled_instance(data, context)?
        } else {
            None
        };
        {
            let mut blocks = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
            if let Some(instance_id) = let_in {
                blocks.note_let_in(instance_id);
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
            if succeeded || paused || target_started {
                blocks.entry_failed.remove(&key);
            } else if let Some((marks, own, window_end)) = at_entry {
                let packages = failure
                    .and_then(|failure| failure.failure_identity.as_deref())
                    .and_then(FailureIdentity::parse)
                    .map(|identity| identity.packages())
                    .unwrap_or_default();
                // Fix round 1 (the bound): a condition adds one to the pair's conditions in a row;
                // the pair's own failure leaves them as they are; changed packages, and (fix round
                // 2, ruling 4a) another activity window, start again.
                let conditions = match blocks.entry_failed.get(&key) {
                    Some(mark) if mark.packages == packages && mark.window_end == window_end => {
                        if own {
                            mark.conditions
                        } else {
                            mark.conditions.saturating_add(1)
                        }
                    }
                    _ => u16::from(!own),
                };
                blocks.entry_failed.insert(
                    key,
                    EntryFailureMark {
                        recovered: marks.recovered,
                        let_in: marks.let_in,
                        packages,
                        conditions,
                        window_end,
                    },
                );
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
    /// is the block's; the run's own recovery that ended without recovering since then raises
    /// `instance_entry_blocked_recovery_failed` at once (once per block) and, exhausted, holds the
    /// block's next climb (fix round 2, ruling 5). A new block starts the instance's stretch when
    /// none runs (ruling 6) and climbs from the application restart when nothing was let in since
    /// the previous block ended (ruling 2).
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
            blocks.stretches.entry(instance_id).or_insert(Stretch {
                since_unix_ms: now_unix_ms,
                told: false,
            });
            let refused = cooled.as_ref().and_then(|held| held.refused);
            let block = blocks
                .blocked
                .entry(instance_id)
                .or_insert_with(|| EntryBlock {
                    gate: gate.clone(),
                    recovery_failed: false,
                    held: None,
                    recoveries: BTreeSet::new(),
                    restarts,
                    exhausted: 0,
                    panicked: false,
                    refusals: u32::from(refused.is_some()),
                    refused,
                });
            block.gate = gate.clone();
            if let Some((_, correlation)) = run {
                block.recoveries.insert(correlation);
            }
            if let Some(held) = cooled {
                block.recoveries.insert(held.ladder.correlation());
                block.held = Some(held);
            }
            match failed {
                Some(failed) => {
                    let (next, dropped) = block.recovery_ended(
                        failed.end,
                        &failed.ladder,
                        failed.ended_at_unix_ms,
                        config,
                    );
                    let told = (!block.recovery_failed).then(|| {
                        (
                            gate,
                            failed.end,
                            failed.ladder.rule().map(str::to_owned),
                            next,
                        )
                    });
                    block.recovery_failed = true;
                    (told, dropped)
                }
                None => (None, None),
            }
        };
        self.record_dropped_entry_recovery(dropped)?;
        match told {
            Some((gate, end, rule, next)) => {
                self.record_entry_recovery_failed(instance_id, &gate, end, rule.as_deref(), next)
            }
            None => Ok(()),
        }
    }

    /// Ends the entry block of `instance_id`, if it has one, and drops its held recovery with
    /// its record (fix round 2, ruling 9): a run on it succeeded, a recovery of it ended
    /// `environment_ready`, a resume lifted a pause of it, or (fix round 1) a blackout covers it or
    /// a fence took it. Counted for a settlement still to come.
    pub(super) fn end_entry_block(&self, instance_id: InstanceId) -> RuntimeHostResult<()> {
        let dropped = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?.end(instance_id);
        self.record_dropped_entry_recovery(dropped)
    }

    /// Fix round 2 (ruling 9): a recovery a block held and let go is recorded as Workflow #489
    /// AVAIL-1 records every recovery it drops (`record_dropped_held_recovery`: its trigger's
    /// `recovery_ladder_suppressed` `cooldown` once more, `until_unix_ms` the time it was held to).
    fn record_dropped_entry_recovery(
        &self,
        dropped: Option<HeldEntryRecovery>,
    ) -> RuntimeHostResult<()> {
        match dropped {
            Some(held) => self.record_dropped_held_recovery(&held.ladder, held.until_unix_ms),
            None => Ok(()),
        }
    }

    /// A recovery ladder of `instance_id` ended `recovered`: it ends the instance's entry block
    /// and, (fix round 2, ruling 3) when its climb restarted the application (`restarted`), it is
    /// (ENTRY-A2) counted as the recovery after which a failure at the entry of a marked pair may
    /// be the pair's own; one at the return-home rung is no such evidence.
    pub(super) fn note_entry_recovered(
        &self,
        instance_id: InstanceId,
        restarted: bool,
    ) -> RuntimeHostResult<()> {
        let dropped = {
            let mut blocks = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
            if restarted {
                let recovered = blocks.recovered.entry(instance_id).or_insert(0);
                *recovered = recovered.saturating_add(1);
                // No run has started and no manual key was granted since this recovery.
                blocks.started.remove(&instance_id);
            }
            blocks.end(instance_id)
        };
        self.record_dropped_entry_recovery(dropped)
    }

    /// Fix round 2, test only: a run's main package started on `instance_id`, as its settlement
    /// tells the blocks.
    #[cfg(test)]
    pub(super) fn note_entry_let_in_for_test(
        &self,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<()> {
        lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?.note_let_in(instance_id);
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

    /// A recovery `ladder` ended without recovering, as `end` says (its ladder `exhausted`, its
    /// worker panicked): when it is one of its blocked instance's own recoveries, the instance
    /// stays blocked and `instance_entry_blocked_recovery_failed` tells a person, once per block;
    /// fix round 2 (ruling 5): an exhausted climb holds the block's next one (`recovery_ended`), a
    /// panicked worker's ends the block's climbs. A scheduled run's own recovery that ended so
    /// before the run's settlement does the same with the block that settlement sets. Any other
    /// ladder's end tells a block nothing.
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
            let Some(block) = blocks
                .blocked
                .get_mut(&instance_id)
                .filter(|block| block.recoveries.contains(&correlation))
            else {
                if let Some(run_id) = ladder.task_run()
                    && blocks.settling.contains_key(&run_id)
                {
                    blocks.unrecovered.insert(
                        run_id,
                        FailedEntryRecovery {
                            end,
                            ladder: ladder.clone(),
                            ended_at_unix_ms,
                        },
                    );
                }
                return Ok(());
            };
            let (next, dropped) = block.recovery_ended(end, ladder, ended_at_unix_ms, config);
            let told = (!block.recovery_failed).then(|| (block.gate.clone(), next));
            block.recovery_failed = true;
            (told, dropped)
        };
        self.record_dropped_entry_recovery(dropped)?;
        match told {
            Some((gate, next)) => {
                self.record_entry_recovery_failed(instance_id, &gate, end, ladder.rule(), next)
            }
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
        let held = HeldEntryRecovery::new(ladder, until_unix_ms);
        let mut blocks = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
        if let Some(block) = blocks.blocked.get_mut(&instance_id) {
            block.recoveries.insert(correlation);
            block.held = Some(held);
        } else if let Some(run_id) = run_id
            && blocks.settling.contains_key(&run_id)
        {
            blocks.cooled.insert(run_id, held);
        }
        Ok(())
    }

    /// Fix round 1: a recovery `ladder` a transient obstacle refused (its admission's capacity
    /// refusal, another ladder that holds or waits for the instance, a later preparation that
    /// superseded its own) or (fix round 2) a holding interruption ended: no failure of the
    /// recovery; held as [`Self::hold_entry_recovery`] holds it, due at once, so its admission is
    /// tried again at the lease sweep's next tick once nothing holds the instance. A person's
    /// drill is refused to them instead.
    pub(super) fn hold_entry_recovery_again(
        &self,
        ladder: PendingRecoveryLadder,
    ) -> RuntimeHostResult<()> {
        if ladder.is_drill() {
            return Ok(());
        }
        self.hold_entry_recovery(ladder.instance_id(), ladder, 0)
    }

    /// Fix round 2: a recovery `ladder` whose climb a holding interruption ended (a pause, business
    /// capacity, a drain, a shutdown; an update dialog that ended a rung's run): no failure of the
    /// recovery, so no person is told; held as [`Self::hold_entry_recovery`] holds it, due at once
    /// but, as the block's own climb after an exhausted one, never taken up before the instance's
    /// cool-down window (which the interrupted climb opened) ends, so an obstacle that ends every
    /// climb does not start one climb after another. A person's drill is refused to them instead.
    pub(super) fn hold_interrupted_entry_recovery(
        &self,
        ladder: PendingRecoveryLadder,
    ) -> RuntimeHostResult<()> {
        if ladder.is_drill() {
            return Ok(());
        }
        let instance_id = ladder.instance_id();
        let correlation = ladder.correlation();
        let run_id = ladder.task_run();
        let held = HeldEntryRecovery {
            retry: true,
            ..HeldEntryRecovery::new(ladder, 0)
        };
        let mut blocks = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
        if let Some(block) = blocks.blocked.get_mut(&instance_id) {
            block.recoveries.insert(correlation);
            block.held = Some(held);
        } else if let Some(run_id) = run_id
            && blocks.settling.contains_key(&run_id)
        {
            blocks.cooled.insert(run_id, held);
        }
        Ok(())
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
        let correlation = ladder.correlation();
        let run_id = ladder.task_run();
        let mut blocks = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
        if let Some(block) = blocks.blocked.get_mut(&instance_id) {
            let recorded = block.refused != Some(cause);
            block.refused = Some(cause);
            let wait_ms = claim_retry_wait_ms(block.refusals);
            block.refusals = block.refusals.saturating_add(1);
            block.recoveries.insert(correlation);
            block.held = Some(HeldEntryRecovery::new(
                ladder,
                now_unix_ms.saturating_add(wait_ms),
            ));
            return Ok(recorded);
        }
        if let Some(run_id) = run_id
            && blocks.settling.contains_key(&run_id)
        {
            blocks.cooled.insert(
                run_id,
                HeldEntryRecovery {
                    refused: Some(cause),
                    ..HeldEntryRecovery::new(
                        ladder,
                        now_unix_ms.saturating_add(CLAIM_RETRY_FIRST_MS),
                    )
                },
            );
        }
        Ok(true)
    }

    /// A recovery Workflow #489 AVAIL-1 held back with `instance_id` while it was unavailable,
    /// which the instance's leaving the unavailable set lets go (`leave_held_recovery`): an
    /// entry-blocked instance's block takes it over, to take it up at `until_unix_ms` (the window
    /// in force read again then); for any other instance it is returned, for AVAIL-1 to drop.
    pub(super) fn hand_over_held_recovery(
        &self,
        instance_id: InstanceId,
        ladder: PendingRecoveryLadder,
        until_unix_ms: u64,
    ) -> RuntimeHostResult<Option<PendingRecoveryLadder>> {
        let mut blocks = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
        let Some(block) = blocks.blocked.get_mut(&instance_id) else {
            return Ok(Some(ladder));
        };
        block.recoveries.insert(ladder.correlation());
        block.held = Some(HeldEntryRecovery::new(ladder, until_unix_ms));
        Ok(None)
    }

    /// The lease sweep's step after the unavailable instances' probes, for each blocked instance:
    ///
    /// - a blackout that covers the instance, or a fence that took it, ends the block (its held
    ///   recovery dropped with its record, ruling 9);
    /// - fix round 2 (ruling 6): a stretch that lasted `scheduling.unavailable_probe.
    ///   attention_after_ms` (the one threshold AVAIL-1's attention reads) brings the instance to a
    ///   person's attention, once per stretch, unless a pause, a fence or an idle stop owns the
    ///   instance (as AVAIL-1's attention: its clock runs on, and it is told at the first tick
    ///   nothing owns it);
    /// - a held recovery waits while a pause or an idle stop owns the instance (nothing recorded;
    ///   a resume ends the block), and after a panicked climb (ruling 5) no recovery is taken up;
    ///   otherwise a due one is admitted as it would have been without its hold
    ///   ([`Self::admit_held_recovery`], every other admission rule as for every ladder, with a
    ///   request id, a causation id, a holder and a claim request of its own), from the
    ///   application restart when the block says so (ruling 2). As Workflow #489 AVAIL-1's
    ///   release reads it, the cool-down that holds it is the instance's window in force now,
    ///   read again here: while a window holds the instance, a recovery that respects the
    ///   cool-down, and the block's own climb after an exhausted one, stays held until its end.
    ///   While another ladder holds or waits for the instance, or business capacity refuses new
    ///   work, a due recovery stays held, nothing recorded, and is tried again at the next tick.
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
                let held_until = block
                    .held
                    .as_ref()
                    .filter(|_| !block.panicked)
                    .map(|held| held.until_unix_ms);
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
                let mut blocks = lock(&self.entry_blocks, HostLocation::TakeUpEntryRecoveries)?;
                let Some(block) = blocks.blocked.get_mut(&instance_id) else {
                    continue;
                };
                if block.panicked {
                    continue;
                }
                let restarts = block.restarts;
                let Some(held) = block.held.as_mut() else {
                    continue;
                };
                if held.until_unix_ms > now_unix_ms {
                    continue;
                }
                if (held.retry || held.ladder.respects_cooldown())
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

    /// `instance_entry_blocked_awaiting_person` of `instance_id`, blocked by `gate` within a
    /// stretch that lasted `elapsed_ms` (fix round 2, ruling 6), which reached `limit_ms`
    /// (`attention_after_ms`).
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
    /// `rule`; fix round 2 (ruling 5): `next` when the block's next climb is due at the earliest
    /// (none after a panicked worker, which ends the block's climbs).
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
    /// live settlement judges one, against the instance's marks now (the live settlement reads
    /// them at the run's start, which the replays exercise) and the activity window of its
    /// admission: when it is its pair's own (ENTRY-A2, the rule's `escalation_threshold` read as
    /// the live settlement reads it), the input returned is `input` with its identity written as a
    /// failure that accumulates (no error frame).
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
        let (instance_alias, task_id, previous, threshold, window_end) = {
            let policy = lock(self.policy()?, HostLocation::InstanceEntryBlock)?;
            let instance_alias = policy.execution_instance_id(decision_id)?.to_owned();
            let task_id = policy.execution_task_id_for_test(decision_id)?.to_owned();
            let previous = policy
                .latest_failure(&task_id, &instance_alias)
                .and_then(|(identity, _)| identity.map(str::to_owned));
            let threshold = policy.dispatch_escalation_threshold(decision_id)?;
            let window_end = policy.dispatch_window_end(decision_id)?.unwrap_or_default();
            (instance_alias, task_id, previous, threshold, window_end)
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
            &instance_alias,
            &task_id,
            marks,
            &parsed.packages(),
            threshold,
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

    /// Test only: the runs started and manual keys granted on `instance_id` since its latest
    /// recovery that is evidence.
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
    /// entry, and the instance's recoveries that are evidence the mark holds.
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

    /// Fix round 2, test only: whether the entry block of `instance_id`, if it has one, climbs
    /// from the application restart (ruling 2).
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

    /// Fix round 2, test only: the stretch of `instance_id`, if one runs: when it began and
    /// whether a person was told (ruling 6).
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
}

/// Fix round 1: the warning `code` of `instance_id`, blocked by `gate`: the values both warnings
/// carry.
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
