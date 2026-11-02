// SPDX-License-Identifier: AGPL-3.0-only

//! ACT-4 (Workflow #407, register §1.254; `contracts/resource-declarations.md`, "Final-state
//! targets"): an operation's `expect_after` may list targets that must pass (`target_present`)
//! and targets that must definitely not pass (`target_gone`) on the frame its step finishes on.
//! Both are task schema `0.9` members of page-graph tasks.
//!
//! - **One judgement.** A frame after the input is the step's final state when its page is one of
//!   the step's destinations, every `target_present` passes on it and every `target_gone` has a
//!   definite non-match: evaluated, not passed, and no registered reason that leaves it open. A
//!   target whose evaluation failed, whose region the frame cannot give or whose reason leaves it
//!   open is neither present nor gone. The settled path, the confirmation and the observation
//!   window ask this one judgement ([`PreparedContainedTask::final_state`]); a frame of a
//!   destination page that is not the final state is, to each of them, a frame of no destination.
//! - **Flip evidence.** A step whose destinations name the page it left may take the settled path
//!   only with listed targets, and goes at once only on a final state that turned one of them: a
//!   `target_present` that definitely did not pass on the frame that decided the input and passes
//!   now, or a `target_gone` that passed there and definitely does not now ([`FlipEvidence`]). A
//!   final state already true on the decision frame is not this input's effect: the step keeps its
//!   `post_delay_ms` and its confirmation, as before. The decision frame and the frame after the
//!   input are two captures; where the runtime gives frame ids, the two ids differ.
//! - **Admission** (`contained_task_program_invalid` with its `program_problem`, the task path and
//!   the list entry's pointer, [`TaskProgram::validate_expected_targets`]): the two lists share no
//!   target and list none twice (`expect_targets_overlap`); each entry names a target of the
//!   package's recognition pack (`expect_target_unknown`) that is judged on one frame, a template
//!   or colour target outside target consensus (`expect_target_kind`), and that stands in the
//!   rules of the operation's `from` page or of one of its destinations, directly or as a member of
//!   a check there (`expect_target_unreachable`). An older task schema refuses the lists with
//!   `contained_task_operation_invalid`, as its declaration gate does with `UnconsumedField`; a
//!   `linear_steps` task refuses them with `contained_task_linear_invalid`, `reason=expect_targets`.
//! - **Records.** Each evaluation is recorded with the guard phase `expect_after` on the frame it
//!   judges (the decision frame's before any frame after the input is taken, a later frame's right
//!   after its capture), and its time as an evaluation of that frame
//!   ([`ContainedTaskRuntime::record_expected_target`]). Each listed target is evaluated on its own:
//!   a template is matched again on every frame it is judged on, since the page recognition's
//!   template results live in that recognition's own scene context; a colour target is computed.

use super::{
    ContainedTaskError, ContainedTaskEvaluationTiming, ContainedTaskRunError, ContainedTaskRuntime,
    ContainedTaskTimingContext, PageObservation, PreparedContainedTask, Scene, TaskControl,
    TaskOperation, TaskProgram, candidates, linear, observe_instant_span, program_refusal,
};
use crate::codes::{ExecutionKernelCode, ProgramProblem};
use actingcommand_contract::{FrameId, TaskSchema, TaskTimingResult};
use actingcommand_page_detector::PageDetector;
use actingcommand_recognition_pack::{
    RecognitionEvaluator, RecognitionPackResult, RecognitionTarget, TargetEvaluation, TargetKind,
};
use std::collections::BTreeSet;

/// The first task schema that admits the lists.
const TASK_SCHEMA: TaskSchema = TaskSchema::V0_9;

/// Which list a target stands in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Listed {
    Present,
    Gone,
}

impl Listed {
    /// The member the list is.
    const fn member(self) -> &'static str {
        match self {
            Self::Present => "target_present",
            Self::Gone => "target_gone",
        }
    }

    /// Whether a target of this list in `state` holds in the final state.
    pub(super) const fn holds(self, state: TargetState) -> bool {
        matches!(
            (self, state),
            (Self::Present, TargetState::Matched) | (Self::Gone, TargetState::Unmatched)
        )
    }
}

/// A listed target's result on one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TargetState {
    /// It passed.
    Matched,
    /// A definite non-match: evaluated, not passed, and no registered reason that leaves it open.
    Unmatched,
    /// Neither: not evaluated, its evaluation failed (the frame could not give its region, for
    /// one), or its reason leaves it open.
    Unknown,
}

