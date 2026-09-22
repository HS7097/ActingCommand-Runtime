// SPDX-License-Identifier: AGPL-3.0-only

//! Device-layer primitives for the Rust ActingCommand runtime mainline.
//!
//! This crate is intentionally narrow: touch input is selected through an
//! explicit backend chain so single-backend failures are visible and bounded.

#![deny(unsafe_op_in_unsafe_fn)]

// Workflow #182: environment readers and writers share one test-only lock.
#[cfg(test)]
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub mod adb;
mod adb_bounds_diagnostic;
mod backend_open;
pub mod capture;
pub mod discovery;
pub mod emulator;
pub mod error;
mod frame_memory;
pub mod input;
pub mod maatouch;
pub mod minitouch;
mod mumu;
pub mod mumu_manager;
mod nemu_diagnostic;
pub mod replay;
pub mod touch;
mod vendor_stdio;
mod vendor_stdio_facts;

pub use adb::*;
pub use adb_bounds_diagnostic::*;
pub use backend_open::*;
pub use capture::*;
pub use discovery::*;
pub use emulator::*;
pub use error::*;
pub use frame_memory::*;
pub use input::*;
pub use maatouch::*;
pub use minitouch::*;
pub use mumu::MumuInstallSource;
pub use mumu_manager::{
    DiscoveredMumuInstance, EmulatorControlFailure, EmulatorControlOutcome, EmulatorControlResult,
    InstanceState, MAX_MUMU_INSTANCE_NAME_BYTES, MUMU_CAPABILITY_PROVIDER_ID,
    MUMU_MANAGER_COMMAND_TIMEOUT, MUMU_MANAGER_CONTROL_TIMEOUT, MUMU_MANAGER_MINIMUM_VERSION,
    MUMU_MANAGER_STATE_WAIT_START, MUMU_MANAGER_STATE_WAIT_STOP, MumuDiscoveryReport,
    MumuEmulatorCapabilityBackend, MumuManagerSource, MumuManagerVersion, ResolvedMumuManager,
    control_instance, discover_mumu_instances, mumu_capability_profile, mumu_state_wait,
    query_instances, query_version, read_instance_state, resolve_mumu_manager,
};
pub use nemu_diagnostic::*;
pub use replay::*;
pub use touch::*;
pub use vendor_stdio::vendor_stdio_session_diagnostic;
pub use vendor_stdio_facts::*;
