// SPDX-License-Identifier: AGPL-3.0-only

//! Steps by serial index, frame arrival, marks, clicks, transitions and the remediation
//! operations. Everything is computed in memory and written once, atomically.

use super::RecordingLock;
use super::boxes::{
    BoxMark, FRAME_CHANGED, RECORDING_FRAME_SOURCE, ScoredFrame, below_because, box_views,
    call_frame, expanded_request, flat_boxes, made_box_marks, press_rect, stage_one_captures,
};
use super::ending::{ENDING_MARK_STEP, check_new_ending};
use super::fold::{fold_of, fold_scan, in_fold, met_on, unfold_scan};
use super::follow::{
    FollowFrames, LocatedClick, check_follow_declaration, check_follow_frames,
    check_located_follow_frames, followed_template,
};
use super::frames::{
    LoadedFrame, check_frame_size, crop_store_path, frame_store_path, read_frame_file,
    read_stored_frame, rect_inside, scaled_to_recording,
};
use super::generate::{page_required, required_ids};
use super::inserted::{optional_after_invalid, recording_active as inserted_recording_active};
use super::marks::{
    EvalFrame, MarkRejection, derive_check, evaluate_mark, prepare_mark, validate_mark_id,
};
use super::model::{
    ApplicationAttempt, BoxBecause, BoxFrameTaken, BoxView, ClickExecution, ClickFollow,
    ClickFollowForm, ClickGesture, ClickSpec, EndingSpec, ForbiddenMarkView,
    LAB_RECORD_MARK_SCHEMA, LAB_RECORDING_DEFAULT_COLOR_MAX_DISTANCE,
    LAB_RECORDING_DEFAULT_MATCH_METRIC, LAB_RECORDING_DEFAULT_SETTLE_MS,
    LAB_RECORDING_DEFAULT_TEMPLATE_THRESHOLD, LAB_RECORDING_MAX_GESTURE_MS, LAB_RECORDING_SCHEMA,
    LabRecording, MarkFamily, MarkRequest, MarkSpec, OpaqueJson, OptionalAfterSpec, RecordPoint,
    RecordRect, RecordSize, RecordedFrame, RecordedMark, RecordingDefaults, RecordingEnding,
    RecordingStep, SelfTestStatus, StepApplication, StepClick, StepOptional, StepSelect,
    StepTransition, ValueTestRight, value_test_side_id,
};
use super::select::{
    OTHER_PAGE_PASSES, SELECT_REJECTION_ID, SELF_TEST_ERROR, SelectChecked, SelectFrame,
    SelectProbe, apply_choice, check_select_rules, check_step_select, clear_select, declare_select,
    probe_marks, select_invalid,
};
use super::store::{
    LabFile, OldRecordView, blocked, create_recording, invalid, lab_dir, load_lab, now_unix_ms,
    read_old_record, read_verified, save_recording, task_id_valid_for_lab, with_details,
    write_content_addressed,
};
use crate::LabOcr;
use crate::codes::{DropComparison, DropProblem, LabCode, OptionalAfterProblem, SelectProblem};
use crate::display_base::{RECORDING_BASE_SIZE, admit_recording_frame};
use crate::recording_api::{change_digest, unchanged_for};
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_contract::{
    InputSamplingEvidence, LabError, LabResult, SEGMENTED_SWIPE_BRAKE_DISTANCE_PX,
};
use actingcommand_execution_kernel::{DriveDraw, path_drag_bounds_hold, sampling_algorithm_name};
use actingcommand_recognition::Rect;
use actingcommand_recognition::color_digest::{
    ColorDigest, FRAME_CHANGE_COLUMNS, FRAME_CHANGE_ROWS, frame_change_digest, frame_change_milli,
};
use actingcommand_recognition_pack::PackRect;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Duration;

pub(crate) const MAX_TRANSITION_MS: u64 = 1_800_000;

/// One recording opened under its lock.
pub(crate) struct Session {
    pub(crate) state_dir: PathBuf,
    pub(crate) lab_dir: PathBuf,
    pub(crate) recording: LabRecording,
    pub(crate) created: bool,
    pub(crate) pending: Vec<(PathBuf, Vec<u8>, String)>,
    pub(crate) cache: BTreeMap<String, LoadedFrame>,
    /// The number of the next frame id (`f0001`, …): every frame admitted in this command
    /// takes and advances it, so frames admitted together never share an id.
    pub(crate) next_frame: u32,
    /// Workflow #390 LAB-2: a stopped recording opened by `record mark` to mark its steps again;
    /// nothing that needs an active recording (a new step, a step operation, an application
    /// operation, a device command) is accepted on it.
    pub(crate) stopped: bool,
    /// Workflow #434 LAB-2: the lock holder's initiator word, written by [`save`] on every step
    /// and mark this command added.
    pub(crate) initiator: String,
}

pub(crate) fn default_recording_defaults() -> RecordingDefaults {
    RecordingDefaults {
        template_threshold: LAB_RECORDING_DEFAULT_TEMPLATE_THRESHOLD,
        color_max_distance: LAB_RECORDING_DEFAULT_COLOR_MAX_DISTANCE,
        match_metric: LAB_RECORDING_DEFAULT_MATCH_METRIC.to_string(),
    }
}

pub(crate) fn not_active(message: impl Into<String>) -> LabError {
    blocked("record_session_not_active", message)
}

pub(crate) fn lab_unavailable(reason: &str) -> LabError {
    with_details(
        blocked(
            "record_lab_unavailable",
            format!("the Lab recording of this session is unavailable: {reason}"),
        ),
        json!({"reason": reason}),
    )
}

/// Workflow #392 DA-5cd fix round 1: `base_size` is the size its header names
/// (`LabRecording::base_size`), the one its first frame must have.
pub(crate) fn new_recording(
    old: &OldRecordView,
    defaults: RecordingDefaults,
    game: Option<String>,
    server: Option<String>,
    locale: Option<String>,
    base_size: Option<RecordSize>,
) -> LabRecording {
    let now = now_unix_ms();
    LabRecording {
        schema_version: LAB_RECORDING_SCHEMA.to_string(),
        record_id: old.record_id.clone(),
        record_started_at_unix_ms: old.started_at_unix_ms,
        task_id: old.task_id.clone(),
        instance: old.instance.clone(),
        status: "active".to_string(),
        game,
        server,
        locale,
        defaults,
        coordinate_space: None,
        created_at_unix_ms: now,
        updated_at_unix_ms: now,
        steps: Vec::new(),
        artifact: None,
        amended_at_unix_ms: None,
        endings: Vec::new(),
        base_size,
    }
}

/// The active recording of the lock's instance; a v0.9.0 session without a Lab recording
/// gets one lazily, with the default settings. Workflow #392 DA-5, the train's fix round 1 (review
/// B): such a recording's header names the base size, as a new recording's does
/// (`RECORDING_BASE_SIZE`, what the installed `record start` writes), so that its first frame is
/// checked against it.
pub(crate) fn open_session(lock: &RecordingLock) -> LabResult<Session> {
    open_session_with(lock, false)
}

/// Workflow #390 LAB-2: `record mark` also opens a stopped recording whose package was
/// generated (`stopped`), to mark its steps again before `record stop` generates it again.
pub(crate) fn open_session_for_marks(lock: &RecordingLock) -> LabResult<Session> {
    open_session_with(lock, true)
}

/// Workflow #390 LAB-2: a change a stopped recording does not take: it needs an active recording.
pub(crate) fn stopped_change(change: &str) -> LabError {
    with_details(
        not_active(format!(
            "the recording is stopped: {change} needs an active recording. A stopped recording \
             takes marks, samples, pages, clicks, transitions and optional steps on its existing \
             steps; record stop then generates its package again"
        )),
        json!({"reason": "recording_stopped", "change": change}),
    )
}

fn open_session_with(lock: &RecordingLock, allow_stopped: bool) -> LabResult<Session> {
    let state_dir = lock.state_dir().to_path_buf();
    let Some(old) = read_old_record(&state_dir, lock.instance())? else {
        return Err(with_details(
            not_active(format!(
                "no recording session exists for {}; run record start first",
                lock.instance()
            )),
            json!({"state_root": state_dir.display().to_string()}),
        ));
    };
    if allow_stopped
        && old.status == "stopped"
        && old.instance == lock.instance()
        && let LabFile::Ready(recording) = load_lab(&state_dir, &old)?
        && recording.status == "stopped"
        && recording.artifact.is_some()
    {
        return Ok(Session {
            lab_dir: lab_dir(&state_dir, &old.record_id),
            next_frame: highest_frame_number(&recording).saturating_add(1),
            state_dir,
            recording: *recording,
            created: false,
            pending: Vec::new(),
            cache: BTreeMap::new(),
            stopped: true,
            initiator: lock.initiator().to_owned(),
        });
    }
    if old.status != "active" {
        return Err(with_details(
            not_active(format!(
                "recording session for {} is {}, not active",
                lock.instance(),
                old.status
            )),
            json!({"state_root": state_dir.display().to_string()}),
        ));
    }
    if old.instance != lock.instance() {
        return Err(blocked(
            "record_instance_mismatch",
            format!(
                "the recording file names instance {}, the command instance is {}",
                old.instance,
                lock.instance()
            ),
        ));
    }
    let (recording, created) = match load_lab(&state_dir, &old)? {
        LabFile::Ready(recording) => (*recording, false),
        LabFile::Unavailable(reason) => return Err(lab_unavailable(reason)),
        LabFile::Missing => {
            if !task_id_valid_for_lab(&old.task_id) {
                return Err(lab_unavailable("task_id_invalid_for_lab_package"));
            }
            (
                new_recording(
                    &old,
                    default_recording_defaults(),
                    None,
                    None,
                    None,
                    Some(RECORDING_BASE_SIZE),
                ),
                true,
            )
        }
    };
    if recording.status != "active" {
        return Err(not_active(format!(
            "the Lab recording {} is {}, not active",
            recording.record_id, recording.status
        )));
    }
    Ok(Session {
        lab_dir: lab_dir(&state_dir, &old.record_id),
        next_frame: highest_frame_number(&recording).saturating_add(1),
        state_dir,
        recording,
        created,
        pending: Vec::new(),
        cache: BTreeMap::new(),
        stopped: false,
        initiator: lock.initiator().to_owned(),
    })
}

/// Writes the pending frames and crops, then the recording. A failure names what was
/// already written; nothing is deleted. Workflow #434 LAB-2: every step this command added (one
/// whose `initiator` is still empty) is written with the lock holder's initiator word; those of
/// earlier commands keep theirs. A mark carries the word from when it is made (`add_marks`, fix 2).
pub(crate) fn save(session: &mut Session) -> LabResult<Vec<String>> {
    let initiator = session.initiator.clone();
    for step in &mut session.recording.steps {
        if step.initiator.is_empty() {
            step.initiator.clone_from(&initiator);
        }
    }
    let mut written = Vec::new();
    let pending = std::mem::take(&mut session.pending);
    for (path, bytes, sha256) in &pending {
        match write_content_addressed(path, bytes, sha256) {
            Ok(true) => written.push(path.display().to_string()),
            Ok(false) => {}
            Err(error) => return Err(attach_written(error, &written)),
        }
    }
    session.recording.updated_at_unix_ms = now_unix_ms();
    let result = if session.created {
        create_recording(&session.state_dir, &session.recording)
    } else {
        save_recording(&session.state_dir, &session.recording)
    };
    match result {
        Ok(path) => {
            session.created = false;
            written.push(path.display().to_string());
            Ok(written)
        }
        Err(error) => Err(attach_written(error, &written)),
    }
}

fn attach_written(mut error: LabError, written: &[String]) -> LabError {
    let mut details = error.details.take().unwrap_or_else(|| json!({}));
    if let Some(object) = details.as_object_mut() {
        object.insert("written_files".to_string(), json!(written));
    }
    error.with_details(details)
}

/// #463 R4: the steps in the order a run meets them: the steps recorded on the path by their serial
/// numbers, each followed by the steps a stopped recording inserted after it (`inserted_after`), by
/// theirs. A recording without an inserted step is in serial order. An inserted step whose
/// recorded step is not there comes last.
pub(crate) fn path_order(recording: &LabRecording) -> Vec<&RecordingStep> {
    let mut recorded = recording
        .steps
        .iter()
        .filter(|step| step.inserted_after.is_none())
        .collect::<Vec<_>>();
    recorded.sort_by_key(|step| step.index);
    let mut inserted = recording
        .steps
        .iter()
        .filter(|step| step.inserted_after.is_some())
        .collect::<Vec<_>>();
    inserted.sort_by_key(|step| step.index);
    let mut ordered = Vec::with_capacity(recording.steps.len());
    for step in recorded {
        ordered.push(step);
        ordered.extend(
            inserted
                .iter()
                .copied()
                .filter(|other| other.inserted_after == Some(step.index)),
        );
    }
    for step in inserted {
        if !ordered.iter().any(|placed| placed.index == step.index) {
            ordered.push(step);
        }
    }
    ordered
}

/// The effective steps' serial numbers in the order a run meets them (#463 R4, [`path_order`]).
pub(crate) fn effective_indices(recording: &LabRecording) -> Vec<u32> {
    path_order(recording)
        .into_iter()
        .filter(|step| step.is_effective())
        .map(|step| step.index)
        .collect()
}

pub(crate) fn last_effective(recording: &LabRecording) -> Option<u32> {
    effective_indices(recording).last().copied()
}

/// The last effective step while it is open.
pub(crate) fn open_step(recording: &LabRecording) -> Option<u32> {
    let index = last_effective(recording)?;
    step_ref(recording, index)
        .filter(|step| !step.closed)
        .map(|step| step.index)
}

pub(crate) fn step_ref(recording: &LabRecording, index: u32) -> Option<&RecordingStep> {
    recording.steps.iter().find(|step| step.index == index)
}

fn step_mut(recording: &mut LabRecording, index: u32) -> LabResult<&mut RecordingStep> {
    recording
        .steps
        .iter_mut()
        .find(|step| step.index == index)
        .ok_or_else(|| step_not_found(index))
}

fn step_not_found(index: u32) -> LabError {
    blocked(
        "record_step_not_found",
        format!("recording step {index} does not exist or is dropped or converted"),
    )
}

/// The step `index` while it is effective; `record_step_not_found` for a step that does not exist,
/// is dropped or was converted into a transition. #463 S2-13: `record frames --step` addresses its
/// step so too (#463 S2-8': and a folded swipe). #463 S2-8': a swipe folded into a press step's
/// scrolling search is no effective step and answers the fold's own word, `record_scan_invalid`
/// with `step_in_fold`.
pub(crate) fn effective_step(recording: &LabRecording, index: u32) -> LabResult<&RecordingStep> {
    let step = step_ref(recording, index);
    if let Some(press) = step.and_then(|step| step.folded_into) {
        return Err(in_fold(index, press));
    }
    step.filter(|step| step.is_effective())
        .ok_or_else(|| step_not_found(index))
}

/// The highest `fNNNN` frame number on record, steps and transitions included; #463 R2: and the
/// endings' frames.
fn highest_frame_number(recording: &LabRecording) -> u32 {
    let mut highest = 0_u32;
    for step in &recording.steps {
        let transition_frames = match &step.transition {
            Some(StepTransition::Page { frames, .. }) => frames.as_slice(),
            _ => &[][..],
        };
        for frame in step.frames.iter().chain(transition_frames) {
            if let Some(number) = frame
                .frame_id
                .strip_prefix('f')
                .and_then(|digits| digits.parse::<u32>().ok())
            {
                highest = highest.max(number);
            }
        }
    }
    for frame in recording
        .endings
        .iter()
        .flat_map(|ending| ending.frames.iter())
    {
        if let Some(number) = frame
            .frame_id
            .strip_prefix('f')
            .and_then(|digits| digits.parse::<u32>().ok())
        {
            highest = highest.max(number);
        }
    }
    highest
}

/// Provenance of a frame from a `--record` command.
pub(crate) struct FrameMeta {
    pub(crate) source: String,
    pub(crate) runtime_artifact: Option<OpaqueJson>,
    pub(crate) capture_backend: Option<String>,
    pub(crate) freshness: Option<OpaqueJson>,
}

impl FrameMeta {
    pub(crate) fn local() -> Self {
        Self {
            source: "local_png".to_string(),
            runtime_artifact: None,
            capture_backend: None,
            freshness: None,
        }
    }
}

/// Checks the size, queues the content-addressed write and keeps the decoded frame. Workflow #392
/// DA-5cd fix round 1 (`contracts/lab-recording.md`, "A recording's size"): first, before
/// anything of it is kept, the recording's size ([`admit_recording_frame`]): a first frame of
/// another size than the base size the recording's header names is refused
/// `record_display_not_base`, and every frame of a recording whose frames are not the base size
/// `record_size_not_base`; the command then returns the refusal and writes nothing.
fn admit_frame(
    session: &mut Session,
    loaded: LoadedFrame,
    role: &str,
    meta: &FrameMeta,
    label: &str,
) -> LabResult<RecordedFrame> {
    admit_recording_frame(&session.recording, loaded.size())?;
    check_frame_size(session.recording.coordinate_space, &loaded, label)?;
    if session.recording.coordinate_space.is_none() {
        session.recording.coordinate_space = Some(loaded.size());
    }
    let path = frame_store_path(&session.lab_dir, &loaded.sha256);
    let frame_id = format!("f{:04}", session.next_frame);
    session.next_frame = session
        .next_frame
        .checked_add(1)
        .ok_or_else(|| invalid("validation_failed", "frame id overflow"))?;
    let entry = RecordedFrame {
        frame_id,
        role: role.to_string(),
        path: path.display().to_string(),
        sha256: loaded.sha256.clone(),
        width: loaded.frame.width,
        height: loaded.frame.height,
        byte_count: loaded.png.len() as u64,
        source: meta.source.clone(),
        runtime_artifact: meta.runtime_artifact.clone(),
        capture_backend: meta.capture_backend.clone(),
        freshness: meta.freshness.clone(),
        recorded_at_unix_ms: now_unix_ms(),
        superseded: false,
        scaled_from: None,
    };
    session
        .pending
        .push((path, loaded.png.clone(), loaded.sha256.clone()));
    session.cache.insert(loaded.sha256.clone(), loaded);
    Ok(entry)
}

