use super::{CliError, CliOutcome, ErrorKind, FlagArgs, RUNTIME_STATE_ROOT_ENV, SESSION_STATE_ENV};
use crate::cli_module::place;
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_lab_ops::cli_line;
use serde_json::json;
use std::{env, path::PathBuf};

pub(super) fn app_state_root() -> CliOutcome<PathBuf> {
    let root = env::var("LOCALAPPDATA")
        .or_else(|_| env::var("APPDATA"))
        .map_err(|_| CliError::usage("LOCALAPPDATA or APPDATA is required for ActingLab state"))?;
    Ok(PathBuf::from(root).join("ActingCommand").join("actinglab"))
}

/// The Runtime state root actinglab's own commands reach: `ACTINGCOMMAND_RUNTIME_STATE_ROOT` when
/// it is set (empty or white space is refused); else (#463 S1-8) the state root of the installation
/// the program runs from, as the CLI module's rows locate it (`cli_module::place`), refused as they
/// refuse it when it cannot be located ([`unlocated`]), so that an installed program never reaches
/// another state root than its installation's; else, from no installation, the default state root.
pub(super) fn runtime_state_root() -> CliOutcome<PathBuf> {
    if let Ok(path) = env::var(RUNTIME_STATE_ROOT_ENV) {
        if path.trim().is_empty() {
            return Err(CliError::usage(format!(
                "{RUNTIME_STATE_ROOT_ENV} must not be empty"
            )));
        }
        return Ok(PathBuf::from(path));
    }
    if let Some(located) = place::installation_state_root() {
        return located.map_err(|outcome| unlocated(&outcome));
    }
    let root = env::var("LOCALAPPDATA")
        .or_else(|_| env::var("APPDATA"))
        .map_err(|_| CliError::usage("LOCALAPPDATA or APPDATA is required for Runtime state"))?;
    Ok(PathBuf::from(root).join("ActingCommand").join("runtime"))
}

/// #463 S1-8: the refusal of an installation whose state root could not be located, as the CLI
/// module's rows refuse it: the located outcome's code, the text `cli_module::place::UNLOCATED`
/// and the outcome in the details.
fn unlocated(outcome: &Outcome) -> CliError {
    CliError::new(
        ErrorKind::UsageValidation,
        outcome.code().as_str(),
        place::UNLOCATED,
        &[],
    )
    .with_details(json!({"outcome": cli_line::outcome_value(outcome, Detail::Full)}))
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
