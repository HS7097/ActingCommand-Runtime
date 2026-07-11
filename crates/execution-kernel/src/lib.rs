// SPDX-License-Identifier: AGPL-3.0-only

//! Daemon-owned execution sessions plus pure task and probe decision planning.
//!
//! Clients never receive backend objects. The resident Runtime owns this kernel and invokes it
//! only after scheduler admission and fencing.

#![forbid(unsafe_code)]

mod bundle;
mod drive;
mod environment;
mod error;
mod kernel;
mod planning;
mod provider;
mod readonly;
mod recovery;
mod run;
mod session;

pub use bundle::*;
pub use drive::*;
pub use environment::*;
pub use error::*;
pub use kernel::*;
pub use planning::*;
pub use provider::*;
pub use readonly::*;
pub use recovery::*;
pub use run::*;
pub use session::*;

#[cfg(test)]
mod tests;