/// Loads every listed frame into the cache, checking stored bytes against their sha256.
fn ensure_loaded(session: &mut Session, frames: &[RecordedFrame]) -> LabResult<()> {
    for frame in frames {
        if !session.cache.contains_key(&frame.sha256) {
            let loaded = read_stored_frame(frame)?;
            session.cache.insert(frame.sha256.clone(), loaded);
        }
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArrivalMode {
    Device,
    Offline,
}

pub(crate) struct Arrival {
    pub(crate) step: u32,
    pub(crate) opened: bool,
    pub(crate) closed_step: Option<u32>,
    pub(crate) frame: RecordedFrame,
    /// #463 S1-1: the step of a press recorded by the command that makes it whose after frame this
    /// frame became; #463 S1-2: or, for a capture of the press's after loop, whose after frame the
    /// loop names when it ends ([`name_after_frame`]).
    pub(crate) pressed_step: Option<u32>,
}

/// The sha256 of the step's live (not superseded) primary frame.
fn live_primary_sha(step: &RecordingStep) -> Option<String> {
    step.frames
        .iter()
        .rev()
        .find(|frame| frame.role == "primary" && !frame.superseded)
        .map(|frame| frame.sha256.clone())
}

/// #463 S1-1: the step's live (not superseded) primary frame, the screen a press on it is
/// committed on.
pub(crate) fn live_primary(step: &RecordingStep) -> Option<&RecordedFrame> {
    step.frames
        .iter()
        .rev()
        .find(|frame| frame.role == "primary" && !frame.superseded)
}

/// #463 S1-1 (`contracts/lab-recording.md`, "Recording while playing"): a frame that opens a step
/// is the after frame of the press of the step before it when that press was recorded by the
/// command that makes it (its execution names its `before_frame`), was performed, and has no after
/// frame yet. Returns that step. A press of `do --capture --record` names no `before_frame` and is
/// left as it was. #463 S1-2: only the step; [`name_after_frame`] writes the frame. #463 S1-3: so
/// is the step of an application operation whose completed receipt names its `before_frame`
/// (`session app <verb> --record` on a step with a frame) and has no after frame yet, when the step
/// has no click; the entry step, whose receipt names none, is left as it was.
fn awaiting_after_frame(recording: &LabRecording) -> Option<u32> {
    let index = last_effective(recording)?;
    let step = step_ref(recording, index)?;
    let awaiting = match (&step.click, &step.application) {
        (Some(click), _) => {
            click.executed()
                && click.execution.as_ref().is_some_and(|execution| {
                    execution.before_frame.is_some() && execution.after_frame.is_none()
                })
        }
        (None, Some(application)) => application.executed.as_ref().is_some_and(|execution| {
            execution.before_frame.is_some() && execution.after_frame.is_none()
        }),
        (None, None) => false,
    };
    awaiting.then_some(index)
}

/// #463 S1-2 (`contracts/lab-recording.md`, "Recording while playing"): the one place a press's
/// `after_frame` is written: the press of step `index` names `frame_id`, the last capture of its
/// own after loop when the loop ends (`record_after_frame`), or, when the loop stored nothing, the
/// first frame that opens a step after it ([`arrive`]). #463 S1-3: on a step without a click, the
/// completed receipt of its application operation names it so ([`after_members`]).
pub(crate) fn name_after_frame(
    recording: &mut LabRecording,
    index: u32,
    frame_id: &str,
) -> LabResult<()> {
    let (after_frame, _) = after_members(recording, index)?;
    *after_frame = Some(frame_id.to_owned());
    Ok(())
}

/// #463 S1-3 (the train of stage one, which joins S1-2's [`name_after_frame`] with S1-3's
/// application steps): the `after_frame` and `after_capture` of the effect of step `index`: its
/// click's execution when it has a click, else the completed receipt of its application operation;
/// `record_step_not_found` when it has neither.
fn after_members(
    recording: &mut LabRecording,
    index: u32,
) -> LabResult<(
    &mut Option<String>,
    &mut Option<super::model::AfterCaptureRecord>,
)> {
    let step = step_mut(recording, index)?;
    match (step.click.as_mut(), step.application.as_mut()) {
        (Some(click), _) => click
            .execution
            .as_mut()
            .map(|execution| (&mut execution.after_frame, &mut execution.after_capture)),
        (None, Some(application)) => application
            .executed
            .as_mut()
            .map(|execution| (&mut execution.after_frame, &mut execution.after_capture)),
        (None, None) => None,
    }
    .ok_or_else(|| step_not_found(index))
}

/// #463 S1-1: how the frame after the press of step `index` was taken, on its execution; #463
/// S1-3: or on the completed receipt of its application operation ([`after_members`]).
pub(crate) fn note_after_capture(
    recording: &mut LabRecording,
    index: u32,
    record: &super::model::AfterCaptureRecord,
) -> LabResult<()> {
    let (_, after_capture) = after_members(recording, index)?;
    *after_capture = Some(record.clone());
    Ok(())
}

/// Whether the step carries marks (its own or reused ones). #463 S1-1 fix round 2: the capture
/// before a press on such an open step is not stored (`record_before_frame`).
pub(crate) fn has_marks(step: &RecordingStep) -> bool {
    !step.marks.is_empty() || !step.reused.is_empty()
}

fn click_missing(index: u32) -> LabError {
    with_details(
        blocked(
            "record_step_click_missing",
            format!(
                "step {index} has marks but no click or application operation. If this frame \
                 shows a loading page or another intermediate screen, use `record mark \
                 --to-transition {index}`; if step {index} is the final step, check with \
                 `record stop --dry-run` first and then run `record stop`"
            ),
        ),
        json!({"step": index}),
    )
}

/// Whether a frame may arrive now, and what it does (frozen model section 2.5 table).
enum ArrivalPlan {
    Open { close: Option<(u32, &'static str)> },
    Replace(u32),
}

fn plan_arrival(recording: &LabRecording, mode: ArrivalMode) -> LabResult<ArrivalPlan> {
    let Some(index) = open_step(recording) else {
        return Ok(ArrivalPlan::Open { close: None });
    };
    let step = effective_step(recording, index)?;
    if let Some(application) = &step.application {
        return application_arrival(index, application, mode);
    }
    match &step.click {
        None if !has_marks(step) => Ok(ArrivalPlan::Replace(index)),
        None => Err(click_missing(index)),
        Some(click) if click.executed() => Ok(ArrivalPlan::Open {
            close: Some((index, "click")),
        }),
        Some(click) if click.execution.is_some() => Err(with_details(
            blocked(
                "record_step_click_not_executed",
                format!(
                    "the click of step {index} has an indeterminate outcome; check the screen, \
                     then run `record mark --reopen-step {index}` to execute it again or \
                     `record mark --close-step` to accept it"
                ),
            ),
            json!({"step": index, "outcome": "indeterminate"}),
        )),
        Some(_) => match mode {
            ArrivalMode::Device => Err(with_details(
                blocked(
                    "record_step_click_not_executed",
                    format!(
                        "step {index} has a declared click that was not executed; run \
                         `do --capture --record` or close the step with \
                         `record mark --close-step`"
                    ),
                ),
                json!({"step": index}),
            )),
            ArrivalMode::Offline => Ok(ArrivalPlan::Open {
                close: Some((index, "offline_frame")),
            }),
        },
    }
}

/// A frame after the application operation of the open step: the rows of the click read
/// as "effect" (R24 section 2.5). A declared operation, also one whose attempts have no
/// completed receipt, refuses a device frame and is closed by an offline frame.
fn application_arrival(
    index: u32,
    application: &StepApplication,
    mode: ArrivalMode,
) -> LabResult<ArrivalPlan> {
    if application.executed.is_some() {
        return Ok(ArrivalPlan::Open {
            close: Some((index, "application")),
        });
    }
    match mode {
        ArrivalMode::Device => Err(with_details(
            blocked(
                "record_step_click_not_executed",
                format!(
                    "step {index} declares the application operation {} without a completed \
                     receipt{}; run `session app {} --record` to execute it or close the step \
                     with `record mark --close-step`",
                    application.action,
                    if application.attempts.is_empty() {
                        ""
                    } else {
                        " (an earlier attempt may have run: check the instance first)"
                    },
                    application.cli_verb
                ),
            ),
            json!({
                "step": index,
                "application": application.action,
                "attempts": application.attempts.len()
            }),
        )),
        ArrivalMode::Offline => Ok(ArrivalPlan::Open {
            close: Some((index, "offline_frame")),
        }),
    }
}

/// Device preflight: refuses before anything is captured.
pub(crate) fn check_device_arrival(recording: &LabRecording) -> LabResult<()> {
    plan_arrival(recording, ArrivalMode::Device).map(|_| ())
}

/// The serial number of the next step (serial numbers are never reused).
fn next_step_index(recording: &LabRecording) -> LabResult<u32> {
    recording
        .steps
        .iter()
        .map(|step| step.index)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| invalid("validation_failed", "step index overflow"))
}

/// A new open step: a frame arrival, or the application entry step (no frame).
fn new_step(index: u32, frames: Vec<RecordedFrame>) -> RecordingStep {
    RecordingStep {
        index,
        page: None,
        dropped: false,
        converted_to_transition: false,
        frames,
        marks: Vec::new(),
        reused: Vec::new(),
        click: None,
        click_guard: None,
        application: None,
        optional: None,
        forbidden: Vec::new(),
        anchor: None,
        select: None,
        choice: None,
        inserted_after: None,
        folded_into: None,
        scan_fold: None,
        transition: None,
        closed: false,
        closed_by: None,
        // Written by `save` with the lock holder's word.
        initiator: String::new(),
    }
}

pub(crate) fn arrive(
    session: &mut Session,
    loaded: LoadedFrame,
    mode: ArrivalMode,
    meta: &FrameMeta,
    label: &str,
) -> LabResult<Arrival> {
    arrive_naming(session, loaded, mode, meta, label, true)
}

/// #463 S1-2 (`contracts/lab-recording.md`, "Recording while playing"): a capture of the after loop
/// of a press recorded by the command that makes it arrives as a device frame does, without naming
/// itself the press's `after_frame`: the loop names its last capture when it ends. Its `Arrival`
/// still answers the press it follows (`pressed_step`).
pub(crate) fn arrive_after_press(
    session: &mut Session,
    loaded: LoadedFrame,
    meta: &FrameMeta,
    label: &str,
) -> LabResult<Arrival> {
    arrive_naming(session, loaded, ArrivalMode::Device, meta, label, false)
}

fn arrive_naming(
    session: &mut Session,
    loaded: LoadedFrame,
    mode: ArrivalMode,
    meta: &FrameMeta,
    label: &str,
    names_after_frame: bool,
) -> LabResult<Arrival> {
    let plan = plan_arrival(&session.recording, mode)?;
    let entry = admit_frame(session, loaded, "primary", meta, label)?;
    match plan {
        ArrivalPlan::Replace(index) => {
            let step = step_mut(&mut session.recording, index)?;
            for frame in &mut step.frames {
                frame.superseded = true;
            }
            step.frames.push(entry.clone());
            Ok(Arrival {
                step: index,
                opened: false,
                closed_step: None,
                frame: entry,
                pressed_step: None,
            })
        }
        ArrivalPlan::Open { close } => {
            if let Some((index, by)) = close {
                let step = step_mut(&mut session.recording, index)?;
                step.closed = true;
                step.closed_by = Some(by.to_string());
            }
            let pressed_step = awaiting_after_frame(&session.recording);
            if let Some(pressed) = pressed_step.filter(|_| names_after_frame) {
                name_after_frame(&mut session.recording, pressed, &entry.frame_id)?;
            }
            let index = next_step_index(&session.recording)?;
            session
                .recording
                .steps
                .push(new_step(index, vec![entry.clone()]));
            Ok(Arrival {
                step: index,
                opened: true,
                closed_step: close.map(|(index, _)| index),
                frame: entry,
                pressed_step,
            })
        }
    }
}

/// Every mark id in use: own marks of effective steps and the marks of their transitions, except
/// the `released` ones. #411 Lab B-9: a value test's side target ids are in use too. #463 R2: and
/// the endings' own marks.
fn ids_in_use(recording: &LabRecording, released: &BTreeSet<String>) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for step in recording.steps.iter().filter(|step| step.is_effective()) {
        let transition_marks = match &step.transition {
            Some(StepTransition::Page { marks, .. }) => marks.as_slice(),
            _ => &[][..],
        };
        for mark in step.marks.iter().chain(transition_marks) {
            if !released.contains(&mark.id) {
                ids.extend(mark.target_ids());
            }
        }
    }
    for mark in recording
        .endings
        .iter()
        .flat_map(|ending| ending.marks.iter())
    {
        ids.extend(mark.target_ids());
    }
    ids
}

/// #411 Lab B-9: the target ids a new mark gives the package: its id and, for a value test, its
/// side target ids.
fn spec_target_ids(spec: &MarkSpec) -> Vec<String> {
    let mut ids = vec![spec.id.clone()];
    if spec.family == MarkFamily::ValueTest {
        ids.push(value_test_side_id(&spec.id, "left"));
        if matches!(spec.right, Some(ValueTestRight::Side(_))) {
            ids.push(value_test_side_id(&spec.id, "right"));
        }
    }
    ids
}

/// #411 Lab B-9: a value test, or a check with one as member: the page detector refuses it in
/// `forbidden`.
fn tests_values(recording: &LabRecording, mark: &RecordedMark) -> bool {
    mark.family == MarkFamily::ValueTest
        || mark.check_members().iter().any(|member| {
            find_step_mark(recording, member)
                .is_some_and(|member| member.family == MarkFamily::ValueTest)
        })
}

fn template_assets(recording: &LabRecording) -> BTreeMap<String, String> {
    let mut assets = BTreeMap::new();
    for step in recording.steps.iter().filter(|step| step.is_effective()) {
        let transition_marks = match &step.transition {
            Some(StepTransition::Page { marks, .. }) => marks.as_slice(),
            _ => &[][..],
        };
        for mark in step.marks.iter().chain(transition_marks) {
            if let Some(crop) = &mark.crop {
                assets.insert(crop.asset.clone(), mark.id.clone());
            }
        }
    }
    // #463 R2: an ending's template crop is an asset of the package too.
    for mark in recording
        .endings
        .iter()
        .flat_map(|ending| ending.marks.iter())
    {
        if let Some(crop) = &mark.crop {
            assets.insert(crop.asset.clone(), mark.id.clone());
        }
    }
    assets
}

/// A step mark (own) of any effective step, by id.
fn find_step_mark(recording: &LabRecording, id: &str) -> Option<RecordedMark> {
    recording
        .steps
        .iter()
        .filter(|step| step.is_effective())
        .flat_map(|step| step.marks.iter())
        .find(|mark| mark.id == id)
        .cloned()
}

/// The template bytes of a stored mark.
fn stored_crop(mark: &RecordedMark) -> LabResult<Option<Vec<u8>>> {
    match &mark.crop {
        Some(crop) => read_verified(std::path::Path::new(&crop.path), &crop.sha256).map(Some),
        None => Ok(None),
    }
}

/// The marks of one target (a step or a transition) after the batch, evaluated together.
pub(crate) struct MarkBatch {
    pub(crate) own: Vec<RecordedMark>,
    pub(crate) reused: Vec<String>,
    pub(crate) new_crops: BTreeMap<String, Vec<u8>>,
    pub(crate) added_ids: BTreeSet<String>,
}

pub(crate) struct BatchResult {
    pub(crate) own: Vec<RecordedMark>,
    pub(crate) reused: Vec<RecordedMark>,
    pub(crate) rejections: Vec<MarkRejection>,
}

/// Self-test of every own and reused mark of the target on its live frames, checks last; OCR
/// marks with the Lab's OCR provider (Workflow #396 LAB-3).
fn evaluate_batch(
    session: &Session,
    batch: &MarkBatch,
    live: &[RecordedFrame],
    ocr: &LabOcr,
) -> LabResult<BatchResult> {
    let frames = live
        .iter()
        .map(|entry| {
            session
                .cache
                .get(&entry.sha256)
                .map(|frame| EvalFrame {
                    id: entry.frame_id.as_str(),
                    frame,
                })
                .ok_or_else(|| invalid("validation_failed", "frame was not loaded"))
        })
        .collect::<LabResult<Vec<_>>>()?;
    let defaults = &session.recording.defaults;
    let mut rejections = Vec::new();
    let mut statuses = BTreeMap::new();
    let mut own = batch.own.clone();
    for mark in own
        .iter_mut()
        .filter(|mark| mark.family != MarkFamily::Check)
    {
        let crop = match batch.new_crops.get(&mark.id) {
            Some(bytes) => Some(bytes.clone()),
            None => stored_crop(mark)?,
        };
        let (mut test, mut failure) = evaluate_mark(mark, crop.as_deref(), &frames, defaults, ocr)?;
        // Workflow #396 REC-4 (REC-M §7): an own template mark with factors passes at every
        // factor on at least one frame of its step, so the author records each zoom.
        let unmatched_factor = match (&mark.scales, &test.matched_scales_milli) {
            (Some(scales), Some(matched))
                if failure.is_none() && matched.len() != scales.factors_milli.len() =>
            {
                Some(json!({
                    "factors_milli": scales.factors_milli,
                    "matched_scales_milli": matched
                }))
            }
            _ => None,
        };
        if let Some(detail) = unmatched_factor {
            test.status = SelfTestStatus::Failed;
            test.reason = Some("self_test_failed".to_string());
            failure = Some(detail);
        }
        if let Some(detail) = failure {
            rejections.push(MarkRejection::new(
                &mark.id,
                test.reason.as_deref().unwrap_or("self_test_failed"),
                detail,
            ));
        }
        statuses.insert(mark.id.clone(), test.status);
        mark.self_test = test;
    }
    let mut reused = Vec::new();
    for id in &batch.reused {
        let Some(mut mark) = find_step_mark(&session.recording, id) else {
            rejections.push(MarkRejection::new(id, "reuse_source_missing", json!({})));
            continue;
        };
        if mark.family == MarkFamily::Check {
            reused.push(mark);
            continue;
        }
        let crop = stored_crop(&mark)?;
        let (test, failure) = evaluate_mark(&mark, crop.as_deref(), &frames, defaults, ocr)?;
        if let Some(detail) = failure {
            rejections.push(MarkRejection::new(
                id,
                test.reason.as_deref().unwrap_or("self_test_failed"),
                detail,
            ));
        }
        statuses.insert(id.clone(), test.status);
        mark.self_test = test;
        reused.push(mark);
    }
    let frame_count = u32::try_from(frames.len()).unwrap_or(u32::MAX);
    let kinds = own
        .iter()
        .chain(reused.iter())
        .map(|mark| (mark.id.clone(), mark.family))
        .collect::<BTreeMap<_, _>>();
    for mark in own
        .iter_mut()
        .chain(reused.iter_mut())
        .filter(|mark| mark.family == MarkFamily::Check)
    {
        let invalid_members = mark
            .check_members()
            .iter()
            .filter(|member| {
                kinds
                    .get(member.as_str())
                    .is_none_or(|family| *family == MarkFamily::Check)
            })
            .cloned()
            .collect::<Vec<_>>();
        if !invalid_members.is_empty() {
            rejections.push(MarkRejection::new(
                &mark.id,
                "check_member_invalid",
                json!({"members": invalid_members}),
            ));
            continue;
        }
        let (test, failure) = derive_check(mark, &statuses, frame_count);
        if let Some(detail) = failure {
            rejections.push(MarkRejection::new(&mark.id, "self_test_failed", detail));
        }
        mark.self_test = test;
    }
    Ok(BatchResult {
        own,
        reused,
        rejections,
    })
}

fn rejected(rejections: &[MarkRejection], total: usize) -> LabError {
    with_details(
        blocked(
            "record_mark_rejected",
            format!(
                "{} of {total} marks failed; nothing recorded",
                rejections.len()
            ),
        ),
        json!({"marks": rejections.iter().map(MarkRejection::to_json).collect::<Vec<_>>()}),
    )
}

/// Adds new marks to `batch`: id rules, the shared id space, template asset names and the
/// values taken from `primary`.
#[allow(clippy::too_many_arguments)]
fn add_marks(
    session: &Session,
    batch: &mut MarkBatch,
    specs: &[MarkSpec],
    primary_sha: &str,
    step: u32,
    transition_of: Option<u32>,
    released: &BTreeSet<String>,
    rejections: &mut Vec<MarkRejection>,
) -> LabResult<()> {
    let primary = session
        .cache
        .get(primary_sha)
        .ok_or_else(|| invalid("validation_failed", "primary frame was not loaded"))?;
    let mut in_use = ids_in_use(&session.recording, released);
    in_use.extend(batch.own.iter().flat_map(RecordedMark::target_ids));
    let mut assets = template_assets(&session.recording);
    assets.retain(|_, id| !released.contains(id));
    let now = now_unix_ms();
    for spec in specs {
        validate_mark_id(&spec.id)?;
        // #411 Lab B-9: a value test's side targets share the package's id space.
        claim_mark_ids(&mut in_use, &spec_target_ids(spec))?;
        let prepared = match prepare_mark(
            spec,
            primary,
            &session.recording.defaults,
            step,
            transition_of,
            now,
        )? {
            Ok(prepared) => prepared,
            Err(rejection) => {
                rejections.push(rejection);
                continue;
            }
        };
        let mut mark = prepared.mark;
        // Workflow #434 LAB-2 fix 2: the lock holder's initiator word, filled when the mark is
        // made, so that the answer of `record mark` carries the word the file does.
        mark.initiator.clone_from(&session.initiator);
        if let (Some(crop), Some(png)) = (mark.crop.as_mut(), prepared.crop_png) {
            claim_asset_name(&mut assets, &spec.id, &crop.asset)?;
            crop.path = crop_store_path(&session.lab_dir, &crop.sha256)
                .display()
                .to_string();
            batch.new_crops.insert(spec.id.clone(), png);
        }
        batch.added_ids.insert(spec.id.clone());
        batch.own.push(mark);
    }
    Ok(())
}

/// The one check of a mark's target ids entering the recording's id space `in_use`, claimed in
/// order: `target_ids` holds the mark's id first, then (#411 Lab B-9) a value test's side target
/// ids; one already in use is `record_mark_id_conflict`. #463 S2-7 fix round 1 (review (d)): its
/// two callers are a new mark of `record mark` ([`add_marks`]) and a dropped step's mark given back
/// ([`restore_step`]).
fn claim_mark_ids(in_use: &mut BTreeSet<String>, target_ids: &[String]) -> LabResult<()> {
    let Some((id, sides)) = target_ids.split_first() else {
        return Ok(());
    };
    if !in_use.insert(id.clone()) {
        return Err(with_details(
            blocked(
                "record_mark_id_conflict",
                format!("mark id '{id}' is already used in this recording"),
            ),
            json!({"id": id}),
        ));
    }
    for target_id in sides {
        if !in_use.insert(target_id.clone()) {
            return Err(with_details(
                blocked(
                    "record_mark_id_conflict",
                    format!(
                        "value test '{id}' declares the OCR target '{target_id}', an id already \
                         used in this recording"
                    ),
                ),
                json!({"id": id, "target_id": target_id}),
            ));
        }
    }
    Ok(())
}

/// The one check of a template mark's asset name: `assets` maps each asset name of the recording to
/// the mark that has it; the name of another mark is `record_asset_name_conflict`, and the name is
/// claimed for `id`. #463 S2-7 fix round 1 (review (d)): its two callers are [`add_marks`] and
/// [`restore_step`].
fn claim_asset_name(assets: &mut BTreeMap<String, String>, id: &str, asset: &str) -> LabResult<()> {
    if let Some(owner) = assets.get(asset)
        && owner.as_str() != id
    {
        return Err(with_details(
            blocked(
                "record_asset_name_conflict",
                format!("template '{id}' maps to {asset} which template '{owner}' already uses"),
            ),
            json!({"id": id, "asset": asset, "existing": owner}),
        ));
    }
    assets.insert(asset.to_owned(), id.to_owned());
    Ok(())
}

fn queue_crops(session: &mut Session, batch: &MarkBatch) {
    for mark in &batch.own {
        if let (Some(crop), Some(png)) = (&mark.crop, batch.new_crops.get(&mark.id)) {
            session
                .pending
                .push((PathBuf::from(&crop.path), png.clone(), crop.sha256.clone()));
        }
    }
}

/// Reuse sources: own marks of other effective steps, not already listed.
fn add_reuse(
    session: &Session,
    batch: &mut MarkBatch,
    ids: &[String],
    own_step: Option<u32>,
) -> LabResult<()> {
    for id in ids {
        let owner = session
            .recording
            .steps
            .iter()
            .filter(|step| step.is_effective())
            .find(|step| step.marks.iter().any(|mark| &mark.id == id));
        match owner {
            Some(step) if Some(step.index) != own_step => {
                if !batch.reused.contains(id) {
                    batch.reused.push(id.clone());
                }
            }
            Some(_) => {
                return Err(invalid(
                    "validation_failed",
                    format!("mark '{id}' already belongs to this step; it cannot be reused here"),
                ));
            }
            None => {
                return Err(with_details(
                    blocked(
                        "record_mark_rejected",
                        format!("mark '{id}' is not a step mark of this recording"),
                    ),
                    json!({"marks": [{"id": id, "reason": "reuse_source_missing"}]}),
                ));
            }
        }
    }
    Ok(())
}

/// Ids of own marks of `index` that other steps or transitions reuse or that checks name.
/// Workflow #390 LAB-2: a mark another step's page forbids is in use too. #463 R2: so is one an
/// ending reuses.
fn reused_elsewhere(recording: &LabRecording, index: u32, ids: &BTreeSet<String>) -> Vec<String> {
    let mut used = BTreeSet::new();
    for id in recording
        .endings
        .iter()
        .flat_map(|ending| ending.reused.iter())
    {
        if ids.contains(id) {
            used.insert(id.clone());
        }
    }
    for step in recording.steps.iter().filter(|step| step.is_effective()) {
        let transition_reused = match &step.transition {
            Some(StepTransition::Page { reused, .. }) => reused.as_slice(),
            _ => &[][..],
        };
        if step.index != index {
            for id in step
                .reused
                .iter()
                .chain(transition_reused)
                .chain(&step.forbidden)
            {
                if ids.contains(id) {
                    used.insert(id.clone());
                }
            }
        } else {
            for id in transition_reused {
                if ids.contains(id) {
                    used.insert(id.clone());
                }
            }
        }
    }
    used.into_iter().collect()
}

fn rect_center(rect: RecordRect) -> RecordPoint {
    RecordPoint {
        x: rect.x + rect.width / 2,
        y: rect.y + rect.height / 2,
    }
}

pub(crate) fn validate_retry(retry: &super::model::ClickRetry) -> LabResult<()> {
    if !(2..=5).contains(&retry.max_attempts) || !(1..=5000).contains(&retry.interval_ms) {
        return Err(invalid(
            "validation_failed",
            "click retry needs max_attempts 2..=5 and interval_ms 1..=5000",
        ));
    }
    Ok(())
}

pub(crate) fn validate_page_transition_timeout(timeout_ms: Option<u64>) -> LabResult<()> {
    if timeout_ms.is_some_and(|timeout| !(1..=MAX_TRANSITION_MS).contains(&timeout)) {
        return Err(invalid(
            "validation_failed",
            "--transition-timeout-ms must be in 1..=1800000",
        ));
    }
    Ok(())
}

pub(crate) fn validate_window(min_ms: u64, max_ms: u64) -> LabResult<()> {
    if min_ms > max_ms || !(1..=MAX_TRANSITION_MS).contains(&max_ms) {
        return Err(with_details(
            invalid(
                "record_transition_window_invalid",
                format!(
                    "a window needs 0 <= min_ms <= max_ms and 1 <= max_ms <= 1800000; got \
                     {min_ms}..{max_ms}"
                ),
            ),
            json!({"min_ms": min_ms, "max_ms": max_ms}),
        ));
    }
    Ok(())
}

/// The marks flow of `record mark`: target step, frame, remove, add, reuse, samples, click,
/// self-test. Returns the outcome pieces; the caller writes.
pub(crate) struct MarkApplied {
    pub(crate) step: u32,
    pub(crate) opened: bool,
    pub(crate) closed_step: Option<u32>,
    pub(crate) frame: Option<RecordedFrame>,
    pub(crate) samples: Vec<RecordedFrame>,
    pub(crate) added: Vec<RecordedMark>,
    pub(crate) reused: Vec<RecordedMark>,
    pub(crate) removed: Vec<String>,
    /// Workflow #390 LAB-2: the marks the step's page forbids, tested on its live frames.
    pub(crate) forbidden: Vec<ForbiddenMarkView>,
    /// #463 S2-5: the request's boxes as the step took them.
    pub(crate) boxes: Vec<BoxView>,
}

/// Workflow #390 LAB-2: the marks the step's page forbids after this command: the step's list
/// without the ones `remove` takes off, then each `forbid` id in its order. A forbidden mark is an
/// own mark of another effective step, as a reused one is (`forbidden_source_missing`
/// otherwise), and never a mark the page also requires (an own or reused mark of the step).
fn forbidden_after(
    session: &Session,
    target: u32,
    existing: &[String],
    removed: &[String],
    added: &[String],
    batch: &MarkBatch,
) -> LabResult<Vec<String>> {
    let mut list = existing
        .iter()
        .filter(|id| !removed.contains(id))
        .cloned()
        .collect::<Vec<_>>();
    let required_here =
        |id: &String| batch.own.iter().any(|mark| &mark.id == id) || batch.reused.contains(id);
    for id in added {
        let owner = session
            .recording
            .steps
            .iter()
            .filter(|step| step.is_effective())
            .find(|step| step.marks.iter().any(|mark| &mark.id == id));
        match owner {
            _ if required_here(id) => {
                return Err(invalid(
                    "validation_failed",
                    format!(
                        "mark '{id}' is a mark of this step; its page requires it and cannot also \
                         forbid it"
                    ),
                ));
            }
            Some(step) if step.index != target => {
                // #411 Lab B-9: a value test that cannot read does not pass, which in `forbidden`
                // would let the page match unseen; the Runtime refuses it there, directly or
                // through a check.
                if let Some(mark) = step.marks.iter().find(|mark| &mark.id == id)
                    && tests_values(&session.recording, mark)
                {
                    return Err(with_details(
                        invalid(
                            "validation_failed",
                            format!(
                                "mark '{id}' tests a value; a page cannot forbid a value test: \
                                 require the opposite comparison instead"
                            ),
                        ),
                        json!({"mark_id": id, "family": mark.family.as_str()}),
                    ));
                }
                if !list.contains(id) {
                    list.push(id.clone());
                }
            }
            Some(_) => {
                return Err(invalid(
                    "validation_failed",
                    format!("mark '{id}' belongs to this step; its page cannot forbid it"),
                ));
            }
            None => {
                return Err(with_details(
                    blocked(
                        "record_mark_rejected",
                        format!("mark '{id}' is not a step mark of this recording"),
                    ),
                    json!({"marks": [{"id": id, "reason": "forbidden_source_missing"}]}),
                ));
            }
        }
    }
    if let Some(id) = list.iter().find(|id| required_here(id)) {
        return Err(invalid(
            "validation_failed",
            format!(
                "mark '{id}' is forbidden on this step's page; it cannot be reused here as well"
            ),
        ));
    }
    Ok(list)
}

/// Workflow #390 LAB-2: whether `mark` passes on one live frame, as its self-test evaluates it
/// (a check from its members on that frame), with the error of an evaluation that failed.
fn mark_on_frame(
    session: &Session,
    mark: &RecordedMark,
    frame: &EvalFrame<'_>,
    ocr: &LabOcr,
) -> LabResult<(SelfTestStatus, Option<Value>)> {
    if mark.family == MarkFamily::Check {
        let mut members = BTreeMap::new();
        for member in mark.check_members() {
            let member_mark = find_step_mark(&session.recording, member).ok_or_else(|| {
                invalid(
                    "validation_failed",
                    format!(
                        "check '{}' names '{member}', which is not a step mark",
                        mark.id
                    ),
                )
            })?;
            let (status, error) = mark_on_frame(session, &member_mark, frame, ocr)?;
            if error.is_some() {
                return Ok((status, error));
            }
            members.insert(member.clone(), status);
        }
        return Ok((derive_check(mark, &members, 1).0.status, None));
    }
    let crop = stored_crop(mark)?;
    let (test, failure) = evaluate_mark(
        mark,
        crop.as_deref(),
        std::slice::from_ref(frame),
        &session.recording.defaults,
        ocr,
    )?;
    let error =
        (test.reason.as_deref() == Some("self_test_error")).then(|| failure.unwrap_or(Value::Null));
    Ok((test.status, error))
}

/// Workflow #390 LAB-2: every forbidden mark on every live frame of the step. One that passes on a
/// frame is refused (`forbidden_mark_passes`, naming the frame), as is one whose evaluation failed
/// (`self_test_error`); one that cannot be evaluated (OCR without a model) is `not_evaluated`.
fn evaluate_forbidden(
    session: &Session,
    forbidden: &[String],
    live: &[RecordedFrame],
    ocr: &LabOcr,
) -> LabResult<(Vec<ForbiddenMarkView>, Vec<MarkRejection>)> {
    let frames = live
        .iter()
        .map(|entry| {
            session
                .cache
                .get(&entry.sha256)
                .map(|frame| EvalFrame {
                    id: entry.frame_id.as_str(),
                    frame,
                })
                .ok_or_else(|| invalid("validation_failed", "frame was not loaded"))
        })
        .collect::<LabResult<Vec<_>>>()?;
    let mut views = Vec::new();
    let mut rejections = Vec::new();
    for id in forbidden {
        let mark = find_step_mark(&session.recording, id).ok_or_else(|| {
            with_details(
                blocked(
                    "record_mark_rejected",
                    format!("mark '{id}' is not a step mark of this recording"),
                ),
                json!({"marks": [{"id": id, "reason": "forbidden_source_missing"}]}),
            )
        })?;
        let mut not_evaluated = false;
        for frame in &frames {
            match mark_on_frame(session, &mark, frame, ocr)? {
                (_, Some(error)) => {
                    rejections.push(MarkRejection::new(
                        id,
                        "self_test_error",
                        json!({"frame_id": frame.id, "error": error, "forbidden": true}),
                    ));
                    break;
                }
                (SelfTestStatus::Passed, None) => {
                    rejections.push(MarkRejection::new(
                        id,
                        "forbidden_mark_passes",
                        json!({"frame_id": frame.id}),
                    ));
                    break;
                }
                (SelfTestStatus::NotEvaluated, None) => not_evaluated = true,
                (SelfTestStatus::Failed, None) => {}
            }
        }
        views.push(ForbiddenMarkView {
            id: id.clone(),
            family: mark.family,
            status: if not_evaluated {
                "not_evaluated"
            } else {
                "absent"
            }
            .to_string(),
            frames: u32::try_from(frames.len()).unwrap_or(u32::MAX),
        });
    }
    Ok((views, rejections))
}

/// Workflow #390 LAB-2: the step's anchor after this command must be an own template mark of the
/// step that its page requires (not only a member of an `any_of` check), or the batch is refused
/// with `anchor_invalid`.
fn anchor_rejection(
    anchor: Option<&String>,
    own: &[RecordedMark],
    reused: &[RecordedMark],
) -> Option<MarkRejection> {
    let id = anchor?;
    let problem = match own.iter().find(|mark| &mark.id == id) {
        Some(mark) if mark.family != MarkFamily::Template => "not_own_template",
        None => "not_own_template",
        Some(_)
            if own
                .iter()
                .chain(reused)
                .filter_map(|mark| mark.any_of.as_ref())
                .flatten()
                .any(|member| member == id) =>
        {
            "not_required"
        }
        Some(_) => return None,
    };
    Some(MarkRejection::new(
        id,
        "anchor_invalid",
        json!({"problem": problem}),
    ))
}

pub(crate) fn apply_marks(
    session: &mut Session,
    request: &super::model::MarkRequest,
    ocr: &LabOcr,
) -> LabResult<MarkApplied> {
    let local = FrameMeta::local();
    let mut opened = false;
    let mut closed_step = None;
    let mut new_frame = None;
    let target = match request.step {
        Some(index) => {
            let step = effective_step(&session.recording, index)?;
            if let Some(path) = &request.frame {
                let loaded = read_frame_file(path)?;
                if live_primary_sha(step).as_deref() != Some(loaded.sha256.as_str()) {
                    return Err(with_details(
                        blocked(
                            "record_step_frame_conflict",
                            format!("step {index} already has a different primary frame"),
                        ),
                        json!({"step": index, "frame": path}),
                    ));
                }
            }
            index
        }
        None => match &request.frame {
            Some(path) => {
                let loaded = read_frame_file(path)?;
                // A frame byte-identical to the open step's live primary frame is not an
                // arrival: the command targets that open step, as `--step n` would.
                let open = open_step(&session.recording);
                let same_as_open = open
                    .and_then(|index| step_ref(&session.recording, index))
                    .and_then(live_primary_sha)
                    .is_some_and(|sha256| sha256 == loaded.sha256);
                match open.filter(|_| same_as_open) {
                    Some(index) => index,
                    // Workflow #390 LAB-2: a stopped recording takes no new step.
                    None if session.stopped => {
                        return Err(stopped_change("a frame that opens a new step"));
                    }
                    None => {
                        let arrival = arrive(session, loaded, ArrivalMode::Offline, &local, path)?;
                        opened = arrival.opened;
                        closed_step = arrival.closed_step;
                        new_frame = Some(arrival.frame);
                        arrival.step
                    }
                }
            }
            None if request.application.is_some() => {
                let (index, entry_opened) = application_mark_target(&mut session.recording)?;
                opened = entry_opened;
                index
            }
            None => open_step(&session.recording).ok_or_else(|| {
                blocked(
                    "record_step_frame_missing",
                    "there is no open step; give --frame <png> or --step <n>",
                )
            })?,
        },
    };
    // #463 S2-5: the request's boxes as the members they stand for on the target step: each
    // recognize box a template mark of its own id after the request's `add`, the press box the
    // click's rectangle. Everything below is the request's own code for those members.
    let expanded = expanded_request(request, target);
    let request: &MarkRequest = &expanded;
    // The application entry step has no frame: only its effect can be declared on it.
    let frameless = step_ref(&session.recording, target)
        .filter(|step| live_primary_sha(step).is_none())
        .map(|step| {
            step.application
                .as_ref()
                .map(|application| application.action.clone())
        });
    if let Some(application) = frameless {
        // #463 R1 fix 1: a select is never dropped here. On an application step it is the
        // declaration's `with_application` (one beside an application operation of the same
        // request was refused before); on any other step without a frame it needs one, as a mark.
        // #463 S2-6: so is a choice marked there.
        if (request.select.is_some() || request.choice == Some(true))
            && let Some(action) = &application
        {
            return Err(select_invalid(
                Some(target),
                SelectProblem::WithApplication,
                format!(
                    "step {target} is an application step ({action}); one step has one effect: a \
                     select needs a step of its own, with its frame; nothing was written"
                ),
            ));
        }
        let needs_frame = request.page.is_some()
            || request.select.is_some()
            || !request.samples.is_empty()
            || !request.add.is_empty()
            || !request.reuse.is_empty()
            || !request.remove.is_empty()
            || request.click.is_some()
            || request.click_guard.is_some()
            || request.retry.is_some()
            || !request.forbid.is_empty()
            || request.anchor.is_some()
            || request.clear_anchor
            || !request.boxes.is_empty()
            || request.choice.is_some()
            || (request.application.is_none() && request.optional.is_none());
        if needs_frame {
            return Err(with_details(
                blocked(
                    "record_step_frame_missing",
                    format!(
                        "step {target} is the application entry step and has no frame; pages, \
                         marks, samples, clicks and selects need a step with a frame"
                    ),
                ),
                json!({"step": target}),
            ));
        }
        apply_application(session, request, target)?;
        apply_optional(&mut session.recording, request, target)?;
        return Ok(MarkApplied {
            step: target,
            opened,
            closed_step,
            frame: new_frame,
            samples: Vec::new(),
            added: Vec::new(),
            reused: Vec::new(),
            removed: Vec::new(),
            forbidden: Vec::new(),
            boxes: Vec::new(),
        });
    }
    if let Some(page) = &request.page {
        step_mut(&mut session.recording, target)?.page = Some(page.clone());
    }

    // remove
    let mut removed = Vec::new();
    // Workflow #390 LAB-2: `remove` also takes a forbidden mark off the page.
    let mut removed_forbidden = Vec::new();
    let mut rejections = Vec::new();
    let anchor = if request.clear_anchor {
        None
    } else {
        request
            .anchor
            .clone()
            .or_else(|| step_ref(&session.recording, target).and_then(|step| step.anchor.clone()))
    };
    {
        let recording = &session.recording;
        let step = effective_step(recording, target)?;
        let own_ids = step
            .marks
            .iter()
            .map(|mark| mark.id.clone())
            .collect::<BTreeSet<_>>();
        let elsewhere = reused_elsewhere(recording, target, &own_ids);
        for id in &request.remove {
            if step.forbidden.contains(id) {
                removed_forbidden.push(id.clone());
                continue;
            }
            let in_step = own_ids.contains(id) || step.reused.contains(id);
            if !in_step {
                rejections.push(MarkRejection::new(id, "mark_not_in_step", json!({})));
                continue;
            }
            let named_by_check = step.marks.iter().any(|mark| {
                mark.check_members().contains(id) && !request.remove.contains(&mark.id)
            });
            // #463 L2: the followed located-text mark may be declared again by the same request
            // (its names changed, for one); the click keeps following it.
            let declared_again = step
                .marks
                .iter()
                .any(|mark| &mark.id == id && mark.locates_text())
                && request
                    .add
                    .iter()
                    .any(|spec| &spec.id == id && spec.locate.is_some());
            let follows_it = step
                .click
                .as_ref()
                .and_then(|click| click.follow.as_ref())
                .is_some_and(|follow| &follow.mark_id == id);
            // #463 L2 fix 1: a click guard that names the followed located-text mark names the
            // guard the package has anyway; declared again, the mark is released as both.
            let released = follows_it && declared_again;
            let guard = (!released
                && (step.click_guard.as_deref() == Some(id.as_str()) || follows_it))
                || anchor.as_ref() == Some(id);
            if elsewhere.contains(id) || named_by_check || guard {
                return Err(with_details(
                    blocked(
                        "record_mark_in_use",
                        format!(
                            "mark '{id}' is reused or forbidden by another step, named by a check, \
                             is the click guard, is the mark the click follows or anchors the page \
                             (a located-text mark the click follows is declared again by removing \
                             and adding it in one request)"
                        ),
                    ),
                    json!({"id": id}),
                ));
            }
            removed.push(id.clone());
        }
    }
    if !rejections.is_empty() {
        return Err(rejected(&rejections, request.remove.len()));
    }

    // add, reuse
    let step = effective_step(&session.recording, target)?;
    let primary_sha = live_primary_sha(step).ok_or_else(|| {
        blocked(
            "record_step_frame_missing",
            format!("step {target} has no primary frame"),
        )
    })?;
    let mut batch = MarkBatch {
        own: step
            .marks
            .iter()
            .filter(|mark| !removed.contains(&mark.id))
            .cloned()
            .collect(),
        reused: step
            .reused
            .iter()
            .filter(|id| !removed.contains(id))
            .cloned()
            .collect(),
        new_crops: BTreeMap::new(),
        added_ids: BTreeSet::new(),
    };
    let live_existing = step
        .frames
        .iter()
        .filter(|frame| !frame.superseded)
        .cloned()
        .collect::<Vec<_>>();
    ensure_loaded(session, &live_existing)?;
    let released = removed.iter().cloned().collect::<BTreeSet<_>>();
    // #463 S2-5: a recognize box of one gray value is refused before its mark is made
    // (`box_flat`), so that it has that one reason.
    let primary = session
        .cache
        .get(&primary_sha)
        .ok_or_else(|| invalid("validation_failed", "primary frame was not loaded"))?;
    let flat = flat_boxes(primary, &request.boxes, target)?;
    let specs = request
        .add
        .iter()
        .filter(|spec| !flat.iter().any(|rejection| rejection.id == spec.id))
        .cloned()
        .collect::<Vec<_>>();
    let flat_count = flat.len();
    rejections.extend(flat);
    add_marks(
        session,
        &mut batch,
        &specs,
        &primary_sha,
        target,
        None,
        &released,
        &mut rejections,
    )?;
    add_reuse(session, &mut batch, &request.reuse, Some(target))?;
    // Workflow #390 LAB-2: the marks the page forbids after this command.
    let existing_forbidden = effective_step(&session.recording, target)?
        .forbidden
        .clone();
    let forbidden = forbidden_after(
        session,
        target,
        &existing_forbidden,
        &removed_forbidden,
        &request.forbid,
        &batch,
    )?;

    // samples; #463 S2-5: beside recognize boxes, a sample of the request on which one of them is
    // below its threshold is another state of the screen: not added, nothing of it stored, and
    // listed in the answer. One on which every one reaches it is a sample as before.
    let box_marks = made_box_marks(&request.boxes, target, &batch.own, &batch.new_crops);
    let mut samples = Vec::new();
    let mut box_frames = Vec::new();
    for (index, path) in request.samples.iter().enumerate() {
        let loaded = read_frame_file(path)?;
        let scores = if box_marks.is_empty() {
            None
        } else {
            check_frame_size(session.recording.coordinate_space, &loaded, path)?;
            let label = format!("sample {index}");
            let frame = EvalFrame {
                id: label.as_str(),
                frame: &loaded,
            };
            match call_frame(&box_marks, &frame, &session.recording.defaults, ocr)? {
                Ok(scores) => Some(scores),
                Err(rejection) => {
                    rejections.push(rejection);
                    continue;
                }
            }
        };
        match scores {
            Some(scores) if scores.below.is_some() => box_frames.push(ScoredFrame {
                frame_id: None,
                sample: Some(index),
                superseded: None,
                step: None,
                taken: BoxFrameTaken::NotAdded,
                because: below_because(&box_marks, &scores),
                scores,
            }),
            scores => {
                let admitted = admit_frame(session, loaded, "sample", &local, path)?;
                if let Some(scores) = scores {
                    box_frames.push(ScoredFrame {
                        frame_id: Some(admitted.frame_id.clone()),
                        sample: Some(index),
                        superseded: None,
                        step: None,
                        taken: BoxFrameTaken::Added,
                        because: None,
                        scores,
                    });
                }
                samples.push(admitted);
            }
        }
    }
    let mut live = live_existing.clone();
    live.extend(samples.iter().cloned());

    // effect: a click or an application operation; #463 R1: or a select, which declares the click
    // it presses (its click guard then applies to it)
    let mut select_fresh = false;
    // #463 R1 fix 1: a select taken off first, its executed click given back what it was.
    if request.clear_select {
        clear_select(step_mut(&mut session.recording, target)?)?;
    }
    if request.application.is_some() {
        apply_application(session, request, target)?;
    } else {
        select_fresh = declare_select(step_mut(&mut session.recording, target)?, request)?;
        apply_click(session, request, target, &batch)?;
    }
    apply_optional(&mut session.recording, request, target)?;
    // #463 S2-6: a choice marked first, or its mark taken off; the last step of a stopped
    // recording holds none.
    let final_of_stopped = session.stopped && last_effective(&session.recording) == Some(target);
    apply_choice(
        step_mut(&mut session.recording, target)?,
        request,
        final_of_stopped,
    )?;
    // #463 R1: a select step keeps its rules whatever this command changed; #463 S2-6: so does a
    // step marked as holding a choice, which a select refines.
    check_select_rules(effective_step(&session.recording, target)?)?;

    // #463 S2-5: the step's page as this command leaves it, which the self-test below and the
    // check of each capture stage one stored (fix round 2: one function, `page_check`) test.
    let page = PageInputs {
        target,
        batch: &batch,
        forbidden: &forbidden,
        anchor: anchor.as_ref(),
        select_fresh,
    };
    // #463 S2-5: beside recognize boxes, the captures stage one stored for this step before its
    // press are frames of the action too: one whose whole frame has not changed from the step's
    // live frame (fix round 2), on which every box reaches its threshold and on which the step's
    // whole page holds becomes a sample; any other is listed and is no refusal.
    if !box_marks.is_empty() && rejections.is_empty() {
        let (admitted, scored) =
            admit_captures(session, &box_marks, &page, &primary_sha, &live, ocr)?;
        box_frames.extend(scored);
        live.extend(admitted.iter().cloned());
        samples.extend(admitted);
    }

    // self-test: the step's page on every live frame (`page_check`), the select's dry run only once
    // nothing else was refused.
    let has_select = effective_step(&session.recording, target)?.select.is_some();
    let total = batch.own.len()
        + batch.reused.len()
        + forbidden.len()
        + usize::from(has_select)
        + flat_count;
    let checked = page_check(session, &page, &live, !rejections.is_empty(), ocr)?;
    rejections.extend(checked.refusals);
    if !rejections.is_empty() {
        return Err(rejected(&rejections, total));
    }
    // #463 S2-5: each recognize box's score on every frame the step held before this command, then
    // on the frames of the action, then on the live frames of the other steps where it reaches its
    // threshold.
    let boxes = if box_marks.is_empty() {
        box_views(
            &request.boxes,
            target,
            &[],
            &[],
            &session.recording.defaults,
        )
    } else {
        let mut scored = scored_frames(session, &box_marks, &live_existing, ocr)?;
        scored.extend(box_frames);
        scored.extend(elsewhere_frames(session, &box_marks, target, ocr)?);
        box_views(
            &request.boxes,
            target,
            &box_marks,
            &scored,
            &session.recording.defaults,
        )
    };
    queue_crops(session, &batch);
    let added = checked
        .own
        .iter()
        .filter(|mark| batch.added_ids.contains(&mark.id))
        .cloned()
        .collect::<Vec<_>>();
    let step = step_mut(&mut session.recording, target)?;
    step.marks = checked.own;
    step.reused = batch.reused;
    step.frames.extend(samples.iter().cloned());
    step.forbidden = forbidden;
    step.anchor = anchor;
    // #463 L2: a click that follows a located-text mark is the box the primary frame gives it;
    // fix 1: and it says whether its names were changed after its press.
    if let (Some(located), Some(click)) = (checked.located, step.click.as_mut()) {
        click.rect = located.rect;
        if let Some(follow) = click.follow.as_mut() {
            follow.offset = located.offset;
            follow.names_changed_after_press = located.names_changed_after_press;
        }
    }
    // #463 R1: the select with its self-test, and its click the slot the pressed name selects on
    // the primary frame (kept as it was without an OCR model).
    if let Some((select, click_rect)) = checked.select {
        step.select = Some(select);
        if let (Some(rect), Some(click)) = (click_rect, step.click.as_mut()) {
            click.rect = rect;
        }
    }
    removed.extend(removed_forbidden);
    Ok(MarkApplied {
        step: target,
        opened,
        closed_step,
        frame: new_frame,
        samples,
        added,
        reused: checked.reused,
        removed,
        forbidden: checked.forbidden,
        boxes,
    })
}

/// #463 S2-5: what the step's page is, as this command leaves it, for the check of a frame of the
/// action: its target step, the batch with its new crops, the marks its page forbids, its anchor,
/// and whether its select is declared by this command.
struct PageInputs<'a> {
    target: u32,
    batch: &'a MarkBatch,
    forbidden: &'a [String],
    anchor: Option<&'a String>,
    select_fresh: bool,
}

/// #463 S2-5 fix round 2: what the step's page, as this command leaves it, gives on `frames`: its
/// own and reused marks with their self-tests, the click a followed located-text mark gives it,
/// the forbidden marks' views, the select with its self-test and the click box it gives, and every
/// refusal in the self-test's order.
struct PageCheck {
    own: Vec<RecordedMark>,
    reused: Vec<RecordedMark>,
    located: Option<LocatedClick>,
    forbidden: Vec<ForbiddenMarkView>,
    select: Option<(StepSelect, Option<RecordRect>)>,
    refusals: Vec<MarkRejection>,
}

/// The mark-time self-test of the step's page `page` on `frames`, the one function that decides
/// whether a page holds on frames (#463 S2-5 fix round 2: `apply_marks` runs it on the step's live
/// frames, and the check of a capture stage one stored on those and the capture): every own and
/// reused mark (`evaluate_batch`); Workflow #396 REC-4 and #463 L2: a click that follows a template
/// or a located-text mark holds on every frame (`follow_frames`); Workflow #390 LAB-2: the anchor
/// is a required own template mark and no forbidden mark passes (`anchor_rejection`,
/// `evaluate_forbidden`); #463 R1: once nothing is refused, `refused_before` included (refusals the
/// command met before its self-test), a select's dry run of the step as `record stop` generates it
/// (`select_frames`), and #463 R1 fix 1, once that passed, the pages of the other steps this mark
/// knows, which must not pass on the select's frames (`other_pages_on_select_frames`; the pages of
/// steps recorded later are `record stop`'s, self-check item 8). The refusals are in that order;
/// none when the whole page holds on every frame.
fn page_check(
    session: &Session,
    page: &PageInputs<'_>,
    frames: &[RecordedFrame],
    refused_before: bool,
    ocr: &LabOcr,
) -> LabResult<PageCheck> {
    let result = evaluate_batch(session, page.batch, frames, ocr)?;
    let followed = follow_frames(session, page.batch, &result, page.target, frames, ocr)?;
    let anchor = anchor_rejection(page.anchor, &result.own, &result.reused);
    let (forbidden, forbidden_refusals) = evaluate_forbidden(session, page.forbidden, frames, ocr)?;
    let BatchResult {
        own,
        reused,
        rejections: mut refusals,
    } = result;
    refusals.extend(followed.rejection);
    refusals.extend(anchor);
    refusals.extend(forbidden_refusals);
    let mut select = None;
    if !refused_before
        && refusals.is_empty()
        && let Some(checked) = select_frames(
            session,
            SelectInputs {
                target: page.target,
                batch: page.batch,
                own: &own,
                reused: &reused,
                forbidden: page.forbidden,
                anchor: page.anchor,
            },
            frames,
            page.select_fresh,
            ocr,
        )?
    {
        let SelectChecked {
            select: checked_select,
            click_rect,
            rejections: refused,
        } = checked;
        refusals.extend(refused);
        select = Some((checked_select, click_rect));
    }
    if !refused_before
        && refusals.is_empty()
        && select
            .as_ref()
            .is_some_and(|(made, _)| made.self_test.status == SelfTestStatus::Passed)
    {
        refusals.extend(other_pages_on_select_frames(
            session,
            page.target,
            frames,
            ocr,
        )?);
    }
    Ok(PageCheck {
        own,
        reused,
        located: followed.located,
        forbidden,
        select,
        refusals,
    })
}

/// #463 S2-5 fix round 2 (the coordinator's ruling: a base image is a frame of the same state of
/// the screen): whether the capture whose frame-change digest is `capture` shows the state of the
/// step's live frame (`live`), and the measure's value. Stage one calls two captures unchanged when
/// the newer differs from the older by at most the Runtime's one frame-change bound
/// (`frame_change_milli` of the whole frame at most `FRAME_CHANGE_MAX_MILLI`); its after loop asks
/// [`unchanged_for`], which, given the two at one instant and no time to stand, decides exactly
/// that, so it is asked here, the capture the older and the live frame the newer. Digests that
/// cannot be compared are `record_frame_unreadable`, as there.
fn frame_unchanged(capture: &ColorDigest, live: &ColorDigest) -> LabResult<(bool, u32)> {
    let milli = frame_change_milli(capture, live).map_err(|error| {
        invalid(
            "record_frame_unreadable",
            format!("failed to compare two frames by the frame-change measure: {error}"),
        )
    })?;
    let unchanged = unchanged_for(
        &[
            (Duration::ZERO, capture.clone()),
            (Duration::ZERO, live.clone()),
        ],
        Duration::ZERO,
    )?;
    Ok((unchanged, milli))
}

/// #463 S2-5: the captures stage one stored for step `page.target` before its press
/// ([`stage_one_captures`]), each scored by the recognize boxes `marks`: one whose bytes are those
/// of a live frame is that frame already (`own`). Fix round 2: of the others, first, one whose
/// whole frame changed from the step's live primary frame (`primary_sha`) as stage one judges two
/// captures changed ([`frame_unchanged`]) shows another state of the screen (`not_added`,
/// `frame_changed` with the measure's value), whatever its scores; then one on which a box is below
/// its threshold is another state of the screen (`not_added`, `below_threshold`); one on which
/// every box reaches it becomes a sample when the step's whole page holds on the live frames `live`
/// and it ([`page_check`]), and otherwise is `not_added` with the first refusal. A capture whose
/// bytes are those of an earlier capture of the list is what that one became: `own` when it was
/// added, else as it. Should two of them hold one by one and not together (a check whose `any_of`
/// members pass on different frames), they are taken one after the other, as long as the page
/// holds. Nothing here is a refusal of the command but a score that cannot be computed. A sample
/// is the stored frame itself (same file, sha256 and provenance, `source: "recording_frame"`),
/// with a frame id of its own; a capture that is not taken leaves the session's cache again.
fn admit_captures(
    session: &mut Session,
    marks: &[BoxMark],
    page: &PageInputs<'_>,
    primary_sha: &str,
    live: &[RecordedFrame],
    ocr: &LabOcr,
) -> LabResult<(Vec<RecordedFrame>, Vec<ScoredFrame>)> {
    let captures = stage_one_captures(&session.recording, page.target);
    if captures.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let live_digest = cached_digest(session, primary_sha, "the step's live frame")?;
    let mut scored = Vec::new();
    // The captures that hold, as the probes they were tested as, with their place in `scored`.
    let mut chosen: Vec<(usize, RecordedFrame)> = Vec::new();
    // The place in `scored` of the first capture of each sha256 that is no live frame, and the
    // places of the later captures of the same bytes with the place of that first one.
    let mut first_of: BTreeMap<String, usize> = BTreeMap::new();
    let mut copies: Vec<(usize, usize)> = Vec::new();
    let mut loaded_here = Vec::new();
    for capture in captures {
        if !session.cache.contains_key(&capture.sha256) {
            let loaded = read_stored_frame(&capture)?;
            session.cache.insert(capture.sha256.clone(), loaded);
            loaded_here.push(capture.sha256.clone());
        }
        let scores = {
            let loaded = session
                .cache
                .get(&capture.sha256)
                .ok_or_else(|| invalid("validation_failed", "frame was not loaded"))?;
            let frame = EvalFrame {
                id: capture.frame_id.as_str(),
                frame: loaded,
            };
            match call_frame(marks, &frame, &session.recording.defaults, ocr)? {
                Ok(scores) => scores,
                Err(rejection) => return Err(rejected(&[rejection], marks.len())),
            }
        };
        let same = live
            .iter()
            .find(|frame| frame.sha256 == capture.sha256)
            .map(|frame| frame.frame_id.clone());
        let earlier = first_of.get(&capture.sha256).copied();
        if same.is_none() && earlier.is_none() {
            first_of.insert(capture.sha256.clone(), scored.len());
        }
        let (taken, because) = if same.is_some() {
            (BoxFrameTaken::Own, None)
        } else if let Some(origin) = earlier {
            copies.push((scored.len(), origin));
            (BoxFrameTaken::NotAdded, None)
        } else {
            let label = format!("capture {}", capture.frame_id);
            let digest = cached_digest(session, &capture.sha256, &label)?;
            let (unchanged, milli) = frame_unchanged(&digest, &live_digest)?;
            if !unchanged {
                (BoxFrameTaken::NotAdded, Some(changed_because(milli)))
            } else if scores.below.is_some() {
                (BoxFrameTaken::NotAdded, below_because(marks, &scores))
            } else {
                let probe = RecordedFrame {
                    role: "sample".to_string(),
                    superseded: false,
                    ..capture.clone()
                };
                let frames = live
                    .iter()
                    .cloned()
                    .chain(std::iter::once(probe.clone()))
                    .collect::<Vec<_>>();
                match page_check(session, page, &frames, false, ocr)?
                    .refusals
                    .first()
                {
                    None => {
                        chosen.push((scored.len(), probe));
                        (BoxFrameTaken::Added, None)
                    }
                    Some(refusal) => (BoxFrameTaken::NotAdded, Some(page_because(refusal))),
                }
            }
        };
        scored.push(ScoredFrame {
            frame_id: same,
            sample: None,
            superseded: Some(capture.frame_id),
            step: None,
            taken,
            because,
            scores,
        });
    }
    if chosen.len() > 1 {
        let together = live
            .iter()
            .cloned()
            .chain(chosen.iter().map(|(_, probe)| probe.clone()))
            .collect::<Vec<_>>();
        if !page_check(session, page, &together, false, ocr)?
            .refusals
            .is_empty()
        {
            let mut kept: Vec<(usize, RecordedFrame)> = Vec::new();
            for (place, probe) in chosen {
                let frames = live
                    .iter()
                    .cloned()
                    .chain(kept.iter().map(|(_, held)| held.clone()))
                    .chain(std::iter::once(probe.clone()))
                    .collect::<Vec<_>>();
                match page_check(session, page, &frames, false, ocr)?
                    .refusals
                    .first()
                {
                    None => kept.push((place, probe)),
                    Some(refusal) => {
                        if let Some(entry) = scored.get_mut(place) {
                            entry.taken = BoxFrameTaken::NotAdded;
                            entry.because = Some(page_because(refusal));
                        }
                    }
                }
            }
            chosen = kept;
        }
    }
    let mut admitted = Vec::new();
    for (place, probe) in chosen {
        let sample = recorded_sample(session, &probe)?;
        if let Some(entry) = scored.get_mut(place) {
            entry.frame_id = Some(sample.frame_id.clone());
        }
        admitted.push(sample);
    }
    // A capture of the same bytes as an earlier one is what that one became: the sample it was
    // added as, a live frame now (`own`), or not added for the same reason.
    for (place, origin) in copies {
        let Some(first) = scored.get(origin) else {
            continue;
        };
        let (taken, because, frame_id) = match first.taken {
            BoxFrameTaken::Added => (BoxFrameTaken::Own, None, first.frame_id.clone()),
            taken => (taken, first.because.clone(), None),
        };
        if let Some(entry) = scored.get_mut(place) {
            entry.taken = taken;
            entry.because = because;
            entry.frame_id = frame_id;
        }
    }
    for sha256 in loaded_here {
        if !admitted.iter().any(|frame| frame.sha256 == sha256) {
            session.cache.remove(&sha256);
        }
    }
    Ok((admitted, scored))
}

/// #463 S2-5: why a frame of the action was not added because the step's page does not hold on
/// it: the refusal's own reason and the mark it names.
fn page_because(refusal: &MarkRejection) -> BoxBecause {
    BoxBecause {
        reason: refusal.reason.clone(),
        mark: Some(refusal.id.clone()),
        index: None,
        frame_change_milli: None,
    }
}

/// #463 S2-5 fix round 2: why a capture was not added because its whole frame changed from the
/// step's live frame: `frame_changed`, with the measure's value; no mark is involved.
fn changed_because(milli: u32) -> BoxBecause {
    BoxBecause {
        reason: FRAME_CHANGED.to_owned(),
        mark: None,
        index: None,
        frame_change_milli: Some(milli),
    }
}

/// #463 S2-5 fix round 2: the frame-change digest ([`change_digest`], stage one's) of the frame
/// `sha256` the session holds decoded, named `label`.
fn cached_digest(session: &Session, sha256: &str, label: &str) -> LabResult<ColorDigest> {
    let loaded = session
        .cache
        .get(sha256)
        .ok_or_else(|| invalid("validation_failed", format!("{label} was not loaded")))?;
    change_digest(loaded, label)
}

/// #463 S2-5: a capture of the step admitted as its sample: the stored frame itself (its file,
/// sha256, size and provenance), role `sample`, `source: "recording_frame"`, a frame id of its own
/// and the time it was admitted. Its file is already in the recording's frame store, so nothing
/// is written for it.
fn recorded_sample(session: &mut Session, capture: &RecordedFrame) -> LabResult<RecordedFrame> {
    let frame_id = format!("f{:04}", session.next_frame);
    session.next_frame = session
        .next_frame
        .checked_add(1)
        .ok_or_else(|| invalid("validation_failed", "frame id overflow"))?;
    Ok(RecordedFrame {
        frame_id,
        role: "sample".to_string(),
        source: RECORDING_FRAME_SOURCE.to_string(),
        recorded_at_unix_ms: now_unix_ms(),
        superseded: false,
        ..capture.clone()
    })
}

/// #463 S2-5: every recognize box's score on each frame the step held before this command (its
/// own). The self-test passed on them, so a score that cannot be computed there is refused as it
/// is.
fn scored_frames(
    session: &Session,
    marks: &[BoxMark],
    frames: &[RecordedFrame],
    ocr: &LabOcr,
) -> LabResult<Vec<ScoredFrame>> {
    let mut scored = Vec::new();
    for entry in frames {
        let loaded = session
            .cache
            .get(&entry.sha256)
            .ok_or_else(|| invalid("validation_failed", "frame was not loaded"))?;
        let frame = EvalFrame {
            id: entry.frame_id.as_str(),
            frame: loaded,
        };
        let scores = match call_frame(marks, &frame, &session.recording.defaults, ocr)? {
            Ok(scores) => scores,
            Err(rejection) => return Err(rejected(&[rejection], marks.len())),
        };
        scored.push(ScoredFrame {
            frame_id: Some(entry.frame_id.clone()),
            sample: None,
            superseded: None,
            step: None,
            taken: BoxFrameTaken::Own,
            because: None,
            scores,
        });
    }
    Ok(scored)
}

/// #463 S2-5: the live frames of the other effective steps on which a recognize box of the
/// request reaches its threshold (`elsewhere`): the box does not tell this step's screen from
/// those. They are never samples of this step and are read from the frame store without being
/// kept.
fn elsewhere_frames(
    session: &Session,
    marks: &[BoxMark],
    target: u32,
    ocr: &LabOcr,
) -> LabResult<Vec<ScoredFrame>> {
    let mut scored = Vec::new();
    for step in session
        .recording
        .steps
        .iter()
        .filter(|step| step.is_effective() && step.index != target)
    {
        for entry in step.frames.iter().filter(|frame| !frame.superseded) {
            let read;
            let loaded = match session.cache.get(&entry.sha256) {
                Some(loaded) => loaded,
                None => {
                    read = read_stored_frame(entry)?;
                    &read
                }
            };
            let frame = EvalFrame {
                id: entry.frame_id.as_str(),
                frame: loaded,
            };
            let scores = match call_frame(marks, &frame, &session.recording.defaults, ocr)? {
                Ok(scores) => scores,
                Err(rejection) => return Err(rejected(&[rejection], marks.len())),
            };
            if scores.scores.iter().any(|score| score.reaches) {
                scored.push(ScoredFrame {
                    frame_id: Some(entry.frame_id.clone()),
                    sample: None,
                    superseded: None,
                    step: Some(step.index),
                    taken: BoxFrameTaken::Elsewhere,
                    because: None,
                    scores,
                });
            }
        }
    }
    Ok(scored)
}

/// #463 R2: what `record mark --ending` took: the ending as the recording now holds it, its frames
/// as admitted, its new marks and the reused ones with their self-tests.
pub(crate) struct EndingApplied {
    pub(crate) ending: RecordingEnding,
    pub(crate) frame: RecordedFrame,
    pub(crate) samples: Vec<RecordedFrame>,
    pub(crate) added: Vec<RecordedMark>,
    pub(crate) reused: Vec<RecordedMark>,
}

/// #463 R2 (`contracts/lab-recording.md`, "Endings"): a stopped recording takes the ending
/// `spec`: its frame and samples, files on the caller's machine, are admitted as a step's are (one
/// size, content-addressed in the recording's `frames/`, never named by the caller's path; fix 1: a
/// frame of a pure scale of the recording is converted to its size first, `scaled_from`), its
/// marks are made from its frame and self-tested with its reused marks on its frames as any mark is
/// (`record_mark_rejected` when one fails), and the ending joins `endings[]`. An active recording
/// takes none (`recording_active`); its key and page name are its own ([`check_new_ending`]).
pub(crate) fn apply_ending(
    session: &mut Session,
    spec: &EndingSpec,
    ocr: &LabOcr,
) -> LabResult<EndingApplied> {
    check_new_ending(session, &spec.outcome_key, spec.page.as_deref())?;
    let local = FrameMeta::local();
    // Fix 1 (the coordinator's ruling): a frame of a pure scale of the recording is converted to
    // the recording's size as the Runtime converts it, and keeps the size it was given at.
    let (loaded, scaled_from) = scaled_to_recording(
        session.recording.coordinate_space,
        read_frame_file(&spec.frame)?,
        &spec.frame,
    )?;
    let primary_sha = loaded.sha256.clone();
    let mut frame = admit_frame(session, loaded, "primary", &local, &spec.frame)?;
    frame.scaled_from = scaled_from;
    let mut samples = Vec::new();
    for path in &spec.samples {
        let (loaded, scaled_from) = scaled_to_recording(
            session.recording.coordinate_space,
            read_frame_file(path)?,
            path,
        )?;
        let mut sample = admit_frame(session, loaded, "sample", &local, path)?;
        sample.scaled_from = scaled_from;
        samples.push(sample);
    }
    let mut batch = MarkBatch {
        own: Vec::new(),
        reused: Vec::new(),
        new_crops: BTreeMap::new(),
        added_ids: BTreeSet::new(),
    };
    let mut rejections = Vec::new();
    add_marks(
        session,
        &mut batch,
        &spec.add,
        &primary_sha,
        ENDING_MARK_STEP,
        None,
        &BTreeSet::new(),
        &mut rejections,
    )?;
    add_reuse(session, &mut batch, &spec.reuse, None)?;
    let mut live = vec![frame.clone()];
    live.extend(samples.iter().cloned());
    let total = batch.own.len() + batch.reused.len();
    let result = evaluate_batch(session, &batch, &live, ocr)?;
    rejections.extend(result.rejections);
    if !rejections.is_empty() {
        return Err(rejected(&rejections, total));
    }
    queue_crops(session, &batch);
    let ending = RecordingEnding {
        outcome_key: spec.outcome_key.clone(),
        page: spec.page.clone(),
        frames: live,
        marks: result.own,
        reused: batch.reused,
        marked_at_unix_ms: now_unix_ms(),
        initiator: session.initiator.clone(),
    };
    session.recording.endings.push(ending.clone());
    Ok(EndingApplied {
        added: ending.marks.clone(),
        reused: result.reused,
        ending,
        frame,
        samples,
    })
}

/// #463 R4 (`contracts/lab-recording.md`, "Inserting an optional step after stop"): a stopped
/// recording takes a pop-up a run met after the effect of step `spec.step` as an optional step,
/// without recording again. Its frame and samples, files on the caller's machine, are admitted as an
/// ending's are ([`apply_ending`]: one size, a pure scale converted with `scaled_from`,
/// content-addressed in the recording); the step takes the next serial number and
/// `inserted_after`, the recorded step it runs after (the step named, or the recorded step an
/// inserted step named runs after, whose run it joins), and is closed as `--close-step` closes a
/// step whose click was declared and never executed. Its page, marks, click and settle then go
/// through the marks flow of `record mark --step <n>` ([`apply_marks`]), so every mark is
/// self-tested on its frames and every rule of a click and an optional step holds. An active
/// recording takes none (`recording_active`); the named step is a step of the recording
/// (`step_unknown`), not its last step (`step_last`) and no application step (`step_application`).
/// Returns the marks flow's result on the new step, its frame and samples as admitted.
pub(crate) fn apply_optional_after(
    session: &mut Session,
    spec: &OptionalAfterSpec,
    ocr: &LabOcr,
) -> LabResult<MarkApplied> {
    let named = spec.step;
    if !session.stopped {
        return Err(inserted_recording_active(named));
    }
    // #463 S2-8': a swipe folded into a press step's scrolling search is no step a run presses
    // after; the fold is undone first.
    if let Some(press) = step_ref(&session.recording, named).and_then(|step| step.folded_into) {
        return Err(in_fold(named, press));
    }
    let Some(step) = step_ref(&session.recording, named).filter(|step| step.is_effective()) else {
        return Err(optional_after_invalid(
            OptionalAfterProblem::StepUnknown,
            Some(named),
            format!(
                "step {named} is no step of the recording (none, or a dropped or converted one); \
                 record status lists the steps in the order they run; nothing was written"
            ),
            json!({}),
        ));
    };
    let after = step.inserted_after.unwrap_or(named);
    let application = step
        .application
        .as_ref()
        .map(|application| application.action.clone());
    if last_effective(&session.recording) == Some(named) {
        return Err(optional_after_invalid(
            OptionalAfterProblem::StepLast,
            Some(named),
            format!(
                "step {named} is the last step: every run ends on its page and presses nothing \
                 after it; a screen after the last press is an ending (record mark --ending); \
                 nothing was written"
            ),
            json!({}),
        ));
    }
    if let Some(action) = application {
        return Err(optional_after_invalid(
            OptionalAfterProblem::StepApplication,
            Some(named),
            format!(
                "step {named} is an application step ({action}); an optional step is inserted \
                 after a press; nothing was written"
            ),
            json!({"application": action}),
        ));
    }
    let local = FrameMeta::local();
    let (loaded, scaled_from) = scaled_to_recording(
        session.recording.coordinate_space,
        read_frame_file(&spec.frame)?,
        &spec.frame,
    )?;
    let mut frame = admit_frame(session, loaded, "primary", &local, &spec.frame)?;
    frame.scaled_from = scaled_from;
    let mut samples = Vec::new();
    for path in &spec.samples {
        let (loaded, scaled_from) = scaled_to_recording(
            session.recording.coordinate_space,
            read_frame_file(path)?,
            path,
        )?;
        let mut sample = admit_frame(session, loaded, "sample", &local, path)?;
        sample.scaled_from = scaled_from;
        samples.push(sample);
    }
    let index = next_step_index(&session.recording)?;
    let mut inserted = new_step(
        index,
        std::iter::once(frame.clone())
            .chain(samples.iter().cloned())
            .collect(),
    );
    inserted.inserted_after = Some(after);
    inserted.closed = true;
    inserted.closed_by = Some("author".to_string());
    session.recording.steps.push(inserted);
    let request = MarkRequest {
        schema_version: LAB_RECORD_MARK_SCHEMA.to_string(),
        step: Some(index),
        page: spec.page.clone(),
        add: spec.add.clone(),
        reuse: spec.reuse.clone(),
        click: spec.click.clone(),
        optional: Some(true),
        optional_settle_ms: spec.settle_ms,
        ..MarkRequest::default()
    };
    let mut applied = apply_marks(session, &request, ocr)?;
    applied.frame = Some(frame);
    applied.samples = samples;
    Ok(applied)
}

/// #463 R4: `record mark --remove-inserted <n>`: the stopped recording gives up its inserted
/// optional step `n`, dropped as `--drop-step` drops a step: it stays in the file with its frames,
/// its marks free their ids, and the next `record stop` generates the package without it. A step
/// that is no inserted step, or one given up already, is `not_inserted`; a mark of it another step
/// or an ending reuses, or another page forbids, is `record_mark_in_use`, as for `--drop-step`.
pub(crate) fn remove_inserted(session: &mut Session, index: u32) -> LabResult<()> {
    if !session.stopped {
        return Err(inserted_recording_active(index));
    }
    let recording = &session.recording;
    let Some(step) = step_ref(recording, index)
        .filter(|step| step.is_effective() && step.inserted_after.is_some())
    else {
        return Err(not_inserted(recording, index));
    };
    let ids = step
        .marks
        .iter()
        .map(|mark| mark.id.clone())
        .collect::<BTreeSet<_>>();
    let used_by_others = recording
        .steps
        .iter()
        .filter(|other| other.is_effective() && other.index != index)
        .flat_map(|other| {
            let transition_reused = match &other.transition {
                Some(StepTransition::Page { reused, .. }) => reused.as_slice(),
                _ => &[][..],
            };
            other
                .reused
                .iter()
                .chain(transition_reused)
                .chain(&other.forbidden)
        })
        .chain(
            recording
                .endings
                .iter()
                .flat_map(|ending| ending.reused.iter()),
        )
        .filter(|id| ids.contains(*id))
        .cloned()
        .collect::<BTreeSet<_>>();
    if !used_by_others.is_empty() {
        return Err(with_details(
            blocked(
                "record_mark_in_use",
                format!("marks of step {index} are reused by other steps; nothing was written"),
            ),
            json!({"step": index, "marks": used_by_others}),
        ));
    }
    step_mut(&mut session.recording, index)?.dropped = true;
    Ok(())
}

/// #463 R4: `remove_inserted` naming no inserted step that is not given up yet: `not_inserted`,
/// with the inserted steps there are.
fn not_inserted(recording: &LabRecording, index: u32) -> LabError {
    let inserted = recording
        .steps
        .iter()
        .filter(|step| step.is_effective() && step.inserted_after.is_some())
        .map(|step| step.index)
        .collect::<Vec<_>>();
    optional_after_invalid(
        OptionalAfterProblem::NotInserted,
        Some(index),
        format!(
            "step {index} is no inserted optional step of the recording, or it was given up \
             already; nothing was written"
        ),
        json!({"inserted": inserted}),
    )
}

/// #463 R1: the step a select's self-test probes, as this command leaves it: its target, the
/// batch with its new crops, its own and reused marks after their self-tests, the marks its page
/// forbids and its anchor.
struct SelectInputs<'a> {
    target: u32,
    batch: &'a MarkBatch,
    own: &'a [RecordedMark],
    reused: &'a [RecordedMark],
    forbidden: &'a [String],
    anchor: Option<&'a String>,
}

