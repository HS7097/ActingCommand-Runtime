// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #428 S168-5b (`contracts/selection-graph.md`, "Judge step"; register §1.291): a judge
//! step evaluates the candidates one frame shows with a selection policy, records the decision,
//! and ends the run Completed on its page with one of the two keys its package declares. Its
//! decision is advice kept in the ledger: no Runtime component acts on it.
//!
//! - **The effect.** `judge` `{layout_id, policy, outcome_key, unknown_outcome_key}` is the sixth
//!   effect of a task schema `0.9` page-graph operation, beside `click`, `application`, `select`,
//!   `wait` and `read_list`. Its `to` is its `from`; it declares no member a tap, its confirmation,
//!   a retry or a repeat uses; and its state is a terminal of the static walks
//!   (`TaskPhaseOperation::judge`): the run ends there.
//! - **Admission.** The operation's members first ([`TaskProgram::validate_judge_members`], before
//!   the common operation checks), then its layout and its document ([`prepare_judge_steps`]): a
//!   `fixed_slots` layout of the step's page that rejects unknown identities, and a document that
//!   passes a select step's checks with mode `none_allowed` and a count that covers every slot.
//!   A step rule's refusal is `contained_task_select_invalid` with the detail
//!   `operation=<id> judge <token>`.
//! - **The judge path.** [`judge`] is a function of the projection, the instance fact snapshot,
//!   the instant, the tables and the document that returns the step's record and the key its run
//!   ends with. It is handed neither the run's device port nor the step's seed, and this module
//!   names no type of the device side: an architecture guard of the register's test
//!   `judge_performs_no_input` reads this file and holds it so.

use super::list_reading::READ_LIST_MEMBER;
use super::scan::{Bound, bound_inputs, record_bound};
use super::selection::{
    check_policy_document, decide, evaluator_candidates, outcome_kind, read_policy, record_outcome,
    record_reasons, record_verdicts, single_identity_feature,
};
use super::{ContainedTaskError, TaskControl, TaskOperation, TaskProgram, safe_task_local_path};
use crate::codes::ExecutionKernelCode;
use actingcommand_contract::outcome::Code;
use actingcommand_contract::{
    CandidateFeature, CandidateLayoutKind, CandidateProjection, InstanceFactSnapshot,
    TaskPhaseJudge, TaskSchema, TaskSelectionBlocking, TaskSelectionConfirmation,
    TaskSelectionPolicy, TaskSelectionRecord, TaskSelectionUnknownCheck,
    validate_candidate_layout_id,
};
use actingcommand_pack_containment::LoadedBundle;
use actingcommand_recognition_pack::{
    CandidateLayout, RecognitionEvaluator, UnknownIdentityHandling,
};
use actingcommand_selection_policy::{
    CandidateStatus, SelectionDecision, SelectionFactSnapshot, SelectionMode, SelectionOutcome,
    SelectionPolicy, SelectionTables, upper_bound_with_tables,
};
use serde::Deserialize;
use std::collections::BTreeSet;

const SELECT_INVALID: Code = ExecutionKernelCode::ContainedTaskSelectInvalid.code();
const SELECT_POLICY_MISSING: Code = ExecutionKernelCode::ContainedTaskSelectPolicyMissing.code();
const SELECT_POLICY_INVALID: Code = ExecutionKernelCode::ContainedTaskSelectPolicyInvalid.code();
const SELECT_POLICY_MISMATCH: Code = ExecutionKernelCode::ContainedTaskSelectPolicyMismatch.code();
const SELECTION_EVALUATION_FAILED: Code = ExecutionKernelCode::SelectionEvaluationFailed.code();

/// A judge step's effect, as the package writes it (`contracts/selection-graph.md`, "Judge step",
/// "Declaration"): `{layout_id, policy: {path}, outcome_key, unknown_outcome_key}`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TaskJudge {
    layout_id: String,
    policy: TaskJudgePolicy,
    outcome_key: String,
    unknown_outcome_key: String,
    /// A judge reads one frame and never declares a scan; the member is read only so that
    /// admission refuses it with its own detail (`judge scan=declared`).
    #[serde(default)]
    scan: Option<serde_json::Value>,
}

