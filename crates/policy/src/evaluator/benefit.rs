// SPDX-License-Identifier: AGPL-3.0-only

//! SS-2b (register §1.225; Alice's ruling 104 point 3, approved 2026-10-30 with 104, held for the
//! research check of #386 comment 6523148251; ruling 116 of 2026-10-30 23:2x–23:3x): the net
//! benefit of a judged candidate within its tier.
//!
//! - **Who is judged.** A candidate whose task the catalog marks `benefit_judged` (the
//!   standard-pack composer marks every rule whose package is not maintenance-role), unless it
//!   continues a business chain or is a manual request ([`manual_request`]). SS-2e (register
//!   §1.229, design `rd-1029/design-sch-state.md` §4.10): a candidate in a chain head's run
//!   window (SS-2, register §1.222 (2)) continues a business chain when at least one link walked
//!   from it up to that head, the head included, is marked; it then rides with that chain. One
//!   whose walked links are all unmarked (a business task after a maintenance task such as a
//!   return home) is a business decision of its own and is judged, like a candidate in no chain
//!   window; it still ranks with the head SS-2 finds. Every other candidate ranks as before.
//! - **The net benefit** `B(k) = Σ_p produce(k,p)·W(k,p)/Q_p − Σ_p consume(k,p)·P(k,p)/Q_p`, in
//!   milli, summed exactly and floored once. `produce` is the resource layer's own `r(k,p)` and
//!   `W(k,p)` its effective weight, the base weight plus a covering goal's shortfall weight;
//!   `consume` is the task's declared consumption, as the reserve gate reads it; `Q_p` is the
//!   pool's `valuation.scale`. `P(k,p)`, the cost of consuming pool `p`, is `W(k,p)`, discounted
//!   for a regenerating pool linearly between the task's reserve line on it and its capacity:
//!   `P = W·clamp((cap − cur)/(cap − reserve), 0, 1)`, 0 at capacity, `W` at the reserve.
//! - **Unknown is never 0.** No declared effect, a pending entry, a pending or missing valuation
//!   of a pool it touches, or a reading `P` needs that is unknown or expired makes `B` unknown:
//!   the candidate is not dispatched (`benefit_unknown`), and the known part is never added up.
//!   An unknown reading also asks the fact refresh for it. A known `B ≤ 0` is not dispatched
//!   either (`policy_benefit_nonpositive`).
//! - **The one-way rule.** Nothing here writes a pool weight: the weights are read from the
//!   resource layer's own term, which reads no task state.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use actingcommand_contract::outcome::Outcome;
use actingcommand_selection_policy::UnknownReason;

use super::fact_refresh::{self, AffectedCandidate};
use super::{
    DecisionReason, DetectionSuggestion, EvaluationFacts, InstanceSnapshot, PlacementCandidate,
    PlacementContext, PolicyEvaluationError, PolicyEvaluationResult, SchedulingDecisionState,
    TaskWork, UnknownReasonToken, live_pool_unknown_reason, min_wake, next_pool_projection_change,
    offset_rank, project_pool_value, reason_with, resource_suggestion, scope_matches_instance,
    unknown_token,
};
use crate::codes::{BenefitGap, PolicyCode};
use crate::{
    ObservationRef, PoolProjection, PoolSpec, PredicateSpec, TaskSpec, ValuationDeclaration,
};

/// The instance fact family of instructions: a person's or an agent's request is a fact of it.
const INSTRUCTION_PREFIX: &str = "session.";

/// A manual request (design `rd-1029/design-sch-state.md` §4.9): a task whose trigger requires
/// an instruction, a `fact` or `fact_time_reached` predicate on a key of the `session.` family,
/// reached from the trigger's root only through `all`. It runs because it was asked for, not by
/// its net benefit.
pub(super) fn manual_request(task: &TaskSpec) -> bool {
    super::tie_break::required_conditions(&task.trigger)
        .into_iter()
        .any(|condition| match condition {
            PredicateSpec::Fact { fact_key, .. }
            | PredicateSpec::FactTimeReached { fact_key, .. } => {
                fact_key.starts_with(INSTRUCTION_PREFIX)
            }
            _ => false,
        })
}

/// A judged candidate's known positive net benefit `B` and the expected duration `E` it is
/// divided by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct KnownBenefit {
    /// `B` in milli, floored, at most `i64::MAX`.
    pub(super) benefit_milli: i64,
    /// `E`: the duration the utility term divides by, at least 1.
    pub(super) expected_duration_ms: u64,
}