/// The state of a listed target from its evaluation `result`. A definite non-match carries the
/// evidence of its evaluation: every evaluation of a template or colour target does, and a target
/// a consumer did not evaluate carries none (`TargetEvaluation::not_evaluated`).
pub(super) fn target_state(result: &RecognitionPackResult<TargetEvaluation>) -> TargetState {
    match result {
        Ok(evaluation) if evaluation.passed => TargetState::Matched,
        Ok(evaluation)
            if (evaluation.template.is_some() || evaluation.color.is_some())
                && candidates::definitely_failed(evaluation) =>
        {
            TargetState::Unmatched
        }
        Ok(_) | Err(_) => TargetState::Unknown,
    }
}

/// The one final-state judgement of a frame after an operation's input.
#[derive(Debug)]
pub(super) struct FinalState {
    /// How many of the operation's destinations the frame's page names; more than one, or one on
    /// an error page, is the caller's recognition conflict.
    pub(super) destinations: usize,
    /// Whether the frame is the step's final state.
    pub(super) reached: bool,
    /// Each listed target's state, `target_present` then `target_gone`, in declared order; none
    /// when the frame's page is no single destination or an error page (nothing is evaluated).
    targets: Vec<(Listed, TargetState)>,
}

/// The decision frame of a step whose destinations name the page it left: its frame id, where the
/// runtime gave one, and each listed target's state on it.
#[derive(Debug)]
pub(super) struct FlipEvidence {
    frame: Option<FrameId>,
    before: Vec<(Listed, TargetState)>,
}

impl FlipEvidence {
    /// Whether the decision frame already shows every listed target as the final state wants it:
    /// a state true before the input proves nothing about it.
    fn already_final(&self) -> bool {
        self.before
            .iter()
            .all(|(listed, state)| listed.holds(*state))
    }

    /// Whether `after`, the judgement of a frame after the input whose id is `frame`, is a final
    /// state that turned a listed target: a `target_present` from a definite non-match to a match,
    /// or a `target_gone` from a match to a definite non-match. The same frame never turns one.
    pub(super) fn turned(&self, frame: Option<FrameId>, after: &FinalState) -> bool {
        if !after.reached || (frame.is_some() && frame == self.frame) {
            return false;
        }
        self.before
            .iter()
            .zip(&after.targets)
            .any(|((listed, before), (_, now))| match listed {
                Listed::Present => {
                    *before == TargetState::Unmatched && *now == TargetState::Matched
                }
                Listed::Gone => *before == TargetState::Matched && *now == TargetState::Unmatched,
            })
    }
}

/// The frame id of `observation`, where the runtime committed it as an input frame.
pub(super) fn frame_id(observation: &PageObservation) -> Option<FrameId> {
    observation
        .input_context
        .as_ref()
        .map(|context| context.reference().frame_id)
}

impl TaskOperation {
    /// ACT-4: the listed targets, `target_present` then `target_gone`, each in declared order with
    /// its list and its index there.
    fn expected_targets(&self) -> impl Iterator<Item = (Listed, usize, &str)> {
        let lists = self.expect_after.as_ref().map(|expectation| {
            let present = expectation
                .target_present
                .iter()
                .enumerate()
                .map(|(index, target)| (Listed::Present, index, target.as_str()));
            let gone = expectation
                .target_gone
                .iter()
                .enumerate()
                .map(|(index, target)| (Listed::Gone, index, target.as_str()));
            present.chain(gone)
        });
        lists.into_iter().flatten()
    }

    /// ACT-4: whether the operation lists a target in `target_present` or `target_gone`.
    pub(super) fn declares_expected_targets(&self) -> bool {
        self.expected_targets().next().is_some()
    }
}

impl TaskProgram {
    /// ACT-4: the admission of every operation's `target_present` and `target_gone` (the module
    /// header): each refusal is `contained_task_program_invalid` with its `program_problem`, the
    /// task declaration's path `path` and the list entry's pointer; an older task schema refuses
    /// the lists with `contained_task_operation_invalid`.
    pub(super) fn validate_expected_targets(
        &self,
        control: &TaskControl,
        detector: &PageDetector,
        evaluator: &RecognitionEvaluator,
        path: &str,
    ) -> Result<(), ContainedTaskError> {
        for (index, operation) in self.operations.iter().enumerate() {
            if !operation.declares_expected_targets() {
                continue;
            }
            if !self.schema_at_least(TASK_SCHEMA) {
                return Err(ContainedTaskError::with_detail(
                    ExecutionKernelCode::ContainedTaskOperationInvalid.code(),
                    format!("operation={} field=expect_after", operation.id),
                ));
            }
            let reachable = reachable_targets(control, detector, evaluator, operation)?;
            let mut seen = BTreeSet::new();
            for (listed, entry, target) in operation.expected_targets() {
                let refuse = |problem: ProgramProblem| {
                    program_refusal(
                        problem,
                        path,
                        &format!(
                            "/operations/{index}/expect_after/{}/{entry}",
                            listed.member()
                        ),
                    )
                };
                if !seen.insert(target) {
                    return Err(refuse(ProgramProblem::ExpectTargetsOverlap));
                }
                let Ok(kind) = evaluator.target_kind(target) else {
                    return Err(refuse(ProgramProblem::ExpectTargetUnknown));
                };
                if !matches!(kind, TargetKind::Template | TargetKind::Color)
                    || evaluator.pack().target_consensus.contains_key(target)
                {
                    return Err(refuse(ProgramProblem::ExpectTargetKind));
                }
                if !reachable.contains(target) {
                    return Err(refuse(ProgramProblem::ExpectTargetUnreachable));
                }
            }
        }
        Ok(())
    }
}

