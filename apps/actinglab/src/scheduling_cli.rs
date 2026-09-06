// SPDX-License-Identifier: AGPL-3.0-only

use crate::{CliError, CliOutcome, FlagArgs, GlobalOptions};
use actingcommand_lab::{
    EvaluationTime, SchedulingCatalogPaths, TimelineQueryContext, compile_scheduling_files,
    inspect_scheduling_timeline_files,
};
use serde_json::Value;

pub(crate) fn run_scheduling(
    sub: &str,
    global: &GlobalOptions,
    args: &[String],
) -> CliOutcome<Value> {
    // Four source paths and at most 4096 event IDs fit the catalog's 1 MiB budget.
    if args.len() > 8210 || args.iter().map(String::len).sum::<usize>() > 1_048_576 {
        return Err(CliError::usage(
            "scheduling arguments exceed 8210 tokens or 1 MiB",
        ));
    }
    if global.run_root.is_some()
        || global.instance.is_some()
        || global.instances.is_some()
        || global.profile.is_some()
        || global.resource_root.is_some()
        || global.dry_run
        || global.game.is_some()
        || global.server.is_some()
        || global.runtime_endpoint.is_some()
        || global.capture_backend.is_some()
        || global.touch_backend.is_some()
        || global.version
    {
        return Err(CliError::usage(
            "scheduling inspection accepts explicit file/query flags and output formatting only",
        ));
    }
    let flags = FlagArgs::parse_values(args)?;
    flags.expect_positionals("scheduling", 0)?;
    let allowed = match sub {
        "compile" => &["--tasks", "--pools", "--activity", "--timeline"][..],
        "timeline" => &[
            "--tasks",
            "--pools",
            "--activity",
            "--timeline",
            "--event-id",
            "--unix-ms",
            "--monotonic-ms",
            "--instance-id",
            "--server-id",
            "--game-id",
        ][..],
        _ => {
            return Err(CliError::usage(format!(
                "unknown scheduling command: {sub}"
            )));
        }
    };
    for (name, values) in &flags.flags {
        if !allowed.contains(&name.as_str())
            || (name != "--event-id" && values.len() != 1)
            || values.iter().any(String::is_empty)
        {
            return Err(CliError::usage(format!(
                "unexpected, duplicate or missing value for {name}"
            )));
        }
    }
    let required = |name: &str| {
        flags
            .optional(name)
            .ok_or_else(|| CliError::usage(format!("missing {name} <value>")))
    };
    let paths = SchedulingCatalogPaths {
        tasks: required("--tasks")?.into(),
        pools: required("--pools")?.into(),
        activity: required("--activity")?.into(),
        timeline: required("--timeline")?.into(),
    };
    match sub {
        "compile" => serde_json::to_value(compile_scheduling_files(&paths)?).map_err(|error| {
            CliError::usage(format!("failed to serialize scheduling response: {error}"))
        }),
        "timeline" => {
            let parse_time = |name| {
                required(name)?
                    .parse::<u64>()
                    .map_err(|error| CliError::usage(format!("invalid {name}: {error}")))
            };
            let time = EvaluationTime {
                unix_ms: parse_time("--unix-ms")?,
                monotonic_ms: parse_time("--monotonic-ms")?,
            };
            let context = TimelineQueryContext {
                instance_id: required("--instance-id")?,
                server_id: required("--server-id")?,
                game_id: required("--game-id")?,
            };
            serde_json::to_value(inspect_scheduling_timeline_files(
                &paths,
                time,
                &context,
                &flags.values("--event-id"),
            )?)
            .map_err(|error| {
                CliError::usage(format!("failed to serialize scheduling response: {error}"))
            })
        }
        _ => Err(CliError::usage(format!(
            "unknown scheduling command: {sub}"
        ))),
    }
}
