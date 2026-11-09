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
//! - **The capped state.** At `r ≥ cap` (at the start too) the pool enters a capped episode on
//!   the instance; it ends once `r ≤ S` and `r < cap`. A round lists the judged consumers of the
//!   pool that are candidates now, whose value without the pool's cost `V` is above its rounding
//!   bound (never by their cost), and, from the second round on, only those not marked
//!   `benefit_limited`. It freezes them by `V ÷ c`, the higher first, then by task id, and plans
//!   from `r`: each one in turn while the accumulated reading is above `S` (or at the cap), one
//!   whose run would end below `H` skipped. Before each dispatch the actual `r` is read again: the
//!   head (the first planned one not dispatched in the round that is a candidate now) must not end
//!   below `H`, or the round ends. A round whose planned tasks were all dispatched ends; the next
//!   one starts while `r` is above `S`. A round that lists or plans nothing though a consumer is a
//!   candidate ends the episode (`capped_round_empty`); while no consumer is a candidate at all the
//!   episode waits for its next round.
//! - **Dispatch (layout 乙).** The head and the planned candidates after it that fit above `H`
//!   are the pool's only consumers in the allocation, one block in the order at the head's place
//!   (the head's tier and `B ÷ E`, `B = V − W·(c − a)`, `a = min(c, max(0, r − S))`); the first of
//!   them that every later gate admits is dispatched (`capped_spend`), the others wait
//!   (`capped_not_head`). Every other gate stays.
//! - **Memory only** ([`CappedMemory`]): the episodes, their round numbers and frozen orders, as
//!   SS-4's `NoProgressMemory` keeps its guards. A start, or a new catalog generation, begins
//!   with none. Which tasks a round dispatched is read from the task snapshots
//!   (`last_dispatched_unix_ms` at or after the round's start).
//!
//! The lines constrain the scheduler only: a manual request, a chain successor that rides with its
//! chain and the Runtime's own tasks are no judged candidates and are not held.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::outcome::Outcome;
use actingcommand_selection_policy::UnknownReason;

use super::benefit::{
    ConsumedTerm, ExactSum, KnownBenefit, compare_ratios, expected_duration_ms, manual_request,
    ratio, saturating_milli,
};
use super::tie_break::TieKey;
use super::{
    DecisionReason, EvaluationFacts, InstanceSnapshot, PlacementCandidate, PlacementContext,
    PolicyEvaluationError, PolicyEvaluationResult, TaskWork, UnknownReasonToken,
    live_pool_unknown_reason, pool_bound, project_pool_value, reason_with, scope_matches_instance,
    unknown_token,
};
use crate::codes::{PolicyCode, PoolLineHold};
use crate::{FactValue, ObservationRef, PoolCapacity, PoolLine, PoolSpec, TaskSpec};

/// SS-2h: the Runtime's memory of the capped episodes, per (instance alias, pool id). Memory
/// only: nothing here enters a snapshot, a decision identity, a ledger record or a state file,
/// and a start begins with none. The Runtime passes it to [`super::evaluate_with_capped_spending`]
/// at every evaluation of a cycle; every other entry point evaluates with an empty one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CappedMemory {
    episodes: BTreeMap<(String, String), Episode>,
    /// How many episodes each pair began since the start.
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

/// One capped episode of a pool on an instance.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Episode {
    /// The catalog generation it began under: another one ends it.
    catalog_hash: String,
    number: u64,
    /// How many rounds it began.
    rounds: u64,
    round: Option<Round>,
}

/// One round: its frozen order and its plan.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Round {
    started_at_unix_ms: u64,
    order: Vec<Member>,
    /// Indices into `order`, in plan order.
    planned: Vec<usize>,
    /// Indices into `order` the plan skipped, with what the pool would have held after them.
    skipped: Vec<(usize, i128)>,
}

/// One task of a round's frozen order.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Member {
    task_id: String,
    /// `⌈C/1000⌉`, the consumption the reading is planned with.
    consumed: u64,
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
        slot: CappedSlot,
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
}

