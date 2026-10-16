// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #384 D2-S4 (`rd-1009/d2/model-d2-fact-refresh.md` v1.4.2 §3.3, §3.4, §3.7, §3.9,
//! §6 S4): the scheduler's fact refresh. A realistic instance fact (a `resource.` or
//! `inventory.` key, which recognition writes) that a candidate reads and that is unknown never
//! fails the round: the instance's recognition maintenance package R_I is dispatched to read it,
//! through the instance's one queue like every policy dispatch, and the candidate is deferred
//! under one registered reason per key until the reading lands and a later round scores it.
//!
//! - Demand comes from the candidates that passed the closure's positions 1-2 (an operator
//!   pause, a holder, a waiting claim or a takeover cooldown): on an instance held there the
//!   candidates carry that reason alone and nothing is collected (§3.4 step 1).
//! - R_I is requested for the unknown keys its marks write; it inherits the highest rank of the
//!   candidates it unlocks (their catalog rank and manual offset) and competes with the other
//!   candidates of its instance (§3.4).
//! - Its gates: the minimum interval between two of its dispatches, the backoff after a failed
//!   one, the pause for the rest of the day after `max_consecutive_failures` failures in a row,
//!   its daily limit, placement, the host budget and the Runtime's admission preview (§3.4,
//!   §3.9). Each affected candidate takes the first row of §3.7 that matches.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use actingcommand_contract::ReaderProblem;
use actingcommand_contract::outcome::{Code, Outcome};
use actingcommand_selection_policy::UnknownReason;

use super::{
    DecisionReason, DetectionSuggestion, EligibilityState, EvaluationFacts, EvaluationTime,
    HostRemaining, InstanceSnapshot, ObservedFact, PlacementCandidate, PolicyEvaluationError,
    PolicyEvaluationResult, RankRecord, ResourceLoad, SchedulingDecisionState, TaskRank,
    TaskRuntimeSnapshot, TaskWork, UnknownReasonToken, activity_window_at,
    deterministic_tie_breaker, min_wake, reason, reason_with, select_activity_profile,
    unknown_token,
};
use crate::codes::{PolicyCode, RefreshLimit};
use crate::{
    ActivityProfile, FailureAction, FailurePolicy, LoadProfile, LoopBudget, OperationRef,
    PredicateSpec, ScopeSelector, TaskSpec, TaskTerminalState,
};

/// The reserved task identity of a recognition maintenance package's dispatch (D2 model §3.4):
/// a catalog task id starts with a lowercase letter or a digit, so it never names one. The
/// dispatch's clocks (last dispatch, terminal state, failure streak) are kept per (this id,
/// instance) like a catalog task's.
pub const FACT_REFRESH_TASK_ID: &str = "_fact_refresh";
/// The Runtime-owned procedure of a fact refresh dispatch; its package is the instance's bound
/// recognition maintenance package (`instances[].recognition_package`).
pub const FACT_REFRESH_PROCEDURE_REF: &str = "_fact_refresh";
/// The Runtime-owned operation of a fact refresh dispatch: every instance with a bound reader
/// provides it.
pub const FACT_REFRESH_OPERATION_ID: &str = "_fact_refresh";
/// The run time one fact refresh reserves of its instance's activity budget. A reservation
/// estimate, never a bound: the run's own package bounds how long it takes.
pub const FACT_REFRESH_EXPECTED_DURATION_MS: u64 = 60_000;
/// The task runtime a fact refresh may reserve in one activity window: high enough never to bind,
/// since the fact refresh settings, not the task's loop budget, limit how often R_I runs.
const FACT_REFRESH_WINDOW_RUNTIME_MS: u64 = u32::MAX as u64;

/// The `scheduling.fact_refresh.*` settings an evaluation reads (D2 model §3.4), as in effect
/// when the cycle starts: a change applies from the next round on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FactRefreshSettings {
    /// Off: a realistic key nobody reads is `fact_refresh_unbound(reader_problem=disabled)`.
    pub enabled: bool,
    /// The least time between two R_I dispatches on one instance.
    pub min_interval_ms: u64,
    /// The most R_I dispatches on one instance in one activity day.
    pub daily_limit: u32,
    /// The least time from a failed R_I run to the next R_I dispatch.
    pub failure_backoff_ms: u64,
    /// The failed R_I runs in a row after which R_I rests for the rest of the activity day.
    pub max_consecutive_failures: u32,
}

