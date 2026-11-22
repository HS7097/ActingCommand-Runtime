// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #477 PC-1 (register §1.438; contracts/pack-catalog.md, "Who writes it"): `actinglab
//! standard-pack catalog --root <root> [--plan]`, the writer of a game root's `pack-catalog.json`,
//! so that nobody edits the file by hand.
//!
//! Fix round 1 (the independent review's first point): `--root` takes either of two directories.
//!
//! - **A resource repository's checkout**: a directory that holds `applications.json` and no
//!   `bundle.json`, as every resource repository's checkout does (its `bundle.json` is made at build
//!   time). Its game is the one `applications.json` names, and its registry is the `bundle.json`
//!   the build's own function, `package_bundle`, makes of `applications.json` and the directories
//!   of `packs/` (`checkout_registry`): each directory is copied, following links as the build's
//!   copy does, into a scratch directory of the system's temporary folder, which is removed again
//!   before the line answers. The file is written at the checkout's root, where the build carries
//!   it from.
//! - **A built game root** outside an installation: a directory that holds `bundle.json`, named as
//!   its game (the form of the first round, kept: it costs nothing).
//!
//! Either way it builds or brings up to date the file from what the root already declares, by the
//! contract crate's one rule ([`merge_standard_pack_catalog`]): an entry for every package of the
//! registry; a new entry's display from a leftover `ui.json`, else none; its `consumes` and
//! `produces` the union over the package's rules in `schedule.json`; an entry already there kept as
//! written, its differences from the rules reported, never overwritten silently; what the root no
//! longer declares left out and warned of (`standard_pack_catalog_left_out`, fix round 1). With
//! `--plan` it writes nothing and answers the document it would write. Without it, it writes the
//! file atomically (a `.part` beside it, renamed over it) when its bytes differ, and leaves it as it
//! is when they do not; a write that fails is `standard_pack_catalog_write_failed`, of the class
//! device or instance (fix round 1). A root inside an installation is refused
//! (`standard_pack_catalog_root_refused`): an installed root takes the file through the build and
//! the installer's `place`. Of the root it reads `bundle.json` (or `applications.json` and
//! `packs/`), `schedule.json`, `pack-catalog.json` and `ui.json`; in it, it writes only
//! `pack-catalog.json` (and its `.part`) and deletes nothing: a leftover `ui.json` stays, and its
//! warning with it.
//!
//! Workflow #477 PC-2 (register §1.462): `--refresh <package_id>` (repeatable) brings the named
//! entries' business part up to date from their rules, their display and their place kept, and
//! `--displays <file>` (an absolute path) replaces the named entries' displays whole by a display
//! file's, both by the same rule ([`merge_standard_pack_catalog`] with its `CatalogEdits`). A
//! package `--refresh` names that the registry does not hold, or names twice, is
//! `standard_pack_catalog_refresh_refused`; a display file the catalog's own checks refuse is
//! `standard_pack_catalog_display_refused`; either before anything is written. The answer gains
//! `refreshed` and `display_written`, each only when its option is given, so that the line without
//! them answers as before, byte for byte. Fix round 1: the display file carries its
//! `schema_version` and may carry groups, which the line adds (`groups_added`, with `--displays`
//! only); a `--refresh` id an answer could not name (over 256 bytes, or with a control character)
//! is a usage error before the root is read, its value not echoed.
//!
//! Workflow #477 CV-1 (register §1.507): `--schema-version 2` writes version 2 (a file of version 2
//! keeps it without the flag), converting a file of version 1, each entry's business part then
//! its rules' (`converted`); `--values <file>` (an absolute path) replaces the named parts' value
//! and marks, refused as the display file is (`standard_pack_catalog_display_refused`, naming the
//! values file), and a usage error on a line that writes version 1. The answer gains `converted`
//! and `values_written`, each only with its option.

