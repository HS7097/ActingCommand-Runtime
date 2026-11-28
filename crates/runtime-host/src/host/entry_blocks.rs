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
//! - **No failure at the entry counts against its rule** (fix round 4, ruling 1). Every one settles
//!   the same way, as a condition of the instance: no streak, no `on_failure`, the admission given
//!   back, the instance blocked. There is no "pair's own" failure.
//! - **A pair that keeps failing at its entry** (fix round 4, rulings 2, 3 and 5). Per pair
//!   (instance, catalog task) the host counts, for the packages its failure identity names and the
//!   activity window its admission was committed in, its failures at the entry in a row
//!   (`EntryPair::in_a_row`): after the n-th the pair is not dispatched on the instance before
//!   `scheduling.unavailable_probe`'s interval for n has passed since the end of the block that
//!   failure set, whatever ends it, and of any later block of the instance (since its settlement
//!   when no block ended since); and those of them that came although the instance had let another
//!   scheduled run in since the pair's previous one
//!   (`EntryPair::against`, by the scheduled runs let in, `entered`): once they reach the rule's
//!   `escalation_threshold` the pair is set aside until its window's end and a person is told once
//!   (`instance_entry_task_set_aside_reported`). Any other settlement of the pair's scheduled run,
//!   another window, other packages and a restart start it from nothing; a store update task's
//!   success starts every pair of its instance from nothing and is no run let in for them; a
//!   catalogue that becomes active starts every pair from nothing. Nothing else follows: no pause,
//!   no streak, nothing that outlives the window. Fix round 5 (ruling 3): past the longest interval
//!   each further failure in a row waits one first interval more (`pair_interval_ms`), and a
//!   scheduled run let in on the instance (the event that raises `entered`) ends every pair's wait
//!   there and its count in a row at once (`EntryBlocks::let_pairs_go`); a store update task's run
//!   is no such run whatever its end (ruling 4), and a changed package frees every pair as an
//!   activation does (ruling 5, the scheduler workbench's check). Fix round 7 (ruling 1, which
//!   replaces fix round 6's rule): at a block's end the pairs take turns in the order of their
//!   latest failure at the entry, the one that failed longest ago first: the first waits one
//!   first interval, the second two, and so on, a wait still running never shortened, a pair set
//!   aside taking no place; fix round 8 (ruling 1): the pairs whose failures set the block take
//!   the places after the others, each waiting the later of its own interval and its place's time
//!   (`EntryBlocks::end`).
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
//!   Fix round 4 (ruling 4): once a ladder of the stretch's blocks ran the rung that restarts the
//!   emulator, every later climb of the stretch skips that rung
//!   (`recovery_rung_skipped_emulator_restarted_in_stretch`), so it ends after the application's
//!   restart.
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
//! Memory only: a restart starts with no block, no pair's count or wait and no stretch (its
//! preparation proves the instance again and its startup package runs where one is configured). A
//! leaf lock.

use super::recovery_ladder::PendingRecoveryLadder;
use super::unavailable_probe::ProbeHold;
use super::*;
use crate::codes::{EntryRecoveryEnd, HostCode, HostLocation};
use crate::entry_block::{EntryBlockGate, EntryBlockGates, EntryPairGate};
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
/// pair's count. A settlement without one (a stop, a reconciled or interrupted run, a run whose
/// chain was not resolved, a failure of another class) is no failure at the entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EntrySettlement {
    /// The run failed at its entry, with what its start knew of its instance; `window_end` the
    /// end of the activity window its admission was committed in (`0` when the policy knows
    /// none). Fix round 4 (ruling 1): it is never its rule's failure.
    AtEntry { marks: EntryMarks, window_end: u64 },
    /// The run's main package started, whatever its end; `entered` (fix round 3) when it started
    /// after an entry check it passed (an `EntryTargetDisposition` `Started`), which lets the run
    /// in on its instance.
    TargetStarted { entered: bool },
}

/// Fix round 4 (rulings 3 and 5): what the policy knows of a settled scheduled run's task, which
/// its pair's count reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct EntryRule {
    /// The task's `on_failure.escalation_threshold` in the dispatch's catalog generation
    /// (`u16::MAX` when the generation holds no such task: nothing is set aside then).
    pub(super) threshold: u16,
    /// The task is a store update package's (UPD-1's remedy table); fix round 5 (ruling 4):
    /// whatever its run's end.
    pub(super) store_task: bool,
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
    /// Fix round 4 (ruling 4): a ladder of the instance's stretch ran the rung that restarts the
    /// emulator already, so this climb skips it.
    pub(super) skip_emulator: bool,
}

