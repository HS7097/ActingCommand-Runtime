// SPDX-License-Identifier: AGPL-3.0-only

//! SS-4 (register §1.231; design `rd-1029/design-sch-state.md` §4.7; Alice's ruling 105 A: no
//! count-based last line, the no-progress guard and the failure retry cap replace counts;
//! `contracts/scheduling/README.md`, "No-progress guard"): after a successful run whose basis got
//! no reading of another value, the (task, instance) pair waits for one.
//!
//! - **The basis** of a dispatch is the pools, stocks, remaining counts and completion times its
//!   admission was decided on (design §4.7), as the evaluation that chose the dispatch held them:
//!   the facts of the predicates that made its trigger true (every child of a true `all`; the
//!   first true child of an `any`, as the evaluation reads them; below a `not`, every fact of the
//!   negated predicate), of which each `resource.` or `inventory.` fact (the families recognition
//!   writes) a `fact` or `record_deadline` names, a fact reference's too, each fact a
//!   `fact_time_reached` names, and the reading, capacity and comparand facts of each pool a
//!   `resource_projection` projects; and the reading and capacity facts of each pool the task
//!   reserves (the reserve gate reads them at every admission). A fact of another family (a lock,
//!   a health flag) is no basis key, and neither is a pool the task only consumes or produces. A
//!   dispatch whose trigger was decided by a clock, a timeline, an outcome or a dependency alone,
//!   or whose trigger is not true as captured (an immediate rerun, a remedy request), has no basis
//!   and no guard: those are edges of their own; one with more than [`MAX_NO_PROGRESS_BASIS_KEYS`]
//!   keys arms none.
//! - **Armed.** The Runtime captures the basis of each new intent ([`NoProgressMemory::capture`])
//!   and arms the pair's guard when that intent is admitted ([`NoProgressMemory::admit`]),
//!   replacing the pair's earlier one.
//! - **Released.** At each evaluation ([`NoProgressMemory::observe`]) a guard one of whose basis
//!   keys has a reading whose value differs from the admission's (a reading where there was none
//!   counts) is released for good: a later reading that brings the old value back does not arm it
//!   again. A newer reading of the same value is no progress; only values are compared. A key
//!   with no reading now (a reset cleared it) releases nothing.
//! - **Held.** A pair whose guard is armed, whose latest run is the guarded dispatch and
//!   succeeded, and whose catalog generation is the one the dispatch pinned is deferred with
//!   `policy_no_progress` and no wake ([`NoProgressGuards`]). A run that failed or was cancelled
//!   is never held: failures keep their retry and escalation path.
//!
//! Memory only: nothing here enters a snapshot, a decision identity, a ledger record or a state
//! file, and a start begins with none.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::outcome::{LIST_MAX_ITEMS, Outcome, Record};
use sha2::{Digest, Sha256};

use super::{
    DecisionReason, DispatchIntent, EvaluationFacts, EvaluationResources, EvaluationTime,
    FACT_REFRESH_TASK_ID, PolicyEvaluationResult, PoolReading, PredicateTruth, TaskRuntimeSnapshot,
    evaluate_predicate, evaluation_pool_values, project_time_validity, reason_with,
};
use crate::codes::PolicyCode;
use crate::{
    CompiledCatalog, FactComparand, FactValue, ObservationRef, PoolCapacity, PoolSpec,
    PredicateSpec, ProjectionComparand, ScopeSelector, TaskSpec, TaskTerminalState, TimelineEvent,
};

/// SS-4: the most basis keys one guard holds, within the bound of a records value
/// (`LIST_MAX_ITEMS`): a rule whose admission reads more arms no guard, so `policy_no_progress`
/// always lists every key it waits on, and one reason stays a few kilobytes at most.
pub const MAX_NO_PROGRESS_BASIS_KEYS: usize = 32;
const _: () = assert!(MAX_NO_PROGRESS_BASIS_KEYS <= LIST_MAX_ITEMS);

/// SS-4: the longest canonical JSON of a reading a `policy_no_progress` record writes as its
/// `reading_value` (within the `name` bound of the key table, 256 bytes); a longer one is written
/// as its SHA-256, `reading_digest`.
const MAX_READING_VALUE_BYTES: usize = 128;

/// SS-4: one basis key of a dispatch and the reading the evaluation that chose it held.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasisReading {
    pub scope: ScopeSelector,
    pub fact_key: String,
    /// The value read; `None` when the key had no reading (none, confidence 0, or cleared by a
    /// reset).
    pub value: Option<FactValue>,
    /// When that reading was observed; `None` with `value`.
    pub observed_at_unix_ms: Option<u64>,
}

