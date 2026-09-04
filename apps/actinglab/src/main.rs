// SPDX-License-Identifier: AGPL-3.0-only

#![allow(clippy::result_large_err)]

use actingcommand_contract::{
    ApplicationLifecycleAction, CLI_SCHEMA_VERSION, EventActor, EventSource, LabError as CliError,
    LabErrorClass as ErrorKind,
};
#[cfg(test)]
use actingcommand_device::CaptureBackendName;
use actingcommand_device::{
    AdbPathSource, CaptureBackendChoice, Frame, InputBackend, PixelFormat, TouchBackendChoice,
    combine_operation_and_close, resolve_adb_path, vendor_stdio_session_diagnostic,
};
use actingcommand_lab::{
    InstanceConfig, PackageValidationResponse, UserConfig,
    derive_absolute_coordinate_rect_from_match,
};
#[cfg(test)]
use actingcommand_ledger::{
    EvidenceStore, LabLedger, LedgerRead, LedgerRecord, LedgerRecordKind, LightEvent, SessionHeader,
};
use actingcommand_ledger::{IdIssuer, IdKind};
use actingcommand_page_detector::{PageDetector, PageEvaluation, load_page_set_from_json_str};
use actingcommand_recognition::{MatchMetric, Scene, ScenePixelFormat};
use actingcommand_recognition_pack::{
    PackRect, RecognitionEvaluator, TargetEvaluation, TargetKind, load_pack_from_json_str,
};
use actingcommand_resource_tooling::{canonical_game, canonical_server};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use cli_information::{
    help_data, run_config, run_devices, run_doctor, run_list, run_paths, run_schema, run_status,
    version_data,
};
use cli_parse::parse_invocation;
#[cfg(test)]
use cli_result::CliErrorExitCode;
use cli_result::CliResult;
#[cfg(test)]
use commands::env_needs_detection_json;
use commands::run_session_transport;
#[cfg(test)]
use commands::session_layer_capability_contract;
#[cfg(test)]
use commands::stale_capture_recovery_json;
use commands::{
    CaptureFreshProbeReport, CaptureFreshProbeStatus, CaptureFreshnessExpectation,
    StreamInputRelayAction, capture_diagnosis_recovery_json, capture_for_command,
    capture_fresh_probe_report, capture_fresh_probe_report_json, open_cli_runtime_input_proxy,
    reject_legacy_session_routing, run_capture, run_direct_input, run_direct_touch,
    run_stream_input_relay, run_touch_probe,
};
use commands::{
    DestructiveClick, NavigationEdge, NavigationGraph, PageDetectionOutcome, SemanticInput,
    canonical_navigation_page, detect_current_page, find_navigation_route, navigation_edge_json,
    page_detection_json, parse_navigation_graph_value, parse_point_pair, point_json, rect_center,
    rect_json, rects_intersect, reject_dangerous_semantic_id, reject_destructive_overlap,
    reject_destructive_overlap_input, run_current_page, run_detect_page, run_is_visible,
    run_locate, run_navigate, run_recognize, run_session_recover, run_tap_target,
    semantic_input_json, target_eval_json, target_evaluation_rect,
};
#[cfg(test)]
use commands::{
    DirectInputCommand, DirectTouchCommand, classify_capture_freshness, instance_health_status,
};
#[cfg(test)]
use commands::{JSON_TMP_SEQ, write_json_file};
#[cfg(test)]
use commands::{
    SESSION_DAEMON_REQUEST_TIMEOUT_MS, SESSION_LEASE_STALE_MS, session_access_contract,
    session_api_contract, session_capture_policy_payload, session_self_heal_policy_payload,
};
#[cfg(test)]
use commands::{
    SessionRecordAnchorArtifact, SessionRecordAnchorBacktest, SessionRecordAnchorRegionResolution,
    SessionRecordContext, SessionRecordFrameProvenance, SessionRecordSourceFrame,
    SessionRecordStep, SessionRecordStepData, SessionRecordStepEvaluation, find_drift_amend_step,
    materialize_anchor_artifact_from_source, parse_session_record_drift_diagnostics,
    session_record_build_draft,
};
use commands::{SessionRecordRect, SessionRecordRegion, run_session_record};
use commands::{attach_env_resolved, record_env_needs_detection, record_env_resolved};
use commands::{command_capabilities, run_capabilities};
use commands::{ensure_path_within, read_json_file, write_json_file_atomic};
use commands::{finish_semantic_result_with_ledger, semantic_ledger_context};
use commands::{
    run_session_api, run_session_capture_policy, run_session_contract, run_session_record_policy,
    run_session_self_heal_policy, run_session_throat_policy,
};
use device_runtime_config::{DeviceRuntimeConfig, device_config, effective_capture_backend_choice};
use flag_args::FlagArgs;
#[rustfmt::skip] use flag_values::{
    parse_match_metric_flag, parse_optional_duration_ms, parse_optional_string_value,
    parse_optional_unit_f64, parse_optional_usize, parse_record_build_resolution,
    parse_record_duration_ms, parse_session_record_region, parse_session_record_swipe_rects,
    parse_touch_backend_override, record_amend_step_id, record_candidates_step_id,
    required_non_empty_flag, session_record_drift_diagnostics_path, split_csv,
    stream_check_requested, stream_input_relay_action, target_argument, parse_session_record_candidate_index,
};
use instance_resolution::{resolve_instance_id, resolve_instance_id_for_flags};
#[cfg(test)]
use runtime_endpoint::RuntimeEndpointChannel;
use runtime_endpoint::{
    runtime_endpoint_policy, runtime_endpoint_policy_json, runtime_tcp_available,
};
use safe_file_stem::safe_file_stem;
use serde_json::{Value, json};
#[cfg(test)]
use sha2::{Digest, Sha256};
use sha256::{file_sha256, hex_sha256};
use state_roots::{app_state_root, runtime_state_root, session_state_dir_from_flags};
#[cfg(test)]
use std::collections::BTreeSet;
use std::collections::VecDeque;
use std::env;
#[cfg(test)]
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
#[cfg(any(test, unix))]
use std::process::Command;
use std::process::ExitCode;
#[cfg(test)]
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;
#[cfg(test)]
use std::time::Instant;
use unix_time::current_unix_ms;
use user_config_store::read_user_config;
#[cfg(test)]
use user_config_store::write_user_config;
use zip::{ZipWriter, write::FileOptions};
use zip_error::{zip_io_error, zip_write_error};

