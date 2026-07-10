// SPDX-License-Identifier: AGPL-3.0-only

use super::{CliError, CliOutcome, FlagArgs, GlobalOptions, ResolvedResourceRoot};
use actingcommand_lab::ResourceConvertRequest;
use serde_json::Value;

pub(super) fn run_resource_convert(
    global: &GlobalOptions,
    flags: &FlagArgs,
    resource_root: &ResolvedResourceRoot,
) -> CliOutcome<Value> {
    let request = ResourceConvertRequest {
        repo: resource_root.input.clone(),
        game: flags.optional("--game").or_else(|| global.game.clone()),
        server: flags.optional("--server").or_else(|| global.server.clone()),
        locale: flags.optional("--locale"),
        maa_tasks_root: flags.optional_path("--maa-tasks"),
        dry_run: global.dry_run || flags.bool("--dry-run"),
    };
    let mut lab = super::env_detection::build_readonly_lab()?;
    serde_json::to_value(lab.resource_convert(request)?)
        .map_err(|error| CliError::device(format!("failed to serialize Lab response: {error}")))
}
