// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{
    CorrelationId, EventQuery, InputAction, ProjectionProfile, RuntimeResult,
};
use actingcommand_ledger::{
    EvidenceStore, IdIssuer, IdKind, ProjectionRequest, ProjectionVerbosity, error_projection,
    forbidden_target_suspicion, guard_reject_suspicion, low_margin_suspicion, project_record,
    stale_frame_suspicion,
};
use actingcommand_runtime_client::RuntimeDebugSession;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use super::*;

struct Lab2Ids {
    issuer: IdIssuer,
    req_id: String,
}

impl Lab2Ids {
    fn new() -> Self {
        let issuer = IdIssuer::new();
        let req_id = issuer.issue(IdKind::Req).value;
        Self { issuer, req_id }
    }

    fn issue(&self, kind: IdKind) -> String {
        self.issuer.issue(kind).value
    }
}

struct Lab2Scene {
    scene: Scene,
    backend: String,
    source: Value,
    frame_age_ms: u64,
    png: Option<Vec<u8>>,
}

struct WaitIds<'a> {
    req_id: &'a str,
    wf_id: &'a str,
}

#[derive(Clone, Copy)]
struct WaitTiming {
    timeout: Duration,
    poll: Duration,
}

struct Lab2ClickRect {
    kind: &'static str,
    rect: PackRect,
    derivation: Value,
}

#[derive(Clone, Copy)]
struct Lab2CommandContract {
    name: &'static str,
    summary: &'static str,
    required: &'static [&'static str],
    optional: &'static [&'static str],
    output_fields: &'static [&'static str],
    requires_lease: bool,
}

pub(crate) fn run_observe(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    if flags.bool("--capture") {
        return run_runtime_observe(global, &flags);
    }
    let ids = Lab2Ids::new();
    let instance = lab2_instance(global, &flags);
    let resources = super::contained_resources::load(&flags, "observe")?;
    let (evaluator, detector) = super::contained_resources::recognition_pipeline(&resources)?;
    let graph = super::contained_resources::navigation_graph(&resources)?;
    let loaded_scene = load_lab2_scene(global, &flags)?;
    let outcome = detect_current_page(&evaluator, &detector, &loaded_scene.scene)?;
    let frame_path = write_frame_if_requested(&flags, &loaded_scene)?;
    let targets = observe_targets(&evaluator, &loaded_scene.scene, &flags, &outcome)?;
    let actions = observe_actions(&graph, &outcome);
    let mut payload = json!({
        "req_id": ids.req_id,
        "state": if outcome.matched { "observed" } else { "unknown" },
        "instance": instance,
        "page": if outcome.matched { outcome.page.clone() } else { "unknown".to_string() },
        "matched": outcome.matched,
        "standby": outcome.standby,
        "frame_age_ms": loaded_scene.frame_age_ms,
        "backend": loaded_scene.backend,
        "frame_source": loaded_scene.source,
        "targets": targets,
        "actions": actions,
        "arbitration": isolated_offline_projection(),
    });
    if !outcome.matched {
        payload["candidates"] = json!(lab2_page_candidates(&outcome));
    }
    if let Some(suspicion) = lab2_observation_suspicion(&outcome, loaded_scene.frame_age_ms) {
        payload["suspicion"] = suspicion;
    }
    if let Some(path) = frame_path {
        payload["frame_path"] = json!(path.display().to_string());
    }
    project_lab2_payload(&payload, &flags)
}

fn run_runtime_observe(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    reject_mixed_online_and_offline_scene(flags, "observe")?;
    let instance = lab2_instance(global, flags);
    let resources = super::contained_resources::load(flags, "observe")?;
    let (evaluator, detector) = super::contained_resources::recognition_pipeline(&resources)?;
    let graph = super::contained_resources::navigation_graph(&resources)?;
    let session = begin_runtime_debug_session()?;
    let result = (|| -> CliOutcome<Value> {
        let loaded_scene = load_runtime_lab2_scene(&session, &instance)?;
        let outcome = detect_current_page(&evaluator, &detector, &loaded_scene.scene)?;
        let frame_path = write_frame_if_requested(flags, &loaded_scene)?;
        let targets = observe_targets(&evaluator, &loaded_scene.scene, flags, &outcome)?;
        let actions = observe_actions(&graph, &outcome);
        let mut payload = json!({
            "req_id": session.correlation_id(),
            "state": if outcome.matched { "observed" } else { "unknown" },
            "instance": instance,
            "page": if outcome.matched { outcome.page.clone() } else { "unknown".to_string() },
            "matched": outcome.matched,
            "standby": outcome.standby,
            "frame_age_ms": loaded_scene.frame_age_ms,
            "backend": loaded_scene.backend,
            "frame_source": loaded_scene.source,
            "targets": targets,
            "actions": actions,
            "arbitration": runtime_scheduler_projection(false),
        });
        if !outcome.matched {
            payload["candidates"] = json!(lab2_page_candidates(&outcome));
        }
        if let Some(suspicion) = lab2_observation_suspicion(&outcome, loaded_scene.frame_age_ms) {
            payload["suspicion"] = suspicion;
        }
        if let Some(path) = frame_path {
            payload["frame_path"] = json!(path.display().to_string());
        }
        attach_env_resolved(&mut payload, &Vec::<env_detection::ResolvedEnvValue>::new());
        Ok(payload)
    })();
    finish_runtime_debug_result(&session, flags, result)
}

fn run_runtime_do(
    flags: &FlagArgs,
    target: &str,
    instance: &str,
    dry_run: bool,
    allow_destructive: bool,
) -> CliOutcome<Value> {
    reject_mixed_online_and_offline_scene(flags, "do")?;
    let resources = super::contained_resources::load(flags, "do")?;
    let (evaluator, detector) = super::contained_resources::recognition_pipeline(&resources)?;
    let session = begin_runtime_debug_session()?;
    let result = (|| -> CliOutcome<Value> {
        guard_evaluable_target(&evaluator, target, "do")?;
        let loaded_scene = load_runtime_lab2_scene(&session, instance)?;
        let before = detect_current_page(&evaluator, &detector, &loaded_scene.scene)?;
        let evaluation = evaluator
            .evaluate_target(&loaded_scene.scene, target)
            .map_err(|err| CliError::usage(err.to_string()))?;
        if !evaluation.passed {
            return Err(CliError::safety_blocked(
                "target_not_visible",
                format!(
                    "target '{target}' did not pass guard recognition: {}",
                    evaluation.message
                ),
                &["guard_target"],
            )
            .with_details(json!({
                "error": "resource_drift",
                "state": before.page,
                "hint": "observe-current-page-and-refresh-resource-or-target",
                "suspicion": guard_reject_suspicion(target, &evaluation.message)
            })));
        }
        let click = evaluator
            .get_click_target(target)
            .map_err(|err| CliError::usage(err.to_string()))?;
        let actual_click = derive_lab2_click_rect(&evaluator, target, click, &evaluation)?;
        if !allow_destructive {
            let graph = super::contained_resources::navigation_graph(&resources)?;
            reject_lab2_destructive_click_overlap(target, &before.page, actual_click.rect, &graph)?;
        }
        let point = rect_center(actual_click.rect)?;
        let (action_id, device) = if dry_run {
            (Value::Null, json!({"executed": false, "mode": "dry_run"}))
        } else {
            let action_id = execute_runtime_debug_input(
                &session,
                instance,
                InputAction::Tap {
                    x: point.x,
                    y: point.y,
                },
            )?;
            (
                serde_json::to_value(action_id)
                    .map_err(|error| CliError::device(error.to_string()))?,
                json!({
                    "executed": true,
                    "backend": "runtime_proxy",
                    "authority": "runtime_execution_kernel",
                    "action": {"type": "tap", "x": point.x, "y": point.y}
                }),
            )
        };
        let after = if dry_run {
            before
        } else {
            let after_scene = load_runtime_lab2_scene(&session, instance)?;
            detect_current_page(&evaluator, &detector, &after_scene.scene)?
        };
        let mut payload = json!({
            "req_id": session.correlation_id(),
            "reco_id": Value::Null,
            "action_id": action_id,
            "state": if dry_run { "planned" } else { "sent" },
            "instance": instance,
            "executed": !dry_run,
            "target": target,
            "page": after.page,
            "frame_age_ms": loaded_scene.frame_age_ms,
            "backend": loaded_scene.backend,
            "actual_click": {
                "kind": actual_click.kind,
                "declared_rect": rect_json(click),
                "rect": rect_json(actual_click.rect),
                "point": point_json(point),
                "coordinate_derivation": actual_click.derivation
            },
            "guard_result": {
                "reco_id": Value::Null,
                "target": target,
                "passed": true,
                "evaluation": target_eval_json(&evaluation)
            },
            "observation": page_detection_json(&after),
            "device": device,
            "arbitration": runtime_scheduler_projection(!dry_run),
        });
        attach_env_resolved(&mut payload, &Vec::<env_detection::ResolvedEnvValue>::new());
        Ok(payload)
    })();
    finish_runtime_debug_result(&session, flags, result)
}

