// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #390 EXEC-2 (forced-compromises mapping, rows E-05 and E-06): an operation's
//! observation window, and the recheck of a click on fresh frames before its input
//! (`contracts/resource-declarations.md`, section Task schema `0.9` observation windows and
//! input rechecks). Both are task schema `0.9` declarations of page-graph tasks.
//!
//! - **Observation window.** After the step's destination passed, the screen is watched for at
//!   most 60 s before the step finishes: until one destination held on a number of consecutive
//!   frames (`stable`), or for the whole window, so that a late popup is seen (`window_end`). A
//!   frame that shows no destination is offered to the shared interrupt pages (Workflow #373
//!   NAV-6, model §6.1 point 3), and a handled one extends the window by the handling time. The
//!   post-input delay keeps its 5 s bound.
//! - **Input recheck.** At the place of the one sampling deadline check before the effect intent
//!   (Workflow #386 SR-1), the click's decision is checked again on one fresh frame
//!   (`fresh_frame`), or on fresh frames until it held on a number of consecutive frames
//!   (`stable_frames`); the input is drawn from and bound to the last of them. Each fresh frame
//!   is an observation before the input: its sampling timeouts take SR-1's observe-only retries.

use super::{
    ContainedTaskBoundaryTiming, ContainedTaskError, ContainedTaskGuardOutcome,
    ContainedTaskRunError, ContainedTaskRuntime, ContainedTaskTimingContext, PageObservation,
    PostAdmissionOcrCollector, PostconditionResolution, PreparedContainedTask, SampleRetries,
    TaskControl, TaskOperation, sample_retry,
};
use crate::codes::ExecutionKernelCode;
use crate::{RunFailureStage, RunOperationFailureDecision, RunOperationPolicy, RunStateMachine};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::{
    TaskTimingBoundary, TaskTimingCheckPosition, TaskTimingFailure, TaskTimingScope,
    TaskTimingStage,
};
use actingcommand_recognition_pack::TargetEvaluation;
use serde::Deserialize;
use std::ops::RangeInclusive;
use std::time::{Duration, Instant};

/// The longest observation window an operation may declare (FCM E-05).
const MAX_WINDOW_MS: u64 = 60_000;
/// How many consecutive frames a stability wait may ask for.
const STABLE_FRAMES: RangeInclusive<u32> = 2..=32;
/// The task schema that admits both declarations.
const TASK_SCHEMA: &str = "0.9";

/// An operation's observation window, as the package writes it.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(tag = "until", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum TaskObservationWindow {
    /// Watch the whole window: a late popup that is a destination of the operation ends it.
    WindowEnd { window_ms: u64 },
    /// End once one destination passed on `frames` consecutive frames, the arrival included.
    Stable { window_ms: u64, frames: u32 },
}

impl TaskObservationWindow {
    const fn window_ms(self) -> u64 {
        match self {
            Self::WindowEnd { window_ms } | Self::Stable { window_ms, .. } => window_ms,
        }
    }
}

/// How a click is checked again before its input, as the package writes it.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum TaskInputRecheck {
    /// One fresh frame must show the step's page with its guard passing.
    FreshFrame,
    /// Fresh frames until `frames` consecutive ones, the step's own frame included, showed the
    /// step's page with its guard passing.
    StableFrames { frames: u32 },
}

/// Admission of an operation's observation window and input recheck
/// (`contained_task_operation_invalid`): task schema `0.9`; a window of 1 to 60,000 ms on an
/// operation with a destination; a recheck on a click; a stability wait of 2 to 32 frames whose
/// frames, one interval apart, fit in its window (the operation's confirmation interval in the
/// observation window, the control capture interval within the control step timeout).
pub(super) fn validate_operation(
    operation: &TaskOperation,
    control: &TaskControl,
    schema_version: &str,
) -> Result<(), ContainedTaskError> {
    let invalid = |field: &str| {
        ContainedTaskError::with_detail(
            ExecutionKernelCode::ContainedTaskOperationInvalid.code(),
            format!("operation={} field={field}", operation.id),
        )
    };
    let fits = |frames: u32, interval_ms: u64, window_ms: u64| {
        STABLE_FRAMES.contains(&frames)
            && u64::from(frames - 1).saturating_mul(interval_ms) <= window_ms
    };
    if let Some(window) = operation.observation_window {
        let interval_ms = operation.effective_timing(control).interval.milliseconds;
        let valid = schema_version == TASK_SCHEMA
            && (1..=MAX_WINDOW_MS).contains(&window.window_ms())
            && !operation.destination_pages()?.is_empty()
            && match window {
                TaskObservationWindow::WindowEnd { .. } => true,
                TaskObservationWindow::Stable { window_ms, frames } => {
                    fits(frames, interval_ms, window_ms)
                }
            };
        if !valid {
            return Err(invalid("observation_window"));
        }
    }
    if let Some(recheck) = operation.input_recheck {
        let valid = schema_version == TASK_SCHEMA
            && operation.click.is_some()
            && operation.application.is_none()
            && operation.select.is_none()
            && match recheck {
                TaskInputRecheck::FreshFrame => true,
                TaskInputRecheck::StableFrames { frames } => fits(
                    frames,
                    control.capture_interval().milliseconds,
                    control.step_timeout().milliseconds,
                ),
            };
        if !valid {
            return Err(invalid("input_recheck"));
        }
    }
    Ok(())
}

