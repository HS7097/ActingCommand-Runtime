// SPDX-License-Identifier: AGPL-3.0-only

//! ACT-1 (Workflow #407, register §1.250; `contracts/performance-control.md`, "Step pacing"): the
//! settled path of a step after its input. Under `execution.action_gap.mode` `settled` a tap reads
//! frames from the first frame after the input instead of waiting its `post_delay_ms` blind:
//!
//! - **A destination frame goes on at once.** That frame is the step's frame and the next step's
//!   decision frame, as a confirmation's is; the next step's guard still judges it. With
//!   `settle_frames` of 2 or more the step takes that many consecutive destination frames unchanged
//!   by at most `settle_change_max_milli`, and only the last of them feeds the run's collectors.
//! - **`post_delay_ms` is the limit, never added on top.** At its end, or at once on a frame of the
//!   confirmation's declared wait page, the step goes to its confirmation as it always did, which
//!   starts its own timeout there. So a step is never given less time than before.
//! - **Each frame keeps its checks.** A frame of neither a destination nor an error page is offered
//!   to the shared interrupt pages, and a handled one moves the limit on. An error page is left to
//!   the confirmation, since the frames right after an input may still show what the input
//!   dismisses. Only a frame that goes on feeds the post-admission collectors and the readings.
//! - **Where the frames prove nothing, or the step must not hurry, nothing changes.** A step keeps
//!   its `post_delay_ms` when no tap was sent, when it declares no `post_delay_ms` or an observation
//!   window, when its destinations name the page it left (the first frame would pass without any
//!   sign of the input), when it is a designated effect, a store step or a scan, when the runtime
//!   hands no settled path (`mode` `declared`, a stage above `settled_max_stage`, an offline run),
//!   after a failure in this run, and while its operation is held ([`SettledHolds`]).
//!
//! The path adds no input and no retry: the failure decision after a failed confirmation is the
//! one before it. No record changes shape: its frames and waits are recorded as a confirmation's.

use super::observation_window::StillScreen;
use super::sample_retry::SampleRetries;
use super::wait_step::HeldPage;
use super::{
    ColorDigest, ContainedTaskBoundaryTiming, ContainedTaskError, ContainedTaskRunError,
    ContainedTaskRuntime, ContainedTaskTimingContext, PageObservation, PostAdmissionOcrCollector,
    PreparedContainedTask, Scene, StepPacing, TaskOperation, TaskTimingCheckPosition,
    TaskTimingStage, budget_elapsed_since, budget_now, budget_observation_now, pace_pause,
};
use crate::codes::{ExecutionKernelCode, ExecutionKernelLocation};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// Decides, for a frame recognized as the page `page_label`, whether it feeds the run's collectors
/// (`capture_frame_gated`); a digest the frame cannot give is its refusal.
pub(super) type FeedGate<'g> = dyn FnMut(&str, &Scene) -> Result<bool, ContainedTaskError> + 'g;

/// The `execution.action_gap` keys of a step on the settled path, handed by the runtime with its
/// pacing ([`StepPacing::settled_path`]). The kernel reads no configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct SettledPath {
    /// The pause between two frames (`poll_ms`).
    pub poll: Duration,
    /// How many consecutive destination frames the step takes (`settle_frames`, at least 1).
    pub settle_frames: u32,
    /// The largest frame change between two of them (`settle_change_max_milli`).
    pub settle_change_max_milli: u32,
}

impl SettledPath {
    /// The path of the three keys.
    pub const fn new(poll: Duration, settle_frames: u32, settle_change_max_milli: u32) -> Self {
        Self {
            poll,
            settle_frames,
            settle_change_max_milli,
        }
    }
}

/// ACT-1: the operations that failed on the settled path, by package and operation, and until when
/// each stays on its `post_delay_ms` (`execution.action_gap.fallback_hold_ms`), in memory: a
/// restart forgets them. The runtime keeps one for its runs; the instants are its clock's.
#[derive(Debug, Default)]
pub struct SettledHolds {
    until: BTreeMap<(String, String), Instant>,
}

