// SPDX-License-Identifier: AGPL-3.0-only

//! `actingd suspended --config <path>` (Workflow #336 L6, §12.8): the read-only list of the
//! scheduled pairs a failure paused, those a package update lifted (and whether the running
//! daemon has read that update yet), and those failing again and again without accumulating.
//! Loads and assembles the configuration exactly as startup does, then reads the state root's
//! ledger read-only; nothing is written and no lock is taken, so it runs beside the daemon.

use super::*;
use actingcommand_runtime_host::{SuspensionReportRequest, suspension_report};
use serde_json::json;

const SUSPENDED_SCHEMA_VERSION: &str = "actingcommand.actingd.suspended.v1";

pub(super) fn run(arguments: Vec<std::ffi::OsString>) -> Result<(), ActingdError> {
    if arguments.len() > 3 {
        return Err(ActingdError::config("suspended_usage_invalid"));
    }
    let mut config = None;
    let (pairs, remaining) = arguments[1..].as_chunks::<2>();
    for pair in pairs {
        if pair[0] != "--config" || config.is_some() || pair[1].is_empty() {
            return Err(ActingdError::config("suspended_option_invalid"));
        }
        config = Some(PathBuf::from(&pair[1]));
    }
    if !remaining.is_empty() {
        return Err(ActingdError::config("suspended_option_invalid"));
    }
    let config_path = config.ok_or_else(|| ActingdError::config("suspended_config_missing"))?;
    let RuntimeAssembly { host, policy, .. } = config::load(&config_path)
        .and_then(config::ActingdConfigFile::assemble)
        .map_err(ActingdError::config)?;
    // The file's modification time tells whether the running daemon read this configuration.
    let config_modified_unix_ms = std::fs::metadata(&config_path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok());
    let report = match policy.as_ref() {
        Some(policy) => suspension_report(&SuspensionReportRequest {
            config_path: &config_path,
            config_modified_unix_ms,
            host: &host,
            catalog: &policy.catalog,
        })
        .map_err(|error| {
            (
                error.code(),
                error.operation(),
                error.detail().map(str::to_owned),
            )
        }),
        None => Err((
            "suspended_policy_unconfigured",
            "read_suspension_config",
            None,
        )),
    };
    match report {
        Ok(report) => {
            let encoded = serde_json::to_string(&report)
                .map_err(|_| ActingdError::process("suspended_report_encode_failed"))?;
            println!("{encoded}");
            Ok(())
        }
        Err((code, operation, detail)) => {
            let encoded = serde_json::to_string(&json!({
                "schema_version": SUSPENDED_SCHEMA_VERSION,
                "status": "failed",
                "code": code,
                "operation": operation,
                "detail": detail,
            }))
            .map_err(|_| ActingdError::process("suspended_report_encode_failed"))?;
            println!("{encoded}");
            let error = ActingdError::config(code);
            Err(match detail {
                Some(detail) => error.with_detail(detail),
                None => error,
            })
        }
    }
}
