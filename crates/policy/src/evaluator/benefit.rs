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
//!   `consume` is the task's declared consumption in thousandths, exact (SS-2g, register
//!   §1.232: `C = Σ amount × 1000 + Σ expected_amount_milli`; the reserve gate reads
//!   `⌈C/1000⌉`); `Q_p` is the pool's `valuation.scale`. `P(k,p)`, the cost of consuming pool
//!   `p`, is `W(k,p)` whatever the pool's fill (SS-2h, register §1.289; Alice's ruling 138 and
//!   her answer of 11-09 16:39: below the cap nothing is spent for free). SS-2b's linear discount
//!   `W·clamp((cap − cur)/(cap − reserve), 0, 1)` is retired; a pool that reached its cap is spent
//!   by the capped spending order ([`super::capped`]).
//! - **Unknown is never 0.** No declared effect, a pending entry, a pending or missing valuation
//!   of a pool it touches, or an unknown or expired reading of a regenerating pool it consumes
//!   makes `B` unknown: the candidate is not dispatched (`benefit_unknown`), and the known part
//!   is never added up. An unknown reading also asks the fact refresh for it. A known `B ≤ 0` is
//!   not dispatched either (`policy_benefit_nonpositive`).
//! - **The one-way rule.** Nothing here writes a pool weight: the weights are read from the
//!   resource layer's own term, which reads no task state.
//! - **The zero band** (SS-2d, register §1.235; Alice's ruling 127, the review's version; design
//!   `rd-1031/design-acm-price-addendum2.1.md` §7.2 and §7.3). The exact sum `S` (before the
//!   floor) differs from the model's value at the exported prices by at most
//!   `ε = Σ_produces [R/(2000·Q) + n·(2W+1)/(4000·Q)] + Σ_consumes [C/(2000·Q) +
//!   m·(2W+1)/(4000·Q)]`: `R` and `C` the per-run production and consumption in thousandths, `n`
//!   and `m` the pool's `expected_amount_milli` entries in each direction, `W` the weight read
//!   above and `Q` the scale, summed exactly as `S` is (SS-2h: the whole `C`, as no share of the
//!   cost is discounted; SS-2d's regenerating share `σ` is retired with the discount). With `|S| ≤ ε`
//!   the sign is the rounding's and is not trusted: the candidate is `benefit_within_tolerance`,
//!   neither positive nor `policy_benefit_nonpositive`. A task marked `benefit_realization` whose
//!   produced pools include one weighed above 0 and whose latest run did not fail is released
//!   (`zero_tier: released`) into the last place of its tier ([`super::allocation_order`]); every
//!   other is deferred (`unmarked`, `no_downstream_value`, `latest_failed`). Outside the band the
//!   gate is SS-2b's: `B = ⌊S⌋ > 0` is positive, any other known `B` not positive.
//! - **The pool lines** (SS-2h, register §1.289): between judging and deciding, the gate asks the
//!   pools' lines ([`super::capped::plan`]). A consumer of a capped pool that its round's order
//!   tries is kept whatever the sign of its `B` (its own `B` charging only the part of its
//!   consumption below the stop line); any other consumer of a capped pool is held; a candidate
//!   that would pass but leave a pool below its hard line is held too.
//! - **A fixed value** (SCHED-C1a, register §1.299; Alice's two kinds of scoring of 11-09 15:47
//!   and 15:50; `contracts/scheduling/README.md`, "Fixed values written in the pack"). A task
//!   that writes `benefit_fixed_value_milli` `V` is judged by
//!   `B = V − Σ_p consume(k,p)·W(k,p)/Q_p`: `V` itself without a consumption, its `produces`
//!   unread. A consumed pool still needs its valuation and a regenerating one its reading, as
//!   above; `ε` holds the consumption's terms only, a fixed value being exact. So below the cap a
//!   consumer's `B` is `V − W·c`, and in a capped round its value without the pool's cost is `V`
//!   less the other pools' cost ([`super::capped`] reads it from `S`). Static and dynamic
//!   candidates rank on the one scale `B ÷ E`. Every decision whose `B` came from a fixed value
//!   carries `benefit_fixed_value` before its other reasons.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use actingcommand_contract::outcome::Outcome;
use actingcommand_selection_policy::UnknownReason;

