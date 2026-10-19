// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::RuntimeReceipt;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::page_projection::{
    self as shared, ActionKey, ElementInput, ElementResolution, ElementRole, FrameIdentity,
    Geometry, MissingTarget, NotEvaluatedReason, PageProjection, ProjectionInput,
};
use actingcommand_lab::{
    CurrentPageRequest, LabOcr, ReadonlyRecognitionInput, RecoveryHintResponse,
};
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
                        skipped: target.actual.skipped,
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

/// Workflow #396 LAB-3: OCR targets resolve with `ocr`, the Runtime's OCR provider the
/// evaluator was loaded with. Without an OCR model an action's OCR target is not evaluated,
/// as observe never evaluated it before; the second value names those targets, which observe
/// reports under `lab_ocr_unavailable` without failing.
pub(super) fn build(
    view: &super::super::contained_resources::ObservationResources,
    evaluator: &RecognitionEvaluator,
    scene: &Scene,
    outcome: &PageDetectionOutcome,
    ocr: &LabOcr,
) -> CliOutcome<(PageProjection, std::collections::BTreeSet<String>)> {
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
                resolution: resolve(&edge.input, evaluator, scene, &mut resolved, ocr)?,
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
                resolution: resolve(&operation.input, evaluator, scene, &mut resolved, ocr)?,
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
                SemanticInput::TargetCenter { target_id }
                | SemanticInput::TargetOffset { target_id, .. } => {
                    ElementResolution::NotEvaluated {
                        target_id: target_id.clone(),
                        reason: NotEvaluatedReason::Unscoped,
                    }
                }
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
    let ocr_not_evaluated = not_evaluated_ocr_targets(evaluator, &resolved);
    let projection = shared::project(
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
    )?;
    Ok((projection, ocr_not_evaluated))
}

/// One action target's resolution on this frame, kept by `resolve` per target.
struct ResolvedTarget {
    resolution: ElementResolution,
    /// Workflow #390 LAB-2 item 7: the evaluation when it passed with a matched template box,
    /// from which an `offset` click takes its box.
    matched: Option<TargetEvaluation>,
}

/// The OCR targets of actions resolved as not evaluated: those `resolve` left without a model.
fn not_evaluated_ocr_targets(
    evaluator: &RecognitionEvaluator,
    resolved: &std::collections::BTreeMap<String, ResolvedTarget>,
) -> std::collections::BTreeSet<String> {
    resolved
        .iter()
        .filter(|(target_id, resolved)| {
            matches!(
                resolved.resolution,
                ElementResolution::NotEvaluated {
                    reason: NotEvaluatedReason::DynamicTarget,
                    ..
                }
            ) && matches!(
                evaluator.target_kind(target_id),
                Ok(TargetKind::Ocr | TargetKind::ValueTest)
            )
        })
        .map(|(target_id, _)| target_id.clone())
        .collect()
}

fn geometry(input: &SemanticInput) -> CliOutcome<Geometry> {
    serde_json::from_value(semantic_input_json(input))
        .map_err(|error| CliError::package_invalid(error.to_string()))
}

fn resolve(
    input: &SemanticInput,
    evaluator: &RecognitionEvaluator,
    scene: &Scene,
    resolved: &mut std::collections::BTreeMap<String, ResolvedTarget>,
    ocr: &LabOcr,
) -> CliOutcome<ElementResolution> {
    let (target_id, offset) = match input {
        SemanticInput::TargetCenter { target_id } => (target_id, None),
        SemanticInput::TargetOffset { target_id, offset } => (target_id, Some(*offset)),
        _ => return Ok(ElementResolution::Declared(geometry(input)?)),
    };
    if !resolved.contains_key(target_id) {
        let target = resolve_target(target_id, evaluator, scene, ocr)?;
        resolved.insert(target_id.clone(), target);
    }
    let target = &resolved[target_id];
    // Workflow #390 LAB-2 item 7: an `offset` click's box is its offset from the box the
    // template matched; a target that did not pass, or matched no template box, gives none.
    match (offset, &target.matched) {
        (Some(offset), Some(matched)) => {
            let rect = target_offset_rect(matched, offset)?;
            Ok(ElementResolution::Target {
                target_id: target_id.clone(),
                passed: matched.passed,
                geometry: Some(geometry(&SemanticInput::Tap {
                    rect,
                    point: rect_center(rect)?,
                })?),
            })
        }
        _ => Ok(target.resolution.clone()),
    }
}

