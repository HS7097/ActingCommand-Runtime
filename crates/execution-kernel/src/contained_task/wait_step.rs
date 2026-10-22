// SPDX-License-Identifier: AGPL-3.0-only

//! #411 B36 + B19 (slice A35-S3; `contracts/resource-declarations.md`, "Wait steps and wait
//! pages"; coordinator decisions Q9 to Q11 of 2026-10-21): an operation of a task schema `0.9`
//! page-graph task may be a wait step, and a step's confirmation may declare a wait page. One
//! kernel wait, the confirmation's (`await_postcondition`), serves both.
//!
//! - **`wait: {"max_ms", "on_timeout"?}`**, the fourth effect beside `click`, `application` and
//!   `select`. The step makes no input: no guard, no `EffectIntent`, no input, no
//!   `EffectCompleted`. It starts on its `from` page, its wait page, and waits for one of its
//!   destinations (`to` or `expect_after`), a frame every `expect_after.interval_ms`, else every
//!   capture interval (Q10: no interval of its own). A frame on `from` keeps it waiting, up to
//!   `max_ms` from the step's start on the run's budget clock (Q9), and restarts the away clock
//!   from zero. A frame of neither (another page, or none) is offered to the shared interrupt
//!   pages first, a handled one pausing both clocks; otherwise it runs the away clock
//!   (`expect_after.timeout_ms`, else the step timeout). An error page fails the run as a
//!   confirmation does (`contained_task_error_page_reached`); the away clock running out fails
//!   it `contained_task_step_unconfirmed`. `max_ms` reached on `from` ends the run there with
//!   `on_timeout`, the key of a `busy` mapping (Q11), as a keyed end does, or else fails
//!   `contained_task_step_unconfirmed` with `page_id` the wait page.
//! - **`expect_after.wait_page: {"page_id", "max_ms"}`.** Frames on that page pause the
//!   confirmation clock, as a handled interrupt does, up to `max_ms` of time on it in all; beyond
//!   that they count as frames of no destination. Everything else of the confirmation is
//!   unchanged.
//! - A frame on the page a wait holds on is a page the wait accepts: the #411 A43b rule (a still
//!   and unrecognised screen is counted unpaced) does not hold across it.
//! - **Admission.** A broken rule is `contained_task_program_invalid` with its rule
//!   (`program_problem`), the task declaration's `path` and the refused member (`json_pointer`),
//!   checked before the common operation checks; a `linear_steps` task refuses both with
//!   `contained_task_linear_invalid`, `reason=wait`. The `on_timeout` end is a keyed end of the
//!   static checks (`TaskOperation::keyed_ends`).

use super::{
    ContainedTaskError, ContainedTaskRunError, ContainedTaskRuntime, ContainedTaskTimingContext,
    ERROR_PAGE_REACHED, PageObservation, PostAdmissionOcrCollector, PostconditionResolution,
    PreparedContainedTask, STEP_UNCONFIRMED, StepPacing, TaskControl, TaskOperation, TaskProgram,
    confirmation_failure_detail, linear, program_refusal, resolve_page_reference,
};
use crate::codes::ProgramProblem;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    SchedulingOutcomeDeclaration, TaskTimingFailure, TaskTimingScope, TaskTimingStage,
};
use actingcommand_page_detector::PageDetector;
use serde::Deserialize;
use std::time::{Duration, Instant};

/// The most a wait step or a wait page waits: the bound of `expect_after.timeout_ms`.
const MAX_WAIT_MS: u64 = actingcommand_contract::MAX_POSTCONDITION_TIMEOUT_MS;

/// A wait step's `wait`, as the package writes it.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TaskWait {
    max_ms: u64,
    #[serde(default)]
    on_timeout: Option<String>,
}

/// A confirmation's `expect_after.wait_page`, as the package writes it.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TaskWaitPage {
    page_id: String,
    max_ms: u64,
}

impl TaskOperation {
    /// The outcome key of a wait step's `on_timeout`, when it declares one.
    pub(super) fn wait_on_timeout_key(&self) -> Option<&str> {
        self.wait
            .as_ref()
            .and_then(|wait| wait.on_timeout.as_deref())
    }

    /// The wait page of the operation's confirmation, when it declares one.
    pub(super) fn wait_page(&self) -> Option<&TaskWaitPage> {
        self.expect_after
            .as_ref()
            .and_then(|expectation| expectation.wait_page.as_ref())
    }

