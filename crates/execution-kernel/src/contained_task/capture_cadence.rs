// SPDX-License-Identifier: AGPL-3.0-only

//! PERF-B1 (Workflow #407 B1; `contracts/resource-declarations.md`, "Capture cadence"): the
//! cadence of a step's captures while it waits for a page.
//!
//! - **A wait's own interval (R1).** A package declares its capture interval for every wait
//!   (`control.capture_interval_ms`, 50 ms when it declares none) and, per step, the interval of
//!   that step's one wait: `expect_after.interval_ms` (the confirmation after its input, or a wait
//!   step's wait) and a `linear_steps` transition's `interval_ms`. The package's other steps keep
//!   their own interval.
//! - **The ceiling is configuration (R2).** Admission bounds a declared interval only by the
//!   longest wait a step may declare (`MAX_DECLARED_INTERVAL_MS`, the range maximum of
//!   `execution.capture.max_interval_ms`). The run refuses a declared interval above the
//!   ceiling in effect ([`CaptureCadence::max_declared_interval_ms`]) before its first capture,
//!   with the code its admission gives a broken interval (`check_declared_intervals`).
//! - **Back-off on unchanged frames (R3).** While a step waits for a page (the page wait and the
//!   confirmation of the page-graph path, the waits of a `linear_steps` step), each frame of the
//!   wait that is unchanged from the wait's frame before it by the Runtime's one frame-change
//!   measure (#411 B12, the whole frame, as #411 A43b compares a wait's frames) doubles the next
//!   pause, up to the idle ceiling or the declared interval, whichever is longer; the first changed
//!   frame returns it to the declared interval (`CaptureBackoff`). A handled shared interrupt
//!   page or a failed observation starts the comparison afresh. The pause is cut, as every pause of
//!   a wait is, at the wait's remaining time and at the task's deadline. Entry checks, observation
//!   windows, input rechecks, samples and settles keep their declared interval.
//! - **The runtime hands it.** The kernel reads no configuration: the cadence rides on each
//!   step's pacing ([`StepPacing::capture_cadence`]). A runtime that hands none (offline
//!   simulation, fixtures) gets [`CaptureCadence::UNCONFIGURED`]: the configuration's default
//!   ceiling and no back-off, so its waits keep the cadence they had.
//! - **The still limit (STILL-1, Workflow #407, register §1.249).** A page wait or a page-graph
//!   confirmation (a wait step's wait included) whose frames stay unchanged for
//!   `execution.capture.still_limit_ms` of wall time, none of them what the wait accepts, ends at
//!   once through its own failure path and code, with `held_ms` and `limit_ms` on the outcome. The
//!   back-off keeps the clock, with the wait's frames, whatever the switch of the back-off says
//!   and whatever the step's factor is: the rule of a still and unrecognised screen
//!   ([`super::StillUnrecognised`]) tracks nothing at the factor 1000. A frame of the page the wait
//!   holds on is never counted, and it and a changed frame, a handled shared interrupt page or a
//!   failed observation restart the clock. 0 switches it off, with no clock read and no frame
//!   compared for it. The waits of a `linear_steps` step keep their limits.

use super::observation_window::StillScreen;
use super::{
    ColorDigest, ContainedTaskError, ContainedTaskRuntime, PreparedContainedTask, Scene,
    StepPacing, linear,
};
use crate::codes::ExecutionKernelCode;
use actingcommand_contract::outcome::Outcome;
use std::time::{Duration, Instant};

/// The configuration's default of the largest capture interval a package may declare, in
/// milliseconds (`execution.capture.max_interval_ms`).
const DEFAULT_MAX_DECLARED_INTERVAL_MS: u64 = 60_000;
/// The configuration's default idle ceiling of the back-off, in milliseconds
/// (`execution.capture.idle_ceiling_ms`).
const DEFAULT_IDLE_CEILING_MS: u64 = 5_000;
/// The largest capture interval admission takes: the longest wait a step may declare
/// (`expect_after.timeout_ms`), which is also the largest ceiling the configuration may set.
pub(super) const MAX_DECLARED_INTERVAL_MS: u64 =
    actingcommand_contract::MAX_POSTCONDITION_TIMEOUT_MS;

/// PERF-B1: the capture cadence in effect for one step, the `execution.capture` keys of the
/// Runtime's configuration, handed by the runtime with the step's pacing
/// ([`StepPacing::capture_cadence`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct CaptureCadence {
    /// The largest capture interval a package may declare, in milliseconds
    /// (`execution.capture.max_interval_ms`): the run refuses a package that declares a
    /// longer one.
    pub max_declared_interval_ms: u64,
    /// Whether a step's wait for a page lengthens its pause over unchanged frames
    /// (`execution.capture.backoff_enabled`).
    pub backoff_enabled: bool,
    /// The longest pause the back-off reaches, in milliseconds, unless the declared interval is
    /// longer (`execution.capture.idle_ceiling_ms`).
    pub idle_ceiling_ms: u64,
    /// STILL-1: how long, in milliseconds of wall time, a wait's frames may stay unchanged
    /// without one it accepts before the wait ends; 0 is off (`execution.capture.still_limit_ms`
    /// in effect for the run's instance).
    pub still_limit_ms: u64,
}