use crate::standard_pack_check::{catalog_refused, read_bounded, root_invalid};
use crate::{
    PackageBundleRequest, StandardPackCatalogRequest, StandardPackCatalogResponse,
    StandardPackRootFiles, installed_root_containing, package_bundle,
};
use actingcommand_contract::codes::{ContractCode, SharedCode};
use actingcommand_contract::outcome::{
    Detail, IoOp, KeyKind, NAME_MAX_BYTES, Outcome, key_text_fits,
};
use actingcommand_contract::{
    CatalogDisplayFile, CatalogDisplayFileIssue, CatalogEdits, CatalogMergeRefusal,
    CatalogValuesFile, LabError, LabErrorClass, LabResult, STANDARD_PACK_BUNDLE_FILE,
    STANDARD_PACK_CATALOG_FILE, STANDARD_PACK_RETIRED_UI_FILE, STANDARD_PACK_SCHEDULE_FILE,
    StandardPackProblem, StandardPackRegistry, StandardPackRootIssue, catalog_display_outcome,
    merge_standard_pack_catalog, parse_standard_pack_schedule, retired_ui_outcome,
};
use serde_json::{Value, json};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The answer's `status` of a plan.
const PLANNED: &str = "planned";
/// The answer's `status` of a line that wrote the file.
const WRITTEN: &str = "written";
/// The answer's `status` of a line that found the file holding the bytes already.
const UNCHANGED: &str = "unchanged";
/// A checkout's applications table, from which the build makes `bundle.json`.
const APPLICATIONS_FILE: &str = "applications.json";
/// A checkout's directory of package directories.
const PACKS_DIRECTORY: &str = "packs";
/// The scratch directories' names below the system's temporary folder,
/// `<prefix>-<process>-<count>`.
const SCRATCH_PREFIX: &str = "actinglab-pack-catalog";
/// The count of the scratch directories this process made, so that each call takes its own.
static SCRATCH_COUNT: AtomicU64 = AtomicU64::new(0);

