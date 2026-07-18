// SPDX-License-Identifier: AGPL-3.0-only

#![allow(clippy::result_large_err)]

use actingcommand_contract::{
    ApplicationLifecycleAction, CLI_SCHEMA_VERSION, Envelope, EventActor, EventSource,
    LabError as CliError, LabErrorClass as ErrorKind, LedgerProjection,
};
#[cfg(test)]
use actingcommand_device::DeviceTarget;
use actingcommand_device::{
    AdbPathSource, CaptureBackendChoice, CaptureBackendName, Frame, InputBackend, PixelFormat,
    TouchBackendChoice, combine_operation_and_close, resolve_adb_path,
    vendor_stdio_session_diagnostic,
};
use actingcommand_lab::{
    InstanceConfig, PackageValidationResponse, SemanticLedgerContext, SemanticRequestContext,
    UserConfig, derive_absolute_coordinate_rect_from_match, project_semantic_payload,
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
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::env;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, IsTerminal, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Component, Path, PathBuf};
#[cfg(any(test, unix))]
use std::process::Command;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use zip::{ZipWriter, write::FileOptions};

mod contained_resources;
mod drive_cli;
mod env_detection;
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
mod runtime_capture_backend;
mod runtime_debug;
mod runtime_input_backend;
mod runtime_session_adapter;
mod runtime_slice_cli;
mod runtime_stream_adapter;
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
static JSON_TMP_SEQ: AtomicU64 = AtomicU64::new(0);
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

#[derive(Debug)]
struct CliResult {
    print_json: bool,
    envelope: Envelope<Value>,
    human: String,
    exit_code: i32,
}

impl CliResult {
    fn ok(command: String, data: Value, print_json: bool, human: String) -> Self {
        Self {
            print_json,
            envelope: Envelope::ok(
                SCHEMA_VERSION,
                env!("CARGO_PKG_VERSION"),
                RUNTIME_VERSION,
                command,
                data,
            ),
            human,
            exit_code: 0,
        }
    }

    fn err(command: String, err: CliError, print_json: bool) -> Self {
        let exit_code = err.exit_code();
        let human = format!("{}: {}", err.code, err.message);
        Self {
            print_json,
            envelope: Envelope::err(
                SCHEMA_VERSION,
                env!("CARGO_PKG_VERSION"),
                RUNTIME_VERSION,
                command,
                err,
            ),
            human,
            exit_code,
        }
    }

    fn exit_code(&self) -> i32 {
        self.exit_code
    }

    fn envelope_json(&self) -> String {
        serde_json::to_string(&self.envelope).unwrap_or_else(|err| {
            format!(r#"{{"ok":false,"error":"json_serialize_failed:{err}"}}"#)
        })
    }

    fn human_text(&self) -> String {
        self.human.clone()
    }
}

trait CliErrorExitCode {
    fn exit_code(&self) -> i32;
}

impl CliErrorExitCode for CliError {
    fn exit_code(&self) -> i32 {
        match self.class {
            ErrorKind::UsageValidation => 2,
            ErrorKind::SafetyBlocked => 3,
            ErrorKind::DeviceInstance => 4,
            ErrorKind::RuntimeUnavailable => 5,
            ErrorKind::NotImplemented => 6,
        }
    }
}

type CliOutcome<T> = Result<T, CliError>;

#[derive(Debug, Clone, Default)]
struct GlobalOptions {
    json: bool,
    run_root: Option<PathBuf>,
    instance: Option<String>,
    instances: Vec<String>,
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
                global.instances = split_csv(&require_raw(&raw, index, "--instances")?);
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

