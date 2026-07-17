// SPDX-License-Identifier: AGPL-3.0-only

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::canonical::{canonical_serialized, catalog_hash};
use crate::source::{ParsedDocument, parse_document};
use crate::validation::{CatalogSourceMaps, sort_diagnostics, validate_catalog};
use crate::{
    ActivityDocument, CatalogBundle, CatalogDiagnostic, CatalogDiagnosticCode,
    CatalogDocumentSource, CatalogSources, DiagnosticSeverity, MAX_CATALOG_BYTES,
    MAX_DOCUMENT_BYTES, MAX_TEXT_BYTES, PoolsDocument, RequiredNullable, SchedulingDocumentKind,
    SourceLocation, TasksDocument, TimelineDocument,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogIrSummary {
    pub schema_version: String,
    pub catalog_id: String,
    pub catalog_version: u64,
    pub catalog_hash: String,
    pub approval_refs: Vec<String>,
    pub counts: CatalogCounts,
    pub task_ids: Vec<String>,
    pub pool_ids: Vec<String>,
    pub activity_profile_ids: Vec<String>,
    pub timeline_event_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogCounts {
    pub tasks: usize,
    pub pools: usize,
    pub activity_profiles: usize,
    pub timeline_events: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticStatistics {
    pub total: usize,
    pub errors: usize,
    pub warnings: usize,
    pub by_code: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledCatalog {
    catalog: CatalogBundle,
    summary: CatalogIrSummary,
    warnings: Vec<CatalogDiagnostic>,
}

impl CompiledCatalog {
    pub fn catalog(&self) -> &CatalogBundle {
        &self.catalog
    }

    pub fn into_catalog(self) -> CatalogBundle {
        self.catalog
    }

    pub fn summary(&self) -> &CatalogIrSummary {
        &self.summary
    }

    pub fn catalog_hash(&self) -> &str {
        &self.summary.catalog_hash
    }

    pub fn warnings(&self) -> &[CatalogDiagnostic] {
        &self.warnings
    }

    pub fn dry_run_json(&self) -> Result<Vec<u8>, DryRunSerializationError> {
        canonical_serialized(&AcceptedDryRunReport {
            status: DryRunStatus::Accepted,
            summary: &self.summary,
            warnings: &self.warnings,
            diagnostic_statistics: diagnostic_statistics(&self.warnings),
        })
        .map_err(DryRunSerializationError)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogCompileFailure {
    diagnostics: Vec<CatalogDiagnostic>,
}

impl CatalogCompileFailure {
    pub fn diagnostics(&self) -> &[CatalogDiagnostic] {
        &self.diagnostics
    }

    pub fn dry_run_json(&self) -> Result<Vec<u8>, DryRunSerializationError> {
        canonical_serialized(&RejectedDryRunReport {
            status: DryRunStatus::Rejected,
            diagnostics: &self.diagnostics,
            diagnostic_statistics: diagnostic_statistics(&self.diagnostics),
        })
        .map_err(DryRunSerializationError)
    }

    fn new(mut diagnostics: Vec<CatalogDiagnostic>) -> Self {
        sort_diagnostics(&mut diagnostics);
        Self { diagnostics }
    }
}

impl fmt::Display for CatalogCompileFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "scheduling catalog rejected with {} diagnostic(s)",
            self.diagnostics.len()
        )
    }
}

impl Error for CatalogCompileFailure {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DryRunSerializationError(String);

impl fmt::Display for DryRunSerializationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "failed to serialize scheduling dry-run: {}",
            self.0
        )
    }
}

impl Error for DryRunSerializationError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum DryRunStatus {
    Accepted,
    Rejected,
}

#[derive(Serialize)]
struct AcceptedDryRunReport<'a> {
    status: DryRunStatus,
    summary: &'a CatalogIrSummary,
    warnings: &'a [CatalogDiagnostic],
    diagnostic_statistics: DiagnosticStatistics,
}

#[derive(Serialize)]
struct RejectedDryRunReport<'a> {
    status: DryRunStatus,
    diagnostics: &'a [CatalogDiagnostic],
    diagnostic_statistics: DiagnosticStatistics,
}

