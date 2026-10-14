// SPDX-License-Identifier: AGPL-3.0-only

//! Installation inputs are pinned at process startup (#352), except where a tool starts a child
//! of the install or reads its configuration: those read the generation selected at the call
//! (`locate_now`, Workflow #389 MCP-7 scope extension). The first tool call connects
//! with `RuntimeClient::connect`, which checks the owner epoch against `runtime-info.json`.
//! A connection the client has locked after a failure is dropped, and the next call
//! connects again; cursors issued on the old connection then answer `cursor_invalid`.
//!
//! Workflow #378 A5: every location and connection failure is an outcome. An installation that
//! cannot be located is its own condition (`install_process_root_mismatch`,
//! `install_process_state_root_mismatch` or the installation selection's code); a state root
//! that cannot be told is `install_state_root_unresolved`; an actingd that does not answer is
//! `runtime_unavailable` caused by the client's outcome.

use super::lock;
use super::tools::{McpBlocker, ToolError};
use crate::codes::ActingctlCode;
use actingcommand_contract::RuntimeErrorCode;
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{IoOp, Outcome, RawSource};
use actingcommand_contract::{EventActor, EventSource, InstanceId};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig, RuntimeClientError};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// `<root>\actingd.config.json` is read whole up to this size.
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

pub(super) struct RuntimeAccess {
    /// Train 4 review ruling on MCP-11: the `--root` this server was started with, as given;
    /// `ac_packs` finds the selected configuration from it at every call, as `actingctl packs`
    /// does from its own `--root`.
    requested_root: Option<PathBuf>,
    root: Option<PathBuf>,
    install_root: Option<PathBuf>,
    config: Option<PathBuf>,
    installation: Option<actingcommand_contract::InstalledProcess>,
    location_error: Option<Outcome>,
    state_root: Option<PathBuf>,
    slot: Mutex<Slot>,
}

#[derive(Default)]
struct Slot {
    client: Option<RuntimeClient>,
    /// Counts connections; a cursor names the one it was issued on.
    generation: u64,
    /// Workflow #389 MCP-1 (MCP-0 command table §2.5): the instance selectors this connection
    /// resolved, to their instance_id; a new connection starts without any.
    instances: HashMap<String, InstanceId>,
}

/// Where the install and the Runtime state live, as found at one call.
#[derive(Clone)]
pub(super) struct Location {
    /// The program root: the selected slot `<root>\<A|B>`, or the install root of a
    /// layout without slots.
    pub(super) root: Option<PathBuf>,
    /// The install root that holds `tools\` (Workflow #359); the program root without slots.
    pub(super) install_root: Option<PathBuf>,
    pub(super) config: Option<PathBuf>,
    pub(super) installation: Option<actingcommand_contract::InstalledProcess>,
    error: Option<Outcome>,
    /// The state root, or `install_state_root_unresolved` with what kept it unknown.
    pub(super) state_root: Result<PathBuf, Outcome>,
}

impl Location {
    /// The installation could not be located at startup: that condition, class usage.
    pub(super) fn check(&self) -> Result<(), ToolError> {
        match &self.error {
            Some(error) => Err(ToolError::outcome("usage", error.clone())),
            None => Ok(()),
        }
    }
}

// Workflow #389 MCP-7: shared with `actingctl diagnose`.
pub(super) use crate::child::pin_child;
// Workflow #389 MCP-8: the bounded file read, shared with `actingctl overview`.
pub(super) use crate::overview::read_bounded;

/// The control connection with (Cli, Cli) origin, as actingctl itself uses.
pub(super) struct Connected {
    pub(super) client: RuntimeClient,
    pub(super) generation: u64,
}

