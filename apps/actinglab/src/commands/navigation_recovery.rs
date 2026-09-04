use crate::{
    CaptureBackendChoice, CaptureFreshProbeReport, CaptureFreshProbeStatus,
    CaptureFreshnessExpectation, CliError, CliOutcome, FlagArgs, GlobalOptions, InputBackend,
    PackRect, PageDetector, PageEvaluation, RecognitionEvaluator, Scene, TargetEvaluation,
    UserConfig, VecDeque, canonical_game, canonical_server, capture_diagnosis_recovery_json,
    capture_for_command, capture_fresh_probe_report, capture_fresh_probe_report_json,
    combine_operation_and_close, device_config, drive_cli, effective_capture_backend_choice,
    effective_resource_root, env_detection, load_pack_from_json_str, load_page_set_from_json_str,
    match_metric_name, open_cli_runtime_input_proxy, parse_match_metric_flag,
    parse_optional_duration_ms, parse_optional_usize, read_user_config, readonly_cli,
    reject_legacy_session_routing, scene_from_frame,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
#[cfg(test)]
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

pub(crate) fn run_recognize(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    readonly_cli::run_recognize(global, args)
}

pub(crate) fn run_detect_page(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    readonly_cli::run_detect_page(global, args)
}

pub(crate) fn run_current_page(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    readonly_cli::run_current_page(global, args)
}

pub(crate) fn run_is_visible(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    readonly_cli::run_is_visible(global, args)
}

pub(crate) fn run_locate(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    reject_legacy_session_routing(&flags)?;
    let template = flags
        .optional_path("--template")
        .or_else(|| flags.positionals.first().map(PathBuf::from))
        .ok_or_else(|| CliError::usage("locate requires <template> or --template <path>"))?;
    let metric = parse_match_metric_flag(&flags)?;
    let scene = load_scene_from_flags(global, &flags)?;
    let template_png = fs::read(&template).map_err(|err| {
        CliError::device(format!(
            "failed to read template {}: {err}",
            template.display()
        ))
    })?;
    let matched = scene
        .match_template_with_metric(&template_png, None, metric)
        .map_err(|err| CliError::device(err.to_string()))?;
    Ok(json!({
        "template": template.display().to_string(),
        "x": matched.x,
        "y": matched.y,
        "score": matched.score,
        "raw_score": matched.raw_score,
        "match_metric": match_metric_name(metric)
    }))
}

pub(crate) fn run_tap_target(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    drive_cli::run_tap_target(global, args)
}

pub(crate) fn run_navigate(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    drive_cli::run_navigate(global, args)
}

pub(crate) fn run_session_recover(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    reject_legacy_session_routing(&flags)?;
    if flags.bool("--stale-capture") {
        return run_session_stale_capture_recover(global, &flags);
    }
    let dry_run = global.dry_run || flags.bool("--dry-run");
    if !dry_run && !flags.bool("--capture") {
        return Err(CliError::usage(
            "session recover real execution requires --capture; use --dry-run with --scene for offline planning",
        ));
    }

    let config = read_user_config()?;
    let (evaluator, detector) = load_semantic_detector(global, &config, &flags)?;
    let graph = load_navigation_graph(global, &config, &flags)?;
    let scene = load_scene_from_flags(global, &flags)?;
    let start = detect_current_page(&evaluator, &detector, &scene)?;
    let target_page = canonical_navigation_page(
        &graph,
        &flags
            .optional("--to")
            .filter(|value| value != "true")
            .unwrap_or_else(|| "home".to_string()),
    );
    let max_actions = parse_optional_usize(&flags, "--max-actions", 3)?;
    let step_timeout = parse_optional_duration_ms(&flags, "--step-timeout-ms", 5_000)?;
    let poll = parse_optional_duration_ms(&flags, "--poll-ms", 500)?;
    if flags.bool("--startup-login") {
        let startup_max_rounds = parse_optional_usize(&flags, "--startup-max-rounds", 25)?;
        let startup_interval = parse_optional_duration_ms(&flags, "--startup-interval-ms", 2_000)?;
        return run_session_startup_login_recover(StartupLoginRecovery {
            global,
            config: &config,
            flags: &flags,
            evaluator: &evaluator,
            detector: &detector,
            start,
            target_page,
            dry_run,
            max_rounds: startup_max_rounds,
            interval: startup_interval,
        });
    }

    if start.matched && start.page == target_page {
        return Ok(json!({
            "status": "already_at_target",
            "mode": "maintenance_recovery",
            "executed": false,
            "from": start.page,
            "to": target_page,
            "steps": []
        }));
    }

    if start.standby {
        let wake = graph.control_points.get("wake").ok_or_else(|| {
            CliError::safety_blocked(
                "wake_control_point_missing",
                "session recover detected standby but navigation resources do not define control_points.wake",
                &["control_point"],
            )
        })?;
        if max_actions == 0 {
            return Err(CliError::safety_blocked(
                "recovery_action_limit_exceeded",
                "session recover requires one wake action but --max-actions is 0",
                &["maintenance_recovery"],
            ));
        }
        if dry_run {
            return Ok(json!({
                "status": "planned",
                "mode": "maintenance_recovery",
                "executed": false,
                "from": "standby",
                "to": target_page,
                "steps": [{
                    "type": "wake",
                    "control_point": control_point_json(wake)
                }],
                "next": "rerun after wake to detect the current page and route to the target if needed"
            }));
        }

        let device = send_semantic_input(global, &config, &wake.input)?;
        let after_wake =
            poll_for_matched_page(global, &flags, &evaluator, &detector, step_timeout, poll)?;
        if !after_wake.matched {
            return Err(CliError::safety_blocked(
                "recovery_wake_failed",
                format!(
                    "wake control point did not produce a known page; last page '{}'",
                    after_wake.page
                ),
                &["maintenance_recovery"],
            ));
        }
        let mut steps = vec![json!({
            "type": "wake",
            "control_point": control_point_json(wake),
            "device": device,
            "arrived": page_detection_json(&after_wake)
        })];
        if after_wake.page == target_page {
            return Ok(json!({
                "status": "recovered",
                "mode": "maintenance_recovery",
                "executed": true,
                "from": "standby",
                "to": target_page,
                "steps": steps
            }));
        }
        let route = safe_recovery_route(&graph, &after_wake.page, &target_page)?;
        ensure_recovery_action_limit(1 + route.len(), max_actions)?;
        let execution = NavigationExecutionContext {
            global,
            flags: &flags,
            config: &config,
            evaluator: &evaluator,
            detector: &detector,
            destructive_clicks: &graph.destructive_clicks,
            step_timeout,
            poll,
        };
        let (mut route_steps, _) = execute_navigation_route(&execution, after_wake.page, route)?;
        steps.append(&mut route_steps);
        return Ok(json!({
            "status": "recovered",
            "mode": "maintenance_recovery",
            "executed": true,
            "from": "standby",
            "to": target_page,
            "steps": steps
        }));
    }

    let route = safe_recovery_route(&graph, &start.page, &target_page)?;
    ensure_recovery_action_limit(route.len(), max_actions)?;
    let route_json = route.iter().map(navigation_edge_json).collect::<Vec<_>>();
    if dry_run {
        return Ok(json!({
            "status": "planned",
            "mode": "maintenance_recovery",
            "executed": false,
            "from": start.page,
            "to": target_page,
            "route": route_json,
            "safety_gate": "maintenance_navigation_only"
        }));
    }

    let execution = NavigationExecutionContext {
        global,
        flags: &flags,
        config: &config,
        evaluator: &evaluator,
        detector: &detector,
        destructive_clicks: &graph.destructive_clicks,
        step_timeout,
        poll,
    };
    let (steps, _) = execute_navigation_route(&execution, start.page, route)?;
    Ok(json!({
        "status": "recovered",
        "mode": "maintenance_recovery",
        "executed": true,
        "to": target_page,
        "steps": steps,
        "safety_gate": "maintenance_navigation_only"
    }))
}

fn run_session_stale_capture_recover(
    global: &GlobalOptions,
    flags: &FlagArgs,
) -> CliOutcome<Value> {
    flags.expect_positionals("session recover --stale-capture", 0)?;
    let config = read_user_config()?;
    let instance = global
        .instance
        .as_ref()
        .and_then(|instance_id| config.instances.get(instance_id));
    let requested = effective_capture_backend_choice(
        global,
        global.instance.as_deref().unwrap_or("default"),
        instance,
    )?;
    let fresh_delay = parse_optional_duration_ms(flags, "--fresh-delay-ms", 160)?;
    let run_diagnosis = flags.bool("--capture") || flags.bool("--diagnose");
    if run_diagnosis {
        let device_config = device_config(global, &config)?;
        let report = capture_fresh_probe_report(
            &device_config,
            requested,
            fresh_delay,
            CaptureFreshnessExpectation::ExpectedChange,
        )?;
        return Ok(stale_capture_recovery_json(
            requested,
            fresh_delay,
            Some(&report),
        ));
    }
    Ok(stale_capture_recovery_json(requested, fresh_delay, None))
}

pub(crate) fn stale_capture_recovery_json(
    requested: CaptureBackendChoice,
    fresh_delay: Duration,
    report: Option<&CaptureFreshProbeReport>,
) -> Value {
    let diagnosis = report.map_or_else(
        || {
            json!({
                "executed": false,
                "command": format!(
                    "capture diagnose --capture-backend {} --fresh-delay-ms {}",
                    requested.as_str(),
                    fresh_delay.as_millis()
                ),
                "read_only": true,
                "reason": "verify fresh frames before treating an unchanged screen as a game freeze"
            })
        },
        |report| {
            json!({
                "executed": true,
                "read_only": true,
                "result": capture_fresh_probe_report_json(report, requested)
            })
        },
    );
    let status = report
        .map(|report| match report.status {
            CaptureFreshProbeStatus::Fresh | CaptureFreshProbeStatus::StaticUnchanged => {
                "diagnosed_fresh"
            }
            CaptureFreshProbeStatus::StaleSuspected => "diagnosed_stale",
        })
        .unwrap_or("planned");
    let recovery_status = report
        .map(|report| report.status)
        .unwrap_or(CaptureFreshProbeStatus::StaleSuspected);
    json!({
        "status": status,
        "mode": "stale_capture_recovery",
        "executed": false,
        "click_allowed": false,
        "app_restart_executed": false,
        "diagnosis_executed": report.is_some(),
        "diagnosis_status": status,
        "requested_backend": requested.as_str(),
        "fresh_delay_ms": fresh_delay.as_millis(),
        "diagnosis": diagnosis,
        "recovery": capture_diagnosis_recovery_json(recovery_status, requested),
        "steps": [
            {
                "order": 1,
                "type": "fresh_probe",
                "command": format!(
                    "capture diagnose --capture-backend {} --fresh-delay-ms {}",
                    requested.as_str(),
                    fresh_delay.as_millis()
                ),
                "read_only": true
            },
            {
                "order": 2,
                "type": "capture_backend",
                "backend": "nemu_ipc",
                "reason": "try MuMu IPC before restarting the game"
            },
            {
                "order": 3,
                "type": "capture_backend",
                "backend": "droidcast_raw",
                "reason": "try alternate capture surface before restarting the game"
            },
            {
                "order": 4,
                "type": "app_restart",
                "command": "session app restart",
                "requires_lease": true,
                "heavy_recovery": true,
                "reason": "last resort after capture-backend recovery checks fail"
            }
        ],
        "safety_gate": "diagnose_capture_backend_before_restart",
        "next": "run capture diagnose with the effective backend selection; only restart the app if lighter capture-backend recovery cannot restore fresh frames"
    })
}

#[derive(Debug)]
pub(crate) struct PageDetectionOutcome {
    pub(crate) page: String,
    pub(crate) matched: bool,
    pub(crate) standby: bool,
    pub(crate) evaluations: Vec<PageEvaluation>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SemanticPoint {
    pub(crate) x: i32,
    pub(crate) y: i32,
}

#[derive(Debug, Clone)]
pub(crate) enum SemanticInput {
    Tap {
        rect: PackRect,
        point: SemanticPoint,
    },
    TargetCenter {
        target_id: String,
    },
    Drag {
        from_rect: PackRect,
        to_rect: PackRect,
        from: SemanticPoint,
        to: SemanticPoint,
        duration_ms: u64,
    },
}

#[derive(Debug)]
pub(crate) struct NavigationGraph {
    game: Option<String>,
    pub(crate) edges: Vec<NavigationEdge>,
    pub(crate) destructive_clicks: Vec<DestructiveClick>,
    control_points: BTreeMap<String, ControlPoint>,
}

#[derive(Debug, Clone)]
pub(crate) struct NavigationEdge {
    pub(crate) id: String,
    pub(crate) from_page: String,
    pub(crate) to_page: String,
    pub(crate) input: SemanticInput,
    pub(crate) source: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct DestructiveClick {
    pub(crate) page: Option<String>,
    pub(crate) rect: PackRect,
}

#[derive(Debug, Clone)]
pub(crate) struct ControlPoint {
    name: String,
    input: SemanticInput,
    note: Option<String>,
}

fn load_semantic_detector(
    global: &GlobalOptions,
    config: &UserConfig,
    flags: &FlagArgs,
) -> CliOutcome<(RecognitionEvaluator, PageDetector)> {
    let (evaluator, detector, _) = load_semantic_detector_with_env(global, config, flags)?;
    Ok((evaluator, detector))
}

fn load_semantic_detector_with_env(
    global: &GlobalOptions,
    config: &UserConfig,
    flags: &FlagArgs,
) -> CliOutcome<(
    RecognitionEvaluator,
    PageDetector,
    Vec<env_detection::ResolvedEnvValue>,
)> {
    let resources = recognition_resources(global, config, flags, true)?;
    let pages_path = resources.pages_path.as_ref().ok_or_else(|| {
        CliError::usage("semantic page commands require --pages or --resource-root --game")
    })?;
    let (evaluator, detector, env_resolved) = load_evaluator_and_detector_with_env(
        global,
        flags,
        &resources.pack_path,
        &resources.pack_root,
        pages_path,
    )?;
    detector
        .validate(&evaluator)
        .map_err(|err| CliError::usage(err.to_string()))?;
    Ok((evaluator, detector, env_resolved))
}

pub(crate) fn detect_current_page(
    evaluator: &RecognitionEvaluator,
    detector: &PageDetector,
    scene: &Scene,
) -> CliOutcome<PageDetectionOutcome> {
    let evaluations = detector
        .evaluate_all(evaluator, scene)
        .map_err(|err| CliError::usage(err.to_string()))?;
    if let Some(match_eval) = evaluations.iter().find(|evaluation| evaluation.matched) {
        return Ok(PageDetectionOutcome {
            page: match_eval.page_id.clone(),
            matched: true,
            standby: false,
            evaluations,
        });
    }
    Ok(PageDetectionOutcome {
        page: "standby".to_string(),
        matched: false,
        standby: true,
        evaluations,
    })
}

pub(crate) fn page_detection_json(outcome: &PageDetectionOutcome) -> Value {
    let mut data = json!({
        "page": outcome.page,
        "matched": outcome.matched,
        "standby": outcome.standby,
        "evaluations": outcome.evaluations.iter().map(page_eval_json).collect::<Vec<_>>()
    });
    if outcome.standby {
        data["recovery_hint"] = json!({
            "action": "wake_safe_point",
            "point": {"x": 300, "y": 2},
            "note": "CLI does not click automatically"
        });
    }
    data
}

pub(crate) fn target_eval_json(evaluation: &TargetEvaluation) -> Value {
    json!({
        "target": evaluation.id,
        "kind": format!("{:?}", evaluation.kind),
        "passed": evaluation.passed,
        "message": evaluation.message,
        "matched_rect": evaluation.template.map(|template| rect_json(PackRect {
            x: template.x,
            y: template.y,
            width: template.width,
            height: template.height
        })),
        "template": evaluation.template.map(|template| {
            json!({
                "x": template.x,
                "y": template.y,
                "width": template.width,
                "height": template.height,
                "score": template.score,
                "raw_score": template.raw_score,
                "threshold": template.threshold
            })
        }),
        "color": evaluation.color.map(|color| {
            json!({
                "distance": color.distance,
                "max_distance": color.max_distance,
                "mean": color.mean,
                "expected": color.expected
            })
        })
    })
}

fn load_navigation_graph(
    global: &GlobalOptions,
    config: &UserConfig,
    flags: &FlagArgs,
) -> CliOutcome<NavigationGraph> {
    let path = navigation_path(global, config, flags)?;
    let text = fs::read_to_string(&path)
        .map_err(|err| CliError::usage(format!("failed to read {}: {err}", path.display())))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|err| CliError::usage(format!("failed to parse {}: {err}", path.display())))?;
    parse_navigation_graph_value(&value)
}

pub(crate) fn parse_navigation_graph_value(value: &Value) -> CliOutcome<NavigationGraph> {
    let game = value
        .get("game")
        .and_then(Value::as_str)
        .map(str::to_string);
    let edges = value
        .get("navigation")
        .and_then(Value::as_array)
        .ok_or_else(|| CliError::usage("navigation file is missing navigation[]"))?
        .iter()
        .map(parse_navigation_edge)
        .collect::<CliOutcome<Vec<_>>>()?;
    let destructive_clicks = value
        .get("destructive_actions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(parse_destructive_click)
        .collect::<CliOutcome<Vec<_>>>()?;
    let control_points = value
        .get("control_points")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(parse_control_point)
        .map(|result| result.map(|point| (point.name.clone(), point)))
        .collect::<CliOutcome<BTreeMap<_, _>>>()?;
    Ok(NavigationGraph {
        game,
        edges,
        destructive_clicks,
        control_points,
    })
}

pub(crate) fn parse_control_point(value: &Value) -> CliOutcome<ControlPoint> {
    let name = required_string_field(value, "name")?.to_string();
    let input = if let Some(click) = value.get("click") {
        parse_navigation_input(click)?
    } else {
        let rect = parse_control_point_rect(value)?;
        SemanticInput::Tap {
            rect,
            point: rect_center(rect)?,
        }
    };
    Ok(ControlPoint {
        name,
        input,
        note: value
            .get("note")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn parse_control_point_rect(value: &Value) -> CliOutcome<PackRect> {
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

fn parse_destructive_click(value: &Value) -> CliOutcome<DestructiveClick> {
    let click = value
        .get("click")
        .ok_or_else(|| CliError::usage("destructive action is missing click"))?;
    Ok(DestructiveClick {
        page: value
            .get("page")
            .and_then(Value::as_str)
            .map(str::to_string),
        rect: parse_navigation_tap_rect(click)?,
    })
}

fn navigation_path(
    global: &GlobalOptions,
    config: &UserConfig,
    flags: &FlagArgs,
) -> CliOutcome<PathBuf> {
    if let Some(path) = flags.optional_path("--navigation") {
        return Ok(path);
    }
    let root = effective_resource_root(global, config).ok_or_else(|| {
        CliError::usage("navigate requires --navigation or --resource-root with --game")
    })?;
    let (game, server) = recognition_selector(global)?;
    Ok(root
        .join("navigation")
        .join(format!("{game}.{server}.navigation.json")))
}

pub(crate) fn parse_navigation_edge(value: &Value) -> CliOutcome<NavigationEdge> {
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

pub(crate) fn parse_navigation_input(value: &Value) -> CliOutcome<SemanticInput> {
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
        other => Err(CliError::usage(format!(
            "unsupported navigation click kind: {other:?}"
        ))),
    }
}

pub(crate) fn parse_navigation_tap_rect(value: &Value) -> CliOutcome<PackRect> {
    match value.get("kind").and_then(Value::as_str) {
        Some("point") => parse_navigation_point(value),
        Some("rect") | None => parse_navigation_rect(value),
        Some("drag") => Err(CliError::usage(
            "drag click cannot be used as a tap rectangle",
        )),
        other => Err(CliError::usage(format!(
            "unsupported navigation click kind for tap rect: {other:?}"
        ))),
    }
}

fn parse_navigation_point(value: &Value) -> CliOutcome<PackRect> {
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

fn parse_navigation_rect(value: &Value) -> CliOutcome<PackRect> {
    Ok(PackRect {
        x: required_i32_value(value, "x")?,
        y: required_i32_value(value, "y")?,
        width: required_i32_value(value, "width")?,
        height: required_i32_value(value, "height")?,
    })
}

fn parse_point_value(value: &Value) -> CliOutcome<(i32, i32)> {
    if let Some(point) = value.as_str() {
        return parse_point_pair(point);
    }
    if let Some(items) = value.as_array() {
        if items.len() != 2 {
            return Err(CliError::usage("point array must have exactly two items"));
        }
        return Ok((
            parse_i32_json_value(&items[0], "point[0]")?,
            parse_i32_json_value(&items[1], "point[1]")?,
        ));
    }
    Err(CliError::usage("point must be a string x,y or [x,y] array"))
}

pub(crate) fn parse_point_pair(value: &str) -> CliOutcome<(i32, i32)> {
    let parts = value.split(',').map(str::trim).collect::<Vec<_>>();
    if parts.len() != 2 {
        return Err(CliError::usage(format!(
            "point must be formatted as x,y: {value}"
        )));
    }
    let x = parts[0]
        .parse::<i32>()
        .map_err(|err| CliError::usage(format!("failed to parse point x '{}': {err}", parts[0])))?;
    let y = parts[1]
        .parse::<i32>()
        .map_err(|err| CliError::usage(format!("failed to parse point y '{}': {err}", parts[1])))?;
    Ok((x, y))
}

pub(crate) fn required_value_field<'a>(value: &'a Value, name: &str) -> CliOutcome<&'a Value> {
    value
        .get(name)
        .ok_or_else(|| CliError::usage(format!("missing field '{name}'")))
}

pub(crate) fn required_string_field<'a>(value: &'a Value, name: &str) -> CliOutcome<&'a str> {
    required_value_field(value, name)?
        .as_str()
        .ok_or_else(|| CliError::usage(format!("field '{name}' must be a string")))
}

fn required_i32_value(value: &Value, name: &str) -> CliOutcome<i32> {
    parse_i32_json_value(required_value_field(value, name)?, name)
}

fn parse_i32_json_value(value: &Value, name: &str) -> CliOutcome<i32> {
    if let Some(value) = value.as_i64() {
        return i32::try_from(value)
            .map_err(|_| CliError::usage(format!("field '{name}' exceeds i32 range")));
    }
    Err(CliError::usage(format!(
        "field '{name}' must be an integer"
    )))
}

pub(crate) fn canonical_navigation_page(graph: &NavigationGraph, page: &str) -> String {
    if page.contains('/') {
        return page.to_string();
    }
    graph
        .game
        .as_ref()
        .map(|game| format!("{game}/{page}"))
        .unwrap_or_else(|| page.to_string())
}

pub(crate) fn find_navigation_route(
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
        let (prev, index) = previous.get(&cursor)?.clone();
        route.push(edges[index].clone());
        cursor = prev;
    }
    route.reverse();
    Some(route)
}

pub(crate) fn navigation_edge_json(edge: &NavigationEdge) -> Value {
    json!({
        "id": edge.id,
        "from_page": edge.from_page,
        "to_page": edge.to_page,
        "input": semantic_input_json(&edge.input),
        "source": edge.source
    })
}

fn control_point_json(point: &ControlPoint) -> Value {
    json!({
        "name": point.name,
        "input": semantic_input_json(&point.input),
        "note": point.note
    })
}

pub(crate) fn semantic_input_json(input: &SemanticInput) -> Value {
    match input {
        SemanticInput::Tap { rect, point } => json!({
            "type": "tap",
            "rect": rect_json(*rect),
            "point": point_json(*point)
        }),
        SemanticInput::TargetCenter { target_id } => json!({
            "type": "target_center",
            "target_id": target_id
        }),
        SemanticInput::Drag {
            from_rect,
            to_rect,
            from,
            to,
            duration_ms,
        } => json!({
            "type": "drag",
            "from_rect": rect_json(*from_rect),
            "to_rect": rect_json(*to_rect),
            "from": point_json(*from),
            "to": point_json(*to),
            "duration_ms": duration_ms
        }),
    }
}

pub(crate) fn reject_destructive_overlap(
    edge: &NavigationEdge,
    destructive: &[DestructiveClick],
) -> CliOutcome<()> {
    reject_destructive_overlap_input(edge, &edge.input, destructive)
}

pub(crate) fn reject_destructive_overlap_input(
    edge: &NavigationEdge,
    input: &SemanticInput,
    destructive: &[DestructiveClick],
) -> CliOutcome<()> {
    let rects = semantic_input_rects(input);
    for rect in rects {
        if destructive.iter().any(|other| {
            other
                .page
                .as_deref()
                .is_none_or(|page| page == "any" || page == edge.from_page)
                && rects_intersect(rect, other.rect)
        }) {
            return Err(CliError::safety_blocked(
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

fn safe_recovery_route(
    graph: &NavigationGraph,
    from_page: &str,
    to_page: &str,
) -> CliOutcome<Vec<NavigationEdge>> {
    let route = find_navigation_route(&graph.edges, from_page, to_page).ok_or_else(|| {
        CliError::safety_blocked(
            "recovery_route_missing",
            format!("no maintenance recovery route from '{from_page}' to '{to_page}'"),
            &["maintenance_recovery"],
        )
    })?;
    for edge in &route {
        reject_dangerous_semantic_id("recovery navigation edge", &edge.id)?;
        reject_destructive_overlap(edge, &graph.destructive_clicks)?;
    }
    Ok(route)
}

struct StartupLoginPlan {
    source: PathBuf,
    target_page: String,
    max_rounds: usize,
    interval: Duration,
    close_popup: SemanticInput,
    continue_input: SemanticInput,
}

struct StartupLoginRecovery<'a> {
    global: &'a GlobalOptions,
    config: &'a UserConfig,
    flags: &'a FlagArgs,
    evaluator: &'a RecognitionEvaluator,
    detector: &'a PageDetector,
    start: PageDetectionOutcome,
    target_page: String,
    dry_run: bool,
    max_rounds: usize,
    interval: Duration,
}

fn run_session_startup_login_recover(ctx: StartupLoginRecovery<'_>) -> CliOutcome<Value> {
    let plan = load_startup_login_plan(
        ctx.global,
        ctx.config,
        ctx.flags,
        ctx.target_page.clone(),
        ctx.max_rounds,
        ctx.interval,
    )?;
    if ctx.start.matched && ctx.start.page == ctx.target_page {
        return Ok(json!({
            "status": "already_at_target",
            "mode": "startup_login_recovery",
            "executed": false,
            "from": ctx.start.page,
            "to": ctx.target_page,
            "startup_login": startup_login_plan_json(&plan),
            "steps": []
        }));
    }
    if ctx.dry_run {
        return Ok(json!({
            "status": "planned",
            "mode": "startup_login_recovery",
            "executed": false,
            "from": page_detection_json(&ctx.start),
            "to": ctx.target_page,
            "startup_login": startup_login_plan_json(&plan),
            "round_plan": startup_login_round_json(&plan, 1),
            "repeat_until": "target_page_or_max_rounds",
            "safety_gate": "maintenance_login_only"
        }));
    }

    let mut steps = Vec::new();
    let mut last = ctx.start;
    for round in 1..=plan.max_rounds {
        let close_device = send_semantic_input(ctx.global, ctx.config, &plan.close_popup)?;
        let continue_device = send_semantic_input(ctx.global, ctx.config, &plan.continue_input)?;
        thread::sleep(plan.interval);
        let scene = load_scene_from_flags(ctx.global, ctx.flags)?;
        last = detect_current_page(ctx.evaluator, ctx.detector, &scene)?;
        steps.push(json!({
            "round": round,
            "actions": [
                {
                    "name": "close_popup",
                    "input": semantic_input_json(&plan.close_popup),
                    "device": close_device
                },
                {
                    "name": "continue",
                    "input": semantic_input_json(&plan.continue_input),
                    "device": continue_device
                }
            ],
            "arrived": page_detection_json(&last)
        }));
        if last.matched && last.page == plan.target_page {
            return Ok(json!({
                "status": "recovered",
                "mode": "startup_login_recovery",
                "executed": true,
                "to": plan.target_page,
                "startup_login": startup_login_plan_json(&plan),
                "steps": steps,
                "safety_gate": "maintenance_login_only"
            }));
        }
    }

    Err(CliError::safety_blocked(
        "startup_login_recovery_failed",
        format!(
            "startup-login recovery did not reach '{}' within {} rounds; last page '{}'",
            plan.target_page, plan.max_rounds, last.page
        ),
        &["maintenance_recovery", "startup_login"],
    ))
}

fn load_startup_login_plan(
    global: &GlobalOptions,
    config: &UserConfig,
    flags: &FlagArgs,
    target_page: String,
    max_rounds: usize,
    interval: Duration,
) -> CliOutcome<StartupLoginPlan> {
    if max_rounds == 0 {
        return Err(CliError::safety_blocked(
            "startup_login_round_limit_zero",
            "startup-login recovery requires --startup-max-rounds greater than 0",
            &["maintenance_recovery", "startup_login"],
        ));
    }
    let source = flags.optional_path("--startup-login-file").map(Ok).unwrap_or_else(|| {
        effective_resource_root(global, config)
            .map(|root| root.join("STARTUP-LOGIN.md"))
            .ok_or_else(|| {
                CliError::usage(
                    "session recover --startup-login requires --resource-root or --startup-login-file",
                )
            })
    })?;
    let text = fs::read_to_string(&source).map_err(|err| {
        CliError::safety_blocked(
            "startup_login_resource_missing",
            format!(
                "failed to read startup-login resource {}: {err}",
                source.display()
            ),
            &["maintenance_recovery", "startup_login_resource"],
        )
    })?;
    Ok(StartupLoginPlan {
        source,
        target_page,
        max_rounds,
        interval,
        close_popup: semantic_tap_input(find_coordinate_by_anchors(
            &text,
            &["弹窗关闭", "关闭 ×", "关闭", "close"],
            "popup close",
        )?),
        continue_input: semantic_tap_input(find_coordinate_by_anchors(
            &text,
            &[
                "推进/点击继续",
                "点击继续",
                "屏幕中心",
                "tap 中心",
                "continue",
            ],
            "continue",
        )?),
    })
}

fn find_coordinate_by_anchors(
    text: &str,
    anchors: &[&str],
    label: &str,
) -> CliOutcome<SemanticPoint> {
    for line in text.lines() {
        if anchors.iter().any(|anchor| line.contains(anchor))
            && let Some(point) = parse_parenthesized_point(line)?
        {
            return Ok(point);
        }
    }
    Err(CliError::safety_blocked(
        "startup_login_coordinate_missing",
        format!("startup-login resource is missing the {label} coordinate"),
        &["maintenance_recovery", "startup_login_resource"],
    ))
}

fn parse_parenthesized_point(line: &str) -> CliOutcome<Option<SemanticPoint>> {
    let mut rest = line;
    while let Some(start) = rest.find('(') {
        let after_start = &rest[start + 1..];
        let Some(end) = after_start.find(')') else {
            return Ok(None);
        };
        let candidate = &after_start[..end];
        if let Some((x, y)) = candidate.split_once(',') {
            let x = x.trim().parse::<i32>().map_err(|err| {
                CliError::safety_blocked(
                    "startup_login_coordinate_invalid",
                    format!("invalid startup-login coordinate x '{}': {err}", x.trim()),
                    &["maintenance_recovery", "startup_login_resource"],
                )
            })?;
            let y = y.trim().parse::<i32>().map_err(|err| {
                CliError::safety_blocked(
                    "startup_login_coordinate_invalid",
                    format!("invalid startup-login coordinate y '{}': {err}", y.trim()),
                    &["maintenance_recovery", "startup_login_resource"],
                )
            })?;
            if x < 0 || y < 0 {
                return Err(CliError::safety_blocked(
                    "startup_login_coordinate_invalid",
                    "startup-login coordinates must be non-negative",
                    &["maintenance_recovery", "startup_login_resource"],
                ));
            }
            return Ok(Some(SemanticPoint { x, y }));
        }
        rest = &after_start[end + 1..];
    }
    Ok(None)
}

fn semantic_tap_input(point: SemanticPoint) -> SemanticInput {
    SemanticInput::Tap {
        rect: PackRect {
            x: point.x,
            y: point.y,
            width: 1,
            height: 1,
        },
        point,
    }
}

fn startup_login_plan_json(plan: &StartupLoginPlan) -> Value {
    json!({
        "source": plan.source.display().to_string(),
        "target_page": plan.target_page,
        "max_rounds": plan.max_rounds,
        "interval_ms": plan.interval.as_millis(),
        "actions_per_round": [
            {
                "name": "close_popup",
                "input": semantic_input_json(&plan.close_popup)
            },
            {
                "name": "continue",
                "input": semantic_input_json(&plan.continue_input)
            }
        ]
    })
}

fn startup_login_round_json(plan: &StartupLoginPlan, round: usize) -> Value {
    json!({
        "round": round,
        "actions": [
            {
                "name": "close_popup",
                "input": semantic_input_json(&plan.close_popup)
            },
            {
                "name": "continue",
                "input": semantic_input_json(&plan.continue_input)
            }
        ]
    })
}

fn ensure_recovery_action_limit(actions: usize, max_actions: usize) -> CliOutcome<()> {
    if actions > max_actions {
        return Err(CliError::safety_blocked(
            "recovery_action_limit_exceeded",
            format!("session recover planned {actions} actions but --max-actions is {max_actions}"),
            &["maintenance_recovery"],
        ));
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

pub(crate) fn rects_intersect(a: PackRect, b: PackRect) -> bool {
    let ax2 = a.x.saturating_add(a.width);
    let ay2 = a.y.saturating_add(a.height);
    let bx2 = b.x.saturating_add(b.width);
    let by2 = b.y.saturating_add(b.height);
    a.x < bx2 && ax2 > b.x && a.y < by2 && ay2 > b.y
}

pub(crate) fn reject_dangerous_semantic_id(label: &str, value: &str) -> CliOutcome<()> {
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
        return Err(CliError::safety_blocked(
            "semantic_action_requires_destructive_opt_in",
            format!("{label} '{value}' looks destructive and requires --allow-destructive"),
            &["navigation_only"],
        ));
    }
    Ok(())
}

pub(crate) fn rect_center(rect: PackRect) -> CliOutcome<SemanticPoint> {
    if rect.width <= 0 || rect.height <= 0 {
        return Err(CliError::usage(format!(
            "click rectangle must have positive dimensions: {}x{}",
            rect.width, rect.height
        )));
    }
    Ok(SemanticPoint {
        x: rect.x + rect.width / 2,
        y: rect.y + rect.height / 2,
    })
}

pub(crate) fn point_json(point: SemanticPoint) -> Value {
    json!({
        "x": point.x,
        "y": point.y
    })
}

fn send_semantic_input(
    global: &GlobalOptions,
    config: &UserConfig,
    input: &SemanticInput,
) -> CliOutcome<Value> {
    #[cfg(test)]
    if let Some(fake) = test_fake_semantic_input(global, config, input)? {
        return Ok(fake);
    }

    let (mut backend, instance_alias) = open_cli_runtime_input_proxy(global, config)?;
    let operation = match input {
        SemanticInput::Tap { point, .. } => backend.tap(point.x, point.y),
        SemanticInput::TargetCenter { .. } => {
            return Err(CliError::usage(
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
        .map_err(|err| CliError::device(err.to_string()))?;
    Ok(json!({
        "backend": "runtime_proxy",
        "touch_backend_requested": "runtime_owned",
        "touch_backend_attempts": [],
        "touch_backend_warnings": [],
        "control_mode": "semantic",
        "instance": instance_alias,
        "serial": Value::Null,
        "device_state": "runtime_owned",
        "screen_size": Value::Null,
        "handshake": Value::Null,
        "action": semantic_input_json(input)
    }))
}

#[cfg(test)]
fn test_fake_semantic_input(
    global: &GlobalOptions,
    config: &UserConfig,
    input: &SemanticInput,
) -> CliOutcome<Option<Value>> {
    let Ok(path) = env::var("ACTINGCOMMAND_TEST_FAKE_TOUCH_LOG") else {
        return Ok(None);
    };
    let device_config = device_config(global, config)?;
    let action = semantic_input_json(input);
    let event = json!({
        "backend": "test_fake_touch",
        "serial": device_config.target.resolved_serial(),
        "action": action
    });
    fs::write(
        &path,
        serde_json::to_vec(&event).map_err(|err| CliError::device(err.to_string()))?,
    )
    .map_err(|err| CliError::device(format!("failed to write fake touch log {path}: {err}")))?;
    Ok(Some(json!({
        "backend": "test_fake_touch",
        "touch_backend_requested": device_config.touch_backend.as_str(),
        "adb_source": device_config.adb_source.as_str(),
        "adb_warning": device_config.adb_warning,
        "touch_backend_attempts": [],
        "touch_backend_warnings": [],
        "control_mode": "semantic",
        "serial": device_config.target.resolved_serial(),
        "device_state": "device",
        "screen_size": "Physical size: 1280x720",
        "handshake": Value::Null,
        "action": action
    })))
}

struct NavigationExecutionContext<'a> {
    global: &'a GlobalOptions,
    flags: &'a FlagArgs,
    config: &'a UserConfig,
    evaluator: &'a RecognitionEvaluator,
    detector: &'a PageDetector,
    destructive_clicks: &'a [DestructiveClick],
    step_timeout: Duration,
    poll: Duration,
}

fn execute_navigation_route(
    ctx: &NavigationExecutionContext<'_>,
    start_page: String,
    route: Vec<NavigationEdge>,
) -> CliOutcome<(Vec<Value>, String)> {
    let mut executed = Vec::new();
    let mut current_page = start_page;
    for edge in route {
        if current_page != edge.from_page {
            return Err(CliError::safety_blocked(
                "navigation_page_drift",
                format!(
                    "navigation expected current page '{}' but last page was '{}'",
                    edge.from_page, current_page
                ),
                &["page_guard"],
            ));
        }
        let (input, recognition) = resolve_navigation_edge_input(ctx, &edge)?;
        reject_destructive_overlap_input(&edge, &input, ctx.destructive_clicks)?;
        let device = send_semantic_input(ctx.global, ctx.config, &input)?;
        let arrived = poll_for_page(
            ctx.global,
            ctx.flags,
            ctx.evaluator,
            ctx.detector,
            &edge.to_page,
            ctx.step_timeout,
            ctx.poll,
        )?;
        if !arrived.matched {
            return Err(CliError::safety_blocked(
                "navigation_arrival_failed",
                format!(
                    "navigation edge '{}' did not arrive at '{}'; last page '{}'",
                    edge.id, edge.to_page, arrived.page
                ),
                &["arrival_page"],
            ));
        }
        current_page = arrived.page.clone();
        executed.push(json!({
            "edge": navigation_edge_json(&edge),
            "resolved_input": semantic_input_json(&input),
            "recognition": recognition,
            "device": device,
            "arrived": page_detection_json(&arrived)
        }));
    }
    Ok((executed, current_page))
}

fn resolve_navigation_edge_input(
    ctx: &NavigationExecutionContext<'_>,
    edge: &NavigationEdge,
) -> CliOutcome<(SemanticInput, Value)> {
    let SemanticInput::TargetCenter { target_id } = &edge.input else {
        return Ok((edge.input.clone(), Value::Null));
    };
    let scene = load_scene_from_flags(ctx.global, ctx.flags)?;
    let evaluation = ctx
        .evaluator
        .evaluate_target(&scene, target_id)
        .map_err(|err| CliError::usage(err.to_string()))?;
    let evaluation_json = target_eval_json(&evaluation);
    if !evaluation.passed {
        return Err(CliError::safety_blocked(
            "navigation_target_not_visible",
            format!(
                "navigation edge '{}' target '{}' did not pass recognition: {}",
                edge.id, target_id, evaluation.message
            ),
            &["visible_target", "navigation"],
        ));
    }
    let rect = target_evaluation_rect(&evaluation)?;
    let input = SemanticInput::Tap {
        rect,
        point: rect_center(rect)?,
    };
    Ok((
        input,
        json!({
            "target_id": target_id,
            "evaluation": evaluation_json
        }),
    ))
}

pub(crate) fn target_evaluation_rect(evaluation: &TargetEvaluation) -> CliOutcome<PackRect> {
    let template = evaluation.template.as_ref().ok_or_else(|| {
        CliError::usage(format!(
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

fn poll_for_page(
    global: &GlobalOptions,
    flags: &FlagArgs,
    evaluator: &RecognitionEvaluator,
    detector: &PageDetector,
    page_id: &str,
    timeout: Duration,
    poll: Duration,
) -> CliOutcome<PageDetectionOutcome> {
    let started = Instant::now();
    let mut last = None;
    while started.elapsed() <= timeout {
        thread::sleep(poll);
        let scene = load_scene_from_flags(global, flags)?;
        let outcome = detect_current_page(evaluator, detector, &scene)?;
        if outcome.matched && outcome.page == page_id {
            return Ok(outcome);
        }
        last = Some(outcome);
    }
    Ok(last.unwrap_or(PageDetectionOutcome {
        page: "standby".to_string(),
        matched: false,
        standby: true,
        evaluations: Vec::new(),
    }))
}

fn poll_for_matched_page(
    global: &GlobalOptions,
    flags: &FlagArgs,
    evaluator: &RecognitionEvaluator,
    detector: &PageDetector,
    timeout: Duration,
    poll: Duration,
) -> CliOutcome<PageDetectionOutcome> {
    let started = Instant::now();
    let mut last = None;
    while started.elapsed() <= timeout {
        thread::sleep(poll);
        let scene = load_scene_from_flags(global, flags)?;
        let outcome = detect_current_page(evaluator, detector, &scene)?;
        if outcome.matched {
            return Ok(outcome);
        }
        last = Some(outcome);
    }
    Ok(last.unwrap_or(PageDetectionOutcome {
        page: "standby".to_string(),
        matched: false,
        standby: true,
        evaluations: Vec::new(),
    }))
}

#[derive(Debug, Clone)]
struct RecognitionResourcePaths {
    pack_path: PathBuf,
    pack_root: PathBuf,
    pages_path: Option<PathBuf>,
}

fn recognition_resources(
    global: &GlobalOptions,
    config: &UserConfig,
    flags: &FlagArgs,
    require_pages: bool,
) -> CliOutcome<RecognitionResourcePaths> {
    if let Some(pack_path) = flags.optional_path("--pack") {
        let pack_root = flags.required_path("--pack-root")?;
        let pages_path = if require_pages {
            Some(flags.required_path("--pages")?)
        } else {
            flags.optional_path("--pages")
        };
        return Ok(RecognitionResourcePaths {
            pack_path,
            pack_root,
            pages_path,
        });
    }

    let root = effective_resource_root(global, config).ok_or_else(|| {
        CliError::usage("command requires --pack/--pack-root or --resource-root with --game")
    })?;
    let (game, server) = recognition_selector(global)?;
    let stem = format!("{game}.{server}");
    let recognition_dir = root.join("recognition");
    Ok(RecognitionResourcePaths {
        pack_path: recognition_dir.join(format!("{stem}.pack.json")),
        pack_root: root,
        pages_path: Some(recognition_dir.join(format!("{stem}.pages.json"))),
    })
}

fn recognition_selector(global: &GlobalOptions) -> CliOutcome<(String, String)> {
    let game = global
        .game
        .as_deref()
        .ok_or_else(|| CliError::usage("--game is required when --pack is omitted"))
        .and_then(canonical_game)?;
    let server = global
        .server
        .clone()
        .ok_or_else(|| CliError::usage("--server is required when --pack is omitted"))?;
    let server = canonical_server(&server)?;
    Ok((game, server))
}

fn load_scene_from_flags(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Scene> {
    if let Some(scene) = flags.optional_path("--scene") {
        let png = fs::read(&scene).map_err(|err| {
            CliError::device(format!("failed to read {}: {err}", scene.display()))
        })?;
        return Scene::from_png(&png).map_err(|err| CliError::device(err.to_string()));
    }
    if flags.bool("--capture") {
        let config = read_user_config()?;
        let device_config = device_config(global, &config)?;
        let requested = device_config.capture_backend;
        let fresh_delay = parse_optional_duration_ms(flags, "--fresh-delay-ms", 160)?;
        let captured = capture_for_command(
            &device_config,
            requested,
            flags.bool("--require-fresh"),
            fresh_delay,
        )?;
        let frame = captured.frame;
        return scene_from_frame(&frame);
    }
    Err(CliError::usage(
        "command requires --scene <png> or --capture",
    ))
}

struct LoadedEvaluator {
    evaluator: RecognitionEvaluator,
    env_resolved: Vec<env_detection::ResolvedEnvValue>,
}

fn load_evaluator_with_env(
    global: &GlobalOptions,
    flags: &FlagArgs,
    pack_path: &Path,
    pack_root: &Path,
) -> CliOutcome<LoadedEvaluator> {
    let pack_json = fs::read_to_string(pack_path)
        .map_err(|err| CliError::usage(format!("failed to read {}: {err}", pack_path.display())))?;
    let mut pack_value: Value = serde_json::from_str(&pack_json).map_err(|err| {
        CliError::usage(format!("failed to parse {}: {err}", pack_path.display()))
    })?;
    let env_resolved =
        env_detection::resolve_env_markers_in_value(global, flags, pack_root, &mut pack_value)?;
    let pack_json = serde_json::to_string(&pack_value).map_err(|err| {
        CliError::usage(format!(
            "failed to serialize resolved recognition pack {}: {err}",
            pack_path.display()
        ))
    })?;
    let pack =
        load_pack_from_json_str(&pack_json).map_err(|err| CliError::usage(err.to_string()))?;
    let evaluator = RecognitionEvaluator::new(pack_root.to_path_buf(), pack)
        .map_err(|err| CliError::usage(err.to_string()))?;
    Ok(LoadedEvaluator {
        evaluator,
        env_resolved,
    })
}

fn load_evaluator_and_detector_with_env(
    global: &GlobalOptions,
    flags: &FlagArgs,
    pack_path: &Path,
    pack_root: &Path,
    pages_path: &Path,
) -> CliOutcome<(
    RecognitionEvaluator,
    PageDetector,
    Vec<env_detection::ResolvedEnvValue>,
)> {
    let loaded = load_evaluator_with_env(global, flags, pack_path, pack_root)?;
    let pages_json = fs::read_to_string(pages_path).map_err(|err| {
        CliError::usage(format!("failed to read {}: {err}", pages_path.display()))
    })?;
    let pages =
        load_page_set_from_json_str(&pages_json).map_err(|err| CliError::usage(err.to_string()))?;
    let detector = PageDetector::new(pages).map_err(|err| CliError::usage(err.to_string()))?;
    Ok((loaded.evaluator, detector, loaded.env_resolved))
}

fn page_eval_json(evaluation: &actingcommand_page_detector::PageEvaluation) -> Value {
    json!({
        "page": evaluation.page_id,
        "matched": evaluation.matched,
        "message": evaluation.message,
        "any_of_passed": evaluation.any_of_passed,
        "any_of_total": evaluation.any_of_total,
        "targets": evaluation
            .target_results
            .iter()
            .map(|target| {
                json!({
                    "id": target.target_id,
                    "role": format!("{:?}", target.role),
                    "passed": target.passed,
                    "message": target.message
                })
            })
            .collect::<Vec<_>>()
    })
}

pub(crate) fn rect_json(rect: actingcommand_recognition_pack::PackRect) -> Value {
    json!({
        "x": rect.x,
        "y": rect.y,
        "width": rect.width,
        "height": rect.height
    })
}
