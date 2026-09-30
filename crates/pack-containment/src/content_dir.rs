// SPDX-License-Identifier: AGPL-3.0-only

//! `content-directory.v1` admission: one bounded snapshot of a local directory, read without
//! following links inside it. The digest is compared before any entry is parsed; assembly
//! then uses the same in-memory bytes and nothing is read from disk afterwards.

use super::*;
use crate::git_source::source_error;
use actingcommand_contract::{
    ContentDirectory, ContentDirectoryVersion, content_directory_digest, digest_named,
    safe_source_path,
};
use std::fs;
use std::time::Instant;

/// Maximum number of path segments below the directory.
const MAX_DEPTH: usize = 64;
/// An unmaterialized Git LFS pointer is never package content.
const LFS_POINTER: &[u8] = b"version https://git-lfs.github.com/spec/v1";

/// Reads every regular file below `locator` once and returns the snapshot together with the
/// verified reference, only when its digest equals `expected`.
pub(super) fn snapshot(
    locator: &Path,
    expected: &ContentDirectory,
    limits: ContainmentLimits,
    deadline: Instant,
) -> ContainmentResult<(BTreeMap<String, Vec<u8>>, PackageRef)> {
    if !locator.is_absolute() {
        return Err(source_error("content_directory_locator_not_absolute"));
    }
    // Links above the directory are resolved here; links inside it are rejected below.
    let root = fs::canonicalize(locator).map_err(|_| source_error("content_directory_missing"))?;
    // A digest-form directory name is part of the declared identity: checked before any read.
    if [locator, root.as_path()]
        .into_iter()
        .filter_map(digest_named)
        .any(|name| name != expected.sha256)
    {
        return Err(source_error("content_directory_name_mismatch"));
    }
    let metadata =
        fs::symlink_metadata(&root).map_err(|_| source_error("content_directory_missing"))?;
    if is_link(&metadata) {
        return Err(source_error("content_directory_link_or_type"));
    }
    if !metadata.is_dir() {
        return Err(source_error("content_directory_not_directory"));
    }
    let mut entries = BTreeMap::new();
    let mut folded = BTreeSet::new();
    let mut total = 0_u64;
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
            if entries.len() >= limits.max_entry_count {
                return Err(source_error("content_directory_entry_limit"));
            }
            if !safe_source_path(&relative)
                || relative
                    .split('/')
                    .any(|part| part.eq_ignore_ascii_case(".git"))
            {
                return Err(source_error("content_directory_path_invalid"));
            }
            if !folded.insert(relative.to_ascii_lowercase()) {
                return Err(source_error("content_directory_case_collision"));
            }
            if has_dangerous_extension(&relative) {
                return Err(ContainmentError::ForbiddenEntry { path: relative });
            }
            let limit = limits
                .max_entry_bytes
                .min(limits.max_total_decompressed_bytes.saturating_sub(total))
                .min(limits.max_resident_bytes_per_instance.saturating_sub(total));
            let bytes = read_regular(&path, limit)?;
            if bytes.starts_with(LFS_POINTER) {
                return Err(source_error("content_directory_lfs_pointer"));
            }
            total = total
                .checked_add(bytes.len() as u64)
                .ok_or_else(|| source_error("content_directory_size_limit"))?;
            entries.insert(relative, bytes);
        }
    }
    check_time(deadline)?;
    let actual = content_directory_digest(
        entries
            .iter()
            .map(|(path, bytes)| (path.as_str(), *Sha256Hash::digest(bytes).as_bytes())),
    );
    let expected_hash = Sha256Hash::parse_hex(&expected.sha256)?;
    let actual_hash = Sha256Hash::parse_hex(&actual)?;
    // Nothing has been parsed yet: a mismatch rejects the raw material as a whole.
    if !constant_time_hash_eq(&actual_hash, &expected_hash) {
        return Err(ContainmentError::ContentDirectoryDigestMismatch {
            expected: expected_hash,
            actual: actual_hash,
            file_count: entries.len(),
        });
    }
    Ok((
        entries,
        PackageRef::ContentDirectory(ContentDirectory {
            schema_version: ContentDirectoryVersion::V1,
            sha256: actual,
        }),
    ))
}

fn check_time(deadline: Instant) -> ContainmentResult<()> {
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

/// Opens the entry itself rather than a link target and reads at most `limit` bytes.
fn read_regular(path: &Path, limit: u64) -> ContainmentResult<Vec<u8>> {
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
        return Err(source_error("content_directory_size_limit"));
    }
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| source_error("content_directory_read_failed"))?;
    if bytes.len() as u64 > limit {
        return Err(source_error("content_directory_size_limit"));
    }
    Ok(bytes)
}
