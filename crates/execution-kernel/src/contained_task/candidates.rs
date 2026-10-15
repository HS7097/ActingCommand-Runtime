// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #390 EXEC-1 (forced-compromises mapping, rows E-04 and E-13): ordered candidate
//! operations on one page (`contracts/resource-declarations.md`, section Task schema `0.9`
//! candidate operations). Both declarations belong to task schema `0.9` page-graph tasks.
//!
//! - **`when`.** An operation is a candidate only while its target passes (`present`) or does
//!   not pass (`absent`) on the frame its step would start on. The candidates of a page keep
//!   the order operations are chosen in today: the operations from the page in declaration
//!   order, then those from `any`. A candidate whose `when` does not hold is withdrawn before
//!   its step starts, and the next one is taken.
//! - **`on_guard_refused: next_candidate`.** A click whose guard refuses on its step's frame,
//!   before any input of the step, ends its step without an input, and the page's next
//!   candidate is taken on the same frame. A refusal on a retry attempt or by an input recheck
//!   (Workflow #390 EXEC-2) keeps the operation's failure decision.
//! - A page whose candidates were all passed over ends the run with
//!   `contained_task_operation_candidates_exhausted`. A frame that matches more than one page
//!   is still a conflict.

use super::{
    ContainedTaskError, ContainedTaskGuardEvaluation, ContainedTaskRunError, ContainedTaskRuntime,
    ContainedTaskTrace, PageObservation, PreparedContainedTask, TaskOperation, TaskProgram,
};
use crate::codes::ExecutionKernelCode;
use crate::{RunDecisionError, RunDirective, RunStateMachine};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_recognition_pack::{RecognitionEvaluator, TargetKind};
use serde::Deserialize;

/// The task schema that admits both declarations.
const TASK_SCHEMA: &str = "0.9";

/// An operation's `when`, as the package writes it: `{"present": "<target>"}` or
/// `{"absent": "<target>"}`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum TaskWhen {
    /// The operation is a candidate while the target passes on the step's frame.
    Present(String),
    /// The operation is a candidate while the target does not pass on the step's frame.
    Absent(String),
}

impl TaskWhen {
    pub(super) fn target_id(&self) -> &str {
        match self {
            Self::Present(target) | Self::Absent(target) => target,
        }
    }

    const fn holds(&self, passed: bool) -> bool {
        match self {
            Self::Present(_) => passed,
            Self::Absent(_) => !passed,
        }
    }
}

/// What a guard refusal on the step's frame does, as the package writes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum TaskGuardRefusal {
    /// The step ends without an input and the page's next candidate is taken on the same
    /// frame.
    NextCandidate,
}

impl TaskOperation {
    /// Workflow #390 EXEC-1: the page's next candidate may run instead of this operation (it
    /// declares `when` or `on_guard_refused`).
    pub(super) const fn may_be_passed_over(&self) -> bool {
        self.when.is_some() || self.on_guard_refused.is_some()
    }
}

fn invalid(operation: &TaskOperation, field: &str) -> ContainedTaskError {
    ContainedTaskError::with_detail(
        ExecutionKernelCode::ContainedTaskOperationInvalid.code(),
        format!("operation={} field={field}", operation.id),
    )
}

/// Admission of an operation's `when` and `on_guard_refused`
/// (`contained_task_operation_invalid`): task schema `0.9`; a `when` names a target;
/// `on_guard_refused` is declared on a click with a declared guard.
pub(super) fn validate_operation(
    operation: &TaskOperation,
    schema_version: &str,
) -> Result<(), ContainedTaskError> {
    if operation
        .when
        .as_ref()
        .is_some_and(|when| schema_version != TASK_SCHEMA || when.target_id().trim().is_empty())
    {
        return Err(invalid(operation, "when"));
    }
    if operation.on_guard_refused.is_some()
        && (schema_version != TASK_SCHEMA
            || operation.click.is_none()
            || operation.guard.is_none()
            || operation.unguarded_trusted_coordinate)
    {
        return Err(invalid(operation, "on_guard_refused"));
    }
    Ok(())
}

impl TaskProgram {
    /// Workflow #390 EXEC-1: a `when` names a target of the recognition pack that is judged on
    /// one frame (not a `click_only` target, not one under target consensus), and is not
    /// declared on the entry application (the first operation, an `application` effect from
    /// `any`, which runs before any page is known). Its text reads, one consumer, stay within
    /// the static budget as a guard's do; an exceeded budget is a refused declaration at the
    /// `when`.
    pub(super) fn validate_when_targets(
        &self,
        operation_path: &str,
        evaluator: &RecognitionEvaluator,
    ) -> Result<(), ContainedTaskError> {
        for (index, operation) in self.operations.iter().enumerate() {
            let Some(when) = &operation.when else {
                continue;
            };
            let target = when.target_id();
            let entry_application =
                index == 0 && operation.from == "any" && operation.application.is_some();
            let judged = evaluator
                .target_kind(target)
                .is_ok_and(|kind| kind != TargetKind::ClickOnly)
                && !evaluator.pack().target_consensus.contains_key(target);
            if entry_application || !judged {
                return Err(invalid(operation, "when"));
            }
            let mut count = actingcommand_recognition_pack::TextReadCount::default();
            if let Err(overrun) = evaluator.count_text_reads(&mut count, target) {
                let mut refused = ContainedTaskError::with_detail(
                    SharedCode::ResourceDeclarationInvalid.code(),
                    format!(
                        "operation={} text_read_budget={} used={} limit={}",
                        operation.id, overrun.budget, overrun.used, overrun.limit
                    ),
                );
                refused.declaration_issue =
                    Some(Box::new(actingcommand_contract::ResourceDeclarationIssue {
                        declaration_file: operation_path.to_owned(),
                        field_path: format!("/operations/{index}/when"),
                        schema_version: Some(self.schema_version.clone()),
                        reason: actingcommand_contract::ResourceDeclarationReason::InvalidValue,
                    }));
                return Err(refused);
            }
        }
        Ok(())
    }
}