impl CaptureCadence {
    /// The cadence of a runtime that hands none: the configuration's default ceiling, no back-off
    /// and no still limit, so a replay or a fixture keeps the cadence its waits had.
    pub const UNCONFIGURED: Self = Self {
        max_declared_interval_ms: DEFAULT_MAX_DECLARED_INTERVAL_MS,
        backoff_enabled: false,
        idle_ceiling_ms: DEFAULT_IDLE_CEILING_MS,
        still_limit_ms: 0,
    };

    /// The cadence of the three `execution.capture` keys of the back-off, with no still limit.
    pub const fn new(
        max_declared_interval_ms: u64,
        backoff_enabled: bool,
        idle_ceiling_ms: u64,
    ) -> Self {
        Self {
            max_declared_interval_ms,
            backoff_enabled,
            idle_ceiling_ms,
            still_limit_ms: 0,
        }
    }

    /// STILL-1: the same cadence with the still limit `still_limit_ms`, 0 for none.
    pub const fn with_still_limit_ms(self, still_limit_ms: u64) -> Self {
        Self {
            still_limit_ms,
            ..self
        }
    }
}

impl Default for CaptureCadence {
    fn default() -> Self {
        Self::UNCONFIGURED
    }
}

/// STILL-1: a wait ended on its still limit: how long its frames had stayed unchanged and the
/// limit in effect, both in milliseconds of wall time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct StillLimitReached {
    held_ms: u64,
    limit_ms: u64,
}

impl StillLimitReached {
    /// `outcome` with the wait's `held_ms` and `limit_ms`.
    pub(super) fn values(self, outcome: Outcome) -> Outcome {
        outcome.held_ms(self.held_ms).limit_ms(self.limit_ms)
    }
}

/// PERF-B1 (R3): the pause before the next frame of one step's wait for a page. STILL-1: and, for
/// a wait under the still limit, how long its frames have stayed unchanged.
#[derive(Debug)]
pub(super) struct CaptureBackoff {
    /// The wait's declared interval.
    declared: Duration,
    /// The longest pause the back-off reaches: the idle ceiling, or the declared interval when
    /// that is longer; `None` while the back-off is switched off.
    ceiling: Option<Duration>,
    /// The pause before the next frame.
    current: Duration,
    /// The frame-change digest of the wait's latest frame.
    previous: Option<ColorDigest>,
    /// STILL-1: the still limit of a wait that has one in effect.
    still_limit: Option<Duration>,
    /// STILL-1: the wall instant of the first frame of the wait's latest still stretch; `None`
    /// before any frame counts and after one that restarts the clock without counting.
    still_since: Option<Instant>,
}

impl CaptureBackoff {
    /// The back-off of a wait whose declared interval is `declared`, at the cadence of its step's
    /// `pacing`; the wait takes no still limit.
    pub(super) fn new(declared: Duration, pacing: StepPacing) -> Self {
        let cadence = pacing.capture_cadence;
        let ceiling = Duration::from_millis(cadence.idle_ceiling_ms).max(declared);
        Self {
            declared,
            ceiling: cadence.backoff_enabled.then_some(ceiling),
            current: declared,
            previous: None,
            still_limit: None,
            still_since: None,
        }
    }

    /// STILL-1: the back-off of a page wait or a page-graph confirmation, which takes the still
    /// limit of its step's `pacing` when that is not 0.
    pub(super) fn still_limited(declared: Duration, pacing: StepPacing) -> Self {
        let limit = pacing.capture_cadence.still_limit_ms;
        Self {
            still_limit: (limit != 0).then_some(Duration::from_millis(limit)),
            ..Self::new(declared, pacing)
        }
    }

    /// A frame of the wait that did not end it, its `scene`: unchanged from the wait's frame
    /// before it, the next pause doubles up to the ceiling; changed, or the wait's first frame, it
    /// is the declared interval. Switched off, with no still limit either, nothing is compared. A
    /// digest the frame cannot give is `contained_task_recognition_failed`.
    pub(super) fn frame(&mut self, scene: &Scene) -> Result<(), ContainedTaskError> {
        self.compare(scene).map(drop)
    }

