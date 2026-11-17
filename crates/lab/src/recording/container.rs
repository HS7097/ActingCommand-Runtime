// SPDX-License-Identifier: AGPL-3.0-only

//! The two content container files `record stop` writes (Workflow #336 R6′, frozen model
//! section 4.5): a ZIP of the content-directory layout when the package carries template
//! crops, otherwise one `actingcommand.package.content-json.v1` document. Both are written
//! deterministically from the content table and named `<package_id>.<ext>` (Workflow #336
//! PKG-S5, M-PKG v1.1 §3.5 and D9): a file of that name is replaced in place, never refused.

use super::store::{blocked, invalid, state_io, with_details};
use actingcommand_contract::{LabError, LabResult};
use actingcommand_pack_containment::{CONTENT_JSON_V1, ContentContainer};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{Cursor, ErrorKind, Write};
use std::path::{Path, PathBuf};
use zip::write::{FileOptions, ZipWriter};
use zip::{CompressionMethod, DateTime};

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

/// The file name of a package: `<package_id>.<ext>`.
pub(crate) fn file_name(package_id: &str, kind: ContentContainer) -> String {
    format!("{package_id}.{}", extension(kind))
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
    /// A file of the target's name is there: it is replaced in place (D9).
    pub(crate) replaces: bool,
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

/// #464 P4: the directory that `dir` is or lies inside whose `install\state.json` is a file, the
/// root of an installation (`contracts/install-cli.md`, "Layout": the installer's state file),
/// whose packages a running Runtime reads; looked for on `dir` made absolute and on its nearest
/// existing part with its links resolved. `None` outside every installed root. #464 P4, fix round
/// 2: a child named `install` alone (a folder of installers, matched without case on Windows) is
/// no installation.
fn installed_root(dir: &Path) -> Option<PathBuf> {
    let absolute = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
    let resolved = absolute
        .ancestors()
        .find(|ancestor| ancestor.exists())
        .and_then(|existing| fs::canonicalize(existing).ok());
    absolute
        .ancestors()
        .chain(resolved.iter().flat_map(|path| path.ancestors()))
        .find(|ancestor| ancestor.join("install").join("state.json").is_file())
        .map(Path::to_path_buf)
}

/// Section 4.7 step 2: the directory (or its parent) exists; an existing
/// `<lab-dir>/<package_id>.<ext>` is a file, which is replaced. Reads only. #464 P4: a directory
/// that is or lies inside an installed root (one holding the file `install\state.json`) is
/// refused, reason `inside_installed_root`: a recording's output never lands where an
/// installation reads its packages.
pub(crate) fn preflight_lab_dir(dir: &Path, file_name: &str) -> LabResult<LabDirPlan> {
    if let Some(root) = installed_root(dir) {
        return Err(with_details(
            invalid(
                "record_lab_dir_invalid",
                format!(
                    "--lab-dir {} cannot be used: it lies inside the installed root {}, whose \
                     packages a running installation reads; give a directory outside it and \
                     adopt the package into its standard pack (actinglab standard-pack adopt)",
                    dir.display(),
                    root.display()
                ),
            ),
            json!({
                "lab_dir": dir.display().to_string(),
                "reason": "inside_installed_root",
                "installed_root": root.display().to_string()
            }),
        ));
    }
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
    let replaces = if create_dir {
        false
    } else {
        match fs::symlink_metadata(&target) {
            Err(error) if error.kind() == ErrorKind::NotFound => false,
            Ok(metadata) if metadata.is_file() => true,
            Ok(_) => return Err(lab_dir_invalid(dir, "target_not_a_file")),
            Err(error) => {
                return Err(lab_dir_invalid(dir, &format!("target_unreadable: {error}")));
            }
        }
    };
    Ok(LabDirPlan {
        dir: dir.to_path_buf(),
        target,
        create_dir,
        replaces,
    })
}

fn written_details(mut error: LabError, written: &[String], extra: Value) -> LabError {
    let mut details = error.details.take().unwrap_or_else(|| json!({}));
    if let Some(object) = details.as_object_mut() {
        object.insert("written_files".to_string(), json!(written));
        if let Value::Object(extra) = extra {
            object.extend(extra);
        }
    }
    error.with_details(details)
}

/// Section 4.7 step 4: creates the directory when the plan says so, then replaces
/// `<lab-dir>/<package_id>.<ext>` in place (#464 P4: the replaced file kept, named in `kept`;
/// `stamp` as [`replace_in_place`] takes it).
pub(crate) fn write_lab_dir(
    plan: &LabDirPlan,
    bytes: &[u8],
    stamp: u64,
    written: &mut Vec<String>,
    kept: &mut Vec<String>,
) -> LabResult<bool> {
    if plan.create_dir {
        fs::create_dir(&plan.dir).map_err(|error| {
            written_details(
                state_io(format!("failed to create {}: {error}", plan.dir.display())),
                written,
                json!({"stage": "lab_dir_create"}),
            )
        })?;
    }
    replace_in_place(&plan.target, bytes, stamp, written, kept)
}

/// #464 P4: the name an existing `<name>` is set aside as, `<name>.old-<unix_ms>`: the first
/// millisecond from `stamp` no file of `parent` holds, so that no kept file is ever overwritten.
fn set_aside_name(parent: &Path, name: &str, stamp: u64, written: &[String]) -> LabResult<PathBuf> {
    free_name(parent, name, "old", stamp, written)
}

/// `<name>.<kind>-<unix_ms>` beside `<name>` in `parent`, the first millisecond from `stamp` no entry
/// of `parent` holds (#464 P4's set-aside name; #463 S2-10: and a new directory's part name too).
fn free_name(
    parent: &Path,
    name: &str,
    kind: &str,
    mut stamp: u64,
    written: &[String],
) -> LabResult<PathBuf> {
    loop {
        let candidate = parent.join(format!("{name}.{kind}-{stamp}"));
        match fs::symlink_metadata(&candidate) {
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(candidate),
            Ok(_) => {
                stamp = stamp.checked_add(1).ok_or_else(|| {
                    state_io(format!(
                        "no free name {name}.{kind}-<unix_ms> in {}",
                        parent.display()
                    ))
                })?;
            }
            Err(error) => {
                return Err(written_details(
                    state_io(format!(
                        "failed to inspect {}: {error}",
                        candidate.display()
                    )),
                    written,
                    json!({"stage": "replace_set_aside"}),
                ));
            }
        }
    }
}

/// Workflow #336 PKG-S5 (M-PKG v1.1 §3.5, D9): writes `bytes` beside `target` as
/// `<name>.part`, sets an existing `target` aside as `<name>.old-<unix_ms>` and renames the
/// `.part` file into place; returns whether a file was replaced. #464 P4: the set-aside file is
/// kept, never deleted and never overwritten ([`set_aside_name`]), and named in `kept`. The target
/// name only ever changes by rename, so a reader opens the old file, the new one or none, never
/// part of one. Every file written is pushed onto `written`. Nothing is skipped and every failed
/// step is `record_state_io_failed`: the `.part` file is named; when the new file cannot be put in
/// place the old one is renamed back (`restored`). #463 S2-10: `stamp` is the caller's one clock
/// read, from which the set-aside name's millisecond is sought, so that the outputs one `record
/// stop` sets aside carry one millisecond.
pub(crate) fn replace_in_place(
    target: &Path,
    bytes: &[u8],
    stamp: u64,
    written: &mut Vec<String>,
    kept: &mut Vec<String>,
) -> LabResult<bool> {
    let (parent, name) = match (
        target.parent(),
        target.file_name().and_then(|name| name.to_str()),
    ) {
        (Some(parent), Some(name)) => (parent, name),
        _ => {
            return Err(state_io(format!(
                "{} names no file in a directory",
                target.display()
            )));
        }
    };
    let replaces = match fs::symlink_metadata(target) {
        Err(error) if error.kind() == ErrorKind::NotFound => false,
        Ok(metadata) if metadata.is_file() => true,
        Ok(_) => {
            return Err(written_details(
                state_io(format!(
                    "{} is not a file; it is left untouched",
                    target.display()
                )),
                written,
                json!({"stage": "replace_inspect"}),
            ));
        }
        Err(error) => {
            return Err(written_details(
                state_io(format!("failed to inspect {}: {error}", target.display())),
                written,
                json!({"stage": "replace_inspect"}),
            ));
        }
    };
    let part = parent.join(format!("{name}.part"));
    let part_display = part.display().to_string();
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&part)
        .map_err(|error| {
            written_details(
                state_io(format!("failed to create {part_display}: {error}")),
                written,
                json!({"stage": "replace_write"}),
            )
        })?;
    written.push(part_display.clone());
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| {
            written_details(
                state_io(format!("failed to write {part_display}: {error}")),
                written,
                json!({"stage": "replace_write"}),
            )
        })?;
    drop(file);
    let old = if replaces {
        Some(set_aside_name(parent, name, stamp, written)?)
    } else {
        None
    };
    let old = swap_into_place(target, &part, old, written)?;
    written.retain(|path| *path != part_display);
    written.push(target.display().to_string());
    // #464 P4: the output an earlier stop wrote stays beside the new one.
    kept.extend(old.map(|old| old.display().to_string()));
    Ok(replaces)
}