/// #463 R1: the self-test of the select of the step `inputs.target`, when it has one, on its live
/// frames `live` ([`check_step_select`]): the probe package holds the step with the marks this
/// command leaves it, every mark its page names and their template crops.
fn select_frames(
    session: &Session,
    inputs: SelectInputs<'_>,
    live: &[RecordedFrame],
    fresh: bool,
    ocr: &LabOcr,
) -> LabResult<Option<SelectChecked>> {
    let step = effective_step(&session.recording, inputs.target)?;
    if step.select.is_none() {
        return Ok(None);
    }
    let mut probe_step = step.clone();
    probe_step.marks = inputs.own.to_vec();
    probe_step.reused = inputs.batch.reused.clone();
    probe_step.forbidden = inputs.forbidden.to_vec();
    probe_step.anchor = inputs.anchor.cloned();
    let marks = probe_marks(inputs.own, inputs.reused, inputs.forbidden, |id| {
        find_step_mark(&session.recording, id)
    })?;
    let mut crops = BTreeMap::new();
    for mark in marks
        .iter()
        .filter(|mark| mark.family == MarkFamily::Template)
    {
        let bytes = match inputs.batch.new_crops.get(&mark.id) {
            Some(bytes) => bytes.clone(),
            None => stored_crop(mark)?.ok_or_else(|| {
                invalid(
                    "validation_failed",
                    format!("template '{}' has no crop", mark.id),
                )
            })?,
        };
        crops.insert(mark.id.clone(), bytes);
    }
    let frames = live
        .iter()
        .map(|entry| {
            session
                .cache
                .get(&entry.sha256)
                .map(|loaded| SelectFrame { entry, loaded })
                .ok_or_else(|| invalid("validation_failed", "frame was not loaded"))
        })
        .collect::<LabResult<Vec<_>>>()?;
    check_step_select(
        &session.recording,
        session.stopped,
        SelectProbe {
            step: probe_step,
            marks,
            crops,
        },
        &frames,
        fresh,
        ocr,
    )
}

