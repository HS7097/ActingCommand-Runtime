// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #392 DA-5cd (`contracts/display-adapter.md` section 12; `contracts/lab-recording.md`,
//! "A recording's size"): what the Lab does with a scene or a recording that is not its package's
//! size.
//!
//! - **Offline recognition.** A scene of its package's `coordinate_space` (its base) is evaluated
//!   as it is, byte for byte as before. The train of DA-5 (register §1.436 and §1.437, "Train"):
//!   any other scene is judged, reduced and answered exactly as a frame captured from a display of
//!   its size is by the Lab's live observation (DA-5ab), by the same function, the kernel's frame
//!   gate for a Lab frame (`lab_scene_on_base`: the gate's map, built with no producer geometry and
//!   no game declaration, and its one resample): a scene of the base's ratio, not smaller and in
//!   its orientation, is reduced to the base and evaluated there (a 5/4 one too: a read is not
//!   refused for whether a press could land exactly), and the answer names the base and the size
//!   it was reduced from (`frame_coordinates` in its base form, `FrameCoordinates::observed`);
//!   every rectangle and point in it is the base's. A scene the gate refuses is refused
//!   `lab_scene_size_unsupported`, the gate's word for why in `display_problem`.
//! - **Recordings.** A recording is made at the base size (`STANDARD_PACK_BASE_SIZE`): the refusal
//!   `record_display_not_base` (this instance's display is not the base size) of `record start` on
//!   a display the Runtime last observed at another size and (fix round 1) of the first frame of a
//!   recording whose header names the base size, a frame of another size; and `record_size_not_base`
//!   (this recording holds frames that are not the base size) of `record stop`, `record ready`'s
//!   item and a frame added to such a recording.

use crate::FrameCoordinates;
use crate::codes::LabCode;
use crate::recording::model::{LabRecording, RecordSize};
use actingcommand_contract::outcome::{Detail, Outcome, VocabularyToken};
use actingcommand_contract::{
    CaptureExtent, DisplayMapKind, LabError, LabErrorClass, LabResult, STANDARD_PACK_BASE_SIZE,
};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_display_adapter::{DisplayMap, DisplayObservation, DisplayProblem};
use actingcommand_execution_kernel::{ContainedTaskError, lab_scene_on_base};
use actingcommand_recognition::Scene;
use actingcommand_recognition_pack::RecognitionEvaluator;
use serde_json::json;

/// The base (width, height) `evaluator`'s package declares, its `coordinate_space`, which a scene
/// is brought to; `None` when it declares none, and then every scene is evaluated as it is.
pub fn package_base(evaluator: &RecognitionEvaluator) -> Option<(u32, u32)> {
    evaluator
        .pack()
        .coordinate_space
        .map(|space| (space.width, space.height))
}

/// `scene` on the base `base` (see the module header): as it is when it is of the base's size, or
/// `base` is `None`, with no `frame_coordinates`; otherwise brought to the base by the live
/// observation's own function, the kernel's `lab_scene_on_base`, with `frame_coordinates` in its
/// base form; a scene that function refuses is refused `lab_scene_size_unsupported`
/// ([`scene_refused`]).
pub fn scene_in_base(
    base: Option<(u32, u32)>,
    scene: Scene,
) -> LabResult<(Scene, Option<FrameCoordinates>)> {
    let Some(base) = base else {
        return Ok((scene, None));
    };
    let given = (scene.width(), scene.height());
    match lab_scene_on_base(base, &scene) {
        Ok(None) => Ok((scene, None)),
        Ok(Some(reduced)) => Ok((reduced.scene, Some(FrameCoordinates::observed(base, given)))),
        Err(refusal) => Err(scene_refused(&refusal, given, base)),
    }
}

/// [`scene_in_base`] of every frame of a frame set, oldest first, the last the current frame; the
/// `frame_coordinates` of the current frame.
pub fn frames_in_base(
    base: Option<(u32, u32)>,
    frames: Vec<Scene>,
) -> LabResult<(Vec<Scene>, Option<FrameCoordinates>)> {
    let mut coordinates = None;
    let mut reduced = Vec::with_capacity(frames.len());
    for scene in frames {
        let (scene, stated) = scene_in_base(base, scene)?;
        coordinates = stated;
        reduced.push(scene);
    }
    Ok((reduced, coordinates))
}

