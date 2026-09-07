// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::page_projection::{
    self as shared, ActionKey, ElementInput, ElementResolution, ElementRole, FrameIdentity,
    Geometry, MissingTarget, NotEvaluatedReason, PageProjection, ProjectionInput,
};
use actingcommand_lab::{CurrentPageRequest, ReadonlyRecognitionInput, RecoveryHintResponse};
use actingcommand_page_detector::{PageTargetEvaluation, PageTargetRole};
use sha2::{Digest, Sha256};

pub(super) fn detect(
    resources: std::sync::Arc<actingcommand_lab::ExternallyVerifiedBundle>,
    scene: &Scene,
) -> CliOutcome<(PageDetectionOutcome, Option<RecoveryHintResponse>)> {
    let mut lab = super::super::env_detection::build_readonly_lab()?;
    let response = lab.current_page(CurrentPageRequest {
        input: ReadonlyRecognitionInput {
            resources,
            scene: Some(
                Scene::from_rgb8(scene.width(), scene.height(), scene.rgb8_pixels())
                    .map_err(|error| CliError::device(error.to_string()))?,
            ),
            scene_path: None,
            capture_config: None,
            require_fresh: false,
            fresh_delay: Duration::ZERO,
        },
    })?;
    let evaluations = response
        .evaluations
        .into_iter()
        .map(|page| {
            let targets = page
                .targets
                .into_iter()
                .map(|target| {
                    let role = match target.role.as_str() {
                        "Required" => PageTargetRole::Required,
                        "AnyOf" => PageTargetRole::AnyOf,
                        "Optional" => PageTargetRole::Optional,
                        "Forbidden" => PageTargetRole::Forbidden,
                        other => {
                            return Err(CliError::device(format!(
                                "unknown readonly target role: {other}"
                            )));
                        }
                    };
                    Ok(PageTargetEvaluation {
                        target_id: target.id,
                        role,
                        passed: target.passed,
                        message: target.message,
                        group_index: target.actual.group_index,
                        target_index: target.actual.target_index,
                        evaluation: target.actual.evaluation,
                    })
                })
                .collect::<CliOutcome<Vec<_>>>()?;
            let count = |role: PageTargetRole, passed_only: bool| {
                targets
                    .iter()
                    .filter(|target| target.role == role && (!passed_only || target.passed))
                    .count()
            };
            Ok(PageEvaluation {
                page_id: page.page,
                matched: page.matched,
                message: page.message,
                required_total: count(PageTargetRole::Required, false),
                required_passed: count(PageTargetRole::Required, true),
                optional_total: count(PageTargetRole::Optional, false),
                optional_passed: count(PageTargetRole::Optional, true),
                forbidden_total: count(PageTargetRole::Forbidden, false),
                forbidden_passed: count(PageTargetRole::Forbidden, true),
                any_of_total: page.any_of_total,
                any_of_passed: page.any_of_passed,
                target_results: targets,
            })
        })
        .collect::<CliOutcome<Vec<_>>>()?;
    Ok((
        PageDetectionOutcome {
            page: response.page,
            matched: response.matched,
            standby: response.standby,
            evaluations,
        },
        response.recovery_hint,
    ))
}