impl SettledHolds {
    /// `operation_label` of `package_id` failed at `now`: held for `span`; none is no hold. Holds
    /// that ended go, so the table never outgrows what failed within one hold.
    pub fn hold(&mut self, package_id: &str, operation_label: &str, now: Instant, span: Duration) {
        self.until.retain(|_, until| *until > now);
        if !span.is_zero() {
            self.until.insert(
                (package_id.to_owned(), operation_label.to_owned()),
                now + span,
            );
        }
    }

    /// Whether `operation_label` of `package_id` is held at `now`.
    pub fn held(&self, package_id: &str, operation_label: &str, now: Instant) -> bool {
        self.until
            .get(&(package_id.to_owned(), operation_label.to_owned()))
            .is_some_and(|until| now < *until)
    }
}

/// One step's reading: its path and its limit, the operation's `post_delay_ms` in budget time.
#[derive(Debug, Clone, Copy)]
pub(super) struct SettledWindow {
    path: SettledPath,
    limit: Duration,
}

/// How many consecutive destination frames the reading has seen.
struct SettleCount {
    frames: u32,
    screen: StillScreen,
    count: u32,
    previous: Option<ColorDigest>,
}

impl SettleCount {
    const fn new(path: SettledPath) -> Self {
        Self {
            frames: path.settle_frames,
            screen: StillScreen::whole_frame_within(path.settle_change_max_milli),
            count: 0,
            previous: None,
        }
    }

    /// A destination frame, its `scene`: whether the reading now goes on it. One frame compares
    /// nothing; otherwise a changed frame starts the count again at one.
    fn frame(&mut self, scene: &Scene) -> Result<bool, ContainedTaskError> {
        if self.frames <= 1 {
            self.count = 1;
            return Ok(true);
        }
        let digest = self.screen.digest(scene)?;
        let unchanged = match &self.previous {
            Some(previous) => !self.screen.moved(previous, &digest)?,
            None => false,
        };
        self.count = if unchanged { self.count + 1 } else { 1 };
        self.previous = Some(digest);
        Ok(self.settled())
    }

    /// A frame of no destination, or one whose screen was acted on: the count starts again.
    fn away(&mut self) {
        self.count = 0;
        self.previous = None;
    }

    const fn settled(&self) -> bool {
        self.count >= self.frames
    }
}