mod cli_information;
mod cli_parse;
mod cli_result;
mod commands;
mod contained_resources;
#[rustfmt::skip] mod device_runtime_config;
mod drive_cli;
mod env_detection;
mod flag_args;
mod flag_values;
#[rustfmt::skip] mod instance_resolution;
mod lab2_cli;
mod lab_run;
mod maa_task_graph;
mod package_build;
mod package_cli;
pub mod project_interface;
mod readonly_cli;
pub mod recovery_exec;
mod resource_authoring;
mod resource_convert;
mod run_summary;
mod runtime_capture_backend;
mod runtime_debug;
mod runtime_endpoint;
mod runtime_input_backend;
mod runtime_session_adapter;
mod runtime_slice_cli;
mod runtime_stream_adapter;
mod safe_file_stem;
mod sha256;
mod state_roots;
mod unix_time;
mod user_config_keys;
mod user_config_store;
mod zip_error;

const SCHEMA_VERSION: &str = CLI_SCHEMA_VERSION;
const RUNTIME_VERSION: &str = "runtime-embedded-p1g";
const CONFIG_ENV: &str = "ACTINGLAB_CONFIG_PATH";
const RUNTIME_STATE_ROOT_ENV: &str = "ACTINGCOMMAND_RUNTIME_STATE_ROOT";
const SESSION_STATE_ENV: &str = "ACTINGLAB_SESSION_STATE_DIR";
const REQUIRE_SESSION_DAEMON_ENV: &str = "ACTINGLAB_REQUIRE_SESSION_DAEMON";
const TRUSTED_REMOTE_TOKEN_ENV: &str = "ACTINGLAB_TRUSTED_REMOTE_TOKEN";
const TRUSTED_REMOTE_CLIENT_CERT_ENV: &str = "ACTINGLAB_TRUSTED_REMOTE_CLIENT_CERT";
const ALLOW_PATH_ADB_FOR_MUMU_ENV: &str = "ACTINGCOMMAND_ALLOW_PATH_ADB_FOR_MUMU";
fn main() -> ExitCode {
    let json_default = !io::stdout().is_terminal();
    let result = run_cli(env::args().skip(1), json_default);
    let exit_code = result.exit_code();
    if result.print_json {
        println!("{}", result.envelope_json());
    } else {
        println!("{}", result.human_text());
    }
    ExitCode::from(exit_code as u8)
}

type CliOutcome<T> = Result<T, CliError>;

#[derive(Debug, Clone, Default)]
struct GlobalOptions {
    json: bool,
    run_root: Option<PathBuf>,
    instance: Option<String>,
    instances: Option<Vec<String>>,
    profile: Option<String>,
    resource_root: Option<PathBuf>,
    dry_run: bool,
    verbose: bool,
    quiet: bool,
    game: Option<String>,
    server: Option<String>,
    runtime_endpoint: Option<String>,
    capture_backend: Option<CaptureBackendChoice>,
    touch_backend: Option<TouchBackendChoice>,
    version: bool,
}

#[derive(Debug, Clone)]
struct Invocation {
    global: GlobalOptions,
    command: Vec<String>,
    args: Vec<String>,
    command_name: String,
}

fn run_cli<I>(args: I, json_default: bool) -> CliResult
where
    I: IntoIterator,
    I::Item: Into<String>,
{
    match parse_invocation(args, json_default).and_then(execute_invocation) {
        Ok((invocation, data, human)) => {
            CliResult::ok(invocation.command_name, data, invocation.global.json, human)
        }
        Err((command, print_json, err)) => CliResult::err(command, err, print_json),
    }
}

fn execute_invocation(
    invocation: Invocation,
) -> Result<(Invocation, Value, String), (String, bool, CliError)> {
    let command_name = invocation.command_name.clone();
    let print_json = invocation.global.json;
    let result = execute(&invocation).map(|data| {
        let human = human_summary(&invocation.command_name, &data);
        (invocation, data, human)
    });
    result.map_err(|err| (command_name, print_json, err))
}

fn execute(invocation: &Invocation) -> CliOutcome<Value> {
    match invocation.command.as_slice() {
        [cmd] if cmd == "help" => Ok(help_data()),
        [cmd] if cmd == "version" => Ok(version_data()),
        [cmd] if cmd == "paths" => run_paths(&invocation.global),
        [cmd] if cmd == "capabilities" => run_capabilities(&invocation.global),
        [cmd] if cmd == "doctor" => run_doctor(&invocation.global),
        [cmd] if cmd == "status" => run_status(&invocation.global, &invocation.args),
        [cmd] if cmd == "devices" => run_devices(&invocation.global, &invocation.args),
        [cmd] if cmd == "schema" => run_schema(&invocation.args),
        [cmd] if cmd == "list" => run_list(&invocation.global, &invocation.args),
        [cmd] if cmd == "touch-probe" => run_touch_probe(&invocation.global, &invocation.args),
        [cmd] if cmd == "tap" => run_direct_touch(&invocation.global, cmd, &invocation.args),
        [cmd] if cmd == "swipe" => run_direct_touch(&invocation.global, cmd, &invocation.args),
        [cmd] if cmd == "long-tap" => run_direct_touch(&invocation.global, cmd, &invocation.args),
        [cmd] if cmd == "key" => run_direct_input(&invocation.global, cmd, &invocation.args),
        [cmd] if cmd == "text" => run_direct_input(&invocation.global, cmd, &invocation.args),
        [cmd] if cmd == "capture" => run_capture(&invocation.global, &invocation.args),
        [cmd] if cmd == "detect" => env_detection::run_detect(&invocation.global, &invocation.args),
        [cmd] if cmd == "detect-page" => run_detect_page(&invocation.global, &invocation.args),
        [cmd] if cmd == "recognize" => run_recognize(&invocation.global, &invocation.args),
        [cmd] if cmd == "observe" => lab2_cli::run_observe(&invocation.global, &invocation.args),
        [cmd] if cmd == "do" => lab2_cli::run_do(&invocation.global, &invocation.args),
        [cmd] if cmd == "ensure" => lab2_cli::run_ensure(&invocation.global, &invocation.args),
        [cmd] if cmd == "wait" => lab2_cli::run_wait(&invocation.global, &invocation.args),
        [cmd] if cmd == "current-page" => run_current_page(&invocation.global, &invocation.args),
        [cmd] if cmd == "is-visible" => run_is_visible(&invocation.global, &invocation.args),
        [cmd] if cmd == "locate" => run_locate(&invocation.global, &invocation.args),
        [cmd] if cmd == "tap-target" => run_tap_target(&invocation.global, &invocation.args),
        [cmd] if cmd == "navigate" => run_navigate(&invocation.global, &invocation.args),
        [cmd] if cmd == "monitor" => run_monitor(&invocation.global, &invocation.args),
        [cmd] if cmd == "stream" => {
            runtime_stream_adapter::run_stream(&invocation.global, &invocation.args)
        }
        [cmd] if cmd == "record" => run_session_record(&invocation.global, &invocation.args),
        [cmd] if cmd == "explain" => run_explain_run(&invocation.args),
        [group, sub] if group == "config" => run_config(sub, &invocation.args),
        [group, sub] if group == "env" => {
            env_detection::run_env(sub, &invocation.global, &invocation.args)
        }
        [group, sub] if group == "lab" => run_lab(sub, &invocation.global, &invocation.args),
        [group, sub] if group == "package" => {
            run_package(sub, &invocation.global, &invocation.args)
        }
        [group, sub] if group == "operation" => {
            run_operation(sub, &invocation.global, &invocation.args)
        }
        [group, sub] if group == "control" => {
            run_control(sub, &invocation.global, &invocation.args)
        }
        [group, sub] if group == "scheduler" => run_scheduler(sub, &invocation.global),
        [group, sub] if group == "resource" => {
            run_resource(sub, &invocation.global, &invocation.args)
        }
        [group, sub] if group == "runtime" => {
            runtime_slice_cli::run(sub, &invocation.global, &invocation.args)
        }
        [group, sub] if group == "ledger" => run_ledger(sub, &invocation.global, &invocation.args),
        [group, sub] if group == "session" => {
            run_session(sub, &invocation.global, &invocation.args)
        }
        [group, sub] if group == "run" => {
            run_summary::dispatch(sub, &invocation.global, &invocation.args)
        }
        [group, sub] if group == "report" => run_report(sub, &invocation.global, &invocation.args),
        _ => Err(CliError::usage(format!(
            "unknown actinglab command: {}",
            invocation.command.join(" ")
        ))),
    }
}

