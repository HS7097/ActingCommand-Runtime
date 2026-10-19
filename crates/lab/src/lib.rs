// SPDX-License-Identifier: AGPL-3.0-only

//! ActingCommand Lab authoring and debug adapter, a required workspace member.
//!
//! Production Runtime, scheduler, device, and ledger ownership live outside this
//! crate. Lab consumes stable contracts and injected ports so production remains
//! buildable and runnable when this crate is excluded.
//!
//! Package inspection uses `Lab::lab_validate`, `validate_lab_package_bytes` or
//! `validate_lab_package_path`.
//! Execution clients submit contained tasks through the Runtime client; the
//! `actinglab lab run` command uses that same Runtime-owned execution path.

#![forbid(unsafe_code)]
#![allow(clippy::result_large_err)]

mod codes;
mod context;
mod drive;
mod drive_api;
mod env_api;
mod env_detection;
// Workflow #396 REC-4 (exec key 65): a Lab recording's click follows a template mark.
#[cfg(test)]
mod gate_396_rec4;
// Workflow #336 PKG-S5 (exec key 66): record stop writes `<package_id>.<ext>`, replaced in place.
#[cfg(test)]
mod gate_336_pkgs5;
// Workflow #373 NAV-3a follow-up (exec key 71): record stop prints its package's registry entry.
#[cfg(test)]
mod gate_373_nav3afu;
// Workflow #390 LAB-2 (exec key 72): forbidden and anchored pages, gestures, marks after stop.
#[cfg(test)]
mod gate_390_lab2;
// Test plan v2.2 E-5 G6 (exec key 35): record stop over a synthetic recording.
#[cfg(test)]
mod gate_e5_record_stop;
// #411 A10 (exec key 85): an undecodable Lab recording makes only the Lab part unavailable.
#[cfg(test)]
mod gate_rc2_a10;
mod lab_run;
mod lab_run_api;
mod ledger_port;
mod maa_task_graph;
mod ocr;
mod package_api;
mod package_build;
mod package_validate;
mod ports;
mod projection;
mod readonly;
mod readonly_api;
mod recording;
mod recording_api;
mod scheduling;
mod state;

pub use codes::LabCode;
pub use context::*;
pub use drive::*;
pub use drive_api::*;
pub use env_api::*;
pub use env_detection::*;
pub use lab_run::{
    target_evaluations_stable_for_wait, validate_lab_package_bytes, validate_lab_package_path,
};
pub use lab_run_api::*;
pub use ledger_port::*;
pub use maa_task_graph::{
    MaaFact, MaaFactOrigin, MaaFactSource, MaaFactValue, MaaTaskFacts, MaaTaskGraph,
    MaaTaskGraphStats, compile_maa_task_graph,
};
pub use ocr::LabOcr;
pub use package_api::*;
pub use ports::*;
pub use projection::*;
pub use readonly_api::*;
pub use recording::*;
pub use recording_api::*;
pub use scheduling::*;
pub use state::*;

pub use actingcommand_artifact_store::{FrameStoreControl, MemorySample, MemorySampleSource};
pub use actingcommand_contract::{LabError, LabResult};
pub use actingcommand_execution_kernel::{
    DetectKind, OfflineDecision, OfflinePostAdmissionOcrStatus, OfflineSimulationError,
    OfflineSimulationResult, PreparedContainedTask, RecoveryAction, RecoveryExecError,
    RecoveryExecutionReport, RecoveryGraph, RecoveryNode, RecoveryResult, RecoveryRuntime,
    RecoverySignal, RecoveryStatus, execute_recovery_graph, simulate_contained_task,
    simulate_contained_task_at,
};
// Workflow #308 SEL-L6a: the whole-package replay of `actinglab replay --frames`.
pub use actingcommand_execution_kernel::{
    OfflineReplayEnd, OfflineReplayError, OfflineReplayFacts, OfflineReplayResult,
    replay_contained_task,
};
// Workflow #308 SEL-L6b: the scan replay of `actinglab replay --frames --select --scan`.
pub use actingcommand_execution_kernel::{ScanDeclaration, ScanDryRun, replay_scan};

pub use actingcommand_execution_kernel::{
    DriveNavigationGraph, parse_environment_catalog_value, validate_control_declaration,
};
// Workflow #390 LAB-2 item 7: the box an `offset` navigation click presses, for actinglab.
pub use actingcommand_execution_kernel::drive_offset_rect;

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
