// SPDX-License-Identifier: AGPL-3.0-only

//! The two content container files `record stop` writes (Workflow #336 R6′, frozen model
//! section 4.5): a ZIP of the content-directory layout when the package carries template
//! crops, otherwise one `actingcommand.package.content-json.v1` document. Both are written
//! deterministically from the content table, whose `content-directory.v1` digest names the
//! file. Writing follows section 4.7: the copy inside the recording, then `--lab-dir` through
//! a `.part` file; nothing is deleted or overwritten.

use super::store::{
    blocked, hex_sha256, invalid, now_unix_ms, state_io, with_details, write_atomic,
};
use actingcommand_contract::{LabError, LabResult, content_directory_digest};
use actingcommand_pack_containment::{CONTENT_JSON_V1, Containment, ContentContainer};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{Cursor, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use zip::write::{FileOptions, ZipWriter};
use zip::{CompressionMethod, DateTime};

/// How long reading an existing `--lab-dir` file for its digest may take.
const SNAPSHOT_BUDGET: Duration = Duration::from_secs(120);

/// The container of a package: ZIP when it carries a file other than text (the template
/// crops), the single JSON otherwise.
pub(crate) fn container_kind(entries: &BTreeMap<String, Vec<u8>>) -> ContentContainer {
    if entries.keys().any(|path| path.ends_with(".png")) {
        ContentContainer::Zip
    } else {
        ContentContainer::Json
    }
}

pub(crate) fn extension(kind: ContentContainer) -> &'static str {
    match kind {
        ContentContainer::Zip => "zip",
        ContentContainer::Json => "json",
    }
}

/// The `content-directory.v1` digest of a content table.
pub(crate) fn table_digest(entries: &BTreeMap<String, Vec<u8>>) -> String {
    content_directory_digest(entries.iter().map(|(path, bytes)| {
        let hash: [u8; 32] = Sha256::digest(bytes).into();
        (path.as_str(), hash)
    }))
}

fn encode_failed(kind: ContentContainer, error: impl std::fmt::Display) -> LabError {
    with_details(
        blocked(
            "record_artifact_admission_failed",
            format!(
                "the {} container could not be encoded: {error}",
                extension(kind)
            ),
        ),
        json!({"stage": "container_encode", "container": extension(kind)}),
    )
}

/// Encodes the content table. ZIP: no directory entries, `/` paths in byte order, deflate,
/// every timestamp 1980-01-01. JSON: `{"schema_version", "files"}` with the files in path
/// order, each file's bytes as its string. The same table gives the same bytes in one build.
pub(crate) fn encode(
    entries: &BTreeMap<String, Vec<u8>>,
    kind: ContentContainer,
) -> LabResult<Vec<u8>> {
    match kind {
        ContentContainer::Zip => {
            let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
            let options = FileOptions::default()
                .compression_method(CompressionMethod::Deflated)
                .last_modified_time(DateTime::default());
            for (path, bytes) in entries {
                writer
                    .start_file(path.as_str(), options)
                    .map_err(|error| encode_failed(kind, error))?;
                writer
                    .write_all(bytes)
                    .map_err(|error| encode_failed(kind, error))?;
            }
            let cursor = writer
                .finish()
                .map_err(|error| encode_failed(kind, error))?;
            Ok(cursor.into_inner())
        }
        ContentContainer::Json => {
            let mut files = Map::new();
            for (path, bytes) in entries {
                let text = String::from_utf8(bytes.clone())
                    .map_err(|error| encode_failed(kind, format!("{path}: {error}")))?;
                files.insert(path.clone(), Value::String(text));
            }
            let document =
                json!({"schema_version": CONTENT_JSON_V1, "files": Value::Object(files)});
            let mut bytes =
                serde_json::to_vec_pretty(&document).map_err(|error| encode_failed(kind, error))?;
            bytes.push(b'\n');
            Ok(bytes)
        }
    }
}

/// What writing to `--lab-dir` will do, decided before anything is written (section 4.7
/// step 2).
#[derive(Debug, Clone)]
pub(crate) struct LabDirPlan {
    pub(crate) dir: PathBuf,
    pub(crate) target: PathBuf,
    /// The directory does not exist; its parent does, and only it is created.
    pub(crate) create_dir: bool,
    /// The target already holds this content: nothing is written.
    pub(crate) present: bool,
}

fn lab_dir_invalid(dir: &Path, reason: &str) -> LabError {
    with_details(
        invalid(
            "record_lab_dir_invalid",
            format!("--lab-dir {} cannot be used: {reason}", dir.display()),
        ),
        json!({"lab_dir": dir.display().to_string(), "reason": reason}),
    )
}

fn name_conflict(
    target: &Path,
    digest: &str,
    found: Option<String>,
    written: &[String],
) -> LabError {
    with_details(
        blocked(
            "record_artifact_name_conflict",
            format!(
                "{} exists and does not hold the content {digest}; it is left untouched",
                target.display()
            ),
        ),
        json!({
            "path": target.display().to_string(),
            "digest": digest,
            "found": found,
            "written_files": written
        }),
    )
}