impl RuntimeAccess {
    pub(super) fn new(root: Option<PathBuf>, state_root: Option<PathBuf>) -> Self {
        let requested_root = root.clone();
        let located = (|| {
            let process = actingcommand_contract::process_installation()
                .map_err(|error| Outcome::new(error.code()))?;
            let installation = match process {
                Some(selected) => {
                    if let Some(root) = &root {
                        // A --root that is not the root this process runs from, readable or not.
                        let mismatch = || {
                            Outcome::new(ActingctlCode::InstallProcessRootMismatch)
                                .path(root)
                                .install_root(selected.root())
                        };
                        let canonical = fs::canonicalize(root).map_err(|error| {
                            mismatch().caused_by(
                                Outcome::from_io_error(&error, IoOp::Canonicalize).path(root),
                            )
                        })?;
                        if canonical != selected.root() {
                            return Err(mismatch());
                        }
                    }
                    Some(selected.clone())
                }
                None => root
                    .as_deref()
                    .map(actingcommand_contract::InstalledProcess::read)
                    .transpose()
                    .map_err(|error| Outcome::new(error.code()))?
                    .flatten(),
            };
            let program_root = installation
                .as_ref()
                .map(|selected| selected.program_root())
                .or(root)
                .or_else(executable_install_root);
            let install_root = installation
                .as_ref()
                .map(|selected| selected.root().to_path_buf())
                .or_else(|| program_root.clone());
            let config = installation
                .as_ref()
                .map(|selected| selected.config_path())
                .or_else(|| {
                    program_root
                        .as_ref()
                        .map(|root| root.join("actingd.config.json"))
                });
            if installation.is_some()
                && let Some(requested) = &state_root
            {
                let configured = configured_state_root(config.as_deref())?;
                if requested != &configured {
                    // A --state-root that is not the configuration's, readable or not.
                    let mismatch = || {
                        let outcome = Outcome::new(ActingctlCode::InstallProcessStateRootMismatch)
                            .state_root(requested)
                            .path(&configured);
                        match &config {
                            Some(config) => outcome.config_path(config),
                            None => outcome,
                        }
                    };
                    let canonical = |path: &Path| {
                        fs::canonicalize(path).map_err(|error| {
                            mismatch().caused_by(
                                Outcome::from_io_error(&error, IoOp::Canonicalize).path(path),
                            )
                        })
                    };
                    if canonical(requested)? != canonical(&configured)? {
                        return Err(mismatch());
                    }
                }
            }
            Ok::<_, Outcome>((program_root, install_root, config, installation))
        })();
        let (root, install_root, config, installation, location_error) = match located {
            Ok((root, install_root, config, installation)) => {
                (root, install_root, config, installation, None)
            }
            Err(error) => (None, None, None, None, Some(error)),
        };
        Self {
            requested_root,
            root,
            install_root,
            config,
            installation,
            location_error,
            state_root,
            slot: Mutex::new(Slot::default()),
        }
    }

    /// `locate`, with the install selection read again from `install\active.json` as it is now
    /// (Workflow #389 MCP-7 scope extension): the program root, the configuration and the
    /// selection handed to children are those of the generation selected at this call, not the
    /// one this server holds from its start (also when the fixed entry handed it that one). The
    /// state root stays the one this server reads. A selection that cannot be read answers the
    /// CLI's codes (`packs::current_selection`).
    pub(super) fn locate_now(&self) -> Result<Location, ToolError> {
        let mut location = self.locate();
        location.check()?;
        let Some(start) = &location.installation else {
            return Ok(location);
        };
        let current = crate::packs::current_selection(start.root())
            .map_err(|error| ToolError::outcome("usage", error.outcome()))?;
        location.root = Some(current.program_root());
        location.install_root = Some(current.root().to_path_buf());
        location.config = Some(current.config_path());
        location.installation = Some(current);
        Ok(location)
    }

    /// The `--root` this server was started with, as given (train 4 review ruling on MCP-11).
    pub(super) fn requested_root(&self) -> Option<&Path> {
        self.requested_root.as_deref()
    }