    let (command, args) = if global.version {
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
        [group, sub] if group == "run" => run_run_report(sub, &invocation.global, &invocation.args),
        [group, sub] if group == "report" => run_report(sub, &invocation.global, &invocation.args),
        _ => Err(CliError::usage(format!(
            "unknown actinglab command: {}",
            invocation.command.join(" ")
        ))),
    }
}

fn human_summary(command: &str, data: &Value) -> String {
    match data {
        Value::String(text) => text.clone(),
        _ => format!("{command} ok"),
    }
}

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
                "--maa-tasks <dir>"
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

fn run_capabilities(global: &GlobalOptions) -> CliOutcome<Value> {
    let config = read_user_config()?;
    let root = effective_resource_root(global, &config);
    let discovered = match root {
        Some(root) if root.is_dir() => discover_recognition_packs(&root)?,
        _ => Vec::new(),
    };
    let recognition_match_policy = discovered
        .iter()
        .map(|pack| {
            json!({
                "game": pack.get("game"),
                "server": pack.get("server"),
                "locale": pack.get("locale"),
                "match_metric": pack.get("match_metric")
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "commands": command_capabilities(),
        "session_layer": session_layer_capability_contract(),
        "exit_codes": exit_code_table(),
        "recognition_match_policy": recognition_match_policy,
        "capture_backends": [
            {"id": "adb", "backend": "adb_screencap", "external_tool": false},
            {"id": "droidcast_raw", "backend": "droidcast_raw", "external_tool_env": "ACTINGCOMMAND_DROIDCAST_RAW_APK"},
            {"id": "nemu_ipc", "backend": "nemu_ipc", "external_tool_env": "ACTINGCOMMAND_NEMU_FOLDER or ACTINGCOMMAND_NEMU_IPC_DLL"},
            {"id": "auto", "fallback_allowed": true, "diagnostics_required": true},
            {"id": "auto-fastest", "probe_all_backends": true, "diagnostics_required": true}
        ],
        "lab2_cli": lab2_cli::capability_summary(&config),
        "discovered_recognition_packs": discovered
    }))
}

fn session_layer_capability_contract() -> Value {
    json!({
        "schema_version": "session.capabilities.v0.1",
        "resident_daemon": {
            "request_command": "session request capabilities",
            "bootstrap_command": "session bootstrap",
            "throat_policy_command": "session throat-policy",
            "capture_policy_command": "session capture-policy",
            "self_heal_policy_command": "session self-heal-policy",
            "self_heal_plan_command": "session self-heal-plan [--trigger <kind>] [--to <page>]",
            "phase_c_plan_command": "session phase-c-plan [--endpoint <url>] [--trigger <kind>] [--to <page>]",
            "status_command": "session status --diagnostics",
            "readiness_command": "session readiness",
            "validation_plan_command": "session validation-plan",
            "status_instance_registry_field": "diagnostics.instances",
            "monitor_policy_command": "session monitor-policy status",
            "journal_command": "session journal"
        },
        "access_channels": [
            {
                "id": "local_cli",
                "status": "available",
                "encryption_required": false,
                "reason": "local operator command surface"
            },
            {
                "id": "trusted_remote",
                "status": "reserved",
                "encryption_required": true,
                "authentication_required": true,
                "plan_command": "session transport plan [--endpoint <url>]",
                "preflight_command": "session transport check --endpoint <url>",
                "auth_env": {
                    "token": TRUSTED_REMOTE_TOKEN_ENV,
                    "client_certificate": TRUSTED_REMOTE_CLIENT_CERT_ENV
                },
                "blocked_without_auth_code": "trusted_remote_auth_required",
                "blocked_without_encryption_code": "trusted_remote_transport_blocked",
                "reason": "future UI/API channel must be authenticated and encrypted"
            }
        ],
        "request_classes": {
            "read_only": {
                "requires_lease": false,
                "examples": ["status", "queue", "journal", "capabilities", "devices", "session bootstrap", "session throat-policy", "session capture-policy", "session record-policy", "session self-heal-policy", "session self-heal-plan", "session phase-c-plan", "session transport plan", "session transport check", "session connect-plan", "session stream-plan", "session submit-plan", "session validation-plan", "session instance registry", "capture", "stream", "session recover --stale-capture", "session record step --capture", "session record step --current-frame", "session monitor-policy status"],
                "device_affecting_examples": ["capture", "stream", "session record step --capture", "session record step --current-frame"]
            },
            "daemon_state": {
                "requires_lease": false,
                "recovery_policy_requires_matching_lease": true,
                "recovery_policy_defers_without_matching_lease": true,
                "examples": ["session monitor-policy set", "session monitor-policy clear", "session record start", "session record step --frame <png>", "session record amend", "session record build-task", "session record promote"]
            },
            "control": {
                "requires_lease": true,
                "examples": ["tap", "swipe", "long-tap", "key", "text", "stream --input-relay", "stream --input-event <action,args>", "stream --relay-event <action,args>", "session app launch", "session app stop", "session app force-stop", "session app restart", "session instance app launch", "session instance app stop", "session instance app force-stop", "session instance app restart", "tap-target", "navigate", "recover except --stale-capture"]
            }
        },
        "safety": {
            "session_layer_only_throat": true,
            "strict_session_throat_flag": "--require-session",
            "strict_session_throat_env": REQUIRE_SESSION_DAEMON_ENV,
            "strict_session_throat_failure_code": "session_daemon_required",
            "ui_must_not_directly_touch_adb_or_device": true,
            "control_requests_require_matching_lease": true,
            "severe_errors_fail_loud": true
        }
    })
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
            "known_stale_frame_md5": "202752fa3e5cab706774819168639b6c",
            "finding": "FINDING-capture-staleness-2026-06-27"
        },
        "freeze_classification_gate": {
            "schema_version": "session.capture_freeze_classification_gate.v0.1",
            "status": "blocked_without_fresh_backend_evidence",
            "safe_to_classify_game_frozen": false,
            "must_not_classify_as_game_freeze_from_adb_screencap_alone": true,
            "finding": "FINDING-capture-staleness-2026-06-27",
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
            "competitive_or_exercise_allowed": false,
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

fn run_touch_probe(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    flags.expect_positionals("touch-probe", 0)?;
    if parse_touch_backend_override(&flags)?.is_some() || global.touch_backend.is_some() {
        return Err(CliError::usage(
            "touch-probe backend selection is owned by actingd; remove --touch-backend",
        ));
    }
    let config = read_user_config()?;
    let (mut backend, instance_alias) = open_cli_runtime_input_proxy(global, &config)?;
    backend
        .close()
        .map_err(|error| CliError::device(error.to_string()))?;
    Ok(json!({
        "status": "available",
        "mode": "touch_probe",
        "requested_backend": "runtime_owned",
        "selected_backend": "runtime_proxy",
        "instance": instance_alias,
        "adb_source": "runtime_owned",
        "adb_warning": Value::Null,
        "action_executed": false,
        "touch_backend_attempts": [],
        "touch_backend_warnings": []
    }))
}

fn parse_touch_backend_override(flags: &FlagArgs) -> CliOutcome<Option<TouchBackendChoice>> {
    let Some(value) = flags.optional("--touch-backend") else {
        return Ok(None);
    };
    if value == "true" {
        return Err(CliError::usage(
            "--touch-backend expects auto, auto-fastest, maatouch, minitouch, or adb_shell_input",
        ));
    }
    TouchBackendChoice::parse(&value)
        .map(Some)
        .map_err(|err| CliError::usage(err.to_string()))
}

fn run_capture(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    if flags.bool("--diagnose")
        || flags
            .positionals
            .first()
            .is_some_and(|value| value == "diagnose")
    {
        return run_capture_diagnose(global, &flags);
    }
    reject_legacy_session_routing(&flags)?;
    let out = flags.required_path("--out")?;
    let config = read_user_config()?;
    let device_config = device_config(global, &config)?;
    let requested = device_config.capture_backend;
    let fresh_delay = parse_optional_duration_ms(&flags, "--fresh-delay-ms", 160)?;
    let captured = capture_for_command(
        &device_config,
        requested,
        flags.bool("--require-fresh"),
        fresh_delay,
    )?;
    let frame = captured.frame;
    if let Some(parent) = out.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|err| {
            CliError::device(format!("failed to create {}: {err}", parent.display()))
        })?;
    }
    let png = frame
        .png_for_artifact()
        .map_err(|err| CliError::device(err.to_string()))?;
    fs::write(&out, &png)
        .map_err(|err| CliError::device(format!("failed to write {}: {err}", out.display())))?;
    Ok(json!({
        "width": frame.width,
        "height": frame.height,
        "capture_backend_used": frame.backend_name.as_str(),
        "adb_source": device_config.adb_source.as_str(),
        "adb_warning": device_config.adb_warning,
        "capture_backend_attempts": captured.attempts,
        "freshness": captured.freshness,
        "out": out.display().to_string()
    }))
}

fn run_capture_diagnose(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    reject_legacy_session_routing(flags)?;
    let config = read_user_config()?;
    let device_config = device_config(global, &config)?;
    let requested = device_config.capture_backend;
    let fresh_delay = parse_optional_duration_ms(flags, "--fresh-delay-ms", 160)?;
    let expectation = if flags.bool("--require-fresh") {
        CaptureFreshnessExpectation::ExpectedChange
    } else {
        CaptureFreshnessExpectation::StaticPageAllowed
    };
    let report = capture_fresh_probe_report(&device_config, requested, fresh_delay, expectation)?;
    Ok(json!({
        "status": report.status.as_str(),
        "mode": "capture_diagnose",
        "requested_backend": requested.as_str(),
        "adb_source": device_config.adb_source.as_str(),
        "adb_warning": device_config.adb_warning,
        "click_allowed": false,
        "action_executed": false,
        "freshness": report.freshness,
        "capture_backend_attempts": report.attempts,
        "frame": report.frame.as_ref().map(capture_frame_summary_json),
        "recovery": capture_diagnosis_recovery_json(report.status, requested)
    }))
}

fn reject_legacy_session_routing(flags: &FlagArgs) -> CliOutcome<()> {
    if flags.bool("--via-daemon")
        || flags.bool("--local")
        || flags.optional("--state-dir").is_some()
    {
        return Err(CliError::not_implemented(
            "legacy_session_authority_retired",
            "legacy Session daemon and file-state routing were retired; use the resident Runtime",
        ));
    }
    Ok(())
}

struct CaptureCommandResult {
    frame: Frame,
    attempts: Vec<Value>,
    freshness: Value,
}

struct CaptureFreshProbeReport {
    status: CaptureFreshProbeStatus,
    frame: Option<Frame>,
    attempts: Vec<Value>,
    freshness: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureFreshProbeStatus {
    Fresh,
    StaticUnchanged,
    StaleSuspected,
}

impl CaptureFreshProbeStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::StaticUnchanged => "static_unchanged",
            Self::StaleSuspected => "stale_suspected",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureFreshnessExpectation {
    StaticPageAllowed,
    ExpectedChange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CaptureFreshnessDecision {
    status: CaptureFreshProbeStatus,
    ok: bool,
    stale_suspected: bool,
    reason: &'static str,
}

fn capture_for_command(
    device_config: &DeviceRuntimeConfig,
    _requested: CaptureBackendChoice,
    require_fresh: bool,
    fresh_delay: Duration,
) -> CliOutcome<CaptureCommandResult> {
    if require_fresh {
        return capture_require_fresh(device_config, fresh_delay);
    }

    let frame = runtime_capture_backend::capture_runtime_sequence(
        &device_config.runtime_capture_endpoint(),
        1,
        Duration::ZERO,
    )
    .map_err(|err| CliError::device(err.to_string()))?
    .into_iter()
    .next()
    .ok_or_else(|| CliError::device("Runtime capture returned no frame"))?;
    let attempts = vec![json!({
        "backend": frame.backend_name.as_str(),
        "ok": true,
        "stage": "runtime_observation",
        "authority": "runtime_execution_kernel"
    })];
    Ok(CaptureCommandResult {
        frame,
        attempts,
        freshness: json!({ "required": false, "authority": "runtime_execution_kernel" }),
    })
}

fn capture_require_fresh(
    device_config: &DeviceRuntimeConfig,
    fresh_delay: Duration,
) -> CliOutcome<CaptureCommandResult> {
    let report = capture_fresh_probe_report(
        device_config,
        device_config.capture_backend,
        fresh_delay,
        CaptureFreshnessExpectation::ExpectedChange,
    )?;
    if let Some(frame) = report.frame {
        return Ok(CaptureCommandResult {
            frame,
            attempts: report.attempts,
            freshness: report.freshness,
        });
    }

    Err(CliError::device(format!(
        "fresh capture required but Runtime did not produce a changing probe frame; attempts={}",
        serde_json::to_string(&report.attempts).unwrap_or_else(|_| "[]".to_string())
    )))
}

fn capture_fresh_probe_report(
    device_config: &DeviceRuntimeConfig,
    requested: CaptureBackendChoice,
    fresh_delay: Duration,
    expectation: CaptureFreshnessExpectation,
) -> CliOutcome<CaptureFreshProbeReport> {
    let frames = runtime_capture_backend::capture_runtime_sequence(
        &device_config.runtime_capture_endpoint(),
        2,
        fresh_delay,
    )
    .map_err(|err| CliError::device(err.to_string()))?;
    let [first, second]: [Frame; 2] = frames.try_into().map_err(|frames: Vec<Frame>| {
        CliError::device(format!(
            "Runtime fresh capture returned {} frames instead of 2",
            frames.len()
        ))
    })?;
    let backend_used = second.backend_name.as_str();
    let first_hash = frame_digest(&first);
    let second_hash = frame_digest(&second);
    let decision = classify_capture_freshness(&first_hash, &second_hash, expectation);
    let attempts = vec![json!({
        "backend": backend_used,
        "ok": decision.ok,
        "stage": "runtime_capture_sequence",
        "authority": "runtime_execution_kernel",
        "first_hash": first_hash,
        "second_hash": second_hash,
        "expectation": capture_freshness_expectation_label(expectation),
        "reason": decision.reason,
        "stale_suspected": decision.stale_suspected,
        "delay_ms": fresh_delay.as_millis()
    })];
    Ok(CaptureFreshProbeReport {
        status: decision.status,
        frame: decision.ok.then_some(second),
        attempts,
        freshness: json!({
            "required": true,
            "fresh": decision.ok,
            "status": decision.status.as_str(),
            "backend": backend_used,
            "requested_backend": requested.as_str(),
            "authority": "runtime_execution_kernel",
            "expectation": capture_freshness_expectation_label(expectation),
            "reason": decision.reason,
            "first_hash": first_hash,
            "second_hash": second_hash
        }),
    })
}

fn classify_capture_freshness(
    first_hash: &str,
    second_hash: &str,
    expectation: CaptureFreshnessExpectation,
) -> CaptureFreshnessDecision {
    if first_hash != second_hash {
        return CaptureFreshnessDecision {
            status: CaptureFreshProbeStatus::Fresh,
            ok: true,
            stale_suspected: false,
            reason: "frame_changed",
        };
    }

    match expectation {
        CaptureFreshnessExpectation::StaticPageAllowed => CaptureFreshnessDecision {
            status: CaptureFreshProbeStatus::StaticUnchanged,
            ok: true,
            stale_suspected: false,
            reason: "static_page_unchanged",
        },
        CaptureFreshnessExpectation::ExpectedChange => CaptureFreshnessDecision {
            status: CaptureFreshProbeStatus::StaleSuspected,
            ok: false,
            stale_suspected: true,
            reason: "expected_change_not_observed",
        },
    }
}

fn capture_freshness_expectation_label(expectation: CaptureFreshnessExpectation) -> &'static str {
    match expectation {
        CaptureFreshnessExpectation::StaticPageAllowed => "static_page_allowed",
        CaptureFreshnessExpectation::ExpectedChange => "expected_change",
    }
}

fn frame_digest(frame: &Frame) -> String {
    let mut hasher = Sha256::new();
    hasher.update(frame.width.to_le_bytes());
    hasher.update(frame.height.to_le_bytes());
    hasher.update(format!("{:?}", frame.pixel_format).as_bytes());
    hasher.update(&frame.pixels);
    format!("{:x}", hasher.finalize())
}

fn capture_frame_summary_json(frame: &Frame) -> Value {
    json!({
        "width": frame.width,
        "height": frame.height,
        "backend": frame.backend_name.as_str(),
        "digest": frame_digest(frame)
    })
}

fn capture_fresh_probe_report_json(
    report: &CaptureFreshProbeReport,
    requested: CaptureBackendChoice,
) -> Value {
    json!({
        "diagnose_requested": true,
        "status": report.status.as_str(),
        "requested_backend": requested.as_str(),
        "freshness": report.freshness,
        "capture_backend_attempts": report.attempts,
        "frame": report.frame.as_ref().map(capture_frame_summary_json),
        "recovery": capture_diagnosis_recovery_json(report.status, requested)
    })
}

#[cfg(test)]
fn instance_health_status(capture_status: Option<CaptureFreshProbeStatus>) -> &'static str {
    match capture_status {
        Some(CaptureFreshProbeStatus::Fresh) => "healthy",
        Some(CaptureFreshProbeStatus::StaticUnchanged) => "healthy_static",
        Some(CaptureFreshProbeStatus::StaleSuspected) => "capture_stale_suspected",
        None => "device_connected",
    }
}

fn capture_diagnosis_recovery_json(
    status: CaptureFreshProbeStatus,
    requested: CaptureBackendChoice,
) -> Value {
    match status {
        CaptureFreshProbeStatus::Fresh | CaptureFreshProbeStatus::StaticUnchanged => json!({
            "needed": false,
            "available": false,
            "reason": match status {
                CaptureFreshProbeStatus::Fresh => "fresh_frame_observed",
                CaptureFreshProbeStatus::StaticUnchanged => "static_page_unchanged",
                _ => "fresh_frame_observed",
            }
        }),
        CaptureFreshProbeStatus::StaleSuspected => {
            let mut recommendations = Vec::new();
            if requested == CaptureBackendChoice::Adb {
                recommendations.push(json!({
                    "type": "capture_backend",
                    "command": "capture diagnose --capture-backend auto",
                    "reason": "adb_screencap returned identical probe frames; prefer fast backends before concluding the game is frozen"
                }));
            }
            recommendations.push(json!({
                "type": "configure_backend",
                "backend": "nemu_ipc",
                "reason": "MuMu IPC can bypass stale adb_screencap surfaces when configured"
            }));
            recommendations.push(json!({
                "type": "configure_backend",
                "backend": "droidcast_raw",
                "reason": "DroidCast_raw can provide an alternate capture surface when adb_screencap is stale"
            }));
            recommendations.push(json!({
                "type": "app_restart",
                "command": "session app restart",
                "reason": "heavy recovery; rebuilds the game capture surface only after lighter capture-backend checks fail"
            }));
            json!({
                "needed": true,
                "available": true,
                "reason": "stale_capture_suspected",
                "recommendations": recommendations
            })
        }
    }
}

fn parse_optional_duration_ms(
    flags: &FlagArgs,
    name: &str,
    default_ms: u64,
) -> CliOutcome<Duration> {
    let Some(value) = flags.optional(name).filter(|value| value != "true") else {
        return Ok(Duration::from_millis(default_ms));
    };
    let ms = value
        .parse::<u64>()
        .map_err(|err| CliError::usage(format!("failed to parse {name} '{value}': {err}")))?;
    Ok(Duration::from_millis(ms))
}

fn parse_optional_usize(flags: &FlagArgs, name: &str, default_value: usize) -> CliOutcome<usize> {
    let Some(value) = flags.optional(name).filter(|value| value != "true") else {
        return Ok(default_value);
    };
    value
        .parse::<usize>()
        .map_err(|err| CliError::usage(format!("failed to parse {name} '{value}': {err}")))
}

fn parse_optional_string_value(flags: &FlagArgs, name: &str) -> CliOutcome<Option<String>> {
    match flags.optional(name) {
        None => Ok(None),
        Some(value) if value == "true" => Err(CliError::usage(format!("missing {name} <value>"))),
        Some(value) if value.trim().is_empty() => {
            Err(CliError::usage(format!("{name} must not be empty")))
        }
        Some(value) => Ok(Some(value)),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DirectTouchCommand {
    Tap {
        x: i32,
        y: i32,
    },
    Swipe {
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        duration_ms: u64,
    },
    LongTap {
        x: i32,
        y: i32,
        duration_ms: u64,
    },
}

impl DirectTouchCommand {
    fn parse(command: &str, flags: &FlagArgs) -> CliOutcome<Self> {
        flags.reject_flags(command)?;
        match command {
            "tap" => {
                flags.expect_positionals(command, 2)?;
                Ok(Self::Tap {
                    x: flags.required_i32(0, "tap x")?,
                    y: flags.required_i32(1, "tap y")?,
                })
            }
            "swipe" => {
                flags.expect_positionals(command, 5)?;
                Ok(Self::Swipe {
                    x1: flags.required_i32(0, "swipe x1")?,
                    y1: flags.required_i32(1, "swipe y1")?,
                    x2: flags.required_i32(2, "swipe x2")?,
                    y2: flags.required_i32(3, "swipe y2")?,
                    duration_ms: flags.required_u64(4, "swipe duration_ms")?,
                })
            }
            "long-tap" => {
                flags.expect_positionals(command, 3)?;
                Ok(Self::LongTap {
                    x: flags.required_i32(0, "long-tap x")?,
                    y: flags.required_i32(1, "long-tap y")?,
                    duration_ms: flags.required_u64(2, "long-tap duration_ms")?,
                })
            }
            other => Err(CliError::usage(format!(
                "unknown direct touch command: {other}"
            ))),
        }
    }

    fn run(&self, backend: &mut dyn InputBackend) -> actingcommand_device::DeviceResult<()> {
        match *self {
            Self::Tap { x, y } => backend.tap(x, y),
            Self::Swipe {
                x1,
                y1,
                x2,
                y2,
                duration_ms,
            } => backend.swipe(x1, y1, x2, y2, duration_ms),
            Self::LongTap { x, y, duration_ms } => backend.long_tap(x, y, duration_ms),
        }
    }

    fn to_json(&self) -> Value {
        match *self {
            Self::Tap { x, y } => json!({
                "type": "tap",
                "x": x,
                "y": y
            }),
            Self::Swipe {
                x1,
                y1,
                x2,
                y2,
                duration_ms,
            } => json!({
                "type": "swipe",
                "x1": x1,
                "y1": y1,
                "x2": x2,
                "y2": y2,
                "duration_ms": duration_ms
            }),
            Self::LongTap { x, y, duration_ms } => json!({
                "type": "long-tap",
                "x": x,
                "y": y,
                "duration_ms": duration_ms
            }),
        }
    }
}

fn open_cli_runtime_input_proxy(
    global: &GlobalOptions,
    config: &UserConfig,
) -> CliOutcome<(runtime_input_backend::RuntimeInputBackend, String)> {
    let instance_alias = resolve_instance_id(global, config)?;
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        runtime_state_root()?,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .map_err(|error| CliError::device(error.to_string()))?;
    let proxy = runtime_input_backend::RuntimeInputBackend::connect(client, &instance_alias)
        .map_err(|error| CliError::device(error.to_string()))?;
    Ok((proxy, instance_alias))
}

fn run_direct_touch(global: &GlobalOptions, command: &str, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    reject_legacy_session_routing(&flags)?;
    let command = DirectTouchCommand::parse(command, &flags)?;
    let config = read_user_config()?;
    send_direct_touch_command(
        global,
        &config,
        &command,
        "direct_trusted_manual",
        "not_required_for_manual_control",
    )
}

fn send_direct_touch_command(
    global: &GlobalOptions,
    config: &UserConfig,
    command: &DirectTouchCommand,
    control_mode: &str,
    safety_gate: &str,
) -> CliOutcome<Value> {
    let (mut backend, instance_alias) = open_cli_runtime_input_proxy(global, config)?;
    let operation = command.run(&mut backend);
    let close = backend.close();
    combine_operation_and_close(operation, close)
        .map_err(|err| CliError::device(err.to_string()))?;
    Ok(json!({
        "status": "sent",
        "backend": "runtime_proxy",
        "touch_backend_requested": "runtime_owned",
        "adb_source": "runtime_owned",
        "adb_warning": Value::Null,
        "touch_backend_attempts": [],
        "touch_backend_warnings": [],
        "control_mode": control_mode,
        "safety_gate": safety_gate,
        "instance": instance_alias,
        "serial": Value::Null,
        "device_state": "runtime_owned",
        "screen_size": Value::Null,
        "handshake": Value::Null,
        "action": command.to_json()
    }))
}

fn run_direct_input(global: &GlobalOptions, command: &str, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    reject_legacy_session_routing(&flags)?;
    let command = DirectInputCommand::parse(command, &flags)?;
    let config = read_user_config()?;
    let (mut backend, instance_alias) = open_cli_runtime_input_proxy(global, &config)?;
    let operation = command.run(&mut backend);
    let close = backend.close();
    combine_operation_and_close(operation, close)
        .map_err(|err| CliError::device(err.to_string()))?;
    Ok(json!({
        "status": "sent",
        "backend": "runtime_proxy",
        "touch_backend_requested": "runtime_owned",
        "adb_source": "runtime_owned",
        "adb_warning": Value::Null,
        "touch_backend_attempts": [],
        "touch_backend_warnings": [],
        "control_mode": "direct_trusted_manual",
        "safety_gate": "not_required_for_manual_control",
        "instance": instance_alias,
        "serial": Value::Null,
        "device_state": "runtime_owned",
        "screen_size": Value::Null,
        "handshake": Value::Null,
        "action": command.to_json()
    }))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DirectInputCommand {
    Key(String),
    Text(String),
}

impl DirectInputCommand {
    fn parse(command: &str, flags: &FlagArgs) -> CliOutcome<Self> {
        flags.reject_flags(command)?;
        match command {
            "key" => {
                flags.expect_positionals("key", 1)?;
                Ok(Self::Key(canonical_key(
                    flags.required_positional(0, "key")?,
                )))
            }
            "text" => {
                if flags.positionals.is_empty() {
                    return Err(CliError::usage(
                        "text expects at least one positional argument",
                    ));
                }
                Ok(Self::Text(flags.positionals.join(" ")))
            }
            other => Err(CliError::usage(format!(
                "unknown direct input command: {other}"
            ))),
        }
    }

    fn run(&self, backend: &mut dyn InputBackend) -> actingcommand_device::DeviceResult<()> {
        match self {
            Self::Key(key) => backend.key(key),
            Self::Text(text) => backend.text(text),
        }
    }

    fn to_json(&self) -> Value {
        match self {
            Self::Key(key) => json!({ "type": "key", "key": key }),
            Self::Text(text) => json!({ "type": "text", "text": text }),
        }
    }
}

fn canonical_key(key: &str) -> String {
    let lower = key.to_ascii_lowercase();
    match lower.as_str() {
        "back" => "4".to_string(),
        "home" => "3".to_string(),
        "menu" => "82".to_string(),
        "enter" => "66".to_string(),
        "escape" | "esc" => "111".to_string(),
        _ => key.to_string(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum StreamInputRelayAction {
    Touch(DirectTouchCommand),
    Input(DirectInputCommand),
}

impl StreamInputRelayAction {
    fn parse_many(flags: &FlagArgs) -> CliOutcome<Vec<Self>> {
        let mut actions = Vec::new();
        if let Some((action, action_args)) = stream_input_relay_action(flags)? {
            actions.push(Self::parse_parts(action, action_args)?);
        }
        for spec in flags
            .values("--input-event")
            .into_iter()
            .chain(flags.values("--relay-event"))
        {
            actions.push(Self::parse_event_spec(&spec)?);
        }
        if actions.len() > 16 {
            return Err(CliError::usage(
                "stream input relay accepts at most 16 input events per bounded stream request",
            ));
        }
        Ok(actions)
    }

    fn parse_event_spec(spec: &str) -> CliOutcome<Self> {
        let mut parts = spec.split(',').map(str::trim);
        let action = parts
            .next()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                CliError::usage(
                    "--input-event expects action,args, for example tap,10,20 or key,back",
                )
            })?
            .to_string();
        let action_args = if action == "text" {
            let text = parts.collect::<Vec<_>>().join(",");
            if text.is_empty() {
                Vec::new()
            } else {
                vec![text]
            }
        } else {
            parts
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .collect()
        };
        Self::parse_parts(action, action_args)
    }

    fn parse_parts(action: String, action_args: Vec<String>) -> CliOutcome<Self> {
        let action_flags = FlagArgs {
            flags: BTreeMap::new(),
            positionals: action_args,
        };
        match action.as_str() {
            "tap" | "swipe" | "long-tap" => {
                DirectTouchCommand::parse(&action, &action_flags).map(Self::Touch)
            }
            "key" | "text" => DirectInputCommand::parse(&action, &action_flags).map(Self::Input),
            other => Err(CliError::usage(format!(
                "unsupported stream input relay action: {other}"
            ))),
        }
    }

    fn run(&self, backend: &mut dyn InputBackend) -> actingcommand_device::DeviceResult<()> {
        match self {
            Self::Touch(command) => command.run(backend),
            Self::Input(command) => command.run(backend),
        }
    }

    fn to_json(&self) -> Value {
        match self {
            Self::Touch(command) => command.to_json(),
            Self::Input(command) => command.to_json(),
        }
    }
}

fn stream_input_relay_action(flags: &FlagArgs) -> CliOutcome<Option<(String, Vec<String>)>> {
    let Some(value) = flags
        .optional("--input-relay")
        .or_else(|| flags.optional("--interactive-input"))
    else {
        return Ok(None);
    };
    if value == "true" {
        let action = flags.positionals.first().cloned().ok_or_else(|| {
            CliError::usage("stream --input-relay expects an action: tap|swipe|long-tap|key|text")
        })?;
        return Ok(Some((
            action,
            flags.positionals.iter().skip(1).cloned().collect(),
        )));
    }
    Ok(Some((value, flags.positionals.clone())))
}

fn run_stream_input_relay(
    global: &GlobalOptions,
    config: &UserConfig,
    actions: &[StreamInputRelayAction],
    dry_run: bool,
) -> CliOutcome<Value> {
    let action_values = actions
        .iter()
        .map(StreamInputRelayAction::to_json)
        .collect::<Vec<_>>();
    if dry_run {
        return Ok(json!({
            "status": "planned",
            "mode": "dry_run",
            "action_count": actions.len(),
            "action": action_values.first().cloned(),
            "actions": action_values
        }));
    }
    let (mut backend, instance_alias) = open_cli_runtime_input_proxy(global, config)?;
    let operation = actions
        .iter()
        .try_for_each(|action| action.run(&mut backend));
    let close = backend.close();
    combine_operation_and_close(operation, close)
        .map_err(|err| CliError::device(err.to_string()))?;
    Ok(json!({
        "status": "sent",
        "backend": "runtime_proxy",
        "touch_backend_requested": "runtime_owned",
        "adb_source": "runtime_owned",
        "adb_warning": Value::Null,
        "touch_backend_attempts": [],
        "touch_backend_warnings": [],
        "control_mode": "stream_input_relay",
        "instance": instance_alias,
        "serial": Value::Null,
        "device_state": "runtime_owned",
        "screen_size": Value::Null,
        "handshake": Value::Null,
        "action_count": actions.len(),
        "action": action_values.first().cloned(),
        "actions": action_values
    }))
}

fn run_recognize(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    readonly_cli::run_recognize(global, args)
}

fn run_detect_page(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    readonly_cli::run_detect_page(global, args)
}

fn run_current_page(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    readonly_cli::run_current_page(global, args)
}

fn run_is_visible(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    readonly_cli::run_is_visible(global, args)
}

fn run_locate(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
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

fn semantic_ledger_context(
    command: &'static str,
    global: &GlobalOptions,
    args: &[String],
) -> SemanticLedgerContext {
    SemanticLedgerContext::new(SemanticRequestContext {
        command: command.to_string(),
        instance: global
            .instance
            .clone()
            .unwrap_or_else(|| "default".to_string()),
        arguments: args.to_vec(),
        dry_run: global.dry_run,
    })
}

fn env_resolved_json(values: &[env_detection::ResolvedEnvValue]) -> Value {
    Value::Array(
        values
            .iter()
            .map(|value| {
                json!({
                    "key": value.key,
                    "value": value.value,
                    "confidence": value.confidence,
                    "source": value.source,
                    "detector_id": value.detector_id,
                    "source_result": value.source_result
                })
            })
            .collect(),
    )
}

fn attach_env_resolved(payload: &mut Value, values: &[env_detection::ResolvedEnvValue]) {
    if values.is_empty() {
        return;
    }
    payload["env_resolved"] = env_resolved_json(values);
}

fn env_needs_detection_json(
    command: &str,
    reason: &str,
    subject: &str,
    values: &[env_detection::ResolvedEnvValue],
) -> Option<Value> {
    if values.is_empty() {
        return None;
    }
    let detector_ids = values
        .iter()
        .map(|value| value.detector_id.clone())
        .collect::<BTreeSet<_>>();
    Some(json!({
        "status": "needs_detection",
        "reason": reason,
        "command": command,
        "subject": subject,
        "detector_ids": detector_ids.into_iter().collect::<Vec<_>>(),
        "keys": env_resolved_json(values),
        "recommended_action": "run_detect"
    }))
}

fn record_env_needs_detection(
    ledger: &mut SemanticLedgerContext,
    command: &str,
    reason: &str,
    subject: &str,
    values: &[env_detection::ResolvedEnvValue],
) -> CliOutcome<()> {
    if let Some(needs_detection) = env_needs_detection_json(command, reason, subject, values) {
        ledger.record_drive(json!({
            "stage": "env_needs_detection",
            "command": command,
            "needs_detection": needs_detection
        }))?;
    }
    Ok(())
}

fn record_env_resolved(
    ledger: &mut SemanticLedgerContext,
    command: &str,
    values: &[env_detection::ResolvedEnvValue],
) -> CliOutcome<()> {
    if values.is_empty() {
        return Ok(());
    }
    ledger.record_drive(json!({
        "stage": "env_resolved",
        "command": command,
        "keys": env_resolved_json(values)
    }))?;
    Ok(())
}

fn finish_semantic_result_with_ledger(
    global: &GlobalOptions,
    ctx: SemanticLedgerContext,
    result: CliOutcome<Value>,
) -> CliOutcome<Value> {
    match result {
        Ok(payload) => finish_semantic_payload_with_ledger(global, ctx, payload),
        Err(error) => return_semantic_error_with_ledger(global, ctx, error),
    }
}

fn finish_semantic_payload_with_ledger(
    _global: &GlobalOptions,
    mut ctx: SemanticLedgerContext,
    mut payload: Value,
) -> CliOutcome<Value> {
    if let Some(object) = payload.as_object_mut() {
        object
            .entry("req_id")
            .or_insert_with(|| json!(ctx.req_id.clone()));
        object
            .entry("instance")
            .or_insert_with(|| json!(ctx.instance.clone()));
    }
    let records = ctx.take_records();
    payload["trace_record_count"] = json!(records.len());
    project_semantic_payload(
        payload,
        LedgerProjection::skipped("isolated_offline_projection"),
    )
}

fn return_semantic_error_with_ledger(
    _global: &GlobalOptions,
    mut ctx: SemanticLedgerContext,
    error: CliError,
) -> CliOutcome<Value> {
    let mut payload = json!({
        "req_id": ctx.req_id.clone(),
        "instance": ctx.instance.clone(),
        "command": ctx.command.clone(),
        "error": error.code.clone(),
        "state": "failed",
        "blocked_error": {
            "code": error.code.clone(),
            "message": error.message.clone(),
            "blocked_by": error.blocked_by.clone()
        },
        "details": error.details.clone().unwrap_or(Value::Null)
    });
    payload["trace_record_count"] = json!(ctx.take_records().len());
    payload = project_semantic_payload(
        payload,
        LedgerProjection::skipped("isolated_offline_projection"),
    )?;
    Err(error.with_details(payload))
}

fn run_tap_target(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    drive_cli::run_tap_target(global, args)
}

fn run_navigate(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    drive_cli::run_navigate(global, args)
}

fn run_session_recover(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
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

fn stale_capture_recovery_json(
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
struct PageDetectionOutcome {
    page: String,
    matched: bool,
    standby: bool,
    evaluations: Vec<PageEvaluation>,
}

#[derive(Debug, Clone, Copy)]
struct SemanticPoint {
    x: i32,
    y: i32,
}

#[derive(Debug, Clone)]
enum SemanticInput {
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
struct NavigationGraph {
    game: Option<String>,
    edges: Vec<NavigationEdge>,
    destructive_clicks: Vec<DestructiveClick>,
    control_points: BTreeMap<String, ControlPoint>,
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

#[derive(Debug, Clone)]
struct ControlPoint {
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

fn detect_current_page(
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

fn page_detection_json(outcome: &PageDetectionOutcome) -> Value {
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

fn target_argument(flags: &FlagArgs, command: &str) -> CliOutcome<String> {
    if let Some(target) = flags.optional("--target").filter(|value| value != "true") {
        return Ok(target);
    }
    flags
        .positionals
        .first()
        .cloned()
        .ok_or_else(|| CliError::usage(format!("{command} requires <target> or --target <id>")))
}

fn target_eval_json(evaluation: &TargetEvaluation) -> Value {
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

fn parse_match_metric_flag(flags: &FlagArgs) -> CliOutcome<MatchMetric> {
    match flags
        .optional("--metric")
        .unwrap_or_else(|| "ccorr_normed".to_string())
        .as_str()
    {
        "ccorr_normed" => Ok(MatchMetric::CrossCorrelationNormalized),
        "ccoeff_normed" => Ok(MatchMetric::CorrelationCoefficientNormalized),
        other => Err(CliError::usage(format!(
            "unsupported --metric '{other}', expected ccorr_normed or ccoeff_normed"
        ))),
    }
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

fn parse_navigation_graph_value(value: &Value) -> CliOutcome<NavigationGraph> {
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

fn parse_control_point(value: &Value) -> CliOutcome<ControlPoint> {
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

fn parse_navigation_edge(value: &Value) -> CliOutcome<NavigationEdge> {
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

fn parse_navigation_input(value: &Value) -> CliOutcome<SemanticInput> {
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

fn parse_navigation_tap_rect(value: &Value) -> CliOutcome<PackRect> {
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

fn parse_point_pair(value: &str) -> CliOutcome<(i32, i32)> {
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

fn required_value_field<'a>(value: &'a Value, name: &str) -> CliOutcome<&'a Value> {
    value
        .get(name)
        .ok_or_else(|| CliError::usage(format!("missing field '{name}'")))
}

fn required_string_field<'a>(value: &'a Value, name: &str) -> CliOutcome<&'a str> {
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
        let (prev, index) = previous.get(&cursor)?.clone();
        route.push(edges[index].clone());
        cursor = prev;
    }
    route.reverse();
    Some(route)
}

fn navigation_edge_json(edge: &NavigationEdge) -> Value {
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

fn semantic_input_json(input: &SemanticInput) -> Value {
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

fn reject_destructive_overlap(
    edge: &NavigationEdge,
    destructive: &[DestructiveClick],
) -> CliOutcome<()> {
    reject_destructive_overlap_input(edge, &edge.input, destructive)
}

fn reject_destructive_overlap_input(
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

fn rects_intersect(a: PackRect, b: PackRect) -> bool {
    let ax2 = a.x.saturating_add(a.width);
    let ay2 = a.y.saturating_add(a.height);
    let bx2 = b.x.saturating_add(b.width);
    let by2 = b.y.saturating_add(b.height);
    a.x < bx2 && ax2 > b.x && a.y < by2 && ay2 > b.y
}

fn reject_dangerous_semantic_id(label: &str, value: &str) -> CliOutcome<()> {
    let lower = value.to_ascii_lowercase();
    let dangerous = [
        "random_draw",
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
        "competitive",
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

fn rect_center(rect: PackRect) -> CliOutcome<SemanticPoint> {
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

fn point_json(point: SemanticPoint) -> Value {
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

fn target_evaluation_rect(evaluation: &TargetEvaluation) -> CliOutcome<PackRect> {
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

fn run_monitor(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let _ = global;
    runtime_session_adapter::retired_authority("monitor", args)
}

fn stream_check_requested(flags: &FlagArgs) -> bool {
    flags.positionals.first().map(String::as_str) == Some("check")
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

fn parse_record_build_resolution(flags: &FlagArgs) -> CliOutcome<Option<(u32, u32)>> {
    let Some(value) = flags
        .optional("--resolution")
        .filter(|value| value != "true")
    else {
        return Ok(None);
    };
    let normalized = value.replace(['X', '*'], "x");
    let Some((width, height)) = normalized.split_once('x') else {
        return Err(CliError::usage(format!(
            "--resolution must use <width>x<height>, got {value}"
        )));
    };
    let width = width.trim().parse::<u32>().map_err(|err| {
        CliError::usage(format!(
            "failed to parse --resolution width '{width}': {err}"
        ))
    })?;
    let height = height.trim().parse::<u32>().map_err(|err| {
        CliError::usage(format!(
            "failed to parse --resolution height '{height}': {err}"
        ))
    })?;
    if width == 0 || height == 0 {
        return Err(CliError::usage(
            "--resolution width and height must be non-zero",
        ));
    }
    Ok(Some((width, height)))
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

fn required_non_empty_flag(flags: &FlagArgs, name: &str) -> CliOutcome<String> {
    let value = flags.required(name)?;
    if value.trim().is_empty() {
        return Err(CliError::usage(format!("{name} must not be empty")));
    }
    Ok(value)
}

fn parse_session_record_region(value: &str) -> CliOutcome<SessionRecordRegion> {
    if value == "auto" {
        return Ok(SessionRecordRegion::Auto);
    }
    let parts = value.split(',').map(str::trim).collect::<Vec<_>>();
    if parts.len() != 4 {
        return Err(CliError::usage(format!(
            "record anchor region must be auto or x,y,width,height: {value}"
        )));
    }
    let parse_part = |index: usize, name: &str| {
        parts[index].parse::<i32>().map_err(|err| {
            CliError::usage(format!(
                "failed to parse record anchor region {name} '{}': {err}",
                parts[index]
            ))
        })
    };
    let rect = SessionRecordRect {
        x: parse_part(0, "x")?,
        y: parse_part(1, "y")?,
        width: parse_part(2, "width")?,
        height: parse_part(3, "height")?,
    };
    if rect.width <= 0 || rect.height <= 0 {
        return Err(CliError::usage(
            "record anchor region width and height must be positive",
        ));
    }
    Ok(SessionRecordRegion::Rect { rect })
}

fn parse_optional_unit_f64(flags: &FlagArgs, name: &str) -> CliOutcome<Option<f64>> {
    let Some(value) = flags.optional(name) else {
        return Ok(None);
    };
    if value == "true" {
        return Err(CliError::usage(format!("missing {name} <value>")));
    };
    let parsed = value
        .parse::<f64>()
        .map_err(|err| CliError::usage(format!("failed to parse {name} '{value}': {err}")))?;
    if !parsed.is_finite() || !(0.0..=1.0).contains(&parsed) {
        return Err(CliError::usage(format!(
            "{name} must be a finite number between 0 and 1"
        )));
    }
    Ok(Some(parsed))
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

fn parse_session_record_swipe_rects(
    value: &str,
) -> CliOutcome<(SessionRecordRect, SessionRecordRect)> {
    let (from, to) = value
        .split_once("->")
        .ok_or_else(|| CliError::usage("--swipe must be formatted as x,y,w,h->x,y,w,h"))?;
    Ok((
        parse_session_record_rect(from, "--swipe from")?,
        parse_session_record_rect(to, "--swipe to")?,
    ))
}

fn parse_session_record_rect(value: &str, label: &str) -> CliOutcome<SessionRecordRect> {
    let parts = value.split(',').map(str::trim).collect::<Vec<_>>();
    if parts.len() != 4 {
        return Err(CliError::usage(format!(
            "{label} must be formatted as x,y,width,height: {value}"
        )));
    }
    let parse = |index: usize, name: &str| {
        parts[index].parse::<i32>().map_err(|err| {
            CliError::usage(format!(
                "failed to parse {label} {name} '{}': {err}",
                parts[index]
            ))
        })
    };
    let rect = SessionRecordRect {
        x: parse(0, "x")?,
        y: parse(1, "y")?,
        width: parse(2, "width")?,
        height: parse(3, "height")?,
    };
    if rect.width <= 0 || rect.height <= 0 {
        return Err(CliError::usage(format!(
            "{label} dimensions must be positive: {}x{}",
            rect.width, rect.height
        )));
    }
    Ok(rect)
}

fn parse_record_duration_ms(flags: &FlagArgs, default_ms: u64) -> CliOutcome<u64> {
    let duration_ms = flags
        .optional("--duration-ms")
        .filter(|value| value != "true")
        .map(|value| {
            value.parse::<u64>().map_err(|err| {
                CliError::usage(format!("failed to parse --duration-ms '{value}': {err}"))
            })
        })
        .transpose()?
        .unwrap_or(default_ms);
    if duration_ms == 0 {
        return Err(CliError::usage("--duration-ms must be positive"));
    }
    Ok(duration_ms)
}

fn session_record_drift_diagnostics_path(flags: &FlagArgs) -> CliOutcome<Option<PathBuf>> {
    let Some(value) = flags.optional("--from-drift-diagnostics") else {
        return Ok(None);
    };
    if value == "true" {
        return Err(CliError::usage(
            "session record amend --from-drift-diagnostics requires <path>",
        ));
    }
    Ok(Some(PathBuf::from(value)))
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

fn record_amend_step_id(flags: &FlagArgs) -> CliOutcome<String> {
    let value = flags
        .optional("--step-id")
        .filter(|value| value != "true")
        .or_else(|| flags.positionals.first().cloned())
        .ok_or_else(|| CliError::usage("session record amend requires <step-id> or --step-id"))?;
    if value.trim().is_empty() {
        return Err(CliError::usage("record amend step id must not be empty"));
    }
    Ok(value)
}

fn record_candidates_step_id(flags: &FlagArgs) -> CliOutcome<String> {
    let value = flags
        .optional("--step-id")
        .filter(|value| value != "true")
        .or_else(|| flags.positionals.first().cloned())
        .ok_or_else(|| {
            CliError::usage("session record candidates requires <step-id> or --step-id")
        })?;
    if value.trim().is_empty() {
        return Err(CliError::usage(
            "record candidates step id must not be empty",
        ));
    }
    Ok(value)
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

fn parse_session_record_candidate_index(flags: &FlagArgs) -> CliOutcome<Option<usize>> {
    let candidate_index = flags.optional("--candidate-index");
    let auto_candidate = flags.optional("--auto-candidate");
    if candidate_index.is_some() && auto_candidate.is_some() {
        return Err(CliError::usage(
            "record amend accepts only one of --candidate-index or --auto-candidate",
        ));
    }
    let Some(value) = candidate_index.or(auto_candidate) else {
        return Ok(None);
    };
    if value == "true" {
        return Err(CliError::usage(
            "record amend candidate selection requires an index value",
        ));
    }
    value.parse::<usize>().map(Some).map_err(|err| {
        CliError::usage(format!(
            "failed to parse record amend candidate index '{value}': {err}"
        ))
    })
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
        "import-upstream" | "drift-upstream" => {
            let upstream_root = flags.required_path("--upstream-root")?;
            Ok(json!({
                "repo": repo.display().to_string(),
                "resource_root": resource_root.root.display().to_string(),
                "resource_layout": resource_root.layout,
                "upstream_root": upstream_root.display().to_string(),
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

fn run_run_report(sub: &str, global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let run_root = effective_run_root(global, &read_user_config()?)
        .unwrap_or_else(|| PathBuf::from("target").join("actinglab-runs"));
    match sub {
        "list" => list_runs(&run_root),
        "show" | "open" | "summary" | "export" => {
            let run_id = args
                .iter()
                .find(|arg| !arg.starts_with("--"))
                .ok_or_else(|| CliError::usage(format!("run {sub} requires <run-id>")))?;
            if sub == "export" {
                let out = flags.required_path("--out")?;
                create_error_report_zip(&out, run_id, "run export placeholder")?;
                return Ok(json!({
                    "run_id": run_id,
                    "out": out.display().to_string()
                }));
            }
            Ok(json!({
                "run_id": run_id,
                "run_root": run_root.display().to_string(),
                "status": "reserved"
            }))
        }
        _ => Err(CliError::usage(format!("unknown run command: {sub}"))),
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

#[derive(Debug, Clone, Default)]
struct FlagArgs {
    flags: BTreeMap<String, Vec<String>>,
    positionals: Vec<String>,
}

impl FlagArgs {
    fn parse(args: &[String]) -> CliOutcome<Self> {
        let mut parsed = Self::default();
        let mut index = 0usize;
        while index < args.len() {
            let arg = &args[index];
            if arg.starts_with("--") {
                if index + 1 < args.len() && !args[index + 1].starts_with("--") {
                    parsed
                        .flags
                        .entry(arg.clone())
                        .or_default()
                        .push(args[index + 1].clone());
                    index += 2;
                } else {
                    parsed
                        .flags
                        .entry(arg.clone())
                        .or_default()
                        .push("true".to_string());
                    index += 1;
                }
            } else {
                parsed.positionals.push(arg.clone());
                index += 1;
            }
        }
        Ok(parsed)
    }

    fn bool(&self, name: &str) -> bool {
        self.flags
            .get(name)
            .and_then(|values| values.last())
            .is_some_and(|value| value == "true")
    }

    fn optional(&self, name: &str) -> Option<String> {
        self.flags
            .get(name)
            .and_then(|values| values.last())
            .cloned()
    }

    fn values(&self, name: &str) -> Vec<String> {
        self.flags.get(name).cloned().unwrap_or_default()
    }

    fn without_first_positional(&self) -> Self {
        let mut next = self.clone();
        if !next.positionals.is_empty() {
            next.positionals.remove(0);
        }
        next
    }

    fn required(&self, name: &str) -> CliOutcome<String> {
        self.optional(name)
            .filter(|value| value != "true")
            .ok_or_else(|| CliError::usage(format!("missing {name} <value>")))
    }

    fn optional_path(&self, name: &str) -> Option<PathBuf> {
        self.optional(name)
            .filter(|value| value != "true")
            .map(PathBuf::from)
    }

    fn required_path(&self, name: &str) -> CliOutcome<PathBuf> {
        self.required(name).map(PathBuf::from)
    }

    fn reject_flags(&self, command: &str) -> CliOutcome<()> {
        if self.flags.is_empty() {
            return Ok(());
        }
        let names = self.flags.keys().cloned().collect::<Vec<_>>();
        Err(CliError::usage(format!(
            "{command} takes positional arguments only; unexpected flags: {}",
            names.join(", ")
        )))
    }

    fn expect_positionals(&self, command: &str, expected: usize) -> CliOutcome<()> {
        if self.positionals.len() == expected {
            return Ok(());
        }
        Err(CliError::usage(format!(
            "{command} expects {expected} positional argument(s), got {}",
            self.positionals.len()
        )))
    }

    fn required_positional(&self, index: usize, name: &str) -> CliOutcome<&str> {
        self.positionals
            .get(index)
            .map(String::as_str)
            .ok_or_else(|| CliError::usage(format!("missing {name}")))
    }

    fn required_i32(&self, index: usize, name: &str) -> CliOutcome<i32> {
        let value = self.required_positional(index, name)?;
        value
            .parse::<i32>()
            .map_err(|err| CliError::usage(format!("failed to parse {name} '{value}': {err}")))
    }

    fn required_u64(&self, index: usize, name: &str) -> CliOutcome<u64> {
        let value = self.required_positional(index, name)?;
        value
            .parse::<u64>()
            .map_err(|err| CliError::usage(format!("failed to parse {name} '{value}': {err}")))
    }
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

#[derive(Debug, Clone)]
struct RuntimeEndpointPolicy {
    scheme: String,
    host: String,
    port: u16,
    channel: RuntimeEndpointChannel,
    auth_material: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeEndpointChannel {
    LocalDirect,
    TrustedRemote,
}

impl RuntimeEndpointChannel {
    fn as_str(self) -> &'static str {
        match self {
            RuntimeEndpointChannel::LocalDirect => "local_direct",
            RuntimeEndpointChannel::TrustedRemote => "trusted_remote",
        }
    }
}

fn runtime_endpoint_check(endpoint: &str) -> Value {
    match runtime_endpoint_policy(endpoint) {
        Ok(policy) => {
            let reachable = runtime_tcp_available(endpoint);
            json!({
                "ok": reachable,
                "endpoint": endpoint,
                "reachable": reachable,
                "policy": runtime_endpoint_policy_json(&policy)
            })
        }
        Err(err) => json!({
            "ok": false,
            "endpoint": endpoint,
            "error_code": err.code,
            "error": err.message,
            "blocked_by": err.blocked_by
        }),
    }
}

fn runtime_endpoint_policy(endpoint: &str) -> CliOutcome<RuntimeEndpointPolicy> {
    let (scheme, host, port) = parse_endpoint_parts(endpoint).ok_or_else(|| {
        CliError::runtime_not_running(format!(
            "runtime endpoint is invalid; expected host:port, http://host:port, or https://host:port, got {endpoint}"
        ))
    })?;
    if is_loopback_host(&host) {
        return Ok(RuntimeEndpointPolicy {
            scheme,
            host,
            port,
            channel: RuntimeEndpointChannel::LocalDirect,
            auth_material: None,
        });
    }
    if scheme != "https" {
        return Err(CliError::safety_blocked(
            "trusted_remote_transport_blocked",
            "trusted remote runtime endpoints must use https:// with encryption",
            &["trusted_remote", "encryption"],
        ));
    }
    let auth_material = trusted_remote_auth_material().ok_or_else(|| {
        CliError::safety_blocked(
            "trusted_remote_auth_required",
            format!(
                "trusted remote runtime endpoints require {TRUSTED_REMOTE_TOKEN_ENV} or {TRUSTED_REMOTE_CLIENT_CERT_ENV}"
            ),
            &["trusted_remote", "authentication"],
        )
    })?;
    Ok(RuntimeEndpointPolicy {
        scheme,
        host,
        port,
        channel: RuntimeEndpointChannel::TrustedRemote,
        auth_material: Some(auth_material),
    })
}

fn runtime_endpoint_policy_json(policy: &RuntimeEndpointPolicy) -> Value {
    json!({
        "channel": policy.channel.as_str(),
        "scheme": policy.scheme,
        "host": policy.host,
        "port": policy.port,
        "encryption_required": policy.channel == RuntimeEndpointChannel::TrustedRemote,
        "authentication_required": policy.channel == RuntimeEndpointChannel::TrustedRemote,
        "auth_material": policy.auth_material,
        "auth_env": {
            "token": TRUSTED_REMOTE_TOKEN_ENV,
            "client_certificate": TRUSTED_REMOTE_CLIENT_CERT_ENV
        }
    })
}

fn trusted_remote_auth_material() -> Option<&'static str> {
    if env_var_non_empty(TRUSTED_REMOTE_TOKEN_ENV) {
        Some("token")
    } else if env_var_non_empty(TRUSTED_REMOTE_CLIENT_CERT_ENV) {
        Some("client_certificate")
    } else {
        None
    }
}

fn env_var_non_empty(name: &str) -> bool {
    env::var(name)
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

fn runtime_tcp_available(endpoint: &str) -> bool {
    let Some((host, port)) = parse_endpoint_host_port(endpoint) else {
        return false;
    };
    let Ok(mut addrs) = (host.as_str(), port).to_socket_addrs() else {
        return false;
    };
    addrs.any(|addr| TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok())
}

fn parse_endpoint_host_port(endpoint: &str) -> Option<(String, u16)> {
    parse_endpoint_parts(endpoint).map(|(_scheme, host, port)| (host, port))
}

fn parse_endpoint_parts(endpoint: &str) -> Option<(String, String, u16)> {
    let (scheme, trimmed) = if let Some(rest) = endpoint.strip_prefix("http://") {
        ("http", rest)
    } else if let Some(rest) = endpoint.strip_prefix("https://") {
        ("https", rest)
    } else {
        ("tcp", endpoint)
    };
    let host_port = trimmed.split('/').next()?;
    let (host, port) = host_port.rsplit_once(':')?;
    Some((
        scheme.to_string(),
        host.trim_matches(['[', ']']).to_string(),
        port.parse().ok()?,
    ))
}

fn is_loopback_host(host: &str) -> bool {
    let normalized = host.trim_matches(['[', ']']).to_ascii_lowercase();
    normalized == "localhost"
        || normalized == "::1"
        || normalized == "0:0:0:0:0:0:0:1"
        || normalized.starts_with("127.")
}

fn device_config(global: &GlobalOptions, config: &UserConfig) -> CliOutcome<DeviceRuntimeConfig> {
    device_config_for_instance(global, config, None)
}

fn device_config_for_instance(
    global: &GlobalOptions,
    config: &UserConfig,
    instance_override: Option<&str>,
) -> CliOutcome<DeviceRuntimeConfig> {
    let instance_id = match instance_override {
        Some(instance) => instance.to_string(),
        None => resolve_instance_id(global, config)?,
    };
    let instance = config.instances.get(&instance_id);
    #[cfg(test)]
    let mut target = DeviceTarget::default();
    #[cfg(test)]
    if let Some(serial) = instance.and_then(|instance| instance.serial.clone()) {
        target.serial = Some(serial);
    } else if global.instance.as_deref() == Some(instance_id.as_str()) && instance.is_none() {
        target.serial = Some(instance_id.clone());
    }
    let capture_backend = effective_capture_backend_choice(global, &instance_id, instance)?;
    #[cfg(test)]
    let touch_backend = effective_touch_backend_choice(global, &instance_id, instance)?;
    let resolved_adb = effective_adb_path_for_instance(config, instance)?;
    enforce_path_adb_target_boundary(&resolved_adb, instance, capture_backend)?;
    Ok(DeviceRuntimeConfig {
        instance_alias: instance_id,
        runtime_state_root: runtime_state_root()?,
        #[cfg(test)]
        target,
        adb_source: resolved_adb.source,
        adb_warning: resolved_adb.warning,
        capture_backend,
        #[cfg(test)]
        touch_backend,
    })
}

#[derive(Debug)]
struct DeviceRuntimeConfig {
    instance_alias: String,
    runtime_state_root: PathBuf,
    #[cfg(test)]
    target: DeviceTarget,
    adb_source: AdbPathSource,
    adb_warning: Option<String>,
    capture_backend: CaptureBackendChoice,
    #[cfg(test)]
    touch_backend: TouchBackendChoice,
}

impl DeviceRuntimeConfig {
    fn runtime_capture_endpoint(&self) -> runtime_capture_backend::RuntimeCaptureEndpoint {
        runtime_capture_backend::RuntimeCaptureEndpoint::new(
            self.instance_alias.clone(),
            self.runtime_state_root.clone(),
        )
    }
}

fn effective_capture_backend_choice(
    global: &GlobalOptions,
    instance_id: &str,
    instance: Option<&InstanceConfig>,
) -> CliOutcome<CaptureBackendChoice> {
    if let Some(choice) = global.capture_backend {
        return Ok(choice);
    }
    let Some(value) = instance.and_then(|instance| instance.capture_backend.as_deref()) else {
        return Ok(CaptureBackendChoice::Auto);
    };
    CaptureBackendChoice::parse(value).map_err(|err| {
        CliError::usage(format!(
            "invalid instance.{instance_id}.capture_backend '{value}': {err}"
        ))
    })
}

#[cfg(test)]
fn effective_touch_backend_choice(
    global: &GlobalOptions,
    instance_id: &str,
    instance: Option<&InstanceConfig>,
) -> CliOutcome<TouchBackendChoice> {
    if let Some(choice) = global.touch_backend {
        return Ok(choice);
    }
    let Some(value) = instance.and_then(|instance| instance.touch_backend.as_deref()) else {
        return Ok(TouchBackendChoice::Auto);
    };
    TouchBackendChoice::parse(value).map_err(|err| {
        CliError::usage(format!(
            "invalid instance.{instance_id}.touch_backend '{value}': {err}"
        ))
    })
}

fn resolve_instance_id(global: &GlobalOptions, config: &UserConfig) -> CliOutcome<String> {
    if let Some(instance) = &global.instance {
        return Ok(instance.clone());
    }
    if let Some((id, _instance)) = config.instances.iter().find(|(_id, instance)| {
        let game_match = global
            .game
            .as_ref()
            .is_none_or(|game| instance.game.as_ref() == Some(game));
        let server_match = global
            .server
            .as_ref()
            .is_none_or(|server| instance.server.as_ref() == Some(server));
        game_match && server_match
    }) {
        return Ok(id.clone());
    }
    Err(CliError::instance(
        "could not resolve instance; pass --instance or configure instance.<id>.game/server",
    ))
}

fn resolve_instance_id_for_flags(
    global: &GlobalOptions,
    config: &UserConfig,
    flags: &FlagArgs,
) -> CliOutcome<String> {
    if let Some(instance) = flags.optional("--instance").filter(|value| value != "true") {
        return Ok(instance);
    }
    resolve_instance_id(global, config)
}

fn read_user_config() -> CliOutcome<UserConfig> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(UserConfig::default());
    }
    let text = fs::read_to_string(&path).map_err(|err| {
        CliError::usage(format!(
            "failed to read config file {}: {err}",
            path.display()
        ))
    })?;
    serde_json::from_str(&text).map_err(|err| {
        CliError::usage(format!(
            "failed to parse config file {}: {err}",
            path.display()
        ))
    })
}

fn write_user_config(config: &UserConfig) -> CliOutcome<()> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            CliError::usage(format!(
                "failed to create config directory {}: {err}",
                parent.display()
            ))
        })?;
    }
    let text = serde_json::to_string_pretty(config)
        .map_err(|err| CliError::usage(format!("failed to serialize config: {err}")))?;
    fs::write(&path, text)
        .map_err(|err| CliError::usage(format!("failed to write {}: {err}", path.display())))
}

fn config_path() -> CliOutcome<PathBuf> {
    if let Ok(path) = env::var(CONFIG_ENV) {
        return Ok(PathBuf::from(path));
    }
    Ok(app_state_root()?.join("config.json"))
}

fn absolute_lexical_path(path: &Path) -> CliOutcome<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .map_err(|err| CliError::usage(format!("failed to resolve current dir: {err}")))?
            .join(path)
    };
    Ok(normalize_path_lexically(&absolute))
}

fn normalize_path_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

fn path_has_root_or_prefix(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component, Component::Prefix(_) | Component::RootDir))
}

fn path_starts_with_case_aware(path: &Path, base: &Path) -> bool {
    #[cfg(windows)]
    {
        let path_components = windows_normalized_path_components(path);
        let base_components = windows_normalized_path_components(base);
        path_components.len() >= base_components.len()
            && path_components
                .iter()
                .zip(base_components.iter())
                .all(|(left, right)| left == right)
    }
    #[cfg(not(windows))]
    {
        path.starts_with(base)
    }
}

#[cfg(windows)]
fn windows_normalized_path_components(path: &Path) -> Vec<String> {
    let raw = path.to_string_lossy();
    let normalized = if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = raw.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        raw.to_string()
    };
    Path::new(&normalized)
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect()
}

fn canonicalize_required_base(
    base: &Path,
    reason: &str,
    blocked_by: &[&'static str],
) -> CliOutcome<PathBuf> {
    base.canonicalize().map(windows_long_path).map_err(|err| {
        CliError::safety_blocked(
            "path_escape",
            format!(
                "{reason}: allowed base {} cannot be canonicalized: {err}",
                base.display()
            ),
            blocked_by,
        )
    })
}

fn canonicalize_with_existing_parent(
    path: &Path,
    reason: &str,
    blocked_by: &[&'static str],
) -> CliOutcome<PathBuf> {
    let mut existing = path.to_path_buf();
    let mut missing = Vec::<OsString>::new();
    while !existing.exists() {
        let Some(name) = existing.file_name().map(OsString::from) else {
            return Err(CliError::safety_blocked(
                "path_escape",
                format!(
                    "{reason}: path {} has no existing parent inside the allowed base",
                    path.display()
                ),
                blocked_by,
            ));
        };
        missing.push(name);
        if !existing.pop() {
            return Err(CliError::safety_blocked(
                "path_escape",
                format!(
                    "{reason}: path {} has no existing parent inside the allowed base",
                    path.display()
                ),
                blocked_by,
            ));
        }
    }
    let mut resolved = existing
        .canonicalize()
        .map(windows_long_path)
        .map_err(|err| {
            CliError::safety_blocked(
                "path_escape",
                format!(
                    "{reason}: existing parent {} cannot be canonicalized: {err}",
                    existing.display()
                ),
                blocked_by,
            )
        })?;
    for component in missing.iter().rev() {
        resolved.push(component);
    }
    Ok(normalize_path_lexically(&resolved))
}

#[cfg(windows)]
fn windows_long_path(path: PathBuf) -> PathBuf {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    type Handle = *mut std::ffi::c_void;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CloseHandle(hObject: Handle) -> i32;
        fn CreateFileW(
            lpFileName: *const u16,
            dwDesiredAccess: u32,
            dwShareMode: u32,
            lpSecurityAttributes: *mut std::ffi::c_void,
            dwCreationDisposition: u32,
            dwFlagsAndAttributes: u32,
            hTemplateFile: Handle,
        ) -> Handle;
        fn GetFinalPathNameByHandleW(
            hFile: Handle,
            lpszFilePath: *mut u16,
            cchFilePath: u32,
            dwFlags: u32,
        ) -> u32;
        fn GetLongPathNameW(
            lpszShortPath: *const u16,
            lpszLongPath: *mut u16,
            cchBuffer: u32,
        ) -> u32;
    }

    let input = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    const FILE_READ_ATTRIBUTES: u32 = 0x80;
    const FILE_SHARE_READ: u32 = 0x01;
    const FILE_SHARE_WRITE: u32 = 0x02;
    const FILE_SHARE_DELETE: u32 = 0x04;
    const OPEN_EXISTING: u32 = 3;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const INVALID_HANDLE_VALUE: Handle = !0usize as Handle;

    // SAFETY: `input` is null-terminated; the handle is closed before this function returns.
    let handle = unsafe {
        CreateFileW(
            input.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    if handle != INVALID_HANDLE_VALUE {
        let mut final_path = vec![0u16; 32_768];
        // SAFETY: `final_path` is a valid writable buffer and `handle` is a live file handle.
        let written = unsafe {
            GetFinalPathNameByHandleW(handle, final_path.as_mut_ptr(), final_path.len() as u32, 0)
        };
        // SAFETY: `handle` was returned by `CreateFileW` above.
        let _ = unsafe { CloseHandle(handle) };
        if written > 0 && (written as usize) < final_path.len() {
            return OsString::from_wide(&final_path[..written as usize]).into();
        }
    }

    // Windows CI can expose temp paths with 8.3 short components; expand them
    // before safety prefix checks so canonicalization does not create a false escape.
    // SAFETY: `input` is null-terminated and the first call only queries the required buffer size.
    let required = unsafe { GetLongPathNameW(input.as_ptr(), std::ptr::null_mut(), 0) };
    if required == 0 {
        return path;
    }
    let mut buffer = vec![0u16; required as usize];
    // SAFETY: `buffer` has the size reported by Windows and remains valid for the call.
    let written = unsafe { GetLongPathNameW(input.as_ptr(), buffer.as_mut_ptr(), required) };
    if written == 0 || written >= required {
        return path;
    }
    OsString::from_wide(&buffer[..written as usize]).into()
}

#[cfg(not(windows))]
fn windows_long_path(path: PathBuf) -> PathBuf {
    path
}

fn ensure_path_within(
    base: &Path,
    candidate: &Path,
    reason: &str,
    blocked_by: &[&'static str],
) -> CliOutcome<PathBuf> {
    if !candidate.is_absolute() && path_has_root_or_prefix(candidate) {
        return Err(CliError::safety_blocked(
            "path_escape",
            format!(
                "{reason}: path {} uses a root or drive prefix outside the allowed base",
                candidate.display()
            ),
            blocked_by,
        ));
    }
    let lexical_base = absolute_lexical_path(base)?;
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        lexical_base.join(candidate)
    };
    let lexical_resolved = normalize_path_lexically(&joined);
    if !path_starts_with_case_aware(&lexical_resolved, &lexical_base) {
        return Err(CliError::safety_blocked(
            "path_escape",
            format!(
                "{reason}: path {} escapes allowed base {}",
                lexical_resolved.display(),
                lexical_base.display()
            ),
            blocked_by,
        ));
    }
    let canonical_base = canonicalize_required_base(&lexical_base, reason, blocked_by)?;
    let resolved = canonicalize_with_existing_parent(&lexical_resolved, reason, blocked_by)?;
    if !path_starts_with_case_aware(&resolved, &canonical_base) {
        return Err(CliError::safety_blocked(
            "path_escape",
            format!(
                "{reason}: path {} escapes allowed base {} after canonicalization",
                resolved.display(),
                canonical_base.display()
            ),
            blocked_by,
        ));
    }
    Ok(resolved)
}

fn app_state_root() -> CliOutcome<PathBuf> {
    let root = env::var("LOCALAPPDATA")
        .or_else(|_| env::var("APPDATA"))
        .map_err(|_| CliError::usage("LOCALAPPDATA or APPDATA is required for ActingLab state"))?;
    Ok(PathBuf::from(root).join("ActingCommand").join("actinglab"))
}

fn runtime_state_root() -> CliOutcome<PathBuf> {
    if let Ok(path) = env::var(RUNTIME_STATE_ROOT_ENV) {
        if path.trim().is_empty() {
            return Err(CliError::usage(format!(
                "{RUNTIME_STATE_ROOT_ENV} must not be empty"
            )));
        }
        return Ok(PathBuf::from(path));
    }
    let root = env::var("LOCALAPPDATA")
        .or_else(|_| env::var("APPDATA"))
        .map_err(|_| CliError::usage("LOCALAPPDATA or APPDATA is required for Runtime state"))?;
    Ok(PathBuf::from(root).join("ActingCommand").join("runtime"))
}

fn session_state_dir_from_flags(flags: &FlagArgs) -> CliOutcome<PathBuf> {
    if let Some(path) = flags.optional_path("--state-dir") {
        return Ok(path);
    }
    if let Ok(path) = env::var(SESSION_STATE_ENV) {
        return Ok(PathBuf::from(path));
    }
    Ok(app_state_root()?.join("session"))
}

fn current_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn read_json_file<T>(path: &Path) -> CliOutcome<Option<T>>
where
    T: for<'de> Deserialize<'de>,
{
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(path)
        .map_err(|err| CliError::usage(format!("failed to read {}: {err}", path.display())))?;
    let value = serde_json::from_str(&text)
        .map_err(|err| CliError::usage(format!("failed to parse {}: {err}", path.display())))?;
    Ok(Some(value))
}

#[cfg(test)]
fn write_json_file<T>(path: &Path, value: &T) -> CliOutcome<()>
where
    T: Serialize,
{
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            CliError::usage(format!("failed to create {}: {err}", parent.display()))
        })?;
    }
    let text = serde_json::to_string_pretty(value)
        .map_err(|err| CliError::usage(format!("failed to serialize JSON: {err}")))?;
    fs::write(path, text)
        .map_err(|err| CliError::usage(format!("failed to write {}: {err}", path.display())))
}

fn write_json_file_atomic<T>(path: &Path, value: &T) -> CliOutcome<()>
where
    T: Serialize,
{
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            CliError::usage(format!("failed to create {}: {err}", parent.display()))
        })?;
    }
    let text = serde_json::to_string_pretty(value)
        .map_err(|err| CliError::usage(format!("failed to serialize JSON: {err}")))?;
    cleanup_current_process_json_tmp_files(path)?;
    let seq = JSON_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = path.with_extension(format!("tmp-{}-{seq}", std::process::id()));
    let mut file = File::create(&tmp)
        .map_err(|err| CliError::usage(format!("failed to create {}: {err}", tmp.display())))?;
    file.write_all(text.as_bytes())
        .map_err(|err| CliError::usage(format!("failed to write {}: {err}", tmp.display())))?;
    file.sync_all()
        .map_err(|err| CliError::usage(format!("failed to sync {}: {err}", tmp.display())))?;
    drop(file);
    fs::rename(&tmp, path).map_err(|err| {
        let _ = fs::remove_file(&tmp);
        CliError::usage(format!(
            "failed to publish {} from {}: {err}",
            path.display(),
            tmp.display()
        ))
    })
}

fn cleanup_current_process_json_tmp_files(path: &Path) -> CliOutcome<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    if !parent.exists() {
        return Ok(());
    }
    let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
        return Ok(());
    };
    let prefix = format!("{stem}.tmp-{}-", std::process::id());
    let entries = fs::read_dir(parent)
        .map_err(|err| CliError::usage(format!("failed to read {}: {err}", parent.display())))?;
    for entry in entries {
        let entry = entry.map_err(|err| {
            CliError::usage(format!("failed to inspect {}: {err}", parent.display()))
        })?;
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if file_name.starts_with(&prefix) {
            fs::remove_file(entry.path()).map_err(|err| {
                CliError::usage(format!(
                    "failed to remove stale temp file {}: {err}",
                    entry.path().display()
                ))
            })?;
        }
    }
    Ok(())
}

fn safe_file_stem(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn config_get(config: &UserConfig, key: &str) -> CliOutcome<Value> {
    match key {
        "adb_path" => Ok(json!(config.adb_path)),
        "runtime_endpoint" => Ok(json!(config.runtime_endpoint)),
        "run_root" => Ok(json!(config.run_root)),
        "resource_root" => Ok(json!(config.resource_root)),
        key if key.starts_with("instance.") => get_instance_value(config, key),
        _ => Err(CliError::usage(format!("unknown config key: {key}"))),
    }
}

fn config_set(config: &mut UserConfig, key: &str, value: &str) -> CliOutcome<()> {
    match key {
        "adb_path" => config.adb_path = Some(value.to_string()),
        "runtime_endpoint" => config.runtime_endpoint = Some(value.to_string()),
        "run_root" => config.run_root = Some(value.to_string()),
        "resource_root" => config.resource_root = Some(value.to_string()),
        key if key.starts_with("instance.") => set_instance_value(config, key, value)?,
        _ => return Err(CliError::usage(format!("unknown config key: {key}"))),
    }
    Ok(())
}

fn get_instance_value(config: &UserConfig, key: &str) -> CliOutcome<Value> {
    let parts = key.split('.').collect::<Vec<_>>();
    if parts.len() != 3 {
        return Err(CliError::usage(
            "instance config keys use instance.<id>.serial|game|server|package|adb_path|capture_backend|touch_backend",
        ));
    }
    let instance = config.instances.get(parts[1]);
    let value = match parts[2] {
        "serial" => instance.and_then(|instance| instance.serial.clone()),
        "game" => instance.and_then(|instance| instance.game.clone()),
        "server" => instance.and_then(|instance| instance.server.clone()),
        "package" => instance.and_then(|instance| instance.package.clone()),
        "adb_path" => instance.and_then(|instance| instance.adb_path.clone()),
        "capture_backend" => instance.and_then(|instance| instance.capture_backend.clone()),
        "touch_backend" => instance.and_then(|instance| instance.touch_backend.clone()),
        other => return Err(CliError::usage(format!("unknown instance field: {other}"))),
    };
    Ok(json!(value))
}

fn set_instance_value(config: &mut UserConfig, key: &str, value: &str) -> CliOutcome<()> {
    let parts = key.split('.').collect::<Vec<_>>();
    if parts.len() != 3 {
        return Err(CliError::usage(
            "instance config keys use instance.<id>.serial|game|server|package|adb_path|capture_backend|touch_backend",
        ));
    }
    let instance = config.instances.entry(parts[1].to_string()).or_default();
    match parts[2] {
        "serial" => instance.serial = Some(value.to_string()),
        "game" => instance.game = Some(value.to_string()),
        "server" => instance.server = Some(value.to_string()),
        "package" => instance.package = Some(value.to_string()),
        "adb_path" => instance.adb_path = Some(value.to_string()),
        "capture_backend" => {
            CaptureBackendChoice::parse(value).map_err(|err| CliError::usage(err.to_string()))?;
            instance.capture_backend = Some(value.to_string());
        }
        "touch_backend" => {
            TouchBackendChoice::parse(value).map_err(|err| CliError::usage(err.to_string()))?;
            instance.touch_backend = Some(value.to_string());
        }
        other => return Err(CliError::usage(format!("unknown instance field: {other}"))),
    }
    Ok(())
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

fn zip_write_error(err: zip::result::ZipError) -> CliError {
    CliError::package_invalid(format!("zip write failed: {err}"))
}

fn zip_io_error(err: io::Error) -> CliError {
    CliError::package_invalid(format!("zip write failed: {err}"))
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

fn discover_recognition_packs(root: &Path) -> CliOutcome<Vec<Value>> {
    let packs = find_files(root, |path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".pack.json"))
    })?;
    let mut discovered = Vec::new();
    for pack in packs {
        let text = fs::read_to_string(&pack)
            .map_err(|err| CliError::usage(format!("failed to read {}: {err}", pack.display())))?;
        let value: Value = serde_json::from_str(&text)
            .map_err(|err| CliError::usage(format!("failed to parse {}: {err}", pack.display())))?;
        discovered.push(json!({
            "path": pack.display().to_string(),
            "game": value.get("game").and_then(Value::as_str),
            "server": value.get("server").and_then(Value::as_str),
            "locale": value.get("locale").and_then(Value::as_str),
            "match_metric": value
                .get("defaults")
                .and_then(|defaults| defaults.get("match_metric"))
                .and_then(Value::as_str)
        }));
    }
    Ok(discovered)
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

fn scene_from_frame(frame: &Frame) -> CliOutcome<Scene> {
    let pixel_format = match frame.pixel_format {
        PixelFormat::Rgb8 => ScenePixelFormat::Rgb8,
        PixelFormat::Rgba8 => ScenePixelFormat::Rgba8,
    };
    Scene::from_pixels(frame.width, frame.height, &frame.pixels, pixel_format)
        .map_err(|err| CliError::device(err.to_string()))
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

fn rect_json(rect: actingcommand_recognition_pack::PackRect) -> Value {
    json!({
        "x": rect.x,
        "y": rect.y,
        "width": rect.width,
        "height": rect.height
    })
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

fn command_capabilities() -> Vec<Value> {
    let mut commands = vec![
        command_cap("version", ["offline"], "available"),
        command_cap("doctor", ["offline"], "available"),
        command_cap("paths", ["offline"], "available"),
        command_cap("config get", ["offline"], "available"),
        command_cap("config set", ["offline"], "available"),
        command_cap("schema", ["offline"], "available"),
        command_cap("list", ["offline"], "available"),
        command_cap("capabilities", ["offline"], "available"),
        command_cap("resource validate", ["offline"], "available"),
        command_cap("resource convert", ["offline"], "available"),
        command_cap("resource import-upstream", ["offline"], "reserved"),
        command_cap("resource drift-upstream", ["offline"], "reserved"),
        command_cap("resource check-release", ["offline"], "available"),
        command_cap("observe", ["offline", "device_optional"], "available"),
        command_cap(
            "do",
            ["offline", "device_optional", "lab_lease"],
            "available",
        ),
        command_cap(
            "ensure",
            ["offline", "device_optional", "lab_lease"],
            "available",
        ),
        command_cap("wait", ["offline", "device_optional"], "available"),
        command_cap("package validate", ["offline"], "available"),
        command_cap("package inspect", ["offline"], "available"),
        command_cap("package build-task", ["offline"], "available"),
        command_cap("package build-pack", ["offline"], "available"),
        command_cap("ledger show", ["offline", "read_only"], "available"),
        command_cap("ledger events", ["offline", "read_only"], "available"),
        command_cap("ledger receipts", ["offline", "read_only"], "available"),
        command_cap("ledger diagnose", ["offline", "read_only"], "available"),
        command_cap("ledger evidence", ["offline", "read_only"], "available"),
        command_cap("operation validate", ["offline"], "available"),
        command_cap("operation inspect", ["offline"], "available"),
        command_cap("operation explain", ["offline"], "available"),
        command_cap("status", ["running_runtime"], "available"),
        command_cap("devices", ["device"], "available"),
        command_cap("touch-probe", ["device"], "available"),
        command_cap("tap", ["device"], "available"),
        command_cap("swipe", ["device"], "available"),
        command_cap("long-tap", ["device"], "available"),
        command_cap("key", ["device"], "available"),
        command_cap("text", ["device"], "available"),
        command_cap("session status", ["offline"], "available"),
        command_cap("session bootstrap", ["offline"], "available"),
        command_cap("session throat-policy", ["offline"], "available"),
        command_cap("session capture-policy", ["offline"], "available"),
        command_cap("session record-policy", ["offline"], "available"),
        command_cap("session self-heal-policy", ["offline"], "available"),
        command_cap("session self-heal-plan", ["offline"], "available"),
        command_cap("session phase-c-plan", ["offline"], "available"),
        command_cap("session readiness", ["offline"], "available"),
        command_cap("session connect-plan", ["offline"], "available"),
        command_cap("session stream-plan", ["offline"], "available"),
        command_cap("session queue", ["offline"], "available"),
        command_cap("session command-check", ["offline"], "available"),
        command_cap("session submit-plan", ["offline"], "available"),
        command_cap("session validation-plan", ["offline"], "available"),
        command_cap("session start", ["offline"], "available"),
        command_cap("session stop", ["offline"], "available"),
        command_cap("session cleanup", ["offline"], "available"),
        command_cap("session journal", ["offline"], "available"),
        command_cap("session events", ["offline"], "available"),
        command_cap("session events wait", ["offline"], "available"),
        command_cap("session response", ["offline"], "available"),
        command_cap("session response get", ["offline"], "available"),
        command_cap("session response wait", ["offline"], "available"),
        command_cap("session request-state", ["offline"], "available"),
        command_cap("session request-state get", ["offline"], "available"),
        command_cap("session request-state wait", ["offline"], "available"),
        command_cap("session request-state list", ["offline"], "available"),
        command_cap("session contract", ["offline"], "available"),
        command_cap("session api", ["offline"], "available"),
        command_cap("session transport", ["offline"], "available"),
        command_cap("session transport plan", ["offline"], "available"),
        command_cap("session transport check", ["offline"], "available"),
        command_cap("session stream", ["offline"], "available"),
        command_cap("session stream check", ["offline"], "available"),
        command_cap("session monitor-policy", ["offline"], "available"),
        command_cap("session request cancel", ["offline"], "available"),
        command_cap("session request status", ["running_runtime"], "available"),
        command_cap(
            "session request bootstrap",
            ["running_runtime"],
            "available",
        ),
        command_cap(
            "session request throat-policy",
            ["running_runtime"],
            "available",
        ),
        command_cap(
            "session request capture-policy",
            ["running_runtime"],
            "available",
        ),
        command_cap(
            "session request record-policy",
            ["running_runtime"],
            "available",
        ),
        command_cap(
            "session request self-heal-policy",
            ["running_runtime"],
            "available",
        ),
        command_cap(
            "session request self-heal-plan",
            ["running_runtime"],
            "available",
        ),
        command_cap(
            "session request phase-c-plan",
            ["running_runtime"],
            "available",
        ),
        command_cap(
            "session request readiness",
            ["running_runtime"],
            "available",
        ),
        command_cap(
            "session request connect-plan",
            ["running_runtime"],
            "available",
        ),
        command_cap(
            "session request stream-plan",
            ["running_runtime"],
            "available",
        ),
        command_cap("session request queue", ["running_runtime"], "available"),
        command_cap(
            "session request command-check",
            ["running_runtime"],
            "available",
        ),
        command_cap(
            "session request submit-plan",
            ["running_runtime"],
            "available",
        ),
        command_cap(
            "session request validation-plan",
            ["running_runtime"],
            "available",
        ),
        command_cap(
            "session request --no-wait",
            ["running_runtime"],
            "available",
        ),
        command_cap("session request journal", ["running_runtime"], "available"),
        command_cap("session request events", ["running_runtime"], "available"),
        command_cap(
            "session request events wait",
            ["running_runtime"],
            "available",
        ),
        command_cap("session request response", ["running_runtime"], "available"),
        command_cap(
            "session request response get",
            ["running_runtime"],
            "available",
        ),
        command_cap(
            "session request response wait",
            ["running_runtime"],
            "available",
        ),
        command_cap(
            "session request request-state",
            ["running_runtime"],
            "available",
        ),
        command_cap(
            "session request request-state get",
            ["running_runtime"],
            "available",
        ),
        command_cap(
            "session request request-state wait",
            ["running_runtime"],
            "available",
        ),
        command_cap(
            "session request request-state list",
            ["running_runtime"],
            "available",
        ),
        command_cap("session request contract", ["running_runtime"], "available"),
        command_cap("session request api", ["running_runtime"], "available"),
        command_cap(
            "session request transport",
            ["running_runtime"],
            "available",
        ),
        command_cap(
            "session request transport plan",
            ["running_runtime"],
            "available",
        ),
        command_cap(
            "session request transport check",
            ["running_runtime"],
            "available",
        ),
        command_cap(
            "session request capabilities",
            ["running_runtime"],
            "available",
        ),
        command_cap(
            "session request monitor-policy",
            ["running_runtime"],
            "available",
        ),
        command_cap("session request devices", ["running_runtime"], "available"),
        command_cap("session request record", ["running_runtime"], "available"),
        command_cap(
            "session request capture",
            ["running_runtime", "device"],
            "available",
        ),
        command_cap(
            "session request capture-diagnose",
            ["running_runtime", "device"],
            "available",
        ),
        command_cap(
            "session request stream",
            ["running_runtime", "device"],
            "available",
        ),
        command_cap(
            "session request stream check",
            ["running_runtime"],
            "available",
        ),
        command_cap(
            "session request recognize",
            ["running_runtime", "device"],
            "available",
        ),
        command_cap(
            "session request detect-page",
            ["running_runtime", "device"],
            "available",
        ),
        command_cap(
            "session request current-page",
            ["running_runtime", "device"],
            "available",
        ),
        command_cap(
            "session request is-visible",
            ["running_runtime", "device"],
            "available",
        ),
        command_cap(
            "session request locate",
            ["running_runtime", "device"],
            "available",
        ),
        command_cap(
            "session request monitor",
            ["running_runtime", "device"],
            "available",
        ),
        command_cap(
            "session request monitor-once",
            ["running_runtime", "device"],
            "available",
        ),
        command_cap(
            "session request instance list",
            ["running_runtime"],
            "available",
        ),
        command_cap(
            "session request instance registry",
            ["running_runtime"],
            "available",
        ),
        command_cap("session request instance health", ["offline"], "retired"),
        command_cap(
            "session request instance keep-alive",
            ["offline"],
            "retired",
        ),
        command_cap("session request instance connect", ["offline"], "retired"),
        command_cap("session request instance reconnect", ["offline"], "retired"),
        command_cap(
            "session request instance app",
            ["running_runtime", "device", "lab_lease"],
            "available",
        ),
        command_cap(
            "session request app",
            ["running_runtime", "device", "lab_lease"],
            "available",
        ),
        command_cap(
            "session request lab-run",
            ["running_runtime", "device", "lab_lease"],
            "available",
        ),
        command_cap(
            "session request package-run",
            ["running_runtime", "device", "lab_lease"],
            "available",
        ),
        command_cap(
            "session request operation-run",
            ["running_runtime", "device", "lab_lease"],
            "available",
        ),
        command_cap(
            "session request tap",
            ["running_runtime", "device", "lab_lease"],
            "available",
        ),
        command_cap(
            "session request swipe",
            ["running_runtime", "device", "lab_lease"],
            "available",
        ),
        command_cap(
            "session request long-tap",
            ["running_runtime", "device", "lab_lease"],
            "available",
        ),
        command_cap(
            "session request key",
            ["running_runtime", "device", "lab_lease"],
            "available",
        ),
        command_cap(
            "session request text",
            ["running_runtime", "device", "lab_lease"],
            "available",
        ),
        command_cap(
            "session request tap-target",
            ["running_runtime", "device", "lab_lease"],
            "available",
        ),
        command_cap(
            "session request navigate",
            ["running_runtime", "device", "lab_lease"],
            "available",
        ),
        command_cap(
            "session request recover",
            ["running_runtime", "device", "lab_lease"],
            "available",
        ),
        command_cap(
            "session request recover --stale-capture",
            ["running_runtime", "device"],
            "available",
        ),
        command_cap("session instance", ["offline", "device"], "available"),
        command_cap("session instance list", ["offline"], "available"),
        command_cap("session instance registry", ["offline"], "available"),
        command_cap("session instance health", ["offline"], "retired"),
        command_cap("session instance keep-alive", ["offline"], "retired"),
        command_cap("session instance connect", ["offline"], "retired"),
        command_cap("session instance reconnect", ["offline"], "retired"),
        command_cap("session instance app", ["device"], "available"),
        command_cap("session instance app launch", ["device"], "available"),
        command_cap("session instance app stop", ["device"], "available"),
        command_cap("session instance app force-stop", ["device"], "available"),
        command_cap("session instance app restart", ["device"], "available"),
        command_cap("session app", ["device"], "available"),
        command_cap("session app launch", ["device"], "available"),
        command_cap("session app stop", ["device"], "available"),
        command_cap("session app force-stop", ["device"], "available"),
        command_cap("session app restart", ["device"], "available"),
        command_cap("session capture", ["device"], "available"),
        command_cap("session capture diagnose", ["device"], "available"),
        command_cap("session recover", ["device"], "available"),
        command_cap("session recover --stale-capture", ["device"], "available"),
        command_cap(
            "session lease run",
            ["running_runtime", "lab_lease"],
            "available",
        ),
        command_cap("session record", ["offline"], "available"),
        command_cap("session record start", ["offline"], "available"),
        command_cap("session record status", ["offline"], "available"),
        command_cap("session record stop", ["offline"], "available"),
        command_cap("session record step", ["offline", "device"], "available"),
        command_cap("session record candidates", ["offline"], "available"),
        command_cap("session record amend", ["offline"], "available"),
        command_cap("session record build-task", ["offline"], "available"),
        command_cap("session record promote", ["offline"], "available"),
        command_cap("record", ["offline"], "available"),
        command_cap("record start", ["offline"], "available"),
        command_cap("record status", ["offline"], "available"),
        command_cap("record stop", ["offline"], "available"),
        command_cap("record step", ["offline", "device"], "available"),
        command_cap("record candidates", ["offline"], "available"),
        command_cap("record amend", ["offline"], "available"),
        command_cap("record build-task", ["offline"], "available"),
        command_cap("record promote", ["offline"], "available"),
        command_cap("current-page", ["device"], "available"),
        command_cap("is-visible", ["device"], "available"),
        command_cap("locate", ["device"], "available"),
        command_cap("tap-target", ["device"], "available"),
        command_cap("navigate", ["device"], "available"),
        command_cap("monitor --once", ["device"], "available"),
        command_cap("monitor", ["device"], "available"),
        command_cap("stream", ["device"], "available"),
        command_cap("scheduler status", ["running_runtime"], "reserved"),
        command_cap("scheduler pause", ["running_runtime"], "reserved"),
        command_cap("scheduler resume", ["running_runtime"], "reserved"),
        command_cap("scheduler start", ["running_runtime"], "reserved"),
        command_cap("scheduler stop", ["running_runtime"], "reserved"),
        command_cap("lab validate", ["offline"], "available"),
        command_cap("lab run", ["device"], "available"),
        command_cap("capture", ["device"], "available"),
        command_cap("capture diagnose", ["device"], "available"),
        command_cap("detect", ["device"], "available"),
        command_cap("env resolve", ["offline"], "available"),
        command_cap("env status", ["offline"], "available"),
        command_cap("detect-page", ["device"], "available"),
        command_cap("recognize", ["device"], "available"),
        command_cap(
            "operation dry-run",
            ["running_runtime", "device"],
            "reserved",
        ),
        command_cap(
            "operation run",
            ["running_runtime", "device", "lab_lease"],
            "blocked_until_lab_lease",
        ),
        command_cap(
            "control probe-click",
            ["running_runtime", "device", "lab_lease"],
            "blocked_until_lab_lease",
        ),
        command_cap(
            "package run",
            ["running_runtime", "device", "lab_lease"],
            "blocked_until_lab_lease",
        ),
    ];
    commands.extend(runtime_debug::capabilities());
    commands
}

fn command_cap<I>(command: &str, needs: I, status: &str) -> Value
where
    I: IntoIterator,
    I::Item: Into<String>,
{
    json!({
        "command": command,
        "needs": needs.into_iter().map(Into::into).collect::<Vec<String>>(),
        "status": status
    })
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

fn split_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn path_string(path: &Path) -> String {
    path.display().to_string()
}

fn file_sha256(path: &Path) -> CliOutcome<String> {
    let bytes = fs::read(path)
        .map_err(|err| CliError::usage(format!("failed to read {}: {err}", path.display())))?;
    Ok(hex_sha256(&bytes))
}

fn hex_sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[path = "contained_semantic.rs"]
    mod contained_semantic;
    #[path = "semantic_fixture.rs"]
    mod semantic_fixture;
    #[path = "test_env.rs"]
    mod test_env;
    use actingcommand_contract::{IdentifierIssuer, InstanceId};
    use actingcommand_device::{CaptureBackend, DeviceError, DeviceResult};
    use actingcommand_runtime_host::{
        ExecutionBackendProvider, ResolvedExecutionInstance, RuntimeHost, RuntimeHostConfig,
    };
    use semantic_fixture::{
        run_semantic_cli, seal_semantic_fixture, semantic_resource_root,
        synthetic_game_resource_root, template_drift_resource_root,
    };
    use std::process::Stdio;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;
    use test_env::TrustedRemoteEnvGuard;

    static ENV_LOCK: Mutex<()> = Mutex::new(());
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn set_config_env(path: impl AsRef<Path>) {
        unsafe {
            env::set_var(CONFIG_ENV, path.as_ref());
        }
    }

    fn set_missing_config_env() {
        let path = env::temp_dir().join(format!(
            "actinglab-missing-config-{}-{}.json",
            std::process::id(),
            JSON_TMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        unsafe {
            env::set_var(CONFIG_ENV, path);
        }
    }

    struct RuntimeStateEnvGuard {
        previous: Option<OsString>,
    }

    impl Drop for RuntimeStateEnvGuard {
        fn drop(&mut self) {
            unsafe {
                if let Some(previous) = self.previous.take() {
                    env::set_var(RUNTIME_STATE_ROOT_ENV, previous);
                } else {
                    env::remove_var(RUNTIME_STATE_ROOT_ENV);
                }
            }
        }
    }

    struct AuthoringRuntimeProvider {
        instance_id: InstanceId,
    }

    impl ExecutionBackendProvider for AuthoringRuntimeProvider {
        fn instance_aliases(&self) -> Vec<String> {
            vec!["fixture".to_string()]
        }

        fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
            (instance_alias == "fixture")
                .then(|| ResolvedExecutionInstance::new(self.instance_id, "<authoring-test>"))
        }

        fn open_input(&self, _instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>> {
            Err(DeviceError::fatal(
                "resource authoring must not open an input backend",
            ))
        }

        fn open_capture(&self, _instance_alias: &str) -> DeviceResult<Box<dyn CaptureBackend>> {
            Err(DeviceError::fatal(
                "resource authoring must not open a capture backend",
            ))
        }

        fn control_application(
            &self,
            _instance_alias: &str,
            _action: actingcommand_contract::ApplicationLifecycleAction,
        ) -> DeviceResult<()> {
            Err(DeviceError::fatal(
                "resource authoring must not control applications",
            ))
        }
    }

    fn use_runtime_state_root(path: &Path) -> RuntimeStateEnvGuard {
        let previous = env::var_os(RUNTIME_STATE_ROOT_ENV);
        unsafe {
            env::set_var(RUNTIME_STATE_ROOT_ENV, path);
        }
        RuntimeStateEnvGuard { previous }
    }

    fn start_authoring_runtime(state_root: &Path) -> RuntimeHost {
        let instance_id = *IdentifierIssuer::new()
            .expect("identifier issuer")
            .mint_instance_id()
            .expect("instance id")
            .transport();
        RuntimeHost::start(
            RuntimeHostConfig::new(state_root, b"actinglab-resource-authoring-test"),
            Arc::new(AuthoringRuntimeProvider { instance_id }),
        )
        .expect("Runtime host")
    }

    fn prepare_promotable_record(config: &Path, state_dir: &Path, frame_path: &Path) {
        fs::write(frame_path, test_record_frame_png(12, 10)).expect("record frame");
        set_config_env(config);
        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let home_anchor = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        let mail_anchor = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "mail-anchor",
                "--id",
                "page/mail",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        let operation = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "operation",
                "--step-id",
                "home-to-mail",
                "--from",
                "page/home",
                "--to",
                "page/mail",
                "--click",
                "5,6",
            ],
            true,
        );
        for result in [start, home_anchor, mail_anchor, operation] {
            assert_eq!(
                result.exit_code(),
                0,
                "{}",
                serde_json::to_string_pretty(&result.envelope).unwrap()
            );
        }
    }

    fn set_isolated_app_env() -> TempDir {
        let temp = TempDir::new().unwrap();
        unsafe {
            env::set_var("LOCALAPPDATA", temp.path());
            env::set_var("APPDATA", temp.path());
        }
        temp
    }

    fn user_config_with_test_adb() -> (TempDir, UserConfig) {
        let temp = tempfile::tempdir().unwrap();
        let adb_name = if cfg!(windows) { "adb.exe" } else { "adb" };
        let adb_path = temp.path().join(adb_name);
        fs::write(&adb_path, b"test adb placeholder").unwrap();
        (
            temp,
            UserConfig {
                adb_path: Some(adb_path.to_string_lossy().to_string()),
                ..Default::default()
            },
        )
    }

    fn path_baseline_adb() -> actingcommand_device::ResolvedAdbPath {
        actingcommand_device::ResolvedAdbPath {
            path: "test-adb".to_string(),
            source: AdbPathSource::PathBaseline,
            warning: Some("WARNING: using PATH adb as a non-MuMu baseline channel".to_string()),
        }
    }

    #[test]
    fn tests_mutate_config_env_only_through_fixture_helpers() {
        let source = include_str!("main.rs");
        assert_eq!(
            source
                .matches(concat!("env::set", "_var(CONFIG_ENV"))
                .count(),
            2
        );
        assert!(!source.contains(concat!("env::remove", "_var(CONFIG_ENV")));
    }

    fn create_test_dir_alias(link: &Path, target: &Path) -> bool {
        #[cfg(windows)]
        {
            Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|status| status.success())
                .unwrap_or(false)
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
        #[cfg(not(any(windows, unix)))]
        {
            let _ = (link, target);
            false
        }
    }

    fn test_record_frame_png(width: u32, height: u32) -> Vec<u8> {
        let mut pixels = Vec::new();
        for y in 0..height {
            for x in 0..width {
                pixels.extend_from_slice(&[x as u8, y as u8, 128, 255]);
            }
        }
        Frame::from_pixels(
            width,
            height,
            pixels,
            PixelFormat::Rgba8,
            CaptureBackendName::AdbScreencap,
        )
        .expect("test frame")
        .png_for_artifact()
        .expect("test frame png")
    }

    fn drift_test_record(steps: Value) -> SessionRecordContext {
        serde_json::from_value(json!({
            "schema_version": "session.record.v0.1",
            "record_id": "record-1",
            "task_id": "daily-check",
            "instance": "fixture",
            "status": "recording",
            "started_at_unix_ms": 1,
            "updated_at_unix_ms": 2,
            "steps": steps
        }))
        .expect("drift test record")
    }

    fn drift_test_anchor_step(step_id: &str, id: &str) -> Value {
        json!({
            "schema_version": "session.record_step.v0.1",
            "step_id": step_id,
            "created_at_unix_ms": 1,
            "updated_at_unix_ms": 2,
            "kind": "anchor",
            "id": id,
            "region": {"mode": "rect", "rect": {"x": 1, "y": 2, "width": 3, "height": 4}},
            "color_check": false,
            "evaluation": {
                "status": "deferred",
                "reason": "synthetic"
            }
        })
    }

    fn test_contrast_record_frame_png(width: u32, height: u32) -> Vec<u8> {
        let mut pixels = Vec::new();
        for y in 0..height {
            for x in 0..width {
                pixels.extend_from_slice(&[
                    ((x * 37 + y * 17 + 91) % 256) as u8,
                    ((x * 13 + y * 53 + 7) % 256) as u8,
                    ((x * 97 + y * 11 + 3) % 256) as u8,
                    255,
                ]);
            }
        }
        Frame::from_pixels(
            width,
            height,
            pixels,
            PixelFormat::Rgba8,
            CaptureBackendName::AdbScreencap,
        )
        .expect("test contrast frame")
        .png_for_artifact()
        .expect("test contrast frame png")
    }

    fn test_auto_region_discrimination_frame_png(contrast: bool) -> Vec<u8> {
        let width = 12;
        let height = 9;
        let mut pixels = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let in_top_left = x < 4 && y < 3;
                let in_center = (4..8).contains(&x) && (3..6).contains(&y);
                let checker = if (x + y) % 2 == 0 { 240 } else { 40 };
                let value = if in_top_left {
                    checker
                } else if in_center && !contrast {
                    255 - checker
                } else {
                    72
                };
                pixels.extend_from_slice(&[value, value, value, 255]);
            }
        }
        Frame::from_pixels(
            width,
            height,
            pixels,
            PixelFormat::Rgba8,
            CaptureBackendName::AdbScreencap,
        )
        .expect("test auto region frame")
        .png_for_artifact()
        .expect("test auto region frame png")
    }

    #[test]
    fn version_outputs_json_envelope() {
        let result = run_cli(["--json", "--version"], true);
        assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
        assert!(result.envelope.ok);
        assert_eq!(result.envelope.command, "version");
    }

    #[test]
    fn status_without_runtime_is_exit_five() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        set_config_env(temp.path().join("config.json"));

        let result = run_cli(["--json", "status"], true);
        set_missing_config_env();

        assert_eq!(result.exit_code(), 5);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "runtime_not_running"
        );
    }

    #[test]
    fn runtime_endpoint_policy_allows_loopback_without_auth() {
        let _guard = env_lock();
        let _trusted_remote_env = TrustedRemoteEnvGuard::clear();
        let policy = runtime_endpoint_policy("http://127.0.0.1:4317").unwrap();
        assert_eq!(policy.channel, RuntimeEndpointChannel::LocalDirect);
        assert_eq!(policy.scheme, "http");
        assert_eq!(policy.host, "127.0.0.1");
        assert_eq!(policy.port, 4317);
        assert_eq!(policy.auth_material, None);
    }

    #[test]
    fn runtime_endpoint_policy_blocks_remote_http() {
        let _guard = env_lock();
        let _trusted_remote_env = TrustedRemoteEnvGuard::clear();
        let err = runtime_endpoint_policy("http://example.invalid:4317").unwrap_err();
        assert_eq!(err.code, "trusted_remote_transport_blocked");
        assert_eq!(err.exit_code(), 3);
    }

    #[test]
    fn runtime_endpoint_policy_blocks_remote_https_without_auth() {
        let _guard = env_lock();
        let _trusted_remote_env = TrustedRemoteEnvGuard::clear();
        let err = runtime_endpoint_policy("https://example.invalid:4317").unwrap_err();
        assert_eq!(err.code, "trusted_remote_auth_required");
        assert_eq!(err.exit_code(), 3);
    }

    #[test]
    fn runtime_endpoint_policy_accepts_remote_https_with_token() {
        let _guard = env_lock();
        let _trusted_remote_env = TrustedRemoteEnvGuard::with_token("test-token");
        let policy = runtime_endpoint_policy("https://example.invalid:4317").unwrap();
        assert_eq!(policy.channel, RuntimeEndpointChannel::TrustedRemote);
        assert_eq!(policy.scheme, "https");
        assert_eq!(policy.auth_material, Some("token"));
    }

    #[test]
    fn session_transport_check_reports_loopback_policy() {
        let _guard = env_lock();
        let _trusted_remote_env = TrustedRemoteEnvGuard::clear();
        let result = run_cli(
            [
                "--json",
                "session",
                "transport",
                "check",
                "--endpoint",
                "http://127.0.0.1:4317",
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
        let data = result.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.get("schema_version").and_then(Value::as_str),
            Some("session.transport_check.v0.1")
        );
        assert_eq!(
            data.pointer("/check/policy/channel")
                .and_then(Value::as_str),
            Some("local_direct")
        );
        assert_eq!(
            data.pointer("/check/policy/authentication_required")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            data.get("does_not_start_listener").and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn session_transport_plan_reports_reserved_trusted_channel_without_listener() {
        let _guard = env_lock();
        let _trusted_remote_env = TrustedRemoteEnvGuard::clear();
        let result = run_cli(["--json", "session", "transport", "plan"], true);

        assert_eq!(result.exit_code(), 0);
        let data = result.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.get("schema_version").and_then(Value::as_str),
            Some("session.transport_plan.v0.1")
        );
        assert_eq!(data.get("status").and_then(Value::as_str), Some("reserved"));
        assert_eq!(
            data.pointer("/trusted_remote/network_listener_implemented")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            data.pointer("/trusted_remote/ready_to_accept_remote_clients")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            data.pointer("/trusted_remote/token_configured")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            data.pointer("/trusted_remote/endpoint_policy/checked")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            data.pointer("/trusted_remote_gate/schema_version")
                .and_then(Value::as_str),
            Some("session.trusted_remote_gate.v0.1")
        );
        assert_eq!(
            data.pointer("/trusted_remote_gate/status")
                .and_then(Value::as_str),
            Some("reserved")
        );
        assert_eq!(
            data.pointer("/trusted_remote_gate/auth_material_configured")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            data.pointer("/trusted_remote_gate/safe_to_accept_remote_clients")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert!(
            data.pointer("/trusted_remote_gate/blocked_reasons")
                .and_then(Value::as_array)
                .expect("trusted remote gate must expose blocked reasons")
                .iter()
                .any(|reason| {
                    reason.get("code").and_then(Value::as_str)
                        == Some("trusted_remote_auth_required")
                })
        );
        assert_eq!(
            data.pointer("/next_actions/schema_version")
                .and_then(Value::as_str),
            Some("session.transport_next_actions.v0.1")
        );
        assert_eq!(
            data.pointer("/next_actions/ordered/0/action")
                .and_then(Value::as_str),
            Some("classify_endpoint_policy")
        );
        assert_eq!(
            data.pointer("/next_actions/trusted_remote/auth_material_configured")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            data.pointer("/guarantees/does_not_start_listener")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/guarantees/does_not_probe_tcp")
                .and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn session_transport_plan_blocks_remote_http_without_tcp_probe() {
        let _guard = env_lock();
        let _trusted_remote_env = TrustedRemoteEnvGuard::clear();
        let result = run_cli(
            [
                "--json",
                "session",
                "transport",
                "plan",
                "--endpoint",
                "http://192.0.2.1:4317",
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0);
        let data = result.envelope.data.as_ref().unwrap();
        assert_eq!(data.get("status").and_then(Value::as_str), Some("blocked"));
        assert_eq!(
            data.pointer("/trusted_remote/endpoint_policy/safe_for_policy")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            data.pointer("/trusted_remote/endpoint_policy/error_code")
                .and_then(Value::as_str),
            Some("trusted_remote_transport_blocked")
        );
        assert_eq!(
            data.pointer("/trusted_remote_gate/status")
                .and_then(Value::as_str),
            Some("blocked")
        );
        assert_eq!(
            data.pointer("/trusted_remote_gate/endpoint_policy_safe")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            data.pointer("/trusted_remote_gate/guarantees/does_not_probe_tcp")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/trusted_remote/endpoint_policy/does_not_probe_tcp")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert!(
            data.get("blockers")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .any(|blocker| blocker.get("kind").and_then(Value::as_str)
                    == Some("trusted_remote_endpoint_policy"))
        );
        assert_eq!(
            data.pointer("/next_actions/status").and_then(Value::as_str),
            Some("blocked")
        );
        assert_eq!(
            data.pointer("/next_actions/ordered/0/action")
                .and_then(Value::as_str),
            Some("review_endpoint_policy_blocker")
        );
        assert_eq!(
            data.pointer("/next_actions/guarantees/does_not_probe_tcp")
                .and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn session_transport_plan_accepts_remote_https_policy_but_keeps_listener_reserved() {
        let _guard = env_lock();
        let _trusted_remote_env = TrustedRemoteEnvGuard::with_token("test-token");
        let result = run_cli(
            [
                "--json",
                "session",
                "transport",
                "plan",
                "--endpoint",
                "https://example.invalid:4317",
            ],
            true,
        );
        assert_eq!(result.exit_code(), 0);
        let data = result.envelope.data.as_ref().unwrap();
        assert_eq!(data.get("status").and_then(Value::as_str), Some("reserved"));
        assert_eq!(
            data.pointer("/trusted_remote/endpoint_policy/safe_for_policy")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/trusted_remote/endpoint_policy/policy/channel")
                .and_then(Value::as_str),
            Some("trusted_remote")
        );
        assert_eq!(
            data.pointer("/trusted_remote/endpoint_policy/policy/auth_material")
                .and_then(Value::as_str),
            Some("token")
        );
        assert_eq!(
            data.pointer("/trusted_remote_gate/status")
                .and_then(Value::as_str),
            Some("reserved")
        );
        assert_eq!(
            data.pointer("/trusted_remote_gate/trusted_remote_requested")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/trusted_remote_gate/auth_material_configured")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/trusted_remote_gate/network_listener_implemented")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            data.pointer("/trusted_remote_gate/guarantees/does_not_start_tls")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/trusted_remote/ready_to_accept_remote_clients")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            data.pointer("/next_actions/status").and_then(Value::as_str),
            Some("reserved")
        );
        assert_eq!(
            data.pointer("/next_actions/trusted_remote/auth_material_configured")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/next_actions/ordered/0/action")
                .and_then(Value::as_str),
            Some("review_listener_and_tls_design")
        );
    }

    #[test]
    fn session_transport_check_blocks_remote_http() {
        let _guard = env_lock();
        let _trusted_remote_env = TrustedRemoteEnvGuard::clear();
        let result = run_cli(
            [
                "--json",
                "session",
                "transport",
                "check",
                "--endpoint",
                "http://192.0.2.1:4317",
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0);
        let data = result.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.get("safe_to_connect").and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            data.pointer("/check/error_code").and_then(Value::as_str),
            Some("trusted_remote_transport_blocked")
        );
        assert_eq!(
            data.pointer("/check/blocked_by/1").and_then(Value::as_str),
            Some("encryption")
        );
    }

    #[test]
    fn status_blocks_untrusted_remote_runtime_endpoint() {
        let _guard = env_lock();
        set_missing_config_env();
        let _trusted_remote_env = TrustedRemoteEnvGuard::clear();
        let result = run_cli(
            [
                "--json",
                "--runtime-endpoint",
                "http://example.invalid:4317",
                "status",
            ],
            true,
        );
        assert_eq!(result.exit_code(), 3);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "trusted_remote_transport_blocked"
        );
    }

    #[test]
    fn doctor_reports_remote_endpoint_policy_without_blocking() {
        let _guard = env_lock();
        set_missing_config_env();
        let _trusted_remote_env = TrustedRemoteEnvGuard::clear();
        let result = run_cli(
            [
                "--json",
                "--runtime-endpoint",
                "https://example.invalid:4317",
                "doctor",
            ],
            true,
        );
        assert_eq!(result.exit_code(), 0);
        let checks = result
            .envelope
            .data
            .as_ref()
            .unwrap()
            .get("checks")
            .and_then(Value::as_array)
            .unwrap();
        let runtime = checks
            .iter()
            .find(|check| check.get("name").and_then(Value::as_str) == Some("runtime_endpoint"))
            .expect("runtime endpoint check");
        assert_eq!(runtime.get("ok").and_then(Value::as_bool), Some(false));
        assert_eq!(
            runtime
                .pointer("/policy/error_code")
                .and_then(Value::as_str),
            Some("trusted_remote_auth_required")
        );
    }

    #[test]
    fn doctor_reports_path_adb_baseline_warning() {
        let adb = resolved_adb_json_from(Ok(path_baseline_adb()));
        assert_eq!(
            adb.get("source").and_then(Value::as_str),
            Some("path_adb_baseline")
        );
        assert!(
            adb.get("warning")
                .and_then(Value::as_str)
                .is_some_and(|warning| warning.contains("non-MuMu baseline"))
        );
    }

    #[test]
    fn device_config_rejects_path_adb_for_nemu_ipc_without_opt_in() {
        let _guard = env_lock();
        unsafe {
            env::remove_var(ALLOW_PATH_ADB_FOR_MUMU_ENV);
        }
        let instance = InstanceConfig {
            capture_backend: Some("nemu_ipc".to_string()),
            ..Default::default()
        };

        let error = enforce_path_adb_target_boundary(
            &path_baseline_adb(),
            Some(&instance),
            CaptureBackendChoice::NemuIpc,
        )
        .expect_err("MuMu/Nemu IPC must not use PATH baseline by default");

        assert_eq!(error.code, "device_error");
        assert!(error.message.contains(ALLOW_PATH_ADB_FOR_MUMU_ENV));
    }

    #[test]
    fn device_config_allows_path_adb_for_nemu_ipc_with_explicit_opt_in() {
        let _guard = env_lock();
        unsafe {
            env::set_var(ALLOW_PATH_ADB_FOR_MUMU_ENV, "1");
        }
        let instance = InstanceConfig {
            capture_backend: Some("nemu_ipc".to_string()),
            ..Default::default()
        };
        let resolved = path_baseline_adb();

        enforce_path_adb_target_boundary(&resolved, Some(&instance), CaptureBackendChoice::NemuIpc)
            .expect("explicit opt-in allows PATH baseline");

        assert_eq!(resolved.source, AdbPathSource::PathBaseline);
        assert!(
            resolved
                .warning
                .as_deref()
                .is_some_and(|warning| warning.contains("non-MuMu baseline"))
        );
        unsafe {
            env::remove_var(ALLOW_PATH_ADB_FOR_MUMU_ENV);
        }
    }

    #[test]
    fn scheduler_stub_is_exit_six() {
        let result = run_cli(["--json", "scheduler", "status"], true);
        assert_eq!(result.exit_code(), 6);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "scheduler_not_available"
        );
    }

    #[test]
    fn config_set_and_get_round_trip() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        set_config_env(&config);

        let set = run_cli(
            [
                "--json",
                "config",
                "set",
                "instance.c.serial",
                "127.0.0.1:16448",
            ],
            true,
        );
        assert_eq!(set.exit_code(), 0);
        let get = run_cli(["--json", "config", "get", "instance.c.serial"], true);
        set_missing_config_env();

        assert_eq!(get.exit_code(), 0);
        assert_eq!(
            get.envelope
                .data
                .as_ref()
                .unwrap()
                .get("value")
                .and_then(Value::as_str),
            Some("127.0.0.1:16448")
        );
    }

    #[test]
    fn config_set_and_get_instance_package() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        set_config_env(&config);

        let set = run_cli(
            [
                "--json",
                "config",
                "set",
                "instance.a.package",
                "org.example.client",
            ],
            true,
        );
        assert_eq!(set.exit_code(), 0);
        let get = run_cli(["--json", "config", "get", "instance.a.package"], true);
        set_missing_config_env();

        assert_eq!(get.exit_code(), 0);
        assert_eq!(
            get.envelope
                .data
                .as_ref()
                .unwrap()
                .get("value")
                .and_then(Value::as_str),
            Some("org.example.client")
        );
    }

    #[test]
    fn config_set_and_get_instance_adb_and_capture_backend() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        set_config_env(&config);

        let adb = run_cli(
            [
                "--json",
                "config",
                "set",
                "instance.b.adb_path",
                "C:\\Tools\\adb.exe",
            ],
            true,
        );
        let backend = run_cli(
            [
                "--json",
                "config",
                "set",
                "instance.b.capture_backend",
                "nemu_ipc",
            ],
            true,
        );
        let get_adb = run_cli(["--json", "config", "get", "instance.b.adb_path"], true);
        let get_backend = run_cli(
            ["--json", "config", "get", "instance.b.capture_backend"],
            true,
        );
        set_missing_config_env();

        assert_eq!(adb.exit_code(), 0);
        assert_eq!(backend.exit_code(), 0);
        assert_eq!(
            get_adb
                .envelope
                .data
                .as_ref()
                .unwrap()
                .get("value")
                .and_then(Value::as_str),
            Some("C:\\Tools\\adb.exe")
        );
        assert_eq!(
            get_backend
                .envelope
                .data
                .as_ref()
                .unwrap()
                .get("value")
                .and_then(Value::as_str),
            Some("nemu_ipc")
        );
    }

    #[test]
    fn config_set_rejects_invalid_instance_capture_backend() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        set_config_env(&config);

        let result = run_cli(
            [
                "--json",
                "config",
                "set",
                "instance.b.capture_backend",
                "not-a-backend",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(result.exit_code(), 2);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "validation_failed"
        );
    }

    #[test]
    fn write_json_file_atomic_uses_unique_tmp_and_publishes_complete_json() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("state.json");
        let stale_tmp = path.with_extension(format!("tmp-{}-stale", std::process::id()));
        fs::write(&stale_tmp, "stale").unwrap();

        for value in [
            json!({"value": 1}),
            json!({"value": 2}),
            json!({"value": 3}),
        ] {
            write_json_file_atomic(&path, &value).unwrap();
        }

        let stored = fs::read_to_string(&path).unwrap();
        let parsed: Value = serde_json::from_str(&stored).unwrap();
        assert_eq!(parsed.get("value").and_then(Value::as_u64), Some(3));
        assert!(!stale_tmp.exists());
        let leftovers = fs::read_dir(temp.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains("tmp-"))
            .count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn session_record_start_status_and_stop_write_context() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
                "--holder",
                "scheduler",
                "--lease-id",
                "lease-1",
            ],
            true,
        );
        let status = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "status",
                "--state-dir",
                state_dir.to_str().unwrap(),
            ],
            true,
        );
        let stop = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "stop",
                "--state-dir",
                state_dir.to_str().unwrap(),
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        let start_data = start.envelope.data.as_ref().unwrap();
        assert_eq!(
            start_data.get("auto_recording").and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            start_data.pointer("/record/status").and_then(Value::as_str),
            Some("active")
        );
        assert_eq!(
            start_data
                .pointer("/record/task_id")
                .and_then(Value::as_str),
            Some("daily-check")
        );
        assert_eq!(
            start_data
                .pointer("/record/instance")
                .and_then(Value::as_str),
            Some("fixture")
        );
        assert_eq!(
            start_data.pointer("/record/holder").and_then(Value::as_str),
            Some("scheduler")
        );
        assert_eq!(
            start_data
                .pointer("/record/lease_id")
                .and_then(Value::as_str),
            Some("lease-1")
        );
        assert!(
            start_data
                .pointer("/record/steps")
                .and_then(Value::as_array)
                .is_some_and(Vec::is_empty)
        );

        assert_eq!(status.exit_code(), 0);
        assert_eq!(
            status
                .envelope
                .data
                .as_ref()
                .unwrap()
                .get("status")
                .and_then(Value::as_str),
            Some("available")
        );

        assert_eq!(stop.exit_code(), 0);
        assert_eq!(
            stop.envelope
                .data
                .as_ref()
                .unwrap()
                .pointer("/record/status")
                .and_then(Value::as_str),
            Some("stopped")
        );
    }

    #[test]
    fn top_level_record_alias_uses_session_record_context() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let status = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "status",
                "--state-dir",
                state_dir.to_str().unwrap(),
            ],
            true,
        );
        let stop = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "record",
                "stop",
                "--state-dir",
                state_dir.to_str().unwrap(),
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(
            start
                .envelope
                .data
                .as_ref()
                .unwrap()
                .pointer("/record/task_id")
                .and_then(Value::as_str),
            Some("daily-check")
        );
        assert_eq!(status.exit_code(), 0);
        assert_eq!(
            status
                .envelope
                .data
                .as_ref()
                .unwrap()
                .pointer("/record/status")
                .and_then(Value::as_str),
            Some("active")
        );
        assert_eq!(stop.exit_code(), 0);
        assert_eq!(stop.envelope.command.as_str(), "record");
        assert_eq!(
            stop.envelope
                .data
                .as_ref()
                .unwrap()
                .pointer("/record/status")
                .and_then(Value::as_str),
            Some("stopped")
        );
    }

    #[test]
    fn top_level_record_build_task_routes_to_session_record() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        set_config_env(&config);

        let build = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "record",
                "build-task",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--out",
                temp.path().join("draft").to_str().unwrap(),
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(build.exit_code(), 3);
        assert_eq!(
            build.envelope.error.as_ref().unwrap().code,
            "record_session_not_active"
        );
    }

    #[test]
    fn stream_command_reports_bounded_dry_run_contract() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        set_config_env(&config);

        let stream = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "stream",
                "--dry-run",
                "--max-frames",
                "2",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(stream.exit_code(), 0);
        let data = stream.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.get("mode").and_then(Value::as_str),
            Some("bounded_stream")
        );
        assert_eq!(
            data.pointer("/input_relay/status").and_then(Value::as_str),
            Some("disabled")
        );
        assert_eq!(
            data.pointer("/capture/dry_run").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/contract/schema_version")
                .and_then(Value::as_str),
            Some("session.stream.v0.1")
        );
        assert_eq!(
            data.pointer("/contract/status").and_then(Value::as_str),
            Some("available")
        );
        assert_eq!(
            data.pointer("/contract/event_schema_version")
                .and_then(Value::as_str),
            Some("session.stream.event.v0.1")
        );
        assert_eq!(
            data.pointer("/contract/safety/session_layer_only_throat")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/contract/input_relay/requested")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            data.pointer("/contract/input_relay/execution_model")
                .and_then(Value::as_str),
            Some("planned_only")
        );
        assert_eq!(
            data.pointer("/trusted_channel/long_lived_stream_implemented")
                .and_then(Value::as_bool),
            Some(false)
        );
        let stream_id = data.get("stream_id").and_then(Value::as_str).unwrap();
        assert!(stream_id.starts_with("stream-"));
        let events = data.get("events").and_then(Value::as_array).unwrap();
        assert_eq!(events.len(), 4);
        assert_eq!(
            events[0].get("schema_version").and_then(Value::as_str),
            Some("session.stream.event.v0.1")
        );
        assert_eq!(
            events[0].get("stream_id").and_then(Value::as_str),
            Some(stream_id)
        );
        assert_eq!(
            events[0].get("event_index").and_then(Value::as_u64),
            Some(0)
        );
        assert_eq!(
            events[0].get("type").and_then(Value::as_str),
            Some("stream.started")
        );
        assert_eq!(
            events[1].get("stream_id").and_then(Value::as_str),
            Some(stream_id)
        );
        assert_eq!(
            events[1].get("event_index").and_then(Value::as_u64),
            Some(1)
        );
        assert_eq!(
            events[1].get("type").and_then(Value::as_str),
            Some("stream.frame_sampled")
        );
        assert_eq!(
            events[3].get("stream_id").and_then(Value::as_str),
            Some(stream_id)
        );
        assert_eq!(
            events[3].get("event_index").and_then(Value::as_u64),
            Some(3)
        );
        assert_eq!(
            events[3].get("type").and_then(Value::as_str),
            Some("stream.completed")
        );
        assert_eq!(
            data.get("frames").and_then(Value::as_array).unwrap().len(),
            2
        );
    }

    #[test]
    fn session_record_active_start_requires_force() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        set_config_env(&config);

        let first = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let conflict = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check-2",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(first.exit_code(), 0);
        assert_eq!(conflict.exit_code(), 3);
        assert_eq!(
            conflict.envelope.error.as_ref().unwrap().code,
            "record_session_active"
        );
    }

    #[test]
    fn session_record_build_task_requires_record() {
        let temp = TempDir::new().unwrap();
        let result = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "build-task",
                "--state-dir",
                temp.path().to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 3);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "record_session_not_active"
        );
    }

    #[test]
    fn session_record_step_anchor_records_region_schema() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "10,20,30,40",
                "--color-check",
                "--threshold",
                "0.96",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        let data = step.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.get("status").and_then(Value::as_str),
            Some("step_recorded")
        );
        assert_eq!(data.get("step_count").and_then(Value::as_u64), Some(1));
        assert_eq!(
            data.pointer("/step/step_id").and_then(Value::as_str),
            Some("home-anchor")
        );
        assert_eq!(
            data.pointer("/step/kind").and_then(Value::as_str),
            Some("anchor")
        );
        assert_eq!(
            data.pointer("/step/id").and_then(Value::as_str),
            Some("page/home")
        );
        assert_eq!(
            data.pointer("/step/region/mode").and_then(Value::as_str),
            Some("rect")
        );
        assert_eq!(
            data.pointer("/step/region/rect/x").and_then(Value::as_i64),
            Some(10)
        );
        assert_eq!(
            data.pointer("/step/color_check").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/step/threshold").and_then(Value::as_f64),
            Some(0.96)
        );
        assert_eq!(
            data.pointer("/step/evaluation/status")
                .and_then(Value::as_str),
            Some("deferred")
        );
        assert_eq!(
            data.pointer("/step/evaluation/reason")
                .and_then(Value::as_str),
            Some("frame_not_provided")
        );
        assert!(data.pointer("/step/evaluation/backtest").is_none());
        assert_eq!(
            data.pointer("/record/steps/0/step_id")
                .and_then(Value::as_str),
            Some("home-anchor")
        );
    }

    #[test]
    fn session_record_step_color_probe_records_deferred_schema() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "color-probe",
                "--step-id",
                "home-color",
                "--id",
                "color/home-status",
                "--region",
                "10,20,30,40",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        let data = step.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/step/kind").and_then(Value::as_str),
            Some("color_probe")
        );
        assert_eq!(
            data.pointer("/step/id").and_then(Value::as_str),
            Some("color/home-status")
        );
        assert_eq!(
            data.pointer("/step/region/mode").and_then(Value::as_str),
            Some("rect")
        );
        assert!(data.pointer("/step/expected").is_none());
        assert_eq!(
            data.pointer("/step/evaluation/status")
                .and_then(Value::as_str),
            Some("deferred")
        );
        assert_eq!(
            data.pointer("/step/evaluation/reason")
                .and_then(Value::as_str),
            Some("frame_not_provided")
        );
    }

    #[test]
    fn session_record_step_color_probe_samples_frame() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "color-probe",
                "--step-id",
                "home-color",
                "--id",
                "color/home-status",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(
            step.exit_code(),
            0,
            "{}",
            serde_json::to_string_pretty(&step.envelope).unwrap()
        );
        let data = step.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/step/kind").and_then(Value::as_str),
            Some("color_probe")
        );
        assert_eq!(
            data.pointer("/step/expected/0").and_then(Value::as_u64),
            Some(3)
        );
        assert_eq!(
            data.pointer("/step/expected/1").and_then(Value::as_u64),
            Some(5)
        );
        assert_eq!(
            data.pointer("/step/expected/2").and_then(Value::as_u64),
            Some(128)
        );
        assert_eq!(
            data.pointer("/step/frame_provenance/source")
                .and_then(Value::as_str),
            Some("local_png")
        );
        assert_eq!(
            data.pointer("/step/evaluation/status")
                .and_then(Value::as_str),
            Some("passed")
        );
        assert_eq!(
            data.pointer("/step/evaluation/reason")
                .and_then(Value::as_str),
            Some("color_probe_sampled")
        );
    }

    #[test]
    fn session_record_step_verify_template_records_deferred_schema() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "verify-template",
                "--step-id",
                "mail-ready",
                "--id",
                "template/mail-ready",
                "--region",
                "10,20,30,40",
                "--threshold",
                "0.97",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        let data = step.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/step/kind").and_then(Value::as_str),
            Some("verify_template")
        );
        assert_eq!(
            data.pointer("/step/id").and_then(Value::as_str),
            Some("template/mail-ready")
        );
        assert_eq!(
            data.pointer("/step/region/mode").and_then(Value::as_str),
            Some("rect")
        );
        assert_eq!(
            data.pointer("/step/threshold").and_then(Value::as_f64),
            Some(0.97)
        );
        assert!(data.pointer("/step/artifact").is_none());
        assert_eq!(
            data.pointer("/step/evaluation/status")
                .and_then(Value::as_str),
            Some("deferred")
        );
        assert_eq!(
            data.pointer("/step/evaluation/reason")
                .and_then(Value::as_str),
            Some("frame_not_provided")
        );
    }

