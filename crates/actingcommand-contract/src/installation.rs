// SPDX-License-Identifier: AGPL-3.0-only

//! Installation inputs (Workflow #352). Only acsetup commits the selection. Running processes
//! retain the selection they started with; these documents contain no Runtime state and are never
//! a substitute for the ledger. Workflow #434 LAB-6S4a (0.12.63): the non-persistent Host
//! installation protocol (`install-control` revision 1, the transition and the held start) is gone;
//! `install-control` is [0, 0].

use crate::codes::{InstallationCode, RuntimeContractCode, SharedCode};
use crate::{RuntimeContractError, RuntimeContractResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, OnceLock};

// Interface anchor `install-selection` (contracts/component-interfaces.md): raise its revision in
// distribution/windows/component-interfaces.json when older peers can no longer read this.
pub const INSTALL_SELECTION_SCHEMA: &str = "actingcommand.install-selection.v1";
pub const INSTALL_SELECTION_PATH: &str = "install/active.json";
pub const INSTALL_ROOT_ENV: &str = "ACTINGCOMMAND_INSTALL_ROOT";
pub const INSTALL_SELECTION_ENV: &str = "ACTINGCOMMAND_INSTALL_SELECTION";
pub const MAX_INSTALL_SELECTION_BYTES: usize = 16 * 1024;

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
            .map_err(|_| RuntimeContractError::new(RuntimeContractCode::InstallRootUnavailable))?;
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
                    std::io::ErrorKind::NotFound => RuntimeContractCode::InstallSlotLockMissing,
                    _ => RuntimeContractCode::InstallSlotLockOpenFailed,
                })
            })?;
        let locked = if exclusive {
            file.try_lock()
        } else {
            file.try_lock_shared()
        };
        locked.map_err(|error| {
            RuntimeContractError::new(match error {
                std::fs::TryLockError::WouldBlock => RuntimeContractCode::InstallSlotInUse,
                std::fs::TryLockError::Error(_) => RuntimeContractCode::InstallSlotLockFailed,
            })
        })?;
        let metadata = file.metadata().map_err(|_| {
            RuntimeContractError::new(RuntimeContractCode::InstallSlotLockMetadataFailed)
        })?;
        if !metadata.is_file() || metadata.len() != 0 {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InstallSlotLockNotEmpty,
            ));
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
            .map_err(|_| RuntimeContractError::new(RuntimeContractCode::InstallSlotUnlockFailed))
    }
}

