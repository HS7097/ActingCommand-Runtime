// SPDX-License-Identifier: AGPL-3.0-only

//! #412 Lab B-3 S1 (Workflow #412 B-3; design `lab-b3-design.md` §1.1, §2.1, §2.2; register
//! §1.161): the kernel runs one operation a package declares as a run runs it, and returns after
//! that one step. The run is a mode of the ordinary page-graph run, not a copy of its step: the
//! operation's `when`, guard, input recheck, point draw, gesture, `post_delay`, `expect_after` and
//! retries run unchanged. The mode changes six points:
//!
//! 1. **Side effects off.** No post-admission OCR, no OCR fields, no reading relay (as an offline
//!    simulation).
//! 2. **Entry.** The run always waits for a page of the package first: no required entry, no
//!    application entry. A frame of another page than the operation's `from` ends the run before
//!    any input with `contained_task_declared_operation_page_mismatch`.
//! 3. **Candidates.** The operation is the page's only candidate; the run has no target page,
//!    stage, designated cap (it reads no fact) or per-run cap, and one step.
//! 4. **One step.** Once the step ended without ending the run, the run returns how:
//!    [`DeclaredOperationOutcome::Confirmed`], [`DeclaredOperationOutcome::PassedOver`] or
//!    [`DeclaredOperationOutcome::RecoveryRequested`].
//! 5. **No stability termination.** It compares steps of a run.
//! 6. **Runs that would end.** A success that ends the package's run (a keyed end) is
//!    [`DeclaredOperationOutcome::RunCompleted`].
//!
//! A failure is the run's error, as a run's. Admission
//! ([`PreparedContainedTask::declared_operation`]) refuses, before any lease, an id the entry task
//! does not declare and an operation the Lab does not run this way (decision Q2: page-graph clicks
//! only).

use super::{
    ContainedTaskCode, ContainedTaskError, ContainedTaskRunError, ContainedTaskRunOptions,
    ContainedTaskRuntime, DEFAULT_TASK_TIMEOUT_MS, PageObservation, PreparedContainedTask, linear,
};
use crate::codes::ExecutionKernelCode;
use crate::{RunStateMachine, RunTerminal};
use actingcommand_contract::LabOperationProblem;
use actingcommand_contract::codes::SharedCode;

/// What admission knows of a declared operation before any lease: what a Lab operation's
/// prepared record states of it (ledger revision 4 row LR-10, `LabDeclaredOperation`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredOperation {
    /// The stage of a phased route whose operations list it; `None` for a route without stages.
    pub phase_id: Option<String>,
    /// The page it runs from, as declared (`any` or a page anchor).
    pub from_page: String,
    /// The pages it reaches, as a run waits for them: its `to`, else its `expect_after` page;
    /// empty when it declares neither.
    pub destination_pages: Vec<String>,
    /// How many inputs it may make: its retry policy's attempts, 1 without one.
    pub max_attempts: u32,
}

/// How a declared operation's run ended after its one step (design §2.1 points 4 and 6). A run
/// that failed returns its error instead, as a run does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclaredOperationOutcome {
    /// The step completed on its confirmed page `page`.
    Confirmed { page: String },
    /// The step ended the package's run as a success (a keyed end): on `page`, with the run's
    /// scheduling outcome key.
    RunCompleted {
        page: Option<String>,
        outcome_key: Option<String>,
    },
    /// The guard refused on the step's frame before any input and the operation declares
    /// `on_guard_refused: next_candidate`; `refusal` is the guard's
    /// `contained_task_guard_refused`, with the page it refused on.
    PassedOver { refusal: ContainedTaskError },
    /// The operation's failure decision asked for recovery, which this run does not run: the
    /// failure's code and the page it was seen on.
    RecoveryRequested {
        failure_reason: ContainedTaskCode,
        after_page: Option<String>,
    },
}

impl PreparedContainedTask {
    /// #412 Lab B-3 (design §1.1, "Refused by the kernel before any lease"): admits
    /// `operation_id`, an operation of the package's entry task, as one a Lab operation runs as
    /// the kernel runs it, and states it. An id the task does not declare is
    /// `contained_task_declared_operation_unknown`; a `linear_steps` or `recognize_only` package,
    /// an application effect, a select step and an operation whose `expect_after` reads the
    /// identity an earlier select step chose are `contained_task_declared_operation_unsupported`
    /// with their `lab_operation_problem`.
    pub fn declared_operation(
        &self,
        operation_id: &str,
    ) -> Result<DeclaredOperation, ContainedTaskError> {
        let index = self.declared_operation_index(operation_id)?;
        let operation = self
            .program
            .operations
            .get(index)
            .ok_or_else(state_invalid)?;
        let max_attempts = operation
            .retry_policy(
                self.program.defaults,
                self.control.timeout_ms.unwrap_or(DEFAULT_TASK_TIMEOUT_MS),
            )?
            .map_or(1, |policy| policy.max_attempts());
        Ok(DeclaredOperation {
            phase_id: self
                .program
                .phases
                .iter()
                .flatten()
                .find(|phase| phase.operations.contains(&operation.id))
                .map(|phase| phase.id.clone()),
            from_page: operation.from.clone(),
            destination_pages: operation.destination_pages()?,
            max_attempts,
        })
    }