use cli_result::human_summary;
fn run_ledger(sub: &str, _global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let _ = FlagArgs::parse(args)?;
    match sub {
        "show" | "events" | "receipts" | "diagnose" | "evidence" => Err(CliError::not_implemented(
            "local_ledger_retired",
            "local ledger queries are retired; use lab watch or lab receipt to query the Runtime global ledger",
        )),
        other => Err(CliError::usage(format!("unknown ledger command: {other}"))),
    }
}

#[cfg(test)]
#[allow(dead_code)]
#[path = "tests/legacy_local_ledger_reader.rs"]
mod legacy_local_ledger_reader;

fn run_monitor(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let _ = global;
    runtime_session_adapter::retired_authority("monitor", args)
}

fn stream_contract_json(
    max_frames: usize,
    interval: Duration,
    fresh_delay: Duration,
    require_fresh: bool,
    input_event_count: usize,
    dry_run: bool,
) -> Value {
    json!({
        "schema_version": "session.stream.v0.1",
        "status": "available",
        "stream_kind": "bounded_cli_frame_sequence",
        "frame_delivery": "json_array",
        "event_schema_version": "session.stream.event.v0.1",
        "event_fields": ["schema_version", "stream_id", "event_index", "type"],
        "input_relay": {
            "supported": true,
            "requested": input_event_count > 0,
            "event_count": input_event_count,
            "execution_model": if dry_run { "planned_only" } else { "per_request" },
            "long_lived_session": false,
            "max_events_per_request": 16,
            "supported_actions": ["tap", "swipe", "long-tap", "key", "text"],
            "requires_matching_lease_when_daemon_routed": true
        },
        "capture": {
            "require_fresh": require_fresh,
            "dry_run": dry_run,
            "interval_ms": interval.as_millis(),
            "fresh_delay_ms": fresh_delay.as_millis(),
            "requested_max_frames": max_frames,
            "max_frames_per_request": 60
        },
        "safety": {
            "session_layer_only_throat": true,
            "ui_must_not_directly_touch_adb_or_device": true,
            "trusted_remote_long_lived_stream": "reserved"
        }
    })
}

fn stream_events_json(stream_id: &str, frames: &[Value], input_relay: &Value) -> Vec<Value> {
    let input_status = input_relay
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let mut events = Vec::with_capacity(frames.len() + 3);
    events.push(json!({
        "schema_version": "session.stream.event.v0.1",
        "stream_id": stream_id,
        "event_index": events.len(),
        "type": "stream.started",
        "frame_count_planned": frames.len(),
        "input_relay_status": input_status
    }));
    let frame_event_base = events.len();
    events.extend(frames.iter().enumerate().map(|(offset, frame)| {
        json!({
            "schema_version": "session.stream.event.v0.1",
            "stream_id": stream_id,
            "event_index": frame_event_base + offset,
            "type": "stream.frame_sampled",
            "index": frame.get("index").cloned().unwrap_or(Value::Null),
            "captured": frame.get("captured").cloned().unwrap_or(Value::Bool(false)),
            "captured_at_unix_ms": frame.get("captured_at_unix_ms").cloned()
        })
    }));
    if input_status != "disabled" {
        events.push(json!({
            "schema_version": "session.stream.event.v0.1",
            "stream_id": stream_id,
            "event_index": events.len(),
            "type": "stream.input_relay",
            "status": input_status,
            "action_count": input_relay.get("action_count").cloned().unwrap_or(Value::Null)
        }));
    }
    events.push(json!({
        "schema_version": "session.stream.event.v0.1",
        "stream_id": stream_id,
        "event_index": events.len(),
        "type": "stream.completed",
        "frame_count": frames.len(),
        "input_relay_status": input_status
    }));
    events
}
fn run_lab(sub: &str, global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    match sub {
        "run" => {
            let flags = FlagArgs::parse(args)?;
            reject_legacy_session_routing(&flags)?;
            lab_run::run_lab_run(global, args)
        }
        "validate" => lab_run::run_lab_validate(args),
        "debug-package" | "watch" => runtime_debug::run_runtime_debug(sub, args),
        "export-evidence" | "replay-evidence" => runtime_debug::run_runtime_debug(sub, args),
        "start" => {
            require_runtime(global)?;
            let flags = FlagArgs::parse(args)?;
            let mode = flags
                .optional("--mode")
                .unwrap_or("passive_mirror".to_string());
            if !["passive_mirror", "scheduler_noop", "exclusive_drain"].contains(&mode.as_str()) {
                return Err(CliError::usage(format!("unsupported lab mode: {mode}")));
            }
            Err(CliError::not_implemented(
                "not_implemented",
                "lab start is reserved until Runtime lab sessions are connected",
            ))
        }
        "status" => run_session_status(global, args),
        "lease" | "preempt" | "release" => runtime_session_adapter::retired_authority(sub, args),
        "receipt" => lab2_cli::run_receipt(global, args),
        "evidence" => lab2_cli::run_evidence(global, args),
        "arbitrator" => lab2_cli::run_arbitrator(global, args),
        "vendor-stdio-selftest" => run_lab_vendor_stdio_selftest(args),
        _ => Err(CliError::usage(format!("unknown lab command: {sub}"))),
    }
}

fn run_lab_vendor_stdio_selftest(args: &[String]) -> CliOutcome<Value> {
    FlagArgs::parse(args)?.expect_positionals("lab vendor-stdio-selftest", 0)?;
    let capture =
        vendor_stdio_session_diagnostic().map_err(|err| CliError::device(err.to_string()))?;
    Ok(json!({
        "status": "ok",
        "stdout_captured": !capture.stdout.is_empty(),
        "stderr_captured": !capture.stderr.is_empty(),
        "captured": capture
    }))
}

