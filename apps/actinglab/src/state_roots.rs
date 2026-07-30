use super::{CliError, CliOutcome, FlagArgs, RUNTIME_STATE_ROOT_ENV, SESSION_STATE_ENV};
use std::{env, path::PathBuf};

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
