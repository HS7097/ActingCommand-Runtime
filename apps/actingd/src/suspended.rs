// SPDX-License-Identifier: AGPL-3.0-only

//! `actingd suspended --config <path> [--deadline-ms <ms>]` (Workflow #336 L6, §12.8; RL-1c fix 1
//! adds the deadline, which bounds the ledger read; RL-1c follow-up #411 L2: without it, a
//! default deadline): the read-only list of the
//! scheduled pairs a failure paused, those a change of a package's content lifted (Workflow #336
//! PKG-S3: the running daemon lifts them at its next check, without a restart), and those failing
//! again and again without accumulating.
//! Loads and assembles the configuration exactly as startup does, then reads the state root's
//! ledger read-only; nothing is written and no lock is taken, so it runs beside the daemon.
//! ROUT-L RL-1c (#411): in composer mode (a `policy` without `catalog`) the pairs are judged
//! against the composition the scheduler workbench makes for the configured instances.

use super::*;
use actingcommand_contract::codes::ContractLocation;
use actingcommand_runtime_host::{
    ComposedSuspensionReportRequest, SuspendedReadBudget, SuspensionReportRequest,
    compose_configured_standard_packs, composed_suspension_report_within, suspension_report_within,
};

/// Workflow #378 A6a (model section 7.4): suspended v2, the report's members with `outcome`
/// (`suspended_reported`); a failure is its outcome alone (v1's `status`, `code`, `operation`
/// and `detail`).
pub(super) const SCHEMA_VERSION: &str = "actingcommand.actingd.suspended.v2";

/// RL-1c follow-up (#411 L2, W1-9): how long a run without `--deadline-ms` (a run by hand) reads
/// the ledger, from the process's start; it then answers with what it read and
/// `ledger_read_incomplete`, as a run under the caller's deadline does. By the tunables rule a
/// configuration key with this default; it stays a constant until the `actingd-config` revision
/// that adds the key (none in 0.12 wave 1).
pub(super) const DEFAULT_READ_MS: u64 = 60_000;

/// The options of one run: `--config <path>`, and how long the ledger read may take from the
/// process's start (`--deadline-ms`, else [`DEFAULT_READ_MS`]).
pub(super) struct Options {
    pub(super) config: Option<PathBuf>,
    pub(super) read_for: std::time::Duration,
}

/// The run's options, or the refusal of its arguments (usage, then an option).
pub(super) fn options(arguments: &[std::ffi::OsString]) -> Result<Options, ActingdError> {
    // At most `--config <path>` and `--deadline-ms <ms>`; a word after a whole option is usage.
    if arguments.len() > 5 || (arguments.len() > 3 && arguments.len().is_multiple_of(2)) {
        return Err(ActingdError::config(ActingdCode::SuspendedUsageInvalid));
    }
    let mut config = None;
    let mut deadline_ms = None;
    let (pairs, remaining) = arguments[1..].as_chunks::<2>();
    for pair in pairs {
        if pair[0] == "--config" && config.is_none() && !pair[1].is_empty() {
            config = Some(PathBuf::from(&pair[1]));
        } else if pair[0] == "--deadline-ms" && deadline_ms.is_none() {
            // RL-1c fix 1 (#411 H1): the caller's deadline; the ledger read stops there and the
            // report answers with what it read and `ledger_read_incomplete`.
            deadline_ms = Some(
                pair[1]
                    .to_str()
                    .and_then(|text| text.parse::<u64>().ok())
                    .ok_or_else(|| ActingdError::config(ActingdCode::SuspendedOptionInvalid))?,
            );
        } else {
            return Err(ActingdError::config(ActingdCode::SuspendedOptionInvalid));
        }
    }
    if !remaining.is_empty() {
        return Err(ActingdError::config(ActingdCode::SuspendedOptionInvalid));
    }
    Ok(Options {
        config,
        // RL-1c follow-up (#411 L2): a run by hand reads under a deadline too.
        read_for: std::time::Duration::from_millis(deadline_ms.unwrap_or(DEFAULT_READ_MS)),
    })
}

pub(super) fn run(arguments: Vec<std::ffi::OsString>) -> Result<Reported, ActingdError> {
    // The read's deadline counts from this process's start.
    let started = std::time::Instant::now();
    let Options { config, read_for } = options(&arguments)?;
    let budget = SuspendedReadBudget::until(started.checked_add(read_for));
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
    let report = match policy.catalog.as_ref() {
        Some(catalog) => suspension_report_within(
            &SuspensionReportRequest {
                config_path: &config_path,
                config_modified_unix_ms,
                host: &host,
                catalog,
            },
            budget,
        ),
        // ROUT-L RL-1c (#411; board 6435964241): composer mode names no catalog; the report reads
        // what the configuration schedules, the composition the scheduler workbench makes for the
        // configured instances.
        None => {
            let schedule =
                compose_configured_standard_packs(&host).map_err(ActingdError::runtime)?;
            composed_suspension_report_within(
                &ComposedSuspensionReportRequest {
                    config_path: &config_path,
                    config_modified_unix_ms,
                    schedule: &schedule,
                },
                budget,
            )
        }
    }
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
