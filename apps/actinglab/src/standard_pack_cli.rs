// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #373 NAV-T (M-373 v1.1 §3.9 second row, §10.1 NAV-4; wave-18 ruling 1):
//! `actinglab standard-pack check --root <game root>` runs, on an installed game root
//! `<standard_packs.root>/<game>/`, the checks `package bundle` runs on the root it lays out,
//! and lists the packs whose entry pages the page list does not hold. It only reads. ROUT-L
//! RL-1a: it checks the root's `schedule.json` too, by the Runtime's composer.
//!
//! ROUT-L RL-1a (contracts/standard-pack-schedule.md): `actinglab standard-pack compose --root
//! <standard_packs.root> --instance <alias>=<game>/<server>... --out <new directory>` composes
//! the schedule rules of the bound games as the Runtime composes them and writes the four V2
//! documents and the manifest. It writes a new directory, so `--dry-run` refuses it.

use crate::{CliError, CliOutcome, FlagArgs, GlobalOptions};
use actingcommand_resource_tooling::{
    StandardPackCheckRequest, StandardPackComposeRequest, check_standard_pack_root,
    compose_standard_packs,
};
use serde_json::Value;

pub(crate) fn options() -> [&'static str; 1] {
    ["--root <installed game root: <standard_packs.root>/<game>>"]
}

/// ROUT-L RL-1a: the flags of `standard-pack compose`.
pub(crate) fn compose_options() -> [&'static str; 3] {
    [
        "--root <standard_packs.root: the directory of the game roots>",
        "--instance <alias>=<game>/<server> (repeatable, once per bound instance)",
        "--out <new directory: tasks.json, pools.json, activity.json, timeline.json, manifest.json>",
    ]
}

pub(crate) fn run_standard_pack(
    sub: &str,
    global: &GlobalOptions,
    args: &[String],
) -> CliOutcome<Value> {
    match sub {
        "check" => {
            let flags = FlagArgs::parse_values(args)?;
            if let Some(flag) = flags.flags.keys().find(|flag| flag.as_str() != "--root") {
                return Err(CliError::usage(format!(
                    "standard-pack check does not accept {flag}"
                )));
            }
            flags.expect_positionals("standard-pack check", 0)?;
            let response = check_standard_pack_root(StandardPackCheckRequest {
                root: flags.required_path("--root")?,
            })?;
            serde_json::to_value(response).map_err(|error| {
                CliError::device(format!("failed to serialize the check result: {error}"))
            })
        }
        "compose" => {
            let flags = FlagArgs::parse_values(args)?;
            if let Some(flag) = flags
                .flags
                .keys()
                .find(|flag| !["--root", "--instance", "--out"].contains(&flag.as_str()))
            {
                return Err(CliError::usage(format!(
                    "standard-pack compose does not accept {flag}"
                )));
            }
            flags.expect_positionals("standard-pack compose", 0)?;
            for flag in ["--root", "--out"] {
                if flags.count(flag) > 1 {
                    return Err(CliError::usage(format!(
                        "standard-pack compose takes {flag} once"
                    )));
                }
            }
            let root = flags.required_path("--root")?;
            let out = flags.required_path("--out")?;
            let instances = flags
                .values("--instance")
                .iter()
                .map(|value| binding(value))
                .collect::<CliOutcome<Vec<_>>>()?;
            if instances.is_empty() {
                return Err(CliError::usage(
                    "missing --instance <alias>=<game>/<server>",
                ));
            }
            crate::dry_run_gate::refuse(
                global,
                "standard-pack compose",
                Some("standard-pack check"),
                Value::Null,
            )?;
            let response = compose_standard_packs(StandardPackComposeRequest {
                root,
                instances,
                out,
            })?;
            serde_json::to_value(response).map_err(|error| {
                CliError::device(format!("failed to serialize the composition: {error}"))
            })
        }
        other => Err(CliError::usage(format!(
            "unknown standard-pack command: {other}"
        ))),
    }
}

/// One `--instance <alias>=<game>/<server>`: the alias before the first `=`, the game and the
/// server around the `/` after it.
fn binding(value: &str) -> CliOutcome<(String, String, String)> {
    let parsed = value.split_once('=').and_then(|(alias, place)| {
        let (game, server) = place.split_once('/')?;
        (!alias.is_empty() && !game.is_empty() && !server.is_empty() && !server.contains('/'))
            .then(|| (alias.to_owned(), game.to_owned(), server.to_owned()))
    });
    parsed.ok_or_else(|| {
        CliError::usage(format!(
            "--instance {value}: expected <alias>=<game>/<server>"
        ))
    })
}

// Workflow #373 NAV-T (exec key 80), test row NT-3.
#[cfg(test)]
#[path = "tests/gate_373_navt.rs"]
mod gate_373_navt;
