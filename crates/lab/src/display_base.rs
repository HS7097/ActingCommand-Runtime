// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #392 DA-5cd (`contracts/display-adapter.md` section 11; `contracts/lab-recording.md`,
//! "A recording's size"): what the Lab does with a scene or a recording that is not its package's
//! size.
//!
//! - **Offline recognition.** A scene of its package's `coordinate_space` is evaluated as it is,
//!   byte for byte as before. A scene that is a pure scale of it, at a ratio the display adapter
//!   admits for a run, is reduced to it by the two calls a contained run's frame gate makes (the
//!   adapter's map, `DisplayMap::build` with no display declaration and checked by
//!   `DisplayMap::admit_input`, then one `resample_pixels` to the map's scale frame) and evaluated
//!   there; the answer names the size it was reduced from (`scaled_from`) and every rectangle and
//!   point in it is in the coordinate space. Any other scene is refused with
//!   `lab_scene_size_unsupported`, the adapter's word for why in `display_problem`.
//! - **Recordings.** A recording is made at the base size (`STANDARD_PACK_BASE_SIZE`): the refusal
//!   `record_display_not_base` (this instance's display is not the base size) of `record start` on
//!   a display the Runtime last observed at another size and (fix round 1) of the first frame of a
//!   recording whose header names the base size, a frame of another size; and `record_size_not_base`
//!   (this recording holds frames that are not the base size) of `record stop`, `record ready`'s
//!   item and a frame added to such a recording.

use crate::SceneExtentResponse;
use crate::codes::LabCode;
use crate::recording::model::{LabRecording, RecordSize};
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_contract::{
    CaptureExtent, DisplayMapKind, InputAction, LabError, LabErrorClass, LabResult,
    STANDARD_PACK_BASE_SIZE,
};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_display_adapter::{
    DisplayMap, DisplayMapError, DisplayObservation, DisplayProblem,
};
use actingcommand_recognition::Scene;
use actingcommand_recognition::resample::resample_pixels;
use actingcommand_recognition_pack::RecognitionEvaluator;
use serde_json::json;

/// The coordinate space `evaluator`'s package declares, the base a scene is reduced to; `None` when
/// it declares none, and then every scene is evaluated as it is.
pub fn package_base(evaluator: &RecognitionEvaluator) -> Option<CaptureExtent> {
    evaluator
        .pack()
        .coordinate_space
        .and_then(|space| CaptureExtent::new(space.width, space.height))
}

/// `scene` in the coordinate space `base`: as it is when it is of that size (or `base` is `None`),
/// reduced to it when it is a pure scale of it that a run admits, with the size it was reduced
/// from; otherwise refused `lab_scene_size_unsupported` (see the module header).
pub fn scene_in_base(
    base: Option<CaptureExtent>,
    scene: Scene,
) -> LabResult<(Scene, Option<SceneExtentResponse>)> {
    let Some(base) = base else {
        return Ok((scene, None));
    };
    if (scene.width(), scene.height()) == (base.width(), base.height()) {
        return Ok((scene, None));
    }
    let given = CaptureExtent::new(scene.width(), scene.height())
        .ok_or_else(|| LabError::usage("the scene has no width or no height"))?;
    let map = admitted_map(base, given).map_err(|problem| scene_refused(problem, given, base))?;
    let target = map.scale_frame_extent();
    let reduced = resample_pixels(
        scene.rgb8_pixels(),
        (given.width(), given.height()),
        3,
        (target.width(), target.height()),
    )
    .and_then(|pixels| Scene::from_rgb8(target.width(), target.height(), &pixels))
    .map_err(|error| {
        LabError::device(format!(
            "failed to reduce the {}x{} scene to its package's {}x{}: {error}",
            given.width(),
            given.height(),
            target.width(),
            target.height()
        ))
    })?;
    Ok((
        reduced,
        Some(SceneExtentResponse {
            width: given.width(),
            height: given.height(),
        }),
    ))
}

/// [`scene_in_base`] of every frame of a frame set, oldest first, the last the current frame; the
/// size the current frame was reduced from.
pub fn frames_in_base(
    base: Option<CaptureExtent>,
    frames: Vec<Scene>,
) -> LabResult<(Vec<Scene>, Option<SceneExtentResponse>)> {
    let mut scaled_from = None;
    let mut reduced = Vec::with_capacity(frames.len());
    for scene in frames {
        let (scene, from) = scene_in_base(base, scene)?;
        scaled_from = from;
        reduced.push(scene);
    }
    Ok((reduced, scaled_from))
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

/// The display adapter's map of a frame of `given` size that shows itself (no producer geometry,
/// as a scene file has none) onto `base`, with no display declaration, when a run takes it: the
/// map builds, and the input side sends under it (`DisplayMap::admit_input`, whose answer is the
/// same for every input); otherwise the adapter's word for why.
fn admitted_map(base: CaptureExtent, given: CaptureExtent) -> Result<DisplayMap, DisplayProblem> {
    let observation = DisplayObservation {
        frame: given,
        display: given,
        rotation: None,
        crop: None,
        insets: None,
    };
    let map = DisplayMap::build(base, observation, None).map_err(|error| match error {
        DisplayMapError::Unresolved(unresolved) => unresolved.problem,
        // A frame of the package's other orientation whose rotation is unknown, as the kernel's
        // frame gate reports it.
        DisplayMapError::RotationUnknown => DisplayProblem::OrientationMismatch,
        DisplayMapError::SafeAreaUnavailable => DisplayProblem::SafeAreaUnavailable,
        // Not met here: the frame is its own display, of its own ratio.
        DisplayMapError::FrameDisplayMismatch => DisplayProblem::LayoutUndeclared,
    })?;
    map.admit_input(&InputAction::Reset)
        .map_err(|unresolved| unresolved.problem)?;
    Ok(map)
}

/// `lab_scene_size_unsupported` of a scene of `given` size and a package of `base`, the adapter's
/// word `problem` in its values.
fn scene_refused(problem: DisplayProblem, given: CaptureExtent, base: CaptureExtent) -> LabError {
    let outcome = Outcome::new(LabCode::LabSceneSizeUnsupported)
        .display_problem(problem)
        .frame_width(given.width())
        .frame_height(given.height())
        .base_width(base.width())
        .base_height(base.height());
    LabError::new(
        LabErrorClass::UsageValidation,
        LabCode::LabSceneSizeUnsupported.as_str(),
        format!(
            "the scene is {}x{} and its package's coordinate_space {}x{}: a scene of that size, \
             or a pure scale of it at a ratio the display adapter admits for a run, is evaluated; \
             nothing was evaluated",
            given.width(),
            given.height(),
            base.width(),
            base.height()
        ),
        &[],
    )
    .with_details(json!({
        "scene": {"width": given.width(), "height": given.height()},
        "coordinate_space": {"width": base.width(), "height": base.height()},
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