/// One instance's recognition maintenance package R_I (D2 model §3.2), as the Runtime start
/// kept its binding, with what the Runtime counts for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactRefreshReader {
    pub instance_id: String,
    pub package_id: String,
    /// The keys its reading marks write.
    pub marked_keys: BTreeSet<String>,
    /// The R_I dispatches admitted on the instance in its current activity day.
    pub dispatched_today: u32,
    /// The code R_I's latest run failed with, while no success has followed it.
    pub last_failure_code: Option<Code>,
    /// Why the binding cannot serve now (its marks could not be read): every key it would read
    /// is `fact_refresh_unbound` with this reader problem.
    pub problem: Option<ReaderProblem>,
}

/// What keeps the scheduler from an instance as a cycle starts: the closure's positions 1-2 (an
/// operator scheduling pause; a holder, a waiting claim or a takeover cooldown), with the
/// reason and wake the closure gives a candidate there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceHold {
    pub reason: DecisionReason,
    pub next_wake_unix_ms: Option<u64>,
}

/// Workflow #384 D2-S4: what one evaluation's fact refresh reads besides the evaluation inputs.
/// Memory only: none of it enters a snapshot or decision identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactRefreshInputs {
    pub settings: FactRefreshSettings,
    /// The bound readers, one per instance at most.
    pub readers: Vec<FactRefreshReader>,
    /// L (D2 model §3.1): when this Runtime finished loading the instance facts. `None` reads
    /// every settled R_I run as one of this Runtime's.
    pub instance_facts_loaded_at_unix_ms: Option<u64>,
    /// Per instance alias, the closure's positions 1-2 that hold it as the cycle starts.
    pub instance_holds: BTreeMap<String, InstanceHold>,
}

/// The task spec a fact refresh dispatch is admitted, budgeted and settled under, the same for
/// every instance. Its loop budget never binds: the scheduler applies the fact refresh
/// settings before admission (so a setting changed while the Runtime runs never makes a
/// recorded admission disagree with the spec). Its failures never retry or pause: the
/// scheduler backs off and rests R_I by the settings (D2 model §3.9 item 1).
pub fn fact_refresh_task() -> &'static TaskSpec {
    static TASK: OnceLock<TaskSpec> = OnceLock::new();
    TASK.get_or_init(|| TaskSpec {
        id: FACT_REFRESH_TASK_ID.to_owned(),
        // Only an instance scope is compared with a dispatch's instance; this one names none.
        scope: ScopeSelector::Game {
            game_id: FACT_REFRESH_TASK_ID.to_owned(),
        },
        entrypoint: OperationRef {
            operation_id: FACT_REFRESH_OPERATION_ID.to_owned(),
        },
        procedure_ref: FACT_REFRESH_PROCEDURE_REF.to_owned(),
        priority: 0,
        // Never evaluated: the scheduler requests R_I for the keys candidates read.
        trigger: PredicateSpec::All {
            predicates: Vec::new(),
        },
        feedback_stop: PredicateSpec::Any {
            predicates: Vec::new(),
        },
        consumes: Vec::new(),
        produces: Vec::new(),
        on_failure: FailurePolicy {
            action: FailureAction::Continue,
            retry_limit: 0,
            retry_backoff_ms: 0,
            escalation_threshold: u16::MAX,
        },
        sensitive: false,
        next_run_clamp_ms: 0,
        yield_points: Vec::new(),
        expected_duration_ms: FACT_REFRESH_EXPECTED_DURATION_MS,
        cooldown_ms: 0,
        load_profile: LoadProfile::Light,
        loop_budget: LoopBudget {
            daily_limit: u32::MAX,
            window_iteration_limit: u32::MAX,
            max_runtime_ms: FACT_REFRESH_WINDOW_RUNTIME_MS,
        },
        strategic_weight_milli: 0,
        instance_overrides: Vec::new(),
        value_milli: None,
        continues_from: None,
    })
}

/// One unknown realistic key a candidate read (model §3.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RefreshKey {
    pub(super) fact_key: String,
    pub(super) unknown_reason: UnknownReason,
    /// The gate or term that read it, when a selection document did.
    pub(super) rule_id: Option<String>,
}

