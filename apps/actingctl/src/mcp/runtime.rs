// SPDX-License-Identifier: AGPL-3.0-only

//! Lazy location and connection (#338 §四 定位 / 启动预算 / 连接). Nothing here runs at
//! startup: the first tool call finds the install root and the state root and connects
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
pub(super) struct Location {
    pub(super) root: Option<PathBuf>,
    pub(super) state_root: Result<PathBuf, String>,
}

/// The control connection with (Cli, Cli) origin, as actingctl itself uses.
pub(super) struct Connected {
    pub(super) client: RuntimeClient,
    pub(super) generation: u64,
}

impl RuntimeAccess {
    pub(super) fn new(root: Option<PathBuf>, state_root: Option<PathBuf>) -> Self {
        Self {
            root,
            state_root,
            slot: Mutex::new(Slot::default()),
        }
    }

    /// `--root`, or two levels above the running executable when
    /// `<root>\runtime\BUILD-MANIFEST.json` is a file (as actingd's install detection);
    /// `--state-root`, or the absolute `state_root` of `<root>\actingd.config.json` (as the
    /// UI's setup reads it).
    pub(super) fn locate(&self) -> Location {
        let root = self.root.clone().or_else(executable_install_root);
        let state_root = match &self.state_root {
            Some(state_root) => Ok(state_root.clone()),
            None => configured_state_root(root.as_deref()),
        };
        Location { root, state_root }
    }

    pub(super) fn connect(&self) -> Result<Connected, ToolError> {
        let mut slot = lock(&self.slot);
        if let Some(client) = &slot.client {
            return Ok(Connected {
                client: client.clone(),
                generation: slot.generation,
            });
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
        runtime_error(error)
    }
}

/// TEMP until Ra: every Runtime client failure is class `runtime`, with the Runtime's own
/// code in `details.runtime_code`. Ra's `RuntimeClientError::disposition()` replaces this
/// function, which is the only place a Runtime failure gets its class.
fn error_class(_error: &RuntimeClientError) -> &'static str {
    "runtime"
}

fn runtime_error(error: &RuntimeClientError) -> ToolError {
    let mut mapped = ToolError::new(error_class(error), error.code(), error.to_string())
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
        error_class(error),
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

fn configured_state_root(root: Option<&Path>) -> Result<PathBuf, String> {
    let root = root.ok_or_else(|| {
        "actingctl does not run from an install root (<root>\\runtime\\actingctl.exe beside <root>\\runtime\\BUILD-MANIFEST.json) and mcp-serve got neither --root nor --state-root".to_owned()
    })?;
    let config = root.join("actingd.config.json");
    let bytes = read_bounded(&config, MAX_CONFIG_BYTES)?;
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