/// Section 4.7 step 2: the directory (or its parent) exists, and an existing
/// `<lab-dir>/<D>.<ext>` holds the content `digest`. Reads only.
pub(crate) fn preflight_lab_dir(
    dir: &Path,
    file_name: &str,
    digest: &str,
) -> LabResult<LabDirPlan> {
    let create_dir = match fs::metadata(dir) {
        Ok(metadata) if metadata.is_dir() => false,
        Ok(_) => return Err(lab_dir_invalid(dir, "not_a_directory")),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let parent = dir
                .parent()
                .map(|parent| {
                    if parent.as_os_str().is_empty() {
                        Path::new(".")
                    } else {
                        parent
                    }
                })
                .ok_or_else(|| lab_dir_invalid(dir, "no_parent_directory"))?;
            if !parent.is_dir() {
                return Err(lab_dir_invalid(dir, "parent_directory_missing"));
            }
            true
        }
        Err(error) => {
            return Err(lab_dir_invalid(dir, &format!("unreadable: {error}")));
        }
    };
    let target = dir.join(file_name);
    let present = if create_dir {
        false
    } else {
        match fs::symlink_metadata(&target) {
            Err(error) if error.kind() == ErrorKind::NotFound => false,
            Err(error) => {
                return Err(name_conflict(
                    &target,
                    digest,
                    Some(format!("unreadable: {error}")),
                    &[],
                ));
            }
            Ok(_) => {
                let snapshot = Containment::default()
                    .snapshot_content_directory(&target, Instant::now() + SNAPSHOT_BUDGET);
                match snapshot {
                    Ok(snapshot) if snapshot.reference.sha256 == digest => true,
                    Ok(snapshot) => {
                        return Err(name_conflict(
                            &target,
                            digest,
                            Some(snapshot.reference.sha256),
                            &[],
                        ));
                    }
                    Err(error) => {
                        return Err(name_conflict(
                            &target,
                            digest,
                            Some(format!("unreadable: {error}")),
                            &[],
                        ));
                    }
                }
            }
        }
    };
    Ok(LabDirPlan {
        dir: dir.to_path_buf(),
        target,
        create_dir,
        present,
    })
}

/// Section 4.7 step 3: the copy inside the recording. An existing file with the same bytes is
/// reused (returns `false`); one with other bytes is a conflict and stays untouched.
pub(crate) fn write_recording_copy(path: &Path, bytes: &[u8], digest: &str) -> LabResult<bool> {
    match fs::read(path) {
        Ok(existing) if existing == bytes => Ok(false),
        Ok(existing) => Err(name_conflict(
            path,
            digest,
            Some(format!("sha256 {}", hex_sha256(&existing))),
            &[],
        )),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            write_atomic(path, bytes)?;
            Ok(true)
        }
        Err(error) => Err(state_io(format!(
            "failed to read {}: {error}",
            path.display()
        ))),
    }
}

fn written_details(mut error: LabError, written: &[String]) -> LabError {
    let mut details = error.details.take().unwrap_or_else(|| json!({}));
    if let Some(object) = details.as_object_mut() {
        object.insert("written_files".to_string(), json!(written));
    }
    error.with_details(details)
}

/// Section 4.7 step 4: `<D>.<ext>.part-<unix_ms>` (create_new), then renamed to `<D>.<ext>`.
/// A target that appeared meanwhile is a conflict and the `.part` file stays. `written`
/// collects every file written, for the error details of a later failure.
pub(crate) fn write_lab_dir(
    plan: &LabDirPlan,
    bytes: &[u8],
    digest: &str,
    written: &mut Vec<String>,
) -> LabResult<()> {
    if plan.create_dir {
        fs::create_dir(&plan.dir).map_err(|error| {
            written_details(
                state_io(format!("failed to create {}: {error}", plan.dir.display())),
                written,
            )
        })?;
    }
    let file_name = plan
        .target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| lab_dir_invalid(&plan.dir, "target_name_invalid"))?;
    let part = plan.dir.join(format!("{file_name}.part-{}", now_unix_ms()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&part)
        .map_err(|error| {
            written_details(
                state_io(format!("failed to create {}: {error}", part.display())),
                written,
            )
        })?;
    written.push(part.display().to_string());
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| {
            written_details(
                state_io(format!("failed to write {}: {error}", part.display())),
                written,
            )
        })?;
    drop(file);
    if fs::symlink_metadata(&plan.target).is_ok() {
        return Err(name_conflict(&plan.target, digest, None, written));
    }
    fs::rename(&part, &plan.target).map_err(|error| {
        written_details(
            state_io(format!(
                "failed to rename {} to {} (left in place): {error}",
                part.display(),
                plan.target.display()
            )),
            written,
        )
    })?;
    written.retain(|path| *path != part.display().to_string());
    written.push(plan.target.display().to_string());
    Ok(())
}
