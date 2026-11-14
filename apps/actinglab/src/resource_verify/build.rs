// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #464 P2 (`contracts/resource-verify.md`, "`standard-pack build`";
//! `contracts/package-reference.md`, "The built standard pack"): `actinglab standard-pack build
//! --repo <checkout> --source-repository <owner>/<name> --out <directory> [--commit <40-hex>]`
//! builds a game's whole standard pack from one commit of its working root, ready for `place`.
//!
//! - **The steps** are `resource verify`'s, on their one function ([`super::run_steps`],
//!   [`StepsFor::Build`]), every one of them, its work under `<out>/work/` as `resource verify`'s
//!   under `--work`. A tree with any change, an untracked file or an ignored file where the build
//!   reads is refused by the checkout precondition, so every build names its commit and its tree.
//! - **`--out`** is a directory that does not exist, whose parent does, or an existing empty one;
//!   never one inside `--repo`. Anything else is refused before anything is read or written. Fix
//!   round 1: a `--out` that does not exist is made by the bundle step, once the checkout
//!   precondition passed, so a call the precondition refuses leaves nothing behind.
//! - **The product**, written only when every step passed: the steps' zip, copied byte for byte,
//!   `SHA256SUMS` (`<hex>  <name>\n`) and `build.json` (`actingcommand.standard-pack-build.v1`),
//!   each written under its `.part` name and renamed when all three are complete, `build.json`
//!   last (fix round 1). `build.json` says the source repository, the commit
//!   and the tree, the Tools head and executable SHA-256, and, read back from the zip, the game,
//!   the `package` range its `interfaces.json` declares, its root files and root directories,
//!   its packs and its name, SHA-256 and size. It has no readiness member: another slice judges
//!   readiness. Steps that did not all pass write no product: `resource_build_failed`, its
//!   cause the steps' outcome, its details the steps' report (`actinglab.resource-verify.v1`),
//!   which is no file.
//!
//! Nothing is written into the repository and nothing is deleted.

use super::bundle::ZIP_DIRECTORY;
use super::{
    BUILD_FLAGS, Counts, Report, StepReport, StepsFor, StepsRequest, ZipCounts, is_commit,
    outcome_error, owner_name, part_path, path_value, run_steps, sha256_hex,
};
use crate::codes::{ActinglabCode, VerifyStep};
use crate::{CliError, CliOutcome, ErrorKind, FlagArgs, GlobalOptions};
use actingcommand_contract::outcome::{Detail, IoOp, Outcome, OutcomeEnvelope};
use actingcommand_contract::{BundleIndex, STANDARD_PACK_BUNDLE_FILE};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Cursor, Read, Write};
use std::path::{Path, PathBuf};

/// `build.json`'s schema, also the `build_schema` of the capability entry.
pub(crate) const BUILD_SCHEMA: &str = "actingcommand.standard-pack-build.v1";
/// Below `--out`: the steps' work directory.
const WORK_DIRECTORY: &str = "work";
const SUMS_FILE: &str = "SHA256SUMS";
const BUILD_FILE: &str = "build.json";
/// The zip's declaration of its `package` range, beside `bundle.json`.
const INTERFACES_FILE: &str = "interfaces.json";
/// The zip's directory of packs, which is no root directory of `build.json`.
const PACKS_DIRECTORY: &str = "packs";

pub(crate) fn build_options() -> [&'static str; 4] {
    [
        "--repo <the top of a clean Git work tree whose HEAD is the commit to build>",
        "--source-repository <owner/name, written into bundle.json source and build.json>",
        "--out <a new directory whose parent exists, or an empty one, not inside --repo: the zip, SHA256SUMS and build.json, and the steps' work in work/>",
        "--commit <40-hex commit: must be HEAD>",
    ]
}

/// What the command was asked, its paths absolute.
struct BuildRequest {
    /// The steps', their work `<out>/work`.
    steps: StepsRequest,
    out: PathBuf,
}

/// `build.json` (`actingcommand.standard-pack-build.v1`).
#[derive(Debug, Serialize)]
struct BuildDocument {
    schema_version: &'static str,
    game: String,
    source_repository: String,
    commit: String,
    tree: String,
    tools: BuildTools,
    /// The range the zip's `interfaces.json` declares.
    package: [u64; 2],
    /// The files at the zip's root, in byte order.
    root_files: Vec<String>,
    /// The directories at the zip's root but `packs`, in byte order.
    root_directories: Vec<String>,
    /// The zip's `bundle.json` packs, in its order.
    packs: Vec<BuildPack>,
    zip: BuildZip,
}

#[derive(Debug, Serialize)]
struct BuildTools {
    runtime_head: &'static str,
    executable_sha256: String,
}