/// #463 S2-10 (`contracts/lab-recording.md`, "Regression cases written with the package"): writes
/// the directory `target` whole, holding `files` (each a `/`-separated path below it and its
/// bytes), as [`replace_in_place`] writes a file: the new set into a directory of its own beside
/// it, `<name>.part-<unix_ms>` (a name no entry of the parent holds), then an existing `target` set
/// aside as `<name>.old-<unix_ms>` and kept, never deleted, and the new directory renamed to
/// `target`. Returns the directory set aside, when one was. `target`'s name only ever changes by
/// rename, so a reader finds the old set, the new one or none, never part of one. Every failed step
/// is `record_state_io_failed` naming what was written (`written` before it, then the part
/// directory); when the new directory cannot be renamed into place the old one is renamed back
/// (`restored`). Both names' milliseconds are sought from `stamp`, the stop's one clock read, the
/// one its package's [`replace_in_place`] takes: the package and the cases one stop sets aside
/// carry the same `<unix_ms>`.
pub(crate) fn replace_dir_in_place(
    target: &Path,
    files: &[(String, Vec<u8>)],
    stamp: u64,
    written: &[String],
) -> LabResult<Option<String>> {
    let (parent, name) = match (
        target.parent(),
        target.file_name().and_then(|name| name.to_str()),
    ) {
        (Some(parent), Some(name)) => (parent, name),
        _ => {
            return Err(state_io(format!(
                "{} names no directory in a directory",
                target.display()
            )));
        }
    };
    let mut written = written.to_vec();
    let replaces = match fs::symlink_metadata(target) {
        Err(error) if error.kind() == ErrorKind::NotFound => false,
        Ok(metadata) if metadata.is_dir() => true,
        Ok(_) => {
            return Err(written_details(
                state_io(format!(
                    "{} is not a directory; it is left untouched",
                    target.display()
                )),
                &written,
                json!({"stage": "replace_inspect"}),
            ));
        }
        Err(error) => {
            return Err(written_details(
                state_io(format!("failed to inspect {}: {error}", target.display())),
                &written,
                json!({"stage": "replace_inspect"}),
            ));
        }
    };
    let part = free_name(parent, name, "part", stamp, &written)?;
    let part_display = part.display().to_string();
    fs::create_dir(&part).map_err(|error| {
        written_details(
            state_io(format!("failed to create {part_display}: {error}")),
            &written,
            json!({"stage": "replace_write"}),
        )
    })?;
    written.push(part_display);
    for (relative, bytes) in files {
        let file = relative
            .split('/')
            .fold(part.clone(), |path, segment| path.join(segment));
        write_new_file(&file, bytes).map_err(|error| {
            written_details(
                state_io(format!("failed to write {}: {error}", file.display())),
                &written,
                json!({"stage": "replace_write"}),
            )
        })?;
    }
    let old = if replaces {
        Some(set_aside_name(parent, name, stamp, &written)?)
    } else {
        None
    };
    let old = swap_into_place(target, &part, old, &written)?;
    Ok(old.map(|old| old.display().to_string()))
}

