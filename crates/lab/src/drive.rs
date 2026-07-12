// SPDX-License-Identifier: AGPL-3.0-only

use crate::readonly::{
    detect_current_page, load_evaluator, load_page_detector, needs_detection, recognition_scene,
    rect_response, target_evaluation_response,
};
use crate::{Clock, Lab, LabPorts, SemanticInputExecutor, SemanticLedgerContext};
use actingcommand_contract::{EnvResolved, LabError, LabResult};
use actingcommand_execution_kernel::{
    DriveDecisionError, DriveDecisionErrorKind, DriveNavigationEdge as NavigationEdge,
    DriveNavigationGraph as NavigationGraph, DrivePoint, DriveSemanticInput as SemanticInput,
    derive_absolute_coordinate_rect_from_match as derive_kernel_coordinate_rect, drive_rect_center,
    reject_dangerous_semantic_id as reject_kernel_dangerous_semantic_id,
};
use actingcommand_ledger::IdKind;
use actingcommand_page_detector::PageDetector;
use actingcommand_recognition_pack::{
    PackRect, RecognitionEvaluator, TargetEvaluation, TargetKind,
};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

impl<P: LabPorts> Lab<P> {
    pub fn tap_target(
        &mut self,
        mut request: crate::TapTargetRequest,
        ledger: &mut SemanticLedgerContext,
    ) -> LabResult<crate::TapTargetResponse> {
        if !request.allow_destructive {
            reject_dangerous_semantic_id("target", &request.target)?;
        }
        if !request.dry_run && !request.capture_requested {
            return Err(LabError::usage(
                "tap-target real execution requires --capture; use --dry-run with --scene for offline planning",
            ));
        }

        let (evaluator, env_resolved) = load_evaluator(self, &mut request.input)?;
        record_env_resolved(ledger, "tap-target", &env_resolved)?;
        if evaluator
            .target_kind(&request.target)
            .map_err(|error| LabError::usage(error.to_string()))?
            == TargetKind::ClickOnly
        {
            return Err(LabError::usage(format!(
                "tap-target requires a visually evaluatable target; '{}' is click-only",
                request.target
            )));
        }

        let scene = recognition_scene(self, &mut request.input)?;
        let evaluation = evaluator
            .evaluate_target(&scene, &request.target)
            .map_err(|error| LabError::usage(error.to_string()))?;
        let evaluation_response = target_evaluation_response(&evaluation);
        let reco_id = ledger.issue(IdKind::Reco);
        ledger.record_drive(json!({
            "stage": "recognition",
            "command": "tap-target",
            "target": request.target,
            "reco_id": reco_id,
            "evaluation": evaluation_response
        }))?;
        if !evaluation.passed {
            record_env_needs_detection(
                ledger,
                "tap-target",
                "target_below_threshold",
                &request.target,
                &env_resolved,
            )?;
            let mut details = json!({
                "target": request.target,
                "evaluation": evaluation_response
            });
            attach_env_details(
                &mut details,
                "tap-target",
                "target_below_threshold",
                &request.target,
                &env_resolved,
            )?;
            return Err(LabError::safety_blocked(
                "target_not_visible",
                format!(
                    "target '{}' did not pass recognition: {}",
                    request.target, evaluation.message
                ),
                &["visible_target"],
            )
            .with_details(details));
        }

        let click = evaluator
            .get_click_target(&request.target)
            .map_err(|error| LabError::usage(error.to_string()))?;
        let point = rect_center(click)?;
        let action_id = ledger.issue(IdKind::Action);
        let response = if request.dry_run {
            ledger.record_drive(json!({
                "stage": "action_plan",
                "command": "tap-target",
                "target": request.target,
                "action_id": action_id,
                "executed": false,
                "click": rect_response(click),
                "point": point
            }))?;
            crate::TapTargetResponse {
                status: "planned".to_string(),
                executed: false,
                target: request.target,
                req_id: ledger.req_id.clone(),
                reco_id,
                action_id,
                click: rect_response(click),
                point,
                evaluation: evaluation_response,
                safety_gate: "navigation_only_default".to_string(),
                device: None,
                env_resolved,
            }
        } else {
            let input = SemanticInput::Tap {
                rect: click,
                point: DrivePoint {
                    x: point.x,
                    y: point.y,
                },
            };
            let device = send_semantic_input(self, &input)?;
            ledger.record_drive(json!({
                "stage": "action",
                "command": "tap-target",
                "target": request.target,
                "action_id": action_id,
                "executed": true,
                "click": rect_response(click),
                "point": point,
                "device": device
            }))?;
            crate::TapTargetResponse {
                status: "sent".to_string(),
                executed: true,
                target: request.target,
                req_id: ledger.req_id.clone(),
                reco_id,
                action_id,
                click: rect_response(click),
                point,
                evaluation: evaluation_response,
                safety_gate: "navigation_only_default".to_string(),
                device: Some(device),
                env_resolved,
            }
        };
        Ok(response)
    }