fn diagnostic_statistics(diagnostics: &[CatalogDiagnostic]) -> DiagnosticStatistics {
    let mut by_code = BTreeMap::new();
    let mut errors = 0;
    let mut warnings = 0;
    for diagnostic in diagnostics {
        match diagnostic.severity {
            DiagnosticSeverity::Error => errors += 1,
            DiagnosticSeverity::Warning => warnings += 1,
        }
        let code = serde_json::to_value(diagnostic.code)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .expect("catalog diagnostic codes serialize as strings");
        *by_code.entry(code).or_insert(0) += 1;
    }
    DiagnosticStatistics {
        total: diagnostics.len(),
        errors,
        warnings,
        by_code,
    }
}

pub fn compile_catalog(sources: &CatalogSources) -> Result<CompiledCatalog, CatalogCompileFailure> {
    let preflight = preflight_sources(sources);
    if !preflight.is_empty() {
        return Err(CatalogCompileFailure::new(preflight));
    }

    let tasks = parse_document::<TasksDocument>(&sources.tasks, SchedulingDocumentKind::Tasks);
    let pools = parse_document::<PoolsDocument>(&sources.pools, SchedulingDocumentKind::Pools);
    let activity =
        parse_document::<ActivityDocument>(&sources.activity, SchedulingDocumentKind::Activity);
    let timeline =
        parse_document::<TimelineDocument>(&sources.timeline, SchedulingDocumentKind::Timeline);

    let mut parse_diagnostics = Vec::new();
    collect_parse_error(&tasks, &mut parse_diagnostics);
    collect_parse_error(&pools, &mut parse_diagnostics);
    collect_parse_error(&activity, &mut parse_diagnostics);
    collect_parse_error(&timeline, &mut parse_diagnostics);
    if !parse_diagnostics.is_empty() {
        return Err(CatalogCompileFailure::new(parse_diagnostics));
    }

    let ParsedDocument {
        value: tasks,
        source_map: tasks_map,
    } = tasks.expect("parse diagnostics were checked");
    let ParsedDocument {
        value: pools,
        source_map: pools_map,
    } = pools.expect("parse diagnostics were checked");
    let ParsedDocument {
        value: activity,
        source_map: activity_map,
    } = activity.expect("parse diagnostics were checked");
    let ParsedDocument {
        value: timeline,
        source_map: timeline_map,
    } = timeline.expect("parse diagnostics were checked");
    let catalog = CatalogBundle {
        tasks,
        pools,
        activity,
        timeline,
    };

    let diagnostics = validate_catalog(
        &catalog,
        CatalogSourceMaps {
            tasks: &tasks_map,
            pools: &pools_map,
            activity: &activity_map,
            timeline: &timeline_map,
        },
    );
    if !diagnostics.is_empty() {
        return Err(CatalogCompileFailure::new(diagnostics));
    }

    let hash = catalog_hash(&catalog).map_err(|reason| {
        CatalogCompileFailure::new(vec![tasks_map.diagnostic(
            CatalogDiagnosticCode::TypeMismatch,
            "",
            reason,
            Some((
                catalog.tasks.catalog.catalog_id.as_str(),
                catalog.tasks.catalog.catalog_version,
            )),
        )])
    })?;
    let summary = build_summary(&catalog, hash);
    Ok(CompiledCatalog {
        catalog,
        summary,
        warnings: Vec::new(),
    })
}

fn collect_parse_error<T>(
    result: &Result<ParsedDocument<T>, Box<CatalogDiagnostic>>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    if let Err(diagnostic) = result {
        diagnostics.push((**diagnostic).clone());
    }
}

