// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_arbitrator::{
    ArbitrationDecision, DegradedArbitrator, RequestEnvelope, RequestPriority, RequestSource,
    RequestVerb,
};
use actingcommand_ledger::{
    IdIssuer, IdKind, LabLedger, LedgerRecord, LedgerRecordKind, ProjectionRequest,
    ProjectionVerbosity, SessionHeader, error_projection, guard_reject_suspicion, project_record,
};
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

pub(crate) fn run_observe(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let ids = Lab2Ids::new();
    let instance = lab2_instance(global, &flags);
    let arbitration = admit_lab2_request(
        &ids,
        instance.clone(),
        RequestVerb::Observe,
        json!({"targets": target_list(&flags)}),
        &flags,
    )?;
    let config = read_user_config()?;
    let (evaluator, detector) = load_semantic_detector(global, &config, &flags)?;
    let loaded_scene = load_lab2_scene(global, &flags)?;
    let outcome = detect_current_page(&evaluator, &detector, &loaded_scene.scene)?;
    let frame_path = write_frame_if_requested(&flags, &loaded_scene)?;
    let targets = observe_targets(&evaluator, &loaded_scene.scene, &flags, &outcome)?;
    let actions = observe_actions(global, &config, &flags, &outcome)?;
    let mut payload = json!({
        "req_id": ids.req_id,
        "state": "observed",
        "instance": instance,
        "page": outcome.page,
        "matched": outcome.matched,
        "standby": outcome.standby,
        "frame_age_ms": loaded_scene.frame_age_ms,
        "backend": loaded_scene.backend,
        "frame_source": loaded_scene.source,
        "targets": targets,
        "actions": actions,
        "arbitration": arbitration_json(&arbitration.decision),
    });
    if let Some(path) = frame_path {
        payload["frame_path"] = json!(path.display().to_string());
    }
    finish_lab2_response(
        global,
        &flags,
        &ids.req_id,
        &instance,
        payload,
        arbitration.ledger_records,
    )
}

