// SPDX-License-Identifier: AGPL-3.0-only

use super::{CliError, CliOutcome, FlagArgs, GlobalOptions};
use actingcommand_contract::{EventActor, EventSource, RuntimeErrorCode};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig, RuntimeClientError};
use serde_json::Value;
use std::path::PathBuf;

pub(super) fn run(subcommand: &str, global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    if !flags.positionals.is_empty() {
        return Err(CliError::usage(
            "runtime commands do not accept positional arguments",
        ));
    }
    validate_runtime_flags(&flags)?;
    let state_root = PathBuf::from(flags.required("--state-root")?);
    let instance = flags
        .optional("--instance")
        .or_else(|| global.instance.clone())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| CliError::usage("missing --instance <value>"))?;
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        state_root,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .map_err(map_runtime_error)?;
    let output = match subcommand {
        "reset" => client.safe_reset(&instance).map_err(map_runtime_error)?,
        "observe" => client
            .observe_readonly(&instance)
            .map_err(map_runtime_error)?,
        _ => {
            return Err(CliError::usage(format!(
                "unknown runtime command: {subcommand}"
            )));
        }
    };
    serde_json::to_value(output)
        .map_err(|error| CliError::usage(format!("runtime output serialization failed: {error}")))
}

pub(super) fn map_runtime_error(error: RuntimeClientError) -> CliError {
    let unavailable = error.projection().is_none_or(|projection| {
        matches!(
            projection.code,
            RuntimeErrorCode::RuntimeUnavailable
                | RuntimeErrorCode::RuntimeFatal
                | RuntimeErrorCode::OwnerConflict
                | RuntimeErrorCode::ProtocolInvalid
                | RuntimeErrorCode::LedgerFailure
        )
    });
    if unavailable {
        CliError::runtime_not_running(error.to_string())
    } else {
        CliError::device(error.to_string())
    }
}

fn validate_runtime_flags(flags: &FlagArgs) -> CliOutcome<()> {
    let allowed = ["--state-root", "--instance"];
    if flags
        .flags
        .keys()
        .any(|name| !allowed.contains(&name.as_str()))
    {
        return Err(CliError::usage(
            "runtime commands accept only Runtime state and instance identity",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_commands_reject_capture_flags() {
        let flags = FlagArgs::parse(&[
            "--state-root".to_string(),
            "state".to_string(),
            "--instance".to_string(),
            "ak.cn".to_string(),
            "--serial".to_string(),
            "127.0.0.1:16416".to_string(),
        ])
        .expect("flags");
        assert!(validate_runtime_flags(&flags).is_err());
    }
}
