// SPDX-License-Identifier: AGPL-3.0-only

use crate::kept::{self, KEPT_DIRECTORY};
use crate::{ArtifactStoreError, ArtifactStoreResult};
use actingcommand_contract::{
    ArtifactEvictionIntentRecord, ArtifactMaterial, ProjectedArtifactReference,
};
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{ErrorKind, Read};
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
        // Workflow #375 R5d: a file another program holds (os error 32 or 33) is busy, like a
        // held lock; any other error fails this guard without stopping the Runtime.
        Err(error) if is_busy_io(&error) => return Ok(None),
        Err(error) => return Err(io_failure("artifact_read_failed", error).non_fatal()),
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
    failure(code, format!("{:?}: {error}", error.kind())).with_io_error(&error)
}

fn lock_failure(code: &'static str, error: TryLockError) -> ArtifactStoreError {
    match error {
        // Workflow #375 R5d: the kind marks a held lock as busy, so a reader retries.
        TryLockError::WouldBlock => failure(code, "WouldBlock: artifact material is in use")
            .with_io_error(&std::io::Error::from(ErrorKind::WouldBlock)),
        TryLockError::Error(error) => io_failure(code, error),
    }
}

/// os error 32 (`ERROR_SHARING_VIOLATION`) or 33 (`ERROR_LOCK_VIOLATION`): another program
/// holds the file right now.
fn is_busy_io(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(32 | 33))
}

/// Workflow #375 R5d: what the frame cleaner did with one frame at its object key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameFileOutcome {
    /// The file was deleted.
    Removed,
    /// The file was renamed into its kept folder.
    Moved,
    /// No file at the object key: deleted by hand, removed or moved before, or never there.
    Absent,
    /// The file or its use lock is held right now (os error 32 or 33, or a held lock); the
    /// cleaner tries again in its next sweep.
    Busy,
}

/// The cleaner's hold on one frame: its use lock, then the file opened for deletion only and
/// shared for deletion only, so no reader or writer opens it until the hold ends. The cleaner
/// never reads or hashes the file.
pub(crate) struct FrameHold {
    path: PathBuf,
    _file: File,
    _lock: File,
}

pub(crate) enum Held {
    Frame(FrameHold),
    Absent,
    Busy,
}

/// Holds the frame at `<root>\<object key>`, and only there.
pub(crate) fn hold_frame(
    root: &Path,
    reference: &ProjectedArtifactReference,
) -> ArtifactStoreResult<Held> {
    let key = reference
        .object_key()
        .ok_or_else(|| failure("artifact_object_key_missing", "object key required").non_fatal())?;
    let path = crate::store::safe_object_path(root, key).map_err(ArtifactStoreError::non_fatal)?;
    // A stat first: an absent frame takes no lock.
    match fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Held::Absent),
        Err(error) if is_busy_io(&error) => return Ok(Held::Busy),
        Err(error) => return Err(io_failure("artifact_cleanup_failed", error).non_fatal()),
    }
    let lock = open_lock(root, reference, true)
        .map_err(ArtifactStoreError::non_fatal)?
        .ok_or_else(|| {
            failure("artifact_use_lock_missing", "delete lock was not created").non_fatal()
        })?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Ok(Held::Busy),
        Err(TryLockError::Error(error)) if is_busy_io(&error) => return Ok(Held::Busy),
        Err(TryLockError::Error(error)) => {
            return Err(io_failure("artifact_use_lock_failed", error).non_fatal());
        }
    }
    match open_for_deletion(&path) {
        Ok(file) => Ok(Held::Frame(FrameHold {
            path,
            _file: file,
            _lock: lock,
        })),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(Held::Absent),
        Err(error) if is_busy_io(&error) => Ok(Held::Busy),
        Err(error) => Err(io_failure("artifact_cleanup_failed", error).non_fatal()),
    }
}

/// Opens the file for deletion only and shares it for deletion only: a program that has it
/// open, or opens it meanwhile, gets os error 32 (Windows).
fn open_for_deletion(path: &Path) -> std::io::Result<File> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        /// The `DELETE` access right.
        const DELETE: u32 = 0x0001_0000;
        /// `FILE_SHARE_DELETE`.
        const FILE_SHARE_DELETE: u32 = 0x0000_0004;
        OpenOptions::new()
            .access_mode(DELETE)
            .share_mode(FILE_SHARE_DELETE)
            .open(path)
    }
    #[cfg(not(windows))]
    {
        OpenOptions::new().read(true).open(path)
    }
}

fn canonical_root(root: &Path) -> ArtifactStoreResult<PathBuf> {
    root.canonicalize()
        .map_err(|error| io_failure("artifact_root_failed", error).non_fatal())
}

