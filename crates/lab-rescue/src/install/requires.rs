// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #429 RB-1 (register §1.514; `contracts/install-cli.md`, "Update and rollback", the
//! plan's "Subsystems the schedules require"): whether the build an update or a rollback makes live
//! names every subsystem the schedules a start of it would read require. A `schedule.json` lists in
//! `requires` the configuration-manifest subsystems its rules need, and a Runtime that does not name
//! one refuses the file (`contracts/standard-pack-schedule.md`, "`requires`"); a start compiles the
//! active catalogue generation, which the scheduler workbench composed from such files and keeps
//! with each game's file as composed (`fragments/<game>.json` of its directory). The target's
//! subsystems come from the check-config answer the plan already asks it for; the schedules are
//! read from disk by this build's reader. Nothing is written and nothing is run; no game is named.

use super::files;
use super::place;
use super::programs::Checked;
use super::state::Paths;
use actingcommand_contract::outcome::{IoOp, Outcome};
use actingcommand_contract::{
    CATALOG_GENERATIONS_DIRECTORY, CATALOG_STORE_PATH, STANDARD_PACK_SCHEDULE_FILE,
    parse_standard_pack_schedule,
};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The plan's edge row of the check: its interface, the member of `schedule.json` it judges.
pub(super) const INTERFACE: &str = "requires";
/// The edge row's check: the target names every subsystem required.
pub(super) const CHECK: &str = "next_names_required";
/// The plan document's member that lists each schedule the target cannot read as written.
pub(super) const MEMBER: &str = "requires_missing";
/// A composed generation's folder of each game's `schedule.json` as composed, `<game>.json`
/// (`contracts/standard-pack-schedule.md`, "The scheduler workbench").
const FRAGMENTS_DIRECTORY: &str = "fragments";
/// The ending of a fragment's file name.
const FRAGMENT_SUFFIX: &str = ".json";
/// Where a check-config answer's report lists the subsystems its build names: one row each, with
/// its `name` and whether it is `enabled`.
const SUBSYSTEMS: &str = "/config_manifest/subsystems";
/// Where a check-config answer's report names the generation the ledger projects as active.
const ACTIVE_HASH: &str = "/policy_plan/active/catalog_hash";
/// The configuration's rows of the instances in use, each naming its game (`game_id`).
const INSTANCES: &str = "/policy/facts/instances";

/// Where a schedule the plan read lies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Held {
    /// A game root's `schedule.json` below `standard_packs.root`.
    Root,
    /// A fragment of a catalogue generation kept in the state root.
    Generation,
}

/// One schedule a start of the target would read whose `requires` lists a subsystem the target
/// does not name, or that could not be read: one entry of the plan's [`MEMBER`].
#[derive(Debug, Clone, Serialize)]
pub(super) struct Unnamed {
    /// The game; absent only for a generation whose folder of fragments could not be listed.
    #[serde(skip_serializing_if = "Option::is_none")]
    game: Option<String>,
    source: Held,
    /// The generation, for a fragment.
    #[serde(skip_serializing_if = "Option::is_none")]
    catalog_hash: Option<String>,
    path: String,
    /// The names its `requires` lists that the target does not name, in the file's order.
    missing: Vec<String>,
    /// Why the file or the folder could not be read: what it requires is then unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    unread: Option<Value>,
}

/// Every schedule a start of the target would read whose `requires` lists a subsystem the target's
/// check-config answer `target` does not name, or that could not be read: first the
/// `schedule.json` of each game in use below `standard_packs.root`, by game, then each fragment of
/// the active generation as the answers `target` and `live` preview it, by generation and game.
/// Empty when every name is named: the plan is then the plan it was.
pub(super) fn unnamed(
    paths: &Paths,
    config: &Value,
    state_root: &Path,
    target: &Checked,
    live: Option<&Checked>,
) -> Vec<Unnamed> {
    let named = subsystems_named(target);
    let mut found = Vec::new();
    // A configuration that names no standard-pack root has no game root a start would read.
    if let Ok(packs) = place::packs_root(paths, config) {
        for game in games_in_use(config) {
            let path = packs.join(&game).join(STANDARD_PACK_SCHEDULE_FILE);
            found.extend(judged(&named, Held::Root, game, None, &path));
        }
    }
    for catalog_hash in active_generations(target, live) {
        match fragments(state_root, &catalog_hash) {
            Ok(listed) => {
                for (game, path) in listed {
                    found.extend(judged(
                        &named,
                        Held::Generation,
                        game,
                        Some(catalog_hash.as_str()),
                        &path,
                    ));
                }
            }
            Err((directory, failure)) => found.push(Unnamed {
                game: None,
                source: Held::Generation,
                catalog_hash: Some(catalog_hash),
                path: files::plain(&directory),
                missing: Vec::new(),
                unread: Some(crate::cli_line::warning(&failure)),
            }),
        }
    }
    found
}

