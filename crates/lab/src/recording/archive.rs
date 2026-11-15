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
//!   what was copied stays, and the recording stays archived. #463 S1-5 fix round 2: the copy's
//!   manifest is written last, once the step table and the frames are copied and checked.
//! - `record reopen` ([`record_reopen`]) finds the archived recording the `record` command makes
//!   the instance's active one again by writing its session header; the Lab writes nothing.
//! - #463 S1-5 ("The manifest"): the manifest written is `actingcommand.lab-recording-archive.v2`,
//!   and only it: each step's row names the frame its effect was on and the frame the screen
//!   became (derived by a rule, and saying so, for an effect recorded before the step table named
//!   them), and `material` is what stage one hands over: each press with the frame it happened on
//!   and where it was pressed, and the final screen ([`record_archive_warnings`] when there is
//!   none). A v1 manifest an earlier build wrote is read (named again, verified, copied as it is,
//!   reopened) and never rewritten. A copy holds the material, or with `--all-frames` every frame,
//!   and its manifest says which.
//!
//! Nothing here asks the Runtime anything or reads a frame's pixels: files are read, copied and
//! hashed only.

use super::RecordingLock;
use super::model::{
    AfterCaptureRecord, ArchiveCopyOutcome, ArchiveManifestRef, ArchiveSessionHeader, ClickGesture,
    LAB_RECORDING_ARCHIVE_SCHEMA, LAB_RECORDING_SCHEMA, LabRecording, OpaqueJson,
    RecordArchiveOptions, RecordArchiveOutcome, RecordPoint, RecordRect, RecordReopenOptions,
    RecordReopenOutcome, RecordSize, RecordedFrame, RecordingStep, StepTransition,
};
use super::steps::{effective_indices, lab_unavailable, live_primary, not_active, step_ref};
use super::store::{
    LabFile, OldRecordView, RECORDING_FILE, blocked, hex_sha256, invalid, lab_dir, load_lab,
    now_unix_ms, read_old_record, recording_path, state_io, with_details,
};
use crate::codes::LabCode;
use actingcommand_contract::{LabError, LabErrorClass, LabResult};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
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
/// #463 S1-5: the schema of the manifest `record archive` writes. [`LAB_RECORDING_ARCHIVE_SCHEMA`]
/// names v1, which builds before this one wrote, and which is read and never written.
const LAB_RECORDING_ARCHIVE_V2_SCHEMA: &str = "actingcommand.lab-recording-archive.v2";
/// #463 S1-5: the warning of `record archive`'s answer (`lab.warnings`) on a manifest that names no
/// final screen.
const FINAL_FRAME_MISSING: &str = "final_frame_missing";
/// #463 S1-1: the source of the click a tap or a long press declares by its point.
const POINT_SOURCE: &str = "point";
/// #463 S1-5: the outcome of an application operation whose completed receipt is on record, and of
/// one with attempts only (it may have run), in the words a click's execution records.
const PERFORMED: &str = "performed";
const INDETERMINATE: &str = "indeterminate";

/// The manifest `record archive` writes (#463 S1-5, `actingcommand.lab-recording-archive.v2`): a
/// header, the material stage one hands over, and one row per step of the step table, in its
/// order. It holds only what the step table and the session header hold (a derived frame id says
/// so), and the sha256 of the step table file it describes.
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
    /// The step table's locale and coordinate space (the size of its frames).
    pub(crate) locale: Option<String>,
    pub(crate) coordinate_space: Option<RecordSize>,
    /// The recording's note and the archive's note; `None` without one.
    pub(crate) note: Option<String>,
    pub(crate) archive_note: Option<String>,
    pub(crate) started_at_unix_ms: u64,
    pub(crate) archived_at_unix_ms: u64,
    pub(crate) step_count: usize,
    /// The commit of the program that wrote the manifest.
    pub(crate) written_by: String,
    /// The step table file and its sha256 when the manifest was written.
    pub(crate) step_table: ArchiveFile,
    /// What stage one hands over.
    pub(crate) material: ArchiveMaterial,
    /// In a copy (`--copy-to`), what it holds of the frames; `None` in the recording's own
    /// directory, which holds every file the manifest lists.
    pub(crate) copy: Option<ArchiveCopy>,
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

/// #463 S1-5: what stage one hands over: the press of each effective step whose effect is on
/// record, in the order of the steps, and the screen the business ended on (`None` when there is
/// none).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArchiveMaterial {
    pub(crate) clicks: Vec<ArchiveClick>,
    pub(crate) final_frame: Option<ArchiveFrameRef>,
}

