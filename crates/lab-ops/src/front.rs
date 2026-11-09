// SPDX-License-Identifier: AGPL-3.0-only

//! The program whose command lines this crate's commands serve (Workflow #434 LAB-2; register
//! §1.287, LAB-2 row; `contracts/lab-two-entries.md` §2.4). The commands LAB-1 moved here connect
//! as actingctl does, on the CLI channel with its envelope (Cli, Cli), unless the program says, once
//! at its start, that it is actinglab: actinglab's CLI module then serves them on the Lab channel
//! with the envelope (Lab, Lab), as every actinglab connection binds today. actingctl never says
//! it, so its connections stay as they were (it stays until LAB-7); LAB-6 fixes the envelope by
//! the channel in the runtime client.

use actingcommand_contract::{EventActor, EventSource};
use actingcommand_runtime_client::{RuntimeChannel, RuntimeClientConfig};
use std::path::PathBuf;
use std::sync::OnceLock;

/// The program that runs this crate's commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Front {
    /// `actingctl`: the CLI channel, envelope (Cli, Cli).
    Actingctl,
    /// `actinglab`'s CLI module (LAB-2): the Lab channel, envelope (Lab, Lab).
    Actinglab,
}

static FRONT: OnceLock<Front> = OnceLock::new();

/// Says which program runs this process's commands; the first call decides and later ones change
/// nothing. Returns the program in force.
pub fn set(front: Front) -> Front {
    *FRONT.get_or_init(|| front)
}

/// The program in force: actingctl unless [`set`] said otherwise.
pub fn current() -> Front {
    FRONT.get().copied().unwrap_or(Front::Actingctl)
}

/// The configuration of a connection to the Runtime of `state_root` for the program in force.
pub fn client_config(state_root: impl Into<PathBuf>) -> RuntimeClientConfig {
    match current() {
        Front::Actingctl => RuntimeClientConfig::new(
            state_root,
            RuntimeChannel::Cli,
            EventActor::Cli,
            EventSource::Cli,
        ),
        Front::Actinglab => RuntimeClientConfig::new(
            state_root,
            RuntimeChannel::Lab,
            EventActor::Lab,
            EventSource::Lab,
        ),
    }
}
