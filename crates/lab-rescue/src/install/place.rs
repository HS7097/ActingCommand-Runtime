// SPDX-License-Identifier: AGPL-3.0-only

//! The rescue module's `place` (Workflow #434 LAB-3; `contracts/lab-two-entries.md` §7;
//! `contracts/install-cli.md`, "The Lab's rescue module", `place`): one game's standard-package
//! bundle zip placed into the configuration's standard-pack root, `<standard_packs.root>\<game>`,
//! while the Runtime runs or (Workflow #453 S6) while it is stopped. The game comes from the
//! bundle's `bundle.json`; no game is named here.
//!
//! - **The plan** (read-only): the zip's SHA-256 (against `SHA256SUMS` when given),
//!   `bundle.json` as the contract crate validates it, the package interface the bundle declares
//!   (`interfaces.json`, its maximum within the running installer's `package` range; Workflow
//!   #444 NOTICES-2: checked before the layout, so a bundle of a later revision is refused through
//!   `package`; Workflow #451 PLACE-PKG: read by the bundle's own rule, its schema, every value
//!   under `speaks` a pair and `speaks.package` named), its layout (the root files,
//!   `packs/<listed id>/…`, `tables/<listed id>.json`, the notices directory `notices/…`, held to
//!   the contract crate's notices rules, and, Workflow #451 LOAD-P, the loading templates
//!   directory `loading/<file>`, exactly the templates the zip's own `loading.json` declares), the
//!   root files as every reader checks them
//!   ([`read_standard_pack_root`]), each pack's file count and size, and per item `new`,
//!   `replace`, `unchanged` or, for a reader's root file, the notices directory or the loading
//!   templates directory the zip no longer carries, `retire`. The notices directory is one item
//!   of kind `notices`, placed first; the loading templates directory one item of kind `loading`,
//!   placed after the packs and before the root files. The plan id is `plan-` and the first 16
//!   hex digits of the SHA-256 of the canonical plan document.
//! - **The operation** (`kind` `pack_place`, journalled) while a Runtime runs: the hold of the
//!   instances bound to the bundle's game (`pause`), then each item that changes (`pack_item`:
//!   written to
//!   `<dst>.new-<ms>.part`, the item in place renamed `<dst>.old-<ms>` and kept, the part renamed
//!   in, the content checked; a root file, the notices directory or the loading templates
//!   directory the zip no longer carries renamed `<dst>.old-<ms>`), the first item waiting until
//!   those instances are idle within
//!   `install.hot.idle_wait_ms` (`instances_not_idle` otherwise; nothing changed yet, so the
//!   operation's own pauses are
//!   handed back at once, LAB-3 fix 1); then the checks after the placement (`post_check`: the
//!   plan of the same bundle again, every item now unchanged; the Tools' `actinglab.exe
//!   standard-pack check` of the game root; the new composition, the catalog the Runtime's
//!   latest evaluation ran under changed within `standard_packs.check_interval_ms` plus
//!   `install.hot.idle_wait_ms` when the game's `schedule.json` changed and, Workflow #453 S5, an
//!   instance was bound to the game when the operation began, the input `bound`; with none bound
//!   the workbench never reads the game root, so the catalog is expected unchanged; a journal
//!   without `bound`, which an earlier build began, keeps the wait), a failed check ending
//!   the command `post_check_failed` with the instances staying paused (the items are placed:
//!   `undo`, then `release-hold`); then the release of the hold (`release_hold`: only the
//!   instances the pause step paused, each while it shows the revision recorded).
//! - **The operation while the Runtime is stopped** (Workflow #453 S6; the caller applies the
//!   stopped rule under the writer lock, `crate::rescue`): the same plan and plan id, the input
//!   `cold`, and only the items that change (`pack_item`, none waiting for idle instances) and the
//!   checks after the placement (`post_check`): no `pause` and no `release_hold`. The checks are
//!   the plan again and `standard-pack check` as above, the live build's check-config of the live
//!   configuration (`check_config`: its result, and, fix 1, its notes that name the bundle's game;
//!   with no instance the configuration binds to the game, the input `bound` empty, it says that
//!   the game's composition was not previewed; fix 2, the game judged from check-config's report,
//!   which the outcome's bounds do not cut, a composition refused as a whole and a collision
//!   across games failing it too), and the composition `not_evaluated` (no Runtime to
//!   ask); a failed check ends the command `post_check_failed` with the items placed and `next`
//!   naming `undo`, no hold kept. Its `undo` and its `resume` apply the stopped rule again under
//!   the writer lock (`super::undo`, `super::update::resume`).
//! - **A failed check** (fix 1, as Workflow #454 closes the post-start checks), with a Runtime or
//!   without one, closes its `post_check` step `done` with what it read (`game`, `checks`,
//!   `failed`) in the write that ends the operation.
//!
//! Nothing is deleted: every replaced item is kept beside its place, and `undo` puts them back.

use super::deploy::{StepEnd, hold_kept};
use super::files;
use super::host::Host;
use super::journal::{Opening, Operation, PlannedStep, Previous, Step, StepKind, StepState};
use super::manifest;
use super::plan::Loaded;
use super::programs::{self, Deadline};
use super::state::{KindLocation, OperationKind, Paths};
use crate::codes::{InstallCaveat, InstallLocation, InstallRefusal};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{CauseRelation, IoOp, Outcome};
use actingcommand_contract::{
    BundleIndex, LOADING_PREFIX, LoadingTarget, MAX_NOTICE_FILE_BYTES, NOTICES_PACKAGE_REVISION,
    NoticeBound, NoticeEntry, NoticeIssue, RESERVED_TABLES_DIRECTORY, STANDARD_PACK_BUNDLE_FILE,
    STANDARD_PACK_DISPLAY_LAYOUT_FILE, STANDARD_PACK_GOALS_FILE, STANDARD_PACK_INTERRUPTS_FILE,
    STANDARD_PACK_LOADING_DIRECTORY, STANDARD_PACK_LOADING_FILE, STANDARD_PACK_NAVIGATION_FILE,
    STANDARD_PACK_NOTICES_DIRECTORY, STANDARD_PACK_PAGES_FILE, STANDARD_PACK_SCHEDULE_FILE,
    STANDARD_PACK_UI_FILE, StandardPackProblem, StandardPackRootBytes, StandardPackRootIssue,
    check_notice_listing, check_notice_pack_ids, check_notice_size, check_notice_text,
    notice_relative_path, parse_standard_pack_loading, read_standard_pack_root,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// The command of every outcome here.
pub(crate) const PLACE: InstallLocation = InstallLocation::PackPlace;
/// The applications table `package bundle` writes beside `bundle.json`; no Runtime reader opens it.
const APPLICATIONS_FILE: &str = "applications.json";
/// The bundle's package interface declaration: checked, never placed.
const INTERFACES_FILE: &str = "interfaces.json";
/// The folder of a bundle's packs, `packs/<package_id>/`.
const PACKS_DIRECTORY: &str = "packs";
/// The file every pack carries.
const PACK_CONTROL_FILE: &str = "control.json";
/// The configuration's standard-pack root (`contracts/runtime-config.md`).
const ROOT_KEY: &str = "standard_packs.root";
/// The least time between two checks of the scheduler workbench.
const CHECK_INTERVAL_KEY: &str = "standard_packs.check_interval_ms";
/// The period of the waits' reads (the hold's busy-read period, fixed by the contract).
const POLL_MS: u64 = 500;
/// The Tools' Lab program, which runs `standard-pack check`.
const LAB_PROGRAM: &str = "actinglab.exe";
/// The root files a bundle zip may carry, in the order they are placed: `bundle.json` first (the
/// others name its packs), the files the Runtime's readers open, `applications.json`,
/// `schedule.json` (its rules name packs the placed `bundle.json` lists) and (Workflow #455 G-R1,
/// `package` 12) `goals.json` last (its entries name the schedule's pools and rules and the
/// bundle's packs).
const ROOT_FILES: [&str; 10] = [
    STANDARD_PACK_BUNDLE_FILE,
    STANDARD_PACK_PAGES_FILE,
    STANDARD_PACK_INTERRUPTS_FILE,
    STANDARD_PACK_NAVIGATION_FILE,
    STANDARD_PACK_DISPLAY_LAYOUT_FILE,
    STANDARD_PACK_LOADING_FILE,
    STANDARD_PACK_UI_FILE,
    APPLICATIONS_FILE,
    STANDARD_PACK_SCHEDULE_FILE,
    STANDARD_PACK_GOALS_FILE,
];
/// The root files the Runtime's readers open, set aside when the new zip no longer carries them
/// (the Runtime would read them against the new registry), in this order; (Workflow #455 G-R1)
/// `goals.json` first, whose entries the Tools' check holds to the placed root.
const RETIRED_WHEN_ABSENT: [&str; 8] = [
    STANDARD_PACK_GOALS_FILE,
    STANDARD_PACK_SCHEDULE_FILE,
    STANDARD_PACK_UI_FILE,
    STANDARD_PACK_LOADING_FILE,
    STANDARD_PACK_DISPLAY_LAYOUT_FILE,
    STANDARD_PACK_NAVIGATION_FILE,
    STANDARD_PACK_INTERRUPTS_FILE,
    STANDARD_PACK_PAGES_FILE,
];

/// What an item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ItemKind {
    /// `tables/<table_id>.json`.
    Table,
    /// `<package_id>\`, a folder.
    Pack,
    /// A root file of the game root.
    Root,
    /// Workflow #444 NOTICES-2: `notices\`, the bundle's notices directory, a folder digested as
    /// a pack's is.
    Notices,
    /// Workflow #451 LOAD-P: `loading\`, the templates the bundle's `loading.json` declares, a
    /// folder digested as a pack's is.
    Loading,
}