/// Workflow #375 R5d: the frame cleaner deletes the frame at `<root>\<object key>`, and never
/// anywhere else. Absent and busy are outcomes; any other failure is a non-fatal error that
/// carries its I/O kind and os error.
pub fn remove_frame_file(
    root: impl AsRef<Path>,
    reference: &ProjectedArtifactReference,
) -> ArtifactStoreResult<FrameFileOutcome> {
    let root = canonical_root(root.as_ref())?;
    let hold = match hold_frame(&root, reference)? {
        Held::Frame(hold) => hold,
        Held::Absent => return Ok(FrameFileOutcome::Absent),
        Held::Busy => return Ok(FrameFileOutcome::Busy),
    };
    let outcome = match fs::remove_file(&hold.path) {
        Ok(()) => Ok(FrameFileOutcome::Removed),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(FrameFileOutcome::Absent),
        Err(error) if is_busy_io(&error) => Ok(FrameFileOutcome::Busy),
        Err(error) => Err(io_failure("artifact_cleanup_failed", error).non_fatal()),
    };
    drop(hold);
    outcome
}

/// Workflow #375 R5d: the frame cleaner renames the frame at `<root>\<object key>` into
/// `<root>\kept\<date>\<leaf>\<file_name>`, creating the folder if needed. The frame goes
/// into this process's kept map before the rename and comes out again if the rename fails, so
/// a read here never misses it. When the rename finds no folder (a leaf deleted by hand right
/// after its creation) and the frame is still there, the folder is created again and the
/// rename tried once more. The caller bumps `kept\.moves` after a move.
pub fn move_frame_file(
    root: impl AsRef<Path>,
    reference: &ProjectedArtifactReference,
    date: &str,
    leaf: &str,
    file_name: &str,
) -> ArtifactStoreResult<FrameFileOutcome> {
    move_frame_file_with(root.as_ref(), reference, [date, leaf, file_name], || {})
}

pub(crate) fn move_frame_file_with(
    root: &Path,
    reference: &ProjectedArtifactReference,
    [date, leaf, file_name]: [&str; 3],
    before_rename: impl FnOnce(),
) -> ArtifactStoreResult<FrameFileOutcome> {
    let root = canonical_root(root)?;
    let key = reference
        .object_key()
        .ok_or_else(|| failure("artifact_object_key_missing", "object key required").non_fatal())?;
    let object_file = Path::new(key)
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .filter(|name| kept::object_id(name).is_some())
        .ok_or_else(|| {
            failure(
                "artifact_path_invalid",
                "only an artifact_<hex>.png frame is kept",
            )
            .non_fatal()
        })?;
    let relative = Path::new(KEPT_DIRECTORY)
        .join(date)
        .join(leaf)
        .join(file_name);
    if relative.components().count() != 4 {
        return Err(failure("artifact_path_invalid", "a kept path has four parts").non_fatal());
    }
    let destination = crate::store::safe_object_path(&root, relative.as_path())
        .map_err(ArtifactStoreError::non_fatal)?;
    let folder = destination
        .parent()
        .ok_or_else(|| failure("artifact_path_invalid", "kept folder missing").non_fatal())?
        .to_path_buf();
    let hold = match hold_frame(&root, reference)? {
        Held::Frame(hold) => hold,
        Held::Absent => return Ok(FrameFileOutcome::Absent),
        Held::Busy => return Ok(FrameFileOutcome::Busy),
    };
    if let Err(error) = fs::create_dir_all(&folder) {
        return if is_busy_io(&error) {
            Ok(FrameFileOutcome::Busy)
        } else {
            Err(io_failure("artifact_directory_failed", error).non_fatal())
        };
    }
    kept::remember(&root, object_file, destination.clone());
    before_rename();
    let mut renamed = fs::rename(&hold.path, &destination);
    if matches!(&renamed, Err(error) if error.kind() == ErrorKind::NotFound)
        && fs::symlink_metadata(&hold.path).is_ok()
    {
        // The folder was deleted between its creation and the rename: once more.
        renamed = fs::create_dir_all(folder.as_path())
            .and_then(|()| fs::rename(&hold.path, destination.as_path()));
    }
    let outcome = match renamed {
        Ok(()) => Ok(FrameFileOutcome::Moved),
        Err(error) => {
            kept::forget(&root, object_file);
            if error.kind() == ErrorKind::NotFound {
                Ok(FrameFileOutcome::Absent)
            } else if is_busy_io(&error) {
                Ok(FrameFileOutcome::Busy)
            } else {
                Err(io_failure("artifact_cleanup_failed", error).non_fatal())
            }
        }
    };
    drop(hold);
    outcome
}

pub(crate) fn is_link(metadata: &fs::Metadata) -> bool {
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
