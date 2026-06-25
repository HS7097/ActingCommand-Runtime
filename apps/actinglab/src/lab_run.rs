// SPDX-License-Identifier: AGPL-3.0-only

use super::{
    CliError, CliOutcome, FlagArgs, GlobalOptions, device_config, effective_adb_path,
    effective_run_root, read_user_config,
};
use actingcommand_device::{
    CaptureBackend, CaptureBackendAttempt, CaptureBackendChoice, CaptureBackendConfig,
    CaptureBackendName, DeviceTarget, InputBackend, MaaTouchBackend, MaaTouchConfig,
    combine_operation_and_close, create_capture_backend,
};
use actingcommand_page_detector::{PageDetector, PageEvaluation, load_page_set_from_json_str};
use actingcommand_recognition::Scene;
use actingcommand_recognition_pack::{PackRect, RecognitionEvaluator, load_pack_from_json_str};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use zip::write::FileOptions;
use zip::{ZipArchive, ZipWriter};

const CONTROL_SCHEMA: &str = "Lab-1y.control.v1";
const SUMMARY_SCHEMA: &str = "Lab-1y.summary.v1";
const DEFAULT_CAPTURE_INTERVAL_MS: u64 = 300;
const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const DEFAULT_STEP_TIMEOUT_MS: u64 = 10_000;
const DEFAULT_MAX_STEPS: usize = 50;
const DEFAULT_TEMPLATE_THRESHOLD: f32 = 0.9;

pub(super) fn run_lab_run(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let zip_path = flags
        .optional_path("--zip")
        .or_else(|| flags.optional_path("--package"))
        .ok_or_else(|| CliError::usage("lab run requires --zip <input.zip>"))?;
    let out_path = flags.required_path("--out")?;
    let config = read_user_config()?;
    let run_root = flags
        .optional_path("--run-root")
        .or_else(|| effective_run_root(global, &config))
        .unwrap_or_else(|| PathBuf::from("target").join("actinglab-runs"));
    let capture_interval_override = parse_optional_u64(&flags, "--capture-interval-ms")?;
    let capture_backend_override = parse_optional_capture_backend(&flags, "--capture-backend")?;

    let mut ctx = LabRunContext::create(&run_root, &zip_path)?;
    ctx.set_phase("run_started");
    ctx.event(
        "run_started",
        json!({"input_zip": zip_path, "out": out_path}),
    )?;

    let result = execute_lab_run(
        &mut ctx,
        global,
        &config,
        &zip_path,
        capture_interval_override,
        capture_backend_override,
    );
    match result {
        Ok(run_state) => {
            let archive = ctx.finish(&out_path, true, None, Some(&run_state))?;
            Ok(json!({
                "ok": true,
                "run_id": ctx.run_id,
                "run_dir": ctx.run_dir.display().to_string(),
                "out": archive.path.display().to_string(),
                "output_zip_sha256": archive.sha256,
                "screenshot_count": ctx.screenshots.len(),
                "executed_step_count": ctx.steps.len()
            }))
        }
        Err(err) => {
            ctx.set_phase("run_failed");
            let message = err.message.clone();
            let archive = ctx.finish(&out_path, false, Some(&message), None);
            match archive {
                Ok(archive) => {
                    let mut err = err;
                    err.message = format!(
                        "{}; failure report written to {}",
                        err.message,
                        archive.path.display()
                    );
                    Err(err)
                }
                Err(write_err) => Err(CliError::package_invalid(format!(
                    "failed to write Lab-1y output package after error: {}; original error: {}",
                    write_err.message, err.message
                ))),
            }
        }
    }
}

fn execute_lab_run(
    ctx: &mut LabRunContext,
    global: &GlobalOptions,
    config: &super::UserConfig,
    zip_path: &Path,
    capture_interval_override: Option<u64>,
    capture_backend_override: Option<CaptureBackendChoice>,
) -> CliOutcome<RunState> {
    ctx.set_phase("input_unpacked");
    let input_sha256 = file_sha256(zip_path)?;
    ctx.input_zip_sha256 = Some(input_sha256);
    let unpacked = unpack_lab_input(zip_path, &ctx.input_dir)?;
    ctx.input_entries = unpacked.entries;
    ctx.event(
        "input_unpacked",
        json!({"entry_count": ctx.input_entries.len(), "input_dir": ctx.input_dir}),
    )?;

    ctx.set_phase("control_loaded");
    let control_path = ctx.input_dir.join("control.json");
    let control = read_json_file::<LabControl>(&control_path)?;
    control.validate()?;
    ctx.control = Some(control.clone());
    if control.producer.is_none() {
        ctx.event(
            "producer_missing",
            json!({"severity": "warning", "message": "control producer is missing; provenance is incomplete but not blocking"}),
        )?;
    }
    ctx.event(
        "control_loaded",
        json!({
            "package_id": control.package_id,
            "game": control.game,
            "server": control.server,
            "entry_task_id": control.entry_task_id,
            "producer_present": control.producer.is_some(),
            "trusted_execution_present": control.trusted_execution.is_some()
        }),
    )?;

    ctx.requested_capture_interval_ms = capture_interval_override.unwrap_or(
        control
            .capture_interval_ms
            .unwrap_or(DEFAULT_CAPTURE_INTERVAL_MS),
    );
    let timeout_ms = control.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);
    let step_timeout_ms = control.step_timeout_ms.unwrap_or(DEFAULT_STEP_TIMEOUT_MS);
    let max_steps = control.max_steps.unwrap_or(DEFAULT_MAX_STEPS);

    ctx.set_phase("resources_loaded");
    let resources = load_lab_resources(ctx, &control)?;
    ctx.event(
        "resources_loaded",
        json!({
            "manifest": resources.manifest_path,
            "operation": resources.operation_path,
            "pack": resources.pack_path,
            "pages": resources.pages_path,
            "navigation": resources.navigation_path,
            "operation_goal": resources.operation_bundle.goal,
            "entry_page": resources.operation_bundle.entry_page,
            "target_page": resources.operation_bundle.target_page,
            "operation_defaults": resources.operation_bundle.defaults.to_json()
        }),
    )?;

    let mut effective_global = global.clone();
    if effective_global.game.is_none() {
        effective_global.game = Some(control.game.clone());
    }
    if effective_global.server.is_none() {
        effective_global.server = Some(control.server.clone());
    }
    let device = device_config(&effective_global, config)?;
    ctx.instance = Some(device.target.resolved_serial());
    ctx.adb_path = Some(effective_adb_path(&effective_global, config));

    ctx.set_phase("lab_lease_acquired");
    let _lease_guard = LabLeaseGuard::acquire(&device.target.resolved_serial())?;
    ctx.event(
        "lab_lease_acquired",
        json!({"mode": "trusted_execution", "instance": ctx.instance}),
    )?;
    ctx.lease_acquired = true;

    let requested_capture_backend = capture_backend_override
        .or(control.capture_backend_choice()?)
        .unwrap_or_default();
    let selected_capture = create_capture_backend(
        CaptureBackendConfig::new(device.adb.clone(), device.target.clone())
            .with_requested(requested_capture_backend),
    )
    .map_err(|err| CliError::device(err.to_string()))?;
    ctx.capture_backend_requested = Some(requested_capture_backend);
    ctx.capture_backend_used = Some(selected_capture.diagnostics.used);
    ctx.capture_backend_attempts = selected_capture.diagnostics.attempts.clone();
    for attempt in ctx.capture_backend_attempts.clone() {
        ctx.event(
            "capture_backend_attempt",
            json!({
                "backend": attempt.backend.as_str(),
                "ok": attempt.ok,
                "severity": if attempt.ok { "info" } else { "warning" },
                "message": attempt.message
            }),
        )?;
    }
    let mut capture = selected_capture.backend;
    let mut input = None::<MaaTouchBackend>;
    let started = Instant::now();
    let mut state = RunState {
        control,
        resources,
        current_page: None,
        failed_step_id: None,
    };
    let actionable_page_candidates = if state.control.execution_mode == "recognize_only" {
        None
    } else {
        Some(actionable_page_ids(&state.resources, &state.control)?)
    };
    let initial_page_candidates = if state.control.execution_mode == "recognize_only" {
        None
    } else {
        Some(initial_page_ids(&state.resources, &state.control)?)
    };

    let first = capture_until_matched_page(
        ctx,
        capture.as_mut(),
        &state.resources,
        "initial",
        step_timeout_ms,
        &state.control,
        initial_page_candidates.as_deref(),
    )?;
    state.current_page = first.matched_anchor(&state.control.game);

    if state.control.execution_mode == "recognize_only" {
        ctx.event(
            "recognize_only_finished",
            json!({"matched_page": first.matched_page, "matched_anchor": state.current_page}),
        )?;
        ctx.event("lab_lease_released", json!({"mode": "trusted_execution"}))?;
        ctx.lease_released = true;
        return Ok(state);
    }

    for step_index in 0..max_steps {
        if started.elapsed() > Duration::from_millis(timeout_ms) {
            return Err(CliError::device(format!(
                "Lab-1y run timeout after {timeout_ms}ms"
            )));
        }
        let current_page = match state.current_page.clone() {
            Some(current_page) => current_page,
            None => {
                let scene = capture_until_matched_page(
                    ctx,
                    capture.as_mut(),
                    &state.resources,
                    "page_wait",
                    step_timeout_ms,
                    &state.control,
                    actionable_page_candidates.as_deref(),
                )?;
                let current_page = scene.matched_anchor(&state.control.game).ok_or_else(|| {
                    CliError::device("no page matched before operation selection")
                })?;
                state.current_page = Some(current_page.clone());
                current_page
            }
        };
        if state
            .resources
            .operation_bundle
            .target_page
            .as_ref()
            .is_some_and(|target| page_anchor_matches(&state.control.game, &current_page, target))
            && state.control.stop_on_confirmation.unwrap_or(true)
        {
            break;
        }

        let operation = state
            .resources
            .operation_bundle
            .operations
            .iter()
            .find(|operation| {
                page_anchor_matches(&state.control.game, &current_page, &operation.from)
            })
            .ok_or_else(|| {
                CliError::device(format!(
                    "no operation can continue from page '{current_page}'"
                ))
            })?
            .clone();

        ctx.event(
            "step_started",
            json!({"step_id": operation.id, "index": step_index, "operation_id": operation.id}),
        )?;
        ctx.event(
            "before_page_detected",
            json!({"step_id": operation.id, "page": current_page}),
        )?;

        let action = operation.input_action(&state.control.resolution, ctx.run_seed)?;
        let backend = ensure_maatouch(&mut input, &device.target, &device.adb)?;
        match &action {
            LabInputAction::Tap(point) => {
                let action_started = Instant::now();
                ctx.event(
                    "click_started",
                    json!({"step_id": operation.id, "actual_click_point": point.to_json()}),
                )?;
                if let Err(err) = backend.tap(point.x, point.y) {
                    return close_backend_after_error(
                        &mut input,
                        CliError::device(err.to_string()),
                    );
                }
                ctx.event(
                    "click_finished",
                    json!({"step_id": operation.id, "actual_click_point": point.to_json()}),
                )?;
                ctx.action_durations_ms
                    .push(action_started.elapsed().as_millis() as u64);
            }
            LabInputAction::Drag {
                from,
                to,
                duration_ms,
            } => {
                let action_started = Instant::now();
                ctx.event(
                    "drag_started",
                    json!({"step_id": operation.id, "from": from.to_json(), "to": to.to_json(), "duration_ms": duration_ms}),
                )?;
                if let Err(err) = backend.swipe(from.x, from.y, to.x, to.y, *duration_ms) {
                    return close_backend_after_error(
                        &mut input,
                        CliError::device(err.to_string()),
                    );
                }
                ctx.event(
                    "drag_finished",
                    json!({"step_id": operation.id, "from": from.to_json(), "to": to.to_json(), "duration_ms": duration_ms}),
                )?;
                ctx.action_durations_ms
                    .push(action_started.elapsed().as_millis() as u64);
            }
        }

        ctx.event(
            "page_guard_started",
            json!({"step_id": operation.id, "to": operation.to, "verify_template": operation.verify_template}),
        )?;
        let after = poll_after_operation(
            ctx,
            capture.as_mut(),
            &state.resources,
            &operation,
            step_timeout_ms,
            &state.control.game,
        )?;
        let verification = operation_verification_status(&state.control.game, &operation, &after);
        if verification == OperationVerification::Failed {
            ctx.event(
                "page_guard_failed",
                json!({"step_id": operation.id, "expected": operation.to, "after_page": after.matched_page}),
            )?;
            ctx.event(
                "step_failed",
                json!({"step_id": operation.id, "reason": "page_confirmation_failed"}),
            )?;
            state.failed_step_id = Some(operation.id.clone());
            return Err(CliError::device(format!(
                "page confirmation failed for operation '{}'",
                operation.id
            )));
        }
        let guard_event = match verification {
            OperationVerification::Verified => "page_guard_passed",
            OperationVerification::ExecutedUnverified => "page_guard_unverified",
            OperationVerification::Failed => unreachable!("failed verification returned earlier"),
        };
        ctx.event(
            guard_event,
            json!({"step_id": operation.id, "after_page": after.matched_page}),
        )?;
        ctx.event(
            "after_page_detected",
            json!({"step_id": operation.id, "page": after.matched_page, "anchor": after.matched_anchor(&state.control.game)}),
        )?;

        ctx.steps.push(json!({
            "id": operation.id,
            "operation_id": operation.id,
            "purpose": operation.purpose,
            "from": operation.from,
            "to": operation.to,
            "before_page": current_page,
            "after_page": after.matched_page,
            "after_anchor": after.matched_anchor(&state.control.game),
            "click_count": if matches!(action, LabInputAction::Tap(_)) { 1 } else { 0 },
            "drag_count": if matches!(action, LabInputAction::Drag { .. }) { 1 } else { 0 },
            "actual_input": action.to_json(),
            "consumes": operation.consumes,
            "produces": operation.produces,
            "verified_live": operation.verified_live,
            "provenance": operation.provenance,
            "result": verification.result_label()
        }));
        ctx.event(
            "step_finished",
            json!({"step_id": operation.id, "result": verification.result_label()}),
        )?;
        state.current_page = next_current_page(&state.control.game, &after, &operation);
        if state.current_page.is_none() {
            break;
        }
    }

    if let Some(mut backend) = input {
        combine_operation_and_close(Ok(()), backend.close())
            .map_err(|err| CliError::device(err.to_string()))?;
    }
    ctx.event("lab_lease_released", json!({"mode": "trusted_execution"}))?;
    ctx.lease_released = true;
    Ok(state)
}

