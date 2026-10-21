// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::codes::{RuntimeContractCode, SharedCode};
use crate::page_projection::{Geometry, PageProjection, Point, Rect};

pub const LAB_OPERATION_PREPARED_SCHEMA: &str = "actingcommand.runtime.lab-operation-prepared.v1";
pub const LAB_OPERATION_TERMINAL_SCHEMA: &str = "actingcommand.runtime.lab-operation-terminal.v1";
pub const MAX_LAB_ARRIVAL_FRAMES: usize = 20;
/// #411 A18 option ④: arrival samples start this far apart, start to start; the wait after a
/// sample is what is left of it after the sample's capture and evaluation, bounded by the
/// deadline.
pub const LAB_ARRIVAL_INTERVAL_MS: u64 = 500;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LabArrivalCondition {
    pub page_id: String,
    pub timeout_ms: u64,
}

impl LabArrivalCondition {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.page_id.is_empty()
            || self.page_id.len() > 2048
            || self.page_id.contains('\0')
            || !self.page_id.contains('/')
            || !(100..=10_000).contains(&self.timeout_ms)
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidLabArrivalCondition,
            ));
        }
        Ok(())
    }
}

crate::outcome_vocabulary! {
    /// What kind of id a Lab operation's target is (the vocabulary `lab_target_type`, #411 A37).
    pub enum LabTargetType: "lab_target_type" {
        /// The element id of an observation: the complete `elements[].id`.
        ElementId => "element_id",
        /// The id of a recognition target in the package's recognition pack.
        TargetId => "target_id",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum LabOperationSelection {
    Element { id: String },
    Coordinates { action: InputAction },
}

impl LabOperationSelection {
    /// #411 Lab L3: explicit coordinates are a tap, a long tap or a swipe, the gestures the kernel
    /// presses for a pack's tap, long press and drag.
    pub fn validate(&self) -> RuntimeContractResult<()> {
        match self {
            Self::Element { id } if !id.is_empty() && id.len() <= 2048 && !id.contains('\0') => {
                Ok(())
            }
            Self::Coordinates {
                action:
                    action @ (InputAction::Tap { .. }
                    | InputAction::LongTap { .. }
                    | InputAction::Swipe { .. }),
            } => action.validate(),
            _ => Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidLabOperationSelection,
            )),
        }
    }

    /// Resolve only the supplied fresh projection; caller hints never enter resolution.
    pub fn resolve(
        &self,
        projection: &PageProjection,
    ) -> RuntimeContractResult<(Option<serde_json::Value>, Geometry, InputAction)> {
        self.validate()?;
        let invalid = || RuntimeContractError::new(RuntimeContractCode::LabElementUnavailable);
        let (element, geometry) = match self {
            Self::Element { id } => {
                let mut matches = projection
                    .elements
                    .iter()
                    .filter(|element| element["id"].as_str() == Some(id));
                let element = matches.next().ok_or_else(invalid)?;
                if matches.next().is_some() || !projection.matched || element["actionable"] != true
                {
                    return Err(invalid());
                }
                let geometry =
                    serde_json::from_value(element["input"].clone()).map_err(|_| invalid())?;
                (Some(element.clone()), geometry)
            }
            Self::Coordinates { action } => {
                let point = |x: i32, y: i32| -> RuntimeContractResult<(Rect, Point)> {
                    let x = u32::try_from(x).map_err(|_| {
                        RuntimeContractError::new(RuntimeContractCode::LabCoordinatesOutOfFrame)
                    })?;
                    let y = u32::try_from(y).map_err(|_| {
                        RuntimeContractError::new(RuntimeContractCode::LabCoordinatesOutOfFrame)
                    })?;
                    Ok((
                        Rect {
                            x,
                            y,
                            width: 1,
                            height: 1,
                        },
                        Point { x, y },
                    ))
                };
                let geometry = match *action {
                    InputAction::Tap { x, y } => {
                        let (rect, point) = point(x, y)?;
                        Geometry::Tap { rect, point }
                    }
                    // #411 Lab L3: a long tap is the projection's long press, a drag that does
                    // not move: one point in one rectangle, held over the duration.
                    InputAction::LongTap { x, y, duration_ms } => {
                        let (rect, point) = point(x, y)?;
                        Geometry::Drag {
                            from_rect: rect,
                            to_rect: rect,
                            from: point,
                            to: point,
                            duration_ms,
                        }
                    }
                    InputAction::Swipe {
                        x1,
                        y1,
                        x2,
                        y2,
                        duration_ms,
                    } => {
                        let (from_rect, from) = point(x1, y1)?;
                        let (to_rect, to) = point(x2, y2)?;
                        Geometry::Drag {
                            from_rect,
                            to_rect,
                            from,
                            to,
                            duration_ms,
                        }
                    }
                    _ => {
                        return Err(RuntimeContractError::new(
                            RuntimeContractCode::InvalidLabOperationSelection,
                        ));
                    }
                };
                (None, geometry)
            }
        };
        geometry
            .validate(projection.frame.width, projection.frame.height)
            .map_err(|_| {
                RuntimeContractError::new(RuntimeContractCode::LabCoordinatesOutOfFrame)
            })?;
        let coordinate = |value| {
            i32::try_from(value).map_err(|_| {
                RuntimeContractError::new(RuntimeContractCode::LabCoordinatesOutOfFrame)
            })
        };
        // #411 Lab L3: the Runtime presses a selection as the kernel presses a pack's click. A drag
        // that does not move (`from` equal to `to` in the same rectangle, the readers' form of a
        // long press) is a long tap at that point over its duration; every other drag is one
        // swipe. The Runtime's preparation and the evidence verifier both resolve by this rule.
        let action = match geometry {
            Geometry::Tap { point, .. } => InputAction::Tap {
                x: coordinate(point.x)?,
                y: coordinate(point.y)?,
            },
            Geometry::Drag {
                from_rect,
                to_rect,
                from,
                to,
                duration_ms,
            } if from == to && from_rect == to_rect => InputAction::LongTap {
                x: coordinate(from.x)?,
                y: coordinate(from.y)?,
                duration_ms,
            },
            Geometry::Drag {
                from,
                to,
                duration_ms,
                ..
            } => InputAction::Swipe {
                x1: coordinate(from.x)?,
                y1: coordinate(from.y)?,
                x2: coordinate(to.x)?,
                y2: coordinate(to.y)?,
                duration_ms,
            },
        };
        action.validate()?;
        Ok((element, geometry, action))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LabProjectionHint {
    pub sequence: Option<u64>,
    pub content_sha256: Option<String>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContainedLabOperationRequest {
    pub package_path: String,
    pub selection: LabOperationSelection,
    pub projection_hint: LabProjectionHint,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<LabArrivalCondition>,
}

impl fmt::Debug for ContainedLabOperationRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ContainedLabOperationRequest(<contained-resource>)")
    }
}

impl ContainedLabOperationRequest {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        ContainedObservationRequest::new(&self.package_path, Vec::new())?;
        self.selection.validate()?;
        if let Some(after) = &self.after {
            after.validate()?;
        }
        if self.projection_hint.sequence == Some(0) {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidLabProjectionHint,
            ));
        }
        if let Some(hash) = &self.projection_hint.content_sha256 {
            validate_sha256_hex(hash)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LabEvidenceReference {
    pub artifact: ProjectedArtifactReference,
    pub verified: TerminalEvent,
}

impl LabEvidenceReference {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        self.artifact.validate().map_err(|_| {
            RuntimeContractError::new(RuntimeContractCode::InvalidLabEvidenceArtifact)
        })?;
        if self.verified.sequence == 0
            || self.artifact.kind != ArtifactKind::DiagnosticJson
            || self.artifact.byte_count > MAX_OBSERVATION_ARTIFACT_BYTES as u64
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidLabEvidenceReference,
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LabOperationFrame {
    pub observation: ReadonlyObservation,
    pub verified: TerminalEvent,
    pub lease_valid_after_capture: bool,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LabOperationPrepared {
    pub schema_version: String,
    pub request_id: RequestId,
    pub correlation_id: CorrelationId,
    pub instance_id: InstanceId,
    /// Workflow #336 PKG-S2 (option 丙): the admitted package's `package_id`, absent when its
    /// control declares none; no package digest is recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_label: Option<String>,
    pub lease_id: Option<LeaseId>,
    pub selection: LabOperationSelection,
    pub projection_hint: LabProjectionHint,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<LabArrivalCondition>,
    pub before_frame: Option<LabOperationFrame>,
    pub before_projection: Option<ContainedPageObservation>,
    pub selected_element: Option<serde_json::Value>,
    pub geometry: Option<Geometry>,
    pub action: Option<InputAction>,
}

impl fmt::Debug for LabOperationPrepared {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LabOperationPrepared(<controlled-evidence>)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabOperationStage {
    Lease,
    BeforeFrame,
    BeforeProjection,
    Selection,
    Input,
    AfterFrame,
    AfterProjection,
    Arrival,
    Release,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabArrivalStatus {
    Reached,
    TimedOut,
    Aborted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LabArrivalSample {
    pub frame: LabOperationFrame,
    pub projection: Option<LabEvidenceReference>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LabArrivalResult {
    pub status: LabArrivalStatus,
    pub elapsed_ms: u64,
    pub samples: Vec<LabArrivalSample>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LabOperationFailure {
    pub stage: LabOperationStage,
    pub code: String,
    pub error: RuntimeErrorProjection,
    pub event: Option<TerminalEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LabOperationRecord {
    pub schema_version: String,
    pub prepared: LabOperationPrepared,
    pub prepared_artifact: LabEvidenceReference,
    pub input_returned: bool,
    pub input_action_id: Option<ActionId>,
    pub input_intent: Option<TerminalEvent>,
    pub input_event: Option<TerminalEvent>,
    pub effect: EffectDisposition,
    pub after_frame: Option<LabOperationFrame>,
    pub after_projection: Option<ContainedPageObservation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrival: Option<LabArrivalResult>,
    pub failure: Option<LabOperationFailure>,
    pub cleanup_failure: Option<LabOperationFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContainedLabOperationResult {
    pub record: LabOperationRecord,
    pub terminal_artifact: LabEvidenceReference,
}

impl ContainedLabOperationResult {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        let record = &self.record;
        let prepared = &record.prepared;
        prepared.selection.validate()?;
        record.prepared_artifact.validate()?;
        self.terminal_artifact.validate()?;
        if prepared.schema_version != LAB_OPERATION_PREPARED_SCHEMA
            || record.schema_version != LAB_OPERATION_TERMINAL_SCHEMA
            || prepared
                .package_label
                .as_deref()
                .is_some_and(|label| !is_package_label(label))
            || record.prepared_artifact.verified.sequence
                >= self.terminal_artifact.verified.sequence
            || record.input_action_id.is_some() != record.input_intent.is_some()
            || (!record.input_returned
                && (record.input_intent.is_some()
                    || record.input_event.is_some()
                    || record.effect != EffectDisposition::NotPerformed))
            || (record.input_returned
                && (prepared.action.is_none()
                    || (record.input_event.is_none()
                        && record.effect != EffectDisposition::Indeterminate)))
            || record.input_intent.is_some_and(|event| {
                event.sequence <= record.prepared_artifact.verified.sequence
                    || event.sequence >= self.terminal_artifact.verified.sequence
            })
            || record.input_event.is_some_and(|event| {
                record
                    .input_intent
                    .is_none_or(|intent| intent.sequence >= event.sequence)
            })
            || record.input_event.is_some_and(|event| {
                event.sequence <= record.prepared_artifact.verified.sequence
                    || event.sequence >= self.terminal_artifact.verified.sequence
            })
            || record.failure.as_ref().is_some_and(|failure| {
                failure.code.is_empty() || failure.code.len() > 128 || failure.error.fatal
            })
            || record.cleanup_failure.as_ref().is_some_and(|failure| {
                record.failure.is_none()
                    || failure.stage != LabOperationStage::Release
                    || failure.code.is_empty()
                    || failure.code.len() > 128
                    || failure.error.fatal
            })
            || prepared.action.is_some() != prepared.geometry.is_some()
            || (prepared.selected_element.is_some() && prepared.action.is_none())
            || prepared.projection_hint.sequence == Some(0)
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidLabOperationRecord,
            ));
        }
        if let Some(hash) = &prepared.projection_hint.content_sha256 {
            validate_sha256_hex(hash)?;
        }
        for (frame, projection) in [
            (&prepared.before_frame, &prepared.before_projection),
            (&record.after_frame, &record.after_projection),
        ] {
            if let Some(frame) = frame {
                frame.observation.validate()?;
                if frame.verified.sequence == 0 || prepared.lease_id.is_none() {
                    return Err(RuntimeContractError::new(
                        RuntimeContractCode::InvalidLabOperationFrame,
                    ));
                }
            }
            if let Some(projection) = projection {
                projection.validate()?;
                if frame
                    .as_ref()
                    .is_none_or(|frame| projection.frame != frame.observation)
                    || projection.instance_id != prepared.instance_id
                    || projection.package_label != prepared.package_label
                {
                    return Err(RuntimeContractError::new(
                        RuntimeContractCode::InvalidLabOperationProjection,
                    ));
                }
            }
        }
        if let Some(action) = &prepared.action {
            action.validate()?;
            let frame = prepared.before_frame.as_ref().ok_or_else(|| {
                RuntimeContractError::new(RuntimeContractCode::LabActionWithoutFrame)
            })?;
            prepared
                .geometry
                .as_ref()
                .ok_or_else(|| {
                    RuntimeContractError::new(RuntimeContractCode::LabActionWithoutGeometry)
                })?
                .validate(frame.observation.width(), frame.observation.height())
                .map_err(|_| {
                    RuntimeContractError::new(RuntimeContractCode::InvalidLabActionGeometry)
                })?;
        }
        if record.failure.is_none()
            && (record.input_event.is_none()
                || record.effect != EffectDisposition::Performed
                || prepared
                    .before_frame
                    .as_ref()
                    .is_none_or(|frame| !frame.lease_valid_after_capture)
                || record
                    .after_frame
                    .as_ref()
                    .is_none_or(|frame| !frame.lease_valid_after_capture)
                || prepared.before_projection.is_none()
                || record.after_projection.is_none())
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::IncompleteLabOperationSuccess,
            ));
        }
        self.validate_arrival()?;
        Ok(())
    }

    fn validate_arrival(&self) -> RuntimeContractResult<()> {
        let record = &self.record;
        let invalid = || RuntimeContractError::new(RuntimeContractCode::InvalidLabArrivalRecord);
        let (condition, arrival) = match (&record.prepared.after, &record.arrival) {
            (None, None) => return Ok(()),
            (Some(condition), Some(arrival)) => (condition, arrival),
            _ => return Err(invalid()),
        };
        condition.validate()?;
        let last_projection =
            record
                .after_projection
                .as_ref()
                .map(|projection| LabEvidenceReference {
                    artifact: projection.artifact.clone(),
                    verified: TerminalEvent {
                        sequence: projection.projection_sequence,
                        event_id: projection.projection_event_id,
                    },
                });
        if arrival.samples.len() > MAX_LAB_ARRIVAL_FRAMES
            || arrival.samples.last().map(|sample| &sample.frame) != record.after_frame.as_ref()
            || arrival
                .samples
                .last()
                .and_then(|sample| sample.projection.as_ref())
                != last_projection.as_ref()
        {
            return Err(invalid());
        }
        let mut frames = BTreeSet::new();
        if let Some(before) = &record.prepared.before_frame {
            frames.insert(before.observation.artifact().frame_id);
        }
        let mut previous = record.input_event.map_or(0, |event| event.sequence);
        for (index, sample) in arrival.samples.iter().enumerate() {
            sample.frame.observation.validate()?;
            if previous == 0
                || sample.frame.verified.sequence <= previous
                || !frames.insert(sample.frame.observation.artifact().frame_id)
                || (index + 1 < arrival.samples.len()
                    && (!sample.frame.lease_valid_after_capture || sample.projection.is_none()))
            {
                return Err(invalid());
            }
            previous = sample.frame.verified.sequence;
            if let Some(projection) = &sample.projection {
                projection.validate()?;
                if projection.verified.sequence <= previous
                    || projection.artifact.frame_id != sample.frame.observation.artifact().frame_id
                    || !sample.frame.lease_valid_after_capture
                {
                    return Err(invalid());
                }
                previous = projection.verified.sequence;
            }
            if previous >= self.terminal_artifact.verified.sequence {
                return Err(invalid());
            }
        }
        match arrival.status {
            LabArrivalStatus::Reached => {
                if record.effect != EffectDisposition::Performed
                    || arrival.elapsed_ms >= condition.timeout_ms
                    || record.after_projection.as_ref().is_none_or(|projection| {
                        projection.status != PageObservationStatus::Recognized
                            || !projection.projection.matched
                            || projection.projection.page != condition.page_id
                    })
                    || record
                        .failure
                        .as_ref()
                        .is_some_and(|failure| failure.stage != LabOperationStage::Release)
                {
                    return Err(invalid());
                }
            }
            LabArrivalStatus::TimedOut => {
                if record.effect != EffectDisposition::Performed
                    || (arrival.elapsed_ms < condition.timeout_ms
                        && arrival.samples.len() < MAX_LAB_ARRIVAL_FRAMES)
                    || record.failure.as_ref().is_none_or(|failure| {
                        failure.stage != LabOperationStage::Arrival
                            || failure.code != SharedCode::LabArrivalTimeout.as_str()
                    })
                {
                    return Err(invalid());
                }
            }
            LabArrivalStatus::Aborted if record.failure.is_none() => return Err(invalid()),
            LabArrivalStatus::Aborted => {}
        }
        Ok(())
    }
}
