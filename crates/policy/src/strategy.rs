// SPDX-License-Identifier: AGPL-3.0-only

//! Pure strategic-report projection into existing scheduling declarations.

use crate::canonical::canonical_serialized;
use crate::{
    ActivityProfile, CompiledCatalog, GoalTarget, LoadProfile, MetricRef, PredicateSpec,
    ScopeSelector, TaskSpec,
};
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
) -> StrategyResult<StrategicProjection> {
    report.validate()?;
    if report.catalog_hash() != catalog.catalog_hash()
        || report.catalog_version() != catalog.summary().catalog_version
    {
        return Err(StrategyError::mismatch(
            "report does not target the supplied catalog generation",
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
        let calculated = calculate_assessment(report.as_of_unix_ms(), assessment)?;
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
) -> StrategyResult<CalculatedAssessment> {
    if !assessment.available {
        return Ok(CalculatedAssessment {
            shortfall: None,
            capacity: None,
            urgency_milli: None,
            band: StrategicBand::Blocked,
        });
    }
    let (Some(current), Some(rate)) = (
        assessment.current_projection,
        assessment.production_rate_per_hour,
    ) else {
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
        .saturating_add(calculated.urgency_milli.unwrap_or(0).min(10_000))
        .min(10_000) as u16;
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
        | PredicateSpec::ResourceProjection { .. }
        | PredicateSpec::Fact { .. } => {}
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
        || template.strategic_weight_milli > 10_000
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
        PredicateSpec::Fact { scope, .. } => {
            require_game_scope(scope, game_id, "template fact predicate")
        }
        PredicateSpec::Clock { .. }
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

    fn catalog() -> CompiledCatalog {
        compile_catalog(&CatalogSources {
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
                        "consumes": [], "produces": [],
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
                    "pools": []
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
        })
        .expect("fixture catalog")
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
        StrategicInstanceAssessment {
            goal_id: "goal.primary".to_owned(),
            instance_id: instance.to_owned(),
            game_id: "fixture-game".to_owned(),
            fact_snapshot_id: format!("snapshot.{instance}"),
            current_projection: current,
            production_rate_per_hour: rate,
            target: 100,
            deadline_unix_ms: 4_600_000,
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
            assessments,
            CohortBudgets {
                max_active: 2,
                max_prompt: 1,
            },
        )
        .expect("strategy report")
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
        let first = project_strategic_report(&catalog, &report).expect("first projection");
        let second = project_strategic_report(&catalog, &report).expect("second projection");
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
        let projection = project_strategic_report(&catalog, &report).expect("projection");
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
        let projection = project_strategic_report(&catalog, &report).expect("projection");
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
        let projection = project_strategic_report(&catalog, &report).expect("projection");
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
