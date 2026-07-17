// SPDX-License-Identifier: AGPL-3.0-only

//! Pure forward planning and evidence-bounded maintenance assessment.

use crate::{
    CompiledCatalog, DetectionSuggestion, EffectDirection, EvaluationFacts, EvaluationResources,
    EvaluationTime, HostResourceSnapshot, ObservationSource, PolicyEvaluationError, PoolSpec,
    PoolValueSnapshot, ScopeSelector, TaskRuntimeSnapshot, TaskTerminalState, evaluate,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

pub const MAX_FORWARD_HORIZON_MS: u64 = 24 * 60 * 60 * 1_000;
pub const MAX_FORWARD_STEPS: u32 = 4_096;
pub const MAX_MAINTENANCE_SAMPLES: usize = 4_096;

pub type ForwardResult<T> = Result<T, ForwardError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardError {
    code: &'static str,
    message: String,
}

impl ForwardError {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: "forward_projection_invalid",
            message: message.into(),
        }
    }

    fn overflow(message: impl Into<String>) -> Self {
        Self {
            code: "forward_projection_overflow",
            message: message.into(),
        }
    }

    fn evaluation(error: PolicyEvaluationError) -> Self {
        Self {
            code: error.code(),
            message: error.message().to_owned(),
        }
    }

    pub const fn code(&self) -> &'static str {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ForwardError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl Error for ForwardError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForwardProjectionConfig {
    pub horizon_ms: u64,
    pub max_steps: u32,
}

impl ForwardProjectionConfig {
    pub const fn next_24_hours() -> Self {
        Self {
            horizon_ms: MAX_FORWARD_HORIZON_MS,
            max_steps: MAX_FORWARD_STEPS,
        }
    }

