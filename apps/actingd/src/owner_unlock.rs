// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::OwnerUnlockActor;
use serde_json::json;

const UNLOCK_OWNER_SCHEMA_VERSION: &str = "actingcommand.actingd.unlock-owner.v1";

/// Offline maintenance for a state root whose last owner exited with device resources
/// unconfirmed: the operator's confirmation becomes that epoch's close evidence in the
/// owner journal, `owner.unlock` records it, and the next start takes the epoch over.
pub(super) fn run(arguments: Vec<std::ffi::OsString>) -> Result<(), ActingdError> {
    if arguments.len() > 6 {
        return Err(ActingdError::config("unlock_owner_usage_invalid"));
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
            _ => return Err(ActingdError::config("unlock_owner_option_invalid")),
        };
        match options.next() {
            Some(value) if slot.is_none() && !value.is_empty() => *slot = Some(value),
            _ => return Err(ActingdError::config("unlock_owner_option_invalid")),
        }
    }
    let config_path =
        PathBuf::from(config.ok_or_else(|| ActingdError::config("unlock_owner_config_missing"))?);
    let actor = actor
        .ok_or_else(|| ActingdError::config("unlock_owner_actor_missing"))?
        .to_str()
        .and_then(|actor| OwnerUnlockActor::new(actor).ok())
        .ok_or_else(|| ActingdError::config("unlock_owner_actor_invalid"))?;
    let loaded = config::load(&config_path).and_then(config::ActingdConfigFile::maintenance_config);
    let (report, result) = match loaded {
        Err(code) => (failed(code, "load", false), Err(ActingdError::config(code))),
        Ok(host) => match RuntimeHost::unlock_owner(host, actor.clone(), confirmed) {
            Ok(receipt) => (
                json!({
                    "schema_version": UNLOCK_OWNER_SCHEMA_VERSION,
                    "status": "ok",
                    "owner_epoch": receipt.owner_epoch,
                    "previous_resource_disposition": receipt.previous_resource_disposition,
                    "actor": actor.as_str(),
                    "revision": receipt.revision,
                }),
                Ok(()),
            ),
            Err(failure) => (
                failed(
                    failure.error.code(),
                    failure.stage,
                    failure.journal_appended,
                ),
                Err(ActingdError::runtime(failure.error)),
            ),
        },
    };
    let encoded = serde_json::to_string(&report)
        .map_err(|_| ActingdError::process("unlock_owner_report_encode_failed"))?;
    println!("{encoded}");
    result
}

fn failed(code: &str, stage: &str, journal_appended: bool) -> serde_json::Value {
    json!({
        "schema_version": UNLOCK_OWNER_SCHEMA_VERSION,
        "status": "failed",
        "error": { "code": code, "stage": stage },
        "journal_appended": journal_appended,
    })
}