pub(super) fn build(
    view: &super::super::contained_resources::ObservationResources,
    evaluator: &RecognitionEvaluator,
    scene: &Scene,
    outcome: &PageDetectionOutcome,
) -> CliOutcome<PageProjection> {
    let mut elements = Vec::new();
    let mut resolved = std::collections::BTreeMap::new();
    if outcome.matched {
        for edge in view
            .edges
            .iter()
            .filter(|edge| edge.from_page == outcome.page)
        {
            elements.push(ElementInput {
                action: ActionKey {
                    role: ElementRole::Navigate,
                    task_id: String::new(),
                    resource_id: edge.id.clone(),
                    page: Some(edge.from_page.clone()),
                },
                purpose: edge.id.clone(),
                source: edge
                    .source
                    .clone()
                    .unwrap_or_else(|| "navigation".to_string()),
                resolution: resolve(&edge.input, evaluator, scene, &mut resolved)?,
            });
        }
        for operation in view
            .operations
            .iter()
            .filter(|operation| operation.page == outcome.page)
        {
            elements.push(ElementInput {
                action: ActionKey {
                    role: ElementRole::PageOp,
                    task_id: operation.task_id.clone(),
                    resource_id: operation.id.clone(),
                    page: Some(operation.page.clone()),
                },
                purpose: operation.purpose.clone(),
                source: "page_operations".to_string(),
                resolution: resolve(&operation.input, evaluator, scene, &mut resolved)?,
            });
        }
    }
    for control in &view.controls {
        elements.push(ElementInput {
            action: ActionKey {
                role: ElementRole::ControlPoint,
                task_id: String::new(),
                resource_id: control.name.clone(),
                page: None,
            },
            purpose: control.name.clone(),
            source: "control_points".to_string(),
            resolution: match &control.input {
                SemanticInput::TargetCenter { target_id } => ElementResolution::NotEvaluated {
                    target_id: target_id.clone(),
                    reason: NotEvaluatedReason::Unscoped,
                },
                input => ElementResolution::Declared(geometry(input)?),
            },
        });
    }
    let mut missing = Vec::new();
    if let Some(page) = outcome
        .evaluations
        .iter()
        .find(|page| outcome.matched && page.page_id == outcome.page)
    {
        let definition = view
            .pages
            .pages
            .iter()
            .find(|definition| definition.id == page.page_id)
            .ok_or_else(|| CliError::package_invalid("matched page definition is unavailable"))?;
        for target in &page.target_results {
            let (role, group) = match target.role {
                PageTargetRole::Required => ("required", None),
                PageTargetRole::Optional => ("optional", None),
                PageTargetRole::AnyOf => (
                    "any_of",
                    definition
                        .any_of
                        .iter()
                        .position(|group| group.contains(&target.target_id)),
                ),
                PageTargetRole::Forbidden => ("forbidden", None),
            };
            missing.push(MissingTarget {
                id: target.target_id.clone(),
                role: role.to_string(),
                passed: target.passed,
                group_index: group,
                group_satisfied: group.map(|index| {
                    definition.any_of[index].iter().any(|id| {
                        page.target_results
                            .iter()
                            .any(|target| &target.target_id == id && target.passed)
                    })
                }),
            });
        }
    }
    shared::project(
        ProjectionInput {
            frame: FrameIdentity {
                kind: shared::FrameKind::Rgb8,
                sha256: format!("{:x}", Sha256::digest(scene.rgb8_pixels())),
                width: scene.width(),
                height: scene.height(),
            },
            matched_pages: outcome
                .evaluations
                .iter()
                .filter(|page| page.matched)
                .map(|page| page.page_id.clone())
                .collect(),
            elements,
            missing,
            fields: Vec::new(),
        },
        &view.metadata,
    )
}

fn geometry(input: &SemanticInput) -> CliOutcome<Geometry> {
    serde_json::from_value(semantic_input_json(input))
        .map_err(|error| CliError::package_invalid(error.to_string()))
}