fn run_runtime_ensure(
    flags: &FlagArgs,
    to: &str,
    instance: &str,
    dry_run: bool,
    allow_destructive: bool,
) -> CliOutcome<Value> {
    reject_mixed_online_and_offline_scene(flags, "ensure")?;
    let resources = super::contained_resources::load(flags, "ensure")?;
    let (evaluator, detector) = super::contained_resources::recognition_pipeline(&resources)?;
    let graph = super::contained_resources::navigation_graph(&resources)?;
    let target_page = canonical_navigation_page(&graph, to);
    let session = begin_runtime_debug_session()?;
    let result = (|| -> CliOutcome<Value> {
        let scene = load_runtime_lab2_scene(&session, instance)?;
        let start = detect_current_page(&evaluator, &detector, &scene.scene)?;
        if start.matched && start.page == target_page {
            return Ok(json!({
                "req_id": session.correlation_id(),
                "state": "already_at_target",
                "instance": instance,
                "executed": false,
                "page": start.page,
                "to": target_page,
                "route": [],
                "frame_age_ms": scene.frame_age_ms,
                "backend": scene.backend,
                "arbitration": runtime_scheduler_projection(false),
            }));
        }
        if !start.matched {
            return Err(CliError::safety_blocked(
                "current_page_unknown",
                "ensure requires a matched current page before navigation",
                &["current_page"],
            )
            .with_details(json!({
                "error": "resource_drift",
                "state": start.page,
                "hint": "observe-current-page-or-route-home-before-ensure"
            })));
        }
        let route =
            find_navigation_route(&graph.edges, &start.page, &target_page).ok_or_else(|| {
                CliError::usage(format!(
                    "no navigation route from '{}' to '{}'",
                    start.page, target_page
                ))
            })?;
        for edge in &route {
            if !allow_destructive {
                reject_dangerous_semantic_id("navigation edge", &edge.id)?;
                reject_destructive_overlap(edge, &graph.destructive_clicks)?;
            }
        }
        let route_json = route.iter().map(navigation_edge_json).collect::<Vec<_>>();
        if dry_run {
            return Ok(json!({
                "req_id": session.correlation_id(),
                "state": "planned",
                "instance": instance,
                "executed": false,
                "page": start.page,
                "to": target_page,
                "route": route_json,
                "frame_age_ms": scene.frame_age_ms,
                "backend": scene.backend,
                "arbitration": runtime_scheduler_projection(false),
            }));
        }

        let step_timeout = parse_optional_duration_ms(flags, "--step-timeout-ms", 5_000)?;
        let poll = parse_optional_duration_ms(flags, "--poll-ms", 500)?;
        let execution = RuntimeNavigationContext {
            session: &session,
            instance,
            evaluator: &evaluator,
            detector: &detector,
            destructive_clicks: &graph.destructive_clicks,
            allow_destructive,
            step_timeout,
            poll,
        };
        let (steps, arrived) =
            execute_runtime_navigation_route(&execution, start.page.clone(), route)?;
        Ok(json!({
            "req_id": session.correlation_id(),
            "state": "arrived",
            "instance": instance,
            "executed": true,
            "from": start.page,
            "page": arrived,
            "to": target_page,
            "route": route_json,
            "steps": steps,
            "arbitration": runtime_scheduler_projection(true),
        }))
    })();
    finish_runtime_debug_result(&session, flags, result)
}

fn run_runtime_wait(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    reject_mixed_online_and_offline_scene(flags, "wait")?;
    let instance = lab2_instance(global, flags);
    let resources = super::contained_resources::load(flags, "wait")?;
    let (evaluator, detector) = super::contained_resources::recognition_pipeline(&resources)?;
    let timeout = parse_optional_duration_ms(flags, "--timeout-ms", 5_000)?;
    let poll = parse_optional_duration_ms(flags, "--poll-ms", 200)?;
    let session = begin_runtime_debug_session()?;
    let result = if let Some(page) = wait_page_target(flags) {
        let graph = super::contained_resources::navigation_graph(&resources)?;
        let page = canonical_navigation_page(&graph, &page);
        runtime_wait_for_page(
            &session, &instance, &evaluator, &detector, &page, timeout, poll,
        )
    } else if let Some(target) = flags.optional("--stable").filter(|value| value != "true") {
        runtime_wait_for_stable_target(&session, &instance, &evaluator, &target, timeout, poll)
    } else {
        Err(CliError::usage(
            "wait requires --page <page> or --stable <target>",
        ))
    };
    let result = result.map(|mut payload| {
        payload["instance"] = json!(instance);
        payload["arbitration"] = runtime_scheduler_projection(false);
        payload
    });
    finish_runtime_debug_result(&session, flags, result)
}

struct RuntimeNavigationContext<'a> {
    session: &'a RuntimeDebugSession,
    instance: &'a str,
    evaluator: &'a RecognitionEvaluator,
    detector: &'a PageDetector,
    destructive_clicks: &'a [DestructiveClick],
    allow_destructive: bool,
    step_timeout: Duration,
    poll: Duration,
}

