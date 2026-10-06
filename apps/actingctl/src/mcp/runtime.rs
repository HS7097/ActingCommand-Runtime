// SPDX-License-Identifier: AGPL-3.0-only

//! Installation inputs are pinned at process startup (#352); the first tool call connects
//! with `RuntimeClient::connect`, which checks the owner epoch against `runtime-info.json`.
//! A connection the client has locked after a failure is dropped, and the next call
//! connects again; cursors issued on the old connection then answer `cursor_invalid`.

use super::lock;
use super::tools::ToolError;
use actingcommand_contract::{EventActor, EventSource};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig, RuntimeClientError};
use serde_json::{Value, json};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// `<root>\actingd.config.json` is read whole up to this size.
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

pub(super) struct RuntimeAccess {
    root: Option<PathBuf>,
    install_root: Option<PathBuf>,
    config: Option<PathBuf>,
    installation: Option<actingcommand_contract::InstalledProcess>,
    location_error: Option<String>,
    state_root: Option<PathBuf>,
    slot: Mutex<Slot>,
}

#[derive(Default)]
struct Slot {
    client: Option<RuntimeClient>,
    /// Counts connections; a cursor names the one it was issued on.
    generation: u64,
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
    error: Option<String>,
    pub(super) state_root: Result<PathBuf, String>,
}

impl Location {
    pub(super) fn check(&self) -> Result<(), ToolError> {
        match &self.error {
            Some(error) => Err(ToolError::usage(
                "install_selection_unavailable",
                error.clone(),
            )),
            None => Ok(()),
        }
    }
}

pub(super) fn pin_child(
    command: &mut std::process::Command,
    installation: Option<&actingcommand_contract::InstalledProcess>,
) {
    if let Some(installation) = installation {
        command
            .env(
                actingcommand_contract::INSTALL_ROOT_ENV,
                installation.root(),
            )
            .env(
                actingcommand_contract::INSTALL_SELECTION_ENV,
                installation.selection_json(),
            )
            .current_dir(installation.root());
    }
}

/// The control connection with (Cli, Cli) origin, as actingctl itself uses.
pub(super) struct Connected {
    pub(super) client: RuntimeClient,
    pub(super) generation: u64,
}

