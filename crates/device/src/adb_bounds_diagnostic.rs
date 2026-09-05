// SPDX-License-Identifier: AGPL-3.0-only

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdbBoundsAction {
    Tap { x: i32, y: i32 },
    Swipe { x1: i32, y1: i32, x2: i32, y2: i32 },
}

impl AdbBoundsAction {
    pub(crate) const fn operation(self) -> &'static str {
        match self {
            Self::Tap { .. } => "tap",
            Self::Swipe { .. } => "swipe",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdbBoundsCoordinate {
    PointX,
    PointY,
    StartX,
    StartY,
    EndX,
    EndY,
}

impl AdbBoundsCoordinate {
    const fn as_str(self) -> &'static str {
        match self {
            Self::PointX => "point_x",
            Self::PointY => "point_y",
            Self::StartX => "start_x",
            Self::StartY => "start_y",
            Self::EndX => "end_x",
            Self::EndY => "end_y",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdbInputConnectGeometry {
    natural_max_x: i32,
    natural_max_y: i32,
    rotation_degrees: u16,
}

impl AdbInputConnectGeometry {
    pub const fn new(natural_max_x: i32, natural_max_y: i32, rotation_degrees: u16) -> Self {
        Self {
            natural_max_x,
            natural_max_y,
            rotation_degrees,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdbInputBoundsContext {
    action: AdbBoundsAction,
    rejected: AdbBoundsCoordinate,
    validation_bounds: (i32, i32),
    connect_geometry: Option<AdbInputConnectGeometry>,
}

impl AdbInputBoundsContext {
    pub const fn new(
        action: AdbBoundsAction,
        rejected: AdbBoundsCoordinate,
        validation_bounds: (i32, i32),
        connect_geometry: Option<AdbInputConnectGeometry>,
    ) -> Self {
        Self {
            action,
            rejected,
            validation_bounds,
            connect_geometry,
        }
    }

    pub(crate) fn render(self) -> String {
        let (x1, y1, x2, y2) = match self.action {
            AdbBoundsAction::Tap { x, y } => (x, y, None, None),
            AdbBoundsAction::Swipe { x1, y1, x2, y2 } => (x1, y1, Some(x2), Some(y2)),
        };
        let point_value = |value: Option<i32>| {
            value.map_or_else(|| "unavailable".to_owned(), |value| value.to_string())
        };
        let (provenance, natural_x, natural_y, rotation) = match self.connect_geometry {
            Some(observed) => (
                "connect_time",
                observed.natural_max_x.to_string(),
                observed.natural_max_y.to_string(),
                observed.rotation_degrees.to_string(),
            ),
            None => (
                "unavailable",
                "unavailable".to_owned(),
                "unavailable".to_owned(),
                "unavailable".to_owned(),
            ),
        };
        format!(
            "operation={} rejected={} supplied_start_x={x1} supplied_start_y={y1} supplied_end_x={} supplied_end_y={} validation_source=coordinate_check validation_max_x={} validation_max_y={} connect_observation={provenance} connect_natural_max_x={natural_x} connect_natural_max_y={natural_y} connect_rotation_degrees={rotation}",
            self.action.operation(),
            self.rejected.as_str(),
            point_value(x2),
            point_value(y2),
            self.validation_bounds.0,
            self.validation_bounds.1,
        )
    }
}
