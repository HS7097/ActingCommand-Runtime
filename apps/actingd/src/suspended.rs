// SPDX-License-Identifier: AGPL-3.0-only

//! `actingd suspended --config <path>` (Workflow #336 L6, §12.8): the read-only list of the
//! scheduled pairs a failure paused, those a change of a package's content lifted (Workflow #336
//! PKG-S3: the running daemon lifts them at its next check, without a restart), and those failing
//! again and again without accumulating.
//! Loads and assembles the configuration exactly as startup does, then reads the state root's
//! ledger read-only; nothing is written and no lock is taken, so it runs beside the daemon.

use super::*;
use actingcommand_contract::codes::ContractLocation;
use actingcommand_runtime_host::{SuspensionReportRequest, suspension_report};

/// Workflow #378 A6a (model section 7.4): suspended v2, the report's members with `outcome`
/// (`suspended_reported`); a failure is its outcome alone (v1's `status`, `code`, `operation`
/// and `detail`).
pub(super) const SCHEMA_VERSION: &str = "actingcommand.actingd.suspended.v2";

pub(super) fn run(arguments: Vec<std::ffi::OsString>) -> Result<Reported, ActingdError> {
    if arguments.len() > 3 {
        return Err(ActingdError::config(ActingdCode::SuspendedUsageInvalid));
    }
    let mut config = None;
    let (pairs, remaining) = arguments[1..].as_chunks::<2>();
    for pair in pairs {
        if pair[0] != "--config" || config.is_some() || pair[1].is_empty() {
            return Err(ActingdError::config(ActingdCode::SuspendedOptionInvalid));
        }
        config = Some(PathBuf::from(&pair[1]));
    }
    if !remaining.is_empty() {
        return Err(ActingdError::config(ActingdCode::SuspendedOptionInvalid));
    }
    let config_path =
        config.ok_or_else(|| ActingdError::config(ActingdCode::SuspendedConfigMissing))?;
    let RuntimeAssembly { host, policy, .. } = config::load(&config_path)
        .map_err(ActingdError::config)?
        .assemble_or_refusal()
        .map_err(|(code, refusal)| ActingdError::refused(code, refusal))?;
    // The file's modification time tells whether the running daemon read this configuration.
    let config_modified_unix_ms = std::fs::metadata(&config_path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok());
    let Some(policy) = policy.as_ref() else {
        let code = SharedCode::SuspendedPolicyUnconfigured;
        return Err(ActingdError::refused(
            code,
            Outcome::new(code).operation(ContractLocation::ReadSuspensionConfig),
        ));
    };
    let report = suspension_report(&SuspensionReportRequest {
        config_path: &config_path,
        config_modified_unix_ms,
        host: &host,
        catalog: &policy.catalog,
    })
    .map_err(|error| ActingdError::refused(error.code(), error.outcome().clone()))?;
    match report {
        serde_json::Value::Object(data) => Ok(Reported {
            outcome: Outcome::new(ActingdCode::SuspendedReported),
            data,
        }),
        _ => Err(ActingdError::process(
            ActingdCode::SuspendedReportEncodeFailed,
        )),
    }
}