/// What the placement does to an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ItemState {
    New,
    Replace,
    Unchanged,
    /// A reader's root file, (Workflow #444 NOTICES-2) the notices directory or (Workflow #451
    /// LOAD-P) the loading templates directory, the zip no longer carries: set aside.
    Retire,
}

/// One item of a placement, as the plan lists it and as its `pack_item` step's intent records it
/// (with the zip, the part and the kept name).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Item {
    /// The table id, the pack id, the root file's name or the directory's (`notices`, `loading`).
    pub(crate) item: String,
    pub(crate) kind: ItemKind,
    pub(crate) state: ItemState,
    /// Its place.
    pub(crate) dst: String,
    /// The new content: a file's SHA-256, a pack folder's listing digest ([`listing_digest`]);
    /// none for a root file set aside.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) digest: Option<String>,
    /// Each file of the new content by its path below `dst` (`""` for a file item) and its zip
    /// member.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) members: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) zip: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) part: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) old: Option<String>,
    /// The operation's first item, which waits until the held instances are idle.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) first: bool,
}

/// A placement's plan.
#[derive(Debug, Clone)]
pub(crate) struct Placement {
    pub(crate) id: String,
    pub(crate) document: Value,
    pub(crate) game: String,
    pub(crate) game_root: PathBuf,
    pub(crate) zip: PathBuf,
    pub(crate) zip_sha256: String,
    pub(crate) items: Vec<Item>,
}

impl Placement {
    /// The items that change, in the order they are placed.
    pub(crate) fn changes(&self) -> impl Iterator<Item = &Item> {
        self.items
            .iter()
            .filter(|item| item.state != ItemState::Unchanged)
    }

    /// Whether the game's `schedule.json` changes (placed, replaced or set aside): the
    /// placement then waits for a new composition.
    pub(crate) fn schedule_changes(&self) -> bool {
        self.changes()
            .any(|item| item.kind == ItemKind::Root && item.item == STANDARD_PACK_SCHEDULE_FILE)
    }

    /// The plan as an answer lists it.
    pub(crate) fn value(&self) -> Value {
        json!({
            "plan_id": self.id,
            "game": self.game,
            "game_root": files::plain(&self.game_root),
            "zip": files::plain(&self.zip),
            "zip_sha256": self.zip_sha256,
            "items": self.items.iter().map(|item| json!({
                "item": item.item,
                "kind": item.kind,
                "state": item.state,
                "dst": item.dst,
            })).collect::<Vec<_>>(),
            "schedule_changes": self.schedule_changes(),
        })
    }
}

/// `pack_invalid` of `place` with `path`.
fn invalid(path: &Path) -> Outcome {
    super::refusal(PLACE, InstallRefusal::PackInvalid).path(path)
}

/// The bundle zip, opened.
struct Archive {
    path: PathBuf,
    zip: zip::ZipArchive<File>,
}

impl Archive {
    fn open(path: &Path) -> Result<Self, Outcome> {
        let file = File::open(path).map_err(|error| files::os_failure(path, &error, IoOp::Open))?;
        let zip = zip::ZipArchive::new(file)
            .map_err(|error| invalid(path).caused_by(files::zip_failure(&error).path(path)))?;
        Ok(Self {
            path: path.to_path_buf(),
            zip,
        })
    }

    /// Every file member by its name, with its size; a member whose name leaves the zip's root
    /// (absolute, `..`, a drive) refuses `pack_invalid`.
    fn members(&mut self) -> Result<BTreeMap<String, u64>, Outcome> {
        let mut members = BTreeMap::new();
        for index in 0..self.zip.len() {
            let entry = self.zip.by_index(index).map_err(|error| {
                invalid(&self.path).caused_by(files::zip_failure(&error).path(&self.path))
            })?;
            if entry.is_dir() {
                continue;
            }
            let name = entry.name().to_owned();
            if entry.enclosed_name().is_none()
                || !files::is_plain_relative(&name)
                || members.insert(name.clone(), entry.size()).is_some()
            {
                return Err(invalid(&self.path.join(&name)));
            }
        }
        Ok(members)
    }

    /// The bytes of the member `name`.
    fn read(&mut self, name: &str) -> Result<Vec<u8>, Outcome> {
        let mut entry = self.zip.by_name(name).map_err(|error| {
            invalid(&self.path).caused_by(files::zip_failure(&error).path(&self.path))
        })?;
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|error| files::os_failure(&self.path, &error, IoOp::Read))?;
        Ok(bytes)
    }

    /// Workflow #444 NOTICES-2: the bytes of the member `name`, at most `limit` of them, whatever
    /// size the zip declares for it.
    fn read_at_most(&mut self, name: &str, limit: u64) -> Result<Vec<u8>, Outcome> {
        let entry = self.zip.by_name(name).map_err(|error| {
            invalid(&self.path).caused_by(files::zip_failure(&error).path(&self.path))
        })?;
        let mut bytes = Vec::new();
        entry
            .take(limit)
            .read_to_end(&mut bytes)
            .map_err(|error| files::os_failure(&self.path, &error, IoOp::Read))?;
        Ok(bytes)
    }

    /// The bytes of the member `name` of an item of `kind`: a file of the notices directory
    /// through its bound plus one byte (Workflow #444 NOTICES-2), any other member whole (a
    /// template of the loading templates directory too, as a pack's file: the contract crate
    /// states no bound for one, Workflow #451 LOAD-P).
    fn read_of(&mut self, kind: ItemKind, name: &str) -> Result<Vec<u8>, Outcome> {
        if kind == ItemKind::Notices {
            self.read_at_most(name, MAX_NOTICE_FILE_BYTES + 1)
        } else {
            self.read(name)
        }
    }
}

/// The digest of a folder's listing: the SHA-256 of each `<path>\n<sha256>\n`, in path order.
fn listing_digest(listing: &BTreeMap<String, String>) -> String {
    let mut text = String::new();
    for (path, sha256) in listing {
        text.push_str(path);
        text.push('\n');
        text.push_str(sha256);
        text.push('\n');
    }
    files::sha256_bytes(text.as_bytes())
}

/// What `dst` holds now as an item of `kind`: a file's SHA-256, a folder's listing digest;
/// `None` when nothing is there, an empty digest when something of the other kind is.
fn disk_digest(kind: ItemKind, dst: &Path) -> Result<Option<String>, Outcome> {
    if !dst.exists() {
        return Ok(None);
    }
    let read = |path: &Path| {
        files::sha256_file(path)
            .map(|(sha256, _)| sha256)
            .map_err(|error| files::os_failure(path, &error, IoOp::Read))
    };
    match kind {
        ItemKind::Pack | ItemKind::Notices | ItemKind::Loading if dst.is_dir() => {
            let mut listing = BTreeMap::new();
            for relative in files::list_files(dst)? {
                let path = files::join_relative(dst, &relative);
                listing.insert(relative, read(&path)?);
            }
            Ok(Some(listing_digest(&listing)))
        }
        ItemKind::Table | ItemKind::Root if dst.is_file() => read(dst).map(Some),
        _ => Ok(Some(String::new())),
    }
}