impl PreparedContainedTask {
    /// The reading of `operation`'s step after its input, sent from the page `from_page`, when the
    /// step takes the settled path; `None` keeps its `post_delay_ms`. `tapped`: the input was a
    /// plain tap.
    pub(super) fn settled_window<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        operation: &TaskOperation,
        from_page: &str,
        pacing: StepPacing,
        tapped: bool,
    ) -> Result<Option<SettledWindow>, ContainedTaskRunError<R::Error>> {
        let Some(path) = pacing.settled_path else {
            return Ok(None);
        };
        let Some(post_delay_ms) = operation.post_delay_ms else {
            return Ok(None);
        };
        if !tapped || operation.observation_window.is_some() || operation.scan_max_ms().is_some() {
            return Ok(None);
        }
        let designated = self.scheduling_outcome.as_ref().is_some_and(|outcome| {
            outcome
                .designated_operations()
                .contains(&operation.id.as_str())
        });
        let store_step = self
            .control
            .store_update
            .as_ref()
            .is_some_and(|store| store.store_for(&operation.id).is_some());
        if designated || store_step {
            return Ok(None);
        }
        let game = &self.control.game;
        let destinations = operation.destination_pages()?;
        let names_the_source = destinations.iter().any(|destination| {
            crate::page_anchor_matches(game, from_page, destination)
                || crate::page_anchor_matches(game, &operation.from, destination)
        });
        if destinations.is_empty() || names_the_source {
            return Ok(None);
        }
        if runtime
            .settled_path_held(&self.control.package_id, &operation.id)
            .map_err(ContainedTaskRunError::operation::<R>)?
        {
            return Ok(None);
        }
        Ok(Some(SettledWindow {
            path,
            limit: Duration::from_millis(post_delay_ms),
        }))
    }

    /// The reading itself: the destination frame the step goes on, or `None` for its confirmation
    /// (the limit passed, or a frame of the confirmation's wait page). `timeout` is the
    /// confirmation's: a sampled observation is made within it, as a confirmation's frame is.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn settled_confirmation<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        ocr_collector: &mut PostAdmissionOcrCollector<'_>,
        operation: &TaskOperation,
        window: SettledWindow,
        timeout: Duration,
        timing: ContainedTaskTimingContext,
        pacing: StepPacing,
    ) -> Result<Option<PageObservation>, ContainedTaskRunError<R::Error>> {
        let task_deadline = timing.deadline();
        let pace = timing.pace();
        let game = &self.control.game;
        let destinations = operation.destination_pages()?;
        let sampled = !self.evaluator.pack().target_consensus.is_empty();
        let mut retries =
            SampleRetries::new(pacing, ExecutionKernelLocation::ObservationAfterInput);
        let mut started = budget_now(runtime, pace);
        let wait_page = HeldPage::of(operation, started);
        let mut settling = SettleCount::new(window.path);
        loop {
            if budget_now(runtime, pace) >= task_deadline {
                return Err(self
                    .task_timeout_error(
                        runtime,
                        TaskTimingStage::Postcondition,
                        task_deadline,
                        None,
                        pace,
                    )
                    .with_timing_check_position(TaskTimingCheckPosition::PostconditionBeforeCapture)
                    .into());
            }
            let frame_timing = if sampled {
                timing.with_deadline(budget_observation_now(runtime, pace) + timeout)
            } else {
                timing
            };
            let mut feeds = |page_label: &str, scene: &Scene| {
                let destination = destinations
                    .iter()
                    .any(|destination| crate::page_anchor_matches(game, page_label, destination));
                if destination {
                    settling.frame(scene)
                } else {
                    settling.away();
                    Ok(false)
                }
            };
            let captured = self.capture_frame_gated(
                runtime,
                Some(&mut *ocr_collector),
                None,
                frame_timing,
                None,
                None,
                Some(&mut feeds),
            );
            let observation = match captured {
                Ok(observation) => observation,
                Err(failure) => {
                    retries.retry(runtime, failure, timeout)?;
                    settling.away();
                    continue;
                }
            };
            if budget_now(runtime, pace) >= task_deadline {
                return Err(self
                    .task_timeout_error(
                        runtime,
                        TaskTimingStage::Postcondition,
                        task_deadline,
                        None,
                        pace,
                    )
                    .with_timing_check_position(TaskTimingCheckPosition::PostconditionAfterCapture)
                    .into());
            }
            let handled = match observation {
                Ok(observation) => {
                    let matches =
                        operation.matching_destination_count(&self.control, &observation)?;
                    let hit_error_page = self
                        .program
                        .is_error_page(&self.control, &observation.page_label);
                    if matches > 1 || (matches == 1 && hit_error_page) {
                        return Err(ContainedTaskError::with_detail(
                            ExecutionKernelCode::ContainedTaskRecognitionConflict.code(),
                            observation.page_label,
                        )
                        .into());
                    }
                    if matches == 1 {
                        if settling.settled() {
                            return Ok(Some(observation));
                        }
                        None
                    } else if wait_page
                        .as_ref()
                        .is_some_and(|held| held.shows(game, &observation.page_label))
                    {
                        return Ok(None);
                    } else if hit_error_page {
                        None
                    } else {
                        self.offer_shared_interrupt(runtime, &observation.scene)?
                    }
                }
                Err(scene) => {
                    settling.away();
                    self.offer_shared_interrupt(runtime, &scene)?
                }
            };
            if let Some(handled) = handled {
                started += pace.budget_span(handled);
                settling.away();
                continue;
            }
            let elapsed = budget_elapsed_since(runtime, pace, started);
            if elapsed >= window.limit {
                return Ok(None);
            }
            let boundary = actingcommand_contract::TaskTimingBoundary::PostconditionWait;
            let identity = runtime.task_boundary_identity(boundary);
            let wait_started = runtime.monotonic_now();
            let task_left = task_deadline.saturating_duration_since(pace.budget(wait_started));
            pace_pause(
                runtime,
                pace,
                window
                    .path
                    .poll
                    .min(window.limit.saturating_sub(elapsed))
                    .min(task_left),
            );
            let wait_ended = runtime.monotonic_now();
            runtime.observe_task_boundary(ContainedTaskBoundaryTiming {
                boundary,
                identity,
                context: timing,
                started: wait_started,
                ended: wait_ended,
                succeeded: true,
            });
        }
    }
}
