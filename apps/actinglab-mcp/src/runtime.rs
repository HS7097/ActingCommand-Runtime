// SPDX-License-Identifier: AGPL-3.0-only

//! Where this server acts and how it reaches the Runtime (Workflow #434 LAB-5G2b; register
//! §1.287, "Slice LAB-5G2b"; `contracts/lab-two-entries.md` §9.3; the coordinator's rulings 11 and
//! 12, board #434 6775192250: until LAB-6 the new server looks, on the wire and in the ledger, like
//! today's).
//!
//! - **The place** ([`place`]): as actinglab finds its own (`apps/actinglab/src/cli_module/place.rs`):
//!   the root from the program's place, the state root from the selected configuration, or from
//!   `ACTINGCOMMAND_RUNTIME_STATE_ROOT` when it is set.
//! - **One kept connection** ([`Runtime::kept`]), `lab_ops::front`'s `Front::Mcp` (today's
//!   envelope, (Cli, Cli); Workflow #434 LAB-6S2-B1: on the Lab channel with `Lab-MCP`, where it
//!   was the MCP channel, which the Runtime still records), made again as today's server makes its own
//!   (`apps/actingctl/src/mcp/runtime.rs`, `connect` and `failure`): before it is used again,
//!   `Health` must answer the owner epoch it was opened on, else it is dropped and a new one is
//!   made, the generation counted; one the client latched a failure on is dropped after the call
//!   ([`Runtime::settle`]). The cursors the serving code issues bind to this server's session and
//!   that generation.
//! - **A write** ([`begin_write`]) goes on a connection of its own, under a correlation of its
//!   own (`begin_interaction`). Until Workflow #434 LAB-6S4a it went after the provenance today's
//!   server recorded before a write (the identity card `{client: "actingctl-mcp"}`, then
//!   `RecordClientAction{surface_id: "mcp", control_id: <tool>, kind: command}` naming the
//!   instance it resolved by one status read on the kept connection); since LAB-6S4a (0.12.62;
//!   the coordinator's ruling N8 on board #434) the write is the first request after the
//!   handshake, the initiator `Lab-MCP` its binding carries naming where it came from. Workflow
//!   #434 LAB-5G4b, second part: the client action after a capture, its value the Lab request's
//!   id, is recorded on a connection of its own ([`record_capture_action`], `crate::after_child`),
//!   since LAB-6S4a without the identity card before it.

