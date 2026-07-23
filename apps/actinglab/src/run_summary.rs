// SPDX-License-Identifier: AGPL-3.0-only

use super::{
    CliError, CliOutcome, FlagArgs, GlobalOptions, create_error_report_zip, effective_run_root,
    list_runs, read_user_config, runtime_slice_cli,
};
use actingcommand_contract::{EventActor, EventSource, RunId};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use serde_json::{Value, json};
use std::path::PathBuf;

pub(super) fn dispatch(sub: &str, global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    match sub {
        "list" => list_runs(&run_root(global)?),
        "show" | "open" | "summary" | "export" => {
            let run_id = flags
                .positionals
                .first()
                .ok_or_else(|| CliError::usage(format!("run {sub} requires <run-id>")))?;
            if sub == "summary" {
                return summarize(run_id, &flags);
            }
            let run_root = run_root(global)?;
            if sub == "export" {
                let out = flags.required_path("--out")?;
                create_error_report_zip(&out, run_id, "run export placeholder")?;
                return Ok(json!({
                    "run_id": run_id,
                    "out": out.display().to_string()
                }));
            }
            Ok(json!({
                "run_id": run_id,
                "run_root": run_root.display().to_string(),
                "status": "reserved"
            }))
        }
        _ => Err(CliError::usage(format!("unknown run command: {sub}"))),
    }
}

fn run_root(global: &GlobalOptions) -> CliOutcome<PathBuf> {
    Ok(effective_run_root(global, &read_user_config()?)
        .unwrap_or_else(|| PathBuf::from("target").join("actinglab-runs")))
}

fn summarize(run_id: &str, flags: &FlagArgs) -> CliOutcome<Value> {
    if flags.positionals.len() != 1
        || flags
            .flags
            .keys()
            .any(|name| name.as_str() != "--state-root")
    {
        return Err(CliError::usage(
            "run summary accepts exactly <run-id> --state-root <path>",
        ));
    }
    let state_root = flags.required_path("--state-root")?;
    let run_id = serde_json::from_value::<RunId>(Value::String(run_id.to_owned()))
        .map_err(|_| CliError::usage("run summary requires a canonical typed run id"))?;
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        state_root,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .map_err(runtime_slice_cli::map_runtime_error)?;
    client
        .summarize_run(run_id)
        .map_err(runtime_slice_cli::map_runtime_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_noncanonical_run_identity_before_connect() {
        let flags = FlagArgs::parse(&[
            "not-a-run-id".to_string(),
            "--state-root".to_string(),
            "missing-runtime".to_string(),
        ])
        .expect("flags");
        let error = summarize("not-a-run-id", &flags).expect_err("noncanonical run id");
        assert_eq!(
            error.message,
            "run summary requires a canonical typed run id"
        );
    }
}