fn run_package(sub: &str, global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    match sub {
        "validate" => package_cli::run_validate(global, &flags),
        "dry-run" => package_cli::run_offline(global, &flags),
        "inspect" => {
            let zip = flags.required_path("--zip")?;
            let validation = package_cli::validate_package(&zip, true)?;
            let mut payload = package_cli::serialize_response(&validation)?;
            attach_package_event(
                global,
                "package.inspect.ok",
                "package-inspect",
                &zip,
                &validation,
                &mut payload,
            )?;
            Ok(payload)
        }
        "run" => {
            reject_legacy_session_routing(&flags)?;
            let zip = flags.required_path("--zip")?;
            let out = flags.optional_path("--out");
            let validation = package_cli::validate_package(&zip, false)?;
            if global.instance.is_none() && global.game.is_none() {
                return Err(CliError::instance(
                    "package run requires --instance or --game/--server selector",
                ));
            }
            let result_zip = out
                .map(|out| create_package_blocked_result_zip(&out, &validation))
                .transpose()?;
            let mut details = package_cli::serialize_response(&validation)?;
            details["status"] = json!("blocked");
            details["blocked_by"] = json!(["lab_lease", "exclusive_drain"]);
            details["result_zip"] =
                json!(result_zip.as_ref().map(|path| path.display().to_string()));
            attach_package_event(
                global,
                "package.run.blocked",
                global.instance.as_deref().unwrap_or("package-run"),
                &zip,
                &validation,
                &mut details,
            )?;
            Err(CliError::safety_blocked(
                "lab_lease_required",
                format!(
                    "package run requires an exclusive_drain LabLease before executing navigation-only operations{}",
                    result_zip
                        .as_ref()
                        .map(|path| format!("; blocked result zip written to {}", path.display()))
                        .unwrap_or_default()
                ),
                &["lab_lease", "exclusive_drain"],
            )
            .with_details(details))
        }
        "build-task" => package_build::run_build_task(global, &flags),
        "build-pack" => package_build::run_build_pack(global, &flags),
        _ => Err(CliError::usage(format!("unknown package command: {sub}"))),
    }
}

fn attach_package_event(
    global: &GlobalOptions,
    event_type: &str,
    instance: &str,
    zip: &Path,
    validation: &PackageValidationResponse,
    payload: &mut Value,
) -> CliOutcome<()> {
    let req_id = IdIssuer::new().issue(IdKind::Req).value;
    let event = write_package_light_event(global, event_type, instance, &req_id, zip, validation)?;
    payload["req_id"] = json!(req_id);
    payload["ledger_event"] = event;
    Ok(())
}

fn write_package_light_event(
    _global: &GlobalOptions,
    event_type: &str,
    instance: &str,
    _req_id: &str,
    _zip: &Path,
    validation: &PackageValidationResponse,
) -> CliOutcome<Value> {
    Ok(json!({
        "written": false,
        "reason": "offline_resource_tooling_projection",
        "event_type": event_type,
        "instance": instance,
        "module": validation.module,
        "task_count": validation.task_count,
        "entry_count": validation.entry_count
    }))
}

fn run_operation(sub: &str, global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    match sub {
        "validate" | "inspect" | "explain" => {
            let dir = flags.required_path("--operation-dir")?;
            let report = validate_operation_dir(&dir)?;
            Ok(json!({
                "operation_dir": dir.display().to_string(),
                "status": "valid",
                "report": report,
                "mode": sub
            }))
        }
        "dry-run" => {
            require_runtime(global)?;
            Err(CliError::not_implemented(
                "not_implemented",
                "operation dry-run is reserved until Runtime operation adapter is connected",
            ))
        }
        "run" => {
            reject_legacy_session_routing(&flags)?;
            Err(CliError::safety_blocked(
                "lab_lease_required",
                "operation run requires Runtime scheduler admission",
                &["runtime_scheduler"],
            ))
        }
        _ => Err(CliError::usage(format!("unknown operation command: {sub}"))),
    }
}

fn run_control(sub: &str, _global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    match sub {
        "inspect" => Ok(json!({
            "control": flags.optional("--control"),
            "status": "reserved"
        })),
        "verify" => {
            let candidate = flags.required_path("--candidate")?;
            let candidate_id = flags.required("--candidate-id")?;
            validate_json_file(&candidate)?;
            Ok(json!({
                "candidate": candidate.display().to_string(),
                "candidate_id": candidate_id,
                "status": "validated",
                "click_executed": false
            }))
        }
        "probe-click" => {
            let effect = flags.optional("--effect").unwrap_or_default();
            if effect != "navigation_only" {
                return Err(CliError::safety_blocked(
                    "effect_not_navigation_only",
                    "control probe-click only allows effect navigation_only",
                    &["navigation_only"],
                ));
            }
            if flags.optional("--expect-before").is_none()
                || flags.optional("--expect-after").is_none()
            {
                return Err(CliError::safety_blocked(
                    "unresolved_coords",
                    "control probe-click requires expect-before and expect-after page guards",
                    &["expect_after", "page_guard"],
                ));
            }
            Err(CliError::safety_blocked(
                "lab_lease_required",
                "control probe-click requires an exclusive_drain LabLease",
                &["lab_lease", "exclusive_drain"],
            ))
        }
        "export" => Err(CliError::not_implemented(
            "not_implemented",
            "control export is reserved for stable-control promotion",
        )),
        "diff" => {
            let candidate = flags.required_path("--candidate")?;
            let stable = flags.required_path("--stable")?;
            let candidate_hash = file_sha256(&candidate)?;
            let stable_hash = file_sha256(&stable)?;
            Ok(json!({
                "candidate": candidate.display().to_string(),
                "stable": stable.display().to_string(),
                "same_hash": candidate_hash == stable_hash,
                "candidate_sha256": candidate_hash,
                "stable_sha256": stable_hash
            }))
        }
        _ => Err(CliError::usage(format!("unknown control command: {sub}"))),
    }
}

fn run_scheduler(sub: &str, _global: &GlobalOptions) -> CliOutcome<Value> {
    match sub {
        "status" | "pause" | "resume" | "start" | "stop" => Err(CliError::not_implemented(
            "scheduler_not_available",
            "Scheduler interface is reserved but not implemented yet.",
        )),
        _ => Err(CliError::usage(format!("unknown scheduler command: {sub}"))),
    }
}