/// The targets that stand in the rules of `operation`'s `from` page or of one of its destinations
/// (`required`, `any_of`, `optional`, `forbidden`), directly or as a member of a check standing
/// there. `from` `any` names no page of its own.
fn reachable_targets(
    control: &TaskControl,
    detector: &PageDetector,
    evaluator: &RecognitionEvaluator,
    operation: &TaskOperation,
) -> Result<BTreeSet<String>, ContainedTaskError> {
    let game = control.game.as_str();
    let mut anchors = operation.destination_pages()?;
    if operation.from != linear::ANY_PAGE {
        anchors.push(operation.from.clone());
    }
    let mut targets = BTreeSet::new();
    for page in detector.page_definitions() {
        if !anchors.iter().any(|anchor| {
            anchor != linear::ANY_PAGE && crate::page_anchor_matches(game, &page.id, anchor)
        }) {
            continue;
        }
        let rules = page
            .required
            .iter()
            .chain(page.any_of.iter().flatten())
            .chain(&page.optional)
            .chain(&page.forbidden);
        for rule in rules {
            targets.insert(rule.clone());
            // A check is a composite of the pack; composites are not nested.
            for target in &evaluator.pack().targets {
                if let RecognitionTarget::Composite(check) = target
                    && check.id == *rule
                {
                    targets.extend(check.members.iter().cloned());
                }
            }
        }
    }
    Ok(targets)
}

impl PreparedContainedTask {
    /// ACT-4: the one final-state judgement of `observation`, a frame after `operation`'s input,
    /// within the run's `timing`: its page is exactly one of the operation's destinations and no
    /// error page, every `target_present` passes and every `target_gone` has a definite non-match.
    /// A frame of no single destination, or of an error page, evaluates no target.
    pub(super) fn final_state<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        operation: &TaskOperation,
        observation: &PageObservation,
        timing: ContainedTaskTimingContext,
    ) -> Result<FinalState, ContainedTaskRunError<R::Error>> {
        let destinations = operation.matching_destination_count(&self.control, observation)?;
        if destinations != 1
            || self
                .program
                .is_error_page(&self.control, &observation.page_label)
        {
            return Ok(FinalState {
                destinations,
                reached: false,
                targets: Vec::new(),
            });
        }
        let targets =
            self.judge_expected_targets(runtime, operation, &observation.scene, timing)?;
        let reached = targets.iter().all(|(listed, state)| listed.holds(*state));
        Ok(FinalState {
            destinations,
            reached,
            targets,
        })
    }

    /// ACT-4: the flip evidence of a step whose destinations name the page it left, on `decision`,
    /// the frame that decided its input; `None` when that frame already shows the final state's
    /// targets, which the input then did not bring about.
    pub(super) fn flip_evidence<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        operation: &TaskOperation,
        decision: &PageObservation,
        timing: ContainedTaskTimingContext,
    ) -> Result<Option<FlipEvidence>, ContainedTaskRunError<R::Error>> {
        let before = self.judge_expected_targets(runtime, operation, &decision.scene, timing)?;
        let evidence = FlipEvidence {
            frame: frame_id(decision),
            before,
        };
        Ok((!evidence.already_final()).then_some(evidence))
    }

    /// Each listed target of `operation` evaluated on `scene`, each evaluation recorded with its
    /// time.
    fn judge_expected_targets<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        operation: &TaskOperation,
        scene: &Scene,
        timing: ContainedTaskTimingContext,
    ) -> Result<Vec<(Listed, TargetState)>, ContainedTaskRunError<R::Error>> {
        let mut states = Vec::new();
        for (listed, _, target) in operation.expected_targets() {
            let started = runtime.monotonic_now();
            let result = self.evaluator.evaluate_target(scene, target);
            let ended = runtime.monotonic_now();
            let measured = ContainedTaskEvaluationTiming {
                elapsed_us: observe_instant_span(started, ended),
                budget_before: timing.budget_at(started),
                result: if result.is_ok() {
                    TaskTimingResult::Ok
                } else {
                    TaskTimingResult::Err
                },
            };
            runtime
                .record_expected_target(target, &result, measured)
                .map_err(ContainedTaskRunError::Boundary)?;
            states.push((listed, target_state(&result)));
        }
        Ok(states)
    }
}