/// The task-local document a judge step names: `{path}`, a `policies/<name>.json` of its task.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskJudgePolicy {
    path: String,
}

impl TaskJudge {
    /// The step's candidate layout.
    pub(super) fn layout_id(&self) -> &str {
        &self.layout_id
    }

    /// The step's two keys as the static walks read them.
    pub(super) fn phase_judge(&self) -> TaskPhaseJudge<'_> {
        TaskPhaseJudge {
            outcome_key: &self.outcome_key,
            unknown_outcome_key: &self.unknown_outcome_key,
        }
    }
}

/// An admitted judge step: its layout, its sealed document and its two keys. Set by admission and
/// never read from the package.
#[derive(Debug)]
pub(super) struct PreparedJudge {
    layout_id: String,
    /// The document's package path, `operations/<task>/<declared path>`, below the resource root.
    path: String,
    policy: SelectionPolicy,
    /// The layout's one identity feature, when it declares one: the unknown check bounds a slot
    /// at its Known identity, or over that feature's whole domain.
    identity_feature: Option<String>,
    outcome_key: String,
    unknown_outcome_key: String,
}

impl PreparedJudge {
    /// The step's candidate layout.
    pub(super) fn layout_id(&self) -> &str {
        &self.layout_id
    }

    /// The step's sealed document, whose tables the run binds.
    pub(super) fn policy(&self) -> &SelectionPolicy {
        &self.policy
    }
}

/// What one judge step decided: its record, with the confirmation `judge_only`, and the key its
/// run ends with.
#[derive(Debug)]
pub(super) struct Judged {
    pub(super) record: TaskSelectionRecord,
    pub(super) outcome_key: String,
}

/// `contained_task_select_invalid` of the operation `operation_id`, `operation=<id> judge <token>`.
fn refusal(operation_id: &str, token: &str) -> ContainedTaskError {
    ContainedTaskError::with_detail(
        SELECT_INVALID,
        format!("operation={operation_id} judge {token}"),
    )
}

impl TaskOperation {
    /// Whether the operation is a judge step.
    pub(super) const fn declares_judge(&self) -> bool {
        self.judge.is_some()
    }

    /// The first member a judge step never takes, in the order of the contract's table "Members
    /// of a judge step": another effect, a retry, a guard and its source form, a confirmation, a
    /// wait after a tap, a recheck before one, a window after a confirmation, a guard refusal, a
    /// retry's members and a recovery, a per-run cap, and the `linear_steps` members.
    fn refused_judge_member(&self) -> Option<&'static str> {
        [
            ("click", self.click.is_some()),
            ("application", self.application.is_some()),
            ("select", self.select.is_some()),
            ("wait", self.wait.is_some()),
            (READ_LIST_MEMBER, self.read_list.is_some()),
            ("retryable", self.retryable == Some(true)),
            ("guard", self.guard.is_some()),
            (
                "unguarded_trusted_coordinate",
                self.unguarded_trusted_coordinate,
            ),
            ("expect_after", self.expect_after.is_some()),
            ("post_delay_ms", self.post_delay_ms.is_some()),
            ("input_recheck", self.input_recheck.is_some()),
            ("observation_window", self.observation_window.is_some()),
            ("on_guard_refused", self.on_guard_refused.is_some()),
            ("max_attempts", self.max_attempts.is_some()),
            ("retry_interval_ms", self.retry_interval_ms.is_some()),
            ("on_error", self.on_error.is_some()),
            ("max_runs", self.max_runs.is_some()),
            ("on_max_runs", self.on_max_runs.is_some()),
            ("transition", self.transition.is_some()),
            ("optional", self.optional.is_some()),
        ]
        .into_iter()
        .find_map(|(member, declared)| declared.then_some(member))
    }
}

impl TaskProgram {
    /// Whether the task declares a judge step.
    pub(super) fn declares_judge_steps(&self) -> bool {
        self.operations.iter().any(TaskOperation::declares_judge)
    }

