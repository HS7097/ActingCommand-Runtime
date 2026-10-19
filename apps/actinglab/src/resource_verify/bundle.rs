// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #411 RV-1: the `bundle` step (R9), the checks of the resource repositories'
//! `scripts/bundle.ps1`, in this process, under `--work`:
//!
//! 1. the pack directories staged in `packs-root/`, and `package bundle` with
//!    `applications.json`, `maintenance.json`, the source (`--source-repository` at HEAD) and the
//!    root files the repository holds, into `bundle/`;
//! 2. the index and copy checks of the written bundle;
//! 3. `interfaces.json` (`package` [3, 3], validated with this build's head), beside
//!    `bundle.json` in the zip only;
//! 4. `out/<game>-resources-<sha7>.zip`, entries in ordinal order, and `out/SHA256SUMS`;
//! 5. the zip read back into `read-back/<game>/`;
//! 6. the installed root `installed/<game>/` (the root files beside `bundle.json`, each
//!    `packs/<id>/` as `<id>/`), checked by `standard-pack check`;
//! 7. every read-back pack admitted again.
//!
//! Nothing is deleted; a failed `package bundle` leaves its `bundle.part`.

use super::steps::{
    Applications, PackIdentity, admission_failure, declaration_invalid, developer_line,
    foreign_json, preflight, relayed,
};
use super::{Counts, StepRun, ZipCounts, path_value, sha256_hex};
use crate::CliError;
use crate::codes::{ActinglabCode, BundleProblem, LayoutProblem, ReadbackProblem, VerifyStep};
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{IoOp, Outcome, RawSource};
use actingcommand_contract::{BundleIndex, BundleMaintenance, BundleSource};
use actingcommand_resource_tooling::{
    PackageBundleRequest, ResourceToolingCode, StandardPackCheckRequest, StandardPackRootFiles,
    check_standard_pack_root, package_bundle,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

const RUNTIME_REPOSITORY: &str = "HS7097/ActingCommand-Runtime";
const INTERFACES_SCHEMA: &str = "actingcommand.component-interfaces.v1";
const INTERFACES_FILE: &str = "interfaces.json";
/// The `package` revision the zip's `interfaces.json` declares, as `bundle.ps1` writes it.
const PACKAGE_REVISION: u32 = 3;
/// The standard-pack root files `package bundle` lays beside `bundle.json`.
const ROOT_FILES: [&str; 3] = ["pages.json", "interrupts.json", "navigation.json"];
const BYTE_ORDER_MARK: &[u8] = b"\xEF\xBB\xBF";

/// What the bundle step reads.
pub(super) struct BundleInputs<'a> {
    pub(super) repo: &'a Path,
    pub(super) work: &'a Path,
    pub(super) source_repository: &'a str,
    pub(super) commit: &'a str,
    pub(super) applications: Option<&'a Applications>,
    /// The pack directories of `packs/`, in name order.
    pub(super) packs: &'a [String],
    /// The identities of the packs the admission step admitted, by directory name.
    pub(super) identities: &'a BTreeMap<String, PackIdentity>,
}

