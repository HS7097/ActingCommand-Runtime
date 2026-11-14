// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #453 S12 (`contracts/state-root-installation.md`, "Handing a state root over"):
//! `actingd claim-state-root --config <file> --actor <initiator> [--new-identity]`, the explicit act
//! of a person that hands the configuration's state root over to the installation the command runs
//! from (the directory of `--config` when it holds the installer's own `install\state.json`). Of
//! the configuration file it uses `state_root` alone (loaded as `unlock-owner` loads it); it takes
//! the owner as a start does, refused while a Runtime holds it, and never opens the ledger.

use super::*;
use actingcommand_contract::OwnerUnlockActor;
use serde_json::json;

/// The report's schema: `{schema_version, outcome, …}`, the hand-over's members on success.
pub(super) const SCHEMA_VERSION: &str = "actingcommand.actingd.claim-state-root.v1";

/// The developer text of a command line this subcommand does not take.
const USAGE: &str = "actingd claim-state-root takes --config <file> and --actor <name>, each \
     once with a value, and the switch --new-identity at most once";

/// What the command line names.
struct Claim {
    config: PathBuf,
    actor: OwnerUnlockActor,
    new_identity: bool,
}

/// `actingd claim-state-root`. Its success is `state_root_claimed` (`path` the state root) with
/// the report members `actor`, `changed`, `identity` (`read`, `minted`, `renewed`), `record`
/// (`written`, `unchanged`, `replaced`), `claimed_at_unix_ms`, `previous` (`{claimed_at_unix_ms}`
/// of a replaced record that could be read) and `kept` (the files set aside). A command line it
/// does not take is `invalid_arguments`; the configuration's refusals are `unlock-owner`'s, at
/// stage `config_load`; every other refusal is the host's.
pub(super) fn run(arguments: Vec<std::ffi::OsString>) -> Result<Reported, ActingdError> {
    let claim = parse(&arguments)?;
    let host = config::load(&claim.config)
        .and_then(config::ActingdConfigFile::maintenance_config)
        .map_err(|code| {
            let mut error = ActingdError::config(code);
            error.stage = Some(ActingdLocation::ConfigLoad.location());
            error
        })?;
    let host = match actingcommand_runtime_host::installation_root_of_config(&claim.config)
        .map_err(ActingdError::runtime)?
    {
        Some(root) => host.with_installation_root(root),
        None => host,
    };
    let host = match option_env!("ACTINGCOMMAND_RUNTIME_HEAD") {
        Some(commit) => host.with_runtime_commit(commit),
        None => host,
    };
    let state_root = host.state_root().to_path_buf();
    let handed =
        RuntimeHost::claim_state_root(host, claim.new_identity).map_err(state_root_refusal)?;
    let kept = handed
        .kept
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let reported = Reported::new(Outcome::new(HostCode::StateRootClaimed).path(&state_root))
        .with("actor", json!(claim.actor.as_str()))
        .with("changed", json!(handed.changed()))
        .with("identity", json!(handed.identity.as_str()))
        .with("record", json!(handed.record.as_str()))
        .with("claimed_at_unix_ms", json!(handed.claimed_at_unix_ms))
        .with("kept", json!(kept));
    Ok(match handed.previous_claimed_at_unix_ms {
        Some(previous) => reported.with("previous", json!({"claimed_at_unix_ms": previous})),
        None => reported,
    })
}

/// The command line after `claim-state-root`: `--config` and `--actor` (a path-safe name, as
/// `unlock-owner`'s), each once with a non-empty value, and `--new-identity` at most once;
/// anything else is `invalid_arguments`.
fn parse(arguments: &[std::ffi::OsString]) -> Result<Claim, ActingdError> {
    let invalid =
        || ActingdError::config(SharedCode::InvalidArguments).with_detail(USAGE.to_owned());
    let mut config = None;
    let mut actor = None;
    let mut new_identity = false;
    let mut words = arguments.iter().skip(1);
    while let Some(word) = words.next() {
        let slot = match word.to_str() {
            Some("--new-identity") if !new_identity => {
                new_identity = true;
                continue;
            }
            Some("--config") => &mut config,
            Some("--actor") => &mut actor,
            _ => return Err(invalid()),
        };
        match words.next() {
            Some(value) if slot.is_none() && !value.is_empty() => *slot = Some(value),
            _ => return Err(invalid()),
        }
    }
    let config = PathBuf::from(config.ok_or_else(invalid)?);
    let actor = actor
        .ok_or_else(invalid)?
        .to_str()
        .and_then(|actor| OwnerUnlockActor::new(actor).ok())
        .ok_or_else(invalid)?;
    Ok(Claim {
        config,
        actor,
        new_identity,
    })
}