/// The resolution of the action target `target_id` on this frame, with its matched evaluation.
fn resolve_target(
    target_id: &str,
    evaluator: &RecognitionEvaluator,
    scene: &Scene,
    ocr: &LabOcr,
) -> CliOutcome<ResolvedTarget> {
    let mut matched = None;
    let resolution = match evaluator
        .target_kind(target_id)
        .map_err(|error| CliError::package_invalid(error.to_string()))?
    {
        // Workflow #396 LAB-3: without an OCR model an action's OCR target stays not evaluated,
        // as before; `observe` reports it under `lab_ocr_unavailable` (`build`). Workflow #396
        // REC-6: so does a value test, which reads OCR targets.
        TargetKind::Ocr | TargetKind::ValueTest if ocr.provider().is_none() => {
            ElementResolution::NotEvaluated {
                target_id: target_id.to_owned(),
                reason: NotEvaluatedReason::DynamicTarget,
            }
        }
        // A digest or composite is evaluated like a color target and, having no template match,
        // gives no geometry. Workflow #396 LAB-3: so is an OCR target, with the Runtime's OCR
        // provider; a composite with an OCR member is refused without an OCR model. Workflow
        // #396 REC-6: a value test is evaluated likewise.
        TargetKind::Template
        | TargetKind::Color
        | TargetKind::ColorDigest
        | TargetKind::Composite
        | TargetKind::ValueTest
        | TargetKind::Ocr => {
            ocr.require(evaluator.pack(), [target_id])?;
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
            let passed = evaluated.passed;
            if geometry.is_some() {
                matched = Some(evaluated);
            }
            ElementResolution::Target {
                target_id: target_id.to_owned(),
                passed,
                geometry,
            }
        }
        TargetKind::Nn => ElementResolution::NotEvaluated {
            target_id: target_id.to_owned(),
            reason: NotEvaluatedReason::DynamicTarget,
        },
        TargetKind::ClickOnly => ElementResolution::NotEvaluated {
            target_id: target_id.to_owned(),
            reason: NotEvaluatedReason::ClickOnly,
        },
    };
    Ok(ResolvedTarget {
        resolution,
        matched,
    })
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
            // #411 A9: the marks of a cut near-miss list stay beside it.
            "candidates_truncated",
            "candidates_omitted_count",
            "recovery_hint",
            "frame_source",
            "frame_path",
            "candidate_sets",
            "candidate_coverage",
            "recognition_coverage",
            "ocr",
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
    let mut omitted_near_misses = 0;
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
        // #391 MCP-391b: the one resolvable element Min keeps does not fit either, so it is
        // omitted too and the identity answers alone, marked truncated with its counts.
        if observation.reduce_to_identity()? {
            continue;
        }
        // #411 A9: an unknown page's near-miss `candidates` are a transport member, not the
        // identity (contracts/page-projection.md), so Min removes them from the end before the
        // identity is judged; only an identity that alone does not fit fails.
        if omit_last_near_miss(&mut payload, &mut omitted_near_misses) {
            continue;
        }
        return Err(identity_limit_exceeded(bytes));
    }
}

/// #411 A9: removes the last entry of the near-miss `candidates` (sorted by score, the lowest
/// last) and of the suspicion's copy of that list, and marks the answer with
/// `candidates_truncated` and the number removed so far, `candidates_omitted_count`. Returns
/// whether an entry was left to remove.
fn omit_last_near_miss(payload: &mut Value, omitted: &mut usize) -> bool {
    let Some(candidates) = payload.get_mut("candidates").and_then(Value::as_array_mut) else {
        return false;
    };
    if candidates.pop().is_none() {
        return false;
    }
    let kept = candidates.len();
    *omitted += 1;
    if let Some(suspected) = payload
        .pointer_mut("/suspicion/candidates")
        .and_then(Value::as_array_mut)
    {
        suspected.truncate(kept);
    }
    payload["candidates_truncated"] = json!(true);
    payload["candidates_omitted_count"] = json!(*omitted);
    true
}

