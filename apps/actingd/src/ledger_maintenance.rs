// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_runtime_host::{LedgerMaintenanceOperation, LedgerMaintenanceRequest};

pub(super) fn run(arguments: Vec<std::ffi::OsString>) -> Result<(), ActingdError> {
    if arguments.len() < 4 || arguments.len() > 10 {
        return Err(ActingdError::config("maintenance_usage_invalid"));
    }
    let operation = match arguments.get(1).and_then(|value| value.to_str()) {
        Some("dry-run") => LedgerMaintenanceOperation::DryRun,
        Some("import") => LedgerMaintenanceOperation::Import,
        Some("verify") => LedgerMaintenanceOperation::Verify,
        Some("backup") => LedgerMaintenanceOperation::Backup,
        Some("restore") => LedgerMaintenanceOperation::Restore,
        _ => return Err(ActingdError::config("maintenance_operation_invalid")),
    };
    let mut config = None;
    let mut backup = None;
    let mut target = None;
    let mut artifact_root = None;
    let mut remaining = arguments[2..].chunks_exact(2);
    for pair in &mut remaining {
        let slot = match pair[0].to_str() {
            Some("--config") => &mut config,
            Some("--backup") => &mut backup,
            Some("--target") => &mut target,
            Some("--artifact-root") => &mut artifact_root,
            _ => return Err(ActingdError::config("maintenance_option_invalid")),
        };
        if slot.is_some() || pair[1].is_empty() {
            return Err(ActingdError::config("maintenance_option_invalid"));
        }
        *slot = Some(PathBuf::from(&pair[1]));
    }
    if !remaining.remainder().is_empty() {
        return Err(ActingdError::config("maintenance_option_invalid"));
    }
    let config = config.ok_or_else(|| ActingdError::config("maintenance_config_missing"))?;
    let config = config::load(&config)
        .and_then(config::ActingdConfigFile::maintenance_config)
        .map_err(ActingdError::config)?;
    let request = LedgerMaintenanceRequest {
        operation,
        backup,
        target,
        artifact_root,
        limits: Default::default(),
    };
    match RuntimeHost::maintain_ledger(config, request) {
        Ok(receipt) => {
            let encoded = serde_json::to_string(&receipt)
                .map_err(|_| ActingdError::process("maintenance_receipt_encode_failed"))?;
            println!("{encoded}");
            Ok(())
        }
        Err(error) => {
            let encoded = serde_json::to_string(&serde_json::json!({ "schema_version": "actingcommand.ledger-maintenance.v1", "status": "failed", "error": &error, "activated": false })).map_err(|_| ActingdError::process("maintenance_receipt_encode_failed"))?;
            println!("{encoded}");
            Err(ActingdError::maintenance(error))
        }
    }
}