/// The new content's digest of an item from the zip.
fn zip_digest(
    archive: &mut Archive,
    kind: ItemKind,
    members: &BTreeMap<String, String>,
) -> Result<String, Outcome> {
    match kind {
        ItemKind::Pack | ItemKind::Notices | ItemKind::Loading => {
            let mut listing = BTreeMap::new();
            for (relative, member) in members {
                listing.insert(
                    relative.clone(),
                    files::sha256_bytes(&archive.read_of(kind, member)?),
                );
            }
            Ok(listing_digest(&listing))
        }
        ItemKind::Table | ItemKind::Root => {
            let member = members.get("").cloned().unwrap_or_default();
            Ok(files::sha256_bytes(&archive.read(&member)?))
        }
    }
}

/// The configuration's standard-pack root: `standard_packs.root`, a relative one against the
/// configuration file's folder (the root); a configuration without it refuses `pack_invalid`
/// with the key.
fn packs_root(paths: &Paths, config: &Value) -> Result<PathBuf, Outcome> {
    let root = actingcommand_runtime_config::member(config, ROOT_KEY)
        .and_then(Value::as_str)
        .filter(|root| !root.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| {
            super::refusal(PLACE, InstallRefusal::PackInvalid)
                .path(paths.config())
                .config_key(ROOT_KEY)
        })?;
    Ok(if root.is_absolute() {
        root
    } else {
        paths.root.join(root)
    })
}

/// Checks `zip` against `SHA256SUMS` (`<sha256 hex>  <file name>` lines): its name listed with
/// its own digest, else `pack_invalid`.
fn check_sums(zip: &Path, zip_sha256: &str, sums: &Path) -> Result<(), Outcome> {
    let text = String::from_utf8(files::read_all(sums)?).map_err(|_| invalid(sums))?;
    let name = zip
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let listed = text.lines().find_map(|line| {
        let (digest, file) = line.trim().split_once(char::is_whitespace)?;
        (file.trim().trim_start_matches('*') == name).then(|| digest.to_ascii_lowercase())
    });
    match listed {
        Some(digest) if digest == zip_sha256 => Ok(()),
        _ => Err(invalid(zip).path(sums)),
    }
}

/// One item of the plan: its new content from the zip, and what its place holds now.
fn planned_item(
    archive: &mut Archive,
    item: String,
    kind: ItemKind,
    dst: &Path,
    members: BTreeMap<String, String>,
) -> Result<Item, Outcome> {
    let digest = zip_digest(archive, kind, &members)?;
    let state = match disk_digest(kind, dst)? {
        None => ItemState::New,
        Some(held) if held == digest => ItemState::Unchanged,
        Some(_) => ItemState::Replace,
    };
    Ok(Item {
        item,
        kind,
        state,
        dst: files::plain(dst),
        digest: Some(digest),
        members,
        zip: None,
        part: None,
        old: None,
        first: false,
    })
}

/// An item the zip no longer carries, set aside: a reader's root file, (Workflow #444
/// NOTICES-2) the notices directory or (Workflow #451 LOAD-P) the loading templates directory.
fn retired_item(item: &str, kind: ItemKind, dst: &Path) -> Item {
    Item {
        item: item.to_owned(),
        kind,
        state: ItemState::Retire,
        dst: files::plain(dst),
        digest: None,
        members: BTreeMap::new(),
        zip: None,
        part: None,
        old: None,
        first: false,
    }
}

/// Workflow #444 NOTICES-2: `pack_invalid` of the bundle `zip` of `game` for a broken notices
/// rule, caused by `standard_pack_root_invalid` as a root file's problem is: `path` the member the
/// issue names below the zip (`<zip>\notices\…`, `<zip>\notices` for the count or the total,
/// `<zip>\bundle.json` with `package_id` for a pack), `json_pointer` empty, and the bound with the
/// value found where the rule has one.
fn notice_refusal(zip: &Path, game: &str, issue: &NoticeIssue) -> Outcome {
    let mut cause = Outcome::new(SharedCode::StandardPackRootInvalid)
        .game(game)
        .path(files::join_relative(zip, &issue.member()))
        .standard_pack_problem(issue.problem)
        .json_pointer(String::new());
    if let Some(package_id) = issue.package_id() {
        cause = cause.package_id(package_id);
    }
    cause = match issue.bound {
        Some(NoticeBound::Count { limit, found }) => cause.limit_count(limit).count(found),
        Some(NoticeBound::Bytes { limit, found }) => cause.limit_bytes(limit).actual_bytes(found),
        None => cause,
    };
    invalid(zip).caused_by(cause)
}

/// Workflow #444 NOTICES-2: the notices directory of the bundle `zip` of `game`, `notices` (each
/// file's path below the directory and its member), held to the contract crate's rules before
/// anything is held or written: a bundle that carries it declares a `package` maximum
/// (`package[1]`) of [`NOTICES_PACKAGE_REVISION`] at least (`notice_undeclared`, at
/// `<zip>\interfaces.json`); the listing with the sizes the zip declares (`sizes`, by member)
/// keeps the set rule; no pack of `package_ids` is named like the directory; and each file, read
/// through its bound plus one byte, keeps the size and text rules with what was read.
fn check_notices<'a>(
    archive: &mut Archive,
    zip: &Path,
    game: &str,
    package: [u64; 2],
    sizes: &BTreeMap<String, u64>,
    notices: &BTreeMap<String, String>,
    package_ids: impl IntoIterator<Item = &'a str>,
) -> Result<(), Outcome> {
    if package[1] < NOTICES_PACKAGE_REVISION {
        return Err(invalid(zip).caused_by(
            Outcome::new(SharedCode::StandardPackRootInvalid)
                .game(game)
                .path(zip.join(INTERFACES_FILE))
                .standard_pack_problem(StandardPackProblem::NoticeUndeclared)
                .json_pointer(String::new()),
        ));
    }
    let listing = notices
        .iter()
        .map(|(path, member)| NoticeEntry {
            path,
            bytes: sizes.get(member).copied().unwrap_or(0),
        })
        .collect::<Vec<_>>();
    check_notice_listing(&listing).map_err(|issue| notice_refusal(zip, game, &issue))?;
    check_notice_pack_ids(true, package_ids).map_err(|issue| notice_refusal(zip, game, &issue))?;
    for (path, member) in notices {
        let bytes = archive.read_of(ItemKind::Notices, member)?;
        check_notice_size(path, bytes.len() as u64)
            .and_then(|()| check_notice_text(path, &bytes))
            .map_err(|issue| notice_refusal(zip, game, &issue))?;
    }
    Ok(())
}

/// `pack_invalid` of the bundle `zip` of `game` for a root file's problem `issue`, caused by
/// `standard_pack_root_invalid` with `path` the file below the zip, its problem and its pointer.
fn root_refusal(zip: &Path, game: &str, issue: &StandardPackRootIssue) -> Outcome {
    invalid(zip).caused_by(
        Outcome::new(SharedCode::StandardPackRootInvalid)
            .game(game)
            .path(zip.join(issue.file))
            .standard_pack_problem(issue.problem)
            .json_pointer(issue.pointer.clone()),
    )
}