/// #463 R1 fix 1: how a page stands on one frame, each of its marks evaluated as its self-test
/// evaluates it.
enum PageOnFrame {
    /// Every mark the page requires passes and none it forbids does.
    Passes,
    /// A mark it requires fails, or one it forbids passes.
    Fails,
    /// No mark decides it there (an OCR mark without a model, or a mark no effective step holds,
    /// which `record stop` refuses as `reuse_source_missing`).
    Undetermined,
    /// A mark's evaluation failed on the frame.
    Error { mark_id: String, error: Value },
}

/// #463 R1 fix 1: the page `required` and `forbidden` name, on `frame` ([`mark_on_frame`]).
fn page_on_frame(
    session: &Session,
    marks: &BTreeMap<String, RecordedMark>,
    (required, forbidden): (&[String], &[String]),
    frame: &EvalFrame<'_>,
    ocr: &LabOcr,
) -> LabResult<PageOnFrame> {
    let mut undetermined = false;
    for (id, wanted) in required
        .iter()
        .map(|id| (id, SelfTestStatus::Passed))
        .chain(forbidden.iter().map(|id| (id, SelfTestStatus::Failed)))
    {
        let Some(mark) = marks.get(id) else {
            return Ok(PageOnFrame::Undetermined);
        };
        match mark_on_frame(session, mark, frame, ocr)? {
            (_, Some(error)) => {
                return Ok(PageOnFrame::Error {
                    mark_id: id.clone(),
                    error,
                });
            }
            (SelfTestStatus::NotEvaluated, None) => undetermined = true,
            (status, None) if status != wanted => return Ok(PageOnFrame::Fails),
            (_, None) => {}
        }
    }
    Ok(if undetermined {
        PageOnFrame::Undetermined
    } else {
        PageOnFrame::Passes
    })
}

