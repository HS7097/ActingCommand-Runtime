// SPDX-License-Identifier: AGPL-3.0-only

//! SS-2 (register §1.222; Alice's ruling 104 points 2, 4 and 5, approved 2026-10-30, Workflow
//! #386 comment 6523113979): chain successors rank with their chain head, and candidates the
//! ranking leaves equal are ordered by how close each is to losing its eligibility, then by the
//! number of conditions it depends on, then by declared order. Point 3 (net benefit) is not here:
//! the within-tier rank value is the one of before.
//!
//! - **A chain link** ([`required_predecessors`]) is a `dependency_completed` that accepts
//!   `succeeded` and is reached from the trigger's root only through `all`: the trigger cannot be
//!   true unless that task's latest run on the instance ended in an accepted state, so the task
//!   continues it. A dependency under an `any` is one alternative among others (an order, not a
//!   continuation) and under a `not` the opposite: neither is a link. A task that names itself is
//!   no link of its own.
//! - **The head's run window** ([`chain_head`]). A successor continues a predecessor on an
//!   instance while that predecessor's latest run there succeeded and the successor has not served
//!   it (`since_own_run`'s test, `dependency_served`). The head is found by walking up the links:
//!   a predecessor whose own latest dispatch is not before the latest successful run of one of its
//!   links continued that one, and so on. In that window the successor ranks with the head's
//!   priority and strategic weight (the head's instance override applied); outside it, with its
//!   own. Only the task layer reads this; no resource layer function takes task state. SS-2e
//!   (register §1.229): the walk also says whether a link it passed, the head included, is
//!   marked `benefit_judged`; a marked candidate is exempt from the net-benefit gate only then
//!   (`benefit.rs`, "Who is judged").
//! - **The tie-break** ([`TieKey`]): among candidates equal in promotion, total and affinity, a
//!   candidate whose measure is bounded goes first, the fewer chances first (the fewer of the runs
//!   its thresholds still allow and the runs that can still start before a window it requires
//!   closes), then the less time left, then the fewer runs left. A candidate with no bound, or one
//!   whose bound reads an unknown input, has no measure: the fallback orders it, more conditions
//!   first, then declared order (the task's position in the catalog, which for a composed catalog
//!   is the rule's position in `schedule.json`). Every input is one the gates read at the same
//!   instant; the arithmetic is integer, and nothing here fails the evaluation.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use super::{
    EvaluationTime, InstanceSnapshot, PlacementContext, PredicateTruth, TaskRuntimeSnapshot,
    dependency_served, evaluate_predicate, project_pool_value, referenced_integer,
};
use crate::{
    Comparison, FactComparand, FactReference, FactValue, ObservationRef, PredicateSpec,
    ProjectionComparand, ReserveSpec, ScopeSelector, TaskSpec, TaskTerminalState,
};

/// The conditions a trigger requires: every node reached from its root only through `all`, an
/// `all` itself excepted. An `any` or a `not` counts as one condition.
pub(super) fn required_conditions(trigger: &PredicateSpec) -> Vec<&PredicateSpec> {
    let mut found = Vec::new();
    collect_required(trigger, &mut found);
    found
}

fn collect_required<'a>(predicate: &'a PredicateSpec, found: &mut Vec<&'a PredicateSpec>) {
    match predicate {
        PredicateSpec::All { predicates } => {
            for predicate in predicates {
                collect_required(predicate, found);
            }
        }
        other => found.push(other),
    }
}

/// The chain links of a trigger: the tasks a required `dependency_completed` that accepts
/// `succeeded` names, in trigger order.
pub(super) fn required_predecessors(trigger: &PredicateSpec) -> Vec<&str> {
    required_conditions(trigger)
        .into_iter()
        .filter_map(|condition| match condition {
            PredicateSpec::DependencyCompleted {
                task_id,
                terminal_states,
                ..
            } if terminal_states.contains(&TaskTerminalState::Succeeded) => Some(task_id.as_str()),
            _ => None,
        })
        .collect()
}

/// What the walk up a candidate's chain links found ([`chain_head`]).
#[derive(Debug, Clone, Copy)]
pub(super) struct ChainWalk<'a> {
    /// The head the candidate ranks with.
    pub(super) head: &'a TaskSpec,
    /// SS-2e (register §1.229): a link the walk passed, the head included, is marked
    /// `benefit_judged`, so the candidate continues a business chain. Only the benefit gate's
    /// choice of who is judged reads it; the rank reads `head` alone.
    pub(super) business_link: bool,
}