#[derive(Debug, Serialize)]
struct BuildPack {
    package_id: String,
    server: String,
    entry_task_id: String,
    file_count: u64,
    byte_count: u64,
}

#[derive(Debug, Serialize)]
struct BuildZip {
    name: String,
    sha256: String,
    bytes: u64,
}

/// The files the build wrote into `--out`, and its work directory.
#[derive(Debug, Serialize)]
struct BuildFiles {
    zip: String,
    sums: String,
    build: String,
    work: String,
}

/// The answer: `build.json`'s document, where it was written and the steps.
#[derive(Serialize)]
struct BuildAnswer<'a> {
    build: &'a BuildDocument,
    out: String,
    files: BuildFiles,
    steps: &'a [StepReport],
}

/// The details of steps that did not all pass: the refusal's outcome and the steps' report.
#[derive(Serialize)]
struct BuildFailure<'a> {
    outcome: OutcomeEnvelope,
    report: &'a Report,
}

/// What the build reads of the zip's `interfaces.json`.
#[derive(Deserialize)]
struct Declared {
    speaks: DeclaredSpeaks,
}

#[derive(Deserialize)]
struct DeclaredSpeaks {
    package: [u64; 2],
}

/// `standard-pack build` (contracts/resource-verify.md, "`standard-pack build`").
pub(crate) fn run_standard_pack_build(
    global: &GlobalOptions,
    args: &[String],
) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let request = parse_request(global, &flags)?;
    crate::dry_run_gate::refuse(global, "standard-pack build", None, Value::Null)?;
    check_out(&request)?;
    let (report, top) = run_steps(&request.steps, std::env::current_exe(), StepsFor::Build);
    if !top.is(ActinglabCode::ResourceVerifyPassed) {
        return Err(steps_failed(&report, &top, &request.out));
    }
    let (document, files) =
        write_product(&request, &report).map_err(|outcome| write_refused(&outcome))?;
    serde_json::to_value(BuildAnswer {
        build: &document,
        out: request.out.display().to_string(),
        files,
        steps: &report.steps,
    })
    .map_err(|error| {
        CliError::device(format!(
            "failed to serialize the standard-pack build answer: {error}"
        ))
    })
}