fn run_session(sub: &str, global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    match sub {
        "status" => run_session_status(global, args),
        "bootstrap" => runtime_session_adapter::retired_authority(sub, args),
        "throat-policy" => run_session_throat_policy(global, args),
        "capture-policy" => run_session_capture_policy(global, args),
        "record-policy" => run_session_record_policy(global, args),
        "self-heal-policy" => run_session_self_heal_policy(global, args),
        "self-heal-plan" => runtime_session_adapter::retired_authority(sub, args),
        "phase-c-plan" => runtime_session_adapter::retired_authority(sub, args),
        "readiness" => runtime_session_adapter::retired_authority(sub, args),
        "connect-plan" => runtime_session_adapter::retired_authority(sub, args),
        "stream-plan" => runtime_session_adapter::retired_authority(sub, args),
        "queue" => runtime_session_adapter::retired_authority(sub, args),
        "command-check" => runtime_session_adapter::retired_authority(sub, args),
        "submit-plan" => runtime_session_adapter::retired_authority(sub, args),
        "validation-plan" => runtime_session_adapter::retired_authority(sub, args),
        "start" => runtime_session_adapter::retired_authority(sub, args),
        "stop" => runtime_session_adapter::retired_authority(sub, args),
        "cleanup" => runtime_session_adapter::retired_authority(sub, args),
        "daemon" => runtime_session_adapter::retired_authority(sub, args),
        "request" => runtime_session_adapter::retired_authority(sub, args),
        "contract" => run_session_contract(global, args),
        "api" => run_session_api(global, args),
        "transport" => run_session_transport(global, args),
        "journal" => runtime_session_adapter::retired_authority(sub, args),
        "events" => runtime_session_adapter::retired_authority(sub, args),
        "response" => runtime_session_adapter::retired_authority(sub, args),
        "request-state" => runtime_session_adapter::retired_authority(sub, args),
        "monitor-policy" => run_session_monitor_policy(global, args),
        "instance" => run_session_instance(global, args),
        "app" => run_session_app(global, args),
        "capture" => run_capture(global, args),
        "stream" => runtime_stream_adapter::run_stream(global, args),
        "recover" => run_session_recover(global, args),
        "lease" => runtime_session_adapter::retired_authority(sub, args),
        "record" => run_session_record(global, args),
        _ => Err(CliError::usage(format!("unknown session command: {sub}"))),
    }
}

fn run_session_monitor_policy(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    runtime_session_adapter::run_monitor_policy(global, args)
}

fn monitor_policy_monitor_args(raw_args: &[String], flags: &FlagArgs) -> CliOutcome<Vec<String>> {
    if flags.optional("--max-iterations").is_some() {
        return Err(CliError::usage(
            "session monitor-policy stores monitor --once arguments; do not use --max-iterations",
        ));
    }
    if flags.bool("--via-daemon") || flags.bool("--local") {
        return Err(CliError::usage(
            "session monitor-policy set does not store --via-daemon or --local",
        ));
    }
    if !flags.bool("--capture") && flags.optional("--scene").is_none() {
        return Err(CliError::usage(
            "session monitor-policy set requires --scene <png> or --capture",
        ));
    }
    let mut out = Vec::new();
    let mut index = 0usize;
    while index < raw_args.len() {
        let arg = &raw_args[index];
        if [
            "--interval-ms",
            "--state-dir",
            "--request-timeout-ms",
            "--lease-holder",
            "--holder",
            "--lease-id",
        ]
        .contains(&arg.as_str())
        {
            index += if index + 1 < raw_args.len() && !raw_args[index + 1].starts_with("--") {
                2
            } else {
                1
            };
            continue;
        }
        if ["--recover", "--via-daemon", "--local", "--max-iterations"].contains(&arg.as_str()) {
            if arg == "--recover" {
                index += 1;
                continue;
            }
            return Err(CliError::usage(format!(
                "session monitor-policy set cannot store {arg}"
            )));
        }
        out.push(arg.clone());
        index += 1;
    }
    Ok(out)
}

fn run_session_status(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    runtime_session_adapter::run_status(global, args)
}

fn session_instance_registry_contract(config: &UserConfig) -> CliOutcome<Value> {
    let instances = config
        .instances
        .iter()
        .map(|(id, instance)| session_instance_registry_entry(id, instance))
        .collect::<CliOutcome<Vec<_>>>()?;
    Ok(json!({
        "schema_version": "session.instance_registry.v0.1",
        "source": "user_config",
        "available": true,
        "count": instances.len(),
        "required_fields": ["serial", "game", "server"],
        "recommended_fields": ["package", "adb_path", "capture_backend", "touch_backend"],
        "capture_backends": ["auto", "adb", "droidcast_raw", "nemu_ipc", "auto-fastest"],
        "touch_backends": ["auto", "auto-fastest", "maatouch", "minitouch", "adb_shell_input"],
        "instances": instances
    }))
}

fn session_instance_registry_entry(id: &str, instance: &InstanceConfig) -> CliOutcome<Value> {
    let effective_capture_backend = match instance.capture_backend.as_deref() {
        Some(value) => CaptureBackendChoice::parse(value)
            .map_err(|err| {
                CliError::usage(format!(
                    "invalid instance.{id}.capture_backend '{value}': {err}"
                ))
            })?
            .as_str()
            .to_string(),
        None => CaptureBackendChoice::Auto.as_str().to_string(),
    };
    let effective_touch_backend = match instance.touch_backend.as_deref() {
        Some(value) => TouchBackendChoice::parse(value)
            .map_err(|err| {
                CliError::usage(format!(
                    "invalid instance.{id}.touch_backend '{value}': {err}"
                ))
            })?
            .as_str()
            .to_string(),
        None => TouchBackendChoice::Auto.as_str().to_string(),
    };
    let missing_required_fields = instance_missing_required_fields(instance);
    let missing_recommended_fields = instance_missing_recommended_fields(instance);
    Ok(json!({
        "id": id,
        "serial": instance.serial,
        "game": instance.game,
        "server": instance.server,
        "package": instance.package,
        "adb_path": instance.adb_path,
        "capture_backend": instance.capture_backend,
        "touch_backend": instance.touch_backend,
        "configured": {
            "serial": instance.serial.is_some(),
            "game": instance.game.is_some(),
            "server": instance.server.is_some(),
            "package": instance.package.is_some(),
            "adb_path": instance.adb_path.is_some(),
            "capture_backend": instance.capture_backend.is_some(),
            "touch_backend": instance.touch_backend.is_some()
        },
        "effective": {
            "capture_backend": effective_capture_backend,
            "touch_backend": effective_touch_backend,
            "adb_path": instance.adb_path,
            "adb_path_source": if instance.adb_path.is_some() { "instance_config" } else { "resolver_default" }
        },
        "validation": {
            "ready_for_device_control": missing_required_fields.is_empty(),
            "missing_required_fields": missing_required_fields,
            "missing_recommended_fields": missing_recommended_fields
        }
    }))
}

fn instance_missing_required_fields(instance: &InstanceConfig) -> Vec<&'static str> {
    [
        ("serial", instance.serial.is_none()),
        ("game", instance.game.is_none()),
        ("server", instance.server.is_none()),
    ]
    .into_iter()
    .filter_map(|(field, missing)| missing.then_some(field))
    .collect()
}