/// The head `task` ranks with on `instance_id`, while it is in that head's run window (module
/// doc), and whether a link walked up to it is a business task; `None` outside every window.
/// `own` is the task's own last result, as its own gates read it; every other task is read from
/// `task_states`, as the gates read another task.
pub(super) fn chain_head<'a>(
    task: &TaskSpec,
    instance_id: &str,
    own: Option<&TaskRuntimeSnapshot>,
    tasks: &BTreeMap<&str, (usize, &'a TaskSpec)>,
    task_states: &BTreeMap<(&str, &str), &TaskRuntimeSnapshot>,
) -> Option<ChainWalk<'a>> {
    let mut visited = BTreeSet::from([task.id.clone()]);
    // The predecessor whose latest successful run the task has not served yet.
    let (mut head, mut head_at) = latest_success(
        required_predecessors(&task.trigger),
        &visited,
        instance_id,
        tasks,
        task_states,
        |state, _| own.is_none_or(|own| !dependency_served(own, state)),
    )?;
    let mut business_link = head.is_benefit_judged();
    loop {
        visited.insert(head.id.clone());
        let limit = head_at;
        // A link whose latest successful run the head's own latest dispatch came after.
        match latest_success(
            required_predecessors(&head.trigger),
            &visited,
            instance_id,
            tasks,
            task_states,
            |_, dispatched| dispatched <= limit,
        ) {
            Some((next, next_at)) => {
                head = next;
                head_at = next_at;
                business_link |= head.is_benefit_judged();
            }
            None => {
                return Some(ChainWalk {
                    head,
                    business_link,
                });
            }
        }
    }
}

/// Of `links` not yet `visited`, the one whose latest run on `instance_id` succeeded and that
/// `admits` (given its snapshot and that run's dispatch) with the latest dispatch; of two at one
/// instant the smaller id.
fn latest_success<'a>(
    links: Vec<&str>,
    visited: &BTreeSet<String>,
    instance_id: &str,
    tasks: &BTreeMap<&str, (usize, &'a TaskSpec)>,
    task_states: &BTreeMap<(&str, &str), &TaskRuntimeSnapshot>,
    admits: impl Fn(&TaskRuntimeSnapshot, u64) -> bool,
) -> Option<(&'a TaskSpec, u64)> {
    let mut found: Option<(&'a TaskSpec, u64)> = None;
    for link in links {
        if visited.contains(link) {
            continue;
        }
        let Some(&(_, spec)) = tasks.get(link) else {
            continue;
        };
        let Some(&state) = task_states.get(&(link, instance_id)) else {
            continue;
        };
        let Some(dispatched) = state.last_dispatched_unix_ms else {
            continue;
        };
        if state.terminal_state != Some(TaskTerminalState::Succeeded) || !admits(state, dispatched)
        {
            continue;
        }
        let later = found.is_none_or(|(best, best_at)| {
            dispatched > best_at || (dispatched == best_at && spec.id < best.id)
        });
        if later {
            found = Some((spec, dispatched));
        }
    }
    found
}

/// Whether a candidate's closeness to losing its eligibility could be measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Measure {
    /// Every input the measure reads is known, and a window or a threshold bounds it.
    Bounded,
    /// Every input is known, and no window it requires and no threshold its own consumption
    /// erodes bounds it.
    Unbounded,
    /// An input the measure reads is unknown: it cannot be computed.
    Unknown,
}

impl Measure {
    const fn token(self) -> &'static str {
        match self {
            Self::Bounded => "bounded",
            Self::Unbounded => "none",
            Self::Unknown => "unknown",
        }
    }
}

/// A candidate's tie-break key (module doc). Only a [`Measure::Bounded`] key carries its
/// numbers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TieKey {
    measure: Measure,
    /// The fewer of `runs_left` and the runs that can still start before `time_left_ms` ends.
    chances: Option<u64>,
    /// Milliseconds until a window the trigger requires closes.
    time_left_ms: Option<u64>,
    /// Runs, this one included, its thresholds still allow by its own declared consumption.
    runs_left: Option<u64>,
    /// The trigger's required conditions and the task's reserves.
    conditions: u32,
    /// The task's position in the catalog; `usize::MAX` for the Runtime's own task.
    declared: usize,
}

impl TieKey {
    /// The key of the recognition maintenance package's candidate, which no catalog declares: no
    /// measure, no condition, after every declared task.
    pub(super) const fn runtime_task() -> Self {
        Self {
            measure: Measure::Unknown,
            chances: None,
            time_left_ms: None,
            runs_left: None,
            conditions: 0,
            declared: usize::MAX,
        }
    }

    /// `Less` when `self` goes first.
    pub(super) fn order(&self, other: &Self) -> Ordering {
        let class = |key: &Self| u8::from(key.measure != Measure::Bounded);
        let bound = |value: Option<u64>| value.unwrap_or(u64::MAX);
        class(self)
            .cmp(&class(other))
            .then_with(|| bound(self.chances).cmp(&bound(other.chances)))
            .then_with(|| bound(self.time_left_ms).cmp(&bound(other.time_left_ms)))
            .then_with(|| bound(self.runs_left).cmp(&bound(other.runs_left)))
            .then_with(|| other.conditions.cmp(&self.conditions))
            .then_with(|| self.declared.cmp(&other.declared))
    }