    pub fn navigate(
        &mut self,
        mut request: crate::NavigateRequest,
        ledger: &mut SemanticLedgerContext,
    ) -> LabResult<crate::NavigateResponse> {
        if !request.dry_run && !request.capture_requested {
            return Err(LabError::usage(
                "navigate real execution requires --capture; use --dry-run with --scene for route planning",
            ));
        }

        let (evaluator, env_resolved) = load_evaluator(self, &mut request.input)?;
        let detector = load_page_detector(
            &request.input,
            "semantic page commands require --pages or --resource-root --game",
        )?;
        detector
            .validate(&evaluator)
            .map_err(|error| LabError::usage(error.to_string()))?;
        record_env_resolved(ledger, "navigate", &env_resolved)?;
        let graph = load_navigation_graph(request.input.resources.loaded_bundle())?;
        let scene = recognition_scene(self, &mut request.input)?;
        let start = detect_current_page(
            &evaluator,
            &detector,
            &scene,
            "navigate",
            env_resolved.clone(),
        )?;
        let reco_id = ledger.issue(IdKind::Reco);
        ledger.record_drive(json!({
            "stage": "recognition",
            "command": "navigate",
            "reco_id": reco_id,
            "page": start.page,
            "matched": start.matched,
            "standby": start.standby
        }))?;
        if start.standby {
            record_env_needs_detection(
                ledger,
                "navigate",
                "current_page_unknown",
                &start.page,
                &env_resolved,
            )?;
            let details = serde_json::to_value(&start).map_err(|error| {
                LabError::device(format!(
                    "failed to serialize page detection details: {error}"
                ))
            })?;
            return Err(LabError::safety_blocked(
                "current_page_unknown",
                "navigate requires a matched current page before clicking",
                &["current_page"],
            )
            .with_details(details));
        }

        let target_page = graph.canonical_page(&request.to);
        if start.page == target_page {
            return Ok(crate::NavigateResponse {
                status: "already_at_target".to_string(),
                executed: false,
                req_id: ledger.req_id.clone(),
                reco_id,
                from: Some(start.page),
                to: target_page,
                route: Some(Vec::new()),
                steps: None,
                safety_gate: None,
                env_resolved,
            });
        }

        let route = graph.find_route(&start.page, &target_page).ok_or_else(|| {
            LabError::usage(format!(
                "no navigation route from '{}' to '{}'",
                start.page, target_page
            ))
        })?;
        if !request.allow_destructive {
            graph.validate_route(&route).map_err(drive_decision_error)?;
        }
        let action_ids = route
            .iter()
            .map(|_| ledger.issue(IdKind::Action))
            .collect::<Vec<_>>();
        let route_response = route
            .iter()
            .zip(&action_ids)
            .map(|(edge, action_id)| navigation_edge_response(edge, Some(action_id.clone())))
            .collect::<Vec<_>>();
        if request.dry_run {
            ledger.record_drive(json!({
                "stage": "action_plan",
                "command": "navigate",
                "executed": false,
                "action_ids": action_ids,
                "route": route_response
            }))?;
            return Ok(crate::NavigateResponse {
                status: "planned".to_string(),
                executed: false,
                req_id: ledger.req_id.clone(),
                reco_id,
                from: Some(start.page),
                to: target_page,
                route: Some(route_response),
                steps: None,
                safety_gate: Some("navigation_only_default".to_string()),
                env_resolved,
            });
        }

        let step_timeout = required_duration(request.step_timeout.as_ref(), "step timeout")?;
        let poll = required_duration(request.poll.as_ref(), "poll interval")?;
        let mut execution = NavigationExecutionContext {
            lab: self,
            input: &mut request.input,
            evaluator: &evaluator,
            detector: &detector,
            graph: &graph,
            step_timeout,
            poll,
        };
        let (steps, _) =
            execute_navigation_route(&mut execution, start.page, route, action_ids.clone())?;
        ledger.record_drive(json!({
            "stage": "action",
            "command": "navigate",
            "executed": true,
            "action_ids": action_ids,
            "steps": steps
        }))?;
        Ok(crate::NavigateResponse {
            status: "arrived".to_string(),
            executed: true,
            req_id: ledger.req_id.clone(),
            reco_id,
            from: None,
            to: target_page,
            route: None,
            steps: Some(steps),
            safety_gate: Some("navigation_only_default".to_string()),
            env_resolved,
        })
    }
}