/// Fix round 4 (rulings 2 and 3): a pair's failures at the entry in its activity window, with the
/// same packages, while its scheduled runs keep failing there.
#[derive(Debug, Clone)]
struct EntryPair {
    /// The packages its latest failure's identity named ([`FailureIdentity::packages`]).
    packages: String,
    /// The end of the activity window its failures' admissions were committed in (`0`: unknown).
    window_end: u64,
    /// The latest failure: its dispatch, its code and its execution record's observation time
    /// (fix round 7: the turn order at a block's end reads it, `EntryBlocks::end`).
    decision_id: String,
    failure_code: CodeStr,
    observed_at_unix_ms: u64,
    /// Ruling 2: its failures at the entry in a row, the interval for that count, and the time
    /// before which it is not dispatched again: the interval after the end of the block the latest
    /// failure set, whatever ends it (the coordinator's addition to fix round 4: the pair is kept
    /// out for its interval after the instance can be dispatched to again), and after the end of
    /// any later block of the instance; after the latest's observation time until a block ends (a
    /// block holds the pair anyway, but for a store update task, which passes it).
    in_a_row: u32,
    interval_ms: u64,
    until_unix_ms: u64,
    /// Ruling 3: the scheduled runs the instance had let in when the latest failure settled, and
    /// the failures that came although that count had grown since the pair's previous one.
    entered: u64,
    against: u32,
    /// Ruling 3: the rule's `escalation_threshold`, and whether `against` reached it, which sets
    /// the pair aside until `window_end`.
    threshold: u16,
    set_aside: bool,
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
    /// Fix round 6 (ruling 3): the catalog tasks whose failures at the entry set or renewed the
    /// block, whose pairs wait their own interval from its end.
    setters: BTreeSet<String>,
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
    /// Fix round 4 (ruling 4): a ladder of its blocks ran the rung that restarts the emulator: its
    /// later climbs skip that rung.
    emulator_restarted: bool,
}

