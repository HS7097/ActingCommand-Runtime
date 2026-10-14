// SPDX-License-Identifier: AGPL-3.0-only

use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;
use std::time::SystemTime;

/// Ordered dimensions in the source's coordinate space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureExtent {
    width: NonZeroU32,
    height: NonZeroU32,
}

impl CaptureExtent {
    pub fn new(width: u32, height: u32) -> Option<Self> {
        Some(Self {
            width: NonZeroU32::new(width)?,
            height: NonZeroU32::new(height)?,
        })
    }

    pub const fn width(self) -> u32 {
        self.width.get()
    }

    pub const fn height(self) -> u32 {
        self.height.get()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureBackendName {
    FixtureSimulation,
    AdbScreencap,
    AdbScreencapEncode,
    AdbScreencapRawGzip,
    DroidcastRaw,
    NemuIpc,
    /// #402 RD-3: the scrcpy continuous stream, selectable on real devices only.
    Scrcpy,
}

impl CaptureBackendName {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FixtureSimulation => "fixture_simulation",
            Self::AdbScreencap => "adb_screencap",
            Self::AdbScreencapEncode => "adb_screencap_encode",
            Self::AdbScreencapRawGzip => "adb_screencap_raw_gzip",
            Self::DroidcastRaw => "droidcast_raw",
            Self::NemuIpc => "nemu_ipc",
            Self::Scrcpy => "scrcpy",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    content = "detail",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum CaptureGeometryObservation {
    Observed(CaptureGeometry),
    Unknown(CaptureGeometryUnknownReason),
    NotApplicable(CaptureGeometryNotApplicable),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureGeometryUnknownReason {
    BackendUnsupported,
    ProducerObservationAbsent,
    ProducerUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureGeometryNotApplicable {
    FixtureSimulation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureGeometry {
    pub backend: CaptureBackendName,
    pub source: CaptureGeometrySource,
    pub logical_display_extent: CaptureExtent,
    pub rotation: CaptureRotationObservation,
    pub sampled_at: SystemTime,
    /// None for a display-only read that did not transform a captured frame.
    pub frame_transform: Option<CaptureFrameTransform>,
    /// Workflow #383 register X6 (model 392 §8.2; first writer DA-4): the display cutout's
    /// insets at this rotation, in device-frame pixels. Absent when they were not read or the
    /// display reports no cutout section.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safe_insets: Option<CaptureInsets>,
}

/// Workflow #383 register X6: how far the safe area lies inside each edge of the device
/// frame, in device-frame pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureInsets {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CaptureGeometrySource {
    /// The original ADB commands select the default display without proving a display ID.
    AdbDefaultDisplay {
        serial: String,
        wm_extent: CaptureExtent,
        wm_size_kind: CaptureWmSizeKind,
    },
    NemuSdkDisplay {
        sdk_instance_id: i32,
        sdk_display_id: i32,
        adb_display_mapping: CaptureAdbDisplayMapping,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureWmSizeKind {
    Physical,
    Override,
    Unlabelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureAdbDisplayMapping {
    Unproven,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum CaptureRotationObservation {
    Observed {
        rotation: CaptureRotation,
        source: CaptureRotationSource,
    },
    NotProvidedBySource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureRotation {
    R0,
    R90,
    R180,
    R270,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureRotationSource {
    DumpsysDisplayOrientation,
    UserRotation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureFrameTransform {
    Identity,
    RotateClockwise90,
    RotateCounterclockwise90,
    FlipVertical,
    /// #402 RD-3 (model #398 R1 §5.3): the frame is exactly this region of the logical display,
    /// pixel for pixel. A stream whose encoder needs aligned sizes crops instead of
    /// downsampling, and records the region here.
    Crop(CaptureFrameCrop),
}

/// #402 RD-3: one region of the logical display, in logical display pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureFrameCrop {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl CaptureGeometry {
    /// Workflow #383 register X6: recorded insets leave a safe area inside the logical
    /// display, which the device frame shows in its own orientation (model 392 §2.2).
    pub fn safe_insets_fit_display(&self) -> bool {
        let display = self.logical_display_extent;
        self.safe_insets.is_none_or(|insets| {
            u64::from(insets.left) + u64::from(insets.right) < u64::from(display.width())
                && u64::from(insets.top) + u64::from(insets.bottom) < u64::from(display.height())
        })
    }

    /// #402 RD-3i (model #398 R1 §5.3): whether a delivered frame of `extent` shows this
    /// geometry's logical display. A recorded crop must name exactly the frame's size and lie
    /// inside the display; every other transform keeps the proportional comparison, so a frame
    /// that differs from the display without a recorded crop is judged as before.
    pub fn frame_matches_display(&self, extent: CaptureExtent) -> bool {
        let display = self.logical_display_extent;
        match self.frame_transform {
            Some(CaptureFrameTransform::Crop(crop)) => {
                crop.width == extent.width()
                    && crop.height == extent.height()
                    && crop
                        .x
                        .checked_add(crop.width)
                        .is_some_and(|right| right <= display.width())
                    && crop
                        .y
                        .checked_add(crop.height)
                        .is_some_and(|bottom| bottom <= display.height())
            }
            _ => {
                u64::from(extent.width()) * u64::from(display.height())
                    == u64::from(extent.height()) * u64::from(display.width())
            }
        }
    }
}