/// #463 S1-5: one press of the material.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArchiveClick {
    pub(crate) step: u32,
    /// The row's `kind`.
    pub(crate) kind: String,
    /// The frame the press happened on, the row's `before_frame`; `None` when no frame was stored
    /// for it (the application entry step, which has no frame).
    pub(crate) frame: Option<ArchiveFrameRef>,
    /// `frame` was derived by the rule (the row's `derived`).
    pub(crate) derived: bool,
    /// Whether the press was sent on a capture that was not stored (an open step with marks), so
    /// that `frame` is the stored frame the step stood on. Fix round 2: never a claim the step
    /// table does not make (`None`, `null`: "the step table does not say"). The train of stage one
    /// (#463 S1-1's third round joined): a press whose execution names its own frame before
    /// (`derived` false) carries the step table's `sent_on_unstored_capture` (`false` where the
    /// step table leaves it out); a press whose frame is derived, and an application operation,
    /// stay `None`.
    pub(crate) sent_on_unstored_capture: Option<bool>,
    /// The operation opened the application entry step: there was no screen before it.
    pub(crate) entry: bool,
    /// Where it was pressed: the point of a tap or a long press, where a swipe began.
    pub(crate) point: Option<RecordPoint>,
    /// Where a swipe ended.
    pub(crate) to: Option<RecordPoint>,
    /// How long a swipe or a long press lasted.
    pub(crate) duration_ms: Option<u64>,
    /// The action of an application operation, in place of a point.
    pub(crate) action: Option<String>,
    /// The row's `outcome`.
    pub(crate) outcome: String,
}

/// #463 S1-5: a frame some row lists: that row's step, the frame's id, its file and its sha256.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArchiveFrameRef {
    pub(crate) step: u32,
    pub(crate) frame_id: String,
    pub(crate) file: String,
    pub(crate) sha256: String,
}

/// #463 S1-5: the frame after a step's effect: its id and the step of the row that lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArchiveAfterFrame {
    pub(crate) step: Option<u32>,
    pub(crate) frame_id: String,
}

/// #463 S1-5: what a copy of the recording holds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArchiveCopy {
    pub(crate) frames: ArchiveCopyFrames,
}

/// #463 S1-5: the frames `record archive --copy-to` copies, as the copy's manifest names them
/// (`copy.frames`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveCopyFrames {
    /// The frames the manifest's `material` names (the default).
    Material,
    /// Every frame the manifest lists, superseded ones included (`--all-frames`).
    All,
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
    /// #463 S1-5: as the step table has them.
    pub(crate) dropped: bool,
    pub(crate) converted_to_transition: bool,
    /// The step's frames and its page transition's, in the step table's order, each file with its
    /// sha256.
    pub(crate) frames: Vec<ArchiveFrame>,
    /// #463 S1-5: the frame the step's effect was on and the frame after it, as the step table
    /// names them, or for an effect recorded before it named them as the rule gives them
    /// (`derived`); how the frame after it was taken.
    pub(crate) before_frame: Option<String>,
    pub(crate) after_frame: Option<ArchiveAfterFrame>,
    pub(crate) after_capture: Option<AfterCaptureRecord>,
    pub(crate) derived: bool,
    /// #463 S1-5: `performed` or `indeterminate`; `None` without an effect on record.
    pub(crate) outcome: Option<String>,
    /// The effect by its kind (#463 S1-5: the long press and the application operation); none of
    /// the four without an effect or for another gesture.
    pub(crate) tap: Option<ArchiveTap>,
    pub(crate) swipe: Option<ArchiveSwipe>,
    pub(crate) long_press: Option<ArchiveLongPress>,
    pub(crate) application_op: Option<ArchiveApplicationOp>,
    /// The Runtime frames before and after the input, as the step table names them.
    pub(crate) before: Option<ArchiveInputFrame>,
    pub(crate) after: Option<ArchiveInputFrame>,
    /// The page recognised on the frame before the input, and the package that recognised it.
    pub(crate) page: Option<Value>,
    pub(crate) package: Option<Value>,
    /// The step's note; `None` without one.
    pub(crate) note: Option<String>,
    /// When the input was executed; for a step without one, when its first frame was recorded.
    /// #463 S1-5: an application operation's is its completed receipt's time, `None` when the
    /// step table has none (an operation an earlier build recorded, or one without a completed
    /// receipt).
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
    /// #463 S1-5: as the step table has them.
    pub(crate) superseded: bool,
    pub(crate) recorded_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArchiveInputFrame {
    pub(crate) frame_id: Option<Value>,
    pub(crate) sha256: Option<Value>,
}

/// A tap; #463 S1-5: `rect` `None` for a tap declared by its point (`source` `point`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArchiveTap {
    pub(crate) rect: Option<RecordRect>,
    pub(crate) point: Option<RecordPoint>,
    pub(crate) source: String,
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

