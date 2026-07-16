// SPDX-License-Identifier: AGPL-3.0-only

//! Resident Runtime ownership, local IPC, lease-gated DeviceProxy, and lifecycle control.
//!
//! The UI and other clients do not own this process. They communicate through the typed
//! loopback API, and upstream/device implementations stay behind the backend-provider boundary.

#![forbid(unsafe_code)]

mod error;
mod events;
mod host;
mod ipc;
mod monitor;
mod owner;
mod performance;
mod policy_control;
mod policy_host;
mod provider;
mod time;

pub use error::*;
pub use host::*;
pub use performance::{PerformanceMonitorConfig, PipelinePerformanceSignal};
pub use policy_control::PolicyExecutionInput;
pub use policy_host::{
    CatalogGeneration, PolicyAdmissionContext, PolicyCadence, PolicyCycle, PolicyDispatchAdmission,
    PolicyRecomputeDirective, PolicyRecomputeKind, PolicyRecomputeReason, PolicyTrigger,
};
pub use provider::*;

#[cfg(test)]
mod tests;