/// #391 MCP-391b: Min's identity alone exceeds the hard byte limit.
/// #411 A21: a failed `observe --capture` answers `device_error` (exit 4) with the Runtime's
/// answer in its details: the Runtime outcome's code as `error`, the failed receipt's
/// `req_id` and `receipt_state` when one was received, the outcome itself, the debug session's
/// correlation and the hint to read the Runtime ledger.
pub(super) fn runtime_observe_failure(
    message: String,
    outcome: &Outcome,
    receipt: Option<&RuntimeReceipt>,
    correlation_id: CorrelationId,
) -> CliError {
    // Red proof M6: the failure carries no details.
    let _ = (outcome, receipt, correlation_id);
    CliError::device(message)
}

fn identity_limit_exceeded(actual_bytes: usize) -> CliError {
    CliError::new(
        ErrorKind::UsageValidation,
        crate::codes::ActinglabCode::ObserveIdentityLimitExceeded.as_str(),
        "the Min output exceeds its byte limit with only its identity left; use --verbose",
        &[],
    )
    .with_details(json!({
        "actual_bytes": actual_bytes,
        "limit_bytes": actingcommand_ledger::MIN_PROJECTION_HARD_LIMIT_BYTES,
    }))
}

#[cfg(test)]
mod tests {
    //! #391 MCP-391b (MCP-0 command table v1.1 §4.23): `observe`'s Min output over its byte
    //! limit answers the identity marked truncated with its omitted counts; an identity that
    //! alone exceeds the limit gets actinglab's own code, never `package_invalid`.

    use super::*;
    use crate::codes::ActinglabCode;
    use actingcommand_contract::outcome::Category;
    use actingcommand_ledger::MIN_PROJECTION_HARD_LIMIT_BYTES;

    const PAGE: &str = "reward_overview";

    /// A recognised page with a swipe and a tap declared on it, projected by the shared owner.
    /// The swipe comes first, so it is the resolvable element Min keeps longest.
    fn recognised_page(kind: shared::FrameKind) -> PageProjection {
        let catalog = shared::ProjectionCatalog::from_resources(
            &json!({"coordinate_space": {"width": 1280, "height": 720}, "targets": [{"id": "title"}]}),
            &json!({"pages": [{"id": PAGE}]}),
            &json!({"navigation": [
                {"id": "scroll_reward_list_down", "from_page": PAGE},
                {"id": "open_reward_detail", "from_page": PAGE}
            ]}),
        )
        .unwrap();
        let element = |resource_id: &str, geometry: Geometry| ElementInput {
            action: ActionKey {
                role: ElementRole::Navigate,
                task_id: String::new(),
                resource_id: resource_id.to_string(),
                page: Some(PAGE.to_string()),
            },
            purpose: resource_id.to_string(),
            source: "navigation".to_string(),
            resolution: ElementResolution::Declared(geometry),
        };
        let rect = |x, y, width, height| shared::Rect {
            x,
            y,
            width,
            height,
        };
        let point = |x, y| shared::Point { x, y };
        shared::project(
            ProjectionInput {
                frame: FrameIdentity {
                    kind,
                    sha256: "5f0c".repeat(16),
                    width: 1280,
                    height: 720,
                },
                matched_pages: vec![PAGE.to_string()],
                elements: vec![
                    element(
                        "scroll_reward_list_down",
                        Geometry::Drag {
                            from_rect: rect(600, 520, 80, 80),
                            to_rect: rect(600, 200, 80, 80),
                            from: point(640, 560),
                            to: point(640, 240),
                            duration_ms: 400,
                        },
                    ),
                    element(
                        "open_reward_detail",
                        Geometry::Tap {
                            rect: rect(1040, 600, 200, 80),
                            point: point(1140, 640),
                        },
                    ),
                ],
                missing: vec![],
                fields: vec![],
            },
            &shared::VerifiedProjectionMetadata::unannotated(catalog),
        )
        .unwrap()
    }

