// SPDX-License-Identifier: AGPL-3.0-only

//! The shutdown wait of `request-shutdown --wait`, `install-transition --wait`, `start`,
//! `restart` and `install` (Workflow #382 A-1, G11). Workflow #434 LAB-1b: the wait itself moved
//! to `crates/lab-ops` (`actingcommand_lab_ops::shutdown_wait`), which serves the operation
//! `shutdown`; this gives its failure as `ActingctlError::ShutdownWait`, as actingctl's callers
//! and the G11 gate read it.

use crate::ActingctlError;
use actingcommand_contract::RuntimeShutdownTarget;
use actingcommand_lab_ops::shutdown_wait;
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

// Test-only: the G11 gate (`gate_382_a1`) waits with its own clock and liveness probe.
#[cfg(test)]
pub(crate) use actingcommand_lab_ops::shutdown_wait::Poller;

/// [`shutdown_wait::wait_for_shutdown`], its failure as `ActingctlError::ShutdownWait`.
pub(crate) fn wait_for_shutdown(
    state_root: &Path,
    target: &RuntimeShutdownTarget,
    wait: Duration,
) -> Result<Value, ActingctlError> {
    shutdown_wait::wait_for_shutdown(state_root, target, wait).map_err(ActingctlError::ShutdownWait)
}

/// [`shutdown_wait::wait_with`], its failure as `ActingctlError::ShutdownWait`.
#[cfg(test)]
pub(crate) fn wait_with(
    state_root: &Path,
    target: &RuntimeShutdownTarget,
    wait: Duration,
    poller: &mut impl Poller,
) -> Result<Value, ActingctlError> {
    shutdown_wait::wait_with(state_root, target, wait, poller).map_err(ActingctlError::ShutdownWait)
}
