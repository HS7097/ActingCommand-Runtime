// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #444 NOTICES-3 (register §1.336; `contracts/package-reference.md`, "Notices"): a
//! notices directory on disk, held to the contract crate's rules
//! (`actingcommand_contract::standard_pack_notices`), which this module calls and restates none
//! of: the source directory of `package bundle --notices <directory>` ([`bundle_source`]), read
//! before anything is written, and a game root's `notices/` for `standard-pack check`
//! ([`game_root_notices`]).
//!
//! The directory is listed: every file below it with its `/`-separated path and its length on
//! disk. A link below it, or an entry that is neither a file nor a directory, is refused as the
//! pack loader refuses one in a pack directory (`package_invalid`, naming the entry, with the
//! loader's `content_directory_link_or_type`). Then the listing goes through
//! `check_notice_listing`, the packs of the bundle through `check_notice_pack_ids`, and each file,
//! read through its bound plus one byte, through `check_notice_size` and `check_notice_text`. A
//! broken rule is `standard_pack_root_invalid` with `game`, `standard_pack_problem` the rule's
//! word, `path` the file on disk (the directory itself for the count and the total; for a pack
//! named like the directory, the pack's source directory, with its `package_id`), `json_pointer`
//! empty and, where the rule has one, the bound and the value found (`limit_count` and `count`, or
//! `limit_bytes` and `actual_bytes`). The bytes are kept as read: nothing is normalised.

use crate::StandardPackNotices;
use crate::package_directory::pack_error;
use crate::package_publish::metadata_is_link_or_reparse;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_contract::{
    LabError, LabErrorClass, LabResult, MAX_NOTICE_FILE_BYTES, NoticeBound, NoticeEntry,
    NoticeIssue, NoticeSubject, STANDARD_PACK_NOTICES_DIRECTORY, check_notice_listing,
    check_notice_pack_ids, check_notice_size, check_notice_text,
};
use actingcommand_pack_containment::{ContainmentError, PackContainmentCode};
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

/// One file of a listing: its `/`-separated path below the directory, where it lies, and its
/// length on disk.
type Listed = (String, PathBuf, u64);

/// A notices directory that kept every rule: each file's bytes as read, by its `/`-separated
/// path below the directory, in byte order of the paths.
pub(crate) struct NoticesDirectory {
    files: BTreeMap<String, Vec<u8>>,
}

impl NoticesDirectory {
    /// Each file's path below the directory and its bytes, in byte order of the paths.
    pub(crate) fn files(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.files
            .iter()
            .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
    }

    /// The count of the files and the sum of their lengths.
    pub(crate) fn summary(&self) -> StandardPackNotices {
        StandardPackNotices {
            files: self.files.len(),
            bytes: self.files.values().map(|bytes| bytes.len() as u64).sum(),
        }
    }
}

/// `package bundle --notices <directory>`, for the bundle of `game` whose packs are `packs`
/// (each `package_id` with its source directory, as `package bundle` reads them before anything
/// is written). A path that names no directory, and a directory that holds no file, are refused
/// as a `--loading` that names no file is (`validation_failed`, naming the path); then the
/// directory is held to the rules (module doc).
pub(crate) fn bundle_source(
    directory: &Path,
    game: &str,
    packs: &[(String, PathBuf)],
) -> LabResult<NoticesDirectory> {
    match fs::metadata(directory) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            return Err(LabError::usage(format!(
                "--notices {} is no directory",
                directory.display()
            )));
        }
        Err(error) => return Err(unreadable(directory, &error)),
    }
    let listed = list(directory)?;
    if listed.is_empty() {
        return Err(LabError::usage(format!(
            "--notices {} holds no file",
            directory.display()
        )));
    }
    judge(directory, game, packs, listed)
}

/// `standard-pack check` of the game root `root` of `game`, whose registry lists `package_ids`:
/// the count and the bytes of its directory `notices`, when it holds one and no package of the
/// registry is named like it in any ASCII case (such a folder is that pack's, as the installer's
/// `place` treats it); `None` otherwise. Nothing beside it (`notices.old-<ms>`,
/// `notices.rolledback-<ms>`, a `.part`) is looked at.
pub(crate) fn game_root_notices<'a>(
    root: &Path,
    game: &str,
    package_ids: impl IntoIterator<Item = &'a str>,
) -> LabResult<Option<StandardPackNotices>> {
    if package_ids
        .into_iter()
        .any(|package_id| package_id.eq_ignore_ascii_case(STANDARD_PACK_NOTICES_DIRECTORY))
    {
        return Ok(None);
    }
    let directory = root.join(STANDARD_PACK_NOTICES_DIRECTORY);
    match fs::metadata(&directory) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(unreadable(&directory, &error)),
    }
    let listed = list(&directory)?;
    judge(&directory, game, &[], listed).map(|notices| Some(notices.summary()))
}