impl PreparedContainedTask {
    /// Workflow #390 EXEC-1: judges `decision`, the execution-owned transition's next directive
    /// (`RunStateMachine::next_directive`) on the frame of `observation`. An operation it
    /// dispatches whose `when` does not hold on that frame is withdrawn before its step starts
    /// (`None`: the caller asks the transition again, and the page's next candidate is
    /// dispatched). A page whose candidates were all passed over on the frame ends the run with
    /// `contained_task_operation_candidates_exhausted` (`page_id`, and `count` the candidates
    /// passed over); any other refused decision is `contained_task_state_invalid`, as before.
    pub(super) fn candidate_directive<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        machine: &mut RunStateMachine,
        decision: Result<RunDirective, RunDecisionError>,
        observation: Option<&PageObservation>,
    ) -> Result<Option<RunDirective>, ContainedTaskRunError<R::Error>> {
        let directive = match decision {
            Ok(directive) => directive,
            Err(error)
                if error.code().as_str()
                    == ExecutionKernelCode::ContainedTaskOperationCandidatesExhausted.as_str() =>
            {
                return Err(candidates_exhausted(
                    machine.current_page().unwrap_or_default(),
                    machine.passed_over().len(),
                    error.message(),
                )
                .into());
            }
            Err(_) => {
                return Err(
                    ContainedTaskError::new(SharedCode::ContainedTaskStateInvalid.code()).into(),
                );
            }
        };
        let operation_id = match directive {
            RunDirective::ExecuteOperation {
                ref operation_id, ..
            } => operation_id.clone(),
            other => return Ok(Some(other)),
        };
        let operation = self
            .program
            .operations
            .iter()
            .find(|operation| operation.id == operation_id)
            .ok_or_else(|| {
                ContainedTaskError::new(ExecutionKernelCode::ContainedTaskOperationMissing.code())
            })?;
        if operation.when.is_none() {
            return Ok(Some(directive));
        }
        let observed = observation
            .ok_or_else(|| ContainedTaskError::new(SharedCode::ContainedTaskPageUnknown.code()))?;
        if self.when_holds(runtime, operation, &observed.scene)? {
            return Ok(Some(directive));
        }
        machine
            .operation_withdrawn(&operation_id)
            .map_err(|_| ContainedTaskError::new(SharedCode::ContainedTaskStateInvalid.code()))?;
        Ok(None)
    }

    /// Workflow #390 EXEC-1: whether `operation`'s `when` holds on `scene`, the frame its step
    /// would start on; `true` without a `when`. The evaluation is recorded as every guard
    /// evaluation is, as a `When` evaluation; one that does not conclude ends the run with
    /// `contained_task_recognition_failed`.
    pub(super) fn when_holds<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        operation: &TaskOperation,
        scene: &actingcommand_recognition::Scene,
    ) -> Result<bool, ContainedTaskRunError<R::Error>> {
        let Some(when) = &operation.when else {
            return Ok(true);
        };
        let target = when.target_id();
        let result = self.evaluator.evaluate_target(scene, target);
        runtime
            .record_guard_evaluation(
                Some(target),
                Some(&result),
                ContainedTaskGuardEvaluation::When,
            )
            .map_err(ContainedTaskRunError::Boundary)?;
        let evaluation = result.map_err(|error| {
            let detail = match error.outcome() {
                Some(outcome) => format!(
                    "operation={} when={target} {error} cause={}",
                    operation.id,
                    outcome.code().as_str()
                ),
                None => format!("operation={} when={target} {error}", operation.id),
            };
            ContainedTaskError::with_detail(
                SharedCode::ContainedTaskRecognitionFailed.code(),
                detail,
            )
        })?;
        Ok(when.holds(evaluation.passed))
    }

    /// Workflow #390 EXEC-1: `operation_id`'s guard refused on the frame of `page_label`, the
    /// step's first attempt, and the operation declares `on_guard_refused: next_candidate`. Its
    /// step finishes on that page without an input, and the page's next candidate is taken.
    pub(super) fn pass_over<R: ContainedTaskRuntime>(
        runtime: &mut R,
        machine: &mut RunStateMachine,
        step_index: u32,
        operation_id: &str,
        page_label: &str,
    ) -> Result<(), ContainedTaskRunError<R::Error>> {
        runtime
            .record(ContainedTaskTrace::StepFinished {
                step_index,
                operation_label: operation_id.to_owned(),
                page_label: page_label.to_owned(),
                phase: None,
            })
            .map_err(ContainedTaskRunError::Boundary)?;
        machine
            .operation_passed_over(operation_id, page_label.to_owned())
            .map_err(|_| ContainedTaskError::new(SharedCode::ContainedTaskStateInvalid.code()))?;
        Ok(())
    }
}

/// Workflow #390 EXEC-1: the end of a run whose page `page_label` has no candidate left after
/// `count` were passed over on its frame.
pub(super) fn candidates_exhausted(
    page_label: &str,
    count: usize,
    detail: &str,
) -> ContainedTaskError {
    let code = ExecutionKernelCode::ContainedTaskOperationCandidatesExhausted.code();
    ContainedTaskError::with_detail(code, detail)
        .with_values(Outcome::new(code).page_id(page_label).count(count))
}