    /// The key as the rank breakdown prints it.
    pub(super) fn detail(&self) -> String {
        let number =
            |value: Option<u64>| value.map_or_else(|| "none".to_owned(), |v| v.to_string());
        let declared = if self.declared == usize::MAX {
            "none".to_owned()
        } else {
            self.declared.to_string()
        };
        format!(
            "tie={} chances={} time_left_ms={} runs_left={} conditions={} declared={declared}",
            self.measure.token(),
            number(self.chances),
            number(self.time_left_ms),
            number(self.runs_left),
            self.conditions,
        )
    }
}

/// What bounds one candidate, gathered over its conditions.
#[derive(Default)]
struct Bounds {
    time_left_ms: Option<u64>,
    runs_left: Option<u64>,
    unknown: bool,
}

impl Bounds {
    fn window(&mut self, until: u64, time: EvaluationTime) {
        let left = until.saturating_sub(time.unix_ms);
        self.time_left_ms = Some(self.time_left_ms.map_or(left, |known| known.min(left)));
    }

    fn runs(&mut self, runs: u64) {
        self.runs_left = Some(self.runs_left.map_or(runs, |known| known.min(runs)));
    }

    /// A threshold `actual ≥ least` that each run lowers by `consumes`: the runs, this one
    /// included, that still meet it.
    fn threshold(&mut self, actual: i128, least: i128, consumes: u64) {
        let runs = if actual < least {
            0
        } else {
            (actual - least) / i128::from(consumes) + 1
        };
        self.runs(u64::try_from(runs).unwrap_or(u64::MAX));
    }
}

/// The tie-break key of `task`'s candidate on `instance` (module doc). `own` is the task's own
/// last result; `expected_duration_ms` the candidate's E; `declared` its catalog position.
pub(super) fn tie_key(
    task: &TaskSpec,
    own: Option<&TaskRuntimeSnapshot>,
    instance: &InstanceSnapshot,
    context: &PlacementContext<'_>,
    expected_duration_ms: u64,
    declared: usize,
) -> TieKey {
    let scope = ScopeSelector::Instance {
        instance_id: instance.instance_id.clone(),
    };
    let mut bounds = Bounds::default();
    let mut conditions = 0_u32;
    for condition in required_conditions(&task.trigger) {
        conditions = conditions.saturating_add(1);
        measure_condition(&mut bounds, condition, task, own, &scope, context);
    }
    for reserve in task.reserves.iter().flatten() {
        conditions = conditions.saturating_add(1);
        measure_reserve(&mut bounds, reserve, task, context);
    }
    let measure = if bounds.unknown {
        Measure::Unknown
    } else if bounds.time_left_ms.is_none() && bounds.runs_left.is_none() {
        Measure::Unbounded
    } else {
        Measure::Bounded
    };
    if measure != Measure::Bounded {
        return TieKey {
            measure,
            chances: None,
            time_left_ms: None,
            runs_left: None,
            conditions,
            declared,
        };
    }
    let starts = bounds
        .time_left_ms
        .map(|left| left.div_ceil(expected_duration_ms.max(1)));
    let chances = match (bounds.runs_left, starts) {
        (Some(runs), Some(starts)) => Some(runs.min(starts)),
        (runs, starts) => runs.or(starts),
    };
    TieKey {
        measure,
        chances,
        time_left_ms: bounds.time_left_ms,
        runs_left: bounds.runs_left,
        conditions,
        declared,
    }
}

/// The least value that keeps `comparison` true against `threshold`, for the two comparisons a
/// run's consumption can make false; `None` for every other one.
fn least_true(comparison: Comparison, threshold: i128) -> Option<i128> {
    match comparison {
        Comparison::GreaterThanOrEqual => Some(threshold),
        Comparison::GreaterThan => Some(threshold + 1),
        _ => None,
    }
}

