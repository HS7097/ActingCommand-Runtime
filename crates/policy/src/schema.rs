// SPDX-License-Identifier: AGPL-3.0-only

use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;

pub const SCHEDULING_SCHEMA_VERSION: &str = "actingcommand.scheduling.v1";
pub const SCHEDULING_SCHEMA_VERSION_V2: &str = "actingcommand.scheduling.v2";
pub const MAX_DOCUMENT_BYTES: usize = 1_048_576;
pub const MAX_CATALOG_BYTES: usize = 4_194_304;
pub const MAX_ID_BYTES: usize = 128;
pub const MAX_TEXT_BYTES: usize = 1_024;
pub const MAX_APPROVAL_REFS: usize = 64;
pub const MAX_TASKS: usize = 4_096;
pub const MAX_POOLS: usize = 1_024;
pub const MAX_ACTIVITY_PROFILES: usize = 1_024;
pub const MAX_TIMELINE_EVENTS: usize = 4_096;
pub const MAX_PREDICATE_DEPTH: usize = 16;
pub const MAX_PREDICATE_NODES: usize = 512;
pub const MAX_EFFECTS_PER_TASK: usize = 128;
pub const MAX_REFERENCES_PER_TASK: usize = 128;
pub const MAX_WINDOWS_PER_PROFILE: usize = 128;
pub const MAX_GOALS_PER_PROFILE: usize = 128;
pub const MAX_INSTANCE_OVERRIDES_PER_TASK: usize = 128;
pub const MAX_BUDGET_COUNT: u32 = 1_000_000;
pub const MAX_CLOCK_DRIFT_MS: i64 = 604_800_000;
pub const MAX_FACT_MAX_AGE_MS: u64 = 31_536_000_000;
pub const MIN_UTC_OFFSET_MINUTES: i16 = -840;
pub const MAX_UTC_OFFSET_MINUTES: i16 = 840;
pub const MIN_DST_OFFSET_MINUTES: i16 = -120;
pub const MAX_DST_OFFSET_MINUTES: i16 = 120;
pub const MIN_CANONICAL_INTEGER: i64 = -9_007_199_254_740_991;
pub const MAX_CANONICAL_INTEGER: i64 = 9_007_199_254_740_991;
/// Longest score-deferral a catalog may declare: one day.
pub const MAX_DEFER_FOR_MS: u64 = 86_400_000;
/// Largest manual priority offset magnitude accepted as evaluation input.
pub const MAX_PRIORITY_OFFSET_MILLI: i32 = 1_000_000;
/// Largest declared task value a catalog may declare.
pub const MAX_VALUE_MILLI: u32 = 1_000_000;
/// Longest eligibility age after which a score deferral turns into a promotion: one day.
pub const MAX_DEFER_AGING_CAP_MS: u64 = 86_400_000;
/// Highest relative threshold a catalog may declare.
pub const MAX_PRIORITY_PERCENTILE: u8 = 100;
/// Longest pool valuation display name, in UTF-8 bytes.
pub const MAX_VALUATION_NAME_BYTES: usize = 128;
/// Longest pool valuation display unit, in UTF-8 bytes.
pub const MAX_VALUATION_UNIT_BYTES: usize = 32;
/// Largest standing weight a pool valuation may declare, in milli per step.
pub const MAX_VALUATION_BASE_WEIGHT_MILLI: u32 = 1_000_000;
/// Largest shortfall weight a pool valuation may declare, in milli per step.
pub const MAX_VALUATION_GAP_WEIGHT_MILLI: u32 = 1_000_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequiredNullable<T>(pub Option<T>);