impl Lines {
    /// Whether the pool is at or above its cap: an episode begins.
    fn at_cap(&self) -> bool {
        self.reading >= i128::from(self.capacity)
    }

    /// Whether `reading` is spent down: at or below the stop line and below the cap.
    fn spent(&self, reading: i128) -> bool {
        reading <= i128::from(self.stop) && reading < i128::from(self.capacity)
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
    /// The lines read break `0 ≤ H < S ≤ cap`.
    Invalid {
        capacity: u64,
        stop: i128,
        hard: i128,
    },
}

/// The pools' lines and the capped spending order over the benefit gate's judged candidates
/// (module doc). `ranked` are the gate's candidates and `known` their judgements, aligned; the
/// answer is aligned with them. Reasons for the pools' consumers that are no candidates are added
/// to `work`; `memory` is updated under `catalog_hash`.
pub(super) fn plan(
    context: &PlacementContext<'_>,
    facts: &EvaluationFacts,
    ranked: &[PlacementCandidate],
    known: &[Option<Known<'_>>],
    work: &mut [TaskWork],
    memory: &mut CappedMemory,
    catalog_hash: &str,
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
            let lines = match lines(context, facts, instance, pool)? {
                Ok(lines) => lines,
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
                        } => Some(invalid_lines_reason(&pool.id, capacity, stop, hard)),
                    };
                    if let Some(hold) = hold {
                        for (index, _, _) in candidates.values() {
                            decided[*index].normal_hold(&hold);
                        }
                    }
                    continue;
                }
            };
            let in_round = step(
                context,
                memory,
                &key,
                catalog_hash,
                lines,
                &candidates,
                work,
                &consumers,
                instance,
            )?;
            let Some(view) = in_round else {
                // Below the cap, or the episode ended: the hard line holds a run that would cross
                // it.
                for (index, _, term) in candidates.values() {
                    let after = lines.reading - i128::from(units(term.consumed_milli)?);
                    if after < i128::from(lines.hard) {
                        decided[*index].normal_hold(&hard_line_reason(&pool.id, lines, after));
                    }
                }
                continue;
            };
            view.decide(
                (&pool.id, instance),
                lines,
                ranked,
                &candidates,
                &mut decided,
                work,
                &consumers,
            )?;
        }
    }
    memory.episodes.retain(|key, _| considered.contains(key));
    Ok(decided.into_iter().map(Decided::disposition).collect())
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
            (None, None, Some((benefit, slot))) => Disposition::Member { benefit, slot },
            (None, hold, None) => Disposition::Normal { hold },
        }
    }
}

/// One consumer of a pool on an instance: a judged task of the instance that is no manual
/// request and declares a consumption of the pool.
struct Consumer<'a> {
    task: &'a TaskSpec,
}

/// The consumers of `pool` on `instance`, by task id.
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
        if crate::resource_targets::declared_consumption_milli(task, &pool.id)? > 0 {
            found.push(Consumer { task });
        }
    }
    Ok(found)
}

/// `⌈C/1000⌉`, the whole units a consumption in thousandths reads.
fn units(consumed_milli: u128) -> PolicyEvaluationResult<u64> {
    u64::try_from(consumed_milli.div_ceil(1_000))
        .map_err(|_| PolicyEvaluationError::overflow("a consumption exceeds the integer range"))
}