/// #463 S1-5: a long press; `rect` `None` for one declared by its point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArchiveLongPress {
    pub(crate) point: Option<RecordPoint>,
    pub(crate) duration_ms: u64,
    pub(crate) rect: Option<RecordRect>,
}

/// #463 S1-5: an application operation, and whether its completed receipt is on record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArchiveApplicationOp {
    pub(crate) action: String,
    pub(crate) cli_verb: String,
    pub(crate) executed: bool,
}

impl ArchiveManifest {
    /// The files of the recording's directory it lists, each once with its sha256: the step
    /// table, then every frame file in the order of the rows (the frames `material` names are
    /// among them).
    fn files(&self) -> Vec<(String, String)> {
        listed(
            &self.step_table,
            self.steps
                .iter()
                .flat_map(|row| &row.frames)
                .map(|frame| (frame.file.as_str(), frame.sha256.as_str()))
                .collect(),
        )
    }

    /// #463 S1-5: the files the material names, each once with its sha256: the step table, then
    /// each press's frame and the final frame.
    fn material_files(&self) -> Vec<(String, String)> {
        listed(
            &self.step_table,
            self.material
                .frames()
                .map(|frame| (frame.file.as_str(), frame.sha256.as_str()))
                .collect(),
        )
    }

    /// Every file it lists lies in the recording's directory: the step table, and frames as
    /// `frames/<name>` with a plain name.
    fn files_are_inside(&self) -> bool {
        self.step_table.file == RECORDING_FILE
            && self
                .steps
                .iter()
                .flat_map(|row| &row.frames)
                .map(|frame| frame.file.as_str())
                .chain(self.material.frames().map(|frame| frame.file.as_str()))
                .all(frame_file_valid)
    }
}

impl ArchiveMaterial {
    /// The frames it names: each press's, then the final frame.
    fn frames(&self) -> impl Iterator<Item = &ArchiveFrameRef> {
        self.clicks
            .iter()
            .filter_map(|click| click.frame.as_ref())
            .chain(&self.final_frame)
    }
}

/// The manifest builds before #463 S1-5 wrote (`actingcommand.lab-recording-archive.v1`), read
/// only: its rows, with the step's frames, and the sha256 of the step table file it describes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArchiveManifestV1 {
    pub(crate) schema_version: String,
    pub(crate) record_id: String,
    pub(crate) instance: String,
    pub(crate) task_id: String,
    pub(crate) application: ArchiveApplication,
    pub(crate) started_at_unix_ms: u64,
    pub(crate) archived_at_unix_ms: u64,
    pub(crate) step_count: usize,
    pub(crate) written_by: String,
    pub(crate) step_table: ArchiveFile,
    pub(crate) steps: Vec<ArchiveRowV1>,
}

