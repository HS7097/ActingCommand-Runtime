// SPDX-License-Identifier: AGPL-3.0-only

//! Resident Runtime ownership, local IPC, lease-gated DeviceProxy, and lifecycle control.
//!
//! The UI and other clients do not own this process. They communicate through the typed
//! loopback API, and upstream/device implementations stay behind the backend-provider boundary.

#![forbid(unsafe_code)]

mod agent_dispatcher;
mod approval;
mod emulator_control;
mod error;
mod events;
mod fact_store;
mod host;
mod ipc;
mod monitor;
mod owner;
mod performance;
mod performance_control;
mod planning;
mod policy_control;
mod policy_host;
mod project_interface;
mod proposal;
mod provider;
mod strategy;
mod time;

pub use strategy::StrategicPlanPreparation;
pub use time::{RuntimeClock, RuntimeClockSample, SystemRuntimeClock};

pub use agent_dispatcher::AgentDispatcherConfig;
pub use emulator_control::admit_emulator_capabilities;
pub use error::*;
pub use host::*;
pub use performance::{PerformanceMonitorConfig, PipelinePerformanceSignal};
pub use performance_control::{
    PerformanceControlConfig, PerformanceControlDirective, PerformanceControlObservation,
    PerformanceControlWorkload,
};
pub use planning::MaintenanceLedgerQuery;
pub use policy_control::PolicyExecutionInput;
pub use policy_host::{
    CatalogGeneration, PolicyAdmissionContext, PolicyCadence, PolicyCycle, PolicyDispatchAdmission,
    PolicyEvaluationCost, PolicyEvaluationExecution, PolicyEvaluationMeasurement,
    PolicyRecomputeDirective, PolicyRecomputeKind, PolicyRecomputeReason, PolicyTrigger,
};
pub use provider::*;

#[cfg(test)]
mod tests;