fn load_navigation_graph(
    bundle: &actingcommand_pack_containment::LoadedBundle,
) -> LabResult<NavigationGraph> {
    let navigation = bundle.navigation().ok_or_else(|| {
        LabError::package_invalid("externally verified resource bundle has no navigation graph")
    })?;
    let text = serde_json::to_string(navigation).map_err(|error| {
        LabError::package_invalid(format!(
            "failed to serialize contained navigation graph: {error}"
        ))
    })?;
    NavigationGraph::parse_json(&text).map_err(drive_decision_error)
}

fn navigation_edge_response(
    edge: &NavigationEdge,
    action_id: Option<String>,
) -> crate::NavigationEdgeResponse {
    crate::NavigationEdgeResponse {
        id: edge.id().to_string(),
        from_page: edge.from_page().to_string(),
        to_page: edge.to_page().to_string(),
        input: semantic_input_response(edge.input()),
        source: edge.source().map(str::to_string),
        action_id,
    }
}

fn semantic_input_response(input: &SemanticInput) -> crate::SemanticInputResponse {
    match input {
        SemanticInput::Tap { rect, point } => crate::SemanticInputResponse::Tap {
            rect: rect_response(*rect),
            point: point_response(*point),
        },
        SemanticInput::TargetCenter { target_id } => crate::SemanticInputResponse::TargetCenter {
            target_id: target_id.clone(),
        },
        SemanticInput::Drag {
            from_rect,
            to_rect,
            from,
            to,
            duration_ms,
        } => crate::SemanticInputResponse::Drag {
            from_rect: rect_response(*from_rect),
            to_rect: rect_response(*to_rect),
            from: point_response(*from),
            to: point_response(*to),
            duration_ms: *duration_ms,
        },
    }
}

fn reject_dangerous_semantic_id(label: &str, value: &str) -> LabResult<()> {
    reject_kernel_dangerous_semantic_id(label, value).map_err(drive_decision_error)
}

fn rect_center(rect: PackRect) -> LabResult<crate::PointResponse> {
    drive_rect_center(rect)
        .map(point_response)
        .map_err(drive_decision_error)
}

pub fn derive_absolute_coordinate_rect_from_match(
    kind: &str,
    declared: PackRect,
    expected_rect: PackRect,
    matched_rect: PackRect,
) -> LabResult<PackRect> {
    derive_kernel_coordinate_rect(kind, declared, expected_rect, matched_rect)
        .map_err(drive_decision_error)
}