/// The usage checks, before anything is read: the global options that name a device, a Runtime
/// or an instance, then the four flags and their values, as `resource verify` checks its own.
fn parse_request(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<BuildRequest> {
    if global.run_root.is_some()
        || global.instance.is_some()
        || global.instances.is_some()
        || global.profile.is_some()
        || global.resource_root.is_some()
        || global.game.is_some()
        || global.server.is_some()
        || global.runtime_endpoint.is_some()
        || global.capture_backend.is_some()
        || global.touch_backend.is_some()
    {
        return Err(CliError::new(
            ErrorKind::UsageValidation,
            "offline_device_scope_forbidden",
            "standard-pack build accepts no global Runtime, device, instance, selector, backend, \
             resource or run-root flag",
            &[],
        ));
    }
    if let Some(positional) = flags.positionals.first() {
        return Err(CliError::usage(format!(
            "standard-pack build takes no positional argument: {positional}"
        )));
    }
    if let Some(name) = flags
        .flags
        .keys()
        .find(|name| !BUILD_FLAGS.contains(&name.as_str()))
    {
        return Err(CliError::usage(format!(
            "standard-pack build does not accept {name}"
        )));
    }
    if let Some((name, _)) = flags.flags.iter().find(|(_, values)| values.len() > 1) {
        return Err(CliError::usage(format!(
            "standard-pack build takes {name} once"
        )));
    }
    let absolute = |name: &str| -> CliOutcome<PathBuf> {
        let given = flags.required_path(name)?;
        std::path::absolute(&given)
            .map_err(|error| CliError::usage(format!("{name} {}: {error}", given.display())))
    };
    let source_repository = flags.required("--source-repository")?;
    if !owner_name(&source_repository) {
        return Err(CliError::usage(format!(
            "--source-repository {source_repository} is not <owner>/<name>"
        )));
    }
    let commit = if flags.flags.contains_key("--commit") {
        Some(flags.required("--commit")?)
    } else {
        None
    };
    if let Some(commit) = &commit
        && !is_commit(commit)
    {
        return Err(CliError::usage(format!(
            "--commit {commit} is not 40 lowercase hexadecimal characters"
        )));
    }
    let out = absolute("--out")?;
    if out.file_name().is_none() {
        return Err(CliError::usage("--out names no directory"));
    }
    Ok(BuildRequest {
        steps: StepsRequest {
            repo: absolute("--repo")?,
            source_repository,
            work: out.join(WORK_DIRECTORY),
            commit,
            base: None,
        },
        out,
    })
}

/// `--out` is a directory that does not exist, whose parent does, or an existing empty directory;
/// never inside `--repo`. Anything else is `resource_build_output_invalid`, before anything is
/// read. Fix round 1: nothing is made here; the bundle step makes a `--out` that does not exist,
/// once the checkout precondition passed (`bundle.rs`).
fn check_out(request: &BuildRequest) -> CliOutcome<()> {
    let out = &request.out;
    let refused = |message: String| {
        let outcome = Outcome::new(ActinglabCode::ResourceBuildOutputInvalid).path(path_value(out));
        outcome_error(&outcome, message)
    };
    if inside(&request.steps.repo, out) {
        return Err(refused(format!(
            "{} lies inside the repository {}; standard-pack build never writes into the \
             repository",
            out.display(),
            request.steps.repo.display()
        )));
    }
    match fs::symlink_metadata(out) {
        Ok(metadata) => {
            let empty = metadata.is_dir()
                && fs::read_dir(out).is_ok_and(|mut entries| entries.next().is_none());
            if empty {
                Ok(())
            } else {
                Err(refused(format!(
                    "{} exists and is no empty directory; standard-pack build writes only into a \
                     new or an empty directory",
                    out.display()
                )))
            }
        }
        Err(_) => {
            let parent = out
                .parent()
                .is_some_and(|parent| fs::metadata(parent).is_ok_and(|metadata| metadata.is_dir()));
            if !parent {
                return Err(refused(format!(
                    "the parent of {} is no directory",
                    out.display()
                )));
            }
            Ok(())
        }
    }
}

/// Whether `out` is `repo` or lies inside it, as their real paths read (`out` need not exist).
fn inside(repo: &Path, out: &Path) -> bool {
    let (Ok(repo), Some(parent), Some(name)) =
        (fs::canonicalize(repo), out.parent(), out.file_name())
    else {
        return false;
    };
    fs::canonicalize(parent).is_ok_and(|parent| parent.join(name).starts_with(&repo))
}

/// Steps that did not all pass: `resource_build_failed` (`failed_step_count`), caused by the
/// steps' outcome; its details that outcome and the steps' report.
fn steps_failed(report: &Report, top: &Outcome, out: &Path) -> CliError {
    let outcome = Outcome::new(ActinglabCode::ResourceBuildFailed)
        .failed_step_count(report.failed_steps.len())
        .caused_by(top.clone());
    let details = serde_json::to_value(BuildFailure {
        outcome: outcome.envelope(Detail::Full),
        report,
    })
    .unwrap_or(Value::Null);
    CliError::new(
        ErrorKind::UsageValidation,
        outcome.code().as_str(),
        format!(
            "standard-pack build did not build ({top}); nothing was written into {} but the \
             steps' work; the details are the steps' report",
            out.display()
        ),
        &[],
    )
    .with_details(details)
}

/// A file of the product that could not be written: `resource_build_write_failed`, its
/// details the outcome.
fn write_refused(outcome: &Outcome) -> CliError {
    let message = format!("standard-pack build could not write its product ({outcome})");
    outcome_error(outcome, message)
}

/// The product, once every step passed: the steps' zip copied into `--out`, `SHA256SUMS` and
/// `build.json`; the document and where each file is.
fn write_product(
    request: &BuildRequest,
    report: &Report,
) -> Result<(BuildDocument, BuildFiles), Outcome> {
    let zip = steps_zip(report).ok_or_else(|| {
        Outcome::new(ActinglabCode::ResourceBuildWriteFailed).path(path_value(&request.out))
    })?;
    let made = request.steps.work.join(ZIP_DIRECTORY).join(&zip.name);
    let bytes = fs::read(&made).map_err(|error| write_failed(&made, &error, IoOp::Read))?;
    if sha256_hex(&bytes) != zip.sha256 {
        // The zip the steps read back is not the one on disk any more.
        return Err(Outcome::new(ActinglabCode::ResourceBuildWriteFailed).path(path_value(&made)));
    }
    // Fix round 1: the three files are written under their `.part` names first and renamed only
    // when all three are complete, `build.json` last, so a failure while they are written leaves
    // no file under a final name, and `build.json` is there only beside the other two.
    let zip_path = request.out.join(&zip.name);
    let sums_path = request.out.join(SUMS_FILE);
    let build_path = request.out.join(BUILD_FILE);
    let document = document(request, report, &zip, &zip_path, &bytes)?;
    let mut encoded = serde_json::to_vec_pretty(&document).map_err(|error| {
        eprintln!("ERROR actinglab: build.json does not serialize: {error}");
        Outcome::new(ActinglabCode::ResourceBuildWriteFailed).path(path_value(&build_path))
    })?;
    encoded.push(b'\n');
    let sums = format!("{}  {}\n", zip.sha256, zip.name);
    let product = [
        (&zip_path, bytes.as_slice()),
        (&sums_path, sums.as_bytes()),
        (&build_path, encoded.as_slice()),
    ];
    for (path, content) in product {
        write_new(&part_path(path), content)?;
    }
    for (path, _) in product {
        rename_new(&part_path(path), path)?;
    }
    let files = BuildFiles {
        zip: zip_path.display().to_string(),
        sums: sums_path.display().to_string(),
        build: build_path.display().to_string(),
        work: request.steps.work.display().to_string(),
    };
    Ok((document, files))
}

/// The zip the bundle step wrote, as its counts name it.
fn steps_zip(report: &Report) -> Option<ZipCounts> {
    report
        .steps
        .iter()
        .find(|step| step.step == VerifyStep::Bundle)
        .and_then(|step| match &step.counts {
            Counts::Bundle { zip, .. } => zip.clone(),
            _ => None,
        })
}

/// `build.json`'s document: the request's source repository, the report's commit, tree and
/// executable SHA-256, and what the zip `bytes` at `zip_path` hold.
fn document(
    request: &BuildRequest,
    report: &Report,
    zip: &ZipCounts,
    zip_path: &Path,
    bytes: &[u8],
) -> Result<BuildDocument, Outcome> {
    let unreadable =
        || Outcome::new(ActinglabCode::ResourceBuildWriteFailed).path(path_value(zip_path));
    let entries = zip_entries(bytes).ok_or_else(unreadable)?;
    let index = entries
        .get(STANDARD_PACK_BUNDLE_FILE)
        .and_then(|content| serde_json::from_slice::<BundleIndex>(content).ok())
        .ok_or_else(unreadable)?;
    let declared = entries
        .get(INTERFACES_FILE)
        .and_then(|content| serde_json::from_slice::<Declared>(content).ok())
        .ok_or_else(unreadable)?;
    let mut root_files = Vec::new();
    let mut root_directories = BTreeSet::new();
    for name in entries.keys() {
        match name.split_once('/') {
            None => root_files.push(name.clone()),
            Some((directory, _)) if directory != PACKS_DIRECTORY => {
                root_directories.insert(directory.to_owned());
            }
            Some(_) => {}
        }
    }
    let (Some(commit), Some(tree), Some(executable_sha256)) = (
        report.repository.commit.clone(),
        report.repository.tree.clone(),
        report.tools.executable_sha256(),
    ) else {
        return Err(unreadable());
    };
    Ok(BuildDocument {
        schema_version: BUILD_SCHEMA,
        game: index.game,
        source_repository: request.steps.source_repository.clone(),
        commit,
        tree,
        tools: BuildTools {
            runtime_head: env!("ACTINGCOMMAND_RUNTIME_HEAD"),
            executable_sha256: executable_sha256.to_owned(),
        },
        package: declared.speaks.package,
        root_files,
        root_directories: root_directories.into_iter().collect(),
        packs: index
            .packs
            .into_iter()
            .map(|pack| BuildPack {
                package_id: pack.package_id,
                server: pack.server,
                entry_task_id: pack.entry_task_id,
                file_count: pack.file_count,
                byte_count: pack.byte_count,
            })
            .collect(),
        zip: BuildZip {
            name: zip.name.clone(),
            sha256: zip.sha256.clone(),
            bytes: bytes.len() as u64,
        },
    })
}

/// Every entry of the zip `bytes`, by name; `None` when it does not read.
fn zip_entries(bytes: &[u8]) -> Option<BTreeMap<String, Vec<u8>>> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
    let mut entries = BTreeMap::new();
    for position in 0..archive.len() {
        let mut entry = archive.by_index(position).ok()?;
        let mut content = Vec::new();
        entry.read_to_end(&mut content).ok()?;
        entries.insert(entry.name().to_owned(), content);
    }
    Some(entries)
}

/// `resource_build_write_failed` naming `path`, caused by the OS failure.
fn write_failed(path: &Path, error: &io::Error, op: IoOp) -> Outcome {
    Outcome::new(ActinglabCode::ResourceBuildWriteFailed)
        .path(path_value(path))
        .caused_by(Outcome::from_io_error(error, op).path(path_value(path)))
}

/// Fix round 1: renames the complete file `part` to `path`, which must not exist: an existing
/// file is never replaced.
fn rename_new(part: &Path, path: &Path) -> Result<(), Outcome> {
    if fs::symlink_metadata(path).is_ok() {
        let exists = io::Error::from(io::ErrorKind::AlreadyExists);
        return Err(write_failed(path, &exists, IoOp::Rename));
    }
    fs::rename(part, path).map_err(|error| write_failed(path, &error, IoOp::Rename))
}

/// Writes the new file `path`; an existing one is never overwritten.
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), Outcome> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_all()
        })
        .map_err(|error| write_failed(path, &error, IoOp::Write))
}
