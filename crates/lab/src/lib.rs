// SPDX-License-Identifier: AGPL-3.0-only

//! ActingCommand application core.
//!
//! The CLI owns parsing, presentation, and process exit codes. This crate owns
//! typed use cases, application state, and injected effect boundaries.

#![forbid(unsafe_code)]
#![allow(clippy::result_large_err)]

mod context;
mod env_api;
mod env_detection;
mod ports;
mod projection;
mod readonly;
mod readonly_api;
mod state;

pub use context::*;
pub use env_api::*;
pub use env_detection::*;
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
