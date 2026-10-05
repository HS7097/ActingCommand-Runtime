// SPDX-License-Identifier: AGPL-3.0-only

//! Installation inputs and the non-persistent Host installation protocol (Workflow #352).
//! Only acsetup commits the selection. Running processes retain the selection they started
//! with; these documents contain no Runtime state and are never a substitute for the ledger.

use crate::{RequestId, RuntimeContractError, RuntimeContractResult, RuntimeShutdownTarget};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, OnceLock};

pub const INSTALL_SELECTION_SCHEMA: &str = "actingcommand.install-selection.v1";
pub const INSTALL_SELECTION_PATH: &str = "install/active.json";
pub const INSTALL_ROOT_ENV: &str = "ACTINGCOMMAND_INSTALL_ROOT";
pub const INSTALL_SELECTION_ENV: &str = "ACTINGCOMMAND_INSTALL_SELECTION";
pub const MAX_INSTALL_SELECTION_BYTES: usize = 16 * 1024;
pub const DEFAULT_INSTALL_TIMEOUT_MS: u64 = 60_000;
pub const MAX_INSTALL_TIMEOUT_MS: u64 = 600_000;

/// OS occupancy of an existing, permanently empty installation locator. Dropping the
/// handle (including process exit) releases occupancy; it never writes installation state.
#[derive(Debug)]
pub struct InstallSlotLock {
    file: std::fs::File,
    root: PathBuf,
    slot: InstallSlot,
}

impl InstallSlotLock {
    pub fn try_shared(root: &Path, slot: InstallSlot) -> RuntimeContractResult<Self> {
        Self::acquire(root, slot, false)
    }

    /// acsetup retains this guard throughout materialization. It must separately prove
    /// that consumers without this protocol no longer occupy the target slot.
    pub fn try_exclusive(root: &Path, slot: InstallSlot) -> RuntimeContractResult<Self> {
        Self::acquire(root, slot, true)
    }

    fn acquire(root: &Path, slot: InstallSlot, exclusive: bool) -> RuntimeContractResult<Self> {
        let root = std::fs::canonicalize(root)
            .map_err(|_| RuntimeContractError::new("install_root_unavailable"))?;
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(exclusive);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // Readers and the installer may open the locator; nobody replaces it in use.
            options.share_mode(0x0000_0001 | 0x0000_0002);
        }
        let file = options
            .open(root.join(format!("install/slot-{}.lock", slot.as_str())))
            .map_err(|error| {
                RuntimeContractError::new(match error.kind() {
                    std::io::ErrorKind::NotFound => "install_slot_lock_missing",
                    _ => "install_slot_lock_open_failed",
                })
            })?;
        let locked = if exclusive {
            file.try_lock()
        } else {
            file.try_lock_shared()
        };
        locked.map_err(|error| {
            RuntimeContractError::new(match error {
                std::fs::TryLockError::WouldBlock => "install_slot_in_use",
                std::fs::TryLockError::Error(_) => "install_slot_lock_failed",
            })
        })?;
        let metadata = file
            .metadata()
            .map_err(|_| RuntimeContractError::new("install_slot_lock_metadata_failed"))?;
        if !metadata.is_file() || metadata.len() != 0 {
            return Err(RuntimeContractError::new("install_slot_lock_not_empty"));
        }
        Ok(Self { file, root, slot })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub const fn slot(&self) -> InstallSlot {
        self.slot
    }

    /// Explicit release reports an OS unlock failure. The consumed handle then closes;
    /// no retry or disk cleanup is needed for normal scope or process termination.
    pub fn release(self) -> RuntimeContractResult<()> {
        self.file
            .unlock()
            .map_err(|_| RuntimeContractError::new("install_slot_unlock_failed"))
    }
}

/// Pins the direct executable's slot before any slot material is read. Static candidate
/// checks use this occupancy without selecting the running installation's configuration.
pub fn process_slot_lock() -> RuntimeContractResult<Option<&'static InstallSlotLock>> {
    static SLOT: OnceLock<Result<Option<InstallSlotLock>, &'static str>> = OnceLock::new();
    SLOT.get_or_init(|| {
        let executable = std::env::current_exe().map_err(|_| "install_executable_unavailable")?;
        let executable =
            std::fs::canonicalize(executable).map_err(|_| "install_executable_unavailable")?;
        let Some(directory) = executable.parent().filter(|path| {
            matches!(
                path.file_name().and_then(|name| name.to_str()),
                Some("runtime" | "tools" | "ui")
            )
        }) else {
            return Ok(None);
        };
        let Some(program_root) = directory.parent() else {
            return Ok(None);
        };
        let slot = match program_root.file_name().and_then(|name| name.to_str()) {
            Some("A") => InstallSlot::A,
            Some("B") => InstallSlot::B,
            _ => return Ok(None),
        };
        let root = program_root.parent().ok_or("install_root_unavailable")?;
        InstallSlotLock::try_shared(root, slot)
            .map(Some)
            .map_err(|error| error.code())
    })
    .as_ref()
    .map(Option::as_ref)
    .map_err(|code| RuntimeContractError::new(code))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallConfigPurpose {
    Running,
    CandidateCheck,
}

