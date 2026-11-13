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
//!
//! Workflow #434 LAB-5G1b: a third front, [`Front::Mcp`], the MCP channel with the envelope
//! today's MCP server uses, (Cli, Cli): the Lab's MCP server connects so until LAB-6 (the
//! coordinator's rulings 11 and 12 on board #434, comment 6775192250). No program says it yet.
//!
//! Workflow #434 LAB-6S2-B1 (`contracts/lab-two-entries.md` §2.4, §4.2; register §1.287, "Slice
//! LAB-6S2-B1"; the coordinator's rulings of 2026-11-13 on board #434, not the owner's words): the
//! Lab's own programs bind the Lab channel with their initiator word and keep the envelope they
//! sent on 0.12.58. actinglab's front carries the word of its process ([`Front::Actinglab`]), and
//! the Lab's MCP server's binds `lab` with `Lab-MCP` and keeps (Cli, Cli) ([`Front::Mcp`]); the
//! Runtime records each request with the channel that front had on 0.12.58 (the map M of
//! LAB-6S2-M). actingctl's front stays on the CLI channel without a word until LAB-6S2-B2.

use crate::initiator::Initiator;
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
    /// `actinglab`'s CLI module (LAB-2): the Lab channel, envelope (Lab, Lab). Workflow #434
    /// LAB-6S2-B1: with the word of the process (§4.2): `Lab-CLI`, the `Lab-制包` or `Lab-救急` of
    /// `ACTINGCOMMAND_INITIATOR`, `Lab-MCP` under `serve --stdio --front mcp`.
    Actinglab(Initiator),
    /// Workflow #434 LAB-5G1b: the Lab's MCP server, with the envelope (Cli, Cli), as
    /// `actingctl mcp-serve` sends it (`apps/actingctl/src/mcp/runtime.rs`). Workflow #434
    /// LAB-6S2-B1: on the Lab channel with `Lab-MCP`, where it was the MCP channel.
    Mcp,
}

impl Front {
    /// Workflow #434 LAB-6S2-B1: the word a connection of this front declares: none for
    /// actingctl's (until LAB-6S2-B2), the process's for actinglab's, `Lab-MCP` for the Lab's MCP
    /// server's.
    pub const fn initiator(self) -> Option<Initiator> {
        match self {
            Self::Actingctl => None,
            Self::Actinglab(initiator) => Some(initiator),
            Self::Mcp => Some(Initiator::LabMcp),
        }
    }
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
        Front::Actinglab(initiator) => RuntimeClientConfig::new(
            state_root,
            RuntimeChannel::Lab,
            EventActor::Lab,
            EventSource::Lab,
        )
        .with_initiator(initiator.word()),
        Front::Actingctl | Front::Mcp => cli_envelope(state_root, front.initiator()),
    }
}

/// Workflow #434 LAB-6S2-B1: the configuration of a connection that sends the CLI's envelope
/// (Cli, Cli) to the Runtime of `state_root`: without a word on the CLI channel, as every such
/// connection bound on 0.12.58; with `initiator`, one of the five Lab words, on the Lab channel
/// with it. The Lab's MCP server ([`Front::Mcp`]), the rescue module's own client and the
/// watchdog's probe connect so with their word; actingctl, whose `install`, `start`, `restart`,
/// `watchdog` and `overview` run the same code, without one until LAB-6S2-B2.
pub fn cli_envelope(
    state_root: impl Into<PathBuf>,
    initiator: Option<Initiator>,
) -> RuntimeClientConfig {
    match initiator {
        None => RuntimeClientConfig::new(
            state_root,
            RuntimeChannel::Cli,
            EventActor::Cli,
            EventSource::Cli,
        ),
        Some(initiator) => RuntimeClientConfig::new(
            state_root,
            RuntimeChannel::Lab,
            EventActor::Cli,
            EventSource::Cli,
        )
        .with_initiator(initiator.word()),
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
