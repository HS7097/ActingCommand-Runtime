// SPDX-License-Identifier: AGPL-3.0-only

// The scripted clock of the Lab's gate tests that run the kernel's interpreter on a device of
// their own (test plan E-3, EXEC-I3: every timer and every wait of a run goes through its
// runtime's `monotonic_now` and `pause`). It is the clock the kernel's own scripted runtime keeps
// (`crates/execution-kernel/src/contained_task/tests/scripted.rs`), which is private to the kernel
// crate: its instants move only by the pauses the run asks for, so a run never sleeps and nothing
// it decides depends on how long the test takes. A gate file reads no clock (the architecture
// guard `gate_tests_are_deterministic`); this support module is no gate file, as the kernel's is
// not, and is read once per device, for its base only.

use std::time::{Duration, Instant};

// The scripted clock starts this far ahead of the wall clock, as the kernel's does: the
// recognition pack still compares a sampled target's deadline with the wall clock (outside the
// seam), and an hour ahead keeps that comparison from ever deciding a scripted run.
const CLOCK_LEAD: Duration = Duration::from_secs(3_600);

/// A clock whose instants move only by its pauses.
pub(crate) struct ScriptedClock {
    base: Instant,
    offset: Duration,
}

impl ScriptedClock {
    /// A clock at its base, an hour ahead of the wall clock.
    pub(crate) fn started() -> Self {
        Self {
            base: Instant::now() + CLOCK_LEAD,
            offset: Duration::ZERO,
        }
    }

    /// The instant the run reads: the base and every pause so far.
    pub(crate) fn now(&self) -> Instant {
        self.base + self.offset
    }

    /// A pause of the run: the clock moves on by `duration`, and nothing sleeps.
    pub(crate) fn pause(&mut self, duration: Duration) {
        self.offset += duration;
    }
}
