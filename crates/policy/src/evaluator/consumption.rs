// SPDX-License-Identifier: AGPL-3.0-only

//! SCHED-C1f1 (register §1.303 SC1F-2 to SC1F-4; the coordinator's rulings of 11-11 14:5x on the
//! nine questions of §1.303, not Alice's words; `contracts/scheduling/README.md`, "A run's
//! consumption read from a fact"): the amount in effect of each `consumes` entry whose `amount` is
//! read from a fact, resolved once per evaluation and instance, and what a person sees of it.
//!
//! - **The one resolution** ([`resolve`]). At each evaluation, for each task whose `consumes`
//!   writes the form and each instance its scope covers, each such entry is read once from the
//!   fact snapshot the evaluation pins, as the time-validity projection leaves it: the fact B in
//!   exactly the instance's scope, never a server's or a game's. A usable record (present, not past
//!   its own expiry and not invalidated by a reset since, which the projection removed; a
//!   confidence above 0; an `integer` in `1..=2^53−1` and at most a constant capacity of the pool)
//!   gives its value, anything else the entry's fallback F (ruling 1). The amount is never
//!   Unknown: B raises no fact refresh demand and gives no detection suggestion.
//! - **Every reader reads that one value**, through [`Amounts`]: the net benefit's cost term and
//!   `ε` (`benefit::judge`), the pool's lines and the capped spending order (`capped`), the
//!   reserves, their selection fields and SS-6's spend values (`resource_targets`), the tie-break
//!   (`tie_break`) and the forward projection, which reads it once at its start and holds it
//!   across its horizon. [`Amounts::Declared`] reads every fact-driven amount at F: the consumers
//!   key of a capped episode and who is a consumer (ruling 2), and the compiler's warning
//!   `pool_lines_unspendable` (ruling 8).
//! - **Freshness and wakes** (ruling 1, by every fact's rule). A usable record's expiry, which the
//!   projection caps before a reset that will invalidate the key, bounds the freshness of every
//!   candidate of its instance whose task consumes or reserves its pool
//!   ([`ConsumptionAmounts::fresh_until`]), and the evaluation wakes a millisecond after it
//!   ([`ConsumptionAmounts::next_wake`]), when it reads F. F bounds nothing and wakes nothing.
//! - **What a person sees** (SC1F-4). A record that is present and current but not usable gives
//!   the warning `consumption_fact_unusable`, the first reason of each decision of the task on the
//!   instance ([`ConsumptionAmounts::warnings`]); the reasons of a pool the task's consumption of
//!   which is fact-driven carry `consumption` and `consumption_source` ([`with_values`]).

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::outcome::Outcome;
use actingcommand_selection_policy::UnknownReason;

use super::{
    DecisionReason, EvaluationFacts, EvaluationTime, PolicyEvaluationError, PolicyEvaluationResult,
    UnknownReasonToken, reason_with, scope_matches_instance, unknown_token,
};
use crate::codes::{ConsumptionSource, PolicyCode};
use crate::{ConsumptionFact, FactValue, MAX_CANONICAL_INTEGER, PoolSpec, ScopeSelector, TaskSpec};

/// One fact-driven entry's amount on one instance at one evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Resolved {
    /// The entry's index in its task's `consumes`.
    index: usize,
    pool_id: String,
    fact_key: String,
    /// The amount in effect: the usable record's value, else the fallback.
    amount: u64,
    source: ConsumptionSource,
    /// The usable record's expiry as the time-validity projection leaves it; `None` for the
    /// fallback and for a usable record without one.
    fresh_until: Option<u64>,
    /// Why a record that is present and current is not usable (`fact_low_confidence`, or
    /// `type_mismatch` for a value of another kind, a record list or an integer out of range).
    unusable: Option<UnknownReason>,
}

/// The fact-driven entries of one task on one instance, and, per pool one of them consumes, the
/// task's whole consumption of it with every entry in effect.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PairAmounts {
    entries: Vec<Resolved>,
    /// `⌈C/1000⌉` of the task's consumption of each pool of a fact-driven entry.
    consumptions: BTreeMap<String, u64>,
}

/// SCHED-C1f1: the amounts in effect of one evaluation, by task id and then instance alias. Empty
/// for a catalog that writes no consumption read from a fact, so every reader reads as before.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ConsumptionAmounts {
    pairs: BTreeMap<String, BTreeMap<String, PairAmounts>>,
}

/// Which amount a reader of a consumption takes of a fact-driven `consumes` entry.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Amounts<'a> {
    /// The catalog's declared consumption: every fact-driven amount at its fallback F.
    Declared,
    /// The amounts in effect on the instance `instance_id` at the evaluation that resolved
    /// `resolved`.
    InEffect {
        resolved: &'a ConsumptionAmounts,
        instance_id: &'a str,
    },
}