/// The PNG of a reduced scene, the frame an answer about it writes or previews.
pub fn scene_png(scene: &Scene) -> LabResult<Vec<u8>> {
    Frame::from_pixels(
        scene.width(),
        scene.height(),
        scene.rgb8_pixels().to_vec(),
        PixelFormat::Rgb8,
        CaptureBackendName::AdbScreencap,
    )
    .and_then(|frame| frame.encode_png_fast())
    .map_err(|error| LabError::device(format!("failed to encode the reduced scene: {error}")))
}

/// `lab_scene_size_unsupported` of a scene of `given` size (width, height) that the kernel's frame
/// gate for a Lab frame refused for the base `base` (`refusal`, `display_map_unresolved` as the live
/// observation answers it): the gate's own word for why, the `display_problem` its refusal carries
/// (a value of that closed vocabulary, whose key bears the vocabulary's name), with the scene's size
/// and the base's. A refusal that carries no such word (a frame the gate could not read at all) is
/// no size of a scene: it is relayed as the device error it is, with its code in the text.
fn scene_refused(refusal: &ContainedTaskError, given: (u32, u32), base: (u32, u32)) -> LabError {
    let carried = refusal.outcome();
    let problem = carried
        .value(DisplayProblem::VOCABULARY)
        .and_then(serde_json::Value::as_str)
        .and_then(|word| {
            DisplayProblem::ALL
                .iter()
                .copied()
                .find(|problem| problem.as_str() == word)
        });
    let Some(problem) = problem else {
        return LabError::device(format!(
            "the {}x{} scene could not be brought to its package's base {}x{}: {}; nothing was \
             evaluated",
            given.0,
            given.1,
            base.0,
            base.1,
            refusal.code()
        ));
    };
    let outcome = Outcome::new(LabCode::LabSceneSizeUnsupported)
        .display_problem(problem)
        .frame_width(given.0)
        .frame_height(given.1)
        .base_width(base.0)
        .base_height(base.1);
    LabError::new(
        LabErrorClass::UsageValidation,
        LabCode::LabSceneSizeUnsupported.as_str(),
        format!(
            "the scene is {}x{} and its package's coordinate_space {}x{}: a scene of that size, \
             or one the Lab's live observation would bring to it (its ratio, not smaller, its \
             orientation), is evaluated; nothing was evaluated",
            given.0, given.1, base.0, base.1
        ),
        &[],
    )
    .with_details(json!({
        "scene": {"width": given.0, "height": given.1},
        "coordinate_space": {"width": base.0, "height": base.1},
        "outcome": outcome.envelope(Detail::Full),
    }))
}

/// Workflow #392 DA-5cd fix round 1: the base size `STANDARD_PACK_BASE_SIZE` as a recording's
/// header names it (`LabRecording::base_size`), the size the installed `record start` writes.
pub const RECORDING_BASE_SIZE: RecordSize = RecordSize {
    width: STANDARD_PACK_BASE_SIZE.width,
    height: STANDARD_PACK_BASE_SIZE.height,
};

/// The base size as the display adapter takes it.
fn base_extent() -> Option<CaptureExtent> {
    CaptureExtent::new(
        STANDARD_PACK_BASE_SIZE.width,
        STANDARD_PACK_BASE_SIZE.height,
    )
}

/// Whether a display of `width` x `height` is the base size.
pub fn is_base_size(width: u32, height: u32) -> bool {
    (width, height)
        == (
            STANDARD_PACK_BASE_SIZE.width,
            STANDARD_PACK_BASE_SIZE.height,
        )
}

/// Whether frames of `width` x `height` show a larger display of the base size's ratio, a pure
/// scale the display adapter maps onto the base size (1920x1080, 3840x2160): what a recording made
/// on a display that is not the base size holds. A size of another ratio or smaller than the base
/// is no such display.
pub(crate) fn larger_display_of_base_ratio(width: u32, height: u32) -> bool {
    let (Some(base), Some(given)) = (base_extent(), CaptureExtent::new(width, height)) else {
        return false;
    };
    if given == base {
        return false;
    }
    let observation = DisplayObservation {
        frame: given,
        display: given,
        rotation: None,
        crop: None,
        insets: None,
    };
    DisplayMap::build(base, observation, None).is_ok_and(|map| map.kind() == DisplayMapKind::Scale)
}

/// `record_display_not_base` (this instance's display is not the base size; nothing was recorded)
/// for a display of `display`'s size and the base size `base`, `message` saying what was refused:
/// raised by `record start` from the display the Runtime last observed, and (fix round 1) by the
/// first frame of a recording whose header names `base`. Its details name the display and the base
/// size, each `{width, height}`, and carry the outcome (values `frame_width`, `frame_height`,
/// `base_width`, `base_height`).
pub fn record_display_not_base(
    display: RecordSize,
    base: RecordSize,
    message: impl Into<String>,
) -> LabError {
    not_base(
        LabCode::RecordDisplayNotBase,
        ("display", "display_size"),
        display,
        base,
        message.into(),
    )
}