impl KnownBenefit {
    /// `Less` when `self` goes first: the higher `B ÷ E`, compared exactly as `B·E' > B'·E` (two
    /// 64-bit factors never overflow 128 bits).
    pub(super) fn order(&self, other: &Self) -> Ordering {
        let left =
            u128::from(self.benefit_milli.unsigned_abs()) * u128::from(other.expected_duration_ms);
        let right =
            u128::from(other.benefit_milli.unsigned_abs()) * u128::from(self.expected_duration_ms);
        right.cmp(&left)
    }

    /// ` benefit_milli=<B> expected_duration_ms=<E>`, the developer text of a record.
    pub(super) fn detail(&self) -> String {
        format!(
            " benefit_milli={} expected_duration_ms={}",
            self.benefit_milli, self.expected_duration_ms
        )
    }
}

/// Why a judged candidate's net benefit is unknown.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Gap {
    NoEffects,
    EntryPending(String),
    ValuationPending(String),
    ValuationMissing(String),
    ReadingUnknown {
        pool_id: String,
        fact_key: Option<String>,
        unknown_reason: Option<UnknownReason>,
    },
}

/// What a judged candidate's inputs give.
enum Judgement {
    Known {
        benefit_milli: i128,
        items: Vec<String>,
        /// The next step of a regenerating pool it consumes: its cost falls as the pool fills.
        wake_unix_ms: Option<u64>,
    },
    Unknown {
        gap: Gap,
        /// The readings it waits for, as a predicate on them would ask.
        suggestions: Vec<DetectionSuggestion>,
    },
}

/// An exact signed sum of rational terms, reduced as it goes.
struct ExactSum {
    numerator: i128,
    denominator: i128,
}

impl ExactSum {
    const fn new() -> Self {
        Self {
            numerator: 0,
            denominator: 1,
        }
    }

    fn add(&mut self, numerator: u128, denominator: u128, negative: bool) -> Option<()> {
        let numerator = i128::try_from(numerator).ok()?;
        let numerator = if negative { -numerator } else { numerator };
        let denominator = i128::try_from(denominator).ok()?;
        let common = gcd(self.denominator.unsigned_abs(), denominator.unsigned_abs());
        let common = i128::try_from(common).ok()?;
        let left = self.numerator.checked_mul(denominator / common)?;
        let right = numerator.checked_mul(self.denominator / common)?;
        let sum = left.checked_add(right)?;
        let product = self.denominator.checked_mul(denominator / common)?;
        let reduce = i128::try_from(gcd(sum.unsigned_abs(), product.unsigned_abs()))
            .ok()?
            .max(1);
        self.numerator = sum / reduce;
        self.denominator = product / reduce;
        Some(())
    }

    /// The floor of the sum.
    fn floor(&self) -> i128 {
        self.numerator.div_euclid(self.denominator)
    }
}

fn gcd(mut left: u128, mut right: u128) -> u128 {
    while right != 0 {
        (left, right) = (right, left % right);
    }
    left
}

/// `Π numerators ÷ Π denominators`, reduced while multiplying; `None` on overflow or a zero
/// denominator.
fn ratio(numerators: &[u128], denominators: &[u128]) -> Option<(u128, u128)> {
    let mut numerator = 1_u128;
    let mut denominator = 1_u128;
    for &factor in denominators {
        if factor == 0 {
            return None;
        }
        denominator = denominator.checked_mul(factor)?;
    }
    for &factor in numerators {
        let common = gcd(factor, denominator).max(1);
        numerator = numerator.checked_mul(factor / common)?;
        denominator /= common;
    }
    Some((numerator, denominator))
}

/// The pool a judged task touches, with its declared valuation, or why it gives none.
fn valued_pool<'a>(
    pool_id: &str,
    instance: &InstanceSnapshot,
    context: &'a PlacementContext<'_>,
) -> Result<(&'a PoolSpec, &'a crate::PoolValuation), Gap> {
    let pool = context
        .pool_specs
        .get(pool_id)
        .copied()
        .filter(|pool| scope_matches_instance(&pool.scope, instance))
        .ok_or_else(|| Gap::ValuationMissing(pool_id.to_owned()))?;
    match &pool.valuation {
        Some(ValuationDeclaration::Declared(valuation)) => Ok((pool, valuation)),
        Some(ValuationDeclaration::Pending) => Err(Gap::ValuationPending(pool_id.to_owned())),
        None => Err(Gap::ValuationMissing(pool_id.to_owned())),
    }
}

