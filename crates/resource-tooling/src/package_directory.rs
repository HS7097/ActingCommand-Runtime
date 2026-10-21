// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #288 A2b, Workflow #336 PKG-S5 (option 丙): `package bundle` lays out the standard
//! package's resource section: one directory per task pack, named by the `package_id` its control
//! declares (`packs/<package_id>`), and its bundle index v4. Each pack is read through the
//! loader's own snapshot, copied, and its copy admitted in full by its path; no digest of a pack
//! is computed. Workflow #373 NAV-T (M-373 v1.1 §3.1, §3.9): the standard-pack root files given
//! (`pages.json`, `interrupts.json`, `navigation.json`) are checked with the game root the
//! bundle makes, its packages cross-checked for the parts the files name them for
//! (`standard_pack_check`), and laid beside `bundle.json` byte for byte.

use crate::codes::ResourceToolingCode;
use crate::standard_pack_check::{RootFileInputs, cross_check};
use crate::{PackageBundleRequest, PackageBundleResponse};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    BundleIndex, BundleIndexVersion, BundleMaintenance, BundlePack, LabError, LabErrorClass,
    LabResult, MaintenanceUse, package_id_names_one_path_segment, reserved_package_id,
};
use actingcommand_execution_kernel::{NavigationPackageDescriptor, TaskPackageDescriptor};
use actingcommand_execution_kernel::{PreparedContainedTask, validate_bundle_maintenance};
use actingcommand_pack_containment::{Containment, ContainmentError, InstanceId};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const APPLICATIONS_SCHEMA: &str = "actingcommand.applications.v1";
/// The read and the admission of one pack directory share one deadline.
pub(crate) const PACK_DEADLINE: Duration = Duration::from_secs(60);

/// The identity a pack's control declaration states, read from its admitted bundle.
struct PackIdentity {
    game: String,
    package_id: String,
    server: String,
    entry_task_id: String,
}

/// Writes `<out>.part` and renames it to `out` only once every pack was copied under its
/// `package_id`, admitted and indexed; a failure leaves `<out>.part` in place and says so.
pub fn package_bundle(request: PackageBundleRequest) -> LabResult<PackageBundleResponse> {
    let out = absolute(&request.out)?;
    let name = out
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| LabError::usage(format!("--out {} names no directory", out.display())))?;
    let staging = out.with_file_name(format!("{name}.part"));
    for path in [&out, &staging] {
        if fs::symlink_metadata(path).is_ok() {
            return Err(bundle_error(
                ResourceToolingCode::PackageBundleOutExists,
                format!(
                    "{} already exists; package bundle only writes a new directory",
                    path.display()
                ),
            ));
        }
    }
    let applications_bytes = fs::read(&request.applications).map_err(|error| {
        bundle_error(
            ResourceToolingCode::PackageBundleApplicationsInvalid,
            format!("{}: {error}", request.applications.display()),
        )
    })?;
    let applications = Applications::parse(&applications_bytes)?;
    let root_files = RootFileInputs::read(&request.root_files)?;
    let directories = pack_directories(&absolute(&request.packs_root)?)?;
    fs::create_dir(&staging).map_err(|error| write_error(&staging, error))?;
    write_bundle(
        &staging,
        &applications,
        &applications_bytes,
        &directories,
        request.source,
        request.maintenance,
        &root_files,
    )
    .and_then(|(index, warnings)| {
        fs::rename(&staging, &out).map_err(|error| write_error(&out, error))?;
        Ok(PackageBundleResponse {
            status: "written".to_string(),
            out: out.display().to_string(),
            index,
            warnings,
        })
    })
    .map_err(|mut error| {
        error.message = format!(
            "{}; the incomplete output remains at {}",
            error.message,
            staging.display()
        );
        error
    })
}

