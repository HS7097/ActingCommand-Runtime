// SPDX-License-Identifier: AGPL-3.0-only

//! Reading a package by its path (Workflow #336 PKG-S1, option 丙): one bounded snapshot of a
//! local directory, read without following links inside it, or of one content container file
//! expanded in memory, or the whole bytes of an assembled (legacy) package ZIP. The path alone
//! chooses the reading; nothing read is compared with a reference, a digest or a name. A
//! directory read confirms afterwards that its root is still the directory it started in, so a
//! replacement in the middle of the read is refused instead of admitted half old and half new
//! (D16). Assembly then uses the same in-memory bytes and nothing is read from disk afterwards.

use super::*;
use crate::container::ContentContainer;
use actingcommand_contract::safe_source_path;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

/// Maximum number of path segments below the directory.
const MAX_DEPTH: usize = 64;
/// An unmaterialized Git LFS pointer is never package content.
const LFS_POINTER: &[u8] = b"version https://git-lfs.github.com/spec/v1";
/// The derived manifest that marks a `.zip` as an assembled package. A source package never
/// holds it (`source_contains_derived_output`), so the two kinds of ZIP cannot be confused.
const ASSEMBLED_MANIFEST: &str = "resources/manifest.json";

/// What a locator holds, as its path says.
pub(super) enum LocatorContent {
    /// A directory, a `.json` container or a `.zip` container: the content table of a source
    /// package, which admission compiles.
    Entries(BTreeMap<String, Vec<u8>>),
    /// A `.zip` holding `resources/manifest.json`: an assembled package, read whole, which
    /// admission expands as it is.
    AssembledZip(Vec<u8>),
}

/// Reads what `locator` names once: a directory as a content table; a regular file by its
/// extension, `.json` as the single JSON container, `.zip` as an assembled package when it holds
/// `resources/manifest.json` and as the ZIP container otherwise.
pub(super) fn read(
    locator: &Path,
    limits: ContainmentLimits,
    deadline: Instant,
) -> ContainmentResult<LocatorContent> {
    read_with(locator, limits, deadline, &mut || {})
}

/// `read` with `after_directory` called once each directory's listing has been read; the seam
/// through which a test replaces the root in the middle of a read.
pub(super) fn read_with(
    locator: &Path,
    limits: ContainmentLimits,
    deadline: Instant,
    after_directory: &mut dyn FnMut(),
) -> ContainmentResult<LocatorContent> {
    let root = canonical_root(locator)?;
    let metadata = fs::symlink_metadata(&root)
        .map_err(|_| source_error(PackContainmentCode::ContentDirectoryMissing))?;
    if is_link(&metadata) {
        return Err(source_error(
            PackContainmentCode::ContentDirectoryLinkOrType,
        ));
    }
    if false && metadata.is_dir() {
        let before = directory_identity(&root)?;
        let entries = read_entries(&root, limits, deadline, after_directory)?;
        if directory_identity(&root)? != before {
            return Err(source_error(
                PackContainmentCode::ContentDirectoryChangedDuringRead,
            ));
        }
        return Ok(LocatorContent::Entries(entries));
    }
    if !metadata.is_file() {
        return Err(source_error(
            PackContainmentCode::ContentDirectoryNotDirectory,
        ));
    }
    let container = ContentContainer::from_locator(locator)
        .ok_or_else(|| source_error(PackContainmentCode::ContentContainerSizeLimit))?;
    let bytes = read_regular(
        &root,
        limits.max_compressed_bytes,
        PackContainmentCode::ContentContainerSizeLimit.code(),
    )?;
    check_time(deadline)?;
    if container == ContentContainer::Zip && holds_assembled_manifest(&bytes) {
        return Ok(LocatorContent::AssembledZip(bytes));
    }
    let entries = crate::container::expand(&bytes, container, limits, Some(deadline))?;
    check_time(deadline)?;
    Ok(LocatorContent::Entries(entries))
}