fn measure_condition(
    bounds: &mut Bounds,
    condition: &PredicateSpec,
    task: &TaskSpec,
    own: Option<&TaskRuntimeSnapshot>,
    scope: &ScopeSelector,
    context: &PlacementContext<'_>,
) {
    match condition {
        // A window: an event's validity, or a clock occurrence's grace.
        PredicateSpec::TimelineActive { .. }
        | PredicateSpec::Clock {
            missed: Some(_), ..
        } => {
            let mut scratch = Vec::new();
            match evaluate_predicate(
                condition,
                own,
                scope,
                context.facts,
                context.pool_specs,
                context.pool_values,
                context.time,
                context.timeline_events,
                &mut scratch,
            ) {
                Ok(result) if result.truth == PredicateTruth::True => {
                    if let Some(until) = result.fresh_until_unix_ms {
                        bounds.window(until, context.time);
                    }
                }
                Ok(result) if result.truth == PredicateTruth::False => {}
                _ => bounds.unknown = true,
            }
        }
        PredicateSpec::ResourceProjection {
            pool_id,
            comparison,
            value,
        } => {
            let Some(consumes) = consumed(task, pool_id, bounds) else {
                return;
            };
            let Some(threshold) = (match value {
                ProjectionComparand::Constant(value) => Some(i128::from(*value)),
                ProjectionComparand::Fact(reference) => {
                    let Some(spec) = context.pool_specs.get(pool_id.as_str()) else {
                        bounds.unknown = true;
                        return;
                    };
                    read_integer(context, &spec.scope, reference, None, bounds)
                        .map(|referenced| referenced + i128::from(reference.offset))
                }
            }) else {
                return;
            };
            let Some(least) = least_true(*comparison, threshold) else {
                return;
            };
            if let Some(actual) = projected(context, pool_id, bounds) {
                bounds.threshold(actual, least, consumes);
            }
        }
        PredicateSpec::Fact {
            scope: fact_scope,
            fact_key,
            comparison,
            value,
            max_age_ms,
            ..
        } => {
            // A fact the task's own runs lower: the reading key of a pool it consumes.
            let Some(pool) = task.consumes.iter().find_map(|effect| {
                context
                    .pool_specs
                    .get(effect.pool_id.as_str())
                    .filter(|spec| {
                        spec.scope == *fact_scope
                            && matches!(
                                &spec.observation,
                                ObservationRef::Fact { fact_key: key } if key == fact_key
                            )
                    })
            }) else {
                return;
            };
            let Some(consumes) = consumed(task, &pool.id, bounds) else {
                return;
            };
            let threshold = match value {
                FactComparand::Value(FactValue::Integer(value)) => Some(i128::from(*value)),
                FactComparand::Value(_) => return,
                FactComparand::Fact(reference) => {
                    read_integer(context, fact_scope, reference, *max_age_ms, bounds)
                        .map(|referenced| referenced + i128::from(reference.offset))
                }
            };
            let Some(least) = threshold.and_then(|threshold| least_true(*comparison, threshold))
            else {
                return;
            };
            let own_key = FactReference {
                fact_key: fact_key.clone(),
                offset: 0,
                scope: None,
            };
            if let Some(actual) = read_integer(context, fact_scope, &own_key, *max_age_ms, bounds) {
                bounds.threshold(actual, least, consumes);
            }
        }
        _ => {}
    }
}

/// A reserve: the runs, this one included, that each leave at least `at_least` in the pool.
fn measure_reserve(
    bounds: &mut Bounds,
    reserve: &ReserveSpec,
    task: &TaskSpec,
    context: &PlacementContext<'_>,
) {
    let Some(consumes) = consumed(task, &reserve.pool_id, bounds) else {
        return;
    };
    if let Some(actual) = projected(context, &reserve.pool_id, bounds) {
        let least = i128::from(reserve.at_least) + i128::from(consumes);
        bounds.threshold(actual, least, consumes);
    }
}

/// The task's declared consumption of the pool, when its runs consume some: an expected
/// consumption rounded up to whole units (SS-2g), as the reserve gate reads it.
fn consumed(task: &TaskSpec, pool_id: &str, bounds: &mut Bounds) -> Option<u64> {
    match crate::resource_targets::declared_consumption(task, pool_id) {
        Ok(0) => None,
        Ok(consumes) => Some(consumes),
        Err(_) => {
            bounds.unknown = true;
            None
        }
    }
}

/// The pool's projected amount at the evaluation, as `resource_projection` reads it.
fn projected(context: &PlacementContext<'_>, pool_id: &str, bounds: &mut Bounds) -> Option<i128> {
    let amount = context
        .pool_specs
        .get(pool_id)
        .zip(context.pool_values.get(pool_id))
        .and_then(|(spec, reading)| {
            project_pool_value(spec, reading.bound, reading.snapshot, context.time.unix_ms).ok()
        });
    if amount.is_none() {
        bounds.unknown = true;
    }
    amount.map(i128::from)
}

/// The integer a fact holds in `scope`, read with `fact`'s rules.
fn read_integer(
    context: &PlacementContext<'_>,
    scope: &ScopeSelector,
    reference: &FactReference,
    max_age_ms: Option<u64>,
    bounds: &mut Bounds,
) -> Option<i128> {
    match referenced_integer(context.facts, scope, reference, max_age_ms, context.time) {
        Ok(Ok(referenced)) => Some(i128::from(referenced.value)),
        _ => {
            bounds.unknown = true;
            None
        }
    }
}