/// Workflow #451 LOAD-P: the loading templates directory of the bundle `zip` of `game`, `loading`
/// (each member's path below the directory and the member), held to the zip's own `loading.json`
/// (`declaring`, its bytes when the zip carries it) before anything is held or written. Without
/// the file, every member of the directory is one no layout knows (`pack_invalid` naming the
/// first). With it, the file is read by the contract crate's reader, a problem refused as a root
/// file's is; then a member that is not a template the file declares (a deeper path never is) is
/// one no layout knows; a template the file declares that the zip lacks is
/// `loading_template_invalid`, as `standard-pack check` answers it (at the pointer of the first
/// target that reads it), `path` the missing member below the zip; and a bundle that carries the
/// directory lists no pack of `package_ids` named `loading` in any ASCII case
/// (`loading_pack_name`, `path` `<zip>\bundle.json`, with `package_id`). Only the declaration and
/// the names are read: what a template shows is `standard-pack check`'s, which `post_check` runs
/// on the placed game root.
fn check_loading<'a>(
    zip: &Path,
    game: &str,
    declaring: Option<&[u8]>,
    loading: &BTreeMap<String, String>,
    package_ids: impl IntoIterator<Item = &'a str>,
) -> Result<(), Outcome> {
    let Some(bytes) = declaring else {
        return match loading.values().next() {
            Some(member) => Err(invalid(&zip.join(member))),
            None => Ok(()),
        };
    };
    let declaration =
        parse_standard_pack_loading(bytes, Some(game)).map_err(|issues| match issues.first() {
            Some(issue) => root_refusal(zip, game, issue),
            None => invalid(&zip.join(STANDARD_PACK_LOADING_FILE)),
        })?;
    let declared = declaration
        .targets
        .iter()
        .filter_map(LoadingTarget::template)
        .collect::<BTreeSet<_>>();
    if let Some(member) = loading
        .values()
        .find(|member| !declared.contains(member.as_str()))
    {
        return Err(invalid(&zip.join(member)));
    }
    let carried = loading
        .values()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let missing = declaration.targets.iter().find_map(|target| {
        target
            .template()
            .filter(|template| !carried.contains(template))
            .map(|template| (target, template))
    });
    if let Some((target, template)) = missing {
        return Err(invalid(zip).caused_by(
            Outcome::new(SharedCode::StandardPackRootInvalid)
                .game(game)
                .path(files::join_relative(zip, template))
                .standard_pack_problem(StandardPackProblem::LoadingTemplateInvalid)
                .json_pointer(
                    declaration
                        .target_pointer(target.id(), "template")
                        .unwrap_or_default(),
                ),
        ));
    }
    if !loading.is_empty()
        && let Some(package_id) = package_ids
            .into_iter()
            .find(|id| id.eq_ignore_ascii_case(STANDARD_PACK_LOADING_DIRECTORY))
    {
        return Err(invalid(zip).caused_by(
            Outcome::new(SharedCode::StandardPackRootInvalid)
                .game(game)
                .path(zip.join(STANDARD_PACK_BUNDLE_FILE))
                .standard_pack_problem(StandardPackProblem::LoadingPackName)
                .json_pointer(String::new())
                .package_id(package_id),
        ));
    }
    Ok(())
}

/// The plan of placing the bundle `zip` (see the module's description). `sums` is the
/// `SHA256SUMS` the zip is checked against, when given.
pub(crate) fn plan(
    host: &dyn Host,
    paths: &Paths,
    loaded: &Loaded,
    zip: &Path,
    sums: Option<&Path>,
) -> Result<Placement, Outcome> {
    let zip = std::fs::canonicalize(zip).map_err(|error| {
        super::unusable("<bundle>").caused_by(files::os_failure(zip, &error, IoOp::Canonicalize))
    })?;
    let (zip_sha256, _) =
        files::sha256_file(&zip).map_err(|error| files::os_failure(&zip, &error, IoOp::Read))?;
    if let Some(sums) = sums {
        check_sums(&zip, &zip_sha256, sums)?;
    }
    let mut archive = Archive::open(&zip)?;
    let members = archive.members()?;
    // bundle.json, as the contract crate validates it.
    let bundle_bytes = archive.read(STANDARD_PACK_BUNDLE_FILE)?;
    let bundle: BundleIndex = serde_json::from_slice(files::without_bom(&bundle_bytes))
        .map_err(|error| invalid(&zip).caused_by(files::json_failure(&error)))?;
    bundle
        .validate()
        .map_err(|error| invalid(&zip).caused_by(Outcome::new(error.code())))?;
    let game = bundle.game.clone();
    // The package interface the bundle declares: its maximum within this installer's range.
    // Workflow #444 NOTICES-2: checked before the layout, so that a bundle of a later revision is
    // refused through `package` and not through a member this installer does not know. Workflow
    // #451 PLACE-PKG: read by the bundle's own rule (the schema, every value under `speaks` a pair,
    // `speaks.package` named), not by the rule of a Runtime's declaration, which names all six v1
    // interfaces; a missing, unreadable or malformed file is the refusal below.
    let declared = archive
        .read(INTERFACES_FILE)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(files::without_bom(&bytes)).ok())
        .and_then(|value| manifest::parse_bundle_package(&value));
    let own = host
        .installer_speaks()
        .and_then(|speaks| speaks.get("package").copied());
    let package = match (declared, own) {
        (Some(declared), Some(own)) if own[0] <= declared[1] && declared[1] <= own[1] => declared,
        _ => {
            return Err(super::refusal(PLACE, InstallRefusal::InterfaceEdge)
                .path(zip.join(INTERFACES_FILE))
                .game(game.clone()));
        }
    };
    let pack_ids = bundle
        .packs
        .iter()
        .map(|pack| pack.package_id.clone())
        .collect::<BTreeSet<_>>();
    let tables = bundle.tables.clone().unwrap_or_default();
    // The layout: the root files, `packs/<listed id>/…`, `tables/<listed id>.json`, (Workflow
    // #444 NOTICES-2) the notices directory's files `notices/<path>` and (Workflow #451 LOAD-P)
    // the loading templates directory's `loading/<path>`, each by its path below its directory.
    let mut roots = BTreeSet::new();
    let mut pack_members: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut notice_members: BTreeMap<String, String> = BTreeMap::new();
    let mut loading_members: BTreeMap<String, String> = BTreeMap::new();
    for name in members.keys() {
        if let Some(path) = notice_relative_path(name) {
            notice_members.insert(path.to_owned(), name.clone());
            continue;
        }
        if let Some(path) = name.strip_prefix(LOADING_PREFIX) {
            loading_members.insert(path.to_owned(), name.clone());
            continue;
        }
        let parts = name.split('/').collect::<Vec<_>>();
        match parts.as_slice() {
            [file] if ROOT_FILES.contains(file) => {
                roots.insert((*file).to_owned());
            }
            [file] if *file == INTERFACES_FILE => {}
            [packs, id, rest @ ..]
                if *packs == PACKS_DIRECTORY && pack_ids.contains(*id) && !rest.is_empty() =>
            {
                pack_members
                    .entry((*id).to_owned())
                    .or_default()
                    .insert(rest.join("/"), name.clone());
            }
            [directory, file]
                if *directory == RESERVED_TABLES_DIRECTORY
                    && file
                        .strip_suffix(".json")
                        .is_some_and(|table| tables.iter().any(|known| known == table)) => {}
            _ => return Err(invalid(&zip.join(name))),
        }
    }
    // Workflow #444 NOTICES-2: the notices directory, by the contract crate's rules.
    if !notice_members.is_empty() {
        check_notices(
            &mut archive,
            &zip,
            &game,
            package,
            &members,
            &notice_members,
            bundle.packs.iter().map(|pack| pack.package_id.as_str()),
        )?;
    }
    // Workflow #451 LOAD-P: the loading templates directory, held to the zip's own
    // `loading.json`; neither is read when the zip carries neither.
    let declaring = if roots.contains(STANDARD_PACK_LOADING_FILE) {
        Some(archive.read(STANDARD_PACK_LOADING_FILE)?)
    } else {
        None
    };
    check_loading(
        &zip,
        &game,
        declaring.as_deref(),
        &loading_members,
        bundle.packs.iter().map(|pack| pack.package_id.as_str()),
    )?;
    // The root files as every reader checks them.
    let mut root_bytes = BTreeMap::new();
    for file in &roots {
        root_bytes.insert(file.clone(), archive.read(file)?);
    }
    let bytes_of = |file: &str| root_bytes.get(file).map(Vec::as_slice);
    if let Err(issue) = read_standard_pack_root(
        StandardPackRootBytes {
            bundle: Some(bundle_bytes.as_slice()),
            pages: bytes_of(STANDARD_PACK_PAGES_FILE),
            interrupts: bytes_of(STANDARD_PACK_INTERRUPTS_FILE),
            navigation: bytes_of(STANDARD_PACK_NAVIGATION_FILE),
            tables: !tables.is_empty(),
            display_layout: bytes_of(STANDARD_PACK_DISPLAY_LAYOUT_FILE),
        },
        &game,
    ) {
        return Err(root_refusal(&zip, &game, &issue));
    }
    // Each pack: its files as `bundle.json` counts them, its `control.json` among them.
    for pack in &bundle.packs {
        let listed = pack_members.get(&pack.package_id);
        let count = listed.map_or(0, BTreeMap::len) as u64;
        let size = listed.map_or(0, |listed| {
            listed
                .values()
                .map(|member| members.get(member).copied().unwrap_or(0))
                .sum::<u64>()
        });
        if count != pack.file_count
            || size != pack.byte_count
            || !listed.is_some_and(|listed| listed.contains_key(PACK_CONTROL_FILE))
        {
            return Err(super::refusal(PLACE, InstallRefusal::PackInvalid)
                .path(zip.join(PACKS_DIRECTORY).join(&pack.package_id))
                .package_id(pack.package_id.clone())
                .game(game.clone()));
        }
    }
    let game_root = packs_root(paths, &loaded.config)?.join(&game);
    // A part another run left refuses: nothing here removes it.
    for directory in [game_root.clone(), game_root.join(RESERVED_TABLES_DIRECTORY)] {
        if let Ok(entries) = std::fs::read_dir(&directory) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().ends_with(".part") {
                    return Err(
                        super::refusal(PLACE, InstallRefusal::ForeignPart).path(entry.path())
                    );
                }
            }
        }
    }
    let mut items = Vec::new();
    // Workflow #444 NOTICES-2: the notices directory first, so that the texts are in place before
    // the material they accompany; set aside, as a reader's root file is, when the game root holds
    // one and the zip carries none (unless a pack of the bundle takes that folder's name).
    let notices = game_root.join(STANDARD_PACK_NOTICES_DIRECTORY);
    if !notice_members.is_empty() {
        items.push(planned_item(
            &mut archive,
            STANDARD_PACK_NOTICES_DIRECTORY.to_owned(),
            ItemKind::Notices,
            &notices,
            notice_members,
        )?);
    } else if notices.exists()
        && !pack_ids
            .iter()
            .any(|id| id.eq_ignore_ascii_case(STANDARD_PACK_NOTICES_DIRECTORY))
    {
        items.push(retired_item(
            STANDARD_PACK_NOTICES_DIRECTORY,
            ItemKind::Notices,
            &notices,
        ));
    }
    for table in &tables {
        let file = format!("{table}.json");
        let member = format!("{RESERVED_TABLES_DIRECTORY}/{file}");
        if !members.contains_key(&member) {
            return Err(invalid(&zip.join(&member)));
        }
        items.push(planned_item(
            &mut archive,
            table.clone(),
            ItemKind::Table,
            &game_root.join(RESERVED_TABLES_DIRECTORY).join(file),
            BTreeMap::from([(String::new(), member)]),
        )?);
    }
    for pack in &bundle.packs {
        items.push(planned_item(
            &mut archive,
            pack.package_id.clone(),
            ItemKind::Pack,
            &game_root.join(&pack.package_id),
            pack_members.remove(&pack.package_id).unwrap_or_default(),
        )?);
    }
    // Workflow #451 LOAD-P: the loading templates directory after the packs and before the root
    // files, so that the templates are in place before the `loading.json` that declares them; set
    // aside, as `loading.json` is, when the game root holds one and the zip carries none (unless a
    // pack of the bundle takes that folder's name).
    let loading = game_root.join(STANDARD_PACK_LOADING_DIRECTORY);
    if !loading_members.is_empty() {
        items.push(planned_item(
            &mut archive,
            STANDARD_PACK_LOADING_DIRECTORY.to_owned(),
            ItemKind::Loading,
            &loading,
            loading_members,
        )?);
    } else if loading.exists()
        && !pack_ids
            .iter()
            .any(|id| id.eq_ignore_ascii_case(STANDARD_PACK_LOADING_DIRECTORY))
    {
        items.push(retired_item(
            STANDARD_PACK_LOADING_DIRECTORY,
            ItemKind::Loading,
            &loading,
        ));
    }
    for file in RETIRED_WHEN_ABSENT {
        let dst = game_root.join(file);
        if !roots.contains(file) && dst.exists() {
            items.push(retired_item(file, ItemKind::Root, &dst));
        }
    }
    for file in ROOT_FILES {
        if roots.contains(file) {
            items.push(planned_item(
                &mut archive,
                file.to_owned(),
                ItemKind::Root,
                &game_root.join(file),
                BTreeMap::from([(String::new(), file.to_owned())]),
            )?);
        }
    }
    let document = json!({
        "game": game,
        "zip_sha256": zip_sha256,
        "package": package,
        "items": items.iter().map(|item| json!({
            "item": item.item,
            "kind": item.kind,
            "state": item.state,
            "digest": item.digest,
        })).collect::<Vec<_>>(),
    });
    let canonical = actingcommand_runtime_config::canonical_json(&document);
    let id = format!("plan-{}", &files::sha256_bytes(canonical.as_bytes())[..16]);
    Ok(Placement {
        id,
        document,
        game,
        game_root,
        zip,
        zip_sha256,
        items,
    })
}