fn execute_runtime_navigation_route(
    ctx: &RuntimeNavigationContext<'_>,
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
        let guard_scene = load_runtime_lab2_scene(ctx.session, ctx.instance)?;
        let guard_page = detect_current_page(ctx.evaluator, ctx.detector, &guard_scene.scene)?;
        if !guard_page.matched || guard_page.page != edge.from_page {
            return Err(CliError::safety_blocked(
                "navigation_page_drift",
                format!(
                    "navigation edge '{}' expected '{}' but Runtime observed '{}'",
                    edge.id, edge.from_page, guard_page.page
                ),
                &["page_guard"],
            ));
        }
        let (input, recognition) =
            resolve_runtime_navigation_edge_input(ctx.evaluator, &guard_scene.scene, &edge)?;
        if !ctx.allow_destructive {
            reject_destructive_overlap_input(&edge, &input, ctx.destructive_clicks)?;
        }
        let runtime_action = runtime_input_action(&input)?;
        let action_id = execute_runtime_debug_input(ctx.session, ctx.instance, runtime_action)?;
        let arrived = poll_for_runtime_page(
            ctx.session,
            ctx.instance,
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
            )
            .with_details(json!({
                "error": "navigation_arrival_failed",
                "state": arrived.page,
                "hint": "inspect-runtime-captures-and-navigation-resource"
            })));
        }
        current_page = arrived.page.clone();
        executed.push(json!({
            "edge": navigation_edge_json(&edge),
            "resolved_input": semantic_input_json(&input),
            "recognition": recognition,
            "action_id": action_id,
            "device": {
                "executed": true,
                "backend": "runtime_proxy",
                "authority": "runtime_execution_kernel"
            },
            "arrived": page_detection_json(&arrived)
        }));
    }
    Ok((executed, current_page))
}

fn resolve_runtime_navigation_edge_input(
    evaluator: &RecognitionEvaluator,
    scene: &Scene,
    edge: &NavigationEdge,
) -> CliOutcome<(SemanticInput, Value)> {
    let SemanticInput::TargetCenter { target_id } = &edge.input else {
        return Ok((edge.input.clone(), Value::Null));
    };
    let evaluation = evaluator
        .evaluate_target(scene, target_id)
        .map_err(|error| CliError::usage(error.to_string()))?;
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
    Ok((
        SemanticInput::Tap {
            rect,
            point: rect_center(rect)?,
        },
        json!({
            "target_id": target_id,
            "evaluation": target_eval_json(&evaluation)
        }),
    ))
}

fn runtime_input_action(input: &SemanticInput) -> CliOutcome<InputAction> {
    match input {
        SemanticInput::Tap { point, .. } => Ok(InputAction::Tap {
            x: point.x,
            y: point.y,
        }),
        SemanticInput::Drag {
            from,
            to,
            duration_ms,
            ..
        } => Ok(InputAction::Swipe {
            x1: from.x,
            y1: from.y,
            x2: to.x,
            y2: to.y,
            duration_ms: *duration_ms,
        }),
        SemanticInput::TargetCenter { .. } => Err(CliError::device(
            "Runtime navigation target was not resolved before input",
        )),
    }
}

fn poll_for_runtime_page(
    session: &RuntimeDebugSession,
    instance: &str,
    evaluator: &RecognitionEvaluator,
    detector: &PageDetector,
    page_id: &str,
    timeout: Duration,
    poll: Duration,
) -> CliOutcome<PageDetectionOutcome> {
    let started = Instant::now();
    loop {
        if !poll.is_zero() {
            thread::sleep(poll.min(timeout.saturating_sub(started.elapsed())));
        }
        let scene = load_runtime_lab2_scene(session, instance)?;
        let outcome = detect_current_page(evaluator, detector, &scene.scene)?;
        if outcome.matched && outcome.page == page_id {
            return Ok(outcome);
        }
        if started.elapsed() >= timeout {
            return Ok(outcome);
        }
    }
}

fn runtime_wait_for_page(
    session: &RuntimeDebugSession,
    instance: &str,
    evaluator: &RecognitionEvaluator,
    detector: &PageDetector,
    page: &str,
    timeout: Duration,
    poll: Duration,
) -> CliOutcome<Value> {
    let started = Instant::now();
    loop {
        let scene = load_runtime_lab2_scene(session, instance)?;
        let outcome = detect_current_page(evaluator, detector, &scene.scene)?;
        if outcome.matched && outcome.page == page {
            return Ok(json!({
                "req_id": session.correlation_id(),
                "wf_id": session.correlation_id(),
                "state": "arrived",
                "page": outcome.page,
                "matched": true,
                "elapsed_ms": started.elapsed().as_millis() as u64,
                "frame_age_ms": scene.frame_age_ms,
                "backend": scene.backend
            }));
        }
        if started.elapsed() >= timeout {
            return Err(CliError::safety_blocked(
                "wait_timeout",
                format!("wait timed out before page '{page}' became current"),
                &["page_wait"],
            )
            .with_details(json!({
                "error": "transient",
                "state": outcome.page,
                "hint": "retry-or-observe-current-page"
            })));
        }
        thread::sleep(poll.min(timeout.saturating_sub(started.elapsed())));
    }
}

fn runtime_wait_for_stable_target(
    session: &RuntimeDebugSession,
    instance: &str,
    evaluator: &RecognitionEvaluator,
    target: &str,
    timeout: Duration,
    poll: Duration,
) -> CliOutcome<Value> {
    guard_evaluable_target(evaluator, target, "wait --stable")?;
    let started = Instant::now();
    let first = load_runtime_lab2_scene(session, instance)?;
    let mut previous = evaluator
        .evaluate_target(&first.scene, target)
        .map_err(|error| CliError::usage(error.to_string()))?;
    if !previous.passed {
        return Err(CliError::safety_blocked(
            "stable_target_not_visible",
            format!("stable target '{target}' did not pass baseline guard"),
            &["stable_target"],
        )
        .with_details(json!({
            "error": "resource_drift",
            "state": "unstable",
            "hint": "observe-current-page-and-refresh-stable-target",
            "suspicion": guard_reject_suspicion(target, &previous.message)
        })));
    }
    loop {
        let next_scene = load_runtime_lab2_scene(session, instance)?;
        let current = evaluator
            .evaluate_target(&next_scene.scene, target)
            .map_err(|error| CliError::usage(error.to_string()))?;
        if actingcommand_lab::target_evaluations_stable_for_wait(&previous, &current) {
            return Ok(json!({
                "req_id": session.correlation_id(),
                "wf_id": session.correlation_id(),
                "state": "stable",
                "target": target,
                "elapsed_ms": started.elapsed().as_millis() as u64,
                "frame_age_ms": next_scene.frame_age_ms,
                "backend": next_scene.backend,
                "evaluation": target_eval_json(&current)
            }));
        }
        if started.elapsed() >= timeout {
            return Err(CliError::safety_blocked(
                "wait_timeout",
                format!("wait timed out before target '{target}' became stable"),
                &["stable_target"],
            )
            .with_details(json!({
                "error": "transient",
                "state": "unstable",
                "hint": "retry-or-inspect-runtime-capture"
            })));
        }
        previous = current;
        thread::sleep(poll.min(timeout.saturating_sub(started.elapsed())));
    }
}

fn begin_runtime_debug_session() -> CliOutcome<RuntimeDebugSession> {
    RuntimeClient::connect(RuntimeClientConfig::new(
        runtime_state_root()?,
        actingcommand_contract::EventActor::Lab,
        actingcommand_contract::EventSource::Lab,
    ))
    .and_then(|client| client.begin_debug_session())
    .map_err(|error| CliError::device(error.to_string()))
}

fn load_runtime_lab2_scene(session: &RuntimeDebugSession, instance: &str) -> CliOutcome<Lab2Scene> {
    let receipt = session
        .observe_readonly(instance)
        .map_err(|error| CliError::device(error.to_string()))?;
    let observation = match receipt.result() {
        Some(RuntimeResult::ReadonlyObservationCompleted { observation }) => observation,
        _ => {
            return Err(CliError::device(
                "Runtime returned an invalid debug observation",
            ));
        }
    };
    let frame =
        runtime_capture_backend::frame_from_observation(&runtime_state_root()?, observation)
            .map_err(|error| CliError::device(error.to_string()))?;
    let frame_age_ms = system_time_age_ms(frame.captured_at);
    let backend = frame.backend_name.as_str().to_string();
    let png = frame.original_png.clone();
    let scene = scene_from_frame(&frame)?;
    Ok(Lab2Scene {
        scene,
        backend,
        source: json!({
            "kind": "runtime_observation",
            "artifact": observation.artifact(),
            "authority": "runtime_artifact_store"
        }),
        frame_age_ms,
        png,
    })
}

