// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #449 REC-ARCH (`contracts/lab-recording.md`, "Archiving"): a recording ended as
//! material only, and reopened later.
//!
//! - `record archive` ([`record_archive`]) ends the instance's active recording whatever its steps
//!   are: it writes a manifest of the step table into the recording's own directory,
//!   `archive-manifest-<n>.json` (`n` one above the highest there, never a file that exists), and
//!   the `record` command marks the session header `archived`. It requires no mark and no last
//!   step without an effect, and never deletes, moves or rewrites a frame or the step table.
//! - Named again by its id, an archived recording gets no second manifest: the latest one is read
//!   and every file it lists is hashed against it.
//! - Fix 1: named by its id, a recording `record start --force` left behind (its directory and
//!   step table intact, the session header another recording's) is archived from its own step
//!   table; its session header, which belongs to the other recording, is left as it is.
//! - `record archive --copy-to` ([`record_archive_copy`]) copies the frames, the step table and the
//!   manifest into a directory that does not exist or is empty, named by an absolute path (fix 1:
//!   an empty, blank or relative one is refused `validation_failed`), checked before anything is
//!   written, and hashes every copy against the manifest. A failure names the file and the reason;
//!   what was copied stays, and the recording stays archived.
//! - `record reopen` ([`record_reopen`]) finds the archived recording the `record` command makes
//!   the instance's active one again by writing its session header; the Lab writes nothing.
//!
//! Nothing here asks the Runtime anything or reads a frame's pixels: files are read, copied and
//! hashed only.

use super::RecordingLock;
use super::model::{
    ArchiveCopyOutcome, ArchiveManifestRef, ArchiveSessionHeader, ClickGesture,
    LAB_RECORDING_ARCHIVE_SCHEMA, LAB_RECORDING_SCHEMA, LabRecording, OpaqueJson,
    RecordArchiveOptions, RecordArchiveOutcome, RecordPoint, RecordRect, RecordReopenOptions,
    RecordReopenOutcome, RecordedFrame, RecordingStep, StepTransition,
};
use super::steps::{lab_unavailable, not_active};
use super::store::{
    LabFile, OldRecordView, RECORDING_FILE, blocked, hex_sha256, invalid, lab_dir, load_lab,
    now_unix_ms, read_old_record, recording_path, state_io, with_details,
};
use crate::codes::LabCode;
use actingcommand_contract::{LabError, LabErrorClass, LabResult};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

/// The session status of an archived recording, in its session header.
pub(crate) const ARCHIVED: &str = "archived";
const ACTIVE: &str = "active";
/// A manifest file is `archive-manifest-<n>.json`.
const MANIFEST_PREFIX: &str = "archive-manifest-";
const MANIFEST_SUFFIX: &str = ".json";
/// The frames' directory inside the recording's directory, as the manifest names its files.
const FRAMES_DIR: &str = "frames";