/// Workflow #392 DA-5cd fix round 1: `record_size_not_base` (this recording holds frames that are
/// not the base size) for a recording whose frames are `frames`' size, judged against the base size
/// `base`, `message` saying what was refused. Its details name the frames' size (`frames`) and the
/// base size (`base`), each `{width, height}`, and carry the outcome, with the values of
/// [`record_display_not_base`].
fn record_size_not_base(frames: RecordSize, base: RecordSize, message: String) -> LabError {
    not_base(
        LabCode::RecordSizeNotBase,
        ("frames", "recording_size"),
        frames,
        base,
        message,
    )
}

/// The refusal `refusal` of a size `given` that is not the base size `base`: `member` names the
/// details' member of `given`, `blocked_by` what it is blocked by.
fn not_base(
    refusal: LabCode,
    (member, blocked_by): (&str, &str),
    given: RecordSize,
    base: RecordSize,
    message: String,
) -> LabError {
    LabError::safety_blocked(refusal.as_str(), message, &[blocked_by]).with_details(json!({
        member: {"width": given.width, "height": given.height},
        "base": {"width": base.width, "height": base.height},
        "outcome": Outcome::new(refusal)
            .frame_width(given.width)
            .frame_height(given.height)
            .base_width(base.width)
            .base_height(base.height)
            .envelope(Detail::Full),
    }))
}

/// Workflow #392 DA-5cd fix round 1 (`contracts/lab-recording.md`, "A recording's size"): the
/// refusal `record_size_not_base` of a recording whose frames are not the base size: with a base
/// size in its header (`LabRecording::base_size`, written by `record start`), frames of any other
/// size; without one (a recording started before fix round 1, and the tests' fixtures), frames that
/// show a larger display of the base size's ratio ([`larger_display_of_base_ratio`], the judgement
/// of the slice's first round), judged against `STANDARD_PACK_BASE_SIZE`. `None` for a recording
/// without a frame and for every other. `record stop` refuses it, `record ready` lists it, and a
/// frame is not added to it.
pub(crate) fn recording_not_base(recording: &LabRecording) -> Option<LabError> {
    let size = recording.coordinate_space?;
    let refused = match recording.base_size {
        Some(base) => size != base,
        None => larger_display_of_base_ratio(size.width, size.height),
    };
    refused.then(|| {
        let base = recording.base_size.unwrap_or(RECORDING_BASE_SIZE);
        record_size_not_base(
            size,
            base,
            format!(
                "the recording {} holds frames of {}x{}, not the base size {}x{}: it makes no \
                 package, nothing of it is converted and no frame is added to it; record status, \
                 record frames and record archive read it. Archive it with record archive and \
                 record the task again on an instance whose display is {}x{}",
                recording.record_id,
                size.width,
                size.height,
                base.width,
                base.height,
                base.width,
                base.height
            ),
        )
    })
}

/// Workflow #392 DA-5cd fix round 1 (`contracts/lab-recording.md`, "A recording's size"): whether
/// a frame of `given` size enters `recording`, checked before anything of it is kept. Its first
/// frame (the recording has no size yet) of another size than the base size its header names is
/// refused `record_display_not_base`; every frame of a recording whose frames are not the base size
/// ([`recording_not_base`]) is refused `record_size_not_base`. A first frame of a recording whose
/// header names no base size enters as before, and the recording takes its size.
pub(crate) fn admit_recording_frame(recording: &LabRecording, given: RecordSize) -> LabResult<()> {
    match (recording.coordinate_space, recording.base_size) {
        (None, Some(base)) if given != base => Err(record_display_not_base(
            given,
            base,
            format!(
                "the first frame of the recording {} is {}x{} and the recording is made at the \
                 base size {}x{} its start wrote: the display of {} is not the base size. Nothing \
                 was recorded and the recording is as it was; record on an instance whose display \
                 is {}x{} (set this one's display to it, or use another), then take the frame again",
                recording.record_id,
                given.width,
                given.height,
                base.width,
                base.height,
                recording.instance,
                base.width,
                base.height
            ),
        )),
        (None, _) => Ok(()),
        (Some(_), _) => match recording_not_base(recording) {
            Some(refusal) => Err(refusal),
            None => Ok(()),
        },
    }
}