fn execute_runtime_debug_input(
    session: &RuntimeDebugSession,
    instance: &str,
    action: InputAction,
) -> CliOutcome<actingcommand_contract::ActionId> {
    let token = session
        .acquire_lease(instance)
        .map_err(|error| CliError::device(error.to_string()))?;
    let input = session.input(&token, action);
    let release = session.release_lease(&token);
    match (input, release) {
        (Ok(action_id), Ok(())) => Ok(action_id),
        (Err(primary), Ok(())) => Err(CliError::device(primary.to_string())),
        (Ok(_), Err(release)) => Err(CliError::device(format!(
            "Runtime input committed but lease release failed: {release}"
        ))),
        (Err(primary), Err(release)) => Err(CliError::device(format!(
            "Runtime input failed: {primary}; lease cleanup also failed: {release}"
        ))),
    }
}

fn finish_runtime_debug_result(
    session: &RuntimeDebugSession,
    flags: &FlagArgs,
    result: CliOutcome<Value>,
) -> CliOutcome<Value> {
    let events = session
        .query_events(ProjectionProfile::Lab)
        .map_err(|error| CliError::device(format!("Runtime debug projection failed: {error}")))?;
    let ledger = json!({
        "authority": "runtime_global_ledger",
        "correlation_id": session.correlation_id(),
        "event_count": events.len(),
        "events": events
    });
    match result {
        Ok(mut payload) => {
            payload["ledger"] = ledger.clone();
            payload["projection_source"] = json!({
                "kind": "runtime_global_ledger",
                "correlation_id": session.correlation_id(),
                "profile": "lab"
            });
            project_lab2_payload(&payload, flags)
        }
        Err(error) => {
            let mut details = error.details.clone().unwrap_or_else(|| {
                json!({
                    "error": error.code,
                    "state": "failed",
                    "hint": "inspect-runtime-ledger"
                })
            });
            details["ledger"] = ledger;
            details["projection_source"] = json!({
                "kind": "runtime_global_ledger",
                "correlation_id": session.correlation_id(),
                "profile": "lab"
            });
            Err(error.with_details(details))
        }
    }
}

fn runtime_scheduler_projection(write_requested: bool) -> Value {
    json!({
        "authority": "runtime_scheduler",
        "decision": if write_requested { "lease_acquired_and_released" } else { "readonly" },
        "local_arbitrator": false
    })
}

fn isolated_offline_projection() -> Value {
    json!({
        "authority": "isolated_offline",
        "decision": "not_applicable",
        "persistent_state": false
    })
}

fn isolated_offline_error_details(
    req_id: &str,
    error: &str,
    state: impl Into<String>,
    hint: &str,
    suspicion: Option<Value>,
) -> CliOutcome<Value> {
    let mut details = json!({
        "req_id": req_id,
        "error": error,
        "state": state.into(),
        "hint": hint,
        "authority": "isolated_offline"
    });
    if let Some(suspicion) = suspicion {
        details["suspicion"] = suspicion;
    }
    Ok(details)
}

fn reject_mixed_online_and_offline_scene(flags: &FlagArgs, command: &str) -> CliOutcome<()> {
    if flags.optional_path("--scene").is_some() {
        return Err(CliError::usage(format!(
            "{command} cannot combine --capture with offline --scene"
        )));
    }
    Ok(())
}

pub(crate) fn run_do(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let ids = Lab2Ids::new();
    let target = target_argument(&flags, "do")?;
    let instance = lab2_instance(global, &flags);
    let dry_run = global.dry_run || flags.bool("--dry-run");
    let allow_destructive = flags.bool("--allow-destructive");
    if flags.bool("--destructive") && !allow_destructive {
        let details = isolated_offline_error_details(
            &ids.req_id,
            "blocked",
            "blocked",
            "rerun-with---allow-destructive-if-this-action-is-intended",
            None,
        )?;
        return Err(CliError::safety_blocked(
            "destructive_action_requires_allow_destructive",
            "do --destructive requires --allow-destructive",
            &["allow_destructive"],
        )
        .with_details(details));
    }
    if !allow_destructive {
        reject_dangerous_semantic_id("target", &target)?;
    }
    if !dry_run && !flags.bool("--capture") {
        return Err(CliError::usage(
            "do real execution requires --capture; use --dry-run with --scene for offline planning",
        ));
    }
    if flags.bool("--capture") {
        return run_runtime_do(&flags, &target, &instance, dry_run, allow_destructive);
    }
    let result = (|| -> CliOutcome<Value> {
        let resources = super::contained_resources::load(&flags, "do")?;
        let (evaluator, detector) = super::contained_resources::recognition_pipeline(&resources)?;
        let env_resolved = Vec::<env_detection::ResolvedEnvValue>::new();
        guard_evaluable_target(&evaluator, &target, "do")?;
        let loaded_scene = load_lab2_scene(global, &flags)?;
        let before = detect_current_page(&evaluator, &detector, &loaded_scene.scene)?;
        let reco_id = ids.issue(IdKind::Reco);
        let evaluation = evaluator
            .evaluate_target(&loaded_scene.scene, &target)
            .map_err(|err| CliError::usage(err.to_string()))?;
        if !evaluation.passed {
            let details = isolated_offline_error_details(
                &ids.req_id,
                "resource_drift",
                before.page,
                "observe-current-page-and-refresh-resource-or-target",
                Some(guard_reject_suspicion(&target, &evaluation.message)),
            )?;
            return Err(CliError::safety_blocked(
                "target_not_visible",
                format!(
                    "target '{target}' did not pass guard recognition: {}",
                    evaluation.message
                ),
                &["guard_target"],
            )
            .with_details(details));
        }
        let click = evaluator
            .get_click_target(&target)
            .map_err(|err| CliError::usage(err.to_string()))?;
        let actual_click = derive_lab2_click_rect(&evaluator, &target, click, &evaluation)?;
        if !allow_destructive {
            let graph = super::contained_resources::navigation_graph(&resources)?;
            if let Err(error) = reject_lab2_destructive_click_overlap(
                &target,
                &before.page,
                actual_click.rect,
                &graph,
            ) {
                let details = isolated_offline_error_details(
                    &ids.req_id,
                    "resource_drift",
                    "blocked",
                    "rerun-with---allow-destructive-if-this-action-is-intended",
                    Some(forbidden_target_suspicion(vec![target.clone()])),
                )?;
                return Err(error.with_details(details));
            }
        }
        let point = rect_center(actual_click.rect)?;
        let action_id = ids.issue(IdKind::Action);
        let device = json!({"executed": false, "mode": "isolated_offline"});
        let after = before;
        let mut payload = json!({
            "req_id": ids.req_id,
            "reco_id": reco_id,
            "action_id": action_id,
            "state": if dry_run { "planned" } else { "sent" },
            "instance": instance,
            "executed": !dry_run,
            "target": target,
            "page": after.page,
            "frame_age_ms": loaded_scene.frame_age_ms,
            "backend": loaded_scene.backend,
            "actual_click": {
                "kind": actual_click.kind,
                "declared_rect": rect_json(click),
                "rect": rect_json(actual_click.rect),
                "point": point_json(point),
                "coordinate_derivation": actual_click.derivation
            },
            "guard_result": {
                "reco_id": reco_id,
                "target": target,
                "passed": true,
                "evaluation": target_eval_json(&evaluation)
            },
            "observation": page_detection_json(&after),
            "device": device,
            "arbitration": isolated_offline_projection(),
        });
        attach_env_resolved(&mut payload, &env_resolved);
        Ok(payload)
    })();
    result.and_then(|payload| project_lab2_payload(&payload, &flags))
}

