// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #288 A2b: content-directory task packs for authors and resource pipelines.
//! `package digest` names a directory by its content; `package bundle` lays out the standard
//! package's resource section: one hash-named directory per task pack and the bundle index v2.
//! Both read through the loader's own snapshot and admit every pack in full.

use crate::{
    PackageBundleRequest, PackageBundleResponse, PackageDigestRequest, PackageDigestResponse,
};
use actingcommand_contract::{
    BundleIndexV2, BundleIndexVersion, BundlePackV2, ContentDirectory, ContentDirectoryVersion,
    LabError, LabErrorClass, LabResult, PackageRef, content_directory_digest,
};
use actingcommand_pack_containment::{Containment, ContainmentError, InstanceId, Sha256Hash};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const APPLICATIONS_SCHEMA: &str = "actingcommand.applications.v1";
/// The read and the admission of one pack directory share one deadline.
const PACK_DEADLINE: Duration = Duration::from_secs(60);

/// The identity a pack's control declaration states, read from its admitted bundle.
struct PackIdentity {
    game: String,
    package_id: String,
    server: String,
    entry_task_id: String,
}

/// Reads the directory once through the loader's snapshot, then admits it in full against
/// the reference computed from that snapshot.
pub fn package_digest(request: PackageDigestRequest) -> LabResult<PackageDigestResponse> {
    let package = absolute(&request.package)?;
    let mut containment = Containment::for_metadata_validation();
    let deadline = Instant::now() + PACK_DEADLINE;
    let snapshot = containment
        .snapshot_content_directory(&package, deadline)
        .map_err(|error| pack_error(&package, error, None))?;
    let file_count = snapshot.entries.len();
    let byte_count = total_bytes(snapshot.entries.values());
    let identity = admit(
        &mut containment,
        &package,
        &snapshot.reference,
        file_count,
        deadline,
    )?;
    Ok(PackageDigestResponse {
        status: "valid".to_string(),
        package: package.display().to_string(),
        reference: snapshot.reference,
        package_id: identity.package_id,
        server: identity.server,
        entry_task_id: identity.entry_task_id,
        file_count,
        byte_count,
    })
}

/// Writes `<out>.part` and renames it to `out` only once every pack was copied, admitted
/// under its digest name and indexed; a failure leaves `<out>.part` in place and says so.
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
                "package_bundle_out_exists",
                format!(
                    "{} already exists; package bundle only writes a new directory",
                    path.display()
                ),
            ));
        }
    }
    let applications_bytes = fs::read(&request.applications).map_err(|error| {
        bundle_error(
            "package_bundle_applications_invalid",
            format!("{}: {error}", request.applications.display()),
        )
    })?;
    let applications = Applications::parse(&applications_bytes)?;
    let directories = pack_directories(&absolute(&request.packs_root)?)?;
    fs::create_dir(&staging).map_err(|error| write_error(&staging, error))?;
    write_bundle(
        &staging,
        &applications,
        &applications_bytes,
        &directories,
        request.source,
    )
    .and_then(|index| {
        fs::rename(&staging, &out).map_err(|error| write_error(&out, error))?;
        Ok(PackageBundleResponse {
            status: "written".to_string(),
            out: out.display().to_string(),
            index,
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
) -> LabResult<BundleIndexV2> {
    let packs_dir = staging.join("packs");
    fs::create_dir(&packs_dir).map_err(|error| write_error(&packs_dir, error))?;
    let mut containment = Containment::for_metadata_validation();
    let mut packs = Vec::new();
    for directory in directories {
        let snapshot = containment
            .snapshot_content_directory(directory, Instant::now() + PACK_DEADLINE)
            .map_err(|error| pack_error(directory, error, None))?;
        // Only the control declaration and the resource tree enter the standard package.
        let files = snapshot
            .entries
            .into_iter()
            .filter(|(path, _)| path == "control.json" || path.starts_with("resources/"))
            .collect::<BTreeMap<_, _>>();
        let digest = content_directory_digest(
            files
                .iter()
                .map(|(path, bytes)| (path.as_str(), *Sha256Hash::digest(bytes).as_bytes())),
        );
        let target = packs_dir.join(&digest);
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
        // The copy is read again and admitted in full under its digest name.
        let reference = ContentDirectory {
            schema_version: ContentDirectoryVersion::V1,
            sha256: digest.clone(),
        };
        let identity = admit(
            &mut containment,
            &target,
            &reference,
            files.len(),
            Instant::now() + PACK_DEADLINE,
        )?;
        if identity.game != applications.game || !applications.servers.contains(&identity.server) {
            return Err(bundle_error(
                "package_bundle_pack_mismatch",
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
        packs.push(BundlePackV2 {
            package_id: identity.package_id,
            server: identity.server,
            entry_task_id: identity.entry_task_id,
            path: format!("packs/{digest}"),
            digest,
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
                "package_bundle_default_package_missing",
                format!(
                    "servers.{server}.default_package_id {package_id:?} names no pack of server {server:?}"
                ),
            ));
        }
    }
    let index = BundleIndexV2 {
        schema_version: BundleIndexVersion::V2,
        game: applications.game.clone(),
        source,
        packs,
    };
    index
        .validate()
        .map_err(|error| bundle_error("package_bundle_index_invalid", error.to_string()))?;
    let mut encoded = serde_json::to_vec_pretty(&index)
        .map_err(|error| bundle_error("package_bundle_index_invalid", error.to_string()))?;
    encoded.push(b'\n');
    write_new(&staging.join("applications.json"), applications_bytes)?;
    write_new(&staging.join("bundle.json"), &encoded)?;
    Ok(index)
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
                "package_bundle_applications_invalid",
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
    let invalid = |message: String| bundle_error("package_bundle_packs_root_invalid", message);
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

/// Admits `locator` in full against `reference` and reads the identity its control states.
fn admit(
    containment: &mut Containment,
    locator: &Path,
    reference: &ContentDirectory,
    file_count: usize,
    deadline: Instant,
) -> LabResult<PackIdentity> {
    let context = Some((reference, file_count));
    let instance = InstanceId::new("package-directory")
        .map_err(|error| pack_error(locator, error, context))?;
    let bundle = containment
        .load_path(
            &instance,
            locator,
            &PackageRef::ContentDirectory(reference.clone()),
            false,
            deadline,
        )
        .map_err(|error| pack_error(locator, error, context))?;
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

/// The loader's refusal of one pack directory, with the digest computed from its snapshot
/// once there is one.
fn pack_error(
    locator: &Path,
    error: ContainmentError,
    context: Option<(&ContentDirectory, usize)>,
) -> LabError {
    let computed = context
        .map(|(reference, file_count)| {
            format!(
                " (content digest {} over {file_count} files)",
                reference.sha256
            )
        })
        .unwrap_or_default();
    LabError::package_invalid(format!("{}: {error}{computed}", locator.display()))
}

fn write_error(path: &Path, error: std::io::Error) -> LabError {
    bundle_error(
        "package_bundle_write_failed",
        format!("{}: {error}", path.display()),
    )
}

fn bundle_error(code: &str, message: String) -> LabError {
    LabError::new(LabErrorClass::UsageValidation, code, message, &[])
}
