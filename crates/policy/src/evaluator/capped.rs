// SPDX-License-Identifier: AGPL-3.0-only

//! SS-2h (register §1.289; Alice's ruling 138 of 11-07, her answers 172 A and 173 of 11-09 16:39,
//! and her approval of the two water lines at 17:37, 17:42 and 17:48; design
//! `rd-1031/design-c424-redline-1109.md` §1.6 and §4.1; `contracts/scheduling/README.md`, "Pool
//! lines", "Capped spending order"): a pool's two lines and the capped spending order, inside the
//! scheduler.
//!
//! - **The lines.** A valued pool on an instance has a stop line `S` and a hard line `H`: its
//!   `stop_line` and `hard_line`, each a fact of the pool's scope plus an offset or a constant,
//!   read at each evaluation; without them `S` is the capacity and `H` is 0. They must hold
//!   `0 ≤ H < S ≤ cap` as read, or the pool's judged consumers are held (`lines_invalid`). The
//!   Runtime never computes a line.
//! - **The reading** `r` is the pool's projected amount less the consumption its reading does
//!   not show yet (the reserve gate's step 3: every task dispatched on the instance at or after
//!   the reading's observation).
//! - **Below the cap** a judged candidate that would pass (positive, or released in the zero
//!   tier) and whose run leaves `r − c < H` is held (`would_cross_hard_line`). A pool without
//!   lines whose reading is unknown has no hard line to keep, as before; one with lines is held
//!   (`capped_cap_unknown`).
//! - **The trigger line** (SCHED-C1c, register §1.299): a pool that writes both lines may write
//!   `trigger_line` `T` in the same two forms, read with them; without it `T` is the capacity. The
//!   lines read must then hold `0 ≤ H < S ≤ T ≤ cap` (`lines_invalid`, with `trigger_line`).
//!   Below `T`, while no episode is under way, the pool is below the cap for the rules above.
//! - **The capped state.** At `r ≥ T` (at the start too; SCHED-C1c: `T` where SS-2h read the cap)
//!   the pool enters a capped episode on the instance; it ends once `r ≤ S` and `r < T`
//!   ([`Lines::at_trigger`], [`Lines::spent`]). A round lists the judged consumers of the
//!   pool that are candidates now, whose value without the pool's cost `V` is above its rounding
//!   bound (never by their cost), and, from the second round on, only those not marked
//!   `benefit_limited`. It freezes them by `V ÷ c`, the higher first, then by task id, and plans
//!   from `r`: each one in turn while the accumulated reading is above `S` (or at the cap), one
//!   whose run would end below `H` skipped. Before each dispatch the actual `r` is read again: the
//!   head (the first planned one not dispatched in the round that is a candidate now) must not end
//!   below `H`, or the round ends. A round whose planned tasks were all dispatched or passed ends;
//!   the next one starts while `r` is above `S`. A round that lists or plans nothing though a
//!   consumer is a candidate ends the episode (`capped_round_empty`); while no consumer is a
//!   candidate at all the episode waits for its next round.
//! - **Only what can be dispatched now** (SS-2h fix 1; Alice 17:37, 「只加入现在空闲且能被调度/执行的
//!   任务进入排序」). (a) A round lists the candidates of the evaluation, past every gate the policy
//!   judges before the order. (b) The gates judged after it (the reserve gate, and the Runtime's
//!   admission with its budgets, windows and holds, which the evaluator learns only at admission):
//!   a block candidate one of them refuses for its own sake is passed for the round with that
//!   gate's code ([`settle`]); the plan goes on without it, so the next one is tried, and in later
//!   evaluations of the round it is held `capped_not_dispatchable` with that `gate_code`. A
//!   whole-instance hold, the Runtime's random delay and a chain step pass none: the round waits.
//!   (c) When no planned one left is a candidate now while a consumer is, the round ends.
//! - **Dispatch (layout 乙).** The head and the planned candidates after it that fit above `H`,
//!   then their fallbacks (what the round would plan once every candidate of the block before them
//!   is passed), are the pool's only consumers in the allocation, one block in the order at the
//!   head's place (the head's tier and `B ÷ E`, `B = V − W·(c − a)`, `a = min(c, max(0, r − S))`);
//!   the first of them that every later gate admits is dispatched (`capped_spend`), the others wait
//!   (`capped_not_head`; a fallback with the hold the round gives it). Every other gate stays.
//! - **Lines that cannot spend** (SCHED-C1c; `contracts/scheduling/README.md`, "A pool its lines
//!   cannot spend"): when every judged consumer of the pool on the instance, candidate or not,
//!   consumes more in one run than `cap − H`, no run can start from the cap above the hard line. At
//!   `r ≥ T` no episode then begins (one under way waits) and the pool's judged candidates are
//!   held with the warning `pool_lines_unspendable`.
//! - **The memory** ([`CappedMemory`]): the episodes, their round numbers, frozen orders and
//!   passed members, as SS-4's `NoProgressMemory` keeps its guards. Which tasks a round
//!   dispatched is read from the task snapshots (`last_dispatched_unix_ms` at or after the
//!   round's start).
//! - **Episodes survive** (SCHED-C1d, register §1.299 SC1-11; `contracts/scheduling/README.md`,
//!   "Episodes survive"): an episode is kept by its pool's consumers on the instance (the
//!   consumers key: each consumer by its accounting identity, its id or one its `continues_from`
//!   names, its `⌈C/1000⌉` and its `benefit_limited`), not by the catalog generation: a new
//!   generation that leaves them unchanged keeps it, any other change ends it. While the
//!   capacity, the reading or a line's fact gives no value, an episode under way waits, and its
//!   judged candidates are held with `capped_cap_unknown`, whether or not the pool writes lines.
//!   The Runtime keeps the memory across a start as the tagged program fact
//!   `host.capped_episodes` ([`CappedMemory::kept`], [`CappedMemory::restore`], module
//!   [`kept`]); the host owns the fact, this module the shape and the rules of an episode.
//! - **Lines for one instance** (SCHED-C1e, register §1.299 SC1-12;
//!   `contracts/scheduling/README.md`, "Lines for one instance"): the host reads each instance's
//!   `instance.<instance id>.scheduling.pool_lines` in effect at every evaluated cycle and hands
//!   it to the evaluation ([`InstancePoolLines`]). For a pool its set names, on that instance,
//!   the set replaces the catalog's lines whole (a trigger line it does not write is the
//!   capacity, not the catalog's); otherwise the catalog's lines, otherwise the defaults
//!   ([`lines`]). `S ≤ cap` and `T ≤ cap` are checked at each evaluation as before
//!   (`lines_invalid`). A change ends no episode: the next evaluation reads the new lines, and
//!   the episode ends only by its own rule against them. Every reason that carries the lines
//!   (`pool_line_held`, `capped_spend`, `pool_lines_unspendable`) names where they come from,
//!   `lines_source` (`instance`, `catalog` or `default`).
//!
//! The lines constrain the scheduler only: a manual request, a chain successor that rides with its
//! chain and the Runtime's own tasks are no judged candidates and are not held.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::outcome::{Code, Outcome};
use actingcommand_selection_policy::UnknownReason;

use super::benefit::{
    ConsumedTerm, ExactSum, KnownBenefit, compare_ratios, expected_duration_ms, manual_request,
    ratio, saturating_milli,
};
use super::tie_break::TieKey;
use super::{
    Amounts, ConsumptionAmounts, DecisionReason, EvaluationFacts, InstanceSnapshot,
    PlacementCandidate, PlacementContext, PolicyEvaluationError, PolicyEvaluationResult,
    SchedulingDecisionState, TaskWork, UnknownReasonToken, live_pool_unknown_reason, pool_bound,
    project_pool_value, reason_with, scope_matches_identity, scope_matches_instance, unknown_token,
    with_values,
};
use crate::codes::{LinesSource, PolicyCode, PoolLineHold};
use crate::{
    CompiledCatalog, FactValue, ObservationRef, PoolCapacity, PoolLine, PoolSpec, TaskSpec,
};

// SCHED-C1d (register §1.299 SC1-11): what the Runtime keeps of the memory across a start.
mod kept;

pub use kept::{
    CappedRestore, CappedStateProblem, KeptConsumer, KeptEpisode, KeptMember, KeptRound,
};

/// SS-2h: the Runtime's memory of the capped episodes, per (instance alias, pool id). Nothing here
/// enters a snapshot or a decision identity. The Runtime passes it to
/// [`super::evaluate_with_capped_spending`] at every evaluation of a cycle; every other entry point
/// evaluates with an empty one. SCHED-C1d: the Runtime keeps it across a start as the tagged
/// program fact `host.capped_episodes` ([`Self::kept`], [`Self::restore`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CappedMemory {
    episodes: BTreeMap<(String, String), Episode>,
    /// How many episodes each pair began since the start; SCHED-C1d: a restored episode's
    /// number counts on from it.
    begun: BTreeMap<(String, String), u64>,
}

