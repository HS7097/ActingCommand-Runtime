// SPDX-License-Identifier: AGPL-3.0-only

//! The rescue module's `instance add` (Workflow #421 INST-5, its first slice;
//! `contracts/install-cli.md`, "Configuration, instances, packs and a fresh root" and "The Lab's
//! rescue module"; register §1.300): one emulator instance composed into a configuration candidate,
//! which the module then commits as `config replace` commits a file (`crate::rescue`).
//!
//! - **What is asked** ([`Asked`]): the alias, the game and the server as the configuration names
//!   them, the emulator's index and the two backends, each required, none with a default; and
//!   (Workflow #452) the application id, optional, of the form the configuration's assembly takes
//!   ([`application_id_form`]).
//! - **The application id** (Workflow #452): the one given, else the one the game's root declares
//!   for the server (`<standard_packs.root>/<game>/applications.json`,
//!   `servers.<server>.application_id`, [`declared_application_id`]), the member the host's idle
//!   action reads; the entry records it and the answer says which (`application_id_from`, `given`
//!   or `declared`). A given id is written whether or not the root declares another.
//! - **What is refused before anything is written**, each `instance_invalid`: an alias the
//!   configuration has (an entry of `instances` or a row of `policy.facts.instances`), with `alias`;
//!   an emulator index an entry binds (`instance_index`), with `alias`, `instance_index` and the
//!   entry's alias as `aliases`; a game whose root under `standard_packs.root` holds no
//!   `bundle.json` the contract crate reads, with `game` and the file's `path` (and the read's
//!   failure as the cause), or a configuration without `standard_packs.root`, with `game` and the
//!   key; a server no pack of that bundle names, with `game`, `server` and `path`; (Workflow #452)
//!   no application id given and none the game's root declares for the server (no file, an
//!   unreadable one, no such member, or none of the form), with `game`, `server`, the file's `path`
//!   and the read's failure as the cause when there is one; a configuration without the list
//!   `instances` or `policy.facts.instances`, with the key as `config_key`; (Workflow #453 S1) one
//!   without `policy`, with `config_key` `policy`; a first row (`policy.facts.instances` empty)
//!   when `policy.resources.hosts` declares no host or several, with that key, or when the policy
//!   names a catalog, with `config_key` `policy.facts.instances` ([`first_row_source`]).
//! - **The candidate** ([`compose`]): the live file's document with the entry `{alias,
//!   instance_id, application_id, instance_index, touch_backend, capture_backend}` appended to
//!   `instances` and the row `{instance_id: <alias>, server_id, game_id, host_id, available: true,
//!   capability_operation_ids, preferred_task_ids: []}` appended to `policy.facts.instances`, the
//!   host and the capabilities those of a row of the same game and server, else of the same game,
//!   else of the first row; (Workflow #453 S1) with no row there, the one declared host and the
//!   composer's one operation ([`STANDARD_PACK_OPERATION_ID`]), and `capabilities_from` none; every
//!   other member as it was, in its order, with the live file's line ends. Until INST-R the
//!   instance is written in both places, as the configuration's assembly requires
//!   (`policy_instance_set_mismatch`).
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
//! - **Starting paused** (Workflow #453 S3; `contracts/scheduling-pause.md`, "Pauses held at a
//!   first start"): the candidate the module commits while no Runtime answers
//!   ([`compose_starting_paused`]) gives the new entry, and no other, the member `start_paused:
//!   true` ([`START_PAUSED`]), last; the Runtime's next start holds the instance with
//!   `instance.added`. The candidate for a running Runtime ([`compose`]) is as before: no entry
//!   gains the member, and the instance is paused after the start.