use crate::lock;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_contract::{ClientActionKind, ClientActionRecord, ClientActionValue};
use actingcommand_lab_ops::cli_line::{self, Exit};
use actingcommand_lab_ops::core_ops::Place;
use actingcommand_lab_ops::events;
use actingcommand_lab_ops::front::{self, Front};
use actingcommand_lab_ops::locate;
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig, RuntimeClientError};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The variable that names the state root when it is set, as actinglab reads it (Workflow #434
/// LAB-5G4b: and as this server sets it for its `serve` children, `crate::children`).
pub(crate) const STATE_ROOT_VARIABLE: &str = "ACTINGCOMMAND_RUNTIME_STATE_ROOT";
/// The developer text of a state root this server could not locate.
pub(crate) const UNLOCATED: &str = "actinglab-mcp finds its root from its own place and the \
     state root from the selected configuration: run the installed actinglab-mcp, or name the \
     state root with ACTINGCOMMAND_RUNTIME_STATE_ROOT";
/// The surface of every client action this server records, today's server's.
const MCP_SURFACE: &str = "mcp";

/// A step that ends the call, with the call's line.
pub(crate) type Ended = Box<Exit>;

/// This process's place, as actinglab finds its own.
pub(crate) fn place() -> Place {
    let located = locate::locate();
    let root = located.as_ref().ok().map(|location| location.root.clone());
    if let Some(state_root) =
        std::env::var_os(STATE_ROOT_VARIABLE).filter(|value| !value.is_empty())
    {
        return Place {
            state_root: Ok(PathBuf::from(state_root)),
            root,
        };
    }
    Place {
        state_root: located.map(|location| location.state_root),
        root,
    }
}

/// The configuration of a connection of this server: `Front::Mcp`'s, in the detail
/// `ACTINGCOMMAND_DETAIL` asks for, as the serving code configures a connection it makes
/// (`lab_ops::core_ops`, `connect`).
fn config(state_root: &Path) -> RuntimeClientConfig {
    let config = front::config(Front::Mcp, state_root);
    match cli_line::detail() {
        Ok(Detail::Codes) => config.with_detail(Detail::Codes),
        _ => config,
    }
}

/// A Runtime client failure as the serving code answers it (`lab_ops::core_ops`,
/// `client_failed`): its outcome refused, its text the developer line's.
fn client_failed(error: &RuntimeClientError) -> Ended {
    Box::new(cli_line::refused(
        &error.outcome(),
        Some(&error.to_string()),
    ))
}

/// This server's Runtime connection.
pub(crate) struct Runtime {
    /// This server process's identity, which binds the cursors the serving code issues.
    session: u64,
    slot: Mutex<Slot>,
}

#[derive(Default)]
struct Slot {
    client: Option<RuntimeClient>,
    /// The state root the kept connection reaches.
    state_root: Option<PathBuf>,
    /// Counts connections; a cursor names the one it was issued on.
    generation: u64,
}

/// The kept connection, and its generation.
pub(crate) struct Kept {
    pub(crate) client: RuntimeClient,
    pub(crate) generation: u64,
}

impl Runtime {
    pub(crate) fn new() -> Self {
        Self {
            session: events::process_session(),
            slot: Mutex::new(Slot::default()),
        }
    }

    /// This server process's identity.
    pub(crate) const fn session(&self) -> u64 {
        self.session
    }

    /// The generation of the connection kept now (0 before the first).
    pub(crate) fn generation(&self) -> u64 {
        lock(&self.slot).generation
    }

    /// The kept connection to the Runtime of `state_root`. One kept before is used when it
    /// reaches that state root and `Health` answers the owner epoch it was opened on (a Runtime
    /// stopped, restarted or redeployed meanwhile answers otherwise); else it is dropped and a new
    /// one is made, the next generation. A Runtime that does not answer is the client's failure.
    pub(crate) fn kept(&self, state_root: &Path) -> Result<Kept, RuntimeClientError> {
        let mut slot = lock(&self.slot);
        let same_root = slot.state_root.as_deref() == Some(state_root);
        if let Some(client) = slot.client.take()
            && same_root
            && client
                .health()
                .is_ok_and(|epoch| epoch == client.runtime_info().owner_epoch())
        {
            slot.client = Some(client.clone());
            return Ok(Kept {
                client,
                generation: slot.generation,
            });
        }
        let client = RuntimeClient::connect(config(state_root))?;
        slot.generation += 1;
        slot.client = Some(client.clone());
        slot.state_root = Some(state_root.to_path_buf());
        Ok(Kept {
            client,
            generation: slot.generation,
        })
    }

    /// After a call on `kept`: a connection the client has latched a failure on is dropped, so
    /// that the next call connects again (today's `failure`). `begin_interaction` only mints a
    /// correlation locally; on a latched connection it answers that failure without any IO.
    pub(crate) fn settle(&self, kept: &Kept) {
        if kept.client.begin_interaction().is_err() {
            let mut slot = lock(&self.slot);
            if slot.generation == kept.generation {
                slot.client = None;
            }
        }
    }
}

// Workflow #434 LAB-6S4a (0.12.62): `Runtime::alias`, the status read that named a write's
// instance in its client action, went with that client action.

/// A connection of its own to the Runtime of `state_root` for one write, under a correlation of
/// its own (`begin_interaction`). Workflow #434 LAB-6S4a (0.12.62; the coordinator's ruling N8 on
/// board #434): the identity card and the client action that came before the write are gone, the
/// initiator `Lab-MCP` the connection's binding carries names where it came from, so the write is
/// the first request after the handshake. A connection that fails ends the call with its line,
/// as the serving code answers a client's failure, and nothing is written.
pub(crate) fn begin_write(state_root: &Path) -> Result<RuntimeClient, Ended> {
    RuntimeClient::connect(config(state_root))
        .and_then(|connection| connection.begin_interaction())
        .map_err(|error| client_failed(&error))
}

/// The client action after a capture that was not recorded (Workflow #434 LAB-5G4b, second part:
/// the client action after a capture reads its outcome).
pub(crate) enum NotRecorded {
    /// The Runtime client's failure: to connect, to begin the interaction, or the client action
    /// refused.
    Client(Box<RuntimeClientError>),
    /// A client action the contract does not build (`client_action_invalid`).
    Invalid,
}

impl NotRecorded {
    fn client(error: RuntimeClientError) -> Self {
        Self::Client(Box::new(error))
    }

    /// Its outcome: the client's, or `client_action_invalid`.
    pub(crate) fn outcome(&self) -> Outcome {
        match self {
            Self::Client(error) => error.outcome(),
            Self::Invalid => Outcome::new(SharedCode::ClientActionInvalid),
        }
    }
}

/// The client action after a capture (Workflow #434 LAB-5G4b, second part; today's
/// `record_action`), on a connection of its own to the Runtime of `state_root`:
/// `begin_interaction`, then the client action of the tool `tool` naming `instance_alias` and
/// carrying `value` when given (the Lab request's id). Workflow #434 LAB-6S4a (0.12.62): no
/// identity card is declared before it. A failure is the step's ([`NotRecorded`]), and nothing
/// more is sent.
pub(crate) fn record_capture_action(
    state_root: &Path,
    tool: &str,
    instance_alias: Option<&str>,
    value: Option<ClientActionValue>,
) -> Result<(), NotRecorded> {
    let connection = RuntimeClient::connect(config(state_root)).map_err(NotRecorded::client)?;
    let client = connection
        .begin_interaction()
        .map_err(NotRecorded::client)?;
    let action = ClientActionRecord::new(
        MCP_SURFACE,
        tool,
        ClientActionKind::Command,
        instance_alias.map(str::to_owned),
        value,
    )
    .map_err(|_| NotRecorded::Invalid)?;
    client
        .record_client_action(action)
        .map_err(NotRecorded::client)?;
    Ok(())
}
