// SPDX-License-Identifier: AGPL-3.0-only

//! Runtime bridge from immutable strategic reports to the existing proposal compiler.

use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::{
    CatalogDeclarationPatch, CatalogProposal, MAX_PROPOSAL_PATCHES, ProjectedArtifactReference,
    ProposalDocument, ProposalKind, ProposalPatchOperation, ProposalPreview, RuntimeErrorCode,
};
use actingcommand_policy::{
    ActivityDocument, CatalogSources, ScopeSelector, StrategicProjection, TasksDocument,
};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrategicPlanPreparation {
    report: ProjectedArtifactReference,
    projection: StrategicProjection,
    proposal: Option<CatalogProposal>,
    preview: Option<ProposalPreview>,
}

impl StrategicPlanPreparation {
    pub(crate) fn new(
        report: ProjectedArtifactReference,
        projection: StrategicProjection,
        proposal: Option<CatalogProposal>,
        preview: Option<ProposalPreview>,
    ) -> RuntimeHostResult<Self> {
        if proposal.is_some() != preview.is_some() {
            return Err(fatal("strategy_plan_shape_invalid"));
        }
        Ok(Self {
            report,
            projection,
            proposal,
            preview,
        })
    }

    pub const fn report(&self) -> &ProjectedArtifactReference {
        &self.report
    }

    pub const fn projection(&self) -> &StrategicProjection {
        &self.projection
    }

    pub const fn proposal(&self) -> Option<&CatalogProposal> {
        self.proposal.as_ref()
    }

    pub const fn preview(&self) -> Option<&ProposalPreview> {
        self.preview.as_ref()
    }
}

pub(crate) fn build_strategy_proposal(
    projection: &StrategicProjection,
    sources: &CatalogSources,
    report: ProjectedArtifactReference,
) -> RuntimeHostResult<Option<CatalogProposal>> {
    let instances = projection
        .instances
        .iter()
        .map(|instance| instance.instance_id.as_str())
        .collect::<BTreeSet<_>>();
    let tasks: TasksDocument = serde_json::from_slice(&sources.tasks.bytes)
        .map_err(|_| fatal("strategy_catalog_source_invalid"))?;
    let activity: ActivityDocument = serde_json::from_slice(&sources.activity.bytes)
        .map_err(|_| fatal("strategy_catalog_source_invalid"))?;
    let task_removals = tasks
        .tasks
        .iter()
        .enumerate()
        .filter_map(|(index, task)| {
            task.id
                .strip_prefix("strategy.task.")
                .is_some_and(|suffix| !suffix.is_empty())
                .then_some(())
                .filter(|_| {
                    matches!(
                        &task.scope,
                        ScopeSelector::Instance { instance_id }
                            if instances.contains(instance_id.as_str())
                    )
                })
                .map(|_| index)
        })
        .collect::<Vec<_>>();
    let profile_removals = activity
        .profiles
        .iter()
        .enumerate()
        .filter_map(|(index, profile)| {
            profile
                .id
                .strip_prefix("strategy.profile.")
                .is_some_and(|suffix| !suffix.is_empty())
                .then_some(())
                .filter(|_| {
                    matches!(
                        &profile.scope,
                        ScopeSelector::Instance { instance_id }
                            if instances.contains(instance_id.as_str())
                    )
                })
                .map(|_| index)
        })
        .collect::<Vec<_>>();
    let mut task_additions = projection.additions.tasks.iter().collect::<Vec<_>>();
    task_additions.sort_by(|left, right| left.id.cmp(&right.id));
    let mut profile_additions = projection
        .additions
        .activity_profiles
        .iter()
        .collect::<Vec<_>>();
    profile_additions.sort_by(|left, right| left.id.cmp(&right.id));
    if task_removals.is_empty()
        && profile_removals.is_empty()
        && task_additions.is_empty()
        && profile_additions.is_empty()
    {
        return Ok(None);
    }
    let patch_count = task_removals
        .len()
        .checked_add(profile_removals.len())
        .and_then(|value| value.checked_add(4))
        .and_then(|value| value.checked_add(task_additions.len()))
        .and_then(|value| value.checked_add(profile_additions.len()))
        .ok_or_else(|| request("strategy_patch_budget_exceeded"))?;
    if patch_count > MAX_PROPOSAL_PATCHES {
        return Err(request("strategy_patch_budget_exceeded"));
    }
    let mut patches = Vec::with_capacity(patch_count);
    for index in task_removals.into_iter().rev() {
        patches.push(remove_patch(
            ProposalDocument::Tasks,
            format!("/tasks/{index}"),
        )?);
    }
    for index in profile_removals.into_iter().rev() {
        patches.push(remove_patch(
            ProposalDocument::Activity,
            format!("/profiles/{index}"),
        )?);
    }
    patches.extend(version_patches(projection.target_catalog_version)?);
    for task in task_additions {
        patches.push(add_patch(ProposalDocument::Tasks, "/tasks/-", task)?);
    }
    for profile in profile_additions {
        patches.push(add_patch(
            ProposalDocument::Activity,
            "/profiles/-",
            profile,
        )?);
    }
    CatalogProposal::new(
        &projection.catalog_hash,
        projection.catalog_version,
        projection.target_catalog_version,
        vec![report],
        ProposalKind::CatalogDiff { patches },
    )
    .map(Some)
    .map_err(|_| request("strategy_proposal_invalid"))
}

