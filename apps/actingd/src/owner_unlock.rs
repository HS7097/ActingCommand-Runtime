// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::OwnerUnlockActor;
use actingcommand_runtime_host::HostLocation;
use serde_json::json;

const UNLOCK_OWNER_SCHEMA_VERSION: &str = "actingcommand.actingd.unlock-owner.v1";

/// Offline maintenance for a state root whose last owner exited with device resources
/// unconfirmed: the operator's confirmation becomes that epoch's close evidence in the
/// owner journal, `owner.unlock` records it, and the next start takes the epoch over.
pub(super) fn run(arguments: Vec<std::ffi::OsString>) -> Result<(), ActingdError> {
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
    let loaded = config::load(&config_path).and_then(config::ActingdConfigFile::maintenance_config);
    let (report, result) = match loaded {
        Err(code) => (
            failed(UnlockFailure::Load(code), false),
            Err(ActingdError::config(code)),
        ),
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
                failed(UnlockFailure::Host(&failure), failure.journal_appended),
                Err(ActingdError::runtime(failure.error)),
            ),
        },
    };
    let encoded = serde_json::to_string(&report)
        .map_err(|_| ActingdError::process(ActingdCode::UnlockOwnerReportEncodeFailed))?;
    println!("{encoded}");
    result
}

/// The unlock-owner v1 token of a runtime host unlock stage (decision D12, as
/// [`v1_stage_token`]): `journal` up to and including the owner journal append, `ledger` for
/// the fact. A6a's unlock-owner v2 replaces these texts.
fn v1_unlock_stage_token(stage: Location) -> &'static str {
    [
        (HostLocation::UnlockOwnerFile, "journal"),
        (HostLocation::UnlockOwnerLedger, "ledger"),
    ]
    .into_iter()
    .find_map(|(location, token)| (location.location() == stage).then_some(token))
    .unwrap_or(stage.as_str())
}

/// What refused an unlock: loading the configuration, or the runtime host.
enum UnlockFailure<'a> {
    Load(Code),
    Host(&'a actingcommand_runtime_host::OwnerUnlockFailure),
}

fn failed(failure: UnlockFailure<'_>, journal_appended: bool) -> serde_json::Value {
    let (code, stage) = match failure {
        UnlockFailure::Load(code) => (
            code.as_str(),
            v1_stage_token(ActingdLocation::ConfigLoad.location()),
        ),
        UnlockFailure::Host(failure) => {
            (failure.error.code(), v1_unlock_stage_token(failure.stage))
        }
    };
    json!({
        "schema_version": UNLOCK_OWNER_SCHEMA_VERSION,
        "status": "failed",
        "error": { "code": code, "stage": stage },
        "journal_appended": journal_appended,
    })
}