/// The zip's `interfaces.json`.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Interfaces {
    schema_version: String,
    speaks: Speaks,
    validated_with: ValidatedWith,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Speaks {
    package: [u32; 2],
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ValidatedWith {
    repository: String,
    commit: String,
}

/// What the step counts as it goes.
#[derive(Default)]
struct BundleTally {
    packs: usize,
    files: u64,
    bytes: u64,
    zip: Option<ZipCounts>,
}

pub(super) fn bundle(inputs: &BundleInputs<'_>) -> StepRun {
    let mut run = StepRun::start(VerifyStep::Bundle);
    let root_files = ROOT_FILES
        .into_iter()
        .filter(|name| is_file(&inputs.repo.join(name)))
        .collect::<Vec<_>>();
    let mut tally = BundleTally::default();
    if let Err(outcome) = lay_out(inputs, &root_files, &mut run, &mut tally) {
        run.fail(outcome);
    }
    run.counts = Counts::Bundle {
        packs: tally.packs,
        files: tally.files,
        bytes: tally.bytes,
        zip: tally.zip,
        root_files: root_files.iter().map(|name| (*name).to_owned()).collect(),
        package_revision: PACKAGE_REVISION,
    };
    run.finish()
}

/// The step's work. A failure it cannot go on after is the error; the others are recorded as
/// they are found.
fn lay_out(
    inputs: &BundleInputs<'_>,
    root_files: &[&'static str],
    run: &mut StepRun,
    tally: &mut BundleTally,
) -> Result<(), Outcome> {
    let work = inputs.work;
    create_dir(work)?;
    let maintenance = read_maintenance(inputs.repo)?;
    let stage = work.join("packs-root");
    create_dir(&stage)?;
    for name in inputs.packs {
        copy_tree(&inputs.repo.join("packs").join(name), &stage.join(name))?;
    }
    let bundle_dir = work.join("bundle");
    let root_file = |name: &str| root_files.contains(&name).then(|| inputs.repo.join(name));
    let response = package_bundle(PackageBundleRequest {
        applications: inputs.repo.join("applications.json"),
        packs_root: stage,
        out: bundle_dir.clone(),
        source: Some(BundleSource {
            repository: inputs.source_repository.to_owned(),
            commit: inputs.commit.to_owned(),
        }),
        maintenance: Some(maintenance.clone()),
        root_files: StandardPackRootFiles {
            pages: root_file("pages.json"),
            interrupts: root_file("interrupts.json"),
            navigation: root_file("navigation.json"),
        },
    })
    .map_err(|error| lab_failure(&error))?;
    for warning in response.warnings {
        run.warn(warning);
    }
    let index = check_index(
        inputs,
        root_files,
        &maintenance,
        &response.index,
        &bundle_dir,
        run,
    )?;
    tally.packs = index.packs.len();
    tally.files = index.packs.iter().map(|pack| pack.file_count).sum();
    tally.bytes = index.packs.iter().map(|pack| pack.byte_count).sum();

    // The zip: the bundle directory and interfaces.json, entries in ordinal order.
    let game = index.game.clone();
    if !one_segment(&game) {
        return Err(mismatch(
            "applications.json",
            BundleProblem::ApplicationsDiffers,
        ));
    }
    let mut entries = BTreeMap::<String, Vec<u8>>::new();
    for relative in list_files(&bundle_dir)
        .map_err(|error| read_failed(&bundle_dir, &error, BundleProblem::RootFileDiffers))?
    {
        let path = join_relative(&bundle_dir, &relative);
        let bytes = fs::read(&path)
            .map_err(|error| read_failed(&path, &error, BundleProblem::RootFileDiffers))?;
        entries.insert(relative, bytes);
    }
    if entries.contains_key(INTERFACES_FILE) {
        // The bundle root check named it already; the zip cannot hold a second one.
        return Ok(());
    }
    let interfaces = Interfaces {
        schema_version: INTERFACES_SCHEMA.to_owned(),
        speaks: Speaks {
            package: [PACKAGE_REVISION, PACKAGE_REVISION],
        },
        validated_with: ValidatedWith {
            repository: RUNTIME_REPOSITORY.to_owned(),
            commit: env!("ACTINGCOMMAND_RUNTIME_HEAD").to_owned(),
        },
    };
    let mut interfaces_bytes = serde_json::to_vec_pretty(&interfaces)
        .map_err(|error| write_failed_with(foreign_json(&error.to_string())))?;
    interfaces_bytes.push(b'\n');
    entries.insert(INTERFACES_FILE.to_owned(), interfaces_bytes);
    let out = work.join("out");
    create_dir(&out)?;
    let short = inputs.commit.get(..7).unwrap_or(inputs.commit);
    let zip_name = format!("{game}-resources-{short}.zip");
    let zip_path = out.join(&zip_name);
    write_zip(&zip_path, &entries)?;
    let zip_bytes =
        fs::read(&zip_path).map_err(|error| readback_io(&zip_name, &zip_path, &error))?;
    let zip_sha256 = sha256_hex(&zip_bytes);
    write_new(
        &out.join("SHA256SUMS"),
        format!("{zip_sha256}  {zip_name}\n").as_bytes(),
    )?;
    tally.zip = Some(ZipCounts {
        name: zip_name.clone(),
        sha256: zip_sha256,
        entries: entries.len(),
    });

    // Read back into read-back/<game>/: the same entries and bytes, and interfaces.json.
    let read = read_zip(&zip_name, &zip_path)?;
    let written = entries.keys().collect::<BTreeSet<_>>();
    let readable = read.keys().collect::<BTreeSet<_>>();
    for name in written.symmetric_difference(&readable) {
        run.fail(readback(name, ReadbackProblem::EntrySetDiffers));
    }
    for (name, bytes) in &read {
        if entries.get(name).is_some_and(|expected| expected != bytes) {
            run.fail(readback(name, ReadbackProblem::BytesDiffer));
        }
    }
    let read_interfaces = read
        .get(INTERFACES_FILE)
        .and_then(|bytes| serde_json::from_slice::<Interfaces>(bytes).ok());
    if read_interfaces.as_ref() != Some(&interfaces) {
        run.fail(readback(
            INTERFACES_FILE,
            ReadbackProblem::InterfacesDiffers,
        ));
    }
    let read_back_parent = work.join("read-back");
    create_dir(&read_back_parent)?;
    let read_back = read_back_parent.join(&game);
    create_dir(&read_back)?;
    for (name, bytes) in &read {
        if !safe_entry(name) {
            run.fail(readback(name, ReadbackProblem::EntrySetDiffers));
            continue;
        }
        let target = join_relative(&read_back, name);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| write_failed(parent, &error, IoOp::CreateDir))?;
        }
        write_new(&target, bytes)?;
    }

    // The installed root installed/<game>/, as the Runtime's registry reads it.
    let installed_parent = work.join("installed");
    create_dir(&installed_parent)?;
    let installed = installed_parent.join(&game);
    create_dir(&installed)?;
    for (name, bytes) in &read {
        if safe_entry(name) && !name.contains('/') {
            write_new(&installed.join(name), bytes)?;
        }
    }
    for pack in &index.packs {
        if safe_entry(&pack.path) && one_segment(&pack.package_id) {
            copy_tree(
                &join_relative(&read_back, &pack.path),
                &installed.join(&pack.package_id),
            )?;
        }
    }
    match check_standard_pack_root(StandardPackCheckRequest {
        root: installed.clone(),
    }) {
        Err(error) => run.fail(lab_failure(&error)),
        Ok(checked) => {
            let read_root_files = checked.root_files.iter().copied().collect::<BTreeSet<_>>();
            let expected = std::iter::once("bundle.json")
                .chain(root_files.iter().copied())
                .collect::<BTreeSet<_>>();
            if read_root_files != expected
                || checked.pack_count != index.packs.len()
                || checked.game != game
            {
                run.fail(mismatch(
                    &installed.display().to_string(),
                    BundleProblem::RootFilesUnread,
                ));
            }
            for warning in checked.warnings {
                run.warn(warning);
            }
        }
    }

    // Every read-back pack admitted again, stating what the index states.
    for pack in &index.packs {
        if !safe_entry(&pack.path) {
            continue;
        }
        match preflight(&join_relative(&read_back, &pack.path)) {
            Err(error) => run.fail(admission_failure(VerifyStep::Bundle, &error, &pack.path)),
            Ok(prepared) => {
                if prepared.package_label() != pack.package_id.as_str()
                    || prepared.server() != pack.server.as_str()
                    || prepared.task_label() != pack.entry_task_id.as_str()
                {
                    run.fail(mismatch(&pack.path, BundleProblem::PackListDiffers));
                }
            }
        }
    }
    Ok(())
}

