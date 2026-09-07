// SPDX-License-Identifier: AGPL-3.0-only

//! Pure strategic-report projection into existing scheduling declarations.

use crate::canonical::canonical_serialized;
use crate::evaluator::scope_matches_instance;
use crate::{
    ActivityProfile, CompiledCatalog, EvaluationFacts, EvaluationResources, FactValue, GoalTarget,
    InstanceSnapshot, LoadProfile, MetricRef, PredicateSpec, ScopeSelector, TaskSpec,
};
use actingcommand_contract::MAX_STRATEGIC_WEIGHT_MILLI;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

pub const STRATEGIC_REPORT_SCHEMA_VERSION: &str = "actingcommand.strategy-report.v1";
const MAX_REPORT_GOALS: usize = 128;
const MAX_REPORT_ASSESSMENTS: usize = 4_096;
const MAX_REPORT_TEMPLATES: usize = 512;
const MAX_REPORT_EVIDENCE: usize = 64;
const MAX_TEMPLATE_TASKS: usize = 64;
const MAX_IDENTIFIER_BYTES: usize = 128;
const MAX_URGENCY_MILLI: u32 = 1_000_000;
const RATE_PERIOD_MS: u64 = 60 * 60 * 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrategyError {
    code: &'static str,
    message: String,
}

impl StrategyError {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: "strategy_report_invalid",
            message: message.into(),
        }
    }

    fn mismatch(message: impl Into<String>) -> Self {
        Self {
            code: "strategy_catalog_mismatch",
            message: message.into(),
        }
    }

    fn ambiguous(message: impl Into<String>) -> Self {
        Self {
            code: "strategy_template_ambiguous",
            message: message.into(),
        }
    }

    fn overflow(message: impl Into<String>) -> Self {
        Self {
            code: "strategy_numeric_overflow",
            message: message.into(),
        }
    }

    pub const fn code(&self) -> &'static str {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for StrategyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl Error for StrategyError {}