fn instance_missing_recommended_fields(instance: &InstanceConfig) -> Vec<&'static str> {
    [
        ("package", instance.package.is_none()),
        ("adb_path", instance.adb_path.is_none()),
        ("capture_backend", instance.capture_backend.is_none()),
        ("touch_backend", instance.touch_backend.is_none()),
    ]
    .into_iter()
    .filter_map(|(field, missing)| missing.then_some(field))
    .collect()
}

fn run_session_instance(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let action = args
        .first()
        .map(String::as_str)
        .ok_or_else(|| CliError::usage("session instance requires list|registry|app"))?;
    if action == "app" {
        if args.get(1).is_none() {
            return Err(CliError::usage(
                "session instance app requires launch|stop|force-stop|restart",
            ));
        }
        return run_session_app(global, &args[1..]);
    }
    let flags = FlagArgs::parse(&args[1..])?;
    reject_legacy_session_routing(&flags)?;
    let config = read_user_config()?;
    match action {
        "list" => Ok(json!({
            "instances": config.instances.iter().map(|(id, instance)| json!({
                "id": id,
                "serial": instance.serial,
                "game": instance.game,
                "server": instance.server,
                "package": instance.package,
                "adb_path": instance.adb_path,
                "capture_backend": instance.capture_backend
            })).collect::<Vec<_>>()
        })),
        "registry" => session_instance_registry_contract(&config),
        "connect" | "health" | "keep-alive" | "reconnect" => Err(CliError::not_implemented(
            "actinglab_device_authority_retired",
            format!(
                "session instance {action} directly owned device state in ActingLab and is retired; use Runtime-backed status or control APIs"
            ),
        )),
        other => Err(CliError::usage(format!(
            "unknown session instance action: {other}"
        ))),
    }
}

fn run_session_app(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let action = args
        .first()
        .map(String::as_str)
        .ok_or_else(|| CliError::usage("session app requires launch|stop|force-stop|restart"))?;
    let flags = FlagArgs::parse(&args[1..])?;
    reject_legacy_session_routing(&flags)?;
    if flags.optional("--package").is_some() {
        return Err(CliError::usage(
            "--package is not accepted by ActingLab; application identity is owned by Runtime configuration",
        ));
    }
    let config = read_user_config()?;
    let instance_id = resolve_instance_id_for_flags(global, &config, &flags)?;
    let action = match action {
        "launch" => ApplicationLifecycleAction::Launch,
        "stop" | "force-stop" => ApplicationLifecycleAction::Stop,
        "restart" => ApplicationLifecycleAction::Restart,
        other => Err(CliError::usage(format!(
            "unknown session app action: {other}"
        )))?,
    };
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        runtime_state_root()?,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .map_err(runtime_slice_cli::map_runtime_error)?;
    let output = client
        .control_application(&instance_id, action)
        .map_err(runtime_slice_cli::map_runtime_error)?;
    serde_json::to_value(output)
        .map_err(|error| CliError::usage(format!("failed to serialize Runtime receipt: {error}")))
}

fn run_resource(sub: &str, global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let repo = flags.required_path("--repo")?;
    let resource_root = resolve_resource_root(&repo);
    match sub {
        "validate" => {
            let mut validation = validate_resource_repo(&resource_root.root)?;
            if let Some(object) = validation.as_object_mut() {
                object.insert(
                    "input".to_string(),
                    Value::String(resource_root.input.display().to_string()),
                );
                object.insert(
                    "resource_root".to_string(),
                    Value::String(resource_root.root.display().to_string()),
                );
                object.insert(
                    "resource_layout".to_string(),
                    Value::String(resource_root.layout.to_string()),
                );
            }
            Ok(validation)
        }
        "convert" => resource_convert::run_resource_convert(global, &flags, &resource_root),
        "compile-maa" => maa_task_graph::run_resource_maa_task_compile(&flags, &resource_root),
        "import-alas" | "drift-alas" => {
            let alas_root = flags.required_path("--alas-root")?;
            Ok(json!({
                "repo": repo.display().to_string(),
                "resource_root": resource_root.root.display().to_string(),
                "resource_layout": resource_root.layout,
                "alas_root": alas_root.display().to_string(),
                "status": "reserved",
                "command": sub
            }))
        }
        "check-release" => Ok(json!({
            "repo": repo.display().to_string(),
            "resource_root": resource_root.root.display().to_string(),
            "resource_layout": resource_root.layout,
            "exists": repo.is_dir(),
            "status": if repo.is_dir() { "checked" } else { "missing" }
        })),
        _ => Err(CliError::usage(format!("unknown resource command: {sub}"))),
    }
}

fn run_report(sub: &str, _global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    match sub {
        "export" => {
            let flags = FlagArgs::parse(args)?;
            if !flags.bool("--last-error") {
                return Err(CliError::usage("report export requires --last-error"));
            }
            let out = flags.required_path("--out")?;
            let report =
                create_error_report_zip(&out, "last-error", "last-error report placeholder")?;
            Ok(json!({
                "report": report.display().to_string()
            }))
        }
        _ => Err(CliError::usage(format!("unknown report command: {sub}"))),
    }
}

fn run_explain_run(args: &[String]) -> CliOutcome<Value> {
    let run_id = args
        .first()
        .ok_or_else(|| CliError::usage("explain requires <run-id>"))?;
    Ok(json!({
        "run_id": run_id,
        "status": "reserved"
    }))
}

fn require_runtime(global: &GlobalOptions) -> CliOutcome<Value> {
    let config = read_user_config()?;
    let endpoint = effective_runtime_endpoint(global, &config)
        .ok_or_else(|| CliError::runtime_not_running("runtime endpoint is not configured"))?;
    let policy = runtime_endpoint_policy(&endpoint)?;
    if !runtime_tcp_available(&endpoint) {
        return Err(CliError::runtime_not_running(format!(
            "Runtime is not reachable at {endpoint}"
        )));
    }
    Ok(json!({
        "endpoint": endpoint,
        "connection": "tcp",
        "policy": runtime_endpoint_policy_json(&policy)
    }))
}

fn effective_adb_path_for_instance(
    config: &UserConfig,
    instance: Option<&InstanceConfig>,
) -> CliOutcome<actingcommand_device::ResolvedAdbPath> {
    let configured = instance
        .and_then(|instance| instance.adb_path.as_deref())
        .or(config.adb_path.as_deref());
    resolve_adb_path(configured).map_err(|err| CliError::device(err.to_string()))
}