/// The manifest `record archive` writes (`actingcommand.lab-recording-archive.v1`): a header and
/// one row per step of the step table, in its order. It holds only what the step table and the
/// session header hold, and the sha256 of the step table file it describes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArchiveManifest {
    pub(crate) schema_version: String,
    /// The session header's `record_id`, `instance`, `task_id` and `started_at_unix_ms`, as the
    /// step table carries them: `record start` writes them from the header, and every command holds
    /// the four equal to it (fix 1: for a recording `record start --force` left behind, what its
    /// replaced header held).
    pub(crate) record_id: String,
    pub(crate) instance: String,
    pub(crate) task_id: String,
    /// The game and server the step table names (the session header names no application).
    pub(crate) application: ArchiveApplication,
    pub(crate) started_at_unix_ms: u64,
    pub(crate) archived_at_unix_ms: u64,
    pub(crate) step_count: usize,
    /// The commit of the program that wrote the manifest.
    pub(crate) written_by: String,
    /// The step table file and its sha256 when the manifest was written.
    pub(crate) step_table: ArchiveFile,
    pub(crate) steps: Vec<ArchiveRow>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArchiveApplication {
    pub(crate) game: Option<String>,
    pub(crate) server: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArchiveFile {
    /// Relative to the recording's directory, `/`-separated.
    pub(crate) file: String,
    pub(crate) sha256: String,
}

/// One step of the step table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArchiveRow {
    /// The step's serial number.
    pub(crate) step: u32,
    /// `tap`, `swipe`, `none` (no input), or the step table's own word for any other effect
    /// (`long_press`, `single_touch_drag_with_vertical_brake_v1`, `single_touch_path_drag_v1`,
    /// `application`).
    pub(crate) kind: String,
    /// The step's frames and its page transition's, each file with its sha256.
    pub(crate) frames: Vec<ArchiveFrame>,
    /// The Runtime frames before and after the input, as the step table names them.
    pub(crate) before: Option<ArchiveInputFrame>,
    pub(crate) after: Option<ArchiveInputFrame>,
    /// The page recognised on the frame before the input, and the package that recognised it.
    pub(crate) page: Option<Value>,
    pub(crate) package: Option<Value>,
    pub(crate) tap: Option<ArchiveTap>,
    pub(crate) swipe: Option<ArchiveSwipe>,
    /// When the input was executed; for a step without one, when its first frame was recorded.
    pub(crate) at_unix_ms: Option<u64>,
    pub(crate) request_id: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArchiveFrame {
    pub(crate) frame_id: String,
    pub(crate) role: String,
    /// `frames/<file name>`, relative to the recording's directory.
    pub(crate) file: String,
    pub(crate) sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArchiveInputFrame {
    pub(crate) frame_id: Option<Value>,
    pub(crate) sha256: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArchiveTap {
    pub(crate) rect: RecordRect,
    pub(crate) point: Option<RecordPoint>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArchiveSwipe {
    pub(crate) from_rect: RecordRect,
    pub(crate) to_rect: RecordRect,
    pub(crate) from: Option<RecordPoint>,
    pub(crate) to: Option<RecordPoint>,
    pub(crate) duration_ms: u64,
}

impl ArchiveManifest {
    /// The files of the recording's directory it lists, each once with its sha256: the step
    /// table, then every frame file in the order of the rows.
    fn files(&self) -> Vec<(String, String)> {
        let mut files = vec![(self.step_table.file.clone(), self.step_table.sha256.clone())];
        let mut seen = BTreeSet::new();
        for frame in self.steps.iter().flat_map(|row| &row.frames) {
            if seen.insert(frame.file.clone()) {
                files.push((frame.file.clone(), frame.sha256.clone()));
            }
        }
        files
    }

    /// Every file it lists lies in the recording's directory: the step table, and frames as
    /// `frames/<name>` with a plain name.
    fn files_are_inside(&self) -> bool {
        self.step_table.file == RECORDING_FILE
            && self
                .steps
                .iter()
                .flat_map(|row| &row.frames)
                .all(|frame| frame_file_valid(&frame.file))
    }
}

/// `record archive`: the instance's active recording (or the one `record_id` names) archived, or
/// an archived recording named again; with `copy_to` its target checked first, before anything is
/// written. The copy itself is [`record_archive_copy`], once the `record` command has marked the
/// session header `archived` (`ends_session`). Fix 1: a recording the session header does not name
/// (`archive_unnamed`) is archived from its own step table, its header left as it is.
pub fn record_archive(
    lock: &RecordingLock,
    options: &RecordArchiveOptions,
) -> LabResult<RecordArchiveOutcome> {
    // Fix 1: a copy target that is no absolute path is refused before the session is read.
    let copy_to = options.copy_to.as_deref().map(copy_target).transpose()?;
    let header = current_header(lock)?;
    let record_id = match (&options.record_id, &header) {
        (Some(record_id), _) => record_id.clone(),
        (None, Some(old)) if old.status == ACTIVE => old.record_id.clone(),
        (None, _) => {
            return Err(with_details(
                not_active(format!(
                    "instance {} has no active recording to archive; name an archived one with \
                     --record-id",
                    lock.instance()
                )),
                json!({
                    "instance": lock.instance(),
                    "session_status": header.as_ref().map(|old| old.status.clone())
                }),
            ));
        }
    };
    // Nothing is written before the copy target is known to be usable.
    if let Some(target) = copy_to {
        preflight_copy_target(target)?;
    }
    match header.as_ref().filter(|old| old.record_id == record_id) {
        Some(old) if old.status == ACTIVE => archive_active(lock.state_dir(), old, options),
        Some(old) if old.status == ARCHIVED => archived_again(lock, &record_id),
        Some(old) => Err(not_archived(Some(&record_id), &old.status)),
        None => archive_unnamed(lock, &record_id, header.as_ref(), options),
    }
}

/// The copy of an archived recording into `target` (`record archive --copy-to`): its frames, its
/// step table and its manifest, each hashed against the manifest once all are copied. `target`
/// must be an absolute path (fix 1) and must not exist or must be empty. A file that cannot be
/// copied is `record_state_io_failed`, a copy that does not hold its sha256
/// `record_archive_verify_failed`; both name the file, and nothing is deleted.
pub fn record_archive_copy(
    lock: &RecordingLock,
    archived: &RecordArchiveOutcome,
    target: &str,
) -> LabResult<ArchiveCopyOutcome> {
    let target = copy_target(target)?;
    if archived.instance != lock.instance() {
        return Err(instance_mismatch(&archived.instance, lock.instance()));
    }
    preflight_copy_target(target)?;
    let dir = Path::new(&archived.path);
    let (manifest, _) = read_manifest(Path::new(&archived.manifest.path), &archived.record_id)?;
    let mut files = manifest.files();
    // The manifest's copy is checked against the manifest as it was written or verified.
    files.insert(
        1,
        (
            archived.manifest.file.clone(),
            archived.manifest.sha256.clone(),
        ),
    );
    let frames = target.join(FRAMES_DIR);
    fs::create_dir_all(&frames).map_err(|error| {
        with_details(
            state_io(format!("failed to create {}: {error}", frames.display())),
            json!({"path": display(&frames), "copied": []}),
        )
    })?;
    let mut copied = Vec::new();
    for (file, _) in &files {
        let from = under(dir, file);
        let to = under(target, file);
        fs::copy(&from, &to).map_err(|error| {
            with_details(
                state_io(format!(
                    "failed to copy {} to {}: {error}; nothing was deleted and the recording \
                     stays archived",
                    from.display(),
                    to.display()
                )),
                json!({
                    "file": file,
                    "path": display(&from),
                    "target": display(&to),
                    "reason": "copy_failed",
                    "copied": copied
                }),
            )
        })?;
        copied.push(file.clone());
    }
    let failures = check_files(target, &files);
    if !failures.is_empty() {
        return Err(verify_failed("copy", failures, &copied));
    }
    Ok(ArchiveCopyOutcome {
        path: display(target),
        files: copied,
    })
}

/// `record reopen`: the archived recording of the instance that `record_id` (or the session
/// header) names, which the `record` command makes the active one again. Refused while another
/// recording is active on the instance (`record_session_active`) and for a recording that is not
/// archived (`record_not_archived`). Nothing is written here; the step table and every manifest
/// stay as they are.
pub fn record_reopen(
    lock: &RecordingLock,
    options: &RecordReopenOptions,
) -> LabResult<RecordReopenOutcome> {
    let header = current_header(lock)?;
    let named = options
        .record_id
        .clone()
        .or_else(|| header.as_ref().map(|old| old.record_id.clone()));
    let Some(record_id) = named else {
        return Err(not_archived(None, "missing"));
    };
    if let Some(old) = &header
        && old.status == ACTIVE
    {
        return Err(if old.record_id == record_id {
            not_archived(Some(&record_id), ACTIVE)
        } else {
            with_details(
                blocked(
                    "record_session_active",
                    format!(
                        "recording {} is active on instance {}; stop or archive it before another \
                         is reopened",
                        old.record_id,
                        lock.instance()
                    ),
                ),
                json!({
                    "instance": lock.instance(),
                    "active_record_id": old.record_id,
                    "record_id": record_id
                }),
            )
        });
    }
    let names_it = header
        .as_ref()
        .is_some_and(|old| old.record_id == record_id);
    if let Some(old) = header.as_ref().filter(|_| names_it)
        && old.status != ARCHIVED
    {
        return Err(not_archived(Some(&record_id), &old.status));
    }
    let (recording, dir, manifest) = archived_recording(lock, &record_id)?;
    Ok(RecordReopenOutcome {
        status: "reopened".to_string(),
        task_id: recording.task_id.clone(),
        instance: recording.instance.clone(),
        record_started_at_unix_ms: recording.record_started_at_unix_ms,
        path: display(&dir),
        manifest,
        steps: recording.steps.len(),
        replaced_record_id: header.filter(|_| !names_it).map(|old| old.record_id),
        record_id,
        names_it,
    })
}

/// The session header of the lock's instance, which must name that instance.
fn current_header(lock: &RecordingLock) -> LabResult<Option<OldRecordView>> {
    let header = read_old_record(lock.state_dir(), lock.instance())?;
    if let Some(old) = &header
        && old.instance != lock.instance()
    {
        return Err(instance_mismatch(&old.instance, lock.instance()));
    }
    Ok(header)
}

/// Archives the active recording the session header `old` names: its manifest written, nothing
/// else changed.
fn archive_active(
    state_dir: &Path,
    old: &OldRecordView,
    options: &RecordArchiveOptions,
) -> LabResult<RecordArchiveOutcome> {
    // `load_lab` holds the step table's record id, instance, task id and start equal to the
    // header's.
    let recording = match load_lab(state_dir, old)? {
        LabFile::Ready(recording) => *recording,
        LabFile::Missing => return Err(lab_unavailable("lab_recording_missing")),
        LabFile::Unavailable(reason) => return Err(lab_unavailable(reason)),
    };
    if recording.status != ACTIVE {
        return Err(not_active(format!(
            "the Lab recording {} is {}, not active",
            recording.record_id, recording.status
        )));
    }
    write_archive(state_dir, &recording, &options.written_by, true)
}

/// Fix 1: a recording of the instance that the session header does not name, by its id: one
/// `record start --force` left behind (its directory and step table intact, the header replaced by
/// another recording's), or an archived one whose header a later `record start` replaced. Without
/// a manifest, its manifest is written from its own step table; with one, it is named again. The
/// session header belongs to another recording (or there is none): neither this nor the `record`
/// command writes it, and the answer's `session_header` says so.
fn archive_unnamed(
    lock: &RecordingLock,
    record_id: &str,
    header: Option<&OldRecordView>,
    options: &RecordArchiveOptions,
) -> LabResult<RecordArchiveOutcome> {
    let (recording, dir, manifest) = named_recording(lock, record_id)?;
    let mut archived = match manifest {
        Some(file) => verified_again(&recording, &dir, file)?,
        None => write_archive(lock.state_dir(), &recording, &options.written_by, false)?,
    };
    archived.session_header = Some(ArchiveSessionHeader {
        record_id: header.map(|old| old.record_id.clone()),
        status: header.map(|old| old.status.clone()),
        changed: false,
    });
    Ok(archived)
}

/// Writes the manifest of `recording`, whose step table is active, into its directory; nothing
/// else changes. `ends_session`: the session header names the recording, and the `record`
/// command then marks it `archived`.
fn write_archive(
    state_dir: &Path,
    recording: &LabRecording,
    written_by: &str,
    ends_session: bool,
) -> LabResult<RecordArchiveOutcome> {
    let dir = lab_dir(state_dir, &recording.record_id);
    let table = recording_path(state_dir, &recording.record_id);
    let bytes = fs::read(&table)
        .map_err(|error| state_io(format!("failed to read {}: {error}", table.display())))?;
    let manifest = manifest_of(recording, hex_sha256(&bytes), written_by);
    let manifest = write_manifest(&dir, &manifest)?;
    Ok(RecordArchiveOutcome {
        status: "archived".to_string(),
        record_id: recording.record_id.clone(),
        record_started_at_unix_ms: recording.record_started_at_unix_ms,
        instance: recording.instance.clone(),
        path: display(&dir),
        manifest,
        copy: None,
        session_header: None,
        ends_session,
    })
}

/// An archived recording named again: no second manifest; the latest one is read and every file
/// it lists is hashed against it.
fn archived_again(lock: &RecordingLock, record_id: &str) -> LabResult<RecordArchiveOutcome> {
    let (recording, dir, file) = archived_recording(lock, record_id)?;
    verified_again(&recording, &dir, file)
}

/// The archived `recording`, in `dir`, named again: its latest manifest `file` is read, and every
/// file it lists is hashed against it.
fn verified_again(
    recording: &LabRecording,
    dir: &Path,
    file: String,
) -> LabResult<RecordArchiveOutcome> {
    let record_id = recording.record_id.as_str();
    let path = dir.join(&file);
    let (manifest, bytes) = read_manifest(&path, record_id)?;
    let failures = check_files(dir, &manifest.files());
    if !failures.is_empty() {
        return Err(verify_failed("recording", failures, &[]));
    }
    Ok(RecordArchiveOutcome {
        status: "already_archived".to_string(),
        record_id: record_id.to_owned(),
        record_started_at_unix_ms: recording.record_started_at_unix_ms,
        instance: recording.instance.clone(),
        path: display(dir),
        manifest: ArchiveManifestRef {
            file,
            path: display(&path),
            sha256: hex_sha256(&bytes),
            steps: manifest.steps.len(),
            written: false,
        },
        copy: None,
        session_header: None,
        ends_session: false,
    })
}

/// The archived recording `record_id` of the lock's instance: [`named_recording`] with a manifest
/// (`record_not_archived`, `unarchived`, without one).
fn archived_recording(
    lock: &RecordingLock,
    record_id: &str,
) -> LabResult<(LabRecording, PathBuf, String)> {
    let (recording, dir, manifest) = named_recording(lock, record_id)?;
    let Some(file) = manifest else {
        return Err(not_archived(Some(record_id), "unarchived"));
    };
    Ok((recording, dir, file))
}

/// The recording `record_id` of the lock's instance, read by its id: its step table (still active:
/// an archive never rewrites it, a stop does), its directory and its latest manifest's file name,
/// `None` before it has one.
fn named_recording(
    lock: &RecordingLock,
    record_id: &str,
) -> LabResult<(LabRecording, PathBuf, Option<String>)> {
    let state_dir = lock.state_dir();
    let Some(recording) = read_recording(state_dir, record_id)? else {
        return Err(not_archived(Some(record_id), "missing"));
    };
    if recording.instance != lock.instance() {
        return Err(instance_mismatch(&recording.instance, lock.instance()));
    }
    if recording.status != ACTIVE {
        return Err(not_archived(Some(record_id), &recording.status));
    }
    let dir = lab_dir(state_dir, record_id);
    let manifest = latest_manifest(&dir)?.map(|(_, file)| file);
    Ok((recording, dir, manifest))
}

/// The step table of the recording `record_id`, read by its id; `None` when there is none.
fn read_recording(state_dir: &Path, record_id: &str) -> LabResult<Option<LabRecording>> {
    let path = recording_path(state_dir, record_id);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(state_io(format!(
                "failed to read {}: {error}",
                path.display()
            )));
        }
    };
    let Ok(recording) = serde_json::from_str::<LabRecording>(&text) else {
        return Err(lab_unavailable("lab_recording_unreadable"));
    };
    if recording.schema_version != LAB_RECORDING_SCHEMA || recording.record_id != record_id {
        return Err(lab_unavailable("lab_recording_mismatch"));
    }
    Ok(Some(recording))
}

/// The manifest at `path`, and its bytes; one that does not read as the manifest of `record_id`,
/// or that lists a file outside the recording's directory, is `record_state_io_failed`.
fn read_manifest(path: &Path, record_id: &str) -> LabResult<(ArchiveManifest, Vec<u8>)> {
    let bytes = fs::read(path)
        .map_err(|error| state_io(format!("failed to read {}: {error}", path.display())))?;
    let manifest = serde_json::from_slice::<ArchiveManifest>(&bytes)
        .ok()
        .filter(|manifest| {
            manifest.schema_version == LAB_RECORDING_ARCHIVE_SCHEMA
                && manifest.record_id == record_id
                && manifest.files_are_inside()
        })
        .ok_or_else(|| {
            with_details(
                state_io(format!(
                    "{} does not read as the {LAB_RECORDING_ARCHIVE_SCHEMA} manifest of recording \
                     {record_id}",
                    path.display()
                )),
                json!({"path": display(path), "record_id": record_id}),
            )
        })?;
    Ok((manifest, bytes))
}

/// The number of a manifest file name, `archive-manifest-<n>.json` with `n` at least 1.
fn manifest_number(name: &str) -> Option<u32> {
    name.strip_prefix(MANIFEST_PREFIX)?
        .strip_suffix(MANIFEST_SUFFIX)?
        .parse::<u32>()
        .ok()
        .filter(|number| *number > 0)
}

/// The highest-numbered manifest in the recording's directory `dir`: its number and file name.
fn latest_manifest(dir: &Path) -> LabResult<Option<(u32, String)>> {
    let listing_failed =
        |error: std::io::Error| state_io(format!("failed to list {}: {error}", dir.display()));
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(listing_failed(error)),
    };
    let mut latest: Option<(u32, String)> = None;
    for entry in entries {
        let entry = entry.map_err(&listing_failed)?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if let Some(number) = manifest_number(&name)
            && latest.as_ref().is_none_or(|(highest, _)| number > *highest)
        {
            latest = Some((number, name));
        }
    }
    Ok(latest)
}

/// Writes `manifest` as the next manifest file of `dir`, one never written before (an existing
/// file is never replaced).
fn write_manifest(dir: &Path, manifest: &ArchiveManifest) -> LabResult<ArchiveManifestRef> {
    let number = latest_manifest(dir)?.map_or(1, |(highest, _)| highest.saturating_add(1));
    let file = format!("{MANIFEST_PREFIX}{number}{MANIFEST_SUFFIX}");
    let path = dir.join(&file);
    let mut bytes = serde_json::to_vec_pretty(manifest)
        .map_err(|error| state_io(format!("failed to encode {}: {error}", path.display())))?;
    bytes.push(b'\n');
    let mut handle = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| state_io(format!("failed to create {}: {error}", path.display())))?;
    handle
        .write_all(&bytes)
        .and_then(|()| handle.sync_all())
        .map_err(|error| {
            with_details(
                state_io(format!(
                    "failed to write {} (left in place): {error}",
                    path.display()
                )),
                json!({"written_files": [display(&path)]}),
            )
        })?;
    Ok(ArchiveManifestRef {
        file,
        path: display(&path),
        sha256: hex_sha256(&bytes),
        steps: manifest.steps.len(),
        written: true,
    })
}

