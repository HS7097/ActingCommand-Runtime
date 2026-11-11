// SPDX-License-Identifier: AGPL-3.0-only

//! The program whose command lines this crate's commands serve (Workflow #434 LAB-2; register
//! §1.287, LAB-2 row; `contracts/lab-two-entries.md` §2.4). The commands LAB-1 moved here connect
//! as actingctl does, on the CLI channel with its envelope (Cli, Cli), unless the program says, once
//! at its start, that it is actinglab: actinglab's CLI module then serves them on the Lab channel
//! with the envelope (Lab, Lab), as every actinglab connection binds today. actingctl never says
//! it, so its connections stay as they were (it stays until LAB-7); LAB-6 fixes the envelope by
//! the channel in the runtime client.
//!
//! Workflow #434 LAB-5G1: how a row's serving code reaches the Runtime is its caller's
//! ([`Connection`]): a connection made for the call as a front connects, as every command line does
//! ([`Connection::per_call`]), or a connection the caller keeps.

use actingcommand_contract::{EventActor, EventSource};
use actingcommand_runtime_client::{
    RuntimeChannel, RuntimeClient, RuntimeClientConfig, RuntimeClientError,
};
use std::ops::Deref;
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
    config(current(), state_root)
}

/// Workflow #434 LAB-5G1: the configuration of a connection of `front` to the Runtime of
/// `state_root` ([`client_config`] for the program in force).
pub fn config(front: Front, state_root: impl Into<PathBuf>) -> RuntimeClientConfig {
    match front {
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

/// Workflow #434 LAB-5G1: how the code that serves a row reaches the Runtime, as its caller says.
#[derive(Clone, Copy)]
pub enum Connection<'a> {
    /// A connection made for this call as `Front` connects ([`config`]), closed when the call
    /// ends.
    Connect(Front),
    /// This connection, which the caller keeps: used as it is, neither closed nor made again here.
    Kept(&'a RuntimeClient),
}

impl<'a> Connection<'a> {
    /// A connection made for each call as the program in force connects, as every command line
    /// connects.
    pub fn per_call() -> Self {
        Self::Connect(current())
    }

    /// The connection of one call to the Runtime of `state_root`: one made with `configure`
    /// applied to its front's configuration, or the kept one, which `configure` does not touch.
    pub fn reach(
        self,
        state_root: impl Into<PathBuf>,
        configure: impl FnOnce(RuntimeClientConfig) -> RuntimeClientConfig,
    ) -> Result<Reached<'a>, RuntimeClientError> {
        match self {
            Self::Connect(front) => {
                RuntimeClient::connect(configure(config(front, state_root))).map(Reached::Made)
            }
            Self::Kept(client) => Ok(Reached::Kept(client)),
        }
    }
}

/// Workflow #434 LAB-5G1: the connection of one call ([`Connection::reach`]).
pub enum Reached<'a> {
    /// Made for this call, and dropped with it.
    Made(RuntimeClient),
    /// The caller's.
    Kept(&'a RuntimeClient),
}

impl Reached<'_> {
    /// The call's client as a value: the one made for it, or a clone of the kept one, which the
    /// caller still holds, so that dropping the clone closes nothing.
    pub fn into_client(self) -> RuntimeClient {
        match self {
            Self::Made(client) => client,
            Self::Kept(client) => client.clone(),
        }
    }
}

impl Deref for Reached<'_> {
    type Target = RuntimeClient;

    fn deref(&self) -> &RuntimeClient {
        match self {
            Self::Made(client) => client,
            Self::Kept(client) => client,
        }
    }
}
