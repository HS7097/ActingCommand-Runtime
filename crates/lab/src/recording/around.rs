// SPDX-License-Identifier: AGPL-3.0-only

//! #463 S2-13 (`contracts/lab-recording.md`, "The frames around a press: `record frames`"): the
//! stored frames just before and just after the press of one recorded step, which stage two of
//! package making asks for when the frame a press happened on and the final screen do not show
//! what the press did. The read takes no lock and writes nothing, and no frame is decoded,
//! compared or copied: it lists what the step table holds.
//!
//! - **The sequence.** Every frame the step table stores under a step, its page transition's too,
//!   superseded ones included, in the order the frames entered the step table (the number of
//!   `frame_id`, `f0001`, `f0002`, …, given as each entered). A frame captured from the device
//!   enters when it is captured; a frame attached from a file (a mark's sample, a page
//!   transition's frame, an offline frame, a step inserted after the stop) enters when it is
//!   attached, and its time (`recorded_at_unix_ms`) is that moment, not the moment its screen was
//!   captured. An ending's frames (#463 R2), taken from a later run and stored under no step, are
//!   not in it, as the archive's manifest lists none either.
//! - **The press.** Its time (`executed_at_unix_ms`), its point and its end point, as the step's
//!   click execution holds them, and the frame it was sent on (`before_frame`). A step without an
//!   executed press (an application operation, an entry launch, a step without an effect) has no
//!   point; its time and its frame are those its application operation's completed receipt names
//!   (`executed_at_unix_ms`, recorded since #463 S1-5, and `before_frame`), when it names them.
//! - **The two sides.** With a frame the press was sent on, `before` are the frames before it in
//!   the sequence and `after` those after it; a press sent on a capture that was not stored names
//!   its step's stored frame instead, and of the frames after that one only those recorded after
//!   the press's time are `after`, the others (a mark's sample, a transition's frame) `before`.
//!   Without a frame but with a time (a press an earlier build recorded), those recorded at or
//!   before the time and those recorded after it. With neither, those stored under the steps
//!   numbered below the step, and those stored under it and the steps numbered above it. Each side
//!   keeps the sequence's order; `before` lists its last `n`, `after` its first `n`.

use super::archive::{instance_mismatch, read_recording, step_kind, stored_frame_path};
use super::model::{LabRecording, RecordPoint, RecordedFrame, RecordingStep, StepTransition};
use super::steps::{effective_indices, effective_step, lab_unavailable, not_active};
use super::store::{LabFile, lab_dir, load_lab, read_old_record, with_details};
use actingcommand_contract::{LabError, LabResult};
use serde::Serialize;
use serde_json::json;
use std::path::Path;

/// How many frames each side lists when none is given (`--before`, `--after`), and the most either
/// may ask for.
const DEFAULT_BEFORE: u64 = 3;
const DEFAULT_AFTER: u64 = 10;
const MAX_FRAMES: u64 = 100;

/// The warning of a listed frame whose file is not where the manifest names it. #463 S2-2 fix round
/// 1: `record review` warns of such a frame with it too.
pub(crate) const FRAME_FILE_MISSING: &str = "frame_file_missing";
/// The warning of a press sent on a capture its command did not store
/// (`sent_on_unstored_capture`): `press.frame` is the stored frame its step names instead.
const PRESS_FRAME_NOT_STORED: &str = "press_frame_not_stored";

/// `record frames`: the recording (`record_id`, or the one the instance's session header names),
/// the step's serial number and how many frames each side lists (`None`: 3 before, 10 after).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecordFramesRequest {
    pub record_id: Option<String>,
    pub step: u32,
    pub before: Option<u64>,
    pub after: Option<u64>,
}

