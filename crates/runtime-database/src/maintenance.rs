// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use rusqlite::backup::{Backup, StepResult};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::time::Instant;

const MANIFEST_FILE: &str = "backup.json";
const MAX_MANIFEST_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaintenanceLimits {
    pub max_bytes: u64,
    pub max_files: usize,
    pub max_events: usize,
    pub timeout_seconds: u64,
    pub pages_per_step: i32,
}

impl Default for MaintenanceLimits {
    fn default() -> Self {
        Self {
            max_bytes: 512 * 1024 * 1024,
            max_files: 16_384,
            max_events: 200_000,
            timeout_seconds: 120,
            pages_per_step: 128,
        }
    }
}

impl MaintenanceLimits {
    pub fn deadline(self) -> RuntimeDatabaseResult<Instant> {
        if self.max_bytes == 0
            || self.max_bytes > 4 * 1024 * 1024 * 1024
            || self.max_files == 0
            || self.max_files > 65_536
            || self.max_events == 0
            || self.max_events > 1_000_000
            || !(1..=600).contains(&self.timeout_seconds)
            || !(1..=1024).contains(&self.pages_per_step)
        {
            return Err(failure(
                DatabaseCode::MaintenanceLimitsInvalid.code(),
                DatabaseLocation::ValidateMaintenanceLimits.location(),
            ));
        }
        Instant::now()
            .checked_add(Duration::from_secs(self.timeout_seconds))
            .ok_or_else(|| {
                failure(
                    DatabaseCode::MaintenanceDeadlineOverflow.code(),
                    DatabaseLocation::ValidateMaintenanceLimits.location(),
                )
            })
    }