/// #463 R1 fix 1: the page of every other effective step this mark knows (the steps before it,
/// and on a stopped recording every other step), and the page of its page transition, on every
/// live frame `live` of the select step `target`. A page that passes on one is
/// `select_other_page_passes` (`frame_id`, `step`, `transition`), as `record stop`'s item 8 would
/// find on the package (the kernel's `select_page_mismatch`); a mark whose evaluation fails there
/// is `self_test_error`. A page no mark decides there is not refused.
fn other_pages_on_select_frames(
    session: &Session,
    target: u32,
    live: &[RecordedFrame],
    ocr: &LabOcr,
) -> LabResult<Vec<MarkRejection>> {
    let recording = &session.recording;
    let mut marks = BTreeMap::new();
    for step in recording.steps.iter().filter(|step| step.is_effective()) {
        for mark in &step.marks {
            marks.insert(mark.id.clone(), mark.clone());
        }
        if let Some(StepTransition::Page {
            marks: transition_marks,
            ..
        }) = &step.transition
        {
            for mark in transition_marks {
                marks.insert(mark.id.clone(), mark.clone());
            }
        }
    }
    let frames = live
        .iter()
        .map(|entry| {
            session
                .cache
                .get(&entry.sha256)
                .map(|frame| EvalFrame {
                    id: entry.frame_id.as_str(),
                    frame,
                })
                .ok_or_else(|| invalid("validation_failed", "frame was not loaded"))
        })
        .collect::<LabResult<Vec<_>>>()?;
    let mut rejections = Vec::new();
    for other in recording
        .steps
        .iter()
        .filter(|step| step.is_effective() && step.index != target)
    {
        let mut pages = Vec::new();
        if live_primary_sha(other).is_some() {
            pages.push((false, page_required(other, &marks), other.forbidden.clone()));
        }
        if let Some(StepTransition::Page {
            marks: transition_marks,
            reused,
            ..
        }) = &other.transition
        {
            pages.push((
                true,
                required_ids(transition_marks, reused, &marks),
                Vec::new(),
            ));
        }
        for (transition, required, forbidden) in pages {
            if required.is_empty() {
                continue;
            }
            for frame in &frames {
                match page_on_frame(
                    session,
                    &marks,
                    (required.as_slice(), forbidden.as_slice()),
                    frame,
                    ocr,
                )? {
                    PageOnFrame::Passes => rejections.push(MarkRejection::new(
                        SELECT_REJECTION_ID,
                        OTHER_PAGE_PASSES,
                        json!({"frame_id": frame.id, "step": other.index, "transition": transition}),
                    )),
                    PageOnFrame::Error { mark_id, error } => rejections.push(MarkRejection::new(
                        SELECT_REJECTION_ID,
                        SELF_TEST_ERROR,
                        json!({
                            "frame_id": frame.id,
                            "step": other.index,
                            "mark_id": mark_id,
                            "error": error
                        }),
                    )),
                    PageOnFrame::Fails | PageOnFrame::Undetermined => {}
                }
            }
        }
    }
    Ok(rejections)
}

/// Workflow #390 LAB-2: the click declaration names a drag or a long press, or (#411 Lab BD) a
/// brake drag, or (Workflow #406 T4) a path drag.
fn gesture_declared(spec: &ClickSpec) -> bool {
    spec.drag_to.is_some()
        || spec.long_press.is_some()
        || spec.duration_ms.is_some()
        || spec.brake_corner.is_some()
        || spec.path_drag.is_some()
}

/// Workflow #406 T4 (plan Q17): a single-touch path drag in the click rectangle `region`, its
/// box: the box inside the frame with width and height >= 1, and its numbers within the bounds
/// the kernel and pack containment check (`turns` 1..=30, `min_segment_px`
/// 1..=(max(width, height) - 1) / 2, `duration_ms` `turns + 1 ..= (turns + 1) * 60000`,
/// `corner_hold_ms` 0..=60000). Its duration is its own, so it takes no `duration_ms` of the
/// click, and it is no drag, long press, brake drag, source-mark click or followed click.
fn path_drag_declaration(
    spec: &ClickSpec,
    path: super::model::PathDragSpec,
    size: Option<RecordSize>,
) -> LabResult<(RecordRect, ClickGesture)> {
    let conflicting = [
        ("duration_ms", spec.duration_ms.is_some()),
        ("drag_to", spec.drag_to.is_some()),
        ("long_press", spec.long_press.is_some()),
        ("brake_corner", spec.brake_corner.is_some()),
        ("from", spec.from.is_some()),
        ("follow", spec.follow.is_some()),
    ]
    .into_iter()
    .filter_map(|(member, given)| given.then_some(member))
    .collect::<Vec<_>>();
    if let Some(first) = conflicting.first() {
        return Err(with_details(
            invalid(
                "validation_failed",
                "a path drag takes the click rectangle (its box) and its own numbers only; it \
                 takes no duration_ms of the click, drag_to, long_press, brake_corner, from or \
                 follow",
            ),
            json!({
                "argument": first,
                "argument_problem": "conflicting",
                "arguments": conflicting
                    .iter()
                    .map(|member| json!({"argument": member, "argument_problem": "conflicting"}))
                    .collect::<Vec<_>>()
            }),
        ));
    }
    let Some(region) = spec.region else {
        return Err(invalid(
            "validation_failed",
            "a path drag needs the click rectangle, the box it turns in",
        ));
    };
    if !size.is_some_and(|size| rect_inside(region, size)) {
        return Err(invalid(
            "validation_failed",
            "a path drag's box must lie inside the frame with width and height >= 1",
        ));
    }
    if !path_drag_bounds_hold(
        (region.width, region.height),
        path.turns,
        path.min_segment_px,
        path.duration_ms,
        path.corner_hold_ms,
    ) {
        return Err(with_details(
            invalid(
                "validation_failed",
                "a path drag's turns, min_segment_px, duration_ms or corner_hold_ms is outside \
                 its bounds for its box",
            ),
            json!({"argument": "path_drag", "argument_problem": "out_of_range", "path_drag": path}),
        ));
    }
    Ok((
        region,
        ClickGesture::SingleTouchPathDragV1 {
            turns: path.turns,
            min_segment_px: path.min_segment_px,
            duration_ms: path.duration_ms,
            corner_hold_ms: path.corner_hold_ms,
        },
    ))
}

/// #411 Lab BD: a single-touch drag with a vertical brake from the click rectangle `region` to
/// the corner box `brake_corner`, both inside the frame with width and height >= 1 and the corner
/// at least the brake distance (100 px) below the top, so the brake ends inside the frame (pack
/// containment's bound). Its timings are the Runtime's fixed ones, so it takes no duration, and
/// it is no drag, long press, source-mark click or followed click.
fn brake_drag_declaration(
    spec: &ClickSpec,
    corner: RecordRect,
    size: Option<RecordSize>,
) -> LabResult<(RecordRect, ClickGesture)> {
    let conflicting = [
        ("duration_ms", spec.duration_ms.is_some()),
        ("drag_to", spec.drag_to.is_some()),
        ("long_press", spec.long_press.is_some()),
        ("from", spec.from.is_some()),
        ("follow", spec.follow.is_some()),
    ]
    .into_iter()
    .filter_map(|(member, given)| given.then_some(member))
    .collect::<Vec<_>>();
    if let Some(first) = conflicting.first() {
        return Err(with_details(
            invalid(
                "validation_failed",
                "a brake drag (--brake-corner) takes --click x,y,w,h and its corner only; its \
                 timings are fixed, and it takes no --duration-ms, --drag-to, --long-press, \
                 --click-from or --click-follow",
            ),
            json!({
                "argument": first,
                "argument_problem": "conflicting",
                "arguments": conflicting
                    .iter()
                    .map(|member| json!({"argument": member, "argument_problem": "conflicting"}))
                    .collect::<Vec<_>>()
            }),
        ));
    }
    let Some(from) = spec.region else {
        return Err(invalid(
            "validation_failed",
            "a brake drag needs --click x,y,w,h, the rectangle it presses in",
        ));
    };
    let inside = |rect: RecordRect| size.is_some_and(|size| rect_inside(rect, size));
    if !inside(from) || !inside(corner) {
        return Err(invalid(
            "validation_failed",
            "a brake drag's click rectangle and corner must lie inside the frame with width and \
             height >= 1",
        ));
    }
    if corner.y < SEGMENTED_SWIPE_BRAKE_DISTANCE_PX {
        return Err(with_details(
            invalid(
                "validation_failed",
                format!(
                    "a brake drag's corner must start at y >= {SEGMENTED_SWIPE_BRAKE_DISTANCE_PX}, \
                     the brake distance, so that the brake ends inside the frame; got y {}",
                    corner.y
                ),
            ),
            json!({
                "argument": "brake_corner",
                "argument_problem": "out_of_range",
                "range_min": SEGMENTED_SWIPE_BRAKE_DISTANCE_PX
            }),
        ));
    }
    Ok((
        from,
        ClickGesture::SingleTouchDragWithVerticalBrakeV1 { corner },
    ))
}

/// Workflow #390 LAB-2: a drag (`region` to `drag_to`) or a long press (Workflow #390 lpbox: the
/// click box `long_press`, recorded as the step's click rectangle), each over `duration_ms`
/// (1..=60000), with every rectangle inside the frame; it takes no source mark and follows no
/// template.
fn gesture_declaration(
    spec: &ClickSpec,
    size: Option<RecordSize>,
) -> LabResult<(RecordRect, ClickGesture)> {
    let inside = |rect: RecordRect| size.is_some_and(|size| rect_inside(rect, size));
    let duration = |duration_ms: u64| -> LabResult<u64> {
        if !(1..=LAB_RECORDING_MAX_GESTURE_MS).contains(&duration_ms) {
            return Err(with_details(
                invalid(
                    "validation_failed",
                    format!(
                        "--duration-ms must be in 1..={LAB_RECORDING_MAX_GESTURE_MS}, got \
                         {duration_ms}"
                    ),
                ),
                json!({"duration_ms": duration_ms, "max_duration_ms": LAB_RECORDING_MAX_GESTURE_MS}),
            ));
        }
        Ok(duration_ms)
    };
    // Workflow #406 T4: a path drag.
    if let Some(path) = spec.path_drag {
        return path_drag_declaration(spec, path, size);
    }
    // #411 Lab BD: a brake drag.
    if let Some(corner) = spec.brake_corner {
        return brake_drag_declaration(spec, corner, size);
    }
    let source_free = spec.from.is_none() && spec.follow.is_none();
    match (spec.region, spec.drag_to, spec.long_press, spec.duration_ms) {
        (Some(from), Some(to), None, Some(duration_ms)) if source_free => {
            if !inside(from) || !inside(to) {
                return Err(invalid(
                    "validation_failed",
                    "a drag's two rectangles must lie inside the frame with width and height >= 1",
                ));
            }
            Ok((
                from,
                ClickGesture::Drag {
                    to,
                    duration_ms: duration(duration_ms)?,
                },
            ))
        }
        (None, None, Some(rect), Some(duration_ms)) if source_free => {
            if !inside(rect) {
                return Err(invalid(
                    "validation_failed",
                    "the long press box must lie inside the frame with width and height >= 1",
                ));
            }
            Ok((
                rect,
                ClickGesture::LongPress {
                    duration_ms: duration(duration_ms)?,
                },
            ))
        }
        _ => Err(invalid(
            "validation_failed",
            "a drag needs --click x,y,w,h, --drag-to x,y,w,h and --duration-ms; a long press needs \
             --long-press x,y,w,h and --duration-ms; neither takes --click-from or --click-follow",
        )),
    }
}

fn apply_click(
    session: &mut Session,
    request: &super::model::MarkRequest,
    target: u32,
    batch: &MarkBatch,
) -> LabResult<()> {
    let size = session.recording.coordinate_space;
    let own_or_reused = |id: &str| -> Option<RecordedMark> {
        batch
            .own
            .iter()
            .find(|mark| mark.id == id)
            .cloned()
            .or_else(|| {
                batch
                    .reused
                    .iter()
                    .any(|reused| reused == id)
                    .then(|| find_step_mark(&session.recording, id))
                    .flatten()
            })
    };
    let declared = match &request.click {
        None => None,
        // Workflow #390 LAB-2: a drag or a long press.
        Some(spec) if gesture_declared(spec) => {
            let (rect, gesture) = gesture_declaration(spec, size)?;
            Some((rect, "declared", None, None, Some(gesture)))
        }
        Some(spec) => {
            let declared_rect = |rect: &RecordRect| -> LabResult<RecordRect> {
                if !size.is_some_and(|size| rect_inside(*rect, size)) {
                    return Err(invalid(
                        "validation_failed",
                        "the click rectangle must lie inside the frame with width and height >= 1",
                    ));
                }
                Ok(*rect)
            };
            let (rect, source, from, follow) = match (&spec.region, &spec.from, &spec.follow) {
                (Some(rect), None, None) => (declared_rect(rect)?, "declared", None, None),
                // Workflow #396 REC-4: the package's click follows a template mark of the step,
                // its box (`target`) or the declared rectangle's offset from the crop (`offset`).
                // #463 L2: or a located-text mark; its box, and the offset from it, come from the
                // primary frame's evaluation in the self-test (`follow_frames`).
                (region, None, Some(id)) => {
                    let mark = followed_template(target, id, own_or_reused(id))?;
                    let crop = mark.region.ok_or_else(|| {
                        invalid("validation_failed", format!("mark '{id}' has no region"))
                    })?;
                    let located = mark.locates_text();
                    let follow = |form, offset| ClickFollow {
                        mark_id: id.clone(),
                        form,
                        offset,
                        names_changed_after_press: false,
                    };
                    match region {
                        None => (
                            crop,
                            "followed_mark",
                            None,
                            Some(follow(ClickFollowForm::Target, None)),
                        ),
                        Some(rect) => {
                            let rect = declared_rect(rect)?;
                            let offset = (!located).then(|| RecordRect {
                                x: rect.x - crop.x,
                                y: rect.y - crop.y,
                                width: rect.width,
                                height: rect.height,
                            });
                            (
                                rect,
                                "declared",
                                None,
                                Some(follow(ClickFollowForm::Offset, offset)),
                            )
                        }
                    }
                }
                (None, Some(id), None) => {
                    let mark = own_or_reused(id).ok_or_else(|| {
                        blocked(
                            "record_click_source_invalid",
                            format!("click source '{id}' is not a mark of this step"),
                        )
                    })?;
                    let region = mark.region.filter(|_| mark.family != MarkFamily::Check);
                    // #411 Lab B-9: a value test has no region either.
                    let Some(region) = region else {
                        return Err(blocked(
                            "record_click_source_invalid",
                            format!(
                                "click source '{id}' is a {} mark and has no region",
                                mark.family.as_str()
                            ),
                        ));
                    };
                    (region, "from_mark", Some(id.clone()), None)
                }
                _ => {
                    return Err(invalid(
                        "validation_failed",
                        "a click needs exactly one of a region (--click), a source mark \
                         (--click-from) and a template it follows (--click-follow, alone or with \
                         --click)",
                    ));
                }
            };
            Some((rect, source, from, follow, None))
        }
    };
    // #463 L2 fix 1: the mark the click follows after this command, if it follows one.
    let followed_after = match &declared {
        Some((_, _, _, follow, _)) => follow.as_ref().map(|follow| follow.mark_id.clone()),
        None => effective_step(&session.recording, target)?
            .click
            .as_ref()
            .and_then(|click| click.follow.as_ref())
            .map(|follow| follow.mark_id.clone()),
    };
    let guard = match &request.click_guard {
        None => None,
        Some(id) => {
            let mark = own_or_reused(id).ok_or_else(|| {
                blocked(
                    "record_guard_family_invalid",
                    format!("click guard '{id}' is not a mark of this step"),
                )
            })?;
            // #411 Lab B-9: a value test guards the click as a check does. #463 L2 fix 1: the
            // located-text mark the click follows names the guard the package has anyway.
            let followed_text =
                mark.locates_text() && followed_after.as_deref() == Some(id.as_str());
            if mark.family == MarkFamily::Ocr && !followed_text {
                let note = followed_after
                    .as_deref()
                    .map_or_else(String::new, |followed| {
                        format!(
                            "; this click follows '{followed}', which is its guard: leave \
                         --click-guard out or name '{followed}'"
                        )
                    });
                return Err(blocked(
                    "record_guard_family_invalid",
                    format!(
                        "click guard '{id}' is an OCR mark; a guard must be a template, color, \
                         color_digest, check or value_test mark, or the located-text mark the \
                         click follows{note}"
                    ),
                ));
            }
            Some(id.clone())
        }
    };
    if let Some(retry) = &request.retry {
        validate_retry(retry)?;
    }
    let step = step_mut(&mut session.recording, target)?;
    // One step has one effect: a click replaces a declared application operation only with
    // --replace-click; retry and guard belong to a click.
    if let Some(application) = &step.application {
        if declared.is_none() {
            if request.retry.is_some() || guard.is_some() {
                return Err(application_with_click(Some(target)));
            }
        } else if application.executed.is_some() {
            return Err(effect_executed(target, "application"));
        } else if !request.replace_click {
            return Err(effect_exists(
                target,
                "application",
                "add --replace-click to replace the declared one",
            ));
        }
    }
    // #463 S2-5: a press box on a step whose click has an outcome on record does not declare a
    // click: it gives a press recorded by the command that made it its rectangle
    // ([`box_on_recorded_press`]).
    let recorded_box = match (&declared, &step.click) {
        (Some((rect, ..)), Some(click)) if press_rect(&request.boxes).is_some() => click
            .execution
            .as_ref()
            .map(|execution| (*rect, execution.point)),
        _ => None,
    };
    if let (Some(boxed), Some(click)) = (recorded_box, step.click.as_mut()) {
        box_on_recorded_press(target, click, boxed, request.replace_click)?;
        if let Some(retry) = &request.retry {
            click.retry = Some(retry.clone());
        }
    } else if let Some((rect, source, from, follow, gesture)) = declared {
        step.application = None;
        let attempts = match &step.click {
            Some(existing) if existing.execution.is_some() => {
                return Err(with_details(
                    blocked(
                        "record_click_executed",
                        format!(
                            "step {target} already has a click with an outcome on record; \
                             run `record mark --reopen-step {target}` first"
                        ),
                    ),
                    json!({"step": target}),
                ));
            }
            Some(_) if !request.replace_click => {
                return Err(with_details(
                    blocked(
                        "record_click_exists",
                        format!(
                            "step {target} already declares a click; add --replace-click to \
                             replace it"
                        ),
                    ),
                    json!({"step": target}),
                ));
            }
            Some(existing) => existing.attempts.clone(),
            None => Vec::new(),
        };
        let retry = request
            .retry
            .clone()
            .or_else(|| step.click.as_ref().and_then(|click| click.retry.clone()));
        // #463 R1: a click declared in place of a select's (with --replace-click) ends the select.
        step.select = None;
        step.click = Some(StepClick {
            rect,
            source: source.to_string(),
            from,
            follow,
            gesture,
            retry,
            declared_at_unix_ms: now_unix_ms(),
            execution: None,
            attempts,
            needs_review: false,
        });
    } else if let Some(retry) = &request.retry {
        let click = step.click.as_mut().ok_or_else(|| {
            invalid(
                "validation_failed",
                "--click-retry needs a click on the step",
            )
        })?;
        click.retry = Some(retry.clone());
    }
    if let Some(guard) = guard {
        if step.click.is_none() {
            return Err(invalid(
                "validation_failed",
                "--click-guard needs a click on the step",
            ));
        }
        step.click_guard = Some(guard);
    }
    // Workflow #396 REC-4: a following click keeps its declaration rules whatever this command
    // changed (a new guard, a new any_of check).
    let step = effective_step(&session.recording, target)?;
    if let Some(follow) = step.click.as_ref().and_then(|click| click.follow.as_ref()) {
        let marks = batch
            .own
            .iter()
            .cloned()
            .chain(
                batch
                    .reused
                    .iter()
                    .filter_map(|id| find_step_mark(&session.recording, id)),
            )
            .collect::<Vec<_>>();
        followed_template(
            target,
            &follow.mark_id,
            marks.iter().find(|mark| mark.id == follow.mark_id).cloned(),
        )?;
        check_follow_declaration(step, follow, &marks)?;
    } else if let Some(guard) = step.click_guard.as_deref() {
        // #463 L2 fix 1: a click guard names an OCR mark only as the located-text mark the click
        // follows; a click that no longer follows it cannot keep it.
        let family = match batch.own.iter().find(|mark| mark.id == guard) {
            Some(mark) => Some(mark.family),
            None => find_step_mark(&session.recording, guard)
                .filter(|_| batch.reused.iter().any(|id| id == guard))
                .map(|mark| mark.family),
        };
        if family == Some(MarkFamily::Ocr) {
            return Err(blocked(
                "record_guard_family_invalid",
                format!(
                    "the click guard '{guard}' is an OCR mark, which guards only a click that \
                     follows it; name another guard with --click-guard"
                ),
            ));
        }
    }
    Ok(())
}

/// #463 S2-5 (`contracts/lab-recording.md`, "Drawing a box"): the press box `rect` on step
/// `index`, whose click `click` has an outcome on record. A tap or a long press recorded by the
/// command that made it (`source: "point"`, a box of one pixel at its point) takes the press box as
/// its rectangle when the box holds the point it pressed; its source, gesture, execution (the
/// point, the frames, the outcome) and every other member stay as recorded, and nothing is pressed
/// again. A box that does not hold the point is `record_click_outside_step_rect` with
/// `details{reason: "recorded_point_outside_box", step, point, box}`: the recorded press would not
/// have been inside the region. A click whose rectangle is a box already (a box given before) takes
/// another only with `--replace-click` (`record_click_exists`), as a declared click does. A
/// recorded swipe keeps its ends: `record_click_rect_conflict`, as a press box beside a drag. Any
/// other click with an outcome (a click `do --capture --record` executed on a declared rectangle,
/// a select's) is today's `record_click_executed`. Nothing is written on a refusal.
fn box_on_recorded_press(
    index: u32,
    click: &mut StepClick,
    (rect, point): (RecordRect, RecordPoint),
    replace: bool,
) -> LabResult<()> {
    if click.source == SWIPE_SOURCE {
        return Err(with_details(
            blocked(
                "record_click_rect_conflict",
                format!(
                    "step {index} recorded a swipe, whose ends stay as recorded; a press box \
                     declares a tap region and cannot be given to it; nothing was written"
                ),
            ),
            json!({"step": index, "gesture": click.gesture, "box": rect}),
        ));
    }
    if click.source != POINT_SOURCE {
        return Err(with_details(
            blocked(
                "record_click_executed",
                format!(
                    "step {index} already has a click with an outcome on record; run `record \
                     mark --reopen-step {index}` first"
                ),
            ),
            json!({"step": index}),
        ));
    }
    if !point_inside(point, rect) {
        return Err(with_details(
            blocked(
                "record_click_outside_step_rect",
                format!(
                    "the press box {},{},{},{} does not hold the point {},{} step {index} \
                     recorded, so the recorded press would not have been inside it; draw the box \
                     around the point; nothing was written",
                    rect.x, rect.y, rect.width, rect.height, point.x, point.y
                ),
            ),
            json!({
                "step": index,
                "reason": RECORDED_POINT_OUTSIDE_BOX,
                "point": point,
                "box": rect
            }),
        ));
    }
    let unboxed = click.rect
        == RecordRect {
            x: point.x,
            y: point.y,
            width: 1,
            height: 1,
        };
    if !unboxed && !replace {
        return Err(with_details(
            blocked(
                "record_click_exists",
                format!(
                    "step {index}'s recorded press already has the press box {},{},{},{}; add \
                     --replace-click (replace_click) to give it another",
                    click.rect.x, click.rect.y, click.rect.width, click.rect.height
                ),
            ),
            json!({"step": index, "rect": click.rect}),
        ));
    }
    click.rect = rect;
    Ok(())
}