/// The answer of `record frames`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordFramesOutcome {
    pub record_id: String,
    pub step: u32,
    /// The step's number in the package (`step_<nn>`), as `record status` gives it.
    pub artifact_step: Option<u32>,
    /// The step's kind, as the archive's manifest names it.
    pub kind: String,
    pub press: RecordFramesPress,
    pub before: Vec<RecordFramesFrame>,
    pub after: Vec<RecordFramesFrame>,
    /// How many frames each side holds in all, listed or not.
    pub before_total: usize,
    pub after_total: usize,
    pub warnings: Vec<RecordFramesWarning>,
}

/// The step's press as the step table holds it: its time (an application operation's completed
/// receipt's), its point and its end point (none on a step without an executed press) and the
/// stored frame it was sent on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordFramesPress {
    pub at_unix_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub point: Option<RecordPoint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_point: Option<RecordPoint>,
    pub frame: Option<RecordFramesPressFrame>,
}

/// The stored frame a press was sent on. `path` is `None` when its file is missing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordFramesPressFrame {
    pub frame_id: String,
    pub path: Option<String>,
    pub sha256: String,
    pub captured_at_unix_ms: u64,
    pub stored_under_step: u32,
}

/// One stored frame of a side. `offset_ms` is its capture time minus the press's time, `None`
/// without a press time; `path` is `None` when its file is missing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordFramesFrame {
    pub frame_id: String,
    pub path: Option<String>,
    pub sha256: String,
    pub captured_at_unix_ms: u64,
    pub offset_ms: Option<i64>,
    pub stored_under_step: u32,
}

/// A warning of the answer: its word and the frames it names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordFramesWarning {
    pub code: &'static str,
    pub frame_ids: Vec<String>,
}

/// One frame of the sequence and the step it is stored under. #463 S2-2: `record review` reads the
/// same sequence.
pub(crate) struct Stored<'a> {
    pub(crate) frame: &'a RecordedFrame,
    pub(crate) step: u32,
}

/// What the step table holds of a step's press.
struct Pressed<'a> {
    at_unix_ms: Option<u64>,
    point: Option<RecordPoint>,
    to_point: Option<RecordPoint>,
    frame_id: Option<&'a str>,
    unstored: bool,
}