fn enforce_path_adb_target_boundary(
    resolved: &actingcommand_device::ResolvedAdbPath,
    instance: Option<&InstanceConfig>,
    capture_backend: CaptureBackendChoice,
) -> CliOutcome<()> {
    if resolved.source != AdbPathSource::PathBaseline
        || !is_mumu_capture_target(instance, capture_backend)
    {
        return Ok(());
    }
    if env_flag(ALLOW_PATH_ADB_FOR_MUMU_ENV) {
        return Ok(());
    }
    Err(CliError::device(format!(
        "PATH adb baseline is not allowed for MuMu/Nemu IPC targets without {ALLOW_PATH_ADB_FOR_MUMU_ENV}=1; configure ACTINGCOMMAND_NEMU_FOLDER, ACTINGCOMMAND_ADB_PATH, or instance adb_path"
    )))
}

fn is_mumu_capture_target(
    instance: Option<&InstanceConfig>,
    capture_backend: CaptureBackendChoice,
) -> bool {
    capture_backend == CaptureBackendChoice::NemuIpc
        || instance
            .and_then(|instance| instance.capture_backend.as_deref())
            .is_some_and(|backend| backend.eq_ignore_ascii_case("nemu_ipc"))
}

fn env_flag(name: &str) -> bool {
    env::var(name).ok().is_some_and(|value| {
        matches!(
            value.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn resolved_adb_json(config: &UserConfig) -> Value {
    resolved_adb_json_from(resolve_adb_path(config.adb_path.as_deref()))
}

fn resolved_adb_json_from(
    resolution: actingcommand_device::DeviceResult<actingcommand_device::ResolvedAdbPath>,
) -> Value {
    match resolution {
        Ok(resolved) => json!({
            "ok": true,
            "path": resolved.path,
            "source": resolved.source.as_str(),
            "warning": resolved.warning
        }),
        Err(err) => json!({
            "ok": false,
            "error": err.to_string(),
            "required_env": "ACTINGCOMMAND_ADB_PATH",
            "mumu_env": "ACTINGCOMMAND_NEMU_FOLDER"
        }),
    }
}

fn effective_runtime_endpoint(global: &GlobalOptions, config: &UserConfig) -> Option<String> {
    global
        .runtime_endpoint
        .clone()
        .or_else(|| config.runtime_endpoint.clone())
}

fn effective_resource_root(global: &GlobalOptions, config: &UserConfig) -> Option<PathBuf> {
    global
        .resource_root
        .clone()
        .or_else(|| config.resource_root.as_ref().map(PathBuf::from))
        .map(|path| resolve_resource_root(&path).root)
}

fn effective_run_root(global: &GlobalOptions, config: &UserConfig) -> Option<PathBuf> {
    global
        .run_root
        .clone()
        .or_else(|| config.run_root.as_ref().map(PathBuf::from))
}

#[derive(Debug, Clone)]
struct ResolvedResourceRoot {
    input: PathBuf,
    root: PathBuf,
    layout: &'static str,
}

fn resolve_resource_root(input: &Path) -> ResolvedResourceRoot {
    if looks_like_resource_root(input) {
        return ResolvedResourceRoot {
            input: input.to_path_buf(),
            root: input.to_path_buf(),
            layout: "direct",
        };
    }
    let ours = input.join("ours");
    if looks_like_resource_root(&ours) {
        return ResolvedResourceRoot {
            input: input.to_path_buf(),
            root: ours,
            layout: "repo_ours",
        };
    }
    ResolvedResourceRoot {
        input: input.to_path_buf(),
        root: input.to_path_buf(),
        layout: "unresolved",
    }
}

fn looks_like_resource_root(path: &Path) -> bool {
    path.join("operations").is_dir()
        && (path.join("recognition").is_dir() || path.join("navigation").is_dir())
}

fn create_package_blocked_result_zip(
    out: &Path,
    validation: &PackageValidationResponse,
) -> CliOutcome<PathBuf> {
    let target = if out.extension().and_then(|ext| ext.to_str()) == Some("zip") {
        out.to_path_buf()
    } else {
        out.join(format!("{}.result.zip", validation.module))
    };
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            CliError::package_invalid(format!("failed to create {}: {err}", parent.display()))
        })?;
    }
    let file = File::create(&target).map_err(|err| {
        CliError::package_invalid(format!("failed to create {}: {err}", target.display()))
    })?;
    let mut zip = ZipWriter::new(file);
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let prefix = format!("{}.result", validation.module);
    zip.add_directory(format!("{prefix}/screenshots/"), options)
        .map_err(zip_write_error)?;
    zip.start_file(format!("{prefix}/logs/summary.json"), options)
        .map_err(zip_write_error)?;
    zip.write_all(
        serde_json::to_string_pretty(&json!({
            "ok": false,
            "blocked_by": ["lab_lease", "exclusive_drain"],
            "module": validation.module
        }))
        .map_err(|err| CliError::package_invalid(format!("failed to serialize summary: {err}")))?
        .as_bytes(),
    )
    .map_err(zip_io_error)?;
    zip.start_file(format!("{prefix}/logs/result.md"), options)
        .map_err(zip_write_error)?;
    zip.write_all(b"Package run was blocked before execution because no exclusive_drain LabLease was present.\n")
        .map_err(zip_io_error)?;
    zip.start_file(format!("{prefix}/logs/events.jsonl"), options)
        .map_err(zip_write_error)?;
    zip.write_all(b"{\"event\":\"blocked\",\"reason\":\"lab_lease_required\"}\n")
        .map_err(zip_io_error)?;
    zip.start_file(format!("{prefix}/logs/command.txt"), options)
        .map_err(zip_write_error)?;
    zip.write_all(b"actinglab package run\n")
        .map_err(zip_io_error)?;
    zip.start_file(format!("{prefix}/logs/validation.json"), options)
        .map_err(zip_write_error)?;
    zip.write_all(
        serde_json::to_string_pretty(validation)
            .map_err(|err| {
                CliError::package_invalid(format!("failed to serialize validation: {err}"))
            })?
            .as_bytes(),
    )
    .map_err(zip_io_error)?;
    zip.start_file(format!("{prefix}/logs/manifest.resolved.json"), options)
        .map_err(zip_write_error)?;
    zip.write_all(
        serde_json::to_string_pretty(&validation.manifest)
            .map_err(|err| {
                CliError::package_invalid(format!("failed to serialize manifest: {err}"))
            })?
            .as_bytes(),
    )
    .map_err(zip_io_error)?;
    zip.finish().map_err(zip_write_error)?;
    Ok(target)
}

fn create_error_report_zip(out: &Path, run_id: &str, message: &str) -> CliOutcome<PathBuf> {
    let target = if out.extension().and_then(|ext| ext.to_str()) == Some("zip") {
        out.to_path_buf()
    } else {
        out.join(format!("error-report-{run_id}.zip"))
    };
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            CliError::usage(format!("failed to create {}: {err}", parent.display()))
        })?;
    }
    let file = File::create(&target)
        .map_err(|err| CliError::usage(format!("failed to create {}: {err}", target.display())))?;
    let mut zip = ZipWriter::new(file);
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zip.add_directory("error-report/screenshots/", options)
        .map_err(zip_write_error)?;
    zip.start_file("error-report/logs/summary.json", options)
        .map_err(zip_write_error)?;
    zip.write_all(
        serde_json::to_string_pretty(&json!({"run_id": run_id, "message": message}))
            .map_err(|err| CliError::usage(format!("failed to serialize report: {err}")))?
            .as_bytes(),
    )
    .map_err(zip_io_error)?;
    zip.start_file("error-report/logs/result.md", options)
        .map_err(zip_write_error)?;
    zip.write_all(message.as_bytes()).map_err(zip_io_error)?;
    zip.start_file("error-report/logs/events.jsonl", options)
        .map_err(zip_write_error)?;
    zip.write_all(b"{\"event\":\"report_exported\"}\n")
        .map_err(zip_io_error)?;
    zip.finish().map_err(zip_write_error)?;
    Ok(target)
}

