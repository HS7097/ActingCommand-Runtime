// SPDX-License-Identifier: AGPL-3.0-only

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use actingcommand_selection_policy::SelectionPolicy;
use serde::{Deserialize, Serialize};

use crate::canonical::{canonical_serialized, catalog_hash};
use crate::source::{ParsedDocument, parse_document, parse_selection_document};
use crate::validation::{CatalogSourceMaps, sort_diagnostics, validate_catalog};
use crate::{
    ActivityDocument, CatalogBundle, CatalogDiagnostic, CatalogDiagnosticCode,
    CatalogDocumentSource, CatalogSources, DiagnosticSeverity, MAX_CATALOG_BYTES,
    MAX_DOCUMENT_BYTES, MAX_TEXT_BYTES, MetricRef, ObservationRef, PoolsDocument, PredicateSpec,
    RequiredNullable, SchedulingDocumentKind, SourceLocation, TaskSpec, TasksDocument,
    TimelineDocument,
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
    selection: Option<SelectionPolicy>,
    summary: CatalogIrSummary,
    warnings: Vec<CatalogDiagnostic>,
}

impl CompiledCatalog {
    pub fn catalog(&self) -> &CatalogBundle {
        &self.catalog
    }

    /// The optional fifth document, already validated by the selection-policy crate and
    /// hashed into `catalog_hash`.
    pub fn selection_policy(&self) -> Option<&SelectionPolicy> {
        self.selection.as_ref()
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

    /// Workflow #384 D2-S4: the task spec a dispatch of `task_id` is admitted, budgeted and
    /// settled under: the catalog's task, or the Runtime-owned spec of a fact refresh
    /// ([`crate::FACT_REFRESH_TASK_ID`]).
    pub fn dispatch_task(&self, task_id: &str) -> Option<&TaskSpec> {
        if task_id == crate::FACT_REFRESH_TASK_ID {
            return Some(crate::fact_refresh_task());
        }
        self.catalog
            .tasks
            .tasks
            .iter()
            .find(|task| task.id == task_id)
    }

    /// Workflow #372 K5: the ids `task_id` accounts as under this catalog: `task_id` itself,
    /// then the ids its task declares in `continues_from` (none for an id that is not a task of
    /// this catalog). Every per-(task, instance) state the Runtime derives from its ledger is
    /// read over these ids.
    pub fn accounting_ids<'a>(&'a self, task_id: &'a str) -> Vec<&'a str> {
        let continued = self
            .catalog
            .tasks
            .tasks
            .iter()
            .find(|task| task.id == task_id)
            .and_then(|task| task.continues_from.as_deref())
            .unwrap_or_default();
        std::iter::once(task_id)
            .chain(continued.iter().map(String::as_str))
            .collect()
    }

    /// Workflow #372 K5: the task of this catalog that declares it continues `task_id`, if one
    /// does (the compiler admits at most one).
    pub fn continued_by(&self, task_id: &str) -> Option<&str> {
        self.catalog
            .tasks
            .tasks
            .iter()
            .find(|task| {
                task.continues_from
                    .as_deref()
                    .is_some_and(|continued| continued.iter().any(|id| id == task_id))
            })
            .map(|task| task.id.as_str())
    }

    pub fn referenced_outcome_keys(&self, task_id: &str) -> BTreeSet<String> {
        let mut keys = BTreeSet::new();
        for task in &self.catalog.tasks.tasks {
            collect_predicate_outcome_keys(&task.trigger, task_id, &mut keys);
            collect_predicate_outcome_keys(&task.feedback_stop, task_id, &mut keys);
        }
        for pool in &self.catalog.pools.pools {
            if let ObservationRef::Outcome {
                task_id: referenced_task,
                outcome_key,
            } = &pool.observation
                && referenced_task == task_id
            {
                keys.insert(outcome_key.clone());
            }
        }
        for profile in &self.catalog.activity.profiles {
            for goal in &profile.goals {
                if let MetricRef::Outcome {
                    task_id: referenced_task,
                    outcome_key,
                } = &goal.metric
                    && referenced_task == task_id
                {
                    keys.insert(outcome_key.clone());
                }
            }
        }
        keys
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

fn collect_predicate_outcome_keys(
    predicate: &PredicateSpec,
    task_id: &str,
    keys: &mut BTreeSet<String>,
) {
    match predicate {
        PredicateSpec::All { predicates } | PredicateSpec::Any { predicates } => {
            for predicate in predicates {
                collect_predicate_outcome_keys(predicate, task_id, keys);
            }
        }
        PredicateSpec::Not { predicate } => {
            collect_predicate_outcome_keys(predicate, task_id, keys);
        }
        PredicateSpec::Outcome {
            task_id: referenced_task,
            outcome_key,
            ..
        } if referenced_task == task_id => {
            keys.insert(outcome_key.clone());
        }
        PredicateSpec::Clock { .. }
        | PredicateSpec::TimelineActive { .. }
        | PredicateSpec::ResourceProjection { .. }
        | PredicateSpec::Fact { .. }
        | PredicateSpec::RecordDeadline { .. }
        | PredicateSpec::DependencyCompleted { .. }
        | PredicateSpec::Outcome { .. } => {}
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
    let selection = sources
        .selection
        .as_ref()
        .map(parse_selection_document)
        .transpose();

    let mut parse_diagnostics = Vec::new();
    collect_parse_error(&tasks, &mut parse_diagnostics);
    collect_parse_error(&pools, &mut parse_diagnostics);
    collect_parse_error(&activity, &mut parse_diagnostics);
    collect_parse_error(&timeline, &mut parse_diagnostics);
    if let Err(diagnostic) = &selection {
        parse_diagnostics.push((**diagnostic).clone());
    }
    if !parse_diagnostics.is_empty() {
        return Err(CatalogCompileFailure::new(parse_diagnostics));
    }
    let selection = selection.expect("parse diagnostics were checked");

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
        selection.is_some(),
    );
    if !diagnostics.is_empty() {
        return Err(CatalogCompileFailure::new(diagnostics));
    }

    let hash = catalog_hash(&catalog, selection.as_ref()).map_err(|reason| {
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
        selection,
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
    let mut documents = vec![
        (&sources.tasks, SchedulingDocumentKind::Tasks),
        (&sources.pools, SchedulingDocumentKind::Pools),
        (&sources.activity, SchedulingDocumentKind::Activity),
        (&sources.timeline, SchedulingDocumentKind::Timeline),
    ];
    if let Some(selection) = &sources.selection {
        documents.push((selection, SchedulingDocumentKind::Selection));
    }
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
            selection: None,
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
            "sha256:24dc8c0b4d63c03ad77ebe8a57e91186070ee0d379f17a3374060207cc3f257c"
        );
        assert_eq!(first.summary().counts.tasks, 1);
        let report: serde_json::Value =
            serde_json::from_slice(&first.dry_run_json().expect("dry-run JSON"))
                .expect("dry-run report");
        assert_eq!(report["diagnostic_statistics"]["total"], 0);
        assert_eq!(report["diagnostic_statistics"]["errors"], 0);
        assert_eq!(report["diagnostic_statistics"]["warnings"], 0);
        // SCHEDULING-INSTANCE-ALIAS-v1; first red: Workflow #269 comment 5578055809.
        for version in [
            crate::SCHEDULING_SCHEMA_VERSION,
            crate::SCHEDULING_SCHEMA_VERSION_V2,
        ] {
            for (alias, valid) in [
                ("Neutral.Instance".to_owned(), true),
                (" Instance Ω ".to_owned(), true),
                (" ".to_owned(), true),
                ("é".repeat(128), true),
                ("a".repeat(257), false),
                ("é".repeat(129), false),
                ("".to_owned(), false),
                ("node\n".to_owned(), false),
                ("node\u{85}".to_owned(), false),
            ] {
                let mut sources = example_sources();
                let encoded = serde_json::to_string(&alias).expect("alias JSON");
                for source in [
                    &mut sources.tasks,
                    &mut sources.pools,
                    &mut sources.activity,
                    &mut sources.timeline,
                ] {
                    let text = String::from_utf8(source.bytes.clone())
                        .expect("source UTF-8")
                        .replace("\"fixture-instance-a\"", &encoded)
                        .replace("\"fixture-instance-b\"", &encoded);
                    let mut doc: serde_json::Value =
                        serde_json::from_str(&text).expect("source JSON");
                    doc["schema_version"] = serde_json::json!(version);
                    if version == crate::SCHEDULING_SCHEMA_VERSION_V2
                        && let Some(events) = doc
                            .get_mut("events")
                            .and_then(serde_json::Value::as_array_mut)
                    {
                        for event in events {
                            event["validity"] =
                                serde_json::json!({"from_unix_ms":0,"until_unix_ms":null});
                        }
                    }
                    source.bytes = serde_json::to_vec(&doc).expect("source bytes");
                }
                let compiled = compile_catalog(&sources);
                if valid {
                    let compiled = compiled.expect("registered alias compiles");
                    assert_eq!(
                        compiled.catalog().tasks.tasks[0].scope,
                        crate::ScopeSelector::Instance {
                            instance_id: alias.clone()
                        }
                    );
                    assert_eq!(
                        compiled.catalog().tasks.tasks[0].instance_overrides[0].instance_id,
                        alias
                    );
                    assert_eq!(
                        compiled.catalog_hash(),
                        compile_catalog(&sources).unwrap().catalog_hash()
                    );
                    assert_ne!(compiled.catalog_hash(), first.catalog_hash());
                } else {
                    assert!(compiled.is_err(), "invalid alias {alias:?}");
                }
            }
        }
        for field in ["id", "procedure_ref"] {
            assert!(
                compile_catalog(&mutate_tasks(
                    |doc| doc["tasks"][0][field] = serde_json::json!("Upper.Id")
                ))
                .is_err()
            );
        }
    }

    // Specification criteria: https://github.com/HS7097/ActingCommand-Workflow/issues/269#issuecomment-5551152553
    #[test]
    fn timeline_v2_versions_fields_and_references_are_closed() {
        let v1 = example_sources();
        let original = compile_catalog(&v1).expect("V1 retains its identity");
        let mut v2 = v1.clone();
        for source in [
            &mut v2.tasks,
            &mut v2.pools,
            &mut v2.activity,
            &mut v2.timeline,
        ] {
            let mut doc: serde_json::Value =
                serde_json::from_slice(&source.bytes).expect("neutral document");
            doc["schema_version"] = serde_json::json!(crate::SCHEDULING_SCHEMA_VERSION_V2);
            if let Some(events) = doc
                .get_mut("events")
                .and_then(serde_json::Value::as_array_mut)
            {
                for event in events {
                    event["validity"] = serde_json::json!({"from_unix_ms":0,"until_unix_ms":null});
                }
            }
            source.bytes = serde_json::to_vec(&doc).expect("V2 bytes");
        }
        let timeline: serde_json::Value =
            serde_json::from_slice(&v2.timeline.bytes).expect("timeline");
        let event_id = timeline["events"][0]["id"].clone();
        let mut tasks: serde_json::Value = serde_json::from_slice(&v2.tasks.bytes).expect("tasks");
        tasks["tasks"][0]["trigger"] =
            serde_json::json!({"kind":"timeline_active","event_id":event_id});
        v2.tasks.bytes = serde_json::to_vec(&tasks).expect("tasks bytes");
        let compiled = compile_catalog(&v2).expect("V2 compiles through the existing compiler");
        assert_eq!(
            compiled.summary().schema_version,
            crate::SCHEDULING_SCHEMA_VERSION_V2
        );
        assert_ne!(compiled.catalog_hash(), original.catalog_hash());
        // WF345 R4 specification: one bounded v2 production quantity; zero is known,
        // absence is invalid, and the unchanged v1 catalog retains its canonical hash.
        for (quantity, accepted) in [
            (serde_json::json!({"expected_amount_milli": 0}), true),
            (serde_json::json!({"expected_amount_milli": 400}), true),
            (serde_json::json!({"expected_amount_milli": 120_000}), true),
            (serde_json::json!({"expected_amount_milli": 120_001}), false),
            (
                serde_json::json!({"expected_amount_milli": 9_007_199_254_740_992_u64}),
                false,
            ),
            (serde_json::json!({"expected_amount_milli": null}), false),
            (serde_json::json!({"expected_amount_milli": 0.4}), false),
            (serde_json::json!({"expected_amount_milli": -1}), false),
            (
                serde_json::json!({"amount": 1, "expected_amount_milli": 400}),
                false,
            ),
            (serde_json::json!({}), false),
        ] {
            let mut sources = v2.clone();
            let mut document = tasks.clone();
            let effect = document["tasks"][0]["produces"][0].as_object_mut().unwrap();
            effect.remove("amount");
            effect.extend(quantity.as_object().unwrap().clone());
            sources.tasks.bytes = serde_json::to_vec(&document).unwrap();
            assert_eq!(compile_catalog(&sources).is_ok(), accepted, "{quantity}");
        }
        for (version, member, direction) in [
            (crate::SCHEDULING_SCHEMA_VERSION, "produces", "produce"),
            (crate::SCHEDULING_SCHEMA_VERSION_V2, "consumes", "consume"),
        ] {
            let mut sources = if version == crate::SCHEDULING_SCHEMA_VERSION {
                v1.clone()
            } else {
                v2.clone()
            };
            let mut document: serde_json::Value =
                serde_json::from_slice(&sources.tasks.bytes).unwrap();
            document["tasks"][0][member] = serde_json::json!([{
                "pool_id": "fixture-pool-a", "direction": direction,
                "expected_amount_milli": 400, "observation_source": "self_reported", "confidence_milli": 1000
            }]);
            sources.tasks.bytes = serde_json::to_vec(&document).unwrap();
            assert!(compile_catalog(&sources).is_err(), "{version} {member}");
        }
        for version in [
            crate::SCHEDULING_SCHEMA_VERSION,
            "actingcommand.scheduling.v3",
        ] {
            let mut sources = v2.clone();
            let mut pools: serde_json::Value =
                serde_json::from_slice(&sources.pools.bytes).expect("pools");
            pools["schema_version"] = serde_json::json!(version);
            sources.pools.bytes = serde_json::to_vec(&pools).expect("pools bytes");
            assert!(
                compile_catalog(&sources).is_err(),
                "mixed/unknown version {version}"
            );
        }
        for event_id in [serde_json::json!("missing.event"), serde_json::json!(42)] {
            let mut sources = v2.clone();
            let mut tasks = tasks.clone();
            tasks["tasks"][0]["trigger"]["event_id"] = event_id;
            sources.tasks.bytes = serde_json::to_vec(&tasks).expect("tasks bytes");
            assert!(compile_catalog(&sources).is_err());
        }
        for validity in [
            serde_json::json!(null),
            serde_json::json!({"from_unix_ms":10}),
            serde_json::json!({"from_unix_ms":10,"until_unix_ms":10}),
            serde_json::json!({"from_unix_ms":10,"until_unix_ms":9}),
            serde_json::json!({"from_unix_ms":-1,"until_unix_ms":null}),
            serde_json::json!({"from_unix_ms":0,"until_unix_ms":null,"extra":true}),
            serde_json::json!({"from_unix_ms":9007199254740992_u64,"until_unix_ms":null}),
        ] {
            let mut sources = v2.clone();
            let mut timeline = timeline.clone();
            timeline["events"][0]["validity"] = validity;
            sources.timeline.bytes = serde_json::to_vec(&timeline).expect("timeline bytes");
            assert!(compile_catalog(&sources).is_err());
        }
        let mut missing = v2.clone();
        let mut no_validity = timeline.clone();
        no_validity["events"][0]
            .as_object_mut()
            .expect("event")
            .remove("validity");
        missing.timeline.bytes = serde_json::to_vec(&no_validity).expect("timeline bytes");
        assert!(compile_catalog(&missing).is_err());
        for with_predicate in [false, true] {
            let mut sources = v1.clone();
            if with_predicate {
                tasks["schema_version"] = serde_json::json!(crate::SCHEDULING_SCHEMA_VERSION);
                sources.tasks.bytes = serde_json::to_vec(&tasks).expect("tasks bytes");
            } else {
                let mut timeline = timeline.clone();
                timeline["schema_version"] = serde_json::json!(crate::SCHEDULING_SCHEMA_VERSION);
                sources.timeline.bytes = serde_json::to_vec(&timeline).expect("timeline bytes");
            }
            assert!(compile_catalog(&sources).is_err(), "V1 rejects V2 fields");
        }
    }

    #[test]
    fn outcome_key_closure_finds_nested_any_references_for_the_producing_task() {
        let sources = mutate_tasks(|tasks| {
            tasks["tasks"][0]["feedback_stop"] = serde_json::json!({
                "kind": "any",
                "predicates": [
                    {
                        "kind": "outcome",
                        "task_id": "fixture.observe",
                        "outcome_key": "effect-applied",
                        "comparison": "eq",
                        "value": {"type": "boolean", "value": true}
                    },
                    {
                        "kind": "not",
                        "predicate": {
                            "kind": "outcome",
                            "task_id": "fixture.observe",
                            "outcome_key": "no-effect",
                            "comparison": "eq",
                            "value": {"type": "boolean", "value": false}
                        }
                    }
                ]
            });
        });
        let compiled = compile_catalog(&sources).expect("compile nested outcome consumers");
        assert_eq!(
            compiled.referenced_outcome_keys("fixture.observe"),
            BTreeSet::from(["effect-applied".to_owned(), "no-effect".to_owned()])
        );
        assert!(compiled.referenced_outcome_keys("missing-task").is_empty());
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
        let second = compile_catalog(&sources).expect_err("second rejection");
        assert_eq!(
            error.dry_run_json().expect("first rejection report"),
            second.dry_run_json().expect("second rejection report")
        );
        let report: serde_json::Value =
            serde_json::from_slice(&error.dry_run_json().expect("rejection JSON"))
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
    fn compiler_enforces_frozen_projection_and_budget_boundaries() {
        let projection_zero = mutate_pools(|pools| {
            pools["pools"][0]["projection"]["amount"] = serde_json::json!(0);
        });
        assert!(compile_catalog(&projection_zero).is_err());

        let nonregenerating = mutate_pools(|pools| {
            pools["pools"][0]["projection"] = serde_json::json!({"kind":"none"});
        });
        let compiled = compile_catalog(&nonregenerating).expect("nonregenerating pool");
        assert_eq!(
            serde_json::to_value(&compiled.catalog().pools.pools[0].projection).unwrap(),
            serde_json::json!({"kind":"none"})
        );
        assert_ne!(
            compiled.catalog_hash(),
            compile_catalog(&example_sources()).unwrap().catalog_hash()
        );
        for projection in [
            serde_json::Value::Null,
            serde_json::json!({}),
            serde_json::json!({"kind":"unknown"}),
            serde_json::json!({"kind":"none","amount":1}),
            serde_json::json!({"kind":"none","per_ms":1}),
            serde_json::json!({"kind":"none","unexpected":true}),
            serde_json::json!({"kind":"none","amount":1,"per_ms":1}),
            serde_json::json!({"amount":1,"per_ms":0}),
            serde_json::json!({"amount":1}),
            serde_json::json!({"per_ms":1}),
            serde_json::json!({"amount":1,"per_ms":1,"unexpected":true}),
        ] {
            assert!(
                compile_catalog(&mutate_pools(|pools| {
                    pools["pools"][0]["projection"] = projection.clone();
                }))
                .is_err(),
                "invalid projection {projection}"
            );
        }
        assert!(
            compile_catalog(&mutate_pools(|pools| {
                pools["pools"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("projection");
            }))
            .is_err()
        );
        assert!(
            compile_catalog(&mutate_pools(|pools| {
                pools["pools"][0]["projection"] = serde_json::json!({"kind":"none"});
                pools["pools"][0]["capacity"] = serde_json::json!(0);
            }))
            .is_err()
        );

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

    #[test]
    fn compiler_clock_offsets_match_the_published_independent_bounds() {
        for (utc_offset_minutes, dst_offset_minutes) in [
            (crate::MIN_UTC_OFFSET_MINUTES, crate::MIN_DST_OFFSET_MINUTES),
            (crate::MAX_UTC_OFFSET_MINUTES, crate::MAX_DST_OFFSET_MINUTES),
        ] {
            let sources = mutate_timeline(|timeline| {
                timeline["events"][0]["schedule"]["clock_source"] = serde_json::json!({
                    "kind": "server",
                    "timezone_id": "fixture/zone",
                    "utc_offset_minutes": utc_offset_minutes,
                    "dst_offset_minutes": dst_offset_minutes,
                    "maintenance_drift_ms": 0
                });
            });
            compile_catalog(&sources).expect("published offset boundary must compile");
        }

        for (utc_offset_minutes, dst_offset_minutes) in [(841, 0), (0, 121), (-841, 0), (0, -121)] {
            let sources = mutate_timeline(|timeline| {
                timeline["events"][0]["schedule"]["clock_source"] = serde_json::json!({
                    "kind": "server",
                    "timezone_id": "fixture/zone",
                    "utc_offset_minutes": utc_offset_minutes,
                    "dst_offset_minutes": dst_offset_minutes,
                    "maintenance_drift_ms": 0
                });
            });
            assert!(compile_catalog(&sources).is_err());
        }
    }

    // Workflow #361 R7: the neutral example catalog declared as `version` (a V2 timeline event
    // gets its explicit unbounded validity), with its tasks document changed by `mutator`.
    fn r7_sources(version: &str, mutator: impl FnOnce(&mut serde_json::Value)) -> CatalogSources {
        let mut sources = example_sources();
        for source in [
            &mut sources.tasks,
            &mut sources.pools,
            &mut sources.activity,
            &mut sources.timeline,
        ] {
            let mut document: serde_json::Value =
                serde_json::from_slice(&source.bytes).expect("neutral document");
            document["schema_version"] = serde_json::json!(version);
            if version == crate::SCHEDULING_SCHEMA_VERSION_V2
                && let Some(events) = document
                    .get_mut("events")
                    .and_then(serde_json::Value::as_array_mut)
            {
                for event in events {
                    event["validity"] =
                        serde_json::json!({"from_unix_ms": 0, "until_unix_ms": null});
                }
            }
            source.bytes = serde_json::to_vec(&document).expect("document bytes");
        }
        let mut tasks: serde_json::Value =
            serde_json::from_slice(&sources.tasks.bytes).expect("tasks JSON");
        mutator(&mut tasks);
        sources.tasks.bytes = serde_json::to_vec(&tasks).expect("tasks bytes");
        sources
    }

    // A UTC `daily [06:30]` clock with `missed`.
    fn r7_missed_clock(missed: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "kind": "clock",
            "schedule": {
                "kind": "daily",
                "clock_source": {
                    "kind": "server",
                    "timezone_id": "fixture/utc",
                    "utc_offset_minutes": 0,
                    "dst_offset_minutes": 0,
                    "maintenance_drift_ms": 0
                },
                "minutes_of_day": [390]
            },
            "missed": missed
        })
    }

    // Whether the catalog is refused with `code` at `json_path`.
    fn refused_at(
        sources: &CatalogSources,
        code: CatalogDiagnosticCode,
        json_path: Option<&str>,
    ) -> bool {
        let failure = compile_catalog(sources).expect_err("the catalog is refused");
        failure.diagnostics().iter().any(|diagnostic| {
            diagnostic.code == code
                && json_path.is_none_or(|json_path| diagnostic.json_path == json_path)
        })
    }

    // Workflow #361 R7, row R7-T5 (M361 addendum of 2026-10-15): a grace of 59 999 or
    // 604 800 001 ms is `limit_exceeded` at its pointer, 60 000 and 604 800 000 are admitted; an
    // unknown kind is `type_mismatch`; `missed` in a V1 tasks document is refused as an unknown
    // member; `missed` on a clock under `not` is `predicate_uncomputable` at the clock's pointer.
    #[test]
    fn compiler_enforces_clock_missed_bounds_version_and_placement() {
        let v2 = crate::SCHEDULING_SCHEMA_VERSION_V2;
        let skip_after =
            |grace_ms: u64| serde_json::json!({"kind": "skip_after", "grace_ms": grace_ms});
        for grace_ms in [60_000, 604_800_000] {
            let sources = r7_sources(v2, |tasks| {
                tasks["tasks"][0]["trigger"] = r7_missed_clock(skip_after(grace_ms));
            });
            compile_catalog(&sources)
                .unwrap_or_else(|failure| panic!("grace {grace_ms}: {:?}", failure.diagnostics()));
        }
        for grace_ms in [59_999, 604_800_001] {
            let sources = r7_sources(v2, |tasks| {
                tasks["tasks"][0]["trigger"] = r7_missed_clock(skip_after(grace_ms));
            });
            assert!(
                refused_at(
                    &sources,
                    CatalogDiagnosticCode::LimitExceeded,
                    Some("/tasks/0/trigger/missed/grace_ms"),
                ),
                "grace {grace_ms}"
            );
        }
        let unknown_kind = r7_sources(v2, |tasks| {
            tasks["tasks"][0]["trigger"] =
                r7_missed_clock(serde_json::json!({"kind": "skip_later", "grace_ms": 60_000}));
        });
        assert!(refused_at(
            &unknown_kind,
            CatalogDiagnosticCode::TypeMismatch,
            None
        ));
        let v1 = r7_sources(crate::SCHEDULING_SCHEMA_VERSION, |tasks| {
            tasks["tasks"][0]["trigger"] = r7_missed_clock(skip_after(7_200_000));
        });
        assert!(refused_at(
            &v1,
            CatalogDiagnosticCode::UnknownField,
            Some("/tasks/0/trigger/missed"),
        ));
        let negated = r7_sources(v2, |tasks| {
            tasks["tasks"][0]["trigger"] = serde_json::json!({
                "kind": "not",
                "predicate": r7_missed_clock(skip_after(7_200_000))
            });
        });
        assert!(refused_at(
            &negated,
            CatalogDiagnosticCode::PredicateUncomputable,
            Some("/tasks/0/trigger/predicate"),
        ));
    }

    // Workflow #361 R7, row R7-T6: an absent `missed` serializes as nothing, so the neutral
    // catalog declared as V2 keeps its golden hash. That hash was computed apart from the code
    // under test: the canonical JSON (keys in UTF-16 order, no whitespace) of the four example
    // documents of `contracts/scheduling/examples/catalog-a` under `activity`, `pools`, `tasks`
    // and `timeline`, each with `schema_version` V2 and every timeline event with validity
    // {from 0, until null}, hashed with SHA-256 by a Python script that reproduces the V1
    // golden of `neutral_catalog_compiles_to_stable_hash_and_dry_run` from the same documents.
    // The same catalog with `missed` on one clock has another hash, and its clock re-serializes
    // as it was written.
    #[test]
    fn v2_catalog_identity_is_unchanged_without_missed_and_changes_with_it() {
        let v2 = crate::SCHEDULING_SCHEMA_VERSION_V2;
        let plain = compile_catalog(&r7_sources(v2, |_| {})).expect("the V2 catalog compiles");
        assert_eq!(
            plain.catalog_hash(),
            "sha256:baf684c3f5864fb105ed351c2553831e7af1d76b165bd0a9233db7b26a725047"
        );
        let mut written = None;
        let sources = r7_sources(v2, |tasks| {
            tasks["tasks"][0]["trigger"]["predicates"][0]["missed"] =
                serde_json::json!({"kind": "skip_after", "grace_ms": 600_000});
            written = Some(tasks["tasks"][0]["trigger"].clone());
        });
        let missed = compile_catalog(&sources).expect("the catalog with missed compiles");
        assert_ne!(missed.catalog_hash(), plain.catalog_hash());
        assert_eq!(
            compile_catalog(&sources)
                .expect("the catalog with missed compiles again")
                .catalog_hash(),
            missed.catalog_hash()
        );
        assert_eq!(
            serde_json::to_value(&missed.catalog().tasks.tasks[0].trigger)
                .expect("the trigger serializes"),
            written.expect("the written trigger")
        );
    }

    // Workflow #372 K5: the neutral example catalog declared as `version`, with a second task
    // `fixture.second` (a copy of the first), `continues_from` set on task `index` for each
    // `(index, ids)` of `continued`.
    fn k5_sources(version: &str, continued: &[(usize, serde_json::Value)]) -> CatalogSources {
        r7_sources(version, |tasks| {
            let mut second = tasks["tasks"][0].clone();
            second["id"] = serde_json::json!("fixture.second");
            tasks["tasks"]
                .as_array_mut()
                .expect("the tasks array")
                .push(second);
            for (index, ids) in continued {
                tasks["tasks"][*index]["continues_from"] = ids.clone();
            }
        })
    }

    // Workflow #372 K5, row K5-T7 (bounds half; K372 addendum of 2026-10-17, K5.2): `duplicate_id`
    // at the item's pointer for an active id in `continues_from`, an id listed twice in one task
    // and an id two tasks claim (a split); `limit_exceeded` at the member for `[]` and for 9 ids;
    // in a V1 tasks document the member is refused as an unknown member. One id and 8 ids are
    // admitted.
    #[test]
    fn compiler_enforces_continues_from_bounds_version_and_identity() {
        let v2 = crate::SCHEDULING_SCHEMA_VERSION_V2;
        let ids = |count: usize| {
            serde_json::json!(
                (0..count)
                    .map(|index| format!("fixture.retired-{index}"))
                    .collect::<Vec<_>>()
            )
        };
        for count in [1, 8] {
            compile_catalog(&k5_sources(v2, &[(0, ids(count))])).unwrap_or_else(|failure| {
                panic!("{count} continued ids: {:?}", failure.diagnostics())
            });
        }
        for (case, continued, json_path) in [
            (
                "an active id",
                vec![(0, serde_json::json!(["fixture.second"]))],
                "/tasks/0/continues_from/0",
            ),
            (
                "an id listed twice",
                vec![(
                    0,
                    serde_json::json!(["fixture.retired-0", "fixture.retired-0"]),
                )],
                "/tasks/0/continues_from/1",
            ),
            (
                "an id claimed by two tasks",
                vec![
                    (0, serde_json::json!(["fixture.retired-0"])),
                    (1, serde_json::json!(["fixture.retired-0"])),
                ],
                "/tasks/1/continues_from/0",
            ),
        ] {
            assert!(
                refused_at(
                    &k5_sources(v2, &continued),
                    CatalogDiagnosticCode::DuplicateId,
                    Some(json_path),
                ),
                "{case}"
            );
        }
        for count in [0, 9] {
            assert!(
                refused_at(
                    &k5_sources(v2, &[(0, ids(count))]),
                    CatalogDiagnosticCode::LimitExceeded,
                    Some("/tasks/0/continues_from"),
                ),
                "{count} continued ids"
            );
        }
        assert!(refused_at(
            &k5_sources(crate::SCHEDULING_SCHEMA_VERSION, &[(0, ids(1))]),
            CatalogDiagnosticCode::UnknownField,
            Some("/tasks/0/continues_from"),
        ));
    }

    // Workflow #372 K5, row K5-T7 (identity half): an absent `continues_from` serializes as
    // nothing, so the neutral catalog declared as V2 keeps the golden hash R7-T6 computed for it
    // apart from the code under test. The same catalog with the member on its task has another
    // hash, the same on every compile, and its task re-serializes as it was written.
    #[test]
    fn v2_catalog_identity_is_unchanged_without_continues_from_and_changes_with_it() {
        let v2 = crate::SCHEDULING_SCHEMA_VERSION_V2;
        let plain = compile_catalog(&r7_sources(v2, |_| {})).expect("the V2 catalog compiles");
        assert_eq!(
            plain.catalog_hash(),
            "sha256:baf684c3f5864fb105ed351c2553831e7af1d76b165bd0a9233db7b26a725047"
        );
        let mut written = None;
        let sources = r7_sources(v2, |tasks| {
            tasks["tasks"][0]["continues_from"] = serde_json::json!(["fixture.retired-0"]);
            written = Some(tasks["tasks"][0].clone());
        });
        let continuing = compile_catalog(&sources).expect("the catalog with the member compiles");
        assert_ne!(continuing.catalog_hash(), plain.catalog_hash());
        assert_eq!(
            compile_catalog(&sources)
                .expect("the catalog with the member compiles again")
                .catalog_hash(),
            continuing.catalog_hash()
        );
        assert_eq!(
            serde_json::to_value(&continuing.catalog().tasks.tasks[0])
                .expect("the task serializes"),
            written.expect("the written task")
        );
    }
}
