// SPDX-License-Identifier: AGPL-3.0-only

//! The rescue module's `instance add` (Workflow #421 INST-5, its first slice;
//! `contracts/install-cli.md`, "Configuration, instances, packs and a fresh root" and "The Lab's
//! rescue module"; register §1.300): one emulator instance composed into a configuration candidate,
//! which the module then commits as `config replace` commits a file (`crate::rescue`).
//!
//! - **What is asked** ([`Asked`]): the alias, the game and the server as the configuration names
//!   them, the emulator's index and the two backends, each required, none with a default.
//! - **What is refused before anything is written**, each `instance_invalid`: an alias the
//!   configuration has (an entry of `instances` or a row of `policy.facts.instances`), with `alias`;
//!   an emulator index an entry binds (`instance_index`), with `alias`, `instance_index` and the
//!   entry's alias as `aliases`; a game whose root under `standard_packs.root` holds no
//!   `bundle.json` the contract crate reads, with `game` and the file's `path` (and the read's
//!   failure as the cause), or a configuration without `standard_packs.root`, with `game` and the
//!   key; a server no pack of that bundle names, with `game`, `server` and `path`; a configuration
//!   without the list `instances` or `policy.facts.instances`, or without a row there to take the
//!   host and the capabilities from, with the key as `config_key`.
//! - **The candidate** ([`compose`]): the live file's document with the entry `{alias,
//!   instance_id, instance_index, touch_backend, capture_backend}` appended to `instances` and the
//!   row `{instance_id: <alias>, server_id, game_id, host_id, available: true,
//!   capability_operation_ids, preferred_task_ids: []}` appended to `policy.facts.instances`, the
//!   host and the capabilities those of a row of the same game and server, else of the same game,
//!   else of the first row; every other member as it was, in its order, with the live file's line
//!   ends. Until INST-R the instance is written in both places, as the configuration's assembly
//!   requires (`policy_instance_set_mismatch`).
//! - **The instance id** is minted (`instance_<32 hex>`, the contract crate's issuer) and checked
//!   against every `instance_id` of the live file and every one a journal of this root recorded as
//!   added (`inputs.instance_add`): it is never one of them. A journal that cannot be read counts
//!   for nothing, as for every reader of the journals; an operations folder that cannot be listed
//!   refuses.
//! - **Where it is written** ([`stage`]): beside the live file, `actingd.config.json.new-<ms>.part`
//!   (`<ms>` the command's own milliseconds), so that check-config resolves its relative paths as
//!   the live file's; the swap takes it as its part and nothing else is staged. A candidate refused
//!   afterwards (check-config, a Runtime gone) stays there, kept like every part; nothing deletes
//!   it, and `leftovers` lists it.

use super::files;
use super::host::Host;
use super::journal::{self, Operation};
use super::plan::Loaded;
use super::state::{CONFIG_FILE, Paths};
use crate::cli_line;
use crate::codes::{ArgumentProblem, InstallLocation, InstallRefusal};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    BundleIndex, IdentifierIssuer, STANDARD_PACK_BUNDLE_FILE, SchedulingPauseReason,
    validate_instance_alias,
};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The command of every outcome here.
pub(crate) const ADD: InstallLocation = InstallLocation::InstanceAdd;
/// The reason of the added instance's own pause, set after the start (the coordinator's ruling of
/// 11-11): a pause reason name (`contracts/scheduling-pause.md`, 1 to 64 bytes of `[a-z0-9_.-]`).
pub(crate) const PAUSE_REASON: &str = "instance.added";
/// The configuration's standard-pack root (`contracts/runtime-config.md`).
const ROOT_KEY: &str = "standard_packs.root";
/// The configuration's instance list.
const INSTANCES_KEY: &str = "instances";
/// The policy section's instance list, which INST-R will derive from the entries.
const POLICY_INSTANCES_KEY: &str = "policy.facts.instances";
/// How many ids are minted at most before the command gives up (each one is new: the issuer's
/// namespace is the clock, the process and a counter).
const MINT_ATTEMPTS: usize = 8;

/// The added instance's pause reason.
pub(crate) fn pause_reason() -> Result<SchedulingPauseReason, Outcome> {
    SchedulingPauseReason::new(PAUSE_REASON).map_err(|error| Outcome::new(error.code()))
}