pub(crate) fn run_ensure(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let ids = Lab2Ids::new();
    let to = ensure_target_page(&flags)?;
    let dry_run = global.dry_run || flags.bool("--dry-run");
    let allow_destructive = flags.bool("--allow-destructive");
    if !dry_run && !flags.bool("--capture") {
        return Err(CliError::usage(
            "ensure real execution requires --capture; use --dry-run with --scene for route planning",
        ));
    }
    let instance = lab2_instance(global, &flags);
    if flags.bool("--capture") {
        return run_runtime_ensure(&flags, &to, &instance, dry_run, allow_destructive);
    }
    let result = (|| -> CliOutcome<Value> {
        let resources = super::contained_resources::load(&flags, "ensure")?;
        let (evaluator, detector) = super::contained_resources::recognition_pipeline(&resources)?;
        let env_resolved = Vec::<env_detection::ResolvedEnvValue>::new();
        let graph = super::contained_resources::navigation_graph(&resources)?;
        let scene = load_lab2_scene(global, &flags)?;
        let start = detect_current_page(&evaluator, &detector, &scene.scene)?;
        let target_page = canonical_navigation_page(&graph, &to);
        if start.matched && start.page == target_page {
            let mut payload = json!({
                "req_id": ids.req_id,
                "state": "already_at_target",
                "instance": instance,
                "executed": false,
                "page": start.page,
                "to": target_page,
                "route": [],
                "frame_age_ms": scene.frame_age_ms,
                "backend": scene.backend,
                "arbitration": isolated_offline_projection(),
            });
            attach_env_resolved(&mut payload, &env_resolved);
            return Ok(payload);
        }
        if !start.matched {
            let details = isolated_offline_error_details(
                &ids.req_id,
                "resource_drift",
                start.page,
                "observe-current-page-or-route-home-before-ensure",
                None,
            )?;
            return Err(CliError::safety_blocked(
                "current_page_unknown",
                "ensure requires a matched current page before navigation",
                &["current_page"],
            )
            .with_details(details));
        }
        let route =
            find_navigation_route(&graph.edges, &start.page, &target_page).ok_or_else(|| {
                CliError::usage(format!(
                    "no navigation route from '{}' to '{}'",
                    start.page, target_page
                ))
            })?;
        for edge in &route {
            if !allow_destructive {
                reject_dangerous_semantic_id("navigation edge", &edge.id)?;
                reject_destructive_overlap(edge, &graph.destructive_clicks)?;
            }
        }
        let route_json = route.iter().map(navigation_edge_json).collect::<Vec<_>>();
        let mut payload = json!({
            "req_id": ids.req_id,
            "state": "planned",
            "instance": instance,
            "executed": false,
            "page": start.page,
            "to": target_page,
            "route": route_json,
            "frame_age_ms": scene.frame_age_ms,
            "backend": scene.backend,
            "arbitration": isolated_offline_projection(),
        });
        attach_env_resolved(&mut payload, &env_resolved);
        Ok(payload)
    })();
    result.and_then(|payload| project_lab2_payload(&payload, &flags))
}

pub(crate) fn run_wait(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    if flags.bool("--capture") {
        return run_runtime_wait(global, &flags);
    }
    let ids = Lab2Ids::new();
    let wf_id = ids.issue(IdKind::Wf);
    let instance = lab2_instance(global, &flags);
    let result = (|| -> CliOutcome<Value> {
        let resources = super::contained_resources::load(&flags, "wait")?;
        let (evaluator, detector) = super::contained_resources::recognition_pipeline(&resources)?;
        let env_resolved = Vec::<env_detection::ResolvedEnvValue>::new();
        let timeout = parse_optional_duration_ms(&flags, "--timeout-ms", 5_000)?;
        let poll = parse_optional_duration_ms(&flags, "--poll-ms", 200)?;
        let wait_ids = WaitIds {
            req_id: &ids.req_id,
            wf_id: &wf_id,
        };
        let timing = WaitTiming { timeout, poll };
        let payload = if let Some(page) = wait_page_target(&flags) {
            wait_for_page(
                global, &flags, &evaluator, &detector, wait_ids, &page, timing,
            )?
        } else if let Some(target) = flags.optional("--stable").filter(|value| value != "true") {
            wait_for_stable_target(global, &flags, &evaluator, wait_ids, &target, timing)?
        } else {
            return Err(CliError::usage(
                "wait requires --page <page> or --stable <target>",
            ));
        };
        let mut payload = payload;
        payload["instance"] = json!(instance);
        payload["arbitration"] = isolated_offline_projection();
        attach_env_resolved(&mut payload, &env_resolved);
        Ok(payload)
    })();
    result.and_then(|payload| project_lab2_payload(&payload, &flags))
}

pub(crate) fn run_receipt(_global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let req_id = flags
        .optional("--req")
        .filter(|value| value != "true")
        .or_else(|| flags.positionals.first().cloned())
        .ok_or_else(|| CliError::usage("lab receipt requires --req <req_id>"))?;
    let correlation = serde_json::from_value::<CorrelationId>(json!(req_id)).map_err(|_| {
        CliError::usage("lab receipt --req must be a Runtime correlation identifier")
    })?;
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        runtime_state_root()?,
        actingcommand_contract::EventActor::Lab,
        actingcommand_contract::EventSource::Lab,
    ))
    .map_err(|error| CliError::device(error.to_string()))?;
    let events = client
        .query_events(
            EventQuery {
                correlation_id: Some(correlation),
                ..EventQuery::default()
            },
            ProjectionProfile::Lab,
        )
        .map_err(|error| CliError::device(error.to_string()))?;
    if events.is_empty() {
        return Err(CliError::usage(format!(
            "no Runtime ledger events found for correlation '{req_id}'"
        )));
    }
    Ok(json!({
        "req_id": req_id,
        "correlation_id": correlation,
        "authority": "runtime_global_ledger",
        "event_count": events.len(),
        "events": events
    }))
}

pub(crate) fn run_evidence(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let evidence_id = flags
        .optional("--id")
        .filter(|value| value != "true")
        .or_else(|| flags.positionals.first().cloned())
        .ok_or_else(|| CliError::usage("lab evidence requires --id <evidence_id>"))?;
    let config = read_user_config()?;
    let run_root = effective_run_root(global, &config)
        .ok_or_else(|| CliError::usage("lab evidence requires --run-root or config run_root"))?;
    let refs = EvidenceStore::new(&run_root, true)
        .list_by_id(&evidence_id)
        .map_err(|err| CliError::device(err.to_string()))?;
    Ok(json!({
        "evidence_id": evidence_id,
        "count": refs.len(),
        "evidence": refs
    }))
}

pub(crate) fn run_arbitrator(_global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    FlagArgs::parse(args)?;
    Err(CliError::not_implemented(
        "legacy_lab2_arbitrator_retired",
        "Lab2 local arbitration was retired; Runtime scheduler is the only lease authority",
    ))
}