fn write_bundle(
    staging: &Path,
    applications: &Applications,
    applications_bytes: &[u8],
    directories: &[PathBuf],
    source: Option<actingcommand_contract::BundleSource>,
    maintenance: Option<Vec<BundleMaintenance>>,
    root_files: &RootFileInputs,
) -> LabResult<(BundleIndex, Vec<Outcome>)> {
    let packs_dir = staging.join("packs");
    fs::create_dir(&packs_dir).map_err(|error| write_error(&packs_dir, error))?;
    let mut containment = Containment::for_metadata_validation();
    let mut packs = Vec::new();
    let mut descriptors = Vec::new();
    // Workflow #373 NAV-T: with root files every pack is admitted for real, once, for both the
    // maintenance check and the cross-check.
    let describe = maintenance.is_some() || !root_files.is_empty();
    let mut admitted = BTreeMap::<String, NavigationPackageDescriptor>::new();
    let mut named = BTreeMap::<String, PathBuf>::new();
    for directory in directories {
        let entries = containment
            .snapshot_content_directory(directory, Instant::now() + PACK_DEADLINE)
            .map_err(|error| pack_error(directory, error))?;
        // Only the control declaration and the resource tree enter the standard package.
        let files = entries
            .into_iter()
            .filter(|(path, _)| path == "control.json" || path.starts_with("resources/"))
            .collect::<BTreeMap<_, _>>();
        // The pack's directory is named by the `package_id` its control declares (bundle v4).
        let package_id = declared_package_id(directory, &files)?;
        if let Some(first) = named.insert(package_id.clone(), directory.clone()) {
            return Err(bundle_error(
                ResourceToolingCode::PackageBundleIndexInvalid,
                format!(
                    "{} and {} both declare package_id {package_id:?}; a bundle holds one pack per \
                     package_id",
                    first.display(),
                    directory.display()
                ),
            ));
        }
        let target = packs_dir.join(&package_id);
        fs::create_dir(&target).map_err(|error| write_error(&target, error))?;
        for (path, bytes) in &files {
            let file = path
                .split('/')
                .fold(target.clone(), |parent, part| parent.join(part));
            if let Some(parent) = file.parent() {
                fs::create_dir_all(parent).map_err(|error| write_error(parent, error))?;
            }
            write_new(&file, bytes)?;
        }
        // The copy is read again and admitted in full by its path.
        let identity = if describe {
            let package = PreparedContainedTask::describe_navigation_path(
                "package-bundle",
                &target,
                Instant::now() + PACK_DEADLINE,
            )
            .map_err(|error| LabError::package_invalid(format!("{}: {error}", target.display())))?;
            let descriptor = package.descriptor().clone();
            let identity = PackIdentity {
                game: descriptor.game().to_owned(),
                package_id: descriptor.package_id().to_owned(),
                server: descriptor.server().to_owned(),
                entry_task_id: descriptor.entry_task_id().to_owned(),
            };
            admitted.insert(package_id.clone(), package);
            descriptors.push(descriptor);
            identity
        } else {
            admit(&mut containment, &target, Instant::now() + PACK_DEADLINE)?
        };
        if identity.package_id != package_id {
            return Err(bundle_error(
                ResourceToolingCode::PackageBundleIndexInvalid,
                format!(
                    "{}: the admitted pack names package_id {:?}, not {package_id:?}",
                    directory.display(),
                    identity.package_id
                ),
            ));
        }
        if identity.game != applications.game || !applications.servers.contains(&identity.server) {
            return Err(bundle_error(
                ResourceToolingCode::PackageBundlePackMismatch,
                format!(
                    "{}: pack {} declares game {:?} and server {:?}; the applications table has game {:?} and servers {:?}",
                    directory.display(),
                    identity.package_id,
                    identity.game,
                    identity.server,
                    applications.game,
                    applications.servers
                ),
            ));
        }
        packs.push(BundlePack {
            path: format!("packs/{}", identity.package_id),
            package_id: identity.package_id,
            server: identity.server,
            entry_task_id: identity.entry_task_id,
            file_count: files.len() as u64,
            byte_count: total_bytes(files.values()),
        });
    }
    packs.sort_by(|left, right| left.package_id.cmp(&right.package_id));
    for (server, package_id) in &applications.defaults {
        if !packs
            .iter()
            .any(|pack| &pack.package_id == package_id && &pack.server == server)
        {
            return Err(bundle_error(
                ResourceToolingCode::PackageBundleDefaultPackageMissing,
                format!(
                    "servers.{server}.default_package_id {package_id:?} names no pack of server {server:?}"
                ),
            ));
        }
    }
    let mut warnings = maintenance
        .as_deref()
        .map(|maintenance| undeclared_recognition_packs(maintenance, &descriptors))
        .unwrap_or_default();
    let qualify = maintenance.is_some();
    let index = BundleIndex {
        schema_version: BundleIndexVersion::V4,
        game: applications.game.clone(),
        source,
        packs,
        maintenance,
        tables: None,
    };
    if qualify {
        validate_bundle_maintenance(&index, &descriptors).map_err(|error| {
            bundle_error(
                ResourceToolingCode::PackageBundleMaintenanceInvalid,
                error.to_string(),
            )
        })?;
    }
    index.validate().map_err(|error| {
        bundle_error(
            ResourceToolingCode::PackageBundleIndexInvalid,
            error.to_string(),
        )
    })?;
    let mut encoded = serde_json::to_vec_pretty(&index).map_err(|error| {
        bundle_error(
            ResourceToolingCode::PackageBundleIndexInvalid,
            error.to_string(),
        )
    })?;
    encoded.push(b'\n');
    let bundle_path = staging.join("bundle.json");
    if !root_files.is_empty() {
        // Workflow #373 NAV-T: the game root this bundle makes, checked as every reader checks
        // it, and its packages cross-checked for their parts; the report joins the warnings.
        let root = root_files.root(&applications.game, &bundle_path, &encoded)?;
        warnings.extend(cross_check(
            &applications.game,
            &root,
            &|directory: &str| packs_dir.join(directory),
            admitted,
        )?);
    }
    write_new(&staging.join("applications.json"), applications_bytes)?;
    write_new(&bundle_path, &encoded)?;
    for (name, file) in root_files.files() {
        write_new(&staging.join(name), &file.bytes)?;
    }
    Ok((index, warnings))
}

