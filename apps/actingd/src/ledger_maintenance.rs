// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_runtime_host::{LedgerMaintenanceOperation, LedgerMaintenanceRequest};

/// Workflow #378 A6a (model section 7.4): the ledger-maintenance report, the receipt's members
/// with `outcome`; a failure is its outcome with `activated: false` (v1's `status: "failed"`
/// and `error`).
pub(super) const SCHEMA_VERSION: &str = "actingcommand.ledger-maintenance.v2";

pub(super) fn run(arguments: Vec<std::ffi::OsString>) -> Result<Reported, ActingdError> {
    if arguments.len() < 4 || arguments.len() > 10 {
        return Err(ActingdError::config(ActingdCode::MaintenanceUsageInvalid));
    }
    let operation = match arguments.get(1).and_then(|value| value.to_str()) {
        Some("dry-run") => LedgerMaintenanceOperation::DryRun,
        Some("import") => LedgerMaintenanceOperation::Import,
        Some("verify") => LedgerMaintenanceOperation::Verify,
        Some("backup") => LedgerMaintenanceOperation::Backup,
        Some("restore") => LedgerMaintenanceOperation::Restore,
        _ => {
            return Err(ActingdError::config(
                ActingdCode::MaintenanceOperationInvalid,
            ));
        }
    };
    let mut config = None;
    let mut backup = None;
    let mut target = None;
    let mut artifact_root = None;
    // #422 LN-7c V: `--frame-query`, a flag without a value, at most once; the host refuses it
    // for an operation other than `verify` and on a root that is not formal.
    let mut frame_query = false;
    let mut options = arguments[2..].iter();
    while let Some(option) = options.next() {
        let slot = match option.to_str() {
            Some("--frame-query") if !frame_query => {
                frame_query = true;
                continue;
            }
            Some("--config") => &mut config,
            Some("--backup") => &mut backup,
            Some("--target") => &mut target,
            Some("--artifact-root") => &mut artifact_root,
            _ => return Err(ActingdError::config(ActingdCode::MaintenanceOptionInvalid)),
        };
        match options.next() {
            Some(value) if slot.is_none() && !value.is_empty() => {
                *slot = Some(PathBuf::from(value))
            }
            _ => return Err(ActingdError::config(ActingdCode::MaintenanceOptionInvalid)),
        }
    }
    let config =
        config.ok_or_else(|| ActingdError::config(ActingdCode::MaintenanceConfigMissing))?;
    let purpose = if matches!(operation, LedgerMaintenanceOperation::Verify) {
        actingcommand_contract::InstallConfigPurpose::CandidateCheck
    } else {
        actingcommand_contract::InstallConfigPurpose::Running
    };
    let config = config::load_for(&config, purpose)
        .and_then(config::ActingdConfigFile::maintenance_config)
        .map_err(ActingdError::config)?;
    let request = LedgerMaintenanceRequest {
        operation,
        backup,
        target,
        artifact_root,
        limits: Default::default(),
        frame_query,
    };
    let receipt = RuntimeHost::maintain_ledger(config, request).map_err(|error| {
        ActingdError::maintenance(error).with_report("activated", serde_json::Value::Bool(false))
    })?;
    // Workflow #375 R5a: the referenced material a restore copied or skipped, on the developer
    // line.
    if let Some(material) = receipt.restored_material {
        write_stderr(&format!(
            "INFO actingd: {} restore artifacts_copied={} artifacts_evicted={} artifacts_absent={}\n",
            ActingdCode::LedgerMaintenanceCompleted.as_str(),
            material.copied,
            material.evicted,
            material.absent
        ));
    }
    // #422 LN-7c V: what `verify --frame-query` compared of the frame cleaner's statements and
    // the full view; without the flag there is nothing to print.
    if let Some(checked) = receipt.frame_query {
        write_stderr(&format!(
            "INFO actingd: {} verify frame_query_live={} frame_query_kept={} frame_query_accounted={}\n",
            ActingdCode::LedgerMaintenanceCompleted.as_str(),
            checked.live_frames,
            checked.kept_frames,
            checked.accounted
        ));
    }
    let data = match serde_json::to_value(&receipt) {
        Ok(serde_json::Value::Object(data)) => data,
        _ => {
            return Err(ActingdError::process(
                ActingdCode::MaintenanceReceiptEncodeFailed,
            ));
        }
    };
    Ok(Reported {
        outcome: Outcome::new(ActingdCode::LedgerMaintenanceCompleted),
        data,
    })
}