/// `standard-pack catalog` (module doc).
pub fn write_standard_pack_catalog(
    request: StandardPackCatalogRequest,
) -> LabResult<StandardPackCatalogResponse> {
    refresh_ids_fit(&request.refresh)?;
    let root = std::path::absolute(&request.root)
        .map_err(|error| LabError::usage(format!("{}: {error}", request.root.display())))?;
    if !fs::metadata(&root).is_ok_and(|metadata| metadata.is_dir()) {
        return Err(LabError::usage(format!(
            "--root {} is no directory",
            root.display()
        )));
    }
    if let Some(installation) = installed_root_containing(&root)? {
        return Err(refused(
            Outcome::new(SharedCode::StandardPackCatalogRootRefused)
                .path(&root)
                .install_root(&installation),
            LabErrorClass::UsageValidation,
            format!(
                "{} lies inside the installation {}; standard-pack catalog writes a resource \
                 repository's checkout or a built game root outside an installation only, and an \
                 installed root takes the file through standard-pack build and place",
                root.display(),
                installation.display()
            ),
        ));
    }
    let (game, registry) = root_registry(&root)?;
    let schedule_path = root.join(STANDARD_PACK_SCHEDULE_FILE);
    let schedule = read_bounded(&schedule_path)?
        .map(|bytes| parse_standard_pack_schedule(&bytes))
        .transpose()
        .map_err(|issue| root_invalid(&game, &schedule_path, &issue))?;
    let path = root.join(STANDARD_PACK_CATALOG_FILE);
    let existing = read_bounded(&path)?;
    let retired_path = root.join(STANDARD_PACK_RETIRED_UI_FILE);
    let retired = read_bounded(&retired_path)?;
    // A leftover `ui.json` that is no JSON gives no display; its warning names it either way.
    let retired_document = retired
        .as_deref()
        .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok());
    // PC-2: the display file, read whole; the contract crate checks it.
    let displays = request
        .displays
        .as_deref()
        .map(|path| read_input_file(DISPLAYS_FLAG, path))
        .transpose()?;
    // Workflow #477 CV-1: the values file, read whole as the display file is.
    let values = request
        .values
        .as_deref()
        .map(|path| read_input_file(VALUES_FLAG, path))
        .transpose()?;
    let edits = CatalogEdits {
        refresh: &request.refresh,
        displays: displays.as_ref().map(|(path, bytes)| CatalogDisplayFile {
            path: path.as_path(),
            bytes: bytes.as_slice(),
        }),
        version_2: request.version_2,
        values: values.as_ref().map(|(path, bytes)| CatalogValuesFile {
            path: path.as_path(),
            bytes: bytes.as_slice(),
        }),
    };
    let merge = merge_standard_pack_catalog(
        &game,
        &registry,
        schedule.as_ref(),
        existing.as_deref(),
        retired_document.as_ref(),
        &edits,
    )
    .map_err(|refusal| match refusal {
        CatalogMergeRefusal::Refresh(refusal) => refused(
            refusal.outcome(&game),
            LabErrorClass::UsageValidation,
            format!(
                "--refresh {}: {} (standard-pack catalog brings up to date only an entry of a \
                 package the root's registry holds, each named once)",
                refusal.package_id, refusal.problem
            ),
        ),
        // Fix round 1: the first refusal names the file it is in (`schedule.json` for a rule's
        // pool that file does not declare).
        CatalogMergeRefusal::Root(issues) => catalog_refused(&game, &root, issues),
        CatalogMergeRefusal::Display { path, issues } => display_refused(
            &game,
            &path,
            issues,
            "the display file is checked by the catalog's own display checks",
        ),
        // Workflow #477 CV-1: the values file's refusal is the display file's, naming its file.
        CatalogMergeRefusal::Values { path, issues } => display_refused(
            &game,
            &path,
            issues,
            "the values file is checked by the catalog's own value and mark checks",
        ),
        CatalogMergeRefusal::ValuesNeedVersion2 => LabError::usage(format!(
            "{VALUES_FLAG} writes a fixed value and marks, which only a catalog of version 2 \
             carries: {} is of version 1 or absent; give {VERSION_FLAG} 2 to write version 2",
            path.display()
        )),
    })?;
    let changed = existing.as_deref() != Some(merge.bytes.as_slice());
    let mut warnings = merge
        .catalog
        .display_issues()
        .iter()
        .map(|issue| catalog_display_outcome(&game, &path, issue))
        .collect::<Vec<_>>();
    // Fix round 1: what the writer left out, warned of as well as answered.
    warnings.extend(
        merge
            .left_out
            .iter()
            .map(|left_out| left_out.outcome(&game, &path)),
    );
    warnings.extend(
        merge
            .differences
            .iter()
            .map(|difference| difference.outcome(&game)),
    );
    if retired.is_some() {
        warnings.push(retired_ui_outcome(&game, &retired_path));
    }
    let (status, document) = if request.plan {
        let document = serde_json::from_slice::<Value>(&merge.bytes).map_err(|error| {
            LabError::usage(format!(
                "the document standard-pack catalog makes does not read back: {error}"
            ))
        })?;
        (PLANNED, Some(document))
    } else if changed {
        write_atomically(&path, &merge.bytes)?;
        (WRITTEN, None)
    } else {
        (UNCHANGED, None)
    };
    Ok(StandardPackCatalogResponse {
        status: status.to_owned(),
        root: root.display().to_string(),
        game,
        path: path.display().to_string(),
        changed,
        added: merge.added,
        kept: merge.kept,
        removed: merge.removed,
        dropped: merge.dropped,
        differences: merge.differences,
        display_from_ui: merge.display_from_ui,
        refreshed: (!request.refresh.is_empty()).then_some(merge.refreshed),
        display_written: request.displays.is_some().then_some(merge.display_written),
        groups_added: request.displays.is_some().then_some(merge.groups_added),
        converted: request.version_2.then_some(merge.converted),
        values_written: request.values.is_some().then_some(merge.values_written),
        document,
        warnings,
    })
}

