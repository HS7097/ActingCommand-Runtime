// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::OwnerUnlockActor;
use serde_json::json;

/// Workflow #378 A6a (model section 7.4): unlock-owner v2, `{schema_version, outcome, …}`: the
/// receipt's members on success, `journal_appended` on a failure, whose outcome names the stage
/// that refused (v1's `error {code, stage}`).
pub(super) const SCHEMA_VERSION: &str = "actingcommand.actingd.unlock-owner.v2";

/// Offline maintenance for a state root whose last owner exited with device resources
/// unconfirmed: the operator's confirmation becomes that epoch's close evidence in the
/// owner journal, `owner.unlock` records it, and the next start takes the epoch over.
pub(super) fn run(arguments: Vec<std::ffi::OsString>) -> Result<Reported, ActingdError> {
    if arguments.len() > 6 {
        return Err(ActingdError::config(ActingdCode::UnlockOwnerUsageInvalid));
    }
    let mut config = None;
    let mut actor = None;
    let mut confirmed = false;
    let mut options = arguments[1..].iter();
    while let Some(option) = options.next() {
        let slot = match option.to_str() {
            Some("--confirm-resources-released") if !confirmed => {
                confirmed = true;
                continue;
            }
            Some("--config") => &mut config,
            Some("--actor") => &mut actor,
            _ => return Err(ActingdError::config(ActingdCode::UnlockOwnerOptionInvalid)),
        };
        match options.next() {
            Some(value) if slot.is_none() && !value.is_empty() => *slot = Some(value),
            _ => return Err(ActingdError::config(ActingdCode::UnlockOwnerOptionInvalid)),
        }
    }
    let config_path = PathBuf::from(
        config.ok_or_else(|| ActingdError::config(ActingdCode::UnlockOwnerConfigMissing))?,
    );
    let actor = actor
        .ok_or_else(|| ActingdError::config(ActingdCode::UnlockOwnerActorMissing))?
        .to_str()
        .and_then(|actor| OwnerUnlockActor::new(actor).ok())
        .ok_or_else(|| ActingdError::config(ActingdCode::UnlockOwnerActorInvalid))?;
    let host = config::load(&config_path)
        .and_then(config::ActingdConfigFile::maintenance_config)
        .map_err(|code| {
            let mut error = ActingdError::config(code);
            error.stage = Some(ActingdLocation::ConfigLoad.location());
            error.with_report("journal_appended", json!(false))
        })?;
    match RuntimeHost::unlock_owner(host, actor.clone(), confirmed) {
        Ok(receipt) => Ok(
            Reported::new(Outcome::new(ActingdCode::UnlockOwnerCompleted))
                .with("owner_epoch", json!(receipt.owner_epoch))
                .with(
                    "previous_resource_disposition",
                    json!(receipt.previous_resource_disposition),
                )
                .with("actor", json!(actor.as_str()))
                .with("revision", json!(receipt.revision)),
        ),
        // The host's unlock stages are locations (`unlock_owner_file` up to and including the
        // owner journal append, `unlock_owner_ledger` for the fact).
        Err(failure) => {
            let journal_appended = failure.journal_appended;
            let stage = failure.stage;
            let mut error = ActingdError::runtime(failure.error);
            error.stage = Some(stage);
            Err(error.with_report("journal_appended", json!(journal_appended)))
        }
    }
}