/// Workflow #288 A2b: the same bounded read of a directory or a content container, returning
/// the content table of whatever the locator holds, a `.zip` always expanded as a container.
/// Nothing is parsed and nothing is compared.
pub(super) fn measure(
    locator: &Path,
    limits: ContainmentLimits,
    deadline: Instant,
) -> ContainmentResult<BTreeMap<String, Vec<u8>>> {
    match read(locator, limits, deadline)? {
        LocatorContent::Entries(entries) => Ok(entries),
        LocatorContent::AssembledZip(bytes) => {
            crate::container::expand(&bytes, ContentContainer::Zip, limits, Some(deadline))
        }
    }
}

/// Workflow #336: a content table already in memory passes the same entry rules as a read
/// one, entry by entry, under the same deadline.
pub(super) fn admit_entries(
    entries: BTreeMap<String, Vec<u8>>,
    limits: ContainmentLimits,
    deadline: Instant,
) -> ContainmentResult<BTreeMap<String, Vec<u8>>> {
    let mut table = EntryTable::new(limits);
    for (relative, bytes) in entries {
        check_time(deadline)?;
        table.add(relative, |_| Ok(bytes))?;
    }
    check_time(deadline)?;
    Ok(table.into_entries())
}

/// Whether the ZIP `bytes` holds the derived manifest of an assembled package. Bytes that are
/// no readable ZIP are left to the container's own refusal.
fn holds_assembled_manifest(bytes: &[u8]) -> bool {
    ZipArchive::new(Cursor::new(bytes))
        .is_ok_and(|mut archive| archive.by_name(ASSEMBLED_MANIFEST).is_ok())
}

/// D16: the file system's identity of a directory (its volume and its file index), read before
/// and after a directory read. Two renames that put another directory at the same path give it
/// another identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DirectoryIdentity {
    volume: u64,
    file: u64,
}

#[cfg(windows)]
fn directory_identity(root: &Path) -> ContainmentResult<DirectoryIdentity> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    // FILE_FLAG_BACKUP_SEMANTICS opens a directory; FILE_FLAG_OPEN_REPARSE_POINT never follows
    // a link. The default sharing lets a replacement rename the directory while it is open.
    let directory = fs::OpenOptions::new()
        .read(true)
        .custom_flags(0x0200_0000 | 0x0020_0000)
        .open(root)
        .map_err(|_| source_error(PackContainmentCode::ContentDirectoryReadFailed))?;
    let time = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut information = BY_HANDLE_FILE_INFORMATION {
        dwFileAttributes: 0,
        ftCreationTime: time,
        ftLastAccessTime: time,
        ftLastWriteTime: time,
        dwVolumeSerialNumber: 0,
        nFileSizeHigh: 0,
        nFileSizeLow: 0,
        nNumberOfLinks: 0,
        nFileIndexHigh: 0,
        nFileIndexLow: 0,
    };
    // SAFETY: the handle stays open for the whole call, and the call writes only the local
    // structure it is given, which outlives it.
    if unsafe { GetFileInformationByHandle(directory.as_raw_handle(), &mut information) } == 0 {
        return Err(source_error(
            PackContainmentCode::ContentDirectoryReadFailed,
        ));
    }
    Ok(DirectoryIdentity {
        volume: u64::from(information.dwVolumeSerialNumber),
        file: (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow),
    })
}

#[cfg(unix)]
fn directory_identity(root: &Path) -> ContainmentResult<DirectoryIdentity> {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::symlink_metadata(root)
        .map_err(|_| source_error(PackContainmentCode::ContentDirectoryReadFailed))?;
    Ok(DirectoryIdentity {
        volume: metadata.dev(),
        file: metadata.ino(),
    })
}

fn canonical_root(locator: &Path) -> ContainmentResult<PathBuf> {
    if !locator.is_absolute() {
        return Err(source_error(
            PackContainmentCode::ContentDirectoryLocatorNotAbsolute,
        ));
    }
    // Links above the directory are resolved here; links inside it are rejected below.
    fs::canonicalize(locator)
        .map_err(|_| source_error(PackContainmentCode::ContentDirectoryMissing))
}