fn preflight_sources(sources: &CatalogSources) -> Vec<CatalogDiagnostic> {
    let documents = [
        (&sources.tasks, SchedulingDocumentKind::Tasks),
        (&sources.pools, SchedulingDocumentKind::Pools),
        (&sources.activity, SchedulingDocumentKind::Activity),
        (&sources.timeline, SchedulingDocumentKind::Timeline),
    ];
    let mut diagnostics = Vec::new();
    let total_bytes = documents.iter().fold(0_usize, |total, (source, _)| {
        total.saturating_add(source.bytes.len())
    });
    if total_bytes > MAX_CATALOG_BYTES {
        diagnostics.push(preflight_diagnostic(
            &sources.tasks,
            SchedulingDocumentKind::Tasks,
            CatalogDiagnosticCode::CatalogTooLarge,
            format!("catalog size {total_bytes} exceeds {MAX_CATALOG_BYTES} bytes"),
        ));
    }
    for (source, kind) in documents {
        if source.bytes.len() > MAX_DOCUMENT_BYTES {
            diagnostics.push(preflight_diagnostic(
                source,
                kind,
                CatalogDiagnosticCode::DocumentTooLarge,
                format!(
                    "document size {} exceeds {MAX_DOCUMENT_BYTES} bytes",
                    source.bytes.len()
                ),
            ));
        }
        if source.source_uri.is_empty() || source.source_uri.len() > MAX_TEXT_BYTES {
            diagnostics.push(preflight_diagnostic(
                source,
                kind,
                CatalogDiagnosticCode::LimitExceeded,
                format!("source URI must contain 1..={MAX_TEXT_BYTES} UTF-8 bytes"),
            ));
        }
    }
    diagnostics
}

fn preflight_diagnostic(
    source: &CatalogDocumentSource,
    document: SchedulingDocumentKind,
    code: CatalogDiagnosticCode,
    reason: String,
) -> CatalogDiagnostic {
    CatalogDiagnostic {
        code,
        severity: DiagnosticSeverity::Error,
        json_path: String::new(),
        source: SourceLocation {
            document,
            source_uri: truncate_utf8(&source.source_uri, MAX_TEXT_BYTES).to_owned(),
            line: 1,
            column: 1,
        },
        reason,
        schema_version: RequiredNullable(None),
        catalog_id: RequiredNullable(None),
        catalog_version: RequiredNullable(None),
    }
}

fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    let mut end = value.len().min(max_bytes);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn build_summary(catalog: &CatalogBundle, catalog_hash: String) -> CatalogIrSummary {
    let mut task_ids: Vec<String> = catalog
        .tasks
        .tasks
        .iter()
        .map(|task| task.id.clone())
        .collect();
    let mut pool_ids: Vec<String> = catalog
        .pools
        .pools
        .iter()
        .map(|pool| pool.id.clone())
        .collect();
    let mut activity_profile_ids: Vec<String> = catalog
        .activity
        .profiles
        .iter()
        .map(|profile| profile.id.clone())
        .collect();
    let mut timeline_event_ids: Vec<String> = catalog
        .timeline
        .events
        .iter()
        .map(|event| event.id.clone())
        .collect();
    task_ids.sort();
    pool_ids.sort();
    activity_profile_ids.sort();
    timeline_event_ids.sort();

    CatalogIrSummary {
        schema_version: catalog.tasks.schema_version.clone(),
        catalog_id: catalog.tasks.catalog.catalog_id.clone(),
        catalog_version: catalog.tasks.catalog.catalog_version,
        catalog_hash,
        approval_refs: catalog.tasks.catalog.approval_refs.clone(),
        counts: CatalogCounts {
            tasks: task_ids.len(),
            pools: pool_ids.len(),
            activity_profiles: activity_profile_ids.len(),
            timeline_events: timeline_event_ids.len(),
        },
        task_ids,
        pool_ids,
        activity_profile_ids,
        timeline_event_ids,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example_sources() -> CatalogSources {
        CatalogSources {
            tasks: CatalogDocumentSource::new(
                "memory://catalog-a/tasks.json",
                include_bytes!("../../../contracts/scheduling/examples/catalog-a/tasks.json")
                    .to_vec(),
            ),
            pools: CatalogDocumentSource::new(
                "memory://catalog-a/pools.json",
                include_bytes!("../../../contracts/scheduling/examples/catalog-a/pools.json")
                    .to_vec(),
            ),
            activity: CatalogDocumentSource::new(
                "memory://catalog-a/activity.json",
                include_bytes!("../../../contracts/scheduling/examples/catalog-a/activity.json")
                    .to_vec(),
            ),
            timeline: CatalogDocumentSource::new(
                "memory://catalog-a/timeline.json",
                include_bytes!("../../../contracts/scheduling/examples/catalog-a/timeline.json")
                    .to_vec(),
            ),
        }
    }

    fn mutate_tasks(mutator: impl FnOnce(&mut serde_json::Value)) -> CatalogSources {
        let mut sources = example_sources();
        let mut tasks: serde_json::Value =
            serde_json::from_slice(&sources.tasks.bytes).expect("tasks JSON");
        mutator(&mut tasks);
        sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("tasks bytes");
        sources
    }

    fn mutate_pools(mutator: impl FnOnce(&mut serde_json::Value)) -> CatalogSources {
        let mut sources = example_sources();
        let mut pools: serde_json::Value =
            serde_json::from_slice(&sources.pools.bytes).expect("pools JSON");
        mutator(&mut pools);
        sources.pools.bytes = serde_json::to_vec_pretty(&pools).expect("pools bytes");
        sources
    }

    fn mutate_activity(mutator: impl FnOnce(&mut serde_json::Value)) -> CatalogSources {
        let mut sources = example_sources();
        let mut activity: serde_json::Value =
            serde_json::from_slice(&sources.activity.bytes).expect("activity JSON");
        mutator(&mut activity);
        sources.activity.bytes = serde_json::to_vec_pretty(&activity).expect("activity bytes");
        sources
    }

    fn mutate_timeline(mutator: impl FnOnce(&mut serde_json::Value)) -> CatalogSources {
        let mut sources = example_sources();
        let mut timeline: serde_json::Value =
            serde_json::from_slice(&sources.timeline.bytes).expect("timeline JSON");
        mutator(&mut timeline);
        sources.timeline.bytes = serde_json::to_vec_pretty(&timeline).expect("timeline bytes");
        sources
    }

    #[test]
    fn neutral_catalog_compiles_to_stable_hash_and_dry_run() {
        let first = compile_catalog(&example_sources()).expect("first compile");
        let second = compile_catalog(&example_sources()).expect("second compile");
        assert_eq!(first.catalog_hash(), second.catalog_hash());
        assert_eq!(
            first.dry_run_json().expect("first dry-run"),
            second.dry_run_json().expect("second dry-run")
        );
        assert!(first.catalog_hash().starts_with("sha256:"));
        assert_eq!(
            first.catalog_hash(),
            "sha256:9ee4623e6057a650960ca1bd5287e4b4c6e042429ab31a93d3b95cf3aebbc7c4"
        );
        assert_eq!(first.summary().counts.tasks, 1);
        let report: serde_json::Value =
            serde_json::from_slice(&first.dry_run_json().expect("dry-run JSON"))
                .expect("dry-run report");
        assert_eq!(report["diagnostic_statistics"]["total"], 0);
        assert_eq!(report["diagnostic_statistics"]["errors"], 0);
        assert_eq!(report["diagnostic_statistics"]["warnings"], 0);
    }

    #[test]
    fn legacy_schema_version_rejects_the_complete_catalog() {
        let sources = mutate_tasks(|tasks| {
            tasks["schema_version"] = serde_json::json!("task-catalog.v0-draft");
        });
        let error = compile_catalog(&sources).expect_err("legacy schema must fail");
        assert!(error.diagnostics().iter().any(|diagnostic| {
            diagnostic.code == CatalogDiagnosticCode::UnsupportedSchemaVersion
                && diagnostic.json_path == "/schema_version"
        }));
    }

    #[test]
    fn dangling_pool_reference_rejects_the_complete_catalog() {
        let sources = mutate_tasks(|tasks| {
            tasks["tasks"][0]["produces"][0]["pool_id"] = serde_json::json!("fixture-pool-missing");
        });
        let error = compile_catalog(&sources).expect_err("dangling reference must fail");
        assert!(
            error
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == CatalogDiagnosticCode::DanglingReference)
        );
    }

    #[test]
    fn zero_loop_budget_rejects_the_complete_catalog() {
        let sources = mutate_tasks(|tasks| {
            tasks["tasks"][0]["loop_budget"]["daily_limit"] = serde_json::json!(0);
        });
        let error = compile_catalog(&sources).expect_err("unbounded loop must fail");
        assert!(
            error
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == CatalogDiagnosticCode::LoopBudgetMissing)
        );
    }

    #[test]
    fn conflicting_equalities_are_statically_unreachable() {
        let sources = mutate_tasks(|tasks| {
            let fact = tasks["tasks"][0]["trigger"]["predicates"][1].clone();
            tasks["tasks"][0]["trigger"] = serde_json::json!({
                "kind": "all",
                "predicates": [
                    fact,
                    {
                        "kind": "fact",
                        "scope": {"kind": "instance", "instance_id": "fixture-instance-a"},
                        "fact_key": "resource.primary",
                        "comparison": "eq",
                        "value": {"type": "integer", "value": 99},
                        "max_age_ms": 900000
                    },
                    {
                        "kind": "fact",
                        "scope": {"kind": "instance", "instance_id": "fixture-instance-a"},
                        "fact_key": "resource.primary",
                        "comparison": "eq",
                        "value": {"type": "integer", "value": 100},
                        "max_age_ms": 900000
                    }
                ]
            });
        });
        let error = compile_catalog(&sources).expect_err("conflicting predicate must fail");
        assert!(
            error
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == CatalogDiagnosticCode::PredicateUnreachable)
        );
    }

    #[test]
    fn effect_direction_mismatch_is_rejected() {
        let sources = mutate_tasks(|tasks| {
            tasks["tasks"][0]["produces"][0]["direction"] = serde_json::json!("consume");
        });
        let error = compile_catalog(&sources).expect_err("effect mismatch must fail");
        assert!(
            error
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == CatalogDiagnosticCode::EffectIncompatible)
        );
    }

    #[test]
    fn missing_required_field_is_rejected_before_ir_creation() {
        let sources = mutate_tasks(|tasks| {
            tasks["tasks"][0]
                .as_object_mut()
                .expect("task object")
                .remove("feedback_stop");
        });
        let error = compile_catalog(&sources).expect_err("missing field must fail");
        assert!(error.diagnostics().iter().any(|diagnostic| {
            diagnostic.code == CatalogDiagnosticCode::MissingRequiredField
                && diagnostic.source.line > 0
                && diagnostic.source.column > 0
        }));
    }

    #[test]
    fn incompatible_comparison_is_rejected_as_uncomputable() {
        let sources = mutate_tasks(|tasks| {
            tasks["tasks"][0]["trigger"]["predicates"][1]["comparison"] =
                serde_json::json!("contains");
        });
        let error = compile_catalog(&sources).expect_err("uncomputable comparison must fail");
        assert!(
            error
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == CatalogDiagnosticCode::PredicateUncomputable)
        );
    }

    #[test]
    fn oversized_document_is_rejected_before_parsing() {
        let mut sources = example_sources();
        sources.tasks.bytes = vec![b' '; MAX_DOCUMENT_BYTES + 1];
        let error = compile_catalog(&sources).expect_err("oversized document must fail");
        assert!(
            error
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == CatalogDiagnosticCode::DocumentTooLarge)
        );
    }

    #[test]
    fn rejection_dry_run_is_byte_stable() {
        let sources = mutate_tasks(|tasks| {
            tasks["tasks"][0]["produces"][0]["pool_id"] = serde_json::json!("fixture-pool-missing");
        });
        let first = compile_catalog(&sources).expect_err("first rejection");
        let second = compile_catalog(&sources).expect_err("second rejection");
        assert_eq!(
            first.dry_run_json().expect("first rejection report"),
            second.dry_run_json().expect("second rejection report")
        );
        let report: serde_json::Value =
            serde_json::from_slice(&first.dry_run_json().expect("rejection JSON"))
                .expect("rejection report");
        assert!(
            report["diagnostic_statistics"]["total"]
                .as_u64()
                .is_some_and(|count| count > 0)
        );
        assert!(
            report["diagnostic_statistics"]["by_code"]["dangling_reference"]
                .as_u64()
                .is_some_and(|count| count > 0)
        );
    }

    #[test]
    fn compiler_enforces_frozen_projection_and_budget_boundaries() {
        let projection_zero = mutate_pools(|pools| {
            pools["pools"][0]["projection"]["amount"] = serde_json::json!(0);
        });
        assert!(compile_catalog(&projection_zero).is_err());

        for value in [1_u32, crate::MAX_BUDGET_COUNT] {
            let tasks = mutate_tasks(|tasks| {
                tasks["tasks"][0]["loop_budget"]["daily_limit"] = serde_json::json!(value);
                tasks["tasks"][0]["loop_budget"]["window_iteration_limit"] =
                    serde_json::json!(value);
            });
            compile_catalog(&tasks).expect("task budget boundary must compile");

            let activity = mutate_activity(|activity| {
                activity["profiles"][0]["daily_budget"] = serde_json::json!(value);
                activity["profiles"][0]["max_window_iterations"] = serde_json::json!(value);
            });
            compile_catalog(&activity).expect("activity budget boundary must compile");
        }

        for value in [0_u64, u64::from(crate::MAX_BUDGET_COUNT) + 1] {
            let tasks = mutate_tasks(|tasks| {
                tasks["tasks"][0]["loop_budget"]["daily_limit"] = serde_json::json!(value);
            });
            assert!(compile_catalog(&tasks).is_err());

            let activity = mutate_activity(|activity| {
                activity["profiles"][0]["daily_budget"] = serde_json::json!(value);
            });
            assert!(compile_catalog(&activity).is_err());
        }
    }

    #[test]
    fn compiler_enforces_fact_freshness_boundaries() {
        for max_age_ms in [
            serde_json::Value::Null,
            serde_json::json!(1),
            serde_json::json!(crate::MAX_FACT_MAX_AGE_MS),
        ] {
            let sources = mutate_tasks(|tasks| {
                tasks["tasks"][0]["trigger"] = serde_json::json!({
                    "kind": "fact",
                    "scope": {"kind": "instance", "instance_id": "fixture-instance-a"},
                    "fact_key": "env.ui_theme",
                    "comparison": "eq",
                    "value": {"type": "string", "value": "Neutral"},
                    "max_age_ms": max_age_ms
                });
            });
            compile_catalog(&sources).expect("fact freshness boundary must compile");
        }

        for max_age_ms in [0, crate::MAX_FACT_MAX_AGE_MS + 1] {
            let sources = mutate_tasks(|tasks| {
                tasks["tasks"][0]["trigger"] = serde_json::json!({
                    "kind": "fact",
                    "scope": {"kind": "instance", "instance_id": "fixture-instance-a"},
                    "fact_key": "env.ui_theme",
                    "comparison": "eq",
                    "value": {"type": "string", "value": "Neutral"},
                    "max_age_ms": max_age_ms
                });
            });
            assert!(compile_catalog(&sources).is_err());
        }
    }

    #[test]
    fn local_clock_rejects_absolute_schedule_and_reveal_identity_changes_hash() {
        let local_at = mutate_timeline(|timeline| {
            timeline["events"][0]["schedule"] = serde_json::json!({
                "kind": "at",
                "clock_source": {"kind": "local"},
                "at_ms": 1
            });
        });
        assert!(compile_catalog(&local_at).is_err());

        let first = mutate_timeline(|timeline| {
            timeline["events"][0]["schedule"]["clock_source"] = serde_json::json!({
                "kind": "reveal",
                "reveal_source": "evidence:alpha",
                "timezone_id": "fixture/zone",
                "utc_offset_minutes": 0,
                "dst_offset_minutes": 0,
                "maintenance_drift_ms": 0
            });
        });
        let second = mutate_timeline(|timeline| {
            timeline["events"][0]["schedule"]["clock_source"] = serde_json::json!({
                "kind": "reveal",
                "reveal_source": "evidence:beta",
                "timezone_id": "fixture/zone",
                "utc_offset_minutes": 0,
                "dst_offset_minutes": 0,
                "maintenance_drift_ms": 0
            });
        });
        assert_ne!(
            compile_catalog(&first)
                .expect("first reveal catalog")
                .catalog_hash(),
            compile_catalog(&second)
                .expect("second reveal catalog")
                .catalog_hash()
        );
    }
}