/// The candidate's net benefit (module doc). `weight` is the resource layer's effective weight
/// `W(k,p)` of a pool the task produces, when its term holds one; otherwise the pool's base
/// weight is `W`.
fn judge(
    task: &TaskSpec,
    instance: &InstanceSnapshot,
    context: &PlacementContext<'_>,
    facts: &EvaluationFacts,
    weight: impl Fn(&str) -> Option<u64>,
) -> PolicyEvaluationResult<Judgement> {
    let unknown = |gap| Judgement::Unknown {
        gap,
        suggestions: Vec::new(),
    };
    if task.produces.is_empty() && task.consumes.is_empty() {
        return Ok(unknown(Gap::NoEffects));
    }
    // Every entry and every pool it touches, in declaration order, produces first.
    for effect in task.produces.iter().chain(&task.consumes) {
        if effect.is_pending() {
            return Ok(unknown(Gap::EntryPending(effect.pool_id.clone())));
        }
        if let Err(gap) = valued_pool(&effect.pool_id, instance, context) {
            return Ok(unknown(gap));
        }
    }
    let overflow = || {
        PolicyEvaluationError::overflow(format!(
            "task '{}' net benefit overflowed on instance '{}'",
            task.id, instance.instance_id
        ))
    };
    let mut sum = ExactSum::new();
    let mut items = Vec::new();
    let mut seen = Vec::<&str>::new();
    for effect in &task.produces {
        if seen.contains(&effect.pool_id.as_str()) {
            continue;
        }
        seen.push(effect.pool_id.as_str());
        let (_, valuation) = valued_pool(&effect.pool_id, instance, context)
            .map_err(|_| PolicyEvaluationError::invalid("a judged pool lost its valuation"))?;
        let produced = crate::resource_targets::per_run_yield(&task.produces, &effect.pool_id);
        let weight_milli =
            weight(&effect.pool_id).unwrap_or_else(|| u64::from(valuation.base_weight_milli));
        let (numerator, denominator) = ratio(
            &[produced, u128::from(weight_milli)],
            &[u128::from(valuation.scale), 1_000],
        )
        .ok_or_else(overflow)?;
        sum.add(numerator, denominator, false)
            .ok_or_else(overflow)?;
        items.push(format!(
            "{}:+{}",
            effect.pool_id,
            numerator / denominator.max(1)
        ));
    }
    let mut seen = Vec::<&str>::new();
    let mut readings = Vec::new();
    let mut wake_unix_ms = None;
    for effect in &task.consumes {
        if seen.contains(&effect.pool_id.as_str()) {
            continue;
        }
        seen.push(effect.pool_id.as_str());
        let (pool, valuation) = valued_pool(&effect.pool_id, instance, context)
            .map_err(|_| PolicyEvaluationError::invalid("a judged pool lost its valuation"))?;
        let consumed = crate::resource_targets::declared_consumption(task, &effect.pool_id)?;
        let weight_milli =
            weight(&effect.pool_id).unwrap_or_else(|| u64::from(valuation.base_weight_milli));
        // The share of `W` a unit consumed costs: 1 for a pool that does not regenerate.
        let (share_numerator, share_denominator) = match &pool.projection {
            PoolProjection::NonRegenerating { .. } => (1_u128, 1_u128),
            PoolProjection::Regenerating(_) => {
                let Some(reading) = context.pool_values.get(effect.pool_id.as_str()) else {
                    readings.push(pool);
                    continue;
                };
                let Ok(current) =
                    project_pool_value(pool, reading.bound, reading.snapshot, context.time.unix_ms)
                else {
                    readings.push(pool);
                    continue;
                };
                if let Ok(Some(next)) = next_pool_projection_change(
                    pool,
                    reading.bound,
                    reading.snapshot,
                    context.time.unix_ms,
                ) {
                    wake_unix_ms = min_wake(wake_unix_ms, Some(next));
                }
                let capacity = i128::from(reading.bound.capacity());
                let reserve = task
                    .reserves
                    .iter()
                    .flatten()
                    .filter(|reserve| reserve.pool_id == effect.pool_id)
                    .map(|reserve| i128::from(reserve.at_least))
                    .max()
                    .unwrap_or(0);
                discount(capacity, i128::from(current), reserve)
            }
        };
        let (numerator, denominator) = ratio(
            &[
                u128::from(consumed) * 1_000,
                u128::from(weight_milli),
                share_numerator,
            ],
            &[u128::from(valuation.scale), 1_000, share_denominator],
        )
        .ok_or_else(overflow)?;
        sum.add(numerator, denominator, true).ok_or_else(overflow)?;
        items.push(format!(
            "{}:-{}",
            effect.pool_id,
            numerator.div_ceil(denominator.max(1))
        ));
    }
    if let Some(first) = readings.first() {
        let fact_key = match &first.observation {
            ObservationRef::Fact { fact_key } => Some(fact_key.clone()),
            ObservationRef::Outcome { .. } => None,
        };
        let unknown_reason = fact_key
            .as_deref()
            .and_then(|key| live_pool_unknown_reason(first, key, facts, context.time));
        return Ok(Judgement::Unknown {
            gap: Gap::ReadingUnknown {
                pool_id: first.id.clone(),
                fact_key,
                unknown_reason,
            },
            suggestions: readings
                .iter()
                .map(|pool| resource_suggestion(pool, facts, context.time))
                .collect(),
        });
    }
    Ok(Judgement::Known {
        benefit_milli: sum.floor(),
        items,
        wake_unix_ms,
    })
}