/// The post-placement check's intent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckIntent {
    place: bool,
    game: String,
    game_root: String,
    schedule_changes: bool,
    catalog_before: Option<String>,
}

/// Whether a `post_check` step's intent is the placement's.
pub(crate) fn is_place_check(intent: &Value) -> bool {
    intent.get("place").and_then(Value::as_bool) == Some(true)
}

/// The steps of a placement: the hold of the instances `held` (`pause_intent`), each item that
/// changes, the checks after the placement and the release of the hold.
pub(crate) fn steps(
    placement: &Placement,
    pause_intent: Value,
    reason: &actingcommand_contract::SchedulingPauseReason,
    catalog_before: Option<String>,
    ms: u64,
) -> Vec<PlannedStep> {
    let mut steps: Vec<PlannedStep> = vec![(StepKind::Pause, "runtime".to_owned(), pause_intent)];
    steps.extend(item_steps(placement, true, ms));
    steps.push(check_planned(placement, catalog_before));
    steps.push((
        StepKind::ReleaseHold,
        "runtime".to_owned(),
        super::hold::ReleaseIntent::value(reason, false),
    ));
    steps
}

/// Workflow #453 S6: the steps of a placement while no Runtime runs: each item that changes and
/// the checks after the placement; no hold, so no `pause` and no `release_hold`, and no item
/// waits for idle instances.
fn cold_steps(placement: &Placement, ms: u64) -> Vec<PlannedStep> {
    let mut steps = item_steps(placement, false, ms);
    steps.push(check_planned(placement, None));
    steps
}

/// One `pack_item` step per item that changes, in the plan's order; with `wait_first` the first
/// waits until the held instances are idle.
fn item_steps(placement: &Placement, wait_first: bool, ms: u64) -> Vec<PlannedStep> {
    placement
        .changes()
        .enumerate()
        .map(|(index, item)| {
            let dst = PathBuf::from(&item.dst);
            let placed = item.state != ItemState::Retire;
            let mut intent = item.clone();
            intent.zip = placed.then(|| files::plain(&placement.zip));
            intent.part =
                placed.then(|| files::plain(&files::with_suffix(&dst, &format!(".new-{ms}.part"))));
            intent.old = (item.state != ItemState::New)
                .then(|| files::plain(&files::with_suffix(&dst, &format!(".old-{ms}"))));
            intent.first = wait_first && index == 0;
            (
                StepKind::PackItem,
                item.dst.clone(),
                serde_json::to_value(&intent).unwrap_or(Value::Null),
            )
        })
        .collect()
}

/// The `post_check` step of a placement; its intent records `catalog_before`, the catalog read
/// before the hold (none while no Runtime runs).
fn check_planned(placement: &Placement, catalog_before: Option<String>) -> PlannedStep {
    (
        StepKind::PostCheck,
        "runtime".to_owned(),
        serde_json::to_value(CheckIntent {
            place: true,
            game: placement.game.clone(),
            game_root: files::plain(&placement.game_root),
            schedule_changes: placement.schedule_changes(),
            catalog_before,
        })
        .unwrap_or(Value::Null),
    )
}

/// What a placement records before its first step.
pub(crate) struct Begun<'a> {
    pub(crate) placement: &'a Placement,
    pub(crate) hold: &'a super::deploy::HoldStart,
    pub(crate) catalog_before: Option<String>,
    pub(crate) initiator: Option<actingcommand_lab_ops::initiator::Initiator>,
    /// Workflow #453 S5: the instances the running Runtime binds to the bundle's game, as the
    /// status read before the hold showed them (paused or not), journalled as the input
    /// [`BOUND_INPUT`].
    pub(crate) bound: &'a [String],
}

/// Workflow #453 S5: the input of a `pack_place` journal that lists the instances bound to the
/// bundle's game when the operation began. A journal an earlier build began has none.
const BOUND_INPUT: &str = "bound";