    /// The members `observe --capture` sends with its projection (`run_runtime_observe`), with
    /// `--with-frame`: three fact rows, the projection source and the terminal.
    fn capture_payload(observation: &PageProjection) -> Value {
        json!({
            "req_id": "request_18dd54d1a2b3c4d50000000000000101",
            "state": "recognized",
            "instance": "node.a",
            "page": PAGE,
            "matched": true,
            "standby": false,
            "frame_source": {"artifact_id": "artifact_18dd54d1a2b3c4d50000000000000103"},
            "facts": {
                "rows": [
                    {"kind": "page_evaluation", "page_id": PAGE, "matched": true, "target_evaluation_count": 2},
                    {"kind": "target_evaluation", "page_id": PAGE, "target_id": "title", "passed": true},
                    {"kind": "target_evaluation", "page_id": PAGE, "target_id": "badge", "passed": true}
                ],
                "item_count": 3,
                "omitted_count": 0,
                "target_evaluation_count": 2,
                "omitted_target_evaluation_count": 0,
                "truncated": false
            },
            "projection_source": {
                "kind": "runtime_global_ledger",
                "correlation_id": "correlation_18dd54d1a2b3c4d50000000000000100",
                "artifact": {
                    "artifact_id": "artifact_18dd54d1a2b3c4d50000000000000103",
                    "sha256": "9a3e".repeat(16)
                },
                "projection_sequence": 4242,
                "projection_event_id": "event_18dd54d1a2b3c4d50000000000000104",
                "content_sha256": observation.content_sha256
            },
            "terminal": {"sequence": 4243, "event_id": "event_18dd54d1a2b3c4d50000000000000105"},
            "arbitration": {"authority": "runtime_scheduler", "decision": "readonly", "local_arbitrator": false},
            "frame_path": "C:/Users/tester/AppData/Local/Temp/actinglab-observe/reward_overview-0001.png"
        })
    }

    /// The members offline `observe` sends for the same page when its package declares a
    /// candidate layout of twelve rows (`run_observe`).
    fn offline_payload_with_candidate_rows() -> Value {
        let candidates = (1..=12u32)
            .map(|index| {
                let y = 96 + 44 * index;
                json!({
                    "id": format!("reward_rows#{index:02}"),
                    "instance_index": index,
                    "actionable": true,
                    "privacy": "public",
                    "rect": {"x": 96, "y": y, "width": 880, "height": 40},
                    "click": {"x": 120, "y": y + 8, "width": 832, "height": 24}
                })
            })
            .collect::<Vec<_>>();
        json!({
            "req_id": "request_18dd54d1a2b3c4d50000000000000201",
            "state": "observed",
            "instance": "node.a",
            "page": PAGE,
            "matched": true,
            "standby": false,
            "frame_age_ms": 0,
            "backend": "scene_file",
            "frame_source": {"kind": "scene_file"},
            "targets": [{"id": "title", "passed": true, "score": 1.0}],
            "actions": [{"id": "scroll_reward_list_down"}, {"id": "open_reward_detail"}],
            "arbitration": {"authority": "isolated_offline", "decision": "not_applicable", "persistent_state": false},
            "candidate_sets": [{
                "schema_version": "actingcommand.candidate-projection.v1",
                "page_id": PAGE,
                "layout_id": "reward_rows",
                "layout_kind": "fixed_slots",
                "frame": {"width": 1280, "height": 720},
                "candidates": candidates,
                "candidate_set_sha256": "7c21".repeat(16)
            }],
            "candidate_coverage": {"provided_frames": 1, "aggregation": "production_owner", "task_preparation": "not_requested"}
        })
    }