impl CappedMemory {
    /// Whether no pool is in a capped episode.
    pub fn is_empty(&self) -> bool {
        self.episodes.is_empty()
    }

    /// The capped episode of `pool_id` on `instance_id`, if one is active: its number since the
    /// start and the number of its current (or last) round.
    pub fn episode(&self, instance_id: &str, pool_id: &str) -> Option<(u64, u64)> {
        self.episodes
            .get(&(instance_id.to_owned(), pool_id.to_owned()))
            .map(|episode| (episode.number, episode.rounds))
    }
}

/// SCHED-C1e (register §1.299 SC1-12; `contracts/scheduling/README.md`, "Lines for one
/// instance"): one pool's lines given for one instance, each in the two forms a catalog line has
/// (a reference reads its fact in the pool's scope). It replaces the catalog's lines of the pool
/// whole: without `trigger_line` the pool's trigger line is its capacity, not the catalog's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceLineSet {
    pub stop_line: PoolLine,
    pub hard_line: PoolLine,
    pub trigger_line: Option<PoolLine>,
}

/// SCHED-C1e: the lines given for each instance, by instance alias and then by the pool id the
/// instance's `scheduling.pool_lines` names: the id the instance's `schedule.json` writes (a
/// composed copy's id without `@<alias>`), or in catalog mode the catalog's id. The Runtime reads
/// them from the configuration in effect at every evaluated cycle and passes them to
/// [`super::evaluate_with_instance_lines`]; every other entry point evaluates with none, so each
/// pool takes its catalog's lines.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstancePoolLines {
    instances: BTreeMap<String, BTreeMap<String, InstanceLineSet>>,
}

impl InstancePoolLines {
    /// Whether no instance sets lines.
    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    /// Sets `lines` for the pool `pool_id` names on the instance `instance_id` (its alias).
    pub fn insert(&mut self, instance_id: &str, pool_id: &str, lines: InstanceLineSet) {
        self.instances
            .entry(instance_id.to_owned())
            .or_default()
            .insert(pool_id.to_owned(), lines);
    }

    /// The set the instance `instance_id` gives the catalog pool `pool_id`: the one named by the
    /// pool's id, or by the id of which it is the instance's composed copy (`<id>@<instance_id>`).
    fn get(&self, instance_id: &str, pool_id: &str) -> Option<&InstanceLineSet> {
        let pools = self.instances.get(instance_id)?;
        pools.get(pool_id).or_else(|| {
            pool_id
                .strip_suffix(instance_id)
                .and_then(|base| base.strip_suffix('@'))
                .and_then(|base| pools.get(base))
        })
    }
}

/// SCHED-C1e (`contracts/scheduling/README.md`, "Lines for one instance", *Checks*): the pools
/// `catalog` gives the instance `instance_id` (its alias, of `server_id` and `game_id`) with a
/// declared valuation, each by the id the instance's `scheduling.pool_lines` names it by (a
/// composed copy's id without `@<instance_id>`, otherwise the catalog's id). The host fills the
/// configuration's check context with them, so an instruction names only these.
pub fn instance_valued_pool_ids(
    catalog: &CompiledCatalog,
    instance_id: &str,
    server_id: &str,
    game_id: &str,
) -> BTreeSet<String> {
    catalog
        .catalog()
        .pools
        .pools
        .iter()
        .filter(|pool| {
            pool.declared_valuation().is_some()
                && scope_matches_identity(&pool.scope, instance_id, server_id, game_id)
        })
        .map(|pool| {
            pool.id
                .strip_suffix(instance_id)
                .and_then(|base| base.strip_suffix('@'))
                .unwrap_or(&pool.id)
                .to_owned()
        })
        .collect()
}

/// One capped episode of a pool on an instance.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Episode {
    /// SCHED-C1d: the consumers key, the pool's consumers on the instance as the episode began,
    /// by task id: a generation that changes them ends the episode ([`Episode::keep_consumers`]).
    consumers: Vec<ConsumerKey>,
    number: u64,
    /// How many rounds it began.
    rounds: u64,
    round: Option<Round>,
}

/// SCHED-C1d: one consumer of an episode's pool on its instance, as the consumers key reads it:
/// its task id, its `⌈C/1000⌉` and whether it is marked `benefit_limited`. SCHED-C1f1 (register
/// §1.303 SC1F-3, ruling 2): `⌈C/1000⌉` is the declared consumption, an amount read from a fact at
/// its fallback, never the value read: a changed reading keeps the episode, a changed fallback
/// ends it as a changed constant does.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ConsumerKey {
    task_id: String,
    consumed: u64,
    limited: bool,
}

impl ConsumerKey {
    fn of(consumer: &Consumer<'_>, pool_id: &str) -> PolicyEvaluationResult<Self> {
        Ok(Self {
            task_id: consumer.task.id.clone(),
            consumed: crate::resource_targets::consumption(
                consumer.task,
                pool_id,
                Amounts::Declared,
            )?,
            limited: consumer.task.is_benefit_limited(),
        })
    }
}

/// SCHED-C1d: the consumers key of `consumers` of `pool_id`, by task id.
fn consumer_keys(
    consumers: &[Consumer<'_>],
    pool_id: &str,
) -> PolicyEvaluationResult<Vec<ConsumerKey>> {
    let mut keys = consumers
        .iter()
        .map(|consumer| ConsumerKey::of(consumer, pool_id))
        .collect::<PolicyEvaluationResult<Vec<_>>>()?;
    keys.sort();
    Ok(keys)
}

/// SCHED-C1d: whether `task` accounts as `task_id`: it is that task, or its `continues_from`
/// names it (Workflow #372 K5's accounting identity).
fn accounts_as(task: &TaskSpec, task_id: &str) -> bool {
    task.id == task_id
        || task
            .continues_from
            .as_deref()
            .is_some_and(|continued| continued.iter().any(|id| id == task_id))
}

impl Episode {
    /// SCHED-C1d ("Episodes survive"): whether `consumers`, the pool's consumers on the
    /// instance at this evaluation, are the episode's, each kept consumer matched by its
    /// accounting identity with the same consumption and the same mark. When they are, a kept
    /// consumer that now runs under another id (its `continues_from` names the kept one) takes
    /// that id in the key and in the round's frozen order, which stays as it is.
    fn keep_consumers(
        &mut self,
        consumers: &[Consumer<'_>],
        pool_id: &str,
    ) -> PolicyEvaluationResult<bool> {
        if consumers.len() != self.consumers.len() {
            return Ok(false);
        }
        let mut taken = BTreeSet::new();
        let mut renamed = BTreeMap::<String, String>::new();
        for kept in &self.consumers {
            let Some((index, consumer)) = consumers.iter().enumerate().find(|(index, consumer)| {
                !taken.contains(index) && accounts_as(consumer.task, &kept.task_id)
            }) else {
                return Ok(false);
            };
            let now = ConsumerKey::of(consumer, pool_id)?;
            if now.consumed != kept.consumed || now.limited != kept.limited {
                return Ok(false);
            }
            taken.insert(index);
            if now.task_id != kept.task_id {
                renamed.insert(kept.task_id.clone(), now.task_id);
            }
        }
        if renamed.is_empty() {
            return Ok(true);
        }
        for kept in &mut self.consumers {
            if let Some(id) = renamed.get(&kept.task_id) {
                kept.task_id.clone_from(id);
            }
        }
        self.consumers.sort();
        if let Some(round) = self.round.as_mut() {
            for member in &mut round.order {
                if let Some(id) = renamed.get(&member.task_id) {
                    member.task_id.clone_from(id);
                }
            }
        }
        Ok(true)
    }
}

/// One round: its frozen order, the reading its plan starts from, and the members it passed.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Round {
    started_at_unix_ms: u64,
    /// The reading at the round's start, which its plan accumulates from ([`Round::plan`]).
    reading: i128,
    order: Vec<Member>,
    /// SS-2h fix 1 (b): the indices into `order` that a gate of their own refused after the round
    /// listed them ([`settle`]), with the registered code of that gate's reason: passed for the
    /// round, they take no part in its plan.
    passed: BTreeMap<usize, Option<Code>>,
    /// The indices into `order` of the block the latest evaluation offered to the allocation,
    /// which [`settle`] reads once.
    offered: Vec<usize>,
}

impl Round {
    /// The round's plan (module doc), the members `assumed` passed as well: from the round's
    /// starting reading, the order taken in turn. A passed member consumes nothing; one
    /// `dispatched` in the round is planned with its consumption; while the accumulated reading is
    /// not spent, one whose run would end below the hard line is skipped (with what the pool would
    /// hold after it) and any other planned. Indices into `order`, in plan order. With nothing
    /// passed or dispatched it is the plan the round began with. SCHED-C1f1 (register §1.303
    /// SC1F-3, ruling 2): each member's consumption is the one in effect at this evaluation
    /// (`amounts`), not the one its order was frozen with.
    fn plan(
        &self,
        lines: Lines,
        (dispatched, assumed): (&BTreeSet<usize>, &BTreeSet<usize>),
        amounts: &MemberAmounts,
    ) -> (Vec<usize>, Vec<(usize, i128)>) {
        let mut planned = Vec::new();
        let mut skipped = Vec::new();
        let mut reading = self.reading;
        for (index, member) in self.order.iter().enumerate() {
            if self.passed.contains_key(&index) || assumed.contains(&index) {
                continue;
            }
            let consumed = i128::from(member.in_effect(amounts));
            if dispatched.contains(&index) {
                planned.push(index);
                reading -= consumed;
                continue;
            }
            if lines.spent(reading) {
                break;
            }
            let after = reading - consumed;
            if after < i128::from(lines.hard) {
                skipped.push((index, after));
                continue;
            }
            planned.push(index);
            reading = after;
        }
        (planned, skipped)
    }
}

/// One task of a round's frozen order.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Member {
    task_id: String,
    /// `⌈C/1000⌉`, the consumption the order was frozen with. SCHED-C1f1 (register §1.303 SC1F-3):
    /// kept as the row of `host.capped_episodes` keeps it; the plan and the checks read the
    /// amount in effect ([`Member::in_effect`]).
    consumed: u64,
}