/// A candidate deferred for the unknown realistic keys it read, and the rank it lends R_I: its
/// catalog rank with its manual offset (model §3.4, "排名").
#[derive(Debug)]
pub(super) struct AffectedCandidate {
    pub(super) work_index: usize,
    pub(super) instance_id: String,
    pub(super) keys: Vec<RefreshKey>,
    pub(super) rank: TaskRank,
    /// It read the keys through a catalog predicate, so it never reached the closure.
    pub(super) predicate: bool,
}

/// What kept R_I from a dispatch this round, by the class the table of model §3.7 reads.
#[derive(Debug, Clone, Copy)]
pub(super) enum RefreshGate {
    /// Row 4: a limit (the daily limit, the failure pause, placement, the activity window).
    Limit {
        limit: RefreshLimit,
        gate: Option<Code>,
        wake: Option<u64>,
    },
    /// Row 6: anything else, for now.
    Temporary {
        gate: Option<Code>,
        wake: Option<u64>,
    },
}

/// One instance's fact refresh this round.
#[derive(Debug)]
pub(super) struct RefreshPlan {
    instance_id: String,
    package_id: String,
    /// R_I's work entry.
    work_index: usize,
    /// The affected candidates and the keys of each that R_I reads.
    affected: Vec<(usize, Vec<RefreshKey>)>,
    /// A gate met before the allocation; R_I then took no part in it.
    pre_gate: Option<RefreshGate>,
    /// R_I's clocks on the instance.
    state: Option<TaskRuntimeSnapshot>,
    /// The code of R_I's latest failure, when its latest run failed.
    last_failure_code: Option<Code>,
}

/// What [`plan`] reads.
pub(super) struct PlanContext<'a> {
    pub(super) inputs: &'a FactRefreshInputs,
    pub(super) facts: &'a EvaluationFacts,
    pub(super) profiles: &'a [ActivityProfile],
    pub(super) hosts: &'a BTreeMap<&'a str, HostRemaining>,
    pub(super) task_states: &'a BTreeMap<(&'a str, &'a str), &'a TaskRuntimeSnapshot>,
    pub(super) time: EvaluationTime,
    pub(super) seed: u64,
}

/// The unknown realistic keys an undecided catalog predicate read in the scope of exactly
/// `instance` (model §3.3, "目录谓词"): a server or game scope cannot be refreshed by a reading,
/// and a key outside the realistic families is an instruction nobody gave.
pub(super) fn predicate_keys(
    suggestions: &[DetectionSuggestion],
    instance: &InstanceSnapshot,
) -> Vec<RefreshKey> {
    let mut seen = BTreeSet::new();
    suggestions
        .iter()
        .filter(|suggestion| {
            matches!(&suggestion.scope, ScopeSelector::Instance { instance_id }
                if *instance_id == instance.instance_id)
        })
        .filter_map(|suggestion| {
            let unknown_reason = suggestion_unknown_reason(suggestion)?;
            (super::is_realistic(&suggestion.fact_key, unknown_reason)
                && seen.insert(suggestion.fact_key.as_str()))
            .then(|| RefreshKey {
                fact_key: suggestion.fact_key.clone(),
                unknown_reason,
                rule_id: None,
            })
        })
        .collect()
}

/// The `unknown_reason` token a predicate's suggestion carries, read as the selection-policy
/// reason of the same spelling; `None` for a reason no reading answers (a dependency or an
/// outcome).
fn suggestion_unknown_reason(suggestion: &DetectionSuggestion) -> Option<UnknownReason> {
    [
        UnknownReason::FactMissing,
        UnknownReason::FactExpired,
        UnknownReason::FactStale,
        UnknownReason::FactLowConfidence,
        UnknownReason::FactNotScalar,
        UnknownReason::FieldMissing,
        UnknownReason::TypeMismatch,
    ]
    .into_iter()
    .find(|candidate| unknown_token(*candidate) == suggestion.reason)
}