/// One step of a v1 manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArchiveRowV1 {
    pub(crate) step: u32,
    pub(crate) kind: String,
    pub(crate) frames: Vec<ArchiveFrameV1>,
    pub(crate) before: Option<ArchiveInputFrame>,
    pub(crate) after: Option<ArchiveInputFrame>,
    pub(crate) page: Option<Value>,
    pub(crate) package: Option<Value>,
    pub(crate) tap: Option<ArchiveTapV1>,
    pub(crate) swipe: Option<ArchiveSwipe>,
    pub(crate) at_unix_ms: Option<u64>,
    pub(crate) request_id: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArchiveFrameV1 {
    pub(crate) frame_id: String,
    pub(crate) role: String,
    pub(crate) file: String,
    pub(crate) sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArchiveTapV1 {
    pub(crate) rect: RecordRect,
    pub(crate) point: Option<RecordPoint>,
}

impl ArchiveManifestV1 {
    /// As [`ArchiveManifest::files`].
    fn files(&self) -> Vec<(String, String)> {
        listed(
            &self.step_table,
            self.steps
                .iter()
                .flat_map(|row| &row.frames)
                .map(|frame| (frame.file.as_str(), frame.sha256.as_str()))
                .collect(),
        )
    }

    /// As [`ArchiveManifest::files_are_inside`].
    fn files_are_inside(&self) -> bool {
        self.step_table.file == RECORDING_FILE
            && self
                .steps
                .iter()
                .flat_map(|row| &row.frames)
                .all(|frame| frame_file_valid(&frame.file))
    }
}

/// The schema a manifest file names, read before the manifest itself.
#[derive(Deserialize)]
struct ArchiveSchema {
    schema_version: String,
}

/// #463 S1-5: a manifest read from the recording's directory, of either schema.
struct ReadManifest {
    /// Every file it lists, each once with its sha256 ([`ArchiveManifest::files`]).
    files: Vec<(String, String)>,
    /// How many rows it has.
    steps: usize,
    /// The manifest, when it is of the schema this build writes; `None` for one an earlier build
    /// wrote (v1), which names no material and is never rewritten.
    current: Option<ArchiveManifest>,
}

/// The step table with its sha256, then each of `frames` once (by its file) with its sha256.
fn listed(step_table: &ArchiveFile, frames: Vec<(&str, &str)>) -> Vec<(String, String)> {
    let mut files = vec![(step_table.file.clone(), step_table.sha256.clone())];
    let mut seen = BTreeSet::new();
    for (file, sha256) in frames {
        if seen.insert(file) {
            files.push((file.to_owned(), sha256.to_owned()));
        }
    }
    files
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

/// The copy of an archived recording into `target` (`record archive --copy-to`): its step table
/// and its frames, each hashed against the manifest once all are copied, then its manifest, written
/// last and hashed (#463 S1-5 fix round 2: a copy that fails before the manifest holds none, so no
/// manifest in the target names a file the target does not hold). `target` must be an absolute
/// path (fix 1) and must not exist or must be empty. A file that cannot be copied is
/// `record_state_io_failed`, a copy that does not hold its sha256 `record_archive_verify_failed`;
/// both name the file, and nothing is deleted. #463 S1-5: of a manifest of the schema this build
/// writes, the frames are those its material names ([`ArchiveCopyFrames::Material`]) or every frame
/// it lists ([`ArchiveCopyFrames::All`]), and the copy's manifest is that manifest saying which
/// (`copy.frames`), checked against itself as written; a v1 manifest an earlier build wrote names
/// no material, so its copy holds every file it lists and the manifest byte for byte, as before.
/// The recording's directory keeps every file.
pub fn record_archive_copy(
    lock: &RecordingLock,
    archived: &RecordArchiveOutcome,
    target: &str,
    frames: ArchiveCopyFrames,
) -> LabResult<ArchiveCopyOutcome> {
    let target = copy_target(target)?;
    if archived.instance != lock.instance() {
        return Err(instance_mismatch(&archived.instance, lock.instance()));
    }
    preflight_copy_target(target)?;
    let dir = Path::new(&archived.path);
    let (read, _) = read_manifest(Path::new(&archived.manifest.path), &archived.record_id)?;
    let (files, manifest_copy) = match read.current {
        Some(manifest) => {
            let held = match frames {
                ArchiveCopyFrames::Material => manifest.material_files(),
                ArchiveCopyFrames::All => manifest.files(),
            };
            let in_copy = ArchiveManifest {
                copy: Some(ArchiveCopy { frames }),
                ..manifest
            };
            let bytes = manifest_bytes(&in_copy, &under(target, &archived.manifest.file))?;
            (held, Some(bytes))
        }
        None => (read.files, None),
    };
    let frames_dir = target.join(FRAMES_DIR);
    fs::create_dir_all(&frames_dir).map_err(|error| {
        with_details(
            state_io(format!(
                "failed to create {}: {error}",
                frames_dir.display()
            )),
            json!({"path": display(&frames_dir), "copied": []}),
        )
    })?;
    // #463 S1-5 fix round 2: the step table and the frames first, checked, and only then the
    // manifest, so that a copy that fails before it leaves no manifest in the target.
    let mut copied: Vec<String> = Vec::new();
    for (file, _) in &files {
        let from = under(dir, file);
        let to = under(target, file);
        fs::copy(&from, &to).map_err(|error| copy_failed(file, &from, &to, error, &copied))?;
        copied.push(file.clone());
    }
    let failures = check_files(target, &files);
    if !failures.is_empty() {
        return Err(verify_failed("copy", failures, &copied));
    }
    let manifest_file = archived.manifest.file.as_str();
    let from = under(dir, manifest_file);
    let to = under(target, manifest_file);
    let written = match manifest_copy.as_deref() {
        Some(bytes) => fs::write(&to, bytes),
        None => fs::copy(&from, &to).map(|_bytes| ()),
    };
    written.map_err(|error| copy_failed(manifest_file, &from, &to, error, &copied))?;
    copied.push(manifest_file.to_owned());
    // The manifest's copy is checked against the manifest as it was written or verified, or (#463
    // S1-5) as the copy's own manifest was written.
    let manifest_sha256 = manifest_copy
        .as_deref()
        .map_or_else(|| archived.manifest.sha256.clone(), hex_sha256);
    let failures = check_files(target, &[(manifest_file.to_owned(), manifest_sha256)]);
    if !failures.is_empty() {
        return Err(verify_failed("copy", failures, &copied));
    }
    Ok(ArchiveCopyOutcome {
        path: display(target),
        files: copied,
    })
}

/// #463 S1-5 ("The manifest", `final_frame`): the warnings of `record archive`'s answer
/// (`lab.warnings`) on the manifest `archived` names, written now or named again:
/// `final_frame_missing` when that manifest names no final screen (`material.final_frame` `null`).
/// The archive stands either way. A v1 manifest an earlier build wrote names no material and so
/// claims no final screen: no warning. Nothing is written.
pub fn record_archive_warnings(archived: &RecordArchiveOutcome) -> LabResult<Vec<String>> {
    let (read, _) = read_manifest(Path::new(&archived.manifest.path), &archived.record_id)?;
    Ok(read
        .current
        .filter(|manifest| manifest.material.final_frame.is_none())
        .map(|_| vec![FINAL_FRAME_MISSING.to_owned()])
        .unwrap_or_default())
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
    let failures = check_files(dir, &manifest.files);
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
            steps: manifest.steps,
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

/// The step table of the recording `record_id`, read by its id; `None` when there is none. #463
/// S2-1: `record ready --record-id` reads a recording the session header does not name with it.
/// #463 S2-13: `record frames --record-id` reads its recording so too, whatever its state.
pub(crate) fn read_recording(state_dir: &Path, record_id: &str) -> LabResult<Option<LabRecording>> {
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

/// The manifest at `path`, and its bytes; one that does not read as a manifest of `record_id`, or
/// that lists a file outside the recording's directory, is `record_state_io_failed`. #463 S1-5: a
/// manifest is read by the schema it names, the one this build writes (v2) or the one builds
/// before it wrote (v1), and no other.
fn read_manifest(path: &Path, record_id: &str) -> LabResult<(ReadManifest, Vec<u8>)> {
    let bytes = fs::read(path)
        .map_err(|error| state_io(format!("failed to read {}: {error}", path.display())))?;
    let schema = serde_json::from_slice::<ArchiveSchema>(&bytes)
        .map(|named| named.schema_version)
        .unwrap_or_default();
    let read = if schema == LAB_RECORDING_ARCHIVE_V2_SCHEMA {
        serde_json::from_slice::<ArchiveManifest>(&bytes)
            .ok()
            .filter(|manifest| manifest.record_id == record_id && manifest.files_are_inside())
            .map(|manifest| ReadManifest {
                files: manifest.files(),
                steps: manifest.steps.len(),
                current: Some(manifest),
            })
    } else if schema == LAB_RECORDING_ARCHIVE_SCHEMA {
        serde_json::from_slice::<ArchiveManifestV1>(&bytes)
            .ok()
            .filter(|manifest| manifest.record_id == record_id && manifest.files_are_inside())
            .map(|manifest| ReadManifest {
                files: manifest.files(),
                steps: manifest.steps.len(),
                current: None,
            })
    } else {
        None
    };
    let read = read.ok_or_else(|| {
        with_details(
            state_io(format!(
                "{} does not read as a manifest of recording {record_id} \
                 ({LAB_RECORDING_ARCHIVE_V2_SCHEMA} or {LAB_RECORDING_ARCHIVE_SCHEMA})",
                path.display()
            )),
            json!({"path": display(path), "record_id": record_id}),
        )
    })?;
    Ok((read, bytes))
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
/// #463 S2-1: `record ready` reads whether a recording the session header does not name is
/// archived with it.
pub(crate) fn latest_manifest(dir: &Path) -> LabResult<Option<(u32, String)>> {
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
    let bytes = manifest_bytes(manifest, &path)?;
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

/// The bytes of `manifest` as a manifest file holds them, indented JSON and a newline (`path`
/// names the file in a failure).
fn manifest_bytes(manifest: &ArchiveManifest, path: &Path) -> LabResult<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(manifest)
        .map_err(|error| state_io(format!("failed to encode {}: {error}", path.display())))?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// The manifest of `recording`, whose step table file has the sha256 `step_table_sha256`. Its
/// record id, instance, task id and start are the step table's, which are the session header's
/// (`record start` writes them from the header, and every command holds the four equal to it);
/// fix 1: for a recording `record start --force` left behind, what its replaced header held.
/// #463 S1-5: of the schema v2, with the material.
fn manifest_of(
    recording: &LabRecording,
    step_table_sha256: String,
    written_by: &str,
) -> ArchiveManifest {
    let index = frame_index(recording);
    let steps = recording
        .steps
        .iter()
        .map(|step| row_of(recording, step, &index))
        .collect::<Vec<_>>();
    let clicks = recording
        .steps
        .iter()
        .zip(&steps)
        .filter_map(|(step, row)| click_of(step, row, &index))
        .collect();
    ArchiveManifest {
        schema_version: LAB_RECORDING_ARCHIVE_V2_SCHEMA.to_owned(),
        record_id: recording.record_id.clone(),
        instance: recording.instance.clone(),
        task_id: recording.task_id.clone(),
        application: ArchiveApplication {
            game: recording.game.clone(),
            server: recording.server.clone(),
        },
        locale: recording.locale.clone(),
        coordinate_space: recording.coordinate_space,
        note: None,
        archive_note: None,
        started_at_unix_ms: recording.record_started_at_unix_ms,
        archived_at_unix_ms: now_unix_ms(),
        step_count: recording.steps.len(),
        written_by: written_by.to_owned(),
        step_table: ArchiveFile {
            file: RECORDING_FILE.to_owned(),
            sha256: step_table_sha256,
        },
        material: ArchiveMaterial {
            clicks,
            final_frame: final_frame(recording, &index),
        },
        copy: None,
        steps,
    }
}

/// #463 S1-5: every frame the rows list, by its id: the step of the row that lists it (its own
/// frames and its page transition's) and the frame.
type FrameIndex<'a> = BTreeMap<&'a str, (u32, &'a RecordedFrame)>;

fn frame_index(recording: &LabRecording) -> FrameIndex<'_> {
    recording
        .steps
        .iter()
        .flat_map(|step| {
            row_frames(step).map(move |frame| (frame.frame_id.as_str(), (step.index, frame)))
        })
        .collect()
}

/// A step's frames and its page transition's, in the step table's order: its own as they arrived,
/// then its transition's.
fn row_frames(step: &RecordingStep) -> impl Iterator<Item = &RecordedFrame> {
    let transition: &[RecordedFrame] = match &step.transition {
        Some(StepTransition::Page { frames, .. }) => frames,
        _ => &[],
    };
    step.frames.iter().chain(transition)
}

/// #463 S1-5: the frame `frame_id` with the step of the row that lists it; `None` when no row lists
/// it.
fn frame_ref(index: &FrameIndex<'_>, frame_id: &str) -> Option<ArchiveFrameRef> {
    index.get(frame_id).map(|&(step, frame)| ArchiveFrameRef {
        step,
        frame_id: frame.frame_id.clone(),
        file: frame_file(frame),
        sha256: frame.sha256.clone(),
    })
}

/// #463 S1-5: what a row says of the frames of its step's effect.
#[derive(Default)]
struct Relation {
    before_frame: Option<String>,
    after_frame: Option<String>,
    after_capture: Option<AfterCaptureRecord>,
    derived: bool,
}

/// #463 S1-5 ("The manifest", `before_frame` and `after_frame`): the frames of the step's effect.
/// A press or an application operation recorded with them (#463 S1-1, S1-3) names them in the step
/// table: those, with how the frame after it was taken. An effect on record without them (recorded
/// before the step table named them, or an application operation without a completed receipt)
/// gets the rule's (`derived`): the frame it was on is the step's live primary frame (a step's
/// primary frame cannot change once its effect is on record); the frame after a click is the first
/// frame of the next effective step when that frame was recorded at or after the click was
/// executed, else none; an application operation's receipt recorded before #463 S1-5 carries no
/// time and an attempt none, so the rule finds none after it. The application entry step, which
/// has no frame, and a step without an effect on record name none.
fn relation(recording: &LabRecording, step: &RecordingStep) -> Relation {
    let live = || live_primary(step).map(|frame| frame.frame_id.clone());
    if let Some(execution) = step
        .click
        .as_ref()
        .and_then(|click| click.execution.as_ref())
    {
        return match &execution.before_frame {
            Some(before_frame) => Relation {
                before_frame: Some(before_frame.clone()),
                after_frame: execution.after_frame.clone(),
                after_capture: execution.after_capture.clone(),
                derived: false,
            },
            None => Relation {
                before_frame: live(),
                after_frame: derived_after(recording, step.index, execution.executed_at_unix_ms),
                after_capture: None,
                derived: true,
            },
        };
    }
    match step
        .application
        .as_ref()
        .and_then(|application| application.executed.as_ref())
    {
        Some(executed) if executed.before_frame.is_some() => Relation {
            before_frame: executed.before_frame.clone(),
            after_frame: executed.after_frame.clone(),
            after_capture: executed.after_capture.clone(),
            derived: false,
        },
        _ if outcome_of(step).is_some() && !step.frames.is_empty() => Relation {
            before_frame: live(),
            derived: true,
            ..Relation::default()
        },
        _ => Relation::default(),
    }
}

/// The rule's frame after a click of step `index` executed at `executed_at_unix_ms`: the first
/// frame of the next effective step by serial number, when it was recorded at or after it.
fn derived_after(recording: &LabRecording, index: u32, executed_at_unix_ms: u64) -> Option<String> {
    recording
        .steps
        .iter()
        .filter(|step| step.is_effective() && step.index > index)
        .min_by_key(|step| step.index)?
        .frames
        .first()
        .filter(|frame| frame.recorded_at_unix_ms >= executed_at_unix_ms)
        .map(|frame| frame.frame_id.clone())
}

/// #463 S1-5 ("The manifest", `outcome`): the outcome of the step's effect, when one is on record:
/// a click's execution's, as the step table records it (`performed` or `indeterminate`); an
/// application operation's `performed` with its completed receipt (attempts may stand beside it: it
/// was run again), `indeterminate` with attempts and no completed receipt; `None` otherwise.
fn outcome_of(step: &RecordingStep) -> Option<String> {
    if let Some(execution) = step
        .click
        .as_ref()
        .and_then(|click| click.execution.as_ref())
    {
        return Some(execution.effect.clone());
    }
    let application = step.application.as_ref()?;
    if application.executed.is_some() {
        Some(PERFORMED.to_owned())
    } else if application.attempts.is_empty() {
        None
    } else {
        Some(INDETERMINATE.to_owned())
    }
}

/// #463 S1-5 ("The manifest", `material.clicks`): the press of the material the row of `step`
/// gives, when the step is effective and its effect is on record: where it was pressed, as the
/// step table holds it (a swipe's end only for a swipe), and the frame it happened on.
fn click_of(
    step: &RecordingStep,
    row: &ArchiveRow,
    index: &FrameIndex<'_>,
) -> Option<ArchiveClick> {
    if !step.is_effective() {
        return None;
    }
    let outcome = row.outcome.clone()?;
    let click = step.click.as_ref();
    let execution = click.and_then(|click| click.execution.as_ref());
    let swiped = matches!(
        click.and_then(|click| click.gesture),
        Some(ClickGesture::Drag { .. })
    );
    Some(ArchiveClick {
        step: row.step,
        kind: row.kind.clone(),
        frame: row
            .before_frame
            .as_deref()
            .and_then(|frame_id| frame_ref(index, frame_id)),
        derived: row.derived,
        // The train of stage one (#463 S1-1's third round joined): a press whose execution names
        // its own frame before carries what the step table says (its member, absent when false);
        // a press whose frame the rule derives, and an application operation, whose receipt does
        // not record it, carry `None`.
        sent_on_unstored_capture: execution
            .filter(|_| !row.derived)
            .map(|execution| execution.sent_on_unstored_capture),
        entry: step.is_application_entry(),
        point: execution.map(|execution| execution.point),
        to: execution
            .and_then(|execution| execution.to_point)
            .filter(|_| swiped),
        duration_ms: execution.and_then(|execution| execution.duration_ms),
        action: step
            .application
            .as_ref()
            .map(|application| application.action.clone()),
        outcome,
    })
}

/// #463 S1-5 ("The manifest", `material.final_frame`): the screen the business ended on, the live
/// primary frame of the last effective step when that step has no effect and the effective step
/// before it has its effect on record (the step that effect's screen after opened); `None`
/// otherwise.
fn final_frame(recording: &LabRecording, index: &FrameIndex<'_>) -> Option<ArchiveFrameRef> {
    let effective = effective_indices(recording);
    let [.., previous, last_index] = effective.as_slice() else {
        return None;
    };
    let last = step_ref(recording, *last_index)?;
    let after_an_effect =
        step_ref(recording, *previous).is_some_and(|step| outcome_of(step).is_some());
    if last.click.is_some() || last.application.is_some() || !after_an_effect {
        return None;
    }
    frame_ref(index, &live_primary(last)?.frame_id)
}

/// A step's kind: `tap` (a click without a gesture), `swipe` (a drag), `none` (no effect), or the
/// step table's word for another gesture or an application operation. #463 S2-13: `record frames`
/// names its step's kind so too.
pub(crate) fn step_kind(step: &RecordingStep) -> &'static str {
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

/// The row of one step of `recording`, from what the step table holds; `index` gives the row that
/// lists its frame after.
fn row_of(recording: &LabRecording, step: &RecordingStep, index: &FrameIndex<'_>) -> ArchiveRow {
    let click = step.click.as_ref();
    let execution = click.and_then(|click| click.execution.as_ref());
    let before = execution.and_then(|execution| execution.before.as_ref());
    let after = execution.and_then(|execution| execution.after.as_ref());
    let kind = step_kind(step);
    let point = execution.map(|execution| execution.point);
    // #463 S1-5: a tap or a long press declared by its point has no rectangle of its own.
    let rect = click
        .filter(|click| click.source != POINT_SOURCE)
        .map(|click| click.rect);
    let tap = click.filter(|_| kind == "tap").map(|click| ArchiveTap {
        rect,
        point,
        source: click.source.clone(),
    });
    let swipe = click.and_then(|click| match click.gesture {
        Some(ClickGesture::Drag { to, duration_ms }) => Some(ArchiveSwipe {
            from_rect: click.rect,
            to_rect: to,
            from: point,
            to: execution.and_then(|execution| execution.to_point),
            duration_ms,
        }),
        _ => None,
    });
    let long_press = click.and_then(|click| match click.gesture {
        Some(ClickGesture::LongPress { duration_ms }) => Some(ArchiveLongPress {
            point,
            duration_ms,
            rect,
        }),
        _ => None,
    });
    let application = step.application.as_ref();
    let receipt = application.and_then(|application| application.executed.as_ref());
    let application_op = application.map(|application| ArchiveApplicationOp {
        action: application.action.clone(),
        cli_verb: application.cli_verb.clone(),
        executed: application.executed.is_some(),
    });
    let outcome = outcome_of(step);
    let request_id = execution
        .and_then(|execution| execution.req_id.as_ref())
        .map(|value| value.value().clone())
        .filter(|value| !value.is_null())
        .or_else(|| receipt.map(|completed| Value::String(completed.request_id.clone())));
    // #463 S1-5: an application operation's time is its completed receipt's, absent on one an
    // earlier build recorded and on one without a completed receipt; none is derived.
    let at_unix_ms = match (execution, application) {
        (Some(execution), _) => Some(execution.executed_at_unix_ms),
        (None, Some(_)) if outcome.is_some() => {
            receipt.and_then(|completed| completed.executed_at_unix_ms)
        }
        _ => step.frames.first().map(|frame| frame.recorded_at_unix_ms),
    };
    let effect_frames = relation(recording, step);
    ArchiveRow {
        step: step.index,
        kind: kind.to_owned(),
        dropped: step.dropped,
        converted_to_transition: step.converted_to_transition,
        frames: row_frames(step).map(archive_frame).collect(),
        before_frame: effect_frames.before_frame,
        after_frame: effect_frames.after_frame.map(|frame_id| ArchiveAfterFrame {
            step: index.get(frame_id.as_str()).map(|&(listed, _)| listed),
            frame_id,
        }),
        after_capture: effect_frames.after_capture,
        derived: effect_frames.derived,
        outcome,
        tap,
        swipe,
        long_press,
        application_op,
        before: input_frame(before),
        after: input_frame(after),
        page: member(before, "page"),
        package: member(
            execution.and_then(|execution| execution.carrier_package.as_ref()),
            "package_label",
        ),
        note: None,
        at_unix_ms,
        request_id,
    }
}

/// #463 S2-13 (`record frames`): the file of `frame` in the recording's directory `dir` as the
/// manifest names it (`frames/<name>`), in the absolute form the answers give a path.
pub(crate) fn stored_frame_path(dir: &Path, frame: &RecordedFrame) -> String {
    display(&under(dir, &archive_frame(frame).file))
}

fn archive_frame(frame: &RecordedFrame) -> ArchiveFrame {
    ArchiveFrame {
        frame_id: frame.frame_id.clone(),
        role: frame.role.clone(),
        file: frame_file(frame),
        sha256: frame.sha256.clone(),
        superseded: frame.superseded,
        recorded_at_unix_ms: frame.recorded_at_unix_ms,
    }
}

/// `frames/` and the frame's file name, from its recorded path (content-addressed,
/// `<sha256>.png`).
fn frame_file(frame: &RecordedFrame) -> String {
    let name = frame
        .path
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .map_or_else(|| format!("{}.png", frame.sha256), str::to_owned);
    format!("{FRAMES_DIR}/{name}")
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

/// The copy's file `file` could not be copied (or, for the copy's own manifest, written) from
/// `from` to `to`: `record_state_io_failed`, naming it and the files copied before it (`copied`);
/// nothing is deleted.
fn copy_failed(
    file: &str,
    from: &Path,
    to: &Path,
    error: std::io::Error,
    copied: &[String],
) -> LabError {
    with_details(
        state_io(format!(
            "failed to copy {} to {}: {error}; nothing was deleted and the recording stays \
             archived",
            from.display(),
            to.display()
        )),
        json!({
            "file": file,
            "path": display(from),
            "target": display(to),
            "reason": "copy_failed",
            "copied": copied
        }),
    )
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

/// The recording belongs to another instance than the command's. #463 S2-13: `record frames`
/// refuses so too.
pub(crate) fn instance_mismatch(recorded: &str, command: &str) -> LabError {
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