/// SCHED-C1f1 (register §1.303 SC1F-3): each consumer's `⌈C/1000⌉` of the pool on the instance
/// at this evaluation, by task id, every amount read from a fact in effect.
type MemberAmounts = BTreeMap<String, u64>;

impl Member {
    /// The member's consumption in effect at this evaluation; the one its order was frozen with
    /// when it is no consumer now (a member's consumer ended the episode if it was dropped).
    fn in_effect(&self, amounts: &MemberAmounts) -> u64 {
        amounts.get(&self.task_id).copied().unwrap_or(self.consumed)
    }
}

/// A judged candidate whose net benefit is known, as the benefit gate judged it.
pub(super) struct Known<'a> {
    pub(super) candidate: &'a PlacementCandidate,
    pub(super) task: &'a TaskSpec,
    pub(super) sum: &'a ExactSum,
    pub(super) tolerance: &'a ExactSum,
    pub(super) consumed: &'a [ConsumedTerm],
}

/// What a pool's lines decide for one ranked candidate.
pub(super) enum Disposition {
    /// No capped order holds it: the gate decides as before; `hold`, when present, holds it if it
    /// would pass (its hard line, or another consumer's capped order).
    Normal { hold: Option<DecisionReason> },
    /// A capped order holds it back this round.
    Held(DecisionReason),
    /// A capped round tries it: it passes with its own `B`, whatever its sign.
    Member {
        benefit: KnownBenefit,
        slot: Box<CappedSlot>,
    },
}

/// SS-2h: a candidate's place in a capped round's block: the pool, its place among the block's
/// candidates, the reason its dispatch carries, and the block's lead, whose order fields the whole
/// block takes in the allocation ([`assign_leads`]).
#[derive(Debug, Clone)]
pub(super) struct CappedSlot {
    pub(super) pool_id: String,
    pub(super) position: usize,
    pub(super) spend: DecisionReason,
    pub(super) lead: Option<LeadKey>,
    /// SS-2h fix 1 (b): for a fallback of the block (one the round would plan only once every
    /// candidate of the block before it is passed), the reason it is held with, and its wake,
    /// when an earlier one is dispatched or another candidate takes the instance: its round's
    /// hold, as when it was in no block. `None` for a planned one, which waits with
    /// `capped_not_head`.
    pub(super) wait: Option<DecisionReason>,
    pub(super) wake_unix_ms: Option<u64>,
}

/// SS-2h: the order fields of a capped block's lead, the candidate of the block with the lowest
/// place.
#[derive(Debug, Clone)]
pub(super) struct LeadKey {
    pub(super) promoted: bool,
    pub(super) business: bool,
    pub(super) zero_tier: bool,
    pub(super) benefit: Option<KnownBenefit>,
    pub(super) total_score: i64,
    pub(super) affinity: bool,
    pub(super) tie: TieKey,
    pub(super) tie_breaker: u64,
    pub(super) task_id: String,
}

/// SS-2h: gives every candidate of a capped block its block's lead ([`LeadKey`]), so that the
/// allocation order keeps the block together at its lead's place, in the block's order. Called
/// before every sort by the allocation order, after the promotions.
pub(super) fn assign_leads(candidates: &mut [PlacementCandidate]) {
    let mut leads = BTreeMap::<(String, String), (usize, LeadKey)>::new();
    for candidate in candidates.iter() {
        let Some(slot) = &candidate.capped else {
            continue;
        };
        let key = (candidate.instance_id.clone(), slot.pool_id.clone());
        if leads
            .get(&key)
            .is_some_and(|(position, _)| *position <= slot.position)
        {
            continue;
        }
        leads.insert(
            key,
            (
                slot.position,
                LeadKey {
                    promoted: candidate.promoted,
                    business: candidate.business,
                    zero_tier: candidate.zero_tier,
                    benefit: candidate.benefit,
                    total_score: candidate.rank.total_score,
                    affinity: candidate.affinity,
                    tie: candidate.tie.clone(),
                    tie_breaker: candidate.tie_breaker,
                    task_id: candidate.task_id.clone(),
                },
            ),
        );
    }
    for candidate in candidates.iter_mut() {
        let instance_id = candidate.instance_id.clone();
        if let Some(slot) = candidate.capped.as_mut() {
            slot.lead = leads
                .get(&(instance_id, slot.pool_id.clone()))
                .map(|(_, lead)| lead.clone());
        }
    }
}

/// The lines of one pool on one instance at the evaluation, in the pool's real units.
#[derive(Debug, Clone, Copy)]
struct Lines {
    capacity: u64,
    /// The projected amount less the consumption the reading does not show yet.
    reading: i128,
    stop: u64,
    hard: u64,
    /// SCHED-C1c: the trigger line the pool writes, as read; `None` without one.
    trigger_line: Option<u64>,
    /// SCHED-C1e: where the lines come from: the instance's own set, the catalog or the defaults.
    source: LinesSource,
}

impl Lines {
    /// SCHED-C1c (register §1.299): the trigger line `T`, the capacity without one. The episode's
    /// rule reads it where SS-2h read the cap: it begins at `r ≥ T` and is spent at `r ≤ S`,
    /// `r < T`.
    fn trigger(&self) -> u64 {
        self.trigger_line.unwrap_or(self.capacity)
    }

    /// Whether the pool is at or above its trigger line: an episode begins.
    fn at_trigger(&self) -> bool {
        self.reading >= i128::from(self.trigger())
    }

    /// Whether `reading` is spent down: at or below the stop line and below the trigger line.
    fn spent(&self, reading: i128) -> bool {
        reading <= i128::from(self.stop) && reading < i128::from(self.trigger())
    }

    /// SCHED-C1c: the room a run has from the cap above the hard line, `cap − H`.
    fn room(&self) -> u64 {
        self.capacity.saturating_sub(self.hard)
    }
}

/// Why a pool's lines cannot be kept at the evaluation.
enum LinesProblem {
    /// The capacity, the reading or a line's fact gives no value; `written` when the pool writes
    /// its lines.
    Unknown {
        written: bool,
        fact_key: Option<String>,
        unknown_reason: Option<UnknownReason>,
    },
    /// The lines read break `0 ≤ H < S ≤ cap` (SCHED-C1c: `0 ≤ H < S ≤ T ≤ cap`, `trigger` the
    /// trigger line the pool writes, as read; SCHED-C1e: `source` where the lines come from).
    Invalid {
        capacity: u64,
        stop: i128,
        hard: i128,
        trigger: Option<i128>,
        source: LinesSource,
    },
}