/// Settles every affected candidate that no reader can serve (§3.7 row 2) and plans R_I on every
/// instance where one can: its work entry with `fact_refresh_requested` for each key it reads,
/// and, unless one of its own gates holds it back, its candidate for the allocation.
pub(super) fn plan(
    context: &PlanContext<'_>,
    affected: Vec<AffectedCandidate>,
    work: &mut Vec<TaskWork>,
    candidates: &mut Vec<PlacementCandidate>,
) -> PolicyEvaluationResult<Vec<RefreshPlan>> {
    let mut by_instance = BTreeMap::<String, Vec<AffectedCandidate>>::new();
    for candidate in affected {
        by_instance
            .entry(candidate.instance_id.clone())
            .or_default()
            .push(candidate);
    }
    let settings = &context.inputs.settings;
    let mut plans = Vec::new();
    for (instance_id, affected) in by_instance {
        // §3.4 step 1: only candidates past the closure's positions 1-2 raise demand. On a held
        // or paused instance a candidate the score stage deferred carries the hold's reason
        // alone, as the closure would have given it; a predicate's keeps its own reasons.
        if let Some(hold) = context.inputs.instance_holds.get(&instance_id) {
            for candidate in affected.iter().filter(|candidate| !candidate.predicate) {
                let entry = &mut work[candidate.work_index];
                entry.state = SchedulingDecisionState::Deferred;
                entry.reasons.push(hold.reason.clone());
                entry.next_wake_unix_ms = min_wake(entry.next_wake_unix_ms, hold.next_wake_unix_ms);
            }
            continue;
        }
        let reader = context
            .inputs
            .readers
            .iter()
            .find(|reader| reader.instance_id == instance_id);
        let mut requested = BTreeMap::<&str, &RefreshKey>::new();
        let mut planned = Vec::new();
        let mut inherited: Option<&TaskRank> = None;
        for candidate in &affected {
            let entry = &mut work[candidate.work_index];
            entry.state = SchedulingDecisionState::Deferred;
            let mut read = Vec::new();
            for key in &candidate.keys {
                // §3.7 row 2: no reader can serve the key.
                let problem = match reader {
                    _ if !settings.enabled => Some(ReaderProblem::Disabled),
                    None => Some(ReaderProblem::PackageUnbound),
                    Some(reader) => reader.problem.or_else(|| {
                        (!reader.marked_keys.contains(&key.fact_key))
                            .then_some(ReaderProblem::KeyNotMarked)
                    }),
                };
                match problem {
                    Some(problem) => {
                        entry
                            .reasons
                            .push(unbound_reason(&instance_id, key, Some(problem)))
                    }
                    None => {
                        requested.entry(key.fact_key.as_str()).or_insert(key);
                        read.push(key.clone());
                    }
                }
            }
            if !read.is_empty() {
                if inherited.is_none_or(|rank| rank.total_score < candidate.rank.total_score) {
                    inherited = Some(&candidate.rank);
                }
                planned.push((candidate.work_index, read));
            }
        }
        let (Some(reader), Some(inherited)) = (reader, inherited) else {
            continue;
        };
        let instance = context
            .facts
            .instances
            .iter()
            .find(|instance| instance.instance_id == instance_id)
            .ok_or_else(|| {
                PolicyEvaluationError::invalid(format!(
                    "fact refresh instance '{instance_id}' is missing from the snapshot"
                ))
            })?;
        let state = context
            .task_states
            .get(&(FACT_REFRESH_TASK_ID, instance_id.as_str()))
            .copied();
        let failed =
            state.is_some_and(|state| state.terminal_state == Some(TaskTerminalState::Failed));
        let work_index = work.len();
        let mut entry = TaskWork::new(FACT_REFRESH_TASK_ID.to_owned(), instance_id.clone());
        entry.eligibility = EligibilityState::True;
        entry.state = SchedulingDecisionState::Eligible;
        for key in requested.values() {
            entry.reasons.push(requested_reason(&instance_id, key));
        }
        let keys = requested.keys().copied().collect::<Vec<_>>();
        let gate = own_gate(context, instance, reader, state, &keys, &mut entry)?;
        match gate {
            Some(gate) => {
                entry.state = SchedulingDecisionState::Deferred;
                let (RefreshGate::Limit { wake, .. } | RefreshGate::Temporary { wake, .. }) = gate;
                entry.next_wake_unix_ms = wake;
            }
            None => {
                let candidate = refresh_candidate(context, instance, state, inherited, work_index)?;
                entry.rank = Some(candidate.rank.clone());
                entry.record = Some(RankRecord {
                    base_total: candidate.rank.total_score,
                    stage: None,
                    resources: None,
                    promoted: false,
                    affinity: false,
                    tie_breaker: candidate.tie_breaker,
                });
                candidates.push(candidate);
            }
        }
        work.push(entry);
        plans.push(RefreshPlan {
            instance_id,
            package_id: reader.package_id.clone(),
            work_index,
            affected: planned,
            pre_gate: gate,
            state: state.cloned(),
            last_failure_code: reader.last_failure_code.filter(|_| failed),
        });
    }
    Ok(plans)
}