/// #463 S2-5: the word of a press box that does not hold the point a recorded press pressed
/// (`record_click_outside_step_rect`, `details.reason`).
const RECORDED_POINT_OUTSIDE_BOX: &str = "recorded_point_outside_box";

/// Workflow #396 REC-4: the rejection of a step whose click follows a template that does not
/// hold on one of its live frames. #463 L2: for a click that follows a located-text mark, the
/// frames are checked with the Lab's OCR provider and give the click its box; without an OCR
/// model the click cannot be checked, and the command is refused with `lab_ocr_unavailable`.
fn follow_frames(
    session: &Session,
    batch: &MarkBatch,
    result: &BatchResult,
    target: u32,
    live: &[RecordedFrame],
    ocr: &LabOcr,
) -> LabResult<FollowFrames> {
    let step = effective_step(&session.recording, target)?;
    let Some(click) = step.click.as_ref() else {
        return Ok(FollowFrames::default());
    };
    let Some(follow) = click.follow.as_ref() else {
        return Ok(FollowFrames::default());
    };
    // `apply_click` checked that the followed template is a mark of the step; a reused source
    // that has gone is already the batch's `reuse_source_missing` rejection.
    let Some(mark) = result
        .own
        .iter()
        .chain(result.reused.iter())
        .find(|mark| mark.id == follow.mark_id)
    else {
        return Ok(FollowFrames::default());
    };
    let frames = live
        .iter()
        .map(|entry| {
            session
                .cache
                .get(&entry.sha256)
                .map(|frame| EvalFrame {
                    id: entry.frame_id.as_str(),
                    frame,
                })
                .ok_or_else(|| invalid("validation_failed", "frame was not loaded"))
        })
        .collect::<LabResult<Vec<_>>>()?;
    if mark.locates_text() {
        let Some(provider) = ocr.ocr_provider() else {
            return Err(ocr.unavailable_error(Some(mark.id.as_str())).unwrap_or_else(|| {
                blocked(
                    LabCode::LabOcrUnavailable.as_str(),
                    format!(
                        "the click follows the located-text mark '{}', which needs an OCR model",
                        mark.id
                    ),
                )
            }));
        };
        // #463 L2 fix 1: a command that changes only the names is the change of the wanted value,
        // not held to the recorded press; nor is one that leaves the mark as it is after such a
        // change. Any other declaration or change is.
        // Fix 2: a press check names the members, besides the names, the command changed.
        let change = followed_change(step, follow, mark)?;
        let unchanged: &[String] = &[];
        let press_check = match &change {
            FollowedChange::NamesOnly => None,
            FollowedChange::Unchanged => (!follow.names_changed_after_press).then_some(unchanged),
            FollowedChange::Other(members) => Some(members.as_slice()),
        };
        let mut checked =
            check_located_follow_frames(follow, click, mark, live, &frames, provider, press_check)?;
        if let Some(located) = checked.located.as_mut() {
            located.names_changed_after_press = match change {
                FollowedChange::NamesOnly => click.executed(),
                FollowedChange::Unchanged => follow.names_changed_after_press,
                FollowedChange::Other(_) => false,
            };
        }
        // A frame without the located text gives nothing here: the mark's own self-test refused
        // it, with its typed reason. A click without its box is never recorded.
        if checked.rejection.is_none()
            && checked.located.is_none()
            && !result
                .rejections
                .iter()
                .any(|rejection| rejection.id == mark.id)
        {
            return Err(blocked(
                LabCode::RecordClickFollowInvalid.as_str(),
                format!(
                    "the click follows the located-text mark '{}', whose names its self-test \
                     found but the click check did not; nothing was written",
                    mark.id
                ),
            ));
        }
        return Ok(checked);
    }
    let crop = match batch.new_crops.get(&mark.id) {
        Some(bytes) => bytes.clone(),
        None => stored_crop(mark)?.ok_or_else(|| {
            invalid(
                "validation_failed",
                format!("template '{}' has no crop", mark.id),
            )
        })?,
    };
    Ok(FollowFrames {
        rejection: check_follow_frames(
            follow,
            mark,
            &crop,
            live,
            &frames,
            &session.recording.defaults,
        )?,
        located: None,
    })
}

/// #463 L2 fix 1: what a `record mark` does to the located-text mark a step's click follows.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FollowedChange {
    /// The mark is as it was, or it is reused from another step, which alone changes it.
    Unchanged,
    /// Only its `expected` names differ: the request removed it and added it again with the same
    /// id, region, `locate`, `unique` and other members.
    NamesOnly,
    /// The mark is new to the step, or another member differs; fix 2: those members by name (an
    /// OCR member by its own name, `languages` for example), none for a mark new to the step.
    Other(Vec<String>),
}

/// #463 L2 fix 1: how `mark`, the followed mark after this command, differs from the step's own
/// mark of that id before it, its self-test, creation time and initiator aside. Every other member
/// of the mark is compared: `family`, `region`, `unique` and the rest of the recorded mark, and
/// every member of its OCR fields (`languages`, `timeout_ms`, `match_mode`, `case_sensitive`,
/// `minimum_confidence`, `model_ref`, `model_sha256`, `locate`) but `expected`.
fn followed_change(
    step: &RecordingStep,
    follow: &ClickFollow,
    mark: &RecordedMark,
) -> LabResult<FollowedChange> {
    let Some(old) = step.marks.iter().find(|old| old.id == follow.mark_id) else {
        return Ok(if step.reused.contains(&follow.mark_id) {
            FollowedChange::Unchanged
        } else {
            FollowedChange::Other(Vec::new())
        });
    };
    let mut new = mark.clone();
    new.self_test = old.self_test.clone();
    new.created_at_unix_ms = old.created_at_unix_ms;
    new.initiator = old.initiator.clone();
    if new == *old {
        return Ok(FollowedChange::Unchanged);
    }
    if let (Some(names), Some(before)) = (new.ocr.as_mut(), old.ocr.as_ref()) {
        names.expected = before.expected.clone();
    }
    if new == *old {
        Ok(FollowedChange::NamesOnly)
    } else {
        Ok(FollowedChange::Other(differing_members(old, &new)?))
    }
}

/// #463 L2 fix 2: the members in which `new` differs from `old`, by name, in the order of their
/// names; a member of the OCR fields by its own name.
fn differing_members(old: &RecordedMark, new: &RecordedMark) -> LabResult<Vec<String>> {
    let encode = |mark: &RecordedMark| {
        serde_json::to_value(mark).map_err(|error| {
            invalid(
                "validation_failed",
                format!("failed to encode mark '{}': {error}", mark.id),
            )
        })
    };
    let (recorded, requested) = (encode(old)?, encode(new)?);
    let mut members = Vec::new();
    for member in differing_keys(&recorded, &requested) {
        let (before, after) = (&recorded[member.as_str()], &requested[member.as_str()]);
        if member == "ocr" && before.is_object() && after.is_object() {
            members.extend(differing_keys(before, after));
        } else {
            members.push(member);
        }
    }
    members.sort();
    members.dedup();
    Ok(members)
}

/// #463 L2 fix 2: the keys whose values differ between two JSON objects (a key that only one of
/// them has among them); nothing when either is not an object.
fn differing_keys(old: &Value, new: &Value) -> Vec<String> {
    let (Some(before), Some(after)) = (old.as_object(), new.as_object()) else {
        return Vec::new();
    };
    before
        .iter()
        .filter(|(key, value)| after.get(key.as_str()) != Some(*value))
        .map(|(key, _)| key.clone())
        .chain(
            after
                .keys()
                .filter(|key| !before.contains_key(key.as_str()))
                .cloned(),
        )
        .collect()
}

/// `launch`, `restart` and `stop` as given; `force-stop` is recorded as `stop` (R24 section
/// 2.3.1).
pub(crate) fn application_action(verb: &str) -> LabResult<&'static str> {
    match verb {
        "launch" => Ok("launch"),
        "restart" => Ok("restart"),
        "stop" | "force-stop" => Ok("stop"),
        other => Err(with_details(
            invalid(
                "record_application_action_invalid",
                format!(
                    "the application operation must be launch, restart, stop or force-stop, got \
                     '{other}'"
                ),
            ),
            json!({"action": other}),
        )),
    }
}

pub(crate) fn application_with_click(step: Option<u32>) -> LabError {
    with_details(
        invalid(
            "record_application_with_click",
            "one step has one effect: --application cannot be combined with --click, \
             --click-from, --click-guard or --click-retry, and an application step takes no \
             click guard or retry",
        ),
        json!({"step": step}),
    )
}

/// The step's effect, `click` or `application`, and whether it has an outcome on record.
fn step_effect(step: &RecordingStep) -> Option<(&'static str, bool)> {
    match (&step.click, &step.application) {
        (Some(click), _) => Some(("click", click.execution.is_some())),
        (None, Some(application)) => Some(("application", application.executed.is_some())),
        (None, None) => None,
    }
}

fn effect_name(effect: &str) -> &'static str {
    if effect == "click" {
        "a click"
    } else {
        "an application operation"
    }
}

/// The step already has an effect (`click` or `application`); `hint` says what to do.
fn effect_exists(index: u32, effect: &str, hint: &str) -> LabError {
    with_details(
        blocked(
            "record_step_effect_exists",
            format!(
                "step {index} already has {}; one step has one effect; {hint}",
                effect_name(effect)
            ),
        ),
        json!({"step": index, "effect": effect}),
    )
}

/// An effect with an outcome on record is not replaced before `--reopen-step`.
fn effect_executed(index: u32, effect: &str) -> LabError {
    with_details(
        blocked(
            "record_click_executed",
            format!(
                "step {index} already has {} with an outcome on record; run \
                 `record mark --reopen-step {index}` first",
                effect_name(effect)
            ),
        ),
        json!({"step": index, "effect": effect}),
    )
}

fn application_entry_invalid() -> LabError {
    blocked(
        "record_application_entry_invalid",
        "an application entry step can only be step 1. After the effect of the previous step, \
         `session app <action> --record` captures the arrival screen itself before the \
         operation; to declare the operation, capture the arrival screen with capture --record \
         first, then declare it on that step (no mark is needed); offline, give the arrival \
         screen with --frame",
    )
}

/// `record mark --application` without `--step` and `--frame`: the open step, or a new
/// application entry step while the recording has no effective step.
fn application_mark_target(recording: &mut LabRecording) -> LabResult<(u32, bool)> {
    if let Some(index) = open_step(recording) {
        return Ok((index, false));
    }
    if !effective_indices(recording).is_empty() {
        return Err(application_entry_invalid());
    }
    let index = next_step_index(recording)?;
    recording.steps.push(new_step(index, Vec::new()));
    Ok((index, true))
}

/// `record mark --application <action>`: the declared application operation becomes the
/// step's effect. #463 S1-3: an open step needs no marks for it (their check is `record stop`'s).
fn apply_application(
    session: &mut Session,
    request: &super::model::MarkRequest,
    target: u32,
) -> LabResult<()> {
    let Some(spec) = &request.application else {
        return Ok(());
    };
    let action = application_action(&spec.action)?;
    let step = step_mut(&mut session.recording, target)?;
    match step_effect(step) {
        Some((effect, true)) => return Err(effect_executed(target, effect)),
        Some((effect, false)) if !request.replace_click => {
            return Err(effect_exists(
                target,
                effect,
                "add --replace-click to replace the declared one",
            ));
        }
        _ => {}
    }
    let attempts = step
        .application
        .as_ref()
        .map(|application| application.attempts.clone())
        .unwrap_or_default();
    step.click = None;
    step.click_guard = None;
    // #463 R1: an application operation in place of a select's click ends the select.
    step.select = None;
    step.application = Some(StepApplication {
        action: action.to_string(),
        cli_verb: spec.action.clone(),
        source: "declared".to_string(),
        executed: None,
        attempts,
        needs_review: false,
    });
    Ok(())
}

/// `record mark --optional [--settle-ms n]` / `--not-optional` (Workflow #339), after the
/// step's effect. Without a settle a step that becomes optional takes the default and an
/// optional step keeps its value. The first effective step and an application step cannot be
/// optional; the last step and the main interface after a restart are checked by
/// `record stop`.
pub(crate) fn apply_optional(
    recording: &mut LabRecording,
    request: &super::model::MarkRequest,
    target: u32,
) -> LabResult<()> {
    let first = effective_indices(recording).first() == Some(&target);
    let step = step_mut(recording, target)?;
    // #463 R4: a step a stopped recording inserted was never recorded on the path; it stays
    // optional, since a required step would make every run need its pop-up.
    if request.optional == Some(false)
        && let Some(after) = step.inserted_after
    {
        return Err(optional_after_invalid(
            OptionalAfterProblem::ClearsOptional,
            Some(target),
            format!(
                "step {target} was inserted after step {after} as an optional step and is never \
                 recorded on the path, so it stays optional; give it up with record mark \
                 --remove-inserted {target}; nothing was written"
            ),
            json!({"inserted_after": after}),
        ));
    }
    match request.optional {
        None => {}
        Some(false) => step.optional = None,
        Some(true) => {
            let current = step.optional.as_ref().map(|optional| optional.settle_ms);
            let settle_ms = request
                .optional_settle_ms
                .or(current)
                .unwrap_or(LAB_RECORDING_DEFAULT_SETTLE_MS);
            if current != Some(settle_ms) {
                step.optional = Some(StepOptional {
                    settle_ms,
                    marked_at_unix_ms: now_unix_ms(),
                });
            }
        }
    }
    if step.optional.is_none() {
        return Ok(());
    }
    if first {
        return Err(with_details(
            blocked(
                "record_optional_first_step",
                format!(
                    "step {target} is the first step of the recording and cannot be optional: the \
                     package starts from its page; start the recording one screen earlier"
                ),
            ),
            json!({"step": target}),
        ));
    }
    if let Some(application) = &step.application {
        return Err(optional_application(target, &application.action));
    }
    Ok(())
}

/// An optional step whose effect is (or would become) an application operation.
fn optional_application(index: u32, action: &str) -> LabError {
    with_details(
        blocked(
            "record_optional_application",
            format!(
                "step {index} is optional and its effect is the application operation {action}; \
                 an application step cannot be optional (there is no conditional restart): mark \
                 the screens after it optional, or clear the step with --not-optional"
            ),
        ),
        json!({"step": index, "application": action}),
    )
}

/// `record mark --step k --transition none|page|window`.
pub(crate) struct TransitionApplied {
    pub(crate) step: u32,
    pub(crate) marks: Vec<RecordedMark>,
    pub(crate) reused: Vec<RecordedMark>,
}

pub(crate) fn apply_transition(
    session: &mut Session,
    request: &super::model::MarkRequest,
    spec: &super::model::TransitionSpec,
    ocr: &LabOcr,
) -> LabResult<TransitionApplied> {
    use super::model::TransitionSpec;
    let index = request.step.ok_or_else(|| {
        invalid(
            "validation_failed",
            "a transition needs --step <k>: the step whose click it follows",
        )
    })?;
    let step = effective_step(&session.recording, index)?;
    if step.click.is_none() && step.application.is_none() {
        return Err(with_details(
            blocked(
                "record_transition_without_click",
                format!(
                    "step {index} has no effect; a transition follows the effect (a click or an \
                     application operation) of an effective step"
                ),
            ),
            json!({"step": index}),
        ));
    }
    let existing = step.transition.clone();
    let mut applied = TransitionApplied {
        step: index,
        marks: Vec::new(),
        reused: Vec::new(),
    };
    let (frame, frame_id, samples, sample_frame_ids, add, reuse, timeout_ms) = match spec {
        TransitionSpec::Clear => {
            step_mut(&mut session.recording, index)?.transition = None;
            return Ok(applied);
        }
        TransitionSpec::Window { min_ms, max_ms } => {
            validate_window(*min_ms, *max_ms)?;
            refuse_existing(index, existing.as_ref(), request.replace_transition)?;
            step_mut(&mut session.recording, index)?.transition = Some(StepTransition::Window {
                min_ms: *min_ms,
                max_ms: *max_ms,
            });
            return Ok(applied);
        }
        TransitionSpec::Page {
            frame,
            samples,
            add,
            reuse,
            timeout_ms,
            frame_id,
            sample_frame_ids,
        } => (
            frame,
            frame_id,
            samples,
            sample_frame_ids,
            add,
            reuse,
            *timeout_ms,
        ),
    };
    validate_page_transition_timeout(timeout_ms)?;
    refuse_existing(index, existing.as_ref(), request.replace_transition)?;
    // #463 S2-7: the frame is a file or a frame this recording stored, never both.
    let given = match (frame, frame_id) {
        (Some(path), None) => FrameGiven::File(path.as_str()),
        (None, Some(stored)) => FrameGiven::Stored(stored.as_str()),
        (Some(_), Some(_)) => {
            return Err(invalid(
                "validation_failed",
                "a page transition takes its frame as a PNG file (frame) or as a frame this \
                 recording stored (frame_id), not both",
            ));
        }
        (None, None) => {
            return Err(invalid(
                "validation_failed",
                "a page transition needs --frame <png> of the intermediate screen, or (request \
                 form) frame_id, a frame this recording stored",
            ));
        }
    };
    if add.is_empty() && reuse.is_empty() {
        return Err(invalid(
            "validation_failed",
            "a page transition needs at least one recognition mark",
        ));
    }
    let released = match &existing {
        Some(StepTransition::Page { marks, .. }) => {
            marks.iter().map(|mark| mark.id.clone()).collect()
        }
        _ => BTreeSet::new(),
    };
    let primary = transition_frame(session, given, "transition", index)?;
    let mut frames = vec![primary.clone()];
    let given_samples = samples
        .iter()
        .map(|path| FrameGiven::File(path.as_str()))
        .chain(
            sample_frame_ids
                .iter()
                .map(|stored| FrameGiven::Stored(stored.as_str())),
        );
    for sample in given_samples {
        frames.push(transition_frame(
            session,
            sample,
            "transition_sample",
            index,
        )?);
    }
    let mut batch = MarkBatch {
        own: Vec::new(),
        reused: Vec::new(),
        new_crops: BTreeMap::new(),
        added_ids: BTreeSet::new(),
    };
    let mut rejections = Vec::new();
    add_marks(
        session,
        &mut batch,
        add,
        &primary.sha256,
        index,
        Some(index),
        &released,
        &mut rejections,
    )?;
    add_reuse(session, &mut batch, reuse, None)?;
    let total = batch.own.len() + batch.reused.len();
    let result = evaluate_batch(session, &batch, &frames, ocr)?;
    rejections.extend(result.rejections);
    if !rejections.is_empty() {
        return Err(rejected(&rejections, total));
    }
    queue_crops(session, &batch);
    let step = step_mut(&mut session.recording, index)?;
    step.transition = Some(StepTransition::Page {
        frames,
        marks: result.own.clone(),
        reused: batch.reused.clone(),
        timeout_ms,
        source: "marked".to_string(),
        converted_step: None,
    });
    applied.marks = result.own;
    applied.reused = result.reused;
    Ok(applied)
}

fn refuse_existing(index: u32, existing: Option<&StepTransition>, replace: bool) -> LabResult<()> {
    match existing {
        Some(transition) if !replace => Err(with_details(
            blocked(
                "record_transition_exists",
                format!(
                    "step {index} already has a {} transition; add --replace-transition to \
                     replace it",
                    transition.kind()
                ),
            ),
            json!({"step": index, "kind": transition.kind()}),
        )),
        _ => Ok(()),
    }
}

/// #463 S2-7 (`contracts/lab-recording.md`, "Transitions"): where a frame of a page transition
/// comes from: a PNG file (`frame`, `samples`), or a frame this recording stored, named by its id
/// (`frame_id`, `sample_frame_ids`; the request form only).
#[derive(Clone, Copy)]
enum FrameGiven<'a> {
    File(&'a str),
    Stored(&'a str),
}

/// #463 S2-7: a frame of the page transition of step `step` in `role`, the one place a file and a
/// stored frame meet. A file is read and decoded; a stored frame is found by its id
/// ([`stored_frame`]; `record_frame_unknown` when the recording holds none of that id) and read back
/// as stored, its sha256 checked (`record_frame_hash_mismatch`). Both are admitted by
/// [`admit_frame`], with every check a file-given frame takes (the recording's base size, its
/// size). A stored frame's entry is its own content-addressed file, the same path and sha256 (no
/// copy is written, the file being there), with a frame id of its own and the stored frame's
/// provenance, `source: "recording_frame"`.
fn transition_frame(
    session: &mut Session,
    given: FrameGiven<'_>,
    role: &str,
    step: u32,
) -> LabResult<RecordedFrame> {
    match given {
        FrameGiven::File(path) => {
            let loaded = read_frame_file(path)?;
            admit_frame(session, loaded, role, &FrameMeta::local(), path)
        }
        FrameGiven::Stored(frame_id) => {
            let stored = stored_frame(&session.recording, frame_id)
                .cloned()
                .ok_or_else(|| frame_unknown(frame_id, step))?;
            let loaded = read_stored_frame(&stored)?;
            let RecordedFrame {
                runtime_artifact,
                capture_backend,
                freshness,
                scaled_from,
                ..
            } = stored;
            let meta = FrameMeta {
                source: RECORDING_FRAME_SOURCE.to_owned(),
                runtime_artifact,
                capture_backend,
                freshness,
            };
            let label = format!("frame {frame_id}");
            let mut entry = admit_frame(session, loaded, role, &meta, &label)?;
            entry.scaled_from = scaled_from;
            Ok(entry)
        }
    }
}

/// #463 S2-7: the frame `frame_id` this recording stored, wherever it is listed: a frame of a step
/// (of any step, a superseded frame too), of a step's page transition, or of an ending; `None` when
/// the recording holds no frame of that id.
fn stored_frame<'r>(recording: &'r LabRecording, frame_id: &str) -> Option<&'r RecordedFrame> {
    recording
        .steps
        .iter()
        .flat_map(|step| {
            let transition_frames = match &step.transition {
                Some(StepTransition::Page { frames, .. }) => frames.as_slice(),
                _ => &[][..],
            };
            step.frames.iter().chain(transition_frames)
        })
        .chain(
            recording
                .endings
                .iter()
                .flat_map(|ending| ending.frames.iter()),
        )
        .find(|frame| frame.frame_id == frame_id)
}

/// #463 S2-7: `record_frame_unknown`: the page transition of step `step` names by its id a frame
/// the recording does not hold.
fn frame_unknown(frame_id: &str, step: u32) -> LabError {
    with_details(
        blocked(
            LabCode::RecordFrameUnknown.as_str(),
            format!(
                "the page transition of step {step} names frame {frame_id}, which this recording \
                 does not hold; record status and record frames --step <k> list its frames; \
                 nothing was written"
            ),
        ),
        json!({"step": step, "frame_id": frame_id}),
    )
}

fn step_not_last(index: u32, last: Option<u32>) -> LabError {
    with_details(
        blocked(
            "record_step_not_last",
            format!(
                "step {index} is not the last effective step; remediation applies to the last \
                 effective step only"
            ),
        ),
        json!({"step": index, "last_effective_step": last}),
    )
}

/// The step a `--reopen-step` names: effective and the last effective one. (#463 S2-7: `--drop-step`
/// takes a step before the last too, `press_changed_nothing`.)
fn last_named(recording: &LabRecording, step: Option<u32>, flag: &str) -> LabResult<u32> {
    let index =
        step.ok_or_else(|| invalid("validation_failed", format!("{flag} needs the step number")))?;
    effective_step(recording, index)?;
    let last = last_effective(recording);
    if last != Some(index) {
        return Err(step_not_last(index, last));
    }
    Ok(index)
}

