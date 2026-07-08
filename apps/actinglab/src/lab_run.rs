// SPDX-License-Identifier: AGPL-3.0-only

use super::{
    CliError, CliOutcome, FlagArgs, GlobalOptions, device_config, effective_adb_path,
    effective_run_root,
    frame_store::{
        FrameStore, FrameStoreConfig, FrameStoreControl, FrameStoreFrameInput,
        FrameStoreScreenshot as ScreenshotRecord, RecognitionState, Tier3PauseCheckpoint,
    },
    read_user_config,
};
use actingcommand_device::{
    CaptureBackend, CaptureBackendAttempt, CaptureBackendChoice, CaptureBackendConfig,
    CaptureBackendName, DeviceTarget, Frame, InputBackend, PixelFormat, SelectedTouchBackend,
    TouchBackendConfig, combine_operation_and_close, create_capture_backend, create_touch_backend,
};
use actingcommand_ledger::{
    CommitProof, EvidenceStore, IdIssuer, IdKind, LabLedger, LabLogError, LastResortError,
    LedgerRecord, LedgerRecordKind, LightEvent, SessionHeader, commit_then_record,
    project_light_events, write_last_resort_error,
};
use actingcommand_pack_containment::{
    Containment, ContainmentError, InstanceId, LoadedBundle, Sha256Hash,
};
use actingcommand_page_detector::{PageDetector, PageEvaluation};
use actingcommand_recognition::{Scene, ScenePixelFormat};
use actingcommand_recognition_pack::{
    PackRect, RecognitionEvaluator, TargetEvaluation, TargetKind, UnsupportedRecognitionTarget,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use zip::ZipWriter;
use zip::write::FileOptions;

const CONTROL_SCHEMA: &str = "Lab-1y.control.v1";
const SUMMARY_SCHEMA: &str = "Lab-1y.summary.v1";
const DEFAULT_CAPTURE_INTERVAL_MS: u64 = 300;
const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const DEFAULT_STEP_TIMEOUT_MS: u64 = 10_000;
const DEFAULT_MAX_STEPS: usize = 50;
const DEFAULT_TEMPLATE_THRESHOLD: f32 = 0.9;
const DEFAULT_ROI_STABLE_FRAMES: u32 = 2;
const DEFAULT_ROI_STABILITY_TIMEOUT_MS: u64 = 1_500;
const DEFAULT_RESOURCE_DRIFT_FRAMES: u32 = 2;
const ROI_TEMPLATE_SCORE_EPSILON: f32 = 0.01;
const ROI_TEMPLATE_POSITION_EPSILON: i32 = 1;
const ROI_COLOR_DISTANCE_EPSILON: f32 = 2.0;
const ROI_COLOR_MEAN_EPSILON: u8 = 2;
const GIT_COMMIT_TIMEOUT: Duration = Duration::from_secs(3);

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
    let frame_store_cli = parse_frame_store_control_from_flags(&flags)?;
    let expected_input_sha256 = parse_optional_sha256(&flags, "--expected-sha256")?;
    let options = LabRunOptions {
        capture_interval_override,
        capture_backend_override,
        frame_store_cli,
        expected_input_sha256,
    };

    let mut ctx = LabRunContext::create(&run_root, &zip_path)?;
    let run_dir = ctx.run_dir.clone();
    if path_is_inside(&out_path, &run_dir) {
        return Err(CliError::usage(
            "--out must not be inside the Lab run directory",
        ));
    }
    let run_dir_string = run_dir.display().to_string();
    ctx.set_phase("run_started");
    ctx.event(
        "run_started",
        json!({"input_zip": zip_path, "out": out_path}),
    )?;

    let result = execute_lab_run(&mut ctx, global, &config, &zip_path, options);
    match result {
        Ok(run_state) => {
            ctx.finish(&out_path, true, None, Some(&run_state))?;
            let completed = ctx.project_completed_run_from_ledger()?;
            let output_zip = completed.require_output_zip()?;
            Ok(json!({
                "ok": completed.ok,
                "status": completed.status,
                "run_id": completed.run_id,
                "result_zip": output_zip.path,
                "run_dir": run_dir_string,
                "run_dir_cleaned": true,
                "out": output_zip.path,
                "output_zip_sha256": output_zip.sha256,
                "ledger": {
                    "projection_source": "runtime_ledger",
                    "path": completed.ledger_path.display().to_string(),
                    "terminal_receipt": completed.record_type
                },
                "screenshot_count": ctx.screenshots.len(),
                "executed_step_count": ctx.steps.len()
            }))
        }
        Err(err) => {
            ctx.set_phase("run_failed");
            let message = err.message.clone();
            let archive = ctx.finish(&out_path, false, Some(&message), None);
            match archive {
                Ok(_) => {
                    let completed = ctx.project_completed_run_from_ledger()?;
                    let output_zip = completed.require_output_zip()?;
                    let mut err = err;
                    err.message = format!(
                        "{}; failure report written to {}",
                        err.message, output_zip.path
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

pub(super) fn run_lab_validate(args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let zip_path = flags.required_path("--zip")?;
    let expected_input_sha256 = parse_optional_sha256(&flags, "--expected-sha256")?;
    validate_lab_package_zip_with_expected(&zip_path, expected_input_sha256)
}

pub(super) fn validate_lab_package_zip(zip_path: &Path) -> CliOutcome<Value> {
    validate_lab_package_zip_with_expected(zip_path, None)
}

fn validate_lab_package_zip_with_expected(
    zip_path: &Path,
    expected_input_sha256: Option<Sha256Hash>,
) -> CliOutcome<Value> {
    let contained =
        load_lab_package_through_containment(zip_path, "lab-validate", expected_input_sha256)?;
    let entry_count = contained.bundle.entry_count();
    let control = lab_control_from_bundle(&contained.bundle)?;
    control.validate()?;
    let resources = load_lab_resources_from_bundle(contained.bundle, &control)?;
    Ok(json!({
        "zip": zip_path.display().to_string(),
        "status": "valid",
        "entry_count": entry_count,
        "control": {
            "package_id": control.package_id,
            "execution_mode": control.execution_mode,
            "game": control.game,
            "server": control.server,
            "resolution": {
                "width": control.resolution.width,
                "height": control.resolution.height
            },
            "entry_task_id": control.entry_task_id
        },
        "resources": {
            "resource_root": resources.resource_root.display().to_string(),
            "manifest": resources.manifest_path.display().to_string(),
            "operation": resources.operation_path.display().to_string(),
            "operation_count": resources.operation_bundle.operations.len(),
            "pack": resources.pack_path.display().to_string(),
            "recognition_unsupported_target_count": resources.evaluator.unsupported_target_count(),
            "recognition_unsupported_targets": unsupported_targets_json(resources.evaluator.unsupported_targets()),
            "pages": resources.pages_path.display().to_string(),
            "navigation": resources.navigation_path.as_ref().map(|path| path.display().to_string())
        }
    }))
}

struct LabRunOptions {
    capture_interval_override: Option<u64>,
    capture_backend_override: Option<CaptureBackendChoice>,
    frame_store_cli: FrameStoreControl,
    expected_input_sha256: Option<Sha256Hash>,
}

fn execute_lab_run(
    ctx: &mut LabRunContext,
    global: &GlobalOptions,
    config: &super::UserConfig,
    zip_path: &Path,
    options: LabRunOptions,
) -> CliOutcome<RunState> {
    ctx.set_phase("input_unpacked");
    let contained =
        load_lab_package_through_containment(zip_path, "lab-run", options.expected_input_sha256)?;
    ctx.input_zip_sha256 = Some(contained.sha256.clone());
    ctx.input_entries = contained.bundle.entry_paths().map(str::to_string).collect();
    ctx.event(
        "input_unpacked",
        json!({"entry_count": ctx.input_entries.len(), "containment": "memory", "input_sha256": contained.sha256}),
    )?;

    ctx.set_phase("control_loaded");
    let control = lab_control_from_bundle(&contained.bundle)?;
    control.validate()?;
    ctx.control = Some(control.clone());
    let mut frame_store_config = FrameStoreConfig::default();
    control.frame_store.apply_to(&mut frame_store_config);
    options.frame_store_cli.apply_to(&mut frame_store_config);
    ctx.set_frame_store_config(frame_store_config)?;
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

    ctx.requested_capture_interval_ms = options.capture_interval_override.unwrap_or(
        control
            .capture_interval_ms
            .unwrap_or(DEFAULT_CAPTURE_INTERVAL_MS),
    );
    let timeout_ms = control.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);
    let step_timeout_ms = control.step_timeout_ms.unwrap_or(DEFAULT_STEP_TIMEOUT_MS);
    let max_steps = control.max_steps.unwrap_or(DEFAULT_MAX_STEPS);

    ctx.set_phase("resources_loaded");
    let resources = load_lab_resources_from_bundle(contained.bundle, &control)?;
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
    ctx.adb_path = Some(effective_adb_path(config)?.path);
    ctx.ensure_ledger()?;

    ctx.set_phase("lab_lease_acquired");
    let _lease_guard = LabLeaseGuard::acquire(&device.target.resolved_serial())?;
    ctx.event(
        "lab_lease_acquired",
        json!({"mode": "trusted_execution", "instance": ctx.instance}),
    )?;
    ctx.lease_acquired = true;

    let requested_capture_backend = global
        .capture_backend
        .or(options.capture_backend_override)
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
            capture_backend_attempt_json(&attempt),
        )?;
    }
    ctx.record_capture_backend_selection()?;
    let mut capture = selected_capture.backend;
    let mut input = None::<SelectedTouchBackend>;
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

        ctx.set_step_context(step_index, &operation);
        ctx.event(
            "step_started",
            json!({"step_id": operation.id, "index": step_index, "operation_id": operation.id}),
        )?;
        ctx.event(
            "before_page_detected",
            json!({"step_id": operation.id, "page": current_page}),
        )?;

        let stability_baseline = match pre_execution_guard(
            ctx,
            capture.as_mut(),
            &state.resources,
            &operation,
            &state.control.game,
            actionable_page_candidates.as_deref(),
        )? {
            PreExecutionGuardOutcome::Passed {
                current_page,
                target,
            } => {
                ctx.event(
                    "pre_execution_guard_passed",
                    json!({"step_id": operation.id, "page": current_page, "target": target_evaluation_json(&target)}),
                )?;
                Some((current_page, target))
            }
            PreExecutionGuardOutcome::TrustedUnguarded => {
                ctx.event(
                    "pre_execution_guard_skipped",
                    json!({"step_id": operation.id, "reason": "unguarded_trusted_coordinate"}),
                )?;
                None
            }
            PreExecutionGuardOutcome::TargetMismatch {
                current_page,
                target,
                diagnostics,
            } => {
                ctx.event(
                    "pre_execution_guard_failed",
                    json!({"step_id": operation.id, "reason": "target_guard_mismatch", "current_page": current_page.as_deref(), "diagnostics": diagnostics}),
                )?;
                match confirm_resource_drift(
                    ctx,
                    capture.as_mut(),
                    ResourceDriftRequest {
                        resources: &state.resources,
                        operation: &operation,
                        game: &state.control.game,
                        initial_page: current_page,
                        initial_target: target,
                        candidate_pages: actionable_page_candidates.as_deref(),
                    },
                )? {
                    ResourceDriftOutcome::Recovered {
                        current_page,
                        target,
                    } => {
                        ctx.event(
                            "pre_execution_guard_passed",
                            json!({"step_id": operation.id, "page": current_page.as_deref(), "target": target_evaluation_json(&target), "after": "target_guard_mismatch_recovered"}),
                        )?;
                        Some((current_page, target))
                    }
                    ResourceDriftOutcome::Failed {
                        reason,
                        current_page,
                        diagnostics,
                    } => {
                        if reason == "resource_drift" {
                            ctx.event(
                                "resource_drift_detected",
                                json!({"step_id": operation.id, "current_page": current_page.as_deref(), "diagnostics": diagnostics}),
                            )?;
                        } else {
                            ctx.event(
                                "pre_execution_guard_failed",
                                json!({"step_id": operation.id, "reason": reason, "current_page": current_page.as_deref(), "diagnostics": diagnostics}),
                            )?;
                        }
                        ctx.event(
                            "step_failed",
                            json!({"step_id": operation.id, "reason": reason}),
                        )?;
                        state.current_page = current_page.clone();
                        state.failed_step_id = Some(operation.id.clone());
                        return Err(CliError::device(format!(
                            "pre-execution guard failed for operation '{}': {reason}; current_page={}",
                            operation.id,
                            current_page.unwrap_or_else(|| "unknown".to_string())
                        )));
                    }
                }
            }
            PreExecutionGuardOutcome::Failed {
                reason,
                current_page,
                diagnostics,
            } => {
                ctx.event(
                    "pre_execution_guard_failed",
                    json!({"step_id": operation.id, "reason": reason, "current_page": current_page, "diagnostics": diagnostics}),
                )?;
                ctx.event(
                    "step_failed",
                    json!({"step_id": operation.id, "reason": "pre_execution_guard_failed"}),
                )?;
                state.current_page = current_page.clone();
                state.failed_step_id = Some(operation.id.clone());
                return Err(CliError::device(format!(
                    "pre-execution guard failed for operation '{}': {reason}; current_page={}",
                    operation.id,
                    current_page.unwrap_or_else(|| "unknown".to_string())
                )));
            }
        };

        let mut action_target = None;
        if let Some((current_page, target)) = stability_baseline {
            match wait_for_roi_stability(
                ctx,
                capture.as_mut(),
                RoiStabilityRequest {
                    resources: &state.resources,
                    operation: &operation,
                    game: &state.control.game,
                    baseline_page: current_page,
                    baseline_target: target,
                    candidate_pages: actionable_page_candidates.as_deref(),
                },
            )? {
                RoiStabilityOutcome::Passed {
                    stable_frames,
                    observed_frames,
                    target,
                } => {
                    ctx.event(
                        "roi_stability_gate_passed",
                        json!({
                            "step_id": operation.id,
                            "stable_frames": stable_frames,
                            "observed_frames": observed_frames,
                            "target": target_evaluation_json(&target)
                        }),
                    )?;
                    action_target = Some(target);
                }
                RoiStabilityOutcome::Failed {
                    reason,
                    current_page,
                    diagnostics,
                } => {
                    ctx.event(
                        "roi_stability_gate_failed",
                        json!({"step_id": operation.id, "reason": reason, "current_page": current_page, "diagnostics": diagnostics}),
                    )?;
                    ctx.event(
                        "step_failed",
                        json!({"step_id": operation.id, "reason": reason}),
                    )?;
                    state.current_page = current_page.clone();
                    state.failed_step_id = Some(operation.id.clone());
                    return Err(CliError::device(format!(
                        "ROI stability gate failed for operation '{}': {reason}; current_page={}",
                        operation.id,
                        current_page.unwrap_or_else(|| "unknown".to_string())
                    )));
                }
            }
        }

        let action = operation.input_action(
            &state.control.resolution,
            ctx.run_seed,
            action_target.as_ref(),
        )?;
        let backend = ensure_touch_backend(
            &mut input,
            &device.target,
            &device.adb,
            device.touch_backend,
        )?;
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
            LabInputAction::LongTap { point, duration_ms } => {
                let action_started = Instant::now();
                ctx.event(
                    "long_tap_started",
                    json!({"step_id": operation.id, "actual_click_point": point.to_json(), "duration_ms": duration_ms}),
                )?;
                if let Err(err) = backend.long_tap(point.x, point.y, *duration_ms) {
                    return close_backend_after_error(
                        &mut input,
                        CliError::device(err.to_string()),
                    );
                }
                ctx.event(
                    "long_tap_finished",
                    json!({"step_id": operation.id, "actual_click_point": point.to_json(), "duration_ms": duration_ms}),
                )?;
                ctx.action_durations_ms
                    .push(action_started.elapsed().as_millis() as u64);
            }
        }

        ctx.event(
            "page_guard_started",
            json!({"step_id": operation.id, "to": operation.to, "expect_after": operation.expect_after.as_ref().map(OperationExpectation::to_json), "verify_template": operation.verify_template}),
        )?;
        let after = poll_after_operation(
            ctx,
            capture.as_mut(),
            &state.resources,
            &operation,
            operation.after_timeout_ms(step_timeout_ms),
            &state.control.game,
        )?;
        let verification = operation_verification_status(&state.control.game, &operation, &after);
        if verification == OperationVerification::Failed {
            ctx.event(
                "page_guard_failed",
                json!({"step_id": operation.id, "expected": operation.expected_after_page(), "after_page": after.matched_page}),
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

        let step_record = json!({
            "id": operation.id,
            "operation_id": operation.id,
            "purpose": operation.purpose,
            "from": operation.from,
            "to": operation.to,
            "expect_after": operation.expect_after.as_ref().map(OperationExpectation::to_json),
            "before_page": current_page,
            "after_page": after.matched_page,
            "after_anchor": after.matched_anchor(&state.control.game),
            "click_count": if matches!(action, LabInputAction::Tap(_)) { 1 } else { 0 },
            "drag_count": if matches!(action, LabInputAction::Drag { .. }) { 1 } else { 0 },
            "long_tap_count": if matches!(action, LabInputAction::LongTap { .. }) { 1 } else { 0 },
            "actual_input": action.to_json(),
            "consumes": operation.consumes,
            "produces": operation.produces,
            "verified_live": operation.verified_live,
            "provenance": operation.provenance,
            "guard": operation.guard.as_ref().map(OperationGuard::to_json),
            "unguarded_trusted_coordinate": operation.unguarded_trusted_coordinate,
            "result": verification.result_label()
        });
        ctx.append_ledger_record(
            ctx.ledger_record(
                LedgerRecordKind::Drive,
                json!({
                    "record_type": "step",
                    "phase": ctx.phase,
                    "step": step_record
                }),
            ),
            "ledger_step",
        )?;
        ctx.steps.push(step_record);
        ctx.event(
            "step_finished",
            json!({"step_id": operation.id, "result": verification.result_label()}),
        )?;
        state.current_page = next_current_page(&state.control.game, &after, &operation);
        ctx.clear_step_context();
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

struct ContainedLabInput {
    sha256: String,
    bundle: LoadedBundle,
}

fn load_lab_package_through_containment(
    zip_path: &Path,
    instance_label: &str,
    expected_input_sha256: Option<Sha256Hash>,
) -> CliOutcome<ContainedLabInput> {
    let bytes = fs::read(zip_path).map_err(|err| {
        CliError::package_invalid(format!(
            "failed to read Lab package {}: {err}",
            zip_path.display()
        ))
    })?;
    let expected = expected_input_sha256.unwrap_or_else(|| Sha256Hash::digest(&bytes));
    let instance = InstanceId::new(instance_label).map_err(containment_error)?;
    let mut containment = Containment::new();
    containment
        .load(&instance, &bytes, &expected)
        .map_err(containment_error)?;
    let bundle = containment
        .take_loaded(&instance)
        .ok_or_else(|| CliError::package_invalid("containment did not retain loaded Lab bundle"))?;
    Ok(ContainedLabInput {
        sha256: expected.to_string(),
        bundle,
    })
}

fn containment_error(err: ContainmentError) -> CliError {
    CliError::package_invalid(err.to_string())
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
    expected_anchor == "any"
        || observed_or_anchor == expected_anchor
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
            .expected_after_page()
            .map(|page| canonical_page_anchor(game, page))
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
        .expected_after_page()
        .is_some_and(|page| matched_page_matches_anchor(game, after.matched_page.as_deref(), page));
    let matched_template = operation.verify_template.is_some() && after.verify_template_matched;
    if matched_to || matched_template {
        return OperationVerification::Verified;
    }
    if operation.expected_after_page().is_none() && operation.verify_template.is_none() {
        return OperationVerification::ExecutedUnverified;
    }
    OperationVerification::Failed
}

#[derive(Debug, PartialEq)]
enum PreExecutionGuardOutcome {
    Passed {
        current_page: Option<String>,
        target: TargetEvaluation,
    },
    TrustedUnguarded,
    TargetMismatch {
        current_page: Option<String>,
        target: TargetEvaluation,
        diagnostics: Value,
    },
    Failed {
        reason: &'static str,
        current_page: Option<String>,
        diagnostics: Value,
    },
}

fn pre_execution_guard(
    ctx: &mut LabRunContext,
    capture: &mut dyn CaptureBackend,
    resources: &LabResources,
    operation: &Operation,
    game: &str,
    candidate_pages: Option<&[String]>,
) -> CliOutcome<PreExecutionGuardOutcome> {
    if operation.unguarded_trusted_coordinate {
        return Ok(PreExecutionGuardOutcome::TrustedUnguarded);
    }
    let guard = operation.guard.as_ref().ok_or_else(|| {
        CliError::package_invalid(format!(
            "operation '{}' coordinate action missing guard metadata",
            operation.id
        ))
    })?;
    ctx.event(
        "pre_execution_guard_started",
        json!({"step_id": operation.id, "guard": guard.to_json()}),
    )?;
    ctx.wait_for_next_capture_start();
    let scene = ctx.capture_scene_with_pages(
        capture,
        &resources.evaluator,
        &resources.detector,
        &format!("pre_execution_guard_{}", operation.id),
        candidate_pages,
    )?;
    evaluate_pre_execution_guard(game, operation, guard, &scene, &resources.evaluator)
}

fn evaluate_pre_execution_guard(
    game: &str,
    operation: &Operation,
    guard: &OperationGuard,
    scene: &CapturedScene,
    evaluator: &RecognitionEvaluator,
) -> CliOutcome<PreExecutionGuardOutcome> {
    let current_page = scene.matched_anchor(game);
    if !matched_page_matches_anchor(game, scene.matched_page.as_deref(), &guard.page_id) {
        return Ok(PreExecutionGuardOutcome::Failed {
            reason: "page_guard_mismatch",
            current_page,
            diagnostics: json!({
                "expected_page": guard.page_id,
                "matched_page": scene.matched_page,
                "operation_from": operation.from
            }),
        });
    }
    let target = evaluator
        .evaluate_target(&scene.scene, &guard.target_id)
        .map_err(|err| CliError::device(err.to_string()))?;
    if !target.passed {
        return Ok(PreExecutionGuardOutcome::TargetMismatch {
            current_page,
            target: target.clone(),
            diagnostics: json!({
                "guard": guard.to_json(),
                "target": target_evaluation_json(&target)
            }),
        });
    }
    Ok(PreExecutionGuardOutcome::Passed {
        current_page,
        target,
    })
}

#[derive(Debug, PartialEq)]
enum RoiStabilityOutcome {
    Passed {
        stable_frames: u32,
        observed_frames: u32,
        target: TargetEvaluation,
    },
    Failed {
        reason: &'static str,
        current_page: Option<String>,
        diagnostics: Value,
    },
}

struct RoiStabilityRequest<'a> {
    resources: &'a LabResources,
    operation: &'a Operation,
    game: &'a str,
    baseline_page: Option<String>,
    baseline_target: TargetEvaluation,
    candidate_pages: Option<&'a [String]>,
}

fn wait_for_roi_stability(
    ctx: &mut LabRunContext,
    capture: &mut dyn CaptureBackend,
    request: RoiStabilityRequest<'_>,
) -> CliOutcome<RoiStabilityOutcome> {
    let guard = request.operation.guard.as_ref().ok_or_else(|| {
        CliError::package_invalid(format!(
            "operation '{}' ROI stability gate missing guard metadata",
            request.operation.id
        ))
    })?;
    let mut gate =
        RoiStabilityGate::new(DEFAULT_ROI_STABLE_FRAMES, request.baseline_target.clone())?;
    ctx.event(
        "roi_stability_gate_started",
        json!({
            "step_id": request.operation.id,
            "required_stable_frames": DEFAULT_ROI_STABLE_FRAMES,
            "timeout_ms": DEFAULT_ROI_STABILITY_TIMEOUT_MS,
            "guard": guard.to_json(),
            "baseline_page": request.baseline_page.as_deref(),
            "baseline_target": target_evaluation_json(&request.baseline_target)
        }),
    )?;

    let started = Instant::now();
    while started.elapsed() <= Duration::from_millis(DEFAULT_ROI_STABILITY_TIMEOUT_MS) {
        ctx.wait_for_next_capture_start();
        let scene = ctx.capture_scene_with_pages(
            capture,
            &request.resources.evaluator,
            &request.resources.detector,
            &format!("roi_stability_{}", request.operation.id),
            request.candidate_pages,
        )?;
        let current_page = scene.matched_anchor(request.game);
        if !matched_page_matches_anchor(request.game, scene.matched_page.as_deref(), &guard.page_id)
        {
            return Ok(RoiStabilityOutcome::Failed {
                reason: "page_guard_mismatch",
                current_page,
                diagnostics: json!({
                    "expected_page": guard.page_id,
                    "matched_page": scene.matched_page,
                    "operation_from": request.operation.from
                }),
            });
        }
        let target = request
            .resources
            .evaluator
            .evaluate_target(&scene.scene, &guard.target_id)
            .map_err(|err| CliError::device(err.to_string()))?;
        if gate.observe(target.clone()) {
            return Ok(RoiStabilityOutcome::Passed {
                stable_frames: gate.stable_frames,
                observed_frames: gate.observed_frames,
                target,
            });
        }
    }

    Ok(RoiStabilityOutcome::Failed {
        reason: "unstable_page",
        current_page: request.baseline_page,
        diagnostics: json!({
            "guard": guard.to_json(),
            "required_stable_frames": DEFAULT_ROI_STABLE_FRAMES,
            "observed_frames": gate.observed_frames,
            "last_target": target_evaluation_json(&gate.last_target),
            "timeout_ms": DEFAULT_ROI_STABILITY_TIMEOUT_MS
        }),
    })
}

#[derive(Debug, PartialEq)]
enum ResourceDriftOutcome {
    Recovered {
        current_page: Option<String>,
        target: TargetEvaluation,
    },
    Failed {
        reason: &'static str,
        current_page: Option<String>,
        diagnostics: Value,
    },
}

struct ResourceDriftRequest<'a> {
    resources: &'a LabResources,
    operation: &'a Operation,
    game: &'a str,
    initial_page: Option<String>,
    initial_target: TargetEvaluation,
    candidate_pages: Option<&'a [String]>,
}

fn confirm_resource_drift(
    ctx: &mut LabRunContext,
    capture: &mut dyn CaptureBackend,
    request: ResourceDriftRequest<'_>,
) -> CliOutcome<ResourceDriftOutcome> {
    let guard = request.operation.guard.as_ref().ok_or_else(|| {
        CliError::package_invalid(format!(
            "operation '{}' resource drift probe missing guard metadata",
            request.operation.id
        ))
    })?;
    let mut gate = ResourceDriftGate::new(
        DEFAULT_RESOURCE_DRIFT_FRAMES,
        request.initial_target.clone(),
    )?;
    ctx.event(
        "resource_drift_probe_started",
        json!({
            "step_id": request.operation.id,
            "required_mismatch_frames": DEFAULT_RESOURCE_DRIFT_FRAMES,
            "timeout_ms": DEFAULT_ROI_STABILITY_TIMEOUT_MS,
            "guard": guard.to_json(),
            "initial_page": request.initial_page.as_deref(),
            "initial_target": target_evaluation_json(&request.initial_target)
        }),
    )?;

    let started = Instant::now();
    while started.elapsed() <= Duration::from_millis(DEFAULT_ROI_STABILITY_TIMEOUT_MS) {
        ctx.wait_for_next_capture_start();
        let scene = ctx.capture_scene_with_pages(
            capture,
            &request.resources.evaluator,
            &request.resources.detector,
            &format!("resource_drift_{}", request.operation.id),
            request.candidate_pages,
        )?;
        let current_page = scene.matched_anchor(request.game);
        if !matched_page_matches_anchor(request.game, scene.matched_page.as_deref(), &guard.page_id)
        {
            return Ok(ResourceDriftOutcome::Failed {
                reason: "page_guard_mismatch",
                current_page,
                diagnostics: json!({
                    "expected_page": guard.page_id,
                    "matched_page": scene.matched_page,
                    "operation_from": request.operation.from
                }),
            });
        }
        let target = request
            .resources
            .evaluator
            .evaluate_target(&scene.scene, &guard.target_id)
            .map_err(|err| CliError::device(err.to_string()))?;
        match gate.observe(target.clone()) {
            ResourceDriftObservation::Recovered => {
                return Ok(ResourceDriftOutcome::Recovered {
                    current_page,
                    target,
                });
            }
            ResourceDriftObservation::Drift => {
                return Ok(ResourceDriftOutcome::Failed {
                    reason: "resource_drift",
                    current_page,
                    diagnostics: resource_drift_diagnostics(
                        request.operation,
                        guard,
                        &target,
                        gate.observed_frames,
                    ),
                });
            }
            ResourceDriftObservation::Waiting => {}
        }
    }

    Ok(ResourceDriftOutcome::Failed {
        reason: "unstable_page",
        current_page: request.initial_page,
        diagnostics: json!({
            "guard": guard.to_json(),
            "required_mismatch_frames": DEFAULT_RESOURCE_DRIFT_FRAMES,
            "observed_frames": gate.observed_frames,
            "last_target": target_evaluation_json(&gate.last_target),
            "timeout_ms": DEFAULT_ROI_STABILITY_TIMEOUT_MS
        }),
    })
}

#[derive(Debug, PartialEq)]
enum ResourceDriftObservation {
    Recovered,
    Drift,
    Waiting,
}

#[derive(Debug)]
struct ResourceDriftGate {
    required_mismatch_frames: u32,
    stable_mismatch_frames: u32,
    observed_frames: u32,
    last_target: TargetEvaluation,
}

impl ResourceDriftGate {
    fn new(required_mismatch_frames: u32, initial_mismatch: TargetEvaluation) -> CliOutcome<Self> {
        if required_mismatch_frames == 0 {
            return Err(CliError::device(
                "resource drift probe requires at least one mismatch frame",
            ));
        }
        if initial_mismatch.passed {
            return Err(CliError::device(
                "resource drift probe requires an initial target mismatch",
            ));
        }
        Ok(Self {
            required_mismatch_frames,
            stable_mismatch_frames: 1,
            observed_frames: 1,
            last_target: initial_mismatch,
        })
    }

    fn observe(&mut self, target: TargetEvaluation) -> ResourceDriftObservation {
        self.observed_frames += 1;
        if target.passed {
            self.stable_mismatch_frames = 0;
            self.last_target = target;
            return ResourceDriftObservation::Recovered;
        }
        if target_measurement_stable_with(&self.last_target, &target) {
            self.stable_mismatch_frames += 1;
        } else {
            self.stable_mismatch_frames = 1;
        }
        self.last_target = target;
        if self.stable_mismatch_frames >= self.required_mismatch_frames {
            ResourceDriftObservation::Drift
        } else {
            ResourceDriftObservation::Waiting
        }
    }
}

#[derive(Debug)]
struct RoiStabilityGate {
    required_stable_frames: u32,
    stable_frames: u32,
    observed_frames: u32,
    last_target: TargetEvaluation,
}

impl RoiStabilityGate {
    fn new(required_stable_frames: u32, baseline: TargetEvaluation) -> CliOutcome<Self> {
        if required_stable_frames == 0 {
            return Err(CliError::device(
                "ROI stability gate requires at least one stable frame",
            ));
        }
        if !baseline.passed {
            return Err(CliError::device(
                "ROI stability baseline target did not pass guard evaluation",
            ));
        }
        Ok(Self {
            required_stable_frames,
            stable_frames: 1,
            observed_frames: 1,
            last_target: baseline,
        })
    }

    fn observe(&mut self, target: TargetEvaluation) -> bool {
        self.observed_frames += 1;
        if !target.passed {
            self.stable_frames = 0;
            self.last_target = target;
            return false;
        }
        if target_stable_with(&self.last_target, &target) {
            self.stable_frames += 1;
        } else {
            self.stable_frames = 1;
        }
        self.last_target = target;
        self.stable_frames >= self.required_stable_frames
    }
}

fn target_stable_with(previous: &TargetEvaluation, current: &TargetEvaluation) -> bool {
    previous.passed && current.passed && target_measurement_stable_with(previous, current)
}

pub(crate) fn target_evaluations_stable_for_wait(
    previous: &TargetEvaluation,
    current: &TargetEvaluation,
) -> bool {
    target_stable_with(previous, current)
}

fn target_measurement_stable_with(previous: &TargetEvaluation, current: &TargetEvaluation) -> bool {
    if previous.id != current.id || previous.kind != current.kind {
        return false;
    }
    if !template_evaluation_stable(previous, current) {
        return false;
    }
    color_evaluation_stable(previous, current)
}

fn template_evaluation_stable(previous: &TargetEvaluation, current: &TargetEvaluation) -> bool {
    match (previous.template, current.template) {
        (Some(previous), Some(current)) => {
            (previous.x - current.x).abs() <= ROI_TEMPLATE_POSITION_EPSILON
                && (previous.y - current.y).abs() <= ROI_TEMPLATE_POSITION_EPSILON
                && (previous.score - current.score).abs() <= ROI_TEMPLATE_SCORE_EPSILON
        }
        (None, None) => true,
        _ => false,
    }
}

fn color_evaluation_stable(previous: &TargetEvaluation, current: &TargetEvaluation) -> bool {
    match (previous.color, current.color) {
        (Some(previous), Some(current)) => {
            let mean_stable = previous
                .mean
                .iter()
                .zip(current.mean.iter())
                .all(|(previous, current)| previous.abs_diff(*current) <= ROI_COLOR_MEAN_EPSILON);
            mean_stable
                && (previous.distance - current.distance).abs() <= ROI_COLOR_DISTANCE_EPSILON
        }
        (None, None) => true,
        _ => false,
    }
}

fn resource_drift_diagnostics(
    operation: &Operation,
    guard: &OperationGuard,
    target: &TargetEvaluation,
    observed_frames: u32,
) -> Value {
    json!({
        "trigger": "resource_drift",
        "resource_status": "needs_recalibration",
        "resource_action": "mark_for_recalibration",
        "target_id": guard.target_id.as_str(),
        "expected_rect": rect_json(guard.expected_rect),
        "measured": target_evaluation_json(target),
        "observed_frames": observed_frames,
        "required_mismatch_frames": DEFAULT_RESOURCE_DRIFT_FRAMES,
        "provenance_version": operation_provenance_version(operation),
        "provenance": operation.provenance.clone().unwrap_or(Value::Null),
        "guard": guard.to_json()
    })
}

fn operation_provenance_version(operation: &Operation) -> Value {
    operation
        .provenance
        .as_ref()
        .and_then(|provenance| {
            provenance
                .get("version")
                .or_else(|| provenance.get("resource_version"))
                .or_else(|| provenance.get("pack_version"))
                .or_else(|| provenance.get("source_commit"))
                .or_else(|| provenance.get("commit"))
        })
        .cloned()
        .unwrap_or(Value::Null)
}

fn target_evaluation_json(target: &TargetEvaluation) -> Value {
    json!({
        "id": target.id.as_str(),
        "kind": format!("{:?}", target.kind),
        "passed": target.passed,
        "message": target.message.as_str(),
        "matched_rect": target.template.map(|template| rect_json(PackRect {
            x: template.x,
            y: template.y,
            width: template.width,
            height: template.height
        })),
        "template": target.template.map(|template| json!({
            "x": template.x,
            "y": template.y,
            "width": template.width,
            "height": template.height,
            "raw_score": template.raw_score,
            "score": template.score,
            "threshold": template.threshold
        })),
        "color": target.color.map(|color| json!({
            "distance": color.distance,
            "max_distance": color.max_distance,
            "mean": color.mean,
            "expected": color.expected
        }))
    })
}

fn unsupported_targets_json(targets: &[UnsupportedRecognitionTarget]) -> Vec<Value> {
    targets
        .iter()
        .map(|target| {
            json!({
                "id": target.id.as_str(),
                "reason": target.reason.as_str()
            })
        })
        .collect()
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
        .expected_after_page()
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
    backend: &mut Option<SelectedTouchBackend>,
    err: CliError,
) -> CliOutcome<T> {
    if let Some(mut backend) = backend.take() {
        let close = backend.close();
        if let Err(close_err) = close {
            return Err(CliError::device(format!(
                "{}; touch backend close also failed: {}",
                err.message, close_err
            )));
        }
    }
    Err(err)
}

fn capture_backend_attempt_json(attempt: &CaptureBackendAttempt) -> Value {
    json!({
        "backend": attempt.backend.as_str(),
        "ok": attempt.ok,
        "severity": if attempt.ok { "info" } else { "warning" },
        "elapsed_ms": attempt.elapsed_ms,
        "cached": attempt.cached,
        "channel_order_contract": attempt.channel_order_contract,
        "message": attempt.message.as_str(),
        "vendor_stdio": &attempt.vendor_stdio
    })
}

fn ensure_touch_backend<'a>(
    backend: &'a mut Option<SelectedTouchBackend>,
    target: &DeviceTarget,
    adb: &actingcommand_device::AdbConfig,
    requested: actingcommand_device::TouchBackendChoice,
) -> CliOutcome<&'a mut SelectedTouchBackend> {
    if backend.is_none() {
        let created = create_touch_backend(
            TouchBackendConfig::new(
                adb.clone(),
                target.clone(),
                actingcommand_device::MaaTouchConfig::default(),
            )
            .with_requested(requested),
        )
        .map_err(|err| CliError::device(err.to_string()))?;
        *backend = Some(created);
    }
    backend
        .as_mut()
        .ok_or_else(|| CliError::device("failed to initialize touch backend"))
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
                resources,
                template,
                resources.operation_bundle.defaults.template_threshold,
            )?;
        }
        let matched_to = operation.expected_after_page().is_some_and(|page| {
            matched_page_matches_anchor(game, scene.matched_page.as_deref(), page)
        });
        let unverified_single_frame =
            operation.expected_after_page().is_none() && operation.verify_template.is_none();
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
    resources: &LabResources,
    template: &str,
    threshold: f32,
) -> CliOutcome<bool> {
    let bytes = resources.operation_asset(template)?;
    let matched = scene
        .match_template(bytes, None)
        .map_err(|err| CliError::device(err.to_string()))?;
    Ok(matched.score >= threshold)
}