fn resolve(
    input: &SemanticInput,
    evaluator: &RecognitionEvaluator,
    scene: &Scene,
    resolved: &mut std::collections::BTreeMap<String, ElementResolution>,
) -> CliOutcome<ElementResolution> {
    let SemanticInput::TargetCenter { target_id } = input else {
        return Ok(ElementResolution::Declared(geometry(input)?));
    };
    if let Some(result) = resolved.get(target_id) {
        return Ok(result.clone());
    }
    let result = match evaluator
        .target_kind(target_id)
        .map_err(|error| CliError::package_invalid(error.to_string()))?
    {
        TargetKind::Template | TargetKind::Color => {
            let evaluated = evaluator
                .evaluate_target(scene, target_id)
                .map_err(|error| CliError::usage(error.to_string()))?;
            let geometry = if evaluated.passed && evaluated.template.is_some() {
                let rect = target_evaluation_rect(&evaluated)?;
                Some(geometry(&SemanticInput::Tap {
                    rect,
                    point: rect_center(rect)?,
                })?)
            } else {
                None
            };
            ElementResolution::Target {
                target_id: target_id.clone(),
                passed: evaluated.passed,
                geometry,
            }
        }
        TargetKind::Ocr | TargetKind::Nn => ElementResolution::NotEvaluated {
            target_id: target_id.clone(),
            reason: NotEvaluatedReason::DynamicTarget,
        },
        TargetKind::ClickOnly => ElementResolution::NotEvaluated {
            target_id: target_id.clone(),
            reason: NotEvaluatedReason::ClickOnly,
        },
    };
    resolved.insert(target_id.clone(), result.clone());
    Ok(result)
}

pub(super) fn project(
    mut payload: Value,
    mut observation: PageProjection,
    flags: &FlagArgs,
    verbose: bool,
) -> CliOutcome<Value> {
    let mut request =
        lab2_projection_request(flags, payload["req_id"].as_str().map(str::to_string));
    if verbose && request.verbosity == ProjectionVerbosity::Min {
        request.verbosity = ProjectionVerbosity::Normal;
    }
    let mut requested_fields = request.fields.clone();
    if payload.get("facts").is_some() {
        requested_fields.insert("arbitration".to_string());
        request.fields.insert("arbitration".to_string());
        request
            .fields
            .extend(["facts", "projection_source", "terminal"].map(str::to_string));
    }
    request.fields.extend(
        [
            "observation",
            "matched",
            "candidates",
            "recovery_hint",
            "frame_source",
            "frame_path",
        ]
        .map(str::to_string),
    );
    if request.verbosity == ProjectionVerbosity::Min
        && let Some(object) = payload.as_object_mut()
    {
        for key in [
            "instance",
            "arbitration",
            "matched",
            "standby",
            "frame_age_ms",
        ] {
            if !requested_fields.contains(key) {
                object.remove(key);
            }
        }
        if !requested_fields.contains("frame_source") {
            object.remove("frame_source");
        }
        if let Some(source) = object.get_mut("projection_source")
            && source["kind"] == "runtime_global_ledger"
            && source.get("artifact").is_some()
        {
            source["artifact"] = json!({"artifact_id":source["artifact"]["artifact_id"], "sha256":source["artifact"]["sha256"]});
        }
    }
    loop {
        payload["observation"] = serde_json::to_value(&observation)
            .map_err(|error| CliError::device(error.to_string()))?;
        let projected = project_record(&payload, &request)
            .map_err(|error| CliError::device(error.to_string()))?;
        let bytes = serde_json::to_vec(&projected)
            .map_err(|error| CliError::device(error.to_string()))?
            .len();
        if request.verbosity != ProjectionVerbosity::Min
            || bytes <= actingcommand_ledger::MIN_PROJECTION_SOFT_LIMIT_BYTES
        {
            return Ok(projected);
        }
        if let Some(facts) = payload.get_mut("facts")
            && facts["rows"]
                .as_array()
                .is_some_and(|rows| !rows.is_empty())
        {
            facts["rows"] = json!([]);
            facts["omitted_count"] = facts["item_count"].clone();
            facts["omitted_target_evaluation_count"] = facts["target_evaluation_count"].clone();
            facts["truncated"] = json!(true);
            continue;
        }
        if observation.reduce_for_min()? {
            continue;
        }
        if bytes <= actingcommand_ledger::MIN_PROJECTION_HARD_LIMIT_BYTES {
            return Ok(projected);
        }
        return Err(CliError::package_invalid(
            "observation identity or its remaining resolvable element exceeds the Min byte limit",
        ));
    }
}