    /// Whether the operation is a wait step or its confirmation declares a wait page, which a
    /// `linear_steps` task refuses.
    pub(super) fn declares_wait(&self) -> bool {
        self.wait.is_some() || self.wait_page().is_some()
    }

    /// The first member, as its JSON pointer below the operation, that a wait step never takes.
    fn refused_wait_member(&self) -> Option<&'static str> {
        [
            ("click", self.click.is_some()),
            ("application", self.application.is_some()),
            ("select", self.select.is_some()),
            ("guard", self.guard.is_some()),
            (
                "unguarded_trusted_coordinate",
                self.unguarded_trusted_coordinate,
            ),
            ("retryable", self.retryable.is_some()),
            ("max_attempts", self.max_attempts.is_some()),
            ("retry_interval_ms", self.retry_interval_ms.is_some()),
            ("post_delay_ms", self.post_delay_ms.is_some()),
            ("on_error", self.on_error.is_some()),
            ("input_recheck", self.input_recheck.is_some()),
            ("observation_window", self.observation_window.is_some()),
            ("on_guard_refused", self.on_guard_refused.is_some()),
            ("expect_after/wait_page", self.wait_page().is_some()),
        ]
        .into_iter()
        .find_map(|(member, declared)| declared.then_some(member))
    }
}

impl TaskProgram {
    /// The page-graph admission of every wait step and wait page, before the common operation
    /// checks. Each refusal is `contained_task_program_invalid` naming its rule, the task
    /// declaration's path `path` and the refused member:
    ///
    /// - a wait step takes no input, guard, retry, delay, observation window, input recheck,
    ///   guard refusal or wait page of its own (`wait_member_refused`);
    /// - its `from` is one page of the package, not `any`, no target page and no error page
    ///   (`wait_from_invalid`);
    /// - it declares a destination (`wait_destination_missing`);
    /// - its `max_ms` is 1..=1800000 (`wait_max_invalid`);
    /// - it is no designated operation and no member of a designated effect set
    ///   (`wait_designated`);
    /// - its `on_timeout` names a mapping that lists its `from` page (`wait_timeout_unmapped`) and
    ///   declares `busy` (`wait_timeout_not_busy`);
    /// - a wait page is one page of the package, no destination of its operation and no error
    ///   page (`wait_page_invalid`), and its `max_ms` is 1..=1800000 (`wait_max_invalid`).
    pub(super) fn validate_wait_steps(
        &self,
        control: &TaskControl,
        detector: &PageDetector,
        path: &str,
    ) -> Result<(), ContainedTaskError> {
        let game = control.game.as_str();
        let target_pages = self.target_pages()?;
        let designated = self
            .scheduling_outcome
            .as_ref()
            .map(SchedulingOutcomeDeclaration::designated_operations)
            .unwrap_or_default();
        for (index, operation) in self.operations.iter().enumerate() {
            let refuse = |problem: ProgramProblem, member: &str| {
                program_refusal(problem, path, &format!("/operations/{index}/{member}"))
            };
            if let Some(wait) = &operation.wait {
                if let Some(member) = operation.refused_wait_member() {
                    return Err(refuse(ProgramProblem::WaitMemberRefused, member));
                }
                let resolved = if operation.from == linear::ANY_PAGE {
                    None
                } else {
                    resolve_page_reference(game, &operation.from, detector).ok()
                };
                let from = resolved
                    .filter(|from| {
                        !target_pages
                            .iter()
                            .any(|target| crate::page_anchor_matches(game, from, target))
                            && !self.is_error_page(control, from)
                    })
                    .ok_or_else(|| refuse(ProgramProblem::WaitFromInvalid, "from"))?;
                if operation.destination_pages()?.is_empty() {
                    return Err(refuse(ProgramProblem::WaitDestinationMissing, "to"));
                }
                if !(1..=MAX_WAIT_MS).contains(&wait.max_ms) {
                    return Err(refuse(ProgramProblem::WaitMaxInvalid, "wait/max_ms"));
                }
                if designated.contains(&operation.id.as_str()) {
                    return Err(refuse(ProgramProblem::WaitDesignated, "wait"));
                }
                if let Some(key) = &wait.on_timeout {
                    let mapping = self.scheduling_outcome.as_ref().and_then(|scheduling| {
                        scheduling.mappings().iter().find(|mapping| {
                            mapping.outcome_key() == key
                                && mapping
                                    .terminal_pages()
                                    .iter()
                                    .any(|page| crate::page_anchor_matches(game, &from, page))
                        })
                    });
                    match mapping {
                        None => {
                            return Err(refuse(
                                ProgramProblem::WaitTimeoutUnmapped,
                                "wait/on_timeout",
                            ));
                        }
                        Some(mapping) if !mapping.busy() => {
                            return Err(refuse(
                                ProgramProblem::WaitTimeoutNotBusy,
                                "wait/on_timeout",
                            ));
                        }
                        Some(_) => {}
                    }
                }
            }
            if let Some(wait_page) = operation.wait_page() {
                let destinations = operation.destination_pages()?;
                resolve_page_reference(game, &wait_page.page_id, detector)
                    .ok()
                    .filter(|page| {
                        !destinations
                            .iter()
                            .any(|destination| crate::page_anchor_matches(game, page, destination))
                            && !self.is_error_page(control, page)
                    })
                    .ok_or_else(|| {
                        refuse(
                            ProgramProblem::WaitPageInvalid,
                            "expect_after/wait_page/page_id",
                        )
                    })?;
                if !(1..=MAX_WAIT_MS).contains(&wait_page.max_ms) {
                    return Err(refuse(
                        ProgramProblem::WaitMaxInvalid,
                        "expect_after/wait_page/max_ms",
                    ));
                }
            }
        }
        Ok(())
    }
}

