// SPDX-License-Identifier: AGPL-3.0-only

//! Runtime bridge from immutable strategic reports to the existing proposal compiler.

use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::{
    CatalogDeclarationPatch, CatalogProposal, MAX_PROPOSAL_PATCHES, ProjectedArtifactReference,
    ProposalDocument, ProposalKind, ProposalPatchOperation, ProposalPreview, RuntimeErrorCode,
};
use actingcommand_policy::StrategicProjection;

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
    report: ProjectedArtifactReference,
) -> RuntimeHostResult<Option<CatalogProposal>> {
    if projection.additions.tasks.is_empty() && projection.additions.activity_profiles.is_empty() {
        return Ok(None);
    }
    let patch_count = 4_usize
        .checked_add(projection.additions.tasks.len())
        .and_then(|value| value.checked_add(projection.additions.activity_profiles.len()))
        .ok_or_else(|| request("strategy_patch_budget_exceeded"))?;
    if patch_count > MAX_PROPOSAL_PATCHES {
        return Err(request("strategy_patch_budget_exceeded"));
    }
    let mut patches = version_patches(projection.target_catalog_version)?;
    for task in &projection.additions.tasks {
        patches.push(add_patch(ProposalDocument::Tasks, "/tasks/-", task)?);
    }
    for profile in &projection.additions.activity_profiles {
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