    /// `--root`, or two levels above the running executable when
    /// `<root>\runtime\BUILD-MANIFEST.json` is a file (as actingd's install detection);
    /// `--state-root`, or the absolute `state_root` of `<root>\actingd.config.json` (as the
    /// UI's setup reads it). An installation that could not be located leaves the state root
    /// `install_state_root_unresolved`, caused by that condition.
    pub(super) fn locate(&self) -> Location {
        let root = self.root.clone();
        let state_root =
            match (&self.location_error, &self.state_root) {
                (Some(error), _) => Err(Outcome::new(ActingctlCode::InstallStateRootUnresolved)
                    .caused_by(error.clone())),
                (None, Some(state_root)) => Ok(state_root.clone()),
                (None, None) => configured_state_root(self.config.as_deref()),
            };
        Location {
            root,
            install_root: self.install_root.clone(),
            state_root,
            config: self.config.clone(),
            installation: self.installation.clone(),
            error: self.location_error.clone(),
        }
    }

    pub(super) fn connect(&self) -> Result<Connected, ToolError> {
        let mut slot = lock(&self.slot);
        if let Some(client) = slot.client.clone() {
            // A cached connection may outlive its daemon (actingd stopped, restarted or
            // redeployed while the client keeps this server). `Health` is answered from the
            // Runtime's memory and writes nothing; a failure, or an owner epoch other than the
            // one this connection was opened on, drops the connection and connects afresh,
            // so earlier cursors answer cursor_invalid.
            match client.health() {
                Ok(epoch) if epoch == client.runtime_info().owner_epoch() => {
                    return Ok(Connected {
                        client,
                        generation: slot.generation,
                    });
                }
                _ => slot.client = None,
            }
        }
        let state_root = self.locate().state_root.map_err(state_root_unresolved)?;
        let client = RuntimeClient::connect(RuntimeClientConfig::new(
            &state_root,
            actingcommand_runtime_client::RuntimeChannel::Mcp,
            EventActor::Cli,
            EventSource::Cli,
        ))
        .map_err(|error| unavailable(&error, &state_root))?;
        slot.generation += 1;
        slot.client = Some(client.clone());
        slot.instances.clear();
        Ok(Connected {
            client,
            generation: slot.generation,
        })
    }

    /// Maps a Runtime client failure on `connected`, and drops that connection when the
    /// client has locked it so the next call reconnects.
    pub(super) fn failure(&self, connected: &Connected, error: &RuntimeClientError) -> ToolError {
        // `begin_interaction` only mints a correlation locally; on a locked connection it
        // answers the latched terminal error without any IO.
        if connected.client.begin_interaction().is_err() {
            let mut slot = lock(&self.slot);
            if slot.generation == connected.generation {
                slot.client = None;
            }
        }
        client_error(error)
    }

    /// The instance_id `selector` named when it was resolved on `connected`, while that is
    /// still the server's connection (Workflow #389 MCP-1, §2.5).
    pub(super) fn resolved_instance(
        &self,
        connected: &Connected,
        selector: &str,
    ) -> Option<InstanceId> {
        let slot = lock(&self.slot);
        if slot.generation == connected.generation {
            slot.instances.get(selector).copied()
        } else {
            None
        }
    }

    /// Remembers what `selector` resolved to on `connected`, for that connection's lifetime.
    pub(super) fn remember_instance(
        &self,
        connected: &Connected,
        selector: &str,
        instance_id: InstanceId,
    ) {
        let mut slot = lock(&self.slot);
        if slot.generation == connected.generation {
            slot.instances.insert(selector.to_owned(), instance_id);
        }
    }

    /// A new connection with this origin, for one write and its job: (Cli, Cli) as actingctl,
    /// or (Agent, Adapter) for resource targets.
    pub(super) fn connect_fresh(
        &self,
        actor: EventActor,
        source: EventSource,
    ) -> Result<RuntimeClient, ToolError> {
        connect_at(&self.locate().state_root, actor, source)
    }
}