    fn encoded_len(value: &Value) -> usize {
        serde_json::to_vec(value).unwrap().len()
    }

    #[test]
    fn min_output_over_its_byte_limit_answers_the_identity_marked_truncated() {
        let observation = recognised_page(shared::FrameKind::Artifact);
        let kept = observation.elements[0].clone();
        let payload = capture_payload(&observation);

        let output = project(payload, observation, &FlagArgs::default(), false)
            .expect("Min answers the identity instead of failing");

        // The scene: the swipe Min keeps longest does not fit beside the identity.
        let mut with_kept = output.clone();
        with_kept["observation"]["elements"] = json!([kept]);
        assert!(encoded_len(&with_kept) > MIN_PROJECTION_HARD_LIMIT_BYTES);

        assert!(encoded_len(&output) <= MIN_PROJECTION_HARD_LIMIT_BYTES);
        let projected = &output["observation"];
        assert_eq!(projected["page"], PAGE);
        assert_eq!(projected["frame"]["sha256"], "5f0c".repeat(16));
        assert_eq!(projected["elements"], json!([]));
        assert_eq!(projected["truncated"], true);
        assert_eq!(projected["output_truncated"], true);
        assert_eq!(projected["omitted_count"], 2);
        assert_eq!(projected["metrics"]["entry_count"], 2);
        assert_eq!(projected["metrics"]["emitted_count"], 0);
        let mut transported: PageProjection = serde_json::from_value(projected.clone()).unwrap();
        transported
            .verify_transport()
            .expect("the truncation state is freshly hashed");
        let facts = &output["facts"];
        assert_eq!(facts["rows"], json!([]));
        assert_eq!(facts["truncated"], true);
        assert_eq!(facts["omitted_count"], 3);
        assert_eq!(facts["item_count"], 3);
        assert_eq!(facts["omitted_target_evaluation_count"], 2);
        assert_eq!(facts["target_evaluation_count"], 2);
    }

    #[test]
    fn min_identity_over_its_byte_limit_reports_its_own_code() {
        let observation = recognised_page(shared::FrameKind::Rgb8);

        let error = project(
            offline_payload_with_candidate_rows(),
            observation,
            &FlagArgs::default(),
            false,
        )
        .expect_err("an identity over the byte limit cannot be answered");

        let code = ActinglabCode::ObserveIdentityLimitExceeded;
        assert_eq!(error.code, code.as_str());
        assert_eq!(code.category(), Category::Error);
        let details = error.details.expect("the code's required values");
        assert_eq!(details["limit_bytes"], MIN_PROJECTION_HARD_LIMIT_BYTES);
        assert!(
            details["actual_bytes"]
                .as_u64()
                .is_some_and(|bytes| bytes > MIN_PROJECTION_HARD_LIMIT_BYTES as u64)
        );
    }
}

// Workflow #390 LAB-2 item 7 (exec key 77): the navigation reader and observe take a REC-4
// `offset` click.
#[cfg(test)]
mod gate_390_navoffset;

// #411 A9 (exec key 86): Min answers an unknown page as unknown, its near-miss list cut from the
// end to fit.
#[cfg(test)]
mod gate_rc2_a9;

// #411 A21 (exec key 101): a failed `observe --capture` carries the request id and the Runtime's
// outcome.
#[cfg(test)]
mod gate_411_a21;

#[cfg(test)]
mod lab3_tests {
    // Workflow #396 LAB-3 (FCM T-02): offline `observe` evaluates an action's OCR target with the
    // Runtime's OCR provider. Without an OCR model the action is not evaluated, as before, and its
    // target is named for the `ocr` member observe reports under `lab_ocr_unavailable`; observe
    // does not fail for an action it only lists.

    use super::*;
    use actingcommand_recognition_pack::{
        FsAssetResolver, NnProviderRequest, NnProviderResult, OcrProviderRequest,
        OcrProviderResult, OcrProviderTextBlock, VisionProvider, VisionProviderError,
        VisionProviderErrorCode,
    };
    use std::collections::BTreeMap;
    use std::sync::Arc;