impl Amounts<'_> {
    /// The amount of the entry `index` of the `consumes` of `task_id`, which writes `form`.
    pub(crate) fn of(&self, task_id: &str, index: usize, form: &ConsumptionFact) -> u64 {
        match self {
            Self::Declared => form.fallback,
            Self::InEffect {
                resolved,
                instance_id,
            } => resolved
                .pair(task_id, instance_id)
                .and_then(|pair| pair.entries.iter().find(|entry| entry.index == index))
                .map_or(form.fallback, |entry| entry.amount),
        }
    }
}

impl ConsumptionAmounts {
    /// The amounts in effect on the instance `instance_id`.
    pub(crate) fn on<'a>(&'a self, instance_id: &'a str) -> Amounts<'a> {
        Amounts::InEffect {
            resolved: self,
            instance_id,
        }
    }

    fn pair(&self, task_id: &str, instance_id: &str) -> Option<&PairAmounts> {
        self.pairs.get(task_id)?.get(instance_id)
    }

    /// SC1F-4: the values `consumption` (`⌈C/1000⌉` in effect) and `consumption_source` of the
    /// consumption of `pool_id` by `task_id` on `instance_id`, when one of its entries of the pool
    /// is read from a fact; `None` for a constant consumption, which carries neither.
    pub(crate) fn values(
        &self,
        task_id: &str,
        instance_id: &str,
        pool_id: &str,
    ) -> Option<(u64, ConsumptionSource)> {
        let pair = self.pair(task_id, instance_id)?;
        let consumption = *pair.consumptions.get(pool_id)?;
        let entry = pair.entries.iter().find(|entry| entry.pool_id == pool_id)?;
        Some((consumption, entry.source))
    }

    /// SC1F-4: one `consumption_fact_unusable` per pool of `task_id` on `instance_id` whose record
    /// is present and current but not usable (warning; `task_id`, `pool_id`, `fact_key`,
    /// `unknown_reason`, `consumption` the `⌈C/1000⌉` the decision reads with the fallback), in
    /// the order of the task's `consumes`: the first reasons of each of its decisions there.
    pub(crate) fn warnings(&self, task_id: &str, instance_id: &str) -> Vec<DecisionReason> {
        let Some(pair) = self.pair(task_id, instance_id) else {
            return Vec::new();
        };
        pair.entries
            .iter()
            .filter_map(|entry| {
                let unknown_reason = entry.unusable?;
                let consumption = pair
                    .consumptions
                    .get(&entry.pool_id)
                    .copied()
                    .unwrap_or(entry.amount);
                let outcome = Outcome::new(PolicyCode::ConsumptionFactUnusable)
                    .task_id(task_id)
                    .pool_id(entry.pool_id.as_str())
                    .fact_key(entry.fact_key.as_str())
                    .unknown_reason(UnknownReasonToken(unknown_reason))
                    .consumption(consumption);
                Some(reason_with(
                    outcome,
                    format!(
                        "task_id={task_id} pool_id={} fact_key={} unknown_reason={} \
                         consumption={consumption}: the fallback is read",
                        entry.pool_id,
                        entry.fact_key,
                        unknown_token(unknown_reason)
                    ),
                ))
            })
            .collect()
    }

    /// Ruling 1: the earliest expiry of the usable records of `instance_id` whose pool `task`
    /// consumes or reserves, which bounds the freshness of its candidate there; `None` when none
    /// has one.
    pub(crate) fn fresh_until(&self, task: &TaskSpec, instance_id: &str) -> Option<u64> {
        if self.pairs.is_empty() {
            return None;
        }
        let touched = task
            .consumes
            .iter()
            .map(|effect| effect.pool_id.as_str())
            .chain(
                task.reserves
                    .iter()
                    .flatten()
                    .map(|reserve| reserve.pool_id.as_str()),
            )
            .collect::<BTreeSet<_>>();
        self.pairs
            .values()
            .filter_map(|instances| instances.get(instance_id))
            .flat_map(|pair| &pair.entries)
            .filter(|entry| touched.contains(entry.pool_id.as_str()))
            .filter_map(|entry| entry.fresh_until)
            .min()
    }

    /// Ruling 1: a millisecond after the earliest expiry of a usable record, when the evaluation
    /// reads the fallback instead; `None` when no usable record has one.
    pub(crate) fn next_wake(&self) -> PolicyEvaluationResult<Option<u64>> {
        self.pairs
            .values()
            .flat_map(BTreeMap::values)
            .flat_map(|pair| &pair.entries)
            .filter_map(|entry| entry.fresh_until)
            .min()
            .map(|expiry| {
                expiry.checked_add(1).ok_or_else(|| {
                    PolicyEvaluationError::overflow(
                        "a consumption fact's expiry has no following Unix millisecond",
                    )
                })
            })
            .transpose()
    }
}

