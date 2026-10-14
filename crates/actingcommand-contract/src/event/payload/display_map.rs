// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #383 register X7 (model 392 §4, §8.1, §8.3; first writer DA-4): the display map a
// task geometry observation used, carried by `TaskGeometryObservation.display_map`. Types,
// reader and validation only; DA-4 writes it.

use crate::SanitizationError;
use crate::codes::SanitizationCode;
use serde::{Deserialize, Serialize};

/// How the scale frame relates to the package's base (model 392 §4.2): the same frame, a pure
/// scale to the base size, or a frame of another size whose regions are moved by anchors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayMapKind {
    Identity,
    Scale,
    Translate,
}

/// The side of the layout area the scale factor is measured on (`scale.by`, model 392 §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayScaleBy {
    Height,
    Width,
}

/// The area a scale factor or the anchors are measured on (`scale.area`, `anchor_area`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayArea {
    Screen,
    SafeArea,
}

/// Where the insets in use came from (model 392 §4.5): the device's report, or a person's or
/// an agent's declaration (`display.safe_area.declared`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayInsetsSource {
    Reported,
    Declared,
}

/// The display map M of one observation: its kind, the scale factor `scale_num / scale_den`
/// (reduced, never below 1), the scale frame's size, the package's base size, the
/// declaration's rule, where the insets came from and whether a declaration exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskGeometryDisplayMap {
    pub kind: DisplayMapKind,
    pub scale_num: u32,
    pub scale_den: u32,
    pub scale_width: u32,
    pub scale_height: u32,
    pub base_width: u32,
    pub base_height: u32,
    pub scale_by: DisplayScaleBy,
    pub scale_area: DisplayArea,
    pub anchor_area: DisplayArea,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub insets_source: Option<DisplayInsetsSource>,
    pub declared: bool,
}

impl TaskGeometryDisplayMap {
    /// The map agrees with itself (model 392 §4.2, §4.5, §11): sizes are pixels; the scale
    /// factor is reduced and at least 1; identity is the factor 1 on the base size, a pure
    /// scale a larger factor on the base size, a translation any other scale frame and only
    /// under a declaration; a map that measures on the safe area names its insets' source.
    pub(super) fn validate(&self) -> Result<(), SanitizationError> {
        let sized = [
            self.scale_num,
            self.scale_den,
            self.scale_width,
            self.scale_height,
            self.base_width,
            self.base_height,
        ]
        .iter()
        .all(|value| *value > 0);
        let base_sized =
            self.scale_width == self.base_width && self.scale_height == self.base_height;
        let kind_agrees = match self.kind {
            DisplayMapKind::Identity => self.scale_num == self.scale_den && base_sized,
            DisplayMapKind::Scale => self.scale_num > self.scale_den && base_sized,
            DisplayMapKind::Translate => !base_sized && self.declared,
        };
        let safe_area =
            self.scale_area == DisplayArea::SafeArea || self.anchor_area == DisplayArea::SafeArea;
        if !sized
            || greatest_common_divisor(self.scale_num, self.scale_den) != 1
            || self.scale_num < self.scale_den
            || !kind_agrees
            || (safe_area && self.insets_source.is_none())
        {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidSanitizedPayload,
                "display_map",
            ));
        }
        Ok(())
    }
}

fn greatest_common_divisor(mut left: u32, mut right: u32) -> u32 {
    while right != 0 {
        (left, right) = (right, left % right);
    }
    left
}
