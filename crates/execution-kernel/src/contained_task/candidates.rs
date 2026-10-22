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
//!
//! #411 B16 part 1 and B22 (B-train plan §2.11), task schema `0.9` page-graph tasks:
//!
//! - **`max_runs` and `on_max_runs`.** An operation may be dispatched at most `max_runs` times
//!   per run (1..=1000; a guard pass-over counts, a `when` withdrawal does not, D10). Once it
//!   was, `on_max_runs: next_candidate` passes it over like a capped member of a designated
//!   effect set, and `on_max_runs: {"outcome_key": K}` ends the run Completed on the current page
//!   with K, written as `task.operation_capped`. Never on a designated operation or a member of
//!   a designated effect set, which have their own cap.
//! - **A keyed `on_guard_refused`.** `{"outcome_key": K, "failed"?: <target>}`: a click whose
//!   guard refuses on its step's frame at attempt 1, before any input, with a definite fail of
//!   the named target (the guard target, or a member of it), ends its step without an input on
//!   the frame's page, and the run ends Completed there with K. Any other refusal keeps the
//!   operation's failure decision.
//! - Both keyed ends follow the one keyed-terminal rule of the static checks
//!   (`actingcommand_contract::keyed_terminal_holds`), and a package that declares one is no
//!   prerequisite or return-home layer.

use super::{
    ContainedTaskError, ContainedTaskGuardEvaluation, ContainedTaskRunError, ContainedTaskRuntime,
    ContainedTaskTrace, PageObservation, PreparedContainedTask, TaskOperation, TaskProgram,
};
use crate::codes::ExecutionKernelCode;
use crate::{OperationRunCap, RunDecisionError, RunDirective, RunMaxRunsEnd, RunStateMachine};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_recognition_pack::{
    CompositeMode, RecognitionEvaluator, RecognitionPackCode, RecognitionTarget, TargetEvaluation,
    TargetKind,
};
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

/// The string `next_candidate` of `on_guard_refused` and `on_max_runs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum NextCandidate {
    NextCandidate,
}

/// What a guard refusal on the step's frame does, as the package writes it.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(super) enum TaskGuardRefusal {
    /// `"next_candidate"`: the step ends without an input and the page's next candidate is
    /// taken on the same frame.
    NextCandidate(NextCandidate),
    /// #411 B22: `{"outcome_key": K, "failed"?: <target>}`: a definite fail of `failed` (the
    /// guard target when absent) ends the step without an input and the run with K.
    Keyed(TaskGuardRefusalEnd),
}

/// #411 B22 (B-train plan §2.11 item 2): the keyed form of `on_guard_refused`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TaskGuardRefusalEnd {
    outcome_key: String,
    /// The guard target or one member of it (the guard target is a composite); absent, the
    /// guard target.
    #[serde(default)]
    failed: Option<String>,
}

/// #411 B16 part 1 (B-train plan §2.11 item 1): what the run does once it dispatched the
/// operation `max_runs` times, as the package writes it.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(super) enum TaskOnMaxRuns {
    /// `"next_candidate"`: the operation is passed over and the page's next operation runs.
    NextCandidate(NextCandidate),
    /// `{"outcome_key": K}`: the run ends Completed on the current page with K.
    Keyed(TaskKeyedEnd),
}

/// #411 B16 part 1: the keyed form of `on_max_runs`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TaskKeyedEnd {
    outcome_key: String,
}

impl TaskOperation {
    /// Workflow #390 EXEC-1: the page's next candidate may run instead of this operation when its
    /// `when` does not hold or its guard refuses (`when`, `on_guard_refused: next_candidate`).
    pub(super) fn may_be_passed_over(&self) -> bool {
        self.when.is_some()
            || matches!(
                self.on_guard_refused,
                Some(TaskGuardRefusal::NextCandidate(_))
            )
    }

    /// #411 B22: the outcome key of a keyed `on_guard_refused`.
    pub(super) fn on_guard_refused_key(&self) -> Option<&str> {
        match &self.on_guard_refused {
            Some(TaskGuardRefusal::Keyed(end)) => Some(end.outcome_key.as_str()),
            Some(TaskGuardRefusal::NextCandidate(_)) | None => None,
        }
    }

    /// #411 B16 part 1: the outcome key of a keyed `on_max_runs`.
    pub(super) fn on_max_runs_key(&self) -> Option<&str> {
        match &self.on_max_runs {
            Some(TaskOnMaxRuns::Keyed(end)) => Some(end.outcome_key.as_str()),
            Some(TaskOnMaxRuns::NextCandidate(_)) | None => None,
        }
    }