pub fn process_installation_for(
    purpose: InstallConfigPurpose,
) -> RuntimeContractResult<Option<&'static InstalledProcess>> {
    match purpose {
        InstallConfigPurpose::Running => process_installation(),
        InstallConfigPurpose::CandidateCheck => {
            process_slot_lock()?;
            Ok(None)
        }
    }
}

/// The immutable installation inputs selected for this process. This reads configuration
/// only, never writes active.json, starts a process or owns Runtime state.
#[derive(Debug, Clone)]
pub struct InstalledProcess {
    root: PathBuf,
    selection: InstallSelection,
    selection_json: String,
    _slot_lock: Arc<InstallSlotLock>,
}

impl InstalledProcess {
    pub fn read(root: &Path) -> RuntimeContractResult<Option<Self>> {
        let root = std::fs::canonicalize(root)
            .map_err(|_| RuntimeContractError::new("install_root_unavailable"))?;
        let environment_root = std::env::var_os(INSTALL_ROOT_ENV);
        let environment_selection = std::env::var_os(INSTALL_SELECTION_ENV);
        let bytes = match (environment_root, environment_selection) {
            (Some(environment_root), Some(selection)) => {
                let environment_root = std::fs::canonicalize(environment_root).map_err(|_| {
                    RuntimeContractError::new("install_environment_root_unavailable")
                })?;
                if environment_root != root {
                    return Err(RuntimeContractError::new(
                        "install_environment_root_mismatch",
                    ));
                }
                selection
                    .into_string()
                    .map_err(|_| {
                        RuntimeContractError::new("install_environment_selection_invalid")
                    })?
                    .into_bytes()
            }
            (None, None) => {
                let path = root.join(INSTALL_SELECTION_PATH);
                match std::fs::File::open(path) {
                    Ok(file) => read_install_input(file, MAX_INSTALL_SELECTION_BYTES)?,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                    Err(_) => {
                        return Err(RuntimeContractError::new("install_selection_unavailable"));
                    }
                }
            }
            _ => return Err(RuntimeContractError::new("install_environment_incomplete")),
        };
        let selection = InstallSelection::from_json(&bytes)?;
        let slot_lock = Arc::new(InstallSlotLock::try_shared(&root, selection.slot)?);
        let selection_json = String::from_utf8(bytes)
            .map_err(|_| RuntimeContractError::new("install_selection_json_invalid"))?;
        let selected = Self {
            root,
            selection,
            selection_json,
            _slot_lock: slot_lock,
        };
        selected.read_reference(&selected.selection.members, 4 * 1024 * 1024)?;
        selected.config_bytes()?;
        if let Some(provider) = &selected.selection.provider {
            selected.read_reference(provider, 1024 * 1024)?;
        }
        Ok(Some(selected))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn selection(&self) -> &InstallSelection {
        &self.selection
    }
    pub fn selection_json(&self) -> &str {
        &self.selection_json
    }
    pub fn program_root(&self) -> PathBuf {
        self.root.join(self.selection.slot.as_str())
    }
    pub fn config_path(&self) -> PathBuf {
        self.root.join(&self.selection.config.path)
    }

    pub fn config_bytes(&self) -> RuntimeContractResult<Vec<u8>> {
        self.read_reference(&self.selection.config, 1024 * 1024)
    }

    pub fn resolve_config(&self, requested: &Path) -> RuntimeContractResult<PathBuf> {
        let requested = std::path::absolute(requested)
            .map_err(|_| RuntimeContractError::new("install_config_path_invalid"))?;
        let selected = self.config_path();
        let root_alias = requested
            .file_name()
            .is_some_and(|name| name == "actingd.config.json")
            && requested
                .parent()
                .and_then(|parent| std::fs::canonicalize(parent).ok())
                .as_ref()
                == Some(&self.root);
        let selected_canonical = std::fs::canonicalize(&selected)
            .map_err(|_| RuntimeContractError::new("install_config_unavailable"))?;
        if root_alias
            || std::fs::canonicalize(&requested).ok().as_ref() == Some(&selected_canonical)
        {
            Ok(selected)
        } else {
            Err(RuntimeContractError::new(
                "install_config_selection_mismatch",
            ))
        }
    }

    fn read_reference(
        &self,
        reference: &InstallFileReference,
        limit: usize,
    ) -> RuntimeContractResult<Vec<u8>> {
        let path = std::fs::canonicalize(reference.resolve(&self.root)?)
            .map_err(|_| RuntimeContractError::new("install_input_unavailable"))?;
        if !path.starts_with(&self.root) {
            return Err(RuntimeContractError::new("install_input_outside_root"));
        }
        let file = std::fs::File::open(path)
            .map_err(|_| RuntimeContractError::new("install_input_unavailable"))?;
        let bytes = read_install_input(file, limit)?;
        if format!("{:x}", Sha256::digest(&bytes)) != reference.sha256 {
            return Err(RuntimeContractError::new("install_input_hash_mismatch"));
        }
        Ok(bytes)
    }
}

fn read_install_input(file: std::fs::File, limit: usize) -> RuntimeContractResult<Vec<u8>> {
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| RuntimeContractError::new("install_input_read_failed"))?;
    if bytes.is_empty() || bytes.len() > limit {
        return Err(RuntimeContractError::new("install_input_size_invalid"));
    }
    Ok(bytes)
}