/// `maintenance.json`, committed beside `applications.json` (a BOM is accepted).
fn read_maintenance(repo: &Path) -> Result<Vec<BundleMaintenance>, Outcome> {
    let path = repo.join("maintenance.json");
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(Outcome::new(ActinglabCode::ResourceVerifyLayoutInvalid)
                .path("maintenance.json")
                .layout_problem(LayoutProblem::MaintenanceMissing));
        }
        Err(error) => {
            return Err(declaration_invalid(
                "maintenance.json",
                Outcome::from_io_error(&error, IoOp::Read).path(path_value(&path)),
            ));
        }
    };
    serde_json::from_slice(bytes.strip_prefix(BYTE_ORDER_MARK).unwrap_or(&bytes))
        .map_err(|error| declaration_invalid("maintenance.json", foreign_json(&error.to_string())))
}

/// The index and copy checks (`bundle.ps1:316-380`); every difference is recorded.
fn check_index(
    inputs: &BundleInputs<'_>,
    root_files: &[&'static str],
    maintenance: &[BundleMaintenance],
    answered: &BundleIndex,
    bundle_dir: &Path,
    run: &mut StepRun,
) -> Result<BundleIndex, Outcome> {
    let index_path = bundle_dir.join("bundle.json");
    let bytes = fs::read(&index_path)
        .map_err(|error| read_failed(&index_path, &error, BundleProblem::IndexSchema))?;
    let index = serde_json::from_slice::<BundleIndex>(&bytes).map_err(|error| {
        mismatch("bundle.json", BundleProblem::IndexSchema)
            .caused_by(foreign_json(&error.to_string()))
    })?;
    // The maintenance read back, from the answer and from bundle.json, equals the source's.
    if index.maintenance.as_deref() != Some(maintenance)
        || answered.maintenance.as_deref() != Some(maintenance)
    {
        run.fail(mismatch("bundle.json", BundleProblem::MaintenanceDiffers));
    }
    // The bundle root: applications.json, bundle.json, packs and the root files given.
    let mut expected = BTreeSet::from([
        "applications.json".to_owned(),
        "bundle.json".to_owned(),
        "packs".to_owned(),
    ]);
    expected.extend(root_files.iter().map(|name| (*name).to_owned()));
    let present = list_names(bundle_dir)
        .map_err(|error| read_failed(bundle_dir, &error, BundleProblem::RootFileDiffers))?;
    for name in present.symmetric_difference(&expected) {
        run.fail(mismatch(name, BundleProblem::RootFileDiffers));
    }
    for name in root_files {
        if !same_bytes(&bundle_dir.join(name), &inputs.repo.join(name)) {
            run.fail(mismatch(name, BundleProblem::RootFileDiffers));
        }
    }
    let game_differs = inputs
        .applications
        .is_some_and(|applications| applications.game != index.game);
    if game_differs
        || !same_bytes(
            &bundle_dir.join("applications.json"),
            &inputs.repo.join("applications.json"),
        )
    {
        run.fail(mismatch(
            "applications.json",
            BundleProblem::ApplicationsDiffers,
        ));
    }
    let source = BundleSource {
        repository: inputs.source_repository.to_owned(),
        commit: inputs.commit.to_owned(),
    };
    if index.source.as_ref() != Some(&source) {
        run.fail(mismatch("bundle.json", BundleProblem::SourceDiffers));
    }
    // The pack list is packs/, and each copy is its source, file for file and byte for byte.
    let sources = inputs
        .packs
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let indexed = index
        .packs
        .iter()
        .map(|pack| pack.package_id.as_str())
        .collect::<BTreeSet<_>>();
    for id in indexed.symmetric_difference(&sources) {
        run.fail(mismatch(
            &format!("packs/{id}"),
            BundleProblem::PackListDiffers,
        ));
    }
    if indexed.len() != index.packs.len() {
        run.fail(mismatch("bundle.json", BundleProblem::PackListDiffers));
    }
    let copies_dir = bundle_dir.join("packs");
    let copies = list_names(&copies_dir)
        .map_err(|error| read_failed(&copies_dir, &error, BundleProblem::PackListDiffers))?;
    for id in copies
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>()
        .symmetric_difference(&indexed)
    {
        run.fail(mismatch(
            &format!("packs/{id}"),
            BundleProblem::PackListDiffers,
        ));
    }
    for pack in &index.packs {
        if !sources.contains(pack.package_id.as_str()) || !safe_entry(&pack.path) {
            continue;
        }
        let source_name = format!("packs/{}", pack.package_id);
        let identity_differs = inputs
            .identities
            .get(&pack.package_id)
            .is_some_and(|identity| {
                identity.server != pack.server || identity.entry_task_id != pack.entry_task_id
            });
        if pack.path != source_name || identity_differs {
            run.fail(mismatch(&source_name, BundleProblem::PackListDiffers));
        }
        let source_dir = inputs.repo.join("packs").join(&pack.package_id);
        let copy_dir = join_relative(bundle_dir, &pack.path);
        let source_files = list_files(&source_dir)
            .map_err(|error| read_failed(&source_dir, &error, BundleProblem::PackCopyDiffers))?;
        let copy_files = list_files(&copy_dir)
            .map_err(|error| read_failed(&copy_dir, &error, BundleProblem::PackCopyDiffers))?;
        if source_files != copy_files {
            run.fail(mismatch(&source_name, BundleProblem::PackCopyDiffers));
            continue;
        }
        let mut byte_count = 0_u64;
        for relative in &copy_files {
            let copy = join_relative(&copy_dir, relative);
            let length = fs::metadata(&copy).map_or(0, |metadata| metadata.len());
            byte_count += length;
            if !same_bytes(&copy, &join_relative(&source_dir, relative)) {
                run.fail(mismatch(
                    &format!("{source_name}/{relative}"),
                    BundleProblem::PackCopyDiffers,
                ));
            }
        }
        if pack.file_count != copy_files.len() as u64 || pack.byte_count != byte_count {
            run.fail(mismatch(&source_name, BundleProblem::PackCountsDiffer));
        }
    }
    // Each server's default package names exactly one bundled pack of that server.
    if let Some(applications) = inputs.applications {
        for (server, package_id) in &applications.defaults {
            let matching = index
                .packs
                .iter()
                .filter(|pack| pack.package_id == *package_id && pack.server == *server)
                .count();
            if matching != 1 {
                run.fail(mismatch(
                    "applications.json",
                    BundleProblem::DefaultPackageUnmatched,
                ));
            }
        }
    }
    Ok(index)
}

fn read_zip(zip_name: &str, zip_path: &Path) -> Result<BTreeMap<String, Vec<u8>>, Outcome> {
    let unreadable =
        |cause: Outcome| readback(zip_name, ReadbackProblem::EntrySetDiffers).caused_by(cause);
    let file = fs::File::open(zip_path).map_err(|error| {
        unreadable(Outcome::from_io_error(&error, IoOp::Open).path(path_value(zip_path)))
    })?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|error| unreadable(foreign_zip(&error)))?;
    let mut read = BTreeMap::new();
    for position in 0..archive.len() {
        let mut entry = archive
            .by_index(position)
            .map_err(|error| unreadable(foreign_zip(&error)))?;
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).map_err(|error| {
            unreadable(Outcome::from_io_error(&error, IoOp::Read).path(path_value(zip_path)))
        })?;
        read.insert(entry.name().to_owned(), bytes);
    }
    Ok(read)
}