/// Pins the direct executable's slot before any slot material is read. Static candidate
/// checks use this occupancy without selecting the running installation's configuration.
pub fn process_slot_lock() -> RuntimeContractResult<Option<&'static InstallSlotLock>> {
    static SLOT: OnceLock<Result<Option<InstallSlotLock>, crate::outcome::Code>> = OnceLock::new();
    SLOT.get_or_init(|| {
        let executable = std::env::current_exe()
            .map_err(|_| InstallationCode::InstallExecutableUnavailable.code())?;
        let executable = std::fs::canonicalize(executable)
            .map_err(|_| InstallationCode::InstallExecutableUnavailable.code())?;
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
        let root = program_root
            .parent()
            .ok_or(RuntimeContractCode::InstallRootUnavailable.code())?;
        InstallSlotLock::try_shared(root, slot)
            .map(Some)
            .map_err(|error| error.code())
    })
    .as_ref()
    .map(Option::as_ref)
    .map_err(|code| RuntimeContractError::new(*code))
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
    /// Running consumers prefer the inherited snapshot and retain this result for their
    /// lifetime. With no inherited snapshot, read the active selection once at startup.
    pub fn read(root: &Path) -> RuntimeContractResult<Option<Self>> {
        let root = std::fs::canonicalize(root)
            .map_err(|_| RuntimeContractError::new(RuntimeContractCode::InstallRootUnavailable))?;
        let environment_root = std::env::var_os(INSTALL_ROOT_ENV);
        let environment_selection = std::env::var_os(INSTALL_SELECTION_ENV);
        let bytes = match (environment_root, environment_selection) {
            (Some(environment_root), Some(selection)) => {
                let environment_root = std::fs::canonicalize(environment_root).map_err(|_| {
                    RuntimeContractError::new(
                        RuntimeContractCode::InstallEnvironmentRootUnavailable,
                    )
                })?;
                if environment_root != root {
                    return Err(RuntimeContractError::new(
                        RuntimeContractCode::InstallEnvironmentRootMismatch,
                    ));
                }
                selection
                    .into_string()
                    .map_err(|_| {
                        RuntimeContractError::new(
                            RuntimeContractCode::InstallEnvironmentSelectionInvalid,
                        )
                    })?
                    .into_bytes()
            }
            (None, None) => return Self::read_active(&root),
            _ => {
                return Err(RuntimeContractError::new(
                    RuntimeContractCode::InstallEnvironmentIncomplete,
                ));
            }
        };
        Self::from_selection_bytes(&root, &bytes).map(Some)
    }

    /// acsetup's explicit current baseline for planning/CAS, independent of inherited
    /// process inputs. Running UI/MCP/Tools consumers keep their original `read` result.
    pub fn read_active(root: &Path) -> RuntimeContractResult<Option<Self>> {
        let root = std::fs::canonicalize(root)
            .map_err(|_| RuntimeContractError::new(RuntimeContractCode::InstallRootUnavailable))?;
        let file = match std::fs::File::open(root.join(INSTALL_SELECTION_PATH)) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => {
                return Err(RuntimeContractError::new(
                    SharedCode::InstallSelectionUnavailable,
                ));
            }
        };
        let bytes = read_install_input(file, MAX_INSTALL_SELECTION_BYTES)?;
        Self::from_selection_bytes(&root, &bytes).map(Some)
    }

    /// Verifies an explicit candidate or returned selection through the same bounded
    /// reader. The caller releases materialization's exclusive guard before this shared
    /// acquisition, then retains this object through candidate checking and commit.
    pub fn from_selection_bytes(root: &Path, bytes: &[u8]) -> RuntimeContractResult<Self> {
        let root = std::fs::canonicalize(root)
            .map_err(|_| RuntimeContractError::new(RuntimeContractCode::InstallRootUnavailable))?;
        let selection = InstallSelection::from_json(bytes)?;
        let slot_lock = Arc::new(InstallSlotLock::try_shared(&root, selection.slot)?);
        let selection_json = std::str::from_utf8(bytes).map_err(|_| {
            RuntimeContractError::new(RuntimeContractCode::InstallSelectionJsonInvalid)
        })?;
        let selected = Self {
            root,
            selection,
            selection_json: selection_json.to_owned(),
            _slot_lock: slot_lock,
        };
        selected.read_reference(&selected.selection.members, 4 * 1024 * 1024)?;
        selected.config_bytes()?;
        if let Some(provider) = &selected.selection.provider {
            selected.read_reference(provider, 1024 * 1024)?;
        }
        Ok(selected)
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
        let requested = std::path::absolute(requested).map_err(|_| {
            RuntimeContractError::new(RuntimeContractCode::InstallConfigPathInvalid)
        })?;
        let selected = self.config_path();
        let root_alias = requested
            .file_name()
            .is_some_and(|name| name == "actingd.config.json")
            && requested
                .parent()
                .and_then(|parent| std::fs::canonicalize(parent).ok())
                .as_ref()
                == Some(&self.root);
        let selected_canonical = std::fs::canonicalize(&selected).map_err(|_| {
            RuntimeContractError::new(RuntimeContractCode::InstallConfigUnavailable)
        })?;
        if root_alias
            || std::fs::canonicalize(&requested).ok().as_ref() == Some(&selected_canonical)
        {
            Ok(selected)
        } else {
            Err(RuntimeContractError::new(
                RuntimeContractCode::InstallConfigSelectionMismatch,
            ))
        }
    }

    fn read_reference(
        &self,
        reference: &InstallFileReference,
        limit: usize,
    ) -> RuntimeContractResult<Vec<u8>> {
        let path = std::fs::canonicalize(reference.resolve(&self.root)?)
            .map_err(|_| RuntimeContractError::new(RuntimeContractCode::InstallInputUnavailable))?;
        if !path.starts_with(&self.root) {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InstallInputOutsideRoot,
            ));
        }
        let file = std::fs::File::open(path)
            .map_err(|_| RuntimeContractError::new(RuntimeContractCode::InstallInputUnavailable))?;
        let bytes = read_install_input(file, limit)?;
        if format!("{:x}", Sha256::digest(&bytes)) != reference.sha256 {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InstallInputHashMismatch,
            ));
        }
        Ok(bytes)
    }
}