/// The `package_id` a pack source's `control.json` declares, which names the pack's directory
/// (`packs/<package_id>`): one path segment, and never a name the game root reserves (its
/// `tables` directory, Workflow #357 M-357a §2.1; Workflow #373 NAV-T, M-373 §3.1: a root file's
/// `<kind>.json`, `<name>.part`, `<name>.old-<unix_ms>`). The full admission of the copy checks
/// the rest.
fn declared_package_id(directory: &Path, files: &BTreeMap<String, Vec<u8>>) -> LabResult<String> {
    let package_id = files
        .get("control.json")
        .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
        .and_then(|control| control.get("package_id")?.as_str().map(str::to_owned))
        .ok_or_else(|| {
            LabError::package_invalid(format!(
                "{}: control.json declares no string package_id",
                directory.display()
            ))
        })?;
    if !package_id_names_one_path_segment(&package_id) || reserved_package_id(&package_id) {
        return Err(bundle_error(
            ResourceToolingCode::PackageBundleIndexInvalid,
            format!(
                "{}: package_id {package_id:?} cannot name its directory packs/<package_id> (one \
                 path segment, not a name the game root reserves: tables, <kind>.json, \
                 <name>.part, <name>.old-<unix_ms>)",
                directory.display()
            ),
        ));
    }
    Ok(package_id)
}

/// Workflow #384 D2-S2 (D2 model §3.2): a pack that only navigates and reads (it qualifies as a
/// recognition maintenance package: marks, no designated operation, no stability termination,
/// not recognize-only) belongs to the standard package's one recognition maintenance package;
/// each such pack the declarations do not name for `recognition` is one
/// `recognition_pack_undeclared`, a warning while the resource repositories declare theirs.
/// Workflow #411 A32: a server (variant) declares at most one, and only the one it declares
/// counts: the other qualifying packs of a server that declares one raise nothing, and each
/// qualifying pack of a server that declares none still does.
pub(crate) fn undeclared_recognition_packs(
    maintenance: &[BundleMaintenance],
    descriptors: &[TaskPackageDescriptor],
) -> Vec<Outcome> {
    // E3 red proof 2 (do not merge): today's count, every qualifying pack not itself declared.
    let declared = maintenance
        .iter()
        .filter(|entry| entry.uses.contains(&MaintenanceUse::Recognition))
        .map(|entry| entry.package_id.as_str())
        .collect::<BTreeSet<_>>();
    descriptors
        .iter()
        .filter(|descriptor| {
            descriptor.recognition_problem().is_none()
                && !declared.contains(descriptor.package_id())
        })
        .map(|descriptor| {
            Outcome::new(ResourceToolingCode::RecognitionPackUndeclared)
                .package_id(descriptor.package_id())
        })
        .collect()
}

/// The parts of the applications table the bundle relies on; the table's full check is
/// `resource validate`.
struct Applications {
    game: String,
    servers: BTreeSet<String>,
    /// `servers.<server>.default_package_id`, where declared.
    defaults: BTreeMap<String, String>,
}

