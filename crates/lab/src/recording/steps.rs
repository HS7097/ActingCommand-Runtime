// SPDX-License-Identifier: AGPL-3.0-only

//! Steps by serial index, frame arrival, marks, clicks, transitions and the remediation
//! operations. Everything is computed in memory and written once, atomically.

use super::RecordingLock;
use super::frames::{
    LoadedFrame, check_frame_size, crop_store_path, frame_store_path, read_frame_file,
    read_stored_frame, rect_inside,
};
use super::marks::{
    EvalFrame, MarkRejection, derive_check, evaluate_mark, prepare_mark, validate_mark_id,
};
use super::model::{
    ApplicationAttempt, ClickExecution, LAB_RECORDING_DEFAULT_COLOR_MAX_DISTANCE,
    LAB_RECORDING_DEFAULT_MATCH_METRIC, LAB_RECORDING_DEFAULT_SETTLE_MS,
    LAB_RECORDING_DEFAULT_TEMPLATE_THRESHOLD, LAB_RECORDING_SCHEMA, LabRecording, MarkFamily,
    MarkSpec, OpaqueJson, RecordPoint, RecordRect, RecordedFrame, RecordedMark, RecordingDefaults,
    RecordingStep, StepApplication, StepClick, StepOptional, StepTransition,
};
use super::store::{
    LabFile, OldRecordView, blocked, create_recording, invalid, lab_dir, load_lab, now_unix_ms,
    read_old_record, read_verified, save_recording, task_id_valid_for_lab, with_details,
    write_content_addressed,
};
use actingcommand_contract::{LabError, LabResult};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

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

pub(crate) fn new_recording(
    old: &OldRecordView,
    defaults: RecordingDefaults,
    game: Option<String>,
    server: Option<String>,
    locale: Option<String>,
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
    }
}

/// The active recording of the lock's instance; a v0.9.0 session without a Lab recording
/// gets one lazily, with the default settings.
pub(crate) fn open_session(lock: &RecordingLock) -> LabResult<Session> {
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
                new_recording(&old, default_recording_defaults(), None, None, None),
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
    })
}

/// Writes the pending frames and crops, then the recording. A failure names what was
/// already written; nothing is deleted.
pub(crate) fn save(session: &mut Session) -> LabResult<Vec<String>> {
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

pub(crate) fn effective_indices(recording: &LabRecording) -> Vec<u32> {
    let mut indices = recording
        .steps
        .iter()
        .filter(|step| step.is_effective())
        .map(|step| step.index)
        .collect::<Vec<_>>();
    indices.sort_unstable();
    indices
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

fn effective_step(recording: &LabRecording, index: u32) -> LabResult<&RecordingStep> {
    step_ref(recording, index)
        .filter(|step| step.is_effective())
        .ok_or_else(|| step_not_found(index))
}

/// The highest `fNNNN` frame number on record, steps and transitions included.
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

/// Checks the size, queues the content-addressed write and keeps the decoded frame.
fn admit_frame(
    session: &mut Session,
    loaded: LoadedFrame,
    role: &str,
    meta: &FrameMeta,
    label: &str,
) -> LabResult<RecordedFrame> {
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
}

/// The sha256 of the step's live (not superseded) primary frame.
fn live_primary_sha(step: &RecordingStep) -> Option<String> {
    step.frames
        .iter()
        .rev()
        .find(|frame| frame.role == "primary" && !frame.superseded)
        .map(|frame| frame.sha256.clone())
}

fn has_marks(step: &RecordingStep) -> bool {
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
        transition: None,
        closed: false,
        closed_by: None,
    }
}

pub(crate) fn arrive(
    session: &mut Session,
    loaded: LoadedFrame,
    mode: ArrivalMode,
    meta: &FrameMeta,
    label: &str,
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
            })
        }
        ArrivalPlan::Open { close } => {
            if let Some((index, by)) = close {
                let step = step_mut(&mut session.recording, index)?;
                step.closed = true;
                step.closed_by = Some(by.to_string());
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
            })
        }
    }
}