    /// #411 B16 part 1: the operation's per-run cap as the static checks see it.
    pub(super) fn phase_run_cap(&self) -> Option<actingcommand_contract::TaskPhaseRunCap<'_>> {
        self.max_runs
            .map(|limit| actingcommand_contract::TaskPhaseRunCap {
                limit,
                outcome_key: self.on_max_runs_key(),
            })
    }

    /// #411 B16/B22 (B-train plan §2.11 item 3): the outcome keys on which a run may end at this
    /// operation's state, on its page, each with the member that declares it: `on_empty`, a
    /// keyed `on_max_runs`, a keyed `on_guard_refused`; #411 B36/B19 (A35-S3): a wait step's
    /// `on_timeout`.
    pub(super) fn keyed_ends(&self) -> impl Iterator<Item = (&'static str, &str)> {
        [
            ("on_empty", self.on_empty_key()),
            ("on_max_runs", self.on_max_runs_key()),
            ("on_guard_refused", self.on_guard_refused_key()),
            ("on_timeout", self.wait_on_timeout_key()),
        ]
        .into_iter()
        .filter_map(|(member, key)| key.map(|key| (member, key)))
    }

    /// #411 B16 part 1: the operation may be capped and passed over to the page's next operation
    /// (`on_max_runs: next_candidate`).
    pub(super) fn may_be_run_capped(&self) -> bool {
        self.max_runs.is_some() && matches!(self.on_max_runs, Some(TaskOnMaxRuns::NextCandidate(_)))
    }

    /// #411 B16 part 1: the per-run cap the run holds the operation to.
    pub(super) fn run_cap(&self) -> Option<OperationRunCap> {
        let max_runs = self.max_runs?;
        let on_max_runs = match self.on_max_runs.as_ref()? {
            TaskOnMaxRuns::NextCandidate(_) => RunMaxRunsEnd::NextCandidate,
            TaskOnMaxRuns::Keyed(end) => RunMaxRunsEnd::OutcomeKey(end.outcome_key.clone()),
        };
        Some(OperationRunCap {
            operation_id: self.id.clone(),
            max_runs,
            on_max_runs,
        })
    }
}

fn invalid(operation: &TaskOperation, field: &str) -> ContainedTaskError {
    ContainedTaskError::with_detail(
        ExecutionKernelCode::ContainedTaskOperationInvalid.code(),
        format!("operation={} field={field}", operation.id),
    )
}

/// A key of a keyed end: not blank.
fn keyed(key: &str) -> bool {
    !key.trim().is_empty()
}

/// Admission of an operation's `when`, `on_guard_refused`, `max_runs` and `on_max_runs`
/// (`contained_task_operation_invalid`): task schema `0.9`; a `when` names a target;
/// `on_guard_refused` is declared on a click with a declared guard, and its keyed form names its
/// key (and a `failed` target, when it declares one); #411 B16 part 1: `max_runs` (1..=1000) and
/// `on_max_runs` are declared together, and a keyed `on_max_runs` names its key.
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
    if let Some(refusal) = &operation.on_guard_refused
        && (schema_version != TASK_SCHEMA
            || operation.click.is_none()
            || operation.guard.is_none()
            || operation.unguarded_trusted_coordinate
            || matches!(refusal, TaskGuardRefusal::Keyed(end)
                if !keyed(&end.outcome_key)
                    || end.failed.as_deref().is_some_and(|failed| !keyed(failed))))
    {
        return Err(invalid(operation, "on_guard_refused"));
    }
    if operation.max_runs.is_some_and(|max_runs| {
        schema_version != TASK_SCHEMA
            || !(1..=actingcommand_contract::MAX_TASK_OPERATION_RUNS).contains(&max_runs)
            || operation.on_max_runs.is_none()
    }) {
        return Err(invalid(operation, "max_runs"));
    }
    if let Some(on_max_runs) = &operation.on_max_runs
        && (operation.max_runs.is_none()
            || matches!(on_max_runs, TaskOnMaxRuns::Keyed(end) if !keyed(&end.outcome_key)))
    {
        return Err(invalid(operation, "on_max_runs"));
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

    /// #411 B16 part 1 and B22 (B-train plan §2.11 items 1 and 2): a keyed `on_max_runs` or
    /// `on_guard_refused` ends the run with its key, so the task declares a `scheduling_outcome`;
    /// a keyed `on_guard_refused` names as `failed` its guard target or one member of that target
    /// (a composite of the recognition pack). Each refusal is `contained_task_operation_invalid`
    /// at the member.
    pub(super) fn validate_keyed_ends(
        &self,
        evaluator: &RecognitionEvaluator,
    ) -> Result<(), ContainedTaskError> {
        for operation in &self.operations {
            if operation.on_max_runs_key().is_some() && self.scheduling_outcome.is_none() {
                return Err(invalid(operation, "on_max_runs"));
            }
            let Some(TaskGuardRefusal::Keyed(end)) = &operation.on_guard_refused else {
                continue;
            };
            let named = match (&operation.guard, end.failed.as_deref()) {
                (Some(_), None) => true,
                (Some(guard), Some(failed)) => {
                    names_guard_member(evaluator, &guard.target_id, failed)
                }
                (None, _) => false,
            };
            if self.scheduling_outcome.is_none() || !named {
                return Err(invalid(operation, "on_guard_refused"));
            }
        }
        Ok(())
    }
}