/// What became of a click's recheck.
pub(super) enum InputRecheck {
    /// The decision held on the fresh frames.
    Held(Box<RecheckedInput>),
    /// The decision did not hold: no input; `error` goes through the operation's failure
    /// decision with the page of the last fresh frame that showed one.
    Refused {
        error: ContainedTaskError,
        after_page: Option<String>,
    },
}

/// A decision that held: the input is drawn from `guard`'s `target` and bound to `observation`,
/// whose observation window is `timing`.
pub(super) struct RecheckedInput {
    pub(super) observation: PageObservation,
    pub(super) timing: ContainedTaskTimingContext,
    pub(super) guard: ContainedTaskGuardOutcome,
    pub(super) target: Option<TargetEvaluation>,
}

impl PreparedContainedTask {
    /// The operation's failure decision on `error`, a refusal before its input seen on a frame of
    /// `after_page`: `Ok(())` once it asked for recovery (the attempt ends there), the refusal
    /// otherwise. Without a retry policy the refusal ends the run.
    pub(super) fn refuse_before_input<E>(
        operation: &TaskOperation,
        retry_policy: Option<&RunOperationPolicy>,
        attempt: u32,
        error: ContainedTaskError,
        after_page: Option<String>,
        machine: &mut RunStateMachine,
    ) -> Result<(), ContainedTaskRunError<E>> {
        let Some(policy) = retry_policy else {
            return Err(error.into());
        };
        match operation.failure_decision(
            policy,
            attempt,
            error.code(),
            after_page,
            RunFailureStage::PreExecutionGuard,
        )? {
            RunOperationFailureDecision::RequestRecovery(trigger) => {
                machine.operation_needs_recovery(trigger).map_err(|_| {
                    ContainedTaskError::new(SharedCode::ContainedTaskStateInvalid.code())
                })?;
                Ok(())
            }
            RunOperationFailureDecision::Fail(_) => Err(error.into()),
            RunOperationFailureDecision::Retry { .. } => {
                Err(ContainedTaskError::new(SharedCode::ContainedTaskStateInvalid.code()).into())
            }
        }
    }