/// The pools' lines and the capped spending order over the benefit gate's judged candidates
/// (module doc). `ranked` are the gate's candidates and `known` their judgements, aligned; the
/// answer is aligned with them. Reasons for the pools' consumers that are no candidates are added
/// to `work`; `memory` is updated.
pub(super) fn plan(
    context: &PlacementContext<'_>,
    facts: &EvaluationFacts,
    ranked: &[PlacementCandidate],
    known: &[Option<Known<'_>>],
    work: &mut [TaskWork],
    memory: &mut CappedMemory,
) -> PolicyEvaluationResult<Vec<Disposition>> {
    let mut decided = (0..ranked.len())
        .map(|_| Decided::default())
        .collect::<Vec<_>>();
    let mut considered = BTreeSet::new();
    for instance in &facts.instances {
        for pool in context.pool_specs.values() {
            if pool.declared_valuation().is_none() || !scope_matches_instance(&pool.scope, instance)
            {
                continue;
            }
            let consumers = consumers(context, instance, pool)?;
            if consumers.is_empty() {
                continue;
            }
            let key = (instance.instance_id.clone(), pool.id.clone());
            // SCHED-C1d ("Episodes survive"): an episode is kept while the pool's consumers on the
            // instance are its own, whatever else a new catalog generation changed.
            let same_consumers = match memory.episodes.get_mut(&key) {
                Some(episode) => episode.keep_consumers(&consumers, &pool.id)?,
                None => true,
            };
            if !same_consumers {
                memory.episodes.remove(&key);
            }
            // The known judged candidates of this instance that consume the pool, by task id.
            let candidates = known
                .iter()
                .enumerate()
                .filter_map(|(index, known)| {
                    let known = known.as_ref()?;
                    if known.candidate.instance_id != instance.instance_id {
                        return None;
                    }
                    let term = known
                        .consumed
                        .iter()
                        .find(|term| term.pool_id == pool.id && term.consumed_milli > 0)?;
                    Some((known.task.id.as_str(), (index, known, term)))
                })
                .collect::<BTreeMap<_, _>>();
            if candidates.is_empty() && !memory.episodes.contains_key(&key) {
                continue;
            }
            considered.insert(key.clone());
            // SCHED-C1f1 (register §1.303 SC1F-4): a hold of a candidate whose consumption of the
            // pool is read from a fact names that consumption and where it comes from.
            let for_task = |task_id: &str, reason: DecisionReason| {
                with_values(
                    reason,
                    context
                        .amounts
                        .values(task_id, &instance.instance_id, &pool.id),
                )
            };
            // SCHED-C1f1 (SC1F-3, ruling 2): each consumer's consumption in effect on the instance,
            // which the round's plan and checks read.
            let in_effect = member_amounts(
                &consumers,
                &pool.id,
                context.amounts.on(&instance.instance_id),
            )?;
            let lines = match lines(context, facts, instance, pool)? {
                Ok(lines) => lines,
                // SCHED-C1d: an episode under way waits on an unknown value: its judged candidates
                // are held with `capped_cap_unknown`, whether or not the pool writes lines, and
                // it goes on, or ends by its rule, once the values come back.
                Err(LinesProblem::Unknown {
                    fact_key,
                    unknown_reason,
                    ..
                }) if memory.episodes.contains_key(&key) => {
                    let hold = unknown_lines_reason(&pool.id, fact_key.as_deref(), unknown_reason);
                    for (&task_id, (index, _, _)) in &candidates {
                        decided[*index].held(for_task(task_id, hold.clone()));
                    }
                    continue;
                }
                Err(problem) => {
                    memory.episodes.remove(&key);
                    let hold = match problem {
                        LinesProblem::Unknown { written: false, .. } => None,
                        LinesProblem::Unknown {
                            fact_key,
                            unknown_reason,
                            ..
                        } => Some(unknown_lines_reason(
                            &pool.id,
                            fact_key.as_deref(),
                            unknown_reason,
                        )),
                        LinesProblem::Invalid {
                            capacity,
                            stop,
                            hard,
                            trigger,
                            source,
                        } => Some(invalid_lines_reason(
                            &pool.id,
                            capacity,
                            (stop, hard, trigger),
                            source,
                        )),
                    };
                    if let Some(hold) = hold {
                        for (&task_id, (index, _, _)) in &candidates {
                            decided[*index].normal_hold(&for_task(task_id, hold.clone()));
                        }
                    }
                    continue;
                }
            };
            // SCHED-C1c ("A pool its lines cannot spend"): at or above the trigger line, lines
            // under which no consumer's run fits from the cap begin no episode (one under way
            // waits), and the pool's judged candidates are held with the warning. SCHED-C1f1: by
            // each consumer's consumption in effect; the warning names the least one's.
            if lines.at_trigger()
                && let Some((least, least_task)) = least_consumption(&consumers, &in_effect)
                && least > lines.room()
            {
                let reason = for_task(least_task, unspendable_reason(&pool.id, lines, least));
                for (index, _, _) in candidates.values() {
                    decided[*index].held(reason.clone());
                }
                continue;
            }
            let in_round = step(
                context,
                memory,
                &key,
                (lines, &in_effect),
                &candidates,
                work,
                &consumers,
                instance,
            )?;
            let Some(view) = in_round else {
                // Below the cap (SCHED-C1c: the trigger line), or the episode ended: the hard line
                // holds a run that would cross it (SCHED-C1f1: by its consumption in effect, which
                // its judgement read).
                for (&task_id, (index, _, term)) in &candidates {
                    let after = lines.reading - i128::from(units(term.consumed_milli)?);
                    if after < i128::from(lines.hard) {
                        decided[*index].normal_hold(&for_task(
                            task_id,
                            hard_line_reason(&pool.id, lines, after),
                        ));
                    }
                }
                continue;
            };
            let offered = view.decide(
                (&pool.id, instance),
                (lines, &in_effect, context.amounts),
                ranked,
                &candidates,
                &mut decided,
                work,
                &consumers,
            )?;
            // SS-2h fix 1: the block offered to the allocation, which `settle` reads after it.
            if let Some(round) = memory
                .episodes
                .get_mut(&key)
                .and_then(|episode| episode.round.as_mut())
            {
                round.offered = offered;
            }
        }
    }
    memory.episodes.retain(|key, _| considered.contains(key));
    Ok(decided.into_iter().map(Decided::disposition).collect())
}

/// SS-2h fix 1 (b) (module doc): after the allocation, each candidate of a capped block whose work
/// entry a gate of its own deferred (the reserve gate, the Runtime's admission with its budgets,
/// windows and package holds, a remedy round) is passed for its round, with the registered code of
/// that gate's reason, so the round's plan goes on without it and the round ends once none is left.
/// None is passed on an instance a whole-instance hold holds (`whole_instance_held`, of a task and
/// an instance: nothing could be dispatched there), nor one whose dispatch only waits (`waits`, the
/// Runtime's random delay), one a chain step goes before, or one the pools' lines hold.
pub(super) fn settle(
    memory: &mut CappedMemory,
    work: &[TaskWork],
    whole_instance_held: impl Fn(&str, &str) -> bool,
    waits: &[Code],
) {
    for ((instance_id, _), episode) in &mut memory.episodes {
        let Some(round) = episode.round.as_mut() else {
            continue;
        };
        for index in std::mem::take(&mut round.offered) {
            let Some(member) = round.order.get(index) else {
                continue;
            };
            let Some(entry) = work.iter().find(|entry| {
                entry.task_id == member.task_id
                    && entry.instance_id.as_deref() == Some(instance_id.as_str())
            }) else {
                continue;
            };
            if entry.state != SchedulingDecisionState::Deferred
                || whole_instance_held(member.task_id.as_str(), instance_id.as_str())
            {
                continue;
            }
            let Some(reason) = entry.reasons.last() else {
                continue;
            };
            if reason.outcome.is(PolicyCode::PoolLineHeld)
                || reason.outcome.is(PolicyCode::PolicyChainHandoverWaiting)
                || waits.iter().any(|code| reason.outcome.is(*code))
            {
                continue;
            }
            round
                .passed
                .insert(index, reason.outcome.code().registered());
        }
    }
}

/// What the pools decided for one candidate so far: the first capped hold, the first hard-line
/// hold and the first capped slot, each in pool order.
#[derive(Default)]
struct Decided {
    held: Option<DecisionReason>,
    hold: Option<DecisionReason>,
    member: Option<(KnownBenefit, CappedSlot)>,
}

impl Decided {
    fn held(&mut self, reason: DecisionReason) {
        if self.held.is_none() {
            self.held = Some(reason);
        }
    }

    fn normal_hold(&mut self, reason: &DecisionReason) {
        if self.hold.is_none() {
            self.hold = Some(reason.clone());
        }
    }

    fn disposition(self) -> Disposition {
        match (self.held, self.hold, self.member) {
            (Some(held), _, _) => Disposition::Held(held),
            (None, Some(hold), Some(_)) => Disposition::Held(hold),
            (None, None, Some((benefit, slot))) => Disposition::Member {
                benefit,
                slot: Box::new(slot),
            },
            (None, hold, None) => Disposition::Normal { hold },
        }
    }
}

/// One consumer of a pool on an instance: a judged task of the instance that is no manual
/// request and declares a consumption of the pool.
struct Consumer<'a> {
    task: &'a TaskSpec,
}

