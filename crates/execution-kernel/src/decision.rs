// SPDX-License-Identifier: AGPL-3.0-only

//! Canonical first-effect resolution shared by every admitted-package adapter.

use actingcommand_contract::InputAction;
use actingcommand_pack_containment::{
    AdmittedAction, AdmittedOperation, AdmittedPackage, PackageResolution, TargetTapMode,
};
use actingcommand_recognition_pack::{TargetEvaluation, TargetKind};
use serde::Serialize;
use std::error::Error;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectDecisionError {
    code: &'static str,
    detail: Option<String>,
}

impl EffectDecisionError {
    fn new(code: &'static str) -> Self {
        Self { code, detail: None }
    }

    fn with_detail(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: Some(detail.into()),
        }
    }

    pub const fn code(&self) -> &'static str {
        self.code
    }

    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }
}

impl fmt::Display for EffectDecisionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code)?;
        if let Some(detail) = &self.detail {
            write!(formatter, ": {detail}")?;
        }
        Ok(())
    }
}

impl Error for EffectDecisionError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CanonicalEffectRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CanonicalEffectPoint {
    pub seed: u64,
    pub algorithm: &'static str,
    pub rect: CanonicalEffectRect,
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CanonicalEffectIntent {
    Tap {
        point: CanonicalEffectPoint,
    },
    LongTap {
        point: CanonicalEffectPoint,
        duration_ms: u64,
    },
    Swipe {
        from: CanonicalEffectPoint,
        to: CanonicalEffectPoint,
        duration_ms: u64,
    },
}

impl CanonicalEffectIntent {
    pub fn input_action(&self) -> InputAction {
        match self {
            Self::Tap { point } => InputAction::Tap {
                x: point.x,
                y: point.y,
            },
            Self::LongTap { point, duration_ms } => InputAction::LongTap {
                x: point.x,
                y: point.y,
                duration_ms: *duration_ms,
            },
            Self::Swipe {
                from,
                to,
                duration_ms,
            } => InputAction::Swipe {
                x1: from.x,
                y1: from.y,
                x2: to.x,
                y2: to.y,
                duration_ms: *duration_ms,
            },
        }
    }
}

/// Resolves the one canonical typed first-effect intent for an admitted operation.
///
/// The deterministic target seed is derived only from canonical package identity and the
/// canonical operation key. Paths, process state, adapters, and device backends cannot affect it.
pub fn resolve_admitted_effect_intent(
    package: &AdmittedPackage,
    operation: &AdmittedOperation,
    target: Option<&TargetEvaluation>,
) -> Result<CanonicalEffectIntent, EffectDecisionError> {
    let resolution = package.control().resolution();
    let intent = match operation.action() {
        AdmittedAction::Tap { rect, point } => CanonicalEffectIntent::Tap {
            point: CanonicalEffectPoint {
                seed: 0,
                algorithm: "explicit_point_v1",
                rect: CanonicalEffectRect {
                    x: rect.x(),
                    y: rect.y(),
                    width: rect.width(),
                    height: rect.height(),
                },
                x: point.x(),
                y: point.y(),
            },
        },
        AdmittedAction::LongTap { point, duration } => CanonicalEffectIntent::LongTap {
            point: CanonicalEffectPoint {
                seed: 0,
                algorithm: "explicit_point_v1",
                rect: CanonicalEffectRect {
                    x: point.x(),
                    y: point.y(),
                    width: 1,
                    height: 1,
                },
                x: point.x(),
                y: point.y(),
            },
            duration_ms: duration.milliseconds(),
        },
        AdmittedAction::Drag {
            from_rect,
            to_rect,
            from,
            to,
            duration,
        } => CanonicalEffectIntent::Swipe {
            from: CanonicalEffectPoint {
                seed: 0,
                algorithm: "explicit_point_v1",
                rect: CanonicalEffectRect {
                    x: from_rect.x(),
                    y: from_rect.y(),
                    width: from_rect.width(),
                    height: from_rect.height(),
                },
                x: from.x(),
                y: from.y(),
            },
            to: CanonicalEffectPoint {
                seed: 0,
                algorithm: "explicit_point_v1",
                rect: CanonicalEffectRect {
                    x: to_rect.x(),
                    y: to_rect.y(),
                    width: to_rect.width(),
                    height: to_rect.height(),
                },
                x: to.x(),
                y: to.y(),
            },
            duration_ms: duration.milliseconds(),
        },
        AdmittedAction::TargetTap {
            target: target_key,
            mode,
            offset,
        } => {
            let target = target
                .ok_or_else(|| EffectDecisionError::new("contained_task_guard_target_missing"))?;
            if target.id != target_key.as_str()
                || target.kind != TargetKind::Template
                || !target.passed
            {
                return Err(EffectDecisionError::with_detail(
                    "contained_task_guard_target_invalid",
                    format!(
                        "target '{}' is not the admitted passing template target '{target_key}'",
                        target.id
                    ),
                ));
            }
            let template = target
                .template
                .ok_or_else(|| EffectDecisionError::new("contained_task_guard_target_invalid"))?;
            let rect = match offset {
                Some(offset) => CanonicalEffectRect {
                    x: template.x.checked_add(offset.x()).ok_or_else(|| {
                        EffectDecisionError::new("contained_task_input_out_of_bounds")
                    })?,
                    y: template.y.checked_add(offset.y()).ok_or_else(|| {
                        EffectDecisionError::new("contained_task_input_out_of_bounds")
                    })?,
                    width: offset.width(),
                    height: offset.height(),
                },
                None => CanonicalEffectRect {
                    x: template.x,
                    y: template.y,
                    width: template.width,
                    height: template.height,
                },
            };
            validate_rect(resolution, rect)?;
            let seed = stable_seed(
                package.semantic_fingerprint(),
                operation.key().task().as_str(),
                operation.key().operation(),
            );
            let point = match mode {
                TargetTapMode::Deterministic => deterministic_point(rect, seed),
                TargetTapMode::Center => CanonicalEffectPoint {
                    seed,
                    algorithm: "center_point_v1",
                    rect,
                    x: rect.x + rect.width / 2,
                    y: rect.y + rect.height / 2,
                },
            };
            CanonicalEffectIntent::Tap { point }
        }
    };
    let action = intent.input_action();
    action.validate().map_err(|error| {
        EffectDecisionError::with_detail("contained_task_operation_invalid", error.code())
    })?;
    validate_action(resolution, &action)?;
    Ok(intent)
}

fn stable_seed(semantic_fingerprint: &str, task_id: &str, operation_id: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for segment in [semantic_fingerprint, task_id, operation_id] {
        for byte in (segment.len() as u64)
            .to_be_bytes()
            .into_iter()
            .chain(segment.bytes())
        {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
    }
    if hash == 0 {
        0x9e37_79b9_7f4a_7c15
    } else {
        hash
    }
}

fn deterministic_point(rect: CanonicalEffectRect, seed: u64) -> CanonicalEffectPoint {
    let mut state = seed;
    let x_offset = next_u64(&mut state) % rect.width as u64;
    let y_offset = next_u64(&mut state) % rect.height as u64;
    CanonicalEffectPoint {
        seed,
        algorithm: "xorshift64_uniform_rect_v1",
        rect,
        x: rect.x + x_offset as i32,
        y: rect.y + y_offset as i32,
    }
}

fn next_u64(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

fn validate_rect(
    resolution: PackageResolution,
    rect: CanonicalEffectRect,
) -> Result<(), EffectDecisionError> {
    if rect.width <= 0 || rect.height <= 0 {
        return Err(EffectDecisionError::new(
            "contained_task_input_out_of_bounds",
        ));
    }
    let right = rect
        .x
        .checked_add(rect.width - 1)
        .ok_or_else(|| EffectDecisionError::new("contained_task_input_out_of_bounds"))?;
    let bottom = rect
        .y
        .checked_add(rect.height - 1)
        .ok_or_else(|| EffectDecisionError::new("contained_task_input_out_of_bounds"))?;
    validate_point(resolution, rect.x, rect.y)?;
    validate_point(resolution, right, bottom)
}

fn validate_action(
    resolution: PackageResolution,
    action: &InputAction,
) -> Result<(), EffectDecisionError> {
    match action {
        InputAction::Tap { x, y } | InputAction::LongTap { x, y, .. } => {
            validate_point(resolution, *x, *y)
        }
        InputAction::Swipe { x1, y1, x2, y2, .. } => {
            validate_point(resolution, *x1, *y1)?;
            validate_point(resolution, *x2, *y2)
        }
        _ => Err(EffectDecisionError::new(
            "contained_task_primitive_unsupported",
        )),
    }
}

fn validate_point(
    resolution: PackageResolution,
    x: i32,
    y: i32,
) -> Result<(), EffectDecisionError> {
    if x < 0 || y < 0 || x as u32 >= resolution.width() || y as u32 >= resolution.height() {
        Err(EffectDecisionError::new(
            "contained_task_input_out_of_bounds",
        ))
    } else {
        Ok(())
    }
}