/// The entry of the schedule of `game` at `path`; `None` when it requires nothing unnamed.
fn judged(
    named: &BTreeSet<String>,
    source: Held,
    game: String,
    catalog_hash: Option<&str>,
    path: &Path,
) -> Option<Unnamed> {
    let (missing, unread) = match required(path) {
        Ok(names) => (
            names
                .into_iter()
                .filter(|name| !named.contains(name))
                .collect::<Vec<_>>(),
            None,
        ),
        Err(failure) => (Vec::new(), Some(crate::cli_line::warning(&failure))),
    };
    (!missing.is_empty() || unread.is_some()).then(|| Unnamed {
        game: Some(game),
        source,
        catalog_hash: catalog_hash.map(str::to_owned),
        path: files::plain(path),
        missing,
        unread,
    })
}

/// The names `requires` lists in the schedule at `path`, read by the one reader of
/// `schedule.json`: none when no file is there, and none for a file the reader refuses (the
/// Runtime's reader refuses it too, so no build composes it, and check-config notes it). A file
/// that is there and cannot be read is the failure.
fn required(path: &Path) -> Result<Vec<String>, Outcome> {
    match fs::read(path) {
        Ok(bytes) => Ok(parse_standard_pack_schedule(&bytes)
            .map(|schedule| schedule.requires)
            .unwrap_or_default()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(files::os_failure(path, &error, IoOp::Read)),
    }
}

/// The subsystems the build whose check-config answered `checked` names: the enabled rows of its
/// report's `config_manifest.subsystems`, the set a start of that build gives the composer. An
/// answer without them names none: a refusal, which prints no report, or a build before them.
fn subsystems_named(checked: &Checked) -> BTreeSet<String> {
    checked
        .report
        .pointer(SUBSYSTEMS)
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter(|row| row.get("enabled").and_then(Value::as_bool) == Some(true))
                .filter_map(|row| row.get("name").and_then(Value::as_str))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// The generation the ledger projects as active, as each check-config answer previews it
/// (`policy_plan.active`): the target's, and the live build's, which still answers when a target
/// refuses the state it would start on.
fn active_generations(target: &Checked, live: Option<&Checked>) -> BTreeSet<String> {
    std::iter::once(target)
        .chain(live)
        .filter_map(|checked| checked.report.pointer(ACTIVE_HASH).and_then(Value::as_str))
        .map(str::to_owned)
        .collect()
}

/// The games of the instances the configuration binds (`policy.facts.instances`), whose roots a
/// start reads; none without a `policy`.
fn games_in_use(config: &Value) -> BTreeSet<String> {
    config
        .pointer(INSTANCES)
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| row.get("game_id").and_then(Value::as_str))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// The fragments of the generation `catalog_hash` below `state_root`, by game, in the catalogue
/// store's layout (the contract crate's constants, which the runtime host's store keeps its
/// generations by): none for a hash that names no generation directory (not `sha256:` and 64
/// lowercase hex digits) and for a generation without fragments (one the workbench did not
/// compose); the folder and its failure when it cannot be listed.
fn fragments(
    state_root: &Path,
    catalog_hash: &str,
) -> Result<Vec<(String, PathBuf)>, (PathBuf, Outcome)> {
    let Some(hex) = catalog_hash
        .strip_prefix("sha256:")
        .filter(|hex| files::is_sha256_hex(hex))
    else {
        return Ok(Vec::new());
    };
    let directory = CATALOG_STORE_PATH
        .iter()
        .fold(state_root.to_path_buf(), |path, part| path.join(part))
        .join(CATALOG_GENERATIONS_DIRECTORY)
        .join(hex)
        .join(FRAGMENTS_DIRECTORY);
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(unlisted(&directory, &error)),
    };
    let mut listed = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| unlisted(&directory, &error))?;
        let file_name = entry.file_name();
        if let Some(game) = file_name
            .to_str()
            .and_then(|text| text.strip_suffix(FRAGMENT_SUFFIX))
        {
            listed.push((game.to_owned(), entry.path()));
        }
    }
    listed.sort();
    Ok(listed)
}

/// The folder `directory` that could not be listed, with the failure.
fn unlisted(directory: &Path, error: &io::Error) -> (PathBuf, Outcome) {
    (
        directory.to_path_buf(),
        files::os_failure(directory, error, IoOp::ReadDir),
    )
}