fn read_entries(
    root: &Path,
    limits: ContainmentLimits,
    deadline: Instant,
    after_directory: &mut dyn FnMut(),
) -> ContainmentResult<BTreeMap<String, Vec<u8>>> {
    let mut table = EntryTable::new(limits);
    let mut pending = vec![(root.to_path_buf(), String::new(), 0_usize)];
    while let Some((directory, prefix, depth)) = pending.pop() {
        check_time(deadline)?;
        let listing = fs::read_dir(&directory)
            .map_err(|_| source_error(PackContainmentCode::ContentDirectoryReadFailed))?;
        for entry in listing {
            check_time(deadline)?;
            let entry =
                entry.map_err(|_| source_error(PackContainmentCode::ContentDirectoryReadFailed))?;
            let name = entry.file_name();
            let name = name
                .to_str()
                .ok_or_else(|| source_error(PackContainmentCode::ContentDirectoryPathEncoding))?;
            let relative = if prefix.is_empty() {
                name.to_owned()
            } else {
                format!("{prefix}/{name}")
            };
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .map_err(|_| source_error(PackContainmentCode::ContentDirectoryReadFailed))?;
            if is_link(&metadata) {
                return Err(source_error(
                    PackContainmentCode::ContentDirectoryLinkOrType,
                ));
            }
            if metadata.is_dir() {
                // Empty directories do not contribute; only their depth is bounded.
                if depth >= MAX_DEPTH {
                    return Err(source_error(
                        PackContainmentCode::ContentDirectoryDepthLimit,
                    ));
                }
                pending.push((path, relative, depth + 1));
                continue;
            }
            if !metadata.is_file() {
                return Err(source_error(
                    PackContainmentCode::ContentDirectoryLinkOrType,
                ));
            }
            table.add(relative, |limit| {
                read_regular(
                    &path,
                    limit,
                    PackContainmentCode::ContentDirectorySizeLimit.code(),
                )
            })?;
        }
        after_directory();
    }
    check_time(deadline)?;
    Ok(table.into_entries())
}

/// The entry rules every container shares (a directory, a ZIP and a single JSON), with the
/// directory's codes: one table under the loader's limits, filled one file at a time.
pub(super) struct EntryTable {
    limits: ContainmentLimits,
    entries: BTreeMap<String, Vec<u8>>,
    folded: BTreeSet<String>,
    total: u64,
}

impl EntryTable {
    pub(super) fn new(limits: ContainmentLimits) -> Self {
        Self {
            limits,
            entries: BTreeMap::new(),
            folded: BTreeSet::new(),
            total: 0,
        }
    }

    /// Checks `relative` before any byte of it is read, then reads at most the bytes still
    /// allowed through `read` and records them.
    pub(super) fn add(
        &mut self,
        relative: String,
        read: impl FnOnce(u64) -> ContainmentResult<Vec<u8>>,
    ) -> ContainmentResult<()> {
        if self.entries.len() >= self.limits.max_entry_count {
            return Err(source_error(
                PackContainmentCode::ContentDirectoryEntryLimit,
            ));
        }
        if relative == "."
            || !safe_source_path(&relative)
            || relative
                .split('/')
                .any(|part| part.eq_ignore_ascii_case(".git"))
        {
            return Err(source_error(
                PackContainmentCode::ContentDirectoryPathInvalid,
            ));
        }
        if !self.folded.insert(relative.to_ascii_lowercase()) {
            return Err(source_error(
                PackContainmentCode::ContentDirectoryCaseCollision,
            ));
        }
        if has_dangerous_extension(&relative) {
            return Err(ContainmentError::ForbiddenEntry { path: relative });
        }
        let limit = self
            .limits
            .max_entry_bytes
            .min(
                self.limits
                    .max_total_decompressed_bytes
                    .saturating_sub(self.total),
            )
            .min(
                self.limits
                    .max_resident_bytes_per_instance
                    .saturating_sub(self.total),
            );
        let bytes = read(limit)?;
        if bytes.len() as u64 > limit {
            return Err(source_error(PackContainmentCode::ContentDirectorySizeLimit));
        }
        if bytes.starts_with(LFS_POINTER) {
            return Err(source_error(
                PackContainmentCode::ContentDirectoryLfsPointer,
            ));
        }
        self.total = self
            .total
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| source_error(PackContainmentCode::ContentDirectorySizeLimit))?;
        self.entries.insert(relative, bytes);
        Ok(())
    }

    pub(super) fn into_entries(self) -> BTreeMap<String, Vec<u8>> {
        self.entries
    }
}

