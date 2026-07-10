// SPDX-License-Identifier: AGPL-3.0-only

use crate::readonly::{
    detect_current_page, load_evaluator, load_page_detector, needs_detection, recognition_scene,
    rect_response, target_evaluation_response,
};
use crate::{
    Clock, InputBackendFactory, InputBackendObservation, InputBackendRequest, Lab, LabPorts,
    SemanticLedgerContext,
};
use actingcommand_contract::{EnvResolved, LabError, LabResult};
use actingcommand_device::{TouchBackendConfig, combine_operation_and_close};
use actingcommand_ledger::IdKind;
use actingcommand_page_detector::PageDetector;
use actingcommand_recognition_pack::{
    PackRect, RecognitionEvaluator, TargetEvaluation, TargetKind,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
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
            let touch_config = required_touch_config(request.touch_config.as_ref())?;
            let input = SemanticInput::Tap { rect: click, point };
            let device = send_semantic_input(self, &touch_config, &input)?;
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
        let navigation_path = request.navigation_path.clone()?;
        let graph = load_navigation_graph(&navigation_path)?;
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

        let target_page = canonical_navigation_page(&graph, &request.to);
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

        let route =
            find_navigation_route(&graph.edges, &start.page, &target_page).ok_or_else(|| {
                LabError::usage(format!(
                    "no navigation route from '{}' to '{}'",
                    start.page, target_page
                ))
            })?;
        for edge in &route {
            if !request.allow_destructive {
                reject_dangerous_semantic_id("navigation edge", &edge.id)?;
                reject_destructive_overlap(edge, &graph.destructive_clicks)?;
            }
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

        let touch_config = required_touch_config(request.touch_config.as_ref())?;
        let step_timeout = required_duration(request.step_timeout.as_ref(), "step timeout")?;
        let poll = required_duration(request.poll.as_ref(), "poll interval")?;
        let mut execution = NavigationExecutionContext {
            lab: self,
            input: &mut request.input,
            evaluator: &evaluator,
            detector: &detector,
            destructive_clicks: &graph.destructive_clicks,
            touch_config: &touch_config,
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

#[derive(Debug, Clone)]
enum SemanticInput {
    Tap {
        rect: PackRect,
        point: crate::PointResponse,
    },
    TargetCenter {
        target_id: String,
    },
    Drag {
        from_rect: PackRect,
        to_rect: PackRect,
        from: crate::PointResponse,
        to: crate::PointResponse,
        duration_ms: u64,
    },
}

#[derive(Debug)]
struct NavigationGraph {
    game: Option<String>,
    edges: Vec<NavigationEdge>,
    destructive_clicks: Vec<DestructiveClick>,
    _control_points: Vec<String>,
}

#[derive(Debug, Clone)]
struct NavigationEdge {
    id: String,
    from_page: String,
    to_page: String,
    input: SemanticInput,
    source: Option<String>,
}

#[derive(Debug, Clone)]
struct DestructiveClick {
    page: Option<String>,
    rect: PackRect,
}

fn load_navigation_graph(path: &std::path::Path) -> LabResult<NavigationGraph> {
    let text = fs::read_to_string(path)
        .map_err(|error| LabError::usage(format!("failed to read {}: {error}", path.display())))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|error| LabError::usage(format!("failed to parse {}: {error}", path.display())))?;
    let game = value
        .get("game")
        .and_then(Value::as_str)
        .map(str::to_string);
    let edges = value
        .get("navigation")
        .and_then(Value::as_array)
        .ok_or_else(|| LabError::usage("navigation file is missing navigation[]"))?
        .iter()
        .map(parse_navigation_edge)
        .collect::<LabResult<Vec<_>>>()?;
    let destructive_clicks = value
        .get("destructive_actions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(parse_destructive_click)
        .collect::<LabResult<Vec<_>>>()?;
    let control_points = value
        .get("control_points")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(parse_control_point)
        .collect::<LabResult<Vec<_>>>()?;
    Ok(NavigationGraph {
        game,
        edges,
        destructive_clicks,
        _control_points: control_points,
    })
}

fn parse_control_point(value: &Value) -> LabResult<String> {
    let name = required_string_field(value, "name")?.to_string();
    if let Some(click) = value.get("click") {
        parse_navigation_input(click)?;
    } else {
        let rect = parse_control_point_rect(value)?;
        rect_center(rect)?;
    }
    if value.get("note").is_some_and(|note| !note.is_string()) {
        return Err(LabError::usage("field 'note' must be a string"));
    }
    Ok(name)
}

fn parse_control_point_rect(value: &Value) -> LabResult<PackRect> {
    if let Some(point) = value.get("point") {
        let (x, y) = parse_point_value(point)?;
        return Ok(PackRect {
            x,
            y,
            width: 1,
            height: 1,
        });
    }
    Ok(PackRect {
        x: required_i32_value(value, "x")?,
        y: required_i32_value(value, "y")?,
        width: 1,
        height: 1,
    })
}

fn parse_destructive_click(value: &Value) -> LabResult<DestructiveClick> {
    let click = value
        .get("click")
        .ok_or_else(|| LabError::usage("destructive action is missing click"))?;
    Ok(DestructiveClick {
        page: value
            .get("page")
            .and_then(Value::as_str)
            .map(str::to_string),
        rect: parse_navigation_tap_rect(click)?,
    })
}

fn parse_navigation_edge(value: &Value) -> LabResult<NavigationEdge> {
    Ok(NavigationEdge {
        id: required_string_field(value, "id")?.to_string(),
        from_page: required_string_field(value, "from_page")?.to_string(),
        to_page: required_string_field(value, "to_page")?.to_string(),
        input: parse_navigation_input(required_value_field(value, "click")?)?,
        source: value
            .get("source")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn parse_navigation_input(value: &Value) -> LabResult<SemanticInput> {
    match value.get("kind").and_then(Value::as_str) {
        Some("point") | Some("rect") => {
            let rect = parse_navigation_tap_rect(value)?;
            Ok(SemanticInput::Tap {
                rect,
                point: rect_center(rect)?,
            })
        }
        Some("target") | Some("target_center") => Ok(SemanticInput::TargetCenter {
            target_id: required_string_field(value, "target_id")?.to_string(),
        }),
        Some("drag") => {
            let from_rect = parse_navigation_tap_rect(required_value_field(value, "from")?)?;
            let to_rect = parse_navigation_tap_rect(required_value_field(value, "to")?)?;
            let duration_ms = value
                .get("duration_ms")
                .and_then(Value::as_u64)
                .unwrap_or(500);
            Ok(SemanticInput::Drag {
                from_rect,
                to_rect,
                from: rect_center(from_rect)?,
                to: rect_center(to_rect)?,
                duration_ms,
            })
        }
        other => Err(LabError::usage(format!(
            "unsupported navigation click kind: {other:?}"
        ))),
    }
}

fn parse_navigation_tap_rect(value: &Value) -> LabResult<PackRect> {
    match value.get("kind").and_then(Value::as_str) {
        Some("point") => parse_navigation_point(value),
        Some("rect") | None => parse_navigation_rect(value),
        Some("drag") => Err(LabError::usage(
            "drag click cannot be used as a tap rectangle",
        )),
        other => Err(LabError::usage(format!(
            "unsupported navigation click kind for tap rect: {other:?}"
        ))),
    }
}

fn parse_navigation_point(value: &Value) -> LabResult<PackRect> {
    if let Some(point) = value.get("point") {
        let (x, y) = parse_point_value(point)?;
        return Ok(PackRect {
            x,
            y,
            width: 1,
            height: 1,
        });
    }
    Ok(PackRect {
        x: required_i32_value(value, "x")?,
        y: required_i32_value(value, "y")?,
        width: 1,
        height: 1,
    })
}

fn parse_navigation_rect(value: &Value) -> LabResult<PackRect> {
    Ok(PackRect {
        x: required_i32_value(value, "x")?,
        y: required_i32_value(value, "y")?,
        width: required_i32_value(value, "width")?,
        height: required_i32_value(value, "height")?,
    })
}

fn parse_point_value(value: &Value) -> LabResult<(i32, i32)> {
    if let Some(point) = value.as_str() {
        return parse_point_pair(point);
    }
    if let Some(items) = value.as_array() {
        if items.len() != 2 {
            return Err(LabError::usage("point array must have exactly two items"));
        }
        return Ok((
            parse_i32_json_value(&items[0], "point[0]")?,
            parse_i32_json_value(&items[1], "point[1]")?,
        ));
    }
    Err(LabError::usage("point must be a string x,y or [x,y] array"))
}

fn parse_point_pair(value: &str) -> LabResult<(i32, i32)> {
    let parts = value.split(',').map(str::trim).collect::<Vec<_>>();
    if parts.len() != 2 {
        return Err(LabError::usage(format!(
            "point must be formatted as x,y: {value}"
        )));
    }
    let x = parts[0].parse::<i32>().map_err(|error| {
        LabError::usage(format!("failed to parse point x '{}': {error}", parts[0]))
    })?;
    let y = parts[1].parse::<i32>().map_err(|error| {
        LabError::usage(format!("failed to parse point y '{}': {error}", parts[1]))
    })?;
    Ok((x, y))
}

fn required_value_field<'a>(value: &'a Value, name: &str) -> LabResult<&'a Value> {
    value
        .get(name)
        .ok_or_else(|| LabError::usage(format!("missing field '{name}'")))
}

fn required_string_field<'a>(value: &'a Value, name: &str) -> LabResult<&'a str> {
    required_value_field(value, name)?
        .as_str()
        .ok_or_else(|| LabError::usage(format!("field '{name}' must be a string")))
}

fn required_i32_value(value: &Value, name: &str) -> LabResult<i32> {
    parse_i32_json_value(required_value_field(value, name)?, name)
}

fn parse_i32_json_value(value: &Value, name: &str) -> LabResult<i32> {
    if let Some(value) = value.as_i64() {
        return i32::try_from(value)
            .map_err(|_| LabError::usage(format!("field '{name}' exceeds i32 range")));
    }
    Err(LabError::usage(format!(
        "field '{name}' must be an integer"
    )))
}

fn canonical_navigation_page(graph: &NavigationGraph, page: &str) -> String {
    if page.contains('/') {
        return page.to_string();
    }
    graph
        .game
        .as_ref()
        .map(|game| format!("{game}/{page}"))
        .unwrap_or_else(|| page.to_string())
}

fn find_navigation_route(
    edges: &[NavigationEdge],
    from_page: &str,
    to_page: &str,
) -> Option<Vec<NavigationEdge>> {
    let mut queue = VecDeque::from([from_page.to_string()]);
    let mut previous = BTreeMap::<String, (String, usize)>::new();
    let mut seen = BTreeSet::from([from_page.to_string()]);
    while let Some(page) = queue.pop_front() {
        if page == to_page {
            break;
        }
        for (index, edge) in edges.iter().enumerate() {
            if edge.from_page != page || seen.contains(&edge.to_page) {
                continue;
            }
            seen.insert(edge.to_page.clone());
            previous.insert(edge.to_page.clone(), (page.clone(), index));
            queue.push_back(edge.to_page.clone());
        }
    }
    if from_page != to_page && !previous.contains_key(to_page) {
        return None;
    }
    let mut route = Vec::new();
    let mut cursor = to_page.to_string();
    while cursor != from_page {
        let (previous_page, index) = previous.get(&cursor)?.clone();
        route.push(edges[index].clone());
        cursor = previous_page;
    }
    route.reverse();
    Some(route)
}

fn navigation_edge_response(
    edge: &NavigationEdge,
    action_id: Option<String>,
) -> crate::NavigationEdgeResponse {
    crate::NavigationEdgeResponse {
        id: edge.id.clone(),
        from_page: edge.from_page.clone(),
        to_page: edge.to_page.clone(),
        input: semantic_input_response(&edge.input),
        source: edge.source.clone(),
        action_id,
    }
}

fn semantic_input_response(input: &SemanticInput) -> crate::SemanticInputResponse {
    match input {
        SemanticInput::Tap { rect, point } => crate::SemanticInputResponse::Tap {
            rect: rect_response(*rect),
            point: *point,
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
            from: *from,
            to: *to,
            duration_ms: *duration_ms,
        },
    }
}

fn reject_destructive_overlap(
    edge: &NavigationEdge,
    destructive: &[DestructiveClick],
) -> LabResult<()> {
    reject_destructive_overlap_input(edge, &edge.input, destructive)
}

fn reject_destructive_overlap_input(
    edge: &NavigationEdge,
    input: &SemanticInput,
    destructive: &[DestructiveClick],
) -> LabResult<()> {
    for rect in semantic_input_rects(input) {
        if destructive.iter().any(|other| {
            other
                .page
                .as_deref()
                .is_none_or(|page| page == "any" || page == edge.from_page)
                && rects_intersect(rect, other.rect)
        }) {
            return Err(LabError::safety_blocked(
                "navigation_destructive_overlap",
                format!(
                    "navigation edge '{}' overlaps a destructive action region",
                    edge.id
                ),
                &["navigation_only"],
            ));
        }
    }
    Ok(())
}

fn semantic_input_rects(input: &SemanticInput) -> Vec<PackRect> {
    match input {
        SemanticInput::Tap { rect, .. } => vec![*rect],
        SemanticInput::TargetCenter { .. } => Vec::new(),
        SemanticInput::Drag {
            from_rect, to_rect, ..
        } => vec![*from_rect, *to_rect],
    }
}

fn rects_intersect(a: PackRect, b: PackRect) -> bool {
    let ax2 = a.x.saturating_add(a.width);
    let ay2 = a.y.saturating_add(a.height);
    let bx2 = b.x.saturating_add(b.width);
    let by2 = b.y.saturating_add(b.height);
    a.x < bx2 && ax2 > b.x && a.y < by2 && ay2 > b.y
}

fn reject_dangerous_semantic_id(label: &str, value: &str) -> LabResult<()> {
    let lower = value.to_ascii_lowercase();
    let dangerous = [
        "gacha",
        "shop",
        "purchase",
        "buy",
        "recruit",
        "construct",
        "retire",
        "delete",
        "decompose",
        "enhance",
        "refill",
        "paid",
        "premium",
        "exercise",
        "pvp",
    ];
    if dangerous.iter().any(|word| lower.contains(word)) {
        return Err(LabError::safety_blocked(
            "semantic_action_requires_destructive_opt_in",
            format!("{label} '{value}' looks destructive and requires --allow-destructive"),
            &["navigation_only"],
        ));
    }
    Ok(())
}

fn rect_center(rect: PackRect) -> LabResult<crate::PointResponse> {
    if rect.width <= 0 || rect.height <= 0 {
        return Err(LabError::usage(format!(
            "click rectangle must have positive dimensions: {}x{}",
            rect.width, rect.height
        )));
    }
    Ok(crate::PointResponse {
        x: rect.x + rect.width / 2,
        y: rect.y + rect.height / 2,
    })
}

pub fn derive_absolute_coordinate_rect_from_match(
    kind: &str,
    declared: PackRect,
    expected_rect: PackRect,
    matched_rect: PackRect,
) -> LabResult<PackRect> {
    let dx = matched_rect
        .x
        .checked_sub(expected_rect.x)
        .ok_or_else(|| LabError::package_invalid(format!("{kind} x delta overflow")))?;
    let dy = matched_rect
        .y
        .checked_sub(expected_rect.y)
        .ok_or_else(|| LabError::package_invalid(format!("{kind} y delta overflow")))?;
    Ok(PackRect {
        x: declared
            .x
            .checked_add(dx)
            .ok_or_else(|| LabError::package_invalid(format!("{kind} translated x overflow")))?,
        y: declared
            .y
            .checked_add(dy)
            .ok_or_else(|| LabError::package_invalid(format!("{kind} translated y overflow")))?,
        width: declared.width,
        height: declared.height,
    })
}

fn required_touch_config(
    config: Option<&LabResult<TouchBackendConfig>>,
) -> LabResult<TouchBackendConfig> {
    config
        .ok_or_else(|| LabError::device("touch backend configuration is missing"))?
        .clone()
}

fn required_duration(duration: Option<&LabResult<Duration>>, label: &str) -> LabResult<Duration> {
    duration
        .ok_or_else(|| LabError::device(format!("{label} is missing")))?
        .clone()
}

fn send_semantic_input<P: LabPorts>(
    lab: &mut Lab<P>,
    config: &TouchBackendConfig,
    input: &SemanticInput,
) -> LabResult<crate::SemanticDeviceResponse> {
    let observation = InputBackendObservation::default();
    let mut backend = lab.ports().input_factory().open(InputBackendRequest {
        config: config.clone(),
        observation: Some(observation.clone()),
    })?;
    let operation = match input {
        SemanticInput::Tap { point, .. } => backend.tap(point.x, point.y),
        SemanticInput::TargetCenter { .. } => {
            return Err(LabError::usage(
                "target_center semantic input must be resolved before device execution",
            ));
        }
        SemanticInput::Drag {
            from,
            to,
            duration_ms,
            ..
        } => backend.swipe(from.x, from.y, to.x, to.y, *duration_ms),
    };
    let close = backend.close();
    combine_operation_and_close(operation, close)
        .map_err(|error| LabError::device(error.to_string()))?;
    Ok(crate::SemanticDeviceResponse {
        report: observation.snapshot()?,
        control_mode: "semantic".to_string(),
        action: semantic_input_response(input),
    })
}

struct NavigationExecutionContext<'a, P: LabPorts> {
    lab: &'a mut Lab<P>,
    input: &'a mut crate::ReadonlyRecognitionInput,
    evaluator: &'a RecognitionEvaluator,
    detector: &'a PageDetector,
    destructive_clicks: &'a [DestructiveClick],
    touch_config: &'a TouchBackendConfig,
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
        if current_page != edge.from_page {
            return Err(LabError::safety_blocked(
                "navigation_page_drift",
                format!(
                    "navigation expected current page '{}' but last page was '{}'",
                    edge.from_page, current_page
                ),
                &["page_guard"],
            ));
        }
        let (input, recognition) = resolve_navigation_edge_input(context, &edge)?;
        reject_destructive_overlap_input(&edge, &input, context.destructive_clicks)?;
        let device = send_semantic_input(context.lab, context.touch_config, &input)?;
        let arrived = poll_for_page(context, &edge.to_page)?;
        if !arrived.matched {
            return Err(LabError::safety_blocked(
                "navigation_arrival_failed",
                format!(
                    "navigation edge '{}' did not arrive at '{}'; last page '{}'",
                    edge.id, edge.to_page, arrived.page
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
    let SemanticInput::TargetCenter { target_id } = &edge.input else {
        return Ok((edge.input.clone(), None));
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
                edge.id, target_id, evaluation.message
            ),
            &["visible_target", "navigation"],
        ));
    }
    let rect = target_evaluation_rect(&evaluation)?;
    Ok((
        SemanticInput::Tap {
            rect,
            point: rect_center(rect)?,
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