/// The page one confirmation wait holds on, when its operation declares one, and the clock that
/// page keeps.
pub(super) struct HeldPage<'o> {
    /// The page as the package declares it.
    page: &'o str,
    /// `max_ms`.
    max: Duration,
    hold: Hold,
}

enum Hold {
    /// A wait step's own page: `max_ms` runs from the budget instant `since`, the step's start,
    /// moved on by every handled shared interrupt.
    Step { since: Instant },
    /// A confirmation's wait page: the budget time the confirmation clock was paused on it.
    Confirmation { spent: Duration },
}

/// What one frame of a confirmation wait does with the page the wait holds on.
pub(super) enum HeldFrame {
    /// The frame is not on the held page, or the wait page's `max_ms` is spent: it counts as a
    /// frame of no destination, as before.
    Away,
    /// A wait step's page shows after its `max_ms`: the wait ends there. `timing` is the step's
    /// wait and its `max_ms`.
    Ended(TaskTimingFailure),
    /// A wait step's page shows within its `max_ms`: the away clock restarts from zero.
    Restart,
    /// The wait page shows: the confirmation clock pauses for this budget time.
    Pause(Duration),
}

impl<'o> HeldPage<'o> {
    /// The page `operation`'s confirmation wait holds on, the wait starting at the budget
    /// instant `started`: a wait step's `from`, or the confirmation's wait page.
    pub(super) fn of(operation: &'o TaskOperation, started: Instant) -> Option<Self> {
        if let Some(wait) = &operation.wait {
            return Some(Self {
                page: &operation.from,
                max: Duration::from_millis(wait.max_ms),
                hold: Hold::Step { since: started },
            });
        }
        operation.wait_page().map(|wait_page| Self {
            page: &wait_page.page_id,
            max: Duration::from_millis(wait_page.max_ms),
            hold: Hold::Confirmation {
                spent: Duration::ZERO,
            },
        })
    }

    /// A frame of the wait whose page is `page_label`, neither a destination nor an error page,
    /// at the budget instant `now`; `previous` is the budget instant of the wait's frame before
    /// it (its start for the first frame). A frame on the wait page pauses the confirmation clock
    /// for the time since that frame, until `max_ms` is spent.
    pub(super) fn frame(
        &mut self,
        game: &str,
        page_label: &str,
        now: Instant,
        previous: Instant,
    ) -> HeldFrame {
        if !crate::page_anchor_matches(game, page_label, self.page) {
            return HeldFrame::Away;
        }
        match &mut self.hold {
            Hold::Step { since } => {
                let waited = now.saturating_duration_since(*since);
                if waited < self.max.saturating_add(Duration::MAX) {
                    return HeldFrame::Restart;
                }
                HeldFrame::Ended(TaskTimingFailure {
                    scope: TaskTimingScope::Postcondition,
                    stage: TaskTimingStage::Postcondition,
                    elapsed_ms: waited.as_millis() as u64,
                    limit_ms: self.max.as_millis() as u64,
                    required_delay_ms: None,
                })
            }
            Hold::Confirmation { spent } => {
                let left = self.max.saturating_sub(*spent);
                if left.is_zero() {
                    return HeldFrame::Away;
                }
                let span = now.saturating_duration_since(previous).min(left);
                *spent += span;
                HeldFrame::Pause(span)
            }
        }
    }