fn point_response(point: DrivePoint) -> crate::PointResponse {
    crate::PointResponse {
        x: point.x,
        y: point.y,
    }
}

fn drive_decision_error(error: DriveDecisionError) -> LabError {
    match error.kind() {
        DriveDecisionErrorKind::InvalidInput => LabError::usage(error.message()),
        DriveDecisionErrorKind::PackageInvalid => LabError::package_invalid(error.message()),
        DriveDecisionErrorKind::SafetyBlocked => {
            LabError::safety_blocked(error.code(), error.message(), error.required_conditions())
        }
    }
}

fn required_duration(duration: Option<&LabResult<Duration>>, label: &str) -> LabResult<Duration> {
    duration
        .ok_or_else(|| LabError::device(format!("{label} is missing")))?
        .clone()
}

fn send_semantic_input<P: LabPorts>(
    lab: &Lab<P>,
    input: &SemanticInput,
) -> LabResult<crate::SemanticDeviceResponse> {
    let action = input
        .resolved_input_action()
        .map_err(drive_decision_error)?;
    let report = lab.ports().semantic_input().execute(action)?;
    Ok(crate::SemanticDeviceResponse {
        report,
        control_mode: "semantic".to_string(),
        action: semantic_input_response(input),
    })
}

struct NavigationExecutionContext<'a, P: LabPorts> {
    lab: &'a mut Lab<P>,
    input: &'a mut crate::ReadonlyRecognitionInput,
    evaluator: &'a RecognitionEvaluator,
    detector: &'a PageDetector,
    graph: &'a NavigationGraph,
    step_timeout: Duration,
    poll: Duration,
}

fn execute_navigation_route<P: LabPorts>(
    context: &mut NavigationExecutionContext<'_, P>,
    start_page: String,
    route: Vec<NavigationEdge>,
    action_ids: Vec<String>,
) -> LabResult<(Vec<crate::NavigationStepResponse>, String)> {
    let mut executed = Vec::new();
    let mut current_page = start_page;
    for (edge, action_id) in route.into_iter().zip(action_ids) {
        if current_page != edge.from_page() {
            return Err(LabError::safety_blocked(
                "navigation_page_drift",
                format!(
                    "navigation expected current page '{}' but last page was '{}'",
                    edge.from_page(),
                    current_page
                ),
                &["page_guard"],
            ));
        }
        let (input, recognition) = resolve_navigation_edge_input(context, &edge)?;
        context
            .graph
            .validate_resolved_input(&edge, &input)
            .map_err(drive_decision_error)?;
        let device = send_semantic_input(context.lab, &input)?;
        let arrived = poll_for_page(context, edge.to_page())?;
        if !arrived.matched {
            return Err(LabError::safety_blocked(
                "navigation_arrival_failed",
                format!(
                    "navigation edge '{}' did not arrive at '{}'; last page '{}'",
                    edge.id(),
                    edge.to_page(),
                    arrived.page
                ),
                &["arrival_page"],
            ));
        }
        current_page = arrived.page.clone();
        executed.push(crate::NavigationStepResponse {
            action_id,
            edge: navigation_edge_response(&edge, None),
            resolved_input: semantic_input_response(&input),
            recognition,
            device,
            arrived,
        });
    }
    Ok((executed, current_page))
}

fn resolve_navigation_edge_input<P: LabPorts>(
    context: &mut NavigationExecutionContext<'_, P>,
    edge: &NavigationEdge,
) -> LabResult<(
    SemanticInput,
    Option<crate::NavigationTargetRecognitionResponse>,
)> {
    let SemanticInput::TargetCenter { target_id } = edge.input() else {
        return Ok((edge.input().clone(), None));
    };
    let scene = recognition_scene(context.lab, context.input)?;
    let evaluation = context
        .evaluator
        .evaluate_target(&scene, target_id)
        .map_err(|error| LabError::usage(error.to_string()))?;
    let evaluation_response = target_evaluation_response(&evaluation);
    if !evaluation.passed {
        return Err(LabError::safety_blocked(
            "navigation_target_not_visible",
            format!(
                "navigation edge '{}' target '{}' did not pass recognition: {}",
                edge.id(),
                target_id,
                evaluation.message
            ),
            &["visible_target", "navigation"],
        ));
    }
    let rect = target_evaluation_rect(&evaluation)?;
    Ok((
        SemanticInput::Tap {
            rect,
            point: drive_rect_center(rect).map_err(drive_decision_error)?,
        },
        Some(crate::NavigationTargetRecognitionResponse {
            target_id: target_id.clone(),
            evaluation: evaluation_response,
        }),
    ))
}