/// The lines of `pool` on `instance` at the evaluation (module doc), or why they cannot be kept.
fn lines(
    context: &PlacementContext<'_>,
    facts: &EvaluationFacts,
    instance: &InstanceSnapshot,
    pool: &PoolSpec,
) -> PolicyEvaluationResult<Result<Lines, LinesProblem>> {
    let written = pool.stop_line.is_some() || pool.hard_line.is_some();
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
    // after the reading's observation (the reserve gate's step 3).
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
            unshown += i128::from(crate::resource_targets::declared_consumption(
                task, &pool.id,
            )?);
        }
    }
    let reading = i128::from(projected) - unshown;
    let (stop, hard) = match (&pool.stop_line, &pool.hard_line) {
        (None, None) => (i128::from(capacity), 0),
        (Some(stop), Some(hard)) => {
            let stop = match line_value(pool, stop, facts, context) {
                Ok(stop) => stop,
                Err((fact_key, reason)) => return Ok(Err(unknown(Some(fact_key), Some(reason)))),
            };
            let hard = match line_value(pool, hard, facts, context) {
                Ok(hard) => hard,
                Err((fact_key, reason)) => return Ok(Err(unknown(Some(fact_key), Some(reason)))),
            };
            (stop, hard)
        }
        // The compiler refuses one without the other; read as lines that break their rules.
        _ => (0, 0),
    };
    if !(0..stop).contains(&hard) || stop > i128::from(capacity) {
        return Ok(Err(LinesProblem::Invalid {
            capacity,
            stop,
            hard,
        }));
    }
    Ok(Ok(Lines {
        capacity,
        reading,
        stop: u64::try_from(stop).map_err(|_| PolicyEvaluationError::overflow("a stop line"))?,
        hard: u64::try_from(hard).map_err(|_| PolicyEvaluationError::overflow("a hard line"))?,
    }))
}

/// One line's value: a constant itself; a reference the integer its fact of the pool's scope
/// holds plus the offset, read as a capacity fact is (at least the pool's minimum confidence, not
/// past its expiry), or the fact's key and why it gives none.
fn line_value(
    pool: &PoolSpec,
    line: &PoolLine,
    facts: &EvaluationFacts,
    context: &PlacementContext<'_>,
) -> Result<i128, (String, UnknownReason)> {
    let reference = match line {
        PoolLine::Constant(value) => return Ok(i128::from(*value)),
        PoolLine::Fact(reference) => reference,
    };
    let missing = |reason| (reference.fact_key.clone(), reason);
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

/// The capped round of a pair at this evaluation: the round's order and plan, with what is left
/// of it.
struct RoundView<'a> {
    episode: u64,
    round: u64,
    order: &'a [Member],
    skipped: &'a [(usize, i128)],
    /// The planned members the round has not dispatched, in plan order.
    remaining: Vec<usize>,
}

/// The candidates of one pair: by task id, their index among the ranked candidates, their
/// judgement and their term of the pool.
type PairCandidates<'a, 'b> = BTreeMap<&'b str, (usize, &'b Known<'a>, &'b ConsumedTerm)>;