impl Stretch {
    const fn new(since_unix_ms: u64) -> Self {
        Self {
            since_unix_ms,
            told: false,
            climbs: 0,
            last_end_unix_ms: 0,
            update_dialog: false,
            emulator_restarted: false,
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

/// The blocks, the pairs' counts and the stretches (memory only). A leaf lock: nothing else is
/// locked while it is held.
#[derive(Default)]
pub(super) struct EntryBlocks {
    blocked: BTreeMap<InstanceId, EntryBlock>,
    ended: BTreeMap<InstanceId, u64>,
    /// Fix round 3, kept by fix round 4 for ruling 3 alone: the scheduled runs let in on the
    /// instance, as their settlements tell (a success, a main package started after an entry check
    /// it passed), but a store update task's success (ruling 5).
    entered: BTreeMap<InstanceId, u64>,
    /// Fix round 3 (rulings 3 and 5): the runs let in on the instance: those of `entered`, and a
    /// run that succeeded there that is not the Runtime's own (a client's or the Lab's).
    let_in: BTreeMap<InstanceId, u64>,
    /// `let_in` when the instance's latest block ended.
    ended_let_in: BTreeMap<InstanceId, u64>,
    /// The instance's stretch of blocks, while one runs.
    stretches: BTreeMap<InstanceId, Stretch>,
    /// Fix round 4 (rulings 2 and 3): the pairs whose latest scheduled runs failed at their entry,
    /// by instance and catalog task.
    pairs: BTreeMap<(InstanceId, String), EntryPair>,
    /// The scheduled runs started whose settlement has not come yet, with their dispatch.
    settling: BTreeMap<RunId, String>,
    /// Fix round 4 (ruling 4): the scheduled runs whose own recovery ran the rung that restarts the
    /// emulator before their settlement, with no stretch yet to note it in: the block that
    /// settlement sets notes it.
    emulator_runs: BTreeSet<RunId>,
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

    /// Ends the block of `instance_id`, if it has one, at `now_unix_ms` and returns the recovery it
    /// held; counted for a settlement still to come. Fix round 4 (the coordinator's addition to
    /// ruling 2): the pair whose failure set the block waits its interval from now, so it is kept
    /// out for its interval after the instance can be dispatched to again. Fix round 5 (ruling 3
    /// (ii)): a pair whose wait a scheduled run let in ended (none in a row) has none to renew.
    ///
    /// Fix round 7 (ruling 1; it replaces fix round 6's rule) and fix round 8 (ruling 1): a turn
    /// order. Every pair of the instance with a count in a row above none, but one set aside (it
    /// takes no place), takes a place: first the pairs whose failures did not set the block, in
    /// the order of their latest failure at the entry (`EntryPair::observed_at_unix_ms`, which
    /// every failure sets), the one that failed longest ago first, equal times by the task's id
    /// (the map's order, kept by a stable sort), so the order is the same on every run; then the
    /// pairs whose failures set or renewed the block (`EntryBlock::setters`), in the same order
    /// among themselves. The pair at place n waits n `first_interval_ms` from now, and a pair that
    /// set the block waits its own interval from now when that is later. A wait still running is
    /// never shortened (the later time stands), and the window's end bounds every wait where it is
    /// read (`pair_deferral`). A pair set aside is left as it is, but one whose failure set the
    /// block waits its own interval from now, as before. So after every block the pairs come due
    /// one at a time, least recently failed first and those that set the block last: a pair the
    /// outage left with a long interval waits its turn, not that interval, and a healthy pair is
    /// let in after each broken pair ahead of it in the order has failed once more at most, about
    /// one block each; with no other pair, a pair that set the block waits its own interval. A
    /// task that never failed has no pair: it keeps the first interval's gap after every block.
    /// Any rule that decides a pair's wait from intervals alone (fix rounds 5 and 6) let the
    /// evaluation's order pick among the pairs that came due together, so broken pairs could take
    /// turns ahead of a healthy pair for hours; fix round 7 left the setters at their own
    /// interval, so a setter with a short one came due with the first turn and could run first
    /// again.
    fn end(
        &mut self,
        instance_id: InstanceId,
        now_unix_ms: u64,
        first_interval_ms: u64,
    ) -> Option<HeldEntryRecovery> {
        bump(&mut self.ended, instance_id);
        let block = self.blocked.remove(&instance_id)?;
        self.ended_let_in
            .insert(instance_id, count(&self.let_in, instance_id));
        let (mut others, mut setters) = (Vec::new(), Vec::new());
        for ((_, task_id), pair) in self
            .pairs
            .iter_mut()
            .filter(|((pair_instance, _), pair)| *pair_instance == instance_id && pair.in_a_row > 0)
        {
            match (block.setters.contains(task_id), pair.set_aside) {
                (true, true) => {
                    pair.until_unix_ms = pair
                        .until_unix_ms
                        .max(now_unix_ms.saturating_add(pair.interval_ms));
                }
                (true, false) => setters.push(pair),
                (false, false) => others.push(pair),
                (false, true) => {}
            }
        }
        // Stable: pairs that failed at the same time keep the map's order, by the task's id.
        others.sort_by_key(|pair| pair.observed_at_unix_ms);
        setters.sort_by_key(|pair| pair.observed_at_unix_ms);
        let order = others
            .into_iter()
            .map(|pair| (pair, false))
            .chain(setters.into_iter().map(|pair| (pair, true)));
        for (place, (pair, set_the_block)) in (1_u64..).zip(order) {
            let own_ms = if set_the_block { pair.interval_ms } else { 0 };
            let wait_ms = first_interval_ms.saturating_mul(place).max(own_ms);
            pair.until_unix_ms = pair.until_unix_ms.max(now_unix_ms.saturating_add(wait_ms));
        }
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

    /// Fix round 5 (ruling 3 (ii)): a scheduled run was let in on `instance_id` (the event that
    /// raises `entered`): every pair of the instance has its count in a row set back to none and
    /// its wait ended at once. What counts against it (`against`, by the `entered` its latest
    /// failure kept), its window and its packages stay, so its next failure at the entry counts
    /// one more against it; a pair set aside stays set aside to its window's end.
    fn let_pairs_go(&mut self, instance_id: InstanceId) {
        for (_, pair) in self
            .pairs
            .iter_mut()
            .filter(|((pair_instance, _), _)| *pair_instance == instance_id)
        {
            pair.in_a_row = 0;
            pair.interval_ms = 0;
            pair.until_unix_ms = 0;
        }
    }

    /// Fix round 4 (rulings 2 and 3): the pair `key` failed at its entry, settled as `data`
    /// (`failure` its failure) in the activity window ending at `window_end`, under its rule's
    /// `threshold`. Its counts go on from its previous failure at the entry when that one named the
    /// same packages in the same window, else start at this one: one more in a row, and one more
    /// against it when the instance let another scheduled run in since the previous one. It waits
    /// `config`'s interval for its count in a row from the end of the block this failure sets and
    /// of any later block of the instance ([`EntryBlocks::end`]; from this settlement until a
    /// block ends), and is set aside until the window's end once the count against it reaches
    /// `threshold` (never without a known window).
    /// Returns the pair when this failure set it aside, which a person is told of.
    fn pair_failed(
        &mut self,
        key: (InstanceId, String),
        data: &PolicyExecutionEventData,
        failure: &actingcommand_contract::PolicyFailureRecord,
        window_end: u64,
        threshold: u16,
        config: UnavailableProbeConfig,
    ) -> Option<EntryPair> {
        let packages = failure
            .failure_identity
            .as_deref()
            .and_then(FailureIdentity::parse)
            .map(|identity| identity.packages())
            .unwrap_or_default();
        let entered = count(&self.entered, key.0);
        let (in_a_row, against, was_set_aside) = self
            .pairs
            .get(&key)
            .filter(|pair| pair.packages == packages && pair.window_end == window_end)
            .map_or((1, 0, false), |pair| {
                (
                    pair.in_a_row.saturating_add(1),
                    pair.against
                        .saturating_add(u32::from(entered > pair.entered)),
                    pair.set_aside,
                )
            });
        let set_aside = window_end > 0 && against >= u32::from(threshold);
        let interval_ms = pair_interval_ms(config, in_a_row);
        let pair = EntryPair {
            packages,
            window_end,
            decision_id: data.decision_id.clone(),
            failure_code: actingcommand_contract::scheduled_failure_code_str(&failure.outcome),
            observed_at_unix_ms: data.observed_at_unix_ms,
            in_a_row,
            interval_ms,
            until_unix_ms: data.observed_at_unix_ms.saturating_add(interval_ms),
            entered,
            against,
            threshold,
            set_aside,
        };
        self.pairs.insert(key, pair.clone());
        (set_aside && !was_set_aside).then_some(pair)
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
                // Fix round 5 (ruling 5): every later climb of the block starts at the
                // application restart, whichever path builds it (this hold, the take-up, or the
                // lease end of a later failure at the entry while the block stands, a store update
                // task's), as the held one below does.
                block.restarts = true;
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

/// Fix round 5 (ruling 3 (i)): a pair's wait after its `in_a_row`-th failure at the entry in a
/// row: `config`'s interval for that count, and past `max_interval_ms` one more first interval for
/// each further failure in a row (the window's end still bounds it, `pair_deferral`), so two pairs
/// of an instance whose counts differ never come due together, and the one that has just failed
/// again comes after the others.
fn pair_interval_ms(config: UnavailableProbeConfig, in_a_row: u32) -> u64 {
    let interval = config.interval_ms(in_a_row);
    if interval < config.max_interval_ms {
        return interval;
    }
    // The first count in a row whose interval is the longest.
    let reached = (1..in_a_row)
        .find(|count| config.interval_ms(*count) >= config.max_interval_ms)
        .unwrap_or(in_a_row);
    interval.saturating_add(
        config
            .interval_ms(1)
            .saturating_mul(u64::from(in_a_row.saturating_sub(reached))),
    )
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

/// Fix round 4, test only: a pair whose latest scheduled run failed at its entry, as a test reads
/// it: its failures at the entry in a row (ruling 2), those that came although another scheduled
/// run was let in since the previous one (ruling 3), the time before which it is not dispatched on
/// the instance (ruling 2), and whether it is set aside until its window's end (ruling 3).
#[cfg(test)]
pub(crate) type EntryPairForTest = (u32, u32, u64, bool);

impl HostShared {
    /// The marks of `instance_id` now.
    #[cfg(test)]
    pub(super) fn entry_marks(&self, instance_id: InstanceId) -> RuntimeHostResult<EntryMarks> {
        Ok(lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?.marks(instance_id))
    }

    /// A scheduled run `run_id` of the dispatch `decision_id` on `instance_id` starts (its
    /// resolution is registered): the marks of its instance now, which its settlement compares;
    /// its settlement is due, so a recovery held for it is kept until then.
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
    /// stopped its climbs); fix round 4 (ruling 4): without the emulator's rung when a ladder of
    /// the stretch ran it already.
    pub(super) fn entry_climb(&self, instance_id: InstanceId) -> RuntimeHostResult<EntryClimb> {
        let config = self.unavailable_probe_config()?;
        let blocks = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
        let stretch = blocks.stretches.get(&instance_id);
        let paced = stretch
            .filter(|stretch| stretch.climbs > 0 || stretch.update_dialog)
            .map(|stretch| stretch.paced_until(config).unwrap_or(0));
        Ok(EntryClimb {
            restarts: blocks.restart_due(instance_id),
            paced,
            skip_emulator: stretch.is_some_and(|stretch| stretch.emulator_restarted),
        })
    }

    /// Workflow #490 ENTRY-A: what the execution record `data`, recorded now, tells its pair's
    /// count and its instance's block (`entry` what the settlement knew of the run, `context` the
    /// run when the settlement had it, `rule` what the policy knows of its task). A success or a
    /// main package started after an entry check lets a run in on the instance (fix round 4,
    /// ruling 5: a store update task's success lets none in for the pairs and starts every pair of
    /// the instance from nothing). A failure at the entry, which the settlement settled as a
    /// condition of the instance (fix round 4, ruling 1: always), counts in its pair
    /// ([`EntryBlocks::pair_failed`]; a person is told when it sets the pair aside) and blocks the
    /// instance; any other settlement of the pair's run starts the pair from nothing (a configured
    /// condition of `scheduling.rerun` blocks nothing). Fix round 4 (ruling 6): a recovery held for
    /// a run whose settlement does not block the instance is let go with its record.
    pub(super) fn settle_entry(
        &self,
        data: &PolicyExecutionEventData,
        entry: Option<EntrySettlement>,
        context: Option<&PolicyRunContext>,
        rule: EntryRule,
    ) -> RuntimeHostResult<()> {
        let failure = match &data.outcome {
            PolicyExecutionOutcome::Failed { failure } => Some(failure),
            PolicyExecutionOutcome::Succeeded { .. } | PolicyExecutionOutcome::Stopped { .. } => {
                None
            }
        };
        let succeeded = matches!(data.outcome, PolicyExecutionOutcome::Succeeded { .. });
        let let_in = succeeded || entry == Some(EntrySettlement::TargetStarted { entered: true });
        let at_entry = match entry {
            Some(EntrySettlement::AtEntry { marks, window_end }) => Some((marks, window_end)),
            Some(EntrySettlement::TargetStarted { .. }) | None => None,
        };
        let blocked_by = match (at_entry, failure) {
            (Some(at_entry), Some(failure)) if crate::policy_control::entry_blocked(failure) => {
                Some((at_entry, failure))
            }
            _ => None,
        };
        let run = context.map(|context| (context.run_id(), context.correlation_id()));
        let instance = self.settled_instance(data, context)?;
        let config = self.unavailable_probe_config()?;
        let (set_aside, dropped) = {
            let mut guard = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
            let blocks = &mut *guard;
            let mut dropped = None;
            if let Some((run_id, _)) = run {
                blocks.settling.remove(&run_id);
                // A recovery held for this run, or its own that ended without recovering, is the
                // block's only when this settlement blocks the instance (`block_instance_entry`);
                // any other lets it go, the held one recorded as dropped (fix round 4, ruling 6).
                if blocked_by.is_none() {
                    dropped = blocks.cooled.remove(&run_id);
                    blocks.unrecovered.remove(&run_id);
                    blocks.emulator_runs.remove(&run_id);
                }
            }
            let set_aside = match instance {
                Some(instance_id) => {
                    if rule.store_task {
                        // Fix round 5 (ruling 4): a store update task's run is never evidence that
                        // the instance lets tasks in, whether it succeeded or its main package
                        // started and it failed: no `entered`, no pair's wait ended. It still lets
                        // a run in for the stretch, and its success starts every pair of the
                        // instance from nothing (fix round 4, ruling 5).
                        if succeeded {
                            blocks.note_let_in(instance_id, false);
                            blocks
                                .pairs
                                .retain(|(pair_instance, _), _| *pair_instance != instance_id);
                        } else if let_in {
                            blocks.note_let_in(instance_id, false);
                        }
                    } else if let_in {
                        blocks.note_let_in(instance_id, true);
                        // Fix round 5 (ruling 3 (ii)): the same event ends every pair's wait on
                        // the instance and sets its count in a row back to none.
                        blocks.let_pairs_go(instance_id);
                    }
                    let key = (instance_id, data.task_id.clone());
                    match blocked_by {
                        Some(((_, window_end), failure)) => blocks.pair_failed(
                            key,
                            data,
                            failure,
                            window_end,
                            rule.threshold,
                            config,
                        ),
                        None => {
                            blocks.pairs.remove(&key);
                            None
                        }
                    }
                }
                None => None,
            };
            (set_aside, dropped)
        };
        self.record_dropped_entry_recoveries([dropped])?;
        if let (Some(instance_id), Some(pair)) = (instance, set_aside) {
            self.record_entry_set_aside(instance_id, &data.task_id, &pair)?;
        }
        let Some(((marks, _), failure)) = blocked_by else {
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
    /// alias no instance has (nothing to count on it). #503 PACE: its pair's pacing reads it too
    /// (`failure_pairs.rs`).
    pub(super) fn settled_instance(
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
    /// Fix round 4: a recovery held for the run when anything ended a block since the run's start
    /// is let go with its record (ruling 6); the run's own recovery that ran the emulator's rung
    /// before this settlement is noted in the stretch (ruling 4).
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
            let (cooled, failed, emulator) = match run {
                Some((run_id, _)) => (
                    blocks.cooled.remove(&run_id),
                    blocks.unrecovered.remove(&run_id),
                    blocks.emulator_runs.remove(&run_id),
                ),
                None => (None, None, false),
            };
            if now.ended != marks.ended {
                // A recovery or a run of the instance succeeded, a resume came, a blackout covered
                // it or a fence took it after the run began: nothing is left to block, and a
                // recovery held for the run is let go (fix round 4, ruling 6: recorded). The
                // pair's wait runs from its settlement, the failure having set no block.
                drop(guard);
                return self.record_dropped_entry_recoveries([cooled]);
            }
            let restarts = blocks.restart_due(instance_id);
            let stretch = blocks
                .stretches
                .entry(instance_id)
                .or_insert_with(|| Stretch::new(now_unix_ms));
            if emulator {
                stretch.emulator_restarted = true;
            }
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
                        setters: BTreeSet::new(),
                        held: None,
                        recoveries: BTreeSet::new(),
                        restarts,
                        panicked: false,
                        refusals: u32::from(refused.is_some()),
                        refused,
                    });
                block.gate = gate.clone();
                // Fix round 6 (ruling 3): this pair's failure set (or renewed) the block.
                block.setters.insert(data.task_id.clone());
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
        let now_unix_ms = self.runtime_clock_sample()?.unix_ms;
        let first_interval_ms = self.unavailable_probe_config()?.interval_ms(1);
        let dropped = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?.end(
            instance_id,
            now_unix_ms,
            first_interval_ms,
        );
        self.record_dropped_entry_recoveries([dropped])
    }

    /// Fix round 4 (the coordinator's addition to rulings 2 and 3): a catalogue became active (an
    /// activation or a rollback), which may bind other packages to the pairs' tasks. Comparing each
    /// pair's packages there would mean measuring every task's packages at the switch, so every
    /// pair of every instance (the host's one active catalogue serves them all) starts from
    /// nothing: its count, its wait and its being set aside are cleared. Blocks and stretches stay.
    /// Fix round 5 (ruling 5): so does the scheduler workbench's check that finds a package's
    /// declaration files changed (a placement that activates no catalogue).
    pub(super) fn clear_entry_pairs(&self) -> RuntimeHostResult<()> {
        lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?
            .pairs
            .clear();
        Ok(())
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
        let now_unix_ms = self.runtime_clock_sample()?.unix_ms;
        let first_interval_ms = self.unavailable_probe_config()?.interval_ms(1);
        let dropped = {
            let mut blocks = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
            let dropped = blocks.end(instance_id, now_unix_ms, first_interval_ms);
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
        let first_interval_ms = self.unavailable_probe_config()?.interval_ms(1);
        let dropped = {
            let mut blocks = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
            if blocks.counts_climb(instance_id, ladder) {
                blocks.climb_ended(instance_id, ended_at_unix_ms);
            }
            blocks.end(instance_id, ended_at_unix_ms, first_interval_ms)
        };
        self.record_dropped_entry_recoveries([dropped])
    }

    /// Fix round 4 (ruling 4): a recovery `ladder` is about to run the rung that restarts the
    /// emulator. When it is one of the ladders of its instance's stretch (a scheduled run's own,
    /// or one of a block's own recoveries), the stretch notes it, and its later climbs skip that
    /// rung; a scheduled run's own that climbs before its settlement set the stretch's first block
    /// is noted for that settlement (by the run).
    pub(super) fn note_entry_emulator_restart(
        &self,
        ladder: &PendingRecoveryLadder,
    ) -> RuntimeHostResult<()> {
        let instance_id = ladder.instance_id();
        let mut guard = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
        let blocks = &mut *guard;
        if blocks.counts_climb(instance_id, ladder) {
            if let Some(stretch) = blocks.stretches.get_mut(&instance_id) {
                stretch.emulator_restarted = true;
            }
        } else if let Some(run_id) = ladder.task_run()
            && blocks.settling.contains_key(&run_id)
        {
            blocks.emulator_runs.insert(run_id);
        }
        Ok(())
    }

    /// Fix round 3, test only: a scheduled run was let in on `instance_id`, as its settlement
    /// tells the blocks (fix round 5, ruling 3 (ii): every pair's wait there ended too).
    #[cfg(test)]
    pub(super) fn note_entry_let_in_for_test(
        &self,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<()> {
        let mut blocks = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
        blocks.note_let_in(instance_id, true);
        blocks.let_pairs_go(instance_id);
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
                let (paced, stopped, skip_emulator) =
                    blocks
                        .stretches
                        .get(&instance_id)
                        .map_or((None, false, false), |stretch| {
                            (
                                stretch.paced_until(config),
                                stretch.update_dialog,
                                stretch.emulator_restarted,
                            )
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
                // Fix round 4 (ruling 4): the emulator is restarted at most once in a stretch.
                if skip_emulator {
                    ladder.skip_emulator_restart();
                }
                ladder
            };
            self.admit_held_recovery(ladder)?;
        }
        Ok(())
    }

    /// The blocked instances and (fix round 4, rulings 2 and 3) the pairs whose latest scheduled
    /// runs failed at their entry, by alias, as the evaluation reads them.
    pub(super) fn entry_block_gates(&self) -> RuntimeHostResult<EntryBlockGates> {
        let (blocked, pairs) = {
            let blocks = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
            (
                blocks
                    .blocked
                    .iter()
                    .map(|(instance_id, block)| (*instance_id, block.gate.clone()))
                    .collect::<Vec<_>>(),
                blocks
                    .pairs
                    .iter()
                    .map(|((instance_id, task_id), pair)| {
                        (
                            *instance_id,
                            task_id.clone(),
                            EntryPairGate {
                                decision_id: pair.decision_id.clone(),
                                failure_code: pair.failure_code.clone(),
                                observed_at_unix_ms: pair.observed_at_unix_ms,
                                in_a_row: pair.in_a_row,
                                until_unix_ms: pair.until_unix_ms,
                                window_end_unix_ms: pair.window_end,
                                set_aside: pair.set_aside.then_some((pair.against, pair.threshold)),
                            },
                        )
                    })
                    .collect::<Vec<_>>(),
            )
        };
        if blocked.is_empty() && pairs.is_empty() {
            return Ok(EntryBlockGates::default());
        }
        let instances = lock(&self.registered_instances, HostLocation::InstanceEntryBlock)?;
        let alias = |instance_id: &InstanceId| {
            instances
                .get(instance_id)
                .map(|instance| instance.instance_alias.clone())
        };
        let blocked = blocked
            .into_iter()
            .filter_map(|(instance_id, gate)| alias(&instance_id).map(|alias| (alias, gate)))
            .collect::<BTreeMap<_, _>>();
        let pairs = pairs
            .into_iter()
            .filter_map(|(instance_id, task_id, gate)| {
                alias(&instance_id).map(|alias| ((alias, task_id), gate))
            })
            .collect::<BTreeMap<_, _>>();
        Ok(EntryBlockGates::new(blocked, pairs))
    }

    /// Fix round 4 (ruling 3): `instance_entry_task_set_aside_reported` of the task `task_id` on
    /// `instance_id`, whose failure at the entry, `pair`'s latest, set it aside until the end of
    /// its activity window: a person is told once, by a warning record under the instance.
    fn record_entry_set_aside(
        &self,
        instance_id: InstanceId,
        task_id: &str,
        pair: &EntryPair,
    ) -> RuntimeHostResult<()> {
        let outcome = Outcome::new(HostCode::InstanceEntryTaskSetAsideReported)
            .operation(HostLocation::InstanceEntryBlock)
            .instance_id(identifier_text(&instance_id))
            .task_id(task_id)
            .decision_id(pair.decision_id.as_str())
            .observed_at_unix_ms(pair.observed_at_unix_ms)
            .count(pair.against)
            .limit_count(pair.threshold)
            .window_end_unix_ms(pair.window_end);
        let outcome = match pair.failure_code.registered() {
            Some(code) => outcome.failure_code(code),
            None => outcome,
        };
        self.record_entry_block_warning(instance_id, outcome)
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

    /// Test only: what the next settlement of the dispatch `decision_id` was armed to be; taken
    /// once. A run armed as failed at its entry carries the instance's marks now (the live
    /// settlement reads them at the run's start, which the replays exercise) and the activity
    /// window of its admission; one armed as past its entry started its main package after its
    /// entry check.
    #[cfg(test)]
    pub(super) fn entry_settlement_for_test(
        &self,
        decision_id: &str,
    ) -> RuntimeHostResult<Option<EntrySettlement>> {
        let armed = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?
            .settle_for_test
            .take();
        let Some(armed) = armed else {
            return Ok(None);
        };
        if armed == SettleForTest::PastEntry {
            return Ok(Some(EntrySettlement::TargetStarted { entered: true }));
        }
        let (instance_alias, window_end) = {
            let policy = lock(self.policy()?, HostLocation::InstanceEntryBlock)?;
            (
                policy.execution_instance_id(decision_id)?.to_owned(),
                policy.dispatch_window_end(decision_id)?.unwrap_or_default(),
            )
        };
        let instance_id = self
            .resolve_instance(&instance_alias)
            .map_err(|failure| *failure.error)?
            .instance_id();
        Ok(Some(EntrySettlement::AtEntry {
            marks: self.entry_marks(instance_id)?,
            window_end,
        }))
    }

    /// Fix round 4, test only: the pair of `task_id` on `instance_id`, if its latest scheduled run
    /// failed at its entry: its failures at the entry in a row, those against it (ruling 3), the
    /// time before which it is not dispatched, and whether it is set aside.
    #[cfg(test)]
    pub(super) fn entry_pair_for_test(
        &self,
        instance_id: InstanceId,
        task_id: &str,
    ) -> RuntimeHostResult<Option<EntryPairForTest>> {
        Ok(lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?
            .pairs
            .get(&(instance_id, task_id.to_owned()))
            .map(|pair| {
                (
                    pair.in_a_row,
                    pair.against,
                    pair.until_unix_ms,
                    pair.set_aside,
                )
            }))
    }

    /// Fix round 6, test only: the pair of `task_id` on `instance_id` as a run of failures at the
    /// entry in an unknown window left it: `in_a_row` failures in a row (its interval the
    /// configuration's for that count), the latest observed at `latest_unix_ms` (fix round 7: the
    /// turn order at a block's end reads it), waiting until `until_unix_ms`; with `set_aside`,
    /// one failure against it under a threshold of 1, set aside until a day after the Runtime
    /// clock's time now (fix round 7: CI on fix round 6's head; the end of time it had was no
    /// integer an outcome takes, which refuses one above `i64::MAX`). It replaces the pair's own.
    #[cfg(test)]
    pub(super) fn seed_entry_pair_for_test(
        &self,
        instance_id: InstanceId,
        task_id: &str,
        (in_a_row, latest_unix_ms, until_unix_ms, set_aside): (u32, u64, u64, bool),
    ) -> RuntimeHostResult<()> {
        const SEEDED_WINDOW_MS: u64 = 86_400_000;
        let config = self.unavailable_probe_config()?;
        let now_unix_ms = self.runtime_clock_sample()?.unix_ms;
        let mut blocks = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
        let entered = count(&blocks.entered, instance_id);
        blocks.pairs.insert(
            (instance_id, task_id.to_owned()),
            EntryPair {
                packages: String::new(),
                window_end: if set_aside {
                    now_unix_ms.saturating_add(SEEDED_WINDOW_MS)
                } else {
                    0
                },
                decision_id: format!("seeded-{task_id}"),
                failure_code: actingcommand_contract::scheduled_failure_code_str(&Outcome::new(
                    actingcommand_contract::codes::SharedCode::ContainedTaskPageUnknown,
                )),
                observed_at_unix_ms: latest_unix_ms,
                in_a_row,
                interval_ms: pair_interval_ms(config, in_a_row),
                until_unix_ms,
                entered,
                against: u32::from(set_aside),
                threshold: 1,
                set_aside,
            },
        );
        Ok(())
    }

    /// Fix round 8, test only: the failure at the entry of `task_id`'s pair on `instance_id` renewed
    /// the instance's block, as a scheduled run's failure at the entry settled while the block
    /// stands does (`block_instance_entry`): the block counts the task among the tasks whose
    /// failures set or renewed it (`EntryBlock::setters`). `false` when the instance is not
    /// blocked.
    #[cfg(test)]
    pub(super) fn seed_entry_setter_for_test(
        &self,
        instance_id: InstanceId,
        task_id: &str,
    ) -> RuntimeHostResult<bool> {
        let mut blocks = lock(&self.entry_blocks, HostLocation::InstanceEntryBlock)?;
        let Some(block) = blocks.blocked.get_mut(&instance_id) else {
            return Ok(false);
        };
        block.setters.insert(task_id.to_owned());
        Ok(true)
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
