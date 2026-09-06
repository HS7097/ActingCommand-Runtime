// SPDX-License-Identifier: AGPL-3.0-only

//! One contained, read-only observation over the existing recognition owners.

use crate::{
    DriveSemanticInput, ExternalExpectedSha256, ExternallyVerifiedBundle, drive_rect_center,
};
use actingcommand_contract::page_projection::{
    self as projection, ActionKey, ElementInput, ElementResolution, ElementRole, FieldInput,
    FrameIdentity, Geometry, MissingTarget, NotEvaluatedReason, PageProjection, Privacy,
    ProjectionInput,
};
use actingcommand_contract::{ObservationFacts, PageObservationStatus};
use actingcommand_page_detector::{PageTargetEvaluation, PageTargetRole};
use actingcommand_recognition::Scene;
use actingcommand_recognition_pack::{
    PackRect, RecognitionEvaluator, RecognitionTarget, TargetEvaluation, TargetKind, VisionProvider,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub struct OnlineObservationError {
    code: &'static str,
    stage: &'static str,
    cause: String,
}
impl std::fmt::Debug for OnlineObservationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}
impl OnlineObservationError {
    fn new(code: &'static str, stage: &'static str, cause: impl ToString) -> Self {
        Self {
            code,
            stage,
            cause: cause.to_string(),
        }
    }
    pub fn code(&self) -> &'static str {
        self.code
    }
    pub fn stage(&self) -> &'static str {
        self.stage
    }
    pub fn cause(&self) -> &str {
        &self.cause
    }
}
impl std::fmt::Display for OnlineObservationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} during {}", self.code, self.stage)
    }
}
impl std::error::Error for OnlineObservationError {}

struct ObservationElement {
    action: ActionKey,
    purpose: String,
    source: String,
    input: DriveSemanticInput,
}

pub struct PreparedPageObservation {
    bundle: ExternallyVerifiedBundle,
    elements: Vec<ObservationElement>,
    targets: Vec<String>,
}

pub struct EvaluatedPageObservation {
    pub projection: PageProjection,
    pub status: PageObservationStatus,
    pub rgb8_sha256: String,
    pub facts: ObservationFacts,
    pub private_facts: ObservationFacts,
}

impl PreparedPageObservation {
    pub fn load(
        instance: &str,
        bytes: &[u8],
        expected: ExternalExpectedSha256,
        targets: &[String],
        provider: Option<Arc<dyn VisionProvider>>,
    ) -> Result<Self, OnlineObservationError> {
        let bundle =
            ExternallyVerifiedBundle::load_observation(instance, bytes, expected, provider)
                .map_err(|error| {
                    OnlineObservationError::new(
                        "observation_containment_failed",
                        "admit_contained_observation",
                        error,
                    )
                })?;
        let loaded = bundle.loaded_bundle();
        let invalid = |cause: &str| {
            OnlineObservationError::new(
                "observation_resources_invalid",
                "load_observation_resources",
                cause,
            )
        };
        let metadata = loaded
            .projection_metadata()
            .ok_or_else(|| invalid("verified catalog missing"))?;
        let evaluator = loaded
            .evaluator()
            .ok_or_else(|| invalid("recognition pack missing"))?;
        let detector = loaded.detector().ok_or_else(|| invalid("pages missing"))?;
        detector
            .validate(evaluator)
            .map_err(|error| invalid(error.message()))?;
        for target in targets {
            if evaluator
                .target_kind(target)
                .map_err(|error| invalid(error.message()))?
                == TargetKind::ClickOnly
            {
                return Err(invalid("explicit target is not evaluable"));
            }
        }
        let navigation = loaded
            .navigation()
            .ok_or_else(|| invalid("navigation missing"))?;
        let mut elements = Vec::new();
        for action in &metadata.catalog().actions {
            let (array, id) = match action.role {
                ElementRole::Navigate => ("navigation", "id"),
                ElementRole::PageOp => ("page_operations", "id"),
                ElementRole::ControlPoint => ("control_points", "name"),
            };
            let row = navigation[array]
                .as_array()
                .and_then(|rows| {
                    rows.iter().find(|row| {
                        row[id].as_str() == Some(&action.resource_id)
                            && (action.role != ElementRole::PageOp
                                || row["task_id"].as_str() == Some(&action.task_id))
                            && (action.role == ElementRole::ControlPoint
                                || row[if action.role == ElementRole::Navigate {
                                    "from_page"
                                } else {
                                    "page"
                                }]
                                .as_str()
                                    == action.page.as_deref())
                    })
                })
                .ok_or_else(|| invalid("catalog action source missing"))?;
            let input = if let Some(click) = row.get("click") {
                crate::drive::parse_navigation_input(click)
                    .map_err(|error| invalid(error.message()))?
            } else if action.role == ElementRole::ControlPoint {
                let rect = crate::drive::parse_control_point_rect(row)
                    .map_err(|error| invalid(error.message()))?;
                DriveSemanticInput::Tap {
                    rect,
                    point: drive_rect_center(rect).map_err(|error| invalid(error.message()))?,
                }
            } else {
                return Err(invalid("action input missing"));
            };
            match &input {
                DriveSemanticInput::TargetCenter { target_id } => {
                    if !metadata.catalog().targets.contains(target_id) {
                        return Err(invalid("action target is not declared"));
                    }
                }
                _ => geometry(&input)?
                    .validate(metadata.catalog().width, metadata.catalog().height)
                    .map_err(|_| invalid("declared action geometry invalid"))?,
            }
            elements.push(ObservationElement {
                action: action.clone(),
                purpose: row["purpose"]
                    .as_str()
                    .unwrap_or(&action.resource_id)
                    .to_string(),
                source: row["source"].as_str().unwrap_or(array).to_string(),
                input,
            });
        }
        Ok(Self {
            bundle,
            elements,
            targets: targets.to_vec(),
        })
    }