/// R_I's own gates before the allocation (§3.4 step 3, §3.9 item 1), each with the reason its
/// decision carries: placement without an activity profile, the failure pause and the daily
/// limit (limits); an unavailable instance, a host that cannot take its load and the minimum
/// interval or failure backoff (for now).
fn own_gate(
    context: &PlanContext<'_>,
    instance: &InstanceSnapshot,
    reader: &FactRefreshReader,
    state: Option<&TaskRuntimeSnapshot>,
    keys: &[&str],
    entry: &mut TaskWork,
) -> PolicyEvaluationResult<Option<RefreshGate>> {
    let settings = &context.inputs.settings;
    let now = context.time.unix_ms;
    let Some(profile) = select_activity_profile(context.profiles, instance) else {
        entry.reasons.push(reason(
            PolicyCode::ActivityProfileMissing,
            format!(
                "instance '{}' has no matching activity profile",
                instance.instance_id
            ),
        ));
        return Ok(Some(RefreshGate::Limit {
            limit: RefreshLimit::Placement,
            gate: Some(PolicyCode::ActivityProfileMissing.code()),
            wake: None,
        }));
    };
    // The activity day the Runtime counts the daily limit in; outside every window the
    // admission preview reports the closed window.
    if let Some(window) = activity_window_at(profile, now)? {
        let paused = match state {
            Some(state)
                if state.terminal_state == Some(TaskTerminalState::Failed)
                    && state.failure_streak.is_some_and(|streak| {
                        u32::from(streak) >= settings.max_consecutive_failures
                    }) =>
            {
                match state.last_dispatched_unix_ms {
                    Some(at) => activity_window_at(profile, at)?
                        .is_some_and(|run| run.local_day == window.local_day),
                    None => false,
                }
            }
            _ => false,
        };
        let limit = if paused {
            Some(RefreshLimit::FailurePause)
        } else if reader.dispatched_today >= settings.daily_limit {
            Some(RefreshLimit::DailyLimit)
        } else {
            None
        };
        if let Some(limit) = limit {
            let wake = Some(window.until_unix_ms);
            for key in keys {
                entry.reasons.push(exhausted_reason(
                    &instance.instance_id,
                    key,
                    &reader.package_id,
                    limit,
                    None,
                    wake,
                ));
            }
            return Ok(Some(RefreshGate::Limit {
                limit,
                gate: None,
                wake,
            }));
        }
    }
    if !instance.available {
        entry.reasons.push(reason(
            PolicyCode::InstanceUnavailable,
            "the instance is unavailable",
        ));
        return Ok(Some(RefreshGate::Temporary {
            gate: Some(PolicyCode::InstanceUnavailable.code()),
            wake: None,
        }));
    }
    let host = context
        .hosts
        .get(instance.host_id.as_str())
        .ok_or_else(|| {
            PolicyEvaluationError::invalid(format!(
                "instance '{}' references missing host '{}'",
                instance.instance_id, instance.host_id
            ))
        })?;
    let load = ResourceLoad::new(
        &fact_refresh_task().load_profile,
        host.heavy_threshold_milli,
    );
    if !host.fits(load) {
        entry.reasons.push(reason(
            PolicyCode::HostCapacityInsufficient,
            format!(
                "host '{}' cannot satisfy the fact refresh load profile",
                instance.host_id
            ),
        ));
        return Ok(Some(RefreshGate::Temporary {
            gate: Some(PolicyCode::HostCapacityInsufficient.code()),
            wake: None,
        }));
    }
    if let Some(until) = next_eligible(settings, state)?
        && now < until
    {
        entry.reasons.push(reason(
            PolicyCode::TaskCooldownActive,
            format!(
                "the fact refresh waits for its minimum interval or failure backoff until {until}"
            ),
        ));
        return Ok(Some(RefreshGate::Temporary {
            gate: Some(PolicyCode::TaskCooldownActive.code()),
            wake: Some(until),
        }));
    }
    Ok(None)
}