/// Opens the placement's operation (`pack_place`): S0 and the hold's reads journalled, the guards
/// recorded, the deferred record in the pause step's evidence.
pub(crate) fn open<'h>(
    host: &'h dyn Host,
    paths: &Paths,
    loaded: &Loaded,
    begun: &Begun<'_>,
) -> Result<Operation<'h>, Outcome> {
    let placement = begun.placement;
    let hold = &begun.hold.hold;
    let pause_intent = super::hold::pause_intent(
        hold,
        &begun.hold.held,
        loaded.settings.stop_drain_timeout_ms,
    );
    let mut inputs = json!({
        "commit": loaded.state.live.as_ref().map(|live| live.commit.clone()),
        "game": placement.game,
        "zip": files::plain(&placement.zip),
        "zip_sha256": placement.zip_sha256,
        "hold": hold.reason,
        "known_pauses": hold.known,
        "instances": begun.hold.held,
        "config_sha256": loaded.config_sha256,
    });
    inputs[BOUND_INPUT] = json!(begun.bound);
    let opening = Opening {
        kind: OperationKind::PackPlace,
        initiator: begun.initiator,
        one_way: false,
        inputs,
        plan_id: Some(placement.id.clone()),
        plan: Some(placement.document.clone()),
        acknowledged: Vec::new(),
        s0: Some(super::hold::s0_of(&begun.hold.seen, &begun.hold.config)),
        previous: Previous {
            live: loaded.state.live.as_ref().map(|live| live.commit.clone()),
            state: None,
        },
    };
    let catalog_before = begun.catalog_before.clone();
    let mut operation = Operation::open(host, paths, opening, |ms| {
        steps(
            placement,
            pause_intent.clone(),
            &hold.reason,
            catalog_before.clone(),
            ms,
        )
    })?;
    operation
        .record_guards(super::guards::values(&begun.hold.rows), &[])
        .map_err(|cause| operation.interrupted(cause))?;
    operation
        .progress(1, json!({"deferred": begun.hold.deferred}))
        .map_err(|cause| operation.interrupted(cause))?;
    Ok(operation)
}

/// Workflow #453 S6: the input of a `pack_place` journal begun while no Runtime ran (`true`).
const COLD_INPUT: &str = "cold";

/// Workflow #453 S6: opens the placement's operation (`pack_place`) while no Runtime runs, the
/// caller having applied the stopped rule under the writer lock: the same plan and plan id, the
/// inputs of a placement without its hold (no `hold`, `known_pauses` or `instances`),
/// [`COLD_INPUT`] and (fix 1) [`BOUND_INPUT`], the instances the configuration binds to the
/// bundle's game ([`configured_bound`]), no S0, no guards, and the steps [`cold_steps`].
pub(crate) fn open_cold<'h>(
    host: &'h dyn Host,
    paths: &Paths,
    loaded: &Loaded,
    placement: &Placement,
    initiator: Option<actingcommand_lab_ops::initiator::Initiator>,
) -> Result<Operation<'h>, Outcome> {
    let mut inputs = json!({
        "commit": loaded.state.live.as_ref().map(|live| live.commit.clone()),
        "game": placement.game,
        "zip": files::plain(&placement.zip),
        "zip_sha256": placement.zip_sha256,
        "config_sha256": loaded.config_sha256,
    });
    inputs[COLD_INPUT] = Value::Bool(true);
    inputs[BOUND_INPUT] = json!(configured_bound(&loaded.config, &placement.game));
    let opening = Opening {
        kind: OperationKind::PackPlace,
        initiator,
        one_way: false,
        inputs,
        plan_id: Some(placement.id.clone()),
        plan: Some(placement.document.clone()),
        acknowledged: Vec::new(),
        s0: None,
        previous: Previous {
            live: loaded.state.live.as_ref().map(|live| live.commit.clone()),
            state: None,
        },
    };
    Operation::open(host, paths, opening, |ms| cold_steps(placement, ms))
}

