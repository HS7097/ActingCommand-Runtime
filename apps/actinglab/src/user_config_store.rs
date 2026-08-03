// SPDX-License-Identifier: AGPL-3.0-only

use super::{CONFIG_ENV, CliError, CliOutcome, app_state_root};
use actingcommand_lab::UserConfig;
use std::{env, fs, path::PathBuf};

pub(super) fn read_user_config() -> CliOutcome<UserConfig> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(UserConfig::default());
    }
    let text = fs::read_to_string(&path).map_err(|err| {
        CliError::usage(format!(
            "failed to read config file {}: {err}",
            path.display()
        ))
    })?;
    serde_json::from_str(&text).map_err(|err| {
        CliError::usage(format!(
            "failed to parse config file {}: {err}",
            path.display()
        ))
    })
}

pub(super) fn write_user_config(config: &UserConfig) -> CliOutcome<()> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            CliError::usage(format!(
                "failed to create config directory {}: {err}",
                parent.display()
            ))
        })?;
    }
    let text = serde_json::to_string_pretty(config)
        .map_err(|err| CliError::usage(format!("failed to serialize config: {err}")))?;
    fs::write(&path, text)
        .map_err(|err| CliError::usage(format!("failed to write {}: {err}", path.display())))
}

pub(super) fn config_path() -> CliOutcome<PathBuf> {
    if let Ok(path) = env::var(CONFIG_ENV) {
        return Ok(PathBuf::from(path));
    }
    Ok(app_state_root()?.join("config.json"))
}