    pub fn check(self, bytes: u64, files: usize, deadline: Instant) -> RuntimeDatabaseResult<()> {
        if bytes > self.max_bytes || files > self.max_files || Instant::now() >= deadline {
            return Err(failure(
                DatabaseCode::MaintenanceBudgetExceeded.code(),
                DatabaseLocation::BoundMaintenance.location(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaintenanceFile {
    pub relative_path: String,
    pub byte_count: u64,
    pub sha256: String,
}

/// A warning of a maintenance pass (Workflow #378 A6a): its outcome, a warning code with the
/// operation it arose in. The backup manifest and the ledger-maintenance report carry it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaintenanceWarning {
    pub outcome: actingcommand_contract::outcome::Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseBackup {
    pub schema_version: String,
    pub binding: serde_json::Value,
    pub files: Vec<MaintenanceFile>,
    pub warnings: Vec<MaintenanceWarning>,
    pub backup_id: String,
    pub integrity_tag: String,
}

impl DatabaseBackup {
    fn material(&self) -> RuntimeDatabaseResult<Vec<u8>> {
        serde_json::to_vec(&(
            &self.schema_version,
            &self.binding,
            &self.files,
            &self.warnings,
        ))
        .map_err(|_| {
            failure(
                DatabaseCode::BackupManifestInvalid.code(),
                DatabaseLocation::EncodeBackupManifest.location(),
            )
        })
    }
}

impl RuntimeDatabase {
    /// The caller retains the Runtime owner and Ledger lock for this complete operation.
    pub fn backup(
        &self,
        destination: &Path,
        material: &[MaintenanceFile],
        locked_material: &[(String, Vec<u8>)],
        binding: serde_json::Value,
        limits: MaintenanceLimits,
        deadline: Instant,
    ) -> RuntimeDatabaseResult<DatabaseBackup> {
        let destination = resolve_maintenance_destination(destination)?;
        let destination = destination.as_path();
        let mut warnings = Vec::new();
        let result = (|| {
            require_disjoint(&self.root, destination)?;
            limits.check(0, material.len(), deadline)?;
            if destination.exists() {
                return Err(failure(
                    DatabaseCode::BackupDestinationExists.code(),
                    DatabaseLocation::CreateDatabaseBackup.location(),
                ));
            }
            fs::create_dir(destination)
                .map_err(|error| io(DatabaseLocation::CreateBackupDirectory.location(), &error))?;
            let destination_file = destination.join(DATABASE_FILE);
            let mut target = Connection::open(&destination_file).map_err(|error| {
                RuntimeDatabaseError::sql(
                    DatabaseLocation::OpenBackupDestination.location(),
                    &error,
                )
            })?;
            target
                .execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")
                .map_err(|error| {
                    RuntimeDatabaseError::sql(
                        DatabaseLocation::ConfigureBackupDestination.location(),
                        &error,
                    )
                })?;
            let connection = self.connection(DatabaseLocation::BackupDatabase.location())?;
            let page_size: i64 = connection
                .pragma_query_value(None, "page_size", |row| row.get(0))
                .map_err(|error| {
                    RuntimeDatabaseError::sql(
                        DatabaseLocation::ReadBackupPageSize.location(),
                        &error,
                    )
                })?;
            let page_count: i64 = connection
                .pragma_query_value(None, "page_count", |row| row.get(0))
                .map_err(|error| {
                    RuntimeDatabaseError::sql(
                        DatabaseLocation::ReadBackupPageCount.location(),
                        &error,
                    )
                })?;
            let page_size = u64::try_from(page_size).map_err(|_| {
                failure(
                    DatabaseCode::BackupPageSizeInvalid.code(),
                    DatabaseLocation::ReadBackupPageSize.location(),
                )
            })?;
            let page_count = u64::try_from(page_count).map_err(|_| {
                failure(
                    DatabaseCode::BackupPageCountInvalid.code(),
                    DatabaseLocation::ReadBackupPageCount.location(),
                )
            })?;
            limits.check(
                page_count.checked_mul(page_size).ok_or_else(|| {
                    failure(
                        DatabaseCode::MaintenanceSizeOverflow.code(),
                        DatabaseLocation::BoundBackupPages.location(),
                    )
                })?,
                material.len(),
                deadline,
            )?;
            let backup = Backup::new(&connection, &mut target).map_err(|error| {
                RuntimeDatabaseError::sql(DatabaseLocation::BeginDatabaseBackup.location(), &error)
            })?;
            let mut busy = 0;
            loop {
                limits.check(page_count * page_size, material.len(), deadline)?;
                match backup.step(limits.pages_per_step).map_err(|error| {
                    RuntimeDatabaseError::sql(
                        DatabaseLocation::StepDatabaseBackup.location(),
                        &error,
                    )
                })? {
                    StepResult::Done => break,
                    StepResult::More => {}
                    state @ (StepResult::Busy | StepResult::Locked) => {
                        busy += 1;
                        if warnings.is_empty() {
                            // At most three waits within the deadline follow; the
                            // code says whether SQLite reported busy or locked.
                            let code = if matches!(state, StepResult::Busy) {
                                DatabaseCode::SqliteBackupBusy
                            } else {
                                DatabaseCode::SqliteBackupLocked
                            };
                            warnings.push(MaintenanceWarning {
                                outcome: actingcommand_contract::outcome::Outcome::new(code)
                                    .operation(DatabaseLocation::StepDatabaseBackup),
                            });
                        }
                        if busy > 3 {
                            return Err(failure(
                                DatabaseCode::BackupBusyExhausted.code(),
                                DatabaseLocation::StepDatabaseBackupAfterWarning.location(),
                            ));
                        }
                        let remaining = deadline.saturating_duration_since(Instant::now());
                        std::thread::sleep(Duration::from_millis(50 * busy).min(remaining));
                    }
                    _ => {
                        return Err(failure(
                            DatabaseCode::BackupResultUnknown.code(),
                            DatabaseLocation::StepDatabaseBackup.location(),
                        ));
                    }
                }
            }
            drop(backup);
            target.close().map_err(|(_, error)| {
                RuntimeDatabaseError::sql(DatabaseLocation::CloseBackupDatabase.location(), &error)
            })?;
            drop(connection);
            sync_file(&destination_file)?;
            let mut files = vec![inspect_file(destination, DATABASE_FILE, limits, deadline)?];
            copy_material(
                &self.root,
                destination,
                INTEGRITY_KEY_FILE,
                limits,
                deadline,
            )?;
            files.push(inspect_file(
                destination,
                INTEGRITY_KEY_FILE,
                limits,
                deadline,
            )?);
            let mut bytes = files.iter().map(|file| file.byte_count).sum::<u64>();
            for file in material {
                if file.relative_path == DATABASE_FILE
                    || file.relative_path == INTEGRITY_KEY_FILE
                    || file.relative_path == MANIFEST_FILE
                {
                    return Err(failure(
                        DatabaseCode::BackupMaterialReserved.code(),
                        DatabaseLocation::CopyBackupMaterial.location(),
                    ));
                }
                limits.check(bytes, files.len(), deadline)?;
                if inspect_file(&self.root, &file.relative_path, limits, deadline)? != *file {
                    return Err(failure(
                        DatabaseCode::BackupSourceChanged.code(),
                        DatabaseLocation::CopyBackupMaterial.location(),
                    ));
                }
                copy_material(
                    &self.root,
                    destination,
                    &file.relative_path,
                    limits,
                    deadline,
                )?;
                let copied = inspect_file(destination, &file.relative_path, limits, deadline)?;
                if copied != *file
                    || inspect_file(&self.root, &file.relative_path, limits, deadline)? != *file
                {
                    return Err(failure(
                        DatabaseCode::BackupSourceChanged.code(),
                        DatabaseLocation::VerifyBackupMaterial.location(),
                    ));
                }
                bytes = bytes.checked_add(copied.byte_count).ok_or_else(|| {
                    failure(
                        DatabaseCode::MaintenanceSizeOverflow.code(),
                        DatabaseLocation::CountBackupMaterial.location(),
                    )
                })?;
                files.push(copied);
            }
            for (relative, content) in locked_material {
                if relative == DATABASE_FILE
                    || relative == INTEGRITY_KEY_FILE
                    || relative == MANIFEST_FILE
                    || files.iter().any(|file| &file.relative_path == relative)
                {
                    return Err(failure(
                        DatabaseCode::BackupMaterialReserved.code(),
                        DatabaseLocation::CopyLockedBackupMaterial.location(),
                    ));
                }
                bytes = bytes.checked_add(content.len() as u64).ok_or_else(|| {
                    failure(
                        DatabaseCode::MaintenanceSizeOverflow.code(),
                        DatabaseLocation::CountLockedBackupMaterial.location(),
                    )
                })?;
                limits.check(bytes, files.len() + 1, deadline)?;
                let path = material_path(destination, relative)?;
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).map_err(|error| {
                        io(
                            DatabaseLocation::CreateLockedBackupDirectory.location(),
                            &error,
                        )
                    })?;
                }
                write_new(&path, content)?;
                let file = inspect_file(destination, relative, limits, deadline)?;
                if file.sha256 != digest(content) || file.byte_count != content.len() as u64 {
                    return Err(failure(
                        DatabaseCode::BackupMaterialMismatch.code(),
                        DatabaseLocation::VerifyLockedBackupMaterial.location(),
                    ));
                }
                files.push(file);
            }
            limits.check(bytes, files.len(), deadline)?;
            files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
            let mut manifest = DatabaseBackup {
                schema_version: "actingcommand.database-backup.v1".into(),
                binding,
                files,
                warnings: warnings.clone(),
                backup_id: String::new(),
                integrity_tag: String::new(),
            };
            let material = manifest.material()?;
            if material.len() as u64 > MAX_MANIFEST_BYTES {
                return Err(failure(
                    DatabaseCode::BackupManifestTooLarge.code(),
                    DatabaseLocation::CompleteDatabaseBackup.location(),
                ));
            }
            manifest.backup_id = digest(&material);
            manifest.integrity_tag = self.integrity_tag(
                "database-backup-v1",
                &[&material, manifest.backup_id.as_bytes()],
            );
            let encoded = serde_json::to_vec(&manifest).map_err(|_| {
                failure(
                    DatabaseCode::BackupManifestInvalid.code(),
                    DatabaseLocation::CompleteDatabaseBackup.location(),
                )
            })?;
            write_new(&destination.join(MANIFEST_FILE), &encoded)?;
            sync_state_directory(destination)?;
            sync_state_directory(destination.parent().ok_or_else(|| {
                failure(
                    DatabaseCode::BackupParentMissing.code(),
                    DatabaseLocation::CompleteDatabaseBackup.location(),
                )
            })?)?;
            Ok(manifest)
        })();
        result.map_err(|error: RuntimeDatabaseError| error.with_warnings(warnings))
    }

    pub fn verify_backup(
        &self,
        backup_root: &Path,
        limits: MaintenanceLimits,
        deadline: Instant,
    ) -> RuntimeDatabaseResult<DatabaseBackup> {
        require_disjoint(&self.root, backup_root)?;
        let manifest = read_manifest(backup_root)?;
        let material = manifest.material()?;
        if manifest.schema_version != "actingcommand.database-backup.v1"
            || manifest.backup_id != digest(&material)
            || manifest.integrity_tag
                != self.integrity_tag(
                    "database-backup-v1",
                    &[&material, manifest.backup_id.as_bytes()],
                )
        {
            return Err(failure(
                DatabaseCode::BackupManifestMismatch.code(),
                DatabaseLocation::VerifyDatabaseBackup.location(),
            ));
        }
        let mut bytes = 0_u64;
        let mut previous: Option<&str> = None;
        for file in &manifest.files {
            if previous.is_some_and(|path| path >= file.relative_path.as_str()) {
                return Err(failure(
                    DatabaseCode::BackupManifestOrderInvalid.code(),
                    DatabaseLocation::VerifyDatabaseBackup.location(),
                ));
            }
            previous = Some(&file.relative_path);
            bytes = bytes.checked_add(file.byte_count).ok_or_else(|| {
                failure(
                    DatabaseCode::MaintenanceSizeOverflow.code(),
                    DatabaseLocation::VerifyDatabaseBackup.location(),
                )
            })?;
            limits.check(bytes, manifest.files.len(), deadline)?;
            if inspect_file(backup_root, &file.relative_path, limits, deadline)? != *file {
                return Err(failure(
                    DatabaseCode::BackupMaterialMismatch.code(),
                    DatabaseLocation::VerifyDatabaseBackup.location(),
                ));
            }
        }
        for required in [DATABASE_FILE, INTEGRITY_KEY_FILE] {
            if !manifest
                .files
                .iter()
                .any(|file| file.relative_path == required)
            {
                return Err(failure(
                    DatabaseCode::BackupMaterialMissing.code(),
                    DatabaseLocation::VerifyDatabaseBackup.location(),
                ));
            }
        }
        let key = fs::read(backup_root.join(INTEGRITY_KEY_FILE))
            .map_err(|error| io(DatabaseLocation::ReadBackupKey.location(), &error))?;
        if key.as_slice() != self.integrity_key.as_ref() {
            return Err(failure(
                DatabaseCode::BackupKeyMismatch.code(),
                DatabaseLocation::VerifyDatabaseBackup.location(),
            ));
        }
        Ok(manifest)
    }

    /// Caller already verified the current source lineage and retained both roots' guards.
    pub fn restore_backup(
        &self,
        backup_root: &Path,
        target: &Path,
        limits: MaintenanceLimits,
        deadline: Instant,
    ) -> RuntimeDatabaseResult<DatabaseBackup> {
        require_disjoint(&self.root, target)?;
        require_disjoint(backup_root, target)?;
        require_regular_directory(target)?;
        for entry in fs::read_dir(target)
            .map_err(|error| io(DatabaseLocation::InspectRestoreTarget.location(), &error))?
        {
            let entry = entry
                .map_err(|error| io(DatabaseLocation::InspectRestoreTarget.location(), &error))?;
            if entry.file_name() != "owner.lock" {
                return Err(failure(
                    DatabaseCode::RestoreTargetNotEmpty.code(),
                    DatabaseLocation::RestoreDatabaseBackup.location(),
                ));
            }
        }
        let manifest = self.verify_backup(backup_root, limits, deadline)?;
        for file in &manifest.files {
            copy_material(backup_root, target, &file.relative_path, limits, deadline)?;
        }
        for file in &manifest.files {
            if inspect_file(target, &file.relative_path, limits, deadline)? != *file {
                return Err(failure(
                    DatabaseCode::RestoredMaterialMismatch.code(),
                    DatabaseLocation::RestoreDatabaseBackup.location(),
                ));
            }
        }
        sync_state_directory(target)?;
        Ok(manifest)
    }
}

pub fn read_manifest(root: &Path) -> RuntimeDatabaseResult<DatabaseBackup> {
    let path = root.join(MANIFEST_FILE);
    require_regular_file(&path)?;
    let metadata = fs::metadata(&path)
        .map_err(|error| io(DatabaseLocation::ReadBackupManifest.location(), &error))?;
    if metadata.len() == 0 || metadata.len() > MAX_MANIFEST_BYTES {
        return Err(failure(
            DatabaseCode::BackupManifestSizeInvalid.code(),
            DatabaseLocation::ReadBackupManifest.location(),
        ));
    }
    let bytes = fs::read(path)
        .map_err(|error| io(DatabaseLocation::ReadBackupManifest.location(), &error))?;
    serde_json::from_slice(&bytes).map_err(|_| {
        failure(
            DatabaseCode::BackupManifestInvalid.code(),
            DatabaseLocation::ReadBackupManifest.location(),
        )
    })
}

pub fn inspect_file(
    root: &Path,
    relative: &str,
    limits: MaintenanceLimits,
    deadline: Instant,
) -> RuntimeDatabaseResult<MaintenanceFile> {
    let path = material_path(root, relative)?;
    require_regular_file(&path)?;
    let mut file = fs::File::open(&path)
        .map_err(|error| io(DatabaseLocation::OpenMaintenanceMaterial.location(), &error))?;
    let length = file
        .metadata()
        .map_err(|error| {
            io(
                DatabaseLocation::InspectMaintenanceMaterial.location(),
                &error,
            )
        })?
        .len();
    limits.check(length, 1, deadline)?;
    let mut hash = Sha256::new();
    let mut count = 0_u64;
    let mut buffer = [0; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| io(DatabaseLocation::ReadMaintenanceMaterial.location(), &error))?;
        if read == 0 {
            break;
        }
        count = count.checked_add(read as u64).ok_or_else(|| {
            failure(
                DatabaseCode::MaintenanceSizeOverflow.code(),
                DatabaseLocation::HashMaintenanceMaterial.location(),
            )
        })?;
        limits.check(count, 1, deadline)?;
        hash.update(&buffer[..read]);
    }
    if count != length {
        return Err(failure(
            DatabaseCode::MaintenanceMaterialChanged.code(),
            DatabaseLocation::HashMaintenanceMaterial.location(),
        ));
    }
    Ok(MaintenanceFile {
        relative_path: relative.into(),
        byte_count: count,
        sha256: format!("sha256:{:x}", hash.finalize()),
    })
}

pub fn list_material(
    root: &Path,
    directory: &str,
    excluded: &[&str],
    limits: MaintenanceLimits,
    deadline: Instant,
) -> RuntimeDatabaseResult<Vec<MaintenanceFile>> {
    let path = material_path(root, directory)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut pending = vec![(path, directory.to_owned())];
    let mut files = Vec::new();
    let mut bytes = 0_u64;
    let mut directories = 0;
    while let Some((path, relative)) = pending.pop() {
        require_regular_directory(&path)?;
        directories += 1;
        limits.check(bytes, files.len() + directories + pending.len(), deadline)?;
        for entry in fs::read_dir(path)
            .map_err(|error| io(DatabaseLocation::ListMaintenanceMaterial.location(), &error))?
        {
            let entry = entry.map_err(|error| {
                io(DatabaseLocation::ListMaintenanceMaterial.location(), &error)
            })?;
            let name = entry.file_name().into_string().map_err(|_| {
                failure(
                    DatabaseCode::MaintenancePathInvalid.code(),
                    DatabaseLocation::ListMaintenanceMaterial.location(),
                )
            })?;
            let child = format!("{relative}/{name}");
            if excluded.contains(&child.as_str()) {
                continue;
            }
            let metadata = fs::symlink_metadata(entry.path()).map_err(|error| {
                io(
                    DatabaseLocation::InspectMaintenanceMaterial.location(),
                    &error,
                )
            })?;
            if is_link_or_reparse(&metadata) {
                return Err(failure(
                    DatabaseCode::MaintenancePathUnsafe.code(),
                    DatabaseLocation::ListMaintenanceMaterial.location(),
                ));
            }
            if metadata.is_dir() {
                pending.push((entry.path(), child));
            } else {
                let file = inspect_file(root, &child, limits, deadline)?;
                bytes = bytes.checked_add(file.byte_count).ok_or_else(|| {
                    failure(
                        DatabaseCode::MaintenanceSizeOverflow.code(),
                        DatabaseLocation::ListMaintenanceMaterial.location(),
                    )
                })?;
                files.push(file);
            }
            limits.check(bytes, files.len() + directories + pending.len(), deadline)?;
        }
    }
    files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(files)
}

/// Resolves caller-relative destinations once against the process working directory.
pub fn resolve_maintenance_destination(target: &Path) -> RuntimeDatabaseResult<PathBuf> {
    if target.as_os_str().is_empty() {
        return Err(failure(
            DatabaseCode::MaintenanceTargetInvalid.code(),
            DatabaseLocation::ResolveMaintenanceTarget.location(),
        ));
    }
    let absolute = std::path::absolute(target).map_err(|error| {
        io(
            DatabaseLocation::ResolveMaintenanceTarget.location(),
            &error,
        )
    })?;
    if absolute.try_exists().map_err(|error| {
        io(
            DatabaseLocation::InspectMaintenanceTarget.location(),
            &error,
        )
    })? {
        return absolute.canonicalize().map_err(|error| {
            io(
                DatabaseLocation::ResolveMaintenanceTarget.location(),
                &error,
            )
        });
    }
    let parent = absolute
        .parent()
        .ok_or_else(|| {
            failure(
                DatabaseCode::MaintenanceTargetInvalid.code(),
                DatabaseLocation::ResolveMaintenanceTarget.location(),
            )
        })?
        .canonicalize()
        .map_err(|error| {
            io(
                DatabaseLocation::ResolveMaintenanceTargetParent.location(),
                &error,
            )
        })?;
    Ok(parent.join(absolute.file_name().ok_or_else(|| {
        failure(
            DatabaseCode::MaintenanceTargetInvalid.code(),
            DatabaseLocation::ResolveMaintenanceTarget.location(),
        )
    })?))
}

pub fn require_disjoint(source: &Path, target: &Path) -> RuntimeDatabaseResult<()> {
    let source = source.canonicalize().map_err(|error| {
        io(
            DatabaseLocation::ResolveMaintenanceSource.location(),
            &error,
        )
    })?;
    let target = resolve_maintenance_destination(target)?;
    if source.starts_with(&target) || target.starts_with(&source) {
        return Err(failure(
            DatabaseCode::MaintenancePathsOverlap.code(),
            DatabaseLocation::ValidateMaintenancePaths.location(),
        ));
    }
    Ok(())
}

fn material_path(root: &Path, relative: &str) -> RuntimeDatabaseResult<PathBuf> {
    if relative.is_empty()
        || relative.contains('\\')
        || relative.contains(':')
        || relative
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(failure(
            DatabaseCode::MaintenancePathInvalid.code(),
            DatabaseLocation::ResolveMaintenanceMaterial.location(),
        ));
    }
    let mut path = root.to_path_buf();
    require_regular_directory(&path)?;
    let parts = relative.split('/').collect::<Vec<_>>();
    for (index, part) in parts.iter().enumerate() {
        path.push(part);
        if index + 1 < parts.len() && path.exists() {
            require_regular_directory(&path)?;
        }
    }
    Ok(path)
}

fn copy_material(
    source: &Path,
    target: &Path,
    relative: &str,
    limits: MaintenanceLimits,
    deadline: Instant,
) -> RuntimeDatabaseResult<()> {
    let input = material_path(source, relative)?;
    require_regular_file(&input)?;
    let output = material_path(target, relative)?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            io(
                DatabaseLocation::CreateBackupMaterialDirectory.location(),
                &error,
            )
        })?;
    }
    let mut source = fs::File::open(input)
        .map_err(|error| io(DatabaseLocation::OpenBackupMaterial.location(), &error))?;
    let mut destination = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)
        .map_err(|error| io(DatabaseLocation::CreateBackupMaterial.location(), &error))?;
    let mut buffer = [0; 64 * 1024];
    let mut count = 0;
    loop {
        let read = source
            .read(&mut buffer)
            .map_err(|error| io(DatabaseLocation::ReadBackupMaterial.location(), &error))?;
        if read == 0 {
            break;
        }
        count += read as u64;
        limits.check(count, 1, deadline)?;
        destination
            .write_all(&buffer[..read])
            .map_err(|error| io(DatabaseLocation::WriteBackupMaterial.location(), &error))?;
    }
    destination
        .sync_all()
        .map_err(|error| io(DatabaseLocation::SyncBackupMaterial.location(), &error))?;
    sync_state_directory(output.parent().ok_or_else(|| {
        failure(
            DatabaseCode::BackupParentMissing.code(),
            DatabaseLocation::SyncBackupMaterial.location(),
        )
    })?)
}

fn sync_file(path: &Path) -> RuntimeDatabaseResult<()> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .and_then(|file| file.sync_all())
        .map_err(|error| io(DatabaseLocation::SyncBackupDatabase.location(), &error))
}
fn write_new(path: &Path, bytes: &[u8]) -> RuntimeDatabaseResult<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| io(DatabaseLocation::CreateBackupManifest.location(), &error))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| io(DatabaseLocation::SyncBackupManifest.location(), &error))
}
pub fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
fn io(operation: Location, error: &std::io::Error) -> RuntimeDatabaseError {
    RuntimeDatabaseError::io(
        DatabaseCode::DatabaseMaintenanceIoFailed.code(),
        operation,
        error,
    )
}