/// The consumers of `pool` on `instance`, by task id. SCHED-C1f1 (register §1.303, "Who reads
/// it" item 5): a task whose entry reads its amount from a fact declares a consumption whatever
/// the value read (its fallback is at least 1).
fn consumers<'a>(
    context: &PlacementContext<'a>,
    instance: &InstanceSnapshot,
    pool: &PoolSpec,
) -> PolicyEvaluationResult<Vec<Consumer<'a>>> {
    let mut found = Vec::new();
    for (_, task) in context.tasks.values() {
        if !task.is_benefit_judged()
            || !scope_matches_instance(&task.scope, instance)
            || manual_request(task)
        {
            continue;
        }
        if crate::resource_targets::consumption_milli(task, &pool.id, Amounts::Declared)? > 0 {
            found.push(Consumer { task });
        }
    }
    Ok(found)
}

/// SCHED-C1f1 (register §1.303 SC1F-3): each of `consumers`' `⌈C/1000⌉` of `pool_id` as
/// `amounts` reads it (in effect on the instance), by task id.
fn member_amounts(
    consumers: &[Consumer<'_>],
    pool_id: &str,
    amounts: Amounts<'_>,
) -> PolicyEvaluationResult<MemberAmounts> {
    consumers
        .iter()
        .map(|consumer| {
            Ok((
                consumer.task.id.clone(),
                crate::resource_targets::consumption(consumer.task, pool_id, amounts)?,
            ))
        })
        .collect()
}

/// `⌈C/1000⌉`, the whole units a consumption in thousandths reads.
fn units(consumed_milli: u128) -> PolicyEvaluationResult<u64> {
    u64::try_from(consumed_milli.div_ceil(1_000))
        .map_err(|_| PolicyEvaluationError::overflow("a consumption exceeds the integer range"))
}

/// The lines of `pool` on `instance` at the evaluation (module doc), or why they cannot be kept.
/// SCHED-C1e: the instance's own set of the pool first, whole (its trigger line the capacity when
/// it writes none); otherwise the catalog's lines; otherwise the defaults.
fn lines(
    context: &PlacementContext<'_>,
    facts: &EvaluationFacts,
    instance: &InstanceSnapshot,
    pool: &PoolSpec,
) -> PolicyEvaluationResult<Result<Lines, LinesProblem>> {
    let (stop_line, hard_line, trigger_line, source) =
        match context.instance_lines.get(&instance.instance_id, &pool.id) {
            Some(own) => (
                Some(&own.stop_line),
                Some(&own.hard_line),
                own.trigger_line.as_ref(),
                LinesSource::Instance,
            ),
            None if pool.stop_line.is_some() || pool.hard_line.is_some() => (
                pool.stop_line.as_ref(),
                pool.hard_line.as_ref(),
                pool.trigger_line.as_ref(),
                LinesSource::Catalog,
            ),
            None => (
                None,
                None,
                pool.trigger_line.as_ref(),
                LinesSource::Defaults,
            ),
        };
    let written = stop_line.is_some() || hard_line.is_some();
    let unknown = |fact_key: Option<String>, unknown_reason| LinesProblem::Unknown {
        written,
        fact_key,
        unknown_reason,
    };
    let Some(reading) = context.pool_values.get(pool.id.as_str()) else {
        // The capacity's fact first, then the pool's own reading (as its predicates ask).
        if let (PoolCapacity::Fact(reference), Err(reason)) =
            (&pool.capacity, pool_bound(pool, facts, context.time))
        {
            return Ok(Err(unknown(Some(reference.fact_key.clone()), Some(reason))));
        }
        return Ok(Err(match &pool.observation {
            ObservationRef::Fact { fact_key } => unknown(
                Some(fact_key.clone()),
                live_pool_unknown_reason(pool, fact_key, facts, context.time),
            ),
            ObservationRef::Outcome { .. } => unknown(None, None),
        }));
    };
    let Ok(projected) =
        project_pool_value(pool, reading.bound, reading.snapshot, context.time.unix_ms)
    else {
        return Ok(Err(unknown(None, None)));
    };
    let capacity = reading.bound.capacity();
    // The consumption the reading does not show yet: every task dispatched on the instance at or
    // after the reading's observation (the reserve gate's step 3); SCHED-C1f1: each by its own
    // amount in effect on the instance.
    let mut unshown = 0_i128;
    for state in &facts.tasks {
        if state.instance_id != instance.instance_id
            || state
                .last_dispatched_unix_ms
                .is_none_or(|dispatched| dispatched < reading.snapshot.observed_at_unix_ms)
        {
            continue;
        }
        if let Some((_, task)) = context.tasks.get(state.task_id.as_str()) {
            unshown += i128::from(crate::resource_targets::consumption(
                task,
                &pool.id,
                context.amounts.on(&instance.instance_id),
            )?);
        }
    }
    let reading = i128::from(projected) - unshown;
    let (stop, hard) = match (stop_line, hard_line) {
        (None, None) => (i128::from(capacity), 0),
        (Some(stop), Some(hard)) => {
            let stop = match line_value(pool, stop, facts, context) {
                Ok(stop) => stop,
                Err(problem) => return Ok(Err(problem)),
            };
            let hard = match line_value(pool, hard, facts, context) {
                Ok(hard) => hard,
                Err(problem) => return Ok(Err(problem)),
            };
            (stop, hard)
        }
        // The compiler refuses one without the other; read as lines that break their rules.
        _ => (0, 0),
    };
    // SCHED-C1c: the trigger line, read as the other two; the capacity without one.
    let trigger = match trigger_line {
        Some(line) => match line_value(pool, line, facts, context) {
            Ok(trigger) => Some(trigger),
            Err(problem) => return Ok(Err(problem)),
        },
        None => None,
    };
    let begins = trigger.unwrap_or(i128::from(capacity));
    if !(0..stop).contains(&hard) || stop > begins || begins > i128::from(capacity) {
        return Ok(Err(LinesProblem::Invalid {
            capacity,
            stop,
            hard,
            trigger,
            source,
        }));
    }
    Ok(Ok(Lines {
        capacity,
        reading,
        stop: u64::try_from(stop).map_err(|_| PolicyEvaluationError::overflow("a stop line"))?,
        hard: u64::try_from(hard).map_err(|_| PolicyEvaluationError::overflow("a hard line"))?,
        trigger_line: trigger
            .map(u64::try_from)
            .transpose()
            .map_err(|_| PolicyEvaluationError::overflow("a trigger line"))?,
        source,
    }))
}

/// One line's value: a constant itself; a reference the integer its fact of the pool's scope
/// holds plus the offset, read as a capacity fact is (at least the pool's minimum confidence, not
/// past its expiry), or, when the fact gives none, [`LinesProblem::Unknown`] of the written lines
/// with the fact's key and why.
fn line_value(
    pool: &PoolSpec,
    line: &PoolLine,
    facts: &EvaluationFacts,
    context: &PlacementContext<'_>,
) -> Result<i128, LinesProblem> {
    let reference = match line {
        PoolLine::Constant(value) => return Ok(i128::from(*value)),
        PoolLine::Fact(reference) => reference,
    };
    let missing = |reason| LinesProblem::Unknown {
        written: true,
        fact_key: Some(reference.fact_key.clone()),
        unknown_reason: Some(reason),
    };
    let fact = facts
        .facts
        .iter()
        .find(|fact| fact.scope == pool.scope && fact.fact_key == reference.fact_key)
        .ok_or_else(|| missing(UnknownReason::FactMissing))?;
    let minimum_confidence = match pool.value_source {
        crate::PoolValueSource::LedgerFact {
            minimum_confidence_milli,
        } => minimum_confidence_milli.max(1),
        crate::PoolValueSource::StaticSnapshot => 1,
    };
    if fact.confidence_milli < minimum_confidence {
        return Err(missing(UnknownReason::FactLowConfidence));
    }
    if fact
        .expires_at_unix_ms
        .is_some_and(|expiry| context.time.unix_ms > expiry)
    {
        return Err(missing(UnknownReason::FactExpired));
    }
    let FactValue::Integer(value) = fact.value else {
        return Err(missing(UnknownReason::TypeMismatch));
    };
    Ok(i128::from(value) + i128::from(reference.offset))
}

/// The capped round of a pair at this evaluation: the round, with its plan now and what is left
/// of it.
struct RoundView<'a> {
    episode: u64,
    round: u64,
    /// The round; `None` while the episode waits for its next one.
    state: Option<&'a Round>,
    /// The members the round dispatched.
    dispatched: BTreeSet<usize>,
    /// The members the plan now skips, with what the pool would hold after each.
    skipped: Vec<(usize, i128)>,
    /// The planned members the round has not dispatched, in plan order.
    remaining: Vec<usize>,
}