fn remove_patch(
    document: ProposalDocument,
    path: String,
) -> RuntimeHostResult<CatalogDeclarationPatch> {
    CatalogDeclarationPatch::new(document, ProposalPatchOperation::Remove, path, None)
        .map_err(|_| request("strategy_patch_invalid"))
}

fn version_patches(version: u64) -> RuntimeHostResult<Vec<CatalogDeclarationPatch>> {
    [
        ProposalDocument::Tasks,
        ProposalDocument::Pools,
        ProposalDocument::Activity,
        ProposalDocument::Timeline,
    ]
    .into_iter()
    .map(|document| {
        CatalogDeclarationPatch::new(
            document,
            ProposalPatchOperation::Replace,
            "/catalog/catalog_version",
            Some(version.to_string()),
        )
        .map_err(|_| fatal("strategy_version_patch_invalid"))
    })
    .collect()
}

fn add_patch(
    document: ProposalDocument,
    path: &'static str,
    value: &impl serde::Serialize,
) -> RuntimeHostResult<CatalogDeclarationPatch> {
    let value = serde_json::to_string(value).map_err(|_| fatal("strategy_patch_encode_failed"))?;
    CatalogDeclarationPatch::new(document, ProposalPatchOperation::Add, path, Some(value))
        .map_err(|_| request("strategy_patch_invalid"))
}

fn request(code: &'static str) -> RuntimeHostError {
    RuntimeHostError::request(
        code,
        "prepare_strategic_report",
        RuntimeErrorCode::InvalidRequest,
    )
}

