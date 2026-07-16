// SPDX-License-Identifier: AGPL-3.0-only

//! Deterministic scheduling decisions over compiled catalogs and pinned input snapshots.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    ActivityProfile, ClockSchedule, Comparison, CompiledCatalog, FactValue, LoadProfile,
    ObservationRef, PoolSpec, PredicateSpec, ResourceEffectSpec, ScopeSelector, TaskSpec,
    TaskTerminalState,
};

pub const MAX_EVALUATION_FACTS: usize = 16_384;
pub const MAX_EVALUATION_OUTCOMES: usize = 16_384;
pub const MAX_EVALUATION_TASK_STATES: usize = 8_192;
pub const MAX_EVALUATION_INSTANCES: usize = 1_024;
pub const MAX_EVALUATION_POOLS: usize = 4_096;
pub const MAX_EVALUATION_HOSTS: usize = 1_024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedFact {
    pub scope: ScopeSelector,
    pub fact_key: String,
    pub value: FactValue,
    pub observed_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedOutcome {
    pub task_id: String,
    pub instance_id: String,
    pub outcome_key: String,
    pub value: FactValue,
    pub observed_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskRuntimeSnapshot {
    pub task_id: String,
    pub instance_id: String,
    pub last_dispatched_unix_ms: Option<u64>,
    pub eligible_since_unix_ms: Option<u64>,
    pub terminal_state: Option<TaskTerminalState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceSnapshot {
    pub instance_id: String,
    pub server_id: String,
    pub game_id: String,
    pub host_id: String,
    pub available: bool,
    pub capability_operation_ids: Vec<String>,
    pub preferred_task_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationFacts {
    pub ledger_position: u64,
    pub fact_snapshot_id: String,
    pub facts: Vec<ObservedFact>,
    pub outcomes: Vec<ObservedOutcome>,
    pub tasks: Vec<TaskRuntimeSnapshot>,
    pub instances: Vec<InstanceSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoolValueSnapshot {
    pub pool_id: String,
    pub value: u64,
    pub observed_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostResourceSnapshot {
    pub host_id: String,
    pub cpu_available_milli: u16,
    pub gpu_available_milli: u16,
    pub io_available_milli: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationResources {
    pub pools: Vec<PoolValueSnapshot>,
    pub hosts: Vec<HostResourceSnapshot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationTime {
    pub unix_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EligibilityState {
    True,
    False,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchedulingDecisionState {
    Eligible,
    Deferred,
    Blocked,
    Selected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DetectionSuggestion {
    pub scope: ScopeSelector,
    pub fact_key: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionReason {
    pub code: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskRank {
    pub priority: i16,
    pub aging_ms: u64,
    pub strategic_weight_milli: u32,
    pub urgency_milli: u16,
    pub total_score: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskDecision {
    pub task_id: String,
    pub instance_id: Option<String>,
    pub eligibility: EligibilityState,
    pub state: SchedulingDecisionState,
    pub rank: Option<TaskRank>,
    pub detection_suggestions: Vec<DetectionSuggestion>,
    pub reasons: Vec<DecisionReason>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchIntent {
    pub decision_id: String,
    pub task_id: String,
    pub instance_id: String,
    pub operation_id: String,
    pub procedure_ref: String,
    pub catalog_hash: String,
    pub catalog_version: u64,
    pub input_ledger_position: u64,
    pub fact_snapshot_id: String,
    pub approval_refs: Vec<String>,
    pub reason_chain_id: String,
    pub expected_duration_ms: u64,
    pub load_profile: LoadProfile,
    pub prerequisites: DispatchPrerequisites,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchPrerequisites {
    pub fencing_required: bool,
    pub evaluated_at_unix_ms: u64,
    pub facts_fresh_until_unix_ms: Option<u64>,
    pub daily_limit: u32,
    pub window_iteration_limit: u32,
    pub max_runtime_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionReasonChain {
    pub id: String,
    pub decision_id: String,
    pub reasons: Vec<DecisionReason>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyEvaluation {
    pub decisions: Vec<TaskDecision>,
    pub next_wake_unix_ms: Option<u64>,
    pub dispatch_intents: Vec<DispatchIntent>,
    pub reason_chains: Vec<DecisionReasonChain>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyEvaluationError {
    code: &'static str,
    message: String,
}

impl PolicyEvaluationError {
    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: "policy_evaluation_input_invalid",
            message: message.into(),
        }
    }

    fn type_mismatch(message: impl Into<String>) -> Self {
        Self {
            code: "policy_evaluation_fact_type_mismatch",
            message: message.into(),
        }
    }

    fn overflow(message: impl Into<String>) -> Self {
        Self {
            code: "policy_evaluation_numeric_overflow",
            message: message.into(),
        }
    }
}

impl fmt::Display for PolicyEvaluationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl Error for PolicyEvaluationError {}

pub type PolicyEvaluationResult<T> = Result<T, PolicyEvaluationError>;

/// Evaluates one pinned snapshot without reading clocks, storage, devices, or process state.
pub fn evaluate(
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    resources: &EvaluationResources,
    time: EvaluationTime,
    seed: u64,
) -> PolicyEvaluationResult<PolicyEvaluation> {
    validate_inputs(catalog, facts, resources, time)?;

    let catalog_bundle = catalog.catalog();
    let task_states: BTreeMap<(&str, &str), &TaskRuntimeSnapshot> = facts
        .tasks
        .iter()
        .map(|state| ((state.task_id.as_str(), state.instance_id.as_str()), state))
        .collect();
    let pool_specs: BTreeMap<&str, &PoolSpec> = catalog_bundle
        .pools
        .pools
        .iter()
        .map(|pool| (pool.id.as_str(), pool))
        .collect();
    let pool_values: BTreeMap<&str, &PoolValueSnapshot> = resources
        .pools
        .iter()
        .map(|pool| (pool.pool_id.as_str(), pool))
        .collect();
    let host_resources: BTreeMap<&str, HostRemaining> = resources
        .hosts
        .iter()
        .map(|host| (host.host_id.as_str(), HostRemaining::from(host)))
        .collect();

    let mut next_wake = catalog_bundle
        .timeline
        .events
        .iter()
        .filter_map(|event| next_schedule_occurrence(&event.schedule, time.unix_ms))
        .min();
    let placement_context = PlacementContext {
        profiles: catalog_bundle.activity.profiles.as_slice(),
        pool_specs: &pool_specs,
        pool_values: &pool_values,
        hosts: &host_resources,
        time,
        seed,
    };
    let mut work = Vec::new();
    let mut candidates = Vec::new();

    for task in &catalog_bundle.tasks.tasks {
        let matching_instances: Vec<&InstanceSnapshot> = facts
            .instances
            .iter()
            .filter(|instance| scope_matches_instance(&task.scope, instance))
            .collect();
        if matching_instances.is_empty() {
            work.push(TaskWork::blocked_without_instance(
                task.id.clone(),
                reason(
                    "scope_without_instance",
                    "no runtime instance matches the task scope",
                ),
            ));
            continue;
        }

        for instance in matching_instances {
            let state = task_states
                .get(&(task.id.as_str(), instance.instance_id.as_str()))
                .copied();
            let decision_scope = ScopeSelector::Instance {
                instance_id: instance.instance_id.clone(),
            };
            let trigger = evaluate_predicate(
                &task.trigger,
                state,
                &decision_scope,
                facts,
                &pool_specs,
                &pool_values,
                time,
            )?;
            next_wake = min_wake(next_wake, trigger.next_wake_unix_ms);

            let mut task_work = TaskWork::new(task.id.clone(), instance.instance_id.clone());
            match trigger.truth {
                PredicateTruth::False => {
                    task_work.eligibility = EligibilityState::False;
                    task_work.state = SchedulingDecisionState::Blocked;
                    task_work.reasons.push(reason(
                        "trigger_false",
                        "the task trigger evaluated to false for this instance",
                    ));
                }
                PredicateTruth::Unknown => {
                    task_work.suggestions = trigger.suggestions;
                    task_work.reasons.push(reason(
                        "trigger_unknown",
                        "the task trigger requires additional observations",
                    ));
                }
                PredicateTruth::True => {
                    let stop = evaluate_predicate(
                        &task.feedback_stop,
                        state,
                        &decision_scope,
                        facts,
                        &pool_specs,
                        &pool_values,
                        time,
                    )?;
                    next_wake = min_wake(next_wake, stop.next_wake_unix_ms);
                    match stop.truth {
                        PredicateTruth::True => {
                            task_work.eligibility = EligibilityState::False;
                            task_work.state = SchedulingDecisionState::Blocked;
                            task_work.reasons.push(reason(
                                "feedback_stop_true",
                                "the feedback stop predicate evaluated to true for this instance",
                            ));
                        }
                        PredicateTruth::Unknown => {
                            task_work.suggestions = stop.suggestions;
                            task_work.reasons.push(reason(
                                "feedback_stop_unknown",
                                "the feedback stop predicate requires additional observations",
                            ));
                        }
                        PredicateTruth::False => {
                            task_work.eligibility = EligibilityState::True;
                            task_work.state = SchedulingDecisionState::Eligible;
                            task_work.reasons.push(reason(
                                "eligible",
                                "trigger passed and feedback stop did not fire",
                            ));
                            let facts_fresh_until_unix_ms =
                                min_wake(trigger.fresh_until_unix_ms, stop.fresh_until_unix_ms);
                            match build_candidate(
                                work.len(),
                                task,
                                state,
                                instance,
                                &placement_context,
                                facts_fresh_until_unix_ms,
                            )? {
                                PlacementResult::Candidate(candidate) => {
                                    task_work.rank = Some(candidate.rank.clone());
                                    candidates.push(candidate);
                                }
                                PlacementResult::Blocked(blocked_reason) => {
                                    task_work.state = SchedulingDecisionState::Blocked;
                                    task_work.reasons.push(blocked_reason);
                                }
                            }
                        }
                    }
                }
            }
            normalize_suggestions(&mut task_work.suggestions);
            work.push(task_work);
        }
    }

    candidates.sort_by(|left, right| {
        right
            .rank
            .total_score
            .cmp(&left.rank.total_score)
            .then_with(|| right.affinity.cmp(&left.affinity))
            .then_with(|| left.tie_breaker.cmp(&right.tie_breaker))
            .then_with(|| left.task_id.cmp(&right.task_id))
            .then_with(|| left.instance_id.cmp(&right.instance_id))
    });

    let mut remaining = host_resources;
    let mut selected_instances = BTreeSet::new();
    let mut dispatch_intents = Vec::new();
    let mut reason_chains = Vec::new();

    for candidate in candidates {
        if selected_instances.contains(candidate.instance_id.as_str()) {
            work[candidate.work_index].reasons.push(reason(
                "instance_already_selected",
                "a higher-ranked task already owns this instance for the evaluation round",
            ));
            continue;
        }
        let host = remaining
            .get_mut(candidate.host_id.as_str())
            .ok_or_else(|| {
                PolicyEvaluationError::invalid(format!(
                    "instance '{}' references missing host '{}'",
                    candidate.instance_id, candidate.host_id
                ))
            })?;
        if !host.fits(candidate.load) {
            work[candidate.work_index].reasons.push(reason(
                "host_budget_deferred",
                "the shared host budget was consumed by higher-ranked dispatches",
            ));
            continue;
        }
        host.consume(candidate.load);
        selected_instances.insert(candidate.instance_id.clone());

        let decision_id = deterministic_decision_id(
            catalog.catalog_hash(),
            &candidate.task_id,
            &candidate.instance_id,
            facts.ledger_position,
            time.unix_ms,
            seed,
        );
        let reason_chain_id = format!("reason:{decision_id}");
        let mut reasons = work[candidate.work_index].reasons.clone();
        reasons.extend([
            reason(
                "placement_selected",
                format!(
                    "instance '{}' satisfies scope, capability, affinity, and host budget",
                    candidate.instance_id
                ),
            ),
            reason(
                "ranked",
                format!("deterministic total score {}", candidate.rank.total_score),
            ),
        ]);
        reason_chains.push(DecisionReasonChain {
            id: reason_chain_id.clone(),
            decision_id: decision_id.clone(),
            reasons: reasons.clone(),
        });
        dispatch_intents.push(DispatchIntent {
            decision_id,
            task_id: candidate.task_id.clone(),
            instance_id: candidate.instance_id.clone(),
            operation_id: candidate.operation_id,
            procedure_ref: candidate.procedure_ref,
            catalog_hash: catalog.catalog_hash().to_owned(),
            catalog_version: catalog_bundle.tasks.catalog.catalog_version,
            input_ledger_position: facts.ledger_position,
            fact_snapshot_id: facts.fact_snapshot_id.clone(),
            approval_refs: catalog_bundle.tasks.catalog.approval_refs.clone(),
            reason_chain_id,
            expected_duration_ms: candidate.expected_duration_ms,
            load_profile: candidate.load_profile,
            prerequisites: DispatchPrerequisites {
                fencing_required: true,
                evaluated_at_unix_ms: time.unix_ms,
                facts_fresh_until_unix_ms: candidate.facts_fresh_until_unix_ms,
                daily_limit: candidate.daily_limit,
                window_iteration_limit: candidate.window_iteration_limit,
                max_runtime_ms: candidate.max_runtime_ms,
            },
        });
        let task_work = &mut work[candidate.work_index];
        task_work.state = SchedulingDecisionState::Selected;
        task_work.rank = Some(candidate.rank);
        task_work.reasons = reasons;
    }
    work.sort_by(|left, right| {
        left.task_id.cmp(&right.task_id).then_with(|| {
            left.instance_id
                .as_deref()
                .cmp(&right.instance_id.as_deref())
        })
    });

    Ok(PolicyEvaluation {
        decisions: work.into_iter().map(TaskDecision::from).collect(),
        next_wake_unix_ms: next_wake,
        dispatch_intents,
        reason_chains,
    })
}

#[derive(Debug)]
struct TaskWork {
    task_id: String,
    instance_id: Option<String>,
    eligibility: EligibilityState,
    state: SchedulingDecisionState,
    rank: Option<TaskRank>,
    suggestions: Vec<DetectionSuggestion>,
    reasons: Vec<DecisionReason>,
}

impl TaskWork {
    fn new(task_id: String, instance_id: String) -> Self {
        Self {
            task_id,
            instance_id: Some(instance_id),
            eligibility: EligibilityState::Unknown,
            state: SchedulingDecisionState::Deferred,
            rank: None,
            suggestions: Vec::new(),
            reasons: Vec::new(),
        }
    }

    fn blocked_without_instance(task_id: String, blocked_reason: DecisionReason) -> Self {
        Self {
            task_id,
            instance_id: None,
            eligibility: EligibilityState::False,
            state: SchedulingDecisionState::Blocked,
            rank: None,
            suggestions: Vec::new(),
            reasons: vec![blocked_reason],
        }
    }
}

impl From<TaskWork> for TaskDecision {
    fn from(value: TaskWork) -> Self {
        Self {
            task_id: value.task_id,
            instance_id: value.instance_id,
            eligibility: value.eligibility,
            state: value.state,
            rank: value.rank,
            detection_suggestions: value.suggestions,
            reasons: value.reasons,
        }
    }
}

#[derive(Debug)]
struct PlacementCandidate {
    work_index: usize,
    task_id: String,
    instance_id: String,
    host_id: String,
    operation_id: String,
    procedure_ref: String,
    expected_duration_ms: u64,
    load_profile: LoadProfile,
    load: ResourceLoad,
    rank: TaskRank,
    affinity: bool,
    tie_breaker: u64,
    facts_fresh_until_unix_ms: Option<u64>,
    daily_limit: u32,
    window_iteration_limit: u32,
    max_runtime_ms: u64,
}

enum PlacementResult {
    Candidate(PlacementCandidate),
    Blocked(DecisionReason),
}

struct PlacementContext<'a> {
    profiles: &'a [ActivityProfile],
    pool_specs: &'a BTreeMap<&'a str, &'a PoolSpec>,
    pool_values: &'a BTreeMap<&'a str, &'a PoolValueSnapshot>,
    hosts: &'a BTreeMap<&'a str, HostRemaining>,
    time: EvaluationTime,
    seed: u64,
}

fn build_candidate(
    work_index: usize,
    task: &TaskSpec,
    task_state: Option<&TaskRuntimeSnapshot>,
    instance: &InstanceSnapshot,
    context: &PlacementContext<'_>,
    facts_fresh_until_unix_ms: Option<u64>,
) -> PolicyEvaluationResult<PlacementResult> {
    if !instance.available {
        return Ok(PlacementResult::Blocked(reason(
            "instance_unavailable",
            "the instance is unavailable",
        )));
    }
    if !instance
        .capability_operation_ids
        .iter()
        .any(|operation| operation == &task.entrypoint.operation_id)
    {
        return Ok(PlacementResult::Blocked(reason(
            "capability_missing",
            format!(
                "instance '{}' does not provide operation '{}'",
                instance.instance_id, task.entrypoint.operation_id
            ),
        )));
    }

    let task_override = task
        .instance_overrides
        .iter()
        .find(|candidate| candidate.instance_id == instance.instance_id);
    if task_override
        .and_then(|value| value.enabled.0)
        .is_some_and(|enabled| !enabled)
    {
        return Ok(PlacementResult::Blocked(reason(
            "instance_override_disabled",
            "the task is disabled by its instance override",
        )));
    }

    let load_profile = task_override
        .and_then(|value| value.load_profile.0.clone())
        .unwrap_or_else(|| task.load_profile.clone());
    let load = ResourceLoad::from(&load_profile);
    let host = context
        .hosts
        .get(instance.host_id.as_str())
        .ok_or_else(|| {
            PolicyEvaluationError::invalid(format!(
                "instance '{}' references missing host '{}'",
                instance.instance_id, instance.host_id
            ))
        })?;
    if !host.fits(load) {
        return Ok(PlacementResult::Blocked(reason(
            "host_capacity_insufficient",
            format!(
                "host '{}' cannot satisfy the task load profile",
                instance.host_id
            ),
        )));
    }

    let priority = task_override
        .and_then(|value| value.priority.0)
        .unwrap_or(task.priority);
    let task_weight = task_override
        .and_then(|value| value.strategic_weight_milli.0)
        .unwrap_or(task.strategic_weight_milli);
    let rank = rank_task(
        task,
        task_state,
        instance,
        context.profiles,
        context.pool_specs,
        context.pool_values,
        priority,
        task_weight,
        context.time,
    )?;

    Ok(PlacementResult::Candidate(PlacementCandidate {
        work_index,
        task_id: task.id.clone(),
        instance_id: instance.instance_id.clone(),
        host_id: instance.host_id.clone(),
        operation_id: task.entrypoint.operation_id.clone(),
        procedure_ref: task.procedure_ref.clone(),
        expected_duration_ms: task.expected_duration_ms,
        load_profile,
        load,
        rank,
        affinity: instance
            .preferred_task_ids
            .iter()
            .any(|task_id| task_id == &task.id),
        tie_breaker: deterministic_tie_breaker(context.seed, &task.id, &instance.instance_id),
        facts_fresh_until_unix_ms,
        daily_limit: task.loop_budget.daily_limit,
        window_iteration_limit: task.loop_budget.window_iteration_limit,
        max_runtime_ms: task.loop_budget.max_runtime_ms,
    }))
}

#[allow(clippy::too_many_arguments)]
fn rank_task(
    task: &TaskSpec,
    task_state: Option<&TaskRuntimeSnapshot>,
    instance: &InstanceSnapshot,
    profiles: &[ActivityProfile],
    pool_specs: &BTreeMap<&str, &PoolSpec>,
    pool_values: &BTreeMap<&str, &PoolValueSnapshot>,
    priority: i16,
    task_weight: u16,
    time: EvaluationTime,
) -> PolicyEvaluationResult<TaskRank> {
    let aging_ms = task_state
        .and_then(|state| state.eligible_since_unix_ms)
        .map(|since| time.unix_ms.saturating_sub(since))
        .unwrap_or(0);
    let matching_profiles: Vec<&ActivityProfile> = profiles
        .iter()
        .filter(|profile| scope_matches_instance(&profile.scope, instance))
        .collect();
    let profile_weight = matching_profiles
        .iter()
        .map(|profile| u32::from(profile.importance_milli))
        .max()
        .unwrap_or(0);
    let goal_weight = matching_profiles
        .iter()
        .flat_map(|profile| profile.goals.iter())
        .map(|goal| u32::from(goal.strategic_weight_milli))
        .max()
        .unwrap_or(0);
    let strategic_weight_milli = u32::from(task_weight)
        .saturating_add(profile_weight)
        .saturating_add(goal_weight);
    let deadline_urgency = matching_profiles
        .iter()
        .flat_map(|profile| profile.goals.iter())
        .map(|goal| deadline_urgency_milli(time.unix_ms, goal.deadline_unix_ms))
        .max()
        .unwrap_or(0);
    let resource_urgency = task
        .consumes
        .iter()
        .filter_map(|effect| resource_urgency_milli(effect, pool_specs, pool_values))
        .max()
        .unwrap_or(0);
    let urgency_milli = deadline_urgency.max(resource_urgency);

    let priority_score = i64::from(priority).saturating_mul(1_000_000);
    let aging_score = i64::try_from(aging_ms).unwrap_or(i64::MAX);
    let strategic_score = i64::from(strategic_weight_milli).saturating_mul(1_000);
    let urgency_score = i64::from(urgency_milli).saturating_mul(1_000);
    let total_score = priority_score
        .saturating_add(aging_score)
        .saturating_add(strategic_score)
        .saturating_add(urgency_score);

    Ok(TaskRank {
        priority,
        aging_ms,
        strategic_weight_milli,
        urgency_milli,
        total_score,
    })
}

fn deadline_urgency_milli(now: u64, deadline: u64) -> u16 {
    const HORIZON_MS: u64 = 7 * 24 * 60 * 60 * 1_000;
    if now >= deadline {
        return 1_000;
    }
    let remaining = deadline - now;
    if remaining >= HORIZON_MS {
        return 0;
    }
    u16::try_from((HORIZON_MS - remaining) * 1_000 / HORIZON_MS).unwrap_or(1_000)
}

fn resource_urgency_milli(
    effect: &ResourceEffectSpec,
    specs: &BTreeMap<&str, &PoolSpec>,
    values: &BTreeMap<&str, &PoolValueSnapshot>,
) -> Option<u16> {
    let spec = specs.get(effect.pool_id.as_str())?;
    let value = values.get(effect.pool_id.as_str())?;
    let ratio = value.value.min(spec.capacity).saturating_mul(1_000) / spec.capacity;
    u16::try_from(ratio).ok()
}

#[derive(Debug, Clone, Copy)]
struct ResourceLoad {
    cpu: u16,
    gpu: u16,
    io: u16,
}

impl From<&LoadProfile> for ResourceLoad {
    fn from(value: &LoadProfile) -> Self {
        match value {
            LoadProfile::Light => Self {
                cpu: 100,
                gpu: 100,
                io: 100,
            },
            LoadProfile::Heavy => Self {
                cpu: 700,
                gpu: 700,
                io: 700,
            },
            LoadProfile::Weighted {
                cpu_milli,
                gpu_milli,
                io_milli,
            } => Self {
                cpu: *cpu_milli,
                gpu: *gpu_milli,
                io: *io_milli,
            },
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct HostRemaining {
    cpu: u16,
    gpu: u16,
    io: u16,
}

impl HostRemaining {
    fn fits(&self, load: ResourceLoad) -> bool {
        self.cpu >= load.cpu && self.gpu >= load.gpu && self.io >= load.io
    }

    fn consume(&mut self, load: ResourceLoad) {
        self.cpu -= load.cpu;
        self.gpu -= load.gpu;
        self.io -= load.io;
    }
}

impl From<&HostResourceSnapshot> for HostRemaining {
    fn from(value: &HostResourceSnapshot) -> Self {
        Self {
            cpu: value.cpu_available_milli,
            gpu: value.gpu_available_milli,
            io: value.io_available_milli,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PredicateTruth {
    True,
    False,
    Unknown,
}

#[derive(Debug)]
struct PredicateEvaluation {
    truth: PredicateTruth,
    suggestions: Vec<DetectionSuggestion>,
    next_wake_unix_ms: Option<u64>,
    fresh_until_unix_ms: Option<u64>,
}

impl PredicateEvaluation {
    fn known(truth: bool, next_wake_unix_ms: Option<u64>) -> Self {
        Self {
            truth: if truth {
                PredicateTruth::True
            } else {
                PredicateTruth::False
            },
            suggestions: Vec::new(),
            next_wake_unix_ms,
            fresh_until_unix_ms: None,
        }
    }

    fn unknown(suggestion: DetectionSuggestion) -> Self {
        Self {
            truth: PredicateTruth::Unknown,
            suggestions: vec![suggestion],
            next_wake_unix_ms: None,
            fresh_until_unix_ms: None,
        }
    }
}

fn evaluate_predicate(
    predicate: &PredicateSpec,
    task_state: Option<&TaskRuntimeSnapshot>,
    decision_scope: &ScopeSelector,
    facts: &EvaluationFacts,
    pool_specs: &BTreeMap<&str, &PoolSpec>,
    pool_values: &BTreeMap<&str, &PoolValueSnapshot>,
    time: EvaluationTime,
) -> PolicyEvaluationResult<PredicateEvaluation> {
    match predicate {
        PredicateSpec::All { predicates } => {
            let mut suggestions = Vec::new();
            let mut next_wake = None;
            let mut fresh_until = None;
            let mut unknown = false;
            for child in predicates {
                let result = evaluate_predicate(
                    child,
                    task_state,
                    decision_scope,
                    facts,
                    pool_specs,
                    pool_values,
                    time,
                )?;
                next_wake = min_wake(next_wake, result.next_wake_unix_ms);
                fresh_until = min_wake(fresh_until, result.fresh_until_unix_ms);
                match result.truth {
                    PredicateTruth::False => {
                        return Ok(PredicateEvaluation::known(false, next_wake));
                    }
                    PredicateTruth::Unknown => {
                        unknown = true;
                        suggestions.extend(result.suggestions);
                    }
                    PredicateTruth::True => {}
                }
            }
            Ok(PredicateEvaluation {
                truth: if unknown {
                    PredicateTruth::Unknown
                } else {
                    PredicateTruth::True
                },
                suggestions,
                next_wake_unix_ms: next_wake,
                fresh_until_unix_ms: fresh_until,
            })
        }
        PredicateSpec::Any { predicates } => {
            let mut suggestions = Vec::new();
            let mut next_wake = None;
            let mut unknown = false;
            for child in predicates {
                let result = evaluate_predicate(
                    child,
                    task_state,
                    decision_scope,
                    facts,
                    pool_specs,
                    pool_values,
                    time,
                )?;
                next_wake = min_wake(next_wake, result.next_wake_unix_ms);
                match result.truth {
                    PredicateTruth::True => {
                        return Ok(PredicateEvaluation {
                            truth: PredicateTruth::True,
                            suggestions: Vec::new(),
                            next_wake_unix_ms: next_wake,
                            fresh_until_unix_ms: result.fresh_until_unix_ms,
                        });
                    }
                    PredicateTruth::Unknown => {
                        unknown = true;
                        suggestions.extend(result.suggestions);
                    }
                    PredicateTruth::False => {}
                }
            }
            Ok(PredicateEvaluation {
                truth: if unknown {
                    PredicateTruth::Unknown
                } else {
                    PredicateTruth::False
                },
                suggestions,
                next_wake_unix_ms: next_wake,
                fresh_until_unix_ms: None,
            })
        }
        PredicateSpec::Not { predicate } => {
            let mut result = evaluate_predicate(
                predicate,
                task_state,
                decision_scope,
                facts,
                pool_specs,
                pool_values,
                time,
            )?;
            result.truth = match result.truth {
                PredicateTruth::True => PredicateTruth::False,
                PredicateTruth::False => PredicateTruth::True,
                PredicateTruth::Unknown => PredicateTruth::Unknown,
            };
            Ok(result)
        }
        PredicateSpec::Clock { schedule } => {
            let (latest, next) = schedule_occurrences(schedule, time.unix_ms);
            let last_dispatched = task_state.and_then(|state| state.last_dispatched_unix_ms);
            let due = latest
                .is_some_and(|occurrence| last_dispatched.is_none_or(|last| last < occurrence));
            Ok(PredicateEvaluation::known(due, next))
        }
        PredicateSpec::ResourceProjection {
            pool_id,
            comparison,
            value,
        } => {
            let spec = pool_specs.get(pool_id.as_str()).ok_or_else(|| {
                PolicyEvaluationError::invalid(format!(
                    "compiled catalog references missing pool '{pool_id}'"
                ))
            })?;
            let Some(snapshot) = pool_values.get(pool_id.as_str()) else {
                return Ok(PredicateEvaluation::unknown(resource_suggestion(spec)));
            };
            let projected = project_pool_value(spec, snapshot, time.unix_ms)?;
            let projected = i64::try_from(projected).map_err(|_| {
                PolicyEvaluationError::overflow(format!(
                    "projected pool '{pool_id}' value exceeds i64"
                ))
            })?;
            Ok(PredicateEvaluation::known(
                compare_i64(projected, *comparison, *value)?,
                next_pool_projection_change(spec, snapshot, time.unix_ms)?,
            ))
        }
        PredicateSpec::Fact {
            scope,
            fact_key,
            comparison,
            value,
            max_age_ms,
        } => {
            let observation = facts
                .facts
                .iter()
                .find(|fact| fact.scope == *scope && fact.fact_key == *fact_key);
            let Some(observation) = observation else {
                return Ok(PredicateEvaluation::unknown(DetectionSuggestion {
                    scope: scope.clone(),
                    fact_key: fact_key.clone(),
                    reason: "fact_missing".to_owned(),
                }));
            };
            let fresh_until = max_age_ms
                .map(|max_age| {
                    observation
                        .observed_at_unix_ms
                        .checked_add(max_age)
                        .ok_or_else(|| {
                            PolicyEvaluationError::overflow(format!(
                                "fact '{}:{}' expiration overflowed",
                                scope_key(scope),
                                fact_key
                            ))
                        })
                })
                .transpose()?;
            if fresh_until.is_some_and(|expiration| time.unix_ms > expiration) {
                return Ok(PredicateEvaluation::unknown(DetectionSuggestion {
                    scope: scope.clone(),
                    fact_key: fact_key.clone(),
                    reason: "fact_stale".to_owned(),
                }));
            }
            Ok(PredicateEvaluation {
                truth: if compare_fact_values(&observation.value, *comparison, value)? {
                    PredicateTruth::True
                } else {
                    PredicateTruth::False
                },
                suggestions: Vec::new(),
                next_wake_unix_ms: fresh_until
                    .and_then(|expiration| expiration.checked_add(1))
                    .filter(|expiration| *expiration > time.unix_ms),
                fresh_until_unix_ms: fresh_until,
            })
        }
        PredicateSpec::DependencyCompleted {
            task_id,
            terminal_states,
        } => {
            let state = facts.tasks.iter().find(|state| {
                state.task_id == *task_id
                    && scope_instance_id(decision_scope)
                        .is_some_and(|instance_id| state.instance_id == instance_id)
            });
            let Some(terminal_state) = state.and_then(|state| state.terminal_state) else {
                return Ok(PredicateEvaluation::unknown(DetectionSuggestion {
                    scope: decision_scope.clone(),
                    fact_key: format!("task.{task_id}.terminal_state"),
                    reason: "dependency_state_missing".to_owned(),
                }));
            };
            Ok(PredicateEvaluation::known(
                terminal_states.contains(&terminal_state),
                None,
            ))
        }
        PredicateSpec::Outcome {
            task_id,
            outcome_key,
            comparison,
            value,
        } => {
            let observation = facts.outcomes.iter().find(|outcome| {
                outcome.task_id == *task_id
                    && outcome.outcome_key == *outcome_key
                    && scope_instance_id(decision_scope)
                        .is_some_and(|instance_id| outcome.instance_id == instance_id)
            });
            let Some(observation) = observation else {
                return Ok(PredicateEvaluation::unknown(DetectionSuggestion {
                    scope: decision_scope.clone(),
                    fact_key: format!("outcome.{task_id}.{outcome_key}"),
                    reason: "outcome_missing".to_owned(),
                }));
            };
            Ok(PredicateEvaluation::known(
                compare_fact_values(&observation.value, *comparison, value)?,
                None,
            ))
        }
    }
}

fn scope_instance_id(scope: &ScopeSelector) -> Option<&str> {
    match scope {
        ScopeSelector::Instance { instance_id } => Some(instance_id),
        ScopeSelector::Server { .. } | ScopeSelector::Game { .. } => None,
    }
}

fn resource_suggestion(pool: &PoolSpec) -> DetectionSuggestion {
    match &pool.observation {
        ObservationRef::Fact { fact_key } => DetectionSuggestion {
            scope: pool.scope.clone(),
            fact_key: fact_key.clone(),
            reason: "resource_observation_missing".to_owned(),
        },
        ObservationRef::Outcome {
            task_id,
            outcome_key,
        } => DetectionSuggestion {
            scope: pool.scope.clone(),
            fact_key: format!("outcome.{task_id}.{outcome_key}"),
            reason: "resource_outcome_missing".to_owned(),
        },
    }
}

fn project_pool_value(
    spec: &PoolSpec,
    snapshot: &PoolValueSnapshot,
    now: u64,
) -> PolicyEvaluationResult<u64> {
    if snapshot.value > spec.capacity {
        return Err(PolicyEvaluationError::invalid(format!(
            "pool '{}' value {} exceeds capacity {}",
            spec.id, snapshot.value, spec.capacity
        )));
    }
    let elapsed = now.saturating_sub(snapshot.observed_at_unix_ms);
    let periods = elapsed / spec.projection.per_ms;
    let regenerated = periods.checked_mul(spec.projection.amount).ok_or_else(|| {
        PolicyEvaluationError::overflow(format!(
            "pool '{}' projection multiplication overflowed",
            spec.id
        ))
    })?;
    Ok(snapshot
        .value
        .saturating_add(regenerated)
        .min(spec.capacity))
}

fn next_pool_projection_change(
    spec: &PoolSpec,
    snapshot: &PoolValueSnapshot,
    now: u64,
) -> PolicyEvaluationResult<Option<u64>> {
    if spec.projection.amount == 0 || project_pool_value(spec, snapshot, now)? >= spec.capacity {
        return Ok(None);
    }
    let elapsed = now.saturating_sub(snapshot.observed_at_unix_ms);
    let next_period = elapsed
        .checked_div(spec.projection.per_ms)
        .and_then(|period| period.checked_add(1))
        .ok_or_else(|| {
            PolicyEvaluationError::overflow(format!(
                "pool '{}' next projection period overflowed",
                spec.id
            ))
        })?;
    let offset = next_period
        .checked_mul(spec.projection.per_ms)
        .ok_or_else(|| {
            PolicyEvaluationError::overflow(format!(
                "pool '{}' next projection offset overflowed",
                spec.id
            ))
        })?;
    snapshot
        .observed_at_unix_ms
        .checked_add(offset)
        .map(Some)
        .ok_or_else(|| {
            PolicyEvaluationError::overflow(format!(
                "pool '{}' next projection time overflowed",
                spec.id
            ))
        })
}

fn compare_fact_values(
    actual: &FactValue,
    comparison: Comparison,
    expected: &FactValue,
) -> PolicyEvaluationResult<bool> {
    match comparison {
        Comparison::Eq | Comparison::NotEq => {
            if fact_kind(actual) != fact_kind(expected) {
                return Err(PolicyEvaluationError::type_mismatch(format!(
                    "cannot compare {} with {}",
                    fact_kind(actual),
                    fact_kind(expected)
                )));
            }
            let equal = actual == expected;
            Ok(if comparison == Comparison::Eq {
                equal
            } else {
                !equal
            })
        }
        Comparison::Contains => match (actual, expected) {
            (FactValue::String(actual), FactValue::String(expected)) => {
                Ok(actual.contains(expected))
            }
            _ => Err(PolicyEvaluationError::type_mismatch(
                "contains requires two string facts",
            )),
        },
        Comparison::LessThan
        | Comparison::LessThanOrEqual
        | Comparison::GreaterThan
        | Comparison::GreaterThanOrEqual => {
            let (actual, expected) = comparable_i128(actual, expected)?;
            compare_ordered(actual, comparison, expected)
        }
    }
}

fn comparable_i128(
    actual: &FactValue,
    expected: &FactValue,
) -> PolicyEvaluationResult<(i128, i128)> {
    match (actual, expected) {
        (FactValue::Integer(actual), FactValue::Integer(expected)) => {
            Ok((i128::from(*actual), i128::from(*expected)))
        }
        (FactValue::TimestampMs(actual), FactValue::TimestampMs(expected))
        | (FactValue::DurationMs(actual), FactValue::DurationMs(expected)) => {
            Ok((i128::from(*actual), i128::from(*expected)))
        }
        _ => Err(PolicyEvaluationError::type_mismatch(format!(
            "ordered comparison requires matching numeric fact kinds, got {} and {}",
            fact_kind(actual),
            fact_kind(expected)
        ))),
    }
}

fn compare_i64(actual: i64, comparison: Comparison, expected: i64) -> PolicyEvaluationResult<bool> {
    match comparison {
        Comparison::Eq => Ok(actual == expected),
        Comparison::NotEq => Ok(actual != expected),
        Comparison::LessThan => Ok(actual < expected),
        Comparison::LessThanOrEqual => Ok(actual <= expected),
        Comparison::GreaterThan => Ok(actual > expected),
        Comparison::GreaterThanOrEqual => Ok(actual >= expected),
        Comparison::Contains => Err(PolicyEvaluationError::type_mismatch(
            "contains is invalid for resource projections",
        )),
    }
}

fn compare_ordered(
    actual: i128,
    comparison: Comparison,
    expected: i128,
) -> PolicyEvaluationResult<bool> {
    match comparison {
        Comparison::LessThan => Ok(actual < expected),
        Comparison::LessThanOrEqual => Ok(actual <= expected),
        Comparison::GreaterThan => Ok(actual > expected),
        Comparison::GreaterThanOrEqual => Ok(actual >= expected),
        _ => Err(PolicyEvaluationError::type_mismatch(
            "comparison is not ordered",
        )),
    }
}

fn fact_kind(value: &FactValue) -> &'static str {
    match value {
        FactValue::Boolean(_) => "boolean",
        FactValue::Integer(_) => "integer",
        FactValue::String(_) => "string",
        FactValue::TimestampMs(_) => "timestamp_ms",
        FactValue::DurationMs(_) => "duration_ms",
    }
}

fn scope_matches_instance(scope: &ScopeSelector, instance: &InstanceSnapshot) -> bool {
    match scope {
        ScopeSelector::Instance { instance_id } => instance_id == &instance.instance_id,
        ScopeSelector::Server { server_id } => server_id == &instance.server_id,
        ScopeSelector::Game { game_id } => game_id == &instance.game_id,
    }
}

fn schedule_occurrences(schedule: &ClockSchedule, now: u64) -> (Option<u64>, Option<u64>) {
    match schedule {
        ClockSchedule::Interval {
            every_ms,
            anchor_unix_ms,
        } => {
            if now < *anchor_unix_ms {
                return (None, Some(*anchor_unix_ms));
            }
            let elapsed = now - *anchor_unix_ms;
            let latest = anchor_unix_ms.saturating_add((elapsed / every_ms) * every_ms);
            let next = latest.checked_add(*every_ms);
            (Some(latest), next)
        }
        ClockSchedule::At { unix_ms } => {
            if now < *unix_ms {
                (None, Some(*unix_ms))
            } else {
                (Some(*unix_ms), None)
            }
        }
        ClockSchedule::Daily {
            utc_offset_minutes,
            minutes_of_day,
        } => daily_occurrences(*utc_offset_minutes, minutes_of_day, now),
        ClockSchedule::Weekly {
            utc_offset_minutes,
            weekday,
            minute_of_day,
        } => weekly_occurrences(*utc_offset_minutes, *weekday, *minute_of_day, now),
    }
}

fn next_schedule_occurrence(schedule: &ClockSchedule, now: u64) -> Option<u64> {
    schedule_occurrences(schedule, now).1
}

fn daily_occurrences(offset_minutes: i16, minutes: &[u16], now: u64) -> (Option<u64>, Option<u64>) {
    const DAY_MS: i128 = 86_400_000;
    let offset_ms = i128::from(offset_minutes) * 60_000;
    let local_now = i128::from(now) + offset_ms;
    let day = local_now.div_euclid(DAY_MS);
    let within_day = local_now.rem_euclid(DAY_MS);
    let current_minute = within_day / 60_000;

    let latest_minute = minutes
        .iter()
        .copied()
        .map(i128::from)
        .filter(|minute| *minute <= current_minute)
        .max();
    let latest_local = latest_minute
        .map(|minute| day * DAY_MS + minute * 60_000)
        .or_else(|| {
            minutes
                .iter()
                .copied()
                .map(i128::from)
                .max()
                .map(|minute| (day - 1) * DAY_MS + minute * 60_000)
        });
    let next_local = minutes
        .iter()
        .copied()
        .map(i128::from)
        .filter(|minute| *minute > current_minute)
        .min()
        .map(|minute| day * DAY_MS + minute * 60_000)
        .or_else(|| {
            minutes
                .iter()
                .copied()
                .map(i128::from)
                .min()
                .map(|minute| (day + 1) * DAY_MS + minute * 60_000)
        });
    (
        latest_local.and_then(|value| local_to_unix(value, offset_ms)),
        next_local.and_then(|value| local_to_unix(value, offset_ms)),
    )
}

fn weekly_occurrences(
    offset_minutes: i16,
    weekday: u8,
    minute_of_day: u16,
    now: u64,
) -> (Option<u64>, Option<u64>) {
    const DAY_MS: i128 = 86_400_000;
    let offset_ms = i128::from(offset_minutes) * 60_000;
    let local_now = i128::from(now) + offset_ms;
    let day = local_now.div_euclid(DAY_MS);
    let current_minute = local_now.rem_euclid(DAY_MS) / 60_000;
    let current_weekday = (day + 3).rem_euclid(7) + 1;
    let target_weekday = i128::from(weekday);
    let target_minute = i128::from(minute_of_day);

    let mut days_since = (current_weekday - target_weekday).rem_euclid(7);
    if days_since == 0 && current_minute < target_minute {
        days_since = 7;
    }
    let latest_local = (day - days_since) * DAY_MS + target_minute * 60_000;

    let mut days_ahead = (target_weekday - current_weekday).rem_euclid(7);
    if days_ahead == 0 && target_minute <= current_minute {
        days_ahead = 7;
    }
    let next_local = (day + days_ahead) * DAY_MS + target_minute * 60_000;
    (
        local_to_unix(latest_local, offset_ms),
        local_to_unix(next_local, offset_ms),
    )
}

fn local_to_unix(local_ms: i128, offset_ms: i128) -> Option<u64> {
    u64::try_from(local_ms - offset_ms).ok()
}

fn min_wake(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

fn deterministic_tie_breaker(seed: u64, task_id: &str, instance_id: &str) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(seed.to_be_bytes());
    hash_part(&mut hasher, task_id.as_bytes());
    hash_part(&mut hasher, instance_id.as_bytes());
    let digest = hasher.finalize();
    u64::from_be_bytes(digest[..8].try_into().expect("sha256 prefix"))
}

fn deterministic_decision_id(
    catalog_hash: &str,
    task_id: &str,
    instance_id: &str,
    ledger_position: u64,
    unix_ms: u64,
    seed: u64,
) -> String {
    let mut hasher = Sha256::new();
    hash_part(&mut hasher, catalog_hash.as_bytes());
    hash_part(&mut hasher, task_id.as_bytes());
    hash_part(&mut hasher, instance_id.as_bytes());
    hasher.update(ledger_position.to_be_bytes());
    hasher.update(unix_ms.to_be_bytes());
    hasher.update(seed.to_be_bytes());
    format!("decision:{:x}", hasher.finalize())
}

fn hash_part(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

fn reason(code: impl Into<String>, detail: impl Into<String>) -> DecisionReason {
    DecisionReason {
        code: code.into(),
        detail: detail.into(),
    }
}

fn normalize_suggestions(suggestions: &mut Vec<DetectionSuggestion>) {
    suggestions.sort_by(|left, right| {
        scope_key(&left.scope)
            .cmp(&scope_key(&right.scope))
            .then_with(|| left.fact_key.cmp(&right.fact_key))
            .then_with(|| left.reason.cmp(&right.reason))
    });
    suggestions.dedup();
}

fn scope_key(scope: &ScopeSelector) -> String {
    match scope {
        ScopeSelector::Instance { instance_id } => format!("instance:{instance_id}"),
        ScopeSelector::Server { server_id } => format!("server:{server_id}"),
        ScopeSelector::Game { game_id } => format!("game:{game_id}"),
    }
}

fn validate_inputs(
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    resources: &EvaluationResources,
    time: EvaluationTime,
) -> PolicyEvaluationResult<()> {
    validate_count("facts", facts.facts.len(), MAX_EVALUATION_FACTS)?;
    validate_count("outcomes", facts.outcomes.len(), MAX_EVALUATION_OUTCOMES)?;
    validate_count("task states", facts.tasks.len(), MAX_EVALUATION_TASK_STATES)?;
    validate_count("instances", facts.instances.len(), MAX_EVALUATION_INSTANCES)?;
    validate_count("pools", resources.pools.len(), MAX_EVALUATION_POOLS)?;
    validate_count("hosts", resources.hosts.len(), MAX_EVALUATION_HOSTS)?;
    validate_id("fact snapshot id", &facts.fact_snapshot_id)?;

    let task_ids: BTreeSet<&str> = catalog
        .catalog()
        .tasks
        .tasks
        .iter()
        .map(|task| task.id.as_str())
        .collect();

    let mut host_ids = BTreeSet::new();
    for host in &resources.hosts {
        validate_id("host id", &host.host_id)?;
        if !host_ids.insert(host.host_id.as_str()) {
            return Err(PolicyEvaluationError::invalid(format!(
                "duplicate host '{}'",
                host.host_id
            )));
        }
        if [
            host.cpu_available_milli,
            host.gpu_available_milli,
            host.io_available_milli,
        ]
        .into_iter()
        .any(|value| value > 1_000)
        {
            return Err(PolicyEvaluationError::invalid(format!(
                "host '{}' resources must be within 0..=1000",
                host.host_id
            )));
        }
    }

    let mut instance_ids = BTreeSet::new();
    for instance in &facts.instances {
        validate_id("instance id", &instance.instance_id)?;
        validate_id("instance server id", &instance.server_id)?;
        validate_id("instance game id", &instance.game_id)?;
        validate_id("instance host id", &instance.host_id)?;
        if !instance_ids.insert(instance.instance_id.as_str()) {
            return Err(PolicyEvaluationError::invalid(format!(
                "duplicate instance '{}'",
                instance.instance_id
            )));
        }
        if !host_ids.contains(instance.host_id.as_str()) {
            return Err(PolicyEvaluationError::invalid(format!(
                "instance '{}' references missing host '{}'",
                instance.instance_id, instance.host_id
            )));
        }
        validate_unique_ids("instance capability", &instance.capability_operation_ids)?;
        validate_unique_ids("instance affinity", &instance.preferred_task_ids)?;
        for task_id in &instance.preferred_task_ids {
            if !task_ids.contains(task_id.as_str()) {
                return Err(PolicyEvaluationError::invalid(format!(
                    "instance '{}' prefers unknown task '{}'",
                    instance.instance_id, task_id
                )));
            }
        }
    }

    let mut fact_keys = BTreeSet::new();
    for fact in &facts.facts {
        validate_id("fact key", &fact.fact_key)?;
        validate_input_scope("fact", &fact.scope, &instance_ids)?;
        validate_observation_time("fact", fact.observed_at_unix_ms, time.unix_ms)?;
        if !fact_keys.insert((scope_key(&fact.scope), fact.fact_key.as_str())) {
            return Err(PolicyEvaluationError::invalid(format!(
                "duplicate fact '{}:{}'",
                scope_key(&fact.scope),
                fact.fact_key
            )));
        }
    }

    let mut outcome_keys = BTreeSet::new();
    for outcome in &facts.outcomes {
        validate_id("outcome task id", &outcome.task_id)?;
        validate_id("outcome instance id", &outcome.instance_id)?;
        validate_id("outcome key", &outcome.outcome_key)?;
        if !task_ids.contains(outcome.task_id.as_str()) {
            return Err(PolicyEvaluationError::invalid(format!(
                "outcome references unknown task '{}'",
                outcome.task_id
            )));
        }
        if !instance_ids.contains(outcome.instance_id.as_str()) {
            return Err(PolicyEvaluationError::invalid(format!(
                "outcome references unknown instance '{}'",
                outcome.instance_id
            )));
        }
        validate_observation_time("outcome", outcome.observed_at_unix_ms, time.unix_ms)?;
        if !outcome_keys.insert((
            outcome.task_id.as_str(),
            outcome.instance_id.as_str(),
            outcome.outcome_key.as_str(),
        )) {
            return Err(PolicyEvaluationError::invalid(format!(
                "duplicate outcome '{}:{}:{}'",
                outcome.task_id, outcome.instance_id, outcome.outcome_key
            )));
        }
    }

    let mut state_ids = BTreeSet::new();
    for state in &facts.tasks {
        validate_id("task state id", &state.task_id)?;
        validate_id("task state instance id", &state.instance_id)?;
        if !task_ids.contains(state.task_id.as_str()) {
            return Err(PolicyEvaluationError::invalid(format!(
                "task state references unknown task '{}'",
                state.task_id
            )));
        }
        if !instance_ids.contains(state.instance_id.as_str()) {
            return Err(PolicyEvaluationError::invalid(format!(
                "task state references unknown instance '{}'",
                state.instance_id
            )));
        }
        if !state_ids.insert((state.task_id.as_str(), state.instance_id.as_str())) {
            return Err(PolicyEvaluationError::invalid(format!(
                "duplicate task state '{}:{}'",
                state.task_id, state.instance_id
            )));
        }
        for timestamp in [state.last_dispatched_unix_ms, state.eligible_since_unix_ms]
            .into_iter()
            .flatten()
        {
            validate_observation_time("task state", timestamp, time.unix_ms)?;
        }
    }

    let pool_specs: BTreeMap<&str, &PoolSpec> = catalog
        .catalog()
        .pools
        .pools
        .iter()
        .map(|pool| (pool.id.as_str(), pool))
        .collect();
    let mut observed_pool_ids = BTreeSet::new();
    for pool in &resources.pools {
        validate_id("pool id", &pool.pool_id)?;
        validate_observation_time("pool", pool.observed_at_unix_ms, time.unix_ms)?;
        let Some(spec) = pool_specs.get(pool.pool_id.as_str()) else {
            return Err(PolicyEvaluationError::invalid(format!(
                "pool snapshot references unknown pool '{}'",
                pool.pool_id
            )));
        };
        if pool.value > spec.capacity {
            return Err(PolicyEvaluationError::invalid(format!(
                "pool '{}' value {} exceeds capacity {}",
                pool.pool_id, pool.value, spec.capacity
            )));
        }
        if !observed_pool_ids.insert(pool.pool_id.as_str()) {
            return Err(PolicyEvaluationError::invalid(format!(
                "duplicate pool snapshot '{}'",
                pool.pool_id
            )));
        }
    }
    Ok(())
}

fn validate_input_scope(
    label: &str,
    scope: &ScopeSelector,
    instance_ids: &BTreeSet<&str>,
) -> PolicyEvaluationResult<()> {
    match scope {
        ScopeSelector::Instance { instance_id } => {
            validate_id(&format!("{label} instance id"), instance_id)?;
            if !instance_ids.contains(instance_id.as_str()) {
                return Err(PolicyEvaluationError::invalid(format!(
                    "{label} scope references unknown instance '{instance_id}'"
                )));
            }
        }
        ScopeSelector::Server { server_id } => {
            validate_id(&format!("{label} server id"), server_id)?;
        }
        ScopeSelector::Game { game_id } => {
            validate_id(&format!("{label} game id"), game_id)?;
        }
    }
    Ok(())
}

fn validate_count(label: &str, count: usize, maximum: usize) -> PolicyEvaluationResult<()> {
    if count > maximum {
        return Err(PolicyEvaluationError::invalid(format!(
            "{label} count {count} exceeds {maximum}"
        )));
    }
    Ok(())
}

fn validate_observation_time(label: &str, observed: u64, now: u64) -> PolicyEvaluationResult<()> {
    if observed > now {
        return Err(PolicyEvaluationError::invalid(format!(
            "{label} timestamp {observed} is later than evaluation time {now}"
        )));
    }
    Ok(())
}

fn validate_unique_ids(label: &str, values: &[String]) -> PolicyEvaluationResult<()> {
    let mut unique = BTreeSet::new();
    for value in values {
        validate_id(label, value)?;
        if !unique.insert(value.as_str()) {
            return Err(PolicyEvaluationError::invalid(format!(
                "duplicate {label} '{value}'"
            )));
        }
    }
    Ok(())
}

fn validate_id(label: &str, value: &str) -> PolicyEvaluationResult<()> {
    if value.trim().is_empty() || value.len() > crate::MAX_ID_BYTES {
        return Err(PolicyEvaluationError::invalid(format!(
            "{label} must contain 1 to {} bytes",
            crate::MAX_ID_BYTES
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CatalogDocumentSource, CatalogSources, compile_catalog};

    const NOW: u64 = 3_600_000;

    #[test]
    fn same_inputs_produce_byte_stable_decisions() {
        let catalog = catalog(|tasks| {
            tasks["tasks"][0]["trigger"] = due_clock();
            tasks["tasks"][0]["feedback_stop"] = false_fact();
        });
        let facts = base_facts();
        let resources = base_resources();

        let first = evaluate(
            &catalog,
            &facts,
            &resources,
            EvaluationTime { unix_ms: NOW },
            7,
        )
        .expect("first evaluation");
        let second = evaluate(
            &catalog,
            &facts,
            &resources,
            EvaluationTime { unix_ms: NOW },
            7,
        )
        .expect("second evaluation");

        assert_eq!(first, second);
        assert_eq!(
            serde_json::to_vec(&first).expect("first JSON"),
            serde_json::to_vec(&second).expect("second JSON")
        );
        assert_eq!(first.dispatch_intents.len(), 1);
        let intent = &first.dispatch_intents[0];
        assert!(intent.decision_id.starts_with("decision:"));
        assert_eq!(intent.catalog_version, 1);
        assert_eq!(intent.fact_snapshot_id, "snapshot:fixture-a");
        assert_eq!(intent.approval_refs, ["approval:fixture-a"]);
        assert!(intent.prerequisites.fencing_required);
        assert_eq!(first.decisions[0].state, SchedulingDecisionState::Selected);
    }

    #[test]
    fn unknown_fact_stays_unknown_and_requests_detection() {
        let catalog = catalog(|tasks| {
            tasks["tasks"][0]["trigger"] = serde_json::json!({
                "kind": "fact",
                "scope": {"kind": "instance", "instance_id": "fixture-instance-a"},
                "fact_key": "fixture.ready",
                "comparison": "eq",
                "value": {"type": "boolean", "value": true},
                "max_age_ms": 1000
            });
            tasks["tasks"][0]["feedback_stop"] = false_fact();
        });

        let result = evaluate(
            &catalog,
            &base_facts(),
            &base_resources(),
            EvaluationTime { unix_ms: NOW },
            1,
        )
        .expect("evaluation");

        assert_eq!(result.decisions[0].eligibility, EligibilityState::Unknown);
        assert_eq!(result.decisions[0].state, SchedulingDecisionState::Deferred);
        assert_eq!(result.decisions[0].detection_suggestions.len(), 1);
        assert_eq!(
            result.decisions[0].detection_suggestions[0].reason,
            "fact_missing"
        );
        assert!(result.dispatch_intents.is_empty());
    }

    #[test]
    fn aging_eventually_prevents_lower_priority_starvation() {
        let catalog = two_task_catalog(|tasks| {
            tasks[0]["priority"] = serde_json::json!(100);
            tasks[1]["priority"] = serde_json::json!(0);
            tasks[0]["trigger"] = due_clock();
            tasks[1]["trigger"] = due_clock();
            tasks[0]["feedback_stop"] = false_fact();
            tasks[1]["feedback_stop"] = false_fact();
        });
        let late_time = EvaluationTime {
            unix_ms: 200_000_000,
        };
        let mut facts = base_facts();
        facts.tasks = vec![
            TaskRuntimeSnapshot {
                task_id: "fixture.observe".to_owned(),
                instance_id: "fixture-instance-a".to_owned(),
                last_dispatched_unix_ms: None,
                eligible_since_unix_ms: Some(late_time.unix_ms - 1),
                terminal_state: None,
            },
            TaskRuntimeSnapshot {
                task_id: "fixture.observe-secondary".to_owned(),
                instance_id: "fixture-instance-a".to_owned(),
                last_dispatched_unix_ms: None,
                eligible_since_unix_ms: Some(0),
                terminal_state: None,
            },
        ];
        let result =
            evaluate(&catalog, &facts, &base_resources(), late_time, 3).expect("evaluation");

        assert_eq!(
            result.dispatch_intents[0].task_id,
            "fixture.observe-secondary"
        );
    }

    #[test]
    fn placement_enforces_scope_capability_affinity_and_host_budget() {
        let catalog = catalog(|tasks| {
            tasks["tasks"][0]["scope"] =
                serde_json::json!({"kind": "server", "server_id": "fixture-server-a"});
            tasks["tasks"][0]["trigger"] = due_clock();
            tasks["tasks"][0]["feedback_stop"] = false_fact();
        });
        let mut facts = base_facts();
        facts.instances.push(InstanceSnapshot {
            instance_id: "fixture-instance-b".to_owned(),
            server_id: "fixture-server-a".to_owned(),
            game_id: "fixture-game-a".to_owned(),
            host_id: "fixture-host-a".to_owned(),
            available: true,
            capability_operation_ids: vec!["operation.observe".to_owned()],
            preferred_task_ids: vec!["fixture.observe".to_owned()],
        });
        let mut resources = base_resources();
        resources.hosts[0].cpu_available_milli = 200;
        resources.hosts[0].gpu_available_milli = 100;
        resources.hosts[0].io_available_milli = 300;

        let result = evaluate(
            &catalog,
            &facts,
            &resources,
            EvaluationTime { unix_ms: NOW },
            4,
        )
        .expect("evaluation");

        assert_eq!(result.dispatch_intents.len(), 1);
        assert_eq!(result.dispatch_intents[0].instance_id, "fixture-instance-b");
        let instance_a = decision_for(&result, "fixture.observe", "fixture-instance-a");
        let instance_b = decision_for(&result, "fixture.observe", "fixture-instance-b");
        assert_eq!(instance_a.state, SchedulingDecisionState::Eligible);
        assert_eq!(instance_b.state, SchedulingDecisionState::Selected);
    }

    #[test]
    fn outcomes_and_dispatches_remain_instance_scoped() {
        let catalog = catalog(|tasks| {
            tasks["tasks"][0]["scope"] =
                serde_json::json!({"kind": "server", "server_id": "fixture-server-a"});
            tasks["tasks"][0]["trigger"] = due_clock();
        });
        let mut facts = base_facts();
        facts.instances.push(InstanceSnapshot {
            instance_id: "fixture-instance-b".to_owned(),
            server_id: "fixture-server-a".to_owned(),
            game_id: "fixture-game-a".to_owned(),
            host_id: "fixture-host-a".to_owned(),
            available: true,
            capability_operation_ids: vec!["operation.observe".to_owned()],
            preferred_task_ids: Vec::new(),
        });
        facts.outcomes = vec![
            ObservedOutcome {
                task_id: "fixture.observe".to_owned(),
                instance_id: "fixture-instance-a".to_owned(),
                outcome_key: "completed".to_owned(),
                value: FactValue::Boolean(true),
                observed_at_unix_ms: NOW,
            },
            ObservedOutcome {
                task_id: "fixture.observe".to_owned(),
                instance_id: "fixture-instance-b".to_owned(),
                outcome_key: "completed".to_owned(),
                value: FactValue::Boolean(false),
                observed_at_unix_ms: NOW,
            },
        ];

        let result = evaluate(
            &catalog,
            &facts,
            &base_resources(),
            EvaluationTime { unix_ms: NOW },
            9,
        )
        .expect("evaluation");

        assert_eq!(result.dispatch_intents.len(), 1);
        assert_eq!(result.dispatch_intents[0].instance_id, "fixture-instance-b");
        let instance_a = decision_for(&result, "fixture.observe", "fixture-instance-a");
        let instance_b = decision_for(&result, "fixture.observe", "fixture-instance-b");
        assert_eq!(instance_a.eligibility, EligibilityState::False);
        assert_eq!(instance_a.state, SchedulingDecisionState::Blocked);
        assert_eq!(instance_b.eligibility, EligibilityState::True);
        assert_eq!(instance_b.state, SchedulingDecisionState::Selected);
    }

    #[test]
    fn unavailable_instance_is_explicitly_blocked() {
        let catalog = catalog(|tasks| {
            tasks["tasks"][0]["trigger"] = due_clock();
            tasks["tasks"][0]["feedback_stop"] = false_fact();
        });
        let mut facts = base_facts();
        facts.instances[0].available = false;

        let result = evaluate(
            &catalog,
            &facts,
            &base_resources(),
            EvaluationTime { unix_ms: NOW },
            10,
        )
        .expect("evaluation");

        assert_eq!(result.decisions[0].eligibility, EligibilityState::True);
        assert_eq!(result.decisions[0].state, SchedulingDecisionState::Blocked);
        assert_eq!(result.decisions[0].reasons[1].code, "instance_unavailable");
        assert!(result.dispatch_intents.is_empty());
    }

    #[test]
    fn dispatch_intent_pins_fact_freshness_and_loop_budget() {
        let catalog = catalog(|tasks| {
            tasks["tasks"][0]["trigger"] = serde_json::json!({
                "kind": "fact",
                "scope": {"kind": "instance", "instance_id": "fixture-instance-a"},
                "fact_key": "fixture.ready",
                "comparison": "eq",
                "value": {"type": "boolean", "value": true},
                "max_age_ms": 1000
            });
            tasks["tasks"][0]["feedback_stop"] = false_fact();
        });
        let mut facts = base_facts();
        facts.facts.push(ObservedFact {
            scope: ScopeSelector::Instance {
                instance_id: "fixture-instance-a".to_owned(),
            },
            fact_key: "fixture.ready".to_owned(),
            value: FactValue::Boolean(true),
            observed_at_unix_ms: NOW,
        });

        let result = evaluate(
            &catalog,
            &facts,
            &base_resources(),
            EvaluationTime { unix_ms: NOW },
            11,
        )
        .expect("evaluation");
        let prerequisites = &result.dispatch_intents[0].prerequisites;

        assert_eq!(prerequisites.facts_fresh_until_unix_ms, Some(NOW + 1_000));
        assert_eq!(prerequisites.daily_limit, 24);
        assert_eq!(prerequisites.window_iteration_limit, 4);
        assert_eq!(prerequisites.max_runtime_ms, 300_000);
    }

    #[test]
    fn resource_projection_and_deadline_contribute_typed_urgency() {
        let catalog = catalog(|tasks| {
            tasks["tasks"][0]["trigger"] = serde_json::json!({
                "kind": "resource_projection",
                "pool_id": "fixture-pool-a",
                "comparison": "greater_than_or_equal",
                "value": 90
            });
            tasks["tasks"][0]["feedback_stop"] = false_fact();
            tasks["tasks"][0]["consumes"] = serde_json::json!([{
                "pool_id": "fixture-pool-a",
                "direction": "consume",
                "amount": 1,
                "observation_source": "scan_verified",
                "confidence_milli": 1000
            }]);
            tasks["tasks"][0]["produces"] = serde_json::json!([]);
        });
        let mut resources = base_resources();
        resources.pools[0].value = 100;

        let result = evaluate(
            &catalog,
            &base_facts(),
            &resources,
            EvaluationTime { unix_ms: NOW },
            5,
        )
        .expect("evaluation");

        assert_eq!(result.decisions[0].eligibility, EligibilityState::True);
        assert!(
            result.decisions[0]
                .rank
                .as_ref()
                .expect("rank")
                .urgency_milli
                > 0
        );
    }

    #[test]
    fn resource_projection_exposes_its_next_change_as_a_wake() {
        let catalog = catalog(|tasks| {
            tasks["tasks"][0]["trigger"] = serde_json::json!({
                "kind": "resource_projection",
                "pool_id": "fixture-pool-a",
                "comparison": "greater_than_or_equal",
                "value": 11
            });
            tasks["tasks"][0]["feedback_stop"] = false_fact();
        });

        let result = evaluate(
            &catalog,
            &base_facts(),
            &base_resources(),
            EvaluationTime { unix_ms: NOW },
            12,
        )
        .expect("evaluation");

        assert_eq!(result.decisions[0].eligibility, EligibilityState::False);
        assert_eq!(result.next_wake_unix_ms, Some(NOW + 360_000));
    }

    #[test]
    fn clock_trigger_uses_last_dispatch_and_returns_next_wake() {
        let catalog = catalog(|tasks| {
            tasks["tasks"][0]["trigger"] = due_clock();
            tasks["tasks"][0]["feedback_stop"] = false_fact();
        });
        let mut facts = base_facts();
        facts.tasks.push(TaskRuntimeSnapshot {
            task_id: "fixture.observe".to_owned(),
            instance_id: "fixture-instance-a".to_owned(),
            last_dispatched_unix_ms: Some(NOW),
            eligible_since_unix_ms: Some(NOW),
            terminal_state: None,
        });

        let result = evaluate(
            &catalog,
            &facts,
            &base_resources(),
            EvaluationTime { unix_ms: NOW },
            6,
        )
        .expect("evaluation");

        assert_eq!(result.decisions[0].eligibility, EligibilityState::False);
        assert_eq!(result.next_wake_unix_ms, Some(NOW + 3_600_000));
    }

    #[test]
    fn stale_fact_is_not_silently_false() {
        let catalog = catalog(|tasks| {
            tasks["tasks"][0]["trigger"] = serde_json::json!({
                "kind": "fact",
                "scope": {"kind": "instance", "instance_id": "fixture-instance-a"},
                "fact_key": "fixture.ready",
                "comparison": "eq",
                "value": {"type": "boolean", "value": true},
                "max_age_ms": 100
            });
            tasks["tasks"][0]["feedback_stop"] = false_fact();
        });
        let mut facts = base_facts();
        facts.facts.push(ObservedFact {
            scope: ScopeSelector::Instance {
                instance_id: "fixture-instance-a".to_owned(),
            },
            fact_key: "fixture.ready".to_owned(),
            value: FactValue::Boolean(true),
            observed_at_unix_ms: NOW - 101,
        });

        let result = evaluate(
            &catalog,
            &facts,
            &base_resources(),
            EvaluationTime { unix_ms: NOW },
            7,
        )
        .expect("evaluation");

        assert_eq!(result.decisions[0].eligibility, EligibilityState::Unknown);
        assert_eq!(
            result.decisions[0].detection_suggestions[0].reason,
            "fact_stale"
        );
    }

    #[test]
    fn invalid_snapshots_fail_loud_before_any_intent() {
        let catalog = catalog(|tasks| {
            tasks["tasks"][0]["trigger"] = due_clock();
            tasks["tasks"][0]["feedback_stop"] = false_fact();
        });
        let mut facts = base_facts();
        facts.instances[0].host_id = "missing-host".to_owned();

        let error = evaluate(
            &catalog,
            &facts,
            &base_resources(),
            EvaluationTime { unix_ms: NOW },
            8,
        )
        .expect_err("invalid host reference");

        assert_eq!(error.code(), "policy_evaluation_input_invalid");
        assert!(error.message().contains("missing host"));
    }

    #[test]
    fn pure_evaluator_source_has_no_runtime_side_effect_authority() {
        let source = include_str!("evaluator.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("production source");
        for forbidden in [
            "std::thread::sleep",
            "std::fs",
            "std::net",
            "SystemTime::now",
            "Instant::now",
            "actingcommand_device",
            "actingcommand_ledger",
            "LeaseToken",
        ] {
            assert!(
                !source.contains(forbidden),
                "forbidden source token {forbidden}"
            );
        }
    }

    fn decision_for<'a>(
        evaluation: &'a PolicyEvaluation,
        task_id: &str,
        instance_id: &str,
    ) -> &'a TaskDecision {
        evaluation
            .decisions
            .iter()
            .find(|decision| {
                decision.task_id == task_id && decision.instance_id.as_deref() == Some(instance_id)
            })
            .expect("task/instance decision")
    }

    fn catalog(mutator: impl FnOnce(&mut serde_json::Value)) -> CompiledCatalog {
        let mut documents = example_documents();
        mutator(&mut documents.0);
        compile_documents(documents)
    }

    fn two_task_catalog(mutator: impl FnOnce(&mut Vec<serde_json::Value>)) -> CompiledCatalog {
        let mut documents = example_documents();
        let mut second = documents.0["tasks"][0].clone();
        second["id"] = serde_json::json!("fixture.observe-secondary");
        second["procedure_ref"] = serde_json::json!("procedure.observe-secondary");
        second["instance_overrides"] = serde_json::json!([]);
        documents.0["tasks"]
            .as_array_mut()
            .expect("tasks array")
            .push(second);
        let tasks = documents.0["tasks"].as_array_mut().expect("tasks array");
        mutator(tasks);
        compile_documents(documents)
    }

    fn example_documents() -> (
        serde_json::Value,
        serde_json::Value,
        serde_json::Value,
        serde_json::Value,
    ) {
        (
            serde_json::from_slice(include_bytes!(
                "../../../contracts/scheduling/examples/catalog-a/tasks.json"
            ))
            .expect("tasks"),
            serde_json::from_slice(include_bytes!(
                "../../../contracts/scheduling/examples/catalog-a/pools.json"
            ))
            .expect("pools"),
            serde_json::from_slice(include_bytes!(
                "../../../contracts/scheduling/examples/catalog-a/activity.json"
            ))
            .expect("activity"),
            serde_json::from_slice(include_bytes!(
                "../../../contracts/scheduling/examples/catalog-a/timeline.json"
            ))
            .expect("timeline"),
        )
    }

    fn compile_documents(
        documents: (
            serde_json::Value,
            serde_json::Value,
            serde_json::Value,
            serde_json::Value,
        ),
    ) -> CompiledCatalog {
        compile_catalog(&CatalogSources {
            tasks: CatalogDocumentSource::new(
                "memory://fixture/tasks.json",
                serde_json::to_vec(&documents.0).expect("tasks bytes"),
            ),
            pools: CatalogDocumentSource::new(
                "memory://fixture/pools.json",
                serde_json::to_vec(&documents.1).expect("pools bytes"),
            ),
            activity: CatalogDocumentSource::new(
                "memory://fixture/activity.json",
                serde_json::to_vec(&documents.2).expect("activity bytes"),
            ),
            timeline: CatalogDocumentSource::new(
                "memory://fixture/timeline.json",
                serde_json::to_vec(&documents.3).expect("timeline bytes"),
            ),
        })
        .expect("compiled catalog")
    }

    fn due_clock() -> serde_json::Value {
        serde_json::json!({
            "kind": "clock",
            "schedule": {"kind": "interval", "every_ms": 3600000, "anchor_unix_ms": 0}
        })
    }

    fn false_fact() -> serde_json::Value {
        serde_json::json!({
            "kind": "fact",
            "scope": {"kind": "instance", "instance_id": "fixture-instance-a"},
            "fact_key": "fixture.completed",
            "comparison": "eq",
            "value": {"type": "boolean", "value": true},
            "max_age_ms": null
        })
    }

    fn base_facts() -> EvaluationFacts {
        EvaluationFacts {
            ledger_position: 42,
            fact_snapshot_id: "snapshot:fixture-a".to_owned(),
            facts: vec![ObservedFact {
                scope: ScopeSelector::Instance {
                    instance_id: "fixture-instance-a".to_owned(),
                },
                fact_key: "fixture.completed".to_owned(),
                value: FactValue::Boolean(false),
                observed_at_unix_ms: NOW,
            }],
            outcomes: Vec::new(),
            tasks: Vec::new(),
            instances: vec![InstanceSnapshot {
                instance_id: "fixture-instance-a".to_owned(),
                server_id: "fixture-server-a".to_owned(),
                game_id: "fixture-game-a".to_owned(),
                host_id: "fixture-host-a".to_owned(),
                available: true,
                capability_operation_ids: vec!["operation.observe".to_owned()],
                preferred_task_ids: Vec::new(),
            }],
        }
    }

    fn base_resources() -> EvaluationResources {
        EvaluationResources {
            pools: vec![PoolValueSnapshot {
                pool_id: "fixture-pool-a".to_owned(),
                value: 10,
                observed_at_unix_ms: NOW,
            }],
            hosts: vec![HostResourceSnapshot {
                host_id: "fixture-host-a".to_owned(),
                cpu_available_milli: 1_000,
                gpu_available_milli: 1_000,
                io_available_milli: 1_000,
            }],
        }
    }
}