use super::files;
use super::host::Host;
use super::journal::{self, Operation};
use super::plan::Loaded;
use super::state::{CONFIG_FILE, Paths};
use crate::cli_line;
use crate::codes::{ArgumentProblem, InstallLocation, InstallRefusal};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    BundleIndex, INSTANCE_ADDED_PAUSE_REASON, IdentifierIssuer, MAX_INSTANCE_ALIAS_BYTES,
    STANDARD_PACK_BUNDLE_FILE, SchedulingPauseReason, validate_instance_alias,
};
use actingcommand_lab_ops::core_reads::Params;
use actingcommand_policy::STANDARD_PACK_OPERATION_ID;
use actingcommand_runtime_config::START_PAUSED;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The command of every outcome here.
pub(crate) const ADD: InstallLocation = InstallLocation::InstanceAdd;
/// The configuration's standard-pack root (`contracts/runtime-config.md`).
const ROOT_KEY: &str = "standard_packs.root";
/// The configuration's instance list.
const INSTANCES_KEY: &str = "instances";
/// The policy section's instance list, which INST-R will derive from the entries.
const POLICY_INSTANCES_KEY: &str = "policy.facts.instances";
/// Workflow #453 S1: the policy section, which a first row is written into.
const POLICY_KEY: &str = "policy";
/// Workflow #453 S1: the policy's declared hosts, whose one host a first row takes.
const HOSTS_KEY: &str = "policy.resources.hosts";
/// Workflow #452: the root file of a game's standard pack that declares its application per server
/// (`servers.<server>.application_id`), which the host's idle action reads.
const APPLICATIONS_FILE: &str = "applications.json";
/// How many ids are minted at most before the command gives up (each one is new: the issuer's
/// namespace is the clock, the process and a counter).
const MINT_ATTEMPTS: usize = 8;

/// The added instance's pause reason: the reason of its pause after the start (the coordinator's
/// ruling of 11-11) and of the Runtime's hold at a first start (Workflow #453 S2), one constant,
/// the contract crate's `INSTANCE_ADDED_PAUSE_REASON` (Workflow #453 S3; before it this module kept
/// its own copy).
pub(crate) fn pause_reason() -> Result<SchedulingPauseReason, Outcome> {
    SchedulingPauseReason::new(INSTANCE_ADDED_PAUSE_REASON)
        .map_err(|error| Outcome::new(error.code()))
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
    /// Workflow #452: the application id the caller gave, if any.
    pub(crate) application_id: Option<String>,
}

/// Workflow #452: an application id the configuration's assembly takes for an entry, exactly its
/// rule and no stricter one: actingd's assembly refuses a blank one
/// (`application_identity_missing`), and the device registration it builds one longer than
/// [`MAX_INSTANCE_ALIAS_BYTES`] bytes or with a control character (`invalid_application_identity`,
/// the registration's own check in `crates/runtime-host`).
fn application_id_form(text: &str) -> bool {
    !text.trim().is_empty()
        && text.len() <= MAX_INSTANCE_ALIAS_BYTES
        && !text.chars().any(char::is_control)
}

/// Where the written application id came from (Workflow #452).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ApplicationIdFrom {
    /// The caller's `--application-id`.
    Given,
    /// The game's root: `applications.json`, `servers.<server>.application_id`.
    Declared,
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
    /// The row's parameters (Workflow #434 LAB-5B1b: read through its constant): a missing one is
    /// `actingctl_usage_invalid` (`required`), one of another type `wrong_type`, an empty or
    /// malformed one `unusable`, an index above 65535 `out_of_range`. Workflow #452: the optional
    /// `application_id` absent is none; given, of another type `wrong_type`, of a form the
    /// configuration's assembly refuses ([`application_id_form`]) `unusable`.
    pub(crate) fn of(params: Params<'_>) -> Result<Self, Outcome> {
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
        let touch_backend = text("touch_backend", "--touch-backend")?;
        let capture_backend = text("capture_backend", "--capture-backend")?;
        let application_id = match params.get("application_id") {
            None => None,
            Some(Value::String(given)) if application_id_form(given) => Some(given.clone()),
            Some(Value::String(_)) => return Err(super::unusable("--application-id")),
            Some(_) => {
                return Err(cli_line::usage_invalid(
                    Some("--application-id"),
                    ArgumentProblem::WrongType,
                ));
            }
        };
        Ok(Self {
            alias,
            game,
            server,
            mumu_index,
            touch_backend,
            capture_backend,
            application_id,
        })
    }
}

/// What the command added: what was asked, the minted id, the application id written and where it
/// came from (Workflow #452), the alias whose policy row gave the host and the capabilities (none
/// for a first row, Workflow #453 S1), the reason the instance is paused with ([`pause_reason`]),
/// and (Workflow #453 S3) whether its entry starts paused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Added {
    pub(crate) asked: Asked,
    pub(crate) instance_id: String,
    pub(crate) application_id: String,
    pub(crate) application_id_from: ApplicationIdFrom,
    pub(crate) capabilities_from: Option<String>,
    pub(crate) pause_reason: SchedulingPauseReason,
    pub(crate) start_paused: bool,
}

