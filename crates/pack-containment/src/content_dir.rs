// SPDX-License-Identifier: AGPL-3.0-only

//! `content-directory.v1` admission: one bounded snapshot of a local directory, read without
//! following links inside it, or of one content container file (Workflow #336) expanded in
//! memory. The digest is compared before any entry is parsed; assembly then uses the same
//! in-memory bytes and nothing is read from disk afterwards.

use super::*;
use crate::container::ContentContainer;
use actingcommand_contract::{
    ContentDirectory, ContentDirectoryVersion, content_directory_digest, digest_named,
    digest_named_stem, safe_source_path,
};
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

/// Maximum number of path segments below the directory.
const MAX_DEPTH: usize = 64;
/// An unmaterialized Git LFS pointer is never package content.
const LFS_POINTER: &[u8] = b"version https://git-lfs.github.com/spec/v1";

/// Reads the content `locator` names once and returns the snapshot together with the verified
/// reference, only when its digest equals `expected`.
pub(super) fn snapshot(
    locator: &Path,
    expected: &ContentDirectory,
    limits: ContainmentLimits,
    deadline: Instant,
) -> ContainmentResult<(BTreeMap<String, Vec<u8>>, PackageRef)> {
    let root = canonical_root(locator)?;
    // A digest-form directory name, or container file stem, is part of the declared identity:
    // checked before any read.
    if [locator, root.as_path()]
        .into_iter()
        .flat_map(|path| [digest_named(path), digest_named_stem(path)])
        .flatten()
        .any(|name| name != expected.sha256)
    {
        return Err(source_error("content_directory_name_mismatch"));
    }
    let entries = read_content(locator, root, limits, deadline)?;
    let verified = verify(&entries, expected)?;
    Ok((entries, verified))
}

