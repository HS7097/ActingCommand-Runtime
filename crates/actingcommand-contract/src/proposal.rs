// SPDX-License-Identifier: AGPL-3.0-only

//! Typed declaration proposal boundary for Runtime-controlled catalog promotion.

use crate::{
    ApprovalTarget, ArtifactId, ArtifactKind, ArtifactRedactionState, ProjectedArtifactReference,
    SanitizationError,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub const PROPOSAL_SCHEMA_VERSION: &str = "actingcommand.proposal.v1";
pub const MAX_PROPOSAL_PATCHES: usize = 64;
pub const MAX_PROPOSAL_REPORTS: usize = 8;
pub const MAX_PROPOSAL_PATCH_VALUE_BYTES: usize = 64 * 1024;
pub const MAX_PROPOSAL_PATCH_BYTES: usize = 512 * 1024;

const MAX_PROPOSAL_IDENTIFIER_BYTES: usize = 128;
const MAX_PROPOSAL_PATH_BYTES: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalClass {
    A,
    B,
    C,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalDisposition {
    ReadyForApproval,
    NeedsHumanSpecification,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalDocument {
    Tasks,
    Pools,
    Activity,
    Timeline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalPatchOperation {
    Add,
    Replace,
    Remove,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogDeclarationPatch {
    document: ProposalDocument,
    operation: ProposalPatchOperation,
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    value_json: Option<String>,
}

impl CatalogDeclarationPatch {
    pub fn new(
        document: ProposalDocument,
        operation: ProposalPatchOperation,
        path: impl Into<String>,
        value_json: Option<String>,
    ) -> Result<Self, SanitizationError> {
        let value = Self {
            document,
            operation,
            path: path.into(),
            value_json,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), SanitizationError> {
        validate_patch_path(&self.path)?;
        match (self.operation, self.value_json.as_deref()) {
            (ProposalPatchOperation::Remove, None) => Ok(()),
            (ProposalPatchOperation::Add | ProposalPatchOperation::Replace, Some(value)) => {
                if value.is_empty() || value.len() > MAX_PROPOSAL_PATCH_VALUE_BYTES {
                    return Err(invalid("invalid_proposal_patch_value", "proposal_patch"));
                }
                serde_json::from_str::<serde_json::Value>(value)
                    .map(|_| ())
                    .map_err(|_| invalid("invalid_proposal_patch_value", "proposal_patch"))
            }
            _ => Err(invalid(
                "invalid_proposal_patch_operation",
                "proposal_patch",
            )),
        }
    }

    pub const fn document(&self) -> ProposalDocument {
        self.document
    }

    pub const fn operation(&self) -> ProposalPatchOperation {
        self.operation
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn value_json(&self) -> Option<&str> {
        self.value_json.as_deref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskTemplateInstantiation {
    template_task_id: String,
    new_task_id: String,
    instance_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    priority: Option<i16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    strategic_weight_milli: Option<u16>,
}

impl TaskTemplateInstantiation {
    pub fn new(
        template_task_id: impl Into<String>,
        new_task_id: impl Into<String>,
        instance_id: impl Into<String>,
        priority: Option<i16>,
        strategic_weight_milli: Option<u16>,
    ) -> Result<Self, SanitizationError> {
        let value = Self {
            template_task_id: template_task_id.into(),
            new_task_id: new_task_id.into(),
            instance_id: instance_id.into(),
            priority,
            strategic_weight_milli,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), SanitizationError> {
        validate_identifier(&self.template_task_id, "template_task_id")?;
        validate_identifier(&self.new_task_id, "new_task_id")?;
        validate_identifier(&self.instance_id, "instance_id")?;
        if self.template_task_id == self.new_task_id
            || self
                .strategic_weight_milli
                .is_some_and(|value| value > 10_000)
        {
            return Err(invalid(
                "invalid_template_instantiation",
                "template_instantiation",
            ));
        }
        Ok(())
    }

    pub fn template_task_id(&self) -> &str {
        &self.template_task_id
    }

    pub fn new_task_id(&self) -> &str {
        &self.new_task_id
    }

    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    pub const fn priority(&self) -> Option<i16> {
        self.priority
    }

    pub const fn strategic_weight_milli(&self) -> Option<u16> {
        self.strategic_weight_milli
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProposalKind {
    ParameterInstantiation {
        instantiation: TaskTemplateInstantiation,
    },
    CatalogDiff {
        patches: Vec<CatalogDeclarationPatch>,
    },
    LanguageExtension {
        extension_code: String,
    },
}

impl ProposalKind {
    pub const fn class(&self) -> ProposalClass {
        match self {
            Self::ParameterInstantiation { .. } => ProposalClass::A,
            Self::CatalogDiff { .. } => ProposalClass::B,
            Self::LanguageExtension { .. } => ProposalClass::C,
        }
    }

    fn validate(&self) -> Result<(), SanitizationError> {
        match self {
            Self::ParameterInstantiation { instantiation } => instantiation.validate(),
            Self::CatalogDiff { patches } => {
                if patches.is_empty() || patches.len() > MAX_PROPOSAL_PATCHES {
                    return Err(invalid("invalid_proposal_patch_count", "proposal_patches"));
                }
                let mut total = 0_usize;
                for patch in patches {
                    patch.validate()?;
                    total = total
                        .checked_add(patch.path.len())
                        .and_then(|value| {
                            value.checked_add(patch.value_json.as_ref().map_or(0, String::len))
                        })
                        .ok_or_else(|| {
                            invalid("proposal_patch_budget_exceeded", "proposal_patches")
                        })?;
                }
                if total > MAX_PROPOSAL_PATCH_BYTES {
                    return Err(invalid(
                        "proposal_patch_budget_exceeded",
                        "proposal_patches",
                    ));
                }
                Ok(())
            }
            Self::LanguageExtension { extension_code } => {
                validate_identifier(extension_code, "extension_code")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogProposal {
    schema_version: String,
    proposal_id: String,
    base_catalog_hash: String,
    base_catalog_version: u64,
    target_catalog_version: u64,
    report_refs: Vec<ProjectedArtifactReference>,
    proposal: ProposalKind,
}

impl CatalogProposal {
    pub fn new(
        base_catalog_hash: impl Into<String>,
        base_catalog_version: u64,
        target_catalog_version: u64,
        report_refs: Vec<ProjectedArtifactReference>,
        proposal: ProposalKind,
    ) -> Result<Self, SanitizationError> {
        let mut value = Self {
            schema_version: PROPOSAL_SCHEMA_VERSION.to_owned(),
            proposal_id: String::new(),
            base_catalog_hash: base_catalog_hash.into(),
            base_catalog_version,
            target_catalog_version,
            report_refs,
            proposal,
        };
        value.validate_components()?;
        value.proposal_id = proposal_id_for(&value)?;
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), SanitizationError> {
        if self.schema_version != PROPOSAL_SCHEMA_VERSION {
            return Err(invalid(
                "unsupported_proposal_schema",
                "proposal_schema_version",
            ));
        }
        self.validate_components()?;
        if self.proposal_id != proposal_id_for(self)? {
            return Err(invalid("proposal_identity_mismatch", "proposal_id"));
        }
        Ok(())
    }

    fn validate_components(&self) -> Result<(), SanitizationError> {
        validate_sha256(&self.base_catalog_hash, "base_catalog_hash")?;
        if self.base_catalog_version == 0
            || self.target_catalog_version <= self.base_catalog_version
            || self.report_refs.is_empty()
            || self.report_refs.len() > MAX_PROPOSAL_REPORTS
        {
            return Err(invalid("invalid_proposal_boundary", "catalog_proposal"));
        }
        let mut reports = BTreeSet::new();
        for reference in &self.report_refs {
            reference.validate()?;
            if !matches!(
                reference.kind(),
                ArtifactKind::TextReport | ArtifactKind::StrategyReport
            ) || reference.object_key().is_none()
                || reference.redaction_state() == ArtifactRedactionState::Pending
                || !reports.insert(reference.artifact_id)
            {
                return Err(invalid("invalid_proposal_report", "proposal_report"));
            }
        }
        self.proposal.validate()
    }

    pub fn schema_version(&self) -> &str {
        &self.schema_version
    }

    pub fn proposal_id(&self) -> &str {
        &self.proposal_id
    }

    pub fn base_catalog_hash(&self) -> &str {
        &self.base_catalog_hash
    }

    pub const fn base_catalog_version(&self) -> u64 {
        self.base_catalog_version
    }

    pub const fn target_catalog_version(&self) -> u64 {
        self.target_catalog_version
    }

    pub fn report_refs(&self) -> &[ProjectedArtifactReference] {
        &self.report_refs
    }

    pub const fn proposal(&self) -> &ProposalKind {
        &self.proposal
    }

    pub const fn class(&self) -> ProposalClass {
        self.proposal.class()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposalPreview {
    proposal_id: String,
    class: ProposalClass,
    disposition: ProposalDisposition,
    base_catalog_hash: String,
    base_catalog_version: u64,
    target_catalog_version: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_catalog_hash: Option<String>,
    report_count: u16,
}

impl ProposalPreview {
    pub fn ready(
        proposal: &CatalogProposal,
        target_catalog_hash: impl Into<String>,
    ) -> Result<Self, SanitizationError> {
        Self::new(
            proposal,
            ProposalDisposition::ReadyForApproval,
            Some(target_catalog_hash.into()),
        )
    }

    pub fn needs_human_specification(
        proposal: &CatalogProposal,
    ) -> Result<Self, SanitizationError> {
        Self::new(proposal, ProposalDisposition::NeedsHumanSpecification, None)
    }

    fn new(
        proposal: &CatalogProposal,
        disposition: ProposalDisposition,
        target_catalog_hash: Option<String>,
    ) -> Result<Self, SanitizationError> {
        let value = Self {
            proposal_id: proposal.proposal_id.clone(),
            class: proposal.class(),
            disposition,
            base_catalog_hash: proposal.base_catalog_hash.clone(),
            base_catalog_version: proposal.base_catalog_version,
            target_catalog_version: proposal.target_catalog_version,
            target_catalog_hash,
            report_count: proposal.report_refs.len() as u16,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), SanitizationError> {
        validate_proposal_id(&self.proposal_id)?;
        validate_sha256(&self.base_catalog_hash, "base_catalog_hash")?;
        if self.base_catalog_version == 0
            || self.target_catalog_version <= self.base_catalog_version
            || self.report_count == 0
            || usize::from(self.report_count) > MAX_PROPOSAL_REPORTS
        {
            return Err(invalid("invalid_proposal_preview", "proposal_preview"));
        }
        match (
            self.class,
            self.disposition,
            self.target_catalog_hash.as_deref(),
        ) {
            (
                ProposalClass::A | ProposalClass::B,
                ProposalDisposition::ReadyForApproval,
                Some(hash),
            ) => validate_sha256(hash, "target_catalog_hash"),
            (ProposalClass::C, ProposalDisposition::NeedsHumanSpecification, None) => Ok(()),
            _ => Err(invalid("invalid_proposal_preview", "proposal_preview")),
        }
    }

    pub fn proposal_id(&self) -> &str {
        &self.proposal_id
    }

    pub const fn class(&self) -> ProposalClass {
        self.class
    }

    pub const fn disposition(&self) -> ProposalDisposition {
        self.disposition
    }

    pub fn base_catalog_hash(&self) -> &str {
        &self.base_catalog_hash
    }

    pub const fn base_catalog_version(&self) -> u64 {
        self.base_catalog_version
    }

    pub const fn target_catalog_version(&self) -> u64 {
        self.target_catalog_version
    }

    pub fn target_catalog_hash(&self) -> Option<&str> {
        self.target_catalog_hash.as_deref()
    }

    pub const fn report_count(&self) -> u16 {
        self.report_count
    }

    pub fn approval_target(&self) -> Option<ApprovalTarget> {
        self.target_catalog_hash
            .as_ref()
            .map(|catalog_hash| ApprovalTarget::Plan {
                plan_id: self.proposal_id.clone(),
                catalog_hash: catalog_hash.clone(),
                catalog_version: self.target_catalog_version,
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposalPromotion {
    preview: ProposalPreview,
    approval_fact_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogPromotionAuthorization {
    proposal_id: String,
    class: ProposalClass,
    approval_fact_ids: Vec<String>,
    report_artifact_ids: Vec<ArtifactId>,
}

impl CatalogPromotionAuthorization {
    pub fn new(
        proposal: &CatalogProposal,
        promotion: &ProposalPromotion,
    ) -> Result<Self, SanitizationError> {
        if promotion.preview.proposal_id != proposal.proposal_id
            || promotion.preview.class != proposal.class()
        {
            return Err(invalid(
                "proposal_authorization_identity_mismatch",
                "proposal_authorization",
            ));
        }
        let mut report_artifact_ids = proposal
            .report_refs
            .iter()
            .map(|reference| reference.artifact_id)
            .collect::<Vec<_>>();
        report_artifact_ids.sort();
        let value = Self {
            proposal_id: proposal.proposal_id.clone(),
            class: proposal.class(),
            approval_fact_ids: promotion.approval_fact_ids.clone(),
            report_artifact_ids,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), SanitizationError> {
        validate_proposal_id(&self.proposal_id)?;
        if self.class == ProposalClass::C
            || self.approval_fact_ids.is_empty()
            || self.approval_fact_ids.len() > 64
            || self.report_artifact_ids.is_empty()
            || self.report_artifact_ids.len() > MAX_PROPOSAL_REPORTS
        {
            return Err(invalid(
                "invalid_proposal_authorization",
                "proposal_authorization",
            ));
        }
        let mut previous_approval = None::<&str>;
        for approval in &self.approval_fact_ids {
            validate_identifier(approval, "approval_fact_id")?;
            if !approval.starts_with("approval:")
                || previous_approval.is_some_and(|value| value >= approval)
            {
                return Err(invalid(
                    "invalid_proposal_approval_facts",
                    "approval_fact_ids",
                ));
            }
            previous_approval = Some(approval);
        }
        let mut previous_report = None::<ArtifactId>;
        for report in &self.report_artifact_ids {
            if previous_report.is_some_and(|value| value >= *report) {
                return Err(invalid(
                    "invalid_proposal_report_facts",
                    "report_artifact_ids",
                ));
            }
            previous_report = Some(*report);
        }
        Ok(())
    }

    pub fn proposal_id(&self) -> &str {
        &self.proposal_id
    }

    pub const fn class(&self) -> ProposalClass {
        self.class
    }

    pub fn approval_fact_ids(&self) -> &[String] {
        &self.approval_fact_ids
    }

    pub fn report_artifact_ids(&self) -> &[ArtifactId] {
        &self.report_artifact_ids
    }
}

impl ProposalPromotion {
    pub fn new(
        preview: ProposalPreview,
        mut approval_fact_ids: Vec<String>,
    ) -> Result<Self, SanitizationError> {
        approval_fact_ids.sort();
        let value = Self {
            preview,
            approval_fact_ids,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), SanitizationError> {
        self.preview.validate()?;
        if self.preview.disposition != ProposalDisposition::ReadyForApproval
            || self.approval_fact_ids.is_empty()
            || self.approval_fact_ids.len() > 64
        {
            return Err(invalid("invalid_proposal_promotion", "proposal_promotion"));
        }
        let mut previous = None::<&str>;
        for approval in &self.approval_fact_ids {
            validate_identifier(approval, "approval_fact_id")?;
            if !approval.starts_with("approval:") || previous.is_some_and(|value| value >= approval)
            {
                return Err(invalid(
                    "invalid_proposal_approval_facts",
                    "approval_fact_ids",
                ));
            }
            previous = Some(approval);
        }
        Ok(())
    }

    pub const fn preview(&self) -> &ProposalPreview {
        &self.preview
    }

    pub fn approval_fact_ids(&self) -> &[String] {
        &self.approval_fact_ids
    }
}

fn proposal_id_for(proposal: &CatalogProposal) -> Result<String, SanitizationError> {
    #[derive(Serialize)]
    struct Identity<'a> {
        schema_version: &'a str,
        base_catalog_hash: &'a str,
        base_catalog_version: u64,
        target_catalog_version: u64,
        report_refs: &'a [ProjectedArtifactReference],
        proposal: &'a ProposalKind,
    }

    let bytes = serde_json::to_vec(&Identity {
        schema_version: &proposal.schema_version,
        base_catalog_hash: &proposal.base_catalog_hash,
        base_catalog_version: proposal.base_catalog_version,
        target_catalog_version: proposal.target_catalog_version,
        report_refs: &proposal.report_refs,
        proposal: &proposal.proposal,
    })
    .map_err(|_| invalid("proposal_identity_encode_failed", "proposal_id"))?;
    Ok(format!("proposal:{:x}", Sha256::digest(bytes)))
}

fn validate_proposal_id(value: &str) -> Result<(), SanitizationError> {
    validate_hash_identity(value, "proposal:", "proposal_id")
}

fn validate_sha256(value: &str, field: &'static str) -> Result<(), SanitizationError> {
    validate_hash_identity(value, "sha256:", field)
}

fn validate_hash_identity(
    value: &str,
    prefix: &'static str,
    field: &'static str,
) -> Result<(), SanitizationError> {
    if value.strip_prefix(prefix).is_none_or(|digest| {
        digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    }) {
        return Err(invalid("invalid_proposal_hash", field));
    }
    Ok(())
}

fn validate_identifier(value: &str, field: &'static str) -> Result<(), SanitizationError> {
    if value.is_empty()
        || value.len() > MAX_PROPOSAL_IDENTIFIER_BYTES
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'.' | b'_' | b':' | b'-')
        })
    {
        return Err(invalid("invalid_proposal_identifier", field));
    }
    Ok(())
}

fn validate_patch_path(path: &str) -> Result<(), SanitizationError> {
    if path.len() < 2
        || path.len() > MAX_PROPOSAL_PATH_BYTES
        || !path.starts_with('/')
        || path.chars().any(char::is_control)
        || path.split('/').skip(1).any(str::is_empty)
        || path == "/schema_version"
        || path.starts_with("/schema_version/")
        || path == "/catalog"
        || path == "/catalog/catalog_id"
        || path.starts_with("/catalog/catalog_id/")
    {
        return Err(invalid("invalid_proposal_patch_path", "proposal_patch"));
    }
    let bytes = path.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'~' {
            if index + 1 >= bytes.len() || !matches!(bytes[index + 1], b'0' | b'1') {
                return Err(invalid("invalid_proposal_patch_path", "proposal_patch"));
            }
            index += 1;
        }
        index += 1;
    }
    Ok(())
}

fn invalid(code: &'static str, field: &'static str) -> SanitizationError {
    SanitizationError::new(code, field)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ArtifactMediaType, ArtifactProducer, IdentifierIssuer, RetentionClass};

    fn valid_report() -> ProjectedArtifactReference {
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
        ProjectedArtifactReference {
            artifact_id,
            kind: ArtifactKind::TextReport,
            run_id: None,
            frame_id: None,
            correlation_id: None,
            object_key: Some(format!("artifacts/aa/{artifact_id_text}.txt")),
            media_type: ArtifactMediaType::TextPlain,
            byte_count: 8,
            sha256: format!("sha256:{}", "a".repeat(64)),
            created_at_unix_ms: 1,
            producer: ArtifactProducer::ArtifactStore,
            retention_class: RetentionClass::Adaptive,
            redaction_state: ArtifactRedactionState::NotRequired,
        }
    }

    #[test]
    fn parameter_proposal_has_stable_identity_and_plan_target() {
        let proposal = CatalogProposal::new(
            format!("sha256:{}", "b".repeat(64)),
            1,
            2,
            vec![valid_report()],
            ProposalKind::ParameterInstantiation {
                instantiation: TaskTemplateInstantiation::new(
                    "template.observe",
                    "instance.observe",
                    "instance-a",
                    Some(10),
                    Some(1_200),
                )
                .expect("instantiation"),
            },
        )
        .expect("proposal");
        let decoded: CatalogProposal =
            serde_json::from_slice(&serde_json::to_vec(&proposal).expect("proposal JSON"))
                .expect("typed proposal");
        decoded.validate().expect("proposal validation");
        assert_eq!(proposal.proposal_id(), decoded.proposal_id());
        let preview = ProposalPreview::ready(&proposal, format!("sha256:{}", "c".repeat(64)))
            .expect("preview");
        assert!(matches!(
            preview.approval_target(),
            Some(ApprovalTarget::Plan { plan_id, .. }) if plan_id == proposal.proposal_id()
        ));
    }

    #[test]
    fn proposal_requires_a_verified_report_shape() {
        let error = CatalogProposal::new(
            format!("sha256:{}", "b".repeat(64)),
            1,
            2,
            Vec::new(),
            ProposalKind::LanguageExtension {
                extension_code: "predicate.new".to_owned(),
            },
        )
        .expect_err("report is mandatory");
        assert_eq!(error.code(), "invalid_proposal_boundary");
    }

    #[test]
    fn language_extension_never_produces_an_approval_target() {
        let proposal = CatalogProposal::new(
            format!("sha256:{}", "b".repeat(64)),
            1,
            2,
            vec![valid_report()],
            ProposalKind::LanguageExtension {
                extension_code: "predicate.new".to_owned(),
            },
        )
        .expect("proposal");
        let preview = ProposalPreview::needs_human_specification(&proposal).expect("preview");
        assert_eq!(preview.class(), ProposalClass::C);
        assert!(preview.approval_target().is_none());
    }

    #[test]
    fn patch_contract_rejects_root_metadata_and_missing_values() {
        assert!(
            CatalogDeclarationPatch::new(
                ProposalDocument::Tasks,
                ProposalPatchOperation::Replace,
                "/schema_version",
                Some("\"v2\"".to_owned()),
            )
            .is_err()
        );
        assert!(
            CatalogDeclarationPatch::new(
                ProposalDocument::Tasks,
                ProposalPatchOperation::Replace,
                "/catalog",
                Some("{}".to_owned()),
            )
            .is_err()
        );
        assert!(
            CatalogDeclarationPatch::new(
                ProposalDocument::Tasks,
                ProposalPatchOperation::Add,
                "/tasks/-",
                None,
            )
            .is_err()
        );
    }
}
