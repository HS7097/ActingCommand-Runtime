// SPDX-License-Identifier: AGPL-3.0-only

//! Installation inputs and the non-persistent Host installation protocol (Workflow #352).
//! Only acsetup commits the selection. Running processes retain the selection they started
//! with; these documents contain no Runtime state and are never a substitute for the ledger.

use crate::{RequestId, RuntimeContractError, RuntimeContractResult, RuntimeShutdownTarget};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

pub const INSTALL_SELECTION_SCHEMA: &str = "actingcommand.install-selection.v1";
pub const INSTALL_SELECTION_PATH: &str = "install/active.json";
pub const INSTALL_ROOT_ENV: &str = "ACTINGCOMMAND_INSTALL_ROOT";
pub const INSTALL_SELECTION_ENV: &str = "ACTINGCOMMAND_INSTALL_SELECTION";
pub const MAX_INSTALL_SELECTION_BYTES: usize = 16 * 1024;
pub const DEFAULT_INSTALL_TIMEOUT_MS: u64 = 60_000;
pub const MAX_INSTALL_TIMEOUT_MS: u64 = 600_000;

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

pub fn validate_install_timeout(timeout_ms: u64) -> RuntimeContractResult<()> {
    if timeout_ms == 0 || timeout_ms > MAX_INSTALL_TIMEOUT_MS {
        return Err(RuntimeContractError::new("invalid_install_timeout"));
    }
    Ok(())
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