/// What the caller asked for: the row's parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Asked {
    pub(crate) alias: String,
    pub(crate) game: String,
    pub(crate) server: String,
    pub(crate) mumu_index: u16,
    pub(crate) touch_backend: String,
    pub(crate) capture_backend: String,
}

/// A byte of a game or server word.
fn identifier_byte(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
}

/// A game or server word: 1 to 128 bytes of `[a-z0-9._-]`, one path segment (the bundle's rule).
fn identifier(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 128
        && text != "."
        && text != ".."
        && text.bytes().all(identifier_byte)
}

impl Asked {
    /// The row's parameters: a missing one is `actingctl_usage_invalid` (`required`), one of
    /// another type `wrong_type`, an empty or malformed one `unusable`, an index above 65535
    /// `out_of_range`.
    pub(crate) fn of(params: &Map<String, Value>) -> Result<Self, Outcome> {
        let text = |name: &str, shown: &str| -> Result<String, Outcome> {
            match params.get(name) {
                None => Err(cli_line::usage_invalid(
                    Some(shown),
                    ArgumentProblem::Required,
                )),
                Some(Value::String(given)) if !given.is_empty() => Ok(given.clone()),
                Some(Value::String(_)) => Err(super::unusable(shown)),
                Some(_) => Err(cli_line::usage_invalid(
                    Some(shown),
                    ArgumentProblem::WrongType,
                )),
            }
        };
        let alias = text("alias", "<alias>")?;
        if validate_instance_alias(&alias).is_err() {
            return Err(super::unusable("<alias>"));
        }
        let game = text("game", "--game")?;
        if !identifier(&game) {
            return Err(super::unusable("--game"));
        }
        let server = text("server", "--server")?;
        if !identifier(&server) {
            return Err(super::unusable("--server"));
        }
        let mumu_index = match params.get("mumu_index") {
            None => {
                return Err(cli_line::usage_invalid(
                    Some("--mumu-index"),
                    ArgumentProblem::Required,
                ));
            }
            Some(value) => {
                let index = value.as_u64().ok_or_else(|| {
                    cli_line::usage_invalid(Some("--mumu-index"), ArgumentProblem::WrongType)
                })?;
                u16::try_from(index).map_err(|_| {
                    cli_line::usage_invalid(Some("--mumu-index"), ArgumentProblem::OutOfRange)
                        .range_min(0_u64)
                        .range_max(u64::from(u16::MAX))
                })?
            }
        };
        Ok(Self {
            alias,
            game,
            server,
            mumu_index,
            touch_backend: text("touch_backend", "--touch-backend")?,
            capture_backend: text("capture_backend", "--capture-backend")?,
        })
    }
}

/// What the command added: what was asked, the minted id and the alias whose policy row gave the
/// host and the capabilities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Added {
    pub(crate) asked: Asked,
    pub(crate) instance_id: String,
    pub(crate) capabilities_from: String,
}

impl Added {
    /// As the journal (`inputs.instance_add`) and the answer (`added`) record it.
    pub(crate) fn value(&self) -> Value {
        json!({
            "alias": self.asked.alias,
            "instance_id": self.instance_id,
            "game": self.asked.game,
            "server": self.asked.server,
            "instance_index": self.asked.mumu_index,
            "touch_backend": self.asked.touch_backend,
            "capture_backend": self.asked.capture_backend,
            "capabilities_from": self.capabilities_from,
            "pause_reason": PAUSE_REASON,
        })
    }
}

/// The candidate: what it adds and its bytes.
pub(crate) struct Composed {
    pub(crate) added: Added,
    pub(crate) bytes: Vec<u8>,
}

/// `instance_invalid` of `instance add`.
fn invalid() -> Outcome {
    super::refusal(ADD, InstallRefusal::InstanceInvalid)
}

/// The members of `config` at `pointer` as a list; a missing or other member refuses with `key`.
fn list<'a>(config: &'a Value, pointer: &str, key: &str) -> Result<&'a Vec<Value>, Outcome> {
    config
        .pointer(pointer)
        .and_then(Value::as_array)
        .ok_or_else(|| invalid().config_key(key))
}

