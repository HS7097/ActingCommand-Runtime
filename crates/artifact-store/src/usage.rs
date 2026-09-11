// SPDX-License-Identifier: AGPL-3.0-only

use crate::{ArtifactStoreError, ArtifactStoreResult};
use actingcommand_contract::{
    ArtifactEvictionIntentRecord, ArtifactMaterial, ProjectedArtifactReference,
};
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::Read;
use std::path::{Path, PathBuf};

/// OS coordination only; retention policy and availability remain Ledger facts.
pub struct ArtifactUseGuard {
    _lock: Option<File>,
}

pub struct ArtifactDeleteGuard {
    root: PathBuf,
    reference: ProjectedArtifactReference,
    _lock: File,
    material: Option<File>,
    removal_attempted: bool,
}

impl ArtifactDeleteGuard {
    pub fn reference(&self) -> &ProjectedArtifactReference {
        &self.reference
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn material_present(&self) -> bool {
        self.material.is_some()
    }

    /// The Runtime's opaque Ledger permit calls this only after durable intent admission.
    /// The fixed and material locks remain held until the outcome has been committed.
    pub fn remove_after_durable_intent(
        &mut self,
        intent: &ArtifactEvictionIntentRecord,
    ) -> std::io::Result<()> {
        if self.removal_attempted
            || self.material.is_none()
            || intent.identity.artifact != self.reference
            || actingcommand_contract::ArtifactRetentionFact::EvictionIntent(intent.clone())
                .validate()
                .is_err()
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "artifact deletion requires the original unconsumed material guard and intent",
            ));
        }
        self.removal_attempted = true;
        let key = self.reference.object_key().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "artifact object key required",
            )
        })?;
        let path =
            crate::store::safe_object_path(&self.root, key).map_err(std::io::Error::other)?;
        fs::remove_file(path)
    }
}

pub(crate) fn publication_guard(
    root: &Path,
    reference: &ProjectedArtifactReference,
) -> ArtifactStoreResult<ArtifactUseGuard> {
    let lock = open_lock(root, reference, true)?;
    let Some(lock) = lock else {
        return Err(failure(
            "artifact_use_lock_missing",
            "publication lock was not created",
        ));
    };
    lock.try_lock_shared()
        .map_err(|error| lock_failure("artifact_use_lock_failed", error))?;
    Ok(ArtifactUseGuard { _lock: Some(lock) })
}

pub(crate) fn reader_guard(
    root: &Path,
    reference: &ProjectedArtifactReference,
    material: &File,
) -> ArtifactStoreResult<ArtifactUseGuard> {
    let lock = open_lock(root, reference, false)?;
    if let Some(lock) = &lock {
        lock.try_lock_shared()
            .map_err(|error| lock_failure("artifact_use_lock_failed", error))?;
    }
    // Existing offline roots need no new lock file. The material lock also fences
    // a deleter that first creates the fixed lock while this reader is open.
    material
        .try_lock_shared()
        .map_err(|error| lock_failure("artifact_use_lock_failed", error))?;
    Ok(ArtifactUseGuard { _lock: lock })
}

/// A busy object is deferred; no deletion or Ledger callback occurs here.
pub fn try_artifact_delete_guard(
    root: impl AsRef<Path>,
    reference: &ProjectedArtifactReference,
) -> ArtifactStoreResult<Option<ArtifactDeleteGuard>> {
    let root = root
        .as_ref()
        .canonicalize()
        .map_err(|error| io_failure("artifact_root_failed", error))?;
    let lock = open_lock(&root, reference, true)?
        .ok_or_else(|| failure("artifact_use_lock_missing", "delete lock was not created"))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Ok(None),
        Err(TryLockError::Error(error)) => {
            return Err(io_failure("artifact_use_lock_failed", error));
        }
    }
    let key = reference
        .object_key()
        .ok_or_else(|| failure("artifact_object_key_missing", "object key required"))?;
    let path = crate::store::safe_object_path(&root, key)?;
    let material = match OpenOptions::new().read(true).write(true).open(&path) {
        Ok(mut file) => {
            match file.try_lock() {
                Ok(()) => {}
                Err(TryLockError::WouldBlock) => return Ok(None),
                Err(TryLockError::Error(error)) => {
                    return Err(io_failure("artifact_use_lock_failed", error));
                }
            }
            let material = ArtifactMaterial::read_from(
                &mut (&mut file).take(reference.byte_count.saturating_add(1)),
            )
            .map_err(|error| io_failure("artifact_read_failed", error))?;
            if material.byte_count() != reference.byte_count
                || material.sha256() != reference.sha256
            {
                return Err(failure(
                    "artifact_hash_mismatch",
                    "exclusive material identity differs from the original Ledger reference",
                ));
            }
            Some(file)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(io_failure("artifact_read_failed", error)),
    };
    Ok(Some(ArtifactDeleteGuard {
        root,
        reference: reference.clone(),
        _lock: lock,
        material,
        removal_attempted: false,
    }))
}

fn open_lock(
    root: &Path,
    reference: &ProjectedArtifactReference,
    create: bool,
) -> ArtifactStoreResult<Option<File>> {
    reference
        .validate()
        .map_err(|error| failure("artifact_reference_invalid", error))?;
    let key = reference
        .object_key()
        .ok_or_else(|| failure("artifact_object_key_missing", "object key required"))?;
    let relative = Path::new(key);
    let name = relative
        .file_name()
        .ok_or_else(|| failure("artifact_path_invalid", "object filename required"))?;
    let directory = root.join("artifact-use-locks");
    if create {
        fs::create_dir_all(&directory)
            .map_err(|error| io_failure("artifact_use_lock_failed", error))?;
    }
    let directory_meta = match fs::symlink_metadata(&directory) {
        Ok(metadata) => metadata,
        Err(error) if !create && error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_failure("artifact_use_lock_failed", error)),
    };
    if !directory_meta.is_dir() || is_link(&directory_meta) {
        return Err(failure(
            "artifact_path_invalid",
            "artifact lock directory must not be a link",
        ));
    }
    let path = directory.join(name).with_extension("lock");
    match fs::symlink_metadata(&path) {
        Ok(metadata) if !metadata.is_file() || is_link(&metadata) => {
            return Err(failure(
                "artifact_path_invalid",
                "artifact lock must be a regular file",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_failure("artifact_use_lock_failed", error)),
    }
    match OpenOptions::new()
        .read(true)
        .write(create)
        .create(create)
        .truncate(false)
        .open(path)
    {
        Ok(file) => Ok(Some(file)),
        Err(error) if !create && error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_failure("artifact_use_lock_failed", error)),
    }
}

fn failure(code: &'static str, detail: impl ToString) -> ArtifactStoreError {
    ArtifactStoreError::fatal(code, "artifact_material_use", detail.to_string())
}

fn io_failure(code: &'static str, error: std::io::Error) -> ArtifactStoreError {
    failure(code, format!("{:?}: {error}", error.kind())).with_raw_os_error(error.raw_os_error())
}

fn lock_failure(code: &'static str, error: TryLockError) -> ArtifactStoreError {
    match error {
        TryLockError::WouldBlock => failure(code, "WouldBlock: artifact material is in use"),
        TryLockError::Error(error) => io_failure(code, error),
    }
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}