/// The share of the weight a consumed unit of a regenerating pool costs, as a reduced
/// fraction: `clamp((cap − cur)/(cap − reserve), 0, 1)`. A reserve at or above the capacity
/// leaves no room between the two lines: the share is 0 at capacity and 1 below it.
fn discount(capacity: i128, current: i128, reserve: i128) -> (u128, u128) {
    let room = capacity - reserve;
    let missing = (capacity - current).max(0);
    if room <= 0 {
        return if missing == 0 { (0, 1) } else { (1, 1) };
    }
    let missing = missing.min(room);
    let (numerator, denominator) = (missing.unsigned_abs(), room.unsigned_abs());
    let common = gcd(numerator, denominator).max(1);
    (numerator / common, denominator / common)
}

/// The expected duration `E` the utility term divides by (SCH-M ED-1): the pair's `E` when its
/// snapshot carries one, else the last settled run's duration, else the declared one; at least 1.
pub(super) fn expected_duration_ms(candidate: &PlacementCandidate) -> u64 {
    candidate
        .expected_runtime_ms
        .unwrap_or_else(|| {
            candidate
                .last_duration_ms
                .filter(|duration| *duration > 0)
                .unwrap_or(candidate.expected_duration_ms)
        })
        .max(1)
}

/// The benefit gate (module doc) over the score stage's ranked candidates, before the reserve
/// gate: an unjudged candidate passes untouched; a judged one passes with its known positive `B`
/// and `E`, or is deferred with `policy_benefit_nonpositive` or `benefit_unknown` and takes no
/// part in the allocation. `weights` holds, by work index, the effective weight of each pool
/// the candidate's resource term weighs. With `refreshing`, a candidate whose only gap is an
/// unknown reading asks the fact refresh for it through `affected`.
pub(super) fn gate(
    context: &PlacementContext<'_>,
    facts: &EvaluationFacts,
    ranked: Vec<PlacementCandidate>,
    work: &mut [TaskWork],
    weights: &BTreeMap<usize, BTreeMap<String, u64>>,
    refreshing: bool,
    affected: &mut Vec<AffectedCandidate>,
) -> PolicyEvaluationResult<Vec<PlacementCandidate>> {
    let mut kept = Vec::with_capacity(ranked.len());
    for mut candidate in ranked {
        if !candidate.judged {
            kept.push(candidate);
            continue;
        }
        let task = context
            .tasks
            .get(candidate.task_id.as_str())
            .map(|(_, task)| *task)
            .ok_or_else(|| {
                PolicyEvaluationError::invalid(format!(
                    "judged task '{}' is missing from the catalog",
                    candidate.task_id
                ))
            })?;
        let instance = facts
            .instances
            .iter()
            .find(|instance| instance.instance_id == candidate.instance_id)
            .ok_or_else(|| {
                PolicyEvaluationError::invalid(format!(
                    "judged candidate's instance '{}' is missing from the snapshot",
                    candidate.instance_id
                ))
            })?;
        let candidate_weights = weights.get(&candidate.work_index);
        let judgement = judge(task, instance, context, facts, |pool_id| {
            candidate_weights.and_then(|weights| weights.get(pool_id).copied())
        })?;
        let expected = expected_duration_ms(&candidate);
        let task_work = &mut work[candidate.work_index];
        match judgement {
            Judgement::Known {
                benefit_milli,
                items,
                wake_unix_ms,
            } => {
                let benefit_milli = i64::try_from(benefit_milli).unwrap_or(if benefit_milli < 0 {
                    i64::MIN
                } else {
                    i64::MAX
                });
                if benefit_milli > 0 {
                    let known = KnownBenefit {
                        benefit_milli,
                        expected_duration_ms: expected,
                    };
                    candidate.benefit = Some(known);
                    if let Some(record) = task_work.record.as_mut() {
                        record.benefit = Some(known);
                    }
                    kept.push(candidate);
                    continue;
                }
                task_work.state = SchedulingDecisionState::Deferred;
                task_work
                    .reasons
                    .push(nonpositive_reason(benefit_milli, expected, &items));
                if let Some(wake) = wake_unix_ms.filter(|wake| *wake > context.time.unix_ms) {
                    task_work.next_wake_unix_ms = min_wake(task_work.next_wake_unix_ms, Some(wake));
                }
            }
            Judgement::Unknown { gap, suggestions } => {
                task_work.state = SchedulingDecisionState::Deferred;
                task_work.reasons.push(unknown_reason(&gap));
                if refreshing && !suggestions.is_empty() {
                    let keys = fact_refresh::predicate_keys(&suggestions, instance);
                    if !keys.is_empty() {
                        // The rank it lends the reader: its catalog rank before the score stage,
                        // with its manual offset (D2 model §3.4).
                        let mut rank = candidate.rank.clone();
                        if let Some(record) = task_work.record.as_ref() {
                            rank.total_score = record.base_total;
                        }
                        affected.push(AffectedCandidate {
                            work_index: candidate.work_index,
                            instance_id: candidate.instance_id.clone(),
                            rank: offset_rank(
                                &rank,
                                facts,
                                &candidate.task_id,
                                &candidate.instance_id,
                            ),
                            keys,
                            predicate: false,
                        });
                    }
                }
            }
        }
    }
    Ok(kept)
}