impl RuntimeAccess {
    pub(super) fn new(root: Option<PathBuf>, state_root: Option<PathBuf>) -> Self {
        let located = (|| {
            let process = actingcommand_contract::process_installation()
                .map_err(|error| error.code().to_owned())?;
            let installation = match process {
                Some(selected) => {
                    if let Some(root) = &root {
                        let root = fs::canonicalize(root).map_err(|error| error.to_string())?;
                        if root != selected.root() {
                            return Err("install_process_root_mismatch".to_owned());
                        }
                    }
                    Some(selected.clone())
                }
                None => root
                    .as_deref()
                    .map(actingcommand_contract::InstalledProcess::read)
                    .transpose()
                    .map_err(|error| error.code().to_owned())?
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
                    let requested =
                        fs::canonicalize(requested).map_err(|error| error.to_string())?;
                    let configured =
                        fs::canonicalize(configured).map_err(|error| error.to_string())?;
                    if requested != configured {
                        return Err("install_process_state_root_mismatch".to_owned());
                    }
                }
            }
            Ok::<_, String>((program_root, install_root, config, installation))
        })();
        let (root, install_root, config, installation, location_error) = match located {
            Ok((root, install_root, config, installation)) => {
                (root, install_root, config, installation, None)
            }
            Err(error) => (None, None, None, None, Some(error)),
        };
        Self {
            root,
            install_root,
            config,
            installation,
            location_error,
            state_root,
            slot: Mutex::new(Slot::default()),
        }
    }

    /// `--root`, or two levels above the running executable when
    /// `<root>\runtime\BUILD-MANIFEST.json` is a file (as actingd's install detection);
    /// `--state-root`, or the absolute `state_root` of `<root>\actingd.config.json` (as the
    /// UI's setup reads it).
    pub(super) fn locate(&self) -> Location {
        let root = self.root.clone();
        let state_root = match (&self.location_error, &self.state_root) {
            (Some(error), _) => Err(error.clone()),
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
        let state_root = self.locate().state_root.map_err(|reason| {
            ToolError::usage("install_state_root_unresolved", reason).blocked_by(
                "mcp-serve --state-root <dir>, or state_root in <root>\\actingd.config.json",
            )
        })?;
        let client = RuntimeClient::connect(RuntimeClientConfig::new(
            &state_root,
            EventActor::Cli,
            EventSource::Cli,
        ))
        .map_err(|error| unavailable(&error, &state_root))?;
        slot.generation += 1;
        slot.client = Some(client.clone());
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
    state_root: &Result<PathBuf, String>,
    actor: EventActor,
    source: EventSource,
) -> Result<RuntimeClient, ToolError> {
    let state_root = state_root.as_ref().map_err(|reason| {
        ToolError::usage("install_state_root_unresolved", reason.clone()).blocked_by(
            "mcp-serve --state-root <dir>, or state_root in <root>\\actingd.config.json",
        )
    })?;
    RuntimeClient::connect(RuntimeClientConfig::new(state_root, actor, source))
        .map_err(|error| unavailable(&error, state_root))
}

/// A Runtime client failure as a tool error. Its class is the client's own
/// `RuntimeClientError::disposition()` (R6), the same class CLI, UI and Lab report.
pub(super) fn client_error(error: &RuntimeClientError) -> ToolError {
    let mut mapped = ToolError::new(
        error.disposition().as_str(),
        error.code(),
        error.to_string(),
    )
    .with_detail("operation", json!(error.operation()));
    if let Some(projection) = error.projection() {
        mapped = mapped.with_detail("runtime_code", json!(projection.code));
    }
    if let Some((code, operation)) = error.host_failure() {
        mapped = mapped.with_detail(
            "host_failure",
            json!({"code": code, "operation": operation}),
        );
    }
    if let Some(receipt) = error.received_receipt() {
        mapped = mapped
            .with_detail("receipt_state", json!(receipt.state()))
            .with_detail("request_id", json!(receipt.request_id()));
    }
    mapped
}

/// actingd does not answer for this state root: no `runtime-info.json`, or nothing listens
/// where it points, or its owner epoch does not match.
fn unavailable(error: &RuntimeClientError, state_root: &Path) -> ToolError {
    ToolError::new(
        error.disposition().as_str(),
        "runtime_unavailable",
        format!(
            "actingd does not answer for state root {}: {error}",
            state_root.display()
        ),
    )
    .with_detail("client_code", json!(error.code()))
    .with_detail("operation", json!(error.operation()))
    .blocked_by("actingd running on this state root; starting it belongs to the person")
}

fn executable_install_root() -> Option<PathBuf> {
    let executable = fs::canonicalize(std::env::current_exe().ok()?).ok()?;
    let root = executable.parent()?.parent()?.to_path_buf();
    root.join("runtime")
        .join("BUILD-MANIFEST.json")
        .is_file()
        .then_some(root)
}

fn configured_state_root(config: Option<&Path>) -> Result<PathBuf, String> {
    let config = config.ok_or_else(|| {
        "actingctl does not run from an install root (<root>\\runtime\\actingctl.exe beside <root>\\runtime\\BUILD-MANIFEST.json) and mcp-serve got neither --root nor --state-root".to_owned()
    })?;
    let bytes = read_bounded(config, MAX_CONFIG_BYTES)?;
    let document = serde_json::from_slice::<Value>(&bytes)
        .map_err(|error| format!("{} is not JSON: {error}", config.display()))?;
    let state_root = document
        .get("state_root")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| format!("{} has no string state_root", config.display()))?;
    if !state_root.is_absolute() {
        return Err(format!(
            "state_root in {} is not absolute: {}",
            config.display(),
            state_root.display()
        ));
    }
    Ok(state_root)
}

/// Reads a whole file of at most `limit` bytes.
pub(super) fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let file =
        fs::File::open(path).map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if bytes.len() as u64 > limit {
        return Err(format!("{} is larger than {limit} bytes", path.display()));
    }
    Ok(bytes)
}
