// SPDX-License-Identifier: AGPL-3.0-only

//! Versioned, read-only project projection consumed by UI, CLI, and external clients.
//!
//! These structs are transport DTOs. Runtime domain state remains internal to the host and is
//! translated into this neutral projection at the IPC boundary.

use crate::{
    ApprovalDisposition, ApprovalTargetKind, EventSeverity, EventType, InstanceId, OwnerEpoch,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

pub const PROJECT_INTERFACE_REQUEST_SCHEMA_VERSION_V1: &str =
    "actingcommand.project-interface.request.v1";
pub const PROJECT_INTERFACE_REQUEST_SCHEMA_VERSION: &str =
    "actingcommand.project-interface.request.v2";
pub const PROJECT_INTERFACE_RESPONSE_SCHEMA_VERSION_V1: &str =
    "actingcommand.project-interface.response.v1";
pub const PROJECT_INTERFACE_RESPONSE_SCHEMA_VERSION: &str =
    "actingcommand.project-interface.response.v2";
pub const PROJECT_INTERFACE_CONTRACT_V1: &str = "actingcommand.project-interface.v1";
pub const PROJECT_INTERFACE_CONTRACT_V2: &str = "actingcommand.project-interface.v2";
pub const PROJECT_INTERFACE_SUPPORTED_VERSIONS: &[&str] =
    &[PROJECT_INTERFACE_CONTRACT_V2, PROJECT_INTERFACE_CONTRACT_V1];
pub const MAX_PROJECT_INTERFACE_RESPONSE_BYTES: usize = 768 * 1024;
pub const DEFAULT_PROJECT_DECISION_PAGE_SIZE: u16 = 128;
pub const MAX_PROJECT_DECISION_PAGE_SIZE: u16 = 512;

const MAX_ACCEPTED_VERSIONS: usize = 8;
const MAX_PROJECT_INSTANCES: usize = 4_096;
const MAX_PROJECT_FACTS: usize = 16_384;
const MAX_PROJECT_GOALS: usize = 16_384;
const MAX_PROJECT_DECISIONS: usize = MAX_PROJECT_DECISION_PAGE_SIZE as usize;
const MAX_PROJECT_APPROVALS: usize = 4_096;
const MAX_PROJECT_DIAGNOSTICS: usize = 256;
const MAX_PROJECT_TEXT_BYTES: usize = 1_024;

pub type ProjectInterfaceResult<T> = Result<T, ProjectInterfaceError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectInterfaceError {
    code: &'static str,
}

impl ProjectInterfaceError {
    const fn new(code: &'static str) -> Self {
        Self { code }
    }

    pub const fn code(self) -> &'static str {
        self.code
    }
}

impl fmt::Display for ProjectInterfaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "project interface contract rejected with {}",
            self.code
        )
    }
}

impl Error for ProjectInterfaceError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectUnknownFieldPolicy {
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectInterfaceCompatibility {
    pub current_version: String,
    pub supported_versions: Vec<String>,
    pub unknown_field_policy: ProjectUnknownFieldPolicy,
}

impl ProjectInterfaceCompatibility {
    pub fn current() -> Self {
        Self {
            current_version: PROJECT_INTERFACE_CONTRACT_V2.to_owned(),
            supported_versions: PROJECT_INTERFACE_SUPPORTED_VERSIONS
                .iter()
                .map(|version| (*version).to_owned())
                .collect(),
            unknown_field_policy: ProjectUnknownFieldPolicy::Reject,
        }
    }

    fn for_contract(contract_version: &str) -> ProjectInterfaceResult<Self> {
        match contract_version {
            PROJECT_INTERFACE_CONTRACT_V2 => Ok(Self::current()),
            PROJECT_INTERFACE_CONTRACT_V1 => Ok(Self {
                current_version: PROJECT_INTERFACE_CONTRACT_V1.to_owned(),
                supported_versions: vec![PROJECT_INTERFACE_CONTRACT_V1.to_owned()],
                unknown_field_policy: ProjectUnknownFieldPolicy::Reject,
            }),
            _ => Err(ProjectInterfaceError::new(
                "project_contract_version_unsupported",
            )),
        }
    }

