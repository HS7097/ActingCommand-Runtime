// SPDX-License-Identifier: AGPL-3.0-only

#![allow(clippy::result_large_err)]

use actingcommand_contract::{
    ApplicationLifecycleAction, CLI_SCHEMA_VERSION, EventActor, EventSource, LabError as CliError,
    LabErrorClass as ErrorKind,
};
use actingcommand_device::{
    AdbPathSource, CaptureBackendChoice, CaptureBackendName, Frame, InputBackend, PixelFormat,
    TouchBackendChoice, combine_operation_and_close, resolve_adb_path,
    vendor_stdio_session_diagnostic,
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
use actingcommand_recognition::{MatchMetric, Rect as RecognitionRect, Scene, ScenePixelFormat};
use actingcommand_recognition_pack::{
    PackRect, RecognitionEvaluator, TargetEvaluation, TargetKind, load_pack_from_json_str,
};
use actingcommand_resource_tooling::{canonical_game, canonical_locale, canonical_server};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
#[cfg(test)]
use cli_result::CliErrorExitCode;
use cli_result::CliResult;
#[cfg(test)]
use commands::env_needs_detection_json;
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
use commands::{attach_env_resolved, record_env_needs_detection, record_env_resolved};
use commands::{command_capabilities, run_capabilities};
use commands::{ensure_path_within, read_json_file, write_json_file_atomic};
use commands::{finish_semantic_result_with_ledger, semantic_ledger_context};
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
    env_var_non_empty, runtime_endpoint_check, runtime_endpoint_policy,
    runtime_endpoint_policy_json, runtime_tcp_available,
};
use safe_file_stem::safe_file_stem;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
#[cfg(test)]
use sha2::{Digest, Sha256};
use sha256::{file_sha256, hex_sha256};
use state_roots::{app_state_root, runtime_state_root, session_state_dir_from_flags};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
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
use std::time::{Duration, Instant};
use unix_time::current_unix_ms;
use user_config_keys::{config_get, config_set};
use user_config_store::{config_path, read_user_config, write_user_config};
use zip::{ZipWriter, write::FileOptions};
use zip_error::{zip_io_error, zip_write_error};

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
const SESSION_LEASE_STALE_MS: u64 = 30_000;
const SESSION_DAEMON_REQUEST_TIMEOUT_MS: u64 = 10_000;
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionRecordContext {
    schema_version: String,
    record_id: String,
    task_id: String,
    instance: String,
    status: String,
    #[serde(default)]
    holder: Option<String>,
    #[serde(default)]
    lease_id: Option<String>,
    started_at_unix_ms: u64,
    updated_at_unix_ms: u64,
    #[serde(default)]
    steps: Vec<SessionRecordStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionRecordStep {
    schema_version: String,
    step_id: String,
    created_at_unix_ms: u64,
    updated_at_unix_ms: u64,
    #[serde(flatten)]
    data: SessionRecordStepData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum SessionRecordStepData {
    Anchor {
        id: String,
        region: SessionRecordRegion,
        color_check: bool,
        #[serde(default)]
        threshold: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frame_provenance: Option<Box<SessionRecordFrameProvenance>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        artifact: Option<Box<SessionRecordAnchorArtifact>>,
        evaluation: Box<SessionRecordStepEvaluation>,
    },
    ColorProbe {
        id: String,
        region: SessionRecordRegion,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected: Option<[u8; 3]>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frame_provenance: Option<Box<SessionRecordFrameProvenance>>,
        evaluation: Box<SessionRecordStepEvaluation>,
    },
    VerifyTemplate {
        id: String,
        region: SessionRecordRegion,
        #[serde(default)]
        threshold: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frame_provenance: Option<Box<SessionRecordFrameProvenance>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        artifact: Option<Box<SessionRecordAnchorArtifact>>,
        evaluation: Box<SessionRecordStepEvaluation>,
    },
    Operation {
        from: String,
        #[serde(default)]
        to: Option<String>,
        click: SessionRecordClick,
        destructive: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
enum SessionRecordRegion {
    Auto,
    Rect { rect: SessionRecordRect },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionRecordRect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SessionRecordClick {
    Coord {
        x: i32,
        y: i32,
    },
    Target {
        target: String,
    },
    Swipe {
        from: SessionRecordRect,
        to: SessionRecordRect,
        duration_ms: u64,
    },
    LongPress {
        x: i32,
        y: i32,
        duration_ms: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionRecordStepEvaluation {
    status: String,
    reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    auto_region: Option<SessionRecordAutoRegionSelection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    backtest: Option<SessionRecordAnchorBacktest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    contrast_backtest: Option<SessionRecordAnchorContrastBacktest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionRecordAutoRegionSelection {
    strategy: String,
    selected_reason: String,
    selected: SessionRecordRect,
    candidates: Vec<SessionRecordAutoRegionCandidate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionRecordAutoRegionCandidate {
    region: SessionRecordRect,
    luma_variance: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    contrast_score: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    contrast_passed: Option<bool>,
    selected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionRecordAnchorBacktest {
    source: String,
    metric: String,
    region: SessionRecordRect,
    x: i32,
    y: i32,
    raw_score: f32,
    score: f32,
    threshold: f32,
    passed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionRecordAnchorContrastBacktest {
    source: String,
    path: String,
    sha256: String,
    width: u32,
    height: u32,
    metric: String,
    region: SessionRecordRect,
    x: i32,
    y: i32,
    raw_score: f32,
    score: f32,
    threshold: f32,
    passed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionRecordFrameProvenance {
    source: String,
    path: String,
    sha256: String,
    width: u32,
    height: u32,
    recorded_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capture_backend: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    freshness: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    capture_attempts: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionRecordAnchorArtifact {
    kind: String,
    path: String,
    sha256: String,
    width: u32,
    height: u32,
    region: SessionRecordRect,
}

struct MaterializedAnchorArtifact {
    region: SessionRecordRegion,
    frame_provenance: SessionRecordFrameProvenance,
    artifact: SessionRecordAnchorArtifact,
    evaluation: SessionRecordStepEvaluation,
}

struct SessionRecordAnchorRegionResolution {
    rect: SessionRecordRect,
    auto_region: Option<SessionRecordAutoRegionSelection>,
}

struct SessionRecordSourceFrame {
    frame: Frame,
    png: Vec<u8>,
    source: String,
    path: PathBuf,
    recorded_at_unix_ms: u64,
    capture_backend: Option<String>,
    freshness: Option<Value>,
    capture_attempts: Vec<Value>,
}

struct SessionRecordContrastFrame {
    frame: Frame,
    path: PathBuf,
    sha256: String,
}

struct SessionRecordStepContext<'a> {
    global: &'a GlobalOptions,
    config: &'a UserConfig,
    record: &'a SessionRecordContext,
    state_dir: &'a Path,
}

struct SessionRecordAmendContext {
    record_id: String,
    state_dir: PathBuf,
}

struct SessionRecordAnchorAmendTarget<'a> {
    id: &'a mut String,
    region: &'a mut SessionRecordRegion,
    color_check: &'a mut bool,
    threshold: &'a mut Option<f64>,
    frame_provenance: &'a mut Option<Box<SessionRecordFrameProvenance>>,
    artifact: &'a mut Option<Box<SessionRecordAnchorArtifact>>,
    evaluation: &'a mut SessionRecordStepEvaluation,
}

struct SessionRecordColorProbeAmendTarget<'a> {
    id: &'a mut String,
    region: &'a mut SessionRecordRegion,
    expected: &'a mut Option<[u8; 3]>,
    frame_provenance: &'a mut Option<Box<SessionRecordFrameProvenance>>,
    evaluation: &'a mut SessionRecordStepEvaluation,
}

struct SessionRecordVerifyTemplateAmendTarget<'a> {
    id: &'a mut String,
    region: &'a mut SessionRecordRegion,
    threshold: &'a mut Option<f64>,
    frame_provenance: &'a mut Option<Box<SessionRecordFrameProvenance>>,
    artifact: &'a mut Option<Box<SessionRecordAnchorArtifact>>,
    evaluation: &'a mut SessionRecordStepEvaluation,
}

#[derive(Debug)]
struct SessionRecordDriftDiagnostics {
    path: PathBuf,
    target_id: String,
    region: SessionRecordRect,
    threshold: Option<f64>,
    changed_fields: Vec<&'static str>,
}

struct SessionRecordBuildDraft {
    root: PathBuf,
    task_dir_name: String,
    bundle: Value,
    task_dir: PathBuf,
    task_path: PathBuf,
    resources_path: PathBuf,
    assets: Vec<SessionRecordBuildAsset>,
}

struct SessionRecordBuildAsset {
    source: PathBuf,
    destination: PathBuf,
    template: String,
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

fn parse_invocation<I>(args: I, json_default: bool) -> Result<Invocation, (String, bool, CliError)>
where
    I: IntoIterator,
    I::Item: Into<String>,
{
    let mut global = GlobalOptions {
        json: json_default,
        ..Default::default()
    };
    let raw = args.into_iter().map(Into::into).collect::<Vec<_>>();
    let mut rest = Vec::new();
    let mut index = 0usize;

    while index < raw.len() {
        match raw[index].as_str() {
            "--json" => global.json = true,
            "--run-root" => {
                index += 1;
                global.run_root = Some(PathBuf::from(require_raw(&raw, index, "--run-root")?));
            }
            "--instance" => {
                index += 1;
                global.instance = Some(require_raw(&raw, index, "--instance")?);
            }
            "--instances" => {
                index += 1;
                global.instances = Some(split_csv(&require_raw(&raw, index, "--instances")?));
            }
            "--profile" => {
                index += 1;
                global.profile = Some(require_raw(&raw, index, "--profile")?);
            }
            "--resource-root" => {
                index += 1;
                global.resource_root =
                    Some(PathBuf::from(require_raw(&raw, index, "--resource-root")?));
            }
            "--dry-run" => global.dry_run = true,
            "--verbose" => global.verbose = true,
            "--quiet" => global.quiet = true,
            "--game" => {
                index += 1;
                global.game = Some(require_raw(&raw, index, "--game")?);
            }
            "--server" => {
                index += 1;
                global.server = Some(require_raw(&raw, index, "--server")?);
            }
            "--runtime-endpoint" => {
                index += 1;
                global.runtime_endpoint = Some(require_raw(&raw, index, "--runtime-endpoint")?);
            }
            "--capture-backend" | "--backend" => {
                index += 1;
                let value = require_raw(&raw, index, raw[index - 1].as_str())?;
                global.capture_backend =
                    Some(CaptureBackendChoice::parse(&value).map_err(|err| {
                        (
                            "help".to_string(),
                            global.json,
                            CliError::usage(err.to_string()),
                        )
                    })?);
            }
            "--touch-backend" => {
                index += 1;
                let value = require_raw(&raw, index, "--touch-backend")?;
                global.touch_backend = Some(TouchBackendChoice::parse(&value).map_err(|err| {
                    (
                        "help".to_string(),
                        global.json,
                        CliError::usage(err.to_string()),
                    )
                })?);
            }
            "--require-session" => {
                return Err((
                    "help".to_string(),
                    global.json,
                    CliError::usage(
                        "--require-session was retired; ActingLab clients use the resident Runtime",
                    ),
                ));
            }
            "--version" => global.version = true,
            other => rest.push(other.to_string()),
        }
        index += 1;
    }

    let (command, args) = if global.version && !package_cli::is_offline_command(&rest) {
        (vec!["version".to_string()], rest)
    } else if rest.is_empty() {
        (vec!["help".to_string()], Vec::new())
    } else {
        command_path_and_args(rest)
    };
    let command_name = command.join(" ");
    Ok(Invocation {
        global,
        command,
        args,
        command_name,
    })
}

fn require_raw(
    raw: &[String],
    index: usize,
    name: &str,
) -> Result<String, (String, bool, CliError)> {
    raw.get(index).cloned().ok_or_else(|| {
        (
            "unknown".to_string(),
            true,
            CliError::usage(format!("missing value for {name}")),
        )
    })
}

fn command_path_and_args(rest: Vec<String>) -> (Vec<String>, Vec<String>) {
    let top = rest[0].clone();
    let path_len = match top.as_str() {
        "config" | "env" | "lab" | "package" | "operation" | "control" | "scheduler"
        | "runtime" | "resource" | "run" | "report" | "session" | "ledger" => {
            rest.get(1).map(|_| 2).unwrap_or(1)
        }
        _ => 1,
    };
    let command = rest.iter().take(path_len).cloned().collect::<Vec<_>>();
    let args = rest.into_iter().skip(path_len).collect::<Vec<_>>();
    (command, args)
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
fn help_data() -> Value {
    json!({
        "usage": "actinglab [global-options] <command> [args]",
        "global_options": [
            "--json",
            "--run-root <path>",
            "--instance <id>",
            "--instances <id,id,...>",
            "--profile <name>",
            "--resource-root <path>",
            "--game <game>",
            "--server <server>",
            "--runtime-endpoint <url>",
            "--capture-backend <auto|auto-fastest|adb|droidcast_raw|nemu_ipc>",
            "--backend <auto|auto-fastest|adb|droidcast_raw|nemu_ipc> (alias of --capture-backend)",
            "--touch-backend <auto|auto-fastest|maatouch|minitouch|adb_shell_input>",
            "--require-session",
            "--dry-run",
            "--verbose",
            "--quiet",
            "--version"
        ],
        "command_options": {
            "resource convert": [
                "--operations <dir>",
                "--out <dir>",
                "--maa-tasks <dir>"
            ],
            "resource compile-maa": [
                "--maa-tasks <dir>", "--task <id> (repeatable with --facts)", "--facts"
            ],
            "session record build-task": [
                "--locale <locale>"
            ]
        },
        "compatibility_notes": {
            "recognize --target": "target output includes width, height, matched_rect, and the shared evaluation object"
        },
        "commands": command_capabilities()
    })
}

fn version_data() -> Value {
    json!({
        "name": "actinglab",
        "cli_version": env!("CARGO_PKG_VERSION"),
        "runtime_version": RUNTIME_VERSION,
        "schema_version": SCHEMA_VERSION
    })
}

fn run_paths(global: &GlobalOptions) -> CliOutcome<Value> {
    let config = read_user_config()?;
    let adb = resolved_adb_json(&config);
    Ok(json!({
        "config_path": config_path()?.display().to_string(),
        "run_root": global.run_root.as_ref().map(|path| path_string(path)).or(config.run_root),
        "resource_root": global.resource_root.as_ref().map(|path| path_string(path)).or(config.resource_root),
        "runtime_endpoint": global.runtime_endpoint.clone().or(config.runtime_endpoint),
        "adb": adb
    }))
}

fn run_config(sub: &str, args: &[String]) -> CliOutcome<Value> {
    match sub {
        "get" => {
            let config = read_user_config()?;
            if args.is_empty() {
                serde_json::to_value(config)
                    .map_err(|err| CliError::usage(format!("failed to serialize config: {err}")))
            } else {
                let key = &args[0];
                Ok(json!({
                    "key": key,
                    "value": config_get(&config, key)?
                }))
            }
        }
        "set" => {
            if args.len() < 2 {
                return Err(CliError::usage("config set requires <key> <value>"));
            }
            let mut config = read_user_config()?;
            config_set(&mut config, &args[0], &args[1])?;
            write_user_config(&config)?;
            Ok(json!({
                "config_path": config_path()?.display().to_string(),
                "key": args[0],
                "value": args[1]
            }))
        }
        _ => Err(CliError::usage(format!("unknown config command: {sub}"))),
    }
}

fn run_doctor(global: &GlobalOptions) -> CliOutcome<Value> {
    let config = read_user_config()?;
    let adb_resolution = resolve_adb_path(config.adb_path.as_deref());
    let runtime_endpoint = effective_runtime_endpoint(global, &config);
    let resource_root = effective_resource_root(global, &config);
    let run_root = effective_run_root(global, &config);
    let mut checks = Vec::new();

    checks.push(json!({
        "name": "config",
        "ok": config_path()?.exists(),
        "path": config_path()?.display().to_string()
    }));
    let mut adb_check = resolved_adb_json_from(adb_resolution);
    adb_check["name"] = json!("adb");
    checks.push(adb_check);
    let runtime_endpoint_check = runtime_endpoint
        .as_ref()
        .map(|endpoint| runtime_endpoint_check(endpoint));
    checks.push(json!({
        "name": "runtime_endpoint",
        "ok": runtime_endpoint_check.as_ref().and_then(|check| check.get("ok")).and_then(Value::as_bool).unwrap_or(false),
        "endpoint": runtime_endpoint,
        "policy": runtime_endpoint_check
    }));
    checks.push(json!({
        "name": "resource_root",
        "ok": resource_root.as_ref().map(|path| path.is_dir()).unwrap_or(false),
        "path": resource_root.as_ref().map(|path| path_string(path))
    }));
    checks.push(json!({
        "name": "run_root",
        "ok": run_root.as_ref().and_then(|path| path.parent()).map(|path| path.exists()).unwrap_or(false),
        "path": run_root.as_ref().map(|path| path_string(path))
    }));
    Ok(json!({
        "checks": checks,
        "note": "doctor is diagnostic; runtime/device unavailability is reported without blocking offline commands"
    }))
}

fn session_throat_policy_payload(
    global: &GlobalOptions,
    flags: &FlagArgs,
    command_name: &str,
) -> CliOutcome<Value> {
    flags.expect_positionals(command_name, 0)?;
    Ok(json!({
        "schema_version": "session.throat_policy.v0.1",
        "status": "offline_policy",
        "purpose": "machine-readable unique Session Layer control throat policy",
        "generated_at_unix_ms": current_unix_ms(),
        "scope": {
            "instance": global.instance.clone(),
            "game": global.game.clone(),
            "server": global.server.clone()
        },
        "session_layer": {
            "resident_daemon": true,
            "only_control_throat": true,
            "clients_must_not_directly_touch_adb_or_devices": true,
            "ui_must_not_directly_touch_adb_or_device": true,
            "scheduler_must_use_session_layer_for_device_control": true,
            "agents_must_use_session_layer_for_device_control": true
        },
        "strict_session_throat": {
            "flag": "--require-session",
            "env": REQUIRE_SESSION_DAEMON_ENV,
            "failure_code": "session_daemon_required",
            "failure_is_visible": true
        },
        "route_policy": {
            "local_read_only_queries": {
                "may_run_local_when_no_resident_daemon": true,
                "prefer_resident_daemon_when_alive": true,
                "local_override_flag": "--local"
            },
            "control_requests": {
                "must_use_resident_daemon_when_available_or_strict": true,
                "requires_matching_lease": true,
                "blocked_without_matching_lease_code": "lab_lease_required"
            },
            "daemon_internal_execution": {
                "forces_local_execution": true,
                "reason": "avoid recursive request requeue inside the resident daemon"
            },
            "trusted_remote": {
                "status": "reserved",
                "requires_encryption": true,
                "requires_authentication": true,
                "blocked_without_auth_code": "trusted_remote_auth_required",
                "blocked_without_encryption_code": "trusted_remote_transport_blocked"
            }
        },
        "lease_gate": {
            "required_for_control": true,
            "matching_fields": ["holder", "lease_id"],
            "preflight": "session command-check <command...>",
            "submit_plan": "session submit-plan <command...>"
        },
        "allowed_offline_evidence": [
            "session command-check",
            "session submit-plan",
            "session api",
            "session contract",
            "session bootstrap",
            "session validation-plan",
            "session throat-policy",
            "session self-heal-policy",
            "session self-heal-plan"
        ],
        "deferred_live_acceptance": {
            "status": "deferred",
            "deferred_code": "requires-live-device",
            "must_not_mark_live_pass_from_offline_checks": true
        },
        "failure_policy": {
            "severe_errors_fail_loud": true,
            "silent_failure_allowed": false,
            "transient_fallback_requires_full_logging": true
        },
        "guarantees": {
            "does_not_enqueue": true,
            "does_not_touch_device": true,
            "does_not_capture": true,
            "does_not_start_maatouch": true,
            "does_not_start_listener": true,
            "does_not_start_apps": true,
            "does_not_read_resource_repositories": true
        }
    }))
}

fn session_capture_policy_payload(
    global: &GlobalOptions,
    flags: &FlagArgs,
    command_name: &str,
) -> CliOutcome<Value> {
    flags.expect_positionals(command_name, 0)?;
    Ok(json!({
        "schema_version": "session.capture_policy.v0.1",
        "status": "offline_policy",
        "purpose": "machine-readable fresh-frame and stale-capture policy for Session Layer clients",
        "generated_at_unix_ms": current_unix_ms(),
        "scope": {
            "instance": global.instance.clone(),
            "game": global.game.clone(),
            "server": global.server.clone()
        },
        "fresh_frame_policy": {
            "require_fresh_flag": "--require-fresh",
            "diagnostic_command": "capture diagnose --require-fresh",
            "session_diagnostic_command": "session capture diagnose --require-fresh",
            "stale_frame_must_be_visible": true,
            "stale_frame_must_not_be_treated_as_success": true
        },
        "backend_policy": {
            "preferred_order": ["nemu_ipc", "droidcast_raw", "adb_screencap"],
            "adb_screencap_is_last_resort": true,
            "fallback_allowed_for_transient_capture_failures": true,
            "fallback_requires_full_logging": true,
            "fallback_log_context": [
                "trigger_reason",
                "source_backend",
                "fallback_backend",
                "instance",
                "game",
                "server",
                "user_visible_impact"
            ]
        },
        "stale_classification": {
            "must_not_classify_as_game_freeze_from_adb_screencap_alone": true,
            "must_compare_or_diagnose_before_freeze_conclusion": true,
            "stale_capture_status": "capture_stale_suspected",
            "game_freeze_status": "unverified_without_fresh_backend_evidence",
            "ak_known_stale_md5": "202752fa3e5cab706774819168639b6c",
            "finding": "FINDING-AK-game-freeze-2026-06-27"
        },
        "freeze_classification_gate": {
            "schema_version": "session.capture_freeze_classification_gate.v0.1",
            "status": "blocked_without_fresh_backend_evidence",
            "safe_to_classify_game_frozen": false,
            "must_not_classify_as_game_freeze_from_adb_screencap_alone": true,
            "finding": "FINDING-AK-game-freeze-2026-06-27",
            "insufficient_evidence": [
                "adb_screencap_same_md5_alone",
                "adb_disconnect_reconnect_same_md5_alone",
                "input_command_returned_ok_without_fresh_frame",
                "high_cpu_without_anr_or_fresh_backend_evidence",
                "page_detector_result_from_stale_frame"
            ],
            "required_before_game_freeze_label": [
                "run capture diagnose --require-fresh",
                "record backend name, frame hash, and timestamp or sequence evidence",
                "compare at least two frames or prove stale status through capture diagnose",
                "try a lighter non-adb_screencap backend when available",
                "record operator/live evidence before accepting a live game-freeze conclusion"
            ],
            "recommended_order": [
                "session capture-policy",
                "capture diagnose --require-fresh",
                "session recover --stale-capture",
                "session self-heal-plan --trigger capture_stale_suspected",
                "operator live validation"
            ],
            "live_validation": {
                "status": "deferred",
                "deferred_code": "requires-live-device",
                "must_not_mark_live_pass_from_offline_checks": true
            }
        },
        "recovery_policy": {
            "read_only_plan": "session recover --stale-capture",
            "diagnosis_first": true,
            "try_lighter_capture_backend_recovery_before_app_restart": true,
            "app_restart_is_heavy_recovery": true,
            "maintenance_recovery_requires_matching_lease_when_it_executes_control": true,
            "does_not_mark_recovery_live_pass_without_operator_observation": true
        },
        "client_guidance": {
            "ui_should_show_degraded_capture_state": true,
            "scheduler_should_not_submit_navigation_on_stale_frame": true,
            "agents_should_recheck_with_capture_policy_before_declaring_game_frozen": true,
            "operator_live_acceptance_deferred_code": "requires-live-device"
        },
        "guarantees": {
            "does_not_enqueue": true,
            "does_not_touch_device": true,
            "does_not_capture": true,
            "does_not_start_maatouch": true,
            "does_not_start_listener": true,
            "does_not_start_apps": true,
            "does_not_read_resource_repositories": true
        }
    }))
}

fn session_record_policy_payload(
    global: &GlobalOptions,
    flags: &FlagArgs,
    command_name: &str,
) -> CliOutcome<Value> {
    flags.expect_positionals(command_name, 0)?;
    Ok(json!({
        "schema_version": "session.record_policy.v0.1",
        "status": "offline_policy",
        "purpose": "machine-readable active recording authorization policy for Session Layer clients",
        "generated_at_unix_ms": current_unix_ms(),
        "scope": {
            "instance": global.instance.clone(),
            "game": global.game.clone(),
            "server": global.server.clone()
        },
        "authorization_model": {
            "active_authorization_required": true,
            "passive_full_recording_allowed": false,
            "navigation_is_not_recorded_by_default": true,
            "operator_selects_step_kind": true,
            "recording_session_required": true,
            "record_start_command": "session record start --task-id <id>",
            "record_step_command": "session record step --kind <kind>",
            "record_amend_command": "session record amend",
            "record_build_command": "session record build-task",
            "record_promote_command": "session record promote"
        },
        "allowed_step_kinds": [
            {
                "kind": "anchor",
                "purpose": "materialize a reviewed page or UI anchor from an authorized frame",
                "requires_explicit_frame_source": true,
                "can_materialize_template": true
            },
            {
                "kind": "operation",
                "purpose": "record reviewed operation metadata and click-bound references",
                "requires_explicit_click_reference": true,
                "can_execute_click": false
            },
            {
                "kind": "color-probe",
                "purpose": "sample a reviewed frame region into color-probe resource metadata",
                "requires_explicit_frame_source": true,
                "can_materialize_color_data": true
            },
            {
                "kind": "verify-template",
                "purpose": "materialize a reviewed verification template from an authorized frame",
                "requires_explicit_frame_source": true,
                "can_materialize_template": true
            }
        ],
        "frame_source_policy": {
            "local_png_allowed": true,
            "current_frame_allowed": true,
            "current_frame_requires_explicit_flag": "--capture or --current-frame",
            "current_frame_uses_existing_capture_backend": true,
            "current_frame_live_validation": "deferred",
            "deferred_code": "requires-live-device",
            "must_store_provenance": true,
            "must_store_hash": true,
            "must_store_freshness_metadata_when_available": true,
            "must_not_read_resource_repositories": true,
            "policy_command_captures": false
        },
        "resource_write_policy": {
            "build_task_writes_local_draft": true,
            "promote_requires_explicit_command": "session record promote",
            "policy_command_writes_resources": false,
            "policy_command_promotes_resources": false,
            "overwrite_requires_opt_in": true,
            "resource_repository_write_requires_explicit_repo": true,
            "promotion_must_preserve_provenance": true
        },
        "safety_policy": {
            "destructive_operation_requires_explicit_flag": true,
            "game_progress_actions_allowed": false,
            "premium_or_paid_resource_use_allowed": false,
            "blind_confirmation_allowed": false,
            "requires_session_layer_for_device_frame_capture": true,
            "requires_matching_lease_for_future_device_control": true,
            "severe_errors_fail_loud": true,
            "silent_failure_allowed": false
        },
        "client_guidance": {
            "ui_should_show_authorization_prompt": true,
            "ui_should_show_step_kind_picker": true,
            "ui_should_show_frame_source_picker": true,
            "ui_should_show_resource_write_warning_before_promote": true,
            "agents_should_call_record_policy_before_record_step": true,
            "operator_can_amend_before_build": true,
            "operator_can_review_candidates_before_build": true,
            "record_policy_query": "session record-policy",
            "daemon_record_policy_query": "session request record-policy"
        },
        "live_validation": {
            "status": "deferred",
            "deferred_code": "requires-live-device",
            "must_not_mark_live_pass_from_offline_checks": true
        },
        "guarantees": {
            "does_not_enqueue": true,
            "does_not_touch_device": true,
            "does_not_capture": true,
            "does_not_start_maatouch": true,
            "does_not_start_apps": true,
            "does_not_read_resource_repositories": true,
            "does_not_write_resource_repositories": true,
            "does_not_start_listener": true,
            "does_not_issue_tokens": true,
            "does_not_start_tls": true
        }
    }))
}

fn session_self_heal_policy_payload(
    global: &GlobalOptions,
    flags: &FlagArgs,
    command_name: &str,
) -> CliOutcome<Value> {
    flags.expect_positionals(command_name, 0)?;
    Ok(json!({
        "schema_version": "session.self_heal_policy.v0.1",
        "status": "offline_policy",
        "purpose": "machine-readable Phase C maintenance self-heal policy for Session Layer clients",
        "generated_at_unix_ms": current_unix_ms(),
        "scope": {
            "instance": global.instance.clone(),
            "game": global.game.clone(),
            "server": global.server.clone()
        },
        "phase_c": {
            "name": "self-heal",
            "goal": "return a session to a known-good state without executing game-progress actions",
            "target_state": "home_or_known_good_page",
            "live_acceptance_status": "deferred",
            "deferred_code": "requires-live-device"
        },
        "flow": [
            {
                "stage": "observe",
                "allowed_commands": ["monitor --once", "session status --diagnostics"],
                "device_control_allowed": false
            },
            {
                "stage": "diagnose",
                "allowed_commands": ["capture diagnose --require-fresh", "session capture diagnose --require-fresh", "current-page", "is-visible"],
                "must_distinguish_capture_stale_from_game_freeze": true
            },
            {
                "stage": "plan",
                "allowed_commands": ["session recover --stale-capture", "session recover --to <page> --dry-run", "session submit-plan <command...>"],
                "must_be_inspectable_before_execution": true
            },
            {
                "stage": "execute",
                "allowed_commands": ["session request recover", "session request app restart", "session monitor-policy set --recover"],
                "requires_matching_lease": true,
                "must_run_through_session_layer": true
            }
        ],
        "trigger_policy": {
            "supported_triggers": [
                "stale_frame",
                "hang",
                "resource_drift",
                "session_expired",
                "standby",
                "modal_popup",
                "off_route_page",
                "unstable_page"
            ],
            "legacy_trigger_aliases": [
                {"alias": "capture_stale_suspected", "canonical": "stale_frame"},
                {"alias": "capture_backend_unavailable", "canonical": "stale_frame"},
                {"alias": "startup_login_required", "canonical": "session_expired"},
                {"alias": "unexpected_page", "canonical": "off_route_page"}
            ],
            "priority_order": [
                ["stale_frame", "hang"],
                ["resource_drift"],
                ["session_expired", "standby"],
                ["modal_popup"],
                ["off_route_page"],
                ["unstable_page"]
            ],
            "stale_adb_screencap_alone_is_not_game_freeze": true,
            "must_diagnose_before_restart": true,
            "must_not_treat_missing_evidence_as_success": true
        },
        "recovery_order": [
            {
                "order": 1,
                "kind": "read_only_diagnosis",
                "examples": ["monitor --once", "capture diagnose --require-fresh"]
            },
            {
                "order": 2,
                "kind": "capture_backend_recovery",
                "examples": ["try nemu_ipc", "try droidcast_raw", "use adb_screencap only as last resort"]
            },
            {
                "order": 3,
                "kind": "maintenance_navigation",
                "examples": ["standby wake", "modal close", "safe route to home"]
            },
            {
                "order": 4,
                "kind": "startup_login_loop",
                "examples": ["session recover --startup-login --dry-run", "bounded popup close loop"]
            },
            {
                "order": 5,
                "kind": "app_lifecycle_restart",
                "examples": ["session app restart"],
                "heavy_recovery": true
            }
        ],
        "maintenance_boundary": {
            "allowed_outcome": "known_good_state_only",
            "game_progress_actions_allowed": false,
            "destructive_actions_allowed": false,
            "premium_or_paid_resource_use_allowed": false,
            "pvp_or_exercise_allowed": false,
            "blind_confirmation_allowed": false,
            "navigation_only_default": true
        },
        "lease_and_scheduler_policy": {
            "scheduler_owns_arbitration": true,
            "session_layer_owns_device_mechanism": true,
            "control_execution_requires_matching_lease": true,
            "monitor_policy_recovery_without_matching_lease": "deferred_by_lease",
            "ui_must_not_bypass_session_layer": true
        },
        "client_guidance": {
            "ui_should_show_degraded_state": true,
            "scheduler_should_pause_task_submission_until_policy_allows_execution": true,
            "agents_should_request_plan_before_execution": true,
            "interactive_stream_should_report_recovery_state_but_not_execute_without_lease": true,
            "operator_live_acceptance_deferred_code": "requires-live-device"
        },
        "guarantees": {
            "does_not_enqueue": true,
            "does_not_touch_device": true,
            "does_not_capture": true,
            "does_not_start_maatouch": true,
            "does_not_start_listener": true,
            "does_not_start_apps": true,
            "does_not_read_resource_repositories": true
        }
    }))
}

fn session_access_contract() -> Value {
    json!({
        "schema_version": "session.access.v0.1",
        "purpose": "machine-readable access boundary for Session Layer clients",
        "session_layer": {
            "resident_daemon": true,
            "only_control_throat": true,
            "ui_direct_device_access_allowed": false,
            "direct_adb_access_allowed_for_clients": false
        },
        "entrypoints": {
            "local_cli": {
                "status": "available",
                "encryption_required": false,
                "authentication_required": false,
                "command": "actinglab"
            },
            "trusted_remote": {
                "status": "reserved",
                "encryption_required": true,
                "authentication_required": true,
                "minimum_transport": "TLS or mutually authenticated local IPC",
                "token_or_certificate_required": true,
                "plan_command": "session transport plan [--endpoint <url>]",
                "auth_env": {
                    "token": TRUSTED_REMOTE_TOKEN_ENV,
                    "client_certificate": TRUSTED_REMOTE_CLIENT_CERT_ENV
                },
                "blocked_without_auth_code": "trusted_remote_auth_required",
                "blocked_without_encryption_code": "trusted_remote_transport_blocked"
            }
        },
        "daemon_queries": {
            "bootstrap": "session request bootstrap",
            "throat_policy": "session request throat-policy",
            "capture_policy": "session request capture-policy",
            "record_policy": "session request record-policy",
            "self_heal_policy": "session request self-heal-policy",
            "self_heal_plan": "session request self-heal-plan [--trigger <kind>] [--to <page>]",
            "phase_c_plan": "session request phase-c-plan [--endpoint <url>] [--trigger <kind>] [--to <page>]",
            "contract": "session request contract",
            "api": "session request api",
            "transport": "session request transport",
            "transport_plan": "session request transport plan [--endpoint <url>]",
            "transport_check": "session request transport check --endpoint <url>",
            "capabilities": "session request capabilities",
            "readiness": "session request readiness",
            "connect_plan": "session request connect-plan",
            "stream_plan": "session request stream-plan",
            "command_check": "session request command-check <command...>",
            "submit_plan": "session request submit-plan <command...>",
            "validation_plan": "session request validation-plan",
            "status": "session request status --diagnostics",
            "queue": "session request queue",
            "journal": "session request journal",
            "events": "session request events",
            "instance_registry": "session request instance registry",
            "monitor_policy": "session request monitor-policy status"
        },
        "daemon_controls": {
            "app_lifecycle": "session request app <launch|stop|force-stop|restart>",
            "instance_app_lifecycle": "session request instance app <launch|stop|force-stop|restart>"
        },
        "request_classes": {
            "read_only": {
                "requires_lease": false,
                "examples": [
                    "status",
                    "bootstrap",
                    "throat-policy",
                    "capture-policy",
                    "record-policy",
                    "self-heal-policy",
                    "self-heal-plan",
                    "phase-c-plan",
                    "queue",
                    "journal",
                    "readiness",
                    "stream-plan",
                    "command-check",
                    "submit-plan",
                    "validation-plan",
                    "contract",
                    "transport plan",
                    "transport check",
                    "capabilities",
                    "devices",
                    "capture",
                    "capture-diagnose",
                    "stream",
                    "recognize",
                    "detect-page",
                    "current-page",
                    "is-visible",
                    "locate",
                    "session recover --stale-capture",
                    "session record step --capture",
                    "session record step --current-frame",
                    "session monitor-policy status",
                    "session instance registry",
                    "monitor-once"
                ],
                "device_affecting_examples": [
                    "capture",
                    "capture-diagnose",
                    "stream",
                    "recognize",
                    "detect-page",
                    "current-page",
                    "is-visible",
                    "locate",
                    "session record step --capture",
                    "session record step --current-frame"
                ]
            },
            "daemon_state": {
                "requires_lease": false,
                "recovery_policy_requires_matching_lease": true,
                "recovery_policy_defers_without_matching_lease": true,
                "examples": [
                    "session record start",
                    "session record status",
                    "session record stop",
                    "session record step --frame <png>",
                    "session record candidates",
                    "session record amend",
                    "session record build-task",
                    "session record promote",
                    "session monitor-policy set",
                    "session monitor-policy clear"
                ]
            },
            "control": {
                "requires_lease": true,
                "examples": [
                    "lease",
                    "session app launch",
                    "session app stop",
                    "session app force-stop",
                    "session app restart",
                    "session instance app launch",
                    "session instance app stop",
                    "session instance app force-stop",
                    "session instance app restart",
                    "lab-run",
                    "package-run",
                    "operation-run",
                    "tap",
                    "swipe",
                    "long-tap",
                    "key",
                    "text",
                    "stream --input-relay",
                    "stream --input-event <action,args>",
                    "stream --relay-event <action,args>",
                    "tap-target",
                    "navigate",
                    "recover except --stale-capture"
                ]
            }
        },
        "safety": {
            "strict_session_throat_flag": "--require-session",
            "strict_session_throat_env": REQUIRE_SESSION_DAEMON_ENV,
            "strict_session_throat_failure_code": "session_daemon_required",
            "clients_must_not_directly_touch_adb_or_devices": true,
            "ui_must_not_directly_touch_adb_or_device": true,
            "control_requests_require_matching_lease": true,
            "requests_are_serialized_by_resident_daemon": true,
            "severe_errors_fail_loud": true,
            "transient_recovery_path_must_be_logged": true
        },
        "local_reliability_threat_model": {
            "schema_version": "session.local_reliability_threat_model.v0.1",
            "scope": "local automation reliability",
            "state_dir_and_endpoint_writable_by_same_user_are_trusted_environment": true,
            "same_user_forged_state_or_endpoint_is_accepted_risk": true,
            "current_readiness_is_not_same_user_authentication": true,
            "must_fail_fast_when_daemon_does_not_ack_request": true,
            "authentication_key_material_and_memory_protection_deferred_to_trusted_channel_scheduler_ui": true,
            "trusted_channel_phase": "P3/#10"
        },
        "out_of_scope": [
            "network listener",
            "TLS implementation",
            "token issuance",
            "same-user state_dir/endpoint forgery authentication",
            "secret challenge proof",
            "memory encryption",
            "UI transport",
            "scheduler runtime"
        ]
    })
}

fn session_transport_contract() -> Value {
    json!({
        "schema_version": "session.transport.v0.1",
        "purpose": "machine-readable transport boundary for Session Layer clients",
        "channels": {
            "local_cli": {
                "status": "available",
                "transport": "process_stdio",
                "command": "actinglab",
                "encryption_required": false,
                "authentication_required": false,
                "intended_clients": ["local_operator", "local_agent"]
            },
            "daemon_file_ipc": {
                "status": "available",
                "transport": "session_state_directory_file_queue",
                "submit_command": "session request <command>",
                "request_dir": "requests/",
                "response_dir": "responses/",
                "journal": "request-journal.jsonl",
                "serialized_by_daemon": true,
                "read_only_requests_require_lease": false,
                "control_requests_require_matching_lease": true
            },
            "trusted_remote": {
                "status": "reserved",
                "network_listener_implemented": false,
                "plan_command": "session transport plan [--endpoint <url>]",
                "plan_gate_field": "trusted_remote_gate",
                "plan_gate_schema_version": "session.trusted_remote_gate.v0.1",
                "preflight_command": "session transport check --endpoint <url>",
                "encryption_required": true,
                "authentication_required": true,
                "minimum_transport": "TLS or mutually authenticated local IPC",
                "token_or_certificate_required": true,
                "auth_env": {
                    "token": TRUSTED_REMOTE_TOKEN_ENV,
                    "client_certificate": TRUSTED_REMOTE_CLIENT_CERT_ENV
                },
                "blocked_without_auth_code": "trusted_remote_auth_required",
                "blocked_without_encryption_code": "trusted_remote_transport_blocked"
            },
            "interactive_stream": {
                "status": "partial",
                "preflight_command": "stream check",
                "daemon_preflight_command": "session request stream check",
                "preflight_schema_version": "session.stream_check.v0.1",
                "implemented_surfaces": {
                    "bounded_local_cli_stream": {
                        "status": "available",
                        "command": "stream --max-frames <N>",
                        "schema_version": "session.stream.v0.1",
                        "frame_delivery": "json_array",
                        "frame_event_schema": "session.stream.event.v0.1",
                        "max_frames_per_request": 60
                    },
                    "daemon_bounded_stream_request": {
                        "status": "available",
                        "command": "session request stream",
                        "read_only_without_input_relay_requires_lease": false,
                        "input_relay_requires_matching_lease": true
                    },
                    "per_request_input_relay": {
                        "status": "available",
                        "actions": ["tap", "swipe", "long-tap", "key", "text"],
                        "max_events_per_request": 16,
                        "long_lived_session": false
                    }
                },
                "trusted_remote_long_lived_stream": {
                    "status": "reserved",
                    "future_transport": "trusted bidirectional channel",
                    "network_listener_implemented": false,
                    "encryption_required": true,
                    "authentication_required": true
                }
            }
        },
        "safety": {
            "strict_session_throat_flag": "--require-session",
            "strict_session_throat_env": REQUIRE_SESSION_DAEMON_ENV,
            "strict_session_throat_failure_code": "session_daemon_required",
            "clients_must_not_directly_touch_adb_or_devices": true,
            "remote_transport_must_not_start_without_authentication": true,
            "remote_transport_must_not_start_without_encryption": true,
            "control_requests_are_lease_gated": true,
            "requests_are_serialized_by_resident_daemon": true
        },
        "out_of_scope": [
            "network listener",
            "TLS implementation",
            "token issuance",
            "trusted remote long-lived stream transport",
            "scheduler runtime"
        ]
    })
}

fn session_api_contract() -> Value {
    let mut contract = json!({
        "schema_version": "session.api.v0.1",
        "purpose": "machine-readable command and envelope contract for Session Layer clients",
        "session_layer": {
            "resident_daemon": true,
            "only_control_throat": true,
            "clients_must_not_directly_touch_adb_or_devices": true,
            "requests_are_serialized_by_resident_daemon": true
        },
        "access_channels": {
            "local_cli": {
                "status": "available",
                "command": "actinglab",
                "encryption_required": false,
                "authentication_required": false
            },
            "trusted_remote": {
                "status": "reserved",
                "network_listener_implemented": false,
                "encryption_required": true,
                "authentication_required": true,
                "minimum_transport": "TLS or mutually authenticated local IPC",
                "token_or_certificate_required": true,
                "auth_env": {
                    "token": TRUSTED_REMOTE_TOKEN_ENV,
                    "client_certificate": TRUSTED_REMOTE_CLIENT_CERT_ENV
                },
                "blocked_without_auth_code": "trusted_remote_auth_required",
                "blocked_without_encryption_code": "trusted_remote_transport_blocked"
            }
        },
        "daemon_request_queue": {
            "status": "available",
            "submit_command": "session request <command>",
            "request_dir": "requests/",
            "response_dir": "responses/",
            "journal": "request-journal.jsonl",
            "request_fields": [
                "request_id",
                "command",
                "global",
                "args",
                "lease",
                "created_at_unix_ms"
            ],
            "submit_modes": {
                "sync_wait": {
                    "default": true,
                    "waits_for_response": true,
                    "consumes_response_on_success": true,
                    "timeout_flag": "--request-timeout-ms"
                },
                "no_wait": {
                    "flag": "--no-wait",
                    "waits_for_acknowledgement": true,
                    "ack_timeout_flag": "--request-ack-timeout-ms",
                    "waits_for_response": false,
                    "response_query": "session response get <request-id>",
                    "consume_query": "session response get <request-id> --consume"
                }
            },
            "cancel_query": "session request cancel <request-id> [--reason text] [--dry-run]",
            "cancel_error_code": "request_cancelled",
            "cancel_records_journal": true,
            "cancel_dry_run_preserves_queue": true,
            "admission_gate": {
                "queue_health_field": "diagnostics.queues.health",
                "blocks_status": "needs_attention",
                "error_code": "request_queue_needs_attention",
                "preflight_command": "session command-check <command...>"
            },
            "response_fields": [
                "request_id",
                "command",
                "ok",
                "data",
                "error",
                "started_at_unix_ms",
                "completed_at_unix_ms"
            ]
        },
        "envelopes": {
            "cli": {
                "schema_version": "0.2",
                "success_fields": ["ok", "command", "data"],
                "error_fields": ["ok", "command", "error"]
            },
            "transport_view": {
                "query": "session transport",
                "daemon_query": "session request transport",
                "schema_version": "session.transport.v0.1",
                "plan_query": "session transport plan [--endpoint <url>]",
                "daemon_plan_query": "session request transport plan [--endpoint <url>]",
                "plan_schema_version": "session.transport_plan.v0.1",
                "plan_next_actions_field": "next_actions",
                "plan_trusted_remote_gate_field": "trusted_remote_gate",
                "plan_trusted_remote_gate_schema_version": "session.trusted_remote_gate.v0.1",
                "check_query": "session transport check --endpoint <url>",
                "daemon_check_query": "session request transport check --endpoint <url>",
                "check_schema_version": "session.transport_check.v0.1"
            },
            "status_view": {
                "query": "session status --diagnostics",
                "daemon_query": "session request status --diagnostics",
                "liveness_field": "diagnostics.liveness",
                "instance_registry_field": "diagnostics.instances",
                "lease_field": "diagnostics.leases",
                "queue_field": "diagnostics.queues",
                "queue_health_field": "diagnostics.queues.health",
                "pending_request_preview_field": "diagnostics.queues.pending_request_preview",
                "pending_response_preview_field": "diagnostics.queues.pending_response_preview",
                "journal_field": "diagnostics.journal",
                "recommended_actions_field": "diagnostics.recommended_actions",
                "capture_freshness_summary_field": "diagnostics.capture_freshness",
                "self_heal_summary_field": "diagnostics.self_heal",
                "interaction_flow_summary_field": "diagnostics.interaction_flow",
                "trusted_channel_summary_field": "diagnostics.trusted_channel",
                "phase_c_summary_field": "diagnostics.phase_c",
                "validation_summary_field": "diagnostics.validation",
                "monitor_policy_lease_actions": [
                    "monitor_policy_inspect_lease",
                    "monitor_policy_acquire_lease",
                    "monitor_policy_preempt_lease"
                ],
                "lease_freshness_actions": [
                    "stale_lease_inspect"
                ],
                "capture_health_actions": [
                    "stale_capture_recover",
                    "capture_backend_health_check"
                ],
                "self_heal_escalation_actions": [
                    "self_heal_escalation_review"
                ],
                "interaction_channel_actions": [
                    "interactive_stream_preflight_review",
                    "trusted_channel_preflight_review"
                ],
                "phase_c_plan_actions": [
                    "phase_c_plan_review"
                ],
                "validation_plan_actions": [
                    "validation_plan_review"
                ],
                "queue_health_actions": [
                    "blocked_request_inspect",
                    "blocked_request_cancel_dry_run",
                    "blocked_request_cancel",
                    "blocked_request_cancel_requires_lease",
                    "blocked_running_request_inspect",
                    "unclaimed_response_read"
                ],
                "journal_error_actions": [
                    "failed_request_inspect"
                ]
            },
            "readiness_view": {
                "query": "session readiness [--endpoint <url>]",
                "daemon_query": "session request readiness [--endpoint <url>]",
                "schema_version": "session.readiness.v0.1",
                "ready_field": "ready",
                "status_field": "status",
                "daemon_ready_field": "daemon.can_accept_requests",
                "queues_field": "queues",
                "queue_health_field": "queues.health",
                "instances_field": "instances",
                "instance_status_field": "instances.status",
                "selected_instance_status_field": "instances.selected_status",
                "selected_instance_missing_required_field": "instances.selected_missing_required",
                "transport_ready_field": "transport.safe_to_connect",
                "policy_summary_field": "policy_summary",
                "policy_summary_schema_version": "session.readiness_policy_summary.v0.1",
                "diagnostics_summary_field": "diagnostics_summary",
                "diagnostics_summary_schema_version": "session.readiness_diagnostics_summary.v0.1",
                "phase_c_summary_field": "diagnostics_summary.phase_c",
                "phase_c_acceptance_gates_schema_version_field": "diagnostics_summary.phase_c.acceptance_gates_schema_version",
                "phase_c_acceptance_gate_lane_count_field": "diagnostics_summary.phase_c.acceptance_gate_lane_count",
                "recommended_actions_field": "recommended_actions",
                "blockers_field": "blockers"
            },
            "queue_view": {
                "query": "session queue",
                "daemon_query": "session request queue",
                "schema_version": "session.queue.v0.1",
                "health_field": "health",
                "counts_field": "counts",
                "previews_field": "previews",
                "recommended_actions_field": "recommended_actions",
                "admission_field": "admission",
                "local_query_inspects_blocked_queue": true,
                "does_not_enqueue": true,
                "does_not_touch_device": true
            },
            "command_check_view": {
                "query": "session command-check <command...>",
                "daemon_query": "session request command-check <command...>",
                "schema_version": "session.command_check.v0.1",
                "safe_to_submit_field": "safe_to_submit",
                "command_class_field": "command_class",
                "lease_gate_field": "lease_gate",
                "queue_gate_field": "queue_gate",
                "instance_gate_field": "instance_gate",
                "throat_gate_field": "throat_gate",
                "phase_c_scope_field": "phase_c_scope",
                "phase_c_scope_schema_version": "session.command_phase_c_scope.v0.1",
                "routing_field": "routing",
                "does_not_enqueue": true,
                "does_not_touch_device": true
            },
            "submit_plan_view": {
                "query": "session submit-plan <command...>",
                "daemon_query": "session request submit-plan <command...>",
                "schema_version": "session.submit_plan.v0.1",
                "ready_to_submit_field": "ready_to_submit",
                "preflight_summary_field": "preflight_summary",
                "phase_c_execution_preflight_field": "phase_c_execution_preflight",
                "phase_c_execution_preflight_schema_version": "session.submit_phase_c_execution_preflight.v0.1",
                "readiness_field": "readiness",
                "command_check_field": "command_check",
                "queue_field": "queue",
                "blockers_field": "blockers",
                "does_not_enqueue": true,
                "does_not_touch_device": true
            },
            "validation_plan_view": {
                "query": "session validation-plan",
                "daemon_query": "session request validation-plan",
                "schema_version": "session.validation_plan.v0.1",
                "live_validation_status_field": "live_validation_status",
                "deferred_code_field": "deferred_code",
                "deferred_live_tasks_field": "deferred_live_tasks",
                "pending_live_acceptance_field": "pending_live_acceptance",
                "phase_acceptance_matrix_field": "phase_acceptance_matrix",
                "next_actions_field": "next_actions",
                "offline_verification_allowed_field": "offline_verification_allowed",
                "does_not_enqueue": true,
                "does_not_touch_device": true,
                "does_not_capture": true,
                "does_not_start_maatouch": true
            },
            "lease_view": {
                "query": "session lease list|status|touch|wait|acquire|release|preempt",
                "daemon_query": "session request lease list|status|touch|wait|acquire|release|preempt",
                "list_schema_version": "session.lease_list.v0.1",
                "list_query": "session lease list [--holder <id>] [--lease-id <id>]",
                "daemon_list_query": "session request lease list [--holder <id>] [--lease-id <id>]",
                "list_filters": ["--holder", "--lease-holder", "--lease-id"],
                "freshness_field": "freshness",
                "freshness_statuses": ["fresh", "stale"],
                "freshness_stale_after_ms": SESSION_LEASE_STALE_MS,
                "status_schema_version": "session.lease_status.v0.1",
                "touch_schema_version": "session.lease_touch.v0.1",
                "touch_query": "session lease touch [--holder <id>] [--lease-id <id>]",
                "daemon_touch_query": "session request lease touch [--holder <id>] [--lease-id <id>]",
                "touch_updates": "updated_at_unix_ms",
                "touch_requires_matching_holder": true,
                "wait_schema_version": "session.lease_wait.v0.1",
                "wait_query": "session lease wait [--status free|held] [--holder <id>] [--lease-id <id>] [--timeout-ms N] [--poll-ms N]",
                "daemon_wait_query": "session request lease wait [--status free|held] [--holder <id>] [--lease-id <id>] [--timeout-ms N] [--poll-ms N]",
                "wait_default_status": "free",
                "wait_statuses": ["free", "held"],
                "wait_timeout_default_ms": SESSION_DAEMON_REQUEST_TIMEOUT_MS,
                "wait_poll_default_ms": 100,
                "wait_timeout_returns_current_state": true
            },
            "journal_view": {
                "query": "session journal",
                "daemon_query": "session request journal",
                "filters": ["--limit", "--command", "--data-summary-kind", "--status", "--lease-holder"],
                "global_filters": ["--instance", "--game", "--server"],
                "command_filter_repeats": true,
                "data_summary_kind_filter_repeats": true,
                "status_filter_values": ["completed", "failed"],
                "status_filter_repeats": true,
                "lease_holder_filter_repeats": true,
                "entry_selector_field": "entries[].global"
            },
            "response_view": {
                "query": "session response get <request-id> [--consume]",
                "daemon_query": "session request response get <request-id> [--consume]",
                "wait_query": "session response wait <request-id> [--timeout-ms N] [--poll-ms N] [--consume]",
                "daemon_wait_query": "session request response wait <request-id> [--timeout-ms N] [--poll-ms N] [--consume]",
                "schema_version": "session.response.v0.1",
                "consume_flag": "--consume",
                "wait_timeout_default_ms": SESSION_DAEMON_REQUEST_TIMEOUT_MS,
                "wait_poll_default_ms": 100,
                "delete_after_successful_parse": true,
                "missing_response_code": "runtime_not_running"
            },
            "request_state_view": {
                "query": "session request-state get <request-id>",
                "daemon_query": "session request request-state get <request-id>",
                "wait_query": "session request-state wait <request-id> [--status <state>] [--timeout-ms N] [--poll-ms N]",
                "daemon_wait_query": "session request request-state wait <request-id> [--status <state>] [--timeout-ms N] [--poll-ms N]",
                "schema_version": "session.request_state.v0.1",
                "list_query": "session request-state list [--limit N] [--status <state>] [--lease-holder <id>]",
                "daemon_list_query": "session request request-state list [--limit N] [--status <state>] [--lease-holder <id>]",
                "list_schema_version": "session.request_state_list.v0.1",
                "list_filters": ["--limit", "--status", "--lease-holder"],
                "list_global_filters": ["--instance", "--game", "--server"],
                "lease_holder_filter_repeats": true,
                "statuses": ["queued", "running", "response_available", "completed", "failed", "unknown"],
                "state_sources": ["requests", "running", "responses", "request-journal"],
                "wait_default_statuses": ["response_available", "completed", "failed"],
                "wait_timeout_default_ms": SESSION_DAEMON_REQUEST_TIMEOUT_MS,
                "wait_poll_default_ms": 100,
                "wait_timeout_returns_current_state": true
            },
            "event_view": {
                "query": "session events",
                "daemon_query": "session request events",
                "wait_query": "session events wait [--timeout-ms N] [--poll-ms N]",
                "daemon_wait_query": "session request events wait [--timeout-ms N] [--poll-ms N]",
                "schema_version": "session.events.v0.1",
                "filters": ["--limit", "--after-unix-ms", "--after-request-id", "--command", "--data-summary-kind", "--status", "--lease-holder"],
                "global_filters": ["--instance", "--game", "--server"],
                "wait_timeout_default_ms": SESSION_DAEMON_REQUEST_TIMEOUT_MS,
                "wait_poll_default_ms": 100,
                "wait_timeout_returns_empty_events": true,
                "command_filter_repeats": true,
                "data_summary_field": "events[].data_summary",
                "stream_data_summary_kind": "stream",
                "data_summary_kinds": ["stream", "queue", "bootstrap", "readiness", "throat_policy", "command_check", "submit_plan", "capture_policy", "record_policy", "self_heal_policy", "self_heal_plan", "phase_c_plan", "connect_plan", "stream_plan", "transport_plan", "validation_plan", "capture_diagnose", "stale_capture_recovery"],
                "data_summary_kind_filter_repeats": true,
                "status_filter_values": ["completed", "failed"],
                "status_filter_repeats": true,
                "lease_holder_filter_repeats": true,
                "cursor_fields": [
                    "latest_timestamp_unix_ms",
                    "next_after_unix_ms",
                    "latest_request_id",
                    "next_after_request_id"
                ],
                "cursor_error": "event_cursor_not_found"
            },
            "monitor_policy_view": {
                "query": "session monitor-policy status",
                "daemon_query": "session request monitor-policy status",
                "schema_version": "session.monitor_policy_status.v0.1",
                "state_field": "state",
                "policy_field": "policy",
                "execution_model": "daemon_owned_monitor_once",
                "default_read_only": true,
                "recovery_requires_matching_lease": true,
                "recovery_without_matching_lease_status": "deferred_by_lease"
            },
            "instance_registry_view": {
                "query": "session instance registry",
                "daemon_query": "session request instance registry",
                "schema_version": "session.instance_registry.v0.1",
                "ready_field": "instances[].validation.ready_for_device_control"
            },
            "app_lifecycle_view": {
                "query": "session app <launch|stop|force-stop|restart>",
                "daemon_query": "session request app <launch|stop|force-stop|restart>",
                "aliases": ["session instance app <launch|stop|force-stop|restart>", "session request instance app <launch|stop|force-stop|restart>"],
                "requires_lease": true,
                "actions": ["launch", "stop", "force-stop", "restart"],
                "action_field": "action",
                "package_field": "package"
            },
            "stream_view": null,
            "stale_capture_recovery_view": {
                "query": "session recover --stale-capture [--capture|--diagnose]",
                "daemon_query": "session request recover --stale-capture [--capture|--diagnose]",
                "read_only": true,
                "requires_lease": false,
                "executes_input": false,
                "executes_app_restart": false,
                "diagnosis_statuses": ["planned", "diagnosed_fresh", "diagnosed_stale", "diagnosis_unavailable"],
                "recovery_gate": "diagnose_capture_backend_before_restart"
            }
        },
        "command_classes": {
            "read_only": {
                "requires_lease": false,
                "examples": [
                    "status",
                    "bootstrap",
                    "readiness",
                    "connect-plan",
                    "stream-plan",
                    "throat-policy",
                    "capture-policy",
                    "record-policy",
                    "self-heal-policy",
                    "self-heal-plan",
                    "command-check",
                    "submit-plan",
                    "validation-plan",
                    "journal",
                    "events",
                    "response",
                    "request-state",
                    "contract",
                    "api",
                    "capabilities",
                    "devices",
                    "capture",
                    "capture-diagnose",
                    "stream",
                    "recognize",
                    "detect-page",
                    "current-page",
                    "is-visible",
                    "locate",
                    "session recover --stale-capture",
                    "session record step --capture",
                    "session record step --current-frame",
                    "session monitor-policy status",
                    "session instance registry",
                    "monitor-once"
                ],
                "device_affecting_examples": [
                    "capture",
                    "capture-diagnose",
                    "stream",
                    "recognize",
                    "detect-page",
                    "current-page",
                    "is-visible",
                    "locate",
                    "session record step --capture",
                    "session record step --current-frame"
                ]
            },
            "control": {
                "requires_lease": true,
                "examples": [
                    "lease",
                    "session app launch",
                    "session app stop",
                    "session app force-stop",
                    "session app restart",
                    "session instance app launch",
                    "session instance app stop",
                    "session instance app force-stop",
                    "session instance app restart",
                    "lab-run",
                    "package-run",
                    "operation-run",
                    "tap",
                    "swipe",
                    "long-tap",
                    "key",
                    "text",
                    "stream --input-relay",
                    "stream --input-event <action,args>",
                    "stream --relay-event <action,args>",
                    "tap-target",
                    "navigate",
                    "recover except --stale-capture"
                ]
            },
            "daemon_state": {
                "requires_lease": false,
                "recovery_policy_requires_matching_lease": true,
                "recovery_policy_defers_without_matching_lease": true,
                "examples": [
                    "session record start",
                    "session record status",
                    "session record stop",
                    "session record step --frame <png>",
                    "session record candidates",
                    "session record amend",
                    "session record build-task",
                    "session record promote",
                    "session monitor-policy set",
                    "session monitor-policy clear"
                ]
            }
        },
        "failure_contract": {
            "missing_or_stale_daemon_code": "runtime_not_running",
            "strict_session_throat_failure_code": "session_daemon_required",
            "control_without_matching_lease_code": "lab_lease_required",
            "untrusted_remote_endpoint_code": "trusted_remote_transport_blocked",
            "missing_trusted_remote_auth_code": "trusted_remote_auth_required",
            "severe_errors_fail_loud": true
        },
        "out_of_scope": [
            "network listener",
            "TLS implementation",
            "token issuance",
            "UI transport",
            "scheduler runtime"
        ]
    });
    contract
        .pointer_mut("/envelopes")
        .and_then(Value::as_object_mut)
        .expect("session api contract envelopes must be an object")
        .insert(
            "bootstrap_view".to_string(),
            session_bootstrap_view_contract(),
        );
    contract
        .pointer_mut("/envelopes")
        .and_then(Value::as_object_mut)
        .expect("session api contract envelopes must be an object")
        .insert(
            "connect_plan_view".to_string(),
            session_connect_plan_view_contract(),
        );
    contract
        .pointer_mut("/envelopes")
        .and_then(Value::as_object_mut)
        .expect("session api contract envelopes must be an object")
        .insert("stream_view".to_string(), session_stream_view_contract());
    contract
        .pointer_mut("/envelopes")
        .and_then(Value::as_object_mut)
        .expect("session api contract envelopes must be an object")
        .insert(
            "stream_plan_view".to_string(),
            session_stream_plan_view_contract(),
        );
    contract
        .pointer_mut("/envelopes")
        .and_then(Value::as_object_mut)
        .expect("session api contract envelopes must be an object")
        .insert(
            "throat_policy_view".to_string(),
            session_throat_policy_view_contract(),
        );
    contract
        .pointer_mut("/envelopes")
        .and_then(Value::as_object_mut)
        .expect("session api contract envelopes must be an object")
        .insert(
            "capture_policy_view".to_string(),
            session_capture_policy_view_contract(),
        );
    contract
        .pointer_mut("/envelopes")
        .and_then(Value::as_object_mut)
        .expect("session api contract envelopes must be an object")
        .insert(
            "record_policy_view".to_string(),
            session_record_policy_view_contract(),
        );
    contract
        .pointer_mut("/envelopes")
        .and_then(Value::as_object_mut)
        .expect("session api contract envelopes must be an object")
        .insert(
            "self_heal_policy_view".to_string(),
            session_self_heal_policy_view_contract(),
        );
    contract
        .pointer_mut("/envelopes")
        .and_then(Value::as_object_mut)
        .expect("session api contract envelopes must be an object")
        .insert(
            "self_heal_plan_view".to_string(),
            session_self_heal_plan_view_contract(),
        );
    contract
        .pointer_mut("/envelopes")
        .and_then(Value::as_object_mut)
        .expect("session api contract envelopes must be an object")
        .insert(
            "phase_c_plan_view".to_string(),
            session_phase_c_plan_view_contract(),
        );
    contract
}

fn session_connect_plan_view_contract() -> Value {
    json!({
        "query": "session connect-plan [--endpoint <url>] [stream check flags]",
        "daemon_query": "session request connect-plan [--endpoint <url>] [stream check flags]",
        "schema_version": "session.connect_plan.v0.1",
        "readiness_field": "readiness",
        "transport_field": "transport",
        "stream_preflight_field": "stream_preflight",
        "phase_c_preflight_field": "phase_c_preflight",
        "phase_c_preflight_schema_version": "session.connect_phase_c_preflight.v0.1",
        "next_actions_field": "next_actions",
        "safe_to_start_client_field": "safe_to_start_client",
        "blocked_reason_field": "blockers",
        "does_not_enqueue": true,
        "does_not_touch_device": true,
        "does_not_capture": true,
        "does_not_start_maatouch": true,
        "does_not_start_listener": true
    })
}

fn session_stream_view_contract() -> Value {
    json!({
        "query": "stream --max-frames <N>",
        "daemon_query": "session request stream",
        "check_query": "stream check",
        "daemon_check_query": "session request stream check",
        "plan_query": "session stream-plan",
        "daemon_plan_query": "session request stream-plan",
        "schema_version": "session.stream.v0.1",
        "check_schema_version": "session.stream_check.v0.1",
        "plan_schema_version": "session.stream_plan.v0.1",
        "event_schema_version": "session.stream.event.v0.1",
        "bounded_local_cli_status": "available",
        "read_only_without_input_relay_requires_lease": false,
        "input_relay_requires_lease": true,
        "safe_to_start_field": "safe_to_start",
        "input_relay_actions": ["tap", "swipe", "long-tap", "key", "text"],
        "input_relay_event_flags": ["--input-relay", "--input-event", "--relay-event"],
        "input_relay_preflight_command": "session command-check stream --input-event <action,args>",
        "trusted_remote_long_lived_stream_status": "reserved"
    })
}

fn session_stream_plan_view_contract() -> Value {
    json!({
        "query": "session stream-plan [--endpoint <url>] [stream check flags]",
        "daemon_query": "session request stream-plan [--endpoint <url>] [stream check flags]",
        "schema_version": "session.stream_plan.v0.1",
        "connect_plan_field": "connect_plan",
        "stream_preflight_field": "stream_preflight",
        "stream_modes_field": "stream_modes",
        "next_actions_field": "next_actions",
        "trusted_remote_long_lived_status_field": "stream_modes.trusted_remote_long_lived.status",
        "safe_to_open_stream_field": "safe_to_open_stream",
        "blocked_reason_field": "blockers",
        "does_not_enqueue": true,
        "does_not_touch_device": true,
        "does_not_capture": true,
        "does_not_start_maatouch": true,
        "does_not_start_listener": true
    })
}

fn session_throat_policy_view_contract() -> Value {
    json!({
        "query": "session throat-policy",
        "daemon_query": "session request throat-policy",
        "schema_version": "session.throat_policy.v0.1",
        "only_control_throat_field": "session_layer.only_control_throat",
        "strict_session_throat_field": "strict_session_throat",
        "route_policy_field": "route_policy",
        "lease_gate_field": "lease_gate",
        "deferred_live_acceptance_field": "deferred_live_acceptance",
        "does_not_enqueue": true,
        "does_not_touch_device": true,
        "does_not_capture": true,
        "does_not_start_maatouch": true
    })
}

fn session_capture_policy_view_contract() -> Value {
    json!({
        "query": "session capture-policy",
        "daemon_query": "session request capture-policy",
        "schema_version": "session.capture_policy.v0.1",
        "fresh_frame_policy_field": "fresh_frame_policy",
        "backend_policy_field": "backend_policy",
        "stale_classification_field": "stale_classification",
        "freeze_classification_gate_field": "freeze_classification_gate",
        "freeze_classification_gate_schema_version": "session.capture_freeze_classification_gate.v0.1",
        "recovery_policy_field": "recovery_policy",
        "does_not_enqueue": true,
        "does_not_touch_device": true,
        "does_not_capture": true,
        "does_not_start_maatouch": true
    })
}

fn session_record_policy_view_contract() -> Value {
    json!({
        "query": "session record-policy",
        "daemon_query": "session request record-policy",
        "schema_version": "session.record_policy.v0.1",
        "authorization_model_field": "authorization_model",
        "allowed_step_kinds_field": "allowed_step_kinds",
        "frame_source_policy_field": "frame_source_policy",
        "resource_write_policy_field": "resource_write_policy",
        "safety_policy_field": "safety_policy",
        "client_guidance_field": "client_guidance",
        "live_validation_field": "live_validation",
        "does_not_enqueue": true,
        "does_not_touch_device": true,
        "does_not_capture": true,
        "does_not_start_maatouch": true,
        "does_not_read_resource_repositories": true,
        "does_not_write_resource_repositories": true
    })
}

fn session_self_heal_policy_view_contract() -> Value {
    json!({
        "query": "session self-heal-policy",
        "daemon_query": "session request self-heal-policy",
        "schema_version": "session.self_heal_policy.v0.1",
        "phase_c_field": "phase_c",
        "flow_field": "flow",
        "trigger_policy_field": "trigger_policy",
        "recovery_order_field": "recovery_order",
        "maintenance_boundary_field": "maintenance_boundary",
        "lease_and_scheduler_policy_field": "lease_and_scheduler_policy",
        "does_not_enqueue": true,
        "does_not_touch_device": true,
        "does_not_capture": true,
        "does_not_start_maatouch": true
    })
}

fn session_self_heal_plan_view_contract() -> Value {
    json!({
        "query": "session self-heal-plan [--trigger <kind>] [--to <page>]",
        "daemon_query": "session request self-heal-plan [--trigger <kind>] [--to <page>]",
        "schema_version": "session.self_heal_plan.v0.1",
        "status_field": "status",
        "trigger_field": "trigger",
        "recovery_field": "recovery",
        "escalation_field": "escalation",
        "readiness_field": "readiness",
        "queue_field": "queue",
        "lease_gate_field": "lease_gate",
        "execution_gate_field": "execution_gate",
        "execution_gate_schema_version": "session.self_heal_execution_gate.v0.1",
        "blockers_field": "blockers",
        "ready_to_execute_field": "ready_to_execute_maintenance",
        "next_actions_field": "next_actions",
        "does_not_enqueue": true,
        "does_not_touch_device": true,
        "does_not_capture": true,
        "does_not_start_maatouch": true
    })
}

fn session_phase_c_plan_view_contract() -> Value {
    json!({
        "query": "session phase-c-plan [--endpoint <url>] [--trigger <kind>] [--to <page>]",
        "daemon_query": "session request phase-c-plan [--endpoint <url>] [--trigger <kind>] [--to <page>]",
        "schema_version": "session.phase_c_plan.v0.1",
        "self_heal_field": "self_heal",
        "interaction_flow_field": "interaction_flow",
        "interaction_plan_schema_version": "session.phase_c_interaction_plan.v0.2",
        "interaction_stream_plan_contract_field": "interaction_flow.contract",
        "trusted_channel_field": "trusted_channel",
        "implementation_plan_field": "implementation_plan",
        "implementation_plan_schema_version": "session.phase_c_implementation_plan.v0.1",
        "acceptance_gates_field": "acceptance_gates",
        "acceptance_gates_schema_version": "session.phase_c_acceptance_gates.v0.1",
        "live_validation_field": "live_validation",
        "next_actions_field": "next_actions",
        "milestones_field": "milestones",
        "does_not_enqueue": true,
        "does_not_touch_device": true,
        "does_not_capture": true,
        "does_not_start_maatouch": true,
        "does_not_start_listener": true,
        "does_not_issue_tokens": true,
        "does_not_start_tls": true
    })
}

fn session_bootstrap_view_contract() -> Value {
    json!({
        "query": "session bootstrap",
        "daemon_query": "session request bootstrap",
        "schema_version": "session.bootstrap.v0.1",
        "status_diagnostics_field": "status_diagnostics",
        "status_diagnostics_capture_freshness_field": "status_diagnostics.capture_freshness",
        "status_diagnostics_self_heal_field": "status_diagnostics.self_heal",
        "status_diagnostics_interaction_flow_field": "status_diagnostics.interaction_flow",
        "status_diagnostics_trusted_channel_field": "status_diagnostics.trusted_channel",
        "status_diagnostics_phase_c_field": "status_diagnostics.phase_c",
        "status_diagnostics_validation_field": "status_diagnostics.validation",
        "readiness_field": "readiness",
        "queue_field": "queue",
        "throat_policy_field": "throat_policy",
        "capture_policy_field": "capture_policy",
        "self_heal_policy_field": "self_heal_policy",
        "validation_plan_field": "validation_plan",
        "phase_c_plan_field": "phase_c_plan",
        "api_contract_field": "api_contract",
        "access_contract_field": "access_contract",
        "does_not_enqueue": true,
        "does_not_touch_device": true,
        "does_not_capture": true,
        "does_not_start_maatouch": true
    })
}

fn run_status(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    reject_legacy_session_routing(&flags)?;
    require_runtime(global).map(|data| {
        json!({
            "state": "running",
            "runtime": data,
        })
    })
}

fn run_devices(_global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    reject_legacy_session_routing(&flags)?;
    flags.expect_positionals("devices", 0)?;
    Err(CliError::not_implemented(
        "actinglab_device_authority_retired",
        "direct ADB device discovery was retired from ActingLab; query the resident Runtime",
    ))
}

fn run_schema(args: &[String]) -> CliOutcome<Value> {
    let kind = if args.is_empty() {
        "all".to_string()
    } else {
        args.join(" ")
    };
    let data = match kind.as_str() {
        "task" => json!({
            "schema_version": "0.1",
            "required": ["schema_version", "id", "steps"],
            "step_action_types": ["complete", "click"]
        }),
        "control" => json!({
            "schema_version": "Lab-1y.control.v1",
            "execution_modes": ["navigable_route", "recognize_only", "in_page_guard"],
            "capture_backend": ["auto", "auto-fastest", "adb", "droidcast_raw", "nemu_ipc"],
            "touch_backend": ["auto", "auto-fastest", "maatouch", "minitouch", "adb_shell_input"],
            "frame_store": {
                "similarity_threshold": "default 0.95; CLI --similarity-threshold overrides control",
                "tier1_ratio": "warning watermark; CLI --tier1-ratio",
                "tier2_ratio": "temp-disk spill watermark; CLI --tier2-ratio",
                "tier3_ratio": "alarm watermark; CLI --tier3-ratio",
                "hysteresis_ratio": "release margin for active watermarks; CLI --hysteresis-ratio",
                "max_mem_bytes": "optional lab frame-store cap; CLI --max-mem-bytes",
                "os_reserve_bytes": "physical-memory reserve left for the OS; CLI --os-reserve-bytes",
                "flush_workspace_reserve_bytes": "required byte gap between tier2 and tier3; CLI --flush-workspace-reserve-bytes",
                "tier3_mode": "synchronous graceful partial-output failure; no runtime pause/resume wait is performed in this CLI"
            },
            "rules": [
                "CLI capture backend overrides control capture_backend",
                "CLI frame-store flags override control frame_store values",
                "trusted_execution is provenance and does not block semantic actions",
                "unresolved or placeholder coordinates are not executable"
            ]
        }),
        "pack" => json!({
            "schema_version": ["0.1", "0.3", "0.4", "0.5"],
            "default_match_metric": "ccorr_normed",
            "supported_match_metric": ["ccorr_normed", "ccoeff_normed"]
        }),
        "package" => json!({
            "schema_version": "0.2",
            "required_paths": ["<module>/manifest.json", "<module>/operations/<task_id>/task.json"],
            "security": ["no zip-slip", "no executable scripts", "hashes verified when declared"]
        }),
        "ledger" => json!({
            "schema_version": "actingcommand.ledger.query.v0.1",
            "commands": ["show", "events", "receipts", "diagnose", "evidence"],
            "filters": ["--run-id", "--req-id", "--instance-id"],
            "read_only": true,
            "device_io": false
        }),
        "all" => json!({
            "schemas": ["task", "control", "pack", "package", "ledger", "observe", "do", "ensure", "wait", "lab receipt"]
        }),
        other => lab2_cli::command_schema(other)
            .ok_or_else(|| CliError::usage(format!("unknown schema kind: {other}")))?,
    };
    Ok(data)
}

fn run_list(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let kind = args.first().map(String::as_str).unwrap_or("commands");
    match kind {
        "commands" => Ok(json!({ "commands": command_capabilities() })),
        "targets" | "pages" | "tasks" | "bundles" | "controls" => {
            let config = read_user_config()?;
            let root = effective_resource_root(global, &config).ok_or_else(|| {
                CliError::usage("list requires --resource-root or config resource_root")
            })?;
            list_resource_kind(&root, kind)
        }
        other => Err(CliError::usage(format!("unknown list kind: {other}"))),
    }
}

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
mod legacy_local_ledger_reader {
    use super::*;

    struct LedgerFile {
        path: PathBuf,
        read: LedgerRead,
    }

    fn run_legacy_ledger(sub: &str, global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
        let flags = FlagArgs::parse(args)?;
        match sub {
            "show" => run_ledger_show(global, &flags),
            "events" => run_ledger_events(global, &flags),
            "receipts" => run_ledger_receipts(global, &flags),
            "diagnose" => run_ledger_diagnose(global, &flags),
            "evidence" => run_ledger_evidence(global, &flags),
            other => Err(CliError::usage(format!(
                "unknown ledger command: {other}; expected show, events, receipts, diagnose, or evidence"
            ))),
        }
    }

    fn run_ledger_show(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
        let filter = LedgerFilter::from_flags(flags)?;
        let run_root = ledger_run_root(global, flags)?;
        let entries = read_ledger_files(&run_root)?;
        let limit = parse_optional_usize(flags, "--limit", 200)?;
        let mut records = Vec::new();
        let mut events = Vec::new();
        for entry in &entries {
            for record in &entry.read.records {
                if filter.matches_record(record, &entry.path, entry.read.header.as_ref()) {
                    records.push(json!({
                        "ledger_path": entry.path.display().to_string(),
                        "kind": record.kind.as_str(),
                        "record": record
                    }));
                }
            }
            for event in &entry.read.events {
                if filter.matches_event(event, &entry.path, entry.read.header.as_ref()) {
                    events.push(json!({
                        "ledger_path": entry.path.display().to_string(),
                        "event": event
                    }));
                }
            }
        }
        let record_count = records.len();
        let event_count = events.len();
        records.truncate(limit);
        events.truncate(limit);
        Ok(json!({
            "schema_version": "actingcommand.ledger.show.v0.1",
            "run_root": run_root.display().to_string(),
            "filter": filter.to_json(),
            "ledgers_scanned": entries.len(),
            "skipped_corrupt_lines": skipped_corrupt_lines(&entries),
            "record_count": record_count,
            "event_count": event_count,
            "records_more": record_count.saturating_sub(records.len()),
            "events_more": event_count.saturating_sub(events.len()),
            "records": records,
            "events": events
        }))
    }

    fn run_ledger_events(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
        let filter = LedgerFilter::from_flags(flags)?;
        let run_root = ledger_run_root(global, flags)?;
        let entries = read_ledger_files(&run_root)?;
        let limit = parse_optional_usize(flags, "--limit", 200)?;
        let mut events = Vec::new();
        for entry in &entries {
            for event in &entry.read.events {
                if filter.matches_event(event, &entry.path, entry.read.header.as_ref()) {
                    events.push(json!({
                        "ledger_path": entry.path.display().to_string(),
                        "event": event
                    }));
                }
            }
        }
        let event_count = events.len();
        events.truncate(limit);
        Ok(json!({
            "schema_version": "actingcommand.ledger.events.v0.1",
            "run_root": run_root.display().to_string(),
            "filter": filter.to_json(),
            "ledgers_scanned": entries.len(),
            "skipped_corrupt_lines": skipped_corrupt_lines(&entries),
            "event_count": event_count,
            "events_more": event_count.saturating_sub(events.len()),
            "events": events
        }))
    }

    fn run_ledger_receipts(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
        let req_id = flags.required("--req-id")?;
        let filter = LedgerFilter::for_req(req_id.clone());
        let run_root = ledger_run_root(global, flags)?;
        let entries = read_ledger_files(&run_root)?;
        let mut receipts = Vec::new();
        for entry in &entries {
            for record in &entry.read.records {
                if record.kind == LedgerRecordKind::Receipt
                    && filter.matches_record(record, &entry.path, entry.read.header.as_ref())
                {
                    receipts.push(json!({
                        "ledger_path": entry.path.display().to_string(),
                        "record": record
                    }));
                }
            }
        }
        Ok(json!({
            "schema_version": "actingcommand.ledger.receipts.v0.1",
            "run_root": run_root.display().to_string(),
            "req_id": req_id,
            "ledgers_scanned": entries.len(),
            "skipped_corrupt_lines": skipped_corrupt_lines(&entries),
            "receipt_count": receipts.len(),
            "receipts": receipts
        }))
    }

    fn run_ledger_diagnose(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
        let filter = LedgerFilter::from_flags(flags)?;
        let run_root = ledger_run_root(global, flags)?;
        let entries = read_ledger_files(&run_root)?;
        let mut matching_records = Vec::new();
        let mut matching_events = Vec::new();
        for entry in &entries {
            for record in &entry.read.records {
                if filter.matches_record(record, &entry.path, entry.read.header.as_ref()) {
                    matching_records.push((entry.path.clone(), record.clone()));
                }
            }
            for event in &entry.read.events {
                if filter.matches_event(event, &entry.path, entry.read.header.as_ref()) {
                    matching_events.push((entry.path.clone(), event.clone()));
                }
            }
        }
        let receipt_records = matching_records
            .iter()
            .filter(|(_, record)| record.kind == LedgerRecordKind::Receipt)
            .collect::<Vec<_>>();
        let finalizing_count = matching_records
            .iter()
            .filter(|(_, record)| record_type(record) == Some("finalizing"))
            .count();
        let terminal = receipt_records
            .iter()
            .rev()
            .find(|(_, record)| matches!(record_type(record), Some("finish_ok" | "finish_error")))
            .copied();
        let status = terminal
            .and_then(|(_, record)| record.payload.get("status").and_then(Value::as_str))
            .or_else(|| {
                receipt_records
                    .iter()
                    .rev()
                    .find_map(|(_, record)| record.payload.get("state").and_then(Value::as_str))
            })
            .unwrap_or(
                if matching_records.is_empty() && matching_events.is_empty() {
                    "not_found"
                } else {
                    "incomplete"
                },
            );
        let output_zip = terminal.and_then(|(_, record)| record.payload.get("output_zip").cloned());
        let output_zip_exists = output_zip
            .as_ref()
            .and_then(|zip| zip.get("path"))
            .and_then(Value::as_str)
            .map(|path| Path::new(path).exists());
        Ok(json!({
            "schema_version": "actingcommand.ledger.diagnose.v0.1",
            "run_root": run_root.display().to_string(),
            "filter": filter.to_json(),
            "status": status,
            "ledgers_scanned": entries.len(),
            "skipped_corrupt_lines": skipped_corrupt_lines(&entries),
            "record_count": matching_records.len(),
            "event_count": matching_events.len(),
            "receipt_count": receipt_records.len(),
            "finalizing_count": finalizing_count,
            "terminal_receipt": terminal.map(|(path, record)| json!({
                "ledger_path": path.display().to_string(),
                "record": record
            })),
            "output_zip": output_zip,
            "output_zip_exists": output_zip_exists,
            "diagnostics": ledger_diagnosis_warnings(
                status,
                finalizing_count,
                receipt_records.len(),
                output_zip_exists
            )
        }))
    }

    fn run_ledger_evidence(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
        let evidence_id = flags.required("--evidence-id")?;
        let run_root = ledger_run_root(global, flags)?;
        let refs = EvidenceStore::new(&run_root, true)
            .list_by_id(&evidence_id)
            .map_err(|err| CliError::device(err.to_string()))?;
        Ok(json!({
            "schema_version": "actingcommand.ledger.evidence.v0.1",
            "run_root": run_root.display().to_string(),
            "evidence_id": evidence_id,
            "evidence_count": refs.len(),
            "evidence": refs
        }))
    }

    #[derive(Debug)]
    struct LedgerFilter {
        run_id: Option<String>,
        req_id: Option<String>,
        instance_id: Option<String>,
    }

    impl LedgerFilter {
        fn from_flags(flags: &FlagArgs) -> CliOutcome<Self> {
            let filter = Self {
                run_id: flags.optional("--run-id").filter(|value| value != "true"),
                req_id: flags
                    .optional("--req-id")
                    .or_else(|| flags.optional("--request-id"))
                    .filter(|value| value != "true"),
                instance_id: flags
                    .optional("--instance-id")
                    .or_else(|| flags.optional("--instance"))
                    .filter(|value| value != "true"),
            };
            if filter.run_id.is_none() && filter.req_id.is_none() && filter.instance_id.is_none() {
                return Err(CliError::usage(
                    "ledger query requires --run-id, --req-id, or --instance-id",
                ));
            }
            Ok(filter)
        }

        fn for_req(req_id: String) -> Self {
            Self {
                run_id: None,
                req_id: Some(req_id),
                instance_id: None,
            }
        }

        fn matches_record(
            &self,
            record: &LedgerRecord,
            path: &Path,
            header: Option<&SessionHeader>,
        ) -> bool {
            self.run_id
                .as_ref()
                .is_none_or(|run_id| record_contains_id(record, path, "run_id", run_id))
                && self.req_id.as_ref().is_none_or(|req_id| {
                    record.req_id.as_deref() == Some(req_id)
                        || record_contains_id(record, path, "req_id", req_id)
                })
                && self.instance_id.as_ref().is_none_or(|instance_id| {
                    header.is_some_and(|header| header.instance == *instance_id)
                        || record_contains_id(record, path, "instance", instance_id)
                        || record_contains_id(record, path, "instance_id", instance_id)
                })
        }

        fn matches_event(
            &self,
            event: &LightEvent,
            path: &Path,
            header: Option<&SessionHeader>,
        ) -> bool {
            self.run_id
                .as_ref()
                .is_none_or(|run_id| event_contains_id(event, path, "run_id", run_id))
                && self.req_id.as_ref().is_none_or(|req_id| {
                    event.ids.get("req_id").is_some_and(|value| value == req_id)
                        || event_contains_id(event, path, "req_id", req_id)
                })
                && self.instance_id.as_ref().is_none_or(|instance_id| {
                    header.is_some_and(|header| header.instance == *instance_id)
                        || event_contains_id(event, path, "instance", instance_id)
                        || event_contains_id(event, path, "instance_id", instance_id)
                })
        }

        fn to_json(&self) -> Value {
            json!({
                "run_id": self.run_id,
                "req_id": self.req_id,
                "instance_id": self.instance_id
            })
        }
    }

    fn ledger_run_root(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<PathBuf> {
        if let Some(path) = flags.optional_path("--run-root") {
            return Ok(path);
        }
        let config = read_user_config()?;
        effective_run_root(global, &config)
            .ok_or_else(|| CliError::usage("ledger query requires --run-root or config run_root"))
    }

    fn read_ledger_files(run_root: &Path) -> CliOutcome<Vec<LedgerFile>> {
        let mut paths = Vec::new();
        collect_runtime_ledger_paths(run_root, &mut paths)?;
        paths.sort();
        paths
            .into_iter()
            .map(|path| {
                let read = LabLedger::read(&path).map_err(|err| {
                    CliError::device(format!("failed to read ledger {}: {err}", path.display()))
                })?;
                Ok(LedgerFile { path, read })
            })
            .collect()
    }

    fn collect_runtime_ledger_paths(root: &Path, paths: &mut Vec<PathBuf>) -> CliOutcome<()> {
        if !root.exists() {
            return Ok(());
        }
        for entry in fs::read_dir(root)
            .map_err(|err| CliError::device(format!("failed to read {}: {err}", root.display())))?
        {
            let entry = entry.map_err(|err| CliError::device(err.to_string()))?;
            let path = entry.path();
            if path.is_dir() {
                collect_runtime_ledger_paths(&path, paths)?;
            } else if path.file_name().and_then(|name| name.to_str()) == Some("ledger.jsonl") {
                paths.push(path);
            }
        }
        Ok(())
    }

    fn skipped_corrupt_lines(entries: &[LedgerFile]) -> usize {
        entries
            .iter()
            .map(|entry| entry.read.skipped_corrupt_lines)
            .sum()
    }

    fn record_contains_id(record: &LedgerRecord, path: &Path, key: &str, expected: &str) -> bool {
        record
            .id_chain
            .get(key)
            .is_some_and(|value| value == expected)
            || value_contains_id(&record.payload, key, expected)
            || path_contains_segment(path, expected)
    }

    fn event_contains_id(event: &LightEvent, path: &Path, key: &str, expected: &str) -> bool {
        event.ids.get(key).is_some_and(|value| value == expected)
            || value_contains_id(&event.payload, key, expected)
            || path_contains_segment(path, expected)
    }

    fn value_contains_id(value: &Value, key: &str, expected: &str) -> bool {
        match value {
            Value::Object(object) => object.iter().any(|(item_key, item)| {
                (item_key == key && item.as_str() == Some(expected))
                    || value_contains_id(item, key, expected)
            }),
            Value::Array(items) => items
                .iter()
                .any(|item| value_contains_id(item, key, expected)),
            _ => false,
        }
    }

    fn path_contains_segment(path: &Path, expected: &str) -> bool {
        path.components()
            .any(|component| component.as_os_str().to_string_lossy() == expected)
    }

    fn record_type(record: &LedgerRecord) -> Option<&str> {
        record.payload.get("record_type").and_then(Value::as_str)
    }

    fn ledger_diagnosis_warnings(
        status: &str,
        finalizing_count: usize,
        receipt_count: usize,
        output_zip_exists: Option<bool>,
    ) -> Vec<Value> {
        let mut diagnostics = Vec::new();
        if finalizing_count == 0 {
            diagnostics.push(json!({
                "severity": "warning",
                "code": "missing_finalizing",
                "message": "runtime ledger query did not find a finalizing record"
            }));
        }
        if receipt_count == 0 {
            diagnostics.push(json!({
                "severity": "warning",
                "code": "missing_receipt",
                "message": "runtime ledger query did not find a receipt record"
            }));
        }
        if status == "ok" && output_zip_exists == Some(false) {
            diagnostics.push(json!({
                "severity": "error",
                "code": "terminal_output_missing",
                "message": "ledger reports ok but the recorded output zip path does not exist"
            }));
        }
        diagnostics
    }
}

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

fn run_session_contract(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let _ = global;
    reject_legacy_session_routing(&flags)?;
    flags.expect_positionals("session contract", 0)?;
    Ok(session_access_contract())
}

fn run_session_api(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let _ = global;
    reject_legacy_session_routing(&flags)?;
    flags.expect_positionals("session api", 0)?;
    Ok(session_api_contract())
}

fn run_session_throat_policy(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    reject_legacy_session_routing(&flags)?;
    session_throat_policy_payload(global, &flags, "session throat-policy")
}

fn run_session_capture_policy(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    reject_legacy_session_routing(&flags)?;
    session_capture_policy_payload(global, &flags, "session capture-policy")
}

fn run_session_record_policy(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    reject_legacy_session_routing(&flags)?;
    session_record_policy_payload(global, &flags, "session record-policy")
}

fn run_session_self_heal_policy(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    reject_legacy_session_routing(&flags)?;
    session_self_heal_policy_payload(global, &flags, "session self-heal-policy")
}

fn session_connect_plan_next_action(
    priority: u8,
    action: &str,
    reason: &str,
    command: &str,
    read_only: bool,
) -> Value {
    json!({
        "priority": priority,
        "action": action,
        "reason": reason,
        "command": command,
        "read_only": read_only
    })
}

fn run_session_transport(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let _ = global;
    reject_legacy_session_routing(&flags)?;
    session_transport_payload(&flags)
}

fn session_transport_payload(flags: &FlagArgs) -> CliOutcome<Value> {
    match flags.positionals.first().map(String::as_str) {
        None => Ok(session_transport_contract()),
        Some("plan") => session_transport_plan_payload(&flags.without_first_positional()),
        Some("check") => session_transport_check_payload(&flags.without_first_positional()),
        Some(other) => Err(CliError::usage(format!(
            "unknown session transport command: {other}"
        ))),
    }
}

fn session_transport_plan_payload(flags: &FlagArgs) -> CliOutcome<Value> {
    flags.expect_positionals("session transport plan", 0)?;
    let endpoint = parse_optional_string_value(flags, "--endpoint")?;
    let endpoint_policy = session_transport_plan_endpoint_policy(endpoint.as_deref());
    let endpoint_policy_safe = endpoint_policy
        .get("safe_for_policy")
        .and_then(Value::as_bool);
    let has_endpoint_policy_blocker = endpoint_policy_safe == Some(false);
    let blockers = session_transport_plan_blockers(&endpoint_policy);
    let trusted_remote_gate =
        session_transport_plan_trusted_remote_gate(&endpoint_policy, &blockers);
    let next_actions = session_transport_plan_next_actions(&endpoint_policy, &blockers);
    Ok(json!({
        "schema_version": "session.transport_plan.v0.1",
        "status": if has_endpoint_policy_blocker { "blocked" } else { "reserved" },
        "mode": "trusted_channel_startup_preflight",
        "local_cli": {
            "status": "available",
            "command": "actinglab",
            "encryption_required": false,
            "authentication_required": false
        },
        "daemon_file_ipc": {
            "status": "available",
            "command": "session request <command>",
            "serialized_by_daemon": true,
            "control_requests_require_matching_lease": true
        },
        "trusted_remote": {
            "status": "reserved",
            "network_listener_implemented": false,
            "safe_to_start_listener": false,
            "ready_to_accept_remote_clients": false,
            "requires_encryption": true,
            "requires_authentication": true,
            "token_configured": env_var_non_empty(TRUSTED_REMOTE_TOKEN_ENV),
            "client_certificate_configured": env_var_non_empty(TRUSTED_REMOTE_CLIENT_CERT_ENV),
            "token_env": TRUSTED_REMOTE_TOKEN_ENV,
            "client_certificate_env": TRUSTED_REMOTE_CLIENT_CERT_ENV,
            "endpoint_policy": endpoint_policy,
            "required_before_enable": [
                "reviewed network listener implementation",
                "TLS or mutually authenticated local IPC",
                "token or client certificate authentication",
                "request serialization through the resident Session Layer",
                "audit logging for accepted remote commands"
            ]
        },
        "trusted_remote_gate": trusted_remote_gate,
        "blockers": blockers,
        "next_actions": next_actions,
        "guarantees": {
            "does_not_enqueue": true,
            "does_not_touch_device": true,
            "does_not_capture": true,
            "does_not_start_maatouch": true,
            "does_not_start_listener": true,
            "does_not_probe_tcp": true,
            "does_not_issue_tokens": true,
            "does_not_start_tls": true,
            "does_not_read_resource_repositories": true
        }
    }))
}

fn session_transport_plan_endpoint_policy(endpoint: Option<&str>) -> Value {
    let Some(endpoint) = endpoint else {
        return json!({
            "checked": false,
            "safe_for_policy": null,
            "does_not_probe_tcp": true,
            "message": "No endpoint was provided; run with --endpoint <url> to classify local versus trusted remote policy."
        });
    };
    match runtime_endpoint_policy(endpoint) {
        Ok(policy) => json!({
            "checked": true,
            "endpoint": endpoint,
            "safe_for_policy": true,
            "policy": runtime_endpoint_policy_json(&policy),
            "does_not_probe_tcp": true
        }),
        Err(err) => json!({
            "checked": true,
            "endpoint": endpoint,
            "safe_for_policy": false,
            "error_code": err.code,
            "error": err.message,
            "blocked_by": err.blocked_by,
            "does_not_probe_tcp": true
        }),
    }
}

fn session_transport_plan_blockers(endpoint_policy: &Value) -> Vec<Value> {
    let mut blockers = vec![json!({
        "kind": "trusted_remote_listener",
        "code": "trusted_remote_listener_reserved",
        "message": "Trusted remote listener is reserved and is not implemented in this offline milestone."
    })];
    if endpoint_policy
        .get("safe_for_policy")
        .and_then(Value::as_bool)
        == Some(false)
    {
        blockers.push(json!({
            "kind": "trusted_remote_endpoint_policy",
            "code": endpoint_policy.get("error_code"),
            "message": endpoint_policy.get("error"),
            "blocked_by": endpoint_policy.get("blocked_by"),
            "endpoint": endpoint_policy.get("endpoint")
        }));
    }
    blockers
}

fn session_transport_plan_trusted_remote_gate(
    endpoint_policy: &Value,
    blockers: &[Value],
) -> Value {
    let endpoint_checked = endpoint_policy
        .get("checked")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let endpoint_safe = endpoint_policy
        .get("safe_for_policy")
        .and_then(Value::as_bool);
    let endpoint_channel = endpoint_policy
        .pointer("/policy/channel")
        .and_then(Value::as_str);
    let trusted_remote_requested = endpoint_channel == Some("trusted_remote");
    let token_configured = env_var_non_empty(TRUSTED_REMOTE_TOKEN_ENV);
    let client_certificate_configured = env_var_non_empty(TRUSTED_REMOTE_CLIENT_CERT_ENV);
    let auth_material_configured = token_configured || client_certificate_configured;
    let mut blocked_reasons = blockers
        .iter()
        .map(|blocker| {
            json!({
                "kind": blocker.get("kind").cloned().unwrap_or(Value::Null),
                "code": blocker.get("code").cloned().unwrap_or(Value::Null),
                "message": blocker.get("message").cloned().unwrap_or(Value::Null)
            })
        })
        .collect::<Vec<_>>();

    if !endpoint_checked {
        blocked_reasons.push(json!({
            "kind": "trusted_remote_endpoint_policy",
            "code": "trusted_remote_endpoint_not_checked",
            "message": "Run session transport check --endpoint <url> before enabling trusted remote access."
        }));
    }
    if !auth_material_configured {
        blocked_reasons.push(json!({
            "kind": "trusted_remote_authentication",
            "code": "trusted_remote_auth_required",
            "message": "Configure a token or client certificate before trusted remote clients can authenticate."
        }));
    }

    let status = if endpoint_safe == Some(false) {
        "blocked"
    } else if endpoint_channel == Some("local_direct") {
        "not_applicable_local_direct"
    } else {
        "reserved"
    };

    json!({
        "schema_version": "session.trusted_remote_gate.v0.1",
        "status": status,
        "trusted_remote_requested": trusted_remote_requested,
        "endpoint_policy_checked": endpoint_checked,
        "endpoint_policy_safe": endpoint_safe,
        "endpoint": endpoint_policy.get("endpoint").cloned().unwrap_or(Value::Null),
        "endpoint_channel": endpoint_channel,
        "requires_encryption": true,
        "requires_authentication": true,
        "token_configured": token_configured,
        "client_certificate_configured": client_certificate_configured,
        "auth_material_configured": auth_material_configured,
        "network_listener_implemented": false,
        "tls_implemented": false,
        "token_issuer_implemented": false,
        "request_serialization_required": true,
        "audit_logging_required": true,
        "safe_to_start_listener": false,
        "safe_to_accept_remote_clients": false,
        "blocked_reason_count": blocked_reasons.len(),
        "blocked_reasons": blocked_reasons,
        "live_validation": {
            "status": "deferred",
            "deferred_code": "requires-live-device"
        },
        "guarantees": {
            "does_not_enqueue": true,
            "does_not_touch_device": true,
            "does_not_capture": true,
            "does_not_start_maatouch": true,
            "does_not_start_listener": true,
            "does_not_probe_tcp": true,
            "does_not_issue_tokens": true,
            "does_not_start_tls": true,
            "does_not_read_resource_repositories": true,
            "does_not_mark_live_validation_passed": true
        }
    })
}

fn session_transport_plan_next_actions(endpoint_policy: &Value, blockers: &[Value]) -> Value {
    let endpoint_checked = endpoint_policy
        .get("checked")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let endpoint_safe = endpoint_policy
        .get("safe_for_policy")
        .and_then(Value::as_bool);
    let token_configured = env_var_non_empty(TRUSTED_REMOTE_TOKEN_ENV);
    let client_certificate_configured = env_var_non_empty(TRUSTED_REMOTE_CLIENT_CERT_ENV);
    let auth_material_configured = token_configured || client_certificate_configured;
    let mut ordered = Vec::new();
    let mut priority = 1;

    if !endpoint_checked {
        ordered.push(session_connect_plan_next_action(
            priority,
            "classify_endpoint_policy",
            "Classify the intended trusted remote endpoint before any listener or client transport work.",
            "session transport check --endpoint <url>",
            true,
        ));
        priority += 1;
    }

    if endpoint_safe == Some(false) {
        ordered.push(session_connect_plan_next_action(
            priority,
            "review_endpoint_policy_blocker",
            "Fix the trusted remote endpoint policy before any remote channel can be enabled.",
            "session transport check --endpoint <url>",
            true,
        ));
        priority += 1;
    }

    if !auth_material_configured {
        ordered.push(session_connect_plan_next_action(
            priority,
            "prepare_remote_auth_material",
            "Configure a token or client certificate before remote clients can authenticate.",
            "configure ACTINGLAB_TRUSTED_REMOTE_TOKEN or ACTINGLAB_TRUSTED_REMOTE_CLIENT_CERT",
            false,
        ));
        priority += 1;
    }

    ordered.push(session_connect_plan_next_action(
        priority,
        "review_listener_and_tls_design",
        "Review the network listener, TLS boundary, and authentication model before implementation.",
        "session transport plan [--endpoint <url>]",
        true,
    ));
    priority += 1;

    ordered.push(session_connect_plan_next_action(
        priority,
        "review_request_serialization_and_audit",
        "Remote commands must serialize through the resident Session Layer and leave an audit trail.",
        "session api",
        true,
    ));
    priority += 1;

    ordered.push(session_connect_plan_next_action(
        priority,
        "review_live_acceptance_checklist",
        "Trusted remote transport still requires live listener, TLS, auth, and operator validation later.",
        "session validation-plan",
        true,
    ));

    json!({
        "schema_version": "session.transport_next_actions.v0.1",
        "status": if endpoint_safe == Some(false) { "blocked" } else { "reserved" },
        "ordered": ordered,
        "trusted_remote": {
            "status": "reserved",
            "network_listener_implemented": false,
            "ready_to_accept_remote_clients": false,
            "endpoint_policy_checked": endpoint_checked,
            "endpoint_policy_safe": endpoint_safe,
            "endpoint": endpoint_policy.get("endpoint").cloned().unwrap_or(Value::Null),
            "token_configured": token_configured,
            "client_certificate_configured": client_certificate_configured,
            "auth_material_configured": auth_material_configured,
            "blocker_count": blockers.len()
        },
        "required_before_enable": [
            "reviewed network listener implementation",
            "TLS or mutually authenticated local IPC",
            "token or client certificate authentication",
            "request serialization through the resident Session Layer",
            "audit logging for accepted remote commands"
        ],
        "local_cli": {
            "status": "available",
            "encryption_required": false,
            "authentication_required": false
        },
        "daemon_file_ipc": {
            "status": "available",
            "serialized_by_daemon": true,
            "control_requests_require_matching_lease": true
        },
        "live_validation": {
            "status": "deferred",
            "deferred_code": "requires-live-device",
            "must_not_mark_live_pass_from_offline_checks": true
        },
        "guarantees": {
            "does_not_enqueue": true,
            "does_not_touch_device": true,
            "does_not_capture": true,
            "does_not_start_maatouch": true,
            "does_not_start_listener": true,
            "does_not_probe_tcp": true,
            "does_not_issue_tokens": true,
            "does_not_start_tls": true,
            "does_not_read_resource_repositories": true,
            "does_not_mark_live_validation_passed": true
        }
    })
}

fn session_transport_check_payload(flags: &FlagArgs) -> CliOutcome<Value> {
    flags.expect_positionals("session transport check", 0)?;
    let endpoint = flags.required("--endpoint")?;
    let check = runtime_endpoint_check(&endpoint);
    Ok(json!({
        "schema_version": "session.transport_check.v0.1",
        "endpoint": endpoint,
        "check": check,
        "safe_to_connect": check.get("ok").and_then(Value::as_bool).unwrap_or(false),
        "does_not_start_listener": true
    }))
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

fn run_session_record(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    run_session_record_inner(global, args, None)
}

fn run_session_record_inner(
    global: &GlobalOptions,
    args: &[String],
    forced_state_dir: Option<&Path>,
) -> CliOutcome<Value> {
    let action = args.first().map(String::as_str).ok_or_else(|| {
        CliError::usage(
            "session record requires start|status|stop|step|candidates|amend|build-task|promote",
        )
    })?;
    let flags = FlagArgs::parse(&args[1..])?;
    let config = read_user_config()?;
    let state_dir = forced_state_dir
        .map(Path::to_path_buf)
        .map(Ok)
        .unwrap_or_else(|| session_state_dir_from_flags(&flags))?;
    fs::create_dir_all(&state_dir).map_err(|err| {
        CliError::runtime_not_running(format!(
            "failed to create session state dir {}: {err}",
            state_dir.display()
        ))
    })?;
    let instance_id = resolve_instance_id_for_flags(global, &config, &flags)?;
    let record_path = session_record_path(&state_dir, &instance_id);
    match action {
        "start" => {
            let task_id = flags.required("--task-id")?;
            if task_id.trim().is_empty() {
                return Err(CliError::usage("--task-id must not be empty"));
            }
            if record_path.exists()
                && !flags.bool("--force")
                && let Some(existing) = read_json_file::<SessionRecordContext>(&record_path)?
                && existing.status == "active"
            {
                return Err(CliError::safety_blocked(
                    "record_session_active",
                    format!(
                        "recording session already active for {} with task {}",
                        existing.instance, existing.task_id
                    ),
                    &["session_record"],
                ));
            }
            let record = new_session_record(&instance_id, &task_id, &flags);
            write_json_file_atomic(&record_path, &record)?;
            Ok(json!({
                "status": "started",
                "record": record,
                "path": record_path.display().to_string(),
                "auto_recording": false
            }))
        }
        "status" => Ok(json!({
            "status": if record_path.exists() { "available" } else { "not_started" },
            "instance": instance_id,
            "record": read_json_file::<SessionRecordContext>(&record_path)?,
            "path": record_path.display().to_string()
        })),
        "stop" => {
            let Some(mut record) = read_json_file::<SessionRecordContext>(&record_path)? else {
                return Ok(json!({
                    "status": "not_started",
                    "instance": instance_id,
                    "path": record_path.display().to_string()
                }));
            };
            record.status = "stopped".to_string();
            record.updated_at_unix_ms = current_unix_ms();
            write_json_file_atomic(&record_path, &record)?;
            Ok(json!({
                "status": "stopped",
                "record": record,
                "path": record_path.display().to_string()
            }))
        }
        "step" => {
            let Some(mut record) = read_json_file::<SessionRecordContext>(&record_path)? else {
                return Err(CliError::safety_blocked(
                    "record_session_not_active",
                    format!(
                        "no recording session exists for {}; run session record start first",
                        instance_id
                    ),
                    &["session_record"],
                ));
            };
            if record.status != "active" {
                return Err(CliError::safety_blocked(
                    "record_session_not_active",
                    format!(
                        "recording session for {} is {}, not active",
                        instance_id, record.status
                    ),
                    &["session_record"],
                ));
            }
            let step_context = SessionRecordStepContext {
                global,
                config: &config,
                record: &record,
                state_dir: &state_dir,
            };
            let step = new_session_record_step(&step_context, &flags)?;
            record.steps.push(step.clone());
            record.updated_at_unix_ms = current_unix_ms();
            write_json_file_atomic(&record_path, &record)?;
            Ok(json!({
                "status": "step_recorded",
                "step": step,
                "record": record,
                "path": record_path.display().to_string(),
                "step_count": record.steps.len()
            }))
        }
        "amend" => {
            let Some(mut record) = read_json_file::<SessionRecordContext>(&record_path)? else {
                return Err(CliError::safety_blocked(
                    "record_session_not_active",
                    format!(
                        "no recording session exists for {}; run session record start first",
                        instance_id
                    ),
                    &["session_record"],
                ));
            };
            if record.status != "active" {
                return Err(CliError::safety_blocked(
                    "record_session_not_active",
                    format!(
                        "recording session for {} is {}, not active",
                        instance_id, record.status
                    ),
                    &["session_record"],
                ));
            }
            let amend_context = SessionRecordAmendContext {
                record_id: record.record_id.clone(),
                state_dir: state_dir.clone(),
            };
            if let Some(diagnostics_path) = session_record_drift_diagnostics_path(&flags)? {
                let amend = amend_session_record_from_drift_diagnostics(
                    &amend_context,
                    &mut record,
                    &flags,
                    diagnostics_path,
                )?;
                record.updated_at_unix_ms = current_unix_ms();
                write_json_file_atomic(&record_path, &record)?;
                return Ok(json!({
                    "status": "drift_diagnostics_amended",
                    "amend": amend,
                    "record": record,
                    "path": record_path.display().to_string(),
                    "step_count": record.steps.len()
                }));
            }
            let step_id = record_amend_step_id(&flags)?;
            let Some(step) = record.steps.iter_mut().find(|step| step.step_id == step_id) else {
                return Err(CliError::safety_blocked(
                    "record_step_not_found",
                    format!("recording step does not exist: {step_id}"),
                    &["session_record"],
                ));
            };
            amend_session_record_step(&amend_context, step, &flags)?;
            record.updated_at_unix_ms = current_unix_ms();
            let amended_step = step.clone();
            write_json_file_atomic(&record_path, &record)?;
            Ok(json!({
                "status": "step_amended",
                "step": amended_step,
                "record": record,
                "path": record_path.display().to_string(),
                "step_count": record.steps.len()
            }))
        }
        "candidates" | "candidate-list" => {
            let Some(record) = read_json_file::<SessionRecordContext>(&record_path)? else {
                return Err(CliError::safety_blocked(
                    "record_session_not_active",
                    format!(
                        "no recording session exists for {}; run session record start first",
                        instance_id
                    ),
                    &["session_record"],
                ));
            };
            let step_id = record_candidates_step_id(&flags)?;
            let Some(step) = record.steps.iter().find(|step| step.step_id == step_id) else {
                return Err(CliError::safety_blocked(
                    "record_step_not_found",
                    format!("recording step does not exist: {step_id}"),
                    &["session_record"],
                ));
            };
            session_record_candidate_report(&record, step, &record_path)
        }
        "build-task" => {
            build_session_record_task(global, &config, &flags, &record_path, &instance_id)
        }
        "promote" | "publish" => {
            promote_session_record_task(global, &config, &flags, &record_path, &instance_id)
        }
        other => Err(CliError::usage(format!(
            "unknown session record action: {other}"
        ))),
    }
}

fn build_session_record_task(
    global: &GlobalOptions,
    config: &UserConfig,
    flags: &FlagArgs,
    record_path: &Path,
    instance_id: &str,
) -> CliOutcome<Value> {
    let Some(record) = read_json_file::<SessionRecordContext>(record_path)? else {
        return Err(CliError::safety_blocked(
            "record_session_not_active",
            format!(
                "no recording session exists for {instance_id}; run session record start first"
            ),
            &["session_record"],
        ));
    };
    if !matches!(record.status.as_str(), "active" | "stopped") {
        return Err(CliError::safety_blocked(
            "record_session_not_active",
            format!(
                "recording session for {} is {}, not active or stopped",
                record.instance, record.status
            ),
            &["session_record"],
        ));
    }
    let out = flags.required_path("--out")?;
    let dry_run = global.dry_run || flags.bool("--dry-run");
    let (game, server, locale) = session_record_selector(global, config, flags, instance_id)?;
    let state_dir = record_path.parent().unwrap_or_else(|| Path::new("."));
    let draft =
        session_record_build_draft(&record, flags, &out, &game, &server, &locale, state_dir)?;
    let authoring = session_record_authoring_input(&record, &draft)?;
    if !dry_run {
        resource_authoring::materialize_record_authoring(&out, &authoring)?;
    }
    Ok(json!({
        "status": if dry_run { "validated" } else { "built" },
        "mode": "session-record-build-task",
        "dry_run": dry_run,
        "instance": instance_id,
        "record_id": record.record_id,
        "task_id": record.task_id,
        "game": game,
        "server": server,
        "locale": locale,
        "out": out.display().to_string(),
        "task_dir": draft.task_dir.display().to_string(),
        "task_path": draft.task_path.display().to_string(),
        "resources_path": draft.resources_path.display().to_string(),
        "anchor_count": draft.bundle.get("anchors").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
        "color_probe_count": draft.bundle.get("color_probes").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
        "verify_template_count": draft.bundle.get("verify_templates").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
        "operation_count": draft.bundle.get("operations").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
        "asset_count": draft.assets.len(),
        "assets": draft.assets.iter().map(|asset| {
            json!({
                "template": &asset.template,
                "source": asset.source.display().to_string(),
                "destination": asset.destination.display().to_string()
            })
        }).collect::<Vec<_>>(),
        "bundle": draft.bundle
    }))
}

fn promote_session_record_task(
    global: &GlobalOptions,
    config: &UserConfig,
    flags: &FlagArgs,
    record_path: &Path,
    instance_id: &str,
) -> CliOutcome<Value> {
    let Some(record) = read_json_file::<SessionRecordContext>(record_path)? else {
        return Err(CliError::safety_blocked(
            "record_session_not_active",
            format!(
                "no recording session exists for {instance_id}; run session record start first"
            ),
            &["session_record"],
        ));
    };
    if !matches!(record.status.as_str(), "active" | "stopped") {
        return Err(CliError::safety_blocked(
            "record_session_not_active",
            format!(
                "recording session for {} is {}, not active or stopped",
                record.instance, record.status
            ),
            &["session_record"],
        ));
    }
    let repo = flags.required_path("--repo")?;
    let resource_root = resolve_resource_root(&repo);
    if resource_root.layout == "unresolved" {
        return Err(CliError::usage(
            "session record promote requires --repo to be an existing resource root or a repository containing ours/",
        ));
    }
    let dry_run = global.dry_run || flags.bool("--dry-run");
    let force = flags.bool("--force");
    let (game, server, locale) = session_record_selector(global, config, flags, instance_id)?;
    let state_dir = record_path.parent().unwrap_or_else(|| Path::new("."));
    let draft = session_record_build_draft(
        &record,
        flags,
        &resource_root.root,
        &game,
        &server,
        &locale,
        state_dir,
    )?;
    let authoring_input = session_record_authoring_input(&record, &draft)?;
    if draft.task_dir.exists() && !force {
        return Err(CliError::safety_blocked(
            "record_promote_target_exists",
            format!(
                "record promote target task directory already exists: {}; use --force to replace it",
                draft.task_dir.display()
            ),
            &["session_record", "resource_repo"],
        ));
    }
    let resources_existed = draft.resources_path.exists();
    let (resources_action, authoring) = if dry_run {
        let action = if resources_existed {
            "would_preserve"
        } else {
            "would_create"
        };
        (action, Value::Null)
    } else {
        let client = RuntimeClient::connect(RuntimeClientConfig::new(
            runtime_state_root()?,
            EventActor::Lab,
            EventSource::Lab,
        ))
        .map_err(runtime_slice_cli::map_runtime_error)?;
        let target_label = format!(
            "{}-{}-resources",
            safe_file_stem(&game),
            safe_file_stem(&server)
        );
        let output = resource_authoring::publish_record_authoring(
            &client,
            &resource_root.root,
            target_label,
            &authoring_input,
            &game,
            &server,
            force,
        )?;
        let output = serde_json::to_value(output).map_err(|error| {
            CliError::usage(format!(
                "failed to serialize resource authoring receipt: {error}"
            ))
        })?;
        (
            if resources_existed {
                "preserved"
            } else {
                "created"
            },
            output,
        )
    };
    Ok(json!({
        "status": if dry_run { "validated" } else { "promoted" },
        "mode": "session-record-promote",
        "dry_run": dry_run,
        "force": force,
        "instance": instance_id,
        "record_id": record.record_id,
        "task_id": record.task_id,
        "game": game,
        "server": server,
        "locale": locale,
        "repo": resource_root.input.display().to_string(),
        "resource_root": resource_root.root.display().to_string(),
        "resource_layout": resource_root.layout,
        "task_dir": draft.task_dir.display().to_string(),
        "task_path": draft.task_path.display().to_string(),
        "resources_path": draft.resources_path.display().to_string(),
        "resources_action": resources_action,
        "authoring": authoring,
        "anchor_count": draft.bundle.get("anchors").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
        "color_probe_count": draft.bundle.get("color_probes").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
        "verify_template_count": draft.bundle.get("verify_templates").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
        "operation_count": draft.bundle.get("operations").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
        "asset_count": draft.assets.len(),
        "assets": draft.assets.iter().map(|asset| {
            json!({
                "template": &asset.template,
                "source": asset.source.display().to_string(),
                "destination": asset.destination.display().to_string()
            })
        }).collect::<Vec<_>>()
    }))
}

fn session_record_build_draft(
    record: &SessionRecordContext,
    flags: &FlagArgs,
    out: &Path,
    game: &str,
    server: &str,
    locale: &str,
    state_dir: &Path,
) -> CliOutcome<SessionRecordBuildDraft> {
    let task_dir_name = safe_task_dir_name(&record.task_id)?;
    let task_dir = out.join("operations").join(&task_dir_name);
    let resources_path = out.join("operations").join("resources.json");
    let task_path = task_dir.join("task.json");
    let assets_dir = task_dir.join("assets");
    let mut assets = Vec::new();
    let mut anchors = Vec::new();
    let mut anchor_templates = BTreeMap::new();
    let mut resolution = parse_record_build_resolution(flags)?;

    for step in &record.steps {
        if let SessionRecordStepData::Anchor {
            id,
            region,
            color_check,
            threshold,
            frame_provenance,
            artifact,
            evaluation,
        } = &step.data
        {
            let artifact = artifact.as_deref().ok_or_else(|| {
                CliError::usage(format!(
                    "record build-task cannot build anchor '{}' without a frame artifact",
                    step.step_id
                ))
            })?;
            if evaluation.status != "passed" {
                return Err(CliError::usage(format!(
                    "record build-task requires anchor '{}' to pass backtest; status is {}",
                    step.step_id, evaluation.status
                )));
            }
            if resolution.is_none()
                && let Some(provenance) = frame_provenance.as_deref()
            {
                resolution = Some((provenance.width, provenance.height));
            }
            let source = ensure_path_within(
                state_dir,
                Path::new(&artifact.path),
                "record build-task artifact source",
                &["record", "artifact_path"],
            )?;
            if !source.is_file() {
                return Err(CliError::usage(format!(
                    "record build-task anchor '{}' artifact is missing: {}",
                    step.step_id,
                    source.display()
                )));
            }
            let color_check_value = session_record_bundle_color_check(
                *color_check,
                frame_provenance.as_deref(),
                &artifact.region,
                &step.step_id,
            )?;
            let asset_name = format!(
                "anchor-{}-{}.png",
                safe_file_stem(&step.step_id),
                safe_file_stem(id)
            );
            let destination = assets_dir.join(&asset_name);
            let template = format!("assets/{asset_name}");
            assets.push(SessionRecordBuildAsset {
                source,
                destination,
                template: template.clone(),
            });
            anchor_templates.insert(id.clone(), template.clone());
            anchors.push(json!({
                "id": id,
                "template": template,
                "region": region,
                "threshold": threshold.unwrap_or_else(|| {
                    evaluation
                        .backtest
                        .as_ref()
                        .map(|backtest| f64::from(backtest.threshold))
                        .unwrap_or(0.95)
                }),
                "color_check": color_check_value,
                "provenance": {
                    "record_step_id": step.step_id,
                    "record_color_check_requested": color_check,
                    "frame_provenance": frame_provenance,
                    "artifact": artifact,
                    "evaluation": evaluation
                }
            }));
        }
    }

    let mut color_probes = Vec::new();
    for step in &record.steps {
        if let SessionRecordStepData::ColorProbe {
            id,
            region,
            expected,
            frame_provenance,
            evaluation,
        } = &step.data
        {
            let expected = expected.ok_or_else(|| {
                CliError::usage(format!(
                    "record build-task cannot build color-probe '{}' without expected color; provide --frame or --capture when recording it",
                    step.step_id
                ))
            })?;
            if evaluation.status != "passed" {
                return Err(CliError::usage(format!(
                    "record build-task requires color-probe '{}' to pass evaluation; status is {}",
                    step.step_id, evaluation.status
                )));
            }
            color_probes.push(json!({
                "id": id,
                "region": region,
                "expected": expected,
                "provenance": {
                    "record_step_id": step.step_id,
                    "frame_provenance": frame_provenance,
                    "evaluation": evaluation,
                    "created_at_unix_ms": step.created_at_unix_ms,
                    "updated_at_unix_ms": step.updated_at_unix_ms
                }
            }));
        }
    }

    let mut verify_templates = Vec::new();
    for step in &record.steps {
        if let SessionRecordStepData::VerifyTemplate {
            id,
            region,
            threshold,
            frame_provenance,
            artifact,
            evaluation,
        } = &step.data
        {
            let artifact = artifact.as_deref().ok_or_else(|| {
                CliError::usage(format!(
                    "record build-task cannot build verify-template '{}' without a frame artifact",
                    step.step_id
                ))
            })?;
            if evaluation.status != "passed" {
                return Err(CliError::usage(format!(
                    "record build-task requires verify-template '{}' to pass backtest; status is {}",
                    step.step_id, evaluation.status
                )));
            }
            if resolution.is_none()
                && let Some(provenance) = frame_provenance.as_deref()
            {
                resolution = Some((provenance.width, provenance.height));
            }
            let source = ensure_path_within(
                state_dir,
                Path::new(&artifact.path),
                "record build-task artifact source",
                &["record", "artifact_path"],
            )?;
            if !source.is_file() {
                return Err(CliError::usage(format!(
                    "record build-task verify-template '{}' artifact is missing: {}",
                    step.step_id,
                    source.display()
                )));
            }
            let asset_name = format!(
                "verify-template-{}-{}.png",
                safe_file_stem(&step.step_id),
                safe_file_stem(id)
            );
            let destination = assets_dir.join(&asset_name);
            let template = format!("assets/{asset_name}");
            assets.push(SessionRecordBuildAsset {
                source,
                destination,
                template: template.clone(),
            });
            verify_templates.push(json!({
                "id": id,
                "template": template,
                "region": region,
                "threshold": threshold.unwrap_or_else(|| {
                    evaluation
                        .backtest
                        .as_ref()
                        .map(|backtest| f64::from(backtest.threshold))
                        .unwrap_or(0.95)
                }),
                "provenance": {
                    "record_step_id": step.step_id,
                    "frame_provenance": frame_provenance,
                    "artifact": artifact,
                    "evaluation": evaluation
                }
            }));
        }
    }

    let mut operations = Vec::new();
    for step in &record.steps {
        if let SessionRecordStepData::Operation {
            from,
            to,
            click,
            destructive,
        } = &step.data
        {
            let click_value = session_record_bundle_click(click, &step.step_id)?;
            validate_record_build_page_ref("from", from, &anchor_templates, &step.step_id)?;
            if let Some(to) = to {
                validate_record_build_page_ref("to", to, &anchor_templates, &step.step_id)?;
            }
            let verify_template = to.as_ref().and_then(|to| anchor_templates.get(to)).cloned();
            let guard = session_record_operation_guard(from, click, &anchor_templates)?;
            operations.push(json!({
                "id": step.step_id,
                "purpose": format!("recorded operation from {from}"),
                "from": from,
                "to": to,
                "click": click_value,
                "verify_template": verify_template,
                "guard": guard,
                "consumes": [],
                "produces": [],
                "destructive": destructive,
                "provenance": {
                    "record_step_id": step.step_id,
                    "created_at_unix_ms": step.created_at_unix_ms,
                    "updated_at_unix_ms": step.updated_at_unix_ms
                }
            }));
        }
    }
    if operations.is_empty() {
        return Err(CliError::usage(
            "record build-task requires at least one operation step",
        ));
    }
    let (width, height) = resolution.ok_or_else(|| {
        CliError::usage("record build-task requires --resolution <width>x<height> when no frame-backed anchor is available")
    })?;
    validate_record_build_operation_clicks(&operations, width, height)?;
    let entry_page = flags
        .optional("--entry-page")
        .filter(|value| value != "true")
        .or_else(|| {
            operations
                .first()
                .and_then(|operation| operation.get("from"))
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    let target_page = flags
        .optional("--target-page")
        .filter(|value| value != "true")
        .or_else(|| {
            operations
                .iter()
                .rev()
                .find_map(|operation| operation.get("to").and_then(Value::as_str))
                .map(str::to_string)
        });
    if let Some(entry_page) = &entry_page {
        validate_record_build_page_ref(
            "entry_page",
            entry_page,
            &anchor_templates,
            &record.task_id,
        )?;
    }
    if let Some(target_page) = &target_page {
        validate_record_build_page_ref(
            "target_page",
            target_page,
            &anchor_templates,
            &record.task_id,
        )?;
    }
    let recorded_at_unix_ms = record
        .steps
        .iter()
        .map(|step| step.created_at_unix_ms)
        .min()
        .unwrap_or(record.started_at_unix_ms);
    let bundle = json!({
        "schema_version": "0.5",
        "task_id": record.task_id,
        "game": game,
        "server_scope": [server],
        "locale": locale,
        "goal": flags
            .optional("--goal")
            .filter(|value| value != "true")
            .unwrap_or_else(|| format!("recorded from {}", record.record_id)),
        "coordinate_space": {"width": width, "height": height},
        "defaults": {
            "template_threshold": parse_optional_unit_f64(flags, "--default-threshold")?.unwrap_or(0.95),
            "color_max_distance": 20.0,
            "match_metric": flags
                .optional("--metric")
                .filter(|value| value != "true")
                .unwrap_or_else(|| "ccorr_normed".to_string())
        },
        "anchors": anchors,
        "color_probes": color_probes,
        "verify_templates": verify_templates,
        "entry_page": entry_page,
        "target_page": target_page,
        "operations": operations,
        "provenance": {
            "source": "session_record",
            "game": game,
            "server": server,
            "locale": locale,
            "resolution": {"width": width, "height": height},
            "recorded_at_unix_ms": recorded_at_unix_ms,
            "runtime_version": RUNTIME_VERSION,
            "client_version": flags.optional("--client-version").filter(|value| value != "true"),
            "record_id": record.record_id,
            "record_status": record.status,
            "instance": record.instance,
            "holder": record.holder,
            "lease_id": record.lease_id,
            "started_at_unix_ms": record.started_at_unix_ms,
            "updated_at_unix_ms": record.updated_at_unix_ms
        }
    });
    Ok(SessionRecordBuildDraft {
        root: out.to_path_buf(),
        task_dir_name,
        bundle,
        task_dir,
        task_path,
        resources_path,
        assets,
    })
}

fn session_record_authoring_input(
    record: &SessionRecordContext,
    draft: &SessionRecordBuildDraft,
) -> CliOutcome<resource_authoring::RecordAuthoringInput> {
    let assets = draft
        .assets
        .iter()
        .map(|asset| {
            let relative_path = asset.destination.strip_prefix(&draft.root).map_err(|_| {
                CliError::safety_blocked(
                    "authoring_asset_path_escape",
                    format!(
                        "record authoring asset {} is outside target root {}",
                        asset.destination.display(),
                        draft.root.display()
                    ),
                    &["session_record", "resource_authoring"],
                )
            })?;
            Ok(resource_authoring::RecordAuthoringAsset {
                source: asset.source.clone(),
                relative_path: relative_path.to_path_buf(),
            })
        })
        .collect::<CliOutcome<Vec<_>>>()?;
    Ok(resource_authoring::RecordAuthoringInput {
        record_id: record.record_id.clone(),
        task_id: record.task_id.clone(),
        task_dir_name: draft.task_dir_name.clone(),
        bundle: draft.bundle.clone(),
        assets,
    })
}

fn session_record_bundle_color_check(
    enabled: bool,
    frame_provenance: Option<&SessionRecordFrameProvenance>,
    rect: &SessionRecordRect,
    step_id: &str,
) -> CliOutcome<Value> {
    if !enabled {
        return Ok(Value::Null);
    }
    let Some(frame_provenance) = frame_provenance else {
        return Err(CliError::usage(format!(
            "record build-task anchor '{step_id}' requested color_check but has no frame provenance"
        )));
    };
    let source_frame = read_session_record_source_frame_from_provenance(frame_provenance)?;
    let expected = mean_session_record_rect_rgb(&source_frame.frame, rect)?;
    Ok(json!({
        "region": {
            "mode": "rect",
            "rect": rect
        },
        "expected": expected
    }))
}

fn mean_session_record_rect_rgb(frame: &Frame, rect: &SessionRecordRect) -> CliOutcome<[u8; 3]> {
    let crop = crop_frame_rect(frame, rect)?;
    let stride = match crop.pixel_format {
        PixelFormat::Rgb8 => 3usize,
        PixelFormat::Rgba8 => 4usize,
    };
    let mut sum = [0_u64; 3];
    for pixel in crop.pixels.chunks_exact(stride) {
        sum[0] += u64::from(pixel[0]);
        sum[1] += u64::from(pixel[1]);
        sum[2] += u64::from(pixel[2]);
    }
    let count = u64::from(crop.width)
        .checked_mul(u64::from(crop.height))
        .ok_or_else(|| CliError::usage("record color_check pixel count overflow"))?;
    if count == 0 {
        return Err(CliError::usage("record color_check region has no pixels"));
    }
    Ok([
        (sum[0] / count) as u8,
        (sum[1] / count) as u8,
        (sum[2] / count) as u8,
    ])
}

fn session_record_bundle_click(click: &SessionRecordClick, step_id: &str) -> CliOutcome<Value> {
    match click {
        SessionRecordClick::Coord { x, y } => Ok(json!({
            "kind": "point",
            "x": x,
            "y": y
        })),
        SessionRecordClick::Swipe {
            from,
            to,
            duration_ms,
        } => Ok(json!({
            "kind": "drag",
            "from": from,
            "to": to,
            "duration_ms": duration_ms
        })),
        SessionRecordClick::LongPress { x, y, duration_ms } => Ok(json!({
            "kind": "long_press",
            "x": x,
            "y": y,
            "duration_ms": duration_ms
        })),
        SessionRecordClick::Target { target } => Err(CliError::usage(format!(
            "record build-task cannot build operation '{step_id}' with unresolved target click '{target}'"
        ))),
    }
}

fn session_record_operation_guard(
    from: &str,
    click: &SessionRecordClick,
    anchors: &BTreeMap<String, String>,
) -> CliOutcome<Value> {
    let (anchor_id, template) = resolve_record_guard_anchor(from, anchors)?;
    Ok(json!({
        "page_id": from,
        "target_id": session_record_anchor_target_id(&anchor_id),
        "expected_rect": session_record_click_expected_rect(click)?,
        "verify_template": template
    }))
}

fn resolve_record_guard_anchor(
    page: &str,
    anchors: &BTreeMap<String, String>,
) -> CliOutcome<(String, String)> {
    if page == "any" {
        return Err(CliError::usage(
            "record build-task cannot build a guarded coordinate operation from page 'any'",
        ));
    }
    if let Some(template) = anchors.get(page) {
        return Ok((page.to_string(), template.clone()));
    }
    let prefix = format!("{page}_");
    anchors
        .iter()
        .find(|(anchor_id, _)| anchor_id.starts_with(&prefix))
        .map(|(anchor_id, template)| (anchor_id.clone(), template.clone()))
        .ok_or_else(|| {
            CliError::usage(format!(
                "record build-task cannot build guard for page '{page}' without a matching anchor"
            ))
        })
}

fn session_record_anchor_target_id(anchor_id: &str) -> String {
    format!("page/{anchor_id}")
}

fn session_record_click_expected_rect(click: &SessionRecordClick) -> CliOutcome<Value> {
    match click {
        SessionRecordClick::Coord { x, y } => Ok(json!({
            "x": x,
            "y": y,
            "width": 1,
            "height": 1
        })),
        SessionRecordClick::Swipe { from, .. } => Ok(json!(from)),
        SessionRecordClick::LongPress { x, y, .. } => Ok(json!({
            "x": x,
            "y": y,
            "width": 1,
            "height": 1
        })),
        SessionRecordClick::Target { target } => Err(CliError::usage(format!(
            "record build-task cannot build a guard for unresolved target click '{target}'"
        ))),
    }
}

fn validate_record_build_operation_clicks(
    operations: &[Value],
    width: u32,
    height: u32,
) -> CliOutcome<()> {
    for operation in operations {
        let operation_id = operation
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("<unknown>");
        let Some(click) = operation.get("click").and_then(Value::as_object) else {
            return Err(CliError::usage(format!(
                "record build-task operation '{operation_id}' is missing click object"
            )));
        };
        match click.get("kind").and_then(Value::as_str) {
            Some("point") | Some("long_press") => {
                let x = click.get("x").and_then(Value::as_i64).ok_or_else(|| {
                    CliError::usage(format!(
                        "record build-task operation '{operation_id}' click.x is missing or not an integer"
                    ))
                })?;
                let y = click.get("y").and_then(Value::as_i64).ok_or_else(|| {
                    CliError::usage(format!(
                        "record build-task operation '{operation_id}' click.y is missing or not an integer"
                    ))
                })?;
                validate_record_build_point(operation_id, x, y, width, height)?;
            }
            Some("drag") => {
                for key in ["from", "to"] {
                    let rect = click.get(key).and_then(Value::as_object).ok_or_else(|| {
                        CliError::usage(format!(
                            "record build-task operation '{operation_id}' drag.{key} is missing"
                        ))
                    })?;
                    let x = rect.get("x").and_then(Value::as_i64).ok_or_else(|| {
                        CliError::usage(format!(
                            "record build-task operation '{operation_id}' drag.{key}.x is missing"
                        ))
                    })?;
                    let y = rect.get("y").and_then(Value::as_i64).ok_or_else(|| {
                        CliError::usage(format!(
                            "record build-task operation '{operation_id}' drag.{key}.y is missing"
                        ))
                    })?;
                    validate_record_build_point(operation_id, x, y, width, height)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_record_build_point(
    operation_id: &str,
    x: i64,
    y: i64,
    width: u32,
    height: u32,
) -> CliOutcome<()> {
    if x < 0 || y < 0 || x >= i64::from(width) || y >= i64::from(height) {
        return Err(CliError::usage(format!(
            "record build-task operation '{operation_id}' click point {x},{y} is outside coordinate_space {width}x{height}"
        )));
    }
    Ok(())
}

fn validate_record_build_page_ref(
    label: &str,
    page: &str,
    anchors: &BTreeMap<String, String>,
    owner_id: &str,
) -> CliOutcome<()> {
    if page == "any" {
        return Ok(());
    }
    if anchors.contains_key(page) {
        return Ok(());
    }
    let prefix = format!("{page}_");
    if anchors
        .keys()
        .any(|anchor_id| anchor_id.starts_with(&prefix))
    {
        return Ok(());
    }
    Err(CliError::usage(format!(
        "record build-task {label} page '{page}' in '{owner_id}' has no matching anchor"
    )))
}

fn session_record_selector(
    global: &GlobalOptions,
    config: &UserConfig,
    flags: &FlagArgs,
    instance_id: &str,
) -> CliOutcome<(String, String, String)> {
    let instance = config.instances.get(instance_id);
    let game = flags
        .optional("--game")
        .filter(|value| value != "true")
        .or_else(|| global.game.clone())
        .or_else(|| instance.and_then(|instance| instance.game.clone()))
        .ok_or_else(|| {
            CliError::usage("record build-task requires --game or configured instance.<id>.game")
        })?;
    let game = canonical_game(&game)?;
    let server = flags
        .optional("--server")
        .filter(|value| value != "true")
        .or_else(|| global.server.clone())
        .or_else(|| instance.and_then(|instance| instance.server.clone()))
        .ok_or_else(|| {
            CliError::usage(
                "record build-task requires --server or configured instance.<id>.server",
            )
        })?;
    let server = canonical_server(&server)?;
    let locale = flags
        .optional("--locale")
        .filter(|value| value != "true")
        .ok_or_else(|| CliError::usage("record build-task requires --locale"))?;
    let locale = canonical_locale(&locale)?;
    Ok((game, server, locale))
}

fn safe_task_dir_name(task_id: &str) -> CliOutcome<String> {
    let safe = safe_file_stem(task_id);
    if safe != task_id || safe.is_empty() {
        return Err(CliError::usage(format!(
            "record build-task task id must be a safe path segment: {task_id}"
        )));
    }
    Ok(safe)
}

fn new_session_record_step(
    context: &SessionRecordStepContext<'_>,
    flags: &FlagArgs,
) -> CliOutcome<SessionRecordStep> {
    let kind = flags.required("--kind")?;
    let step_id = flags
        .optional("--step-id")
        .filter(|value| value != "true")
        .unwrap_or_else(|| format!("step-{:04}", context.record.steps.len() + 1));
    if step_id.trim().is_empty() {
        return Err(CliError::usage("--step-id must not be empty"));
    }
    if context
        .record
        .steps
        .iter()
        .any(|step| step.step_id == step_id)
    {
        return Err(CliError::safety_blocked(
            "record_step_id_conflict",
            format!("recording step id already exists: {step_id}"),
            &["session_record"],
        ));
    }
    let data = match kind.as_str() {
        "anchor" => new_session_record_anchor_step(context, &step_id, flags)?,
        "color-probe" | "color_probe" => {
            new_session_record_color_probe_step(context, &step_id, flags)?
        }
        "verify-template" | "verify_template" => {
            new_session_record_verify_template_step(context, &step_id, flags)?
        }
        "operation" => new_session_record_operation_step(flags)?,
        other => {
            return Err(CliError::usage(format!(
                "unsupported record step kind: {other}"
            )));
        }
    };
    Ok(SessionRecordStep {
        schema_version: "session-record-step-v0".to_string(),
        step_id,
        created_at_unix_ms: current_unix_ms(),
        updated_at_unix_ms: current_unix_ms(),
        data,
    })
}

fn new_session_record_anchor_step(
    context: &SessionRecordStepContext<'_>,
    step_id: &str,
    flags: &FlagArgs,
) -> CliOutcome<SessionRecordStepData> {
    let id = required_non_empty_flag(flags, "--id")?;
    let region = parse_session_record_region(&flags.required("--region")?)?;
    let threshold = parse_optional_unit_f64(flags, "--threshold")?;
    let materialized =
        materialize_anchor_artifact(context, step_id, &id, &region, threshold, flags)?;
    let evaluation = materialized
        .as_ref()
        .map(|materialized| materialized.evaluation.clone())
        .unwrap_or_else(|| SessionRecordStepEvaluation {
            status: "deferred".to_string(),
            reason: "frame_not_provided".to_string(),
            auto_region: None,
            backtest: None,
            contrast_backtest: None,
        });
    let stored_region = materialized
        .as_ref()
        .map(|materialized| materialized.region.clone())
        .unwrap_or(region);
    Ok(SessionRecordStepData::Anchor {
        id,
        region: stored_region,
        color_check: flags.bool("--color-check"),
        threshold,
        frame_provenance: materialized
            .as_ref()
            .map(|materialized| Box::new(materialized.frame_provenance.clone())),
        artifact: materialized.map(|materialized| Box::new(materialized.artifact)),
        evaluation: Box::new(evaluation),
    })
}

fn new_session_record_verify_template_step(
    context: &SessionRecordStepContext<'_>,
    step_id: &str,
    flags: &FlagArgs,
) -> CliOutcome<SessionRecordStepData> {
    let id = required_non_empty_flag(flags, "--id")?;
    let region = parse_session_record_region(&flags.required("--region")?)?;
    let threshold = parse_optional_unit_f64(flags, "--threshold")?;
    let materialized =
        materialize_anchor_artifact(context, step_id, &id, &region, threshold, flags)?;
    let evaluation = materialized
        .as_ref()
        .map(|materialized| materialized.evaluation.clone())
        .unwrap_or_else(|| SessionRecordStepEvaluation {
            status: "deferred".to_string(),
            reason: "frame_not_provided".to_string(),
            auto_region: None,
            backtest: None,
            contrast_backtest: None,
        });
    let stored_region = materialized
        .as_ref()
        .map(|materialized| materialized.region.clone())
        .unwrap_or(region);
    Ok(SessionRecordStepData::VerifyTemplate {
        id,
        region: stored_region,
        threshold,
        frame_provenance: materialized
            .as_ref()
            .map(|materialized| Box::new(materialized.frame_provenance.clone())),
        artifact: materialized.map(|materialized| Box::new(materialized.artifact)),
        evaluation: Box::new(evaluation),
    })
}

fn new_session_record_color_probe_step(
    context: &SessionRecordStepContext<'_>,
    step_id: &str,
    flags: &FlagArgs,
) -> CliOutcome<SessionRecordStepData> {
    let id = required_non_empty_flag(flags, "--id")?;
    let region = parse_session_record_region(&flags.required("--region")?)?;
    let materialized = materialize_color_probe(context, step_id, &id, &region, flags)?;
    let evaluation = materialized
        .as_ref()
        .map(|materialized| materialized.evaluation.clone())
        .unwrap_or_else(|| SessionRecordStepEvaluation {
            status: "deferred".to_string(),
            reason: "frame_not_provided".to_string(),
            auto_region: None,
            backtest: None,
            contrast_backtest: None,
        });
    let stored_region = materialized
        .as_ref()
        .map(|materialized| materialized.region.clone())
        .unwrap_or(region);
    Ok(SessionRecordStepData::ColorProbe {
        id,
        region: stored_region,
        expected: materialized
            .as_ref()
            .map(|materialized| materialized.expected),
        frame_provenance: materialized
            .as_ref()
            .map(|materialized| Box::new(materialized.frame_provenance.clone())),
        evaluation: Box::new(evaluation),
    })
}

fn new_session_record_operation_step(flags: &FlagArgs) -> CliOutcome<SessionRecordStepData> {
    let from = required_non_empty_flag(flags, "--from")?;
    let to = flags
        .optional("--to")
        .filter(|value| value != "true")
        .unwrap_or_else(|| "null".to_string());
    Ok(SessionRecordStepData::Operation {
        from,
        to: if to == "null" { None } else { Some(to) },
        click: parse_session_record_operation_click(flags)?,
        destructive: flags.bool("--destructive"),
    })
}

fn materialize_anchor_artifact(
    context: &SessionRecordStepContext<'_>,
    step_id: &str,
    anchor_id: &str,
    region: &SessionRecordRegion,
    threshold: Option<f64>,
    flags: &FlagArgs,
) -> CliOutcome<Option<MaterializedAnchorArtifact>> {
    let local_frame_path = flags
        .optional_path("--frame")
        .or_else(|| flags.optional_path("--source-frame"));
    let capture_current_frame = flags.bool("--capture") || flags.bool("--current-frame");
    if local_frame_path.is_some() && capture_current_frame {
        return Err(CliError::usage(
            "record anchor requires either --frame/--source-frame or --capture, not both",
        ));
    }
    if local_frame_path.is_none() && !capture_current_frame {
        return Ok(None);
    }
    let artifact_dir = session_record_artifact_dir(
        context.state_dir,
        &context.record.record_id,
        flags.optional_path("--artifact-dir").as_deref(),
    )?;
    let source_frame = if capture_current_frame {
        capture_session_record_source_frame(
            context.global,
            context.config,
            flags,
            &artifact_dir,
            step_id,
            anchor_id,
        )?
    } else {
        let frame_path = local_frame_path.expect("checked local frame path");
        read_session_record_source_frame(&frame_path)?
    };
    let resolution =
        resolve_session_record_anchor_rect(&source_frame.frame, region, threshold, flags)?;
    materialize_anchor_artifact_from_source(
        source_frame,
        resolution,
        &artifact_dir,
        step_id,
        anchor_id,
        threshold,
        flags,
    )
    .map(Some)
}

struct MaterializedColorProbe {
    region: SessionRecordRegion,
    expected: [u8; 3],
    frame_provenance: SessionRecordFrameProvenance,
    evaluation: SessionRecordStepEvaluation,
}

fn materialize_color_probe(
    context: &SessionRecordStepContext<'_>,
    step_id: &str,
    probe_id: &str,
    region: &SessionRecordRegion,
    flags: &FlagArgs,
) -> CliOutcome<Option<MaterializedColorProbe>> {
    let local_frame_path = flags
        .optional_path("--frame")
        .or_else(|| flags.optional_path("--source-frame"));
    let capture_current_frame = flags.bool("--capture") || flags.bool("--current-frame");
    if local_frame_path.is_some() && capture_current_frame {
        return Err(CliError::usage(
            "record color-probe requires either --frame/--source-frame or --capture, not both",
        ));
    }
    if local_frame_path.is_none() && !capture_current_frame {
        return Ok(None);
    }
    let artifact_dir = session_record_artifact_dir(
        context.state_dir,
        &context.record.record_id,
        flags.optional_path("--artifact-dir").as_deref(),
    )?;
    let source_frame = if capture_current_frame {
        capture_session_record_source_frame(
            context.global,
            context.config,
            flags,
            &artifact_dir,
            step_id,
            probe_id,
        )?
    } else {
        let frame_path = local_frame_path.expect("checked local frame path");
        read_session_record_source_frame(&frame_path)?
    };
    let resolution = resolve_session_record_anchor_rect(&source_frame.frame, region, None, flags)?;
    let expected = mean_session_record_rect_rgb(&source_frame.frame, &resolution.rect)?;
    Ok(Some(MaterializedColorProbe {
        region: SessionRecordRegion::Rect {
            rect: resolution.rect.clone(),
        },
        expected,
        frame_provenance: session_record_frame_provenance(source_frame),
        evaluation: SessionRecordStepEvaluation {
            status: "passed".to_string(),
            reason: "color_probe_sampled".to_string(),
            auto_region: resolution.auto_region,
            backtest: None,
            contrast_backtest: None,
        },
    }))
}

fn read_session_record_source_frame(frame_path: &Path) -> CliOutcome<SessionRecordSourceFrame> {
    let frame_png = fs::read(frame_path).map_err(|err| {
        CliError::usage(format!(
            "failed to read record source frame {}: {err}",
            frame_path.display()
        ))
    })?;
    let frame = Frame::from_png(frame_png.clone(), CaptureBackendName::AdbScreencap)
        .map_err(|err| CliError::usage(format!("failed to decode record source frame: {err}")))?;
    Ok(SessionRecordSourceFrame {
        frame,
        png: frame_png,
        source: "local_png".to_string(),
        path: frame_path.to_path_buf(),
        recorded_at_unix_ms: current_unix_ms(),
        capture_backend: None,
        freshness: None,
        capture_attempts: Vec::new(),
    })
}

fn session_record_artifact_root(state_dir: &Path, record_id: &str) -> PathBuf {
    state_dir
        .join("record-artifacts")
        .join(safe_file_stem(record_id))
}

fn session_record_artifact_dir(
    state_dir: &Path,
    record_id: &str,
    requested: Option<&Path>,
) -> CliOutcome<PathBuf> {
    let default_dir = session_record_artifact_root(state_dir, record_id);
    let candidate = requested.unwrap_or(default_dir.as_path());
    let resolved = ensure_path_within(
        state_dir,
        candidate,
        "record artifact directory",
        &["record", "artifact_dir"],
    )?;
    fs::create_dir_all(&resolved).map_err(|err| {
        CliError::usage(format!(
            "failed to create record artifact dir {}: {err}",
            resolved.display()
        ))
    })?;
    ensure_path_within(
        state_dir,
        &resolved,
        "record artifact directory",
        &["record", "artifact_dir"],
    )
}

fn capture_session_record_source_frame(
    global: &GlobalOptions,
    config: &UserConfig,
    flags: &FlagArgs,
    artifact_dir: &Path,
    step_id: &str,
    anchor_id: &str,
) -> CliOutcome<SessionRecordSourceFrame> {
    let device_config = device_config(global, config)?;
    let requested = device_config.capture_backend;
    let fresh_delay = parse_optional_duration_ms(flags, "--fresh-delay-ms", 160)?;
    let captured = capture_for_command(
        &device_config,
        requested,
        flags.bool("--require-fresh"),
        fresh_delay,
    )?;
    let png = captured.frame.png_for_artifact().map_err(|err| {
        CliError::device(format!("failed to encode record source capture: {err}"))
    })?;
    fs::create_dir_all(artifact_dir).map_err(|err| {
        CliError::usage(format!(
            "failed to create record artifact dir {}: {err}",
            artifact_dir.display()
        ))
    })?;
    let source_path = artifact_dir.join(format!(
        "source-frame-{}-{}.png",
        safe_file_stem(step_id),
        safe_file_stem(anchor_id)
    ));
    fs::write(&source_path, &png).map_err(|err| {
        CliError::usage(format!(
            "failed to write record source frame {}: {err}",
            source_path.display()
        ))
    })?;
    Ok(SessionRecordSourceFrame {
        capture_backend: Some(captured.frame.backend_name.as_str().to_string()),
        freshness: Some(captured.freshness),
        capture_attempts: captured.attempts,
        frame: captured.frame,
        png,
        source: "current_capture".to_string(),
        path: source_path,
        recorded_at_unix_ms: current_unix_ms(),
    })
}

fn session_record_frame_provenance(
    source_frame: SessionRecordSourceFrame,
) -> SessionRecordFrameProvenance {
    SessionRecordFrameProvenance {
        source: source_frame.source,
        path: source_frame.path.display().to_string(),
        sha256: hex_sha256(&source_frame.png),
        width: source_frame.frame.width,
        height: source_frame.frame.height,
        recorded_at_unix_ms: source_frame.recorded_at_unix_ms,
        capture_backend: source_frame.capture_backend,
        freshness: source_frame.freshness,
        capture_attempts: source_frame.capture_attempts,
    }
}

fn materialize_anchor_artifact_from_source(
    source_frame: SessionRecordSourceFrame,
    resolution: SessionRecordAnchorRegionResolution,
    artifact_dir: &Path,
    step_id: &str,
    anchor_id: &str,
    threshold: Option<f64>,
    flags: &FlagArgs,
) -> CliOutcome<MaterializedAnchorArtifact> {
    let rect = &resolution.rect;
    let crop = crop_frame_rect(&source_frame.frame, rect)?;
    let crop_png = crop
        .png_for_artifact()
        .map_err(|err| CliError::usage(format!("failed to encode record anchor crop: {err}")))?;
    let mut evaluation =
        backtest_anchor_crop(&source_frame.frame, rect, &crop_png, threshold, flags)?;
    evaluation.auto_region = resolution.auto_region;
    fs::create_dir_all(artifact_dir).map_err(|err| {
        CliError::usage(format!(
            "failed to create record artifact dir {}: {err}",
            artifact_dir.display()
        ))
    })?;
    let artifact_path = artifact_dir.join(format!(
        "anchor-{}-{}.png",
        safe_file_stem(step_id),
        safe_file_stem(anchor_id)
    ));
    fs::write(&artifact_path, &crop_png).map_err(|err| {
        CliError::usage(format!(
            "failed to write record anchor artifact {}: {err}",
            artifact_path.display()
        ))
    })?;
    Ok(MaterializedAnchorArtifact {
        region: SessionRecordRegion::Rect {
            rect: resolution.rect.clone(),
        },
        frame_provenance: session_record_frame_provenance(source_frame),
        artifact: SessionRecordAnchorArtifact {
            kind: "template_crop".to_string(),
            path: artifact_path.display().to_string(),
            sha256: hex_sha256(&crop_png),
            width: crop.width,
            height: crop.height,
            region: resolution.rect,
        },
        evaluation,
    })
}

fn backtest_anchor_crop(
    frame: &Frame,
    rect: &SessionRecordRect,
    crop_png: &[u8],
    threshold: Option<f64>,
    flags: &FlagArgs,
) -> CliOutcome<SessionRecordStepEvaluation> {
    let metric = parse_match_metric_flag(flags)?;
    let threshold = threshold.unwrap_or(0.95) as f32;
    let backtest = match_anchor_crop_in_frame(
        frame,
        rect,
        crop_png,
        metric,
        threshold,
        "local_png_self_test",
    )?;
    let contrast_backtest =
        backtest_contrast_anchor_crop(rect, crop_png, metric, threshold, flags)?;
    let positive_passed = backtest.passed;
    let contrast_passed = contrast_backtest
        .as_ref()
        .map(|backtest| backtest.passed)
        .unwrap_or(true);
    let passed = positive_passed && contrast_passed;
    let reason = if !positive_passed {
        "self_backtest_below_threshold"
    } else if !contrast_passed {
        "contrast_backtest_matched"
    } else if contrast_backtest.is_some() {
        "self_and_contrast_backtest_passed"
    } else {
        "self_backtest_passed"
    };
    Ok(SessionRecordStepEvaluation {
        status: if passed { "passed" } else { "failed" }.to_string(),
        reason: reason.to_string(),
        auto_region: None,
        backtest: Some(backtest),
        contrast_backtest,
    })
}

fn resolve_session_record_anchor_rect(
    frame: &Frame,
    region: &SessionRecordRegion,
    threshold: Option<f64>,
    flags: &FlagArgs,
) -> CliOutcome<SessionRecordAnchorRegionResolution> {
    match region {
        SessionRecordRegion::Auto => auto_session_record_anchor_rect(frame, threshold, flags),
        SessionRecordRegion::Rect { rect } => Ok(SessionRecordAnchorRegionResolution {
            rect: rect.clone(),
            auto_region: None,
        }),
    }
}

fn auto_session_record_anchor_rect(
    frame: &Frame,
    threshold: Option<f64>,
    flags: &FlagArgs,
) -> CliOutcome<SessionRecordAnchorRegionResolution> {
    if frame.width == 0 || frame.height == 0 {
        return Err(CliError::usage(
            "record anchor auto region requires a non-empty source frame",
        ));
    }
    let width = auto_session_record_axis_len(frame.width);
    let height = auto_session_record_axis_len(frame.height);
    let contrast_frame = read_session_record_contrast_frame(flags)?;
    let metric = if contrast_frame.is_some() {
        Some(parse_match_metric_flag(flags)?)
    } else {
        None
    };
    let match_threshold = threshold.unwrap_or(0.95) as f32;
    let mut candidates = Vec::new();
    for y in auto_session_record_axis_positions(frame.height, height) {
        for x in auto_session_record_axis_positions(frame.width, width) {
            let rect = SessionRecordRect {
                x: i32::try_from(x)
                    .map_err(|_| CliError::usage("record anchor auto x exceeds i32"))?,
                y: i32::try_from(y)
                    .map_err(|_| CliError::usage("record anchor auto y exceeds i32"))?,
                width: i32::try_from(width)
                    .map_err(|_| CliError::usage("record anchor auto width exceeds i32"))?,
                height: i32::try_from(height)
                    .map_err(|_| CliError::usage("record anchor auto height exceeds i32"))?,
            };
            let score = score_session_record_region_luma_variance(frame, &rect)?;
            let (contrast_score, contrast_passed) = if let Some(contrast_frame) = &contrast_frame {
                let crop = crop_frame_rect(frame, &rect)?;
                let crop_png = crop.png_for_artifact().map_err(|err| {
                    CliError::usage(format!("failed to encode record auto-region crop: {err}"))
                })?;
                let backtest = match_anchor_crop_in_frame(
                    &contrast_frame.frame,
                    &rect,
                    &crop_png,
                    metric.ok_or_else(|| {
                        CliError::usage("record auto-region contrast scoring requires match metric")
                    })?,
                    match_threshold,
                    "auto_region_contrast",
                )?;
                (Some(backtest.score), Some(backtest.score < match_threshold))
            } else {
                (None, None)
            };
            candidates.push(SessionRecordAutoRegionCandidate {
                region: rect,
                luma_variance: score,
                contrast_score,
                contrast_passed,
                selected: false,
            });
        }
    }
    let Some((selected_index, selected_reason)) =
        select_session_record_auto_region_candidate(&candidates, contrast_frame.is_some())
    else {
        return Err(CliError::usage(
            "record anchor auto region produced no candidates",
        ));
    };
    let selected = candidates[selected_index].region.clone();
    for (index, candidate) in candidates.iter_mut().enumerate() {
        candidate.selected = index == selected_index;
    }
    Ok(SessionRecordAnchorRegionResolution {
        rect: selected.clone(),
        auto_region: Some(SessionRecordAutoRegionSelection {
            strategy: "bounded_luma_variance_grid_v1".to_string(),
            selected_reason,
            selected,
            candidates,
        }),
    })
}

fn select_session_record_auto_region_candidate(
    candidates: &[SessionRecordAutoRegionCandidate],
    has_contrast: bool,
) -> Option<(usize, String)> {
    let has_discriminating_candidate = candidates
        .iter()
        .any(|candidate| candidate.contrast_passed == Some(true));
    let selected_reason = if has_discriminating_candidate {
        "contrast_rejected_highest_variance"
    } else if has_contrast {
        "lowest_contrast_score"
    } else {
        "highest_luma_variance"
    };
    let mut selected = None;
    for (index, candidate) in candidates.iter().enumerate() {
        let Some(best_index) = selected else {
            selected = Some(index);
            continue;
        };
        if session_record_auto_region_candidate_is_better(
            candidate,
            &candidates[best_index],
            has_discriminating_candidate,
            has_contrast,
        ) {
            selected = Some(index);
        }
    }
    selected.map(|index| (index, selected_reason.to_string()))
}

fn session_record_auto_region_candidate_is_better(
    candidate: &SessionRecordAutoRegionCandidate,
    best: &SessionRecordAutoRegionCandidate,
    prefer_discriminating: bool,
    prefer_lowest_contrast: bool,
) -> bool {
    if prefer_discriminating {
        let candidate_passed = candidate.contrast_passed == Some(true);
        let best_passed = best.contrast_passed == Some(true);
        if candidate_passed != best_passed {
            return candidate_passed;
        }
    }
    if prefer_lowest_contrast {
        match (candidate.contrast_score, best.contrast_score) {
            (Some(candidate_score), Some(best_score))
                if (candidate_score - best_score).abs() > f32::EPSILON =>
            {
                return candidate_score < best_score;
            }
            (Some(_), None) => return true,
            (None, Some(_)) => return false,
            _ => {}
        }
    }
    if (candidate.luma_variance - best.luma_variance).abs() > f64::EPSILON {
        return candidate.luma_variance > best.luma_variance;
    }
    (candidate.region.y, candidate.region.x) < (best.region.y, best.region.x)
}

fn auto_session_record_axis_len(total: u32) -> u32 {
    (total / 3).max(1).min(total)
}

fn auto_session_record_axis_positions(total: u32, len: u32) -> Vec<u32> {
    if total <= len {
        return vec![0];
    }
    let end = total - len;
    let mut positions = vec![0, end / 2, end];
    positions.sort_unstable();
    positions.dedup();
    positions
}

fn score_session_record_region_luma_variance(
    frame: &Frame,
    rect: &SessionRecordRect,
) -> CliOutcome<f64> {
    let stride = match frame.pixel_format {
        PixelFormat::Rgb8 => 3usize,
        PixelFormat::Rgba8 => 4usize,
    };
    let x = usize::try_from(rect.x)
        .map_err(|_| CliError::usage("record anchor auto rect x exceeds usize"))?;
    let y = usize::try_from(rect.y)
        .map_err(|_| CliError::usage("record anchor auto rect y exceeds usize"))?;
    let width = usize::try_from(rect.width)
        .map_err(|_| CliError::usage("record anchor auto rect width exceeds usize"))?;
    let height = usize::try_from(rect.height)
        .map_err(|_| CliError::usage("record anchor auto rect height exceeds usize"))?;
    let frame_width = usize::try_from(frame.width)
        .map_err(|_| CliError::usage("record source frame width exceeds usize"))?;
    let mut count = 0f64;
    let mut sum = 0f64;
    let mut sum_sq = 0f64;
    for row in 0..height {
        for col in 0..width {
            let column = x
                .checked_add(col)
                .ok_or_else(|| CliError::usage("record anchor auto score column overflow"))?;
            let offset = ((y + row)
                .checked_mul(frame_width)
                .and_then(|value| value.checked_add(column))
                .and_then(|value| value.checked_mul(stride)))
            .ok_or_else(|| CliError::usage("record anchor auto score offset overflow"))?;
            let r = f64::from(frame.pixels[offset]);
            let g = f64::from(frame.pixels[offset + 1]);
            let b = f64::from(frame.pixels[offset + 2]);
            let luma = (r + g + b) / 3.0;
            count += 1.0;
            sum += luma;
            sum_sq += luma * luma;
        }
    }
    if count == 0.0 {
        return Err(CliError::usage(
            "record anchor auto region cannot score an empty candidate",
        ));
    }
    let mean = sum / count;
    Ok((sum_sq / count) - (mean * mean))
}

fn match_anchor_crop_in_frame(
    frame: &Frame,
    rect: &SessionRecordRect,
    crop_png: &[u8],
    metric: MatchMetric,
    threshold: f32,
    source: &str,
) -> CliOutcome<SessionRecordAnchorBacktest> {
    let scene = scene_from_frame(frame)?;
    let matched = scene
        .match_template_with_metric(
            crop_png,
            Some(RecognitionRect {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height,
            }),
            metric,
        )
        .map_err(|err| CliError::usage(format!("failed to backtest record anchor crop: {err}")))?;
    Ok(SessionRecordAnchorBacktest {
        source: source.to_string(),
        metric: match_metric_name(metric).to_string(),
        region: rect.clone(),
        x: matched.x,
        y: matched.y,
        raw_score: matched.raw_score,
        score: matched.score,
        threshold,
        passed: matched.score >= threshold,
    })
}

fn backtest_contrast_anchor_crop(
    rect: &SessionRecordRect,
    crop_png: &[u8],
    metric: MatchMetric,
    threshold: f32,
    flags: &FlagArgs,
) -> CliOutcome<Option<SessionRecordAnchorContrastBacktest>> {
    let Some(contrast_frame) = read_session_record_contrast_frame(flags)? else {
        return Ok(None);
    };
    let backtest = match_anchor_crop_in_frame(
        &contrast_frame.frame,
        rect,
        crop_png,
        metric,
        threshold,
        "local_png_contrast",
    )?;
    Ok(Some(SessionRecordAnchorContrastBacktest {
        source: "local_png_contrast".to_string(),
        path: contrast_frame.path.display().to_string(),
        sha256: contrast_frame.sha256,
        width: contrast_frame.frame.width,
        height: contrast_frame.frame.height,
        metric: backtest.metric,
        region: backtest.region,
        x: backtest.x,
        y: backtest.y,
        raw_score: backtest.raw_score,
        score: backtest.score,
        threshold: backtest.threshold,
        passed: backtest.score < threshold,
    }))
}

fn read_session_record_contrast_frame(
    flags: &FlagArgs,
) -> CliOutcome<Option<SessionRecordContrastFrame>> {
    let Some(frame_path) = flags
        .optional_path("--contrast-frame")
        .or_else(|| flags.optional_path("--negative-frame"))
    else {
        return Ok(None);
    };
    let frame_png = fs::read(&frame_path).map_err(|err| {
        CliError::usage(format!(
            "failed to read record contrast frame {}: {err}",
            frame_path.display()
        ))
    })?;
    let frame_hash = hex_sha256(&frame_png);
    let frame = Frame::from_png(frame_png, CaptureBackendName::AdbScreencap)
        .map_err(|err| CliError::usage(format!("failed to decode record contrast frame: {err}")))?;
    Ok(Some(SessionRecordContrastFrame {
        frame,
        path: frame_path,
        sha256: frame_hash,
    }))
}

fn crop_frame_rect(frame: &Frame, rect: &SessionRecordRect) -> CliOutcome<Frame> {
    if rect.x < 0 || rect.y < 0 || rect.width <= 0 || rect.height <= 0 {
        return Err(CliError::usage(
            "record anchor crop rect must have non-negative origin and positive size",
        ));
    }
    let x = u32::try_from(rect.x).map_err(|_| CliError::usage("record anchor rect x overflow"))?;
    let y = u32::try_from(rect.y).map_err(|_| CliError::usage("record anchor rect y overflow"))?;
    let width = u32::try_from(rect.width)
        .map_err(|_| CliError::usage("record anchor rect width overflow"))?;
    let height = u32::try_from(rect.height)
        .map_err(|_| CliError::usage("record anchor rect height overflow"))?;
    let right = x
        .checked_add(width)
        .ok_or_else(|| CliError::usage("record anchor crop rect x+width overflow"))?;
    let bottom = y
        .checked_add(height)
        .ok_or_else(|| CliError::usage("record anchor crop rect y+height overflow"))?;
    if right > frame.width || bottom > frame.height {
        return Err(CliError::usage(format!(
            "record anchor crop rect {}x{} at {},{} exceeds frame {}x{}",
            width, height, x, y, frame.width, frame.height
        )));
    }
    let stride = match frame.pixel_format {
        PixelFormat::Rgb8 => 3usize,
        PixelFormat::Rgba8 => 4usize,
    };
    let frame_width = usize::try_from(frame.width)
        .map_err(|_| CliError::usage("record source frame width exceeds usize"))?;
    let x = usize::try_from(x).map_err(|_| CliError::usage("record anchor x exceeds usize"))?;
    let y = usize::try_from(y).map_err(|_| CliError::usage("record anchor y exceeds usize"))?;
    let width =
        usize::try_from(width).map_err(|_| CliError::usage("record anchor width exceeds usize"))?;
    let height = usize::try_from(height)
        .map_err(|_| CliError::usage("record anchor height exceeds usize"))?;
    let row_bytes = width
        .checked_mul(stride)
        .ok_or_else(|| CliError::usage("record anchor row byte length overflow"))?;
    let mut pixels = Vec::with_capacity(
        row_bytes
            .checked_mul(height)
            .ok_or_else(|| CliError::usage("record anchor crop byte length overflow"))?,
    );
    for row in 0..height {
        let offset = ((y + row)
            .checked_mul(frame_width)
            .and_then(|value| value.checked_add(x))
            .and_then(|value| value.checked_mul(stride)))
        .ok_or_else(|| CliError::usage("record anchor crop offset overflow"))?;
        let end = offset
            .checked_add(row_bytes)
            .ok_or_else(|| CliError::usage("record anchor crop row end overflow"))?;
        pixels.extend_from_slice(&frame.pixels[offset..end]);
    }
    Frame::from_pixels(
        u32::try_from(width).map_err(|_| CliError::usage("record anchor width exceeds u32"))?,
        u32::try_from(height).map_err(|_| CliError::usage("record anchor height exceeds u32"))?,
        pixels,
        frame.pixel_format,
        frame.backend_name,
    )
    .map_err(|err| CliError::usage(format!("failed to build record anchor crop frame: {err}")))
}

fn parse_session_record_operation_click(flags: &FlagArgs) -> CliOutcome<SessionRecordClick> {
    let gesture_flags = [
        flags.optional("--click").is_some(),
        flags
            .optional("--swipe")
            .or_else(|| flags.optional("--drag"))
            .is_some(),
        flags
            .optional("--long-press")
            .or_else(|| flags.optional("--long-tap"))
            .is_some(),
    ]
    .into_iter()
    .filter(|present| *present)
    .count();
    if gesture_flags != 1 {
        return Err(CliError::usage(
            "record operation requires exactly one of --click, --swipe/--drag, or --long-press/--long-tap",
        ));
    }
    if let Some(click) = flags.optional("--click").filter(|value| value != "true") {
        return parse_session_record_click(&click);
    }
    if let Some(swipe) = flags
        .optional("--swipe")
        .or_else(|| flags.optional("--drag"))
        .filter(|value| value != "true")
    {
        let (from, to) = parse_session_record_swipe_rects(&swipe)?;
        return Ok(SessionRecordClick::Swipe {
            from,
            to,
            duration_ms: parse_record_duration_ms(flags, 500)?,
        });
    }
    if let Some(long_press) = flags
        .optional("--long-press")
        .or_else(|| flags.optional("--long-tap"))
        .filter(|value| value != "true")
    {
        let (x, y) = parse_point_pair(&long_press)?;
        return Ok(SessionRecordClick::LongPress {
            x,
            y,
            duration_ms: parse_record_duration_ms(flags, 700)?,
        });
    }
    Err(CliError::usage(
        "record operation action parser reached an impossible state",
    ))
}

fn parse_session_record_click(value: &str) -> CliOutcome<SessionRecordClick> {
    if value.trim().is_empty() {
        return Err(CliError::usage("--click must not be empty"));
    }
    if value.contains(',') {
        let (x, y) = parse_point_pair(value)?;
        return Ok(SessionRecordClick::Coord { x, y });
    }
    Ok(SessionRecordClick::Target {
        target: value.to_string(),
    })
}

fn amend_session_record_from_drift_diagnostics(
    context: &SessionRecordAmendContext,
    record: &mut SessionRecordContext,
    flags: &FlagArgs,
    diagnostics_path: PathBuf,
) -> CliOutcome<Value> {
    reject_direct_drift_amend_flags(flags)?;
    let diagnostics = read_session_record_drift_diagnostics(&diagnostics_path)?;
    let selector = flags
        .optional("--step-id")
        .filter(|value| value != "true")
        .or_else(|| flags.positionals.first().cloned());
    if flags.positionals.len() > 1 {
        return Err(CliError::usage(
            "session record amend --from-drift-diagnostics accepts at most one positional selector",
        ));
    }
    let step_index = find_drift_amend_step(record, &diagnostics, selector.as_deref())?;
    let mut amended_step = record.steps[step_index].clone();
    let resource_kind = amend_drift_record_step(context, &mut amended_step, flags, &diagnostics)?;
    let step_id = amended_step.step_id.clone();
    record.steps[step_index] = amended_step;
    Ok(json!({
        "schema_version": "session.record_drift_amend.v0.1",
        "diagnostics_path": diagnostics.path.display().to_string(),
        "target_id": diagnostics.target_id,
        "step_id": step_id,
        "resource_kind": resource_kind,
        "changed_fields": diagnostics.changed_fields,
        "region": diagnostics.region,
        "threshold": diagnostics.threshold,
        "build_task_command": "session record build-task"
    }))
}

fn reject_direct_drift_amend_flags(flags: &FlagArgs) -> CliOutcome<()> {
    const ALLOWED: &[&str] = &[
        "--from-drift-diagnostics",
        "--state-dir",
        "--step-id",
        "--holder",
        "--lease-holder",
        "--lease-id",
        "--contrast-frame",
    ];
    let unsupported = flags
        .flags
        .keys()
        .filter(|name| !ALLOWED.contains(&name.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if !unsupported.is_empty() {
        return Err(CliError::usage(format!(
            "session record amend --from-drift-diagnostics only accepts drift diagnostics changes; unsupported direct flags: {}",
            unsupported.join(", ")
        )));
    }
    Ok(())
}

fn read_session_record_drift_diagnostics(path: &Path) -> CliOutcome<SessionRecordDriftDiagnostics> {
    let Some(value) = read_json_file::<Value>(path)? else {
        return Err(CliError::usage(format!(
            "session record amend drift diagnostics file is missing: {}",
            path.display()
        )));
    };
    parse_session_record_drift_diagnostics(path.to_path_buf(), &value)
}

fn parse_session_record_drift_diagnostics(
    path: PathBuf,
    value: &Value,
) -> CliOutcome<SessionRecordDriftDiagnostics> {
    let trigger = value
        .get("trigger")
        .and_then(Value::as_str)
        .ok_or_else(|| CliError::usage("drift diagnostics must include trigger: resource_drift"))?;
    if trigger != "resource_drift" {
        return Err(CliError::usage(format!(
            "drift diagnostics trigger must be resource_drift, got {trigger}"
        )));
    }
    let target_id = value
        .get("target_id")
        .or_else(|| value.pointer("/guard/target_id"))
        .and_then(Value::as_str)
        .filter(|target_id| !target_id.trim().is_empty())
        .ok_or_else(|| CliError::usage("drift diagnostics must include target_id"))?
        .to_string();
    let proposed = value.get("proposed_changes");
    let (threshold, proposed_region) = parse_drift_proposed_changes(proposed)?;
    let region = proposed_region
        .or_else(|| {
            value
                .pointer("/measured/matched_rect")
                .map(|rect| parse_session_record_rect_value(rect, "measured.matched_rect"))
        })
        .transpose()?
        .ok_or_else(|| {
            CliError::usage(
                "drift diagnostics must include proposed_changes.region or measured.matched_rect",
            )
        })?;
    let mut changed_fields = vec!["region"];
    if threshold.is_some() {
        changed_fields.push("threshold");
    }
    Ok(SessionRecordDriftDiagnostics {
        path,
        target_id,
        region,
        threshold,
        changed_fields,
    })
}

fn parse_drift_proposed_changes(
    proposed: Option<&Value>,
) -> CliOutcome<(Option<f64>, Option<CliOutcome<SessionRecordRect>>)> {
    let Some(proposed) = proposed else {
        return Ok((None, None));
    };
    let object = proposed.as_object().ok_or_else(|| {
        CliError::usage("drift diagnostics proposed_changes must be an object when provided")
    })?;
    let mut unsupported = object
        .keys()
        .filter(|key| !matches!(key.as_str(), "region" | "threshold"))
        .cloned()
        .collect::<Vec<_>>();
    unsupported.sort();
    if !unsupported.is_empty() {
        return Err(CliError::usage(format!(
            "drift diagnostics proposed_changes contains fields outside the amend whitelist: {}",
            unsupported.join(", ")
        )));
    }
    let threshold = object
        .get("threshold")
        .map(|value| parse_unit_f64_value(value, "proposed_changes.threshold"))
        .transpose()?;
    let region = object
        .get("region")
        .map(|value| parse_session_record_region_value(value, "proposed_changes.region"));
    Ok((threshold, region))
}

fn parse_session_record_region_value(value: &Value, label: &str) -> CliOutcome<SessionRecordRect> {
    if value.get("mode").and_then(Value::as_str) == Some("rect") {
        let rect = value
            .get("rect")
            .ok_or_else(|| CliError::usage(format!("{label}.rect is missing")))?;
        return parse_session_record_rect_value(rect, label);
    }
    parse_session_record_rect_value(value, label)
}

fn parse_session_record_rect_value(value: &Value, label: &str) -> CliOutcome<SessionRecordRect> {
    let field = |name: &str| {
        value
            .get(name)
            .and_then(Value::as_i64)
            .ok_or_else(|| CliError::usage(format!("{label}.{name} must be an integer")))
    };
    let to_i32 = |name: &str, raw: i64| {
        i32::try_from(raw).map_err(|_| {
            CliError::usage(format!("{label}.{name} is outside the supported i32 range"))
        })
    };
    let rect = SessionRecordRect {
        x: to_i32("x", field("x")?)?,
        y: to_i32("y", field("y")?)?,
        width: to_i32("width", field("width")?)?,
        height: to_i32("height", field("height")?)?,
    };
    if rect.width <= 0 || rect.height <= 0 {
        return Err(CliError::usage(format!(
            "{label} dimensions must be positive: {}x{}",
            rect.width, rect.height
        )));
    }
    Ok(rect)
}

fn parse_unit_f64_value(value: &Value, label: &str) -> CliOutcome<f64> {
    let parsed = value
        .as_f64()
        .ok_or_else(|| CliError::usage(format!("{label} must be a number")))?;
    if !parsed.is_finite() || !(0.0..=1.0).contains(&parsed) {
        return Err(CliError::usage(format!(
            "{label} must be a finite number between 0 and 1"
        )));
    }
    Ok(parsed)
}

fn find_drift_amend_step(
    record: &SessionRecordContext,
    diagnostics: &SessionRecordDriftDiagnostics,
    selector: Option<&str>,
) -> CliOutcome<usize> {
    let mut matches = record
        .steps
        .iter()
        .enumerate()
        .filter(|(_, step)| drift_step_matches_target(step, &diagnostics.target_id))
        .filter(|(_, step)| {
            selector.is_none_or(|selector| {
                step.step_id == selector || drift_step_matches_target(step, selector)
            })
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if matches.is_empty() {
        return Err(CliError::safety_blocked(
            "record_drift_target_not_found",
            format!(
                "no anchor or verify-template record step matches drift target '{}'",
                diagnostics.target_id
            ),
            &["session_record", "resource_drift"],
        ));
    }
    if matches.len() > 1 {
        let step_ids = matches
            .iter()
            .map(|index| record.steps[*index].step_id.as_str())
            .collect::<Vec<_>>();
        return Err(CliError::safety_blocked(
            "record_drift_target_ambiguous",
            format!(
                "drift target '{}' matches multiple record steps: {}",
                diagnostics.target_id,
                step_ids.join(", ")
            ),
            &["session_record", "resource_drift"],
        ));
    }
    Ok(matches.remove(0))
}

fn drift_step_matches_target(step: &SessionRecordStep, target: &str) -> bool {
    match &step.data {
        SessionRecordStepData::Anchor { id, .. }
        | SessionRecordStepData::VerifyTemplate { id, .. } => {
            session_record_resource_id_matches_target(id, target)
        }
        SessionRecordStepData::ColorProbe { .. } | SessionRecordStepData::Operation { .. } => false,
    }
}

fn session_record_resource_id_matches_target(id: &str, target: &str) -> bool {
    id == target
        || session_record_anchor_target_id(id) == target
        || target
            .strip_prefix("page/")
            .is_some_and(|stripped| stripped == id)
}

fn amend_drift_record_step(
    context: &SessionRecordAmendContext,
    step: &mut SessionRecordStep,
    flags: &FlagArgs,
    diagnostics: &SessionRecordDriftDiagnostics,
) -> CliOutcome<&'static str> {
    match &mut step.data {
        SessionRecordStepData::Anchor {
            id,
            region,
            color_check,
            threshold,
            frame_provenance,
            artifact,
            evaluation,
        } => {
            *region = SessionRecordRegion::Rect {
                rect: diagnostics.region.clone(),
            };
            if let Some(next_threshold) = diagnostics.threshold {
                *threshold = Some(next_threshold);
            }
            let mut target = SessionRecordAnchorAmendTarget {
                id,
                region,
                color_check,
                threshold,
                frame_provenance,
                artifact,
                evaluation,
            };
            refresh_amended_anchor_artifact(context, &step.step_id, &mut target, flags, None)?;
            step.updated_at_unix_ms = current_unix_ms();
            Ok("anchor")
        }
        SessionRecordStepData::VerifyTemplate {
            id,
            region,
            threshold,
            frame_provenance,
            artifact,
            evaluation,
        } => {
            *region = SessionRecordRegion::Rect {
                rect: diagnostics.region.clone(),
            };
            if let Some(next_threshold) = diagnostics.threshold {
                *threshold = Some(next_threshold);
            }
            let mut target = SessionRecordVerifyTemplateAmendTarget {
                id,
                region,
                threshold,
                frame_provenance,
                artifact,
                evaluation,
            };
            refresh_amended_verify_template(context, &step.step_id, &mut target, flags, None)?;
            step.updated_at_unix_ms = current_unix_ms();
            Ok("verify_template")
        }
        SessionRecordStepData::ColorProbe { .. } | SessionRecordStepData::Operation { .. } => {
            Err(CliError::usage(
                "drift diagnostics amend supports only anchor and verify-template record steps",
            ))
        }
    }
}

fn session_record_candidate_report(
    record: &SessionRecordContext,
    step: &SessionRecordStep,
    record_path: &Path,
) -> CliOutcome<Value> {
    let (resource_kind, resource_id, region, evaluation) = match &step.data {
        SessionRecordStepData::Anchor {
            id,
            region,
            evaluation,
            ..
        } => ("anchor", id, region, evaluation),
        SessionRecordStepData::ColorProbe {
            id,
            region,
            evaluation,
            ..
        } => ("color_probe", id, region, evaluation),
        SessionRecordStepData::VerifyTemplate {
            id,
            region,
            evaluation,
            ..
        } => ("verify_template", id, region, evaluation),
        SessionRecordStepData::Operation { .. } => {
            return Err(CliError::usage(
                "session record candidates requires a resource step with an auto-region candidate report",
            ));
        }
    };
    let Some(auto_region) = &evaluation.auto_region else {
        return Err(CliError::usage(
            "session record candidates requires an existing auto-region candidate report",
        ));
    };
    let selected_index = auto_region
        .candidates
        .iter()
        .position(|candidate| candidate.selected);
    Ok(json!({
        "status": "candidates_listed",
        "record_id": record.record_id.as_str(),
        "task_id": record.task_id.as_str(),
        "instance": record.instance.as_str(),
        "record_status": record.status.as_str(),
        "step_id": step.step_id.as_str(),
        "resource_kind": resource_kind,
        "resource_id": resource_id,
        "anchor_id": resource_id,
        "region": region,
        "evaluation_status": evaluation.status.as_str(),
        "auto_region": auto_region,
        "candidate_count": auto_region.candidates.len(),
        "selected_index": selected_index,
        "path": record_path.display().to_string()
    }))
}

fn amend_session_record_step(
    context: &SessionRecordAmendContext,
    step: &mut SessionRecordStep,
    flags: &FlagArgs,
) -> CliOutcome<()> {
    let step_id = step.step_id.clone();
    let changed = match &mut step.data {
        SessionRecordStepData::Anchor {
            id,
            region,
            color_check,
            threshold,
            frame_provenance,
            artifact,
            evaluation,
        } => {
            let mut target = SessionRecordAnchorAmendTarget {
                id,
                region,
                color_check,
                threshold,
                frame_provenance,
                artifact,
                evaluation,
            };
            amend_anchor_record_step(context, &step_id, &mut target, flags)?
        }
        SessionRecordStepData::ColorProbe {
            id,
            region,
            expected,
            frame_provenance,
            evaluation,
        } => {
            let mut target = SessionRecordColorProbeAmendTarget {
                id,
                region,
                expected,
                frame_provenance,
                evaluation,
            };
            amend_color_probe_record_step(&step_id, &mut target, flags)?
        }
        SessionRecordStepData::VerifyTemplate {
            id,
            region,
            threshold,
            frame_provenance,
            artifact,
            evaluation,
        } => {
            let mut target = SessionRecordVerifyTemplateAmendTarget {
                id,
                region,
                threshold,
                frame_provenance,
                artifact,
                evaluation,
            };
            amend_verify_template_record_step(context, &step_id, &mut target, flags)?
        }
        SessionRecordStepData::Operation {
            from,
            to,
            click,
            destructive,
        } => amend_operation_record_step(from, to, click, destructive, flags)?,
    };
    if !changed {
        return Err(CliError::usage(
            "session record amend did not include any supported fields for this step kind",
        ));
    }
    step.updated_at_unix_ms = current_unix_ms();
    Ok(())
}

fn amend_anchor_record_step(
    context: &SessionRecordAmendContext,
    step_id: &str,
    target: &mut SessionRecordAnchorAmendTarget<'_>,
    flags: &FlagArgs,
) -> CliOutcome<bool> {
    let mut changed = false;
    let mut auto_region_override = None;
    if let Some(value) = flags.optional("--id").filter(|value| value != "true") {
        if value.trim().is_empty() {
            return Err(CliError::usage("--id must not be empty"));
        }
        *target.id = value;
        changed = true;
    }
    if let Some(candidate_index) = parse_session_record_candidate_index(flags)? {
        let selection = select_recorded_auto_region_candidate(target.evaluation, candidate_index)?;
        *target.region = SessionRecordRegion::Rect {
            rect: selection.selected.clone(),
        };
        auto_region_override = Some(selection);
        changed = true;
    }
    if let Some(value) = flags.optional("--region").filter(|value| value != "true") {
        *target.region = parse_session_record_region(&value)?;
        auto_region_override = None;
        changed = true;
    }
    if flags.bool("--color-check") {
        *target.color_check = true;
        changed = true;
    }
    if flags.bool("--no-color-check") {
        *target.color_check = false;
        changed = true;
    }
    if flags.flags.contains_key("--threshold") {
        *target.threshold = parse_optional_unit_f64(flags, "--threshold")?;
        changed = true;
    }
    if flags.bool("--clear-threshold") {
        *target.threshold = None;
        changed = true;
    }
    if changed {
        refresh_amended_anchor_artifact(context, step_id, target, flags, auto_region_override)?;
    }
    Ok(changed)
}

fn amend_color_probe_record_step(
    step_id: &str,
    target: &mut SessionRecordColorProbeAmendTarget<'_>,
    flags: &FlagArgs,
) -> CliOutcome<bool> {
    let mut changed = false;
    let mut auto_region_override = None;
    if let Some(value) = flags.optional("--id").filter(|value| value != "true") {
        if value.trim().is_empty() {
            return Err(CliError::usage("--id must not be empty"));
        }
        *target.id = value;
        changed = true;
    }
    if let Some(candidate_index) = parse_session_record_candidate_index(flags)? {
        let selection = select_recorded_auto_region_candidate(target.evaluation, candidate_index)?;
        *target.region = SessionRecordRegion::Rect {
            rect: selection.selected.clone(),
        };
        auto_region_override = Some(selection);
        changed = true;
    }
    if let Some(value) = flags.optional("--region").filter(|value| value != "true") {
        *target.region = parse_session_record_region(&value)?;
        auto_region_override = None;
        changed = true;
    }
    if changed {
        refresh_amended_color_probe(step_id, target, flags, auto_region_override)?;
    }
    Ok(changed)
}

fn amend_verify_template_record_step(
    context: &SessionRecordAmendContext,
    step_id: &str,
    target: &mut SessionRecordVerifyTemplateAmendTarget<'_>,
    flags: &FlagArgs,
) -> CliOutcome<bool> {
    let mut changed = false;
    let mut auto_region_override = None;
    if let Some(value) = flags.optional("--id").filter(|value| value != "true") {
        if value.trim().is_empty() {
            return Err(CliError::usage("--id must not be empty"));
        }
        *target.id = value;
        changed = true;
    }
    if let Some(candidate_index) = parse_session_record_candidate_index(flags)? {
        let selection = select_recorded_auto_region_candidate(target.evaluation, candidate_index)?;
        *target.region = SessionRecordRegion::Rect {
            rect: selection.selected.clone(),
        };
        auto_region_override = Some(selection);
        changed = true;
    }
    if let Some(value) = flags.optional("--region").filter(|value| value != "true") {
        *target.region = parse_session_record_region(&value)?;
        auto_region_override = None;
        changed = true;
    }
    if flags.flags.contains_key("--threshold") {
        *target.threshold = parse_optional_unit_f64(flags, "--threshold")?;
        changed = true;
    }
    if flags.bool("--clear-threshold") {
        *target.threshold = None;
        changed = true;
    }
    if changed {
        refresh_amended_verify_template(context, step_id, target, flags, auto_region_override)?;
    }
    Ok(changed)
}

fn select_recorded_auto_region_candidate(
    evaluation: &SessionRecordStepEvaluation,
    candidate_index: usize,
) -> CliOutcome<SessionRecordAutoRegionSelection> {
    let Some(auto_region) = &evaluation.auto_region else {
        return Err(CliError::usage(
            "record amend --candidate-index requires an existing auto-region candidate report",
        ));
    };
    let Some(candidate) = auto_region.candidates.get(candidate_index) else {
        return Err(CliError::usage(format!(
            "record amend candidate index {candidate_index} is out of range for {} candidates",
            auto_region.candidates.len()
        )));
    };
    let mut selection = auto_region.clone();
    selection.selected = candidate.region.clone();
    selection.selected_reason = "operator_selected_candidate".to_string();
    for (index, candidate) in selection.candidates.iter_mut().enumerate() {
        candidate.selected = index == candidate_index;
    }
    Ok(selection)
}

fn refresh_amended_anchor_artifact(
    context: &SessionRecordAmendContext,
    step_id: &str,
    target: &mut SessionRecordAnchorAmendTarget<'_>,
    flags: &FlagArgs,
    auto_region_override: Option<SessionRecordAutoRegionSelection>,
) -> CliOutcome<()> {
    let Some(provenance) = target.frame_provenance.as_deref() else {
        *target.evaluation = SessionRecordStepEvaluation {
            status: "deferred".to_string(),
            reason: "amended_without_frame_provenance".to_string(),
            auto_region: None,
            backtest: None,
            contrast_backtest: None,
        };
        return Ok(());
    };
    let source_frame = read_session_record_source_frame_from_provenance(provenance)?;
    let resolution = if let Some(auto_region) = auto_region_override {
        SessionRecordAnchorRegionResolution {
            rect: auto_region.selected.clone(),
            auto_region: Some(auto_region),
        }
    } else {
        resolve_session_record_anchor_rect(
            &source_frame.frame,
            target.region,
            *target.threshold,
            flags,
        )?
    };
    let artifact_dir = amended_anchor_artifact_dir(context, target.artifact.as_deref())?;
    let materialized = materialize_anchor_artifact_from_source(
        source_frame,
        resolution,
        &artifact_dir,
        step_id,
        target.id,
        *target.threshold,
        flags,
    )?;
    *target.region = materialized.region.clone();
    *target.frame_provenance = Some(Box::new(materialized.frame_provenance));
    *target.artifact = Some(Box::new(materialized.artifact));
    *target.evaluation = materialized.evaluation;
    Ok(())
}

fn refresh_amended_color_probe(
    step_id: &str,
    target: &mut SessionRecordColorProbeAmendTarget<'_>,
    flags: &FlagArgs,
    auto_region_override: Option<SessionRecordAutoRegionSelection>,
) -> CliOutcome<()> {
    let Some(provenance) = target.frame_provenance.as_deref() else {
        *target.expected = None;
        *target.evaluation = SessionRecordStepEvaluation {
            status: "deferred".to_string(),
            reason: "amended_without_frame_provenance".to_string(),
            auto_region: None,
            backtest: None,
            contrast_backtest: None,
        };
        return Ok(());
    };
    let source_frame = read_session_record_source_frame_from_provenance(provenance)?;
    let resolution = if let Some(auto_region) = auto_region_override {
        SessionRecordAnchorRegionResolution {
            rect: auto_region.selected.clone(),
            auto_region: Some(auto_region),
        }
    } else {
        resolve_session_record_anchor_rect(&source_frame.frame, target.region, None, flags)?
    };
    let expected = mean_session_record_rect_rgb(&source_frame.frame, &resolution.rect)?;
    *target.region = SessionRecordRegion::Rect {
        rect: resolution.rect.clone(),
    };
    *target.expected = Some(expected);
    *target.frame_provenance = Some(Box::new(session_record_frame_provenance(source_frame)));
    *target.evaluation = SessionRecordStepEvaluation {
        status: "passed".to_string(),
        reason: "color_probe_sampled".to_string(),
        auto_region: resolution.auto_region,
        backtest: None,
        contrast_backtest: None,
    };
    if target.id.trim().is_empty() {
        return Err(CliError::usage(format!(
            "record amend color-probe '{step_id}' id must not be empty"
        )));
    }
    Ok(())
}

fn refresh_amended_verify_template(
    context: &SessionRecordAmendContext,
    step_id: &str,
    target: &mut SessionRecordVerifyTemplateAmendTarget<'_>,
    flags: &FlagArgs,
    auto_region_override: Option<SessionRecordAutoRegionSelection>,
) -> CliOutcome<()> {
    let Some(provenance) = target.frame_provenance.as_deref() else {
        *target.artifact = None;
        *target.evaluation = SessionRecordStepEvaluation {
            status: "deferred".to_string(),
            reason: "amended_without_frame_provenance".to_string(),
            auto_region: None,
            backtest: None,
            contrast_backtest: None,
        };
        return Ok(());
    };
    let source_frame = read_session_record_source_frame_from_provenance(provenance)?;
    let resolution = if let Some(auto_region) = auto_region_override {
        SessionRecordAnchorRegionResolution {
            rect: auto_region.selected.clone(),
            auto_region: Some(auto_region),
        }
    } else {
        resolve_session_record_anchor_rect(
            &source_frame.frame,
            target.region,
            *target.threshold,
            flags,
        )?
    };
    let artifact_dir = amended_anchor_artifact_dir(context, target.artifact.as_deref())?;
    let materialized = materialize_anchor_artifact_from_source(
        source_frame,
        resolution,
        &artifact_dir,
        step_id,
        target.id,
        *target.threshold,
        flags,
    )?;
    *target.region = materialized.region.clone();
    *target.frame_provenance = Some(Box::new(materialized.frame_provenance));
    *target.artifact = Some(Box::new(materialized.artifact));
    *target.evaluation = materialized.evaluation;
    Ok(())
}

fn read_session_record_source_frame_from_provenance(
    provenance: &SessionRecordFrameProvenance,
) -> CliOutcome<SessionRecordSourceFrame> {
    let frame_path = PathBuf::from(&provenance.path);
    let frame_png = fs::read(&frame_path).map_err(|err| {
        CliError::usage(format!(
            "failed to read record source frame {} for amend: {err}",
            frame_path.display()
        ))
    })?;
    let backend_name = match provenance.capture_backend.as_deref() {
        Some("nemu_ipc") => CaptureBackendName::NemuIpc,
        Some("droidcast_raw") => CaptureBackendName::DroidcastRaw,
        _ => CaptureBackendName::AdbScreencap,
    };
    let frame = Frame::from_png(frame_png.clone(), backend_name).map_err(|err| {
        CliError::usage(format!(
            "failed to decode record source frame {} for amend: {err}",
            frame_path.display()
        ))
    })?;
    Ok(SessionRecordSourceFrame {
        frame,
        png: frame_png,
        source: provenance.source.clone(),
        path: frame_path,
        recorded_at_unix_ms: provenance.recorded_at_unix_ms,
        capture_backend: provenance.capture_backend.clone(),
        freshness: provenance.freshness.clone(),
        capture_attempts: provenance.capture_attempts.clone(),
    })
}

fn amended_anchor_artifact_dir(
    context: &SessionRecordAmendContext,
    artifact: Option<&SessionRecordAnchorArtifact>,
) -> CliOutcome<PathBuf> {
    let default_dir = session_record_artifact_root(&context.state_dir, &context.record_id);
    let candidate = artifact
        .and_then(|artifact| Path::new(&artifact.path).parent().map(Path::to_path_buf))
        .unwrap_or(default_dir);
    ensure_path_within(
        &context.state_dir,
        &candidate,
        "record amend artifact directory",
        &["record", "artifact_dir"],
    )
}

fn amend_operation_record_step(
    from: &mut String,
    to: &mut Option<String>,
    click: &mut SessionRecordClick,
    destructive: &mut bool,
    flags: &FlagArgs,
) -> CliOutcome<bool> {
    let mut changed = false;
    if let Some(value) = flags.optional("--from").filter(|value| value != "true") {
        if value.trim().is_empty() {
            return Err(CliError::usage("--from must not be empty"));
        }
        *from = value;
        changed = true;
    }
    if let Some(value) = flags.optional("--to").filter(|value| value != "true") {
        if value.trim().is_empty() {
            return Err(CliError::usage("--to must not be empty"));
        }
        *to = if value == "null" { None } else { Some(value) };
        changed = true;
    }
    if let Some(value) = flags.optional("--click").filter(|value| value != "true") {
        *click = parse_session_record_click(&value)?;
        changed = true;
    }
    if flags.bool("--destructive") {
        *destructive = true;
        changed = true;
    }
    if flags.bool("--non-destructive") {
        *destructive = false;
        changed = true;
    }
    Ok(changed)
}

fn session_record_path(state_dir: &Path, instance_id: &str) -> PathBuf {
    state_dir.join(format!("record-{}.json", safe_file_stem(instance_id)))
}

fn new_session_record(instance: &str, task_id: &str, flags: &FlagArgs) -> SessionRecordContext {
    let now = current_unix_ms();
    let holder = flags
        .optional("--holder")
        .or_else(|| flags.optional("--lease-holder"))
        .filter(|value| value != "true");
    let record_id = flags
        .optional("--record-id")
        .filter(|value| value != "true")
        .unwrap_or_else(|| {
            format!(
                "{now}-{}-{}",
                std::process::id(),
                safe_file_stem(task_id.trim())
            )
        });
    SessionRecordContext {
        schema_version: "session-record-v0".to_string(),
        record_id,
        task_id: task_id.trim().to_string(),
        instance: instance.to_string(),
        status: "active".to_string(),
        holder,
        lease_id: flags.optional("--lease-id").filter(|value| value != "true"),
        started_at_unix_ms: now,
        updated_at_unix_ms: now,
        steps: Vec::new(),
    }
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