fn target_evaluation_rect(evaluation: &TargetEvaluation) -> LabResult<PackRect> {
    let template = evaluation.template.as_ref().ok_or_else(|| {
        LabError::usage(format!(
            "target '{}' has no matched template rect",
            evaluation.id
        ))
    })?;
    Ok(PackRect {
        x: template.x,
        y: template.y,
        width: template.width,
        height: template.height,
    })
}

fn poll_for_page<P: LabPorts>(
    context: &mut NavigationExecutionContext<'_, P>,
    page_id: &str,
) -> LabResult<crate::PageDetectionResponse> {
    let started = Instant::now();
    let mut last = None;
    while started.elapsed() <= context.step_timeout {
        context.lab.ports().clock().sleep(context.poll);
        let scene = recognition_scene(context.lab, context.input)?;
        let outcome = detect_current_page(
            context.evaluator,
            context.detector,
            &scene,
            "navigate",
            Vec::new(),
        )?;
        if outcome.matched && outcome.page == page_id {
            return Ok(outcome);
        }
        last = Some(outcome);
    }
    Ok(last.unwrap_or_else(standby_page_response))
}

fn standby_page_response() -> crate::PageDetectionResponse {
    crate::PageDetectionResponse {
        page: "standby".to_string(),
        matched: false,
        standby: true,
        evaluations: Vec::new(),
        recovery_hint: Some(crate::RecoveryHintResponse {
            action: "wake_safe_point".to_string(),
            point: crate::PointResponse { x: 300, y: 2 },
            note: "CLI does not click automatically".to_string(),
        }),
        req_id: None,
        reco_id: None,
        env_resolved: Vec::new(),
        needs_detection: None,
    }
}

fn record_env_resolved(
    ledger: &mut SemanticLedgerContext,
    command: &str,
    values: &[EnvResolved],
) -> LabResult<()> {
    if values.is_empty() {
        return Ok(());
    }
    ledger.record_drive(json!({
        "stage": "env_resolved",
        "command": command,
        "keys": values
    }))
}

fn record_env_needs_detection(
    ledger: &mut SemanticLedgerContext,
    command: &str,
    reason: &str,
    subject: &str,
    values: &[EnvResolved],
) -> LabResult<()> {
    if let Some(needs) = needs_detection(command, reason, subject, values) {
        ledger.record_drive(json!({
            "stage": "env_needs_detection",
            "command": command,
            "needs_detection": needs
        }))?;
    }
    Ok(())
}

fn attach_env_details(
    details: &mut Value,
    command: &str,
    reason: &str,
    subject: &str,
    values: &[EnvResolved],
) -> LabResult<()> {
    let Some(object) = details.as_object_mut() else {
        return Err(LabError::device(
            "target error details must serialize as an object",
        ));
    };
    if !values.is_empty() {
        object.insert(
            "env_resolved".to_string(),
            serde_json::to_value(values).map_err(|error| {
                LabError::device(format!("failed to serialize resolved env details: {error}"))
            })?,
        );
    }
    if let Some(needs) = needs_detection(command, reason, subject, values) {
        object.insert(
            "needs_detection".to_string(),
            serde_json::to_value(needs).map_err(|error| {
                LabError::device(format!(
                    "failed to serialize needs-detection details: {error}"
                ))
            })?,
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests;