/// SC1F-2: the one resolution of an evaluation (module doc). `facts` are the facts as the
/// time-validity projection leaves them at `time`; `pool_specs` the catalog's pools, for a
/// constant capacity. A catalog that writes no form resolves nothing.
pub(crate) fn resolve(
    tasks: &[TaskSpec],
    pool_specs: &BTreeMap<&str, &PoolSpec>,
    facts: &EvaluationFacts,
    time: EvaluationTime,
) -> PolicyEvaluationResult<ConsumptionAmounts> {
    let mut resolved = ConsumptionAmounts::default();
    for task in tasks {
        let forms = task
            .consumes
            .iter()
            .enumerate()
            .filter_map(|(index, effect)| {
                effect
                    .fact_amount()
                    .map(|form| (index, effect.pool_id.as_str(), form))
            })
            .collect::<Vec<_>>();
        if forms.is_empty() {
            continue;
        }
        for instance in facts
            .instances
            .iter()
            .filter(|instance| scope_matches_instance(&task.scope, instance))
        {
            let scope = ScopeSelector::Instance {
                instance_id: instance.instance_id.clone(),
            };
            let mut pair = PairAmounts {
                entries: forms
                    .iter()
                    .map(|&(index, pool_id, form)| {
                        let capacity = pool_specs
                            .get(pool_id)
                            .and_then(|pool| pool.capacity.constant());
                        read(index, pool_id, form, (&scope, capacity), facts, time)
                    })
                    .collect(),
                consumptions: BTreeMap::new(),
            };
            for &(_, pool_id, _) in &forms {
                let milli =
                    crate::resource_targets::consumption_milli_by(task, pool_id, |index, form| {
                        pair.entries
                            .iter()
                            .find(|entry| entry.index == index)
                            .map_or(form.fallback, |entry| entry.amount)
                    })?;
                let units = u64::try_from(milli.div_ceil(1_000)).map_err(|_| {
                    PolicyEvaluationError::overflow(format!(
                        "task '{}' consumes more of pool '{pool_id}' than the integer range",
                        task.id
                    ))
                })?;
                pair.consumptions.insert(pool_id.to_owned(), units);
            }
            resolved
                .pairs
                .entry(task.id.clone())
                .or_default()
                .insert(instance.instance_id.clone(), pair);
        }
    }
    Ok(resolved)
}

/// One fact-driven entry on one instance (module doc): the record of `form`'s key in `scope` (the
/// instance's), usable or not, against the pool's constant `capacity`.
fn read(
    index: usize,
    pool_id: &str,
    form: &ConsumptionFact,
    (scope, capacity): (&ScopeSelector, Option<u64>),
    facts: &EvaluationFacts,
    time: EvaluationTime,
) -> Resolved {
    let fallback = |unusable| Resolved {
        index,
        pool_id: pool_id.to_owned(),
        fact_key: form.fact_key.clone(),
        amount: form.fallback,
        source: ConsumptionSource::Fallback,
        fresh_until: None,
        unusable,
    };
    // None, or invalidated by a reset (the projection removed it): the fallback, no warning.
    let Some(fact) = facts
        .facts
        .iter()
        .find(|fact| fact.scope == *scope && fact.fact_key == form.fact_key)
    else {
        return fallback(None);
    };
    // Past its own expiry: the fallback, no warning.
    if fact
        .expires_at_unix_ms
        .is_some_and(|expiry| time.unix_ms > expiry)
    {
        return fallback(None);
    }
    // Present and current but not usable: the fallback, with the warning.
    if fact.confidence_milli == 0 {
        return fallback(Some(UnknownReason::FactLowConfidence));
    }
    let usable = match fact.value {
        FactValue::Integer(value) => u64::try_from(value).ok().filter(|value| {
            (1..=MAX_CANONICAL_INTEGER as u64).contains(value)
                && capacity.is_none_or(|capacity| *value <= capacity)
        }),
        _ => None,
    };
    match usable {
        Some(amount) => Resolved {
            amount,
            source: ConsumptionSource::Fact,
            fresh_until: fact.expires_at_unix_ms,
            ..fallback(None)
        },
        None => fallback(Some(UnknownReason::TypeMismatch)),
    }
}

/// SC1F-4: `reason` with the values `consumption` and `consumption_source` when `values` holds
/// them (a consumption read from a fact, [`ConsumptionAmounts::values`]); as it is otherwise.
pub(crate) fn with_values(
    mut reason: DecisionReason,
    values: Option<(u64, ConsumptionSource)>,
) -> DecisionReason {
    if let Some((consumption, source)) = values {
        reason.outcome = reason
            .outcome
            .consumption(consumption)
            .consumption_source(source);
        reason.detail.push_str(&format!(
            " consumption={consumption} consumption_source={}",
            source.as_str()
        ));
    }
    reason
}