/// The candidates of one pair: by task id, their index among the ranked candidates, their
/// judgement and their term of the pool.
type PairCandidates<'a, 'b> = BTreeMap<&'b str, (usize, &'b Known<'a>, &'b ConsumedTerm)>;

/// The transitions of one pair's episode at the evaluation (module doc): `None` when the pool is
/// not in a capped round now (below the cap, or the episode ended), else the round to decide by.
/// SCHED-C1f1 (register §1.303 SC1F-3): the round's plan and the head's check read each member's
/// consumption in effect (`in_effect`); its frozen order stays.
#[allow(clippy::too_many_arguments)]
fn step<'m>(
    context: &PlacementContext<'_>,
    memory: &'m mut CappedMemory,
    key: &(String, String),
    (lines, in_effect): (Lines, &MemberAmounts),
    candidates: &PairCandidates<'_, '_>,
    work: &mut [TaskWork],
    consumers: &[Consumer<'_>],
    instance: &InstanceSnapshot,
) -> PolicyEvaluationResult<Option<RoundView<'m>>> {
    // SCHED-C1d: a generation that changed the pool's consumers ended the episode already
    // ([`plan`]); here only its own rule ends it.
    if memory.episodes.contains_key(key) && lines.spent(lines.reading) {
        memory.episodes.remove(key);
    }
    if !memory.episodes.contains_key(key) {
        // SCHED-C1c: an episode begins at the trigger line, the cap without one.
        if !lines.at_trigger() {
            return Ok(None);
        }
        let consumers_key = consumer_keys(consumers, &key.1)?;
        let begun = memory.begun.entry(key.clone()).or_insert(0);
        *begun += 1;
        let number = *begun;
        memory.episodes.insert(
            key.clone(),
            Episode {
                consumers: consumers_key,
                number,
                rounds: 0,
                round: None,
            },
        );
    }
    let now = context.time.unix_ms;
    let dispatched_in = |task_id: &str, since: u64| {
        context
            .last_results
            .get(&(task_id, instance.instance_id.as_str()))
            .and_then(|state| state.last_dispatched_unix_ms)
            .is_some_and(|dispatched| dispatched >= since)
    };
    // The members a round dispatched.
    let dispatched_of = |round: &Round| {
        round
            .order
            .iter()
            .enumerate()
            .filter(|(_, member)| dispatched_in(&member.task_id, round.started_at_unix_ms))
            .map(|(index, _)| index)
            .collect::<BTreeSet<_>>()
    };
    let mut started = false;
    loop {
        let Some(episode) = memory.episodes.get_mut(key) else {
            return Ok(None);
        };
        if episode.round.is_none() {
            let number = episode.rounds + 1;
            match start_round(candidates, (lines, in_effect), number, now)? {
                Some(round) => {
                    episode.rounds = number;
                    episode.round = Some(round);
                    started = true;
                }
                // No consumer is a candidate now (each in its cooldown, running, or held by its
                // own conditions): nothing could be dispatched in any case, so the episode waits
                // for its next round, and the consumers say why they are not taken.
                None if candidates.is_empty() => {
                    return Ok(Some(RoundView {
                        episode: episode.number,
                        round: episode.rounds,
                        state: None,
                        dispatched: BTreeSet::new(),
                        skipped: Vec::new(),
                        remaining: Vec::new(),
                    }));
                }
                None => {
                    // Nothing listed or planned: the episode ends.
                    let reason = round_empty_reason(&key.1, episode.number, number);
                    memory.episodes.remove(key);
                    for consumer in consumers {
                        if let Some(entry) = work_entry(work, &consumer.task.id, instance) {
                            // SCHED-C1f1 (SC1F-4): with the consumer's consumption of the pool
                            // when it is read from a fact.
                            entry.reasons.push(with_values(
                                reason.clone(),
                                context.amounts.values(
                                    &consumer.task.id,
                                    &instance.instance_id,
                                    &key.1,
                                ),
                            ));
                        }
                    }
                    return Ok(None);
                }
            }
        }
        let Some(round) = episode.round.as_ref() else {
            return Ok(None);
        };
        if started {
            break;
        }
        let dispatched = dispatched_of(round);
        let remaining = round
            .plan(lines, (&dispatched, &BTreeSet::new()), in_effect)
            .0
            .into_iter()
            .filter(|index| !dispatched.contains(index))
            .collect::<Vec<_>>();
        // Every planned task ran or was passed: the round is over.
        if remaining.is_empty() {
            episode.round = None;
            continue;
        }
        // The head: the first planned one not dispatched that is a candidate now. Before the
        // dispatch it must not end below the hard line, or the round ends. SS-2h fix 1 (c): when no
        // planned one left is a candidate now while a consumer is, none of the round can be
        // dispatched now: the round ends too (the ones left say why, `capped_not_dispatchable`).
        // While no consumer is a candidate at all, the round waits.
        let ends = match remaining
            .iter()
            .find(|index| candidates.contains_key(round.order[**index].task_id.as_str()))
        {
            // SCHED-C1f1: by the head's consumption in effect, the amount its run is judged by.
            Some(head) => {
                lines.reading - i128::from(round.order[*head].in_effect(in_effect))
                    < i128::from(lines.hard)
            }
            None => !candidates.is_empty(),
        };
        if !ends {
            break;
        }
        episode.round = None;
    }
    let Some(episode) = memory.episodes.get(key) else {
        return Ok(None);
    };
    let Some(round) = episode.round.as_ref() else {
        return Ok(None);
    };
    // A round begun at this evaluation has dispatched nothing yet.
    let dispatched = if started {
        BTreeSet::new()
    } else {
        dispatched_of(round)
    };
    let (planned, skipped) = round.plan(lines, (&dispatched, &BTreeSet::new()), in_effect);
    let remaining = planned
        .into_iter()
        .filter(|index| !dispatched.contains(index))
        .collect();
    Ok(Some(RoundView {
        episode: episode.number,
        round: episode.rounds,
        state: Some(round),
        dispatched,
        skipped,
        remaining,
    }))
}

/// A new round (module doc): its frozen order of the listed candidates and its plan from the
/// reading; `None` when it lists or plans nothing. SCHED-C1f1: each listed by its consumption in
/// effect at the round's start, as its judgement read it, which its member keeps.
fn start_round(
    candidates: &PairCandidates<'_, '_>,
    (lines, in_effect): (Lines, &MemberAmounts),
    number: u64,
    now: u64,
) -> PolicyEvaluationResult<Option<Round>> {
    let overflow = || PolicyEvaluationError::overflow("a capped spending order overflowed");
    // Listed: its value without the pool's cost is above that value's rounding bound; from the
    // second round, not limited. Never by its cost. SCHED-C1a (register §1.299): for a task with
    // a fixed value `V` the benefit gate's `S` starts at `V`, so this value is `V` less the cost
    // of the other pools it consumes, and its bound their terms alone.
    let mut listed = Vec::new();
    for (task_id, (_, known, term)) in candidates {
        if number > 1 && known.task.is_benefit_limited() {
            continue;
        }
        let value = known.sum.plus(term.cost, false).ok_or_else(overflow)?;
        let bound = known
            .tolerance
            .plus(term.bound, true)
            .ok_or_else(overflow)?;
        let listed_by_value = value.exceeds(&bound).ok_or_else(|| {
            PolicyEvaluationError::invalid(format!(
                "task '{task_id}' has a negative rounding bound without the cost of pool '{}'",
                term.pool_id
            ))
        })?;
        if !listed_by_value {
            continue;
        }
        let (numerator, denominator) = value.non_negative().ok_or_else(overflow)?;
        let per_unit = (
            numerator,
            denominator
                .checked_mul(term.consumed_milli)
                .ok_or_else(overflow)?,
        );
        listed.push((*task_id, per_unit, units(term.consumed_milli)?));
    }
    if listed.is_empty() {
        return Ok(None);
    }
    // Frozen by V ÷ c, the higher first, then by task id.
    listed.sort_by(|left, right| compare_ratios(right.1, left.1).then_with(|| left.0.cmp(right.0)));
    let order = listed
        .iter()
        .map(|(task_id, _, consumed)| Member {
            task_id: (*task_id).to_owned(),
            consumed: *consumed,
        })
        .collect::<Vec<_>>();
    let round = Round {
        started_at_unix_ms: now,
        reading: lines.reading,
        order,
        passed: BTreeMap::new(),
        offered: Vec::new(),
    };
    if round
        .plan(lines, (&BTreeSet::new(), &BTreeSet::new()), in_effect)
        .0
        .is_empty()
    {
        return Ok(None);
    }
    Ok(Some(round))
}