/// When R_I may next be dispatched by its own clocks: the minimum interval after its latest
/// dispatch and, after a failed run, the failure backoff from the run's end.
fn next_eligible(
    settings: &FactRefreshSettings,
    state: Option<&TaskRuntimeSnapshot>,
) -> PolicyEvaluationResult<Option<u64>> {
    let Some(state) = state else {
        return Ok(None);
    };
    let overflow = || PolicyEvaluationError::overflow("fact refresh interval overflowed");
    let interval = state
        .last_dispatched_unix_ms
        .map(|at| {
            at.checked_add(settings.min_interval_ms)
                .ok_or_else(overflow)
        })
        .transpose()?;
    let backoff = if state.terminal_state == Some(TaskTerminalState::Failed) {
        state
            .completed_window
            .as_ref()
            .map(|completed| completed.completed_at_unix_ms)
            .or(state.last_dispatched_unix_ms)
            .map(|at| {
                at.checked_add(settings.failure_backoff_ms)
                    .ok_or_else(overflow)
            })
            .transpose()?
    } else {
        None
    };
    Ok(match (interval, backoff) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (one, other) => one.or(other),
    })
}

/// R_I's candidate for the allocation: the Runtime-owned task on the instance, with the rank of
/// the highest-ranked candidate it unlocks.
fn refresh_candidate(
    context: &PlanContext<'_>,
    instance: &InstanceSnapshot,
    state: Option<&TaskRuntimeSnapshot>,
    inherited: &TaskRank,
    work_index: usize,
) -> PolicyEvaluationResult<PlacementCandidate> {
    let task = fact_refresh_task();
    let profile = select_activity_profile(context.profiles, instance).ok_or_else(|| {
        PolicyEvaluationError::invalid(format!(
            "fact refresh instance '{}' lost its activity profile",
            instance.instance_id
        ))
    })?;
    let host = context
        .hosts
        .get(instance.host_id.as_str())
        .ok_or_else(|| {
            PolicyEvaluationError::invalid(format!(
                "instance '{}' references missing host '{}'",
                instance.instance_id, instance.host_id
            ))
        })?;
    Ok(PlacementCandidate {
        work_index,
        task_id: task.id.clone(),
        instance_id: instance.instance_id.clone(),
        host_id: instance.host_id.clone(),
        operation_id: task.entrypoint.operation_id.clone(),
        procedure_ref: task.procedure_ref.clone(),
        expected_duration_ms: task.expected_duration_ms,
        yield_points: task.yield_points.clone(),
        load_profile: task.load_profile.clone(),
        load: ResourceLoad::new(&task.load_profile, host.heavy_threshold_milli),
        rank: inherited.clone(),
        affinity: false,
        promoted: false,
        value_milli: None,
        last_duration_ms: state.and_then(|state| state.last_duration_ms),
        tie_breaker: deterministic_tie_breaker(context.seed, &task.id, &instance.instance_id),
        facts_fresh_until_unix_ms: None,
        activity_profile_id: profile.id.clone(),
        daily_limit: task.loop_budget.daily_limit,
        window_iteration_limit: task.loop_budget.window_iteration_limit,
        max_runtime_ms: task.loop_budget.max_runtime_ms,
    })
}