/// The manifest of `recording`, whose step table file has the sha256 `step_table_sha256`. Its
/// record id, instance, task id and start are the step table's, which are the session header's
/// (`record start` writes them from the header, and every command holds the four equal to it);
/// fix 1: for a recording `record start --force` left behind, what its replaced header held.
fn manifest_of(
    recording: &LabRecording,
    step_table_sha256: String,
    written_by: &str,
) -> ArchiveManifest {
    ArchiveManifest {
        schema_version: LAB_RECORDING_ARCHIVE_SCHEMA.to_owned(),
        record_id: recording.record_id.clone(),
        instance: recording.instance.clone(),
        task_id: recording.task_id.clone(),
        application: ArchiveApplication {
            game: recording.game.clone(),
            server: recording.server.clone(),
        },
        started_at_unix_ms: recording.record_started_at_unix_ms,
        archived_at_unix_ms: now_unix_ms(),
        step_count: recording.steps.len(),
        written_by: written_by.to_owned(),
        step_table: ArchiveFile {
            file: RECORDING_FILE.to_owned(),
            sha256: step_table_sha256,
        },
        steps: recording.steps.iter().map(row_of).collect(),
    }
}

/// A step's kind: `tap` (a click without a gesture), `swipe` (a drag), `none` (no effect), or the
/// step table's word for another gesture or an application operation.
fn step_kind(step: &RecordingStep) -> &'static str {
    if step.application.is_some() {
        return "application";
    }
    match step.click.as_ref().map(|click| click.gesture) {
        None => "none",
        Some(None) => "tap",
        Some(Some(ClickGesture::Drag { .. })) => "swipe",
        Some(Some(gesture)) => gesture.kind(),
    }
}