    /// Compares `scene` with the wait's frame before it, as [`Self::frame`] says, and answers
    /// whether it is unchanged.
    fn compare(&mut self, scene: &Scene) -> Result<bool, ContainedTaskError> {
        if self.ceiling.is_none() && self.still_limit.is_none() {
            return Ok(false);
        }
        let digest = StillScreen::WHOLE_FRAME.digest(scene)?;
        let unchanged = match &self.previous {
            Some(previous) => !StillScreen::WHOLE_FRAME.moved(previous, &digest)?,
            None => false,
        };
        if let Some(ceiling) = self.ceiling {
            self.current = if unchanged {
                self.current.saturating_mul(2).min(ceiling)
            } else {
                self.declared
            };
        }
        self.previous = Some(digest);
        Ok(unchanged)
    }

    /// STILL-1: [`Self::frame`] for a frame that the still limit counts, which is neither what the
    /// wait accepts nor the page it holds on: a changed frame starts the still clock again, and
    /// an unchanged one keeps it running. Without a limit the clock is not read.
    pub(super) fn still_frame<R: ContainedTaskRuntime>(
        &mut self,
        runtime: &mut R,
        scene: &Scene,
    ) -> Result<(), ContainedTaskError> {
        let unchanged = self.compare(scene)?;
        if self.still_limit.is_some() && (!unchanged || self.still_since.is_none()) {
            self.still_since = Some(runtime.monotonic_now());
        }
        Ok(())
    }

    /// STILL-1: [`Self::frame`] for a frame of the page the wait holds on: the wait accepts it, so
    /// it counts for nothing and the still clock stops.
    pub(super) fn held_frame(&mut self, scene: &Scene) -> Result<(), ContainedTaskError> {
        self.still_since = None;
        self.frame(scene)
    }

    /// STILL-1: whether the wait's frames have stayed unchanged for its still limit, up to now.
    pub(super) fn still_limit_reached<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
    ) -> Option<StillLimitReached> {
        let (limit, since) = (self.still_limit?, self.still_since?);
        let held = runtime.monotonic_now().saturating_duration_since(since);
        (held >= limit).then(|| StillLimitReached {
            held_ms: u64::try_from(held.as_millis()).unwrap_or(u64::MAX),
            limit_ms: u64::try_from(limit.as_millis()).unwrap_or(u64::MAX),
        })
    }

    /// The screen was acted on (a handled shared interrupt page) or the wait's frames are not
    /// known (a failed observation): the next frame is compared with none, at the declared
    /// interval, and it starts the still clock again.
    pub(super) fn reset(&mut self) {
        self.previous = None;
        self.current = self.declared;
        self.still_since = None;
    }

    /// The pause before the next frame, before the wait cuts it at its remaining time.
    pub(super) const fn interval(&self) -> Duration {
        self.current
    }
}

impl PreparedContainedTask {
    /// PERF-B1 (R2): every capture interval the package declares is at most the ceiling of
    /// `pacing`'s cadence, checked once at the start of a run or an entry check, before its first
    /// capture. One above it is refused with the code admission gives a broken interval:
    /// `contained_task_control_invalid` for `capture_interval_ms` and an `expect_after.interval_ms`,
    /// `contained_task_linear_invalid` (`reason=transition_interval`) for a transition's.
    pub(super) fn check_declared_intervals(
        &self,
        pacing: StepPacing,
    ) -> Result<(), ContainedTaskError> {
        let ceiling = pacing.capture_cadence.max_declared_interval_ms;
        let above = |declared: Option<u64>| declared.is_some_and(|declared| declared > ceiling);
        let refused = |member: &str, operation: Option<&str>| {
            ContainedTaskError::with_detail(
                ExecutionKernelCode::ContainedTaskControlInvalid.code(),
                match operation {
                    Some(operation) => format!(
                        "reason=capture_interval_ceiling operation={operation} member={member} ceiling_ms={ceiling}"
                    ),
                    None => {
                        format!(
                            "reason=capture_interval_ceiling member={member} ceiling_ms={ceiling}"
                        )
                    }
                },
            )
        };
        if above(self.control.capture_interval_ms) {
            return Err(refused("capture_interval_ms", None));
        }
        for operation in &self.program.operations {
            if above(
                operation
                    .expect_after
                    .as_ref()
                    .and_then(|expectation| expectation.interval_ms),
            ) {
                return Err(refused(
                    "expect_after/interval_ms",
                    Some(operation.id.as_str()),
                ));
            }
            if let Some(linear::TaskTransition::Page { interval_ms, .. }) = &operation.transition
                && above(*interval_ms)
            {
                return Err(ContainedTaskError::with_detail(
                    ExecutionKernelCode::ContainedTaskLinearInvalid.code(),
                    format!(
                        "reason=transition_interval operation={} ceiling_ms={ceiling}",
                        operation.id
                    ),
                ));
            }
        }
        Ok(())
    }
}
