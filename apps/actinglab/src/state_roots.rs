use super::{CliError, CliOutcome, FlagArgs, RUNTIME_STATE_ROOT_ENV, SESSION_STATE_ENV};
use std::sync::OnceLock;
use std::{env, path::PathBuf};

/// Workflow #434 LAB-2: the state root the CLI module located from the selected configuration
/// (`contracts/lab-two-entries.md` §9.3), which the commands it runs use when
/// `ACTINGCOMMAND_RUNTIME_STATE_ROOT` is not set.
static LOCATED_STATE_ROOT: OnceLock<PathBuf> = OnceLock::new();

/// Workflow #434 LAB-2: the commands of this process use the located state root `path`; the
/// first call decides.
pub(super) fn use_located_state_root(path: PathBuf) {
    let _ = LOCATED_STATE_ROOT.set(path);
}

pub(super) fn app_state_root() -> CliOutcome<PathBuf> {
    let root = env::var("LOCALAPPDATA")
        .or_else(|_| env::var("APPDATA"))
        .map_err(|_| CliError::usage("LOCALAPPDATA or APPDATA is required for ActingLab state"))?;
    Ok(PathBuf::from(root).join("ActingCommand").join("actinglab"))
}

pub(super) fn runtime_state_root() -> CliOutcome<PathBuf> {
    if let Ok(path) = env::var(RUNTIME_STATE_ROOT_ENV) {
        if path.trim().is_empty() {
            return Err(CliError::usage(format!(
                "{RUNTIME_STATE_ROOT_ENV} must not be empty"
            )));
        }
        return Ok(PathBuf::from(path));
    }
    if let Some(path) = LOCATED_STATE_ROOT.get() {
        return Ok(path.clone());
    }
    let root = env::var("LOCALAPPDATA")
        .or_else(|_| env::var("APPDATA"))
        .map_err(|_| CliError::usage("LOCALAPPDATA or APPDATA is required for Runtime state"))?;
    Ok(PathBuf::from(root).join("ActingCommand").join("runtime"))
}

pub(super) fn session_state_dir_from_flags(flags: &FlagArgs) -> CliOutcome<PathBuf> {
    if let Some(path) = flags.optional_path("--state-dir") {
        return Ok(path);
    }
    if let Ok(path) = env::var(SESSION_STATE_ENV) {
        return Ok(PathBuf::from(path));
    }
    Ok(app_state_root()?.join("session"))
}
