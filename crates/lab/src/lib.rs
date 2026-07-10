// SPDX-License-Identifier: AGPL-3.0-only

//! ActingCommand application core.
//!
//! The CLI owns parsing, presentation, and process exit codes. This crate owns
//! typed use cases, application state, and injected effect boundaries.

#![forbid(unsafe_code)]
#![allow(clippy::result_large_err)]

mod context;
mod drive;
mod drive_api;
mod env_api;
mod env_detection;
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
pub use maa_task_graph::{MaaTaskGraph, MaaTaskGraphStats, compile_maa_task_graph};
pub use package_api::*;
pub use package_build::PackageBuildCatalog;
pub use ports::*;
pub use projection::*;
pub use readonly_api::*;
pub use state::*;

pub use actingcommand_contract::{LabError, LabResult};

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

    pub fn state(&self) -> &LabState {
        &self.state
    }
}
