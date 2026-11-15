// SPDX-License-Identifier: AGPL-3.0-only

//! Test-only: the shutdown wait of `request-shutdown --wait` (Workflow #382 A-1, G11). Workflow
//! #434 LAB-1b: the wait itself moved to `crates/lab-ops` (`actingcommand_lab_ops::shutdown_wait`),
//! which serves the operation `shutdown`; this gives its failure as `ActingctlError::ShutdownWait`,
//! as the G11 gate reads it. `start`, `restart` and `install` (in `crates/lab-rescue` since LAB-3)
//! take the crate's wait directly. Workflow #434 LAB-6S4a (0.12.62): `install-transition --wait`,
//! the one product caller of the wait here, is gone, so only the gate mounts this module.

use crate::ActingctlError;
use actingcommand_contract::RuntimeShutdownTarget;
use actingcommand_lab_ops::shutdown_wait;
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

// The G11 gate (`gate_382_a1`) waits with its own clock and liveness probe.
pub(crate) use actingcommand_lab_ops::shutdown_wait::Poller;

/// [`shutdown_wait::wait_with`], its failure as `ActingctlError::ShutdownWait`.
pub(crate) fn wait_with(
    state_root: &Path,
    target: &RuntimeShutdownTarget,
    wait: Duration,
    poller: &mut impl Poller,
) -> Result<Value, ActingctlError> {
    shutdown_wait::wait_with(state_root, target, wait, poller).map_err(ActingctlError::ShutdownWait)
}