/// A member of an opaque value of the step table, when it has one that is not null.
fn member(value: Option<&OpaqueJson>, name: &str) -> Option<Value> {
    value
        .and_then(|value| value.value().get(name))
        .filter(|value| !value.is_null())
        .cloned()
}

/// A Runtime frame the step table names before or after an input (the frame summary of `do`).
fn input_frame(summary: Option<&OpaqueJson>) -> Option<ArchiveInputFrame> {
    summary.map(|summary| ArchiveInputFrame {
        frame_id: member(Some(summary), "frame_id"),
        sha256: member(Some(summary), "frame_sha256"),
    })
}

/// The row of one step, from what the step table holds.
fn row_of(step: &RecordingStep) -> ArchiveRow {
    let click = step.click.as_ref();
    let execution = click.and_then(|click| click.execution.as_ref());
    let before = execution.and_then(|execution| execution.before.as_ref());
    let after = execution.and_then(|execution| execution.after.as_ref());
    let kind = step_kind(step);
    let tap = click.filter(|_| kind == "tap").map(|click| ArchiveTap {
        rect: click.rect,
        point: execution.map(|execution| execution.point),
    });
    let swipe = click.and_then(|click| match click.gesture {
        Some(ClickGesture::Drag { to, duration_ms }) => Some(ArchiveSwipe {
            from_rect: click.rect,
            to_rect: to,
            from: execution.map(|execution| execution.point),
            to: execution.and_then(|execution| execution.to_point),
            duration_ms,
        }),
        _ => None,
    });
    let transition_frames = match &step.transition {
        Some(StepTransition::Page { frames, .. }) => frames.as_slice(),
        _ => &[][..],
    };
    let request_id = execution
        .and_then(|execution| execution.req_id.as_ref())
        .map(|value| value.value().clone())
        .filter(|value| !value.is_null())
        .or_else(|| {
            step.application
                .as_ref()
                .and_then(|application| application.executed.as_ref())
                .map(|executed| Value::String(executed.request_id.clone()))
        });
    ArchiveRow {
        step: step.index,
        kind: kind.to_owned(),
        frames: step
            .frames
            .iter()
            .chain(transition_frames)
            .map(archive_frame)
            .collect(),
        before: input_frame(before),
        after: input_frame(after),
        page: member(before, "page"),
        package: member(
            execution.and_then(|execution| execution.carrier_package.as_ref()),
            "package_label",
        ),
        tap,
        swipe,
        at_unix_ms: execution
            .map(|execution| execution.executed_at_unix_ms)
            .or_else(|| step.frames.first().map(|frame| frame.recorded_at_unix_ms)),
        request_id,
    }
}