    pub fn validate(&self) -> ProjectInterfaceResult<()> {
        let current = Self::for_contract(&self.current_version)?;
        if self != &current {
            return Err(ProjectInterfaceError::new(
                "project_compatibility_matrix_invalid",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectDecisionPageCursor {
    snapshot_ledger_position: u64,
    before_intent_sequence: u64,
    before_decision_id: String,
}

impl ProjectDecisionPageCursor {
    pub fn new(
        snapshot_ledger_position: u64,
        before_intent_sequence: u64,
        before_decision_id: impl Into<String>,
    ) -> ProjectInterfaceResult<Self> {
        let cursor = Self {
            snapshot_ledger_position,
            before_intent_sequence,
            before_decision_id: before_decision_id.into(),
        };
        cursor.validate()?;
        Ok(cursor)
    }

    pub fn validate(&self) -> ProjectInterfaceResult<()> {
        if self.snapshot_ledger_position == 0
            || self.before_intent_sequence == 0
            || self.before_intent_sequence > self.snapshot_ledger_position
        {
            return Err(ProjectInterfaceError::new(
                "project_decision_cursor_invalid",
            ));
        }
        validate_text(&self.before_decision_id, "project_decision_cursor_invalid")
    }

    pub const fn snapshot_ledger_position(&self) -> u64 {
        self.snapshot_ledger_position
    }

    pub const fn before_intent_sequence(&self) -> u64 {
        self.before_intent_sequence
    }

    pub fn before_decision_id(&self) -> &str {
        &self.before_decision_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectDecisionPageRequest {
    limit: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cursor: Option<ProjectDecisionPageCursor>,
}

impl ProjectDecisionPageRequest {
    pub fn new(
        limit: u16,
        cursor: Option<ProjectDecisionPageCursor>,
    ) -> ProjectInterfaceResult<Self> {
        let request = Self { limit, cursor };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> ProjectInterfaceResult<()> {
        if self.limit == 0 || self.limit > MAX_PROJECT_DECISION_PAGE_SIZE {
            return Err(ProjectInterfaceError::new("project_decision_page_invalid"));
        }
        if let Some(cursor) = &self.cursor {
            cursor.validate()?;
        }
        Ok(())
    }

    pub const fn limit(&self) -> u16 {
        self.limit
    }

    pub const fn cursor(&self) -> Option<&ProjectDecisionPageCursor> {
        self.cursor.as_ref()
    }
}

impl Default for ProjectDecisionPageRequest {
    fn default() -> Self {
        Self {
            limit: DEFAULT_PROJECT_DECISION_PAGE_SIZE,
            cursor: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectInterfaceRequest {
    schema_version: String,
    accepted_contract_versions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    decision_page: Option<ProjectDecisionPageRequest>,
}

impl ProjectInterfaceRequest {
    pub fn current() -> Self {
        Self {
            schema_version: PROJECT_INTERFACE_REQUEST_SCHEMA_VERSION.to_owned(),
            accepted_contract_versions: PROJECT_INTERFACE_SUPPORTED_VERSIONS
                .iter()
                .map(|version| (*version).to_owned())
                .collect(),
            decision_page: Some(ProjectDecisionPageRequest::default()),
        }
    }

    pub fn new(accepted_contract_versions: Vec<String>) -> ProjectInterfaceResult<Self> {
        let schema_version = if accepted_contract_versions
            .iter()
            .any(|version| version == PROJECT_INTERFACE_CONTRACT_V2)
        {
            PROJECT_INTERFACE_REQUEST_SCHEMA_VERSION
        } else {
            PROJECT_INTERFACE_REQUEST_SCHEMA_VERSION_V1
        };
        let request = Self {
            schema_version: schema_version.to_owned(),
            accepted_contract_versions,
            decision_page: None,
        };
        request.validate()?;
        Ok(request)
    }

    pub fn with_decision_page(
        mut self,
        decision_page: ProjectDecisionPageRequest,
    ) -> ProjectInterfaceResult<Self> {
        self.schema_version = PROJECT_INTERFACE_REQUEST_SCHEMA_VERSION.to_owned();
        self.decision_page = Some(decision_page);
        self.validate()?;
        Ok(self)
    }

    pub fn validate(&self) -> ProjectInterfaceResult<()> {
        if !matches!(
            self.schema_version.as_str(),
            PROJECT_INTERFACE_REQUEST_SCHEMA_VERSION | PROJECT_INTERFACE_REQUEST_SCHEMA_VERSION_V1
        ) {
            return Err(ProjectInterfaceError::new(
                "unsupported_project_request_schema",
            ));
        }
        if self.accepted_contract_versions.is_empty()
            || self.accepted_contract_versions.len() > MAX_ACCEPTED_VERSIONS
        {
            return Err(ProjectInterfaceError::new(
                "project_contract_versions_invalid",
            ));
        }
        let mut seen = BTreeSet::new();
        for version in &self.accepted_contract_versions {
            validate_text(version, "project_contract_versions_invalid")?;
            if !seen.insert(version) {
                return Err(ProjectInterfaceError::new(
                    "project_contract_versions_invalid",
                ));
            }
        }
        if self.schema_version == PROJECT_INTERFACE_REQUEST_SCHEMA_VERSION_V1
            && self.decision_page.is_some()
        {
            return Err(ProjectInterfaceError::new(
                "project_decision_page_requires_v2",
            ));
        }
        if let Some(page) = &self.decision_page {
            if !self
                .accepted_contract_versions
                .iter()
                .any(|version| version == PROJECT_INTERFACE_CONTRACT_V2)
            {
                return Err(ProjectInterfaceError::new(
                    "project_decision_page_requires_v2",
                ));
            }
            page.validate()?;
        }
        Ok(())
    }

    pub fn negotiate(&self) -> ProjectInterfaceResult<&'static str> {
        self.validate()?;
        PROJECT_INTERFACE_SUPPORTED_VERSIONS
            .iter()
            .copied()
            .find(|supported| {
                self.accepted_contract_versions
                    .iter()
                    .any(|accepted| accepted == supported)
            })
            .ok_or_else(|| ProjectInterfaceError::new("project_contract_version_unsupported"))
    }

    pub fn accepted_contract_versions(&self) -> &[String] {
        &self.accepted_contract_versions
    }

    pub const fn decision_page(&self) -> Option<&ProjectDecisionPageRequest> {
        self.decision_page.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectView {
    pub project_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectInstanceView {
    pub instance_alias: String,
    pub instance_id: InstanceId,
    pub lease_active: bool,
    pub queued_request_count: u32,
    pub takeover_cooldown_active: bool,
    pub destructive_step_active: bool,
    pub preempt_requested: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectCatalogView {
    pub catalog_id: String,
    pub catalog_version: u64,
    pub catalog_hash: String,
    pub task_count: u32,
    pub pool_count: u32,
    pub activity_profile_count: u32,
    pub goal_count: u32,
    pub timeline_event_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectScopeView {
    Instance { instance_id: String },
    Server { server_id: String },
    Game { game_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectFactContentState {
    InlineRedacted,
    ArtifactRedacted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectFactView {
    pub scope: ProjectScopeView,
    pub key: String,
    pub content_state: ProjectFactContentState,
    pub observed_at_unix_ms: u64,
    pub expires_at_unix_ms: Option<u64>,
    pub confidence_milli: u16,
    pub source_snapshot_id: String,
    pub fact_schema_version: String,
    pub resource_bundle_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectGoalMetricView {
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
pub struct ProjectGoalView {
    pub goal_id: String,
    pub activity_profile_id: String,
    pub scope: ProjectScopeView,
    pub metric: ProjectGoalMetricView,
    pub target: i64,
    pub deadline_unix_ms: u64,
    pub strategic_weight_milli: u16,
    pub best_effort: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectDecisionState {
    Intent,
    Admitted,
    Rejected,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectDecisionView {
    pub decision_id: String,
    pub task_id: String,
    pub instance_id: String,
    pub operation_id: String,
    pub state: ProjectDecisionState,
    pub catalog_hash: String,
    pub catalog_version: u64,
    pub input_ledger_position: u64,
    pub fact_snapshot_id: String,
    pub approval_fact_ids: Vec<String>,
    pub reason_codes: Vec<String>,
    pub urgency_milli: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectApprovalView {
    pub approval_id: String,
    pub disposition: ApprovalDisposition,
    pub target_kind: ApprovalTargetKind,
    pub target_id: Option<String>,
    pub catalog_hash: String,
    pub catalog_version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectRuntimeView {
    pub owner_epoch: OwnerEpoch,
    pub ledger_position: u64,
    pub fatal: bool,
    pub instance_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectDiagnosticView {
    pub sequence: u64,
    pub timestamp_unix_ms: u64,
    pub severity: EventSeverity,
    pub event_type: EventType,
    pub code: Option<String>,
    pub fatal: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectDecisionPage {
    snapshot_ledger_position: u64,
    requested_limit: u16,
    returned_count: u16,
    has_more: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    next_cursor: Option<ProjectDecisionPageCursor>,
}

impl ProjectDecisionPage {
    pub fn new(
        snapshot_ledger_position: u64,
        requested_limit: u16,
        returned_count: u16,
        has_more: bool,
        next_cursor: Option<ProjectDecisionPageCursor>,
    ) -> ProjectInterfaceResult<Self> {
        let page = Self {
            snapshot_ledger_position,
            requested_limit,
            returned_count,
            has_more,
            next_cursor,
        };
        page.validate()?;
        Ok(page)
    }

    pub fn validate(&self) -> ProjectInterfaceResult<()> {
        if self.snapshot_ledger_position == 0
            || self.requested_limit == 0
            || self.requested_limit > MAX_PROJECT_DECISION_PAGE_SIZE
            || self.returned_count > self.requested_limit
            || self.has_more != self.next_cursor.is_some()
        {
            return Err(ProjectInterfaceError::new("project_decision_page_invalid"));
        }
        if let Some(cursor) = &self.next_cursor {
            cursor.validate()?;
            if cursor.snapshot_ledger_position() != self.snapshot_ledger_position {
                return Err(ProjectInterfaceError::new("project_decision_page_invalid"));
            }
        }
        Ok(())
    }

    pub const fn snapshot_ledger_position(&self) -> u64 {
        self.snapshot_ledger_position
    }

    pub const fn requested_limit(&self) -> u16 {
        self.requested_limit
    }

    pub const fn returned_count(&self) -> u16 {
        self.returned_count
    }

    pub const fn has_more(&self) -> bool {
        self.has_more
    }

    pub const fn next_cursor(&self) -> Option<&ProjectDecisionPageCursor> {
        self.next_cursor.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectInterfaceSnapshot {
    pub ledger_position: u64,
    pub project: Option<ProjectView>,
    pub instances: Vec<ProjectInstanceView>,
    pub catalog: Option<ProjectCatalogView>,
    pub facts: Vec<ProjectFactView>,
    pub goals: Vec<ProjectGoalView>,
    pub decisions: Vec<ProjectDecisionView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_page: Option<ProjectDecisionPage>,
    pub approvals: Vec<ProjectApprovalView>,
    pub runtime: ProjectRuntimeView,
    pub diagnostics: Vec<ProjectDiagnosticView>,
}

impl ProjectInterfaceSnapshot {
    pub fn validate(&self) -> ProjectInterfaceResult<()> {
        if self.ledger_position == 0
            || self.runtime.ledger_position != self.ledger_position
            || self.runtime.instance_count != self.instances.len() as u32
            || self.instances.len() > MAX_PROJECT_INSTANCES
            || self.facts.len() > MAX_PROJECT_FACTS
            || self.goals.len() > MAX_PROJECT_GOALS
            || self.decisions.len() > MAX_PROJECT_DECISIONS
            || self.approvals.len() > MAX_PROJECT_APPROVALS
            || self.diagnostics.len() > MAX_PROJECT_DIAGNOSTICS
        {
            return Err(ProjectInterfaceError::new("project_snapshot_invalid"));
        }
        match (&self.project, &self.catalog) {
            (Some(project), Some(catalog)) if project.project_id == catalog.catalog_id => {}
            (None, None) => {}
            _ => {
                return Err(ProjectInterfaceError::new(
                    "project_catalog_identity_mismatch",
                ));
            }
        }
        if let Some(project) = &self.project {
            validate_text(&project.project_id, "project_identity_invalid")?;
        }
        if let Some(catalog) = &self.catalog {
            validate_text(&catalog.catalog_id, "project_catalog_invalid")?;
            validate_hash(&catalog.catalog_hash, "project_catalog_invalid")?;
            if catalog.catalog_version == 0 {
                return Err(ProjectInterfaceError::new("project_catalog_invalid"));
            }
        }
        validate_instances(&self.instances)?;
        validate_facts(&self.facts)?;
        validate_goals(&self.goals)?;
        validate_decisions(&self.decisions, self.ledger_position)?;
        if let Some(page) = &self.decision_page {
            page.validate()?;
            if page.snapshot_ledger_position() != self.ledger_position
                || usize::from(page.returned_count()) != self.decisions.len()
            {
                return Err(ProjectInterfaceError::new("project_decision_page_invalid"));
            }
        }
        validate_approvals(&self.approvals)?;
        validate_diagnostics(&self.diagnostics, self.ledger_position)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectInterfaceResponse {
    schema_version: String,
    contract_version: String,
    compatibility: ProjectInterfaceCompatibility,
    snapshot: ProjectInterfaceSnapshot,
}

impl ProjectInterfaceResponse {
    pub fn new(
        negotiated_version: &str,
        snapshot: ProjectInterfaceSnapshot,
    ) -> ProjectInterfaceResult<Self> {
        let compatibility = ProjectInterfaceCompatibility::for_contract(negotiated_version)?;
        let schema_version = match negotiated_version {
            PROJECT_INTERFACE_CONTRACT_V2 => PROJECT_INTERFACE_RESPONSE_SCHEMA_VERSION,
            PROJECT_INTERFACE_CONTRACT_V1 => PROJECT_INTERFACE_RESPONSE_SCHEMA_VERSION_V1,
            _ => unreachable!("validated contract version"),
        };
        let response = Self {
            schema_version: schema_version.to_owned(),
            contract_version: negotiated_version.to_owned(),
            compatibility,
            snapshot,
        };
        response.validate()?;
        let encoded = serde_json::to_vec(&response)
            .map_err(|_| ProjectInterfaceError::new("project_response_encode_failed"))?;
        if encoded.len() > MAX_PROJECT_INTERFACE_RESPONSE_BYTES {
            return Err(ProjectInterfaceError::new(
                "project_interface_response_too_large",
            ));
        }
        Ok(response)
    }

    pub fn validate(&self) -> ProjectInterfaceResult<()> {
        let expected_schema = match self.contract_version.as_str() {
            PROJECT_INTERFACE_CONTRACT_V2 => PROJECT_INTERFACE_RESPONSE_SCHEMA_VERSION,
            PROJECT_INTERFACE_CONTRACT_V1 => PROJECT_INTERFACE_RESPONSE_SCHEMA_VERSION_V1,
            _ => {
                return Err(ProjectInterfaceError::new(
                    "project_contract_version_unsupported",
                ));
            }
        };
        if self.schema_version != expected_schema {
            return Err(ProjectInterfaceError::new(
                "unsupported_project_response_schema",
            ));
        }
        let expected_compatibility =
            ProjectInterfaceCompatibility::for_contract(&self.contract_version)?;
        if self.compatibility != expected_compatibility {
            return Err(ProjectInterfaceError::new(
                "project_compatibility_matrix_invalid",
            ));
        }
        self.compatibility.validate()?;
        self.snapshot.validate()?;
        match self.contract_version.as_str() {
            PROJECT_INTERFACE_CONTRACT_V2 if self.snapshot.decision_page.is_some() => Ok(()),
            PROJECT_INTERFACE_CONTRACT_V1 if self.snapshot.decision_page.is_none() => Ok(()),
            _ => Err(ProjectInterfaceError::new(
                "project_decision_page_contract_mismatch",
            )),
        }
    }

    pub fn contract_version(&self) -> &str {
        &self.contract_version
    }

    pub const fn compatibility(&self) -> &ProjectInterfaceCompatibility {
        &self.compatibility
    }

    pub const fn snapshot(&self) -> &ProjectInterfaceSnapshot {
        &self.snapshot
    }

    pub fn into_snapshot(self) -> ProjectInterfaceResult<ProjectInterfaceSnapshot> {
        self.validate()?;
        Ok(self.snapshot)
    }
}

fn validate_instances(instances: &[ProjectInstanceView]) -> ProjectInterfaceResult<()> {
    let mut aliases = BTreeSet::new();
    let mut ids = BTreeSet::new();
    let mut previous = None;
    for instance in instances {
        validate_text(&instance.instance_alias, "project_instance_invalid")?;
        if !aliases.insert(instance.instance_alias.as_str())
            || !ids.insert(instance.instance_id)
            || previous.is_some_and(|value| value >= instance.instance_alias.as_str())
            || (instance.destructive_step_active || instance.preempt_requested)
                && !instance.lease_active
        {
            return Err(ProjectInterfaceError::new("project_instance_invalid"));
        }
        previous = Some(instance.instance_alias.as_str());
    }
    Ok(())
}

fn validate_facts(facts: &[ProjectFactView]) -> ProjectInterfaceResult<()> {
    let mut identities = BTreeSet::new();
    for fact in facts {
        fact.scope.validate()?;
        for value in [
            &fact.key,
            &fact.source_snapshot_id,
            &fact.fact_schema_version,
        ] {
            validate_text(value, "project_fact_invalid")?;
        }
        validate_raw_hash(&fact.resource_bundle_hash, "project_fact_invalid")?;
        if fact.observed_at_unix_ms == 0
            || fact
                .expires_at_unix_ms
                .is_some_and(|expires| expires <= fact.observed_at_unix_ms)
            || fact.confidence_milli > 1_000
            || !identities.insert((fact.scope.clone(), fact.key.as_str()))
        {
            return Err(ProjectInterfaceError::new("project_fact_invalid"));
        }
    }
    Ok(())
}

fn validate_goals(goals: &[ProjectGoalView]) -> ProjectInterfaceResult<()> {
    let mut identities = BTreeSet::new();
    for goal in goals {
        validate_text(&goal.goal_id, "project_goal_invalid")?;
        validate_text(&goal.activity_profile_id, "project_goal_invalid")?;
        goal.scope.validate()?;
        goal.metric.validate()?;
        if goal.deadline_unix_ms == 0
            || goal.strategic_weight_milli > 1_000
            || !identities.insert((goal.activity_profile_id.as_str(), goal.goal_id.as_str()))
        {
            return Err(ProjectInterfaceError::new("project_goal_invalid"));
        }
    }
    Ok(())
}

fn validate_decisions(
    decisions: &[ProjectDecisionView],
    ledger_position: u64,
) -> ProjectInterfaceResult<()> {
    let mut ids = BTreeSet::new();
    for decision in decisions {
        for value in [
            &decision.decision_id,
            &decision.task_id,
            &decision.instance_id,
            &decision.operation_id,
            &decision.fact_snapshot_id,
        ] {
            validate_text(value, "project_decision_invalid")?;
        }
        validate_hash(&decision.catalog_hash, "project_decision_invalid")?;
        if decision.catalog_version == 0
            || decision.input_ledger_position == 0
            || decision.input_ledger_position > ledger_position
            || decision.urgency_milli > 1_000
            || !ids.insert(decision.decision_id.as_str())
        {
            return Err(ProjectInterfaceError::new("project_decision_invalid"));
        }
        for value in decision
            .approval_fact_ids
            .iter()
            .chain(decision.reason_codes.iter())
        {
            validate_text(value, "project_decision_invalid")?;
        }
    }
    Ok(())
}

fn validate_approvals(approvals: &[ProjectApprovalView]) -> ProjectInterfaceResult<()> {
    let mut ids = BTreeSet::new();
    for approval in approvals {
        validate_text(&approval.approval_id, "project_approval_invalid")?;
        validate_hash(&approval.catalog_hash, "project_approval_invalid")?;
        if approval.catalog_version == 0 || !ids.insert(approval.approval_id.as_str()) {
            return Err(ProjectInterfaceError::new("project_approval_invalid"));
        }
        match approval.target_kind {
            ApprovalTargetKind::Catalog if approval.target_id.is_none() => {}
            ApprovalTargetKind::Plan | ApprovalTargetKind::Decision
                if approval.target_id.as_ref().is_some_and(|value| {
                    validate_text(value, "project_approval_invalid").is_ok()
                }) => {}
            _ => return Err(ProjectInterfaceError::new("project_approval_invalid")),
        }
    }
    Ok(())
}

fn validate_diagnostics(
    diagnostics: &[ProjectDiagnosticView],
    ledger_position: u64,
) -> ProjectInterfaceResult<()> {
    let mut previous = 0;
    for diagnostic in diagnostics {
        if diagnostic.sequence == 0
            || diagnostic.sequence <= previous
            || diagnostic.sequence > ledger_position
            || diagnostic.timestamp_unix_ms == 0
            || diagnostic.fatal != (diagnostic.severity == EventSeverity::Fatal)
        {
            return Err(ProjectInterfaceError::new("project_diagnostic_invalid"));
        }
        if let Some(code) = &diagnostic.code {
            validate_text(code, "project_diagnostic_invalid")?;
        }
        previous = diagnostic.sequence;
    }
    Ok(())
}

impl ProjectScopeView {
    fn validate(&self) -> ProjectInterfaceResult<()> {
        let value = match self {
            Self::Instance { instance_id } => instance_id,
            Self::Server { server_id } => server_id,
            Self::Game { game_id } => game_id,
        };
        validate_text(value, "project_scope_invalid")
    }
}

impl ProjectGoalMetricView {
    fn validate(&self) -> ProjectInterfaceResult<()> {
        match self {
            Self::Fact { fact_key } => validate_text(fact_key, "project_goal_metric_invalid"),
            Self::Pool { pool_id } => validate_text(pool_id, "project_goal_metric_invalid"),
            Self::Outcome {
                task_id,
                outcome_key,
            } => {
                validate_text(task_id, "project_goal_metric_invalid")?;
                validate_text(outcome_key, "project_goal_metric_invalid")
            }
        }
    }
}

fn validate_text(value: &str, code: &'static str) -> ProjectInterfaceResult<()> {
    if value.is_empty()
        || value.len() > MAX_PROJECT_TEXT_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(ProjectInterfaceError::new(code));
    }
    Ok(())
}

fn validate_hash(value: &str, code: &'static str) -> ProjectInterfaceResult<()> {
    if !value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    }) {
        return Err(ProjectInterfaceError::new(code));
    }
    Ok(())
}

fn validate_raw_hash(value: &str, code: &'static str) -> ProjectInterfaceResult<()> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ProjectInterfaceError::new(code));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::IdentifierIssuer;

    fn hash() -> String {
        format!("sha256:{}", "a".repeat(64))
    }

    fn snapshot() -> ProjectInterfaceSnapshot {
        let ids = IdentifierIssuer::new().expect("ids");
        let instance_id = *ids.mint_instance_id().expect("instance").transport();
        ProjectInterfaceSnapshot {
            ledger_position: 7,
            project: Some(ProjectView {
                project_id: "project:neutral".to_owned(),
            }),
            instances: vec![ProjectInstanceView {
                instance_alias: "instance-neutral".to_owned(),
                instance_id,
                lease_active: false,
                queued_request_count: 0,
                takeover_cooldown_active: false,
                destructive_step_active: false,
                preempt_requested: false,
            }],
            catalog: Some(ProjectCatalogView {
                catalog_id: "project:neutral".to_owned(),
                catalog_version: 1,
                catalog_hash: hash(),
                task_count: 1,
                pool_count: 1,
                activity_profile_count: 1,
                goal_count: 1,
                timeline_event_count: 1,
            }),
            facts: vec![ProjectFactView {
                scope: ProjectScopeView::Instance {
                    instance_id: "instance-neutral".to_owned(),
                },
                key: "resource.current".to_owned(),
                content_state: ProjectFactContentState::InlineRedacted,
                observed_at_unix_ms: 10,
                expires_at_unix_ms: Some(20),
                confidence_milli: 900,
                source_snapshot_id: "snapshot-neutral".to_owned(),
                fact_schema_version: "fact.v1".to_owned(),
                resource_bundle_hash: "a".repeat(64),
            }],
            goals: vec![ProjectGoalView {
                goal_id: "goal-neutral".to_owned(),
                activity_profile_id: "activity-neutral".to_owned(),
                scope: ProjectScopeView::Instance {
                    instance_id: "instance-neutral".to_owned(),
                },
                metric: ProjectGoalMetricView::Fact {
                    fact_key: "resource.current".to_owned(),
                },
                target: 10,
                deadline_unix_ms: 30,
                strategic_weight_milli: 500,
                best_effort: false,
            }],
            decisions: vec![ProjectDecisionView {
                decision_id: "decision:neutral".to_owned(),
                task_id: "task-neutral".to_owned(),
                instance_id: "instance-neutral".to_owned(),
                operation_id: "operation-neutral".to_owned(),
                state: ProjectDecisionState::Admitted,
                catalog_hash: hash(),
                catalog_version: 1,
                input_ledger_position: 6,
                fact_snapshot_id: "snapshot-neutral".to_owned(),
                approval_fact_ids: vec!["approval:neutral".to_owned()],
                reason_codes: vec!["eligible".to_owned()],
                urgency_milli: 500,
            }],
            decision_page: Some(
                ProjectDecisionPage::new(7, DEFAULT_PROJECT_DECISION_PAGE_SIZE, 1, false, None)
                    .expect("decision page"),
            ),
            approvals: vec![ProjectApprovalView {
                approval_id: "approval:neutral".to_owned(),
                disposition: ApprovalDisposition::Approved,
                target_kind: ApprovalTargetKind::Catalog,
                target_id: None,
                catalog_hash: hash(),
                catalog_version: 1,
            }],
            runtime: ProjectRuntimeView {
                owner_epoch: *ids.mint_owner_epoch().expect("owner").transport(),
                ledger_position: 7,
                fatal: false,
                instance_count: 1,
            },
            diagnostics: vec![ProjectDiagnosticView {
                sequence: 7,
                timestamp_unix_ms: 40,
                severity: EventSeverity::Warning,
                event_type: EventType::CommandRejected,
                code: Some("neutral_warning".to_owned()),
                fatal: false,
            }],
        }
    }

    #[test]
    fn current_contract_round_trips_neutral_data() {
        let response = ProjectInterfaceResponse::new(
            ProjectInterfaceRequest::current()
                .negotiate()
                .expect("version"),
            snapshot(),
        )
        .expect("response");
        let bytes = serde_json::to_vec(&response).expect("encode");
        let json = std::str::from_utf8(&bytes).expect("UTF-8 JSON");
        assert!(!json.contains("\"source_detector\":"));
        assert!(!json.contains("\"reason_code\":"));
        let decoded: ProjectInterfaceResponse = serde_json::from_slice(&bytes).expect("decode");
        decoded.validate().expect("valid response");
        assert_eq!(decoded, response);
    }

    #[test]
    fn compatibility_matrix_rejects_unknown_only_clients() {
        let request =
            ProjectInterfaceRequest::new(vec!["actingcommand.project-interface.v9".into()])
                .expect("well formed request");
        assert_eq!(
            request.negotiate().expect_err("unsupported").code(),
            "project_contract_version_unsupported"
        );
        let compatible = ProjectInterfaceRequest::new(vec![
            "actingcommand.project-interface.v9".into(),
            PROJECT_INTERFACE_CONTRACT_V1.into(),
        ])
        .expect("mixed versions");
        assert_eq!(
            compatible.negotiate().expect("v1"),
            PROJECT_INTERFACE_CONTRACT_V1
        );
    }

    #[test]
    fn transport_rejects_unknown_fields_and_response_versions() {
        let mut request = serde_json::to_value(ProjectInterfaceRequest::current()).expect("value");
        request["unexpected"] = serde_json::json!(true);
        assert!(serde_json::from_value::<ProjectInterfaceRequest>(request).is_err());

        let mut legacy_snapshot = snapshot();
        legacy_snapshot.decision_page = None;
        let response =
            ProjectInterfaceResponse::new(PROJECT_INTERFACE_CONTRACT_V1, legacy_snapshot)
                .expect("response");
        let mut value = serde_json::to_value(response).expect("value");
        value["contract_version"] = serde_json::json!("actingcommand.project-interface.v9");
        let decoded: ProjectInterfaceResponse = serde_json::from_value(value).expect("shape");
        assert_eq!(
            decoded.validate().expect_err("version").code(),
            "project_contract_version_unsupported"
        );
    }

    #[test]
    fn decision_page_cursor_round_trips_and_is_snapshot_bound() {
        let cursor =
            ProjectDecisionPageCursor::new(10, 7, "decision:neutral").expect("decision cursor");
        let page = ProjectDecisionPageRequest::new(32, Some(cursor.clone())).expect("page request");
        let request = ProjectInterfaceRequest::current()
            .with_decision_page(page)
            .expect("paged request");
        let decoded: ProjectInterfaceRequest =
            serde_json::from_slice(&serde_json::to_vec(&request).expect("encode request"))
                .expect("decode request");
        assert_eq!(
            decoded
                .decision_page()
                .and_then(ProjectDecisionPageRequest::cursor),
            Some(&cursor)
        );
        assert_eq!(
            ProjectDecisionPageCursor::new(6, 7, "decision:neutral")
                .expect_err("cursor cannot exceed snapshot")
                .code(),
            "project_decision_cursor_invalid"
        );
    }

    #[test]
    fn snapshot_rejects_a_decision_page_from_another_ledger_position() {
        let mut snapshot = snapshot();
        snapshot.decision_page = Some(
            ProjectDecisionPage::new(6, DEFAULT_PROJECT_DECISION_PAGE_SIZE, 1, false, None)
                .expect("decision page"),
        );
        assert_eq!(
            snapshot
                .validate()
                .expect_err("mixed snapshot position")
                .code(),
            "project_decision_page_invalid"
        );
    }
}