/// Reads the launcher snapshot (or direct executable's installation selection) once.
/// Reconnection and child calls retain it for this process's entire lifetime.
pub fn process_installation() -> RuntimeContractResult<Option<&'static InstalledProcess>> {
    static PIN: OnceLock<Result<Option<InstalledProcess>, &'static str>> = OnceLock::new();
    PIN.get_or_init(|| {
        let own_slot = process_slot_lock().map_err(|error| error.code())?;
        let executable = std::env::current_exe().map_err(|_| "install_executable_unavailable")?;
        let executable =
            std::fs::canonicalize(executable).map_err(|_| "install_executable_unavailable")?;
        let program_root = executable.parent().and_then(Path::parent);
        let root = if let Some(root) = std::env::var_os(INSTALL_ROOT_ENV) {
            Some(PathBuf::from(root))
        } else if std::env::var_os(INSTALL_SELECTION_ENV).is_some() {
            return Err("install_environment_incomplete");
        } else if let Some(slot) = own_slot {
            Some(slot.root().to_path_buf())
        } else if let Some(program_root) =
            program_root.filter(|root| root.join("runtime/BUILD-MANIFEST.json").is_file())
        {
            if matches!(
                program_root.file_name().and_then(|name| name.to_str()),
                Some("A" | "B")
            ) {
                program_root.parent().map(Path::to_path_buf)
            } else {
                Some(program_root.to_path_buf())
            }
        } else {
            None
        };
        let Some(root) = root else {
            return Ok(None);
        };
        let selected = InstalledProcess::read(&root).map_err(|error| error.code())?;
        if own_slot.is_some() && selected.is_none() {
            return Err("install_selection_unavailable");
        }
        if let Some(selected) = &selected
            && !executable.starts_with(selected.program_root())
        {
            return Err("install_process_slot_mismatch");
        }
        Ok(selected)
    })
    .as_ref()
    .map(Option::as_ref)
    .map_err(|code| RuntimeContractError::new(code))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstallSlot {
    A,
    B,
}

impl InstallSlot {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallFileReference {
    /// Portable, slash-separated path relative to the one installation root.
    pub path: String,
    /// Lowercase, unprefixed SHA-256 of the complete input bytes.
    pub sha256: String,
}

impl InstallFileReference {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        let path = Path::new(&self.path);
        if self.path.is_empty()
            || self.path.len() > 1024
            || self.path.contains(['\\', ':', '\0'])
            || self
                .path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
            || !path
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
            || !canonical_sha256(&self.sha256)
        {
            return Err(RuntimeContractError::new("invalid_install_file_reference"));
        }
        Ok(())
    }

    pub fn resolve(&self, root: &Path) -> RuntimeContractResult<PathBuf> {
        self.validate()?;
        Ok(root.join(&self.path))
    }
}

/// One atomic acsetup selection of both program material and private configuration inputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallSelection {
    pub schema_version: String,
    pub slot: InstallSlot,
    pub generation: u64,
    pub members: InstallFileReference,
    pub config: InstallFileReference,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<InstallFileReference>,
}