pub(crate) fn capability_summary(config: &UserConfig) -> Value {
    let commands = lab2_command_contracts()
        .into_iter()
        .map(command_contract_summary)
        .collect::<Vec<_>>();
    json!({
        "schema_version": "actingcommand.lab2.capabilities.v0.1",
        "verbs": commands,
        "click_kinds": ["target_rect_center"],
        "schema_versions": {
            "min": "0.3",
            "max": "0.5",
            "supported": ["0.3", "0.4", "0.5"]
        },
        "engine_capabilities": recognition_engine_capabilities(),
        "instances": lab2_config_instances(config),
        "recovery_transparency": {
            "authority": "runtime_global_ledger",
            "state_file": Value::Null,
            "event_type": "recovery.state.changed"
        },
        "error_codes": lab2_error_code_table(),
        "exit_codes": exit_code_table(),
        "escape_toolbox": escape_toolbox_groups()
    })
}

pub(crate) fn command_schema(command: &str) -> Option<Value> {
    let normalized = command.trim();
    lab2_command_contracts()
        .into_iter()
        .find(|contract| contract.name == normalized)
        .map(command_contract_schema)
}

fn project_lab2_payload(payload: &Value, flags: &FlagArgs) -> CliOutcome<Value> {
    let request = lab2_projection_request(
        flags,
        payload
            .get("req_id")
            .and_then(Value::as_str)
            .map(str::to_string),
    );
    project_record(payload, &request).map_err(|err| CliError::device(err.to_string()))
}

fn lab2_projection_request(flags: &FlagArgs, evidence_id: Option<String>) -> ProjectionRequest {
    ProjectionRequest {
        verbosity: if flags.bool("--pretty") {
            ProjectionVerbosity::Debug
        } else if flags.bool("--verbose") {
            ProjectionVerbosity::Normal
        } else {
            ProjectionVerbosity::Min
        },
        fields: fields_from_flags(flags),
        evidence_id,
    }
}

fn lab2_error_payload_for_flags(
    flags: &FlagArgs,
    req_id: &str,
    error: &str,
    state: impl Into<String>,
    hint: &str,
    suspicion: Option<Value>,
) -> CliOutcome<Value> {
    let mut payload = error_projection(req_id, error, state, hint);
    if let Some(suspicion) = suspicion {
        payload["suspicion"] = suspicion;
    }
    project_record(
        &payload,
        &lab2_projection_request(flags, Some(req_id.to_string())),
    )
    .map_err(|err| CliError::device(err.to_string()))
}

fn lab2_error_details_for_flags(
    flags: &FlagArgs,
    req_id: &str,
    error: &str,
    state: &str,
    hint: &str,
) -> CliOutcome<Value> {
    lab2_error_payload_for_flags(flags, req_id, error, state, hint, None)
}

fn lab2_command_contracts() -> Vec<Lab2CommandContract> {
    vec![
        Lab2CommandContract {
            name: "observe",
            summary: "read one frame, detect page, evaluate visible targets, and list navigation actions",
            required: &["--scene <png> or --capture"],
            optional: &[
                "--targets <id,id>",
                "--with-frame <path>",
                "--require-fresh",
                "--fields <field,field>",
                "--verbose",
                "--pretty",
                "--test-capture-delay-ms <ms> (test-only scene delay)",
            ],
            output_fields: &[
                "req_id",
                "state",
                "page",
                "targets",
                "actions",
                "frame_age_ms",
                "backend",
            ],
            requires_lease: false,
        },
        Lab2CommandContract {
            name: "do",
            summary: "guard a semantic target, derive the click point, optionally execute it, and observe once",
            required: &["<target>", "--scene <png> or --capture"],
            optional: &[
                "--dry-run",
                "--allow-destructive",
                "--destructive",
                "--priority <normal|high>",
                "--lease-id <id>",
                "--fields <field,field>",
                "--no-wait",
                "--recovery-timeout-ms <ms>",
                "--recovery-poll-ms <ms>",
                "--test-capture-delay-ms <ms> (test-only scene delay)",
            ],
            output_fields: &[
                "req_id",
                "reco_id",
                "action_id",
                "actual_click",
                "guard_result",
                "observation",
                "ledger",
            ],
            requires_lease: true,
        },
        Lab2CommandContract {
            name: "ensure",
            summary: "route from the current page to a requested page through guarded actions",
            required: &[
                "<page> or --page <page> or --to <page>",
                "--scene <png> or --capture",
            ],
            optional: &[
                "--dry-run",
                "--allow-destructive",
                "--lease-id <id>",
                "--step-timeout-ms <ms>",
                "--poll-ms <ms>",
                "--no-wait",
                "--recovery-timeout-ms <ms>",
                "--recovery-poll-ms <ms>",
                "--test-capture-delay-ms <ms> (test-only scene delay)",
            ],
            output_fields: &["req_id", "state", "page", "to", "route", "steps", "ledger"],
            requires_lease: true,
        },
        Lab2CommandContract {
            name: "wait",
            summary: "wait for a page or target stability using the shared ROI stability comparator",
            required: &[
                "--page <page> or --stable <target>",
                "--scene <png> or --capture",
            ],
            optional: &[
                "--timeout-ms <ms>",
                "--poll-ms <ms>",
                "--fields <field,field>",
                "--test-capture-delay-ms <ms> (test-only scene delay)",
            ],
            output_fields: &[
                "req_id",
                "wf_id",
                "state",
                "page",
                "target",
                "elapsed_ms",
                "ledger",
            ],
            requires_lease: false,
        },
        Lab2CommandContract {
            name: "lab receipt",
            summary: "load all ledger records tied to a req_id",
            required: &["--req <req_id>", "--run-root <path> or config run_root"],
            optional: &[],
            output_fields: &["req_id", "ledger_count", "record_count", "records"],
            requires_lease: false,
        },
        Lab2CommandContract {
            name: "lab evidence",
            summary: "list debug evidence refs tied to an evidence id",
            required: &["--id <evidence_id>", "--run-root <path> or config run_root"],
            optional: &[],
            output_fields: &["evidence_id", "count", "evidence"],
            requires_lease: false,
        },
        Lab2CommandContract {
            name: "lab arbitrator",
            summary: "retired compatibility command; Runtime scheduler is the only lease authority",
            required: &[],
            optional: &[],
            output_fields: &["error"],
            requires_lease: false,
        },
    ]
}

fn command_contract_summary(contract: Lab2CommandContract) -> Value {
    json!({
        "command": contract.name,
        "summary": contract.summary,
        "requires_lease": contract.requires_lease,
        "required": contract.required,
        "optional": contract.optional,
        "output_fields": contract.output_fields
    })
}

fn command_contract_schema(contract: Lab2CommandContract) -> Value {
    json!({
        "schema_version": "actingcommand.lab2.command_shape.v0.1",
        "command": contract.name,
        "summary": contract.summary,
        "stdout": {
            "format": "single_line_json",
            "ansi": false,
            "image_bytes": false
        },
        "requires_lease": contract.requires_lease,
        "parameters": {
            "required": contract.required,
            "optional": contract.optional
        },
        "output": {
            "shape": "json_object",
            "fields": contract.output_fields
        }
    })
}