/// `record frames` (#463 S2-13): the stored frames just before and just after the press of step
/// `request.step` of the recording `record status` reads for `instance` under `state_dir` (or of
/// the one `request.record_id` names: active, archived or stopped). Offline: no lock is taken and
/// nothing is written. Refused: a side outside 0..=100 (`validation_failed`, `details{argument,
/// reason: "range", value, min, max}`), a step that does not exist, is dropped or was converted
/// into a transition (`record_step_not_found`), no recording to read
/// (`record_session_not_active`, `record_lab_unavailable`, `record_instance_mismatch`). A frame
/// whose file is missing is listed with no `path` and named in the warning `frame_file_missing`; a
/// press sent on a capture that was not stored is named in `press_frame_not_stored`, and its
/// `after` are the frames recorded after its time.
pub fn record_frames(
    state_dir: &Path,
    instance: &str,
    request: &RecordFramesRequest,
) -> LabResult<RecordFramesOutcome> {
    let before = side_count("before", request.before, DEFAULT_BEFORE)?;
    let after = side_count("after", request.after, DEFAULT_AFTER)?;
    let recording = read_named(state_dir, instance, request.record_id.as_deref())?;
    let step = effective_step(&recording, request.step)?;
    let dir = lab_dir(state_dir, &recording.record_id);
    let pressed = press_record(step);
    let sequence = sequence_of(&recording);
    let position = pressed.frame_id.and_then(|frame_id| {
        sequence
            .iter()
            .position(|stored| stored.frame.frame_id == frame_id)
    });
    let (earlier, later): (Vec<&Stored<'_>>, Vec<&Stored<'_>>) =
        match (position, pressed.at_unix_ms) {
            // Sent on a capture that was not stored: the frame is the step's stored one, and a
            // frame stored after it but before the press was sent (a mark's sample, a
            // transition's frame) is before the press.
            (Some(at), Some(time)) if pressed.unstored => {
                let (between, sent_after): (Vec<&Stored<'_>>, Vec<&Stored<'_>>) = sequence
                    [at + 1..]
                    .iter()
                    .partition(|stored| stored.frame.recorded_at_unix_ms <= time);
                (sequence[..at].iter().chain(between).collect(), sent_after)
            }
            (Some(at), _) => (
                sequence[..at].iter().collect(),
                sequence[at + 1..].iter().collect(),
            ),
            (None, Some(at)) => sequence
                .iter()
                .partition(|stored| stored.frame.recorded_at_unix_ms <= at),
            (None, None) => sequence.iter().partition(|stored| stored.step < step.index),
        };
    let mut missing = Vec::new();
    let frame = position.map(|at| {
        let stored = &sequence[at];
        RecordFramesPressFrame {
            frame_id: stored.frame.frame_id.clone(),
            path: located(&dir, stored.frame, &mut missing),
            sha256: stored.frame.sha256.clone(),
            captured_at_unix_ms: stored.frame.recorded_at_unix_ms,
            stored_under_step: stored.step,
        }
    });
    let first = earlier.len().saturating_sub(before);
    let before_frames = earlier[first..]
        .iter()
        .map(|stored| listed(&dir, stored, pressed.at_unix_ms, &mut missing))
        .collect();
    let after_frames = later
        .iter()
        .take(after)
        .map(|stored| listed(&dir, stored, pressed.at_unix_ms, &mut missing))
        .collect();
    let mut warnings = Vec::new();
    if pressed.unstored {
        warnings.push(RecordFramesWarning {
            code: PRESS_FRAME_NOT_STORED,
            frame_ids: pressed.frame_id.map(str::to_owned).into_iter().collect(),
        });
    }
    if !missing.is_empty() {
        warnings.push(RecordFramesWarning {
            code: FRAME_FILE_MISSING,
            frame_ids: missing,
        });
    }
    Ok(RecordFramesOutcome {
        record_id: recording.record_id.clone(),
        step: step.index,
        artifact_step: effective_indices(&recording)
            .iter()
            .position(|index| *index == step.index)
            .and_then(|position| u32::try_from(position + 1).ok()),
        kind: step_kind(step).to_owned(),
        press: RecordFramesPress {
            at_unix_ms: pressed.at_unix_ms,
            point: pressed.point,
            to_point: pressed.to_point,
            frame,
        },
        before: before_frames,
        after: after_frames,
        before_total: earlier.len(),
        after_total: later.len(),
        warnings,
    })
}

/// How many frames the side `argument` lists: `given`, or `default` when none is given; outside
/// 0..=100 `validation_failed`, `details{argument, reason: "range", value, min, max}`, as the waits
/// of a recorded press are refused.
fn side_count(argument: &str, given: Option<u64>, default: u64) -> LabResult<usize> {
    let value = given.unwrap_or(default);
    if value > MAX_FRAMES {
        return Err(LabError::usage(format!(
            "--{argument} (MCP {argument}) must be in 0..={MAX_FRAMES}, got {value}"
        ))
        .with_details(json!({
            "argument": argument,
            "reason": "range",
            "value": value,
            "min": 0,
            "max": MAX_FRAMES
        })));
    }
    Ok(usize::try_from(value).unwrap_or(usize::MAX))
}