fn fatal(code: &'static str) -> RuntimeHostError {
    RuntimeHostError::fatal(
        code,
        "prepare_strategic_report",
        RuntimeErrorCode::RuntimeFatal,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_contract::{
        ArtifactKind, ArtifactMediaType, ArtifactProducer, ArtifactRedactionState,
        IdentifierIssuer, ProposalKind, RetentionClass,
    };
    use actingcommand_policy::{
        CatalogDocumentSource, PlanningDisposition, StrategicBand, StrategicInstanceProjection,
        StrategyCatalogAdditions, TaskSpec,
    };

    #[test]
    fn strategic_replacement_removals_are_descending_and_count_toward_budget() {
        let mut sources = CatalogSources {
            tasks: CatalogDocumentSource::new(
                "memory://strategy/tasks.json",
                include_bytes!("../../../contracts/scheduling/examples/catalog-a/tasks.json")
                    .to_vec(),
            ),
            pools: CatalogDocumentSource::new(
                "memory://strategy/pools.json",
                include_bytes!("../../../contracts/scheduling/examples/catalog-a/pools.json")
                    .to_vec(),
            ),
            activity: CatalogDocumentSource::new(
                "memory://strategy/activity.json",
                include_bytes!("../../../contracts/scheduling/examples/catalog-a/activity.json")
                    .to_vec(),
            ),
            timeline: CatalogDocumentSource::new(
                "memory://strategy/timeline.json",
                include_bytes!("../../../contracts/scheduling/examples/catalog-a/timeline.json")
                    .to_vec(),
            ),
        };
        for source in [
            &mut sources.tasks,
            &mut sources.pools,
            &mut sources.activity,
            &mut sources.timeline,
        ] {
            let mut document: serde_json::Value =
                serde_json::from_slice(&source.bytes).expect("catalog fixture");
            document["catalog"]["catalog_version"] = serde_json::json!(1);
            source.bytes = serde_json::to_vec(&document).expect("catalog bytes");
        }

        let mut tasks: serde_json::Value =
            serde_json::from_slice(&sources.tasks.bytes).expect("task document");
        let task_template = tasks["tasks"][0].clone();
        let scoped_task = |id: &str, instance_id: &str| {
            let mut task = task_template.clone();
            task["id"] = serde_json::json!(id);
            task["scope"] = serde_json::json!({
                "kind": "instance",
                "instance_id": instance_id,
            });
            task
        };
        tasks["tasks"] = serde_json::json!([
            scoped_task("manual.task.a", "instance-a"),
            scoped_task("strategy.task.old-a.0", "instance-a"),
            scoped_task("strategy.task.old-b.0", "instance-b"),
            scoped_task("strategy.task.old-a.1", "instance-a"),
        ]);
        sources.tasks.bytes = serde_json::to_vec(&tasks).expect("task bytes");

        let mut activity: serde_json::Value =
            serde_json::from_slice(&sources.activity.bytes).expect("activity document");
        let profile_template = activity["profiles"][0].clone();
        let scoped_profile = |id: &str, instance_id: &str| {
            let mut profile = profile_template.clone();
            profile["id"] = serde_json::json!(id);
            profile["scope"] = serde_json::json!({
                "kind": "instance",
                "instance_id": instance_id,
            });
            profile
        };
        activity["profiles"] = serde_json::json!([
            scoped_profile("manual.profile.a", "instance-a"),
            scoped_profile("strategy.profile.old-a.0", "instance-a"),
            scoped_profile("strategy.profile.old-b.0", "instance-b"),
            scoped_profile("strategy.profile.old-a.1", "instance-a"),
        ]);
        sources.activity.bytes = serde_json::to_vec(&activity).expect("activity bytes");

        let projection = StrategicProjection {
            report_id: "strategy-report:replacement".to_owned(),
            game_id: "fixture-game".to_owned(),
            catalog_hash: format!("sha256:{}", "b".repeat(64)),
            catalog_version: 1,
            target_catalog_version: 2,
            instances: vec![StrategicInstanceProjection {
                goal_id: "goal.primary".to_owned(),
                goal_version: 1,
                instance_id: "instance-a".to_owned(),
                fact_snapshot_id: "snapshot.frozen".to_owned(),
                shortfall: Some(0),
                capacity: Some(0),
                urgency_milli: Some(0),
                band: StrategicBand::NoPressure,
                planning_disposition: PlanningDisposition::ExecutionContinues,
                template_id: None,
                decision_signature: "strategy-decision:replacement".to_owned(),
                deadline_unix_ms: 1,
            }],
            cohorts: Vec::new(),
            active_cohort_ids: Vec::new(),
            prompt_cohort_ids: Vec::new(),
            deferred_cohort_ids: Vec::new(),
            outliers: Vec::new(),
            planning_lane: Vec::new(),
            additions: StrategyCatalogAdditions {
                tasks: Vec::new(),
                activity_profiles: Vec::new(),
            },
        };
        let artifact_id = *IdentifierIssuer::new()
            .expect("issuer")
            .mint_artifact_id()
            .expect("artifact id")
            .transport();
        let artifact_id_text = serde_json::to_value(artifact_id)
            .expect("artifact id JSON")
            .as_str()
            .expect("artifact id string")
            .to_owned();
        let report = ProjectedArtifactReference {
            artifact_id,
            kind: ArtifactKind::StrategyReport,
            run_id: None,
            frame_id: None,
            correlation_id: None,
            object_key: Some(format!("artifacts/aa/{artifact_id_text}.json")),
            media_type: ArtifactMediaType::ApplicationJson,
            byte_count: 8,
            sha256: format!("sha256:{}", "a".repeat(64)),
            created_at_unix_ms: 1,
            producer: ArtifactProducer::ArtifactStore,
            retention_class: RetentionClass::Adaptive,
            redaction_state: ArtifactRedactionState::Applied,
        };

        let proposal = build_strategy_proposal(&projection, &sources, report.clone())
            .expect("removal proposal result");
        assert!(proposal.is_some(), "removals require a proposal");
        let proposal = proposal.expect("removal proposal");
        let ProposalKind::CatalogDiff { patches } = proposal.proposal() else {
            panic!("expected catalog diff")
        };
        assert_eq!(patches.len(), 8);
        assert_eq!(
            patches
                .iter()
                .take(4)
                .map(|patch| (patch.document(), patch.operation(), patch.path()))
                .collect::<Vec<_>>(),
            vec![
                (
                    ProposalDocument::Tasks,
                    ProposalPatchOperation::Remove,
                    "/tasks/3"
                ),
                (
                    ProposalDocument::Tasks,
                    ProposalPatchOperation::Remove,
                    "/tasks/1"
                ),
                (
                    ProposalDocument::Activity,
                    ProposalPatchOperation::Remove,
                    "/profiles/3"
                ),
                (
                    ProposalDocument::Activity,
                    ProposalPatchOperation::Remove,
                    "/profiles/1"
                ),
            ]
        );

        let mut budget_projection = projection.clone();
        budget_projection.additions.tasks = vec![
            serde_json::from_value::<TaskSpec>(scoped_task(
                "strategy.task.current.0",
                "instance-a",
            ))
            .expect("addition task"),
        ];
        let mut budget_tasks = tasks.clone();
        budget_tasks["tasks"] = serde_json::Value::Array(
            (0..59)
                .map(|index| scoped_task(&format!("strategy.task.budget.{index}"), "instance-a"))
                .collect(),
        );
        let mut budget_sources = sources.clone();
        budget_sources.tasks.bytes =
            serde_json::to_vec(&budget_tasks).expect("bounded budget tasks");
        budget_sources.activity.bytes = {
            let mut empty_activity = activity.clone();
            empty_activity["profiles"] = serde_json::json!([]);
            serde_json::to_vec(&empty_activity).expect("empty activity")
        };
        let bounded = build_strategy_proposal(&budget_projection, &budget_sources, report.clone())
            .expect("64-patch proposal")
            .expect("bounded proposal");
        let ProposalKind::CatalogDiff { patches } = bounded.proposal() else {
            panic!("expected bounded catalog diff")
        };
        assert_eq!(patches.len(), MAX_PROPOSAL_PATCHES);

        budget_tasks["tasks"]
            .as_array_mut()
            .expect("budget task array")
            .push(scoped_task("strategy.task.budget.59", "instance-a"));
        budget_sources.tasks.bytes =
            serde_json::to_vec(&budget_tasks).expect("overflow budget tasks");
        assert_eq!(
            build_strategy_proposal(&budget_projection, &budget_sources, report)
                .expect_err("65 patches must fail")
                .code(),
            "strategy_patch_budget_exceeded"
        );
    }
}
