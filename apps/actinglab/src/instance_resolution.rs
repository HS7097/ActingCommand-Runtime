// SPDX-License-Identifier: AGPL-3.0-only

use super::{CliError, CliOutcome, FlagArgs, GlobalOptions};
use actingcommand_lab::UserConfig;

pub(super) fn resolve_instance_id(global: &GlobalOptions, config: &UserConfig) -> CliOutcome<String> {
    if let Some(instance) = &global.instance {
        return Ok(instance.clone());
    }
    if let Some((id, _instance)) = config.instances.iter().find(|(_id, instance)| {
        let game_match = global
            .game
            .as_ref()
            .is_none_or(|game| instance.game.as_ref() == Some(game));
        let server_match = global
            .server
            .as_ref()
            .is_none_or(|server| instance.server.as_ref() == Some(server));
        game_match && server_match
    }) {
        return Ok(id.clone());
    }
    Err(CliError::instance(
        "could not resolve instance; pass --instance or configure instance.<id>.game/server",
    ))
}

pub(super) fn resolve_instance_id_for_flags(
    global: &GlobalOptions,
    config: &UserConfig,
    flags: &FlagArgs,
) -> CliOutcome<String> {
    if let Some(instance) = flags.optional("--instance").filter(|value| value != "true") {
        return Ok(instance);
    }
    resolve_instance_id(global, config)
}