/// The transitions of one pair's episode at the evaluation (module doc): `None` when the pool is
/// not in a capped round now (below the cap, or the episode ended), else the round to decide by.
#[allow(clippy::too_many_arguments)]
fn step<'m>(
    context: &PlacementContext<'_>,
    memory: &'m mut CappedMemory,
    key: &(String, String),
    catalog_hash: &str,
    lines: Lines,
    candidates: &PairCandidates<'_, '_>,
    work: &mut [TaskWork],
    consumers: &[Consumer<'_>],
    instance: &InstanceSnapshot,
) -> PolicyEvaluationResult<Option<RoundView<'m>>> {
    if memory
        .episodes
        .get(key)
        .is_some_and(|episode| episode.catalog_hash != catalog_hash || lines.spent(lines.reading))
    {
        memory.episodes.remove(key);
    }
    if !memory.episodes.contains_key(key) {
        if !lines.at_cap() {
            return Ok(None);
        }
        let begun = memory.begun.entry(key.clone()).or_insert(0);
        *begun += 1;
        let number = *begun;
        memory.episodes.insert(
            key.clone(),
            Episode {
                catalog_hash: catalog_hash.to_owned(),
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
    let mut started = false;
    loop {
        let Some(episode) = memory.episodes.get_mut(key) else {
            return Ok(None);
        };
        if episode.round.is_none() {
            let number = episode.rounds + 1;
            match start_round(candidates, lines, number, now)? {
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
                        order: &[],
                        skipped: &[],
                        remaining: Vec::new(),
                    }));
                }
                None => {
                    // Nothing listed or planned: the episode ends.
                    let reason = round_empty_reason(&key.1, episode.number, number);
                    memory.episodes.remove(key);
                    for consumer in consumers {
                        if let Some(entry) = work_entry(work, &consumer.task.id, instance) {
                            entry.reasons.push(reason.clone());
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
        let remaining = round
            .planned
            .iter()
            .copied()
            .filter(|index| !dispatched_in(&round.order[*index].task_id, round.started_at_unix_ms))
            .collect::<Vec<_>>();
        // Every planned task ran: the round is over.
        if remaining.is_empty() {
            episode.round = None;
            continue;
        }
        // Before the dispatch, the head must not end below the hard line, or the round ends.
        let head = remaining
            .iter()
            .find(|index| candidates.contains_key(round.order[**index].task_id.as_str()));
        if let Some(head) = head
            && lines.reading - i128::from(round.order[*head].consumed) < i128::from(lines.hard)
        {
            episode.round = None;
            continue;
        }
        break;
    }
    let Some(episode) = memory.episodes.get(key) else {
        return Ok(None);
    };
    let Some(round) = episode.round.as_ref() else {
        return Ok(None);
    };
    let remaining = if started {
        round.planned.clone()
    } else {
        round
            .planned
            .iter()
            .copied()
            .filter(|index| !dispatched_in(&round.order[*index].task_id, round.started_at_unix_ms))
            .collect()
    };
    Ok(Some(RoundView {
        episode: episode.number,
        round: episode.rounds,
        order: &round.order,
        skipped: &round.skipped,
        remaining,
    }))
}

/// A new round (module doc): its frozen order of the listed candidates and its plan from the
/// reading; `None` when it lists or plans nothing.
fn start_round(
    candidates: &PairCandidates<'_, '_>,
    lines: Lines,
    number: u64,
    now: u64,
) -> PolicyEvaluationResult<Option<Round>> {
    let overflow = || PolicyEvaluationError::overflow("a capped spending order overflowed");
    // Listed: its value without the pool's cost is above that value's rounding bound; from the
    // second round, not limited. Never by its cost.
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
    let mut planned = Vec::new();
    let mut skipped = Vec::new();
    let mut reading = lines.reading;
    for (index, member) in order.iter().enumerate() {
        if lines.spent(reading) {
            break;
        }
        let after = reading - i128::from(member.consumed);
        if after < i128::from(lines.hard) {
            skipped.push((index, after));
            continue;
        }
        planned.push(index);
        reading = after;
    }
    if planned.is_empty() {
        return Ok(None);
    }
    Ok(Some(Round {
        started_at_unix_ms: now,
        order,
        planned,
        skipped,
    }))
}

impl RoundView<'_> {
    /// The dispositions of one capped pair's candidates and the reasons of its consumers that are
    /// no candidates (module doc).
    #[allow(clippy::too_many_arguments)]
    fn decide(
        &self,
        (pool_id, instance): (&str, &InstanceSnapshot),
        lines: Lines,
        ranked: &[PlacementCandidate],
        candidates: &PairCandidates<'_, '_>,
        decided: &mut [Decided],
        work: &mut [TaskWork],
        consumers: &[Consumer<'_>],
    ) -> PolicyEvaluationResult<()> {
        let overflow = || PolicyEvaluationError::overflow("a capped head's benefit overflowed");
        // The head and the planned candidates after it that fit above the hard line, in order.
        let mut block = Vec::new();
        for index in &self.remaining {
            let member = &self.order[*index];
            if !candidates.contains_key(member.task_id.as_str()) {
                continue;
            }
            if lines.reading - i128::from(member.consumed) < i128::from(lines.hard) {
                break;
            }
            block.push(*index);
        }
        let head = block
            .first()
            .map(|index| self.order[*index].task_id.as_str());
        for (task_id, (index, known, term)) in candidates {
            let at = self
                .order
                .iter()
                .position(|member| member.task_id == *task_id);
            if let Some(position) = block.iter().position(|member| Some(*member) == at) {
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
                        },
                        CappedSlot {
                            pool_id: pool_id.to_owned(),
                            position,
                            spend: spend_reason(pool_id, lines, self.episode, self.round, rank),
                            lead: None,
                        },
                    ));
                }
                continue;
            }
            let held = match at {
                Some(at) => match self.skipped.iter().find(|(skipped, _)| *skipped == at) {
                    Some((_, after)) => Some(self.reason(
                        PoolLineHold::CappedWouldCrossHardLine,
                        pool_id,
                        lines,
                        Some(*after),
                        None,
                    )),
                    None => {
                        Some(self.reason(PoolLineHold::CappedNotHead, pool_id, lines, None, head))
                    }
                },
                None if self.round > 1 && known.task.is_benefit_limited() => {
                    Some(self.reason(PoolLineHold::CappedLimitedSpent, pool_id, lines, None, None))
                }
                // Listed after the order was frozen, or of no value without the pool's cost:
                // held only if it would pass.
                None => {
                    decided[*index].normal_hold(&self.reason(
                        PoolLineHold::CappedNotHead,
                        pool_id,
                        lines,
                        None,
                        head,
                    ));
                    None
                }
            };
            if let Some(held) = held {
                decided[*index].held(held);
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
            entry.reasons.push(reason);
        }
        Ok(())
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
    let outcome = Outcome::new(PolicyCode::CappedSpend)
        .pool_id(pool_id)
        .pool_capacity(lines.capacity)
        .stop_line(lines.stop)
        .hard_line(lines.hard)
        .capped_episode(episode)
        .capped_round(round)
        .capped_rank(u64::try_from(rank).unwrap_or(u64::MAX))
        .pool_reading(record_integer(lines.reading));
    reason_with(
        outcome,
        format!(
            "pool_id={pool_id} pool_capacity={} stop_line={} hard_line={} capped_episode={episode} \
             capped_round={round} capped_rank={rank} pool_reading={}",
            lines.capacity, lines.stop, lines.hard, lines.reading
        ),
    )
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

/// `pool_line_held` `would_cross_hard_line`, below the cap.
fn hard_line_reason(pool_id: &str, lines: Lines, after: i128) -> DecisionReason {
    let hold = PoolLineHold::WouldCrossHardLine;
    let outcome = Outcome::new(PolicyCode::PoolLineHeld)
        .pool_line_hold(hold)
        .pool_id(pool_id)
        .pool_reading(record_integer(lines.reading))
        .pool_capacity(lines.capacity)
        .stop_line(lines.stop)
        .hard_line(lines.hard)
        .amount_after(record_integer(after));
    reason_with(
        outcome,
        format!(
            "pool_line_hold={} pool_id={pool_id} pool_reading={} pool_capacity={} stop_line={} \
             hard_line={} amount_after={after}",
            hold.as_str(),
            lines.reading,
            lines.capacity,
            lines.stop,
            lines.hard
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

/// `pool_line_held` `lines_invalid`.
fn invalid_lines_reason(pool_id: &str, capacity: u64, stop: i128, hard: i128) -> DecisionReason {
    let outcome = Outcome::new(PolicyCode::PoolLineHeld)
        .pool_line_hold(PoolLineHold::LinesInvalid)
        .pool_id(pool_id)
        .pool_capacity(capacity)
        .stop_line(record_integer(stop))
        .hard_line(record_integer(hard));
    reason_with(
        outcome,
        format!(
            "pool_line_hold=lines_invalid pool_id={pool_id} pool_capacity={capacity} \
             stop_line={stop} hard_line={hard}"
        ),
    )
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