fn text_of<'a>(entry: &'a Value, member: &str) -> Option<&'a str> {
    entry.get(member).and_then(Value::as_str)
}

/// Composes the candidate from the live configuration `loaded.config` (the selected one) for
/// `asked`, refusing (`instance_invalid`) what the module description lists.
pub(crate) fn compose(
    host: &dyn Host,
    paths: &Paths,
    loaded: &Loaded,
    asked: &Asked,
) -> Result<Composed, Outcome> {
    let config = &loaded.config;
    let instances = list(config, "/instances", INSTANCES_KEY)?;
    let rows = list(config, "/policy/facts/instances", POLICY_INSTANCES_KEY)?;
    // The alias: neither place may have it.
    if instances
        .iter()
        .any(|entry| text_of(entry, "alias") == Some(asked.alias.as_str()))
        || rows
            .iter()
            .any(|row| text_of(row, "instance_id") == Some(asked.alias.as_str()))
    {
        return Err(invalid().alias(asked.alias.clone()));
    }
    // The emulator's index: no entry may bind it.
    let holders = instances
        .iter()
        .filter(|entry| {
            entry.get("instance_index").and_then(Value::as_u64) == Some(u64::from(asked.mumu_index))
        })
        .map(|entry| text_of(entry, "alias").unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    if !holders.is_empty() {
        return Err(invalid()
            .alias(asked.alias.clone())
            .instance_index(asked.mumu_index)
            .aliases(holders));
    }
    // The game and the server: the installation's packages must bind them.
    bound(paths, config, asked)?;
    // The host and the capabilities: those of a row of the same game and server, else of the same
    // game, else of the first row.
    let same = |row: &&Value, server: bool| {
        text_of(row, "game_id") == Some(asked.game.as_str())
            && (!server || text_of(row, "server_id") == Some(asked.server.as_str()))
    };
    let sibling = rows
        .iter()
        .find(|row| same(row, true))
        .or_else(|| rows.iter().find(|row| same(row, false)))
        .or_else(|| rows.first())
        .ok_or_else(|| invalid().config_key(POLICY_INSTANCES_KEY))?;
    let (Some(capabilities_from), Some(host_id), Some(capabilities)) = (
        text_of(sibling, "instance_id"),
        text_of(sibling, "host_id"),
        sibling
            .get("capability_operation_ids")
            .and_then(Value::as_array),
    ) else {
        return Err(invalid().config_key(POLICY_INSTANCES_KEY));
    };
    let instance_id = mint(&used_ids(host, paths, instances)?)?;
    let added = Added {
        asked: asked.clone(),
        instance_id,
        capabilities_from: capabilities_from.to_owned(),
    };
    let entry = json!({
        "alias": asked.alias,
        "instance_id": added.instance_id,
        "instance_index": asked.mumu_index,
        "touch_backend": asked.touch_backend,
        "capture_backend": asked.capture_backend,
    });
    let row = json!({
        "instance_id": asked.alias,
        "server_id": asked.server,
        "game_id": asked.game,
        "host_id": host_id,
        "available": true,
        "capability_operation_ids": capabilities,
        "preferred_task_ids": [],
    });
    let mut document = config.clone();
    for (pointer, member) in [("/instances", entry), ("/policy/facts/instances", row)] {
        if let Some(Value::Array(items)) = document.pointer_mut(pointer) {
            items.push(member);
        }
    }
    let live = files::read_all(&paths.config()).map_err(|cause| {
        super::refusal(ADD, InstallRefusal::ConfigUnreadable)
            .path(paths.config())
            .caused_by(cause)
    })?;
    Ok(Composed {
        added,
        bytes: encoded(&document, live.windows(2).any(|pair| pair == b"\r\n"))?,
    })
}

/// The document as pretty JSON with a final line end, `\r\n` when the live file used it (a JSON
/// text holds no line end inside a string, so every `\n` is one).
fn encoded(document: &Value, crlf: bool) -> Result<Vec<u8>, Outcome> {
    let mut bytes =
        serde_json::to_vec_pretty(document).map_err(|error| files::json_failure(&error))?;
    bytes.push(b'\n');
    if !crlf {
        return Ok(bytes);
    }
    let mut converted = Vec::with_capacity(bytes.len() + bytes.len() / 16);
    for byte in bytes {
        if byte == b'\n' {
            converted.push(b'\r');
        }
        converted.push(byte);
    }
    Ok(converted)
}

/// The standard-pack root the configuration names, a relative one against the root.
fn packs_root(paths: &Paths, config: &Value) -> Option<PathBuf> {
    let root = actingcommand_runtime_config::member(config, ROOT_KEY)
        .and_then(Value::as_str)
        .filter(|root| !root.is_empty())?;
    Some(if Path::new(root).is_absolute() {
        PathBuf::from(root)
    } else {
        paths.root.join(root)
    })
}

/// The game is a game root of the configuration's standard-pack root whose `bundle.json` the
/// contract crate reads (its `game` the directory's), and the server one a pack of it names.
fn bound(paths: &Paths, config: &Value, asked: &Asked) -> Result<(), Outcome> {
    let game = || invalid().game(asked.game.clone());
    let root = packs_root(paths, config).ok_or_else(|| game().config_key(ROOT_KEY))?;
    let bundle = root.join(&asked.game).join(STANDARD_PACK_BUNDLE_FILE);
    let unbound = || game().path(&bundle);
    let bytes = files::read_all(&bundle).map_err(|cause| unbound().caused_by(cause))?;
    let index: BundleIndex = serde_json::from_slice(files::without_bom(&bytes))
        .map_err(|error| unbound().caused_by(files::json_failure(&error)))?;
    index
        .validate()
        .map_err(|error| unbound().caused_by(Outcome::new(error.code())))?;
    if index.game != asked.game {
        return Err(unbound());
    }
    if !index.packs.iter().any(|pack| pack.server == asked.server) {
        return Err(unbound().server(asked.server.clone()));
    }
    Ok(())
}

/// Every instance id the live file names and every one a journal of the root recorded as added.
fn used_ids(
    host: &dyn Host,
    paths: &Paths,
    instances: &[Value],
) -> Result<BTreeSet<String>, Outcome> {
    let mut used = instances
        .iter()
        .filter_map(|entry| text_of(entry, "instance_id").map(str::to_owned))
        .collect::<BTreeSet<_>>();
    let ids = journal::operation_ids(paths).map_err(|cause| {
        super::refusal(ADD, InstallRefusal::StateInvalid)
            .path(paths.operations())
            .caused_by(cause)
    })?;
    for id in ids {
        let Ok(operation) = Operation::load(host, paths, &id, ADD) else {
            continue;
        };
        if let Some(added) = operation
            .journal()
            .inputs
            .get(super::config_swap::ADDED_INPUT)
            .and_then(|added| text_of(added, "instance_id"))
        {
            used.insert(added.to_owned());
        }
    }
    Ok(used)
}

/// A minted instance id that `used` does not hold.
fn mint(used: &BTreeSet<String>) -> Result<String, Outcome> {
    let issuer =
        IdentifierIssuer::new().map_err(|error| invalid().caused_by(Outcome::new(error.code())))?;
    for _ in 0..MINT_ATTEMPTS {
        let issued = issuer
            .mint_instance_id()
            .map_err(|error| invalid().caused_by(Outcome::new(error.code())))?;
        let Ok(Value::String(id)) = serde_json::to_value(issued.transport()) else {
            return Err(invalid().config_key(INSTANCES_KEY));
        };
        if !used.contains(&id) {
            return Ok(id);
        }
    }
    Err(invalid().config_key(INSTANCES_KEY))
}

/// Writes the candidate's `bytes` beside the live file as `actingd.config.json.new-<ms>.part`, a
/// name no file has (`<ms>` the host's clock, the next free one); the file's name.
pub(crate) fn stage(host: &dyn Host, paths: &Paths, bytes: &[u8]) -> Result<String, Outcome> {
    let mut ms = host.now_unix_ms();
    loop {
        let name = format!("{CONFIG_FILE}.new-{ms}.part");
        if files::create_new(&paths.root.join(&name), bytes)? {
            return Ok(name);
        }
        ms += 1;
    }
}