/// SS-4: the basis of one dispatch intent, captured as the evaluation that chose it held its facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoProgressBasis {
    pub decision_id: String,
    pub task_id: String,
    pub instance_id: String,
    /// The catalog generation the intent pinned: a guard holds only under it.
    pub catalog_hash: String,
    /// One reading per basis key, ordered by scope and key; never empty.
    pub readings: Vec<BasisReading>,
}

/// SS-4: the guard of one pair: the basis of its admitted dispatch, admitted at
/// `dispatched_at_unix_ms` (the dispatch time the pair's task state carries).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoProgressGuard {
    pub dispatched_at_unix_ms: u64,
    pub basis: NoProgressBasis,
}

/// SS-4: the armed guards one evaluation applies, by (task id, instance alias). The Runtime passes
/// them when it evaluates a cycle ([`super::evaluate_with_no_progress`]); every other entry point
/// has none, and with none the evaluation is the one before SS-4.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NoProgressGuards {
    guards: BTreeMap<(String, String), NoProgressGuard>,
}

impl NoProgressGuards {
    /// Adds `guard` for its pair, replacing the pair's earlier one.
    pub fn insert(&mut self, guard: NoProgressGuard) {
        self.guards.insert(
            (guard.basis.task_id.clone(), guard.basis.instance_id.clone()),
            guard,
        );
    }

    pub fn is_empty(&self) -> bool {
        self.guards.is_empty()
    }

    pub fn len(&self) -> usize {
        self.guards.len()
    }

    /// The reason `task_id` on `instance_id` is held, when its guard holds it: the guard's
    /// catalog generation is `catalog_hash`, the pair's latest run (`last_result`, its own last
    /// result, which no reset clears) is the guarded dispatch and succeeded, and no basis key of
    /// `facts` (the facts as the resets leave them) reads a value other than at admission.
    pub(super) fn hold(
        &self,
        task_id: &str,
        instance_id: &str,
        last_result: Option<&TaskRuntimeSnapshot>,
        facts: &EvaluationFacts,
        catalog_hash: &str,
    ) -> Option<DecisionReason> {
        if self.guards.is_empty() {
            return None;
        }
        let guard = self
            .guards
            .get(&(task_id.to_owned(), instance_id.to_owned()))?;
        let guarded_run = last_result.is_some_and(|state| {
            state.terminal_state == Some(TaskTerminalState::Succeeded)
                && state.last_dispatched_unix_ms == Some(guard.dispatched_at_unix_ms)
        });
        if guard.basis.catalog_hash != catalog_hash
            || !guarded_run
            || guard.basis.readings.is_empty()
            || changed(&guard.basis.readings, facts)
        {
            return None;
        }
        Some(no_progress_reason(guard))
    }
}

/// SS-4: the Runtime's memory of the guard, per (task id, instance alias): the basis of the
/// pair's latest intent not admitted yet, and its armed guard. At most one of each per pair.
/// Memory only; a start begins with none, so after a start each pair may run once more before
/// its guard is armed again.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NoProgressMemory {
    pending: BTreeMap<(String, String), NoProgressBasis>,
    armed: BTreeMap<(String, String), NoProgressGuard>,
}