    /// #412 Lab B-3 (design §2.1): runs the declared operation `operation_id` as the kernel runs
    /// it in a run, and returns after that one step. Admission is
    /// [`Self::declared_operation`]'s; the run is the page-graph run in its declared-operation
    /// mode (the module documentation names the six points it changes).
    pub fn run_declared_operation<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        operation_id: &str,
    ) -> Result<DeclaredOperationOutcome, ContainedTaskRunError<R::Error>> {
        let declared = self.declared_operation_index(operation_id)?;
        // Point 1: post-admission OCR, OCR fields and the reading relay are off.
        let options = ContainedTaskRunOptions {
            declared: Some(declared),
            ..ContainedTaskRunOptions::offline_simulation()
        };
        let mut end = None;
        let outcome = self.run_with_reading_keys(runtime, options, None, &mut end)?;
        // Point 6: without a step end, the step was a success that ended the package's run.
        Ok(
            end.unwrap_or_else(|| DeclaredOperationOutcome::RunCompleted {
                page: outcome.final_page,
                outcome_key: outcome.selected_scheduling_outcome,
            }),
        )
    }

    /// The index of the operation `operation_id` among the entry task's operations, when a Lab
    /// operation may run it (design §1.1).
    fn declared_operation_index(&self, operation_id: &str) -> Result<usize, ContainedTaskError> {
        let Some((index, operation)) = self
            .program
            .operations
            .iter()
            .enumerate()
            .find(|(_, operation)| operation.id == operation_id)
        else {
            return Err(ContainedTaskError::valued(
                ExecutionKernelCode::ContainedTaskDeclaredOperationUnknown.code(),
                |outcome| outcome.pack_operation_id(operation_id),
            ));
        };
        let problem = if self.control.execution_mode == linear::LINEAR_STEPS {
            Some(LabOperationProblem::LinearSteps)
        } else if self.control.execution_mode == "recognize_only" {
            Some(LabOperationProblem::RecognizeOnly)
        } else if operation.application.is_some() {
            Some(LabOperationProblem::ApplicationEffect)
        } else if operation
            .expect_after
            .as_ref()
            .is_some_and(|expectation| expectation.expect_identity.is_some())
        {
            Some(LabOperationProblem::ChosenIdentity)
        } else {
            None
        };
        match problem {
            None => Ok(index),
            Some(problem) => Err(ContainedTaskError::valued(
                ExecutionKernelCode::ContainedTaskDeclaredOperationUnsupported.code(),
                |outcome| {
                    outcome
                        .pack_operation_id(operation_id)
                        .lab_operation_problem(problem)
                },
            )),
        }
    }

    /// Point 2: the run's first frame, `observation`, shows the page the declared operation
    /// (index `declared`) runs from; another page ends the run before any input with
    /// `contained_task_declared_operation_page_mismatch` (`pack_operation_id`, the frame's
    /// `page_id`, and `page_ids` the declared `from`).
    pub(super) fn declared_operation_entry(
        &self,
        declared: usize,
        observation: Option<&PageObservation>,
    ) -> Result<(), ContainedTaskError> {
        let operation = self
            .program
            .operations
            .get(declared)
            .ok_or_else(state_invalid)?;
        let page = observation
            .map(|observed| observed.page_label.as_str())
            .ok_or_else(|| ContainedTaskError::new(SharedCode::ContainedTaskPageUnknown.code()))?;
        if crate::page_anchor_matches(&self.control.game, page, &operation.from) {
            return Ok(());
        }
        Err(ContainedTaskError::valued(
            ExecutionKernelCode::ContainedTaskDeclaredOperationPageMismatch.code(),
            |outcome| {
                outcome
                    .pack_operation_id(operation.id.as_str())
                    .page_id(page)
                    .page_ids([operation.from.as_str()])
            },
        ))
    }
}

/// Point 4: how the declared operation's one step ended, from the run's state machine once the
/// step ended without ending the run: the recovery its failure decision asked for, the guard
/// refusal it was passed over on (`refusal`), or the page it completed on. Any other state is
/// `contained_task_state_invalid`.
pub(super) fn step_end(
    machine: &RunStateMachine,
    refusal: Option<ContainedTaskError>,
) -> Result<DeclaredOperationOutcome, ContainedTaskError> {
    match (machine.terminal(), refusal) {
        (Some(RunTerminal::SuccessorSuggested { suggestion }), None) => {
            Ok(DeclaredOperationOutcome::RecoveryRequested {
                failure_reason: suggestion.failure_reason,
                after_page: suggestion.after_page.clone(),
            })
        }
        (None, Some(refusal)) => Ok(DeclaredOperationOutcome::PassedOver { refusal }),
        (None, None) => machine
            .current_page()
            .map(|page| DeclaredOperationOutcome::Confirmed {
                page: page.to_owned(),
            })
            .ok_or_else(state_invalid),
        _ => Err(state_invalid()),
    }
}

fn state_invalid() -> ContainedTaskError {
    ContainedTaskError::new(SharedCode::ContainedTaskStateInvalid.code())
}