impl RoundView<'_> {
    /// The dispositions of one capped pair's candidates and the reasons of its consumers that are
    /// no candidates (module doc). SCHED-C1f1 (register §1.303 SC1F-3, SC1F-4): the block's fit
    /// and the plan of its fallbacks read each member's consumption in effect (`in_effect`), and
    /// every reason given to a task whose consumption of the pool is read from a fact names it
    /// (`amounts`).
    #[allow(clippy::too_many_arguments)]
    fn decide(
        &self,
        (pool_id, instance): (&str, &InstanceSnapshot),
        (lines, in_effect, amounts): (Lines, &MemberAmounts, &ConsumptionAmounts),
        ranked: &[PlacementCandidate],
        candidates: &PairCandidates<'_, '_>,
        decided: &mut [Decided],
        work: &mut [TaskWork],
        consumers: &[Consumer<'_>],
    ) -> PolicyEvaluationResult<Vec<usize>> {
        let overflow = || PolicyEvaluationError::overflow("a capped head's benefit overflowed");
        let for_task = |task_id: &str, reason: DecisionReason| {
            with_values(
                reason,
                amounts.values(task_id, &instance.instance_id, pool_id),
            )
        };
        let order: &[Member] = match self.state {
            Some(round) => &round.order,
            None => &[],
        };
        // The block: the head and the planned candidates after it that fit above the hard line, in
        // order. SS-2h fix 1 (b): then its fallbacks, each tried only once every candidate of the
        // block before it is refused: what the round would plan with those passed, taken the same
        // way, again while one is added. With each `true` when it is a fallback.
        let mut block = Vec::<(usize, bool)>::new();
        if let Some(round) = self.state {
            let mut group = self.remaining.clone();
            let mut assumed = BTreeSet::new();
            let mut fallback = false;
            'groups: loop {
                let mut added = false;
                for index in group {
                    let member = &order[index];
                    if !candidates.contains_key(member.task_id.as_str()) {
                        continue;
                    }
                    // As the head, it would end the round.
                    if lines.reading - i128::from(member.in_effect(in_effect))
                        < i128::from(lines.hard)
                    {
                        break 'groups;
                    }
                    block.push((index, fallback));
                    added = true;
                }
                if !added {
                    break;
                }
                assumed.extend(block.iter().map(|(index, _)| *index));
                fallback = true;
                group = round
                    .plan(lines, (&self.dispatched, &assumed), in_effect)
                    .0
                    .into_iter()
                    .filter(|index| !self.dispatched.contains(index))
                    .collect();
            }
        }
        let head = block
            .first()
            .map(|(index, _)| order[*index].task_id.as_str());
        for (&task_id, (index, known, term)) in candidates {
            let at = order.iter().position(|member| member.task_id == task_id);
            // Its hold when the round does not try it now.
            let held = match at {
                Some(at) => Some(self.member_hold(at, pool_id, lines, head)),
                None if self.round > 1 && known.task.is_benefit_limited() => {
                    Some(self.reason(PoolLineHold::CappedLimitedSpent, pool_id, lines, None, None))
                }
                None => None,
            }
            .map(|held| for_task(task_id, held));
            let place = at.and_then(|at| block.iter().position(|(member, _)| *member == at));
            if let Some(position) = place {
                // B = V − W·(c − a), a = min(c, max(0, r − S)): only the part below the stop line
                // costs.
                let above = (lines.reading - i128::from(lines.stop)).max(0);
                let free = u128::try_from(above)
                    .ok()
                    .and_then(|above| above.checked_mul(1_000))
                    .ok_or_else(overflow)?
                    .min(term.consumed_milli);
                let refund = ratio(
                    &[free, u128::from(term.weight_milli)],
                    &[u128::from(term.scale), 1_000],
                )
                .ok_or_else(overflow)?;
                let benefit = known.sum.plus(refund, false).ok_or_else(overflow)?;
                let rank = at.map_or(0, |at| at + 1);
                // The first pool that tries it keeps it (pools in id order).
                if decided[*index].member.is_none() {
                    decided[*index].member = Some((
                        KnownBenefit {
                            benefit_milli: saturating_milli(benefit.floor()),
                            expected_duration_ms: expected_duration_ms(known.candidate),
                            // SCHED-C1a: the fixed value its `S` starts from, when it has one.
                            fixed_value_milli: known.task.benefit_fixed_value_milli,
                        },
                        CappedSlot {
                            pool_id: pool_id.to_owned(),
                            position,
                            spend: for_task(
                                task_id,
                                spend_reason(pool_id, lines, self.episode, self.round, rank),
                            ),
                            lead: None,
                            wait: held.filter(|_| block[position].1),
                            wake_unix_ms: None,
                        },
                    ));
                }
                continue;
            }
            match held {
                Some(held) => decided[*index].held(held),
                // Listed after the order was frozen, or of no value without the pool's cost:
                // held only if it would pass.
                None => decided[*index].normal_hold(&for_task(
                    task_id,
                    self.reason(PoolLineHold::CappedNotHead, pool_id, lines, None, head),
                )),
            }
        }
        // The consumers that are no candidates of the gate (nor riders of a chain, nor of an
        // unknown benefit, whose own reasons say why): why the round does not take them.
        for consumer in consumers {
            if ranked.iter().any(|candidate| {
                candidate.task_id == consumer.task.id
                    && candidate.instance_id == instance.instance_id
            }) {
                continue;
            }
            let Some(entry) = work_entry(work, &consumer.task.id, instance) else {
                continue;
            };
            let gate = entry
                .reasons
                .last()
                .and_then(|reason| reason.outcome.code().registered());
            entry.reasons.push(for_task(
                &consumer.task.id,
                self.not_dispatchable(pool_id, lines, gate),
            ));
        }
        Ok(block.into_iter().map(|(index, _)| index).collect())
    }

    /// The hold of the order's member `at` that the round does not try now: passed for the round
    /// (SS-2h fix 1 (b): `capped_not_dispatchable`, `gate_code` its recorded refusal), skipped by
    /// the plan (`capped_would_cross_hard_line`), or waiting for the head (`capped_not_head`).
    fn member_hold(
        &self,
        at: usize,
        pool_id: &str,
        lines: Lines,
        head: Option<&str>,
    ) -> DecisionReason {
        if let Some(gate) = self.state.and_then(|round| round.passed.get(&at)) {
            return self.not_dispatchable(pool_id, lines, *gate);
        }
        match self.skipped.iter().find(|(skipped, _)| *skipped == at) {
            Some((_, after)) => self.reason(
                PoolLineHold::CappedWouldCrossHardLine,
                pool_id,
                lines,
                Some(*after),
                None,
            ),
            None => self.reason(PoolLineHold::CappedNotHead, pool_id, lines, None, head),
        }
    }

    /// `pool_line_held` `capped_not_dispatchable` of this round, with `gate_code` the gate that
    /// keeps it when registered.
    fn not_dispatchable(&self, pool_id: &str, lines: Lines, gate: Option<Code>) -> DecisionReason {
        let mut reason = self.reason(
            PoolLineHold::CappedNotDispatchable,
            pool_id,
            lines,
            None,
            None,
        );
        if let Some(gate) = gate {
            reason.outcome = reason.outcome.gate_code(gate);
            reason
                .detail
                .push_str(&format!(" gate_code={}", gate.as_str()));
        }
        reason
    }

    /// `pool_line_held` of this round.
    fn reason(
        &self,
        hold: PoolLineHold,
        pool_id: &str,
        lines: Lines,
        after: Option<i128>,
        head: Option<&str>,
    ) -> DecisionReason {
        let mut outcome = Outcome::new(PolicyCode::PoolLineHeld)
            .pool_line_hold(hold)
            .pool_id(pool_id)
            .pool_reading(record_integer(lines.reading))
            .pool_capacity(lines.capacity)
            .stop_line(lines.stop)
            .hard_line(lines.hard)
            .capped_episode(self.episode)
            .capped_round(self.round);
        let mut detail = format!(
            "pool_line_hold={} pool_id={pool_id} pool_reading={} pool_capacity={} stop_line={} \
             hard_line={} capped_episode={} capped_round={}",
            hold.as_str(),
            lines.reading,
            lines.capacity,
            lines.stop,
            lines.hard,
            self.episode,
            self.round
        );
        if let Some(after) = after {
            outcome = outcome.amount_after(record_integer(after));
            detail.push_str(&format!(" amount_after={after}"));
        }
        if let Some(head) = head {
            outcome = outcome.task_id(head);
            detail.push_str(&format!(" task_id={head}"));
        }
        // SCHED-C1e: where the lines come from.
        outcome = outcome.lines_source(lines.source);
        detail.push_str(&format!(" lines_source={}", lines.source.as_str()));
        reason_with(outcome, detail)
    }
}