impl NoProgressMemory {
    /// Captures the basis of each of `intents` (an evaluation's new dispatch intents) whose
    /// instance `enabled` names, from the evaluation's inputs `facts` and `resources` at `time`,
    /// read as the evaluation reads them (the facts as the resets leave them). An intent replaces
    /// the pair's earlier pending basis. A dispatch with no basis key or more than
    /// [`MAX_NO_PROGRESS_BASIS_KEYS`], the Runtime's fact refresh and a task the catalog does not
    /// hold capture none, and the pair's pending basis is dropped.
    pub fn capture(
        &mut self,
        catalog: &CompiledCatalog,
        facts: &EvaluationFacts,
        resources: &EvaluationResources,
        time: EvaluationTime,
        intents: &[DispatchIntent],
        enabled: impl Fn(&str) -> bool,
    ) -> PolicyEvaluationResult<()> {
        let intents = intents
            .iter()
            .filter(|intent| enabled(intent.instance_id.as_str()))
            .collect::<Vec<_>>();
        if intents.is_empty() {
            return Ok(());
        }
        let effective = project_time_validity(catalog, facts, time)?;
        let bundle = catalog.catalog();
        let pool_specs: BTreeMap<&str, &PoolSpec> = bundle
            .pools
            .pools
            .iter()
            .map(|pool| (pool.id.as_str(), pool))
            .collect();
        let pool_values = evaluation_pool_values(&pool_specs, resources, &effective, time);
        let timeline_events: BTreeMap<&str, &TimelineEvent> = bundle
            .timeline
            .events
            .iter()
            .map(|event| (event.id.as_str(), event))
            .collect();
        for intent in intents {
            let pair = (intent.task_id.clone(), intent.instance_id.clone());
            self.pending.remove(&pair);
            let Some(task) = bundle
                .tasks
                .tasks
                .iter()
                .find(|task| task.id == intent.task_id && task.id != FACT_REFRESH_TASK_ID)
            else {
                continue;
            };
            let context = Decision {
                // Workflow #411 P19: the task's own last result, as the evaluation reads it.
                task_state: facts.tasks.iter().find(|state| {
                    state.task_id == intent.task_id && state.instance_id == intent.instance_id
                }),
                scope: ScopeSelector::Instance {
                    instance_id: intent.instance_id.clone(),
                },
                facts: &effective,
                pool_specs: &pool_specs,
                pool_values: &pool_values,
                time,
                timeline_events: &timeline_events,
            };
            let Some(keys) = basis_keys(&context, task)? else {
                continue;
            };
            if keys.is_empty() || keys.len() > MAX_NO_PROGRESS_BASIS_KEYS {
                continue;
            }
            let readings = keys
                .into_iter()
                .map(|(scope, fact_key)| {
                    let reading = current_reading(&effective, &scope, &fact_key);
                    BasisReading {
                        value: reading.map(|(value, _)| value.clone()),
                        observed_at_unix_ms: reading.map(|(_, observed_at)| observed_at),
                        scope,
                        fact_key,
                    }
                })
                .collect();
            self.pending.insert(
                pair,
                NoProgressBasis {
                    decision_id: intent.decision_id.clone(),
                    task_id: intent.task_id.clone(),
                    instance_id: intent.instance_id.clone(),
                    catalog_hash: intent.catalog_hash.clone(),
                    readings,
                },
            );
        }
        Ok(())
    }

    /// The dispatch `decision_id` of `task_id` on `instance_id` was admitted at
    /// `admitted_at_unix_ms`: the pair's guard is armed with its captured basis, replacing the
    /// pair's earlier guard. A dispatch whose basis was not captured (another intent of the pair
    /// replaced it, its rule has none, the guard is off on its instance) leaves the pair without
    /// a guard.
    pub fn admit(
        &mut self,
        task_id: &str,
        instance_id: &str,
        decision_id: &str,
        admitted_at_unix_ms: u64,
    ) {
        let pair = (task_id.to_owned(), instance_id.to_owned());
        self.armed.remove(&pair);
        let Some(basis) = self.pending.remove(&pair) else {
            return;
        };
        if basis.decision_id != decision_id {
            self.pending.insert(pair, basis);
            return;
        }
        self.armed.insert(
            pair,
            NoProgressGuard {
                dispatched_at_unix_ms: admitted_at_unix_ms,
                basis,
            },
        );
    }

    /// Forgets every pair of an instance `enabled` does not name (turning the guard off and on
    /// releases it), then releases every armed guard one of whose basis keys reads, in `facts` as
    /// the resets leave them at `time`, a value other than at admission. A released guard is
    /// gone: a later reading of the old value does not arm it again.
    pub fn observe(
        &mut self,
        catalog: &CompiledCatalog,
        facts: &EvaluationFacts,
        time: EvaluationTime,
        enabled: impl Fn(&str) -> bool,
    ) -> PolicyEvaluationResult<()> {
        self.pending
            .retain(|(_, instance_id), _| enabled(instance_id.as_str()));
        self.armed
            .retain(|(_, instance_id), _| enabled(instance_id.as_str()));
        if self.armed.is_empty() {
            return Ok(());
        }
        let facts = project_time_validity(catalog, facts, time)?;
        self.armed
            .retain(|_, guard| !changed(&guard.basis.readings, &facts));
        Ok(())
    }

    /// The armed guards, for the evaluation.
    pub fn guards(&self) -> NoProgressGuards {
        NoProgressGuards {
            guards: self.armed.clone(),
        }
    }

