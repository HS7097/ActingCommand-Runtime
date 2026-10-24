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

use super::observation_window::StillScreen;
use super::{ColorDigest, ContainedTaskError, PreparedContainedTask, Scene, StepPacing, linear};
use crate::codes::ExecutionKernelCode;
use std::time::Duration;

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
}

impl CaptureCadence {
    /// The cadence of a runtime that hands none: the configuration's default ceiling, and no
    /// back-off, so a replay or a fixture keeps the cadence its waits had.
    pub const UNCONFIGURED: Self = Self {
        max_declared_interval_ms: DEFAULT_MAX_DECLARED_INTERVAL_MS,
        backoff_enabled: false,
        idle_ceiling_ms: DEFAULT_IDLE_CEILING_MS,
    };

    /// The cadence of the three `execution.capture` keys.
    pub const fn new(
        max_declared_interval_ms: u64,
        backoff_enabled: bool,
        idle_ceiling_ms: u64,
    ) -> Self {
        Self {
            max_declared_interval_ms,
            backoff_enabled,
            idle_ceiling_ms,
        }
    }
}

impl Default for CaptureCadence {
    fn default() -> Self {
        Self::UNCONFIGURED
    }
}

/// PERF-B1 (R3): the pause before the next frame of one step's wait for a page.
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
}

impl CaptureBackoff {
    /// The back-off of a wait whose declared interval is `declared`, at the cadence of its step's
    /// `pacing`.
    pub(super) fn new(declared: Duration, pacing: StepPacing) -> Self {
        let cadence = pacing.capture_cadence;
        let ceiling = Duration::from_millis(cadence.idle_ceiling_ms).max(declared);
        Self {
            declared,
            ceiling: cadence.backoff_enabled.then_some(ceiling),
            current: declared,
            previous: None,
        }
    }

    /// A frame of the wait that did not end it, its `scene`: unchanged from the wait's frame
    /// before it, the next pause doubles up to the ceiling; changed, or the wait's first frame, it
    /// is the declared interval. Switched off, nothing is compared. A digest the frame cannot give
    /// is `contained_task_recognition_failed`.
    pub(super) fn frame(&mut self, scene: &Scene) -> Result<(), ContainedTaskError> {
        let Some(ceiling) = self.ceiling else {
            return Ok(());
        };
        let digest = StillScreen::WHOLE_FRAME.digest(scene)?;
        let unchanged = match &self.previous {
            Some(previous) => !StillScreen::WHOLE_FRAME.moved(previous, &digest)?,
            None => false,
        };
        self.current = if unchanged {
            self.current.saturating_mul(2).min(ceiling)
        } else {
            self.declared
        };
        self.previous = Some(digest);
        Ok(())
    }

    /// The screen was acted on (a handled shared interrupt page) or the wait's frames are not
    /// known (a failed observation): the next frame is compared with none, at the declared
    /// interval.
    pub(super) fn reset(&mut self) {
        self.previous = None;
        self.current = self.declared;
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