    /// The page-graph admission of every judge step's members, before the common operation
    /// checks (`contracts/selection-graph.md`, "Judge step", "Declaration" and "Members of a
    /// judge step"): task schema `0.9`, a task that declares `scheduling_outcome`
    /// (`contained_task_outcome_declaration_incomplete` otherwise), `to` its `from` page, two
    /// distinct keys, no `scan`, none of the members [`TaskOperation::refused_judge_member`]
    /// names, and no designated operation or member of a designated effect set. Each step
    /// refusal is `contained_task_select_invalid`, `operation=<id> judge <token>`.
    pub(super) fn validate_judge_members(&self) -> Result<(), ContainedTaskError> {
        for operation in &self.operations {
            let Some(judge) = &operation.judge else {
                continue;
            };
            let refuse = |token: &str| Err(refusal(&operation.id, token));
            if !self.schema_at_least(TaskSchema::V0_9) {
                return refuse("schema=before_0_9");
            }
            let Some(scheduling) = &self.scheduling_outcome else {
                return Err(ContainedTaskError::with_detail(
                    ExecutionKernelCode::ContainedTaskOutcomeDeclarationIncomplete.code(),
                    format!("operation={} judge scheduling_outcome=absent", operation.id),
                ));
            };
            // The step stays on its page: `to` names exactly its `from` page.
            let to_from = operation
                .to
                .as_ref()
                .and_then(|to| to.normalized().ok())
                .is_some_and(|pages| match pages.as_slice() {
                    [page] => {
                        crate::canonical_page_anchor(&self.game, page)
                            == crate::canonical_page_anchor(&self.game, &operation.from)
                    }
                    _ => false,
                });
            if !to_from {
                return refuse("to=not_from");
            }
            if judge.outcome_key == judge.unknown_outcome_key {
                return refuse("keys=equal");
            }
            if judge.scan.as_ref().is_some_and(|scan| !scan.is_null()) {
                return refuse("scan=declared");
            }
            if let Some(member) = operation.refused_judge_member() {
                return refuse(&format!("{member}=declared"));
            }
            if scheduling
                .designated_operations()
                .contains(&operation.id.as_str())
            {
                return refuse("designated=declared");
            }
        }
        Ok(())
    }
}

/// Contained task admission of every judge step's layout and document, after the program and
/// the page set are validated (`contracts/selection-graph.md`, "Judge step", "Declaration"): the
/// layout is a candidate layout of the recognition pack on the step's `from` page, as for a select
/// step; it is `fixed_slots` and rejects unknown identities (`judge layout=not_fixed_slots`); the
/// document passes every check of a select step's (present, read, applying to the layout) but the
/// one candidate a select step chooses, with mode `none_allowed` (`judge mode=not_none_allowed`)
/// and a `required_count` of at least the layout's slots (`judge required_count=below_slots`);
/// and every field it reads has a domain the layout gives it (`upper_bound <detail>`).
pub(super) fn prepare_judge_steps(
    program: &mut TaskProgram,
    control: &TaskControl,
    bundle: &LoadedBundle,
    evaluator: &RecognitionEvaluator,
) -> Result<(), ContainedTaskError> {
    let task_id = program.task_id.clone();
    for operation in &mut program.operations {
        let Some(judge) = &operation.judge else {
            continue;
        };
        let id = operation.id.clone();
        let failure = |code: Code, detail: String| {
            ContainedTaskError::with_detail(code, format!("operation={id} {detail}"))
        };
        if validate_candidate_layout_id(&judge.layout_id).is_err()
            || !safe_task_local_path(&judge.policy.path)
            || !judge.policy.path.starts_with("policies/")
            || !judge.policy.path.ends_with(".json")
        {
            return Err(failure(
                SELECT_INVALID,
                "judge declaration is malformed".to_owned(),
            ));
        }
        let layout = evaluator
            .candidate_layout(&judge.layout_id)
            .ok_or_else(|| {
                failure(
                    SELECT_INVALID,
                    format!(
                        "layout={} is not a candidate layout of the recognition pack",
                        judge.layout_id
                    ),
                )
            })?;
        if crate::canonical_page_anchor(&control.game, &layout.page_id)
            != crate::canonical_page_anchor(&control.game, &operation.from)
        {
            return Err(failure(
                SELECT_INVALID,
                format!(
                    "layout={} belongs to page {}, the step runs from {}",
                    layout.id, layout.page_id, operation.from
                ),
            ));
        }
        if layout.kind != CandidateLayoutKind::FixedSlots
            || layout.unknown_identity != UnknownIdentityHandling::Reject
        {
            return Err(refusal(&id, "layout=not_fixed_slots"));
        }
        let path = format!("operations/{task_id}/{}", judge.policy.path);
        let bytes = bundle
            .resource_entry(&path)
            .map_err(|_| failure(SELECT_POLICY_MISSING, format!("policy={path}")))?;
        let policy = read_policy(bytes)
            .map_err(|reason| failure(SELECT_POLICY_INVALID, format!("policy={path} {reason}")))?;
        check_policy_document(&policy, layout)
            .map_err(|reason| failure(SELECT_POLICY_MISMATCH, format!("policy={path} {reason}")))?;
        if policy.selection.mode != SelectionMode::NoneAllowed {
            return Err(refusal(&id, "mode=not_none_allowed"));
        }
        if (policy.selection.required_count as usize) < layout.slots.len() {
            return Err(refusal(&id, "required_count=below_slots"));
        }
        let identity_feature = single_identity_feature(layout);
        bound_inputs(layout, &policy, identity_feature.as_deref())
            .map_err(|detail| failure(SELECT_INVALID, format!("upper_bound {detail}")))?;
        let prepared = PreparedJudge {
            layout_id: judge.layout_id.clone(),
            path,
            policy,
            identity_feature,
            outcome_key: judge.outcome_key.clone(),
            unknown_outcome_key: judge.unknown_outcome_key.clone(),
        };
        operation.prepared_judge = Some(Box::new(prepared));
    }
    Ok(())
}