    pub fn package_sha256(&self) -> String {
        self.bundle.loaded_bundle().verified_hash().to_string()
    }

    pub fn evaluate(
        &self,
        png: &[u8],
        frame: FrameIdentity,
    ) -> Result<EvaluatedPageObservation, OnlineObservationError> {
        let loaded = self.bundle.loaded_bundle();
        let metadata = loaded.projection_metadata().expect("admitted metadata");
        let evaluator = loaded.evaluator().expect("admitted evaluator");
        let detector = loaded.detector().expect("admitted detector");
        let scene = Scene::from_png(png).map_err(|error| {
            OnlineObservationError::new(
                "observation_frame_invalid",
                "decode_observation_frame",
                error,
            )
        })?;
        let (pages, batch_error) = match detector.evaluate_all_outcomes(evaluator, &scene) {
            Ok(pages) => (pages, None),
            Err(error) => (error.completed.clone(), Some(error)),
        };
        let mut complete = batch_error.is_none() && pages.iter().all(|page| page.result.is_ok());
        let mut private_facts = ObservationFacts::default();
        let mut facts = ObservationFacts::default();
        let mut actual: Vec<(Option<String>, PageTargetEvaluation)> = Vec::new();
        let mut matched_pages = Vec::new();
        for page in &pages {
            let values = match &page.result {
                Ok(value) => {
                    if value.matched {
                        matched_pages.push(value.page_id.clone());
                    }
                    &value.target_results
                }
                Err(error) => &error.completed_targets,
            };
            actual.extend(
                values
                    .iter()
                    .cloned()
                    .map(|value| (Some(page.page_id.clone()), value)),
            );
            let mut summary = serde_json::to_value(page).map_err(fact_error)?;
            for branch in ["Ok", "Err"] {
                if let Some(result) = summary["result"][branch].as_object_mut() {
                    result.remove("target_results");
                    result.remove("completed_targets");
                }
            }
            let row = json!({"kind":"page_evaluation", "stage":"evaluate_page", "outcome":summary, "target_evaluation_count":values.len()});
            private_facts.push(row.clone(), 0).map_err(fact_error)?;
            facts
                .push(redact_row(row, metadata), 0)
                .map_err(fact_error)?;
            for (index, value) in values.iter().enumerate() {
                let row = json!({"kind":"target_evaluation", "stage":"evaluate_page_target", "page_id":page.page_id, "page_index":page.index,
                    "evaluation_index":actual.len() - values.len() + index, "target":value});
                private_facts.push(row.clone(), 1).map_err(fact_error)?;
                facts
                    .push(redact_row(row, metadata), 1)
                    .map_err(fact_error)?;
            }
            if let Err(error) = &page.result {
                let definition = &detector.page_definitions()[page.index];
                let mut declared = Vec::new();
                let mut groups = vec![(PageTargetRole::Required, None, &definition.required)];
                groups.extend(
                    definition
                        .any_of
                        .iter()
                        .enumerate()
                        .map(|(index, targets)| (PageTargetRole::AnyOf, Some(index), targets)),
                );
                groups.push((PageTargetRole::Optional, None, &definition.optional));
                groups.push((PageTargetRole::Forbidden, None, &definition.forbidden));
                for (role, group, targets) in groups {
                    for (index, target) in targets.iter().enumerate() {
                        declared.push((role, group, index, target));
                    }
                }
                let attempted = values.len() + usize::from(error.failed_target.is_some());
                for (role, group, index, target) in declared.into_iter().skip(attempted) {
                    let row = json!({"kind":"target_not_evaluated", "stage":"evaluate_page_target", "page_id":page.page_id,
                        "page_index":page.index, "target_id":target, "role":role, "group_index":group, "target_index":index,
                        "state":"not_evaluated", "reason":"earlier_target_failed"});
                    private_facts.push(row.clone(), 0).map_err(fact_error)?;
                    facts.push(row, 0).map_err(fact_error)?;
                }
            }
        }
        if let Some(error) = batch_error {
            let row = json!({"kind":"page_batch_failure", "stage":"evaluate_pages", "cause":error.cause, "unexecuted":error.unexecuted});
            private_facts.push(row.clone(), 0).map_err(fact_error)?;
            facts
                .push(redact_row(row, metadata), 0)
                .map_err(fact_error)?;
        }
        for target in &self.targets {
            let uses = actual
                .iter()
                .enumerate()
                .filter(|(_, (_, value))| value.target_id == *target)
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            if !uses.is_empty() {
                let row = json!({"kind":"explicit_target_uses", "target_id":target, "evaluation_uses":uses});
                private_facts.push(row.clone(), 0).map_err(fact_error)?;
                facts.push(row, 0).map_err(fact_error)?;
                continue;
            }
            // A target which failed in this frame is not called again for projection.
            if pages.iter().any(|page| {
                page.result
                    .as_ref()
                    .err()
                    .and_then(|error| error.failed_target.as_ref())
                    .is_some_and(|failed| failed.target_id == *target)
            }) {
                let row = json!({"kind":"explicit_target_uses_failure", "target_id":target});
                private_facts.push(row.clone(), 0).map_err(fact_error)?;
                facts.push(row, 0).map_err(fact_error)?;
                continue;
            }
            match evaluator.evaluate_target(&scene, target) {
                Ok(value) => {
                    let evaluated = PageTargetEvaluation {
                        target_id: target.clone(),
                        role: PageTargetRole::Optional,
                        passed: value.passed,
                        message: value.message.clone(),
                        group_index: None,
                        target_index: 0,
                        evaluation: value,
                    };
                    let row = json!({"kind":"explicit_target", "stage":"evaluate_requested_target", "target":evaluated});
                    private_facts.push(row.clone(), 1).map_err(fact_error)?;
                    facts
                        .push(redact_row(row, metadata), 1)
                        .map_err(fact_error)?;
                    actual.push((None, evaluated));
                }
                Err(error) => {
                    complete = false;
                    let row = json!({"kind":"target_failure", "stage":"evaluate_requested_target", "target_id":target, "cause":error});
                    private_facts.push(row.clone(), 0).map_err(fact_error)?;
                    facts
                        .push(redact_row(row, metadata), 0)
                        .map_err(fact_error)?;
                }
            }
        }
        let status = if !complete {
            PageObservationStatus::Partial
        } else {
            match matched_pages.len() {
                0 => PageObservationStatus::NoMatch,
                1 => PageObservationStatus::Recognized,
                _ => PageObservationStatus::Conflict,
            }
        };
        let mut elements = Vec::new();
        let selected_page =
            (complete && matched_pages.len() == 1).then(|| matched_pages[0].as_str());
        for element in &self.elements {
            if element
                .action
                .page
                .as_deref()
                .is_some_and(|page| Some(page) != selected_page)
            {
                continue;
            }
            let resolution = match &element.input {
                DriveSemanticInput::TargetCenter { target_id } => {
                    let values = actual
                        .iter()
                        .filter(|(page, value)| {
                            value.target_id == *target_id
                                && (page.as_deref() == selected_page || page.is_none())
                        })
                        .map(|(_, value)| &value.evaluation)
                        .collect::<Vec<_>>();
                    match values.as_slice() {
                        [value] if element.action.role != ElementRole::ControlPoint => {
                            ElementResolution::Target {
                                target_id: target_id.clone(),
                                passed: value.passed,
                                geometry: target_geometry(value, evaluator)?,
                            }
                        }
                        [_, _, ..] => ElementResolution::Ambiguous {
                            target_id: target_id.clone(),
                        },
                        _ => ElementResolution::NotEvaluated {
                            target_id: target_id.clone(),
                            reason: NotEvaluatedReason::Unscoped,
                        },
                    }
                }
                input => ElementResolution::Declared(geometry(input)?),
            };
            elements.push(ElementInput {
                action: element.action.clone(),
                purpose: element.purpose.clone(),
                source: element.source.clone(),
                resolution,
            });
        }
        let mut missing = Vec::new();
        for page in pages
            .iter()
            .filter_map(|page| page.result.as_ref().ok())
            .filter(|page| Some(page.page_id.as_str()) == selected_page)
        {
            for target in &page.target_results {
                missing.push(MissingTarget {
                    passed: target.passed,
                    id: target.target_id.clone(),
                    role: role_name(target.role).to_string(),
                    group_index: target.group_index,
                    group_satisfied: target.group_index.map(|index| {
                        page.target_results.iter().any(|value| {
                            value.role == PageTargetRole::AnyOf
                                && value.group_index == Some(index)
                                && value.passed
                        })
                    }),
                });
            }
        }
        let mut fields = Vec::new();
        for target in &metadata.catalog().targets {
            let values = actual.iter().enumerate().filter(|(_, (_, value))| value.target_id == *target)
                .map(|(index, (page, value))| json!({"evaluation_index":index,"page":page,"group_index":value.group_index,"target_index":value.target_index,"role":value.role,"evaluation":value.evaluation})).collect::<Vec<_>>();
            if values.is_empty() {
                continue;
            }
            let input = FieldInput {
                target_id: target.clone(),
                field: None,
                parsed: false,
                raw_text: None,
                value: Some(json!({"evaluations":values})),
                detail: None,
                privacy: metadata.target_privacy(target),
            };
            fields.push(input.clone());
            for field in metadata
                .catalog()
                .fields
                .iter()
                .filter(|field| field.target_id == *target)
            {
                let mut bound = input.clone();
                bound.field = Some(field.clone());
                bound.privacy = metadata.field_privacy(field);
                fields.push(bound);
            }
        }
        let projection = projection::project_observation(
            ProjectionInput {
                frame,
                matched_pages,
                elements,
                missing,
                fields,
            },
            metadata,
            complete,
        )
        .map_err(|error| {
            OnlineObservationError::new(
                "observation_projection_failed",
                "project_observation",
                error,
            )
        })?;
        Ok(EvaluatedPageObservation {
            projection,
            status,
            rgb8_sha256: format!("{:x}", Sha256::digest(scene.rgb8_pixels())),
            facts,
            private_facts,
        })
    }
}