fn capture_until_matched_page(
    ctx: &mut LabRunContext,
    capture: &mut dyn CaptureBackend,
    resources: &LabResources,
    label: &str,
    timeout_ms: u64,
    control: &LabControl,
    candidate_pages: Option<&[String]>,
) -> CliOutcome<CapturedScene> {
    let started = Instant::now();
    loop {
        ctx.wait_for_next_capture_start();
        let scene = ctx.capture_scene_with_pages(
            capture,
            &resources.evaluator,
            &resources.detector,
            label,
            candidate_pages,
        )?;
        validate_frame_resolution(control, scene.width, scene.height)?;
        if scene.matched_page.is_some() {
            return Ok(scene);
        }
        if started.elapsed() >= Duration::from_millis(timeout_ms) {
            return Ok(scene);
        }
    }
}

fn canonical_page_anchor(game: &str, page_id: &str) -> String {
    let prefix = format!("{game}/");
    page_id.strip_prefix(&prefix).unwrap_or(page_id).to_string()
}

fn page_anchor_matches(game: &str, observed_or_anchor: &str, expected_anchor: &str) -> bool {
    observed_or_anchor == expected_anchor
        || canonical_page_anchor(game, observed_or_anchor) == expected_anchor
        || observed_or_anchor == format!("{game}/{expected_anchor}")
}

fn matched_page_matches_anchor(
    game: &str,
    matched_page: Option<&str>,
    expected_anchor: &str,
) -> bool {
    matched_page.is_some_and(|page| page_anchor_matches(game, page, expected_anchor))
}

fn next_current_page(game: &str, after: &CapturedScene, operation: &Operation) -> Option<String> {
    after.matched_anchor(game).or_else(|| {
        operation
            .to
            .as_ref()
            .map(|to| canonical_page_anchor(game, to))
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperationVerification {
    Verified,
    ExecutedUnverified,
    Failed,
}

impl OperationVerification {
    fn result_label(self) -> &'static str {
        match self {
            OperationVerification::Verified => "ok",
            OperationVerification::ExecutedUnverified => "executed_unverified",
            OperationVerification::Failed => "failed",
        }
    }
}

fn operation_verification_status(
    game: &str,
    operation: &Operation,
    after: &CapturedScene,
) -> OperationVerification {
    let matched_to = operation
        .to
        .as_ref()
        .is_some_and(|to| matched_page_matches_anchor(game, after.matched_page.as_deref(), to));
    let matched_template = operation.verify_template.is_some() && after.verify_template_matched;
    if matched_to || matched_template {
        return OperationVerification::Verified;
    }
    if operation.to.is_none() && operation.verify_template.is_none() {
        return OperationVerification::ExecutedUnverified;
    }
    OperationVerification::Failed
}

fn actionable_page_ids(resources: &LabResources, control: &LabControl) -> CliOutcome<Vec<String>> {
    let mut pages = Vec::new();
    let mut seen = BTreeSet::new();
    if let Some(entry_page) = &resources.operation_bundle.entry_page
        && entry_page != "any"
    {
        push_resolved_page_id(&mut pages, &mut seen, resources, &control.game, entry_page)?;
    }
    if let Some(target_page) = &resources.operation_bundle.target_page {
        push_resolved_page_id(&mut pages, &mut seen, resources, &control.game, target_page)?;
    }
    for operation in &resources.operation_bundle.operations {
        push_resolved_page_id(
            &mut pages,
            &mut seen,
            resources,
            &control.game,
            &operation.from,
        )?;
        if let Some(to) = &operation.to {
            push_resolved_page_id(&mut pages, &mut seen, resources, &control.game, to)?;
        }
    }
    Ok(pages)
}

fn initial_page_ids(resources: &LabResources, control: &LabControl) -> CliOutcome<Vec<String>> {
    let mut pages = Vec::new();
    let mut seen = BTreeSet::new();
    if let Some(entry_page) = &resources.operation_bundle.entry_page
        && entry_page != "any"
    {
        push_resolved_page_id(&mut pages, &mut seen, resources, &control.game, entry_page)?;
    }
    if let Some(target_page) = &resources.operation_bundle.target_page {
        push_resolved_page_id(&mut pages, &mut seen, resources, &control.game, target_page)?;
    }
    if pages.is_empty() {
        return actionable_page_ids(resources, control);
    }
    Ok(pages)
}

fn operation_arrival_page_ids(
    resources: &LabResources,
    game: &str,
    operation: &Operation,
) -> CliOutcome<Option<Vec<String>>> {
    operation
        .to
        .as_ref()
        .map(|to| resolve_detector_page_id(resources, game, to).map(|page| vec![page]))
        .transpose()
}

fn resolve_detector_page_id(
    resources: &LabResources,
    game: &str,
    anchor: &str,
) -> CliOutcome<String> {
    let namespaced = format!("{game}/{anchor}");
    if resources.detector.contains_page(&namespaced) {
        return Ok(namespaced);
    }
    if resources.detector.contains_page(anchor) {
        return Ok(anchor.to_string());
    }
    Err(CliError::package_invalid(format!(
        "operation page anchor '{anchor}' does not resolve to a detector page id"
    )))
}

fn push_resolved_page_id(
    pages: &mut Vec<String>,
    seen: &mut BTreeSet<String>,
    resources: &LabResources,
    game: &str,
    anchor: &str,
) -> CliOutcome<()> {
    let page = resolve_detector_page_id(resources, game, anchor)?;
    if seen.insert(page.clone()) {
        pages.push(page);
    }
    Ok(())
}

fn close_backend_after_error<T>(
    backend: &mut Option<MaaTouchBackend>,
    err: CliError,
) -> CliOutcome<T> {
    if let Some(mut backend) = backend.take() {
        let close = backend.close();
        if let Err(close_err) = close {
            return Err(CliError::device(format!(
                "{}; MaaTouch close also failed: {}",
                err.message, close_err
            )));
        }
    }
    Err(err)
}

fn ensure_maatouch<'a>(
    backend: &'a mut Option<MaaTouchBackend>,
    target: &DeviceTarget,
    adb: &actingcommand_device::AdbConfig,
) -> CliOutcome<&'a mut MaaTouchBackend> {
    if backend.is_none() {
        let mut created =
            MaaTouchBackend::new(adb.clone(), target.clone(), MaaTouchConfig::default());
        created
            .connect()
            .map_err(|err| CliError::device(err.to_string()))?;
        *backend = Some(created);
    }
    backend
        .as_mut()
        .ok_or_else(|| CliError::device("failed to initialize MaaTouch backend"))
}