    #[test]
    fn session_record_step_verify_template_materializes_frame_crop() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "verify-template",
                "--step-id",
                "mail-ready",
                "--id",
                "template/mail-ready",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(
            step.exit_code(),
            0,
            "{}",
            serde_json::to_string_pretty(&step.envelope).unwrap()
        );
        let data = step.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/step/kind").and_then(Value::as_str),
            Some("verify_template")
        );
        assert_eq!(
            data.pointer("/step/artifact/width").and_then(Value::as_u64),
            Some(4)
        );
        assert_eq!(
            data.pointer("/step/artifact/height")
                .and_then(Value::as_u64),
            Some(5)
        );
        assert_eq!(
            data.pointer("/step/evaluation/status")
                .and_then(Value::as_str),
            Some("passed")
        );
        assert_eq!(
            data.pointer("/step/evaluation/backtest/passed")
                .and_then(Value::as_bool),
            Some(true)
        );
        let artifact_path = data
            .pointer("/step/artifact/path")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .expect("artifact path");
        assert!(artifact_path.exists());
    }

    #[test]
    fn session_record_amend_recomputes_frame_backed_color_probe() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "color-probe",
                "--step-id",
                "home-color",
                "--id",
                "color/home-status",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        let amend = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "amend",
                "home-color",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--region",
                "4,1,2,3",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        assert_eq!(
            amend.exit_code(),
            0,
            "{}",
            serde_json::to_string_pretty(&amend.envelope).unwrap()
        );
        let data = amend.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/step/kind").and_then(Value::as_str),
            Some("color_probe")
        );
        assert_eq!(
            data.pointer("/step/region/rect/x").and_then(Value::as_i64),
            Some(4)
        );
        assert_eq!(
            data.pointer("/step/region/rect/y").and_then(Value::as_i64),
            Some(1)
        );
        assert_eq!(
            data.pointer("/step/expected/0").and_then(Value::as_u64),
            Some(4)
        );
        assert_eq!(
            data.pointer("/step/expected/1").and_then(Value::as_u64),
            Some(2)
        );
        assert_eq!(
            data.pointer("/step/expected/2").and_then(Value::as_u64),
            Some(128)
        );
        assert_eq!(
            data.pointer("/step/evaluation/status")
                .and_then(Value::as_str),
            Some("passed")
        );
        assert_eq!(
            data.pointer("/step/evaluation/reason")
                .and_then(Value::as_str),
            Some("color_probe_sampled")
        );
    }

    #[test]
    fn session_record_amend_deferred_color_probe_does_not_fake_expected_color() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "color-probe",
                "--step-id",
                "home-color",
                "--id",
                "color/home-status",
                "--region",
                "2,3,4,5",
            ],
            true,
        );
        let amend = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "amend",
                "home-color",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--region",
                "4,1,2,3",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        assert_eq!(amend.exit_code(), 0);
        let data = amend.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/step/kind").and_then(Value::as_str),
            Some("color_probe")
        );
        assert!(data.pointer("/step/expected").is_none());
        assert_eq!(
            data.pointer("/step/evaluation/status")
                .and_then(Value::as_str),
            Some("deferred")
        );
        assert_eq!(
            data.pointer("/step/evaluation/reason")
                .and_then(Value::as_str),
            Some("amended_without_frame_provenance")
        );
    }

    #[test]
    fn session_record_amend_rebacktests_frame_backed_verify_template() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "verify-template",
                "--step-id",
                "mail-ready",
                "--id",
                "template/mail-ready",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        let amend = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "amend",
                "mail-ready",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--region",
                "1,2,3,4",
                "--threshold",
                "0.90",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        assert_eq!(
            amend.exit_code(),
            0,
            "{}",
            serde_json::to_string_pretty(&amend.envelope).unwrap()
        );
        let data = amend.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/step/kind").and_then(Value::as_str),
            Some("verify_template")
        );
        assert_eq!(
            data.pointer("/step/threshold").and_then(Value::as_f64),
            Some(0.90)
        );
        assert_eq!(
            data.pointer("/step/artifact/width").and_then(Value::as_u64),
            Some(3)
        );
        assert_eq!(
            data.pointer("/step/artifact/height")
                .and_then(Value::as_u64),
            Some(4)
        );
        assert_eq!(
            data.pointer("/step/evaluation/status")
                .and_then(Value::as_str),
            Some("passed")
        );
        assert_eq!(
            data.pointer("/step/evaluation/backtest/x")
                .and_then(Value::as_i64),
            Some(1)
        );
        assert_eq!(
            data.pointer("/step/evaluation/backtest/y")
                .and_then(Value::as_i64),
            Some(2)
        );
        assert!(
            data.pointer("/step/evaluation/backtest/threshold")
                .and_then(Value::as_f64)
                .is_some_and(|threshold| (threshold - 0.90).abs() < 0.00001)
        );
        let artifact_path = data
            .pointer("/step/artifact/path")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .expect("artifact path");
        assert!(artifact_path.is_file());
    }

    #[test]
    fn session_record_step_anchor_materializes_frame_crop() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let artifact_dir = state_dir.join("artifacts");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
                "--artifact-dir",
                artifact_dir.to_str().unwrap(),
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        let data = step.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/step/frame_provenance/source")
                .and_then(Value::as_str),
            Some("local_png")
        );
        assert_eq!(
            data.pointer("/step/frame_provenance/width")
                .and_then(Value::as_u64),
            Some(12)
        );
        assert_eq!(
            data.pointer("/step/artifact/kind").and_then(Value::as_str),
            Some("template_crop")
        );
        assert_eq!(
            data.pointer("/step/artifact/width").and_then(Value::as_u64),
            Some(4)
        );
        assert_eq!(
            data.pointer("/step/artifact/height")
                .and_then(Value::as_u64),
            Some(5)
        );
        assert_eq!(
            data.pointer("/step/evaluation/status")
                .and_then(Value::as_str),
            Some("passed")
        );
        assert_eq!(
            data.pointer("/step/evaluation/reason")
                .and_then(Value::as_str),
            Some("self_backtest_passed")
        );
        assert_eq!(
            data.pointer("/step/evaluation/backtest/source")
                .and_then(Value::as_str),
            Some("local_png_self_test")
        );
        assert_eq!(
            data.pointer("/step/evaluation/backtest/metric")
                .and_then(Value::as_str),
            Some("ccorr_normed")
        );
        assert_eq!(
            data.pointer("/step/evaluation/backtest/x")
                .and_then(Value::as_i64),
            Some(2)
        );
        assert_eq!(
            data.pointer("/step/evaluation/backtest/y")
                .and_then(Value::as_i64),
            Some(3)
        );
        assert_eq!(
            data.pointer("/step/evaluation/backtest/passed")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert!(
            data.pointer("/step/evaluation/backtest/score")
                .and_then(Value::as_f64)
                .is_some_and(|score| score >= 0.99)
        );
        assert!(
            data.pointer("/step/evaluation/backtest/threshold")
                .and_then(Value::as_f64)
                .is_some_and(|threshold| (threshold - 0.95).abs() < 0.00001)
        );
        assert!(data.pointer("/step/evaluation/contrast_backtest").is_none());
        let artifact_path = data
            .pointer("/step/artifact/path")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .expect("artifact path");
        assert!(artifact_path.exists());
        let artifact_png = fs::read(&artifact_path).unwrap();
        let artifact_frame = Frame::from_png(artifact_png, CaptureBackendName::AdbScreencap)
            .expect("artifact frame");
        assert_eq!(artifact_frame.width, 4);
        assert_eq!(artifact_frame.height, 5);
        assert_eq!(
            data.pointer("/record/steps/0/artifact/path")
                .and_then(Value::as_str),
            Some(artifact_path.to_str().unwrap())
        );
    }

    #[test]
    fn session_record_step_rejects_artifact_dir_outside_state_dir() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let escaped_artifact_dir = temp.path().join("outside-artifacts");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
                "--artifact-dir",
                escaped_artifact_dir.to_str().unwrap(),
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 3);
        assert_eq!(step.envelope.error.as_ref().unwrap().code, "path_escape");
        assert!(!escaped_artifact_dir.exists());
    }

    #[test]
    fn ensure_path_within_rejects_directory_alias_escape() {
        let temp = TempDir::new().unwrap();
        let base = temp.path().join("state");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&base).unwrap();
        fs::create_dir_all(&outside).unwrap();

        let absolute_escape =
            ensure_path_within(&base, &outside.join("artifact.png"), "test", &["record"])
                .unwrap_err();
        assert_eq!(absolute_escape.code, "path_escape");

        let link = base.join("linked-outside");
        if create_test_dir_alias(&link, &outside) {
            let linked_escape = ensure_path_within(
                &base,
                Path::new("linked-outside/artifact.png"),
                "test",
                &["record"],
            )
            .unwrap_err();
            assert_eq!(linked_escape.code, "path_escape");
            let _ = fs::remove_dir(&link);
        }
    }

    #[test]
    fn session_record_build_rejects_artifact_source_outside_state_dir() {
        let temp = TempDir::new().unwrap();
        let state_dir = temp.path().join("session");
        fs::create_dir_all(&state_dir).unwrap();
        let escaped_artifact = temp.path().join("outside.png");
        fs::write(&escaped_artifact, test_record_frame_png(4, 5)).unwrap();
        let rect = SessionRecordRect {
            x: 0,
            y: 0,
            width: 4,
            height: 5,
        };
        let record = SessionRecordContext {
            schema_version: "session-record-context-v0".to_string(),
            record_id: "record-1".to_string(),
            task_id: "daily-check".to_string(),
            instance: "fixture".to_string(),
            status: "stopped".to_string(),
            holder: None,
            lease_id: None,
            started_at_unix_ms: 1,
            updated_at_unix_ms: 2,
            steps: vec![SessionRecordStep {
                schema_version: "session-record-step-v0".to_string(),
                step_id: "home-anchor".to_string(),
                created_at_unix_ms: 1,
                updated_at_unix_ms: 2,
                data: SessionRecordStepData::Anchor {
                    id: "page/home".to_string(),
                    region: SessionRecordRegion::Rect { rect: rect.clone() },
                    color_check: false,
                    threshold: Some(0.95),
                    frame_provenance: Some(Box::new(SessionRecordFrameProvenance {
                        source: "local_png".to_string(),
                        path: escaped_artifact.display().to_string(),
                        sha256: "sha256".to_string(),
                        width: 12,
                        height: 10,
                        recorded_at_unix_ms: 1,
                        capture_backend: None,
                        freshness: None,
                        capture_attempts: Vec::new(),
                    })),
                    artifact: Some(Box::new(SessionRecordAnchorArtifact {
                        kind: "template_crop".to_string(),
                        path: escaped_artifact.display().to_string(),
                        sha256: "sha256".to_string(),
                        width: 4,
                        height: 5,
                        region: rect.clone(),
                    })),
                    evaluation: Box::new(SessionRecordStepEvaluation {
                        status: "passed".to_string(),
                        reason: "test".to_string(),
                        auto_region: None,
                        backtest: Some(SessionRecordAnchorBacktest {
                            source: "local_png_self_test".to_string(),
                            metric: "ccorr_normed".to_string(),
                            region: rect,
                            x: 0,
                            y: 0,
                            raw_score: 1.0,
                            score: 1.0,
                            threshold: 0.95,
                            passed: true,
                        }),
                        contrast_backtest: None,
                    }),
                },
            }],
        };
        let flags = FlagArgs::parse(&Vec::<String>::new()).unwrap();

        let result = session_record_build_draft(
            &record,
            &flags,
            &temp.path().join("out"),
            "sample",
            "local",
            "zh-CN",
            &state_dir,
        );
        let err = match result {
            Ok(_) => panic!("expected path_escape error"),
            Err(err) => err,
        };

        assert_eq!(err.code, "path_escape");
        assert_eq!(err.exit_code(), 3);
    }

    #[test]
    fn session_record_anchor_materializes_current_capture_source_frame_metadata() {
        let temp = TempDir::new().unwrap();
        let png = test_record_frame_png(12, 10);
        let frame =
            Frame::from_png(png.clone(), CaptureBackendName::NemuIpc).expect("source frame");
        let source_path = temp.path().join("source-frame-home.png");
        fs::write(&source_path, &png).unwrap();
        let empty_args = Vec::<String>::new();
        let flags = FlagArgs::parse(&empty_args).unwrap();
        let source_frame = SessionRecordSourceFrame {
            frame,
            png,
            source: "current_capture".to_string(),
            path: source_path.clone(),
            recorded_at_unix_ms: current_unix_ms(),
            capture_backend: Some("nemu_ipc".to_string()),
            freshness: Some(json!({
                "required": true,
                "fresh": true,
                "backend": "nemu_ipc"
            })),
            capture_attempts: vec![json!({
                "backend": "nemu_ipc",
                "ok": true,
                "message": "primed"
            })],
        };
        let materialized = materialize_anchor_artifact_from_source(
            source_frame,
            SessionRecordAnchorRegionResolution {
                rect: SessionRecordRect {
                    x: 2,
                    y: 3,
                    width: 4,
                    height: 5,
                },
                auto_region: None,
            },
            &temp.path().join("artifacts"),
            "home-anchor",
            "page/home",
            Some(0.95),
            &flags,
        )
        .expect("materialized current capture source frame");

        assert_eq!(materialized.frame_provenance.source, "current_capture");
        assert_eq!(
            materialized.frame_provenance.path,
            source_path.display().to_string()
        );
        assert_eq!(
            materialized.frame_provenance.capture_backend.as_deref(),
            Some("nemu_ipc")
        );
        assert_eq!(
            materialized
                .frame_provenance
                .freshness
                .as_ref()
                .and_then(|value| value.get("fresh"))
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(materialized.frame_provenance.capture_attempts.len(), 1);
        assert_eq!(materialized.artifact.width, 4);
        assert_eq!(materialized.artifact.height, 5);
        assert_eq!(materialized.evaluation.status, "passed");
        assert!(PathBuf::from(&materialized.artifact.path).is_file());
    }

    #[test]
    fn session_record_step_anchor_rejects_frame_and_capture_together() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--id",
                "page/home",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
                "--capture",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 2);
        assert_eq!(
            step.envelope.error.as_ref().unwrap().code,
            "validation_failed"
        );
        assert!(
            step.envelope
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("not both")
        );
    }

    #[test]
    fn session_record_step_anchor_contrast_frame_passes_when_distinct() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let contrast_path = temp.path().join("contrast.png");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        fs::write(&contrast_path, test_contrast_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
                "--contrast-frame",
                contrast_path.to_str().unwrap(),
                "--threshold",
                "0.999",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(
            step.exit_code(),
            0,
            "{}",
            serde_json::to_string_pretty(&step.envelope).unwrap()
        );
        let data = step.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/step/evaluation/status")
                .and_then(Value::as_str),
            Some("passed")
        );
        assert_eq!(
            data.pointer("/step/evaluation/reason")
                .and_then(Value::as_str),
            Some("self_and_contrast_backtest_passed")
        );
        assert_eq!(
            data.pointer("/step/evaluation/contrast_backtest/source")
                .and_then(Value::as_str),
            Some("local_png_contrast")
        );
        assert_eq!(
            data.pointer("/step/evaluation/contrast_backtest/passed")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert!(
            data.pointer("/step/evaluation/contrast_backtest/score")
                .and_then(Value::as_f64)
                .is_some_and(|score| score < 0.999)
        );
    }

    #[test]
    fn session_record_step_anchor_contrast_frame_fails_when_matching() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
                "--negative-frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        let data = step.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/step/evaluation/status")
                .and_then(Value::as_str),
            Some("failed")
        );
        assert_eq!(
            data.pointer("/step/evaluation/reason")
                .and_then(Value::as_str),
            Some("contrast_backtest_matched")
        );
        assert_eq!(
            data.pointer("/step/evaluation/backtest/passed")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/step/evaluation/contrast_backtest/passed")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert!(
            data.pointer("/step/evaluation/contrast_backtest/score")
                .and_then(Value::as_f64)
                .is_some_and(|score| score >= 0.95)
        );
    }

    #[test]
    fn session_record_step_anchor_auto_region_materializes_frame_crop() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let artifact_dir = state_dir.join("artifacts");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--id",
                "page/home",
                "--region",
                "auto",
                "--frame",
                frame_path.to_str().unwrap(),
                "--artifact-dir",
                artifact_dir.to_str().unwrap(),
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        let data = step.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/step/region/mode").and_then(Value::as_str),
            Some("rect")
        );
        assert!(
            data.pointer("/step/region/rect/width")
                .and_then(Value::as_i64)
                .is_some_and(|width| width > 0 && width <= 12)
        );
        assert!(
            data.pointer("/step/region/rect/height")
                .and_then(Value::as_i64)
                .is_some_and(|height| height > 0 && height <= 10)
        );
        assert_eq!(
            data.pointer("/step/artifact/kind").and_then(Value::as_str),
            Some("template_crop")
        );
        assert_eq!(
            data.pointer("/step/evaluation/status")
                .and_then(Value::as_str),
            Some("passed")
        );
        let artifact_path = data
            .pointer("/step/artifact/path")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .expect("artifact path");
        assert!(artifact_path.exists());
    }

    #[test]
    fn session_record_step_anchor_auto_region_prefers_contrast_rejected_candidate() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let contrast_path = temp.path().join("contrast.png");
        fs::write(
            &frame_path,
            test_auto_region_discrimination_frame_png(false),
        )
        .unwrap();
        fs::write(
            &contrast_path,
            test_auto_region_discrimination_frame_png(true),
        )
        .unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--id",
                "page/home",
                "--region",
                "auto",
                "--frame",
                frame_path.to_str().unwrap(),
                "--contrast-frame",
                contrast_path.to_str().unwrap(),
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        let data = step.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/step/evaluation/auto_region/selected_reason")
                .and_then(Value::as_str),
            Some("contrast_rejected_highest_variance")
        );
        assert_eq!(
            data.pointer("/step/evaluation/auto_region/selected/x")
                .and_then(Value::as_i64),
            Some(4)
        );
        assert_eq!(
            data.pointer("/step/evaluation/auto_region/selected/y")
                .and_then(Value::as_i64),
            Some(3)
        );
        assert_eq!(
            data.pointer("/step/region/rect/x").and_then(Value::as_i64),
            Some(4)
        );
        assert_eq!(
            data.pointer("/step/region/rect/y").and_then(Value::as_i64),
            Some(3)
        );
        let candidates = data
            .pointer("/step/evaluation/auto_region/candidates")
            .and_then(Value::as_array)
            .expect("auto-region candidates");
        assert_eq!(candidates.len(), 9);
        assert_eq!(
            candidates
                .iter()
                .filter(
                    |candidate| candidate.get("selected").and_then(Value::as_bool) == Some(true)
                )
                .count(),
            1
        );
        assert!(candidates.iter().any(|candidate| {
            candidate.get("contrast_passed").and_then(Value::as_bool) == Some(true)
        }));
        assert!(candidates.iter().any(|candidate| {
            candidate.get("contrast_passed").and_then(Value::as_bool) == Some(false)
        }));
        assert_eq!(
            data.pointer("/step/evaluation/contrast_backtest/passed")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/step/evaluation/status")
                .and_then(Value::as_str),
            Some("passed")
        );
    }

    #[test]
    fn session_record_candidates_lists_auto_region_report() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let contrast_path = temp.path().join("contrast.png");
        fs::write(
            &frame_path,
            test_auto_region_discrimination_frame_png(false),
        )
        .unwrap();
        fs::write(
            &contrast_path,
            test_auto_region_discrimination_frame_png(true),
        )
        .unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "auto",
                "--frame",
                frame_path.to_str().unwrap(),
                "--contrast-frame",
                contrast_path.to_str().unwrap(),
            ],
            true,
        );
        let candidates = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "candidates",
                "home-anchor",
                "--state-dir",
                state_dir.to_str().unwrap(),
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        assert_eq!(candidates.exit_code(), 0);
        let data = candidates.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/status").and_then(Value::as_str),
            Some("candidates_listed")
        );
        assert_eq!(
            data.pointer("/step_id").and_then(Value::as_str),
            Some("home-anchor")
        );
        assert_eq!(
            data.pointer("/candidate_count").and_then(Value::as_u64),
            Some(9)
        );
        assert_eq!(
            data.pointer("/selected_index").and_then(Value::as_u64),
            Some(4)
        );
        assert_eq!(
            data.pointer("/auto_region/selected_reason")
                .and_then(Value::as_str),
            Some("contrast_rejected_highest_variance")
        );
        assert_eq!(
            data.pointer("/auto_region/selected/x")
                .and_then(Value::as_i64),
            Some(4)
        );
        assert_eq!(
            data.pointer("/auto_region/selected/y")
                .and_then(Value::as_i64),
            Some(3)
        );
    }

    #[test]
    fn session_record_candidates_lists_color_probe_auto_region_report() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        fs::write(
            &frame_path,
            test_auto_region_discrimination_frame_png(false),
        )
        .unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "color-probe",
                "--step-id",
                "home-color",
                "--id",
                "color/home-status",
                "--region",
                "auto",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        let candidates = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "candidates",
                "home-color",
                "--state-dir",
                state_dir.to_str().unwrap(),
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        assert_eq!(
            candidates.exit_code(),
            0,
            "{}",
            serde_json::to_string_pretty(&candidates.envelope).unwrap()
        );
        let data = candidates.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/resource_kind").and_then(Value::as_str),
            Some("color_probe")
        );
        assert_eq!(
            data.pointer("/resource_id").and_then(Value::as_str),
            Some("color/home-status")
        );
        assert_eq!(
            data.pointer("/anchor_id").and_then(Value::as_str),
            Some("color/home-status")
        );
        assert!(
            data.pointer("/candidate_count")
                .and_then(Value::as_u64)
                .is_some_and(|count| count > 0)
        );
    }

    #[test]
    fn session_record_candidates_requires_auto_region_report() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        let candidates = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "candidates",
                "home-anchor",
                "--state-dir",
                state_dir.to_str().unwrap(),
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        assert_eq!(candidates.exit_code(), 2);
        assert_eq!(
            candidates.envelope.error.as_ref().unwrap().code,
            "validation_failed"
        );
        assert!(
            candidates
                .envelope
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("auto-region candidate report")
        );
    }

    #[test]
    fn session_record_step_anchor_auto_without_frame_stays_deferred() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--id",
                "page/home",
                "--region",
                "auto",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        let data = step.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/step/region/mode").and_then(Value::as_str),
            Some("auto")
        );
        assert!(data.pointer("/step/artifact").is_none());
        assert_eq!(
            data.pointer("/step/evaluation/status")
                .and_then(Value::as_str),
            Some("deferred")
        );
        assert_eq!(
            data.pointer("/step/evaluation/reason")
                .and_then(Value::as_str),
            Some("frame_not_provided")
        );
    }

    #[test]
    fn session_record_step_anchor_rejects_out_of_bounds_frame_crop() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--id",
                "page/home",
                "--region",
                "10,8,4,4",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 2);
        assert_eq!(
            step.envelope.error.as_ref().unwrap().code,
            "validation_failed"
        );
        assert!(
            step.envelope
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("exceeds frame")
        );
    }

    #[test]
    fn session_record_step_operation_records_coord_click() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "operation",
                "--from",
                "page/home",
                "--to",
                "page/mail",
                "--click",
                "100,200",
                "--destructive",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        let data = step.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/step/kind").and_then(Value::as_str),
            Some("operation")
        );
        assert_eq!(
            data.pointer("/step/from").and_then(Value::as_str),
            Some("page/home")
        );
        assert_eq!(
            data.pointer("/step/to").and_then(Value::as_str),
            Some("page/mail")
        );
        assert_eq!(
            data.pointer("/step/click/type").and_then(Value::as_str),
            Some("coord")
        );
        assert_eq!(
            data.pointer("/step/click/x").and_then(Value::as_i64),
            Some(100)
        );
        assert_eq!(
            data.pointer("/step/destructive").and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn session_record_build_task_writes_draft_bundle() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let out = temp.path().join("draft");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let anchor = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
                "--color-check",
            ],
            true,
        );
        let mail_anchor = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "mail-anchor",
                "--id",
                "page/mail",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        let color_probe = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "color-probe",
                "--step-id",
                "home-color",
                "--id",
                "color/home-status",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        let verify_template = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "verify-template",
                "--step-id",
                "mail-ready",
                "--id",
                "template/mail-ready",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        let operation = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "operation",
                "--step-id",
                "home-to-mail",
                "--from",
                "page/home",
                "--to",
                "page/mail",
                "--click",
                "5,6",
            ],
            true,
        );
        let swipe = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "operation",
                "--step-id",
                "mail-swipe-home",
                "--from",
                "page/mail",
                "--to",
                "page/home",
                "--swipe",
                "3,4,2,2->7,8,2,2",
                "--duration-ms",
                "650",
            ],
            true,
        );
        let long_press = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "operation",
                "--step-id",
                "home-long-press",
                "--from",
                "page/home",
                "--long-press",
                "6,7",
                "--duration-ms",
                "900",
            ],
            true,
        );
        let build = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "build-task",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
                "--game",
                "sample",
                "--server",
                "local",
                "--locale",
                "zh-CN",
                "--client-version",
                "record-test-client",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(anchor.exit_code(), 0);
        assert_eq!(mail_anchor.exit_code(), 0);
        assert_eq!(
            color_probe.exit_code(),
            0,
            "{}",
            serde_json::to_string_pretty(&color_probe.envelope).unwrap()
        );
        assert_eq!(
            verify_template.exit_code(),
            0,
            "{}",
            serde_json::to_string_pretty(&verify_template.envelope).unwrap()
        );
        assert_eq!(
            operation.exit_code(),
            0,
            "{}",
            serde_json::to_string_pretty(&operation.envelope).unwrap()
        );
        assert_eq!(
            swipe.exit_code(),
            0,
            "{}",
            serde_json::to_string_pretty(&swipe.envelope).unwrap()
        );
        assert_eq!(
            long_press.exit_code(),
            0,
            "{}",
            serde_json::to_string_pretty(&long_press.envelope).unwrap()
        );
        assert_eq!(
            build.exit_code(),
            0,
            "{}",
            serde_json::to_string_pretty(&build.envelope).unwrap()
        );
        let data = build.envelope.data.as_ref().unwrap();
        assert_eq!(data.get("status").and_then(Value::as_str), Some("built"));
        assert_eq!(data.get("anchor_count").and_then(Value::as_u64), Some(2));
        assert_eq!(
            data.get("color_probe_count").and_then(Value::as_u64),
            Some(1)
        );
        assert_eq!(
            data.get("verify_template_count").and_then(Value::as_u64),
            Some(1)
        );
        assert_eq!(data.get("operation_count").and_then(Value::as_u64), Some(3));
        assert_eq!(
            data.pointer("/bundle/schema_version")
                .and_then(Value::as_str),
            Some("0.5")
        );
        assert_eq!(
            data.pointer("/bundle/task_id").and_then(Value::as_str),
            Some("daily-check")
        );
        assert_eq!(
            data.pointer("/bundle/game").and_then(Value::as_str),
            Some("sample")
        );
        assert_eq!(
            data.pointer("/bundle/server_scope/0")
                .and_then(Value::as_str),
            Some("local")
        );
        assert_eq!(
            data.pointer("/bundle/coordinate_space/width")
                .and_then(Value::as_u64),
            Some(12)
        );
        assert_eq!(
            data.pointer("/bundle/provenance/game")
                .and_then(Value::as_str),
            Some("sample")
        );
        assert_eq!(
            data.pointer("/bundle/provenance/server")
                .and_then(Value::as_str),
            Some("local")
        );
        assert_eq!(
            data.pointer("/bundle/provenance/resolution/height")
                .and_then(Value::as_u64),
            Some(10)
        );
        assert_eq!(
            data.pointer("/bundle/provenance/client_version")
                .and_then(Value::as_str),
            Some("record-test-client")
        );
        assert_eq!(
            data.pointer("/bundle/anchors/0/template")
                .and_then(Value::as_str),
            Some("assets/anchor-home-anchor-page_home.png")
        );
        assert_eq!(
            data.pointer("/bundle/anchors/0/color_check/region/rect/x")
                .and_then(Value::as_i64),
            Some(2)
        );
        assert_eq!(
            data.pointer("/bundle/anchors/0/color_check/expected/0")
                .and_then(Value::as_u64),
            Some(3)
        );
        assert_eq!(
            data.pointer("/bundle/anchors/0/color_check/expected/1")
                .and_then(Value::as_u64),
            Some(5)
        );
        assert_eq!(
            data.pointer("/bundle/anchors/0/color_check/expected/2")
                .and_then(Value::as_u64),
            Some(128)
        );
        assert_eq!(
            data.pointer("/bundle/color_probes/0/id")
                .and_then(Value::as_str),
            Some("color/home-status")
        );
        assert_eq!(
            data.pointer("/bundle/color_probes/0/expected/0")
                .and_then(Value::as_u64),
            Some(3)
        );
        assert_eq!(
            data.pointer("/bundle/color_probes/0/expected/1")
                .and_then(Value::as_u64),
            Some(5)
        );
        assert_eq!(
            data.pointer("/bundle/color_probes/0/expected/2")
                .and_then(Value::as_u64),
            Some(128)
        );
        assert_eq!(
            data.pointer("/bundle/verify_templates/0/id")
                .and_then(Value::as_str),
            Some("template/mail-ready")
        );
        assert_eq!(
            data.pointer("/bundle/verify_templates/0/template")
                .and_then(Value::as_str),
            Some("assets/verify-template-mail-ready-template_mail-ready.png")
        );
        assert_eq!(
            data.pointer("/bundle/verify_templates/0/region/rect/x")
                .and_then(Value::as_i64),
            Some(2)
        );
        assert_eq!(
            data.pointer("/bundle/operations/0/click/kind")
                .and_then(Value::as_str),
            Some("point")
        );
        assert_eq!(
            data.pointer("/bundle/operations/0/click/x")
                .and_then(Value::as_i64),
            Some(5)
        );
        assert_eq!(
            data.pointer("/bundle/operations/0/guard/page_id")
                .and_then(Value::as_str),
            Some("page/home")
        );
        assert_eq!(
            data.pointer("/bundle/operations/0/guard/target_id")
                .and_then(Value::as_str),
            Some("page/page/home")
        );
        assert_eq!(
            data.pointer("/bundle/operations/0/guard/expected_rect/x")
                .and_then(Value::as_i64),
            Some(5)
        );
        assert_eq!(
            data.pointer("/bundle/operations/1/click/kind")
                .and_then(Value::as_str),
            Some("drag")
        );
        assert_eq!(
            data.pointer("/bundle/operations/1/click/from/x")
                .and_then(Value::as_i64),
            Some(3)
        );
        assert_eq!(
            data.pointer("/bundle/operations/1/click/to/y")
                .and_then(Value::as_i64),
            Some(8)
        );
        assert_eq!(
            data.pointer("/bundle/operations/1/click/duration_ms")
                .and_then(Value::as_u64),
            Some(650)
        );
        assert_eq!(
            data.pointer("/bundle/operations/1/guard/expected_rect/x")
                .and_then(Value::as_i64),
            Some(3)
        );
        assert_eq!(
            data.pointer("/bundle/operations/2/click/kind")
                .and_then(Value::as_str),
            Some("long_press")
        );
        assert_eq!(
            data.pointer("/bundle/operations/2/click/duration_ms")
                .and_then(Value::as_u64),
            Some(900)
        );
        assert!(out.join("operations/resources.json").is_file());
        assert!(out.join("operations/daily-check/task.json").is_file());
        assert!(
            out.join("operations/daily-check/assets/anchor-home-anchor-page_home.png")
                .is_file()
        );
        assert!(
            out.join("operations/daily-check/assets/anchor-mail-anchor-page_mail.png")
                .is_file()
        );
        assert!(
            out.join(
                "operations/daily-check/assets/verify-template-mail-ready-template_mail-ready.png"
            )
            .is_file()
        );
        let written: Value = serde_json::from_str(
            &fs::read_to_string(out.join("operations/daily-check/task.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            written.pointer("/operations/0/id").and_then(Value::as_str),
            Some("home-to-mail")
        );
        assert_eq!(
            written
                .pointer("/operations/1/click/kind")
                .and_then(Value::as_str),
            Some("drag")
        );
        assert_eq!(
            written
                .pointer("/operations/2/click/kind")
                .and_then(Value::as_str),
            Some("long_press")
        );
        assert_eq!(
            written
                .pointer("/anchors/0/color_check/expected/0")
                .and_then(Value::as_u64),
            Some(3)
        );
        assert_eq!(
            written
                .pointer("/color_probes/0/expected/0")
                .and_then(Value::as_u64),
            Some(3)
        );
        assert_eq!(
            written
                .pointer("/verify_templates/0/template")
                .and_then(Value::as_str),
            Some("assets/verify-template-mail-ready-template_mail-ready.png")
        );

        let packaged = run_cli(
            [
                "--json",
                "package",
                "build-task",
                "--repo",
                out.to_str().unwrap(),
                "--task",
                "daily-check",
                "--out",
                temp.path().join("daily-check.zip").to_str().unwrap(),
                "--dry-run",
            ],
            true,
        );
        assert_eq!(
            packaged.exit_code(),
            0,
            "{}",
            serde_json::to_string_pretty(&packaged.envelope).unwrap()
        );
        let packaged_data = packaged.envelope.data.as_ref().unwrap();
        assert_eq!(
            packaged_data.get("status").and_then(Value::as_str),
            Some("validated")
        );
        assert_eq!(
            packaged_data.get("task_id").and_then(Value::as_str),
            Some("daily-check")
        );
        let converted = run_cli(
            [
                "--json",
                "resource",
                "convert",
                "--repo",
                out.to_str().unwrap(),
            ],
            true,
        );
        assert_eq!(
            converted.exit_code(),
            0,
            "{}",
            serde_json::to_string_pretty(&converted.envelope).unwrap()
        );
        let navigation: Value = serde_json::from_str(
            &fs::read_to_string(out.join("navigation/sample.local.navigation.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            navigation
                .pointer("/navigation/1/id")
                .and_then(Value::as_str),
            Some("mail-swipe-home")
        );
        assert_eq!(
            navigation
                .pointer("/navigation/1/click/kind")
                .and_then(Value::as_str),
            Some("drag")
        );
    }

    #[test]
    fn session_record_build_task_rejects_deferred_color_probe() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let out = temp.path().join("draft");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let home_anchor = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        let mail_anchor = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "mail-anchor",
                "--id",
                "page/mail",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        let color_probe = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "color-probe",
                "--step-id",
                "home-color",
                "--id",
                "color/home-status",
                "--region",
                "2,3,4,5",
            ],
            true,
        );
        let operation = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "operation",
                "--step-id",
                "home-to-mail",
                "--from",
                "page/home",
                "--to",
                "page/mail",
                "--click",
                "5,6",
            ],
            true,
        );
        let build = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "build-task",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
                "--game",
                "sample",
                "--server",
                "local",
                "--locale",
                "zh-CN",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(home_anchor.exit_code(), 0);
        assert_eq!(mail_anchor.exit_code(), 0);
        assert_eq!(color_probe.exit_code(), 0);
        assert_eq!(operation.exit_code(), 0);
        assert_ne!(build.exit_code(), 0);
        let error = build.envelope.error.as_ref().expect("build error");
        assert!(
            error.message.contains("without expected color"),
            "{}",
            error.message
        );
    }

    #[test]
    fn session_record_build_task_rejects_deferred_verify_template() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let out = temp.path().join("draft");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let home_anchor = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        let mail_anchor = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "mail-anchor",
                "--id",
                "page/mail",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        let verify_template = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "verify-template",
                "--step-id",
                "mail-ready",
                "--id",
                "template/mail-ready",
                "--region",
                "2,3,4,5",
            ],
            true,
        );
        let operation = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "operation",
                "--step-id",
                "home-to-mail",
                "--from",
                "page/home",
                "--to",
                "page/mail",
                "--click",
                "5,6",
            ],
            true,
        );
        let build = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "build-task",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
                "--game",
                "sample",
                "--server",
                "local",
                "--locale",
                "zh-CN",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(home_anchor.exit_code(), 0);
        assert_eq!(mail_anchor.exit_code(), 0);
        assert_eq!(verify_template.exit_code(), 0);
        assert_eq!(operation.exit_code(), 0);
        assert_ne!(build.exit_code(), 0);
        let error = build.envelope.error.as_ref().expect("build error");
        assert!(
            error.message.contains("without a frame artifact"),
            "{}",
            error.message
        );
    }

    #[test]
    fn session_record_promote_writes_repo_ours_and_guards_overwrite() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let runtime_root = temp.path().join("runtime");
        let _runtime_env = use_runtime_state_root(&runtime_root);
        let host = start_authoring_runtime(&runtime_root);
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let repo = temp.path().join("resource-repo");
        let ours = repo.join("ours");
        let resources_path = ours.join("operations/resources.json");
        fs::create_dir_all(ours.join("operations")).unwrap();
        fs::create_dir_all(ours.join("recognition")).unwrap();
        fs::write(
            &resources_path,
            r#"{"schema_version":"1.0","resources":[{"id":"keep"}],"resource_count":1}"#,
        )
        .unwrap();
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let home_anchor = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        let mail_anchor = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "mail-anchor",
                "--id",
                "page/mail",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        let operation = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "operation",
                "--step-id",
                "home-to-mail",
                "--from",
                "page/home",
                "--to",
                "page/mail",
                "--click",
                "5,6",
            ],
            true,
        );
        let promote = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "promote",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--repo",
                repo.to_str().unwrap(),
                "--game",
                "sample",
                "--server",
                "local",
                "--locale",
                "zh-CN",
            ],
            true,
        );
        let reject = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "promote",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--repo",
                repo.to_str().unwrap(),
                "--game",
                "sample",
                "--server",
                "local",
                "--locale",
                "zh-CN",
            ],
            true,
        );
        fs::write(
            ours.join("operations/daily-check/obsolete.txt"),
            "stale task file",
        )
        .unwrap();
        let forced = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "promote",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--repo",
                repo.to_str().unwrap(),
                "--game",
                "sample",
                "--server",
                "local",
                "--locale",
                "zh-CN",
                "--force",
            ],
            true,
        );
        let packaged = run_cli(
            [
                "--json",
                "package",
                "build-task",
                "--repo",
                repo.to_str().unwrap(),
                "--task",
                "daily-check",
                "--out",
                temp.path().join("daily-check.zip").to_str().unwrap(),
                "--dry-run",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(home_anchor.exit_code(), 0);
        assert_eq!(mail_anchor.exit_code(), 0);
        assert_eq!(operation.exit_code(), 0);
        assert_eq!(
            promote.exit_code(),
            0,
            "{}",
            serde_json::to_string_pretty(&promote.envelope).unwrap()
        );
        let data = promote.envelope.data.as_ref().unwrap();
        assert_eq!(data.get("status").and_then(Value::as_str), Some("promoted"));
        assert_eq!(
            data.get("resource_layout").and_then(Value::as_str),
            Some("repo_ours")
        );
        assert_eq!(
            data.get("resources_action").and_then(Value::as_str),
            Some("preserved")
        );
        assert!(
            data.pointer("/authoring/runtime_correlation_id")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty())
        );
        assert_eq!(
            data.pointer("/authoring/receipt/validation/checks")
                .and_then(Value::as_array)
                .map(|checks| checks.iter().filter_map(Value::as_str).collect::<Vec<_>>()),
            Some(vec![
                "draft_schema",
                "resource_convert",
                "repository_references",
                "package_build",
                "containment_round_trip"
            ])
        );
        assert!(ours.join("operations/daily-check/task.json").is_file());
        assert!(
            ours.join("operations/daily-check/assets/anchor-home-anchor-page_home.png")
                .is_file()
        );
        for generated in [
            "recognition/sample.local.pack.json",
            "recognition/sample.local.pages.json",
            "navigation/sample.local.navigation.json",
            "operations/operations.index.json",
            "operations/operations.primitives.json",
        ] {
            assert!(ours.join(generated).is_file(), "missing {generated}");
        }
        let resources: Value =
            serde_json::from_str(&fs::read_to_string(&resources_path).unwrap()).unwrap();
        assert_eq!(
            resources.pointer("/resources/0/id").and_then(Value::as_str),
            Some("keep")
        );
        assert_eq!(reject.exit_code(), 3);
        assert_eq!(
            reject.envelope.error.as_ref().unwrap().code,
            "record_promote_target_exists"
        );
        assert_eq!(
            forced.exit_code(),
            0,
            "{}",
            serde_json::to_string_pretty(&forced.envelope).unwrap()
        );
        assert!(!ours.join("operations/daily-check/obsolete.txt").exists());
        assert_eq!(
            serde_json::from_str::<Value>(&fs::read_to_string(&resources_path).unwrap())
                .unwrap()
                .pointer("/resources/0/id")
                .and_then(Value::as_str),
            Some("keep")
        );
        assert_eq!(
            packaged.exit_code(),
            0,
            "{}",
            serde_json::to_string_pretty(&packaged.envelope).unwrap()
        );
        assert_eq!(
            packaged
                .envelope
                .data
                .as_ref()
                .unwrap()
                .get("status")
                .and_then(Value::as_str),
            Some("validated")
        );
        host.close().expect("close Runtime host");
    }

    #[test]
    fn session_record_promote_requires_runtime_before_mutating_target() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let runtime_root = temp.path().join("missing-runtime");
        let _runtime_env = use_runtime_state_root(&runtime_root);
        let repo = temp.path().join("resource-repo");
        let ours = repo.join("ours");
        fs::create_dir_all(ours.join("operations")).unwrap();
        fs::create_dir_all(ours.join("recognition")).unwrap();
        prepare_promotable_record(&config, &state_dir, &frame_path);

        let promote = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "promote",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--repo",
                repo.to_str().unwrap(),
                "--game",
                "sample",
                "--server",
                "local",
                "--locale",
                "zh-CN",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(promote.exit_code(), 5);
        assert_eq!(
            promote.envelope.error.as_ref().unwrap().code,
            "runtime_not_running"
        );
        assert!(!ours.join("operations/daily-check").exists());
        assert!(!ours.join("operations/resources.json").exists());
        assert!(!ours.join("recognition/sample.local.pack.json").exists());
    }

    #[test]
    fn session_record_promote_validation_failure_rolls_back_canonical_tree() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let runtime_root = temp.path().join("runtime");
        let _runtime_env = use_runtime_state_root(&runtime_root);
        let host = start_authoring_runtime(&runtime_root);
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let repo = temp.path().join("resource-repo");
        let ours = repo.join("ours");
        let existing_task = ours.join("operations/daily-check");
        let broken_task = ours.join("operations/broken");
        fs::create_dir_all(&existing_task).unwrap();
        fs::create_dir_all(&broken_task).unwrap();
        fs::create_dir_all(ours.join("recognition")).unwrap();
        fs::write(existing_task.join("sentinel.txt"), "canonical-before").unwrap();
        fs::write(broken_task.join("task.json"), "{not-json").unwrap();
        fs::write(
            ours.join("operations/resources.json"),
            r#"{"schema_version":"1.0","resources":[],"resource_count":0}"#,
        )
        .unwrap();
        prepare_promotable_record(&config, &state_dir, &frame_path);

        let promote = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "promote",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--repo",
                repo.to_str().unwrap(),
                "--game",
                "sample",
                "--server",
                "local",
                "--locale",
                "zh-CN",
                "--force",
            ],
            true,
        );
        set_missing_config_env();

        assert_ne!(promote.exit_code(), 0);
        assert_eq!(
            fs::read_to_string(existing_task.join("sentinel.txt")).unwrap(),
            "canonical-before"
        );
        assert!(!existing_task.join("task.json").exists());
        assert_eq!(
            fs::read_to_string(broken_task.join("task.json")).unwrap(),
            "{not-json"
        );
        assert!(!ours.join("recognition/sample.local.pack.json").exists());
        host.close().expect("close Runtime host");
    }

    #[test]
    fn session_record_build_task_rejects_unresolved_target_click() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let out = temp.path().join("draft");
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let operation = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "operation",
                "--step-id",
                "open-mail",
                "--from",
                "page/home",
                "--to",
                "page/mail",
                "--click",
                "mail_button",
            ],
            true,
        );
        let build = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "build-task",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
                "--game",
                "sample",
                "--server",
                "local",
                "--locale",
                "zh-CN",
                "--resolution",
                "1280x720",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(operation.exit_code(), 0);
        assert_eq!(build.exit_code(), 2);
        assert_eq!(
            build.envelope.error.as_ref().unwrap().code,
            "validation_failed"
        );
        assert!(
            build
                .envelope
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("unresolved target click")
        );
    }

    #[test]
    fn session_record_build_task_rejects_missing_page_anchor() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let out = temp.path().join("draft");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let anchor = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        let operation = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "operation",
                "--step-id",
                "missing-mail-anchor",
                "--from",
                "page/home",
                "--to",
                "page/mail",
                "--click",
                "5,6",
            ],
            true,
        );
        let build = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "build-task",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
                "--game",
                "sample",
                "--server",
                "local",
                "--locale",
                "zh-CN",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(anchor.exit_code(), 0);
        assert_eq!(operation.exit_code(), 0);
        assert_eq!(build.exit_code(), 2);
        assert_eq!(
            build.envelope.error.as_ref().unwrap().code,
            "validation_failed"
        );
        assert!(
            build
                .envelope
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("has no matching anchor")
        );
    }

    #[test]
    fn session_record_build_task_rejects_out_of_bounds_click() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let out = temp.path().join("draft");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let anchor = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        let operation = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "operation",
                "--step-id",
                "bad-click",
                "--from",
                "page/home",
                "--to",
                "page/home",
                "--click",
                "100,200",
            ],
            true,
        );
        let build = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "build-task",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
                "--game",
                "sample",
                "--server",
                "local",
                "--locale",
                "zh-CN",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(anchor.exit_code(), 0);
        assert_eq!(operation.exit_code(), 0);
        assert_eq!(build.exit_code(), 2);
        assert_eq!(
            build.envelope.error.as_ref().unwrap().code,
            "validation_failed"
        );
        assert!(
            build
                .envelope
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("outside coordinate_space"),
            "{}",
            serde_json::to_string_pretty(&build.envelope).unwrap()
        );
    }

    #[test]
    fn session_record_step_requires_active_record() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        set_config_env(&config);

        let result = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--id",
                "page/home",
                "--region",
                "auto",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(result.exit_code(), 3);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "record_session_not_active"
        );
    }

    #[test]
    fn session_record_step_rejects_duplicate_step_id() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let first = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "auto",
            ],
            true,
        );
        let duplicate = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "operation",
                "--step-id",
                "home-anchor",
                "--from",
                "page/home",
                "--to",
                "null",
                "--click",
                "mail_button",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(first.exit_code(), 0);
        assert_eq!(duplicate.exit_code(), 3);
        assert_eq!(
            duplicate.envelope.error.as_ref().unwrap().code,
            "record_step_id_conflict"
        );
    }

    #[test]
    fn session_record_amend_updates_anchor_metadata() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "10,20,30,40",
                "--color-check",
                "--threshold",
                "0.96",
            ],
            true,
        );
        let amend = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "amend",
                "home-anchor",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--region",
                "auto",
                "--no-color-check",
                "--clear-threshold",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        assert_eq!(amend.exit_code(), 0);
        let data = amend.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.get("status").and_then(Value::as_str),
            Some("step_amended")
        );
        assert_eq!(
            data.pointer("/step/region/mode").and_then(Value::as_str),
            Some("auto")
        );
        assert_eq!(
            data.pointer("/step/color_check").and_then(Value::as_bool),
            Some(false)
        );
        assert!(data.pointer("/step/threshold").is_some_and(Value::is_null));
        assert_eq!(
            data.pointer("/step/evaluation/reason")
                .and_then(Value::as_str),
            Some("amended_without_frame_provenance")
        );
    }

    #[test]
    fn session_record_amend_rebacktests_frame_backed_anchor() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let artifact_dir = state_dir.join("artifacts");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
                "--artifact-dir",
                artifact_dir.to_str().unwrap(),
            ],
            true,
        );
        let amend = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "amend",
                "home-anchor",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--region",
                "1,2,3,4",
                "--threshold",
                "0.90",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        assert_eq!(
            amend.exit_code(),
            0,
            "{}",
            serde_json::to_string_pretty(&amend.envelope).unwrap()
        );
        let data = amend.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/step/evaluation/status")
                .and_then(Value::as_str),
            Some("passed")
        );
        assert_eq!(
            data.pointer("/step/evaluation/reason")
                .and_then(Value::as_str),
            Some("self_backtest_passed")
        );
        assert_eq!(
            data.pointer("/step/evaluation/backtest/x")
                .and_then(Value::as_i64),
            Some(1)
        );
        assert_eq!(
            data.pointer("/step/evaluation/backtest/y")
                .and_then(Value::as_i64),
            Some(2)
        );
        assert!(
            data.pointer("/step/evaluation/backtest/threshold")
                .and_then(Value::as_f64)
                .is_some_and(|threshold| (threshold - 0.90).abs() < 0.00001)
        );
        assert_eq!(
            data.pointer("/step/artifact/width").and_then(Value::as_u64),
            Some(3)
        );
        assert_eq!(
            data.pointer("/step/artifact/height")
                .and_then(Value::as_u64),
            Some(4)
        );
        assert_eq!(
            data.pointer("/step/frame_provenance/path")
                .and_then(Value::as_str),
            Some(frame_path.to_str().unwrap())
        );
        let artifact_path = data
            .pointer("/step/artifact/path")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .expect("artifact path");
        assert!(artifact_path.is_file());
    }

    #[test]
    fn session_record_amend_selects_auto_region_candidate_and_rebacktests() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let contrast_path = temp.path().join("contrast.png");
        fs::write(
            &frame_path,
            test_auto_region_discrimination_frame_png(false),
        )
        .unwrap();
        fs::write(
            &contrast_path,
            test_auto_region_discrimination_frame_png(true),
        )
        .unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "auto",
                "--frame",
                frame_path.to_str().unwrap(),
                "--contrast-frame",
                contrast_path.to_str().unwrap(),
            ],
            true,
        );
        let amend = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "amend",
                "home-anchor",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--candidate-index",
                "0",
                "--contrast-frame",
                contrast_path.to_str().unwrap(),
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        assert_eq!(amend.exit_code(), 0);
        let data = amend.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/step/region/rect/x").and_then(Value::as_i64),
            Some(0)
        );
        assert_eq!(
            data.pointer("/step/region/rect/y").and_then(Value::as_i64),
            Some(0)
        );
        assert_eq!(
            data.pointer("/step/evaluation/auto_region/selected_reason")
                .and_then(Value::as_str),
            Some("operator_selected_candidate")
        );
        assert_eq!(
            data.pointer("/step/evaluation/auto_region/selected/x")
                .and_then(Value::as_i64),
            Some(0)
        );
        assert_eq!(
            data.pointer("/step/evaluation/auto_region/selected/y")
                .and_then(Value::as_i64),
            Some(0)
        );
        let candidates = data
            .pointer("/step/evaluation/auto_region/candidates")
            .and_then(Value::as_array)
            .expect("auto-region candidates");
        assert_eq!(
            candidates
                .iter()
                .filter(
                    |candidate| candidate.get("selected").and_then(Value::as_bool) == Some(true)
                )
                .count(),
            1
        );
        assert_eq!(
            candidates
                .first()
                .and_then(|candidate| candidate.get("selected"))
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/step/evaluation/status")
                .and_then(Value::as_str),
            Some("failed")
        );
        assert_eq!(
            data.pointer("/step/evaluation/reason")
                .and_then(Value::as_str),
            Some("contrast_backtest_matched")
        );
        assert_eq!(
            data.pointer("/step/evaluation/contrast_backtest/passed")
                .and_then(Value::as_bool),
            Some(false)
        );
    }

    #[test]
    fn session_record_amend_candidate_index_requires_auto_region_report() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        let amend = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "amend",
                "home-anchor",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--candidate-index",
                "0",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        assert_eq!(amend.exit_code(), 2);
        assert_eq!(
            amend.envelope.error.as_ref().unwrap().code,
            "validation_failed"
        );
        assert!(
            amend
                .envelope
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("auto-region candidate report")
        );
    }

    #[test]
    fn session_record_amend_updates_operation_metadata() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "operation",
                "--step-id",
                "open-mail",
                "--from",
                "page/home",
                "--to",
                "page/mail",
                "--click",
                "100,200",
                "--destructive",
            ],
            true,
        );
        let amend = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "amend",
                "--step-id",
                "open-mail",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--to",
                "null",
                "--click",
                "mail_button",
                "--non-destructive",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        assert_eq!(amend.exit_code(), 0);
        let data = amend.envelope.data.as_ref().unwrap();
        assert!(data.pointer("/step/to").is_some_and(Value::is_null));
        assert_eq!(
            data.pointer("/step/click/type").and_then(Value::as_str),
            Some("target")
        );
        assert_eq!(
            data.pointer("/step/click/target").and_then(Value::as_str),
            Some("mail_button")
        );
        assert_eq!(
            data.pointer("/step/destructive").and_then(Value::as_bool),
            Some(false)
        );
    }

    #[test]
    fn session_record_amend_requires_supported_field() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "auto",
            ],
            true,
        );
        let amend = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "amend",
                "home-anchor",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--from",
                "page/other",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        assert_eq!(amend.exit_code(), 2);
        assert_eq!(
            amend.envelope.error.as_ref().unwrap().code,
            "validation_failed"
        );
    }

    #[test]
    fn drift_diagnostics_contract_rejects_fields_outside_amend_whitelist() {
        let diagnostics = json!({
            "trigger": "resource_drift",
            "target_id": "page/home",
            "measured": {
                "matched_rect": {"x": 1, "y": 2, "width": 3, "height": 4}
            },
            "proposed_changes": {
                "region": {"x": 1, "y": 2, "width": 3, "height": 4},
                "click": {"x": 10, "y": 20}
            }
        });

        let err = parse_session_record_drift_diagnostics(PathBuf::from("drift.json"), &diagnostics)
            .expect_err("unsupported proposed field");

        assert!(err.message.contains("outside the amend whitelist"));
    }

    #[test]
    fn recognize_target_output_uses_shared_evaluation_shape() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let pack_root = temp.path().to_path_buf();
        let template_dir = pack_root.join("operations/task/assets");
        let pack_path = temp.path().join("pack.json");
        let pages_path = temp.path().join("pages.json");
        let scene_path = temp.path().join("scene.png");
        fs::create_dir_all(&template_dir).unwrap();
        let png = test_record_frame_png(1, 1);
        fs::write(template_dir.join("HOME.png"), &png).unwrap();
        fs::write(&scene_path, &png).unwrap();
        write_json_file(
            &pack_path,
            &json!({
                "schema_version": "0.3",
                "game": "sample",
                "server": "local",
                "locale": "zh-CN",
                "coordinate_space": {"width": 1, "height": 1},
                "defaults": {"template_threshold": 0.9, "color_max_distance": 20.0},
                "targets": [{
                    "type": "template",
                    "id": "page/home",
                    "template_path": "operations/task/assets/HOME.png",
                    "region": {"x": 0, "y": 0, "width": 1, "height": 1},
                    "threshold": 0.9
                }]
            }),
        )
        .unwrap();
        write_json_file(&pages_path, &json!({"schema_version":"0.3","pages":[]})).unwrap();
        set_missing_config_env();
        let temp = seal_semantic_fixture(temp, "sample", "local", &pack_path, &pages_path, None);

        let result = run_semantic_cli(
            &temp,
            [
                "--json",
                "recognize",
                "--target",
                "page/home",
                "--scene",
                scene_path.to_str().unwrap(),
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(
            result.exit_code(),
            0,
            "{}",
            serde_json::to_string_pretty(&result.envelope).unwrap()
        );
        let data = result.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/matched_rect/width").and_then(Value::as_i64),
            Some(1)
        );
        assert_eq!(
            data.pointer("/template/height").and_then(Value::as_i64),
            Some(1)
        );
        assert_eq!(
            data.pointer("/evaluation/matched_rect/width")
                .and_then(Value::as_i64),
            Some(1)
        );
        assert_eq!(
            data.pointer("/evaluation/template/height")
                .and_then(Value::as_i64),
            Some(1)
        );
    }

    #[test]
    fn env_needs_detection_hint_is_machine_readable() {
        let values = vec![env_detection::ResolvedEnvValue {
            key: "ui_theme".to_string(),
            value: "Siege".to_string(),
            confidence: 0.72,
            source: "detect_ui_theme@Siege".to_string(),
            detector_id: "detect_ui_theme".to_string(),
            source_result: "detect_ui_theme@1783600000000".to_string(),
        }];

        let hint = env_needs_detection_json(
            "recognize",
            "target_below_threshold",
            "button/depot_enter",
            &values,
        )
        .expect("env hint");

        assert_eq!(
            hint.pointer("/status").and_then(Value::as_str),
            Some("needs_detection")
        );
        assert_eq!(
            hint.pointer("/detector_ids/0").and_then(Value::as_str),
            Some("detect_ui_theme")
        );
        assert_eq!(
            hint.pointer("/keys/0/key").and_then(Value::as_str),
            Some("ui_theme")
        );
        assert_eq!(
            hint.pointer("/keys/0/detector_id").and_then(Value::as_str),
            Some("detect_ui_theme")
        );
    }

    #[test]
    fn drift_diagnostics_uses_measured_matched_rect_without_proposed_region() {
        let diagnostics = json!({
            "trigger": "resource_drift",
            "target_id": "page/home",
            "measured": {
                "matched_rect": {"x": 4, "y": 5, "width": 6, "height": 7}
            }
        });

        let parsed =
            parse_session_record_drift_diagnostics(PathBuf::from("drift.json"), &diagnostics)
                .expect("measured matched_rect is a valid fallback");

        assert_eq!(parsed.region.x, 4);
        assert_eq!(parsed.region.y, 5);
        assert_eq!(parsed.region.width, 6);
        assert_eq!(parsed.region.height, 7);
        assert_eq!(parsed.changed_fields, vec!["region"]);
    }

    #[test]
    fn record_drift_target_not_found_is_safety_blocked() {
        let record = drift_test_record(json!([drift_test_anchor_step("home-anchor", "page/home")]));
        let diagnostics = parse_session_record_drift_diagnostics(
            PathBuf::from("drift.json"),
            &json!({
                "trigger": "resource_drift",
                "target_id": "page/missing",
                "measured": {
                    "matched_rect": {"x": 1, "y": 2, "width": 3, "height": 4}
                }
            }),
        )
        .unwrap();

        let err = find_drift_amend_step(&record, &diagnostics, None)
            .expect_err("missing drift target must fail");

        assert_eq!(err.code, "record_drift_target_not_found");
    }

    #[test]
    fn record_drift_target_ambiguous_is_safety_blocked() {
        let record = drift_test_record(json!([
            drift_test_anchor_step("home-anchor", "home"),
            drift_test_anchor_step("page-home-anchor", "page/home")
        ]));
        let diagnostics = parse_session_record_drift_diagnostics(
            PathBuf::from("drift.json"),
            &json!({
                "trigger": "resource_drift",
                "target_id": "page/home",
                "measured": {
                    "matched_rect": {"x": 1, "y": 2, "width": 3, "height": 4}
                }
            }),
        )
        .unwrap();

        let err = find_drift_amend_step(&record, &diagnostics, None)
            .expect_err("ambiguous drift target must fail");

        assert_eq!(err.code, "record_drift_target_ambiguous");
    }

    #[test]
    fn session_record_amend_from_drift_diagnostics_updates_anchor_and_build_task() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let diagnostics_path = temp.path().join("drift.json");
        let out = temp.path().join("draft");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        write_json_file(
            &diagnostics_path,
            &json!({
                "trigger": "resource_drift",
                "target_id": "page/home",
                "measured": {
                    "matched_rect": {"x": 1, "y": 2, "width": 3, "height": 4},
                    "template": {"score": 0.82, "threshold": 0.95}
                },
                "proposed_changes": {
                    "region": {"mode": "rect", "rect": {"x": 1, "y": 2, "width": 3, "height": 4}},
                    "threshold": 0.90
                }
            }),
        )
        .unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let anchor = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "home",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        let operation = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "operation",
                "--step-id",
                "open-home",
                "--from",
                "home",
                "--to",
                "home",
                "--click",
                "4,4",
            ],
            true,
        );
        let amend = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "amend",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--from-drift-diagnostics",
                diagnostics_path.to_str().unwrap(),
            ],
            true,
        );
        let build = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "build-task",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
                "--game",
                "sample",
                "--server",
                "local",
                "--locale",
                "zh-CN",
                "--dry-run",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(anchor.exit_code(), 0);
        assert_eq!(operation.exit_code(), 0);
        assert_eq!(
            amend.exit_code(),
            0,
            "{}",
            serde_json::to_string_pretty(&amend.envelope).unwrap()
        );
        let amend_data = amend.envelope.data.as_ref().unwrap();
        assert_eq!(
            amend_data.get("status").and_then(Value::as_str),
            Some("drift_diagnostics_amended")
        );
        assert_eq!(
            amend_data.pointer("/amend/step_id").and_then(Value::as_str),
            Some("home-anchor")
        );
        assert_eq!(
            amend_data
                .pointer("/amend/changed_fields/0")
                .and_then(Value::as_str),
            Some("region")
        );
        assert_eq!(
            amend_data
                .pointer("/amend/changed_fields/1")
                .and_then(Value::as_str),
            Some("threshold")
        );
        assert_eq!(
            amend_data
                .pointer("/record/steps/0/region/rect/x")
                .and_then(Value::as_i64),
            Some(1)
        );
        assert_eq!(
            amend_data
                .pointer("/record/steps/0/threshold")
                .and_then(Value::as_f64),
            Some(0.90)
        );
        assert_eq!(
            amend_data
                .pointer("/record/steps/0/evaluation/status")
                .and_then(Value::as_str),
            Some("passed")
        );
        assert!(
            amend_data
                .pointer("/record/steps/0/evaluation/backtest")
                .is_some()
        );
        assert_eq!(
            build.exit_code(),
            0,
            "{}",
            serde_json::to_string_pretty(&build.envelope).unwrap()
        );
        let build_data = build.envelope.data.as_ref().unwrap();
        assert_eq!(
            build_data
                .pointer("/bundle/anchors/0/region/rect/x")
                .and_then(Value::as_i64),
            Some(1)
        );
        assert_eq!(
            build_data
                .pointer("/bundle/anchors/0/threshold")
                .and_then(Value::as_f64),
            Some(0.90)
        );
    }

    #[test]
    fn session_record_amend_from_drift_diagnostics_requires_readable_json() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let amend = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "amend",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--from-drift-diagnostics",
                temp.path().join("missing.json").to_str().unwrap(),
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(amend.exit_code(), 2);
        assert_eq!(
            amend.envelope.error.as_ref().unwrap().code,
            "validation_failed"
        );
        assert!(
            amend
                .envelope
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("drift diagnostics file is missing")
        );
    }

    #[test]
    fn session_record_start_requires_task_id() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        set_config_env(&config);

        let result = run_cli(
            [
                "--json",
                "--instance",
                "fixture",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(result.exit_code(), 2);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "validation_failed"
        );
    }

    #[test]
    fn session_instance_list_reads_config() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        set_config_env(&config);

        let _ = run_cli(
            [
                "--json",
                "config",
                "set",
                "instance.a.serial",
                "127.0.0.1:16385",
            ],
            true,
        );
        let _ = run_cli(
            ["--json", "config", "set", "instance.a.game", "sample-c"],
            true,
        );
        let _ = run_cli(
            ["--json", "config", "set", "instance.a.server", "remote"],
            true,
        );
        let _ = run_cli(
            [
                "--json",
                "config",
                "set",
                "instance.a.adb_path",
                "C:\\Tools\\adb.exe",
            ],
            true,
        );
        let _ = run_cli(
            [
                "--json",
                "config",
                "set",
                "instance.a.capture_backend",
                "droidcast_raw",
            ],
            true,
        );
        let result = run_cli(["--json", "session", "instance", "list"], true);
        set_missing_config_env();

        assert_eq!(result.exit_code(), 0);
        let instances = result
            .envelope
            .data
            .as_ref()
            .unwrap()
            .get("instances")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(instances.len(), 1);
        assert_eq!(instances[0].get("id").and_then(Value::as_str), Some("a"));
        assert_eq!(
            instances[0].get("adb_path").and_then(Value::as_str),
            Some("C:\\Tools\\adb.exe")
        );
        assert_eq!(
            instances[0].get("capture_backend").and_then(Value::as_str),
            Some("droidcast_raw")
        );
    }

    #[test]
    fn session_instance_registry_reports_contract() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config_path = temp.path().join("config.json");
        set_config_env(&config_path);

        let mut config = UserConfig::default();
        config.instances.insert(
            "fixture-b".to_string(),
            InstanceConfig {
                serial: Some("127.0.0.1:16416".to_string()),
                game: Some("sample".to_string()),
                server: Some("local-provider".to_string()),
                package: Some("org.example.client".to_string()),
                adb_path: Some("C:\\Tools\\adb.exe".to_string()),
                capture_backend: Some("nemu_ipc".to_string()),
                touch_backend: None,
            },
        );
        config.instances.insert(
            "fixture-c".to_string(),
            InstanceConfig {
                serial: Some("127.0.0.1:16384".to_string()),
                game: Some("sample-b".to_string()),
                server: None,
                package: None,
                adb_path: None,
                capture_backend: None,
                touch_backend: None,
            },
        );
        write_user_config(&config).unwrap();

        let result = run_cli(["--json", "session", "instance", "registry"], true);
        set_missing_config_env();

        assert_eq!(result.exit_code(), 0);
        let data = result.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.get("schema_version").and_then(Value::as_str),
            Some("session.instance_registry.v0.1")
        );
        assert_eq!(data.get("count").and_then(Value::as_u64), Some(2));
        assert_eq!(
            data.pointer("/capture_backends/2").and_then(Value::as_str),
            Some("droidcast_raw")
        );
        let instances = data.get("instances").and_then(Value::as_array).unwrap();
        let x = instances
            .iter()
            .find(|instance| instance.get("id").and_then(Value::as_str) == Some("fixture-b"))
            .unwrap();
        assert_eq!(
            x.pointer("/effective/capture_backend")
                .and_then(Value::as_str),
            Some("nemu_ipc")
        );
        assert_eq!(
            x.pointer("/validation/ready_for_device_control")
                .and_then(Value::as_bool),
            Some(true)
        );
        let y = instances
            .iter()
            .find(|instance| instance.get("id").and_then(Value::as_str) == Some("fixture-c"))
            .unwrap();
        assert_eq!(
            y.pointer("/effective/capture_backend")
                .and_then(Value::as_str),
            Some("auto")
        );
        assert_eq!(
            y.pointer("/validation/ready_for_device_control")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert!(
            y.pointer("/validation/missing_required_fields")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .any(|field| field.as_str() == Some("server"))
        );
    }

    #[test]
    fn session_instance_registry_rejects_invalid_configured_backend() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config_path = temp.path().join("config.json");
        set_config_env(&config_path);

        let mut config = UserConfig::default();
        config.instances.insert(
            "fixture-b".to_string(),
            InstanceConfig {
                serial: Some("127.0.0.1:16416".to_string()),
                game: Some("sample".to_string()),
                server: Some("local-provider".to_string()),
                package: None,
                adb_path: None,
                capture_backend: Some("not-a-backend".to_string()),
                touch_backend: None,
            },
        );
        write_user_config(&config).unwrap();

        let result = run_cli(["--json", "session", "instance", "registry"], true);
        set_missing_config_env();

        assert_eq!(result.exit_code(), 2);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "validation_failed"
        );
        assert!(
            result
                .envelope
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("invalid instance.fixture-b.capture_backend")
        );
    }

    #[test]
    fn capabilities_are_offline() {
        let result = run_cli(["--json", "capabilities"], true);
        assert_eq!(result.exit_code(), 0);
        assert!(
            result
                .envelope
                .data
                .as_ref()
                .unwrap()
                .get("commands")
                .is_some()
        );
        let data = result.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/session_layer/schema_version")
                .and_then(Value::as_str),
            Some("session.capabilities.v0.1")
        );
        assert_eq!(
            data.pointer("/session_layer/access_channels/1/id")
                .and_then(Value::as_str),
            Some("trusted_remote")
        );
        assert!(
            data.get("commands")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .any(|command| command.get("command").and_then(Value::as_str)
                    == Some("session request capabilities"))
        );
        assert!(
            data.get("commands")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .any(|command| command.get("command").and_then(Value::as_str)
                    == Some("session request contract"))
        );
        assert!(
            data.get("commands")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .any(|command| command.get("command").and_then(Value::as_str)
                    == Some("session request api"))
        );
        assert!(
            data.get("commands")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .any(|command| command.get("command").and_then(Value::as_str)
                    == Some("session queue"))
        );
        assert!(
            data.get("commands")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .any(|command| command.get("command").and_then(Value::as_str)
                    == Some("session request queue"))
        );
        assert!(
            data.get("commands")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .any(|command| command.get("command").and_then(Value::as_str)
                    == Some("session submit-plan"))
        );
        assert!(
            data.get("commands")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .any(|command| command.get("command").and_then(Value::as_str)
                    == Some("session bootstrap"))
        );
        assert!(
            data.get("commands")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .any(|command| command.get("command").and_then(Value::as_str)
                    == Some("session request bootstrap"))
        );
        assert!(
            data.get("commands")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .any(|command| command.get("command").and_then(Value::as_str)
                    == Some("session request submit-plan"))
        );
        assert!(
            data.get("commands")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .any(|command| command.get("command").and_then(Value::as_str)
                    == Some("session validation-plan"))
        );
        assert!(
            data.get("commands")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .any(|command| command.get("command").and_then(Value::as_str)
                    == Some("session request validation-plan"))
        );
        assert!(
            data.get("commands")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .any(|command| command.get("command").and_then(Value::as_str)
                    == Some("session request events"))
        );
        assert!(
            data.get("commands")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .any(|command| command.get("command").and_then(Value::as_str)
                    == Some("session transport"))
        );
        assert!(
            data.get("commands")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .any(|command| command.get("command").and_then(Value::as_str)
                    == Some("session request transport"))
        );
        assert!(
            data.get("commands")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .any(|command| command.get("command").and_then(Value::as_str)
                    == Some("session instance registry"))
        );
        let retired = data
            .get("commands")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .find(|command| {
                command.get("command").and_then(Value::as_str)
                    == Some("session instance keep-alive")
            })
            .expect("retired command remains discoverable");
        assert_eq!(
            retired.get("status").and_then(Value::as_str),
            Some("retired")
        );
    }

    #[test]
    fn session_contract_is_offline_access_contract() {
        let _guard = env_lock();
        unsafe {
            env::remove_var(REQUIRE_SESSION_DAEMON_ENV);
        }
        let result = run_cli(["--json", "session", "contract"], true);
        assert_eq!(result.exit_code(), 0);
        let data = result.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.get("schema_version").and_then(Value::as_str),
            Some("session.access.v0.1")
        );
        assert_eq!(
            data.pointer("/entrypoints/local_cli/status")
                .and_then(Value::as_str),
            Some("available")
        );
        assert_eq!(
            data.pointer("/entrypoints/trusted_remote/authentication_required")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/entrypoints/trusted_remote/auth_env/token")
                .and_then(Value::as_str),
            Some(TRUSTED_REMOTE_TOKEN_ENV)
        );
        assert_eq!(
            data.pointer("/safety/control_requests_require_matching_lease")
                .and_then(Value::as_bool),
            Some(true)
        );
        let control_examples = data
            .pointer("/request_classes/control/examples")
            .and_then(Value::as_array)
            .unwrap();
        assert!(
            control_examples
                .iter()
                .any(|item| { item.as_str() == Some("stream --input-event <action,args>") })
        );
        assert!(
            control_examples
                .iter()
                .any(|item| { item.as_str() == Some("stream --relay-event <action,args>") })
        );
        assert_eq!(
            data.pointer("/daemon_queries/bootstrap")
                .and_then(Value::as_str),
            Some("session request bootstrap")
        );
        assert_eq!(
            data.pointer("/daemon_queries/throat_policy")
                .and_then(Value::as_str),
            Some("session request throat-policy")
        );
        assert_eq!(
            data.pointer("/daemon_queries/capture_policy")
                .and_then(Value::as_str),
            Some("session request capture-policy")
        );
        assert_eq!(
            data.pointer("/daemon_queries/self_heal_policy")
                .and_then(Value::as_str),
            Some("session request self-heal-policy")
        );
        assert_eq!(
            data.pointer("/daemon_queries/api").and_then(Value::as_str),
            Some("session request api")
        );
        assert_eq!(
            data.pointer("/daemon_queries/queue")
                .and_then(Value::as_str),
            Some("session request queue")
        );
        assert_eq!(
            data.pointer("/daemon_queries/submit_plan")
                .and_then(Value::as_str),
            Some("session request submit-plan <command...>")
        );
        assert_eq!(
            data.pointer("/daemon_queries/validation_plan")
                .and_then(Value::as_str),
            Some("session request validation-plan")
        );
        assert_eq!(
            data.pointer("/daemon_queries/phase_c_plan")
                .and_then(Value::as_str),
            Some(
                "session request phase-c-plan [--endpoint <url>] [--trigger <kind>] [--to <page>]"
            )
        );
        assert_eq!(
            data.pointer("/daemon_queries/transport")
                .and_then(Value::as_str),
            Some("session request transport")
        );
    }

    #[test]
    fn session_api_is_offline_api_contract() {
        let result = run_cli(["--json", "session", "api"], true);
        assert_eq!(result.exit_code(), 0);
        let data = result.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.get("schema_version").and_then(Value::as_str),
            Some("session.api.v0.1")
        );
        assert_eq!(
            data.pointer("/access_channels/local_cli/status")
                .and_then(Value::as_str),
            Some("available")
        );
        assert_eq!(
            data.pointer("/access_channels/trusted_remote/authentication_required")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/access_channels/trusted_remote/network_listener_implemented")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            data.pointer("/access_channels/trusted_remote/blocked_without_auth_code")
                .and_then(Value::as_str),
            Some("trusted_remote_auth_required")
        );
        assert_eq!(
            data.pointer("/daemon_request_queue/submit_modes/no_wait/flag")
                .and_then(Value::as_str),
            Some("--no-wait")
        );
        assert_eq!(
            data.pointer("/daemon_request_queue/submit_modes/no_wait/waits_for_acknowledgement")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/daemon_request_queue/submit_modes/no_wait/ack_timeout_flag")
                .and_then(Value::as_str),
            Some("--request-ack-timeout-ms")
        );
        assert_eq!(
            data.pointer("/daemon_request_queue/cancel_query")
                .and_then(Value::as_str),
            Some("session request cancel <request-id> [--reason text] [--dry-run]")
        );
        assert_eq!(
            data.pointer("/daemon_request_queue/cancel_records_journal")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/daemon_request_queue/cancel_dry_run_preserves_queue")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/daemon_request_queue/admission_gate/error_code")
                .and_then(Value::as_str),
            Some("request_queue_needs_attention")
        );
        assert_eq!(
            data.pointer("/daemon_request_queue/admission_gate/preflight_command")
                .and_then(Value::as_str),
            Some("session command-check <command...>")
        );
        assert_eq!(
            data.pointer("/envelopes/command_check_view/throat_gate_field")
                .and_then(Value::as_str),
            Some("throat_gate")
        );
        assert_eq!(
            data.pointer("/envelopes/command_check_view/phase_c_scope_field")
                .and_then(Value::as_str),
            Some("phase_c_scope")
        );
        assert_eq!(
            data.pointer("/envelopes/command_check_view/phase_c_scope_schema_version")
                .and_then(Value::as_str),
            Some("session.command_phase_c_scope.v0.1")
        );
        let api_control_examples = data
            .pointer("/command_classes/control/examples")
            .and_then(Value::as_array)
            .unwrap();
        assert!(
            api_control_examples
                .iter()
                .any(|item| { item.as_str() == Some("stream --input-event <action,args>") })
        );
        assert!(
            api_control_examples
                .iter()
                .any(|item| { item.as_str() == Some("stream --relay-event <action,args>") })
        );
        let api_readonly_device_examples = data
            .pointer("/command_classes/read_only/device_affecting_examples")
            .and_then(Value::as_array)
            .unwrap();
        assert!(
            api_readonly_device_examples
                .iter()
                .any(|item| item.as_str() == Some("session record step --current-frame"))
        );
        let api_daemon_examples = data
            .pointer("/command_classes/daemon_state/examples")
            .and_then(Value::as_array)
            .unwrap();
        assert!(
            api_daemon_examples
                .iter()
                .any(|item| item.as_str() == Some("session record step --frame <png>"))
        );
        assert_eq!(
            data.pointer("/envelopes/record_policy_view/schema_version")
                .and_then(Value::as_str),
            Some("session.record_policy.v0.1")
        );
        assert_eq!(
            data.pointer("/envelopes/record_policy_view/authorization_model_field")
                .and_then(Value::as_str),
            Some("authorization_model")
        );
        assert_eq!(
            data.pointer("/envelopes/record_policy_view/allowed_step_kinds_field")
                .and_then(Value::as_str),
            Some("allowed_step_kinds")
        );
        assert_eq!(
            data.pointer("/envelopes/record_policy_view/does_not_write_resource_repositories")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer(
                "/daemon_request_queue/submit_modes/sync_wait/consumes_response_on_success"
            )
            .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/envelopes/lease_view/list_schema_version")
                .and_then(Value::as_str),
            Some("session.lease_list.v0.1")
        );
        assert_eq!(
            data.pointer("/envelopes/lease_view/list_query")
                .and_then(Value::as_str),
            Some("session lease list [--holder <id>] [--lease-id <id>]")
        );
        assert_eq!(
            data.pointer("/envelopes/lease_view/daemon_list_query")
                .and_then(Value::as_str),
            Some("session request lease list [--holder <id>] [--lease-id <id>]")
        );
        assert_eq!(
            data.pointer("/envelopes/lease_view/freshness_field")
                .and_then(Value::as_str),
            Some("freshness")
        );
        assert_eq!(
            data.pointer("/envelopes/lease_view/freshness_stale_after_ms")
                .and_then(Value::as_u64),
            Some(SESSION_LEASE_STALE_MS)
        );
        assert_eq!(
            data.pointer("/envelopes/lease_view/status_schema_version")
                .and_then(Value::as_str),
            Some("session.lease_status.v0.1")
        );
        assert_eq!(
            data.pointer("/envelopes/lease_view/touch_schema_version")
                .and_then(Value::as_str),
            Some("session.lease_touch.v0.1")
        );
        assert_eq!(
            data.pointer("/envelopes/lease_view/touch_query")
                .and_then(Value::as_str),
            Some("session lease touch [--holder <id>] [--lease-id <id>]")
        );
        assert_eq!(
            data.pointer("/envelopes/lease_view/daemon_touch_query")
                .and_then(Value::as_str),
            Some("session request lease touch [--holder <id>] [--lease-id <id>]")
        );
        assert_eq!(
            data.pointer("/envelopes/lease_view/touch_requires_matching_holder")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/envelopes/lease_view/wait_schema_version")
                .and_then(Value::as_str),
            Some("session.lease_wait.v0.1")
        );
        assert_eq!(
            data.pointer("/envelopes/lease_view/wait_query")
                .and_then(Value::as_str),
            Some(
                "session lease wait [--status free|held] [--holder <id>] [--lease-id <id>] [--timeout-ms N] [--poll-ms N]"
            )
        );
        assert_eq!(
            data.pointer("/envelopes/lease_view/daemon_wait_query")
                .and_then(Value::as_str),
            Some(
                "session request lease wait [--status free|held] [--holder <id>] [--lease-id <id>] [--timeout-ms N] [--poll-ms N]"
            )
        );
        assert_eq!(
            data.pointer("/envelopes/lease_view/wait_default_status")
                .and_then(Value::as_str),
            Some("free")
        );
        assert_eq!(
            data.pointer("/envelopes/lease_view/wait_timeout_returns_current_state")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/envelopes/event_view/schema_version")
                .and_then(Value::as_str),
            Some("session.events.v0.1")
        );
        assert_eq!(
            data.pointer("/envelopes/event_view/wait_query")
                .and_then(Value::as_str),
            Some("session events wait [--timeout-ms N] [--poll-ms N]")
        );
        assert_eq!(
            data.pointer("/envelopes/event_view/wait_timeout_default_ms")
                .and_then(Value::as_u64),
            Some(SESSION_DAEMON_REQUEST_TIMEOUT_MS)
        );
        assert_eq!(
            data.pointer("/envelopes/event_view/wait_poll_default_ms")
                .and_then(Value::as_u64),
            Some(100)
        );
        assert_eq!(
            data.pointer("/envelopes/event_view/wait_timeout_returns_empty_events")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/envelopes/event_view/filters/1")
                .and_then(Value::as_str),
            Some("--after-unix-ms")
        );
        assert_eq!(
            data.pointer("/envelopes/event_view/cursor_fields/1")
                .and_then(Value::as_str),
            Some("next_after_unix_ms")
        );
        assert_eq!(
            data.pointer("/envelopes/event_view/cursor_fields/3")
                .and_then(Value::as_str),
            Some("next_after_request_id")
        );
        assert_eq!(
            data.pointer("/envelopes/response_view/schema_version")
                .and_then(Value::as_str),
            Some("session.response.v0.1")
        );
        assert_eq!(
            data.pointer("/envelopes/response_view/delete_after_successful_parse")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/envelopes/response_view/wait_query")
                .and_then(Value::as_str),
            Some("session response wait <request-id> [--timeout-ms N] [--poll-ms N] [--consume]")
        );
        assert_eq!(
            data.pointer("/envelopes/response_view/wait_timeout_default_ms")
                .and_then(Value::as_u64),
            Some(SESSION_DAEMON_REQUEST_TIMEOUT_MS)
        );
        assert_eq!(
            data.pointer("/envelopes/response_view/wait_poll_default_ms")
                .and_then(Value::as_u64),
            Some(100)
        );
        assert_eq!(
            data.pointer("/envelopes/request_state_view/schema_version")
                .and_then(Value::as_str),
            Some("session.request_state.v0.1")
        );
        assert_eq!(
            data.pointer("/envelopes/request_state_view/list_schema_version")
                .and_then(Value::as_str),
            Some("session.request_state_list.v0.1")
        );
        assert_eq!(
            data.pointer("/envelopes/request_state_view/wait_query")
                .and_then(Value::as_str),
            Some(
                "session request-state wait <request-id> [--status <state>] [--timeout-ms N] [--poll-ms N]"
            )
        );
        assert_eq!(
            data.pointer("/envelopes/request_state_view/daemon_wait_query")
                .and_then(Value::as_str),
            Some(
                "session request request-state wait <request-id> [--status <state>] [--timeout-ms N] [--poll-ms N]"
            )
        );
        assert_eq!(
            data.pointer("/envelopes/request_state_view/wait_default_statuses/0")
                .and_then(Value::as_str),
            Some("response_available")
        );
        assert_eq!(
            data.pointer("/envelopes/request_state_view/wait_timeout_default_ms")
                .and_then(Value::as_u64),
            Some(SESSION_DAEMON_REQUEST_TIMEOUT_MS)
        );
        assert_eq!(
            data.pointer("/envelopes/request_state_view/wait_timeout_returns_current_state")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/envelopes/request_state_view/daemon_list_query")
                .and_then(Value::as_str),
            Some(
                "session request request-state list [--limit N] [--status <state>] [--lease-holder <id>]"
            )
        );
        assert_eq!(
            data.pointer("/envelopes/request_state_view/list_global_filters/0")
                .and_then(Value::as_str),
            Some("--instance")
        );
        assert_eq!(
            data.pointer("/envelopes/request_state_view/lease_holder_filter_repeats")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/envelopes/request_state_view/statuses/1")
                .and_then(Value::as_str),
            Some("running")
        );
        assert_eq!(
            data.pointer("/envelopes/request_state_view/statuses/2")
                .and_then(Value::as_str),
            Some("response_available")
        );
        assert_eq!(
            data.pointer("/envelopes/request_state_view/state_sources/3")
                .and_then(Value::as_str),
            Some("request-journal")
        );
        assert_eq!(
            data.pointer("/envelopes/transport_view/schema_version")
                .and_then(Value::as_str),
            Some("session.transport.v0.1")
        );
        assert_eq!(
            data.pointer("/envelopes/transport_view/check_query")
                .and_then(Value::as_str),
            Some("session transport check --endpoint <url>")
        );
        assert_eq!(
            data.pointer("/envelopes/transport_view/plan_query")
                .and_then(Value::as_str),
            Some("session transport plan [--endpoint <url>]")
        );
        assert_eq!(
            data.pointer("/envelopes/transport_view/plan_schema_version")
                .and_then(Value::as_str),
            Some("session.transport_plan.v0.1")
        );
        assert_eq!(
            data.pointer("/envelopes/transport_view/plan_next_actions_field")
                .and_then(Value::as_str),
            Some("next_actions")
        );
        assert_eq!(
            data.pointer("/envelopes/transport_view/plan_trusted_remote_gate_field")
                .and_then(Value::as_str),
            Some("trusted_remote_gate")
        );
        assert_eq!(
            data.pointer("/envelopes/transport_view/plan_trusted_remote_gate_schema_version")
                .and_then(Value::as_str),
            Some("session.trusted_remote_gate.v0.1")
        );
        assert_eq!(
            data.pointer("/envelopes/transport_view/check_schema_version")
                .and_then(Value::as_str),
            Some("session.transport_check.v0.1")
        );
        assert_eq!(
            data.pointer("/envelopes/validation_plan_view/pending_live_acceptance_field")
                .and_then(Value::as_str),
            Some("pending_live_acceptance")
        );
        assert_eq!(
            data.pointer("/envelopes/validation_plan_view/phase_acceptance_matrix_field")
                .and_then(Value::as_str),
            Some("phase_acceptance_matrix")
        );
        assert_eq!(
            data.pointer("/envelopes/validation_plan_view/next_actions_field")
                .and_then(Value::as_str),
            Some("next_actions")
        );
        assert_eq!(
            data.pointer("/envelopes/bootstrap_view/schema_version")
                .and_then(Value::as_str),
            Some("session.bootstrap.v0.1")
        );
        assert_eq!(
            data.pointer("/envelopes/bootstrap_view/status_diagnostics_field")
                .and_then(Value::as_str),
            Some("status_diagnostics")
        );
        assert_eq!(
            data.pointer("/envelopes/bootstrap_view/status_diagnostics_capture_freshness_field")
                .and_then(Value::as_str),
            Some("status_diagnostics.capture_freshness")
        );
        assert_eq!(
            data.pointer("/envelopes/bootstrap_view/status_diagnostics_self_heal_field")
                .and_then(Value::as_str),
            Some("status_diagnostics.self_heal")
        );
        assert_eq!(
            data.pointer("/envelopes/bootstrap_view/status_diagnostics_interaction_flow_field")
                .and_then(Value::as_str),
            Some("status_diagnostics.interaction_flow")
        );
        assert_eq!(
            data.pointer("/envelopes/bootstrap_view/status_diagnostics_trusted_channel_field")
                .and_then(Value::as_str),
            Some("status_diagnostics.trusted_channel")
        );
        assert_eq!(
            data.pointer("/envelopes/bootstrap_view/status_diagnostics_phase_c_field")
                .and_then(Value::as_str),
            Some("status_diagnostics.phase_c")
        );
        assert_eq!(
            data.pointer("/envelopes/bootstrap_view/status_diagnostics_validation_field")
                .and_then(Value::as_str),
            Some("status_diagnostics.validation")
        );
        assert_eq!(
            data.pointer("/envelopes/bootstrap_view/validation_plan_field")
                .and_then(Value::as_str),
            Some("validation_plan")
        );
        assert_eq!(
            data.pointer("/envelopes/bootstrap_view/throat_policy_field")
                .and_then(Value::as_str),
            Some("throat_policy")
        );
        assert_eq!(
            data.pointer("/envelopes/throat_policy_view/schema_version")
                .and_then(Value::as_str),
            Some("session.throat_policy.v0.1")
        );
        assert_eq!(
            data.pointer("/envelopes/throat_policy_view/only_control_throat_field")
                .and_then(Value::as_str),
            Some("session_layer.only_control_throat")
        );
        assert_eq!(
            data.pointer("/envelopes/capture_policy_view/schema_version")
                .and_then(Value::as_str),
            Some("session.capture_policy.v0.1")
        );
        assert_eq!(
            data.pointer("/envelopes/capture_policy_view/stale_classification_field")
                .and_then(Value::as_str),
            Some("stale_classification")
        );
        assert_eq!(
            data.pointer("/envelopes/capture_policy_view/freeze_classification_gate_field")
                .and_then(Value::as_str),
            Some("freeze_classification_gate")
        );
        assert_eq!(
            data.pointer(
                "/envelopes/capture_policy_view/freeze_classification_gate_schema_version"
            )
            .and_then(Value::as_str),
            Some("session.capture_freeze_classification_gate.v0.1")
        );
        assert_eq!(
            data.pointer("/envelopes/self_heal_policy_view/schema_version")
                .and_then(Value::as_str),
            Some("session.self_heal_policy.v0.1")
        );
        assert_eq!(
            data.pointer("/envelopes/self_heal_policy_view/maintenance_boundary_field")
                .and_then(Value::as_str),
            Some("maintenance_boundary")
        );
        assert_eq!(
            data.pointer("/envelopes/self_heal_plan_view/next_actions_field")
                .and_then(Value::as_str),
            Some("next_actions")
        );
        assert_eq!(
            data.pointer("/envelopes/self_heal_plan_view/execution_gate_field")
                .and_then(Value::as_str),
            Some("execution_gate")
        );
        assert_eq!(
            data.pointer("/envelopes/self_heal_plan_view/execution_gate_schema_version")
                .and_then(Value::as_str),
            Some("session.self_heal_execution_gate.v0.1")
        );
        assert_eq!(
            data.pointer("/envelopes/status_view/queue_health_actions/1")
                .and_then(Value::as_str),
            Some("blocked_request_cancel_dry_run")
        );
        assert_eq!(
            data.pointer("/envelopes/status_view/queue_health_actions/5")
                .and_then(Value::as_str),
            Some("unclaimed_response_read")
        );
        assert_eq!(
            data.pointer("/envelopes/readiness_view/queues_field")
                .and_then(Value::as_str),
            Some("queues")
        );
        assert_eq!(
            data.pointer("/envelopes/readiness_view/queue_health_field")
                .and_then(Value::as_str),
            Some("queues.health")
        );
        assert_eq!(
            data.pointer("/envelopes/readiness_view/instances_field")
                .and_then(Value::as_str),
            Some("instances")
        );
        assert_eq!(
            data.pointer("/envelopes/readiness_view/instance_status_field")
                .and_then(Value::as_str),
            Some("instances.status")
        );
        assert_eq!(
            data.pointer("/envelopes/readiness_view/selected_instance_status_field")
                .and_then(Value::as_str),
            Some("instances.selected_status")
        );
        assert_eq!(
            data.pointer("/envelopes/readiness_view/selected_instance_missing_required_field")
                .and_then(Value::as_str),
            Some("instances.selected_missing_required")
        );
        assert_eq!(
            data.pointer("/envelopes/readiness_view/policy_summary_field")
                .and_then(Value::as_str),
            Some("policy_summary")
        );
        assert_eq!(
            data.pointer("/envelopes/readiness_view/policy_summary_schema_version")
                .and_then(Value::as_str),
            Some("session.readiness_policy_summary.v0.1")
        );
        assert_eq!(
            data.pointer("/envelopes/readiness_view/diagnostics_summary_field")
                .and_then(Value::as_str),
            Some("diagnostics_summary")
        );
        assert_eq!(
            data.pointer("/envelopes/readiness_view/diagnostics_summary_schema_version")
                .and_then(Value::as_str),
            Some("session.readiness_diagnostics_summary.v0.1")
        );
        assert_eq!(
            data.pointer("/envelopes/readiness_view/phase_c_summary_field")
                .and_then(Value::as_str),
            Some("diagnostics_summary.phase_c")
        );
        assert_eq!(
            data.pointer("/envelopes/readiness_view/phase_c_acceptance_gates_schema_version_field")
                .and_then(Value::as_str),
            Some("diagnostics_summary.phase_c.acceptance_gates_schema_version")
        );
        assert_eq!(
            data.pointer("/envelopes/readiness_view/phase_c_acceptance_gate_lane_count_field")
                .and_then(Value::as_str),
            Some("diagnostics_summary.phase_c.acceptance_gate_lane_count")
        );
        assert_eq!(
            data.pointer("/envelopes/connect_plan_view/schema_version")
                .and_then(Value::as_str),
            Some("session.connect_plan.v0.1")
        );
        assert_eq!(
            data.pointer("/envelopes/connect_plan_view/stream_preflight_field")
                .and_then(Value::as_str),
            Some("stream_preflight")
        );
        assert_eq!(
            data.pointer("/envelopes/connect_plan_view/phase_c_preflight_field")
                .and_then(Value::as_str),
            Some("phase_c_preflight")
        );
        assert_eq!(
            data.pointer("/envelopes/connect_plan_view/phase_c_preflight_schema_version")
                .and_then(Value::as_str),
            Some("session.connect_phase_c_preflight.v0.1")
        );
        assert_eq!(
            data.pointer("/envelopes/connect_plan_view/next_actions_field")
                .and_then(Value::as_str),
            Some("next_actions")
        );
        assert_eq!(
            data.pointer("/envelopes/connect_plan_view/does_not_start_listener")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/envelopes/stream_plan_view/schema_version")
                .and_then(Value::as_str),
            Some("session.stream_plan.v0.1")
        );
        assert_eq!(
            data.pointer("/envelopes/stream_plan_view/safe_to_open_stream_field")
                .and_then(Value::as_str),
            Some("safe_to_open_stream")
        );
        assert_eq!(
            data.pointer("/envelopes/stream_plan_view/next_actions_field")
                .and_then(Value::as_str),
            Some("next_actions")
        );
        assert_eq!(
            data.pointer("/envelopes/stream_plan_view/does_not_start_listener")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/envelopes/stream_view/input_relay_preflight_command")
                .and_then(Value::as_str),
            Some("session command-check stream --input-event <action,args>")
        );
        assert_eq!(
            data.pointer("/envelopes/stream_view/input_relay_event_flags/1")
                .and_then(Value::as_str),
            Some("--input-event")
        );
        assert_eq!(
            data.pointer("/envelopes/queue_view/schema_version")
                .and_then(Value::as_str),
            Some("session.queue.v0.1")
        );
        assert_eq!(
            data.pointer("/envelopes/queue_view/query")
                .and_then(Value::as_str),
            Some("session queue")
        );
        assert_eq!(
            data.pointer("/envelopes/queue_view/local_query_inspects_blocked_queue")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/envelopes/command_check_view/queue_gate_field")
                .and_then(Value::as_str),
            Some("queue_gate")
        );
        assert_eq!(
            data.pointer("/envelopes/command_check_view/instance_gate_field")
                .and_then(Value::as_str),
            Some("instance_gate")
        );
        assert_eq!(
            data.pointer("/envelopes/submit_plan_view/schema_version")
                .and_then(Value::as_str),
            Some("session.submit_plan.v0.1")
        );
        assert_eq!(
            data.pointer("/envelopes/submit_plan_view/query")
                .and_then(Value::as_str),
            Some("session submit-plan <command...>")
        );
        assert_eq!(
            data.pointer("/envelopes/submit_plan_view/daemon_query")
                .and_then(Value::as_str),
            Some("session request submit-plan <command...>")
        );
        assert_eq!(
            data.pointer("/envelopes/submit_plan_view/preflight_summary_field")
                .and_then(Value::as_str),
            Some("preflight_summary")
        );
        assert_eq!(
            data.pointer("/envelopes/submit_plan_view/phase_c_execution_preflight_field")
                .and_then(Value::as_str),
            Some("phase_c_execution_preflight")
        );
        assert_eq!(
            data.pointer("/envelopes/submit_plan_view/phase_c_execution_preflight_schema_version")
                .and_then(Value::as_str),
            Some("session.submit_phase_c_execution_preflight.v0.1")
        );
        assert_eq!(
            data.pointer("/envelopes/validation_plan_view/schema_version")
                .and_then(Value::as_str),
            Some("session.validation_plan.v0.1")
        );
        assert_eq!(
            data.pointer("/envelopes/validation_plan_view/deferred_code_field")
                .and_then(Value::as_str),
            Some("deferred_code")
        );
        assert_eq!(
            data.pointer("/failure_contract/untrusted_remote_endpoint_code")
                .and_then(Value::as_str),
            Some("trusted_remote_transport_blocked")
        );
    }

    #[test]
    fn session_transport_is_offline_transport_contract() {
        let result = run_cli(["--json", "session", "transport"], true);
        assert_eq!(result.exit_code(), 0);
        let data = result.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.get("schema_version").and_then(Value::as_str),
            Some("session.transport.v0.1")
        );
        assert_eq!(
            data.pointer("/channels/daemon_file_ipc/status")
                .and_then(Value::as_str),
            Some("available")
        );
        assert_eq!(
            data.pointer("/channels/trusted_remote/encryption_required")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/channels/trusted_remote/auth_env/client_certificate")
                .and_then(Value::as_str),
            Some(TRUSTED_REMOTE_CLIENT_CERT_ENV)
        );
        assert_eq!(
            data.pointer("/channels/trusted_remote/preflight_command")
                .and_then(Value::as_str),
            Some("session transport check --endpoint <url>")
        );
        assert_eq!(
            data.pointer("/channels/trusted_remote/plan_command")
                .and_then(Value::as_str),
            Some("session transport plan [--endpoint <url>]")
        );
        assert_eq!(
            data.pointer("/safety/remote_transport_must_not_start_without_authentication")
                .and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn schema_pack_describes_current_supported_versions() {
        let result = run_cli(["--json", "schema", "pack"], true);
        assert_eq!(result.exit_code(), 0);
        let versions = result
            .envelope
            .data
            .as_ref()
            .unwrap()
            .get("schema_version")
            .and_then(Value::as_array)
            .expect("schema versions");
        assert!(versions.iter().any(|value| value.as_str() == Some("0.4")));
        assert!(versions.iter().any(|value| value.as_str() == Some("0.5")));
    }

    #[test]
    fn top_level_record_capability_is_available() {
        let commands = command_capabilities();
        let record = commands
            .iter()
            .find(|command| command.get("command").and_then(Value::as_str) == Some("record"))
            .expect("record capability");
        assert_eq!(
            record.get("status").and_then(Value::as_str),
            Some("available")
        );
        assert_eq!(
            record
                .get("needs")
                .and_then(Value::as_array)
                .and_then(|needs| {
                    needs
                        .iter()
                        .find(|need| need.as_str() == Some("offline"))
                        .and_then(Value::as_str)
                }),
            Some("offline")
        );
        for command_name in [
            "record start",
            "record status",
            "record stop",
            "record build-task",
            "session record start",
            "session record status",
            "session record stop",
            "session record build-task",
            "session stream",
            "session stream check",
            "session request stream check",
        ] {
            let command = commands
                .iter()
                .find(|command| {
                    command.get("command").and_then(Value::as_str) == Some(command_name)
                })
                .unwrap_or_else(|| panic!("{command_name} capability"));
            assert_eq!(
                command.get("status").and_then(Value::as_str),
                Some("available")
            );
        }
        let stream = commands
            .iter()
            .find(|command| command.get("command").and_then(Value::as_str) == Some("stream"))
            .expect("stream capability");
        assert_eq!(
            stream.get("status").and_then(Value::as_str),
            Some("available")
        );
    }

    #[test]
    fn session_response_capabilities_are_available() {
        let commands = command_capabilities();
        for command_name in [
            "session response",
            "session response get",
            "session response wait",
            "session request response",
            "session request response get",
            "session request response wait",
        ] {
            let command = commands
                .iter()
                .find(|command| {
                    command.get("command").and_then(Value::as_str) == Some(command_name)
                })
                .unwrap_or_else(|| panic!("{command_name} capability"));
            assert_eq!(
                command.get("status").and_then(Value::as_str),
                Some("available")
            );
        }
    }

    #[test]
    fn session_request_no_wait_capability_is_available() {
        let commands = command_capabilities();
        let command = commands
            .iter()
            .find(|command| {
                command.get("command").and_then(Value::as_str) == Some("session request --no-wait")
            })
            .expect("session request --no-wait capability");
        assert_eq!(
            command.get("status").and_then(Value::as_str),
            Some("available")
        );
    }

    #[test]
    fn session_request_cancel_capability_is_available() {
        let commands = command_capabilities();
        let command = commands
            .iter()
            .find(|command| {
                command.get("command").and_then(Value::as_str) == Some("session request cancel")
            })
            .expect("session request cancel capability");
        assert_eq!(
            command.get("status").and_then(Value::as_str),
            Some("available")
        );
        assert_eq!(
            command.get("needs").and_then(Value::as_array).unwrap(),
            &vec![Value::String("offline".to_string())]
        );
    }

    #[test]
    fn session_request_state_capabilities_are_available() {
        let commands = command_capabilities();
        for command_name in [
            "session request-state",
            "session request-state get",
            "session request-state wait",
            "session request-state list",
            "session request request-state",
            "session request request-state get",
            "session request request-state wait",
            "session request request-state list",
        ] {
            let command = commands
                .iter()
                .find(|command| {
                    command.get("command").and_then(Value::as_str) == Some(command_name)
                })
                .unwrap_or_else(|| panic!("{command_name} capability"));
            assert_eq!(
                command.get("status").and_then(Value::as_str),
                Some("available")
            );
        }
    }

    #[test]
    fn retired_session_and_lab_lease_authority_is_not_advertised() {
        let commands = command_capabilities();
        for name in [
            "session lease",
            "session lease list",
            "session lease touch",
            "session lease wait",
            "session request lease list",
            "session request lease touch",
            "session request lease wait",
            "lab lease",
            "lab lease list",
            "lab lease status",
            "lab lease touch",
            "lab lease wait",
            "lab preempt",
            "lab release",
        ] {
            assert!(
                commands.iter().all(|command| command["command"] != name),
                "retired Lab authority must not be advertised: {name}"
            );
        }
        assert!(commands.iter().any(|command| {
            command["command"] == "lab status" && command["needs"] == json!(["running_runtime"])
        }));
        assert!(commands.iter().any(|command| {
            command["command"] == "lab debug-package"
                && command["needs"] == json!(["running_runtime"])
        }));
    }

    #[test]
    fn direct_touch_positionals_parse() {
        let tap = FlagArgs::parse(&["300".to_string(), "2".to_string()]).unwrap();
        assert_eq!(
            DirectTouchCommand::parse("tap", &tap).unwrap(),
            DirectTouchCommand::Tap { x: 300, y: 2 }
        );

        let swipe = FlagArgs::parse(&[
            "10".to_string(),
            "20".to_string(),
            "300".to_string(),
            "400".to_string(),
            "500".to_string(),
        ])
        .unwrap();
        assert_eq!(
            DirectTouchCommand::parse("swipe", &swipe).unwrap(),
            DirectTouchCommand::Swipe {
                x1: 10,
                y1: 20,
                x2: 300,
                y2: 400,
                duration_ms: 500
            }
        );

        let long_tap =
            FlagArgs::parse(&["100".to_string(), "200".to_string(), "900".to_string()]).unwrap();
        assert_eq!(
            DirectTouchCommand::parse("long-tap", &long_tap).unwrap(),
            DirectTouchCommand::LongTap {
                x: 100,
                y: 200,
                duration_ms: 900
            }
        );
    }

    #[test]
    fn direct_touch_missing_args_are_usage_errors() {
        let flags = FlagArgs::parse(&["300".to_string()]).unwrap();
        let err = DirectTouchCommand::parse("tap", &flags).unwrap_err();
        assert_eq!(err.exit_code(), 2);
        assert_eq!(err.code, "validation_failed");
        assert!(err.message.contains("tap expects 2"));
    }

    #[test]
    fn direct_input_positionals_parse() {
        let key = FlagArgs::parse(&["back".to_string()]).unwrap();
        assert_eq!(
            DirectInputCommand::parse("key", &key).unwrap(),
            DirectInputCommand::Key("4".to_string())
        );

        let text = FlagArgs::parse(&["hello".to_string(), "world".to_string()]).unwrap();
        assert_eq!(
            DirectInputCommand::parse("text", &text).unwrap(),
            DirectInputCommand::Text("hello world".to_string())
        );
    }

    #[test]
    fn capture_static_page_same_hash_does_not_switch() {
        let decision = classify_capture_freshness(
            "same-frame",
            "same-frame",
            CaptureFreshnessExpectation::StaticPageAllowed,
        );

        assert_eq!(decision.status, CaptureFreshProbeStatus::StaticUnchanged);
        assert!(decision.ok);
        assert!(!decision.stale_suspected);
    }

    #[test]
    fn capture_expected_change_stall_marks_stale_without_runtime_switch() {
        let decision = classify_capture_freshness(
            "same-frame",
            "same-frame",
            CaptureFreshnessExpectation::ExpectedChange,
        );

        assert_eq!(decision.status, CaptureFreshProbeStatus::StaleSuspected);
        assert!(!decision.ok);
        assert!(decision.stale_suspected);
    }

    #[test]
    fn capture_diagnosis_recommends_fast_backends_before_restart_for_adb_stale() {
        let recovery = capture_diagnosis_recovery_json(
            CaptureFreshProbeStatus::StaleSuspected,
            CaptureBackendChoice::Adb,
        );
        assert_eq!(recovery.get("needed").and_then(Value::as_bool), Some(true));
        let recommendations = recovery
            .get("recommendations")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(
            recommendations[0].get("type").and_then(Value::as_str),
            Some("capture_backend")
        );
        assert_eq!(
            recommendations
                .last()
                .and_then(|value| value.get("type"))
                .and_then(Value::as_str),
            Some("app_restart")
        );
    }

    #[test]
    fn instance_health_status_reflects_capture_freshness() {
        assert_eq!(instance_health_status(None), "device_connected");
        assert_eq!(
            instance_health_status(Some(CaptureFreshProbeStatus::Fresh)),
            "healthy"
        );
        assert_eq!(
            instance_health_status(Some(CaptureFreshProbeStatus::StaticUnchanged)),
            "healthy_static"
        );
        assert_eq!(
            instance_health_status(Some(CaptureFreshProbeStatus::StaleSuspected)),
            "capture_stale_suspected"
        );
    }

    #[test]
    fn instance_health_capture_diagnose_json_reports_recovery() {
        let report = CaptureFreshProbeReport {
            status: CaptureFreshProbeStatus::StaleSuspected,
            frame: None,
            attempts: vec![json!({
                "backend": "adb_screencap",
                "ok": false,
                "stage": "fresh_probe",
                "stale_suspected": true
            })],
            freshness: json!({
                "required": true,
                "fresh": false,
                "status": "stale_suspected"
            }),
        };

        let value = capture_fresh_probe_report_json(&report, CaptureBackendChoice::Adb);
        assert_eq!(
            value.get("status").and_then(Value::as_str),
            Some("stale_suspected")
        );
        assert_eq!(
            value.get("requested_backend").and_then(Value::as_str),
            Some("adb")
        );
        assert_eq!(
            value.pointer("/recovery/reason").and_then(Value::as_str),
            Some("stale_capture_suspected")
        );
        assert_eq!(
            value
                .pointer("/capture_backend_attempts/0/backend")
                .and_then(Value::as_str),
            Some("adb_screencap")
        );
    }

    #[test]
    fn session_recover_stale_capture_plans_lighter_steps_before_restart() {
        let result = run_cli(
            [
                "--json",
                "--capture-backend",
                "adb",
                "session",
                "recover",
                "--stale-capture",
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0);
        let data = result.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.get("mode").and_then(Value::as_str),
            Some("stale_capture_recovery")
        );
        assert_eq!(data.get("executed").and_then(Value::as_bool), Some(false));
        assert_eq!(
            data.pointer("/steps/0/type").and_then(Value::as_str),
            Some("fresh_probe")
        );
        assert_eq!(
            data.pointer("/steps/3/type").and_then(Value::as_str),
            Some("app_restart")
        );
        assert_eq!(
            data.pointer("/steps/3/requires_lease")
                .and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn stale_capture_recovery_json_reports_executed_fresh_diagnosis() {
        let report = CaptureFreshProbeReport {
            status: CaptureFreshProbeStatus::Fresh,
            frame: None,
            attempts: vec![json!({
                "backend": "nemu_ipc",
                "ok": true,
                "stage": "fresh_probe"
            })],
            freshness: json!({
                "required": true,
                "fresh": true,
                "status": "fresh"
            }),
        };

        let value = stale_capture_recovery_json(
            CaptureBackendChoice::Auto,
            Duration::from_millis(200),
            Some(&report),
        );

        assert_eq!(
            value.get("status").and_then(Value::as_str),
            Some("diagnosed_fresh")
        );
        assert_eq!(
            value.get("diagnosis_executed").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            value
                .pointer("/diagnosis/result/status")
                .and_then(Value::as_str),
            Some("fresh")
        );
        assert_eq!(
            value.pointer("/recovery/needed").and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            value
                .pointer("/diagnosis/result/capture_backend_attempts/0/backend")
                .and_then(Value::as_str),
            Some("nemu_ipc")
        );
    }

    #[test]
    fn direct_touch_commands_are_capability_registered() {
        let commands = command_capabilities();
        for command in ["tap", "swipe", "long-tap", "key", "text"] {
            let capability = commands
                .iter()
                .find(|value| value.get("command").and_then(Value::as_str) == Some(command))
                .unwrap_or_else(|| panic!("{command} capability missing"));
            assert_eq!(
                capability.get("status").and_then(Value::as_str),
                Some("available")
            );
            assert_eq!(
                capability.get("needs").and_then(Value::as_array).unwrap(),
                &vec![Value::String("device".to_string())]
            );
        }
        for command in [
            "session status",
            "session bootstrap",
            "session throat-policy",
            "session capture-policy",
            "session self-heal-policy",
            "session submit-plan",
            "session validation-plan",
            "session journal",
            "session events",
            "session events wait",
            "session contract",
            "session api",
            "session instance",
            "session instance list",
            "session instance app",
            "session instance app launch",
            "session instance app stop",
            "session instance app force-stop",
            "session instance app restart",
            "session app",
            "session app launch",
            "session app stop",
            "session app force-stop",
            "session app restart",
            "session capture",
            "session capture diagnose",
            "session recover --stale-capture",
            "session request status",
            "session request bootstrap",
            "session request throat-policy",
            "session request capture-policy",
            "session request self-heal-policy",
            "session request submit-plan",
            "session request validation-plan",
            "session request journal",
            "session request events",
            "session request events wait",
            "session request cancel",
            "session request contract",
            "session request api",
            "session request devices",
            "session request record",
            "session request capture",
            "session request capture-diagnose",
            "session request stream",
            "session request recognize",
            "session request detect-page",
            "session request current-page",
            "session request is-visible",
            "session request locate",
            "session request monitor-once",
            "session request instance list",
            "session request instance registry",
            "session request instance app",
            "session request app",
            "session request recover --stale-capture",
            "session request lab-run",
            "session request package-run",
            "session request operation-run",
            "ledger show",
            "ledger events",
            "ledger receipts",
            "ledger diagnose",
            "ledger evidence",
            "stream",
            "capture diagnose",
        ] {
            let capability = commands
                .iter()
                .find(|value| value.get("command").and_then(Value::as_str) == Some(command))
                .unwrap_or_else(|| panic!("{command} capability missing"));
            assert_eq!(
                capability.get("status").and_then(Value::as_str),
                Some("available")
            );
        }
        for command in [
            "session instance health",
            "session instance keep-alive",
            "session instance connect",
            "session instance reconnect",
            "session request instance health",
            "session request instance keep-alive",
            "session request instance connect",
            "session request instance reconnect",
        ] {
            let capability = commands
                .iter()
                .find(|value| value.get("command").and_then(Value::as_str) == Some(command))
                .unwrap_or_else(|| panic!("{command} retirement marker missing"));
            assert_eq!(
                capability.get("status").and_then(Value::as_str),
                Some("retired")
            );
            assert_eq!(
                capability.get("needs").and_then(Value::as_array).unwrap(),
                &vec![Value::String("offline".to_string())]
            );
        }
    }

    #[test]
    fn retired_instance_commands_are_absent_from_live_contracts() {
        let global = GlobalOptions::default();
        let flags = FlagArgs::default();
        let contracts = [
            session_layer_capability_contract(),
            session_access_contract(),
            session_api_contract(),
            session_capture_policy_payload(&global, &flags, "session capture-policy").unwrap(),
            session_self_heal_policy_payload(&global, &flags, "session self-heal-policy").unwrap(),
            stale_capture_recovery_json(CaptureBackendChoice::Adb, Duration::from_millis(1), None),
        ];
        for contract in contracts {
            let text = serde_json::to_string(&contract).unwrap();
            for command in [
                "session instance health",
                "session instance keep-alive",
                "session instance connect",
                "session instance reconnect",
                "session request instance health",
                "session request instance keep-alive",
                "session request instance connect",
                "session request instance reconnect",
            ] {
                assert!(
                    !text.contains(command),
                    "retired command advertised: {command}"
                );
            }
        }
    }

    #[test]
    fn package_validate_accepts_safe_zip() {
        let temp = TempDir::new().unwrap();
        let zip = temp.path().join("bundle.zip");
        write_test_zip(
            &zip,
            &[
                (
                    "module/manifest.json",
                    br#"{"schema_version":"0.2"}"#.as_slice(),
                ),
                (
                    "module/operations/task/task.json",
                    br#"{"id":"task"}"#.as_slice(),
                ),
                ("module/operations/resources.json", br#"{}"#.as_slice()),
            ],
        );
        let result = run_cli(
            [
                "--json",
                "package",
                "validate",
                "--zip",
                zip.to_str().unwrap(),
            ],
            true,
        );
        assert_eq!(result.exit_code(), 0);
        let data = result.envelope.data.as_ref().expect("validation payload");
        assert_eq!(
            data.get("hash_source").and_then(Value::as_str),
            Some("self_computed_provenance_only")
        );
        assert_eq!(
            data.get("externally_verified").and_then(Value::as_bool),
            Some(false)
        );
    }

    #[test]
    fn package_validate_accepts_matching_external_hash() {
        let temp = TempDir::new().unwrap();
        let zip = temp.path().join("bundle.zip");
        write_test_zip(
            &zip,
            &[
                (
                    "module/manifest.json",
                    br#"{"schema_version":"0.2"}"#.as_slice(),
                ),
                (
                    "module/operations/task/task.json",
                    br#"{"id":"task"}"#.as_slice(),
                ),
                ("module/operations/resources.json", br#"{}"#.as_slice()),
            ],
        );
        let hash = format!("{:x}", Sha256::digest(fs::read(&zip).unwrap()));

        let result = run_cli(
            [
                "--json",
                "package",
                "validate",
                "--zip",
                zip.to_str().unwrap(),
                "--expected-sha256",
                &hash,
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
        let data = result.envelope.data.as_ref().expect("validation payload");
        assert_eq!(
            data.get("hash_source").and_then(Value::as_str),
            Some("externally_supplied")
        );
        assert_eq!(
            data.get("externally_verified").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.get("input_sha256").and_then(Value::as_str),
            Some(hash.as_str())
        );
    }

    #[test]
    fn package_validate_rejects_mismatched_external_hash() {
        let temp = TempDir::new().unwrap();
        let zip = temp.path().join("bundle.zip");
        write_test_zip(
            &zip,
            &[
                (
                    "module/manifest.json",
                    br#"{"schema_version":"0.2"}"#.as_slice(),
                ),
                (
                    "module/operations/task/task.json",
                    br#"{"id":"task"}"#.as_slice(),
                ),
                ("module/operations/resources.json", br#"{}"#.as_slice()),
            ],
        );

        let result = run_cli(
            [
                "--json",
                "package",
                "validate",
                "--zip",
                zip.to_str().unwrap(),
                "--expected-sha256",
                "0000000000000000000000000000000000000000000000000000000000000000",
            ],
            true,
        );

        assert_eq!(result.exit_code(), 2);
        assert!(result.envelope_json().contains("hash mismatch"));
    }

    #[test]
    fn package_validate_rejects_bare_external_hash_flag() {
        let temp = TempDir::new().unwrap();
        let zip = temp.path().join("bundle.zip");
        write_test_zip(
            &zip,
            &[
                (
                    "module/manifest.json",
                    br#"{"schema_version":"0.2"}"#.as_slice(),
                ),
                (
                    "module/operations/task/task.json",
                    br#"{"id":"task"}"#.as_slice(),
                ),
                ("module/operations/resources.json", br#"{}"#.as_slice()),
            ],
        );

        let result = run_cli(
            [
                "--json",
                "package",
                "validate",
                "--zip",
                zip.to_str().unwrap(),
                "--expected-sha256",
            ],
            true,
        );

        assert_eq!(result.exit_code(), 2);
        assert!(
            result
                .envelope_json()
                .contains("requires an explicit SHA-256 value")
        );
    }

    #[test]
    fn package_validate_reports_offline_projection_without_local_ledger() {
        let _guard = env_lock();
        set_missing_config_env();
        let temp = TempDir::new().unwrap();
        let run_root = temp.path().join("runs");
        let zip = temp.path().join("bundle.zip");
        write_test_zip(
            &zip,
            &[
                (
                    "module/manifest.json",
                    br#"{"schema_version":"0.2"}"#.as_slice(),
                ),
                (
                    "module/operations/task/task.json",
                    br#"{"id":"task"}"#.as_slice(),
                ),
                ("module/operations/resources.json", br#"{}"#.as_slice()),
            ],
        );

        let result = run_cli(
            [
                "--json",
                "--run-root",
                run_root.to_str().unwrap(),
                "package",
                "validate",
                "--zip",
                zip.to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
        let data = result.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/ledger_event/written")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            data.pointer("/ledger_event/reason").and_then(Value::as_str),
            Some("offline_resource_tooling_projection")
        );
        assert!(!run_root.exists());
    }

    #[test]
    fn list_resource_kind_unknown_returns_usage_error() {
        let temp = TempDir::new().unwrap();

        let err = list_resource_kind(temp.path(), "future-kind").expect_err("unknown kind");

        assert_eq!(err.code, "validation_failed");
        assert!(err.message.contains("unknown list kind"));
    }

    #[test]
    fn lab_run_capture_backend_flag_is_global_even_after_subcommand() {
        let invocation = parse_invocation(
            [
                "--json",
                "lab",
                "run",
                "--zip",
                "in.zip",
                "--capture-backend",
                "nemu_ipc",
                "--out",
                "out.zip",
            ],
            true,
        )
        .expect("invocation");

        assert_eq!(
            invocation.global.capture_backend,
            Some(CaptureBackendChoice::NemuIpc)
        );
        assert_eq!(invocation.command, ["lab", "run"]);
        assert_eq!(invocation.args, ["--zip", "in.zip", "--out", "out.zip"]);
    }

    #[test]
    fn capture_backend_short_alias_is_global_even_after_subcommand() {
        let invocation = parse_invocation(
            [
                "--json",
                "capture",
                "--out",
                "frame.png",
                "--backend",
                "adb",
                "--require-fresh",
            ],
            true,
        )
        .expect("invocation");

        assert_eq!(
            invocation.global.capture_backend,
            Some(CaptureBackendChoice::Adb)
        );
        assert_eq!(invocation.command, ["capture"]);
        assert_eq!(invocation.args, ["--out", "frame.png", "--require-fresh"]);
    }

    #[test]
    fn touch_backend_flag_is_global_even_after_subcommand() {
        let invocation = parse_invocation(
            [
                "--json",
                "tap",
                "10",
                "20",
                "--touch-backend",
                "adb_shell_input",
            ],
            true,
        )
        .expect("invocation");

        assert_eq!(
            invocation.global.touch_backend,
            Some(TouchBackendChoice::AdbShellInput)
        );
        assert_eq!(invocation.command, ["tap"]);
        assert_eq!(invocation.args, ["10", "20"]);
    }

    #[test]
    fn help_lists_capture_backend_short_alias() {
        let help = help_data();
        assert!(
            help.get("global_options")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .any(|option| option
                    .as_str()
                    .is_some_and(|text| text.starts_with("--backend ")))
        );
    }

    #[test]
    fn help_lists_resource_convert_maa_tasks_option() {
        let help = help_data();
        let options = help
            .pointer("/command_options/resource convert")
            .and_then(Value::as_array)
            .expect("resource convert options");

        assert!(
            options
                .iter()
                .any(|option| option.as_str() == Some("--maa-tasks <dir>"))
        );
    }

    #[test]
    fn help_lists_required_external_authoring_metadata() {
        let help = help_data();
        assert_eq!(
            help.pointer("/command_options/resource compile-maa/0")
                .and_then(Value::as_str),
            Some("--maa-tasks <dir>")
        );
        assert_eq!(
            help.pointer("/command_options/session record build-task/0")
                .and_then(Value::as_str),
            Some("--locale <locale>")
        );
    }

    #[test]
    fn help_documents_recognize_target_shared_output_shape() {
        let help = help_data();
        let note = help
            .pointer("/compatibility_notes/recognize --target")
            .and_then(Value::as_str)
            .expect("recognize note");

        assert!(note.contains("width"));
        assert!(note.contains("height"));
        assert!(note.contains("matched_rect"));
    }

    #[test]
    fn bare_instance_argument_is_used_as_adb_serial_without_config_entry() {
        let global = GlobalOptions {
            instance: Some("127.0.0.1:16416".to_string()),
            ..Default::default()
        };
        let (_adb_dir, config) = user_config_with_test_adb();
        let resolved = device_config(&global, &config).expect("device config");
        assert_eq!(resolved.target.serial.as_deref(), Some("127.0.0.1:16416"));
    }

    #[test]
    fn device_config_uses_instance_capture_backend_default() {
        let global = GlobalOptions {
            instance: Some("fixture-b".to_string()),
            ..Default::default()
        };
        let (_adb_dir, mut config) = user_config_with_test_adb();
        config.instances.insert(
            "fixture-b".to_string(),
            InstanceConfig {
                serial: Some("127.0.0.1:16416".to_string()),
                capture_backend: Some("nemu_ipc".to_string()),
                ..Default::default()
            },
        );

        let resolved = device_config(&global, &config).expect("device config");

        assert_eq!(resolved.target.serial.as_deref(), Some("127.0.0.1:16416"));
        assert_eq!(resolved.capture_backend, CaptureBackendChoice::NemuIpc);
    }

    #[test]
    fn device_config_cli_capture_backend_overrides_instance_default() {
        let global = GlobalOptions {
            instance: Some("fixture-b".to_string()),
            capture_backend: Some(CaptureBackendChoice::Adb),
            ..Default::default()
        };
        let (_adb_dir, mut config) = user_config_with_test_adb();
        config.instances.insert(
            "fixture-b".to_string(),
            InstanceConfig {
                serial: Some("127.0.0.1:16416".to_string()),
                capture_backend: Some("nemu_ipc".to_string()),
                ..Default::default()
            },
        );

        let resolved = device_config(&global, &config).expect("device config");

        assert_eq!(resolved.capture_backend, CaptureBackendChoice::Adb);
    }

    #[test]
    fn device_config_cli_touch_backend_overrides_instance_default() {
        let global = GlobalOptions {
            instance: Some("fixture-b".to_string()),
            touch_backend: Some(TouchBackendChoice::AdbShellInput),
            ..Default::default()
        };
        let (_adb_dir, mut config) = user_config_with_test_adb();
        config.instances.insert(
            "fixture-b".to_string(),
            InstanceConfig {
                serial: Some("127.0.0.1:16416".to_string()),
                touch_backend: Some("maatouch".to_string()),
                ..Default::default()
            },
        );

        let resolved = device_config(&global, &config).expect("device config");

        assert_eq!(resolved.touch_backend, TouchBackendChoice::AdbShellInput);
    }

    #[test]
    fn current_page_resolves_semantic_page() {
        let _guard = env_lock();
        unsafe {
            set_missing_config_env();
            env::remove_var(SESSION_STATE_ENV);
        }
        let temp = semantic_resource_root(false);
        let scene = temp.path().join("home.png");
        fs::write(&scene, encode_png(1, 1, [255, 0, 0])).unwrap();

        let result = run_semantic_cli(
            &temp,
            [
                "--json",
                "--resource-root",
                temp.path().to_str().unwrap(),
                "--game",
                "sample",
                "current-page",
                "--scene",
                scene.to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0);
        assert_eq!(
            result
                .envelope
                .data
                .as_ref()
                .unwrap()
                .get("page")
                .and_then(Value::as_str),
            Some("sample/home")
        );
    }

    #[test]
    fn tap_target_dry_run_requires_visible_target_and_returns_point() {
        let _guard = env_lock();
        unsafe {
            set_missing_config_env();
            env::remove_var(SESSION_STATE_ENV);
        }
        let temp = semantic_resource_root(false);
        let scene = temp.path().join("home.png");
        fs::write(&scene, encode_png(1, 1, [255, 0, 0])).unwrap();

        let result = run_semantic_cli(
            &temp,
            [
                "--json",
                "--resource-root",
                temp.path().to_str().unwrap(),
                "--game",
                "sample",
                "tap-target",
                "home_button",
                "--scene",
                scene.to_str().unwrap(),
                "--dry-run",
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0);
        let point = result.envelope.data.as_ref().unwrap().get("point").unwrap();
        assert_eq!(point.get("x").and_then(Value::as_i64), Some(12));
        assert_eq!(point.get("y").and_then(Value::as_i64), Some(23));
    }

    #[test]
    fn navigate_dry_run_uses_navigation_graph() {
        let temp = semantic_resource_root(false);
        let scene = temp.path().join("home.png");
        fs::write(&scene, encode_png(1, 1, [255, 0, 0])).unwrap();

        let result = run_semantic_cli(
            &temp,
            [
                "--json",
                "--dry-run",
                "--resource-root",
                temp.path().to_str().unwrap(),
                "--game",
                "sample",
                "navigate",
                "--to",
                "target",
                "--scene",
                scene.to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0);
        let route = result
            .envelope
            .data
            .as_ref()
            .unwrap()
            .get("route")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(route.len(), 1);
        assert_eq!(
            route[0].get("id").and_then(Value::as_str),
            Some("home_to_target")
        );
    }

    #[test]
    fn local_ledger_query_commands_fail_loud() {
        let result = run_cli(["--json", "ledger", "show"], true);

        assert_eq!(result.exit_code(), 6, "{}", result.envelope_json());
        let error = result.envelope.error.as_ref().unwrap();
        assert_eq!(error.code, "local_ledger_retired");
        assert!(error.message.contains("lab watch or lab receipt"));
    }

    #[test]
    fn navigate_blocks_destructive_overlap_by_default() {
        let temp = semantic_resource_root(true);
        let scene = temp.path().join("home.png");
        fs::write(&scene, encode_png(1, 1, [255, 0, 0])).unwrap();

        let result = run_semantic_cli(
            &temp,
            [
                "--json",
                "--dry-run",
                "--resource-root",
                temp.path().to_str().unwrap(),
                "--game",
                "sample",
                "navigate",
                "--to",
                "target",
                "--scene",
                scene.to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 3);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "navigation_destructive_overlap"
        );
    }

    #[test]
    fn lab2_observe_reports_page_targets_actions_and_frame_path() {
        let _guard = env_lock();
        let _app_env = set_isolated_app_env();
        unsafe {
            set_missing_config_env();
            env::remove_var(SESSION_STATE_ENV);
        }
        let temp = semantic_resource_root(false);
        let scene = temp.path().join("home.png");
        let frame_out = temp.path().join("observe-frame.png");
        fs::write(&scene, encode_png(1, 1, [255, 0, 0])).unwrap();

        let result = run_semantic_cli(
            &temp,
            [
                "--json",
                "--resource-root",
                temp.path().to_str().unwrap(),
                "--game",
                "sample",
                "observe",
                "--scene",
                scene.to_str().unwrap(),
                "--targets",
                "home_button",
                "--with-frame",
                frame_out.to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0);
        let data = result.envelope.data.as_ref().unwrap();
        assert!(data.get("req_id").and_then(Value::as_str).is_some());
        assert_eq!(
            data.get("page").and_then(Value::as_str),
            Some("sample/home")
        );
        assert_eq!(
            data.get("backend").and_then(Value::as_str),
            Some("scene_file")
        );
        assert_eq!(
            data.get("targets")
                .and_then(Value::as_array)
                .and_then(|targets| targets.first())
                .and_then(|target| target.get("id"))
                .and_then(Value::as_str),
            Some("home_button")
        );
        assert_eq!(
            data.get("actions")
                .and_then(Value::as_array)
                .and_then(|actions| actions.first())
                .and_then(|action| action.get("id"))
                .and_then(Value::as_str),
            Some("home_to_target")
        );
        assert!(frame_out.exists());
        assert!(!result.envelope_json().contains("base64"));
    }

    #[test]
    fn lab2_do_dry_run_reports_guard_and_actual_click() {
        let _guard = env_lock();
        let _app_env = set_isolated_app_env();
        unsafe {
            set_missing_config_env();
            env::remove_var(SESSION_STATE_ENV);
        }
        let temp = semantic_resource_root(false);
        let scene = temp.path().join("home.png");
        fs::write(&scene, encode_png(1, 1, [255, 0, 0])).unwrap();

        let result = run_semantic_cli(
            &temp,
            [
                "--json",
                "--dry-run",
                "--resource-root",
                temp.path().to_str().unwrap(),
                "--game",
                "sample",
                "do",
                "home_button",
                "--scene",
                scene.to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0);
        let data = result.envelope.data.as_ref().unwrap();
        assert_eq!(data.get("executed").and_then(Value::as_bool), Some(false));
        assert_eq!(
            data.get("actual_click")
                .and_then(|value| value.get("point"))
                .and_then(|value| value.get("x"))
                .and_then(Value::as_i64),
            Some(12)
        );
        assert_eq!(
            data.get("guard_result")
                .and_then(|value| value.get("passed"))
                .and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn lab2_do_guard_miss_returns_actionable_error_details() {
        let _guard = env_lock();
        let _app_env = set_isolated_app_env();
        unsafe {
            set_missing_config_env();
            env::remove_var(SESSION_STATE_ENV);
        }
        let temp = semantic_resource_root(false);
        let scene = temp.path().join("target.png");
        fs::write(&scene, encode_png(1, 1, [0, 0, 255])).unwrap();

        let result = run_semantic_cli(
            &temp,
            [
                "--json",
                "--dry-run",
                "--resource-root",
                temp.path().to_str().unwrap(),
                "--game",
                "sample",
                "do",
                "home_button",
                "--scene",
                scene.to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 3);
        let error = result.envelope.error.as_ref().unwrap();
        assert_eq!(error.code, "target_not_visible");
        let details = error.details.as_ref().unwrap();
        assert!(details.get("req_id").and_then(Value::as_str).is_some());
        assert_eq!(
            details.get("error").and_then(Value::as_str),
            Some("resource_drift")
        );
        let hint = details
            .get("hint")
            .and_then(Value::as_str)
            .expect("resource drift hint");
        assert!(!hint.contains("retry"));
    }

    #[test]
    fn lab2_ensure_is_idempotent_and_plans_routes() {
        let _guard = env_lock();
        let _app_env = set_isolated_app_env();
        unsafe {
            set_missing_config_env();
            env::remove_var(SESSION_STATE_ENV);
        }
        let temp = semantic_resource_root(false);
        let home = temp.path().join("home.png");
        fs::write(&home, encode_png(1, 1, [255, 0, 0])).unwrap();

        let idempotent = run_semantic_cli(
            &temp,
            [
                "--json",
                "--dry-run",
                "--resource-root",
                temp.path().to_str().unwrap(),
                "--game",
                "sample",
                "ensure",
                "home",
                "--scene",
                home.to_str().unwrap(),
            ],
            true,
        );
        assert_eq!(idempotent.exit_code(), 0);
        assert_eq!(
            idempotent
                .envelope
                .data
                .as_ref()
                .unwrap()
                .get("state")
                .and_then(Value::as_str),
            Some("already_at_target")
        );

        let planned = run_semantic_cli(
            &temp,
            [
                "--json",
                "--dry-run",
                "--resource-root",
                temp.path().to_str().unwrap(),
                "--game",
                "sample",
                "ensure",
                "target",
                "--scene",
                home.to_str().unwrap(),
            ],
            true,
        );
        assert_eq!(planned.exit_code(), 0);
        let route = planned
            .envelope
            .data
            .as_ref()
            .unwrap()
            .get("route")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(route.len(), 1);
        assert_eq!(
            route[0].get("id").and_then(Value::as_str),
            Some("home_to_target")
        );
    }

    #[test]
    fn lab2_wait_reports_page_and_stable_target() {
        let _guard = env_lock();
        let _app_env = set_isolated_app_env();
        unsafe {
            set_missing_config_env();
            env::remove_var(SESSION_STATE_ENV);
        }
        let temp = semantic_resource_root(false);
        let scene = temp.path().join("home.png");
        fs::write(&scene, encode_png(1, 1, [255, 0, 0])).unwrap();

        let page = run_semantic_cli(
            &temp,
            [
                "--json",
                "--resource-root",
                temp.path().to_str().unwrap(),
                "--game",
                "sample",
                "wait",
                "--page",
                "sample/home",
                "--scene",
                scene.to_str().unwrap(),
                "--timeout-ms",
                "100",
            ],
            true,
        );
        assert_eq!(page.exit_code(), 0);
        assert_eq!(
            page.envelope
                .data
                .as_ref()
                .unwrap()
                .get("state")
                .and_then(Value::as_str),
            Some("arrived")
        );

        let stable = run_semantic_cli(
            &temp,
            [
                "--json",
                "--resource-root",
                temp.path().to_str().unwrap(),
                "--game",
                "sample",
                "wait",
                "--stable",
                "home_anchor",
                "--scene",
                scene.to_str().unwrap(),
                "--timeout-ms",
                "100",
            ],
            true,
        );
        assert_eq!(stable.exit_code(), 0);
        assert_eq!(
            stable
                .envelope
                .data
                .as_ref()
                .unwrap()
                .get("state")
                .and_then(Value::as_str),
            Some("stable")
        );
        assert!(
            stable
                .envelope
                .data
                .as_ref()
                .unwrap()
                .get("wf_id")
                .and_then(Value::as_str)
                .is_some()
        );
    }

    #[test]
    fn lab2_capabilities_and_schema_report_compiled_contracts() {
        let _guard = env_lock();
        let _app_env = set_isolated_app_env();
        let temp = tempfile::tempdir().unwrap();
        set_config_env(temp.path().join("config.json"));
        let mut config = UserConfig::default();
        config.instances.insert(
            "fixture-d".to_string(),
            InstanceConfig {
                serial: Some("127.0.0.1:16416".to_string()),
                game: Some("sample".to_string()),
                server: Some("local".to_string()),
                capture_backend: Some("adb".to_string()),
                touch_backend: Some("maatouch".to_string()),
                ..Default::default()
            },
        );
        write_user_config(&config).unwrap();

        let capabilities = run_cli(["--json", "capabilities"], true);

        assert_eq!(capabilities.exit_code(), 0);
        let data = capabilities.envelope.data.as_ref().unwrap();
        assert!(
            data.pointer("/lab2_cli/verbs")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .any(|verb| verb.get("command").and_then(Value::as_str) == Some("do"))
        );
        assert!(
            data.pointer("/lab2_cli/engine_capabilities/template_matching/families")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .any(|family| family.get("id").and_then(Value::as_str) == Some("ccoeff"))
        );
        assert!(
            data.pointer("/lab2_cli/engine_capabilities/template_matching/unsupported")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .any(|capability| {
                    capability.get("id").and_then(Value::as_str) == Some("masked_template_match")
                })
        );
        assert_eq!(
            data.pointer("/lab2_cli/instances/0/id")
                .and_then(Value::as_str),
            Some("fixture-d")
        );
        assert_eq!(
            data.pointer("/lab2_cli/recovery_transparency/event_type")
                .and_then(Value::as_str),
            Some("recovery.state.changed")
        );

        let do_schema = run_cli(["--json", "schema", "do"], true);
        assert_eq!(do_schema.exit_code(), 0);
        assert_eq!(
            do_schema
                .envelope
                .data
                .as_ref()
                .unwrap()
                .get("command")
                .and_then(Value::as_str),
            Some("do")
        );

        let receipt_schema = run_cli(["--json", "schema", "lab", "receipt"], true);
        assert_eq!(receipt_schema.exit_code(), 0);
        assert_eq!(
            receipt_schema
                .envelope
                .data
                .as_ref()
                .unwrap()
                .get("command")
                .and_then(Value::as_str),
            Some("lab receipt")
        );
    }

    #[test]
    fn lab2_chain_acceptance_min_projection_and_error_shape_are_actionable() {
        let _guard = env_lock();
        let _app_env = set_isolated_app_env();
        unsafe {
            set_missing_config_env();
            env::remove_var(SESSION_STATE_ENV);
        }
        let temp = semantic_resource_root(false);
        let home = temp.path().join("home.png");
        let target = temp.path().join("target.png");
        fs::write(&home, encode_png(1, 1, [255, 0, 0])).unwrap();
        fs::write(&target, encode_png(1, 1, [0, 0, 255])).unwrap();

        let observe_started = Instant::now();
        let observe = run_semantic_cli(
            &temp,
            [
                "--json",
                "--resource-root",
                temp.path().to_str().unwrap(),
                "--game",
                "sample",
                "observe",
                "--scene",
                home.to_str().unwrap(),
                "--targets",
                "home_button",
            ],
            true,
        );
        let observe_elapsed = observe_started.elapsed();

        assert_eq!(observe.exit_code(), 0);
        let data = observe.envelope.data.as_ref().unwrap();
        assert!(
            serde_json::to_string(data).unwrap().len() <= 1024,
            "min projection data exceeded 1 KiB: {}",
            serde_json::to_string(data).unwrap().len()
        );
        assert!(
            observe_elapsed < Duration::from_millis(300),
            "synthetic observe took {observe_elapsed:?}"
        );
        assert!(!observe.envelope_json().contains('\u{1b}'));

        let error = run_semantic_cli(
            &temp,
            [
                "--json",
                "--dry-run",
                "--resource-root",
                temp.path().to_str().unwrap(),
                "--game",
                "sample",
                "do",
                "home_button",
                "--scene",
                target.to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(error.exit_code(), 3);
        let details = error
            .envelope
            .error
            .as_ref()
            .unwrap()
            .details
            .as_ref()
            .unwrap();
        assert!(details.get("req_id").and_then(Value::as_str).is_some());
        assert!(details.get("state").and_then(Value::as_str).is_some());
        assert!(details.get("hint").and_then(Value::as_str).is_some());
        assert!(!error.envelope_json().contains('\u{1b}'));
    }

    #[test]
    fn lab2_do_destructive_overlap_requires_opt_in_and_allows_explicit_opt_in() {
        let _guard = env_lock();
        let _app_env = set_isolated_app_env();
        unsafe {
            set_missing_config_env();
            env::remove_var(SESSION_STATE_ENV);
        }
        let temp = semantic_resource_root(true);
        let scene = temp.path().join("home.png");
        fs::write(&scene, encode_png(1, 1, [255, 0, 0])).unwrap();

        let destructive_without_allow = run_semantic_cli(
            &temp,
            [
                "--json",
                "--dry-run",
                "--resource-root",
                temp.path().to_str().unwrap(),
                "--game",
                "sample",
                "do",
                "home_button",
                "--scene",
                scene.to_str().unwrap(),
                "--destructive",
            ],
            true,
        );
        assert_eq!(destructive_without_allow.exit_code(), 3);
        assert_eq!(
            destructive_without_allow
                .envelope
                .error
                .as_ref()
                .unwrap()
                .code,
            "destructive_action_requires_allow_destructive"
        );

        let allowed = run_semantic_cli(
            &temp,
            [
                "--json",
                "--dry-run",
                "--resource-root",
                temp.path().to_str().unwrap(),
                "--game",
                "sample",
                "do",
                "home_button",
                "--scene",
                scene.to_str().unwrap(),
                "--destructive",
                "--allow-destructive",
            ],
            true,
        );

        assert_eq!(allowed.exit_code(), 0, "{}", allowed.envelope_json());
        assert_eq!(
            allowed
                .envelope
                .data
                .as_ref()
                .unwrap()
                .get("executed")
                .and_then(Value::as_bool),
            Some(false)
        );
    }

    #[test]
    fn lab2_evidence_lists_debug_evidence_refs() {
        let _guard = env_lock();
        let _app_env = set_isolated_app_env();
        unsafe {
            set_missing_config_env();
            env::remove_var(SESSION_STATE_ENV);
        }
        let temp = tempfile::tempdir().unwrap();
        let evidence_dir = temp.path().join("evidence").join("req-1");
        fs::create_dir_all(&evidence_dir).unwrap();
        fs::write(evidence_dir.join("frame-deadbeef.bin"), b"frame").unwrap();

        let result = run_cli(
            [
                "--json",
                "--run-root",
                temp.path().to_str().unwrap(),
                "lab",
                "evidence",
                "--id",
                "req-1",
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0);
        assert_eq!(
            result
                .envelope
                .data
                .as_ref()
                .unwrap()
                .get("count")
                .and_then(Value::as_u64),
            Some(1)
        );
    }

    #[test]
    fn lab2_observe_unknown_reports_candidates() {
        let _guard = env_lock();
        let _app_env = set_isolated_app_env();
        unsafe {
            set_missing_config_env();
            env::remove_var(SESSION_STATE_ENV);
        }
        let temp = semantic_resource_root(false);
        let scene = temp.path().join("unknown.png");
        fs::write(&scene, encode_png(1, 1, [12, 34, 56])).unwrap();

        let result = run_semantic_cli(
            &temp,
            [
                "--json",
                "--resource-root",
                temp.path().to_str().unwrap(),
                "--game",
                "sample",
                "observe",
                "--scene",
                scene.to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0);
        let data = result.envelope.data.as_ref().unwrap();
        assert_eq!(data.get("state").and_then(Value::as_str), Some("unknown"));
        assert_eq!(data.get("page").and_then(Value::as_str), Some("unknown"));
        assert!(
            data.get("candidates")
                .and_then(Value::as_array)
                .is_some_and(|items| !items.is_empty())
        );
        assert_eq!(
            data.pointer("/suspicion/reason").and_then(Value::as_str),
            Some("low_page_margin")
        );
    }

    #[test]
    fn lab2_do_click_rect_follows_live_template_match_delta() {
        let _guard = env_lock();
        let _app_env = set_isolated_app_env();
        unsafe {
            set_missing_config_env();
            env::remove_var(SESSION_STATE_ENV);
        }
        let temp = template_drift_resource_root();
        let scene = temp.path().join("shifted.png");
        fs::write(
            &scene,
            encode_rgb_png(3, 1, &[[0, 0, 0], [255, 0, 0], [0, 0, 0]]),
        )
        .unwrap();

        let result = run_semantic_cli(
            &temp,
            [
                "--json",
                "--dry-run",
                "--resource-root",
                temp.path().to_str().unwrap(),
                "--game",
                "sample",
                "do",
                "home_button",
                "--scene",
                scene.to_str().unwrap(),
            ],
            true,
        );

        if result.exit_code() != 0 {
            panic!("{}", result.envelope_json());
        }
        let click = result
            .envelope
            .data
            .as_ref()
            .unwrap()
            .get("actual_click")
            .unwrap();
        assert_eq!(
            click.get("kind").and_then(Value::as_str),
            Some("target_rect_center_live_match")
        );
        assert_eq!(click.pointer("/rect/x").and_then(Value::as_i64), Some(1));
        assert_eq!(
            click
                .pointer("/coordinate_derivation/matched_rect/x")
                .and_then(Value::as_i64),
            Some(1)
        );
    }

    #[test]
    fn lab2_do_rejects_mixed_online_capture_and_offline_scene_before_touch() {
        let _guard = env_lock();
        let _app_env = set_isolated_app_env();
        unsafe {
            set_missing_config_env();
            env::remove_var(SESSION_STATE_ENV);
            env::remove_var("ACTINGCOMMAND_TEST_FAKE_TOUCH_LOG");
        }
        let temp = semantic_resource_root(false);
        let scene = temp.path().join("target-drift.png");
        let touch_log = temp.path().join("fake-touch.json");
        fs::write(&scene, encode_png(1, 1, [0, 0, 255])).unwrap();
        unsafe {
            env::set_var("ACTINGCOMMAND_TEST_FAKE_TOUCH_LOG", &touch_log);
        }

        let result = run_semantic_cli(
            &temp,
            [
                "--json",
                "--instance",
                "default",
                "--resource-root",
                temp.path().to_str().unwrap(),
                "--game",
                "sample",
                "do",
                "home_button",
                "--scene",
                scene.to_str().unwrap(),
                "--capture",
                "--fields",
                "executed,device,actual_click,guard_result",
            ],
            true,
        );
        unsafe {
            env::remove_var("ACTINGCOMMAND_TEST_FAKE_TOUCH_LOG");
        }

        assert_eq!(result.exit_code(), 2, "{}", result.envelope_json());
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "validation_failed"
        );
        assert!(!touch_log.exists());
    }

    #[test]
    fn lab2_synthetic_cross_game_pack_runs_core_verbs_without_game_flag() {
        let _guard = env_lock();
        let _app_env = set_isolated_app_env();
        unsafe {
            set_missing_config_env();
            env::remove_var(SESSION_STATE_ENV);
        }
        let temp = synthetic_game_resource_root();
        let scene = temp.path().join("synthetic-home.png");
        fs::write(&scene, encode_png(1, 1, [10, 20, 30])).unwrap();
        let pack = temp.path().join("synthetic.pack.json");
        let pages = temp.path().join("synthetic.pages.json");
        let navigation = temp.path().join("synthetic.navigation.json");
        let shared = [
            "--pack",
            pack.to_str().unwrap(),
            "--pack-root",
            temp.path().to_str().unwrap(),
            "--pages",
            pages.to_str().unwrap(),
            "--navigation",
            navigation.to_str().unwrap(),
        ];

        let observe = run_semantic_cli(
            &temp,
            ["--json", "observe"]
                .into_iter()
                .chain(shared.iter().copied())
                .chain(["--scene", scene.to_str().unwrap()])
                .collect::<Vec<_>>(),
            true,
        );
        assert_eq!(observe.exit_code(), 0, "{}", observe.envelope_json());
        assert_eq!(
            observe
                .envelope
                .data
                .as_ref()
                .unwrap()
                .get("page")
                .and_then(Value::as_str),
            Some("synthetic/home")
        );

        let do_result = run_semantic_cli(
            &temp,
            [
                "--json",
                "--dry-run",
                "do",
                "synthetic_button",
                "--pack",
                pack.to_str().unwrap(),
                "--pack-root",
                temp.path().to_str().unwrap(),
                "--pages",
                pages.to_str().unwrap(),
                "--navigation",
                navigation.to_str().unwrap(),
                "--scene",
                scene.to_str().unwrap(),
            ],
            true,
        );
        assert_eq!(do_result.exit_code(), 0, "{}", do_result.envelope_json());

        let ensure = run_semantic_cli(
            &temp,
            [
                "--json",
                "--dry-run",
                "ensure",
                "synthetic/target",
                "--pack",
                pack.to_str().unwrap(),
                "--pack-root",
                temp.path().to_str().unwrap(),
                "--pages",
                pages.to_str().unwrap(),
                "--navigation",
                navigation.to_str().unwrap(),
                "--scene",
                scene.to_str().unwrap(),
            ],
            true,
        );
        assert_eq!(ensure.exit_code(), 0, "{}", ensure.envelope_json());

        let wait = run_semantic_cli(
            &temp,
            [
                "--json",
                "wait",
                "--page",
                "synthetic/home",
                "--pack",
                pack.to_str().unwrap(),
                "--pack-root",
                temp.path().to_str().unwrap(),
                "--pages",
                pages.to_str().unwrap(),
                "--navigation",
                navigation.to_str().unwrap(),
                "--scene",
                scene.to_str().unwrap(),
                "--timeout-ms",
                "100",
            ],
            true,
        );
        assert_eq!(wait.exit_code(), 0, "{}", wait.envelope_json());
    }

    #[test]
    fn lab2_observe_uses_delayed_stub_capture_and_stays_under_budget() {
        let _guard = env_lock();
        let _app_env = set_isolated_app_env();
        unsafe {
            set_missing_config_env();
            env::remove_var(SESSION_STATE_ENV);
        }
        let temp = semantic_resource_root(false);
        let scene = temp.path().join("home.png");
        fs::write(&scene, encode_png(1, 1, [255, 0, 0])).unwrap();
        let started = Instant::now();
        let result = run_semantic_cli(
            &temp,
            [
                "--json",
                "--resource-root",
                temp.path().to_str().unwrap(),
                "--game",
                "sample",
                "observe",
                "--scene",
                scene.to_str().unwrap(),
                "--test-capture-delay-ms",
                "25",
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
        assert!(started.elapsed() < Duration::from_millis(300));
        assert_eq!(
            result
                .envelope
                .data
                .as_ref()
                .unwrap()
                .get("backend")
                .and_then(Value::as_str),
            Some("test_stub_capture")
        );
    }

    #[test]
    fn session_recover_standby_dry_run_uses_wake_control_point() {
        let temp = semantic_resource_root(false);
        let scene = temp.path().join("standby.png");
        fs::write(&scene, encode_png(1, 1, [1, 1, 1])).unwrap();

        let result = run_cli(
            [
                "--json",
                "--dry-run",
                "--resource-root",
                temp.path().to_str().unwrap(),
                "--game",
                "sample",
                "--server",
                "local",
                "session",
                "recover",
                "--scene",
                scene.to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0);
        let steps = result
            .envelope
            .data
            .as_ref()
            .unwrap()
            .get("steps")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(steps[0].get("type").and_then(Value::as_str), Some("wake"));
        let point = steps[0]
            .get("control_point")
            .and_then(|value| value.get("input"))
            .and_then(|value| value.get("point"))
            .unwrap();
        assert_eq!(point.get("x").and_then(Value::as_i64), Some(3));
        assert_eq!(point.get("y").and_then(Value::as_i64), Some(4));
    }

    #[test]
    fn session_recover_dry_run_plans_route_to_home() {
        let temp = semantic_resource_root(false);
        let scene = temp.path().join("target.png");
        fs::write(&scene, encode_png(1, 1, [0, 0, 255])).unwrap();

        let result = run_cli(
            [
                "--json",
                "--dry-run",
                "--resource-root",
                temp.path().to_str().unwrap(),
                "--game",
                "sample",
                "--server",
                "local",
                "session",
                "recover",
                "--to",
                "home",
                "--scene",
                scene.to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0);
        let route = result
            .envelope
            .data
            .as_ref()
            .unwrap()
            .get("route")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(
            route[0].get("id").and_then(Value::as_str),
            Some("target_to_home")
        );
    }

    #[test]
    fn session_recover_real_execution_requires_capture() {
        let temp = semantic_resource_root(false);
        let scene = temp.path().join("target.png");
        fs::write(&scene, encode_png(1, 1, [0, 0, 255])).unwrap();

        let result = run_cli(
            [
                "--json",
                "--resource-root",
                temp.path().to_str().unwrap(),
                "--game",
                "sample",
                "--server",
                "local",
                "session",
                "recover",
                "--scene",
                scene.to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 2);
        assert!(
            result
                .envelope
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("requires --capture")
        );
    }

    #[test]
    fn session_recover_startup_login_dry_run_reads_resource_file() {
        let temp = semantic_resource_root(false);
        write_startup_login_resource(temp.path());
        let scene = temp.path().join("standby.png");
        fs::write(&scene, encode_png(1, 1, [1, 1, 1])).unwrap();

        let result = run_cli(
            [
                "--json",
                "--dry-run",
                "--resource-root",
                temp.path().to_str().unwrap(),
                "--game",
                "sample",
                "--server",
                "local",
                "session",
                "recover",
                "--startup-login",
                "--to",
                "home",
                "--scene",
                scene.to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0, "{:?}", result.envelope.error);
        let data = result.envelope.data.as_ref().unwrap();
        assert_eq!(data.get("status").and_then(Value::as_str), Some("planned"));
        assert_eq!(
            data.pointer("/startup_login/actions_per_round/0/input/point/x")
                .and_then(Value::as_i64),
            Some(1205)
        );
        assert_eq!(
            data.pointer("/startup_login/actions_per_round/1/input/point/y")
                .and_then(Value::as_i64),
            Some(360)
        );
        assert_eq!(
            data.get("safety_gate").and_then(Value::as_str),
            Some("maintenance_login_only")
        );
    }

    #[test]
    fn session_recover_startup_login_missing_resource_is_fatal() {
        let temp = semantic_resource_root(false);
        let scene = temp.path().join("standby.png");
        fs::write(&scene, encode_png(1, 1, [1, 1, 1])).unwrap();

        let result = run_cli(
            [
                "--json",
                "--dry-run",
                "--resource-root",
                temp.path().to_str().unwrap(),
                "--game",
                "sample",
                "--server",
                "local",
                "session",
                "recover",
                "--startup-login",
                "--scene",
                scene.to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 3);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "startup_login_resource_missing"
        );
    }

    #[test]
    fn session_recover_startup_login_missing_coordinate_is_fatal() {
        let temp = semantic_resource_root(false);
        fs::write(
            temp.path().join("STARTUP-LOGIN.md"),
            "# startup\n| 推进/点击继续 | (640, 360) |\n",
        )
        .unwrap();
        let scene = temp.path().join("standby.png");
        fs::write(&scene, encode_png(1, 1, [1, 1, 1])).unwrap();

        let result = run_cli(
            [
                "--json",
                "--dry-run",
                "--resource-root",
                temp.path().to_str().unwrap(),
                "--game",
                "sample",
                "--server",
                "local",
                "session",
                "recover",
                "--startup-login",
                "--scene",
                scene.to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 3);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "startup_login_coordinate_missing"
        );
    }

    #[test]
    fn locate_template_returns_coordinates() {
        let temp = TempDir::new().unwrap();
        let scene = temp.path().join("scene.png");
        let template = temp.path().join("template.png");
        fs::write(&scene, encode_png(1, 1, [7, 8, 9])).unwrap();
        fs::write(&template, encode_png(1, 1, [7, 8, 9])).unwrap();

        let result = run_cli(
            [
                "--json",
                "locate",
                template.to_str().unwrap(),
                "--scene",
                scene.to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0);
        assert_eq!(
            result
                .envelope
                .data
                .as_ref()
                .unwrap()
                .get("x")
                .and_then(Value::as_i64),
            Some(0)
        );
    }

    fn write_test_zip(path: &Path, files: &[(&str, &[u8])]) {
        let file = File::create(path).unwrap();
        let mut zip = ZipWriter::new(file);
        let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, content) in files {
            zip.start_file(*name, options).unwrap();
            zip.write_all(content).unwrap();
        }
        zip.finish().unwrap();
    }

    fn write_startup_login_resource(root: &Path) {
        fs::write(
            root.join("STARTUP-LOGIN.md"),
            "# startup\n| **弹窗关闭 ×** | **(1205, 67)** |\n| 推进/点击继续 | (640, 360) |\n",
        )
        .unwrap();
    }

    fn encode_png(width: u32, height: u32, color: [u8; 3]) -> Vec<u8> {
        let mut scanlines = Vec::with_capacity((width * height * 3 + height) as usize);
        for _y in 0..height {
            scanlines.push(0);
            for _x in 0..width {
                scanlines.extend_from_slice(&color);
            }
        }

        let mut png = Vec::new();
        png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        write_chunk(&mut png, b"IHDR", &ihdr);

        let mut zlib = vec![0x78, 0x01];
        write_uncompressed_deflate(&mut zlib, &scanlines);
        zlib.extend_from_slice(&adler32(&scanlines).to_be_bytes());
        write_chunk(&mut png, b"IDAT", &zlib);
        write_chunk(&mut png, b"IEND", &[]);
        png
    }

    fn encode_rgb_png(width: u32, height: u32, pixels: &[[u8; 3]]) -> Vec<u8> {
        assert_eq!(pixels.len(), (width * height) as usize);
        let mut scanlines = Vec::with_capacity((width * height * 3 + height) as usize);
        for row in pixels.chunks(width as usize) {
            scanlines.push(0);
            for pixel in row {
                scanlines.extend_from_slice(pixel);
            }
        }

        let mut png = Vec::new();
        png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        write_chunk(&mut png, b"IHDR", &ihdr);

        let mut zlib = vec![0x78, 0x01];
        write_uncompressed_deflate(&mut zlib, &scanlines);
        zlib.extend_from_slice(&adler32(&scanlines).to_be_bytes());
        write_chunk(&mut png, b"IDAT", &zlib);
        write_chunk(&mut png, b"IEND", &[]);
        png
    }

    fn write_uncompressed_deflate(out: &mut Vec<u8>, data: &[u8]) {
        for (index, chunk) in data.chunks(65_535).enumerate() {
            let is_last = index == data.len().div_ceil(65_535) - 1;
            out.push(u8::from(is_last));
            let len = chunk.len() as u16;
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&(!len).to_le_bytes());
            out.extend_from_slice(chunk);
        }
    }

    fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let mut crc_input = Vec::with_capacity(kind.len() + data.len());
        crc_input.extend_from_slice(kind);
        crc_input.extend_from_slice(data);
        out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    }

    fn adler32(data: &[u8]) -> u32 {
        const MOD: u32 = 65_521;
        let mut a = 1_u32;
        let mut b = 0_u32;
        for byte in data {
            a = (a + u32::from(*byte)) % MOD;
            b = (b + a) % MOD;
        }
        (b << 16) | a
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xffff_ffff_u32;
        for byte in data {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                let mask = 0_u32.wrapping_sub(crc & 1);
                crc = (crc >> 1) ^ (0xedb8_8320 & mask);
            }
        }
        !crc
    }
}