fn fact_error(error: impl ToString) -> OnlineObservationError {
    OnlineObservationError::new("observation_fact_failed", "encode_observation_fact", error)
}
fn role_name(role: PageTargetRole) -> &'static str {
    match role {
        PageTargetRole::Required => "required",
        PageTargetRole::AnyOf => "any_of",
        PageTargetRole::Optional => "optional",
        PageTargetRole::Forbidden => "forbidden",
    }
}

fn geometry(input: &DriveSemanticInput) -> Result<Geometry, OnlineObservationError> {
    let integer = |value: i32| u32::try_from(value).map_err(|_| fact_error("negative geometry"));
    let rect = |value: PackRect| -> Result<projection::Rect, OnlineObservationError> {
        Ok(projection::Rect {
            x: integer(value.x)?,
            y: integer(value.y)?,
            width: integer(value.width)?,
            height: integer(value.height)?,
        })
    };
    let point = |value: crate::DrivePoint| -> Result<projection::Point, OnlineObservationError> {
        Ok(projection::Point {
            x: integer(value.x)?,
            y: integer(value.y)?,
        })
    };
    match input {
        DriveSemanticInput::Tap {
            rect: value,
            point: position,
        } => Ok(Geometry::Tap {
            rect: rect(*value)?,
            point: point(*position)?,
        }),
        DriveSemanticInput::Drag {
            from_rect,
            to_rect,
            from,
            to,
            duration_ms,
        } => Ok(Geometry::Drag {
            from_rect: rect(*from_rect)?,
            to_rect: rect(*to_rect)?,
            from: point(*from)?,
            to: point(*to)?,
            duration_ms: *duration_ms,
        }),
        DriveSemanticInput::TargetCenter { .. } => Err(fact_error("target geometry unresolved")),
    }
}
fn target_geometry(
    value: &TargetEvaluation,
    evaluator: &RecognitionEvaluator,
) -> Result<Option<Geometry>, OnlineObservationError> {
    if !value.passed {
        return Ok(None);
    }
    let rect = if let Some(template) = value.template {
        PackRect {
            x: template.x,
            y: template.y,
            width: template.width,
            height: template.height,
        }
    } else {
        let declared = evaluator
            .pack()
            .targets
            .iter()
            .find_map(|target| {
                let (id, click) = match target {
                    RecognitionTarget::Template(target) => (&target.id, target.click),
                    RecognitionTarget::Color(target) => (&target.id, target.click),
                    RecognitionTarget::Ocr(target) => (&target.id, target.click),
                    RecognitionTarget::Nn(target) => (&target.id, target.click),
                    RecognitionTarget::ClickOnly(target) => (&target.id, Some(target.click)),
                };
                (id == &value.id).then_some(click)
            })
            .ok_or_else(|| fact_error("actual target identity missing from admitted pack"))?;
        let Some(rect) = declared else {
            return Ok(None);
        };
        rect
    };
    geometry(&DriveSemanticInput::Tap {
        rect,
        point: drive_rect_center(rect).map_err(fact_error)?,
    })
    .map(Some)
}

fn redact_row(mut value: Value, metadata: &projection::VerifiedProjectionMetadata) -> Value {
    fn visit(
        value: &mut Value,
        metadata: &projection::VerifiedProjectionMetadata,
        restricted: bool,
    ) {
        match value {
            Value::Array(values) => {
                for value in values {
                    visit(value, metadata, restricted);
                }
            }
            Value::Object(object) => {
                let local = object
                    .get("target_id")
                    .and_then(Value::as_str)
                    .map(|id| metadata.target_privacy(id) != Some(Privacy::Public));
                let restricted = local.unwrap_or(restricted);
                if restricted {
                    for key in ["message", "evaluation", "text", "value", "detail"] {
                        if object.contains_key(key) {
                            object.insert(key.to_string(), Value::Null);
                        }
                    }
                    if local.is_some() {
                        object.insert("redacted".to_string(), Value::Bool(true));
                    }
                }
                // A page/batch error message can contain nested target details; typed cause stays.
                if object.contains_key("completed_targets") {
                    object.insert("message".to_string(), Value::Null);
                }
                for value in object.values_mut() {
                    visit(value, metadata, restricted);
                }
            }
            _ => {}
        }
    }
    visit(&mut value, metadata, true);
    value
}