/// `--drop-step`, `--reopen-step`, `--close-step`, `--to-transition`; returns the status and
/// the step acted on. #463 S2-8': `fold_scan` and `unfold_scan` ([`fold_scan`], [`unfold_scan`]);
/// every other step action that names a step of a fold is refused first (`step_in_fold`), and
/// `first` is `fold_scan`'s alone (`validation_failed`).
pub(crate) fn apply_step_action(
    session: &mut Session,
    action: &super::model::StepAction,
) -> LabResult<(&'static str, u32)> {
    use super::model::StepActionKind;
    let recording = &mut session.recording;
    let folds = matches!(
        action.kind,
        StepActionKind::FoldScan | StepActionKind::UnfoldScan
    );
    if action.first.is_some() && action.kind != StepActionKind::FoldScan {
        return Err(invalid(
            "validation_failed",
            "first names the first swipe fold_scan folds; no other step operation takes it",
        ));
    }
    if let Some(index) = action.step.filter(|_| !folds)
        && let Some(press) = step_ref(recording, index).and_then(fold_of)
    {
        return Err(in_fold(index, press));
    }
    match action.kind {
        StepActionKind::DropStep => {
            let index = action
                .step
                .ok_or_else(|| invalid("validation_failed", "--drop-step needs the step number"))?;
            // #463 stage two, train, fix round (the coordinator's ruling): a step that holds a
            // choice is not dropped, the last one too, before its frames are compared.
            if holds_choice(effective_step(recording, index)?) {
                return Err(drop_of_choice(index));
            }
            // #463 S2-7: a step before the last effective one is dropped only when the frames stage
            // one stored do not contradict that its press changed nothing (fix round 1: by two
            // comparisons, `press_changed_nothing`); the last one as before.
            if last_effective(recording) != Some(index) {
                press_changed_nothing(recording, index)?;
            }
            let step = effective_step(recording, index)?;
            let ids = step
                .marks
                .iter()
                .map(|mark| mark.id.clone())
                .collect::<BTreeSet<_>>();
            let used_by_others = recording
                .steps
                .iter()
                .filter(|other| other.is_effective() && other.index != index)
                .flat_map(|other| {
                    let transition_reused = match &other.transition {
                        Some(StepTransition::Page { reused, .. }) => reused.as_slice(),
                        _ => &[][..],
                    };
                    // Workflow #390 LAB-2: a page that forbids a mark uses it too.
                    other
                        .reused
                        .iter()
                        .chain(transition_reused)
                        .chain(&other.forbidden)
                })
                .filter(|id| ids.contains(*id))
                .cloned()
                .collect::<BTreeSet<_>>();
            if !used_by_others.is_empty() {
                return Err(with_details(
                    blocked(
                        "record_mark_in_use",
                        format!("marks of step {index} are reused by other steps"),
                    ),
                    json!({"step": index, "marks": used_by_others}),
                ));
            }
            step_mut(recording, index)?.dropped = true;
            Ok(("step_dropped", index))
        }
        StepActionKind::ReopenStep => {
            let index = last_named(recording, action.step, "--reopen-step")?;
            let step = step_mut(recording, index)?;
            if let Some(click) = step.click.as_mut()
                && let Some(execution) = click.execution.take()
            {
                click.attempts.push(execution);
                click.needs_review = false;
                // #463 L2 fix 1: the press the names were changed after is an attempt now.
                if let Some(follow) = click.follow.as_mut() {
                    follow.names_changed_after_press = false;
                }
            }
            if let Some(application) = step.application.as_mut()
                && let Some(executed) = application.executed.take()
            {
                application.attempts.push(ApplicationAttempt {
                    receipt_state: executed.receipt_state,
                    runtime_code: None,
                    request_id: Some(executed.request_id),
                });
                application.source = "declared".to_string();
                application.needs_review = false;
            }
            step.closed = false;
            step.closed_by = None;
            Ok(("step_reopened", index))
        }
        StepActionKind::CloseStep => {
            let index = open_step(recording).ok_or_else(|| {
                blocked("record_step_not_found", "there is no open step to close")
            })?;
            if let Some(requested) = action.step
                && requested != index
            {
                return Err(step_not_last(requested, Some(index)));
            }
            let step = step_mut(recording, index)?;
            if step.click.is_none() && step.application.is_none() {
                return Err(click_missing(index));
            }
            step.closed = true;
            step.closed_by = Some("author".to_string());
            Ok(("step_closed", index))
        }
        StepActionKind::ToTransition => {
            let index = action.step.ok_or_else(|| {
                invalid("validation_failed", "--to-transition needs the step number")
            })?;
            to_transition(recording, index)?;
            Ok(("step_converted_to_transition", index))
        }
        StepActionKind::RestoreStep => {
            let index = action.step.ok_or_else(|| {
                invalid("validation_failed", "restore_step needs the step number")
            })?;
            restore_step(recording, index)?;
            Ok(("step_restored", index))
        }
        StepActionKind::FoldScan => {
            let (Some(index), Some(first)) = (action.step, action.first) else {
                return Err(invalid(
                    "validation_failed",
                    "fold_scan needs step, the press of the wanted row, and first, the first swipe \
                     before it",
                ));
            };
            fold_scan(recording, index, first)?;
            Ok(("scan_folded", index))
        }
        StepActionKind::UnfoldScan => {
            let index = action
                .step
                .ok_or_else(|| invalid("validation_failed", "unfold_scan needs the step number"))?;
            unfold_scan(recording, index)?;
            Ok(("scan_unfolded", index))
        }
    }
}

/// #463 S2-7 fix round 1 (the coordinator's ruling R1; `contracts/lab-recording.md`, "Steps,
/// frames and remediation"): how far the neighbourhood of a press reaches beyond the press's
/// rectangle on every side, in cells of the whole-frame measure's grid ([`FRAME_CHANGE_COLUMNS`] by
/// [`FRAME_CHANGE_ROWS`]) at the size of the recording's frames: one cell, 80 by 80 pixels at
/// 1280x720.
const PRESS_REGION_MARGIN_CELLS: u32 = 1;

/// #463 S2-7 (`contracts/lab-recording.md`, "Steps, frames and remediation"): whether the frames
/// stage one stored contradict that the press of step `index`, an effective step before the last,
/// changed nothing; the tool refuses only what they contradict, and whether the press mattered is
/// the agent's judgement. Its press is on record with the stored frame it was sent on
/// (`before_frame`) and the stored frame after it (`after_frame`): a click performed by the command
/// that made it (`tap`, `swipe`, `long-tap --record`) and sent on that stored frame. The frame
/// after it is the same screen as the frame before it, and so is the frame the next effective
/// step's press was sent on ([`screen_before`]). Fix round 1: two frames are the same screen when
/// both comparisons of [`changed_by`] hold, the whole frame and the neighbourhood of the press
/// ([`press_region`]), each by the one measure and bound stage one calls a screen still by
/// ([`frame_unchanged`]: `frame_change_milli` at most `FRAME_CHANGE_MAX_MILLI`); a change smaller
/// than that measure sees is not seen. Every frame is read back with its sha256 checked. Anything
/// else is `record_drop_unproven` with its `drop_problem` and, for a change the frames show, the
/// `drop_comparison` that showed it; nothing is proposed.
fn press_changed_nothing(recording: &LabRecording, index: u32) -> LabResult<()> {
    let step = effective_step(recording, index)?;
    let click = step.click.as_ref().filter(|declared| declared.executed());
    let press = click
        .and_then(|declared| declared.execution.as_ref())
        .filter(|execution| !execution.sent_on_unstored_capture);
    let before_id = press.and_then(|execution| execution.before_frame.as_deref());
    let after_id = press.and_then(|execution| execution.after_frame.as_deref());
    let next = effective_indices(recording)
        .into_iter()
        .skip_while(|other| *other != index)
        .nth(1);
    // #463 S2-8': a next step that holds a fold is met on the screen its scrolling search starts
    // on, its first folded swipe's.
    let next_before = next
        .and_then(|serial| step_ref(recording, serial))
        .map(|step| met_on(recording, step))
        .and_then(screen_before);
    let known = json!({
        "before_frame": before_id,
        "after_frame": after_id,
        "next_step": next,
        "next_before_frame": next_before
    });
    let (Some(click), Some(before), Some(after), Some(next_frame)) = (
        click,
        before_id.and_then(|frame_id| stored_frame(recording, frame_id)),
        after_id.and_then(|frame_id| stored_frame(recording, frame_id)),
        next_before.and_then(|frame_id| stored_frame(recording, frame_id)),
    ) else {
        return Err(drop_unproven(
            DropProblem::FramesNotRecorded,
            None,
            index,
            format!(
                "step {index} is not the last effective step, and the frames this recording stored \
                 do not hold what the comparison needs: the press is not on record with the stored \
                 frame it was sent on and the stored frame after it (as tap, swipe and long-tap \
                 --record record a press), or the next step has no frame; nothing was written"
            ),
            known,
        ));
    };
    let region = press_region(pressed_rect(click), before.width, before.height);
    let before_digests = stored_digests(before, region)?;
    if let Some((comparison, milli)) = changed_by(&before_digests, &stored_digests(after, region)?)?
    {
        return Err(drop_unproven(
            DropProblem::ScreenChanged,
            Some(comparison),
            index,
            format!(
                "the stored frames show a change at the press of step {index}: the frame after it \
                 ({}) differs from the frame before it ({}) by {milli} milli, more than the \
                 frame-change bound, over the whole frame or over the neighbourhood of the press \
                 (details drop_comparison); a step before the last is dropped only when its stored \
                 frames show no change; nothing was written",
                after.frame_id, before.frame_id
            ),
            with_change(known, milli),
        ));
    }
    if let Some((comparison, milli)) =
        changed_by(&before_digests, &stored_digests(next_frame, region)?)?
    {
        return Err(drop_unproven(
            DropProblem::NextScreenDiffers,
            Some(comparison),
            index,
            format!(
                "the stored frames show a change after the press of step {index}: the frame the \
                 next step's press was sent on ({}) differs from the frame before this press ({}) \
                 by {milli} milli, more than the frame-change bound, over the whole frame or over \
                 the neighbourhood of this press (details drop_comparison); nothing was written",
                next_frame.frame_id, before.frame_id
            ),
            with_change(known, milli),
        ));
    }
    Ok(())
}

/// #463 S2-7 fix round 1 (the coordinator's ruling R1): the press's rectangle as the step table
/// holds it: the click's rectangle (a press by its point holds a rectangle of one pixel there), and
/// for a drag (a recorded swipe) the rectangle spanned by its two ends, the click's rectangle and
/// its `to`.
fn pressed_rect(click: &StepClick) -> RecordRect {
    let Some(ClickGesture::Drag { to, .. }) = click.gesture else {
        return click.rect;
    };
    let from = click.rect;
    let left = from.x.min(to.x);
    let top = from.y.min(to.y);
    let right =
        (i64::from(from.x) + i64::from(from.width)).max(i64::from(to.x) + i64::from(to.width));
    let bottom =
        (i64::from(from.y) + i64::from(from.height)).max(i64::from(to.y) + i64::from(to.height));
    let side = |end: i64, start: i32| i32::try_from(end - i64::from(start)).unwrap_or(i32::MAX);
    RecordRect {
        x: left,
        y: top,
        width: side(right, left),
        height: side(bottom, top),
    }
}

/// #463 S2-7 fix round 1 (the coordinator's ruling R1): the neighbourhood of the press `rect` on
/// frames of `width` by `height`: the rectangle grown on every side by
/// [`PRESS_REGION_MARGIN_CELLS`] cells of the whole-frame measure's grid at that size (a cell is
/// `width / FRAME_CHANGE_COLUMNS` by `height / FRAME_CHANGE_ROWS` pixels, rounded up), clamped to
/// the frame.
fn press_region(rect: RecordRect, width: u32, height: u32) -> Rect {
    let margin = |side: u32, cells: u32| {
        i64::from(
            side.saturating_mul(PRESS_REGION_MARGIN_CELLS)
                .div_ceil(cells),
        )
    };
    let (margin_x, margin_y) = (
        margin(width, FRAME_CHANGE_COLUMNS),
        margin(height, FRAME_CHANGE_ROWS),
    );
    let left = (i64::from(rect.x) - margin_x).max(0);
    let top = (i64::from(rect.y) - margin_y).max(0);
    let right = (i64::from(rect.x) + i64::from(rect.width) + margin_x).min(i64::from(width));
    let bottom = (i64::from(rect.y) + i64::from(rect.height) + margin_y).min(i64::from(height));
    let field = |value: i64| i32::try_from(value).unwrap_or(i32::MAX);
    Rect {
        x: field(left),
        y: field(top),
        width: field(right - left),
        height: field(bottom - top),
    }
}

/// #463 S2-7 fix round 1: the two frame-change digests of one stored frame, of the whole frame and
/// of the neighbourhood of the press.
struct ScreenDigests {
    whole: ColorDigest,
    press: ColorDigest,
}

/// #463 S2-7 fix round 1 (the coordinator's ruling R1): the first of the two comparisons by which
/// the frames of `older` and `newer` are not the same screen, with the measure's value: the whole
/// frame (`frame`), then the neighbourhood of the press (`press_region`), each by
/// [`frame_unchanged`], the one measure and bound stage one calls a screen still by; `None` when
/// both hold.
fn changed_by(
    older: &ScreenDigests,
    newer: &ScreenDigests,
) -> LabResult<Option<(DropComparison, u32)>> {
    for (comparison, old, new) in [
        (DropComparison::Frame, &older.whole, &newer.whole),
        (DropComparison::PressRegion, &older.press, &newer.press),
    ] {
        let (unchanged, milli) = frame_unchanged(old, new)?;
        if !unchanged {
            return Ok(Some((comparison, milli)));
        }
    }
    Ok(None)
}

/// #463 S2-7: the frame a step was pressed on, as the step table records it (its click's or its
/// application operation's `before_frame`), else its live primary frame: the screen a run meets the
/// step on.
fn screen_before(step: &RecordingStep) -> Option<&str> {
    step.click
        .as_ref()
        .and_then(|click| click.execution.as_ref())
        .and_then(|execution| execution.before_frame.as_deref())
        .or_else(|| {
            step.application
                .as_ref()
                .and_then(|application| application.executed.as_ref())
                .and_then(|executed| executed.before_frame.as_deref())
        })
        .or_else(|| live_primary(step).map(|frame| frame.frame_id.as_str()))
}

/// #463 S2-7: the stored frame `frame` read back once with its sha256 checked
/// (`record_frame_hash_mismatch`), and (fix round 1) its two frame-change digests: of the whole
/// frame ([`change_digest`], stage one's) and of the neighbourhood of the press `region`, by the same
/// measure over that region (`frame_change_digest`); a digest that cannot be made is
/// `record_frame_unreadable`, as there.
fn stored_digests(frame: &RecordedFrame, region: Rect) -> LabResult<ScreenDigests> {
    let loaded = read_stored_frame(frame)?;
    let label = format!("frame {}", frame.frame_id);
    let whole = change_digest(&loaded, &label)?;
    let press = frame_change_digest(&loaded.scene, Some(region)).map_err(|error| {
        invalid(
            "record_frame_unreadable",
            format!(
                "failed to digest the neighbourhood of the press on {label} for the frame-change \
                 measure: {error}"
            ),
        )
    })?;
    Ok(ScreenDigests { whole, press })
}

/// #463 S2-7: `details` with the frame-change measure's value of the two frames compared.
fn with_change(mut details: Value, milli: u32) -> Value {
    if let Some(object) = details.as_object_mut() {
        object.insert("frame_change_milli".to_owned(), json!(milli));
    }
    details
}

/// #463 S2-7: `record_drop_unproven` with why, for step `step`, its details carrying the outcome
/// with `drop_problem` and the members of `extra`. Fix round 1: with the `drop_comparison` that
/// showed a change (`comparison`), in the details and in the outcome.
fn drop_unproven(
    problem: DropProblem,
    comparison: Option<DropComparison>,
    step: u32,
    message: String,
    extra: Value,
) -> LabError {
    let mut outcome = Outcome::new(LabCode::RecordDropUnproven).drop_problem(problem);
    if let Some(comparison) = comparison {
        outcome = outcome.drop_comparison(comparison);
    }
    let mut details = json!({
        "step": step,
        "drop_problem": problem.as_str(),
        "outcome": outcome.envelope(Detail::Full)
    });
    if let Some(object) = details.as_object_mut() {
        if let Some(comparison) = comparison {
            object.insert("drop_comparison".to_owned(), json!(comparison.as_str()));
        }
        if let Some(extra) = extra.as_object() {
            for (name, value) in extra {
                object.insert(name.clone(), value.clone());
            }
        }
    }
    with_details(
        blocked(LabCode::RecordDropUnproven.as_str(), message),
        details,
    )
}

/// #463 S2-7 (`contracts/lab-recording.md`, "Steps, frames and remediation"): `restore_step` undoes
/// `drop_step`: step `index`, a dropped step, is effective again and the step table is what it was
/// before the drop, when nothing else changed in between. Its marks take their ids and template
/// asset names back, so an id or an asset name another mark took since is refused as a new mark's
/// is, by the one check `add_marks` makes ([`claim_mark_ids`]: `record_mark_id_conflict`;
/// [`claim_asset_name`]: `record_asset_name_conflict`); fix round 1 (the coordinator's ruling R2,
/// review (k)): a mark it reuses (on its page or its page transition) or forbids that is no longer a
/// step mark of an effective step is refused as the mark path refuses a missing source,
/// `record_mark_rejected` with `reuse_source_missing` or `forbidden_source_missing` for each, so
/// that the fault is told here and not by the next command; a step that is no dropped step is
/// `record_step_not_found`. Nothing is changed on a refusal.
fn restore_step(recording: &mut LabRecording, index: u32) -> LabResult<()> {
    let step = step_ref(recording, index)
        .filter(|step| step.dropped)
        .ok_or_else(|| {
            with_details(
                blocked(
                    "record_step_not_found",
                    format!(
                        "step {index} is no dropped step of the recording; restore_step gives back \
                         a step drop_step dropped"
                    ),
                ),
                json!({"step": index}),
            )
        })?;
    let (transition_marks, transition_reused) = match &step.transition {
        Some(StepTransition::Page { marks, reused, .. }) => (marks.as_slice(), reused.as_slice()),
        _ => (&[][..], &[][..]),
    };
    let mut in_use = ids_in_use(recording, &BTreeSet::new());
    let mut assets = template_assets(recording);
    for mark in step.marks.iter().chain(transition_marks) {
        claim_mark_ids(&mut in_use, &mark.target_ids())?;
        if let Some(crop) = &mark.crop {
            claim_asset_name(&mut assets, &mark.id, &crop.asset)?;
        }
    }
    let sources = step
        .reused
        .iter()
        .chain(transition_reused)
        .map(|id| (id, "reuse_source_missing"))
        .chain(
            step.forbidden
                .iter()
                .map(|id| (id, "forbidden_source_missing")),
        )
        .collect::<Vec<_>>();
    let missing = sources
        .iter()
        .filter(|(id, _)| find_step_mark(recording, id).is_none())
        .map(|(id, word)| MarkRejection::new(id, word, json!({})))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(rejected(&missing, sources.len()));
    }
    step_mut(recording, index)?.dropped = false;
    Ok(())
}

/// #463 stage two, train, fix round (`contracts/lab-recording.md`, "Steps, frames and
/// remediation"): whether `step` holds a choice: marked as holding one (#463 S2-6) or refined by a
/// select (#463 R1).
fn holds_choice(step: &RecordingStep) -> bool {
    step.choice.is_some() || step.select.is_some()
}

/// #463 stage two, train, fix round: `drop_step` names step `index`, which holds a choice (the
/// coordinator's ruling: a press that chose the item already selected changes nothing on the
/// screen and still matters to a run handed another value); nothing is written.
fn drop_of_choice(index: u32) -> LabError {
    select_invalid(
        Some(index),
        SelectProblem::DropOfChoice,
        format!(
            "step {index} holds a choice (it is marked as holding one, or a select refines it), and \
             a step that holds a choice is not dropped: its press may have chosen the item already \
             selected, which changes nothing on the screen and still matters to a run handed \
             another value; to drop it, withdraw the choice first (record mark --step {index} with \
             choice false where the step carries the mark, and clear_select where a select refines \
             it), then drop the step; nothing was written"
        ),
    )
}

/// #463 stage two, train, fix round: `to_transition` names step `index`, which holds a choice; a
/// page transition holds none, so the conversion would lose it; nothing is written.
fn transition_of_choice(index: u32) -> LabError {
    select_invalid(
        Some(index),
        SelectProblem::TransitionOfChoice,
        format!(
            "step {index} holds a choice (it is marked as holding one, or a select refines it), and \
             a page transition holds no choice, so converting the step would lose it; withdraw the \
             choice first (record mark --step {index} with choice false where the step carries the \
             mark, and clear_select where a select refines it), then convert the step; nothing was \
             written"
        ),
    )
}

fn to_transition_invalid(index: u32, reason: &str) -> LabError {
    with_details(
        blocked(
            "record_to_transition_invalid",
            format!("step {index} cannot become a transition: {reason}"),
        ),
        json!({"step": index, "reason": reason}),
    )
}

fn to_transition(recording: &mut LabRecording, index: u32) -> LabResult<()> {
    let effective = effective_indices(recording);
    if effective.last() != Some(&index) {
        return Err(to_transition_invalid(index, "not_last_effective_step"));
    }
    let Some(previous) = effective
        .iter()
        .copied()
        .filter(|other| *other < index)
        .max()
    else {
        return Err(to_transition_invalid(index, "no_previous_effective_step"));
    };
    let step = effective_step(recording, index)?;
    if step.click.is_some() || step.application.is_some() {
        return Err(to_transition_invalid(index, "step_has_effect"));
    }
    // A transition must be seen; an optional step may not appear (Workflow #339).
    if step.optional.is_some() {
        return Err(to_transition_invalid(index, "optional"));
    }
    // Workflow #390 LAB-2: a page transition only requires its marks; it forbids nothing and has
    // no anchor.
    if !step.forbidden.is_empty() || step.anchor.is_some() {
        return Err(to_transition_invalid(index, "page_rule"));
    }
    if !has_marks(step) {
        return Err(to_transition_invalid(index, "step_has_no_marks"));
    }
    let ids = step
        .marks
        .iter()
        .map(|mark| mark.id.clone())
        .collect::<BTreeSet<_>>();
    if !reused_elsewhere(recording, index, &ids).is_empty() {
        return Err(to_transition_invalid(index, "marks_reused_by_other_steps"));
    }
    let target = effective_step(recording, previous)?;
    if target.click.is_none() && target.application.is_none() {
        return Err(to_transition_invalid(index, "previous_step_has_no_effect"));
    }
    if target.transition.is_some() {
        return Err(to_transition_invalid(index, "previous_step_has_transition"));
    }
    // #463 stage two, train, fix round: a step that holds a choice is not converted, after every
    // other refusal of the conversion (a select step has its click, `step_has_effect` above).
    if holds_choice(step) {
        return Err(transition_of_choice(index));
    }
    let step = step_mut(recording, index)?;
    let mut frames = std::mem::take(&mut step.frames);
    let mut marks = std::mem::take(&mut step.marks);
    let reused = std::mem::take(&mut step.reused);
    step.converted_to_transition = true;
    step.closed = true;
    step.closed_by = Some("converted_to_transition".to_string());
    for frame in &mut frames {
        frame.role = if frame.role == "primary" {
            "transition".to_string()
        } else {
            "transition_sample".to_string()
        };
    }
    for mark in &mut marks {
        mark.step = previous;
        mark.transition_of = Some(previous);
    }
    step_mut(recording, previous)?.transition = Some(StepTransition::Page {
        frames,
        marks,
        reused,
        timeout_ms: None,
        source: "converted_from_step".to_string(),
        converted_step: Some(index),
    });
    Ok(())
}

fn click_missing_for_do(index: u32) -> LabError {
    with_details(
        blocked(
            "record_step_click_missing",
            format!(
                "step {index} declares no click; give --tap-rect x,y,w,h or declare it with \
                 `record mark --click` first"
            ),
        ),
        json!({"step": index}),
    )
}

