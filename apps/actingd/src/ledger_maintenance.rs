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
    let (pairs, remaining) = arguments[2..].as_chunks::<2>();
    for pair in pairs {
        let slot = match pair[0].to_str() {
            Some("--config") => &mut config,
            Some("--backup") => &mut backup,
            Some("--target") => &mut target,
            Some("--artifact-root") => &mut artifact_root,
            _ => return Err(ActingdError::config(ActingdCode::MaintenanceOptionInvalid)),
        };
        if slot.is_some() || pair[1].is_empty() {
            return Err(ActingdError::config(ActingdCode::MaintenanceOptionInvalid));
        }
        *slot = Some(PathBuf::from(&pair[1]));
    }
    if !remaining.is_empty() {
        return Err(ActingdError::config(ActingdCode::MaintenanceOptionInvalid));
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