/// Workflow #477 PC-2 fix round 1 (the independent review): each `--refresh` id is one an answer's
/// `package_id` (a name of the key table: at most [`NAME_MAX_BYTES`] bytes, no control character)
/// can carry; the first that is not is a usage error (`validation_failed`) before the root is read,
/// naming the parameter, the id's place and its length, never its value.
fn refresh_ids_fit(refresh: &[String]) -> LabResult<()> {
    match refresh
        .iter()
        .enumerate()
        .find(|(_, package_id)| !key_text_fits(KeyKind::Name, package_id))
    {
        Some((index, package_id)) => Err(LabError::usage(format!(
            "--refresh value {} ({} bytes) is no package id: a package id is at most \
             {NAME_MAX_BYTES} bytes, without a control character",
            index + 1,
            package_id.len()
        ))),
        None => Ok(()),
    }
}

/// Workflow #477 PC-2: the flag of the display file. Workflow #477 CV-1: the flags of the values
/// file and of the version asked for, which the refusals name.
const DISPLAYS_FLAG: &str = "--displays";
const VALUES_FLAG: &str = "--values";
const VERSION_FLAG: &str = "--schema-version";

/// Workflow #477 PC-2: the display file `--displays` names, an absolute path, and its bytes; CV-1:
/// and so the values file `--values` names (`flag` the one that names it). A relative path, a file
/// that does not exist, cannot be read or exceeds the package input byte limit is a usage error
/// naming it (`validation_failed`), as a root file's is; its content is the contract crate's to
/// check.
fn read_input_file(flag: &str, path: &Path) -> LabResult<(PathBuf, Vec<u8>)> {
    if !path.is_absolute() {
        return Err(LabError::usage(format!(
            "{flag} {} is no absolute path",
            path.display()
        )));
    }
    match read_bounded(path)? {
        Some(bytes) => Ok((path.to_owned(), bytes)),
        None => Err(LabError::usage(format!(
            "{flag} {}: no such file",
            path.display()
        ))),
    }
}

/// Workflow #477 PC-2: the refusal `standard_pack_catalog_display_refused` of the display file at
/// `path` of the game root of `game`, with its first error (the contract crate answers at least
/// one), of the class usage or validation: the line names a file that cannot be written into the
/// catalog as it stands. Workflow #477 CV-1: the same refusal of the values file (`checks` what
/// the message says the file is checked by).
fn display_refused(
    game: &str,
    path: &Path,
    issues: Vec<CatalogDisplayFileIssue>,
    checks: &str,
) -> LabError {
    let first = issues.into_iter().next().unwrap_or_else(whole_display_file);
    refused(
        first.outcome(game, path),
        LabErrorClass::UsageValidation,
        format!(
            "{}: standard_pack_problem={} json_pointer={} ({checks} before anything is written)",
            path.display(),
            first.problem,
            first.pointer
        ),
    )
}

/// The display file as a whole, `malformed`: a refusal without an error is the whole file's.
fn whole_display_file() -> CatalogDisplayFileIssue {
    CatalogDisplayFileIssue {
        problem: StandardPackProblem::Malformed,
        pointer: String::new(),
    }
}

/// The game and the registry of the directory `root` (module doc). A directory that holds
/// `bundle.json` is a built game root: its game is its directory's name and its registry that
/// file's. One that holds `applications.json` instead is a checkout ([`checkout_registry`]). One
/// that holds neither is `standard_pack_root_invalid`, `bundle_missing`, at `bundle.json`.
fn root_registry(root: &Path) -> LabResult<(String, StandardPackRegistry)> {
    let bundle_path = root.join(STANDARD_PACK_BUNDLE_FILE);
    let named = || {
        root.file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
            .ok_or_else(|| {
                LabError::usage(format!("--root {} names no game directory", root.display()))
            })
    };
    if let Some(bundle) = read_bounded(&bundle_path)? {
        let game = named()?;
        let registry = StandardPackRegistry::from_bundle_bytes(&bundle, &game)
            .map_err(|issue| root_invalid(&game, &bundle_path, &issue))?;
        return Ok((game, registry));
    }
    if fs::symlink_metadata(root.join(APPLICATIONS_FILE)).is_ok() {
        return checkout_registry(root);
    }
    let mut error = root_invalid(
        &named()?,
        &bundle_path,
        &StandardPackRootIssue {
            file: STANDARD_PACK_BUNDLE_FILE,
            problem: StandardPackProblem::BundleMissing,
            pointer: String::new(),
        },
    );
    error.message = format!(
        "{}; --root takes a resource repository's checkout (applications.json and packs/) or a \
         built game root (bundle.json)",
        error.message
    );
    Err(error)
}