fn write_zip(path: &Path, entries: &BTreeMap<String, Vec<u8>>) -> Result<(), Outcome> {
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| write_failed(path, &error, IoOp::Create))?;
    let mut zip = zip::ZipWriter::new(file);
    let options =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, bytes) in entries {
        zip.start_file(name.as_str(), options)
            .map_err(|error| write_failed_with(foreign_zip(&error)))?;
        zip.write_all(bytes)
            .map_err(|error| write_failed(path, &error, IoOp::Write))?;
    }
    zip.finish()
        .map_err(|error| write_failed_with(foreign_zip(&error)))?;
    Ok(())
}

/// A failure of `package bundle` or `standard-pack check` as an outcome, caused by the
/// admission's code where the refusal carries one.
fn lab_failure(error: &CliError) -> Outcome {
    developer_line(VerifyStep::Bundle, error);
    let outcome = relayed(error);
    match error
        .details
        .as_ref()
        .and_then(|details| details.get("code"))
        .and_then(serde_json::Value::as_str)
    {
        Some(code) => outcome.caused_by(Outcome::relayed(code)),
        None => outcome,
    }
}

fn mismatch(path: &str, problem: BundleProblem) -> Outcome {
    Outcome::new(ActinglabCode::ResourceVerifyBundleMismatch)
        .path(path_value(path))
        .bundle_problem(problem)
}

