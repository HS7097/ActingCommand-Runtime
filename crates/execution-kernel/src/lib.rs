// SPDX-License-Identifier: AGPL-3.0-only

//! Daemon-owned execution sessions plus pure task and probe decision planning.
//!
//! Clients never receive backend objects. The resident Runtime owns this kernel and invokes it
//! only after scheduler admission and fencing.

#![forbid(unsafe_code)]

mod bundle;
mod contained_task;
mod decision;
mod drive;
mod environment;
mod error;
mod kernel;
mod monitor;
mod offline;
mod planning;
mod provider;
mod readonly;
mod recovery;
mod run;
mod session;

pub use bundle::*;
pub use contained_task::*;
pub use decision::*;
pub use drive::*;
pub use environment::*;
pub use error::*;
pub use kernel::*;
pub use monitor::*;
pub use offline::*;
pub use planning::*;
pub use provider::*;
pub use readonly::*;
pub use recovery::*;
pub use run::*;
pub use session::*;

// The execution kernel is the sole production package-ingress capability. Downstream Lab and
// ActingLab code receives only closed admitted types and non-executable hash/limit metadata;
// raw ZIP loaders and entry-byte access stay in pack-containment behind this crate boundary.
pub use actingcommand_pack_containment::{
    AdmittedAction, AdmittedEffectCapability, AdmittedGuard, AdmittedOperation, AdmittedPackage,
    AdmittedTask, BoundedRect, ContainmentError, ContainmentLimits, GuardVerification,
    OpaqueMetadata, PageKey, PageSelector, Sha256Hash, TargetTapMode,
};

#[cfg(test)]
mod tests;