pub type StrategyResult<T> = Result<T, StrategyError>;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrategicEvidencePointer {
    pub artifact_id: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategicBand {
    NoPressure,
    Actionable,
    InfeasibleBestEffort,
    NeedsDetection,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanningDisposition {
    ExecutionContinues,
    NeedsPlanning,
    NeedsDetection,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutlierMetric {
    Shortfall,
    UrgencyMilli,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutlierPolicy {
    pub metric: OutlierMetric,
    pub mad_multiplier_milli: u32,
    pub top_n: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CohortBudgets {
    pub max_active: u16,
    pub max_prompt: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrategicTemplate {
    pub template_id: String,
    pub task_template_ids: Vec<String>,
    pub activity_profile_template_id: String,
    pub eligibility: PredicateSpec,
    pub match_bands: Vec<StrategicBand>,
    pub minimum_urgency_milli: u32,
    pub maximum_urgency_milli: u32,
    pub strategic_weight_milli: u16,
    pub load_profile: LoadProfile,
    pub risk_class: String,
    pub budget_class: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrategicGoal {
    pub goal_id: String,
    pub goal_version: u64,
    pub metric: MetricRef,
    pub templates: Vec<StrategicTemplate>,
    pub outlier_policy: OutlierPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrategicInstanceAssessment {
    pub goal_id: String,
    pub instance_id: String,
    pub game_id: String,
    pub fact_snapshot_id: String,
    pub current_projection: Option<i64>,
    pub production_rate_per_hour: Option<u64>,
    pub target: i64,
    pub deadline_unix_ms: u64,
    pub available: bool,
    pub capability_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrategicReport {
    schema_version: String,
    report_id: String,
    game_id: String,
    catalog_hash: String,
    catalog_version: u64,
    target_catalog_version: u64,
    as_of_ledger_position: u64,
    as_of_unix_ms: u64,
    policy_hash: String,
    classifier_hash: String,
    evidence: Vec<StrategicEvidencePointer>,
    goals: Vec<StrategicGoal>,
    assessments: Vec<StrategicInstanceAssessment>,
    cohort_budgets: CohortBudgets,
}

impl StrategicReport {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        game_id: impl Into<String>,
        catalog_hash: impl Into<String>,
        catalog_version: u64,
        target_catalog_version: u64,
        as_of_ledger_position: u64,
        as_of_unix_ms: u64,
        policy_hash: impl Into<String>,
        classifier_hash: impl Into<String>,
        mut evidence: Vec<StrategicEvidencePointer>,
        mut goals: Vec<StrategicGoal>,
        mut assessments: Vec<StrategicInstanceAssessment>,
        cohort_budgets: CohortBudgets,
    ) -> StrategyResult<Self> {
        evidence.sort();
        for goal in &mut goals {
            for template in &mut goal.templates {
                template.task_template_ids.sort();
                template.match_bands.sort();
            }
            goal.templates
                .sort_by(|left, right| left.template_id.cmp(&right.template_id));
        }
        goals.sort_by(|left, right| left.goal_id.cmp(&right.goal_id));
        for assessment in &mut assessments {
            assessment.capability_ids.sort();
        }
        assessments.sort_by(|left, right| {
            left.goal_id
                .cmp(&right.goal_id)
                .then_with(|| left.instance_id.cmp(&right.instance_id))
        });
        let mut report = Self {
            schema_version: STRATEGIC_REPORT_SCHEMA_VERSION.to_owned(),
            report_id: String::new(),
            game_id: game_id.into(),
            catalog_hash: catalog_hash.into(),
            catalog_version,
            target_catalog_version,
            as_of_ledger_position,
            as_of_unix_ms,
            policy_hash: policy_hash.into(),
            classifier_hash: classifier_hash.into(),
            evidence,
            goals,
            assessments,
            cohort_budgets,
        };
        report.validate_components()?;
        report.report_id = report_identity(&report)?;
        report.validate()?;
        Ok(report)
    }

    pub fn validate(&self) -> StrategyResult<()> {
        if self.schema_version != STRATEGIC_REPORT_SCHEMA_VERSION {
            return Err(StrategyError::invalid("unsupported report schema"));
        }
        self.validate_components()?;
        if self.report_id != report_identity(self)? {
            return Err(StrategyError::invalid(
                "report identity does not match its content",
            ));
        }
        Ok(())
    }

    fn validate_components(&self) -> StrategyResult<()> {
        validate_identifier(&self.game_id, "game_id")?;
        validate_sha256(&self.catalog_hash, "catalog_hash")?;
        validate_sha256(&self.policy_hash, "policy_hash")?;
        validate_sha256(&self.classifier_hash, "classifier_hash")?;
        if self.catalog_version == 0
            || self.target_catalog_version <= self.catalog_version
            || self.as_of_ledger_position == 0
            || self.as_of_unix_ms == 0
            || self.evidence.is_empty()
            || self.evidence.len() > MAX_REPORT_EVIDENCE
            || self.goals.is_empty()
            || self.goals.len() > MAX_REPORT_GOALS
            || self.assessments.is_empty()
            || self.assessments.len() > MAX_REPORT_ASSESSMENTS
            || self.cohort_budgets.max_active == 0
            || self.cohort_budgets.max_prompt == 0
            || self.cohort_budgets.max_prompt > self.cohort_budgets.max_active
        {
            return Err(StrategyError::invalid("report boundary is invalid"));
        }
        let mut previous_evidence = None;
        for pointer in &self.evidence {
            validate_identifier(&pointer.artifact_id, "artifact_id")?;
            validate_sha256(&pointer.sha256, "evidence_sha256")?;
            if previous_evidence.is_some_and(|value: &StrategicEvidencePointer| value >= pointer) {
                return Err(StrategyError::invalid(
                    "evidence pointers must be unique and canonical",
                ));
            }
            previous_evidence = Some(pointer);
        }
        let mut goals = BTreeMap::new();
        let mut template_count = 0_usize;
        let mut previous_goal_id = None::<&str>;
        for goal in &self.goals {
            validate_identifier(&goal.goal_id, "goal_id")?;
            if goal.goal_version == 0
                || previous_goal_id.is_some_and(|value| value >= goal.goal_id.as_str())
                || goals.insert(goal.goal_id.as_str(), goal).is_some()
            {
                return Err(StrategyError::invalid("goal identity is invalid"));
            }
            previous_goal_id = Some(&goal.goal_id);
            validate_outlier_policy(&goal.outlier_policy)?;
            if goal.templates.is_empty() {
                return Err(StrategyError::invalid("goal has no conditional templates"));
            }
            template_count = template_count
                .checked_add(goal.templates.len())
                .ok_or_else(|| StrategyError::overflow("template count overflow"))?;
            let mut template_ids = BTreeSet::new();
            let mut previous_template_id = None::<&str>;
            for template in &goal.templates {
                validate_template(template)?;
                if previous_template_id.is_some_and(|value| value >= template.template_id.as_str())
                    || !template_ids.insert(template.template_id.as_str())
                {
                    return Err(StrategyError::invalid("duplicate template identity"));
                }
                previous_template_id = Some(&template.template_id);
            }
        }
        if template_count > MAX_REPORT_TEMPLATES {
            return Err(StrategyError::invalid(
                "template count exceeds the report budget",
            ));
        }
        let mut assessment_ids = BTreeSet::new();
        let mut assessed_goals = BTreeSet::new();
        let mut previous_assessment = None::<(&str, &str)>;
        for assessment in &self.assessments {
            validate_assessment(assessment, &self.game_id)?;
            let identity = (assessment.goal_id.as_str(), assessment.instance_id.as_str());
            if previous_assessment.is_some_and(|value| value >= identity)
                || !goals.contains_key(assessment.goal_id.as_str())
                || !assessment_ids
                    .insert((assessment.goal_id.as_str(), assessment.instance_id.as_str()))
            {
                return Err(StrategyError::invalid(
                    "assessment goal or instance identity is invalid",
                ));
            }
            previous_assessment = Some(identity);
            assessed_goals.insert(assessment.goal_id.as_str());
        }
        if assessed_goals.len() != goals.len() {
            return Err(StrategyError::invalid("every goal requires an assessment"));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> StrategyResult<Vec<u8>> {
        self.validate()?;
        canonical_serialized(self)
            .map_err(|error| StrategyError::invalid(format!("canonical report failed: {error}")))
    }

    pub fn report_id(&self) -> &str {
        &self.report_id
    }

    pub fn game_id(&self) -> &str {
        &self.game_id
    }

    pub fn catalog_hash(&self) -> &str {
        &self.catalog_hash
    }

    pub const fn catalog_version(&self) -> u64 {
        self.catalog_version
    }

    pub const fn target_catalog_version(&self) -> u64 {
        self.target_catalog_version
    }

    pub const fn as_of_ledger_position(&self) -> u64 {
        self.as_of_ledger_position
    }

    pub const fn as_of_unix_ms(&self) -> u64 {
        self.as_of_unix_ms
    }

    pub fn policy_hash(&self) -> &str {
        &self.policy_hash
    }

    pub fn classifier_hash(&self) -> &str {
        &self.classifier_hash
    }

    pub fn evidence(&self) -> &[StrategicEvidencePointer] {
        &self.evidence
    }

    pub fn goals(&self) -> &[StrategicGoal] {
        &self.goals
    }

    pub fn assessments(&self) -> &[StrategicInstanceAssessment] {
        &self.assessments
    }

    pub const fn cohort_budgets(&self) -> &CohortBudgets {
        &self.cohort_budgets
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrategicInstanceProjection {
    pub goal_id: String,
    pub goal_version: u64,
    pub instance_id: String,
    pub fact_snapshot_id: String,
    pub shortfall: Option<u64>,
    pub capacity: Option<u64>,
    pub urgency_milli: Option<u32>,
    pub band: StrategicBand,
    pub planning_disposition: PlanningDisposition,
    pub template_id: Option<String>,
    pub decision_signature: String,
    pub deadline_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrategicPlanningLaneEntry {
    pub goal_id: String,
    pub instance_id: String,
    pub fact_snapshot_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrategicOutlier {
    pub goal_id: String,
    pub instance_id: String,
    pub metric: OutlierMetric,
    pub value: u64,
    pub median: u64,
    pub mad: u64,
    pub absolute_deviation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CohortAggregateStats {
    pub member_count: u32,
    pub total_shortfall: u64,
    pub minimum_urgency_milli: Option<u32>,
    pub median_urgency_milli: Option<u32>,
    pub maximum_urgency_milli: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CohortProjection {
    pub cohort_id: String,
    pub goal_id: String,
    pub goal_version: u64,
    pub policy_hash: String,
    pub classifier_hash: String,
    pub decision_signature: String,
    pub result_class: StrategicBand,
    pub member_instance_ids: Vec<String>,
    pub member_fact_snapshot_ids: Vec<String>,
    pub aggregate_stats: CohortAggregateStats,
    pub boundary_instance_refs: Vec<String>,
    pub created_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrategyCatalogAdditions {
    pub tasks: Vec<TaskSpec>,
    pub activity_profiles: Vec<ActivityProfile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrategicProjection {
    pub report_id: String,
    pub game_id: String,
    pub catalog_hash: String,
    pub catalog_version: u64,
    pub target_catalog_version: u64,
    pub instances: Vec<StrategicInstanceProjection>,
    pub cohorts: Vec<CohortProjection>,
    pub active_cohort_ids: Vec<String>,
    pub prompt_cohort_ids: Vec<String>,
    pub deferred_cohort_ids: Vec<String>,
    pub outliers: Vec<StrategicOutlier>,
    pub planning_lane: Vec<StrategicPlanningLaneEntry>,
    pub additions: StrategyCatalogAdditions,
}

/// Projects one immutable, single-game report without reading clocks, storage, or mutable state.
pub fn project_strategic_report(
    catalog: &CompiledCatalog,
    report: &StrategicReport,
    facts: &EvaluationFacts,
    resources: &EvaluationResources,
) -> StrategyResult<StrategicProjection> {
    report.validate()?;
    if report.catalog_hash() != catalog.catalog_hash()
        || report.catalog_version() != catalog.summary().catalog_version
    {
        return Err(StrategyError::mismatch(
            "report does not target the supplied catalog generation",
        ));
    }
    if report.as_of_ledger_position() != facts.ledger_position {
        return Err(StrategyError::invalid(
            "report ledger position does not match the frozen facts",
        ));
    }
    let goals = report
        .goals()
        .iter()
        .map(|goal| (goal.goal_id.as_str(), goal))
        .collect::<BTreeMap<_, _>>();
    let mut projections = Vec::with_capacity(report.assessments().len());
    let mut planning_lane = Vec::new();
    let mut additions = StrategyCatalogAdditions {
        tasks: Vec::new(),
        activity_profiles: Vec::new(),
    };
    for assessment in report.assessments() {
        let goal = goals
            .get(assessment.goal_id.as_str())
            .ok_or_else(|| StrategyError::invalid("assessment references an unknown goal"))?;
        if assessment.fact_snapshot_id != facts.fact_snapshot_id {
            return Err(StrategyError::invalid(
                "assessment fact snapshot does not match the frozen facts",
            ));
        }
        let instance = resolve_assessment_instance(assessment, facts)?;
        if instance.game_id != report.game_id() {
            return Err(StrategyError::invalid(
                "assessment instance is outside the report game",
            ));
        }
        let current = resolve_metric_current(
            &goal.metric,
            assessment,
            instance,
            facts,
            resources,
            report.as_of_unix_ms(),
        )?;
        let production_rate = catalog_production_rate(catalog, &goal.metric, instance)?;
        require_optional_check("current projection", assessment.current_projection, current)?;
        require_optional_check(
            "production rate",
            assessment.production_rate_per_hour,
            production_rate,
        )?;
        let calculated =
            calculate_assessment(report.as_of_unix_ms(), assessment, current, production_rate)?;
        let template = select_template(goal, &calculated)?;
        let disposition = match calculated.band {
            StrategicBand::NeedsDetection => PlanningDisposition::NeedsDetection,
            StrategicBand::Blocked => PlanningDisposition::Blocked,
            StrategicBand::NoPressure => PlanningDisposition::ExecutionContinues,
            StrategicBand::Actionable | StrategicBand::InfeasibleBestEffort => {
                if template.is_some() {
                    PlanningDisposition::ExecutionContinues
                } else {
                    PlanningDisposition::NeedsPlanning
                }
            }
        };
        if disposition != PlanningDisposition::ExecutionContinues {
            planning_lane.push(StrategicPlanningLaneEntry {
                goal_id: goal.goal_id.clone(),
                instance_id: assessment.instance_id.clone(),
                fact_snapshot_id: assessment.fact_snapshot_id.clone(),
                reason: planning_reason(disposition).to_owned(),
            });
        }
        let signature = decision_signature(report, goal, assessment, &calculated, template)?;
        if let Some(template) = template
            && matches!(
                calculated.band,
                StrategicBand::Actionable | StrategicBand::InfeasibleBestEffort
            )
        {
            let generated = instantiate_catalog_declarations(
                catalog,
                report,
                goal,
                assessment,
                &calculated,
                template,
            )?;
            additions.tasks.extend(generated.tasks);
            for profile in generated.activity_profiles {
                merge_activity_profile(&mut additions.activity_profiles, profile)?;
            }
        }
        projections.push(StrategicInstanceProjection {
            goal_id: goal.goal_id.clone(),
            goal_version: goal.goal_version,
            instance_id: assessment.instance_id.clone(),
            fact_snapshot_id: assessment.fact_snapshot_id.clone(),
            shortfall: calculated.shortfall,
            capacity: calculated.capacity,
            urgency_milli: calculated.urgency_milli,
            band: calculated.band,
            planning_disposition: disposition,
            template_id: template.map(|value| value.template_id.clone()),
            decision_signature: signature,
            deadline_unix_ms: assessment.deadline_unix_ms,
        });
    }
    projections.sort_by(|left, right| {
        left.goal_id
            .cmp(&right.goal_id)
            .then_with(|| left.instance_id.cmp(&right.instance_id))
    });
    additions
        .tasks
        .sort_by(|left, right| left.id.cmp(&right.id));
    additions
        .activity_profiles
        .sort_by(|left, right| left.id.cmp(&right.id));
    for profile in &mut additions.activity_profiles {
        profile.goals.sort_by(|left, right| left.id.cmp(&right.id));
    }
    ensure_generated_ids_unique(&additions)?;
    let outliers = detect_outliers(&projections, &goals)?;
    let mut cohorts = form_cohorts(report, &projections)?;
    cohorts.sort_by(cohort_priority_order);
    let active_count = usize::from(report.cohort_budgets().max_active).min(cohorts.len());
    let prompt_count = usize::from(report.cohort_budgets().max_prompt).min(active_count);
    let active_cohort_ids = cohorts[..active_count]
        .iter()
        .map(|cohort| cohort.cohort_id.clone())
        .collect();
    let prompt_cohort_ids = cohorts[..prompt_count]
        .iter()
        .map(|cohort| cohort.cohort_id.clone())
        .collect();
    let deferred_cohort_ids = cohorts[active_count..]
        .iter()
        .map(|cohort| cohort.cohort_id.clone())
        .collect();
    cohorts.sort_by(|left, right| left.cohort_id.cmp(&right.cohort_id));
    planning_lane.sort_by(|left, right| {
        left.goal_id
            .cmp(&right.goal_id)
            .then_with(|| left.instance_id.cmp(&right.instance_id))
    });
    Ok(StrategicProjection {
        report_id: report.report_id().to_owned(),
        game_id: report.game_id().to_owned(),
        catalog_hash: report.catalog_hash().to_owned(),
        catalog_version: report.catalog_version(),
        target_catalog_version: report.target_catalog_version(),
        instances: projections,
        cohorts,
        active_cohort_ids,
        prompt_cohort_ids,
        deferred_cohort_ids,
        outliers,
        planning_lane,
        additions,
    })
}

#[derive(Clone, Copy)]
struct CalculatedAssessment {
    shortfall: Option<u64>,
    capacity: Option<u64>,
    urgency_milli: Option<u32>,
    band: StrategicBand,
}

fn calculate_assessment(
    as_of_unix_ms: u64,
    assessment: &StrategicInstanceAssessment,
    current: Option<i64>,
    production_rate: Option<u64>,
) -> StrategyResult<CalculatedAssessment> {
    if !assessment.available {
        return Ok(CalculatedAssessment {
            shortfall: None,
            capacity: None,
            urgency_milli: None,
            band: StrategicBand::Blocked,
        });
    }
    let Some(current) = current else {
        return Ok(CalculatedAssessment {
            shortfall: None,
            capacity: None,
            urgency_milli: None,
            band: StrategicBand::NeedsDetection,
        });
    };
    let shortfall_i128 = i128::from(assessment.target) - i128::from(current);
    if shortfall_i128 <= 0 {
        return Ok(CalculatedAssessment {
            shortfall: Some(0),
            capacity: Some(0),
            urgency_milli: Some(0),
            band: StrategicBand::NoPressure,
        });
    }
    let shortfall = u64::try_from(shortfall_i128)
        .map_err(|_| StrategyError::overflow("shortfall exceeds u64"))?;
    let Some(rate) = production_rate else {
        return Ok(CalculatedAssessment {
            shortfall: Some(shortfall),
            capacity: None,
            urgency_milli: None,
            band: StrategicBand::NeedsDetection,
        });
    };
    let remaining_ms = assessment.deadline_unix_ms.saturating_sub(as_of_unix_ms);
    let capacity = u128::from(rate)
        .checked_mul(u128::from(remaining_ms))
        .ok_or_else(|| StrategyError::overflow("capacity multiplication overflow"))?
        / u128::from(RATE_PERIOD_MS);
    let capacity =
        u64::try_from(capacity).map_err(|_| StrategyError::overflow("capacity exceeds u64"))?;
    let urgency_milli = if capacity == 0 {
        MAX_URGENCY_MILLI
    } else {
        let ratio = u128::from(shortfall)
            .checked_mul(1_000)
            .ok_or_else(|| StrategyError::overflow("urgency multiplication overflow"))?
            / u128::from(capacity);
        u32::try_from(ratio.min(u128::from(MAX_URGENCY_MILLI)))
            .map_err(|_| StrategyError::overflow("urgency exceeds u32"))?
    };
    Ok(CalculatedAssessment {
        shortfall: Some(shortfall),
        capacity: Some(capacity),
        urgency_milli: Some(urgency_milli),
        band: if shortfall <= capacity {
            StrategicBand::Actionable
        } else {
            StrategicBand::InfeasibleBestEffort
        },
    })
}

fn resolve_assessment_instance<'a>(
    assessment: &StrategicInstanceAssessment,
    facts: &'a EvaluationFacts,
) -> StrategyResult<&'a InstanceSnapshot> {
    let mut matches = facts
        .instances
        .iter()
        .filter(|instance| instance.instance_id == assessment.instance_id);
    let instance = matches
        .next()
        .ok_or_else(|| StrategyError::invalid("assessment instance is missing"))?;
    if matches.next().is_some() {
        return Err(StrategyError::invalid("assessment instance is duplicated"));
    }
    Ok(instance)
}

fn resolve_metric_current(
    metric: &MetricRef,
    assessment: &StrategicInstanceAssessment,
    instance: &InstanceSnapshot,
    facts: &EvaluationFacts,
    resources: &EvaluationResources,
    as_of_unix_ms: u64,
) -> StrategyResult<Option<i64>> {
    match metric {
        MetricRef::Fact { fact_key } => {
            let mut matches = facts.facts.iter().filter(|fact| {
                fact.fact_key == *fact_key && scope_matches_instance(&fact.scope, instance)
            });
            let Some(fact) = matches.next() else {
                return Ok(None);
            };
            if matches.next().is_some() {
                return Err(StrategyError::invalid("strategic fact input is duplicated"));
            }
            if fact
                .expires_at_unix_ms
                .is_some_and(|expires| as_of_unix_ms > expires)
            {
                return Ok(None);
            }
            Ok(match &fact.value {
                FactValue::Integer(value) => Some(*value),
                _ => None,
            })
        }
        MetricRef::Pool { pool_id } => {
            let mut matches = resources
                .pools
                .iter()
                .filter(|pool| pool.pool_id == *pool_id);
            let Some(pool) = matches.next() else {
                return Ok(None);
            };
            if matches.next().is_some() {
                return Err(StrategyError::invalid("strategic pool input is duplicated"));
            }
            i64::try_from(pool.value)
                .map(Some)
                .map_err(|_| StrategyError::overflow("strategic pool value exceeds i64"))
        }
        MetricRef::Outcome {
            task_id,
            outcome_key,
        } => {
            let mut matches = facts.outcomes.iter().filter(|outcome| {
                outcome.instance_id == assessment.instance_id
                    && outcome.task_id == *task_id
                    && outcome.outcome_key == *outcome_key
            });
            let Some(outcome) = matches.next() else {
                return Ok(None);
            };
            if matches.next().is_some() {
                return Err(StrategyError::invalid(
                    "strategic outcome input is duplicated",
                ));
            }
            Ok(match &outcome.value {
                FactValue::Integer(value) => Some(*value),
                _ => None,
            })
        }
    }
}

fn catalog_production_rate(
    catalog: &CompiledCatalog,
    metric: &MetricRef,
    instance: &InstanceSnapshot,
) -> StrategyResult<Option<u64>> {
    let MetricRef::Pool { pool_id } = metric else {
        return Ok(None);
    };
    let mut total = 0_u64;
    for task in &catalog.catalog().tasks.tasks {
        if !scope_matches_instance(&task.scope, instance)
            || !instance
                .capability_operation_ids
                .iter()
                .any(|operation| operation == &task.entrypoint.operation_id)
            || task
                .instance_overrides
                .iter()
                .find(|candidate| candidate.instance_id == instance.instance_id)
                .and_then(|override_spec| override_spec.enabled.0)
                .is_some_and(|enabled| !enabled)
        {
            continue;
        }
        if !task
            .produces
            .iter()
            .any(|effect| effect.pool_id == *pool_id)
        {
            continue;
        }
        let duration_with_cooldown = task
            .expected_duration_ms
            .checked_add(task.cooldown_ms)
            .ok_or_else(|| StrategyError::overflow("task cycle addition overflow"))?;
        let task_cycle = task.next_run_clamp_ms.max(duration_with_cooldown);
        if task.expected_duration_ms == 0 || task_cycle == 0 {
            return Err(StrategyError::invalid(
                "catalog production task has a zero duration",
            ));
        }
        let executions = [
            u64::from(task.loop_budget.daily_limit),
            u64::from(task.loop_budget.window_iteration_limit),
            task.loop_budget.max_runtime_ms / task.expected_duration_ms,
            RATE_PERIOD_MS / task_cycle,
        ]
        .into_iter()
        .min()
        .expect("four production bounds are present");
        for effect in task
            .produces
            .iter()
            .filter(|effect| effect.pool_id == *pool_id)
        {
            let contribution = effect.amount.checked_mul(executions).ok_or_else(|| {
                StrategyError::overflow("catalog production contribution overflow")
            })?;
            total = total
                .checked_add(contribution)
                .ok_or_else(|| StrategyError::overflow("catalog production rate overflow"))?;
        }
    }
    Ok(Some(total))
}

fn require_optional_check<T: Copy + PartialEq>(
    label: &str,
    reported: Option<T>,
    derived: Option<T>,
) -> StrategyResult<()> {
    if reported.is_some_and(|reported| Some(reported) != derived) {
        return Err(StrategyError::invalid(format!(
            "reported {label} does not match the derived value"
        )));
    }
    Ok(())
}

fn select_template<'a>(
    goal: &'a StrategicGoal,
    calculated: &CalculatedAssessment,
) -> StrategyResult<Option<&'a StrategicTemplate>> {
    let urgency = calculated.urgency_milli.unwrap_or(0);
    let matches = goal
        .templates
        .iter()
        .filter(|template| {
            template.match_bands.contains(&calculated.band)
                && (template.minimum_urgency_milli..=template.maximum_urgency_milli)
                    .contains(&urgency)
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Ok(None),
        [template] => Ok(Some(*template)),
        _ => Err(StrategyError::ambiguous(format!(
            "goal '{}' has overlapping template conditions",
            goal.goal_id
        ))),
    }
}

fn decision_signature(
    report: &StrategicReport,
    goal: &StrategicGoal,
    assessment: &StrategicInstanceAssessment,
    calculated: &CalculatedAssessment,
    template: Option<&StrategicTemplate>,
) -> StrategyResult<String> {
    #[derive(Serialize)]
    struct Signature<'a> {
        goal_id: &'a str,
        goal_version: u64,
        policy_hash: &'a str,
        classifier_hash: &'a str,
        band: StrategicBand,
        template_id: Option<&'a str>,
        required_action_set: &'a [String],
        capability_ids: &'a [String],
        deadline_band: u8,
        risk_class: Option<&'a str>,
        budget_class: Option<&'a str>,
        load_profile: Option<&'a LoadProfile>,
    }
    let remaining = assessment
        .deadline_unix_ms
        .saturating_sub(report.as_of_unix_ms());
    let deadline_band = match remaining {
        0..=86_400_000 => 0,
        86_400_001..=259_200_000 => 1,
        259_200_001..=604_800_000 => 2,
        _ => 3,
    };
    let value = Signature {
        goal_id: &goal.goal_id,
        goal_version: goal.goal_version,
        policy_hash: report.policy_hash(),
        classifier_hash: report.classifier_hash(),
        band: calculated.band,
        template_id: template.map(|value| value.template_id.as_str()),
        required_action_set: template.map_or(&[], |value| value.task_template_ids.as_slice()),
        capability_ids: &assessment.capability_ids,
        deadline_band,
        risk_class: template.map(|value| value.risk_class.as_str()),
        budget_class: template.map(|value| value.budget_class.as_str()),
        load_profile: template.map(|value| &value.load_profile),
    };
    hash_serializable("decision", &value)
}

fn instantiate_catalog_declarations(
    catalog: &CompiledCatalog,
    report: &StrategicReport,
    goal: &StrategicGoal,
    assessment: &StrategicInstanceAssessment,
    calculated: &CalculatedAssessment,
    template: &StrategicTemplate,
) -> StrategyResult<StrategyCatalogAdditions> {
    let suffix = generated_suffix(
        report.report_id(),
        &goal.goal_id,
        &assessment.instance_id,
        &template.template_id,
    );
    let task_templates = template
        .task_template_ids
        .iter()
        .map(|task_id| {
            catalog
                .catalog()
                .tasks
                .tasks
                .iter()
                .find(|task| task.id == *task_id)
                .ok_or_else(|| {
                    StrategyError::mismatch(format!("task template '{task_id}' is unavailable"))
                })
        })
        .collect::<StrategyResult<Vec<_>>>()?;
    let activity_template = catalog
        .catalog()
        .activity
        .profiles
        .iter()
        .find(|profile| profile.id == template.activity_profile_template_id)
        .ok_or_else(|| StrategyError::mismatch("activity profile template is unavailable"))?;
    require_game_scope(
        &activity_template.scope,
        report.game_id(),
        "activity profile",
    )?;
    require_game_predicate_scopes(&template.eligibility, report.game_id())?;
    for task in &task_templates {
        require_game_scope(&task.scope, report.game_id(), "task template")?;
        require_game_predicate_scopes(&task.trigger, report.game_id())?;
        require_game_predicate_scopes(&task.feedback_stop, report.game_id())?;
        if task.load_profile != template.load_profile {
            return Err(StrategyError::mismatch(
                "template load profile does not match its task declarations",
            ));
        }
    }
    let task_ids = task_templates
        .iter()
        .enumerate()
        .map(|(index, task)| (task.id.clone(), format!("strategy.task.{suffix}.{index}")))
        .collect::<BTreeMap<_, _>>();
    let computed_weight = u32::from(template.strategic_weight_milli)
        .saturating_add(
            calculated
                .urgency_milli
                .unwrap_or(0)
                .min(u32::from(MAX_STRATEGIC_WEIGHT_MILLI)),
        )
        .min(u32::from(MAX_STRATEGIC_WEIGHT_MILLI)) as u16;
    let mut tasks = Vec::with_capacity(task_templates.len());
    for task in task_templates {
        let mut task = task.clone();
        task.id = task_ids[&task.id].clone();
        task.scope = ScopeSelector::Instance {
            instance_id: assessment.instance_id.clone(),
        };
        rewrite_predicate_task_ids(&mut task.trigger, &task_ids);
        rewrite_predicate_task_ids(&mut task.feedback_stop, &task_ids);
        task.trigger = PredicateSpec::All {
            predicates: vec![template.eligibility.clone(), task.trigger],
        };
        task.strategic_weight_milli = computed_weight;
        task.instance_overrides.clear();
        tasks.push(task);
    }
    let mut activity = activity_template.clone();
    let activity_suffix = generated_suffix(
        report.report_id(),
        "activity",
        &assessment.instance_id,
        &template.activity_profile_template_id,
    );
    activity.id = format!("strategy.profile.{activity_suffix}");
    activity.scope = ScopeSelector::Instance {
        instance_id: assessment.instance_id.clone(),
    };
    activity.importance_milli = computed_weight;
    activity.goals = vec![GoalTarget {
        id: goal.goal_id.clone(),
        metric: goal.metric.clone(),
        target: assessment.target,
        deadline_unix_ms: assessment.deadline_unix_ms,
        strategic_weight_milli: computed_weight,
        best_effort: calculated.band == StrategicBand::InfeasibleBestEffort,
    }];
    Ok(StrategyCatalogAdditions {
        tasks,
        activity_profiles: vec![activity],
    })
}

fn rewrite_predicate_task_ids(predicate: &mut PredicateSpec, mapping: &BTreeMap<String, String>) {
    match predicate {
        PredicateSpec::All { predicates } | PredicateSpec::Any { predicates } => {
            for predicate in predicates {
                rewrite_predicate_task_ids(predicate, mapping);
            }
        }
        PredicateSpec::Not { predicate } => rewrite_predicate_task_ids(predicate, mapping),
        PredicateSpec::DependencyCompleted { task_id, .. }
        | PredicateSpec::Outcome { task_id, .. } => {
            if let Some(replacement) = mapping.get(task_id) {
                *task_id = replacement.clone();
            }
        }
        PredicateSpec::Clock { .. }
        | PredicateSpec::TimelineActive { .. }
        | PredicateSpec::ResourceProjection { .. }
        | PredicateSpec::Fact { .. }
        | PredicateSpec::RecordDeadline { .. } => {}
    }
}

fn merge_activity_profile(
    profiles: &mut Vec<ActivityProfile>,
    incoming: ActivityProfile,
) -> StrategyResult<()> {
    let Some(existing) = profiles
        .iter_mut()
        .find(|profile| profile.scope == incoming.scope)
    else {
        profiles.push(incoming);
        return Ok(());
    };
    if existing.windows != incoming.windows
        || existing.daily_budget != incoming.daily_budget
        || existing.max_window_iterations != incoming.max_window_iterations
        || existing.session_max_ms != incoming.session_max_ms
        || existing.detection_budget != incoming.detection_budget
        || existing.minimum_interval_ms != incoming.minimum_interval_ms
        || existing.maximum_interval_ms != incoming.maximum_interval_ms
        || existing.seed_source != incoming.seed_source
        || existing.resample_policy != incoming.resample_policy
    {
        return Err(StrategyError::ambiguous(
            "one instance matched incompatible activity templates",
        ));
    }
    existing.importance_milli = existing.importance_milli.max(incoming.importance_milli);
    for goal in incoming.goals {
        if existing.goals.iter().any(|current| current.id == goal.id) {
            return Err(StrategyError::ambiguous(
                "one instance produced duplicate strategic goals",
            ));
        }
        existing.goals.push(goal);
    }
    Ok(())
}

fn form_cohorts(
    report: &StrategicReport,
    projections: &[StrategicInstanceProjection],
) -> StrategyResult<Vec<CohortProjection>> {
    let mut grouped = BTreeMap::<(String, u64, String), Vec<&StrategicInstanceProjection>>::new();
    for projection in projections {
        grouped
            .entry((
                projection.goal_id.clone(),
                projection.goal_version,
                projection.decision_signature.clone(),
            ))
            .or_default()
            .push(projection);
    }
    grouped
        .into_iter()
        .map(|((goal_id, goal_version, signature), mut members)| {
            members.sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
            let member_instance_ids = members
                .iter()
                .map(|value| value.instance_id.clone())
                .collect::<Vec<_>>();
            let member_fact_snapshot_ids = members
                .iter()
                .map(|value| value.fact_snapshot_id.clone())
                .collect::<Vec<_>>();
            let urgency = members
                .iter()
                .filter_map(|value| value.urgency_milli)
                .collect::<Vec<_>>();
            let boundary_instance_refs = boundary_members(&members);
            let identity = (
                &goal_id,
                goal_version,
                report.policy_hash(),
                report.classifier_hash(),
                &signature,
                &member_instance_ids,
                &member_fact_snapshot_ids,
            );
            Ok(CohortProjection {
                cohort_id: hash_serializable("cohort", &identity)?,
                goal_id,
                goal_version,
                policy_hash: report.policy_hash().to_owned(),
                classifier_hash: report.classifier_hash().to_owned(),
                decision_signature: signature,
                result_class: members[0].band,
                member_instance_ids,
                member_fact_snapshot_ids,
                aggregate_stats: CohortAggregateStats {
                    member_count: u32::try_from(members.len())
                        .map_err(|_| StrategyError::overflow("cohort size exceeds u32"))?,
                    total_shortfall: members.iter().try_fold(0_u64, |total, value| {
                        total
                            .checked_add(value.shortfall.unwrap_or(0))
                            .ok_or_else(|| StrategyError::overflow("cohort shortfall overflow"))
                    })?,
                    minimum_urgency_milli: urgency.iter().copied().min(),
                    median_urgency_milli: median_u32(&urgency),
                    maximum_urgency_milli: urgency.iter().copied().max(),
                },
                boundary_instance_refs,
                created_at_unix_ms: report.as_of_unix_ms(),
                expires_at_unix_ms: members
                    .iter()
                    .map(|value| value.deadline_unix_ms)
                    .min()
                    .unwrap_or(report.as_of_unix_ms()),
            })
        })
        .collect()
}

fn detect_outliers(
    projections: &[StrategicInstanceProjection],
    goals: &BTreeMap<&str, &StrategicGoal>,
) -> StrategyResult<Vec<StrategicOutlier>> {
    let mut result = Vec::new();
    for (goal_id, goal) in goals {
        let values = projections
            .iter()
            .filter(|projection| projection.goal_id == *goal_id)
            .filter_map(|projection| {
                let value = match goal.outlier_policy.metric {
                    OutlierMetric::Shortfall => projection.shortfall,
                    OutlierMetric::UrgencyMilli => projection.urgency_milli.map(u64::from),
                }?;
                Some((projection.instance_id.clone(), value))
            })
            .collect::<Vec<_>>();
        if values.len() < 3 {
            continue;
        }
        let median = median_u64(&values.iter().map(|(_, value)| *value).collect::<Vec<_>>())
            .ok_or_else(|| StrategyError::invalid("outlier median is unavailable"))?;
        let deviations = values
            .iter()
            .map(|(_, value)| value.abs_diff(median))
            .collect::<Vec<_>>();
        let mad = median_u64(&deviations)
            .ok_or_else(|| StrategyError::invalid("outlier MAD is unavailable"))?;
        let mut candidates = values
            .into_iter()
            .filter_map(|(instance_id, value)| {
                let deviation = value.abs_diff(median);
                let exceeds = if mad == 0 {
                    deviation > 0
                } else {
                    u128::from(deviation) * 1_000
                        > u128::from(goal.outlier_policy.mad_multiplier_milli) * u128::from(mad)
                };
                exceeds.then_some(StrategicOutlier {
                    goal_id: (*goal_id).to_owned(),
                    instance_id,
                    metric: goal.outlier_policy.metric,
                    value,
                    median,
                    mad,
                    absolute_deviation: deviation,
                })
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            right
                .absolute_deviation
                .cmp(&left.absolute_deviation)
                .then_with(|| left.instance_id.cmp(&right.instance_id))
        });
        candidates.truncate(usize::from(goal.outlier_policy.top_n));
        result.extend(candidates);
    }
    result.sort_by(|left, right| {
        left.goal_id
            .cmp(&right.goal_id)
            .then_with(|| left.instance_id.cmp(&right.instance_id))
    });
    Ok(result)
}

fn validate_template(template: &StrategicTemplate) -> StrategyResult<()> {
    validate_identifier(&template.template_id, "template_id")?;
    validate_identifier(
        &template.activity_profile_template_id,
        "activity_profile_template_id",
    )?;
    validate_identifier(&template.risk_class, "risk_class")?;
    validate_identifier(&template.budget_class, "budget_class")?;
    if template.task_template_ids.is_empty()
        || template.task_template_ids.len() > MAX_TEMPLATE_TASKS
        || template.match_bands.is_empty()
        || template.minimum_urgency_milli > template.maximum_urgency_milli
        || template.maximum_urgency_milli > MAX_URGENCY_MILLI
        || template.strategic_weight_milli > MAX_STRATEGIC_WEIGHT_MILLI
        || template.match_bands.iter().any(|band| {
            !matches!(
                band,
                StrategicBand::Actionable | StrategicBand::InfeasibleBestEffort
            )
        })
    {
        return Err(StrategyError::invalid("template boundary is invalid"));
    }
    let mut task_ids = BTreeSet::new();
    let mut previous_task_id = None::<&str>;
    for task_id in &template.task_template_ids {
        validate_identifier(task_id, "task_template_id")?;
        if previous_task_id.is_some_and(|value| value >= task_id.as_str())
            || !task_ids.insert(task_id)
        {
            return Err(StrategyError::invalid("duplicate task template"));
        }
        previous_task_id = Some(task_id);
    }
    let mut bands = BTreeSet::new();
    let mut previous_band = None;
    if template.match_bands.iter().any(|band| {
        let invalid = previous_band.is_some_and(|value| value >= *band) || !bands.insert(*band);
        previous_band = Some(*band);
        invalid
    }) {
        return Err(StrategyError::invalid("duplicate template band"));
    }
    Ok(())
}

fn validate_assessment(
    assessment: &StrategicInstanceAssessment,
    report_game_id: &str,
) -> StrategyResult<()> {
    validate_identifier(&assessment.goal_id, "assessment_goal_id")?;
    validate_identifier(&assessment.instance_id, "assessment_instance_id")?;
    validate_identifier(&assessment.game_id, "assessment_game_id")?;
    validate_identifier(&assessment.fact_snapshot_id, "fact_snapshot_id")?;
    if assessment.game_id != report_game_id || assessment.deadline_unix_ms == 0 {
        return Err(StrategyError::invalid(
            "assessment violates the single-game report boundary",
        ));
    }
    let mut capabilities = BTreeSet::new();
    let mut previous_capability = None::<&str>;
    for capability in &assessment.capability_ids {
        validate_identifier(capability, "capability_id")?;
        if previous_capability.is_some_and(|value| value >= capability.as_str())
            || !capabilities.insert(capability)
        {
            return Err(StrategyError::invalid("duplicate instance capability"));
        }
        previous_capability = Some(capability);
    }
    Ok(())
}

fn validate_outlier_policy(policy: &OutlierPolicy) -> StrategyResult<()> {
    if policy.mad_multiplier_milli == 0 || policy.top_n == 0 {
        return Err(StrategyError::invalid("outlier policy is unbounded"));
    }
    Ok(())
}

fn ensure_generated_ids_unique(additions: &StrategyCatalogAdditions) -> StrategyResult<()> {
    if additions.tasks.len() > 4_096 || additions.activity_profiles.len() > 1_024 {
        return Err(StrategyError::invalid(
            "generated declarations exceed catalog limits",
        ));
    }
    let mut task_ids = BTreeSet::new();
    if additions
        .tasks
        .iter()
        .any(|task| !task_ids.insert(&task.id))
    {
        return Err(StrategyError::invalid("generated task identity collision"));
    }
    let mut profile_ids = BTreeSet::new();
    if additions
        .activity_profiles
        .iter()
        .any(|profile| !profile_ids.insert(&profile.id))
    {
        return Err(StrategyError::invalid(
            "generated activity identity collision",
        ));
    }
    Ok(())
}

fn require_game_scope(scope: &ScopeSelector, game_id: &str, kind: &str) -> StrategyResult<()> {
    if matches!(scope, ScopeSelector::Game { game_id: value } if value == game_id) {
        Ok(())
    } else {
        Err(StrategyError::mismatch(format!(
            "{kind} is not scoped to the report game"
        )))
    }
}

fn require_game_predicate_scopes(predicate: &PredicateSpec, game_id: &str) -> StrategyResult<()> {
    match predicate {
        PredicateSpec::All { predicates } | PredicateSpec::Any { predicates } => {
            for predicate in predicates {
                require_game_predicate_scopes(predicate, game_id)?;
            }
            Ok(())
        }
        PredicateSpec::Not { predicate } => require_game_predicate_scopes(predicate, game_id),
        PredicateSpec::Fact { scope, .. } | PredicateSpec::RecordDeadline { scope, .. } => {
            require_game_scope(scope, game_id, "template fact predicate")
        }
        PredicateSpec::Clock { .. }
        | PredicateSpec::TimelineActive { .. }
        | PredicateSpec::ResourceProjection { .. }
        | PredicateSpec::DependencyCompleted { .. }
        | PredicateSpec::Outcome { .. } => Ok(()),
    }
}

fn boundary_members(members: &[&StrategicInstanceProjection]) -> Vec<String> {
    let known = members
        .iter()
        .filter_map(|value| value.urgency_milli.map(|urgency| (*value, urgency)))
        .collect::<Vec<_>>();
    let minimum = known
        .iter()
        .min_by(|(left, left_value), (right, right_value)| {
            left_value
                .cmp(right_value)
                .then_with(|| left.instance_id.cmp(&right.instance_id))
        })
        .map(|(value, _)| value.instance_id.clone());
    let maximum = known
        .iter()
        .max_by(|(left, left_value), (right, right_value)| {
            left_value
                .cmp(right_value)
                .then_with(|| right.instance_id.cmp(&left.instance_id))
        })
        .map(|(value, _)| value.instance_id.clone());
    let mut result = Vec::new();
    if let Some(minimum) = minimum {
        result.push(minimum);
    }
    if let Some(maximum) = maximum
        && !result.contains(&maximum)
    {
        result.push(maximum);
    }
    result
}

fn cohort_priority_order(left: &CohortProjection, right: &CohortProjection) -> std::cmp::Ordering {
    right
        .aggregate_stats
        .maximum_urgency_milli
        .cmp(&left.aggregate_stats.maximum_urgency_milli)
        .then_with(|| left.cohort_id.cmp(&right.cohort_id))
}

fn planning_reason(disposition: PlanningDisposition) -> &'static str {
    match disposition {
        PlanningDisposition::ExecutionContinues => "execution_continues",
        PlanningDisposition::NeedsPlanning => "no_conditional_template_matched",
        PlanningDisposition::NeedsDetection => "pinned_observation_missing",
        PlanningDisposition::Blocked => "instance_unavailable",
    }
}

fn report_identity(report: &StrategicReport) -> StrategyResult<String> {
    #[derive(Serialize)]
    struct Identity<'a> {
        schema_version: &'a str,
        game_id: &'a str,
        catalog_hash: &'a str,
        catalog_version: u64,
        target_catalog_version: u64,
        as_of_ledger_position: u64,
        as_of_unix_ms: u64,
        policy_hash: &'a str,
        classifier_hash: &'a str,
        evidence: &'a [StrategicEvidencePointer],
        goals: &'a [StrategicGoal],
        assessments: &'a [StrategicInstanceAssessment],
        cohort_budgets: &'a CohortBudgets,
    }
    hash_serializable(
        "strategy-report",
        &Identity {
            schema_version: &report.schema_version,
            game_id: &report.game_id,
            catalog_hash: &report.catalog_hash,
            catalog_version: report.catalog_version,
            target_catalog_version: report.target_catalog_version,
            as_of_ledger_position: report.as_of_ledger_position,
            as_of_unix_ms: report.as_of_unix_ms,
            policy_hash: &report.policy_hash,
            classifier_hash: &report.classifier_hash,
            evidence: &report.evidence,
            goals: &report.goals,
            assessments: &report.assessments,
            cohort_budgets: &report.cohort_budgets,
        },
    )
}

fn hash_serializable(prefix: &str, value: &impl Serialize) -> StrategyResult<String> {
    let bytes = canonical_serialized(value)
        .map_err(|error| StrategyError::invalid(format!("canonical identity failed: {error}")))?;
    Ok(format!("{prefix}:{:x}", Sha256::digest(bytes)))
}

fn generated_suffix(
    report_id: &str,
    goal_id: &str,
    instance_id: &str,
    template_id: &str,
) -> String {
    let digest = Sha256::digest(
        [report_id, goal_id, instance_id, template_id]
            .join("\0")
            .as_bytes(),
    );
    format!("{digest:x}")[..16].to_owned()
}

fn median_u64(values: &[u64]) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    let mut values = values.to_vec();
    values.sort_unstable();
    Some(values[(values.len() - 1) / 2])
}

fn median_u32(values: &[u32]) -> Option<u32> {
    if values.is_empty() {
        return None;
    }
    let mut values = values.to_vec();
    values.sort_unstable();
    Some(values[(values.len() - 1) / 2])
}

fn validate_identifier(value: &str, label: &str) -> StrategyResult<()> {
    if value.is_empty()
        || value.len() > MAX_IDENTIFIER_BYTES
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'.' | b'_' | b':' | b'-')
        })
    {
        return Err(StrategyError::invalid(format!("{label} is invalid")));
    }
    Ok(())
}

fn validate_sha256(value: &str, label: &str) -> StrategyResult<()> {
    if value.strip_prefix("sha256:").is_none_or(|digest| {
        digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    }) {
        return Err(StrategyError::invalid(format!("{label} is invalid")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CatalogDocumentSource, CatalogSources, compile_catalog};

    fn source(uri: &str, value: serde_json::Value) -> CatalogDocumentSource {
        CatalogDocumentSource::new(uri, serde_json::to_vec(&value).expect("fixture JSON"))
    }

    fn catalog_sources() -> CatalogSources {
        CatalogSources {
            tasks: source(
                "tasks.json",
                serde_json::json!({
                    "schema_version": "actingcommand.scheduling.v1",
                    "catalog": {"catalog_id": "fixture-catalog", "catalog_version": 1, "approval_refs": ["approval:fixture"]},
                    "tasks": [{
                        "id": "template.observe",
                        "scope": {"kind": "game", "game_id": "fixture-game"},
                        "entrypoint": {"operation_id": "operation.observe"},
                        "procedure_ref": "procedure.observe",
                        "priority": 10,
                        "trigger": {"kind": "clock", "schedule": {"kind": "interval", "clock_source": {"kind": "local"}, "every_ms": 60000, "anchor_ms": 1}},
                        "feedback_stop": {"kind": "outcome", "task_id": "template.observe", "outcome_key": "completed", "comparison": "eq", "value": {"type": "boolean", "value": true}},
                        "consumes": [],
                        "produces": [{
                            "pool_id": "fixture-pool",
                            "direction": "produce",
                            "amount": 10,
                            "observation_source": "scan_verified",
                            "confidence_milli": 1000
                        }],
                        "on_failure": {"action": "continue", "retry_limit": 1, "retry_backoff_ms": 1000, "escalation_threshold": 2},
                        "sensitive": false, "next_run_clamp_ms": 1000, "yield_points": ["safe"],
                        "expected_duration_ms": 1000, "cooldown_ms": 0, "load_profile": {"kind": "light"},
                        "loop_budget": {"daily_limit": 10, "window_iteration_limit": 5, "max_runtime_ms": 60000},
                        "strategic_weight_milli": 100, "instance_overrides": []
                    }]
                }),
            ),
            pools: source(
                "pools.json",
                serde_json::json!({
                    "schema_version": "actingcommand.scheduling.v1",
                    "catalog": {"catalog_id": "fixture-catalog", "catalog_version": 1, "approval_refs": ["approval:fixture"]},
                    "pools": [{
                        "id": "fixture-pool",
                        "scope": {"kind": "game", "game_id": "fixture-game"},
                        "capacity": 1000000,
                        "projection": {"amount": 1, "per_ms": 1000},
                        "observation": {"kind": "fact", "fact_key": "resource.current"},
                        "group_delay": {"minimum_delay_ms": 1000, "maximum_delay_ms": 2000}
                    }]
                }),
            ),
            activity: source(
                "activity.json",
                serde_json::json!({
                    "schema_version": "actingcommand.scheduling.v1",
                    "catalog": {"catalog_id": "fixture-catalog", "catalog_version": 1, "approval_refs": ["approval:fixture"]},
                    "profiles": [{
                        "id": "template.activity", "scope": {"kind": "game", "game_id": "fixture-game"},
                        "windows": [{"weekdays": [1,2,3,4,5,6,7], "utc_offset_minutes": 0, "start_minute_of_day": 0, "end_minute_of_day": 1439}],
                        "daily_budget": 10, "max_window_iterations": 5, "session_max_ms": 60000,
                        "detection_budget": {"window_dispatch_limit": 2, "window_runtime_ms": 30000, "expected_duration_ms": 10000},
                        "minimum_interval_ms": 1000, "maximum_interval_ms": 2000,
                        "seed_source": "ledger", "resample_policy": "same_round_stable",
                        "importance_milli": 100, "goals": []
                    }]
                }),
            ),
            timeline: source(
                "timeline.json",
                serde_json::json!({
                    "schema_version": "actingcommand.scheduling.v1",
                    "catalog": {"catalog_id": "fixture-catalog", "catalog_version": 1, "approval_refs": ["approval:fixture"]},
                    "events": []
                }),
            ),
        }
    }

    fn catalog() -> CompiledCatalog {
        compile_catalog(&catalog_sources()).expect("fixture catalog")
    }

    fn catalog_with_tasks(tasks: Vec<serde_json::Value>) -> CompiledCatalog {
        let mut sources = catalog_sources();
        let mut document: serde_json::Value =
            serde_json::from_slice(&sources.tasks.bytes).expect("task document");
        document["tasks"] = serde_json::Value::Array(tasks);
        sources.tasks.bytes = serde_json::to_vec(&document).expect("task document bytes");
        compile_catalog(&sources).expect("custom fixture catalog")
    }

    fn template(maximum_urgency_milli: u32) -> StrategicTemplate {
        StrategicTemplate {
            template_id: "template.primary".to_owned(),
            task_template_ids: vec!["template.observe".to_owned()],
            activity_profile_template_id: "template.activity".to_owned(),
            eligibility: PredicateSpec::Fact {
                scope: ScopeSelector::Game {
                    game_id: "fixture-game".to_owned(),
                },
                fact_key: "feature.enabled".to_owned(),
                comparison: crate::Comparison::Eq,
                value: crate::FactValue::Boolean(true),
                max_age_ms: Some(60_000),
            },
            match_bands: vec![
                StrategicBand::Actionable,
                StrategicBand::InfeasibleBestEffort,
            ],
            minimum_urgency_milli: 0,
            maximum_urgency_milli,
            strategic_weight_milli: 500,
            load_profile: LoadProfile::Light,
            risk_class: "standard".to_owned(),
            budget_class: "bounded".to_owned(),
        }
    }

    fn assessment(
        instance: &str,
        current: Option<i64>,
        rate: Option<u64>,
    ) -> StrategicInstanceAssessment {
        let target = current
            .and_then(|value| 100_i64.checked_sub(value))
            .expect("bounded fixture target");
        let remaining_ms = rate
            .and_then(|value| value.checked_mul(RATE_PERIOD_MS))
            .and_then(|value| value.checked_div(50))
            .expect("bounded fixture deadline");
        StrategicInstanceAssessment {
            goal_id: "goal.primary".to_owned(),
            instance_id: instance.to_owned(),
            game_id: "fixture-game".to_owned(),
            fact_snapshot_id: "snapshot.frozen".to_owned(),
            current_projection: Some(0),
            production_rate_per_hour: Some(50),
            target,
            deadline_unix_ms: 1_000_000 + remaining_ms,
            available: true,
            capability_ids: vec!["operation.observe".to_owned()],
        }
    }

    fn report(
        catalog: &CompiledCatalog,
        assessments: Vec<StrategicInstanceAssessment>,
    ) -> StrategicReport {
        StrategicReport::new(
            "fixture-game",
            catalog.catalog_hash(),
            1,
            2,
            7,
            1_000_000,
            format!("sha256:{}", "a".repeat(64)),
            format!("sha256:{}", "b".repeat(64)),
            vec![StrategicEvidencePointer {
                artifact_id: "artifact:fixture".to_owned(),
                sha256: format!("sha256:{}", "c".repeat(64)),
            }],
            vec![StrategicGoal {
                goal_id: "goal.primary".to_owned(),
                goal_version: 1,
                metric: MetricRef::Pool {
                    pool_id: "fixture-pool".to_owned(),
                },
                templates: vec![template(MAX_URGENCY_MILLI)],
                outlier_policy: OutlierPolicy {
                    metric: OutlierMetric::Shortfall,
                    mad_multiplier_milli: 2_000,
                    top_n: 1,
                },
            }],
            assessments,
            CohortBudgets {
                max_active: 2,
                max_prompt: 1,
            },
        )
        .expect("strategy report")
    }

    fn frozen_inputs(report: &StrategicReport) -> (EvaluationFacts, EvaluationResources) {
        let instances = report
            .assessments()
            .iter()
            .map(|assessment| {
                (
                    assessment.instance_id.clone(),
                    InstanceSnapshot {
                        instance_id: assessment.instance_id.clone(),
                        server_id: format!("server.{}", assessment.instance_id),
                        game_id: assessment.game_id.clone(),
                        host_id: format!("host.{}", assessment.instance_id),
                        available: assessment.available,
                        capability_operation_ids: assessment.capability_ids.clone(),
                        preferred_task_ids: Vec::new(),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>()
            .into_values()
            .collect();
        (
            EvaluationFacts {
                ledger_position: report.as_of_ledger_position(),
                fact_snapshot_id: "snapshot.frozen".to_owned(),
                facts: Vec::new(),
                outcomes: Vec::new(),
                tasks: Vec::new(),
                instances,
            },
            EvaluationResources {
                pools: vec![crate::PoolValueSnapshot {
                    pool_id: "fixture-pool".to_owned(),
                    value: 0,
                    observed_at_unix_ms: report.as_of_unix_ms(),
                }],
                hosts: Vec::new(),
            },
        )
    }

    fn refresh_report_identity(report: &mut StrategicReport) {
        report.report_id = report_identity(report).expect("report identity");
    }

    #[test]
    fn projection_is_deterministic_and_mechanical() {
        let catalog = catalog();
        let report = report(
            &catalog,
            vec![
                assessment("instance-a", Some(50), Some(100)),
                assessment("instance-b", Some(0), Some(10)),
            ],
        );
        let (facts, resources) = frozen_inputs(&report);
        let first = project_strategic_report(&catalog, &report, &facts, &resources)
            .expect("first projection");
        let second = project_strategic_report(&catalog, &report, &facts, &resources)
            .expect("second projection");
        assert_eq!(first, second);
        assert_eq!(first.instances[0].band, StrategicBand::Actionable);
        assert_eq!(first.instances[1].band, StrategicBand::InfeasibleBestEffort);
        assert_eq!(first.additions.tasks.len(), 2);
        assert_eq!(first.additions.activity_profiles.len(), 2);
        assert!(
            first
                .additions
                .activity_profiles
                .iter()
                .any(|profile| profile.goals[0].best_effort)
        );
    }

    #[test]
    fn metric_refs_are_derived_from_frozen_inputs_and_report_values_are_only_checks() {
        let catalog = catalog();
        let mut report = report(
            &catalog,
            vec![assessment("instance-a", Some(50), Some(100))],
        );
        report.goals[0].metric = MetricRef::Fact {
            fact_key: "resource.current".to_owned(),
        };
        report.assessments[0].current_projection = Some(60);
        report.assessments[0].production_rate_per_hour = None;
        report.assessments[0].target = 100;
        report.assessments[0].deadline_unix_ms = report.as_of_unix_ms + RATE_PERIOD_MS;
        refresh_report_identity(&mut report);
        let (mut facts, mut resources) = frozen_inputs(&report);
        facts.facts.push(crate::ObservedFact {
            scope: ScopeSelector::Server {
                server_id: "server.instance-a".to_owned(),
            },
            fact_key: "resource.current".to_owned(),
            value: FactValue::Integer(60),
            observed_at_unix_ms: report.as_of_unix_ms(),
            expires_at_unix_ms: Some(report.as_of_unix_ms() + 1),
            confidence_milli: 1_000,
        });
        let projection =
            project_strategic_report(&catalog, &report, &facts, &resources).expect("projection");
        assert_eq!(projection.instances[0].shortfall, Some(40));
        assert_eq!(projection.instances[0].capacity, None);
        assert_eq!(projection.instances[0].band, StrategicBand::NeedsDetection);

        let mut no_pressure = report.clone();
        no_pressure.assessments[0].target = 50;
        refresh_report_identity(&mut no_pressure);
        let projection = project_strategic_report(&catalog, &no_pressure, &facts, &resources)
            .expect("no-pressure projection");
        assert_eq!(projection.instances[0].shortfall, Some(0));
        assert_eq!(projection.instances[0].capacity, Some(0));
        assert_eq!(projection.instances[0].urgency_milli, Some(0));
        assert_eq!(projection.instances[0].band, StrategicBand::NoPressure);

        let mut mismatch = report.clone();
        mismatch.assessments[0].current_projection = Some(61);
        refresh_report_identity(&mut mismatch);
        assert_eq!(
            project_strategic_report(&catalog, &mismatch, &facts, &resources)
                .expect_err("reported current is only an equality check")
                .code(),
            "strategy_report_invalid"
        );
        let mut rate_claim = report.clone();
        rate_claim.assessments[0].production_rate_per_hour = Some(1);
        refresh_report_identity(&mut rate_claim);
        assert_eq!(
            project_strategic_report(&catalog, &rate_claim, &facts, &resources)
                .expect_err("a fact metric has no catalog rate")
                .code(),
            "strategy_report_invalid"
        );

        let mut wrong_snapshot = report.clone();
        wrong_snapshot.assessments[0].fact_snapshot_id = "snapshot.other".to_owned();
        refresh_report_identity(&mut wrong_snapshot);
        assert!(project_strategic_report(&catalog, &wrong_snapshot, &facts, &resources).is_err());
        let mut wrong_position = facts.clone();
        wrong_position.ledger_position += 1;
        assert!(project_strategic_report(&catalog, &report, &wrong_position, &resources).is_err());
        let mut duplicate_instance = facts.clone();
        duplicate_instance
            .instances
            .push(facts.instances[0].clone());
        assert!(
            project_strategic_report(&catalog, &report, &duplicate_instance, &resources).is_err()
        );

        let mut duplicate_fact = facts.clone();
        duplicate_fact.facts.push(facts.facts[0].clone());
        assert!(project_strategic_report(&catalog, &report, &duplicate_fact, &resources).is_err());
        for fact_value in [None, Some(FactValue::String("60".to_owned()))] {
            let mut unknown_facts = facts.clone();
            match fact_value {
                None => unknown_facts.facts.clear(),
                Some(value) => unknown_facts.facts[0].value = value,
            }
            let mut unknown_report = report.clone();
            unknown_report.assessments[0].current_projection = None;
            refresh_report_identity(&mut unknown_report);
            let projection =
                project_strategic_report(&catalog, &unknown_report, &unknown_facts, &resources)
                    .expect("unknown metric projection");
            assert_eq!(projection.instances[0].band, StrategicBand::NeedsDetection);
            assert_eq!(projection.instances[0].shortfall, None);
        }
        let mut expired_facts = facts.clone();
        expired_facts.facts[0].expires_at_unix_ms = Some(report.as_of_unix_ms() - 1);
        let mut expired_report = report.clone();
        expired_report.assessments[0].current_projection = None;
        refresh_report_identity(&mut expired_report);
        assert_eq!(
            project_strategic_report(&catalog, &expired_report, &expired_facts, &resources)
                .expect("expired metric projection")
                .instances[0]
                .band,
            StrategicBand::NeedsDetection
        );

        resources.pools[0].value = 23;
        let instance = &facts.instances[0];
        assert_eq!(
            resolve_metric_current(
                &MetricRef::Pool {
                    pool_id: "fixture-pool".to_owned()
                },
                &report.assessments[0],
                instance,
                &facts,
                &resources,
                report.as_of_unix_ms(),
            )
            .expect("pool metric"),
            Some(23)
        );
        let mut duplicate_pool = resources.clone();
        duplicate_pool.pools.push(resources.pools[0].clone());
        assert!(
            resolve_metric_current(
                &MetricRef::Pool {
                    pool_id: "fixture-pool".to_owned()
                },
                &report.assessments[0],
                instance,
                &facts,
                &duplicate_pool,
                report.as_of_unix_ms(),
            )
            .is_err()
        );
        let mut overflow_pool = resources.clone();
        overflow_pool.pools[0].value = u64::MAX;
        assert_eq!(
            resolve_metric_current(
                &MetricRef::Pool {
                    pool_id: "fixture-pool".to_owned()
                },
                &report.assessments[0],
                instance,
                &facts,
                &overflow_pool,
                report.as_of_unix_ms(),
            )
            .expect_err("pool conversion must be checked")
            .code(),
            "strategy_numeric_overflow"
        );

        let outcome_metric = MetricRef::Outcome {
            task_id: "template.observe".to_owned(),
            outcome_key: "completed".to_owned(),
        };
        facts.outcomes.push(crate::ObservedOutcome {
            task_id: "template.observe".to_owned(),
            instance_id: "instance-a".to_owned(),
            outcome_key: "completed".to_owned(),
            value: FactValue::Integer(31),
            observed_at_unix_ms: report.as_of_unix_ms(),
        });
        assert_eq!(
            resolve_metric_current(
                &outcome_metric,
                &report.assessments[0],
                instance,
                &facts,
                &resources,
                report.as_of_unix_ms(),
            )
            .expect("outcome metric"),
            Some(31)
        );
        let mut duplicate_outcome = facts.clone();
        duplicate_outcome.outcomes.push(facts.outcomes[0].clone());
        assert!(
            resolve_metric_current(
                &outcome_metric,
                &report.assessments[0],
                instance,
                &duplicate_outcome,
                &resources,
                report.as_of_unix_ms(),
            )
            .is_err()
        );
        facts.outcomes[0].value = FactValue::Boolean(true);
        assert_eq!(
            resolve_metric_current(
                &outcome_metric,
                &report.assessments[0],
                instance,
                &facts,
                &resources,
                report.as_of_unix_ms(),
            )
            .expect("non-integer outcome"),
            None
        );
    }

    #[test]
    fn catalog_production_rate_is_bounded_and_deterministic() {
        let base_task = {
            let sources = catalog_sources();
            let document: serde_json::Value =
                serde_json::from_slice(&sources.tasks.bytes).expect("task document");
            document["tasks"][0].clone()
        };
        let task = |id: &str,
                    amount: u64,
                    daily_limit: u32,
                    window_iteration_limit: u32,
                    max_runtime_ms: u64,
                    expected_duration_ms: u64,
                    next_run_clamp_ms: u64| {
            let mut task = base_task.clone();
            task["id"] = serde_json::json!(id);
            task["feedback_stop"]["task_id"] = serde_json::json!(id);
            task["produces"][0]["amount"] = serde_json::json!(amount);
            task["loop_budget"] = serde_json::json!({
                "daily_limit": daily_limit,
                "window_iteration_limit": window_iteration_limit,
                "max_runtime_ms": max_runtime_ms
            });
            task["expected_duration_ms"] = serde_json::json!(expected_duration_ms);
            task["next_run_clamp_ms"] = serde_json::json!(next_run_clamp_ms);
            task["cooldown_ms"] = serde_json::json!(0);
            task
        };
        let mut excluded_scope = task("producer.scope", 10_000, 10, 10, 10_000, 1_000, 1_000);
        excluded_scope["scope"] = serde_json::json!({"kind": "game", "game_id": "other-game"});
        let mut excluded_capability =
            task("producer.capability", 10_000, 10, 10, 10_000, 1_000, 1_000);
        excluded_capability["entrypoint"]["operation_id"] =
            serde_json::json!("operation.unavailable");
        let mut excluded_override = task("producer.override", 10_000, 10, 10, 10_000, 1_000, 1_000);
        excluded_override["instance_overrides"] = serde_json::json!([{
            "instance_id": "instance-a",
            "enabled": false,
            "priority": null,
            "strategic_weight_milli": null,
            "load_profile": null
        }]);
        let catalog = catalog_with_tasks(vec![
            task("producer.daily", 1, 1, 10, 10_000, 1_000, 1_000),
            task("producer.window", 10, 10, 2, 10_000, 1_000, 1_000),
            task("producer.runtime", 100, 10, 10, 3_000, 1_000, 1_000),
            task("producer.cycle", 1_000, 100, 100, 100_000, 1_000, 1_200_000),
            excluded_scope,
            excluded_capability,
            excluded_override,
        ]);
        let report = report(
            &catalog,
            vec![assessment("instance-a", Some(50), Some(100))],
        );
        let (facts, _) = frozen_inputs(&report);
        let instance = &facts.instances[0];
        let metric = MetricRef::Pool {
            pool_id: "fixture-pool".to_owned(),
        };
        assert_eq!(
            catalog_production_rate(&catalog, &metric, instance).expect("first rate"),
            Some(3_321)
        );
        assert_eq!(
            catalog_production_rate(&catalog, &metric, instance).expect("second rate"),
            Some(3_321)
        );
        assert_eq!(
            catalog_production_rate(
                &catalog,
                &MetricRef::Fact {
                    fact_key: "resource.current".to_owned()
                },
                instance,
            )
            .expect("fact rate"),
            None
        );
        assert_eq!(
            catalog_production_rate(
                &catalog,
                &MetricRef::Outcome {
                    task_id: "producer.daily".to_owned(),
                    outcome_key: "completed".to_owned()
                },
                instance,
            )
            .expect("outcome rate"),
            None
        );

        let mut no_producer = base_task.clone();
        no_producer["produces"] = serde_json::json!([]);
        let no_producer = catalog_with_tasks(vec![no_producer]);
        assert_eq!(
            catalog_production_rate(&no_producer, &metric, instance).expect("no-producer rate"),
            Some(0)
        );

        let overflow_amount = 9_007_199_254_740_991_u64;
        let overflow_tasks = (0..3)
            .map(|index| {
                task(
                    &format!("producer.overflow-{index}"),
                    overflow_amount,
                    1_000,
                    1_000,
                    1_000,
                    1,
                    1,
                )
            })
            .collect::<Vec<_>>();
        let mut sources = catalog_sources();
        let mut tasks_document: serde_json::Value =
            serde_json::from_slice(&sources.tasks.bytes).expect("overflow task document");
        tasks_document["tasks"] = serde_json::Value::Array(overflow_tasks);
        sources.tasks.bytes = serde_json::to_vec(&tasks_document).expect("overflow task bytes");
        let mut pools_document: serde_json::Value =
            serde_json::from_slice(&sources.pools.bytes).expect("overflow pool document");
        pools_document["pools"][0]["capacity"] = serde_json::json!(overflow_amount);
        sources.pools.bytes = serde_json::to_vec(&pools_document).expect("overflow pool bytes");
        let overflow_catalog = compile_catalog(&sources).expect("overflow catalog");
        assert_eq!(
            catalog_production_rate(&overflow_catalog, &metric, instance)
                .expect_err("production sum must be checked")
                .code(),
            "strategy_numeric_overflow"
        );
    }

    #[test]
    fn missing_template_enters_planning_without_stopping_other_instances() {
        let catalog = catalog();
        let mut report = report(
            &catalog,
            vec![
                assessment("instance-a", Some(50), Some(100)),
                assessment("instance-b", Some(0), Some(10)),
            ],
        );
        report.goals[0].templates[0].maximum_urgency_milli = 1_000;
        report.report_id = report_identity(&report).expect("report identity");
        let (facts, resources) = frozen_inputs(&report);
        let projection =
            project_strategic_report(&catalog, &report, &facts, &resources).expect("projection");
        assert_eq!(projection.additions.tasks.len(), 1);
        assert_eq!(projection.planning_lane.len(), 1);
        assert_eq!(projection.planning_lane[0].instance_id, "instance-b");
    }

    #[test]
    fn outlier_and_cohort_budgets_never_drop_or_merge_members() {
        let catalog = catalog();
        let report = report(
            &catalog,
            vec![
                assessment("instance-a", Some(90), Some(100)),
                assessment("instance-b", Some(89), Some(100)),
                assessment("instance-c", Some(-900), Some(100)),
            ],
        );
        let (facts, resources) = frozen_inputs(&report);
        let projection =
            project_strategic_report(&catalog, &report, &facts, &resources).expect("projection");
        assert_eq!(projection.outliers.len(), 1);
        assert_eq!(projection.outliers[0].instance_id, "instance-c");
        assert_eq!(
            projection
                .cohorts
                .iter()
                .map(|cohort| cohort.member_instance_ids.len())
                .sum::<usize>(),
            3
        );
        assert!(projection.active_cohort_ids.len() <= 2);
        assert!(projection.prompt_cohort_ids.len() <= 1);
        assert_eq!(
            projection.active_cohort_ids.len() + projection.deferred_cohort_ids.len(),
            projection.cohorts.len()
        );
    }

    #[test]
    fn cohorts_never_form_cross_goal_cartesian_groups() {
        let catalog = catalog();
        let baseline = report(
            &catalog,
            vec![assessment("instance-a", Some(50), Some(100))],
        );
        let mut secondary_goal = baseline.goals[0].clone();
        secondary_goal.goal_id = "goal.secondary".to_owned();
        let mut secondary_assessment = baseline.assessments[0].clone();
        secondary_assessment.goal_id = "goal.secondary".to_owned();
        let report = StrategicReport::new(
            baseline.game_id(),
            baseline.catalog_hash(),
            baseline.catalog_version(),
            baseline.target_catalog_version(),
            baseline.as_of_ledger_position(),
            baseline.as_of_unix_ms(),
            baseline.policy_hash(),
            baseline.classifier_hash(),
            baseline.evidence().to_vec(),
            vec![baseline.goals[0].clone(), secondary_goal],
            vec![baseline.assessments[0].clone(), secondary_assessment],
            baseline.cohort_budgets().clone(),
        )
        .expect("two-goal report");
        let (facts, resources) = frozen_inputs(&report);
        let projection =
            project_strategic_report(&catalog, &report, &facts, &resources).expect("projection");
        assert_eq!(projection.cohorts.len(), 2);
        assert_eq!(projection.additions.activity_profiles.len(), 1);
        assert_eq!(projection.additions.activity_profiles[0].goals.len(), 2);
        assert_eq!(
            projection
                .cohorts
                .iter()
                .map(|cohort| cohort.goal_id.as_str())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["goal.primary", "goal.secondary"])
        );
    }

    #[test]
    fn report_rejects_cross_game_assessments() {
        let catalog = catalog();
        let mut input = assessment("instance-a", Some(50), Some(100));
        input.game_id = "other-game".to_owned();
        let error = StrategicReport::new(
            "fixture-game",
            catalog.catalog_hash(),
            1,
            2,
            7,
            1_000_000,
            format!("sha256:{}", "a".repeat(64)),
            format!("sha256:{}", "b".repeat(64)),
            vec![StrategicEvidencePointer {
                artifact_id: "artifact:fixture".to_owned(),
                sha256: format!("sha256:{}", "c".repeat(64)),
            }],
            vec![StrategicGoal {
                goal_id: "goal.primary".to_owned(),
                goal_version: 1,
                metric: MetricRef::Fact {
                    fact_key: "resource.current".to_owned(),
                },
                templates: vec![template(MAX_URGENCY_MILLI)],
                outlier_policy: OutlierPolicy {
                    metric: OutlierMetric::Shortfall,
                    mad_multiplier_milli: 2_000,
                    top_n: 1,
                },
            }],
            vec![input],
            CohortBudgets {
                max_active: 1,
                max_prompt: 1,
            },
        )
        .expect_err("cross-game report must fail");
        assert_eq!(error.code(), "strategy_report_invalid");
    }

    #[test]
    fn report_identity_rejects_mutated_instance_state() {
        let catalog = catalog();
        let report = report(
            &catalog,
            vec![assessment("instance-a", Some(50), Some(100))],
        );
        let mut encoded = serde_json::to_value(report).expect("report JSON");
        encoded["assessments"][0]["current_projection"] = serde_json::json!(51);
        let changed: StrategicReport = serde_json::from_value(encoded).expect("typed report");
        assert_eq!(
            changed.validate().expect_err("identity mismatch").code(),
            "strategy_report_invalid"
        );
    }
}