fn readback(entry: &str, problem: ReadbackProblem) -> Outcome {
    Outcome::new(ActinglabCode::ResourceVerifyReadbackMismatch)
        .entry(path_value(entry))
        .readback_problem(problem)
}

fn readback_io(zip_name: &str, path: &Path, error: &io::Error) -> Outcome {
    readback(zip_name, ReadbackProblem::BytesDiffer)
        .caused_by(Outcome::from_io_error(error, IoOp::Read).path(path_value(path)))
}

fn read_failed(path: &Path, error: &io::Error, problem: BundleProblem) -> Outcome {
    mismatch(&path.display().to_string(), problem)
        .caused_by(Outcome::from_io_error(error, IoOp::Read).path(path_value(path)))
}

fn write_failed(path: &Path, error: &io::Error, op: IoOp) -> Outcome {
    write_failed_with(Outcome::from_io_error(error, op).path(path_value(path)))
}

/// `package_bundle_write_failed`: a file under `--work` could not be written.
fn write_failed_with(cause: Outcome) -> Outcome {
    Outcome::new(ResourceToolingCode::PackageBundleWriteFailed).caused_by(cause)
}

fn foreign_zip(error: &zip::result::ZipError) -> Outcome {
    Outcome::new(ContractCode::ForeignOtherError)
        .raw_source(RawSource::Zip)
        .raw_text(error.to_string())
}