/// The recording `record_id` names, read by its id whatever its state; without one the recording
/// the instance's session header names, as `record status` reads it.
fn read_named(
    state_dir: &Path,
    instance: &str,
    record_id: Option<&str>,
) -> LabResult<LabRecording> {
    if let Some(record_id) = record_id {
        let recording = read_recording(state_dir, record_id)?
            .ok_or_else(|| lab_unavailable("lab_recording_missing"))?;
        if recording.instance != instance {
            return Err(instance_mismatch(&recording.instance, instance));
        }
        return Ok(recording);
    }
    let Some(old) = read_old_record(state_dir, instance)? else {
        return Err(with_details(
            not_active(format!(
                "no recording session exists for {instance}; name a recording with --record-id"
            )),
            json!({"instance": instance}),
        ));
    };
    if old.instance != instance {
        return Err(instance_mismatch(&old.instance, instance));
    }
    match load_lab(state_dir, &old)? {
        LabFile::Ready(recording) => Ok(*recording),
        LabFile::Missing => Err(lab_unavailable("lab_recording_missing")),
        LabFile::Unavailable(reason) => Err(lab_unavailable(reason)),
    }
}

/// What the step table holds of the press of `step`: its click execution's time, point, end point
/// and `before_frame`; without one, the time (#463 S1-5) and the `before_frame` of its application
/// operation's completed receipt, when it names them, and no point. #463 KR-1: a key step's, the
/// press its key replaced, whose frames stand.
fn press_record(step: &RecordingStep) -> Pressed<'_> {
    if let Some(execution) = step
        .recorded_click()
        .and_then(|click| click.execution.as_ref())
    {
        return Pressed {
            at_unix_ms: Some(execution.executed_at_unix_ms),
            point: Some(execution.point),
            to_point: execution.to_point,
            frame_id: execution.before_frame.as_deref(),
            unstored: execution.sent_on_unstored_capture,
        };
    }
    let receipt = step
        .application
        .as_ref()
        .and_then(|application| application.executed.as_ref());
    Pressed {
        at_unix_ms: receipt.and_then(|completed| completed.executed_at_unix_ms),
        point: None,
        to_point: None,
        frame_id: receipt.and_then(|completed| completed.before_frame.as_deref()),
        unstored: false,
    }
}

/// Every frame the step table stores under a step, its page transition's too, in the order of its
/// frame ids' numbers, each with its step.
pub(crate) fn sequence_of(recording: &LabRecording) -> Vec<Stored<'_>> {
    let mut sequence = Vec::new();
    for step in &recording.steps {
        let transition = match &step.transition {
            Some(StepTransition::Page { frames, .. }) => frames.as_slice(),
            _ => &[][..],
        };
        sequence.extend(step.frames.iter().chain(transition).map(|frame| Stored {
            frame,
            step: step.index,
        }));
    }
    sequence.sort_by_key(|stored| arrival_number(&stored.frame.frame_id));
    sequence
}

/// The number the step table gave a frame as it arrived (`f0007` is 7); one that is no such id
/// comes last.
fn arrival_number(frame_id: &str) -> u32 {
    frame_id
        .strip_prefix('f')
        .and_then(|digits| digits.parse().ok())
        .unwrap_or(u32::MAX)
}

/// The absolute path of `frame`'s stored file in the recording's directory `dir`, as the archive's
/// manifest names it; `None`, and its id added to `missing`, when no file is there. #463 S2-2 fix
/// round 1: `record review` gives each frame's path by it.
pub(crate) fn located(
    dir: &Path,
    frame: &RecordedFrame,
    missing: &mut Vec<String>,
) -> Option<String> {
    let path = stored_frame_path(dir, frame);
    if Path::new(&path).is_file() {
        Some(path)
    } else {
        missing.push(frame.frame_id.clone());
        None
    }
}

/// One frame of a side, its offset from the press's time `at`.
fn listed(
    dir: &Path,
    stored: &Stored<'_>,
    at: Option<u64>,
    missing: &mut Vec<String>,
) -> RecordFramesFrame {
    let captured = stored.frame.recorded_at_unix_ms;
    RecordFramesFrame {
        frame_id: stored.frame.frame_id.clone(),
        path: located(dir, stored.frame, missing),
        sha256: stored.frame.sha256.clone(),
        captured_at_unix_ms: captured,
        offset_ms: at.and_then(|at| i64::try_from(i128::from(captured) - i128::from(at)).ok()),
        stored_under_step: stored.step,
    }
}