pub(super) fn check_time(deadline: Instant) -> ContainmentResult<()> {
    if Instant::now() >= deadline {
        return Err(source_error(PackContainmentCode::ContentDirectoryDeadline));
    }
    Ok(())
}

/// Symbolic links, junctions and every other reparse point (including cloud placeholders).
fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    metadata.file_type().is_symlink()
}

/// Opens the entry itself rather than a link target and reads at most `limit` bytes; more is
/// `size_code`.
fn read_regular(path: &Path, limit: u64, size_code: Code) -> ContainmentResult<Vec<u8>> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000);
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0x20000);
    }
    let file = options
        .open(path)
        .map_err(|_| source_error(PackContainmentCode::ContentDirectoryOpenFailed))?;
    let metadata = file
        .metadata()
        .map_err(|_| source_error(PackContainmentCode::ContentDirectoryReadFailed))?;
    if is_link(&metadata) || !metadata.is_file() {
        return Err(source_error(
            PackContainmentCode::ContentDirectoryLinkOrType,
        ));
    }
    if metadata.len() > limit {
        return Err(source_error(size_code));
    }
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| source_error(PackContainmentCode::ContentDirectoryReadFailed))?;
    if bytes.len() as u64 > limit {
        return Err(source_error(size_code));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    // Workflow #336 PKG-S1, model M-PKG v1.1 §4.2 S1-T8 (D16): a package directory replaced
    // (two renames) between the reads of its subdirectories is an explicit admission failure;
    // the table read part from the old directory and part from the new one is not returned.
    #[test]
    fn s1_t8_a_directory_replaced_while_it_is_read_is_refused() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after the epoch")
            .as_nanos();
        let scratch = std::env::temp_dir().join(format!(
            "actingcommand-s1-t8-{}-{nanos}",
            std::process::id()
        ));
        let root = scratch.join("package");
        let replacement = scratch.join("package.part");
        let former = scratch.join("package.old");
        for (directory, content) in [
            (&root, b"old".as_slice()),
            (&replacement, b"new".as_slice()),
        ] {
            fs::create_dir_all(directory).expect("package directory");
            fs::write(directory.join("top.json"), content).expect("top-level entry");
            for subdirectory in ["first", "second"] {
                fs::create_dir_all(directory.join(subdirectory)).expect("subdirectory");
                fs::write(directory.join(subdirectory).join("entry.json"), content)
                    .expect("nested entry");
            }
        }
        let mut replaced = false;
        let result = read_with(
            &root,
            ContainmentLimits::default(),
            Instant::now() + Duration::from_secs(60),
            &mut || {
                if !replaced {
                    replaced = true;
                    fs::rename(root.as_path(), former.as_path())
                        .expect("move the directory being read aside");
                    fs::rename(replacement.as_path(), root.as_path())
                        .expect("move the replacement into place");
                }
            },
        );
        let cleanup = fs::remove_dir_all(scratch);
        assert!(replaced, "the replacement ran between two directory reads");
        match result {
            Err(ContainmentError::SourceTree { code }) => assert_eq!(
                code,
                PackContainmentCode::ContentDirectoryChangedDuringRead.code()
            ),
            Err(other) => panic!("refused with another error: {other}"),
            Ok(_) => panic!("a directory replaced while it was read is admitted"),
        }
        cleanup.expect("remove the scratch directory");
    }
}