/// Workflow #421 INST-5 fix 1: [`Added`] as a record, its pause reason the name type as the hold's
/// records keep a reason (`super::hold`). Workflow #452: and the application id with where it came
/// from, two additive members. Workflow #453 S1: `capabilities_from` is `null` for a first row.
/// Workflow #453 S3: `start_paused: true` when the entry carries the member (the candidate committed
/// while no Runtime answers), absent otherwise, so the record of a candidate for a running Runtime
/// is as before.
#[derive(Serialize)]
struct AddedRecord<'a> {
    alias: &'a str,
    instance_id: &'a str,
    game: &'a str,
    server: &'a str,
    instance_index: u16,
    touch_backend: &'a str,
    capture_backend: &'a str,
    capabilities_from: Option<&'a str>,
    pause_reason: &'a SchedulingPauseReason,
    application_id: &'a str,
    application_id_from: ApplicationIdFrom,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    start_paused: bool,
}

impl Added {
    /// As the journal (`inputs.instance_add`) and the answer (`added`) record it.
    pub(crate) fn value(&self) -> Value {
        super::hold::record_value(AddedRecord {
            alias: &self.asked.alias,
            instance_id: &self.instance_id,
            game: &self.asked.game,
            server: &self.asked.server,
            instance_index: self.asked.mumu_index,
            touch_backend: &self.asked.touch_backend,
            capture_backend: &self.asked.capture_backend,
            capabilities_from: self.capabilities_from.as_deref(),
            pause_reason: &self.pause_reason,
            application_id: &self.application_id,
            application_id_from: self.application_id_from,
            start_paused: self.start_paused,
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

/// The new row's host and capabilities, and the alias of the row they were taken from (none for a
/// first row, Workflow #453 S1).
struct RowSource {
    host_id: String,
    capabilities: Vec<Value>,
    capabilities_from: Option<String>,
}

/// The host and the capabilities of a row of `rows` (not empty) of the asked game and server,
/// else of the asked game, else of the first row; a row without them refuses with
/// `policy.facts.instances`.
fn sibling_source(rows: &[Value], asked: &Asked) -> Result<RowSource, Outcome> {
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
    Ok(RowSource {
        host_id: host_id.to_owned(),
        capabilities: capabilities.clone(),
        capabilities_from: Some(capabilities_from.to_owned()),
    })
}

/// Workflow #453 S1: the host and the capabilities of a first row (`policy.facts.instances`
/// empty), taken from the configuration alone. In catalog mode (`policy.catalog` present and not
/// `null`) the catalog, not the installer, says what an instance can run: refused with
/// `policy.facts.instances`, as before. In composer mode the host is the one
/// `policy.resources.hosts[].host_id` (no list, no host, several, or one without a `host_id`
/// refuses with `policy.resources.hosts`), the capabilities the composer's one operation, and no
/// row gave them.
fn first_row_source(config: &Value) -> Result<RowSource, Outcome> {
    if config
        .pointer("/policy/catalog")
        .is_some_and(|catalog| !catalog.is_null())
    {
        return Err(invalid().config_key(POLICY_INSTANCES_KEY));
    }
    let hosts = list(config, "/policy/resources/hosts", HOSTS_KEY)?;
    let [host] = hosts.as_slice() else {
        return Err(invalid().config_key(HOSTS_KEY));
    };
    let host_id = text_of(host, "host_id").ok_or_else(|| invalid().config_key(HOSTS_KEY))?;
    Ok(RowSource {
        host_id: host_id.to_owned(),
        capabilities: vec![Value::from(STANDARD_PACK_OPERATION_ID)],
        capabilities_from: None,
    })
}

/// Composes the candidate from the live configuration `loaded.config` (the selected one) for
/// `asked`, refusing (`instance_invalid`) what the module description lists: the candidate for a
/// running Runtime, whose entries gain no `start_paused`.
pub(crate) fn compose(
    host: &dyn Host,
    paths: &Paths,
    loaded: &Loaded,
    asked: &Asked,
) -> Result<Composed, Outcome> {
    composed(host, paths, loaded, asked, false)
}

/// Workflow #453 S3: the candidate [`compose`] composes, its new entry, and no other, carrying
/// `start_paused: true` as its last member: the candidate committed while no Runtime answers,
/// whose instance the Runtime's next start holds with `instance.added`.
pub(crate) fn compose_starting_paused(
    host: &dyn Host,
    paths: &Paths,
    loaded: &Loaded,
    asked: &Asked,
) -> Result<Composed, Outcome> {
    composed(host, paths, loaded, asked, true)
}

/// The candidate for `asked`; its new entry declares `start_paused: true` when `start_paused`.
fn composed(
    host: &dyn Host,
    paths: &Paths,
    loaded: &Loaded,
    asked: &Asked,
    start_paused: bool,
) -> Result<Composed, Outcome> {
    let config = &loaded.config;
    let instances = list(config, "/instances", INSTANCES_KEY)?;
    // Workflow #453 S1: without `policy` (absent or `null`, as the configuration's assembly reads
    // it) there is nothing to put a row in.
    if config.get(POLICY_KEY).is_none_or(Value::is_null) {
        return Err(invalid().config_key(POLICY_KEY));
    }
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
    let game_root = bound(paths, config, asked)?;
    // Workflow #452: the application id, the one given, else the one the game's root declares for
    // the server; with neither the command refuses here, before anything is written.
    let (application_id, application_id_from) = match &asked.application_id {
        Some(given) => (given.clone(), ApplicationIdFrom::Given),
        None => (
            declared_application_id(&game_root, asked)?,
            ApplicationIdFrom::Declared,
        ),
    };
    // The host and the capabilities: those of a row of the same game and server, else of the same
    // game, else of the first row; (Workflow #453 S1) with no row, a first row's.
    let RowSource {
        host_id,
        capabilities,
        capabilities_from,
    } = if rows.is_empty() {
        first_row_source(config)?
    } else {
        sibling_source(rows, asked)?
    };
    let instance_id = mint(&used_ids(host, paths, instances)?)?;
    let added = Added {
        asked: asked.clone(),
        instance_id,
        application_id,
        application_id_from,
        capabilities_from,
        pause_reason: pause_reason()?,
        start_paused,
    };
    let mut entry = json!({
        "alias": asked.alias,
        "instance_id": added.instance_id,
        "application_id": added.application_id,
        "instance_index": asked.mumu_index,
        "touch_backend": asked.touch_backend,
        "capture_backend": asked.capture_backend,
    });
    // Workflow #453 S3: the entry of a candidate committed while no Runtime answers starts paused;
    // the member comes last, after the six members of before.
    if start_paused && let Value::Object(members) = &mut entry {
        members.insert(START_PAUSED.to_owned(), Value::Bool(true));
    }
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
/// contract crate reads (its `game` the directory's), and the server one a pack of it names; the
/// game root.
fn bound(paths: &Paths, config: &Value, asked: &Asked) -> Result<PathBuf, Outcome> {
    let game = || invalid().game(asked.game.clone());
    let root = packs_root(paths, config).ok_or_else(|| game().config_key(ROOT_KEY))?;
    let game_root = root.join(&asked.game);
    let bundle = game_root.join(STANDARD_PACK_BUNDLE_FILE);
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
    Ok(game_root)
}

/// Workflow #452: the application id the game root `game_root` declares for the asked server, read
/// as the host's idle action reads it (`applications.json`, a leading byte-order mark skipped,
/// `servers.<server>.application_id`). That reader is the host's own (`crates/runtime-host`,
/// `host::idle`), which this crate does not depend on, and the contract crate has none: this is
/// the smallest reader of the same member. No file, an unreadable or non-JSON one, no such member,
/// or an id not of the form the configuration's assembly takes ([`application_id_form`]) refuses
/// `instance_invalid` with `game`, `server`, the file's `path` and the read's failure as the cause
/// when there is one.
fn declared_application_id(game_root: &Path, asked: &Asked) -> Result<String, Outcome> {
    let path = game_root.join(APPLICATIONS_FILE);
    let undeclared = || {
        invalid()
            .game(asked.game.clone())
            .server(asked.server.clone())
            .path(&path)
    };
    let bytes = files::read_all(&path).map_err(|cause| undeclared().caused_by(cause))?;
    let table: Value = serde_json::from_slice(files::without_bom(&bytes))
        .map_err(|error| undeclared().caused_by(files::json_failure(&error)))?;
    table
        .get("servers")
        .and_then(|servers| servers.get(asked.server.as_str()))
        .and_then(|server| server.get("application_id"))
        .and_then(Value::as_str)
        .filter(|declared| application_id_form(declared))
        .map(str::to_owned)
        .ok_or_else(undeclared)
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
