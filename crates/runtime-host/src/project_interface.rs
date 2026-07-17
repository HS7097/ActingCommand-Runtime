// SPDX-License-Identifier: AGPL-3.0-only

//! Translation from Runtime-owned domain state into the read-only project transport contract.

use crate::policy_host::{
    LoadedCatalog, PolicyDispatchPage, PolicyDispatchProjection, PolicyDispatchProjectionState,
};
use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::{
    ApprovalDecisionRecord, ApprovalTarget, EventSeverity, EventType, FactRecord, FactScope,
    ProjectApprovalView, ProjectCatalogView, ProjectDecisionPage, ProjectDecisionPageCursor,
    ProjectDecisionState, ProjectDecisionView, ProjectDiagnosticView, ProjectFactContentState,
    ProjectFactView, ProjectGoalMetricView, ProjectGoalView, ProjectInstanceView,
    ProjectInterfaceRequest, ProjectInterfaceResponse, ProjectInterfaceSnapshot,
    ProjectRuntimeView, ProjectScopeView, ProjectView, RuntimeControlPlaneStatus, RuntimeErrorCode,
};
use actingcommand_policy::{MetricRef, ScopeSelector};

const MAX_PROJECT_DIAGNOSTICS: usize = 256;

pub(crate) struct ProjectDiagnosticProjection {
    pub(crate) sequence: u64,
    pub(crate) timestamp_unix_ms: u64,
    pub(crate) severity: EventSeverity,
    pub(crate) event_type: EventType,
}

pub(crate) struct ProjectInterfaceProjection {
    pub(crate) ledger_position: u64,
    pub(crate) catalog: Option<LoadedCatalog>,
    pub(crate) instances: RuntimeControlPlaneStatus,
    pub(crate) facts: Vec<FactRecord>,
    pub(crate) decisions: PolicyDispatchPage,
    pub(crate) approvals: Vec<ApprovalDecisionRecord>,
    pub(crate) diagnostics: Vec<ProjectDiagnosticProjection>,
    pub(crate) fatal: bool,
}

impl ProjectInterfaceProjection {
    pub(crate) fn into_response(
        self,
        request: &ProjectInterfaceRequest,
    ) -> RuntimeHostResult<ProjectInterfaceResponse> {
        let negotiated_version = request.negotiate().map_err(|error| {
            RuntimeHostError::request(
                error.code(),
                "negotiate_project_interface",
                RuntimeErrorCode::ProtocolInvalid,
            )
        })?;
        let catalog = self.catalog.as_ref().map(project_catalog).transpose()?;
        let project = catalog.as_ref().map(|catalog| ProjectView {
            project_id: catalog.catalog_id.clone(),
        });
        let instances = self
            .instances
            .instances()
            .iter()
            .map(|instance| ProjectInstanceView {
                instance_alias: instance.instance_alias().to_owned(),
                instance_id: instance.instance_id(),
                lease_active: instance.lease_active(),
                queued_request_count: instance.queued_request_count(),
                takeover_cooldown_active: instance.takeover_cooldown_active(),
                destructive_step_active: instance.destructive_step_active(),
                preempt_requested: instance.preempt_requested(),
            })
            .collect::<Vec<_>>();
        let facts = self.facts.into_iter().map(project_fact).collect::<Vec<_>>();
        let goals = self.catalog.as_ref().map(project_goals).unwrap_or_default();
        let mut decisions = self
            .decisions
            .dispatches
            .iter()
            .map(|decision| (decision.intent_sequence, project_decision(decision.clone())))
            .collect::<Vec<_>>();
        let original_decision_count = decisions.len();
        let decision_snapshot_position = self.decisions.snapshot_ledger_position;
        let requested_decision_limit = self.decisions.requested_limit;
        let older_decisions_exist = self.decisions.has_more;
        let legacy_contract =
            negotiated_version == actingcommand_contract::PROJECT_INTERFACE_CONTRACT_V1;
        if legacy_contract && older_decisions_exist {
            return Err(RuntimeHostError::request(
                "project_interface_v1_requires_v2",
                "project_runtime_interface",
                RuntimeErrorCode::ProtocolInvalid,
            ));
        }
        let approvals = self
            .approvals
            .into_iter()
            .map(project_approval)
            .collect::<Vec<_>>();
        let diagnostics = self
            .diagnostics
            .into_iter()
            .map(|diagnostic| ProjectDiagnosticView {
                sequence: diagnostic.sequence,
                timestamp_unix_ms: diagnostic.timestamp_unix_ms,
                severity: diagnostic.severity,
                event_type: diagnostic.event_type,
                code: None,
                fatal: diagnostic.severity == EventSeverity::Fatal,
            })
            .collect::<Vec<_>>();
        loop {
            let decision_page = if negotiated_version
                == actingcommand_contract::PROJECT_INTERFACE_CONTRACT_V2
            {
                let has_more = older_decisions_exist || decisions.len() < original_decision_count;
                let next_cursor = if has_more {
                    let (intent_sequence, decision) = decisions.last().ok_or_else(|| {
                        RuntimeHostError::request(
                            "project_interface_response_too_large",
                            "project_runtime_interface",
                            RuntimeErrorCode::ProtocolInvalid,
                        )
                    })?;
                    Some(
                        ProjectDecisionPageCursor::new(
                            decision_snapshot_position,
                            *intent_sequence,
                            decision.decision_id.clone(),
                        )
                        .map_err(project_contract_error)?,
                    )
                } else {
                    None
                };
                Some(
                    ProjectDecisionPage::new(
                        decision_snapshot_position,
                        requested_decision_limit,
                        u16::try_from(decisions.len()).map_err(|_| {
                            RuntimeHostError::fatal(
                                "project_decision_count_overflow",
                                "project_runtime_interface",
                                RuntimeErrorCode::RuntimeFatal,
                            )
                        })?,
                        has_more,
                        next_cursor,
                    )
                    .map_err(project_contract_error)?,
                )
            } else {
                None
            };
            let snapshot = ProjectInterfaceSnapshot {
                ledger_position: self.ledger_position,
                project: project.clone(),
                instances: instances.clone(),
                catalog: catalog.clone(),
                facts: facts.clone(),
                goals: goals.clone(),
                decisions: decisions
                    .iter()
                    .map(|(_, decision)| decision.clone())
                    .collect(),
                decision_page,
                approvals: approvals.clone(),
                runtime: ProjectRuntimeView {
                    owner_epoch: self.instances.owner_epoch(),
                    ledger_position: self.ledger_position,
                    fatal: self.fatal,
                    instance_count: self.instances.instances().len() as u32,
                },
                diagnostics: diagnostics.clone(),
            };
            match ProjectInterfaceResponse::new(negotiated_version, snapshot) {
                Ok(response) => return Ok(response),
                Err(error)
                    if error.code() == "project_interface_response_too_large"
                        && decisions.len() > 1
                        && !legacy_contract =>
                {
                    decisions.pop();
                }
                Err(error)
                    if error.code() == "project_interface_response_too_large"
                        && legacy_contract =>
                {
                    return Err(RuntimeHostError::request(
                        "project_interface_v1_requires_v2",
                        "project_runtime_interface",
                        RuntimeErrorCode::ProtocolInvalid,
                    ));
                }
                Err(error) => return Err(project_contract_error(error)),
            }
        }
    }
}