use super::capped::{self, CappedMemory, Disposition};
use super::fact_refresh::{self, AffectedCandidate};
use super::{
    DecisionReason, DetectionSuggestion, EvaluationFacts, InstanceSnapshot, PlacementCandidate,
    PlacementContext, PolicyEvaluationError, PolicyEvaluationResult, SchedulingDecisionState,
    TaskWork, UnknownReasonToken, live_pool_unknown_reason, min_wake, next_pool_projection_change,
    offset_rank, project_pool_value, reason_with, resource_suggestion, scope_matches_instance,
    unknown_token,
};
use crate::codes::{BenefitGap, PolicyCode, ZeroTier};
use crate::{
    ObservationRef, PoolProjection, PoolSpec, PredicateSpec, TaskSpec, TaskTerminalState,
    ValuationDeclaration,
};

/// The instance fact family of instructions: a person's or an agent's request is a fact of it.
const INSTRUCTION_PREFIX: &str = "session.";

/// A manual request (design `rd-1029/design-sch-state.md` §4.9): a task whose trigger requires
/// an instruction, a `fact` or `fact_time_reached` predicate on a key of the `session.` family,
/// reached from the trigger's root only through `all`. It runs because it was asked for, not by
/// its net benefit. SCHED-C1c (register §1.299): the compiler's check of a pool its lines cannot
/// spend reads the capped order's consumers by it too.
pub(crate) fn manual_request(task: &TaskSpec) -> bool {
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

/// A judged candidate's known net benefit `B` and the expected duration `E` it is divided by:
/// positive, except for a consumer a capped spending order tries (SS-2h), whose `B` may be any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct KnownBenefit {
    /// `B` in milli, floored, within the `i64` range.
    pub(super) benefit_milli: i64,
    /// `E`: the duration the utility term divides by, at least 1.
    pub(super) expected_duration_ms: u64,
    /// SCHED-C1a (register §1.299): `V`, the task's fixed value `B` was computed from; `None` for
    /// a `B` the effects give. A judged winner's `rank_breakdown` names it.
    pub(super) fixed_value_milli: Option<i64>,
}

impl KnownBenefit {
    /// `Less` when `self` goes first: the higher `B ÷ E`, compared exactly as `B·E' > B'·E` (a
    /// 64-bit and a 63-bit factor never overflow 128 bits). SS-2h: signed, so a capped head whose
    /// `B` is not positive goes after every positive one; two positive ones compare as before.
    pub(super) fn order(&self, other: &Self) -> Ordering {
        let left = i128::from(self.benefit_milli) * i128::from(other.expected_duration_ms);
        let right = i128::from(other.benefit_milli) * i128::from(self.expected_duration_ms);
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
        /// SS-2d: `S`, the exact sum before the floor; `B = ⌊S⌋`.
        sum: ExactSum,
        /// SS-2d: `ε`, the exact rounding bound of `S` (module doc).
        tolerance: ExactSum,
        items: Vec<String>,
        /// The next step of a regenerating pool it consumes (SS-2h: its cost stays `W` a unit;
        /// the evaluation still wakes as the pool fills, as before).
        wake_unix_ms: Option<u64>,
        /// SS-2d: a pool it produces is weighed above 0 on the instance (a model price or a
        /// goal's shortfall gives it a use).
        downstream: bool,
        /// SS-2h: the term of each pool it consumes, in declaration order (each pool once).
        consumed: Vec<ConsumedTerm>,
    },
    Unknown {
        gap: Gap,
        /// The readings it waits for, as a predicate on them would ask.
        suggestions: Vec<DetectionSuggestion>,
    },
}