fn validate_operation_dir(dir: &Path) -> CliOutcome<Value> {
    if !dir.is_dir() {
        return Err(CliError::usage(format!(
            "operation dir does not exist: {}",
            dir.display()
        )));
    }
    let task = dir.join("task.json");
    if !task.is_file() {
        return Err(CliError::usage(format!("missing {}", task.display())));
    }
    let task_json = fs::read_to_string(&task)
        .map_err(|err| CliError::usage(format!("failed to read {}: {err}", task.display())))?;
    let value: Value = serde_json::from_str(&task_json)
        .map_err(|err| CliError::usage(format!("failed to parse {}: {err}", task.display())))?;
    let unresolved = contains_string_value(&value, "unresolved_coords");
    if unresolved {
        return Err(CliError::safety_blocked(
            "unresolved_coords",
            "operation contains unresolved_coords and cannot be executed",
            &["unresolved_coords"],
        ));
    }
    Ok(json!({
        "task_json": task.display().to_string(),
        "unresolved_coords": false
    }))
}

fn validate_resource_repo(repo: &Path) -> CliOutcome<Value> {
    if !repo.is_dir() {
        return Err(CliError::usage(format!(
            "resource repo does not exist: {}",
            repo.display()
        )));
    }
    let recognition_dir = repo.join("recognition");
    let packs = find_files(repo, |path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".pack.json"))
    })?;
    let pages = find_files(repo, |path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".pages.json"))
    })?;
    Ok(json!({
        "repo": repo.display().to_string(),
        "recognition_dir_exists": recognition_dir.is_dir(),
        "pack_count": packs.len(),
        "pages_count": pages.len(),
        "packs": packs.iter().map(|path| path_string(path)).collect::<Vec<_>>(),
        "pages": pages.iter().map(|path| path_string(path)).collect::<Vec<_>>()
    }))
}

fn validate_json_file(path: &Path) -> CliOutcome<Value> {
    let text = fs::read_to_string(path)
        .map_err(|err| CliError::usage(format!("failed to read {}: {err}", path.display())))?;
    serde_json::from_str(&text)
        .map_err(|err| CliError::usage(format!("failed to parse {}: {err}", path.display())))
}

fn list_runs(run_root: &Path) -> CliOutcome<Value> {
    let mut runs = Vec::new();
    let mut warnings = Vec::new();
    if run_root.is_dir() {
        for entry in fs::read_dir(run_root).map_err(|err| {
            CliError::usage(format!("failed to list {}: {err}", run_root.display()))
        })? {
            match entry {
                Ok(entry) => {
                    if entry.path().is_dir() {
                        runs.push(entry.file_name().to_string_lossy().to_string());
                    }
                }
                Err(err) => warnings.push(format!("failed to read run directory entry: {err}")),
            }
        }
    }
    Ok(json!({
        "run_root": run_root.display().to_string(),
        "runs": runs,
        "warnings": warnings
    }))
}

fn list_resource_kind(root: &Path, kind: &str) -> CliOutcome<Value> {
    let suffix = match kind {
        "targets" => ".pack.json",
        "pages" => ".pages.json",
        "tasks" | "bundles" => "task.json",
        "controls" => ".controls.json",
        other => return Err(CliError::usage(format!("unknown list kind: {other}"))),
    };
    let files = find_files(root, |path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(suffix))
    })?;
    Ok(json!({
        "kind": kind,
        "root": root.display().to_string(),
        "files": files.iter().map(|path| path_string(path)).collect::<Vec<_>>()
    }))
}

fn find_files<F>(root: &Path, predicate: F) -> CliOutcome<Vec<PathBuf>>
where
    F: Fn(&Path) -> bool,
{
    let mut out = Vec::new();
    find_files_inner(root, &predicate, &mut out)?;
    Ok(out)
}

fn find_files_inner<F>(root: &Path, predicate: &F, out: &mut Vec<PathBuf>) -> CliOutcome<()>
where
    F: Fn(&Path) -> bool,
{
    if !root.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(root)
        .map_err(|err| CliError::usage(format!("failed to list {}: {err}", root.display())))?
    {
        let entry = entry
            .map_err(|err| CliError::usage(format!("failed to read directory entry: {err}")))?;
        let path = entry.path();
        if path.is_dir() {
            find_files_inner(&path, predicate, out)?;
        } else if predicate(&path) {
            out.push(path);
        }
    }
    Ok(())
}

fn scene_from_frame(frame: &Frame) -> CliOutcome<Scene> {
    let pixel_format = match frame.pixel_format {
        PixelFormat::Rgb8 => ScenePixelFormat::Rgb8,
        PixelFormat::Rgba8 => ScenePixelFormat::Rgba8,
    };
    Scene::from_pixels(frame.width, frame.height, &frame.pixels, pixel_format)
        .map_err(|err| CliError::device(err.to_string()))
}

fn match_metric_name(metric: MatchMetric) -> &'static str {
    match metric {
        MatchMetric::CrossCorrelationNormalized => "ccorr_normed",
        MatchMetric::CorrelationCoefficientNormalized => "ccoeff_normed",
    }
}

fn contains_string_value(value: &Value, needle: &str) -> bool {
    match value {
        Value::String(text) => text.contains(needle),
        Value::Array(items) => items.iter().any(|item| contains_string_value(item, needle)),
        Value::Object(map) => map
            .iter()
            .any(|(key, value)| key.contains(needle) || contains_string_value(value, needle)),
        _ => false,
    }
}

fn exit_code_table() -> Value {
    json!([
        {"exit_code": 0, "meaning": "ok"},
        {"exit_code": 2, "meaning": "usage_or_validation"},
        {"exit_code": 3, "meaning": "safety_blocked"},
        {"exit_code": 4, "meaning": "device_or_instance"},
        {"exit_code": 5, "meaning": "runtime_not_running"},
        {"exit_code": 6, "meaning": "not_implemented_or_scheduler_not_available"}
    ])
}

fn path_string(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests;