    const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    // Reads "Start" in every region, at confidence 0.95.
    #[derive(Debug)]
    struct ReadsStart;

    impl VisionProvider for ReadsStart {
        fn require_ocr_model(
            &self,
            _model_ref: &str,
            _model_sha256: &str,
        ) -> Result<(), VisionProviderError> {
            Ok(())
        }

        fn require_nn_model(
            &self,
            _model_ref: &str,
            _model_sha256: &str,
        ) -> Result<(), VisionProviderError> {
            Err(VisionProviderError::new(
                VisionProviderErrorCode::Unavailable,
                "no NN model in this fixture",
            ))
        }

        fn read_text(
            &self,
            request: OcrProviderRequest<'_>,
        ) -> Result<OcrProviderResult, VisionProviderError> {
            Ok(OcrProviderResult {
                ppocr_diagnostics: Vec::new(),
                text: "Start".to_string(),
                blocks: vec![OcrProviderTextBlock {
                    text: "Start".to_string(),
                    rect: request.region,
                    confidence: Some(0.95),
                }],
                confidence: Some(0.95),
                detection: None,
            })
        }

        fn classify(
            &self,
            _request: NnProviderRequest<'_>,
        ) -> Result<NnProviderResult, VisionProviderError> {
            Err(VisionProviderError::new(
                VisionProviderErrorCode::Unavailable,
                "no NN model in this fixture",
            ))
        }
    }

    fn evaluator(provider: Option<Arc<dyn VisionProvider>>) -> RecognitionEvaluator {
        let pack = load_pack_from_json_str(
            &json!({
                "schema_version": "0.7",
                "coordinate_space": {"width": 64, "height": 32},
                "targets": [{
                    "type": "ocr", "id": "label/open", "region": {"x": 8, "y": 8, "width": 32, "height": 16},
                    "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact",
                    "expected": ["Start"], "case_sensitive": false, "minimum_confidence": 0.8,
                    "model_ref": "neutral-ocr", "model_sha256": MODEL_SHA256
                }]
            })
            .to_string(),
        )
        .expect("the neutral pack");
        let resolver = Arc::new(FsAssetResolver::new(PathBuf::new()));
        match provider {
            Some(provider) => RecognitionEvaluator::with_vision_provider(pack, resolver, provider),
            None => RecognitionEvaluator::with_asset_resolver(pack, resolver),
        }
        .expect("evaluator")
    }

    #[test]
    fn lab3_observe_evaluates_an_action_ocr_target_and_names_it_not_evaluated_without_a_model() {
        let scene = Scene::from_rgb8(64, 32, &[255u8, 0, 0].repeat(64 * 32)).expect("scene");
        let input = SemanticInput::TargetCenter {
            target_id: "label/open".to_string(),
        };

        let provider: Arc<dyn VisionProvider> = Arc::new(ReadsStart);
        let available = LabOcr::with_provider(Arc::clone(&provider));
        let resolution = resolve(
            &input,
            &evaluator(Some(provider)),
            &scene,
            &mut BTreeMap::new(),
            &available,
        )
        .expect("an OCR target is evaluated");
        assert!(
            matches!(
                &resolution,
                ElementResolution::Target { target_id, passed: true, .. } if target_id == "label/open"
            ),
            "{resolution:?}"
        );

        let unavailable = LabOcr::from_installation(None);
        let without_model = evaluator(None);
        let mut resolved = BTreeMap::new();
        let resolution = resolve(&input, &without_model, &scene, &mut resolved, &unavailable)
            .expect("an action's OCR target without a model does not fail observe");
        assert!(
            matches!(
                &resolution,
                ElementResolution::NotEvaluated {
                    target_id,
                    reason: NotEvaluatedReason::DynamicTarget,
                } if target_id == "label/open"
            ),
            "{resolution:?}"
        );
        assert_eq!(
            not_evaluated_ocr_targets(&without_model, &resolved),
            std::collections::BTreeSet::from(["label/open".to_string()])
        );
    }
}
