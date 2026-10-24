// SPDX-License-Identifier: AGPL-3.0-only

//! PERF-B1 (Workflow #407 B1): the cadence of a step's captures while it waits for a page, as the
//! runtime hands it with each step's pacing (`capture_cadence.*` keys). The kernel reads no
//! configuration.

/// The configuration's default of the largest capture interval a package may declare, in
/// milliseconds (`capture_cadence.max_declared_interval_ms`).
pub const DEFAULT_MAX_DECLARED_INTERVAL_MS: u64 = 60_000;
/// The configuration's default idle ceiling of the back-off, in milliseconds
/// (`capture_cadence.idle_ceiling_ms`).
pub const DEFAULT_IDLE_CEILING_MS: u64 = 5_000;

/// PERF-B1: the capture cadence in effect for one step, the `capture_cadence` keys of the
/// Runtime's configuration, handed by the runtime with the step's pacing
/// ([`super::StepPacing::capture_cadence`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct CaptureCadence {
    /// The largest capture interval a package may declare, in milliseconds.
    pub max_declared_interval_ms: u64,
    /// Whether a wait lengthens its pause over unchanged frames.
    pub backoff_enabled: bool,
    /// The longest pause the back-off reaches, in milliseconds.
    pub idle_ceiling_ms: u64,
}

impl CaptureCadence {
    /// The cadence of a runtime that hands none.
    pub const UNCONFIGURED: Self = Self {
        max_declared_interval_ms: DEFAULT_MAX_DECLARED_INTERVAL_MS,
        backoff_enabled: false,
        idle_ceiling_ms: DEFAULT_IDLE_CEILING_MS,
    };

    /// The cadence of the three `capture_cadence` keys.
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