impl InstallSelection {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.schema_version != INSTALL_SELECTION_SCHEMA || self.generation == 0 {
            return Err(RuntimeContractError::new("invalid_install_selection"));
        }
        self.members.validate()?;
        self.config.validate()?;
        if self.members.path != format!("{}/MEMBERS.json", self.slot.as_str()) {
            return Err(RuntimeContractError::new("install_members_slot_mismatch"));
        }
        let generation = format!("install/generations/{}/", self.generation);
        if !self.config.path.starts_with(&generation) {
            return Err(RuntimeContractError::new(
                "install_config_generation_mismatch",
            ));
        }
        if let Some(provider) = &self.provider {
            provider.validate()?;
            if !provider.path.starts_with(&generation) || provider.path == self.config.path {
                return Err(RuntimeContractError::new(
                    "install_provider_generation_mismatch",
                ));
            }
        }
        Ok(())
    }

    pub fn from_json(bytes: &[u8]) -> RuntimeContractResult<Self> {
        if bytes.is_empty() || bytes.len() > MAX_INSTALL_SELECTION_BYTES {
            return Err(RuntimeContractError::new("install_selection_size_invalid"));
        }
        let selection: Self = serde_json::from_slice(bytes)
            .map_err(|_| RuntimeContractError::new("install_selection_json_invalid"))?;
        selection.validate()?;
        Ok(selection)
    }

    pub fn slot_root(&self, root: &Path) -> RuntimeContractResult<PathBuf> {
        self.validate()?;
        Ok(root.join(self.slot.as_str()))
    }
}

/// This identity names one in-memory transition of one exact Host, including its first request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallTransitionTicket {
    pub target: RuntimeShutdownTarget,
    pub transition_id: String,
    pub request_id: RequestId,
}

impl InstallTransitionTicket {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        self.target.validate()?;
        validate_transition_id(&self.transition_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum InstallTransitionAction {
    BeginDrain {
        transition_id: String,
        #[serde(default = "default_install_timeout_ms")]
        timeout_ms: u64,
    },
    Query {
        transition_id: String,
    },
    Abort {
        ticket: InstallTransitionTicket,
    },
    CommitShutdown {
        ticket: InstallTransitionTicket,
    },
    Release {
        ticket: InstallTransitionTicket,
        #[serde(default = "default_install_timeout_ms")]
        timeout_ms: u64,
    },
}

impl InstallTransitionAction {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        match self {
            Self::BeginDrain {
                transition_id,
                timeout_ms,
            } => {
                validate_transition_id(transition_id)?;
                validate_install_timeout(*timeout_ms)
            }
            Self::Query { transition_id } => validate_transition_id(transition_id),
            Self::Abort { ticket } | Self::CommitShutdown { ticket } => ticket.validate(),
            Self::Release { ticket, timeout_ms } => {
                ticket.validate()?;
                validate_install_timeout(*timeout_ms)
            }
        }
    }
}

/// Startup input supplied by the installation transaction, before Provider assembly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallHeldStartup {
    pub transition_id: String,
    pub request_id: RequestId,
    #[serde(default = "default_install_timeout_ms")]
    pub timeout_ms: u64,
    /// The prior Host's drained transition; only its ledger facts may restore user pauses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<InstallTransitionTicket>,
}

impl InstallHeldStartup {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        validate_transition_id(&self.transition_id)?;
        validate_install_timeout(self.timeout_ms)?;
        if let Some(previous) = &self.previous {
            previous.validate()?;
            if previous.transition_id != self.transition_id {
                return Err(RuntimeContractError::new(
                    "install_previous_transition_mismatch",
                ));
            }
        }
        Ok(())
    }
}

/// A live IPC projection, never serialized as a new ledger payload or lifecycle enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallTransitionPhase {
    Draining,
    Drained,
    Held,
    Preparing,
    Released,
    Aborted,
    Failed,
}

impl InstallTransitionPhase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draining => "draining",
            Self::Drained => "drained",
            Self::Held => "held",
            Self::Preparing => "preparing",
            Self::Released => "released",
            Self::Aborted => "aborted",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallTransitionStatus {
    pub ticket: InstallTransitionTicket,
    pub phase: InstallTransitionPhase,
    pub admission_closed: bool,
    pub timeout_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<String>,
}

impl InstallTransitionStatus {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        self.ticket.validate()?;
        validate_install_timeout(self.timeout_ms)?;
        let closed = !matches!(
            self.phase,
            InstallTransitionPhase::Released | InstallTransitionPhase::Aborted
        );
        if self.admission_closed != closed
            || self.failure_code.as_ref().is_some_and(|code| {
                code.is_empty()
                    || code.len() > 128
                    || !code
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            })
        {
            return Err(RuntimeContractError::new(
                "invalid_install_transition_status",
            ));
        }
        Ok(())
    }
}

pub fn validate_install_timeout(timeout_ms: u64) -> RuntimeContractResult<()> {
    if timeout_ms == 0 || timeout_ms > MAX_INSTALL_TIMEOUT_MS {
        return Err(RuntimeContractError::new("invalid_install_timeout"));
    }
    Ok(())
}

const fn default_install_timeout_ms() -> u64 {
    DEFAULT_INSTALL_TIMEOUT_MS
}

fn validate_transition_id(value: &str) -> RuntimeContractResult<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(RuntimeContractError::new("invalid_install_transition_id"));
    }
    Ok(())
}

fn canonical_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