/// Workflow #453 S6 fix 1: the instances the configuration `config` binds to `game`, in its
/// order: the `instance_id` of each row of `policy.facts.instances` whose `game_id` is the game.
/// Those rows are the games in use that check-config previews the composition for and the
/// Runtime's scheduler workbench reads; none without a `policy`. Fix 2: the cold done answer's
/// `next` reads it too (`crate::rescue`).
pub(crate) fn configured_bound(config: &Value, game: &str) -> Vec<String> {
    config
        .pointer("/policy/facts/instances")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter(|row| row.get("game_id").and_then(Value::as_str) == Some(game))
                .filter_map(|row| row.get("instance_id").and_then(Value::as_str))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Workflow #453 S6: whether the operation was begun while no Runtime ran (its input
/// [`COLD_INPUT`]). Fix 1: its `undo` and its `resume` read it too, to apply the stopped rule
/// again under the writer lock.
pub(crate) fn is_cold(operation: &Operation<'_>) -> bool {
    operation
        .journal()
        .inputs
        .get(COLD_INPUT)
        .and_then(Value::as_bool)
        == Some(true)
}

/// The instances the operation's hold took (its pause step's intent).
fn held_of(
    operation: &Operation<'_>,
) -> (
    Option<actingcommand_contract::SchedulingPauseReason>,
    Vec<String>,
) {
    operation
        .journal()
        .steps
        .iter()
        .find(|step| step.kind == StepKind::Pause)
        .and_then(|step| super::hold::PauseIntent::of(&step.intent))
        .map_or((None, Vec::new()), |intent| {
            (Some(intent.reason), intent.instances)
        })
}

/// Waits until the held instances hold no lease and nothing is queued, within
/// `install.hot.idle_wait_ms`; `Some` with the refusal `instances_not_idle` when they are not.
/// Workflow #434 LAB-3 fix 1: nothing changed on disk yet, so the operation hands its own pauses
/// back at once and says so (`super::deploy::hand_back_own`).
fn wait_idle(
    operation: &Operation<'_>,
    loaded: &Loaded,
    notes: &mut Vec<Outcome>,
) -> Result<Option<StepEnd>, Outcome> {
    let host = operation.host();
    let (_, held) = held_of(operation);
    let since = host.now_unix_ms();
    loop {
        let seen = host
            .runtime()
            .status(&loaded.state_root)?
            .ok_or_else(|| super::refusal(PLACE, InstallRefusal::RuntimeNotRunning))?;
        let busy = seen
            .instances
            .iter()
            .filter(|instance| held.contains(&instance.alias))
            .filter(|instance| instance.lease_active || instance.queued_requests > 0)
            .map(|instance| instance.alias.clone())
            .collect::<Vec<_>>();
        if busy.is_empty() {
            return Ok(None);
        }
        if host.now_unix_ms().saturating_sub(since) >= loaded.settings.hot_idle_wait_ms {
            return Ok(Some(StepEnd::Ends(
                super::refusal(PLACE, InstallRefusal::InstancesNotIdle).aliases(busy),
                super::deploy::hand_back_own(operation, loaded, notes, false)?,
            )));
        }
        host.pause_ms(POLL_MS);
    }
}

/// Writes `bytes` to `path` (its parent created), flushed to disk.
fn write_file(path: &Path, bytes: &[u8]) -> Result<(), Outcome> {
    if let Some(parent) = path.parent() {
        files::create_dir_all(parent)?;
    }
    let mut file =
        File::create(path).map_err(|error| files::os_failure(path, &error, IoOp::Create))?;
    file.write_all(bytes)
        .map_err(|error| files::os_failure(path, &error, IoOp::Write))?;
    file.sync_all()
        .map_err(|error| files::os_failure(path, &error, IoOp::Sync))
}

/// Writes the item's new content from the zip to `part`.
fn write_part(item: &Item, part: &Path) -> Result<(), Outcome> {
    let zip = item.zip.as_deref().map(PathBuf::from).unwrap_or_default();
    let mut archive = Archive::open(&zip)?;
    for (relative, member) in &item.members {
        let bytes = archive.read_of(item.kind, member)?;
        let target = if relative.is_empty() {
            part.to_path_buf()
        } else {
            files::join_relative(part, relative)
        };
        write_file(&target, &bytes)?;
    }
    Ok(())
}

/// One `pack_item` step: the item placed (or set aside), each half done only when it is missing,
/// so a step interrupted anywhere is finished; the first item waits for idle instances first.
pub(crate) fn item_step(
    operation: &mut Operation<'_>,
    n: usize,
    loaded: &Loaded,
    notes: &mut Vec<Outcome>,
) -> Result<StepEnd, Outcome> {
    let item: Item = operation.intent(n)?;
    let command = operation.journal().kind.location();
    if item.first
        && operation.journal().steps[n - 1].state == StepState::Planned
        && let Some(end) = wait_idle(operation, loaded, notes)?
    {
        return Ok(end);
    }
    operation.start(n)?;
    let dst = PathBuf::from(&item.dst);
    let old = item.old.as_deref().map(PathBuf::from);
    match item.state {
        ItemState::Retire => {
            if dst.exists()
                && let Some(old) = &old
                && !old.exists()
            {
                files::rename(&dst, old)?;
            }
        }
        ItemState::New | ItemState::Replace => {
            if disk_digest(item.kind, &dst)? != item.digest {
                let part = item.part.as_deref().map(PathBuf::from).unwrap_or_default();
                if part.exists() && disk_digest(item.kind, &part)? != item.digest {
                    // A part an earlier attempt left half-written: kept aside, written again.
                    super::undo::set_aside(&part, "abandoned", operation.host().now_unix_ms())?;
                }
                if !part.exists() {
                    write_part(&item, &part)?;
                }
                let host = operation.host();
                let operation_id = operation.id().to_owned();
                super::update::swap(command, &dst, &part, old.clone(), || {
                    host.between_renames(&operation_id, n)
                })?;
                if disk_digest(item.kind, &dst)? != item.digest {
                    return Err(super::refusal(command, InstallRefusal::PackInvalid).path(&dst));
                }
            }
        }
        ItemState::Unchanged => {}
    }
    Ok(StepEnd::Done(Some(json!({
        "item": item.item,
        "kind": item.kind,
        "state": item.state,
        "dst": item.dst,
        "kept": old.filter(|old| old.exists()).map(|old| files::plain(&old)),
    }))))
}

/// One row of the checks after a placement.
fn row(id: &str, result: &str, values: Value) -> Value {
    json!({"id": id, "result": result, "values": values})
}

/// The checks after the placement (see the module's description): every check a row; a failed
/// one ends the command `post_check_failed` with the hold kept.
pub(crate) fn check_step(
    operation: &mut Operation<'_>,
    n: usize,
    loaded: &Loaded,
    notes: &mut Vec<Outcome>,
) -> Result<StepEnd, Outcome> {
    let intent: CheckIntent = operation.intent(n)?;
    let command = operation.journal().kind.location();
    // Workflow #453 S5: no instance bound to the game when the operation began (its input
    // [`BOUND_INPUT`] empty). The workbench reads only the games of configured instances, so no
    // new composition comes; a journal without the input keeps the rule below.
    let unbound = operation
        .journal()
        .inputs
        .get(BOUND_INPUT)
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty);
    operation.start(n)?;
    let host = operation.host();
    let paths = operation.paths().clone();
    let mut rows = Vec::new();
    // 1. The plan of the same bundle again: every item as the placement left it.
    let items = operation
        .journal()
        .steps
        .iter()
        .filter(|step| step.kind == StepKind::PackItem)
        .filter_map(|step| serde_json::from_value::<Item>(step.intent.clone()).ok())
        .collect::<Vec<_>>();
    let mut differ = Vec::new();
    for item in &items {
        let dst = PathBuf::from(&item.dst);
        let holds = match item.state {
            ItemState::Retire => !dst.exists(),
            _ => disk_digest(item.kind, &dst)? == item.digest,
        };
        if !holds {
            differ.push(item.item.clone());
        }
    }
    rows.push(row(
        "rerun",
        if differ.is_empty() { "pass" } else { "fail" },
        json!({"items": items.len(), "differ": differ}),
    ));
    // 2. The Tools' `standard-pack check` of the game root.
    let lab = paths.root.join("tools").join(LAB_PROGRAM);
    if lab.is_file() {
        let ran = programs::run(
            host,
            &lab,
            &[
                "--json",
                "standard-pack",
                "check",
                "--root",
                intent.game_root.as_str(),
            ],
            Deadline::check_config(&loaded.settings),
        );
        rows.push(match ran {
            Ok(ran) if ran.exit_code == Some(0) => {
                row("standard_pack_check", "pass", json!({"exit": 0}))
            }
            Ok(ran) => row(
                "standard_pack_check",
                "fail",
                json!({
                    "exit": ran.exit_code,
                    "answer": serde_json::from_slice::<Value>(files::without_bom(&ran.stdout))
                        .unwrap_or(Value::Null),
                }),
            ),
            Err(failure) => row(
                "standard_pack_check",
                "fail",
                json!({"failure": crate::cli_line::warning(
                    &failure.onto(super::refusal(command, InstallRefusal::PackInvalid))
                )}),
            ),
        });
    } else {
        notes.push(super::caveat(command, InstallCaveat::PackFullCheckSkipped).path(&lab));
        rows.push(row(
            "standard_pack_check",
            "not_evaluated",
            json!({"path": files::plain(&lab)}),
        ));
    }
    // 3. Workflow #453 S6: an operation begun while no Runtime ran checks the live configuration
    // with the live build's check-config ([`check_config_row`]: its result, and its notes that
    // name the game) and leaves the composition itself not evaluated; any other checks the new
    // composition ([`composition_row`]).
    let cold = is_cold(operation);
    let composition = if cold {
        let bound = operation
            .journal()
            .inputs
            .get(BOUND_INPUT)
            .and_then(Value::as_array)
            .map(|bound| {
                bound
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        rows.push(check_config_row(
            host,
            &paths,
            loaded,
            command,
            &intent.game,
            &bound,
        ));
        row(
            "composition",
            "not_evaluated",
            json!({"runtime": "stopped"}),
        )
    } else {
        composition_row(host, &paths, loaded, &intent, unbound)
    };
    rows.push(composition);
    let failed = rows
        .iter()
        .filter(|row| row["result"] == "fail")
        .filter_map(|row| row["id"].as_str().map(str::to_owned))
        .collect::<Vec<_>>();
    let evidence = json!({"game": intent.game, "checks": rows, "failed": failed});
    if failed.is_empty() {
        return Ok(StepEnd::Done(Some(evidence)));
    }
    // Workflow #453 S6 fix 1 (as Workflow #454 closes the post-start checks): the step is closed
    // with what it read, the evidence a passed check records, in the write that ends the
    // operation, with a Runtime and without one.
    operation.close_with_end(n, evidence.clone());
    let (reason, _) = held_of(operation);
    let mut members = hold_kept(&paths, reason.as_ref(), false);
    members.insert("checks".to_owned(), evidence);
    // Workflow #453 S6: no hold to keep; the items stay placed, and `next` names their `undo`.
    if cold {
        members.insert(
            "next".to_owned(),
            json!({"undo": ["install", "undo", "--root", paths.plain, "--operation", operation.id()]}),
        );
    }
    Ok(StepEnd::Ends(
        super::refusal(command, InstallRefusal::PostCheckFailed)
            .count(failed.len())
            .game(intent.game),
        members,
    ))
}

/// The row `composition` of the checks after a placement while a Runtime runs: the catalog of the
/// latest evaluation changed, within the workbench's check interval and the hot idle wait, when
/// the game's rules changed and (Workflow #453 S5) an instance was bound to the game (`unbound`
/// false); with none bound, the catalog read once and expected unchanged.
fn composition_row(
    host: &dyn Host,
    paths: &Paths,
    loaded: &Loaded,
    intent: &CheckIntent,
    unbound: bool,
) -> Value {
    let interval = actingcommand_runtime_config::member(&loaded.config, CHECK_INTERVAL_KEY)
        .and_then(Value::as_u64)
        .or_else(|| {
            actingcommand_runtime_config::member(
                &super::plan::defaults_of(&paths.root),
                CHECK_INTERVAL_KEY,
            )
            .and_then(Value::as_u64)
        })
        .unwrap_or(0);
    let runtime = host.runtime();
    let since = host.now_unix_ms();
    loop {
        let read = runtime.catalog_hash(&loaded.state_root);
        let now = match &read {
            Ok(hash) => hash.clone(),
            Err(failure) => {
                break row(
                    "composition",
                    "not_evaluated",
                    json!({"failure": crate::cli_line::warning(failure)}),
                );
            }
        };
        if !intent.schedule_changes {
            break row(
                "composition",
                "pass",
                json!({"catalog_hash": now, "catalog_before": intent.catalog_before, "expected": "unchanged"}),
            );
        }
        if unbound {
            break row(
                "composition",
                "pass",
                json!({"catalog_hash": now, "catalog_before": intent.catalog_before, "expected": "unchanged", "bound": []}),
            );
        }
        if now != intent.catalog_before {
            break row(
                "composition",
                "pass",
                json!({"catalog_hash": now, "catalog_before": intent.catalog_before, "expected": "new"}),
            );
        }
        let waited = host.now_unix_ms().saturating_sub(since);
        if waited >= interval.saturating_add(loaded.settings.hot_idle_wait_ms) {
            break row(
                "composition",
                "fail",
                json!({"catalog_hash": now, "waited_ms": waited, "expected": "new"}),
            );
        }
        host.pause_ms(POLL_MS);
    }
}

/// Workflow #453 S6: the row `check_config` of the checks after a placement begun while no
/// Runtime ran: the live build's check-config of the live configuration with `--install-root
/// <root>`, as `config replace` checks a file (`super::config_swap::candidate`). A configuration
/// it refuses fails with its answer; one it does not answer fails with
/// `check_config_unanswered`. Fix 1: check-config answers `check_config_passed` with a problem of
/// a game's root or of its composition as a note, so the row also fails when a note names the
/// placed `game` (its value `game`), those notes in `game_notes`; and with no instance bound to
/// the game (`bound` empty) check-config did not preview its composition, which the row says as
/// `composition` `not_previewed` beside check-config's result for the rest of the configuration.
/// Fix 2: the outcome keeps at most [`actingcommand_contract::outcome::CAUSES_MAX_LINKS`] links
/// within its byte bound and check-config joins the composition's problems last, so they are cut
/// first; the row judges the game from the report, which nothing cuts ([`GameReport`]). An outcome
/// whose links were cut (`causes_total`) says so (`notes` `cut`, `causes_total`), and fails the
/// row when the report cannot rule out the one note of the game it does not carry (`game_tables`
/// `not_judged`).
fn check_config_row(
    host: &dyn Host,
    paths: &Paths,
    loaded: &Loaded,
    command: InstallLocation,
    game: &str,
    bound: &[String],
) -> Value {
    match programs::check_config(
        host,
        &paths.root.join(manifest::Half::Runtime.dir()),
        &paths.config(),
        Some(&paths.root),
        Deadline::check_config(&loaded.settings),
    ) {
        Ok(checked) => {
            let game_notes = checked
                .received
                .causes()
                .iter()
                .filter(|link| *link.relation() == CauseRelation::Note)
                .map(|link| link.outcome())
                .filter(|note| note.value("game").and_then(Value::as_str) == Some(game))
                .map(crate::cli_line::warning)
                .collect::<Vec<_>>();
            // Fix 2: the game as the report, whole, says it; and an outcome whose links were cut
            // may lack the one note of the game the report does not carry, which only an entry
            // that lists a table rules out.
            let whole = GameReport::of(&checked.report, game);
            let cut = checked.received.causes_total();
            let tables_not_judged = cut.is_some() && !whole.tables_listed;
            let passed =
                checked.passed() && game_notes.is_empty() && whole.passes() && !tables_not_judged;
            let mut values = json!({
                "install_root": checked.install_root,
                "answer": crate::cli_line::warning(&checked.received),
                "packs": checked.summary.packs,
                "game_notes": game_notes,
                "bound": bound,
            });
            if bound.is_empty() {
                values["composition"] = json!("not_previewed");
            }
            whole.record(&mut values);
            if let Some(total) = cut {
                values["notes"] = json!("cut");
                values["causes_total"] = json!(total);
            }
            if tables_not_judged {
                values["game_tables"] = json!("not_judged");
            }
            row("check_config", if passed { "pass" } else { "fail" }, values)
        }
        Err(failure) => row(
            "check_config",
            "fail",
            json!({"failure": crate::cli_line::warning(
                &failure.onto(super::refusal(command, InstallRefusal::CheckConfigUnanswered))
            )}),
        ),
    }
}

/// Workflow #453 S6 fix 2: the problem a collision across the games in use records (two composed
/// rules, pools, profiles or events with one id, or two games' rules naming one package), on the
/// game whose name sorts later; a file's own repeated id records the same token.
const COLLISION: StandardPackProblem = StandardPackProblem::RuleIdConflict;

/// Workflow #453 S6 fix 2: what check-config's report says of the placed game. The report is
/// printed beside the outcome and no bound of the outcome applies to it
/// ([`programs::Checked::report`]), so its members are whole: the game's entry of
/// `standard_packs.games` (every game root below `standard_packs.root`, read whatever binds it)
/// and the composition's problems (`standard_pack_schedule.problems`, the games of
/// `policy.facts.instances` only).
struct GameReport {
    /// The game's entry when it records a problem: its root's `problem`, or a table's `problem`
    /// or `expired_columns` (each a note of check-config naming the game).
    root: Option<Value>,
    /// Whether the entry lists a table, so that check-config listed the game's `tables/`: a
    /// `tables/` it cannot list is the one problem of the game the report does not carry (a note
    /// only, the entry's `tables` then empty).
    tables_listed: bool,
    /// The composition's problems that name the game.
    named: Vec<Value>,
    /// The composition's problems that name no game: the composition refused as a whole, which
    /// fails every placement.
    refused: Vec<Value>,
    /// Each [`COLLISION`] the composition records on another game: it names the game it is
    /// recorded on and not the other one, so whether the placed bundle caused it cannot be told.
    collisions: Vec<Value>,
}

impl GameReport {
    /// What `report` (check-config's `report`, `null` without one) says of `game`.
    fn of(report: &Value, game: &str) -> Self {
        let entry = report
            .pointer("/standard_packs/games")
            .and_then(Value::as_array)
            .and_then(|games| games.iter().find(|entry| entry["game"] == game));
        let tables = entry
            .and_then(|entry| entry["tables"].as_array())
            .map(Vec::as_slice)
            .unwrap_or_default();
        let troubled = entry.is_some_and(|entry| !entry["problem"].is_null())
            || tables.iter().any(|table| {
                !table["problem"].is_null()
                    || table["expired_columns"]
                        .as_array()
                        .is_some_and(|columns| !columns.is_empty())
            });
        let mut found = Self {
            root: entry.filter(|_| troubled).cloned(),
            tables_listed: !tables.is_empty(),
            named: Vec::new(),
            refused: Vec::new(),
            collisions: Vec::new(),
        };
        let problems = report
            .pointer("/standard_pack_schedule/problems")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        for problem in problems {
            let kind = problem
                .pointer("/values/standard_pack_problem")
                .and_then(Value::as_str);
            match problem.pointer("/values/game").and_then(Value::as_str) {
                Some(on) if on == game => found.named.push(problem.clone()),
                None => found.refused.push(problem.clone()),
                Some(_) if kind == Some(COLLISION.as_str()) => {
                    found.collisions.push(problem.clone());
                }
                Some(_) => {}
            }
        }
        found
    }

    /// Whether nothing here fails the row.
    fn passes(&self) -> bool {
        self.root.is_none()
            && self.named.is_empty()
            && self.refused.is_empty()
            && self.collisions.is_empty()
    }

    /// Its members in the row's `values`, each only when it holds something: `game_root`,
    /// `composition_problems`, `composition_refused`, and `collisions` with `collision_cause`
    /// `not_told`.
    fn record(self, values: &mut Value) {
        if let Some(root) = self.root {
            values["game_root"] = root;
        }
        if !self.named.is_empty() {
            values["composition_problems"] = Value::Array(self.named);
        }
        if !self.refused.is_empty() {
            values["composition_refused"] = Value::Array(self.refused);
        }
        if !self.collisions.is_empty() {
            values["collisions"] = Value::Array(self.collisions);
            values["collision_cause"] = json!("not_told");
        }
    }
}

/// `undo` of one `pack_item` step: the placed item set aside as `.rolledback-<ms2>` and the kept
/// one put back; a root file, the notices directory or the loading templates directory set aside
/// put back; a part left behind set aside as `.abandoned-<ms2>`. The paths it set aside.
pub(crate) fn reverse_item(
    _operation: &Operation<'_>,
    step: &Step,
    ms2: u64,
) -> Result<Vec<String>, Outcome> {
    let item: Item =
        serde_json::from_value(step.intent.clone()).map_err(|error| files::json_failure(&error))?;
    let dst = PathBuf::from(&item.dst);
    let old = item.old.as_deref().map(PathBuf::from);
    let mut evidence = Vec::new();
    match item.state {
        ItemState::Retire => {
            if let Some(old) = &old
                && old.exists()
                && !dst.exists()
            {
                files::rename(old, &dst)?;
            }
        }
        ItemState::New | ItemState::Replace => {
            if dst.exists() && disk_digest(item.kind, &dst)? == item.digest {
                evidence.extend(super::undo::set_aside(&dst, "rolledback", ms2)?);
            }
            if let Some(old) = &old
                && old.exists()
                && !dst.exists()
            {
                files::rename(old, &dst)?;
            }
            if let Some(part) = &item.part {
                evidence.extend(super::undo::set_aside(Path::new(part), "abandoned", ms2)?);
            }
        }
        ItemState::Unchanged => {}
    }
    Ok(evidence)
}