/// The work entry of `task_id` on `instance`.
fn work_entry<'w>(
    work: &'w mut [TaskWork],
    task_id: &str,
    instance: &InstanceSnapshot,
) -> Option<&'w mut TaskWork> {
    work.iter_mut().find(|entry| {
        entry.task_id == task_id
            && entry.instance_id.as_deref() == Some(instance.instance_id.as_str())
    })
}

/// An `i128` amount as a record integer, saturated at the `i64` range.
fn record_integer(value: i128) -> i64 {
    i64::try_from(value).unwrap_or(if value < 0 { i64::MIN } else { i64::MAX })
}

/// `capped_spend`: the head's dispatch.
fn spend_reason(
    pool_id: &str,
    lines: Lines,
    episode: u64,
    round: u64,
    rank: usize,
) -> DecisionReason {
    let mut outcome = Outcome::new(PolicyCode::CappedSpend)
        .pool_id(pool_id)
        .pool_capacity(lines.capacity)
        .stop_line(lines.stop)
        .hard_line(lines.hard)
        .capped_episode(episode)
        .capped_round(round)
        .capped_rank(u64::try_from(rank).unwrap_or(u64::MAX))
        .pool_reading(record_integer(lines.reading));
    let mut detail = format!(
        "pool_id={pool_id} pool_capacity={} stop_line={} hard_line={} capped_episode={episode} \
         capped_round={round} capped_rank={rank} pool_reading={}",
        lines.capacity, lines.stop, lines.hard, lines.reading
    );
    // SCHED-C1c: the trigger line, when the pool writes one.
    if let Some(trigger) = lines.trigger_line {
        outcome = outcome.trigger_line(trigger);
        detail.push_str(&format!(" trigger_line={trigger}"));
    }
    // SCHED-C1e: where the lines come from.
    outcome = outcome.lines_source(lines.source);
    detail.push_str(&format!(" lines_source={}", lines.source.as_str()));
    reason_with(outcome, detail)
}

/// `pool_line_held` `capped_round_empty`.
fn round_empty_reason(pool_id: &str, episode: u64, round: u64) -> DecisionReason {
    let outcome = Outcome::new(PolicyCode::PoolLineHeld)
        .pool_line_hold(PoolLineHold::CappedRoundEmpty)
        .pool_id(pool_id)
        .capped_episode(episode)
        .capped_round(round);
    reason_with(
        outcome,
        format!(
            "pool_line_hold=capped_round_empty pool_id={pool_id} capped_episode={episode} \
             capped_round={round}"
        ),
    )
}

/// `pool_line_held` `would_cross_hard_line`, below the cap; SCHED-C1e: with `lines_source`.
fn hard_line_reason(pool_id: &str, lines: Lines, after: i128) -> DecisionReason {
    let hold = PoolLineHold::WouldCrossHardLine;
    let outcome = Outcome::new(PolicyCode::PoolLineHeld)
        .pool_line_hold(hold)
        .pool_id(pool_id)
        .pool_reading(record_integer(lines.reading))
        .pool_capacity(lines.capacity)
        .stop_line(lines.stop)
        .hard_line(lines.hard)
        .amount_after(record_integer(after))
        .lines_source(lines.source);
    reason_with(
        outcome,
        format!(
            "pool_line_hold={} pool_id={pool_id} pool_reading={} pool_capacity={} stop_line={} \
             hard_line={} amount_after={after} lines_source={}",
            hold.as_str(),
            lines.reading,
            lines.capacity,
            lines.stop,
            lines.hard,
            lines.source.as_str()
        ),
    )
}

/// `pool_line_held` `capped_cap_unknown`.
fn unknown_lines_reason(
    pool_id: &str,
    fact_key: Option<&str>,
    unknown_reason: Option<UnknownReason>,
) -> DecisionReason {
    let mut outcome = Outcome::new(PolicyCode::PoolLineHeld)
        .pool_line_hold(PoolLineHold::CappedCapUnknown)
        .pool_id(pool_id);
    let mut detail = format!("pool_line_hold=capped_cap_unknown pool_id={pool_id}");
    if let Some(fact_key) = fact_key {
        outcome = outcome.fact_key(fact_key);
        detail.push_str(&format!(" fact_key={fact_key}"));
    }
    if let Some(unknown_reason) = unknown_reason {
        outcome = outcome.unknown_reason(UnknownReasonToken(unknown_reason));
        detail.push_str(&format!(
            " unknown_reason={}",
            unknown_token(unknown_reason)
        ));
    }
    reason_with(outcome, detail)
}

/// `pool_line_held` `lines_invalid` of the lines read `(stop, hard, trigger)`; SCHED-C1c: with
/// `trigger_line` when the pool writes one; SCHED-C1e: with `lines_source`.
fn invalid_lines_reason(
    pool_id: &str,
    capacity: u64,
    (stop, hard, trigger): (i128, i128, Option<i128>),
    source: LinesSource,
) -> DecisionReason {
    let mut outcome = Outcome::new(PolicyCode::PoolLineHeld)
        .pool_line_hold(PoolLineHold::LinesInvalid)
        .pool_id(pool_id)
        .pool_capacity(capacity)
        .stop_line(record_integer(stop))
        .hard_line(record_integer(hard));
    let mut detail = format!(
        "pool_line_hold=lines_invalid pool_id={pool_id} pool_capacity={capacity} \
         stop_line={stop} hard_line={hard}"
    );
    if let Some(trigger) = trigger {
        outcome = outcome.trigger_line(record_integer(trigger));
        detail.push_str(&format!(" trigger_line={trigger}"));
    }
    outcome = outcome.lines_source(source);
    detail.push_str(&format!(" lines_source={}", source.as_str()));
    reason_with(outcome, detail)
}

/// SCHED-C1c (register §1.299): `pool_lines_unspendable`, the warning of a pool whose lines no
/// consumer's run fits under from the cap (`least` the least consumption among its consumers).
fn unspendable_reason(pool_id: &str, lines: Lines, least: u64) -> DecisionReason {
    let outcome = Outcome::new(PolicyCode::PoolLinesUnspendable)
        .pool_id(pool_id)
        .pool_capacity(lines.capacity)
        .stop_line(lines.stop)
        .hard_line(lines.hard)
        .least_consumption(least)
        // SCHED-C1e: where the lines come from.
        .lines_source(lines.source);
    reason_with(
        outcome,
        format!(
            "pool_id={pool_id} pool_capacity={} stop_line={} hard_line={} \
             least_consumption={least} lines_source={}",
            lines.capacity,
            lines.stop,
            lines.hard,
            lines.source.as_str()
        ),
    )
}

/// SCHED-C1c: the least `⌈C/1000⌉` of one run among a pool's `consumers` (candidate or not), with
/// the consumer that gives it, the first by task id among equal ones; `None` without a consumer.
/// SCHED-C1f1 (register §1.303, "Who reads it" item 3): each by its consumption in effect on the
/// instance (`in_effect`).
fn least_consumption<'a>(
    consumers: &'a [Consumer<'_>],
    in_effect: &MemberAmounts,
) -> Option<(u64, &'a str)> {
    let mut least = None::<(u64, &str)>;
    for consumer in consumers {
        let consumed = in_effect.get(&consumer.task.id).copied().unwrap_or(0);
        if least.is_none_or(|(known, _)| consumed < known) {
            least = Some((consumed, consumer.task.id.as_str()));
        }
    }
    least
}

/// SS-2h: `capped_not_head` for a block candidate behind the block's dispatched one (`head`).
pub(super) fn not_head_reason(pool_id: &str, head: &str) -> DecisionReason {
    let outcome = Outcome::new(PolicyCode::PoolLineHeld)
        .pool_line_hold(PoolLineHold::CappedNotHead)
        .pool_id(pool_id)
        .task_id(head);
    reason_with(
        outcome,
        format!("pool_line_hold=capped_not_head pool_id={pool_id} task_id={head}"),
    )
}
