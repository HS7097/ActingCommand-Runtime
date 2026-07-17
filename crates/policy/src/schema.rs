// SPDX-License-Identifier: AGPL-3.0-only

use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;

pub const SCHEDULING_SCHEMA_VERSION: &str = "actingcommand.scheduling.v1";
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
pub const MIN_CANONICAL_INTEGER: i64 = -9_007_199_254_740_991;
pub const MAX_CANONICAL_INTEGER: i64 = 9_007_199_254_740_991;

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
        self.tasks.schema_version == SCHEDULING_SCHEMA_VERSION
            && self.pools.schema_version == SCHEDULING_SCHEMA_VERSION
            && self.activity.schema_version == SCHEDULING_SCHEMA_VERSION
            && self.timeline.schema_version == SCHEDULING_SCHEMA_VERSION
            && self.tasks.catalog == self.pools.catalog
            && self.tasks.catalog == self.activity.catalog
            && self.tasks.catalog == self.timeline.catalog
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScopeSelector {
    Instance { instance_id: String },
    Server { server_id: String },
    Game { game_id: String },
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceTaskOverride {
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
    pub group_delay: Option<GroupDelayPolicy>,
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

    #[test]
    fn published_schemas_are_valid_json_documents() {
        let schemas = [
            include_str!("../../../contracts/scheduling/common.schema.json"),
            include_str!("../../../contracts/scheduling/tasks.schema.json"),
            include_str!("../../../contracts/scheduling/pools.schema.json"),
            include_str!("../../../contracts/scheduling/activity.schema.json"),
            include_str!("../../../contracts/scheduling/timeline.schema.json"),
            include_str!("../../../contracts/scheduling/diagnostic.schema.json"),
        ];

        for schema in schemas {
            serde_json::from_str::<serde_json::Value>(schema).expect("schema JSON");
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

        for (name, schema) in [
            (
                "common",
                include_str!("../../../contracts/scheduling/common.schema.json"),
            ),
            (
                "tasks",
                include_str!("../../../contracts/scheduling/tasks.schema.json"),
            ),
            (
                "pools",
                include_str!("../../../contracts/scheduling/pools.schema.json"),
            ),
            (
                "activity",
                include_str!("../../../contracts/scheduling/activity.schema.json"),
            ),
            (
                "timeline",
                include_str!("../../../contracts/scheduling/timeline.schema.json"),
            ),
            (
                "diagnostic",
                include_str!("../../../contracts/scheduling/diagnostic.schema.json"),
            ),
        ] {
            let schema = serde_json::from_str::<serde_json::Value>(schema).expect("schema JSON");
            verify(&schema, name);
        }
    }

    #[test]
    fn published_schema_bounds_match_compiler_constants() {
        let tasks: serde_json::Value = serde_json::from_str(include_str!(
            "../../../contracts/scheduling/tasks.schema.json"
        ))
        .expect("tasks schema");
        let activity: serde_json::Value = serde_json::from_str(include_str!(
            "../../../contracts/scheduling/activity.schema.json"
        ))
        .expect("activity schema");
        let pools: serde_json::Value = serde_json::from_str(include_str!(
            "../../../contracts/scheduling/pools.schema.json"
        ))
        .expect("pools schema");
        let common: serde_json::Value = serde_json::from_str(include_str!(
            "../../../contracts/scheduling/common.schema.json"
        ))
        .expect("common schema");

        assert_eq!(
            tasks["$defs"]["loopBudget"]["properties"]["daily_limit"]["minimum"],
            1
        );
        assert_eq!(
            tasks["$defs"]["loopBudget"]["properties"]["daily_limit"]["maximum"],
            MAX_BUDGET_COUNT
        );
        assert_eq!(
            activity["$defs"]["profile"]["properties"]["daily_budget"]["minimum"],
            1
        );
        assert_eq!(
            activity["$defs"]["profile"]["properties"]["daily_budget"]["maximum"],
            MAX_BUDGET_COUNT
        );
        assert_eq!(
            pools["$defs"]["pool"]["properties"]["projection"]["properties"]["amount"]["minimum"],
            1
        );
        assert_eq!(
            common["$defs"]["predicate"]["oneOf"][5]["properties"]["max_age_ms"]["minimum"],
            1
        );
        assert_eq!(
            common["$defs"]["predicate"]["oneOf"][5]["properties"]["max_age_ms"]["maximum"],
            MAX_FACT_MAX_AGE_MS
        );
        let schedules = common["$defs"]["clockSchedule"]["oneOf"]
            .as_array()
            .expect("clock schedule variants");
        assert_eq!(
            schedules[0]["properties"]["clock_source"]["$ref"],
            "#/$defs/clockSource"
        );
        for schedule in &schedules[1..] {
            assert_eq!(
                schedule["properties"]["clock_source"]["$ref"],
                "#/$defs/wallClockSource"
            );
        }
    }
}
