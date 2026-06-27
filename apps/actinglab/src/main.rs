// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_device::{
    Adb, AdbConfig, CaptureBackendChoice, CaptureBackendConfig, CaptureBackendName, DeviceTarget,
    Frame, HandshakeInfo, InputBackend, MaaTouchBackend, MaaTouchConfig, PixelFormat,
    combine_operation_and_close, create_capture_backend, resolve_adb_path,
};
use actingcommand_page_detector::{PageDetector, PageEvaluation, load_page_set_from_json_str};
use actingcommand_recognition::{MatchMetric, Rect as RecognitionRect, Scene, ScenePixelFormat};
use actingcommand_recognition_pack::{
    PackRect, RecognitionEvaluator, TargetEvaluation, TargetKind, load_pack_from_json_str,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::env;
use std::fs::{self, File};
use std::io::{self, IsTerminal, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use zip::write::FileOptions;
use zip::{ZipArchive, ZipWriter};

mod frame_store;
mod lab_run;
mod package_build;
mod resource_convert;

const SCHEMA_VERSION: &str = "0.2";
const RUNTIME_VERSION: &str = "runtime-embedded-p1g";
const CONFIG_ENV: &str = "ACTINGLAB_CONFIG_PATH";
const SESSION_STATE_ENV: &str = "ACTINGLAB_SESSION_STATE_DIR";
const SESSION_INFO_FILE: &str = "session.json";
const SESSION_HEARTBEAT_FILE: &str = "heartbeat.json";
const SESSION_STOP_FILE: &str = "stop.request";
const SESSION_REQUESTS_DIR: &str = "requests";
const SESSION_RESPONSES_DIR: &str = "responses";
const SESSION_REQUEST_VALUE_FLAGS: &[&str] = &[
    "--state-dir",
    "--request-timeout-ms",
    "--lease-holder",
    "--holder",
    "--lease-id",
];
const DANGEROUS_EXTENSIONS: &[&str] = &[
    "py", "exe", "bat", "cmd", "ps1", "sh", "js", "vbs", "msi", "dll", "scr", "com", "jar",
];
const MAX_PACKAGE_ZIP_ENTRY_BYTES: u64 = 256 * 1024 * 1024;
const MAX_PACKAGE_ZIP_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;

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
    envelope: Envelope,
    human: String,
    exit_code: i32,
}

impl CliResult {
    fn ok(command: String, data: Value, print_json: bool, human: String) -> Self {
        Self {
            print_json,
            envelope: Envelope::ok(command, data),
            human,
            exit_code: 0,
        }
    }

    fn err(command: String, err: CliError, print_json: bool) -> Self {
        let exit_code = err.exit_code();
        let human = format!("{}: {}", err.code, err.message);
        Self {
            print_json,
            envelope: Envelope::err(command, err),
            human,
            exit_code,
        }
    }

    fn exit_code(&self) -> i32 {
        self.exit_code
    }

    fn envelope_json(&self) -> String {
        serde_json::to_string_pretty(&self.envelope).unwrap_or_else(|err| {
            format!(r#"{{"ok":false,"error":"json_serialize_failed:{err}"}}"#)
        })
    }

    fn human_text(&self) -> String {
        self.human.clone()
    }
}

#[derive(Debug, Serialize)]
struct Envelope {
    schema_version: &'static str,
    cli_version: &'static str,
    runtime_version: &'static str,
    ok: bool,
    command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<EnvelopeError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    artifacts: Option<Value>,
}

impl Envelope {
    fn ok(command: String, data: Value) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            cli_version: env!("CARGO_PKG_VERSION"),
            runtime_version: RUNTIME_VERSION,
            ok: true,
            command,
            data: Some(data),
            error: None,
            run_id: None,
            artifacts: None,
        }
    }

    fn err(command: String, err: CliError) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            cli_version: env!("CARGO_PKG_VERSION"),
            runtime_version: RUNTIME_VERSION,
            ok: false,
            command,
            data: None,
            error: Some(EnvelopeError {
                code: err.code,
                message: err.message,
                blocked_by: err.blocked_by,
            }),
            run_id: None,
            artifacts: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EnvelopeError {
    code: String,
    message: String,
    blocked_by: Vec<String>,
}

#[derive(Debug, Clone)]
struct CliError {
    kind: ErrorKind,
    code: String,
    message: String,
    blocked_by: Vec<String>,
}

impl CliError {
    fn usage(message: impl Into<String>) -> Self {
        Self::new(
            ErrorKind::UsageValidation,
            "validation_failed",
            message,
            &[],
        )
    }

    fn package_invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::UsageValidation, "package_invalid", message, &[])
    }

    fn safety_blocked(code: &'static str, message: impl Into<String>, blocked_by: &[&str]) -> Self {
        Self::new(ErrorKind::SafetyBlocked, code, message, blocked_by)
    }

    fn instance(message: impl Into<String>) -> Self {
        Self::new(
            ErrorKind::DeviceInstance,
            "instance_not_found",
            message,
            &["instance"],
        )
    }

    fn device(message: impl Into<String>) -> Self {
        Self::new(
            ErrorKind::DeviceInstance,
            "device_error",
            message,
            &["device"],
        )
    }

    fn runtime_not_running(message: impl Into<String>) -> Self {
        Self::new(
            ErrorKind::RuntimeNotRunning,
            "runtime_not_running",
            message,
            &["running_runtime"],
        )
    }

    fn not_implemented(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(ErrorKind::NotImplemented, code, message, &[])
    }

    fn new(
        kind: ErrorKind,
        code: &'static str,
        message: impl Into<String>,
        blocked_by: &[&str],
    ) -> Self {
        Self {
            kind,
            code: code.to_string(),
            message: message.into(),
            blocked_by: blocked_by.iter().map(|value| value.to_string()).collect(),
        }
    }

    fn exit_code(&self) -> i32 {
        match self.kind {
            ErrorKind::UsageValidation => 2,
            ErrorKind::SafetyBlocked => 3,
            ErrorKind::DeviceInstance => 4,
            ErrorKind::RuntimeNotRunning => 5,
            ErrorKind::NotImplemented => 6,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum ErrorKind {
    UsageValidation,
    SafetyBlocked,
    DeviceInstance,
    RuntimeNotRunning,
    NotImplemented,
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
    version: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct UserConfig {
    adb_path: Option<String>,
    runtime_endpoint: Option<String>,
    run_root: Option<String>,
    resource_root: Option<String>,
    #[serde(default)]
    instances: BTreeMap<String, InstanceConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct InstanceConfig {
    serial: Option<String>,
    game: Option<String>,
    server: Option<String>,
    package: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionInfo {
    pid: u32,
    started_at_unix_ms: u64,
    state_dir: String,
    runtime_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionHeartbeat {
    pid: u32,
    updated_at_unix_ms: u64,
    state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionCommandRequest {
    request_id: String,
    command: String,
    global: SessionCommandGlobal,
    args: Vec<String>,
    #[serde(default)]
    lease: Option<SessionCommandLease>,
    created_at_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionCommandGlobal {
    instance: Option<String>,
    game: Option<String>,
    server: Option<String>,
    resource_root: Option<String>,
    capture_backend: Option<String>,
    dry_run: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionCommandResponse {
    request_id: String,
    command: String,
    ok: bool,
    data: Option<Value>,
    error: Option<EnvelopeError>,
    started_at_unix_ms: u64,
    completed_at_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionCommandLease {
    holder: String,
    #[serde(default)]
    lease_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionLease {
    instance: String,
    holder: String,
    #[serde(default)]
    lease_id: String,
    acquired_at_unix_ms: u64,
    #[serde(default)]
    updated_at_unix_ms: u64,
    #[serde(default)]
    preempted: bool,
    #[serde(default)]
    previous: Option<SessionLeasePrevious>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionLeasePrevious {
    holder: String,
    lease_id: String,
    acquired_at_unix_ms: u64,
    updated_at_unix_ms: u64,
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
    Coord { x: i32, y: i32 },
    Target { target: String },
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

struct SessionRecordBuildDraft {
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
            "--capture-backend" => {
                index += 1;
                let value = require_raw(&raw, index, "--capture-backend")?;
                global.capture_backend =
                    Some(CaptureBackendChoice::parse(&value).map_err(|err| {
                        (
                            "help".to_string(),
                            global.json,
                            CliError::usage(err.to_string()),
                        )
                    })?);
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
        "config" | "lab" | "package" | "operation" | "control" | "scheduler" | "resource"
        | "run" | "report" | "session" => rest.get(1).map(|_| 2).unwrap_or(1),
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
        [cmd] if cmd == "status" => require_runtime(&invocation.global).map(|data| {
            json!({
                "state": "running",
                "runtime": data,
            })
        }),
        [cmd] if cmd == "devices" => run_devices(&invocation.global),
        [cmd] if cmd == "schema" => run_schema(&invocation.args),
        [cmd] if cmd == "list" => run_list(&invocation.global, &invocation.args),
        [cmd] if cmd == "tap" => run_direct_touch(&invocation.global, cmd, &invocation.args),
        [cmd] if cmd == "swipe" => run_direct_touch(&invocation.global, cmd, &invocation.args),
        [cmd] if cmd == "long-tap" => run_direct_touch(&invocation.global, cmd, &invocation.args),
        [cmd] if cmd == "key" => run_direct_input(&invocation.global, cmd, &invocation.args),
        [cmd] if cmd == "text" => run_direct_input(&invocation.global, cmd, &invocation.args),
        [cmd] if cmd == "capture" => run_capture(&invocation.global, &invocation.args),
        [cmd] if cmd == "detect-page" => run_detect_page(&invocation.global, &invocation.args),
        [cmd] if cmd == "recognize" => run_recognize(&invocation.global, &invocation.args),
        [cmd] if cmd == "current-page" => run_current_page(&invocation.global, &invocation.args),
        [cmd] if cmd == "is-visible" => run_is_visible(&invocation.global, &invocation.args),
        [cmd] if cmd == "locate" => run_locate(&invocation.global, &invocation.args),
        [cmd] if cmd == "tap-target" => run_tap_target(&invocation.global, &invocation.args),
        [cmd] if cmd == "navigate" => run_navigate(&invocation.global, &invocation.args),
        [cmd] if cmd == "monitor" => run_monitor(&invocation.global, &invocation.args),
        [cmd] if cmd == "stream" => run_stream(&invocation.global, &invocation.args),
        [cmd] if cmd == "record" => run_session_record(&invocation.global, &invocation.args),
        [cmd] if cmd == "explain" => run_explain_run(&invocation.args),
        [group, sub] if group == "config" => run_config(sub, &invocation.args),
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
            "--capture-backend <auto|adb|droidcast_raw|nemu_ipc>",
            "--dry-run",
            "--verbose",
            "--quiet",
            "--version"
        ],
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
    let adb_check = match adb_resolution {
        Ok(resolved) => json!({
            "name": "adb",
            "ok": true,
            "path": resolved.path,
            "source": resolved.source.as_str()
        }),
        Err(err) => json!({
            "name": "adb",
            "ok": false,
            "error": err.to_string(),
            "required_env": "ACTINGCOMMAND_ADB_PATH",
            "mumu_env": "ACTINGCOMMAND_NEMU_FOLDER"
        }),
    };
    checks.push(adb_check);
    checks.push(json!({
        "name": "runtime_endpoint",
        "ok": runtime_endpoint.as_ref().map(|endpoint| runtime_tcp_available(endpoint)).unwrap_or(false),
        "endpoint": runtime_endpoint
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
    Ok(json!({
        "commands": command_capabilities(),
        "exit_codes": exit_code_table(),
        "recognition_match_policy": [
            {"family": "BAAH", "game": "ba", "match_metric": "ccoeff_normed"},
            {"family": "MAA", "game": "ark", "match_metric": "ccoeff_normed"},
            {"family": "Alas", "game": "azur", "match_metric": "ccorr_normed+color"}
        ],
        "capture_backends": [
            {"id": "adb", "backend": "adb_screencap", "external_tool": false},
            {"id": "droidcast_raw", "backend": "droidcast_raw", "external_tool_env": "ACTINGCOMMAND_DROIDCAST_RAW_APK"},
            {"id": "nemu_ipc", "backend": "nemu_ipc", "external_tool_env": "ACTINGCOMMAND_NEMU_FOLDER or ACTINGCOMMAND_NEMU_IPC_DLL"},
            {"id": "auto", "fallback_allowed": true, "diagnostics_required": true}
        ],
        "discovered_recognition_packs": discovered
    }))
}

fn run_devices(_global: &GlobalOptions) -> CliOutcome<Value> {
    let config = read_user_config()?;
    let resolved = effective_adb_path(&config)?;
    let adb = Adb::new(AdbConfig {
        adb_path: resolved.path.clone(),
        ..Default::default()
    });
    let output = adb
        .run(&["devices", "-l"])
        .map_err(|err| CliError::device(err.to_string()))?;
    Ok(json!({
        "adb_stdout": output.stdout,
        "adb_stderr": output.stderr,
        "adb_path": resolved.path,
        "adb_source": resolved.source.as_str()
    }))
}

fn run_schema(args: &[String]) -> CliOutcome<Value> {
    let kind = args.first().map(String::as_str).unwrap_or("all");
    let data = match kind {
        "task" => json!({
            "schema_version": "0.1",
            "required": ["schema_version", "id", "steps"],
            "step_action_types": ["complete", "click"]
        }),
        "control" => json!({
            "schema_version": "Lab-1y.control.v1",
            "execution_modes": ["navigable_route", "recognize_only", "in_page_guard"],
            "capture_backend": ["auto", "adb", "droidcast_raw", "nemu_ipc"],
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
            "schema_version": ["0.1", "0.3"],
            "default_match_metric": "ccorr_normed",
            "supported_match_metric": ["ccorr_normed", "ccoeff_normed"]
        }),
        "package" => json!({
            "schema_version": "0.2",
            "required_paths": ["<module>/manifest.json", "<module>/operations/<task_id>/task.json"],
            "security": ["no zip-slip", "no executable scripts", "hashes verified when declared"]
        }),
        "all" => json!({
            "schemas": ["task", "control", "pack", "package"]
        }),
        other => return Err(CliError::usage(format!("unknown schema kind: {other}"))),
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
    let out = flags.required_path("--out")?;
    let config = read_user_config()?;
    let device_config = device_config(global, &config)?;
    let requested = global.capture_backend.unwrap_or_default();
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
        "capture_backend_attempts": captured.attempts,
        "freshness": captured.freshness,
        "out": out.display().to_string()
    }))
}

fn run_capture_diagnose(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    if flags.bool("--via-daemon") {
        return submit_capture_diagnose_request(global, flags);
    }
    let config = read_user_config()?;
    let device_config = device_config(global, &config)?;
    let requested = global.capture_backend.unwrap_or_default();
    let fresh_delay = parse_optional_duration_ms(flags, "--fresh-delay-ms", 160)?;
    let report = capture_fresh_probe_report(&device_config, requested, fresh_delay)?;
    Ok(json!({
        "status": report.status.as_str(),
        "mode": "capture_diagnose",
        "requested_backend": requested.as_str(),
        "click_allowed": false,
        "action_executed": false,
        "freshness": report.freshness,
        "capture_backend_attempts": report.attempts,
        "frame": report.frame.as_ref().map(capture_frame_summary_json),
        "recovery": capture_diagnosis_recovery_json(report.status, requested)
    }))
}

fn submit_capture_diagnose_request(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    let mut args = vec!["diagnose".to_string()];
    push_optional_flag_value(&mut args, flags, "--fresh-delay-ms");
    submit_session_command_request(global, flags, "capture_diagnose", args)
}

fn submit_readonly_session_request(
    global: &GlobalOptions,
    flags: &FlagArgs,
    command: &str,
    args: &[String],
) -> CliOutcome<Value> {
    submit_session_command_request(global, flags, command, session_request_payload_args(args))
}

fn submit_control_session_request(
    global: &GlobalOptions,
    flags: &FlagArgs,
    command: &str,
    args: &[String],
) -> CliOutcome<Value> {
    submit_session_command_request(global, flags, command, session_request_payload_args(args))
}

fn session_request_payload_args(args: &[String]) -> Vec<String> {
    let mut payload = Vec::new();
    let mut index = 0usize;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--via-daemon" {
            index += 1;
            continue;
        }
        if SESSION_REQUEST_VALUE_FLAGS.contains(&arg.as_str()) {
            index += if index + 1 < args.len() && !args[index + 1].starts_with("--") {
                2
            } else {
                1
            };
            continue;
        }
        payload.push(arg.clone());
        index += 1;
    }
    payload
}

fn session_command_lease_from_flags(flags: &FlagArgs) -> Option<SessionCommandLease> {
    let holder = flags
        .optional("--lease-holder")
        .or_else(|| flags.optional("--holder"))
        .filter(|value| value != "true");
    let lease_id = flags.optional("--lease-id").filter(|value| value != "true");
    if holder.is_none() && lease_id.is_none() {
        return None;
    }
    Some(SessionCommandLease {
        holder: holder.unwrap_or_default(),
        lease_id,
    })
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
    StaleSuspected,
    Unavailable,
}

impl CaptureFreshProbeStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::StaleSuspected => "stale_suspected",
            Self::Unavailable => "capture_unavailable",
        }
    }
}

struct MonitorSceneInput {
    scene: Scene,
    source: Value,
}

fn capture_for_command(
    device_config: &DeviceRuntimeConfig,
    requested: CaptureBackendChoice,
    require_fresh: bool,
    fresh_delay: Duration,
) -> CliOutcome<CaptureCommandResult> {
    if require_fresh {
        return capture_require_fresh(device_config, requested, fresh_delay);
    }

    let selected = create_capture_backend(device_config.capture_backend_config(requested))
        .map_err(|err| CliError::device(err.to_string()))?;
    let attempts = capture_attempts_json(&selected.diagnostics.attempts);
    let mut backend = selected.backend;
    let frame = backend
        .capture()
        .map_err(|err| CliError::device(err.to_string()))?;
    Ok(CaptureCommandResult {
        frame,
        attempts,
        freshness: json!({ "required": false }),
    })
}

fn capture_require_fresh(
    device_config: &DeviceRuntimeConfig,
    requested: CaptureBackendChoice,
    fresh_delay: Duration,
) -> CliOutcome<CaptureCommandResult> {
    let report = capture_fresh_probe_report(device_config, requested, fresh_delay)?;
    if let Some(frame) = report.frame {
        return Ok(CaptureCommandResult {
            frame,
            attempts: report.attempts,
            freshness: report.freshness,
        });
    }

    Err(CliError::device(format!(
        "fresh capture required but no backend produced a changing probe frame; attempts={}",
        serde_json::to_string(&report.attempts).unwrap_or_else(|_| "[]".to_string())
    )))
}

fn capture_fresh_probe_report(
    device_config: &DeviceRuntimeConfig,
    requested: CaptureBackendChoice,
    fresh_delay: Duration,
) -> CliOutcome<CaptureFreshProbeReport> {
    let mut attempts = Vec::new();
    let mut stale_suspected = false;
    for choice in fresh_probe_choices(requested) {
        let selected = match create_capture_backend(device_config.capture_backend_config(choice)) {
            Ok(selected) => selected,
            Err(err) => {
                attempts.push(json!({
                    "backend": choice.as_str(),
                    "ok": false,
                    "stage": "create",
                    "message": err.to_string()
                }));
                continue;
            }
        };
        let backend_used = selected.diagnostics.used.as_str();
        attempts.extend(capture_attempts_json(&selected.diagnostics.attempts));
        let mut backend = selected.backend;
        let first = match backend.capture() {
            Ok(frame) => frame,
            Err(err) => {
                attempts.push(json!({
                    "backend": backend_used,
                    "ok": false,
                    "stage": "first_capture",
                    "message": err.to_string()
                }));
                continue;
            }
        };
        thread::sleep(fresh_delay);
        let second = match backend.capture() {
            Ok(frame) => frame,
            Err(err) => {
                attempts.push(json!({
                    "backend": backend_used,
                    "ok": false,
                    "stage": "second_capture",
                    "message": err.to_string()
                }));
                continue;
            }
        };
        let first_hash = frame_digest(&first);
        let second_hash = frame_digest(&second);
        let fresh = first_hash != second_hash;
        stale_suspected |= !fresh;
        attempts.push(json!({
            "backend": backend_used,
            "ok": fresh,
            "stage": "fresh_probe",
            "first_hash": first_hash,
            "second_hash": second_hash,
            "stale_suspected": !fresh,
            "delay_ms": fresh_delay.as_millis()
        }));
        if fresh {
            return Ok(CaptureFreshProbeReport {
                status: CaptureFreshProbeStatus::Fresh,
                frame: Some(second),
                attempts,
                freshness: json!({
                    "required": true,
                    "fresh": true,
                    "backend": backend_used,
                    "first_hash": first_hash,
                    "second_hash": second_hash
                }),
            });
        }
    }

    let status = if stale_suspected {
        CaptureFreshProbeStatus::StaleSuspected
    } else {
        CaptureFreshProbeStatus::Unavailable
    };
    Ok(CaptureFreshProbeReport {
        status,
        frame: None,
        attempts,
        freshness: json!({
            "required": true,
            "fresh": false,
            "status": status.as_str()
        }),
    })
}

fn capture_attempts_json(attempts: &[actingcommand_device::CaptureBackendAttempt]) -> Vec<Value> {
    attempts
        .iter()
        .map(|attempt| {
            json!({
                "backend": attempt.backend.as_str(),
                "ok": attempt.ok,
                "message": attempt.message
            })
        })
        .collect()
}

fn fresh_probe_choices(requested: CaptureBackendChoice) -> Vec<CaptureBackendChoice> {
    match requested {
        CaptureBackendChoice::Auto => vec![
            CaptureBackendChoice::NemuIpc,
            CaptureBackendChoice::DroidcastRaw,
            CaptureBackendChoice::Adb,
        ],
        other => vec![other],
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

fn capture_diagnosis_recovery_json(
    status: CaptureFreshProbeStatus,
    requested: CaptureBackendChoice,
) -> Value {
    match status {
        CaptureFreshProbeStatus::Fresh => json!({
            "needed": false,
            "available": false,
            "reason": "fresh_frame_observed"
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
        CaptureFreshProbeStatus::Unavailable => json!({
            "needed": true,
            "available": false,
            "reason": "capture_backend_unavailable",
            "blocked_by": ["capture_backend", "device"],
            "recommendations": [{
                "type": "device_health",
                "command": "session instance health",
                "reason": "capture could not obtain probe frames from any requested backend"
            }]
        }),
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

    fn run(&self, backend: &mut MaaTouchBackend) -> actingcommand_device::DeviceResult<()> {
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

fn handshake_json(handshake: HandshakeInfo) -> Value {
    json!({
        "max_contacts": handshake.max_contacts,
        "max_x": handshake.max_x,
        "max_y": handshake.max_y,
        "max_pressure": handshake.max_pressure,
        "pid": handshake.pid
    })
}

fn run_direct_touch(global: &GlobalOptions, command: &str, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    if flags.bool("--via-daemon") {
        return submit_control_session_request(global, &flags, command, args);
    }
    let command = DirectTouchCommand::parse(command, &flags)?;
    let config = read_user_config()?;
    let device_config = device_config(global, &config)?;
    let mut backend = MaaTouchBackend::new(
        device_config.adb,
        device_config.target,
        MaaTouchConfig::default(),
    );
    let serial = backend.serial().to_string();
    let device = backend
        .connect()
        .map_err(|err| CliError::device(err.to_string()))?;
    let handshake = backend.handshake_info().cloned();
    let operation = command.run(&mut backend);
    let close = backend.close();
    combine_operation_and_close(operation, close)
        .map_err(|err| CliError::device(err.to_string()))?;
    Ok(json!({
        "status": "sent",
        "backend": "maatouch",
        "control_mode": "direct_trusted_manual",
        "safety_gate": "not_required_for_manual_control",
        "serial": serial,
        "device_state": device.state,
        "screen_size": device.screen_size,
        "handshake": handshake.map(handshake_json),
        "action": command.to_json()
    }))
}

fn run_direct_input(global: &GlobalOptions, command: &str, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    if flags.bool("--via-daemon") {
        return submit_control_session_request(global, &flags, command, args);
    }
    let command = DirectInputCommand::parse(command, &flags)?;
    let config = read_user_config()?;
    let device_config = device_config(global, &config)?;
    let mut backend = MaaTouchBackend::new(
        device_config.adb,
        device_config.target,
        MaaTouchConfig::default(),
    );
    let serial = backend.serial().to_string();
    let device = backend
        .connect()
        .map_err(|err| CliError::device(err.to_string()))?;
    let handshake = backend.handshake_info().cloned();
    let operation = command.run(&mut backend);
    let close = backend.close();
    combine_operation_and_close(operation, close)
        .map_err(|err| CliError::device(err.to_string()))?;
    Ok(json!({
        "status": "sent",
        "backend": "maatouch",
        "control_mode": "direct_trusted_manual",
        "safety_gate": "not_required_for_manual_control",
        "serial": serial,
        "device_state": device.state,
        "screen_size": device.screen_size,
        "handshake": handshake.map(handshake_json),
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

    fn run(&self, backend: &mut MaaTouchBackend) -> actingcommand_device::DeviceResult<()> {
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

fn run_recognize(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    if flags.bool("--via-daemon") {
        return submit_readonly_session_request(global, &flags, "recognize", args);
    }
    let target = flags.required("--target")?;
    let config = read_user_config()?;
    let resources = recognition_resources(global, &config, &flags, false)?;
    let evaluator = load_evaluator(&resources.pack_path, &resources.pack_root)?;
    if is_click_only_target(&evaluator, &target)? {
        let click = evaluator
            .get_click_target(&target)
            .map_err(|err| CliError::usage(err.to_string()))?;
        return Ok(json!({
            "target": target,
            "kind": "click_only",
            "evaluated": false,
            "click": rect_json(click),
            "match_metric": match_metric_name(evaluator.default_match_metric())
        }));
    }
    let scene = load_scene_from_flags(global, &flags)?;
    let evaluation = evaluator
        .evaluate_target(&scene, &target)
        .map_err(|err| CliError::usage(err.to_string()))?;
    let template = evaluation.template.map(|template| {
        json!({
            "x": template.x,
            "y": template.y,
            "score": template.score,
            "raw_score": template.raw_score,
            "threshold": template.threshold
        })
    });
    let color = evaluation.color.map(|color| {
        json!({
            "distance": color.distance,
            "max_distance": color.max_distance,
            "mean": color.mean,
            "expected": color.expected
        })
    });
    Ok(json!({
        "target": target,
        "passed": evaluation.passed,
        "message": evaluation.message,
        "template": template,
        "color": color,
        "match_metric": match_metric_name(evaluator.default_match_metric())
    }))
}

fn run_detect_page(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    if flags.bool("--via-daemon") {
        return submit_readonly_session_request(global, &flags, "detect_page", args);
    }
    let config = read_user_config()?;
    let resources = recognition_resources(global, &config, &flags, true)?;
    let pages_path = resources
        .pages_path
        .as_ref()
        .ok_or_else(|| CliError::usage("detect-page requires --pages or --resource-root --game"))?;
    let (evaluator, detector) =
        load_evaluator_and_detector(&resources.pack_path, &resources.pack_root, pages_path)?;
    detector
        .validate(&evaluator)
        .map_err(|err| CliError::usage(err.to_string()))?;
    if flags.bool("--check-pages") {
        return Ok(json!({"check_pages": "passed"}));
    }
    let scene = load_scene_from_flags(global, &flags)?;
    let outcome = detect_current_page(&evaluator, &detector, &scene)?;
    Ok(page_detection_json(&outcome))
}

fn run_current_page(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    if flags.bool("--via-daemon") {
        return submit_readonly_session_request(global, &flags, "current_page", args);
    }
    let config = read_user_config()?;
    let (evaluator, detector) = load_semantic_detector(global, &config, &flags)?;
    let scene = load_scene_from_flags(global, &flags)?;
    let outcome = detect_current_page(&evaluator, &detector, &scene)?;
    Ok(page_detection_json(&outcome))
}

fn run_is_visible(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    if flags.bool("--via-daemon") {
        return submit_readonly_session_request(global, &flags, "is_visible", args);
    }
    let target = target_argument(&flags, "is-visible")?;
    let config = read_user_config()?;
    let resources = recognition_resources(global, &config, &flags, false)?;
    let evaluator = load_evaluator(&resources.pack_path, &resources.pack_root)?;
    if evaluator
        .target_kind(&target)
        .map_err(|err| CliError::usage(err.to_string()))?
        == TargetKind::ClickOnly
    {
        return Err(CliError::usage(format!(
            "target '{target}' is click-only and cannot be evaluated for visibility"
        )));
    }
    let scene = load_scene_from_flags(global, &flags)?;
    let evaluation = evaluator
        .evaluate_target(&scene, &target)
        .map_err(|err| CliError::usage(err.to_string()))?;
    Ok(json!({
        "target": target,
        "visible": evaluation.passed,
        "evaluation": target_eval_json(&evaluation),
        "match_metric": match_metric_name(evaluator.default_match_metric())
    }))
}

fn run_locate(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    if flags.bool("--via-daemon") {
        return submit_readonly_session_request(global, &flags, "locate", args);
    }
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

fn run_tap_target(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    if flags.bool("--via-daemon") {
        return submit_control_session_request(global, &flags, "tap_target", args);
    }
    let target = target_argument(&flags, "tap-target")?;
    let allow_destructive = flags.bool("--allow-destructive");
    let dry_run = global.dry_run || flags.bool("--dry-run");
    if !allow_destructive {
        reject_dangerous_semantic_id("target", &target)?;
    }
    if !dry_run && !flags.bool("--capture") {
        return Err(CliError::usage(
            "tap-target real execution requires --capture; use --dry-run with --scene for offline planning",
        ));
    }

    let config = read_user_config()?;
    let resources = recognition_resources(global, &config, &flags, false)?;
    let evaluator = load_evaluator(&resources.pack_path, &resources.pack_root)?;
    if evaluator
        .target_kind(&target)
        .map_err(|err| CliError::usage(err.to_string()))?
        == TargetKind::ClickOnly
    {
        return Err(CliError::usage(format!(
            "tap-target requires a visually evaluatable target; '{target}' is click-only"
        )));
    }
    let scene = load_scene_from_flags(global, &flags)?;
    let evaluation = evaluator
        .evaluate_target(&scene, &target)
        .map_err(|err| CliError::usage(err.to_string()))?;
    if !evaluation.passed {
        return Err(CliError::safety_blocked(
            "target_not_visible",
            format!(
                "target '{target}' did not pass recognition: {}",
                evaluation.message
            ),
            &["visible_target"],
        ));
    }
    let click = evaluator
        .get_click_target(&target)
        .map_err(|err| CliError::usage(err.to_string()))?;
    let point = rect_center(click)?;
    if dry_run {
        return Ok(json!({
            "status": "planned",
            "executed": false,
            "target": target,
            "click": rect_json(click),
            "point": point_json(point),
            "evaluation": target_eval_json(&evaluation),
            "safety_gate": "navigation_only_default"
        }));
    }

    let action_result = send_semantic_tap(global, &config, point)?;
    Ok(json!({
        "status": "sent",
        "executed": true,
        "target": target,
        "click": rect_json(click),
        "point": point_json(point),
        "evaluation": target_eval_json(&evaluation),
        "safety_gate": "navigation_only_default",
        "device": action_result
    }))
}

fn run_navigate(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    if flags.bool("--via-daemon") {
        return submit_control_session_request(global, &flags, "navigate", args);
    }
    let to = flags.required("--to")?;
    let allow_destructive = flags.bool("--allow-destructive");
    let dry_run = global.dry_run || flags.bool("--dry-run");
    if !dry_run && !flags.bool("--capture") {
        return Err(CliError::usage(
            "navigate real execution requires --capture; use --dry-run with --scene for route planning",
        ));
    }

    let config = read_user_config()?;
    let (evaluator, detector) = load_semantic_detector(global, &config, &flags)?;
    let graph = load_navigation_graph(global, &config, &flags)?;
    let scene = load_scene_from_flags(global, &flags)?;
    let start = detect_current_page(&evaluator, &detector, &scene)?;
    if start.standby {
        return Err(CliError::safety_blocked(
            "current_page_unknown",
            "navigate requires a matched current page before clicking",
            &["current_page"],
        ));
    }
    let target_page = canonical_navigation_page(&graph, &to);
    if start.page == target_page {
        return Ok(json!({
            "status": "already_at_target",
            "executed": false,
            "from": start.page,
            "to": target_page,
            "route": []
        }));
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
            "status": "planned",
            "executed": false,
            "from": start.page,
            "to": target_page,
            "route": route_json,
            "safety_gate": "navigation_only_default"
        }));
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
    let (executed, _) = execute_navigation_route(&execution, start.page, route)?;
    Ok(json!({
        "status": "arrived",
        "executed": true,
        "to": target_page,
        "steps": executed,
        "safety_gate": "navigation_only_default"
    }))
}

fn run_session_recover(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    if flags.bool("--via-daemon") {
        return submit_control_session_request(global, &flags, "recover", args);
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
    let resources = recognition_resources(global, config, flags, true)?;
    let pages_path = resources.pages_path.as_ref().ok_or_else(|| {
        CliError::usage("semantic page commands require --pages or --resource-root --game")
    })?;
    let (evaluator, detector) =
        load_evaluator_and_detector(&resources.pack_path, &resources.pack_root, pages_path)?;
    detector
        .validate(&evaluator)
        .map_err(|err| CliError::usage(err.to_string()))?;
    Ok((evaluator, detector))
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
        "template": evaluation.template.map(|template| {
            json!({
                "x": template.x,
                "y": template.y,
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
    let rects = semantic_input_rects(&edge.input);
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

fn send_semantic_tap(
    global: &GlobalOptions,
    config: &UserConfig,
    point: SemanticPoint,
) -> CliOutcome<Value> {
    send_semantic_input(
        global,
        config,
        &SemanticInput::Tap {
            rect: PackRect {
                x: point.x,
                y: point.y,
                width: 1,
                height: 1,
            },
            point,
        },
    )
}

fn send_semantic_input(
    global: &GlobalOptions,
    config: &UserConfig,
    input: &SemanticInput,
) -> CliOutcome<Value> {
    let device_config = device_config(global, config)?;
    let mut backend = MaaTouchBackend::new(
        device_config.adb,
        device_config.target,
        MaaTouchConfig::default(),
    );
    let serial = backend.serial().to_string();
    let device = backend
        .connect()
        .map_err(|err| CliError::device(err.to_string()))?;
    let handshake = backend.handshake_info().cloned();
    let operation = match input {
        SemanticInput::Tap { point, .. } => backend.tap(point.x, point.y),
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
        "backend": "maatouch",
        "control_mode": "semantic",
        "serial": serial,
        "device_state": device.state,
        "screen_size": device.screen_size,
        "handshake": handshake.map(handshake_json),
        "action": semantic_input_json(input)
    }))
}

struct NavigationExecutionContext<'a> {
    global: &'a GlobalOptions,
    flags: &'a FlagArgs,
    config: &'a UserConfig,
    evaluator: &'a RecognitionEvaluator,
    detector: &'a PageDetector,
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
        let device = send_semantic_input(ctx.global, ctx.config, &edge.input)?;
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
            "device": device,
            "arrived": page_detection_json(&arrived)
        }));
    }
    Ok((executed, current_page))
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
    let flags = FlagArgs::parse(args)?;
    if flags.bool("--via-daemon") {
        if flags.bool("--recover") {
            return submit_monitor_session_request(global, &flags, args);
        }
        if flags.bool("--once") {
            return submit_monitor_once_session_request(global, &flags, args);
        }
        return submit_monitor_session_request(global, &flags, args);
    }
    if flags.bool("--once") {
        return run_monitor_once(global, &flags);
    }
    run_monitor_loop(global, &flags)
}

fn run_stream(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let config = read_user_config()?;
    let instance_id = resolve_instance_id_for_flags(global, &config, &flags)?;
    Err(CliError::not_implemented(
        "stream_not_implemented",
        format!(
            "stream for instance {instance_id} is reserved for the future interactive frame/input channel; use capture, monitor, or daemon requests for current CLI workflows"
        ),
    ))
}

fn submit_monitor_once_session_request(
    global: &GlobalOptions,
    flags: &FlagArgs,
    args: &[String],
) -> CliOutcome<Value> {
    let mut payload = session_request_payload_args(args);
    if !payload.iter().any(|arg| arg == "--once") {
        payload.push("--once".to_string());
    }
    submit_session_command_request(global, flags, "monitor_once", payload)
}

fn submit_monitor_session_request(
    global: &GlobalOptions,
    flags: &FlagArgs,
    args: &[String],
) -> CliOutcome<Value> {
    submit_session_command_request(global, flags, "monitor", session_request_payload_args(args))
}

fn run_monitor_loop(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    let max_iterations = parse_optional_usize(flags, "--max-iterations", 1)?;
    if max_iterations == 0 {
        return Err(CliError::usage("--max-iterations must be greater than 0"));
    }
    let interval = parse_optional_duration_ms(flags, "--interval-ms", 1_000)?;
    let recover = flags.bool("--recover");
    let mut iterations = Vec::with_capacity(max_iterations);
    for index in 0..max_iterations {
        if index > 0 && !interval.is_zero() {
            thread::sleep(interval);
        }
        let diagnosis = run_monitor_once(global, flags)?;
        let recovery =
            if recover && diagnosis.get("status").and_then(Value::as_str) != Some("healthy") {
                let recover_args = monitor_recover_args(flags);
                Some(run_session_recover(global, &recover_args)?)
            } else {
                None
            };
        iterations.push(json!({
            "iteration": index + 1,
            "diagnosis": diagnosis,
            "recovery": recovery
        }));
    }
    Ok(json!({
        "status": "completed",
        "mode": "monitor_loop",
        "read_only": !recover,
        "recover_requested": recover,
        "click_allowed": recover && !global.dry_run,
        "scheduler_pause": false,
        "max_iterations": max_iterations,
        "interval_ms": interval.as_millis(),
        "iterations": iterations
    }))
}

fn monitor_recover_args(flags: &FlagArgs) -> Vec<String> {
    let mut args = Vec::new();
    let target = flags
        .optional("--to")
        .or_else(|| flags.optional("--expect"))
        .filter(|value| value != "true")
        .unwrap_or_else(|| "home".to_string());
    args.extend(["--to".to_string(), target]);
    push_optional_flag_value(&mut args, flags, "--scene");
    if flags.bool("--capture") {
        args.push("--capture".to_string());
    }
    if flags.bool("--require-fresh") {
        args.push("--require-fresh".to_string());
    }
    if flags.bool("--startup-login") {
        args.push("--startup-login".to_string());
    }
    push_optional_flag_value(&mut args, flags, "--startup-login-file");
    push_optional_flag_value(&mut args, flags, "--startup-max-rounds");
    push_optional_flag_value(&mut args, flags, "--startup-interval-ms");
    push_optional_flag_value(&mut args, flags, "--fresh-delay-ms");
    push_optional_flag_value(&mut args, flags, "--max-actions");
    push_optional_flag_value(&mut args, flags, "--step-timeout-ms");
    push_optional_flag_value(&mut args, flags, "--poll-ms");
    args
}

fn push_optional_flag_value(args: &mut Vec<String>, flags: &FlagArgs, name: &str) {
    if let Some(value) = flags.optional(name).filter(|value| value != "true") {
        args.extend([name.to_string(), value]);
    }
}

fn run_monitor_once(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    let config = read_user_config()?;
    let (evaluator, detector) = load_semantic_detector(global, &config, flags)?;
    let graph = load_navigation_graph(global, &config, flags)?;
    let input = load_monitor_scene_from_flags(global, flags)?;
    let outcome = detect_current_page(&evaluator, &detector, &input.scene)?;
    let expected_page = canonical_navigation_page(
        &graph,
        &flags
            .optional("--expect")
            .or_else(|| flags.optional("--to"))
            .filter(|value| value != "true")
            .unwrap_or_else(|| "home".to_string()),
    );
    let diagnosis = if outcome.matched && outcome.page == expected_page {
        MonitorDiagnosis::Healthy
    } else if outcome.standby {
        MonitorDiagnosis::Standby
    } else {
        MonitorDiagnosis::UnexpectedPage
    };
    Ok(json!({
        "status": diagnosis.status(),
        "mode": "monitor_once",
        "click_allowed": false,
        "expected_page": expected_page,
        "current_page": page_detection_json(&outcome),
        "scene_source": input.source,
        "recovery": monitor_recovery_json(&diagnosis, &graph, &outcome, &expected_page)
    }))
}

enum MonitorDiagnosis {
    Healthy,
    Standby,
    UnexpectedPage,
}

impl MonitorDiagnosis {
    fn status(&self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Standby => "standby",
            Self::UnexpectedPage => "unexpected_page",
        }
    }
}

fn monitor_recovery_json(
    diagnosis: &MonitorDiagnosis,
    graph: &NavigationGraph,
    outcome: &PageDetectionOutcome,
    expected_page: &str,
) -> Value {
    match diagnosis {
        MonitorDiagnosis::Healthy => json!({
            "needed": false,
            "available": false,
            "reason": "already_at_expected_page"
        }),
        MonitorDiagnosis::Standby => {
            if let Some(wake) = graph.control_points.get("wake") {
                json!({
                    "needed": true,
                    "available": true,
                    "reason": "standby",
                    "recommended_command": format!("session recover --to {expected_page} --capture"),
                    "steps": [{
                        "type": "wake",
                        "control_point": control_point_json(wake)
                    }]
                })
            } else {
                json!({
                    "needed": true,
                    "available": false,
                    "reason": "standby",
                    "blocked_by": ["control_point"],
                    "message": "navigation resources do not define control_points.wake"
                })
            }
        }
        MonitorDiagnosis::UnexpectedPage => {
            match safe_recovery_route(graph, &outcome.page, expected_page) {
                Ok(route) => json!({
                    "needed": true,
                    "available": true,
                    "reason": "unexpected_page",
                    "recommended_command": format!("session recover --to {expected_page} --capture"),
                    "route": route.iter().map(navigation_edge_json).collect::<Vec<_>>()
                }),
                Err(err) => json!({
                    "needed": true,
                    "available": false,
                    "reason": "unexpected_page",
                    "blocked_by": err.blocked_by,
                    "error_code": err.code,
                    "message": err.message
                }),
            }
        }
    }
}

fn run_lab(sub: &str, global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    match sub {
        "run" => lab_run::run_lab_run(global, args),
        "validate" => lab_run::run_lab_validate(args),
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
        "status" | "lease" | "release" => {
            require_runtime(global)?;
            Err(CliError::not_implemented(
                "not_implemented",
                "Runtime lab session API is reserved but not implemented yet",
            ))
        }
        _ => Err(CliError::usage(format!("unknown lab command: {sub}"))),
    }
}

fn run_package(sub: &str, global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    match sub {
        "validate" => {
            let zip = flags.required_path("--zip")?;
            Ok(package_validation_json(&validate_package_zip(&zip)?, false))
        }
        "inspect" => {
            let zip = flags.required_path("--zip")?;
            Ok(package_validation_json(&validate_package_zip(&zip)?, true))
        }
        "run" => {
            let zip = flags.required_path("--zip")?;
            let out = flags.optional_path("--out");
            let validation = validate_package_zip(&zip)?;
            if global.instance.is_none() && global.game.is_none() {
                return Err(CliError::instance(
                    "package run requires --instance or --game/--server selector",
                ));
            }
            let result_zip = out
                .map(|out| create_package_blocked_result_zip(&out, &validation))
                .transpose()?;
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
            ))
        }
        "build-task" => package_build::run_build_task(global, &flags),
        "build-pack" => package_build::run_build_pack(global, &flags),
        _ => Err(CliError::usage(format!("unknown package command: {sub}"))),
    }
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
        "run" => Err(CliError::safety_blocked(
            "lab_lease_required",
            "operation run requires navigation_only operations and an exclusive_drain LabLease",
            &["lab_lease", "exclusive_drain"],
        )),
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
        "status" => run_session_status(args),
        "start" => run_session_start(args),
        "stop" => run_session_stop(args),
        "daemon" => run_session_daemon(args),
        "request" => run_session_request(global, args),
        "instance" => run_session_instance(global, args),
        "app" => run_session_app(global, args),
        "capture" => run_capture(global, args),
        "recover" => run_session_recover(global, args),
        "lease" => run_session_lease(global, args),
        "record" => run_session_record(global, args),
        _ => Err(CliError::usage(format!("unknown session command: {sub}"))),
    }
}

fn run_session_status(args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let state_dir = session_state_dir_from_flags(&flags)?;
    let info_path = session_info_path(&state_dir);
    let heartbeat_path = session_heartbeat_path(&state_dir);
    let info = read_json_file::<SessionInfo>(&info_path)?;
    let heartbeat = read_json_file::<SessionHeartbeat>(&heartbeat_path)?;
    Ok(json!({
        "state_dir": state_dir.display().to_string(),
        "running": info.is_some(),
        "info": info,
        "heartbeat": heartbeat
    }))
}

fn run_session_request(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let command = args
        .first()
        .map(String::as_str)
        .ok_or_else(|| {
            CliError::usage(
                "session request requires capture-diagnose, recognize, detect-page, current-page, is-visible, locate, monitor, monitor-once, app, tap, swipe, long-tap, key, text, tap-target, navigate, or recover",
            )
        })?;
    let flags = FlagArgs::parse(&args[1..])?;
    match command {
        "capture-diagnose" => {
            let mut request_args = vec!["diagnose".to_string()];
            push_optional_flag_value(&mut request_args, &flags, "--fresh-delay-ms");
            submit_session_command_request(global, &flags, "capture_diagnose", request_args)
        }
        "recognize" => submit_readonly_session_request(global, &flags, "recognize", &args[1..]),
        "detect-page" => submit_readonly_session_request(global, &flags, "detect_page", &args[1..]),
        "current-page" => {
            submit_readonly_session_request(global, &flags, "current_page", &args[1..])
        }
        "is-visible" => submit_readonly_session_request(global, &flags, "is_visible", &args[1..]),
        "locate" => submit_readonly_session_request(global, &flags, "locate", &args[1..]),
        "monitor" => submit_monitor_session_request(global, &flags, &args[1..]),
        "monitor-once" => submit_monitor_once_session_request(global, &flags, &args[1..]),
        "app" => submit_control_session_request(global, &flags, "app", &args[1..]),
        "tap" | "swipe" | "long-tap" | "key" | "text" => {
            submit_control_session_request(global, &flags, command, &args[1..])
        }
        "tap-target" => submit_control_session_request(global, &flags, "tap_target", &args[1..]),
        "navigate" | "recover" => {
            submit_control_session_request(global, &flags, command, &args[1..])
        }
        other => Err(CliError::usage(format!(
            "unknown session request command: {other}"
        ))),
    }
}

fn submit_session_command_request(
    global: &GlobalOptions,
    flags: &FlagArgs,
    command: &str,
    args: Vec<String>,
) -> CliOutcome<Value> {
    let state_dir = session_state_dir_from_flags(flags)?;
    let info_path = session_info_path(&state_dir);
    let info = read_json_file::<SessionInfo>(&info_path)?.ok_or_else(|| {
        CliError::runtime_not_running(format!(
            "session daemon is not running; missing {}",
            info_path.display()
        ))
    })?;
    let request_id = format!("{}-{}", current_unix_ms(), std::process::id());
    let request = SessionCommandRequest {
        request_id: request_id.clone(),
        command: command.to_string(),
        global: SessionCommandGlobal::from_global(global),
        args,
        lease: session_command_lease_from_flags(flags),
        created_at_unix_ms: current_unix_ms(),
    };
    let request_path = session_requests_dir(&state_dir).join(format!("{request_id}.json"));
    let response_path = session_responses_dir(&state_dir).join(format!("{request_id}.json"));
    write_json_file_atomic(&request_path, &request)?;
    let timeout = parse_optional_duration_ms(flags, "--request-timeout-ms", 10_000)?;
    let started = Instant::now();
    while started.elapsed() <= timeout {
        if let Some(response) = read_json_file::<SessionCommandResponse>(&response_path)? {
            let _ = fs::remove_file(&response_path);
            if response.ok {
                return Ok(json!({
                    "status": "completed",
                    "mode": "daemon_request",
                    "state_dir": state_dir.display().to_string(),
                    "daemon_pid": info.pid,
                    "request_id": response.request_id,
                    "daemon_command": response.command,
                    "response": response.data
                }));
            }
            let error = response.error.ok_or_else(|| {
                CliError::runtime_not_running("daemon request failed without error details")
            })?;
            return Err(cli_error_from_envelope(error));
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err(CliError::runtime_not_running(format!(
        "session daemon request {request_id} timed out after {} ms",
        timeout.as_millis()
    )))
}

impl SessionCommandGlobal {
    fn from_global(global: &GlobalOptions) -> Self {
        Self {
            instance: global.instance.clone(),
            game: global.game.clone(),
            server: global.server.clone(),
            resource_root: global
                .resource_root
                .as_ref()
                .map(|path| path.display().to_string()),
            capture_backend: global
                .capture_backend
                .map(|backend| backend.as_str().to_string()),
            dry_run: global.dry_run,
        }
    }

    fn to_global(&self) -> CliOutcome<GlobalOptions> {
        let capture_backend = self
            .capture_backend
            .as_deref()
            .map(CaptureBackendChoice::parse)
            .transpose()
            .map_err(|err| CliError::usage(err.to_string()))?;
        Ok(GlobalOptions {
            instance: self.instance.clone(),
            game: self.game.clone(),
            server: self.server.clone(),
            resource_root: self.resource_root.as_ref().map(PathBuf::from),
            capture_backend,
            dry_run: self.dry_run,
            json: true,
            ..Default::default()
        })
    }
}

fn cli_error_from_envelope(error: EnvelopeError) -> CliError {
    match error.code.as_str() {
        "validation_failed" | "package_invalid" => CliError::usage(error.message),
        "runtime_not_running" => CliError::runtime_not_running(error.message),
        "instance_not_found" => CliError::instance(error.message),
        "device_error" => CliError::device(error.message),
        "not_implemented" => CliError::not_implemented("not_implemented", error.message),
        code if code.starts_with("navigation_")
            || code.starts_with("lab_lease")
            || code.starts_with("lease_")
            || code.contains("blocked") =>
        {
            let blocked = error
                .blocked_by
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>();
            CliError::safety_blocked("daemon_request_blocked", error.message, &blocked)
        }
        _ => CliError::runtime_not_running(format!(
            "daemon request failed with {}: {}",
            error.code, error.message
        )),
    }
}

#[cfg(windows)]
fn spawn_session_daemon(exe: &Path, state_dir: &Path) -> CliOutcome<()> {
    let state_dir = absolutize_path(state_dir);
    let command_text = format!(
        "$p = Start-Process -FilePath {} -ArgumentList {} -WindowStyle Hidden -PassThru; $p.Id",
        powershell_quote(&exe.display().to_string()),
        powershell_array(&[
            "--json".to_string(),
            "session".to_string(),
            "daemon".to_string(),
            "--state-dir".to_string(),
            state_dir.display().to_string(),
        ])
    );
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command"])
        .arg(command_text)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| {
            CliError::runtime_not_running(format!(
                "failed to invoke PowerShell Start-Process: {err}"
            ))
        })?;
    if !output.status.success() {
        return Err(CliError::runtime_not_running(format!(
            "PowerShell Start-Process failed with status {}; stdout={}; stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

#[cfg(not(windows))]
fn spawn_session_daemon(exe: &Path, state_dir: &Path) -> CliOutcome<()> {
    let stdout = File::create(state_dir.join("daemon.out.log")).map_err(|err| {
        CliError::runtime_not_running(format!("failed to create daemon stdout log: {err}"))
    })?;
    let stderr = File::create(state_dir.join("daemon.err.log")).map_err(|err| {
        CliError::runtime_not_running(format!("failed to create daemon stderr log: {err}"))
    })?;
    let _child = Command::new(exe)
        .arg("--json")
        .arg("session")
        .arg("daemon")
        .arg("--state-dir")
        .arg(state_dir)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .map_err(|err| CliError::runtime_not_running(format!("failed to start session: {err}")))?;
    Ok(())
}

fn absolutize_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

#[cfg(windows)]
fn powershell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(windows)]
fn powershell_array(values: &[String]) -> String {
    format!(
        "@({})",
        values
            .iter()
            .map(|value| powershell_quote(value))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn run_session_start(args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let state_dir = session_state_dir_from_flags(&flags)?;
    fs::create_dir_all(&state_dir).map_err(|err| {
        CliError::runtime_not_running(format!(
            "failed to create session state dir {}: {err}",
            state_dir.display()
        ))
    })?;

    let info_path = session_info_path(&state_dir);
    if let Some(info) = read_json_file::<SessionInfo>(&info_path)? {
        return Ok(json!({
            "status": "already_running",
            "state_dir": state_dir.display().to_string(),
            "info": info
        }));
    }

    let stop_path = session_stop_path(&state_dir);
    if stop_path.exists() {
        fs::remove_file(&stop_path).map_err(|err| {
            CliError::runtime_not_running(format!(
                "failed to remove stale stop request {}: {err}",
                stop_path.display()
            ))
        })?;
    }

    let exe = env::current_exe().map_err(|err| {
        CliError::runtime_not_running(format!("failed to resolve actinglab executable: {err}"))
    })?;
    spawn_session_daemon(&exe, &state_dir)?;

    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(2) {
        if let Some(info) = read_json_file::<SessionInfo>(&info_path)? {
            return Ok(json!({
                "status": "started",
                "state_dir": state_dir.display().to_string(),
                "spawned_pid": info.pid,
                "info": info
            }));
        }
        thread::sleep(Duration::from_millis(100));
    }

    Err(CliError::runtime_not_running(format!(
        "session daemon did not write {} within startup deadline",
        info_path.display()
    )))
}

fn run_session_stop(args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let state_dir = session_state_dir_from_flags(&flags)?;
    let info_path = session_info_path(&state_dir);
    let info = read_json_file::<SessionInfo>(&info_path)?;
    if info.is_none() {
        return Ok(json!({
            "status": "not_running",
            "state_dir": state_dir.display().to_string()
        }));
    }
    fs::create_dir_all(&state_dir).map_err(|err| {
        CliError::runtime_not_running(format!(
            "failed to create session state dir {}: {err}",
            state_dir.display()
        ))
    })?;
    fs::write(session_stop_path(&state_dir), current_unix_ms().to_string()).map_err(|err| {
        CliError::runtime_not_running(format!("failed to write session stop request: {err}"))
    })?;

    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(3) {
        if !info_path.exists() {
            return Ok(json!({
                "status": "stopped",
                "state_dir": state_dir.display().to_string()
            }));
        }
        thread::sleep(Duration::from_millis(100));
    }
    Ok(json!({
        "status": "stop_requested",
        "state_dir": state_dir.display().to_string(),
        "info": info
    }))
}

fn process_session_requests(state_dir: &Path) -> CliOutcome<usize> {
    let requests_dir = session_requests_dir(state_dir);
    if !requests_dir.exists() {
        return Ok(0);
    }
    let mut paths = fs::read_dir(&requests_dir)
        .map_err(|err| {
            CliError::runtime_not_running(format!(
                "failed to read session request dir {}: {err}",
                requests_dir.display()
            ))
        })?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    paths.sort();
    let mut processed = 0usize;
    for path in paths {
        let request = read_json_file::<SessionCommandRequest>(&path)?.ok_or_else(|| {
            CliError::runtime_not_running(format!(
                "session request disappeared before processing: {}",
                path.display()
            ))
        })?;
        let response = execute_session_command_request(request, state_dir);
        let response_path =
            session_responses_dir(state_dir).join(format!("{}.json", response.request_id));
        write_json_file_atomic(&response_path, &response)?;
        fs::remove_file(&path).map_err(|err| {
            CliError::runtime_not_running(format!(
                "failed to remove processed request {}: {err}",
                path.display()
            ))
        })?;
        processed += 1;
    }
    Ok(processed)
}

fn execute_session_command_request(
    request: SessionCommandRequest,
    state_dir: &Path,
) -> SessionCommandResponse {
    let started_at_unix_ms = current_unix_ms();
    let result = execute_session_command_request_inner(&request, state_dir);
    let completed_at_unix_ms = current_unix_ms();
    match result {
        Ok(data) => SessionCommandResponse {
            request_id: request.request_id,
            command: request.command,
            ok: true,
            data: Some(data),
            error: None,
            started_at_unix_ms,
            completed_at_unix_ms,
        },
        Err(err) => SessionCommandResponse {
            request_id: request.request_id,
            command: request.command,
            ok: false,
            data: None,
            error: Some(EnvelopeError {
                code: err.code,
                message: err.message,
                blocked_by: err.blocked_by,
            }),
            started_at_unix_ms,
            completed_at_unix_ms,
        },
    }
}

fn execute_session_command_request_inner(
    request: &SessionCommandRequest,
    state_dir: &Path,
) -> CliOutcome<Value> {
    match request.command.as_str() {
        "capture_diagnose" => {
            let global = request.global.to_global()?;
            let flags = FlagArgs::parse(&request.args)?;
            run_capture_diagnose(&global, &flags)
        }
        "recognize" => {
            let global = request.global.to_global()?;
            run_recognize(&global, &request.args)
        }
        "detect_page" => {
            let global = request.global.to_global()?;
            run_detect_page(&global, &request.args)
        }
        "current_page" => {
            let global = request.global.to_global()?;
            run_current_page(&global, &request.args)
        }
        "is_visible" => {
            let global = request.global.to_global()?;
            run_is_visible(&global, &request.args)
        }
        "locate" => {
            let global = request.global.to_global()?;
            run_locate(&global, &request.args)
        }
        "monitor_once" => {
            let global = request.global.to_global()?;
            let flags = FlagArgs::parse(&request.args)?;
            if flags.bool("--recover") {
                return Err(CliError::safety_blocked(
                    "daemon_recovery_requires_lease",
                    "monitor-once daemon requests are read-only; use monitor --recover with a session lease",
                    &["lab_lease", "monitor_recovery"],
                ));
            }
            run_monitor_once(&global, &flags)
        }
        "monitor" => {
            let global = request.global.to_global()?;
            let flags = FlagArgs::parse(&request.args)?;
            if flags.bool("--recover") {
                ensure_session_request_lease(state_dir, request)?;
            }
            run_monitor_loop(&global, &flags)
        }
        "app" => {
            ensure_session_request_lease(state_dir, request)?;
            let global = request.global.to_global()?;
            run_session_app(&global, &request.args)
        }
        "tap" | "swipe" | "long-tap" => {
            ensure_session_request_lease(state_dir, request)?;
            let global = request.global.to_global()?;
            run_direct_touch(&global, &request.command, &request.args)
        }
        "key" | "text" => {
            ensure_session_request_lease(state_dir, request)?;
            let global = request.global.to_global()?;
            run_direct_input(&global, &request.command, &request.args)
        }
        "tap_target" => {
            ensure_session_request_lease(state_dir, request)?;
            let global = request.global.to_global()?;
            run_tap_target(&global, &request.args)
        }
        "navigate" => {
            ensure_session_request_lease(state_dir, request)?;
            let global = request.global.to_global()?;
            run_navigate(&global, &request.args)
        }
        "recover" => {
            ensure_session_request_lease(state_dir, request)?;
            let global = request.global.to_global()?;
            run_session_recover(&global, &request.args)
        }
        other => Err(CliError::usage(format!(
            "unsupported daemon request command: {other}"
        ))),
    }
}

fn ensure_session_request_lease(
    state_dir: &Path,
    request: &SessionCommandRequest,
) -> CliOutcome<SessionLease> {
    let requested = request
        .lease
        .as_ref()
        .filter(|lease| !lease.holder.is_empty())
        .ok_or_else(|| {
            CliError::safety_blocked(
                "lab_lease_required",
                format!(
                    "daemon control request '{}' requires --lease-holder <id>",
                    request.command
                ),
                &["lab_lease", "lease_holder"],
            )
        })?;
    let instance_id = session_command_instance_id(&request.global);
    let lease_path = session_lease_path(state_dir, &instance_id);
    let Some(current) = read_json_file::<SessionLease>(&lease_path)? else {
        return Err(CliError::safety_blocked(
            "lab_lease_missing",
            format!("daemon control request requires an active lease for {instance_id}"),
            &["lab_lease"],
        ));
    };
    validate_lease_request(&current, requested)?;
    Ok(current)
}

fn run_session_daemon(args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let state_dir = session_state_dir_from_flags(&flags)?;
    fs::create_dir_all(&state_dir).map_err(|err| {
        CliError::runtime_not_running(format!(
            "failed to create session state dir {}: {err}",
            state_dir.display()
        ))
    })?;
    let info = SessionInfo {
        pid: std::process::id(),
        started_at_unix_ms: current_unix_ms(),
        state_dir: state_dir.display().to_string(),
        runtime_version: RUNTIME_VERSION.to_string(),
    };
    write_json_file(&session_info_path(&state_dir), &info)?;
    let stop_path = session_stop_path(&state_dir);
    while !stop_path.exists() {
        let processed = process_session_requests(&state_dir)?;
        let heartbeat = SessionHeartbeat {
            pid: std::process::id(),
            updated_at_unix_ms: current_unix_ms(),
            state: if processed > 0 {
                "processed_request".to_string()
            } else {
                "idle".to_string()
            },
        };
        write_json_file(&session_heartbeat_path(&state_dir), &heartbeat)?;
        thread::sleep(Duration::from_millis(100));
    }
    let _ = fs::remove_file(session_info_path(&state_dir));
    let _ = fs::remove_file(stop_path);
    Ok(json!({
        "status": "stopped",
        "state_dir": state_dir.display().to_string()
    }))
}

fn run_session_instance(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let action = args
        .first()
        .map(String::as_str)
        .ok_or_else(|| CliError::usage("session instance requires list|health|reconnect"))?;
    let flags = FlagArgs::parse(&args[1..])?;
    let config = read_user_config()?;
    match action {
        "list" => Ok(json!({
            "instances": config.instances.iter().map(|(id, instance)| json!({
                "id": id,
                "serial": instance.serial,
                "game": instance.game,
                "server": instance.server,
                "package": instance.package
            })).collect::<Vec<_>>()
        })),
        "health" | "reconnect" => {
            let instance_id = resolve_instance_id_for_flags(global, &config, &flags)?;
            let device_config = device_config_for_instance(global, &config, Some(&instance_id))?;
            let serial = device_config.target.resolved_serial();
            let adb = Adb::new(device_config.adb);
            let state = adb
                .ensure_device(&serial, device_config.target.connect)
                .map_err(|err| CliError::device(err.to_string()))?;
            let screen_size = adb
                .screen_size(&serial)
                .map_err(|err| CliError::device(err.to_string()))?;
            Ok(json!({
                "instance": instance_id,
                "serial": serial,
                "state": state,
                "screen_size": screen_size,
                "action": action
            }))
        }
        other => Err(CliError::usage(format!(
            "unknown session instance action: {other}"
        ))),
    }
}

fn run_session_app(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let action = args
        .first()
        .map(String::as_str)
        .ok_or_else(|| CliError::usage("session app requires launch|stop|restart"))?;
    let flags = FlagArgs::parse(&args[1..])?;
    if flags.bool("--via-daemon") {
        return submit_control_session_request(global, &flags, "app", args);
    }
    let config = read_user_config()?;
    let instance_id = resolve_instance_id_for_flags(global, &config, &flags)?;
    let package = resolve_app_package(global, &config, &flags, &instance_id)?;
    let device_config = device_config_for_instance(global, &config, Some(&instance_id))?;
    let serial = device_config.target.resolved_serial();
    let adb = Adb::new(device_config.adb);
    adb.ensure_device(&serial, device_config.target.connect)
        .map_err(|err| CliError::device(err.to_string()))?;
    match action {
        "launch" => {
            let output = adb
                .launch_package(&serial, &package)
                .map_err(|err| CliError::device(err.to_string()))?;
            Ok(json!({
                "action": "launch",
                "instance": instance_id,
                "serial": serial,
                "package": package,
                "stdout": output.stdout,
                "stderr": output.stderr
            }))
        }
        "stop" => {
            let output = adb
                .force_stop(&serial, &package)
                .map_err(|err| CliError::device(err.to_string()))?;
            Ok(json!({
                "action": "stop",
                "instance": instance_id,
                "serial": serial,
                "package": package,
                "stdout": output.stdout,
                "stderr": output.stderr
            }))
        }
        "restart" => {
            let stop = adb
                .force_stop(&serial, &package)
                .map_err(|err| CliError::device(err.to_string()))?;
            thread::sleep(Duration::from_millis(500));
            let launch = adb
                .launch_package(&serial, &package)
                .map_err(|err| CliError::device(err.to_string()))?;
            Ok(json!({
                "action": "restart",
                "instance": instance_id,
                "serial": serial,
                "package": package,
                "stop_stdout": stop.stdout,
                "stop_stderr": stop.stderr,
                "launch_stdout": launch.stdout,
                "launch_stderr": launch.stderr
            }))
        }
        other => Err(CliError::usage(format!(
            "unknown session app action: {other}"
        ))),
    }
}

fn run_session_lease(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let action = args
        .first()
        .map(String::as_str)
        .ok_or_else(|| CliError::usage("session lease requires acquire|release|preempt|status"))?;
    let flags = FlagArgs::parse(&args[1..])?;
    let config = read_user_config()?;
    let state_dir = session_state_dir_from_flags(&flags)?;
    fs::create_dir_all(&state_dir).map_err(|err| {
        CliError::runtime_not_running(format!(
            "failed to create session state dir {}: {err}",
            state_dir.display()
        ))
    })?;
    let instance_id = resolve_instance_id_for_flags(global, &config, &flags)?;
    let holder = flags
        .optional("--holder")
        .filter(|value| value != "true")
        .unwrap_or_else(|| "manual".to_string());
    let lease_path = session_lease_path(&state_dir, &instance_id);
    match action {
        "status" => Ok(json!({
            "instance": instance_id,
            "lease": read_json_value(&lease_path)?,
            "path": lease_path.display().to_string()
        })),
        "acquire" => {
            if lease_path.exists() {
                let current = read_json_file::<SessionLease>(&lease_path)?;
                return Err(CliError::safety_blocked(
                    "lease_conflict",
                    format!(
                        "session lease already exists for {instance_id}{}",
                        current
                            .as_ref()
                            .map(|lease| format!(" held by {}", lease.holder))
                            .unwrap_or_default()
                    ),
                    &["lab_lease", "lease_holder"],
                ));
            }
            let lease = new_session_lease(
                instance_id,
                holder,
                flags.optional("--lease-id"),
                false,
                None,
            );
            write_json_file_atomic(&lease_path, &lease)?;
            Ok(json!({
                "status": "acquired",
                "lease": lease,
                "path": lease_path.display().to_string()
            }))
        }
        "preempt" => {
            let previous = read_json_file::<SessionLease>(&lease_path)?;
            let lease = new_session_lease(
                instance_id,
                holder,
                flags.optional("--lease-id"),
                true,
                previous.as_ref().map(SessionLeasePrevious::from),
            );
            write_json_file_atomic(&lease_path, &lease)?;
            Ok(json!({
                "status": "preempted",
                "lease": lease,
                "previous": previous,
                "path": lease_path.display().to_string()
            }))
        }
        "release" => {
            let Some(lease) = read_json_file::<SessionLease>(&lease_path)? else {
                return Ok(json!({
                    "status": "not_held",
                    "instance": instance_id,
                    "path": lease_path.display().to_string()
                }));
            };
            let force = flags.bool("--force");
            validate_lease_release(&lease, &holder, flags.optional("--lease-id"), force)?;
            fs::remove_file(&lease_path).map_err(|err| {
                CliError::runtime_not_running(format!(
                    "failed to remove lease {}: {err}",
                    lease_path.display()
                ))
            })?;
            Ok(json!({
                "status": "released",
                "instance": instance_id,
                "holder": holder,
                "force": force,
                "released_lease": lease,
                "path": lease_path.display().to_string()
            }))
        }
        other => Err(CliError::usage(format!(
            "unknown session lease action: {other}"
        ))),
    }
}

fn session_lease_path(state_dir: &Path, instance_id: &str) -> PathBuf {
    state_dir.join(format!("lease-{}.json", safe_file_stem(instance_id)))
}

fn session_command_instance_id(global: &SessionCommandGlobal) -> String {
    global
        .instance
        .clone()
        .unwrap_or_else(|| "default".to_string())
}

fn new_session_lease(
    instance: String,
    holder: String,
    lease_id: Option<String>,
    preempted: bool,
    previous: Option<SessionLeasePrevious>,
) -> SessionLease {
    let now = current_unix_ms();
    let lease_id = lease_id
        .filter(|value| value != "true")
        .unwrap_or_else(|| format!("{now}-{}-{}", std::process::id(), safe_file_stem(&holder)));
    SessionLease {
        instance,
        holder,
        lease_id,
        acquired_at_unix_ms: now,
        updated_at_unix_ms: now,
        preempted,
        previous,
    }
}

impl From<&SessionLease> for SessionLeasePrevious {
    fn from(lease: &SessionLease) -> Self {
        Self {
            holder: lease.holder.clone(),
            lease_id: lease.lease_id.clone(),
            acquired_at_unix_ms: lease.acquired_at_unix_ms,
            updated_at_unix_ms: lease.updated_at_unix_ms,
        }
    }
}

fn validate_lease_release(
    lease: &SessionLease,
    holder: &str,
    lease_id: Option<String>,
    force: bool,
) -> CliOutcome<()> {
    if force {
        return Ok(());
    }
    validate_lease_request(
        lease,
        &SessionCommandLease {
            holder: holder.to_string(),
            lease_id,
        },
    )
}

fn validate_lease_request(lease: &SessionLease, requested: &SessionCommandLease) -> CliOutcome<()> {
    if lease.holder != requested.holder {
        return Err(CliError::safety_blocked(
            "lease_holder_mismatch",
            format!(
                "lease for {} is held by {}, not {}",
                lease.instance, lease.holder, requested.holder
            ),
            &["lab_lease", "lease_holder"],
        ));
    }
    if let Some(expected) = requested.lease_id.as_ref().filter(|value| *value != "true")
        && !lease.lease_id.is_empty()
        && lease.lease_id.as_str() != expected.as_str()
    {
        return Err(CliError::safety_blocked(
            "lease_id_mismatch",
            format!(
                "lease for {} has id {}, not {}",
                lease.instance, lease.lease_id, expected
            ),
            &["lab_lease", "lease_id"],
        ));
    }
    Ok(())
}

fn run_session_record(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let action = args.first().map(String::as_str).ok_or_else(|| {
        CliError::usage(
            "session record requires start|status|stop|step|candidates|amend|build-task|promote",
        )
    })?;
    let flags = FlagArgs::parse(&args[1..])?;
    let config = read_user_config()?;
    let state_dir = session_state_dir_from_flags(&flags)?;
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
            let step_id = record_amend_step_id(&flags)?;
            let amend_context = SessionRecordAmendContext {
                record_id: record.record_id.clone(),
                state_dir: state_dir.clone(),
            };
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
    let (game, server) = session_record_game_server(global, config, flags, instance_id)?;
    let draft = session_record_build_draft(&record, flags, &out, &game, &server)?;
    if !dry_run {
        write_session_record_build_draft(&draft)?;
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
    let (game, server) = session_record_game_server(global, config, flags, instance_id)?;
    let draft = session_record_build_draft(&record, flags, &resource_root.root, &game, &server)?;
    validate_session_record_promote_target(&draft, force)?;
    let resources_action = if dry_run {
        if draft.resources_path.exists() {
            "would_preserve"
        } else {
            "would_create"
        }
    } else {
        write_session_record_promoted_task(&draft, force)?
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
        "repo": resource_root.input.display().to_string(),
        "resource_root": resource_root.root.display().to_string(),
        "resource_layout": resource_root.layout,
        "task_dir": draft.task_dir.display().to_string(),
        "task_path": draft.task_path.display().to_string(),
        "resources_path": draft.resources_path.display().to_string(),
        "resources_action": resources_action,
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
) -> CliOutcome<SessionRecordBuildDraft> {
    let task_dir_name = safe_task_dir_name(&record.task_id)?;
    let task_dir = out.join("operations").join(task_dir_name);
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
            let source = PathBuf::from(&artifact.path);
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
            let source = PathBuf::from(&artifact.path);
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
            let click = session_record_bundle_click(click, &step.step_id)?;
            validate_record_build_page_ref("from", from, &anchor_templates, &step.step_id)?;
            if let Some(to) = to {
                validate_record_build_page_ref("to", to, &anchor_templates, &step.step_id)?;
            }
            let verify_template = to.as_ref().and_then(|to| anchor_templates.get(to)).cloned();
            operations.push(json!({
                "id": step.step_id,
                "purpose": format!("recorded operation from {from}"),
                "from": from,
                "to": to,
                "click": click,
                "verify_template": verify_template,
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
    let bundle = json!({
        "schema_version": "0.3",
        "task_id": record.task_id,
        "game": game,
        "server_scope": [server],
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
        bundle,
        task_dir,
        task_path,
        resources_path,
        assets,
    })
}

fn write_session_record_build_draft(draft: &SessionRecordBuildDraft) -> CliOutcome<()> {
    copy_session_record_build_assets(draft)?;
    write_json_file(
        &draft.resources_path,
        &json!({
            "schema_version": "1.0",
            "resources": [],
            "resource_count": 0
        }),
    )?;
    write_json_file(&draft.task_path, &draft.bundle)
}

fn validate_session_record_promote_target(
    draft: &SessionRecordBuildDraft,
    force: bool,
) -> CliOutcome<()> {
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
    Ok(())
}

fn write_session_record_promoted_task(
    draft: &SessionRecordBuildDraft,
    force: bool,
) -> CliOutcome<&'static str> {
    if draft.task_dir.exists() {
        if !force {
            return Err(CliError::safety_blocked(
                "record_promote_target_exists",
                format!(
                    "record promote target task directory already exists: {}; use --force to replace it",
                    draft.task_dir.display()
                ),
                &["session_record", "resource_repo"],
            ));
        }
        remove_record_promote_task_dir(&draft.task_dir)?;
    }
    copy_session_record_build_assets(draft)?;
    let resources_action = if draft.resources_path.exists() {
        "preserved"
    } else {
        write_json_file(
            &draft.resources_path,
            &json!({
                "schema_version": "1.0",
                "resources": [],
                "resource_count": 0
            }),
        )?;
        "created"
    };
    write_json_file(&draft.task_path, &draft.bundle)?;
    Ok(resources_action)
}

fn remove_record_promote_task_dir(task_dir: &Path) -> CliOutcome<()> {
    if task_dir.is_dir() {
        fs::remove_dir_all(task_dir).map_err(|err| {
            CliError::usage(format!(
                "failed to remove existing promoted task directory {}: {err}",
                task_dir.display()
            ))
        })
    } else {
        Err(CliError::usage(format!(
            "record promote target exists but is not a directory: {}",
            task_dir.display()
        )))
    }
}

fn copy_session_record_build_assets(draft: &SessionRecordBuildDraft) -> CliOutcome<()> {
    for asset in &draft.assets {
        if let Some(parent) = asset.destination.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                CliError::usage(format!("failed to create {}: {err}", parent.display()))
            })?;
        }
        fs::copy(&asset.source, &asset.destination).map_err(|err| {
            CliError::usage(format!(
                "failed to copy record asset {} to {}: {err}",
                asset.source.display(),
                asset.destination.display()
            ))
        })?;
    }
    Ok(())
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
        SessionRecordClick::Target { target } => Err(CliError::usage(format!(
            "record build-task cannot build operation '{step_id}' with unresolved target click '{target}'"
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
        if click.get("kind").and_then(Value::as_str) != Some("point") {
            continue;
        }
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
        if x < 0 || y < 0 || x >= i64::from(width) || y >= i64::from(height) {
            return Err(CliError::usage(format!(
                "record build-task operation '{operation_id}' click point {x},{y} is outside coordinate_space {width}x{height}"
            )));
        }
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

fn session_record_game_server(
    global: &GlobalOptions,
    config: &UserConfig,
    flags: &FlagArgs,
    instance_id: &str,
) -> CliOutcome<(String, String)> {
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
        .unwrap_or_else(|| default_server_for_game(&game).to_string());
    Ok((game, server))
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
    let to = required_non_empty_flag(flags, "--to")?;
    Ok(SessionRecordStepData::Operation {
        from,
        to: if to == "null" { None } else { Some(to) },
        click: parse_session_record_click(&flags.required("--click")?)?,
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
    let artifact_dir = flags.optional_path("--artifact-dir").unwrap_or_else(|| {
        context
            .state_dir
            .join("record-artifacts")
            .join(safe_file_stem(&context.record.record_id))
    });
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
    let artifact_dir = flags.optional_path("--artifact-dir").unwrap_or_else(|| {
        context
            .state_dir
            .join("record-artifacts")
            .join(safe_file_stem(&context.record.record_id))
    });
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

fn capture_session_record_source_frame(
    global: &GlobalOptions,
    config: &UserConfig,
    flags: &FlagArgs,
    artifact_dir: &Path,
    step_id: &str,
    anchor_id: &str,
) -> CliOutcome<SessionRecordSourceFrame> {
    let device_config = device_config(global, config)?;
    let requested = global.capture_backend.unwrap_or_default();
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
    let artifact_dir = amended_anchor_artifact_dir(context, target.artifact.as_deref());
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
    let artifact_dir = amended_anchor_artifact_dir(context, target.artifact.as_deref());
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
) -> PathBuf {
    artifact
        .and_then(|artifact| Path::new(&artifact.path).parent().map(Path::to_path_buf))
        .unwrap_or_else(|| {
            context
                .state_dir
                .join("record-artifacts")
                .join(safe_file_stem(&context.record_id))
        })
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

#[derive(Debug, Default)]
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
    if !runtime_tcp_available(&endpoint) {
        return Err(CliError::runtime_not_running(format!(
            "Runtime is not reachable at {endpoint}"
        )));
    }
    Ok(json!({
        "endpoint": endpoint,
        "connection": "tcp"
    }))
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
    let trimmed = endpoint
        .strip_prefix("http://")
        .or_else(|| endpoint.strip_prefix("https://"))
        .unwrap_or(endpoint);
    let host_port = trimmed.split('/').next()?;
    let (host, port) = host_port.rsplit_once(':')?;
    Some((host.to_string(), port.parse().ok()?))
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
    let mut target = DeviceTarget::default();
    if let Some(serial) = instance.and_then(|instance| instance.serial.clone()) {
        target.serial = Some(serial);
    } else if global.instance.as_deref() == Some(instance_id.as_str()) && instance.is_none() {
        target.serial = Some(instance_id.clone());
    }
    let adb = AdbConfig {
        adb_path: effective_adb_path(config)?.path,
        ..Default::default()
    };
    Ok(DeviceRuntimeConfig { adb, target })
}

#[derive(Debug)]
struct DeviceRuntimeConfig {
    adb: AdbConfig,
    target: DeviceTarget,
}

impl DeviceRuntimeConfig {
    fn capture_backend_config(&self, requested: CaptureBackendChoice) -> CaptureBackendConfig {
        CaptureBackendConfig::new(self.adb.clone(), self.target.clone()).with_requested(requested)
    }
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

fn resolve_app_package(
    global: &GlobalOptions,
    config: &UserConfig,
    flags: &FlagArgs,
    instance_id: &str,
) -> CliOutcome<String> {
    if let Some(package) = flags.optional("--package").filter(|value| value != "true") {
        return Ok(package);
    }
    let instance = config.instances.get(instance_id);
    if let Some(package) = instance.and_then(|instance| instance.package.clone()) {
        return Ok(package);
    }
    let game = global
        .game
        .clone()
        .or_else(|| instance.and_then(|instance| instance.game.clone()));
    let server = global
        .server
        .clone()
        .or_else(|| instance.and_then(|instance| instance.server.clone()));
    default_package_name(game.as_deref(), server.as_deref())
        .map(str::to_string)
        .ok_or_else(|| {
            CliError::usage(
                "session app requires --package, instance.<id>.package, or a known game/server",
            )
        })
}

fn default_package_name(game: Option<&str>, server: Option<&str>) -> Option<&'static str> {
    let game = match game?.to_ascii_lowercase().as_str() {
        "ak" | "ark" | "arknights" => "arknights",
        "azur" | "azurlane" | "azur_lane" | "al" => "azurlane",
        "ba" | "bluearchive" | "blue_archive" => "bluearchive",
        _ => return None,
    };
    let server = server.unwrap_or_else(|| default_server_for_game(game));
    match (game, server) {
        ("arknights", "cn") => Some("com.hypergryph.arknights.bilibili"),
        ("azurlane", "jp") => Some("com.YoStarJP.AzurLane"),
        ("bluearchive", "jp") => Some("com.YostarJP.BlueArchive"),
        _ => None,
    }
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

fn app_state_root() -> CliOutcome<PathBuf> {
    let root = env::var("LOCALAPPDATA")
        .or_else(|_| env::var("APPDATA"))
        .map_err(|_| CliError::usage("LOCALAPPDATA or APPDATA is required for ActingLab state"))?;
    Ok(PathBuf::from(root).join("ActingCommand").join("actinglab"))
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

fn session_info_path(state_dir: &Path) -> PathBuf {
    state_dir.join(SESSION_INFO_FILE)
}

fn session_heartbeat_path(state_dir: &Path) -> PathBuf {
    state_dir.join(SESSION_HEARTBEAT_FILE)
}

fn session_stop_path(state_dir: &Path) -> PathBuf {
    state_dir.join(SESSION_STOP_FILE)
}

fn session_requests_dir(state_dir: &Path) -> PathBuf {
    state_dir.join(SESSION_REQUESTS_DIR)
}

fn session_responses_dir(state_dir: &Path) -> PathBuf {
    state_dir.join(SESSION_RESPONSES_DIR)
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

fn read_json_value(path: &Path) -> CliOutcome<Option<Value>> {
    read_json_file(path)
}

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
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&tmp, text)
        .map_err(|err| CliError::usage(format!("failed to write {}: {err}", tmp.display())))?;
    fs::rename(&tmp, path).map_err(|err| {
        CliError::usage(format!(
            "failed to publish {} from {}: {err}",
            path.display(),
            tmp.display()
        ))
    })
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
            "instance config keys use instance.<id>.serial|game|server|package",
        ));
    }
    let instance = config.instances.get(parts[1]);
    let value = match parts[2] {
        "serial" => instance.and_then(|instance| instance.serial.clone()),
        "game" => instance.and_then(|instance| instance.game.clone()),
        "server" => instance.and_then(|instance| instance.server.clone()),
        "package" => instance.and_then(|instance| instance.package.clone()),
        other => return Err(CliError::usage(format!("unknown instance field: {other}"))),
    };
    Ok(json!(value))
}

fn set_instance_value(config: &mut UserConfig, key: &str, value: &str) -> CliOutcome<()> {
    let parts = key.split('.').collect::<Vec<_>>();
    if parts.len() != 3 {
        return Err(CliError::usage(
            "instance config keys use instance.<id>.serial|game|server|package",
        ));
    }
    let instance = config.instances.entry(parts[1].to_string()).or_default();
    match parts[2] {
        "serial" => instance.serial = Some(value.to_string()),
        "game" => instance.game = Some(value.to_string()),
        "server" => instance.server = Some(value.to_string()),
        "package" => instance.package = Some(value.to_string()),
        other => return Err(CliError::usage(format!("unknown instance field: {other}"))),
    }
    Ok(())
}

fn effective_adb_path(config: &UserConfig) -> CliOutcome<actingcommand_device::ResolvedAdbPath> {
    resolve_adb_path(config.adb_path.as_deref()).map_err(|err| CliError::device(err.to_string()))
}

fn resolved_adb_json(config: &UserConfig) -> Value {
    match resolve_adb_path(config.adb_path.as_deref()) {
        Ok(resolved) => json!({
            "ok": true,
            "path": resolved.path,
            "source": resolved.source.as_str()
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
        .unwrap_or_else(|| default_server_for_game(&game).to_string());
    Ok((game, server))
}

fn canonical_game(value: &str) -> CliOutcome<String> {
    match value.to_ascii_lowercase().as_str() {
        "ak" | "ark" | "arknights" => Ok("arknights".to_string()),
        "azur" | "azurlane" | "azur_lane" | "al" => Ok("azurlane".to_string()),
        "ba" | "bluearchive" | "blue_archive" => Ok("bluearchive".to_string()),
        other => Err(CliError::usage(format!("unknown game selector: {other}"))),
    }
}

fn default_server_for_game(game: &str) -> &'static str {
    match game {
        "arknights" => "cn",
        "azurlane" | "bluearchive" => "jp",
        _ => "jp",
    }
}

#[derive(Debug)]
struct PackageValidation {
    module: String,
    manifest_path: String,
    task_count: usize,
    entry_count: usize,
    dangerous_entries: Vec<String>,
    entries: Vec<String>,
    manifest: Value,
}

fn validate_package_zip(path: &Path) -> CliOutcome<PackageValidation> {
    let file = File::open(path).map_err(|err| {
        CliError::package_invalid(format!("failed to open package {}: {err}", path.display()))
    })?;
    let mut archive = ZipArchive::new(file).map_err(|err| {
        CliError::package_invalid(format!("failed to read zip {}: {err}", path.display()))
    })?;
    let mut paths = BTreeSet::new();
    let mut entries = BTreeMap::<String, Vec<u8>>::new();
    let mut dangerous = Vec::new();
    let mut module_roots = BTreeSet::new();
    let mut total_uncompressed = 0u64;

    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|err| CliError::package_invalid(format!("failed to read zip entry: {err}")))?;
        let Some(path_name) = normalize_zip_path(file.name())? else {
            continue;
        };
        if !paths.insert(path_name.clone()) {
            return Err(CliError::package_invalid(format!(
                "duplicate zip entry: {path_name}"
            )));
        }
        if let Some(root) = path_name.split('/').next() {
            module_roots.insert(root.to_string());
        }
        if has_dangerous_extension(&path_name) {
            dangerous.push(path_name.clone());
        }
        if file.size() > MAX_PACKAGE_ZIP_ENTRY_BYTES {
            return Err(CliError::package_invalid(format!(
                "zip entry {path_name} exceeds {} bytes",
                MAX_PACKAGE_ZIP_ENTRY_BYTES
            )));
        }
        let bytes = read_zip_entry_limited(&mut file, &path_name, MAX_PACKAGE_ZIP_ENTRY_BYTES)?;
        total_uncompressed = total_uncompressed
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| CliError::package_invalid("package uncompressed size overflowed"))?;
        if total_uncompressed > MAX_PACKAGE_ZIP_TOTAL_BYTES {
            return Err(CliError::package_invalid(format!(
                "package exceeds total uncompressed limit of {} bytes",
                MAX_PACKAGE_ZIP_TOTAL_BYTES
            )));
        }
        entries.insert(path_name, bytes);
    }

    if !dangerous.is_empty() {
        return Err(CliError::package_invalid(format!(
            "package contains executable/script entries: {}",
            dangerous.join(", ")
        )));
    }
    if module_roots.len() != 1 {
        return Err(CliError::package_invalid(
            "package must contain exactly one top-level module directory",
        ));
    }
    let module = module_roots.into_iter().next().expect("one module root");
    let manifest_path = format!("{module}/manifest.json");
    let manifest_bytes = entries
        .get(&manifest_path)
        .ok_or_else(|| CliError::package_invalid(format!("missing {manifest_path}")))?;
    let manifest: Value = serde_json::from_slice(manifest_bytes).map_err(|err| {
        CliError::package_invalid(format!("failed to parse {manifest_path}: {err}"))
    })?;
    let task_count = entries
        .keys()
        .filter(|path| {
            path.starts_with(&format!("{module}/operations/")) && path.ends_with("/task.json")
        })
        .count();
    if task_count == 0 {
        return Err(CliError::package_invalid(format!(
            "missing {module}/operations/<task_id>/task.json"
        )));
    }
    validate_manifest_hashes(&manifest, &entries, &module)?;
    Ok(PackageValidation {
        module,
        manifest_path,
        task_count,
        entry_count: entries.len(),
        dangerous_entries: dangerous,
        entries: entries.keys().cloned().collect(),
        manifest,
    })
}

fn read_zip_entry_limited<R: Read>(
    reader: &mut R,
    path_name: &str,
    limit: u64,
) -> CliOutcome<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut limited = reader.take(limit.saturating_add(1));
    limited.read_to_end(&mut bytes).map_err(|err| {
        CliError::package_invalid(format!("failed to read zip entry {path_name}: {err}"))
    })?;
    if bytes.len() as u64 > limit {
        return Err(CliError::package_invalid(format!(
            "zip entry {path_name} exceeds {limit} bytes"
        )));
    }
    Ok(bytes)
}

fn normalize_zip_path(name: &str) -> CliOutcome<Option<String>> {
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
            DANGEROUS_EXTENSIONS
                .iter()
                .any(|dangerous| extension.eq_ignore_ascii_case(dangerous))
        })
}

fn validate_manifest_hashes(
    manifest: &Value,
    entries: &BTreeMap<String, Vec<u8>>,
    module: &str,
) -> CliOutcome<()> {
    for (path, expected) in manifest_hashes(manifest)? {
        let resolved = if entries.contains_key(&path) {
            path
        } else {
            format!("{module}/{path}")
        };
        let bytes = entries.get(&resolved).ok_or_else(|| {
            CliError::package_invalid(format!("manifest hash references missing path: {resolved}"))
        })?;
        let actual = hex_sha256(bytes);
        let expected = expected
            .strip_prefix("sha256:")
            .unwrap_or(&expected)
            .to_ascii_lowercase();
        if actual != expected {
            return Err(CliError::package_invalid(format!(
                "hash mismatch for {resolved}: expected {expected}, actual {actual}"
            )));
        }
    }
    Ok(())
}

fn manifest_hashes(manifest: &Value) -> CliOutcome<Vec<(String, String)>> {
    let mut hashes = Vec::new();
    if let Some(object) = manifest.get("hashes").and_then(Value::as_object) {
        for (path, value) in object {
            if let Some(hash) = value.as_str() {
                hashes.push((normalize_manifest_hash_path(path)?, hash.to_string()));
            }
        }
    }
    if let Some(files) = manifest.get("files").and_then(Value::as_array) {
        for file in files {
            let Some(path) = file.get("path").and_then(Value::as_str) else {
                continue;
            };
            let hash = file
                .get("sha256")
                .or_else(|| file.get("hash"))
                .and_then(Value::as_str);
            if let Some(hash) = hash {
                hashes.push((normalize_manifest_hash_path(path)?, hash.to_string()));
            }
        }
    }
    Ok(hashes)
}

fn normalize_manifest_hash_path(path: &str) -> CliOutcome<String> {
    if path.ends_with('/')
        || path.contains('\\')
        || path.contains(':')
        || path.starts_with('/')
        || Path::new(path).components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(CliError::package_invalid("manifest hash path is unsafe"));
    }
    Ok(path.to_string())
}

fn package_validation_json(validation: &PackageValidation, include_entries: bool) -> Value {
    json!({
        "status": "valid",
        "module": validation.module,
        "manifest_path": validation.manifest_path,
        "task_count": validation.task_count,
        "entry_count": validation.entry_count,
        "dangerous_entries": validation.dangerous_entries,
        "manifest": validation.manifest,
        "entries": if include_entries { json!(validation.entries) } else { Value::Null }
    })
}

fn create_package_blocked_result_zip(
    out: &Path,
    validation: &PackageValidation,
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
        serde_json::to_string_pretty(&package_validation_json(validation, false))
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
            "match_metric": value
                .get("defaults")
                .and_then(|defaults| defaults.get("match_metric"))
                .and_then(Value::as_str)
                .unwrap_or("ccorr_normed")
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
        let requested = global.capture_backend.unwrap_or_default();
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

fn load_monitor_scene_from_flags(
    global: &GlobalOptions,
    flags: &FlagArgs,
) -> CliOutcome<MonitorSceneInput> {
    if let Some(scene_path) = flags.optional_path("--scene") {
        let png = fs::read(&scene_path).map_err(|err| {
            CliError::device(format!("failed to read {}: {err}", scene_path.display()))
        })?;
        let scene = Scene::from_png(&png).map_err(|err| CliError::device(err.to_string()))?;
        return Ok(MonitorSceneInput {
            scene,
            source: json!({
                "kind": "scene",
                "path": scene_path.display().to_string()
            }),
        });
    }
    if flags.bool("--capture") {
        let config = read_user_config()?;
        let device_config = device_config(global, &config)?;
        let requested = global.capture_backend.unwrap_or_default();
        let fresh_delay = parse_optional_duration_ms(flags, "--fresh-delay-ms", 160)?;
        let captured = capture_for_command(
            &device_config,
            requested,
            flags.bool("--require-fresh"),
            fresh_delay,
        )?;
        let frame = captured.frame;
        let source = json!({
            "kind": "capture",
            "width": frame.width,
            "height": frame.height,
            "capture_backend_used": frame.backend_name.as_str(),
            "capture_backend_attempts": captured.attempts,
            "freshness": captured.freshness
        });
        let scene = scene_from_frame(&frame)?;
        return Ok(MonitorSceneInput { scene, source });
    }
    Err(CliError::usage(
        "monitor --once requires --scene <png> or --capture",
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

fn load_evaluator(pack_path: &Path, pack_root: &Path) -> CliOutcome<RecognitionEvaluator> {
    let pack_json = fs::read_to_string(pack_path)
        .map_err(|err| CliError::usage(format!("failed to read {}: {err}", pack_path.display())))?;
    let pack =
        load_pack_from_json_str(&pack_json).map_err(|err| CliError::usage(err.to_string()))?;
    RecognitionEvaluator::new(pack_root.to_path_buf(), pack)
        .map_err(|err| CliError::usage(err.to_string()))
}

fn load_evaluator_and_detector(
    pack_path: &Path,
    pack_root: &Path,
    pages_path: &Path,
) -> CliOutcome<(RecognitionEvaluator, PageDetector)> {
    let evaluator = load_evaluator(pack_path, pack_root)?;
    let pages_json = fs::read_to_string(pages_path).map_err(|err| {
        CliError::usage(format!("failed to read {}: {err}", pages_path.display()))
    })?;
    let pages =
        load_page_set_from_json_str(&pages_json).map_err(|err| CliError::usage(err.to_string()))?;
    let detector = PageDetector::new(pages).map_err(|err| CliError::usage(err.to_string()))?;
    Ok((evaluator, detector))
}

fn is_click_only_target(evaluator: &RecognitionEvaluator, target: &str) -> CliOutcome<bool> {
    let kind = evaluator
        .target_kind(target)
        .map_err(|err| CliError::usage(err.to_string()))?;
    Ok(matches!(
        kind,
        actingcommand_recognition_pack::TargetKind::ClickOnly
    ))
}

fn page_eval_json(evaluation: &actingcommand_page_detector::PageEvaluation) -> Value {
    json!({
        "page": evaluation.page_id,
        "matched": evaluation.matched,
        "message": evaluation.message,
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
    vec![
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
        command_cap("resource import-alas", ["offline"], "reserved"),
        command_cap("resource drift-alas", ["offline"], "reserved"),
        command_cap("resource check-release", ["offline"], "available"),
        command_cap("package validate", ["offline"], "available"),
        command_cap("package inspect", ["offline"], "available"),
        command_cap("package build-task", ["offline"], "available"),
        command_cap("package build-pack", ["offline"], "available"),
        command_cap("operation validate", ["offline"], "available"),
        command_cap("operation inspect", ["offline"], "available"),
        command_cap("operation explain", ["offline"], "available"),
        command_cap("status", ["running_runtime"], "available"),
        command_cap("devices", ["device"], "available"),
        command_cap("tap", ["device"], "available"),
        command_cap("swipe", ["device"], "available"),
        command_cap("long-tap", ["device"], "available"),
        command_cap("key", ["device"], "available"),
        command_cap("text", ["device"], "available"),
        command_cap("session status", ["offline"], "available"),
        command_cap("session start", ["offline"], "available"),
        command_cap("session stop", ["offline"], "available"),
        command_cap(
            "session request capture-diagnose",
            ["running_runtime", "device"],
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
            "session request app",
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
        command_cap("session instance", ["offline", "device"], "available"),
        command_cap("session app", ["device"], "available"),
        command_cap("session app launch", ["device"], "available"),
        command_cap("session app stop", ["device"], "available"),
        command_cap("session app restart", ["device"], "available"),
        command_cap("session capture", ["device"], "available"),
        command_cap("session capture diagnose", ["device"], "available"),
        command_cap("session recover", ["device"], "available"),
        command_cap("session lease", ["offline"], "available"),
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
        command_cap(
            "stream",
            ["running_runtime", "device", "trusted_channel"],
            "reserved",
        ),
        command_cap("scheduler status", ["running_runtime"], "reserved"),
        command_cap("scheduler pause", ["running_runtime"], "reserved"),
        command_cap("scheduler resume", ["running_runtime"], "reserved"),
        command_cap("scheduler start", ["running_runtime"], "reserved"),
        command_cap("scheduler stop", ["running_runtime"], "reserved"),
        command_cap("lab status", ["running_runtime"], "reserved"),
        command_cap("lab lease", ["running_runtime"], "reserved"),
        command_cap("lab release", ["running_runtime"], "reserved"),
        command_cap("lab validate", ["offline"], "available"),
        command_cap("lab run", ["device"], "available"),
        command_cap("capture", ["device"], "available"),
        command_cap("capture diagnose", ["device"], "available"),
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
    ]
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
    use std::sync::Mutex;
    use tempfile::TempDir;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

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
        assert_eq!(result.exit_code(), 0);
        assert!(result.envelope.ok);
        assert_eq!(result.envelope.command, "version");
    }

    #[test]
    fn status_without_runtime_is_exit_five() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        unsafe {
            env::set_var(CONFIG_ENV, temp.path().join("config.json"));
        }
        let result = run_cli(["--json", "status"], true);
        unsafe {
            env::remove_var(CONFIG_ENV);
        }
        assert_eq!(result.exit_code(), 5);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "runtime_not_running"
        );
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
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let set = run_cli(
            [
                "--json",
                "config",
                "set",
                "instance.ba.serial",
                "127.0.0.1:16448",
            ],
            true,
        );
        assert_eq!(set.exit_code(), 0);
        let get = run_cli(["--json", "config", "get", "instance.ba.serial"], true);
        unsafe {
            env::remove_var(CONFIG_ENV);
        }
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
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let set = run_cli(
            [
                "--json",
                "config",
                "set",
                "instance.ak.package",
                "com.hypergryph.arknights.bilibili",
            ],
            true,
        );
        assert_eq!(set.exit_code(), 0);
        let get = run_cli(["--json", "config", "get", "instance.ak.package"], true);
        unsafe {
            env::remove_var(CONFIG_ENV);
        }
        assert_eq!(get.exit_code(), 0);
        assert_eq!(
            get.envelope
                .data
                .as_ref()
                .unwrap()
                .get("value")
                .and_then(Value::as_str),
            Some("com.hypergryph.arknights.bilibili")
        );
    }

    #[test]
    fn session_status_without_daemon_is_offline_ok() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        unsafe {
            env::set_var(SESSION_STATE_ENV, temp.path());
        }
        let result = run_cli(["--json", "session", "status"], true);
        unsafe {
            env::remove_var(SESSION_STATE_ENV);
        }
        assert_eq!(result.exit_code(), 0);
        assert_eq!(
            result
                .envelope
                .data
                .as_ref()
                .unwrap()
                .get("running")
                .and_then(Value::as_bool),
            Some(false)
        );
    }

    #[test]
    fn session_lease_enforces_holder_and_lease_id_on_release() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let acquire = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "lease",
                "acquire",
                "--state-dir",
                temp.path().to_str().unwrap(),
                "--holder",
                "scheduler",
                "--lease-id",
                "lease-1",
            ],
            true,
        );
        assert_eq!(acquire.exit_code(), 0);
        assert_eq!(
            acquire
                .envelope
                .data
                .as_ref()
                .unwrap()
                .pointer("/lease/holder")
                .and_then(Value::as_str),
            Some("scheduler")
        );

        let conflict = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "lease",
                "acquire",
                "--state-dir",
                temp.path().to_str().unwrap(),
                "--holder",
                "lab",
            ],
            true,
        );
        assert_eq!(conflict.exit_code(), 3);
        assert_eq!(
            conflict.envelope.error.as_ref().unwrap().code,
            "lease_conflict"
        );

        let wrong_holder = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "lease",
                "release",
                "--state-dir",
                temp.path().to_str().unwrap(),
                "--holder",
                "lab",
            ],
            true,
        );
        assert_eq!(wrong_holder.exit_code(), 3);
        assert_eq!(
            wrong_holder.envelope.error.as_ref().unwrap().code,
            "lease_holder_mismatch"
        );

        let wrong_id = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "lease",
                "release",
                "--state-dir",
                temp.path().to_str().unwrap(),
                "--holder",
                "scheduler",
                "--lease-id",
                "other",
            ],
            true,
        );
        assert_eq!(wrong_id.exit_code(), 3);
        assert_eq!(
            wrong_id.envelope.error.as_ref().unwrap().code,
            "lease_id_mismatch"
        );

        let released = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "lease",
                "release",
                "--state-dir",
                temp.path().to_str().unwrap(),
                "--holder",
                "scheduler",
                "--lease-id",
                "lease-1",
            ],
            true,
        );
        assert_eq!(released.exit_code(), 0);
        assert_eq!(
            released
                .envelope
                .data
                .as_ref()
                .unwrap()
                .get("status")
                .and_then(Value::as_str),
            Some("released")
        );
    }

    #[test]
    fn session_lease_preempt_records_previous_holder() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let _ = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "lease",
                "acquire",
                "--state-dir",
                temp.path().to_str().unwrap(),
                "--holder",
                "scheduler",
                "--lease-id",
                "scheduler-lease",
            ],
            true,
        );
        let preempted = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "lease",
                "preempt",
                "--state-dir",
                temp.path().to_str().unwrap(),
                "--holder",
                "lab",
                "--lease-id",
                "lab-lease",
            ],
            true,
        );

        assert_eq!(preempted.exit_code(), 0);
        let data = preempted.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/lease/holder").and_then(Value::as_str),
            Some("lab")
        );
        assert_eq!(
            data.pointer("/lease/previous/holder")
                .and_then(Value::as_str),
            Some("scheduler")
        );
        assert_eq!(
            data.pointer("/previous/lease_id").and_then(Value::as_str),
            Some("scheduler-lease")
        );
    }

    #[test]
    fn session_record_start_status_and_stop_write_context() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
                "ak",
                "session",
                "record",
                "stop",
                "--state-dir",
                state_dir.to_str().unwrap(),
            ],
            true,
        );
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
            Some("ak")
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
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
                "ak",
                "record",
                "stop",
                "--state-dir",
                state_dir.to_str().unwrap(),
            ],
            true,
        );
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let build = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "record",
                "build-task",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--out",
                temp.path().join("draft").to_str().unwrap(),
            ],
            true,
        );
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

        assert_eq!(build.exit_code(), 3);
        assert_eq!(
            build.envelope.error.as_ref().unwrap().code,
            "record_session_not_active"
        );
    }

    #[test]
    fn stream_command_is_reserved_not_unknown() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let stream = run_cli(["--json", "--instance", "ak", "stream"], true);
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

        assert_eq!(stream.exit_code(), 6);
        assert_eq!(
            stream.envelope.error.as_ref().unwrap().code,
            "stream_not_implemented"
        );
    }

    #[test]
    fn session_record_active_start_requires_force() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let first = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
                "ak",
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
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
                "ak",
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
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
                "ak",
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
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
                "ak",
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
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let artifact_dir = temp.path().join("artifacts");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let contrast_path = temp.path().join("contrast.png");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        fs::write(&contrast_path, test_contrast_record_frame_png(12, 10)).unwrap();
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let artifact_dir = temp.path().join("artifacts");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
        let _guard = ENV_LOCK.lock().unwrap();
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
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
        let _guard = ENV_LOCK.lock().unwrap();
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
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
                "ak",
                "session",
                "record",
                "candidates",
                "home-anchor",
                "--state-dir",
                state_dir.to_str().unwrap(),
            ],
            true,
        );
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        fs::write(
            &frame_path,
            test_auto_region_discrimination_frame_png(false),
        )
        .unwrap();
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
                "ak",
                "session",
                "record",
                "candidates",
                "home-color",
                "--state-dir",
                state_dir.to_str().unwrap(),
            ],
            true,
        );
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
                "ak",
                "session",
                "record",
                "candidates",
                "home-anchor",
                "--state-dir",
                state_dir.to_str().unwrap(),
            ],
            true,
        );
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let out = temp.path().join("draft");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
                "ak",
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
                "ak",
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
                "ak",
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
                "ak",
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
                "ak",
                "session",
                "record",
                "build-task",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
                "--game",
                "arknights",
                "--server",
                "cn",
            ],
            true,
        );
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
        assert_eq!(data.get("operation_count").and_then(Value::as_u64), Some(1));
        assert_eq!(
            data.pointer("/bundle/schema_version")
                .and_then(Value::as_str),
            Some("0.3")
        );
        assert_eq!(
            data.pointer("/bundle/task_id").and_then(Value::as_str),
            Some("daily-check")
        );
        assert_eq!(
            data.pointer("/bundle/game").and_then(Value::as_str),
            Some("arknights")
        );
        assert_eq!(
            data.pointer("/bundle/server_scope/0")
                .and_then(Value::as_str),
            Some("cn")
        );
        assert_eq!(
            data.pointer("/bundle/coordinate_space/width")
                .and_then(Value::as_u64),
            Some(12)
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
    }

    #[test]
    fn session_record_build_task_rejects_deferred_color_probe() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let out = temp.path().join("draft");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
                "ak",
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
                "ak",
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
                "ak",
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
                "ak",
                "session",
                "record",
                "build-task",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
                "--game",
                "arknights",
                "--server",
                "cn",
            ],
            true,
        );
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let out = temp.path().join("draft");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
                "ak",
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
                "ak",
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
                "ak",
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
                "ak",
                "session",
                "record",
                "build-task",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
                "--game",
                "arknights",
                "--server",
                "cn",
            ],
            true,
        );
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
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
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
                "ak",
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
                "ak",
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
                "ak",
                "session",
                "record",
                "promote",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--repo",
                repo.to_str().unwrap(),
                "--game",
                "arknights",
                "--server",
                "cn",
            ],
            true,
        );
        let reject = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "promote",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--repo",
                repo.to_str().unwrap(),
                "--game",
                "arknights",
                "--server",
                "cn",
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
                "ak",
                "session",
                "record",
                "promote",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--repo",
                repo.to_str().unwrap(),
                "--game",
                "arknights",
                "--server",
                "cn",
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
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
        assert!(ours.join("operations/daily-check/task.json").is_file());
        assert!(
            ours.join("operations/daily-check/assets/anchor-home-anchor-page_home.png")
                .is_file()
        );
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
    }

    #[test]
    fn session_record_build_task_rejects_unresolved_target_click() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let out = temp.path().join("draft");
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
                "ak",
                "session",
                "record",
                "build-task",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
                "--game",
                "arknights",
                "--server",
                "cn",
                "--resolution",
                "1280x720",
            ],
            true,
        );
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let out = temp.path().join("draft");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
                "ak",
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
                "ak",
                "session",
                "record",
                "build-task",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
                "--game",
                "arknights",
                "--server",
                "cn",
            ],
            true,
        );
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let out = temp.path().join("draft");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
                "ak",
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
                "ak",
                "session",
                "record",
                "build-task",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
                "--game",
                "arknights",
                "--server",
                "cn",
            ],
            true,
        );
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let result = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

        assert_eq!(result.exit_code(), 3);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "record_session_not_active"
        );
    }

    #[test]
    fn session_record_step_rejects_duplicate_step_id() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
                "ak",
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
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
                "ak",
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
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let artifact_dir = temp.path().join("artifacts");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
                "ak",
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
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
        let _guard = ENV_LOCK.lock().unwrap();
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
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
                "ak",
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
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
                "ak",
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
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
                "ak",
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
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

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
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
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
                "ak",
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
                "ak",
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
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        assert_eq!(amend.exit_code(), 2);
        assert_eq!(
            amend.envelope.error.as_ref().unwrap().code,
            "validation_failed"
        );
    }

    #[test]
    fn session_record_start_requires_task_id() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let result = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
            ],
            true,
        );
        unsafe {
            env::remove_var(CONFIG_ENV);
        }

        assert_eq!(result.exit_code(), 2);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "validation_failed"
        );
    }

    #[test]
    fn session_control_request_requires_lease_metadata() {
        let temp = TempDir::new().unwrap();
        let request = SessionCommandRequest {
            request_id: "request-1".to_string(),
            command: "tap".to_string(),
            global: SessionCommandGlobal {
                instance: Some("ak".to_string()),
                game: None,
                server: None,
                resource_root: None,
                capture_backend: None,
                dry_run: false,
            },
            args: vec!["100".to_string(), "200".to_string()],
            lease: None,
            created_at_unix_ms: 1,
        };

        let err = execute_session_command_request_inner(&request, temp.path()).unwrap_err();
        assert_eq!(err.code, "lab_lease_required");
        assert_eq!(err.exit_code(), 3);
    }

    #[test]
    fn session_app_request_requires_lease_before_device_io() {
        let temp = TempDir::new().unwrap();
        let request = SessionCommandRequest {
            request_id: "request-1".to_string(),
            command: "app".to_string(),
            global: SessionCommandGlobal {
                instance: Some("ak".to_string()),
                game: None,
                server: None,
                resource_root: None,
                capture_backend: None,
                dry_run: false,
            },
            args: vec![
                "launch".to_string(),
                "--package".to_string(),
                "com.example.game".to_string(),
            ],
            lease: None,
            created_at_unix_ms: 1,
        };

        let err = execute_session_command_request_inner(&request, temp.path()).unwrap_err();
        assert_eq!(err.code, "lab_lease_required");
        assert_eq!(err.exit_code(), 3);
    }

    #[test]
    fn session_control_request_rejects_wrong_holder_before_device_io() {
        let temp = TempDir::new().unwrap();
        let lease = new_session_lease(
            "ak".to_string(),
            "scheduler".to_string(),
            Some("lease-1".to_string()),
            false,
            None,
        );
        write_json_file_atomic(&session_lease_path(temp.path(), "ak"), &lease).unwrap();
        let request = SessionCommandRequest {
            request_id: "request-1".to_string(),
            command: "tap".to_string(),
            global: SessionCommandGlobal {
                instance: Some("ak".to_string()),
                game: None,
                server: None,
                resource_root: None,
                capture_backend: None,
                dry_run: false,
            },
            args: vec!["100".to_string(), "200".to_string()],
            lease: Some(SessionCommandLease {
                holder: "lab".to_string(),
                lease_id: Some("lease-1".to_string()),
            }),
            created_at_unix_ms: 1,
        };

        let err = execute_session_command_request_inner(&request, temp.path()).unwrap_err();
        assert_eq!(err.code, "lease_holder_mismatch");
        assert_eq!(err.exit_code(), 3);
    }

    #[test]
    fn session_control_request_rejects_wrong_lease_id_before_device_io() {
        let temp = TempDir::new().unwrap();
        let lease = new_session_lease(
            "ak".to_string(),
            "scheduler".to_string(),
            Some("lease-1".to_string()),
            false,
            None,
        );
        write_json_file_atomic(&session_lease_path(temp.path(), "ak"), &lease).unwrap();
        let request = SessionCommandRequest {
            request_id: "request-1".to_string(),
            command: "key".to_string(),
            global: SessionCommandGlobal {
                instance: Some("ak".to_string()),
                game: None,
                server: None,
                resource_root: None,
                capture_backend: None,
                dry_run: false,
            },
            args: vec!["back".to_string()],
            lease: Some(SessionCommandLease {
                holder: "scheduler".to_string(),
                lease_id: Some("lease-2".to_string()),
            }),
            created_at_unix_ms: 1,
        };

        let err = execute_session_command_request_inner(&request, temp.path()).unwrap_err();
        assert_eq!(err.code, "lease_id_mismatch");
        assert_eq!(err.exit_code(), 3);
    }

    #[test]
    fn session_monitor_recover_request_requires_lease_metadata() {
        let temp = TempDir::new().unwrap();
        let request = SessionCommandRequest {
            request_id: "request-1".to_string(),
            command: "monitor".to_string(),
            global: SessionCommandGlobal {
                instance: Some("ak".to_string()),
                game: None,
                server: None,
                resource_root: None,
                capture_backend: None,
                dry_run: false,
            },
            args: vec!["--recover".to_string(), "--capture".to_string()],
            lease: None,
            created_at_unix_ms: 1,
        };

        let err = execute_session_command_request_inner(&request, temp.path()).unwrap_err();
        assert_eq!(err.code, "lab_lease_required");
        assert_eq!(err.exit_code(), 3);
    }

    #[test]
    fn session_monitor_recover_request_rejects_wrong_holder_before_recovery() {
        let temp = TempDir::new().unwrap();
        let lease = new_session_lease(
            "ak".to_string(),
            "scheduler".to_string(),
            Some("lease-1".to_string()),
            false,
            None,
        );
        write_json_file_atomic(&session_lease_path(temp.path(), "ak"), &lease).unwrap();
        let request = SessionCommandRequest {
            request_id: "request-1".to_string(),
            command: "monitor".to_string(),
            global: SessionCommandGlobal {
                instance: Some("ak".to_string()),
                game: None,
                server: None,
                resource_root: None,
                capture_backend: None,
                dry_run: false,
            },
            args: vec!["--recover".to_string(), "--capture".to_string()],
            lease: Some(SessionCommandLease {
                holder: "lab".to_string(),
                lease_id: Some("lease-1".to_string()),
            }),
            created_at_unix_ms: 1,
        };

        let err = execute_session_command_request_inner(&request, temp.path()).unwrap_err();
        assert_eq!(err.code, "lease_holder_mismatch");
        assert_eq!(err.exit_code(), 3);
    }

    #[test]
    fn session_monitor_once_recover_request_stays_read_only() {
        let temp = TempDir::new().unwrap();
        let request = SessionCommandRequest {
            request_id: "request-1".to_string(),
            command: "monitor_once".to_string(),
            global: SessionCommandGlobal {
                instance: Some("ak".to_string()),
                game: None,
                server: None,
                resource_root: None,
                capture_backend: None,
                dry_run: false,
            },
            args: vec!["--recover".to_string(), "--capture".to_string()],
            lease: Some(SessionCommandLease {
                holder: "scheduler".to_string(),
                lease_id: Some("lease-1".to_string()),
            }),
            created_at_unix_ms: 1,
        };

        let err = execute_session_command_request_inner(&request, temp.path()).unwrap_err();
        assert_eq!(err.code, "daemon_recovery_requires_lease");
        assert_eq!(err.exit_code(), 3);
    }

    #[test]
    fn session_request_without_daemon_is_runtime_error() {
        let temp = TempDir::new().unwrap();
        let result = run_cli(
            [
                "--json",
                "session",
                "request",
                "capture-diagnose",
                "--state-dir",
                temp.path().to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 5);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "runtime_not_running"
        );
    }

    #[test]
    fn capture_diagnose_via_daemon_without_daemon_is_runtime_error() {
        let temp = TempDir::new().unwrap();
        let result = run_cli(
            [
                "--json",
                "capture",
                "diagnose",
                "--via-daemon",
                "--state-dir",
                temp.path().to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 5);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "runtime_not_running"
        );
    }

    #[test]
    fn readonly_via_daemon_without_daemon_is_runtime_error() {
        let temp = TempDir::new().unwrap();
        let result = run_cli(
            [
                "--json",
                "current-page",
                "--via-daemon",
                "--state-dir",
                temp.path().to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 5);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "runtime_not_running"
        );
    }

    #[test]
    fn direct_touch_via_daemon_accepts_lease_flags_before_daemon_lookup() {
        let temp = TempDir::new().unwrap();
        let result = run_cli(
            [
                "--json",
                "tap",
                "--via-daemon",
                "--state-dir",
                temp.path().to_str().unwrap(),
                "--lease-holder",
                "scheduler",
                "--lease-id",
                "lease-1",
                "100",
                "200",
            ],
            true,
        );

        assert_eq!(result.exit_code(), 5);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "runtime_not_running"
        );
    }

    #[test]
    fn session_app_via_daemon_accepts_lease_flags_before_daemon_lookup() {
        let temp = TempDir::new().unwrap();
        let result = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "app",
                "launch",
                "--via-daemon",
                "--state-dir",
                temp.path().to_str().unwrap(),
                "--lease-holder",
                "scheduler",
                "--lease-id",
                "lease-1",
                "--package",
                "com.example.game",
            ],
            true,
        );

        assert_eq!(result.exit_code(), 5);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "runtime_not_running"
        );
    }

    #[test]
    fn session_request_app_without_daemon_is_runtime_error() {
        let temp = TempDir::new().unwrap();
        let result = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "request",
                "app",
                "launch",
                "--state-dir",
                temp.path().to_str().unwrap(),
                "--lease-holder",
                "scheduler",
                "--lease-id",
                "lease-1",
                "--package",
                "com.example.game",
            ],
            true,
        );

        assert_eq!(result.exit_code(), 5);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "runtime_not_running"
        );
    }

    #[test]
    fn monitor_once_via_daemon_without_daemon_is_runtime_error() {
        let temp = TempDir::new().unwrap();
        let result = run_cli(
            [
                "--json",
                "monitor",
                "--once",
                "--via-daemon",
                "--state-dir",
                temp.path().to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 5);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "runtime_not_running"
        );
    }

    #[test]
    fn monitor_via_daemon_without_once_submits_request() {
        let temp = TempDir::new().unwrap();
        let result = run_cli(
            [
                "--json",
                "monitor",
                "--via-daemon",
                "--state-dir",
                temp.path().to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 5);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "runtime_not_running"
        );
    }

    #[test]
    fn monitor_via_daemon_recover_accepts_lease_flags_before_daemon_lookup() {
        let temp = TempDir::new().unwrap();
        let result = run_cli(
            [
                "--json",
                "monitor",
                "--via-daemon",
                "--recover",
                "--state-dir",
                temp.path().to_str().unwrap(),
                "--lease-holder",
                "scheduler",
                "--lease-id",
                "lease-1",
            ],
            true,
        );

        assert_eq!(result.exit_code(), 5);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "runtime_not_running"
        );
    }

    #[test]
    fn session_request_readonly_without_daemon_is_runtime_error() {
        let temp = TempDir::new().unwrap();
        let result = run_cli(
            [
                "--json",
                "session",
                "request",
                "is-visible",
                "--target",
                "arknights/home",
                "--state-dir",
                temp.path().to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 5);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "runtime_not_running"
        );
    }

    #[test]
    fn session_request_payload_strips_client_only_flags() {
        let args = [
            "--target".to_string(),
            "arknights/home".to_string(),
            "--via-daemon".to_string(),
            "--state-dir".to_string(),
            "target/session".to_string(),
            "--request-timeout-ms".to_string(),
            "15000".to_string(),
            "--lease-holder".to_string(),
            "scheduler".to_string(),
            "--lease-id".to_string(),
            "lease-1".to_string(),
            "--capture".to_string(),
        ];

        assert_eq!(
            session_request_payload_args(&args),
            vec![
                "--target".to_string(),
                "arknights/home".to_string(),
                "--capture".to_string()
            ]
        );
    }

    #[test]
    fn session_instance_list_reads_config() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        unsafe {
            env::set_var(CONFIG_ENV, &config);
        }
        let _ = run_cli(
            [
                "--json",
                "config",
                "set",
                "instance.azur.serial",
                "127.0.0.1:16385",
            ],
            true,
        );
        let _ = run_cli(
            ["--json", "config", "set", "instance.azur.game", "azurlane"],
            true,
        );
        let _ = run_cli(
            ["--json", "config", "set", "instance.azur.server", "jp"],
            true,
        );
        let result = run_cli(["--json", "session", "instance", "list"], true);
        unsafe {
            env::remove_var(CONFIG_ENV);
        }
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
        assert_eq!(instances[0].get("id").and_then(Value::as_str), Some("azur"));
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
            Some("reserved")
        );
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
    fn fresh_auto_probe_prefers_fast_backends_before_adb() {
        assert_eq!(
            fresh_probe_choices(CaptureBackendChoice::Auto),
            vec![
                CaptureBackendChoice::NemuIpc,
                CaptureBackendChoice::DroidcastRaw,
                CaptureBackendChoice::Adb,
            ]
        );
        assert_eq!(
            fresh_probe_choices(CaptureBackendChoice::Adb),
            vec![CaptureBackendChoice::Adb]
        );
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
    fn capture_diagnosis_unavailable_points_to_instance_health() {
        let recovery = capture_diagnosis_recovery_json(
            CaptureFreshProbeStatus::Unavailable,
            CaptureBackendChoice::Auto,
        );
        assert_eq!(
            recovery.get("available").and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            recovery
                .pointer("/recommendations/0/command")
                .and_then(Value::as_str),
            Some("session instance health")
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
            "session instance",
            "session app",
            "session app launch",
            "session app stop",
            "session app restart",
            "session capture",
            "session capture diagnose",
            "session request capture-diagnose",
            "session request recognize",
            "session request detect-page",
            "session request current-page",
            "session request is-visible",
            "session request locate",
            "session request monitor-once",
            "session request app",
            "session lease",
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
    }

    #[test]
    fn package_validate_rejects_zip_slip() {
        let temp = TempDir::new().unwrap();
        let zip = temp.path().join("bundle.zip");
        write_test_zip(
            &zip,
            &[
                ("module/manifest.json", br#"{}"#.as_slice()),
                ("module/operations/task/task.json", br#"{}"#.as_slice()),
                ("module/../escape.json", br#"{}"#.as_slice()),
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
        assert_eq!(result.exit_code(), 2);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "package_invalid"
        );
    }

    #[test]
    fn package_validate_rejects_executable_entry() {
        let temp = TempDir::new().unwrap();
        let zip = temp.path().join("bundle.zip");
        write_test_zip(
            &zip,
            &[
                ("module/manifest.json", br#"{}"#.as_slice()),
                ("module/operations/task/task.json", br#"{}"#.as_slice()),
                ("module/tools/run.ps1", b"Write-Host no"),
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
        assert_eq!(result.exit_code(), 2);
        assert!(
            result
                .envelope
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("executable")
        );
    }

    #[test]
    fn package_validate_rejects_hash_mismatch() {
        let temp = TempDir::new().unwrap();
        let zip = temp.path().join("bundle.zip");
        write_test_zip(
            &zip,
            &[
                (
                    "module/manifest.json",
                    br#"{"hashes":{"operations/resources.json":"sha256:0000"}}"#.as_slice(),
                ),
                ("module/operations/task/task.json", br#"{}"#.as_slice()),
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
        assert_eq!(result.exit_code(), 2);
        assert!(
            result
                .envelope
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("hash mismatch")
        );
    }

    #[test]
    fn package_validate_rejects_unsafe_manifest_hash_path_without_echoing_traversal() {
        let temp = TempDir::new().unwrap();
        let zip = temp.path().join("bundle.zip");
        write_test_zip(
            &zip,
            &[
                (
                    "module/manifest.json",
                    br#"{"hashes":{"../outside.json":"sha256:0000"}}"#.as_slice(),
                ),
                ("module/operations/task/task.json", br#"{}"#.as_slice()),
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

        assert_eq!(result.exit_code(), 2);
        let message = &result.envelope.error.as_ref().unwrap().message;
        assert!(message.contains("manifest hash path is unsafe"));
        assert!(!message.contains(".."));
    }

    #[test]
    fn read_package_zip_entry_limited_rejects_oversized_entry() {
        let mut input = std::io::Cursor::new(vec![1, 2, 3]);

        let err = read_zip_entry_limited(&mut input, "module/large.bin", 2).expect_err("oversized");

        assert_eq!(err.code, "package_invalid");
        assert!(err.message.contains("exceeds 2 bytes"));
    }

    #[test]
    fn list_resource_kind_unknown_returns_usage_error() {
        let temp = TempDir::new().unwrap();

        let err = list_resource_kind(temp.path(), "future-kind").expect_err("unknown kind");

        assert_eq!(err.code, "validation_failed");
        assert!(err.message.contains("unknown list kind"));
    }

    #[test]
    fn detect_page_returns_standby_when_no_page_matches() {
        let temp = TempDir::new().unwrap();
        let pack = temp.path().join("pack.json");
        let pages = temp.path().join("pages.json");
        let scene = temp.path().join("scene.png");
        fs::write(
            &pack,
            r#"{
                "schema_version":"0.3",
                "coordinate_space":{"width":1,"height":1},
                "targets":[{"type":"color","id":"home","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]}]
            }"#,
        )
        .unwrap();
        fs::write(
            &pages,
            r#"{"schema_version":"0.3","pages":[{"id":"home","required":["home"]}]}"#,
        )
        .unwrap();
        fs::write(&scene, encode_png(1, 1, [0, 0, 255])).unwrap();
        let result = run_cli(
            [
                "--json",
                "detect-page",
                "--pack",
                pack.to_str().unwrap(),
                "--pack-root",
                temp.path().to_str().unwrap(),
                "--pages",
                pages.to_str().unwrap(),
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
            Some("standby")
        );
    }

    #[test]
    fn detect_page_resolves_pack_from_resource_root_and_game_alias() {
        let temp = TempDir::new().unwrap();
        let recognition = temp.path().join("recognition");
        fs::create_dir(&recognition).unwrap();
        let pack = recognition.join("arknights.cn.pack.json");
        let pages = recognition.join("arknights.cn.pages.json");
        let scene = temp.path().join("scene.png");
        fs::write(
            &pack,
            r#"{
                "schema_version":"0.3",
                "coordinate_space":{"width":1,"height":1},
                "targets":[{"type":"color","id":"home","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]}]
            }"#,
        )
        .unwrap();
        fs::write(
            &pages,
            r#"{"schema_version":"0.3","pages":[{"id":"home","required":["home"]}]}"#,
        )
        .unwrap();
        fs::write(&scene, encode_png(1, 1, [0, 0, 255])).unwrap();
        let result = run_cli(
            [
                "--json",
                "--resource-root",
                temp.path().to_str().unwrap(),
                "--game",
                "ark",
                "detect-page",
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
            Some("standby")
        );
    }

    #[test]
    fn detect_page_accepts_reorganized_repo_root_resource_root() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let repo = temp.path().join("repo");
        let ours = repo.join("ours");
        let recognition = ours.join("recognition");
        let operations = ours.join("operations");
        fs::create_dir_all(&recognition).unwrap();
        fs::create_dir_all(&operations).unwrap();
        let pack = recognition.join("arknights.cn.pack.json");
        let pages = recognition.join("arknights.cn.pages.json");
        let scene = temp.path().join("scene.png");
        fs::write(
            &pack,
            r#"{
                "schema_version":"0.3",
                "coordinate_space":{"width":1,"height":1},
                "targets":[{"type":"color","id":"home","region":{"x":0,"y":0,"width":1,"height":1},"expected":[0,0,255]}]
            }"#,
        )
        .unwrap();
        fs::write(
            &pages,
            r#"{"schema_version":"0.3","pages":[{"id":"home","required":["home"]}]}"#,
        )
        .unwrap();
        fs::write(&scene, encode_png(1, 1, [0, 0, 255])).unwrap();

        let result = run_cli(
            [
                "--json",
                "--resource-root",
                repo.to_str().unwrap(),
                "--game",
                "ark",
                "detect-page",
                "--scene",
                scene.to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0, "{:?}", result.envelope.error);
        assert_eq!(
            result
                .envelope
                .data
                .as_ref()
                .unwrap()
                .get("page")
                .and_then(Value::as_str),
            Some("home")
        );
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
    fn bare_instance_argument_is_used_as_adb_serial_without_config_entry() {
        let global = GlobalOptions {
            instance: Some("127.0.0.1:16416".to_string()),
            ..Default::default()
        };
        let config = UserConfig::default();
        let resolved = device_config(&global, &config).expect("device config");
        assert_eq!(resolved.target.serial.as_deref(), Some("127.0.0.1:16416"));
    }

    #[test]
    fn current_page_resolves_semantic_page() {
        let temp = semantic_resource_root(false);
        let scene = temp.path().join("home.png");
        fs::write(&scene, encode_png(1, 1, [255, 0, 0])).unwrap();

        let result = run_cli(
            [
                "--json",
                "--resource-root",
                temp.path().to_str().unwrap(),
                "--game",
                "ark",
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
            Some("arknights/home")
        );
    }

    #[test]
    fn tap_target_dry_run_requires_visible_target_and_returns_point() {
        let temp = semantic_resource_root(false);
        let scene = temp.path().join("home.png");
        fs::write(&scene, encode_png(1, 1, [255, 0, 0])).unwrap();

        let result = run_cli(
            [
                "--json",
                "--resource-root",
                temp.path().to_str().unwrap(),
                "--game",
                "ark",
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

        let result = run_cli(
            [
                "--json",
                "--dry-run",
                "--resource-root",
                temp.path().to_str().unwrap(),
                "--game",
                "ark",
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
    fn navigate_blocks_destructive_overlap_by_default() {
        let temp = semantic_resource_root(true);
        let scene = temp.path().join("home.png");
        fs::write(&scene, encode_png(1, 1, [255, 0, 0])).unwrap();

        let result = run_cli(
            [
                "--json",
                "--dry-run",
                "--resource-root",
                temp.path().to_str().unwrap(),
                "--game",
                "ark",
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
                "ark",
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
                "ark",
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
                "ark",
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
                "ark",
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
                "ark",
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
                "ark",
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
    fn monitor_once_reports_healthy_expected_page() {
        let temp = semantic_resource_root(false);
        let scene = temp.path().join("home.png");
        fs::write(&scene, encode_png(1, 1, [255, 0, 0])).unwrap();

        let result = run_cli(
            [
                "--json",
                "--resource-root",
                temp.path().to_str().unwrap(),
                "--game",
                "ark",
                "monitor",
                "--once",
                "--scene",
                scene.to_str().unwrap(),
                "--expect",
                "home",
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0);
        let data = result.envelope.data.as_ref().unwrap();
        assert_eq!(data.get("status").and_then(Value::as_str), Some("healthy"));
        assert_eq!(
            data.get("recovery")
                .and_then(|value| value.get("needed"))
                .and_then(Value::as_bool),
            Some(false)
        );
    }

    #[test]
    fn monitor_once_reports_standby_wake_recovery() {
        let temp = semantic_resource_root(false);
        let scene = temp.path().join("standby.png");
        fs::write(&scene, encode_png(1, 1, [1, 1, 1])).unwrap();

        let result = run_cli(
            [
                "--json",
                "--resource-root",
                temp.path().to_str().unwrap(),
                "--game",
                "ark",
                "monitor",
                "--once",
                "--scene",
                scene.to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0);
        let data = result.envelope.data.as_ref().unwrap();
        assert_eq!(data.get("status").and_then(Value::as_str), Some("standby"));
        let recovery = data.get("recovery").unwrap();
        assert_eq!(
            recovery.get("available").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            recovery
                .get("steps")
                .and_then(Value::as_array)
                .and_then(|steps| steps.first())
                .and_then(|step| step.get("type"))
                .and_then(Value::as_str),
            Some("wake")
        );
    }

    #[test]
    fn monitor_once_reports_unexpected_page_route() {
        let temp = semantic_resource_root(false);
        let scene = temp.path().join("target.png");
        fs::write(&scene, encode_png(1, 1, [0, 0, 255])).unwrap();

        let result = run_cli(
            [
                "--json",
                "--resource-root",
                temp.path().to_str().unwrap(),
                "--game",
                "ark",
                "monitor",
                "--once",
                "--scene",
                scene.to_str().unwrap(),
                "--expect",
                "home",
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0);
        let data = result.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.get("status").and_then(Value::as_str),
            Some("unexpected_page")
        );
        let route = data
            .get("recovery")
            .and_then(|value| value.get("route"))
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(
            route[0].get("id").and_then(Value::as_str),
            Some("target_to_home")
        );
    }

    #[test]
    fn monitor_loop_reports_bounded_read_only_iterations() {
        let temp = semantic_resource_root(false);
        let scene = temp.path().join("home.png");
        fs::write(&scene, encode_png(1, 1, [255, 0, 0])).unwrap();

        let result = run_cli(
            [
                "--json",
                "--resource-root",
                temp.path().to_str().unwrap(),
                "--game",
                "ark",
                "monitor",
                "--max-iterations",
                "2",
                "--interval-ms",
                "0",
                "--scene",
                scene.to_str().unwrap(),
                "--expect",
                "home",
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0);
        let data = result.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.get("mode").and_then(Value::as_str),
            Some("monitor_loop")
        );
        assert_eq!(data.get("read_only").and_then(Value::as_bool), Some(true));
        let iterations = data.get("iterations").and_then(Value::as_array).unwrap();
        assert_eq!(iterations.len(), 2);
        assert!(iterations.iter().all(|iteration| {
            iteration
                .pointer("/diagnosis/status")
                .and_then(Value::as_str)
                == Some("healthy")
                && iteration.get("recovery").is_some_and(Value::is_null)
        }));
    }

    #[test]
    fn monitor_loop_recover_dry_run_uses_session_recover_path() {
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
                "ark",
                "monitor",
                "--max-iterations",
                "1",
                "--interval-ms",
                "0",
                "--recover",
                "--scene",
                scene.to_str().unwrap(),
                "--expect",
                "home",
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0, "{:?}", result.envelope.error);
        let iteration = result
            .envelope
            .data
            .as_ref()
            .unwrap()
            .get("iterations")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .unwrap();
        assert_eq!(
            iteration
                .pointer("/diagnosis/status")
                .and_then(Value::as_str),
            Some("standby")
        );
        assert_eq!(
            iteration
                .pointer("/recovery/status")
                .and_then(Value::as_str),
            Some("planned")
        );
        assert_eq!(
            iteration
                .pointer("/recovery/executed")
                .and_then(Value::as_bool),
            Some(false)
        );
    }

    #[test]
    fn monitor_loop_recover_without_capture_fails_loud_for_real_execution() {
        let temp = semantic_resource_root(false);
        let scene = temp.path().join("standby.png");
        fs::write(&scene, encode_png(1, 1, [1, 1, 1])).unwrap();

        let result = run_cli(
            [
                "--json",
                "--resource-root",
                temp.path().to_str().unwrap(),
                "--game",
                "ark",
                "monitor",
                "--max-iterations",
                "1",
                "--interval-ms",
                "0",
                "--recover",
                "--scene",
                scene.to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 2);
        let error = result.envelope.error.as_ref().unwrap();
        assert_eq!(error.code, "validation_failed");
        assert!(error.message.contains("requires --capture"));
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

    fn semantic_resource_root(include_destructive_overlap: bool) -> TempDir {
        let temp = TempDir::new().unwrap();
        let recognition = temp.path().join("recognition");
        let navigation = temp.path().join("navigation");
        fs::create_dir(&recognition).unwrap();
        fs::create_dir(&navigation).unwrap();
        fs::write(
            recognition.join("arknights.cn.pack.json"),
            r#"{
                "schema_version":"0.3",
                "coordinate_space":{"width":1,"height":1},
                "targets":[
                    {"type":"color","id":"home_anchor","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]},
                    {"type":"color","id":"target_anchor","region":{"x":0,"y":0,"width":1,"height":1},"expected":[0,0,255]},
                    {"type":"color","id":"home_button","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0],"click":{"x":10,"y":20,"width":4,"height":6}}
                ]
            }"#,
        )
        .unwrap();
        fs::write(
            recognition.join("arknights.cn.pages.json"),
            r#"{
                "schema_version":"0.3",
                "pages":[
                    {"id":"arknights/home","required":["home_anchor"]},
                    {"id":"arknights/target","required":["target_anchor"]}
                ]
            }"#,
        )
        .unwrap();
        let destructive = if include_destructive_overlap {
            r#"[{"id":"delete","click":{"kind":"rect","x":10,"y":20,"width":4,"height":6}}]"#
        } else {
            "[]"
        };
        fs::write(
            navigation.join("arknights.cn.navigation.json"),
            format!(
                r#"{{
                    "schema_version":"0.3",
                    "game":"arknights",
                    "server":"cn",
                    "control_points":[{{"name":"wake","point":[3,4],"note":"test wake"}}],
                    "navigation":[{{
                        "id":"home_to_target",
                        "from_page":"arknights/home",
                        "to_page":"arknights/target",
                        "click":{{"kind":"rect","x":10,"y":20,"width":4,"height":6}}
                    }},
                    {{
                        "id":"target_to_home",
                        "from_page":"arknights/target",
                        "to_page":"arknights/home",
                        "click":{{"kind":"point","point":"2,3"}}
                    }}],
                    "destructive_actions":{destructive}
                }}"#
            ),
        )
        .unwrap();
        temp
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
