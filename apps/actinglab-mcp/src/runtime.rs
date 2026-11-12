// SPDX-License-Identifier: AGPL-3.0-only

//! Where this server acts and how it reaches the Runtime (Workflow #434 LAB-5G2b; register
//! §1.287, "Slice LAB-5G2b"; `contracts/lab-two-entries.md` §9.3; the coordinator's rulings 11 and
//! 12, board #434 6775192250: until LAB-6 the new server looks, on the wire and in the ledger, like
//! today's).
//!
//! - **The place** ([`place`]): as actinglab finds its own (`apps/actinglab/src/cli_module/place.rs`):
//!   the root from the program's place, the state root from the selected configuration, or from
//!   `ACTINGCOMMAND_RUNTIME_STATE_ROOT` when it is set.
//! - **One kept connection** ([`Runtime::kept`]), `lab_ops::front`'s `Front::Mcp` (the MCP channel
//!   with today's envelope, (Cli, Cli)), made again as today's server makes its own
//!   (`apps/actingctl/src/mcp/runtime.rs`, `connect` and `failure`): before it is used again,
//!   `Health` must answer the owner epoch it was opened on, else it is dropped and a new one is
//!   made, the generation counted; one the client latched a failure on is dropped after the call
//!   ([`Runtime::settle`]). The cursors the serving code issues bind to this server's session and
//!   that generation.
//! - **A write** ([`begin_write`]) goes on a connection of its own, after the provenance today's
//!   server records before a write (`apps/actingctl/src/mcp/operator.rs`, `record_provenance`):
//!   `begin_interaction`, the identity card `{client: "actingctl-mcp"}`, then
//!   `RecordClientAction{surface_id: "mcp", control_id: <tool>, kind: command}` naming the
//!   instance, all under that one correlation, and the write after them. The instance is
//!   resolved as today's server resolves it, by one status read on the kept connection
//!   ([`Runtime::alias`]).

use crate::codes::McpCode;
use crate::lock;
use actingcommand_contract::codes::{GovernanceIdentityRefusal, SharedCode};
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_contract::{ClientActionKind, ClientActionRecord, GovernanceIdentityCard};
use actingcommand_lab_ops::cli_line::{self, Exit};
use actingcommand_lab_ops::core_ops::Place;
use actingcommand_lab_ops::events;
use actingcommand_lab_ops::front::{self, Front};
use actingcommand_lab_ops::instance_selector::{self, SelectorMiss};
use actingcommand_lab_ops::locate;
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig, RuntimeClientError};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The variable that names the state root when it is set, as actinglab reads it.
const STATE_ROOT_VARIABLE: &str = "ACTINGCOMMAND_RUNTIME_STATE_ROOT";
/// The developer text of a state root this server could not locate.
pub(crate) const UNLOCATED: &str = "actinglab-mcp finds its root from its own place and the \
     state root from the selected configuration: run the installed actinglab-mcp, or name the \
     state root with ACTINGCOMMAND_RUNTIME_STATE_ROOT";
/// The surface of every client action this server records, today's server's.
const MCP_SURFACE: &str = "mcp";
/// The client the identity card names, today's server's (ruling 11: the same identity card).
const MCP_CLIENT: &str = "actingctl-mcp";

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

    /// The alias of the instance `selector` names (an alias, an `instance_id` or an ADB port),
    /// for a write's client action: one status read on the kept connection, as today's server
    /// resolves it. A Runtime that does not answer, a read that fails and a selector that names no
    /// instance, or more than one, end the call with the line the serving code gives each, and
    /// nothing is written.
    pub(crate) fn alias(&self, state_root: &Path, selector: &str) -> Result<String, Ended> {
        let kept = self
            .kept(state_root)
            .map_err(|error| client_failed(&error))?;
        let read = kept.client.status();
        self.settle(&kept);
        let status = read.map_err(|error| client_failed(&error))?;
        instance_selector::select(status.instances(), selector)
            .map(|instance| instance.instance_alias().to_owned())
            .map_err(|miss| selector_missed(miss, selector))
    }
}

/// A selector that names no instance, or more than one, as the serving code refuses it
/// (`lab_ops::core_ops`, `selector_missed`).
fn selector_missed(miss: SelectorMiss, selector: &str) -> Ended {
    let text = match miss {
        SelectorMiss::Unknown => "the runtime status lists no instance with that alias",
        SelectorMiss::Ambiguous => {
            "more than one instance of the runtime status matches that selector"
        }
    };
    Box::new(cli_line::refused(&miss.outcome(selector), Some(text)))
}

/// One write's connection after its provenance was recorded, and the warnings recording it gave
/// (each `{outcome}`), which the line carries.
pub(crate) struct Write {
    pub(crate) client: RuntimeClient,
    pub(crate) warnings: Vec<Value>,
}

/// A connection of its own to the Runtime of `state_root` for one write of the tool `tool`, with
/// today's provenance recorded on it: `begin_interaction`, the identity card, then the client
/// action naming `instance_alias`. A card the Runtime refuses because its `allowed_clients` does
/// not list the client is no failure, as today: the client action and the write go ahead, and
/// the line carries the warning `mcp_identity_declaration_refused` caused by the refusal (where
/// today's server answers `mcp_identity_card_refused` with `mcp_blocker`, a vocabulary only
/// actingctl's server builds). Any other failure ends the call with the client's failure, and
/// nothing is written.
pub(crate) fn begin_write(
    state_root: &Path,
    tool: &str,
    instance_alias: Option<&str>,
) -> Result<Write, Ended> {
    let connection =
        RuntimeClient::connect(config(state_root)).map_err(|error| client_failed(&error))?;
    let client = connection
        .begin_interaction()
        .map_err(|error| client_failed(&error))?;
    let mut warnings = Vec::new();
    let card = GovernanceIdentityCard {
        client: MCP_CLIENT.to_owned(),
        client_version: None,
        instance: None,
    };
    if let Err(error) = client.declare_governance_identity(&card) {
        let refusal = error.outcome();
        if !refusal.is(GovernanceIdentityRefusal::ClientNotAllowed) {
            return Err(client_failed(&error));
        }
        warnings.push(cli_line::warning(
            &Outcome::new(McpCode::IdentityDeclarationRefused).caused_by(refusal),
        ));
    }
    let action = ClientActionRecord::new(
        MCP_SURFACE,
        tool,
        ClientActionKind::Command,
        instance_alias.map(str::to_owned),
        None,
    )
    .map_err(|_| {
        Box::new(cli_line::failure(
            &Outcome::new(SharedCode::ClientActionInvalid),
            None,
        ))
    })?;
    client
        .record_client_action(action)
        .map_err(|error| client_failed(&error))?;
    Ok(Write { client, warnings })
}