fn deserialize_required_nullable<'de, D, T>(
    deserializer: D,
) -> Result<RequiredNullable<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(RequiredNullable)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogDiagnostic {
    pub code: CatalogDiagnosticCode,
    pub severity: DiagnosticSeverity,
    pub json_path: String,
    pub source: SourceLocation,
    pub reason: String,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub schema_version: RequiredNullable<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub catalog_id: RequiredNullable<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub catalog_version: RequiredNullable<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogDiagnosticCode {
    DocumentTooLarge,
    CatalogTooLarge,
    InvalidJson,
    DuplicateKey,
    UnsupportedSchemaVersion,
    UnknownField,
    DescriptorMismatch,
    LimitExceeded,
    DuplicateId,
    DanglingReference,
    TypeMismatch,
    MissingRequiredField,
    PredicateUnreachable,
    PredicateUncomputable,
    LoopBudgetMissing,
    EffectIncompatible,
    ApprovalMissing,
    PrioritySelectionWithoutDocument,
    PrioritySelectionPercentileInvalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceLocation {
    pub document: SchedulingDocumentKind,
    pub source_uri: String,
    pub line: u32,
    pub column: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchedulingDocumentKind {
    Tasks,
    Pools,
    Activity,
    Timeline,
    /// The optional fifth document: an `actingcommand.selection-policy.v1` scoring policy.
    Selection,
    /// An instance's `actingcommand.resource-targets.v1` policy (Workflow #308 RT-S1a); not a
    /// catalog document, parsed by the same declaration parser.
    ResourceTargets,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogDescriptor {
    pub catalog_id: String,
    pub catalog_version: u64,
    pub approval_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TasksDocument {
    pub schema_version: String,
    pub catalog: CatalogDescriptor,
    pub tasks: Vec<TaskSpec>,
    /// Catalog-level score thresholds; allowed only next to a selection document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority_selection: Option<PrioritySelection>,
}

/// Turns a selection-policy score into a scheduling disposition.
///
/// A candidate whose effective score is below `defer_below_milli` is deferred for
/// `defer_for_ms`; one above `promote_above_milli` ranks ahead of every other candidate.
/// With the optional percentile pair the thresholds are taken from the cycle's own
/// candidates instead; the absolute pair stays the fallback for small cycles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrioritySelection {
    pub defer_below_milli: i64,
    pub defer_for_ms: u64,
    pub promote_above_milli: i64,
    /// Eligibility age that adds one milli of urgency to the utility term; absent adds none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aging_ms_per_milli: Option<u32>,
    /// Nearest-rank percentile at or below which a candidate is deferred; paired with
    /// `promote_above_percentile`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defer_below_percentile: Option<u8>,
    /// Nearest-rank percentile above which a candidate is promoted; paired with
    /// `defer_below_percentile`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promote_above_percentile: Option<u8>,
    /// Eligibility age at which a candidate that would be deferred is promoted instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defer_aging_cap_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoolsDocument {
    pub schema_version: String,
    pub catalog: CatalogDescriptor,
    pub pools: Vec<PoolSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityDocument {
    pub schema_version: String,
    pub catalog: CatalogDescriptor,
    pub profiles: Vec<ActivityProfile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimelineDocument {
    pub schema_version: String,
    pub catalog: CatalogDescriptor,
    pub events: Vec<TimelineEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogBundle {
    pub tasks: TasksDocument,
    pub pools: PoolsDocument,
    pub activity: ActivityDocument,
    pub timeline: TimelineDocument,
}

impl CatalogBundle {
    pub fn descriptors_match(&self) -> bool {
        matches!(
            self.tasks.schema_version.as_str(),
            SCHEDULING_SCHEMA_VERSION | SCHEDULING_SCHEMA_VERSION_V2
        ) && self.pools.schema_version == self.tasks.schema_version
            && self.activity.schema_version == self.tasks.schema_version
            && self.timeline.schema_version == self.tasks.schema_version
            && self.tasks.catalog == self.pools.catalog
            && self.tasks.catalog == self.activity.catalog
            && self.tasks.catalog == self.timeline.catalog
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScopeSelector {
    /// Exact registered instance alias, preserved byte-for-byte.
    Instance {
        instance_id: String,
    },
    Server {
        server_id: String,
    },
    Game {
        game_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSpec {
    pub id: String,
    pub scope: ScopeSelector,
    pub entrypoint: OperationRef,
    pub procedure_ref: String,
    pub priority: i16,
    pub trigger: PredicateSpec,
    pub feedback_stop: PredicateSpec,
    pub consumes: Vec<ResourceEffectSpec>,
    pub produces: Vec<ResourceEffectSpec>,
    pub on_failure: FailurePolicy,
    pub sensitive: bool,
    pub next_run_clamp_ms: u64,
    pub yield_points: Vec<String>,
    pub expected_duration_ms: u64,
    pub cooldown_ms: u64,
    pub load_profile: LoadProfile,
    pub loop_budget: LoopBudget,
    pub strategic_weight_milli: u16,
    pub instance_overrides: Vec<InstanceTaskOverride>,
    /// Declared value of one run; absent means the task carries no utility term.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_milli: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceTaskOverride {
    /// Exact registered instance alias.
    pub instance_id: String,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub enabled: RequiredNullable<bool>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub priority: RequiredNullable<i16>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub strategic_weight_milli: RequiredNullable<u16>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub load_profile: RequiredNullable<LoadProfile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationRef {
    pub operation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LoadProfile {
    Light,
    Heavy,
    Weighted {
        cpu_milli: u16,
        gpu_milli: u16,
        io_milli: u16,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoopBudget {
    pub daily_limit: u32,
    pub window_iteration_limit: u32,
    pub max_runtime_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailurePolicy {
    pub action: FailureAction,
    pub retry_limit: u16,
    pub retry_backoff_ms: u64,
    pub escalation_threshold: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureAction {
    Continue,
    Pause,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceEffectSpec {
    pub pool_id: String,
    pub direction: EffectDirection,
    pub amount: u64,
    pub observation_source: ObservationSource,
    pub confidence_milli: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectDirection {
    Consume,
    Produce,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationSource {
    SelfReported,
    ScanVerified,
    Inferred,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PredicateSpec {
    All {
        predicates: Vec<PredicateSpec>,
    },
    Any {
        predicates: Vec<PredicateSpec>,
    },
    Not {
        predicate: Box<PredicateSpec>,
    },
    Clock {
        schedule: ClockSchedule,
    },
    TimelineActive {
        event_id: String,
    },
    ResourceProjection {
        pool_id: String,
        comparison: Comparison,
        value: i64,
    },
    Fact {
        scope: ScopeSelector,
        fact_key: String,
        comparison: Comparison,
        value: FactValue,
        max_age_ms: Option<u64>,
    },
    RecordDeadline {
        scope: ScopeSelector,
        fact_key: String,
        timestamp_field: String,
        within_ms: u64,
        max_age_ms: Option<u64>,
    },
    DependencyCompleted {
        task_id: String,
        terminal_states: Vec<TaskTerminalState>,
    },
    Outcome {
        task_id: String,
        outcome_key: String,
        comparison: Comparison,
        value: FactValue,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClockSchedule {
    Interval {
        clock_source: ClockSource,
        every_ms: u64,
        anchor_ms: u64,
    },
    At {
        clock_source: ClockSource,
        at_ms: u64,
    },
    Daily {
        clock_source: ClockSource,
        minutes_of_day: Vec<u16>,
    },
    Weekly {
        clock_source: ClockSource,
        weekday: u8,
        minute_of_day: u16,
    },
}

/// Selects the independently pinned clock coordinate used by a schedule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClockSource {
    Local,
    Server {
        timezone_id: String,
        utc_offset_minutes: i16,
        dst_offset_minutes: i16,
        maintenance_drift_ms: i64,
    },
    Reveal {
        reveal_source: String,
        timezone_id: String,
        utc_offset_minutes: i16,
        dst_offset_minutes: i16,
        maintenance_drift_ms: i64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Comparison {
    Eq,
    NotEq,
    LessThan,
    LessThanOrEqual,
    GreaterThan,
    GreaterThanOrEqual,
    Contains,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum FactValue {
    Boolean(bool),
    Integer(i64),
    String(String),
    TimestampMs(u64),
    DurationMs(u64),
    RecordList(Vec<BTreeMap<String, FactScalar>>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum FactScalar {
    Boolean(bool),
    Integer(i64),
    String(String),
    TimestampMs(u64),
    DurationMs(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskTerminalState {
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoolSpec {
    pub id: String,
    pub scope: ScopeSelector,
    pub capacity: u64,
    pub projection: RegenProjection,
    pub observation: ObservationRef,
    #[serde(default, skip_serializing_if = "PoolValueSource::is_static")]
    pub value_source: PoolValueSource,
    pub group_delay: Option<GroupDelayPolicy>,
    /// What one step of this pool's resource is worth; absent keeps the catalog hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valuation: Option<PoolValuation>,
}

/// A pool's resource valuation declaration (Workflow #335 S2a). The resource kind is the
/// pool id; `name` and `unit` are display text only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoolValuation {
    pub name: String,
    pub unit: String,
    /// Measuring step: the resource amount one weight applies to.
    pub scale: u64,
    /// Standing weight in milli for each produced step.
    pub base_weight_milli: u32,
    /// Shortfall conversion; absent means a resource target must carry its own importance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap: Option<PoolValuationGap>,
}

/// How a shortfall against a resource target converts into extra weight.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoolValuationGap {
    pub rule: ValuationGapRule,
    /// Extra milli for each produced step per missing step of shortfall.
    pub weight_milli: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValuationGapRule {
    /// The target is to hold at least an amount.
    ShortfallLinear,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PoolValueSource {
    #[default]
    StaticSnapshot,
    LedgerFact {
        minimum_confidence_milli: u16,
    },
}

impl PoolValueSource {
    pub fn is_static(&self) -> bool {
        matches!(self, Self::StaticSnapshot)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegenProjection {
    pub amount: u64,
    pub per_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ObservationRef {
    Fact {
        fact_key: String,
    },
    Outcome {
        task_id: String,
        outcome_key: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupDelayPolicy {
    pub minimum_delay_ms: u64,
    pub maximum_delay_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityProfile {
    pub id: String,
    pub scope: ScopeSelector,
    pub windows: Vec<ActivityWindow>,
    pub daily_budget: u32,
    pub max_window_iterations: u32,
    pub session_max_ms: u64,
    pub detection_budget: DetectionBudget,
    pub minimum_interval_ms: u64,
    pub maximum_interval_ms: u64,
    pub seed_source: SeedSource,
    pub resample_policy: ResamplePolicy,
    pub importance_milli: u16,
    pub goals: Vec<GoalTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DetectionBudget {
    pub window_dispatch_limit: u32,
    pub window_runtime_ms: u64,
    pub expected_duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityWindow {
    pub weekdays: Vec<u8>,
    pub utc_offset_minutes: i16,
    pub start_minute_of_day: u16,
    pub end_minute_of_day: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeedSource {
    Ledger,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResamplePolicy {
    SameRoundStable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalTarget {
    pub id: String,
    pub metric: MetricRef,
    pub target: i64,
    pub deadline_unix_ms: u64,
    pub strategic_weight_milli: u16,
    pub best_effort: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MetricRef {
    Fact {
        fact_key: String,
    },
    Pool {
        pool_id: String,
    },
    Outcome {
        task_id: String,
        outcome_key: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimelineEvent {
    pub id: String,
    pub scope: ScopeSelector,
    pub event_kind: TimelineEventKind,
    pub schedule: ClockSchedule,
    pub duration_ms: u64,
    pub invalidates_fact_prefixes: Vec<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_timeline_validity"
    )]
    pub validity: Option<TimelineValidity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimelineValidity {
    pub from_unix_ms: u64,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub until_unix_ms: RequiredNullable<u64>,
}

fn deserialize_timeline_validity<'de, D>(
    deserializer: D,
) -> Result<Option<TimelineValidity>, D::Error>
where
    D: Deserializer<'de>,
{
    TimelineValidity::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimelineEventKind {
    Reset,
    Maintenance,
    Activity,
    Deadline,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_bundle() -> CatalogBundle {
        CatalogBundle {
            tasks: serde_json::from_str(include_str!(
                "../../../contracts/scheduling/examples/catalog-a/tasks.json"
            ))
            .expect("tasks example"),
            pools: serde_json::from_str(include_str!(
                "../../../contracts/scheduling/examples/catalog-a/pools.json"
            ))
            .expect("pools example"),
            activity: serde_json::from_str(include_str!(
                "../../../contracts/scheduling/examples/catalog-a/activity.json"
            ))
            .expect("activity example"),
            timeline: serde_json::from_str(include_str!(
                "../../../contracts/scheduling/examples/catalog-a/timeline.json"
            ))
            .expect("timeline example"),
        }
    }

    #[test]
    fn neutral_examples_share_one_frozen_descriptor() {
        let bundle = sample_bundle();
        assert!(bundle.descriptors_match());
        assert_eq!(bundle.tasks.tasks.len(), 1);
        assert_eq!(bundle.pools.pools.len(), 1);
        assert_eq!(bundle.activity.profiles.len(), 2);
        assert_eq!(bundle.timeline.events.len(), 1);
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let mut value: serde_json::Value = serde_json::from_str(include_str!(
            "../../../contracts/scheduling/examples/catalog-a/tasks.json"
        ))
        .expect("tasks JSON");
        value["unexpected"] = serde_json::json!(true);
        let error = serde_json::from_value::<TasksDocument>(value)
            .expect_err("unknown top-level field must fail");
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn descriptor_mismatch_is_visible() {
        let mut bundle = sample_bundle();
        bundle.timeline.catalog.catalog_version += 1;
        assert!(!bundle.descriptors_match());
    }

    #[test]
    fn nullable_override_fields_must_be_present() {
        let mut value: serde_json::Value = serde_json::from_str(include_str!(
            "../../../contracts/scheduling/examples/catalog-a/tasks.json"
        ))
        .expect("tasks JSON");
        value["tasks"][0]["instance_overrides"][0]
            .as_object_mut()
            .expect("override object")
            .remove("load_profile");

        let error = serde_json::from_value::<TasksDocument>(value)
            .expect_err("missing nullable override field must fail");
        assert!(error.to_string().contains("load_profile"));
    }

    #[test]
    fn diagnostic_contract_round_trips() {
        let diagnostic = CatalogDiagnostic {
            code: CatalogDiagnosticCode::DanglingReference,
            severity: DiagnosticSeverity::Error,
            json_path: "/tasks/0/produces/0/pool_id".to_owned(),
            source: SourceLocation {
                document: SchedulingDocumentKind::Tasks,
                source_uri: "memory://fixture/tasks.json".to_owned(),
                line: 12,
                column: 17,
            },
            reason: "referenced pool does not exist".to_owned(),
            schema_version: RequiredNullable(Some(SCHEDULING_SCHEMA_VERSION.to_owned())),
            catalog_id: RequiredNullable(Some("fixture.catalog-a".to_owned())),
            catalog_version: RequiredNullable(Some(1)),
        };

        let encoded = serde_json::to_vec(&diagnostic).expect("serialize diagnostic");
        let decoded: CatalogDiagnostic =
            serde_json::from_slice(&encoded).expect("deserialize diagnostic");
        assert_eq!(decoded, diagnostic);
    }

    /// The published V1 scheduling schemas, by file name.
    const PUBLISHED_V1_SCHEMAS: [(&str, &str); 6] = [
        (
            "common.schema.json",
            include_str!("../../../contracts/scheduling/common.schema.json"),
        ),
        (
            "tasks.schema.json",
            include_str!("../../../contracts/scheduling/tasks.schema.json"),
        ),
        (
            "pools.schema.json",
            include_str!("../../../contracts/scheduling/pools.schema.json"),
        ),
        (
            "activity.schema.json",
            include_str!("../../../contracts/scheduling/activity.schema.json"),
        ),
        (
            "timeline.schema.json",
            include_str!("../../../contracts/scheduling/timeline.schema.json"),
        ),
        (
            "diagnostic.schema.json",
            include_str!("../../../contracts/scheduling/diagnostic.schema.json"),
        ),
    ];

    /// The five published V2 scheduling schemas, by file name.
    const PUBLISHED_V2_SCHEMAS: [(&str, &str); 5] = [
        (
            "common.schema.json",
            include_str!("../../../contracts/scheduling/v2/common.schema.json"),
        ),
        (
            "tasks.schema.json",
            include_str!("../../../contracts/scheduling/v2/tasks.schema.json"),
        ),
        (
            "pools.schema.json",
            include_str!("../../../contracts/scheduling/v2/pools.schema.json"),
        ),
        (
            "activity.schema.json",
            include_str!("../../../contracts/scheduling/v2/activity.schema.json"),
        ),
        (
            "timeline.schema.json",
            include_str!("../../../contracts/scheduling/v2/timeline.schema.json"),
        ),
    ];

    /// Each published schema set as (version, file name -> parsed document).
    fn published_schema_sets() -> [(&'static str, BTreeMap<&'static str, serde_json::Value>); 2] {
        fn parse(
            set: &str,
            schemas: &[(&'static str, &'static str)],
        ) -> BTreeMap<&'static str, serde_json::Value> {
            schemas
                .iter()
                .map(|(name, text)| {
                    let schema =
                        serde_json::from_str::<serde_json::Value>(text).unwrap_or_else(|error| {
                            panic!("{set}/{name} is not a JSON document: {error}")
                        });
                    (*name, schema)
                })
                .collect()
        }
        [
            ("v1", parse("v1", &PUBLISHED_V1_SCHEMAS)),
            ("v2", parse("v2", &PUBLISHED_V2_SCHEMAS)),
        ]
    }

    #[test]
    fn published_schemas_are_valid_json_documents() {
        fn collect_references<'a>(value: &'a serde_json::Value, references: &mut Vec<&'a str>) {
            match value {
                serde_json::Value::Object(fields) => {
                    for (key, child) in fields {
                        if key == "$ref" {
                            references.push(child.as_str().expect("$ref is a string"));
                        } else {
                            collect_references(child, references);
                        }
                    }
                }
                serde_json::Value::Array(items) => {
                    for child in items {
                        collect_references(child, references);
                    }
                }
                _ => {}
            }
        }

        for (set, schemas) in published_schema_sets() {
            let mut resolved = 0;
            for (name, schema) in &schemas {
                assert!(schema.is_object(), "{set}/{name} is not a schema object");
                let mut references = Vec::new();
                collect_references(schema, &mut references);
                for reference in references {
                    let (document, pointer) = reference.split_once('#').unwrap_or_else(|| {
                        panic!("{set}/{name}: $ref {reference} has no fragment")
                    });
                    let target = if document.is_empty() {
                        *name
                    } else {
                        document.strip_prefix("./").unwrap_or_else(|| {
                            panic!("{set}/{name}: $ref {reference} leaves the schema directory")
                        })
                    };
                    let target_schema = schemas.get(target).unwrap_or_else(|| {
                        panic!(
                            "{set}/{name}: $ref {reference} names a document outside the {set} set"
                        )
                    });
                    assert!(
                        target_schema.pointer(pointer).is_some(),
                        "{set}/{name}: $ref {reference} does not resolve"
                    );
                    resolved += 1;
                }
            }
            assert!(resolved > 0, "{set}: the published schemas hold no $ref");
        }
    }

    #[test]
    fn published_integer_schemas_are_bounded_to_jcs_safe_values() {
        fn verify(value: &serde_json::Value, path: &str) {
            match value {
                serde_json::Value::Object(fields) => {
                    let integer_type = fields.get("type").is_some_and(|kind| {
                        kind.as_str() == Some("integer")
                            || kind
                                .as_array()
                                .is_some_and(|kinds| kinds.iter().any(|kind| kind == "integer"))
                    });
                    if integer_type {
                        let minimum = fields
                            .get("minimum")
                            .and_then(serde_json::Value::as_i64)
                            .unwrap_or_else(|| panic!("integer minimum missing at {path}"));
                        let maximum = fields
                            .get("maximum")
                            .and_then(serde_json::Value::as_i64)
                            .unwrap_or_else(|| panic!("integer maximum missing at {path}"));
                        assert!(
                            minimum >= MIN_CANONICAL_INTEGER && maximum <= MAX_CANONICAL_INTEGER,
                            "integer bounds exceed JCS safe range at {path}"
                        );
                    }
                    for (key, child) in fields {
                        verify(child, &format!("{path}/{key}"));
                    }
                }
                serde_json::Value::Array(items) => {
                    for (index, child) in items.iter().enumerate() {
                        verify(child, &format!("{path}/{index}"));
                    }
                }
                _ => {}
            }
        }

        for (set, schemas) in published_schema_sets() {
            for (name, schema) in &schemas {
                verify(schema, &format!("{set}/{name}"));
            }
        }
    }

    #[test]
    fn published_schema_bounds_match_compiler_constants() {
        let [(_, v1), (_, v2)] = published_schema_sets();
        for (set, schemas) in [("v1", &v1), ("v2", &v2)] {
            let tasks = &schemas["tasks.schema.json"];
            let activity = &schemas["activity.schema.json"];
            let pools = &schemas["pools.schema.json"];
            let common = &schemas["common.schema.json"];

            assert_eq!(
                tasks["$defs"]["loopBudget"]["properties"]["daily_limit"]["minimum"], 1,
                "{set}"
            );
            assert_eq!(
                tasks["$defs"]["loopBudget"]["properties"]["daily_limit"]["maximum"],
                MAX_BUDGET_COUNT,
                "{set}"
            );
            assert_eq!(
                activity["$defs"]["profile"]["properties"]["daily_budget"]["minimum"], 1,
                "{set}"
            );
            assert_eq!(
                activity["$defs"]["profile"]["properties"]["daily_budget"]["maximum"],
                MAX_BUDGET_COUNT,
                "{set}"
            );
            assert_eq!(
                pools["$defs"]["pool"]["properties"]["projection"]["properties"]["amount"]["minimum"],
                1,
                "{set}"
            );
            let fact = common["$defs"]["predicate"]["oneOf"]
                .as_array()
                .expect("predicate variants")
                .iter()
                .find(|variant| variant["properties"]["kind"]["const"] == "fact")
                .unwrap_or_else(|| panic!("{set}: fact predicate"));
            assert_eq!(fact["properties"]["max_age_ms"]["minimum"], 1, "{set}");
            assert_eq!(
                fact["properties"]["max_age_ms"]["maximum"], MAX_FACT_MAX_AGE_MS,
                "{set}"
            );
            let schedules = common["$defs"]["clockSchedule"]["oneOf"]
                .as_array()
                .expect("clock schedule variants");
            assert_eq!(
                schedules[0]["properties"]["clock_source"]["$ref"], "#/$defs/clockSource",
                "{set}"
            );
            for schedule in &schedules[1..] {
                assert_eq!(
                    schedule["properties"]["clock_source"]["$ref"], "#/$defs/wallClockSource",
                    "{set}"
                );
            }
            for variant in [1, 2] {
                let properties = &common["$defs"]["clockSource"]["oneOf"][variant]["properties"];
                assert_eq!(
                    properties["utc_offset_minutes"]["minimum"], MIN_UTC_OFFSET_MINUTES,
                    "{set}"
                );
                assert_eq!(
                    properties["utc_offset_minutes"]["maximum"], MAX_UTC_OFFSET_MINUTES,
                    "{set}"
                );
                assert_eq!(
                    properties["dst_offset_minutes"]["minimum"], MIN_DST_OFFSET_MINUTES,
                    "{set}"
                );
                assert_eq!(
                    properties["dst_offset_minutes"]["maximum"], MAX_DST_OFFSET_MINUTES,
                    "{set}"
                );
            }
        }

        // V2 alone bounds the timeline validity interval: unsigned Unix milliseconds that the
        // compiler admits only as canonical safe integers.
        let validity =
            &v2["timeline.schema.json"]["$defs"]["event"]["properties"]["validity"]["properties"];
        for field in ["from_unix_ms", "until_unix_ms"] {
            assert_eq!(validity[field]["minimum"], 0, "v2 {field}");
            assert_eq!(
                validity[field]["maximum"], MAX_CANONICAL_INTEGER,
                "v2 {field}"
            );
        }
    }
}