/// A new connection with this origin to the Runtime of a state root found earlier; a Lab
/// call's job connects this way once its child has returned.
pub(super) fn connect_at(
    state_root: &Result<PathBuf, Outcome>,
    actor: EventActor,
    source: EventSource,
) -> Result<RuntimeClient, ToolError> {
    let state_root = state_root
        .as_ref()
        .map_err(|outcome| state_root_unresolved(outcome.clone()))?;
    RuntimeClient::connect(RuntimeClientConfig::new(
        state_root,
        actingcommand_runtime_client::RuntimeChannel::Mcp,
        actor,
        source,
    ))
    .map_err(|error| unavailable(&error, state_root))
}

/// A state root that cannot be told (`install_state_root_unresolved`): class usage; the person
/// gives `mcp-serve --state-root`, or `state_root` in the install's configuration.
fn state_root_unresolved(outcome: Outcome) -> ToolError {
    ToolError::outcome("usage", outcome).blocked_by(McpBlocker::StateRoot)
}

/// A Runtime client failure as a tool error (Workflow #378 A5): its class is the client's own
/// `RuntimeClientError::disposition()` (R6), the same class CLI, UI and Lab report, and its
/// outcome the client's: a denied or failed receipt's outcome on top as the Runtime wrote it,
/// else the client's code. A refusal's top also carries its receipt's `receipt_state` and
/// `request_id` (coordinator ruling on A5): a consumer judges refusal by the state (A4), and the
/// request id is a run's handle for `ac_get_run`.
pub(super) fn client_error(error: &RuntimeClientError) -> ToolError {
    let outcome = error.outcome();
    let outcome = match (error.receipt_outcome(), error.received_receipt()) {
        (Some(_), Some(receipt)) => {
            let outcome = outcome.receipt_state(receipt.state());
            match serde_json::to_value(receipt.request_id()) {
                Ok(Value::String(request_id)) => outcome.request_id(request_id),
                _ => outcome,
            }
        }
        _ => outcome,
    };
    ToolError::outcome(error.disposition().as_str(), outcome)
}

/// actingd does not answer for this state root: no `runtime-info.json`, or nothing listens
/// where it points, or its owner epoch does not match. `runtime_unavailable` with the state
/// root, caused by the client's outcome; starting actingd belongs to the person.
fn unavailable(error: &RuntimeClientError, state_root: &Path) -> ToolError {
    ToolError::outcome(
        error.disposition().as_str(),
        Outcome::new(RuntimeErrorCode::RuntimeUnavailable)
            .state_root(state_root)
            .caused_by(error.outcome()),
    )
    .blocked_by(McpBlocker::RuntimeRunning)
}

fn executable_install_root() -> Option<PathBuf> {
    let executable = fs::canonicalize(std::env::current_exe().ok()?).ok()?;
    let root = executable.parent()?.parent()?.to_path_buf();
    root.join("runtime")
        .join("BUILD-MANIFEST.json")
        .is_file()
        .then_some(root)
}

/// The absolute `state_root` of the install's configuration. Without a configuration (no
/// install root and no `--root`) it is `install_state_root_unresolved` caused by
/// `install_root_unresolved`; otherwise that code with `config_path`, caused by the read or
/// parse failure, or with the configured value as `path` when it is not absolute.
fn configured_state_root(config: Option<&Path>) -> Result<PathBuf, Outcome> {
    let unresolved = || Outcome::new(ActingctlCode::InstallStateRootUnresolved);
    let config = config.ok_or_else(|| {
        unresolved().caused_by(Outcome::new(ActingctlCode::InstallRootUnresolved))
    })?;
    let bytes = read_bounded(config, MAX_CONFIG_BYTES)
        .map_err(|cause| unresolved().config_path(config).caused_by(cause))?;
    let document = serde_json::from_slice::<Value>(&bytes).map_err(|error| {
        unresolved().config_path(config).caused_by(
            Outcome::new(ContractCode::ForeignDataError)
                .raw_source(RawSource::Json)
                .raw_text(error.to_string())
                .path(config),
        )
    })?;
    let state_root = document
        .get("state_root")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| unresolved().config_path(config))?;
    if !state_root.is_absolute() {
        return Err(unresolved().config_path(config).path(&state_root));
    }
    Ok(state_root)
}
