// SPDX-License-Identifier: AGPL-3.0-only

use super::{load_evaluator_and_detector, page_error, task_error};
use actingcommand_device::{
    CaptureBackend, DeviceError, DeviceResult, Frame, InputBackend, MaaTouchValidationConfig,
    PixelFormat, ScreencapBackend, SelectedTouchBackend, TouchBackendConfig,
    TouchBackendDiagnostics, TouchBackendName, combine_operation_and_close, create_touch_backend,
};
use actingcommand_execution_kernel::{
    ProbeClickEffect, ProbeDecisionLoop, ProbeReferenceOverrides, ProbeStepDecision,
    ResourcePolicy, ResourcePolicyKind, load_probe_plan_from_json_str,
};
use actingcommand_page_detector::{PageDetector, require_all_page_evaluations};
use actingcommand_recognition::{Rect as RecognitionRect, Scene, ScenePixelFormat};
use actingcommand_recognition_pack::{PackRect, RecognitionEvaluator};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DEFAULT_CLICK_RECT_RADIUS: i32 = 20;
const DEFAULT_FORBIDDEN_RADIUS: i32 = 20;
const DEFAULT_EXPECT_TIMEOUT_MS: u64 = 3000;
const DEFAULT_EXPECT_INTERVAL_MS: u64 = 100;
pub const DEFAULT_CHECKPOINT_FRAMES: usize = 8;
const MAX_POLL_FRAMES: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeRunOptions {
    pub pack: PathBuf,
    pub pack_root: PathBuf,
    pub pages: PathBuf,
    pub probe: PathBuf,
    pub run_root: PathBuf,
    pub navigation: Option<PathBuf>,
    pub capture: bool,
    pub scene: Option<PathBuf>,
    pub checkpoint_frames: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActualClickPoint {
    seed: u64,
    algorithm: &'static str,
    rect: PackRect,
    x: i32,
    y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActualInputAction {
    Tap(ActualClickPoint),
    Drag {
        from: ActualClickPoint,
        to: ActualClickPoint,
        duration_ms: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ClickSpace {
    width: i32,
    height: i32,
}

#[derive(Debug, Clone, Deserialize)]
struct NavigationFile {
    coordinate_space: NavigationCoordinateSpace,
    #[serde(default)]
    control_points: Vec<NavigationControlPoint>,
    #[serde(default)]
    pages: Vec<NavigationPage>,
    #[serde(default)]
    navigation: Vec<NavigationRoute>,
    #[serde(default)]
    destructive_actions: Vec<NavigationDestructiveAction>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct NavigationCoordinateSpace {
    width: i32,
    height: i32,
}

#[derive(Debug, Clone, Deserialize)]
struct NavigationControlPoint {
    name: String,
    point: [i32; 2],
}

#[derive(Debug, Clone, Deserialize)]
struct NavigationRoute {
    id: String,
    #[serde(default)]
    from_page: String,
    #[serde(default)]
    to_page: String,
    click: NavigationClick,
}

#[derive(Debug, Clone, Deserialize)]
struct ArrivalAnchor {
    template: String,
    #[serde(default)]
    threshold: Option<f32>,
    #[serde(default)]
    region: Option<PackRect>,
}

#[derive(Debug, Clone, Deserialize)]
struct NavigationPage {
    id: String,
    #[serde(default)]
    anchors: Vec<NavigationAnchor>,
}

#[derive(Debug, Clone, Deserialize)]
struct NavigationAnchor {
    kind: String,
    template_path: String,
    #[serde(default)]
    region: String,
    #[serde(default)]
    threshold: Option<f32>,
}

#[derive(Debug, Clone, Deserialize)]
struct NavigationClick {
    kind: String,
    #[serde(default)]
    point: String,
    #[serde(default)]
    rect: String,
    #[serde(default)]
    template_path: String,
    #[serde(default)]
    x: Option<i32>,
    #[serde(default)]
    y: Option<i32>,
    #[serde(default)]
    width: Option<i32>,
    #[serde(default)]
    height: Option<i32>,
    #[serde(default, rename = "from")]
    from_rect: Option<PackRect>,
    #[serde(default, rename = "to")]
    to_rect: Option<PackRect>,
    #[serde(default)]
    duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
struct NavigationDestructiveAction {
    id: String,
    #[serde(default)]
    page: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    point: String,
    #[serde(default)]
    rect: String,
}

#[derive(Debug, Clone)]
struct ForbiddenDestructivePoint {
    id: String,
    point: Option<[i32; 2]>,
    rect: Option<PackRect>,
    radius: Option<i32>,
}

#[derive(Debug)]
struct NavigationBridge {
    click_space: ClickSpace,
    overrides: ProbeReferenceOverrides,
    arrival_anchors: HashMap<String, ArrivalAnchor>,
    drag_targets: HashMap<String, NavigationDrag>,
    control_points: HashMap<String, [i32; 2]>,
    forbidden: Vec<ForbiddenDestructivePoint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NavigationDrag {
    from: PackRect,
    to: PackRect,
    duration_ms: u64,
}

struct OperationJournal {
    run_id: String,
    run_dir: PathBuf,
    events: PathBuf,
    frames: PathBuf,
    observations: PathBuf,
    checkpoints: PathBuf,
    pack_root: PathBuf,
}

#[derive(Default)]
struct ProbeRunState {
    executed: bool,
    click_count: usize,
    claims_executed: usize,
    regenerating_resource_actions_executed: usize,
    last_resource_kind: Option<String>,
    last_max_cost: Option<u32>,
    initial_page: Option<String>,
    last_before_page: Option<String>,
    last_after_page: Option<String>,
    final_page: Option<String>,
    frames: usize,
    observations: usize,
    checkpoint_count: usize,
    guard_failed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ProbeRunFinish {
    Completed,
    Blocked { message: String },
    PausedForReview { message: String },
}

impl ProbeRunFinish {
    fn result(&self) -> &str {
        match self {
            ProbeRunFinish::Completed => "completed",
            ProbeRunFinish::Blocked { .. } => "blocked",
            ProbeRunFinish::PausedForReview { .. } => "paused_for_review",
        }
    }

    fn message(&self) -> Option<&str> {
        match self {
            ProbeRunFinish::Completed => None,
            ProbeRunFinish::Blocked { message } | ProbeRunFinish::PausedForReview { message } => {
                Some(message)
            }
        }
    }
}

pub fn run_probe_command(
    config: MaaTouchValidationConfig,
    options: &ProbeRunOptions,
) -> DeviceResult<String> {
    if !options.capture {
        return Err(DeviceError::fatal("probe-run requires --capture"));
    }
    if options.scene.is_some() {
        return Err(DeviceError::fatal(
            "probe-run does not support --scene for click execution",
        ));
    }

    let probe_json = fs::read_to_string(&options.probe).map_err(|err| {
        DeviceError::fatal(format!(
            "failed to read probe plan {}: {err}",
            options.probe.display()
        ))
    })?;
    let journal = OperationJournal::create(options, &probe_json)?;
    let mut state = ProbeRunState::default();
    let result = execute_probe_run(config, options, &probe_json, &journal, &mut state);
    match result {
        Ok(finish) => {
            journal.event(
                "run_finished",
                json!({"result": finish.result(), "message": finish.message()}),
            )?;
            journal.summary(
                options,
                finish.result(),
                &state,
                finish.message().map(str::to_string),
            )?;
            Ok(format!(
                "run_id={}\nrun_dir={}\nprobe={}\nresult={}\nmessage={}\nexecuted={}\nclick_count={}\nsummary={}\nevents={}\n",
                journal.run_id,
                journal.run_dir.display(),
                options.probe.display(),
                finish.result(),
                finish.message().unwrap_or(""),
                state.executed,
                state.click_count,
                journal.run_dir.join("summary.json").display(),
                journal.events.display()
            ))
        }
        Err(err) => {
            let _ = journal.event(
                "run_failed",
                json!({"result": "failed", "error": err.to_string()}),
            );
            let _ = journal.summary(options, "failed", &state, Some(err.to_string()));
            Err(err)
        }
    }
}

fn execute_probe_run(
    config: MaaTouchValidationConfig,
    options: &ProbeRunOptions,
    probe_json: &str,
    journal: &OperationJournal,
    state: &mut ProbeRunState,
) -> DeviceResult<ProbeRunFinish> {
    journal.event(
        "run_started",
        json!({
            "run_id": journal.run_id,
            "destructive_allowed": false,
            "command": env::args().collect::<Vec<_>>().join(" ")
        }),
    )?;
    journal.record_inputs(options)?;

    let (evaluator, detector) =
        load_evaluator_and_detector(&options.pack, &options.pack_root, &options.pages)?;
    journal.event("pack_loaded", json!({"path": options.pack}))?;
    journal.event("pages_loaded", json!({"path": options.pages}))?;

    let probe_plan = load_probe_plan_from_json_str(probe_json).map_err(task_error)?;
    let probe_loop = ProbeDecisionLoop::new(probe_plan).map_err(task_error)?;
    journal.event(
        "probe_plan_loaded",
        json!({"id": probe_loop.plan().id, "steps": probe_loop.plan().steps.len()}),
    )?;

    let navigation = match &options.navigation {
        Some(path) => Some(load_navigation_bridge(path)?),
        None => None,
    };
    let overrides = navigation
        .as_ref()
        .map(|bridge| &bridge.overrides)
        .cloned()
        .unwrap_or_default();
    probe_loop
        .validate_with_overrides(&detector, &evaluator, &overrides)
        .map_err(task_error)?;
    journal.event("validate_done", json!({"status": "passed"}))?;

    let mut capture = ScreencapBackend::new(config.adb.clone(), config.target.clone());
    let before = capture_frame(&mut capture, journal, "000_before.png", "before")?;
    state.frames += 1;
    if let Some(finish) = maybe_checkpoint(options, journal, state, "initial_frame")? {
        return Ok(finish);
    }
    let mut scene = scene_from_frame(&before)?;
    let seed_base = run_seed();
    let mut backend = None::<SelectedTouchBackend>;
    let initial_page = detect_current_page(&detector, &evaluator, &scene)?;
    state.initial_page = initial_page.clone();
    state.final_page = initial_page.clone();
    journal.event(
        "page_detected",
        json!({"phase": "before_run", "page_id": initial_page}),
    )?;
    if state.initial_page.is_none()
        && let Some(wake_point) = navigation
            .as_ref()
            .and_then(|bridge| bridge.control_point("wake"))
    {
        journal.event(
            "standby_wake_started",
            json!({"point": {"x": wake_point[0], "y": wake_point[1]}}),
        )?;
        let backend_ref = ensure_touch_backend(&mut backend, &config, journal)?;
        if let Err(err) = backend_ref.tap(wake_point[0], wake_point[1]) {
            return close_backend_after_error(&mut backend, err);
        }
        journal.event(
            "standby_wake_done",
            json!({"point": {"x": wake_point[0], "y": wake_point[1]}}),
        )?;
        let after_wake = capture_frame(&mut capture, journal, "001_after_wake.png", "after_wake")?;
        state.frames += 1;
        scene = scene_from_frame(&after_wake)?;
        let after_wake_page = detect_current_page(&detector, &evaluator, &scene)?;
        state.initial_page = after_wake_page.clone();
        state.final_page = after_wake_page.clone();
        journal.event(
            "page_detected",
            json!({"phase": "after_wake", "page_id": after_wake_page}),
        )?;
    }

    for (step_index, step) in probe_loop.plan().steps.iter().enumerate() {
        journal.event(
            "step_started",
            json!({"step_id": step.id, "index": step_index}),
        )?;
        let decision = probe_loop
            .decide_step_with_known_page(
                step,
                &detector,
                &evaluator,
                &scene,
                &overrides,
                state.final_page.as_deref(),
            )
            .map_err(task_error)?;

        match decision {
            ProbeStepDecision::SkippedPageGuard {
                page_id,
                evaluation,
                ..
            } => {
                state.guard_failed = true;
                journal.event(
                    "step_skipped",
                    json!({
                        "step_id": step.id,
                        "reason": "page_guard_not_matched",
                        "page_id": page_id,
                        "message": evaluation.message
                    }),
                )?;
                return Ok(ProbeRunFinish::Blocked {
                    message: "page_guard_not_matched".to_string(),
                });
            }
            ProbeStepDecision::SkippedExternalPageGuard {
                page_id,
                current_page_id,
                ..
            } => {
                state.guard_failed = true;
                journal.event(
                    "step_skipped",
                    json!({
                        "step_id": step.id,
                        "reason": "external_page_guard_not_matched",
                        "page_id": page_id,
                        "current_page_id": current_page_id
                    }),
                )?;
                return Ok(ProbeRunFinish::Blocked {
                    message: "page_guard_not_matched".to_string(),
                });
            }
            ProbeStepDecision::DetectPage {
                page_id,
                evaluation,
                ..
            }
            | ProbeStepDecision::ObservePage {
                page_id,
                evaluation,
                ..
            } => {
                state.observations += 1;
                journal.event(
                    "observe_done",
                    json!({
                        "step_id": step.id,
                        "page_id": page_id,
                        "matched": evaluation.matched,
                        "message": evaluation.message
                    }),
                )?;
            }
            ProbeStepDecision::ObserveTargets { evaluations, .. } => {
                state.observations += evaluations.len();
                journal.event(
                    "observe_done",
                    json!({
                        "step_id": step.id,
                        "targets": evaluations.iter().map(|evaluation| {
                            json!({
                                "id": evaluation.id,
                                "passed": evaluation.passed,
                                "message": evaluation.message
                            })
                        }).collect::<Vec<_>>()
                    }),
                )?;
            }
            ProbeStepDecision::Click {
                target_id,
                click,
                effect,
                resource_policy,
                expect_after,
                ..
            } => {
                if let Some(finish) = maybe_pause_for_risky_effect(
                    effect,
                    &resource_policy,
                    journal,
                    state,
                    &step.id,
                )? {
                    return Ok(finish);
                }
                if state.click_count >= probe_loop.max_navigation_clicks() {
                    return Err(DeviceError::fatal(
                        "probe-run navigation click limit exceeded",
                    ));
                }
                let before_page = state.final_page.clone();
                state.last_before_page = before_page.clone();
                journal.event(
                    "safety_check_started",
                    json!({"step_id": step.id, "target_id": target_id, "before_page": before_page}),
                )?;
                let click_space = click_space_for_scene(&scene, navigation.as_ref())?;
                let input_action = actual_input_action(
                    click,
                    navigation
                        .as_ref()
                        .and_then(|bridge| bridge.drag_target(&target_id)),
                    seed_base ^ hash_text(&step.id),
                    click_space,
                )?;
                validate_input_action(&input_action, navigation.as_ref(), click_space)?;
                journal.event(
                    "safety_check_done",
                    json!({
                        "step_id": step.id,
                        "target_id": target_id,
                        "before_page": before_page,
                        "effect": format_effect(effect),
                        "forbidden_radius": DEFAULT_FORBIDDEN_RADIUS,
                        "actual_click_point": legacy_actual_click_json(input_action),
                        "actual_input": actual_input_json(input_action)
                    }),
                )?;

                let backend_ref = ensure_touch_backend(&mut backend, &config, journal)?;
                journal.event(
                    "click_started",
                    json!({
                        "step_id": step.id,
                        "target_id": target_id,
                        "before_page": before_page,
                        "actual_input": actual_input_json(input_action)
                    }),
                )?;
                if let Err(err) = execute_input_action(backend_ref, input_action) {
                    return close_backend_after_error(&mut backend, err);
                }
                state.executed = true;
                state.click_count += 1;
                record_effect_execution(effect, resource_policy.as_ref(), state);
                journal.event(
                    "click_done",
                    json!({
                        "step_id": step.id,
                        "target_id": target_id,
                        "effect": format_effect(effect),
                        "resource_policy": resource_policy_json(resource_policy.as_ref()),
                        "actual_click_point": legacy_actual_click_json(input_action),
                        "actual_input": actual_input_json(input_action)
                    }),
                )?;

                let poll_context = ArrivalPollContext {
                    journal,
                    detector: &detector,
                    evaluator: &evaluator,
                    navigation: navigation.as_ref(),
                    page_id: &expect_after.page_id,
                    step_id: &step.id,
                };
                let poll_timing = ArrivalPollTiming {
                    timeout_ms: expect_after.timeout_ms.unwrap_or(DEFAULT_EXPECT_TIMEOUT_MS),
                    interval_ms: expect_after
                        .interval_ms
                        .unwrap_or(DEFAULT_EXPECT_INTERVAL_MS),
                };
                let arrived = match poll_arrival(&mut capture, poll_context, poll_timing) {
                    Ok(arrived) => arrived,
                    Err(err) => return close_backend_after_error(&mut backend, err),
                };
                scene = arrived.scene;
                state.frames += arrived.frames;
                let after_page = Some(expect_after.page_id);
                state.last_after_page = after_page.clone();
                state.final_page = after_page.clone();
                journal.event(
                    "page_transition_recorded",
                    json!({
                        "step_id": step.id,
                        "target_id": target_id,
                        "before_page": before_page,
                        "after_page": after_page
                    }),
                )?;
                if let Some(finish) = maybe_checkpoint(options, journal, state, "frame_batch")? {
                    if let Some(mut backend) = backend.take() {
                        combine_operation_and_close(Ok(()), backend.close())?;
                    }
                    return Ok(finish);
                }
            }
        }
        journal.event("step_finished", json!({"step_id": step.id}))?;
    }

    if let Some(mut backend) = backend {
        let operation = Ok(());
        let close = backend.close();
        combine_operation_and_close(operation, close)?;
    }

    Ok(ProbeRunFinish::Completed)
}

fn close_backend_after_error<T>(
    backend: &mut Option<SelectedTouchBackend>,
    err: DeviceError,
) -> DeviceResult<T> {
    if let Some(mut backend) = backend.take() {
        combine_operation_and_close(Err(err), backend.close())?;
        unreachable!("combine_operation_and_close returned Ok for an operation error");
    }
    Err(err)
}

fn maybe_pause_for_risky_effect(
    effect: ProbeClickEffect,
    policy: &Option<ResourcePolicy>,
    journal: &OperationJournal,
    state: &mut ProbeRunState,
    step_id: &str,
) -> DeviceResult<Option<ProbeRunFinish>> {
    if effect == ProbeClickEffect::NavigationOnly {
        return Ok(None);
    }
    write_checkpoint_artifact(journal, state, "state_change_review_required")?;
    journal.event(
        "checkpoint",
        json!({
            "reason": "state_change_review_required",
            "step_id": step_id,
            "effect": format_effect(effect),
            "resource_policy": resource_policy_json(policy.as_ref())
        }),
    )?;
    Ok(Some(ProbeRunFinish::PausedForReview {
        message: "state_change_review_required".to_string(),
    }))
}

fn maybe_checkpoint(
    options: &ProbeRunOptions,
    journal: &OperationJournal,
    state: &mut ProbeRunState,
    reason: &str,
) -> DeviceResult<Option<ProbeRunFinish>> {
    if options.checkpoint_frames == 0 || state.frames < options.checkpoint_frames {
        return Ok(None);
    }
    write_checkpoint_artifact(journal, state, reason)?;
    journal.event(
        "checkpoint",
        json!({
            "reason": reason,
            "frames": state.frames,
            "click_count": state.click_count,
            "checkpoint_index": state.checkpoint_count
        }),
    )?;
    Ok(Some(ProbeRunFinish::PausedForReview {
        message: reason.to_string(),
    }))
}

fn write_checkpoint_artifact(
    journal: &OperationJournal,
    state: &mut ProbeRunState,
    reason: &str,
) -> DeviceResult<()> {
    state.checkpoint_count += 1;
    let dir = journal.checkpoints.join(format!(
        "{:03}_{}",
        state.checkpoint_count,
        safe_file_part(reason)
    ));
    fs::create_dir_all(&dir).map_err(|err| {
        DeviceError::fatal(format!(
            "failed to create checkpoint directory {}: {err}",
            dir.display()
        ))
    })?;
    let checkpoint = json!({
        "reason": reason,
        "frames": state.frames,
        "click_count": state.click_count,
        "executed": state.executed,
        "guard_failed": state.guard_failed,
        "initial_page": state.initial_page,
        "last_before_page": state.last_before_page,
        "last_after_page": state.last_after_page,
        "final_page": state.final_page,
    });
    fs::write(
        dir.join("checkpoint.json"),
        serde_json::to_vec_pretty(&checkpoint).map_err(|err| {
            DeviceError::fatal(format!("failed to serialize checkpoint.json: {err}"))
        })?,
    )
    .map_err(|err| DeviceError::fatal(format!("failed to write checkpoint.json: {err}")))?;
    Ok(())
}

fn ensure_touch_backend<'a>(
    backend: &'a mut Option<SelectedTouchBackend>,
    config: &MaaTouchValidationConfig,
    journal: &OperationJournal,
) -> DeviceResult<&'a mut SelectedTouchBackend> {
    if backend.is_none() {
        let created = create_touch_backend(
            TouchBackendConfig::new(
                config.adb.clone(),
                config.target.clone(),
                config.maatouch.clone(),
            )
            .with_minitouch_config(config.minitouch.clone())
            .with_requested(config.touch_backend),
        )?;
        let device = created.device_info().clone();
        journal.event(
            "touch_backend_selected",
            json!({
                "serial": device.serial,
                "state": device.state,
                "screen_size": device.screen_size,
                "backend": created.backend_name().as_str(),
                "diagnostics": touch_diagnostics_json(created.diagnostics())
            }),
        )?;
        *backend = Some(created);
    }
    backend
        .as_mut()
        .ok_or_else(|| DeviceError::fatal("failed to initialize touch backend"))
}

fn touch_diagnostics_json(diagnostics: &TouchBackendDiagnostics) -> serde_json::Value {
    json!({
        "requested": diagnostics.requested.as_str(),
        "selected": diagnostics.selected.map(TouchBackendName::as_str),
        "attempts": diagnostics.attempts.iter().map(|attempt| json!({
            "attempt_id": attempt.attempt_id,
            "backend": attempt.backend.as_str(),
            "ok": attempt.ok,
            "elapsed_ms": attempt.elapsed_ms,
            "action": attempt.action.as_deref(),
            "fallback_backend": attempt.fallback_backend.map(TouchBackendName::as_str),
            "error_reason": attempt.error_reason.as_deref(),
            "selected": attempt.selected
        })).collect::<Vec<_>>(),
        "warnings": diagnostics.warnings
    })
}

fn detect_current_page(
    detector: &PageDetector,
    evaluator: &RecognitionEvaluator,
    scene: &Scene,
) -> DeviceResult<Option<String>> {
    let outcomes = detector
        .evaluate_all(evaluator, scene)
        .map_err(page_error)?;
    let evaluations = require_all_page_evaluations(outcomes).map_err(page_error)?;
    Ok(evaluations
        .into_iter()
        .find(|evaluation| evaluation.matched)
        .map(|evaluation| evaluation.page_id))
}

struct ArrivalResult {
    scene: Scene,
    frames: usize,
}

struct ArrivalPollContext<'a> {
    journal: &'a OperationJournal,
    detector: &'a PageDetector,
    evaluator: &'a RecognitionEvaluator,
    navigation: Option<&'a NavigationBridge>,
    page_id: &'a str,
    step_id: &'a str,
}

struct ArrivalPollTiming {
    timeout_ms: u64,
    interval_ms: u64,
}

fn poll_arrival(
    capture: &mut ScreencapBackend,
    context: ArrivalPollContext<'_>,
    timing: ArrivalPollTiming,
) -> DeviceResult<ArrivalResult> {
    let timeout = Duration::from_millis(timing.timeout_ms);
    let interval = Duration::from_millis(timing.interval_ms.max(1));
    let started = Instant::now();
    let mut frames = 0;
    let max_frames =
        ((timing.timeout_ms / timing.interval_ms.max(1)) as usize).clamp(1, MAX_POLL_FRAMES);

    loop {
        let frame_name = format!(
            "{:03}_after_click_{}.png",
            frames + 1,
            safe_file_part(context.step_id)
        );
        let frame = capture_frame(capture, context.journal, &frame_name, "poll")?;
        frames += 1;
        let scene = scene_from_frame(&frame)?;
        let matched = match context
            .navigation
            .and_then(|bridge| bridge.arrival_anchors.get(context.page_id))
        {
            Some(anchor) => {
                match_arrival_anchor(&scene, anchor, context.journal, context.evaluator)?
            }
            None => {
                context
                    .detector
                    .evaluate_page(context.evaluator, &scene, context.page_id)
                    .map_err(page_error)?
                    .matched
            }
        };
        context.journal.event(
            "poll_iter",
            json!({
                "step_id": context.step_id,
                "page_id": context.page_id,
                "frame": frame_name,
                "matched": matched,
                "elapsed_ms": started.elapsed().as_millis()
            }),
        )?;
        if matched {
            let arrived_name = format!(
                "{}{:03}_arrived_{}.png",
                "",
                frames,
                safe_file_part(context.page_id)
            );
            let png = frame.png_for_artifact()?;
            fs::write(context.journal.frames.join(&arrived_name), &png).map_err(|err| {
                DeviceError::fatal(format!(
                    "failed to write arrived frame {}: {err}",
                    arrived_name
                ))
            })?;
            context.journal.event(
                "arrived",
                json!({"step_id": context.step_id, "page_id": context.page_id, "frame": arrived_name}),
            )?;
            return Ok(ArrivalResult { scene, frames });
        }
        if frames >= max_frames || started.elapsed() >= timeout {
            return Err(DeviceError::fatal(format!(
                "timed out waiting for expected page '{}' after {frames} frames",
                context.page_id
            )));
        }
        thread::sleep(interval);
    }
}

fn match_arrival_anchor(
    scene: &Scene,
    anchor: &ArrivalAnchor,
    journal: &OperationJournal,
    evaluator: &RecognitionEvaluator,
) -> DeviceResult<bool> {
    let template_path = journal.pack_root().join(&anchor.template);
    let template = fs::read(&template_path).map_err(|err| {
        DeviceError::fatal(format!(
            "failed to read arrival anchor template {}: {err}",
            template_path.display()
        ))
    })?;
    let matched = scene
        .match_template_with_metric(
            &template,
            anchor.region.map(pack_rect_to_recognition_rect),
            evaluator.default_match_metric(),
        )
        .map_err(|err| DeviceError::fatal(err.to_string()))?;
    Ok(matched.score >= anchor.threshold.unwrap_or(0.9))
}

fn capture_frame(
    capture: &mut ScreencapBackend,
    journal: &OperationJournal,
    name: &str,
    label: &str,
) -> DeviceResult<Frame> {
    journal.event("capture_started", json!({"label": label, "frame": name}))?;
    let started = Instant::now();
    let frame = capture.capture()?;
    let png = frame.png_for_artifact()?;
    fs::write(journal.frames.join(name), &png)
        .map_err(|err| DeviceError::fatal(format!("failed to write frame {}: {err}", name)))?;
    journal.event(
        "capture_done",
        json!({
            "label": label,
            "frame": name,
            "width": frame.width,
            "height": frame.height,
            "elapsed_ms": started.elapsed().as_millis()
        }),
    )?;
    Ok(frame)
}

fn scene_from_frame(frame: &Frame) -> DeviceResult<Scene> {
    let pixel_format = match frame.pixel_format {
        PixelFormat::Rgb8 => ScenePixelFormat::Rgb8,
        PixelFormat::Rgba8 => ScenePixelFormat::Rgba8,
    };
    Scene::from_pixels(frame.width, frame.height, &frame.pixels, pixel_format)
        .map_err(|err| DeviceError::fatal(err.to_string()))
}

fn load_navigation_bridge(path: &Path) -> DeviceResult<NavigationBridge> {
    let json = fs::read_to_string(path).map_err(|err| {
        DeviceError::fatal(format!(
            "failed to read navigation file {}: {err}",
            path.display()
        ))
    })?;
    let navigation: NavigationFile = serde_json::from_str(&json).map_err(|err| {
        DeviceError::fatal(format!(
            "failed to parse navigation file {}: {err}",
            path.display()
        ))
    })?;
    let click_space = ClickSpace {
        width: navigation.coordinate_space.width,
        height: navigation.coordinate_space.height,
    };
    if click_space.width <= 0 || click_space.height <= 0 {
        return Err(DeviceError::fatal(format!(
            "navigation coordinate_space must be positive, got {}x{}",
            click_space.width, click_space.height
        )));
    }

    let mut overrides = ProbeReferenceOverrides::new();
    let mut arrival_anchors = HashMap::new();
    let mut drag_targets = HashMap::new();
    let mut control_points = HashMap::new();
    let page_anchors = navigation
        .pages
        .iter()
        .filter_map(|page| page_arrival_anchor(page).map(|anchor| (page.id.clone(), anchor)))
        .collect::<HashMap<_, _>>();
    for (page_id, anchor) in &page_anchors {
        overrides.insert_page(page_id.clone());
        arrival_anchors.insert(page_id.clone(), anchor.clone());
    }
    for route in &navigation.navigation {
        if !route.from_page.is_empty() {
            overrides.insert_page(route.from_page.clone());
        }
        let target_id = navigation_target_id(&route.id);
        match route_input(route, click_space)? {
            NavigationInput::Tap(click_rect) => {
                overrides.insert_click_target(target_id, click_rect);
            }
            NavigationInput::Drag(drag) => {
                overrides.insert_click_target(target_id.clone(), drag.from);
                drag_targets.insert(target_id, drag);
            }
            NavigationInput::None => {
                continue;
            }
        }
        if !route.to_page.is_empty() {
            overrides.insert_page(route.to_page.clone());
            if let Some(anchor) = page_anchors.get(&route.to_page) {
                arrival_anchors.insert(route.to_page.clone(), anchor.clone());
                arrival_anchors.insert(navigation_arrival_page_id(&route.id), anchor.clone());
                overrides.insert_page(navigation_arrival_page_id(&route.id));
            }
        }
    }
    for point in &navigation.control_points {
        overrides.insert_click_target(
            control_target_id(&point.name),
            point_rect(point.point, click_space),
        );
        control_points.insert(point.name.clone(), point.point);
    }
    let forbidden = navigation
        .destructive_actions
        .iter()
        .filter_map(forbidden_from_destructive_action)
        .collect::<Vec<_>>();

    Ok(NavigationBridge {
        click_space,
        overrides,
        arrival_anchors,
        drag_targets,
        control_points,
        forbidden,
    })
}

fn page_arrival_anchor(page: &NavigationPage) -> Option<ArrivalAnchor> {
    page.anchors
        .iter()
        .find(|anchor| anchor.kind == "template" && !anchor.template_path.is_empty())
        .map(|anchor| ArrivalAnchor {
            template: anchor.template_path.clone(),
            threshold: anchor.threshold,
            region: parse_optional_region(&anchor.region),
        })
}

enum NavigationInput {
    Tap(PackRect),
    Drag(NavigationDrag),
    None,
}

fn route_input(route: &NavigationRoute, click_space: ClickSpace) -> DeviceResult<NavigationInput> {
    match route.click.kind.as_str() {
        "point" => {
            let point = parse_point_string(&route.click.point).map_err(|message| {
                DeviceError::fatal(format!(
                    "navigation route '{}' invalid point: {message}",
                    route.id
                ))
            })?;
            Ok(NavigationInput::Tap(point_rect(point, click_space)))
        }
        "rect" => {
            let rect = route_click_rect(route)?;
            validate_rect_in_click_space(rect, click_space)?;
            Ok(NavigationInput::Tap(rect))
        }
        "drag" => {
            let drag = route_drag(route)?;
            validate_rect_in_click_space(drag.from, click_space)?;
            validate_rect_in_click_space(drag.to, click_space)?;
            Ok(NavigationInput::Drag(drag))
        }
        "template_center" => {
            let _template_path = &route.click.template_path;
            Ok(NavigationInput::None)
        }
        other => Err(DeviceError::fatal(format!(
            "navigation route '{}' has unsupported click kind '{other}'",
            route.id
        ))),
    }
}

fn route_click_rect(route: &NavigationRoute) -> DeviceResult<PackRect> {
    if !route.click.rect.is_empty() {
        return parse_rect_string(&route.click.rect).map_err(|message| {
            DeviceError::fatal(format!(
                "navigation route '{}' invalid rect: {message}",
                route.id
            ))
        });
    }
    route_inline_rect(route, "click")
}

fn route_drag(route: &NavigationRoute) -> DeviceResult<NavigationDrag> {
    let from = route.click.from_rect.ok_or_else(|| {
        DeviceError::fatal(format!(
            "navigation route '{}' drag click missing from rect",
            route.id
        ))
    })?;
    let to = route.click.to_rect.ok_or_else(|| {
        DeviceError::fatal(format!(
            "navigation route '{}' drag click missing to rect",
            route.id
        ))
    })?;
    let duration_ms = route.click.duration_ms.ok_or_else(|| {
        DeviceError::fatal(format!(
            "navigation route '{}' drag click missing duration_ms",
            route.id
        ))
    })?;
    if duration_ms == 0 {
        return Err(DeviceError::fatal(format!(
            "navigation route '{}' drag duration_ms must be positive",
            route.id
        )));
    }
    Ok(NavigationDrag {
        from,
        to,
        duration_ms,
    })
}

fn route_inline_rect(route: &NavigationRoute, label: &str) -> DeviceResult<PackRect> {
    match (
        route.click.x,
        route.click.y,
        route.click.width,
        route.click.height,
    ) {
        (Some(x), Some(y), Some(width), Some(height)) => Ok(PackRect {
            x,
            y,
            width,
            height,
        }),
        _ => Err(DeviceError::fatal(format!(
            "navigation route '{}' {label} rect requires x, y, width, and height",
            route.id
        ))),
    }
}

fn forbidden_from_destructive_action(
    action: &NavigationDestructiveAction,
) -> Option<ForbiddenDestructivePoint> {
    let id = if action.page.is_empty() && action.kind.is_empty() {
        action.id.clone()
    } else {
        format!("{}:{}:{}", action.page, action.kind, action.id)
    };
    if !action.rect.is_empty() {
        return parse_rect_string(&action.rect)
            .ok()
            .map(|rect| ForbiddenDestructivePoint {
                id,
                point: None,
                rect: Some(rect),
                radius: Some(DEFAULT_FORBIDDEN_RADIUS),
            });
    }
    if action.point.is_empty() {
        return None;
    }
    match parse_i32_list(&action.point).ok()?.as_slice() {
        [x, y] => Some(ForbiddenDestructivePoint {
            id,
            point: Some([*x, *y]),
            rect: None,
            radius: Some(DEFAULT_FORBIDDEN_RADIUS),
        }),
        [x, y, width, height] => Some(ForbiddenDestructivePoint {
            id,
            point: None,
            rect: Some(PackRect {
                x: *x,
                y: *y,
                width: *width,
                height: *height,
            }),
            radius: Some(DEFAULT_FORBIDDEN_RADIUS),
        }),
        _ => None,
    }
}

fn parse_optional_region(value: &str) -> Option<PackRect> {
    if value.is_empty() || value == "full_frame" {
        None
    } else {
        parse_rect_string(value).ok()
    }
}

fn parse_point_string(value: &str) -> Result<[i32; 2], String> {
    match parse_i32_list(value)?.as_slice() {
        [x, y] => Ok([*x, *y]),
        values => Err(format!("expected x,y but got {} values", values.len())),
    }
}

fn parse_rect_string(value: &str) -> Result<PackRect, String> {
    match parse_i32_list(value)?.as_slice() {
        [x, y, width, height] => Ok(PackRect {
            x: *x,
            y: *y,
            width: *width,
            height: *height,
        }),
        values => Err(format!("expected x,y,w,h but got {} values", values.len())),
    }
}

fn parse_i32_list(value: &str) -> Result<Vec<i32>, String> {
    if value.trim().is_empty() {
        return Err("value is empty".to_string());
    }
    value
        .split(',')
        .map(|part| {
            part.trim()
                .parse::<i32>()
                .map_err(|err| format!("invalid integer '{}': {err}", part.trim()))
        })
        .collect()
}

impl NavigationBridge {
    fn drag_target(&self, target_id: &str) -> Option<NavigationDrag> {
        self.drag_targets.get(target_id).copied()
    }

    fn control_point(&self, name: &str) -> Option<[i32; 2]> {
        self.control_points.get(name).copied()
    }

    fn validate_rect_not_forbidden(&self, rect: PackRect) -> DeviceResult<()> {
        for forbidden in &self.forbidden {
            let intersects_rect = forbidden
                .rect
                .is_some_and(|forbidden_rect| rects_intersect(rect, forbidden_rect));
            let contains_point = forbidden
                .point
                .is_some_and(|point| rect_contains(rect, point[0], point[1]));
            let intersects_radius = forbidden.point.is_some_and(|point| {
                rect_intersects_radius(
                    rect,
                    point[0],
                    point[1],
                    forbidden.radius.unwrap_or(DEFAULT_FORBIDDEN_RADIUS),
                )
            });
            if intersects_rect || contains_point || intersects_radius {
                return Err(DeviceError::fatal(format!(
                    "click rect {} intersects forbidden destructive action '{}'",
                    format_rect_for_error(rect),
                    forbidden.id
                )));
            }
        }
        Ok(())
    }

    fn validate_point_not_forbidden(&self, x: i32, y: i32) -> DeviceResult<()> {
        for forbidden in &self.forbidden {
            let in_rect = forbidden.rect.is_some_and(|rect| rect_contains(rect, x, y));
            let in_radius = forbidden.point.is_some_and(|point| {
                point_in_radius(
                    x,
                    y,
                    point[0],
                    point[1],
                    forbidden.radius.unwrap_or(DEFAULT_FORBIDDEN_RADIUS),
                )
            });
            if in_rect || in_radius {
                return Err(DeviceError::fatal(format!(
                    "actual click point {x},{y} falls inside forbidden destructive action '{}'",
                    forbidden.id
                )));
            }
        }
        Ok(())
    }
}

impl OperationJournal {
    fn create(options: &ProbeRunOptions, probe_json: &str) -> DeviceResult<Self> {
        let run_id = format!("probe-{}", run_timestamp());
        let run_dir = options.run_root.join(&run_id);
        let frames = run_dir.join("frames");
        let observations = run_dir.join("observations");
        let checkpoints = run_dir.join("checkpoints");
        fs::create_dir_all(&frames).map_err(|err| {
            DeviceError::fatal(format!(
                "failed to create probe run directory {}: {err}",
                frames.display()
            ))
        })?;
        fs::create_dir_all(&observations).map_err(|err| {
            DeviceError::fatal(format!(
                "failed to create observations directory {}: {err}",
                observations.display()
            ))
        })?;
        fs::create_dir_all(&checkpoints).map_err(|err| {
            DeviceError::fatal(format!(
                "failed to create checkpoints directory {}: {err}",
                checkpoints.display()
            ))
        })?;
        fs::write(
            run_dir.join("command.txt"),
            env::args().collect::<Vec<_>>().join(" "),
        )
        .map_err(|err| DeviceError::fatal(format!("failed to write command.txt: {err}")))?;
        fs::write(run_dir.join("probe-plan.json"), probe_json)
            .map_err(|err| DeviceError::fatal(format!("failed to write probe-plan.json: {err}")))?;
        Ok(Self {
            run_id,
            events: run_dir.join("events.jsonl"),
            run_dir,
            frames,
            observations,
            checkpoints,
            pack_root: options.pack_root.clone(),
        })
    }

    fn event(&self, event: &str, payload: serde_json::Value) -> DeviceResult<()> {
        let value = json!({
            "timestamp_ms": now_millis(),
            "event": event,
            "payload": payload,
        });
        let line = serde_json::to_string(&value).map_err(|err| {
            DeviceError::fatal(format!("failed to serialize journal event: {err}"))
        })?;
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.events)
            .and_then(|mut file| {
                use std::io::Write;
                writeln!(file, "{line}")
            })
            .map_err(|err| {
                DeviceError::fatal(format!(
                    "failed to append journal event {}: {err}",
                    self.events.display()
                ))
            })
    }

    fn record_inputs(&self, options: &ProbeRunOptions) -> DeviceResult<()> {
        let inputs = json!({
            "pack": file_entry(&options.pack)?,
            "pages": file_entry(&options.pages)?,
            "probe": file_entry(&options.probe)?,
            "navigation": match &options.navigation {
                Some(path) => Some(file_entry(path)?),
                None => None,
            },
            "pack_root": options.pack_root,
        });
        fs::write(
            self.run_dir.join("input-paths.json"),
            serde_json::to_vec_pretty(&inputs).map_err(|err| {
                DeviceError::fatal(format!("failed to serialize input-paths.json: {err}"))
            })?,
        )
        .map_err(|err| DeviceError::fatal(format!("failed to write input-paths.json: {err}")))
    }

    fn summary(
        &self,
        options: &ProbeRunOptions,
        result: &str,
        state: &ProbeRunState,
        error: Option<String>,
    ) -> DeviceResult<()> {
        let summary = json!({
            "run_id": self.run_id,
            "result": result,
            "executed": state.executed,
            "click_count": state.click_count,
            "claims_executed": state.claims_executed,
            "regenerating_resource_actions_executed": state.regenerating_resource_actions_executed,
            "resource_kind": state.last_resource_kind,
            "max_cost": state.last_max_cost,
            "destructive_allowed": false,
            "premium_currency_allowed": false,
            "auto_refill_allowed": false,
            "guard_failed": state.guard_failed,
            "initial_page": state.initial_page,
            "last_before_page": state.last_before_page,
            "last_after_page": state.last_after_page,
            "final_page": state.final_page,
            "frames": state.frames,
            "observations": state.observations,
            "checkpoints": state.checkpoint_count,
            "paths": {
                "pack": options.pack,
                "pack_root": options.pack_root,
                "pages": options.pages,
                "probe": options.probe,
                "navigation": options.navigation,
                "run_dir": self.run_dir,
                "frames": self.frames,
                "observations": self.observations,
                "checkpoints": self.checkpoints
            },
            "error": error,
        });
        fs::write(
            self.run_dir.join("summary.json"),
            serde_json::to_vec_pretty(&summary).map_err(|err| {
                DeviceError::fatal(format!("failed to serialize summary.json: {err}"))
            })?,
        )
        .map_err(|err| DeviceError::fatal(format!("failed to write summary.json: {err}")))
    }

    fn pack_root(&self) -> &Path {
        &self.pack_root
    }
}

fn actual_click_point(rect: PackRect, seed: u64) -> ActualClickPoint {
    let mut state = if seed == 0 {
        0x9e37_79b9_7f4a_7c15
    } else {
        seed
    };
    let x_offset = next_u64(&mut state) % rect.width as u64;
    let y_offset = next_u64(&mut state) % rect.height as u64;
    ActualClickPoint {
        seed,
        algorithm: "xorshift64_uniform_rect_v1",
        rect,
        x: rect.x + x_offset as i32,
        y: rect.y + y_offset as i32,
    }
}

fn actual_input_action(
    click: PackRect,
    drag: Option<NavigationDrag>,
    seed: u64,
    click_space: ClickSpace,
) -> DeviceResult<ActualInputAction> {
    match drag {
        Some(drag) => {
            validate_rect_in_click_space(drag.from, click_space)?;
            validate_rect_in_click_space(drag.to, click_space)?;
            let from = actual_click_point(drag.from, seed ^ hash_text("drag.from"));
            let to = actual_click_point(drag.to, seed ^ hash_text("drag.to"));
            Ok(ActualInputAction::Drag {
                from,
                to,
                duration_ms: drag.duration_ms,
            })
        }
        None => {
            validate_rect_in_click_space(click, click_space)?;
            Ok(ActualInputAction::Tap(actual_click_point(click, seed)))
        }
    }
}

fn validate_input_action(
    action: &ActualInputAction,
    navigation: Option<&NavigationBridge>,
    click_space: ClickSpace,
) -> DeviceResult<()> {
    match action {
        ActualInputAction::Tap(actual) => {
            validate_point_in_click_space(actual.x, actual.y, click_space)?;
            if let Some(bridge) = navigation {
                bridge.validate_rect_not_forbidden(actual.rect)?;
                bridge.validate_point_not_forbidden(actual.x, actual.y)?;
            }
        }
        ActualInputAction::Drag { from, to, .. } => {
            validate_point_in_click_space(from.x, from.y, click_space)?;
            validate_point_in_click_space(to.x, to.y, click_space)?;
            if let Some(bridge) = navigation {
                bridge.validate_rect_not_forbidden(from.rect)?;
                bridge.validate_rect_not_forbidden(to.rect)?;
                bridge.validate_point_not_forbidden(from.x, from.y)?;
                bridge.validate_point_not_forbidden(to.x, to.y)?;
            }
        }
    }
    Ok(())
}

fn execute_input_action(
    backend: &mut dyn InputBackend,
    action: ActualInputAction,
) -> DeviceResult<()> {
    match action {
        ActualInputAction::Tap(actual) => backend.tap(actual.x, actual.y),
        ActualInputAction::Drag {
            from,
            to,
            duration_ms,
        } => backend.swipe(from.x, from.y, to.x, to.y, duration_ms),
    }
}

fn next_u64(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

fn actual_click_json(actual: ActualClickPoint) -> serde_json::Value {
    json!({
        "seed": actual.seed,
        "algorithm": actual.algorithm,
        "rect": rect_json(actual.rect),
        "point": {"x": actual.x, "y": actual.y}
    })
}

fn legacy_actual_click_json(action: ActualInputAction) -> serde_json::Value {
    match action {
        ActualInputAction::Tap(actual) => actual_click_json(actual),
        ActualInputAction::Drag { .. } => serde_json::Value::Null,
    }
}

fn actual_input_json(action: ActualInputAction) -> serde_json::Value {
    match action {
        ActualInputAction::Tap(actual) => {
            json!({
                "kind": "tap",
                "point": actual_click_json(actual)
            })
        }
        ActualInputAction::Drag {
            from,
            to,
            duration_ms,
        } => {
            json!({
                "kind": "drag",
                "from": actual_click_json(from),
                "to": actual_click_json(to),
                "duration_ms": duration_ms
            })
        }
    }
}

fn record_effect_execution(
    effect: ProbeClickEffect,
    policy: Option<&ResourcePolicy>,
    state: &mut ProbeRunState,
) {
    match effect {
        ProbeClickEffect::NavigationOnly => {}
        ProbeClickEffect::FreeClaim => {
            state.claims_executed += 1;
        }
        ProbeClickEffect::ConsumeRegeneratingResource => {
            state.regenerating_resource_actions_executed += 1;
        }
    }
    if let Some(policy) = policy {
        state.last_resource_kind = Some(format_resource_kind(policy.kind).to_string());
        state.last_max_cost = policy.max_cost;
    }
}

fn resource_policy_json(policy: Option<&ResourcePolicy>) -> serde_json::Value {
    match policy {
        Some(policy) => json!({
            "kind": format_resource_kind(policy.kind),
            "max_cost": policy.max_cost,
            "premium_currency_allowed": policy.premium_currency_allowed,
            "auto_refill_allowed": policy.auto_refill_allowed,
            "cost_allowed": policy.cost_allowed,
        }),
        None => serde_json::Value::Null,
    }
}

fn format_effect(effect: ProbeClickEffect) -> &'static str {
    match effect {
        ProbeClickEffect::NavigationOnly => "navigation_only",
        ProbeClickEffect::FreeClaim => "free_claim",
        ProbeClickEffect::ConsumeRegeneratingResource => "consume_regenerating_resource",
    }
}

fn format_resource_kind(kind: ResourcePolicyKind) -> &'static str {
    match kind {
        ResourcePolicyKind::FreeReward => "free_reward",
        ResourcePolicyKind::AzurlaneOil => "azurlane.oil",
        ResourcePolicyKind::BluearchiveAp => "bluearchive.ap",
        ResourcePolicyKind::ArknightsSanity => "arknights.sanity",
    }
}

fn rect_json(rect: PackRect) -> serde_json::Value {
    json!({"x": rect.x, "y": rect.y, "width": rect.width, "height": rect.height})
}

fn click_space_for_scene(
    scene: &Scene,
    navigation: Option<&NavigationBridge>,
) -> DeviceResult<ClickSpace> {
    if let Some(bridge) = navigation {
        return Ok(bridge.click_space);
    }
    let width = i32::try_from(scene.width())
        .map_err(|_| DeviceError::fatal(format!("scene width exceeds i32: {}", scene.width())))?;
    let height = i32::try_from(scene.height())
        .map_err(|_| DeviceError::fatal(format!("scene height exceeds i32: {}", scene.height())))?;
    Ok(ClickSpace { width, height })
}

fn point_rect(point: [i32; 2], click_space: ClickSpace) -> PackRect {
    let x = (point[0] - DEFAULT_CLICK_RECT_RADIUS).max(0);
    let y = (point[1] - DEFAULT_CLICK_RECT_RADIUS).max(0);
    let right = (point[0] + DEFAULT_CLICK_RECT_RADIUS).min(click_space.width - 1);
    let bottom = (point[1] + DEFAULT_CLICK_RECT_RADIUS).min(click_space.height - 1);
    PackRect {
        x,
        y,
        width: right - x + 1,
        height: bottom - y + 1,
    }
}

fn validate_rect_in_click_space(rect: PackRect, click_space: ClickSpace) -> DeviceResult<()> {
    if rect.width <= 0 || rect.height <= 0 {
        return Err(DeviceError::fatal(format!(
            "click rect dimensions must be positive: {}x{}",
            rect.width, rect.height
        )));
    }
    validate_point_in_click_space(rect.x, rect.y, click_space)?;
    validate_point_in_click_space(
        rect.x + rect.width - 1,
        rect.y + rect.height - 1,
        click_space,
    )
}

fn validate_point_in_click_space(x: i32, y: i32, click_space: ClickSpace) -> DeviceResult<()> {
    if !(0..click_space.width).contains(&x) || !(0..click_space.height).contains(&y) {
        return Err(DeviceError::fatal(format!(
            "click point {x},{y} is outside {}x{} click space",
            click_space.width, click_space.height
        )));
    }
    Ok(())
}

fn rect_contains(rect: PackRect, x: i32, y: i32) -> bool {
    x >= rect.x && y >= rect.y && x < rect.x + rect.width && y < rect.y + rect.height
}

fn rects_intersect(a: PackRect, b: PackRect) -> bool {
    a.x < b.x + b.width && a.x + a.width > b.x && a.y < b.y + b.height && a.y + a.height > b.y
}

fn rect_intersects_radius(rect: PackRect, cx: i32, cy: i32, radius: i32) -> bool {
    let closest_x = cx.clamp(rect.x, rect.x + rect.width - 1);
    let closest_y = cy.clamp(rect.y, rect.y + rect.height - 1);
    point_in_radius(closest_x, closest_y, cx, cy, radius)
}

fn point_in_radius(x: i32, y: i32, cx: i32, cy: i32, radius: i32) -> bool {
    let dx = i64::from(x - cx);
    let dy = i64::from(y - cy);
    let radius = i64::from(radius.max(0));
    dx * dx + dy * dy <= radius * radius
}

fn pack_rect_to_recognition_rect(rect: PackRect) -> RecognitionRect {
    RecognitionRect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}

fn format_rect_for_error(rect: PackRect) -> String {
    format!("{},{},{},{}", rect.x, rect.y, rect.width, rect.height)
}

fn navigation_target_id(id: &str) -> String {
    format!("navigation/{id}")
}

fn control_target_id(id: &str) -> String {
    format!("control/{id}")
}

fn navigation_arrival_page_id(id: &str) -> String {
    format!("navigation/{id}/arrive_anchor")
}

fn safe_file_part(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn file_entry(path: &Path) -> DeviceResult<serde_json::Value> {
    Ok(json!({
        "path": path,
        "fnv1a64": format!("{:016x}", hash_file(path)?),
    }))
}

fn hash_file(path: &Path) -> DeviceResult<u64> {
    let bytes = fs::read(path).map_err(|err| {
        DeviceError::fatal(format!(
            "failed to read {} for hashing: {err}",
            path.display()
        ))
    })?;
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    Ok(hash)
}

fn hash_text(value: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

fn run_seed() -> u64 {
    now_millis() ^ 0x517c_c1b7_2722_0a95
}

fn run_timestamp() -> String {
    now_millis().to_string()
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actual_click_point_records_seed_algorithm_rect_and_point() {
        let rect = PackRect {
            x: 10,
            y: 20,
            width: 30,
            height: 40,
        };
        let point = actual_click_point(rect, 12345);

        assert_eq!(point.seed, 12345);
        assert_eq!(point.algorithm, "xorshift64_uniform_rect_v1");
        assert_eq!(point.rect, rect);
        assert!(rect_contains(rect, point.x, point.y));
    }

    #[test]
    fn point_rect_wraps_point_with_radius() {
        let click_space = ClickSpace {
            width: 1280,
            height: 720,
        };
        assert_eq!(
            point_rect([66, 237], click_space),
            PackRect {
                x: 46,
                y: 217,
                width: 41,
                height: 41
            }
        );
    }

    #[test]
    fn forbidden_point_uses_radius_not_exact_equality() {
        let bridge = NavigationBridge {
            click_space: ClickSpace {
                width: 1280,
                height: 720,
            },
            overrides: ProbeReferenceOverrides::new(),
            arrival_anchors: HashMap::new(),
            drag_targets: HashMap::new(),
            control_points: HashMap::new(),
            forbidden: vec![ForbiddenDestructivePoint {
                id: "collect".to_string(),
                point: Some([100, 100]),
                rect: None,
                radius: Some(10),
            }],
        };

        assert!(bridge.validate_point_not_forbidden(106, 100).is_err());
        assert!(bridge.validate_point_not_forbidden(111, 100).is_ok());
    }

    #[test]
    fn forbidden_rect_rejects_inside_point() {
        let bridge = NavigationBridge {
            click_space: ClickSpace {
                width: 1280,
                height: 720,
            },
            overrides: ProbeReferenceOverrides::new(),
            arrival_anchors: HashMap::new(),
            drag_targets: HashMap::new(),
            control_points: HashMap::new(),
            forbidden: vec![ForbiddenDestructivePoint {
                id: "rect".to_string(),
                point: None,
                rect: Some(PackRect {
                    x: 40,
                    y: 50,
                    width: 10,
                    height: 20,
                }),
                radius: None,
            }],
        };

        assert!(bridge.validate_point_not_forbidden(49, 69).is_err());
        assert!(bridge.validate_point_not_forbidden(50, 69).is_ok());
    }

    #[test]
    fn forbidden_candidate_rect_rejects_point_and_radius_overlap() {
        let bridge = NavigationBridge {
            click_space: ClickSpace {
                width: 1280,
                height: 720,
            },
            overrides: ProbeReferenceOverrides::new(),
            arrival_anchors: HashMap::new(),
            drag_targets: HashMap::new(),
            control_points: HashMap::new(),
            forbidden: vec![ForbiddenDestructivePoint {
                id: "radius".to_string(),
                point: Some([100, 100]),
                rect: None,
                radius: Some(10),
            }],
        };

        assert!(
            bridge
                .validate_rect_not_forbidden(rect(90, 90, 5, 5))
                .is_err()
        );
        assert!(
            bridge
                .validate_rect_not_forbidden(rect(110, 96, 5, 5))
                .is_err()
        );
        assert!(
            bridge
                .validate_rect_not_forbidden(rect(120, 120, 5, 5))
                .is_ok()
        );
    }

    #[test]
    fn navigation_bridge_adds_navigation_targets_and_arrival_pages() {
        let dir =
            std::env::temp_dir().join(format!("actingcommand-probe-nav-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("navigation.json");
        fs::write(
            &path,
            r#"{
                "schema_version": "0.1",
                "coordinate_space": {"width": 1280, "height": 720},
                "control_points": [
                  {"name": "home", "point": [1236, 25], "note": "safe home"}
                ],
                "pages": [
                  {
                    "id": "bluearchive/task",
                    "anchors": [
                      {
                        "kind": "template",
                        "template_path": "page.png",
                        "region": "full_frame",
                        "threshold": 0.9,
                        "pack_target_id": "task_center"
                      }
                    ]
                  }
                ],
                "navigation": [
                  {
                    "id": "home_to_task",
                    "from_page": "bluearchive/home",
                    "to_page": "bluearchive/task",
                    "click": {"kind": "point", "point": "66,237"}
                  },
                  {
                    "id": "home_drag_to_task",
                    "from_page": "bluearchive/home",
                    "to_page": "bluearchive/task",
                    "click": {
                      "kind": "drag",
                      "from": {"x": 300, "y": 600, "width": 30, "height": 20},
                      "to": {"x": 900, "y": 600, "width": 30, "height": 20},
                      "duration_ms": 450
                    }
                  }
                ],
                "destructive_actions": [
                  {"id": "collect", "page": "bluearchive/task", "kind": "claim", "point": "1150,671"}
                ]
            }"#,
        )
        .expect("nav");

        let bridge = load_navigation_bridge(&path).expect("bridge");
        assert!(
            bridge
                .overrides
                .click_target("navigation/home_to_task")
                .is_some()
        );
        assert_eq!(
            bridge
                .overrides
                .click_target("navigation/home_drag_to_task")
                .expect("drag target"),
            rect(300, 600, 30, 20)
        );
        assert_eq!(
            bridge
                .drag_target("navigation/home_drag_to_task")
                .expect("drag"),
            NavigationDrag {
                from: rect(300, 600, 30, 20),
                to: rect(900, 600, 30, 20),
                duration_ms: 450
            }
        );
        assert!(bridge.overrides.click_target("control/home").is_some());
        assert!(
            bridge
                .overrides
                .contains_page("navigation/home_to_task/arrive_anchor")
        );
        assert!(bridge.overrides.contains_page("bluearchive/task"));
        let _ = fs::remove_dir_all(&dir);
    }

    fn rect(x: i32, y: i32, width: i32, height: i32) -> PackRect {
        PackRect {
            x,
            y,
            width,
            height,
        }
    }
}