/// The judge path (`contracts/selection-graph.md`, "Execution of a judge step", steps 2 to 5):
/// the document `prepared` evaluated over the actionable candidates of `projection`, the
/// projection of the step's frame F1 with its layout `layout`, with the instance fact snapshot
/// and the instant the host supplied and the run's `tables`. It returns the record, with the
/// confirmation `judge_only` and `unknown_check`, and the key the run ends with: the step's
/// `outcome_key` only when the evaluation is known, the unknown check does not block and no
/// candidate is `unknown_dropped` (a static table drop included), its `unknown_outcome_key`
/// otherwise. An `unknown` evaluation keeps no verdict. A failure of the evaluation ends the run
/// with its code and no record.
pub(super) fn judge(
    prepared: &PreparedJudge,
    layout: &CandidateLayout,
    projection: &CandidateProjection,
    (snapshot, now_unix_ms): (&InstanceFactSnapshot, u64),
    tables: &SelectionTables,
) -> Result<Judged, ContainedTaskError> {
    // The rows of the record lists the document declares, which `keyed_fact` reads, as for a
    // select step.
    let facts = SelectionFactSnapshot::from_instance_snapshot_with_lists(
        snapshot,
        now_unix_ms,
        &prepared.policy.record_list_keys(),
    );
    let decision = decide(
        &prepared.policy,
        tables,
        layout,
        projection,
        &facts,
        now_unix_ms,
    )?;
    // Mode `none_allowed` with a count that covers every slot chooses every survivor, so the
    // evaluator answers `selected`, `empty` or `unknown` only.
    let known = match decision.outcome {
        SelectionOutcome::Selected { .. } | SelectionOutcome::Empty => true,
        SelectionOutcome::Unknown { .. } => false,
        SelectionOutcome::Insufficient { .. } | SelectionOutcome::Ambiguous { .. } => {
            return Err(ContainedTaskError::with_detail(
                SELECTION_EVALUATION_FAILED,
                format!(
                    "judge outcome={} where every survivor is chosen",
                    outcome_kind(&decision.outcome)
                ),
            ));
        }
    };
    let unknown_check = unknown_check(
        prepared,
        layout,
        projection,
        &decision,
        known,
        (&facts, tables, now_unix_ms),
    )?;
    let dropped = decision
        .candidates
        .iter()
        .any(|verdict| verdict.status == CandidateStatus::UnknownDropped);
    let judged = known && unknown_check.blocking == TaskSelectionBlocking::None && !dropped;
    let record = TaskSelectionRecord {
        layout_id: projection.layout_id().to_owned(),
        page_id: projection.page_id().to_owned(),
        projection: projection.clone().into(),
        policy: TaskSelectionPolicy {
            path: prepared.path.clone(),
            policy_sha256: decision.policy_sha256.clone(),
            policy_id: decision.policy_id.clone(),
        },
        fact_snapshot_id: decision.fact_snapshot_id.clone(),
        input_ledger_position: snapshot.ledger_position,
        input_fact_revisions: None,
        now_unix_ms,
        input_sha256: decision.input_sha256.clone(),
        selection_result: record_outcome(&decision.outcome)?,
        outcome_key: decision.outcome_key.clone(),
        selected: decision.selected.clone(),
        // An `unknown` evaluation has no verdict (the table of results).
        verdicts: if known {
            record_verdicts(&decision.candidates)?
        } else {
            Vec::new()
        },
        reasons: record_reasons(&decision.reasons),
        confirmation: TaskSelectionConfirmation::JudgeOnly,
        unknown_check: Some(unknown_check),
        multi_select: None,
    };
    Ok(Judged {
        record,
        outcome_key: if judged {
            prepared.outcome_key.clone()
        } else {
            prepared.unknown_outcome_key.clone()
        },
    })
}

