// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #373 NAV-T (M-373 v1.1 §3.9 second row, §10.1 NAV-4; wave-18 ruling 1):
//! `actinglab standard-pack check --root <game root>` runs, on an installed game root
//! `<standard_packs.root>/<game>/`, the checks `package bundle` runs on the root it lays out,
//! and lists the packs whose entry pages the page list does not hold. It only reads.

use crate::{CliError, CliOutcome, FlagArgs};
use actingcommand_resource_tooling::{StandardPackCheckRequest, check_standard_pack_root};
use serde_json::Value;

pub(crate) fn options() -> [&'static str; 1] {
    ["--root <installed game root: <standard_packs.root>/<game>>"]
}

pub(crate) fn run(sub: &str, args: &[String]) -> CliOutcome<Value> {
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
        other => Err(CliError::usage(format!(
            "unknown standard-pack command: {other}"
        ))),
    }
}

// Workflow #373 NAV-T (exec key 80), test row NT-3.
#[cfg(test)]
#[path = "tests/gate_373_navt.rs"]
mod gate_373_navt;