    /// A shared interrupt page was handled for `paused` of budget time: a wait step's `max_ms`
    /// clock pauses as well.
    pub(super) fn interrupted(&mut self, paused: Duration) {
        if let Hold::Step { since } = &mut self.hold {
            *since += paused;
        }
    }
}

/// How a wait step's wait ended, when the run goes on or ends Completed.
pub(super) enum WaitStepEnd {
    /// A destination showed: the step finishes on it as a confirmed step does.
    Reached(PageObservation),
    /// `max_ms` was reached on the step's page and it declares `on_timeout`: the run ends there,
    /// on `page_label`, with `outcome_key`.
    Keyed {
        page_label: String,
        outcome_key: String,
    },
}

impl PreparedContainedTask {
    /// The wait of the wait step `operation`, whose `StepStarted` (index `step_index`) is
    /// written; no input. A failure writes the step's `StepFinished` on the last frame that
    /// matched a page and ends the run: an error page with `contained_task_error_page_reached`,
    /// the away clock or `max_ms` without `on_timeout` with `contained_task_step_unconfirmed`.
    pub(super) fn wait_step<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        ocr_collector: &mut PostAdmissionOcrCollector<'_>,
        operation: &TaskOperation,
        step_index: u32,
        (timing, pacing): (ContainedTaskTimingContext, StepPacing),
    ) -> Result<WaitStepEnd, ContainedTaskRunError<R::Error>> {
        let effective = operation.effective_timing(&self.control);
        let resolution = self.await_postcondition(
            runtime,
            ocr_collector,
            operation,
            Duration::from_millis(effective.timeout.milliseconds),
            Duration::from_millis(effective.interval.milliseconds),
            timing,
            pacing,
        )?;
        let (observation, last_frame_page, timing_failure) = match resolution {
            PostconditionResolution::Reached(reached) => return Ok(WaitStepEnd::Reached(reached)),
            PostconditionResolution::Held {
                observation,
                timing_failure,
            } => {
                if let Some(key) = operation.wait_on_timeout_key() {
                    return Ok(WaitStepEnd::Keyed {
                        page_label: observation.page_label,
                        outcome_key: key.to_owned(),
                    });
                }
                let page = observation.page_label.clone();
                (Some(observation), Some(page), Some(timing_failure))
            }
            PostconditionResolution::Failed {
                observation,
                hit_error_page: true,
                ..
            } => {
                Self::finish_effect_attempt(
                    runtime,
                    step_index,
                    &operation.id,
                    observation.as_ref(),
                )?;
                return Err(ContainedTaskError::with_detail(
                    ERROR_PAGE_REACHED,
                    confirmation_failure_detail(&operation.id, 1, observation.as_ref(), true, None),
                )
                .into());
            }
            PostconditionResolution::Failed {
                observation,
                last_frame_page,
                timing_failure,
                ..
            } => (observation, last_frame_page, timing_failure),
        };
        Self::finish_effect_attempt(runtime, step_index, &operation.id, observation.as_ref())?;
        // #411 A15: the operation, its destination pages, its one attempt and the page of the
        // wait's last frame (none when that frame matched no page).
        let mut values = Outcome::new(STEP_UNCONFIRMED)
            .pack_operation_id(operation.id.as_str())
            .page_ids(operation.destination_pages()?)
            .attempts(1_u32);
        if let Some(page) = last_frame_page {
            values = values.page_id(page);
        }
        Err(ContainedTaskError::with_detail(
            STEP_UNCONFIRMED,
            confirmation_failure_detail(
                &operation.id,
                1,
                observation.as_ref(),
                false,
                timing_failure.as_ref(),
            ),
        )
        .with_values(values)
        .with_timing(timing_failure)
        .into())
    }
}
