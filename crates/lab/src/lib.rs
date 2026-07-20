// SPDX-License-Identifier: AGPL-3.0-only

//! Optional ActingCommand Lab authoring and debug adapter.
//!
//! Production Runtime, scheduler, device, and ledger ownership live outside this
//! crate. Lab consumes stable contracts and injected ports so production remains
//! buildable and runnable when this crate is excluded.

#![forbid(unsafe_code)]
#![allow(clippy::result_large_err)]

mod context;
mod drive;
mod drive_api;
mod env_api;
mod env_detection;
mod lab_run;
mod lab_run_api;
mod ledger_port;
mod maa_task_graph;
mod package_api;
mod package_build;
mod package_validate;
mod ports;
mod projection;
mod readonly;
mod readonly_api;
mod resource_convert;
mod state;

pub use context::*;
pub use drive::*;
pub use drive_api::*;
pub use env_api::*;
pub use env_detection::*;
pub use lab_run::{target_evaluations_stable_for_wait, validate_lab_package_bytes};
pub use lab_run_api::*;
pub use ledger_port::*;
pub use maa_task_graph::{MaaTaskGraph, MaaTaskGraphStats, compile_maa_task_graph};
pub use package_api::*;
pub use package_build::PackageBuildCatalog;
pub use ports::*;
pub use projection::*;
pub use readonly_api::*;
pub use state::*;

pub use actingcommand_artifact_store::{FrameStoreControl, MemorySample, MemorySampleSource};
pub use actingcommand_contract::{LabError, LabResult};
pub use actingcommand_execution_kernel::{
    DetectKind, OfflineSimulationError, OfflineSimulationResult, PreparedContainedTask,
    RecoveryAction, RecoveryExecError, RecoveryExecutionReport, RecoveryGraph, RecoveryNode,
    RecoveryResult, RecoveryRuntime, RecoverySignal, RecoveryStatus, execute_recovery_graph,
    simulate_contained_task,
};

pub struct Lab<P: LabPorts> {
    ports: P,
    state: LabState,
}

impl<P: LabPorts> Lab<P> {
    pub fn new(ports: P, state: LabState) -> LabResult<Self> {
        Ok(Self { ports, state })
    }

    pub fn ports(&self) -> &P {
        &self.ports
    }

    pub(crate) fn ports_mut(&mut self) -> &mut P {
        &mut self.ports
    }

    pub fn state(&self) -> &LabState {
        &self.state
    }
}