fn project_contract_error(
    error: actingcommand_contract::ProjectInterfaceError,
) -> RuntimeHostError {
    if error.code() == "project_interface_response_too_large" {
        RuntimeHostError::request(
            error.code(),
            "project_runtime_interface",
            RuntimeErrorCode::ProtocolInvalid,
        )
    } else {
        RuntimeHostError::fatal(
            error.code(),
            "project_runtime_interface",
            RuntimeErrorCode::RuntimeFatal,
        )
    }
}

fn project_catalog(catalog: &LoadedCatalog) -> RuntimeHostResult<ProjectCatalogView> {
    let bundle = catalog.compiled().catalog();
    let goal_count = bundle
        .activity
        .profiles
        .iter()
        .map(|profile| profile.goals.len())
        .sum::<usize>();
    Ok(ProjectCatalogView {
        catalog_id: catalog.generation().catalog_id().to_owned(),
        catalog_version: catalog.generation().catalog_version(),
        catalog_hash: catalog.generation().catalog_hash().to_owned(),
        task_count: bounded_count(bundle.tasks.tasks.len())?,
        pool_count: bounded_count(bundle.pools.pools.len())?,
        activity_profile_count: bounded_count(bundle.activity.profiles.len())?,
        goal_count: bounded_count(goal_count)?,
        timeline_event_count: bounded_count(bundle.timeline.events.len())?,
    })
}

fn project_fact(fact: FactRecord) -> ProjectFactView {
    let content_state = match fact.content {
        actingcommand_contract::FactContent::Inline { .. } => {
            ProjectFactContentState::InlineRedacted
        }
        actingcommand_contract::FactContent::Artifact { .. } => {
            ProjectFactContentState::ArtifactRedacted
        }
    };
    ProjectFactView {
        scope: project_fact_scope(fact.scope),
        key: fact.key,
        content_state,
        observed_at_unix_ms: fact.observed_at_unix_ms,
        expires_at_unix_ms: fact.expires_at_unix_ms,
        confidence_milli: fact.confidence_milli,
        source_snapshot_id: fact.source_snapshot_id,
        fact_schema_version: fact.schema_version,
        resource_bundle_hash: fact.resource_bundle_hash,
    }
}