/// `policy_benefit_nonpositive` (info; `benefit_milli`).
fn nonpositive_reason(benefit_milli: i64, expected: u64, items: &[String]) -> DecisionReason {
    let outcome = Outcome::new(PolicyCode::PolicyBenefitNonpositive)
        .benefit_milli(benefit_milli)
        .expected_duration_ms(expected);
    reason_with(
        outcome,
        format!(
            "benefit_milli={benefit_milli} expected_duration_ms={expected} items={}",
            items.join(",")
        ),
    )
}

/// `benefit_unknown` (info; `benefit_gap`; `pool_id`, `fact_key`, `unknown_reason` when they
/// apply).
fn unknown_reason(gap: &Gap) -> DecisionReason {
    let (token, pool_id) = match gap {
        Gap::NoEffects => (BenefitGap::NoEffects, None),
        Gap::EntryPending(pool) => (BenefitGap::EntryPending, Some(pool)),
        Gap::ValuationPending(pool) => (BenefitGap::ValuationPending, Some(pool)),
        Gap::ValuationMissing(pool) => (BenefitGap::ValuationMissing, Some(pool)),
        Gap::ReadingUnknown { pool_id, .. } => (BenefitGap::ReadingUnknown, Some(pool_id)),
    };
    let mut outcome = Outcome::new(PolicyCode::BenefitUnknown).benefit_gap(token);
    let mut detail = format!("benefit_gap={}", token.as_str());
    if let Some(pool_id) = pool_id {
        outcome = outcome.pool_id(pool_id.as_str());
        detail.push_str(&format!(" pool_id={pool_id}"));
    }
    if let Gap::ReadingUnknown {
        fact_key,
        unknown_reason,
        ..
    } = gap
    {
        if let Some(fact_key) = fact_key {
            outcome = outcome.fact_key(fact_key.as_str());
            detail.push_str(&format!(" fact_key={fact_key}"));
        }
        if let Some(unknown_reason) = unknown_reason {
            outcome = outcome.unknown_reason(UnknownReasonToken(*unknown_reason));
            detail.push_str(&format!(
                " unknown_reason={}",
                unknown_token(*unknown_reason)
            ));
        }
    }
    reason_with(outcome, detail)
}