/// Step 4 of "Execution of a judge step": the unknown instances are the actionable candidates the
/// kernel kept from the evaluator (an unresolved consensus feature, an identity feature that read
/// no identity) and, when the evaluation is `known`, those the evaluator dropped as unknown; a
/// static table drop is none of them. Each is bounded at its Known identity, or over the identity
/// feature's whole domain without one, with the pinned facts at their instant, as for a
/// single-frame select that declares `on_empty`. The check never blocks an `unknown` evaluation;
/// a known one is blocked exactly by an unknown instance.
fn unknown_check(
    prepared: &PreparedJudge,
    layout: &CandidateLayout,
    projection: &CandidateProjection,
    decision: &SelectionDecision,
    known: bool,
    (facts, tables, now_unix_ms): (&SelectionFactSnapshot, &SelectionTables, u64),
) -> Result<TaskSelectionUnknownCheck, ContainedTaskError> {
    let handed = evaluator_candidates(layout, projection)
        .into_iter()
        .map(|candidate| candidate.candidate_id)
        .collect::<BTreeSet<_>>();
    let dropped_as_unknown = decision
        .candidates
        .iter()
        .filter(|verdict| {
            known && verdict.status == CandidateStatus::UnknownDropped && !verdict.static_table_drop
        })
        .map(|verdict| verdict.candidate_id.as_str())
        .collect::<BTreeSet<_>>();
    let unknown = projection
        .candidates()
        .iter()
        .filter(|candidate| {
            candidate.actionable
                && (!handed.contains(&candidate.id)
                    || dropped_as_unknown.contains(candidate.id.as_str()))
        })
        .collect::<Vec<_>>();
    let upper = if unknown.is_empty() {
        None
    } else {
        let identity_feature = prepared.identity_feature.as_deref();
        let (field_domains, domain) = bound_inputs(layout, &prepared.policy, identity_feature)
            .map_err(|detail| {
                ContainedTaskError::with_detail(SELECT_INVALID, format!("upper_bound {detail}"))
            })?;
        let mut upper = Bound::None;
        for candidate in &unknown {
            let identities = identity_feature
                .and_then(|name| match candidate.features.get(name) {
                    Some(CandidateFeature::Identity { value, .. }) => Some(vec![value.clone()]),
                    _ => None,
                })
                .unwrap_or_else(|| domain.clone());
            let bound = upper_bound_with_tables(
                &prepared.policy,
                tables,
                facts,
                now_unix_ms,
                &identities,
                &field_domains,
            )
            .map_err(|error| {
                ContainedTaskError::with_detail(SELECTION_EVALUATION_FAILED, error.to_string())
            })?;
            upper = upper.max(Bound::from(bound));
        }
        Some(upper)
    };
    let unknown_instances = u32::try_from(unknown.len()).unwrap_or(u32::MAX);
    Ok(TaskSelectionUnknownCheck {
        unknown_instances,
        blocking: if known && unknown_instances > 0 {
            TaskSelectionBlocking::UnknownInstances
        } else {
            TaskSelectionBlocking::None
        },
        upper_bound: record_bound(upper),
    })
}