pub(crate) fn run_do(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let ids = Lab2Ids::new();
    let target = target_argument(&flags, "do")?;
    let instance = lab2_instance(global, &flags);
    let dry_run = global.dry_run || flags.bool("--dry-run");
    let allow_destructive = flags.bool("--allow-destructive");
    if flags.bool("--destructive") && !allow_destructive {
        let details = lab2_error_details(
            &ids.req_id,
            "resource_drift",
            "blocked",
            "rerun-with---allow-destructive-if-this-action-is-intended",
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

    let arbitration = admit_lab2_request(
        &ids,
        instance.clone(),
        RequestVerb::Do,
        json!({"target": target, "dry_run": dry_run}),
        &flags,
    )?;
    let config = read_user_config()?;
    let (evaluator, detector) = load_semantic_detector(global, &config, &flags)?;
    guard_evaluable_target(&evaluator, &target, "do")?;
    let loaded_scene = load_lab2_scene(global, &flags)?;
    let before = detect_current_page(&evaluator, &detector, &loaded_scene.scene)?;
    let evaluation = evaluator
        .evaluate_target(&loaded_scene.scene, &target)
        .map_err(|err| CliError::usage(err.to_string()))?;
    if !evaluation.passed {
        let details = lab2_error_payload(
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
    let point = rect_center(click)?;
    let action_id = ids.issue(IdKind::Action);
    let device = if dry_run {
        json!({"executed": false, "mode": "dry_run"})
    } else {
        send_semantic_tap(global, &config, point)?
    };
    let after = if dry_run {
        before
    } else {
        let after_scene = load_lab2_scene(global, &flags)?;
        detect_current_page(&evaluator, &detector, &after_scene.scene)?
    };
    let payload = json!({
        "req_id": ids.req_id,
        "action_id": action_id,
        "state": if dry_run { "planned" } else { "sent" },
        "instance": instance,
        "executed": !dry_run,
        "target": target,
        "page": after.page,
        "frame_age_ms": loaded_scene.frame_age_ms,
        "backend": loaded_scene.backend,
        "actual_click": {
            "kind": "target_rect_center",
            "rect": rect_json(click),
            "point": point_json(point)
        },
        "guard_result": {
            "target": target,
            "passed": true,
            "evaluation": target_eval_json(&evaluation)
        },
        "observation": page_detection_json(&after),
        "device": device,
        "arbitration": arbitration_json(&arbitration.decision),
    });
    finish_lab2_response(
        global,
        &flags,
        &ids.req_id,
        &instance,
        payload,
        arbitration.ledger_records,
    )
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
    let arbitration = admit_lab2_request(
        &ids,
        instance.clone(),
        RequestVerb::Ensure,
        json!({"to": to, "dry_run": dry_run}),
        &flags,
    )?;
    let config = read_user_config()?;
    let (evaluator, detector) = load_semantic_detector(global, &config, &flags)?;
    let graph = load_navigation_graph(global, &config, &flags)?;
    let scene = load_lab2_scene(global, &flags)?;
    let start = detect_current_page(&evaluator, &detector, &scene.scene)?;
    let target_page = canonical_navigation_page(&graph, &to);
    if start.matched && start.page == target_page {
        let payload = json!({
            "req_id": ids.req_id,
            "state": "already_at_target",
            "instance": instance,
            "executed": false,
            "page": start.page,
            "to": target_page,
            "route": [],
            "frame_age_ms": scene.frame_age_ms,
            "backend": scene.backend,
            "arbitration": arbitration_json(&arbitration.decision),
        });
        return finish_lab2_response(
            global,
            &flags,
            &ids.req_id,
            &instance,
            payload,
            arbitration.ledger_records,
        );
    }
    if !start.matched {
        let details = lab2_error_payload(
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
    if dry_run {
        let payload = json!({
            "req_id": ids.req_id,
            "state": "planned",
            "instance": instance,
            "executed": false,
            "page": start.page,
            "to": target_page,
            "route": route_json,
            "frame_age_ms": scene.frame_age_ms,
            "backend": scene.backend,
            "arbitration": arbitration_json(&arbitration.decision),
        });
        return finish_lab2_response(
            global,
            &flags,
            &ids.req_id,
            &instance,
            payload,
            arbitration.ledger_records,
        );
    }

    let step_timeout = parse_optional_duration_ms(&flags, "--step-timeout-ms", 5_000)?;
    let poll = parse_optional_duration_ms(&flags, "--poll-ms", 500)?;
    let execution = NavigationExecutionContext {
        global,
        flags: &flags,
        config: &config,
        evaluator: &evaluator,
        detector: &detector,
        step_timeout,
        poll,
    };
    let (steps, arrived) = execute_navigation_route(&execution, start.page.clone(), route)?;
    let payload = json!({
        "req_id": ids.req_id,
        "state": "arrived",
        "instance": instance,
        "executed": true,
        "from": start.page,
        "page": arrived,
        "to": target_page,
        "route": route_json,
        "steps": steps,
        "arbitration": arbitration_json(&arbitration.decision),
    });
    finish_lab2_response(
        global,
        &flags,
        &ids.req_id,
        &instance,
        payload,
        arbitration.ledger_records,
    )
}

pub(crate) fn run_wait(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let ids = Lab2Ids::new();
    let wf_id = ids.issue(IdKind::Wf);
    let instance = lab2_instance(global, &flags);
    let arbitration = admit_lab2_request(
        &ids,
        instance.clone(),
        RequestVerb::Wait,
        json!({"page": flags.optional("--page"), "stable": flags.optional("--stable")}),
        &flags,
    )?;
    let config = read_user_config()?;
    let (evaluator, detector) = load_semantic_detector(global, &config, &flags)?;
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
    payload["arbitration"] = arbitration_json(&arbitration.decision);
    finish_lab2_response(
        global,
        &flags,
        &ids.req_id,
        &instance,
        payload,
        arbitration.ledger_records,
    )
}

fn admit_lab2_request(
    ids: &Lab2Ids,
    instance: String,
    verb: RequestVerb,
    payload: Value,
    flags: &FlagArgs,
) -> CliOutcome<actingcommand_arbitrator::ArbitrationOutcome> {
    let mut request = RequestEnvelope::new(
        ids.req_id.clone(),
        RequestSource::Cli,
        instance,
        verb,
        payload,
        current_unix_ms(),
    );
    request.allow_destructive = flags.bool("--allow-destructive");
    request.priority = match flags.optional("--priority").as_deref() {
        Some("high") => RequestPriority::High,
        Some("normal") | None => RequestPriority::Normal,
        Some(other) => {
            return Err(CliError::usage(format!(
                "unsupported --priority '{other}', expected normal or high"
            )));
        }
    };
    if let Some(deadline) = flags
        .optional("--queue-deadline-ms")
        .filter(|value| value != "true")
    {
        request.queue_deadline_ms = Some(deadline.parse::<u64>().map_err(|err| {
            CliError::usage(format!("invalid --queue-deadline-ms '{deadline}': {err}"))
        })?);
    }
    let mut arbitrator = DegradedArbitrator::new(IdIssuer::new());
    arbitrator
        .admit(request, current_unix_ms())
        .map_err(|err| CliError::device(err.to_string()))
}

fn finish_lab2_response(
    global: &GlobalOptions,
    flags: &FlagArgs,
    req_id: &str,
    instance: &str,
    payload: Value,
    mut records: Vec<LedgerRecord>,
) -> CliOutcome<Value> {
    let mut payload = payload;
    payload["ledger"] = write_lab2_ledger(global, instance, req_id, &payload, &mut records)?;
    project_lab2_payload(&payload, flags)
}

fn write_lab2_ledger(
    global: &GlobalOptions,
    instance: &str,
    req_id: &str,
    payload: &Value,
    records: &mut Vec<LedgerRecord>,
) -> CliOutcome<Value> {
    let config = read_user_config()?;
    let Some(run_root) = effective_run_root(global, &config) else {
        return Ok(json!({"written": false, "reason": "run_root_not_configured"}));
    };
    let game = global.game.clone().unwrap_or_else(|| "unknown".to_string());
    let server = global
        .server
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let mut ledger = LabLedger::create(
        &run_root,
        &format!("lab2-{req_id}"),
        SessionHeader::new(RUNTIME_VERSION, game, server, instance),
    )
    .map_err(|err| CliError::device(err.to_string()))?;
    for record in records.drain(..) {
        ledger
            .append(record)
            .map_err(|err| CliError::device(err.to_string()))?;
    }
    ledger
        .append(LedgerRecord::new(
            LedgerRecordKind::Receipt,
            Some(req_id.to_string()),
            payload.clone(),
        ))
        .map_err(|err| CliError::device(err.to_string()))?;
    Ok(json!({
        "written": true,
        "path": ledger.ledger_path().display().to_string()
    }))
}

fn project_lab2_payload(payload: &Value, flags: &FlagArgs) -> CliOutcome<Value> {
    let verbosity = if flags.bool("--pretty") {
        ProjectionVerbosity::Debug
    } else if flags.bool("--verbose") {
        ProjectionVerbosity::Normal
    } else {
        ProjectionVerbosity::Min
    };
    let request = ProjectionRequest {
        verbosity,
        fields: fields_from_flags(flags),
        evidence_id: payload
            .get("req_id")
            .and_then(Value::as_str)
            .map(str::to_string),
    };
    project_record(payload, &request).map_err(|err| CliError::device(err.to_string()))
}

fn lab2_error_payload(
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
        &ProjectionRequest::min().with_evidence_id(req_id.to_string()),
    )
    .map_err(|err| CliError::device(err.to_string()))
}

fn lab2_error_details(req_id: &str, error: &str, state: &str, hint: &str) -> CliOutcome<Value> {
    lab2_error_payload(req_id, error, state, hint, None)
}

fn load_lab2_scene(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Lab2Scene> {
    if let Some(scene_path) = flags.optional_path("--scene") {
        let png = fs::read(&scene_path).map_err(|err| {
            CliError::device(format!("failed to read {}: {err}", scene_path.display()))
        })?;
        let scene = Scene::from_png(&png).map_err(|err| CliError::device(err.to_string()))?;
        return Ok(Lab2Scene {
            scene,
            backend: "scene_file".to_string(),
            source: json!({"kind": "scene", "path": scene_path.display().to_string()}),
            frame_age_ms: 0,
            png: Some(png),
        });
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
        let frame_age_ms = system_time_age_ms(frame.captured_at);
        let backend = frame.backend_name.as_str().to_string();
        let png = frame.original_png.clone();
        let scene = scene_from_frame(&frame)?;
        return Ok(Lab2Scene {
            scene,
            backend,
            source: json!({
                "kind": "capture",
                "capture_backend_used": frame.backend_name.as_str(),
                "capture_backend_attempts": captured.attempts,
                "freshness": captured.freshness
            }),
            frame_age_ms,
            png,
        });
    }
    Err(CliError::usage(
        "Lab-2 CLI verbs require --scene <png> or --capture",
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

fn observe_actions(
    global: &GlobalOptions,
    config: &UserConfig,
    flags: &FlagArgs,
    outcome: &PageDetectionOutcome,
) -> CliOutcome<Vec<Value>> {
    let graph = load_navigation_graph(global, config, flags)?;
    Ok(graph
        .edges
        .iter()
        .filter(|edge| outcome.matched && edge.from_page == outcome.page)
        .map(navigation_edge_json)
        .collect::<Vec<_>>())
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
            let details = lab2_error_payload(
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
        let details = lab2_error_payload(
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
        if lab_run::target_evaluations_stable_for_wait(&previous, &current) {
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
            let details = lab2_error_details(
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

fn arbitration_json(decision: &ArbitrationDecision) -> Value {
    json!({
        "decision": decision.as_str(),
        "details": decision
    })
}