/// Gives every affected candidate its reason for each key R_I reads, by the first row of
/// model §3.7 that matches, and the earliest wake those rows name:
///
/// 3. R_I is selected this round: `fact_refresh_pending`, no wake;
/// 4. a limit holds R_I back: `fact_refresh_exhausted`, the limit's wake;
/// 5. R_I's latest dispatch since L settled successfully and still left the key unknown (no
///    record from it, or one it read with too little confidence or the wrong type):
///    `fact_refresh_unresolved`, R_I's next eligible time;
/// 6. anything else: `fact_refresh_pending` with the gate's code and wake, and the code of
///    R_I's latest failure when its latest run failed.
pub(super) fn finish(
    plans: &[RefreshPlan],
    gates: &BTreeMap<usize, RefreshGate>,
    inputs: &FactRefreshInputs,
    facts: &EvaluationFacts,
    time: EvaluationTime,
    work: &mut [TaskWork],
) -> PolicyEvaluationResult<()> {
    for plan in plans {
        let selected = work[plan.work_index].state == SchedulingDecisionState::Selected;
        let gate = gates.get(&plan.work_index).copied().or(plan.pre_gate);
        let next_eligible = next_eligible(&inputs.settings, plan.state.as_ref())?
            .filter(|until| *until > time.unix_ms);
        for (work_index, keys) in &plan.affected {
            let mut wake = None;
            for key in keys {
                let (reason, key_wake) = match gate {
                    _ if selected => (pending_reason(plan, key, None, None, None), None),
                    Some(RefreshGate::Limit { limit, gate, wake }) => (
                        exhausted_reason(
                            &plan.instance_id,
                            &key.fact_key,
                            &plan.package_id,
                            limit,
                            gate,
                            wake,
                        ),
                        wake,
                    ),
                    Some(RefreshGate::Temporary { gate, wake }) => {
                        if unresolved(plan, key, inputs, facts) {
                            let wake = wake.or(next_eligible);
                            (unresolved_reason(plan, key, wake), wake)
                        } else {
                            (
                                pending_reason(plan, key, gate, plan.last_failure_code, wake),
                                wake,
                            )
                        }
                    }
                    None => (
                        pending_reason(plan, key, None, plan.last_failure_code, None),
                        None,
                    ),
                };
                work[*work_index].reasons.push(reason);
                wake = min_wake(wake, key_wake);
            }
            let entry = &mut work[*work_index];
            entry.next_wake_unix_ms = min_wake(entry.next_wake_unix_ms, wake);
        }
    }
    Ok(())
}

/// §3.7 row 5: R_I's latest dispatch settled successfully at or after L, and the key is still
/// unknown because of it: no record of the key since that dispatch, or one taken since then
/// that recognition could not make usable (too little confidence, not a scalar, another type).
/// A record R_I wrote that merely expired is not this row: R_I reads it again.
fn unresolved(
    plan: &RefreshPlan,
    key: &RefreshKey,
    inputs: &FactRefreshInputs,
    facts: &EvaluationFacts,
) -> bool {
    let Some(state) = &plan.state else {
        return false;
    };
    let Some(dispatched_at) = state.last_dispatched_unix_ms else {
        return false;
    };
    if state.terminal_state != Some(TaskTerminalState::Succeeded)
        || inputs
            .instance_facts_loaded_at_unix_ms
            .is_some_and(|loaded_at| dispatched_at < loaded_at)
    {
        return false;
    }
    match instance_record(facts, &plan.instance_id, &key.fact_key) {
        None => true,
        Some(record) if record.observed_at_unix_ms < dispatched_at => true,
        Some(_) => matches!(
            key.unknown_reason,
            UnknownReason::FactLowConfidence
                | UnknownReason::FactNotScalar
                | UnknownReason::TypeMismatch
        ),
    }
}

/// The instance's own record of `fact_key`, the only scope a reading writes (RR:158).
fn instance_record<'a>(
    facts: &'a EvaluationFacts,
    instance_id: &str,
    fact_key: &str,
) -> Option<&'a ObservedFact> {
    facts.facts.iter().find(|fact| {
        fact.fact_key == fact_key
            && matches!(&fact.scope, ScopeSelector::Instance { instance_id: scope }
                if scope == instance_id)
    })
}

/// `fact_refresh_unbound` (§3.7 row 2): no reader can refresh the key; `problem` says why when
/// the evaluation knows the readers (D2-S4), and is absent without them (D2-S1).
pub(super) fn unbound_reason(
    instance_id: &str,
    key: &RefreshKey,
    problem: Option<ReaderProblem>,
) -> DecisionReason {
    let outcome = Outcome::new(PolicyCode::FactRefreshUnbound)
        .instance_id(instance_id)
        .fact_key(key.fact_key.as_str())
        .unknown_reason(UnknownReasonToken(key.unknown_reason));
    let mut detail = format!(
        "instance_id={instance_id} fact_key={} unknown_reason={}",
        key.fact_key,
        unknown_token(key.unknown_reason)
    );
    let outcome = match problem {
        Some(problem) => {
            detail.push_str(&format!(" reader_problem={}", problem.as_str()));
            outcome.reader_problem(problem)
        }
        None => outcome,
    };
    reason_with(outcome, detail)
}

