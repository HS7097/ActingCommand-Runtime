// SPDX-License-Identifier: AGPL-3.0-only

#![allow(clippy::result_large_err)]

use actingcommand_contract::{
    CLI_SCHEMA_VERSION, LabError as CliError, LabErrorClass as ErrorKind,
};
#[cfg(test)]
use actingcommand_device::CaptureBackendName;
#[cfg(test)]
use actingcommand_device::{AdbPathSource, Frame, PixelFormat};
use actingcommand_device::{
    CaptureBackendChoice, InputBackend, TouchBackendChoice, combine_operation_and_close,
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
use actingcommand_recognition::{MatchMetric, Scene};
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
use lab_package_control::{
    attach_package_event, run_control, run_lab, run_operation, run_package, run_scheduler,
};
#[rustfmt::skip] use flag_values::{
    parse_match_metric_flag, parse_optional_duration_ms, parse_optional_string_value,
    parse_optional_unit_f64, parse_optional_usize, parse_record_build_resolution,
    parse_record_duration_ms, parse_session_record_region, parse_session_record_swipe_rects,
    parse_touch_backend_override, record_amend_step_id, record_candidates_step_id,
    required_non_empty_flag, session_record_drift_diagnostics_path, split_csv,
    stream_check_requested, stream_input_relay_action, target_argument, parse_session_record_candidate_index,
};
use instance_resolution::{resolve_instance_id, resolve_instance_id_for_flags};
use resource_runtime_support::{
    ResolvedResourceRoot, create_error_report_zip, create_package_blocked_result_zip,
    effective_adb_path_for_instance, effective_resource_root, effective_run_root,
    effective_runtime_endpoint, enforce_path_adb_target_boundary, exit_code_table, find_files,
    list_resource_kind, list_runs, match_metric_name, path_string, require_runtime,
    resolve_resource_root, resolved_adb_json, resolved_adb_json_from, run_explain_run, run_report,
    run_resource, scene_from_frame, validate_json_file, validate_operation_dir,
};
#[cfg(test)]
use runtime_endpoint::RuntimeEndpointChannel;
#[cfg(test)]
use runtime_endpoint::runtime_endpoint_policy;
use safe_file_stem::safe_file_stem;
use serde_json::{Value, json};
use session_management::{
    monitor_policy_monitor_args, run_session_app, run_session_instance, run_session_monitor_policy,
    run_session_status,
};
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
#[cfg(test)]
use std::fs::{self, File};
#[cfg(test)]
use std::io::Write;
use std::io::{self, IsTerminal};
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
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
#[cfg(test)]
use zip::{ZipWriter, write::FileOptions};

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
mod lab_package_control;
mod lab_run;
mod maa_task_graph;
mod package_build;
mod package_cli;
pub mod project_interface;
mod readonly_cli;
pub mod recovery_exec;
mod resource_authoring;
mod resource_convert;
mod resource_runtime_support;
mod run_summary;
mod runtime_capture_backend;
mod runtime_debug;
mod runtime_endpoint;
mod runtime_input_backend;
mod runtime_session_adapter;
mod runtime_slice_cli;
mod runtime_stream_adapter;
mod safe_file_stem;
mod session_management;
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

#[cfg(test)]
mod tests;
