// SPDX-License-Identifier: AGPL-3.0-only

//! Deterministic scheduling decisions over compiled catalogs and pinned input snapshots.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{Code, CodeStr, Outcome, VocabularyToken};
use actingcommand_selection_policy::{
    Candidate, CollectedVerdict, ScalarValue, SelectionFactEntry, SelectionFactSnapshot,
    SelectionPolicy, UnknownInput, UnknownReason, ValueRef,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::codes::{PolicyCode, SchedulingRulePolicyCode};
use crate::remedy::ProviderView;
use crate::resource_targets::{InstanceTargetPolicy, TargetEffect, TargetPolicyState};
use crate::{
    ActivityProfile, ClockSchedule, ClockSource, Comparison, CompiledCatalog, FactScalar,
    FactValue, LoadProfile, MAX_PRIORITY_OFFSET_MILLI, MAX_TEXT_BYTES, ManualOffset,
    MissedSlotRule, ObservationRef, PoolProjection, PoolSpec, PredicateSpec, PrioritySelection,
    RemedyPlan, ResourceEffectSpec, ScopeSelector, TargetMode, TaskSpec, TaskTerminalState,
    TimelineEvent,
};

mod fact_refresh;

use fact_refresh::{AffectedCandidate, RefreshGate, RefreshKey};
pub use fact_refresh::{
    FACT_REFRESH_EXPECTED_DURATION_MS, FACT_REFRESH_OPERATION_ID, FACT_REFRESH_PROCEDURE_REF,
    FACT_REFRESH_TASK_ID, FactRefreshInputs, FactRefreshReader, FactRefreshSettings, InstanceHold,
    fact_refresh_task,
};

pub const MAX_EVALUATION_FACTS: usize = 16_384;
pub const MAX_EVALUATION_OUTCOMES: usize = 16_384;
pub const MAX_EVALUATION_TASK_STATES: usize = 8_192;
pub const MAX_EVALUATION_INSTANCES: usize = 1_024;
pub const MAX_EVALUATION_POOLS: usize = 4_096;
pub const MAX_EVALUATION_HOSTS: usize = 1_024;
pub const MAX_EVALUATION_PRIORITY_OFFSETS: usize = 16_384;
/// Longest last-run duration accepted as evaluation input: one day.
pub const MAX_TASK_LAST_DURATION_MS: u64 = 86_400_000;
/// Longest failure streak accepted as evaluation input.
pub const MAX_TASK_FAILURE_STREAK: u16 = 10_000;
/// Aging contributes at most this much urgency to the utility term.
const MAX_AGING_URGENCY_MILLI: u64 = 1_000;
/// Fewer Eligible candidates than this fall back to the absolute thresholds.
const MIN_PERCENTILE_CANDIDATES: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedFact {
    pub scope: ScopeSelector,
    pub fact_key: String,
    pub value: FactValue,
    pub observed_at_unix_ms: u64,
    #[serde(default)]
    pub expires_at_unix_ms: Option<u64>,
    #[serde(default = "default_fact_confidence_milli")]
    pub confidence_milli: u16,
}

const fn default_fact_confidence_milli() -> u16 {
    1_000
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedOutcome {
    pub task_id: String,
    pub instance_id: String,
    pub outcome_key: String,
    pub value: FactValue,
    pub observed_at_unix_ms: u64,
    #[serde(default)]
    pub expires_at_unix_ms: Option<u64>,
    /// The admitted activity window, projected from the settled run's ledger facts.
    #[serde(default)]
    pub activity_window_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletedActivityWindow {
    pub window_id: String,
    pub completed_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskRuntimeSnapshot {
    pub task_id: String,
    pub instance_id: String,
    pub last_dispatched_unix_ms: Option<u64>,
    pub eligible_since_unix_ms: Option<u64>,
    pub terminal_state: Option<TaskTerminalState>,
    /// Settled execution evidence, including tasks with no mapped outcome consumer.
    #[serde(default)]
    pub completed_window: Option<CompletedActivityWindow>,
    /// Wall-clock duration of the last settled run; replaces the declared duration as cost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_duration_ms: Option<u64>,
    /// Consecutive failed runs since the last success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_streak: Option<u16>,
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

/// Who published a manual priority offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PriorityOffsetOrigin {
    User,
    Agent,
}

/// One manual priority offset in milli. An entry naming an instance overrides the task-level
/// entry for that instance; a candidate without either gets no offset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PriorityOffset {
    pub task_id: String,
    #[serde(default)]
    pub instance_id: Option<String>,
    pub offset_milli: i32,
    pub origin: PriorityOffsetOrigin,
    pub observed_at_unix_ms: u64,
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
    /// Manual priority offsets; absent in older planning documents and fixtures.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub priority_offsets: Vec<PriorityOffset>,
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
    #[serde(default = "default_host_responsiveness_basis_points")]
    pub host_responsiveness_basis_points: u16,
    #[serde(default)]
    pub third_party_pressure_basis_points: u16,
    #[serde(default = "default_heavy_dispatch_limit")]
    pub heavy_dispatch_limit: u16,
    #[serde(default)]
    pub active_heavy_dispatches: u16,
    /// Workflow #385 PM-CFG: `performance.load.heavy_threshold_milli` in effect. A weighted
    /// load whose `cpu_milli + gpu_milli + io_milli` reaches it is heavy. The host's performance
    /// controller sets it with the heavy-dispatch budget (`apply_to_resources`), so the
    /// controller and the evaluator judge heavy work by one value. It is no member of the
    /// document: no configuration file or request names it.
    #[serde(skip, default = "default_heavy_threshold_milli")]
    pub heavy_threshold_milli: u64,
}

/// Workflow #385 PM-CFG: the default of `performance.load.heavy_threshold_milli` (PERF §10.1),
/// which a snapshot carries until the host's performance controller sets the value in effect.
pub const DEFAULT_HEAVY_THRESHOLD_MILLI: u64 = 1_500;

const fn default_host_responsiveness_basis_points() -> u16 {
    10_000
}

const fn default_heavy_dispatch_limit() -> u16 {
    1
}

const fn default_heavy_threshold_milli() -> u64 {
    DEFAULT_HEAVY_THRESHOLD_MILLI
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
    pub monotonic_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EligibilityState {
    True,
    False,
    Unknown,
}

/// Explicit instance identity for a pure catalog query, without runtime facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimelineQueryContext {
    pub instance_id: String,
    pub server_id: String,
    pub game_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimelineAvailability {
    pub state: EligibilityState,
    /// Half-open Unix-millisecond interval containing the query time, if active.
    pub active_interval: Option<(u64, u64)>,
    pub next_wake_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimelineEventInspection {
    pub event_id: String,
    pub scope: ScopeSelector,
    pub scope_applies: bool,
    pub availability: TimelineAvailability,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimelineInspection {
    pub catalog_id: String,
    pub catalog_version: u64,
    pub catalog_hash: String,
    pub time: EvaluationTime,
    pub context: TimelineQueryContext,
    pub events: Vec<TimelineEventInspection>,
    pub next_wake_unix_ms: Option<u64>,
}

/// Inspects selected events using the same availability owner as TimelineActive.
/// No clocks, facts, resources, capabilities or task state are read or synthesized.
pub fn inspect_timeline(
    catalog: &CompiledCatalog,
    time: EvaluationTime,
    context: &TimelineQueryContext,
    event_ids: &[String],
) -> PolicyEvaluationResult<TimelineInspection> {
    validate_instance_alias(&context.instance_id)?;
    validate_id("server id", &context.server_id)?;
    validate_id("game id", &context.game_id)?;
    validate_count("event ids", event_ids.len(), crate::MAX_TIMELINE_EVENTS)?;
    if event_ids.is_empty() {
        return Err(PolicyEvaluationError::invalid(
            "at least one event id is required",
        ));
    }
    validate_unique_ids("event id", event_ids)?;
    let selected = catalog
        .catalog()
        .timeline
        .events
        .iter()
        .map(|event| (event.id.as_str(), event))
        .collect::<BTreeMap<_, _>>();
    let mut events = Vec::with_capacity(event_ids.len());
    let mut next_wake = None;
    for id in event_ids {
        let event = selected.get(id.as_str()).ok_or_else(|| {
            PolicyEvaluationError::invalid(format!("unknown timeline event '{id}'"))
        })?;
        let scope_applies = scope_matches_identity(
            &event.scope,
            &context.instance_id,
            &context.server_id,
            &context.game_id,
        );
        let availability = timeline_availability(event, time, scope_applies)?;
        next_wake = min_wake(next_wake, availability.next_wake_unix_ms);
        events.push(TimelineEventInspection {
            event_id: id.clone(),
            scope: event.scope.clone(),
            scope_applies,
            availability,
        });
    }
    Ok(TimelineInspection {
        catalog_id: catalog.summary().catalog_id.clone(),
        catalog_version: catalog.summary().catalog_version,
        catalog_hash: catalog.catalog_hash().to_owned(),
        time,
        context: context.clone(),
        events,
        next_wake_unix_ms: next_wake,
    })
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

/// One reason of a scheduling decision: a registered code with its values (Workflow #378
/// A2-P), which a selected chain persists as its `PolicyReasonRecord`, and developer text that
/// is never persisted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionReason {
    pub outcome: Outcome,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskRank {
    pub priority: i16,
    pub aging_ms: u64,
    pub strategic_weight_milli: u32,
    pub urgency_milli: u16,
    pub load_cost_milli: u16,
    pub contention_penalty: i64,
    pub total_score: i64,
    /// The score stage's effective value (score + utility + offset, plus an applied resource
    /// target (RT-S1b)); 0 when no score applies.
    #[serde(default)]
    pub effective_milli: i64,
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
    /// Workflow #386 SR-6b, MCP-19a (register §1.143 M19A-1): this decision's own wake, one of
    /// the values the evaluation's `next_wake_unix_ms` is the least of. Never serialized, so an
    /// evaluation's bytes are unchanged; a decision read back has none.
    #[serde(skip)]
    pub next_wake_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchIntent {
    pub decision_id: String,
    pub task_id: String,
    pub instance_id: String,
    pub operation_id: String,
    pub procedure_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub procedure_binding_digest: Option<String>,
    pub catalog_hash: String,
    pub catalog_version: u64,
    pub input_ledger_position: u64,
    pub fact_snapshot_id: String,
    pub approval_refs: Vec<String>,
    pub reason_chain_id: String,
    pub expected_duration_ms: u64,
    #[serde(default)]
    pub yield_points: Vec<String>,
    pub load_profile: LoadProfile,
    pub prerequisites: DispatchPrerequisites,
}

/// Bounded look-ahead metadata for warming the next task package without granting execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreloadHint {
    pub task_id: String,
    pub package_ref: String,
    pub confidence_milli: u16,
    pub not_before_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchPrerequisites {
    pub fencing_required: bool,
    pub evaluated_at_unix_ms: u64,
    pub facts_fresh_until_unix_ms: Option<u64>,
    pub activity_profile_id: String,
    pub daily_limit: u32,
    pub window_iteration_limit: u32,
    pub max_runtime_ms: u64,
    #[serde(default)]
    pub urgency_milli: u16,
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
    pub preload_hint: Option<PreloadHint>,
    pub dispatch_intents: Vec<DispatchIntent>,
    pub reason_chains: Vec<DecisionReasonChain>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyEvaluationError {
    code: Code,
    message: String,
}

impl PolicyEvaluationError {
    pub fn code(&self) -> Code {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: PolicyCode::PolicyEvaluationInputInvalid.code(),
            message: message.into(),
        }
    }

    fn type_mismatch(message: impl Into<String>) -> Self {
        Self {
            code: PolicyCode::PolicyEvaluationFactTypeMismatch.code(),
            message: message.into(),
        }
    }

    fn overflow(message: impl Into<String>) -> Self {
        Self {
            code: PolicyCode::PolicyEvaluationNumericOverflow.code(),
            message: message.into(),
        }
    }

    fn score_overflow(message: impl Into<String>) -> Self {
        Self {
            code: PolicyCode::ScoreOverflow.code(),
            message: message.into(),
        }
    }

    fn selection_invalid(message: impl Into<String>) -> Self {
        Self {
            code: PolicyCode::SelectionPolicyInvalid.code(),
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

/// Derives pool values from the very same scoped fact projection; never persists a balance.
pub fn project_fact_pools(
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    resources: &EvaluationResources,
) -> EvaluationResources {
    let mut projected = resources.clone();
    for pool in &catalog.catalog().pools.pools {
        if pool.value_source.is_static() {
            continue;
        }
        projected.pools.retain(|value| value.pool_id != pool.id);
        if let Some(observation) = pool_fact(pool, facts)
            && let FactValue::Integer(value) = observation.value
            && let Ok(value) = u64::try_from(value)
            && value <= pool.capacity
        {
            projected.pools.push(PoolValueSnapshot {
                pool_id: pool.id.clone(),
                value,
                observed_at_unix_ms: observation.observed_at_unix_ms,
            });
        }
    }
    projected
        .pools
        .sort_by(|left, right| left.pool_id.cmp(&right.pool_id));
    projected
}

fn pool_fact<'a>(pool: &PoolSpec, facts: &'a EvaluationFacts) -> Option<&'a ObservedFact> {
    let crate::PoolValueSource::LedgerFact {
        minimum_confidence_milli,
    } = pool.value_source
    else {
        return None;
    };
    let ObservationRef::Fact { fact_key } = &pool.observation else {
        return None;
    };
    facts.facts.iter().find(|fact| {
        fact.scope == pool.scope
            && fact.fact_key == *fact_key
            && fact.confidence_milli >= minimum_confidence_milli
    })
}

pub(crate) fn project_time_validity(
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    time: EvaluationTime,
) -> PolicyEvaluationResult<EvaluationFacts> {
    let resets = catalog
        .catalog()
        .timeline
        .events
        .iter()
        .filter(|event| !event.invalidates_fact_prefixes.is_empty())
        .map(|event| Ok((event, invalidation_occurrences(event, time)?)))
        .collect::<PolicyEvaluationResult<Vec<_>>>()?;
    let validity = |scope: &ScopeSelector, key: &str, observed: u64, expiry: &mut Option<u64>| {
        for (event, (latest, next)) in &resets {
            if !scope_covers(&event.scope, scope, &facts.instances)
                || !event
                    .invalidates_fact_prefixes
                    .iter()
                    .any(|prefix| key.starts_with(prefix))
            {
                continue;
            }
            // Millisecond timestamps cannot order an observation at the reset instant.
            if latest.is_some_and(|at| observed <= at) {
                return false;
            }
            *expiry = min_wake(*expiry, next.and_then(|at| at.checked_sub(1)));
        }
        true
    };
    let mut projected = facts.clone();
    projected.facts.retain_mut(|fact| {
        validity(
            &fact.scope,
            &fact.fact_key,
            fact.observed_at_unix_ms,
            &mut fact.expires_at_unix_ms,
        )
    });
    projected.outcomes.retain_mut(|outcome| {
        validity(
            &ScopeSelector::Instance {
                instance_id: outcome.instance_id.clone(),
            },
            &format!("outcome.{}.{}", outcome.task_id, outcome.outcome_key),
            outcome.observed_at_unix_ms,
            &mut outcome.expires_at_unix_ms,
        )
    });
    for state in &mut projected.tasks {
        if state.completed_window.as_ref().is_some_and(|completed| {
            !validity(
                &ScopeSelector::Instance {
                    instance_id: state.instance_id.clone(),
                },
                &format!("task.{}.terminal_state", state.task_id),
                completed.completed_at_unix_ms,
                &mut None,
            )
        }) {
            state.completed_window = None;
            state.terminal_state = None;
        }
    }
    Ok(projected)
}

fn scope_covers(
    scope: &ScopeSelector,
    observed: &ScopeSelector,
    instances: &[InstanceSnapshot],
) -> bool {
    if scope == observed {
        return true;
    }
    match observed {
        ScopeSelector::Instance { instance_id } => instances.iter().any(|instance| {
            instance.instance_id == *instance_id && scope_matches_instance(scope, instance)
        }),
        ScopeSelector::Server { server_id } => {
            matches!(scope, ScopeSelector::Game { .. })
                && instances.iter().any(|instance| {
                    instance.server_id == *server_id && scope_matches_instance(scope, instance)
                })
        }
        ScopeSelector::Game { .. } => false,
    }
}

/// Evaluates one pinned snapshot without reading clocks, storage, devices, or process state.
pub fn evaluate(
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    resources: &EvaluationResources,
    time: EvaluationTime,
    seed: u64,
) -> PolicyEvaluationResult<PolicyEvaluation> {
    evaluate_with_eligibility(catalog, facts, resources, time, seed, |_| {
        Ok(CandidateEligibility::Eligible)
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CandidateEligibility {
    Eligible,
    Deferred {
        reason: DecisionReason,
        next_wake_unix_ms: Option<u64>,
    },
    /// Workflow #384 D2-S4: the Runtime's deferral of a fact refresh dispatch, with the code of
    /// the gate that deferred it and, when that gate is a limit, which one (D2 model §3.7 row 4);
    /// without a limit the deferral is for now (row 6). The candidate is deferred as under
    /// `Deferred`.
    Refresh {
        reason: DecisionReason,
        next_wake_unix_ms: Option<u64>,
        gate: Code,
        limit: Option<crate::codes::RefreshLimit>,
    },
}

/// Consults the Runtime's read-only admission state before allocating an instance winner.
/// Final admission remains the caller's fenced, locked operation.
pub fn evaluate_with_eligibility<E: From<PolicyEvaluationError>>(
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    resources: &EvaluationResources,
    time: EvaluationTime,
    seed: u64,
    eligibility: impl FnMut(&DispatchIntent) -> Result<CandidateEligibility, E>,
) -> Result<PolicyEvaluation, E> {
    evaluate_with_immediate_retries(
        catalog,
        facts,
        resources,
        time,
        seed,
        &BTreeSet::new(),
        eligibility,
    )
}

/// `evaluate_with_eligibility` where every (task, instance) pair in `immediate_retries` counts
/// as triggered and passes its task cooldown (Workflow #336 R22): the Runtime names the pairs
/// whose failure is scheduled for a retry within its original admission's window and budget
/// cycle. The eligibility callback still enforces backoff and other admission gates. The
/// feedback stop, placement, budgets and activity windows are unchanged.
pub fn evaluate_with_immediate_retries<E: From<PolicyEvaluationError>>(
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    resources: &EvaluationResources,
    time: EvaluationTime,
    seed: u64,
    immediate_retries: &BTreeSet<(String, String)>,
    eligibility: impl FnMut(&DispatchIntent) -> Result<CandidateEligibility, E>,
) -> Result<PolicyEvaluation, E> {
    evaluate_scheduling(
        catalog,
        facts,
        resources,
        time,
        seed,
        immediate_retries,
        None,
        None,
        eligibility,
    )
}

/// Workflow #384 D2-S4: `evaluate_with_immediate_retries` with the fact refresh (D2 model §3.3,
/// §3.4, §3.7). A candidate that reads an unknown realistic instance fact is deferred under
/// one reason per key, and the instance's recognition maintenance package is requested for the
/// keys its marks write: a dispatch intent of the reserved task [`FACT_REFRESH_TASK_ID`] that
/// competes with the instance's other candidates. Without these inputs (the other entry
/// points) such a candidate is only deferred, `fact_refresh_unbound` without a reader problem.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_with_fact_refresh<E: From<PolicyEvaluationError>>(
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    resources: &EvaluationResources,
    time: EvaluationTime,
    seed: u64,
    immediate_retries: &BTreeSet<(String, String)>,
    fact_refresh: &FactRefreshInputs,
    eligibility: impl FnMut(&DispatchIntent) -> Result<CandidateEligibility, E>,
) -> Result<PolicyEvaluation, E> {
    evaluate_scheduling(
        catalog,
        facts,
        resources,
        time,
        seed,
        immediate_retries,
        Some(fact_refresh),
        None,
        eligibility,
    )
}

/// Workflow #411 B15 R2 (`contracts/remedies.md` sections 4, 5 and 6): `evaluate_with_fact_refresh`
/// with the plan of [`crate::plan_remedies`]. A pair the plan requests or retries is treated as
/// triggered and past its cooldown, as an immediate rerun is, ranks among the promoted
/// candidates and carries the plan's reasons; a selected dispatch of a pair with run parameters
/// carries `policy_dispatch_parameters`; a pair the plan defers is deferred with the plan's reason
/// when the eligibility callback answered `Eligible` (the host places it at position 8a itself).
/// Once the allocation is known, a requesting layer's hold names its provider's gate, and a layer
/// whose every provider met a persistent gate stops its round's root. Placement, the feedback
/// stop, budgets, the host budget and one winner per instance are unchanged; between several
/// requested providers the existing ranking decides. With an empty plan the evaluation is the
/// one `evaluate_with_fact_refresh` gives.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_with_remedies<E: From<PolicyEvaluationError>>(
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    resources: &EvaluationResources,
    time: EvaluationTime,
    seed: u64,
    immediate_retries: &BTreeSet<(String, String)>,
    fact_refresh: &FactRefreshInputs,
    remedies: &RemedyPlan,
    eligibility: impl FnMut(&DispatchIntent) -> Result<CandidateEligibility, E>,
) -> Result<PolicyEvaluation, E> {
    evaluate_scheduling(
        catalog,
        facts,
        resources,
        time,
        seed,
        immediate_retries,
        Some(fact_refresh),
        Some(remedies),
        eligibility,
    )
}

#[allow(clippy::too_many_arguments)]
fn evaluate_scheduling<E: From<PolicyEvaluationError>>(
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    resources: &EvaluationResources,
    time: EvaluationTime,
    seed: u64,
    immediate_retries: &BTreeSet<(String, String)>,
    refresh_inputs: Option<&FactRefreshInputs>,
    remedies: Option<&RemedyPlan>,
    mut eligibility: impl FnMut(&DispatchIntent) -> Result<CandidateEligibility, E>,
) -> Result<PolicyEvaluation, E> {
    validate_inputs(catalog, facts, resources, time)?;
    // Stored resource target policies are read before the time-validity projection, so a
    // timeline reset never silently withdraws one (Workflow #308 RT-S1b).
    let target_policies = crate::resource_targets::instance_target_policies(facts, time)?;

    let effective_facts = project_time_validity(catalog, facts, time)?;
    let facts = &effective_facts;

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
        .filter(|snapshot| {
            pool_specs
                .get(snapshot.pool_id.as_str())
                .is_some_and(|pool| {
                    pool.value_source.is_static()
                        || pool_fact(pool, facts).is_some_and(|fact| {
                            fact.observed_at_unix_ms <= time.unix_ms
                                && fact
                                    .expires_at_unix_ms
                                    .is_none_or(|expiry| time.unix_ms <= expiry)
                        })
                })
        })
        .map(|pool| (pool.pool_id.as_str(), pool))
        .collect();
    let host_resources: BTreeMap<&str, HostRemaining> = resources
        .hosts
        .iter()
        .map(|host| (host.host_id.as_str(), HostRemaining::from(host)))
        .collect();

    let pool_fresh_until = catalog_bundle
        .pools
        .pools
        .iter()
        .filter_map(|pool| pool_fact(pool, facts).and_then(|fact| fact.expires_at_unix_ms))
        .filter(|expiry| *expiry >= time.unix_ms)
        .min();
    let mut next_wake = pool_fresh_until.and_then(|expiry| expiry.checked_add(1));
    let timeline_events: BTreeMap<&str, &TimelineEvent> = catalog_bundle
        .timeline
        .events
        .iter()
        .map(|event| (event.id.as_str(), event))
        .collect();
    let mut invalidation_wake = None;
    for event in &catalog_bundle.timeline.events {
        let wake = if event.validity.is_some() {
            timeline_window(event, time)?.next_wake
        } else {
            next_schedule_occurrence(&event.schedule, time)?
        };
        next_wake = min_wake(next_wake, wake);
        if !event.invalidates_fact_prefixes.is_empty() {
            invalidation_wake =
                min_wake(invalidation_wake, invalidation_occurrences(event, time)?.1);
        }
    }
    next_wake = min_wake(next_wake, invalidation_wake);
    // Admission's existing freshness ceiling is inclusive; V2 boundaries are exclusive.
    let timeline_fresh_until =
        if catalog_bundle.timeline.schema_version == crate::SCHEDULING_SCHEMA_VERSION_V2 {
            next_wake
                .map(|boundary| {
                    boundary.checked_sub(1).ok_or_else(|| {
                        PolicyEvaluationError::overflow(
                            "timeline boundary has no preceding Unix millisecond",
                        )
                    })
                })
                .transpose()?
        } else {
            invalidation_wake.and_then(|boundary| boundary.checked_sub(1))
        };
    let mut preload_hint = None;
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
    // Workflow #384 D2-S4: the candidates deferred for unknown realistic keys they read.
    let mut affected = Vec::new();

    for task in &catalog_bundle.tasks.tasks {
        let referenced_outcome_keys = catalog.referenced_outcome_keys(&task.id);
        let matching_instances: Vec<&InstanceSnapshot> = facts
            .instances
            .iter()
            .filter(|instance| scope_matches_instance(&task.scope, instance))
            .collect();
        if matching_instances.is_empty() {
            work.push(TaskWork::blocked_without_instance(
                task.id.clone(),
                reason(
                    PolicyCode::ScopeWithoutInstance,
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
            let mut task_work = TaskWork::new(task.id.clone(), instance.instance_id.clone());
            let mut trigger = evaluate_predicate(
                &task.trigger,
                state,
                &decision_scope,
                facts,
                &pool_specs,
                &pool_values,
                time,
                &timeline_events,
                &mut task_work.reasons,
            )?;
            // Workflow #336 R22: a failed task whose failure carries a failure identity (a
            // linear task's, and since Workflow #390 EXEC-3 a page-graph task's) reruns at once,
            // without waiting for its trigger or its cooldown.
            let immediate_retry = immediate_retries.iter().any(|(task_id, instance_id)| {
                *task_id == task.id && *instance_id == instance.instance_id
            });
            if immediate_retry {
                trigger.truth = PredicateTruth::True;
                trigger.suggestions.clear();
                task_work.reasons.push(reason(
                    PolicyCode::FailureRetryImmediate,
                    "the latest failure is rerun at once: its trigger and cooldown are not awaited",
                ));
            }
            // Workflow #411 B15 R2: a remedy request or retry is treated as triggered and past
            // its cooldown too, with the plan's reasons.
            let remedy_override =
                remedies.and_then(|plan| plan.trigger_override(&task.id, &instance.instance_id));
            if let Some(remedy_override) = remedy_override {
                trigger.truth = PredicateTruth::True;
                trigger.suggestions.clear();
                task_work
                    .reasons
                    .extend(remedy_override.reasons().iter().cloned());
            }
            task_work.next_wake_unix_ms = trigger.next_wake_unix_ms;
            if let Some(not_before_unix_ms) = trigger.next_wake_unix_ms {
                consider_preload_hint(
                    &mut preload_hint,
                    PreloadHint {
                        task_id: task.id.clone(),
                        package_ref: task.procedure_ref.clone(),
                        confidence_milli: if trigger.suggestions.is_empty() {
                            750
                        } else {
                            250
                        },
                        not_before_unix_ms,
                    },
                );
            }

            match trigger.truth {
                PredicateTruth::False => {
                    task_work.eligibility = EligibilityState::False;
                    task_work.state = SchedulingDecisionState::Blocked;
                    task_work.reasons.push(reason(
                        PolicyCode::TriggerFalse,
                        "the task trigger evaluated to false for this instance",
                    ));
                }
                PredicateTruth::Unknown => {
                    if refresh_inputs.is_some() {
                        affected.extend(predicate_refresh(
                            work.len(),
                            task,
                            state,
                            instance,
                            &placement_context,
                            facts,
                            &trigger.suggestions,
                        )?);
                    }
                    task_work.suggestions = trigger.suggestions;
                    task_work.reasons.push(reason(
                        PolicyCode::TriggerUnknown,
                        "the task trigger requires additional observations",
                    ));
                }
                PredicateTruth::True => {
                    let window =
                        select_activity_profile(&catalog_bundle.activity.profiles, instance)
                            .map(|profile| activity_window_at(profile, time.unix_ms))
                            .transpose()?
                            .flatten();
                    let has_window_result = window.as_ref().is_some_and(|window| {
                        if referenced_outcome_keys.is_empty() {
                            state
                                .filter(|state| state.terminal_state.is_some())
                                .and_then(|state| state.completed_window.as_ref())
                                .is_some_and(|result| {
                                    result.window_id == window.window_id
                                        && window.contains(result.completed_at_unix_ms)
                                })
                        } else {
                            facts.outcomes.iter().any(|outcome| {
                                outcome.task_id == task.id
                                    && referenced_outcome_keys.contains(&outcome.outcome_key)
                                    && outcome.instance_id == instance.instance_id
                                    && outcome.activity_window_id.as_deref()
                                        == Some(window.window_id.as_str())
                                    && window.contains(outcome.observed_at_unix_ms)
                                    && outcome
                                        .expires_at_unix_ms
                                        .is_none_or(|expiry| time.unix_ms <= expiry)
                            })
                        }
                    });
                    let mut stop = if has_window_result {
                        evaluate_predicate(
                            &task.feedback_stop,
                            state,
                            &decision_scope,
                            facts,
                            &pool_specs,
                            &pool_values,
                            time,
                            &timeline_events,
                            &mut task_work.reasons,
                        )?
                    } else {
                        PredicateEvaluation::known(false, None)
                    };
                    if has_window_result {
                        let result_expiry = facts
                            .outcomes
                            .iter()
                            .filter(|outcome| {
                                outcome.task_id == task.id
                                    && outcome.instance_id == instance.instance_id
                                    && referenced_outcome_keys.contains(&outcome.outcome_key)
                                    && window.as_ref().is_some_and(|window| {
                                        outcome.activity_window_id.as_deref()
                                            == Some(window.window_id.as_str())
                                            && window.contains(outcome.observed_at_unix_ms)
                                    })
                                    && outcome
                                        .expires_at_unix_ms
                                        .is_none_or(|expiry| expiry >= time.unix_ms)
                            })
                            .filter_map(|outcome| outcome.expires_at_unix_ms)
                            .min();
                        stop.fresh_until_unix_ms =
                            min_wake(stop.fresh_until_unix_ms, result_expiry);
                        stop.next_wake_unix_ms = min_wake(
                            stop.next_wake_unix_ms,
                            result_expiry.and_then(|expiry| expiry.checked_add(1)),
                        );
                    }
                    if let Some(window) = window.filter(|_| has_window_result) {
                        stop.next_wake_unix_ms =
                            min_wake(stop.next_wake_unix_ms, Some(window.until_unix_ms));
                        stop.fresh_until_unix_ms = min_wake(
                            stop.fresh_until_unix_ms,
                            window.until_unix_ms.checked_sub(1),
                        );
                    }
                    task_work.next_wake_unix_ms =
                        min_wake(task_work.next_wake_unix_ms, stop.next_wake_unix_ms);
                    match stop.truth {
                        PredicateTruth::True => {
                            task_work.eligibility = EligibilityState::False;
                            task_work.state = SchedulingDecisionState::Blocked;
                            task_work.reasons.push(reason(
                                PolicyCode::FeedbackStopTrue,
                                "the feedback stop predicate evaluated to true for this instance",
                            ));
                        }
                        PredicateTruth::Unknown => {
                            if refresh_inputs.is_some() {
                                affected.extend(predicate_refresh(
                                    work.len(),
                                    task,
                                    state,
                                    instance,
                                    &placement_context,
                                    facts,
                                    &stop.suggestions,
                                )?);
                            }
                            task_work.suggestions = stop.suggestions;
                            task_work.reasons.push(reason(
                                PolicyCode::FeedbackStopUnknown,
                                "the feedback stop predicate requires additional observations",
                            ));
                        }
                        PredicateTruth::False => {
                            task_work.eligibility = EligibilityState::True;
                            task_work.state = SchedulingDecisionState::Eligible;
                            task_work.reasons.push(reason(
                                PolicyCode::TaskEligible,
                                "trigger passed and feedback stop did not fire",
                            ));
                            let facts_fresh_until_unix_ms = min_wake(
                                min_wake(trigger.fresh_until_unix_ms, stop.fresh_until_unix_ms),
                                min_wake(timeline_fresh_until, pool_fresh_until),
                            );
                            let cooldown_until = state
                                .filter(|_| !immediate_retry && remedy_override.is_none())
                                .and_then(|state| state.last_dispatched_unix_ms)
                                .map(|last| {
                                    last.checked_add(task.cooldown_ms).ok_or_else(|| {
                                        PolicyEvaluationError::overflow(format!(
                                            "task '{}' cooldown overflowed",
                                            task.id
                                        ))
                                    })
                                })
                                .transpose()?;
                            if cooldown_until.is_some_and(|until| time.unix_ms < until) {
                                task_work.state = SchedulingDecisionState::Deferred;
                                task_work.reasons.push(reason(
                                    PolicyCode::TaskCooldownActive,
                                    "the task-specific dispatch cooldown has not elapsed",
                                ));
                                task_work.next_wake_unix_ms =
                                    min_wake(task_work.next_wake_unix_ms, cooldown_until);
                            } else {
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
                                        task_work.record = Some(RankRecord {
                                            base_total: candidate.rank.total_score,
                                            stage: None,
                                            resources: None,
                                            promoted: false,
                                            affinity: candidate.affinity,
                                            tie_breaker: candidate.tie_breaker,
                                        });
                                        candidates.push(*candidate);
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
            }
            normalize_suggestions(&mut task_work.suggestions);
            work.push(task_work);
        }
    }

    let (mut candidates, scored_affected) = apply_priority_selection(
        catalog,
        facts,
        time,
        candidates,
        &mut work,
        &target_policies,
        refresh_inputs.is_some(),
    )?;
    affected.extend(scored_affected);
    // Workflow #411 B15 R2: a remedy request or retry ranks among the promoted candidates.
    if let Some(plan) = remedies {
        for candidate in &mut candidates {
            if plan
                .trigger_override(&candidate.task_id, &candidate.instance_id)
                .is_some_and(crate::RemedyOverride::promoted)
            {
                candidate.promoted = true;
                if let Some(record) = work[candidate.work_index].record.as_mut() {
                    record.promoted = true;
                }
            }
        }
    }
    // Workflow #384 D2-S4: every instance's recognition maintenance package joins the allocation
    // for the keys its affected candidates read, unless its own gates hold it back.
    let refresh_plans = match refresh_inputs {
        Some(inputs) => fact_refresh::plan(
            &fact_refresh::PlanContext {
                inputs,
                facts,
                profiles: catalog_bundle.activity.profiles.as_slice(),
                hosts: &host_resources,
                task_states: &task_states,
                time,
                seed,
            },
            affected,
            &mut work,
            &mut candidates,
        )?,
        None => Vec::new(),
    };
    // The gate that kept each planned package from the allocation's dispatch, by work index.
    let mut refresh_gates = BTreeMap::<usize, RefreshGate>::new();

    candidates.sort_by(|left, right| {
        right
            .promoted
            .cmp(&left.promoted)
            .then_with(|| right.rank.total_score.cmp(&left.rank.total_score))
            .then_with(|| right.affinity.cmp(&left.affinity))
            .then_with(|| left.tie_breaker.cmp(&right.tie_breaker))
            .then_with(|| left.task_id.cmp(&right.task_id))
            .then_with(|| left.instance_id.cmp(&right.instance_id))
    });

    let mut remaining = host_resources;
    let mut selected_instances = BTreeSet::new();
    let mut dispatch_intents = Vec::new();
    let mut reason_chains = Vec::new();
    // (reason chain index, work index) of every selected candidate, for its decision record.
    let mut selected = Vec::new();

    for candidate in candidates {
        // Workflow #384 D2-S4: the recognition maintenance package's own deferrals are read by
        // the fact refresh's table (D2 model §3.7 rows 4 and 6).
        let refresh = candidate.task_id == FACT_REFRESH_TASK_ID;
        if selected_instances.contains(candidate.instance_id.as_str()) {
            work[candidate.work_index].reasons.push(reason(
                PolicyCode::InstanceAlreadySelected,
                "a higher-ranked task already owns this instance for the evaluation round",
            ));
            if refresh {
                refresh_gates.insert(
                    candidate.work_index,
                    RefreshGate::Temporary {
                        gate: Some(PolicyCode::InstanceAlreadySelected.code()),
                        wake: None,
                    },
                );
            }
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
            let (code, detail) = if candidate.load.heavy
                && host.active_heavy_dispatches >= host.heavy_dispatch_limit
            {
                (
                    PolicyCode::HeavySceneBudgetDeferred,
                    "the shared host heavy-scene concurrency budget was consumed by higher-ranked dispatches",
                )
            } else {
                (
                    PolicyCode::HostBudgetDeferred,
                    "the shared host budget was consumed by higher-ranked dispatches",
                )
            };
            work[candidate.work_index]
                .reasons
                .push(reason(code, detail));
            if refresh {
                refresh_gates.insert(
                    candidate.work_index,
                    RefreshGate::Temporary {
                        gate: Some(code.code()),
                        wake: None,
                    },
                );
            }
            continue;
        }
        let decision_id = deterministic_decision_id(
            catalog.catalog_hash(),
            &candidate.task_id,
            &candidate.instance_id,
            facts.ledger_position,
            &facts.fact_snapshot_id,
            time.unix_ms,
            seed,
        );
        let reason_chain_id = format!("reason:{decision_id}");
        let mut reasons = work[candidate.work_index].reasons.clone();
        // Workflow #411 B15 R2: the read-only parameters the run receives.
        if let Some(parameters) = remedies
            .and_then(|plan| plan.dispatch_parameters(&candidate.task_id, &candidate.instance_id))
        {
            reasons.push(parameters.clone());
        }
        reasons.extend([
            reason(
                PolicyCode::PlacementSelected,
                format!(
                    "instance '{}' satisfies scope, capability, affinity, and host budget",
                    candidate.instance_id
                ),
            ),
            reason(
                PolicyCode::RankAssigned,
                format!("deterministic total score {}", candidate.rank.total_score),
            ),
        ]);
        let intent = DispatchIntent {
            decision_id,
            task_id: candidate.task_id.clone(),
            instance_id: candidate.instance_id.clone(),
            operation_id: candidate.operation_id,
            procedure_ref: candidate.procedure_ref,
            procedure_binding_digest: None,
            catalog_hash: catalog.catalog_hash().to_owned(),
            catalog_version: catalog_bundle.tasks.catalog.catalog_version,
            input_ledger_position: facts.ledger_position,
            fact_snapshot_id: facts.fact_snapshot_id.clone(),
            approval_refs: catalog_bundle.tasks.catalog.approval_refs.clone(),
            reason_chain_id,
            expected_duration_ms: candidate.expected_duration_ms,
            yield_points: candidate.yield_points,
            load_profile: candidate.load_profile,
            prerequisites: DispatchPrerequisites {
                fencing_required: true,
                evaluated_at_unix_ms: time.unix_ms,
                facts_fresh_until_unix_ms: candidate.facts_fresh_until_unix_ms,
                activity_profile_id: candidate.activity_profile_id,
                daily_limit: candidate.daily_limit,
                window_iteration_limit: candidate.window_iteration_limit,
                max_runtime_ms: candidate.max_runtime_ms,
                urgency_milli: candidate.rank.urgency_milli,
            },
        };
        let (deferral, gate) = match eligibility(&intent)? {
            // Workflow #411 B15 R2: the plan's hold or stop, at position 8a.
            CandidateEligibility::Eligible => match remedies
                .and_then(|plan| plan.deferral(&candidate.task_id, &candidate.instance_id))
            {
                Some(planned) => (
                    Some((planned.reason.clone(), planned.next_wake_unix_ms)),
                    None,
                ),
                None => (None, None),
            },
            CandidateEligibility::Deferred {
                reason,
                next_wake_unix_ms,
            } => (
                Some((reason, next_wake_unix_ms)),
                Some(RefreshGate::Temporary {
                    gate: None,
                    wake: next_wake_unix_ms,
                }),
            ),
            CandidateEligibility::Refresh {
                reason,
                next_wake_unix_ms,
                gate,
                limit,
            } => (
                Some((reason, next_wake_unix_ms)),
                Some(match limit {
                    Some(limit) => RefreshGate::Limit {
                        limit,
                        gate: Some(gate),
                        wake: next_wake_unix_ms,
                    },
                    None => RefreshGate::Temporary {
                        gate: Some(gate),
                        wake: next_wake_unix_ms,
                    },
                }),
            ),
        };
        if let Some((reason, next_wake_unix_ms)) = deferral {
            let task_work = &mut work[candidate.work_index];
            task_work.state = SchedulingDecisionState::Deferred;
            task_work.reasons.push(reason);
            task_work.next_wake_unix_ms = next_wake_unix_ms;
            if let Some(gate) = gate.filter(|_| refresh) {
                refresh_gates.insert(candidate.work_index, gate);
            }
            continue;
        }
        host.consume(candidate.load);
        selected_instances.insert(candidate.instance_id.clone());
        selected.push((reason_chains.len(), candidate.work_index));
        reason_chains.push(DecisionReasonChain {
            id: intent.reason_chain_id.clone(),
            decision_id: intent.decision_id.clone(),
            reasons: reasons.clone(),
        });
        dispatch_intents.push(intent);
        let task_work = &mut work[candidate.work_index];
        task_work.state = SchedulingDecisionState::Selected;
        task_work.rank = Some(candidate.rank);
        task_work.reasons = reasons;
    }
    // Workflow #384 D2-S4: the affected candidates' reasons, before any dispatch decision record
    // lists them.
    if let Some(inputs) = refresh_inputs {
        fact_refresh::finish(
            &refresh_plans,
            &refresh_gates,
            inputs,
            facts,
            time,
            &mut work,
        )?;
    }
    if let Some(plan) = remedies.filter(|plan| plan.has_requests()) {
        finish_remedy_holds(plan, &mut work);
    }
    append_decision_records(&mut work, &mut reason_chains, &selected, &target_policies)?;
    work.sort_by(|left, right| {
        left.task_id.cmp(&right.task_id).then_with(|| {
            left.instance_id
                .as_deref()
                .cmp(&right.instance_id.as_deref())
        })
    });

    for task_work in &work {
        next_wake = min_wake(next_wake, task_work.next_wake_unix_ms);
    }
    Ok(PolicyEvaluation {
        decisions: work.into_iter().map(TaskDecision::from).collect(),
        next_wake_unix_ms: next_wake,
        preload_hint,
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
    next_wake_unix_ms: Option<u64>,
    /// The score breakdown of a candidate that reached the ranking; never serialized.
    record: Option<RankRecord>,
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
            next_wake_unix_ms: None,
            record: None,
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
            next_wake_unix_ms: None,
            record: None,
        }
    }
}

/// The record is dropped and the wake kept unserialized: the decision shape is unchanged.
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
            next_wake_unix_ms: value.next_wake_unix_ms,
        }
    }
}

/// Workflow #411 B15 R2 (`contracts/remedies.md` section 5.3): once the allocation is known, a
/// requesting layer's hold (the plan's `policy_remedy_waiting`, placed by this evaluation or by
/// the host's callback) names the gate of the provider it waits for, and the root of a layer whose
/// every provider met a persistent gate is stopped instead (`policy_remedy_unavailable`,
/// `provider_unavailable`). Only a deferred decision whose last reason is the hold changes.
fn finish_remedy_holds(plan: &RemedyPlan, work: &mut [TaskWork]) {
    let finals = plan.finalized_reasons(|task_id, instance_id| {
        work.iter()
            .find(|entry| {
                entry.task_id == task_id && entry.instance_id.as_deref() == Some(instance_id)
            })
            .map(|entry| ProviderView {
                selected: entry.state == SchedulingDecisionState::Selected,
                last_code: entry
                    .reasons
                    .last()
                    .map(|reason| reason.outcome.code().clone()),
                next_wake_unix_ms: entry.next_wake_unix_ms,
            })
    });
    if finals.is_empty() {
        return;
    }
    for entry in work.iter_mut() {
        let Some(instance_id) = entry.instance_id.as_deref() else {
            continue;
        };
        let Some(planned) = finals.get(&(entry.task_id.clone(), instance_id.to_owned())) else {
            continue;
        };
        if entry.state != SchedulingDecisionState::Deferred {
            continue;
        }
        if let Some(last) = entry
            .reasons
            .last_mut()
            .filter(|last| last.outcome.is(PolicyCode::PolicyRemedyWaiting))
        {
            *last = planned.reason.clone();
            entry.next_wake_unix_ms = planned.next_wake_unix_ms;
        }
    }
}

/// How one candidate's `total_score` was put together (Workflow #308 RT-S1c). Written when the
/// candidate is built and completed by the score stage; it only feeds the dispatch decision
/// record and is never serialized.
#[derive(Debug, Clone)]
struct RankRecord {
    /// `total_score` before the score stage.
    base_total: i64,
    /// `None` when the score stage left the candidate untouched.
    stage: Option<StageRecord>,
    /// Every candidate of an instance with an active v2 resource target policy.
    resources: Option<ResourceRecord>,
    promoted: bool,
    affinity: bool,
    tie_breaker: u64,
}

/// The score stage's terms of one staged candidate.
#[derive(Debug, Clone)]
struct StageRecord {
    score: Option<i64>,
    utility: i64,
    offset: i64,
    /// An applied resource target: `(id, mode, task target score)`.
    target: Option<(String, TargetMode, u64)>,
    /// An applied override superseded the selection score (and, unless a v2 override keeps
    /// it, the manual offset).
    superseded: bool,
}

/// A candidate's resource term under an active v2 policy (Workflow #335 S2b).
#[derive(Debug, Clone)]
struct ResourceRecord {
    /// `R`, zero for a candidate nothing of the policy weighs.
    term: u64,
    /// The effective override's target id.
    override_id: Option<String>,
    /// The manual offset stays in the effective score: always but under a `supersede` override.
    offset_kept: bool,
}

impl ResourceRecord {
    fn new(term: &crate::resource_targets::ResourceTerm) -> Self {
        Self {
            term: term.term_milli,
            override_id: term.override_target.as_ref().map(|(id, _)| id.clone()),
            offset_kept: !matches!(term.override_target, Some((_, ManualOffset::Supersede))),
        }
    }
}

/// The longest reason chain admission accepts; a longer one fails it fatally.
const MAX_REASON_CHAIN_LENGTH: usize = 128;
/// At most this many related candidates are listed in one dispatch decision record.
const MAX_DECISION_RECORD_CANDIDATES: usize = 8;
/// A decision record detail over `MAX_TEXT_BYTES` keeps at most this many bytes before the mark.
const TRUNCATED_DETAIL_BYTES: usize = 1_010;
const TRUNCATION_MARK: &str = "…[truncated]";

/// Closes the round's explanations. First every decision left without a reason is settled with
/// `decision_reason_missing` (Workflow #386 SR-6a), so no record below names `none`. Then the
/// dispatch decision record is appended after `rank_assigned` on every selected chain (Workflow
/// #308 RT-S1c), and each selected decision's reasons are kept equal to its chain:
/// `rank_breakdown`, `decision_record`, then `decision_candidate_not_selected` (with the
/// candidate's `task_id`) for at most eight related candidates. A chain without room for the
/// first two appends nothing and says so in `rank_assigned`'s detail. No other decision gains a
/// reason.
fn append_decision_records(
    work: &mut [TaskWork],
    reason_chains: &mut [DecisionReasonChain],
    selected: &[(usize, usize)],
    target_policies: &BTreeMap<String, InstanceTargetPolicy>,
) -> PolicyEvaluationResult<()> {
    settle_decisions_left_without_reason(work);
    for &(chain_index, work_index) in selected {
        let length = reason_chains
            .get(chain_index)
            .ok_or_else(|| {
                PolicyEvaluationError::invalid("a selected decision has no reason chain")
            })?
            .reasons
            .len();
        let room = MAX_REASON_CHAIN_LENGTH.saturating_sub(length);
        let record = if room >= 2 {
            Some(decision_record(
                work,
                work_index,
                target_policies,
                room - 2,
            )?)
        } else {
            None
        };
        let chain = &mut reason_chains[chain_index].reasons;
        match record {
            Some(record) => chain.extend(record),
            None => {
                let ranked = chain
                    .last_mut()
                    .filter(|reason| reason.outcome.is(PolicyCode::RankAssigned))
                    .ok_or_else(|| {
                        PolicyEvaluationError::invalid(
                            "a selected reason chain does not end with rank_assigned",
                        )
                    })?;
                ranked.detail = bounded_detail(format!(
                    "{}; decision_record omitted: reason chain at {length}/{MAX_REASON_CHAIN_LENGTH}",
                    ranked.detail
                ));
            }
        }
        work[work_index].reasons.clone_from(chain);
    }
    Ok(())
}

/// Workflow #386 SR-6a (scheduling model v1.4.2 §8.2 item 2): every decision that is not
/// selected carries at least one reason, its last one being why it does not run. A path that left
/// one without any reason is a programming error. The decision gets `decision_reason_missing`
/// (error; `task_id`, `instance_id`) and stays unselected; an eligible one is deferred. The round
/// goes on, and the gap is visible in the decision and in every dispatch decision record that
/// lists it.
fn settle_decisions_left_without_reason(work: &mut [TaskWork]) {
    for entry in work.iter_mut() {
        if entry.state == SchedulingDecisionState::Selected || !entry.reasons.is_empty() {
            continue;
        }
        // Only `TaskWork::blocked_without_instance` builds a decision without an instance, and
        // always with its reason.
        let Some(instance_id) = entry.instance_id.as_deref() else {
            continue;
        };
        let detail = format!("task_id={} instance_id={instance_id}", entry.task_id);
        let outcome = Outcome::new(SchedulingRulePolicyCode::DecisionReasonMissing)
            .task_id(entry.task_id.as_str())
            .instance_id(instance_id);
        entry.reasons.push(reason_with(outcome, detail));
        if entry.state == SchedulingDecisionState::Eligible {
            entry.state = SchedulingDecisionState::Deferred;
        }
    }
}

/// A work entry's rank and rank record, when it reached the ranking.
fn ranked_view(entry: &TaskWork) -> PolicyEvaluationResult<Option<(&TaskRank, &RankRecord)>> {
    match (&entry.rank, &entry.record) {
        (Some(rank), Some(record)) => Ok(Some((rank, record))),
        (_, None) => Ok(None),
        (None, Some(_)) => Err(PolicyEvaluationError::invalid(format!(
            "task '{}' has a rank record without a rank",
            entry.task_id
        ))),
    }
}

/// The decision record of the selected entry `work[work_index]`: its breakdown, the instance's
/// target policy and its related candidates, every work entry of the same instance but itself,
/// ranked ones first by total descending, then the rest, each by task id; `candidate_room`
/// bounds how many are listed. The unlisted ones are counted by their last reason code.
fn decision_record(
    work: &[TaskWork],
    work_index: usize,
    target_policies: &BTreeMap<String, InstanceTargetPolicy>,
    candidate_room: usize,
) -> PolicyEvaluationResult<Vec<DecisionReason>> {
    let winner = &work[work_index];
    let instance_id = winner
        .instance_id
        .as_deref()
        .ok_or_else(|| PolicyEvaluationError::invalid("a selected decision has no instance"))?;
    let (rank, record) = ranked_view(winner)?.ok_or_else(|| {
        PolicyEvaluationError::invalid(format!(
            "selected task '{}' has no rank record",
            winner.task_id
        ))
    })?;
    let mut related = Vec::new();
    for (index, entry) in work.iter().enumerate() {
        if index != work_index && entry.instance_id.as_deref() == Some(instance_id) {
            related.push((entry, ranked_view(entry)?));
        }
    }
    related.sort_by(|(left, left_view), (right, right_view)| {
        match (left_view, right_view) {
            (Some((left_rank, _)), Some((right_rank, _))) => {
                right_rank.total_score.cmp(&left_rank.total_score)
            }
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
        .then_with(|| left.task_id.cmp(&right.task_id))
    });
    let lead = related
        .first()
        .and_then(|(_, view)| *view)
        .map(|(related_rank, _)| {
            i128::from(rank.total_score) - i128::from(related_rank.total_score)
        });
    let shown = related
        .len()
        .min(MAX_DECISION_RECORD_CANDIDATES)
        .min(candidate_room);
    let mut omitted_why = BTreeMap::<CodeStr, usize>::new();
    for (entry, _) in &related[shown..] {
        *omitted_why.entry(last_reason_code(entry)).or_default() += 1;
    }
    let mut reasons = Vec::with_capacity(2 + shown);
    reasons.push(reason(
        PolicyCode::RankBreakdown,
        bounded_detail(rank_breakdown_detail(rank, record, lead)),
    ));
    reasons.push(reason(
        PolicyCode::DecisionRecord,
        bounded_detail(decision_record_detail(
            &target_policy_field(target_policies.get(instance_id)),
            related.len(),
            shown,
            &omitted_why,
        )),
    ));
    for (entry, view) in &related[..shown] {
        let why = last_reason_code(entry);
        let state = state_name(entry.state);
        let detail = match view {
            Some((related_rank, related_record)) => format!(
                "why={why} state={state} total={} behind_by={} {} promoted={} tie_breaker={}",
                related_rank.total_score,
                i128::from(rank.total_score) - i128::from(related_rank.total_score),
                rank_terms(
                    related_rank,
                    related_record.stage.as_ref(),
                    related_record.resources.as_ref()
                ),
                u8::from(related_record.promoted),
                related_record.tie_breaker
            ),
            None => format!(
                "why={why} state={state} eligibility={}",
                eligibility_name(entry.eligibility)
            ),
        };
        reasons.push(reason_with(
            Outcome::new(PolicyCode::DecisionCandidateNotSelected).task_id(entry.task_id.as_str()),
            bounded_detail(detail),
        ));
    }
    Ok(reasons)
}

/// Every path that leaves a candidate unselected ends with the reason that settled it, and a
/// decision left without one has been given `decision_reason_missing` before any record names it
/// (`settle_decisions_left_without_reason`), so `why` is never `none` (Workflow #386 SR-6a).
fn last_reason_code(entry: &TaskWork) -> CodeStr {
    entry.reasons.last().map_or_else(
        || CodeStr::from(SchedulingRulePolicyCode::DecisionReasonMissing.code()),
        |reason| reason.outcome.code().clone(),
    )
}

/// The ranking terms shared by `rank_breakdown` and a ranked `decision_candidate_not_selected`.
/// Under an active v2 policy `target` is the candidate's resource term `R`.
fn rank_terms(
    rank: &TaskRank,
    stage: Option<&StageRecord>,
    resources: Option<&ResourceRecord>,
) -> String {
    format!(
        "priority={} aging_ms={} strategic_milli={} urgency_milli={} contention={} effective={} score={} utility={} offset={} target={}",
        rank.priority,
        rank.aging_ms,
        rank.strategic_weight_milli,
        rank.urgency_milli,
        rank.contention_penalty,
        rank.effective_milli,
        optional_milli(stage.and_then(|stage| stage.score)),
        stage.map_or(0, |stage| stage.utility),
        stage.map_or(0, |stage| stage.offset),
        match resources {
            Some(resources) => resources.term.to_string(),
            None => stage
                .and_then(|stage| stage.target.as_ref())
                .map_or_else(|| "none".to_owned(), |(_, _, score)| score.to_string()),
        },
    )
}

/// `saturated` compares the total with the unsaturated sum of its terms. `lead` is the total
/// minus the highest total among the instance's other rank-recorded entries, `score_deferred`
/// ones included: negative when the winner was promoted above a higher total or a higher-total
/// entry was deferred by score, host budget or admission eligibility, and possibly positive
/// while a better-ranked (promoted) entry was deferred.
fn rank_breakdown_detail(rank: &TaskRank, record: &RankRecord, lead: Option<i128>) -> String {
    let stage = record.stage.as_ref();
    let resources = record.resources.as_ref();
    // Under an active v2 policy `target_id` names the effective override, `*` otherwise.
    let (target_id, mode) = match resources {
        Some(ResourceRecord {
            override_id: Some(id),
            ..
        }) => (id.as_str(), "override"),
        Some(_) => ("*", "adjust"),
        None => stage
            .and_then(|stage| stage.target.as_ref())
            .map_or(("-", "-"), |(id, mode, _)| {
                (id.as_str(), crate::resource_targets::mode_name(*mode))
            }),
    };
    let unsaturated = i128::from(rank.priority) * 1_000_000
        + i128::from(rank.aging_ms)
        + i128::from(rank.strategic_weight_milli) * 1_000
        + i128::from(rank.urgency_milli) * 1_000
        - i128::from(rank.contention_penalty)
        + i128::from(rank.effective_milli) * 1_000;
    let mut detail = format!(
        "total={} base={} {} target_id={target_id} mode={mode} superseded={} promoted={} affinity={} tie_breaker={} saturated={} lead={}",
        rank.total_score,
        record.base_total,
        rank_terms(rank, stage, resources),
        u8::from(stage.is_some_and(|stage| stage.superseded)),
        u8::from(record.promoted),
        u8::from(record.affinity),
        record.tie_breaker,
        u8::from(unsaturated != i128::from(rank.total_score)),
        lead.map_or_else(|| "none".to_owned(), |lead| lead.to_string()),
    );
    if let Some(resources) = resources {
        detail.push_str(&format!(" offset_kept={}", u8::from(resources.offset_kept)));
    }
    detail
}

/// The instance's stored target policy as its version `sha@applied_at`, or its state.
fn target_policy_field(policy: Option<&InstanceTargetPolicy>) -> String {
    let mut field = match policy.and_then(|policy| policy.state.as_ref()) {
        Some(TargetPolicyState::Active {
            policy_sha256,
            applied_at_unix_ms,
            ..
        }) => format!("active:{policy_sha256}@{applied_at_unix_ms}"),
        Some(TargetPolicyState::Expired {
            policy_sha256,
            applied_at_unix_ms,
            ..
        }) => format!("expired:{policy_sha256}@{applied_at_unix_ms}"),
        Some(TargetPolicyState::Unreadable { code }) => format!("unreadable:{code}"),
        Some(TargetPolicyState::ActiveV2 {
            policy_sha256,
            applied_at_unix_ms,
            ..
        }) => format!("active.v2:{policy_sha256}@{applied_at_unix_ms}"),
        Some(TargetPolicyState::ExpiredV2 {
            policy_sha256,
            applied_at_unix_ms,
            ..
        }) => format!("expired.v2:{policy_sha256}@{applied_at_unix_ms}"),
        None => "none".to_owned(),
    };
    if let Some((scope_kind, observed_at_unix_ms)) = policy.and_then(|policy| policy.ignored) {
        field.push_str(&format!(" ignored={scope_kind}@{observed_at_unix_ms}"));
    }
    field
}

/// `omitted_why` lists `<code>:<count>` in ascending code order, counted while joining so the
/// whole detail stays within 1010 bytes; the codes that do not fit fold into `+<n>more`.
fn decision_record_detail(
    targets: &str,
    related: usize,
    shown: usize,
    omitted_why: &BTreeMap<CodeStr, usize>,
) -> String {
    let mut detail = format!(
        "targets={targets} related={related} shown={shown} omitted={} omitted_why=",
        related - shown
    );
    if omitted_why.is_empty() {
        detail.push_str("none");
        return detail;
    }
    let items = omitted_why
        .iter()
        .map(|(code, count)| format!("{code}:{count}"))
        .collect::<Vec<_>>();
    for (index, item) in items.iter().enumerate() {
        let separator = usize::from(index > 0);
        let rest = items.len() - index - 1;
        // Room for the fold marker should a later code not fit.
        let reserve = if rest == 0 {
            0
        } else {
            format!(",+{rest}more").len()
        };
        if detail.len() + separator + item.len() + reserve > TRUNCATED_DETAIL_BYTES {
            if separator == 1 {
                detail.push(',');
            }
            detail.push_str(&format!("+{}more", items.len() - index));
            break;
        }
        if separator == 1 {
            detail.push(',');
        }
        detail.push_str(item);
    }
    detail
}

/// A detail over `MAX_TEXT_BYTES` is cut at a character boundary to at most 1010 bytes and
/// marked `…[truncated]`, 1024 bytes at most.
fn bounded_detail(mut detail: String) -> String {
    if detail.len() <= MAX_TEXT_BYTES {
        return detail;
    }
    let mut end = TRUNCATED_DETAIL_BYTES;
    while !detail.is_char_boundary(end) {
        end -= 1;
    }
    detail.truncate(end);
    detail.push_str(TRUNCATION_MARK);
    detail
}

const fn state_name(state: SchedulingDecisionState) -> &'static str {
    match state {
        SchedulingDecisionState::Eligible => "eligible",
        SchedulingDecisionState::Deferred => "deferred",
        SchedulingDecisionState::Blocked => "blocked",
        SchedulingDecisionState::Selected => "selected",
    }
}

const fn eligibility_name(eligibility: EligibilityState) -> &'static str {
    match eligibility {
        EligibilityState::True => "true",
        EligibilityState::False => "false",
        EligibilityState::Unknown => "unknown",
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
    yield_points: Vec<String>,
    load_profile: LoadProfile,
    load: ResourceLoad,
    rank: TaskRank,
    affinity: bool,
    /// Set by the score stage; a promoted candidate ranks ahead of every other candidate.
    promoted: bool,
    /// The task's declared value; absent means no utility term.
    value_milli: Option<u32>,
    /// The last settled run's duration for this task on this instance.
    last_duration_ms: Option<u64>,
    tie_breaker: u64,
    facts_fresh_until_unix_ms: Option<u64>,
    activity_profile_id: String,
    daily_limit: u32,
    window_iteration_limit: u32,
    max_runtime_ms: u64,
}

enum PlacementResult {
    Candidate(Box<PlacementCandidate>),
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
            PolicyCode::InstanceUnavailable,
            "the instance is unavailable",
        )));
    }
    if !instance
        .capability_operation_ids
        .iter()
        .any(|operation| operation == &task.entrypoint.operation_id)
    {
        return Ok(PlacementResult::Blocked(reason(
            PolicyCode::CapabilityMissing,
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
            PolicyCode::InstanceOverrideDisabled,
            "the task is disabled by its instance override",
        )));
    }

    let Some(activity_profile) = select_activity_profile(context.profiles, instance) else {
        return Ok(PlacementResult::Blocked(reason(
            PolicyCode::ActivityProfileMissing,
            format!(
                "instance '{}' has no matching activity profile",
                instance.instance_id
            ),
        )));
    };

    let load_profile = task_override
        .and_then(|value| value.load_profile.0.clone())
        .unwrap_or_else(|| task.load_profile.clone());
    let host = context
        .hosts
        .get(instance.host_id.as_str())
        .ok_or_else(|| {
            PolicyEvaluationError::invalid(format!(
                "instance '{}' references missing host '{}'",
                instance.instance_id, instance.host_id
            ))
        })?;
    let load = ResourceLoad::new(&load_profile, host.heavy_threshold_milli);
    if !host.fits(load) {
        let (code, detail) =
            if load.heavy && host.active_heavy_dispatches >= host.heavy_dispatch_limit {
                (
                    PolicyCode::HeavySceneBudgetExhausted,
                    format!(
                        "host '{}' has no remaining heavy-scene concurrency budget",
                        instance.host_id
                    ),
                )
            } else {
                (
                    PolicyCode::HostCapacityInsufficient,
                    format!(
                        "host '{}' cannot satisfy the task load profile",
                        instance.host_id
                    ),
                )
            };
        return Ok(PlacementResult::Blocked(reason(code, detail)));
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
        load,
        host,
        context.time,
    )?;

    Ok(PlacementResult::Candidate(Box::new(PlacementCandidate {
        work_index,
        task_id: task.id.clone(),
        instance_id: instance.instance_id.clone(),
        host_id: instance.host_id.clone(),
        operation_id: task.entrypoint.operation_id.clone(),
        procedure_ref: task.procedure_ref.clone(),
        expected_duration_ms: task.expected_duration_ms,
        yield_points: task.yield_points.clone(),
        load_profile,
        load,
        rank,
        affinity: instance
            .preferred_task_ids
            .iter()
            .any(|task_id| task_id == &task.id),
        promoted: false,
        value_milli: task.value_milli,
        last_duration_ms: task_state.and_then(|state| state.last_duration_ms),
        tie_breaker: deterministic_tie_breaker(context.seed, &task.id, &instance.instance_id),
        facts_fresh_until_unix_ms,
        activity_profile_id: activity_profile.id.clone(),
        daily_limit: task.loop_budget.daily_limit,
        window_iteration_limit: task.loop_budget.window_iteration_limit,
        max_runtime_ms: task.loop_budget.max_runtime_ms,
    })))
}

/// What the selection document said about one candidate, before any manual offset.
#[derive(Debug, Clone)]
enum CandidateScore {
    /// `known_until_unix_ms` is the last instant every document fact the score read stays
    /// known.
    Scored {
        score_milli: i64,
        known_until_unix_ms: Option<u64>,
    },
    /// A gate decided against the candidate, so it carries no score; names that gate.
    Unscored {
        gate_id: String,
        known_until_unix_ms: Option<u64>,
    },
    /// A gate or term met an input no reading can supply: the candidate is deferred with these
    /// reasons and no wake, and takes no part in ranking (Workflow #384, D2 S1).
    InputUnknown(Vec<DecisionReason>),
    /// Gates or terms met only unknown realistic keys: the candidate is deferred and takes no
    /// part in ranking; the fact refresh gives its reasons (Workflow #384, D2 S1, S4).
    RefreshNeeded(Vec<RefreshKey>),
}

/// How one scored candidate leaves the score stage, with the reason detail it records.
enum ScoreDisposition {
    Deferred {
        wake_unix_ms: u64,
        detail: String,
    },
    Promoted {
        detail: String,
    },
    /// It would have been deferred, but it has been eligible for at least the aging cap.
    AgingCap {
        aging_ms: u64,
        cap_ms: u64,
    },
    Neutral,
}

impl ScoreDisposition {
    const fn label(&self) -> &'static str {
        match self {
            Self::Deferred { .. } => "deferred",
            Self::Promoted { .. } | Self::AgingCap { .. } => "promoted",
            Self::Neutral => "none",
        }
    }
}

/// The utility term `value_milli * urgency_total_milli / cost_ms` of one candidate.
struct CandidateUtility {
    utility_milli: i64,
    /// `None` when the task declares no value.
    cost_ms: Option<u64>,
    urgency_total_milli: u64,
    /// A declared value whose cost is zero: the term is dropped and the candidate stays neutral.
    cost_unknown: bool,
}

fn candidate_utility(
    candidate: &PlacementCandidate,
    aging_ms_per_milli: Option<u32>,
) -> PolicyEvaluationResult<CandidateUtility> {
    let aging_urgency_milli = match aging_ms_per_milli {
        None => 0,
        Some(0) => {
            return Err(PolicyEvaluationError::invalid(
                "priority_selection aging_ms_per_milli must be at least 1",
            ));
        }
        Some(per_milli) => {
            (candidate.rank.aging_ms / u64::from(per_milli)).min(MAX_AGING_URGENCY_MILLI)
        }
    };
    let urgency_total_milli = u64::from(candidate.rank.urgency_milli) + aging_urgency_milli;
    let Some(value_milli) = candidate.value_milli else {
        return Ok(CandidateUtility {
            utility_milli: 0,
            cost_ms: None,
            urgency_total_milli,
            cost_unknown: false,
        });
    };
    let cost_ms = candidate
        .last_duration_ms
        .filter(|duration| *duration > 0)
        .unwrap_or(candidate.expected_duration_ms);
    if cost_ms == 0 {
        return Ok(CandidateUtility {
            utility_milli: 0,
            cost_ms: Some(0),
            urgency_total_milli,
            cost_unknown: true,
        });
    }
    let overflow = || {
        PolicyEvaluationError::score_overflow(format!(
            "task '{}' utility overflowed",
            candidate.task_id
        ))
    };
    let urgency = i64::try_from(urgency_total_milli).map_err(|_| overflow())?;
    let cost = i64::try_from(cost_ms).map_err(|_| overflow())?;
    // Integer division truncates toward zero.
    let utility_milli = i64::from(value_milli)
        .checked_mul(urgency)
        .ok_or_else(overflow)?
        / cost;
    Ok(CandidateUtility {
        utility_milli,
        cost_ms: Some(cost_ms),
        urgency_total_milli,
        cost_unknown: false,
    })
}

#[derive(Clone, Copy)]
enum ThresholdSource {
    Absolute,
    Percentile { defer: u8, promote: u8 },
}

/// The thresholds one evaluation cycle applies to every scored candidate.
struct CycleThresholds {
    source: ThresholdSource,
    /// Absolute: deferred strictly below. Percentile: deferred at or below, because a
    /// nearest-rank percentile is the upper end of the bottom share it names; `None` for the
    /// 0th percentile, which bounds no candidate.
    defer_below_milli: Option<i64>,
    promote_above_milli: Option<i64>,
}

impl CycleThresholds {
    /// Percentiles apply to the effective scores of all of the cycle's Eligible candidates
    /// across instances; a cycle with fewer than three uses the absolute pair.
    fn new(selection: &PrioritySelection, effective: &[i64]) -> Self {
        match (
            selection.defer_below_percentile,
            selection.promote_above_percentile,
        ) {
            (Some(defer), Some(promote)) if effective.len() >= MIN_PERCENTILE_CANDIDATES => {
                let mut sorted = effective.to_vec();
                sorted.sort_unstable();
                Self {
                    source: ThresholdSource::Percentile { defer, promote },
                    defer_below_milli: nearest_rank(&sorted, defer),
                    promote_above_milli: nearest_rank(&sorted, promote),
                }
            }
            _ => Self {
                source: ThresholdSource::Absolute,
                defer_below_milli: Some(selection.defer_below_milli),
                promote_above_milli: Some(selection.promote_above_milli),
            },
        }
    }

    fn defer_detail(&self, effective_milli: i64) -> Option<String> {
        let threshold = self.defer_below_milli?;
        match self.source {
            ThresholdSource::Absolute if effective_milli < threshold => Some(format!(
                "effective score {effective_milli} is below defer_below_milli {threshold}"
            )),
            ThresholdSource::Percentile { defer, .. } if effective_milli <= threshold => {
                Some(format!(
                    "effective score {effective_milli} is at or below the {defer}th-percentile threshold {threshold}"
                ))
            }
            _ => None,
        }
    }

    fn promote_detail(&self, effective_milli: i64) -> Option<String> {
        let threshold = self.promote_above_milli?;
        if effective_milli <= threshold {
            return None;
        }
        Some(match self.source {
            ThresholdSource::Absolute => format!(
                "effective score {effective_milli} is above promote_above_milli {threshold}"
            ),
            ThresholdSource::Percentile { promote, .. } => format!(
                "effective score {effective_milli} is above the {promote}th-percentile threshold {threshold}"
            ),
        })
    }

    fn reason_fields(&self) -> String {
        let source = match self.source {
            ThresholdSource::Absolute => "absolute",
            ThresholdSource::Percentile { .. } => "percentile",
        };
        format!(
            " threshold_source={source} defer_below={} promote_above={}",
            optional_milli(self.defer_below_milli),
            optional_milli(self.promote_above_milli)
        )
    }
}

/// Nearest-rank percentile: the smallest value with at least `percentile` percent of the
/// sorted values at or below it. The 0th percentile has no rank.
fn nearest_rank(sorted: &[i64], percentile: u8) -> Option<i64> {
    let rank = usize::from(percentile)
        .saturating_mul(sorted.len())
        .div_ceil(100);
    rank.checked_sub(1)
        .and_then(|index| sorted.get(index))
        .copied()
}

fn optional_milli(value: Option<i64>) -> String {
    value.map_or_else(|| "none".to_owned(), |value| value.to_string())
}

/// One candidate between computing its effective score and applying the cycle thresholds.
struct StagedCandidate {
    candidate: PlacementCandidate,
    score_milli: Option<i64>,
    unknown_rule: Option<String>,
    offset_milli: i64,
    utility: CandidateUtility,
    /// score + utility + offset: the cycle thresholds are computed from it.
    base_milli: i64,
    effective_milli: i64,
    target: TargetEffect,
}

/// The score stage between the predicate gate and ranking. Predicate outcomes are never
/// revisited here: only candidates that already passed trigger, feedback stop, cooldown and
/// placement are scored. Without a selection document, without a declared value, without
/// offsets and without a resource target policy it is a no-op.
///
/// Workflow #384 D2-S4: with `refreshing`, a candidate whose document read unknown realistic
/// keys only is deferred and returned as affected, for the fact refresh to give its reasons;
/// without it, it is deferred under `fact_refresh_unbound` (D2-S1).
#[allow(clippy::type_complexity)]
fn apply_priority_selection(
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    time: EvaluationTime,
    candidates: Vec<PlacementCandidate>,
    work: &mut [TaskWork],
    target_policies: &BTreeMap<String, InstanceTargetPolicy>,
    refreshing: bool,
) -> PolicyEvaluationResult<(Vec<PlacementCandidate>, Vec<AffectedCandidate>)> {
    let policy = catalog.selection_policy();
    // The utility fields join the `score_computed` detail only once the catalog can score or
    // value a task, so an offset-only evaluation keeps its detail byte for byte.
    let utility_fields = policy.is_some()
        || catalog
            .catalog()
            .tasks
            .tasks
            .iter()
            .any(|task| task.value_milli.is_some());
    if !utility_fields && facts.priority_offsets.is_empty() && target_policies.is_empty() {
        return Ok((candidates, Vec::new()));
    }
    // Each instance's stored policy is resolved once: every target's pool and named tasks
    // against the catalog, its inventory in the time-validity projected facts.
    let mut instance_targets = BTreeMap::new();
    for (instance_id, target_policy) in target_policies {
        let instance = facts
            .instances
            .iter()
            .find(|instance| instance.instance_id == *instance_id)
            .ok_or_else(|| {
                PolicyEvaluationError::invalid(format!(
                    "resource target policy names unknown instance '{instance_id}'"
                ))
            })?;
        let resolved = crate::resource_targets::resolve_instance_targets(
            catalog,
            facts,
            instance,
            target_policy,
            time,
        )
        .map_err(PolicyEvaluationError::invalid)?;
        instance_targets.insert(instance_id.as_str(), resolved);
    }
    let selection = catalog.catalog().tasks.priority_selection;
    let aging_ms_per_milli = selection.and_then(|selection| selection.aging_ms_per_milli);
    let scores = match policy {
        Some(policy) => score_candidates(policy, facts, time, &candidates)?,
        None => vec![None; candidates.len()],
    };
    let mut ranked = Vec::with_capacity(candidates.len());
    let mut staged = Vec::with_capacity(candidates.len());
    let mut affected = Vec::new();
    for (mut candidate, score) in candidates.into_iter().zip(scores) {
        let (score_milli, unknown_rule, known_until_unix_ms) = match score {
            None => (None, None, None),
            Some(CandidateScore::Scored {
                score_milli,
                known_until_unix_ms,
            }) => (Some(score_milli), None, known_until_unix_ms),
            Some(CandidateScore::Unscored {
                gate_id,
                known_until_unix_ms,
            }) => (None, Some(gate_id), known_until_unix_ms),
            // An unknown input of the document defers only this candidate, with no wake: it is
            // evaluated again when its facts, the catalog or the configuration change, and it
            // takes no part in ranking (D2 S1; model §3.3, §3.7 rows 1-2).
            Some(CandidateScore::InputUnknown(reasons)) => {
                let task_work = &mut work[candidate.work_index];
                task_work.state = SchedulingDecisionState::Deferred;
                task_work.reasons.extend(reasons);
                continue;
            }
            // Unknown realistic keys only: the fact refresh gives the reasons (D2-S4; model
            // §3.7 rows 2-6), with the rank this candidate lends the reader; without it, no reader
            // is bound (D2-S1).
            Some(CandidateScore::RefreshNeeded(keys)) => {
                let task_work = &mut work[candidate.work_index];
                task_work.state = SchedulingDecisionState::Deferred;
                if refreshing {
                    affected.push(AffectedCandidate {
                        work_index: candidate.work_index,
                        instance_id: candidate.instance_id.clone(),
                        rank: offset_rank(
                            &candidate.rank,
                            facts,
                            &candidate.task_id,
                            &candidate.instance_id,
                        ),
                        keys,
                        predicate: false,
                    });
                } else {
                    task_work.reasons.extend(keys.iter().map(|key| {
                        fact_refresh::unbound_reason(&candidate.instance_id, key, None)
                    }));
                }
                continue;
            }
        };
        // Every document fact the score read bounds the candidate's freshness, and the
        // evaluation wakes when the first of them lapses, as for an applied resource target
        // (D2 S1; model §3.8).
        if let Some(known_until) = known_until_unix_ms {
            candidate.facts_fresh_until_unix_ms =
                min_wake(candidate.facts_fresh_until_unix_ms, Some(known_until));
            if let Some(wake) = known_until
                .checked_add(1)
                .filter(|wake| *wake > time.unix_ms)
            {
                let task_work = &mut work[candidate.work_index];
                task_work.next_wake_unix_ms = min_wake(task_work.next_wake_unix_ms, Some(wake));
            }
        }
        let targets = instance_targets.get(candidate.instance_id.as_str());
        let target = targets.map_or(TargetEffect::None, |targets| {
            targets.effect(&candidate.task_id)
        });
        let offset = effective_priority_offset(facts, &candidate.task_id, &candidate.instance_id);
        // Under an active v2 policy a candidate enters the stage when its resource term is
        // positive or an override covering it is effective (Workflow #335 S2b).
        if policy.is_none()
            && offset.is_none()
            && candidate.value_milli.is_none()
            && target.applied().is_none()
            && !target
                .resources()
                .is_some_and(crate::resource_targets::ResourceTerm::takes_part)
        {
            // Released untouched: its rank record keeps `stage: None`.
            let task_work = &mut work[candidate.work_index];
            if let Some(targets) = targets {
                task_work.reasons.extend(targets.reasons(&target, None, 0));
            }
            if let Some(resources) = target.resources() {
                task_work
                    .record
                    .as_mut()
                    .ok_or_else(|| {
                        PolicyEvaluationError::invalid(format!(
                            "task '{}' reached the score stage without a rank record",
                            candidate.task_id
                        ))
                    })?
                    .resources = Some(ResourceRecord::new(resources));
            }
            ranked.push(candidate);
            continue;
        }
        let offset_milli = i64::from(offset.unwrap_or(0));
        let utility = candidate_utility(&candidate, aging_ms_per_milli)?;
        let overflow = || {
            PolicyEvaluationError::overflow(format!(
                "task '{}' effective score overflowed",
                candidate.task_id
            ))
        };
        let base_milli = score_milli
            .unwrap_or(0)
            .checked_add(utility.utility_milli)
            .and_then(|sum| sum.checked_add(offset_milli))
            .ok_or_else(overflow)?;
        // Adjust adds the task target score to the base; override replaces the selection
        // score and the manual offset with it and keeps the utility term. A v2 resource term
        // is added to the base, or under an effective override to the utility term and, unless
        // the override supersedes it, the manual offset.
        let effective_milli = match target.applied() {
            None => match target.resources() {
                None => base_milli,
                Some(resources) => {
                    let term_milli = i64::try_from(resources.term_milli).map_err(|_| overflow())?;
                    match &resources.override_target {
                        None => base_milli.checked_add(term_milli),
                        Some((_, ManualOffset::Keep)) => utility
                            .utility_milli
                            .checked_add(term_milli)
                            .and_then(|sum| sum.checked_add(offset_milli)),
                        Some((_, ManualOffset::Supersede)) => {
                            utility.utility_milli.checked_add(term_milli)
                        }
                    }
                    .ok_or_else(overflow)?
                }
            },
            Some(applied) => {
                let task_target_milli =
                    i64::try_from(applied.task_target_milli).map_err(|_| overflow())?;
                match applied.mode {
                    TargetMode::Adjust => base_milli.checked_add(task_target_milli),
                    TargetMode::Override => utility.utility_milli.checked_add(task_target_milli),
                }
                .ok_or_else(overflow)?
            }
        };
        staged.push(StagedCandidate {
            candidate,
            score_milli,
            unknown_rule,
            offset_milli,
            utility,
            base_milli,
            effective_milli,
            target,
        });
    }
    let thresholds = selection.map(|selection| {
        let base = staged
            .iter()
            .map(|staged| staged.base_milli)
            .collect::<Vec<_>>();
        (selection, CycleThresholds::new(&selection, &base))
    });
    for StagedCandidate {
        mut candidate,
        score_milli,
        unknown_rule,
        offset_milli,
        utility,
        base_milli: _,
        effective_milli,
        target,
    } in staged
    {
        candidate.rank.total_score = candidate
            .rank
            .total_score
            .saturating_add(effective_milli.saturating_mul(1_000));
        candidate.rank.effective_milli = effective_milli;
        let applied = target.applied();
        let resources = target.resources();
        let overridden = applied.is_some_and(|applied| applied.mode == TargetMode::Override)
            || resources.is_some_and(|resources| resources.override_target.is_some());
        // An unknown verdict or cost never defers or promotes; only a scored one meets the
        // thresholds. An applied override is never deferred or promoted either.
        let disposition = match (&thresholds, score_milli) {
            _ if overridden => ScoreDisposition::Neutral,
            (Some((selection, thresholds)), Some(_)) if !utility.cost_unknown => {
                if let Some(detail) = thresholds.defer_detail(effective_milli) {
                    match selection.defer_aging_cap_ms {
                        Some(cap_ms) if candidate.rank.aging_ms >= cap_ms => {
                            ScoreDisposition::AgingCap {
                                aging_ms: candidate.rank.aging_ms,
                                cap_ms,
                            }
                        }
                        _ => ScoreDisposition::Deferred {
                            wake_unix_ms: time
                                .unix_ms
                                .checked_add(selection.defer_for_ms)
                                .ok_or_else(|| {
                                    PolicyEvaluationError::overflow(
                                        "score deferral wake overflowed",
                                    )
                                })?,
                            detail,
                        },
                    }
                } else if let Some(detail) = thresholds.promote_detail(effective_milli) {
                    ScoreDisposition::Promoted { detail }
                } else {
                    ScoreDisposition::Neutral
                }
            }
            _ => ScoreDisposition::Neutral,
        };
        let mut detail = format!(
            "score={} offset={offset_milli} effective={effective_milli} disposition={}",
            score_milli.map_or_else(|| "none".to_owned(), |score| score.to_string()),
            disposition.label()
        );
        if utility_fields {
            detail.push_str(&format!(
                " utility={} cost_ms={} urgency_total={}",
                utility.utility_milli,
                utility
                    .cost_ms
                    .map_or_else(|| "-".to_owned(), |cost| cost.to_string()),
                utility.urgency_total_milli
            ));
        }
        if let Some((_, thresholds)) = &thresholds {
            detail.push_str(&thresholds.reason_fields());
        }
        if let Some(applied) = applied {
            detail.push_str(&format!(
                " target={}:{}:{}",
                applied.id,
                crate::resource_targets::mode_name(applied.mode),
                applied.task_target_milli
            ));
        }
        if let Some(resources) = resources {
            detail.push_str(&format!(" resources={}", resources.term_milli));
            if let Some((id, manual_offset)) = &resources.override_target {
                detail.push_str(&format!(
                    " override={id}:{}",
                    crate::resource_targets::manual_offset_name(*manual_offset)
                ));
            }
        }
        let task_work = &mut work[candidate.work_index];
        task_work.rank = Some(candidate.rank.clone());
        let record = task_work.record.as_mut().ok_or_else(|| {
            PolicyEvaluationError::invalid(format!(
                "task '{}' reached the score stage without a rank record",
                candidate.task_id
            ))
        })?;
        record.stage = Some(StageRecord {
            score: score_milli,
            utility: utility.utility_milli,
            offset: offset_milli,
            target: applied
                .map(|applied| (applied.id.clone(), applied.mode, applied.task_target_milli)),
            superseded: overridden,
        });
        record.resources = resources.map(ResourceRecord::new);
        record.promoted = matches!(
            disposition,
            ScoreDisposition::Promoted { .. } | ScoreDisposition::AgingCap { .. }
        );
        task_work
            .reasons
            .push(reason(PolicyCode::ScoreComputed, detail));
        if let Some(rule) = unknown_rule {
            task_work.reasons.push(reason_with(
                Outcome::new(PolicyCode::ScoreUnknown).rule_id(rule),
                match applied.map(|applied| applied.mode) {
                    Some(TargetMode::Adjust) => {
                        "the selection document could not score this candidate; the utility term, the manual offset and the applied resource target apply"
                    }
                    Some(TargetMode::Override) => {
                        "the selection document could not score this candidate; the utility term and the applied resource target apply, the manual offset is superseded"
                    }
                    None if utility.cost_ms.is_some() => {
                        "the selection document could not score this candidate; only the utility term and the manual offset apply"
                    }
                    None => {
                        "the selection document could not score this candidate; only the manual offset applies"
                    }
                },
            ));
        }
        if utility.cost_unknown {
            task_work.reasons.push(reason(
                PolicyCode::ScoreCostUnknown,
                "neither the last run duration nor the declared expected duration is above zero; the utility term is dropped",
            ));
        }
        if let Some(targets) = instance_targets.get(candidate.instance_id.as_str()) {
            task_work
                .reasons
                .extend(targets.reasons(&target, score_milli, offset_milli));
        }
        // An applied target is only as fresh as its inventory observation and its policy, and
        // the evaluation wakes when either lapses (RT17).
        if let Some(applied) = applied {
            candidate.facts_fresh_until_unix_ms = min_wake(
                min_wake(
                    candidate.facts_fresh_until_unix_ms,
                    applied.inventory_expires_at_unix_ms,
                ),
                Some(applied.valid_until_unix_ms),
            );
            for lapse in [
                applied.inventory_expires_at_unix_ms,
                Some(applied.valid_until_unix_ms),
            ]
            .into_iter()
            .flatten()
            {
                let wake = lapse.checked_add(1).ok_or_else(|| {
                    PolicyEvaluationError::overflow("resource target wake overflowed")
                })?;
                if wake > time.unix_ms {
                    task_work.next_wake_unix_ms = min_wake(task_work.next_wake_unix_ms, Some(wake));
                }
            }
        }
        // A candidate a v2 policy moves is only as fresh as the policy and every inventory
        // observation behind a shortfall weighing it, and wakes when one lapses.
        if let Some(resources) = resources.filter(|resources| resources.takes_part()) {
            for lapse in resources
                .inventory_expiries
                .iter()
                .copied()
                .chain([resources.valid_until_unix_ms])
            {
                candidate.facts_fresh_until_unix_ms =
                    min_wake(candidate.facts_fresh_until_unix_ms, Some(lapse));
                let wake = lapse.checked_add(1).ok_or_else(|| {
                    PolicyEvaluationError::overflow("resource target wake overflowed")
                })?;
                if wake > time.unix_ms {
                    task_work.next_wake_unix_ms = min_wake(task_work.next_wake_unix_ms, Some(wake));
                }
            }
        }
        match disposition {
            ScoreDisposition::Deferred {
                wake_unix_ms,
                detail,
            } => {
                task_work.state = SchedulingDecisionState::Deferred;
                task_work.reasons.push(reason(
                    PolicyCode::ScoreDeferred,
                    format!("{detail}; reevaluation at {wake_unix_ms}"),
                ));
                task_work.next_wake_unix_ms =
                    min_wake(task_work.next_wake_unix_ms, Some(wake_unix_ms));
            }
            ScoreDisposition::Promoted { detail } => {
                candidate.promoted = true;
                task_work
                    .reasons
                    .push(reason(PolicyCode::ScorePromoted, detail));
                ranked.push(candidate);
            }
            ScoreDisposition::AgingCap { aging_ms, cap_ms } => {
                candidate.promoted = true;
                task_work.reasons.push(reason(
                    PolicyCode::ScoreAgingCap,
                    format!("aging_ms={aging_ms} cap_ms={cap_ms}"),
                ));
                ranked.push(candidate);
            }
            ScoreDisposition::Neutral => ranked.push(candidate),
        }
    }
    Ok((ranked, affected))
}

/// Runs the selection document once per instance over that instance's eligible candidates
/// and its own fact projection, under the collect option. Per-candidate scores do not depend
/// on the other candidates, so grouping by instance changes nothing but which facts the
/// snapshot can answer. The document's `on_unknown` clauses do not apply at this layer: a
/// candidate that meets an unknown input is deferred on its own, and the evaluation goes on.
fn score_candidates(
    policy: &SelectionPolicy,
    facts: &EvaluationFacts,
    time: EvaluationTime,
    candidates: &[PlacementCandidate],
) -> PolicyEvaluationResult<Vec<Option<CandidateScore>>> {
    let mut by_instance: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (index, candidate) in candidates.iter().enumerate() {
        by_instance
            .entry(candidate.instance_id.as_str())
            .or_default()
            .push(index);
    }
    let mut scores = vec![None; candidates.len()];
    for (instance_id, indices) in by_instance {
        let instance = facts
            .instances
            .iter()
            .find(|instance| instance.instance_id == instance_id)
            .ok_or_else(|| {
                PolicyEvaluationError::invalid(format!(
                    "candidate instance '{instance_id}' is missing from the snapshot"
                ))
            })?;
        let projection = instance_fact_projection(facts, instance, time.unix_ms);
        let snapshot = SelectionFactSnapshot {
            snapshot_id: facts.fact_snapshot_id.clone(),
            snapshot_at_unix_ms: time.unix_ms,
            facts: projection.entries,
        };
        let selection_candidates = indices
            .iter()
            .map(|&index| selection_candidate(&candidates[index], &projection.fields))
            .collect::<PolicyEvaluationResult<Vec<_>>>()?;
        let collected = actingcommand_selection_policy::evaluate_collect(
            policy,
            &selection_candidates,
            &snapshot,
            time.unix_ms,
        )
        .map_err(|error| {
            PolicyEvaluationError::selection_invalid(format!("instance '{instance_id}': {error}"))
        })?;
        for (&index, selection_candidate) in indices.iter().zip(&selection_candidates) {
            let result = collected
                .iter()
                .find(|result| result.candidate_id == selection_candidate.candidate_id)
                .ok_or_else(|| {
                    PolicyEvaluationError::selection_invalid(format!(
                        "verdict for candidate '{}' is missing",
                        selection_candidate.candidate_id
                    ))
                })?;
            // The facts a score read: a `fact` source carries its own bound, an instance-fact
            // field its record's expiry; a fixed field has none. A `keyed_fact` read never comes
            // back here (the collect option refuses a record-list document), so it is a fact
            // source only to keep the match total.
            let known_until_unix_ms = result
                .known_inputs
                .iter()
                .filter_map(|known| match &known.input {
                    ValueRef::Fact { .. } | ValueRef::KeyedFact { .. } => known.known_until_unix_ms,
                    ValueRef::Field { field } => projection.field_expiries.get(field).copied(),
                })
                .min();
            scores[index] = Some(match &result.verdict {
                CollectedVerdict::Scored { score_milli } => CandidateScore::Scored {
                    score_milli: *score_milli,
                    known_until_unix_ms,
                },
                CollectedVerdict::GateRejected { gate_id } => CandidateScore::Unscored {
                    gate_id: gate_id.clone(),
                    known_until_unix_ms,
                },
                CollectedVerdict::Unknown { inputs } => {
                    unknown_inputs(instance_id, inputs, &projection.expired_fields)
                }
            });
        }
    }
    Ok(scores)
}

/// The instance fact families recognition writes: the realistic keys (model §3.1).
const REALISTIC_FACT_PREFIXES: [&str; 2] = ["resource.", "inventory."];

/// What a candidate the document met unknown inputs for carries, by the first row of model
/// §3.7 that matches:
///
/// 1. any input that is not a realistic key (an instruction key, pack data, a fixed field or a
///    lookup that covers no value) gives one `selection_input_unknown` per gate or term that
///    met one: a reading cannot supply it;
/// 2. otherwise every unknown key, once, with the rule that read it first, for the fact refresh
///    (D2-S4) or, without it, for `fact_refresh_unbound` with no reader problem (D2-S1,
///    decision D6).
fn unknown_inputs(
    instance_id: &str,
    inputs: &[UnknownInput],
    expired_fields: &BTreeSet<String>,
) -> CandidateScore {
    let inputs = inputs
        .iter()
        .map(|unknown| {
            let (key, unknown_reason) = match &unknown.input {
                // The collect option refuses a record-list document, so a `keyed_fact` input is
                // never listed; its list's fact key keeps the match total.
                ValueRef::Fact { fact_key } | ValueRef::KeyedFact { fact_key, .. } => {
                    (fact_key.as_str(), unknown.reason)
                }
                // A field that names an instance fact whose record has expired is read as
                // expired, not as missing (decision D1).
                ValueRef::Field { field }
                    if unknown.reason == UnknownReason::FieldMissing
                        && expired_fields.contains(field) =>
                {
                    (field.as_str(), UnknownReason::FactExpired)
                }
                ValueRef::Field { field } => (field.as_str(), unknown.reason),
            };
            (unknown.rule_id.as_str(), key, unknown_reason)
        })
        .collect::<Vec<_>>();
    let mut seen = BTreeSet::new();
    if inputs
        .iter()
        .all(|(_, key, unknown_reason)| is_realistic(key, *unknown_reason))
    {
        CandidateScore::RefreshNeeded(
            inputs
                .iter()
                .filter(|(_, key, _)| seen.insert(*key))
                .map(|(rule_id, key, unknown_reason)| RefreshKey {
                    fact_key: (*key).to_owned(),
                    unknown_reason: *unknown_reason,
                    rule_id: Some((*rule_id).to_owned()),
                })
                .collect(),
        )
    } else {
        CandidateScore::InputUnknown(
            inputs
                .iter()
                .filter(|(_, key, unknown_reason)| !is_realistic(key, *unknown_reason))
                .filter(|(rule_id, _, _)| seen.insert(*rule_id))
                .map(|(rule_id, _, unknown_reason)| {
                    reason_with(
                        Outcome::new(PolicyCode::SelectionInputUnknown)
                            .instance_id(instance_id)
                            .rule_id(*rule_id)
                            .unknown_reason(UnknownReasonToken(*unknown_reason)),
                        format!(
                            "instance_id={instance_id} rule_id={rule_id} unknown_reason={}",
                            unknown_token(*unknown_reason)
                        ),
                    )
                })
                .collect(),
        )
    }
}

/// A realistic input: a key of a family recognition writes, read without a lookup miss (model
/// §3.1). A lookup that covers no value is not something a reading can fix.
fn is_realistic(key: &str, unknown_reason: UnknownReason) -> bool {
    unknown_reason != UnknownReason::LookupMiss
        && REALISTIC_FACT_PREFIXES
            .iter()
            .any(|prefix| key.starts_with(prefix))
}

/// A selection-policy reason as a token of the catalog vocabulary `unknown_reason`, so a reason's
/// outcome can carry it (Workflow #378 A2-P).
#[derive(Clone, Copy)]
struct UnknownReasonToken(UnknownReason);

impl VocabularyToken for UnknownReasonToken {
    const VOCABULARY: &'static str = "unknown_reason";

    fn token(self) -> &'static str {
        unknown_token(self.0)
    }
}

/// The token of the `unknown_reason` vocabulary for one selection-policy reason.
const fn unknown_token(unknown_reason: UnknownReason) -> &'static str {
    match unknown_reason {
        UnknownReason::FactMissing => "fact_missing",
        UnknownReason::FactExpired => "fact_expired",
        UnknownReason::FactStale => "fact_stale",
        UnknownReason::FactLowConfidence => "fact_low_confidence",
        UnknownReason::FactNotScalar => "fact_not_scalar",
        UnknownReason::FieldMissing => "field_missing",
        // The vocabulary token shares the registered code's spelling.
        UnknownReason::TypeMismatch => SharedCode::TypeMismatch.as_str(),
        UnknownReason::LookupMiss => "lookup_miss",
    }
}

struct InstanceFactProjection {
    entries: BTreeMap<String, SelectionFactEntry>,
    fields: BTreeMap<String, ScalarValue>,
    /// The expiry of each field whose record has one.
    field_expiries: BTreeMap<String, u64>,
    /// The keys whose record has expired and so carry no field.
    expired_fields: BTreeSet<String>,
}

/// Projects the facts one instance can see (its own, its server's and its game's scope, the
/// most specific scope winning per key) into the snapshot the document reads as `fact`
/// sources and the scalar fields every candidate on that instance carries. A field holds only
/// a record that has not expired at `now_unix_ms`, by the predicate the fact contract and the
/// `fact` source share (D2 S1, decision D2); the snapshot keeps an expired record, which the
/// selection-policy crate reads as expired.
fn instance_fact_projection(
    facts: &EvaluationFacts,
    instance: &InstanceSnapshot,
    now_unix_ms: u64,
) -> InstanceFactProjection {
    let mut chosen: BTreeMap<&str, (u8, &ObservedFact)> = BTreeMap::new();
    for fact in &facts.facts {
        if !scope_matches_instance(&fact.scope, instance) {
            continue;
        }
        let specificity = activity_scope_specificity(&fact.scope);
        match chosen.get(fact.fact_key.as_str()) {
            Some((previous, _)) if *previous >= specificity => {}
            _ => {
                chosen.insert(fact.fact_key.as_str(), (specificity, fact));
            }
        }
    }
    let mut projection = InstanceFactProjection {
        entries: BTreeMap::new(),
        fields: BTreeMap::new(),
        field_expiries: BTreeMap::new(),
        expired_fields: BTreeSet::new(),
    };
    for (key, (_, fact)) in chosen {
        let entry = match selection_scalar(&fact.value) {
            Ok(value) => {
                match fact.expires_at_unix_ms {
                    Some(expiry) if now_unix_ms > expiry => {
                        projection.expired_fields.insert(key.to_owned());
                    }
                    expiry => {
                        if let Some(expiry) = expiry {
                            projection.field_expiries.insert(key.to_owned(), expiry);
                        }
                        projection.fields.insert(key.to_owned(), value.clone());
                    }
                }
                SelectionFactEntry::Published {
                    value,
                    published_at_unix_ms: fact.observed_at_unix_ms,
                    expires_at_unix_ms: fact.expires_at_unix_ms,
                    confidence_milli: fact.confidence_milli,
                }
            }
            Err(reason) => SelectionFactEntry::Unusable { reason },
        };
        projection.entries.insert(key.to_owned(), entry);
    }
    projection
}

fn selection_scalar(value: &FactValue) -> Result<ScalarValue, UnknownReason> {
    match value {
        FactValue::Boolean(value) => Ok(ScalarValue::Boolean(*value)),
        FactValue::Integer(value) => Ok(ScalarValue::Integer(*value)),
        FactValue::String(value) => Ok(ScalarValue::String(value.clone())),
        FactValue::TimestampMs(value) | FactValue::DurationMs(value) => i64::try_from(*value)
            .map(ScalarValue::Integer)
            .map_err(|_| UnknownReason::TypeMismatch),
        FactValue::RecordList(_) => Err(UnknownReason::FactNotScalar),
    }
}

fn selection_candidate(
    candidate: &PlacementCandidate,
    instance_fields: &BTreeMap<String, ScalarValue>,
) -> PolicyEvaluationResult<Candidate> {
    let aging_ms = i64::try_from(candidate.rank.aging_ms).map_err(|_| {
        PolicyEvaluationError::overflow(format!(
            "task '{}' aging exceeds the scoring range",
            candidate.task_id
        ))
    })?;
    let mut fields = instance_fields.clone();
    for (name, value) in [
        (
            "task.priority",
            ScalarValue::Integer(i64::from(candidate.rank.priority)),
        ),
        (
            "task.strategic_weight_milli",
            ScalarValue::Integer(i64::from(candidate.rank.strategic_weight_milli)),
        ),
        (
            "task.urgency_milli",
            ScalarValue::Integer(i64::from(candidate.rank.urgency_milli)),
        ),
        ("task.aging_ms", ScalarValue::Integer(aging_ms)),
        (
            "task.load_cost_milli",
            ScalarValue::Integer(i64::from(candidate.rank.load_cost_milli)),
        ),
        (
            "instance.affinity",
            ScalarValue::Boolean(candidate.affinity),
        ),
    ] {
        fields.insert(name.to_owned(), value);
    }
    Ok(Candidate {
        candidate_id: format!("{}@{}", candidate.task_id, candidate.instance_id),
        fields,
    })
}

/// Workflow #384 D2-S4 (D2 model §3.3, catalog predicates): a task whose undecided trigger or
/// feedback stop read unknown realistic keys in exactly its instance's scope is affected by
/// them, with the rank its placement on the instance gives it; a task the instance cannot place
/// is not.
fn predicate_refresh(
    work_index: usize,
    task: &TaskSpec,
    state: Option<&TaskRuntimeSnapshot>,
    instance: &InstanceSnapshot,
    context: &PlacementContext<'_>,
    facts: &EvaluationFacts,
    suggestions: &[DetectionSuggestion],
) -> PolicyEvaluationResult<Option<AffectedCandidate>> {
    let keys = fact_refresh::predicate_keys(suggestions, instance);
    if keys.is_empty() {
        return Ok(None);
    }
    let PlacementResult::Candidate(candidate) =
        build_candidate(work_index, task, state, instance, context, None)?
    else {
        return Ok(None);
    };
    Ok(Some(AffectedCandidate {
        work_index,
        instance_id: instance.instance_id.clone(),
        keys,
        rank: offset_rank(&candidate.rank, facts, &task.id, &instance.instance_id),
        predicate: true,
    }))
}

/// The rank a candidate lends its instance's recognition maintenance package (D2 model §3.4):
/// its catalog rank with its manual offset, in the unit the score stage adds it in.
fn offset_rank(
    rank: &TaskRank,
    facts: &EvaluationFacts,
    task_id: &str,
    instance_id: &str,
) -> TaskRank {
    let offset = effective_priority_offset(facts, task_id, instance_id).unwrap_or(0);
    let mut rank = rank.clone();
    rank.total_score = rank
        .total_score
        .saturating_add(i64::from(offset).saturating_mul(1_000));
    rank
}

/// The instance-level entry wins over the task-level entry; neither means no offset.
fn effective_priority_offset(
    facts: &EvaluationFacts,
    task_id: &str,
    instance_id: &str,
) -> Option<i32> {
    let mut task_level = None;
    for offset in &facts.priority_offsets {
        if offset.task_id != task_id {
            continue;
        }
        match offset.instance_id.as_deref() {
            Some(instance) if instance == instance_id => return Some(offset.offset_milli),
            Some(_) => {}
            None => task_level = Some(offset.offset_milli),
        }
    }
    task_level
}

fn select_activity_profile<'a>(
    profiles: &'a [ActivityProfile],
    instance: &InstanceSnapshot,
) -> Option<&'a ActivityProfile> {
    profiles
        .iter()
        .filter(|profile| scope_matches_instance(&profile.scope, instance))
        .max_by(|left, right| {
            activity_scope_specificity(&left.scope)
                .cmp(&activity_scope_specificity(&right.scope))
                .then_with(|| right.id.cmp(&left.id))
        })
}

/// One half-open occurrence of a declared activity window, in declaration order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityWindowOccurrence {
    pub local_day: i64,
    pub window_id: String,
    pub from_unix_ms: u64,
    pub until_unix_ms: u64,
}

impl ActivityWindowOccurrence {
    fn contains(&self, timestamp: u64) -> bool {
        self.from_unix_ms <= timestamp && timestamp < self.until_unix_ms
    }
}

/// Pure activity-window calculation shared with Runtime's admission owner.
pub fn activity_window_at(
    profile: &ActivityProfile,
    now_unix_ms: u64,
) -> PolicyEvaluationResult<Option<ActivityWindowOccurrence>> {
    const DAY_MS: i128 = 86_400_000;
    const MINUTE_MS: i128 = 60_000;
    for (index, window) in profile.windows.iter().enumerate() {
        let offset = i128::from(window.utc_offset_minutes) * MINUTE_MS;
        let local_ms = i128::from(now_unix_ms) + offset;
        let day = local_ms.div_euclid(DAY_MS);
        let minute = local_ms.rem_euclid(DAY_MS) / MINUTE_MS;
        let start = i128::from(window.start_minute_of_day);
        let end = i128::from(window.end_minute_of_day);
        let full_day = start == end;
        let active_day = if !full_day && start > end && minute < end {
            day - 1
        } else {
            day
        };
        let weekday = ((active_day + 3).rem_euclid(7) + 1) as u8;
        let active = full_day
            || if start < end {
                minute >= start && minute < end
            } else {
                minute >= start || minute < end
            };
        if !active || !window.weekdays.contains(&weekday) {
            continue;
        }
        let from = active_day * DAY_MS + if full_day { 0 } else { start * MINUTE_MS } - offset;
        let until = active_day * DAY_MS
            + if full_day {
                DAY_MS
            } else if start > end {
                DAY_MS + end * MINUTE_MS
            } else {
                end * MINUTE_MS
            }
            - offset;
        let local_day = i64::try_from(active_day)
            .map_err(|_| PolicyEvaluationError::overflow("activity window day overflowed"))?;
        return Ok(Some(ActivityWindowOccurrence {
            local_day,
            window_id: format!("{}:{local_day}:{index}", profile.id),
            from_unix_ms: u64::try_from(from.max(0))
                .map_err(|_| PolicyEvaluationError::overflow("activity window start overflowed"))?,
            until_unix_ms: u64::try_from(until)
                .map_err(|_| PolicyEvaluationError::overflow("activity window end overflowed"))?,
        }));
    }
    Ok(None)
}

pub(crate) const fn activity_scope_specificity(scope: &ScopeSelector) -> u8 {
    match scope {
        ScopeSelector::Instance { .. } => 3,
        ScopeSelector::Server { .. } => 2,
        ScopeSelector::Game { .. } => 1,
    }
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
    load: ResourceLoad,
    host: &HostRemaining,
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
    let contention_basis_points = host
        .third_party_pressure_basis_points
        .max(10_000u16.saturating_sub(host.host_responsiveness_basis_points));
    let contention_penalty = i64::from(load.cost_milli)
        .saturating_mul(i64::from(contention_basis_points))
        .saturating_mul(100);
    let total_score = priority_score
        .saturating_add(aging_score)
        .saturating_add(strategic_score)
        .saturating_add(urgency_score)
        .saturating_sub(contention_penalty);

    Ok(TaskRank {
        priority,
        aging_ms,
        strategic_weight_milli,
        urgency_milli,
        load_cost_milli: load.cost_milli,
        contention_penalty,
        total_score,
        effective_milli: 0,
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
    if matches!(spec.projection, PoolProjection::NonRegenerating { .. }) {
        return Some(0);
    }
    let value = values.get(effect.pool_id.as_str())?;
    let ratio = value.value.min(spec.capacity).saturating_mul(1_000) / spec.capacity;
    u16::try_from(ratio).ok()
}

#[derive(Debug, Clone, Copy)]
struct ResourceLoad {
    cpu: u16,
    gpu: u16,
    io: u16,
    cost_milli: u16,
    heavy: bool,
}

impl ResourceLoad {
    /// A weighted load is heavy when `cpu_milli + gpu_milli + io_milli` reaches
    /// `heavy_threshold_milli`, the threshold of the host it would run on (Workflow #385 PM-CFG).
    fn new(value: &LoadProfile, heavy_threshold_milli: u64) -> Self {
        match value {
            LoadProfile::Light => Self {
                cpu: 100,
                gpu: 100,
                io: 100,
                cost_milli: 100,
                heavy: false,
            },
            LoadProfile::Heavy => Self {
                cpu: 700,
                gpu: 700,
                io: 700,
                cost_milli: 700,
                heavy: true,
            },
            LoadProfile::Weighted {
                cpu_milli,
                gpu_milli,
                io_milli,
            } => Self {
                cpu: *cpu_milli,
                gpu: *gpu_milli,
                io: *io_milli,
                cost_milli: (*cpu_milli).max(*gpu_milli).max(*io_milli),
                heavy: u64::from(*cpu_milli)
                    .saturating_add(u64::from(*gpu_milli))
                    .saturating_add(u64::from(*io_milli))
                    >= heavy_threshold_milli,
            },
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct HostRemaining {
    cpu: u16,
    gpu: u16,
    io: u16,
    host_responsiveness_basis_points: u16,
    third_party_pressure_basis_points: u16,
    heavy_dispatch_limit: u16,
    active_heavy_dispatches: u16,
    heavy_threshold_milli: u64,
}

impl HostRemaining {
    fn fits(&self, load: ResourceLoad) -> bool {
        self.cpu >= load.cpu
            && self.gpu >= load.gpu
            && self.io >= load.io
            && (!load.heavy || self.active_heavy_dispatches < self.heavy_dispatch_limit)
    }

    fn consume(&mut self, load: ResourceLoad) {
        self.cpu -= load.cpu;
        self.gpu -= load.gpu;
        self.io -= load.io;
        if load.heavy {
            self.active_heavy_dispatches += 1;
        }
    }
}

impl From<&HostResourceSnapshot> for HostRemaining {
    fn from(value: &HostResourceSnapshot) -> Self {
        Self {
            cpu: value.cpu_available_milli,
            gpu: value.gpu_available_milli,
            io: value.io_available_milli,
            host_responsiveness_basis_points: value.host_responsiveness_basis_points,
            third_party_pressure_basis_points: value.third_party_pressure_basis_points,
            heavy_dispatch_limit: value.heavy_dispatch_limit,
            active_heavy_dispatches: value.active_heavy_dispatches,
            heavy_threshold_milli: value.heavy_threshold_milli,
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

#[allow(clippy::too_many_arguments)]
fn evaluate_predicate(
    predicate: &PredicateSpec,
    task_state: Option<&TaskRuntimeSnapshot>,
    decision_scope: &ScopeSelector,
    facts: &EvaluationFacts,
    pool_specs: &BTreeMap<&str, &PoolSpec>,
    pool_values: &BTreeMap<&str, &PoolValueSnapshot>,
    time: EvaluationTime,
    timeline_events: &BTreeMap<&str, &TimelineEvent>,
    reasons: &mut Vec<DecisionReason>,
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
                    timeline_events,
                    reasons,
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
                    timeline_events,
                    reasons,
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
                timeline_events,
                reasons,
            )?;
            result.truth = match result.truth {
                PredicateTruth::True => PredicateTruth::False,
                PredicateTruth::False => PredicateTruth::True,
                PredicateTruth::Unknown => PredicateTruth::Unknown,
            };
            Ok(result)
        }
        PredicateSpec::Clock { schedule, missed } => {
            let (latest, next) = schedule_occurrences(schedule, time)?;
            let last_dispatched = task_state.and_then(|state| state.last_dispatched_unix_ms);
            let unserved =
                latest.filter(|occurrence| last_dispatched.is_none_or(|last| last < *occurrence));
            // Workflow #361 R7: with `missed`, an unserved occurrence is due only before its
            // expiry. `latest` and `time.unix_ms` are the schedule's own coordinate shifted by
            // one and the same offset, so comparing them in Unix time is comparing them there.
            let (Some(occurrence), Some(missed)) = (unserved, missed) else {
                return Ok(PredicateEvaluation::known(unserved.is_some(), next));
            };
            let MissedSlotRule::SkipAfter { grace_ms } = *missed;
            let expiry = occurrence_expiry(occurrence, grace_ms)?;
            if time.unix_ms < expiry {
                let mut result = PredicateEvaluation::known(true, min_wake(next, Some(expiry)));
                result.fresh_until_unix_ms = Some(expiry);
                return Ok(result);
            }
            reasons.push(reason_with(
                Outcome::new(PolicyCode::ClockOccurrenceExpired)
                    .occurrence_unix_ms(occurrence)
                    .grace_ms(grace_ms),
                format!(
                    "clock occurrence {occurrence} was not served within its grace of {grace_ms} ms"
                ),
            ));
            Ok(PredicateEvaluation::known(false, next))
        }
        PredicateSpec::TimelineActive { event_id } => {
            let event = timeline_events.get(event_id.as_str()).ok_or_else(|| {
                PolicyEvaluationError::invalid(format!(
                    "compiled timeline reference '{event_id}' is missing"
                ))
            })?;
            let applicable = facts.instances.iter().any(|instance| {
                scope_matches_instance(decision_scope, instance)
                    && scope_matches_instance(&event.scope, instance)
            });
            let availability = timeline_availability(event, time, applicable)?;
            if availability.state == EligibilityState::Unknown {
                reasons.push(reason(
                    PolicyCode::TimelineUnavailable,
                    format!("event '{event_id}' has no positive applicable interval"),
                ));
                let mut result = PredicateEvaluation::known(false, None);
                result.truth = PredicateTruth::Unknown;
                return Ok(result);
            }
            let window = availability.active_interval;
            let next = availability.next_wake_unix_ms;
            reasons.push(reason(
                PolicyCode::TimelineActive,
                format!(
                    "event '{event_id}' at {}: interval {window:?}; next boundary {next:?}",
                    time.unix_ms
                ),
            ));
            let mut result = PredicateEvaluation::known(window.is_some(), next);
            result.fresh_until_unix_ms = next;
            Ok(result)
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
                return Ok(PredicateEvaluation::unknown(resource_suggestion(
                    spec, facts, time,
                )));
            };
            let projected = project_pool_value(spec, snapshot, time.unix_ms)?;
            let projected = i64::try_from(projected).map_err(|_| {
                PolicyEvaluationError::overflow(format!(
                    "projected pool '{pool_id}' value exceeds i64"
                ))
            })?;
            let mut result = PredicateEvaluation::known(
                compare_i64(projected, *comparison, *value)?,
                next_pool_projection_change(spec, snapshot, time.unix_ms)?,
            );
            result.fresh_until_unix_ms =
                pool_fact(spec, facts).and_then(|fact| fact.expires_at_unix_ms);
            result.next_wake_unix_ms = min_wake(
                result.next_wake_unix_ms,
                result
                    .fresh_until_unix_ms
                    .and_then(|expiry| expiry.checked_add(1)),
            );
            Ok(result)
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
            if observation.confidence_milli == 0 {
                return Ok(PredicateEvaluation::unknown(DetectionSuggestion {
                    scope: scope.clone(),
                    fact_key: fact_key.clone(),
                    reason: "fact_low_confidence".to_owned(),
                }));
            }
            let max_age_expiration = max_age_ms
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
            let fresh_until = min_wake(max_age_expiration, observation.expires_at_unix_ms);
            if fresh_until.is_some_and(|expiration| time.unix_ms > expiration) {
                return Ok(PredicateEvaluation::unknown(DetectionSuggestion {
                    scope: scope.clone(),
                    fact_key: fact_key.clone(),
                    reason: if observation
                        .expires_at_unix_ms
                        .is_some_and(|expiration| time.unix_ms > expiration)
                    {
                        "fact_expired".to_owned()
                    } else {
                        "fact_stale".to_owned()
                    },
                }));
            }
            // Workflow #355 D12: an observed kind the predicate does not compare makes this
            // predicate Unknown; it no longer fails the whole evaluation.
            if fact_kind(&observation.value) != fact_kind(value) {
                return Ok(PredicateEvaluation::unknown(DetectionSuggestion {
                    scope: scope.clone(),
                    fact_key: fact_key.clone(),
                    reason: SharedCode::TypeMismatch.as_str().to_owned(),
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
        PredicateSpec::RecordDeadline {
            scope,
            fact_key,
            timestamp_field,
            within_ms,
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
            if observation.confidence_milli == 0 {
                return Ok(PredicateEvaluation::unknown(DetectionSuggestion {
                    scope: scope.clone(),
                    fact_key: fact_key.clone(),
                    reason: "fact_low_confidence".to_owned(),
                }));
            }
            let max_age_expiration = max_age_ms
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
            let fresh_until = min_wake(max_age_expiration, observation.expires_at_unix_ms);
            if fresh_until.is_some_and(|expiration| time.unix_ms > expiration) {
                return Ok(PredicateEvaluation::unknown(DetectionSuggestion {
                    scope: scope.clone(),
                    fact_key: fact_key.clone(),
                    reason: if observation
                        .expires_at_unix_ms
                        .is_some_and(|expiration| time.unix_ms > expiration)
                    {
                        "fact_expired".to_owned()
                    } else {
                        "fact_stale".to_owned()
                    },
                }));
            }
            let FactValue::RecordList(records) = &observation.value else {
                return Ok(PredicateEvaluation::unknown(DetectionSuggestion {
                    scope: scope.clone(),
                    fact_key: fact_key.clone(),
                    reason: SharedCode::TypeMismatch.as_str().to_owned(),
                }));
            };
            let cutoff = time.unix_ms.checked_add(*within_ms).ok_or_else(|| {
                PolicyEvaluationError::overflow(format!(
                    "record deadline window for '{}:{}' overflowed",
                    scope_key(scope),
                    fact_key
                ))
            })?;
            let mut actionable = false;
            let mut next_wake = fresh_until
                .and_then(|expiration| expiration.checked_add(1))
                .filter(|expiration| *expiration > time.unix_ms);
            for record in records {
                let Some(value) = record.get(timestamp_field) else {
                    return Ok(PredicateEvaluation::unknown(DetectionSuggestion {
                        scope: scope.clone(),
                        fact_key: fact_key.clone(),
                        reason: "field_missing".to_owned(),
                    }));
                };
                let FactScalar::TimestampMs(deadline) = value else {
                    return Ok(PredicateEvaluation::unknown(DetectionSuggestion {
                        scope: scope.clone(),
                        fact_key: fact_key.clone(),
                        reason: SharedCode::TypeMismatch.as_str().to_owned(),
                    }));
                };
                if *deadline > time.unix_ms && *deadline <= cutoff {
                    actionable = true;
                    next_wake = min_wake(next_wake, deadline.checked_add(1));
                } else if *deadline > cutoff {
                    next_wake = min_wake(next_wake, deadline.checked_sub(*within_ms));
                }
            }
            Ok(PredicateEvaluation {
                truth: if actionable {
                    PredicateTruth::True
                } else {
                    PredicateTruth::False
                },
                suggestions: Vec::new(),
                next_wake_unix_ms: next_wake,
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
            if observation
                .expires_at_unix_ms
                .is_some_and(|expiry| time.unix_ms > expiry)
            {
                return Ok(PredicateEvaluation::unknown(DetectionSuggestion {
                    scope: decision_scope.clone(),
                    fact_key: format!("outcome.{task_id}.{outcome_key}"),
                    reason: "outcome_expired".to_owned(),
                }));
            }
            if fact_kind(&observation.value) != fact_kind(value) {
                return Ok(PredicateEvaluation::unknown(DetectionSuggestion {
                    scope: decision_scope.clone(),
                    fact_key: format!("outcome.{task_id}.{outcome_key}"),
                    reason: SharedCode::TypeMismatch.as_str().to_owned(),
                }));
            }
            let mut result = PredicateEvaluation::known(
                compare_fact_values(&observation.value, *comparison, value)?,
                observation
                    .expires_at_unix_ms
                    .and_then(|expiry| expiry.checked_add(1)),
            );
            result.fresh_until_unix_ms = observation.expires_at_unix_ms;
            Ok(result)
        }
    }
}

fn scope_instance_id(scope: &ScopeSelector) -> Option<&str> {
    match scope {
        ScopeSelector::Instance { instance_id } => Some(instance_id),
        ScopeSelector::Server { .. } | ScopeSelector::Game { .. } => None,
    }
}

fn resource_suggestion(
    pool: &PoolSpec,
    facts: &EvaluationFacts,
    time: EvaluationTime,
) -> DetectionSuggestion {
    match &pool.observation {
        ObservationRef::Fact { fact_key } => DetectionSuggestion {
            scope: pool.scope.clone(),
            fact_key: fact_key.clone(),
            reason: match live_pool_unknown_reason(pool, fact_key, facts, time) {
                Some(unknown_reason) => unknown_token(unknown_reason).to_owned(),
                None => "resource_observation_missing".to_owned(),
            },
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

/// Workflow #390 POOL-1: why a `ledger_fact` pool has no value at `time`, as the unknown reason
/// of the fact it observes in its own scope: none, a confidence below the pool's minimum, an
/// expired record, or a value that is not an integer within `0..=capacity`. A reading of the key
/// resets the pool's base, so the pool's unknown value asks for a reading as a predicate on the
/// fact itself does (D2 model §3.3). `None` for a static snapshot pool, which no reading
/// supplies, and for a usable record the caller's pool values do not hold.
fn live_pool_unknown_reason(
    pool: &PoolSpec,
    fact_key: &str,
    facts: &EvaluationFacts,
    time: EvaluationTime,
) -> Option<UnknownReason> {
    let crate::PoolValueSource::LedgerFact {
        minimum_confidence_milli,
    } = pool.value_source
    else {
        return None;
    };
    let Some(fact) = facts
        .facts
        .iter()
        .find(|fact| fact.scope == pool.scope && fact.fact_key == fact_key)
    else {
        return Some(UnknownReason::FactMissing);
    };
    if fact.confidence_milli < minimum_confidence_milli {
        Some(UnknownReason::FactLowConfidence)
    } else if fact
        .expires_at_unix_ms
        .is_some_and(|expiry| time.unix_ms > expiry)
    {
        Some(UnknownReason::FactExpired)
    } else if !matches!(fact.value, FactValue::Integer(value)
        if u64::try_from(value).is_ok_and(|value| value <= pool.capacity))
    {
        Some(UnknownReason::TypeMismatch)
    } else {
        None
    }
}

pub(crate) fn project_pool_value(
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
    let PoolProjection::Regenerating(projection) = &spec.projection else {
        return Ok(snapshot.value);
    };
    let elapsed = now.saturating_sub(snapshot.observed_at_unix_ms);
    let periods = elapsed / projection.per_ms;
    let regenerated = periods.checked_mul(projection.amount).ok_or_else(|| {
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
    let PoolProjection::Regenerating(projection) = &spec.projection else {
        return Ok(None);
    };
    if projection.amount == 0 || project_pool_value(spec, snapshot, now)? >= spec.capacity {
        return Ok(None);
    }
    let elapsed = now.saturating_sub(snapshot.observed_at_unix_ms);
    let next_period = elapsed
        .checked_div(projection.per_ms)
        .and_then(|period| period.checked_add(1))
        .ok_or_else(|| {
            PolicyEvaluationError::overflow(format!(
                "pool '{}' next projection period overflowed",
                spec.id
            ))
        })?;
    let offset = next_period.checked_mul(projection.per_ms).ok_or_else(|| {
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
            (FactValue::RecordList(actual), FactValue::RecordList(expected))
                if expected.len() == 1 =>
            {
                Ok(actual.contains(&expected[0]))
            }
            _ => Err(PolicyEvaluationError::type_mismatch(
                "contains requires strings or a one-record list needle",
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
        FactValue::RecordList(_) => "record_list",
    }
}

pub(crate) fn scope_matches_instance(scope: &ScopeSelector, instance: &InstanceSnapshot) -> bool {
    scope_matches_identity(
        scope,
        &instance.instance_id,
        &instance.server_id,
        &instance.game_id,
    )
}

fn scope_matches_identity(scope: &ScopeSelector, instance: &str, server: &str, game: &str) -> bool {
    match scope {
        ScopeSelector::Instance { instance_id } => instance_id == instance,
        ScopeSelector::Server { server_id } => server_id == server,
        ScopeSelector::Game { game_id } => game_id == game,
    }
}

fn timeline_availability(
    event: &TimelineEvent,
    time: EvaluationTime,
    applicable: bool,
) -> PolicyEvaluationResult<TimelineAvailability> {
    if !applicable || event.duration_ms == 0 {
        return Ok(TimelineAvailability {
            state: EligibilityState::Unknown,
            active_interval: None,
            next_wake_unix_ms: None,
        });
    }
    let window = timeline_window(event, time)?;
    Ok(TimelineAvailability {
        state: if window.active.is_some() {
            EligibilityState::True
        } else {
            EligibilityState::False
        },
        active_interval: window.active,
        next_wake_unix_ms: window.next_wake,
    })
}

/// Resolves a V2 event on the same clock path as clock predicates. All boundaries
/// are Unix milliseconds, including the clipped interval carried into admission.
struct TimelineWindow {
    active: Option<(u64, u64)>,
    next_wake: Option<u64>,
}

fn timeline_window(
    event: &TimelineEvent,
    time: EvaluationTime,
) -> PolicyEvaluationResult<TimelineWindow> {
    let validity = event.validity.as_ref().ok_or_else(|| {
        PolicyEvaluationError::invalid("timeline_active requires explicit V2 validity")
    })?;
    if event.duration_ms == 0
        || validity
            .until_unix_ms
            .0
            .is_some_and(|until| time.unix_ms >= until)
    {
        return Ok(TimelineWindow {
            active: None,
            next_wake: None,
        });
    }
    let (latest, next) = schedule_occurrences(&event.schedule, time)?;
    if next.is_none() && !matches!(event.schedule, ClockSchedule::At { .. }) {
        return Err(PolicyEvaluationError::overflow(
            "timeline next occurrence is outside Unix time",
        ));
    }
    let mut wake = (validity.from_unix_ms > time.unix_ms).then_some(validity.from_unix_ms);
    wake = min_wake(wake, validity.until_unix_ms.0);
    wake = min_wake(
        wake,
        next.filter(|next| {
            *next >= validity.from_unix_ms
                && validity.until_unix_ms.0.is_none_or(|until| *next < until)
        }),
    );
    let mut active = None;
    if let Some(occurrence) = latest {
        let end = occurrence.checked_add(event.duration_ms).ok_or_else(|| {
            PolicyEvaluationError::overflow(format!(
                "timeline event '{}' duration overflowed",
                event.id
            ))
        })?;
        let start = occurrence.max(validity.from_unix_ms);
        let end = validity.until_unix_ms.0.map_or(end, |until| end.min(until));
        if start < end && time.unix_ms < end {
            wake = min_wake(wake, Some(end));
            if time.unix_ms >= start {
                active = Some((start, end));
            }
        }
    }
    Ok(TimelineWindow {
        active,
        next_wake: wake,
    })
}

/// Interval clocks shorter than this come due too often to be traced one slot at a time.
const CLOCK_SLOT_MIN_INTERVAL_MS: u64 = 3_600_000;
/// Workflow #372 K4: the lapse pass reads occurrences at most this long before the evaluation.
const CLOCK_SLOT_LAPSE_LOOKBACK_MS: u64 = 48 * 3_600_000;
/// Workflow #372 K4: the lapse pass reads at most this many occurrences of one pair.
const CLOCK_SLOT_LAPSE_LIMIT: usize = 32;

/// The clock slots of one (task, instance) pair at one evaluation time. In memory only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockSlotView {
    /// The greatest clock occurrence the pair has not served yet.
    pub due_occurrence: Option<u64>,
    /// The least next occurrence over the same clocks.
    pub next_occurrence: Option<u64>,
}

/// Reads the clock predicates of `task.trigger` with the trigger's own due rule: an occurrence
/// is due when the pair has no dispatch, or its last dispatch precedes the occurrence, and,
/// for a clock with `missed` (Workflow #361 R7), the evaluation is before the occurrence's
/// grace ran out. Every `clock` reached through `all` and `any` counts. A `not` subtree and
/// every other predicate kind are ignored. `state` is the pair's runtime snapshot.
///
/// Returns `None` when no clock counts, or when a counted clock is an interval shorter than one
/// hour. Otherwise the view holds the greatest due occurrence and the least next occurrence.
pub fn clock_slot_view(
    task: &TaskSpec,
    state: Option<&TaskRuntimeSnapshot>,
    time: EvaluationTime,
) -> PolicyEvaluationResult<Option<ClockSlotView>> {
    let Some(clocks) = counted_clocks(&task.trigger) else {
        return Ok(None);
    };
    let last_dispatched = state.and_then(|state| state.last_dispatched_unix_ms);
    let mut view = ClockSlotView {
        due_occurrence: None,
        next_occurrence: None,
    };
    for clock in clocks {
        let (latest, next) = schedule_occurrences(clock.schedule, time)?;
        let mut due =
            latest.filter(|occurrence| last_dispatched.is_none_or(|last| last < *occurrence));
        if let (Some(occurrence), Some(MissedSlotRule::SkipAfter { grace_ms })) =
            (due, clock.missed)
            && occurrence_expiry(occurrence, *grace_ms)? <= time.unix_ms
        {
            due = None;
        }
        view.due_occurrence = view.due_occurrence.max(due);
        view.next_occurrence = min_wake(view.next_occurrence, next);
    }
    Ok(Some(view))
}

/// How an occurrence the pair did not serve was lost (Workflow #372 K4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockSlotLapseKind {
    /// The grace of its clock's `missed` rule ran out before the next occurrence came.
    Expired { grace_ms: u64 },
    /// A later occurrence came, inside its grace or without one, and absorbed it.
    Superseded,
}

/// One unserved occurrence of a pair that is no longer due (Workflow #372 K4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockSlotLapse {
    pub occurrence_unix_ms: u64,
    /// The next occurrence over the counted clocks after this one (O′); for a superseded
    /// occurrence, the one that absorbed it. `None` when no clock occurs again.
    pub next_occurrence_unix_ms: Option<u64>,
    pub kind: ClockSlotLapseKind,
}

/// Workflow #372 K4: the occurrences of the pair's counted clocks (as `clock_slot_view` counts
/// them) that the pair has not served and that are lost at `time`, newest first. Only a pair
/// with a dispatch history has any. The occurrences read are those after its last dispatch, at
/// or before `time` and within 48 h before it, at most 32, newest first. With O′ the next
/// occurrence after O over every counted clock, O expired when its clock has `missed`, its
/// grace ran out at or before `time`, and O′ is absent or not before that; O was superseded
/// when O′ is at or before `time` and O did not expire. Occurrences of several clocks at one
/// instant are one occurrence, which has the largest grace of its clocks, and none when one of
/// them has no `missed`. Arithmetic only: whether the pair is live, in flight or already
/// recorded is the caller's.
pub fn clock_slot_lapses(
    task: &TaskSpec,
    state: Option<&TaskRuntimeSnapshot>,
    time: EvaluationTime,
) -> PolicyEvaluationResult<Vec<ClockSlotLapse>> {
    let Some(clocks) = counted_clocks(&task.trigger) else {
        return Ok(Vec::new());
    };
    let Some(last_dispatched) = state.and_then(|state| state.last_dispatched_unix_ms) else {
        return Ok(Vec::new());
    };
    let earliest = time.unix_ms.saturating_sub(CLOCK_SLOT_LAPSE_LOOKBACK_MS);
    let read = |occurrence: u64| occurrence > last_dispatched && occurrence >= earliest;
    let mut occurrences = BTreeMap::<u64, Option<u64>>::new();
    let mut next = None;
    for clock in clocks {
        next = min_wake(next, schedule_occurrences(clock.schedule, time)?.1);
        let grace = clock
            .missed
            .map(|MissedSlotRule::SkipAfter { grace_ms }| grace_ms);
        for occurrence in past_occurrences(clock.schedule, time, &read)? {
            occurrences
                .entry(occurrence)
                .and_modify(|existing| {
                    *existing = existing.zip(grace).map(|(left, right)| left.max(right));
                })
                .or_insert(grace);
        }
    }
    let mut lapses = Vec::new();
    let mut successor = next;
    for (occurrence, grace) in occurrences.into_iter().rev().take(CLOCK_SLOT_LAPSE_LIMIT) {
        let expired = match grace {
            Some(grace_ms) => {
                let expiry = occurrence_expiry(occurrence, grace_ms)?;
                (expiry <= time.unix_ms && successor.is_none_or(|after| expiry <= after))
                    .then_some(grace_ms)
            }
            None => None,
        };
        let kind = match expired {
            Some(grace_ms) => Some(ClockSlotLapseKind::Expired { grace_ms }),
            None => successor
                .is_some_and(|after| after <= time.unix_ms)
                .then_some(ClockSlotLapseKind::Superseded),
        };
        if let Some(kind) = kind {
            lapses.push(ClockSlotLapse {
                occurrence_unix_ms: occurrence,
                next_occurrence_unix_ms: successor,
                kind,
            });
        }
        successor = Some(occurrence);
    }
    Ok(lapses)
}

/// Workflow #372 K4 ("Live"): whether the pair would have come due, read as the evaluation
/// reads its trigger. The task is placed on the instance (its scope matches), and its trigger,
/// evaluated with `facts`, `resources` and `time` as `evaluate` evaluates it, is true once
/// every counted clock (as `clock_slot_view` counts them) is counted due. A false or unknown
/// trigger, for example a follower whose head failed or a task outside its timeline window, is
/// not live; an instance missing from `facts` is not either.
pub fn clock_slot_live(
    catalog: &CompiledCatalog,
    task: &TaskSpec,
    instance_id: &str,
    facts: &EvaluationFacts,
    resources: &EvaluationResources,
    time: EvaluationTime,
) -> PolicyEvaluationResult<bool> {
    let placed = facts.instances.iter().any(|instance| {
        instance.instance_id == instance_id && scope_matches_instance(&task.scope, instance)
    });
    if !placed {
        return Ok(false);
    }
    let effective_facts = project_time_validity(catalog, facts, time)?;
    let facts = &effective_facts;
    let bundle = catalog.catalog();
    let pool_specs: BTreeMap<&str, &PoolSpec> = bundle
        .pools
        .pools
        .iter()
        .map(|pool| (pool.id.as_str(), pool))
        .collect();
    // The pool values the evaluation reads: a static pool's, or a live pool's while the
    // record it reads is current.
    let pool_values: BTreeMap<&str, &PoolValueSnapshot> = resources
        .pools
        .iter()
        .filter(|snapshot| {
            pool_specs
                .get(snapshot.pool_id.as_str())
                .is_some_and(|pool| {
                    pool.value_source.is_static()
                        || pool_fact(pool, facts).is_some_and(|fact| {
                            fact.observed_at_unix_ms <= time.unix_ms
                                && fact
                                    .expires_at_unix_ms
                                    .is_none_or(|expiry| time.unix_ms <= expiry)
                        })
                })
        })
        .map(|pool| (pool.pool_id.as_str(), pool))
        .collect();
    let timeline_events: BTreeMap<&str, &TimelineEvent> = bundle
        .timeline
        .events
        .iter()
        .map(|event| (event.id.as_str(), event))
        .collect();
    let state = facts
        .tasks
        .iter()
        .find(|state| state.task_id == task.id && state.instance_id == instance_id);
    let trigger = evaluate_predicate(
        &counted_clocks_due(&task.trigger),
        state,
        &ScopeSelector::Instance {
            instance_id: instance_id.to_owned(),
        },
        facts,
        &pool_specs,
        &pool_values,
        time,
        &timeline_events,
        &mut Vec::new(),
    )?;
    Ok(trigger.truth == PredicateTruth::True)
}

/// A clock predicate `clock_slot_view` counts: its schedule and its `missed` rule.
struct CountedClock<'a> {
    schedule: &'a ClockSchedule,
    missed: &'a Option<MissedSlotRule>,
}

/// The counted clocks of a trigger: every `clock` reached through `all` and `any`; `None` when
/// there is none, or when one of them is an interval shorter than one hour.
fn counted_clocks(trigger: &PredicateSpec) -> Option<Vec<CountedClock<'_>>> {
    let mut clocks = Vec::new();
    collect_clock_schedules(trigger, &mut clocks);
    let short_interval = clocks.iter().any(|clock| {
        matches!(clock.schedule, ClockSchedule::Interval { every_ms, .. }
            if *every_ms < CLOCK_SLOT_MIN_INTERVAL_MS)
    });
    (!clocks.is_empty() && !short_interval).then_some(clocks)
}

fn collect_clock_schedules<'a>(predicate: &'a PredicateSpec, clocks: &mut Vec<CountedClock<'a>>) {
    match predicate {
        PredicateSpec::All { predicates } | PredicateSpec::Any { predicates } => {
            for child in predicates {
                collect_clock_schedules(child, clocks);
            }
        }
        PredicateSpec::Clock { schedule, missed } => {
            clocks.push(CountedClock { schedule, missed });
        }
        PredicateSpec::Not { .. }
        | PredicateSpec::TimelineActive { .. }
        | PredicateSpec::ResourceProjection { .. }
        | PredicateSpec::Fact { .. }
        | PredicateSpec::RecordDeadline { .. }
        | PredicateSpec::DependencyCompleted { .. }
        | PredicateSpec::Outcome { .. } => {}
    }
}

/// `trigger` with every counted clock (through `all` and `any`, not under a `not`) replaced by
/// a predicate that is always true: an `all` without members.
fn counted_clocks_due(trigger: &PredicateSpec) -> PredicateSpec {
    match trigger {
        PredicateSpec::All { predicates } => PredicateSpec::All {
            predicates: predicates.iter().map(counted_clocks_due).collect(),
        },
        PredicateSpec::Any { predicates } => PredicateSpec::Any {
            predicates: predicates.iter().map(counted_clocks_due).collect(),
        },
        PredicateSpec::Clock { .. } => PredicateSpec::All {
            predicates: Vec::new(),
        },
        PredicateSpec::Not { .. }
        | PredicateSpec::TimelineActive { .. }
        | PredicateSpec::ResourceProjection { .. }
        | PredicateSpec::Fact { .. }
        | PredicateSpec::RecordDeadline { .. }
        | PredicateSpec::DependencyCompleted { .. }
        | PredicateSpec::Outcome { .. } => trigger.clone(),
    }
}

/// Workflow #361 R7: the instant an occurrence's grace runs out; the occurrence is due only
/// before it.
fn occurrence_expiry(occurrence: u64, grace_ms: u64) -> PolicyEvaluationResult<u64> {
    occurrence
        .checked_add(grace_ms)
        .ok_or_else(|| PolicyEvaluationError::overflow("clock occurrence grace overflowed"))
}

/// The occurrences of `schedule` at or before `time`, newest first, for as long as `read`
/// accepts them, and at most `CLOCK_SLOT_LAPSE_LIMIT`.
fn past_occurrences(
    schedule: &ClockSchedule,
    time: EvaluationTime,
    read: &impl Fn(u64) -> bool,
) -> PolicyEvaluationResult<Vec<u64>> {
    let source = match schedule {
        ClockSchedule::Interval { clock_source, .. }
        | ClockSchedule::At { clock_source, .. }
        | ClockSchedule::Daily { clock_source, .. }
        | ClockSchedule::Weekly { clock_source, .. } => clock_source,
    };
    let clock = ClockCoordinate::new(source, time)?;
    let mut found = Vec::new();
    let mut cursor = clock.now_ms;
    while found.len() < CLOCK_SLOT_LAPSE_LIMIT {
        let at = ClockCoordinate {
            now_ms: cursor,
            unix_delta_ms: clock.unix_delta_ms,
            utc_offset_minutes: clock.utc_offset_minutes,
        };
        let Some(occurrence) = schedule_occurrences_on_clock(schedule, &at)?
            .0
            .filter(|occurrence| read(*occurrence))
        else {
            break;
        };
        found.push(occurrence);
        // The next cursor is the coordinate one millisecond before this occurrence.
        let Some(before) = u64::try_from(i128::from(occurrence) - clock.unix_delta_ms)
            .ok()
            .and_then(|coordinate| coordinate.checked_sub(1))
        else {
            break;
        };
        cursor = before;
    }
    Ok(found)
}

fn schedule_occurrences(
    schedule: &ClockSchedule,
    time: EvaluationTime,
) -> PolicyEvaluationResult<(Option<u64>, Option<u64>)> {
    let source = match schedule {
        ClockSchedule::Interval { clock_source, .. }
        | ClockSchedule::At { clock_source, .. }
        | ClockSchedule::Daily { clock_source, .. }
        | ClockSchedule::Weekly { clock_source, .. } => clock_source,
    };
    let clock = ClockCoordinate::new(source, time)?;
    schedule_occurrences_on_clock(schedule, &clock)
}

fn schedule_occurrences_on_clock(
    schedule: &ClockSchedule,
    clock: &ClockCoordinate,
) -> PolicyEvaluationResult<(Option<u64>, Option<u64>)> {
    let occurrences = match schedule {
        ClockSchedule::Interval {
            every_ms,
            anchor_ms,
            ..
        } => {
            if clock.now_ms < *anchor_ms {
                (None, Some(*anchor_ms))
            } else {
                let elapsed = clock.now_ms - *anchor_ms;
                let latest = anchor_ms.saturating_add((elapsed / every_ms) * every_ms);
                (Some(latest), latest.checked_add(*every_ms))
            }
        }
        ClockSchedule::At { at_ms, .. } => {
            if clock.now_ms < *at_ms {
                (None, Some(*at_ms))
            } else {
                (Some(*at_ms), None)
            }
        }
        ClockSchedule::Daily { minutes_of_day, .. } => {
            daily_occurrences(clock.utc_offset_minutes, minutes_of_day, clock.now_ms)
        }
        ClockSchedule::Weekly {
            weekday,
            minute_of_day,
            ..
        } => weekly_occurrences(
            clock.utc_offset_minutes,
            *weekday,
            *minute_of_day,
            clock.now_ms,
        ),
    };
    Ok((
        occurrences
            .0
            .map(|value| clock.to_unix_ms(value))
            .transpose()?,
        occurrences
            .1
            .map(|value| clock.to_unix_ms(value))
            .transpose()?,
    ))
}

/// Invalidation is an occurrence, independent of availability duration. The last
/// occurrence inside validity remains effective after the interval has closed.
fn invalidation_occurrences(
    event: &TimelineEvent,
    time: EvaluationTime,
) -> PolicyEvaluationResult<(Option<u64>, Option<u64>)> {
    let Some(validity) = &event.validity else {
        return schedule_occurrences(&event.schedule, time);
    };
    let source = match &event.schedule {
        ClockSchedule::Interval { clock_source, .. }
        | ClockSchedule::At { clock_source, .. }
        | ClockSchedule::Daily { clock_source, .. }
        | ClockSchedule::Weekly { clock_source, .. } => clock_source,
    };
    let clock = ClockCoordinate::new(source, time)?;
    let at = |unix_ms: u64| -> PolicyEvaluationResult<(Option<u64>, Option<u64>)> {
        let coordinate = i128::from(unix_ms) - clock.unix_delta_ms;
        if coordinate < 0 {
            return Ok((None, None));
        }
        let cursor = ClockCoordinate {
            now_ms: u64::try_from(coordinate)
                .map_err(|_| PolicyEvaluationError::overflow("reset coordinate overflowed"))?,
            unix_delta_ms: clock.unix_delta_ms,
            utc_offset_minutes: clock.utc_offset_minutes,
        };
        schedule_occurrences_on_clock(&event.schedule, &cursor)
    };
    let until = validity.until_unix_ms.0;
    let latest = if time.unix_ms < validity.from_unix_ms {
        None
    } else {
        at(until.map_or(time.unix_ms, |end| time.unix_ms.min(end - 1)))?
            .0
            .filter(|occurrence| *occurrence >= validity.from_unix_ms)
    };
    let next = if until.is_some_and(|end| time.unix_ms >= end) {
        None
    } else if time.unix_ms < validity.from_unix_ms {
        let (latest, next) = at(validity.from_unix_ms)?;
        latest
            .filter(|occurrence| *occurrence == validity.from_unix_ms)
            .or(next)
    } else {
        at(time.unix_ms)?.1
    };
    Ok((
        latest,
        next.filter(|occurrence| until.is_none_or(|end| *occurrence < end)),
    ))
}

fn next_schedule_occurrence(
    schedule: &ClockSchedule,
    time: EvaluationTime,
) -> PolicyEvaluationResult<Option<u64>> {
    Ok(schedule_occurrences(schedule, time)?.1)
}

struct ClockCoordinate {
    now_ms: u64,
    unix_delta_ms: i128,
    utc_offset_minutes: i16,
}

impl ClockCoordinate {
    fn new(source: &ClockSource, time: EvaluationTime) -> PolicyEvaluationResult<Self> {
        match source {
            ClockSource::Local => Ok(Self {
                now_ms: time.monotonic_ms,
                unix_delta_ms: i128::from(time.unix_ms) - i128::from(time.monotonic_ms),
                utc_offset_minutes: 0,
            }),
            ClockSource::Server {
                utc_offset_minutes,
                dst_offset_minutes,
                maintenance_drift_ms,
                ..
            }
            | ClockSource::Reveal {
                utc_offset_minutes,
                dst_offset_minutes,
                maintenance_drift_ms,
                ..
            } => {
                let nominal_now = i128::from(time.unix_ms) - i128::from(*maintenance_drift_ms);
                let now_ms = u64::try_from(nominal_now).map_err(|_| {
                    PolicyEvaluationError::overflow(
                        "clock maintenance drift moved the evaluation before the Unix epoch",
                    )
                })?;
                let effective_offset = i32::from(*utc_offset_minutes)
                    .checked_add(i32::from(*dst_offset_minutes))
                    .and_then(|value| i16::try_from(value).ok())
                    .ok_or_else(|| {
                        PolicyEvaluationError::overflow("clock UTC/DST offset overflowed")
                    })?;
                Ok(Self {
                    now_ms,
                    unix_delta_ms: i128::from(*maintenance_drift_ms),
                    utc_offset_minutes: effective_offset,
                })
            }
        }
    }

    fn to_unix_ms(&self, coordinate_ms: u64) -> PolicyEvaluationResult<u64> {
        u64::try_from(i128::from(coordinate_ms) + self.unix_delta_ms).map_err(|_| {
            PolicyEvaluationError::overflow("clock occurrence cannot be represented as Unix time")
        })
    }
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

fn consider_preload_hint(current: &mut Option<PreloadHint>, candidate: PreloadHint) {
    let replace = current.as_ref().is_none_or(|existing| {
        candidate.not_before_unix_ms < existing.not_before_unix_ms
            || (candidate.not_before_unix_ms == existing.not_before_unix_ms
                && candidate.task_id < existing.task_id)
    });
    if replace {
        *current = Some(candidate);
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
    fact_snapshot_id: &str,
    unix_ms: u64,
    seed: u64,
) -> String {
    let mut hasher = Sha256::new();
    hash_part(&mut hasher, catalog_hash.as_bytes());
    hash_part(&mut hasher, task_id.as_bytes());
    hash_part(&mut hasher, instance_id.as_bytes());
    hasher.update(ledger_position.to_be_bytes());
    hash_part(&mut hasher, fact_snapshot_id.as_bytes());
    hasher.update(unix_ms.to_be_bytes());
    hasher.update(seed.to_be_bytes());
    format!("decision:{:x}", hasher.finalize())
}

fn hash_part(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

/// A reason whose code carries no values.
fn reason(code: impl Into<Code>, detail: impl Into<String>) -> DecisionReason {
    reason_with(Outcome::new(code), detail)
}

/// A reason with its outcome built, values included.
fn reason_with(outcome: Outcome, detail: impl Into<String>) -> DecisionReason {
    DecisionReason {
        outcome,
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
    validate_count(
        "priority offsets",
        facts.priority_offsets.len(),
        MAX_EVALUATION_PRIORITY_OFFSETS,
    )?;
    validate_id("fact snapshot id", &facts.fact_snapshot_id)?;

    let mut offset_keys = BTreeSet::new();
    for offset in &facts.priority_offsets {
        validate_id("priority offset task id", &offset.task_id)?;
        if let Some(instance_id) = &offset.instance_id {
            validate_instance_alias(instance_id)?;
        }
        validate_observation_time("priority offset", offset.observed_at_unix_ms, time.unix_ms)?;
        if offset.offset_milli < -MAX_PRIORITY_OFFSET_MILLI
            || offset.offset_milli > MAX_PRIORITY_OFFSET_MILLI
        {
            return Err(PolicyEvaluationError::invalid(format!(
                "priority offset for task '{}' exceeds {MAX_PRIORITY_OFFSET_MILLI} milli",
                offset.task_id
            )));
        }
        if !offset_keys.insert((offset.task_id.as_str(), offset.instance_id.as_deref())) {
            return Err(PolicyEvaluationError::invalid(format!(
                "duplicate priority offset '{}:{}'",
                offset.task_id,
                offset.instance_id.as_deref().unwrap_or("*")
            )));
        }
    }

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
            || host.host_responsiveness_basis_points > 10_000
            || host.third_party_pressure_basis_points > 10_000
            || host.heavy_dispatch_limit == 0
            || host.active_heavy_dispatches > host.heavy_dispatch_limit
            || host.heavy_threshold_milli == 0
        {
            return Err(PolicyEvaluationError::invalid(format!(
                "host '{}' resource and contention limits are invalid",
                host.host_id
            )));
        }
    }

    let mut instance_ids = BTreeSet::new();
    for instance in &facts.instances {
        validate_instance_alias(&instance.instance_id)?;
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
        if fact.confidence_milli > 1_000
            || fact
                .expires_at_unix_ms
                .is_some_and(|expires| expires <= fact.observed_at_unix_ms)
            || !observed_fact_value_is_bounded(&fact.value)
        {
            return Err(PolicyEvaluationError::invalid(format!(
                "fact '{}:{}' metadata or value is invalid",
                scope_key(&fact.scope),
                fact.fact_key
            )));
        }
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
        validate_instance_alias(&outcome.instance_id)?;
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
        if outcome
            .expires_at_unix_ms
            .is_some_and(|expiry| expiry <= outcome.observed_at_unix_ms)
        {
            return Err(PolicyEvaluationError::invalid(
                "outcome expiration must follow observation",
            ));
        }
        if let Some(window_id) = &outcome.activity_window_id {
            validate_id("outcome activity window id", window_id)?;
        }
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
        validate_instance_alias(&state.instance_id)?;
        // Workflow #384 D2-S4: a recognition maintenance package's dispatches keep their clocks
        // under the reserved fact refresh task.
        if state.task_id != FACT_REFRESH_TASK_ID && !task_ids.contains(state.task_id.as_str()) {
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
        if state
            .last_duration_ms
            .is_some_and(|duration| duration > MAX_TASK_LAST_DURATION_MS)
            || state
                .failure_streak
                .is_some_and(|streak| streak > MAX_TASK_FAILURE_STREAK)
        {
            return Err(PolicyEvaluationError::invalid(format!(
                "task state '{}:{}' last duration exceeds {MAX_TASK_LAST_DURATION_MS} ms or failure streak exceeds {MAX_TASK_FAILURE_STREAK}",
                state.task_id, state.instance_id
            )));
        }
        if let Some(completed) = &state.completed_window {
            validate_id("completed activity window id", &completed.window_id)?;
            validate_observation_time(
                "completed activity window",
                completed.completed_at_unix_ms,
                time.unix_ms,
            )?;
            if state.terminal_state.is_none() {
                return Err(PolicyEvaluationError::invalid(
                    "completed activity window requires a settled terminal state",
                ));
            }
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

fn observed_fact_value_is_bounded(value: &FactValue) -> bool {
    match value {
        FactValue::String(value) => value.len() <= MAX_TEXT_BYTES,
        FactValue::RecordList(records) => {
            records.len() <= 256
                && records.iter().all(|record| {
                    record.len() <= 64
                        && record.iter().all(|(key, value)| {
                            !key.is_empty()
                                && key.len() <= MAX_TEXT_BYTES
                                && !key.chars().any(char::is_control)
                                && !matches!(
                                    value,
                                    crate::FactScalar::String(text)
                                        if text.len() > MAX_TEXT_BYTES
                                            || text.chars().any(char::is_control)
                                )
                        })
                })
        }
        FactValue::Boolean(_)
        | FactValue::Integer(_)
        | FactValue::TimestampMs(_)
        | FactValue::DurationMs(_) => true,
    }
}

fn validate_input_scope(
    label: &str,
    scope: &ScopeSelector,
    instance_ids: &BTreeSet<&str>,
) -> PolicyEvaluationResult<()> {
    match scope {
        ScopeSelector::Instance { instance_id } => {
            validate_instance_alias(instance_id)?;
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

fn validate_instance_alias(value: &str) -> PolicyEvaluationResult<()> {
    actingcommand_contract::validate_instance_alias(value).map_err(|_| {
        PolicyEvaluationError::invalid(
            "instance alias must contain 1 to 256 UTF-8 bytes without control characters",
        )
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{CatalogDocumentSource, CatalogSources, compile_catalog};

    pub(crate) const NOW: u64 = 3_600_000;

    // Specification criteria: https://github.com/HS7097/ActingCommand-Workflow/issues/269#issuecomment-5551152553
    #[test]
    fn timeline_v2_weekly_tables_follow_four_hour_boundaries() {
        const DAY: u64 = 86_400_000;
        let monday = 4 * DAY - 4 * 3_600_000;
        for days in [vec![1_u8, 3, 5], vec![2_u8, 4, 6]] {
            let mut docs = example_documents();
            for doc in [&mut docs.0, &mut docs.1, &mut docs.2, &mut docs.3] {
                doc["schema_version"] = serde_json::json!(crate::SCHEDULING_SCHEMA_VERSION_V2);
            }
            docs.3["events"] = serde_json::json!(days.iter().map(|day| serde_json::json!({
                "id": format!("neutral.day-{day}"), "scope": {"kind":"server","server_id":"fixture-server-a"},
                "event_kind":"activity", "schedule":{"kind":"weekly","weekday":day,"minute_of_day":240,
                    "clock_source":{"kind":"server","timezone_id":"fixed/plus-eight","utc_offset_minutes":480,"dst_offset_minutes":0,"maintenance_drift_ms":0}},
                "duration_ms":DAY, "invalidates_fact_prefixes":[], "validity":{"from_unix_ms":0,"until_unix_ms":null}
            })).collect::<Vec<_>>());
            docs.0["tasks"][0]["trigger"] = serde_json::json!({"kind":"any","predicates":days.iter().map(|day|
                serde_json::json!({"kind":"timeline_active","event_id":format!("neutral.day-{day}")})).collect::<Vec<_>>()});
            docs.0["tasks"][0]["feedback_stop"] = false_fact();
            let compiled = compile_documents(docs);
            let instance = base_facts().instances.remove(0);
            let context = TimelineQueryContext {
                instance_id: instance.instance_id,
                server_id: instance.server_id,
                game_id: instance.game_id,
            };
            let event_ids = days
                .iter()
                .map(|day| format!("neutral.day-{day}"))
                .collect::<Vec<_>>();
            for day in 1..=7_u8 {
                let start = monday + u64::from(day - 1) * DAY;
                for (now, expected) in [
                    (
                        start - 60_000,
                        days.contains(&(if day == 1 { 7 } else { day - 1 })),
                    ),
                    (start, days.contains(&day)),
                    (start + 20 * 3_600_000, days.contains(&day)),
                    (start + DAY - 1, days.contains(&day)),
                ] {
                    let result = evaluate(
                        &compiled,
                        &base_facts(),
                        &base_resources(),
                        EvaluationTime {
                            unix_ms: now,
                            monotonic_ms: now,
                        },
                        7,
                    )
                    .expect("weekly evaluation");
                    assert_eq!(
                        !result.dispatch_intents.is_empty(),
                        expected,
                        "days={days:?}, now={now}"
                    );
                    assert!(result.next_wake_unix_ms.is_some_and(|wake| wake > now));
                    let inspection = inspect_timeline(
                        &compiled,
                        EvaluationTime {
                            unix_ms: now,
                            monotonic_ms: now,
                        },
                        &context,
                        &event_ids,
                    )
                    .expect("pure weekly inspection");
                    assert_eq!(inspection.catalog_hash, compiled.catalog_hash());
                    assert_eq!(inspection.next_wake_unix_ms, result.next_wake_unix_ms);
                    assert_eq!(
                        inspection
                            .events
                            .iter()
                            .any(|event| event.availability.state == EligibilityState::True),
                        expected
                    );
                    for event in &inspection.events {
                        assert!(event.scope_applies);
                        assert_eq!(
                            event.availability.state == EligibilityState::True,
                            event
                                .availability
                                .active_interval
                                .is_some_and(|(start, end)| start <= now && now < end)
                        );
                    }
                }
            }
        }
        // Same specification: scope, zero-duration and clipped validity share the production owner.
        let mut docs = example_documents();
        for doc in [&mut docs.0, &mut docs.1, &mut docs.2, &mut docs.3] {
            doc["schema_version"] = serde_json::json!(crate::SCHEDULING_SCHEMA_VERSION_V2);
        }
        docs.0["tasks"][0]["trigger"] =
            serde_json::json!({"kind":"timeline_active","event_id":"neutral.window"});
        docs.0["tasks"][0]["feedback_stop"] = false_fact();
        docs.3["events"] = serde_json::json!([{
            "id":"neutral.window", "scope":{"kind":"server","server_id":"fixture-server-a"},
            "event_kind":"activity", "schedule":{"kind":"at","at_ms":NOW,
                "clock_source":{"kind":"server","timezone_id":"fixed/utc","utc_offset_minutes":0,"dst_offset_minutes":0,"maintenance_drift_ms":0}},
            "duration_ms":1000,"invalidates_fact_prefixes":[],
            "validity":{"from_unix_ms":NOW+100,"until_unix_ms":NOW+900}
        }]);
        let instance = base_facts().instances.remove(0);
        let context = TimelineQueryContext {
            instance_id: instance.instance_id,
            server_id: instance.server_id,
            game_id: instance.game_id,
        };
        for (scope, duration, unavailable) in [
            (
                serde_json::json!({"kind":"server","server_id":"fixture-server-a"}),
                1000,
                false,
            ),
            (
                serde_json::json!({"kind":"instance","instance_id":"fixture-instance-a"}),
                1000,
                false,
            ),
            (
                serde_json::json!({"kind":"game","game_id":context.game_id}),
                1000,
                false,
            ),
            (
                serde_json::json!({"kind":"server","server_id":"other-server"}),
                1000,
                true,
            ),
            (
                serde_json::json!({"kind":"server","server_id":"fixture-server-a"}),
                0,
                true,
            ),
        ] {
            docs.3["events"][0]["scope"] = scope;
            docs.3["events"][0]["duration_ms"] = serde_json::json!(duration);
            let compiled = compile_documents(docs.clone());
            for now in [NOW, NOW + 100, NOW + 899, NOW + 900] {
                let time = EvaluationTime {
                    unix_ms: now,
                    monotonic_ms: now,
                };
                let inspected =
                    inspect_timeline(&compiled, time, &context, &["neutral.window".into()])
                        .unwrap();
                let event = &inspected.events[0];
                let active = (NOW + 100..NOW + 900).contains(&now);
                assert_eq!(
                    event.availability.state,
                    if unavailable {
                        EligibilityState::Unknown
                    } else if active {
                        EligibilityState::True
                    } else {
                        EligibilityState::False
                    }
                );
                let produced =
                    evaluate(&compiled, &base_facts(), &base_resources(), time, 7).unwrap();
                assert_eq!(
                    !produced.dispatch_intents.is_empty(),
                    !unavailable && active
                );
                if !unavailable {
                    assert_eq!(
                        event.availability.active_interval,
                        active.then_some((NOW + 100, NOW + 900))
                    );
                    assert_eq!(
                        event.availability.next_wake_unix_ms,
                        if now < NOW + 100 {
                            Some(NOW + 100)
                        } else if active {
                            Some(NOW + 900)
                        } else {
                            None
                        }
                    );
                }
            }
            assert_eq!(
                inspect_timeline(
                    &compiled,
                    EvaluationTime {
                        unix_ms: NOW,
                        monotonic_ms: 0
                    },
                    &context,
                    &["unknown".into()]
                )
                .unwrap_err()
                .code()
                .as_str(),
                "policy_evaluation_input_invalid"
            );
        }
    }

    // Specification criteria: https://github.com/HS7097/ActingCommand-Workflow/issues/269#issuecomment-5551152553
    #[test]
    fn timeline_v2_at_window_clips_validity_and_wakes_at_end() {
        let mut docs = example_documents();
        for doc in [&mut docs.0, &mut docs.1, &mut docs.2, &mut docs.3] {
            doc["schema_version"] = serde_json::json!(crate::SCHEDULING_SCHEMA_VERSION_V2);
        }
        docs.0["tasks"][0]["trigger"] =
            serde_json::json!({"kind":"timeline_active","event_id":"neutral.window"});
        docs.0["tasks"][0]["feedback_stop"] = false_fact();
        docs.3["events"] = serde_json::json!([{
            "id":"neutral.window","scope":{"kind":"server","server_id":"fixture-server-a"},"event_kind":"activity",
            "schedule":{"kind":"at","at_ms":NOW+100,"clock_source":{"kind":"reveal","reveal_source":"evidence:neutral",
                "timezone_id":"fixed/utc","utc_offset_minutes":0,"dst_offset_minutes":0,"maintenance_drift_ms":0}},
            "duration_ms":1000,"invalidates_fact_prefixes":[],"validity":{"from_unix_ms":NOW+200,"until_unix_ms":NOW+900}
        }]);
        for (from, until, open, close) in [
            (NOW + 200, Some(NOW + 900), NOW + 200, NOW + 900),
            (0, None, NOW + 100, NOW + 1100),
        ] {
            docs.3["events"][0]["validity"] =
                serde_json::json!({"from_unix_ms":from,"until_unix_ms":until});
            let compiled = compile_documents(docs.clone());
            for (now, active) in [
                (open - 1, false),
                (open, true),
                (close - 1, true),
                (close, false),
            ] {
                let result = evaluate(
                    &compiled,
                    &base_facts(),
                    &base_resources(),
                    EvaluationTime {
                        unix_ms: now,
                        monotonic_ms: now,
                    },
                    7,
                )
                .expect("bounded evaluation");
                assert_eq!(!result.dispatch_intents.is_empty(), active);
                assert_eq!(
                    result.next_wake_unix_ms,
                    if now < open {
                        Some(open)
                    } else if active {
                        Some(close)
                    } else {
                        None
                    }
                );
                if active {
                    assert_eq!(
                        result.dispatch_intents[0]
                            .prerequisites
                            .facts_fresh_until_unix_ms,
                        Some(close - 1)
                    );
                }
            }
        }
    }

    // Specification criteria: https://github.com/HS7097/ActingCommand-Workflow/issues/269#issuecomment-5551152553
    #[test]
    fn timeline_v2_scope_and_expired_instance_windows_do_not_dispatch() {
        let mut docs = example_documents();
        for doc in [&mut docs.0, &mut docs.1, &mut docs.2, &mut docs.3] {
            doc["schema_version"] = serde_json::json!(crate::SCHEDULING_SCHEMA_VERSION_V2);
        }
        docs.0["tasks"][0]["trigger"] =
            serde_json::json!({"kind":"timeline_active","event_id":"neutral.instance"});
        docs.0["tasks"][0]["feedback_stop"] = false_fact();
        docs.3["events"] = serde_json::json!([{"id":"neutral.instance","scope":{"kind":"instance","instance_id":"fixture-instance-a"},
            "event_kind":"activity","schedule":{"kind":"at","at_ms":NOW,"clock_source":{"kind":"server","timezone_id":"fixed/utc",
                "utc_offset_minutes":0,"dst_offset_minutes":0,"maintenance_drift_ms":0}},"duration_ms":1000,
            "invalidates_fact_prefixes":[],"validity":{"from_unix_ms":NOW,"until_unix_ms":NOW+1000}}]);
        for (scope, allowed) in [
            (
                serde_json::json!({"kind":"instance","instance_id":"fixture-instance-a"}),
                true,
            ),
            (
                serde_json::json!({"kind":"instance","instance_id":"fixture-instance-b"}),
                false,
            ),
            (
                serde_json::json!({"kind":"server","server_id":"fixture-server-a"}),
                true,
            ),
            (
                serde_json::json!({"kind":"server","server_id":"other-server"}),
                false,
            ),
        ] {
            docs.3["events"][0]["scope"] = scope;
            let compiled = compile_documents(docs.clone());
            for (now, active) in [(NOW, allowed), (NOW + 1000, false)] {
                let result = evaluate(
                    &compiled,
                    &base_facts(),
                    &base_resources(),
                    EvaluationTime {
                        unix_ms: now,
                        monotonic_ms: now,
                    },
                    7,
                )
                .expect("scope evaluation");
                assert_eq!(!result.dispatch_intents.is_empty(), active);
            }
        }
        let compiled = compile_documents(docs.clone());
        let mut facts = base_facts();
        facts.instances.clear();
        facts.facts.clear();
        assert!(
            evaluate(
                &compiled,
                &facts,
                &base_resources(),
                EvaluationTime {
                    unix_ms: NOW,
                    monotonic_ms: NOW
                },
                7
            )
            .expect("no instance")
            .dispatch_intents
            .is_empty()
        );
        docs.0["tasks"][0]["trigger"] = serde_json::json!({"kind":"not","predicate":{"kind":"timeline_active","event_id":"neutral.instance"}});
        let compiled = compile_documents(docs);
        assert!(
            evaluate(
                &compiled,
                &base_facts(),
                &base_resources(),
                EvaluationTime {
                    unix_ms: NOW,
                    monotonic_ms: NOW
                },
                7
            )
            .expect("unknown scope stays unknown")
            .dispatch_intents
            .is_empty()
        );
    }

    // Specification criteria: https://github.com/HS7097/ActingCommand-Workflow/issues/269#issuecomment-5551152553
    #[test]
    fn timeline_v2_zero_duration_and_overflow_fail_closed() {
        let mut docs = example_documents();
        for doc in [&mut docs.0, &mut docs.1, &mut docs.2, &mut docs.3] {
            doc["schema_version"] = serde_json::json!(crate::SCHEDULING_SCHEMA_VERSION_V2);
        }
        docs.0["tasks"][0]["trigger"] = serde_json::json!({"kind":"not","predicate":{"kind":"timeline_active","event_id":"neutral.zero"}});
        docs.0["tasks"][0]["feedback_stop"] = false_fact();
        docs.3["events"] = serde_json::json!([{"id":"neutral.zero","scope":{"kind":"server","server_id":"fixture-server-a"},"event_kind":"activity",
            "schedule":{"kind":"interval","every_ms":1,"anchor_ms":0,"clock_source":{"kind":"local"}},"duration_ms":0,
            "invalidates_fact_prefixes":[],"validity":{"from_unix_ms":0,"until_unix_ms":null}}]);
        let compiled = compile_documents(docs.clone());
        assert!(
            evaluate(
                &compiled,
                &base_facts(),
                &base_resources(),
                EvaluationTime {
                    unix_ms: NOW,
                    monotonic_ms: NOW
                },
                7
            )
            .expect("zero stays unknown")
            .dispatch_intents
            .is_empty()
        );
        docs.3["events"][0]["duration_ms"] = serde_json::json!(100);
        let compiled = compile_documents(docs);
        for now in [u64::MAX - 50, u64::MAX] {
            let error = evaluate(
                &compiled,
                &base_facts(),
                &base_resources(),
                EvaluationTime {
                    unix_ms: now,
                    monotonic_ms: now,
                },
                7,
            )
            .expect_err("overflow must fail visibly");
            assert_eq!(error.code().as_str(), "policy_evaluation_numeric_overflow");
        }
        let event = &compiled.catalog().timeline.events[0];
        let result = timeline_window(
            event,
            EvaluationTime {
                unix_ms: 0,
                monotonic_ms: 0,
            },
        )
        .expect("Unix epoch");
        assert_eq!(result.active, Some((0, 100)));

        // Specification criterion: Workflow #267 POLICY-TIME-VALIDITY-v1.
        let mut docs = example_documents();
        for doc in [&mut docs.0, &mut docs.1, &mut docs.2, &mut docs.3] {
            doc["schema_version"] = serde_json::json!(crate::SCHEDULING_SCHEMA_VERSION_V2);
        }
        let mut predicate = false_fact();
        predicate["value"]["value"] = serde_json::json!(false);
        docs.0["tasks"][0]["trigger"] = serde_json::json!({"kind":"all","predicates":[predicate, {
            "kind":"outcome","task_id":"fixture.observe","outcome_key":"completed",
            "comparison":"eq","value":{"type":"boolean","value":false}
        }]});
        docs.0["tasks"][0]["feedback_stop"] = false_fact();
        docs.3["events"] = serde_json::json!([{
            "id":"reset","scope":{"kind":"server","server_id":"fixture-server-a"},
            "event_kind":"reset","schedule":{"kind":"interval","every_ms":100,"anchor_ms":NOW+10,
                "clock_source":{"kind":"server","timezone_id":"fixed/utc","utc_offset_minutes":0,
                    "dst_offset_minutes":0,"maintenance_drift_ms":0}},
            "duration_ms":0,"invalidates_fact_prefixes":["fixture.","outcome.fixture.observe."],
            "validity":{"from_unix_ms":NOW+10,"until_unix_ms":NOW+50}
        }]);
        let mut facts = base_facts();
        facts.outcomes.push(ObservedOutcome {
            task_id: "fixture.observe".to_owned(),
            instance_id: "fixture-instance-a".to_owned(),
            outcome_key: "completed".to_owned(),
            value: FactValue::Boolean(false),
            observed_at_unix_ms: NOW,
            expires_at_unix_ms: None,
            activity_window_id: None,
        });
        for duration in [0, 5] {
            docs.3["events"][0]["duration_ms"] = serde_json::json!(duration);
            let compiled = compile_documents(docs.clone());
            for (now, expected) in [
                (NOW, EligibilityState::True),
                (NOW + 10, EligibilityState::Unknown),
                (NOW + 16, EligibilityState::Unknown),
                (NOW + 200, EligibilityState::Unknown),
            ] {
                let time = EvaluationTime {
                    unix_ms: now,
                    monotonic_ms: now,
                };
                let evaluation = evaluate(&compiled, &facts, &base_resources(), time, 7).unwrap();
                assert_eq!(evaluation.decisions[0].eligibility, expected);
                if now == NOW {
                    assert_eq!(evaluation.next_wake_unix_ms, Some(NOW + 10));
                    assert_eq!(
                        evaluation.dispatch_intents[0]
                            .prerequisites
                            .facts_fresh_until_unix_ms,
                        Some(NOW + 9)
                    );
                } else {
                    let projected = project_time_validity(&compiled, &facts, time).unwrap();
                    assert!(projected.facts.is_empty());
                    assert!(projected.outcomes.is_empty());
                }
            }
            let mut refreshed = facts.clone();
            refreshed.facts[0].observed_at_unix_ms = NOW + 11;
            refreshed.outcomes[0].observed_at_unix_ms = NOW + 11;
            assert_eq!(
                evaluate(
                    &compiled,
                    &refreshed,
                    &base_resources(),
                    EvaluationTime {
                        unix_ms: NOW + 200,
                        monotonic_ms: NOW + 200,
                    },
                    7
                )
                .unwrap()
                .dispatch_intents
                .len(),
                1
            );
        }
        for scope in [
            serde_json::json!({"kind":"server","server_id":"other-server"}),
            serde_json::json!({"kind":"instance","instance_id":"fixture-instance-b"}),
        ] {
            docs.3["events"][0]["scope"] = scope;
            let compiled = compile_documents(docs.clone());
            assert_eq!(
                evaluate(
                    &compiled,
                    &facts,
                    &base_resources(),
                    EvaluationTime {
                        unix_ms: NOW + 200,
                        monotonic_ms: NOW + 200,
                    },
                    7
                )
                .unwrap()
                .dispatch_intents
                .len(),
                1
            );
        }
    }

    // Specification criteria: https://github.com/HS7097/ActingCommand-Workflow/issues/269#issuecomment-5551152553
    #[test]
    fn timeline_v2_generation_and_reason_chain_bind_actual_consumption() {
        let mut docs = example_documents();
        for doc in [&mut docs.0, &mut docs.1, &mut docs.2, &mut docs.3] {
            doc["schema_version"] = serde_json::json!(crate::SCHEDULING_SCHEMA_VERSION_V2);
        }
        docs.0["tasks"][0]["trigger"] = serde_json::json!({"kind":"all","predicates":[due_clock(),
            {"kind":"any","predicates":[{"kind":"timeline_active","event_id":"neutral.window"}, false_fact()]}]});
        docs.0["tasks"][0]["feedback_stop"] = false_fact();
        docs.3["events"] = serde_json::json!([{"id":"neutral.window","scope":{"kind":"server","server_id":"fixture-server-a"},"event_kind":"activity",
            "schedule":{"kind":"at","at_ms":NOW,"clock_source":{"kind":"server","timezone_id":"fixed/utc",
                "utc_offset_minutes":0,"dst_offset_minutes":0,"maintenance_drift_ms":0}},"duration_ms":1000,
            "invalidates_fact_prefixes":[],"validity":{"from_unix_ms":0,"until_unix_ms":NOW+500}}]);
        let first = compile_documents(docs.clone());
        let result = evaluate(
            &first,
            &base_facts(),
            &base_resources(),
            EvaluationTime {
                unix_ms: NOW,
                monotonic_ms: NOW,
            },
            7,
        )
        .expect("same evaluator");
        assert_eq!(result.dispatch_intents.len(), 1);
        assert_eq!(
            result.dispatch_intents[0].catalog_hash,
            first.catalog_hash()
        );
        assert_eq!(
            result.dispatch_intents[0]
                .prerequisites
                .facts_fresh_until_unix_ms,
            Some(NOW + 499)
        );
        assert!(
            result.reason_chains[0]
                .reasons
                .iter()
                .any(|reason| reason.outcome.code().as_str() == "timeline_active"
                    && reason.detail.contains("neutral.window"))
        );
        docs.3["events"][0]["validity"]["from_unix_ms"] = serde_json::json!(NOW + 1);
        let second = compile_documents(docs.clone());
        assert_ne!(first.catalog_hash(), second.catalog_hash());
        assert!(
            evaluate(
                &second,
                &base_facts(),
                &base_resources(),
                EvaluationTime {
                    unix_ms: NOW,
                    monotonic_ms: NOW
                },
                7
            )
            .expect("changed validity")
            .dispatch_intents
            .is_empty()
        );
        docs.0["tasks"][0]["trigger"] = serde_json::json!({"kind":"any","predicates":[due_clock(),{"kind":"timeline_active","event_id":"neutral.window"}]});
        let compiled = compile_documents(docs);
        let result = evaluate(
            &compiled,
            &base_facts(),
            &base_resources(),
            EvaluationTime {
                unix_ms: NOW,
                monotonic_ms: NOW,
            },
            7,
        )
        .expect("short circuit still wakes");
        assert_eq!(result.next_wake_unix_ms, Some(NOW + 1));
        assert_eq!(
            result.dispatch_intents[0]
                .prerequisites
                .facts_fresh_until_unix_ms,
            Some(NOW)
        );
    }

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
            EvaluationTime {
                unix_ms: NOW,
                monotonic_ms: NOW,
            },
            7,
        )
        .expect("first evaluation");
        let second = evaluate(
            &catalog,
            &facts,
            &resources,
            EvaluationTime {
                unix_ms: NOW,
                monotonic_ms: NOW,
            },
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
            EvaluationTime {
                unix_ms: NOW,
                monotonic_ms: NOW,
            },
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
    fn record_list_fact_and_ttl_share_the_typed_predicate_surface() {
        let expected = BTreeMap::from([
            (
                "label".to_owned(),
                crate::FactScalar::String("alpha".to_owned()),
            ),
            ("count".to_owned(), crate::FactScalar::Integer(2)),
        ]);
        let catalog = catalog(|tasks| {
            tasks["tasks"][0]["trigger"] = serde_json::json!({
                "kind": "fact",
                "scope": {"kind": "server", "server_id": "fixture-server-a"},
                "fact_key": "inventory.items",
                "comparison": "contains",
                "value": {
                    "type": "record_list",
                    "value": [{"label": {"type": "string", "value": "alpha"}, "count": {"type": "integer", "value": 2}}]
                },
                "max_age_ms": null
            });
            tasks["tasks"][0]["feedback_stop"] = false_fact();
        });
        let mut facts = base_facts();
        facts.facts.push(ObservedFact {
            scope: ScopeSelector::Server {
                server_id: "fixture-server-a".to_owned(),
            },
            fact_key: "inventory.items".to_owned(),
            value: FactValue::RecordList(vec![expected]),
            observed_at_unix_ms: NOW,
            expires_at_unix_ms: Some(NOW + 5),
            confidence_milli: 900,
        });

        let fresh = evaluate(
            &catalog,
            &facts,
            &base_resources(),
            EvaluationTime {
                unix_ms: NOW,
                monotonic_ms: NOW,
            },
            1,
        )
        .expect("fresh record-list fact");
        assert_eq!(fresh.decisions[0].eligibility, EligibilityState::True);
        assert_eq!(fresh.next_wake_unix_ms, Some(NOW + 6));

        let expired = evaluate(
            &catalog,
            &facts,
            &base_resources(),
            EvaluationTime {
                unix_ms: NOW + 6,
                monotonic_ms: NOW + 6,
            },
            1,
        )
        .expect("expired record-list fact");
        assert_eq!(expired.decisions[0].eligibility, EligibilityState::Unknown);
        assert_eq!(
            expired.decisions[0].detection_suggestions[0].reason,
            "fact_expired"
        );
    }

    #[test]
    fn record_deadline_uses_each_item_expiry_and_preserves_unknown_and_blocked_states() {
        const HOUR_MS: u64 = 3_600_000;
        let catalog = catalog(|tasks| {
            tasks["tasks"][0]["trigger"] = serde_json::json!({
                "kind": "record_deadline",
                "scope": {"kind": "instance", "instance_id": "fixture-instance-a"},
                "fact_key": "inventory.expiring_items",
                "timestamp_field": "expires_at_unix_ms",
                "within_ms": 48 * HOUR_MS,
                "max_age_ms": 1000
            });
            tasks["tasks"][0]["feedback_stop"] = false_fact();
        });
        let evaluate_case = |deadlines: &[u64], observed_at_unix_ms: u64, available: bool| {
            let mut facts = base_facts();
            facts.instances[0].available = available;
            facts.facts.push(ObservedFact {
                scope: ScopeSelector::Instance {
                    instance_id: "fixture-instance-a".to_owned(),
                },
                fact_key: "inventory.expiring_items".to_owned(),
                value: FactValue::RecordList(
                    deadlines
                        .iter()
                        .map(|deadline| {
                            BTreeMap::from([(
                                "expires_at_unix_ms".to_owned(),
                                FactScalar::TimestampMs(*deadline),
                            )])
                        })
                        .collect(),
                ),
                observed_at_unix_ms,
                expires_at_unix_ms: None,
                confidence_milli: 1_000,
            });
            evaluate(
                &catalog,
                &facts,
                &base_resources(),
                EvaluationTime {
                    unix_ms: NOW,
                    monotonic_ms: NOW,
                },
                1,
            )
            .expect("record deadline evaluation")
        };

        let within = evaluate_case(
            &[
                NOW + 47 * HOUR_MS,
                NOW + 47 * HOUR_MS + 1,
                NOW + 47 * HOUR_MS + 2,
            ],
            NOW,
            true,
        );
        assert_eq!(within.decisions[0].eligibility, EligibilityState::True);
        assert_eq!(within.dispatch_intents.len(), 1);

        let outside = evaluate_case(&[NOW + 49 * HOUR_MS], NOW, true);
        assert_eq!(outside.decisions[0].eligibility, EligibilityState::False);
        assert!(outside.dispatch_intents.is_empty());
        assert_eq!(outside.next_wake_unix_ms, Some(NOW + 1_001));

        let empty = evaluate_case(&[], NOW, true);
        assert_eq!(empty.decisions[0].eligibility, EligibilityState::False);
        assert!(empty.dispatch_intents.is_empty());

        let stale = evaluate_case(&[NOW + 47 * HOUR_MS], NOW - 1_001, true);
        assert_eq!(stale.decisions[0].eligibility, EligibilityState::Unknown);
        assert_eq!(
            stale.decisions[0].detection_suggestions[0].reason,
            "fact_stale"
        );

        assert!(stale.dispatch_intents.is_empty());

        let unavailable = evaluate_case(&[NOW + 47 * HOUR_MS], NOW, false);
        assert_eq!(unavailable.decisions[0].eligibility, EligibilityState::True);
        assert_eq!(
            unavailable.decisions[0].state,
            SchedulingDecisionState::Blocked
        );
        assert!(unavailable.dispatch_intents.is_empty());
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
            monotonic_ms: 200_000_000,
        };
        let mut facts = base_facts();
        facts.tasks = vec![
            TaskRuntimeSnapshot {
                task_id: "fixture.observe".to_owned(),
                instance_id: "fixture-instance-a".to_owned(),
                last_dispatched_unix_ms: None,
                eligible_since_unix_ms: Some(late_time.unix_ms - 1),
                terminal_state: None,
                completed_window: None,
                last_duration_ms: None,
                failure_streak: None,
            },
            TaskRuntimeSnapshot {
                task_id: "fixture.observe-secondary".to_owned(),
                instance_id: "fixture-instance-a".to_owned(),
                last_dispatched_unix_ms: None,
                eligible_since_unix_ms: Some(0),
                terminal_state: None,
                completed_window: None,
                last_duration_ms: None,
                failure_streak: None,
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
    // Also Workflow #308 RT-S1b/S1c (Alice 2026-09-29 answer 3): dynamic target scoring, release, instance isolation.
    fn resource_target_scoring_release_override_and_instance_isolation() {
        // A resource target on instance A names the priority-0 task: the gap outranks the
        // 100-level priority difference, a closed gap releases it, an override supersedes a
        // manual offset, and instance B decides as if A had no policy.
        let catalog = two_task_catalog(|tasks| {
            for task in tasks.iter_mut() {
                task["scope"] =
                    serde_json::json!({"kind": "server", "server_id": "fixture-server-a"});
                task["trigger"] = due_clock();
                task["feedback_stop"] = false_fact();
            }
            tasks[0]["priority"] = serde_json::json!(100);
            tasks[1]["priority"] = serde_json::json!(0);
        });
        let time = EvaluationTime {
            unix_ms: NOW,
            monotonic_ms: NOW,
        };
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
        let instance_a = || ScopeSelector::Instance {
            instance_id: "fixture-instance-a".to_owned(),
        };
        // The stored policy exactly as the formal entry checks and encodes it.
        let policy_fact = |mode: &str| {
            let document = format!(
                r#"{{"schema_version":"actingcommand.resource-targets.v1","instance":"fixture-instance-a","valid_until_unix_ms":{},"targets":[{{"id":"primary-floor","resource":"fixture-pool-a","condition":{{"kind":"at_least","amount":100}},"scale":100,"importance_milli":200000,"rule":"shortfall_linear","apply":{{"mode":"{mode}","weight":"score_stage"}},"tasks":["fixture.observe-secondary"]}}]}}"#,
                NOW + 3_600_000
            );
            let parsed = crate::parse_resource_targets(document.as_bytes()).expect("parse");
            let checked =
                crate::check_resource_targets(&parsed, &catalog, &facts, time).expect("check");
            let rows = checked
                .rows
                .into_iter()
                .map(|row| {
                    row.into_iter()
                        .map(|(field, value)| {
                            let value = match value {
                                actingcommand_contract::FactScalar::Boolean(value) => {
                                    FactScalar::Boolean(value)
                                }
                                actingcommand_contract::FactScalar::Integer(value) => {
                                    FactScalar::Integer(value)
                                }
                                actingcommand_contract::FactScalar::String(value) => {
                                    FactScalar::String(value)
                                }
                                actingcommand_contract::FactScalar::TimestampMs(value) => {
                                    FactScalar::TimestampMs(value)
                                }
                                actingcommand_contract::FactScalar::DurationMs(value) => {
                                    FactScalar::DurationMs(value)
                                }
                            };
                            (field, value)
                        })
                        .collect()
                })
                .collect();
            ObservedFact {
                scope: instance_a(),
                fact_key: actingcommand_contract::RESOURCE_TARGETS_FACT_KEY.to_owned(),
                value: FactValue::RecordList(rows),
                observed_at_unix_ms: NOW,
                expires_at_unix_ms: parsed.document().valid_until_unix_ms,
                confidence_milli: 1_000,
            }
        };
        let (adjust, overriding) = (policy_fact("adjust"), policy_fact("override"));
        // Every pair compared below shares ledger position, snapshot id, instant and seed.
        let evaluate_at = |current: i64, policy: Option<&ObservedFact>, offset: Option<i32>| {
            let mut facts = facts.clone();
            facts.facts.push(ObservedFact {
                scope: instance_a(),
                fact_key: "resource.primary".to_owned(),
                value: FactValue::Integer(current),
                observed_at_unix_ms: NOW,
                expires_at_unix_ms: None,
                confidence_milli: 1_000,
            });
            facts.facts.extend(policy.cloned());
            facts
                .priority_offsets
                .extend(offset.map(|offset_milli| PriorityOffset {
                    task_id: "fixture.observe-secondary".to_owned(),
                    instance_id: Some("fixture-instance-a".to_owned()),
                    offset_milli,
                    origin: PriorityOffsetOrigin::Agent,
                    observed_at_unix_ms: NOW,
                }));
            evaluate(&catalog, &facts, &base_resources(), time, 5).expect("target evaluation")
        };
        let winner_on_a = |evaluation: &PolicyEvaluation| {
            evaluation
                .dispatch_intents
                .iter()
                .find(|intent| intent.instance_id == "fixture-instance-a")
                .expect("instance A dispatch")
                .task_id
                .clone()
        };
        let secondary = |evaluation: &PolicyEvaluation| {
            decision_for(
                evaluation,
                "fixture.observe-secondary",
                "fixture-instance-a",
            )
            .clone()
        };
        // A resource target reason names its target by its `resource_target_id` value.
        let reason_detail = |evaluation: &PolicyEvaluation, code: &str, target_id: &str| {
            secondary(evaluation)
                .reasons
                .into_iter()
                .find(|reason| {
                    reason.outcome.code().as_str() == code
                        && reason
                            .outcome
                            .value("resource_target_id")
                            .and_then(serde_json::Value::as_str)
                            == Some(target_id)
                })
                .map(|reason| reason.detail)
        };
        // The decision record names the stored policy; every other record reason must match.
        let without_target_reasons = |evaluation: &PolicyEvaluation| {
            let mut evaluation = evaluation.clone();
            for reasons in evaluation
                .decisions
                .iter_mut()
                .map(|decision| &mut decision.reasons)
                .chain(
                    evaluation
                        .reason_chains
                        .iter_mut()
                        .map(|chain| &mut chain.reasons),
                )
            {
                reasons.retain(|reason| {
                    let code = reason.outcome.code().as_str();
                    !code.starts_with("resource_target_") && code != "decision_record"
                });
            }
            serde_json::to_vec(&evaluation).expect("evaluation bytes")
        };
        let instance_b = |evaluation: &PolicyEvaluation| {
            let intents = evaluation
                .dispatch_intents
                .iter()
                .filter(|intent| intent.instance_id == "fixture-instance-b")
                .collect::<Vec<_>>();
            let chains = evaluation
                .reason_chains
                .iter()
                .filter(|chain| {
                    intents
                        .iter()
                        .any(|intent| intent.decision_id == chain.decision_id)
                })
                .collect::<Vec<_>>();
            let decisions = evaluation
                .decisions
                .iter()
                .filter(|decision| decision.instance_id.as_deref() == Some("fixture-instance-b"))
                .collect::<Vec<_>>();
            serde_json::to_vec(&(decisions, intents, chains)).expect("instance B bytes")
        };

        // Dynamic scoring: s = min(g * 200000 / 100, 1e6) against a 100000 priority lead.
        let mut previous = i64::MAX;
        for (current, effective, winner) in [
            (0, 200_000, "fixture.observe-secondary"),
            (40, 120_000, "fixture.observe-secondary"),
            (60, 80_000, "fixture.observe"),
            (99, 2_000, "fixture.observe"),
        ] {
            let scored = evaluate_at(current, Some(&adjust), None);
            let rank = secondary(&scored).rank.expect("secondary rank");
            assert_eq!(rank.effective_milli, effective, "current={current}");
            assert!(rank.effective_milli <= previous);
            previous = rank.effective_milli;
            assert_eq!(winner_on_a(&scored), winner, "current={current}");
            assert!(reason_detail(&scored, "resource_target_applied", "primary-floor").is_some());
            assert_eq!(
                instance_b(&scored),
                instance_b(&evaluate_at(current, None, None))
            );
        }

        // RT-S1c: each selected chain ends with its dispatch decision record.
        let scored = evaluate_at(0, Some(&adjust), None);
        let selected_chain = |instance: &str| {
            let intent = scored
                .dispatch_intents
                .iter()
                .find(|intent| intent.instance_id == instance)
                .expect("dispatch");
            scored
                .reason_chains
                .iter()
                .find(|chain| chain.id == intent.reason_chain_id)
                .expect("chain")
                .reasons
                .clone()
        };
        let chain = selected_chain("fixture-instance-a");
        let tail = &chain[chain.len() - 5..];
        assert_eq!(
            tail.iter()
                .map(|reason| reason.outcome.code().as_str())
                .collect::<Vec<_>>(),
            [
                "placement_selected",
                "rank_assigned",
                "rank_breakdown",
                "decision_record",
                "decision_candidate_not_selected"
            ]
        );
        assert_eq!(
            tail[4].outcome.value("task_id"),
            Some(&serde_json::json!("fixture.observe"))
        );
        assert!(tail[2].detail.contains(" target=200000 "));
        assert!(tail[3].detail.starts_with("targets=active:"));
        assert!(tail[4].detail.starts_with("why=instance_already_selected "));
        assert_eq!(secondary(&scored).reasons, chain);
        assert!(selected_chain("fixture-instance-b").iter().any(|reason| {
            reason.outcome.code().as_str() == "decision_record"
                && reason.detail.starts_with("targets=none ")
        }));

        // Release: a closed gap decides exactly as no policy, apart from its own reasons.
        let released = evaluate_at(100, Some(&adjust), None);
        assert_eq!(
            without_target_reasons(&released),
            without_target_reasons(&evaluate_at(100, None, None))
        );
        assert!(reason_detail(&released, "resource_target_satisfied", "primary-floor").is_some());
        assert_eq!(
            instance_b(&released),
            instance_b(&evaluate_at(100, None, None))
        );

        // Override: it supersedes the manual offset while the gap is open, and releases it.
        let adjusted = evaluate_at(0, Some(&adjust), Some(-300_000));
        assert_eq!(winner_on_a(&adjusted), "fixture.observe");
        assert_eq!(
            secondary(&adjusted).rank.expect("rank").effective_milli,
            -100_000
        );
        let overridden = evaluate_at(0, Some(&overriding), Some(-300_000));
        assert_eq!(winner_on_a(&overridden), "fixture.observe-secondary");
        assert_eq!(
            secondary(&overridden).rank.expect("rank").effective_milli,
            200_000
        );
        assert!(reason_detail(&overridden, "resource_target_override", "primary-floor").is_some());
        let released = evaluate_at(100, Some(&overriding), Some(-300_000));
        assert_eq!(
            without_target_reasons(&released),
            without_target_reasons(&evaluate_at(100, None, Some(-300_000)))
        );
        assert!(
            reason_detail(&released, "resource_target_satisfied", "primary-floor")
                .is_some_and(|detail| detail.ends_with("; override released"))
        );
        for (evaluation, current) in [(adjusted, 0), (overridden, 0), (released, 100)] {
            assert_eq!(
                instance_b(&evaluation),
                instance_b(&evaluate_at(current, None, Some(-300_000)))
            );
        }
    }

    #[test]
    fn resource_target_valuation_maps_milli_yields_before_rounding() {
        let time = EvaluationTime {
            unix_ms: NOW,
            monotonic_ms: NOW,
        };
        let instance_a = || ScopeSelector::Instance {
            instance_id: "fixture-instance-a".to_owned(),
        };
        // WF345 R4 specification: milli-yields map before rounding, sum within and
        // across resources, and read the same fresh inventory facts for gap weights.
        for (amounts, other, weight, base_score, gap_score) in [
            (vec![400_u64], 0_u64, 1_000_u64, 400_u64, 800_u64),
            (vec![400, 400], 400, 1, 1, 801),
            (vec![0], 0, 1_000, 0, 0),
        ] {
            let mut docs = example_documents();
            for document in [&mut docs.0, &mut docs.1, &mut docs.2, &mut docs.3] {
                document["schema_version"] = serde_json::json!(crate::SCHEDULING_SCHEMA_VERSION_V2);
            }
            for event in docs.3["events"].as_array_mut().unwrap() {
                event["validity"] = serde_json::json!({"from_unix_ms":0,"until_unix_ms":null});
            }
            let mut second_pool = docs.1["pools"][0].clone();
            second_pool["id"] = serde_json::json!("fixture-pool-b");
            docs.1["pools"].as_array_mut().unwrap().push(second_pool);
            for pool in docs.1["pools"].as_array_mut().unwrap() {
                pool["valuation"] = serde_json::json!({"name":"Material","unit":"piece","scale":1,"base_weight_milli":weight});
            }
            docs.0["tasks"][0]["trigger"] = due_clock();
            docs.0["tasks"][0]["feedback_stop"] = false_fact();
            docs.0["tasks"][0]["priority"] = serde_json::json!(0);
            docs.0["tasks"][0]["produces"] = serde_json::json!(amounts.iter().map(|amount|
                serde_json::json!({"pool_id":"fixture-pool-a","direction":"produce","expected_amount_milli":amount,
                    "observation_source":"self_reported","confidence_milli":400})).chain(std::iter::once(
                serde_json::json!({"pool_id":"fixture-pool-b","direction":"produce","expected_amount_milli":other,
                    "observation_source":"self_reported","confidence_milli":1000}))).collect::<Vec<_>>());
            let expected = compile_documents(docs);
            let document = serde_json::json!({
                "schema_version": crate::RESOURCE_TARGETS_SCHEMA_VERSION_V2,
                "instance":"fixture-instance-a", "valid_until_unix_ms":NOW+3_600_000,
                "targets":[{"id":"material-floor","resource":"fixture-pool-a",
                    "condition":{"kind":"at_least","amount":100},"scale":1,"importance_milli":1000,
                    "apply":{"mode":"adjust","weight":"score_stage"}}]
            });
            let parsed =
                crate::parse_resource_targets(&serde_json::to_vec(&document).unwrap()).unwrap();
            let mut inputs = base_facts();
            let checked = crate::check_resource_targets(&parsed, &expected, &inputs, time);
            if amounts.iter().sum::<u64>() == 0 {
                assert!(
                    matches!(checked, Err(crate::ResourceTargetsError::Rejected(ref error))
                    if error.reason == actingcommand_contract::ResourceTargetsRejectionReason::UnmappedTask)
                );
                continue;
            }
            let checked = checked.expect("positive fractional production maps without inventory");
            inputs.facts.push(ObservedFact {
                scope: instance_a(),
                fact_key: actingcommand_contract::RESOURCE_TARGETS_FACT_KEY.to_owned(),
                value: FactValue::RecordList(
                    serde_json::from_value(serde_json::to_value(&checked.rows).unwrap()).unwrap(),
                ),
                observed_at_unix_ms: NOW,
                expires_at_unix_ms: Some(NOW + 3_600_000),
                confidence_milli: 1000,
            });
            let policies =
                crate::resource_targets::instance_target_policies(&inputs, time).unwrap();
            for (current, score) in [
                (None, base_score),
                (Some(100), base_score),
                (Some(99), gap_score),
            ] {
                let mut observed = inputs.clone();
                if let Some(current) = current {
                    observed.facts.push(ObservedFact {
                        scope: instance_a(),
                        fact_key: "resource.primary".to_owned(),
                        value: FactValue::Integer(current),
                        observed_at_unix_ms: NOW,
                        expires_at_unix_ms: Some(NOW + 60_000),
                        confidence_milli: 1000,
                    });
                }
                let resolved = crate::resource_targets::resolve_instance_targets(
                    &expected,
                    &observed,
                    &observed.instances[0],
                    &policies["fixture-instance-a"],
                    time,
                )
                .unwrap();
                assert_eq!(
                    resolved
                        .effect("fixture.observe")
                        .resources()
                        .unwrap()
                        .term_milli,
                    score
                );
                let result = evaluate(&expected, &observed, &base_resources(), time, 5).unwrap();
                let decision = decision_for(&result, "fixture.observe", "fixture-instance-a");
                assert!(
                    decision
                        .reasons
                        .iter()
                        .any(
                            |reason| reason.outcome.code().as_str() == "resource_weights"
                                && reason.detail.contains(&format!(" term={score} items="))
                        )
                );
            }
        }
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
            EvaluationTime {
                unix_ms: NOW,
                monotonic_ms: NOW,
            },
            4,
        )
        .expect("evaluation");

        assert_eq!(result.dispatch_intents.len(), 1);
        assert_eq!(result.dispatch_intents[0].instance_id, "fixture-instance-b");
        assert_eq!(
            result.dispatch_intents[0].prerequisites.activity_profile_id,
            "fixture-activity-game"
        );
        let instance_a = decision_for(&result, "fixture.observe", "fixture-instance-a");
        let instance_b = decision_for(&result, "fixture.observe", "fixture-instance-b");
        assert_eq!(instance_a.state, SchedulingDecisionState::Eligible);
        assert_eq!(instance_b.state, SchedulingDecisionState::Selected);
    }

    #[test]
    fn contention_penalizes_heavier_load_in_ranking() {
        let catalog = two_task_catalog(|tasks| {
            for task in tasks.iter_mut() {
                task["priority"] = serde_json::json!(0);
                task["trigger"] = due_clock();
                task["feedback_stop"] = false_fact();
            }
            tasks[0]["load_profile"] = serde_json::json!({"kind": "heavy"});
            tasks[1]["load_profile"] = serde_json::json!({"kind": "light"});
        });
        let mut resources = base_resources();
        resources.hosts[0].host_responsiveness_basis_points = 6_000;
        resources.hosts[0].third_party_pressure_basis_points = 5_000;

        let result = evaluate(
            &catalog,
            &base_facts(),
            &resources,
            EvaluationTime {
                unix_ms: NOW,
                monotonic_ms: NOW,
            },
            4,
        )
        .expect("evaluation");

        assert_eq!(result.dispatch_intents.len(), 1);
        assert_eq!(
            result.dispatch_intents[0].task_id,
            "fixture.observe-secondary"
        );
        let light = decision_for(&result, "fixture.observe-secondary", "fixture-instance-a")
            .rank
            .as_ref()
            .expect("light rank");
        let heavy = decision_for(&result, "fixture.observe", "fixture-instance-a")
            .rank
            .as_ref()
            .expect("heavy rank");
        assert!(heavy.contention_penalty > light.contention_penalty);
    }

    #[test]
    fn heavy_scene_concurrency_budget_blocks_another_heavy_dispatch() {
        let catalog = catalog(|tasks| {
            tasks["tasks"][0]["trigger"] = due_clock();
            tasks["tasks"][0]["feedback_stop"] = false_fact();
            tasks["tasks"][0]["load_profile"] = serde_json::json!({"kind": "heavy"});
        });
        let mut resources = base_resources();
        resources.hosts[0].heavy_dispatch_limit = 1;
        resources.hosts[0].active_heavy_dispatches = 1;

        let result = evaluate(
            &catalog,
            &base_facts(),
            &resources,
            EvaluationTime {
                unix_ms: NOW,
                monotonic_ms: NOW,
            },
            4,
        )
        .expect("evaluation");

        assert!(result.dispatch_intents.is_empty());
        assert!(
            result.decisions[0]
                .reasons
                .iter()
                .any(|reason| reason.outcome.code().as_str() == "heavy_scene_budget_exhausted")
        );
    }

    #[test]
    fn outcomes_and_dispatches_remain_instance_scoped() {
        // Specification criterion: Workflow #267 POLICY-TIME-VALIDITY-v1.
        let mut docs = example_documents();
        docs.0["tasks"][0]["scope"] =
            serde_json::json!({"kind": "server", "server_id": "fixture-server-a"});
        docs.0["tasks"][0]["trigger"] = due_clock();
        for profile in docs.2["profiles"].as_array_mut().expect("profiles") {
            profile["windows"][0]["start_minute_of_day"] = serde_json::json!(0);
            profile["windows"][0]["end_minute_of_day"] = serde_json::json!(0);
        }
        let catalog = compile_documents(docs.clone());
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
                expires_at_unix_ms: None,
                activity_window_id: Some("fixture-activity-a:0:0".to_owned()),
            },
            ObservedOutcome {
                task_id: "fixture.observe".to_owned(),
                instance_id: "fixture-instance-b".to_owned(),
                outcome_key: "completed".to_owned(),
                value: FactValue::Boolean(false),
                observed_at_unix_ms: NOW,
                expires_at_unix_ms: None,
                activity_window_id: Some("fixture-activity-game:0:0".to_owned()),
            },
        ];

        let result = evaluate(
            &catalog,
            &facts,
            &base_resources(),
            EvaluationTime {
                unix_ms: NOW,
                monotonic_ms: NOW,
            },
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
        for alias in [
            "Neutral.Instance".to_owned(),
            format!(" {} ", "é".repeat(127)),
            " ".to_owned(),
        ] {
            let mut aliased = facts.clone();
            aliased.instances[1].instance_id = alias.clone();
            aliased.outcomes[1].instance_id = alias.clone();
            aliased.tasks.push(TaskRuntimeSnapshot {
                task_id: "fixture.observe".to_owned(),
                instance_id: alias.clone(),
                last_dispatched_unix_ms: None,
                eligible_since_unix_ms: None,
                terminal_state: None,
                completed_window: None,
                last_duration_ms: None,
                failure_streak: None,
            });
            let result = evaluate(
                &catalog,
                &aliased,
                &base_resources(),
                EvaluationTime {
                    unix_ms: NOW,
                    monotonic_ms: NOW,
                },
                9,
            )
            .expect("registered alias evaluation");
            assert_eq!(result.dispatch_intents.len(), 1);
            assert_eq!(result.dispatch_intents[0].instance_id, alias);
            aliased.instances[1].instance_id.push('\n');
            assert!(
                evaluate(
                    &catalog,
                    &aliased,
                    &base_resources(),
                    EvaluationTime {
                        unix_ms: NOW,
                        monotonic_ms: NOW
                    },
                    9
                )
                .is_err()
            );
        }

        let time = EvaluationTime {
            unix_ms: NOW,
            monotonic_ms: NOW,
        };
        for window_id in [None, Some("fixture-activity-a:-1:0".to_owned())] {
            let mut historical = facts.clone();
            historical.outcomes[0].activity_window_id = window_id;
            let result = evaluate(&catalog, &historical, &base_resources(), time, 9).unwrap();
            assert_eq!(
                result.dispatch_intents.len(),
                2,
                "first dispatch follows trigger"
            );
        }
        let mut historical = serde_json::to_value(&facts).unwrap();
        for outcome in historical["outcomes"].as_array_mut().unwrap() {
            outcome
                .as_object_mut()
                .unwrap()
                .remove("activity_window_id");
            outcome
                .as_object_mut()
                .unwrap()
                .remove("expires_at_unix_ms");
        }
        let historical: EvaluationFacts = serde_json::from_value(historical).unwrap();
        assert!(
            historical
                .outcomes
                .iter()
                .all(|outcome| outcome.activity_window_id.is_none())
        );
        assert_eq!(
            evaluate(&catalog, &historical, &base_resources(), time, 9)
                .unwrap()
                .dispatch_intents
                .len(),
            2
        );

        // No mapped consumer: only a settled execution in this window enables stop.
        docs.0["tasks"][0]["feedback_stop"] = false_fact();
        let unmapped = compile_documents(docs);
        assert!(
            unmapped
                .referenced_outcome_keys("fixture.observe")
                .is_empty()
        );
        let mut unmapped_facts = facts.clone();
        unmapped_facts.outcomes.clear();
        unmapped_facts.facts[0].value = FactValue::Boolean(true);
        unmapped_facts.tasks.push(TaskRuntimeSnapshot {
            task_id: "fixture.observe".to_owned(),
            instance_id: "fixture-instance-a".to_owned(),
            last_dispatched_unix_ms: None,
            eligible_since_unix_ms: None,
            terminal_state: Some(TaskTerminalState::Succeeded),
            completed_window: None,
            last_duration_ms: None,
            failure_streak: None,
        });
        assert_eq!(
            evaluate(&unmapped, &unmapped_facts, &base_resources(), time, 9)
                .unwrap()
                .dispatch_intents
                .len(),
            2
        );
        unmapped_facts.tasks[0].completed_window = Some(CompletedActivityWindow {
            window_id: "fixture-activity-a:0:0".to_owned(),
            completed_at_unix_ms: NOW,
        });
        let stopped = evaluate(&unmapped, &unmapped_facts, &base_resources(), time, 9).unwrap();
        assert_eq!(stopped.dispatch_intents.len(), 1);
        assert_eq!(
            stopped.dispatch_intents[0].instance_id,
            "fixture-instance-b"
        );
        unmapped_facts.facts.clear();
        let unknown = evaluate(&unmapped, &unmapped_facts, &base_resources(), time, 9).unwrap();
        assert_eq!(
            decision_for(&unknown, "fixture.observe", "fixture-instance-a").eligibility,
            EligibilityState::Unknown
        );

        let next_day = EvaluationTime {
            unix_ms: NOW + 86_400_000,
            monotonic_ms: NOW + 86_400_000,
        };
        assert_eq!(
            evaluate(&catalog, &facts, &base_resources(), next_day, 9)
                .unwrap()
                .dispatch_intents
                .len(),
            2
        );
        let mut profile = catalog.catalog().activity.profiles[0].clone();
        profile.windows[0].weekdays = vec![4];
        profile.windows[0].start_minute_of_day = 22 * 60;
        profile.windows[0].end_minute_of_day = 2 * 60;
        let before_midnight = activity_window_at(&profile, 23 * 3_600_000)
            .unwrap()
            .unwrap();
        let after_midnight = activity_window_at(&profile, 25 * 3_600_000)
            .unwrap()
            .unwrap();
        assert_eq!(before_midnight, after_midnight);
        assert_eq!(after_midnight.window_id, "fixture-activity-a:0:0");
        assert!(
            activity_window_at(&profile, 26 * 3_600_000)
                .unwrap()
                .is_none()
        );
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
            EvaluationTime {
                unix_ms: NOW,
                monotonic_ms: NOW,
            },
            10,
        )
        .expect("evaluation");

        assert_eq!(result.decisions[0].eligibility, EligibilityState::True);
        assert_eq!(result.decisions[0].state, SchedulingDecisionState::Blocked);
        assert_eq!(
            result.decisions[0].reasons[1].outcome.code().as_str(),
            "instance_unavailable"
        );
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
            expires_at_unix_ms: None,
            confidence_milli: 1_000,
        });

        let result = evaluate(
            &catalog,
            &facts,
            &base_resources(),
            EvaluationTime {
                unix_ms: NOW,
                monotonic_ms: NOW,
            },
            11,
        )
        .expect("evaluation");
        let prerequisites = &result.dispatch_intents[0].prerequisites;

        assert_eq!(prerequisites.facts_fresh_until_unix_ms, Some(NOW + 1_000));
        assert_eq!(prerequisites.activity_profile_id, "fixture-activity-a");
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
            EvaluationTime {
                unix_ms: NOW,
                monotonic_ms: NOW,
            },
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

        let mut docs = example_documents();
        docs.0 = serde_json::to_value(&catalog.catalog().tasks).unwrap();
        docs.1["pools"][0]["projection"] = serde_json::json!({"kind":"none"});
        for (deadline, expected_urgency) in [(NOW + 10 * 86_400_000, 0), (NOW, 1000)] {
            docs.2["profiles"][0]["goals"][0]["deadline_unix_ms"] = serde_json::json!(deadline);
            let catalog = compile_documents(docs.clone());
            let result = evaluate(
                &catalog,
                &base_facts(),
                &resources,
                EvaluationTime {
                    unix_ms: NOW,
                    monotonic_ms: NOW,
                },
                5,
            )
            .unwrap();
            assert_eq!(result.decisions[0].eligibility, EligibilityState::True);
            assert_eq!(
                result.decisions[0].rank.as_ref().unwrap().urgency_milli,
                expected_urgency
            );
        }
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
            EvaluationTime {
                unix_ms: NOW,
                monotonic_ms: NOW,
            },
            12,
        )
        .expect("evaluation");

        assert_eq!(result.decisions[0].eligibility, EligibilityState::False);
        assert_eq!(result.next_wake_unix_ms, Some(NOW + 360_000));

        let mut fixed_docs = example_documents();
        fixed_docs.0 = serde_json::to_value(&catalog.catalog().tasks).unwrap();
        fixed_docs.1["pools"][0]["projection"] = serde_json::json!({"kind":"none"});
        let fixed = compile_documents(fixed_docs);
        for now in [NOW, NOW + 360_000] {
            let result = evaluate(
                &fixed,
                &base_facts(),
                &base_resources(),
                EvaluationTime {
                    unix_ms: now,
                    monotonic_ms: now,
                },
                12,
            )
            .unwrap();
            assert_eq!(result.decisions[0].eligibility, EligibilityState::False);
            // The catalog's daily reset still wakes the evaluator at 04:00.
            assert_eq!(result.next_wake_unix_ms, Some(240 * 60_000));
        }
        let spec = &fixed.catalog().pools.pools[0];
        let mut snapshot = base_resources().pools.remove(0);
        for value in [0, 10, spec.capacity] {
            snapshot.value = value;
            assert_eq!(
                project_pool_value(spec, &snapshot, NOW + 86_400_000).unwrap(),
                value
            );
        }
        snapshot.value = spec.capacity + 1;
        assert!(project_pool_value(spec, &snapshot, NOW).is_err());

        // LIVE-FACT-POOL-v1: one scoped observation supplies predicate and pool freshness.
        let mut docs = example_documents();
        docs.0["tasks"][0]["trigger"] = serde_json::json!({"kind":"all","predicates":[
            {"kind":"resource_projection","pool_id":"fixture-pool-a","comparison":"greater_than_or_equal","value":11},
            {"kind":"fact","scope":docs.1["pools"][0]["scope"].clone(),"fact_key":"resource.current","comparison":"greater_than_or_equal","value":{"type":"integer","value":11},"max_age_ms":null}
        ]});
        docs.0["tasks"][0]["feedback_stop"] = false_fact();
        docs.1["pools"][0]["observation"] =
            serde_json::json!({"kind":"fact","fact_key":"resource.current"});
        docs.1["pools"][0]["value_source"] =
            serde_json::json!({"kind":"ledger_fact","minimum_confidence_milli":900});
        let scope: ScopeSelector =
            serde_json::from_value(docs.1["pools"][0]["scope"].clone()).unwrap();
        for projection in [
            docs.1["pools"][0]["projection"].clone(),
            serde_json::json!({"kind":"none"}),
        ] {
            docs.1["pools"][0]["projection"] = projection;
            let catalog = compile_documents(docs.clone());
            let mut facts = base_facts();
            facts.facts.push(ObservedFact {
                scope: scope.clone(),
                fact_key: "resource.current".into(),
                value: FactValue::Integer(11),
                observed_at_unix_ms: NOW,
                expires_at_unix_ms: Some(NOW + 100),
                confidence_milli: 900,
            });
            let mut inputs = base_resources();
            inputs.pools.clear();
            let resources = project_fact_pools(&catalog, &facts, &inputs);
            assert_eq!(
                (
                    resources.pools[0].value,
                    resources.pools[0].observed_at_unix_ms
                ),
                (11, NOW)
            );
            for (now, eligible) in [
                (NOW, true),
                (NOW + 100, true),
                (NOW + 101, false),
                (NOW + 360_000, false),
            ] {
                let result = evaluate(
                    &catalog,
                    &facts,
                    &resources,
                    EvaluationTime {
                        unix_ms: now,
                        monotonic_ms: now,
                    },
                    12,
                )
                .unwrap();
                assert_eq!(!result.dispatch_intents.is_empty(), eligible);
                if eligible {
                    assert_eq!(
                        result.dispatch_intents[0]
                            .prerequisites
                            .facts_fresh_until_unix_ms,
                        Some(NOW + 100)
                    );
                }
            }
            for (value, confidence) in [
                (FactValue::Integer(11), 899),
                (FactValue::Boolean(true), 900),
                (FactValue::Integer(-1), 900),
                (FactValue::Integer(i64::MAX), 900),
            ] {
                let record = facts.facts.last_mut().unwrap();
                record.value = value;
                record.confidence_milli = confidence;
                assert!(
                    project_fact_pools(&catalog, &facts, &inputs)
                        .pools
                        .is_empty()
                );
            }
            let record = facts.facts.last_mut().unwrap();
            record.value = FactValue::Integer(11);
            record.confidence_milli = 900;
            record.scope = ScopeSelector::Instance {
                instance_id: "other-instance".into(),
            };
            assert!(
                project_fact_pools(&catalog, &facts, &inputs)
                    .pools
                    .is_empty()
            );
            facts.facts.pop();
            assert!(
                project_fact_pools(&catalog, &facts, &inputs)
                    .pools
                    .is_empty()
            );
        }
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
            completed_window: None,
            last_duration_ms: None,
            failure_streak: None,
        });

        let result = evaluate(
            &catalog,
            &facts,
            &base_resources(),
            EvaluationTime {
                unix_ms: NOW,
                monotonic_ms: NOW,
            },
            6,
        )
        .expect("evaluation");

        assert_eq!(result.decisions[0].eligibility, EligibilityState::False);
        assert_eq!(result.next_wake_unix_ms, Some(NOW + 3_600_000));
        let preload = result.preload_hint.expect("preload hint");
        assert_eq!(preload.task_id, "fixture.observe");
        assert_eq!(
            preload.package_ref,
            catalog.catalog().tasks.tasks[0].procedure_ref
        );
        assert_eq!(preload.not_before_unix_ms, NOW + 3_600_000);
        // WF342 P specification: an ended Runtime round restores the normal trigger gate.
        let retries = BTreeSet::from([(
            "fixture.observe".to_owned(),
            "fixture-instance-a".to_owned(),
        )]);
        let no_retries = BTreeSet::new();
        for (pairs, last_dispatch, should_dispatch) in [
            (&retries, Some(NOW), true),
            (&no_retries, Some(NOW), false),
            (&no_retries, None, true),
        ] {
            facts.tasks[0].last_dispatched_unix_ms = last_dispatch;
            let result = evaluate_with_immediate_retries::<PolicyEvaluationError>(
                &catalog,
                &facts,
                &base_resources(),
                EvaluationTime {
                    unix_ms: NOW,
                    monotonic_ms: NOW,
                },
                6,
                pairs,
                |_| Ok(CandidateEligibility::Eligible),
            )
            .expect("trigger with Runtime retry eligibility");
            assert_eq!(!result.dispatch_intents.is_empty(), should_dispatch);
        }
    }

    #[test]
    fn cooldown_defers_an_otherwise_due_task() {
        let catalog = catalog(|tasks| {
            tasks["tasks"][0]["trigger"] = due_clock();
            tasks["tasks"][0]["feedback_stop"] = false_fact();
            tasks["tasks"][0]["cooldown_ms"] = serde_json::json!(1_000);
        });
        let mut facts = base_facts();
        facts.tasks.push(TaskRuntimeSnapshot {
            task_id: "fixture.observe".to_owned(),
            instance_id: "fixture-instance-a".to_owned(),
            last_dispatched_unix_ms: Some(NOW - 100),
            eligible_since_unix_ms: Some(NOW - 100),
            terminal_state: None,
            completed_window: None,
            last_duration_ms: None,
            failure_streak: None,
        });

        let result = evaluate(
            &catalog,
            &facts,
            &base_resources(),
            EvaluationTime {
                unix_ms: NOW,
                monotonic_ms: NOW,
            },
            6,
        )
        .expect("evaluation");

        assert!(result.dispatch_intents.is_empty());
        assert_eq!(result.decisions[0].state, SchedulingDecisionState::Deferred);
        assert_eq!(result.next_wake_unix_ms, Some(NOW + 900));
    }

    #[test]
    fn local_clock_uses_monotonic_coordinate_and_projects_to_unix() {
        let schedule = ClockSchedule::Interval {
            clock_source: ClockSource::Local,
            every_ms: 100,
            anchor_ms: 0,
        };

        assert_eq!(
            schedule_occurrences(
                &schedule,
                EvaluationTime {
                    unix_ms: 10_000,
                    monotonic_ms: 250,
                }
            )
            .expect("local clock"),
            (Some(9_950), Some(10_050))
        );
    }

    // Workflow #372 K1: the due and next clock occurrence of one pair.
    #[test]
    fn clock_slot_view_reads_due_and_next_trigger_clocks() {
        const DAY: u64 = 86_400_000;
        const MINUTE: u64 = 60_000;
        // Minute `minute` of local day `day` (0 = yesterday, 1 = today) at UTC+09:00.
        let at = |day: u64, minute: u64| (20_000 + day) * DAY - 540 * MINUTE + minute * MINUTE;
        let daily = |minutes: &[u16]| PredicateSpec::Clock {
            schedule: ClockSchedule::Daily {
                clock_source: ClockSource::Server {
                    timezone_id: "fixture/zone".to_owned(),
                    utc_offset_minutes: 540,
                    dst_offset_minutes: 0,
                    maintenance_drift_ms: 0,
                },
                minutes_of_day: minutes.to_vec(),
            },
            missed: None,
        };
        let interval = |every_ms: u64| PredicateSpec::Clock {
            schedule: ClockSchedule::Interval {
                clock_source: ClockSource::Local,
                every_ms,
                anchor_ms: 0,
            },
            missed: None,
        };
        let dependency = PredicateSpec::DependencyCompleted {
            task_id: "fixture.head".to_owned(),
            terminal_states: vec![TaskTerminalState::Succeeded],
        };
        let two_slots = daily(&[360, 1140]);
        let one_slot = daily(&[390]);
        let compiled = catalog(|_| {});
        let mut task = compiled.catalog().tasks.tasks[0].clone();
        for (case, trigger, last, now, expected) in [
            (
                "second slot due after the first was served",
                two_slots.clone(),
                Some(at(1, 365)),
                at(1, 1140) + 30_000,
                Some((Some(at(1, 1140)), Some(at(2, 360)))),
            ),
            (
                "second slot served",
                two_slots,
                Some(at(1, 1140) + 10_000),
                at(1, 1140) + 30_000,
                Some((None, Some(at(2, 360)))),
            ),
            (
                "owed slot before the day's own slot",
                one_slot.clone(),
                Some(at(0, 367)),
                at(1, 367),
                Some((Some(at(0, 390)), Some(at(1, 390)))),
            ),
            (
                "no dispatch history",
                one_slot.clone(),
                None,
                at(1, 367),
                Some((Some(at(0, 390)), Some(at(1, 390)))),
            ),
            (
                "any: greatest due and least next",
                PredicateSpec::Any {
                    predicates: vec![daily(&[360]), daily(&[1140])],
                },
                None,
                at(1, 1140) + 30_000,
                Some((Some(at(1, 1140)), Some(at(2, 360)))),
            ),
            (
                "a negated clock is ignored",
                PredicateSpec::All {
                    predicates: vec![
                        one_slot.clone(),
                        PredicateSpec::Not {
                            predicate: Box::new(daily(&[1140])),
                        },
                    ],
                },
                Some(at(0, 367)),
                at(1, 367),
                Some((Some(at(0, 390)), Some(at(1, 390)))),
            ),
            (
                "all: a clock beside a dependency",
                PredicateSpec::All {
                    predicates: vec![one_slot.clone(), dependency.clone()],
                },
                Some(at(0, 367)),
                at(1, 367),
                Some((Some(at(0, 390)), Some(at(1, 390)))),
            ),
            (
                "only a negated clock",
                PredicateSpec::Not {
                    predicate: Box::new(one_slot.clone()),
                },
                None,
                at(1, 367),
                None,
            ),
            ("no clock", dependency, None, at(1, 367), None),
            (
                "interval under one hour",
                interval(60_000),
                None,
                at(1, 367),
                None,
            ),
            (
                "interval under one hour beside a daily clock",
                PredicateSpec::Any {
                    predicates: vec![one_slot, interval(60_000)],
                },
                None,
                at(1, 367),
                None,
            ),
            (
                "hourly interval",
                interval(3_600_000),
                None,
                at(1, 30),
                Some((Some(at(1, 0)), Some(at(1, 60)))),
            ),
        ] {
            task.trigger = trigger;
            let state = TaskRuntimeSnapshot {
                task_id: task.id.clone(),
                instance_id: "fixture-instance-a".to_owned(),
                last_dispatched_unix_ms: last,
                eligible_since_unix_ms: None,
                terminal_state: None,
                completed_window: None,
                last_duration_ms: None,
                failure_streak: None,
            };
            let time = EvaluationTime {
                unix_ms: now,
                monotonic_ms: now,
            };
            let view = clock_slot_view(&task, last.map(|_| &state), time).expect(case);
            assert_eq!(
                view.map(|view| (view.due_occurrence, view.next_occurrence)),
                expected,
                "{case}"
            );
        }
    }

    // Workflow #361 R7: a UTC `daily` clock at the given minutes of the day, with
    // `missed: skip_after` of `grace_ms` when one is given.
    fn r7_daily(minutes: &[u16], grace_ms: Option<u64>) -> PredicateSpec {
        PredicateSpec::Clock {
            schedule: ClockSchedule::Daily {
                clock_source: ClockSource::Server {
                    timezone_id: "fixture/utc".to_owned(),
                    utc_offset_minutes: 0,
                    dst_offset_minutes: 0,
                    maintenance_drift_ms: 0,
                },
                minutes_of_day: minutes.to_vec(),
            },
            missed: grace_ms.map(|grace_ms| MissedSlotRule::SkipAfter { grace_ms }),
        }
    }

    // Workflow #361 R7: `hour:minute` UTC of day `day` (0 = yesterday, 1 = today).
    const fn r7_at(day: u64, hour: u64, minute: u64) -> u64 {
        (20_000 + day) * 86_400_000 + hour * 3_600_000 + minute * 60_000
    }

    fn r7_time(now: u64) -> EvaluationTime {
        EvaluationTime {
            unix_ms: now,
            monotonic_ms: now,
        }
    }

    // The fixture pair's snapshot with its last dispatch, `None` for a pair never dispatched.
    fn r7_state(last_dispatched_unix_ms: Option<u64>) -> TaskRuntimeSnapshot {
        TaskRuntimeSnapshot {
            task_id: "fixture.observe".to_owned(),
            instance_id: "fixture-instance-a".to_owned(),
            last_dispatched_unix_ms,
            eligible_since_unix_ms: None,
            terminal_state: None,
            completed_window: None,
            last_duration_ms: None,
            failure_streak: None,
        }
    }

    // One predicate evaluated for the fixture pair at `now`, and the reasons it added.
    fn r7_evaluate(
        predicate: &PredicateSpec,
        last_dispatched_unix_ms: Option<u64>,
        now: u64,
    ) -> (PredicateEvaluation, Vec<DecisionReason>) {
        let mut reasons = Vec::new();
        let result = evaluate_predicate(
            predicate,
            Some(&r7_state(last_dispatched_unix_ms)),
            &ScopeSelector::Instance {
                instance_id: "fixture-instance-a".to_owned(),
            },
            &base_facts(),
            &BTreeMap::new(),
            &BTreeMap::new(),
            r7_time(now),
            &BTreeMap::new(),
            &mut reasons,
        )
        .unwrap_or_else(|error| panic!("the clock evaluates at {now}: {error}"));
        (result, reasons)
    }

    // The one reason an expired clock adds: clock_occurrence_expired with its occurrence and
    // its grace.
    fn assert_expired_reason(reasons: &[DecisionReason], occurrence: u64, grace_ms: u64) {
        assert_eq!(reasons.len(), 1, "{reasons:?}");
        let outcome = &reasons[0].outcome;
        assert!(outcome.is(PolicyCode::ClockOccurrenceExpired), "{outcome}");
        assert_eq!(
            outcome.value("occurrence_unix_ms"),
            Some(&serde_json::json!(occurrence)),
            "{outcome}"
        );
        assert_eq!(
            outcome.value("grace_ms"),
            Some(&serde_json::json!(grace_ms)),
            "{outcome}"
        );
    }

    // Workflow #361 R7, row R7-T1 (M361 addendum of 2026-10-15): `daily [06:30]` with
    // `missed {skip_after, 7 200 000}`, last dispatched yesterday at 06:35. Today's 06:30 is due
    // at 06:31 and at 08:29:59.999; at 08:30:00.000 it is not, with the reason
    // clock_occurrence_expired {06:30, 7 200 000} and the next wake tomorrow at 06:30. Without
    // `missed` the same three times are all due, as before R7.
    #[test]
    fn r7_t1_a_missed_clock_is_due_only_inside_its_grace() {
        let last = Some(r7_at(0, 6, 35));
        let missed = r7_daily(&[390], Some(7_200_000));
        for now in [r7_at(1, 6, 31), r7_at(1, 8, 30) - 1] {
            let (result, reasons) = r7_evaluate(&missed, last, now);
            assert_eq!(result.truth, PredicateTruth::True, "due at {now}");
            assert!(reasons.is_empty(), "{reasons:?}");
        }
        let (result, reasons) = r7_evaluate(&missed, last, r7_at(1, 8, 30));
        assert_eq!(result.truth, PredicateTruth::False);
        assert_eq!(result.next_wake_unix_ms, Some(r7_at(2, 6, 30)));
        assert_expired_reason(&reasons, r7_at(1, 6, 30), 7_200_000);

        let plain = r7_daily(&[390], None);
        for now in [r7_at(1, 6, 31), r7_at(1, 8, 30) - 1, r7_at(1, 8, 30)] {
            let (result, reasons) = r7_evaluate(&plain, last, now);
            assert_eq!(
                result.truth,
                PredicateTruth::True,
                "due at {now} without missed"
            );
            assert!(reasons.is_empty(), "{reasons:?}");
        }
    }

    // Workflow #361 R7, row R7-T2: at 06:31 the due clock of R7-T1 wakes, and stays fresh, until
    // 08:30, the instant its grace runs out.
    #[test]
    fn r7_t2_a_due_missed_clock_wakes_and_stays_fresh_until_its_grace_ends() {
        let (result, _) = r7_evaluate(
            &r7_daily(&[390], Some(7_200_000)),
            Some(r7_at(0, 6, 35)),
            r7_at(1, 6, 31),
        );
        assert_eq!(result.truth, PredicateTruth::True);
        assert_eq!(result.next_wake_unix_ms, Some(r7_at(1, 8, 30)));
        assert_eq!(result.fresh_until_unix_ms, Some(r7_at(1, 8, 30)));
    }

    // Workflow #361 R7, row R7-T3: a pair with no dispatch history follows the same rule, which
    // is its whole first-run bound: 07:00 is due, 09:00 is not.
    #[test]
    fn r7_t3_a_first_run_is_due_only_inside_the_grace() {
        let missed = r7_daily(&[390], Some(7_200_000));
        let (result, reasons) = r7_evaluate(&missed, None, r7_at(1, 7, 0));
        assert_eq!(result.truth, PredicateTruth::True);
        assert!(reasons.is_empty(), "{reasons:?}");
        let (result, reasons) = r7_evaluate(&missed, None, r7_at(1, 9, 0));
        assert_eq!(result.truth, PredicateTruth::False);
        assert_expired_reason(&reasons, r7_at(1, 6, 30), 7_200_000);
    }

    // Workflow #361 R7, row R7-T4: `any(clock [06:00] {missed 3 600 000}, clock [19:00])` at
    // 07:30, last dispatched the day before at 18:05. Today's 06:00 expired; yesterday's 19:00,
    // whose clock has no `missed`, is still due, so the trigger is due through it, and the
    // clock-slot view's due occurrence is yesterday's 19:00.
    #[test]
    fn r7_t4_an_expired_clock_leaves_the_trigger_due_through_a_clock_without_missed() {
        let trigger = PredicateSpec::Any {
            predicates: vec![r7_daily(&[360], Some(3_600_000)), r7_daily(&[1140], None)],
        };
        let last = Some(r7_at(0, 18, 5));
        let now = r7_at(1, 7, 30);
        let (result, reasons) = r7_evaluate(&trigger, last, now);
        assert_eq!(result.truth, PredicateTruth::True);
        assert_expired_reason(&reasons, r7_at(1, 6, 0), 3_600_000);

        let mut task = catalog(|_| {}).catalog().tasks.tasks[0].clone();
        task.trigger = trigger;
        let view = clock_slot_view(&task, Some(&r7_state(last)), r7_time(now))
            .expect("the clock-slot view reads")
            .expect("both clocks count");
        assert_eq!(view.due_occurrence, Some(r7_at(0, 19, 0)));
    }

    // Workflow #361 R7, row R7-T8: the clock-slot view never names an expired occurrence. For
    // R7-T1's clock, today's 06:30 is the due occurrence at 08:29:59.999 and there is none at
    // 08:30; the next occurrence is tomorrow's 06:30 at both. Without `missed` today's 06:30
    // is still due at 08:30.
    #[test]
    fn r7_t8_the_clock_slot_view_never_names_an_expired_occurrence() {
        let mut task = catalog(|_| {}).catalog().tasks.tasks[0].clone();
        let state = r7_state(Some(r7_at(0, 6, 35)));
        let view = |task: &TaskSpec, now: u64| {
            clock_slot_view(task, Some(&state), r7_time(now))
                .expect("the clock-slot view reads")
                .map(|view| (view.due_occurrence, view.next_occurrence))
        };
        task.trigger = r7_daily(&[390], Some(7_200_000));
        assert_eq!(
            view(&task, r7_at(1, 8, 30) - 1),
            Some((Some(r7_at(1, 6, 30)), Some(r7_at(2, 6, 30))))
        );
        assert_eq!(
            view(&task, r7_at(1, 8, 30)),
            Some((None, Some(r7_at(2, 6, 30))))
        );
        task.trigger = r7_daily(&[390], None);
        assert_eq!(
            view(&task, r7_at(1, 8, 30)),
            Some((Some(r7_at(1, 6, 30)), Some(r7_at(2, 6, 30))))
        );
    }

    #[test]
    fn server_clock_applies_dst_and_maintenance_drift() {
        let schedule = ClockSchedule::Daily {
            clock_source: ClockSource::Server {
                timezone_id: "fixture/zone".to_owned(),
                utc_offset_minutes: 540,
                dst_offset_minutes: 60,
                maintenance_drift_ms: 3_600_000,
            },
            minutes_of_day: vec![0],
        };

        assert_eq!(
            schedule_occurrences(
                &schedule,
                EvaluationTime {
                    unix_ms: 86_400_000,
                    monotonic_ms: 86_400_000,
                }
            )
            .expect("server clock"),
            (Some(54_000_000), Some(140_400_000))
        );
    }

    #[test]
    fn server_clock_accepts_independently_bounded_utc_and_dst_offsets() {
        for (utc_offset_minutes, dst_offset_minutes, expected) in [
            (
                crate::MIN_UTC_OFFSET_MINUTES,
                crate::MIN_DST_OFFSET_MINUTES,
                -960,
            ),
            (
                crate::MAX_UTC_OFFSET_MINUTES,
                crate::MAX_DST_OFFSET_MINUTES,
                960,
            ),
        ] {
            let coordinate = ClockCoordinate::new(
                &ClockSource::Server {
                    timezone_id: "fixture/zone".to_owned(),
                    utc_offset_minutes,
                    dst_offset_minutes,
                    maintenance_drift_ms: 0,
                },
                EvaluationTime {
                    unix_ms: NOW,
                    monotonic_ms: NOW,
                },
            )
            .expect("bounded clock coordinate");
            assert_eq!(coordinate.utc_offset_minutes, expected);
        }
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
            expires_at_unix_ms: None,
            confidence_milli: 1_000,
        });

        let result = evaluate(
            &catalog,
            &facts,
            &base_resources(),
            EvaluationTime {
                unix_ms: NOW,
                monotonic_ms: NOW,
            },
            7,
        )
        .expect("evaluation");

        assert_eq!(result.decisions[0].eligibility, EligibilityState::Unknown);
        assert_eq!(
            result.decisions[0].detection_suggestions[0].reason,
            "fact_stale"
        );

        // Specification criterion: outcome TTL shares the fact freshness boundary.
        let mut docs = example_documents();
        docs.0["tasks"][0]["trigger"] = serde_json::json!({
            "kind":"outcome","task_id":"fixture.observe","outcome_key":"completed",
            "comparison":"eq","value":{"type":"boolean","value":true}
        });
        let compiled = compile_documents(docs);
        facts.outcomes.push(ObservedOutcome {
            task_id: "fixture.observe".to_owned(),
            instance_id: "fixture-instance-a".to_owned(),
            outcome_key: "completed".to_owned(),
            value: FactValue::Boolean(true),
            observed_at_unix_ms: NOW,
            expires_at_unix_ms: Some(NOW + 10),
            activity_window_id: None,
        });
        for (now, expected) in [
            (NOW, EligibilityState::True),
            (NOW + 10, EligibilityState::True),
            (NOW + 11, EligibilityState::Unknown),
        ] {
            let result = evaluate(
                &compiled,
                &facts,
                &base_resources(),
                EvaluationTime {
                    unix_ms: now,
                    monotonic_ms: now,
                },
                7,
            )
            .unwrap();
            assert_eq!(result.decisions[0].eligibility, expected);
            if expected == EligibilityState::True {
                assert_eq!(
                    result.dispatch_intents[0]
                        .prerequisites
                        .facts_fresh_until_unix_ms,
                    Some(NOW + 10)
                );
                assert_eq!(result.next_wake_unix_ms, Some(NOW + 11));
            } else {
                assert_eq!(
                    result.decisions[0].detection_suggestions[0].reason,
                    "outcome_expired"
                );
            }
        }
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
            EvaluationTime {
                unix_ms: NOW,
                monotonic_ms: NOW,
            },
            8,
        )
        .expect_err("invalid host reference");

        assert_eq!(error.code().as_str(), "policy_evaluation_input_invalid");
        assert!(error.message().contains("missing host"));
    }

    /// P-1 G10 (Workflow #386 SR-6a; model-sched-rules.md v1.4.2 §8.2 item 2): the close of a
    /// round in which one path left a decision without any reason, the programming error the
    /// evaluator guards against. `fixture.observe` was selected on `fixture-instance-a`;
    /// `fixture.observe-secondary` reached the close of the round on the same instance without a
    /// reason. No catalog or input reaches this state through `evaluate`, so the round is built
    /// here and closed by the evaluator's own `append_decision_records`.
    pub(crate) fn round_with_a_decision_left_without_a_reason()
    -> PolicyEvaluationResult<(Vec<TaskDecision>, Vec<DecisionReasonChain>)> {
        let rank = TaskRank {
            priority: 100,
            aging_ms: 0,
            strategic_weight_milli: 1_000,
            urgency_milli: 0,
            load_cost_milli: 300,
            contention_penalty: 0,
            total_score: 101_000_000,
            effective_milli: 0,
        };
        let mut selected = TaskWork::new(
            "fixture.observe".to_owned(),
            "fixture-instance-a".to_owned(),
        );
        selected.eligibility = EligibilityState::True;
        selected.state = SchedulingDecisionState::Selected;
        selected.reasons = vec![
            reason(
                PolicyCode::TaskEligible,
                "trigger passed and feedback stop did not fire",
            ),
            reason(
                PolicyCode::PlacementSelected,
                "instance 'fixture-instance-a' satisfies scope, capability, affinity, and host budget",
            ),
            reason(
                PolicyCode::RankAssigned,
                "deterministic total score 101000000",
            ),
        ];
        selected.record = Some(RankRecord {
            base_total: rank.total_score,
            stage: None,
            resources: None,
            promoted: false,
            affinity: false,
            tie_breaker: 1,
        });
        selected.rank = Some(rank);
        let mut reason_chains = vec![DecisionReasonChain {
            id: "reason:fixture-round".to_owned(),
            decision_id: "decision:fixture-round".to_owned(),
            reasons: selected.reasons.clone(),
        }];
        let mut work = vec![
            selected,
            TaskWork::new(
                "fixture.observe-secondary".to_owned(),
                "fixture-instance-a".to_owned(),
            ),
        ];
        append_decision_records(&mut work, &mut reason_chains, &[(0, 0)], &BTreeMap::new())?;
        Ok((
            work.into_iter().map(TaskDecision::from).collect(),
            reason_chains,
        ))
    }

    pub(crate) fn decision_for<'a>(
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

    pub(crate) fn two_task_catalog(
        mutator: impl FnOnce(&mut Vec<serde_json::Value>),
    ) -> CompiledCatalog {
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
            selection: None,
        })
        .expect("compiled catalog")
    }

    pub(crate) fn due_clock() -> serde_json::Value {
        serde_json::json!({
            "kind": "clock",
            "schedule": {"kind": "interval", "clock_source": {"kind": "local"}, "every_ms": 3600000, "anchor_ms": 0}
        })
    }

    pub(crate) fn false_fact() -> serde_json::Value {
        serde_json::json!({
            "kind": "fact",
            "scope": {"kind": "instance", "instance_id": "fixture-instance-a"},
            "fact_key": "fixture.completed",
            "comparison": "eq",
            "value": {"type": "boolean", "value": true},
            "max_age_ms": null
        })
    }

    pub(crate) fn base_facts() -> EvaluationFacts {
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
                expires_at_unix_ms: None,
                confidence_milli: 1_000,
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
            priority_offsets: Vec::new(),
        }
    }

    pub(crate) fn base_resources() -> EvaluationResources {
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
                host_responsiveness_basis_points: 10_000,
                third_party_pressure_basis_points: 0,
                heavy_dispatch_limit: 1,
                active_heavy_dispatches: 0,
                heavy_threshold_milli: crate::DEFAULT_HEAVY_THRESHOLD_MILLI,
            }],
        }
    }
}