/// The game and the registry of the checkout `root`: the build's own `package_bundle` of its
/// `applications.json` and of each directory of its `packs/` (a file there, such as a
/// `README.md`, is no package and is passed over, as the build's layout passes it over), with no
/// root file, made in a scratch directory of the system's temporary folder; the game is the
/// bundle's, which `applications.json` names, and the registry that of the `bundle.json` it writes.
/// The scratch directory is removed whatever the bundle's end. A refusal of `package_bundle` is
/// its own (a checkout without a package is `package_bundle_packs_empty`); an OS failure of the
/// scratch directory is `foreign_os_error` ([`scratch_failed`]), and one that cannot be removed
/// refuses the line too, naming it.
fn checkout_registry(root: &Path) -> LabResult<(String, StandardPackRegistry)> {
    let scratch = make_scratch()?;
    let made = bundle_in(root, &scratch);
    let removed = fs::remove_dir_all(&scratch);
    match (made, removed) {
        (Ok(made), Ok(())) => Ok(made),
        (Ok(_), Err(error)) => Err(scratch_failed(&scratch, &error, IoOp::Remove)),
        (Err(mut error), Ok(())) => {
            error.message = format!(
                "{}; the scratch directory {} the writer made was removed",
                error.message,
                scratch.display()
            );
            Err(error)
        }
        (Err(mut error), Err(removal)) => {
            error.message = format!(
                "{}; the scratch directory {} the writer made could not be removed: {removal}",
                error.message,
                scratch.display()
            );
            Err(error)
        }
    }
}

/// A new scratch directory `<temporary folder>/actinglab-pack-catalog-<process>-<count>`; a name
/// that is taken already (a leftover of an earlier process) is passed over for the next count.
fn make_scratch() -> LabResult<PathBuf> {
    let temporary = std::env::temp_dir();
    loop {
        let scratch = temporary.join(format!(
            "{SCRATCH_PREFIX}-{}-{}",
            std::process::id(),
            SCRATCH_COUNT.fetch_add(1, Ordering::Relaxed)
        ));
        match fs::create_dir(&scratch) {
            Ok(()) => return Ok(scratch),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(scratch_failed(&scratch, &error, IoOp::CreateDir)),
        }
    }
}

/// [`checkout_registry`]'s work in `scratch`: the directories of `<root>/packs` copied into
/// `<scratch>/packs-root`, in name order, then `package_bundle` into `<scratch>/bundle`.
fn bundle_in(root: &Path, scratch: &Path) -> LabResult<(String, StandardPackRegistry)> {
    let stage = scratch.join("packs-root");
    fs::create_dir(&stage).map_err(|error| scratch_failed(&stage, &error, IoOp::CreateDir))?;
    let packs = root.join(PACKS_DIRECTORY);
    let mut names = Vec::new();
    match fs::read_dir(&packs) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry.map_err(|error| scratch_failed(&packs, &error, IoOp::ReadDir))?;
                let source = entry.path();
                let metadata = fs::symlink_metadata(&source)
                    .map_err(|error| scratch_failed(&source, &error, IoOp::Metadata))?;
                if metadata.is_dir() {
                    names.push(entry.file_name());
                }
            }
        }
        // No `packs/`: `package_bundle` refuses the empty table.
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(scratch_failed(&packs, &error, IoOp::ReadDir)),
    }
    names.sort();
    for name in names {
        copy_tree(&packs.join(&name), &stage.join(&name))?;
    }
    let out = scratch.join("bundle");
    let response = package_bundle(PackageBundleRequest {
        applications: root.join(APPLICATIONS_FILE),
        packs_root: stage,
        out: out.clone(),
        source: None,
        maintenance: None,
        root_files: StandardPackRootFiles::default(),
    })?;
    let game = response.index.game;
    let bundle_path = out.join(STANDARD_PACK_BUNDLE_FILE);
    let bundle =
        fs::read(&bundle_path).map_err(|error| scratch_failed(&bundle_path, &error, IoOp::Read))?;
    let registry = StandardPackRegistry::from_bundle_bytes(&bundle, &game)
        .map_err(|issue| root_invalid(&game, &bundle_path, &issue))?;
    Ok((game, registry))
}

