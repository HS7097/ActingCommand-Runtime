// SPDX-License-Identifier: AGPL-3.0-only

//! ROUT-L RL-1a (frozen model v1.0 §3.1, §3.2, §6.1 row RL-1a; contracts/standard-pack-schedule.md):
//! the Tools side of a game root's `schedule.json` (`actingcommand.schedule.v1`). Every reader
//! composes through the one function the Runtime runs, `compose_standard_pack_rules` of the policy
//! crate, so the Tools accept exactly what the Runtime accepts.
//!
//! - [`check_schedule`]: `package bundle --schedule` and `actinglab standard-pack check` compose
//!   the file with one bound instance per server of the root's registry (an alias of the check's
//!   own, `standard-pack-check.<server>`), so every rule, pool, profile and event is placed and
//!   compiled as the Runtime places it for one instance of each server. `requires` is reported, not
//!   checked: no Runtime manifest is at hand, and the Runtime that loads the rules checks it. A
//!   refusal is `standard_pack_root_invalid` naming `schedule.json`, its `standard_pack_problem` and
//!   JSON pointer; each unclassified package is one `standard_pack_package_unclassified` warning.
//! - [`compose_standard_packs`]: `actinglab standard-pack compose --root <standard_packs.root>
//!   --instance <alias>=<game>/<server>... --out <new directory>` composes the game roots of the
//!   bound games for those bindings and writes the four V2 documents (`tasks.json`, `pools.json`,
//!   `activity.json`, `timeline.json`, as compiled) and `manifest.json` (`procedure_ref` → game,
//!   package and directory) into `<out>.part`, renamed to `<out>` once all are written. It reads
//!   only the game roots and writes only that new directory.

use crate::standard_pack_check::{read_bounded, root_invalid};
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{Detail, IoOp, Outcome};
use actingcommand_contract::{
    BundleIndex, BundleMaintenance, LabError, LabErrorClass, LabResult, STANDARD_PACK_BUNDLE_FILE,
    STANDARD_PACK_DISPLAY_LAYOUT_FILE, STANDARD_PACK_INTERRUPTS_FILE,
    STANDARD_PACK_NAVIGATION_FILE, STANDARD_PACK_PAGES_FILE, STANDARD_PACK_SCHEDULE_FILE,
    StandardPackRoot, StandardPackRootBytes, read_standard_pack_root, validate_instance_alias,
};
use actingcommand_policy::{
    ComposedGame, ComposedProcedure, StandardPackBinding, StandardPackComposeFailure,
    StandardPackComposeInput, StandardPackComposition, StandardPackGameFiles,
    compose_standard_pack_rules,
};
use serde::Serialize;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// The alias prefix of the instances the check binds, one per server.
const CHECK_ALIAS_PREFIX: &str = "standard-pack-check.";
/// The catalog version of a composition the Tools make; only the Runtime's ledger numbers them.
const TOOLS_CATALOG_VERSION: u64 = 1;
/// The files `standard-pack compose` writes, besides `manifest.json`.
const COMPOSED_FILES: [&str; 4] = ["tasks.json", "pools.json", "activity.json", "timeline.json"];
/// The composition's manifest.
const MANIFEST_FILE: &str = "manifest.json";

/// `actinglab standard-pack compose`: the `standard_packs.root` that holds the game roots, the
/// bound instances, and the new output directory.
#[derive(Debug, Clone)]
pub struct StandardPackComposeRequest {
    pub root: PathBuf,
    /// Each bound instance: (alias, game, server).
    pub instances: Vec<(String, String, String)>,
    pub out: PathBuf,
}

/// The composition `standard-pack compose` wrote.
#[derive(Debug, Clone, Serialize)]
pub struct StandardPackComposeResponse {
    pub status: String,
    pub out: String,
    pub catalog_hash: String,
    /// `sha256:<hex>` over the composed elements and the manifest (model §3.2 step 5).
    pub rules_digest: String,
    pub tasks: usize,
    pub pools: usize,
    pub profiles: usize,
    pub events: usize,
    /// Every bound game, by game id, with its rules and its classification.
    pub games: Vec<ComposedGame>,
    pub manifest: Vec<ComposedProcedure>,
    /// One `standard_pack_package_unclassified` per unclassified package of a bound game.
    pub warnings: Vec<Outcome>,
}