    /// The armed guard of `task_id` on `instance_id`, if any.
    pub fn armed(&self, task_id: &str, instance_id: &str) -> Option<&NoProgressGuard> {
        self.armed
            .get(&(task_id.to_owned(), instance_id.to_owned()))
    }
}

/// SS-4: whether one of `readings` has a reading in `facts` whose value differs from the one
/// captured (a reading where none was counts; no reading now counts as none).
fn changed(readings: &[BasisReading], facts: &EvaluationFacts) -> bool {
    readings.iter().any(|reading| {
        current_reading(facts, &reading.scope, &reading.fact_key)
            .is_some_and(|(value, _)| reading.value.as_ref() != Some(value))
    })
}

/// The reading of (`scope`, `fact_key`) in `facts`, as every predicate reads one: present and
/// above confidence 0. Its value and observation time.
fn current_reading<'a>(
    facts: &'a EvaluationFacts,
    scope: &ScopeSelector,
    fact_key: &str,
) -> Option<(&'a FactValue, u64)> {
    facts
        .facts
        .iter()
        .find(|fact| fact.scope == *scope && fact.fact_key == fact_key)
        .filter(|fact| fact.confidence_milli > 0)
        .map(|fact| (&fact.value, fact.observed_at_unix_ms))
}

/// SS-4: what the evaluation that chose a dispatch read its trigger with, to evaluate each part
/// of it again exactly as it did.
struct Decision<'a> {
    task_state: Option<&'a TaskRuntimeSnapshot>,
    scope: ScopeSelector,
    facts: &'a EvaluationFacts,
    pool_specs: &'a BTreeMap<&'a str, &'a PoolSpec>,
    pool_values: &'a BTreeMap<&'a str, PoolReading<'a>>,
    time: EvaluationTime,
    timeline_events: &'a BTreeMap<&'a str, &'a TimelineEvent>,
}

impl Decision<'_> {
    /// Whether `predicate` is true, as the evaluation reads it.
    fn holds(&self, predicate: &PredicateSpec) -> PolicyEvaluationResult<bool> {
        Ok(evaluate_predicate(
            predicate,
            self.task_state,
            &self.scope,
            self.facts,
            self.pool_specs,
            self.pool_values,
            self.time,
            self.timeline_events,
            &mut Vec::new(),
        )?
        .truth
            == PredicateTruth::True)
    }
}

/// SS-4: the basis keys of `task`'s dispatch (module doc), each (scope, key) once, ordered by
/// scope and key; `None` when its trigger is not true as `decision` reads it.
fn basis_keys(
    decision: &Decision<'_>,
    task: &TaskSpec,
) -> PolicyEvaluationResult<Option<Vec<(ScopeSelector, String)>>> {
    let mut keys = BTreeMap::<(String, String), ScopeSelector>::new();
    let mut add = |scope: &ScopeSelector, fact_key: &str| {
        keys.entry((super::scope_key(scope), fact_key.to_owned()))
            .or_insert_with(|| scope.clone());
    };
    let mut pools = PoolKeys::default();
    if !decided_by(&task.trigger, decision, &mut add, &mut pools)? {
        return Ok(None);
    }
    pools.read.extend(
        task.reserves
            .iter()
            .flatten()
            .map(|reserve| reserve.pool_id.as_str()),
    );
    for pool in decision.pool_specs.values() {
        if pools.read.contains(pool.id.as_str()) {
            if let ObservationRef::Fact { fact_key } = &pool.observation {
                add(&pool.scope, fact_key.as_str());
            }
            if let PoolCapacity::Fact(reference) = &pool.capacity {
                add(&pool.scope, reference.fact_key.as_str());
            }
        }
        // #386 CF-5: a projection's comparand fact is read in the pool's own scope.
        for (pool_id, fact_key) in &pools.references {
            if *pool_id == pool.id {
                add(&pool.scope, fact_key);
            }
        }
    }
    Ok(Some(
        keys.into_iter()
            .map(|((_, fact_key), scope)| (scope, fact_key))
            .collect(),
    ))
}

