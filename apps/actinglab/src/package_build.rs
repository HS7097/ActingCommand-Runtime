// SPDX-License-Identifier: AGPL-3.0-only

use super::{CliError, CliOutcome, FlagArgs, GlobalOptions};
use actingcommand_lab::{
    PackageBuildPackRequest, PackageBuildTaskRequest, PackageEnvOptions, PackageResolution,
    PackageSource,
};
use serde::Serialize;
use serde_json::Value;

pub(super) fn run_build_task(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    let game = flags.optional("--game").or_else(|| global.game.clone());
    let server = flags.optional("--server").or_else(|| global.server.clone());
    let request = PackageBuildTaskRequest {
        source: package_source(flags)?,
        task_id: flags.required("--task")?,
        game: game.clone(),
        server: server.clone(),
        locale: flags.optional("--locale"),
        package_id: flags.optional("--package-id"),
        execution_mode: flags.optional("--execution-mode"),
        resolution: parse_resolution(flags)?,
        include_recovery: flags.bool("--include-recovery"),
        out: flags.required_path("--out")?,
        dry_run: global.dry_run || flags.bool("--dry-run"),
        env: package_env(global, flags, game, server),
    };
    let mut lab = super::env_detection::build_readonly_lab()?;
    serialize_response(lab.package_build_task(request)?)
}

pub(super) fn run_build_pack(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    let game = flags.optional("--game").or_else(|| global.game.clone());
    let server = flags.optional("--server").or_else(|| global.server.clone());
    let request = PackageBuildPackRequest {
        source: package_source(flags)?,
        game: game.clone(),
        server: server.clone(),
        locale: flags.optional("--locale"),
        package_id: flags.optional("--package-id"),
        execution_mode: flags.optional("--execution-mode"),
        resolution: parse_resolution(flags)?,
        entry_task: flags.optional("--entry-task"),
        out: flags.optional_path("--out"),
        split_dir: flags.optional_path("--split-dir"),
        dry_run: global.dry_run || flags.bool("--dry-run"),
        env: package_env(global, flags, game, server),
    };
    let mut lab = super::env_detection::build_readonly_lab()?;
    serialize_response(lab.package_build_pack(request)?)
}

fn package_source(flags: &FlagArgs) -> CliOutcome<PackageSource> {
    match (
        flags.optional("--from-remote"),
        flags.optional_path("--repo"),
    ) {
        (Some(_), Some(_)) => Err(CliError::usage(
            "pass either --repo or --from-remote, not both",
        )),
        (Some(url), None) => Ok(PackageSource::Remote(url)),
        (None, Some(path)) => Ok(PackageSource::Local(path)),
        (None, None) => Err(CliError::usage(
            "missing --repo <path> or --from-remote <url>",
        )),
    }
}

fn package_env(
    global: &GlobalOptions,
    flags: &FlagArgs,
    game: Option<String>,
    server: Option<String>,
) -> PackageEnvOptions {
    PackageEnvOptions {
        instance: flags
            .optional("--instance")
            .or_else(|| global.instance.clone()),
        game,
        server,
        env_task: flags.optional("--env-task"),
    }
}

fn parse_resolution(flags: &FlagArgs) -> CliOutcome<Option<PackageResolution>> {
    let Some(value) = flags.optional("--resolution") else {
        return Ok(None);
    };
    let Some((width, height)) = value.split_once('x').or_else(|| value.split_once('X')) else {
        return Err(CliError::usage("--resolution must use <width>x<height>"));
    };
    let width = width
        .parse::<u32>()
        .map_err(|error| CliError::usage(format!("invalid resolution width: {error}")))?;
    let height = height
        .parse::<u32>()
        .map_err(|error| CliError::usage(format!("invalid resolution height: {error}")))?;
    Ok(Some(PackageResolution { width, height }))
}

fn serialize_response<T: Serialize>(response: T) -> CliOutcome<Value> {
    serde_json::to_value(response)
        .map_err(|error| CliError::device(format!("failed to serialize Lab response: {error}")))
}