    pub fn for_hours(hours: u16, max_steps: u32) -> ForwardResult<Self> {
        let config = Self {
            horizon_ms: u64::from(hours)
                .checked_mul(60 * 60 * 1_000)
                .ok_or_else(|| ForwardError::overflow("projection horizon overflowed"))?,
            max_steps,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> ForwardResult<()> {
        if self.horizon_ms == 0
            || self.horizon_ms > MAX_FORWARD_HORIZON_MS
            || self.max_steps == 0
            || self.max_steps > MAX_FORWARD_STEPS
        {
            return Err(ForwardError::invalid(
                "projection horizon or step budget is outside the frozen bounds",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForwardProjectionCompleteness {
    Complete,
    EvidenceInsufficient,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForwardEvidenceGap {
    pub code: String,
    pub task_id: Option<String>,
    pub fact_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForwardPoolState {
    pub pool_id: String,
    pub value: u64,
    pub cumulative_waste: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForwardProjectionStep {
    pub step: u32,
    pub evaluated_at_unix_ms: u64,
    pub completed_at_unix_ms: u64,
    pub decision_ids: Vec<String>,
    pub task_ids: Vec<String>,
    pub pools: Vec<ForwardPoolState>,
    pub waste_delta: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForwardProjection {
    pub catalog_hash: String,
    pub catalog_version: u64,
    pub as_of_unix_ms: u64,
    pub horizon_end_unix_ms: u64,
    pub completeness: ForwardProjectionCompleteness,
    pub evidence_gaps: Vec<ForwardEvidenceGap>,
    pub steps: Vec<ForwardProjectionStep>,
    pub final_pools: Vec<ForwardPoolState>,
    pub cumulative_waste: u64,
    pub next_wake_unix_ms: Option<u64>,
}

struct SimulatedPool {
    spec: PoolSpec,
    snapshot: PoolValueSnapshot,
    remainder_ms: u64,
    cumulative_waste: u64,
}

/// Runs the existing deterministic evaluator over a bounded future horizon.
pub fn project_forward(
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    resources: &EvaluationResources,
    time: EvaluationTime,
    seed: u64,
    config: ForwardProjectionConfig,
) -> ForwardResult<ForwardProjection> {
    config.validate()?;
    let horizon_end_unix_ms = time
        .unix_ms
        .checked_add(config.horizon_ms)
        .ok_or_else(|| ForwardError::overflow("projection end time overflowed"))?;
    let mut projected_facts = facts.clone();
    let mut pools = initialize_pools(catalog, resources, time.unix_ms)?;
    let hosts = resources.hosts.clone();
    let mut current = time.unix_ms;
    let mut steps = Vec::new();
    let mut gaps = BTreeMap::<(String, String, String), ForwardEvidenceGap>::new();
    let mut next_wake = None;
    let mut evaluation_budget_exhausted = true;

    for step_index in 0..config.max_steps {
        if current >= horizon_end_unix_ms {
            evaluation_budget_exhausted = false;
            break;
        }
        let current_resources = projected_resources(&pools, &hosts);
        let evaluation = evaluate(
            catalog,
            &projected_facts,
            &current_resources,
            EvaluationTime { unix_ms: current },
            seed.wrapping_add(u64::from(step_index)),
        )
        .map_err(ForwardError::evaluation)?;
        collect_detection_gaps(&evaluation.decisions, &mut gaps);
        next_wake = evaluation.next_wake_unix_ms;
        if evaluation.dispatch_intents.is_empty() {
            let Some(wake) = evaluation.next_wake_unix_ms else {
                evaluation_budget_exhausted = false;
                break;
            };
            if wake <= current {
                return Err(ForwardError::invalid(
                    "the evaluator returned a non-advancing wake time",
                ));
            }
            current = wake.min(horizon_end_unix_ms);
            continue;
        }
        if let Some(gap) = uncertain_effect_gap(catalog, &evaluation.dispatch_intents)? {
            gaps.insert(
                (
                    gap.task_id.clone().unwrap_or_default(),
                    gap.fact_key.clone().unwrap_or_default(),
                    gap.code.clone(),
                ),
                gap,
            );
            evaluation_budget_exhausted = false;
            break;
        }
        let duration = evaluation
            .dispatch_intents
            .iter()
            .map(|intent| intent.expected_duration_ms)
            .max()
            .ok_or_else(|| ForwardError::invalid("selected projection round was empty"))?;
        let completed_at = current
            .checked_add(duration)
            .ok_or_else(|| ForwardError::overflow("projected completion time overflowed"))?;
        if completed_at > horizon_end_unix_ms {
            evaluation_budget_exhausted = false;
            break;
        }
        let before_waste = total_waste(&pools)?;
        apply_round_effects(catalog, &evaluation.dispatch_intents, &mut pools, current)?;
        update_task_states(
            &mut projected_facts.tasks,
            &evaluation.dispatch_intents,
            current,
        );
        let after_waste = total_waste(&pools)?;
        let mut decision_ids = evaluation
            .dispatch_intents
            .iter()
            .map(|intent| intent.decision_id.clone())
            .collect::<Vec<_>>();
        let mut task_ids = evaluation
            .dispatch_intents
            .iter()
            .map(|intent| intent.task_id.clone())
            .collect::<Vec<_>>();
        decision_ids.sort();
        task_ids.sort();
        task_ids.dedup();
        steps.push(ForwardProjectionStep {
            step: step_index + 1,
            evaluated_at_unix_ms: current,
            completed_at_unix_ms: completed_at,
            decision_ids,
            task_ids,
            pools: pool_projection(&pools),
            waste_delta: after_waste
                .checked_sub(before_waste)
                .ok_or_else(|| ForwardError::invalid("projected waste regressed"))?,
        });
        current = completed_at;
    }
    if current < horizon_end_unix_ms && evaluation_budget_exhausted {
        gaps.insert(
            (
                String::new(),
                String::new(),
                "step_budget_exhausted".to_owned(),
            ),
            ForwardEvidenceGap {
                code: "step_budget_exhausted".to_owned(),
                task_id: None,
                fact_key: None,
            },
        );
    }
    advance_pools(&mut pools, horizon_end_unix_ms, true)?;
    let evidence_gaps = gaps.into_values().collect::<Vec<_>>();
    Ok(ForwardProjection {
        catalog_hash: catalog.catalog_hash().to_owned(),
        catalog_version: catalog.catalog().tasks.catalog.catalog_version,
        as_of_unix_ms: time.unix_ms,
        horizon_end_unix_ms,
        completeness: if evidence_gaps.is_empty() {
            ForwardProjectionCompleteness::Complete
        } else {
            ForwardProjectionCompleteness::EvidenceInsufficient
        },
        evidence_gaps,
        steps,
        final_pools: pool_projection(&pools),
        cumulative_waste: total_waste(&pools)?,
        next_wake_unix_ms: next_wake.filter(|wake| *wake > horizon_end_unix_ms),
    })
}

fn apply_round_effects(
    catalog: &CompiledCatalog,
    intents: &[crate::DispatchIntent],
    pools: &mut BTreeMap<String, SimulatedPool>,
    started_at_unix_ms: u64,
) -> ForwardResult<()> {
    let mut ordered = intents.to_vec();
    ordered.sort_by(|left, right| {
        left.expected_duration_ms
            .cmp(&right.expected_duration_ms)
            .then_with(|| left.task_id.cmp(&right.task_id))
            .then_with(|| left.decision_id.cmp(&right.decision_id))
    });
    let mut start = 0;
    while start < ordered.len() {
        let duration_ms = ordered[start].expected_duration_ms;
        let completed_at_unix_ms = started_at_unix_ms
            .checked_add(duration_ms)
            .ok_or_else(|| ForwardError::overflow("projected completion time overflowed"))?;
        advance_pools(pools, completed_at_unix_ms, true)?;
        let mut end = start + 1;
        while end < ordered.len() && ordered[end].expected_duration_ms == duration_ms {
            end += 1;
        }
        apply_declared_effects(catalog, &ordered[start..end], pools)?;
        start = end;
    }
    Ok(())
}

fn initialize_pools(
    catalog: &CompiledCatalog,
    resources: &EvaluationResources,
    as_of_unix_ms: u64,
) -> ForwardResult<BTreeMap<String, SimulatedPool>> {
    let specs = catalog
        .catalog()
        .pools
        .pools
        .iter()
        .map(|pool| (pool.id.as_str(), pool))
        .collect::<BTreeMap<_, _>>();
    let mut pools = BTreeMap::new();
    for snapshot in &resources.pools {
        let spec = specs.get(snapshot.pool_id.as_str()).ok_or_else(|| {
            ForwardError::invalid(format!(
                "pool '{}' has no catalog declaration",
                snapshot.pool_id
            ))
        })?;
        if snapshot.observed_at_unix_ms > as_of_unix_ms || snapshot.value > spec.capacity {
            return Err(ForwardError::invalid(format!(
                "pool '{}' has an invalid initial snapshot",
                snapshot.pool_id
            )));
        }
        if pools
            .insert(
                snapshot.pool_id.clone(),
                SimulatedPool {
                    spec: (*spec).clone(),
                    snapshot: snapshot.clone(),
                    remainder_ms: 0,
                    cumulative_waste: 0,
                },
            )
            .is_some()
        {
            return Err(ForwardError::invalid("duplicate projected pool"));
        }
    }
    advance_pools(&mut pools, as_of_unix_ms, false)?;
    Ok(pools)
}

fn projected_resources(
    pools: &BTreeMap<String, SimulatedPool>,
    hosts: &[HostResourceSnapshot],
) -> EvaluationResources {
    EvaluationResources {
        pools: pools.values().map(|pool| pool.snapshot.clone()).collect(),
        hosts: hosts.to_vec(),
    }
}

fn advance_pools(
    pools: &mut BTreeMap<String, SimulatedPool>,
    to_unix_ms: u64,
    count_waste: bool,
) -> ForwardResult<()> {
    for pool in pools.values_mut() {
        if to_unix_ms < pool.snapshot.observed_at_unix_ms {
            return Err(ForwardError::invalid("projected pool time moved backwards"));
        }
        let elapsed = to_unix_ms - pool.snapshot.observed_at_unix_ms;
        let total_elapsed = elapsed
            .checked_add(pool.remainder_ms)
            .ok_or_else(|| ForwardError::overflow("pool elapsed time overflowed"))?;
        let periods = total_elapsed / pool.spec.projection.per_ms;
        pool.remainder_ms = total_elapsed % pool.spec.projection.per_ms;
        let regenerated = periods
            .checked_mul(pool.spec.projection.amount)
            .ok_or_else(|| ForwardError::overflow("pool regeneration overflowed"))?;
        let raw = pool
            .snapshot
            .value
            .checked_add(regenerated)
            .ok_or_else(|| ForwardError::overflow("pool value overflowed"))?;
        if count_waste {
            pool.cumulative_waste = pool
                .cumulative_waste
                .checked_add(raw.saturating_sub(pool.spec.capacity))
                .ok_or_else(|| ForwardError::overflow("pool waste overflowed"))?;
        }
        pool.snapshot.value = raw.min(pool.spec.capacity);
        pool.snapshot.observed_at_unix_ms = to_unix_ms;
    }
    Ok(())
}

fn apply_declared_effects(
    catalog: &CompiledCatalog,
    intents: &[crate::DispatchIntent],
    pools: &mut BTreeMap<String, SimulatedPool>,
) -> ForwardResult<()> {
    let tasks = catalog
        .catalog()
        .tasks
        .tasks
        .iter()
        .map(|task| (task.id.as_str(), task))
        .collect::<BTreeMap<_, _>>();
    for intent in intents {
        let task = tasks.get(intent.task_id.as_str()).ok_or_else(|| {
            ForwardError::invalid(format!("projected task '{}' is missing", intent.task_id))
        })?;
        for effect in task.consumes.iter().chain(task.produces.iter()) {
            let pool = pools.get_mut(&effect.pool_id).ok_or_else(|| {
                ForwardError::invalid(format!(
                    "task '{}' references missing projected pool '{}'",
                    task.id, effect.pool_id
                ))
            })?;
            match effect.direction {
                EffectDirection::Consume => {
                    pool.snapshot.value = pool
                        .snapshot
                        .value
                        .checked_sub(effect.amount)
                        .ok_or_else(|| {
                            ForwardError::invalid(format!(
                                "task '{}' consumes more '{}' than projected",
                                task.id, effect.pool_id
                            ))
                        })?;
                }
                EffectDirection::Produce => {
                    let raw = pool
                        .snapshot
                        .value
                        .checked_add(effect.amount)
                        .ok_or_else(|| ForwardError::overflow("task production overflowed"))?;
                    pool.cumulative_waste = pool
                        .cumulative_waste
                        .checked_add(raw.saturating_sub(pool.spec.capacity))
                        .ok_or_else(|| ForwardError::overflow("task waste overflowed"))?;
                    pool.snapshot.value = raw.min(pool.spec.capacity);
                }
            }
        }
    }
    Ok(())
}

fn uncertain_effect_gap(
    catalog: &CompiledCatalog,
    intents: &[crate::DispatchIntent],
) -> ForwardResult<Option<ForwardEvidenceGap>> {
    let tasks = catalog
        .catalog()
        .tasks
        .tasks
        .iter()
        .map(|task| (task.id.as_str(), task))
        .collect::<BTreeMap<_, _>>();
    for intent in intents {
        let task = tasks.get(intent.task_id.as_str()).ok_or_else(|| {
            ForwardError::invalid(format!("projected task '{}' is missing", intent.task_id))
        })?;
        if task
            .consumes
            .iter()
            .chain(task.produces.iter())
            .any(|effect| {
                effect.confidence_milli < 1_000
                    || effect.observation_source == ObservationSource::Inferred
            })
        {
            return Ok(Some(ForwardEvidenceGap {
                code: "effect_evidence_insufficient".to_owned(),
                task_id: Some(task.id.clone()),
                fact_key: None,
            }));
        }
    }
    Ok(None)
}

fn update_task_states(
    states: &mut Vec<TaskRuntimeSnapshot>,
    intents: &[crate::DispatchIntent],
    dispatched_at_unix_ms: u64,
) {
    for intent in intents {
        if let Some(state) = states.iter_mut().find(|state| {
            state.task_id == intent.task_id && state.instance_id == intent.instance_id
        }) {
            state.last_dispatched_unix_ms = Some(dispatched_at_unix_ms);
            state.eligible_since_unix_ms = None;
            state.terminal_state = Some(TaskTerminalState::Succeeded);
        } else {
            states.push(TaskRuntimeSnapshot {
                task_id: intent.task_id.clone(),
                instance_id: intent.instance_id.clone(),
                last_dispatched_unix_ms: Some(dispatched_at_unix_ms),
                eligible_since_unix_ms: None,
                terminal_state: Some(TaskTerminalState::Succeeded),
            });
        }
    }
    states.sort_by(|left, right| {
        left.task_id
            .cmp(&right.task_id)
            .then_with(|| left.instance_id.cmp(&right.instance_id))
    });
}

fn collect_detection_gaps(
    decisions: &[crate::TaskDecision],
    gaps: &mut BTreeMap<(String, String, String), ForwardEvidenceGap>,
) {
    for decision in decisions {
        for DetectionSuggestion {
            scope,
            fact_key,
            reason,
        } in &decision.detection_suggestions
        {
            let identity = format!("{}\u{1f}{}", decision.task_id, scope_key(scope));
            gaps.entry((identity, fact_key.clone(), reason.clone()))
                .or_insert_with(|| ForwardEvidenceGap {
                    code: reason.clone(),
                    task_id: Some(decision.task_id.clone()),
                    fact_key: Some(fact_key.clone()),
                });
        }
    }
}

fn scope_key(scope: &ScopeSelector) -> String {
    match scope {
        ScopeSelector::Instance { instance_id } => format!("instance:{instance_id}"),
        ScopeSelector::Server { server_id } => format!("server:{server_id}"),
        ScopeSelector::Game { game_id } => format!("game:{game_id}"),
    }
}

fn pool_projection(pools: &BTreeMap<String, SimulatedPool>) -> Vec<ForwardPoolState> {
    pools
        .iter()
        .map(|(pool_id, pool)| ForwardPoolState {
            pool_id: pool_id.clone(),
            value: pool.snapshot.value,
            cumulative_waste: pool.cumulative_waste,
        })
        .collect()
}

fn total_waste(pools: &BTreeMap<String, SimulatedPool>) -> ForwardResult<u64> {
    pools.values().try_fold(0_u64, |total, pool| {
        total
            .checked_add(pool.cumulative_waste)
            .ok_or_else(|| ForwardError::overflow("total projected waste overflowed"))
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaintenanceTrendPolicy {
    pub minimum_duration_samples: u16,
    pub minimum_confidence_samples: u16,
    pub lookback_ms: u64,
    pub confidence_drop_milli: u16,
    pub duration_growth_basis_points: u16,
}

impl Default for MaintenanceTrendPolicy {
    fn default() -> Self {
        Self {
            minimum_duration_samples: 4,
            minimum_confidence_samples: 4,
            lookback_ms: 7 * 24 * 60 * 60 * 1_000,
            confidence_drop_milli: 100,
            duration_growth_basis_points: 2_500,
        }
    }
}

impl MaintenanceTrendPolicy {
    pub fn validate(&self) -> ForwardResult<()> {
        if self.minimum_duration_samples < 4
            || self.minimum_confidence_samples < 4
            || usize::from(self.minimum_duration_samples) > MAX_MAINTENANCE_SAMPLES
            || usize::from(self.minimum_confidence_samples) > MAX_MAINTENANCE_SAMPLES
            || self.lookback_ms == 0
            || self.confidence_drop_milli == 0
            || self.confidence_drop_milli > 1_000
            || self.duration_growth_basis_points == 0
        {
            return Err(ForwardError::invalid("maintenance trend policy is invalid"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DurationEvidence {
    pub ledger_sequence: u64,
    pub observed_at_unix_ms: u64,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfidenceEvidence {
    pub ledger_sequence: u64,
    pub observed_at_unix_ms: u64,
    pub confidence_milli: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaintenanceEvidence {
    pub subject_id: String,
    pub as_of_unix_ms: u64,
    pub durations: Vec<DurationEvidence>,
    pub confidences: Vec<ConfidenceEvidence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceDisposition {
    Healthy,
    RecheckSuggested,
    EvidenceInsufficient,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaintenanceAssessment {
    pub assessment_id: String,
    pub subject_id: String,
    pub as_of_unix_ms: u64,
    pub disposition: MaintenanceDisposition,
    pub duration_sample_count: u16,
    pub confidence_sample_count: u16,
    pub first_ledger_sequence: Option<u64>,
    pub last_ledger_sequence: Option<u64>,
    pub earlier_duration_median_ms: Option<u64>,
    pub recent_duration_median_ms: Option<u64>,
    pub duration_growth_basis_points: Option<u64>,
    pub earlier_confidence_median_milli: Option<u16>,
    pub recent_confidence_median_milli: Option<u16>,
    pub confidence_drop_milli: Option<u16>,
    pub reasons: Vec<String>,
}

impl MaintenanceAssessment {
    pub const fn recheck_suggested(&self) -> bool {
        matches!(self.disposition, MaintenanceDisposition::RecheckSuggested)
    }
}

/// Assesses trends only from ledger-pinned samples inside the declared lookback window.
pub fn assess_predictive_maintenance(
    evidence: &MaintenanceEvidence,
    policy: MaintenanceTrendPolicy,
) -> ForwardResult<MaintenanceAssessment> {
    policy.validate()?;
    validate_subject(&evidence.subject_id)?;
    if evidence.as_of_unix_ms == 0
        || evidence.durations.len() > MAX_MAINTENANCE_SAMPLES
        || evidence.confidences.len() > MAX_MAINTENANCE_SAMPLES
    {
        return Err(ForwardError::invalid("maintenance evidence is invalid"));
    }
    let window_start = evidence.as_of_unix_ms.saturating_sub(policy.lookback_ms);
    let durations = bounded_durations(&evidence.durations, window_start, evidence.as_of_unix_ms)?;
    let confidences =
        bounded_confidences(&evidence.confidences, window_start, evidence.as_of_unix_ms)?;
    validate_combined_evidence_identity(&durations, &confidences)?;
    let duration_count = u16::try_from(durations.len())
        .map_err(|_| ForwardError::overflow("duration evidence count overflowed"))?;
    let confidence_count = u16::try_from(confidences.len())
        .map_err(|_| ForwardError::overflow("confidence evidence count overflowed"))?;
    let sequence_bounds = evidence_sequence_bounds(&durations, &confidences);
    let assessment_id = maintenance_assessment_id(evidence, &durations, &confidences, policy);
    let mut assessment = MaintenanceAssessment {
        assessment_id,
        subject_id: evidence.subject_id.clone(),
        as_of_unix_ms: evidence.as_of_unix_ms,
        disposition: MaintenanceDisposition::EvidenceInsufficient,
        duration_sample_count: duration_count,
        confidence_sample_count: confidence_count,
        first_ledger_sequence: sequence_bounds.map(|bounds| bounds.0),
        last_ledger_sequence: sequence_bounds.map(|bounds| bounds.1),
        earlier_duration_median_ms: None,
        recent_duration_median_ms: None,
        duration_growth_basis_points: None,
        earlier_confidence_median_milli: None,
        recent_confidence_median_milli: None,
        confidence_drop_milli: None,
        reasons: Vec::new(),
    };
    if duration_count < policy.minimum_duration_samples {
        assessment
            .reasons
            .push("duration_evidence_insufficient".to_owned());
    }
    if confidence_count < policy.minimum_confidence_samples {
        assessment
            .reasons
            .push("confidence_evidence_insufficient".to_owned());
    }
    if !assessment.reasons.is_empty() {
        return Ok(assessment);
    }
    let (earlier_duration, recent_duration) = split_duration_medians(&durations)?;
    let (earlier_confidence, recent_confidence) = split_confidence_medians(&confidences)?;
    let duration_growth = duration_growth_basis_points(earlier_duration, recent_duration)?;
    let confidence_drop = earlier_confidence.saturating_sub(recent_confidence);
    assessment.earlier_duration_median_ms = Some(earlier_duration);
    assessment.recent_duration_median_ms = Some(recent_duration);
    assessment.duration_growth_basis_points = Some(duration_growth);
    assessment.earlier_confidence_median_milli = Some(earlier_confidence);
    assessment.recent_confidence_median_milli = Some(recent_confidence);
    assessment.confidence_drop_milli = Some(confidence_drop);
    if confidence_drop >= policy.confidence_drop_milli
        || duration_growth >= u64::from(policy.duration_growth_basis_points)
    {
        assessment.disposition = MaintenanceDisposition::RecheckSuggested;
        if confidence_drop >= policy.confidence_drop_milli {
            assessment
                .reasons
                .push("confidence_trend_degraded".to_owned());
        }
        if duration_growth >= u64::from(policy.duration_growth_basis_points) {
            assessment
                .reasons
                .push("duration_trend_degraded".to_owned());
        }
    } else {
        assessment.disposition = MaintenanceDisposition::Healthy;
        assessment.reasons.push("trend_within_bounds".to_owned());
    }
    Ok(assessment)
}

fn bounded_durations(
    samples: &[DurationEvidence],
    window_start: u64,
    as_of: u64,
) -> ForwardResult<Vec<DurationEvidence>> {
    let mut selected = samples
        .iter()
        .copied()
        .filter(|sample| sample.observed_at_unix_ms >= window_start)
        .collect::<Vec<_>>();
    selected.sort_by_key(|sample| (sample.observed_at_unix_ms, sample.ledger_sequence));
    validate_evidence_order(
        selected
            .iter()
            .map(|sample| (sample.ledger_sequence, sample.observed_at_unix_ms)),
        as_of,
    )?;
    if selected.iter().any(|sample| sample.duration_ms == 0) {
        return Err(ForwardError::invalid("zero duration evidence is invalid"));
    }
    Ok(selected)
}

fn bounded_confidences(
    samples: &[ConfidenceEvidence],
    window_start: u64,
    as_of: u64,
) -> ForwardResult<Vec<ConfidenceEvidence>> {
    let mut selected = samples
        .iter()
        .copied()
        .filter(|sample| sample.observed_at_unix_ms >= window_start)
        .collect::<Vec<_>>();
    selected.sort_by_key(|sample| (sample.observed_at_unix_ms, sample.ledger_sequence));
    validate_evidence_order(
        selected
            .iter()
            .map(|sample| (sample.ledger_sequence, sample.observed_at_unix_ms)),
        as_of,
    )?;
    if selected
        .iter()
        .any(|sample| sample.confidence_milli > 1_000)
    {
        return Err(ForwardError::invalid(
            "confidence evidence exceeds the closed range",
        ));
    }
    Ok(selected)
}

fn validate_evidence_order(
    samples: impl Iterator<Item = (u64, u64)>,
    as_of: u64,
) -> ForwardResult<()> {
    let mut sequences = BTreeSet::new();
    for (sequence, timestamp) in samples {
        if sequence == 0 || timestamp == 0 || timestamp > as_of || !sequences.insert(sequence) {
            return Err(ForwardError::invalid(
                "maintenance evidence identity or time is invalid",
            ));
        }
    }
    Ok(())
}

fn validate_combined_evidence_identity(
    durations: &[DurationEvidence],
    confidences: &[ConfidenceEvidence],
) -> ForwardResult<()> {
    let mut sequences = BTreeSet::new();
    if durations
        .iter()
        .map(|sample| sample.ledger_sequence)
        .chain(confidences.iter().map(|sample| sample.ledger_sequence))
        .any(|sequence| !sequences.insert(sequence))
    {
        return Err(ForwardError::invalid(
            "maintenance evidence reuses a ledger sequence",
        ));
    }
    Ok(())
}

fn evidence_sequence_bounds(
    durations: &[DurationEvidence],
    confidences: &[ConfidenceEvidence],
) -> Option<(u64, u64)> {
    durations
        .iter()
        .map(|sample| sample.ledger_sequence)
        .chain(confidences.iter().map(|sample| sample.ledger_sequence))
        .fold(None, |bounds, sequence| match bounds {
            None => Some((sequence, sequence)),
            Some((minimum, maximum)) => Some((minimum.min(sequence), maximum.max(sequence))),
        })
}

fn split_duration_medians(samples: &[DurationEvidence]) -> ForwardResult<(u64, u64)> {
    let split = samples.len() / 2;
    Ok((
        median_u64(samples[..split].iter().map(|sample| sample.duration_ms))?,
        median_u64(samples[split..].iter().map(|sample| sample.duration_ms))?,
    ))
}

fn split_confidence_medians(samples: &[ConfidenceEvidence]) -> ForwardResult<(u16, u16)> {
    let split = samples.len() / 2;
    let earlier = median_u64(
        samples[..split]
            .iter()
            .map(|sample| u64::from(sample.confidence_milli)),
    )?;
    let recent = median_u64(
        samples[split..]
            .iter()
            .map(|sample| u64::from(sample.confidence_milli)),
    )?;
    Ok((
        u16::try_from(earlier)
            .map_err(|_| ForwardError::overflow("confidence median overflowed"))?,
        u16::try_from(recent)
            .map_err(|_| ForwardError::overflow("confidence median overflowed"))?,
    ))
}

fn median_u64(values: impl Iterator<Item = u64>) -> ForwardResult<u64> {
    let mut values = values.collect::<Vec<_>>();
    if values.is_empty() {
        return Err(ForwardError::invalid("median requires evidence"));
    }
    values.sort_unstable();
    let middle = values.len() / 2;
    if values.len() % 2 == 1 {
        Ok(values[middle])
    } else {
        let lower = values[middle - 1];
        let upper = values[middle];
        Ok(lower + (upper - lower) / 2)
    }
}

fn duration_growth_basis_points(earlier: u64, recent: u64) -> ForwardResult<u64> {
    if recent <= earlier {
        return Ok(0);
    }
    let increase = recent - earlier;
    increase
        .checked_mul(10_000)
        .map(|scaled| scaled / earlier)
        .ok_or_else(|| ForwardError::overflow("duration trend overflowed"))
}

fn maintenance_assessment_id(
    evidence: &MaintenanceEvidence,
    durations: &[DurationEvidence],
    confidences: &[ConfidenceEvidence],
    policy: MaintenanceTrendPolicy,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"actingcommand-maintenance-assessment-v1");
    hasher.update((evidence.subject_id.len() as u64).to_be_bytes());
    hasher.update(evidence.subject_id.as_bytes());
    hasher.update(policy.minimum_duration_samples.to_be_bytes());
    hasher.update(policy.minimum_confidence_samples.to_be_bytes());
    hasher.update(policy.lookback_ms.to_be_bytes());
    hasher.update(policy.confidence_drop_milli.to_be_bytes());
    hasher.update(policy.duration_growth_basis_points.to_be_bytes());
    hasher.update((durations.len() as u64).to_be_bytes());
    for sample in durations {
        hasher.update(sample.ledger_sequence.to_be_bytes());
        hasher.update(sample.observed_at_unix_ms.to_be_bytes());
        hasher.update(sample.duration_ms.to_be_bytes());
    }
    hasher.update((confidences.len() as u64).to_be_bytes());
    for sample in confidences {
        hasher.update(sample.ledger_sequence.to_be_bytes());
        hasher.update(sample.observed_at_unix_ms.to_be_bytes());
        hasher.update(sample.confidence_milli.to_be_bytes());
    }
    format!("maintenance:{:x}", hasher.finalize())
}

fn validate_subject(value: &str) -> ForwardResult<()> {
    if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
        return Err(ForwardError::invalid("maintenance subject is invalid"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CatalogDocumentSource, CatalogSources, FactValue, InstanceSnapshot, ObservedOutcome,
        PoolValueSnapshot, compile_catalog,
    };

    const NOW: u64 = 1_699_963_200_000;

    fn catalog() -> CompiledCatalog {
        compile_catalog(&CatalogSources {
            tasks: CatalogDocumentSource::new(
                "memory://neutral/tasks.json",
                include_bytes!("../../../contracts/scheduling/examples/catalog-a/tasks.json")
                    .to_vec(),
            ),
            pools: CatalogDocumentSource::new(
                "memory://neutral/pools.json",
                include_bytes!("../../../contracts/scheduling/examples/catalog-a/pools.json")
                    .to_vec(),
            ),
            activity: CatalogDocumentSource::new(
                "memory://neutral/activity.json",
                include_bytes!("../../../contracts/scheduling/examples/catalog-a/activity.json")
                    .to_vec(),
            ),
            timeline: CatalogDocumentSource::new(
                "memory://neutral/timeline.json",
                include_bytes!("../../../contracts/scheduling/examples/catalog-a/timeline.json")
                    .to_vec(),
            ),
        })
        .expect("compiled catalog")
    }

    fn facts(include_outcome: bool) -> EvaluationFacts {
        EvaluationFacts {
            ledger_position: 7,
            fact_snapshot_id: "snapshot:forward-neutral".to_owned(),
            facts: Vec::new(),
            outcomes: include_outcome
                .then(|| ObservedOutcome {
                    task_id: "fixture.observe".to_owned(),
                    instance_id: "fixture-instance-a".to_owned(),
                    outcome_key: "completed".to_owned(),
                    value: FactValue::Boolean(false),
                    observed_at_unix_ms: NOW,
                })
                .into_iter()
                .collect(),
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

    fn resources(value: u64) -> EvaluationResources {
        EvaluationResources {
            pools: vec![PoolValueSnapshot {
                pool_id: "fixture-pool-a".to_owned(),
                value,
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
            }],
        }
    }

    #[test]
    fn forward_projection_reuses_evaluator_and_is_deterministic() {
        let catalog = catalog();
        let config = ForwardProjectionConfig::for_hours(2, 64).expect("config");
        let first = project_forward(
            &catalog,
            &facts(true),
            &resources(119),
            EvaluationTime { unix_ms: NOW },
            11,
            config,
        )
        .expect("projection");
        let second = project_forward(
            &catalog,
            &facts(true),
            &resources(119),
            EvaluationTime { unix_ms: NOW },
            11,
            config,
        )
        .expect("projection");
        assert_eq!(first, second);
        assert!(!first.steps.is_empty());
        assert!(first.cumulative_waste > 0);
        assert_eq!(first.catalog_hash, catalog.catalog_hash());
    }

    #[test]
    fn missing_observation_reports_insufficient_evidence_without_fake_dispatch() {
        let projection = project_forward(
            &catalog(),
            &facts(false),
            &resources(10),
            EvaluationTime {
                unix_ms: NOW + 60_000,
            },
            5,
            ForwardProjectionConfig::for_hours(1, 32).expect("config"),
        )
        .expect("projection");
        assert_eq!(
            projection.completeness,
            ForwardProjectionCompleteness::EvidenceInsufficient
        );
        assert!(projection.steps.is_empty());
        assert!(
            projection.evidence_gaps.iter().any(|gap| {
                gap.task_id.as_deref() == Some("fixture.observe")
                    && gap.fact_key.as_deref() == Some("outcome.fixture.observe.completed")
            }),
            "{:?}",
            projection.evidence_gaps
        );
    }

    #[test]
    fn exhausted_evaluation_budget_is_not_reported_as_a_complete_projection() {
        let projection = project_forward(
            &catalog(),
            &facts(true),
            &resources(120),
            EvaluationTime { unix_ms: NOW + 1 },
            9,
            ForwardProjectionConfig::for_hours(2, 1).expect("config"),
        )
        .expect("projection");
        assert_eq!(
            projection.completeness,
            ForwardProjectionCompleteness::EvidenceInsufficient
        );
        assert!(
            projection
                .evidence_gaps
                .iter()
                .any(|gap| gap.code == "step_budget_exhausted")
        );
    }

    fn maintenance_evidence(degraded: bool) -> MaintenanceEvidence {
        let durations = if degraded {
            [100, 110, 200, 240]
        } else {
            [100, 110, 112, 115]
        }
        .into_iter()
        .enumerate()
        .map(|(index, duration_ms)| DurationEvidence {
            ledger_sequence: (index + 1) as u64,
            observed_at_unix_ms: NOW - 4_000 + index as u64 * 1_000,
            duration_ms,
        })
        .collect();
        let confidences = if degraded {
            [950, 940, 800, 780]
        } else {
            [950, 940, 935, 930]
        }
        .into_iter()
        .enumerate()
        .map(|(index, confidence_milli)| ConfidenceEvidence {
            ledger_sequence: (index + 10) as u64,
            observed_at_unix_ms: NOW - 4_000 + index as u64 * 1_000,
            confidence_milli,
        })
        .collect();
        MaintenanceEvidence {
            subject_id: "maintenance:neutral".to_owned(),
            as_of_unix_ms: NOW,
            durations,
            confidences,
        }
    }

    #[test]
    fn maintenance_requires_both_evidence_series() {
        let mut evidence = maintenance_evidence(true);
        evidence.confidences.clear();
        let assessment =
            assess_predictive_maintenance(&evidence, MaintenanceTrendPolicy::default())
                .expect("assessment");
        assert_eq!(
            assessment.disposition,
            MaintenanceDisposition::EvidenceInsufficient
        );
        assert!(!assessment.recheck_suggested());
    }

    #[test]
    fn maintenance_rejects_reused_ledger_sequence_identity() {
        let mut evidence = maintenance_evidence(true);
        evidence.confidences[0].ledger_sequence = evidence.durations[0].ledger_sequence;
        let error = assess_predictive_maintenance(&evidence, MaintenanceTrendPolicy::default())
            .expect_err("one ledger event cannot identify two evidence samples");
        assert_eq!(error.code(), "forward_projection_invalid");
    }

    #[test]
    fn maintenance_suggestion_is_evidence_pinned_and_deterministic() {
        let evidence = maintenance_evidence(true);
        let first = assess_predictive_maintenance(&evidence, MaintenanceTrendPolicy::default())
            .expect("assessment");
        let second = assess_predictive_maintenance(&evidence, MaintenanceTrendPolicy::default())
            .expect("assessment");
        assert_eq!(first, second);
        assert_eq!(first.disposition, MaintenanceDisposition::RecheckSuggested);
        assert_eq!(first.first_ledger_sequence, Some(1));
        assert_eq!(first.last_ledger_sequence, Some(13));
    }

    #[test]
    fn stable_trends_do_not_create_a_recheck_suggestion() {
        let assessment = assess_predictive_maintenance(
            &maintenance_evidence(false),
            MaintenanceTrendPolicy::default(),
        )
        .expect("assessment");
        assert_eq!(assessment.disposition, MaintenanceDisposition::Healthy);
        assert!(!assessment.recheck_suggested());
    }
}