/// Every file below `directory`, in byte order of its `/`-separated path, with where it lies and
/// its length on disk. A link below it, or an entry that is neither a file nor a directory, is
/// refused as the pack loader refuses one.
fn list(directory: &Path) -> LabResult<Vec<Listed>> {
    let mut files = Vec::new();
    let mut pending = vec![(directory.to_owned(), String::new())];
    while let Some((current, prefix)) = pending.pop() {
        let entries = fs::read_dir(&current).map_err(|error| unreadable(&current, &error))?;
        for entry in entries {
            let entry = entry.map_err(|error| unreadable(&current, &error))?;
            let path = entry.path();
            let metadata =
                fs::symlink_metadata(&path).map_err(|error| unreadable(&path, &error))?;
            if metadata_is_link_or_reparse(&metadata) || !(metadata.is_dir() || metadata.is_file())
            {
                let code = PackContainmentCode::ContentDirectoryLinkOrType.into();
                return Err(pack_error(&path, ContainmentError::SourceTree { code }));
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let relative = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if metadata.is_dir() {
                pending.push((path, relative));
            } else {
                files.push((relative, path, metadata.len()));
            }
        }
    }
    files.sort();
    Ok(files)
}

/// The listing `listed` of `directory` held to the rules (module doc), each file read through its
/// bound plus one byte; `packs` are the packs of the bundle that carries it.
fn judge(
    directory: &Path,
    game: &str,
    packs: &[(String, PathBuf)],
    listed: Vec<Listed>,
) -> LabResult<NoticesDirectory> {
    let refused =
        |issue: NoticeIssue| notice_invalid(game, &issue_path(directory, packs, &issue), &issue);
    let entries = listed
        .iter()
        .map(|(path, _, bytes)| NoticeEntry {
            path: path.as_str(),
            bytes: *bytes,
        })
        .collect::<Vec<_>>();
    check_notice_listing(&entries).map_err(refused)?;
    check_notice_pack_ids(
        true,
        packs.iter().map(|(package_id, _)| package_id.as_str()),
    )
    .map_err(refused)?;
    let mut files = BTreeMap::new();
    for (path, file, _) in listed {
        let bytes = read_through_bound(&file)?;
        check_notice_size(&path, bytes.len() as u64)
            .and_then(|()| check_notice_text(&path, &bytes))
            .map_err(refused)?;
        files.insert(path, bytes);
    }
    Ok(NoticesDirectory { files })
}

/// The bytes of the file at `path`, at most its bound plus one of them.
fn read_through_bound(path: &Path) -> LabResult<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|file| file.take(MAX_NOTICE_FILE_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|error| unreadable(path, &error))?;
    Ok(bytes)
}

/// Where on disk `issue` lies: the file below `directory`, the directory itself for the count and
/// the total, and the source directory of the pack named like it.
fn issue_path(directory: &Path, packs: &[(String, PathBuf)], issue: &NoticeIssue) -> PathBuf {
    match &issue.subject {
        NoticeSubject::Directory => directory.to_owned(),
        NoticeSubject::Path(path) => path
            .split('/')
            .fold(directory.to_owned(), |parent, segment| parent.join(segment)),
        NoticeSubject::Package(package_id) => packs
            .iter()
            .find(|(listed, _)| listed == package_id)
            .map_or_else(|| directory.to_owned(), |(_, source)| source.clone()),
    }
}

/// `standard_pack_root_invalid` for a broken notices rule, as a root file's problem is answered
/// (`standard_pack_check::root_invalid`), with the rule's `package_id` and bound where it has
/// them.
fn notice_invalid(game: &str, path: &Path, issue: &NoticeIssue) -> LabError {
    let code = SharedCode::StandardPackRootInvalid;
    let mut outcome = Outcome::new(code)
        .game(game)
        .path(path)
        .standard_pack_problem(issue.problem)
        .json_pointer(String::new());
    if let Some(package_id) = issue.package_id() {
        outcome = outcome.package_id(package_id);
    }
    outcome = match issue.bound {
        Some(NoticeBound::Count { limit, found }) => outcome.limit_count(limit).count(found),
        Some(NoticeBound::Bytes { limit, found }) => outcome.limit_bytes(limit).actual_bytes(found),
        None => outcome,
    };
    LabError::new(
        LabErrorClass::UsageValidation,
        code.as_str(),
        format!("{}: {issue}", path.display()),
        &[],
    )
    .with_details(json!({"outcome": outcome.envelope(Detail::Full)}))
}

fn unreadable(path: &Path, error: &io::Error) -> LabError {
    LabError::usage(format!("{}: {error}", path.display()))
}