fn read_install_input(file: std::fs::File, limit: usize) -> RuntimeContractResult<Vec<u8>> {
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| RuntimeContractError::new(RuntimeContractCode::InstallInputReadFailed))?;
    if bytes.is_empty() || bytes.len() > limit {
        return Err(RuntimeContractError::new(
            RuntimeContractCode::InstallInputSizeInvalid,
        ));
    }
    Ok(bytes)
}

/// Reads the launcher snapshot (or direct executable's installation selection) once.
/// Reconnection and child calls retain it for this process's entire lifetime.
pub fn process_installation() -> RuntimeContractResult<Option<&'static InstalledProcess>> {
    static PIN: OnceLock<Result<Option<InstalledProcess>, crate::outcome::Code>> = OnceLock::new();
    PIN.get_or_init(|| {
        let own_slot = process_slot_lock().map_err(|error| error.code())?;
        let executable = std::env::current_exe()
            .map_err(|_| InstallationCode::InstallExecutableUnavailable.code())?;
        let executable = std::fs::canonicalize(executable)
            .map_err(|_| InstallationCode::InstallExecutableUnavailable.code())?;
        let program_root = executable.parent().and_then(Path::parent);
        let inherited_root = std::env::var_os(INSTALL_ROOT_ENV);
        let inherited = inherited_root.is_some();
        let root = if let Some(root) = inherited_root {
            Some(PathBuf::from(root))
        } else if std::env::var_os(INSTALL_SELECTION_ENV).is_some() {
            return Err(RuntimeContractCode::InstallEnvironmentIncomplete.code());
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
            return Err(SharedCode::InstallSelectionUnavailable.code());
        }
        // Workflow #359: the tools sit beside the slots in `<root>\tools`, admitted with an
        // inherited selection only. Started directly they find no selection, unless
        // `<root>\runtime\BUILD-MANIFEST.json` exists; then active.json is read and refused here.
        if let Some(selected) = &selected
            && !executable.starts_with(selected.program_root())
            && !(inherited && executable.starts_with(selected.root().join("tools")))
        {
            return Err(InstallationCode::InstallProcessSlotMismatch.code());
        }
        Ok(selected)
    })
    .as_ref()
    .map(Option::as_ref)
    .map_err(|code| RuntimeContractError::new(*code))
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
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidInstallFileReference,
            ));
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
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidInstallSelection,
            ));
        }
        self.members.validate()?;
        self.config.validate()?;
        if self.members.path != format!("{}/MEMBERS.json", self.slot.as_str()) {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InstallMembersSlotMismatch,
            ));
        }
        let generation = format!("install/generations/{}/", self.generation);
        if !self.config.path.starts_with(&generation) {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InstallConfigGenerationMismatch,
            ));
        }
        if let Some(provider) = &self.provider {
            provider.validate()?;
            if !provider.path.starts_with(&generation) || provider.path == self.config.path {
                return Err(RuntimeContractError::new(
                    RuntimeContractCode::InstallProviderGenerationMismatch,
                ));
            }
        }
        Ok(())
    }

    pub fn from_json(bytes: &[u8]) -> RuntimeContractResult<Self> {
        if bytes.is_empty() || bytes.len() > MAX_INSTALL_SELECTION_BYTES {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InstallSelectionSizeInvalid,
            ));
        }
        let selection: Self = serde_json::from_slice(bytes).map_err(|_| {
            RuntimeContractError::new(RuntimeContractCode::InstallSelectionJsonInvalid)
        })?;
        selection.validate()?;
        Ok(selection)
    }

    pub fn slot_root(&self, root: &Path) -> RuntimeContractResult<PathBuf> {
        self.validate()?;
        Ok(root.join(self.slot.as_str()))
    }
}

/// The phases of the installer's transition, the vocabulary `install_phase` of the outcome key
/// `phase`. Workflow #434 LAB-6S4a (0.12.63): the transition itself is gone with the request kinds
/// `InstallTransition`, `UpgradeState` and `UpgradeStateStatus` and actingd's held start
/// (`install-control` [0, 0]); the vocabulary stays, since the released codes that carry the key
/// keep their signature.
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

fn canonical_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