fn poll_after_operation(
    ctx: &mut LabRunContext,
    capture: &mut dyn CaptureBackend,
    resources: &LabResources,
    operation: &Operation,
    step_timeout_ms: u64,
    game: &str,
) -> CliOutcome<CapturedScene> {
    let started = Instant::now();
    let arrival_page_candidates = operation_arrival_page_ids(resources, game, operation)?;
    loop {
        ctx.wait_for_next_capture_start();
        let mut scene = ctx.capture_scene_with_pages(
            capture,
            &resources.evaluator,
            &resources.detector,
            &operation.id,
            arrival_page_candidates.as_deref(),
        )?;
        if let Some(template) = &operation.verify_template {
            scene.verify_template_matched = verify_template(
                &scene.scene,
                &resources.operation_dir,
                template,
                resources.operation_bundle.defaults.template_threshold,
            )?;
        }
        let matched_to = operation
            .to
            .as_ref()
            .is_some_and(|to| matched_page_matches_anchor(game, scene.matched_page.as_deref(), to));
        let unverified_single_frame = operation.to.is_none() && operation.verify_template.is_none();
        if matched_to || scene.verify_template_matched || unverified_single_frame {
            return Ok(scene);
        }
        if started.elapsed() >= Duration::from_millis(step_timeout_ms) {
            return Ok(scene);
        }
    }
}

fn verify_template(
    scene: &Scene,
    operation_dir: &Path,
    template: &str,
    threshold: f32,
) -> CliOutcome<bool> {
    let path = safe_join(operation_dir, template)?;
    let bytes = fs::read(&path).map_err(|err| {
        CliError::package_invalid(format!("failed to read {}: {err}", path.display()))
    })?;
    let matched = scene
        .match_template(&bytes, None)
        .map_err(|err| CliError::device(err.to_string()))?;
    Ok(matched.score >= threshold)
}

fn load_lab_resources(ctx: &LabRunContext, control: &LabControl) -> CliOutcome<LabResources> {
    let resource_root_name = control.resource_root.as_deref().unwrap_or("resources");
    if resource_root_name != "resources" {
        validate_relative_path(resource_root_name)?;
    }
    let resource_root = ctx.input_dir.join(resource_root_name);
    if !resource_root.is_dir() {
        return Err(CliError::package_invalid(format!(
            "missing resource root {}",
            resource_root.display()
        )));
    }
    let manifest_path = resource_root.join("manifest.json");
    let manifest = read_json_value(&manifest_path)?;
    validate_manifest_entry_task_id(&manifest_path, &manifest, control)?;
    let operation_dir = resource_root
        .join("operations")
        .join(&control.entry_task_id);
    let operation_path = operation_dir.join("task.json");
    let operation_bundle = read_json_file::<OperationBundle>(&operation_path)?;
    operation_bundle.validate(control, &operation_dir)?;

    let stem = format!("{}.{}", control.game, control.server);
    let recognition_dir = resource_root.join("recognition");
    let pack_path = recognition_dir.join(format!("{stem}.pack.json"));
    let pages_path = recognition_dir.join(format!("{stem}.pages.json"));
    let evaluator = load_evaluator(&pack_path, &resource_root)?;
    let detector = load_detector(&pages_path, &evaluator)?;
    let navigation_path = resource_root
        .join("navigation")
        .join(format!("{stem}.navigation.json"));
    let (navigation_path, navigation) = if navigation_path.exists() {
        let navigation = read_json_value(&navigation_path)?;
        (Some(navigation_path), Some(navigation))
    } else {
        (None, None)
    };

    Ok(LabResources {
        resource_root,
        manifest_path,
        manifest,
        operation_dir,
        operation_path,
        operation_bundle,
        pack_path,
        pages_path,
        evaluator,
        detector,
        navigation_path,
        navigation,
    })
}

fn load_evaluator(pack_path: &Path, pack_root: &Path) -> CliOutcome<RecognitionEvaluator> {
    let text = read_text_file(pack_path)?;
    let pack = load_pack_from_json_str(text.trim_start_matches('\u{feff}'))
        .map_err(|err| CliError::package_invalid(err.to_string()))?;
    RecognitionEvaluator::new(pack_root.to_path_buf(), pack)
        .map_err(|err| CliError::package_invalid(err.to_string()))
}

fn load_detector(path: &Path, evaluator: &RecognitionEvaluator) -> CliOutcome<PageDetector> {
    let text = read_text_file(path)?;
    let page_set = load_page_set_from_json_str(text.trim_start_matches('\u{feff}'))
        .map_err(|err| CliError::package_invalid(err.to_string()))?;
    let detector =
        PageDetector::new(page_set).map_err(|err| CliError::package_invalid(err.to_string()))?;
    detector
        .validate(evaluator)
        .map_err(|err| CliError::package_invalid(err.to_string()))?;
    Ok(detector)
}