/// `do --capture --record`: the rectangle and point of the open step's click.
pub(crate) fn plan_click(
    recording: &LabRecording,
    request: &super::model::PlanClickRequest,
) -> LabResult<super::model::ClickPlan> {
    let index = open_step(recording).ok_or_else(|| {
        blocked(
            "record_step_frame_missing",
            "there is no open step; record its frame with capture --record first",
        )
    })?;
    let step = effective_step(recording, index)?;
    if step.application.is_some() {
        return Err(effect_exists(
            index,
            "application",
            "a click needs a step of its own: execute the operation with `session app <action> \
             --record` or close the step with `record mark --close-step`, then capture the next \
             screen",
        ));
    }
    if let Some(click) = &step.click
        && click.execution.is_some()
    {
        return Err(with_details(
            blocked(
                "record_click_executed",
                format!(
                    "the click of step {index} already has an outcome on record; run \
                     `record mark --reopen-step {index}` to execute it again or \
                     `record mark --close-step` to accept it"
                ),
            ),
            json!({"step": index}),
        ));
    }
    // Workflow #390 LAB-2: a step that declares a drag is executed as one swipe, and (#411 Lab L3)
    // one that declares a long press as one long tap.
    if let Some((click, gesture)) = step
        .click
        .as_ref()
        .and_then(|click| click.gesture.map(|gesture| (click, gesture)))
    {
        return plan_gesture(recording, index, click, gesture, request);
    }
    if let Some(swipe) = request.swipe {
        return match &step.click {
            // Workflow #449 REC-ARCH: a swipe on a step without a declared effect is the step's
            // effect, as `--tap-rect` declares a tap: a drag between the two points it presses.
            None => plan_swipe(recording, index, swipe, request),
            Some(click) => Err(with_details(
                blocked(
                    "record_click_rect_conflict",
                    format!("step {index} declares a tap; --swipe executes a declared drag"),
                ),
                json!({"step": index, "declared": click.rect}),
            )),
        };
    }
    let (rect, source, declares_click) = match (request.tap_rect, &step.click) {
        (Some(rect), None) => {
            if !recording
                .coordinate_space
                .is_some_and(|size| rect_inside(rect, size))
            {
                return Err(invalid(
                    "validation_failed",
                    "--tap-rect must lie inside the frame with width and height >= 1",
                ));
            }
            (rect, "tap_rect".to_string(), true)
        }
        (Some(rect), Some(click)) if click.rect == rect => (rect, click.source.clone(), false),
        (Some(rect), Some(click)) => {
            return Err(with_details(
                blocked(
                    "record_click_rect_conflict",
                    format!("--tap-rect differs from the click step {index} declares"),
                ),
                json!({"step": index, "declared": click.rect, "tap_rect": rect}),
            ));
        }
        (None, Some(click)) => (click.rect, click.source.clone(), false),
        (None, None) => return Err(click_missing_for_do(index)),
    };
    let (point, point_rule, sampling) = match request.tap {
        Some(point) => {
            let inside = i64::from(point.x) >= i64::from(rect.x)
                && i64::from(point.y) >= i64::from(rect.y)
                && i64::from(point.x) < i64::from(rect.x) + i64::from(rect.width)
                && i64::from(point.y) < i64::from(rect.y) + i64::from(rect.height);
            if !inside {
                return Err(with_details(
                    blocked(
                        "record_click_outside_step_rect",
                        format!(
                            "--tap {},{} is outside the click of step {index}",
                            point.x, point.y
                        ),
                    ),
                    json!({"step": index, "rect": rect, "tap": point}),
                ));
            }
            (point, "explicit".to_string(), None)
        }
        // Workflow #406 T4 (plan Q20): with a seed, the point is drawn in the rectangle and the
        // rule names the draw; without one, the rectangle's centre.
        None => match &request.draw {
            Some(draw) => {
                let (point, sampling) = drawn_point(draw, rect)?;
                (point, point_rule_of(&sampling), Some(sampling))
            }
            None => (rect_center(rect), "rect_center".to_string(), None),
        },
    };
    Ok(super::model::ClickPlan {
        record_id: recording.record_id.clone(),
        step: index,
        rect,
        point,
        point_rule,
        source,
        declares_click,
        gesture: None,
        to_point: None,
        sampling,
    })
}

/// Workflow #449 REC-ARCH: `do --capture --record --swipe x1,y1,x2,y2,ms` on the open step that
/// declares no effect. The swipe becomes the step's effect, a drag from a box of one pixel at its
/// first point to a box of one pixel at its second over its duration, `source: "swipe"`, as
/// `--tap-rect` declares a tap; `record stop` generates it as the `drag` click of the standard
/// package's task schema. Both points lie inside the frame and the duration is 1..=60000 ms, as a
/// declared drag's (`validation_failed` otherwise, nothing pressed); `--tap`, `--tap-rect` and a
/// seed press no swipe and are refused with it.
fn plan_swipe(
    recording: &LabRecording,
    index: u32,
    swipe: super::model::PlannedSwipe,
    request: &super::model::PlanClickRequest,
) -> LabResult<super::model::ClickPlan> {
    if request.tap.is_some() || request.tap_rect.is_some() || request.draw.is_some() {
        return Err(invalid(
            "validation_failed",
            "--swipe declares the step's swipe by its two points; it takes no --tap, --tap-rect or \
             --seed",
        ));
    }
    let point_box = |point: RecordPoint| RecordRect {
        x: point.x,
        y: point.y,
        width: 1,
        height: 1,
    };
    let (from, to) = (point_box(swipe.from), point_box(swipe.to));
    let inside = recording
        .coordinate_space
        .is_some_and(|size| rect_inside(from, size) && rect_inside(to, size));
    // #463 S1-1 fix round 2: the refusals name their argument and reason, as a tap's do
    // (`plan_point`).
    if !inside {
        return Err(with_details(
            invalid(
                "validation_failed",
                "--swipe must start and end inside the frame",
            ),
            json!({
                "step": index,
                "argument": "point",
                "reason": "outside_frame",
                "swipe_from": swipe.from,
                "swipe_to": swipe.to
            }),
        ));
    }
    if !(1..=LAB_RECORDING_MAX_GESTURE_MS).contains(&swipe.duration_ms) {
        return Err(with_details(
            invalid(
                "validation_failed",
                format!("--swipe must last 1..={LAB_RECORDING_MAX_GESTURE_MS} ms"),
            ),
            json!({
                "step": index,
                "argument": "duration_ms",
                "reason": "duration_range",
                "duration_ms": swipe.duration_ms
            }),
        ));
    }
    Ok(super::model::ClickPlan {
        record_id: recording.record_id.clone(),
        step: index,
        rect: from,
        point: swipe.from,
        point_rule: "explicit".to_string(),
        source: SWIPE_SOURCE.to_string(),
        declares_click: true,
        gesture: Some(ClickGesture::Drag {
            to,
            duration_ms: swipe.duration_ms,
        }),
        to_point: Some(swipe.to),
        sampling: None,
    })
}

/// Workflow #449 REC-ARCH: the `source` of a click a recorded swipe declared. #463 S2-1: `record
/// ready` reads it as a press stage one recorded.
pub(crate) const SWIPE_SOURCE: &str = "swipe";

/// #463 S1-1: the `source` of a click a tap or a long press declared by its point. #463 S2-1:
/// `record ready` reads it as a press stage one recorded.
pub(crate) const POINT_SOURCE: &str = "point";

/// #463 S1-1 (`contracts/lab-recording.md`, "Recording while playing"): the press of `tap`,
/// `swipe` or `long-tap --record` on the open step, which must declare no effect. A tap or a long
/// press declares a box of one pixel at its point ([`plan_point`]); a swipe is today's recorded
/// swipe, planned by [`plan_click`] (a drag between boxes of one pixel at its two points,
/// `source: "swipe"`). A step that declares an application operation is
/// `record_step_effect_exists`, one whose click has an outcome on record `record_click_executed`,
/// and one that declares a click without one `record_click_exists`: such a step is pressed with
/// `do --capture --record`. Every refusal comes before anything is pressed.
pub(crate) fn plan_press(
    recording: &LabRecording,
    click: &super::model::PlanClickRequest,
    point_press: Option<super::model::PointPress>,
) -> LabResult<super::model::ClickPlan> {
    let index = open_step(recording).ok_or_else(|| {
        blocked(
            "record_step_frame_missing",
            "there is no open step; record its frame with capture --record first",
        )
    })?;
    let step = effective_step(recording, index)?;
    if step.application.is_some() {
        return Err(effect_exists(
            index,
            "application",
            "a press needs a step of its own: execute the operation with `session app <action> \
             --record` or close the step with `record mark --close-step`, then press",
        ));
    }
    if let Some(declared) = &step.click {
        if declared.execution.is_some() {
            return Err(with_details(
                blocked(
                    "record_click_executed",
                    format!(
                        "the click of step {index} already has an outcome on record; run \
                         `record mark --reopen-step {index}` to execute it again or \
                         `record mark --close-step` to accept it"
                    ),
                ),
                json!({"step": index}),
            ));
        }
        return Err(with_details(
            blocked(
                "record_click_exists",
                format!(
                    "step {index} already declares a click; press it with `do --capture \
                     --record`, or replace it first with `record mark --replace-click`; nothing \
                     was pressed"
                ),
            ),
            json!({"step": index}),
        ));
    }
    match point_press {
        Some(press) => plan_point(recording, index, press),
        None => plan_click(recording, click),
    }
}

/// #463 S1-1: a tap or a long press by its point on the open step `index`, which declares no
/// effect: the box of one pixel at the point, `rect {x, y, 1, 1}`, `source: "point"`, rule
/// `explicit`; a long press with `gesture{kind: "long_press", duration_ms}`, ending where it
/// began. The point lies inside the frame and a long press lasts 1..=60000 ms, else
/// `validation_failed` with `details{argument, reason}` (`point`, `outside_frame`;
/// `duration_ms`, `duration_range`).
fn plan_point(
    recording: &LabRecording,
    index: u32,
    press: super::model::PointPress,
) -> LabResult<super::model::ClickPlan> {
    use super::model::PointPress;
    let (point, gesture) = match press {
        PointPress::Tap { point } => (point, None),
        PointPress::LongPress { point, duration_ms } => {
            (point, Some(ClickGesture::LongPress { duration_ms }))
        }
    };
    let rect = RecordRect {
        x: point.x,
        y: point.y,
        width: 1,
        height: 1,
    };
    if !recording
        .coordinate_space
        .is_some_and(|size| rect_inside(rect, size))
    {
        return Err(with_details(
            invalid(
                "validation_failed",
                format!(
                    "the point {},{} must lie inside the frame of the recording",
                    point.x, point.y
                ),
            ),
            json!({
                "step": index,
                "argument": "point",
                "reason": "outside_frame",
                "point": point,
                "coordinate_space": recording.coordinate_space
            }),
        ));
    }
    if let Some(gesture) = gesture
        && !(1..=LAB_RECORDING_MAX_GESTURE_MS).contains(&gesture.duration_ms())
    {
        return Err(with_details(
            invalid(
                "validation_failed",
                format!("a long press must last 1..={LAB_RECORDING_MAX_GESTURE_MS} ms"),
            ),
            json!({
                "step": index,
                "argument": "duration_ms",
                "reason": "duration_range",
                "duration_ms": gesture.duration_ms()
            }),
        ));
    }
    Ok(super::model::ClickPlan {
        record_id: recording.record_id.clone(),
        step: index,
        rect,
        point,
        point_rule: "explicit".to_string(),
        source: POINT_SOURCE.to_string(),
        declares_click: true,
        gesture,
        to_point: gesture.map(|_| point),
        sampling: None,
    })
}

/// Workflow #406 T4: a recorded click's rectangle as a drawn box.
fn pack_rect(rect: RecordRect) -> PackRect {
    PackRect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}

/// Workflow #406 T4 (plan Q20; `contracts/lab-recording.md`, the point rule): the point the Lab
/// presses in `rect` drawn by `draw`, a recorded click being its command's first press with no
/// element label, and how it was drawn.
fn drawn_point(
    draw: &DriveDraw,
    rect: RecordRect,
) -> LabResult<(RecordPoint, InputSamplingEvidence)> {
    let (point, sampling) = draw
        .point(pack_rect(rect), 0, "")
        .map_err(|error| LabError::usage(error.message()))?;
    Ok((
        RecordPoint {
            x: point.x,
            y: point.y,
        },
        sampling,
    ))
}

/// Workflow #406 T4: the point rule of a drawn press, the name of its draw's algorithm
/// (`center_hotspot_normal_v1` with touch keys, `xorshift64_uniform_rect_v1` without).
fn point_rule_of(sampling: &InputSamplingEvidence) -> String {
    sampling_algorithm_name(sampling.algorithm()).to_string()
}

fn point_inside(point: RecordPoint, rect: RecordRect) -> bool {
    i64::from(point.x) >= i64::from(rect.x)
        && i64::from(point.y) >= i64::from(rect.y)
        && i64::from(point.x) < i64::from(rect.x) + i64::from(rect.width)
        && i64::from(point.y) < i64::from(rect.y) + i64::from(rect.height)
}

/// Workflow #390 LAB-2: `do --capture --record` on a step that declares a drag or a long press.
/// A drag goes from a point of its rectangle to a point of `to` (their centers, or `--swipe`'s
/// points, which must lie inside them, with the declared duration); a long press holds the
/// center of its click box (Workflow #390 lpbox), the box being recorded around that point.
/// `--tap` and `--tap-rect` press a tap and are refused here. #411 Lab BD: a brake drag is not
/// pressed by a Lab operation (`record_gesture_execution_unsupported`), before anything else is
/// checked; its step is closed offline or with `--close-step`.
fn plan_gesture(
    recording: &LabRecording,
    index: u32,
    click: &StepClick,
    gesture: ClickGesture,
    request: &super::model::PlanClickRequest,
) -> LabResult<super::model::ClickPlan> {
    // Workflow #406 T4: nor a path drag (the online Lab performs neither until B-3).
    if let ClickGesture::SingleTouchDragWithVerticalBrakeV1 { .. }
    | ClickGesture::SingleTouchPathDragV1 { .. } = gesture
    {
        return Err(gesture_execution_unsupported(index, gesture));
    }
    let conflict = |message: String, details: Value| {
        with_details(blocked("record_click_rect_conflict", message), details)
    };
    if request.tap.is_some() || request.tap_rect.is_some() {
        return Err(conflict(
            format!(
                "step {index} declares a {}; --tap and --tap-rect press a tap",
                gesture.kind()
            ),
            json!({"step": index, "gesture": gesture}),
        ));
    }
    let rect = click.rect;
    let (point, to_point, rule, sampling) = match (gesture, request.swipe) {
        (ClickGesture::Drag { to, duration_ms }, Some(swipe)) => {
            if !point_inside(swipe.from, rect) || !point_inside(swipe.to, to) {
                return Err(with_details(
                    blocked(
                        "record_click_outside_step_rect",
                        format!(
                            "--swipe {},{},{},{} does not start in the drag's rectangle and end in \
                             its destination of step {index}",
                            swipe.from.x, swipe.from.y, swipe.to.x, swipe.to.y
                        ),
                    ),
                    json!({"step": index, "rect": rect, "to": to, "swipe_from": swipe.from, "swipe_to": swipe.to}),
                ));
            }
            if swipe.duration_ms != duration_ms {
                return Err(conflict(
                    format!(
                        "--swipe lasts {} ms; the drag of step {index} declares {duration_ms} ms",
                        swipe.duration_ms
                    ),
                    json!({
                        "step": index,
                        "declared_duration_ms": duration_ms,
                        "swipe_duration_ms": swipe.duration_ms
                    }),
                ));
            }
            (swipe.from, swipe.to, "explicit".to_string(), None)
        }
        // Workflow #406 T4: with a seed, both ends are drawn in their boxes, as a run draws a
        // drag's; without one, their centres.
        (ClickGesture::Drag { to, .. }, None) => match &request.draw {
            Some(draw) => {
                let (from, end, sampling) = draw
                    .drag(pack_rect(rect), pack_rect(to), 0, "")
                    .map_err(|error| LabError::usage(error.message()))?;
                (
                    RecordPoint {
                        x: from.x,
                        y: from.y,
                    },
                    RecordPoint { x: end.x, y: end.y },
                    point_rule_of(&sampling),
                    Some(sampling),
                )
            }
            None => (
                rect_center(rect),
                rect_center(to),
                "rect_center".to_string(),
                None,
            ),
        },
        (ClickGesture::LongPress { .. }, None) => match &request.draw {
            Some(draw) => {
                let (point, sampling) = drawn_point(draw, rect)?;
                (point, point, point_rule_of(&sampling), Some(sampling))
            }
            None => {
                let point = rect_center(rect);
                (point, point, "rect_center".to_string(), None)
            }
        },
        (ClickGesture::LongPress { .. }, Some(_)) => {
            return Err(conflict(
                format!("step {index} declares a long press; --swipe executes a declared drag"),
                json!({"step": index, "gesture": gesture}),
            ));
        }
        // #411 Lab BD (and Workflow #406 T4): refused above, before `--tap` and `--swipe` are
        // looked at.
        (
            ClickGesture::SingleTouchDragWithVerticalBrakeV1 { .. }
            | ClickGesture::SingleTouchPathDragV1 { .. },
            _,
        ) => {
            return Err(gesture_execution_unsupported(index, gesture));
        }
    };
    Ok(super::model::ClickPlan {
        record_id: recording.record_id.clone(),
        step: index,
        rect,
        point,
        point_rule: rule,
        source: click.source.clone(),
        declares_click: false,
        gesture: Some(gesture),
        to_point: Some(to_point),
        sampling,
    })
}

/// #411 Lab BD: a step whose declared gesture a Lab operation cannot press (a brake drag); the
/// refusal comes before any Runtime request, capture or recording change.
fn gesture_execution_unsupported(index: u32, gesture: ClickGesture) -> LabError {
    with_details(
        blocked(
            LabCode::RecordGestureExecutionUnsupported.as_str(),
            format!(
                "step {index} declares a {}, which a Lab operation does not press; reach the \
                 screen after it by other means, then close the step with `record mark \
                 --close-step` or give its next frame with `record mark --frame <png>`; the \
                 generated package still declares the gesture",
                gesture.kind()
            ),
        ),
        json!({"step": index, "gesture": gesture}),
    )
}

/// Records the Runtime outcome of a planned click on its step. #463 S1-1: with `names_frames` (a
/// press recorded by the command that makes it, `record_press`), the execution names the step's
/// live primary frame as its `before_frame`, and the frame after it becomes its `after_frame`
/// ([`name_after_frame`]); `do --capture --record` names none. Fix round 3: beside
/// `before_frame`, `sent_on_unstored_capture` when such a press was sent on a capture its command
/// did not store (`record_press` hands it; every other caller passes `false`).
pub(crate) fn commit_click(
    recording: &mut LabRecording,
    plan: &super::model::ClickPlan,
    request: &super::model::CommitClickRequest,
    names_frames: bool,
    sent_on_unstored_capture: bool,
) -> LabResult<(StepClick, bool)> {
    use super::model::ClickEffect;
    if recording.record_id != plan.record_id || open_step(recording) != Some(plan.step) {
        return Err(blocked(
            "record_step_not_found",
            format!(
                "the planned step {} is no longer the open step of recording {}",
                plan.step, plan.record_id
            ),
        ));
    }
    let now = now_unix_ms();
    let step = step_mut(recording, plan.step)?;
    let before_frame = if names_frames {
        live_primary(step).map(|frame| frame.frame_id.clone())
    } else {
        None
    };
    if plan.declares_click {
        step.click = Some(StepClick {
            rect: plan.rect,
            source: plan.source.clone(),
            from: None,
            follow: None,
            // Workflow #449 REC-ARCH: a recorded swipe declares its drag; `--tap-rect` a tap.
            gesture: plan.gesture,
            retry: None,
            declared_at_unix_ms: now,
            execution: None,
            attempts: Vec::new(),
            needs_review: false,
        });
    }
    let performed = request.effect == ClickEffect::Performed;
    let click = step
        .click
        .as_mut()
        .ok_or_else(|| click_missing_for_do(plan.step))?;
    click.execution = Some(ClickExecution {
        point: plan.point,
        point_rule: plan.point_rule.clone(),
        effect: if performed {
            "performed"
        } else {
            "indeterminate"
        }
        .to_string(),
        carrier_package: request.carrier_package.clone(),
        req_id: request.req_id.clone(),
        correlation_id: request.correlation_id.clone(),
        action_id: request.action_id.clone(),
        lease_id: request.lease_id.clone(),
        failure: request.failure.clone(),
        before: request.before.clone(),
        after: request.after.clone(),
        executed_at_unix_ms: now,
        to_point: plan.to_point,
        duration_ms: plan.gesture.map(ClickGesture::duration_ms),
        sampling: plan.sampling.clone(),
        before_frame,
        after_frame: None,
        after_capture: None,
        sent_on_unstored_capture,
    });
    if performed {
        click.needs_review = request.has_failure;
    }
    let click = click.clone();
    if performed {
        step.closed = true;
        step.closed_by = Some("click".to_string());
    }
    Ok((click, performed))
}

/// `session app <verb> --record`: the step the application operation lands on (R24 section
/// 2.5), decided before anything is sent. Every refusal here means nothing was sent.
pub(crate) fn plan_application(
    recording: &LabRecording,
    request: &super::model::PlanApplicationRequest,
) -> LabResult<super::model::ApplicationPlan> {
    let action = application_action(&request.verb)?;
    let hint = "one step has one effect: capture the next screen with capture --record first (no \
                mark is needed), or replace a declared effect with `record mark --application \
                <action> --replace-click`";
    let plan = |step: u32, opens_entry_step: bool| super::model::ApplicationPlan {
        record_id: recording.record_id.clone(),
        step,
        opens_entry_step,
        action: action.to_string(),
        cli_verb: request.verb.clone(),
    };
    let Some(index) = open_step(recording) else {
        if !effective_indices(recording).is_empty() {
            return Err(application_entry_invalid());
        }
        return Ok(plan(next_step_index(recording)?, true));
    };
    let step = effective_step(recording, index)?;
    match (&step.click, &step.application) {
        (Some(_), _) => Err(effect_exists(index, "click", hint)),
        (None, Some(existing)) if existing.action != action || existing.executed.is_some() => {
            Err(with_details(
                effect_exists(index, "application", hint),
                json!({
                    "step": index,
                    "effect": "application",
                    "declared_action": existing.action,
                    "requested_action": action
                }),
            ))
        }
        // The operation would land on an optional step (Workflow #339): refused before any
        // request, as `record mark --application` is.
        _ if step.optional.is_some() => Err(optional_application(index, action)),
        // #463 S1-3: the open step's effect, its marks or none (their check is `record stop`'s).
        (None, _) => Ok(plan(index, false)),
    }
}

/// Records the Runtime result of a planned application operation. Performed: the completed
/// receipt is the step's executed effect and closes it (`closed_by:"application"`); #463 S1-3: it
/// names the step's live primary frame as its `before_frame` (none on the entry step), and its
/// screen after becomes its `after_frame` ([`name_after_frame`]: the last capture of its after
/// loop, or, when that loop stored nothing, the first frame that opens a step after it); #463
/// S1-5: and it is stamped `executed_at_unix_ms`, on the clock a click's execution is stamped with.
/// Indeterminate: the step declares the operation with one more attempt and stays open.
/// Returns the step's application, whether it was opened and whether it was closed.
pub(crate) fn commit_application(
    recording: &mut LabRecording,
    plan: &super::model::ApplicationPlan,
    request: &super::model::CommitApplicationRequest,
) -> LabResult<(StepApplication, bool, bool)> {
    use super::model::CommitApplicationRequest;
    let still_planned = recording.record_id == plan.record_id
        && if plan.opens_entry_step {
            effective_indices(recording).is_empty()
                && next_step_index(recording).ok() == Some(plan.step)
        } else {
            open_step(recording) == Some(plan.step)
        };
    if !still_planned {
        return Err(blocked(
            "record_step_not_found",
            format!(
                "the planned step {} is no longer the step the application operation lands on \
                 in recording {}",
                plan.step, plan.record_id
            ),
        ));
    }
    if plan.opens_entry_step {
        recording.steps.push(new_step(plan.step, Vec::new()));
    }
    let step = step_mut(recording, plan.step)?;
    let before_frame = live_primary(step).map(|frame| frame.frame_id.clone());
    let application = step.application.get_or_insert_with(|| StepApplication {
        action: plan.action.clone(),
        cli_verb: plan.cli_verb.clone(),
        source: "declared".to_string(),
        executed: None,
        attempts: Vec::new(),
        needs_review: false,
    });
    let performed = match request {
        CommitApplicationRequest::Performed(execution) => {
            application.executed = Some(super::model::ApplicationExecution {
                before_frame,
                executed_at_unix_ms: Some(now_unix_ms()),
                ..execution.clone()
            });
            application.source = "executed".to_string();
            application.cli_verb = plan.cli_verb.clone();
            true
        }
        CommitApplicationRequest::Indeterminate(attempt) => {
            application.attempts.push(attempt.clone());
            false
        }
    };
    let application = application.clone();
    if performed {
        step.closed = true;
        step.closed_by = Some("application".to_string());
    }
    Ok((application, plan.opens_entry_step, performed))
}

/// The JSON of a value for error details.
pub(crate) fn to_json<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}