/// SS-4: whether `predicate` is true as `decision` reads it, and, when it is, the facts that made
/// it true through `add` and the pools it projects into `pools`: every child of an `all`, the
/// first true child of an `any` (in the evaluation's order), and below a `not` every fact of the
/// negated predicate. A clock, a timeline, an outcome and a dependency add nothing.
fn decided_by<'a>(
    predicate: &'a PredicateSpec,
    decision: &Decision<'_>,
    add: &mut impl FnMut(&ScopeSelector, &str),
    pools: &mut PoolKeys<'a>,
) -> PolicyEvaluationResult<bool> {
    if !decision.holds(predicate)? {
        return Ok(false);
    }
    match predicate {
        PredicateSpec::All { predicates } => {
            for child in predicates {
                decided_by(child, decision, &mut *add, &mut *pools)?;
            }
        }
        PredicateSpec::Any { predicates } => {
            for child in predicates {
                if decided_by(child, decision, &mut *add, &mut *pools)? {
                    break;
                }
            }
        }
        PredicateSpec::Not { predicate } => predicate_keys(predicate, add, pools),
        leaf => predicate_keys(leaf, add, pools),
    }
    Ok(true)
}

/// Whether `fact_key` is of a family recognition writes (`resource.`, `inventory.`): a pool, stock
/// or remaining count. A fact of another family (a lock, a health flag, a session value) is no
/// progress a run makes, so it is no basis key: a rule gated by one beside its clock keeps no guard.
fn realistic(fact_key: &str) -> bool {
    super::REALISTIC_FACT_PREFIXES
        .iter()
        .any(|prefix| fact_key.starts_with(prefix))
}

/// The pools a task's predicates read: each pool a `resource_projection` projects, and each
/// (pool, fact) a projection compares with (#386 CF-5).
#[derive(Default)]
struct PoolKeys<'a> {
    read: BTreeSet<&'a str>,
    references: Vec<(&'a str, &'a str)>,
}

/// The facts `predicate` names, through `add`, and the pools it projects, into `pools`.
fn predicate_keys<'a>(
    predicate: &'a PredicateSpec,
    add: &mut impl FnMut(&ScopeSelector, &str),
    pools: &mut PoolKeys<'a>,
) {
    match predicate {
        PredicateSpec::All { predicates } | PredicateSpec::Any { predicates } => {
            for child in predicates {
                predicate_keys(child, &mut *add, &mut *pools);
            }
        }
        PredicateSpec::Not { predicate } => predicate_keys(predicate, add, pools),
        PredicateSpec::Fact {
            scope,
            fact_key,
            value,
            ..
        } => {
            if realistic(fact_key) {
                add(scope, fact_key.as_str());
            }
            if let FactComparand::Fact(reference) = value
                && realistic(&reference.fact_key)
            {
                add(scope, reference.fact_key.as_str());
            }
        }
        PredicateSpec::RecordDeadline {
            scope, fact_key, ..
        } => {
            if realistic(fact_key) {
                add(scope, fact_key.as_str());
            }
        }
        // A completion time, whatever its family.
        PredicateSpec::FactTimeReached {
            scope, fact_key, ..
        } => add(scope, fact_key.as_str()),
        PredicateSpec::ResourceProjection { pool_id, value, .. } => {
            pools.read.insert(pool_id.as_str());
            if let ProjectionComparand::Fact(reference) = value {
                pools
                    .references
                    .push((pool_id.as_str(), reference.fact_key.as_str()));
            }
        }
        PredicateSpec::Clock { .. }
        | PredicateSpec::TimelineActive { .. }
        | PredicateSpec::DependencyCompleted { .. }
        | PredicateSpec::Outcome { .. } => {}
    }
}

/// The `policy_no_progress` reason of a held pair: its basis keys with the admission's readings,
/// and the guarded run's dispatch.
fn no_progress_reason(guard: &NoProgressGuard) -> DecisionReason {
    let records = guard.basis.readings.iter().map(|reading| {
        let mut record = Record::new().fact_key(reading.fact_key.as_str());
        if let Some(value) = &reading.value {
            let text = serde_json::to_string(value).unwrap_or_default();
            record = if !text.is_empty() && text.len() <= MAX_READING_VALUE_BYTES {
                record.reading_value(text)
            } else {
                record.reading_digest(format!("sha256:{:x}", Sha256::digest(text.as_bytes())))
            };
        }
        if let Some(observed_at) = reading.observed_at_unix_ms {
            record = record.observed_at_unix_ms(observed_at);
        }
        record
    });
    let outcome = Outcome::new(PolicyCode::PolicyNoProgress)
        .basis_readings(records)
        .served_by_dispatch_unix_ms(guard.dispatched_at_unix_ms);
    reason_with(
        outcome,
        format!(
            "the run dispatched at {} succeeded and none of its {} basis keys has read a value other than at its admission since",
            guard.dispatched_at_unix_ms,
            guard.basis.readings.len()
        ),
    )
}
