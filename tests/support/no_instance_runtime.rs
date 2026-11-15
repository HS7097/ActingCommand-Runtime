// SPDX-License-Identifier: AGPL-3.0-only

//! A Runtime without instances for tests outside runtime-host: its provider, a salt and the bound
//! of a broken run's waits. Workflow #434 LAB-6S4a (0.12.63): the held start this support also
//! held (`HeldStart`, actingd's `--install-held` and its release through `InstallTransition`) went
//! with the held start; the provider and the constants stay for the tests that start a Runtime of
//! no device.

use actingcommand_contract::{ApplicationLifecycleAction, FencedWrite};
use actingcommand_device::{
    CaptureBackend, DeviceError, DeviceResult, FrameMemoryBudget, InputBackend, OpenedBackend,
};
use actingcommand_runtime_host::{ExecutionBackendProvider, ResolvedExecutionInstance};
use std::time::Duration;

/// Bounds a broken run's waits only; every wait returns as soon as its message arrives.
pub const GATE_WAIT: Duration = Duration::from_secs(120);
/// The secret fingerprint salt of these Runtimes, its bytes unchanged from the held start's.
pub const TEST_SALT: &[u8] = b"held-runtime-test-salt";

/// A provider without instances: the Runtime it starts has no device.
pub struct NoInstances;

impl ExecutionBackendProvider for NoInstances {
    fn instance_aliases(&self) -> Vec<String> {
        Vec::new()
    }

    fn resolve(&self, _instance_alias: &str) -> Option<ResolvedExecutionInstance> {
        None
    }

    fn open_input(
        &self,
        _instance_alias: &str,
    ) -> DeviceResult<OpenedBackend<Box<dyn InputBackend>>> {
        Err(DeviceError::fatal("the test Runtime has no instance"))
    }

    fn open_capture(
        &self,
        _instance_alias: &str,
        _memory: Option<&FrameMemoryBudget>,
    ) -> DeviceResult<OpenedBackend<Box<dyn CaptureBackend>>> {
        Err(DeviceError::fatal("the test Runtime has no instance"))
    }

    fn control_application(
        &self,
        _witness: &FencedWrite,
        _instance_alias: &str,
        _action: ApplicationLifecycleAction,
    ) -> DeviceResult<()> {
        Err(DeviceError::fatal("the test Runtime has no instance"))
    }
}
