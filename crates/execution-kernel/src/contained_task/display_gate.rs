// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #392 / #401 DA-2a (model 392 v1.2 §0.2 C1 and C10, §3.2, §5.1, §12.1 row DA-2a;
//! `contracts/display-adapter.md` section 8): the display adapter's frame gate. Every frame the
//! kernel recognises goes through `Resolution::validate_frame`, at each of its six entries (the
//! entry page, a navigation locate, a sample, a page frame, a reading's agreement frame and the
//! Lab's dry-run select). The gate builds the frame's display map M from the frame's size and its
//! producer's geometry, the package's base and the game's declaration (the run's, read in its
//! preparation), with the adapter crate:
//!
//! - **the identity** (a frame of the base's size): the frame is its own scale frame and its
//!   buffer is used as it is, so a 1280x720 package on a 1280x720 frame recognises byte for byte
//!   as before;
//! - **a pure scale** (a larger frame of the base's ratio): the frame is resampled once to the
//!   base's size with the REC-5 primitive, and recognition, its regions and its lengths stay in
//!   base pixels;
//! - **anything else** (a translation, a frame cropped from its display, a frame turned upright)
//!   waits for DA-2b: `display_map_unresolved` with `translation_unsupported` (a turned frame:
//!   `orientation_mismatch`), and the run ends with no input;
//! - **a frame the map refuses**: `display_map_unresolved` with the map's own `display_problem`
//!   (a frame whose geometry does not show its display: `contained_task_frame_invalid`).
//!
//! Evidence leaves the adapter in device-frame pixels: a recognised target's region is the
//! device box that covers it (outward rounding). The input side maps nothing yet (DA-3); the
//! host holds input back on a frame that is not the plain identity.

use super::{
    ContainedTaskError, ExecutionKernelCode, Resolution, scene_from_frame, scene_from_pixels,
};
use actingcommand_contract::{
    CaptureExtent, DisplayMapKind, RecognizedTarget, StandardPackDisplayLayout,
};
use actingcommand_device::{Frame, PixelFormat};
use actingcommand_display_adapter::{
    DisplayCode, DisplayLocation, DisplayMap, DisplayMapError, DisplayMapUnresolved,
    DisplayObservation, DisplayProblem, DisplayRule, Rect,
};
use actingcommand_recognition::Scene;
use actingcommand_recognition::resample::resample_pixels;

/// One delivered frame through the gate: its map M and the scale frame recognition sees.
pub(super) struct ScaleFrame<'a> {
    frame: &'a Frame,
    map: DisplayMap,
    /// The frame resampled to the scale frame under a pure scale; `None` under the identity,
    /// where the frame's own buffer is the scale frame.
    resampled: Option<Vec<u8>>,
}

/// The pixels a stability sample is cut from: a delivered frame's, or a scale frame's.
pub(super) struct FramePixels<'a> {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) pixel_format: PixelFormat,
    pub(super) pixels: &'a [u8],
}

impl<'a> From<&'a Frame> for FramePixels<'a> {
    fn from(frame: &'a Frame) -> Self {
        Self {
            width: frame.width,
            height: frame.height,
            pixel_format: frame.pixel_format,
            pixels: frame.pixels.as_slice(),
        }
    }
}

impl<'a> From<&'a ScaleFrame<'_>> for FramePixels<'a> {
    fn from(scale: &'a ScaleFrame<'_>) -> Self {
        scale.pixels()
    }
}

impl Resolution {
    /// The gate (see the module header): the scale frame of `frame` under the game's declaration
    /// `layout` (`None` without one), or the frame's refusal.
    pub(super) fn validate_frame<'a>(
        &self,
        frame: &'a Frame,
        layout: Option<&StandardPackDisplayLayout>,
    ) -> Result<ScaleFrame<'a>, ContainedTaskError> {
        let base = CaptureExtent::new(self.width, self.height).ok_or_else(|| {
            ContainedTaskError::new(ExecutionKernelCode::ContainedTaskResolutionInvalid.code())
        })?;
        let delivered = CaptureExtent::new(frame.width, frame.height).ok_or_else(frame_invalid)?;
        let map = DisplayMap::build(
            base,
            DisplayObservation::of_frame(delivered, &frame.geometry),
            layout.map(DisplayRule::from),
        )
        .map_err(|error| refusal(error, delivered, base))?;
        let waits = |problem| {
            refused(DisplayMapUnresolved {
                problem,
                stage: DisplayLocation::MapBuild,
                frame: delivered,
                base,
            })
        };
        if map.turn().is_some() {
            return Err(waits(DisplayProblem::OrientationMismatch));
        }
        if map.kind() == DisplayMapKind::Translate || map.crop().is_some() {
            return Err(waits(DisplayProblem::TranslationUnsupported));
        }
        let resampled = match map.kind() {
            DisplayMapKind::Identity => None,
            DisplayMapKind::Scale | DisplayMapKind::Translate => {
                let target = map.scale_frame_extent();
                let channels = match frame.pixel_format {
                    PixelFormat::Rgb8 => 3,
                    PixelFormat::Rgba8 => 4,
                };
                Some(
                    resample_pixels(
                        &frame.pixels,
                        (frame.width, frame.height),
                        channels,
                        (target.width(), target.height()),
                    )
                    .map_err(|_| frame_invalid())?,
                )
            }
        };
        Ok(ScaleFrame {
            frame,
            map,
            resampled,
        })
    }
}