/// Every mark id in use: own marks of effective steps and the marks of their transitions.
fn ids_in_use(recording: &LabRecording) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for step in recording.steps.iter().filter(|step| step.is_effective()) {
        ids.extend(step.marks.iter().map(|mark| mark.id.clone()));
        if let Some(StepTransition::Page { marks, .. }) = &step.transition {
            ids.extend(marks.iter().map(|mark| mark.id.clone()));
        }
    }
    ids
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

/// Self-test of every own and reused mark of the target on its live frames, checks last.
fn evaluate_batch(
    session: &Session,
    batch: &MarkBatch,
    live: &[RecordedFrame],
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
        let (test, failure) = evaluate_mark(mark, crop.as_deref(), &frames, defaults)?;
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
        let (test, failure) = evaluate_mark(&mark, crop.as_deref(), &frames, defaults)?;
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
    let mut in_use = ids_in_use(&session.recording);
    for id in released {
        in_use.remove(id);
    }
    in_use.extend(batch.own.iter().map(|mark| mark.id.clone()));
    let mut assets = template_assets(&session.recording);
    assets.retain(|_, id| !released.contains(id));
    let now = now_unix_ms();
    for spec in specs {
        validate_mark_id(&spec.id)?;
        if !in_use.insert(spec.id.clone()) {
            return Err(with_details(
                blocked(
                    "record_mark_id_conflict",
                    format!("mark id '{}' is already used in this recording", spec.id),
                ),
                json!({"id": spec.id}),
            ));
        }
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
        if let (Some(crop), Some(png)) = (mark.crop.as_mut(), prepared.crop_png) {
            if let Some(owner) = assets.get(&crop.asset)
                && owner != &spec.id
            {
                return Err(with_details(
                    blocked(
                        "record_asset_name_conflict",
                        format!(
                            "template '{}' maps to {} which template '{owner}' already uses",
                            spec.id, crop.asset
                        ),
                    ),
                    json!({"id": spec.id, "asset": crop.asset, "existing": owner}),
                ));
            }
            assets.insert(crop.asset.clone(), spec.id.clone());
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
fn reused_elsewhere(recording: &LabRecording, index: u32, ids: &BTreeSet<String>) -> Vec<String> {
    let mut used = BTreeSet::new();
    for step in recording.steps.iter().filter(|step| step.is_effective()) {
        let transition_reused = match &step.transition {
            Some(StepTransition::Page { reused, .. }) => reused.as_slice(),
            _ => &[][..],
        };
        if step.index != index {
            for id in step.reused.iter().chain(transition_reused) {
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
}

pub(crate) fn apply_marks(
    session: &mut Session,
    request: &super::model::MarkRequest,
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
    // The application entry step has no frame: only its effect can be declared on it.
    if step_ref(&session.recording, target).is_some_and(|step| live_primary_sha(step).is_none()) {
        let needs_frame = request.page.is_some()
            || !request.samples.is_empty()
            || !request.add.is_empty()
            || !request.reuse.is_empty()
            || !request.remove.is_empty()
            || request.click.is_some()
            || request.click_guard.is_some()
            || request.retry.is_some()
            || (request.application.is_none() && request.optional.is_none());
        if needs_frame {
            return Err(with_details(
                blocked(
                    "record_step_frame_missing",
                    format!(
                        "step {target} is the application entry step and has no frame; pages, \
                         marks, samples and clicks need a step with a frame"
                    ),
                ),
                json!({"step": target}),
            ));
        }
        apply_application(session, request, target, None)?;
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
        });
    }
    if let Some(page) = &request.page {
        step_mut(&mut session.recording, target)?.page = Some(page.clone());
    }

    // remove
    let mut removed = Vec::new();
    let mut rejections = Vec::new();
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
            let in_step = own_ids.contains(id) || step.reused.contains(id);
            if !in_step {
                rejections.push(MarkRejection::new(id, "mark_not_in_step", json!({})));
                continue;
            }
            let named_by_check = step.marks.iter().any(|mark| {
                mark.check_members().contains(id) && !request.remove.contains(&mark.id)
            });
            let guard = step.click_guard.as_deref() == Some(id.as_str());
            if elsewhere.contains(id) || named_by_check || guard {
                return Err(with_details(
                    blocked(
                        "record_mark_in_use",
                        format!(
                            "mark '{id}' is reused by another step, named by a check or is the click guard"
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
    add_marks(
        session,
        &mut batch,
        &request.add,
        &primary_sha,
        target,
        None,
        &released,
        &mut rejections,
    )?;
    add_reuse(session, &mut batch, &request.reuse, Some(target))?;

    // samples
    let mut samples = Vec::new();
    for path in &request.samples {
        let loaded = read_frame_file(path)?;
        samples.push(admit_frame(session, loaded, "sample", &local, path)?);
    }
    let mut live = live_existing;
    live.extend(samples.iter().cloned());

    // effect: a click or an application operation
    if request.application.is_some() {
        apply_application(session, request, target, Some(&batch))?;
    } else {
        apply_click(session, request, target, &batch)?;
    }
    apply_optional(&mut session.recording, request, target)?;

    // self-test
    let total = batch.own.len() + batch.reused.len();
    let result = evaluate_batch(session, &batch, &live)?;
    rejections.extend(result.rejections);
    if !rejections.is_empty() {
        return Err(rejected(&rejections, total));
    }
    queue_crops(session, &batch);
    let added = result
        .own
        .iter()
        .filter(|mark| batch.added_ids.contains(&mark.id))
        .cloned()
        .collect::<Vec<_>>();
    let step = step_mut(&mut session.recording, target)?;
    step.marks = result.own;
    step.reused = batch.reused;
    step.frames.extend(samples.iter().cloned());
    Ok(MarkApplied {
        step: target,
        opened,
        closed_step,
        frame: new_frame,
        samples,
        added,
        reused: result.reused,
        removed,
    })
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
        Some(spec) => {
            let (rect, source, from) = match (&spec.region, &spec.from) {
                (Some(rect), None) => {
                    if !size.is_some_and(|size| rect_inside(*rect, size)) {
                        return Err(invalid(
                            "validation_failed",
                            "the click rectangle must lie inside the frame with width and height >= 1",
                        ));
                    }
                    (*rect, "declared", None)
                }
                (None, Some(id)) => {
                    let mark = own_or_reused(id).ok_or_else(|| {
                        blocked(
                            "record_click_source_invalid",
                            format!("click source '{id}' is not a mark of this step"),
                        )
                    })?;
                    let region = mark.region.filter(|_| mark.family != MarkFamily::Check);
                    let Some(region) = region else {
                        return Err(blocked(
                            "record_click_source_invalid",
                            format!("click source '{id}' is a check and has no region"),
                        ));
                    };
                    (region, "from_mark", Some(id.clone()))
                }
                _ => {
                    return Err(invalid(
                        "validation_failed",
                        "a click needs exactly one of a region (--click) and a source mark (--click-from)",
                    ));
                }
            };
            Some((rect, source, from))
        }
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
            if mark.family == MarkFamily::Ocr {
                return Err(blocked(
                    "record_guard_family_invalid",
                    format!(
                        "click guard '{id}' is an OCR mark; a guard must be a template, color, \
                         color_digest or check mark"
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
    if let Some((rect, source, from)) = declared {
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
        step.click = Some(StepClick {
            rect,
            source: source.to_string(),
            from,
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
    Ok(())
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
         capture the arrival screen with capture --record and mark it, then execute or declare \
         the application operation on that step; offline, give the arrival screen with --frame",
    )
}

fn application_marks_missing(index: u32) -> LabError {
    with_details(
        blocked(
            "record_application_step_marks_missing",
            format!(
                "step {index} has a frame but no marks; mark the screen the application \
                 operation starts from first"
            ),
        ),
        json!({"step": index}),
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
/// step's effect. `batch` holds the step's marks after this command (none for the entry step).
fn apply_application(
    session: &mut Session,
    request: &super::model::MarkRequest,
    target: u32,
    batch: Option<&MarkBatch>,
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
    if let Some(batch) = batch
        && !step.closed
        && batch.own.is_empty()
        && batch.reused.is_empty()
    {
        return Err(application_marks_missing(target));
    }
    let attempts = step
        .application
        .as_ref()
        .map(|application| application.attempts.clone())
        .unwrap_or_default();
    step.click = None;
    step.click_guard = None;
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
        return Err(with_details(
            blocked(
                "record_optional_application",
                format!(
                    "step {target} has the application operation {}; an application step cannot \
                     be optional (there is no conditional restart): mark the screens after it \
                     optional, or clear the step with --not-optional",
                    application.action
                ),
            ),
            json!({"step": target, "application": application.action}),
        ));
    }
    Ok(())
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
    let (frame, samples, add, reuse, timeout_ms) = match spec {
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
        } => (frame, samples, add, reuse, *timeout_ms),
    };
    validate_page_transition_timeout(timeout_ms)?;
    refuse_existing(index, existing.as_ref(), request.replace_transition)?;
    let frame_path = frame.as_ref().ok_or_else(|| {
        invalid(
            "validation_failed",
            "a page transition needs --frame <png> of the intermediate screen",
        )
    })?;
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
    let local = FrameMeta::local();
    let loaded = read_frame_file(frame_path)?;
    let primary = admit_frame(session, loaded, "transition", &local, frame_path)?;
    let mut frames = vec![primary.clone()];
    for path in samples {
        let loaded = read_frame_file(path)?;
        frames.push(admit_frame(
            session,
            loaded,
            "transition_sample",
            &local,
            path,
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
    let result = evaluate_batch(session, &batch, &frames)?;
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

/// The step a `--drop-step`/`--reopen-step` names: effective and the last effective one.
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
/// the step acted on.
pub(crate) fn apply_step_action(
    session: &mut Session,
    action: &super::model::StepAction,
) -> LabResult<(&'static str, u32)> {
    use super::model::StepActionKind;
    let recording = &mut session.recording;
    match action.kind {
        StepActionKind::DropStep => {
            let index = last_named(recording, action.step, "--drop-step")?;
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
                    other.reused.iter().chain(transition_reused)
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
    }
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
    let (point, point_rule) = match request.tap {
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
            (point, "explicit")
        }
        None => (rect_center(rect), "rect_center"),
    };
    Ok(super::model::ClickPlan {
        record_id: recording.record_id.clone(),
        step: index,
        rect,
        point,
        point_rule: point_rule.to_string(),
        source,
        declares_click,
    })
}

/// Records the Runtime outcome of a planned click on its step.
pub(crate) fn commit_click(
    recording: &mut LabRecording,
    plan: &super::model::ClickPlan,
    request: &super::model::CommitClickRequest,
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
    if plan.declares_click {
        step.click = Some(StepClick {
            rect: plan.rect,
            source: plan.source.clone(),
            from: None,
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
    let hint = "one step has one effect: capture and mark the next screen with capture --record \
                first, or replace a declared effect with `record mark --application <action> \
                --replace-click`";
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
        (None, Some(_)) => Ok(plan(index, false)),
        (None, None) if !has_marks(step) => Err(application_marks_missing(index)),
        (None, None) => Ok(plan(index, false)),
    }
}

/// Records the Runtime result of a planned application operation. Performed: the completed
/// receipt is the step's executed effect and closes it (`closed_by:"application"`).
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
            application.executed = Some(execution.clone());
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