fn lab_control_from_bundle(bundle: &LoadedBundle) -> CliOutcome<LabControl> {
    let Some(control) = bundle.control() else {
        return Err(CliError::package_invalid(
            "Lab package must include control.json",
        ));
    };
    serde_json::from_value(control.clone())
        .map_err(|err| CliError::package_invalid(format!("failed to parse control.json: {err}")))
}

fn load_lab_resources_from_bundle(
    bundle: LoadedBundle,
    control: &LabControl,
) -> CliOutcome<LabResources> {
    let resource_root = PathBuf::from(bundle.resource_root());
    let manifest_path = PathBuf::from(bundle.manifest_path());
    let manifest = bundle.manifest().clone();
    validate_manifest_entry_task_id(&manifest_path, &manifest, control)?;
    let operation_path = PathBuf::from(bundle.operation_path());
    let operation_bundle: OperationBundle = serde_json::from_value(bundle.operation().clone())
        .map_err(|err| {
            CliError::package_invalid(format!(
                "failed to parse {}: {err}",
                bundle.operation_path()
            ))
        })?;
    operation_bundle.validate(control, |relative| {
        bundle
            .resource_entry(&format!(
                "operations/{}/{}",
                control.entry_task_id, relative
            ))
            .map(|_| true)
            .or_else(|err| match err {
                ContainmentError::MissingEntry { .. } => Ok(false),
                other => Err(containment_error(other)),
            })
    })?;
    let pack_path = bundle
        .recognition_pack_path()
        .map(PathBuf::from)
        .ok_or_else(|| CliError::package_invalid("missing recognition pack for Lab package"))?;
    let pages_path = bundle
        .pages_path()
        .map(PathBuf::from)
        .ok_or_else(|| CliError::package_invalid("missing page set for Lab package"))?;
    let evaluator = bundle.evaluator().cloned().ok_or_else(|| {
        CliError::package_invalid("missing recognition evaluator for Lab package")
    })?;
    let detector = bundle
        .detector()
        .cloned()
        .ok_or_else(|| CliError::package_invalid("missing page detector for Lab package"))?;
    let navigation_path = bundle.navigation_path().map(PathBuf::from);
    let navigation = bundle.navigation().cloned();

    Ok(LabResources {
        bundle,
        resource_root,
        manifest_path,
        manifest,
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
    allow_placeholder_coords: Option<bool>,
    #[serde(default)]
    output: Option<Value>,
    #[serde(default)]
    capture_backend: Option<String>,
    #[serde(default)]
    frame_store: FrameStoreControl,
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
        self.frame_store
            .validate()
            .map_err(CliError::package_invalid)?;
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
    bundle: LoadedBundle,
    resource_root: PathBuf,
    manifest_path: PathBuf,
    manifest: Value,
    operation_path: PathBuf,
    operation_bundle: OperationBundle,
    pack_path: PathBuf,
    pages_path: PathBuf,
    evaluator: RecognitionEvaluator,
    detector: PageDetector,
    navigation_path: Option<PathBuf>,
    navigation: Option<Value>,
}

impl LabResources {
    fn operation_asset(&self, relative: &str) -> CliOutcome<&[u8]> {
        self.bundle
            .resource_entry(&format!(
                "operations/{}/{}",
                self.operation_bundle.task_id, relative
            ))
            .map_err(containment_error)
    }
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
    fn validate(
        &self,
        control: &LabControl,
        mut operation_asset_exists: impl FnMut(&str) -> CliOutcome<bool>,
    ) -> CliOutcome<()> {
        if !matches!(self.schema_version.as_str(), "0.3" | "0.4" | "0.5") {
            return Err(CliError::package_invalid(format!(
                "unsupported operation schema_version '{}', expected one of 0.3, 0.4, 0.5",
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
            if !operation_asset_exists(&anchor.template)? {
                return Err(CliError::package_invalid(format!(
                    "operation anchor '{}' references missing template {}",
                    anchor.id, anchor.template
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
            if let Some(template) = &operation.verify_template
                && !operation_asset_exists(template)?
            {
                return Err(CliError::package_invalid(format!(
                    "operation '{}' references missing verify_template {}",
                    operation.id, template
                )));
            }
            if let Some(guard_template) = operation
                .guard
                .as_ref()
                .and_then(|guard| guard.verify_template.as_ref())
                && !operation_asset_exists(guard_template)?
            {
                return Err(CliError::package_invalid(format!(
                    "operation '{}' guard references missing verify_template {}",
                    operation.id, guard_template
                )));
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
    expect_after: Option<OperationExpectation>,
    #[serde(default)]
    guard: Option<OperationGuard>,
    #[serde(default)]
    unguarded_trusted_coordinate: bool,
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
        self.click.validate(control)?;
        if self.click.kind == "offset" {
            let guard = self.guard.as_ref().ok_or_else(|| {
                CliError::package_invalid(format!(
                    "operation '{}' offset click requires guard metadata",
                    self.id
                ))
            })?;
            if let Some(target_id) = self.click.target_id.as_deref()
                && target_id != guard.target_id
            {
                return Err(CliError::package_invalid(format!(
                    "operation '{}' offset click target_id '{}' does not match guard target_id '{}'",
                    self.id, target_id, guard.target_id
                )));
            }
            if guard.verify_template.is_none() {
                return Err(CliError::package_invalid(format!(
                    "operation '{}' offset click requires template guard metadata; color-probe guards cannot produce a matched_rect",
                    self.id
                )));
            }
        } else if self.click.is_absolute_coordinate()
            && let Some(guard) = &self.guard
            && guard.verify_template.is_none()
        {
            return Err(CliError::package_invalid(format!(
                "operation '{}' coordinate action requires template guard metadata; color-probe guards cannot produce a matched_rect",
                self.id
            )));
        }
        if let Some(expect_after) = &self.expect_after {
            expect_after.validate(&self.id)?;
        }
        self.validate_guard(control)
    }

    fn input_action(
        &self,
        resolution: &Resolution,
        seed_base: u64,
        target: Option<&TargetEvaluation>,
    ) -> CliOutcome<LabInputAction> {
        self.click.input_action(
            resolution,
            seed_base ^ hash_text(&self.id),
            self.guard.as_ref(),
            target,
        )
    }

    fn validate_guard(&self, control: &LabControl) -> CliOutcome<()> {
        match (&self.guard, self.unguarded_trusted_coordinate) {
            (Some(_), true) => Err(CliError::package_invalid(format!(
                "operation '{}' cannot set both guard and unguarded_trusted_coordinate",
                self.id
            ))),
            (None, true) => Ok(()),
            (None, false) => Err(CliError::package_invalid(format!(
                "operation '{}' coordinate action missing guard metadata; add guard or set unguarded_trusted_coordinate for reviewed trusted coordinates",
                self.id
            ))),
            (Some(guard), false) => guard.validate(&self.id, &self.from, control),
        }
    }

    fn expected_after_page(&self) -> Option<&str> {
        self.expect_after
            .as_ref()
            .map(|expectation| expectation.page_id.as_str())
            .or(self.to.as_deref())
    }

    fn after_timeout_ms(&self, default_timeout_ms: u64) -> u64 {
        self.expect_after
            .as_ref()
            .and_then(|expectation| expectation.timeout_ms)
            .unwrap_or(default_timeout_ms)
    }
}

#[derive(Debug, Clone, Deserialize)]
struct OperationExpectation {
    page_id: String,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    interval_ms: Option<u64>,
}

impl OperationExpectation {
    fn validate(&self, operation_id: &str) -> CliOutcome<()> {
        if self.page_id.trim().is_empty() {
            return Err(CliError::package_invalid(format!(
                "operation '{operation_id}' expect_after.page_id must not be empty"
            )));
        }
        if self.timeout_ms == Some(0) {
            return Err(CliError::package_invalid(format!(
                "operation '{operation_id}' expect_after.timeout_ms must be positive when provided"
            )));
        }
        if self.interval_ms == Some(0) {
            return Err(CliError::package_invalid(format!(
                "operation '{operation_id}' expect_after.interval_ms must be positive when provided"
            )));
        }
        Ok(())
    }

    fn to_json(&self) -> Value {
        json!({
            "page_id": self.page_id.as_str(),
            "timeout_ms": self.timeout_ms,
            "interval_ms": self.interval_ms
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
struct OperationGuard {
    page_id: String,
    target_id: String,
    expected_rect: PackRect,
    #[serde(default)]
    verify_template: Option<String>,
    #[serde(default)]
    color_probe: Option<String>,
}

impl OperationGuard {
    fn validate(
        &self,
        operation_id: &str,
        operation_from: &str,
        control: &LabControl,
    ) -> CliOutcome<()> {
        if self.page_id.trim().is_empty() {
            return Err(CliError::package_invalid(format!(
                "operation '{operation_id}' guard.page_id must not be empty"
            )));
        }
        if self.target_id.trim().is_empty() {
            return Err(CliError::package_invalid(format!(
                "operation '{operation_id}' guard.target_id must not be empty"
            )));
        }
        if !page_anchor_matches(&control.game, &self.page_id, operation_from) {
            return Err(CliError::package_invalid(format!(
                "operation '{operation_id}' guard.page_id '{}' does not match operation from '{}'",
                self.page_id, operation_from
            )));
        }
        validate_guard_rect(self.expected_rect, &control.resolution)?;
        let has_verify_target = self.verify_template.is_some() || self.color_probe.is_some();
        if !has_verify_target {
            return Err(CliError::package_invalid(format!(
                "operation '{operation_id}' guard requires verify_template or color_probe"
            )));
        }
        Ok(())
    }

    fn to_json(&self) -> Value {
        json!({
            "page_id": self.page_id.as_str(),
            "target_id": self.target_id.as_str(),
            "expected_rect": rect_json(self.expected_rect),
            "verify_template": self.verify_template.as_deref(),
            "color_probe": self.color_probe.as_deref()
        })
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
    #[serde(default)]
    offset: Option<PackRect>,
    #[serde(default)]
    target_id: Option<String>,
}

impl OperationClick {
    fn validate(&self, control: &LabControl) -> CliOutcome<()> {
        match self.kind.as_str() {
            "rect" | "specific_rect" => {
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
            "long_press" | "long_tap" => {
                let x = self
                    .x
                    .ok_or_else(|| CliError::package_invalid("long_press click missing x"))?;
                let y = self
                    .y
                    .ok_or_else(|| CliError::package_invalid("long_press click missing y"))?;
                validate_click_point(
                    x,
                    y,
                    &control.resolution,
                    control.allow_placeholder_coords.unwrap_or(false),
                )?;
                if self.duration_ms.unwrap_or(0) == 0 {
                    return Err(CliError::package_invalid(
                        "long_press duration_ms must be positive",
                    ));
                }
                Ok(())
            }
            "offset" => {
                let offset = self
                    .offset
                    .ok_or_else(|| CliError::package_invalid("offset click missing offset rect"))?;
                if offset.width <= 0 || offset.height <= 0 {
                    return Err(CliError::package_invalid(format!(
                        "offset click dimensions must be positive: {}x{}",
                        offset.width, offset.height
                    )));
                }
                Ok(())
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

    fn input_action(
        &self,
        resolution: &Resolution,
        seed: u64,
        guard: Option<&OperationGuard>,
        target: Option<&TargetEvaluation>,
    ) -> CliOutcome<LabInputAction> {
        match self.kind.as_str() {
            "rect" | "specific_rect" => {
                let rect = derive_absolute_coordinate_rect(
                    self.kind.as_str(),
                    self.required_rect()?,
                    guard,
                    target,
                )?;
                validate_click_rect(rect, resolution, false)?;
                Ok(LabInputAction::Tap(actual_click_point(rect, seed)))
            }
            "point" => {
                let rect = derive_absolute_coordinate_rect(
                    "point",
                    self.required_point_rect("point")?,
                    guard,
                    target,
                )?;
                validate_click_rect(rect, resolution, false)?;
                Ok(LabInputAction::Tap(actual_explicit_point(rect, seed)))
            }
            "long_press" | "long_tap" => {
                let rect = derive_absolute_coordinate_rect(
                    "long_press",
                    self.required_point_rect("long_press")?,
                    guard,
                    target,
                )?;
                validate_click_rect(rect, resolution, false)?;
                Ok(LabInputAction::LongTap {
                    point: actual_explicit_point(rect, seed),
                    duration_ms: self.duration_ms.unwrap_or(600),
                })
            }
            "offset" => {
                let guard = guard.ok_or_else(|| {
                    CliError::package_invalid("offset click requires guard metadata")
                })?;
                let target = target.ok_or_else(|| {
                    CliError::package_invalid("offset click requires matched template target")
                })?;
                if target.id != guard.target_id {
                    return Err(CliError::package_invalid(format!(
                        "offset click matched target '{}' does not match guard target_id '{}'",
                        target.id, guard.target_id
                    )));
                }
                let matched_rect = matched_template_rect(target)?;
                let offset = self
                    .offset
                    .ok_or_else(|| CliError::package_invalid("offset click missing offset rect"))?;
                let rect = PackRect {
                    x: matched_rect.x + offset.x,
                    y: matched_rect.y + offset.y,
                    width: offset.width,
                    height: offset.height,
                };
                validate_click_rect(rect, resolution, false)?;
                Ok(LabInputAction::Tap(actual_click_point(rect, seed)))
            }
            "drag" => {
                let declared_from = self
                    .from_rect
                    .ok_or_else(|| CliError::package_invalid("drag click missing from rect"))?;
                let to = self
                    .to_rect
                    .ok_or_else(|| CliError::package_invalid("drag click missing to rect"))?;
                let from = derive_absolute_coordinate_rect("drag", declared_from, guard, target)?;
                let to = derive_absolute_coordinate_rect("drag", to, guard, target)?;
                validate_click_rect(from, resolution, false)?;
                validate_click_rect(to, resolution, false)?;
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

    fn required_point_rect(&self, kind: &str) -> CliOutcome<PackRect> {
        Ok(PackRect {
            x: self
                .x
                .ok_or_else(|| CliError::package_invalid(format!("{kind} click missing x")))?,
            y: self
                .y
                .ok_or_else(|| CliError::package_invalid(format!("{kind} click missing y")))?,
            width: 1,
            height: 1,
        })
    }

    fn is_absolute_coordinate(&self) -> bool {
        matches!(
            self.kind.as_str(),
            "rect" | "specific_rect" | "point" | "long_press" | "long_tap" | "drag"
        )
    }
}

fn matched_template_rect(target: &TargetEvaluation) -> CliOutcome<PackRect> {
    if target.kind != TargetKind::Template {
        return Err(CliError::package_invalid(format!(
            "template-target matched_rect required, got {:?}",
            target.kind
        )));
    }
    let template = target
        .template
        .ok_or_else(|| CliError::package_invalid("template target missing matched_rect"))?;
    if !target.passed {
        return Err(CliError::package_invalid(format!(
            "template target '{}' did not pass guard evaluation",
            target.id
        )));
    }
    let rect = PackRect {
        x: template.x,
        y: template.y,
        width: template.width,
        height: template.height,
    };
    if rect.width <= 0 || rect.height <= 0 {
        return Err(CliError::package_invalid(format!(
            "matched_rect dimensions must be positive: {}x{}",
            rect.width, rect.height
        )));
    }
    Ok(rect)
}

fn derive_absolute_coordinate_rect(
    kind: &str,
    declared: PackRect,
    guard: Option<&OperationGuard>,
    target: Option<&TargetEvaluation>,
) -> CliOutcome<PackRect> {
    let Some(guard) = guard else {
        return Ok(declared);
    };
    if guard.verify_template.is_none() {
        return Err(CliError::package_invalid(format!(
            "{kind} coordinate action requires template guard metadata"
        )));
    }
    let target =
        target.ok_or_else(|| CliError::package_invalid(format!("{kind} requires guard target")))?;
    if target.id != guard.target_id {
        return Err(CliError::package_invalid(format!(
            "{kind} matched target '{}' does not match guard target_id '{}'",
            target.id, guard.target_id
        )));
    }
    let matched_rect = matched_template_rect(target)?;
    super::derive_absolute_coordinate_rect_from_match(
        kind,
        declared,
        guard.expected_rect,
        matched_rect,
    )
}

#[derive(Debug, Clone, Copy)]
enum LabInputAction {
    Tap(ActualClickPoint),
    LongTap {
        point: ActualClickPoint,
        duration_ms: u64,
    },
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
            LabInputAction::LongTap { point, duration_ms } => {
                json!({"kind": "long_tap", "actual_click_point": point.to_json(), "duration_ms": duration_ms})
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

fn actual_explicit_point(rect: PackRect, seed: u64) -> ActualClickPoint {
    ActualClickPoint {
        seed,
        algorithm: "explicit_point_v1",
        rect,
        x: rect.x,
        y: rect.y,
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

fn validate_guard_rect(rect: PackRect, resolution: &Resolution) -> CliOutcome<()> {
    if rect.width <= 0 || rect.height <= 0 {
        return Err(CliError::package_invalid(format!(
            "guard expected_rect dimensions must be positive: {}x{}",
            rect.width, rect.height
        )));
    }
    validate_rect_point(rect.x, rect.y, resolution, "guard expected_rect")?;
    validate_rect_point(
        rect.x + rect.width - 1,
        rect.y + rect.height - 1,
        resolution,
        "guard expected_rect",
    )
}

fn validate_rect_point(x: i32, y: i32, resolution: &Resolution, label: &str) -> CliOutcome<()> {
    if x < 0 || y < 0 || x >= resolution.width as i32 || y >= resolution.height as i32 {
        return Err(CliError::package_invalid(format!(
            "{label} point {x},{y} is outside {}x{}",
            resolution.width, resolution.height
        )));
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
    id_issuer: IdIssuer,
    run_id: String,
    run_seed: u64,
    started_at: SystemTime,
    started_instant: Instant,
    run_root: PathBuf,
    run_dir: PathBuf,
    output_dir: PathBuf,
    logs_dir: PathBuf,
    screenshots_dir: PathBuf,
    ledger: Option<LabLedger>,
    ledger_path: Option<PathBuf>,
    ledger_dispatch_written: bool,
    input_zip_sha256: Option<String>,
    input_entries: Vec<String>,
    requested_capture_interval_ms: u64,
    screenshot_names: HashMap<String, usize>,
    screenshots: Vec<ScreenshotRecord>,
    screenshot_evidence: Vec<Value>,
    frame_store: FrameStore,
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
    partial_output: bool,
    current_step_index: Option<usize>,
    current_step_id: Option<String>,
    current_operation_id: Option<String>,
    expected_page: Option<String>,
}

impl LabRunContext {
    fn create(run_root: &Path, input_zip: &Path) -> CliOutcome<Self> {
        let now = SystemTime::now();
        let issuer = IdIssuer::new();
        let run_id = issuer.issue(IdKind::Run).value;
        let run_dir = run_root.join(&run_id);
        let output_dir = run_dir.join("output");
        let logs_dir = output_dir.join("logs");
        let screenshots_dir = output_dir.join("screenshots");
        fs::create_dir_all(&logs_dir).map_err(|err| {
            CliError::package_invalid(format!("failed to create {}: {err}", logs_dir.display()))
        })?;
        fs::create_dir_all(&screenshots_dir).map_err(|err| {
            CliError::package_invalid(format!(
                "failed to create {}: {err}",
                screenshots_dir.display()
            ))
        })?;
        let frame_store = FrameStore::new(
            run_dir.join("frame-store-temp"),
            FrameStoreConfig::default(),
        )?;
        Ok(Self {
            id_issuer: issuer,
            run_id,
            run_seed: hash_text(&input_zip.display().to_string()),
            started_at: now,
            started_instant: Instant::now(),
            run_root: run_root.to_path_buf(),
            run_dir,
            output_dir,
            logs_dir,
            screenshots_dir,
            ledger: None,
            ledger_path: None,
            ledger_dispatch_written: false,
            input_zip_sha256: None,
            input_entries: Vec::new(),
            requested_capture_interval_ms: DEFAULT_CAPTURE_INTERVAL_MS,
            screenshot_names: HashMap::new(),
            screenshots: Vec::new(),
            screenshot_evidence: Vec::new(),
            frame_store,
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
            partial_output: false,
            current_step_index: None,
            current_step_id: None,
            current_operation_id: None,
            expected_page: None,
        })
    }

    fn set_phase(&mut self, phase: &str) {
        self.phase = phase.to_string();
    }

    fn set_step_context(&mut self, step_index: usize, operation: &Operation) {
        self.current_step_index = Some(step_index);
        self.current_step_id = Some(operation.id.clone());
        self.current_operation_id = Some(operation.id.clone());
        self.expected_page = operation.to.clone();
    }

    fn clear_step_context(&mut self) {
        self.current_step_index = None;
        self.current_step_id = None;
        self.current_operation_id = None;
        self.expected_page = None;
    }

    fn record_capture_backend_selection(&mut self) -> CliOutcome<()> {
        let attempts = self
            .capture_backend_attempts
            .iter()
            .map(capture_backend_attempt_json)
            .collect::<Vec<_>>();
        self.append_ledger_record(
            self.ledger_record(
                LedgerRecordKind::Drive,
                json!({
                    "record_type": "capture_backend_selection",
                    "phase": self.phase,
                    "requested": self.capture_backend_requested.map(|backend| backend.as_str()),
                    "used": self.capture_backend_used.map(|backend| backend.as_str()),
                    "attempt_count": attempts.len(),
                    "attempts": attempts
                }),
            ),
            "ledger_capture_backend_selection",
        )
    }

    fn ensure_ledger(&mut self) -> CliOutcome<()> {
        if self.ledger.is_some() {
            return Ok(());
        }
        let instance = self.instance.as_deref().unwrap_or("unknown").to_string();
        let control = self.control.as_ref();
        let header = SessionHeader::new(
            "runtime-embedded-lab1y",
            control
                .map(|control| control.game.as_str())
                .unwrap_or("unknown"),
            control
                .map(|control| control.server.as_str())
                .unwrap_or("unknown"),
            &instance,
        );
        let mut ledger =
            LabLedger::create_runtime_shard(&self.run_root, &self.run_id, &instance, header)
                .map_err(|err| self.ledger_failure(err, "ledger_create"))?;
        let path = ledger.ledger_path().to_path_buf();
        let backlog = self.events.clone();
        for event in backlog {
            ledger
                .append_event(self.light_event_from_legacy_event(&event)?)
                .map_err(|err| self.ledger_failure(err, "ledger_backfill_event"))?;
        }
        self.ledger_path = Some(path);
        self.ledger = Some(ledger);
        self.write_dispatch_record()
    }

    fn write_dispatch_record(&mut self) -> CliOutcome<()> {
        if self.ledger_dispatch_written {
            return Ok(());
        }
        let record = self.ledger_record(
            LedgerRecordKind::Dispatch,
            json!({
                "record_type": "lab_run_dispatch",
                "command": "lab run",
                "phase": self.phase,
                "input_summary": self.input_summary()
            }),
        );
        self.append_ledger_record(record, "ledger_dispatch")?;
        self.ledger_dispatch_written = true;
        Ok(())
    }

    fn append_ledger_event(&mut self, event: &Value) -> CliOutcome<()> {
        let light_event = self.light_event_from_legacy_event(event)?;
        let Some(ledger) = self.ledger.as_mut() else {
            return Ok(());
        };
        ledger
            .append_event(light_event)
            .map_err(|err| self.ledger_failure(err, "ledger_event"))
    }

    fn append_ledger_record(
        &mut self,
        record: LedgerRecord,
        failure_phase: &str,
    ) -> CliOutcome<()> {
        let Some(ledger) = self.ledger.as_mut() else {
            return Err(self.ledger_failure(
                LabLogError::InvalidInput("runtime ledger handle is unavailable".to_string()),
                failure_phase,
            ));
        };
        ledger
            .append(record)
            .map_err(|err| self.ledger_failure(err, failure_phase))
    }

    fn ledger_record(&self, kind: LedgerRecordKind, payload: Value) -> LedgerRecord {
        let mut record = LedgerRecord::new(kind, None, payload);
        for (key, value) in self.id_chain() {
            record = record.with_id(key, value);
        }
        record
    }

    fn id_chain(&self) -> BTreeMap<String, String> {
        let mut ids = BTreeMap::from([("run_id".to_string(), self.run_id.clone())]);
        if let Some(instance) = &self.instance {
            ids.insert("instance_id".to_string(), instance.clone());
        }
        if let Some(control) = &self.control {
            ids.insert("task_id".to_string(), control.entry_task_id.clone());
        }
        ids
    }

    fn input_summary(&self) -> Value {
        json!({
            "input_zip_sha256": self.input_zip_sha256,
            "entry_count": self.input_entries.len(),
            "run_id": self.run_id,
            "instance": self.instance,
            "phase": self.phase
        })
    }

    fn light_event_from_legacy_event(&self, event: &Value) -> CliOutcome<LightEvent> {
        let event_name = event
            .get("event")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        LightEvent::new(
            format!("lab.{event_name}.event"),
            self.id_chain(),
            event.clone(),
        )
        .map_err(|err| self.ledger_failure(err, "ledger_event_shape"))
    }

    fn ledger_failure(&self, err: LabLogError, phase: &str) -> CliError {
        let attempted_ledger_path = self
            .ledger_path
            .as_ref()
            .map(|path| path.display().to_string());
        let message = err.to_string();
        let last_resort = LastResortError::new(
            "lab run",
            phase,
            "runtime_ledger_failed",
            &message,
            self.input_summary(),
            attempted_ledger_path,
        );
        let last_resort_result = write_last_resort_error(Some(&self.run_root), &last_resort);
        let suffix = match last_resort_result {
            Ok(path) => format!("; last-resort error file written to {}", path.display()),
            Err(last_resort_err) => {
                format!("; additionally failed to write last-resort error file: {last_resort_err}")
            }
        };
        CliError::package_invalid(format!(
            "runtime ledger failure during {phase}: {message}{suffix}"
        ))
    }

    fn set_frame_store_config(&mut self, config: FrameStoreConfig) -> CliOutcome<()> {
        self.frame_store.set_config(config)
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
        let event = Value::Object(object);
        self.append_ledger_event(&event)?;
        self.events.push(event);
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
        let width = frame.width;
        let height = frame.height;
        let backend = frame.backend_name.as_str();
        let pixel_format = frame.pixel_format.as_str();
        let captured_at = frame.captured_at;
        let scene = scene_from_frame(&frame)?;
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
        let mut store_outcome = self.frame_store.add_frame(FrameStoreFrameInput {
            frame_index: self.frame_index,
            file_name,
            label: label.to_string(),
            recognition_state: RecognitionState::from_matched_page(matched_page.clone()),
            frame,
        })?;
        if let Some(checkpoint) = store_outcome.checkpoint.as_mut() {
            self.fill_pause_checkpoint(checkpoint, matched_page.as_deref());
        }
        let retained_file = store_outcome.file.clone();
        let merged_into = store_outcome.merged_into.clone();
        let pause_checkpoint = store_outcome
            .checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.to_json());
        self.event(
            "screenshot_recorded",
            json!({
                "frame_index": self.frame_index,
                "file": retained_file.clone(),
                "retained": store_outcome.retained,
                "merged_into": merged_into.clone(),
                "storage_state": store_outcome.storage_state.as_str(),
                "tier1_active": store_outcome.tier1_active,
                "tier2_active": store_outcome.tier2_active,
                "tier3_triggered": store_outcome.tier3_triggered,
                "backpressure_state": store_outcome.backpressure_state.as_str(),
                "pause_required": store_outcome.pause_required,
                "warnings": store_outcome.warnings.clone(),
                "pause_checkpoint": pause_checkpoint,
                "width": width,
                "height": height,
                "backend": backend,
                "pixel_format": pixel_format,
                "captured_at": timestamp_iso(captured_at),
                "label": label
            }),
        )?;
        let reco_id = self.id_issuer.issue(IdKind::Reco).value;
        let recognition = json!({
            "reco_id": reco_id,
            "timestamp": timestamp_iso(SystemTime::now()),
            "frame_index": self.frame_index,
            "file": retained_file,
            "retained": store_outcome.retained,
            "merged_into": merged_into,
            "storage_state": store_outcome.storage_state.as_str(),
            "backpressure_state": store_outcome.backpressure_state.as_str(),
            "matched_page": matched_page.clone(),
            "candidates": evaluations.iter().map(page_evaluation_json).collect::<Vec<_>>(),
            "diagnostics": {"label": label}
        });
        self.append_ledger_record(
            self.ledger_record(
                LedgerRecordKind::Drive,
                json!({
                    "record_type": "recognition",
                    "phase": self.phase,
                    "recognition": recognition
                }),
            )
            .with_id("reco_id", reco_id.clone()),
            "ledger_recognition",
        )?;
        self.recognition.push(recognition);
        self.event(
            "recognition_recorded",
            json!({"frame_index": self.frame_index, "reco_id": reco_id, "matched_page": matched_page}),
        )?;
        if store_outcome.tier3_triggered && !store_outcome.pause_required {
            return self.tier3_resume_check(capture, evaluator, detector, candidate_pages);
        }
        if store_outcome.pause_required {
            self.partial_output = true;
            self.event(
                "backpressure_paused",
                json!({
                    "reason": "tier3",
                    "checkpoint": store_outcome.checkpoint.map(|checkpoint| checkpoint.to_json()),
                    "current_phase": self.phase,
                    "last_frame_index": self.frame_index,
                    "last_matched_page": matched_page,
                    "tier3_mode": "synchronous_graceful_failure",
                    "partial_output": true
                }),
            )?;
            return Err(CliError::device(
                "Lab-1z frame store tier3 pause timed out or could not recover; partial output will be written",
            ));
        }
        Ok(CapturedScene {
            scene,
            matched_page,
            verify_template_matched: false,
            width,
            height,
        })
    }

    fn fill_pause_checkpoint(
        &self,
        checkpoint: &mut Tier3PauseCheckpoint,
        matched_page: Option<&str>,
    ) {
        checkpoint.current_step_index = self.current_step_index;
        checkpoint.current_step_id = self.current_step_id.clone();
        checkpoint.current_operation_id = self.current_operation_id.clone();
        checkpoint.current_phase = Some(self.phase.clone());
        checkpoint.expected_page = self.expected_page.clone();
        checkpoint.last_matched_page = matched_page.map(str::to_string);
    }

    fn tier3_resume_check(
        &mut self,
        capture: &mut dyn CaptureBackend,
        evaluator: &RecognitionEvaluator,
        detector: &PageDetector,
        candidate_pages: Option<&[String]>,
    ) -> CliOutcome<CapturedScene> {
        self.event(
            "tier3_resume_capture",
            json!({"reason": "resident_bytes_below_release_line"}),
        )?;
        let started = Instant::now();
        let frame = capture
            .capture()
            .map_err(|err| CliError::device(err.to_string()))?;
        self.capture_durations_ms
            .push(started.elapsed().as_millis() as u64);
        let width = frame.width;
        let height = frame.height;
        let scene = scene_from_frame(&frame)?;
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
        let allowed = match (&matched_page, candidate_pages) {
            (Some(page), Some(pages)) => pages.iter().any(|candidate| candidate == page),
            (Some(_), None) => true,
            (None, _) => false,
        };
        self.event(
            "tier3_resume_page_check",
            json!({"matched_page": matched_page, "allowed": allowed}),
        )?;
        if !allowed {
            self.event(
                "tier3_resume_blocked",
                json!({"matched_page": matched_page, "reason": "resume page check failed"}),
            )?;
            return Err(CliError::device(
                "Lab-1z tier3 resume blocked; manual review required",
            ));
        }
        self.event(
            "tier3_resume_allowed",
            json!({"matched_page": matched_page}),
        )?;
        Ok(CapturedScene {
            scene,
            matched_page,
            verify_template_matched: false,
            width,
            height,
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
        self.ensure_ledger()?;
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
        self.frame_store.materialize(&self.screenshots_dir)?;
        self.screenshots = self.frame_store.screenshots();
        self.event(
            "frame_store_materialized",
            json!({"screenshot_count": self.screenshots.len()}),
        )?;
        self.index_screenshot_evidence()?;
        for warning in self.frame_store.cleanup_temp() {
            self.event(
                "frame_store_temp_cleanup_warning",
                json!({"severity": "warning", "message": warning}),
            )?;
        }
        let summary = self.summary_json(ok, failure_reason, state);
        let diagnostics = self.diagnostics_json(failure_reason, state);
        let environment = self.environment_json(state);
        self.append_finalizing_record(ok, failure_reason, summary, diagnostics, environment)?;
        let committed = match commit_then_record(|| {
            self.write_logs(ok, failure_reason, state)?;
            write_output_zip(&self.output_dir, out_path)?;
            let sha256 = file_sha256(out_path)?;
            Ok(ArchiveResult {
                path: out_path.to_path_buf(),
                sha256,
            })
        }) {
            Ok(proof) => proof,
            Err(err) => return self.record_terminal_output_failure(out_path, err),
        };
        self.event(
            "output_zip_written",
            json!({"out": out_path, "sha256": committed.value().sha256.clone()}),
        )?;
        self.append_terminal_receipt(ok, failure_reason, state, Some(&committed))?;
        let archive = committed.into_inner();
        if ok {
            self.cleanup_run_dir();
        }
        Ok(archive)
    }

    fn index_screenshot_evidence(&mut self) -> CliOutcome<()> {
        self.screenshot_evidence.clear();
        if self.screenshots.is_empty() {
            return Ok(());
        }
        let store = EvidenceStore::new(&self.run_root, true);
        let mut degraded = Vec::new();
        for screenshot in &self.screenshots {
            let evidence_id = self.id_issuer.issue(IdKind::Evidence).value;
            let path = self.output_dir.join(&screenshot.file);
            let mut record = json!({
                "frame_index": screenshot.frame_index,
                "file": screenshot.file,
                "evidence_id": evidence_id,
                "status": "indexed",
                "refs": []
            });
            match fs::read(&path) {
                Ok(bytes) => match store.put(&evidence_id, "screenshot", &bytes) {
                    Ok(Some(reference)) => {
                        record["refs"] = json!([reference]);
                    }
                    Ok(None) => {
                        record["status"] = json!("degraded");
                        record["warnings"] =
                            json!(["evidence store debug mode disabled during lab run"]);
                    }
                    Err(err) => {
                        record["status"] = json!("degraded");
                        record["warnings"] = json!([format!(
                            "failed to store screenshot evidence {}: {err}",
                            path.display()
                        )]);
                    }
                },
                Err(err) => {
                    record["status"] = json!("degraded");
                    record["warnings"] = json!([format!(
                        "failed to read screenshot evidence {}: {err}",
                        path.display()
                    )]);
                }
            }
            if record.get("status").and_then(Value::as_str) == Some("degraded") {
                degraded.push(record.clone());
            }
            self.screenshot_evidence.push(record);
        }
        self.append_ledger_record(
            self.ledger_record(
                LedgerRecordKind::Drive,
                json!({
                    "record_type": "evidence_index",
                    "phase": self.phase,
                    "evidence_kind": "screenshots",
                    "screenshot_count": self.screenshots.len(),
                    "indexed_count": self.screenshot_evidence.iter().filter(|item| item.get("status").and_then(Value::as_str) == Some("indexed")).count(),
                    "degraded_count": degraded.len(),
                    "evidence": self.screenshot_evidence.clone()
                }),
            ),
            "ledger_evidence_index",
        )?;
        if !degraded.is_empty() {
            self.event(
                "evidence_index_degraded",
                json!({
                    "evidence_kind": "screenshots",
                    "degraded_count": degraded.len(),
                    "degraded": degraded
                }),
            )?;
        }
        Ok(())
    }

    fn append_finalizing_record(
        &mut self,
        ok: bool,
        failure_reason: Option<&str>,
        summary: Value,
        diagnostics: Value,
        environment: Value,
    ) -> CliOutcome<()> {
        self.append_ledger_record(
            self.ledger_record(
                LedgerRecordKind::Drive,
                json!({
                    "record_type": "finalizing",
                    "status": if ok { "ok" } else { "failed" },
                    "sealed": false,
                    "phase": self.phase,
                    "failure_reason": failure_reason,
                    "input_summary": self.input_summary(),
                    "summary": summary,
                    "diagnostics": diagnostics,
                    "environment": environment
                }),
            ),
            "ledger_finalizing",
        )
    }

    fn append_terminal_receipt(
        &mut self,
        ok: bool,
        failure_reason: Option<&str>,
        state: Option<&RunState>,
        archive: Option<&CommitProof<ArchiveResult>>,
    ) -> CliOutcome<()> {
        let output_zip = archive.map(|proof| {
            json!({
                "path": proof.value().path.display().to_string(),
                "sha256": proof.value().sha256.clone()
            })
        });
        self.append_ledger_record(
            self.ledger_record(
                LedgerRecordKind::Receipt,
                json!({
                    "record_type": if ok { "finish_ok" } else { "finish_error" },
                    "status": if ok { "ok" } else { "failed" },
                    "phase": self.phase,
                    "failure_reason": failure_reason,
                    "input_summary": self.input_summary(),
                    "summary": self.summary_json(ok, failure_reason, state),
                    "diagnostics": self.diagnostics_json(failure_reason, state),
                    "environment": self.environment_json(state),
                    "output_zip": output_zip
                }),
            ),
            "ledger_terminal_receipt",
        )
    }

    fn record_terminal_output_failure(
        &mut self,
        out_path: &Path,
        err: CliError,
    ) -> CliOutcome<ArchiveResult> {
        let message = err.message.clone();
        let failure_reason = format!("terminal output failed: {message}");
        let last_resort = LastResortError::new(
            "lab run",
            "terminal_output",
            &err.code,
            &message,
            json!({
                "run_id": self.run_id,
                "instance": self.instance,
                "phase": self.phase,
                "out": out_path.display().to_string(),
                "input_summary": self.input_summary()
            }),
            self.ledger_path
                .as_ref()
                .map(|path| path.display().to_string()),
        );
        let last_resort_result = write_last_resort_error(Some(&self.run_root), &last_resort);
        self.append_terminal_receipt(false, Some(&failure_reason), None, None)?;
        let suffix = match last_resort_result {
            Ok(path) => format!("; last-resort error file written to {}", path.display()),
            Err(last_resort_err) => {
                format!("; additionally failed to write last-resort error file: {last_resort_err}")
            }
        };
        Err(CliError::package_invalid(format!(
            "{failure_reason}{suffix}"
        )))
    }

    fn cleanup_run_dir(&self) {
        let _ = fs::remove_dir_all(&self.run_dir);
    }

    fn write_logs(
        &self,
        ok: bool,
        failure_reason: Option<&str>,
        state: Option<&RunState>,
    ) -> CliOutcome<()> {
        let projection = self.project_logs_from_ledger()?;
        write_json_lines(&self.logs_dir.join("events.jsonl"), &projection.events)?;
        write_json_lines(
            &self.logs_dir.join("recognition.jsonl"),
            &projection.recognition,
        )?;
        write_json_lines(
            &self.logs_dir.join("frame_timeline.jsonl"),
            &self.frame_store.timeline(),
        )?;
        write_json(
            &self.logs_dir.join("evidence.json"),
            &json!(projection.evidence),
        )?;
        write_json(
            &self.logs_dir.join("frame_store.json"),
            &self.frame_store.diagnostics_json(),
        )?;
        write_json(&self.logs_dir.join("summary.json"), &projection.summary)?;
        write_json(
            &self.logs_dir.join("diagnostics.json"),
            &projection.diagnostics,
        )?;
        write_json(
            &self.logs_dir.join("environment.json"),
            &projection.environment,
        )?;
        fs::write(
            self.logs_dir.join("result.md"),
            self.result_markdown(ok, failure_reason, state),
        )
        .map_err(|err| CliError::package_invalid(format!("failed to write result.md: {err}")))?;
        Ok(())
    }

    fn project_logs_from_ledger(&self) -> CliOutcome<LabLogProjection> {
        let ledger_path = self.ledger_path.as_ref().ok_or_else(|| {
            CliError::package_invalid(
                "runtime ledger path is unavailable for Lab result projection",
            )
        })?;
        if let Some(ledger) = &self.ledger {
            ledger
                .sync()
                .map_err(|err| self.ledger_failure(err, "ledger_projection_sync"))?;
        }
        let read = LabLedger::read(ledger_path)
            .map_err(|err| self.ledger_failure(err, "ledger_projection_read"))?;
        let events = project_light_events(&read);
        if events.is_empty() {
            return Err(CliError::package_invalid(
                "runtime ledger projection has no events",
            ));
        }
        let mut recognition = Vec::new();
        let mut steps = Vec::new();
        let mut evidence = Vec::new();
        let mut finalizing_summary = None;
        let mut finalizing_diagnostics = None;
        let mut finalizing_environment = None;
        for record in &read.records {
            match record.payload.get("record_type").and_then(Value::as_str) {
                Some("recognition") => {
                    if let Some(value) = record.payload.get("recognition") {
                        recognition.push(value.clone());
                    }
                }
                Some("step") => {
                    if let Some(value) = record.payload.get("step") {
                        steps.push(value.clone());
                    }
                }
                Some("evidence_index") => {
                    evidence.push(record.payload.clone());
                }
                Some("finalizing") => {
                    finalizing_summary = record.payload.get("summary").cloned();
                    finalizing_diagnostics = record.payload.get("diagnostics").cloned();
                    finalizing_environment = record.payload.get("environment").cloned();
                }
                _ => {}
            }
        }
        let projection_source = json!({
            "kind": "runtime_ledger",
            "ledger_path": ledger_path.display().to_string(),
            "event_count": read.events.len(),
            "record_count": read.records.len(),
            "skipped_corrupt_lines": read.skipped_corrupt_lines
        });
        let mut summary = finalizing_summary.ok_or_else(|| {
            CliError::package_invalid("runtime ledger projection missing finalizing summary")
        })?;
        summary["projection_source"] = projection_source.clone();
        summary["steps"] = Value::Array(steps.clone());
        let mut diagnostics = finalizing_diagnostics.ok_or_else(|| {
            CliError::package_invalid("runtime ledger projection missing finalizing diagnostics")
        })?;
        diagnostics["projection_source"] = projection_source.clone();
        diagnostics["command"] = json!("lab run");
        diagnostics["phase"] = json!(self.phase);
        diagnostics["input_summary"] = self.input_summary();
        let mut environment = finalizing_environment.ok_or_else(|| {
            CliError::package_invalid("runtime ledger projection missing finalizing environment")
        })?;
        environment["projection_source"] = projection_source;
        Ok(LabLogProjection {
            events,
            recognition,
            evidence,
            summary,
            diagnostics,
            environment,
        })
    }

    fn project_completed_run_from_ledger(&self) -> CliOutcome<LabCompletedProjection> {
        let ledger_path = self.ledger_path.as_ref().ok_or_else(|| {
            CliError::package_invalid(
                "runtime ledger path is unavailable for completed Lab run projection",
            )
        })?;
        if let Some(ledger) = &self.ledger {
            ledger
                .sync()
                .map_err(|err| self.ledger_failure(err, "ledger_completed_projection_sync"))?;
        }
        let read = LabLedger::read(ledger_path)
            .map_err(|err| self.ledger_failure(err, "ledger_completed_projection_read"))?;
        let mut saw_finalizing = false;
        let mut terminal_receipt = None;
        for record in &read.records {
            match record.payload.get("record_type").and_then(Value::as_str) {
                Some("finalizing") => saw_finalizing = true,
                Some("finish_ok") | Some("finish_error") => terminal_receipt = Some(record),
                _ => {}
            }
        }
        if !saw_finalizing {
            return Err(CliError::package_invalid(
                "runtime ledger completed projection missing finalizing record",
            ));
        }
        let receipt = terminal_receipt.ok_or_else(|| {
            CliError::package_invalid(
                "runtime ledger completed projection missing terminal receipt",
            )
        })?;
        let record_type = receipt
            .payload
            .get("record_type")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CliError::package_invalid("runtime ledger terminal receipt missing record_type")
            })?;
        let status = receipt
            .payload
            .get("status")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CliError::package_invalid("runtime ledger terminal receipt missing status")
            })?;
        let ok = match (record_type, status) {
            ("finish_ok", "ok") => true,
            ("finish_error", "failed") => false,
            _ => {
                return Err(CliError::package_invalid(format!(
                    "runtime ledger terminal receipt has inconsistent record_type/status: {record_type}/{status}"
                )));
            }
        };
        let output_zip = match receipt.payload.get("output_zip") {
            Some(Value::Object(object)) => Some(TerminalOutputZip {
                path: object
                    .get("path")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CliError::package_invalid(
                            "runtime ledger terminal receipt output_zip missing path",
                        )
                    })?
                    .to_string(),
                sha256: object
                    .get("sha256")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CliError::package_invalid(
                            "runtime ledger terminal receipt output_zip missing sha256",
                        )
                    })?
                    .to_string(),
            }),
            Some(Value::Null) | None => None,
            Some(_) => {
                return Err(CliError::package_invalid(
                    "runtime ledger terminal receipt output_zip must be an object or null",
                ));
            }
        };
        if ok && output_zip.is_none() {
            return Err(CliError::package_invalid(
                "runtime ledger successful terminal receipt missing output_zip",
            ));
        }
        Ok(LabCompletedProjection {
            run_id: self.run_id.clone(),
            status: status.to_string(),
            ok,
            record_type: record_type.to_string(),
            output_zip,
            ledger_path: ledger_path.to_path_buf(),
        })
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
        let frame_store = self.frame_store.diagnostics_json();
        let screenshots = self
            .screenshots
            .iter()
            .map(|record| {
                let mut item = json!({
                    "frame_index": record.frame_index,
                    "file": record.file,
                    "width": record.width,
                    "height": record.height,
                    "dwell_ms": record.dwell_ms,
                    "merged_count": record.merged_count,
                    "matched_page": record.matched_page,
                    "recognition_state": record.recognition_state.as_json(),
                    "storage_state": record.storage_state.as_str(),
                    "key_frame": record.key_frame
                });
                if let Some(evidence) = self.screenshot_evidence.iter().find(|evidence| {
                    evidence.get("file").and_then(Value::as_str) == Some(record.file.as_str())
                }) {
                    item["evidence"] = evidence.clone();
                }
                item
            })
            .collect::<Vec<_>>();
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
            "partial_output": self.partial_output,
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
            "frame_store": frame_store,
            "screenshot_evidence": self.screenshot_evidence.clone(),
            "screenshots": screenshots,
            "steps": self.steps
        })
    }

    fn diagnostics_json(&self, failure_reason: Option<&str>, state: Option<&RunState>) -> Value {
        let frame_store = self.frame_store.diagnostics_json();
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
                "channel_order_contract": attempt.channel_order_contract,
                "message": attempt.message
            })).collect::<Vec<_>>(),
            "frame_store": frame_store,
            "screenshot_evidence": self.screenshot_evidence.clone(),
            "input_structure": self.input_entries,
            "resource_load_results": state.map(|state| json!({
                "manifest": state.resources.manifest_path,
                "operation": state.resources.operation_path,
                "resource_root": state.resources.resource_root,
                "pack": state.resources.pack_path,
                "recognition_unsupported_target_count": state.resources.evaluator.unsupported_target_count(),
                "recognition_unsupported_targets": unsupported_targets_json(state.resources.evaluator.unsupported_targets()),
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

fn scene_from_frame(frame: &Frame) -> CliOutcome<Scene> {
    let pixel_format = match frame.pixel_format {
        PixelFormat::Rgb8 => ScenePixelFormat::Rgb8,
        PixelFormat::Rgba8 => ScenePixelFormat::Rgba8,
    };
    Scene::from_pixels(frame.width, frame.height, &frame.pixels, pixel_format)
        .map_err(|err| CliError::device(err.to_string()))
}

#[derive(Debug)]
struct ArchiveResult {
    path: PathBuf,
    sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TerminalOutputZip {
    path: String,
    sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LabCompletedProjection {
    run_id: String,
    status: String,
    ok: bool,
    record_type: String,
    output_zip: Option<TerminalOutputZip>,
    ledger_path: PathBuf,
}

impl LabCompletedProjection {
    fn require_output_zip(&self) -> CliOutcome<&TerminalOutputZip> {
        self.output_zip.as_ref().ok_or_else(|| {
            CliError::package_invalid("runtime ledger completed projection missing output_zip")
        })
    }
}

struct LabLogProjection {
    events: Vec<Value>,
    recognition: Vec<Value>,
    evidence: Vec<Value>,
    summary: Value,
    diagnostics: Value,
    environment: Value,
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

fn path_is_inside(path: &Path, parent: &Path) -> bool {
    let path = normalized_absolute_path(path);
    let parent = normalized_absolute_path(parent);
    path != parent && path.starts_with(parent)
}

fn normalized_absolute_path(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    normalize_path_components(&absolute)
}

fn normalize_path_components(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(value) => normalized.push(value),
            Component::RootDir | Component::Prefix(_) => normalized.push(component.as_os_str()),
        }
    }
    normalized
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
    let result = write_output_zip_inner(output_dir, out_path);
    if result.is_err() {
        let _ = fs::remove_file(out_path);
    }
    result
}

fn write_output_zip_inner(output_dir: &Path, out_path: &Path) -> CliOutcome<()> {
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
    let file = zip
        .finish()
        .map_err(|err| CliError::package_invalid(format!("failed to finish output zip: {err}")))?;
    file.sync_all().map_err(|err| {
        CliError::package_invalid(format!(
            "failed to sync output zip {}: {err}",
            out_path.display()
        ))
    })?;
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
        "any_of_passed": evaluation.any_of_passed,
        "any_of_total": evaluation.any_of_total,
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

fn parse_optional_sha256(flags: &FlagArgs, name: &str) -> CliOutcome<Option<Sha256Hash>> {
    flags
        .optional(name)
        .filter(|value| value != "true")
        .map(|value| Sha256Hash::parse_hex(&value).map_err(containment_error))
        .transpose()
}

fn parse_optional_f32(flags: &FlagArgs, name: &str) -> CliOutcome<Option<f32>> {
    flags
        .optional(name)
        .filter(|value| value != "true")
        .map(|value| {
            value.parse::<f32>().map_err(|err| {
                CliError::usage(format!("failed to parse {name} value '{value}': {err}"))
            })
        })
        .transpose()
}

fn parse_optional_f64(flags: &FlagArgs, name: &str) -> CliOutcome<Option<f64>> {
    flags
        .optional(name)
        .filter(|value| value != "true")
        .map(|value| {
            value.parse::<f64>().map_err(|err| {
                CliError::usage(format!("failed to parse {name} value '{value}': {err}"))
            })
        })
        .transpose()
}

fn parse_frame_store_control_from_flags(flags: &FlagArgs) -> CliOutcome<FrameStoreControl> {
    let control = FrameStoreControl {
        similarity_threshold: parse_optional_f32(flags, "--similarity-threshold")?,
        tier1_ratio: parse_optional_f64(flags, "--tier1-ratio")?,
        tier2_ratio: parse_optional_f64(flags, "--tier2-ratio")?,
        tier3_ratio: parse_optional_f64(flags, "--tier3-ratio")?,
        hysteresis_ratio: parse_optional_f64(flags, "--hysteresis-ratio")?,
        max_mem_bytes: parse_optional_u64(flags, "--max-mem-bytes")?,
        os_reserve_bytes: parse_optional_u64(flags, "--os-reserve-bytes")?,
        flush_workspace_reserve_bytes: parse_optional_u64(
            flags,
            "--flush-workspace-reserve-bytes",
        )?,
    };
    control.validate().map_err(CliError::usage)?;
    Ok(control)
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
    let mut child = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "echo")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().ok()? {
            break status;
        }
        if started.elapsed() >= GIT_COMMIT_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        thread::sleep(Duration::from_millis(25));
    };

    if !status.success() {
        return None;
    }
    let mut stdout = Vec::new();
    child.stdout.take()?.read_to_end(&mut stdout).ok()?;
    Some(String::from_utf8_lossy(&stdout).trim().to_string())
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
    use actingcommand_recognition_pack::load_pack_from_json_str;
    use tempfile::TempDir;
    use zip::ZipArchive;

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
    fn lab_validate_accepts_minimal_self_contained_package() {
        let temp = TempDir::new().expect("temp");
        let zip = temp.path().join("input.zip");
        write_minimal_lab_package(&zip);

        let result = super::super::run_cli(
            ["--json", "lab", "validate", "--zip", zip.to_str().unwrap()],
            true,
        );

        assert_eq!(result.exit_code(), 0);
        let data = result.envelope.data.as_ref().unwrap();
        assert_eq!(data["status"], "valid");
        assert_eq!(data["control"]["entry_task_id"], "task");
        assert_eq!(data["resources"]["operation_count"], 1);
    }

    #[test]
    fn lab_validate_rejects_expected_sha256_mismatch() {
        let temp = TempDir::new().expect("temp");
        let zip = temp.path().join("input.zip");
        write_minimal_lab_package(&zip);

        let result = super::super::run_cli(
            [
                "--json",
                "lab",
                "validate",
                "--zip",
                zip.to_str().unwrap(),
                "--expected-sha256",
                "0000000000000000000000000000000000000000000000000000000000000000",
            ],
            true,
        );

        assert_eq!(result.exit_code(), 2);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "package_invalid"
        );
    }

    #[test]
    fn lab_validate_reports_unsupported_recognition_target_count() {
        let temp = TempDir::new().expect("temp");
        let zip = temp.path().join("input.zip");
        write_lab_package_with_unsupported_recognition(&zip);

        let result = super::super::run_cli(
            ["--json", "lab", "validate", "--zip", zip.to_str().unwrap()],
            true,
        );

        assert_eq!(result.exit_code(), 0);
        let resources = result
            .envelope
            .data
            .as_ref()
            .unwrap()
            .get("resources")
            .unwrap();
        assert_eq!(
            resources
                .get("recognition_unsupported_target_count")
                .and_then(Value::as_u64),
            Some(1)
        );
        assert_eq!(
            resources
                .pointer("/recognition_unsupported_targets/0/id")
                .and_then(Value::as_str),
            Some("page/home")
        );
    }

    #[test]
    fn lab_validate_rejects_missing_control() {
        let temp = TempDir::new().expect("temp");
        let zip = temp.path().join("input.zip");
        write_test_zip(&zip, &[("resources/manifest.json", br#"{}"#)]);

        let result = super::super::run_cli(
            ["--json", "lab", "validate", "--zip", zip.to_str().unwrap()],
            true,
        );

        assert_eq!(result.exit_code(), 2);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "package_invalid"
        );
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
            allow_placeholder_coords: None,
            output: None,
            capture_backend: None,
            frame_store: FrameStoreControl::default(),
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
            offset: None,
            target_id: None,
        };

        let err = click.validate(&control).expect_err("fullscreen rejected");
        assert_eq!(err.code, "package_invalid");
    }

    #[test]
    fn operation_validate_rejects_missing_coordinate_guard() {
        let control = test_control();
        let mut operation = test_operation(None, None);
        operation.unguarded_trusted_coordinate = false;

        let err = operation
            .validate(&control)
            .expect_err("missing guard must fail");

        assert_eq!(err.code, "package_invalid");
        assert!(err.message.contains("missing guard metadata"));
    }

    #[test]
    fn operation_validate_allows_explicit_trusted_unguarded_coordinate() {
        let control = test_control();
        let operation = test_operation(None, None);

        operation
            .validate(&control)
            .expect("explicit trusted unguarded coordinate allowed");
    }

    #[test]
    fn offset_click_rejects_color_probe_guard() {
        let control = test_control();
        let mut operation = test_operation(None, None);
        operation.unguarded_trusted_coordinate = false;
        operation.guard = Some(OperationGuard {
            page_id: "home".to_string(),
            target_id: "target/button".to_string(),
            expected_rect: PackRect {
                x: 100,
                y: 200,
                width: 20,
                height: 30,
            },
            verify_template: None,
            color_probe: Some("target/button".to_string()),
        });
        operation.click = OperationClick {
            kind: "offset".to_string(),
            x: None,
            y: None,
            width: None,
            height: None,
            from_rect: None,
            to_rect: None,
            duration_ms: None,
            offset: Some(PackRect {
                x: 3,
                y: 4,
                width: 5,
                height: 6,
            }),
            target_id: Some("target/button".to_string()),
        };

        let err = operation
            .validate(&control)
            .expect_err("color guard cannot drive offset");
        assert!(err.message.contains("color-probe guards cannot produce"));
    }

    #[test]
    fn offset_click_uses_matched_rect_and_offset_for_actual_point() {
        let control = test_control();
        let mut operation = test_operation(None, None);
        operation.unguarded_trusted_coordinate = false;
        operation.guard = Some(OperationGuard {
            page_id: "home".to_string(),
            target_id: "target/button".to_string(),
            expected_rect: PackRect {
                x: 100,
                y: 200,
                width: 20,
                height: 30,
            },
            verify_template: Some("target/button".to_string()),
            color_probe: None,
        });
        operation.click = OperationClick {
            kind: "offset".to_string(),
            x: None,
            y: None,
            width: None,
            height: None,
            from_rect: None,
            to_rect: None,
            duration_ms: None,
            offset: Some(PackRect {
                x: 3,
                y: 4,
                width: 5,
                height: 6,
            }),
            target_id: Some("target/button".to_string()),
        };

        operation.validate(&control).expect("offset valid");
        let action = operation
            .input_action(
                &control.resolution,
                0,
                Some(&template_target_evaluation(
                    "target/button",
                    PackRect {
                        x: 300,
                        y: 400,
                        width: 20,
                        height: 30,
                    },
                )),
            )
            .expect("input action");

        match action {
            LabInputAction::Tap(point) => {
                assert_eq!(point.rect.x, 303);
                assert_eq!(point.rect.y, 404);
                assert_eq!(point.rect.width, 5);
                assert_eq!(point.rect.height, 6);
            }
            _ => panic!("expected tap"),
        }
    }

    #[test]
    fn offset_click_rejects_mismatched_guard_target_at_action_time() {
        let control = test_control();
        let mut operation = test_operation(None, None);
        operation.unguarded_trusted_coordinate = false;
        operation.guard = Some(OperationGuard {
            page_id: "home".to_string(),
            target_id: "target/button".to_string(),
            expected_rect: PackRect {
                x: 100,
                y: 200,
                width: 20,
                height: 30,
            },
            verify_template: Some("target/button".to_string()),
            color_probe: None,
        });
        operation.click = OperationClick {
            kind: "offset".to_string(),
            x: None,
            y: None,
            width: None,
            height: None,
            from_rect: None,
            to_rect: None,
            duration_ms: None,
            offset: Some(PackRect {
                x: 3,
                y: 4,
                width: 5,
                height: 6,
            }),
            target_id: Some("target/button".to_string()),
        };

        operation.validate(&control).expect("offset valid");
        let err = operation
            .input_action(
                &control.resolution,
                0,
                Some(&template_target_evaluation(
                    "target/other",
                    PackRect {
                        x: 300,
                        y: 400,
                        width: 20,
                        height: 30,
                    },
                )),
            )
            .expect_err("mismatched target must fail");

        assert!(err.message.contains("does not match guard target_id"));
    }

    #[test]
    fn guarded_drag_uses_matched_target_rect_for_start_point() {
        let control = test_control();
        let mut operation = test_operation(Some("terminal"), None);
        operation.unguarded_trusted_coordinate = false;
        operation.guard = Some(OperationGuard {
            page_id: "home".to_string(),
            target_id: "target/thumb".to_string(),
            expected_rect: PackRect {
                x: 100,
                y: 200,
                width: 20,
                height: 30,
            },
            verify_template: Some("target/thumb".to_string()),
            color_probe: None,
        });
        operation.click = OperationClick {
            kind: "drag".to_string(),
            x: None,
            y: None,
            width: None,
            height: None,
            from_rect: Some(PackRect {
                x: 103,
                y: 204,
                width: 5,
                height: 6,
            }),
            to_rect: Some(PackRect {
                x: 800,
                y: 300,
                width: 10,
                height: 10,
            }),
            duration_ms: Some(500),
            offset: None,
            target_id: None,
        };

        operation.validate(&control).expect("guarded drag valid");
        let action = operation
            .input_action(
                &control.resolution,
                0,
                Some(&template_target_evaluation(
                    "target/thumb",
                    PackRect {
                        x: 300,
                        y: 400,
                        width: 20,
                        height: 30,
                    },
                )),
            )
            .expect("drag input action");

        match action {
            LabInputAction::Drag {
                from,
                to,
                duration_ms,
            } => {
                assert_eq!(from.rect.x, 303);
                assert_eq!(from.rect.y, 404);
                assert_eq!(from.rect.width, 5);
                assert_eq!(from.rect.height, 6);
                assert_eq!(to.rect.x, 1000);
                assert_eq!(to.rect.y, 500);
                assert_eq!(to.rect.width, 10);
                assert_eq!(to.rect.height, 10);
                assert_eq!(to.rect.x - from.rect.x, 697);
                assert_eq!(to.rect.y - from.rect.y, 96);
                assert_eq!(duration_ms, 500);
            }
            _ => panic!("expected drag"),
        }
    }

    #[test]
    fn guarded_drag_rejects_mismatched_guard_target() {
        let control = test_control();
        let mut operation = test_operation(Some("terminal"), None);
        operation.unguarded_trusted_coordinate = false;
        operation.guard = Some(OperationGuard {
            page_id: "home".to_string(),
            target_id: "target/thumb".to_string(),
            expected_rect: PackRect {
                x: 100,
                y: 200,
                width: 20,
                height: 30,
            },
            verify_template: Some("target/thumb".to_string()),
            color_probe: None,
        });
        operation.click = OperationClick {
            kind: "drag".to_string(),
            x: None,
            y: None,
            width: None,
            height: None,
            from_rect: Some(PackRect {
                x: 100,
                y: 200,
                width: 20,
                height: 30,
            }),
            to_rect: Some(PackRect {
                x: 800,
                y: 300,
                width: 10,
                height: 10,
            }),
            duration_ms: Some(500),
            offset: None,
            target_id: None,
        };

        operation.validate(&control).expect("guarded drag valid");
        let err = operation
            .input_action(
                &control.resolution,
                0,
                Some(&template_target_evaluation(
                    "target/other",
                    PackRect {
                        x: 300,
                        y: 400,
                        width: 20,
                        height: 30,
                    },
                )),
            )
            .expect_err("guard target mismatch must fail");

        assert!(err.message.contains("does not match guard target_id"));
    }

    #[test]
    fn trusted_unguarded_drag_uses_original_start_rect() {
        let control = test_control();
        let mut operation = test_operation(Some("terminal"), None);
        operation.click = OperationClick {
            kind: "drag".to_string(),
            x: None,
            y: None,
            width: None,
            height: None,
            from_rect: Some(PackRect {
                x: 100,
                y: 200,
                width: 20,
                height: 30,
            }),
            to_rect: Some(PackRect {
                x: 800,
                y: 300,
                width: 10,
                height: 10,
            }),
            duration_ms: Some(500),
            offset: None,
            target_id: None,
        };

        operation
            .validate(&control)
            .expect("trusted unguarded drag valid");
        let action = operation
            .input_action(&control.resolution, 0, None)
            .expect("drag input action");

        match action {
            LabInputAction::Drag { from, .. } => {
                assert_eq!(from.rect.x, 100);
                assert_eq!(from.rect.y, 200);
                assert_eq!(from.rect.width, 20);
                assert_eq!(from.rect.height, 30);
            }
            _ => panic!("expected drag"),
        }
    }

    #[test]
    fn guarded_rect_zero_delta_matches_declared_rect() {
        let control = test_control();
        let mut operation = test_operation(Some("terminal"), None);
        operation.unguarded_trusted_coordinate = false;
        operation.guard = Some(OperationGuard {
            page_id: "home".to_string(),
            target_id: "target/button".to_string(),
            expected_rect: PackRect {
                x: 100,
                y: 200,
                width: 20,
                height: 30,
            },
            verify_template: Some("target/button".to_string()),
            color_probe: None,
        });
        operation.click = OperationClick {
            kind: "rect".to_string(),
            x: Some(110),
            y: Some(210),
            width: Some(12),
            height: Some(14),
            from_rect: None,
            to_rect: None,
            duration_ms: None,
            offset: None,
            target_id: None,
        };

        operation.validate(&control).expect("guarded rect valid");
        let action = operation
            .input_action(
                &control.resolution,
                77,
                Some(&template_target_evaluation(
                    "target/button",
                    PackRect {
                        x: 100,
                        y: 200,
                        width: 20,
                        height: 30,
                    },
                )),
            )
            .expect("input action");

        match action {
            LabInputAction::Tap(point) => {
                let expected = actual_click_point(
                    PackRect {
                        x: 110,
                        y: 210,
                        width: 12,
                        height: 14,
                    },
                    77 ^ hash_text("open_terminal"),
                );
                assert_eq!(point.rect.x, expected.rect.x);
                assert_eq!(point.rect.y, expected.rect.y);
                assert_eq!(point.x, expected.x);
                assert_eq!(point.y, expected.y);
            }
            _ => panic!("expected tap"),
        }
    }

    #[test]
    fn guarded_rect_point_and_long_press_follow_matched_delta() {
        let control = test_control();
        for kind in ["rect", "specific_rect", "point", "long_press"] {
            let mut operation = test_operation(Some("terminal"), None);
            operation.unguarded_trusted_coordinate = false;
            operation.guard = Some(OperationGuard {
                page_id: "home".to_string(),
                target_id: "target/button".to_string(),
                expected_rect: PackRect {
                    x: 100,
                    y: 200,
                    width: 20,
                    height: 30,
                },
                verify_template: Some("target/button".to_string()),
                color_probe: None,
            });
            operation.click = match kind {
                "rect" | "specific_rect" => OperationClick {
                    kind: kind.to_string(),
                    x: Some(103),
                    y: Some(204),
                    width: Some(7),
                    height: Some(9),
                    from_rect: None,
                    to_rect: None,
                    duration_ms: None,
                    offset: None,
                    target_id: None,
                },
                "point" => OperationClick {
                    kind: kind.to_string(),
                    x: Some(103),
                    y: Some(204),
                    width: None,
                    height: None,
                    from_rect: None,
                    to_rect: None,
                    duration_ms: None,
                    offset: None,
                    target_id: None,
                },
                "long_press" => OperationClick {
                    kind: kind.to_string(),
                    x: Some(103),
                    y: Some(204),
                    width: None,
                    height: None,
                    from_rect: None,
                    to_rect: None,
                    duration_ms: Some(900),
                    offset: None,
                    target_id: None,
                },
                _ => unreachable!(),
            };

            operation
                .validate(&control)
                .expect("guarded coordinate valid");
            let action = operation
                .input_action(
                    &control.resolution,
                    0,
                    Some(&template_target_evaluation(
                        "target/button",
                        PackRect {
                            x: 300,
                            y: 400,
                            width: 20,
                            height: 30,
                        },
                    )),
                )
                .expect("input action");

            match (kind, action) {
                ("long_press", LabInputAction::LongTap { point, duration_ms }) => {
                    assert_eq!(duration_ms, 900);
                    assert_eq!(point.rect.x, 303);
                    assert_eq!(point.rect.y, 404);
                    assert_eq!(point.rect.width, 1);
                    assert_eq!(point.rect.height, 1);
                    assert_eq!(point.x, 303);
                    assert_eq!(point.y, 404);
                }
                ("point", LabInputAction::Tap(point)) => {
                    assert_eq!(point.rect.x, 303);
                    assert_eq!(point.rect.y, 404);
                    assert_eq!(point.rect.width, 1);
                    assert_eq!(point.rect.height, 1);
                    assert_eq!(point.x, 303);
                    assert_eq!(point.y, 404);
                }
                (_, LabInputAction::Tap(point)) => {
                    assert_eq!(point.rect.x, 303);
                    assert_eq!(point.rect.y, 404);
                    assert_eq!(point.rect.width, 7);
                    assert_eq!(point.rect.height, 9);
                }
                _ => panic!("unexpected action for {kind}"),
            }
        }
    }

    #[test]
    fn guarded_point_rejects_mismatched_guard_target() {
        let control = test_control();
        let mut operation = test_operation(Some("terminal"), None);
        operation.unguarded_trusted_coordinate = false;
        operation.guard = Some(OperationGuard {
            page_id: "home".to_string(),
            target_id: "target/button".to_string(),
            expected_rect: PackRect {
                x: 100,
                y: 200,
                width: 20,
                height: 30,
            },
            verify_template: Some("target/button".to_string()),
            color_probe: None,
        });

        operation.validate(&control).expect("guarded point valid");
        let err = operation
            .input_action(
                &control.resolution,
                0,
                Some(&template_target_evaluation(
                    "target/other",
                    PackRect {
                        x: 300,
                        y: 400,
                        width: 20,
                        height: 30,
                    },
                )),
            )
            .expect_err("target mismatch must fail");

        assert!(err.message.contains("does not match guard target_id"));
    }

    #[test]
    fn operation_validate_rejects_color_guard_for_absolute_coordinate() {
        let control = test_control();
        let mut operation = test_operation(Some("terminal"), None);
        operation.unguarded_trusted_coordinate = false;
        operation.guard = Some(test_color_guard());

        let err = operation
            .validate(&control)
            .expect_err("color guard cannot translate coordinates");

        assert!(err.message.contains("requires template guard metadata"));
    }

    #[test]
    fn trusted_unguarded_point_and_long_press_use_original_coordinate() {
        let control = test_control();
        for kind in ["point", "long_press"] {
            let mut operation = test_operation(Some("terminal"), None);
            operation.click = OperationClick {
                kind: kind.to_string(),
                x: Some(123),
                y: Some(234),
                width: None,
                height: None,
                from_rect: None,
                to_rect: None,
                duration_ms: Some(800),
                offset: None,
                target_id: None,
            };

            operation
                .validate(&control)
                .expect("trusted unguarded coordinate valid");
            let action = operation
                .input_action(&control.resolution, 0, None)
                .expect("input action");

            match (kind, action) {
                ("long_press", LabInputAction::LongTap { point, duration_ms }) => {
                    assert_eq!(duration_ms, 800);
                    assert_eq!(point.x, 123);
                    assert_eq!(point.y, 234);
                }
                ("point", LabInputAction::Tap(point)) => {
                    assert_eq!(point.x, 123);
                    assert_eq!(point.y, 234);
                }
                _ => panic!("unexpected action for {kind}"),
            }
        }
    }

    #[test]
    fn pre_execution_guard_passes_when_page_and_target_match() {
        let mut operation = test_operation(None, None);
        operation.unguarded_trusted_coordinate = false;
        operation.guard = Some(test_color_guard());
        let guard = operation.guard.as_ref().expect("guard");
        let evaluator = one_pixel_color_evaluator([0, 0, 0]);
        let scene = captured_rgb_scene(Some("arknights/home"), [0, 0, 0]);

        let outcome =
            evaluate_pre_execution_guard("arknights", &operation, guard, &scene, &evaluator)
                .expect("guard evaluation");

        match outcome {
            PreExecutionGuardOutcome::Passed {
                current_page,
                target,
            } => {
                assert_eq!(current_page, Some("home".to_string()));
                assert!(target.passed);
                assert_eq!(target.id, "target/button");
            }
            other => panic!("expected guard pass, got {other:?}"),
        }
    }

    #[test]
    fn pre_execution_guard_rejects_changed_execution_page() {
        let mut operation = test_operation(None, None);
        operation.unguarded_trusted_coordinate = false;
        operation.guard = Some(test_color_guard());
        let guard = operation.guard.as_ref().expect("guard");
        let evaluator = one_pixel_color_evaluator([0, 0, 0]);
        let scene = captured_rgb_scene(Some("arknights/terminal"), [0, 0, 0]);

        let outcome =
            evaluate_pre_execution_guard("arknights", &operation, guard, &scene, &evaluator)
                .expect("guard evaluation");

        assert_eq!(
            outcome,
            PreExecutionGuardOutcome::Failed {
                reason: "page_guard_mismatch",
                current_page: Some("terminal".to_string()),
                diagnostics: json!({
                    "expected_page": "home",
                    "matched_page": "arknights/terminal",
                    "operation_from": "home"
                })
            }
        );
    }

    #[test]
    fn pre_execution_guard_allows_any_page_guard_when_target_matches() {
        let mut operation = test_operation(None, None);
        operation.from = "any".to_string();
        operation.unguarded_trusted_coordinate = false;
        let mut guard = test_color_guard();
        guard.page_id = "any".to_string();
        operation.guard = Some(guard);
        let guard = operation.guard.as_ref().expect("guard");
        let evaluator = one_pixel_color_evaluator([0, 0, 0]);
        let scene = captured_rgb_scene(Some("arknights/terminal"), [0, 0, 0]);

        let outcome =
            evaluate_pre_execution_guard("arknights", &operation, guard, &scene, &evaluator)
                .expect("guard evaluation");

        match outcome {
            PreExecutionGuardOutcome::Passed {
                current_page,
                target,
            } => {
                assert_eq!(current_page, Some("terminal".to_string()));
                assert!(target.passed);
            }
            other => panic!("expected guard pass, got {other:?}"),
        }
    }

    #[test]
    fn pre_execution_guard_rejects_target_mismatch_on_same_page() {
        let mut operation = test_operation(None, None);
        operation.unguarded_trusted_coordinate = false;
        operation.guard = Some(test_color_guard());
        let guard = operation.guard.as_ref().expect("guard");
        let evaluator = one_pixel_color_evaluator([255, 255, 255]);
        let scene = captured_rgb_scene(Some("arknights/home"), [0, 0, 0]);

        let outcome =
            evaluate_pre_execution_guard("arknights", &operation, guard, &scene, &evaluator)
                .expect("guard evaluation");

        match outcome {
            PreExecutionGuardOutcome::TargetMismatch {
                current_page,
                target,
                diagnostics,
            } => {
                assert_eq!(current_page, Some("home".to_string()));
                assert!(!target.passed);
                assert_eq!(
                    diagnostics
                        .pointer("/target/passed")
                        .and_then(Value::as_bool),
                    Some(false)
                );
            }
            other => panic!("expected target mismatch, got {other:?}"),
        }
    }

    #[test]
    fn resource_drift_gate_detects_stable_target_mismatch() {
        let initial = color_target_evaluation("target/button", [9, 0, 0], false);
        let mut gate = ResourceDriftGate::new(2, initial).expect("gate");

        assert_eq!(
            gate.observe(color_target_evaluation("target/button", [9, 0, 0], false)),
            ResourceDriftObservation::Drift
        );
        assert_eq!(gate.stable_mismatch_frames, 2);
        assert_eq!(gate.observed_frames, 2);
    }

    #[test]
    fn resource_drift_gate_waits_on_moving_target_mismatch() {
        let initial = color_target_evaluation("target/button", [0, 0, 0], false);
        let mut gate = ResourceDriftGate::new(2, initial).expect("gate");

        for mean in [[3, 0, 0], [6, 0, 0], [9, 0, 0]] {
            assert_eq!(
                gate.observe(color_target_evaluation("target/button", mean, false)),
                ResourceDriftObservation::Waiting
            );
        }
        assert_eq!(gate.stable_mismatch_frames, 1);
    }

    #[test]
    fn resource_drift_gate_recovers_when_target_passes() {
        let initial = color_target_evaluation("target/button", [0, 0, 0], false);
        let mut gate = ResourceDriftGate::new(2, initial).expect("gate");

        assert_eq!(
            gate.observe(color_target_evaluation("target/button", [0, 0, 0], true)),
            ResourceDriftObservation::Recovered
        );
    }

    #[test]
    fn resource_drift_gate_rejects_initial_passing_target() {
        let err =
            ResourceDriftGate::new(2, color_target_evaluation("target/button", [0, 0, 0], true))
                .expect_err("passing target is not drift");

        assert_eq!(err.code, "device_error");
        assert!(err.message.contains("initial target mismatch"));
    }

    #[test]
    fn resource_drift_diagnostics_include_recalibration_context() {
        let mut operation = test_operation(None, None);
        operation.provenance = Some(json!({"version": "pack-20260703"}));
        let guard = test_color_guard();
        let target = color_target_evaluation("target/button", [9, 0, 0], false);

        let diagnostics = resource_drift_diagnostics(&operation, &guard, &target, 2);

        assert_eq!(
            diagnostics.get("trigger").and_then(Value::as_str),
            Some("resource_drift")
        );
        assert_eq!(
            diagnostics.get("resource_status").and_then(Value::as_str),
            Some("needs_recalibration")
        );
        assert_eq!(
            diagnostics.get("target_id").and_then(Value::as_str),
            Some("target/button")
        );
        assert_eq!(
            diagnostics
                .pointer("/expected_rect/width")
                .and_then(Value::as_u64),
            Some(1)
        );
        assert_eq!(
            diagnostics
                .pointer("/measured/passed")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            diagnostics
                .get("provenance_version")
                .and_then(Value::as_str),
            Some("pack-20260703")
        );
    }

    #[test]
    fn roi_stability_gate_waits_until_roi_becomes_stable() {
        let baseline = color_target_evaluation("target/button", [0, 0, 0], true);
        let mut gate = RoiStabilityGate::new(2, baseline).expect("gate");

        assert!(!gate.observe(color_target_evaluation("target/button", [8, 0, 0], true)));
        assert!(gate.observe(color_target_evaluation("target/button", [8, 0, 0], true)));
        assert_eq!(gate.stable_frames, 2);
        assert_eq!(gate.observed_frames, 3);
    }

    #[test]
    fn roi_stability_gate_passes_static_roi_on_first_followup_frame() {
        let baseline = color_target_evaluation("target/button", [0, 0, 0], true);
        let mut gate = RoiStabilityGate::new(2, baseline).expect("gate");

        assert!(gate.observe(color_target_evaluation("target/button", [0, 0, 0], true)));
        assert_eq!(gate.observed_frames, 2);
    }

    #[test]
    fn roi_stability_gate_rejects_continuously_changing_roi() {
        let baseline = color_target_evaluation("target/button", [0, 0, 0], true);
        let mut gate = RoiStabilityGate::new(2, baseline).expect("gate");

        for mean in [[3, 0, 0], [6, 0, 0], [9, 0, 0]] {
            assert!(!gate.observe(color_target_evaluation("target/button", mean, true)));
        }
        assert_eq!(gate.stable_frames, 1);
    }

    #[test]
    fn roi_stability_gate_resets_when_target_fails() {
        let baseline = color_target_evaluation("target/button", [0, 0, 0], true);
        let mut gate = RoiStabilityGate::new(2, baseline).expect("gate");

        assert!(!gate.observe(color_target_evaluation("target/button", [0, 0, 0], false)));
        assert!(!gate.observe(color_target_evaluation("target/button", [0, 0, 0], true)));
        assert!(gate.observe(color_target_evaluation("target/button", [0, 0, 0], true)));
        assert_eq!(gate.stable_frames, 2);
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
    fn operation_verification_uses_expect_after_page() {
        let mut operation = test_operation(None, None);
        operation.expect_after = Some(OperationExpectation {
            page_id: "terminal".to_string(),
            timeout_ms: Some(50),
            interval_ms: None,
        });
        let matched = captured_scene(Some("arknights/terminal"), false);
        let mismatched = captured_scene(Some("arknights/home"), false);

        assert_eq!(
            operation_verification_status("arknights", &operation, &matched),
            OperationVerification::Verified
        );
        assert_eq!(
            operation_verification_status("arknights", &operation, &mismatched),
            OperationVerification::Failed
        );
        assert_eq!(operation.expected_after_page(), Some("terminal"));
        assert_eq!(operation.after_timeout_ms(10_000), 50);
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

    #[test]
    fn failure_zip_materializes_frame_store_screenshots() {
        let temp = TempDir::new().expect("temp");
        let mut ctx = LabRunContext::create(temp.path(), Path::new("input.zip")).expect("ctx");
        let frame = Frame::from_pixels(
            1,
            1,
            vec![0, 0, 0, 255],
            PixelFormat::Rgba8,
            CaptureBackendName::NemuIpc,
        )
        .expect("frame");
        ctx.frame_index = 1;
        ctx.frame_store
            .add_frame(FrameStoreFrameInput {
                frame_index: 1,
                file_name: "frame1.png".to_string(),
                label: "initial".to_string(),
                recognition_state: RecognitionState::from_matched_page(Some(
                    "arknights/home".to_string(),
                )),
                frame,
            })
            .expect("frame store");
        let out = temp.path().join("out.zip");

        ctx.finish(&out, false, Some("synthetic failure"), None)
            .expect("finish");

        let file = File::open(&out).expect("zip");
        let mut archive = ZipArchive::new(file).expect("archive");
        assert!(archive.by_name("screenshots/frame1.png").is_ok());
        assert!(archive.by_name("logs/evidence.json").is_ok());
        assert!(archive.by_name("logs/frame_store.json").is_ok());
        assert!(archive.by_name("logs/frame_timeline.jsonl").is_ok());
        let summary: Value =
            serde_json::from_reader(archive.by_name("logs/summary.json").expect("summary"))
                .expect("summary json");
        assert_eq!(
            summary
                .pointer("/projection_source/kind")
                .and_then(Value::as_str),
            Some("runtime_ledger")
        );
        let screenshot_evidence = summary
            .pointer("/screenshots/0/evidence")
            .expect("screenshot evidence");
        assert_eq!(
            screenshot_evidence.get("status").and_then(Value::as_str),
            Some("indexed")
        );
        let evidence_relative_path = screenshot_evidence
            .pointer("/refs/0/relative_path")
            .and_then(Value::as_str)
            .expect("relative evidence path");
        assert!(temp.path().join(evidence_relative_path).is_file());
        let events = zip_text(&mut archive, "logs/events.jsonl");
        assert!(events.contains("runtime_ledger"));
        let diagnostics: Value = serde_json::from_reader(
            archive
                .by_name("logs/diagnostics.json")
                .expect("diagnostics"),
        )
        .expect("diagnostics json");
        assert_eq!(
            diagnostics.get("command").and_then(Value::as_str),
            Some("lab run")
        );
        assert!(ctx.ledger_path.as_ref().expect("ledger path").is_file());
        let ledger = LabLedger::read(ctx.ledger_path.as_ref().unwrap()).expect("ledger read");
        assert!(!ledger.events.is_empty());
        assert!(ledger.records.iter().any(|record| {
            record.kind == LedgerRecordKind::Drive
                && record.payload.get("record_type").and_then(Value::as_str)
                    == Some("evidence_index")
        }));
        assert!(ledger.records.iter().any(|record| {
            record.kind == LedgerRecordKind::Drive
                && record.payload.get("record_type").and_then(Value::as_str) == Some("finalizing")
        }));
        assert!(ledger.records.iter().any(|record| {
            record.kind == LedgerRecordKind::Receipt
                && record.payload.get("record_type").and_then(Value::as_str) == Some("finish_error")
        }));
        assert!(ctx.run_dir.exists());
    }

    #[test]
    fn screenshot_evidence_records_degradation_when_file_is_missing() {
        let temp = TempDir::new().expect("temp");
        let mut ctx = LabRunContext::create(temp.path(), Path::new("input.zip")).expect("ctx");
        let frame = Frame::from_pixels(
            1,
            1,
            vec![0, 0, 0, 255],
            PixelFormat::Rgba8,
            CaptureBackendName::NemuIpc,
        )
        .expect("frame");
        ctx.ensure_ledger().expect("ledger");
        ctx.frame_store
            .add_frame(FrameStoreFrameInput {
                frame_index: 1,
                file_name: "missing.png".to_string(),
                label: "missing".to_string(),
                recognition_state: RecognitionState::from_matched_page(Some(
                    "arknights/home".to_string(),
                )),
                frame,
            })
            .expect("frame store");
        ctx.frame_store
            .materialize(&ctx.screenshots_dir)
            .expect("materialize");
        ctx.screenshots = ctx.frame_store.screenshots();
        let screenshot_path = ctx.output_dir.join(&ctx.screenshots[0].file);
        fs::remove_file(&screenshot_path).expect("remove screenshot");

        ctx.index_screenshot_evidence().expect("index evidence");

        let evidence = ctx.screenshot_evidence.first().expect("evidence");
        assert_eq!(
            evidence.get("status").and_then(Value::as_str),
            Some("degraded")
        );
        assert!(
            evidence
                .get("warnings")
                .and_then(Value::as_array)
                .and_then(|warnings| warnings.first())
                .and_then(Value::as_str)
                .is_some_and(|warning| warning.contains("failed to read screenshot evidence"))
        );
        let ledger = LabLedger::read(ctx.ledger_path.as_ref().unwrap()).expect("ledger read");
        let record = ledger
            .records
            .iter()
            .find(|record| {
                record.kind == LedgerRecordKind::Drive
                    && record.payload.get("record_type").and_then(Value::as_str)
                        == Some("evidence_index")
            })
            .expect("evidence index record");
        assert_eq!(
            record.payload.get("indexed_count").and_then(Value::as_u64),
            Some(0)
        );
        assert_eq!(
            record.payload.get("degraded_count").and_then(Value::as_u64),
            Some(1)
        );
    }

    #[test]
    fn recognition_projection_keeps_reco_id_from_ledger() {
        let temp = TempDir::new().expect("temp");
        let mut ctx = LabRunContext::create(temp.path(), Path::new("input.zip")).expect("ctx");
        ctx.ensure_ledger().expect("ledger");
        let frame = Frame::from_pixels(
            1,
            1,
            vec![0, 0, 0, 255],
            PixelFormat::Rgba8,
            CaptureBackendName::NemuIpc,
        )
        .expect("frame");
        let mut capture = StaticCapture { frame };
        let evaluator = one_pixel_color_evaluator([0, 0, 0]);
        let page_set = actingcommand_page_detector::load_page_set_from_json_str(
            r#"{
                "schema_version":"0.3",
                "pages":[
                    {"id":"arknights/home","required":["target/button"],"optional":[],"forbidden":[]}
                ]
            }"#,
        )
        .expect("page set");
        let detector = PageDetector::new(page_set).expect("detector");

        let scene = ctx
            .capture_scene_with_pages(
                &mut capture,
                &evaluator,
                &detector,
                "initial",
                Some(&["arknights/home".to_string()]),
            )
            .expect("capture scene");
        assert_eq!(scene.matched_page.as_deref(), Some("arknights/home"));
        let out = temp.path().join("out.zip");
        ctx.finish(&out, true, None, None).expect("finish");

        let file = File::open(&out).expect("zip");
        let mut archive = ZipArchive::new(file).expect("archive");
        let recognition_text = zip_text(&mut archive, "logs/recognition.jsonl");
        let recognition = recognition_text
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("recognition line"))
            .collect::<Vec<_>>();
        let reco_id = recognition
            .first()
            .and_then(|item| item.get("reco_id"))
            .and_then(Value::as_str)
            .expect("reco_id");
        assert!(reco_id.starts_with("reco-"));
        let ledger = LabLedger::read(ctx.ledger_path.as_ref().unwrap()).expect("ledger read");
        let record = ledger
            .records
            .iter()
            .find(|record| {
                record.kind == LedgerRecordKind::Drive
                    && record.payload.get("record_type").and_then(Value::as_str)
                        == Some("recognition")
            })
            .expect("recognition record");
        assert_eq!(
            record.id_chain.get("reco_id").map(String::as_str),
            Some(reco_id)
        );
        assert_eq!(
            record
                .payload
                .pointer("/recognition/reco_id")
                .and_then(Value::as_str),
            Some(reco_id)
        );
    }

    #[test]
    fn capture_backend_selection_is_recorded_in_ledger() {
        let temp = TempDir::new().expect("temp");
        let mut ctx = LabRunContext::create(temp.path(), Path::new("input.zip")).expect("ctx");
        ctx.ensure_ledger().expect("ledger");
        ctx.capture_backend_requested = Some(CaptureBackendChoice::Auto);
        ctx.capture_backend_used = Some(CaptureBackendName::NemuIpc);
        ctx.capture_backend_attempts = vec![CaptureBackendAttempt {
            backend: CaptureBackendName::NemuIpc,
            ok: true,
            message: "selected nemu ipc".to_string(),
            elapsed_ms: Some(12),
            cached: false,
            channel_order_contract: "test-order",
            vendor_stdio: Vec::new(),
        }];

        ctx.record_capture_backend_selection()
            .expect("record capture selection");

        let ledger = LabLedger::read(ctx.ledger_path.as_ref().unwrap()).expect("ledger read");
        let record = ledger
            .records
            .iter()
            .find(|record| {
                record.kind == LedgerRecordKind::Drive
                    && record.payload.get("record_type").and_then(Value::as_str)
                        == Some("capture_backend_selection")
            })
            .expect("capture selection record");
        assert_eq!(
            record.payload.get("requested").and_then(Value::as_str),
            Some("auto")
        );
        assert_eq!(
            record.payload.get("used").and_then(Value::as_str),
            Some("nemu_ipc")
        );
        assert_eq!(
            record.payload.get("attempt_count").and_then(Value::as_u64),
            Some(1)
        );
        assert_eq!(
            record
                .payload
                .pointer("/attempts/0/backend")
                .and_then(Value::as_str),
            Some("nemu_ipc")
        );
        assert_eq!(
            record
                .payload
                .pointer("/attempts/0/message")
                .and_then(Value::as_str),
            Some("selected nemu ipc")
        );
    }

    #[test]
    fn success_finish_cleans_run_dir_but_keeps_outside_zip() {
        let temp = TempDir::new().expect("temp");
        let mut ctx = LabRunContext::create(temp.path(), Path::new("input.zip")).expect("ctx");
        let out = temp.path().join("out.zip");

        ctx.finish(&out, true, None, None).expect("finish");

        assert!(out.is_file());
        assert!(!ctx.run_dir.exists());
        assert!(ctx.ledger_path.as_ref().expect("ledger path").is_file());
        let ledger_text =
            fs::read_to_string(ctx.ledger_path.as_ref().unwrap()).expect("ledger text");
        assert_ordered(
            &ledger_text,
            &[
                "\"record_type\":\"finalizing\"",
                "\"event\":\"output_zip_written\"",
                "\"record_type\":\"finish_ok\"",
            ],
        );
    }

    #[test]
    fn completed_projection_reports_terminal_output_zip() {
        let temp = TempDir::new().expect("temp");
        let mut ctx = LabRunContext::create(temp.path(), Path::new("input.zip")).expect("ctx");
        let out = temp.path().join("out.zip");

        let archive = ctx.finish(&out, true, None, None).expect("finish");
        let completed = ctx
            .project_completed_run_from_ledger()
            .expect("completed projection");
        let output_zip = completed.require_output_zip().expect("output zip");

        assert!(completed.ok);
        assert_eq!(completed.status, "ok");
        assert_eq!(completed.record_type, "finish_ok");
        assert_eq!(completed.run_id, ctx.run_id);
        assert_eq!(output_zip.path, out.display().to_string());
        assert_eq!(output_zip.sha256, archive.sha256);
        assert_eq!(
            completed.ledger_path,
            ctx.ledger_path.as_ref().expect("ledger path").to_path_buf()
        );
    }

    #[test]
    fn completed_projection_requires_finalizing_record() {
        let temp = TempDir::new().expect("temp");
        let mut ctx = LabRunContext::create(temp.path(), Path::new("input.zip")).expect("ctx");
        ctx.ensure_ledger().expect("ledger");
        ctx.append_terminal_receipt(false, Some("terminal failure"), None, None)
            .expect("terminal receipt");

        let err = ctx
            .project_completed_run_from_ledger()
            .expect_err("missing finalizing must fail");

        assert!(err.message.contains("missing finalizing record"));
    }

    #[test]
    fn completed_projection_requires_terminal_receipt() {
        let temp = TempDir::new().expect("temp");
        let mut ctx = LabRunContext::create(temp.path(), Path::new("input.zip")).expect("ctx");
        ctx.ensure_ledger().expect("ledger");
        let summary = ctx.summary_json(true, None, None);
        let diagnostics = ctx.diagnostics_json(None, None);
        let environment = ctx.environment_json(None);
        ctx.append_finalizing_record(true, None, summary, diagnostics, environment)
            .expect("finalizing");

        let err = ctx
            .project_completed_run_from_ledger()
            .expect_err("missing terminal receipt must fail");

        assert!(err.message.contains("missing terminal receipt"));
    }

    #[test]
    fn zip_failure_after_success_does_not_record_finish_ok() {
        let temp = TempDir::new().expect("temp");
        let mut ctx = LabRunContext::create(temp.path(), Path::new("input.zip")).expect("ctx");
        let blocked_parent = temp.path().join("blocked-parent");
        fs::write(&blocked_parent, b"not a directory").expect("blocker");
        let out = blocked_parent.join("out.zip");

        let err = ctx
            .finish(&out, true, None, None)
            .expect_err("zip write failure");

        assert!(err.message.contains("terminal output failed"));
        assert!(temp.path().join("last-error.json").is_file());
        let ledger = LabLedger::read(ctx.ledger_path.as_ref().unwrap()).expect("ledger read");
        assert!(has_record_type(&ledger, "finalizing"));
        assert!(has_record_type(&ledger, "finish_error"));
        assert!(!has_record_type(&ledger, "finish_ok"));
        assert!(!has_event(&ledger, "output_zip_written"));
    }

    #[test]
    fn write_logs_failure_does_not_record_finish_ok() {
        let temp = TempDir::new().expect("temp");
        let mut ctx = LabRunContext::create(temp.path(), Path::new("input.zip")).expect("ctx");
        fs::remove_dir_all(&ctx.logs_dir).expect("remove logs dir");
        fs::write(&ctx.logs_dir, b"not a directory").expect("logs blocker");
        let out = temp.path().join("out.zip");

        let err = ctx
            .finish(&out, true, None, None)
            .expect_err("write_logs failure");

        assert!(err.message.contains("terminal output failed"));
        assert!(temp.path().join("last-error.json").is_file());
        let ledger = LabLedger::read(ctx.ledger_path.as_ref().unwrap()).expect("ledger read");
        assert!(has_record_type(&ledger, "finalizing"));
        assert!(has_record_type(&ledger, "finish_error"));
        assert!(!has_record_type(&ledger, "finish_ok"));
        assert!(!has_event(&ledger, "output_zip_written"));
    }

    #[test]
    fn path_inside_detects_run_dir_output() {
        let temp = TempDir::new().expect("temp");
        let ctx = LabRunContext::create(temp.path(), Path::new("input.zip")).expect("ctx");
        let inside = ctx.run_dir.join("result.zip");
        let outside = temp.path().join("result.zip");

        assert!(path_is_inside(&inside, &ctx.run_dir));
        assert!(!path_is_inside(&outside, &ctx.run_dir));
    }

    #[test]
    fn tier3_pause_checkpoint_includes_step_context() {
        let temp = TempDir::new().expect("temp");
        let mut ctx = LabRunContext::create(temp.path(), Path::new("input.zip")).expect("ctx");
        ctx.set_phase("page_guard_started");
        let operation = test_operation(Some("terminal"), None);
        ctx.set_step_context(7, &operation);
        let mut checkpoint = Tier3PauseCheckpoint {
            last_frame_index: 12,
            resident_bytes: 34,
            tier1_bytes: 10,
            tier2_bytes: 20,
            tier3_bytes: 30,
            active_segment_id: None,
            in_flight_flush_state: "idle".to_string(),
            current_step_index: None,
            current_step_id: None,
            current_operation_id: None,
            current_phase: None,
            expected_page: None,
            last_matched_page: None,
        };

        ctx.fill_pause_checkpoint(&mut checkpoint, Some("arknights/home"));
        let json = checkpoint.to_json();

        assert_eq!(json["current_step_index"], 7);
        assert_eq!(json["current_step_id"], "open_terminal");
        assert_eq!(json["current_operation_id"], "open_terminal");
        assert_eq!(json["current_phase"], "page_guard_started");
        assert_eq!(json["expected_page"], "terminal");
        assert_eq!(json["last_matched_page"], "arknights/home");
    }

    #[test]
    fn rejects_dangerous_zip_entry_without_writing_it() {
        let temp = TempDir::new().expect("temp");
        let zip = temp.path().join("input.zip");
        write_test_zip(
            &zip,
            &[
                ("control.json", br#"{}"#),
                ("resources/manifest.json", br#"{}"#),
                ("resources/tool.exe", b"danger"),
            ],
        );

        let err = match validate_lab_package_zip(&zip) {
            Ok(_) => panic!("dangerous entry accepted"),
            Err(err) => err,
        };

        assert_eq!(err.code, "package_invalid");
        assert!(
            !temp
                .path()
                .join("input")
                .join("resources")
                .join("tool.exe")
                .exists()
        );
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
            allow_placeholder_coords: None,
            output: None,
            capture_backend: None,
            frame_store: FrameStoreControl::default(),
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
                offset: None,
                target_id: None,
            },
            verify_template: verify_template.map(str::to_string),
            expect_after: None,
            guard: None,
            unguarded_trusted_coordinate: true,
            consumes: Vec::new(),
            produces: Vec::new(),
            verified_live: None,
            provenance: None,
        }
    }

    fn test_color_guard() -> OperationGuard {
        OperationGuard {
            page_id: "home".to_string(),
            target_id: "target/button".to_string(),
            expected_rect: PackRect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
            verify_template: None,
            color_probe: Some("target/button".to_string()),
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

    fn captured_rgb_scene(page: Option<&str>, rgb: [u8; 3]) -> CapturedScene {
        CapturedScene {
            scene: Scene::from_pixels(1, 1, &rgb, ScenePixelFormat::Rgb8).expect("scene"),
            matched_page: page.map(str::to_string),
            verify_template_matched: false,
            width: 1,
            height: 1,
        }
    }

    fn one_pixel_color_evaluator(expected: [u8; 3]) -> RecognitionEvaluator {
        let pack = load_pack_from_json_str(&format!(
            r#"{{
                "schema_version":"0.3",
                "game":"arknights",
                "server":"cn",
                "coordinate_space":{{"width":1,"height":1}},
                "defaults":{{"color_max_distance":0.0}},
                "targets":[{{
                    "type":"color",
                    "id":"target/button",
                    "region":{{"x":0,"y":0,"width":1,"height":1}},
                    "expected":[{},{},{}]
                }}]
            }}"#,
            expected[0], expected[1], expected[2]
        ))
        .expect("pack");
        RecognitionEvaluator::new(PathBuf::from("."), pack).expect("evaluator")
    }

    struct StaticCapture {
        frame: Frame,
    }

    impl CaptureBackend for StaticCapture {
        fn capture(&mut self) -> actingcommand_device::DeviceResult<Frame> {
            Ok(self.frame.clone())
        }
    }

    fn color_target_evaluation(id: &str, mean: [u8; 3], passed: bool) -> TargetEvaluation {
        TargetEvaluation {
            id: id.to_string(),
            kind: TargetKind::Color,
            passed,
            template: None,
            color: Some(actingcommand_recognition_pack::ColorEvaluation {
                distance: 0.0,
                max_distance: 20.0,
                mean,
                expected: mean,
            }),
            message: if passed {
                "color passed".to_string()
            } else {
                "color failed".to_string()
            },
        }
    }

    fn template_target_evaluation(id: &str, rect: PackRect) -> TargetEvaluation {
        TargetEvaluation {
            id: id.to_string(),
            kind: TargetKind::Template,
            passed: true,
            template: Some(actingcommand_recognition_pack::TemplateEvaluation {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height,
                raw_score: 1.0,
                score: 1.0,
                threshold: 0.9,
            }),
            color: None,
            message: "template passed".to_string(),
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

    fn zip_text(archive: &mut ZipArchive<File>, name: &str) -> String {
        let mut entry = archive.by_name(name).expect("zip entry");
        let mut text = String::new();
        entry.read_to_string(&mut text).expect("zip text");
        text
    }

    fn has_record_type(ledger: &actingcommand_ledger::LedgerRead, record_type: &str) -> bool {
        ledger.records.iter().any(|record| {
            record.payload.get("record_type").and_then(Value::as_str) == Some(record_type)
        })
    }

    fn has_event(ledger: &actingcommand_ledger::LedgerRead, event: &str) -> bool {
        ledger
            .events
            .iter()
            .any(|entry| entry.payload.get("event").and_then(Value::as_str) == Some(event))
    }

    fn assert_ordered(text: &str, needles: &[&str]) {
        let mut previous = 0;
        for needle in needles {
            let offset = text[previous..].find(needle).expect("needle order");
            previous += offset + needle.len();
        }
    }

    fn write_minimal_lab_package(path: &Path) {
        write_test_zip(
            path,
            &[
                (
                    "control.json",
                    br#"{
                        "schema_version":"Lab-1y.control.v1",
                        "package_id":"fixture.task",
                        "execution_mode":"recognize_only",
                        "game":"arknights",
                        "server":"cn",
                        "resolution":{"width":1280,"height":720},
                        "entry_task_id":"task"
                    }"#,
                ),
                (
                    "resources/manifest.json",
                    br#"{"schema_version":"0.3","entry_task_id":"task"}"#,
                ),
                (
                    "resources/operations/task/task.json",
                    br#"{
                        "schema_version":"0.3",
                        "task_id":"task",
                        "game":"arknights",
                        "server_scope":["cn"],
                        "goal":"fixture",
                        "coordinate_space":{"width":1280,"height":720},
                        "defaults":{"template_threshold":0.9,"color_max_distance":20.0},
                        "anchors":[{"id":"home","template":"assets/PAGE_HOME.png"}],
                        "entry_page":"home",
                        "target_page":"home",
                        "operations":[
                            {
                                "id":"noop",
                                "purpose":"fixture",
                                "from":"home",
                                "to":null,
                                "click":{"kind":"point","x":1,"y":1},
                                "verify_template":null,
                                "unguarded_trusted_coordinate":true,
                                "consumes":[],
                                "produces":[]
                            }
                        ]
                    }"#,
                ),
                ("resources/operations/task/assets/PAGE_HOME.png", one_pixel_png()),
                (
                    "resources/recognition/arknights.cn.pack.json",
                    br#"{
                        "schema_version":"0.3",
                        "game":"arknights",
                        "server":"cn",
                        "locale":"zh-CN",
                        "coordinate_space":{"width":1280,"height":720},
                        "defaults":{"template_threshold":0.9,"color_max_distance":20.0},
                        "targets":[
                            {
                                "type":"template",
                                "id":"page/home",
                                "template_path":"operations/task/assets/PAGE_HOME.png",
                                "region":{"x":0,"y":0,"width":1,"height":1},
                                "threshold":0.9
                            }
                        ]
                    }"#,
                ),
                (
                    "resources/recognition/arknights.cn.pages.json",
                    br#"{
                        "schema_version":"0.3",
                        "pages":[
                            {"id":"arknights/home","required":["page/home"],"optional":[],"forbidden":[]}
                        ]
                    }"#,
                ),
            ],
        );
    }

    fn write_lab_package_with_unsupported_recognition(path: &Path) {
        write_test_zip(
            path,
            &[
                (
                    "control.json",
                    br#"{
                        "schema_version":"Lab-1y.control.v1",
                        "package_id":"fixture.task",
                        "execution_mode":"recognize_only",
                        "game":"arknights",
                        "server":"cn",
                        "resolution":{"width":1280,"height":720},
                        "entry_task_id":"task"
                    }"#,
                ),
                (
                    "resources/manifest.json",
                    br#"{"schema_version":"0.3","entry_task_id":"task"}"#,
                ),
                (
                    "resources/operations/task/task.json",
                    br#"{
                        "schema_version":"0.3",
                        "task_id":"task",
                        "game":"arknights",
                        "server_scope":["cn"],
                        "goal":"fixture",
                        "coordinate_space":{"width":1280,"height":720},
                        "defaults":{"template_threshold":0.9,"color_max_distance":20.0},
                        "anchors":[{"id":"home","template":"assets/PAGE_HOME.png"}],
                        "entry_page":"home",
                        "target_page":"home",
                        "operations":[
                            {
                                "id":"noop",
                                "purpose":"fixture",
                                "from":"home",
                                "to":null,
                                "click":{"kind":"point","x":1,"y":1},
                                "verify_template":null,
                                "unguarded_trusted_coordinate":true,
                                "consumes":[],
                                "produces":[]
                            }
                        ]
                    }"#,
                ),
                ("resources/operations/task/assets/PAGE_HOME.png", one_pixel_png()),
                (
                    "resources/recognition/arknights.cn.pack.json",
                    br#"{
                        "schema_version":"0.5",
                        "game":"arknights",
                        "server":"cn",
                        "locale":"zh-CN",
                        "coordinate_space":{"width":1280,"height":720},
                        "defaults":{"template_threshold":0.9,"color_max_distance":20.0},
                        "targets":[
                            {
                                "type":"template",
                                "id":"page/home",
                                "template_path":"operations/task/assets/PAGE_HOME.png",
                                "region":{"x":0,"y":0,"width":1,"height":1},
                                "threshold":0.9,
                                "method":"rgb_count",
                                "mask":{"type":"range","lower":1,"upper":255}
                            }
                        ]
                    }"#,
                ),
                (
                    "resources/recognition/arknights.cn.pages.json",
                    br#"{
                        "schema_version":"0.3",
                        "pages":[
                            {"id":"arknights/home","required":["page/home"],"optional":[],"forbidden":[]}
                        ]
                    }"#,
                ),
            ],
        );
    }
}