impl ScaleFrame<'_> {
    /// The frame's map M.
    pub(super) fn map(&self) -> &DisplayMap {
        &self.map
    }

    /// The scene recognition runs on: the scale frame's.
    pub(super) fn scene(&self) -> Result<Scene, ContainedTaskError> {
        match &self.resampled {
            None => scene_from_frame(self.frame),
            Some(pixels) => {
                let extent = self.map.scale_frame_extent();
                scene_from_pixels(
                    extent.width(),
                    extent.height(),
                    pixels,
                    self.frame.pixel_format,
                )
            }
        }
    }

    /// The scale frame's pixels: the frame's own buffer under the identity.
    pub(super) fn pixels(&self) -> FramePixels<'_> {
        match &self.resampled {
            None => FramePixels::from(self.frame),
            Some(pixels) => {
                let extent = self.map.scale_frame_extent();
                FramePixels {
                    width: extent.width(),
                    height: extent.height(),
                    pixel_format: self.frame.pixel_format,
                    pixels: pixels.as_slice(),
                }
            }
        }
    }

    /// `targets`, recognised on the scale frame, with their regions in device-frame pixels: each
    /// the device box that covers it (outward). Under the plain identity they are unchanged.
    pub(super) fn device_targets(
        &self,
        targets: Vec<RecognizedTarget>,
    ) -> Result<Vec<RecognizedTarget>, ContainedTaskError> {
        if self.map.is_plain_identity() {
            return Ok(targets);
        }
        targets
            .into_iter()
            .map(|mut target| {
                if let Some(region) = target.region {
                    let device = self.map.device_evidence_box(Rect::new(
                        i64::from(region.x),
                        i64::from(region.y),
                        i64::from(region.width),
                        i64::from(region.height),
                    ));
                    target.region = Some(projection_rect(device).ok_or_else(|| {
                        ContainedTaskError::with_detail(
                            ExecutionKernelCode::ContainedTaskRecognitionTargetRegionInvalid.code(),
                            target.target_id.clone(),
                        )
                    })?);
                }
                Ok(target)
            })
            .collect()
    }
}

/// A device box as a projection rectangle, when it fits one.
fn projection_rect(rect: Rect) -> Option<actingcommand_contract::page_projection::Rect> {
    Some(actingcommand_contract::page_projection::Rect {
        x: u32::try_from(rect.x).ok()?,
        y: u32::try_from(rect.y).ok()?,
        width: u32::try_from(rect.width).ok()?,
        height: u32::try_from(rect.height).ok()?,
    })
}

fn frame_invalid() -> ContainedTaskError {
    ContainedTaskError::new(ExecutionKernelCode::ContainedTaskFrameInvalid.code())
}

/// `display_map_unresolved` with its `display_problem`, `stage` and the frame's and the base's
/// sizes.
fn refused(unresolved: DisplayMapUnresolved) -> ContainedTaskError {
    ContainedTaskError::new(DisplayCode::MapUnresolved.code()).with_values(unresolved.outcome())
}

/// The kernel's code of a map the adapter refused. The adapter's three refusals that the host
/// reports under its own codes reach the kernel only from a runtime that did not report them
/// first: a safe area without insets and an unknown rotation of a disagreeing frame are
/// `display_map_unresolved` (`safe_area_unavailable`, `orientation_mismatch`), and a frame that
/// does not show its display is `contained_task_frame_invalid`.
fn refusal(
    error: DisplayMapError,
    frame: CaptureExtent,
    base: CaptureExtent,
) -> ContainedTaskError {
    let problem = match error {
        DisplayMapError::Unresolved(unresolved) => return refused(unresolved),
        DisplayMapError::SafeAreaUnavailable => DisplayProblem::SafeAreaUnavailable,
        DisplayMapError::RotationUnknown => DisplayProblem::OrientationMismatch,
        DisplayMapError::FrameDisplayMismatch => return frame_invalid(),
    };
    refused(DisplayMapUnresolved {
        problem,
        stage: DisplayLocation::MapBuild,
        frame,
        base,
    })
}

/// The identity map of a `width` x `height` package on a frame of its size, for observations
/// a test builds by hand.
#[cfg(test)]
pub(super) fn identity_for_test(width: u32, height: u32) -> DisplayMap {
    let extent = CaptureExtent::new(width, height).expect("a test frame's extent");
    DisplayMap::build(
        extent,
        DisplayObservation {
            frame: extent,
            display: extent,
            rotation: None,
            crop: None,
            insets: None,
        },
        None,
    )
    .expect("the identity")
}

#[cfg(test)]
mod tests {
    //! Workflow #392 DA-2a, row DA2-B of model 392 v1.2 §13: the time a 1080x2340 frame takes to
    //! resample to its 720x1560 scale frame, measured and printed only: no gate, no bound.

    use super::*;
    use actingcommand_device::CaptureBackendName;
    use std::time::Instant;

    #[test]
    fn da2_b_resampling_a_1080x2340_frame_to_720x1560_is_measured() {
        let (width, height) = (1080_u32, 2340_u32);
        let pixels = (0..width * height)
            .flat_map(|index| {
                let value = (index % 251) as u8;
                [value, value.wrapping_mul(3), value.wrapping_add(7), 255]
            })
            .collect::<Vec<_>>();
        let frame = Frame::from_pixels(
            width,
            height,
            pixels,
            PixelFormat::Rgba8,
            CaptureBackendName::FixtureSimulation,
        )
        .expect("a 1080x2340 frame");
        let package = Resolution {
            width: 720,
            height: 1560,
        };
        let started = Instant::now();
        let scale = package
            .validate_frame(&frame, None)
            .expect("a pure scale of the package");
        let elapsed = started.elapsed();
        let pixels = scale.pixels();
        assert_eq!((pixels.width, pixels.height), (720, 1560));
        assert_eq!(pixels.pixels.len(), 720 * 1560 * 4);
        eprintln!("DA2-B: 1080x2340 RGBA to 720x1560 in {elapsed:?}");
    }
}
