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
}