/// Checks `schedule` (the bytes of the game root's `schedule.json` at `path`) against the game
/// root `root` of `game` and the bundle's `maintenance` entries (module doc): the unclassified
/// warnings, or the refusal.
pub(crate) fn check_schedule(
    game: &str,
    root: &StandardPackRoot,
    maintenance: &[BundleMaintenance],
    schedule: &[u8],
    path: &Path,
) -> LabResult<Vec<Outcome>> {
    let servers = root
        .registry
        .listed()
        .into_iter()
        .map(|(_, server)| server)
        .collect::<BTreeSet<_>>();
    let instances = servers
        .into_iter()
        .map(|server| {
            (
                format!("{CHECK_ALIAS_PREFIX}{server}"),
                StandardPackBinding {
                    game_id: game.to_owned(),
                    server_id: server,
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    let games = BTreeMap::from([(
        game.to_owned(),
        StandardPackGameFiles {
            schedule,
            root,
            maintenance,
        },
    )]);
    let composition = compose_standard_pack_rules(&StandardPackComposeInput {
        instances: &instances,
        games: &games,
        subsystems: None,
        catalog_version: TOOLS_CATALOG_VERSION,
    })
    .map_err(|failure| refused(&failure, game, &|_| path.to_owned()))?;
    Ok(composition.warnings())
}

/// `actinglab standard-pack compose` (module doc).
pub fn compose_standard_packs(
    request: StandardPackComposeRequest,
) -> LabResult<StandardPackComposeResponse> {
    let root = absolute(&request.root)?;
    if !fs::metadata(&root).is_ok_and(|metadata| metadata.is_dir()) {
        return Err(LabError::usage(format!(
            "--root {} is no directory",
            root.display()
        )));
    }
    if request.instances.is_empty() {
        return Err(LabError::usage("standard-pack compose needs --instance"));
    }
    let mut instances = BTreeMap::new();
    for (alias, game, server) in &request.instances {
        if validate_instance_alias(alias).is_err() || alias.chars().any(char::is_whitespace) {
            return Err(LabError::usage(format!(
                "--instance {alias:?}: an alias is 1 to 256 bytes without a control character or \
                 whitespace"
            )));
        }
        if game.is_empty() || server.is_empty() {
            return Err(LabError::usage(format!(
                "--instance {alias}: <alias>=<game>/<server> names a game and a server"
            )));
        }
        let binding = StandardPackBinding {
            game_id: game.clone(),
            server_id: server.clone(),
        };
        if instances.insert(alias.clone(), binding).is_some() {
            return Err(LabError::usage(format!(
                "--instance {alias} is given twice"
            )));
        }
    }
    let out = absolute(&request.out)?;
    let name = out
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| LabError::usage(format!("--out {} names no directory", out.display())))?;
    let staging = out.with_file_name(format!("{name}.part"));
    for path in [&out, &staging] {
        if fs::symlink_metadata(path).is_ok() {
            return Err(LabError::usage(format!(
                "{} already exists; standard-pack compose only writes a new directory",
                path.display()
            )));
        }
    }

    let bound = instances
        .values()
        .map(|binding| binding.game_id.clone())
        .collect::<BTreeSet<_>>();
    let mut roots = Vec::new();
    for game in &bound {
        roots.push(read_game_root(&root, game)?);
    }
    let games = roots
        .iter()
        .filter_map(|read| {
            read.schedule.as_ref().map(|schedule| {
                (
                    read.game.clone(),
                    StandardPackGameFiles {
                        schedule: schedule.as_slice(),
                        root: &read.root,
                        maintenance: read.maintenance.as_slice(),
                    },
                )
            })
        })
        .collect::<BTreeMap<_, _>>();
    let first_game = bound.first().cloned().unwrap_or_default();
    let composition = compose_standard_pack_rules(&StandardPackComposeInput {
        instances: &instances,
        games: &games,
        subsystems: None,
        catalog_version: TOOLS_CATALOG_VERSION,
    })
    .map_err(|failure| {
        refused(&failure, &first_game, &|game| {
            root.join(game).join(STANDARD_PACK_SCHEDULE_FILE)
        })
    })?;

    fs::create_dir(&staging).map_err(|error| write_failed(&staging, &error, IoOp::CreateDir))?;
    write_composition(&staging, &composition)
        .and_then(|()| {
            fs::rename(&staging, &out).map_err(|error| write_failed(&out, &error, IoOp::Rename))
        })
        .map_err(|mut error| {
            error.message = format!(
                "{}; the incomplete output remains at {}",
                error.message,
                staging.display()
            );
            error
        })?;
    let catalog = composition.compiled().catalog();
    Ok(StandardPackComposeResponse {
        status: "composed".to_owned(),
        out: out.display().to_string(),
        catalog_hash: composition.compiled().catalog_hash().to_owned(),
        rules_digest: composition.rules_digest().to_owned(),
        tasks: catalog.tasks.tasks.len(),
        pools: catalog.pools.pools.len(),
        profiles: catalog.activity.profiles.len(),
        events: catalog.timeline.events.len(),
        games: composition.games().to_vec(),
        manifest: composition.manifest().to_vec(),
        warnings: composition.warnings(),
    })
}

/// One bound game's root as the composer reads it.
struct GameRoot {
    game: String,
    root: StandardPackRoot,
    maintenance: Vec<BundleMaintenance>,
    /// `schedule.json`'s bytes; `None` when the root holds none (the game has no rules).
    schedule: Option<Vec<u8>>,
}

/// Reads `<root>/<game>/`: its root files by the one structure check of every reader, the
/// bundle's maintenance entries, and `schedule.json` when it holds one. A bound game without a
/// game root, or a root without `bundle.json`, is refused.
fn read_game_root(root: &Path, game: &str) -> LabResult<GameRoot> {
    let directory = root.join(game);
    if !fs::metadata(&directory).is_ok_and(|metadata| metadata.is_dir()) {
        return Err(LabError::usage(format!(
            "--instance names the game {game}, but {} is no game root",
            directory.display()
        )));
    }
    let mut files = BTreeMap::new();
    for name in [
        STANDARD_PACK_BUNDLE_FILE,
        STANDARD_PACK_PAGES_FILE,
        STANDARD_PACK_INTERRUPTS_FILE,
        STANDARD_PACK_NAVIGATION_FILE,
        // Workflow #392 DA-4: checked as every reader checks it.
        STANDARD_PACK_DISPLAY_LAYOUT_FILE,
    ] {
        if let Some(bytes) = read_bounded(&directory.join(name))? {
            files.insert(name, bytes);
        }
    }
    let bytes = |name: &str| files.get(name).map(Vec::as_slice);
    let checked = read_standard_pack_root(
        StandardPackRootBytes {
            bundle: bytes(STANDARD_PACK_BUNDLE_FILE),
            pages: bytes(STANDARD_PACK_PAGES_FILE),
            interrupts: bytes(STANDARD_PACK_INTERRUPTS_FILE),
            navigation: bytes(STANDARD_PACK_NAVIGATION_FILE),
            tables: false,
            display_layout: bytes(STANDARD_PACK_DISPLAY_LAYOUT_FILE),
        },
        game,
    )
    .map_err(|issue| root_invalid(game, &directory.join(issue.file), &issue))?
    .ok_or_else(|| {
        LabError::usage(format!(
            "{} holds no {STANDARD_PACK_BUNDLE_FILE}",
            directory.display()
        ))
    })?;
    let maintenance = bytes(STANDARD_PACK_BUNDLE_FILE)
        .and_then(|bundle| serde_json::from_slice::<BundleIndex>(bundle).ok())
        .map(|index| index.maintenance().to_vec())
        .unwrap_or_default();
    Ok(GameRoot {
        game: game.to_owned(),
        root: checked,
        maintenance,
        schedule: read_bounded(&directory.join(STANDARD_PACK_SCHEDULE_FILE))?,
    })
}

/// Writes the four composed documents and the manifest into `directory`.
fn write_composition(directory: &Path, composition: &StandardPackComposition) -> LabResult<()> {
    let sources = composition.sources();
    for (name, source) in COMPOSED_FILES.into_iter().zip([
        &sources.tasks,
        &sources.pools,
        &sources.activity,
        &sources.timeline,
    ]) {
        write_new(&directory.join(name), &source.bytes)?;
    }
    let mut manifest = serde_json::to_vec_pretty(&json!({
        "rules_digest": composition.rules_digest(),
        "procedures": composition.manifest(),
    }))
    .map_err(|error| LabError::usage(format!("the manifest does not serialize: {error}")))?;
    manifest.push(b'\n');
    write_new(&directory.join(MANIFEST_FILE), &manifest)
}

/// `standard_pack_root_invalid` for the first refusal of `failure`, at the `schedule.json` of its
/// game (`path_of`; `fallback_game` for a refusal of the composition as a whole). The developer
/// message lists every refusal.
fn refused(
    failure: &StandardPackComposeFailure,
    fallback_game: &str,
    path_of: &dyn Fn(&str) -> PathBuf,
) -> LabError {
    let Some(first) = failure.issues().first() else {
        return LabError::usage(format!("{failure} (no issue named)"));
    };
    let game = first.game_id.as_deref().unwrap_or(fallback_game);
    let mut error = root_invalid(game, &path_of(game), &first.issue);
    error.message = format!("{}; {failure}", error.message);
    error
}

fn absolute(path: &Path) -> LabResult<PathBuf> {
    std::path::absolute(path)
        .map_err(|error| LabError::usage(format!("{}: {error}", path.display())))
}

fn write_new(path: &Path, bytes: &[u8]) -> LabResult<()> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .and_then(|mut file| file.write_all(bytes))
        .map_err(|error| write_failed(path, &error, IoOp::Write))
}

/// `foreign_os_error` for a failed write of the output.
fn write_failed(path: &Path, error: &std::io::Error, op: IoOp) -> LabError {
    let outcome = Outcome::from_io_error(error, op).path(path);
    LabError::new(
        LabErrorClass::UsageValidation,
        ContractCode::ForeignOsError.as_str(),
        format!("{}: {error}", path.display()),
        &[],
    )
    .with_details(json!({"outcome": outcome.envelope(Detail::Full)}))
}