/// SS-2h (register §1.289): one consumed pool's part of a judged candidate's `S` and `ε`, which
/// the capped spending order takes out again: its value without the pool's cost is `S` plus
/// `cost`, and that value's bound `ε` less `bound`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ConsumedTerm {
    pub(super) pool_id: String,
    /// `C`, the declared consumption in thousandths (SS-2g).
    pub(super) consumed_milli: u128,
    /// `W`, the weight the cost reads.
    pub(super) weight_milli: u64,
    /// `Q`, the pool's valuation scale.
    pub(super) scale: u64,
    /// `C·W/(1000·Q)`, the cost subtracted from `S`, as a reduced fraction.
    pub(super) cost: (u128, u128),
    /// `[2C + m·(2W+1)]/(4000·Q)`, the term added to `ε`, as a reduced fraction.
    pub(super) bound: (u128, u128),
}

/// An exact signed sum of rational terms, reduced as it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ExactSum {
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

    /// SS-2h: this sum with one more term; `None` on overflow.
    pub(super) fn plus(
        &self,
        (numerator, denominator): (u128, u128),
        negative: bool,
    ) -> Option<Self> {
        let mut sum = self.clone();
        sum.add(numerator, denominator, negative)?;
        Some(sum)
    }

    /// SS-2h: the sum as a non-negative reduced fraction `(numerator, denominator)`; `None` when
    /// it is negative.
    pub(super) fn non_negative(&self) -> Option<(u128, u128)> {
        Some((
            u128::try_from(self.numerator).ok()?,
            self.denominator.unsigned_abs(),
        ))
    }

    /// SS-2h: `self > bound` for a bound that is not negative, compared exactly; `None` for a
    /// negative bound.
    pub(super) fn exceeds(&self, bound: &Self) -> Option<bool> {
        Some(self.numerator > 0 && !self.within(bound)?)
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
    pub(super) fn floor(&self) -> i128 {
        self.numerator.div_euclid(self.denominator)
    }

    /// SS-2d: the ceiling of a sum that is not negative (a rounding bound).
    fn ceil(&self) -> i128 {
        -(-self.numerator).div_euclid(self.denominator)
    }

    /// SS-2d: `|self| ≤ bound` for a bound that is not negative, compared exactly by
    /// [`compare_ratios`], which never multiplies; `None` for a negative bound.
    fn within(&self, bound: &Self) -> Option<bool> {
        let bound_numerator = u128::try_from(bound.numerator).ok()?;
        Some(
            compare_ratios(
                (
                    self.numerator.unsigned_abs(),
                    self.denominator.unsigned_abs(),
                ),
                (bound_numerator, bound.denominator.unsigned_abs()),
            ) != Ordering::Greater,
        )
    }

    /// SS-2d: the reduced fraction `<numerator>/<denominator>`, for a record's developer text.
    fn fraction(&self) -> String {
        format!("{}/{}", self.numerator, self.denominator)
    }
}

/// SS-2d: `a/b` against `c/d` (non-negative numerators, positive denominators), exactly and
/// without a product that could overflow: equal integer parts compare their remainders' reciprocals
/// the other way round, as the continued fractions of the two ratios do; Euclid's steps end it.
pub(super) fn compare_ratios(
    (mut a, mut b): (u128, u128),
    (mut c, mut d): (u128, u128),
) -> Ordering {
    loop {
        let (left, right) = (a / b, c / d);
        if left != right {
            return left.cmp(&right);
        }
        let (left_rest, right_rest) = (a % b, c % d);
        match (left_rest == 0, right_rest == 0) {
            (true, true) => return Ordering::Equal,
            (true, false) => return Ordering::Less,
            (false, true) => return Ordering::Greater,
            (false, false) => {}
        }
        // left_rest/b against right_rest/d is d/right_rest against b/left_rest.
        (a, b, c, d) = (d, right_rest, b, left_rest);
    }
}