/// #411 B22: `failed` is the guard target itself or one member of it, a composite of the pack
/// (composites are not nested).
fn names_guard_member(evaluator: &RecognitionEvaluator, guard_target: &str, failed: &str) -> bool {
    failed == guard_target
        || evaluator.pack().targets.iter().any(|target| {
            matches!(target, RecognitionTarget::Composite(composite)
                if composite.id == guard_target
                    && composite.members.iter().any(|member| member == failed))
        })
}

/// #411 B22 (B-train plan §2.11 item 2): the key a guard refusal on the step's frame ends the
/// run with: the operation declares a keyed `on_guard_refused`, and `evaluation`, the guard
/// target's evaluation that refused, shows a definite fail of the named target (`failed`, else
/// the guard target). The failing member travels typed, in the evaluation, never parsed from a
/// developer detail.
pub(super) fn guard_refusal_key<'o>(
    operation: &'o TaskOperation,
    evaluation: &TargetEvaluation,
) -> Option<&'o str> {
    let Some(TaskGuardRefusal::Keyed(end)) = &operation.on_guard_refused else {
        return None;
    };
    let named = match end.failed.as_deref() {
        None => Some(evaluation),
        Some(failed) => evaluation_of(evaluation, failed),
    }?;
    definitely_failed(named).then_some(end.outcome_key.as_str())
}

/// The evaluation of `target_id` within the guard target's `evaluation`: the guard target's own,
/// or one member's of a composite.
fn evaluation_of<'e>(
    evaluation: &'e TargetEvaluation,
    target_id: &str,
) -> Option<&'e TargetEvaluation> {
    if evaluation.id == target_id {
        return Some(evaluation);
    }
    evaluation
        .composite
        .as_deref()?
        .members
        .iter()
        .find(|member| member.target_id == target_id)
        .map(|member| &member.evaluation)
}

/// #411 B22: a definite fail ("read, and not met"): the target was evaluated and did not pass,
/// either as a value test whose comparison does not hold (`value_test_not_met`) or without a
/// registered reason (a plain miss). A registered reason other than `value_test_not_met`
/// (`value_test_unresolved`, an unresolved OCR region, a match that is not unique, …) is no
/// definite fail. A composite fails definitely when its members' definite fails decide it: one
/// of `all_of`, every one of `any_of`.
fn definitely_failed(evaluation: &TargetEvaluation) -> bool {
    if evaluation.passed {
        return false;
    }
    if let Some(failure) = &evaluation.failure {
        return failure.is(RecognitionPackCode::ValueTestNotMet);
    }
    match evaluation.composite.as_deref() {
        None => true,
        Some(composite) => match composite.mode {
            CompositeMode::AllOf => composite
                .members
                .iter()
                .any(|member| definitely_failed(&member.evaluation)),
            CompositeMode::AnyOf => composite
                .members
                .iter()
                .all(|member| definitely_failed(&member.evaluation)),
        },
    }
}

impl PreparedContainedTask {
    /// Workflow #390 EXEC-1: judges `decision`, the execution-owned transition's next directive
    /// (`RunStateMachine::next_directive`) on the frame of `observation`. An operation it
    /// dispatches whose `when` does not hold on that frame is withdrawn before its step starts
    /// (`None`: the caller asks the transition again, and the page's next candidate is
    /// dispatched). A page whose candidates were all passed over on the frame ends the run with
    /// `contained_task_operation_candidates_exhausted` (`page_id`, and `count` the candidates
    /// passed over). #411 A45: a page the package declares as an error page, from which no
    /// operation leaves, ends the run with `contained_task_error_page_reached`, before any input
    /// from it, as a step that reached it does. Any other refused decision is
    /// `contained_task_state_invalid`, as before.
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
            Err(error)
                if error.code().as_str() == ExecutionKernelCode::RunDecisionInvalid.as_str()
                    && machine
                        .current_page()
                        .is_some_and(|page| self.program.is_error_page(&self.control, page)) =>
            {
                return Err(ContainedTaskError::with_detail(
                    ExecutionKernelCode::ContainedTaskErrorPageReached.code(),
                    format!(
                        "after_page={} hit_error_page=true",
                        machine.current_page().unwrap_or_default()
                    ),
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