fn recognition_engine_capabilities() -> Value {
    let metrics = [
        MatchMetric::CrossCorrelationNormalized,
        MatchMetric::CorrelationCoefficientNormalized,
    ]
    .into_iter()
    .map(match_metric_name)
    .collect::<Vec<_>>();
    json!({
        "template_matching": {
            "supported_metrics": metrics,
            "families": [
                {"id": "ncc", "implemented_by": "ccorr_normed score normalization"},
                {"id": "ccoeff", "implemented_by": "ccoeff_normed"}
            ],
            "unsupported": [
                {"id": "masked_template_match", "reason": "not implemented in current recognition engine"},
                {"id": "count_match", "reason": "resource-level count semantics are not implemented in Lab-2 CLI"}
            ]
        },
        "color": {"rgb_mean_distance": true},
        "ocr": {"default_screen_state": false, "explicit_command_required": true}
    })
}

fn lab2_config_instances(config: &UserConfig) -> Vec<Value> {
    config
        .instances
        .iter()
        .map(|(id, instance)| {
            json!({
                "id": id,
                "game": instance.game,
                "server": instance.server,
                "serial_configured": instance.serial.is_some(),
                "package_configured": instance.package.is_some(),
                "capture_backend": instance.capture_backend,
                "touch_backend": instance.touch_backend
            })
        })
        .collect()
}

fn lab2_error_code_table() -> Value {
    json!([
        {"code": "transient", "meaning": "retryable issue; retry may be appropriate if bounded and logged"},
        {"code": "recovering", "meaning": "runtime recovery is active; write actions should wait or fail visibly"},
        {"code": "resource_drift", "meaning": "resource or page expectation drift; stop and inspect rather than retry blindly"},
        {"code": "lease_held", "meaning": "another holder owns the write lease"},
        {"code": "queue_full", "meaning": "single-slot degraded queue is full"},
        {"code": "fatal", "meaning": "non-recoverable fault; fail loud"},
        {"code": "target_not_visible", "meaning": "guard target did not pass recognition"},
        {"code": "current_page_unknown", "meaning": "current page was not matched before routing"}
    ])
}

fn escape_toolbox_groups() -> Value {
    json!([
        {
            "group": "evidence",
            "commands": ["capture --with-frame", "detect-page --all", "locate --template", "read-text --region", "session journal"]
        },
        {
            "group": "recovery",
            "commands": ["session recover", "session self-heal-plan", "session capture-policy", "session instance app restart"]
        },
        {
            "group": "runtime_control",
            "commands": ["status", "session request status"]
        }
    ])
}

fn load_lab2_scene(_global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Lab2Scene> {
    if let Some(scene_path) = flags.optional_path("--scene") {
        let test_delay = flags
            .optional("--test-capture-delay-ms")
            .filter(|value| value != "true")
            .map(|value| {
                value
                    .parse::<u64>()
                    .map(Duration::from_millis)
                    .map_err(|err| {
                        CliError::usage(format!("invalid --test-capture-delay-ms '{value}': {err}"))
                    })
            })
            .transpose()?;
        if let Some(delay) = test_delay {
            thread::sleep(delay);
        }
        let png = fs::read(&scene_path).map_err(|err| {
            CliError::device(format!("failed to read {}: {err}", scene_path.display()))
        })?;
        let scene = Scene::from_png(&png).map_err(|err| CliError::device(err.to_string()))?;
        return Ok(Lab2Scene {
            scene,
            backend: if test_delay.is_some() {
                "test_stub_capture".to_string()
            } else {
                "scene_file".to_string()
            },
            source: json!({
                "kind": if test_delay.is_some() { "test_stub_capture" } else { "scene" },
                "path": scene_path.display().to_string(),
                "delay_ms": test_delay.map(|delay| delay.as_millis() as u64)
            }),
            frame_age_ms: 0,
            png: Some(png),
        });
    }
    Err(CliError::usage(
        "offline Lab-2 CLI verbs require --scene <png>; use --capture for Runtime-backed mode",
    ))
}

fn write_frame_if_requested(flags: &FlagArgs, scene: &Lab2Scene) -> CliOutcome<Option<PathBuf>> {
    let Some(path) = flags.optional_path("--with-frame") else {
        return Ok(None);
    };
    let png = scene.png.as_ref().ok_or_else(|| {
        CliError::device(
            "--with-frame requires a PNG-backed frame; selected capture backend returned raw pixels",
        )
    })?;
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|err| {
            CliError::device(format!("failed to create {}: {err}", parent.display()))
        })?;
    }
    fs::write(&path, png)
        .map_err(|err| CliError::device(format!("failed to write {}: {err}", path.display())))?;
    Ok(Some(path))
}

fn observe_targets(
    evaluator: &RecognitionEvaluator,
    scene: &Scene,
    flags: &FlagArgs,
    outcome: &PageDetectionOutcome,
) -> CliOutcome<Vec<Value>> {
    let requested = target_list(flags);
    if !requested.is_empty() {
        return requested
            .into_iter()
            .map(|target| {
                guard_evaluable_target(evaluator, &target, "observe --targets")?;
                let evaluation = evaluator
                    .evaluate_target(scene, &target)
                    .map_err(|err| CliError::usage(err.to_string()))?;
                Ok(json!({
                    "id": target,
                    "passed": evaluation.passed,
                    "score": target_score(&evaluation),
                    "evaluation": target_eval_json(&evaluation)
                }))
            })
            .collect();
    }
    let mut targets = Vec::new();
    if let Some(page) = outcome
        .evaluations
        .iter()
        .find(|evaluation| evaluation.page_id == outcome.page)
    {
        for target in &page.target_results {
            targets.push(json!({
                "id": target.target_id,
                "passed": target.passed,
                "score": Value::Null,
                "message": target.message
            }));
        }
    }
    Ok(targets)
}

fn observe_actions(graph: &NavigationGraph, outcome: &PageDetectionOutcome) -> Vec<Value> {
    graph
        .edges
        .iter()
        .filter(|edge| outcome.matched && edge.from_page == outcome.page)
        .map(navigation_edge_json)
        .collect::<Vec<_>>()
}