impl Applications {
    fn parse(bytes: &[u8]) -> LabResult<Self> {
        let invalid = |reason: String| {
            bundle_error(
                ResourceToolingCode::PackageBundleApplicationsInvalid,
                format!("applications table: {reason}"),
            )
        };
        let value: Value =
            serde_json::from_slice(bytes).map_err(|error| invalid(format!("not JSON: {error}")))?;
        if value.get("schema_version").and_then(Value::as_str) != Some(APPLICATIONS_SCHEMA) {
            return Err(invalid(format!(
                "schema_version must be {APPLICATIONS_SCHEMA}"
            )));
        }
        let game = value
            .get("game")
            .and_then(Value::as_str)
            .filter(|game| !game.is_empty())
            .ok_or_else(|| invalid("game must be a non-empty string".to_string()))?;
        let servers = value
            .get("servers")
            .and_then(Value::as_object)
            .filter(|servers| !servers.is_empty())
            .ok_or_else(|| invalid("servers must be a non-empty object".to_string()))?;
        let mut defaults = BTreeMap::new();
        for (server, entry) in servers {
            let entry = entry
                .as_object()
                .ok_or_else(|| invalid(format!("servers.{server} must be an object")))?;
            match entry.get("default_package_id") {
                None => {}
                Some(Value::String(package_id)) => {
                    defaults.insert(server.clone(), package_id.clone());
                }
                Some(_) => {
                    return Err(invalid(format!(
                        "servers.{server}.default_package_id must be a string"
                    )));
                }
            }
        }
        Ok(Self {
            game: game.to_string(),
            servers: servers.keys().cloned().collect(),
            defaults,
        })
    }
}

/// Every entry of `packs_root` is one pack source directory, in name order.
fn pack_directories(packs_root: &Path) -> LabResult<Vec<PathBuf>> {
    let invalid =
        |message: String| bundle_error(ResourceToolingCode::PackageBundlePacksRootInvalid, message);
    let mut directories = Vec::new();
    for entry in fs::read_dir(packs_root)
        .map_err(|error| invalid(format!("{}: {error}", packs_root.display())))?
    {
        let path = entry
            .map_err(|error| invalid(format!("{}: {error}", packs_root.display())))?
            .path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| invalid(format!("{}: {error}", path.display())))?;
        if !metadata.is_dir() {
            return Err(invalid(format!(
                "{} is not a pack directory",
                path.display()
            )));
        }
        directories.push(path);
    }
    if directories.is_empty() {
        return Err(invalid(format!(
            "{} holds no pack directory",
            packs_root.display()
        )));
    }
    directories.sort();
    Ok(directories)
}

/// Admits `locator` in full by its path and reads the identity its control states.
fn admit(
    containment: &mut Containment,
    locator: &Path,
    deadline: Instant,
) -> LabResult<PackIdentity> {
    let instance =
        InstanceId::new("package-directory").map_err(|error| pack_error(locator, error))?;
    let bundle = containment
        .load_path(&instance, locator, false, deadline)
        .map_err(|error| pack_error(locator, error))?;
    let control = bundle.control().ok_or_else(|| {
        LabError::package_invalid(format!(
            "{}: the pack has no control.json",
            locator.display()
        ))
    })?;
    let text = |key: &str| {
        control
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                LabError::package_invalid(format!(
                    "{}: control.json has no string {key}",
                    locator.display()
                ))
            })
    };
    Ok(PackIdentity {
        game: text("game")?,
        package_id: text("package_id")?,
        server: text("server")?,
        entry_task_id: text("entry_task_id")?,
    })
}

fn absolute(path: &Path) -> LabResult<PathBuf> {
    std::path::absolute(path)
        .map_err(|error| LabError::usage(format!("{}: {error}", path.display())))
}

fn total_bytes<'a>(files: impl Iterator<Item = &'a Vec<u8>>) -> u64 {
    files.map(|bytes| bytes.len() as u64).sum()
}

fn write_new(path: &Path, bytes: &[u8]) -> LabResult<()> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .and_then(|mut file| file.write_all(bytes))
        .map_err(|error| write_error(path, error))
}

/// The loader's refusal of one pack directory.
fn pack_error(locator: &Path, error: ContainmentError) -> LabError {
    LabError::package_invalid(format!("{}: {error}", locator.display()))
}

fn write_error(path: &Path, error: std::io::Error) -> LabError {
    bundle_error(
        ResourceToolingCode::PackageBundleWriteFailed,
        format!("{}: {error}", path.display()),
    )
}

fn bundle_error(code: ResourceToolingCode, message: String) -> LabError {
    LabError::new(LabErrorClass::UsageValidation, code.as_str(), message, &[])
}