/// Workflow #288 A2b: the same bounded read without an expected reference or a name
/// comparison, returning the reference of whatever the locator holds. Nothing is parsed.
pub(super) fn measure(
    locator: &Path,
    limits: ContainmentLimits,
    deadline: Instant,
) -> ContainmentResult<(BTreeMap<String, Vec<u8>>, ContentDirectory)> {
    let entries = read_content(locator, canonical_root(locator)?, limits, deadline)?;
    let sha256 = entries_digest(&entries);
    Ok((
        entries,
        ContentDirectory {
            schema_version: ContentDirectoryVersion::V1,
            sha256,
        },
    ))
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

/// Compares the digest of `entries` in constant time with `expected`. Nothing has been parsed
/// yet: a mismatch rejects the raw material as a whole.
pub(super) fn verify(
    entries: &BTreeMap<String, Vec<u8>>,
    expected: &ContentDirectory,
) -> ContainmentResult<PackageRef> {
    let actual = entries_digest(entries);
    let expected_hash = Sha256Hash::parse_hex(&expected.sha256)?;
    let actual_hash = Sha256Hash::parse_hex(&actual)?;
    if !constant_time_hash_eq(&actual_hash, &expected_hash) {
        return Err(ContainmentError::ContentDirectoryDigestMismatch {
            expected: expected_hash,
            actual: actual_hash,
            file_count: entries.len(),
        });
    }
    Ok(PackageRef::ContentDirectory(ContentDirectory {
        schema_version: ContentDirectoryVersion::V1,
        sha256: actual,
    }))
}

fn canonical_root(locator: &Path) -> ContainmentResult<PathBuf> {
    if !locator.is_absolute() {
        return Err(source_error("content_directory_locator_not_absolute"));
    }
    // Links above the directory are resolved here; links inside it are rejected below.
    fs::canonicalize(locator).map_err(|_| source_error("content_directory_missing"))
}

/// Dispatches on what the canonical locator is: a directory is read file by file; a regular
/// file is a content container, read whole and expanded by the extension of `locator`.
fn read_content(
    locator: &Path,
    root: PathBuf,
    limits: ContainmentLimits,
    deadline: Instant,
) -> ContainmentResult<BTreeMap<String, Vec<u8>>> {
    let metadata =
        fs::symlink_metadata(&root).map_err(|_| source_error("content_directory_missing"))?;
    if is_link(&metadata) {
        return Err(source_error("content_directory_link_or_type"));
    }
    if metadata.is_dir() {
        return read_entries(root, limits, deadline);
    }
    if !metadata.is_file() {
        return Err(source_error("content_directory_not_directory"));
    }
    let container = ContentContainer::from_locator(locator)
        .ok_or_else(|| source_error("content_container_unsupported"))?;
    let bytes = read_regular(
        &root,
        limits.max_compressed_bytes,
        "content_container_size_limit",
    )?;
    check_time(deadline)?;
    let entries = crate::container::expand(&bytes, container, limits, Some(deadline))?;
    check_time(deadline)?;
    Ok(entries)
}

fn read_entries(
    root: PathBuf,
    limits: ContainmentLimits,
    deadline: Instant,
) -> ContainmentResult<BTreeMap<String, Vec<u8>>> {
    let mut table = EntryTable::new(limits);
    let mut pending = vec![(root, String::new(), 0_usize)];
    while let Some((directory, prefix, depth)) = pending.pop() {
        check_time(deadline)?;
        for entry in
            fs::read_dir(&directory).map_err(|_| source_error("content_directory_read_failed"))?
        {
            check_time(deadline)?;
            let entry = entry.map_err(|_| source_error("content_directory_read_failed"))?;
            let name = entry.file_name();
            let name = name
                .to_str()
                .ok_or_else(|| source_error("content_directory_path_encoding"))?;
            let relative = if prefix.is_empty() {
                name.to_owned()
            } else {
                format!("{prefix}/{name}")
            };
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .map_err(|_| source_error("content_directory_read_failed"))?;
            if is_link(&metadata) {
                return Err(source_error("content_directory_link_or_type"));
            }
            if metadata.is_dir() {
                // Empty directories do not contribute; only their depth is bounded.
                if depth >= MAX_DEPTH {
                    return Err(source_error("content_directory_depth_limit"));
                }
                pending.push((path, relative, depth + 1));
                continue;
            }
            if !metadata.is_file() {
                return Err(source_error("content_directory_link_or_type"));
            }
            table.add(relative, |limit| {
                read_regular(&path, limit, "content_directory_size_limit")
            })?;
        }
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
            return Err(source_error("content_directory_entry_limit"));
        }
        if relative == "."
            || !safe_source_path(&relative)
            || relative
                .split('/')
                .any(|part| part.eq_ignore_ascii_case(".git"))
        {
            return Err(source_error("content_directory_path_invalid"));
        }
        if !self.folded.insert(relative.to_ascii_lowercase()) {
            return Err(source_error("content_directory_case_collision"));
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
            return Err(source_error("content_directory_size_limit"));
        }
        if bytes.starts_with(LFS_POINTER) {
            return Err(source_error("content_directory_lfs_pointer"));
        }
        self.total = self
            .total
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| source_error("content_directory_size_limit"))?;
        self.entries.insert(relative, bytes);
        Ok(())
    }

    pub(super) fn into_entries(self) -> BTreeMap<String, Vec<u8>> {
        self.entries
    }
}

fn entries_digest(entries: &BTreeMap<String, Vec<u8>>) -> String {
    content_directory_digest(
        entries
            .iter()
            .map(|(path, bytes)| (path.as_str(), *Sha256Hash::digest(bytes).as_bytes())),
    )
}

pub(super) fn check_time(deadline: Instant) -> ContainmentResult<()> {
    if Instant::now() >= deadline {
        return Err(source_error("content_directory_deadline"));
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
fn read_regular(path: &Path, limit: u64, size_code: &'static str) -> ContainmentResult<Vec<u8>> {
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
        .map_err(|_| source_error("content_directory_open_failed"))?;
    let metadata = file
        .metadata()
        .map_err(|_| source_error("content_directory_read_failed"))?;
    if is_link(&metadata) || !metadata.is_file() {
        return Err(source_error("content_directory_link_or_type"));
    }
    if metadata.len() > limit {
        return Err(source_error(size_code));
    }
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| source_error("content_directory_read_failed"))?;
    if bytes.len() as u64 > limit {
        return Err(source_error(size_code));
    }
    Ok(bytes)
}
