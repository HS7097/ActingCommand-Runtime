// SPDX-License-Identifier: AGPL-3.0-only

//! #411 A43b (Workflow #411 A43; coordinator 10-21, Q8 of 10-21 20:2x): a wait that sees a still
//! screen it cannot recognise is not stretched by the step's pacing factor. One rule for the page
//! wait (`capture_until_page`) and a step's confirmation (`await_postcondition`):
//!
//! - **Still and unrecognised.** It holds while the latest two frames the wait captured are
//!   unchanged by the Runtime's one frame-change measure (Workflow #411 B12: the whole frame's
//!   colour digest, still at most `FRAME_CHANGE_MAX_MILLI`) and neither matched a page the wait
//!   accepts. A wait's frame is the frame its observation recognised.
//! - **Counted unpaced.** While it holds, the rest of the wait is counted at the factor 1000: its
//!   pauses last their declared length and the wall time that passes counts in full against its
//!   limit. Otherwise the wait is paced as every wait is (#385 P-k).
//! - **Back at once.** A frame the wait accepts ends it as before; a frame that changed returns
//!   it to its pacing. A handled shared interrupt page or a failed observation clears the state.
//! - **Unchanged.** The run's budget clock, the task's deadline, the extra wait rounds and the
//!   outcome of a wait that runs out (its code, and its elapsed time as the wait counts it).
//!
//! The wait's limits stay on the run's budget clock. A stretch counted unpaced ran there
//! `1000 / f` as fast as the wait counts it; the difference is the wait's credit, which shortens
//! the limit of its window. At the factor 1000 nothing is tracked and no clock is read.

use super::observation_window::StillScreen;
use super::{
    ColorDigest, ContainedTaskError, ContainedTaskRuntime, PaceClock, Scene,
    budget_observation_now, pace_pause,
};
use std::time::{Duration, Instant};

/// #411 A43b: whether a wait's latest two frames are still and unrecognised, and how much of its
/// window it has counted unpaced since the window started.
#[derive(Debug)]
pub(super) struct StillUnrecognised {
    /// The run's budget clock at the wait's factor.
    pace: PaceClock,
    /// The frame-change digest of the wait's latest frame, which matched no page it accepts.
    previous: Option<ColorDigest>,
    /// While the latest two frames are still and unrecognised: the wall instant up to which the
    /// wait's unpaced count is settled.
    since: Option<Instant>,
    /// The budget time the wait's window counted beyond the run's budget clock: each stretch
    /// counted unpaced less its budget time.
    credit: Duration,
}

impl StillUnrecognised {
    /// The rule for one wait on the run's budget clock `pace`, before its first frame.
    pub(super) fn new(pace: PaceClock) -> Self {
        Self {
            pace,
            previous: None,
            since: None,
            credit: Duration::ZERO,
        }
    }

    /// A frame of the wait that matched no page it accepts, its `scene`: from now on the state
    /// holds when this frame and the one before it are unchanged. The stretch up to this frame
    /// is settled first. A digest the frame cannot give is `contained_task_recognition_failed`.
    pub(super) fn frame<R: ContainedTaskRuntime>(
        &mut self,
        runtime: &mut R,
        scene: &Scene,
    ) -> Result<(), ContainedTaskError> {
        if self.pace.at_unit_factor() {
            return Ok(());
        }
        let now = runtime.monotonic_now();
        self.settle_to(now);
        let digest = StillScreen::WHOLE_FRAME.digest(scene)?;
        let still = match &self.previous {
            Some(previous) => !StillScreen::WHOLE_FRAME.moved(previous, &digest)?,
            None => false,
        };
        self.previous = Some(digest);
        self.since = still.then_some(now);
        Ok(())
    }

    /// A shared interrupt page was handled on the wait's latest frame: the screen was acted on
    /// and the handling is no time of the wait, so the wait is paced until two later frames are
    /// still and unrecognised.
    pub(super) fn interrupted(&mut self) {
        self.previous = None;
        self.since = None;
    }

    /// An observation of the wait failed and is made again in a new window (Workflow #386
    /// SR-1): its frames are not known, so the wait is paced again and the new window counts
    /// from its start.
    pub(super) fn observation_failed(&mut self) {
        self.previous = None;
        self.since = None;
        self.credit = Duration::ZERO;
    }

    /// The wait starts a new window of its limit (an extra round, #385 P-k): the window counts
    /// from its start; the wait's frames go on.
    pub(super) fn new_window<R: ContainedTaskRuntime>(&mut self, runtime: &mut R) {
        self.settle(runtime);
        self.credit = Duration::ZERO;
    }

    /// The window's credit, settled up to now.
    pub(super) fn credit<R: ContainedTaskRuntime>(&mut self, runtime: &mut R) -> Duration {
        self.settle(runtime);
        self.credit
    }

    /// The wait's limit `timeout` from its window's start, in budget time of the run: shortened
    /// by the window's credit.
    pub(super) fn limit<R: ContainedTaskRuntime>(
        &mut self,
        runtime: &mut R,
        timeout: Duration,
    ) -> Duration {
        timeout.saturating_sub(self.credit(runtime))
    }

    /// The end of the window that started at `window_started` (a budget instant read from
    /// [`ContainedTaskRuntime::observation_now`]) and lasts `timeout`, as a budget instant of
    /// the run: while the state holds, the rest of the window is counted unpaced.
    pub(super) fn window_end<R: ContainedTaskRuntime>(
        &mut self,
        runtime: &mut R,
        window_started: Instant,
        timeout: Duration,
    ) -> Instant {
        let end = window_started + self.limit(runtime, timeout);
        if self.since.is_none() {
            return end;
        }
        let now = budget_observation_now(runtime, self.pace);
        now + self.pace.budget_span(end.saturating_duration_since(now))
    }

    /// The wait's pause before its next frame: `wait` as the window counts it, and at most
    /// `task_left` of the run's budget time. While the state holds it lasts `wait` itself, as at
    /// the factor 1000, never past the task's deadline; otherwise it is paced as every wait,
    /// `wait.min(task_left) × f / 1000`.
    pub(super) fn pause<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        wait: Duration,
        task_left: Duration,
    ) {
        if self.since.is_some() {
            runtime.pause(wait.min(self.pace.wall_span(task_left)));
        } else {
            pace_pause(runtime, self.pace, wait.min(task_left));
        }
    }

    /// Settles the unpaced count up to now; the clock is read only while the state holds.
    fn settle<R: ContainedTaskRuntime>(&mut self, runtime: &mut R) {
        if self.since.is_some() {
            let now = runtime.monotonic_now();
            self.settle_to(now);
        }
    }

    /// Settles the unpaced count up to the wall instant `now`: the stretch since the last
    /// settlement counts in full, of which the run's budget clock counted `1000 / f`.
    fn settle_to(&mut self, now: Instant) {
        if let Some(since) = self.since {
            let wall = now.saturating_duration_since(since);
            self.credit = self
                .credit
                .saturating_add(wall.saturating_sub(self.pace.budget_span(wall)));
            self.since = Some(now);
        }
    }
}