/// Copies the directory tree `from` into the new directory `to`, file by file, following links as
/// the build's copy of a checkout's packs does.
fn copy_tree(from: &Path, to: &Path) -> LabResult<()> {
    fs::create_dir(to).map_err(|error| scratch_failed(to, &error, IoOp::CreateDir))?;
    let entries =
        fs::read_dir(from).map_err(|error| scratch_failed(from, &error, IoOp::ReadDir))?;
    for entry in entries {
        let entry = entry.map_err(|error| scratch_failed(from, &error, IoOp::ReadDir))?;
        let source = entry.path();
        let target = to.join(entry.file_name());
        let metadata = fs::metadata(&source)
            .map_err(|error| scratch_failed(&source, &error, IoOp::Metadata))?;
        if metadata.is_dir() {
            copy_tree(&source, &target)?;
        } else {
            let bytes =
                fs::read(&source).map_err(|error| scratch_failed(&source, &error, IoOp::Read))?;
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)
                .and_then(|mut file| file.write_all(&bytes))
                .map_err(|error| scratch_failed(&target, &error, IoOp::Write))?;
        }
    }
    Ok(())
}

/// Fix round 1: `foreign_os_error` at `path`, the operating system's error of `op` (a checkout's
/// `packs/` that cannot be read, or the scratch directory its registry is made in that cannot be
/// made, written, read or removed), of the class device or instance, as the Lab's other local
/// write failures: the line was right, the machine could not carry it out.
fn scratch_failed(path: &Path, error: &io::Error, op: IoOp) -> LabError {
    LabError::new(
        LabErrorClass::DeviceInstance,
        ContractCode::ForeignOsError.as_str(),
        format!(
            "{}: {error} (while making the registry of the checkout's packages)",
            path.display()
        ),
        &[],
    )
    .with_details(
        json!({"outcome": Outcome::from_io_error(error, op).path(path).envelope(Detail::Full)}),
    )
}

/// Writes `bytes` to `path` atomically: to `<path>.part` (made anew, synced), then renamed over
/// `path`. A failure is `standard_pack_catalog_write_failed` naming `path`, the OS error beneath
/// it, of the class device or instance (fix round 1: no usage refusal); the file in place is then
/// as it was.
fn write_atomically(path: &Path, bytes: &[u8]) -> LabResult<()> {
    let part = part_path(path);
    let failed = |error: &io::Error, op: IoOp| {
        refused(
            Outcome::new(SharedCode::StandardPackCatalogWriteFailed)
                .path(path)
                .caused_by(Outcome::from_io_error(error, op).path(&part)),
            LabErrorClass::DeviceInstance,
            format!("{} could not be written: {error}", path.display()),
        )
    };
    fs::File::create(&part)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_all()
        })
        .map_err(|error| failed(&error, IoOp::Write))?;
    fs::rename(&part, path).map_err(|error| failed(&error, IoOp::Rename))
}

/// `<path>.part`.
fn part_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    path.with_file_name(name)
}

/// The Lab error of the refusal `outcome`, of `class`.
fn refused(outcome: Outcome, class: LabErrorClass, message: String) -> LabError {
    LabError::new(class, outcome.code().as_str(), message, &[])
        .with_details(json!({"outcome": outcome.envelope(Detail::Full)}))
}