fn lab2_page_candidates(outcome: &PageDetectionOutcome) -> Vec<Value> {
    let mut candidates = outcome
        .evaluations
        .iter()
        .map(|evaluation| {
            let passed = evaluation.required_passed
                + evaluation.any_of_passed
                + evaluation.optional_passed
                + evaluation.forbidden_passed;
            let total = evaluation.required_total
                + evaluation.any_of_total
                + evaluation.optional_total
                + evaluation.forbidden_total;
            json!({
                "id": evaluation.page_id,
                "matched": evaluation.matched,
                "passed": evaluation.matched,
                "score": if total == 0 { 0.0 } else { passed as f64 / total as f64 },
                "required_passed": evaluation.required_passed,
                "required_total": evaluation.required_total,
                "forbidden_passed": evaluation.forbidden_passed,
                "message": evaluation.message
            })
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .get("score")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            .partial_cmp(&left.get("score").and_then(Value::as_f64).unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    candidates.truncate(4);
    candidates
}

fn lab2_observation_suspicion(outcome: &PageDetectionOutcome, frame_age_ms: u64) -> Option<Value> {
    let candidates = lab2_page_candidates(outcome);
    if !outcome.matched {
        return Some(low_margin_suspicion(candidates));
    }
    if candidates.len() >= 2 {
        let top = candidates[0]
            .get("score")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let runner_up = candidates[1]
            .get("score")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        if (top - runner_up).abs() < 0.05 {
            return Some(low_margin_suspicion(candidates));
        }
    }
    if frame_age_ms > 1_000 {
        return Some(stale_frame_suspicion(frame_age_ms));
    }
    None
}

fn wait_for_page(
    global: &GlobalOptions,
    flags: &FlagArgs,
    evaluator: &RecognitionEvaluator,
    detector: &PageDetector,
    ids: WaitIds<'_>,
    page: &str,
    timing: WaitTiming,
) -> CliOutcome<Value> {
    let started = Instant::now();
    loop {
        let scene = load_lab2_scene(global, flags)?;
        let outcome = detect_current_page(evaluator, detector, &scene.scene)?;
        if outcome.matched && outcome.page == page {
            return Ok(json!({
                "req_id": ids.req_id,
                "wf_id": ids.wf_id,
                "state": "arrived",
                "page": outcome.page,
                "matched": true,
                "elapsed_ms": started.elapsed().as_millis() as u64,
                "frame_age_ms": scene.frame_age_ms,
                "backend": scene.backend
            }));
        }
        if started.elapsed() >= timing.timeout {
            let details = lab2_error_payload_for_flags(
                flags,
                ids.req_id,
                "transient",
                outcome.page,
                "retry-or-observe-current-page",
                None,
            )?;
            return Err(CliError::safety_blocked(
                "wait_timeout",
                format!("wait timed out before page '{page}' became current"),
                &["page_wait"],
            )
            .with_details(details));
        }
        thread::sleep(timing.poll);
    }
}

fn wait_for_stable_target(
    global: &GlobalOptions,
    flags: &FlagArgs,
    evaluator: &RecognitionEvaluator,
    ids: WaitIds<'_>,
    target: &str,
    timing: WaitTiming,
) -> CliOutcome<Value> {
    guard_evaluable_target(evaluator, target, "wait --stable")?;
    let started = Instant::now();
    let first = load_lab2_scene(global, flags)?;
    let mut previous = evaluator
        .evaluate_target(&first.scene, target)
        .map_err(|err| CliError::usage(err.to_string()))?;
    if !previous.passed {
        let details = lab2_error_payload_for_flags(
            flags,
            ids.req_id,
            "resource_drift",
            "unstable",
            "observe-current-page-and-refresh-stable-target",
            Some(guard_reject_suspicion(target, &previous.message)),
        )?;
        return Err(CliError::safety_blocked(
            "stable_target_not_visible",
            format!("stable target '{target}' did not pass baseline guard"),
            &["stable_target"],
        )
        .with_details(details));
    }
    loop {
        let next_scene = load_lab2_scene(global, flags)?;
        let current = evaluator
            .evaluate_target(&next_scene.scene, target)
            .map_err(|err| CliError::usage(err.to_string()))?;
        if actingcommand_lab::target_evaluations_stable_for_wait(&previous, &current) {
            return Ok(json!({
                "req_id": ids.req_id,
                "wf_id": ids.wf_id,
                "state": "stable",
                "target": target,
                "elapsed_ms": started.elapsed().as_millis() as u64,
                "frame_age_ms": next_scene.frame_age_ms,
                "backend": next_scene.backend,
                "evaluation": target_eval_json(&current)
            }));
        }
        if started.elapsed() >= timing.timeout {
            let details = lab2_error_details_for_flags(
                flags,
                ids.req_id,
                "transient",
                "unstable",
                "retry-or-switch-backend",
            )?;
            return Err(CliError::safety_blocked(
                "wait_timeout",
                format!("wait timed out before target '{target}' became stable"),
                &["stable_target"],
            )
            .with_details(details));
        }
        previous = current;
        thread::sleep(timing.poll);
    }
}

fn derive_lab2_click_rect(
    evaluator: &RecognitionEvaluator,
    target: &str,
    declared: PackRect,
    evaluation: &TargetEvaluation,
) -> CliOutcome<Lab2ClickRect> {
    let Some(anchor) = evaluator
        .get_template_anchor_rect(target)
        .map_err(|err| CliError::usage(err.to_string()))?
    else {
        return Ok(Lab2ClickRect {
            kind: "target_rect_center",
            rect: declared,
            derivation: json!({"mode": "declared_rect"}),
        });
    };
    let Some(template) = evaluation.template else {
        return Ok(Lab2ClickRect {
            kind: "target_rect_center",
            rect: declared,
            derivation: json!({"mode": "declared_rect", "reason": "non_template_evaluation"}),
        });
    };
    let matched = PackRect {
        x: template.x,
        y: template.y,
        width: template.width,
        height: template.height,
    };
    let rect = derive_absolute_coordinate_rect_from_match("lab2_do", declared, anchor, matched)?;
    Ok(Lab2ClickRect {
        kind: "target_rect_center_live_match",
        rect,
        derivation: json!({
            "mode": "matched_template_delta",
            "expected_rect": rect_json(anchor),
            "matched_rect": rect_json(matched)
        }),
    })
}

fn reject_lab2_destructive_click_overlap(
    target: &str,
    page: &str,
    click: PackRect,
    graph: &NavigationGraph,
) -> CliOutcome<()> {
    if graph.destructive_clicks.iter().any(|destructive| {
        destructive
            .page
            .as_deref()
            .is_none_or(|expected| expected == page)
            && rects_intersect(click, destructive.rect)
    }) {
        return Err(CliError::safety_blocked(
            "semantic_action_requires_destructive_opt_in",
            format!(
                "target '{target}' click rect overlaps a destructive_actions region and requires --allow-destructive"
            ),
            &["destructive_actions", "allow_destructive"],
        ));
    }
    Ok(())
}

fn target_list(flags: &FlagArgs) -> Vec<String> {
    flags
        .optional("--targets")
        .filter(|value| value != "true")
        .into_iter()
        .flat_map(|value| split_csv(&value))
        .collect()
}

fn fields_from_flags(flags: &FlagArgs) -> BTreeSet<String> {
    flags
        .optional("--fields")
        .filter(|value| value != "true")
        .into_iter()
        .flat_map(|value| split_csv(&value))
        .collect()
}

fn wait_page_target(flags: &FlagArgs) -> Option<String> {
    flags
        .optional("--page")
        .filter(|value| value != "true")
        .or_else(|| flags.positionals.first().cloned())
}

fn ensure_target_page(flags: &FlagArgs) -> CliOutcome<String> {
    flags
        .optional("--to")
        .filter(|value| value != "true")
        .or_else(|| flags.optional("--page").filter(|value| value != "true"))
        .or_else(|| flags.positionals.first().cloned())
        .ok_or_else(|| CliError::usage("ensure requires <page>, --page <page>, or --to <page>"))
}

fn guard_evaluable_target(
    evaluator: &RecognitionEvaluator,
    target: &str,
    command: &str,
) -> CliOutcome<()> {
    if evaluator
        .target_kind(target)
        .map_err(|err| CliError::usage(err.to_string()))?
        == TargetKind::ClickOnly
    {
        return Err(CliError::usage(format!(
            "{command} requires a visually evaluatable target; '{target}' is click-only"
        )));
    }
    Ok(())
}

fn target_score(evaluation: &TargetEvaluation) -> Value {
    if let Some(template) = evaluation.template {
        json!(template.score)
    } else if let Some(color) = evaluation.color {
        json!(1.0_f32 - color.distance)
    } else {
        Value::Null
    }
}

fn lab2_instance(global: &GlobalOptions, flags: &FlagArgs) -> String {
    flags
        .optional("--instance")
        .filter(|value| value != "true")
        .or_else(|| global.instance.clone())
        .unwrap_or_else(|| "default".to_string())
}

fn system_time_age_ms(timestamp: SystemTime) -> u64 {
    timestamp
        .elapsed()
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