/// `fact_refresh_requested`: R_I is requested for the key (its decision and dispatch chain).
fn requested_reason(instance_id: &str, key: &RefreshKey) -> DecisionReason {
    let mut outcome = Outcome::new(PolicyCode::FactRefreshRequested)
        .instance_id(instance_id)
        .fact_key(key.fact_key.as_str())
        .unknown_reason(UnknownReasonToken(key.unknown_reason));
    let mut detail = format!(
        "instance_id={instance_id} fact_key={} unknown_reason={}",
        key.fact_key,
        unknown_token(key.unknown_reason)
    );
    if let Some(rule_id) = &key.rule_id {
        outcome = outcome.rule_id(rule_id.as_str());
        detail.push_str(&format!(" rule_id={rule_id}"));
    }
    reason_with(outcome, detail)
}

/// `fact_refresh_pending` (§3.7 rows 3 and 6).
fn pending_reason(
    plan: &RefreshPlan,
    key: &RefreshKey,
    gate: Option<Code>,
    failure: Option<Code>,
    wake: Option<u64>,
) -> DecisionReason {
    let mut outcome = Outcome::new(PolicyCode::FactRefreshPending)
        .instance_id(plan.instance_id.as_str())
        .fact_key(key.fact_key.as_str())
        .unknown_reason(UnknownReasonToken(key.unknown_reason))
        .package_id(plan.package_id.as_str());
    let mut detail = format!(
        "instance_id={} fact_key={} unknown_reason={} package_id={}",
        plan.instance_id,
        key.fact_key,
        unknown_token(key.unknown_reason),
        plan.package_id
    );
    if let Some(gate) = gate {
        outcome = outcome.gate_code(gate);
        detail.push_str(&format!(" gate_code={gate}"));
    }
    if let Some(failure) = failure {
        outcome = outcome.failure_code(failure);
        detail.push_str(&format!(" failure_code={failure}"));
    }
    if let Some(wake) = wake {
        outcome = outcome.next_eligible_unix_ms(wake);
        detail.push_str(&format!(" next_eligible_unix_ms={wake}"));
    }
    reason_with(outcome, detail)
}

/// `fact_refresh_unresolved` (§3.7 row 5).
fn unresolved_reason(plan: &RefreshPlan, key: &RefreshKey, wake: Option<u64>) -> DecisionReason {
    let mut outcome = Outcome::new(PolicyCode::FactRefreshUnresolved)
        .instance_id(plan.instance_id.as_str())
        .fact_key(key.fact_key.as_str())
        .unknown_reason(UnknownReasonToken(key.unknown_reason))
        .package_id(plan.package_id.as_str());
    let mut detail = format!(
        "instance_id={} fact_key={} unknown_reason={} package_id={}",
        plan.instance_id,
        key.fact_key,
        unknown_token(key.unknown_reason),
        plan.package_id
    );
    if let Some(wake) = wake {
        outcome = outcome.next_eligible_unix_ms(wake);
        detail.push_str(&format!(" next_eligible_unix_ms={wake}"));
    }
    reason_with(outcome, detail)
}

/// `fact_refresh_exhausted` (§3.7 row 4).
fn exhausted_reason(
    instance_id: &str,
    fact_key: &str,
    package_id: &str,
    limit: RefreshLimit,
    gate: Option<Code>,
    wake: Option<u64>,
) -> DecisionReason {
    let mut outcome = Outcome::new(PolicyCode::FactRefreshExhausted)
        .instance_id(instance_id)
        .fact_key(fact_key)
        .package_id(package_id)
        .refresh_limit(limit);
    let mut detail = format!(
        "instance_id={instance_id} fact_key={fact_key} package_id={package_id} refresh_limit={}",
        limit.as_str()
    );
    if let Some(gate) = gate {
        outcome = outcome.gate_code(gate);
        detail.push_str(&format!(" gate_code={gate}"));
    }
    if let Some(wake) = wake {
        outcome = outcome.next_eligible_unix_ms(wake);
        detail.push_str(&format!(" next_eligible_unix_ms={wake}"));
    }
    reason_with(outcome, detail)
}