fn validate_manifest_entry_task_id(
    manifest_path: &Path,
    manifest: &Value,
    control: &LabControl,
) -> CliOutcome<()> {
    let Some(value) = manifest.get("entry_task_id") else {
        return Ok(());
    };
    let Some(manifest_entry_task_id) = value.as_str() else {
        return Err(CliError::package_invalid(format!(
            "{} entry_task_id must be a string when present",
            manifest_path.display()
        )));
    };
    if manifest_entry_task_id != control.entry_task_id {
        return Err(CliError::package_invalid(format!(
            "{} entry_task_id '{}' conflicts with control entry_task_id '{}'",
            manifest_path.display(),
            manifest_entry_task_id,
            control.entry_task_id
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, Deserialize)]
struct LabControl {
    schema_version: String,
    package_id: String,
    execution_mode: String,
    game: String,
    server: String,
    resolution: Resolution,
    entry_task_id: String,
    #[serde(default)]
    capture_interval_ms: Option<u64>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    step_timeout_ms: Option<u64>,
    #[serde(default)]
    max_steps: Option<usize>,
    #[serde(default)]
    stop_on_error: Option<bool>,
    #[serde(default)]
    stop_on_confirmation: Option<bool>,
    #[serde(default)]
    resource_root: Option<String>,
    #[serde(default)]
    allow_placeholder_coords: Option<bool>,
    #[serde(default)]
    output: Option<Value>,
    #[serde(default)]
    capture_backend: Option<String>,
    #[serde(default)]
    producer: Option<Value>,
    #[serde(default)]
    trusted_execution: Option<Value>,
}

impl LabControl {
    fn validate(&self) -> CliOutcome<()> {
        if self.schema_version != CONTROL_SCHEMA {
            return Err(CliError::package_invalid(format!(
                "unsupported control schema_version '{}', expected {CONTROL_SCHEMA}",
                self.schema_version
            )));
        }
        if !matches!(
            self.execution_mode.as_str(),
            "navigable_route" | "recognize_only" | "in_page_guard"
        ) {
            return Err(CliError::package_invalid(format!(
                "unsupported execution_mode '{}', expected navigable_route, recognize_only, or in_page_guard",
                self.execution_mode
            )));
        }
        for (name, value) in [
            ("package_id", &self.package_id),
            ("game", &self.game),
            ("server", &self.server),
            ("entry_task_id", &self.entry_task_id),
        ] {
            if value.trim().is_empty() {
                return Err(CliError::package_invalid(format!(
                    "control {name} is empty"
                )));
            }
        }
        if self.resolution.width == 0 || self.resolution.height == 0 {
            return Err(CliError::package_invalid(
                "control resolution width and height must be non-zero",
            ));
        }
        if self.capture_interval_ms == Some(0) {
            return Err(CliError::package_invalid(
                "capture_interval_ms must be positive when provided",
            ));
        }
        if let Some(capture_backend) = &self.capture_backend {
            CaptureBackendChoice::parse(capture_backend)
                .map_err(|err| CliError::package_invalid(err.to_string()))?;
        }
        Ok(())
    }

    fn capture_backend_choice(&self) -> CliOutcome<Option<CaptureBackendChoice>> {
        self.capture_backend
            .as_deref()
            .map(CaptureBackendChoice::parse)
            .transpose()
            .map_err(|err| CliError::package_invalid(err.to_string()))
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct Resolution {
    width: u32,
    height: u32,
}

#[derive(Debug)]
struct LabResources {
    resource_root: PathBuf,
    manifest_path: PathBuf,
    manifest: Value,
    operation_dir: PathBuf,
    operation_path: PathBuf,
    operation_bundle: OperationBundle,
    pack_path: PathBuf,
    pages_path: PathBuf,
    evaluator: RecognitionEvaluator,
    detector: PageDetector,
    navigation_path: Option<PathBuf>,
    navigation: Option<Value>,
}

#[derive(Debug)]
struct RunState {
    control: LabControl,
    resources: LabResources,
    current_page: Option<String>,
    failed_step_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct OperationBundle {
    schema_version: String,
    task_id: String,
    game: String,
    #[serde(default)]
    server_scope: Vec<String>,
    #[serde(default)]
    goal: String,
    coordinate_space: Resolution,
    #[serde(default)]
    defaults: OperationDefaults,
    #[serde(default)]
    anchors: Vec<OperationAnchor>,
    #[serde(default)]
    entry_page: Option<String>,
    #[serde(default)]
    target_page: Option<String>,
    operations: Vec<Operation>,
}

impl OperationBundle {
    fn validate(&self, control: &LabControl, operation_dir: &Path) -> CliOutcome<()> {
        if self.schema_version != "0.3" {
            return Err(CliError::package_invalid(format!(
                "unsupported operation schema_version '{}', expected 0.3",
                self.schema_version
            )));
        }
        if self.task_id != control.entry_task_id {
            return Err(CliError::package_invalid(format!(
                "operation task_id '{}' does not match control entry_task_id '{}'",
                self.task_id, control.entry_task_id
            )));
        }
        if self.game != control.game {
            return Err(CliError::package_invalid(format!(
                "operation game '{}' does not match control game '{}'",
                self.game, control.game
            )));
        }
        if !self.server_scope.is_empty()
            && !self
                .server_scope
                .iter()
                .any(|server| server == &control.server)
        {
            return Err(CliError::package_invalid(format!(
                "operation server_scope does not include '{}'",
                control.server
            )));
        }
        if self.coordinate_space.width != control.resolution.width
            || self.coordinate_space.height != control.resolution.height
        {
            return Err(CliError::package_invalid(format!(
                "operation coordinate_space {}x{} does not match control resolution {}x{}",
                self.coordinate_space.width,
                self.coordinate_space.height,
                control.resolution.width,
                control.resolution.height
            )));
        }
        if self.operations.is_empty() {
            return Err(CliError::package_invalid(
                "operation bundle has no operations",
            ));
        }
        for anchor in &self.anchors {
            if anchor.id.trim().is_empty() {
                return Err(CliError::package_invalid(
                    "operation anchor id must not be empty",
                ));
            }
            let path = safe_join(operation_dir, &anchor.template)?;
            if !path.is_file() {
                return Err(CliError::package_invalid(format!(
                    "operation anchor '{}' references missing template {}",
                    anchor.id,
                    path.display()
                )));
            }
        }
        let mut ids = BTreeSet::new();
        for operation in &self.operations {
            operation.validate(control)?;
            if !ids.insert(operation.id.clone()) {
                return Err(CliError::package_invalid(format!(
                    "duplicate operation id '{}'",
                    operation.id
                )));
            }
            if let Some(template) = &operation.verify_template {
                let path = safe_join(operation_dir, template)?;
                if !path.is_file() {
                    return Err(CliError::package_invalid(format!(
                        "operation '{}' references missing verify_template {}",
                        operation.id,
                        path.display()
                    )));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct OperationDefaults {
    #[serde(default = "default_template_threshold")]
    template_threshold: f32,
    #[serde(default)]
    color_max_distance: Option<f32>,
}

impl Default for OperationDefaults {
    fn default() -> Self {
        Self {
            template_threshold: DEFAULT_TEMPLATE_THRESHOLD,
            color_max_distance: None,
        }
    }
}

impl OperationDefaults {
    fn to_json(self) -> Value {
        json!({
            "template_threshold": self.template_threshold,
            "color_max_distance": self.color_max_distance
        })
    }
}

fn default_template_threshold() -> f32 {
    DEFAULT_TEMPLATE_THRESHOLD
}

#[derive(Debug, Clone, Deserialize)]
struct OperationAnchor {
    id: String,
    template: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Operation {
    id: String,
    purpose: String,
    from: String,
    #[serde(default)]
    to: Option<String>,
    click: OperationClick,
    #[serde(default)]
    verify_template: Option<String>,
    #[serde(default)]
    consumes: Vec<String>,
    #[serde(default)]
    produces: Vec<String>,
    #[serde(default)]
    verified_live: Option<bool>,
    #[serde(default)]
    provenance: Option<Value>,
}

impl Operation {
    fn validate(&self, control: &LabControl) -> CliOutcome<()> {
        for (name, value) in [("id", &self.id), ("from", &self.from)] {
            if value.trim().is_empty() {
                return Err(CliError::package_invalid(format!(
                    "operation {name} must not be empty"
                )));
            }
        }
        self.click.validate(control)
    }

    fn input_action(&self, resolution: &Resolution, seed_base: u64) -> CliOutcome<LabInputAction> {
        self.click
            .input_action(resolution, seed_base ^ hash_text(&self.id))
    }
}

#[derive(Debug, Clone, Deserialize)]
struct OperationClick {
    kind: String,
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

impl OperationClick {
    fn validate(&self, control: &LabControl) -> CliOutcome<()> {
        match self.kind.as_str() {
            "rect" => {
                let rect = self.required_rect()?;
                validate_click_rect(
                    rect,
                    &control.resolution,
                    control.allow_placeholder_coords.unwrap_or(false),
                )
            }
            "point" => {
                let x = self
                    .x
                    .ok_or_else(|| CliError::package_invalid("point click missing x"))?;
                let y = self
                    .y
                    .ok_or_else(|| CliError::package_invalid("point click missing y"))?;
                validate_click_point(
                    x,
                    y,
                    &control.resolution,
                    control.allow_placeholder_coords.unwrap_or(false),
                )
            }
            "drag" => {
                let from = self
                    .from_rect
                    .ok_or_else(|| CliError::package_invalid("drag click missing from rect"))?;
                let to = self
                    .to_rect
                    .ok_or_else(|| CliError::package_invalid("drag click missing to rect"))?;
                validate_click_rect(
                    from,
                    &control.resolution,
                    control.allow_placeholder_coords.unwrap_or(false),
                )?;
                validate_click_rect(
                    to,
                    &control.resolution,
                    control.allow_placeholder_coords.unwrap_or(false),
                )?;
                if self.duration_ms.unwrap_or(0) == 0 {
                    return Err(CliError::package_invalid(
                        "drag duration_ms must be positive",
                    ));
                }
                Ok(())
            }
            other => Err(CliError::package_invalid(format!(
                "unknown operation click kind '{other}'"
            ))),
        }
    }

    fn input_action(&self, resolution: &Resolution, seed: u64) -> CliOutcome<LabInputAction> {
        match self.kind.as_str() {
            "rect" => Ok(LabInputAction::Tap(actual_click_point(
                self.required_rect()?,
                seed,
            ))),
            "point" => {
                let x = self
                    .x
                    .ok_or_else(|| CliError::package_invalid("point click missing x"))?;
                let y = self
                    .y
                    .ok_or_else(|| CliError::package_invalid("point click missing y"))?;
                validate_click_point(x, y, resolution, false)?;
                Ok(LabInputAction::Tap(ActualClickPoint {
                    seed,
                    algorithm: "explicit_point_v1",
                    rect: PackRect {
                        x,
                        y,
                        width: 1,
                        height: 1,
                    },
                    x,
                    y,
                }))
            }
            "drag" => {
                let from = self
                    .from_rect
                    .ok_or_else(|| CliError::package_invalid("drag click missing from rect"))?;
                let to = self
                    .to_rect
                    .ok_or_else(|| CliError::package_invalid("drag click missing to rect"))?;
                Ok(LabInputAction::Drag {
                    from: actual_click_point(from, seed ^ hash_text("drag.from")),
                    to: actual_click_point(to, seed ^ hash_text("drag.to")),
                    duration_ms: self.duration_ms.unwrap_or(300),
                })
            }
            other => Err(CliError::package_invalid(format!(
                "unknown operation click kind '{other}'"
            ))),
        }
    }

    fn required_rect(&self) -> CliOutcome<PackRect> {
        Ok(PackRect {
            x: self
                .x
                .ok_or_else(|| CliError::package_invalid("rect click missing x"))?,
            y: self
                .y
                .ok_or_else(|| CliError::package_invalid("rect click missing y"))?,
            width: self
                .width
                .ok_or_else(|| CliError::package_invalid("rect click missing width"))?,
            height: self
                .height
                .ok_or_else(|| CliError::package_invalid("rect click missing height"))?,
        })
    }
}

#[derive(Debug, Clone, Copy)]
enum LabInputAction {
    Tap(ActualClickPoint),
    Drag {
        from: ActualClickPoint,
        to: ActualClickPoint,
        duration_ms: u64,
    },
}

impl LabInputAction {
    fn to_json(self) -> Value {
        match self {
            LabInputAction::Tap(point) => {
                json!({"kind": "tap", "actual_click_point": point.to_json()})
            }
            LabInputAction::Drag {
                from,
                to,
                duration_ms,
            } => {
                json!({"kind": "drag", "from": from.to_json(), "to": to.to_json(), "duration_ms": duration_ms})
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ActualClickPoint {
    seed: u64,
    algorithm: &'static str,
    rect: PackRect,
    x: i32,
    y: i32,
}

impl ActualClickPoint {
    fn to_json(self) -> Value {
        json!({
            "seed": self.seed,
            "algorithm": self.algorithm,
            "rect": rect_json(self.rect),
            "point": {"x": self.x, "y": self.y}
        })
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

fn next_u64(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

fn validate_click_rect(
    rect: PackRect,
    resolution: &Resolution,
    allow_placeholder: bool,
) -> CliOutcome<()> {
    if rect.width <= 0 || rect.height <= 0 {
        return Err(CliError::package_invalid(format!(
            "click rect dimensions must be positive: {}x{}",
            rect.width, rect.height
        )));
    }
    validate_click_point(rect.x, rect.y, resolution, allow_placeholder)?;
    validate_click_point(
        rect.x + rect.width - 1,
        rect.y + rect.height - 1,
        resolution,
        allow_placeholder,
    )?;
    if !allow_placeholder
        && rect.x == 0
        && rect.y == 0
        && rect.width as u32 == resolution.width
        && rect.height as u32 == resolution.height
    {
        return Err(CliError::package_invalid(
            "full-screen click rect is treated as unresolved coordinates",
        ));
    }
    Ok(())
}

fn validate_click_point(
    x: i32,
    y: i32,
    resolution: &Resolution,
    allow_placeholder: bool,
) -> CliOutcome<()> {
    if x < 0 || y < 0 || x >= resolution.width as i32 || y >= resolution.height as i32 {
        return Err(CliError::package_invalid(format!(
            "click point {x},{y} is outside {}x{}",
            resolution.width, resolution.height
        )));
    }
    if !allow_placeholder && x == 0 && y == 0 {
        return Err(CliError::package_invalid(
            "click point 0,0 is treated as unresolved coordinates",
        ));
    }
    Ok(())
}

fn validate_frame_resolution(control: &LabControl, width: u32, height: u32) -> CliOutcome<()> {
    if width != control.resolution.width || height != control.resolution.height {
        return Err(CliError::device(format!(
            "device frame resolution {width}x{height} does not match package resolution {}x{}",
            control.resolution.width, control.resolution.height
        )));
    }
    Ok(())
}

struct LabRunContext {
    run_id: String,
    run_seed: u64,
    started_at: SystemTime,
    started_instant: Instant,
    run_dir: PathBuf,
    input_dir: PathBuf,
    output_dir: PathBuf,
    logs_dir: PathBuf,
    screenshots_dir: PathBuf,
    input_zip_sha256: Option<String>,
    input_entries: Vec<String>,
    requested_capture_interval_ms: u64,
    screenshot_names: HashMap<String, usize>,
    screenshots: Vec<ScreenshotRecord>,
    recognition: Vec<Value>,
    events: Vec<Value>,
    steps: Vec<Value>,
    intervals_ms: Vec<u64>,
    capture_durations_ms: Vec<u64>,
    action_durations_ms: Vec<u64>,
    loop_lag_ms: Vec<u64>,
    last_capture_at: Option<Instant>,
    frame_index: usize,
    phase: String,
    control: Option<LabControl>,
    instance: Option<String>,
    adb_path: Option<String>,
    capture_backend_requested: Option<CaptureBackendChoice>,
    capture_backend_used: Option<CaptureBackendName>,
    capture_backend_attempts: Vec<CaptureBackendAttempt>,
    lease_acquired: bool,
    lease_released: bool,
}

impl LabRunContext {
    fn create(run_root: &Path, input_zip: &Path) -> CliOutcome<Self> {
        let now = SystemTime::now();
        let run_id = format!("lab1y-{}", timestamp_file_stem(now));
        let run_dir = run_root.join(&run_id);
        let input_dir = run_dir.join("input");
        let output_dir = run_dir.join("output");
        let logs_dir = output_dir.join("logs");
        let screenshots_dir = output_dir.join("screenshots");
        fs::create_dir_all(&input_dir).map_err(|err| {
            CliError::package_invalid(format!("failed to create {}: {err}", input_dir.display()))
        })?;
        fs::create_dir_all(&logs_dir).map_err(|err| {
            CliError::package_invalid(format!("failed to create {}: {err}", logs_dir.display()))
        })?;
        fs::create_dir_all(&screenshots_dir).map_err(|err| {
            CliError::package_invalid(format!(
                "failed to create {}: {err}",
                screenshots_dir.display()
            ))
        })?;
        Ok(Self {
            run_id,
            run_seed: hash_text(&input_zip.display().to_string()),
            started_at: now,
            started_instant: Instant::now(),
            run_dir,
            input_dir,
            output_dir,
            logs_dir,
            screenshots_dir,
            input_zip_sha256: None,
            input_entries: Vec::new(),
            requested_capture_interval_ms: DEFAULT_CAPTURE_INTERVAL_MS,
            screenshot_names: HashMap::new(),
            screenshots: Vec::new(),
            recognition: Vec::new(),
            events: Vec::new(),
            steps: Vec::new(),
            intervals_ms: Vec::new(),
            capture_durations_ms: Vec::new(),
            action_durations_ms: Vec::new(),
            loop_lag_ms: Vec::new(),
            last_capture_at: None,
            frame_index: 0,
            phase: "created".to_string(),
            control: None,
            instance: None,
            adb_path: None,
            capture_backend_requested: None,
            capture_backend_used: None,
            capture_backend_attempts: Vec::new(),
            lease_acquired: false,
            lease_released: false,
        })
    }

    fn set_phase(&mut self, phase: &str) {
        self.phase = phase.to_string();
    }

    fn event(&mut self, event: &str, data: Value) -> CliOutcome<()> {
        if event == "lab_lease_acquired" {
            self.lease_acquired = true;
        } else if event == "lab_lease_released" {
            self.lease_released = true;
        }
        let mut object = serde_json::Map::new();
        object.insert("event".to_string(), json!(event));
        object.insert(
            "timestamp".to_string(),
            json!(timestamp_iso(SystemTime::now())),
        );
        object.insert("phase".to_string(), json!(self.phase));
        object.insert("data".to_string(), data);
        self.events.push(Value::Object(object));
        Ok(())
    }

    fn wait_for_next_capture_start(&mut self) {
        let Some(last) = self.last_capture_at else {
            return;
        };
        let interval = Duration::from_millis(self.requested_capture_interval_ms.max(1));
        let target = last + interval;
        let now = Instant::now();
        if now < target {
            std::thread::sleep(target.duration_since(now));
        } else {
            self.loop_lag_ms
                .push(now.duration_since(target).as_millis() as u64);
        }
    }

    fn capture_scene_with_pages(
        &mut self,
        capture: &mut dyn CaptureBackend,
        evaluator: &RecognitionEvaluator,
        detector: &PageDetector,
        label: &str,
        candidate_pages: Option<&[String]>,
    ) -> CliOutcome<CapturedScene> {
        let now = Instant::now();
        if let Some(last) = self.last_capture_at.replace(now) {
            self.intervals_ms
                .push(now.duration_since(last).as_millis() as u64);
        }
        let frame = capture
            .capture()
            .map_err(|err| CliError::device(err.to_string()))?;
        self.capture_durations_ms
            .push(now.elapsed().as_millis() as u64);
        self.frame_index += 1;
        let file_name = self.next_screenshot_name(SystemTime::now());
        let relative = format!("screenshots/{file_name}");
        let path = self.screenshots_dir.join(&file_name);
        fs::write(&path, &frame.png).map_err(|err| {
            CliError::device(format!("failed to write {}: {err}", path.display()))
        })?;
        self.screenshots.push(ScreenshotRecord {
            frame_index: self.frame_index,
            file: relative.clone(),
            width: frame.width,
            height: frame.height,
        });
        self.event(
            "screenshot_saved",
            json!({
                "frame_index": self.frame_index,
                "file": relative,
                "width": frame.width,
                "height": frame.height,
                "backend": frame.backend_name.as_str(),
                "pixel_format": frame.pixel_format.as_str(),
                "captured_at": timestamp_iso(frame.captured_at),
                "label": label
            }),
        )?;

        let scene = Scene::from_png(&frame.png).map_err(|err| CliError::device(err.to_string()))?;
        let evaluations = match candidate_pages {
            Some(pages) => pages
                .iter()
                .map(|page| detector.evaluate_page(evaluator, &scene, page))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|err| CliError::device(err.to_string()))?,
            None => detector
                .evaluate_all(evaluator, &scene)
                .map_err(|err| CliError::device(err.to_string()))?,
        };
        let matched_page = evaluations
            .iter()
            .find(|evaluation| evaluation.matched)
            .map(|evaluation| evaluation.page_id.clone());
        let recognition = json!({
            "timestamp": timestamp_iso(SystemTime::now()),
            "frame_index": self.frame_index,
            "file": self.screenshots.last().map(|record| record.file.clone()),
            "matched_page": matched_page,
            "candidates": evaluations.iter().map(page_evaluation_json).collect::<Vec<_>>(),
            "diagnostics": {"label": label}
        });
        self.recognition.push(recognition);
        self.event(
            "recognition_recorded",
            json!({"frame_index": self.frame_index, "matched_page": matched_page}),
        )?;
        Ok(CapturedScene {
            scene,
            matched_page,
            verify_template_matched: false,
            width: frame.width,
            height: frame.height,
        })
    }

    fn next_screenshot_name(&mut self, now: SystemTime) -> String {
        let stem = timestamp_file_stem(now);
        let count = self.screenshot_names.entry(stem.clone()).or_insert(0);
        *count += 1;
        if *count == 1 {
            format!("{stem}.png")
        } else {
            format!("{stem}_{:02}.png", *count)
        }
    }

    fn finish(
        &mut self,
        out_path: &Path,
        ok: bool,
        failure_reason: Option<&str>,
        state: Option<&RunState>,
    ) -> CliOutcome<ArchiveResult> {
        if self.lease_acquired && !self.lease_released {
            self.event(
                "lab_lease_released",
                json!({"mode": "trusted_one_shot", "reason": "finish_cleanup"}),
            )?;
        }
        let final_event = if ok { "run_finished" } else { "run_failed" };
        self.event(
            final_event,
            json!({"ok": ok, "failure_reason": failure_reason}),
        )?;
        self.event("output_zip_written", json!({"out": out_path}))?;
        self.write_logs(ok, failure_reason, state)?;
        write_output_zip(&self.output_dir, out_path)?;
        let sha256 = file_sha256(out_path)?;
        Ok(ArchiveResult {
            path: out_path.to_path_buf(),
            sha256,
        })
    }

    fn write_logs(
        &self,
        ok: bool,
        failure_reason: Option<&str>,
        state: Option<&RunState>,
    ) -> CliOutcome<()> {
        write_json_lines(&self.logs_dir.join("events.jsonl"), &self.events)?;
        write_json_lines(&self.logs_dir.join("recognition.jsonl"), &self.recognition)?;
        write_json(
            &self.logs_dir.join("summary.json"),
            &self.summary_json(ok, failure_reason, state),
        )?;
        write_json(
            &self.logs_dir.join("diagnostics.json"),
            &self.diagnostics_json(failure_reason, state),
        )?;
        write_json(
            &self.logs_dir.join("environment.json"),
            &self.environment_json(state),
        )?;
        fs::write(
            self.logs_dir.join("result.md"),
            self.result_markdown(ok, failure_reason, state),
        )
        .map_err(|err| CliError::package_invalid(format!("failed to write result.md: {err}")))?;
        Ok(())
    }

    fn summary_json(
        &self,
        ok: bool,
        failure_reason: Option<&str>,
        state: Option<&RunState>,
    ) -> Value {
        let finished = SystemTime::now();
        let stats = interval_stats(&self.intervals_ms);
        let capture_stats = interval_stats(&self.capture_durations_ms);
        let action_stats = interval_stats(&self.action_durations_ms);
        let lag_stats = interval_stats(&self.loop_lag_ms);
        let control = self
            .control
            .as_ref()
            .or_else(|| state.map(|state| &state.control));
        json!({
            "schema_version": SUMMARY_SCHEMA,
            "ok": ok,
            "run_id": self.run_id,
            "package_id": control.map(|control| control.package_id.as_str()).unwrap_or("unknown"),
            "game": control.map(|control| control.game.as_str()).unwrap_or("unknown"),
            "server": control.map(|control| control.server.as_str()).unwrap_or("unknown"),
            "instance": self.instance,
            "started_at": timestamp_iso(self.started_at),
            "finished_at": timestamp_iso(finished),
            "duration_ms": self.started_instant.elapsed().as_millis(),
            "input_zip_sha256": self.input_zip_sha256,
            "output_zip_sha256": Value::Null,
            "executed_step_count": self.steps.len(),
            "failed_step_id": state.and_then(|state| state.failed_step_id.as_deref()),
            "failure_reason": failure_reason,
            "screenshot_count": self.screenshots.len(),
            "requested_capture_interval_ms": self.requested_capture_interval_ms,
            "actual_capture_interval_min_ms": stats.map(|stats| stats.min),
            "actual_capture_interval_median_ms": stats.map(|stats| stats.median),
            "actual_capture_interval_max_ms": stats.map(|stats| stats.max),
            "capture_duration_min_ms": capture_stats.map(|stats| stats.min),
            "capture_duration_median_ms": capture_stats.map(|stats| stats.median),
            "capture_duration_max_ms": capture_stats.map(|stats| stats.max),
            "action_duration_min_ms": action_stats.map(|stats| stats.min),
            "action_duration_median_ms": action_stats.map(|stats| stats.median),
            "action_duration_max_ms": action_stats.map(|stats| stats.max),
            "loop_lag_min_ms": lag_stats.map(|stats| stats.min),
            "loop_lag_median_ms": lag_stats.map(|stats| stats.median),
            "loop_lag_max_ms": lag_stats.map(|stats| stats.max),
            "capture_backend_requested": self.capture_backend_requested.map(|backend| backend.as_str()),
            "capture_backend_used": self.capture_backend_used.map(|backend| backend.as_str()),
            "screenshots": self.screenshots.iter().map(|record| json!({
                "frame_index": record.frame_index,
                "file": record.file,
                "width": record.width,
                "height": record.height
            })).collect::<Vec<_>>(),
            "steps": self.steps
        })
    }

    fn diagnostics_json(&self, failure_reason: Option<&str>, state: Option<&RunState>) -> Value {
        json!({
            "actinglab_cli_version": env!("CARGO_PKG_VERSION"),
            "runtime_version": "runtime-embedded-lab1y",
            "runtime_commit": git_commit(),
            "os": std::env::consts::OS,
            "timezone": "UTC",
            "adb_path": self.adb_path,
            "serial": self.instance,
            "capture_backend_requested": self.capture_backend_requested.map(|backend| backend.as_str()),
            "capture_backend_used": self.capture_backend_used.map(|backend| backend.as_str()),
            "capture_backend_attempts": self.capture_backend_attempts.iter().map(|attempt| json!({
                "backend": attempt.backend.as_str(),
                "ok": attempt.ok,
                "message": attempt.message
            })).collect::<Vec<_>>(),
            "input_structure": self.input_entries,
            "resource_load_results": state.map(|state| json!({
                "manifest": state.resources.manifest_path,
                "operation": state.resources.operation_path,
                "resource_root": state.resources.resource_root,
                "pack": state.resources.pack_path,
                "pages": state.resources.pages_path,
                "navigation": state.resources.navigation_path,
                "navigation_loaded": state.resources.navigation.is_some(),
                "operation_goal": state.resources.operation_bundle.goal,
                "entry_page": state.resources.operation_bundle.entry_page,
                "target_page": state.resources.operation_bundle.target_page,
                "operation_defaults": state.resources.operation_bundle.defaults.to_json()
            })),
            "interval_stats": interval_stats(&self.intervals_ms).map(|stats| json!({
                "min_ms": stats.min,
                "median_ms": stats.median,
                "max_ms": stats.max,
                "count": stats.count
            })),
            "capture_duration_stats": interval_stats(&self.capture_durations_ms).map(|stats| json!({
                "min_ms": stats.min,
                "median_ms": stats.median,
                "max_ms": stats.max,
                "count": stats.count
            })),
            "action_duration_stats": interval_stats(&self.action_durations_ms).map(|stats| json!({
                "min_ms": stats.min,
                "median_ms": stats.median,
                "max_ms": stats.max,
                "count": stats.count
            })),
            "loop_lag_stats": interval_stats(&self.loop_lag_ms).map(|stats| json!({
                "min_ms": stats.min,
                "median_ms": stats.median,
                "max_ms": stats.max,
                "count": stats.count
            })),
            "error": failure_reason.map(|message| json!({
                "code": "lab1y_failed",
                "exception": message,
                "failure_phase": self.phase
            }))
        })
    }

    fn environment_json(&self, state: Option<&RunState>) -> Value {
        json!({
            "os": std::env::consts::OS,
            "timezone": "UTC",
            "local_time": timestamp_iso(SystemTime::now()),
            "cwd": std::env::current_dir().ok().map(|path| path.display().to_string()),
            "run_root": self.run_dir.parent().map(|path| path.display().to_string()),
            "run_dir": self.run_dir,
            "adb_path": self.adb_path,
            "instance_serial": self.instance,
            "runtime_repository_commit": git_commit(),
            "control_output": self.control.as_ref().and_then(|control| control.output.clone()),
            "control_stop_on_error": self.control.as_ref().and_then(|control| control.stop_on_error),
            "resource_manifest": state.map(|state| state.resources.manifest.clone())
        })
    }

    fn result_markdown(
        &self,
        ok: bool,
        failure_reason: Option<&str>,
        state: Option<&RunState>,
    ) -> String {
        let control = self
            .control
            .as_ref()
            .or_else(|| state.map(|state| &state.control));
        format!(
            "# Lab-1y Result\n\n- Package: {}\n- Game: {}\n- Server: {}\n- Instance: {}\n- Success: {}\n- Failure: {}\n- Screenshots: {}\n- Run ID: {}\n",
            control
                .map(|control| control.package_id.as_str())
                .unwrap_or("unknown"),
            control
                .map(|control| control.game.as_str())
                .unwrap_or("unknown"),
            control
                .map(|control| control.server.as_str())
                .unwrap_or("unknown"),
            self.instance.as_deref().unwrap_or("unknown"),
            ok,
            failure_reason.unwrap_or("none"),
            self.screenshots.len(),
            self.run_id
        )
    }
}

struct CapturedScene {
    scene: Scene,
    matched_page: Option<String>,
    verify_template_matched: bool,
    width: u32,
    height: u32,
}

impl CapturedScene {
    fn matched_anchor(&self, game: &str) -> Option<String> {
        self.matched_page
            .as_deref()
            .map(|page| canonical_page_anchor(game, page))
    }
}

struct ScreenshotRecord {
    frame_index: usize,
    file: String,
    width: u32,
    height: u32,
}

struct ArchiveResult {
    path: PathBuf,
    sha256: String,
}

struct LabLeaseGuard {
    path: PathBuf,
    _file: File,
}

impl LabLeaseGuard {
    fn acquire(serial: &str) -> CliOutcome<Self> {
        let root = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("ActingCommand")
            .join("actinglab")
            .join("locks");
        fs::create_dir_all(&root).map_err(|err| {
            CliError::package_invalid(format!(
                "failed to create LabLease lock directory {}: {err}",
                root.display()
            ))
        })?;
        let path = root.join(format!("{}.lock", sanitize_path_segment(serial)));
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .map_err(|err| {
                if err.kind() == std::io::ErrorKind::AlreadyExists {
                    CliError::safety_blocked(
                        "lab_lease_lock_conflict",
                        format!(
                            "LabLease lock already exists for instance {serial}: {}",
                            path.display()
                        ),
                        &["lab_lease"],
                    )
                } else {
                    CliError::package_invalid(format!(
                        "failed to acquire LabLease lock {}: {err}",
                        path.display()
                    ))
                }
            })?;
        Ok(Self { path, _file: file })
    }
}

impl Drop for LabLeaseGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[derive(Debug, Clone, Copy)]
struct IntervalStats {
    min: u64,
    median: u64,
    max: u64,
    count: usize,
}

fn interval_stats(values: &[u64]) -> Option<IntervalStats> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    Some(IntervalStats {
        min: sorted[0],
        median: sorted[sorted.len() / 2],
        max: *sorted.last().expect("non-empty"),
        count: sorted.len(),
    })
}

struct LabInput {
    entries: Vec<String>,
}

fn unpack_lab_input(zip_path: &Path, input_dir: &Path) -> CliOutcome<LabInput> {
    let file = File::open(zip_path).map_err(|err| {
        CliError::package_invalid(format!(
            "failed to open input zip {}: {err}",
            zip_path.display()
        ))
    })?;
    let mut archive = ZipArchive::new(file).map_err(|err| {
        CliError::package_invalid(format!(
            "failed to read input zip {}: {err}",
            zip_path.display()
        ))
    })?;
    let mut seen = BTreeSet::new();
    let mut entries = Vec::new();
    let mut has_control = false;
    let mut has_resources = false;
    let mut dangerous = Vec::new();

    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|err| CliError::package_invalid(format!("failed to read zip entry: {err}")))?;
        let Some(path_name) = normalize_lab_zip_path(entry.name())? else {
            continue;
        };
        let duplicate_key = path_name.to_ascii_lowercase();
        if !seen.insert(duplicate_key) {
            return Err(CliError::package_invalid(format!(
                "duplicate zip entry: {path_name}"
            )));
        }
        if has_dangerous_extension(&path_name) {
            dangerous.push(path_name.clone());
        }
        has_control |= path_name == "control.json";
        has_resources |= path_name.starts_with("resources/");
        let target = input_dir.join(&path_name);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                CliError::package_invalid(format!("failed to create {}: {err}", parent.display()))
            })?;
        }
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).map_err(|err| {
            CliError::package_invalid(format!("failed to read zip entry {path_name}: {err}"))
        })?;
        fs::write(&target, bytes).map_err(|err| {
            CliError::package_invalid(format!("failed to write {}: {err}", target.display()))
        })?;
        entries.push(path_name);
    }
    if !dangerous.is_empty() {
        return Err(CliError::package_invalid(format!(
            "input zip contains executable/script entries: {}",
            dangerous.join(", ")
        )));
    }
    if !has_control {
        return Err(CliError::package_invalid("missing control.json"));
    }
    if !has_resources {
        return Err(CliError::package_invalid("missing resources/"));
    }
    Ok(LabInput { entries })
}

fn normalize_lab_zip_path(name: &str) -> CliOutcome<Option<String>> {
    if name.ends_with('/') {
        return Ok(None);
    }
    if name.contains('\\') || name.contains(':') || name.starts_with('/') {
        return Err(CliError::package_invalid(format!(
            "unsafe zip path: {name}"
        )));
    }
    let path = Path::new(name);
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(CliError::package_invalid(format!(
            "zip-slip path is not allowed: {name}"
        )));
    }
    Ok(Some(name.to_string()))
}

fn has_dangerous_extension(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            super::DANGEROUS_EXTENSIONS
                .iter()
                .any(|dangerous| extension.eq_ignore_ascii_case(dangerous))
        })
}

fn safe_join(root: &Path, relative: &str) -> CliOutcome<PathBuf> {
    validate_relative_path(relative)?;
    Ok(root.join(relative))
}

fn validate_relative_path(relative: &str) -> CliOutcome<()> {
    if relative.contains('\\') || relative.contains(':') || relative.starts_with('/') {
        return Err(CliError::package_invalid(format!(
            "unsafe resource path: {relative}"
        )));
    }
    if Path::new(relative).components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(CliError::package_invalid(format!(
            "resource path escapes root: {relative}"
        )));
    }
    Ok(())
}

fn read_json_file<T>(path: &Path) -> CliOutcome<T>
where
    T: for<'de> Deserialize<'de>,
{
    let text = read_text_file(path)?;
    serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|err| {
        CliError::package_invalid(format!("failed to parse {}: {err}", path.display()))
    })
}

fn read_text_file(path: &Path) -> CliOutcome<String> {
    fs::read_to_string(path).map_err(|err| {
        CliError::package_invalid(format!("failed to read {}: {err}", path.display()))
    })
}

fn read_json_value(path: &Path) -> CliOutcome<Value> {
    read_json_file(path)
}

fn write_json(path: &Path, value: &Value) -> CliOutcome<()> {
    let text = serde_json::to_vec_pretty(value).map_err(|err| {
        CliError::package_invalid(format!("failed to serialize {}: {err}", path.display()))
    })?;
    fs::write(path, text).map_err(|err| {
        CliError::package_invalid(format!("failed to write {}: {err}", path.display()))
    })
}

fn write_json_lines(path: &Path, values: &[Value]) -> CliOutcome<()> {
    let mut file = File::create(path).map_err(|err| {
        CliError::package_invalid(format!("failed to create {}: {err}", path.display()))
    })?;
    for value in values {
        let line = serde_json::to_string(value).map_err(|err| {
            CliError::package_invalid(format!("failed to serialize {}: {err}", path.display()))
        })?;
        writeln!(file, "{line}").map_err(|err| {
            CliError::package_invalid(format!("failed to write {}: {err}", path.display()))
        })?;
    }
    Ok(())
}

fn write_output_zip(output_dir: &Path, out_path: &Path) -> CliOutcome<()> {
    if let Some(parent) = out_path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|err| {
            CliError::package_invalid(format!("failed to create {}: {err}", parent.display()))
        })?;
    }
    let file = File::create(out_path).map_err(|err| {
        CliError::package_invalid(format!("failed to create {}: {err}", out_path.display()))
    })?;
    let mut zip = ZipWriter::new(file);
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zip.add_directory("logs/", options)
        .map_err(|err| CliError::package_invalid(format!("failed to add logs directory: {err}")))?;
    zip.add_directory("screenshots/", options).map_err(|err| {
        CliError::package_invalid(format!("failed to add screenshots directory: {err}"))
    })?;
    add_zip_dir(&mut zip, output_dir, &output_dir.join("logs"), options)?;
    add_zip_dir(
        &mut zip,
        output_dir,
        &output_dir.join("screenshots"),
        options,
    )?;
    zip.finish()
        .map_err(|err| CliError::package_invalid(format!("failed to finish output zip: {err}")))?;
    Ok(())
}

fn add_zip_dir(
    zip: &mut ZipWriter<File>,
    root: &Path,
    dir: &Path,
    options: FileOptions,
) -> CliOutcome<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(dir).map_err(|err| {
        CliError::package_invalid(format!("failed to list {}: {err}", dir.display()))
    })? {
        let entry = entry.map_err(|err| {
            CliError::package_invalid(format!("failed to read directory entry: {err}"))
        })?;
        let path = entry.path();
        if path.is_dir() {
            add_zip_dir(zip, root, &path, options)?;
        } else {
            let relative = path.strip_prefix(root).map_err(|err| {
                CliError::package_invalid(format!("failed to relativize {}: {err}", path.display()))
            })?;
            let name = path_to_zip_name(relative)?;
            zip.start_file(name, options).map_err(|err| {
                CliError::package_invalid(format!("failed to start zip file: {err}"))
            })?;
            let bytes = fs::read(&path).map_err(|err| {
                CliError::package_invalid(format!("failed to read {}: {err}", path.display()))
            })?;
            zip.write_all(&bytes).map_err(|err| {
                CliError::package_invalid(format!("failed to write output zip: {err}"))
            })?;
        }
    }
    Ok(())
}

fn path_to_zip_name(path: &Path) -> CliOutcome<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => parts.push(value.to_string_lossy().to_string()),
            _ => {
                return Err(CliError::package_invalid(format!(
                    "invalid output zip path {}",
                    path.display()
                )));
            }
        }
    }
    Ok(parts.join("/"))
}

fn page_evaluation_json(evaluation: &PageEvaluation) -> Value {
    json!({
        "page": evaluation.page_id,
        "matched": evaluation.matched,
        "message": evaluation.message,
        "required_passed": evaluation.required_passed,
        "required_total": evaluation.required_total,
        "optional_passed": evaluation.optional_passed,
        "optional_total": evaluation.optional_total,
        "forbidden_passed": evaluation.forbidden_passed,
        "forbidden_total": evaluation.forbidden_total,
        "targets": evaluation.target_results.iter().map(|target| json!({
            "id": target.target_id,
            "role": format!("{:?}", target.role),
            "passed": target.passed,
            "message": target.message
        })).collect::<Vec<_>>()
    })
}

fn rect_json(rect: PackRect) -> Value {
    json!({"x": rect.x, "y": rect.y, "width": rect.width, "height": rect.height})
}

fn parse_optional_u64(flags: &FlagArgs, name: &str) -> CliOutcome<Option<u64>> {
    flags
        .optional(name)
        .filter(|value| value != "true")
        .map(|value| {
            value.parse::<u64>().map_err(|err| {
                CliError::usage(format!("failed to parse {name} value '{value}': {err}"))
            })
        })
        .transpose()
}

fn parse_optional_capture_backend(
    flags: &FlagArgs,
    name: &str,
) -> CliOutcome<Option<CaptureBackendChoice>> {
    flags
        .optional(name)
        .filter(|value| value != "true")
        .map(|value| {
            CaptureBackendChoice::parse(&value).map_err(|err| CliError::usage(err.to_string()))
        })
        .transpose()
}

fn file_sha256(path: &Path) -> CliOutcome<String> {
    let bytes = fs::read(path).map_err(|err| {
        CliError::package_invalid(format!("failed to read {}: {err}", path.display()))
    })?;
    Ok(hex_sha256(&bytes))
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hash_text(text: &str) -> u64 {
    let digest = Sha256::digest(text.as_bytes());
    u64::from_be_bytes([
        digest[0], digest[1], digest[2], digest[3], digest[4], digest[5], digest[6], digest[7],
    ])
}

fn sanitize_path_segment(value: &str) -> String {
    let mut output = String::new();
    let mut previous_underscore = false;
    for ch in value.chars() {
        let safe = if ch.is_ascii_alphanumeric() { ch } else { '_' };
        if safe == '_' {
            if !previous_underscore {
                output.push(safe);
            }
            previous_underscore = true;
        } else {
            output.push(safe);
            previous_underscore = false;
        }
    }
    let output = output.trim_matches('_').to_string();
    if output.is_empty() {
        "unknown".to_string()
    } else {
        output
    }
}

fn git_commit() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn timestamp_iso(time: SystemTime) -> String {
    let (date, h, m, s, ms) = timestamp_parts(time);
    format!("{date}T{h:02}:{m:02}:{s:02}.{ms:03}Z")
}

fn timestamp_file_stem(time: SystemTime) -> String {
    let (date, h, m, s, ms) = timestamp_parts(time);
    format!("{}_{h:02}{m:02}{s:02}_{ms:03}", date.replace('-', ""))
}

fn timestamp_parts(time: SystemTime) -> (String, u64, u64, u64, u32) {
    let duration = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let seconds = duration.as_secs();
    let days = seconds / 86_400;
    let seconds_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    (
        format!("{year:04}-{month:02}-{day:02}"),
        seconds_of_day / 3600,
        (seconds_of_day % 3600) / 60,
        seconds_of_day % 60,
        duration.subsec_millis(),
    )
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if m <= 2 { 1 } else { 0 };
    (year, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn rejects_missing_control_and_writes_failure_zip() {
        let temp = TempDir::new().expect("temp");
        let zip = temp.path().join("input.zip");
        write_test_zip(&zip, &[("resources/manifest.json", br#"{}"#)]);
        let out = temp.path().join("out.zip");
        let result = super::super::run_cli(
            [
                "--json",
                "--run-root",
                temp.path().join("runs").to_str().unwrap(),
                "lab",
                "run",
                "--zip",
                zip.to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
                "--instance",
                "127.0.0.1:1",
            ],
            true,
        );

        assert_eq!(result.exit_code(), 2);
        assert!(out.is_file());
    }

    #[test]
    fn rejects_fullscreen_rect_unless_explicitly_allowed() {
        let control = LabControl {
            schema_version: CONTROL_SCHEMA.to_string(),
            package_id: "pkg".to_string(),
            execution_mode: "navigable_route".to_string(),
            game: "arknights".to_string(),
            server: "cn".to_string(),
            resolution: Resolution {
                width: 1280,
                height: 720,
            },
            entry_task_id: "task".to_string(),
            capture_interval_ms: None,
            timeout_ms: None,
            step_timeout_ms: None,
            max_steps: None,
            stop_on_error: None,
            stop_on_confirmation: None,
            resource_root: None,
            allow_placeholder_coords: None,
            output: None,
            capture_backend: None,
            producer: None,
            trusted_execution: None,
        };
        let click = OperationClick {
            kind: "rect".to_string(),
            x: Some(0),
            y: Some(0),
            width: Some(1280),
            height: Some(720),
            from_rect: None,
            to_rect: None,
            duration_ms: None,
        };

        let err = click.validate(&control).expect_err("fullscreen rejected");
        assert_eq!(err.code, "package_invalid");
    }

    #[test]
    fn page_namespace_matches_operation_anchors_without_blind_split() {
        assert_eq!(canonical_page_anchor("arknights", "arknights/home"), "home");
        assert_eq!(
            canonical_page_anchor("arknights", "arknights/navigation/home_to_task"),
            "navigation/home_to_task"
        );
        assert_eq!(canonical_page_anchor("arknights", "home"), "home");
        assert!(page_anchor_matches("arknights", "arknights/home", "home"));
        assert!(page_anchor_matches("arknights", "home", "home"));
        assert!(page_anchor_matches(
            "arknights",
            "arknights/quickswitch_dropdown",
            "quickswitch_dropdown"
        ));
        assert!(!page_anchor_matches(
            "arknights",
            "bluearchive/home",
            "home"
        ));
    }

    #[test]
    fn operation_verification_marks_to_null_without_template_unverified() {
        let operation = test_operation(None, None);
        let scene = captured_scene(Some("arknights/home"), false);

        let result = operation_verification_status("arknights", &operation, &scene);

        assert_eq!(result, OperationVerification::ExecutedUnverified);
        assert_eq!(result.result_label(), "executed_unverified");
    }

    #[test]
    fn operation_verification_requires_template_when_to_is_null_with_template() {
        let operation = test_operation(None, Some("terminal.png"));
        let failed = captured_scene(Some("arknights/home"), false);
        let passed = captured_scene(Some("arknights/home"), true);

        assert_eq!(
            operation_verification_status("arknights", &operation, &failed),
            OperationVerification::Failed
        );
        assert_eq!(
            operation_verification_status("arknights", &operation, &passed),
            OperationVerification::Verified
        );
    }

    #[test]
    fn operation_verification_accepts_namespaced_arrival_page() {
        let operation = test_operation(Some("terminal"), None);
        let scene = captured_scene(Some("arknights/terminal"), false);

        assert_eq!(
            operation_verification_status("arknights", &operation, &scene),
            OperationVerification::Verified
        );
    }

    #[test]
    fn manifest_entry_task_id_conflict_is_fatal() {
        let control = test_control();
        let manifest = json!({"entry_task_id": "other_task"});

        let err = validate_manifest_entry_task_id(Path::new("manifest.json"), &manifest, &control)
            .expect_err("conflict is fatal");

        assert_eq!(err.code, "package_invalid");
        assert!(err.message.contains("conflicts with control entry_task_id"));
    }

    #[test]
    fn screenshot_names_are_timestamp_based_with_suffixes() {
        let temp = TempDir::new().expect("temp");
        let mut ctx = LabRunContext::create(temp.path(), Path::new("input.zip")).expect("ctx");
        let time = UNIX_EPOCH + Duration::from_millis(1_672_531_200_123);
        let first = ctx.next_screenshot_name(time);
        let second = ctx.next_screenshot_name(time);

        assert!(first.ends_with(".png"));
        assert!(second.ends_with("_02.png"));
        assert!(first.starts_with("20230101_000000_123"));
    }

    fn test_control() -> LabControl {
        LabControl {
            schema_version: CONTROL_SCHEMA.to_string(),
            package_id: "pkg".to_string(),
            execution_mode: "navigable_route".to_string(),
            game: "arknights".to_string(),
            server: "cn".to_string(),
            resolution: Resolution {
                width: 1280,
                height: 720,
            },
            entry_task_id: "task".to_string(),
            capture_interval_ms: None,
            timeout_ms: None,
            step_timeout_ms: None,
            max_steps: None,
            stop_on_error: None,
            stop_on_confirmation: None,
            resource_root: None,
            allow_placeholder_coords: None,
            output: None,
            capture_backend: None,
            producer: None,
            trusted_execution: None,
        }
    }

    fn test_operation(to: Option<&str>, verify_template: Option<&str>) -> Operation {
        Operation {
            id: "open_terminal".to_string(),
            purpose: "test".to_string(),
            from: "home".to_string(),
            to: to.map(str::to_string),
            click: OperationClick {
                kind: "point".to_string(),
                x: Some(100),
                y: Some(100),
                width: None,
                height: None,
                from_rect: None,
                to_rect: None,
                duration_ms: None,
            },
            verify_template: verify_template.map(str::to_string),
            consumes: Vec::new(),
            produces: Vec::new(),
            verified_live: None,
            provenance: None,
        }
    }

    fn captured_scene(page: Option<&str>, verify_template_matched: bool) -> CapturedScene {
        CapturedScene {
            scene: Scene::from_png(one_pixel_png()).expect("scene"),
            matched_page: page.map(str::to_string),
            verify_template_matched,
            width: 1,
            height: 1,
        }
    }

    fn one_pixel_png() -> &'static [u8] {
        &[
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1,
            8, 6, 0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 10, 73, 68, 65, 84, 120, 156, 99, 0, 1, 0, 0,
            5, 0, 1, 13, 10, 45, 180, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
        ]
    }

    fn write_test_zip(path: &Path, files: &[(&str, &[u8])]) {
        let file = File::create(path).expect("zip file");
        let mut zip = ZipWriter::new(file);
        let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, content) in files {
            zip.start_file(*name, options).expect("start file");
            zip.write_all(content).expect("write file");
        }
        zip.finish().expect("finish");
    }
}