/// The swap of [`replace_in_place`] (a file) and [`replace_dir_in_place`] (a directory), one
/// function with two callers (#463 S2-10 fix round 3): an existing `target` is set aside as `old`,
/// the free `<name>.old-<unix_ms>` its caller sought ([`set_aside_name`]), when one is there, and
/// `part`, the new output written beside it, is renamed to `target`; when that rename fails the old
/// one is renamed back (`restored`). Returns `old`. Every failed step is `record_state_io_failed`
/// naming `written`, what was written before it.
fn swap_into_place(
    target: &Path,
    part: &Path,
    old: Option<PathBuf>,
    written: &[String],
) -> LabResult<Option<PathBuf>> {
    if let Some(old) = &old {
        fs::rename(target, old).map_err(|error| {
            written_details(
                state_io(format!(
                    "failed to set {} aside as {} (left in place): {error}",
                    target.display(),
                    old.display()
                )),
                written,
                json!({"stage": "replace_set_aside"}),
            )
        })?;
    }
    if let Err(error) = fs::rename(part, target) {
        let restored = old.as_ref().map(|old| fs::rename(old, target).is_ok());
        let left: Vec<String> = old
            .iter()
            .filter(|_| restored != Some(true))
            .map(|old| old.display().to_string())
            .collect();
        return Err(written_details(
            state_io(format!(
                "failed to rename {} to {}: {error}",
                part.display(),
                target.display()
            )),
            written,
            json!({"stage": "replace_rename", "restored": restored, "left_files": left}),
        ));
    }
    Ok(old)
}

/// A new file `path` holding `bytes`, its directories created first, flushed to the disk.
fn write_new_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory)?;
    }
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}