fn create_dir(path: &Path) -> Result<(), Outcome> {
    fs::create_dir(path).map_err(|error| write_failed(path, &error, IoOp::CreateDir))
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), Outcome> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .and_then(|mut file| file.write_all(bytes))
        .map_err(|error| write_failed(path, &error, IoOp::Write))
}

/// Copies a directory tree into the new directory `to`, file by file.
fn copy_tree(from: &Path, to: &Path) -> Result<(), Outcome> {
    create_dir(to)?;
    let entries = fs::read_dir(from).map_err(|error| write_failed(from, &error, IoOp::ReadDir))?;
    for entry in entries {
        let entry = entry.map_err(|error| write_failed(from, &error, IoOp::ReadDir))?;
        let source = entry.path();
        let target = to.join(entry.file_name());
        let metadata =
            fs::metadata(&source).map_err(|error| write_failed(&source, &error, IoOp::Metadata))?;
        if metadata.is_dir() {
            copy_tree(&source, &target)?;
        } else {
            let bytes =
                fs::read(&source).map_err(|error| write_failed(&source, &error, IoOp::Read))?;
            write_new(&target, &bytes)?;
        }
    }
    Ok(())
}

/// Every file below `root`, as `/`-separated relative paths in ordinal order.
fn list_files(root: &Path) -> io::Result<Vec<String>> {
    let mut files = Vec::new();
    let mut directories = vec![(root.to_path_buf(), String::new())];
    while let Some((directory, prefix)) = directories.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let relative = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if fs::metadata(entry.path())?.is_dir() {
                directories.push((entry.path(), relative));
            } else {
                files.push(relative);
            }
        }
    }
    files.sort();
    Ok(files)
}

/// The names of the entries of a directory.
fn list_names(directory: &Path) -> io::Result<BTreeSet<String>> {
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(directory)? {
        names.insert(entry?.file_name().to_string_lossy().into_owned());
    }
    Ok(names)
}

fn join_relative(root: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(root.to_path_buf(), |path, segment| path.join(segment))
}

fn is_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file())
}

fn same_bytes(left: &Path, right: &Path) -> bool {
    matches!((fs::read(left), fs::read(right)), (Ok(left), Ok(right)) if left == right)
}

/// One name a directory can hold: no separator, drive, parent or empty name.
fn one_segment(text: &str) -> bool {
    !text.is_empty()
        && text != "."
        && text != ".."
        && !text.contains(['/', '\\', ':'])
        && !text.chars().any(char::is_control)
}

/// A zip entry name that stays below the directory it is extracted to.
fn safe_entry(name: &str) -> bool {
    name.split('/').all(one_segment)
}