    /// The click's decision on the step's frame (a page of `decision_page`) checked again on
    /// fresh frames, each made in a new window of `window` within the run's `timing`
    /// (`observation`): one fresh frame, or fresh frames one control capture interval apart until
    /// the decision held on the declared number of consecutive frames. A fresh frame with no page
    /// of the package is `contained_task_page_unknown` for `fresh_frame` and a frame that did not
    /// hold for `stable_frames`; a guard that does not pass refuses the input for `fresh_frame`
    /// and is a frame that did not hold for `stable_frames`. A stability wait that does not hold
    /// within `window` of its start refuses the input with
    /// `contained_task_input_stability_unconfirmed`.
    pub(super) fn recheck_before_input<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        operation: &TaskOperation,
        recheck: TaskInputRecheck,
        decision_page: &str,
        observation: (ContainedTaskTimingContext, Duration),
        retries: &mut SampleRetries,
    ) -> Result<InputRecheck, ContainedTaskRunError<R::Error>> {
        let (timing, window) = observation;
        let frames = match recheck {
            TaskInputRecheck::FreshFrame => loop {
                let (fresh, step_timing) =
                    self.observe_before_input(runtime, timing, window, retries)?;
                match operation.guard_outcome(
                    &self.control,
                    &fresh,
                    &self.evaluator,
                    runtime,
                    Some(step_timing),
                ) {
                    Ok((guard, target)) => {
                        return Ok(InputRecheck::Held(Box::new(RecheckedInput {
                            observation: fresh,
                            timing: step_timing,
                            guard,
                            target,
                        })));
                    }
                    Err(failure) if sample_retry::is_timeout(&failure) => {
                        retries.retry(failure, window)?;
                    }
                    Err(ContainedTaskRunError::Task(error)) => {
                        return Ok(InputRecheck::Refused {
                            error,
                            after_page: Some(fresh.page_label),
                        });
                    }
                    Err(failure) => return Err(failure),
                }
            },
            TaskInputRecheck::StableFrames { frames } => frames,
        };
        let task_deadline = timing.deadline();
        let interval = Duration::from_millis(self.control.capture_interval().milliseconds);
        let deadline = runtime.observation_now() + window;
        let mut held = 1_u32;
        let mut after_page = Some(decision_page.to_owned());
        loop {
            let now = runtime.observation_now();
            if now >= deadline {
                return Ok(InputRecheck::Refused {
                    error: ContainedTaskError::valued(
                        ExecutionKernelCode::ContainedTaskInputStabilityUnconfirmed.code(),
                        |outcome| {
                            outcome
                                .pack_operation_id(operation.id.as_str())
                                .timeout_ms(window)
                        },
                    ),
                    after_page,
                });
            }
            let task_left = task_deadline.saturating_duration_since(runtime.monotonic_now());
            pause(
                runtime,
                TaskTimingBoundary::PageRecognitionWait,
                interval
                    .min(deadline.saturating_duration_since(now))
                    .min(task_left),
                timing,
            );
            if runtime.monotonic_now() >= task_deadline {
                return Err(self
                    .task_timeout_error(runtime, TaskTimingStage::BeforeInput, task_deadline, None)
                    .into());
            }
            let (fresh, step_timing) =
                self.observe_fresh_frame(runtime, timing, window, retries)?;
            let Ok(fresh) = fresh else {
                held = 0;
                after_page = None;
                continue;
            };
            match operation.guard_outcome(
                &self.control,
                &fresh,
                &self.evaluator,
                runtime,
                Some(step_timing),
            ) {
                Ok((guard, target)) => {
                    held += 1;
                    if held >= frames {
                        return Ok(InputRecheck::Held(Box::new(RecheckedInput {
                            observation: fresh,
                            timing: step_timing,
                            guard,
                            target,
                        })));
                    }
                    after_page = Some(fresh.page_label);
                }
                Err(failure) if sample_retry::is_timeout(&failure) => {
                    retries.retry(failure, window)?;
                    held = 0;
                }
                Err(ContainedTaskRunError::Task(error))
                    if error.code().as_str()
                        == ExecutionKernelCode::ContainedTaskGuardRefused.as_str() =>
                {
                    held = 0;
                    after_page = Some(fresh.page_label);
                }
                Err(ContainedTaskRunError::Task(error)) => {
                    return Ok(InputRecheck::Refused {
                        error,
                        after_page: Some(fresh.page_label),
                    });
                }
                Err(failure) => return Err(failure),
            }
        }
    }

    /// The operation's observation window after its destination passed on `arrived`. Its frames
    /// are taken one `interval` apart and evaluated as confirmation frames are: an error page
    /// fails the step as during the confirmation; a frame on which no destination passed is
    /// offered to the shared interrupt pages, and a handled one extends the window by the time
    /// the handling took. `window_end`: another destination ends the window at once (the step
    /// arrived there); otherwise the window ends at its end on a frame that shows a destination,
    /// waiting up to `budget` (the step's confirmation budget) after its end for one, after which
    /// the step is unconfirmed. `stable`: the window ends on the frame that makes the declared
    /// number of consecutive frames of one destination; a window that ends first fails the run
    /// with `contained_task_observation_stability_unconfirmed`. A frame whose sampling timed out
    /// is observed again in a new window of `budget` (Workflow #386 SR-1), as `retries` allows.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn observe_window<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        ocr_collector: &mut PostAdmissionOcrCollector<'_>,
        operation: &TaskOperation,
        window: TaskObservationWindow,
        arrived: PageObservation,
        (interval, budget): (Duration, Duration),
        timing: ContainedTaskTimingContext,
        retries: &mut SampleRetries,
    ) -> Result<PostconditionResolution, ContainedTaskRunError<R::Error>> {
        let task_deadline = timing.deadline();
        let consensus = !self.evaluator.pack().target_consensus.is_empty();
        let started = runtime.observation_now();
        let mut end = started + Duration::from_millis(window.window_ms());
        // #411 A15: the page the last frame matched, none when it matched no page.
        let mut last_frame_page = Some(arrived.page_label.clone());
        let mut current = arrived;
        // Whether the latest frame showed a destination (`current`), and on how many consecutive
        // frames `current`'s page passed.
        let mut latest = true;
        let mut held = 1_u32;
        loop {
            let now = runtime.observation_now();
            let limit = match window {
                TaskObservationWindow::Stable { frames, window_ms } => {
                    if held >= frames {
                        return Ok(PostconditionResolution::Reached(current));
                    }
                    if now >= end {
                        return Err(ContainedTaskError::valued(
                            ExecutionKernelCode::ContainedTaskObservationStabilityUnconfirmed
                                .code(),
                            |outcome| {
                                outcome
                                    .pack_operation_id(operation.id.as_str())
                                    .timeout_ms(window_ms)
                            },
                        )
                        .into());
                    }
                    end
                }
                TaskObservationWindow::WindowEnd { .. } => {
                    if now >= end && latest {
                        return Ok(PostconditionResolution::Reached(current));
                    }
                    let limit = end + budget;
                    if now >= limit {
                        let observed_ms = |at: Instant| {
                            u64::try_from(at.saturating_duration_since(started).as_millis())
                                .unwrap_or(u64::MAX)
                        };
                        return Ok(PostconditionResolution::Failed {
                            observation: None,
                            last_frame_page,
                            hit_error_page: false,
                            timing_failure: Some(TaskTimingFailure {
                                scope: TaskTimingScope::Postcondition,
                                stage: TaskTimingStage::Postcondition,
                                elapsed_ms: observed_ms(now),
                                limit_ms: observed_ms(limit),
                                required_delay_ms: None,
                            }),
                        });
                    }
                    limit
                }
            };
            let task_left = task_deadline.saturating_duration_since(runtime.monotonic_now());
            pause(
                runtime,
                TaskTimingBoundary::PostconditionWait,
                interval
                    .min(limit.saturating_duration_since(now))
                    .min(task_left),
                timing,
            );
            if runtime.monotonic_now() >= task_deadline {
                return Err(self
                    .task_timeout_error(
                        runtime,
                        TaskTimingStage::Postcondition,
                        task_deadline,
                        None,
                    )
                    .with_timing_check_position(TaskTimingCheckPosition::PostconditionBeforeCapture)
                    .into());
            }
            let frame_timing = if consensus {
                timing.with_deadline(runtime.observation_now() + budget)
            } else {
                timing
            };
            let observed = match self.capture_page_scene(runtime, ocr_collector, None, frame_timing)
            {
                Ok(observed) => observed,
                Err(failure) => {
                    retries.retry(failure, budget)?;
                    latest = false;
                    held = 0;
                    continue;
                }
            };
            if runtime.monotonic_now() >= task_deadline {
                return Err(self
                    .task_timeout_error(
                        runtime,
                        TaskTimingStage::Postcondition,
                        task_deadline,
                        None,
                    )
                    .with_timing_check_position(TaskTimingCheckPosition::PostconditionAfterCapture)
                    .into());
            }
            let scene = match observed {
                Ok(observation) => {
                    last_frame_page = Some(observation.page_label.clone());
                    let destinations =
                        operation.matching_destination_count(&self.control, &observation)?;
                    let error_page = self
                        .program
                        .is_error_page(&self.control, &observation.page_label);
                    if destinations > 1 || (destinations == 1 && error_page) {
                        return Err(ContainedTaskError::with_detail(
                            ExecutionKernelCode::ContainedTaskRecognitionConflict.code(),
                            observation.page_label,
                        )
                        .into());
                    }
                    if error_page {
                        return Ok(PostconditionResolution::Failed {
                            observation: Some(observation),
                            last_frame_page,
                            hit_error_page: true,
                            timing_failure: None,
                        });
                    }
                    if destinations == 1 {
                        latest = true;
                        if observation.page_label == current.page_label {
                            held += 1;
                        } else if matches!(window, TaskObservationWindow::WindowEnd { .. }) {
                            return Ok(PostconditionResolution::Reached(observation));
                        } else {
                            held = 1;
                        }
                        current = observation;
                        continue;
                    }
                    observation.scene
                }
                Err(scene) => {
                    last_frame_page = None;
                    scene
                }
            };
            latest = false;
            held = 0;
            if let Some(paused) = self.offer_shared_interrupt(runtime, &scene)? {
                end += paused;
            }
        }
    }
}

/// A wait of `duration` between two frames, recorded at `boundary`.
fn pause<R: ContainedTaskRuntime>(
    runtime: &mut R,
    boundary: TaskTimingBoundary,
    duration: Duration,
    timing: ContainedTaskTimingContext,
) {
    let identity = runtime.task_boundary_identity(boundary);
    let started = runtime.monotonic_now();
    runtime.pause(duration);
    let ended = runtime.monotonic_now();
    runtime.observe_task_boundary(ContainedTaskBoundaryTiming {
        boundary,
        identity,
        context: timing,
        started,
        ended,
        succeeded: true,
    });
}