fn archive_frame(frame: &RecordedFrame) -> ArchiveFrame {
    // The frame's file name, from its recorded path (content-addressed, `<sha256>.png`).
    let name = frame
        .path
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .map_or_else(|| format!("{}.png", frame.sha256), str::to_owned);
    ArchiveFrame {
        frame_id: frame.frame_id.clone(),
        role: frame.role.clone(),
        file: format!("{FRAMES_DIR}/{name}"),
        sha256: frame.sha256.clone(),
    }
}

/// `frames/<name>`, the name plain: ASCII letters, digits, `.`, `_` and `-`, not starting with a
/// dot.
fn frame_file_valid(file: &str) -> bool {
    file.strip_prefix(FRAMES_DIR)
        .and_then(|rest| rest.strip_prefix('/'))
        .is_some_and(|name| {
            !name.is_empty()
                && !name.starts_with('.')
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
}

/// The file `file` (relative, `/`-separated) under `dir`.
fn under(dir: &Path, file: &str) -> PathBuf {
    file.split('/')
        .fold(dir.to_path_buf(), |path, part| path.join(part))
}

/// Each of `files` (relative to `dir`, with its sha256) that cannot be read or does not hold its
/// sha256, as `{file, path, reason, ...}`.
fn check_files(dir: &Path, files: &[(String, String)]) -> Vec<Value> {
    let mut failures = Vec::new();
    for (file, sha256) in files {
        let path = under(dir, file);
        match fs::read(&path) {
            Ok(bytes) => {
                let actual = hex_sha256(&bytes);
                if actual != *sha256 {
                    failures.push(json!({
                        "file": file,
                        "path": display(&path),
                        "reason": "sha256_mismatch",
                        "expected_sha256": sha256,
                        "actual_sha256": actual
                    }));
                }
            }
            Err(error) => failures.push(json!({
                "file": file,
                "path": display(&path),
                "reason": "unreadable",
                "error": error.to_string()
            })),
        }
    }
    failures
}

/// The files `failures` names do not hold what the manifest gives, in the copy (`checked`
/// `copy`) or in the recording's directory (`recording`).
fn verify_failed(checked: &str, failures: Vec<Value>, copied: &[String]) -> LabError {
    let first = failures.first().cloned().unwrap_or(Value::Null);
    let text = |name: &str| {
        first
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let (file, reason) = (text("file"), text("reason"));
    LabError::new(
        LabErrorClass::RuntimeUnavailable,
        LabCode::RecordArchiveVerifyFailed.as_str(),
        format!(
            "{file} in the {checked} does not hold the sha256 the manifest gives ({reason}); \
             nothing was deleted and the recording stays archived"
        ),
        &["session_record"],
    )
    .with_details(json!({
        "file": file,
        "path": first.get("path"),
        "checked": checked,
        "reason": reason,
        "failures": failures,
        "copied": copied
    }))
}

/// Fix 1: `--copy-to` as a path, which must be absolute: an empty value, one of only white space
/// and a relative path are refused `validation_failed` (`details{path, reason}`, `empty` or
/// `not_absolute`), as the module refuses its other parameters, before anything is read or written
/// for the copy. A relative path would name a place under the current directory of whichever
/// process serves the command, and an empty one that directory itself.
fn copy_target(target: &str) -> LabResult<&Path> {
    let (reason, said) = if target.trim().is_empty() {
        ("empty", "empty")
    } else if Path::new(target).is_absolute() {
        return Ok(Path::new(target));
    } else {
        ("not_absolute", "not an absolute path")
    };
    Err(with_details(
        invalid(
            "validation_failed",
            format!(
                "--copy-to '{target}' is {said}: the copy goes into a directory named by an \
                 absolute path; nothing was written"
            ),
        ),
        json!({"path": target, "reason": reason}),
    ))
}

/// `--copy-to` names a directory that does not exist or is empty; anything else is refused before
/// anything is written.
fn preflight_copy_target(target: &Path) -> LabResult<()> {
    match fs::metadata(target) {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(state_io(format!(
                "failed to read {}: {error}",
                target.display()
            )));
        }
        Ok(metadata) if !metadata.is_dir() => {
            return Err(target_not_empty(target, "not_a_directory"));
        }
        Ok(_) => {}
    }
    let mut entries = fs::read_dir(target)
        .map_err(|error| state_io(format!("failed to list {}: {error}", target.display())))?;
    if entries.next().is_some() {
        return Err(target_not_empty(target, "not_empty"));
    }
    Ok(())
}

fn target_not_empty(target: &Path, reason: &str) -> LabError {
    with_details(
        blocked(
            LabCode::RecordArchiveTargetNotEmpty.as_str(),
            format!(
                "--copy-to {} is {}: the copy goes into a directory that does not exist or is \
                 empty; nothing was written",
                target.display(),
                if reason == "not_empty" {
                    "not empty"
                } else {
                    "no directory"
                }
            ),
        ),
        json!({"path": display(target), "reason": reason}),
    )
}

fn not_archived(record_id: Option<&str>, state: &str) -> LabError {
    let named = record_id.map_or_else(
        || "no recording is named and the instance's session names none".to_owned(),
        |record_id| format!("recording {record_id} is {state}"),
    );
    with_details(
        blocked(
            LabCode::RecordNotArchived.as_str(),
            format!(
                "{named}: it is no archived recording of this instance; record status shows the \
                 instance's recording; nothing was written"
            ),
        ),
        json!({"record_id": record_id, "state": state}),
    )
}

fn instance_mismatch(recorded: &str, command: &str) -> LabError {
    with_details(
        blocked(
            "record_instance_mismatch",
            format!("the recording belongs to instance {recorded}, the command targets {command}"),
        ),
        json!({"record_instance": recorded, "command_instance": command}),
    )
}

/// The absolute form of a path for the answer.
fn display(path: &Path) -> String {
    std::path::absolute(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}