fn project_goals(catalog: &LoadedCatalog) -> Vec<ProjectGoalView> {
    let mut goals = catalog
        .compiled()
        .catalog()
        .activity
        .profiles
        .iter()
        .flat_map(|profile| {
            profile.goals.iter().map(|goal| ProjectGoalView {
                goal_id: goal.id.clone(),
                activity_profile_id: profile.id.clone(),
                scope: project_scope(&profile.scope),
                metric: project_metric(&goal.metric),
                target: goal.target,
                deadline_unix_ms: goal.deadline_unix_ms,
                strategic_weight_milli: goal.strategic_weight_milli,
                best_effort: goal.best_effort,
            })
        })
        .collect::<Vec<_>>();
    goals.sort_by(|left, right| {
        left.activity_profile_id
            .cmp(&right.activity_profile_id)
            .then_with(|| left.goal_id.cmp(&right.goal_id))
    });
    goals
}

fn project_decision(decision: PolicyDispatchProjection) -> ProjectDecisionView {
    ProjectDecisionView {
        decision_id: decision.data.decision_id,
        task_id: decision.data.task_id,
        instance_id: decision.data.instance_id,
        operation_id: decision.data.operation_id,
        state: match decision.state {
            PolicyDispatchProjectionState::Intent => ProjectDecisionState::Intent,
            PolicyDispatchProjectionState::Admitted => ProjectDecisionState::Admitted,
            PolicyDispatchProjectionState::Rejected => ProjectDecisionState::Rejected,
            PolicyDispatchProjectionState::Completed => ProjectDecisionState::Completed,
        },
        catalog_hash: decision.data.catalog_hash,
        catalog_version: decision.data.catalog_version,
        input_ledger_position: decision.data.input_ledger_position,
        fact_snapshot_id: decision.data.fact_snapshot_id,
        approval_fact_ids: decision.data.approval_fact_ids,
        reason_codes: decision
            .data
            .reasons
            .into_iter()
            .map(|reason| reason.code)
            .collect(),
        urgency_milli: decision.data.urgency_milli,
    }
}

fn project_approval(approval: ApprovalDecisionRecord) -> ProjectApprovalView {
    let target_id = match approval.target() {
        ApprovalTarget::Catalog { .. } => None,
        ApprovalTarget::Plan { plan_id, .. } => Some(plan_id.clone()),
        ApprovalTarget::Decision { decision_id, .. } => Some(decision_id.clone()),
    };
    ProjectApprovalView {
        approval_id: approval.approval_id().to_owned(),
        disposition: approval.disposition(),
        target_kind: approval.target().kind(),
        target_id,
        catalog_hash: approval.target().catalog_hash().to_owned(),
        catalog_version: approval.target().catalog_version(),
    }
}

fn project_fact_scope(scope: FactScope) -> ProjectScopeView {
    match scope {
        FactScope::Instance { instance_id } => ProjectScopeView::Instance { instance_id },
        FactScope::Server { server_id } => ProjectScopeView::Server { server_id },
        FactScope::Game { game_id } => ProjectScopeView::Game { game_id },
    }
}

fn project_scope(scope: &ScopeSelector) -> ProjectScopeView {
    match scope {
        ScopeSelector::Instance { instance_id } => ProjectScopeView::Instance {
            instance_id: instance_id.clone(),
        },
        ScopeSelector::Server { server_id } => ProjectScopeView::Server {
            server_id: server_id.clone(),
        },
        ScopeSelector::Game { game_id } => ProjectScopeView::Game {
            game_id: game_id.clone(),
        },
    }
}

fn project_metric(metric: &MetricRef) -> ProjectGoalMetricView {
    match metric {
        MetricRef::Fact { fact_key } => ProjectGoalMetricView::Fact {
            fact_key: fact_key.clone(),
        },
        MetricRef::Pool { pool_id } => ProjectGoalMetricView::Pool {
            pool_id: pool_id.clone(),
        },
        MetricRef::Outcome {
            task_id,
            outcome_key,
        } => ProjectGoalMetricView::Outcome {
            task_id: task_id.clone(),
            outcome_key: outcome_key.clone(),
        },
    }
}

fn bounded_count(value: usize) -> RuntimeHostResult<u32> {
    u32::try_from(value).map_err(|_| {
        RuntimeHostError::fatal(
            "project_interface_count_overflow",
            "project_runtime_interface",
            RuntimeErrorCode::RuntimeFatal,
        )
    })
}

pub(crate) fn retain_recent_diagnostics(
    mut diagnostics: Vec<ProjectDiagnosticProjection>,
) -> Vec<ProjectDiagnosticProjection> {
    if diagnostics.len() > MAX_PROJECT_DIAGNOSTICS {
        diagnostics.drain(..diagnostics.len() - MAX_PROJECT_DIAGNOSTICS);
    }
    diagnostics
}