/// SS-2d: the numerator over `4000·Q` of one pool's rounding bound before its regenerating share,
/// `2·quantity + expected·(2·W + 1)`: `quantity/(2000·Q)` for the weight's half step on what it
/// produces or consumes, and `(2·W + 1)/(4000·Q)` for each `expected_amount_milli` entry's half
/// thousandth (design §7.2). `None` on overflow.
fn rounding_numerator(
    quantity_milli: u128,
    expected_entries: usize,
    weight_milli: u64,
) -> Option<u128> {
    let per_entry = u128::from(weight_milli).checked_mul(2)?.checked_add(1)?;
    quantity_milli.checked_mul(2)?.checked_add(
        u128::try_from(expected_entries)
            .ok()?
            .checked_mul(per_entry)?,
    )
}

/// SS-2d: how many of `effects`' entries for `pool_id` declare `expected_amount_milli`.
fn expected_entries(effects: &[crate::ResourceEffectSpec], pool_id: &str) -> usize {
    effects
        .iter()
        .filter(|effect| effect.pool_id == pool_id && effect.expected_amount_milli.is_some())
        .count()
}

fn gcd(mut left: u128, mut right: u128) -> u128 {
    while right != 0 {
        (left, right) = (right, left % right);
    }
    left
}

/// `Π numerators ÷ Π denominators`, reduced while multiplying; `None` on overflow or a zero
/// denominator.
pub(super) fn ratio(numerators: &[u128], denominators: &[u128]) -> Option<(u128, u128)> {
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
///
/// SCHED-C1a (register §1.299): with the task's fixed value `V` the sum starts at `V`, its
/// `produces` are not read (neither their pools' valuations nor `ε`'s production terms), and the
/// consumption is judged as before: `B = V` without one, `V` less its cost otherwise.
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
    let fixed_value = task.benefit_fixed_value_milli;
    let produces: &[crate::ResourceEffectSpec] = match fixed_value {
        Some(_) => &[],
        None => &task.produces,
    };
    if fixed_value.is_none() && task.produces.is_empty() && task.consumes.is_empty() {
        return Ok(unknown(Gap::NoEffects));
    }
    // Every entry and every pool it touches, in declaration order, produces first.
    for effect in produces.iter().chain(&task.consumes) {
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
    // SCHED-C1a: a fixed value is exact, so it adds nothing to `ε`.
    if let Some(value) = fixed_value {
        let value = u128::try_from(value).map_err(|_| {
            PolicyEvaluationError::invalid(format!(
                "task '{}' has a negative fixed value on instance '{}'",
                task.id, instance.instance_id
            ))
        })?;
        sum.add(value, 1, false).ok_or_else(overflow)?;
    }
    // SS-2d: the rounding bound `ε` of `sum`, accumulated as exactly (module doc).
    let mut tolerance = ExactSum::new();
    let mut downstream = false;
    let mut items = Vec::new();
    let mut seen = Vec::<&str>::new();
    for effect in produces {
        if seen.contains(&effect.pool_id.as_str()) {
            continue;
        }
        seen.push(effect.pool_id.as_str());
        let (_, valuation) = valued_pool(&effect.pool_id, instance, context)
            .map_err(|_| PolicyEvaluationError::invalid("a judged pool lost its valuation"))?;
        let produced = crate::resource_targets::per_run_yield(&task.produces, &effect.pool_id);
        let weight_milli =
            weight(&effect.pool_id).unwrap_or_else(|| u64::from(valuation.base_weight_milli));
        downstream |= weight_milli > 0;
        let (numerator, denominator) = ratio(
            &[produced, u128::from(weight_milli)],
            &[u128::from(valuation.scale), 1_000],
        )
        .ok_or_else(overflow)?;
        sum.add(numerator, denominator, false)
            .ok_or_else(overflow)?;
        // SS-2d: `R/(2000·Q) + n·(2W+1)/(4000·Q)`.
        let rounding = rounding_numerator(
            produced,
            expected_entries(&task.produces, &effect.pool_id),
            weight_milli,
        )
        .ok_or_else(overflow)?;
        let (bound_numerator, bound_denominator) =
            ratio(&[rounding], &[u128::from(valuation.scale), 4_000]).ok_or_else(overflow)?;
        tolerance
            .add(bound_numerator, bound_denominator, false)
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
    let mut consumed = Vec::new();
    for effect in &task.consumes {
        if seen.contains(&effect.pool_id.as_str()) {
            continue;
        }
        seen.push(effect.pool_id.as_str());
        let (pool, valuation) = valued_pool(&effect.pool_id, instance, context)
            .map_err(|_| PolicyEvaluationError::invalid("a judged pool lost its valuation"))?;
        // SS-2g: in thousandths, so an expected consumption costs exactly its share; an integer
        // catalog's numerator is the `amount × 1000` of before.
        let consumed_milli =
            crate::resource_targets::declared_consumption_milli(task, &effect.pool_id)?;
        let weight_milli =
            weight(&effect.pool_id).unwrap_or_else(|| u64::from(valuation.base_weight_milli));
        // SS-2h (register §1.289): a unit consumed costs its full weight `W` whatever the pool's
        // fill; SS-2b's linear discount is retired. A regenerating pool still needs its reading,
        // as before (unknown or expired, `B` is unknown), and wakes the evaluation at its next
        // step.
        if let PoolProjection::Regenerating(_) = &pool.projection {
            let Some(reading) = context.pool_values.get(effect.pool_id.as_str()) else {
                readings.push(pool);
                continue;
            };
            if project_pool_value(pool, reading.bound, reading.snapshot, context.time.unix_ms)
                .is_err()
            {
                readings.push(pool);
                continue;
            }
            if let Ok(Some(next)) = next_pool_projection_change(
                pool,
                reading.bound,
                reading.snapshot,
                context.time.unix_ms,
            ) {
                wake_unix_ms = min_wake(wake_unix_ms, Some(next));
            }
        }
        let (numerator, denominator) = ratio(
            &[consumed_milli, u128::from(weight_milli)],
            &[u128::from(valuation.scale), 1_000],
        )
        .ok_or_else(overflow)?;
        sum.add(numerator, denominator, true).ok_or_else(overflow)?;
        // SS-2d: `C/(2000·Q) + m·(2W+1)/(4000·Q)`, SS-2g's thousandths as `C`, the whole of `C`
        // (SS-2h: no share of it is discounted).
        let rounding = rounding_numerator(
            consumed_milli,
            expected_entries(&task.consumes, &effect.pool_id),
            weight_milli,
        )
        .ok_or_else(overflow)?;
        let (bound_numerator, bound_denominator) =
            ratio(&[rounding], &[u128::from(valuation.scale), 4_000]).ok_or_else(overflow)?;
        tolerance
            .add(bound_numerator, bound_denominator, false)
            .ok_or_else(overflow)?;
        items.push(format!(
            "{}:-{}",
            effect.pool_id,
            numerator.div_ceil(denominator.max(1))
        ));
        consumed.push(ConsumedTerm {
            pool_id: effect.pool_id.clone(),
            consumed_milli,
            weight_milli,
            scale: valuation.scale,
            cost: (numerator, denominator),
            bound: (bound_numerator, bound_denominator),
        });
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
        sum,
        tolerance,
        items,
        wake_unix_ms,
        downstream,
        consumed,
    })
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
///
/// SS-2d (register §1.235): three ways for a known `B`. Within its rounding error (`|S| ≤ ε`)
/// it is `benefit_within_tolerance`: released (`zero_tier`, the allocation's last place in its
/// tier) or deferred; outside, as before, positive with `B` or `policy_benefit_nonpositive`.
///
/// SS-2h (register §1.289): every judged candidate is judged first; the pools' lines then decide
/// ([`capped::plan`], with the Runtime's `memory` of the capped episodes):
/// a consumer a capped round tries is kept with its own `B` whatever its sign, another consumer of
/// a capped pool is held, and a candidate that would pass but leave a pool below its hard line is
/// held. Without a valued pool nothing changes.
#[allow(clippy::too_many_arguments)]
pub(super) fn gate(
    context: &PlacementContext<'_>,
    facts: &EvaluationFacts,
    ranked: Vec<PlacementCandidate>,
    work: &mut [TaskWork],
    weights: &BTreeMap<usize, BTreeMap<String, u64>>,
    refreshing: bool,
    affected: &mut Vec<AffectedCandidate>,
    memory: &mut CappedMemory,
) -> PolicyEvaluationResult<Vec<PlacementCandidate>> {
    // Phase 1: judge every judged candidate.
    let mut judged = Vec::with_capacity(ranked.len());
    for candidate in &ranked {
        if !candidate.judged {
            judged.push(None);
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
        judged.push(Some((task, instance, judgement)));
    }
    // Phase 2 (SS-2h): the pools' lines and the capped spending order.
    let dispositions = {
        let known = ranked
            .iter()
            .zip(&judged)
            .map(|(candidate, judged)| match judged {
                Some((
                    task,
                    _,
                    Judgement::Known {
                        sum,
                        tolerance,
                        consumed,
                        ..
                    },
                )) => Some(capped::Known {
                    candidate,
                    task,
                    sum,
                    tolerance,
                    consumed,
                }),
                _ => None,
            })
            .collect::<Vec<_>>();
        capped::plan(context, facts, &ranked, &known, work, memory)?
    };
    // Phase 3: decide.
    let mut kept = Vec::with_capacity(ranked.len());
    for ((mut candidate, judged), disposition) in ranked.into_iter().zip(judged).zip(dispositions) {
        let Some((task, instance, judgement)) = judged else {
            kept.push(candidate);
            continue;
        };
        let expected = expected_duration_ms(&candidate);
        let task_work = &mut work[candidate.work_index];
        match judgement {
            Judgement::Known {
                sum,
                tolerance,
                items,
                wake_unix_ms,
                downstream,
                consumed: _,
            } => {
                let defer = |task_work: &mut TaskWork, reason: DecisionReason| {
                    task_work.state = SchedulingDecisionState::Deferred;
                    task_work.reasons.push(reason);
                    if let Some(wake) = wake_unix_ms.filter(|wake| *wake > context.time.unix_ms) {
                        task_work.next_wake_unix_ms =
                            min_wake(task_work.next_wake_unix_ms, Some(wake));
                    }
                };
                // SS-2h: a capped pool's order decides before the sign of `B`. SCHED-C1a: a `B`
                // computed from a fixed value says so first, whatever the gate then decides.
                let hold = match disposition {
                    Disposition::Held(reason) => {
                        note_fixed_value(task_work, task, saturating_milli(sum.floor()), expected);
                        defer(task_work, reason);
                        continue;
                    }
                    Disposition::Member { benefit, mut slot } => {
                        note_fixed_value(task_work, task, benefit.benefit_milli, expected);
                        // SS-2h fix 1: a fallback of its block that waits takes the wake a held
                        // candidate takes.
                        slot.wake_unix_ms =
                            wake_unix_ms.filter(|wake| *wake > context.time.unix_ms);
                        candidate.benefit = Some(benefit);
                        candidate.capped = Some(*slot);
                        if let Some(record) = task_work.record.as_mut() {
                            record.benefit = Some(benefit);
                        }
                        kept.push(candidate);
                        continue;
                    }
                    Disposition::Normal { hold } => hold,
                };
                let benefit_milli = saturating_milli(sum.floor());
                note_fixed_value(task_work, task, benefit_milli, expected);
                // SS-2d: within its rounding error the sign is not trusted (module doc).
                let within = sum.within(&tolerance).ok_or_else(|| {
                    PolicyEvaluationError::invalid(format!(
                        "task '{}' net benefit tolerance is negative on instance '{}'",
                        task.id, instance.instance_id
                    ))
                })?;
                if within {
                    let tier = if !task.is_benefit_realization() {
                        ZeroTier::Unmarked
                    } else if !downstream {
                        ZeroTier::NoDownstreamValue
                    } else if context.last_result(task, instance).is_some_and(|state| {
                        state.terminal_state == Some(TaskTerminalState::Failed)
                    }) {
                        ZeroTier::LatestFailed
                    } else {
                        ZeroTier::Released
                    };
                    task_work.reasons.push(tolerance_reason(
                        (benefit_milli, &sum),
                        (saturating_milli(tolerance.ceil()), &tolerance),
                        tier,
                        &items,
                    ));
                    if matches!(tier, ZeroTier::Released) {
                        // SS-2h: a released one is held by a pool's lines as a positive one is.
                        if let Some(hold) = hold {
                            defer(task_work, hold);
                            continue;
                        }
                        // Every later gate applies; the allocation takes it last in its tier.
                        candidate.zero_tier = true;
                        kept.push(candidate);
                        continue;
                    }
                    task_work.state = SchedulingDecisionState::Deferred;
                    if let Some(wake) = wake_unix_ms.filter(|wake| *wake > context.time.unix_ms) {
                        task_work.next_wake_unix_ms =
                            min_wake(task_work.next_wake_unix_ms, Some(wake));
                    }
                    continue;
                }
                if benefit_milli > 0 {
                    // SS-2h: positive, but a pool's lines hold it (its hard line, or a capped
                    // pool's order).
                    if let Some(hold) = hold {
                        defer(task_work, hold);
                        continue;
                    }
                    let known = KnownBenefit {
                        benefit_milli,
                        expected_duration_ms: expected,
                        fixed_value_milli: task.benefit_fixed_value_milli,
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

/// `B` or `ε` in milli as a record value, saturated at the `i64` range.
pub(super) fn saturating_milli(value: i128) -> i64 {
    i64::try_from(value).unwrap_or(if value < 0 { i64::MIN } else { i64::MAX })
}

/// SS-2d: `benefit_within_tolerance` (info; `benefit_milli` ⌊S⌋, `tolerance_milli` ⌈ε⌉,
/// `zero_tier`); the developer text also gives `S` and `ε` as exact reduced fractions in milli.
fn tolerance_reason(
    (benefit_milli, sum): (i64, &ExactSum),
    (tolerance_milli, tolerance): (i64, &ExactSum),
    tier: ZeroTier,
    items: &[String],
) -> DecisionReason {
    let outcome = Outcome::new(PolicyCode::BenefitWithinTolerance)
        .benefit_milli(benefit_milli)
        .tolerance_milli(tolerance_milli)
        .zero_tier(tier);
    reason_with(
        outcome,
        format!(
            "benefit_milli={benefit_milli} tolerance_milli={tolerance_milli} zero_tier={} sum={} \
             tolerance={} items={}",
            tier.as_str(),
            sum.fraction(),
            tolerance.fraction(),
            items.join(",")
        ),
    )
}

/// SCHED-C1a (register §1.299): a judged candidate whose task has a fixed value carries
/// `benefit_fixed_value` with the `B` its decision reads before its other reasons.
fn note_fixed_value(task_work: &mut TaskWork, task: &TaskSpec, benefit_milli: i64, expected: u64) {
    if let Some(value) = task.benefit_fixed_value_milli {
        task_work
            .reasons
            .insert(0, fixed_value_reason(benefit_milli, value, expected));
    }
}

/// SCHED-C1a (register §1.299): `benefit_fixed_value` (info; `benefit_milli` `B`,
/// `fixed_value_milli` `V`, `expected_duration_ms` `E`).
fn fixed_value_reason(benefit_milli: i64, fixed_value_milli: i64, expected: u64) -> DecisionReason {
    let outcome = Outcome::new(PolicyCode::BenefitFixedValue)
        .benefit_milli(benefit_milli)
        .fixed_value_milli(fixed_value_milli)
        .expected_duration_ms(expected);
    reason_with(
        outcome,
        format!(
            "benefit_milli={benefit_milli} fixed_value_milli={fixed_value_milli} \
             expected_duration_ms={expected}"
        ),
    )
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
